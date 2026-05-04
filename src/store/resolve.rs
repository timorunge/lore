use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use tantivy::collector::DocSetCollector;

use crate::store::Store;
use crate::store::schema::fields;
use crate::store::types::SourceResolveResult;
use crate::types::SourceId;

impl Store {
    /// Collect the distinct `source_id` hashes present in the Tantivy index.
    ///
    /// Iterates each segment's FAST `source` StrColumn over live docs,
    /// collecting unique ordinals per segment and computing source_id from
    /// each unique source path. Skips soft-deleted docs via `doc_ids_alive()`.
    pub fn index_source_keys(&self) -> Result<HashSet<SourceId>> {
        let searcher = self.reader.searcher();
        let mut keys = HashSet::<SourceId>::new();
        for segment_reader in searcher.segment_readers() {
            let source_col = segment_reader
                .fast_fields()
                .str(fields::SOURCE)
                .context("index_source_keys: FAST source column")?;
            let Some(col) = source_col else {
                continue;
            };
            let mut seen_ords = HashSet::<u64>::new();
            for doc_id in segment_reader.doc_ids_alive() {
                if let Some(ord) = col.term_ords(doc_id).next() {
                    seen_ords.insert(ord);
                }
            }
            let mut buf = String::new();
            for ord in seen_ords {
                buf.clear();
                if col.ord_to_str(ord, &mut buf).is_ok() && !buf.is_empty() {
                    keys.insert(crate::types::source_id(&buf));
                }
            }
        }
        Ok(keys)
    }

    /// Count chunks in the Tantivy index for the given `source_id` hashes.
    ///
    /// Runs a targeted `BooleanQuery` to find matching doc addresses, then
    /// reads `source` from the FAST StrColumn and computes `source_id`.
    pub fn count_chunks_for_sources(&self, sources: &[&str]) -> Result<HashMap<SourceId, u64>> {
        if sources.is_empty() {
            return Ok(HashMap::new());
        }
        let searcher = self.reader.searcher();
        let query = self.source_term_query(sources);
        let doc_addrs = searcher
            .search(&query, &DocSetCollector)
            .context("count_chunks_for_sources query")?;

        // Group doc addresses by segment to batch FAST column lookups.
        let mut by_segment: HashMap<u32, Vec<u32>> = HashMap::new();
        for addr in &doc_addrs {
            by_segment
                .entry(addr.segment_ord)
                .or_default()
                .push(addr.doc_id);
        }

        let mut counts: HashMap<SourceId, u64> = HashMap::new();
        let mut buf = String::new();
        for (seg_ord, doc_ids) in &by_segment {
            let seg_reader = searcher.segment_reader(*seg_ord);
            let source_col = seg_reader
                .fast_fields()
                .str(fields::SOURCE)
                .context("count_chunks_for_sources: FAST source column")?;
            let Some(col) = source_col else {
                continue;
            };
            let mut ord_cache: HashMap<u64, SourceId> = HashMap::new();
            for &doc_id in doc_ids {
                let ord = col.term_ords(doc_id).next().unwrap_or(u64::MAX);
                let sid = ord_cache.entry(ord).or_insert_with(|| {
                    buf.clear();
                    if col.ord_to_str(ord, &mut buf).is_ok() {
                        crate::types::source_id(&buf)
                    } else {
                        SourceId::from("")
                    }
                });
                *counts.entry(sid.clone()).or_default() += 1;
            }
        }
        Ok(counts)
    }

    /// Resolve a user-supplied path/basename query against document `source` values.
    /// Returns the matching `source_id` (hash) for identity lookups.
    ///
    /// Uses the Tantivy term dictionary for the `source` field to enumerate all
    /// distinct source paths without loading lore.docs. Falls back to lore.docs
    /// on index errors.
    pub(crate) fn resolve_source_key(&self, query: &str) -> SourceResolveResult {
        match self.resolve_source_key_from_index(query) {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!("failed to resolve source from index: {e}");
                self.resolve_source_key_from_docs(query)
            }
        }
    }

    /// Resolve a source query against the Tantivy term dictionary (no lore.docs read).
    fn resolve_source_key_from_index(&self, query: &str) -> Result<SourceResolveResult> {
        let sources = self.enumerate_field_terms(self.fields.source)?;
        let result = resolve_source_match(sources.iter().map(String::as_str), query);
        match result {
            SourceResolveResult::Exact(source) => {
                let sid = crate::types::source_id(&source);
                Ok(SourceResolveResult::Exact(String::from(sid.as_str())))
            }
            other => Ok(other),
        }
    }

    /// Fallback: resolve a source query by scanning the lore.docs sidecar.
    fn resolve_source_key_from_docs(&self, query: &str) -> SourceResolveResult {
        let docs = self.read_docs();
        let source_to_id: HashMap<&str, &SourceId> = docs
            .values()
            .map(|d| (d.source.as_str(), &d.source_id))
            .collect();
        match resolve_source_match(source_to_id.keys().copied(), query) {
            SourceResolveResult::Exact(source) => {
                let id = source_to_id[source.as_str()];
                SourceResolveResult::Exact(String::from(id.as_str()))
            }
            other => other,
        }
    }
}

/// Pure matching logic for resolving a partial source path against a set of keys.
pub(super) fn resolve_source_match<'a, I>(sources: I, query: &str) -> SourceResolveResult
where
    I: IntoIterator<Item = &'a str>,
{
    // Reject traversal attempts before any matching.
    if query.contains("..") {
        tracing::warn!("resolve_source_match: rejecting query containing '..': {query:?}");
        return SourceResolveResult::NotFound;
    }
    if query.contains('\0') {
        tracing::warn!("resolve_source_match: rejecting query containing null byte");
        return SourceResolveResult::NotFound;
    }

    let query_lower = query.to_ascii_lowercase();
    // Strip a leading '/' so that "/foo/bar" behaves identically to "foo/bar"
    // when building the path-component boundary suffix pattern.
    let query_lower_stripped = query_lower.trim_start_matches('/');
    let sep_query = format!("/{query_lower_stripped}");
    let query_base = Path::new(query)
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let query_stem = Path::new(query)
        .file_stem()
        .and_then(|n| n.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    let mut exact_match: Option<&str> = None;
    let mut suffix_matches: Vec<&str> = Vec::new();
    let mut contains_matches: Vec<&str> = Vec::new();
    let mut basename_matches: Vec<&str> = Vec::new();
    let mut stem_matches: Vec<&str> = Vec::new();

    for source in sources {
        if source == query {
            return SourceResolveResult::Exact(query.to_owned());
        }
        let source_lower = source.to_ascii_lowercase();
        if source_lower == query_lower {
            exact_match = Some(source);
        } else if source_lower.ends_with(&sep_query) {
            suffix_matches.push(source);
        } else if !query_lower_stripped.is_empty() && source_lower.contains(query_lower_stripped) {
            contains_matches.push(source);
        } else {
            let entry_path = Path::new(source);
            let entry_base = entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            if !query_base.is_empty() && entry_base == query_base {
                basename_matches.push(source);
            } else if !query_stem.is_empty() {
                let entry_stem = entry_path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .map(str::to_ascii_lowercase)
                    .unwrap_or_default();
                if entry_stem == query_stem {
                    stem_matches.push(source);
                }
            }
        }
    }

    if let Some(source) = exact_match {
        return SourceResolveResult::Exact(source.to_owned());
    }

    let candidates = if !suffix_matches.is_empty() {
        suffix_matches
    } else if !contains_matches.is_empty() {
        contains_matches
    } else if !basename_matches.is_empty() {
        basename_matches
    } else {
        stem_matches
    };

    match candidates.len() {
        0 => SourceResolveResult::NotFound,
        1 => SourceResolveResult::Exact(candidates[0].to_owned()),
        _ => SourceResolveResult::Ambiguous(
            candidates
                .into_iter()
                .map(std::borrow::ToOwned::to_owned)
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_SOURCES: &[&str] = &[
        "docs/api/reference.md",
        "docs/guide/intro.md",
        "src/main.rs",
        "src/lib.rs",
    ];

    #[test]
    fn resolve_source_cases() {
        struct Case {
            desc: &'static str,
            sources: &'static [&'static str],
            query: &'static str,
            expected: SourceResolveResult,
        }

        let cases = vec![
            Case {
                desc: "exact path match",
                sources: DEFAULT_SOURCES,
                query: "src/main.rs",
                expected: SourceResolveResult::Exact("src/main.rs".into()),
            },
            Case {
                desc: "leading slash normalised to same result as without slash",
                sources: DEFAULT_SOURCES,
                query: "/api/reference.md",
                expected: SourceResolveResult::Exact("docs/api/reference.md".into()),
            },
            Case {
                desc: "suffix match resolves to full path",
                sources: DEFAULT_SOURCES,
                query: "api/reference.md",
                expected: SourceResolveResult::Exact("docs/api/reference.md".into()),
            },
            Case {
                desc: "leading-slash basename match",
                sources: DEFAULT_SOURCES,
                query: "/reference.md",
                expected: SourceResolveResult::Exact("docs/api/reference.md".into()),
            },
            Case {
                desc: "stem match resolves to full path",
                sources: DEFAULT_SOURCES,
                query: "intro",
                expected: SourceResolveResult::Exact("docs/guide/intro.md".into()),
            },
            Case {
                desc: "basename preferred over stem with .bak sibling",
                sources: &["docs/api/reference.md", "docs/api/reference.md.bak"],
                query: "reference.md",
                expected: SourceResolveResult::Exact("docs/api/reference.md".into()),
            },
            Case {
                desc: "URL substring match (case-sensitive fragment)",
                sources: &[
                    "https://www.youtube.com/watch?v=BrT3AO8bVQY",
                    "docs/guide/intro.md",
                ],
                query: "BrT3AO8bVQY",
                expected: SourceResolveResult::Exact(
                    "https://www.youtube.com/watch?v=BrT3AO8bVQY".into(),
                ),
            },
            Case {
                desc: "URL substring match (lowercased query)",
                sources: &[
                    "https://www.youtube.com/watch?v=BrT3AO8bVQY",
                    "docs/guide/intro.md",
                ],
                query: "brt3ao8bvqy",
                expected: SourceResolveResult::Exact(
                    "https://www.youtube.com/watch?v=BrT3AO8bVQY".into(),
                ),
            },
            Case {
                desc: "ambiguous when multiple sources share the same substring",
                sources: &[
                    "https://www.youtube.com/watch?v=abc123",
                    "https://example.com/page?id=abc123",
                ],
                query: "abc123",
                expected: SourceResolveResult::Ambiguous(vec![
                    "https://www.youtube.com/watch?v=abc123".into(),
                    "https://example.com/page?id=abc123".into(),
                ]),
            },
            Case {
                desc: "leading-slash query matches URL source via suffix",
                sources: &["https://example.com/docs/guide.html"],
                query: "/docs/guide.html",
                expected: SourceResolveResult::Exact("https://example.com/docs/guide.html".into()),
            },
        ];

        for case in cases {
            let result = resolve_source_match(case.sources.iter().copied(), case.query);
            match (&result, &case.expected) {
                (SourceResolveResult::Exact(got), SourceResolveResult::Exact(want)) => {
                    assert_eq!(got, want, "case {:?}: wrong exact match", case.desc);
                }
                (SourceResolveResult::Ambiguous(_), SourceResolveResult::Ambiguous(_))
                | (SourceResolveResult::NotFound, SourceResolveResult::NotFound) => {}
                _ => panic!(
                    "case {:?}: expected {:?} but got {:?}",
                    case.desc, case.expected, result
                ),
            }
        }
    }

    #[test]
    fn resolve_not_found_cases() {
        let result = resolve_source_match(DEFAULT_SOURCES.iter().copied(), "/");
        assert!(matches!(result, SourceResolveResult::NotFound));

        for query in ["../secret", "docs/../api/reference.md"] {
            let result = resolve_source_match(DEFAULT_SOURCES.iter().copied(), query);
            assert!(
                matches!(result, SourceResolveResult::NotFound),
                "expected NotFound for {query:?}"
            );
        }

        let result = resolve_source_match(DEFAULT_SOURCES.iter().copied(), "nonexistent.txt");
        assert!(matches!(result, SourceResolveResult::NotFound));
    }
}
