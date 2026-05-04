use std::path::Path;

use anyhow::{Context, Result};

use crate::inject;

struct McpTool {
    name: &'static str,
    description: &'static str,
}

const MCP_TOOLS: &[McpTool] = &[
    McpTool {
        name: "lore_info",
        description: "Knowledge base overview: stats, topics, languages",
    },
    McpTool {
        name: "lore_list_topics",
        description: "List all topics with chunk counts",
    },
    McpTool {
        name: "lore_search",
        description: "Full-text search across body, title, sections, and tags",
    },
    McpTool {
        name: "lore_read_topic",
        description: "Read all chunks for a given topic",
    },
    McpTool {
        name: "lore_list_docs",
        description: "List documents with optional filters",
    },
    McpTool {
        name: "lore_read_doc",
        description: "Read a document's chunks sequentially",
    },
];

pub fn inject_all(repo_root: &Path) -> Result<()> {
    println!("mcp:");

    let mcp_md = repo_root.join("docs/mcp-integration.md");
    let cli_md = repo_root.join("docs/cli.md");

    let tools_table = render_tools_list();

    let replacements = [
        (
            "params-lore_search",
            render_tool_table::<lore::query::SearchArgs>()?,
        ),
        (
            "params-lore_list_topics",
            render_tool_table::<lore::query::TopicsArgs>()?,
        ),
        (
            "params-lore_read_topic",
            render_tool_table::<lore::query::TopicArgs>()?,
        ),
        (
            "params-lore_list_docs",
            render_tool_table::<lore::query::DocsArgs>()?,
        ),
        (
            "params-lore_read_doc",
            render_tool_table::<lore::query::ReadArgs>()?,
        ),
    ];

    let refs: Vec<(&str, &str)> = replacements
        .iter()
        .map(|(id, content)| (*id, content.as_str()))
        .collect();
    inject::inject(&mcp_md, &refs)?;

    inject::inject(&cli_md, &[("mcp-tools-list", &tools_table)])?;

    Ok(())
}

fn render_tools_list() -> String {
    let mut out = String::new();
    out.push_str("| Tool | Description |\n");
    out.push_str("|------|-------------|\n");
    for t in MCP_TOOLS {
        out.push_str(&format!("| `{}` | {} |\n", t.name, t.description));
    }
    out
}

fn render_tool_table<T: schemars::JsonSchema>() -> Result<String> {
    let schema = schemars::schema_for!(T);
    let json = serde_json::to_value(&schema).context("failed to serialize schema")?;

    let properties = json
        .get("properties")
        .and_then(|p| p.as_object())
        .context("schema has no properties")?;
    let required: Vec<&str> = json
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut out = String::new();
    out.push_str("| Parameter | Type | Default | Description |\n");
    out.push_str("|-----------|------|---------|-------------|\n");

    for (field_name, field_schema) in properties {
        let resolved = resolve_ref(field_schema, &json);
        let type_str = infer_type(resolved);
        let default_str = if required.contains(&field_name.as_str()) {
            "required".to_owned()
        } else {
            extract_default(resolved)
        };
        let desc = resolved
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        out.push_str(&format!(
            "| `{field_name}` | {type_str} | {default_str} | {desc} |\n"
        ));
    }

    Ok(out)
}

use crate::schema::resolve_ref;

fn infer_type(schema: &serde_json::Value) -> &'static str {
    if let Some(t) = schema.get("type").and_then(|t| t.as_str()) {
        return match t {
            "string" => "string",
            "integer" => "integer",
            "number" => "number",
            "boolean" => "boolean",
            "array" => "array",
            _ => "any",
        };
    }
    if let Some(any_of) = schema.get("anyOf").and_then(|a| a.as_array()) {
        for variant in any_of {
            if variant.get("type").and_then(|t| t.as_str()) == Some("null") {
                continue;
            }
            if let Some(t) = variant.get("type").and_then(|t| t.as_str()) {
                return match t {
                    "string" => "string",
                    "integer" => "integer",
                    "number" => "number",
                    "boolean" => "boolean",
                    _ => "any",
                };
            }
        }
    }
    // enum variants (oneOf with const values) -> string
    if schema.get("enum").is_some() || schema.get("oneOf").is_some() {
        return "string";
    }
    "any"
}

fn extract_default(schema: &serde_json::Value) -> String {
    if let Some(d) = schema.get("default") {
        if d.is_null() {
            return "--".to_owned();
        }
        if let Some(n) = d.as_u64() {
            return n.to_string();
        }
        if let Some(n) = d.as_f64() {
            return format!("{n}");
        }
        if let Some(b) = d.as_bool() {
            return b.to_string();
        }
        if let Some(s) = d.as_str() {
            return format!("`{s}`");
        }
        return d.to_string();
    }
    "--".to_owned()
}
