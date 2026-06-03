// Documentation lints are enforced on the public library surface only (the
// workspace table leaves them allow-by-default so binaries aren't burdened).
#![warn(clippy::missing_errors_doc, clippy::missing_panics_doc)]
//! Knowledge base library: ingest, index, search, and serve documents over MCP.

/// Infallible `write!` to a `String` -- panics on `Err` (which `String` never returns).
#[macro_export]
macro_rules! w {
    ($dst:expr, $($arg:tt)*) => { write!($dst, $($arg)*).unwrap() };
}

/// Infallible `writeln!` to a `String` -- panics on `Err` (which `String` never returns).
#[macro_export]
macro_rules! wln {
    ($dst:expr, $($arg:tt)*) => { writeln!($dst, $($arg)*).unwrap() };
}

/// Build version, including short git hash for non-release builds.
pub const VERSION: &str = env!("LORE_VERSION");

pub mod error;

#[cfg(feature = "ingest")]
pub mod cache;
pub mod config;
pub mod fmt;
#[cfg(feature = "ingest")]
pub mod ingest;
#[cfg(feature = "llm")]
pub mod llm;
pub mod net;
pub mod output;
pub mod query;
pub mod store;
pub mod types;
pub mod util;

#[doc(hidden)]
pub mod bench {
    /// Re-export of query sanitization for benchmarking.
    pub fn sanitize_query(q: &str) -> String {
        crate::store::schema::sanitize_query(q)
    }
    /// Re-export of visible width calculation for benchmarking.
    pub fn visible_width(s: &str) -> usize {
        crate::fmt::wrap::visible_width(s)
    }
    /// Re-export of markdown chunking for benchmarking.
    #[cfg(feature = "ingest")]
    pub fn chunk_markdown(content: &str, max_chars: usize, min_chars: usize) -> usize {
        let attrs = crate::ingest::chunker::ChunkAttrs {
            source_id: crate::types::SourceId::from("bench"),
            source: std::sync::Arc::from("bench.md"),
            origin: crate::types::SourceType::Local,
            kind: crate::types::DocKind::Document,
            format: Some(std::sync::Arc::from("md")),
            title: None,
            author: None,
            lang: None,
            created_at: None,
            tags: None,
            topic: Some(std::sync::Arc::from("bench")),
        };
        crate::ingest::chunker::chunk_markdown_content(content, &attrs, &[], max_chars, min_chars)
            .len()
    }
}

#[cfg(fuzzing)]
#[doc(hidden)]
pub mod fuzz {
    pub fn sanitize_query(q: &str) -> String {
        crate::store::schema::sanitize_query(q)
    }
    pub fn visible_width(s: &str) -> usize {
        crate::fmt::wrap::visible_width(s)
    }
    #[cfg(feature = "ingest")]
    pub fn chunk_markdown(content: &str) {
        crate::ingest::chunker::fuzz_entry(content);
    }
}

/// Helpers for integration tests that need to read/write `lore.docs`.
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub mod store_test_support {
    use std::path::Path;

    use crate::store::types::{DOCS_FORMAT_VERSION, read_sidecar, write_sidecar};

    pub fn read_store_docs_json(store_path: &Path) -> serde_json::Value {
        let docs_path = store_path.join("lore.docs");
        if !docs_path.exists() {
            return serde_json::Value::Array(Vec::new());
        }
        let bytes = std::fs::read(&docs_path).expect("failed to read lore.docs");
        let payload =
            read_sidecar(&bytes, DOCS_FORMAT_VERSION, "lore.docs").expect("invalid lore.docs");
        let docs: Vec<crate::types::DocMeta> =
            bitcode::decode(payload).expect("failed to decode lore.docs");
        serde_json::to_value(docs).expect("DocMeta to JSON Value")
    }

    pub fn write_store_docs_json(store_path: &Path, value: serde_json::Value) {
        let docs: Vec<crate::types::DocMeta> =
            serde_json::from_value(value).expect("JSON Value to Vec<DocMeta>");
        let payload = bitcode::encode(&docs);
        let bytes = write_sidecar(DOCS_FORMAT_VERSION, &payload);
        crate::util::atomic_write(&store_path.join("lore.docs"), &bytes).expect("write lore.docs");
    }
}
