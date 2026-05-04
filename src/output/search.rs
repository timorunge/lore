use std::borrow::Cow;
use std::fmt::Write as _;

use anyhow::Result;

use crate::fmt::style;
use crate::fmt::table::{Align, Cell, Column, Truncate, format_table};
use crate::fmt::{plural, truncate_body, write_wrapped};
use crate::output::{OutputMode, clean_body, format_paged_json, pagination_note};
use crate::store::SearchHit;

/// Format search results for display or JSON output.
pub(crate) fn format_search_results(
    results: &[SearchHit],
    query: &str,
    total: usize,
    offset: usize,
    mode: OutputMode,
) -> Result<String> {
    match mode {
        OutputMode::Mcp => Ok(format_search_results_mcp(results, query, total, offset)),
        OutputMode::Cli { .. } => Ok(format_search_results_cli(
            results, query, total, offset, mode,
        )),
        OutputMode::Json => format_paged_json(Some(query), total, offset, results),
    }
}

/// Format search results for MCP output.
fn format_search_results_mcp(
    results: &[SearchHit],
    query: &str,
    total: usize,
    offset: usize,
) -> String {
    if results.is_empty() {
        return format!(
            "No results for \"{query}\". Try broadening the query, removing filters, \
             or use lore_list_topics to browse topics."
        );
    }
    let mut out = String::with_capacity(results.len() * 512);
    wln!(out, "{total} result{} for \"{query}\"", plural(total));
    for hit in results {
        let chunk = &hit.chunk;
        let title_hl = hit.highlight_ranges(|fh| &fh.title);
        let heading_raw = chunk.title.as_deref().unwrap_or(&chunk.source);
        let heading = if !title_hl.is_empty() && chunk.title.is_some() {
            apply_md_highlights(heading_raw, title_hl)
        } else {
            heading_raw.to_owned()
        };
        w!(out, "\n## {heading}");
        if let Some(score) = hit.score {
            w!(out, " ({score:.2})");
        }
        out.push('\n');

        let section_hl = hit.highlight_ranges(|fh| &fh.section);
        let mut meta_parts: Vec<String> = Vec::new();
        if let Some(l) = topic_section_label(chunk.topic.as_deref(), chunk.section.as_deref()) {
            if section_hl.is_empty() {
                meta_parts.push(l.into_owned());
            } else if let Some(sec) = chunk.section.as_deref() {
                let highlighted_sec = apply_md_highlights(sec, section_hl);
                let label = match chunk.topic.as_deref() {
                    Some(t) => format!("{t} > {highlighted_sec}"),
                    None => highlighted_sec,
                };
                meta_parts.push(label);
            } else {
                meta_parts.push(l.into_owned());
            }
        }
        if let Some(total) = hit.chunk_total {
            meta_parts.push(format!(
                "{} #{}/{total}",
                chunk.source,
                chunk.chunk_index + 1
            ));
        } else {
            meta_parts.push(format!("{} #{}", chunk.source, chunk.chunk_index + 1));
        }
        if let Some(q) = chunk.llm_quality_score {
            meta_parts.push(format!("q:{q:.2}"));
        }
        wln!(out, "{}", meta_parts.join(" | "));

        if let Some(ref snip) = hit.snippet {
            out.push_str(&apply_md_highlights(&snip.text, &snip.highlights));
        } else {
            out.push_str(&clean_body(&chunk.body));
        }
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    if let Some(note) = pagination_note(offset, results.len(), total, OutputMode::Mcp) {
        w!(out, "\n{note}");
    }
    out
}

/// Format search results for CLI output.
fn format_search_results_cli(
    results: &[SearchHit],
    query: &str,
    total: usize,
    offset: usize,
    mode: OutputMode,
) -> String {
    let paint = mode.painter();
    let color = mode.color();
    if results.is_empty() {
        return format!(
            "No results for \"{query}\". Try broadening the query, removing filters, or use `lore topics` to browse topics."
        );
    }

    let mut out = format!("{total} result{} for \"{query}\"", plural(total));
    // Reserve roughly 512 bytes per result up-front to reduce reallocations.
    out.reserve(results.len() * 512);
    if let Some(note) = pagination_note(offset, results.len(), total, mode) {
        w!(out, ", {note}");
    }
    out.push('\n');

    let meta_cols = [Column {
        align: Align::Left,
        min_width: None,
        max_width: None,
        truncate: Truncate::Left,
        flexible: true,
    }];

    let heading_cols = [
        Column {
            align: Align::Left,
            min_width: None,
            max_width: None,
            truncate: Truncate::Right,
            flexible: true,
        },
        Column {
            align: Align::Right,
            min_width: None,
            max_width: Some(24),
            truncate: Truncate::Right,
            flexible: false,
        },
    ];

    let rank_width = results
        .iter()
        .map(|h| h.rank.to_string().len())
        .max()
        .unwrap_or(1);
    let content_indent = " ".repeat(rank_width + 3); // [N] + space

    let mut prev_source: Option<&str> = None;
    for hit in results {
        let chunk = &hit.chunk;
        let same_source = prev_source == Some(&chunk.source_id);
        if !same_source {
            prev_source = Some(&chunk.source_id);
        }
        let group_indent = if same_source { &content_indent } else { "" };

        let rank = hit.rank.to_string();
        let pad = rank_width - rank.len();
        let prefix = format!("{group_indent}{}[{}] ", " ".repeat(pad), paint.dim(&rank));

        let heading = chunk.title.as_deref().unwrap_or(&chunk.source);
        let title_hl = hit.highlight_ranges(|fh| &fh.title);
        let heading_cell = if color && !title_hl.is_empty() && chunk.title.is_some() {
            Cell::plain(apply_ansi_highlights(
                heading,
                title_hl,
                paint,
                Some(style::Painter::blue),
            ))
        } else {
            Cell::blue(heading.to_owned())
        };

        let score_str = {
            let mut buf = String::with_capacity(32);
            buf.push('(');
            if let Some(s) = hit.score {
                write!(buf, "{s:.2}").unwrap();
            }
            if let Some(q) = chunk.llm_quality_score {
                if buf.len() > 1 {
                    buf.push_str(", ");
                }
                write!(buf, "q:{q:.2}").unwrap();
            }
            if buf.len() > 1 {
                buf.push_str(", ");
            }
            if let Some(total) = hit.chunk_total {
                write!(buf, "#{}/{total}", chunk.chunk_index + 1).unwrap();
            } else {
                write!(buf, "#{}", chunk.chunk_index + 1).unwrap();
            }
            buf.push(')');
            buf
        };

        let rows = [vec![heading_cell, Cell::dim(score_str)]];
        let table = format_table(&rows, &heading_cols, &prefix, color, mode.width());
        w!(out, "\n{}", table.trim_end());
        out.push('\n');

        let sub_indent = format!("{group_indent}{content_indent}");

        if chunk.title.is_some() && !same_source {
            let rows = [vec![Cell::dim(chunk.source.to_string())]];
            let table = format_table(&rows, &meta_cols, &sub_indent, color, mode.width());
            out.push_str(table.trim_end());
            out.push('\n');
        }

        let section_hl = hit.highlight_ranges(|fh| &fh.section);
        if let Some(label) = topic_section_label(chunk.topic.as_deref(), chunk.section.as_deref()) {
            let label_cell = if color && !section_hl.is_empty() {
                Cell::plain(highlight_topic_section(
                    chunk.topic.as_deref(),
                    chunk.section.as_deref(),
                    section_hl,
                    paint,
                ))
            } else {
                Cell::dim(label)
            };
            let rows = [vec![label_cell]];
            let table = format_table(&rows, &meta_cols, &sub_indent, color, mode.width());
            out.push_str(table.trim_end());
            out.push('\n');
        }

        if let Some(ref snip) = hit.snippet {
            if color {
                let display = apply_ansi_highlights(&snip.text, &snip.highlights, paint, None);
                write_wrapped(&mut out, &display, &sub_indent, mode.width());
            } else {
                write_wrapped(&mut out, &snip.text, &sub_indent, mode.width());
            }
        } else {
            let body = clean_body(&chunk.body);
            let snippet_text = truncate_body(&body, 300);
            write_wrapped(&mut out, snippet_text, &sub_indent, mode.width());
            if snippet_text.chars().count() < body.chars().count() {
                if out.ends_with('\n') {
                    out.pop();
                }
                wln!(out, " {}", paint.dim("(...)"));
            }
        }
    }

    out
}

/// Apply highlighting to matched byte ranges using caller-supplied wrap functions.
///
/// `wrap_gap` styles the non-highlighted text between matches; `wrap_hit` styles
/// the matched span itself.  Invalid or out-of-order byte ranges are silently
/// skipped so a bad highlight index never panics.
fn apply_highlights(
    text: &str,
    highlights: &[(usize, usize)],
    wrap_gap: impl Fn(&str) -> String,
    wrap_hit: impl Fn(&str) -> String,
) -> String {
    let mut result = String::with_capacity(text.len() + highlights.len() * 10);
    let mut pos = 0;
    for &(start, end) in highlights {
        if start < pos
            || end > text.len()
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
            || !text.is_char_boundary(pos)
        {
            continue;
        }
        if start > pos {
            result.push_str(&wrap_gap(&text[pos..start]));
        }
        result.push_str(&wrap_hit(&text[start..end]));
        pos = end;
    }
    if pos < text.len() {
        result.push_str(&wrap_gap(&text[pos..]));
    }
    result
}

/// Apply bold yellow ANSI highlighting to matched byte ranges.
/// Non-highlighted text uses the optional `base` style (e.g. blue for titles).
fn apply_ansi_highlights(
    text: &str,
    highlights: &[(usize, usize)],
    paint: style::Painter,
    base: Option<fn(style::Painter, &str) -> style::Styled<'_>>,
) -> String {
    apply_highlights(
        text,
        highlights,
        |gap| match base {
            Some(style_fn) => style_fn(paint, gap).to_string(),
            None => gap.to_owned(),
        },
        |hit| paint.bold_yellow(hit).to_string(),
    )
}

/// Wrap highlighted byte ranges in Markdown bold markers.
fn apply_md_highlights(text: &str, highlights: &[(usize, usize)]) -> String {
    apply_highlights(text, highlights, str::to_owned, |hit| format!("**{hit}**"))
}

/// Build a topic > section label with ANSI highlights on the section portion.
fn highlight_topic_section(
    topic: Option<&str>,
    section: Option<&str>,
    section_hl: &[(usize, usize)],
    paint: style::Painter,
) -> String {
    let highlighted_sec = section.map(|s| apply_ansi_highlights(s, section_hl, paint, None));
    topic_section_label(topic, highlighted_sec.as_deref())
        .map(Cow::into_owned)
        .unwrap_or_default()
}

/// Combine topic and section into a display label.
fn topic_section_label<'a>(
    topic: Option<&'a str>,
    section: Option<&'a str>,
) -> Option<Cow<'a, str>> {
    match (topic, section) {
        (Some(t), Some(s)) => Some(Cow::Owned(format!("{t} > {s}"))),
        (Some(t), None) => Some(Cow::Borrowed(t)),
        (None, Some(s)) => Some(Cow::Borrowed(s)),
        (None, None) => None,
    }
}
