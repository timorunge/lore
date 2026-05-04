use std::fmt::Write as _;

use anyhow::Result;

use crate::fmt::table::{Align, Cell, Column, Truncate, format_table};
use crate::fmt::{plural, to_json_pretty};
use crate::output::{
    OutputMode, format_chunks_page_cli, format_chunks_page_mcp, format_paged_json, pagination_note,
};
use crate::store::{TopicResult, TopicStat};

/// Format a paginated list of topic statistics.
pub(crate) fn format_topic_list(
    stats: &[TopicStat],
    total: usize,
    offset: usize,
    mode: OutputMode,
) -> Result<String> {
    match mode {
        OutputMode::Mcp => {
            if stats.is_empty() {
                return Ok(
                    "No topics found. Try removing filters or use lore_search to search across all documents.".to_owned(),
                );
            }
            let mut out = format!("{total} topic{}\n", plural(total));
            for s in stats {
                w!(
                    out,
                    "- {}: {} doc{}",
                    s.name,
                    s.doc_count,
                    plural(s.doc_count)
                );
                if s.word_count > 0 {
                    w!(out, ", {} words", s.word_count);
                }
                wln!(out, ", {} chunks", s.chunk_count);
            }
            if let Some(note) = pagination_note(offset, stats.len(), total, OutputMode::Mcp) {
                w!(out, "\n{note}");
            }
            Ok(out)
        }
        OutputMode::Cli { .. } => {
            if stats.is_empty() {
                return Ok("No topics found. Try removing filters or use `lore search` to search across all documents.".to_owned());
            }
            let mut out = format!("{total} topic{}", plural(total));
            if let Some(note) = pagination_note(offset, stats.len(), total, mode) {
                w!(out, ", {note}");
            }
            out.push_str("\n\n");
            let table = format_topic_table(stats, mode);
            out.push_str(table.trim_end_matches('\n'));
            Ok(out)
        }
        OutputMode::Json => format_paged_json(None, total, offset, stats),
    }
}

/// Render topic stats as a table (name, docs, words, chunks).
pub(crate) fn format_topic_table(stats: &[TopicStat], mode: OutputMode) -> String {
    let color = mode.color();
    let width = mode.width();
    let columns = [
        Column {
            align: Align::Left,
            min_width: Some(10),
            max_width: None,
            truncate: Truncate::Right,
            flexible: false,
        },
        Column {
            align: Align::Right,
            min_width: None,
            max_width: Some(10),
            truncate: Truncate::Right,
            flexible: false,
        },
        Column {
            align: Align::Right,
            min_width: Some(10),
            max_width: Some(13),
            truncate: Truncate::Right,
            flexible: false,
        },
        Column {
            align: Align::Right,
            min_width: None,
            max_width: Some(12),
            truncate: Truncate::Right,
            flexible: false,
        },
    ];
    let rows: Vec<Vec<Cell>> = stats
        .iter()
        .map(|s| {
            let words = if s.word_count > 0 {
                format!("{} word{}", s.word_count, plural(s.word_count))
            } else {
                String::new()
            };
            vec![
                Cell::blue(s.name.clone()),
                Cell::dim(format!("{} doc{}", s.doc_count, plural(s.doc_count))),
                Cell::dim(words),
                Cell::dim(format!("{} chunk{}", s.chunk_count, plural(s.chunk_count))),
            ]
        })
        .collect();
    format_table(&rows, &columns, "", color, width)
}

/// Format a single topic with its chunk content.
pub(crate) fn format_topic(
    topic: &TopicResult,
    offset: usize,
    limit: usize,
    mode: OutputMode,
) -> Result<String> {
    match mode {
        OutputMode::Mcp => {
            let header = format!(
                "{}\n{} chunks, {} sources: {}\n",
                topic.name,
                topic.chunk_count,
                topic.source_paths.len(),
                topic.source_paths.join(", "),
            );
            Ok(format_chunks_page_mcp(
                &header,
                &topic.chunks,
                offset,
                limit,
            ))
        }
        OutputMode::Cli { .. } => {
            let mut header = topic.name.clone();
            w!(header, "\n  chunks:  {}", topic.chunk_count);
            w!(header, "\n  sources: {}", topic.source_paths.join(", "));
            Ok(format_chunks_page_cli(
                &header,
                &topic.chunks,
                offset,
                limit,
                mode,
                true,
            ))
        }
        OutputMode::Json => to_json_pretty(topic),
    }
}
