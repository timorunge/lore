use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use tantivy::TantivyDocument;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, TermQuery};
use tantivy::schema::{IndexRecordOption, Value};
use tantivy::snippet::SnippetGenerator;

use crate::error::LoreError;
use crate::store::schema::{fields, get_f64_opt, get_i64, get_text, get_text_opt, sanitize_query};
use crate::store::types::{
    FieldHighlights, MAX_QUERY_BYTES, MAX_SEARCH_LIMIT, SearchHit, SearchQuery, SearchSort,
    SnippetData, sort_chunks_natural,
};
use crate::store::{DiversityCollector, Store};
use crate::types::{Chunk, DocKind, SourceType};
use crate::util;

impl Store {
    /// Execute a full-text search. Returns `(page, total)`.
    ///
    /// `total` is the exact Tantivy match count for the query+filters (excluding
    /// in-memory-only filters like source substring and `max_per_source`).
    pub fn search(
        &self,
        query: &SearchQuery,
    ) -> std::result::Result<(Vec<SearchHit>, usize), LoreError> {
        let searcher = self.reader.searcher();

        let truncated_query = util::truncate_str_ref(&query.query, MAX_QUERY_BYTES);
        if truncated_query.trim().is_empty() {
            return Err(LoreError::EmptyQuery);
        }
        let Ok(text_query) = self
            .search_parser
            .parse_query(&sanitize_query(truncated_query))
        else {
            return Err(LoreError::InvalidQuery);
        };

        let mut must_clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> =
            vec![(Occur::Must, text_query)];

        push_term_filter(&mut must_clauses, self.fields.topic, query.topic.as_deref());
        push_term_filter(
            &mut must_clauses,
            self.fields.author,
            query.author.as_deref(),
        );
        push_term_filter(&mut must_clauses, self.fields.lang, query.lang.as_deref());
        push_term_filter(
            &mut must_clauses,
            self.fields.origin,
            query.origin.as_deref(),
        );
        push_term_filter(&mut must_clauses, self.fields.kind, query.kind.as_deref());
        push_term_filter(
            &mut must_clauses,
            self.fields.format,
            query.format.as_deref(),
        );

        let combined = BooleanQuery::new(must_clauses);

        let limit = query.limit.min(MAX_SEARCH_LIMIT);
        if limit == 0 {
            return Ok((Vec::new(), 0));
        }

        let snippet_gen = SnippetGenerator::create(&searcher, &combined, self.fields.body)
            .ok()
            .map(|mut sg| {
                sg.set_max_num_chars(300);
                sg
            });
        let title_gen = SnippetGenerator::create(&searcher, &combined, self.fields.title).ok();
        let section_gen = SnippetGenerator::create(&searcher, &combined, self.fields.section).ok();

        let needs_post_filter = query.max_per_source.is_some()
            || query.source.is_some()
            || !matches!(query.sort, SearchSort::Score)
            || query.reverse;
        if needs_post_filter {
            let score_sorted = matches!(query.sort, SearchSort::Score) && !query.reverse;

            let mut results = if let Some(max) = query.max_per_source {
                // DiversityCollector: single Tantivy pass with per-source caps.
                // The limit is applied in merge_fruits after score sort +
                // re-diversification. Use the full pool when post-collector
                // filtering (source substring, non-score sort, reverse) may
                // further reduce the result set.
                let collect_limit = if score_sorted && query.source.is_none() {
                    limit.saturating_add(query.offset)
                } else {
                    MAX_SEARCH_LIMIT
                };
                let result =
                    searcher.search(&combined, &DiversityCollector::new(max, collect_limit))?;
                let hits = if let Some(ref src) = query.source {
                    Self::filter_hits_by_source(&searcher, result, src)?
                } else {
                    result
                };
                self.collect_hits(
                    &searcher,
                    hits,
                    0,
                    snippet_gen.as_ref(),
                    title_gen.as_ref(),
                    section_gen.as_ref(),
                )?
            } else {
                let top_docs = searcher.search(
                    &combined,
                    &TopDocs::with_limit(MAX_SEARCH_LIMIT).order_by_score(),
                )?;
                let hits = if let Some(ref src) = query.source {
                    Self::filter_hits_by_source(&searcher, top_docs, src)?
                } else {
                    top_docs
                };
                self.collect_hits(
                    &searcher,
                    hits,
                    0,
                    snippet_gen.as_ref(),
                    title_gen.as_ref(),
                    section_gen.as_ref(),
                )?
            };
            if !matches!(query.sort, SearchSort::Score) {
                query.sort.apply(&mut results, query.reverse);
            } else if query.reverse {
                results.reverse();
            }

            let total = searcher.search(&combined, &tantivy::collector::Count)?;
            let mut page: Vec<SearchHit> = util::paginate(&results, query.offset, limit).to_vec();

            for (i, r) in page.iter_mut().enumerate() {
                r.rank = (query.offset + i) as u64;
            }

            Ok((page, total))
        } else {
            // No post-filtering needed; Tantivy handles offset and count directly.
            let (total, top_docs) = searcher.search(
                &combined,
                &(
                    tantivy::collector::Count,
                    TopDocs::with_limit(limit)
                        .and_offset(query.offset)
                        .order_by_score(),
                ),
            )?;

            let results = self.collect_hits(
                &searcher,
                top_docs,
                query.offset,
                snippet_gen.as_ref(),
                title_gen.as_ref(),
                section_gen.as_ref(),
            )?;

            Ok((results, total))
        }
    }

    /// Reconstruct a `Chunk` from a Tantivy document.
    pub fn chunk_from_doc(&self, doc: &TantivyDocument) -> Chunk {
        let source_str = get_text(doc, self.fields.source);
        Chunk {
            chunk_id: get_text(doc, self.fields.chunk_id),
            source_id: crate::types::source_id(&source_str),
            source: Arc::from(source_str),
            origin: doc
                .get_first(self.fields.origin)
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<SourceType>().ok())
                .unwrap_or_default(),
            kind: doc
                .get_first(self.fields.kind)
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<DocKind>().ok())
                .unwrap_or_default(),
            body: get_text(doc, self.fields.body),
            chunk_index: get_i64(doc, self.fields.chunk_index),
            section: get_text_opt(doc, self.fields.section).map(Arc::from),
            format: get_text_opt(doc, self.fields.format).map(Arc::from),
            topic: get_text_opt(doc, self.fields.topic).map(Arc::from),
            author: get_text_opt(doc, self.fields.author).map(Arc::from),
            lang: get_text_opt(doc, self.fields.lang).map(Arc::from),
            tags: get_text_opt(doc, self.fields.tags).map(Arc::from),
            title: get_text_opt(doc, self.fields.title).map(Arc::from),
            created_at: get_text_opt(doc, self.fields.created_at).map(Arc::from),
            updated_at: get_text_opt(doc, self.fields.updated_at),
            llm_summary: get_text_opt(doc, self.fields.llm_summary),
            llm_tags: get_text_opt(doc, self.fields.llm_tags),
            llm_quality_score: get_f64_opt(doc, self.fields.llm_quality_score),
        }
    }

    /// Build a BooleanQuery that matches any of the given source strings.
    pub fn source_term_query(&self, sources: &[impl AsRef<str>]) -> BooleanQuery {
        assert!(
            !sources.is_empty(),
            "source_term_query called with empty sources"
        );
        let clauses = sources
            .iter()
            .map(|s| {
                let term = tantivy::Term::from_field_text(self.fields.source_id, s.as_ref());
                (
                    Occur::Should,
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic))
                        as Box<dyn tantivy::query::Query>,
                )
            })
            .collect();
        BooleanQuery::new(clauses)
    }

    /// Fetch all chunks for a set of source paths. Returns chunks sorted by
    /// source then position.
    pub fn chunks_for_sources(&self, sources: &[impl AsRef<str>]) -> Result<Vec<Chunk>> {
        if sources.is_empty() {
            return Ok(Vec::new());
        }

        let searcher = self.reader.searcher();
        let mut all_chunks = Vec::new();

        let query = self.source_term_query(sources);
        let doc_addrs = searcher
            .search(&query, &tantivy::collector::DocSetCollector)
            .context("chunks_for_sources search")?;

        for addr in doc_addrs {
            let doc: TantivyDocument = searcher.doc(addr).context("failed to read doc")?;
            all_chunks.push(self.chunk_from_doc(&doc));
        }

        sort_chunks_natural(&mut all_chunks);

        Ok(all_chunks)
    }

    /// Collect scored Tantivy doc addresses into `SearchHit`s, assigning ranks starting at `rank_offset`.
    fn collect_hits(
        &self,
        searcher: &tantivy::Searcher,
        top_docs: Vec<(f32, tantivy::DocAddress)>,
        rank_offset: usize,
        snippet_gen: Option<&SnippetGenerator>,
        title_gen: Option<&SnippetGenerator>,
        section_gen: Option<&SnippetGenerator>,
    ) -> Result<Vec<SearchHit>> {
        let docs = self.read_docs();
        let mut results = Vec::with_capacity(top_docs.len());
        for (rank, (score, doc_address)) in top_docs.into_iter().enumerate() {
            let doc: TantivyDocument = searcher.doc(doc_address).context("failed to read doc")?;
            let chunk = self.chunk_from_doc(&doc);
            let snippet = snippet_gen.and_then(|sg| {
                let s = sg.snippet(&chunk.body);
                if s.fragment().is_empty() || s.highlighted().is_empty() {
                    None
                } else {
                    Some(SnippetData {
                        text: s.fragment().to_owned(),
                        highlights: s.highlighted().iter().map(|r| (r.start, r.end)).collect(),
                    })
                }
            });
            let field_highlights = highlight_fields(&chunk, title_gen, section_gen);
            let chunk_total = docs.get(&chunk.source_id).map(|d| d.chunk_count);
            results.push(SearchHit {
                rank: (rank_offset + rank) as u64,
                score: Some(f64::from(score)),
                snippet,
                field_highlights,
                chunk_total,
                chunk,
            });
        }
        Ok(results)
    }

    /// Filter scored doc addresses by source substring using the FAST column.
    /// Avoids stored-doc deserialization for non-matching sources.
    fn filter_hits_by_source(
        searcher: &tantivy::Searcher,
        hits: Vec<(f32, tantivy::DocAddress)>,
        source_substr: &str,
    ) -> Result<Vec<(f32, tantivy::DocAddress)>> {
        // Group by segment to open each FAST column once.
        let mut by_segment: HashMap<u32, Vec<usize>> = HashMap::with_capacity(8);
        for (i, (_score, addr)) in hits.iter().enumerate() {
            by_segment.entry(addr.segment_ord).or_default().push(i);
        }

        let mut keep = vec![false; hits.len()];
        let mut buf = String::new();
        for (seg_ord, indices) in &by_segment {
            let seg = searcher.segment_reader(*seg_ord);
            let col = seg
                .fast_fields()
                .str(fields::SOURCE)
                .context("filter_hits_by_source: FAST source column")?;
            let Some(col) = col else {
                continue;
            };
            // Cache ordinal->match result to avoid repeated string decoding.
            let mut ord_match: HashMap<u64, bool> = HashMap::with_capacity(indices.len());
            for &idx in indices {
                let doc_id = hits[idx].1.doc_id;
                let ord = col.term_ords(doc_id).next().unwrap_or(u64::MAX);
                let matches = *ord_match.entry(ord).or_insert_with(|| {
                    buf.clear();
                    col.ord_to_str(ord, &mut buf).is_ok() && buf.contains(source_substr)
                });
                keep[idx] = matches;
            }
        }

        Ok(hits
            .into_iter()
            .zip(keep)
            .filter_map(|(hit, k)| k.then_some(hit))
            .collect())
    }
}

/// Extract highlight ranges for title and section fields.
fn highlight_fields(
    chunk: &Chunk,
    title_gen: Option<&SnippetGenerator>,
    section_gen: Option<&SnippetGenerator>,
) -> Option<FieldHighlights> {
    let title_hl = title_gen
        .zip(chunk.title.as_deref())
        .map(|(sg, text)| {
            let s = sg.snippet(text);
            s.highlighted()
                .iter()
                .map(|r| (r.start, r.end))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let section_hl = section_gen
        .zip(chunk.section.as_deref())
        .map(|(sg, text)| {
            let s = sg.snippet(text);
            s.highlighted()
                .iter()
                .map(|r| (r.start, r.end))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if title_hl.is_empty() && section_hl.is_empty() {
        None
    } else {
        Some(FieldHighlights {
            title: title_hl,
            section: section_hl,
        })
    }
}

/// Append a Must `TermQuery` for `value` (if `Some`) to `clauses`.
fn push_term_filter(
    clauses: &mut Vec<(Occur, Box<dyn tantivy::query::Query>)>,
    field: tantivy::schema::Field,
    value: Option<&str>,
) {
    if let Some(v) = value {
        let term = tantivy::Term::from_field_text(field, v);
        clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
        ));
    }
}
