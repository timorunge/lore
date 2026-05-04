use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use futures::stream::StreamExt;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

use crate::cache;
use crate::config::{ExtractMode, ProcessingLimits};
use crate::ingest::loaders::file;
use crate::ingest::types::{FailedDoc, LoaderResult};
use crate::types::SourceType;
use crate::util::progress::ProgressHandle;

/// Maximum number of S3 objects to list in a single discovery run.
const MAX_S3_OBJECTS: usize = 100_000;

pub(crate) struct S3Object {
    pub(crate) key: String,
    pub(crate) etag: Option<String>,
    pub(crate) last_modified: Option<String>,
}

/// Discovered S3 objects ready for batched fetching.
pub(crate) struct S3Discovered {
    pub(crate) bucket: String,
    pub(crate) objects: Vec<S3Object>,
    pub(crate) client: aws_sdk_s3::Client,
}

/// Parse an `s3://bucket/prefix` URI into `(bucket, prefix)` components.
fn parse_s3_uri(uri: &str) -> Result<(String, String)> {
    let stripped = uri
        .strip_prefix("s3://")
        .context("S3 URI must start with s3://")?;
    let (bucket, prefix) = stripped.split_once('/').unwrap_or((stripped, ""));
    Ok((bucket.to_owned(), prefix.trim_start_matches('/').to_owned()))
}

/// List all objects under `prefix` in `bucket`, paginating until complete or the cap is reached.
async fn list_objects(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    prefix: &str,
    progress: &ProgressHandle,
) -> Result<Vec<S3Object>> {
    let mut objects = Vec::new();
    let mut continuation_token: Option<String> = None;

    progress.inc_length(1);

    loop {
        let mut req = client.list_objects_v2().bucket(bucket).prefix(prefix);

        if let Some(token) = &continuation_token {
            req = req.continuation_token(token);
        }

        let resp = req.send().await.context("S3 list_objects_v2 failed")?;

        for obj in resp.contents() {
            let key = obj.key().unwrap_or_default();
            if key.ends_with('/') {
                continue;
            }
            objects.push(S3Object {
                key: key.to_owned(),
                etag: obj.e_tag().map(|s: &str| s.to_owned()),
                last_modified: obj
                    .last_modified()
                    .map(|dt: &aws_sdk_s3::primitives::DateTime| {
                        dt.fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime)
                            .unwrap_or_default()
                    }),
            });
            if objects.len() >= MAX_S3_OBJECTS {
                warn!(
                    bucket,
                    prefix,
                    limit = MAX_S3_OBJECTS,
                    "S3 object listing capped at MAX_S3_OBJECTS"
                );
                return Ok(objects);
            }
        }

        progress.inc(1);

        if resp.is_truncated() == Some(true) {
            continuation_token = resp
                .next_continuation_token()
                .map(std::borrow::ToOwned::to_owned);
            progress.inc_length(1);
        } else {
            break;
        }
    }

    Ok(objects)
}

/// Derive the cache path for an S3 object.
fn s3_cache_paths(bucket: &str, key: &str) -> Result<(PathBuf, PathBuf)> {
    let cache_key = format!("s3://{bucket}/{key}");
    let dir = cache::http_cache_dir()?;
    let hash = crate::util::blake3_hex(&cache_key);
    let ext = Path::new(key)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let body = dir.join(format!("{hash}{ext}"));
    let meta = dir.join(format!("{hash}.s3meta"));
    Ok((body, meta))
}

/// Read the ETag and content-type stored in the `.s3meta` sidecar file.
///
/// Format: `{etag}\n{content_type}`.
fn read_s3meta(meta_path: &Path) -> (Option<String>, Option<String>) {
    let Ok(raw) = std::fs::read_to_string(meta_path) else {
        return (None, None);
    };
    let mut lines = raw.splitn(2, '\n');
    let etag = lines
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let content_type = lines
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    (etag, content_type)
}

/// Download an S3 object to the local cache, skipping the download if the ETag matches.
async fn download_to_cache(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
    etag: Option<&str>,
    max_file_bytes: u64,
) -> Result<(PathBuf, Option<String>)> {
    let (dest, meta_path) = s3_cache_paths(bucket, key)?;

    // Fast path: body already on disk with matching ETag -- skip GetObject entirely.
    let (cached_etag, cached_content_type) = read_s3meta(&meta_path);
    if dest.exists()
        && let Some(expected) = etag
        && cached_etag.as_deref() == Some(expected)
    {
        debug!(key, "S3 cache hit, skipping download");
        return Ok((dest, cached_content_type));
    }

    // Conditional GET: if we have an ETag, ask S3 to return 304 if unchanged.
    let mut req = client.get_object().bucket(bucket).key(key);
    if let Some(etag_val) = etag {
        req = req.if_none_match(etag_val);
    }

    let resp = match req.send().await {
        Ok(resp) => resp,
        Err(err) => {
            // S3 SDK surfaces 304 as an unmodeled error. Check the raw HTTP status.
            if err
                .raw_response()
                .is_some_and(|r| r.status().as_u16() == 304)
            {
                debug!(key, "S3 conditional GET: not modified");
                return Ok((dest, None));
            }
            return Err(err).context("S3 get_object failed");
        }
    };

    let content_type = resp.content_type().map(std::borrow::ToOwned::to_owned);

    if let Some(len) = resp.content_length()
        && u64::try_from(len).unwrap_or(u64::MAX) > max_file_bytes
    {
        anyhow::bail!(
            "S3 object s3://{bucket}/{key} is {len} bytes, exceeds max_file_bytes={max_file_bytes}"
        );
    }

    let tmp = dest.with_extension("body.tmp");
    let mut file = tokio::fs::File::create(&tmp)
        .await
        .context("failed to create S3 temp file")?;
    let mut bytes_written: u64 = 0;
    let mut stream = resp.body;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("S3 download stream error")?;
        bytes_written += chunk.len() as u64;
        if bytes_written > max_file_bytes {
            drop(file);
            if let Err(rm_err) = tokio::fs::remove_file(&tmp).await {
                warn!(path = %tmp.display(), "failed to remove partial S3 download: {rm_err}");
            }
            anyhow::bail!("S3 object s3://{bucket}/{key} exceeds max_file_bytes={max_file_bytes}");
        }
        file.write_all(&chunk)
            .await
            .context("failed to write S3 chunk")?;
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&tmp, &dest)
        .await
        .context("failed to finalize S3 download")?;

    if let Some(etag_val) = etag {
        let ct = content_type.as_deref().unwrap_or("");
        let sidecar = format!("{etag_val}\n{ct}");
        if let Err(e) = std::fs::write(&meta_path, &sidecar) {
            warn!(path = %meta_path.display(), "failed to write S3 metadata sidecar: {e}");
        }
    }

    Ok((dest, content_type))
}

pub(crate) async fn discover(
    s3_uri: &str,
    glob: Option<&str>,
    include: Option<&str>,
    existing_etags: &std::collections::HashMap<crate::types::SourceId, Option<String>>,
    progress: &ProgressHandle,
) -> Result<S3Discovered> {
    let (bucket, prefix) = parse_s3_uri(s3_uri)?;

    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let client = aws_sdk_s3::Client::new(&config);

    info!(bucket = bucket, prefix = prefix, "listing S3 objects");
    let objects = list_objects(&client, &bucket, &prefix, progress).await?;

    let glob_matcher = glob
        .map(|p| globset::Glob::new(p).map(|g| g.compile_matcher()))
        .transpose()
        .context("invalid glob pattern")?;

    let include_re = crate::ingest::compile_include_re(include)?;

    let filtered: Vec<S3Object> = objects
        .into_iter()
        .filter(|obj| {
            let rel = obj.key.strip_prefix(&prefix).unwrap_or(&obj.key);
            let rel = rel.trim_start_matches('/');
            if let Some(ref matcher) = glob_matcher
                && !matcher.is_match(rel)
            {
                return false;
            }
            if let Some(ref re) = include_re
                && !re.is_match(&obj.key)
            {
                return false;
            }
            true
        })
        .collect();

    // `existing_etags` is keyed by `source_id` (SHA-256 of the source string),
    // matching the format stored in `DocMeta.source_id` by `download_and_read`.
    let to_fetch = filter_unchanged(filtered, s3_uri, existing_etags);

    info!(
        bucket = bucket,
        fetching = to_fetch.len(),
        "S3 objects to fetch"
    );

    Ok(S3Discovered {
        bucket,
        objects: to_fetch,
        client,
    })
}

/// Fetch a range of discovered S3 objects, downloading and reading each one.
/// If `fetch_pb` is provided, it is incremented once per completed object.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn fetch_range(
    discovered: &S3Discovered,
    start: usize,
    end: usize,
    s3_uri: &str,
    topic: Option<&str>,
    limits: &ProcessingLimits,
    concurrency: usize,
    extract: ExtractMode,
    fetch_pb: &ProgressHandle,
) -> Result<(Vec<LoaderResult>, Vec<FailedDoc>)> {
    let slice = &discovered.objects[start..end];
    let limits = *limits;

    let mut results = Vec::new();
    let mut failures = Vec::new();
    let futs: Vec<_> = slice
        .iter()
        .map(|obj| {
            let client = &discovered.client;
            let bucket = &discovered.bucket;
            async move {
                let source = format!("{}/{}", s3_uri.trim_end_matches('/'), obj.key);
                let result =
                    download_and_read(client, bucket, obj, s3_uri, topic, &limits, extract).await;
                (source, result)
            }
        })
        .collect();
    let mut stream = futures::stream::iter(futs).buffer_unordered(concurrency);
    while let Some((source, r)) = stream.next().await {
        fetch_pb.inc(1);
        match r {
            Ok(result) => results.push(result),
            Err(e) => {
                failures.push(FailedDoc::new(source, format!("{e}")));
                warn!("failed to process S3 object: {e}");
            }
        }
    }
    Ok((results, failures))
}

/// Download an S3 object to cache and extract its content via the file loader.
async fn download_and_read(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    obj: &S3Object,
    s3_uri: &str,
    topic: Option<&str>,
    limits: &ProcessingLimits,
    extract: ExtractMode,
) -> Result<LoaderResult> {
    let (cached_path, content_type) = download_to_cache(
        client,
        bucket,
        &obj.key,
        obj.etag.as_deref(),
        limits.max_file_bytes,
    )
    .await?;
    let mut result = file::read_file(
        &cached_path,
        topic,
        None,
        limits,
        extract,
        content_type.as_deref(),
    )
    .await?;

    result.source = format!("{}/{}", s3_uri.trim_end_matches('/'), obj.key);
    result.source_id = crate::types::source_id(&result.source);
    result.origin = SourceType::S3;
    result.etag.clone_from(&obj.etag);
    result.last_modified.clone_from(&obj.last_modified);
    result.mtime_ns = None;
    result.size_bytes = None;

    Ok(result)
}

/// Applies the ETag-skip logic from `discover()` to a slice of objects.
/// Extracted to allow unit testing without a live AWS connection.
///
/// `existing_etags` must be keyed by `source_id` (blake3 of the full S3
/// source string, e.g. `blake3_hex("s3://bucket/key")[..32]`), matching the
/// format written by `download_and_read` into `DocMeta.source_id`.
fn filter_unchanged(
    objects: Vec<S3Object>,
    s3_uri: &str,
    existing_etags: &std::collections::HashMap<crate::types::SourceId, Option<String>>,
) -> Vec<S3Object> {
    objects
        .into_iter()
        .filter(|obj| {
            let source = format!("{}/{}", s3_uri.trim_end_matches('/'), obj.key);
            let source_key = crate::types::source_id(&source);
            if let Some(stored_etag) = existing_etags.get(&source_key)
                && stored_etag.as_deref() == obj.etag.as_deref()
            {
                debug!(key = obj.key, "S3 object unchanged, skipping");
                return false;
            }
            true
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{S3Object, filter_unchanged, read_s3meta};
    use crate::types::{SourceId, source_id};

    fn make_obj(key: &str, etag: Option<&str>) -> S3Object {
        S3Object {
            key: key.to_owned(),
            etag: etag.map(str::to_owned),
            last_modified: None,
        }
    }

    // `existing_etags` is keyed by `source_id(source_string)`, not by the raw
    // source string.  A matching object must be skipped; a changed ETag or an
    // absent key must be kept.
    #[test]
    fn s3_cache_key_cases() {
        let s3_uri = "s3://my-bucket/prefix";
        let obj_key = "prefix/docs/readme.md";
        let etag = "\"abc123\"";

        let source = format!("{}/{}", s3_uri.trim_end_matches('/'), obj_key);
        let hashed_key = source_id(&source);

        let mut existing_etags: HashMap<SourceId, Option<String>> = HashMap::new();
        existing_etags.insert(hashed_key, Some(etag.to_owned()));

        let unchanged = make_obj(obj_key, Some(etag));
        let result = filter_unchanged(vec![unchanged], s3_uri, &existing_etags);
        assert!(
            result.is_empty(),
            "unchanged object should be skipped but was returned"
        );

        let changed = make_obj(obj_key, Some("\"different\""));
        let result = filter_unchanged(vec![changed], s3_uri, &existing_etags);
        assert_eq!(result.len(), 1, "changed object should be fetched");

        let new_obj = make_obj("prefix/docs/new.md", Some(etag));
        let result = filter_unchanged(vec![new_obj], s3_uri, &existing_etags);
        assert_eq!(result.len(), 1, "new object should be fetched");

        let tmp = tempfile::tempdir().unwrap();
        let meta_path = tmp.path().join("test.s3meta");
        std::fs::write(&meta_path, "etag123\napplication/pdf").unwrap();
        let (etag, ct) = read_s3meta(&meta_path);
        assert_eq!(etag.as_deref(), Some("etag123"));
        assert_eq!(ct.as_deref(), Some("application/pdf"));
    }
}
