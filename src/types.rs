//! Shared domain types used across the crate.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

macro_rules! impl_str_enum {
    ($ty:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        impl $ty {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $s),+ }
            }
        }
        impl std::str::FromStr for $ty {
            type Err = ();
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s { $($s => Ok(Self::$variant),)+ _ => Err(()) }
            }
        }
        impl std::fmt::Display for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

/// Classification of how a document source is accessed (local, git, HTTP, etc.).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    Default,
    bitcode::Encode,
    bitcode::Decode,
)]
#[repr(u8)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    #[default]
    Local,
    Url,
    Git,
    Sitemap,
    Feed,
    S3,
    Youtube,
    Maildir,
    Exec,
    Mcp,
}

impl_str_enum!(SourceType {
    Local => "local",
    Url => "url",
    Git => "git",
    Sitemap => "sitemap",
    Feed => "feed",
    S3 => "s3",
    Youtube => "youtube",
    Maildir => "maildir",
    Exec => "exec",
    Mcp => "mcp",
});

/// Content category of a document (document, code, data, e-mail).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    Default,
    bitcode::Encode,
    bitcode::Decode,
)]
#[repr(u8)]
#[serde(rename_all = "lowercase")]
pub enum DocKind {
    #[default]
    Document,
    Code,
    Data,
    Email,
}

impl_str_enum!(DocKind {
    Document => "document",
    Code => "code",
    Data => "data",
    Email => "email",
});

/// Opaque document identity -- a deterministic hash of the source path/URL.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, bitcode::Encode, bitcode::Decode,
)]
#[serde(transparent)]
pub struct SourceId(Arc<str>);

impl SourceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn from_raw(s: impl Into<String>) -> Self {
        let s: String = s.into();
        Self(Arc::from(s.as_str()))
    }
}

impl std::ops::Deref for SourceId {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::borrow::Borrow<str> for SourceId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for SourceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for SourceId {
    fn from(s: String) -> Self {
        Self(Arc::from(s))
    }
}

impl From<&str> for SourceId {
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

impl From<SourceId> for String {
    fn from(id: SourceId) -> Self {
        id.0.to_string()
    }
}

impl From<SourceId> for Arc<str> {
    fn from(id: SourceId) -> Self {
        id.0
    }
}

/// Compute a deterministic source ID from a human-readable source path/URL.
pub fn source_id(source: &str) -> SourceId {
    // 128-bit truncation (32 hex chars) -- collision at ~2^64 docs, acceptable for source dedup.
    SourceId::from(&crate::util::blake3_hex(source)[..32])
}

/// A single indexed text chunk with identity, content, and optional metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_field_names)]
pub struct Chunk {
    pub source_id: SourceId,
    pub source: Arc<str>,
    pub chunk_id: String,
    pub origin: SourceType,
    pub kind: DocKind,
    pub body: String,
    pub chunk_index: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Arc<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<Arc<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<Arc<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<Arc<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Arc<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<Arc<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<Arc<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_tags: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_quality_score: Option<f64>,
}

/// Change-detection metadata stored in a separate sidecar (`lore.stamps`).
/// Only loaded during ingest/watch.
#[derive(Debug, Clone, Serialize, Deserialize, bitcode::Encode, bitcode::Decode)]
pub struct StampsMeta {
    pub content_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime_ns: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
}

/// Persistent per-document metadata stored in `lore.docs`.
#[derive(Debug, Clone, Serialize, Deserialize, bitcode::Encode, bitcode::Decode)]
pub struct DocMeta {
    /// Opaque document identity hash.
    pub source_id: SourceId,
    /// Human-readable source path or URL.
    pub source: String,
    pub origin: SourceType,
    pub kind: DocKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// Average LLM quality score across chunks (0.0 = noise, 1.0 = informative).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_llm_quality_score: Option<f64>,
    /// LLM-generated document-level summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_summary: Option<String>,
    pub chunk_count: u64,
    pub word_count: u64,
}

impl DocMeta {
    /// Populate chunk-derived fields from processed chunks.
    pub fn apply_chunks(&mut self, chunks: &[Chunk], llm_summary: Option<String>) {
        if self.topic.is_none() {
            self.topic = chunks
                .first()
                .and_then(|c| c.topic.as_deref().map(str::to_owned));
        }
        self.chunk_count = chunks.len() as u64;
        self.word_count = chunks
            .iter()
            .map(|c| c.body.split_whitespace().count() as u64)
            .sum();
        self.updated_at = Some(crate::util::iso8601_now());
        let scores: Vec<f64> = chunks.iter().filter_map(|c| c.llm_quality_score).collect();
        self.avg_llm_quality_score = if scores.is_empty() {
            None
        } else {
            Some(scores.iter().sum::<f64>() / scores.len() as f64)
        };
        self.llm_summary = llm_summary;
    }
}

/// Stemming and stop-word language for the full-text index.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum IndexLanguage {
    #[default]
    English,
    Arabic,
    Danish,
    Dutch,
    Finnish,
    French,
    German,
    Greek,
    Hungarian,
    Italian,
    Norwegian,
    Portuguese,
    Romanian,
    Russian,
    Spanish,
    Swedish,
    Tamil,
    Turkish,
    /// Whitespace tokenization only -- no stemming or stop-word removal.
    None,
}

impl IndexLanguage {
    /// Convert to the Tantivy stemming language enum, or `None` for the `None` variant.
    pub(crate) fn to_tantivy(self) -> Option<tantivy::tokenizer::Language> {
        use tantivy::tokenizer::Language;
        match self {
            Self::English => Some(Language::English),
            Self::Arabic => Some(Language::Arabic),
            Self::Danish => Some(Language::Danish),
            Self::Dutch => Some(Language::Dutch),
            Self::Finnish => Some(Language::Finnish),
            Self::French => Some(Language::French),
            Self::German => Some(Language::German),
            Self::Greek => Some(Language::Greek),
            Self::Hungarian => Some(Language::Hungarian),
            Self::Italian => Some(Language::Italian),
            Self::Norwegian => Some(Language::Norwegian),
            Self::Portuguese => Some(Language::Portuguese),
            Self::Romanian => Some(Language::Romanian),
            Self::Russian => Some(Language::Russian),
            Self::Spanish => Some(Language::Spanish),
            Self::Swedish => Some(Language::Swedish),
            Self::Tamil => Some(Language::Tamil),
            Self::Turkish => Some(Language::Turkish),
            Self::None => Option::None,
        }
    }
}

impl_str_enum!(IndexLanguage {
    English => "english",
    Arabic => "arabic",
    Danish => "danish",
    Dutch => "dutch",
    Finnish => "finnish",
    French => "french",
    German => "german",
    Greek => "greek",
    Hungarian => "hungarian",
    Italian => "italian",
    Norwegian => "norwegian",
    Portuguese => "portuguese",
    Romanian => "romanian",
    Russian => "russian",
    Spanish => "spanish",
    Swedish => "swedish",
    Tamil => "tamil",
    Turkish => "turkish",
    None => "none",
});

#[cfg(any(test, feature = "ingest", feature = "test-support"))]
pub(crate) fn chunk_id(source_id: &str, body: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(source_id.as_bytes());
    h.update(b"\x00");
    h.update(body.as_bytes());
    let hash = h.finalize();
    let mut hex = String::with_capacity(32);
    crate::util::hex_encode(&hash.as_bytes()[..16], &mut hex);
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_generation_properties() {
        let sid = source_id("https://example.com/doc");
        let cid_one = chunk_id(&sid, "body one");
        let cid_two = chunk_id(&sid, "body two");
        assert_ne!(
            cid_one, cid_two,
            "different bodies must produce different chunk IDs"
        );
    }
}
