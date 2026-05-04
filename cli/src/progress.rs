use std::sync::{Arc, Mutex};
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use lore::util::progress::{ProgressHandle, ProgressSink};

use crate::terminal;

/// Tick interval for the waiting state (slow blink).
pub(crate) const TICK_WAITING: Duration = Duration::from_millis(800);

/// Tick interval for active spinners.
pub(crate) const TICK_ACTIVE: Duration = Duration::from_millis(100);

fn style(template: &str) -> ProgressStyle {
    ProgressStyle::with_template(template).expect("hardcoded template is valid")
}

/// Source line: waiting state -- blinking purple dot `[. ] [  ]`
pub(crate) fn source_waiting() -> ProgressStyle {
    let p = terminal::stderr_painter();
    let tick0 = format!("[{} ]", p.purple("."));
    style("{prefix}{spinner} {msg}").tick_strings(&[&tick0, "[  ]", &tick0])
}

/// Source line: active state -- animated spinner `[- ] [\ ] [| ] [/ ]`
pub(crate) fn source_active() -> ProgressStyle {
    let p = terminal::stderr_painter();
    let ticks = ["-", "\\", "|", "/"].map(|c| format!("[{} ]", p.blue(c)));
    style("{prefix}{spinner} {msg}").tick_strings(&[&ticks[0], &ticks[1], &ticks[2], &ticks[3]])
}

/// Source line: done state -- `[+ ]` (green)
pub(crate) fn source_done() -> ProgressStyle {
    let marker = format!("[{} ]", terminal::stderr_painter().green("+"));
    style(&format!("{{prefix}}{marker} {{msg}}"))
}

/// Source line: error state -- `[x ]` (red)
pub(crate) fn source_error() -> ProgressStyle {
    let marker = format!("[{} ]", terminal::stderr_painter().red("x"));
    style(&format!("{{prefix}}{marker} {{msg}}"))
}

/// Header status line -- plain message that updates in place.
pub(crate) fn status_line() -> ProgressStyle {
    style("{msg}")
}

/// Write a line through `MultiProgress` if it has an active draw target,
/// otherwise fall back to direct stderr. This ensures messages are visible
/// both in interactive (TTY) and non-interactive (piped/test) contexts.
pub(crate) fn mp_println(mp: &MultiProgress, msg: impl AsRef<str>) {
    if mp.is_hidden() {
        eprintln!("{}", msg.as_ref());
    } else {
        mp.println(msg.as_ref()).ok();
    }
}

/// Create an active spinner step below the current progress area.
pub(crate) fn add_step(mp: &MultiProgress, pfx: &str, msg: &str) -> ProgressBar {
    let pb = mp.add(ProgressBar::new_spinner());
    pb.set_style(source_active());
    pb.set_prefix(pfx.to_owned());
    pb.set_message(msg.to_owned());
    pb.enable_steady_tick(TICK_ACTIVE);
    pb
}

/// Clear a spinner step and print a `[+ ]` completion line.
pub(crate) fn finish_step(mp: &MultiProgress, pb: &ProgressBar, pfx: &str, msg: &str) {
    pb.finish_and_clear();
    let paint = terminal::stderr_painter();
    mp_println(mp, format!("{pfx}[{} ] {msg}", paint.green("+")));
}

/// Which sub-step a progress handle drives.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepKind {
    Fetching,
    Enriching,
    Indexing,
}

impl StepKind {
    pub(crate) fn from_str(s: &str) -> Option<Self> {
        match s {
            "fetching" => Some(Self::Fetching),
            "enriching" => Some(Self::Enriching),
            "indexing" => Some(Self::Indexing),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Fetching => "fetch",
            Self::Enriching => "enr",
            Self::Indexing => "idx",
        }
    }

    fn char_label(self) -> char {
        match self {
            Self::Fetching => 'F',
            Self::Enriching => 'E',
            Self::Indexing => 'I',
        }
    }
}

/// Per-sub-step counters held inside [`SourceState`].
pub(crate) struct SubStepState {
    pub(crate) pos: u64,
    pub(crate) len: u64,
}

impl SubStepState {
    fn new() -> Self {
        Self { pos: 0, len: 0 }
    }
}

/// Aggregated state for a single source's indicatif bar.
///
/// One bar, multiple virtual sub-steps rendered inline in its message.
pub(crate) struct SourceState {
    pub(crate) label: String,
    pub(crate) steps: [(StepKind, SubStepState); 3],
    pub(crate) term_width: usize,
    pub(crate) pb: ProgressBar,
}

impl SourceState {
    pub(crate) fn new(pb: ProgressBar, label: String, term_width: usize) -> Self {
        Self {
            label,
            steps: [
                (StepKind::Fetching, SubStepState::new()),
                (StepKind::Enriching, SubStepState::new()),
                (StepKind::Indexing, SubStepState::new()),
            ],
            term_width,
            pb,
        }
    }

    /// Render the inline message showing all active sub-steps.
    ///
    /// 5-tier progressive degradation:
    /// T1: label + short names (fetch/idx/enr) + 8-wide bars
    /// T2: label + short names + 5-wide bars
    /// T3: label + char names (F/I/E) + 5-wide bars
    /// T4: no label + char names + 5-wide bars
    /// T5: no label + char names + no bars
    pub(crate) fn render(&self) -> String {
        let active = self.active_steps();
        let tw = self.term_width;

        if active.is_empty() {
            return self.label.clone();
        }

        for (use_label, bar_w, chars) in [
            (true, Some(8), false),
            (true, Some(5), false),
            (true, Some(5), true),
            (false, Some(5), true),
            (false, None, true),
        ] {
            let steps_str = Self::render_steps(&active, bar_w, chars);
            let label = if use_label {
                let steps_w = lore::fmt::visible_width(&steps_str);
                let label_budget = tw.saturating_sub(5 + 2 + steps_w);
                truncate_label(&self.label, label_budget)
            } else {
                String::new()
            };
            if let Some(s) = Self::try_render(&label, &steps_str, tw) {
                return s;
            }
        }
        Self::render_steps(&active, None, true)
    }

    fn step_mut(&mut self, kind: StepKind) -> &mut SubStepState {
        &mut self
            .steps
            .iter_mut()
            .find(|(k, _)| *k == kind)
            .expect("step kind always present")
            .1
    }

    fn active_steps(&self) -> Vec<&(StepKind, SubStepState)> {
        self.steps.iter().filter(|(_, s)| s.len > 0).collect()
    }

    fn render_steps(
        steps: &[&(StepKind, SubStepState)],
        bar_width: Option<usize>,
        char_labels: bool,
    ) -> String {
        steps
            .iter()
            .map(|(kind, step)| {
                let name = if char_labels {
                    kind.char_label().to_string()
                } else {
                    kind.label().to_owned()
                };
                let counter = compact_counter(step.pos, step.len);
                match bar_width {
                    Some(w) => {
                        let bar = render_bar(step.pos, step.len, w);
                        format!("{name} {bar} {counter}")
                    }
                    None => format!("{name} {counter}"),
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
    }

    fn try_render(label: &str, steps_str: &str, tw: usize) -> Option<String> {
        let s = if steps_str.is_empty() {
            label.to_owned()
        } else if label.is_empty() {
            steps_str.to_owned()
        } else {
            format!("{label}  {steps_str}")
        };
        if lore::fmt::visible_width(&s) <= tw {
            Some(s)
        } else {
            None
        }
    }
}

/// `ProgressSink` implementation that drives a sub-step within a shared [`SourceState`].
pub(crate) struct SubStepSink {
    pub(crate) state: Arc<Mutex<SourceState>>,
    pub(crate) kind: StepKind,
}

impl ProgressSink for SubStepSink {
    fn inc(&self, n: u64) {
        if n == 0 {
            return;
        }
        let mut s = self.state.lock().expect("mutex should not be poisoned");
        s.step_mut(self.kind).pos += n;
        let msg = s.render();
        s.pb.set_message(msg);
    }

    fn inc_length(&self, n: u64) {
        if n == 0 {
            return;
        }
        let mut s = self.state.lock().expect("mutex should not be poisoned");
        s.step_mut(self.kind).len += n;
        let msg = s.render();
        s.pb.set_message(msg);
    }

    fn set_length(&self, n: u64) {
        let mut s = self.state.lock().expect("mutex should not be poisoned");
        let step = s.step_mut(self.kind);
        if step.len == n {
            return;
        }
        step.len = n;
        let msg = s.render();
        s.pb.set_message(msg);
    }

    fn set_position(&self, n: u64) {
        let mut s = self.state.lock().expect("mutex should not be poisoned");
        let step = s.step_mut(self.kind);
        if step.pos == n {
            return;
        }
        step.pos = n;
        let msg = s.render();
        s.pb.set_message(msg);
    }

    fn set_prefix(&self, _s: &str) {}

    fn finish(&self) {}
}

/// Manages indicatif bars for a set of sources.
#[derive(Clone)]
pub(crate) struct SourceBars {
    mp: MultiProgress,
    sources: Arc<Mutex<Vec<Arc<Mutex<SourceState>>>>>,
    term_width: usize,
}

impl SourceBars {
    pub(crate) fn new(mp: MultiProgress, term_width: usize) -> Self {
        Self {
            mp,
            sources: Arc::new(Mutex::new(Vec::new())),
            term_width,
        }
    }

    pub(crate) fn clear(&self) {
        self.sources
            .lock()
            .expect("mutex should not be poisoned")
            .clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.sources
            .lock()
            .expect("mutex should not be poisoned")
            .len()
    }

    pub(crate) fn add_waiting(&self, label: &str) {
        self.add_source(label, source_waiting(), TICK_WAITING);
    }

    pub(crate) fn add_active(&self, label: &str) {
        self.add_source(label, source_active(), TICK_ACTIVE);
    }

    /// Add a new source progress bar with the given style and tick interval.
    fn add_source(&self, label: &str, style: ProgressStyle, tick: Duration) {
        let pb = self.mp.add(ProgressBar::new_spinner());
        pb.set_style(style);
        pb.set_message(label.to_owned());
        pb.enable_steady_tick(tick);
        let state = Arc::new(Mutex::new(SourceState::new(
            pb,
            label.to_owned(),
            self.term_width,
        )));
        self.sources
            .lock()
            .expect("mutex should not be poisoned")
            .push(state);
    }

    pub(crate) fn activate(&self, index: usize) {
        let sources = self.sources.lock().expect("mutex should not be poisoned");
        if let Some(state) = sources.get(index) {
            let s = state.lock().expect("mutex should not be poisoned");
            s.pb.set_style(source_active());
            s.pb.enable_steady_tick(TICK_ACTIVE);
        }
    }

    pub(crate) fn done(
        &self,
        index: usize,
        docs: u64,
        chunks: u64,
        finish: bool,
    ) -> Option<String> {
        let msg_fn = |label: &str| {
            if docs > 0 {
                format!(
                    "{label} -- {} doc{}, {} chunk{}",
                    docs,
                    lore::fmt::plural(docs),
                    chunks,
                    lore::fmt::plural(chunks),
                )
            } else {
                format!("{label} -- up to date")
            }
        };
        self.finish_source(index, source_done(), msg_fn, finish)
    }

    pub(crate) fn empty(&self, index: usize, finish: bool) -> Option<String> {
        self.finish_source(
            index,
            source_done(),
            |label| format!("{label} -- no documents"),
            finish,
        )
    }

    pub(crate) fn error(&self, index: usize, finish: bool) -> Option<String> {
        self.finish_source(index, source_error(), str::to_owned, finish)
    }

    pub(crate) fn set_active_msg(&self, index: usize, msg: &str) {
        let sources = self.sources.lock().expect("mutex should not be poisoned");
        if let Some(state) = sources.get(index) {
            let s = state.lock().expect("mutex should not be poisoned");
            s.pb.set_message(msg.to_owned());
        }
    }

    pub(crate) fn set_done_msg(&self, index: usize, msg: &str, finish: bool) {
        let m = msg.to_owned();
        self.finish_source(index, source_done(), |_| m, finish);
    }

    fn finish_source(
        &self,
        index: usize,
        style: ProgressStyle,
        msg_fn: impl FnOnce(&str) -> String,
        finish: bool,
    ) -> Option<String> {
        let sources = self.sources.lock().expect("mutex should not be poisoned");
        let state = sources.get(index)?;
        let s = state.lock().expect("mutex should not be poisoned");
        s.pb.set_style(style);
        let msg = msg_fn(&s.label);
        s.pb.set_message(msg.clone());
        if finish {
            s.pb.finish();
        }
        Some(msg)
    }

    pub(crate) fn create_progress(&self, source_index: usize, label: &str) -> ProgressHandle {
        let Some(kind) = StepKind::from_str(label) else {
            return ProgressHandle::noop();
        };
        let sources = self.sources.lock().expect("mutex should not be poisoned");
        let Some(state) = sources.get(source_index).cloned() else {
            return ProgressHandle::noop();
        };
        ProgressHandle::new(Arc::new(SubStepSink { state, kind }))
    }

    pub(crate) fn finish_and_clear_all(&self) {
        for state in self
            .sources
            .lock()
            .expect("mutex should not be poisoned")
            .iter()
        {
            state
                .lock()
                .expect("mutex should not be poisoned")
                .pb
                .finish_and_clear();
        }
    }

    pub(crate) fn tick_all(&self) {
        for state in self
            .sources
            .lock()
            .expect("mutex should not be poisoned")
            .iter()
        {
            state
                .lock()
                .expect("mutex should not be poisoned")
                .pb
                .tick();
        }
    }
}

/// Format a `pos/len` counter string using compact number formatting.
fn compact_counter(pos: u64, len: u64) -> String {
    format!(
        "{}/{}",
        lore::fmt::format_count(pos),
        lore::fmt::format_count(len)
    )
}

/// Truncate a source label to fit `max_width` visible columns.
///
/// Labels have the structure `[i/n] <path> (<type>)`. When truncation is
/// needed the path is shortened from the left -- the end of the path (the
/// actual source name) is more informative than the common prefix.
fn truncate_label(label: &str, max_width: usize) -> String {
    let w = lore::fmt::visible_width(label);
    if w <= max_width {
        return label.to_owned();
    }
    if max_width < 4 {
        return String::new();
    }

    let (prefix, rest) = match label.find("] ") {
        Some(i) => (&label[..i + 2], &label[i + 2..]),
        None => ("", label),
    };
    let (path, suffix) = match rest.rfind(" (") {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };

    let prefix_w = lore::fmt::visible_width(prefix);
    let suffix_w = lore::fmt::visible_width(suffix);

    // prefix + ellipsis + shortened_path + suffix
    let path_budget = max_width.saturating_sub(prefix_w + suffix_w + 1);
    if path_budget >= 3 {
        let tail: String = path
            .chars()
            .rev()
            .take(path_budget)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return format!("{prefix}\u{2026}{tail}{suffix}");
    }

    // prefix + ellipsis + shortened_path (drop suffix)
    let path_budget = max_width.saturating_sub(prefix_w + 1);
    if path_budget >= 3 {
        let tail: String = path
            .chars()
            .rev()
            .take(path_budget)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return format!("{prefix}\u{2026}{tail}");
    }

    // last resort: right-truncate the whole thing
    let mut result: String = label.chars().take(max_width.saturating_sub(1)).collect();
    result.truncate(result.trim_end().len());
    result.push('\u{2026}');
    result
}

/// Render a compact ASCII progress bar with color: `[=====>---------]`
pub(crate) fn render_bar(pos: u64, len: u64, width: usize) -> String {
    let p = terminal::stderr_painter();
    if len == 0 || width == 0 {
        return format!("[{}]", p.dim(&format!("{:->w$}", "", w = width)));
    }
    let filled = ((pos as f64 / len as f64) * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width.saturating_sub(filled);
    if filled == width {
        format!("[{}]", p.blue(&format!("{:=>w$}", "", w = width)))
    } else if filled > 0 {
        format!(
            "[{}{}]",
            p.blue(&format!("{:=>fw$}>", "", fw = filled.saturating_sub(1))),
            p.dim(&format!("{:->ew$}", "", ew = empty)),
        )
    } else {
        format!("[{}]", p.dim(&format!("{:->w$}", "", w = width)))
    }
}
