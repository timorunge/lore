//! General-purpose utility functions re-exported from submodules.

mod fs;
mod hash;
mod lang;
mod marker;
mod net;
pub mod platform;
pub mod progress;
mod time;

pub use fs::{atomic_write, dir_size, is_lexically_safe_path, normalize_path, relativize_path};
pub use hash::blake3_hex;
#[cfg(any(test, feature = "ingest", feature = "test-support"))]
pub(crate) use hash::hex_encode;
pub use lang::normalize_language;
pub use marker::{ensure_marker, verify_marker};
pub use net::is_loopback;
pub use time::{iso8601_now, unix_now};

/// XDG-style config directory: `$XDG_CONFIG_HOME` if set, else `~/.config`.
///
/// On Windows, falls back to `dirs::config_dir()` (`%APPDATA%`).
pub fn config_dir() -> Option<std::path::PathBuf> {
    #[cfg(unix)]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            let p = std::path::PathBuf::from(xdg);
            if p.is_absolute() {
                return Some(p);
            }
        }
        dirs::home_dir().map(|h| h.join(".config"))
    }
    #[cfg(not(unix))]
    {
        dirs::config_dir()
    }
}

/// Truncate a string to at most `max` bytes, respecting char boundaries.
pub fn truncate_str_ref(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    // Manual floor_char_boundary: walk back from max to find a char boundary.
    let mut boundary = max;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &s[..boundary]
}

/// Truncate to at most `max` characters (Unicode scalar values).
pub fn truncate_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Left-truncate to at most `max` characters, showing tail with `".."` prefix.
/// When `max <= 1`, the `..` prefix is omitted.
///
/// Returns `Cow::Borrowed` when no truncation is needed (zero allocation);
/// `Cow::Owned` only when the string is actually shortened.
pub fn truncate_left_chars(s: &str, max: usize) -> std::borrow::Cow<'_, str> {
    let count = s.chars().count();
    if count <= max {
        return std::borrow::Cow::Borrowed(s);
    }
    if max == 0 {
        return std::borrow::Cow::Owned(String::new());
    }
    if max == 1 {
        return std::borrow::Cow::Owned(
            s.chars().last().map_or_else(String::new, |c| c.to_string()),
        );
    }
    let tail_chars = max - 2;
    let skip = count - tail_chars;
    let start = s.char_indices().nth(skip).map_or(s.len(), |(i, _)| i);
    std::borrow::Cow::Owned(format!("..{}", &s[start..]))
}

/// Slice a collection by offset and limit, clamping both to bounds.
///
/// Returns an empty slice when `offset >= items.len()` (out-of-range offset
/// is safe and never panics). `limit` may be `usize::MAX` (the federated
/// store passes it to fetch all items); `saturating_add` prevents overflow.
pub fn paginate<T>(items: &[T], offset: usize, limit: usize) -> &[T] {
    let start = offset.min(items.len());
    &items[start..start.saturating_add(limit).min(items.len())]
}

/// Half of available cores, clamped to [2, 32].
pub fn half_cores() -> usize {
    let cores = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    (cores / 2).clamp(2, 32)
}

/// Extract the scheme from a URL.
pub fn url_scheme(url: &str) -> String {
    match url.find("://") {
        Some(idx) => url[..idx].to_ascii_lowercase(),
        None => String::new(),
    }
}

/// Case-insensitive equality check against a pre-lowercased needle.
/// Avoids allocating a lowered copy of `haystack`.
pub fn eq_lowercase(haystack: &str, needle_lc: &str) -> bool {
    let mut haystack_chars = haystack.chars().flat_map(char::to_lowercase);
    let mut needle_chars = needle_lc.chars();
    loop {
        match (haystack_chars.next(), needle_chars.next()) {
            (Some(a), Some(b)) if a == b => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

/// Case-insensitive substring check against a pre-lowercased needle.
/// Zero-allocation for ASCII haystacks; falls back to `to_lowercase()` for
/// non-ASCII content.
pub fn contains_lowercase(haystack: &str, needle_lc: &str) -> bool {
    if needle_lc.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle_lc.is_ascii() {
        let needle_bytes = needle_lc.as_bytes();
        haystack.as_bytes().windows(needle_bytes.len()).any(|w| {
            w.iter()
                .zip(needle_bytes)
                .all(|(h, n)| h.to_ascii_lowercase() == *n)
        })
    } else {
        haystack.to_lowercase().contains(needle_lc)
    }
}

/// Returns `true` if `url` uses the `http` or `https` scheme.
pub fn is_http_url(url: &str) -> bool {
    let scheme = url_scheme(url);
    scheme == "http" || scheme == "https"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_scheme() {
        let cases: &[(&str, &str)] = &[
            ("https://example.com", "https"),
            ("HTTP://FOO", "http"),
            ("ftp://x", "ftp"),
            ("notaurl", ""),
            ("", ""),
        ];
        for (input, expected) in cases {
            assert_eq!(url_scheme(input), *expected, "input={input:?}");
        }
    }

    #[test]
    fn truncate_functions() {
        let cases: &[(&str, usize, &str)] = &[
            ("hello", 0, ""),
            ("hello", 3, "hel"),
            ("hello", 5, "hello"),
            ("hello", 10, "hello"),
            ("", 0, ""),
            ("café", 3, "caf"),
        ];
        for (input, max, expected) in cases {
            assert_eq!(
                truncate_chars(input, *max),
                *expected,
                "input={input:?} max={max}"
            );
        }

        let cases: &[(&str, usize, &str)] = &[
            ("hello", 0, ""),
            ("hello", 1, "o"),
            ("hello", 2, ".."),
            ("hello", 3, "..o"),
            ("hello", 5, "hello"),
            ("hello", 10, "hello"),
            ("", 0, ""),
            ("café", 3, "..é"),
        ];
        for (input, max, expected) in cases {
            assert_eq!(
                truncate_left_chars(input, *max),
                *expected,
                "input={input:?} max={max}"
            );
        }

        let cases: &[(&str, usize, &str)] = &[
            ("hello", 0, ""),
            ("café", 1, "c"),
            ("café", 3, "caf"),
            ("café", 4, "caf"),
            ("café", 5, "café"),
            ("café", 100, "café"),
        ];
        for (input, max, expected) in cases {
            assert_eq!(
                truncate_str_ref(input, *max),
                *expected,
                "input={input:?} max={max}"
            );
        }
    }

    #[test]
    fn test_paginate() {
        let items = [1, 2, 3, 4, 5];
        assert_eq!(paginate(&items, 0, 2), &[1, 2]);
        assert_eq!(paginate(&items, 4, 10), &[5]);
        assert_eq!(paginate(&items, 10, 5), &[] as &[i32]);
    }

    #[test]
    fn eq_lowercase_cases() {
        assert!(eq_lowercase("Hello", "hello"));
        assert!(eq_lowercase("HELLO", "hello"));
        assert!(eq_lowercase("hello", "hello"));
        assert!(!eq_lowercase("hell", "hello"));
        assert!(!eq_lowercase("hello!", "hello"));
        assert!(eq_lowercase("Café", "café"));
        assert!(eq_lowercase("STRASSE", "strasse"));
        assert!(eq_lowercase("", ""));
        assert!(!eq_lowercase("a", ""));
        assert!(!eq_lowercase("", "a"));
    }

    #[test]
    fn contains_lowercase_cases() {
        assert!(contains_lowercase("Hello World", "world"));
        assert!(contains_lowercase("HELLO", "ello"));
        assert!(contains_lowercase("test", "test"));
        assert!(!contains_lowercase("hello", "xyz"));
        assert!(contains_lowercase("anything", ""));
        assert!(contains_lowercase("Café Latte", "café"));
        assert!(!contains_lowercase("short", "longer_needle"));
    }

    #[test]
    fn config_dir_returns_xdg_style_path() {
        let dir = config_dir();
        #[cfg(unix)]
        {
            let dir = dir.expect("config_dir should return Some on unix");
            assert!(dir.is_absolute());
            assert!(
                dir.ends_with(".config") || std::env::var("XDG_CONFIG_HOME").is_ok(),
                "expected ~/.config or XDG_CONFIG_HOME override, got: {dir:?}"
            );
        }
        #[cfg(not(unix))]
        {
            assert!(dir.is_some(), "config_dir should return Some on Windows");
        }
    }
}
