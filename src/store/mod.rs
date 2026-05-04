mod collector;
mod documents;
mod info;
pub(crate) mod meta_key;
pub(crate) mod multi;
mod resolve;
pub(crate) mod schema;

pub use schema::{SEARCH_FIELDS, SearchFieldDef};
mod search;
#[cfg(any(test, feature = "test-support"))]
pub mod test_helpers;
mod topics;
pub(crate) mod types;
mod writer;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock, atomic::AtomicBool};

use anyhow::{Context, Result};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy};
use tracing::info;

use crate::types::{DocMeta, SourceId, StampsMeta};
use crate::util;
pub(crate) use collector::DiversityCollector;
pub use multi::StoreSet;
use schema::{Fields, build_schema, fields, register_tokenizer};
#[cfg(all(test, feature = "ingest"))]
use types::DOCS_FILE;
#[cfg(feature = "ingest")]
pub(crate) use types::LOCK_FILE;
use types::{
    DOCS_FORMAT_VERSION, MARKER_FILE, MARKER_SALT, META_FORMAT_VERSION, STAMPS_FORMAT_VERSION,
    TANTIVY_DIR, load_sidecar_file, read_sidecar, store_docs_path, store_meta_path,
    store_stamps_path,
};
pub use types::{
    DocDetail, DocFilter, DocSort, FieldHighlights, SearchHit, SearchQuery, SearchSort,
    SourceResolveResult, StoreInfo, TopicFilter, TopicResult, TopicSort, TopicStat,
};

/// SI megabytes (not MiB) to match Tantivy's heap budget convention.
const BYTES_PER_MB: usize = 1_000_000;

/// Tantivy-backed document store with metadata and full-text search.
pub struct Store {
    path: PathBuf,
    index: Index,
    reader: IndexReader,
    writer: Mutex<Option<IndexWriter>>,
    fields: Fields,
    search_parser: tantivy::query::QueryParser,
    /// Tantivy writer heap budget in bytes.
    writer_heap_bytes: usize,
    documents: OnceLock<RwLock<HashMap<SourceId, DocMeta>>>,
    stamps: OnceLock<RwLock<HashMap<SourceId, StampsMeta>>>,
    metadata: RwLock<HashMap<String, String>>,
    /// Set when any write occurs (chunks, documents, metadata). Cleared on commit.
    dirty: AtomicBool,
    /// Set when lore.docs content changes. Cleared after sidecar write.
    docs_dirty: AtomicBool,
    /// Set when lore.stamps content changes. Cleared after sidecar write.
    stamps_dirty: AtomicBool,
}

impl Store {
    /// Open a store for reading with default settings (phrase search on, default heap).
    /// The language is read from lore.meta if present, falling back to English.
    pub fn open_readonly(path: &Path) -> Result<Self> {
        use crate::types::IndexLanguage;
        let preloaded = load_meta_map(path);
        let lang = preloaded
            .as_ref()
            .and_then(|m| m.get(meta_key::LANGUAGE))
            .and_then(|v| v.parse::<IndexLanguage>().ok())
            .unwrap_or_default();
        Self::open_inner(path, true, 256, lang, 500, preloaded)
    }

    /// Open or create a store at `path`.
    pub fn open(
        path: &Path,
        phrase_search: bool,
        writer_heap_mb: usize,
        language: crate::types::IndexLanguage,
        doc_store_cache_blocks: usize,
    ) -> Result<Self> {
        Self::open_inner(
            path,
            phrase_search,
            writer_heap_mb,
            language,
            doc_store_cache_blocks,
            None,
        )
    }

    fn open_inner(
        path: &Path,
        phrase_search: bool,
        writer_heap_mb: usize,
        language: crate::types::IndexLanguage,
        doc_store_cache_blocks: usize,
        preloaded_meta: Option<HashMap<String, String>>,
    ) -> Result<Self> {
        std::fs::create_dir_all(path)
            .with_context(|| format!("failed to create store directory: {}", path.display()))?;

        let schema = build_schema(phrase_search);
        let index_path = path.join(TANTIVY_DIR);

        let index = if index_path.exists() {
            let idx = Index::open_in_dir(&index_path).context("failed to open Tantivy index")?;
            if idx.schema().get_field(fields::CHUNK_ID).is_err()
                || idx.schema().get_field(fields::SOURCE).is_err()
            {
                return Err(crate::error::LoreError::SchemaOutdated.into());
            }
            idx
        } else {
            std::fs::create_dir_all(&index_path)?;
            Index::builder()
                .schema(schema.clone())
                .settings(tantivy::IndexSettings {
                    docstore_compression: tantivy::store::Compressor::Zstd(
                        tantivy::store::ZstdCompressor {
                            compression_level: Some(3),
                        },
                    ),
                    ..tantivy::IndexSettings::default()
                })
                .create_in_dir(&index_path)
                .context("failed to create Tantivy index")?
        };

        register_tokenizer(&index, language);

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .doc_store_cache_num_blocks(doc_store_cache_blocks)
            .try_into()
            .context("failed to create index reader")?;

        let metadata = if let Some(m) = preloaded_meta {
            m
        } else {
            let meta_path = store_meta_path(path);
            if meta_path.exists() {
                load_sidecar_file::<HashMap<String, String>>(
                    &meta_path,
                    META_FORMAT_VERSION,
                    "lore.meta",
                )?
            } else {
                HashMap::new()
            }
        };

        let actual_schema = index.schema();
        let fields =
            Fields::from_schema(&actual_schema).context("schema field resolution failed")?;

        let search_fields = fields.search_fields();
        let mut search_parser = tantivy::query::QueryParser::new(
            actual_schema.clone(),
            search_fields.iter().map(|(field, _)| *field).collect(),
            index.tokenizers().clone(),
        );
        for (field, boost) in &search_fields {
            search_parser.set_field_boost(*field, *boost as f32);
        }

        util::ensure_marker(path, MARKER_FILE, MARKER_SALT)?;
        info!(path = %path.display(), "opened Tantivy store");
        Ok(Self {
            path: path.to_owned(),
            index,
            reader,
            writer: Mutex::new(None),
            fields,
            search_parser,
            writer_heap_bytes: writer_heap_mb * BYTES_PER_MB,
            documents: OnceLock::new(),
            stamps: OnceLock::new(),
            metadata: RwLock::new(metadata),
            dirty: AtomicBool::new(false),
            docs_dirty: AtomicBool::new(false),
            stamps_dirty: AtomicBool::new(false),
        })
    }

    /// Return the filesystem path of this store.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Re-read lore.docs, lore.stamps, and lore.meta from disk into the in-memory
    /// caches and reload the Tantivy reader so a read-only store picks up
    /// changes written by an external ingest process.
    pub fn reload_sidecars(&self) {
        if let Some(lock) = self.documents.get() {
            match load_docs(&self.path) {
                Ok(fresh) => *rw_write(lock) = fresh,
                Err(e) => tracing::warn!("reload docs sidecar failed: {e}"),
            }
        }
        if let Some(lock) = self.stamps.get() {
            match load_stamps(&self.path) {
                Ok(fresh) => *rw_write(lock) = fresh,
                Err(e) => tracing::warn!("reload stamps sidecar failed: {e}"),
            }
        }
        let meta_path = store_meta_path(&self.path);
        if meta_path.exists() {
            match load_sidecar_file::<HashMap<String, String>>(
                &meta_path,
                META_FORMAT_VERSION,
                "lore.meta",
            ) {
                Ok(fresh) => *rw_write(&self.metadata) = fresh,
                Err(e) => tracing::warn!("reload meta sidecar failed: {e}"),
            }
        }
        if let Err(e) = self.reader.reload() {
            tracing::warn!("tantivy reader reload failed: {e}");
        }
    }

    #[cfg(feature = "ingest")]
    pub(crate) async fn destroy(path: &Path) -> Result<()> {
        if path.exists() {
            // verify_marker is a fast local read; no spawn_blocking needed.
            anyhow::ensure!(
                util::verify_marker(path, MARKER_FILE, MARKER_SALT),
                "refusing to destroy {}: not a lore store (missing or invalid {} marker)",
                path.display(),
                MARKER_FILE,
            );
            tokio::fs::remove_dir_all(path)
                .await
                .with_context(|| format!("failed to remove store: {}", path.display()))?;
        }
        Ok(())
    }

    #[cfg(feature = "ingest")]
    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty.load(std::sync::atomic::Ordering::Acquire)
    }

    fn documents_lock(&self) -> &RwLock<HashMap<SourceId, DocMeta>> {
        self.documents.get_or_init(|| {
            let docs = load_docs(&self.path).unwrap_or_else(|e| {
                tracing::warn!("failed to load docs sidecar: {e}");
                HashMap::new()
            });
            RwLock::new(docs)
        })
    }

    fn read_docs(&self) -> std::sync::RwLockReadGuard<'_, HashMap<SourceId, DocMeta>> {
        rw_read(self.documents_lock())
    }

    fn write_docs(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<SourceId, DocMeta>> {
        rw_write(self.documents_lock())
    }

    fn read_meta(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, String>> {
        rw_read(&self.metadata)
    }

    #[cfg(feature = "ingest")]
    fn write_meta(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, String>> {
        rw_write(&self.metadata)
    }

    fn stamps_lock(&self) -> &RwLock<HashMap<SourceId, StampsMeta>> {
        self.stamps.get_or_init(|| {
            let fm = load_stamps(&self.path).unwrap_or_else(|e| {
                tracing::warn!("failed to load stamps sidecar: {e}");
                HashMap::new()
            });
            RwLock::new(fm)
        })
    }

    fn read_stamps(&self) -> std::sync::RwLockReadGuard<'_, HashMap<SourceId, StampsMeta>> {
        rw_read(self.stamps_lock())
    }

    fn write_stamps(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<SourceId, StampsMeta>> {
        rw_write(self.stamps_lock())
    }
}

fn rw_read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn rw_write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn load_meta_map(store_path: &Path) -> Option<HashMap<String, String>> {
    let meta_path = store_meta_path(store_path);
    if !meta_path.exists() {
        return None;
    }
    let bytes = std::fs::read(&meta_path).ok()?;
    let payload = read_sidecar(&bytes, META_FORMAT_VERSION, "lore.meta").ok()?;
    bitcode::decode::<HashMap<String, String>>(payload).ok()
}

fn load_docs(store_path: &Path) -> Result<HashMap<SourceId, DocMeta>> {
    let path = store_docs_path(store_path);
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let docs_vec: Vec<DocMeta> = load_sidecar_file(&path, DOCS_FORMAT_VERSION, "lore.docs")?;
    Ok(docs_vec
        .into_iter()
        .map(|d| (d.source_id.clone(), d))
        .collect())
}

fn load_stamps(store_path: &Path) -> Result<HashMap<SourceId, StampsMeta>> {
    let path = store_stamps_path(store_path);
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let entries: Vec<(SourceId, StampsMeta)> =
        load_sidecar_file(&path, STAMPS_FORMAT_VERSION, "lore.stamps")?;
    Ok(entries.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "ingest")]
    use crate::store::test_helpers::open_test_store;
    use crate::store::test_helpers::{temp_store, test_chunk, test_meta};
    use crate::types::SourceType;

    #[test]
    fn insert_and_delete_chunks() {
        let (store, _dir) = temp_store();
        store
            .insert_chunks(&[
                test_chunk("/a.md", "chunk a content", "t", 0),
                test_chunk("/b.md", "chunk b content", "t", 0),
            ])
            .unwrap();
        store.commit().unwrap();
        assert_eq!(store.reader.searcher().num_docs(), 2);

        store
            .delete_chunks_by_source(&crate::types::source_id("/a.md"))
            .unwrap();
        store.commit().unwrap();
        assert_eq!(store.reader.searcher().num_docs(), 1);
    }

    #[test]
    #[cfg(feature = "ingest")]
    fn store_persistence_invariants() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = open_test_store(dir.path());
            store.set_metadata("key", "value");
            store.upsert_document(test_meta("/a.md", "t"));
            store.commit().unwrap();
        }
        {
            let store = open_test_store(dir.path());
            assert_eq!(store.get_metadata("key").as_deref(), Some("value"));
            assert_eq!(store.get_all_documents().len(), 1);
        }

        // commit_index does not flush lore.docs; commit() does
        let dir = tempfile::tempdir().unwrap();
        let docs_file = dir.path().join(DOCS_FILE);
        let store = open_test_store(dir.path());
        store
            .insert_chunks(&[test_chunk("/a.md", "searchable content here", "topic", 0)])
            .unwrap();
        store.upsert_document(test_meta("/a.md", "topic"));
        store.commit_index().unwrap();
        assert!(
            !docs_file.exists(),
            "lore.docs should not be written by commit_index"
        );
        let (results, _) = store.search(&SearchQuery::new("searchable")).unwrap();
        assert!(!results.is_empty(), "search should work after commit_index");
        store.commit().unwrap();
        assert!(
            docs_file.exists(),
            "lore.docs should be written by full commit"
        );

        // optimize() writes lore.docs and the data survives a reopen
        let dir = tempfile::tempdir().unwrap();
        let docs_file = dir.path().join(DOCS_FILE);
        {
            let store = open_test_store(dir.path());
            store
                .insert_chunks(&[
                    test_chunk("/a.md", "first document content here", "t", 0),
                    test_chunk("/b.md", "second document content here", "t", 0),
                ])
                .unwrap();
            store.upsert_document(test_meta("/a.md", "t"));
            store.upsert_document(test_meta("/b.md", "t"));
            store.optimize(|_| {}).unwrap();
            assert!(
                docs_file.exists(),
                "optimize() must write lore.docs even without a prior commit()"
            );
        }
        {
            let store2 = open_test_store(dir.path());
            assert_eq!(
                store2.get_all_documents().len(),
                2,
                "reopened store should reflect both documents written by optimize()"
            );
        }
    }

    #[test]
    fn search_returns_bm25_ranked_results() {
        let (store, _dir) = temp_store();
        let highly_relevant = test_chunk(
            "/arch.md",
            "The system architecture defines structural patterns. Architecture \
             decisions shape the entire system. Good architecture enables scalability.",
            "tech",
            0,
        );
        let marginally_relevant = test_chunk(
            "/intro.md",
            "This is a general introduction. It briefly mentions architecture once.",
            "tech",
            0,
        );
        let irrelevant = test_chunk(
            "/other.md",
            "Buy milk, eggs, and bread. Also get some vegetables and fruit.",
            "tech",
            0,
        );
        store
            .insert_chunks(&[highly_relevant, marginally_relevant, irrelevant])
            .unwrap();
        store.commit().unwrap();

        let (results, _) = store.search(&SearchQuery::new("architecture")).unwrap();
        assert!(!results.is_empty(), "expected at least one search result");
        assert_eq!(
            &*results[0].chunk.source, "/arch.md",
            "most relevant chunk should come first"
        );
        for window in results.windows(2) {
            let s0 = window[0].score.unwrap();
            let s1 = window[1].score.unwrap();
            assert!(s0 >= s1, "results should be sorted descending: {s0} < {s1}");
        }
    }

    #[test]
    fn search_topic_filter_excludes_other_topics() {
        let (store, _dir) = temp_store();
        store
            .insert_chunks(&[
                test_chunk("/a.md", "rust programming language features", "rust", 0),
                test_chunk("/b.md", "rust systems programming concepts", "systems", 0),
                test_chunk("/c.md", "rust language introduction guide", "rust", 0),
            ])
            .unwrap();
        store.commit().unwrap();

        let (results, _) = store
            .search(&SearchQuery {
                topic: Some("rust".to_owned()),
                ..SearchQuery::new("rust")
            })
            .unwrap();
        assert!(!results.is_empty(), "topic filter should return results");
        for r in &results {
            assert_eq!(
                r.chunk.topic.as_deref(),
                Some("rust"),
                "all results should have topic=rust"
            );
        }
    }

    #[test]
    fn resolve_topic_name_uses_exact_then_case_then_substr() {
        let (store, _dir) = temp_store();
        store
            .insert_chunks(&[
                test_chunk("/arch.md", "architecture content", "Architecture", 0),
                test_chunk("/impl.md", "implementation content", "Implementation", 0),
            ])
            .unwrap();
        store.commit().unwrap();

        assert_eq!(
            store.resolve_topic_name("Architecture"),
            Some("Architecture".to_owned()),
            "exact match"
        );
        assert_eq!(
            store.resolve_topic_name("architecture"),
            Some("Architecture".to_owned()),
            "case-insensitive"
        );
        assert_eq!(
            store.resolve_topic_name("Arch"),
            Some("Architecture".to_owned()),
            "substring match"
        );
        assert_eq!(
            store.resolve_topic_name("impl"),
            Some("Implementation".to_owned()),
            "lowercase substring"
        );
        assert_eq!(
            store.resolve_topic_name("nonexistent"),
            None,
            "no match returns None"
        );
    }

    #[test]
    fn store_info_aggregates_counts_and_origins() {
        let (store, _dir) = temp_store();
        let mut a = test_meta("/a.md", "t");
        a.lang = Some("en".to_owned());
        a.chunk_count = 3;
        a.word_count = 150;
        store.upsert_document(a);
        let mut b = test_meta("/b.md", "t");
        b.lang = Some("en".to_owned());
        b.chunk_count = 2;
        b.word_count = 100;
        store.upsert_document(b);
        let mut url = test_meta("https://example.com/page", "t");
        url.origin = SourceType::Url;
        url.lang = Some("de".to_owned());
        url.chunk_count = 4;
        url.word_count = 200;
        store.upsert_document(url);
        let mut s3 = test_meta("s3://bucket/key", "t");
        s3.origin = SourceType::S3;
        s3.chunk_count = 1;
        s3.word_count = 50;
        store.upsert_document(s3);

        let info = store.store_info();
        assert_eq!(info.documents, 4);
        assert_eq!(info.chunks, 10, "3+2+4+1=10");
        assert_eq!(info.words, 500, "150+100+200+50=500");
        assert_eq!(info.source_types.get(&SourceType::Local).copied(), Some(2));
        assert_eq!(info.source_types.get(&SourceType::Url).copied(), Some(1));
        assert_eq!(info.source_types.get(&SourceType::S3).copied(), Some(1));
        assert_eq!(info.languages.get("en").copied(), Some(2));
        assert_eq!(info.languages.get("de").copied(), Some(1));
        assert!(
            !info.languages.contains_key(""),
            "empty lang should not appear"
        );
    }

    #[test]
    fn search_max_per_source_caps_results_per_document() {
        let (store, _dir) = temp_store();
        store
            .insert_chunks(&[
                test_chunk("/a.md", "rust programming one", "tech", 0),
                test_chunk("/a.md", "rust programming two", "tech", 1),
                test_chunk("/a.md", "rust programming three", "tech", 2),
                test_chunk("/b.md", "rust systems one", "tech", 0),
                test_chunk("/b.md", "rust systems two", "tech", 1),
            ])
            .unwrap();
        store.commit().unwrap();
        let (results, total) = store
            .search(&SearchQuery {
                max_per_source: Some(1),
                ..SearchQuery::new("rust")
            })
            .unwrap();
        assert_eq!(
            results.len(),
            2,
            "expected 1 per source = 2 total, got {}",
            results.len()
        );
        assert_eq!(
            total, 5,
            "total is the index-level match count, not post-filter"
        );
    }

    #[test]
    fn source_filter_with_max_per_source_returns_full_limit() {
        let (store, _dir) = temp_store();
        let mut chunks = Vec::new();
        for i in 0..6_i64 {
            let source = format!("/docs/relevant-{i}.md");
            for j in 0..3_i64 {
                chunks.push(test_chunk(
                    &source,
                    "relevant content about systems programming",
                    "tech",
                    j,
                ));
            }
        }
        for i in 0..3_i64 {
            let source = format!("/docs/other-{i}.md");
            chunks.push(test_chunk(
                &source,
                "relevant content about systems programming",
                "tech",
                0,
            ));
        }
        store.insert_chunks(&chunks).unwrap();
        store.commit().unwrap();

        let (results, total) = store
            .search(&SearchQuery {
                source: Some("relevant".to_owned()),
                max_per_source: Some(1),
                limit: 5,
                ..SearchQuery::new("relevant")
            })
            .unwrap();

        assert_eq!(
            results.len(),
            5,
            "expected 5 results (limit), got {}",
            results.len()
        );
        assert_eq!(
            total, 21,
            "total is the index-level match count (all chunks matching query)"
        );
        for r in &results {
            assert!(
                r.chunk.source.contains("relevant"),
                "unexpected source in result: {}",
                r.chunk.source
            );
        }
    }

    #[test]
    fn search_returns_snippets() {
        let (store, _dir) = temp_store();
        store
            .insert_chunks(&[test_chunk(
                "/a.md",
                "The authentication system uses JWT tokens for secure access control",
                "t",
                0,
            )])
            .unwrap();
        store.commit().unwrap();
        let (results, _) = store.search(&SearchQuery::new("authentication")).unwrap();
        assert!(!results.is_empty());
        let hit = &results[0];
        assert!(hit.snippet.is_some(), "search should produce snippet");
        let snip = hit.snippet.as_ref().unwrap();
        assert!(
            !snip.highlights.is_empty(),
            "snippet should have highlights"
        );
    }

    #[test]
    fn search_empty_query_returns_error() {
        let (store, _dir) = temp_store();
        store
            .insert_chunks(&[test_chunk("/a.md", "content", "t", 0)])
            .unwrap();
        store.commit().unwrap();
        let err = store.search(&SearchQuery::new("")).unwrap_err();
        assert!(
            matches!(err, crate::error::LoreError::EmptyQuery),
            "blank query should return EmptyQuery, got: {err:?}"
        );
        let err = store.search(&SearchQuery::new("   ")).unwrap_err();
        assert!(
            matches!(err, crate::error::LoreError::EmptyQuery),
            "whitespace-only query should return EmptyQuery, got: {err:?}"
        );
    }

    #[test]
    fn search_sort_cases() {
        let (store, _dir) = temp_store();
        store
            .insert_chunks(&[
                test_chunk("/z.md", "programming concepts overview", "Zebra", 0),
                test_chunk("/a.md", "programming language basics", "Alpha", 0),
                test_chunk("/m.md", "programming paradigms guide", "Middle", 0),
            ])
            .unwrap();
        store.commit().unwrap();

        // SearchSort::Source ascending
        let (asc, _) = store
            .search(&SearchQuery {
                sort: SearchSort::Source,
                ..SearchQuery::new("programming")
            })
            .unwrap();
        assert!(asc.len() >= 3, "should return all three results");
        for w in asc.windows(2) {
            assert!(
                w[0].chunk.source <= w[1].chunk.source,
                "expected ascending source order: {} > {}",
                w[0].chunk.source,
                w[1].chunk.source
            );
        }

        // SearchSort::Source with reverse=true
        let (desc, _) = store
            .search(&SearchQuery {
                sort: SearchSort::Source,
                reverse: true,
                ..SearchQuery::new("programming")
            })
            .unwrap();
        assert_eq!(asc.len(), desc.len());
        assert_eq!(
            asc.first().unwrap().chunk.source,
            desc.last().unwrap().chunk.source,
            "reverse should invert the order"
        );

        // SearchSort::Topic ascending
        let (results, _) = store
            .search(&SearchQuery {
                sort: SearchSort::Topic,
                ..SearchQuery::new("programming")
            })
            .unwrap();
        assert!(results.len() >= 2, "should return multiple results");
        for w in results.windows(2) {
            assert!(
                w[0].chunk.topic <= w[1].chunk.topic,
                "expected ascending topic order: {:?} > {:?}",
                w[0].chunk.topic,
                w[1].chunk.topic
            );
        }
    }
}
