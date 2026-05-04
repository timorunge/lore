use std::collections::HashMap;

use anyhow::{Context, Result};
use tantivy::collector::DocSetCollector;
use tantivy::query::TermQuery;
use tantivy::schema::IndexRecordOption;

use crate::store::Store;
use crate::store::documents::{DocMatchCriteria, doc_matches};
use crate::store::schema::get_text_opt;
use crate::store::types::{
    TopicFilter, TopicResult, TopicSort, TopicStat, accumulate_topic, sort_chunks_natural,
    topic_agg_to_stats,
};
use crate::util;

impl Store {
    /// Resolve a topic by name and return its metadata, sources, and chunks.
    /// Uses the Tantivy index directly -- no lore.docs access.
    pub(crate) fn get_topic(&self, name: &str) -> Result<Option<TopicResult>> {
        let Some(topic_name) = self.resolve_topic_name(name) else {
            return Ok(None);
        };

        let searcher = self.reader.searcher();
        let term = tantivy::Term::from_field_text(self.fields.topic, &topic_name);
        let query = TermQuery::new(term, IndexRecordOption::Basic);
        let doc_addrs = searcher
            .search(&query, &DocSetCollector)
            .context("get_topic: topic query")?;

        if doc_addrs.is_empty() {
            tracing::warn!(topic = topic_name, "topic has no indexed chunks");
            return Ok(None);
        }

        let mut paths_set = std::collections::HashSet::new();
        let mut chunks = Vec::with_capacity(doc_addrs.len());
        for addr in doc_addrs {
            let doc: tantivy::TantivyDocument = searcher
                .doc(addr)
                .context("get_topic: failed to read doc")?;
            if let Some(source) = get_text_opt(&doc, self.fields.source) {
                paths_set.insert(source);
            }
            chunks.push(self.chunk_from_doc(&doc));
        }

        sort_chunks_natural(&mut chunks);

        let mut paths: Vec<String> = paths_set.into_iter().collect();
        paths.sort();

        Ok(Some(TopicResult {
            name: topic_name,
            chunk_count: chunks.len() as u64,
            source_paths: paths,
            chunks,
        }))
    }

    /// List topic statistics with configurable sort order.
    ///
    /// Returns `(page, total)` where `page` is the requested slice and
    /// `total` is the count of all topics (for pagination).
    /// The full set must be collected and sorted to support stable pagination,
    /// but only the requested page is returned to the caller.
    pub(crate) fn list_topic_stats(
        &self,
        filter: &TopicFilter,
        sort: TopicSort,
        reverse: bool,
        limit: usize,
        offset: usize,
    ) -> (Vec<TopicStat>, usize) {
        let lang_lc = filter.lang.as_ref().map(|l| l.to_lowercase());
        let format_lc = filter.format.as_ref().map(|f| f.to_lowercase());
        let topic_lc = filter.topic.as_ref().map(|t| t.to_lowercase());
        let author_lc = filter.author.as_ref().map(|a| a.to_lowercase());
        let criteria = DocMatchCriteria {
            lang_lc: lang_lc.as_deref(),
            origin: filter.origin.as_deref(),
            kind: filter.kind.as_deref(),
            source: filter.source.as_deref(),
            format_lc: format_lc.as_deref(),
        };
        let docs = self.read_docs();

        let mut topic_agg: HashMap<&str, (u64, u64, u64)> = HashMap::new();
        for doc in docs.values() {
            if !doc_matches(doc, &criteria) {
                continue;
            }
            if let Some(ref author) = author_lc
                && !doc
                    .author
                    .as_deref()
                    .is_some_and(|a| crate::util::eq_lowercase(a, author))
            {
                continue;
            }
            if let Some(ref topic_q) = topic_lc
                && doc
                    .topic
                    .as_deref()
                    .is_none_or(|t| !crate::util::contains_lowercase(t, topic_q))
            {
                continue;
            }
            accumulate_topic(&mut topic_agg, doc);
        }

        let mut stats: Vec<TopicStat> = topic_agg_to_stats(topic_agg);
        // Release read lock before O(n log n) sort.
        drop(docs);
        sort.apply(&mut stats, reverse);
        let total = stats.len();
        (util::paginate(&stats, offset, limit).to_vec(), total)
    }
}

#[cfg(test)]
mod tests {
    use crate::store::test_helpers::{temp_store, test_meta};

    #[test]
    fn topic_orphan_not_indexed() {
        let (store, _dir) = temp_store();
        let mut meta = test_meta("/doc.md", "orphan-topic");
        meta.chunk_count = 1;
        store.upsert_document(meta);
        // Simulates a doc registered but not yet indexed.
        let result = store.get_topic("orphan-topic").unwrap();
        assert!(
            result.is_none(),
            "expected None when topic has docs but no indexed chunks"
        );
    }
}
