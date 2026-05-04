use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget};

use lore::config::{IngestConfig, SourceConfig};
use lore::fmt::plural;
use lore::fmt::style::Painter;
use lore::ingest::watch::WatchObserver;
use lore::ingest::{IngestMode, IngestObserver, IngestResult};
use lore::util::progress::ProgressHandle;

use crate::cli::{LinePrefix, ResolvedConfig, config_label};
use crate::progress::{self, SourceBars};
use crate::terminal;

/// Cycle-scoped progress bars using shared types.
struct WatchIngestObserver {
    mp: MultiProgress,
    shutdown: Arc<AtomicBool>,
    bars: SourceBars,
    idle_bar: Arc<std::sync::Mutex<ProgressBar>>,
}

impl WatchIngestObserver {
    fn new(mp: MultiProgress, shutdown: Arc<AtomicBool>) -> Self {
        let term_width = terminal::output_width();
        let idle = mp.add(ProgressBar::new_spinner());
        idle.set_style(progress::source_waiting());
        idle.set_message("waiting for changes...");
        idle.enable_steady_tick(progress::TICK_WAITING);

        Self {
            bars: SourceBars::new(mp.clone(), term_width),
            mp,
            shutdown,
            idle_bar: Arc::new(std::sync::Mutex::new(idle)),
        }
    }

    fn show_idle(&self) {
        let mut idle = self.idle_bar.lock().expect("mutex should not be poisoned");
        idle.finish_and_clear();
        let bar = self.mp.add(ProgressBar::new_spinner());
        bar.set_style(progress::source_waiting());
        bar.set_message("waiting for changes...");
        bar.enable_steady_tick(progress::TICK_WAITING);
        *idle = bar;
    }

    fn hide_idle(&self) {
        self.idle_bar
            .lock()
            .expect("mutex should not be poisoned")
            .finish_and_clear();
    }
}

impl IngestObserver for WatchIngestObserver {
    fn shutdown_flag(&self) -> &AtomicBool {
        &self.shutdown
    }

    fn create_progress(&self, source_index: usize, label: &str, _len: u64) -> ProgressHandle {
        self.bars.create_progress(source_index, label)
    }

    fn remove_progress(&self, _handle: &ProgressHandle) {}

    fn on_start(&self, _kb_name: &str, _n_sources: usize, _mode: IngestMode, _dry_run: bool) {
        self.hide_idle();
        self.bars.clear();
    }

    fn on_status(&self, _msg: &str) {}

    fn on_source_waiting(&self, _index: usize, label: &str) {
        self.bars.add_waiting(label);
    }

    fn on_source_active(&self, index: usize) {
        self.bars.activate(index);
    }

    fn on_source_done(
        &self,
        index: usize,
        _label: &str,
        docs: u64,
        chunks: u64,
        _docs_failed: u64,
    ) {
        self.bars.done(index, docs, chunks, false);
    }

    fn on_source_empty(&self, index: usize, _label: &str) {
        self.bars.empty(index, false);
    }

    fn on_source_error(&self, index: usize, _label: &str, _error: &str) {
        self.bars.error(index, false);
    }

    fn on_complete(&self, _result: &IngestResult, _store_path: &Path, _index_size: u64) {
        self.bars.finish_and_clear_all();
        self.show_idle();
    }

    fn on_interrupted(&self) {
        self.mp.set_draw_target(ProgressDrawTarget::hidden());
    }

    fn on_interrupted_result(&self, _msg: &str, _success: bool) {}
}

/// Formats lifecycle events via `mp.println`.
struct CliWatchObserver {
    mp: MultiProgress,
    paint: Painter,
    prefix: LinePrefix,
    idle_bar: Arc<std::sync::Mutex<ProgressBar>>,
}

impl WatchObserver for CliWatchObserver {
    fn on_watching(&self, path: &Path) {
        progress::mp_println(
            &self.mp,
            format!(
                "{}[{} ] watching {}",
                self.prefix,
                self.paint.blue("i"),
                path.display()
            ),
        );
    }

    fn on_watch_error(&self, path: &Path, error: &notify::Error) {
        progress::mp_println(
            &self.mp,
            format!(
                "{}[{} ] skipping {} ({error})",
                self.prefix,
                self.paint.yellow("-"),
                path.display(),
            ),
        );
    }

    fn on_mode(&self, has_watcher: bool, interval_secs: Option<u64>, debounce_secs: u64) {
        let msg = match (has_watcher, interval_secs) {
            (true, Some(secs)) => format!(
                "{}[{} ] watching local paths, polling every {secs}s",
                self.prefix,
                self.paint.blue("i"),
            ),
            (false, Some(secs)) => format!(
                "{}[{} ] polling every {secs}s (no local sources to watch)",
                self.prefix,
                self.paint.blue("i"),
            ),
            (true, None) if self.prefix.is_some() => format!(
                "{}[{} ] debounce: {debounce_secs}s",
                self.prefix,
                self.paint.blue("i"),
            ),
            (true, None) => format!(
                "{}[{} ] debounce: {debounce_secs}s. Press Ctrl+C to stop.",
                self.prefix,
                self.paint.blue("i"),
            ),
            (false, None) => unreachable!("watch bails before this"),
        };
        progress::mp_println(&self.mp, msg);
    }

    fn on_stopping(&self) {
        self.idle_bar
            .lock()
            .expect("mutex should not be poisoned")
            .finish_and_clear();
        progress::mp_println(
            &self.mp,
            format!("{}[{} ] stopping", self.prefix, self.paint.blue("i")),
        );
    }

    fn on_cycle_ok(&self, result: &IngestResult) {
        let secs = result.elapsed.as_secs_f64();
        let n_failed = result.failed_docs.len();
        if result.documents == 0 && n_failed == 0 {
            progress::mp_println(
                &self.mp,
                format!(
                    "{}[{} ] up to date ({secs:.1}s)",
                    self.prefix,
                    self.paint.purple(".")
                ),
            );
        } else {
            progress::mp_println(
                &self.mp,
                format!(
                    "{}[{} ] {} doc{}, {} chunk{} ({secs:.1}s)",
                    self.prefix,
                    self.paint.green("+"),
                    result.documents,
                    plural(result.documents),
                    result.chunks,
                    plural(result.chunks),
                ),
            );
        }
        if n_failed > 0 {
            progress::mp_println(
                &self.mp,
                format!(
                    "{}[{} ] {} document{} failed",
                    self.prefix,
                    self.paint.yellow("!"),
                    n_failed,
                    plural(n_failed),
                ),
            );
        }
    }

    fn on_cycle_error(&self, context: &str, error: &anyhow::Error) {
        progress::mp_println(
            &self.mp,
            format!(
                "{}[{} ] {context}: {error:#}",
                self.prefix,
                self.paint.yellow("-")
            ),
        );
    }
}

/// Create paired ingest and watch observers sharing a `MultiProgress`.
fn make_observers(
    mp: MultiProgress,
    paint: Painter,
    prefix: LinePrefix,
) -> (Arc<dyn IngestObserver>, CliWatchObserver) {
    let shutdown = Arc::new(AtomicBool::new(false));
    let ingest_obs = Arc::new(WatchIngestObserver::new(mp.clone(), shutdown));
    let watch_obs = CliWatchObserver {
        mp,
        paint,
        prefix,
        idle_bar: ingest_obs.idle_bar.clone(),
    };
    (ingest_obs, watch_obs)
}

/// Watch local source paths and re-ingest on changes, with optional periodic polling.
pub async fn watch(
    config: &IngestConfig,
    config_path: &Path,
    debounce_secs: u64,
    interval_secs: Option<u64>,
    source_filter: Option<String>,
    prefix: &LinePrefix,
) -> Result<()> {
    let mp = MultiProgress::new();
    let paint = terminal::stderr_painter();
    let (ingest_observer, watch_observer) = make_observers(mp, paint, prefix.clone());

    lore::ingest::watch::watch(
        config.clone(),
        config_path.to_path_buf(),
        debounce_secs,
        interval_secs,
        source_filter,
        Box::new(watch_observer),
        ingest_observer,
    )
    .await
}

/// Run watch sessions for all configs concurrently. A single Ctrl+C stops all.
pub async fn watch_all(
    configs: &[ResolvedConfig],
    prefixes: &[LinePrefix],
    debounce_secs: u64,
    interval_secs: Option<u64>,
    source_filter: Option<&str>,
) -> Result<()> {
    let paint = terminal::stderr_painter();
    let mp = MultiProgress::new();

    let mut set = tokio::task::JoinSet::new();
    for (rc, pfx) in configs.iter().zip(prefixes) {
        let has_local = rc
            .config
            .sources
            .iter()
            .any(|s| matches!(s, SourceConfig::Local(_)));
        if !has_local && interval_secs.is_none() {
            let label = config_label(rc.config.name.as_ref(), &rc.config_path);
            progress::mp_println(
                &mp,
                format!(
                    "{pfx}[{} ] skipping \"{label}\" (no local sources)",
                    paint.blue("i"),
                ),
            );
            continue;
        }
        let config = rc.config.clone();
        let config_path = rc.config_path.clone();
        let prefix = pfx.clone();
        let mp_clone = mp.clone();
        let sf = source_filter.map(String::from);
        set.spawn(async move {
            let (ingest_observer, watch_observer) = make_observers(mp_clone, paint, prefix);
            lore::ingest::watch::watch(
                config,
                config_path,
                debounce_secs,
                interval_secs,
                sf,
                Box::new(watch_observer),
                ingest_observer,
            )
            .await
        });
    }

    if set.is_empty() {
        anyhow::bail!("no local sources and no --interval; nothing to watch");
    }

    let mut had_error = false;
    while let Some(join_result) = set.join_next().await {
        match join_result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                progress::mp_println(&mp, format!("[{} ] {e:#}", paint.red("-")));
                had_error = true;
            }
            Err(e) => {
                progress::mp_println(
                    &mp,
                    format!("[{} ] watch task panicked: {e}", paint.red("-")),
                );
                had_error = true;
            }
        }
    }
    if had_error {
        anyhow::bail!("one or more watch sessions failed");
    }
    Ok(())
}
