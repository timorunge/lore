//! Query execution and result formatting.

use serde::Deserialize;

use crate::error::LoreError;
use crate::output::{self, OutputMode};
use crate::store::{DocFilter, SearchQuery, SourceResolveResult, StoreSet, TopicFilter};
pub use crate::store::{DocSort, SearchSort, TopicSort};

/// Hard cap on pagination limits (shared between CLI and MCP).
const MAX_PAGE_LIMIT: usize = 500;
/// Default page size when the caller omits `limit`.
const DEFAULT_PAGE_LIMIT: usize = 10;

type Result<T> = std::result::Result<T, LoreError>;

/// Pagination metadata for list operations.
#[derive(Debug, schemars::JsonSchema)]
pub struct Pagination {
    /// Maximum number of items to return (minimum 1, maximum 500).
    #[schemars(range(min = 1, max = 500))]
    pub limit: usize,
    /// Number of items to skip before returning results.
    pub offset: usize,
}

impl Pagination {
    /// Create a new pagination with `limit` clamped to the allowed range.
    pub fn new(limit: usize, offset: usize) -> Self {
        Self {
            limit: limit.clamp(1, MAX_PAGE_LIMIT),
            offset,
        }
    }

    /// Create an unlimited pagination (all results, no offset).
    pub fn unlimited() -> Self {
        Self {
            limit: usize::MAX,
            offset: 0,
        }
    }
}

impl<'de> serde::Deserialize<'de> for Pagination {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        fn default_limit() -> usize {
            DEFAULT_PAGE_LIMIT
        }

        #[derive(serde::Deserialize)]
        struct Raw {
            #[serde(default = "default_limit")]
            limit: usize,
            #[serde(default)]
            offset: usize,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(Self::new(raw.limit, raw.offset))
    }
}

/// Arguments for a full-text search query with optional filters and pagination.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// Free-text search query. Supports English stemming.
    pub query: String,
    #[serde(flatten)]
    pub pagination: Pagination,
    /// Filter by source path (substring match, case-sensitive).
    pub source: Option<String>,
    /// Filter results to a specific topic name.
    pub topic: Option<String>,
    /// Filter results by author.
    pub author: Option<String>,
    /// Filter results by language code (e.g. "en", "de").
    pub lang: Option<String>,
    /// Filter by source origin (e.g. "local", "url", "git", "s3").
    pub origin: Option<String>,
    /// Filter by document kind (e.g. "document", "code", "data").
    pub kind: Option<String>,
    /// Filter by file format (e.g. "md", "pdf", "rs", "json").
    pub format: Option<String>,
    /// Maximum results per source document for diversity. When set,
    /// over-fetches and filters to ensure diverse sources in results.
    pub max_per_source: Option<usize>,
    /// Sort order: score (default), source, topic.
    pub sort: Option<SearchSort>,
    /// Reverse the sort order.
    pub reverse: Option<bool>,
}

/// Arguments for retrieving a single topic by name.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TopicArgs {
    /// Topic name. Resolves via exact match, case-insensitive match, then substring.
    pub topic: String,
    #[serde(flatten)]
    pub pagination: Pagination,
}

/// Arguments for listing indexed documents with optional filters.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DocsArgs {
    #[serde(flatten)]
    pub pagination: Pagination,
    /// Filter by source path (substring match, case-sensitive).
    pub source: Option<String>,
    /// Filter by topic name.
    pub topic: Option<String>,
    /// Filter by title (substring match).
    pub title: Option<String>,
    /// Filter by author.
    pub author: Option<String>,
    /// Filter by language code.
    pub lang: Option<String>,
    /// Filter by source origin (e.g. "local", "url", "git", "s3").
    pub origin: Option<String>,
    /// Filter by document kind (e.g. "document", "code", "data").
    pub kind: Option<String>,
    /// Filter by file format (e.g. "md", "pdf", "rs", "json").
    pub format: Option<String>,
    /// Sort order: source (default), topic, title, author, chunks, words.
    pub sort: Option<DocSort>,
    /// Reverse the sort order.
    pub reverse: Option<bool>,
}

/// Arguments for reading a single document's chunks by source path.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadArgs {
    /// Full path, partial path, or basename of the document.
    pub source: String,
    #[serde(flatten)]
    pub pagination: Pagination,
    /// Show the full document as continuous text without chunk separators.
    #[serde(default)]
    pub full: bool,
}

/// Arguments for listing all topics with optional sorting and filters.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TopicsArgs {
    #[serde(flatten)]
    pub pagination: Pagination,
    /// Filter by topic name (fuzzy: exact, case-insensitive, then substring).
    pub topic: Option<String>,
    /// Filter by author (fuzzy: exact, case-insensitive, then substring).
    pub author: Option<String>,
    /// Filter by source path (substring match, case-sensitive).
    pub source: Option<String>,
    /// Filter by language code (e.g. "en", "de").
    pub lang: Option<String>,
    /// Filter by source origin (e.g. "local", "url", "git", "s3").
    pub origin: Option<String>,
    /// Filter by document kind (e.g. "document", "code", "data").
    pub kind: Option<String>,
    /// Filter by file format (e.g. "md", "pdf", "rs", "json").
    pub format: Option<String>,
    /// Sort order: name (default), chunks, words.
    pub sort: Option<TopicSort>,
    /// Reverse the sort order.
    pub reverse: Option<bool>,
}

/// Formatted query output with pagination metadata.
pub struct QueryResult {
    /// Human-readable (or machine-readable JSON) formatted output string.
    pub formatted: String,
    /// Total number of matching items before pagination.
    pub total: usize,
}

/// Canonicalised filter values after fuzzy name resolution against the store.
pub struct ResolvedFilters {
    pub topic: Option<String>,
    pub author: Option<String>,
    pub format: Option<String>,
}

/// Resolve topic, author, and format strings against the store's term dictionary.
pub fn resolve_filters(
    stores: &StoreSet,
    topic: Option<String>,
    author: Option<String>,
    format: Option<String>,
) -> ResolvedFilters {
    ResolvedFilters {
        topic: topic.map(|t| stores.resolve_topic_name(&t).unwrap_or(t)),
        author: author.map(|a| stores.resolve_author_name(&a).unwrap_or(a)),
        format: format.map(|f| f.to_lowercase()),
    }
}

/// Build a `SearchQuery` from `SearchArgs` and run it against the store set.
///
/// # Errors
///
/// Returns an error if the query is empty or the Tantivy search fails.
pub fn execute_search(
    stores: &StoreSet,
    args: SearchArgs,
) -> Result<(Vec<crate::store::SearchHit>, SearchQuery, usize)> {
    let limit = args.pagination.limit;
    let offset = args.pagination.offset;
    let resolved = resolve_filters(stores, args.topic, args.author, args.format);
    let sq = SearchQuery {
        query: args.query,
        limit,
        offset,
        source: args.source,
        topic: resolved.topic,
        author: resolved.author,
        lang: args.lang,
        origin: args.origin,
        kind: args.kind,
        format: resolved.format,
        max_per_source: args.max_per_source,
        sort: args.sort.unwrap_or_default(),
        reverse: args.reverse.unwrap_or_default(),
    };
    let (results, total) = stores.search(&sq)?;
    Ok((results, sq, total))
}

/// Full-text search across indexed chunks with optional filters.
///
/// # Errors
///
/// Returns an error if the search query fails or output formatting fails.
pub fn search(stores: &StoreSet, args: SearchArgs, mode: OutputMode) -> Result<QueryResult> {
    let offset = args.pagination.offset;
    let (results, sq, total) = execute_search(stores, args)?;
    let formatted = output::format_search_results(&results, &sq.query, total, offset, mode)?;
    Ok(QueryResult { formatted, total })
}

/// List all topics in the store with document and chunk counts.
///
/// # Errors
///
/// Returns an error if the store query fails or output formatting fails.
// `args` is taken by value because the store query consumes its fields;
// accepting `&TopicsArgs` would require either cloning every field or
// changing the store API to borrow, adding noise with no runtime benefit.
#[allow(clippy::needless_pass_by_value)]
pub fn list_topics(stores: &StoreSet, args: TopicsArgs, mode: OutputMode) -> Result<QueryResult> {
    let limit = args.pagination.limit;
    let offset = args.pagination.offset;
    let sort = args.sort.unwrap_or_default();
    let reverse = args.reverse.unwrap_or_default();
    let resolved = resolve_filters(stores, args.topic, args.author, args.format);
    let filter = TopicFilter {
        topic: resolved.topic,
        author: resolved.author,
        source: args.source,
        lang: args.lang,
        origin: args.origin,
        kind: args.kind,
        format: resolved.format,
    };
    let (page, total) = stores.list_topic_stats(&filter, sort, reverse, limit, offset);
    let formatted = output::format_topic_list(&page, total, offset, mode)?;
    Ok(QueryResult { formatted, total })
}

/// Retrieve details for a single topic by name.
///
/// # Errors
///
/// Returns an error if the store query fails, the topic is not found, or output formatting fails.
// Same by-value rationale as `list_topics`: fields are consumed by the query.
#[allow(clippy::needless_pass_by_value)]
pub fn get_topic(stores: &StoreSet, args: TopicArgs, mode: OutputMode) -> Result<String> {
    let limit = args.pagination.limit;
    let offset = args.pagination.offset;
    if let Some(topic) = stores.get_topic(&args.topic)? {
        Ok(output::format_topic(&topic, offset, limit, mode)?)
    } else {
        let hint = match mode {
            OutputMode::Mcp => "Use lore_list_topics to browse topics.",
            _ => "Use `lore topics` to browse topics.",
        };
        Err(LoreError::NotFound(format!(
            "topic not found: {:?}. {hint}",
            args.topic
        )))
    }
}

/// List indexed documents with optional filtering and pagination.
///
/// # Errors
///
/// Returns an error if output formatting fails.
pub fn list_documents(stores: &StoreSet, args: DocsArgs, mode: OutputMode) -> Result<QueryResult> {
    let limit = args.pagination.limit;
    let offset = args.pagination.offset;
    let resolved = resolve_filters(stores, args.topic, args.author, args.format);
    let filter = DocFilter {
        source: args.source,
        topic: resolved.topic,
        title: args.title,
        author: resolved.author,
        lang: args.lang,
        origin: args.origin,
        kind: args.kind,
        format: resolved.format,
        sort: args.sort.unwrap_or_default(),
        reverse: args.reverse.unwrap_or_default(),
    };
    let (page, total) = stores.list_documents(&filter, limit, offset);
    let formatted = output::format_document_list(&page, total, offset, mode)?;
    Ok(QueryResult { formatted, total })
}

/// Retrieve a single document's metadata and chunks by source path.
///
/// # Errors
///
/// Returns an error if the store query fails, the document is not found, or output formatting fails.
#[allow(clippy::needless_pass_by_value)]
pub fn get_document(stores: &StoreSet, args: ReadArgs, mode: OutputMode) -> Result<String> {
    let (limit, offset) = if args.full {
        (usize::MAX, 0)
    } else {
        (args.pagination.limit, args.pagination.offset)
    };
    let is_mcp = mode == OutputMode::Mcp;

    let not_found_hint = if is_mcp {
        "Use lore_list_docs to browse documents."
    } else {
        "Use `lore docs` to browse documents."
    };

    match stores.resolve_source_key(&args.source) {
        SourceResolveResult::NotFound => Err(LoreError::NotFound(format!(
            "document not found: {:?}. {not_found_hint}",
            args.source
        ))),
        SourceResolveResult::Ambiguous(candidates) => {
            let mut lines = vec![format!(
                "multiple documents match {:?}. Be more specific:\n",
                args.source
            )];
            let prefix = if is_mcp { "- " } else { "  " };
            for c in &candidates {
                lines.push(format!("{prefix}{c}"));
            }
            if !is_mcp {
                lines.push(format!("\n{not_found_hint}"));
            }
            Err(LoreError::Ambiguous(lines.join("\n")))
        }
        SourceResolveResult::Exact(resolved) => {
            if let Some(doc) = stores.get_document(&resolved)? {
                Ok(output::format_document(
                    &doc, offset, limit, args.full, mode, false,
                )?)
            } else {
                let sep = if is_mcp { " " } else { "\n" };
                Err(LoreError::NotFound(format!(
                    "document not found: {resolved:?}.{sep}{not_found_hint}"
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoreSet;
    use crate::store::test_helpers::{temp_store, test_chunk, test_meta};

    fn populated_store() -> (StoreSet, tempfile::TempDir) {
        let (store, dir) = temp_store();
        let chunks = vec![
            test_chunk(
                "/a.md",
                "the quick brown fox jumps over the lazy dog",
                "animals",
                0,
            ),
            test_chunk(
                "/a.md",
                "foxes are wild animals that live in forests",
                "animals",
                1,
            ),
            test_chunk(
                "/b.md",
                "rust programming language is fast and safe",
                "tech",
                0,
            ),
        ];
        store.insert_chunks(&chunks).unwrap();
        store.commit().unwrap();
        store.upsert_document(test_meta("/a.md", "animals"));
        store.upsert_document(test_meta("/b.md", "tech"));
        (StoreSet::single(store), dir)
    }

    #[test]
    fn pagination_clamp_and_defaults() {
        assert_eq!(Pagination::new(0, 0).limit, 1);

        let p: Pagination = serde_json::from_str("{}").unwrap();
        assert_eq!(p.limit, DEFAULT_PAGE_LIMIT);
        assert_eq!(p.offset, 0);

        let p: Pagination = serde_json::from_str(r#"{"limit": 5}"#).unwrap();
        assert_eq!(p.limit, 5);
        assert_eq!(p.offset, 0);

        let p: Pagination = serde_json::from_str(r#"{"offset": 3}"#).unwrap();
        assert_eq!(p.limit, DEFAULT_PAGE_LIMIT);
        assert_eq!(p.offset, 3);

        let p: Pagination = serde_json::from_str(r#"{"limit": 99999}"#).unwrap();
        assert_eq!(p.limit, MAX_PAGE_LIMIT);
    }

    #[test]
    fn search_matching_query() {
        let (store, _dir) = populated_store();
        let result = search(
            &store,
            SearchArgs {
                query: "fox".to_owned(),
                pagination: Pagination::new(10, 0),
                source: None,
                topic: None,
                author: None,
                lang: None,
                origin: None,
                kind: None,
                format: None,
                max_per_source: None,
                sort: None,
                reverse: None,
            },
            OutputMode::Mcp,
        )
        .unwrap();
        assert_eq!(result.total, 2, "fixture has 2 chunks containing 'fox'");
        assert!(
            result.formatted.contains("/a.md"),
            "result should reference the source document"
        );
        assert!(
            result.formatted.contains("fox"),
            "search result should contain query term, got: {}",
            result.formatted
        );
    }

    #[test]
    fn list_documents_with_topic_filter() {
        let (store, _dir) = populated_store();
        let result = list_documents(
            &store,
            DocsArgs {
                pagination: Pagination::new(10, 0),
                source: None,
                topic: Some("animals".to_owned()),
                title: None,
                author: None,
                lang: None,
                origin: None,
                kind: None,
                format: None,
                sort: None,
                reverse: None,
            },
            OutputMode::Mcp,
        )
        .unwrap();
        assert_eq!(result.total, 1, "exactly one doc in 'animals' topic");
        assert!(
            result.formatted.contains("/a.md"),
            "output should include /a.md for 'animals' topic, got: {}",
            result.formatted
        );
        assert!(
            !result.formatted.contains("/b.md"),
            "output should not include /b.md for 'animals' topic, got: {}",
            result.formatted
        );
    }

    #[test]
    fn get_topic_not_found() {
        let (store, _dir) = populated_store();
        let err = get_topic(
            &store,
            TopicArgs {
                topic: "nonexistent_topic_xyzzy".to_owned(),
                pagination: Pagination::new(10, 0),
            },
            OutputMode::Mcp,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not found"),
            "error should indicate topic not found, got: {msg}"
        );
    }
}
