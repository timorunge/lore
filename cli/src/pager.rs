//! External pager support for piping CLI output through `less`, `bat`, etc.

use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// Resolve which pager command to use, if any.
///
/// Priority: `--pager` flag > `LORE_PAGER` env > `PAGER` env > `None`.
/// Returns `None` when paging is disabled (`--no-pager`, non-TTY stdout,
/// or no pager configured).
pub fn resolve_pager(flag: Option<&str>, no_pager: bool) -> Option<String> {
    if no_pager {
        return None;
    }
    if !crate::terminal::is_stdout_tty() {
        return None;
    }
    if let Some(cmd) = flag {
        let trimmed = cmd.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    for var in ["LORE_PAGER", "PAGER"] {
        if let Ok(val) = std::env::var(var) {
            let trimmed = val.trim().to_owned();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

/// Write text through an external pager, or directly to stdout.
///
/// Falls back to direct stdout if no pager command is given.
pub fn page_output(text: &str, pager_cmd: Option<&str>) -> Result<()> {
    if let Some(cmd) = pager_cmd {
        run_pager(text, cmd)
    } else {
        println!("{text}");
        Ok(())
    }
}

/// Extract the binary name (without path) from a pager command string.
fn pager_bin(cmd: &str) -> &str {
    let first_word = cmd.split_whitespace().next().unwrap_or("");
    first_word.rsplit('/').next().unwrap_or(first_word)
}

/// Spawn a pager subprocess and pipe text into it.
///
/// Sets `LESS=R` as a default (ANSI passthrough) unless the user already
/// has a `LESS` env var. Also sets `LESSCHARSET=UTF-8` and
/// `LESSANSIENDCHARS=mK`. Falls back to direct stdout on spawn failure
/// or when the pager binary is not found. Handles BrokenPipe (user quit
/// pager) gracefully.
fn run_pager(text: &str, cmd: &str) -> Result<()> {
    let bin = pager_bin(cmd);
    let is_self = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .is_some_and(|name| name == bin);
    if is_self {
        let paint = crate::terminal::stderr_painter();
        eprintln!(
            "[{} ] pager is set to {:?} which would cause a loop, ignoring",
            paint.yellow("!"),
            bin,
        );
        println!("{text}");
        return Ok(());
    }

    let mut c = shell_command(cmd);
    c.stdin(Stdio::piped()).stderr(Stdio::piped());
    set_less_env(&mut c);
    let spawn_result = c.spawn();

    let mut child = match spawn_result {
        Ok(c) => c,
        Err(e) => {
            let paint = crate::terminal::stderr_painter();
            eprintln!("[{} ] pager \"{bin}\" not found: {e}", paint.yellow("!"));
            println!("{text}");
            return Ok(());
        }
    };

    let mut broken_pipe = false;
    if let Some(mut stdin) = child.stdin.take() {
        match stdin.write_all(text.as_bytes()) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                broken_pipe = true;
            }
            Err(e) => {
                drop(stdin);
                child.wait().ok();
                return Err(e).context("failed to write to pager stdin");
            }
        }
    }

    drop(child.stdin.take());
    let status = child.wait().context("failed to wait for pager process")?;

    if broken_pipe && status.success() {
        return Ok(());
    }

    if !status.success() {
        let paint = crate::terminal::stderr_painter();
        if status.code() == Some(127) {
            eprintln!("[{} ] pager \"{bin}\" not found", paint.yellow("!"));
        } else {
            eprintln!("[{} ] pager exited with {status}", paint.yellow("!"));
        }
        println!("{text}");
    }

    Ok(())
}

/// Set less-friendly environment variables on a pager command.
///
/// `LESS=R` provides ANSI passthrough as a default. The user's own `LESS`
/// env var takes precedence (we only set it when absent).
fn set_less_env(cmd: &mut Command) {
    cmd.env("LESSCHARSET", "UTF-8");
    if std::env::var("LESS").is_err() {
        cmd.env("LESS", "R");
    }
    if std::env::var("LESSANSIENDCHARS").is_err() {
        cmd.env("LESSANSIENDCHARS", "mK");
    }
}

/// Build a shell-wrapped command for the given pager string.
fn shell_command(cmd: &str) -> Command {
    #[cfg(unix)]
    {
        let mut c = Command::new("sh");
        c.args(["-c", cmd]);
        c
    }
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.args(["/C", cmd]);
        c
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = cmd;
        let mut c = Command::new("false");
        c
    }
}
