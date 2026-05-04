use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::IngestConfig;
use crate::ingest::IngestMode;
use crate::ingest::transforms::CompiledProfile;
use crate::ingest::types::LoaderResult;
#[cfg(feature = "llm")]
use crate::llm::LlmClient;
use crate::store::Store;
use crate::types::{Chunk, DocMeta, SourceId, StampsMeta};

/// Periodic commit state for the streaming write task.
///
/// Encapsulates the timing check so the pipeline code only calls `maybe_commit()`
/// and the commit + status update happen transparently.
pub(crate) struct PeriodicCommit<'a> {
    pub(crate) store: &'a Store,
    pub(crate) interval: Duration,
    pub(crate) last: Instant,
    pub(crate) observer: &'a dyn crate::ingest::IngestObserver,
    pub(crate) sources: &'a [crate::config::SourceConfig],
    pub(crate) discovered_total: Option<usize>,
}

impl PeriodicCommit<'_> {
    pub(crate) fn maybe_commit(&mut self) {
        if self.interval.is_zero() || self.last.elapsed() < self.interval {
            return;
        }
        crate::ingest::commit_and_update_status(
            self.store,
            self.observer,
            self.sources,
            false,
            self.discovered_total,
        );
        self.last = Instant::now();
    }
}

/// Shared processing context, invariant across all sources in an ingest run.
#[derive(Clone)]
pub(crate) struct ProcessContext {
    pub(crate) store: Arc<Store>,
    pub(crate) config: Arc<IngestConfig>,
    pub(crate) default_profile: Arc<CompiledProfile>,
    pub(crate) mode: IngestMode,
    pub(crate) dry_run: bool,
    pub(crate) force: bool,
    pub(crate) existing_docs: Arc<HashMap<SourceId, DocMeta>>,
    pub(crate) existing_stamps: Arc<HashMap<SourceId, StampsMeta>>,
    pub(crate) text_ext: Option<Arc<Vec<String>>>,
    /// Present only when the `llm` feature is enabled and an LLM config is
    /// provided. When absent, topic detection and chunk enrichment are skipped.
    #[cfg(feature = "llm")]
    pub(crate) llm_client: Option<Arc<LlmClient>>,
}

/// Return the document's effective content hash, using the override if present.
pub(crate) fn effective_hash(doc: &LoaderResult) -> String {
    if let Some(hash) = &doc.content_hash_override {
        return hash.clone();
    }
    crate::util::blake3_hex(&doc.content)
}

/// Build the `DocMeta` record for a processed document from its chunks and optional LLM summary.
/// Takes `&mut` to move fields out of the `LoaderResult` instead of cloning.
pub(crate) fn build_document_meta(
    doc: &mut LoaderResult,
    chunks: &[Chunk],
    llm_summary: Option<String>,
) -> DocMeta {
    let mut meta = DocMeta {
        source_id: doc.source_id.clone(),
        source: std::mem::take(&mut doc.source),
        origin: doc.origin,
        kind: doc.kind,
        format: doc.format.take(),
        topic: doc.topic.take(),
        title: doc.title.take(),
        author: doc.author.take(),
        lang: doc.lang.take(),
        tags: doc.tags.take(),
        created_at: doc.created_at.take(),
        updated_at: None,
        avg_llm_quality_score: None,
        llm_summary: None,
        chunk_count: 0,
        word_count: 0,
    };
    meta.apply_chunks(chunks, llm_summary);
    meta
}

/// Build the `StampsMeta` record for a processed document from its loader result and hash.
/// Takes `&mut` to move fields out of the `LoaderResult` instead of cloning.
pub(crate) fn build_stamps_meta(doc: &mut LoaderResult, content_hash: String) -> StampsMeta {
    StampsMeta {
        content_hash,
        etag: doc.etag.take(),
        last_modified: doc.last_modified.take(),
        mtime_ns: doc.mtime_ns,
        size_bytes: doc.size_bytes,
    }
}

/// Returns `true` if the document is unchanged (mtime+size, ETag, or content hash).
pub(crate) fn is_unchanged(doc: &LoaderResult, existing: &HashMap<SourceId, StampsMeta>) -> bool {
    let Some(prev) = existing.get(&doc.source_id) else {
        return false;
    };

    if let (Some(new_mtime), Some(old_mtime)) = (doc.mtime_ns, prev.mtime_ns)
        && let (Some(new_size), Some(old_size)) = (doc.size_bytes, prev.size_bytes)
        && new_mtime == old_mtime
        && new_size == old_size
    {
        return true;
    }

    if let (Some(new_etag), Some(old_etag)) = (&doc.etag, &prev.etag)
        && new_etag == old_etag
    {
        return true;
    }

    let new_hash = effective_hash(doc);
    new_hash == prev.content_hash
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::ingest::types::LoaderResult;
    use crate::types::{Chunk, DocKind, SourceId, SourceType, StampsMeta};

    use super::{build_document_meta, effective_hash, is_unchanged};

    fn make_loader_result(source: &str) -> LoaderResult {
        let mut doc = LoaderResult::test_doc("hello world");
        doc.source_id = SourceId::from_raw(source);
        doc.source = source.to_string();
        doc
    }

    fn make_chunk(body: &str, topic: Option<&str>) -> Chunk {
        Chunk {
            chunk_id: "id".to_string(),
            source_id: SourceId::from_raw("src"),
            source: Arc::from("src"),
            origin: SourceType::Local,
            kind: DocKind::Document,
            body: body.to_string(),
            chunk_index: 0,
            section: None,
            format: None,
            topic: topic.map(Arc::from),
            author: None,
            lang: None,
            tags: None,
            title: None,
            created_at: None,
            updated_at: None,
            llm_summary: None,
            llm_tags: None,
            llm_quality_score: None,
        }
    }

    fn make_stamps(hash: &str) -> StampsMeta {
        StampsMeta {
            content_hash: hash.to_string(),
            etag: None,
            last_modified: None,
            mtime_ns: None,
            size_bytes: None,
        }
    }

    #[test]
    fn pipeline_hash_and_staleness_cases() {
        // effective_hash: normal content hash
        let doc = make_loader_result("src/doc.md");
        let expected = crate::util::blake3_hex("hello world");
        assert_eq!(effective_hash(&doc), expected);

        // effective_hash: content_hash_override takes precedence
        let mut doc = make_loader_result("src/doc.md");
        doc.content_hash_override = Some("override-abc".to_string());
        assert_eq!(effective_hash(&doc), "override-abc");

        // is_unchanged: unknown source
        let doc = make_loader_result("new-doc");
        assert!(!is_unchanged(&doc, &HashMap::new()), "unknown source");

        // is_unchanged: hash match
        let doc = make_loader_result("doc");
        let hash = effective_hash(&doc);
        let mut existing = HashMap::new();
        existing.insert(SourceId::from_raw("doc"), make_stamps(&hash));
        assert!(is_unchanged(&doc, &existing), "hash match");

        // is_unchanged: hash mismatch
        let mut existing = HashMap::new();
        existing.insert(SourceId::from_raw("doc"), make_stamps("stale-hash"));
        assert!(!is_unchanged(&doc, &existing), "hash mismatch");

        // is_unchanged: mtime+size short-circuit
        let mut doc = make_loader_result("doc");
        doc.mtime_ns = Some(1_000_000);
        doc.size_bytes = Some(42);
        let mut fm = make_stamps("stale-hash");
        fm.mtime_ns = Some(1_000_000);
        fm.size_bytes = Some(42);
        let mut existing = HashMap::new();
        existing.insert(SourceId::from_raw("doc"), fm);
        assert!(is_unchanged(&doc, &existing), "mtime+size short-circuit");

        // is_unchanged: etag match
        let mut doc = make_loader_result("doc");
        doc.etag = Some("\"v1\"".to_string());
        let mut fm = make_stamps("stale-hash");
        fm.etag = Some("\"v1\"".to_string());
        let mut existing = HashMap::new();
        existing.insert(SourceId::from_raw("doc"), fm);
        assert!(is_unchanged(&doc, &existing), "etag match short-circuit");

        // is_unchanged: etag mismatch falls through to hash
        let mut doc = make_loader_result("doc");
        doc.etag = Some("\"v2\"".to_string());
        let hash = effective_hash(&doc);
        let mut fm = make_stamps(&hash);
        fm.etag = Some("\"v1\"".to_string());
        let mut existing = HashMap::new();
        existing.insert(SourceId::from_raw("doc"), fm);
        assert!(
            is_unchanged(&doc, &existing),
            "etag mismatch falls through to hash"
        );
    }

    #[test]
    fn build_document_meta_cases() {
        let mut doc = make_loader_result("doc");
        let chunks = vec![
            make_chunk("body", Some("rust")),
            make_chunk("body2", Some("other")),
        ];
        let meta = build_document_meta(&mut doc, &chunks, None);
        assert_eq!(
            meta.topic.as_deref(),
            Some("rust"),
            "topic from first chunk"
        );

        let mut doc = make_loader_result("doc");
        doc.topic = Some("fallback-topic".to_string());
        let chunks = vec![make_chunk("body", None)];
        let meta = build_document_meta(&mut doc, &chunks, None);
        assert_eq!(
            meta.topic.as_deref(),
            Some("fallback-topic"),
            "doc topic fallback"
        );

        let mut doc = make_loader_result("doc");
        let chunks = vec![
            make_chunk("one two three", None),
            make_chunk("four five", None),
        ];
        let meta = build_document_meta(&mut doc, &chunks, None);
        assert_eq!(meta.chunk_count, 2, "chunk count");
        assert_eq!(meta.word_count, 5, "word count across chunks");
    }
}
