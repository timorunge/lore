use std::path::Path;

use anyhow::{Context, Result};
use schemars::JsonSchema;

use crate::inject;

pub fn inject_all(repo_root: &Path) -> Result<()> {
    println!("enums:");

    let config_md = repo_root.join("docs/configuration.md");
    let cli_md = repo_root.join("docs/cli.md");
    let architecture_md = repo_root.join("docs/architecture.md");

    inject::inject(
        &config_md,
        &[
            (
                "enum-update-mode",
                &render_enum_table::<lore::config::UpdateMode>("Mode", "Behavior")?,
            ),
            ("enum-source-types", &render_source_types_table()?),
            ("enum-extract-mode-detail", &render_extract_mode_3col()?),
            ("enum-transform-types", &render_transform_types_table()?),
            ("cache-subdirs", &render_cache_subdirs_table()?),
        ],
    )?;

    inject::inject(
        &cli_md,
        &[("enum-cache-scope", &render_cache_scope_table()?)],
    )?;

    inject::inject(
        &architecture_md,
        &[
            ("enum-extract-mode", &render_extract_mode_2col()?),
            ("source-discovery", &render_source_discovery_table()?),
        ],
    )?;

    Ok(())
}

fn render_enum_table<T: JsonSchema>(col1: &str, col2: &str) -> Result<String> {
    let schema = schemars::schema_for!(T);
    let json = serde_json::to_value(&schema).context("failed to serialize schema")?;
    render_variants_table(&json, col1, col2)
}

fn render_variants_table(json: &serde_json::Value, col1: &str, col2: &str) -> Result<String> {
    let variants = json
        .get("oneOf")
        .and_then(|v| v.as_array())
        .context("enum schema has no oneOf")?;

    let mut out = String::new();
    out.push_str(&format!("| {col1} | {col2} |\n"));
    out.push_str(&format!(
        "|{}|{}|\n",
        "-".repeat(col1.len() + 2),
        "-".repeat(col2.len() + 2)
    ));

    for variant in variants {
        let name = variant
            .get("const")
            .and_then(|c| c.as_str())
            .context("variant has no const")?;
        let desc = variant
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        let desc_oneline = desc
            .split('\n')
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&format!("| `{name}` | {desc_oneline} |\n"));
    }

    Ok(out)
}

struct SourceType {
    label: &'static str,
    key: &'static str,
    discovery: &'static str,
    desc_fn: fn() -> Result<String>,
}

const SOURCE_TYPES: &[SourceType] = &[
    SourceType {
        label: "Local",
        key: "path",
        discovery: "Walk directory, match glob, return file paths",
        desc_fn: source_desc::<lore::config::LocalSource>,
    },
    SourceType {
        label: "Git",
        key: "git",
        discovery: "Clone/fetch repo, check HEAD against stored hash, return file paths",
        desc_fn: source_desc::<lore::config::GitSource>,
    },
    SourceType {
        label: "S3",
        key: "s3",
        discovery: "List objects under prefix, filter by glob/regex, download inline",
        desc_fn: source_desc::<lore::config::S3Source>,
    },
    // -- web-based --
    SourceType {
        label: "URL",
        key: "url",
        discovery: "Build URL list with existing ETags for conditional fetching",
        desc_fn: source_desc::<lore::config::UrlSource>,
    },
    SourceType {
        label: "Sitemap",
        key: "sitemap",
        discovery: "Fetch and parse XML (including nested sitemaps), return URL list",
        desc_fn: source_desc::<lore::config::SitemapSource>,
    },
    SourceType {
        label: "Feed",
        key: "feed",
        discovery: "Fetch RSS/Atom XML, extract entry links, return URL list",
        desc_fn: source_desc::<lore::config::FeedSource>,
    },
    SourceType {
        label: "YouTube",
        key: "youtube",
        discovery: "Parse URL, discover video IDs (playlist/channel via `yt-dlp --flat-playlist`), preload transcripts",
        desc_fn: source_desc::<lore::config::YoutubeSource>,
    },
    SourceType {
        label: "Maildir",
        key: "maildir",
        discovery: "Walk cur/ and new/ subdirectories, parse RFC 2822 messages with mail-parser",
        desc_fn: source_desc::<lore::config::MaildirSource>,
    },
    SourceType {
        label: "Exec",
        key: "exec",
        discovery: "Run shell command(s) via `sh -c`, read JSONL from stdout, one document per line",
        desc_fn: source_desc::<lore::config::ExecSource>,
    },
    SourceType {
        label: "MCP",
        key: "mcp",
        discovery: "Connect to upstream MCP server, auto-discover resources and/or call tools, read text content",
        desc_fn: source_desc::<lore::config::McpSource>,
    },
];

fn render_source_types_table() -> Result<String> {
    let mut out = String::new();
    out.push_str("| Type | Key | Description |\n");
    out.push_str("|------|-----|-------------|\n");
    for st in SOURCE_TYPES {
        let desc = (st.desc_fn)()?;
        out.push_str(&format!("| {} | `{}` | {} |\n", st.label, st.key, desc));
    }
    Ok(out)
}

fn render_source_discovery_table() -> Result<String> {
    let mut out = String::new();
    out.push_str("| Source type | Discovery |\n");
    out.push_str("|-------------|-----------|\n");
    for st in SOURCE_TYPES {
        out.push_str(&format!("| {} | {} |\n", st.label, st.discovery));
    }
    Ok(out)
}

fn source_desc<T: JsonSchema>() -> Result<String> {
    let schema = schemars::schema_for!(T);
    let json = serde_json::to_value(&schema).context("failed to serialize schema")?;
    let desc = json
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("");
    let oneline = desc
        .split('\n')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ");
    Ok(oneline)
}

fn render_cache_scope_table() -> Result<String> {
    let schema = schemars::schema_for!(lore::cache::CacheScope);
    let json = serde_json::to_value(&schema).context("failed to serialize schema")?;
    render_variants_table(&json, "Scope", "Description")
}

fn render_extract_mode_2col() -> Result<String> {
    render_enum_table::<lore::config::ExtractMode>("Mode", "Behaviour")
}

fn render_extract_mode_3col() -> Result<String> {
    let schema = schemars::schema_for!(lore::config::ExtractMode);
    let json = serde_json::to_value(&schema).context("failed to serialize schema")?;
    let variants = json
        .get("oneOf")
        .and_then(|v| v.as_array())
        .context("ExtractMode schema has no oneOf")?;

    let mut out = String::new();
    out.push_str("| Mode | Text files | Binary files |\n");
    out.push_str("|------|------------|--------------|\n");

    for variant in variants {
        let name = variant
            .get("const")
            .and_then(|c| c.as_str())
            .context("variant has no const")?;
        let desc = variant
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        let desc_oneline = desc
            .split('\n')
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ");

        let (text_col, binary_col) = parse_text_binary(&desc_oneline);
        out.push_str(&format!("| `{name}` | {text_col} | {binary_col} |\n"));
    }

    Ok(out)
}

fn parse_text_binary(desc: &str) -> (String, String) {
    let text_start = desc.find("Text:");
    let binary_start = desc.find("Binary:");

    match (text_start, binary_start) {
        (Some(t), Some(b)) => {
            let text = desc[t + "Text:".len()..b].trim().trim_end_matches('.');
            let binary = desc[b + "Binary:".len()..].trim().trim_end_matches('.');
            (capitalize(text), capitalize(binary))
        }
        _ => {
            let clean = desc.trim_end_matches('.');
            (capitalize(clean), capitalize(clean))
        }
    }
}

fn capitalize(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return String::new();
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn render_transform_types_table() -> Result<String> {
    let schema = schemars::schema_for!(lore::config::Transform);
    let json = serde_json::to_value(&schema).context("failed to serialize schema")?;
    let variants = json
        .get("oneOf")
        .and_then(|v| v.as_array())
        .context("Transform schema has no oneOf")?;

    let mut out = String::new();
    out.push_str("| Type | Fields | Effect |\n");
    out.push_str("|------|--------|--------|\n");

    for variant in variants {
        let props = variant.get("properties").and_then(|p| p.as_object());
        let tag = props
            .and_then(|p| p.get("type"))
            .and_then(|t| t.get("const"))
            .and_then(|c| c.as_str())
            .context("transform variant has no type tag")?;

        let desc = variant
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        let desc_oneline = desc
            .split('\n')
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ");

        let fields: Vec<&str> = props
            .map(|p| {
                p.keys()
                    .filter(|k| *k != "type")
                    .map(|k| k.as_str())
                    .collect()
            })
            .unwrap_or_default();
        let fields_str = if fields.is_empty() {
            "(none)".to_owned()
        } else {
            fields
                .iter()
                .map(|f| format!("`{f}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };

        out.push_str(&format!("| `{tag}` | {fields_str} | {desc_oneline} |\n"));
    }

    Ok(out)
}

fn render_cache_subdirs_table() -> Result<String> {
    let schema = schemars::schema_for!(lore::cache::CacheScope);
    let json = serde_json::to_value(&schema).context("failed to serialize schema")?;
    let variants = json
        .get("oneOf")
        .and_then(|v| v.as_array())
        .context("CacheScope schema has no oneOf")?;

    let mut out = String::new();
    out.push_str("| Directory | Contents |\n");
    out.push_str("|-----------|----------|\n");

    for variant in variants {
        let name = variant
            .get("const")
            .and_then(|c| c.as_str())
            .context("variant has no const")?;
        if name == "all" {
            continue;
        }
        let desc = variant
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        let desc_oneline = desc
            .split('\n')
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&format!("| `{name}/` | {desc_oneline} |\n"));
    }

    Ok(out)
}
