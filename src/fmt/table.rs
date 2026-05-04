use std::fmt::Write;

use crate::util::{truncate_chars, truncate_left_chars};

/// A table cell with plain text and an optional ANSI style code.
pub(crate) struct Cell {
    pub text: String,
    /// ANSI SGR code, e.g. "34" for blue, "2" for dim. Empty = no style.
    pub style: &'static str,
}

impl Cell {
    /// Create an unstyled cell.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: "",
        }
    }

    /// Create a cell rendered in blue.
    pub fn blue(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: "34",
        }
    }

    /// Create a dimmed cell.
    pub fn dim(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: "2",
        }
    }
}

/// Overflow handling when cell text exceeds column width.
#[derive(Clone, Copy)]
pub(crate) enum Truncate {
    /// Show the tail with `".."` prefix -- for file paths.
    Left,
    /// Clip from the right -- for names and text.
    Right,
    /// Word-wrap onto continuation lines (other columns padded with blanks).
    Wrap,
}

/// Text alignment within a column.
#[derive(Clone, Copy)]
pub(crate) enum Align {
    Left,
    Right,
}

/// Layout descriptor for a single table column.
pub(crate) struct Column {
    pub align: Align,
    /// Floor on column width -- never shrink below this.
    pub min_width: Option<usize>,
    /// Hard cap on column width before terminal shrinking.
    pub max_width: Option<usize>,
    pub truncate: Truncate,
    /// Whether this column can shrink/grow to fit the terminal.
    pub flexible: bool,
}

/// Format rows into aligned columns.
pub(crate) fn format_table(
    rows: &[Vec<Cell>],
    columns: &[Column],
    indent: &str,
    color: bool,
    width: usize,
) -> String {
    if rows.is_empty() || columns.is_empty() {
        return String::new();
    }
    let ncols = columns.len();

    // Step 1: natural widths from data
    let natural_widths: Vec<usize> = (0..ncols)
        .map(|i| {
            rows.iter()
                .map(|r| r.get(i).map_or(0, |c| c.text.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();

    // Step 2: apply min/max width bounds
    let mut col_widths: Vec<usize> = natural_widths
        .iter()
        .enumerate()
        .map(|(i, &w)| {
            let w = match columns[i].max_width {
                Some(max) => w.min(max),
                None => w,
            };
            match columns[i].min_width {
                Some(min) => w.max(min),
                None => w,
            }
        })
        .collect();

    // Step 3: fit to terminal -- shrink or grow flexible columns
    let gap = 2usize;
    let total_gaps = ncols.saturating_sub(1) * gap;
    let total: usize = indent.len() + col_widths.iter().sum::<usize>() + total_gaps;
    let term_w = width;
    if total > term_w {
        let mut overflow = total - term_w;
        // Distribute overflow across all flexible columns proportionally.
        let flex: Vec<usize> = columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.flexible)
            .map(|(i, _)| i)
            .collect();
        let flex_total: usize = flex.iter().map(|&i| col_widths[i]).sum();
        for &i in &flex {
            if overflow == 0 {
                break;
            }
            let floor = columns[i].min_width.unwrap_or(12);
            // Shrink proportionally to column's share of total flexible width.
            let share = if flex_total > 0 {
                (col_widths[i] * (total - term_w)).div_ceil(flex_total)
            } else {
                overflow
            };
            let shrink = share.min(col_widths[i].saturating_sub(floor)).min(overflow);
            col_widths[i] -= shrink;
            overflow -= shrink;
        }
    } else if total < term_w && term_w < usize::MAX {
        let mut spare = term_w - total;

        // Phase 1: grow each flexible column toward its natural width.
        for (i, col) in columns.iter().enumerate() {
            if spare == 0 {
                break;
            }
            if col.flexible {
                let cap = col.max_width.unwrap_or(usize::MAX);
                let target = natural_widths[i].min(cap);
                let room = target.saturating_sub(col_widths[i]);
                let give = room.min(spare);
                col_widths[i] += give;
                spare -= give;
            }
        }

        // Phase 2: distribute remaining space among uncapped flexible columns.
        if spare > 0 {
            let uncapped: Vec<usize> = columns
                .iter()
                .enumerate()
                .filter(|(_, c)| c.flexible && c.max_width.is_none())
                .map(|(i, _)| i)
                .collect();
            if !uncapped.is_empty() {
                let per_col = spare / uncapped.len();
                let mut remainder = spare % uncapped.len();
                for &i in &uncapped {
                    let extra = if remainder > 0 {
                        remainder -= 1;
                        1
                    } else {
                        0
                    };
                    col_widths[i] += per_col + extra;
                }
            }
        }
    }

    // Step 4: split each cell into visual lines (wrapping or truncating).
    let mut out = String::new();
    for row in rows {
        let cell_lines: Vec<(Vec<String>, &str)> = columns
            .iter()
            .enumerate()
            .map(|(i, col)| {
                let cell = row.get(i);
                let text = cell.map_or("", |c| c.text.as_str());
                let style = cell.map_or("", |c| c.style);
                let w = col_widths[i];
                // chars().count() guards against splitting at a non-char boundary.
                let lines = if text.chars().count() <= w {
                    vec![text.to_owned()]
                } else {
                    match col.truncate {
                        Truncate::Left => vec![truncate_left_chars(text, w).into_owned()],
                        Truncate::Right => vec![truncate_chars(text, w).to_owned()],
                        Truncate::Wrap => wrap_cell(text, w),
                    }
                };
                (lines, style)
            })
            .collect();

        let max_lines = cell_lines.iter().map(|(ls, _)| ls.len()).max().unwrap_or(1);

        for line_idx in 0..max_lines {
            out.push_str(indent);
            for (i, col) in columns.iter().enumerate() {
                let (ref lines, style) = cell_lines[i];
                let text = lines.get(line_idx).map_or("", String::as_str);
                let w = col_widths[i];

                let is_last = i == ncols - 1;
                let padded = if is_last && matches!(col.align, Align::Left) {
                    text.to_owned()
                } else {
                    match col.align {
                        Align::Left => format!("{text:<w$}"),
                        Align::Right => {
                            if line_idx > 0 {
                                format!("{text:<w$}")
                            } else {
                                format!("{text:>w$}")
                            }
                        }
                    }
                };

                if color && !style.is_empty() && !text.is_empty() {
                    w!(out, "\x1b[{style}m{padded}\x1b[0m");
                } else {
                    out.push_str(&padded);
                }

                if !is_last {
                    out.push_str("  ");
                }
            }
            out.push('\n');
        }
    }
    out
}

/// Word-wrap text into lines that fit within `width` characters.
fn wrap_cell(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_owned()];
    }
    let mut lines = Vec::new();
    let mut pos = 0;
    while pos < text.len() {
        let remaining = &text[pos..];
        if remaining.chars().count() <= width {
            lines.push(remaining.to_owned());
            break;
        }
        // Find a char-boundary at `width` chars.
        let mut end = remaining
            .char_indices()
            .nth(width)
            .map_or(remaining.len(), |(i, _)| i);
        let chunk = &remaining[..end];
        // Prefer breaking after ", " or at a space.
        let break_at = chunk
            .rfind(", ")
            .map(|i| i + 2)
            .or_else(|| chunk.rfind(' ').map(|i| i + 1))
            .unwrap_or(end);
        end = break_at;
        lines.push(remaining[..end].trim_end().to_owned());
        pos += end;
        // Skip leading whitespace on continuation (char-aware to handle multi-byte UTF-8).
        while pos < text.len() {
            // pos < text.len() guarantees at least one char remains.
            let ch = text[pos..]
                .chars()
                .next()
                .expect("pos < text.len() guarantees a char");
            if ch.is_whitespace() {
                pos += ch.len_utf8();
            } else {
                break;
            }
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Format label-value pairs as a two-column table with wrapping values.
pub(crate) fn format_kv(pairs: &[(&str, &str)], indent: &str, width: usize) -> String {
    if pairs.is_empty() {
        return String::new();
    }
    let lw = pairs.iter().map(|(l, _)| l.len() + 1).max().unwrap_or(0);
    let columns = [
        Column {
            align: Align::Left,
            min_width: Some(lw),
            max_width: Some(lw),
            truncate: Truncate::Right,
            flexible: false,
        },
        Column {
            align: Align::Left,
            min_width: None,
            max_width: None,
            truncate: Truncate::Wrap,
            flexible: true,
        },
    ];
    let rows: Vec<Vec<Cell>> = pairs
        .iter()
        .map(|(label, value)| {
            vec![
                Cell::plain(format!("{label}:")),
                Cell::plain((*value).to_owned()),
            ]
        })
        .collect();
    format_table(&rows, &columns, indent, false, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_columns_align() {
        let columns = [
            Column {
                align: Align::Left,
                min_width: None,
                max_width: Some(10),
                truncate: Truncate::Left,
                flexible: true,
            },
            Column {
                align: Align::Left,
                min_width: None,
                max_width: None,
                truncate: Truncate::Right,
                flexible: false,
            },
        ];
        let rows = vec![
            vec![Cell::plain("short"), Cell::plain("val1")],
            vec![Cell::plain("longer_src"), Cell::plain("v2")],
        ];
        let out = format_table(&rows, &columns, "  ", false, 120);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        let val1_pos = lines[0].find("val1").unwrap();
        let v2_pos = lines[1].find("v2").unwrap();
        assert_eq!(val1_pos, v2_pos, "columns should align");
    }

    #[test]
    fn table_left_truncation() {
        let columns = [Column {
            align: Align::Left,
            min_width: None,
            max_width: Some(8),
            truncate: Truncate::Left,
            flexible: false,
        }];
        let rows = vec![vec![Cell::plain("very/long/path/file.rs")]];
        let out = format_table(&rows, &columns, "", false, 120);
        assert!(out.starts_with(".."), "should left-truncate with '..'");
        assert!(out.trim().len() <= 8);
    }

    #[test]
    fn table_color_toggle() {
        let columns = [
            Column {
                align: Align::Left,
                min_width: None,
                max_width: None,
                truncate: Truncate::Right,
                flexible: false,
            },
            Column {
                align: Align::Left,
                min_width: None,
                max_width: None,
                truncate: Truncate::Right,
                flexible: false,
            },
        ];
        let rows = vec![vec![Cell::blue("hello"), Cell::dim("world")]];

        let on = format_table(&rows, &columns, "", true, 120);
        assert!(
            on.contains("\x1b["),
            "color mode should produce ANSI escapes"
        );
        assert!(on.contains("hello"));
        assert!(on.contains("world"));

        let off = format_table(&rows, &columns, "", false, 120);
        assert!(!off.contains("\x1b["), "no ANSI when color is off");
        assert!(off.contains("hello"));
        assert!(off.contains("world"));
    }

    #[test]
    fn table_word_wrap() {
        let columns = [
            Column {
                align: Align::Left,
                min_width: None,
                max_width: Some(10),
                truncate: Truncate::Right,
                flexible: false,
            },
            Column {
                align: Align::Left,
                min_width: None,
                max_width: Some(20),
                truncate: Truncate::Wrap,
                flexible: false,
            },
        ];
        let rows = vec![vec![
            Cell::plain("source"),
            Cell::plain("a very long title that wraps around"),
        ]];
        let out = format_table(&rows, &columns, "", false, 120);
        let lines: Vec<&str> = out.lines().collect();
        assert!(
            lines.len() > 1,
            "wrapped cell should produce multiple lines, got: {out:?}"
        );
        assert!(lines[0].contains("source"));
        // col1 (10) + gap (2) = 12 chars before the wrapped column starts.
        // Continuation lines must be indented by the same 12 chars.
        let value_start = lines[0].find("a very").unwrap();
        for line in &lines[1..] {
            let trimmed = line.trim_start();
            let indent_len = line.len() - trimmed.len();
            assert_eq!(
                indent_len, value_start,
                "continuation indent mismatch: {line:?}"
            );
        }
    }

    #[test]
    fn wrap_cell_bounds() {
        let long = "alpha, bravo, charlie, delta, echo, foxtrot, golf, hotel, india, juliet, kilo, lima, mike, november, oscar, papa, quebec, romeo";
        let cell_lines = wrap_cell(long, 40);
        assert!(
            cell_lines.len() > 1,
            "long value should wrap to multiple lines, got: {cell_lines:?}"
        );
        for line in &cell_lines {
            assert!(
                line.chars().count() <= 40,
                "wrapped line too wide: {line:?}"
            );
        }
        let joined: String = cell_lines.join(" ");
        assert!(joined.contains("alpha"));
        assert!(joined.contains("romeo"));
    }
}
