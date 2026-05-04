use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use serde::Serialize;
use tracing::warn;

use crate::config::{FetchConfig, SourceConfig, UpdateMode};
use crate::net::{FetchRequest, Fetcher};
use crate::types::{DocMeta, SourceId, SourceType, StampsMeta, source_id};

const HEAD_MARKER: &str = "__HEAD__";

/// Per-source diff result from a remote status check.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteDiff {
    pub source_label: String,
    #[serde(rename = "type")]
    pub source_type: SourceType,
    pub added: usize,
    pub changed: usize,
    pub deleted: usize,
    pub unchanged: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RemoteDiff {
    fn unchanged(label: String, source_type: SourceType) -> Self {
        Self {
            source_label: label,
            source_type,
            added: 0,
            changed: 0,
            deleted: 0,
            unchanged: 0,
            error: None,
        }
    }

    fn error(label: String, source_type: SourceType, error: String) -> Self {
        Self {
            source_label: label,
            source_type,
            added: 0,
            changed: 0,
            deleted: 0,
            unchanged: 0,
            error: Some(error),
        }
    }

    pub fn has_changes(&self) -> bool {
        self.added > 0 || self.changed > 0 || self.deleted > 0
    }
}

/// Query all non-local sources for changes without downloading content.
pub async fn check_remote_sources(
    sources: &[SourceConfig],
    all_docs: &HashMap<SourceId, DocMeta>,
    all_stamps: &HashMap<SourceId, StampsMeta>,
    fetch_config: &FetchConfig,
) -> Vec<RemoteDiff> {
    let fetcher = match Fetcher::new(fetch_config) {
        Ok(f) => Arc::new(f),
        Err(e) => {
            warn!("failed to create HTTP client for remote checks: {e}");
            return Vec::new();
        }
    };

    let futures: FuturesUnordered<_> = sources
        .iter()
        .filter(|s| !matches!(s, SourceConfig::Local(_)) && s.update() != UpdateMode::Never)
        .flat_map(|source| {
            let fetcher = &fetcher;
            dispatch_source(source, all_docs, all_stamps, fetcher)
        })
        .collect();

    let mut results = Vec::new();
    let mut futures = futures;
    while let Some(diffs) = futures.next().await {
        results.extend(diffs);
    }
    results
}

type BoxFut<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = Vec<RemoteDiff>> + Send + 'a>>;

fn dispatch_source<'a>(
    source: &'a SourceConfig,
    all_docs: &'a HashMap<SourceId, DocMeta>,
    all_stamps: &'a HashMap<SourceId, StampsMeta>,
    fetcher: &'a Arc<Fetcher>,
) -> Vec<BoxFut<'a>> {
    match source {
        SourceConfig::Local(_) => vec![],
        SourceConfig::Git(s) => s
            .git
            .iter()
            .map(|url| {
                let git_ref = s.git_ref.as_deref();
                Box::pin(async move { vec![check_git(url, git_ref, all_stamps).await] }) as BoxFut
            })
            .collect(),
        SourceConfig::Url(s) => {
            let headers = &s.headers;
            vec![
                Box::pin(async move { check_urls(&s.url, headers, all_stamps, fetcher).await })
                    as BoxFut,
            ]
        }
        SourceConfig::Sitemap(s) => s
            .sitemap
            .iter()
            .map(|url| {
                let include = s.include.as_deref();
                let headers = &s.headers;
                Box::pin(async move {
                    vec![check_sitemap(url, include, headers, all_docs, fetcher).await]
                }) as BoxFut
            })
            .collect(),
        SourceConfig::Feed(s) => s
            .feed
            .iter()
            .map(|url| {
                let include = s.include.as_deref();
                let headers = &s.headers;
                Box::pin(
                    async move { vec![check_feed(url, include, headers, all_docs, fetcher).await] },
                ) as BoxFut
            })
            .collect(),
        #[cfg(feature = "s3")]
        SourceConfig::S3(s) => {
            s.s3.iter()
                .map(|uri| {
                    let glob = s.glob.as_deref();
                    let include = s.include.as_deref();
                    Box::pin(async move {
                        vec![check_s3(uri, glob, include, all_docs, all_stamps).await]
                    }) as BoxFut
                })
                .collect()
        }
        #[cfg(not(feature = "s3"))]
        SourceConfig::S3(_) => vec![],
        SourceConfig::Youtube(s) => s
            .youtube
            .iter()
            .map(|url| {
                let max = s.max_videos;
                Box::pin(async move { vec![check_youtube(url, max, all_docs).await] }) as BoxFut
            })
            .collect(),
        SourceConfig::Maildir(_) => vec![],
        SourceConfig::Exec(_) => vec![],
        SourceConfig::Mcp(_) => vec![],
    }
}

async fn check_git(
    url: &str,
    git_ref: Option<&str>,
    all_stamps: &HashMap<SourceId, StampsMeta>,
) -> RemoteDiff {
    let label = url.to_owned();
    let marker_key = format!("{url}#{HEAD_MARKER}");
    let stored_sha = all_stamps
        .get(source_id(&marker_key).as_str())
        .map(|s| s.content_hash.to_lowercase());

    let timeout = crate::config::ProcessingConfig::default().extraction_timeout_secs;
    match crate::ingest::loaders::git::git_ls_remote(url, git_ref, timeout).await {
        Ok(remote_sha) => {
            let remote_lower = remote_sha.to_lowercase();
            if stored_sha.as_deref() == Some(remote_lower.as_str()) {
                let mut d = RemoteDiff::unchanged(label, SourceType::Git);
                d.unchanged = 1;
                d
            } else {
                RemoteDiff {
                    source_label: label,
                    source_type: SourceType::Git,
                    added: 0,
                    changed: 1,
                    deleted: 0,
                    unchanged: 0,
                    error: None,
                }
            }
        }
        Err(e) => RemoteDiff::error(label, SourceType::Git, format!("{e:#}")),
    }
}

async fn check_urls(
    urls: &[String],
    headers: &HashMap<String, String>,
    all_stamps: &HashMap<SourceId, StampsMeta>,
    fetcher: &Fetcher,
) -> Vec<RemoteDiff> {
    let futs: FuturesUnordered<_> = urls
        .iter()
        .map(|url| async {
            let stamps = all_stamps.get(source_id(url).as_str());
            let req = FetchRequest {
                url,
                etag: stamps.and_then(|s| s.etag.as_deref()),
                last_modified: stamps.and_then(|s| s.last_modified.as_deref()),
                extra_headers: if headers.is_empty() {
                    None
                } else {
                    Some(headers)
                },
            };
            let label = url.clone();
            let is_new = stamps.is_none();
            match fetcher.fetch_head(&req).await {
                Ok(None) => RemoteDiff {
                    source_label: label,
                    source_type: SourceType::Url,
                    added: 0,
                    changed: 0,
                    deleted: 0,
                    unchanged: 1,
                    error: None,
                },
                Ok(Some(true)) => {
                    let mut d = RemoteDiff::unchanged(label, SourceType::Url);
                    if is_new {
                        d.added = 1;
                    } else {
                        d.changed = 1;
                    }
                    d
                }
                Ok(Some(false)) => {
                    let mut d = RemoteDiff::unchanged(label, SourceType::Url);
                    if !is_new {
                        d.deleted = 1;
                    }
                    d
                }
                Err(e) => RemoteDiff::error(label, SourceType::Url, format!("{e:#}")),
            }
        })
        .collect();

    let mut results = Vec::new();
    let mut futs = futs;
    while let Some(diff) = futs.next().await {
        results.push(diff);
    }
    results
}

async fn check_sitemap(
    url: &str,
    include: Option<&str>,
    headers: &HashMap<String, String>,
    all_docs: &HashMap<SourceId, DocMeta>,
    fetcher: &Fetcher,
) -> RemoteDiff {
    let label = url.to_owned();
    let include_re = match crate::ingest::compile_include_re(include) {
        Ok(re) => re,
        Err(e) => return RemoteDiff::error(label, SourceType::Sitemap, format!("{e:#}")),
    };
    let extra_headers = if headers.is_empty() {
        None
    } else {
        Some(headers)
    };
    let progress = crate::util::progress::ProgressHandle::noop();
    match crate::ingest::loaders::sitemap::discover_urls(
        fetcher,
        url,
        include_re.as_ref(),
        extra_headers,
        &progress,
    )
    .await
    {
        Ok(None) => {
            let stored = count_docs_by_origin(all_docs, SourceType::Sitemap);
            let mut d = RemoteDiff::unchanged(label, SourceType::Sitemap);
            d.unchanged = stored;
            d
        }
        Ok(Some(remote_urls)) => diff_url_set(&label, SourceType::Sitemap, &remote_urls, all_docs),
        Err(e) => RemoteDiff::error(label, SourceType::Sitemap, format!("{e:#}")),
    }
}

async fn check_feed(
    url: &str,
    include: Option<&str>,
    headers: &HashMap<String, String>,
    all_docs: &HashMap<SourceId, DocMeta>,
    fetcher: &Fetcher,
) -> RemoteDiff {
    let label = url.to_owned();
    let include_re = match crate::ingest::compile_include_re(include) {
        Ok(re) => re,
        Err(e) => return RemoteDiff::error(label, SourceType::Feed, format!("{e:#}")),
    };
    let req = FetchRequest {
        url,
        etag: None,
        last_modified: None,
        extra_headers: if headers.is_empty() {
            None
        } else {
            Some(headers)
        },
    };
    match fetcher.fetch(&req).await {
        Ok(None) => {
            let stored = count_docs_by_origin(all_docs, SourceType::Feed);
            let mut d = RemoteDiff::unchanged(label, SourceType::Feed);
            d.unchanged = stored;
            d
        }
        Ok(Some(body)) => {
            let xml = String::from_utf8_lossy(&body);
            let mut remote_urls = crate::ingest::loaders::feed::parse_feed_urls(&xml);
            if let Some(re) = &include_re {
                remote_urls.retain(|u| re.is_match(u));
            }
            diff_url_set(&label, SourceType::Feed, &remote_urls, all_docs)
        }
        Err(e) => RemoteDiff::error(label, SourceType::Feed, format!("{e:#}")),
    }
}

#[cfg(feature = "s3")]
async fn check_s3(
    uri: &str,
    glob: Option<&str>,
    include: Option<&str>,
    all_docs: &HashMap<SourceId, DocMeta>,
    all_stamps: &HashMap<SourceId, StampsMeta>,
) -> RemoteDiff {
    let label = uri.to_owned();
    let existing_etags: HashMap<SourceId, Option<String>> = all_stamps
        .iter()
        .map(|(k, v)| (k.clone(), v.etag.clone()))
        .collect();
    let progress = crate::util::progress::ProgressHandle::noop();
    match crate::ingest::loaders::s3::discover(uri, glob, include, &existing_etags, &progress).await
    {
        Ok(discovered) => {
            let new_or_changed = discovered.objects.len();
            let remote_keys: HashSet<String> = discovered
                .objects
                .iter()
                .map(|o| format!("s3://{}/{}", discovered.bucket, o.key))
                .collect();
            let stored_keys: HashSet<&str> = all_docs
                .values()
                .filter(|d| d.origin == SourceType::S3)
                .map(|d| d.source.as_str())
                .collect();
            let deleted = stored_keys
                .iter()
                .filter(|k| !remote_keys.contains(**k))
                .count();
            let previously_stored = stored_keys.len();
            let unchanged = previously_stored
                .saturating_sub(new_or_changed)
                .saturating_sub(deleted);
            let added = new_or_changed
                .saturating_sub(previously_stored.saturating_sub(unchanged + deleted));
            RemoteDiff {
                source_label: label,
                source_type: SourceType::S3,
                added,
                changed: new_or_changed.saturating_sub(added),
                deleted,
                unchanged,
                error: None,
            }
        }
        Err(e) => RemoteDiff::error(label, SourceType::S3, format!("{e:#}")),
    }
}

async fn check_youtube(
    url: &str,
    max_videos: usize,
    all_docs: &HashMap<SourceId, DocMeta>,
) -> RemoteDiff {
    let label = url.to_owned();
    match crate::ingest::loaders::youtube::parse_youtube_url(url) {
        Ok(crate::ingest::loaders::youtube::YoutubeTarget::Video(id)) => {
            let source_key = format!("https://www.youtube.com/watch?v={id}");
            let sid = source_id(&source_key);
            if all_docs.contains_key(&sid) {
                let mut d = RemoteDiff::unchanged(label, SourceType::Youtube);
                d.unchanged = 1;
                d
            } else {
                RemoteDiff {
                    source_label: label,
                    source_type: SourceType::Youtube,
                    added: 1,
                    changed: 0,
                    deleted: 0,
                    unchanged: 0,
                    error: None,
                }
            }
        }
        Ok(_) => {
            let timeout = crate::config::ProcessingConfig::default().extraction_timeout_secs;
            match crate::ingest::loaders::youtube::ytdlp_flat_playlist(url, max_videos, timeout)
                .await
            {
                Ok(video_ids) => {
                    let remote_sources: HashSet<String> = video_ids
                        .iter()
                        .map(|id| format!("https://www.youtube.com/watch?v={id}"))
                        .collect();
                    diff_video_set(&label, &remote_sources, all_docs)
                }
                Err(e) => RemoteDiff::error(label, SourceType::Youtube, format!("{e:#}")),
            }
        }
        Err(e) => RemoteDiff::error(label, SourceType::Youtube, format!("{e:#}")),
    }
}

fn diff_url_set(
    label: &str,
    origin: SourceType,
    remote_urls: &[String],
    all_docs: &HashMap<SourceId, DocMeta>,
) -> RemoteDiff {
    let stored_sources: HashSet<&str> = all_docs
        .values()
        .filter(|d| d.origin == origin)
        .map(|d| d.source.as_str())
        .collect();

    let remote_set: HashSet<&str> = remote_urls.iter().map(String::as_str).collect();

    let added = remote_set
        .iter()
        .filter(|u| !stored_sources.contains(**u))
        .count();
    let deleted = stored_sources
        .iter()
        .filter(|u| !remote_set.contains(**u))
        .count();
    let unchanged = stored_sources.len().saturating_sub(deleted);

    RemoteDiff {
        source_label: label.to_owned(),
        source_type: origin,
        added,
        changed: 0,
        deleted,
        unchanged,
        error: None,
    }
}

fn diff_video_set(
    label: &str,
    remote_sources: &HashSet<String>,
    all_docs: &HashMap<SourceId, DocMeta>,
) -> RemoteDiff {
    let stored_sources: HashSet<&str> = all_docs
        .values()
        .filter(|d| d.origin == SourceType::Youtube)
        .map(|d| d.source.as_str())
        .collect();

    let added = remote_sources
        .iter()
        .filter(|u| !stored_sources.contains(u.as_str()))
        .count();
    let deleted = stored_sources
        .iter()
        .filter(|u| !remote_sources.contains(**u))
        .count();
    let unchanged = stored_sources.len().saturating_sub(deleted);

    RemoteDiff {
        source_label: label.to_owned(),
        source_type: SourceType::Youtube,
        added,
        changed: 0,
        deleted,
        unchanged,
        error: None,
    }
}

fn count_docs_by_origin(all_docs: &HashMap<SourceId, DocMeta>, origin: SourceType) -> usize {
    all_docs.values().filter(|d| d.origin == origin).count()
}
