use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget};
use tokio::sync::mpsc;

use lore::fmt::style::Painter;
use lore::fmt::{format_bytes, format_elapsed, plural};
use lore::ingest::{self, IngestMode, IngestObserver, IngestResult};
use lore::util::platform::{SuppressStdin, write_to_real_stderr};
use lore::util::progress::ProgressHandle;

use crate::cli::ResolvedConfig;
use crate::cli::drift::DriftEvent;
use crate::progress::{self, SourceBars};
use crate::terminal;

/// Maximum number of individual failure details shown inline in the terminal.
const MAX_INLINE_FAILURES: usize = 20;

struct CliIngestObserver {
    mp: MultiProgress,
    paint: Painter,
    shutdown: Arc<AtomicBool>,
    header_bar: Arc<std::sync::Mutex<ProgressBar>>,
    status_bar: Arc<std::sync::Mutex<ProgressBar>>,
    bars: SourceBars,
    is_multi: bool,
    kb_prefix: std::sync::Mutex<String>,
    completed: std::sync::Mutex<Vec<IngestResult>>,
    last_status: std::sync::Mutex<String>,
    done_lines: std::sync::Mutex<Vec<(usize, String)>>,
    deferred: Arc<std::sync::Mutex<Vec<String>>>,
    drift_active: Arc<AtomicBool>,
    drift_tx: Option<mpsc::UnboundedSender<DriftEvent>>,
    _stdin_guard: Option<SuppressStdin>,
}

impl CliIngestObserver {
    fn new(
        mp: MultiProgress,
        quiet: bool,
        drift_tx: Option<mpsc::UnboundedSender<DriftEvent>>,
        is_multi: bool,
    ) -> Self {
        let paint = terminal::stderr_painter();
        let shutdown = Arc::new(AtomicBool::new(false));
        let stdin_guard = if quiet { None } else { SuppressStdin::new() };
        let term_width = terminal::output_width();

        crate::cli::spawn_signal_handlers(&shutdown, &mp);

        let bars = SourceBars::new(mp.clone(), term_width);

        Self {
            mp,
            paint,
            shutdown: shutdown.clone(),
            header_bar: Arc::new(std::sync::Mutex::new(ProgressBar::hidden())),
            status_bar: Arc::new(std::sync::Mutex::new(ProgressBar::hidden())),
            bars,
            is_multi,
            kb_prefix: std::sync::Mutex::new(String::new()),
            completed: std::sync::Mutex::new(Vec::new()),
            last_status: std::sync::Mutex::new(String::new()),
            done_lines: std::sync::Mutex::new(Vec::new()),
            deferred: Arc::new(std::sync::Mutex::new(Vec::new())),
            drift_active: Arc::new(AtomicBool::new(false)),
            drift_tx,
            _stdin_guard: stdin_guard,
        }
    }

    /// Return the current KB prefix string for log lines.
    fn kb_prefix(&self) -> String {
        self.kb_prefix
            .lock()
            .expect("mutex should not be poisoned")
            .clone()
    }

    /// Print a line, deferring if drift animation is active.
    fn println(&self, msg: impl AsRef<str>) {
        if self.drift_active.load(Ordering::Relaxed) {
            self.deferred
                .lock()
                .expect("mutex should not be poisoned")
                .push(msg.as_ref().to_owned());
        } else {
            progress::mp_println(&self.mp, msg);
        }
    }
}

impl IngestObserver for CliIngestObserver {
    fn shutdown_flag(&self) -> &AtomicBool {
        &self.shutdown
    }

    fn create_progress(&self, source_index: usize, label: &str, _len: u64) -> ProgressHandle {
        self.bars.create_progress(source_index, label)
    }

    fn remove_progress(&self, _handle: &ProgressHandle) {}

    fn on_start(&self, kb_name: &str, n_sources: usize, mode: IngestMode, _dry_run: bool) {
        self.bars.clear();
        self.last_status
            .lock()
            .expect("mutex should not be poisoned")
            .clear();
        self.done_lines
            .lock()
            .expect("mutex should not be poisoned")
            .clear();

        let prefix = if self.is_multi {
            format!("{} ", self.paint.dim(&format!("[{kb_name}]")))
        } else {
            String::new()
        };
        self.kb_prefix
            .lock()
            .expect("mutex should not be poisoned")
            .clone_from(&prefix);

        let mut header = self
            .header_bar
            .lock()
            .expect("mutex should not be poisoned");
        header.finish_and_clear();
        let hb = self.mp.add(ProgressBar::new_spinner());
        hb.set_style(progress::source_waiting());
        hb.set_prefix(prefix.clone());
        hb.set_message(format!(
            "ingesting \"{kb_name}\" ({n_sources} source{}, mode: {})",
            plural(n_sources),
            mode.as_str(),
        ));
        hb.enable_steady_tick(progress::TICK_WAITING);
        *header = hb;

        let mut status = self
            .status_bar
            .lock()
            .expect("mutex should not be poisoned");
        status.finish_and_clear();
        let sb = self.mp.add(ProgressBar::new_spinner());
        sb.set_style(progress::status_line());
        *status = sb;
    }

    fn on_dry_run_notice(&self) {
        let prefix = self.kb_prefix();
        self.println(format!(
            "{prefix}[{} ] no changes will be written (dry run)",
            self.paint.blue("i")
        ));
    }

    fn on_status(&self, msg: &str) {
        let formatted = format!("[{} ] {}", self.paint.blue("i"), msg);
        self.last_status
            .lock()
            .expect("mutex should not be poisoned")
            .clone_from(&formatted);
        self.status_bar
            .lock()
            .expect("mutex should not be poisoned")
            .set_message(formatted);
    }

    fn on_source_waiting(&self, _index: usize, label: &str) {
        let prefix = self.kb_prefix();
        self.bars.add_waiting(&format!("{prefix}{label}"));
    }

    fn on_source_active(&self, index: usize) {
        self.bars.activate(index);
    }

    fn on_source_done(&self, index: usize, _label: &str, docs: u64, chunks: u64, docs_failed: u64) {
        if let Some(msg) = self.bars.done(index, docs, chunks, false) {
            let sigil = if docs_failed > 0 {
                self.paint.yellow("!")
            } else {
                self.paint.green("+")
            };
            let done = format!("[{sigil} ] {msg}");
            self.done_lines
                .lock()
                .expect("mutex should not be poisoned")
                .push((index, done));
        }
    }

    fn on_source_empty(&self, index: usize, _label: &str) {
        if let Some(msg) = self.bars.empty(index, false) {
            let done = format!("[{} ] {msg}", self.paint.green("+"));
            self.done_lines
                .lock()
                .expect("mutex should not be poisoned")
                .push((index, done));
        }
    }

    fn on_source_error(&self, index: usize, _label: &str, _error: &str) {
        if let Some(msg) = self.bars.error(index, false) {
            let done = format!("[{} ] {msg}", self.paint.red("x"));
            self.done_lines
                .lock()
                .expect("mutex should not be poisoned")
                .push((index, done));
        }
    }

    fn on_optimize_start(&self, segments: usize) {
        self.bars
            .add_active(&format!("optimizing index ({segments} segments)"));
    }

    fn on_optimize_progress(&self, segments: usize) {
        let last = self.bars.len().saturating_sub(1);
        self.bars
            .set_active_msg(last, &format!("optimizing index ({segments} segments)"));
    }

    fn on_optimize_done(&self) {
        let last = self.bars.len().saturating_sub(1);
        self.bars.set_done_msg(last, "optimizing index", false);
        let done = format!("[{} ] optimizing index", self.paint.green("+"));
        self.done_lines
            .lock()
            .expect("mutex should not be poisoned")
            .push((usize::MAX, done));
    }

    fn on_interrupted(&self) {
        self.header_bar
            .lock()
            .expect("mutex should not be poisoned")
            .finish_and_clear();
        self.mp.set_draw_target(ProgressDrawTarget::hidden());
    }

    fn on_interrupted_result(&self, msg: &str, success: bool) {
        if success {
            write_to_real_stderr(&format!("[{} ] {msg}\n", self.paint.green("+")));
        } else {
            write_to_real_stderr(&format!("[{} ] {msg}\n", self.paint.red("x")));
        }
    }

    fn on_complete(&self, result: &IngestResult, store_path: &Path, index_size: u64) {
        let status_msg = self
            .last_status
            .lock()
            .expect("mutex should not be poisoned")
            .clone();
        let mut done_lines = std::mem::take(
            &mut *self
                .done_lines
                .lock()
                .expect("mutex should not be poisoned"),
        );
        done_lines.sort_by_key(|(idx, _)| *idx);

        {
            let hb = self
                .header_bar
                .lock()
                .expect("mutex should not be poisoned");
            let msg = hb.message();
            let prefix = hb.prefix();
            hb.finish_and_clear();
            let static_line = format!("{prefix}[{} ] {msg}", self.paint.purple("."));
            self.println(&static_line);
        }
        self.status_bar
            .lock()
            .expect("mutex should not be poisoned")
            .finish_and_clear();
        self.bars.finish_and_clear_all();

        if !status_msg.is_empty() {
            self.println(&status_msg);
        }
        for (_, line) in &done_lines {
            self.println(line);
        }

        let index_size_str = if index_size > 0 {
            format!(" ({})", format_bytes(index_size))
        } else {
            String::new()
        };

        if self.is_multi {
            self.completed
                .lock()
                .expect("mutex should not be poisoned")
                .push(result.clone());
            let prefix = self.kb_prefix();
            print_failed_docs(self, &prefix, result);
            self.println(format!(
                "{prefix}[{} ] {} document{}, {} chunk{} in {} -> {}{index_size_str}",
                self.paint.blue("i"),
                result.documents,
                plural(result.documents),
                result.chunks,
                plural(result.chunks),
                format_elapsed(result.elapsed),
                store_path.display(),
            ));
        } else {
            self.println(format!(
                "[{} ] {} document{}, {} chunk{} in {} -> {}{index_size_str}",
                self.paint.blue("i"),
                result.documents,
                plural(result.documents),
                result.chunks,
                plural(result.chunks),
                format_elapsed(result.elapsed),
                store_path.display(),
            ));
            print_failed_docs(self, "", result);
            print_throughput_stats(
                &self.mp,
                self.paint,
                result.documents,
                result.chunks,
                result.elapsed,
            );
        }
    }

    fn on_source_errors(&self, errors: &[(String, String)]) {
        let prefix = self.kb_prefix();
        let n = errors.len();
        let max_shown = 10;
        self.println(format!(
            "{prefix}[{} ] {n} source{} failed:",
            self.paint.red("x"),
            plural(n)
        ));
        for (label, err) in errors.iter().take(max_shown) {
            self.println(format!("  {label}"));
            let mut buf = String::new();
            lore::fmt::write_wrapped(&mut buf, err, "    ", usize::MAX);
            self.println(buf.trim_end());
        }
        if n > max_shown {
            self.println(format!("  ... and {} more", n - max_shown));
        }
    }

    fn on_final_status(&self, status: &str) {
        let prefix = self.kb_prefix();
        match status {
            "dry_run_done" => {
                self.println(format!(
                    "{prefix}[{} ] no changes have been written (dry run)",
                    self.paint.blue("i")
                ));
            }
            "up_to_date" => {
                self.println(format!("{prefix}[{} ] up to date", self.paint.purple(".")));
            }
            "no_documents" => {
                self.println(format!(
                    "{prefix}[{} ] no documents indexed -- check source paths/globs or run `lore preview` to debug",
                    self.paint.blue("i"),
                ));
            }
            _ => {}
        }
    }

    fn on_document_indexed(&self, source: &str, _chunks: u64) {
        if let Some(tx) = &self.drift_tx {
            tx.send(DriftEvent::Doc(source.to_owned())).ok();
        }
    }
}

/// Print inline failure details and write the full list to a JSONL log in the cache directory.
fn print_failed_docs(observer: &CliIngestObserver, prefix: &str, result: &IngestResult) {
    if result.failed_docs.is_empty() {
        return;
    }
    let n = result.failed_docs.len();

    let wrote_log = write_failures_log(&result.failed_docs);

    observer.println(format!(
        "{prefix}[{} ] {} document{} failed:",
        observer.paint.yellow("!"),
        n,
        plural(n),
    ));

    let term_width = terminal::output_width();
    // "[! ] " = 5 chars, "  " indent = 2 chars, " " separator = 1 char
    // prefix contains ANSI codes so measure its display width via strip
    let prefix_visible = strip_ansi(prefix).chars().count();
    let overhead = prefix_visible + 2 + 1; // indent + separator
    let usable = term_width.saturating_sub(overhead);
    // Give source at most 60% of the space, reason gets the rest (at least 20 chars each).
    let source_budget = (usable * 3 / 5).max(20).min(usable.saturating_sub(20));
    let reason_budget = usable.saturating_sub(source_budget).max(20);

    for fd in result.failed_docs.iter().take(MAX_INLINE_FAILURES) {
        let src = lore::util::truncate_left_chars(&fd.source, source_budget);
        let reason = lore::util::truncate_chars(&fd.reason, reason_budget);
        observer.println(format!(
            "{prefix}     {} {}",
            observer.paint.dim(&src),
            observer.paint.yellow(reason),
        ));
    }
    if n > MAX_INLINE_FAILURES {
        if let Some(ref path) = wrote_log {
            observer.println(format!(
                "{prefix}     ... and {} more (see {})",
                n - MAX_INLINE_FAILURES,
                path.display(),
            ));
        } else {
            observer.println(format!(
                "{prefix}     ... and {} more",
                n - MAX_INLINE_FAILURES
            ));
        }
    } else if let Some(ref path) = wrote_log {
        observer.println(format!("{prefix}     full log: {}", path.display()));
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            if ch.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else if ch == '\x1b' {
            in_escape = true;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Write all failures as JSONL to a dated file in the cache logs dir.
/// Returns the path on success, `None` on failure.
fn write_failures_log(failures: &[lore::ingest::types::FailedDoc]) -> Option<std::path::PathBuf> {
    use std::io::Write;
    let dir = lore::cache::logs_dir().ok()?;
    let ts = lore::util::iso8601_now().replace(':', "-");
    let path = dir.join(format!("failures-{ts}.jsonl"));
    let mut f = std::fs::File::create(&path).ok()?;
    for fd in failures {
        let obj = serde_json::json!({ "source": fd.source, "reason": fd.reason });
        writeln!(f, "{obj}").ok()?;
    }
    Some(path)
}

/// Print docs/s, chunks/s, peak memory, and CPU time via `mp.println`.
fn print_throughput_stats(
    mp: &MultiProgress,
    paint: Painter,
    docs: u64,
    chunks: u64,
    elapsed: std::time::Duration,
) {
    if docs > 0 && elapsed.as_secs() > 0 {
        let docs_per_sec = docs as f64 / elapsed.as_secs_f64();
        let chunks_per_sec = chunks as f64 / elapsed.as_secs_f64();
        let (peak_rss, cpu_time) = lore::util::platform::resource_usage();
        progress::mp_println(
            mp,
            format!(
                "[{} ] {docs_per_sec:.0} docs/s, {chunks_per_sec:.0} chunks/s, peak mem {}, cpu {}",
                paint.purple("."),
                format_bytes(peak_rss),
                format_elapsed(cpu_time),
            ),
        );
    }
}

/// Run ingest for one or more knowledge bases using a single shared observer.
#[allow(clippy::fn_params_excessive_bools)]
pub async fn ingest(
    configs: &[ResolvedConfig],
    recreate: bool,
    dry_run: bool,
    force: bool,
    quiet: bool,
    source_filter: Option<&str>,
) -> Result<()> {
    let is_multi = configs.len() > 1;
    let mp = if quiet {
        MultiProgress::with_draw_target(ProgressDrawTarget::hidden())
    } else {
        MultiProgress::new()
    };
    let (drift_tx, drift_rx) = mpsc::unbounded_channel::<DriftEvent>();
    let observer = Arc::new(CliIngestObserver::new(
        mp.clone(),
        quiet,
        Some(drift_tx),
        is_multi,
    ));
    let drift_handle = spawn_drift(&mp, quiet, drift_rx, &observer);

    let mut had_error = false;
    let multi_start = std::time::Instant::now();

    for rc in configs {
        let mode = if recreate {
            IngestMode::Recreate
        } else {
            IngestMode::Update
        };
        if let Err(e) = ingest::ingest(
            rc.config.clone(),
            rc.config_path.clone(),
            mode,
            dry_run,
            force,
            source_filter.map(String::from),
            observer.clone() as Arc<dyn IngestObserver>,
        )
        .await
        {
            if is_multi {
                self::mp_println_error(&mp, observer.paint, &observer.kb_prefix(), &e);
                had_error = true;
            } else {
                return Err(e);
            }
        }
        if observer.shutdown.load(Ordering::SeqCst) {
            break;
        }
    }

    let (total_docs, total_chunks, done_count) = {
        let completed = observer
            .completed
            .lock()
            .expect("mutex should not be poisoned");
        let docs: u64 = completed.iter().map(|c| c.documents).sum();
        let chunks: u64 = completed.iter().map(|c| c.chunks).sum();
        let count = completed.len();
        (docs, chunks, count)
    };
    let paint = observer.paint;
    drop(observer);
    crate::cli::drift::await_or_abort(drift_handle).await;

    if is_multi {
        print_multi_summary(
            &mp,
            paint,
            total_docs,
            total_chunks,
            done_count,
            multi_start.elapsed(),
        );
        if had_error {
            anyhow::bail!("one or more ingests failed");
        }
    } else if !terminal::is_stdout_tty() && total_docs > 0 {
        println!(
            "ingest: {total_docs} documents, {total_chunks} chunks, {:.1}s",
            multi_start.elapsed().as_secs_f64()
        );
    }

    Ok(())
}

/// Spawn the drift animation and build the redraw callback.
fn spawn_drift(
    mp: &MultiProgress,
    quiet: bool,
    drift_rx: mpsc::UnboundedReceiver<DriftEvent>,
    observer: &CliIngestObserver,
) -> Option<crate::cli::drift::DriftHandle> {
    let redraw_header = observer.header_bar.clone();
    let redraw_status = observer.status_bar.clone();
    let redraw_bars = observer.bars.clone();
    let deferred = observer.deferred.clone();
    let drift_active = observer.drift_active.clone();
    let drift_mp = mp.clone();
    crate::cli::drift::maybe_spawn(mp, quiet, drift_rx, drift_active, move || {
        for line in std::mem::take(&mut *deferred.lock().expect("mutex should not be poisoned")) {
            drift_mp.println(&line).ok();
        }
        redraw_header
            .lock()
            .expect("mutex should not be poisoned")
            .tick();
        redraw_status
            .lock()
            .expect("mutex should not be poisoned")
            .tick();
        redraw_bars.tick_all();
    })
}

/// Print the aggregate summary for a multi-KB ingest.
fn print_multi_summary(
    mp: &MultiProgress,
    paint: Painter,
    total_docs: u64,
    total_chunks: u64,
    done_count: usize,
    total_elapsed: std::time::Duration,
) {
    progress::mp_println(mp, "");
    progress::mp_println(
        mp,
        format!(
            "[{} ] {total_docs} document{}, {total_chunks} chunk{} across {done_count} knowledge base{} in {}",
            paint.blue("i"),
            plural(total_docs),
            plural(total_chunks),
            plural(done_count),
            format_elapsed(total_elapsed),
        ),
    );
    print_throughput_stats(mp, paint, total_docs, total_chunks, total_elapsed);

    if !terminal::is_stdout_tty() {
        println!(
            "ingest: {done_count} knowledge bases, {total_docs} documents, {total_chunks} chunks"
        );
    }
}

fn mp_println_error(mp: &MultiProgress, paint: Painter, prefix: &str, e: &anyhow::Error) {
    progress::mp_println(mp, format!("{prefix}[{} ] {e:#}", paint.red("x")));
}
