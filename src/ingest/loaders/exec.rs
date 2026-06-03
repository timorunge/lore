use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::warn;

use crate::config::ExecOutputMode;
use crate::ingest::types::{FailedDoc, LoaderResult};
use crate::types::{DocKind, SourceType};
use crate::util::progress::ProgressHandle;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_exec(
    cmds: &[String],
    dir: Option<&str>,
    env: &HashMap<String, String>,
    timeout_secs: u64,
    max_output_bytes: usize,
    topic: Option<&str>,
    output: ExecOutputMode,
    source_key: Option<&str>,
    format: Option<&str>,
    cwd: &Path,
    progress: &ProgressHandle,
) -> Result<(Vec<LoaderResult>, Vec<FailedDoc>)> {
    let effective_dir = match dir {
        Some(d) => {
            let expanded = crate::config::expand_path(d);
            let p = std::path::PathBuf::from(&expanded);
            if p.is_relative() { cwd.join(p) } else { p }
        }
        None => cwd.to_owned(),
    };

    progress.inc_length(cmds.len() as u64);
    let mut all_docs = Vec::new();
    let mut failures = Vec::new();
    for cmd in cmds {
        let stdout =
            run_single_cmd(cmd, &effective_dir, env, timeout_secs, max_output_bytes).await?;
        match output {
            ExecOutputMode::Jsonl => {
                for line in stdout.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    match parse_jsonl_line(line, cmd, topic, format) {
                        Some(doc) => all_docs.push(doc),
                        None => failures.push(FailedDoc::new(cmd, "invalid JSONL line")),
                    }
                }
            }
            ExecOutputMode::Raw => {
                let content = stdout.trim().to_owned();
                if content.is_empty() {
                    warn!(cmd, "raw exec command produced empty stdout, skipping");
                    failures.push(FailedDoc::new(cmd, "empty stdout in raw mode"));
                } else {
                    let src = source_key.unwrap_or(cmd).to_owned();
                    let source_id = crate::types::source_id(&src);
                    all_docs.push(LoaderResult {
                        source_id,
                        source: src,
                        origin: SourceType::Exec,
                        kind: DocKind::default(),
                        content,
                        unchanged: false,
                        format: format.map(str::to_owned),
                        topic: topic.map(str::to_owned),
                        title: None,
                        author: None,
                        lang: None,
                        created_at: None,
                        tags: None,
                        mtime_ns: None,
                        size_bytes: None,
                        etag: None,
                        last_modified: None,
                        content_hash_override: None,
                    });
                }
            }
        }
        progress.inc(1);
    }
    Ok((all_docs, failures))
}

async fn run_single_cmd(
    cmd: &str,
    dir: &Path,
    env: &HashMap<String, String>,
    timeout_secs: u64,
    max_output_bytes: usize,
) -> Result<String> {
    const STDERR_CAP: usize = 16 * 1024;

    let mut command = tokio::process::Command::new("sh");
    command
        .args(["-c", cmd])
        .current_dir(dir)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        command.env(k, v);
    }

    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!("sh is not available; exec sources require a POSIX shell on PATH")
        } else {
            anyhow::Error::from(e).context("failed to spawn exec command")
        }
    })?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let (stdout_bytes, stderr_bytes, status) =
        tokio::time::timeout(Duration::from_secs(timeout_secs), async {
            use tokio::io::AsyncReadExt;
            let (stdout_res, stderr_res) = tokio::join!(
                async {
                    let mut buf = Vec::new();
                    stdout
                        .take((max_output_bytes as u64).saturating_add(1))
                        .read_to_end(&mut buf)
                        .await
                        .map(|_| buf)
                },
                async {
                    let mut buf = Vec::new();
                    stderr
                        .take((STDERR_CAP as u64).saturating_add(1))
                        .read_to_end(&mut buf)
                        .await
                        .map(|_| buf)
                },
            );
            let s_bytes = stdout_res?;
            let e_bytes = stderr_res?;
            let st = child.wait().await?;
            Ok::<_, std::io::Error>((s_bytes, e_bytes, st))
        })
        .await
        .context("exec command timed out")?
        .map_err(|e| anyhow::Error::from(e).context("failed to read exec command output"))?;

    if stdout_bytes.len() > max_output_bytes {
        anyhow::bail!(
            "exec command exceeded max_output_bytes ({max_output_bytes}); \
             raise exec.max_output_bytes or narrow the command"
        );
    }

    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_bytes);
        anyhow::bail!(
            "exec command {:?} exited with {}: {}",
            cmd,
            status,
            stderr.trim()
        );
    }

    Ok(String::from_utf8_lossy(&stdout_bytes).into_owned())
}

fn parse_jsonl_line(
    line: &str,
    cmd: &str,
    topic: Option<&str>,
    default_format: Option<&str>,
) -> Option<LoaderResult> {
    let obj = match serde_json::from_str::<serde_json::Value>(line) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                cmd,
                line = &line[..line.len().min(80)],
                "invalid JSONL: {e}"
            );
            return None;
        }
    };

    let source = match obj["source"].as_str() {
        Some(s) if !s.is_empty() => s.to_owned(),
        _ => {
            warn!(cmd, "JSONL line missing required 'source' field, skipping");
            return None;
        }
    };
    let Some(content) = obj["content"].as_str() else {
        warn!(
            cmd,
            source, "JSONL line missing required 'content' field, skipping"
        );
        return None;
    };
    let content = content.to_owned();

    let tags = match &obj["tags"] {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(arr) => {
            let joined: String = arr
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            if joined.is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        _ => None,
    };

    let kind = obj["kind"]
        .as_str()
        .and_then(|s| s.parse::<DocKind>().ok())
        .unwrap_or_default();

    let source_id = crate::types::source_id(&source);

    Some(LoaderResult {
        source_id,
        source,
        origin: SourceType::Exec,
        kind,
        content,
        unchanged: false,
        format: obj["format"].as_str().or(default_format).map(str::to_owned),
        topic: topic.map(str::to_owned),
        title: obj["title"].as_str().map(str::to_owned),
        author: obj["author"].as_str().map(str::to_owned),
        lang: obj["lang"].as_str().map(str::to_owned),
        created_at: obj["created_at"].as_str().map(str::to_owned),
        tags,
        mtime_ns: None,
        size_bytes: None,
        etag: None,
        last_modified: None,
        content_hash_override: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_object() {
        let line = r#"{"source":"doc/1","content":"hello world","title":"Hello","author":"alice","lang":"en","format":"md","kind":"code","tags":["a","b"],"created_at":"2026-01-01T00:00:00Z"}"#;
        let doc = parse_jsonl_line(line, "test", Some("topic"), None).unwrap();
        assert_eq!(doc.source, "doc/1");
        assert_eq!(doc.content, "hello world");
        assert_eq!(doc.title.as_deref(), Some("Hello"));
        assert_eq!(doc.author.as_deref(), Some("alice"));
        assert_eq!(doc.lang.as_deref(), Some("en"));
        assert_eq!(doc.format.as_deref(), Some("md"));
        assert_eq!(doc.kind, DocKind::Code);
        assert_eq!(doc.tags.as_deref(), Some("a, b"));
        assert_eq!(doc.created_at.as_deref(), Some("2026-01-01T00:00:00Z"));
        assert_eq!(doc.topic.as_deref(), Some("topic"));
        assert_eq!(doc.origin, SourceType::Exec);
        assert!(!doc.unchanged);
    }

    #[test]
    fn parse_missing_source_returns_none() {
        let line = r#"{"content":"hello"}"#;
        assert!(parse_jsonl_line(line, "test", None, None).is_none());
    }

    #[test]
    fn parse_missing_content_returns_none() {
        let line = r#"{"source":"doc/1"}"#;
        assert!(parse_jsonl_line(line, "test", None, None).is_none());
    }

    #[test]
    fn parse_empty_source_returns_none() {
        let line = r#"{"source":"","content":"hello"}"#;
        assert!(parse_jsonl_line(line, "test", None, None).is_none());
    }

    #[test]
    fn parse_invalid_json_returns_none() {
        assert!(parse_jsonl_line("{bad json", "test", None, None).is_none());
    }

    #[test]
    fn parse_tags_as_string() {
        let line = r#"{"source":"x","content":"y","tags":"a, b"}"#;
        let doc = parse_jsonl_line(line, "test", None, None).unwrap();
        assert_eq!(doc.tags.as_deref(), Some("a, b"));
    }

    #[test]
    fn parse_tags_as_array() {
        let line = r#"{"source":"x","content":"y","tags":["rust","programming"]}"#;
        let doc = parse_jsonl_line(line, "test", None, None).unwrap();
        assert_eq!(doc.tags.as_deref(), Some("rust, programming"));
    }

    #[test]
    fn parse_kind_defaults_to_document() {
        let line = r#"{"source":"x","content":"y"}"#;
        let doc = parse_jsonl_line(line, "test", None, None).unwrap();
        assert_eq!(doc.kind, DocKind::Document);
    }

    #[test]
    fn parse_kind_code() {
        let line = r#"{"source":"x","content":"y","kind":"code"}"#;
        let doc = parse_jsonl_line(line, "test", None, None).unwrap();
        assert_eq!(doc.kind, DocKind::Code);
    }

    #[test]
    fn jsonl_format_fallback() {
        let line = r#"{"source":"x","content":"y"}"#;
        let doc = parse_jsonl_line(line, "test", None, Some("txt")).unwrap();
        assert_eq!(doc.format.as_deref(), Some("txt"));

        let line_with_format = r#"{"source":"x","content":"y","format":"md"}"#;
        let doc = parse_jsonl_line(line_with_format, "test", None, Some("txt")).unwrap();
        assert_eq!(
            doc.format.as_deref(),
            Some("md"),
            "line format wins over default"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn raw_mode_cmd_is_source() {
        let (docs, failures) = run_exec(
            &["echo hello".to_owned()],
            None,
            &HashMap::new(),
            10,
            10 * 1024 * 1024,
            None,
            ExecOutputMode::Raw,
            None,
            None,
            Path::new("/tmp"),
            &ProgressHandle::noop(),
        )
        .await
        .unwrap();
        assert_eq!(docs.len(), 1);
        assert!(failures.is_empty());
        assert_eq!(docs[0].source, "echo hello");
        assert_eq!(docs[0].content, "hello");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn raw_mode_explicit_source_key() {
        let (docs, _) = run_exec(
            &["echo world".to_owned()],
            None,
            &HashMap::new(),
            10,
            10 * 1024 * 1024,
            None,
            ExecOutputMode::Raw,
            Some("my-doc"),
            None,
            Path::new("/tmp"),
            &ProgressHandle::noop(),
        )
        .await
        .unwrap();
        assert_eq!(docs[0].source, "my-doc");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn raw_mode_empty_stdout_yields_failure() {
        let (docs, failures) = run_exec(
            &["true".to_owned()],
            None,
            &HashMap::new(),
            10,
            10 * 1024 * 1024,
            None,
            ExecOutputMode::Raw,
            None,
            None,
            Path::new("/tmp"),
            &ProgressHandle::noop(),
        )
        .await
        .unwrap();
        assert!(docs.is_empty());
        assert_eq!(failures.len(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn raw_mode_topic_and_format() {
        let (docs, _) = run_exec(
            &["echo '# Title'".to_owned()],
            None,
            &HashMap::new(),
            10,
            10 * 1024 * 1024,
            Some("Docs"),
            ExecOutputMode::Raw,
            None,
            Some("md"),
            Path::new("/tmp"),
            &ProgressHandle::noop(),
        )
        .await
        .unwrap();
        assert_eq!(docs[0].topic.as_deref(), Some("Docs"));
        assert_eq!(docs[0].format.as_deref(), Some("md"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_output_within_cap_ok() {
        let (docs, failures) = run_exec(
            &["echo hello".to_owned()],
            None,
            &HashMap::new(),
            10,
            10 * 1024 * 1024,
            None,
            ExecOutputMode::Raw,
            None,
            None,
            Path::new("/tmp"),
            &ProgressHandle::noop(),
        )
        .await
        .unwrap();
        assert_eq!(docs.len(), 1);
        assert!(failures.is_empty());
        assert_eq!(docs[0].content, "hello");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_output_exceeding_cap_errors() {
        // Emit 10 bytes via a POSIX-portable command (no bash brace expansion,
        // which dash -- Ubuntu's /bin/sh -- does not support).
        let result = run_exec(
            &["printf 1234567890".to_owned()],
            None,
            &HashMap::new(),
            10,
            5, // 5-byte cap
            None,
            ExecOutputMode::Raw,
            None,
            None,
            Path::new("/tmp"),
            &ProgressHandle::noop(),
        )
        .await;
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("max_output_bytes"),
            "error should mention max_output_bytes: {err}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_respects_configured_cap() {
        // 'echo hi' outputs "hi\n" (3 bytes), which exceeds a 2-byte cap.
        let result = run_exec(
            &["echo hi".to_owned()],
            None,
            &HashMap::new(),
            10,
            2, // 2-byte cap
            None,
            ExecOutputMode::Raw,
            None,
            None,
            Path::new("/tmp"),
            &ProgressHandle::noop(),
        )
        .await;
        assert!(
            result.is_err(),
            "should fail when output exceeds configured cap"
        );
        assert!(
            result.unwrap_err().to_string().contains("max_output_bytes"),
            "error should mention max_output_bytes"
        );
    }
}
