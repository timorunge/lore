use std::sync::Mutex;
use std::sync::atomic::Ordering;

use anyhow::{Context, Result};
use tantivy::{IndexReader, IndexWriter, TantivyDocument};

use crate::store::Store;
use crate::store::types::{
    DOCS_FORMAT_VERSION, META_FORMAT_VERSION, STAMPS_FORMAT_VERSION, store_docs_path,
    store_meta_path, store_stamps_path, write_sidecar,
};
use crate::types::{Chunk, DocMeta, StampsMeta};
use crate::util::atomic_write;

fn mutex_lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Maximum number of segments to merge in a single Tantivy merge operation.
const MAX_FANIN: usize = 256;

/// Maximum number of merge rounds in `optimize()` before giving up.
const MAX_ROUNDS: usize = 10;

impl Store {
    /// Commit only the Tantivy index without writing sidecar files.
    /// Used for periodic mid-ingest checkpoints where the cost of full
    /// serialization is too high and losing document metadata on crash
    /// is acceptable (the next ingest or `lore maintain` recovers).
    ///
    /// Unlike `commit()`, this keeps the writer alive so that Tantivy's merge
    /// policy can compact segments in the background between commits (background
    /// compaction is managed by Tantivy, not controlled directly here).
    ///
    /// # Errors
    ///
    /// Returns an error if the Tantivy commit or reader reload fails.
    pub fn commit_index(&self) -> Result<()> {
        if !self.dirty.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut guard = mutex_lock(&self.writer);
        if let Some(writer) = guard.as_mut() {
            commit_and_reload(writer, &self.reader)?;
        } else {
            self.reader.reload().context("tantivy reader reload")?;
        }
        drop(guard);
        Ok(())
    }

    /// Full commit: Tantivy index + sidecar files (`lore.meta`, `lore.docs`).
    /// Used for per-source commits and the final commit. Skips writing when
    /// no changes have been made since the last commit.
    ///
    /// # Errors
    ///
    /// Returns an error if the Tantivy commit or sidecar file writes fail.
    pub fn commit(&self) -> Result<()> {
        if !self.dirty.load(Ordering::Acquire) {
            return Ok(());
        }
        self.commit_inner(false)
    }

    /// Unconditional commit: always writes sidecar files regardless of dirty
    /// flags. Used on interrupt to guarantee persistence even if a prior
    /// commit already cleared the flags.
    ///
    /// # Errors
    ///
    /// Returns an error if the Tantivy commit or sidecar file writes fail.
    pub fn force_commit(&self) -> Result<()> {
        self.commit_inner(true)
    }

    /// Add chunks to the Tantivy index.
    ///
    /// # Errors
    ///
    /// Returns an error if the index writer cannot be initialised or adding a document fails.
    ///
    /// # Panics
    ///
    /// Panics if the writer lock is in an unexpected state (should never occur in normal use).
    pub fn insert_chunks(&self, chunks: &[Chunk]) -> Result<()> {
        let guard = self.ensure_writer()?;
        let writer = guard.as_ref().expect("writer initialized by ensure_writer");
        for chunk in chunks {
            let mut doc = TantivyDocument::new();
            doc.add_text(self.fields.chunk_id, &chunk.chunk_id);
            doc.add_text(self.fields.source_id, &chunk.source_id);
            doc.add_text(self.fields.source, &chunk.source);
            doc.add_text(self.fields.origin, chunk.origin.as_str());
            doc.add_text(self.fields.kind, chunk.kind.as_str());
            if let Some(ref fmt) = chunk.format {
                doc.add_text(self.fields.format, &**fmt);
            }
            doc.add_i64(self.fields.chunk_index, chunk.chunk_index);
            if let Some(ref s) = chunk.section {
                doc.add_text(self.fields.section, s);
            }
            doc.add_text(self.fields.body, &chunk.body);
            add_opt_text(&mut doc, self.fields.topic, chunk.topic.as_deref());
            add_opt_text(&mut doc, self.fields.title, chunk.title.as_deref());
            add_opt_text(&mut doc, self.fields.author, chunk.author.as_deref());
            add_opt_text(&mut doc, self.fields.lang, chunk.lang.as_deref());
            add_opt_text(
                &mut doc,
                self.fields.created_at,
                chunk.created_at.as_deref(),
            );
            add_opt_text(
                &mut doc,
                self.fields.updated_at,
                chunk.updated_at.as_deref(),
            );
            add_opt_text(&mut doc, self.fields.tags, chunk.tags.as_deref());
            add_opt_text(
                &mut doc,
                self.fields.llm_summary,
                chunk.llm_summary.as_deref(),
            );
            add_opt_text(&mut doc, self.fields.llm_tags, chunk.llm_tags.as_deref());
            if let Some(v) = chunk.llm_quality_score {
                doc.add_f64(self.fields.llm_quality_score, v);
            }
            writer
                .add_document(doc)
                .context("failed to add document to Tantivy index")?;
        }
        Ok(())
    }

    /// Delete all chunks for a given source ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the index writer cannot be initialised.
    ///
    /// # Panics
    ///
    /// Panics if the writer lock is in an unexpected state (should never occur in normal use).
    pub fn delete_chunks_by_source(&self, source_id: &str) -> Result<()> {
        let term = tantivy::Term::from_field_text(self.fields.source_id, source_id);
        let guard = self.ensure_writer()?;
        let writer = guard.as_ref().expect("writer initialized by ensure_writer");
        writer.delete_term(term);
        Ok(())
    }

    /// Number of searchable Tantivy segments.
    pub fn segment_count(&self) -> usize {
        self.index.searchable_segment_ids().map_or(0, |s| s.len())
    }

    /// Compact index segments for read performance.
    ///
    /// Smart merge strategy:
    /// - Finds the largest segment and merges only the smaller ones together,
    ///   avoiding a costly full rewrite when a dominant segment already exists.
    /// - Falls back to full merge when no single segment dominates (e.g. after
    ///   `--recreate` with many similarly-sized segments).
    /// - Uses `NoMergePolicy` to prevent background threads from consuming
    ///   segments during explicit merges.
    /// - When segment count exceeds `MAX_FANIN`, merges in disjoint groups
    ///   first, then merges results.
    ///
    /// # Lock semantics
    ///
    /// The `writer` mutex is acquired and released once **per round** rather
    /// than held across all rounds.  This is intentional: `SegmentMeta`
    /// references must be dropped before calling `writer.merge()` so that
    /// Tantivy's GC can delete the old segment files after each merge commit.
    /// Holding the lock across rounds would not provide additional safety
    /// because each round is self-contained (commit + reader reload + GC).
    ///
    /// # Errors
    ///
    /// Returns an error if listing segments, merging, or committing fails.
    pub fn optimize(&self, on_progress: impl Fn(usize)) -> Result<()> {
        // Phase 1: smart merge -- collapse small segments while skipping the
        // dominant one to avoid a full rewrite when most data is already in a
        // single large segment.
        for _ in 0..MAX_ROUNDS {
            let merge_ids: Vec<_> = {
                let metas = self
                    .index
                    .searchable_segment_metas()
                    .context("failed to list segments")?;
                if metas.len() <= 1 {
                    break;
                }

                let max_docs = metas
                    .iter()
                    .map(tantivy::SegmentMeta::num_docs)
                    .max()
                    .unwrap_or(0);
                let total_docs: u64 = metas.iter().map(|m| u64::from(m.num_docs())).sum();

                if u64::from(max_docs) * 5 > total_docs * 4 {
                    let small: Vec<_> = metas
                        .iter()
                        .filter(|m| m.num_docs() < max_docs)
                        .map(tantivy::SegmentMeta::id)
                        .collect();
                    if small.len() <= 1 {
                        break;
                    }
                    small
                } else {
                    metas.iter().map(tantivy::SegmentMeta::id).collect()
                }
            };

            self.merge_segments(&merge_ids)?;
            on_progress(self.segment_count());
        }

        // Phase 2: unconditional final merge -- the smart path above exits with
        // 2 segments (1 dominant + 1 merged tail) and never converges further.
        // Force a single full merge to guarantee exactly 1 segment.
        {
            let all: Vec<_> = self
                .index
                .searchable_segment_metas()
                .context("failed to list segments")?
                .iter()
                .map(tantivy::SegmentMeta::id)
                .collect();
            if all.len() > 1 {
                self.merge_segments(&all)?;
                on_progress(self.segment_count());
            }
        }

        {
            let mut guard = mutex_lock(&self.writer);
            *guard = None;
        }
        self.commit()
    }

    fn merge_segments(&self, segment_ids: &[tantivy::index::SegmentId]) -> Result<()> {
        let mut guard = self.acquire_writer()?;
        let writer = guard
            .as_mut()
            .expect("writer initialized by acquire_writer");

        writer.set_merge_policy(Box::new(tantivy::merge_policy::NoMergePolicy));

        for group in segment_ids.chunks(MAX_FANIN) {
            if group.len() < 2 {
                continue;
            }
            writer.merge(group).wait().context("segment merge failed")?;
        }

        commit_and_reload(writer, &self.reader)?;
        Ok(())
    }

    fn commit_inner(&self, force: bool) -> Result<()> {
        // Hold the writer mutex across both the Tantivy commit and the sidecar
        // writes to eliminate the crash window where Tantivy is committed but
        // sidecar files are not yet written.
        let mut guard = mutex_lock(&self.writer);
        if let Some(writer) = guard.as_mut() {
            commit_and_reload(writer, &self.reader)?;
            *guard = None;
        } else {
            self.reader.reload().context("tantivy reader reload")?;
        }

        let meta_bytes = {
            let meta = self.read_meta();
            let payload = bitcode::encode(&*meta);
            write_sidecar(META_FORMAT_VERSION, &payload)
        };
        atomic_write(&store_meta_path(&self.path), &meta_bytes)?;

        let docs_need_write = force
            || self.docs_dirty.load(Ordering::Acquire)
            || !store_docs_path(&self.path).exists();
        if docs_need_write && self.documents.get().is_some() {
            let docs_bytes = {
                let guard = self.read_docs();
                let mut docs: Vec<DocMeta> = Vec::with_capacity(guard.len());
                docs.extend(guard.values().cloned());
                drop(guard);
                let payload = bitcode::encode(&docs);
                write_sidecar(DOCS_FORMAT_VERSION, &payload)
            };
            atomic_write(&store_docs_path(&self.path), &docs_bytes)?;
            self.docs_dirty.store(false, Ordering::Release);
        }

        let stamps_need_write = force
            || self.stamps_dirty.load(Ordering::Acquire)
            || !store_stamps_path(&self.path).exists();
        if stamps_need_write && self.stamps.get().is_some() {
            let fm_bytes = {
                let guard = self.read_stamps();
                let mut entries: Vec<(crate::types::SourceId, StampsMeta)> =
                    Vec::with_capacity(guard.len());
                entries.extend(guard.iter().map(|(k, v)| (k.clone(), v.clone())));
                drop(guard);
                let payload = bitcode::encode(&entries);
                write_sidecar(STAMPS_FORMAT_VERSION, &payload)
            };
            atomic_write(&store_stamps_path(&self.path), &fm_bytes)?;
            self.stamps_dirty.store(false, Ordering::Release);
        }

        self.dirty.store(false, Ordering::Release);
        drop(guard);

        Ok(())
    }

    /// Lock the writer mutex and lazily create the `IndexWriter` if not already open.
    fn acquire_writer(&self) -> Result<std::sync::MutexGuard<'_, Option<IndexWriter>>> {
        let mut guard = mutex_lock(&self.writer);
        if guard.is_none() {
            let w = self
                .index
                .writer(self.writer_heap_bytes)
                .context("failed to create index writer")?;
            let mut policy = tantivy::merge_policy::LogMergePolicy::default();
            policy.set_min_num_segments(2);
            policy.set_del_docs_ratio_before_merge(0.1);
            w.set_merge_policy(Box::new(policy));
            *guard = Some(w);
        }
        Ok(guard)
    }

    /// Acquire the writer and mark the store dirty (called before every write operation).
    fn ensure_writer(&self) -> Result<std::sync::MutexGuard<'_, Option<IndexWriter>>> {
        let guard = self.acquire_writer()?;
        self.dirty.store(true, Ordering::Release);
        Ok(guard)
    }
}

fn add_opt_text(doc: &mut TantivyDocument, field: tantivy::schema::Field, val: Option<&str>) {
    if let Some(v) = val.filter(|s| !s.is_empty()) {
        doc.add_text(field, v);
    }
}

/// Commit the Tantivy writer, reload the reader, and run segment GC.
fn commit_and_reload(writer: &mut IndexWriter, reader: &IndexReader) -> Result<()> {
    writer.commit().context("tantivy commit")?;
    reader.reload().context("tantivy reader reload")?;
    if let Err(e) = writer.garbage_collect_files().wait() {
        tracing::debug!("segment GC: {e}");
    }
    Ok(())
}
