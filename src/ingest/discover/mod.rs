pub(crate) mod load;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::{debug, info};

use crate::config::{ExtractMode, ProcessingLimits, SourceConfig};
use crate::ingest::compile_include_re;
use crate::ingest::loaders;
use crate::ingest::types::{FailedDoc, LoaderResult};
use crate::net::{FetchRequest, Fetcher};
use crate::types::{DocMeta, SourceId, SourceType, StampsMeta};
use crate::util::progress::ProgressHandle;
use crate::util::relativize_path;

const HEAD_MARKER: &str = "__HEAD__";

/// Cap on URLs discovered from a single feed or sitemap to prevent runaway
/// memory use on pathological or malicious inputs.
pub(crate) const MAX_DISCOVERED_URLS: usize = 50_000;

/// Build the source key for a marker document (`{prefix}#{tag}`).
fn marker_key(prefix: &str, tag: &str) -> String {
    format!("{prefix}#{tag}")
}

/// Result of source discovery: items to load in batches + marker docs.
pub(crate) struct DiscoveredSource {
    pub(crate) items: SourceItems,
    /// Extra docs processed after all batches (e.g., git HEAD markers).
    pub(crate) extras: Vec<LoaderResult>,
    /// Documents that failed during loading (e.g., parse errors, extraction panics).
    pub(crate) load_failures: Vec<FailedDoc>,
}

impl DiscoveredSource {
    fn preloaded_with_failures(docs: Vec<LoaderResult>, failures: Vec<FailedDoc>) -> Self {
        Self {
            items: SourceItems::Preloaded(docs),
            extras: Vec::new(),
            load_failures: failures,
        }
    }
}

/// Items discovered from a source, ready for batched loading.
///
/// Each variant carries a different key space:
/// - `Files`: source keys are rewritten paths (relative or `{git_prefix}#path`)
/// - `Urls`: source keys are the raw URL strings
/// - `Preloaded`: source keys are whatever the loader produced (used by git, S3, YouTube, and single-file local sources)
pub(crate) enum SourceItems {
    /// File paths (local or git checkout). Loaded via `read_file` per batch.
    Files {
        paths: Vec<PathBuf>,
        topic: Option<String>,
        text_extensions: Option<Arc<Vec<String>>>,
        limits: ProcessingLimits,
        rewrite_base: PathBuf,
        /// When set, source keys are formatted as `{prefix}#{relative_path}`
        /// instead of just `{relative_path}`. Used for git sources.
        source_prefix: Option<String>,
        /// When set, overrides each doc's origin, etag, and last_modified.
        /// Used for git sources to stamp `(SourceType::Git, Some(head_commit), None)`.
        stamp: Option<(SourceType, Option<String>, Option<String>)>,
    },
    /// URL list (url, sitemap, feed). Downloaded to disk in batches.
    Urls {
        urls: Vec<String>,
        source_type: SourceType,
        topic: Option<String>,
        etags: HashMap<String, (Option<String>, Option<String>)>,
        limits: ProcessingLimits,
        headers: HashMap<String, String>,
    },
    /// Already loaded (small results). Single-batch fallback.
    Preloaded(Vec<LoaderResult>),
}

impl SourceItems {
    /// Total number of items available for batched loading.
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Files { paths, .. } => paths.len(),
            Self::Urls { urls, .. } => urls.len(),
            Self::Preloaded(v) => v.len(),
        }
    }

    /// Derive source identity hashes without loading content.
    pub(crate) fn source_keys(&self) -> Vec<SourceId> {
        match self {
            Self::Files {
                paths,
                rewrite_base,
                source_prefix,
                ..
            } => paths
                .iter()
                .map(|p| {
                    let rel = relativize_path(p, rewrite_base);
                    let key = match source_prefix {
                        Some(pfx) => format!("{pfx}#{rel}"),
                        None => rel,
                    };
                    crate::types::source_id(&key)
                })
                .collect(),
            Self::Urls { urls, .. } => urls.iter().map(|u| crate::types::source_id(u)).collect(),
            Self::Preloaded(docs) => docs.iter().map(|d| d.source_id.clone()).collect(),
        }
    }
}

/// Overwrite download-related metadata on loaded docs.
fn stamp_download_meta(
    docs: &mut [LoaderResult],
    source_type: SourceType,
    etag: Option<&str>,
    last_modified: Option<&str>,
) {
    for doc in docs {
        doc.origin = source_type;
        doc.etag = etag.map(str::to_owned);
        doc.last_modified = last_modified.map(str::to_owned);
        doc.mtime_ns = None;
        doc.size_bytes = None;
    }
}

/// Shared state threaded through per-source discovery helpers.
pub(crate) struct DiscoverCtx<'a> {
    pub(crate) fetcher: &'a Fetcher,
    pub(crate) existing_docs: Arc<HashMap<SourceId, DocMeta>>,
    pub(crate) existing_stamps: Arc<HashMap<SourceId, StampsMeta>>,
    pub(crate) cwd: &'a Path,
    pub(crate) progress: &'a ProgressHandle,
    pub(crate) extract: ExtractMode,
    pub(crate) force: bool,
    pub(crate) limits: ProcessingLimits,
    pub(crate) topic: Option<String>,
    pub(crate) text_ext: Option<Arc<Vec<String>>>,
}

/// Discover and prepare items from a single source configuration entry.
///
/// Source-level errors (bad URL, unreachable host, invalid git ref) abort the
/// entire source. Item-level errors (corrupted file, single 404) are logged
/// as warnings and skipped. This asymmetry surfaces configuration problems
/// loudly while being resilient to incidental content issues.
pub(crate) async fn discover_source(
    source: &SourceConfig,
    ctx: &DiscoverCtx<'_>,
) -> Result<DiscoveredSource> {
    match source {
        SourceConfig::Local(s) => discover_local(s, ctx).await,
        SourceConfig::Url(s) => Ok(discover_url(s, ctx)),
        SourceConfig::Git(s) => discover_git(s, ctx).await,
        SourceConfig::Sitemap(s) => discover_sitemap(s, ctx).await,
        SourceConfig::Feed(s) => discover_feed(s, ctx).await,
        #[cfg(feature = "s3")]
        SourceConfig::S3(s) => discover_s3(s, ctx).await,
        #[cfg(not(feature = "s3"))]
        SourceConfig::S3(_) => {
            anyhow::bail!(
                "S3 sources require the 's3' feature; recompile lore with: cargo install lore --features s3"
            )
        }
        SourceConfig::Youtube(s) => discover_youtube(s, ctx).await,
        SourceConfig::Maildir(s) => discover_maildir(s, ctx).await,
        SourceConfig::Exec(s) => discover_exec(s, ctx).await,
        #[cfg(feature = "mcp")]
        SourceConfig::Mcp(s) => discover_mcp(s, ctx).await,
        #[cfg(not(feature = "mcp"))]
        SourceConfig::Mcp(_) => {
            anyhow::bail!(
                "MCP sources require the 'mcp' feature; recompile lore with: cargo install lore --features mcp"
            )
        }
    }
}

/// Discover items from a local file-system source (files or directories).
async fn discover_local(
    s: &crate::config::LocalSource,
    ctx: &DiscoverCtx<'_>,
) -> Result<DiscoveredSource> {
    let pattern = s.glob.as_deref().unwrap_or("**/*");
    let mut all_preloaded: Vec<LoaderResult> = Vec::new();
    let mut all_paths: Vec<PathBuf> = Vec::new();
    let mut load_failures: Vec<FailedDoc> = Vec::new();

    ctx.progress.inc_length(s.path.len() as u64);
    for path_str in &s.path {
        let base = Path::new(path_str);

        if base.is_file() {
            let effective_stamps = if ctx.force {
                Arc::new(HashMap::new())
            } else {
                Arc::clone(&ctx.existing_stamps)
            };
            let load_ctx = load::LoadContext {
                limits: ctx.limits,
                extract: ctx.extract,
                fetch_pb: ProgressHandle::noop(),
                existing_stamps: effective_stamps,
                rewrite_base: ctx.cwd.to_path_buf(),
                topic: ctx.topic.clone(),
                text_ext: ctx.text_ext.clone(),
            };
            match load::load_file_or_archive(base, &load_ctx).await {
                Ok((mut docs, archive_failures)) => {
                    load_failures.extend(archive_failures);
                    for doc in &mut docs {
                        if let Some((archive, entry)) = doc.source.split_once('#') {
                            let rel = relativize_path(Path::new(archive), ctx.cwd);
                            doc.source = format!("{rel}#{entry}");
                        } else {
                            doc.source = relativize_path(Path::new(&doc.source), ctx.cwd);
                        }
                        doc.source_id = crate::types::source_id(&doc.source);
                    }
                    all_preloaded.extend(docs);
                }
                Err(e) => {
                    load_failures
                        .push(FailedDoc::new(base.display().to_string(), format!("{e:#}")));
                }
            }
            ctx.progress.inc(1);
            continue;
        }

        let paths = loaders::file::list_files(base, pattern, None).await?;
        all_paths.extend(paths);
        ctx.progress.inc(1);
    }

    if !all_preloaded.is_empty() && all_paths.is_empty() {
        return Ok(DiscoveredSource::preloaded_with_failures(
            all_preloaded,
            load_failures,
        ));
    }

    Ok(DiscoveredSource {
        items: SourceItems::Files {
            paths: all_paths,
            topic: ctx.topic.clone(),
            text_extensions: ctx.text_ext.clone(),
            limits: ctx.limits,
            rewrite_base: ctx.cwd.to_path_buf(),
            source_prefix: None,
            stamp: None,
        },
        extras: all_preloaded,
        load_failures,
    })
}

/// Build a `DiscoveredSource` from an explicit URL list.
fn discover_url(s: &crate::config::UrlSource, ctx: &DiscoverCtx<'_>) -> DiscoveredSource {
    let etags = build_etag_map(&s.url, &ctx.existing_stamps);
    DiscoveredSource {
        items: SourceItems::Urls {
            urls: s.url.clone(),
            source_type: SourceType::Url,
            topic: ctx.topic.clone(),
            etags,
            limits: ctx.limits,
            headers: s.headers.clone(),
        },
        extras: Vec::new(),
        load_failures: Vec::new(),
    }
}

/// Clone or fetch each git repository and load all matching files.
async fn discover_git(
    s: &crate::config::GitSource,
    ctx: &DiscoverCtx<'_>,
) -> Result<DiscoveredSource> {
    for url in &s.git {
        crate::config::validate_git_url(url)?;
    }
    if let Some(r) = &s.git_ref {
        crate::config::validate_git_ref(r)?;
    }

    let pattern = s.glob.as_deref().unwrap_or("**/*.md");

    // Single-URL fast path: return SourceItems::Files for streaming backpressure.
    if s.git.len() == 1 {
        return discover_git_single(&s.git[0], s.git_ref.as_deref(), pattern, ctx).await;
    }

    // Multi-URL fallback: each repo has a different rewrite_base, so we load
    // eagerly and return Preloaded.
    let mut all_docs: Vec<LoaderResult> = Vec::new();
    let mut total_failures: Vec<FailedDoc> = Vec::new();

    for url in &s.git {
        let (extras, docs, failures) =
            clone_and_load_git(url, s.git_ref.as_deref(), pattern, ctx).await?;
        all_docs.extend(extras);
        all_docs.extend(docs);
        total_failures.extend(failures);
    }
    Ok(DiscoveredSource::preloaded_with_failures(
        all_docs,
        total_failures,
    ))
}

/// Single-URL git: return `SourceItems::Files` so the streaming pipeline loads
/// files with backpressure instead of reading everything up front.
async fn discover_git_single(
    url: &str,
    git_ref: Option<&str>,
    pattern: &str,
    ctx: &DiscoverCtx<'_>,
) -> Result<DiscoveredSource> {
    info!(repo = url, "cloning/fetching git repository");
    let clone_path = loaders::git::clone_or_fetch(
        url,
        git_ref,
        ctx.limits.extraction_timeout_secs,
        ctx.progress,
    )
    .await?;
    let head_commit =
        loaders::git::get_head_commit(&clone_path, ctx.limits.extraction_timeout_secs)
            .await?
            .to_lowercase();

    let stored_head = ctx
        .existing_stamps
        .get(marker_key(url, HEAD_MARKER).as_str())
        .map(|f| f.content_hash.to_lowercase());
    if !ctx.force && stored_head.as_deref() == Some(head_commit.as_str()) {
        debug!(repo = url, head = %head_commit, "HEAD unchanged, skipping");
        return Ok(DiscoveredSource {
            items: SourceItems::Preloaded(Vec::new()),
            extras: Vec::new(),
            load_failures: Vec::new(),
        });
    }

    info!(repo = url, head = %head_commit, "loading files from git repo");
    let paths = loaders::file::list_files(&clone_path, pattern, None).await?;

    let marker = git_head_marker(url, &head_commit);

    Ok(DiscoveredSource {
        items: SourceItems::Files {
            paths,
            topic: ctx.topic.clone(),
            text_extensions: ctx.text_ext.clone(),
            limits: ctx.limits,
            rewrite_base: clone_path,
            source_prefix: Some(url.to_owned()),
            stamp: Some((SourceType::Git, Some(head_commit), None)),
        },
        extras: vec![marker],
        load_failures: Vec::new(),
    })
}

/// Clone/fetch a single git URL and eagerly read all files. Used by the multi-URL fallback path.
async fn clone_and_load_git(
    url: &str,
    git_ref: Option<&str>,
    pattern: &str,
    ctx: &DiscoverCtx<'_>,
) -> Result<(Vec<LoaderResult>, Vec<LoaderResult>, Vec<FailedDoc>)> {
    info!(repo = url, "cloning/fetching git repository");
    let clone_path = loaders::git::clone_or_fetch(
        url,
        git_ref,
        ctx.limits.extraction_timeout_secs,
        ctx.progress,
    )
    .await?;
    let head_commit =
        loaders::git::get_head_commit(&clone_path, ctx.limits.extraction_timeout_secs)
            .await?
            .to_lowercase();

    let stored_head = ctx
        .existing_stamps
        .get(marker_key(url, HEAD_MARKER).as_str())
        .map(|f| f.content_hash.to_lowercase());
    if !ctx.force && stored_head.as_deref() == Some(head_commit.as_str()) {
        debug!(repo = url, head = %head_commit, "HEAD unchanged, skipping");
        return Ok((Vec::new(), Vec::new(), Vec::new()));
    }

    info!(repo = url, head = %head_commit, "loading files from git repo");
    let paths = loaders::file::list_files(&clone_path, pattern, None).await?;
    ctx.progress.inc_length(paths.len() as u64);

    let (mut docs, failures) = load::read_files_parallel(
        paths,
        ctx.topic.as_deref(),
        ctx.text_ext.as_deref().map(Vec::as_slice),
        &ctx.limits,
        ctx.extract,
        ctx.progress,
    )
    .await?;

    for doc in &mut docs {
        let rel = Path::new(&doc.source)
            .strip_prefix(&clone_path)
            .map_or_else(|_| doc.source.clone(), |r| r.to_string_lossy().into_owned());
        doc.source = format!("{url}#{rel}");
        doc.source_id = crate::types::source_id(&doc.source);
    }
    stamp_download_meta(&mut docs, SourceType::Git, Some(&head_commit), None);

    let marker = git_head_marker(url, &head_commit);
    Ok((vec![marker], docs, failures))
}

/// Build a synthetic HEAD marker document for a git repo.
fn git_head_marker(url: &str, head_commit: &str) -> LoaderResult {
    let marker_source = marker_key(url, HEAD_MARKER);
    LoaderResult {
        source_id: crate::types::source_id(&marker_source),
        source: marker_source,
        origin: SourceType::Git,
        kind: crate::types::DocKind::default(),
        content: String::new(),
        unchanged: false,
        format: None,
        topic: None,
        title: None,
        author: None,
        lang: None,
        created_at: None,
        tags: None,
        mtime_ns: None,
        size_bytes: None,
        etag: None,
        last_modified: None,
        content_hash_override: Some(head_commit.to_owned()),
    }
}

/// Truncate a URL list to `MAX_DISCOVERED_URLS`, logging a warning if truncated.
fn cap_urls(urls: &mut Vec<String>, context: &str) {
    loaders::cap_urls(urls, MAX_DISCOVERED_URLS, context);
}

/// Fetch each sitemap URL and collect all page URLs for download.
async fn discover_sitemap(
    s: &crate::config::SitemapSource,
    ctx: &DiscoverCtx<'_>,
) -> Result<DiscoveredSource> {
    let include_re = compile_include_re(s.include.as_deref())?;
    let headers_opt = (!s.headers.is_empty()).then_some(&s.headers);
    let mut urls = Vec::new();
    for sitemap_url in &s.sitemap {
        let result = loaders::sitemap::discover_urls(
            ctx.fetcher,
            sitemap_url,
            include_re.as_ref(),
            headers_opt,
            ctx.progress,
        )
        .await?;
        let Some(mut discovered) = result else {
            debug!(
                sitemap = sitemap_url.as_str(),
                "sitemap 304, preserving existing URLs"
            );
            continue;
        };
        cap_urls(&mut discovered, sitemap_url.as_str());
        urls.extend(discovered);
    }
    cap_urls(&mut urls, "total sitemap");
    Ok(DiscoveredSource {
        items: SourceItems::Urls {
            urls,
            source_type: SourceType::Sitemap,
            topic: ctx.topic.clone(),
            etags: HashMap::new(),
            limits: ctx.limits,
            headers: s.headers.clone(),
        },
        extras: Vec::new(),
        load_failures: Vec::new(),
    })
}

/// Fetch each feed and collect article URLs for download.
async fn discover_feed(
    s: &crate::config::FeedSource,
    ctx: &DiscoverCtx<'_>,
) -> Result<DiscoveredSource> {
    let include_re = compile_include_re(s.include.as_deref())?;
    let mut urls = Vec::new();
    for feed_url in &s.feed {
        ctx.progress.inc_length(1);
        let resp = ctx
            .fetcher
            .fetch(&FetchRequest {
                url: feed_url,
                extra_headers: (!s.headers.is_empty()).then_some(&s.headers),
                ..Default::default()
            })
            .await
            .context("failed to fetch feed")?;
        ctx.progress.inc(1);
        let Some(resp) = resp else {
            continue;
        };

        let xml = String::from_utf8_lossy(&resp).into_owned();
        let mut feed_urls = loaders::feed::parse_feed_urls(&xml);

        if let Some(re) = &include_re {
            feed_urls.retain(|u| re.is_match(u));
        }

        cap_urls(&mut feed_urls, feed_url.as_str());

        info!(
            feed = feed_url.as_str(),
            urls = feed_urls.len(),
            "discovered URLs from feed"
        );
        urls.extend(feed_urls);
    }

    cap_urls(&mut urls, "total feed");
    Ok(DiscoveredSource {
        items: SourceItems::Urls {
            urls,
            source_type: SourceType::Feed,
            topic: ctx.topic.clone(),
            etags: HashMap::new(),
            limits: ctx.limits,
            headers: s.headers.clone(),
        },
        extras: Vec::new(),
        load_failures: Vec::new(),
    })
}

/// List and fetch S3 objects matching the configured URI and filters.
#[cfg(feature = "s3")]
async fn discover_s3(
    s: &crate::config::S3Source,
    ctx: &DiscoverCtx<'_>,
) -> Result<DiscoveredSource> {
    let mut all_items = Vec::new();
    let mut total_failures: Vec<FailedDoc> = Vec::new();
    for s3_uri in &s.s3 {
        let prefix = format!("{}/", s3_uri.trim_end_matches('/'));
        let etags: HashMap<SourceId, Option<String>> = ctx
            .existing_stamps
            .iter()
            .filter(|(k, _)| {
                ctx.existing_docs
                    .get(k.as_str())
                    .is_some_and(|d| d.source.starts_with(&prefix))
            })
            .map(|(k, v)| (k.clone(), v.etag.clone()))
            .collect();
        let effective_etags = if ctx.force { HashMap::new() } else { etags };
        let discovered = loaders::s3::discover(
            s3_uri,
            s.glob.as_deref(),
            s.include.as_deref(),
            &effective_etags,
            ctx.progress,
        )
        .await?;
        let (docs, failures) = loaders::s3::fetch_range(
            &discovered,
            0,
            discovered.objects.len(),
            s3_uri,
            ctx.topic.as_deref(),
            &ctx.limits,
            ctx.limits.concurrency,
            ctx.extract,
            ctx.progress,
        )
        .await?;
        all_items.extend(docs);
        total_failures.extend(failures);
    }
    Ok(DiscoveredSource::preloaded_with_failures(
        all_items,
        total_failures,
    ))
}

/// Fetch YouTube video transcripts for a configured source.
async fn discover_youtube(
    s: &crate::config::YoutubeSource,
    ctx: &DiscoverCtx<'_>,
) -> Result<DiscoveredSource> {
    let include_re = compile_include_re(s.include.as_deref())?;
    let mut all_docs = Vec::new();
    let mut total_failures: Vec<FailedDoc> = Vec::new();
    for url in &s.youtube {
        let (docs, failures) = loaders::youtube::fetch_youtube(
            url,
            &s.lang,
            ctx.topic.as_deref(),
            include_re.as_ref(),
            s.max_videos,
            &ctx.existing_docs,
            ctx.force,
            ctx.limits.concurrency,
            ctx.limits.extraction_timeout_secs,
            ctx.progress,
        )
        .await?;
        all_docs.extend(docs);
        total_failures.extend(failures);
    }
    Ok(DiscoveredSource::preloaded_with_failures(
        all_docs,
        total_failures,
    ))
}

/// Walk Maildir directories and load all messages.
async fn discover_maildir(
    s: &crate::config::MaildirSource,
    ctx: &DiscoverCtx<'_>,
) -> Result<DiscoveredSource> {
    let (docs, failures) = loaders::maildir::load_maildir(
        &s.maildir,
        s.skip_trashed,
        ctx.topic.as_deref(),
        &ctx.limits,
        &ctx.existing_stamps,
        ctx.cwd,
        ctx.force,
    )
    .await?;
    info!(
        count = docs.len(),
        failures = failures.len(),
        "loaded maildir messages"
    );
    Ok(DiscoveredSource::preloaded_with_failures(docs, failures))
}

/// Run exec commands and load all documents from JSONL output.
async fn discover_exec(
    s: &crate::config::ExecSource,
    ctx: &DiscoverCtx<'_>,
) -> Result<DiscoveredSource> {
    let timeout = s.timeout_secs.unwrap_or(ctx.limits.extraction_timeout_secs);
    let (docs, failures) = loaders::exec::run_exec(
        &s.exec,
        s.dir.as_deref(),
        &s.env,
        timeout,
        s.max_output_bytes,
        ctx.topic.as_deref(),
        s.output,
        s.source_key.as_deref(),
        s.format.as_deref(),
        ctx.cwd,
        ctx.progress,
    )
    .await?;
    info!(
        count = docs.len(),
        failures = failures.len(),
        "loaded exec source documents"
    );
    Ok(DiscoveredSource::preloaded_with_failures(docs, failures))
}

/// Connect to an upstream MCP server and ingest resources and/or tool results.
#[cfg(feature = "mcp")]
async fn discover_mcp(
    s: &crate::config::McpSource,
    ctx: &DiscoverCtx<'_>,
) -> Result<DiscoveredSource> {
    let timeout = s.timeout_secs.unwrap_or(ctx.limits.extraction_timeout_secs);
    let (docs, failures) =
        loaders::mcp::run_mcp(s, ctx.topic.as_deref(), timeout, ctx.progress).await?;
    info!(
        count = docs.len(),
        failures = failures.len(),
        "loaded MCP source documents"
    );
    Ok(DiscoveredSource::preloaded_with_failures(docs, failures))
}

/// Build a per-URL map of cached ETags and Last-Modified values for conditional requests.
fn build_etag_map(
    urls: &[String],
    existing_stamps: &HashMap<SourceId, StampsMeta>,
) -> HashMap<String, (Option<String>, Option<String>)> {
    if existing_stamps.is_empty() {
        return HashMap::new();
    }
    urls.iter()
        .filter_map(|url| {
            let fm = existing_stamps.get(&crate::types::source_id(url))?;
            if fm.etag.is_none() && fm.last_modified.is_none() {
                return None;
            }
            Some((url.clone(), (fm.etag.clone(), fm.last_modified.clone())))
        })
        .collect()
}
