use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::Result;
use indicatif::MultiProgress;
use serde::Serialize;

use lore::config::{IngestConfig, SourceConfig, UpdateMode};
use lore::fmt::plural;
use lore::ingest::loaders::file::list_files;
use lore::store::Store;
use lore::types::{SourceId, SourceType, StampsMeta, source_id};
use lore::util::relativize_path;

use crate::cli::LinePrefix;
use crate::progress;

const MAX_PER_STATUS: usize = 50;

/// Classification of a file's change state relative to the last ingest.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
enum DiffStatus {
    Added,
    Changed,
    Deleted,
}

/// A single diff entry pairing a source path with its change status.
#[derive(Debug, Serialize)]
struct DiffEntry {
    source: String,
    status: DiffStatus,
}

/// Show what would change on next ingest by comparing filesystem state against stored stamps.
pub async fn status(
    store_path: &Path,
    cfg: &IngestConfig,
    json: bool,
    remote: bool,
    prefix: &LinePrefix,
) -> Result<()> {
    let mp = MultiProgress::new();
    let pfx = prefix.to_string();
    let step = if json {
        None
    } else {
        Some(progress::add_step(&mp, &pfx, "checking status..."))
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let (all_stamps, all_docs) = if store_path.is_dir() {
        let store = Store::open_readonly(store_path)?;
        let stamps = (*store.get_all_stamps()).clone();
        let docs = (*store.get_all_documents()).clone();
        (stamps, docs)
    } else {
        (HashMap::new(), HashMap::new())
    };

    let mut entries: Vec<DiffEntry> = Vec::new();
    let mut seen_local_ids: HashSet<SourceId> = HashSet::new();
    let mut unchanged_count: usize = 0;
    let mut skipped_sources: usize = 0;

    for source in &cfg.sources {
        if source.update() == UpdateMode::Never {
            skipped_sources += 1;
            continue;
        }
        let SourceConfig::Local(s) = source else {
            skipped_sources += 1;
            continue;
        };

        let pattern = s.glob.as_deref().unwrap_or("**/*");

        for path_str in &s.path {
            let base = Path::new(path_str);

            if base.is_file() {
                classify_file(
                    base,
                    &cwd,
                    &all_stamps,
                    &mut seen_local_ids,
                    &mut entries,
                    &mut unchanged_count,
                );
                continue;
            }

            if !base.is_dir() {
                continue;
            }

            let paths = match list_files(base, pattern, None).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("failed to list {}: {e}", base.display());
                    continue;
                }
            };

            for file_path in &paths {
                classify_file(
                    file_path,
                    &cwd,
                    &all_stamps,
                    &mut seen_local_ids,
                    &mut entries,
                    &mut unchanged_count,
                );
            }
        }
    }

    for (sid, doc) in &all_docs {
        if doc.origin == SourceType::Local && !seen_local_ids.contains(sid) {
            entries.push(DiffEntry {
                source: doc.source.clone(),
                status: DiffStatus::Deleted,
            });
        }
    }

    entries.sort_by(|a, b| a.status.cmp(&b.status).then(a.source.cmp(&b.source)));

    let remote_diffs = if remote {
        if let Some(s) = &step {
            s.set_message("checking remote sources...");
        }
        lore::ingest::status::check_remote_sources(&cfg.sources, &all_docs, &all_stamps, &cfg.fetch)
            .await
    } else {
        Vec::new()
    };

    if let Some(s) = &step {
        s.finish_and_clear();
    }

    if json {
        let local_json = serde_json::to_value(&entries)?;
        let remote_json = serde_json::to_value(&remote_diffs)?;
        let combined = serde_json::json!({
            "local": local_json,
            "remote": remote_json,
        });
        println!("{}", serde_json::to_string_pretty(&combined)?);
    } else {
        print_status_summary(
            &entries,
            unchanged_count,
            skipped_sources,
            &remote_diffs,
            remote,
            prefix,
        );
    }

    Ok(())
}

/// Print a human-readable status summary with per-status counts and overflow markers.
fn print_status_summary(
    entries: &[DiffEntry],
    unchanged_count: usize,
    skipped_sources: usize,
    remote_diffs: &[lore::ingest::status::RemoteDiff],
    remote_checked: bool,
    prefix: &LinePrefix,
) {
    let paint = crate::terminal::stderr_painter();

    let mut counts = [0usize; 3];
    for entry in entries {
        let (idx, marker, label) = match entry.status {
            DiffStatus::Added => (0, paint.green("+"), "added"),
            DiffStatus::Changed => (1, paint.yellow("!"), "changed"),
            DiffStatus::Deleted => (2, paint.red("x"), "deleted"),
        };
        if counts[idx] < MAX_PER_STATUS {
            eprintln!("{prefix}[{marker} ] {label:<8} {}", entry.source);
        }
        counts[idx] += 1;
    }
    let labels = ["added", "changed", "deleted"];
    for i in 0..3 {
        if counts[i] > MAX_PER_STATUS {
            eprintln!(
                "{prefix}     ... and {} more {}",
                counts[i] - MAX_PER_STATUS,
                labels[i]
            );
        }
    }

    for diff in remote_diffs {
        if let Some(err) = &diff.error {
            eprintln!(
                "{prefix}[{} ] {:<8} {} ({})",
                paint.red("-"),
                "error",
                diff.source_label,
                err,
            );
        } else if diff.has_changes() {
            let mut parts = Vec::new();
            if diff.added > 0 {
                parts.push(format!("{} added", diff.added));
            }
            if diff.changed > 0 {
                parts.push(format!("{} changed", diff.changed));
            }
            if diff.deleted > 0 {
                parts.push(format!("{} deleted", diff.deleted));
            }
            let marker = if diff.added > 0 {
                paint.green("+")
            } else if diff.deleted > 0 {
                paint.red("x")
            } else {
                paint.yellow("!")
            };
            eprintln!(
                "{prefix}[{marker} ] {} ({})",
                diff.source_label,
                parts.join(", "),
            );
        } else {
            eprintln!(
                "{prefix}[{} ] no changes ({})",
                paint.blue("i"),
                diff.source_label,
            );
        }
    }

    let [added, changed, deleted] = counts;
    let total_changes = added + changed + deleted;

    let skipped_suffix = if !remote_checked && skipped_sources > 0 {
        format!(
            ", {} remote source{} skipped",
            skipped_sources,
            plural(skipped_sources)
        )
    } else {
        String::new()
    };

    let remote_suffix = if remote_checked && !remote_diffs.is_empty() {
        let r_added: usize = remote_diffs.iter().map(|d| d.added).sum();
        let r_changed: usize = remote_diffs.iter().map(|d| d.changed).sum();
        let r_deleted: usize = remote_diffs.iter().map(|d| d.deleted).sum();
        let r_errors: usize = remote_diffs.iter().filter(|d| d.error.is_some()).count();
        let mut parts = Vec::new();
        if r_added > 0 {
            parts.push(format!("{r_added} added"));
        }
        if r_changed > 0 {
            parts.push(format!("{r_changed} changed"));
        }
        if r_deleted > 0 {
            parts.push(format!("{r_deleted} deleted"));
        }
        if r_errors > 0 {
            parts.push(format!("{r_errors} error{}", plural(r_errors)));
        }
        if parts.is_empty() {
            " | remote: no changes".to_owned()
        } else {
            format!(" | remote: {}", parts.join(", "))
        }
    } else {
        String::new()
    };

    if total_changes == 0 {
        eprintln!(
            "{prefix}[{} ] no changes ({unchanged_count} unchanged{skipped_suffix}{remote_suffix})",
            paint.blue("i")
        );
    } else {
        eprintln!(
            "{prefix}[{} ] {added} added, {changed} changed, {deleted} deleted ({unchanged_count} unchanged{skipped_suffix}{remote_suffix})",
            paint.blue("i")
        );
    }
}

/// Classify a single file as added, changed, or unchanged and record it.
fn classify_file(
    path: &Path,
    cwd: &Path,
    stamps: &HashMap<SourceId, StampsMeta>,
    seen: &mut HashSet<SourceId>,
    entries: &mut Vec<DiffEntry>,
    unchanged: &mut usize,
) {
    let key = relativize_path(path, cwd);
    let sid = source_id(&key);
    if !seen.insert(sid.clone()) {
        return;
    }

    match classify_by_stat(path, &sid, stamps) {
        Some(status) => entries.push(DiffEntry {
            source: key,
            status,
        }),
        None => *unchanged += 1,
    }
}

/// Compare a file's mtime and size against stored stamps to detect changes.
fn classify_by_stat(
    path: &Path,
    sid: &SourceId,
    stamps: &HashMap<SourceId, StampsMeta>,
) -> Option<DiffStatus> {
    let Some(stamp) = stamps.get(sid) else {
        return Some(DiffStatus::Added);
    };

    let Ok(meta) = std::fs::metadata(path) else {
        return Some(DiffStatus::Added);
    };

    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_nanos()).ok());

    if let (Some(m), Some(sm)) = (mtime_ns, stamp.mtime_ns)
        && let Some(sz) = stamp.size_bytes
        && m == sm
        && meta.len() as i64 == sz
    {
        None
    } else {
        Some(DiffStatus::Changed)
    }
}
