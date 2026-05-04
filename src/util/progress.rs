//! Abstract progress reporting for the ingest pipeline.

use std::sync::Arc;

/// Trait for receiving progress updates from the ingest pipeline.
///
/// Implementors control how progress is displayed -- via `indicatif` bars
/// in the CLI, tracing in the server, or no-ops in tests.
pub trait ProgressSink: Send + Sync {
    /// Advance the progress counter by `n` units.
    fn inc(&self, n: u64);
    /// Increase the total expected length by `n` units.
    fn inc_length(&self, n: u64);
    /// Set the total expected length to exactly `n` units.
    fn set_length(&self, n: u64);
    /// Set the current position to `n` units.
    fn set_position(&self, n: u64);
    /// Set the progress bar prefix label.
    fn set_prefix(&self, s: &str);
    /// Mark this progress bar as finished and clear it from the display.
    fn finish(&self) {}
}

/// Progress handle threaded through the ingest pipeline.
///
/// `Clone + Send + 'static` so it can cross `tokio::spawn` boundaries.
/// Wraps `Option<Arc<dyn ProgressSink>>` -- noop when absent.
#[derive(Clone)]
pub struct ProgressHandle(Option<Arc<dyn ProgressSink>>);

impl ProgressHandle {
    /// Create a handle backed by the given sink.
    pub fn new(sink: Arc<dyn ProgressSink>) -> Self {
        Self(Some(sink))
    }

    /// Create a no-op handle that discards all progress updates.
    pub fn noop() -> Self {
        Self(None)
    }

    /// Advance the progress counter by `n` units.
    pub fn inc(&self, n: u64) {
        if let Some(s) = &self.0 {
            s.inc(n);
        }
    }

    /// Increase the total expected length by `n` units.
    pub fn inc_length(&self, n: u64) {
        if let Some(s) = &self.0 {
            s.inc_length(n);
        }
    }

    /// Set the total expected length to exactly `n` units.
    pub fn set_length(&self, n: u64) {
        if let Some(s) = &self.0 {
            s.set_length(n);
        }
    }

    /// Set the current position to `n` units.
    pub fn set_position(&self, n: u64) {
        if let Some(s) = &self.0 {
            s.set_position(n);
        }
    }

    /// Set the progress bar prefix label.
    pub fn set_prefix(&self, s: impl Into<String>) {
        if let Some(sink) = &self.0 {
            sink.set_prefix(&s.into());
        }
    }

    /// Finish and clear this progress bar.
    pub fn finish(&self) {
        if let Some(s) = &self.0 {
            s.finish();
        }
    }
}
