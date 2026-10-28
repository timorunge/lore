//! Terminal display width of text, measured and cut per grapheme cluster.
//!
//! A CLI table pads by columns, not scalars: CJK and emoji take two, combining
//! marks and joiners take none. Cutting on a grapheme boundary keeps a ZWJ
//! family, a flag, or an accented letter whole instead of leaving a stray
//! joiner or a different glyph behind.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Display columns of `s` (no ANSI handling -- see `visible_width`).
pub fn display_width(s: &str) -> usize {
    s.graphemes(true).map(UnicodeWidthStr::width).sum()
}

/// The longest prefix of `s` that fits in `max` columns.
pub fn prefix_within_width(s: &str, max: usize) -> &str {
    let mut used = 0;
    for (i, g) in s.grapheme_indices(true) {
        used += g.width();
        if used > max {
            return &s[..i];
        }
    }
    s
}

/// The longest suffix of `s` that fits in `max` columns.
pub fn suffix_within_width(s: &str, max: usize) -> &str {
    let mut used = 0;
    for (i, g) in s.grapheme_indices(true).rev() {
        used += g.width();
        if used > max {
            return &s[i + g.len()..];
        }
    }
    s
}

/// Right-pad `s` with spaces to `width` columns. Longer text is returned as is.
pub fn pad_right(s: &str, width: usize) -> String {
    let pad = width.saturating_sub(display_width(s));
    format!("{s}{}", " ".repeat(pad))
}

/// Left-pad `s` with spaces to `width` columns. Longer text is returned as is.
pub fn pad_left(s: &str, width: usize) -> String {
    let pad = width.saturating_sub(display_width(s));
    format!("{}{s}", " ".repeat(pad))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAMILY: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";

    #[test]
    fn display_width_table() {
        let cases: &[(&str, usize)] = &[
            ("abc", 3),
            ("日本語", 6),
            ("한국어", 6),
            ("e\u{301}", 1),
            (FAMILY, 2),
            ("👍🏽", 2),
            ("🇯🇵", 2),
            ("❤\u{fe0f}", 2),
            ("שלום", 4),
            ("مرحبا", 5),
            ("\u{200b}", 0),
            ("", 0),
        ];
        for (s, w) in cases {
            assert_eq!(display_width(s), *w, "{s:?}");
        }
    }

    #[test]
    fn prefix_and_suffix_never_split_a_cluster() {
        let cases: &[(&str, usize, &str, &str)] = &[
            ("日本語", 5, "日本", "本語"),
            ("日本語", 1, "", ""),
            ("e\u{301}e\u{301}", 1, "e\u{301}", "e\u{301}"),
            ("a🇯🇵b", 2, "a", "b"),
            ("abc", 9, "abc", "abc"),
        ];
        for (s, max, prefix, suffix) in cases {
            assert_eq!(prefix_within_width(s, *max), *prefix, "{s:?} {max}");
            assert_eq!(suffix_within_width(s, *max), *suffix, "{s:?} {max}");
        }
        let fam = format!("{FAMILY}x");
        assert_eq!(prefix_within_width(&fam, 1), "");
        assert_eq!(suffix_within_width(&fam, 1), "x");
    }

    #[test]
    fn padding_counts_columns() {
        assert_eq!(pad_right("日本", 6), "日本  ");
        assert_eq!(pad_left("🚀", 4), "  🚀");
        assert_eq!(pad_right("toolong", 3), "toolong");
        assert_eq!(display_width(&pad_right(FAMILY, 5)), 5);
    }
}
