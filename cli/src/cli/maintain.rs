use std::path::Path;

use anyhow::{Context, Result};
use indicatif::MultiProgress;
use serde::Serialize;

use lore::cache::{CacheScope, clear_cache};
use lore::config::StoreConfig;
use lore::fmt::{format_bytes, plural, to_json_pretty};
use lore::output::OutputMode;
use lore::store;
use lore::types::SourceId;
use lore::util::normalize_path;

use crate::cli::LinePrefix;
use crate::progress;

const MAX_PER_KIND: usize = 50;

/// Category of store consistency issue detected during maintenance.
#[derive(PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
enum IssueKind {
    CountMismatch,
    MissingChunks,
    OrphanedChunks,
}

/// JSON report emitted when `--json` is requested.
#[derive(Serialize)]
struct Report<'a> {
    issues: &'a [Issue],
    fixed: usize,
}

/// Detected issue for reporting.
#[derive(Serialize)]
struct Issue {
    #[serde(skip)]
    source_id: SourceId,
    source: String,
    kind: IssueKind,
    detail: String,
}

impl std::fmt::Display for IssueKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::CountMismatch => "count_mismatch",
            Self::MissingChunks => "missing_chunks",
            Self::OrphanedChunks => "orphaned_chunks",
        };
        f.write_str(s)
    }
}

fn ensure_store_exists(store_path: &Path) -> Result<()> {
    if !store_path.is_dir() {
        anyhow::bail!("no store found at {}", store_path.display());
    }
    Ok(())
}

/// Clear cached downloads, git repos, or temporary files.
pub fn clean(scope: CacheScope) -> Result<()> {
    let mp = MultiProgress::new();
    let paint = crate::terminal::stderr_painter();

    progress::mp_println(&mp, format!("[{} ] cleaning cache", paint.purple(".")));

    let step = progress::add_step(&mp, "", "cleaning...");
    let (count, bytes) = clear_cache(scope)?;
    let s = if count == 1 { "" } else { "s" };
    progress::finish_step(
        &mp,
        &step,
        "",
        &format!("cleared {count} cached item{s} ({})", format_bytes(bytes)),
    );
    Ok(())
}

/// Check store consistency (read-only).
pub fn check(store_path: &Path, mode: OutputMode, prefix: &LinePrefix) -> Result<()> {
    let store_path = &normalize_path(store_path);
    ensure_store_exists(store_path)?;
    let json = mode == OutputMode::Json;
    let mp = MultiProgress::new();
    let paint = crate::terminal::stderr_painter();
    let pfx = prefix.to_string();

    if !json {
        progress::mp_println(
            &mp,
            format!(
                "{prefix}[{} ] checking store consistency",
                paint.purple(".")
            ),
        );
    }

    let store = store::Store::open_readonly(store_path).context("failed to open store")?;

    let step = progress::add_step(&mp, &pfx, "checking consistency...");
    let mut issues = detect_issues(&store)?;
    issues.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.source.cmp(&b.source)));
    let issue_msg = if issues.is_empty() {
        "no issues found".to_owned()
    } else {
        format!("{} issue{} found", issues.len(), plural(issues.len()))
    };
    if json {
        step.finish_and_clear();
    } else {
        progress::finish_step(&mp, &step, &pfx, &issue_msg);
        report_issues(&mp, &issues, false, prefix);
    }

    let segs = store.segment_count();
    if !json && segs > 1 {
        progress::mp_println(
            &mp,
            format!(
                "{prefix}[{} ] {segs} segment{}, run `lore maintain compact` to merge",
                paint.blue("i"),
                plural(segs),
            ),
        );
    }

    if json {
        let report = Report {
            issues: &issues,
            fixed: 0,
        };
        println!("{}", to_json_pretty(&report)?);
    }

    Ok(())
}

/// Fix detected consistency issues.
pub fn repair(
    store_path: &Path,
    store_config: &StoreConfig,
    mode: OutputMode,
    prefix: &LinePrefix,
) -> Result<()> {
    let store_path = &normalize_path(store_path);
    ensure_store_exists(store_path)?;
    let json = mode == OutputMode::Json;
    let mp = MultiProgress::new();
    let paint = crate::terminal::stderr_painter();
    let pfx = prefix.to_string();

    if !json {
        progress::mp_println(
            &mp,
            format!("{prefix}[{} ] repairing store", paint.purple(".")),
        );
    }

    let store = store::Store::open(
        store_path,
        store_config.phrase_search,
        store_config.writer_heap_mb,
        store_config.language,
        store_config.doc_store_cache_blocks,
    )
    .context("failed to open store")?;

    let step = progress::add_step(&mp, &pfx, "checking consistency...");
    let mut issues = detect_issues(&store)?;
    issues.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.source.cmp(&b.source)));
    let has_mismatch = issues.iter().any(|i| i.kind == IssueKind::CountMismatch);
    let issue_msg = if issues.is_empty() {
        "no issues found".to_owned()
    } else {
        format!("{} issue{} found", issues.len(), plural(issues.len()))
    };
    if json {
        step.finish_and_clear();
    } else {
        progress::finish_step(&mp, &step, &pfx, &issue_msg);
        report_issues(&mp, &issues, true, prefix);
    }

    let fixed = if issues.is_empty() {
        0
    } else {
        let n = issues.len();
        let step = progress::add_step(&mp, &pfx, &format!("fixing {n} issue{}...", plural(n)));
        let fixed = fix_issues(&store, &issues)?;
        if json {
            step.finish_and_clear();
        } else {
            progress::finish_step(
                &mp,
                &step,
                &pfx,
                &format!("fixed {fixed} issue{}", plural(fixed)),
            );
            if has_mismatch {
                progress::mp_println(
                    &mp,
                    format!(
                        "{prefix}[{} ] run `lore ingest` to re-index cleared documents",
                        paint.blue("i")
                    ),
                );
            }
        }
        fixed
    };

    if json {
        let report = Report {
            issues: &issues,
            fixed,
        };
        println!("{}", to_json_pretty(&report)?);
    }

    Ok(())
}

/// Merge index segments for lower memory and faster reads.
pub fn compact(store_path: &Path, store_config: &StoreConfig, prefix: &LinePrefix) -> Result<()> {
    let store_path = &normalize_path(store_path);
    ensure_store_exists(store_path)?;
    let mp = MultiProgress::new();
    let paint = crate::terminal::stderr_painter();
    let pfx = prefix.to_string();

    progress::mp_println(
        &mp,
        format!("{prefix}[{} ] compacting store", paint.purple(".")),
    );

    let store = store::Store::open(
        store_path,
        store_config.phrase_search,
        store_config.writer_heap_mb,
        store_config.language,
        store_config.doc_store_cache_blocks,
    )
    .context("failed to open store")?;

    let before = store.segment_count();
    if before <= 1 {
        progress::mp_println(
            &mp,
            format!(
                "{prefix}[{} ] {before} segment, already optimal",
                paint.green("+")
            ),
        );
        return Ok(());
    }

    let step = progress::add_step(
        &mp,
        &pfx,
        &format!("compacting {before} segment{}...", plural(before)),
    );

    store.optimize(|segs| {
        step.set_message(format!(
            "compacting... {segs} segment{} remaining",
            plural(segs)
        ));
    })?;

    let after = store.segment_count();
    progress::finish_step(
        &mp,
        &step,
        &pfx,
        &format!("compacted: {before} -> {after} segment{}", plural(after)),
    );

    Ok(())
}

/// Cross-reference document metadata against index chunks to find inconsistencies.
fn detect_issues(store: &store::Store) -> Result<Vec<Issue>> {
    let docs = store.get_all_documents();
    let index_keys = store.index_source_keys()?;
    let mut issues: Vec<Issue> = Vec::new();

    for source in &index_keys {
        if !docs.contains_key(source) {
            let counts = store.count_chunks_for_sources(&[source.as_str()])?;
            let count = counts.get(source).copied().unwrap_or(0);
            issues.push(Issue {
                source_id: source.clone(),
                source: format!("[hash:{source}]"),
                kind: IssueKind::OrphanedChunks,
                detail: format!(
                    "{count} chunk{} in index with no document entry",
                    plural(count)
                ),
            });
        }
    }

    let doc_source_refs: Vec<&str> = docs.keys().map(SourceId::as_str).collect();
    let targeted_counts = store.count_chunks_for_sources(&doc_source_refs)?;

    for (source, meta) in docs.iter() {
        let actual = targeted_counts.get(source).copied().unwrap_or(0);
        if actual == 0 && meta.chunk_count > 0 {
            issues.push(Issue {
                source_id: source.clone(),
                source: meta.source.clone(),
                kind: IssueKind::MissingChunks,
                detail: format!(
                    "document entry claims {} chunk{}, but 0 found in index",
                    meta.chunk_count,
                    plural(meta.chunk_count)
                ),
            });
        } else if actual != meta.chunk_count && actual > 0 {
            issues.push(Issue {
                source_id: source.clone(),
                source: meta.source.clone(),
                kind: IssueKind::CountMismatch,
                detail: format!(
                    "document entry claims {} chunk{}, index has {actual}",
                    meta.chunk_count,
                    plural(meta.chunk_count)
                ),
            });
        }
    }

    Ok(issues)
}

/// Print detected issues using bracket notation.
fn report_issues(mp: &MultiProgress, issues: &[Issue], will_fix: bool, prefix: &LinePrefix) {
    if issues.is_empty() {
        return;
    }
    let paint = crate::terminal::stderr_painter();
    let mut counts = [0usize; 3];
    for issue in issues {
        let idx = match issue.kind {
            IssueKind::CountMismatch => 0,
            IssueKind::MissingChunks => 1,
            IssueKind::OrphanedChunks => 2,
        };
        if counts[idx] < MAX_PER_KIND {
            progress::mp_println(
                mp,
                format!(
                    "{prefix}[{} ] {}: {} -- {}",
                    paint.yellow("-"),
                    issue.kind,
                    issue.source,
                    issue.detail
                ),
            );
        }
        counts[idx] += 1;
    }
    let labels = ["count_mismatch", "missing_chunks", "orphaned_chunks"];
    for i in 0..3 {
        if counts[i] > MAX_PER_KIND {
            progress::mp_println(
                mp,
                format!(
                    "{prefix}     ... and {} more {}",
                    counts[i] - MAX_PER_KIND,
                    labels[i]
                ),
            );
        }
    }
    if !will_fix {
        let n = issues.len();
        progress::mp_println(
            mp,
            format!(
                "{prefix}[{} ] {n} issue{}, run `lore maintain repair` to fix",
                paint.blue("i"),
                plural(n),
            ),
        );
    }
}

/// Apply fixes for detected issues, returning the count of issues fixed.
fn fix_issues(store: &store::Store, issues: &[Issue]) -> Result<usize> {
    if issues.is_empty() {
        return Ok(0);
    }
    let mut fixed = 0usize;
    for issue in issues {
        match issue.kind {
            IssueKind::CountMismatch => {
                store.delete_chunks_by_source(&issue.source_id)?;
                store.delete_document(&issue.source_id);
                fixed += 1;
            }
            IssueKind::MissingChunks => {
                store.delete_document(&issue.source_id);
                fixed += 1;
            }
            IssueKind::OrphanedChunks => {
                store.delete_chunks_by_source(&issue.source_id)?;
                fixed += 1;
            }
        }
    }
    if fixed > 0 {
        store.commit()?;
    }
    Ok(fixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    use lore::store::test_helpers::{test_chunk, test_meta};
    use lore::types::source_id;

    fn mismatch_store(source: &str) -> (tempfile::TempDir, SourceId) {
        let dir = tempfile::tempdir().unwrap();
        let store = store::Store::open(
            dir.path(),
            true,
            256,
            lore::types::IndexLanguage::default(),
            100,
        )
        .unwrap();
        let chunk = test_chunk(source, "mismatch body text", "test", 0);
        store.insert_chunks(&[chunk]).unwrap();
        let mut meta = test_meta(source, "test");
        meta.chunk_count = 5;
        store.upsert_document(meta);
        store.commit().unwrap();
        (dir, source_id(source))
    }

    #[test]
    fn repair_issue_cases() {
        // Case 1: orphaned chunks (index entry with no doc metadata) are removed.
        let dir = tempfile::tempdir().unwrap();
        let store = store::Store::open(
            dir.path(),
            true,
            256,
            lore::types::IndexLanguage::default(),
            100,
        )
        .unwrap();
        let mut chunk = test_chunk("orphan.md", "orphan body text here", "test", 0);
        chunk.title = None;
        chunk.section = None;
        store.insert_chunks(&[chunk]).unwrap();
        store.commit().unwrap();
        let sid = source_id("orphan.md");
        drop(store);

        repair(
            dir.path(),
            &StoreConfig::default(),
            OutputMode::Json,
            &LinePrefix::none(),
        )
        .unwrap();
        let s = store::Store::open(
            dir.path(),
            true,
            256,
            lore::types::IndexLanguage::default(),
            100,
        )
        .unwrap();
        assert!(
            !s.index_source_keys().unwrap().contains(&sid),
            "orphaned chunks should be removed after repair"
        );

        // Case 2: doc entry with no chunks (missing_chunks) is removed.
        let dir = tempfile::tempdir().unwrap();
        let store = store::Store::open(
            dir.path(),
            true,
            256,
            lore::types::IndexLanguage::default(),
            100,
        )
        .unwrap();
        let mut meta = test_meta("missing.md", "test");
        meta.chunk_count = 5;
        meta.word_count = 0;
        meta.title = None;
        store.upsert_document(meta);
        store.commit().unwrap();
        let sid = source_id("missing.md");
        drop(store);

        repair(
            dir.path(),
            &StoreConfig::default(),
            OutputMode::Json,
            &LinePrefix::none(),
        )
        .unwrap();
        let s = store::Store::open(
            dir.path(),
            true,
            256,
            lore::types::IndexLanguage::default(),
            100,
        )
        .unwrap();
        assert!(
            !s.get_all_documents().contains_key(&sid),
            "document entry with no chunks should be removed after repair"
        );

        // Case 3: count mismatch (chunk_count=5 but only 1 chunk) removes both doc and chunks.
        let (dir, sid) = mismatch_store("mismatch_fix.md");

        repair(
            dir.path(),
            &StoreConfig::default(),
            OutputMode::Json,
            &LinePrefix::none(),
        )
        .unwrap();

        let s = store::Store::open(
            dir.path(),
            true,
            256,
            lore::types::IndexLanguage::default(),
            100,
        )
        .unwrap();
        assert!(
            !s.get_all_documents().contains_key(&sid),
            "document entry should be removed after repair"
        );
        let counts = s.count_chunks_for_sources(&[sid.as_str()]).unwrap();
        let actual = counts.get(&sid).copied().unwrap_or(0);
        assert_eq!(actual, 0, "chunks should be removed after repair");
    }

    #[test]
    fn check_does_not_modify() {
        let (dir, sid) = mismatch_store("mismatch.md");

        check(dir.path(), OutputMode::Json, &LinePrefix::none()).unwrap();

        let s = store::Store::open(
            dir.path(),
            true,
            256,
            lore::types::IndexLanguage::default(),
            100,
        )
        .unwrap();
        let docs = s.get_all_documents();
        let doc_meta = docs
            .get(&sid)
            .expect("document entry should still exist after check");
        assert_eq!(
            doc_meta.chunk_count, 5,
            "check must not modify the document entry"
        );
        let counts = s.count_chunks_for_sources(&[sid.as_str()]).unwrap();
        let actual = counts.get(&sid).copied().unwrap_or(0);
        assert_eq!(actual, 1, "check must not remove index chunks");
    }
}
