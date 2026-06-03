use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::config::{IngestConfig, SourceConfig, UpdateMode};
use crate::ingest::{IngestMode, IngestObserver, IngestResult, ingest};

/// Observer for watch loop lifecycle events.
///
/// Callers implement this trait to control how watch events are presented.
/// The CLI provides colored bracket output; the MCP server can use
/// tracing or a no-op implementation.
pub trait WatchObserver: Send + Sync {
    /// A local path is now being watched for changes.
    fn on_watching(&self, path: &Path);
    /// A local path could not be watched.
    fn on_watch_error(&self, path: &Path, error: &notify::Error);
    /// The watch mode configuration summary (emitted once at startup).
    fn on_mode(&self, has_watcher: bool, interval_secs: Option<u64>, debounce_secs: u64);
    /// The watch loop is shutting down (e.g. Ctrl+C).
    fn on_stopping(&self);
    /// An ingest cycle completed successfully.
    fn on_cycle_ok(&self, result: &IngestResult);
    /// Post-cycle hook (e.g. reload caches). Called after each successful ingest.
    fn on_cycle_complete(&self) {}
    /// An ingest cycle (or the initial ingest) failed.
    fn on_cycle_error(&self, context: &str, error: &anyhow::Error);
}

/// Watch local source paths and re-ingest on changes, with optional periodic
/// polling.
///
/// # Errors
///
/// Returns an error if the filesystem watcher cannot be initialised or encounters a fatal I/O failure.
pub async fn watch(
    config: IngestConfig,
    config_path: PathBuf,
    debounce_secs: u64,
    interval_secs: Option<u64>,
    source_filter: Option<String>,
    observer: Box<dyn WatchObserver>,
    ingest_observer: Arc<dyn IngestObserver>,
) -> Result<()> {
    let watch_paths: Vec<PathBuf> = config
        .sources
        .iter()
        .filter(|s| {
            source_filter
                .as_deref()
                .is_none_or(|f| s.label().contains(f))
        })
        .flat_map(|s| match s {
            SourceConfig::Local(s) if s.update != UpdateMode::Never => s
                .path
                .iter()
                .map(|p| crate::util::normalize_path(Path::new(p)))
                .collect(),
            _ => Vec::new(),
        })
        .collect();

    if watch_paths.is_empty() && interval_secs.is_none() {
        anyhow::bail!("no local sources and no --interval; nothing to watch");
    }

    match ingest(
        config.clone(),
        config_path.clone(),
        IngestMode::Update,
        false,
        false,
        source_filter.clone(),
        ingest_observer.clone(),
    )
    .await
    {
        Ok(result) => {
            observer.on_cycle_ok(&result);
            observer.on_cycle_complete();
        }
        Err(e) => observer.on_cycle_error("initial ingest failed", &e),
    }

    let has_watcher = !watch_paths.is_empty();
    let (tx, mut rx) = mpsc::unbounded_channel::<()>();

    let mut watcher: Option<RecommendedWatcher> = if has_watcher {
        let tx_watcher = tx.clone();
        let w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok() {
                tx_watcher.send(()).ok();
            }
        })?;
        Some(w)
    } else {
        None
    };
    drop(tx);

    if let Some(ref mut w) = watcher {
        for path in &watch_paths {
            match w.watch(path, RecursiveMode::Recursive) {
                Ok(()) => observer.on_watching(path),
                Err(e) => observer.on_watch_error(path, &e),
            }
        }
    }

    observer.on_mode(has_watcher, interval_secs, debounce_secs);

    let debounce = Duration::from_secs(debounce_secs);

    let mut interval_timer = interval_secs.map(|s| tokio::time::interval(Duration::from_secs(s)));

    if let Some(ref mut timer) = interval_timer {
        timer.tick().await;
    }

    loop {
        let triggered = tokio::select! {
            msg = rx.recv(), if has_watcher => {
                if msg.is_none() { break; }
                true
            }
            () = async {
                if let Some(ref mut timer) = interval_timer {
                    timer.tick().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => { false }
            _ = tokio::signal::ctrl_c() => {
                observer.on_stopping();
                break;
            }
        };

        if triggered {
            loop {
                tokio::select! {
                    () = tokio::time::sleep(debounce) => break,
                    msg = rx.recv() => {
                        if msg.is_none() { break; }
                    }
                    _ = tokio::signal::ctrl_c() => {
                        observer.on_stopping();
                        return Ok(());
                    }
                }
            }
        }

        ingest_observer
            .shutdown_flag()
            .store(false, Ordering::Release);

        let observer_for_ctrlc = ingest_observer.clone();
        let ctrlc_task = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                observer_for_ctrlc
                    .shutdown_flag()
                    .store(true, Ordering::Release);
            }
        });

        let ingest_result = ingest(
            config.clone(),
            config_path.clone(),
            IngestMode::Update,
            false,
            false,
            source_filter.clone(),
            ingest_observer.clone(),
        )
        .await;

        ctrlc_task.abort();

        if ingest_observer.shutdown_flag().load(Ordering::Acquire) {
            observer.on_stopping();
            return Ok(());
        }

        match ingest_result {
            Ok(result) => {
                observer.on_cycle_ok(&result);
                observer.on_cycle_complete();
            }
            Err(e) => observer.on_cycle_error("ingest failed", &e),
        }
    }

    Ok(())
}
