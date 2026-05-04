use std::sync::Arc;

use anyhow::{Context, Result};
use futures::StreamExt;
use tracing::warn;
use url::Url;

use crate::net::client::{DEFAULT_USER_AGENT, Fetcher, parse_http_url};

/// Maximum robots.txt response size (1 MB).
const ROBOTS_MAX_BYTES: u64 = 1024 * 1024;

impl Fetcher {
    /// Check robots.txt for the given URL.
    ///
    /// Returns `Ok(())` if the URL is allowed (or robots.txt is absent / returns 4xx).
    /// Returns `Err` when robots.txt is unavailable (5xx or network error), exceeds
    /// the size limit, or explicitly disallows access to the URL.
    pub(super) async fn check_robots(&self, url: &Url) -> Result<()> {
        if !self.config.respect_robots {
            return Ok(());
        }

        let origin = url.origin().ascii_serialization();

        // Check-lock-check: avoid holding the mutex across HTTP I/O.
        let needs_fetch = {
            let cache = self.robots.lock().await;
            !cache.contains_key(&origin)
        };

        if needs_fetch {
            let robots_url = format!("{origin}/robots.txt");
            let ua = self
                .config
                .user_agent
                .as_deref()
                .unwrap_or(DEFAULT_USER_AGENT);
            let robot = match self.fetch_raw(&robots_url).await {
                Ok(resp) if resp.status().is_success() => {
                    if resp
                        .content_length()
                        .is_some_and(|cl| cl > ROBOTS_MAX_BYTES)
                    {
                        warn!(url = %robots_url, "robots.txt too large, blocking this request");
                        self.robots
                            .lock()
                            .await
                            .entry(origin.clone())
                            .or_insert(None);
                        anyhow::bail!("robots.txt too large, skipping {url}");
                    }
                    let mut bytes = Vec::new();
                    let mut stream = resp.bytes_stream();
                    while let Some(chunk) = stream.next().await {
                        let chunk = chunk.with_context(|| {
                            format!("failed to read robots.txt body from {robots_url}")
                        })?;
                        bytes.extend_from_slice(&chunk);
                        if bytes.len() as u64 > ROBOTS_MAX_BYTES {
                            warn!(url = %robots_url, "robots.txt body exceeds limit during streaming");
                            self.robots
                                .lock()
                                .await
                                .entry(origin.clone())
                                .or_insert(None);
                            anyhow::bail!("robots.txt too large, skipping {url}");
                        }
                    }
                    let body = String::from_utf8_lossy(&bytes);
                    texting_robots::Robot::new(ua, body.as_bytes())
                        .ok()
                        .map(Arc::new)
                }
                // 4xx = no robots.txt or access denied -> treat as "allow all"
                Ok(resp) if resp.status().is_client_error() => None,
                // Server errors or network failures -> cache None so subsequent
                // requests for the same origin skip the re-fetch.
                Ok(resp) => {
                    warn!(url = %robots_url, status = %resp.status(), "robots.txt fetch failed, blocking this request");
                    self.robots
                        .lock()
                        .await
                        .entry(origin.clone())
                        .or_insert(None);
                    anyhow::bail!(
                        "robots.txt unavailable (HTTP {}), skipping {url}",
                        resp.status()
                    );
                }
                Err(e) => {
                    warn!(url = %robots_url, "robots.txt fetch error: {e}, blocking this request");
                    self.robots
                        .lock()
                        .await
                        .entry(origin.clone())
                        .or_insert(None);
                    anyhow::bail!("robots.txt unavailable ({e}), skipping {url}");
                }
            };
            // Re-acquire lock and insert.
            // Race window: two concurrent callers may both observe `needs_fetch = true`
            // for the same origin and both fetch robots.txt. The second writer's result
            // is silently discarded by `entry().or_insert()`, so the first writer wins.
            // This is benign: both fetches produce equivalent results from the same URL,
            // and the redundant request is rare (only on the first request per origin
            // when concurrency is high).
            self.robots
                .lock()
                .await
                .entry(origin.clone())
                .or_insert(robot);
        }

        let cache = self.robots.lock().await;
        if let Some(Some(robot)) = cache.get(&origin)
            && !robot.allowed(url.as_str())
        {
            anyhow::bail!("blocked by robots.txt: {url}");
        }

        Ok(())
    }

    /// Perform a rate-limited HTTP GET without a robots.txt check.
    /// Skipping the robots check is intentional: this is the primitive used
    /// to *fetch* robots.txt, so checking it here would recurse infinitely.
    async fn fetch_raw(&self, url: &str) -> Result<reqwest::Response> {
        let parsed = parse_http_url(url)?;
        // _rate_permit is held across the request to enforce per-host concurrency.
        let (host, _rate_permit) = self.rate_limit(&parsed).await?;
        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("HTTP request failed")?;
        self.mark_request_done(&host).await;
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::net::client::{Fetcher, parse_http_url};

    async fn setup(respect_robots: bool) -> (MockServer, Fetcher) {
        let server = MockServer::start().await;
        let mut fetcher = Fetcher::for_test();
        fetcher.config.respect_robots = respect_robots;
        (server, fetcher)
    }

    // 404 response -> no robots.txt present -> all URLs allowed.
    // Disallow rule -> matching URL is blocked with an appropriate error.
    #[tokio::test]
    async fn robots_policy_cases() {
        // 404: allow all.
        {
            let (server, fetcher) = setup(true).await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            let parsed = parse_http_url(&format!("{}/anything", server.uri())).unwrap();
            fetcher
                .check_robots(&parsed)
                .await
                .expect("404 robots.txt should allow all URLs");
        }

        // Disallow rule: block matching URL.
        {
            let (server, fetcher) = setup(true).await;
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string("User-agent: *\nDisallow: /private/"),
                )
                .mount(&server)
                .await;
            let parsed = parse_http_url(&format!("{}/private/secret.html", server.uri())).unwrap();
            let err = fetcher
                .check_robots(&parsed)
                .await
                .expect_err("should block disallowed URL");
            assert!(
                err.to_string().contains("blocked by robots.txt"),
                "error should mention robots.txt: {err}"
            );
        }
    }

    // Disabled config skips the fetch entirely.
    // 5xx response is cached so subsequent requests skip the re-fetch.
    #[tokio::test]
    async fn robots_fetch_behavior() {
        // respect_robots=false: no HTTP requests made.
        {
            let (server, fetcher) = setup(false).await;
            let parsed = parse_http_url(&format!("{}/page", server.uri())).unwrap();
            fetcher
                .check_robots(&parsed)
                .await
                .expect("respect_robots=false should skip check");
            let requests = server.received_requests().await.unwrap();
            assert_eq!(
                requests.len(),
                0,
                "no HTTP requests should be made when robots is disabled"
            );
        }

        // 5xx: first call blocks, second call uses cache (no second fetch).
        {
            let (server, fetcher) = setup(true).await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(500))
                .mount(&server)
                .await;
            let parsed = parse_http_url(&format!("{}/some/page", server.uri())).unwrap();
            let first = fetcher.check_robots(&parsed).await;
            assert!(
                first.is_err(),
                "first 5xx robots.txt should block the request"
            );
            let second = fetcher.check_robots(&parsed).await;
            assert!(
                second.is_ok(),
                "cached 5xx result should allow subsequent requests"
            );
            let requests = server.received_requests().await.unwrap();
            assert_eq!(
                requests.len(),
                1,
                "expected exactly 1 robots.txt fetch, got {}",
                requests.len()
            );
        }
    }
}
