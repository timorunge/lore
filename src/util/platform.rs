use std::time::Duration;

/// Disables terminal echo and canonical mode on stdin for the duration of
/// ingest to prevent keypresses from corrupting the progress bar display.
///
/// On Unix, `new` returns `None` when stdin is not a TTY. On non-Unix
/// platforms the type is a no-op that always returns `Some(Self)`.
#[cfg(unix)]
pub struct SuppressStdin {
    original: libc::termios,
    fd: std::os::unix::io::RawFd,
}

#[cfg(unix)]
impl SuppressStdin {
    /// Disables echo and canonical mode on stdin; returns `None` when stdin is not a TTY.
    pub fn new() -> Option<Self> {
        use std::os::unix::io::AsRawFd;
        let fd = std::io::stdin().as_raw_fd();
        // SAFETY: `fd` is stdin's raw fd, which remains valid for the duration
        // of this call. `original` is zero-initialized before being passed as
        // an out-pointer to `tcgetattr`.
        unsafe {
            if libc::isatty(fd) == 0 {
                return None;
            }
            let mut original: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &raw mut original) != 0 {
                return None;
            }
            let mut modified = original;
            modified.c_lflag &= !(libc::ECHO | libc::ICANON);
            if libc::tcsetattr(fd, libc::TCSANOW, &raw const modified) != 0 {
                return None;
            }
            Some(Self { original, fd })
        }
    }
}

#[cfg(unix)]
impl Drop for SuppressStdin {
    fn drop(&mut self) {
        // SAFETY: `self.fd` was validated as a live TTY fd in `new`, and
        // `self.original` is the termios state saved at construction time.
        unsafe {
            if libc::tcsetattr(self.fd, libc::TCSANOW, &raw const self.original) != 0 {
                eprintln!("[! ] failed to restore terminal settings");
            }
        }
    }
}

/// No-op stdin suppression for non-Unix platforms.
#[cfg(not(unix))]
pub struct SuppressStdin;

#[cfg(not(unix))]
impl SuppressStdin {
    /// Always returns `Some(Self)` on non-Unix platforms.
    pub fn new() -> Option<Self> {
        Some(Self)
    }
}

/// Return peak RSS and CPU time (user + system) from getrusage.
#[cfg(unix)]
pub fn resource_usage() -> (u64, Duration) {
    // SAFETY: `RUSAGE_SELF` is always a valid target, and `usage` is
    // zero-initialized to satisfy the out-pointer contract of `getrusage`.
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &raw mut usage);
        // macOS: ru_maxrss is in bytes. Linux: in kilobytes.
        let peak_rss = if cfg!(target_os = "macos") {
            usage.ru_maxrss as u64
        } else {
            usage.ru_maxrss as u64 * 1024
        };
        let cpu = Duration::from_secs(usage.ru_utime.tv_sec.max(0) as u64)
            + Duration::from_micros(usage.ru_utime.tv_usec.max(0) as u64)
            + Duration::from_secs(usage.ru_stime.tv_sec.max(0) as u64)
            + Duration::from_micros(usage.ru_stime.tv_usec.max(0) as u64);
        (peak_rss, cpu)
    }
}

/// Stub for non-Unix platforms where getrusage is not available.
#[cfg(not(unix))]
pub fn resource_usage() -> (u64, Duration) {
    (0, Duration::ZERO)
}

#[cfg(unix)]
static STDERR_SUPPRESS: std::sync::Mutex<StderrState> = std::sync::Mutex::new(StderrState {
    count: 0,
    saved_fd: -1,
});

/// Reference-counted state for the stderr-suppression guard.
#[cfg(unix)]
struct StderrState {
    count: usize,
    saved_fd: libc::c_int,
}

/// RAII guard that restores stderr to the original file descriptor when the last concurrent holder drops.
pub struct StderrGuard;

impl Drop for StderrGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Ok(mut state) = STDERR_SUPPRESS.lock() {
            state.count -= 1;
            if state.count == 0 && state.saved_fd >= 0 {
                // SAFETY: `state.saved_fd` is a valid fd saved by
                // `suppress_native_stderr`; restoring it to `STDERR_FILENO`
                // and closing the duplicate is the correct teardown sequence.
                unsafe {
                    libc::dup2(state.saved_fd, libc::STDERR_FILENO);
                    libc::close(state.saved_fd);
                }
                state.saved_fd = -1;
            }
        }
    }
}

/// Temporarily redirect fd 2 (stderr) to `/dev/null` to suppress noise from
/// native C libraries (pdfium, tesseract) that write directly to stderr.
///
/// Reference-counted: the first call saves the real stderr and redirects;
/// concurrent callers just increment the count. The last guard to drop
/// restores the original fd. Safe with `buffer_unordered` concurrency.
pub fn suppress_native_stderr() -> Option<StderrGuard> {
    #[cfg(unix)]
    {
        let mut state = STDERR_SUPPRESS.lock().ok()?;
        if state.count == 0 {
            // SAFETY: `STDERR_FILENO` (2) is always open, `dup`/`dup2`/`open`
            // return values are checked before use, and the lock ensures only
            // one thread performs the save-and-redirect at a time.
            unsafe {
                let saved = libc::dup(libc::STDERR_FILENO);
                if saved < 0 {
                    return None;
                }
                let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
                if devnull < 0 {
                    libc::close(saved);
                    return None;
                }
                libc::dup2(devnull, libc::STDERR_FILENO);
                libc::close(devnull);
                state.saved_fd = saved;
            }
        }
        state.count += 1;
        Some(StderrGuard)
    }
    #[cfg(not(unix))]
    {
        Some(StderrGuard)
    }
}

/// Write a message to the real stderr even when it is suppressed.
///
/// During PDF extraction, fd 2 points to `/dev/null`. This function writes
/// directly to the saved original fd so signal-handler messages are visible
/// without restoring fd 2 globally (which would let in-flight C library
/// noise leak through).
#[cfg(unix)]
pub fn write_to_real_stderr(msg: &str) {
    use std::io::Write;
    use std::os::unix::io::FromRawFd;
    if let Ok(state) = STDERR_SUPPRESS.lock()
        && state.saved_fd >= 0
    {
        // Write to saved fd (real terminal), not fd 2 (/dev/null).
        // SAFETY: `state.saved_fd` was produced by `dup` in
        // `suppress_native_stderr` and is still open; `mem::forget`
        // below prevents `File` from closing the fd we don't own.
        let mut f = unsafe { std::fs::File::from_raw_fd(state.saved_fd) };
        f.write_all(msg.as_bytes()).ok();
        // Don't let File close the fd -- it's owned by the suppress state.
        std::mem::forget(f);
        return;
    }
    // Not suppressed -- write normally.
    eprint!("{msg}");
}

/// Non-unix fallback: writes directly to stderr.
#[cfg(not(unix))]
pub fn write_to_real_stderr(msg: &str) {
    eprint!("{msg}");
}

/// Permanently redirect stderr to `/dev/null`. Call right before
/// `std::process::exit()` so C library destructors (tesseract, pdfium)
/// don't leak warnings during static cleanup.
pub fn silence_stderr_for_exit() {
    #[cfg(unix)]
    // SAFETY: called immediately before `process::exit`; redirecting fd 2 to
    // `/dev/null` here is intentional and the process is about to terminate.
    unsafe {
        let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        if devnull >= 0 {
            libc::dup2(devnull, libc::STDERR_FILENO);
            libc::close(devnull);
        }
    }
}
