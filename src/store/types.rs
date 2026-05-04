use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::types::{Chunk, DocKind, DocMeta, SourceType};

/// Hard cap on search results to prevent unbounded heap usage.
pub(crate) const MAX_SEARCH_LIMIT: usize = 1000;

/// Maximum query length in bytes. Queries beyond this are truncated to
/// prevent excessive parsing cost in Tantivy's QueryParser.
pub(crate) const MAX_QUERY_BYTES: usize = 4096;

pub(super) const MARKER_FILE: &str = ".lore.store";
pub(super) const MARKER_SALT: &str = "lore-store-v1";
pub(super) const TANTIVY_DIR: &str = "tantivy";
pub(super) const DOCS_FILE: &str = "lore.docs";
pub(super) const META_FILE: &str = "lore.meta";
pub(super) const STAMPS_FILE: &str = "lore.stamps";
#[cfg(feature = "ingest")]
pub(crate) const LOCK_FILE: &str = ".lore.lock";

pub(crate) const SIDECAR_MAGIC: &[u8; 4] = b"LORE";
pub(crate) const DOCS_FORMAT_VERSION: u8 = 1;
pub(crate) const STAMPS_FORMAT_VERSION: u8 = 1;
pub(crate) const META_FORMAT_VERSION: u8 = 1;

/// A ranked search result pairing a relevance score with its matched chunk.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    /// Position in the result set (0-based, offset-adjusted).
    pub rank: u64,
    /// BM25 relevance score from Tantivy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<SnippetData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_highlights: Option<FieldHighlights>,
    /// Total chunks in the source document (for position context).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_total: Option<u64>,
    #[serde(flatten)]
    pub chunk: Chunk,
}

impl SearchHit {
    pub fn highlight_ranges(
        &self,
        get: impl Fn(&FieldHighlights) -> &Vec<(usize, usize)>,
    ) -> &[(usize, usize)] {
        self.field_highlights
            .as_ref()
            .map(|fh| get(fh).as_slice())
            .unwrap_or_default()
    }
}

/// Search result snippet with highlighted term positions.
#[derive(Debug, Clone, Serialize)]
pub struct SnippetData {
    pub text: String,
    /// Byte ranges within `text` that correspond to matched query terms.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub highlights: Vec<(usize, usize)>,
}

/// Highlight ranges for title and section fields (byte offsets into the
/// original field text, not a snippet window).
#[derive(Debug, Clone, Default, Serialize)]
pub struct FieldHighlights {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub title: Vec<(usize, usize)>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub section: Vec<(usize, usize)>,
}

/// A document's metadata together with its current chunks.
#[derive(Debug, Serialize)]
pub struct DocDetail {
    #[serde(flatten)]
    pub meta: DocMeta,
    pub chunks: Vec<Chunk>,
}

/// Sort order for document listings.
#[derive(Default, Clone, Copy, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DocSort {
    #[default]
    Source,
    Topic,
    Title,
    Author,
    Chunks,
    Words,
    Quality,
}

impl DocSort {
    /// Sort a slice of document metadata in-place, optionally reversing.
    pub fn apply(self, docs: &mut [DocMeta], reverse: bool) {
        match self {
            DocSort::Source => docs.sort_by(|a, b| a.source.cmp(&b.source)),
            DocSort::Topic => docs.sort_by(|a, b| a.topic.as_deref().cmp(&b.topic.as_deref())),
            DocSort::Title => docs.sort_by(|a, b| a.title.as_deref().cmp(&b.title.as_deref())),
            DocSort::Author => docs.sort_by(|a, b| a.author.as_deref().cmp(&b.author.as_deref())),
            DocSort::Chunks => docs.sort_by_key(|d| std::cmp::Reverse(d.chunk_count)),
            DocSort::Words => docs.sort_by_key(|d| std::cmp::Reverse(d.word_count)),
            DocSort::Quality => {
                docs.sort_by(
                    |a, b| match (a.avg_llm_quality_score, b.avg_llm_quality_score) {
                        (Some(qa), Some(qb)) => qb.total_cmp(&qa),
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, None) => std::cmp::Ordering::Equal,
                    },
                );
            }
        }
        if reverse {
            docs.reverse();
        }
    }
}

/// Sort order for topic listings.
#[derive(Default, Clone, Copy, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TopicSort {
    #[default]
    Name,
    Chunks,
    Words,
}

impl TopicSort {
    /// Sort a slice of topic stats in-place, optionally reversing.
    pub fn apply(self, stats: &mut [TopicStat], reverse: bool) {
        match self {
            TopicSort::Name => stats.sort_by(|a, b| a.name.cmp(&b.name)),
            TopicSort::Chunks => stats.sort_by_key(|s| std::cmp::Reverse(s.chunk_count)),
            TopicSort::Words => stats.sort_by_key(|s| std::cmp::Reverse(s.word_count)),
        }
        if reverse {
            stats.reverse();
        }
    }
}

/// Sort order for search results.
#[derive(Default, Clone, Copy, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SearchSort {
    #[default]
    Score,
    Source,
    Topic,
}

impl SearchSort {
    /// Sort a slice of search hits in-place, optionally reversing.
    pub fn apply(self, hits: &mut [SearchHit], reverse: bool) {
        match self {
            SearchSort::Score => {
                hits.sort_by(|a, b| b.score.unwrap_or(0.0).total_cmp(&a.score.unwrap_or(0.0)));
            }
            SearchSort::Source => {
                hits.sort_by(|a, b| a.chunk.source.cmp(&b.chunk.source));
            }
            SearchSort::Topic => {
                hits.sort_by(|a, b| a.chunk.topic.as_deref().cmp(&b.chunk.topic.as_deref()));
            }
        }
        if reverse {
            hits.reverse();
        }
    }
}

/// Full-text search request with filters, pagination, and sort.
#[derive(Clone)]
pub struct SearchQuery {
    pub query: String,
    pub limit: usize,
    pub offset: usize,
    pub sort: SearchSort,
    pub reverse: bool,
    /// Substring filter on the source path.
    pub source: Option<String>,
    pub topic: Option<String>,
    pub author: Option<String>,
    pub lang: Option<String>,
    pub origin: Option<String>,
    pub kind: Option<String>,
    pub format: Option<String>,
    /// Cap results per source document; triggers over-fetch when set.
    pub max_per_source: Option<usize>,
}

/// Filter and sort criteria for document listings.
#[derive(Default)]
pub struct DocFilter {
    pub sort: DocSort,
    pub reverse: bool,
    pub source: Option<String>,
    pub topic: Option<String>,
    pub title: Option<String>,
    pub author: Option<String>,
    pub lang: Option<String>,
    pub origin: Option<String>,
    pub kind: Option<String>,
    pub format: Option<String>,
}

/// Filter criteria for topic listings.
#[derive(Default)]
pub struct TopicFilter {
    pub topic: Option<String>,
    pub author: Option<String>,
    pub source: Option<String>,
    pub lang: Option<String>,
    pub origin: Option<String>,
    pub kind: Option<String>,
    pub format: Option<String>,
}

/// Outcome of resolving a user-supplied source string against the store.
#[derive(Debug)]
pub enum SourceResolveResult {
    Exact(String),
    Ambiguous(Vec<String>),
    NotFound,
}

/// Topic detail with source paths and all chunks.
#[derive(Debug, Serialize)]
pub struct TopicResult {
    pub name: String,
    pub chunk_count: u64,
    pub source_paths: Vec<String>,
    pub chunks: Vec<Chunk>,
}

/// Topic name with aggregate statistics.
#[derive(Debug, Clone, Serialize)]
pub struct TopicStat {
    pub name: String,
    pub doc_count: u64,
    pub chunk_count: u64,
    pub word_count: u64,
}

/// Flat, serializable snapshot of a store's metadata and statistics.
#[derive(Debug, Clone, Serialize)]
pub struct StoreInfo {
    pub documents: u64,
    pub chunks: u64,
    pub words: u64,
    pub topics: Vec<TopicStat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_words_per_chunk: Option<f64>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub source_types: HashMap<SourceType, u64>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub kinds: HashMap<DocKind, u64>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub formats: HashMap<String, u64>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub languages: HashMap<String, u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phrase_search: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub writer_heap_mb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lore_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub segments: usize,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            query: String::new(),
            limit: 10,
            offset: 0,
            sort: SearchSort::default(),
            reverse: false,
            source: None,
            topic: None,
            author: None,
            lang: None,
            origin: None,
            kind: None,
            format: None,
            max_per_source: None,
        }
    }
}

#[cfg(test)]
impl SearchQuery {
    pub(crate) fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            ..Self::default()
        }
    }
}

/// Returns the path to the `lore.docs` sidecar file within a store directory.
pub(crate) fn store_docs_path(base: &Path) -> PathBuf {
    base.join(DOCS_FILE)
}

/// Returns the path to the `lore.meta` sidecar file within a store directory.
pub(crate) fn store_meta_path(base: &Path) -> PathBuf {
    base.join(META_FILE)
}

/// Returns the path to the `lore.stamps` sidecar file within a store directory.
pub(crate) fn store_stamps_path(base: &Path) -> PathBuf {
    base.join(STAMPS_FILE)
}

/// Sort chunks by source then chunk_index for stable display ordering.
pub(crate) fn sort_chunks_natural(chunks: &mut [Chunk]) {
    chunks.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then(a.chunk_index.cmp(&b.chunk_index))
    });
}

/// Increment topic aggregation counters (docs, chunks, words) for a document.
pub(crate) fn accumulate_topic<'a>(map: &mut HashMap<&'a str, (u64, u64, u64)>, doc: &'a DocMeta) {
    if let Some(ref topic) = doc.topic {
        let entry = map.entry(topic.as_str()).or_default();
        entry.0 += 1;
        entry.1 += doc.chunk_count;
        entry.2 += doc.word_count;
    }
}

/// Convert the raw topic aggregation map into a vec of `TopicStat`.
pub(crate) fn topic_agg_to_stats(map: HashMap<&str, (u64, u64, u64)>) -> Vec<TopicStat> {
    map.into_iter()
        .map(|(name, (doc_count, chunk_count, word_count))| TopicStat {
            name: name.to_owned(),
            doc_count,
            chunk_count,
            word_count,
        })
        .collect()
}

/// Average words per chunk, rounded to one decimal place.
pub(crate) fn avg_words_per_chunk(words: u64, chunks: u64) -> Option<f64> {
    if chunks > 0 {
        let avg = words as f64 / chunks as f64;
        Some((avg * 10.0).round() / 10.0)
    } else {
        None
    }
}

/// Deserialize a version-prefixed bitcode sidecar file.
pub(crate) fn load_sidecar_file<T: bitcode::DecodeOwned>(
    path: &Path,
    version: u8,
    name: &str,
) -> anyhow::Result<T> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let payload = read_sidecar(&bytes, version, name)?;
    bitcode::decode(payload).with_context(|| {
        format!(
            "failed to parse {} -- run `lore ingest --recreate` to rebuild",
            path.display()
        )
    })
}

/// Encode a sidecar file: 4-byte magic + version byte + raw payload.
pub(crate) fn write_sidecar(version: u8, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.extend_from_slice(SIDECAR_MAGIC);
    buf.push(version);
    buf.extend_from_slice(payload);
    buf
}

/// Decode a sidecar file, validating magic bytes and format version.
pub(crate) fn read_sidecar<'a>(
    bytes: &'a [u8],
    expected_version: u8,
    file_name: &str,
) -> anyhow::Result<&'a [u8]> {
    if bytes.len() < 5 || &bytes[..4] != SIDECAR_MAGIC {
        anyhow::bail!(
            "{file_name} was created by an incompatible lore version. \
             Run `lore ingest --recreate` to rebuild."
        );
    }
    let version = bytes[4];
    if version != expected_version {
        anyhow::bail!(
            "{file_name} format version {version} is not supported \
             (expected {expected_version}). \
             Run `lore ingest --recreate` to rebuild."
        );
    }
    Ok(&bytes[5..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DocKind, DocMeta, SourceId, SourceType, StampsMeta};

    #[test]
    fn avg_words_per_chunk_cases() {
        assert_eq!(avg_words_per_chunk(0, 0), None);
        assert_eq!(avg_words_per_chunk(100, 0), None);
        assert_eq!(avg_words_per_chunk(100, 3), Some(33.3));
        assert_eq!(avg_words_per_chunk(200, 4), Some(50.0));
        assert_eq!(avg_words_per_chunk(1, 3), Some(0.3));
    }

    #[test]
    fn sidecar_roundtrip() {
        let payload = b"hello world";
        let encoded = write_sidecar(1, payload);
        let decoded = read_sidecar(&encoded, 1, "test").unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn sidecar_error_cases() {
        let wrong_version_bytes = write_sidecar(1, b"data");

        let cases: &[(&[u8], &[&str])] = &[
            (b"BAD\x01data", &["incompatible lore version"]),
            (b"LOR", &["incompatible"]),
            (&wrong_version_bytes, &["format version 1", "expected 2"]),
        ];

        for (input, fragments) in cases {
            let err = read_sidecar(input, 2, "test.dat").unwrap_err().to_string();
            for fragment in *fragments {
                assert!(
                    err.contains(fragment),
                    "expected {fragment:?} in error: {err}"
                );
            }
        }
    }

    fn sorted_keys(value: &serde_json::Value) -> String {
        let obj = value.as_object().expect("expected JSON object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        keys.join("\n")
    }

    /// CI guard: fails if `DocMeta` fields change accidentally.
    ///
    /// If this test fails, a field was added, removed, or renamed.
    /// Bump `DOCS_FORMAT_VERSION` in this file, then update the expected
    /// fingerprint below.
    #[test]
    fn doc_meta_fingerprint_matches_expected() {
        let doc = DocMeta {
            source_id: SourceId::from("test"),
            source: String::new(),
            origin: SourceType::default(),
            kind: DocKind::default(),
            format: Some(String::new()),
            topic: Some(String::new()),
            title: Some(String::new()),
            author: Some(String::new()),
            lang: Some(String::new()),
            tags: Some(String::new()),
            created_at: Some(String::new()),
            updated_at: Some(String::new()),
            avg_llm_quality_score: Some(0.0),
            llm_summary: Some(String::new()),
            chunk_count: 0,
            word_count: 0,
        };
        let json = serde_json::to_value(doc).expect("serialize DocMeta");
        let fingerprint = sorted_keys(&json);

        let expected = "\
author\n\
avg_llm_quality_score\n\
chunk_count\n\
created_at\n\
format\n\
kind\n\
lang\n\
llm_summary\n\
origin\n\
source\n\
source_id\n\
tags\n\
title\n\
topic\n\
updated_at\n\
word_count";

        assert_eq!(
            fingerprint, expected,
            "DocMeta fields changed. Bump DOCS_FORMAT_VERSION, then update expected.\n\
             New fingerprint:\n{fingerprint}"
        );
    }

    /// CI guard: fails if `StampsMeta` fields change accidentally.
    ///
    /// If this test fails, bump `STAMPS_FORMAT_VERSION` in this file,
    /// then update the expected fingerprint below.
    #[test]
    fn stamps_meta_fingerprint_matches_expected() {
        let stamps = StampsMeta {
            content_hash: String::new(),
            etag: Some(String::new()),
            last_modified: Some(String::new()),
            mtime_ns: Some(0),
            size_bytes: Some(0),
        };
        let json = serde_json::to_value(stamps).expect("serialize StampsMeta");
        let fingerprint = sorted_keys(&json);

        let expected = "\
content_hash\n\
etag\n\
last_modified\n\
mtime_ns\n\
size_bytes";

        assert_eq!(
            fingerprint, expected,
            "StampsMeta fields changed. Bump STAMPS_FORMAT_VERSION, then update expected.\n\
             New fingerprint:\n{fingerprint}"
        );
    }
}
