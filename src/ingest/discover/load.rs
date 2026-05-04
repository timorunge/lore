use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use anyhow::Result;
use futures::stream::StreamExt;
use tracing::warn;

use crate::cache::http::DownloadResult;
use crate::config::{ExtractMode, ProcessingLimits};
use crate::ingest::discover::stamp_download_meta;
use crate::ingest::loaders;
use crate::ingest::types::{FailedDoc, LoaderResult};
use crate::types::{SourceId, SourceType, StampsMeta};
use crate::util::progress::ProgressHandle;
use crate::util::relativize_path;

/// Bundled context for loading a file or archive, reducing argument count.
pub(crate) struct LoadContext {
    pub(crate) limits: ProcessingLimits,
    pub(crate) extract: ExtractMode,
    pub(crate) fetch_pb: ProgressHandle,
    pub(crate) existing_stamps: Arc<HashMap<SourceId, StampsMeta>>,
    pub(crate) rewrite_base: PathBuf,
    pub(crate) topic: Option<String>,
    pub(crate) text_ext: Option<Arc<Vec<String>>>,
}

/// Process a completed download: detect archives, extract if needed, and return loaded documents.
pub(crate) async fn process_download(
    url: &str,
    dl: &DownloadResult,
    topic: Option<&str>,
    source_type: SourceType,
    limits: ProcessingLimits,
    extract: ExtractMode,
) -> Result<(Vec<LoaderResult>, Vec<FailedDoc>)> {
    let is_archive = loaders::archive::is_archive_content_type(dl.content_type.as_deref())
        || loaders::archive::is_archive(&dl.path);

    let (mut docs, failures) = if is_archive {
        let extracted =
            loaders::archive::extract(&dl.path, limits.max_archive_files, limits.max_archive_bytes)
                .await?;
        let extract_dir = crate::cache::archive_extract_dir(&dl.path)?;
        let (mut archive_docs, archive_failures) = read_files_parallel(
            extracted,
            topic,
            None,
            &limits,
            extract,
            &ProgressHandle::noop(),
        )
        .await?;
        for doc in &mut archive_docs {
            let rel = Path::new(&doc.source)
                .strip_prefix(&extract_dir)
                .map_or_else(|_| doc.source.clone(), |r| r.to_string_lossy().into_owned());
            doc.source = format!("{url}#{rel}");
            doc.source_id = crate::types::source_id(&doc.source);
        }
        (archive_docs, archive_failures)
    } else {
        let mut doc = loaders::file::read_file(
            &dl.path,
            topic,
            None,
            &limits,
            extract,
            dl.content_type.as_deref(),
        )
        .await?;
        url.clone_into(&mut doc.source);
        doc.source_id = crate::types::source_id(&doc.source);
        (vec![doc], Vec::new())
    };

    stamp_download_meta(
        &mut docs,
        source_type,
        dl.etag.as_deref(),
        dl.last_modified.as_deref(),
    );

    Ok((docs, failures))
}

/// Load a single file path, expanding archives into multiple documents with stable `{archive}#{entry}` source keys.
pub(crate) async fn load_file_or_archive(
    path: &Path,
    ctx: &LoadContext,
) -> Result<(Vec<LoaderResult>, Vec<FailedDoc>)> {
    let limits = &ctx.limits;
    let topic = ctx.topic.as_deref();
    let text_ext = ctx.text_ext.as_deref().map(Vec::as_slice);
    let extract = ctx.extract;
    let fetch_pb = &ctx.fetch_pb;
    let existing_stamps: &HashMap<SourceId, StampsMeta> = &ctx.existing_stamps;
    let rewrite_base = &ctx.rewrite_base;

    if loaders::archive::is_archive(path) {
        let archive_display = relativize_path(path, rewrite_base);
        fetch_pb.set_prefix(&archive_display);
        let extracted =
            loaders::archive::extract(path, limits.max_archive_files, limits.max_archive_bytes)
                .await?;
        let extract_dir = crate::cache::archive_extract_dir(path)?;

        let (to_read, mut skipped): (Vec<PathBuf>, Vec<LoaderResult>) =
            partition_unchanged_entries(extracted, &extract_dir, &archive_display, existing_stamps);

        let n_changed = to_read.len();
        if n_changed > 1 {
            fetch_pb.inc_length(n_changed as u64 - 1);
        }

        let (mut docs, archive_failures) =
            read_files_parallel(to_read, topic, text_ext, limits, extract, fetch_pb).await?;
        if n_changed == 0 {
            fetch_pb.inc(1);
        }
        for doc in &mut docs {
            let rel = Path::new(&doc.source)
                .strip_prefix(&extract_dir)
                .map_or_else(|_| doc.source.clone(), |r| r.to_string_lossy().into_owned());
            doc.source = format!("{archive_display}#{rel}");
            doc.source_id = crate::types::source_id(&doc.source);
        }
        docs.append(&mut skipped);
        Ok((docs, archive_failures))
    } else {
        let rewritten_source = relativize_path(path, rewrite_base);
        if let Some(stub) = check_file_unchanged(&rewritten_source, path, existing_stamps) {
            return Ok((vec![stub], Vec::new()));
        }
        fetch_pb.set_prefix(&rewritten_source);
        loaders::file::read_file(path, topic, text_ext, limits, extract, None)
            .await
            .map(|r| (vec![r], Vec::new()))
    }
}

/// Extract a short display name (filename only) for progress output.
fn file_display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Returns `true` when the on-disk `path` has the same mtime and size as `prev`.
fn mtime_size_match(prev: &StampsMeta, path: &Path) -> bool {
    (|| -> Option<bool> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime_ns = meta
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_nanos()).ok())?;
        let size = meta.len() as i64;
        Some(prev.mtime_ns == Some(mtime_ns) && prev.size_bytes == Some(size))
    })()
    .unwrap_or(false)
}

/// Return an unchanged stub if the file's mtime+size match the existing store entry for its rewritten source path.
fn check_file_unchanged(
    rewritten_source: &str,
    path: &Path,
    existing_stamps: &HashMap<SourceId, StampsMeta>,
) -> Option<LoaderResult> {
    if existing_stamps.is_empty() {
        return None;
    }
    let source_id = crate::types::source_id(rewritten_source);
    let prev = existing_stamps.get(&source_id)?;
    if mtime_size_match(prev, path) {
        Some(LoaderResult::unchanged_stub(
            rewritten_source.to_owned(),
            source_id,
            crate::types::SourceType::Local,
        ))
    } else {
        None
    }
}

/// Partition archive entries into (need_reading, unchanged_stubs).
fn partition_unchanged_entries(
    paths: Vec<PathBuf>,
    extract_dir: &Path,
    archive_display: &str,
    existing_stamps: &HashMap<SourceId, StampsMeta>,
) -> (Vec<PathBuf>, Vec<LoaderResult>) {
    if existing_stamps.is_empty() {
        return (paths, Vec::new());
    }
    let mut to_read = Vec::new();
    let mut skipped = Vec::new();
    for path in paths {
        let rel = path.strip_prefix(extract_dir).map_or_else(
            |_| path.to_string_lossy().into_owned(),
            |r| r.to_string_lossy().into_owned(),
        );
        let source = format!("{archive_display}#{rel}");
        let source_id = crate::types::source_id(&source);
        let unchanged = existing_stamps
            .get(&source_id)
            .is_some_and(|prev| mtime_size_match(prev, &path));
        if unchanged {
            skipped.push(LoaderResult::unchanged_stub(
                source,
                source_id,
                crate::types::SourceType::Local,
            ));
        } else {
            to_read.push(path);
        }
    }
    (to_read, skipped)
}

/// Read multiple files concurrently, logging warnings for individual failures.
pub(super) async fn read_files_parallel(
    paths: Vec<PathBuf>,
    topic: Option<&str>,
    text_ext: Option<&[String]>,
    limits: &ProcessingLimits,
    extract: ExtractMode,
    fetch_pb: &ProgressHandle,
) -> Result<(Vec<LoaderResult>, Vec<FailedDoc>)> {
    let concurrency = limits.concurrency;
    let limits = *limits;
    let text_ext: Option<Arc<[String]>> = text_ext.map(Arc::from);
    let topic: Option<Arc<str>> = topic.map(Arc::from);

    let mut docs = Vec::with_capacity(paths.len());
    let mut failures = Vec::new();
    let mut stream = futures::stream::iter(paths.into_iter().map(|path| {
        let topic = topic.clone();
        let text_ext = text_ext.clone();
        let pb = fetch_pb.clone();
        async move {
            let display = path.to_string_lossy().into_owned();
            pb.set_prefix(file_display_name(&path));
            let result = loaders::file::read_file(
                &path,
                topic.as_deref(),
                text_ext.as_deref(),
                &limits,
                extract,
                None,
            )
            .await;
            (display, result)
        }
    }))
    .buffer_unordered(concurrency);

    while let Some((source, result)) = stream.next().await {
        fetch_pb.inc(1);
        match result {
            Ok(doc) => docs.push(doc),
            Err(e) => {
                failures.push(FailedDoc::new(source, format!("{e}")));
                warn!("failed to read extracted file: {e}");
            }
        }
    }
    Ok((docs, failures))
}
