mod documents;
mod info;
mod search;
mod topics;

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;

use serde::Serialize;

use crate::fmt::{plural, style, visible_width, write_wrapped};
use crate::types::{Chunk, DocKind, SourceType};
use crate::util::paginate;

pub use documents::{format_document, format_document_list};
pub use info::format_store_info;
pub(crate) use search::format_search_results;
pub(crate) use topics::format_topic_table;
pub(crate) use topics::{format_topic, format_topic_list};

/// JSON envelope for paginated list responses.
#[derive(Serialize)]
pub(crate) struct PagedJson<'a, T: Serialize> {
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<&'a str>,
    total: usize,
    offset: usize,
    items: &'a [T],
}

pub(crate) fn format_paged_json<T: Serialize>(
    query: Option<&str>,
    total: usize,
    offset: usize,
    items: &[T],
) -> anyhow::Result<String> {
    let wrapper = PagedJson {
        query,
        total,
        offset,
        items,
    };
    crate::fmt::to_json_pretty(&wrapper)
}

/// Controls how command output is formatted: human-readable CLI, JSON for scripting, or MCP Markdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Cli { width: usize, color: bool },
    Json,
    Mcp,
}

impl OutputMode {
    /// Terminal width for CLI mode, `usize::MAX` for unbounded modes.
    pub fn width(&self) -> usize {
        match self {
            Self::Cli { width, .. } => *width,
            _ => usize::MAX,
        }
    }

    /// Whether ANSI color output is enabled.
    pub fn color(&self) -> bool {
        match self {
            Self::Cli { color, .. } => *color,
            _ => false,
        }
    }

    /// Build a color painter from the current mode's color setting.
    pub fn painter(&self) -> style::Painter {
        style::Painter::new(self.color())
    }
}

/// Strip carriage returns and trailing whitespace from chunk body for display.
pub(crate) fn clean_body(body: &str) -> Cow<'_, str> {
    let s = if body.contains('\r') {
        Cow::Owned(body.replace('\r', ""))
    } else {
        Cow::Borrowed(body)
    };
    let trimmed = s.trim_end();
    if trimmed.len() == s.len() {
        s
    } else {
        Cow::Owned(trimmed.to_owned())
    }
}

/// Sort a frequency map by count descending and join with a custom formatter.
fn format_count_map<K>(map: &HashMap<K, u64>, fmt: impl Fn(&K, u64) -> String) -> String {
    let mut entries: Vec<_> = map.iter().map(|(k, n)| (fmt(k, *n), *n)).collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    entries
        .into_iter()
        .map(|(s, _)| s)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Format a source-type frequency map into a human-readable summary line
/// (e.g. `"42 local, 7 git"`), sorted by count descending.
pub fn format_source_types(m: &HashMap<SourceType, u64>) -> String {
    format_count_map(m, |t, n| format!("{n} {t}"))
}

/// Format a document-kind frequency map into a summary line
/// (e.g. `"10 documents, 3 snippets"`), sorted by count descending.
pub fn format_kinds(m: &HashMap<DocKind, u64>) -> String {
    format_count_map(m, |k, n| format!("{n} {k}{}", plural(n)))
}

/// Format a file-format frequency map into a summary line
/// (e.g. `"markdown (30), rst (5)"`), sorted by count descending.
pub fn format_formats(m: &HashMap<String, u64>) -> String {
    format_count_map(m, |f, n| format!("{f} ({n})"))
}

/// Format a language frequency map into a summary line.
pub fn format_lang_summary(langs: &HashMap<String, u64>) -> Option<String> {
    if langs.is_empty() {
        return None;
    }
    let mut lv: Vec<_> = langs.iter().collect();
    lv.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let top: Vec<String> = lv
        .iter()
        .take(5)
        .map(|(l, n)| format!("{l} ({n})"))
        .collect();
    let suffix = if lv.len() > 5 {
        format!(", +{} more", lv.len() - 5)
    } else {
        String::new()
    };
    Some(format!("{}{suffix}", top.join(", ")))
}

/// Return a pagination hint string when there are more items beyond the current page,
/// or `None` when the current page already covers all remaining items.
/// The hint text is mode-specific: CLI uses `--offset N`, MCP uses `offset=N`.
/// JSON mode always returns `None` because pagination is encoded in the JSON envelope.
pub(crate) fn pagination_note(
    offset: usize,
    page_len: usize,
    total: usize,
    mode: OutputMode,
) -> Option<String> {
    if total <= offset + page_len {
        return None;
    }
    let next = offset + page_len;
    let start = offset + 1;
    let end = offset + page_len;
    let range = if start == end {
        format!("{start}")
    } else {
        format!("{start}-{end}")
    };
    match mode {
        OutputMode::Mcp => Some(format!("{range}/{total}. Use offset={next} to see more.")),
        OutputMode::Cli { .. } => {
            Some(format!("{range}/{total}. Use --offset {next} to see more."))
        }
        OutputMode::Json => None,
    }
}

/// Format a paginated slice of chunks for MCP output.
/// Each chunk body is rendered verbatim with an optional `[section]` label.
/// A pagination hint is appended when more chunks remain beyond the current page.
pub(crate) fn format_chunks_page_mcp(
    header: &str,
    chunks: &[Chunk],
    offset: usize,
    limit: usize,
) -> String {
    let page_len = paginate(chunks, offset, limit).len();
    let mut out = format_chunks_page(header, chunks, offset, limit, |chunk, _index, out| {
        out.push('\n');
        if let Some(ref section) = chunk.section {
            wln!(out, "[{section}]");
        }
        let body = clean_body(&chunk.body);
        out.push_str(&body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
    });
    if let Some(note) = pagination_note(offset, page_len, chunks.len(), OutputMode::Mcp) {
        out.push('\n');
        out.push_str(&note);
    }
    out
}

/// Format a paginated slice of chunks for CLI output.
/// Each chunk is prefixed with a right-aligned index `[N]` and its section heading.
/// Body text is word-wrapped and indented to align with the heading. A quality
/// score is right-aligned on the heading line when present.
pub(crate) fn format_chunks_page_cli(
    header: &str,
    chunks: &[Chunk],
    offset: usize,
    limit: usize,
    mode: OutputMode,
    show_pagination: bool,
) -> String {
    let page_len = paginate(chunks, offset, limit).len();
    let mut header_line = header.to_owned();
    if show_pagination && let Some(note) = pagination_note(offset, page_len, chunks.len(), mode) {
        let total = chunks.len();
        w!(header_line, "\n\n{total} chunk{}, {note}", plural(total));
    }
    header_line.push('\n');
    // Indent body to align with text after `[N] `: brackets + space + digits.
    let index_width = (offset + page_len).to_string().len();
    let body_indent = " ".repeat(index_width + 3);
    format_chunks_page(&header_line, chunks, offset, limit, |chunk, index, out| {
        let paint = mode.painter();
        let heading = match chunk.section.as_deref() {
            Some(s) if !s.is_empty() => s.to_owned(),
            _ => format!("Chunk {index}"),
        };
        let idx = index.to_string();
        let pad = index_width - idx.len();
        out.push('\n');
        let left = format!(
            "{}[{}] {}",
            " ".repeat(pad),
            paint.dim(&idx),
            paint.blue(&heading),
        );
        if let Some(q) = chunk.llm_quality_score {
            let right = paint.dim(&format!("quality: {q:.2}")).to_string();
            let used = visible_width(&left) + visible_width(&right);
            let term_w = mode.width();
            if used <= term_w {
                wln!(out, "{left}{}{right}", " ".repeat(term_w - used));
            } else {
                wln!(out, "{left}");
            }
        } else {
            wln!(out, "{left}");
        }
        let body = clean_body(&chunk.body);
        write_wrapped(out, &body, &body_indent, mode.width());
    })
}

/// Format all chunks as continuous text for `--full` mode.
pub(crate) fn format_chunks_full_cli(header: &str, chunks: &[Chunk], mode: OutputMode) -> String {
    let mut out = header.to_owned();
    out.push_str("\n\n---\n");
    for chunk in chunks {
        out.push('\n');
        write_wrapped(&mut out, &clean_body(&chunk.body), "", mode.width());
    }
    out
}

/// Paginate chunks, deduplicate by chunk_id, and render each via `format_chunk`.
fn format_chunks_page(
    header: &str,
    chunks: &[Chunk],
    offset: usize,
    limit: usize,
    format_chunk: impl Fn(&Chunk, usize, &mut String),
) -> String {
    let page = paginate(chunks, offset, limit);
    let mut out = header.to_owned();
    // Deduplicate by chunk_id within the page to avoid showing the same chunk
    // twice when the caller passes a list that already contains duplicates
    // (e.g. from merged result sets).
    let mut seen = std::collections::HashSet::new();
    let mut rendered = 0;
    for chunk in page {
        if seen.insert(chunk.chunk_id.as_str()) {
            rendered += 1;
            format_chunk(chunk, offset + rendered, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::store::TopicStat;
    use crate::store::test_helpers::{test_search_hit, test_search_hit_bare};

    #[test]
    fn search_results_formatting() {
        let hits = vec![test_search_hit_bare("/docs/bare.md", "Body text.", 1)];
        let out = super::format_search_results(&hits, "bare", 1, 0, OutputMode::Mcp).unwrap();
        assert!(
            !out.contains(" | /docs"),
            "source should not be prefixed with ` | ` when topic and section are absent"
        );
        assert!(
            out.contains("/docs/bare.md"),
            "source path should still appear"
        );

        let hits = vec![test_search_hit("/docs/test.md", "Some body text here.", 1)];
        for (mode, expected) in [
            (OutputMode::Mcp, "## Test Doc"),
            (
                OutputMode::Cli {
                    width: 120,
                    color: false,
                },
                "Test Doc",
            ),
            (OutputMode::Json, "\"chunk_id\""),
        ] {
            let out = super::format_search_results(&hits, "test", 1, 0, mode).unwrap();
            assert!(
                out.contains(expected),
                "{mode:?} output should contain {expected:?}, got: {out}"
            );
        }

        let two_hits = vec![
            test_search_hit("/docs/test.md", "First result body text.", 1),
            test_search_hit("/docs/test.md", "Second result body text.", 2),
        ];
        let out = super::format_search_results(
            &two_hits,
            "test",
            2,
            0,
            OutputMode::Cli {
                width: 120,
                color: false,
            },
        )
        .unwrap();
        assert_eq!(
            out.matches("/docs/test.md").count(),
            1,
            "source path should appear only once for consecutive same-source hits"
        );

        let long_body = "word ".repeat(100);
        let long_hits = vec![test_search_hit("/docs/test.md", &long_body, 1)];
        let out = super::format_search_results(
            &long_hits,
            "test",
            1,
            0,
            OutputMode::Cli {
                width: 120,
                color: false,
            },
        )
        .unwrap();
        assert!(
            out.contains("..."),
            "long body should be truncated with ellipsis"
        );

        let out = super::format_search_results(
            &[],
            "test",
            0,
            0,
            OutputMode::Cli {
                width: 120,
                color: false,
            },
        )
        .unwrap();
        assert!(
            out.contains("No results"),
            "empty CLI results should say No results"
        );
        let out = super::format_search_results(&[], "test", 0, 0, OutputMode::Mcp).unwrap();
        assert!(
            out.contains("No results"),
            "empty MCP results should say No results"
        );
    }

    #[test]
    fn pagination_note_cases() {
        let cli = OutputMode::Cli {
            width: 120,
            color: false,
        };

        // Offset hints: CLI uses --offset N, MCP uses offset=N.
        for (mode, expected) in [(cli, "--offset 10"), (OutputMode::Mcp, "offset=10")] {
            let note = pagination_note(0, 10, 25, mode);
            assert!(
                note.is_some(),
                "{mode:?} should return a note when more pages remain"
            );
            assert!(
                note.unwrap().contains(expected),
                "{mode:?} note should include {expected:?}"
            );
        }

        // Last page returns None when offset + page_len == total.
        assert!(
            pagination_note(0, 10, 10, cli).is_none(),
            "should return None when page covers all items"
        );

        // Range formatting: single item shows "13/33", multiple shows "13-15/33".
        let note = pagination_note(12, 1, 33, cli).unwrap();
        assert!(
            note.starts_with("13/33"),
            "single item should show '13/33', not '13-13/33'; got: {note}"
        );
        let note = pagination_note(12, 3, 33, cli).unwrap();
        assert!(
            note.starts_with("13-15/33"),
            "multi item should show '13-15/33'; got: {note}"
        );
    }

    #[test]
    fn lang_summary_cases() {
        let empty_langs: HashMap<String, u64> = HashMap::new();
        assert!(
            format_lang_summary(&empty_langs).is_none(),
            "empty language map should return None"
        );

        let mut langs = HashMap::new();
        langs.insert("en".to_owned(), 10);
        langs.insert("de".to_owned(), 3);
        let summary = format_lang_summary(&langs).unwrap();
        assert!(
            summary.contains("en (10)"),
            "summary should include en count"
        );
        assert!(
            summary.contains("de (3)"),
            "summary should include de count"
        );
    }

    #[test]
    fn topic_list_cases() {
        let stats = vec![TopicStat {
            name: "docs".to_owned(),
            doc_count: 5,
            chunk_count: 42,
            word_count: 1200,
        }];
        let out = format_topic_list(&stats, 1, 0, OutputMode::Mcp).unwrap();
        assert!(out.contains("docs:"), "should contain topic name: {out}");
        assert!(out.contains("5 docs"), "should contain doc count: {out}");
        assert!(
            out.contains("1200 words"),
            "should contain word count: {out}"
        );
        assert!(
            out.contains("42 chunks"),
            "should contain chunk count: {out}"
        );

        let out = format_topic_list(
            &[],
            0,
            0,
            OutputMode::Cli {
                width: 120,
                color: false,
            },
        )
        .unwrap();
        assert!(
            out.contains("No topics"),
            "empty topic list should say No topics"
        );
    }
}
