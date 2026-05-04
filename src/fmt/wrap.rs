use unicode_width::UnicodeWidthChar;

use crate::util::truncate_chars;

/// Post-process output to fit within terminal width.
pub fn cap_output(text: &str, width: usize) -> String {
    let max = width;
    let mut out = String::with_capacity(text.len() + text.len() / 20);
    let mut first = true;
    for line in text.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        cap_line(&mut out, line, max);
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum EscState {
    Normal,
    Esc,
    Csi,
}

/// Visible display width excluding ANSI escape sequences.
///
/// Returns the number of terminal columns the string occupies.
/// CJK double-width characters count as 2, control characters as 0.
pub fn visible_width(s: &str) -> usize {
    let mut w = 0usize;
    let mut state = EscState::Normal;
    for c in s.chars() {
        match state {
            EscState::Normal => {
                if c == '\x1b' {
                    state = EscState::Esc;
                } else {
                    w += c.width().unwrap_or(0);
                }
            }
            EscState::Esc => {
                state = if c == '[' {
                    EscState::Csi
                } else {
                    EscState::Normal
                };
            }
            EscState::Csi => {
                if ('\x40'..='\x7e').contains(&c) {
                    state = EscState::Normal;
                }
            }
        }
    }
    w
}

/// Byte offset of the longest prefix whose visible width fits within `target`
/// columns. Always returns a char boundary.
///
/// A double-width character that would push the total past `target` is excluded
/// from the prefix. Returns 0 when the first visible character alone exceeds
/// `target` (callers must advance by at least one char to avoid infinite loops).
fn byte_at_visible(s: &str, target: usize) -> usize {
    let mut vis = 0usize;
    let mut state = EscState::Normal;
    for (i, c) in s.char_indices() {
        match state {
            EscState::Normal => {
                if c == '\x1b' {
                    state = EscState::Esc;
                } else {
                    let w = c.width().unwrap_or(0);
                    if vis + w > target {
                        return i;
                    }
                    vis += w;
                }
            }
            EscState::Esc => {
                state = if c == '[' {
                    EscState::Csi
                } else {
                    EscState::Normal
                };
            }
            EscState::Csi => {
                if ('\x40'..='\x7e').contains(&c) {
                    state = EscState::Normal;
                }
            }
        }
    }
    s.len()
}

fn cap_line(out: &mut String, line: &str, max: usize) {
    let mut rest = line;
    loop {
        let byte_limit = byte_at_visible(rest, max);
        // byte_limit == rest.len() means all visible chars fit within max.
        // When it falls short, trailing ANSI escape codes (e.g. \x1b[0m) may
        // be the only remaining bytes -- check actual visible width as fallback.
        if byte_limit == rest.len() || visible_width(rest) <= max {
            out.push_str(rest);
            return;
        }
        // When a single wide character exceeds max, emit it anyway to avoid
        // infinite loops (byte_at_visible returns 0 in that case).
        let byte_limit = if byte_limit == 0 {
            rest.chars().next().map_or(0, char::len_utf8)
        } else {
            byte_limit
        };
        let segment = &rest[..byte_limit];
        // Break at last space (spaces never appear inside ANSI escapes).
        let break_at = segment.rfind(' ').unwrap_or(byte_limit);
        if break_at == 0 {
            out.push_str(segment);
            out.push('\n');
            rest = &rest[byte_limit..];
        } else {
            out.push_str(&rest[..break_at]);
            out.push('\n');
            rest = rest[break_at..].trim_start_matches(' ');
        }
    }
}

/// Write `text` into `out`, wrapping lines that exceed the terminal width.
pub fn write_wrapped(out: &mut String, text: &str, indent: &str, width: usize) {
    let max = width.saturating_sub(indent.len());
    if max == 0 {
        out.push_str(text);
        return;
    }
    for line in text.lines() {
        if visible_width(line) <= max {
            out.push_str(indent);
            out.push_str(line);
            out.push('\n');
        } else {
            wrap_line(out, line, indent, max);
        }
    }
}

/// Wrap a single line at word boundaries and write each continuation with `indent`.
fn wrap_line(out: &mut String, line: &str, indent: &str, max: usize) {
    let mut pos = 0;
    while pos < line.len() {
        let remaining = &line[pos..];
        if visible_width(remaining) <= max {
            out.push_str(indent);
            out.push_str(remaining);
            out.push('\n');
            break;
        }
        let mut chunk_end = byte_at_visible(remaining, max);
        if chunk_end == 0 {
            chunk_end = remaining.chars().next().map_or(0, char::len_utf8);
        }
        let chunk = &remaining[..chunk_end];
        let break_at = chunk.rfind(char::is_whitespace).unwrap_or(chunk_end);
        out.push_str(indent);
        out.push_str(&remaining[..break_at]);
        out.push('\n');
        pos += break_at;
        // Skip whitespace at the break point using char-aware advancement so
        // multi-byte whitespace (e.g. U+00A0, U+2003) does not panic.
        while pos < line.len() {
            let rest = &line[pos..];
            match rest.chars().next() {
                Some(c) if c.is_whitespace() => pos += c.len_utf8(),
                _ => break,
            }
        }
    }
}

/// Truncate a string at a word boundary for display.
pub(crate) fn truncate_body(s: &str, max: usize) -> &str {
    let truncated = truncate_chars(s, max);
    if truncated.len() == s.len() {
        return s;
    }
    truncated
        .rfind(char::is_whitespace)
        .map_or(truncated, |pos| &truncated[..pos])
}

#[cfg(test)]
mod tests {
    use super::*;

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(256))]

        #[test]
        fn prop_visible_width_never_panics(s in ".*") {
            let _ = visible_width(&s);
        }

        #[test]
        fn prop_byte_at_visible_never_panics(s in ".*", target in 0usize..256) {
            let pos = byte_at_visible(&s, target);
            // Result must be a valid char boundary and within bounds
            prop_assert!(pos <= s.len());
            prop_assert!(s.is_char_boundary(pos));
        }

        #[test]
        fn prop_wrap_line_never_panics_output_within_max(
            s in "\\PC*",
            max in 1usize..120,
        ) {
            let mut out = String::new();
            wrap_line(&mut out, &s, "", max);
            for line in out.lines() {
                let w = visible_width(line);
                // A single wide character may exceed max -- that is
                // unavoidable since we cannot split a character.
                let max_single = line.chars()
                    .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
                    .max()
                    .unwrap_or(0);
                let effective_max = max.max(max_single);
                prop_assert!(
                    w <= effective_max,
                    "wrapped line width {w} exceeds max {max}: {:?}",
                    line
                );
            }
        }

        #[test]
        fn prop_cap_line_never_panics_output_within_max(
            s in "\\PC*",
            max in 1usize..120,
        ) {
            let mut out = String::new();
            cap_line(&mut out, &s, max);
            for line in out.lines() {
                let w = visible_width(line);
                let max_single = line.chars()
                    .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
                    .max()
                    .unwrap_or(0);
                let effective_max = max.max(max_single);
                prop_assert!(
                    w <= effective_max,
                    "capped line width {w} exceeds max {max}: {:?}",
                    line
                );
            }
        }
    }

    #[test]
    fn cap_output_wrapping() {
        assert_eq!(cap_output("hello world", usize::MAX), "hello world");

        let long = "word ".repeat(30);
        let mut capped = String::new();
        cap_line(&mut capped, long.trim_end(), 80);
        for line in capped.lines() {
            assert!(
                visible_width(line) <= 80,
                "line exceeds max: {:?} ({})",
                line,
                visible_width(line)
            );
        }
        let joined: String = capped.lines().collect::<Vec<_>>().join(" ");
        assert!(joined.contains("word"));

        let long_out = cap_output(&long, usize::MAX);
        assert!(long_out.contains("word"));

        let input = "short\nlong line here";
        let capped = cap_output(input, usize::MAX);
        let lines: Vec<&str> = capped.lines().collect();
        assert_eq!(lines[0], "short");

        assert!(!cap_output("no newline", usize::MAX).ends_with('\n'));
        assert!(cap_output("has newline\n", usize::MAX).ends_with('\n'));

        let cjk = "驱动器托架电源 状态指示灯变为蓝色 ".repeat(10);
        let mut capped = String::new();
        cap_line(&mut capped, cjk.trim_end(), 80);
        for line in capped.lines() {
            assert!(
                visible_width(line) <= 80,
                "CJK line exceeds max: {} chars",
                visible_width(line),
            );
        }

        let mut out = String::new();
        write_wrapped(&mut out, &cjk, "  ", usize::MAX);
        assert!(!out.is_empty());

        // U+00A0 NON-BREAKING SPACE is a 2-byte whitespace character.
        // Place it at a wrap boundary so rfind(char::is_whitespace) picks it up.
        // The old byte-by-byte skip loop would either panic (invalid char boundary)
        // or silently corrupt output.  The new char-aware loop must handle this
        // without panicking and must preserve every character.
        let nbsp = '\u{00A0}';
        // Build a line that is exactly `max+1` visible chars with a U+00A0
        // right at position `max` (the last char before the limit).
        let max = 20usize;
        let prefix = "a".repeat(max - 1); // 19 ASCII chars
        let suffix = "b".repeat(10); // 10 more chars (would exceed max)
        let line = format!("{prefix}{nbsp}{suffix}");

        let mut wrap_out = String::new();
        // Must not panic.
        wrap_line(&mut wrap_out, &line, "", max);

        // All characters must be present in the output (ignoring newlines).
        let recovered: String = wrap_out.lines().collect();
        let original: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        let got: String = recovered.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(got, original, "characters lost or corrupted during wrap");

        // Every output line must be within the max visible width.
        for l in wrap_out.lines() {
            assert!(
                visible_width(l) <= max,
                "wrapped line too wide: {} > {}",
                visible_width(l),
                max
            );
        }

        // A line with exactly `max` visible chars followed by an ANSI reset
        // code must NOT be wrapped -- the trailing escape is invisible.
        let text = format!("{}\x1b[0m", "a".repeat(80));
        assert_eq!(visible_width(&text), 80);
        let mut cap_out = String::new();
        cap_line(&mut cap_out, &text, 80);
        assert!(
            !cap_out.contains('\n'),
            "trailing ANSI caused unwanted wrap"
        );
        assert_eq!(cap_out, text);
    }

    #[test]
    fn visible_width_cases() {
        // CJK: each character occupies 2 terminal columns.
        assert_eq!(visible_width("驱"), 2);
        assert_eq!(visible_width("驱动"), 4);
        assert_eq!(visible_width("a驱b"), 4); // 1 + 2 + 1
        let cjk_40 = "驱".repeat(40);
        assert_eq!(visible_width(&cjk_40), 80);

        // byte_at_visible stops at the correct display column for CJK.
        let s = "a驱b"; // widths: a=1, 驱=2, b=1 -> total 4
        assert_eq!(byte_at_visible(s, 1), 1); // after 'a' (1 col)
        assert_eq!(byte_at_visible(s, 3), 4); // after '驱' (3 cols, byte offset past 3-byte char)

        // ANSI CSI sequences have zero visible width.
        assert_eq!(visible_width("\x1b[2K"), 0);
        assert_eq!(visible_width("\x1b[1;31mhello\x1b[0m"), 5);
        assert_eq!(visible_width("\x1b[1A"), 0);
        assert_eq!(visible_width("\x1b[2J\x1b[Hhello\x1b[K"), 5);

        // byte_at_visible skips over ANSI prefix bytes correctly.
        // `\x1b[2K` is 4 bytes with 0 visible chars; "hi" follows at byte 4.
        let s = "\x1b[2Khi";
        assert_eq!(byte_at_visible(s, 0), 4); // ANSI prefix has 0 width, fits in 0 cols
        assert_eq!(byte_at_visible(s, 1), 5); // slice [..5] = "\x1b[2Kh" = 1 visible
        assert_eq!(byte_at_visible(s, 2), 6); // slice [..6] = whole string = 2 visible

        let s2 = "\x1b[1Aabc";
        assert_eq!(byte_at_visible(s2, 1), 5); // slice [..5] = "\x1b[1Aa" = 1 visible
        assert_eq!(byte_at_visible(s2, 3), 7); // whole string = 3 visible
    }

    #[test]
    fn truncate_body_cases() {
        assert_eq!(truncate_body("hello world", 100), "hello world");

        let result = truncate_body("hello world foo bar baz", 15);
        assert!(
            !result.contains("baz"),
            "truncated body should not contain trailing words"
        );
        assert!(
            result.len() <= 15,
            "truncated body should be within the limit"
        );
    }
}
