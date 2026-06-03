//! Constants for store metadata keys in `lore.meta`.

/// Key for the store's human-readable name.
pub(crate) const NAME: &str = "name";
/// Key for the store's human-readable description.
pub(crate) const DESCRIPTION: &str = "description";
/// Key for the ISO-8601 timestamp of when the store was first created.
pub(crate) const CREATED_AT: &str = "created_at";
/// Key for the ISO-8601 timestamp of the most recent ingest.
pub(crate) const UPDATED_AT: &str = "updated_at";
/// Key for the last ingest mode used.
pub(crate) const MODE: &str = "mode";
/// The `lore` binary version that last wrote this store (from Cargo.toml).
pub(crate) const LORE_VERSION: &str = "lore_version";
/// Whether the store was created with phrase search (positional data) enabled.
pub(crate) const PHRASE_SEARCH: &str = "phrase_search";
/// Tantivy writer heap size in MB used when the index was last written.
pub(crate) const WRITER_HEAP_MB: &str = "writer_heap_mb";
/// Stemming/stop-word language used to build the tokenizer pipeline.
pub(crate) const LANGUAGE: &str = "language";
/// Fingerprint of the chunker-relevant processing config (max/min chunk chars).
/// Used to warn when a store was built with different chunking parameters.
#[cfg(feature = "ingest")]
pub(crate) const CHUNK_CONFIG: &str = "chunk_config";
