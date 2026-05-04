use std::path::Path;

use anyhow::{Context, Result};

use crate::inject;

pub fn inject_all(repo_root: &Path) -> Result<()> {
    println!("config:");

    let config_md = repo_root.join("docs/configuration.md");

    let replacements = vec![
        (
            "config-store",
            render_config_table::<lore::config::StoreConfig>()?,
        ),
        ("config-processing", render_processing_table()?),
        (
            "config-content-filter",
            render_config_table::<lore::config::ContentFilterConfig>()?,
        ),
        (
            "config-processing-profile",
            render_config_table::<lore::config::ProcessingProfile>()?,
        ),
        (
            "config-fetch",
            render_config_table::<lore::config::FetchConfig>()?,
        ),
        ("config-llm", render_llm_table()?),
        (
            "config-llm-detect-topics",
            render_config_table::<lore::config::DetectTopicsConfig>()?,
        ),
        (
            "config-llm-summarize-docs",
            render_config_table::<lore::config::SummarizeDocsConfig>()?,
        ),
        (
            "config-llm-enrich-chunks",
            render_config_table::<lore::config::EnrichChunksConfig>()?,
        ),
        (
            "config-source-local",
            render_config_table::<lore::config::LocalSource>()?,
        ),
        (
            "config-source-url",
            render_config_table::<lore::config::UrlSource>()?,
        ),
        (
            "config-source-git",
            render_config_table::<lore::config::GitSource>()?,
        ),
        (
            "config-source-sitemap",
            render_config_table::<lore::config::SitemapSource>()?,
        ),
        (
            "config-source-feed",
            render_config_table::<lore::config::FeedSource>()?,
        ),
        (
            "config-source-s3",
            render_config_table::<lore::config::S3Source>()?,
        ),
        (
            "config-source-youtube",
            render_config_table::<lore::config::YoutubeSource>()?,
        ),
        (
            "config-source-maildir",
            render_config_table::<lore::config::MaildirSource>()?,
        ),
        (
            "config-source-exec",
            render_config_table::<lore::config::ExecSource>()?,
        ),
        (
            "config-source-mcp",
            render_config_table::<lore::config::McpSource>()?,
        ),
        (
            "config-global",
            render_config_table::<lore::config::GlobalConfig>()?,
        ),
    ];

    let refs: Vec<(&str, &str)> = replacements
        .iter()
        .map(|(id, content)| (*id, content.as_str()))
        .collect();
    inject::inject(&config_md, &refs)?;

    Ok(())
}

fn render_config_table<T: schemars::JsonSchema>() -> Result<String> {
    let schema = schemars::schema_for!(T);
    let json = serde_json::to_value(&schema).context("failed to serialize schema")?;
    render_table_from_schema(&json)
}

fn render_table_from_schema(json: &serde_json::Value) -> Result<String> {
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
    out.push_str("| Field | Type | Default | Description |\n");
    out.push_str("|-------|------|---------|-------------|\n");

    for (field_name, field_schema) in properties {
        let resolved = resolve_ref(field_schema, json);
        let desc = collapse_desc_with_fallback(field_schema, resolved);
        let type_str = override_type(&desc, infer_type(resolved, json));
        let default_str = if required.contains(&field_name.as_str()) {
            "--".to_owned()
        } else {
            override_default(
                field_name,
                extract_default_with_fallback(field_schema, resolved),
            )
        };
        out.push_str(&format!(
            "| `{field_name}` | {type_str} | {default_str} | {desc} |\n"
        ));
    }

    Ok(out)
}

/// ProcessingConfig has many fields; generate a table from the full schema
/// but skip the `presets` field (it's a map, documented separately).
fn render_processing_table() -> Result<String> {
    let schema = schemars::schema_for!(lore::config::ProcessingConfig);
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
    out.push_str("| Field | Type | Default | Description |\n");
    out.push_str("|-------|------|---------|-------------|\n");

    for (field_name, field_schema) in properties {
        let resolved = resolve_ref(field_schema, &json);
        let desc = collapse_desc_with_fallback(field_schema, resolved);
        let type_str = override_type(&desc, infer_type(resolved, &json));
        let default_str = if required.contains(&field_name.as_str()) {
            "--".to_owned()
        } else {
            override_default(
                field_name,
                extract_default_with_fallback(field_schema, resolved),
            )
        };
        out.push_str(&format!(
            "| `{field_name}` | {type_str} | {default_str} | {desc} |\n"
        ));
    }

    Ok(out)
}

/// LlmConfig: render only the top-level provider/model fields,
/// not the nested sub-config objects (they get their own tables).
fn render_llm_table() -> Result<String> {
    let schema = schemars::schema_for!(lore::config::LlmConfig);
    let json = serde_json::to_value(&schema).context("failed to serialize schema")?;

    let properties = json
        .get("properties")
        .and_then(|p| p.as_object())
        .context("schema has no properties")?;

    let skip_fields = ["detect_topics", "summarize_docs", "enrich_chunks"];

    let mut out = String::new();
    out.push_str("| Field | Type | Default | Description |\n");
    out.push_str("|-------|------|---------|-------------|\n");

    for (field_name, field_schema) in properties {
        if skip_fields.contains(&field_name.as_str()) {
            continue;
        }
        let resolved = resolve_ref(field_schema, &json);
        let desc = collapse_desc_with_fallback(field_schema, resolved);
        let type_str = override_type(&desc, infer_type(resolved, &json));
        let default_str = override_default(
            field_name,
            extract_default_with_fallback(field_schema, resolved),
        );
        out.push_str(&format!(
            "| `{field_name}` | {type_str} | {default_str} | {desc} |\n"
        ));
    }

    Ok(out)
}

fn collapse_desc(schema: &serde_json::Value) -> String {
    let raw = schema
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("");
    raw.split('\n')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

fn collapse_desc_with_fallback(
    field_schema: &serde_json::Value,
    resolved: &serde_json::Value,
) -> String {
    let from_field = collapse_desc(field_schema);
    if !from_field.is_empty() {
        return from_field;
    }
    collapse_desc(resolved)
}

fn extract_default_with_fallback(
    field_schema: &serde_json::Value,
    resolved: &serde_json::Value,
) -> String {
    if field_schema.get("default").is_some() {
        return extract_default(field_schema);
    }
    extract_default(resolved)
}

use crate::schema::resolve_ref;

fn infer_type(schema: &serde_json::Value, root: &serde_json::Value) -> String {
    if let Some(t) = schema.get("type") {
        if let Some(arr) = t.as_array() {
            let types: Vec<&str> = arr
                .iter()
                .filter_map(|v| v.as_str())
                .filter(|s| *s != "null")
                .collect();
            if types.len() == 1 {
                return infer_simple_type(types[0], schema, root);
            }
        }
        if let Some(s) = t.as_str() {
            return infer_simple_type(s, schema, root);
        }
    }
    if let Some(any_of) = schema.get("anyOf").and_then(|a| a.as_array()) {
        let non_null: Vec<_> = any_of
            .iter()
            .filter(|v| v.get("type").and_then(|t| t.as_str()) != Some("null"))
            .collect();
        if non_null.len() > 1 {
            let types: Vec<String> = non_null
                .iter()
                .map(|v| {
                    let resolved = resolve_ref(v, root);
                    infer_type(resolved, root)
                })
                .collect();
            return types.join(" or ");
        }
        if let Some(variant) = non_null.first() {
            let resolved = resolve_ref(variant, root);
            return infer_type(resolved, root);
        }
    }
    if let Some(one_of) = schema.get("oneOf").and_then(|a| a.as_array()) {
        if is_string_enum(one_of) {
            return "string".to_owned();
        }
        if one_of
            .iter()
            .all(|v| v.get("type").and_then(|t| t.as_str()) == Some("object"))
        {
            return "object".to_owned();
        }
    }
    if schema.get("enum").is_some() {
        return "string".to_owned();
    }
    "any".to_owned()
}

fn is_string_enum(variants: &[serde_json::Value]) -> bool {
    variants.iter().all(|v| {
        v.get("const").and_then(|c| c.as_str()).is_some()
            || v.get("type").and_then(|t| t.as_str()) == Some("string")
            || v.get("enum")
                .and_then(|e| e.as_array())
                .is_some_and(|a| a.iter().all(|val| val.is_string()))
    })
}

fn infer_simple_type(t: &str, schema: &serde_json::Value, root: &serde_json::Value) -> String {
    match t {
        "string" => "string".to_owned(),
        "integer" => "int".to_owned(),
        "number" => "float".to_owned(),
        "boolean" => "bool".to_owned(),
        "array" => {
            if let Some(items) = schema.get("items") {
                let resolved_items = resolve_ref(items, root);
                let inner = infer_type(resolved_items, root);
                format!("list of {inner}s")
            } else {
                "list".to_owned()
            }
        }
        "object" => {
            if let Some(ap) = schema.get("additionalProperties")
                && ap.as_bool() != Some(false)
            {
                return "map".to_owned();
            }
            "object".to_owned()
        }
        _ => "any".to_owned(),
    }
}

fn extract_default(schema: &serde_json::Value) -> String {
    if let Some(d) = schema.get("default") {
        if d.is_null() {
            return "`null`".to_owned();
        }
        if let Some(n) = d.as_u64() {
            return format!("`{n}`");
        }
        if let Some(n) = d.as_i64() {
            return format!("`{n}`");
        }
        if let Some(n) = d.as_f64() {
            if n == n.trunc() {
                return format!("`{}`", n as i64);
            }
            return format!("`{n}`");
        }
        if let Some(b) = d.as_bool() {
            return format!("`{b}`");
        }
        if let Some(s) = d.as_str() {
            if s.is_empty() {
                return "`\"\"`".to_owned();
            }
            return format!("`\"{s}\"`");
        }
        if d.is_array() {
            if let Some(arr) = d.as_array()
                && arr.is_empty()
            {
                return "`[]`".to_owned();
            }
            return format!("`{d}`");
        }
        if d.is_object() {
            if let Some(obj) = d.as_object()
                && obj.is_empty()
            {
                return "`{}`".to_owned();
            }
            return "see below".to_owned();
        }
        return format!("`{d}`");
    }
    "--".to_owned()
}

const CONCURRENCY_FIELDS: &[&str] = &["concurrency"];

fn is_concurrency_field(field_name: &str) -> bool {
    CONCURRENCY_FIELDS.contains(&field_name)
}

fn override_default(field_name: &str, default: String) -> String {
    if is_concurrency_field(field_name) {
        return "half cores".to_owned();
    }
    default
}

fn override_type(desc: &str, type_str: String) -> String {
    if type_str.starts_with("list of string") && desc.contains("string or list") {
        return "string or list".to_owned();
    }
    type_str
}
