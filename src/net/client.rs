use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures::StreamExt;
use reqwest::{
    Client, Response, StatusCode,
    header::{
        ACCEPT, HeaderMap, HeaderName, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH, USER_AGENT,
    },
};
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, warn};
use url::Url;

use crate::cache::http::{
    DownloadResult, cache_get_sync, cache_paths, cache_touch_sync, cache_write_async,
    download_cache_get,
};
use crate::config::FetchConfig;
use crate::net::ssrf::SafeResolver;
use crate::util::progress::ProgressHandle;

pub(super) const DEFAULT_USER_AGENT: &str = env!("LORE_USER_AGENT");
const MAX_REDIRECTS: usize = 10;
const RETRY_BACKOFF_INITIAL_SECS: f64 = 5.0;
const RETRY_BACKOFF_MAX_SECS: f64 = 120.0;
/// Maximum per-host rate-limit entries before pruning stale entries.
const RATE_LIMIT_MAP_MAX: usize = 1000;
/// Age threshold for pruning stale rate-limit map entries.
const RATE_LIMIT_PRUNE_SECS: u64 = 3600;

/// Parameters for a single HTTP fetch request, including optional caching hints.
#[derive(Default)]
pub(crate) struct FetchRequest<'a> {
    pub(crate) url: &'a str,
    pub(crate) etag: Option<&'a str>,
    pub(crate) last_modified: Option<&'a str>,
    pub(crate) extra_headers: Option<&'a HashMap<String, String>>,
}

/// HTTP client with SSRF protection, per-host rate limiting, robots.txt enforcement, and caching.
pub(crate) struct Fetcher {
    pub(super) client: Client,
    download_client: Client,
    pub(crate) config: FetchConfig,
    max_response_bytes: u64,
    host_semaphores: Mutex<HashMap<String, Arc<Semaphore>>>,
    host_last_request: Mutex<HashMap<String, Instant>>,
    // NOTE: robots.txt is cached for the process lifetime with no TTL.
    // Sites that relax Disallow rules require a restart.
    pub(super) robots: Mutex<HashMap<String, Option<Arc<texting_robots::Robot>>>>,
}

impl Fetcher {
    pub(crate) fn new(config: &FetchConfig) -> Result<Self> {
        let ua = config.user_agent.as_deref().unwrap_or(DEFAULT_USER_AGENT);

        let default_headers = {
            let mut h = HeaderMap::new();
            h.insert(USER_AGENT, HeaderValue::from_str(ua)?);
            if config.prefer_markdown {
                h.insert(
                    ACCEPT,
                    HeaderValue::from_static("text/markdown, text/html;q=0.9, */*;q=0.8"),
                );
            }
            h
        };

        // Both clients use SafeResolver so private IPs are blocked at connect
        // time, closing the TOCTOU gap between pre-flight DNS and actual connect.
        let client = Client::builder()
            .timeout(Duration::from_secs_f64(config.timeout))
            .default_headers(default_headers.clone())
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .dns_resolver(Arc::new(SafeResolver))
            .build()
            .context("failed to build HTTP client")?;

        // Streaming downloads need a generous total timeout in addition to the
        // connect timeout. Without a total timeout a slow server can send data
        // at 1 byte/second forever. 10 minutes is long enough for large files
        // over slow connections but prevents indefinite hangs.
        let download_client = Client::builder()
            .connect_timeout(Duration::from_secs_f64(config.timeout))
            .timeout(Duration::from_secs(config.download_timeout))
            .default_headers(default_headers)
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .dns_resolver(Arc::new(SafeResolver))
            .build()
            .context("failed to build download HTTP client")?;

        Ok(Self {
            client,
            download_client,
            max_response_bytes: crate::fmt::mb_to_bytes(config.max_download_mb),
            config: config.clone(),
            host_semaphores: Mutex::new(HashMap::new()),
            host_last_request: Mutex::new(HashMap::new()),
            robots: Mutex::new(HashMap::new()),
        })
    }

    /// Acquire per-host semaphore permit and enforce inter-request delay.
    /// Returns the host string and the semaphore permit (must be held for the request duration).
    pub(super) async fn rate_limit(
        &self,
        parsed: &Url,
    ) -> Result<(String, tokio::sync::OwnedSemaphorePermit)> {
        let host = parsed.host_str().unwrap_or("unknown").to_owned();

        let sem = {
            let mut sems = self.host_semaphores.lock().await;
            sems.entry(host.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(self.config.concurrency)))
                .clone()
        };

        let permit = sem.acquire_owned().await?;

        let sleep_dur = {
            let last_req = self.host_last_request.lock().await;
            if let Some(last) = last_req.get(&host) {
                let elapsed = last.elapsed();
                let delay = Duration::from_secs_f64(self.config.delay);
                delay.saturating_sub(elapsed)
            } else {
                Duration::ZERO
            }
        };
        if !sleep_dur.is_zero() {
            tokio::time::sleep(sleep_dur).await;
        }

        Ok((host, permit))
    }

    /// Record that a request to the given host just completed.
    /// Called after the response is fully received so the delay is measured
    /// from completion, not start (avoids hammering slow servers).
    pub(super) async fn mark_request_done(&self, host: &str) {
        // Guarantee the timestamp is always recorded even if the caller is cancelled.
        // The rate-limit map is only held briefly for timestamp updates so
        // contention is minimal; correctness (accurate inter-request delay)
        // matters more than avoiding a brief wait.
        let mut last_req = self.host_last_request.lock().await;
        last_req.insert(host.to_owned(), Instant::now());
        // Prune stale entries when the map grows large to prevent unbounded
        // memory growth when crawling many distinct hosts.
        if last_req.len() > RATE_LIMIT_MAP_MAX {
            let prune_threshold = Duration::from_secs(RATE_LIMIT_PRUNE_SECS);
            last_req.retain(|_, ts| ts.elapsed() < prune_threshold);
        }
    }

    /// If the response is 429/503, sleep for the Retry-After duration and re-send the request.
    /// Retries up to MAX_RETRIES times with exponential backoff (starting at 5 s, capped at 120 s).
    /// If the Retry-After header is present its value takes precedence over the backoff schedule.
    ///
    /// Each retry releases the current per-host semaphore permit, sleeps the
    /// backoff, then re-acquires a permit (enforcing the inter-request delay)
    /// before sending the next request.
    ///
    /// Returns the final response and a held semaphore permit for any
    /// non-rate-limit status, or an error after exhausting all retries.
    async fn maybe_retry(
        &self,
        response: Response,
        url: &str,
        parsed: &Url,
        client: &reqwest::Client,
        opts: &FetchRequest<'_>,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<(Response, tokio::sync::OwnedSemaphorePermit)> {
        let mut response = response;
        let mut permit = permit;
        let mut backoff_secs = RETRY_BACKOFF_INITIAL_SECS;

        for attempt in 1..=self.config.max_retries {
            if response.status() != StatusCode::TOO_MANY_REQUESTS
                && response.status() != StatusCode::SERVICE_UNAVAILABLE
            {
                return Ok((response, permit));
            }
            let wait = parse_retry_after(&response)
                .unwrap_or(backoff_secs)
                .min(RETRY_BACKOFF_MAX_SECS);
            warn!(url, retry_after = wait, attempt, "rate limited, retrying");

            // Release the permit before sleeping -- don't hold a slot while backed off.
            drop(permit);
            tokio::time::sleep(Duration::from_secs_f64(wait)).await;
            backoff_secs = (backoff_secs * 2.0).min(RETRY_BACKOFF_MAX_SECS);

            // Re-acquire: gets a semaphore permit and enforces inter-request delay.
            let (host, new_permit) = self.rate_limit(parsed).await?;
            permit = new_permit;

            let retry_req = apply_headers(client.get(url), opts);
            response = retry_req.send().await?;
            self.mark_request_done(&host).await;
        }

        // Final check: if still rate-limited after all retries, bail.
        if response.status() == StatusCode::TOO_MANY_REQUESTS
            || response.status() == StatusCode::SERVICE_UNAVAILABLE
        {
            let max = self.config.max_retries;
            anyhow::bail!("rate limited after {max} retries: {url}");
        }

        Ok((response, permit))
    }

    /// Fetch a URL with SSRF protection, rate limiting, and optional caching.
    /// Returns None for 304 Not Modified.
    pub(crate) async fn fetch(&self, req: &FetchRequest<'_>) -> Result<Option<Vec<u8>>> {
        let url = req.url;
        let parsed = parse_http_url(url)?;

        self.check_robots(&parsed).await?;

        if let Some(ttl) = self.config.cache_ttl
            && ttl > 0.0
        {
            let url_owned = url.to_owned();
            match tokio::task::spawn_blocking(move || cache_get_sync(&url_owned, ttl)).await {
                Ok(Some(cached)) => {
                    debug!(url, "cache hit");
                    return Ok(Some(cached));
                }
                Err(e) => warn!(url, "cache read task panicked: {e}"),
                Ok(None) => {}
            }
        }

        let (host, rate_permit) = self.rate_limit(&parsed).await?;

        let http_req = apply_headers(self.client.get(url), req);
        let response = http_req.send().await.context("HTTP request failed")?;

        if response.status() == StatusCode::NOT_MODIFIED {
            if self.config.cache_ttl.is_some() {
                let url_owned = url.to_owned();
                if let Err(e) =
                    tokio::task::spawn_blocking(move || cache_touch_sync(&url_owned)).await
                {
                    warn!(url, "cache touch task panicked: {e}");
                }
            }
            return Ok(None);
        }

        let (response, _rate_permit) = self
            .maybe_retry(response, url, &parsed, &self.client, req, rate_permit)
            .await?;
        let result = self.process_response(url, response).await;
        self.mark_request_done(&host).await;
        result
    }

    /// Download a single URL with robots.txt check, caching, rate limiting, and retry logic.
    async fn download_inner(&self, req: &FetchRequest<'_>) -> Result<Option<DownloadResult>> {
        let url = req.url;
        let parsed = parse_http_url(url)?;

        self.check_robots(&parsed).await?;

        if let Some(ttl) = self.config.cache_ttl
            && ttl > 0.0
        {
            let url_owned = url.to_owned();
            let cached = tokio::task::spawn_blocking(move || download_cache_get(&url_owned, ttl))
                .await
                .context("cache check panicked")?;
            if let Some(dl) = cached {
                debug!(url, "download cache hit");
                return Ok(Some(dl));
            }
        }

        let (host, rate_permit) = self.rate_limit(&parsed).await?;

        let http_req = apply_headers(self.download_client.get(url), req);
        let response = http_req.send().await.context("HTTP request failed")?;

        if response.status() == StatusCode::NOT_MODIFIED {
            if self.config.cache_ttl.is_some() {
                let url_owned = url.to_owned();
                if let Err(e) =
                    tokio::task::spawn_blocking(move || cache_touch_sync(&url_owned)).await
                {
                    warn!(url, "cache touch task panicked: {e}");
                }
            }
            return Ok(None);
        }

        let (response, _rate_permit) = self
            .maybe_retry(
                response,
                url,
                &parsed,
                &self.download_client,
                req,
                rate_permit,
            )
            .await?;
        let result = self.process_download_response(url, response).await;
        self.mark_request_done(&host).await;
        result
    }

    /// Download multiple URLs to disk concurrently.
    /// Returns (url, DownloadResult) pairs. Failed URLs and 304s are skipped.
    /// If `fetch_pb` is provided, it is incremented once per completed download.
    pub(crate) async fn try_download_all(
        &self,
        requests: &[FetchRequest<'_>],
        fetch_pb: &ProgressHandle,
    ) -> Vec<(String, DownloadResult)> {
        let mut results = Vec::with_capacity(requests.len());

        let futs: Vec<_> = requests
            .iter()
            .map(|req| {
                let url = req.url.to_owned();
                async move {
                    let result = self.download_inner(req).await;
                    (url, result)
                }
            })
            .collect();
        let mut stream = futures::stream::iter(futs).buffer_unordered(self.config.concurrency);

        while let Some((url, outcome)) = stream.next().await {
            fetch_pb.inc(1);
            match outcome {
                Ok(Some(dl)) => results.push((url, dl)),
                Ok(None) => {} // 304
                Err(e) => {
                    warn!(url = url.as_str(), "download failed: {e}");
                }
            }
        }
        results
    }

    /// Issue an HTTP HEAD request with conditional headers to check if a URL has changed.
    /// Returns `Ok(None)` for 304 Not Modified, `Ok(Some(true))` for 2xx (changed),
    /// `Ok(Some(false))` for 404/410 (gone).
    pub(crate) async fn fetch_head(&self, req: &FetchRequest<'_>) -> Result<Option<bool>> {
        let url = req.url;
        let parsed = parse_http_url(url)?;
        self.check_robots(&parsed).await?;

        let (host, rate_permit) = self.rate_limit(&parsed).await?;
        let http_req = apply_headers(self.client.head(url), req);
        let response = http_req.send().await.context("HTTP HEAD request failed")?;
        self.mark_request_done(&host).await;

        let status = response.status();
        drop(rate_permit);

        if status == StatusCode::NOT_MODIFIED {
            return Ok(None);
        }
        if status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
            return Ok(Some(false));
        }
        if status.is_success() {
            return Ok(Some(true));
        }
        anyhow::bail!("HTTP HEAD {status} for {url}");
    }

    /// Stream response body directly to a cache file on disk.
    async fn process_download_response(
        &self,
        url: &str,
        response: Response,
    ) -> Result<Option<DownloadResult>> {
        let status = response.status();
        anyhow::ensure!(status.is_success(), "HTTP {status} for {url}");

        let resp_headers = response.headers().clone();

        // Reject before streaming if Content-Length already exceeds the cap.
        if let Some(cl) = response.content_length() {
            anyhow::ensure!(
                cl <= self.max_response_bytes,
                "download too large: {cl} bytes (max {})",
                self.max_response_bytes
            );
        }

        let header_str = |name: &str| -> Option<String> {
            resp_headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(std::borrow::ToOwned::to_owned)
        };
        let etag = header_str("etag");
        let last_modified = header_str("last-modified");
        let content_type = header_str("content-type");

        // Stream directly to disk
        let (_, body_path) = cache_paths(url).context("failed to get http cache dir")?;
        let tmp_path = body_path.with_extension("body.tmp");

        let mut file = tokio::fs::File::create(&tmp_path)
            .await
            .with_context(|| format!("failed to create download file: {}", tmp_path.display()))?;

        let mut bytes_written: u64 = 0;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("download stream error")?;
            bytes_written += chunk.len() as u64;
            anyhow::ensure!(
                bytes_written <= self.max_response_bytes,
                "download too large: exceeded {} bytes for {url}",
                self.max_response_bytes
            );
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        }
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        drop(file);

        tokio::fs::rename(&tmp_path, &body_path)
            .await
            .with_context(|| {
                format!(
                    "failed to rename {} to {}",
                    tmp_path.display(),
                    body_path.display()
                )
            })?;

        if let Some(ttl) = self.config.cache_ttl
            && ttl > 0.0
            && !has_no_store(&resp_headers)
        {
            // Fire-and-forget: cache write errors are non-fatal.
            cache_write_async(url, &resp_headers, None).await;
        }

        Ok(Some(DownloadResult {
            path: body_path,
            content_type,
            etag,
            last_modified,
        }))
    }

    /// Read a response body into memory, enforcing the size limit and writing to cache.
    async fn process_response(&self, url: &str, response: Response) -> Result<Option<Vec<u8>>> {
        let status = response.status();
        anyhow::ensure!(status.is_success(), "HTTP {status} for {url}");

        if let Some(cl) = response.content_length() {
            anyhow::ensure!(
                cl <= self.max_response_bytes,
                "response too large: {cl} bytes (max {})",
                self.max_response_bytes
            );
        }

        let resp_headers = response.headers().clone();

        let capacity = response
            .content_length()
            .unwrap_or(0)
            .min(self.max_response_bytes) as usize;
        let mut body = Vec::with_capacity(capacity);
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("response stream error")?;
            body.extend_from_slice(&chunk);
            anyhow::ensure!(
                body.len() as u64 <= self.max_response_bytes,
                "response body too large: {} bytes",
                body.len()
            );
        }

        if let Some(ttl) = self.config.cache_ttl
            && ttl > 0.0
            && !has_no_store(&resp_headers)
        {
            // Fire-and-forget: cache write errors are non-fatal.
            cache_write_async(url, &resp_headers, Some(&body)).await;
        }

        Ok(Some(body))
    }

    /// Build a `Fetcher` without SSRF protection, suitable for tests that
    /// talk to a local mock server on 127.0.0.1.
    ///
    // SAFETY: no `SafeResolver` -- acceptable because `#[cfg(test)]` is a
    // compile-time gate that cannot be active in release builds.
    #[cfg(test)]
    #[doc(hidden)]
    pub(crate) fn for_test() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("test client");
        Self {
            client: client.clone(),
            download_client: client,
            max_response_bytes: crate::fmt::mb_to_bytes(10),
            config: crate::config::FetchConfig::default(),
            host_semaphores: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            host_last_request: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            robots: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

/// Parse a URL and validate that it uses HTTP or HTTPS.
pub(super) fn parse_http_url(url: &str) -> Result<Url> {
    let parsed = Url::parse(url).context("invalid URL")?;
    let scheme = parsed.scheme();
    anyhow::ensure!(
        scheme == "http" || scheme == "https",
        "URL must use http or https (got {scheme})"
    );
    Ok(parsed)
}

/// Apply conditional and extra request headers from `opts` to the builder.
fn apply_headers(
    mut req: reqwest::RequestBuilder,
    opts: &FetchRequest<'_>,
) -> reqwest::RequestBuilder {
    if let Some(et) = opts.etag {
        req = req.header(IF_NONE_MATCH, et);
    }
    if let Some(lm) = opts.last_modified {
        req = req.header(IF_MODIFIED_SINCE, lm);
    }
    if let Some(hdrs) = opts.extra_headers {
        for (k, v) in hdrs {
            let Ok(name) = HeaderName::from_bytes(k.as_bytes()) else {
                warn!(header_name = k, "skipping extra header with invalid name");
                continue;
            };
            let Ok(value) = HeaderValue::from_str(v) else {
                warn!(header_name = k, "skipping extra header with invalid value");
                continue;
            };
            req = req.header(name, value);
        }
    }
    req
}

/// Return true if the response headers contain `Cache-Control: no-store`.
fn has_no_store(headers: &HeaderMap) -> bool {
    headers
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|cc| cc.split(',').any(|d| d.trim() == "no-store"))
}

/// Parse the `Retry-After` header as a delay in seconds (numeric or HTTP-date).
fn parse_retry_after(response: &Response) -> Option<f64> {
    let raw = response.headers().get("retry-after")?.to_str().ok()?;
    if let Ok(secs) = raw.parse::<f64>() {
        return Some(secs.max(0.0));
    }
    httpdate::parse_http_date(raw).ok().map(|t| {
        t.duration_since(std::time::SystemTime::now())
            .unwrap_or_default()
            .as_secs_f64()
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{FetchRequest, Fetcher, parse_http_url};

    // Two 429s then 200 -> succeeds on the third attempt.
    // All 429s -> exhausts MAX_RETRIES and returns an error naming the count and URL.
    #[tokio::test]
    async fn retry_cases() {
        // Succeeds after two 429s.
        {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
                .up_to_n_times(2)
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200))
                .mount(&server)
                .await;

            let fetcher = Fetcher::for_test();
            let url = server.uri();
            let parsed = parse_http_url(&url).unwrap();
            let opts = FetchRequest {
                url: &url,
                etag: None,
                last_modified: None,
                extra_headers: Some(&HashMap::new()),
            };

            let (_, permit) = fetcher.rate_limit(&parsed).await.unwrap();
            let initial = fetcher.client.get(&url).send().await.unwrap();
            let (response, _permit) = fetcher
                .maybe_retry(initial, &url, &parsed, &fetcher.client, &opts, permit)
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
        }

        // Exhausted retries return an error.
        {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
                .mount(&server)
                .await;

            let fetcher = Fetcher::for_test();
            let url = server.uri();
            let parsed = parse_http_url(&url).unwrap();
            let opts = FetchRequest {
                url: &url,
                etag: None,
                last_modified: None,
                extra_headers: None,
            };

            let (_, permit) = fetcher.rate_limit(&parsed).await.unwrap();
            let initial = fetcher.client.get(&url).send().await.unwrap();
            let err = fetcher
                .maybe_retry(initial, &url, &parsed, &fetcher.client, &opts, permit)
                .await
                .unwrap_err();
            let msg = err.to_string();
            let max_retries = crate::config::FetchConfig::default().max_retries;
            assert!(
                msg.contains(&max_retries.to_string()),
                "error should mention retry count; got: {msg}"
            );
            assert!(
                msg.contains(url.as_str()),
                "error should mention the URL; got: {msg}"
            );
        }
    }

    /// A non-rate-limit response (e.g. 404) should be returned immediately
    /// without any retries.
    #[tokio::test]
    async fn non_rate_limit_response_returned_immediately() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let fetcher = Fetcher::for_test();
        let url = server.uri();
        let parsed = parse_http_url(&url).unwrap();
        let opts = FetchRequest {
            url: &url,
            etag: None,
            last_modified: None,
            extra_headers: None,
        };

        let (_, permit) = fetcher.rate_limit(&parsed).await.unwrap();
        let initial = fetcher.client.get(&url).send().await.unwrap();
        let (response, _permit) = fetcher
            .maybe_retry(initial, &url, &parsed, &fetcher.client, &opts, permit)
            .await
            .unwrap();

        assert_eq!(response.status(), 404);
    }
}
