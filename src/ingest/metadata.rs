use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::util::normalize_language;

// Require the opening `---` to be followed immediately by a newline (no
// trailing whitespace).  This matches the YAML frontmatter spec and avoids
// accidentally treating a horizontal-rule (`--- ` with trailing space) as a
// frontmatter delimiter.
static FRONTMATTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\A---\r?\n(.*?)\r?\n---").expect("valid regex"));

static TOML_FRONTMATTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\A\+\+\+\s*\n(.*?)\n\+\+\+").expect("valid regex"));

static FM_KEY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^([a-zA-Z_][a-zA-Z0-9_]*)\s*[:=]\s*(.+)$").expect("valid regex")
});

static ORG_KEYWORD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^#\+([A-Z_]+):\s+(.+)$").expect("valid regex"));

static HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z][A-Za-z \-]+):\s+(.+)$").expect("valid regex"));

/// Characters in a header value that indicate code rather than metadata.
fn is_code_value(value: &str) -> bool {
    value.ends_with(',')
        || value.ends_with(';')
        || value.ends_with('{')
        || value.ends_with('}')
        || value.contains('<')
        || value.contains('(')
}

static BRACKET_SUFFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\[.*?\]\s*$").expect("valid regex"));

static BY_ATTRIBUTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[Bb]y\s+((?:[A-Z][a-z]+(?:\s+(?:and|&)\s+)?)+(?:\s+[A-Z][a-z]+)*)$")
        .expect("valid regex")
});

static COPYRIGHT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)copyright\s+(?:\([cC]\)\s*|©\s*)?(\d{4}(?:\s*[-–]\s*\d{4})?)\s+(.+)")
        .expect("valid regex")
});

static KEY_MAP: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("title", "title"),
        ("author", "author"),
        ("authors", "author"),
        ("creator", "author"),
        ("editor", "author"),
        ("by", "author"),
        ("from", "author"),
        ("language", "lang"),
        ("lang", "lang"),
        ("date", "created"),
        ("release date", "created"),
        ("posting date", "created"),
        ("publication date", "created"),
        ("published", "created"),
        ("last updated", "created"),
        ("subject", "tags"),
        ("keywords", "tags"),
        ("tags", "tags"),
        ("category", "tags"),
        ("categories", "tags"),
        ("topic", "topic"),
    ])
});

static ORG_KEY_MAP: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("TITLE", "title"),
        ("AUTHOR", "author"),
        ("DATE", "created"),
        ("LANGUAGE", "lang"),
        ("KEYWORDS", "tags"),
        ("DESCRIPTION", "tags"),
    ])
});

#[derive(Debug, Default)]
pub(crate) struct ExtractedMeta {
    pub(crate) title: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) lang: Option<String>,
    pub(crate) created: Option<String>,
    pub(crate) tags: Option<String>,
    pub(crate) topic: Option<String>,
}

impl ExtractedMeta {
    /// Returns `true` if at least one metadata field has been populated.
    fn has_any(&self) -> bool {
        self.title.is_some()
            || self.author.is_some()
            || self.lang.is_some()
            || self.created.is_some()
            || self.tags.is_some()
            || self.topic.is_some()
    }

    /// Set a metadata field by name if it is currently `None`.
    fn set_if_empty(&mut self, field: &str, value: String) {
        let slot = match field {
            "title" => &mut self.title,
            "author" => &mut self.author,
            "lang" => &mut self.lang,
            "created" => &mut self.created,
            "tags" => &mut self.tags,
            "topic" => &mut self.topic,
            _ => return,
        };
        slot.get_or_insert(value);
    }
}

/// Extract metadata from text content.
///
/// Tries formats in order of reliability:
/// 1. YAML frontmatter (`---`)
/// 2. TOML frontmatter (`+++`)
/// 3. Org-mode keywords (`#+TITLE:`)
/// 4. Key: Value header lines (RFC 2822-style, Gutenberg, etc.)
/// 5. Heuristic fallbacks ("by Author", copyright lines, first line as title)
///
/// Steps 1-3 return early when they produce any metadata. Steps 4-5 always
/// run after steps 1-3 are exhausted; heuristic fallbacks for author and title
/// still apply even when frontmatter was found, filling in any fields that
/// frontmatter left absent.
pub(crate) fn extract_metadata(text: &str) -> ExtractedMeta {
    // YAML frontmatter
    if text.starts_with("---")
        && let Some(caps) = FRONTMATTER_RE.captures(text)
    {
        let fm = extract_kv_block(&caps[1]);
        if fm.has_any() {
            return fm;
        }
    }

    // TOML frontmatter
    if text.starts_with("+++")
        && let Some(caps) = TOML_FRONTMATTER_RE.captures(text)
    {
        let fm = extract_kv_block(&caps[1]);
        if fm.has_any() {
            return fm;
        }
    }

    // Org-mode keywords
    if text.starts_with("#+") {
        let fm = extract_org_keywords(text);
        if fm.has_any() {
            return fm;
        }
    }

    // Key: Value headers -- skip if a frontmatter delimiter was present but
    // unrecognized (e.g. invalid YAML). Falling through to text-header
    // extraction would misparse frontmatter keys as headers.
    let has_frontmatter_prefix =
        text.starts_with("---") || text.starts_with("+++") || text.starts_with("#+");
    let mut meta = if has_frontmatter_prefix {
        ExtractedMeta::default()
    } else {
        extract_text_headers(text)
    };

    // Heuristic fallbacks for missing fields
    let head = crate::util::truncate_str_ref(text, 4096);
    if meta.author.is_none() {
        meta.author = extract_by_attribution(head);
    }
    if (meta.author.is_none() || meta.created.is_none())
        && let Some((author, year)) = extract_copyright(head)
    {
        if meta.author.is_none() {
            meta.author = Some(author);
        }
        if meta.created.is_none() {
            meta.created = Some(year);
        }
    }
    if meta.title.is_none() {
        meta.title = extract_first_line_title(text);
    }

    meta
}

/// Extract metadata from a YAML or TOML frontmatter key-value block.
fn extract_kv_block(body: &str) -> ExtractedMeta {
    let mut meta = ExtractedMeta::default();

    for caps in FM_KEY_RE.captures_iter(body) {
        let key = caps[1].trim().to_lowercase();
        let value = caps[2]
            .trim()
            .trim_matches(|c| c == '"' || c == '\'')
            .to_owned();
        if let Some(&mapped) = KEY_MAP.get(key.as_str()) {
            meta.set_if_empty(mapped, value);
        }
    }

    finalize_lang(&mut meta);
    meta
}

/// Extract metadata from Org-mode `#+KEY: value` keyword lines.
fn extract_org_keywords(text: &str) -> ExtractedMeta {
    let mut meta = ExtractedMeta::default();

    for caps in ORG_KEYWORD_RE.captures_iter(text) {
        let key = caps[1].trim();
        let value = caps[2].trim().to_owned();
        if let Some(&mapped) = ORG_KEY_MAP.get(key) {
            meta.set_if_empty(mapped, value);
        }
    }

    finalize_lang(&mut meta);
    meta
}

/// Extract metadata from RFC 2822-style `Key: Value` header lines at the top of the document.
fn extract_text_headers(text: &str) -> ExtractedMeta {
    let mut meta = ExtractedMeta::default();

    for line in text.lines().take(100) {
        let stripped = line.trim();
        if stripped.is_empty() {
            continue;
        }

        if stripped.starts_with("***") || stripped.starts_with("===") {
            break;
        }

        let indent = line.len() - line.trim_start().len();
        if indent >= 4 || line.starts_with('\t') {
            continue;
        }

        if let Some(caps) = HEADER_RE.captures(stripped) {
            let key = caps[1].trim().to_lowercase();
            let raw_value = caps[2].trim();
            if is_code_value(raw_value) {
                continue;
            }
            if let Some(&mapped) = KEY_MAP.get(key.as_str()) {
                let value = clean_header_value(raw_value);
                if !value.is_empty() {
                    meta.set_if_empty(mapped, value);
                }
            }
        }
    }

    finalize_lang(&mut meta);
    meta
}

/// Strip common annotations from header values.
/// e.g. "December 1, 1971 [eBook #1]" -> "December 1, 1971"
fn clean_header_value(value: &str) -> String {
    let cleaned = BRACKET_SUFFIX_RE.replace(value, "");
    cleaned.trim().to_owned()
}

/// Return the byte offset just past the Nth `\n` in `text`.
/// If `text` has fewer than `n` newlines, returns `text.len()`.
/// Handles both `\n` and `\r\n` correctly (scans raw bytes).
fn byte_end_of_line_n(text: &str, n: usize) -> usize {
    let mut count = 0;
    for (i, &byte) in text.as_bytes().iter().enumerate() {
        if byte == b'\n' {
            count += 1;
            if count >= n {
                return i;
            }
        }
    }
    text.len()
}

/// Detect "by Author Name" attribution lines near the top.
fn extract_by_attribution(head: &str) -> Option<String> {
    let scan_end = byte_end_of_line_n(head, 20);
    let caps = BY_ATTRIBUTION_RE.captures(&head[..scan_end])?;
    let name = caps[1].trim();
    (name.split_whitespace().count() >= 2).then(|| name.to_owned())
}

/// Extract author and year from copyright lines.
fn extract_copyright(head: &str) -> Option<(String, String)> {
    let scan_end = byte_end_of_line_n(head, 30);
    let caps = COPYRIGHT_RE.captures(&head[..scan_end])?;
    let year = caps[1].trim().to_owned();
    let rest = caps[2].trim();
    let author = rest.split(['.', ',']).next().unwrap_or(rest).trim();
    let lower = author.to_lowercase();
    let author = if let Some(pos) = lower.rfind("all rights reserved") {
        author[..pos].trim()
    } else {
        author
    };
    if author.is_empty() || author.len() > 100 {
        return None;
    }
    Some((author.to_owned(), year))
}

/// Use the first non-blank line as title if it looks like one:
/// short (< 120 chars), no trailing period, followed by a blank line.
fn extract_first_line_title(text: &str) -> Option<String> {
    let mut lines = text.lines();
    let first = lines.next()?.trim();
    if first.is_empty() {
        let first = lines.next()?.trim();
        return check_title_line(first, lines.next());
    }
    check_title_line(first, lines.next())
}

/// Validate and return a candidate title line, requiring it to be short, not ending in `.`, and followed by a blank line.
fn check_title_line(line: &str, next_line: Option<&str>) -> Option<String> {
    if line.is_empty() || line.len() > 120 {
        return None;
    }
    if line.ends_with('.') || line.ends_with('{') || line.ends_with(';') {
        return None;
    }
    if is_code_value(line) {
        return None;
    }
    if HEADER_RE.is_match(line) {
        return None;
    }
    if line.starts_with("---")
        || line.starts_with("+++")
        || line.starts_with("#+")
        || line.starts_with("//")
        || line.starts_with('#')
    {
        return None;
    }
    if let Some(next) = next_line
        && !next.trim().is_empty()
    {
        return None;
    }
    Some(line.to_owned())
}

/// Convert kreuzberg's native metadata into an `ExtractedMeta`.
pub(crate) fn from_kreuzberg(meta: &kreuzberg::Metadata) -> ExtractedMeta {
    let author = meta
        .authors
        .as_ref()
        .filter(|a| !a.is_empty())
        .map(|a| a.join(", "))
        .or_else(|| meta.created_by.clone());

    let lang = meta.language.as_ref().and_then(|l| normalize_language(l));

    let tags = meta
        .tags
        .as_ref()
        .filter(|t| !t.is_empty())
        .map(|t| t.join(", "))
        .or_else(|| {
            meta.keywords
                .as_ref()
                .filter(|k| !k.is_empty())
                .map(|k| k.join(", "))
        })
        .or_else(|| meta.subject.clone())
        .or_else(|| meta.category.clone());

    ExtractedMeta {
        title: meta
            .title
            .as_deref()
            .map(strip_heading_marker)
            .map(str::to_owned),
        author,
        lang,
        created: meta.created_at.clone(),
        tags,
        topic: None,
    }
}

/// Merge two metadata sets. `primary` (typically from binary extraction)
/// takes priority; `fallback` (typically from text extraction) fills gaps.
pub(crate) fn merge(primary: ExtractedMeta, fallback: ExtractedMeta) -> ExtractedMeta {
    ExtractedMeta {
        title: primary.title.or(fallback.title),
        author: primary.author.or(fallback.author),
        lang: primary.lang.or(fallback.lang),
        created: primary.created.or(fallback.created),
        tags: primary.tags.or(fallback.tags),
        topic: primary.topic.or(fallback.topic),
    }
}

/// Strip leading markdown heading markers (`# `, `## `, etc.) from a title.
fn strip_heading_marker(s: &str) -> &str {
    let trimmed = s.trim_start_matches('#');
    if trimmed.len() < s.len() && trimmed.starts_with(' ') {
        trimmed.trim_start()
    } else {
        s
    }
}

/// Normalize the `lang` field in-place using the shared language normalizer.
fn finalize_lang(meta: &mut ExtractedMeta) {
    if let Some(raw) = meta.lang.take() {
        meta.lang = normalize_language(&raw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_cases() {
        struct Case {
            input: &'static str,
            title: Option<&'static str>,
            author: Option<&'static str>,
            created: Option<&'static str>,
            lang: Option<&'static str>,
        }

        let cases = &[
            Case {
                input: "---\ntitle: My Document\nauthor: Alice\ndate: 2024-01-15\n---\n\nBody text.",
                title: Some("My Document"),
                author: Some("Alice"),
                created: Some("2024-01-15"),
                lang: None,
            },
            Case {
                input: "+++\ntitle = \"My Document\"\nauthor = \"Bob\"\ndate = \"2024-06-01\"\n+++\n\nBody.",
                title: Some("My Document"),
                author: Some("Bob"),
                created: Some("2024-06-01"),
                lang: None,
            },
            Case {
                input: "+++\ntitle = \"TOML Lang\"\nlanguage = \"German\"\n+++\n\nContent here.",
                title: Some("TOML Lang"),
                author: None,
                created: None,
                lang: Some("de"),
            },
            Case {
                input: "#+TITLE: Org Document\n#+AUTHOR: Carol\n#+DATE: 2024-02-20\n#+LANGUAGE: French\n\n* Heading\nBody.",
                title: Some("Org Document"),
                author: Some("Carol"),
                created: Some("2024-02-20"),
                lang: Some("fr"),
            },
            Case {
                input: "Title: Project Gutenberg\nAuthor: Various\nLanguage: English\n\nThe actual content.",
                title: Some("Project Gutenberg"),
                author: Some("Various"),
                created: None,
                lang: Some("en"),
            },
            Case {
                input: "Title: Some Book [Volume 1]\nDate: 2024-03-15 [revised]\n\nBody.",
                title: Some("Some Book"),
                author: None,
                created: Some("2024-03-15"),
                lang: None,
            },
            Case {
                input: "Some introductory paragraph...\n\nTitle: My Document\nAuthor: Jane Doe\nLanguage: French\n\nContent.",
                title: Some("My Document"),
                author: Some("Jane Doe"),
                created: None,
                lang: Some("fr"),
            },
            Case {
                input: "The Great Novel\n\nby Herman Melville\n\nChapter 1...",
                title: Some("The Great Novel"),
                author: Some("Herman Melville"),
                created: None,
                lang: None,
            },
            Case {
                input: "Some Manual\n\nCopyright (c) 2024 Jane Smith. All rights reserved.\n\nContent here.",
                title: Some("Some Manual"),
                author: Some("Jane Smith"),
                created: Some("2024"),
                lang: None,
            },
            Case {
                input: "My Important Document\n\nThis is the body...",
                title: Some("My Important Document"),
                author: None,
                created: None,
                lang: None,
            },
            Case {
                input: "This is a normal sentence.\n\nFollowed by more text.",
                title: None,
                author: None,
                created: None,
                lang: None,
            },
        ];

        for case in cases {
            let meta = extract_metadata(case.input);
            assert_eq!(meta.title.as_deref(), case.title, "input: {:?}", case.input);
            assert_eq!(
                meta.author.as_deref(),
                case.author,
                "input: {:?}",
                case.input
            );
            assert_eq!(
                meta.created.as_deref(),
                case.created,
                "input: {:?}",
                case.input
            );
            assert_eq!(meta.lang.as_deref(), case.lang, "input: {:?}", case.input);
        }

        // Gutenberg fixture: multi-line input cannot be &'static str inline above.
        let gutenberg = "\
The Project Gutenberg eBook of The Declaration of Independence

This eBook is for the use of anyone anywhere in the United States and
most other parts of the world at no cost and with almost no restrictions
whatsoever.

Title: The Declaration of Independence of the United States of America

Author: Thomas Jefferson


\x20\x20\x20\x20\x20\x20\x20\x20
Release date: December 1, 1971 [eBook #1]
                Most recently updated: September 2, 2025

Language: English

Credits: This etext was produced by Michael S. Hart.


*** START OF THE PROJECT GUTENBERG EBOOK ***

Actual body text here.";
        let meta = extract_metadata(gutenberg);
        assert_eq!(
            meta.title.as_deref(),
            Some("The Declaration of Independence of the United States of America")
        );
        assert_eq!(meta.author.as_deref(), Some("Thomas Jefferson"));
        assert_eq!(meta.created.as_deref(), Some("December 1, 1971"));
        assert_eq!(meta.lang.as_deref(), Some("en"));

        // Title longer than 120 chars must not be extracted.
        let long_title = format!("{}\n\nBody.", "A".repeat(130));
        let meta = extract_metadata(&long_title);
        assert!(meta.title.is_none());

        // Empty input must return no metadata.
        let meta = extract_metadata("");
        assert!(meta.title.is_none());

        // byte_end_of_line_n: fewer newlines than requested returns text.len()
        assert_eq!(byte_end_of_line_n("hello", 20), 5);
        assert_eq!(byte_end_of_line_n("", 20), 0);
        assert_eq!(byte_end_of_line_n("abc\ndef", 1), 3);
        // Only 1 newline, returns text.len()
        assert_eq!(byte_end_of_line_n("abc\ndef", 2), 7);
        assert_eq!(byte_end_of_line_n("abc\r\ndef", 1), 4);
        assert_eq!(byte_end_of_line_n("café\nlatte", 1), "café".len());

        // extract_copyright: symbol variant
        let result = extract_copyright("Copyright © 2024 Acme Corp");
        assert_eq!(result.as_ref().map(|(_, y)| y.as_str()), Some("2024"));
        assert_eq!(result.as_ref().map(|(a, _)| a.as_str()), Some("Acme Corp"));

        // extract_copyright: plain-text variant
        let result = extract_copyright("Copyright 2024 Acme Corp");
        assert_eq!(result.as_ref().map(|(_, y)| y.as_str()), Some("2024"));
        assert_eq!(result.as_ref().map(|(a, _)| a.as_str()), Some("Acme Corp"));

        // U+00C2 before U+00A9 must NOT be treated as an optional prefix.
        // The combined sequence is not a recognised symbol so the regex does
        // not match at all (the stray byte blocks the year digit anchor). The
        // key invariant is that the function returns None cleanly rather than
        // panicking or producing garbage output with a wrong year.
        let result = extract_copyright("Copyright Â© 2024 Stray Corp");
        assert!(
            result.is_none(),
            "stray Â© should not produce a match, got: {result:?}"
        );

        // extract_copyright: year range
        let result = extract_copyright("copyright © 2023-2024 Example Inc");
        assert!(result.is_some());
        let (author, year) = result.unwrap();
        assert_eq!(year, "2023-2024");
        assert_eq!(author, "Example Inc");

        // Indented lines and code-like values must not be parsed as headers.
        let rust_struct = "\
pub struct Foo {
    topic: Option<String>,
    author: Option<String>,
    title: String,
}";
        let meta = extract_metadata(rust_struct);
        assert!(meta.topic.is_none(), "indented 'topic:' must not match");
        assert!(meta.author.is_none(), "indented 'author:' must not match");

        // Code-like values (parens, angle brackets, trailing commas)
        // must be rejected even at column 0.
        let code_like = "\
topic: Some(Arc::from(topic)),
author: Vec<String>,
title: doc.topic.take(),
";
        let meta = extract_metadata(code_like);
        assert!(
            meta.topic.is_none(),
            "code value with parens must not match"
        );
        assert!(
            meta.author.is_none(),
            "code value with angle brackets must not match"
        );

        // Tab-indented lines must not match.
        let tabbed = "\ttopic: something\n";
        let meta = extract_metadata(tabbed);
        assert!(meta.topic.is_none(), "tab-indented must not match");

        // Comment and code first lines must not become titles.
        let comment_first = "//! Module documentation\n\nBody text.";
        let meta = extract_metadata(comment_first);
        assert!(meta.title.is_none(), "comment line must not be title");

        let code_first = "use std::io;\n\nfn main() {}";
        let meta = extract_metadata(code_first);
        assert!(meta.title.is_none(), "code line must not be title");

        let hash_first = "#!/usr/bin/env python\n\nprint('hello')";
        let meta = extract_metadata(hash_first);
        assert!(meta.title.is_none(), "shebang must not be title");
    }
}
