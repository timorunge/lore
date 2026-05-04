use crate::types::{DocKind, SourceId, SourceType};

/// A document that failed during ingest, with source identifier and reason.
#[derive(Debug, Clone)]
pub struct FailedDoc {
    /// Source path, URL, or key identifying the document that failed.
    pub source: String,
    /// Human-readable description of the failure.
    pub reason: String,
}

impl FailedDoc {
    /// Create a new failure record from a source identifier and reason.
    pub fn new(source: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            reason: reason.into(),
        }
    }
}

/// Raw document from a loader, before chunking and indexing.
#[derive(Debug, Clone)]
pub struct LoaderResult {
    /// Opaque identity hash, computed from `source`.
    pub source_id: SourceId,
    /// Human-readable source path or URL.
    pub source: String,
    /// Origin type of the source (local, url, git, etc.).
    pub origin: SourceType,
    /// Document kind classification (document, code, data, etc.).
    pub kind: DocKind,
    /// Full document text content.
    pub content: String,
    /// Pre-filtered as unchanged (mtime+size match). Carried only for its
    /// `source_id` so the stale-document sweep keeps it; `process_documents`
    /// skips it without hashing.
    pub unchanged: bool,
    /// File format hint (e.g. `"md"`, `"pdf"`, `"rs"`).
    pub format: Option<String>,
    /// Topic label assigned by the source config or LLM detection.
    pub topic: Option<String>,
    /// Document title extracted from metadata or content.
    pub title: Option<String>,
    /// Author extracted from metadata.
    pub author: Option<String>,
    /// Language code (e.g. `"en"`, `"de"`).
    pub lang: Option<String>,
    /// Document creation timestamp as an ISO-8601 string.
    pub created_at: Option<String>,
    /// Comma-separated tags extracted from metadata.
    pub tags: Option<String>,
    /// File modification time in nanoseconds since the Unix epoch.
    pub mtime_ns: Option<i64>,
    /// File size in bytes at the time of loading.
    pub size_bytes: Option<i64>,
    /// HTTP ETag from a remote source, used for freshness checks.
    pub etag: Option<String>,
    /// HTTP Last-Modified header value from a remote source.
    pub last_modified: Option<String>,
    /// Caller-supplied content hash that overrides the computed hash for freshness checks.
    pub content_hash_override: Option<String>,
}

impl LoaderResult {
    #[cfg(test)]
    pub(crate) fn test_doc(content: &str) -> Self {
        Self {
            source_id: crate::types::source_id("/test/file.md"),
            source: "/test/file.md".to_owned(),
            origin: SourceType::Local,
            kind: DocKind::Document,
            content: content.to_owned(),
            unchanged: false,
            format: None,
            topic: None,
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
        }
    }

    /// Create a stub for a file that was pre-filtered as unchanged.
    /// Only the `source_id` matters; the rest is filler.
    pub(crate) fn unchanged_stub(source: String, source_id: SourceId, origin: SourceType) -> Self {
        Self {
            source_id,
            source,
            origin,
            kind: DocKind::default(),
            content: String::new(),
            unchanged: true,
            format: None,
            topic: None,
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
        }
    }
}
