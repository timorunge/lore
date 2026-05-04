use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::Validate;
use crate::types::IndexLanguage;

const DEFAULT_STORE_PATH: &str = "./store";
const DEFAULT_WRITER_HEAP_MB: usize = 256;
const DEFAULT_PHRASE_SEARCH: bool = false;
const DEFAULT_DOC_STORE_CACHE_BLOCKS: usize = 500;

/// Store-level configuration controlling the Tantivy index.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct StoreConfig {
    /// Store directory path, relative to the config file (default: `"./store"`).
    pub path: String,
    /// Tantivy writer heap size in MB (default: 256).
    /// Divided across indexing threads. Larger values reduce segment flushes
    /// and speed up bulk ingests at the cost of higher memory use.
    pub writer_heap_mb: usize,
    /// Enable phrase query support (default: false).
    /// When false, uses `WithFreqs` instead of `WithFreqsAndPositions`,
    /// reducing index size ~18% and speeding up indexing. Exact phrase
    /// queries (`"like this"`) won't work; BM25 ranking is unaffected.
    /// Only takes effect on `--recreate` or first-time store creation.
    pub phrase_search: bool,
    /// Stemming and stop-word language (default: english).
    /// Only takes effect on `--recreate` or first-time store creation.
    pub language: IndexLanguage,
    /// Tantivy document store LRU cache size in 16 KB blocks (default: 500).
    /// Higher values trade memory for faster random-access reads.
    pub doc_store_cache_blocks: usize,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            path: DEFAULT_STORE_PATH.to_owned(),
            writer_heap_mb: DEFAULT_WRITER_HEAP_MB,
            phrase_search: DEFAULT_PHRASE_SEARCH,
            language: IndexLanguage::default(),
            doc_store_cache_blocks: DEFAULT_DOC_STORE_CACHE_BLOCKS,
        }
    }
}

impl Validate for StoreConfig {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.writer_heap_mb > 0, "store.writer_heap_mb must be > 0");
        Ok(())
    }
}
