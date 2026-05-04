use std::path::Path;

use anyhow::Result;

use crate::inject;

struct Feature {
    name: &'static str,
    default_on: bool,
    description: &'static str,
    formats_description: &'static str,
    extra_dep: &'static str,
    show_in_readme: bool,
    show_in_formats: bool,
}

const FEATURES: &[Feature] = &[
    Feature {
        name: "ocr",
        default_on: true,
        description: "OCR for scanned PDFs and images (requires cmake)",
        formats_description: "OCR for scanned PDFs and images. Builds tesseract from source, requires cmake.",
        extra_dep: "cmake",
        show_in_readme: true,
        show_in_formats: true,
    },
    Feature {
        name: "llm",
        default_on: false,
        description: "LLM enrichment (`lore ingest`, `lore enrich`): Ollama, Anthropic, OpenAI, and Bedrock",
        formats_description: "LLM enrichment: per-chunk quality scoring, summaries, and keyword generation via Ollama, Anthropic, OpenAI, or Bedrock. Enriched fields are indexed and boost search relevance.",
        extra_dep: "AWS SDK (Bedrock)",
        show_in_readme: true,
        show_in_formats: true,
    },
    Feature {
        name: "s3",
        default_on: false,
        description: "Amazon S3 source support",
        formats_description: "",
        extra_dep: "AWS SDK crates",
        show_in_readme: true,
        show_in_formats: false,
    },
    Feature {
        name: "mcp",
        default_on: true,
        description: "Upstream MCP server ingestion (resources and tool calls)",
        formats_description: "",
        extra_dep: "rmcp",
        show_in_readme: true,
        show_in_formats: false,
    },
    Feature {
        name: "iwork",
        default_on: false,
        description: "Apple iWork documents (Keynote, Pages, Numbers)",
        formats_description: "Apple Keynote, Pages, Numbers",
        extra_dep: "none",
        show_in_readme: true,
        show_in_formats: true,
    },
    Feature {
        name: "tree-sitter",
        default_on: false,
        description: "Source code parsing via tree-sitter",
        formats_description: "Language-aware source code parsing",
        extra_dep: "none",
        show_in_readme: true,
        show_in_formats: true,
    },
    Feature {
        name: "test-support",
        default_on: false,
        description: "Exposes `store_test_support` helpers for integration tests",
        formats_description: "",
        extra_dep: "none",
        show_in_readme: false,
        show_in_formats: false,
    },
];

pub fn inject_all(repo_root: &Path) -> Result<()> {
    println!("features:");

    inject::inject(
        &repo_root.join("README.md"),
        &[("compile-features", &render_readme())],
    )?;

    inject::inject(
        &repo_root.join("docs/formats.md"),
        &[("compile-features-formats", &render_formats())],
    )?;

    inject::inject(
        &repo_root.join("docs/contributing.md"),
        &[("compile-features-contributing", &render_contributing())],
    )?;

    Ok(())
}

fn render_readme() -> String {
    let mut out = String::new();
    out.push_str("| Flag | Default | Description |\n");
    out.push_str("|------|---------|-------------|\n");
    for f in FEATURES.iter().filter(|f| f.show_in_readme) {
        let default = if f.default_on { "on" } else { "off" };
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            f.name, default, f.description
        ));
    }
    out
}

fn render_formats() -> String {
    let mut out = String::new();
    out.push_str("| Flag | What it adds |\n");
    out.push_str("|------|--------------|\n");
    for f in FEATURES.iter().filter(|f| f.show_in_formats) {
        let flag = if f.default_on {
            format!("`{}` (default: on)", f.name)
        } else {
            format!("`{}`", f.name)
        };
        out.push_str(&format!("| {} | {} |\n", flag, f.formats_description));
    }
    out
}

fn render_contributing() -> String {
    let mut out = String::new();
    out.push_str("| Feature | What it adds | Extra dep |\n");
    out.push_str("|---------|--------------|-----------|\n");
    for f in FEATURES {
        let feature = if f.default_on {
            format!("`{}` (default)", f.name)
        } else {
            format!("`{}`", f.name)
        };
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            feature, f.description, f.extra_dep
        ));
    }
    out
}
