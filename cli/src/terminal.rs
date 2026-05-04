//! Terminal detection: TTY status, color support, and output width.
//!
//! All terminal-specific probing lives here so the `lore` library crate
//! stays free of I/O and platform syscalls for output formatting.

use std::sync::OnceLock;

use lore::fmt::style::Painter;
use lore::output::OutputMode;

static STDOUT_COLOR: OnceLock<bool> = OnceLock::new();
static STDERR_COLOR: OnceLock<bool> = OnceLock::new();
static STDOUT_TTY: OnceLock<bool> = OnceLock::new();

const MIN_OUTPUT_WIDTH: usize = 40;
const MAX_OUTPUT_WIDTH: usize = 120;

fn detect_color(fd: i32) -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: isatty is always safe to call with a valid fd constant.
        unsafe { libc::isatty(fd) != 0 }
    }
    #[cfg(windows)]
    {
        let _ = fd;
        windows_enable_ansi()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = fd;
        false
    }
}

#[cfg(windows)]
fn windows_enable_ansi() -> bool {
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE,
        SetConsoleMode,
    };
    // SAFETY: GetStdHandle, GetConsoleMode, and SetConsoleMode are safe to call
    // with a valid handle constant; we check for null/invalid before use.
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return false;
        }
        let mut mode: u32 = 0;
        if GetConsoleMode(handle, &raw mut mode) == 0 {
            return false;
        }
        if mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0 {
            return true;
        }
        SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

/// Whether stdout supports ANSI color.
fn detect_color_stdout() -> bool {
    #[cfg(unix)]
    let color = *STDOUT_COLOR.get_or_init(|| detect_color(libc::STDOUT_FILENO));
    #[cfg(not(unix))]
    let color = *STDOUT_COLOR.get_or_init(|| detect_color(0));
    color
}

/// Whether stderr supports ANSI color.
fn detect_color_stderr() -> bool {
    #[cfg(unix)]
    let color = *STDERR_COLOR.get_or_init(|| detect_color(libc::STDERR_FILENO));
    #[cfg(not(unix))]
    let color = *STDERR_COLOR.get_or_init(|| detect_color(2));
    color
}

/// Whether stdout is connected to a TTY.
pub fn is_stdout_tty() -> bool {
    *STDOUT_TTY.get_or_init(|| {
        #[cfg(unix)]
        {
            // SAFETY: isatty is always safe to call with a valid fd constant.
            unsafe { libc::isatty(libc::STDOUT_FILENO) != 0 }
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Console::{
                GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE,
            };
            // SAFETY: GetStdHandle and GetConsoleMode are safe with a valid
            // handle constant; we check for null/invalid before use.
            unsafe {
                let handle = GetStdHandle(STD_OUTPUT_HANDLE);
                if handle.is_null()
                    || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE
                {
                    return false;
                }
                let mut mode: u32 = 0;
                GetConsoleMode(handle, &raw mut mode) != 0
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            false
        }
    })
}

/// Query the terminal size (columns, rows) for a given file descriptor.
///
/// Returns `None` on non-Unix platforms or when the ioctl fails (e.g.
/// the fd is not a TTY).
#[cfg(unix)]
pub fn query_terminal_size(fd: i32) -> Option<(u16, u16)> {
    // SAFETY: TIOCGWINSZ is a read-only ioctl on a valid fd constant.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(fd, libc::TIOCGWINSZ, &raw mut ws) == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
            return Some((ws.ws_col, ws.ws_row));
        }
    }
    None
}

#[cfg(not(unix))]
pub fn query_terminal_size(_fd: i32) -> Option<(u16, u16)> {
    None
}

/// Effective output width.
///
/// When a terminal width is detectable (via `COLUMNS` env var or `TIOCGWINSZ`
/// ioctl on Unix), the value is clamped to `[40, 120]`. When no width source
/// is available (non-TTY without `COLUMNS`), returns `usize::MAX` for
/// unbounded output.
pub fn output_width() -> usize {
    static WIDTH: OnceLock<usize> = OnceLock::new();
    *WIDTH.get_or_init(|| {
        if let Ok(cols) = std::env::var("COLUMNS")
            .as_deref()
            .unwrap_or("")
            .parse::<usize>()
        {
            return cols.clamp(MIN_OUTPUT_WIDTH, MAX_OUTPUT_WIDTH);
        }

        #[cfg(unix)]
        if let Some((cols, _)) = query_terminal_size(libc::STDOUT_FILENO) {
            return (cols as usize).clamp(MIN_OUTPUT_WIDTH, MAX_OUTPUT_WIDTH);
        }

        usize::MAX
    })
}

/// Painter for stderr output (colored brackets, status messages).
pub fn stderr_painter() -> Painter {
    Painter::new(detect_color_stderr())
}

/// Construct `OutputMode::Cli` with auto-detected width and color.
pub fn cli_mode() -> OutputMode {
    OutputMode::Cli {
        width: output_width(),
        color: detect_color_stdout(),
    }
}

/// Construct `OutputMode` from `--json` flag.
pub fn output_mode(json: bool) -> OutputMode {
    if json { OutputMode::Json } else { cli_mode() }
}
