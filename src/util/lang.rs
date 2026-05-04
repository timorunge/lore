//! Language code normalisation utilities.

use std::sync::LazyLock;

use regex::Regex;

static LANG_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z]{2,3}(-[a-zA-Z0-9]{2,8})*$").expect("valid regex"));

/// Sorted by key for binary search.
const LANG_MAP: &[(&str, &str)] = &[
    ("arabic", "ar"),
    ("catalan", "ca"),
    ("chinese", "zh"),
    ("czech", "cs"),
    ("danish", "da"),
    ("dutch", "nl"),
    ("english", "en"),
    ("finnish", "fi"),
    ("french", "fr"),
    ("german", "de"),
    ("greek", "el"),
    ("hebrew", "he"),
    ("hindi", "hi"),
    ("hungarian", "hu"),
    ("indonesian", "id"),
    ("italian", "it"),
    ("japanese", "ja"),
    ("korean", "ko"),
    ("latin", "la"),
    ("malay", "ms"),
    ("norwegian", "no"),
    ("polish", "pl"),
    ("portuguese", "pt"),
    ("romanian", "ro"),
    ("russian", "ru"),
    ("spanish", "es"),
    ("swedish", "sv"),
    ("thai", "th"),
    ("turkish", "tr"),
    ("ukrainian", "uk"),
    ("vietnamese", "vi"),
];

/// Normalise a raw language string to a BCP-47-like code (e.g. `"English"` -> `"en"`).
pub fn normalize_language(lang: &str) -> Option<String> {
    let trimmed = lang.trim();
    let lang_normalized = if let Some(hyphen) = trimmed.find('-') {
        format!("{}{}", trimmed[..hyphen].to_lowercase(), &trimmed[hyphen..])
    } else {
        trimmed.to_lowercase()
    };
    let lang = lang_normalized.as_str();
    if let Ok(i) = LANG_MAP.binary_search_by_key(&lang, |(k, _)| k) {
        return Some(LANG_MAP[i].1.to_owned());
    }
    if LANG_CODE_RE.is_match(lang) {
        return Some(lang.to_owned());
    }
    if let Some(first) = lang.split([';', ',', '/']).next() {
        let first = first.trim();
        if let Ok(i) = LANG_MAP.binary_search_by_key(&first, |(k, _)| k) {
            return Some(LANG_MAP[i].1.to_owned());
        }
        if LANG_CODE_RE.is_match(first) {
            return Some(first.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_language_cases() {
        let cases: &[(&str, Option<&str>)] = &[
            ("English", Some("en")),
            ("en-US", Some("en-US")),
            ("English; French", Some("en")),
            ("&lang,", None),
            ("'eng'", None),
        ];
        for (input, expected) in cases {
            assert_eq!(
                normalize_language(input).as_deref(),
                *expected,
                "normalize_language({input:?})"
            );
        }
    }
}
