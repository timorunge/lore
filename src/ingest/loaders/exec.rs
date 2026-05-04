use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::warn;

use crate::ingest::types::{FailedDoc, LoaderResult};
use crate::types::{DocKind, SourceType};
use crate::util::progress::ProgressHandle;

pub(crate) async fn run_exec(
    cmds: &[String],
    dir: Option<&str>,
    env: &HashMap<String, String>,
    timeout_secs: u64,
    topic: Option<&str>,
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
        let stdout = run_single_cmd(cmd, &effective_dir, env, timeout_secs).await?;
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match parse_jsonl_line(line, cmd, topic) {
                Some(doc) => all_docs.push(doc),
                None => failures.push(FailedDoc::new(cmd, "invalid JSONL line")),
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
) -> Result<String> {
    let mut child = tokio::process::Command::new("sh");
    child
        .args(["-c", cmd])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        child.env(k, v);
    }

    let output = tokio::time::timeout(Duration::from_secs(timeout_secs), child.output())
        .await
        .context("exec command timed out")?
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!("sh is not available; exec sources require a POSIX shell on PATH")
            } else {
                anyhow::Error::from(e).context("failed to spawn exec command")
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "exec command {:?} exited with {}: {}",
            cmd,
            output.status,
            stderr.trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_jsonl_line(line: &str, cmd: &str, topic: Option<&str>) -> Option<LoaderResult> {
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
        format: obj["format"].as_str().map(str::to_owned),
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
        let doc = parse_jsonl_line(line, "test", Some("topic")).unwrap();
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
        assert!(parse_jsonl_line(line, "test", None).is_none());
    }

    #[test]
    fn parse_missing_content_returns_none() {
        let line = r#"{"source":"doc/1"}"#;
        assert!(parse_jsonl_line(line, "test", None).is_none());
    }

    #[test]
    fn parse_empty_source_returns_none() {
        let line = r#"{"source":"","content":"hello"}"#;
        assert!(parse_jsonl_line(line, "test", None).is_none());
    }

    #[test]
    fn parse_invalid_json_returns_none() {
        assert!(parse_jsonl_line("{bad json", "test", None).is_none());
    }

    #[test]
    fn parse_tags_as_string() {
        let line = r#"{"source":"x","content":"y","tags":"a, b"}"#;
        let doc = parse_jsonl_line(line, "test", None).unwrap();
        assert_eq!(doc.tags.as_deref(), Some("a, b"));
    }

    #[test]
    fn parse_tags_as_array() {
        let line = r#"{"source":"x","content":"y","tags":["rust","programming"]}"#;
        let doc = parse_jsonl_line(line, "test", None).unwrap();
        assert_eq!(doc.tags.as_deref(), Some("rust, programming"));
    }

    #[test]
    fn parse_kind_defaults_to_document() {
        let line = r#"{"source":"x","content":"y"}"#;
        let doc = parse_jsonl_line(line, "test", None).unwrap();
        assert_eq!(doc.kind, DocKind::Document);
    }

    #[test]
    fn parse_kind_code() {
        let line = r#"{"source":"x","content":"y","kind":"code"}"#;
        let doc = parse_jsonl_line(line, "test", None).unwrap();
        assert_eq!(doc.kind, DocKind::Code);
    }
}
