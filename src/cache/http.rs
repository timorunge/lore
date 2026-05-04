use std::path::PathBuf;

use reqwest::header::HeaderMap;
use tracing::warn;

use crate::cache::http_cache_dir;
use crate::util::{atomic_write, unix_now};

/// Result of a streaming download to disk.
#[derive(Debug)]
pub(crate) struct DownloadResult {
    pub(crate) path: PathBuf,
    pub(crate) content_type: Option<String>,
    pub(crate) etag: Option<String>,
    pub(crate) last_modified: Option<String>,
}

/// Normalize a URL for cache key consistency. Lowercases scheme and host,
/// strips default ports (80 for http, 443 for https), and removes trailing slash
/// from paths (except bare root "/").
fn normalize_cache_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            if matches!(
                (parsed.scheme(), parsed.port()),
                ("http", Some(80)) | ("https", Some(443))
            ) {
                parsed.set_port(None).ok(); // Cannot fail for http/https URLs
            }
            if parsed.path().len() > 1 && parsed.path().ends_with('/') {
                let trimmed = parsed.path()[..parsed.path().len() - 1].to_owned();
                parsed.set_path(&trimmed);
            }
            parsed.as_str().to_owned()
        }
        Err(_) => url.to_owned(),
    }
}

/// Returns (meta_path, body_path) for a URL's HTTP cache entry.
pub(crate) fn cache_paths(url: &str) -> Option<(PathBuf, PathBuf)> {
    let dir = http_cache_dir().ok()?;
    let key = crate::util::blake3_hex(&normalize_cache_url(url));
    Some((
        dir.join(format!("{key}.meta")),
        dir.join(format!("{key}.body")),
    ))
}

/// Write HTTP response headers and optional body to the on-disk cache synchronously.
fn cache_write_sync(url: &str, headers_json: serde_json::Value, body: Option<&[u8]>) {
    let Some((meta_path, body_path)) = cache_paths(url) else {
        return;
    };

    let mut meta = serde_json::Map::new();
    meta.insert(
        "fetched_at".into(),
        serde_json::Value::Number(unix_now().as_secs().into()),
    );
    meta.insert("headers".into(), headers_json);

    // Write meta first, then body (crash-safe ordering via atomic rename).
    // If we crash after meta but before body: next read finds meta but no body
    // -> cache miss (safe). If we crash after body but before meta: already the
    // previous failure mode -> cache miss (safe). Both use atomic_write
    // (tempfile + rename) so partial writes are never visible.
    // body=None means the download path already streamed to .body separately.
    if let Ok(json) = serde_json::to_vec(&serde_json::Value::Object(meta))
        && let Err(e) = atomic_write(&meta_path, &json)
    {
        warn!("cache write failed for meta: {e}");
        return;
    }
    if let Some(body) = body
        && let Err(e) = atomic_write(&body_path, body)
    {
        warn!("cache write failed for body: {e}");
    }
}

/// Touch a cached entry to refresh its mtime, extending its TTL.
///
/// Best-effort: silently returns on any read/write failure since a missed
/// touch only affects cache freshness, not correctness.
pub(crate) fn cache_touch_sync(url: &str) {
    let Some((meta_path, _)) = cache_paths(url) else {
        return;
    };
    let Ok(bytes) = std::fs::read(&meta_path) else {
        return;
    };
    let Ok(mut meta) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return;
    };
    if let Some(obj) = meta.as_object_mut() {
        obj.insert(
            "fetched_at".into(),
            serde_json::Value::Number(unix_now().as_secs().into()),
        );
        if let Ok(json) = serde_json::to_vec(&meta)
            && let Err(e) = atomic_write(&meta_path, &json)
        {
            warn!("cache touch write failed for {}: {e}", meta_path.display());
        }
    }
}

/// Serialize response headers and offload `cache_write_sync` to a blocking thread.
pub(crate) async fn cache_write_async(url: &str, headers: &HeaderMap, body: Option<&[u8]>) {
    let mut hdrs = serde_json::Map::new();
    for (name, value) in headers {
        if let Ok(v) = value.to_str() {
            hdrs.insert(
                name.as_str().to_lowercase(),
                serde_json::Value::String(v.to_owned()),
            );
        }
    }
    let headers_json = serde_json::Value::Object(hdrs);

    let url = url.to_owned();
    let body = body.map(<[u8]>::to_vec);
    if let Err(e) = tokio::task::spawn_blocking(move || {
        cache_write_sync(&url, headers_json, body.as_deref());
    })
    .await
    {
        warn!("cache write task panicked: {e}");
    }
}

/// Read and parse cached metadata, returning (meta_json, age_secs, body_path).
/// Returns None if the cache entry doesn't exist.
fn read_cache_meta(url: &str) -> Option<(serde_json::Value, f64, PathBuf)> {
    let (meta_path, body_path) = cache_paths(url)?;
    let meta_bytes = std::fs::read(&meta_path).ok()?;
    let meta: serde_json::Value = serde_json::from_slice(&meta_bytes).ok()?;
    let fetched_at = meta.get("fetched_at")?.as_f64()?;
    let age = (unix_now().as_secs_f64() - fetched_at).max(0.0);
    Some((meta, age, body_path))
}

/// Extract a single header string from a cached metadata JSON object.
fn cache_header<'a>(meta: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    meta.get("headers")
        .and_then(|h| h.get(key))
        .and_then(|v| v.as_str())
}

/// Check if a downloaded file is still cached and fresh (for download() path).
/// Returns DownloadResult pointing to the existing body file if still valid.
pub(crate) fn download_cache_get(url: &str, ttl_secs: f64) -> Option<DownloadResult> {
    let (meta, age, body_path) = read_cache_meta(url)?;
    if !body_path.exists() || age > ttl_secs {
        return None;
    }
    Some(DownloadResult {
        path: body_path,
        content_type: cache_header(&meta, "content-type").map(std::borrow::ToOwned::to_owned),
        etag: cache_header(&meta, "etag").map(std::borrow::ToOwned::to_owned),
        last_modified: cache_header(&meta, "last-modified").map(std::borrow::ToOwned::to_owned),
    })
}

/// Return cached body bytes if the entry exists and is still fresh, respecting Cache-Control and Expires headers.
/// The `ttl_secs` parameter acts as a hard ceiling even when `Cache-Control` would allow longer freshness.
pub(crate) fn cache_get_sync(url: &str, ttl_secs: f64) -> Option<Vec<u8>> {
    let (meta, age, body_path) = read_cache_meta(url)?;
    if !body_path.exists() {
        return None;
    }

    let cc_ttl = cache_header(&meta, "cache-control").and_then(|cc| {
        if cc.contains("no-store") || cc.contains("no-cache") {
            return Some(0.0);
        }
        cc.split(',').find_map(|part| {
            let part = part.trim();
            part.strip_prefix("max-age=")
                .and_then(|rest| rest.parse::<f64>().ok())
        })
    });

    let expires_ttl = if cc_ttl.is_none() {
        cache_header(&meta, "expires")
            .and_then(|s| httpdate::parse_http_date(s).ok())
            .map(|exp| {
                exp.duration_since(std::time::SystemTime::now())
                    .unwrap_or_default()
                    .as_secs_f64()
            })
    } else {
        None
    };

    let effective_ttl = cc_ttl.or(expires_ttl).unwrap_or(ttl_secs).min(ttl_secs);
    if age > effective_ttl {
        return None;
    }

    std::fs::read(&body_path).ok()
}
