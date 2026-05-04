use std::borrow::Cow;
use std::fmt::Write as _;

use anyhow::Result;

use crate::fmt::table::{Align, Cell, Column, Truncate, format_kv, format_table};
use crate::fmt::{plural, to_json_pretty};
use crate::output::{
    OutputMode, clean_body, format_chunks_full_cli, format_chunks_page_cli, format_chunks_page_mcp,
    format_paged_json, pagination_note,
};
use crate::store::DocDetail;
use crate::types::{Chunk, DocMeta};

/// Format a paginated list of document metadata.
pub fn format_document_list(
    entries: &[DocMeta],
    total: usize,
    offset: usize,
    mode: OutputMode,
) -> Result<String> {
    match mode {
        OutputMode::Mcp => Ok(format_document_list_mcp(entries, total, offset)),
        OutputMode::Cli { .. } => Ok(format_document_list_cli(entries, total, offset, mode)),
        OutputMode::Json => format_paged_json(None, total, offset, entries),
    }
}

/// Format a document list for MCP output.
fn format_document_list_mcp(entries: &[DocMeta], total: usize, offset: usize) -> String {
    if entries.is_empty() {
        return "No documents found. Try removing filters or use lore_list_topics to browse topics.".to_owned();
    }
    let mut out = format!("{total} document{}\n", plural(total));
    for e in entries {
        w!(out, "- {}", e.source);
        if let Some(title) = &e.title {
            w!(out, " -- {title}");
        }
        let mut meta = Vec::new();
        if let Some(topic) = &e.topic {
            meta.push(topic.clone());
        }
        if let Some(author) = &e.author {
            meta.push(author.clone());
        }
        if e.word_count > 0 {
            meta.push(format!("{} words", e.word_count));
        }
        meta.push(format!("{} chunks", e.chunk_count));
        if let Some(q) = e.avg_llm_quality_score {
            meta.push(format!("q:{q:.2}"));
        }
        wln!(out, " ({})", meta.join(", "));
        if let Some(summary) = &e.llm_summary {
            wln!(out, "  {summary}");
        }
    }
    if let Some(note) = pagination_note(offset, entries.len(), total, OutputMode::Mcp) {
        w!(out, "\n{note}");
    }
    out
}

/// Format a document list for CLI output.
fn format_document_list_cli(
    entries: &[DocMeta],
    total: usize,
    offset: usize,
    mode: OutputMode,
) -> String {
    if entries.is_empty() {
        return "No documents found. Try removing filters or use `lore topics` to browse topics."
            .to_owned();
    }

    let show_quality = entries.iter().any(|e| e.avg_llm_quality_score.is_some());

    let mut columns = vec![
        // Source path -- left-truncated so the filename stays visible.
        Column {
            align: Align::Left,
            min_width: Some(20),
            max_width: Some(50),
            truncate: Truncate::Left,
            flexible: true,
        },
        Column {
            align: Align::Left,
            min_width: Some(15),
            max_width: Some(50),
            truncate: Truncate::Wrap,
            flexible: true,
        },
    ];
    columns.push(Column {
        align: Align::Right,
        min_width: Some(10),
        max_width: Some(13),
        truncate: Truncate::Right,
        flexible: false,
    });
    columns.push(Column {
        align: Align::Right,
        min_width: None,
        max_width: Some(12),
        truncate: Truncate::Right,
        flexible: false,
    });
    if show_quality {
        columns.push(Column {
            align: Align::Right,
            min_width: Some(7),
            max_width: Some(7),
            truncate: Truncate::Right,
            flexible: false,
        });
    }

    let rows: Vec<Vec<Cell>> = entries
        .iter()
        .map(|e| {
            let words = if e.word_count > 0 {
                format!("{} word{}", e.word_count, plural(e.word_count))
            } else {
                String::new()
            };
            let mut row = vec![
                Cell::blue(e.source.clone()),
                Cell::dim(e.title.as_deref().unwrap_or("").to_owned()),
            ];
            row.push(Cell::dim(words));
            row.push(Cell::dim(format!(
                "{} chunk{}",
                e.chunk_count,
                plural(e.chunk_count)
            )));
            if show_quality {
                row.push(Cell::dim(
                    e.avg_llm_quality_score
                        .map(|q| format!("q: {q:.2}"))
                        .unwrap_or_default(),
                ));
            }
            row
        })
        .collect();

    let mut out = format!("{total} document{}", plural(total));
    if let Some(note) = pagination_note(offset, entries.len(), total, mode) {
        w!(out, ", {note}");
    }
    out.push_str("\n\n");
    let table = format_table(&rows, &columns, "", mode.color(), mode.width());
    out.push_str(table.trim_end_matches('\n'));
    out
}

/// Format a single document with its chunk content.
pub fn format_document(
    doc: &DocDetail,
    offset: usize,
    limit: usize,
    full: bool,
    mode: OutputMode,
    preview: bool,
) -> Result<String> {
    match mode {
        OutputMode::Cli { .. } if full => Ok(format_document_full_cli(doc, mode)),
        OutputMode::Cli { .. } => Ok(format_document_cli(doc, offset, limit, mode, preview)),
        OutputMode::Mcp if full => Ok(format_document_full_mcp(doc)),
        OutputMode::Mcp => Ok(format_document_mcp(doc, offset, limit)),
        OutputMode::Json => to_json_pretty(doc),
    }
}

/// Return the display metadata fields for a document as `(label, value)` pairs.
///
/// Groups: identification -> classification -> size -> enrichment -> timestamps.
fn doc_meta_fields<'a>(
    doc: &'a DocMeta,
    chunks: &[Chunk],
    preview: bool,
) -> Vec<(&'static str, Cow<'a, str>)> {
    let mut fields = Vec::with_capacity(10);

    if let Some(title) = &doc.title {
        fields.push(("Title", Cow::Borrowed(title.as_str())));
    }
    if let Some(author) = &doc.author {
        fields.push(("Author", Cow::Borrowed(author.as_str())));
    }
    if let Some(topic) = &doc.topic {
        fields.push(("Topic", Cow::Borrowed(topic.as_str())));
    }

    fields.push(("Kind", Cow::Owned(doc.kind.to_string())));
    if let Some(ref fmt) = doc.format {
        fields.push(("Format", Cow::Borrowed(fmt.as_str())));
    }
    fields.push(("Origin", Cow::Owned(doc.origin.to_string())));
    if let Some(lang) = &doc.lang {
        fields.push(("Language", Cow::Borrowed(lang.as_str())));
    }
    if let Some(tags) = &doc.tags {
        fields.push(("Tags", Cow::Borrowed(tags.as_str())));
    }

    if !preview {
        fields.push(("Chunks", Cow::Owned(doc.chunk_count.to_string())));
        if doc.word_count > 0 {
            fields.push(("Words", Cow::Owned(doc.word_count.to_string())));
        }
    }

    if let Some(summary) = &doc.llm_summary {
        fields.push(("Summary", Cow::Borrowed(summary.as_str())));
    }
    let scores: Vec<f64> = chunks.iter().filter_map(|c| c.llm_quality_score).collect();
    if !scores.is_empty() {
        let avg = scores.iter().sum::<f64>() / scores.len() as f64;
        fields.push(("Quality", Cow::Owned(format!("{avg:.2}"))));
    }

    if let Some(created) = &doc.created_at {
        fields.push(("Created", Cow::Borrowed(created.as_str())));
    }
    fields
}

/// Format a single document for MCP output.
fn format_document_mcp(doc: &DocDetail, offset: usize, limit: usize) -> String {
    let mut header = format!("{}\n", doc.meta.source);
    for (label, value) in doc_meta_fields(&doc.meta, &doc.chunks, false) {
        wln!(header, "{label}: {value}");
    }
    format_chunks_page_mcp(&header, &doc.chunks, offset, limit)
}

/// Format a full document for MCP output.
fn format_document_full_mcp(doc: &DocDetail) -> String {
    let mut out = format!("{}\n", doc.meta.source);
    for (label, value) in doc_meta_fields(&doc.meta, &doc.chunks, false) {
        wln!(out, "{label}: {value}");
    }
    for chunk in &doc.chunks {
        out.push('\n');
        out.push_str(&clean_body(&chunk.body));
        if !chunk.body.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn build_doc_header_cli(doc: &DocDetail, mode: OutputMode, preview: bool) -> String {
    let paint = mode.painter();
    let heading = doc.meta.title.as_deref().unwrap_or(&doc.meta.source);
    let mut header = format!("{}", paint.blue(heading));
    if doc.meta.title.is_some() {
        w!(header, "\n  {}", paint.dim(&doc.meta.source));
    }
    let fields = doc_meta_fields(&doc.meta, &doc.chunks, preview);
    let pairs: Vec<(&str, &str)> = fields.iter().map(|(l, v)| (*l, &**v)).collect();
    let kv = format_kv(&pairs, "  ", mode.width());
    if !kv.is_empty() {
        header.push('\n');
        header.push_str(kv.trim_end_matches('\n'));
    }
    header
}

/// Format a document as continuous text for CLI `--full` mode.
fn format_document_full_cli(doc: &DocDetail, mode: OutputMode) -> String {
    let header = build_doc_header_cli(doc, mode, false);
    format_chunks_full_cli(&header, &doc.chunks, mode)
}

/// Format a single document for CLI output.
fn format_document_cli(
    doc: &DocDetail,
    offset: usize,
    limit: usize,
    mode: OutputMode,
    preview: bool,
) -> String {
    let header = build_doc_header_cli(doc, mode, preview);
    format_chunks_page_cli(&header, &doc.chunks, offset, limit, mode, !preview)
}
