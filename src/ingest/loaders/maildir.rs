use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use tracing::{debug, warn};

use crate::config::ProcessingLimits;
use crate::ingest::types::{FailedDoc, LoaderResult};
use crate::types::{DocKind, SourceId, SourceType, StampsMeta};

struct MaildirFlags {
    seen: bool,
    replied: bool,
    flagged: bool,
    trashed: bool,
    draft: bool,
    passed: bool,
}

fn parse_flags(filename: &str) -> MaildirFlags {
    let mut flags = MaildirFlags {
        seen: false,
        replied: false,
        flagged: false,
        trashed: false,
        draft: false,
        passed: false,
    };
    if let Some(suffix) = filename.rsplit_once(":2,").map(|(_, f)| f) {
        for ch in suffix.chars() {
            match ch {
                'S' => flags.seen = true,
                'R' => flags.replied = true,
                'F' => flags.flagged = true,
                'T' => flags.trashed = true,
                'D' => flags.draft = true,
                'P' => flags.passed = true,
                _ => {}
            }
        }
    }
    flags
}

fn strip_flags(filename: &str) -> &str {
    filename
        .rsplit_once(":2,")
        .map_or(filename, |(base, _)| base)
}

fn flags_as_tags(flags: &MaildirFlags) -> Option<String> {
    let mut tags = Vec::new();
    if flags.seen {
        tags.push("seen");
    }
    if flags.replied {
        tags.push("replied");
    }
    if flags.flagged {
        tags.push("flagged");
    }
    if flags.trashed {
        tags.push("trashed");
    }
    if flags.draft {
        tags.push("draft");
    }
    if flags.passed {
        tags.push("passed");
    }
    if tags.is_empty() {
        None
    } else {
        Some(tags.join(","))
    }
}

fn mtime_size(path: &Path) -> Option<(i64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime_ns = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_nanos()).ok())?;
    Some((mtime_ns, meta.len() as i64))
}

fn is_unchanged(
    source_key: &str,
    path: &Path,
    existing_stamps: &HashMap<SourceId, StampsMeta>,
) -> bool {
    if existing_stamps.is_empty() {
        return false;
    }
    let sid = crate::types::source_id(source_key);
    let Some(prev) = existing_stamps.get(&sid) else {
        return false;
    };
    let Some((mtime_ns, size)) = mtime_size(path) else {
        return false;
    };
    prev.mtime_ns == Some(mtime_ns) && prev.size_bytes == Some(size)
}

fn extract_text_body(msg: &mail_parser::Message<'_>) -> String {
    if let Some(plain) = msg.body_text(0)
        && !plain.is_empty()
    {
        return plain.into_owned();
    }
    if let Some(html) = msg.body_html(0) {
        return strip_html_tags(&html);
    }
    String::new()
}

fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn format_address(addr: &mail_parser::Address<'_>) -> Option<String> {
    let parts: Vec<String> = addr
        .clone()
        .into_list()
        .iter()
        .map(|a| match (a.name(), a.address()) {
            (Some(name), Some(email)) => format!("{name} <{email}>"),
            (Some(name), None) => name.to_string(),
            (None, Some(email)) => email.to_string(),
            (None, None) => String::new(),
        })
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

fn format_date(dt: &mail_parser::DateTime) -> Option<String> {
    let year = dt.year;
    let month = dt.month;
    let day = dt.day;
    let hour = dt.hour;
    let minute = dt.minute;
    let second = dt.second;
    if year == 0 {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

struct MaildirEntry {
    path: PathBuf,
    source_key: String,
    filename: String,
}

async fn list_subdir(root: &Path, subdir: &str) -> Result<Vec<PathBuf>> {
    let dir = root.join(subdir);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    let mut rd = tokio::fs::read_dir(&dir)
        .await
        .with_context(|| format!("reading {}", dir.display()))?;
    while let Some(entry) = rd.next_entry().await? {
        let ft = entry.file_type().await?;
        if ft.is_file() {
            entries.push(entry.path());
        }
    }
    Ok(entries)
}

fn is_maildir(path: &Path) -> bool {
    path.join("cur").is_dir() || path.join("new").is_dir()
}

async fn discover_maildir_roots(root: &Path) -> Result<Vec<PathBuf>> {
    if is_maildir(root) {
        return Ok(vec![root.to_path_buf()]);
    }
    let mut roots = Vec::new();
    let mut rd = tokio::fs::read_dir(root)
        .await
        .with_context(|| format!("reading {}", root.display()))?;
    while let Some(entry) = rd.next_entry().await? {
        let path = entry.path();
        if path.is_dir() && is_maildir(&path) {
            roots.push(path);
        }
    }
    roots.sort();
    if roots.is_empty() {
        warn!(
            path = %root.display(),
            "no Maildir folders found (no cur/ or new/ subdirectory), skipping"
        );
    } else {
        debug!(
            path = %root.display(),
            folders = roots.len(),
            "discovered Maildir account root"
        );
    }
    Ok(roots)
}

async fn collect_entries(
    maildir_root: &Path,
    rel_root: &str,
    skip_trashed: bool,
    entries: &mut Vec<MaildirEntry>,
) -> Result<()> {
    for subdir in &["new", "cur"] {
        let files = list_subdir(maildir_root, subdir).await?;
        for file_path in files {
            let filename = file_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            if skip_trashed {
                let flags = parse_flags(&filename);
                if flags.trashed {
                    continue;
                }
            }

            let stable_name = strip_flags(&filename);
            let source_key = format!("{rel_root}/{subdir}/{stable_name}");

            entries.push(MaildirEntry {
                path: file_path,
                source_key,
                filename,
            });
        }
    }
    Ok(())
}

pub(crate) async fn load_maildir(
    maildir_paths: &[String],
    skip_trashed: bool,
    topic: Option<&str>,
    limits: &ProcessingLimits,
    existing_stamps: &HashMap<SourceId, StampsMeta>,
    cwd: &Path,
    force: bool,
) -> Result<(Vec<LoaderResult>, Vec<FailedDoc>)> {
    let mut entries: Vec<MaildirEntry> = Vec::new();

    for path_str in maildir_paths {
        let expanded = crate::config::expand_path(path_str);
        let root = Path::new(&expanded);

        let maildir_roots = discover_maildir_roots(root).await?;
        for maildir_root in &maildir_roots {
            let rel_root = crate::util::relativize_path(maildir_root, cwd);
            collect_entries(maildir_root, &rel_root, skip_trashed, &mut entries).await?;
        }
    }

    debug!(count = entries.len(), "maildir messages discovered");

    let empty_stamps = HashMap::new();
    let effective_stamps = if force {
        &empty_stamps
    } else {
        existing_stamps
    };

    let max_bytes = limits.max_file_bytes;
    let topic = topic.map(str::to_owned);
    let failed: Mutex<Vec<FailedDoc>> = Mutex::new(Vec::new());
    let results: Vec<LoaderResult> = stream::iter(entries.into_iter().map(|entry| {
        let topic = topic.clone();
        let failed = &failed;
        async move {
            if is_unchanged(&entry.source_key, &entry.path, effective_stamps) {
                let sid = crate::types::source_id(&entry.source_key);
                return Some(LoaderResult::unchanged_stub(
                    entry.source_key,
                    sid,
                    SourceType::Maildir,
                ));
            }

            if let Some((_, size)) = mtime_size(&entry.path)
                && size as u64 > max_bytes
            {
                debug!(
                    path = %entry.path.display(),
                    size,
                    max = max_bytes,
                    "message exceeds max_file_bytes, skipping"
                );
                failed.lock().expect("not poisoned").push(FailedDoc::new(
                    &entry.source_key,
                    format!("exceeds max_file_bytes ({size} > {max_bytes})"),
                ));
                return None;
            }

            let bytes = match tokio::fs::read(&entry.path).await {
                Ok(b) => b,
                Err(e) => {
                    failed.lock().expect("not poisoned").push(FailedDoc::new(
                        &entry.source_key,
                        format!("failed to read: {e}"),
                    ));
                    return None;
                }
            };

            let Some(msg) = mail_parser::MessageParser::default().parse(&bytes) else {
                failed
                    .lock()
                    .expect("not poisoned")
                    .push(FailedDoc::new(&entry.source_key, "failed to parse message"));
                return None;
            };

            let body = extract_text_body(&msg);
            if body.is_empty() {
                failed
                    .lock()
                    .expect("not poisoned")
                    .push(FailedDoc::new(&entry.source_key, "empty message body"));
                return None;
            }

            let title = msg.subject().map(str::to_owned);
            let author = msg.from().and_then(format_address);
            let created_at = msg.date().and_then(format_date);
            let flags = parse_flags(&entry.filename);
            let tags = flags_as_tags(&flags);

            let (mtime_ns, size_bytes) = mtime_size(&entry.path).unzip();

            let source_id = crate::types::source_id(&entry.source_key);
            Some(LoaderResult {
                source_id,
                source: entry.source_key,
                origin: SourceType::Maildir,
                kind: DocKind::Email,
                content: body,
                unchanged: false,
                format: Some("eml".to_owned()),
                topic,
                title,
                author,
                lang: None,
                created_at,
                tags,
                mtime_ns,
                size_bytes,
                etag: None,
                last_modified: None,
                content_hash_override: None,
            })
        }
    }))
    .buffer_unordered(limits.concurrency)
    .filter_map(|r| async { r })
    .collect()
    .await;

    let failed = failed.into_inner().expect("not poisoned");
    Ok((results, failed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_flags_standard() {
        let flags = parse_flags("1234567890.M123.host:2,RS");
        assert!(flags.replied);
        assert!(flags.seen);
        assert!(!flags.flagged);
        assert!(!flags.trashed);
    }

    #[test]
    fn parse_flags_no_info_suffix() {
        let flags = parse_flags("1234567890.M123.host");
        assert!(!flags.seen);
        assert!(!flags.replied);
    }

    #[test]
    fn strip_flags_removes_suffix() {
        assert_eq!(strip_flags("msg:2,RS"), "msg");
        assert_eq!(strip_flags("msg"), "msg");
    }

    #[test]
    fn flags_as_tags_formats() {
        let flags = MaildirFlags {
            seen: true,
            replied: false,
            flagged: true,
            trashed: false,
            draft: false,
            passed: false,
        };
        assert_eq!(flags_as_tags(&flags).as_deref(), Some("seen,flagged"));
    }

    #[test]
    fn flags_as_tags_empty() {
        let flags = parse_flags("msg");
        assert!(flags_as_tags(&flags).is_none());
    }

    #[test]
    fn strip_html_tags_basic() {
        assert_eq!(strip_html_tags("<p>Hello</p>"), "Hello");
        assert_eq!(strip_html_tags("no tags"), "no tags");
    }
}
