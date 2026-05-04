use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use crate::store::documents::resolve_name_from_terms;
use crate::store::types::{avg_words_per_chunk, sort_chunks_natural};
use crate::store::{
    DocDetail, DocFilter, SearchHit, SearchQuery, SourceResolveResult, Store, StoreInfo,
    TopicFilter, TopicResult, TopicSort, TopicStat,
};
use crate::types::{Chunk, DocKind, DocMeta, SourceType};
use crate::util;

/// Federated view over one or more `Store` instances.
///
/// Queries fan out to all stores and results are merged before pagination.
/// Single-store usage delegates directly with no overhead.
pub struct StoreSet {
    stores: Vec<Arc<Store>>,
}

impl StoreSet {
    /// Wrap multiple stores into a federated set.
    pub fn new(stores: Vec<Store>) -> Result<Self> {
        anyhow::ensure!(!stores.is_empty(), "StoreSet requires at least one store");
        Ok(Self {
            stores: stores.into_iter().map(Arc::new).collect(),
        })
    }

    /// Wrap a single store.
    pub fn single(store: Store) -> Self {
        Self::new(vec![store]).expect("single-element vec is always non-empty")
    }

    /// Return the first store unconditionally.
    pub fn first_store(&self) -> &Store {
        &self.stores[0]
    }

    /// Iterate over all stores.
    pub fn iter(&self) -> impl Iterator<Item = &Store> {
        self.stores.iter().map(AsRef::as_ref)
    }

    /// Full-text search across all stores, merged by score and re-paginated.
    pub fn search(
        &self,
        sq: &SearchQuery,
    ) -> std::result::Result<(Vec<SearchHit>, usize), crate::error::LoreError> {
        if self.stores.len() == 1 {
            return self.stores[0].search(sq);
        }

        // Over-fetch from each store so the global merge has enough candidates.
        let fetch_limit = sq.limit + sq.offset;
        let overfetch = SearchQuery {
            limit: fetch_limit,
            offset: 0,
            ..sq.clone()
        };

        let mut all_hits: Vec<SearchHit> = Vec::new();
        let mut total: usize = 0;

        for store in &self.stores {
            let (hits, store_total) = store.search(&overfetch)?;
            total = total.saturating_add(store_total);
            all_hits.extend(hits);
        }

        sq.sort.apply(&mut all_hits, sq.reverse);

        let page = util::paginate(&all_hits, sq.offset, sq.limit).to_vec();
        let re_ranked: Vec<SearchHit> = page
            .into_iter()
            .enumerate()
            .map(|(i, mut h)| {
                h.rank = (sq.offset + i) as u64;
                h
            })
            .collect();

        Ok((re_ranked, total))
    }

    /// List topic statistics across all stores, merging by topic name.
    pub fn list_topic_stats(
        &self,
        filter: &TopicFilter,
        sort: TopicSort,
        reverse: bool,
        limit: usize,
        offset: usize,
    ) -> (Vec<TopicStat>, usize) {
        if self.stores.len() == 1 {
            return self.stores[0].list_topic_stats(filter, sort, reverse, limit, offset);
        }

        let mut merged: HashMap<String, TopicStat> = HashMap::new();
        for store in &self.stores {
            // Fetch all from each store (large limit, no offset).
            let (stats, _) = store.list_topic_stats(filter, sort, false, usize::MAX, 0);
            for stat in stats {
                merge_topic_stat(&mut merged, stat);
            }
        }

        let mut stats: Vec<TopicStat> = merged.into_values().collect();
        sort.apply(&mut stats, reverse);
        let total = stats.len();
        (util::paginate(&stats, offset, limit).to_vec(), total)
    }

    /// List documents across all stores, merged and re-paginated.
    pub fn list_documents(
        &self,
        filter: &DocFilter,
        limit: usize,
        offset: usize,
    ) -> (Vec<DocMeta>, usize) {
        if self.stores.len() == 1 {
            return self.stores[0].list_documents(filter, limit, offset);
        }

        let mut all: Vec<DocMeta> = Vec::new();
        for store in &self.stores {
            let (docs, _) = store.list_documents(filter, usize::MAX, 0);
            all.extend(docs);
        }

        filter.sort.apply(&mut all, filter.reverse);
        let total = all.len();
        (util::paginate(&all, offset, limit).to_vec(), total)
    }

    /// Retrieve topic details, merging chunks from all stores that have the topic.
    pub fn get_topic(&self, name: &str) -> Result<Option<TopicResult>> {
        if self.stores.len() == 1 {
            return self.stores[0].get_topic(name);
        }

        let mut merged_name: Option<String> = None;
        let mut all_chunks: Vec<Chunk> = Vec::new();
        let mut all_sources: Vec<String> = Vec::new();
        let mut total_chunks: u64 = 0;

        for store in &self.stores {
            if let Some(result) = store.get_topic(name)? {
                if merged_name.is_none() {
                    merged_name = Some(result.name);
                }
                total_chunks = total_chunks.saturating_add(result.chunk_count);
                all_chunks.extend(result.chunks);
                all_sources.extend(result.source_paths);
            }
        }

        match merged_name {
            None => Ok(None),
            Some(topic_name) => {
                sort_chunks_natural(&mut all_chunks);
                all_sources.sort();
                all_sources.dedup();
                Ok(Some(TopicResult {
                    name: topic_name,
                    chunk_count: total_chunks,
                    source_paths: all_sources,
                    chunks: all_chunks,
                }))
            }
        }
    }

    /// Resolve a source key across all stores.
    ///
    /// If exactly one store resolves `Exact`, return that.  If multiple stores
    /// resolve `Exact`, they are combined into `Ambiguous`.  All `Ambiguous`
    /// candidates across stores are pooled.
    pub fn resolve_source_key(&self, query: &str) -> SourceResolveResult {
        if self.stores.len() == 1 {
            return self.stores[0].resolve_source_key(query);
        }

        let mut exact_results: Vec<String> = Vec::new();
        let mut ambiguous_candidates: Vec<String> = Vec::new();

        for store in &self.stores {
            match store.resolve_source_key(query) {
                SourceResolveResult::Exact(s) => exact_results.push(s),
                SourceResolveResult::Ambiguous(candidates) => {
                    ambiguous_candidates.extend(candidates);
                }
                SourceResolveResult::NotFound => {}
            }
        }

        if exact_results.len() == 1 && ambiguous_candidates.is_empty() {
            return SourceResolveResult::Exact(exact_results.remove(0));
        }

        let mut all_candidates = exact_results;
        all_candidates.extend(ambiguous_candidates);
        all_candidates.sort();
        all_candidates.dedup();

        match all_candidates.len() {
            0 => SourceResolveResult::NotFound,
            1 => SourceResolveResult::Exact(all_candidates.remove(0)),
            _ => SourceResolveResult::Ambiguous(all_candidates),
        }
    }

    /// Retrieve a document's metadata and chunks, resolving the source key first.
    pub fn get_document(&self, source_id: &str) -> Result<Option<DocDetail>> {
        if self.stores.len() == 1 {
            return self.stores[0].get_document(source_id);
        }

        for store in &self.stores {
            if let Some(doc) = store.get_document(source_id)? {
                return Ok(Some(doc));
            }
        }
        Ok(None)
    }

    /// Aggregate store info across all stores.
    pub fn store_info(&self) -> StoreInfo {
        if self.stores.len() == 1 {
            return self.stores[0].store_info();
        }

        let infos: Vec<StoreInfo> = self.stores.iter().map(|s| s.store_info()).collect();
        merge_store_infos(infos)
    }

    /// Resolve a topic name by pooling terms from all stores.
    pub fn resolve_topic_name(&self, name: &str) -> Option<String> {
        if self.stores.len() == 1 {
            return self.stores[0].resolve_topic_name(name);
        }
        self.resolve_field_across_stores(name, Store::topic_terms, Store::resolve_topic_name)
    }

    /// Resolve an author name by pooling terms from all stores.
    pub fn resolve_author_name(&self, name: &str) -> Option<String> {
        if self.stores.len() == 1 {
            return self.stores[0].resolve_author_name(name);
        }
        self.resolve_field_across_stores(name, Store::author_terms, Store::resolve_author_name)
    }

    fn resolve_field_across_stores(
        &self,
        name: &str,
        terms: fn(&Store) -> Result<Vec<String>>,
        single: fn(&Store, &str) -> Option<String>,
    ) -> Option<String> {
        let mut all_terms: Vec<String> = Vec::new();
        for store in &self.stores {
            match terms(store) {
                Ok(t) => all_terms.extend(t),
                Err(_) => {
                    if let Some(resolved) = single(store, name) {
                        all_terms.push(resolved);
                    }
                }
            }
        }
        resolve_name_from_terms(name, all_terms.iter().map(String::as_str))
    }
}

fn merge_counts<K: Eq + std::hash::Hash>(target: &mut HashMap<K, u64>, source: HashMap<K, u64>) {
    for (k, v) in source {
        *target.entry(k).or_default() += v;
    }
}

/// Accumulate a `TopicStat` into a topic map, merging counts for duplicate names.
fn merge_topic_stat(map: &mut HashMap<String, TopicStat>, stat: TopicStat) {
    let entry = map.entry(stat.name.clone()).or_insert_with(|| TopicStat {
        name: stat.name,
        doc_count: 0,
        chunk_count: 0,
        word_count: 0,
    });
    entry.doc_count = entry.doc_count.saturating_add(stat.doc_count);
    entry.chunk_count = entry.chunk_count.saturating_add(stat.chunk_count);
    entry.word_count = entry.word_count.saturating_add(stat.word_count);
}

/// Merge a list of `StoreInfo` snapshots into a single aggregate.
fn merge_store_infos(infos: Vec<StoreInfo>) -> StoreInfo {
    let mut documents: u64 = 0;
    let mut chunks: u64 = 0;
    let mut words: u64 = 0;
    let mut segments: usize = 0;
    let mut topic_map: HashMap<String, TopicStat> = HashMap::new();
    let mut source_types: HashMap<SourceType, u64> = HashMap::new();
    let mut kinds: HashMap<DocKind, u64> = HashMap::new();
    let mut formats: HashMap<String, u64> = HashMap::new();
    let mut languages: HashMap<String, u64> = HashMap::new();

    for info in infos {
        documents = documents.saturating_add(info.documents);
        chunks = chunks.saturating_add(info.chunks);
        words = words.saturating_add(info.words);
        segments += info.segments;

        for stat in info.topics {
            merge_topic_stat(&mut topic_map, stat);
        }
        merge_counts(&mut source_types, info.source_types);
        merge_counts(&mut kinds, info.kinds);
        merge_counts(&mut formats, info.formats);
        merge_counts(&mut languages, info.languages);
    }

    let avg_words_per_chunk = avg_words_per_chunk(words, chunks);

    let mut topics: Vec<TopicStat> = topic_map.into_values().collect();
    topics.sort_by(|a, b| a.name.cmp(&b.name));

    StoreInfo {
        documents,
        chunks,
        words,
        topics,
        avg_words_per_chunk,
        source_types,
        kinds,
        formats,
        languages,
        // Metadata fields are not meaningful for a federated view.
        name: None,
        description: None,
        created_at: None,
        updated_at: None,
        last_mode: None,
        phrase_search: None,
        writer_heap_mb: None,
        lore_version: None,
        language: None,
        segments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_helpers::{temp_store, test_chunk, test_meta};

    fn two_store_set(
        topic1: &str,
        topic2: &str,
    ) -> (StoreSet, tempfile::TempDir, tempfile::TempDir) {
        let (s1, dir1) = temp_store();
        s1.insert_chunks(&[test_chunk("/a.md", "alpha content", topic1, 0)])
            .unwrap();
        s1.upsert_document(test_meta("/a.md", topic1));
        s1.commit().unwrap();

        let (s2, dir2) = temp_store();
        s2.insert_chunks(&[test_chunk("/b.md", "beta content", topic2, 0)])
            .unwrap();
        s2.upsert_document(test_meta("/b.md", topic2));
        s2.commit().unwrap();

        (StoreSet::new(vec![s1, s2]).unwrap(), dir1, dir2)
    }

    #[test]
    fn get_topic_merges_chunks_from_both_stores() {
        let (set, _d1, _d2) = two_store_set("Wildlife", "Wildlife");
        let result = set
            .get_topic("Wildlife")
            .unwrap()
            .expect("topic should exist");
        assert_eq!(result.chunk_count, 2);
        assert_eq!(result.source_paths.len(), 2);
        assert!(result.source_paths.iter().any(|s| s.contains("/a.md")));
        assert!(result.source_paths.iter().any(|s| s.contains("/b.md")));
    }

    #[test]
    fn get_topic_returns_none_when_missing() {
        let (set, _d1, _d2) = two_store_set("Animals", "Programming");
        assert!(set.get_topic("NoSuchTopic").unwrap().is_none());
    }

    #[test]
    fn resolve_source_key_deduplicates_across_stores() {
        let (s1, d1) = temp_store();
        s1.insert_chunks(&[
            test_chunk("/docs/guide.md", "content a", "t", 0),
            test_chunk("/docs/intro.md", "content b", "t", 0),
        ])
        .unwrap();
        s1.commit().unwrap();

        let (s2, d2) = temp_store();
        s2.insert_chunks(&[
            test_chunk("/docs/guide.md", "content c", "t", 0),
            test_chunk("/docs/readme.md", "content d", "t", 0),
        ])
        .unwrap();
        s2.commit().unwrap();

        let set = StoreSet::new(vec![s1, s2]).unwrap();
        match set.resolve_source_key("docs") {
            SourceResolveResult::Ambiguous(candidates) => {
                let unique: std::collections::HashSet<_> = candidates.iter().collect();
                assert_eq!(
                    unique.len(),
                    candidates.len(),
                    "ambiguous candidates should be deduplicated: {candidates:?}"
                );
            }
            other => panic!("expected Ambiguous for broad match; got: {other:?}"),
        }

        drop((d1, d2));
    }

    #[test]
    fn multi_store_search() {
        let (set, _d1, _d2) = two_store_set("t1", "t2");

        // Full result: both stores contribute, ranks are re-assigned.
        let sq = SearchQuery::new("content");
        let (hits, total) = set.search(&sq).unwrap();
        assert_eq!(total, 2, "both stores should contribute a hit");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].rank, 0);
        assert_eq!(hits[1].rank, 1);

        // Paginated: offset=1 limit=1 still sees total=2, returns the second hit.
        let sq = SearchQuery {
            query: "content".into(),
            limit: 1,
            offset: 1,
            ..SearchQuery::default()
        };
        let (hits, total) = set.search(&sq).unwrap();
        assert_eq!(total, 2);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rank, 1);
    }

    #[test]
    fn multi_store_name_resolution() {
        // Topic resolution: terms from both stores are pooled.
        let (set, _d1, _d2) = two_store_set("Security", "security-audit");
        assert_eq!(
            set.resolve_topic_name("audit"),
            Some("security-audit".to_owned()),
            "should resolve via second store's terms"
        );

        // Author resolution: author terms from both stores are pooled.
        let (s1, d1) = temp_store();
        let mut c1 = test_chunk("/a.md", "content", "t", 0);
        c1.author = Some(Arc::from("Alice"));
        s1.insert_chunks(&[c1]).unwrap();
        s1.commit().unwrap();

        let (s2, d2) = temp_store();
        let mut c2 = test_chunk("/b.md", "content", "t", 0);
        c2.author = Some(Arc::from("Bob Smith"));
        s2.insert_chunks(&[c2]).unwrap();
        s2.commit().unwrap();

        let set = StoreSet::new(vec![s1, s2]).unwrap();
        assert_eq!(
            set.resolve_author_name("smith"),
            Some("Bob Smith".to_owned()),
            "should resolve via second store's author terms"
        );

        drop((d1, d2));
    }
}
