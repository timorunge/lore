use std::collections::HashMap;

use crate::store::Store;
use crate::store::meta_key;
use crate::store::types::{
    StoreInfo, TopicStat, accumulate_topic, avg_words_per_chunk, topic_agg_to_stats,
};
use crate::types::{DocKind, SourceType};

struct RawStats {
    doc_count: u64,
    chunk_count: u64,
    word_count: u64,
    languages: HashMap<String, u64>,
    source_types: HashMap<SourceType, u64>,
    kinds: HashMap<DocKind, u64>,
    formats: HashMap<String, u64>,
    topics: Vec<TopicStat>,
}

impl Store {
    /// Compute live store statistics from the document metadata map.
    ///
    /// Acquires the `documents` read lock to snapshot the map, then releases
    /// it before the O(n log n) topic sort so the lock is held for the minimum
    /// duration.  The `metadata` read lock is acquired separately at the end
    /// to read store-level key/value pairs.
    pub(crate) fn store_info(&self) -> StoreInfo {
        let mut raw = {
            let docs = self.read_docs();
            let mut chunk_count: u64 = 0;
            let mut word_count: u64 = 0;
            let mut languages: HashMap<&str, u64> = HashMap::new();
            let mut source_types: HashMap<SourceType, u64> = HashMap::new();
            let mut kinds: HashMap<DocKind, u64> = HashMap::new();
            let mut formats: HashMap<&str, u64> = HashMap::new();
            let mut topic_agg: HashMap<&str, (u64, u64, u64)> = HashMap::new();
            for doc in docs.values() {
                chunk_count = chunk_count.saturating_add(doc.chunk_count);
                word_count = word_count.saturating_add(doc.word_count);
                *source_types.entry(doc.origin).or_default() += 1;
                *kinds.entry(doc.kind).or_default() += 1;
                if let Some(ref fmt) = doc.format {
                    *formats.entry(fmt.as_str()).or_default() += 1;
                }
                if let Some(ref lang) = doc.lang {
                    *languages.entry(lang.as_str()).or_default() += 1;
                }
                accumulate_topic(&mut topic_agg, doc);
            }
            RawStats {
                doc_count: docs.len() as u64,
                chunk_count,
                word_count,
                languages: languages
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v))
                    .collect(),
                source_types,
                kinds,
                formats: formats
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v))
                    .collect(),
                topics: topic_agg_to_stats(topic_agg),
            }
        };
        raw.topics.sort_by(|a, b| a.name.cmp(&b.name));

        let avg_words_per_chunk = avg_words_per_chunk(raw.word_count, raw.chunk_count);
        let meta = self.read_meta();
        StoreInfo {
            documents: raw.doc_count,
            chunks: raw.chunk_count,
            words: raw.word_count,
            topics: raw.topics,
            avg_words_per_chunk,
            source_types: raw.source_types,
            kinds: raw.kinds,
            formats: raw.formats,
            languages: raw.languages,
            name: meta.get(meta_key::NAME).cloned(),
            description: meta.get(meta_key::DESCRIPTION).cloned(),
            created_at: meta.get(meta_key::CREATED_AT).cloned(),
            updated_at: meta.get(meta_key::UPDATED_AT).cloned(),
            last_mode: meta.get(meta_key::MODE).cloned(),
            phrase_search: meta.get(meta_key::PHRASE_SEARCH).cloned(),
            writer_heap_mb: meta.get(meta_key::WRITER_HEAP_MB).cloned(),
            lore_version: meta.get(meta_key::LORE_VERSION).cloned(),
            language: meta.get(meta_key::LANGUAGE).cloned(),
            segments: self.segment_count(),
        }
    }
}
