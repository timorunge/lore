use rmcp::model::{
    ListResourceTemplatesResult, ListResourcesResult, ReadResourceResult, Resource,
    ResourceContents, ResourceTemplate,
};
use tracing::error;

use lore::error::LoreError;
use lore::fmt::plural;
use lore::output::OutputMode;
use lore::query::{self, Pagination, ReadArgs, TopicArgs};
use lore::store::{DocFilter, StoreInfo, StoreSet};

const INFO_URI: &str = "lore://info";
const TOPIC_URI_PREFIX: &str = "lore://topics/";
const DOC_URI_PREFIX: &str = "lore://docs/";

const PAGE_SIZE: usize = 200;

/// Build concrete resource entries for every topic and document in the store.
pub(super) fn build_resource_list(stores: &StoreSet, info: &StoreInfo) -> Vec<Resource> {
    let mut resources = Vec::new();

    resources.push(
        Resource::new(INFO_URI, "Knowledge Base Info")
            .with_description("Overview of the knowledge base: document count, topics, languages")
            .with_mime_type("text/plain"),
    );

    for topic in &info.topics {
        let uri = format!("{TOPIC_URI_PREFIX}{}", topic.name);
        let description = format!(
            "{} doc{}, {} chunk{}",
            topic.doc_count,
            plural(topic.doc_count),
            topic.chunk_count,
            plural(topic.chunk_count),
        );
        resources.push(
            Resource::new(uri, format!("Topic: {}", topic.name))
                .with_description(description)
                .with_mime_type("text/markdown"),
        );
    }

    let (docs, _) = stores.list_documents(&DocFilter::default(), usize::MAX, 0);
    for doc in docs {
        let uri = format!("{DOC_URI_PREFIX}{}", doc.source);
        let name = doc.title.unwrap_or_else(|| {
            doc.source
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or(&doc.source)
                .to_owned()
        });
        let mut desc_parts: Vec<String> = Vec::new();
        if let Some(t) = doc.topic {
            desc_parts.push(t);
        }
        desc_parts.push(format!(
            "{} chunk{}",
            doc.chunk_count,
            plural(doc.chunk_count)
        ));
        resources.push(
            Resource::new(uri, name)
                .with_description(desc_parts.join(" -- "))
                .with_mime_type("text/markdown"),
        );
    }

    resources
}

/// Slice the full resource list into a cursor-paginated response.
pub(super) fn page_resources(all: &[Resource], offset: usize) -> ListResourcesResult {
    if offset >= all.len() {
        return ListResourcesResult::default();
    }
    let end = (offset + PAGE_SIZE).min(all.len());
    let next_cursor = if end < all.len() {
        Some(end.to_string())
    } else {
        None
    };
    ListResourcesResult {
        resources: all[offset..end].to_vec(),
        next_cursor,
        ..Default::default()
    }
}

/// Build the list of resource templates for topics and documents.
pub(super) fn list_resource_templates() -> ListResourceTemplatesResult {
    let topic_template = ResourceTemplate::new("lore://topics/{name}", "Topic")
        .with_description("Read chunks for a topic (first 500)")
        .with_mime_type("text/markdown");
    let doc_template = ResourceTemplate::new("lore://docs/{path}", "Document")
        .with_description("Read a document's chunks by source path (first 500)")
        .with_mime_type("text/markdown");
    ListResourceTemplatesResult {
        resource_templates: vec![topic_template, doc_template],
        ..Default::default()
    }
}

/// Handle a resource read request.
pub(super) fn read_resource(
    stores: &StoreSet,
    info: &StoreInfo,
    uri: &str,
) -> Result<ReadResourceResult, rmcp::model::ErrorData> {
    if uri == INFO_URI {
        let text = format_info(info);
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, uri).with_mime_type("text/plain"),
        ]))
    } else if let Some(name) = uri.strip_prefix(TOPIC_URI_PREFIX) {
        let args = TopicArgs {
            topic: name.to_owned(),
            pagination: Pagination::new(500, 0),
        };
        let text = query::get_topic(stores, args, OutputMode::Mcp)
            .map_err(|e| lore_error_to_resource_error(&e, uri))?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, uri).with_mime_type("text/markdown"),
        ]))
    } else if let Some(path) = uri.strip_prefix(DOC_URI_PREFIX) {
        let args = ReadArgs {
            source: path.to_owned(),
            pagination: Pagination::new(500, 0),
            full: false,
        };
        let text = query::get_document(stores, args, OutputMode::Mcp)
            .map_err(|e| lore_error_to_resource_error(&e, uri))?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, uri).with_mime_type("text/markdown"),
        ]))
    } else {
        Err(rmcp::model::ErrorData::resource_not_found(
            format!("unknown resource URI: {uri}"),
            None,
        ))
    }
}

/// Convert a `LoreError` into an MCP `ErrorData`, logging internal errors.
fn lore_error_to_resource_error(e: &LoreError, uri: &str) -> rmcp::model::ErrorData {
    match e {
        LoreError::NotFound(_) | LoreError::Ambiguous(_) => {
            rmcp::model::ErrorData::resource_not_found(e.to_string(), None)
        }
        LoreError::EmptyQuery
        | LoreError::InvalidQuery
        | LoreError::StoreNotFound { .. }
        | LoreError::SchemaOutdated => rmcp::model::ErrorData::internal_error(e.to_string(), None),
        LoreError::Internal(inner) => {
            error!(error = %inner, uri, "read_resource failed");
            rmcp::model::ErrorData::internal_error("Internal error -- check server logs.", None)
        }
    }
}

/// Format the plain-text body for the lore://info resource.
fn format_info(info: &StoreInfo) -> String {
    crate::serve::format_store_overview(info, |total| {
        format!(
            "{total} topic{} total. Use lore://topics/{{name}} to browse.",
            plural(total)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lore::store::StoreSet;
    use lore::store::test_helpers::temp_store;

    #[test]
    fn read_resource_cases() {
        let (store, _dir) = temp_store();
        let stores = StoreSet::single(store);
        let info = stores.store_info();
        let result = read_resource(&stores, &info, INFO_URI);
        let result = result.unwrap();
        assert!(!result.contents.is_empty());

        let (store, _dir) = temp_store();
        let stores = StoreSet::single(store);
        let info = stores.store_info();
        let result = read_resource(&stores, &info, "lore://unknown/foo");
        assert!(result.is_err());

        let (server, _dir) = super::super::make_test_server();
        let result = read_resource(&server.store, &server.cached_info(), "lore://topics/api");
        assert!(result.is_ok(), "topic resource should succeed: {result:?}");

        let (server, _dir) = super::super::make_test_server();
        let result = read_resource(
            &server.store,
            &server.cached_info(),
            "lore://docs//docs/api.md",
        );
        assert!(result.is_ok(), "doc resource should succeed: {result:?}");
    }

    #[test]
    fn build_resource_list_enumerates_store() {
        let (server, _dir) = super::super::make_test_server();
        let info = server.cached_info();
        let resources = build_resource_list(&server.store, &info);

        // 1 info + 2 topics + 2 docs
        assert_eq!(resources.len(), 5);
        assert_eq!(resources[0].uri, INFO_URI);
        assert!(resources[1].uri.starts_with(TOPIC_URI_PREFIX));
        assert!(resources[3].uri.starts_with(DOC_URI_PREFIX));
    }

    #[test]
    fn page_resources_pagination() {
        let make = |i: usize| Resource::new(format!("test://r{i}"), format!("r{i}"));
        let all: Vec<Resource> = (0..450).map(make).collect();

        let p0 = page_resources(&all, 0);
        assert_eq!(p0.resources.len(), PAGE_SIZE);
        assert_eq!(p0.next_cursor, Some("200".to_owned()));

        let last = page_resources(&all, 400);
        assert_eq!(last.resources.len(), 50);
        assert!(last.next_cursor.is_none());

        let past = page_resources(&all, 500);
        assert!(past.resources.is_empty());
    }
}
