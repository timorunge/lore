pub mod chunker;
pub(crate) mod discover;
pub mod loaders;
pub(crate) mod metadata;
pub(crate) mod pipeline;
pub mod status;
pub(crate) mod streaming;
pub mod transforms;
pub mod types;
pub mod watch;

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use anyhow::{Context, Result};
use futures::stream::{FuturesUnordered, StreamExt};
use tracing::info;

use crate::config::{IngestConfig, ProcessingLimits, SourceConfig, UpdateMode};
use crate::fmt::plural;
use crate::ingest::discover::{DiscoverCtx, discover_source};
use crate::ingest::pipeline::ProcessContext;
use crate::ingest::transforms::CompiledProfile;
#[cfg(feature = "llm")]
use crate::llm::LlmClient;
use crate::net::Fetcher;
use crate::store::{LOCK_FILE, Store, meta_key};
use crate::types::{SourceId, SourceType};
use crate::util::{dir_size, progress::ProgressHandle, relativize_path, truncate_str_ref};

/// Observer for ingest lifecycle events.
///
/// The CLI provides `indicatif` progress bars, signal handlers, and formatted
/// stderr output. The MCP server uses tracing. Tests use the no-op
/// [`QuietIngestObserver`].
pub trait IngestObserver: Send + Sync {
    /// Shutdown flag for graceful cancellation. The library checks this flag
    /// periodically; the observer is responsible for setting it (e.g. on Ctrl+C).
    fn shutdown_flag(&self) -> &AtomicBool;

    /// Create a progress handle for a labeled sub-step (fetch, index, enrich, loading).
    /// `len` is the initial expected total (0 means unknown/spinner).
    fn create_progress(&self, source_index: usize, label: &str, len: u64) -> ProgressHandle;

    /// Remove a previously created progress bar (e.g. when a source has no items).
    fn remove_progress(&self, _handle: &ProgressHandle) {}

    /// Ingest run starting. Called once before any sources are processed.
    fn on_start(&self, _kb_name: &str, _n_sources: usize, _mode: IngestMode, _dry_run: bool) {}

    /// Dry-run notice.
    fn on_dry_run_notice(&self) {}

    /// Status line update (e.g. "2 local, 1 git, 42 indexed documents").
    fn on_status(&self, _msg: &str) {}

    /// A source is now waiting (queued).
    fn on_source_waiting(&self, _index: usize, _label: &str) {}

    /// A source is now being processed.
    fn on_source_active(&self, _index: usize) {}

    /// A source completed successfully.
    fn on_source_done(
        &self,
        _index: usize,
        _label: &str,
        _docs: u64,
        _chunks: u64,
        _docs_failed: u64,
    ) {
    }

    /// A source completed with no documents.
    fn on_source_empty(&self, _index: usize, _label: &str) {}

    /// A source failed with an error.
    fn on_source_error(&self, _index: usize, _label: &str, _error: &str) {}

    /// An optimization step is starting.
    fn on_optimize_start(&self, _segments: usize) {}

    /// Progress within optimization (segment count decreased after a merge round).
    fn on_optimize_progress(&self, _segments: usize) {}

    /// The optimization step finished.
    fn on_optimize_done(&self) {}

    /// Ingest was interrupted -- committing progress.
    fn on_interrupted(&self) {}

    /// Interrupted commit result.
    fn on_interrupted_result(&self, _msg: &str, _success: bool) {}

    /// Final summary after completion.
    fn on_complete(&self, _result: &IngestResult, _store_path: &Path, _index_size: u64) {}

    /// Source errors summary (called when at least one source failed).
    fn on_source_errors(&self, _errors: &[(String, String)]) {}

    /// Final status messages (up-to-date, no docs, dry-run done).
    fn on_final_status(&self, _msg: &str) {}

    /// A single document was successfully indexed.
    fn on_document_indexed(&self, _source: &str, _chunks: u64) {}
}

/// No-op observer for watch mode, tests, and contexts where no UI is needed.
pub struct QuietIngestObserver {
    shutdown: Arc<AtomicBool>,
}

impl QuietIngestObserver {
    /// Create a no-op observer backed by the given shutdown flag.
    pub fn new(shutdown: Arc<AtomicBool>) -> Self {
        Self { shutdown }
    }
}

impl IngestObserver for QuietIngestObserver {
    fn shutdown_flag(&self) -> &AtomicBool {
        &self.shutdown
    }

    fn create_progress(&self, _source_index: usize, _label: &str, _len: u64) -> ProgressHandle {
        ProgressHandle::noop()
    }
}

/// Controls how ingestion handles the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestMode {
    Create,
    Recreate,
    Update,
}

impl IngestMode {
    /// Return a lowercase static string representation for display and metadata storage.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Recreate => "recreate",
            Self::Update => "update",
        }
    }
}

/// Result counters from processing a single source.
struct SourceResult {
    docs: u64,
    chunks: u64,
    failed_docs: Vec<types::FailedDoc>,
    seen_source_ids: HashSet<SourceId>,
}

/// All context needed to process a single source during ingest.
struct SourceArgs {
    source_index: usize,
    ctx: ProcessContext,
    fetcher: Arc<Fetcher>,
    cwd: Arc<Path>,
    observer: Arc<dyn IngestObserver>,
}

/// Summary returned by [`ingest`] for callers that need stats (e.g. `lore watch`).
#[derive(Clone)]
pub struct IngestResult {
    pub documents: u64,
    pub chunks: u64,
    pub elapsed: Duration,
    pub sources_ok: u64,
    pub sources_failed: u64,
    pub failed_docs: Vec<types::FailedDoc>,
}

/// Run the full ingest pipeline: discover, fetch, chunk, and index documents
/// from all configured sources into the store.
///
/// The `observer` controls all user-facing output (progress bars, status
/// messages, signal handling). Use [`QuietIngestObserver`] for silent
/// operation.
pub async fn ingest(
    config: IngestConfig,
    config_path: PathBuf,
    mode: IngestMode,
    dry_run: bool,
    force: bool,
    source_filter: Option<String>,
    observer: Arc<dyn IngestObserver>,
) -> Result<IngestResult> {
    let store_path = config.store_dir(&config_path);
    let kb_name = config.name.as_deref().unwrap_or("unnamed");

    let store_exists =
        store_path.is_dir() && std::fs::read_dir(&store_path).is_ok_and(|mut d| d.next().is_some());

    let mode = if store_exists {
        mode
    } else {
        IngestMode::Create
    };

    if mode == IngestMode::Recreate && store_exists && !dry_run {
        Store::destroy(&store_path).await?;
        info!(path = %store_path.display(), "destroyed existing store");
    }

    let store = Store::open(
        &store_path,
        config.store.phrase_search,
        config.store.writer_heap_mb,
        config.store.language,
        config.store.doc_store_cache_blocks,
    )
    .context("failed to open store")?;

    // Acquire an exclusive lock to prevent concurrent ingests against the
    // same store. The lock is acquired after Store::open (which creates the
    // directory) and after any --recreate destroy, so the lock file lives
    // in the final store directory.
    // Skip locking in dry-run mode since no writes occur.
    let lock_path = store_path.join(LOCK_FILE);
    let lock_file = if dry_run {
        None
    } else {
        Some(
            OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&lock_path)
                .with_context(|| format!("failed to create lock file: {}", lock_path.display()))?,
        )
    };
    let mut lock_rw = lock_file.map(fd_lock::RwLock::new);
    let mut lock_guard = lock_rw
        .as_mut()
        .map(|rw| {
            rw.try_write().map_err(|_| {
                let holder = std::fs::read_to_string(&lock_path)
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .map(|pid| format!(" (pid {pid})"))
                    .unwrap_or_default();
                anyhow::anyhow!(
                    "another ingest is already running against {}{holder}",
                    store_path.display()
                )
            })
        })
        .transpose()?;

    // Write our PID so a failed lock attempt can report who holds it.
    if let Some(guard) = lock_guard.as_mut() {
        use std::io::{Seek, Write};
        let file: &mut File = &mut *guard;
        let pid = std::process::id().to_string();
        if let Err(e) = file.seek(std::io::SeekFrom::Start(0)) {
            tracing::warn!("failed to seek lock file: {e}");
        }
        if let Err(e) = file.write_all(pid.as_bytes()) {
            tracing::warn!("failed to write PID to lock file: {e}");
        } else if let Err(e) = file.set_len(pid.len() as u64) {
            tracing::warn!("failed to truncate lock file: {e}");
        }
    }

    if dry_run {
        observer.on_dry_run_notice();
    }

    let active_sources: Vec<usize> = if let Some(ref filter) = source_filter {
        config
            .sources
            .iter()
            .enumerate()
            .filter(|(_, s)| s.label().contains(filter.as_str()))
            .map(|(i, _)| i)
            .collect()
    } else {
        (0..config.sources.len()).collect()
    };
    let n_sources = active_sources.len();

    let default_profile = Arc::new(
        CompiledProfile::compile(&config.processing.default_profile())
            .context("failed to compile default processing profile")?,
    );

    let orphans = crate::cache::cleanup_tmp();
    if orphans > 0 {
        info!(count = orphans, "cleaned up orphaned temp files");
    }

    let ingest_start = std::time::Instant::now();

    let shutdown = observer.shutdown_flag();

    let fetcher = Arc::new(Fetcher::new(&config.fetch)?);

    // Snapshot existing documents once before processing begins. This provides
    // a consistent "before state" for change detection and stale-document
    // removal, avoiding races where concurrent source processing could see
    // partially-updated document maps.
    let existing_docs = if mode == IngestMode::Update {
        store.get_all_documents()
    } else {
        Arc::new(HashMap::new())
    };
    let existing_stamps = if mode == IngestMode::Update {
        store.get_all_stamps()
    } else {
        Arc::new(HashMap::new())
    };

    let n_existing = existing_docs.len();
    observer.on_start(kb_name, n_sources, mode, dry_run);
    observer.on_status(&format_status_line(&config.sources, n_existing, None));

    let text_ext: Option<Arc<Vec<String>>> = config
        .processing
        .text_extensions
        .as_ref()
        .map(|v| Arc::new(v.clone()));

    let ctx = ProcessContext {
        store: Arc::new(store),
        config: Arc::new(config.clone()),
        default_profile,
        mode,
        dry_run,
        force,
        existing_docs: existing_docs.clone(),
        existing_stamps: existing_stamps.clone(),
        text_ext,
        #[cfg(feature = "llm")]
        llm_client: config
            .llm
            .as_ref()
            .map(|c| LlmClient::new(c).map(Arc::new))
            .transpose()
            .context("failed to initialize LLM client")?,
    };
    let store = &*ctx.store;

    let mut total_docs = 0u64;
    let mut total_chunks = 0u64;
    let mut all_failed_docs: Vec<types::FailedDoc> = Vec::new();
    let mut source_errors: Vec<(String, String)> = Vec::new();
    // Tracks source_id hashes seen during this run. Note: archive sub-files
    // use `url#relative/path` format which may not appear in the pre-computed
    // source keys from `discover_source`. This is expected -- archives are
    // treated as a single source for staleness purposes.
    let mut seen_source_ids: HashSet<SourceId> = HashSet::new();
    let mut sources_ok: u64 = 0;

    let cwd: Arc<Path> = Arc::from(std::env::current_dir().unwrap_or_else(|e| {
        tracing::warn!("failed to determine current directory: {e}");
        std::path::PathBuf::from(".")
    }));

    let source_labels: Vec<String> = active_sources
        .iter()
        .enumerate()
        .map(|(display_idx, &src_idx)| {
            let src = &config.sources[src_idx];
            let label = if matches!(src, SourceConfig::Local(_)) {
                relativize_path(Path::new(&*src.label()), &cwd)
            } else {
                src.label().into_owned()
            };
            format!(
                "[{}/{}] {} ({})",
                display_idx + 1,
                n_sources,
                truncate_str_ref(&label, 60),
                src.config_key(),
            )
        })
        .collect();

    for (i, label) in source_labels.iter().enumerate() {
        observer.on_source_waiting(i, label);
    }

    let futures: FuturesUnordered<_> = active_sources
        .iter()
        .enumerate()
        .map(|(display_idx, &src_idx)| {
            let args = SourceArgs {
                source_index: src_idx,
                ctx: ctx.clone(),
                fetcher: fetcher.clone(),
                cwd: cwd.clone(),
                observer: observer.clone(),
            };
            async move {
                let result = process_source(args).await;
                (display_idx, result)
            }
        })
        .collect();

    let mut futures = futures;
    while let Some((i, result)) = futures.next().await {
        if !dry_run {
            commit_and_update_status(store, &*observer, &config.sources, false, None);
        }
        let source_label = &source_labels[i];
        match result {
            Ok(sr) => {
                total_docs += sr.docs;
                total_chunks += sr.chunks;
                let n_failed = sr.failed_docs.len() as u64;
                all_failed_docs.extend(sr.failed_docs);
                sources_ok += 1;

                let had_items = !sr.seen_source_ids.is_empty();
                seen_source_ids.extend(sr.seen_source_ids);

                if sr.docs > 0 {
                    observer.on_source_done(i, source_label, sr.docs, sr.chunks, n_failed);
                } else if had_items {
                    observer.on_source_done(i, source_label, 0, 0, n_failed);
                } else {
                    observer.on_source_empty(i, source_label);
                }
            }
            Err(e) => {
                let err_str = e.to_string();
                observer.on_source_error(i, source_label, &err_str);
                source_errors.push((source_label.clone(), err_str));
            }
        }
    }

    drop(futures);

    let interrupted = shutdown.load(Ordering::SeqCst);

    if interrupted {
        observer.on_interrupted();

        if dry_run {
            observer.on_interrupted_result("interrupted", true);
        } else {
            match tokio::task::block_in_place(|| store.force_commit()) {
                Ok(()) => {
                    observer.on_interrupted_result("done\nhint: resume with `lore ingest`", true);
                }
                Err(e) => {
                    observer.on_interrupted_result(&format!("commit failed: {e}"), false);
                }
            }
        }

        drop(lock_guard);
        std::fs::remove_file(&lock_path).ok();
        std::io::Write::flush(&mut std::io::stderr()).ok();
        crate::util::platform::silence_stderr_for_exit();
        std::process::exit(130);
    }

    let n = store.document_count();
    observer.on_status(&format_status_line(&config.sources, n, None));

    if mode == IngestMode::Update && !dry_run && source_filter.is_none() {
        cleanup_stale_documents(
            store,
            &config,
            &ctx.existing_docs,
            &seen_source_ids,
            sources_ok,
        )?;
    }

    let has_changes = total_docs > 0 || total_chunks > 0 || store.is_dirty();

    if !dry_run && has_changes {
        write_store_metadata(store, &config, mode, &*observer)?;
    }

    let elapsed = ingest_start.elapsed();
    let result = IngestResult {
        documents: total_docs,
        chunks: total_chunks,
        elapsed,
        sources_ok,
        sources_failed: source_errors.len() as u64,
        failed_docs: all_failed_docs,
    };

    let index_size = if dry_run { 0 } else { dir_size(&store_path) };
    observer.on_complete(&result, &store_path, index_size);

    if !source_errors.is_empty() {
        let n = source_errors.len();
        observer.on_source_errors(&source_errors);
        drop(lock_guard);
        std::fs::remove_file(&lock_path).ok();
        anyhow::bail!("{n} source{} failed", plural(n));
    }

    if dry_run {
        observer.on_final_status("dry_run_done");
    } else if total_docs == 0 && mode == IngestMode::Update {
        observer.on_final_status("up_to_date");
    } else if total_docs == 0 {
        observer.on_final_status("no_documents");
    }

    drop(lock_guard);
    std::fs::remove_file(&lock_path).ok();

    Ok(result)
}

/// Discover, fetch, chunk, and index all documents from a single source entry.
async fn process_source(args: SourceArgs) -> Result<SourceResult> {
    let SourceArgs {
        source_index,
        ctx,
        fetcher,
        cwd,
        observer,
    } = args;
    let source = &ctx.config.sources[source_index];
    let fetcher = &*fetcher;

    if source.update() == UpdateMode::Never && !ctx.force && ctx.mode == IngestMode::Update {
        let matcher = SourceMatcher::new(source, &cwd);
        let mut seen = HashSet::new();
        for (key, meta) in ctx.existing_docs.iter() {
            if matcher.matches(meta.source.as_str()) {
                seen.insert(key.clone());
            }
        }
        if matches!(
            source,
            SourceConfig::Feed(_)
                | SourceConfig::Sitemap(_)
                | SourceConfig::Youtube(_)
                | SourceConfig::Exec(_)
                | SourceConfig::Mcp(_)
        ) {
            let origin = match source {
                SourceConfig::Feed(_) => SourceType::Feed,
                SourceConfig::Sitemap(_) => SourceType::Sitemap,
                SourceConfig::Youtube(_) => SourceType::Youtube,
                SourceConfig::Exec(_) => SourceType::Exec,
                SourceConfig::Mcp(_) => SourceType::Mcp,
                _ => unreachable!(),
            };
            for (key, meta) in ctx.existing_docs.iter() {
                if meta.origin == origin {
                    seen.insert(key.clone());
                }
            }
        }
        if !seen.is_empty() {
            return Ok(SourceResult {
                docs: 0,
                chunks: 0,
                failed_docs: Vec::new(),
                seen_source_ids: seen,
            });
        }
    }

    let effective_force = ctx.force || source.update() == UpdateMode::Always;

    observer.on_source_active(source_index);

    let loading_token = ProgressHandle::noop();
    let fetch_token = observer.create_progress(source_index, "fetching", 0);
    let has_enrich = {
        #[cfg(feature = "llm")]
        {
            ctx.llm_client.is_some()
                && ctx
                    .config
                    .llm
                    .as_ref()
                    .is_some_and(crate::config::LlmConfig::has_enrichment)
        }
        #[cfg(not(feature = "llm"))]
        {
            false
        }
    };
    let enrich_token = if has_enrich {
        observer.create_progress(source_index, "enriching", 0)
    } else {
        ProgressHandle::noop()
    };
    let index_token = observer.create_progress(source_index, "indexing", 0);

    let compiled = if source.processing().is_some() {
        let profile = ctx
            .config
            .processing
            .resolve(source.processing())
            .context("failed to resolve processing profile")?;
        Arc::new(
            CompiledProfile::compile(&profile).context("failed to compile processing profile")?,
        )
    } else {
        ctx.default_profile.clone()
    };

    let effective_stamps = if effective_force && !ctx.force {
        Arc::new(HashMap::new())
    } else {
        ctx.existing_stamps.clone()
    };

    let discover_ctx = DiscoverCtx {
        fetcher,
        existing_docs: Arc::clone(&ctx.existing_docs),
        existing_stamps: Arc::clone(&effective_stamps),
        cwd: &cwd,
        progress: &loading_token,
        extract: compiled.extract,
        force: effective_force,
        limits: ProcessingLimits::from_config(&ctx.config.processing),
        topic: source.topic().map(str::to_owned),
        text_ext: ctx.text_ext.clone(),
    };
    let discovered = discover_source(source, &discover_ctx).await?;

    let item_count = discovered.items.len();

    let mut doc_sources: HashSet<SourceId> = discovered
        .items
        .source_keys()
        .into_iter()
        .chain(discovered.extras.iter().map(|e| e.source_id.clone()))
        .collect();

    if item_count == 0 && discovered.extras.is_empty() {
        // No items discovered. For remote sources (git, url, s3), this may be
        // a transient failure -- protect existing docs so they survive cleanup.
        // For local sources, zero items means the directory is genuinely empty
        // (or all files were deleted), so return an empty seen set to let the
        // stale-document sweep remove them.
        let seen = if matches!(source, SourceConfig::Local(_) | SourceConfig::Maildir(_)) {
            HashSet::new()
        } else {
            let matcher = SourceMatcher::new(source, &cwd);
            let mut s = HashSet::new();
            for (key, meta) in ctx.existing_docs.iter() {
                if matcher.matches(meta.source.as_str()) {
                    s.insert(key.clone());
                }
            }
            s
        };
        observer.remove_progress(&loading_token);
        observer.remove_progress(&fetch_token);
        observer.remove_progress(&index_token);
        observer.remove_progress(&enrich_token);
        return Ok(SourceResult {
            docs: 0,
            chunks: 0,
            failed_docs: Vec::new(),
            seen_source_ids: seen,
        });
    }

    let discovered_total = (item_count + discovered.extras.len()) as u64;
    let already_indexed = if effective_force {
        0u64
    } else {
        let matcher = SourceMatcher::new(source, &cwd);
        ctx.existing_docs
            .iter()
            .filter(|(_, meta)| matcher.matches(meta.source.as_str()))
            .count() as u64
    };
    let work_total = discovered_total.saturating_sub(already_indexed);
    fetch_token.set_length(work_total);
    enrich_token.set_length(work_total);
    index_token.set_length(work_total);

    observer.on_status(&format_status_line(
        &ctx.config.sources,
        ctx.store.document_count(),
        Some(discovered_total as usize),
    ));

    let pipeline_ctx = if effective_force && !ctx.force {
        let mut c = ctx.clone();
        c.force = true;
        c.existing_stamps = effective_stamps;
        c
    } else {
        ctx.clone()
    };
    let (source_docs, source_chunks, source_failed_docs) = streaming::run_streaming_pipeline(
        discovered,
        &pipeline_ctx,
        &compiled,
        fetcher,
        &*observer,
        &fetch_token,
        &enrich_token,
        &index_token,
        &mut doc_sources,
        discovered_total as usize,
        source.tags(),
    )
    .await?;

    observer.remove_progress(&loading_token);
    observer.remove_progress(&fetch_token);
    observer.remove_progress(&index_token);
    observer.remove_progress(&enrich_token);

    Ok(SourceResult {
        docs: source_docs,
        chunks: source_chunks,
        failed_docs: source_failed_docs,
        seen_source_ids: doc_sources,
    })
}

/// Commit store changes and update the observer's status line.
///
/// When `index_only` is true, only the Tantivy index is flushed (periodic
/// checkpoint). When false, both the index and sidecar files are committed.
fn commit_and_update_status(
    store: &Store,
    observer: &dyn IngestObserver,
    sources: &[SourceConfig],
    index_only: bool,
    discovered: Option<usize>,
) {
    let result = tokio::task::block_in_place(|| {
        if index_only {
            store.commit_index()
        } else {
            store.commit()
        }
    });
    match result {
        Ok(()) => {
            let n = store.document_count();
            observer.on_status(&format_status_line(sources, n, discovered));
        }
        Err(e) => {
            let kind = if index_only { "periodic" } else { "source" };
            tracing::warn!("{kind} commit failed: {e}");
        }
    }
}

/// Remove documents from sources that completed successfully but whose IDs
/// were not seen during this run. Defers feed/sitemap/youtube cleanup to
/// handle the discard flag and transient fetch failures correctly.
fn cleanup_stale_documents(
    store: &Store,
    config: &IngestConfig,
    existing_docs: &HashMap<SourceId, crate::types::DocMeta>,
    seen_source_ids: &HashSet<SourceId>,
    sources_ok: u64,
) -> Result<()> {
    if sources_ok > 0 {
        let removed = remove_docs_where(store, existing_docs, |source_id, meta| {
            !seen_source_ids.contains(source_id)
                && meta.origin != crate::types::SourceType::Feed
                && meta.origin != crate::types::SourceType::Sitemap
                && meta.origin != crate::types::SourceType::Youtube
                && meta.origin != crate::types::SourceType::Exec
                && meta.origin != crate::types::SourceType::Mcp
        })?;
        if removed > 0 {
            info!(removed, "removed stale documents");
        }
    }

    let discard_feed = config
        .sources
        .iter()
        .any(|s| matches!(s, SourceConfig::Feed(f) if f.discard));
    for origin in [
        crate::types::SourceType::Sitemap,
        crate::types::SourceType::Youtube,
        crate::types::SourceType::Exec,
        crate::types::SourceType::Mcp,
    ] {
        let removed = remove_docs_where(store, existing_docs, |id, meta| {
            meta.origin == origin && !seen_source_ids.contains(id)
        })?;
        if removed > 0 {
            info!(removed, origin = ?origin, "removed stale documents");
        }
    }
    if discard_feed {
        let removed = remove_docs_where(store, existing_docs, |id, meta| {
            meta.origin == crate::types::SourceType::Feed && !seen_source_ids.contains(id)
        })?;
        if removed > 0 {
            info!(removed, "removed stale feed documents (discard: true)");
        }
    }
    Ok(())
}

/// Write run metadata to the store, commit, and optimize if needed.
fn write_store_metadata(
    store: &Store,
    config: &IngestConfig,
    mode: IngestMode,
    observer: &dyn IngestObserver,
) -> Result<()> {
    store.set_metadata(meta_key::MODE, mode.as_str());

    let now = crate::util::iso8601_now();
    if mode == IngestMode::Create || mode == IngestMode::Recreate {
        store.set_metadata(meta_key::CREATED_AT, &now);
    }
    store.set_metadata(meta_key::UPDATED_AT, &now);
    store.set_metadata(meta_key::LORE_VERSION, crate::VERSION);
    store.set_metadata(
        meta_key::PHRASE_SEARCH,
        if config.store.phrase_search {
            "true"
        } else {
            "false"
        },
    );
    store.set_metadata(
        meta_key::WRITER_HEAP_MB,
        &config.store.writer_heap_mb.to_string(),
    );
    store.set_metadata(meta_key::LANGUAGE, config.store.language.as_str());

    if let Some(name) = &config.name {
        store.set_metadata(meta_key::NAME, name);
    }
    if let Some(description) = &config.description {
        store.set_metadata(meta_key::DESCRIPTION, description);
    }

    tokio::task::block_in_place(|| store.commit())?;

    let segments = store.segment_count();
    if segments > 1 {
        observer.on_optimize_start(segments);
        tokio::task::block_in_place(|| store.optimize(|segs| observer.on_optimize_progress(segs)))?;
        observer.on_optimize_done();
    }
    Ok(())
}

/// Pre-computed prefix strings for matching documents against a source config.
/// Built once per source, then reused across all document checks.
enum SourceMatcher {
    /// Each prefix is "url#"; a doc matches if its source starts with any prefix.
    Git(Vec<String>),
    /// Each entry is (exact_path, prefix_with_slash).
    Local(Vec<(String, String)>),
    /// Exact URL matches only.
    Url(Vec<String>),
    /// Each prefix is "rel/"; a doc matches if its source starts with any prefix.
    Maildir(Vec<String>),
    /// Each entry is (exact_uri, prefix_with_slash).
    S3(Vec<(String, String)>),
    /// Source types that never own persisted documents.
    None,
}

impl SourceMatcher {
    fn new(source: &SourceConfig, cwd: &Path) -> Self {
        match source {
            SourceConfig::Git(s) => Self::Git(s.git.iter().map(|url| format!("{url}#")).collect()),
            SourceConfig::Local(s) => Self::Local(
                s.path
                    .iter()
                    .map(|p| {
                        let rel = relativize_path(Path::new(p), cwd);
                        let pfx = format!("{rel}/");
                        (rel, pfx)
                    })
                    .collect(),
            ),
            SourceConfig::Url(s) => Self::Url(s.url.clone()),
            SourceConfig::Sitemap(_)
            | SourceConfig::Feed(_)
            | SourceConfig::Youtube(_)
            | SourceConfig::Exec(_)
            | SourceConfig::Mcp(_) => Self::None,
            SourceConfig::Maildir(s) => Self::Maildir(
                s.maildir
                    .iter()
                    .map(|p| {
                        let expanded = crate::config::expand_path(p);
                        let rel = relativize_path(Path::new(&expanded), cwd);
                        format!("{rel}/")
                    })
                    .collect(),
            ),
            SourceConfig::S3(s) => Self::S3(
                s.s3.iter()
                    .map(|uri| {
                        let pfx = format!("{uri}/");
                        (uri.clone(), pfx)
                    })
                    .collect(),
            ),
        }
    }

    fn matches(&self, src: &str) -> bool {
        match self {
            Self::Git(prefixes) => prefixes.iter().any(|pfx| src.starts_with(pfx.as_str())),
            Self::Local(entries) => entries
                .iter()
                .any(|(exact, pfx)| src == exact.as_str() || src.starts_with(pfx.as_str())),
            Self::Url(urls) => urls.iter().any(|u| u == src),
            Self::Maildir(prefixes) => prefixes.iter().any(|pfx| src.starts_with(pfx.as_str())),
            Self::S3(entries) => entries
                .iter()
                .any(|(exact, pfx)| src == exact.as_str() || src.starts_with(pfx.as_str())),
            Self::None => false,
        }
    }
}

/// Remove documents from the store where the predicate returns true.
/// The `source_id` parameter is the hashed source key, not the raw path.
fn remove_docs_where(
    store: &Store,
    existing_docs: &HashMap<SourceId, crate::types::DocMeta>,
    should_remove: impl Fn(&str, &crate::types::DocMeta) -> bool,
) -> Result<u64> {
    let mut removed = 0u64;
    for (source_id, meta) in existing_docs {
        if should_remove(source_id.as_str(), meta) {
            store.delete_chunks_by_source(source_id)?;
            store.delete_document(source_id);
            removed += 1;
        }
    }
    Ok(removed)
}

/// Format a plain status string showing source summary and current indexed count.
pub fn format_status_line(
    sources: &[SourceConfig],
    indexed: usize,
    discovered: Option<usize>,
) -> String {
    let summary = summarize_source_configs(sources);
    match (indexed, discovered) {
        (0, _) => summary,
        (n, Some(total)) => format!("{summary}, {n}/{total} indexed"),
        (n, None) => format!("{summary}, {n} indexed document{}", plural(n)),
    }
}

/// Compile an optional include regex from a pattern string.
pub(crate) fn compile_include_re(pattern: Option<&str>) -> Result<Option<regex::Regex>> {
    pattern
        .map(regex::Regex::new)
        .transpose()
        .context("invalid include regex")
}

/// Summarize the source list as a compact human-readable string (e.g. "2 local, 1 url").
fn summarize_source_configs(sources: &[SourceConfig]) -> String {
    use std::fmt::Write as _;

    let mut counts: Vec<(&str, usize)> = Vec::with_capacity(8);
    for src in sources {
        let tag = src.config_key();
        let n = src.item_count();
        if let Some(entry) = counts.iter_mut().find(|(t, _)| *t == tag) {
            entry.1 += n;
        } else {
            counts.push((tag, n));
        }
    }
    let mut out = String::new();
    for (i, (tag, n)) in counts.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        w!(out, "{n} {tag}{}", plural(*n));
    }
    out
}
