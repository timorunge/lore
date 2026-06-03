use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::{Context, Result};
use tantivy::schema::Field;

use crate::store::Store;
use crate::store::types::{DocDetail, DocFilter};
use crate::types::{Chunk, DocMeta, SourceId, StampsMeta};
use crate::util;

impl Store {
    /// Enumerate all distinct values for a STRING field from the Tantivy term
    /// dictionary. Deduplicates across segments.
    pub(crate) fn enumerate_field_terms(&self, field: Field) -> Result<Vec<String>> {
        let searcher = self.reader.searcher();
        let mut seen = HashSet::<String>::new();
        for segment_reader in searcher.segment_readers() {
            let inv_idx = segment_reader
                .inverted_index(field)
                .context("enumerate_field_terms: inverted_index")?;
            let mut stream = inv_idx
                .terms()
                .stream()
                .context("enumerate_field_terms: term stream")?;
            while let Some((bytes, _term_info)) = stream.next() {
                if let Ok(s) = std::str::from_utf8(bytes)
                    && !s.is_empty()
                    && !seen.contains(s)
                {
                    seen.insert(s.to_owned());
                }
            }
        }
        Ok(seen.into_iter().collect())
    }

    /// Acquires the `documents` write lock to insert/update the metadata entry.
    pub fn upsert_document(&self, meta: DocMeta) {
        let mut docs = self.write_docs();
        self.dirty.store(true, Ordering::Release);
        self.docs_dirty.store(true, Ordering::Release);
        docs.insert(meta.source_id.clone(), meta);
    }

    /// Acquires the `documents` write lock to remove the entry.
    pub fn delete_document(&self, source_id: &str) {
        let mut docs = self.write_docs();
        docs.remove(source_id);
        let mut fm = self.write_stamps();
        fm.remove(source_id);
        self.dirty.store(true, Ordering::Release);
        self.docs_dirty.store(true, Ordering::Release);
        self.stamps_dirty.store(true, Ordering::Release);
    }

    #[cfg(feature = "ingest")]
    pub(crate) fn upsert_stamps(&self, source_id: SourceId, meta: StampsMeta) {
        let mut fm = self.write_stamps();
        self.dirty.store(true, Ordering::Release);
        self.stamps_dirty.store(true, Ordering::Release);
        fm.insert(source_id, meta);
    }

    /// Snapshot of all stamps metadata (clone-on-write).
    pub fn get_all_stamps(&self) -> Arc<HashMap<SourceId, StampsMeta>> {
        Arc::new(self.read_stamps().clone())
    }

    #[cfg(feature = "ingest")]
    pub(crate) fn set_metadata(&self, key: &str, value: &str) {
        let mut meta = self.write_meta();
        meta.insert(key.to_owned(), value.to_owned());
        self.dirty.store(true, Ordering::Release);
    }

    #[cfg(feature = "ingest")]
    pub(crate) fn get_metadata(&self, key: &str) -> Option<String> {
        let meta = self.read_meta();
        meta.get(key).cloned()
    }

    /// Convenience wrapper: delete existing chunks, insert new ones, upsert document meta.
    ///
    /// # Errors
    ///
    /// Returns an error if deleting old chunks or inserting new chunks into the Tantivy index fails.
    pub fn replace_document(&self, source_id: &str, chunks: &[Chunk], meta: DocMeta) -> Result<()> {
        self.delete_chunks_by_source(source_id)?;
        if !chunks.is_empty() {
            self.insert_chunks(chunks)?;
        }
        self.upsert_document(meta);
        Ok(())
    }

    /// Fetch metadata and all chunks for a single document by source_id.
    ///
    /// # Errors
    ///
    /// Returns an error if the Tantivy index query fails.
    pub fn get_document(&self, source_id: &str) -> Result<Option<DocDetail>> {
        let Some(meta) = self.read_docs().get(source_id).cloned() else {
            return Ok(None);
        };

        let chunks = self.chunks_for_sources(&[source_id])?;

        Ok(Some(DocDetail { meta, chunks }))
    }

    /// List documents matching `filter`, sorted by `filter.sort`. Returns `(page, total)`.
    pub(crate) fn list_documents(
        &self,
        filter: &DocFilter,
        limit: usize,
        offset: usize,
    ) -> (Vec<DocMeta>, usize) {
        let lang_lc = filter.lang.as_ref().map(|l| l.to_lowercase());
        let topic_lc = filter.topic.as_ref().map(|t| t.to_lowercase());
        let title_lc = filter.title.as_ref().map(|t| t.to_lowercase());
        let author_lc = filter.author.as_ref().map(|a| a.to_lowercase());
        let format_lc = filter.format.as_ref().map(|f| f.to_lowercase());
        let criteria = DocMatchCriteria {
            lang_lc: lang_lc.as_deref(),
            origin: filter.origin.as_deref(),
            kind: filter.kind.as_deref(),
            source: filter.source.as_deref(),
            format_lc: format_lc.as_deref(),
        };
        let mut results: Vec<DocMeta> = {
            let docs = self.read_docs();
            docs.values()
                .filter(|d| {
                    if !doc_matches(d, &criteria) {
                        return false;
                    }
                    if let Some(ref topic_lc) = topic_lc
                        && d.topic
                            .as_deref()
                            .is_none_or(|t| !crate::util::eq_lowercase(t, topic_lc))
                    {
                        return false;
                    }
                    if let Some(ref title_lc) = title_lc
                        && !d
                            .title
                            .as_deref()
                            .is_some_and(|t| crate::util::contains_lowercase(t, title_lc))
                    {
                        return false;
                    }
                    if let Some(ref author_lc) = author_lc
                        && !d
                            .author
                            .as_deref()
                            .is_some_and(|a| crate::util::contains_lowercase(a, author_lc))
                    {
                        return false;
                    }
                    true
                })
                .cloned()
                .collect()
        };
        filter.sort.apply(&mut results, filter.reverse);
        let total = results.len();
        (util::paginate(&results, offset, limit).to_vec(), total)
    }

    #[cfg(feature = "ingest")]
    pub(crate) fn document_count(&self) -> usize {
        self.read_docs().len()
    }

    /// Snapshot of all document metadata (clone-on-write).
    pub fn get_all_documents(&self) -> Arc<HashMap<SourceId, DocMeta>> {
        Arc::new(self.read_docs().clone())
    }

    pub(crate) fn resolve_topic_name(&self, name: &str) -> Option<String> {
        self.resolve_indexed_field_name(name, self.fields.topic, |d| d.topic.as_deref())
    }

    pub(crate) fn resolve_author_name(&self, name: &str) -> Option<String> {
        self.resolve_indexed_field_name(name, self.fields.author, |d| d.author.as_deref())
    }

    fn resolve_indexed_field_name(
        &self,
        name: &str,
        field: tantivy::schema::Field,
        fallback: impl Fn(&DocMeta) -> Option<&str>,
    ) -> Option<String> {
        match self.enumerate_field_terms(field) {
            Ok(terms) => resolve_name_from_terms(name, terms.iter().map(String::as_str)),
            Err(e) => {
                tracing::warn!("failed to enumerate field terms: {e}");
                self.resolve_field_name(name, fallback)
            }
        }
    }

    /// Enumerate all distinct topic values from the Tantivy term dictionary.
    pub(crate) fn topic_terms(&self) -> Result<Vec<String>> {
        self.enumerate_field_terms(self.fields.topic)
    }

    /// Enumerate all distinct author values from the Tantivy term dictionary.
    pub(crate) fn author_terms(&self) -> Result<Vec<String>> {
        self.enumerate_field_terms(self.fields.author)
    }

    /// Fallback: resolve a field value from lore.docs when the term dictionary
    /// is unavailable.
    fn resolve_field_name(
        &self,
        name: &str,
        field: impl Fn(&DocMeta) -> Option<&str>,
    ) -> Option<String> {
        let docs = self.read_docs();
        let mut seen = HashSet::<&str>::new();
        let unique_values: Vec<&str> = docs
            .values()
            .filter_map(&field)
            .filter(|v| seen.insert(v))
            .collect();
        resolve_name_from_terms(name, unique_values)
    }
}

/// Pre-lowercased filter criteria for document matching.
pub(crate) struct DocMatchCriteria<'a> {
    pub lang_lc: Option<&'a str>,
    pub origin: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub source: Option<&'a str>,
    pub format_lc: Option<&'a str>,
}

/// Test whether a document matches all non-None criteria.
pub(crate) fn doc_matches(doc: &DocMeta, c: &DocMatchCriteria<'_>) -> bool {
    if let Some(lang) = c.lang_lc
        && doc
            .lang
            .as_deref()
            .is_none_or(|l| l.to_lowercase() != *lang)
    {
        return false;
    }
    if let Some(origin) = c.origin
        && doc.origin.as_str() != origin
    {
        return false;
    }
    if let Some(kind) = c.kind
        && doc.kind.as_str() != kind
    {
        return false;
    }
    if let Some(src) = c.source
        && !doc.source.contains(src)
    {
        return false;
    }
    if let Some(fmt) = c.format_lc
        && !format_matches(doc.format.as_deref(), fmt)
    {
        return false;
    }
    true
}

/// Return true if `stored` (an Option<&str>) case-insensitively equals `filter_lc`
/// (already lowercased by the caller).  A `None` stored value never matches.
///
/// Centralises the lowercasing pattern used by `list_documents` and
/// `list_topic_stats` -- format is not normalised at storage time so both sides
/// of the comparison must be lowercased at query time.
#[inline]
pub(crate) fn format_matches(stored: Option<&str>, filter_lc: &str) -> bool {
    stored.is_some_and(|f| crate::util::eq_lowercase(f, filter_lc))
}

/// Fuzzy-match a name against a set of candidate terms.
/// Priority: exact byte-equal > case-insensitive > case-insensitive substring.
/// Within each tier the shortest candidate wins; ties break alphabetically.
pub(crate) fn resolve_name_from_terms<'a>(
    name: &str,
    terms: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let name_lower = name.to_lowercase();
    let mut case_match: Option<&str> = None;
    let mut substr_match: Option<&str> = None;

    let is_better = |current: Option<&str>, candidate: &str| -> bool {
        match current {
            None => true,
            Some(prev) => {
                // len() is O(1) and correct for "shorter wins" on ASCII-dominant field names.
                candidate.len() < prev.len() || (candidate.len() == prev.len() && candidate < prev)
            }
        }
    };

    for value in terms {
        if value == name {
            return Some(value.to_owned());
        }
        if crate::util::eq_lowercase(value, &name_lower) && is_better(case_match, value) {
            case_match = Some(value);
        } else if crate::util::contains_lowercase(value, &name_lower)
            && is_better(substr_match, value)
        {
            substr_match = Some(value);
        }
    }

    case_match
        .or(substr_match)
        .map(std::borrow::ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_helpers::{temp_store, test_meta};

    #[test]
    fn lang_filter_case_insensitive() {
        let (store, _dir) = temp_store();

        let mut meta = test_meta("doc://test/lang", "topic");
        meta.lang = Some("EN".into());
        store.upsert_document(meta);

        let filter = DocFilter {
            lang: Some("en".into()),
            ..DocFilter::default()
        };
        let (results, total) = store.list_documents(&filter, 100, 0);
        assert_eq!(total, 1);
        assert_eq!(results[0].lang.as_deref(), Some("EN"));

        let filter_upper = DocFilter {
            lang: Some("EN".into()),
            ..DocFilter::default()
        };
        let (results2, total2) = store.list_documents(&filter_upper, 100, 0);
        assert_eq!(total2, 1);
        assert_eq!(results2[0].lang.as_deref(), Some("EN"));

        let filter_other = DocFilter {
            lang: Some("fr".into()),
            ..DocFilter::default()
        };
        let (_, total3) = store.list_documents(&filter_other, 100, 0);
        assert_eq!(total3, 0);
    }

    #[test]
    fn list_documents_cases() {
        let (store, _dir) = temp_store();

        let mut m1 = test_meta("/docs/guide.md", "Architecture");
        m1.origin = crate::types::SourceType::Local;
        m1.kind = crate::types::DocKind::Document;
        m1.format = Some("markdown".into());
        store.upsert_document(m1);

        let mut m2 = test_meta("https://example.com/api", "API");
        m2.origin = crate::types::SourceType::Url;
        m2.kind = crate::types::DocKind::Data;
        m2.format = Some("html".into());
        store.upsert_document(m2);

        store.upsert_document(test_meta("/z.md", "t"));
        store.upsert_document(test_meta("/a.md", "t"));
        store.upsert_document(test_meta("/m.md", "t"));

        // filtering
        let (_, total) = store.list_documents(
            &DocFilter {
                source: Some("guide".into()),
                ..DocFilter::default()
            },
            100,
            0,
        );
        assert_eq!(total, 1, "source substring filter");

        let (_, total) = store.list_documents(
            &DocFilter {
                topic: Some("API".into()),
                ..DocFilter::default()
            },
            100,
            0,
        );
        assert_eq!(total, 1, "topic filter");

        let (_, total) = store.list_documents(
            &DocFilter {
                origin: Some("url".into()),
                ..DocFilter::default()
            },
            100,
            0,
        );
        assert_eq!(total, 1, "origin filter");

        let (_, total) = store.list_documents(
            &DocFilter {
                kind: Some("data".into()),
                ..DocFilter::default()
            },
            100,
            0,
        );
        assert_eq!(total, 1, "kind filter");

        let (_, total) = store.list_documents(
            &DocFilter {
                format: Some("markdown".into()),
                ..DocFilter::default()
            },
            100,
            0,
        );
        assert_eq!(total, 1, "format filter");

        let (_, total) = store.list_documents(
            &DocFilter {
                source: Some("nonexistent".into()),
                ..DocFilter::default()
            },
            100,
            0,
        );
        assert_eq!(total, 0, "no match");

        // sort and reverse
        let (asc, _) = store.list_documents(
            &DocFilter {
                source: Some(".md".into()),
                ..DocFilter::default()
            },
            100,
            0,
        );
        assert_eq!(
            asc[0].source, "/a.md",
            "default sort should be by source ascending"
        );

        let (desc, _) = store.list_documents(
            &DocFilter {
                source: Some(".md".into()),
                reverse: true,
                ..DocFilter::default()
            },
            100,
            0,
        );
        assert_eq!(desc[0].source, "/z.md", "reverse should put /z.md first");
    }
}
