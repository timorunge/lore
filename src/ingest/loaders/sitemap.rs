use std::collections::HashMap;

use anyhow::{Context, Result};
use futures::stream::StreamExt;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use tracing::{info, warn};

use crate::ingest::discover::MAX_DISCOVERED_URLS;
use crate::ingest::loaders::cap_urls;
use crate::net::{FetchRequest, Fetcher};
use crate::util::progress::ProgressHandle;

const MAX_SUB_SITEMAPS: usize = 500;

/// Parse sitemap XML and return `(is_index, urls)`, where `is_index` indicates a sitemap index.
fn parse_sitemap_urls(xml: &str) -> (bool, Vec<String>) {
    let mut reader = Reader::from_str(xml);
    let mut urls = Vec::new();
    let mut is_index = false;
    let mut in_loc = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                let local = e.local_name();
                if local.as_ref() == b"sitemapindex" {
                    is_index = true;
                }
                if local.as_ref() == b"loc" {
                    in_loc = true;
                }
            }
            Ok(Event::Text(e)) if in_loc => {
                if let Ok(text) = e.decode() {
                    let url = text.trim().to_owned();
                    if !url.is_empty() {
                        urls.push(url);
                    }
                }
                in_loc = false;
            }
            Ok(Event::CData(e)) if in_loc => {
                if let Ok(text) = std::str::from_utf8(e.as_ref()) {
                    let url = text.trim().to_owned();
                    if !url.is_empty() {
                        urls.push(url);
                    }
                }
                in_loc = false;
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == b"loc" => {
                in_loc = false;
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                warn!("sitemap XML parse error: {e}");
                break;
            }
            _ => {}
        }
    }

    (is_index, urls)
}

/// Discover URLs from a sitemap. Returns `Ok(None)` when the sitemap returned
/// 304 Not Modified (caller should preserve previously-indexed URLs), or
/// `Ok(Some(urls))` on success.
pub(crate) async fn discover_urls(
    fetcher: &Fetcher,
    sitemap_url: &str,
    include: Option<&regex::Regex>,
    extra_headers: Option<&HashMap<String, String>>,
    progress: &ProgressHandle,
) -> Result<Option<Vec<String>>> {
    progress.inc_length(1);
    let resp = fetcher
        .fetch(&FetchRequest {
            url: sitemap_url,
            extra_headers,
            ..Default::default()
        })
        .await
        .context("failed to fetch sitemap")?;
    // 304 Not Modified -- signal the caller to keep the existing URL set.
    let Some(resp) = resp else {
        progress.inc(1);
        return Ok(None);
    };

    let xml = String::from_utf8_lossy(&resp).into_owned();
    let (is_index, mut urls) = parse_sitemap_urls(&xml);

    let page_urls = if is_index {
        cap_urls(&mut urls, MAX_SUB_SITEMAPS, "sitemap index sub-sitemaps");
        progress.inc_length(urls.len() as u64);
        progress.inc(1);
        let concurrency = fetcher.config.concurrency;
        let mut all = Vec::new();
        let futs: Vec<_> = urls
            .iter()
            .map(|sub_url| async move {
                let sub_req = FetchRequest {
                    url: sub_url,
                    extra_headers,
                    ..Default::default()
                };
                match fetcher.fetch(&sub_req).await {
                    Ok(Some(sub_resp)) => {
                        let (sub_is_index, sub_urls) =
                            parse_sitemap_urls(&String::from_utf8_lossy(&sub_resp));
                        if sub_is_index {
                            warn!(
                                url = sub_url.as_str(),
                                "nested sitemap index detected, skipping"
                            );
                            Vec::new()
                        } else {
                            sub_urls
                        }
                    }
                    Ok(None) => Vec::new(),
                    Err(e) => {
                        warn!(url = sub_url.as_str(), "failed to fetch sub-sitemap: {e}");
                        Vec::new()
                    }
                }
            })
            .collect();
        let mut stream = futures::stream::iter(futs).buffer_unordered(concurrency);
        while let Some(sub_urls) = stream.next().await {
            progress.inc(1);
            all.extend(sub_urls);
            if all.len() >= MAX_DISCOVERED_URLS {
                cap_urls(&mut all, MAX_DISCOVERED_URLS, "sitemap URL count");
                break;
            }
        }
        all
    } else {
        progress.inc(1);
        cap_urls(&mut urls, MAX_DISCOVERED_URLS, "sitemap URL count");
        urls
    };

    let filtered: Vec<String> = page_urls
        .into_iter()
        .filter(|u| crate::util::is_http_url(u))
        .filter(|u| include.as_ref().is_none_or(|re| re.is_match(u)))
        .collect();

    info!(
        sitemap = sitemap_url,
        urls = filtered.len(),
        "discovered URLs from sitemap"
    );
    Ok(Some(filtered))
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[tokio::test]
    async fn sitemap_cases() {
        let server = MockServer::start().await;
        // robots.txt: 404 so the fetcher treats this origin as "allow all"
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/sitemap.xml"))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;

        let fetcher = Fetcher::for_test();
        let url = format!("{}/sitemap.xml", server.uri());
        let result = discover_urls(&fetcher, &url, None, None, &ProgressHandle::noop())
            .await
            .expect("discover_urls should not error on 304");
        assert!(
            result.is_none(),
            "304 response should yield Ok(None), preserving existing URLs"
        );

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.com/page1</loc></url>
  <url><loc>https://example.com/page2</loc></url>
</urlset>"#;
        let (is_index, urls) = parse_sitemap_urls(xml);
        assert!(!is_index);
        assert_eq!(
            urls,
            vec!["https://example.com/page1", "https://example.com/page2"]
        );

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <sitemap><loc>https://example.com/sitemap1.xml</loc></sitemap>
  <sitemap><loc>https://example.com/sitemap2.xml</loc></sitemap>
</sitemapindex>"#;
        let (is_index, urls) = parse_sitemap_urls(xml);
        assert!(is_index);
        assert_eq!(
            urls,
            vec![
                "https://example.com/sitemap1.xml",
                "https://example.com/sitemap2.xml"
            ]
        );

        // Error recovery: non-XML garbage should produce empty output, not panic.
        let (_, urls) = parse_sitemap_urls("not valid xml at all <><>");
        assert!(urls.is_empty());
    }
}
