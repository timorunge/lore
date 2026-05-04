use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use tracing::error;

use lore::error::LoreError;
use lore::output::OutputMode;
use lore::query;
use lore::query::{DocsArgs, ReadArgs, SearchArgs, TopicArgs, TopicsArgs};

use crate::serve::LoreServer;

const INTERNAL_ERROR: &str = "Internal error -- check server logs.";

#[tool_router(vis = "pub(super)")]
impl LoreServer {
    /// Returns a knowledge base overview including document/chunk counts, topics, languages, and source types.
    #[tool(
        name = "lore_info",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn info(&self) -> String {
        crate::serve::format_store_overview(&self.cached_info(), |_| {
            "Use lore_list_topics to list all.".to_owned()
        })
    }

    /// List all topics, sorted alphabetically by default.
    /// Use topic names as filters in lore_search or as the argument to lore_read_topic.
    #[tool(
        name = "lore_list_topics",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn list_topics(&self, params: Parameters<TopicsArgs>) -> String {
        match query::list_topics(&self.store, params.0, OutputMode::Mcp) {
            Ok(r) => r.formatted,
            Err(e) => lore_error_to_string(&e, "lore_list_topics"),
        }
    }

    /// Full-text BM25 search with English stemming
    /// ("install" matches "installed", "installer").
    ///
    /// `total` reflects index-level matches; source filter and `max_per_source`
    /// reduce the page in memory, so a page may be shorter than `limit`.
    #[tool(
        name = "lore_search",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn search(&self, params: Parameters<SearchArgs>) -> String {
        match query::search(&self.store, params.0, OutputMode::Mcp) {
            Ok(r) => r.formatted,
            Err(e) => lore_error_to_string(&e, "lore_search"),
        }
    }

    /// Read chunks for a topic in document order. Prefer over
    /// lore_search for systematic topic coverage. Use offset to paginate.
    ///
    /// Topic name resolution: exact match, then case-insensitive, then substring
    /// ("auth" matches "Authentication"). Use lore_list_topics to see topic names.
    #[tool(
        name = "lore_read_topic",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn read_topic(&self, params: Parameters<TopicArgs>) -> String {
        match query::get_topic(&self.store, params.0, OutputMode::Mcp) {
            Ok(r) => r,
            Err(e) => lore_error_to_string(&e, "lore_read_topic"),
        }
    }

    /// List source documents with metadata. Filter by topic, title (case-insensitive
    /// substring), author, or language. Use source paths with lore_read_doc.
    #[tool(
        name = "lore_list_docs",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn list_docs(&self, params: Parameters<DocsArgs>) -> String {
        match query::list_documents(&self.store, params.0, OutputMode::Mcp) {
            Ok(r) => r.formatted,
            Err(e) => lore_error_to_string(&e, "lore_list_docs"),
        }
    }

    /// Read a document's chunks in order.
    /// Accepts partial paths: "readme.md", "docs/api", or full source paths.
    /// Returns candidates if multiple documents match.
    #[tool(
        name = "lore_read_doc",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn read_doc(&self, params: Parameters<ReadArgs>) -> String {
        match query::get_document(&self.store, params.0, OutputMode::Mcp) {
            Ok(r) => r,
            Err(e) => lore_error_to_string(&e, "lore_read_doc"),
        }
    }
}

/// Convert a `LoreError` into a user-facing string, logging internal errors.
fn lore_error_to_string(e: &LoreError, context: &str) -> String {
    match e {
        LoreError::EmptyQuery
        | LoreError::InvalidQuery
        | LoreError::NotFound(_)
        | LoreError::Ambiguous(_)
        | LoreError::StoreNotFound { .. }
        | LoreError::SchemaOutdated => e.to_string(),
        LoreError::Internal(inner) => {
            error!(error = %inner, "{context} failed");
            INTERNAL_ERROR.to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use rmcp::handler::server::wrapper::Parameters;

    use lore::query::{DocsArgs, Pagination, ReadArgs, SearchArgs, TopicArgs, TopicsArgs};
    use lore::store::test_helpers::temp_store;

    use super::super::make_test_server;

    fn has_kv_line(text: &str, key: &str, value: &str) -> bool {
        let prefix = format!("{key}:");
        text.lines().any(|line| {
            let trimmed = line.trim();
            trimmed.starts_with(&prefix) && trimmed[prefix.len()..].trim_start().starts_with(value)
        })
    }

    #[tokio::test]
    async fn mcp_tool_operations() {
        let (store, _dir) = temp_store();
        let server = super::LoreServer::from_store(store);
        let result: String = server.info().await;
        assert!(
            has_kv_line(&result, "Documents", "0"),
            "empty store info should show 0 documents; got: {result}"
        );
        assert!(
            has_kv_line(&result, "Chunks", "0"),
            "empty store info should show 0 chunks; got: {result}"
        );

        let (server, _dir) = make_test_server();
        let result: String = server.info().await;
        assert!(
            has_kv_line(&result, "Documents", "2"),
            "info should show 2 documents (intro + api); got: {result}"
        );
        assert!(
            has_kv_line(&result, "Chunks", "4"),
            "info should show 4 chunks (test_meta chunk_count=2 per doc, 2 docs); got: {result}"
        );

        let args = TopicsArgs {
            topic: None,
            author: None,
            source: None,
            lang: None,
            origin: None,
            kind: None,
            format: None,
            sort: None,
            reverse: None,
            pagination: Pagination::new(50, 0),
        };
        let result: String = server.list_topics(Parameters(args)).await;
        assert!(
            result.contains("api"),
            "list_topics should contain 'api' topic; got: {result}"
        );
        assert!(
            result.contains("intro"),
            "list_topics should contain 'intro' topic; got: {result}"
        );

        let args = SearchArgs {
            query: "API reference".to_owned(),
            pagination: Pagination::new(10, 0),
            topic: None,
            author: None,
            lang: None,
            source: None,
            origin: None,
            kind: None,
            format: None,
            max_per_source: None,
            sort: None,
            reverse: None,
        };
        let result: String = server.search(Parameters(args)).await;
        assert!(
            result.contains("/docs/api.md"),
            "search for 'API reference' should return results containing '/docs/api.md'; got: {result}"
        );
    }

    #[tokio::test]
    async fn mcp_search_error_cases() {
        let (server, _dir) = make_test_server();

        let args = SearchArgs {
            query: "   ".to_owned(),
            pagination: Pagination::new(10, 0),
            topic: None,
            author: None,
            lang: None,
            source: None,
            origin: None,
            kind: None,
            format: None,
            max_per_source: None,
            sort: None,
            reverse: None,
        };
        let result: String = server.search(Parameters(args)).await;
        assert_eq!(result, "search query must not be empty");

        let args = SearchArgs {
            query: "AND".to_owned(),
            pagination: Pagination::new(10, 0),
            topic: None,
            author: None,
            lang: None,
            source: None,
            origin: None,
            kind: None,
            format: None,
            max_per_source: None,
            sort: None,
            reverse: None,
        };
        let result: String = server.search(Parameters(args)).await;
        assert_eq!(result, "invalid search query syntax");
        assert!(!result.contains("tantivy"), "error must not leak internals");
        assert!(!result.contains("Syntax"), "error must not leak internals");
    }

    #[tokio::test]
    async fn mcp_read_and_list() {
        let (server, _dir) = make_test_server();

        let args = TopicArgs {
            topic: "api".to_owned(),
            pagination: Pagination::new(50, 0),
        };
        let result: String = server.read_topic(Parameters(args)).await;
        assert!(
            result.contains("API reference"),
            "read_topic('api') should contain chunk body text; got: {result}"
        );

        let args = TopicArgs {
            topic: "API".to_owned(),
            pagination: Pagination::new(50, 0),
        };
        let result: String = server.read_topic(Parameters(args)).await;
        assert!(
            result.contains("API reference"),
            "read_topic('API') should resolve case-insensitively; got: {result}"
        );

        let args = DocsArgs {
            pagination: Pagination::new(50, 0),
            source: None,
            topic: None,
            title: None,
            author: None,
            lang: None,
            origin: None,
            kind: None,
            format: None,
            sort: None,
            reverse: None,
        };
        let result: String = server.list_docs(Parameters(args)).await;
        assert!(
            result.contains("/docs/intro.md"),
            "list_docs should contain intro.md; got: {result}"
        );
        assert!(
            result.contains("/docs/api.md"),
            "list_docs should contain api.md; got: {result}"
        );

        let args = DocsArgs {
            pagination: Pagination::new(50, 0),
            source: None,
            topic: Some("api".to_owned()),
            title: None,
            author: None,
            lang: None,
            origin: None,
            kind: None,
            format: None,
            sort: None,
            reverse: None,
        };
        let result: String = server.list_docs(Parameters(args)).await;
        assert!(
            result.contains("/docs/api.md"),
            "list_docs filtered by topic='api' should contain api.md; got: {result}"
        );
        assert!(
            !result.contains("/docs/intro.md"),
            "list_docs filtered by topic='api' should not contain intro.md; got: {result}"
        );
    }

    #[tokio::test]
    async fn mcp_read_doc() {
        let (server, _dir) = make_test_server();

        let args = ReadArgs {
            source: "/docs/api.md".to_owned(),
            pagination: Pagination::new(50, 0),
            full: false,
        };
        let result: String = server.read_doc(Parameters(args)).await;
        assert!(
            result.contains("API reference"),
            "read_doc('/docs/api.md') should contain chunk text; got: {result}"
        );

        let args = ReadArgs {
            source: "api.md".to_owned(),
            pagination: Pagination::new(50, 0),
            full: false,
        };
        let result: String = server.read_doc(Parameters(args)).await;
        assert!(
            result.contains("API reference"),
            "read_doc('api.md') should resolve partial path; got: {result}"
        );

        let args = ReadArgs {
            source: "nonexistent.md".to_owned(),
            pagination: Pagination::new(50, 0),
            full: false,
        };
        let result: String = server.read_doc(Parameters(args)).await;
        assert!(
            result.contains("not found") || result.contains("No document"),
            "read_doc('nonexistent.md') should report not found; got: {result}"
        );
    }
}
