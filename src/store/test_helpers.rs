use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::store;
use crate::store::SearchHit;
use crate::types::{Chunk, DocKind, DocMeta, SourceId, SourceType};

/// Open a store at an existing path (for tests that need the path to outlive the store).
#[allow(clippy::missing_panics_doc)]
pub fn open_test_store(path: &std::path::Path) -> store::Store {
    store::Store::open(path, true, 256, crate::types::IndexLanguage::default(), 100).unwrap()
}

/// Create a temporary store backed by a `TempDir`.
#[allow(clippy::missing_panics_doc)]
pub fn temp_store() -> (store::Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = open_test_store(dir.path());
    (store, dir)
}

/// Build a minimal test `Chunk` with the given body and topic.
pub fn test_chunk(source: &str, body: &str, topic: &str, idx: i64) -> Chunk {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let sid = crate::types::source_id(source);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let unique_body = format!("{body}#{counter}");
    Chunk {
        chunk_id: crate::types::chunk_id(&sid, &unique_body),
        source_id: sid,
        source: Arc::from(source),
        origin: SourceType::Local,
        kind: DocKind::Document,
        body: unique_body.clone(),
        chunk_index: idx,
        section: Some(Arc::from("Introduction")),
        format: None,
        topic: Some(Arc::from(topic)),
        author: None,
        lang: None,
        tags: None,
        title: Some(Arc::from("Test Doc")),
        created_at: None,
        updated_at: None,
        llm_summary: None,
        llm_tags: None,
        llm_quality_score: None,
    }
}

/// Build a `SearchHit` wrapping a minimal `Chunk`.
pub fn test_search_hit(source: &str, body: &str, rank: u64) -> SearchHit {
    SearchHit {
        rank,
        score: Some(4.82),
        snippet: None,
        field_highlights: None,
        chunk_total: None,
        chunk: Chunk {
            chunk_id: "test-chunk-id".to_owned(),
            source_id: SourceId::from_raw(source),
            source: Arc::from(source),
            origin: SourceType::Local,
            kind: DocKind::Document,
            body: body.to_owned(),
            chunk_index: 0,
            section: Some(Arc::from("Introduction")),
            format: None,
            topic: Some(Arc::from("test-topic")),
            author: None,
            lang: None,
            tags: None,
            title: Some(Arc::from("Test Doc")),
            created_at: None,
            updated_at: None,
            llm_summary: None,
            llm_tags: None,
            llm_quality_score: None,
        },
    }
}

/// Build a `SearchHit` with no optional metadata (bare chunk).
pub fn test_search_hit_bare(source: &str, body: &str, rank: u64) -> SearchHit {
    SearchHit {
        rank,
        score: None,
        snippet: None,
        field_highlights: None,
        chunk_total: None,
        chunk: Chunk {
            chunk_id: "bare-chunk-id".to_owned(),
            source_id: SourceId::from_raw(source),
            source: Arc::from(source),
            origin: SourceType::Local,
            kind: DocKind::Document,
            body: body.to_owned(),
            chunk_index: 0,
            section: None,
            format: None,
            topic: None,
            author: None,
            lang: None,
            tags: None,
            title: None,
            created_at: None,
            updated_at: None,
            llm_summary: None,
            llm_tags: None,
            llm_quality_score: None,
        },
    }
}

/// Build a minimal test `DocMeta` for the given source and topic.
pub fn test_meta(source: &str, topic: &str) -> DocMeta {
    DocMeta {
        source_id: crate::types::source_id(source),
        source: source.to_owned(),
        origin: SourceType::Local,
        kind: DocKind::Document,
        format: None,
        topic: Some(topic.to_owned()),
        title: Some("Test Doc".to_owned()),
        author: None,
        lang: None,
        tags: None,
        created_at: None,
        updated_at: None,
        avg_llm_quality_score: None,
        llm_summary: None,
        chunk_count: 2,
        word_count: 100,
    }
}
