use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::AsyncReadExt;
use tracing::warn;

use crate::cache;
use crate::util::progress::ProgressHandle;

/// Build a sandboxed `git` subprocess with credential prompts and system config disabled.
fn git_command(args: &[&str], cwd: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        // Disable interactive credential prompts -- stdout/stderr are
        // captured so the user would never see them. Without this, git
        // hangs waiting for input that can never arrive.
        .env("GIT_TERMINAL_PROMPT", "0")
        // Prevent user/system gitconfig from causing unexpected behaviour
        // (e.g. custom credential helpers, hooks, or aliases).
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // Suppress any credential-helper GUI that ignores TERMINAL_PROMPT.
        // /nonexistent is intentional: it names a program that cannot be
        // executed, so git falls back to no credential prompting rather than
        // prompting interactively or launching a GUI helper.
        .env("GIT_ASKPASS", "/nonexistent");
    cmd
}

/// Map an I/O error from spawning git into a user-friendly message.
fn map_spawn_error(e: std::io::Error) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        anyhow::anyhow!("git is not installed; git sources require git on PATH")
    } else {
        anyhow::Error::from(e).context("failed to run git")
    }
}

/// Run a git command, capturing stdout, with a timeout.
///
/// When `progress` is provided, stderr is streamed and parsed for git
/// progress updates (percentage, object counts).  Otherwise stderr is
/// captured and included in the error message on failure.
async fn run_git_cmd(
    args: &[&str],
    cwd: &Path,
    progress: Option<&ProgressHandle>,
    timeout_secs: u64,
) -> Result<String> {
    let timeout = std::time::Duration::from_secs(timeout_secs);

    let Some(pb) = progress else {
        let output = tokio::time::timeout(timeout, git_command(args, cwd).output())
            .await
            .context("git operation timed out")?
            .map_err(map_spawn_error)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git {:?} failed: {}", args, stderr.trim());
        }
        let stdout = String::from_utf8(output.stdout).context("git output is not valid UTF-8")?;
        return Ok(stdout.trim().to_owned());
    };

    let mut child = git_command(args, cwd)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(map_spawn_error)?;

    let mut stderr_handle = child.stderr.take().context("missing stderr pipe")?;
    let mut stdout_handle = child.stdout.take().context("missing stdout pipe")?;

    let pb_clone = pb.clone();
    let stderr_task = tokio::spawn(async move {
        let mut buf = [0u8; 512];
        let mut accumulated = String::new();
        let mut all_stderr = String::new();
        loop {
            let n = match stderr_handle.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let chunk = String::from_utf8_lossy(&buf[..n]);
            all_stderr.push_str(&chunk);
            accumulated.push_str(&chunk);

            while let Some(pos) = accumulated.find(['\r', '\n']) {
                let line = accumulated[..pos].to_owned();
                update_progress_from_token(&line, &pb_clone);
                accumulated.drain(..=pos);
            }
        }
        if !accumulated.is_empty() {
            update_progress_from_token(&accumulated, &pb_clone);
        }
        all_stderr
    });

    let stdout_task = tokio::spawn(async move {
        let mut out = String::new();
        stdout_handle.read_to_string(&mut out).await.ok();
        out
    });

    let status = tokio::time::timeout(timeout, child.wait())
        .await
        .context("git operation timed out")?
        .context("failed to wait on git process")?;

    let stderr_output = stderr_task.await.unwrap_or_default();
    let stdout_output = stdout_task.await.unwrap_or_default();

    if !status.success() {
        anyhow::bail!("git {:?} failed: {}", args, stderr_output.trim());
    }

    Ok(stdout_output.trim().to_owned())
}

/// Parse a git progress line like `Receiving objects:  42% (42/100), 1.2 MiB`
/// and update the progress token position, length, and prefix (phase name).
fn update_progress_from_token(line: &str, pb: &ProgressHandle) {
    if let Some(colon) = line.find(':') {
        let phase = line[..colon].trim();
        if !phase.is_empty() {
            pb.set_prefix(phase);
        }
    }
    let Some(open) = line.find('(') else { return };
    let Some(close) = line[open..].find(')') else {
        return;
    };
    let inner = &line[open + 1..open + close];
    let Some((pos_str, total_str)) = inner.split_once('/') else {
        return;
    };
    let Ok(pos) = pos_str.trim().parse::<u64>() else {
        return;
    };
    let Ok(total) = total_str.trim().parse::<u64>() else {
        return;
    };
    if total > 0 {
        pb.set_length(total);
        pb.set_position(pos);
    }
}

/// Open the lock file for a cross-process exclusive lock on the git cache directory.
fn open_lock_file(cache_path: &Path) -> Result<fd_lock::RwLock<std::fs::File>> {
    let lock_path = cache_path.with_extension("lock");
    if let Some(parent) = lock_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        warn!(path = %parent.display(), "failed to create lock file directory: {e}");
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("failed to open lock file: {}", lock_path.display()))?;
    Ok(fd_lock::RwLock::new(file))
}

/// Clone a new git repository or fetch updates if already cached, then checkout the ref.
pub(crate) async fn clone_or_fetch(
    repo_url: &str,
    git_ref: Option<&str>,
    timeout_secs: u64,
    progress: &ProgressHandle,
) -> Result<std::path::PathBuf> {
    let cache_path = cache::repo_cache_path(repo_url)?;

    // Cross-process lock to prevent concurrent clone/fetch of the same repo
    let mut lock = open_lock_file(&cache_path)?;
    let _guard = lock
        .write()
        .map_err(|e| anyhow::anyhow!("failed to acquire git lock: {e}"))?;

    if cache_path.join(".git").exists() {
        let mut fetch_args = vec!["fetch", "--progress", "--depth", "1", "origin"];
        if let Some(r) = git_ref {
            fetch_args.push(r);
        }
        run_git_cmd(&fetch_args, &cache_path, Some(progress), timeout_secs).await?;
    } else {
        let path_str = cache_path.to_str().context("non-UTF-8 path")?;
        let mut args = vec!["clone", "--progress", "--no-checkout", "--depth", "1"];
        if let Some(r) = git_ref {
            args.extend_from_slice(&["--branch", r, "--single-branch"]);
        } else {
            args.push("--single-branch");
        }
        args.extend_from_slice(&[repo_url, path_str]);
        if let Err(e) = run_git_cmd(&args, Path::new("."), Some(progress), timeout_secs).await {
            // Clean up partial directory left by the failed clone.
            if let Err(rm_err) = tokio::fs::remove_dir_all(&cache_path).await {
                warn!(path = %cache_path.display(), "failed to clean up partial clone: {rm_err}");
            }
            return Err(e);
        }

        // Set remote HEAD so `origin/HEAD` resolves correctly
        if let Err(e) = run_git_cmd(
            &["remote", "set-head", "origin", "--auto"],
            &cache_path,
            None,
            timeout_secs,
        )
        .await
        {
            warn!(repo = repo_url, "failed to set remote HEAD: {e}");
        }
    }

    let checkout_ref = git_ref.unwrap_or("origin/HEAD");
    run_git_cmd(
        &["checkout", "--quiet", "--force", checkout_ref],
        &cache_path,
        None,
        timeout_secs,
    )
    .await?;

    Ok(cache_path)
}

/// Return the full SHA of the current HEAD commit in the given repository.
pub(crate) async fn get_head_commit(repo_path: &Path, timeout_secs: u64) -> Result<String> {
    run_git_cmd(&["rev-parse", "HEAD"], repo_path, None, timeout_secs).await
}

/// Query the remote HEAD SHA without cloning the repository.
pub(crate) async fn git_ls_remote(
    repo_url: &str,
    git_ref: Option<&str>,
    timeout_secs: u64,
) -> Result<String> {
    let ref_arg = git_ref.unwrap_or("HEAD");
    let output = run_git_cmd(
        &["ls-remote", repo_url, ref_arg],
        Path::new("."),
        None,
        timeout_secs,
    )
    .await?;
    output
        .split_whitespace()
        .next()
        .map(str::to_owned)
        .context("git ls-remote returned no output")
}
