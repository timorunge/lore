use anyhow::{Context, Result};

use crate::config::{ExtractMode, MetadataField, ProcessingProfile, Transform};
use crate::ingest::chunker::resolve_headings;
use crate::ingest::metadata;
use crate::ingest::types::LoaderResult;

/// A compiled pipeline step ready to execute against a document.
#[derive(Clone)]
enum CompiledEntry {
    /// Remove `first` lines from the start and `last` lines from the end.
    /// Uses `str::lines()`, so `\n`-splitting normalizes CRLF to LF.
    StripLines {
        first: usize,
        last: usize,
    },
    ReplaceText {
        re: regex::Regex,
        replacement: String,
    },
    ExtractBuiltin,
    ExtractMetadata {
        re: regex::Regex,
        field: MetadataField,
    },
}

/// A processing profile with pre-compiled regexes and lowercased headings.
/// Built once, then shared across all documents processed for a source.
#[derive(Clone)]
pub struct CompiledProfile {
    pub extract: ExtractMode,
    pub max_chunk_chars: usize,
    pub min_chunk_chars: usize,
    drop_sections: Vec<String>,
    pipeline: Vec<CompiledEntry>,
}

impl CompiledProfile {
    /// Compile a `ProcessingProfile` into an executable form.
    pub fn compile(profile: &ProcessingProfile) -> Result<Self> {
        let mut pipeline = Vec::with_capacity(profile.pipeline.len());
        for (i, step) in profile.pipeline.iter().enumerate() {
            let entry = match step {
                Transform::StripLines(t) => CompiledEntry::StripLines {
                    first: t.first,
                    last: t.last,
                },
                Transform::ReplaceText(t) => CompiledEntry::ReplaceText {
                    re: compile_multiline(&t.pattern)
                        .with_context(|| format!("pipeline[{i}]: invalid regex: {}", t.pattern))?,
                    replacement: t.replacement.clone(),
                },
                Transform::ExtractBuiltin => CompiledEntry::ExtractBuiltin,
                Transform::ExtractMetadata(t) => CompiledEntry::ExtractMetadata {
                    re: compile_multiline(&t.pattern)
                        .with_context(|| format!("pipeline[{i}]: invalid regex: {}", t.pattern))?,
                    field: t.field,
                },
            };
            pipeline.push(entry);
        }
        Ok(Self {
            extract: profile.extract,
            max_chunk_chars: profile.max_chunk_chars,
            min_chunk_chars: profile.min_chunk_chars,
            drop_sections: resolve_headings(&profile.drop_sections),
            pipeline,
        })
    }

    /// Heading names to strip from documents during chunking.
    pub fn drop_sections(&self) -> &[String] {
        &self.drop_sections
    }

    /// Run pipeline transforms in declaration order on a document.
    /// Extraction steps use first-wins semantics; content transforms always apply.
    pub fn apply_pipeline(&self, doc: &mut LoaderResult) {
        for entry in &self.pipeline {
            match entry {
                CompiledEntry::StripLines { first, last } => {
                    apply_strip_lines(doc, *first, *last);
                }
                CompiledEntry::ReplaceText { re, replacement } => {
                    let result = re.replace_all(&doc.content, replacement.as_str());
                    if matches!(result, std::borrow::Cow::Owned(_)) {
                        doc.content = result.into_owned();
                    }
                }
                CompiledEntry::ExtractBuiltin => {
                    let extracted = metadata::extract_metadata(&doc.content);
                    merge_first_wins(doc, extracted);
                }
                CompiledEntry::ExtractMetadata { re, field } => {
                    if let Some(caps) = re.captures(&doc.content)
                        && let Some(m) = caps.get(1)
                    {
                        let value = m.as_str().trim().to_owned();
                        if !value.is_empty() {
                            set_field_if_empty(doc, *field, value);
                        }
                    }
                }
            }
        }
    }
}

/// Compile a regex with multiline mode enabled by default so `^` and `$`
/// match line boundaries.  If the pattern already starts with inline flags
/// (`(?...)`) the user's flags take precedence.
fn compile_multiline(pattern: &str) -> anyhow::Result<regex::Regex> {
    if pattern.starts_with("(?") {
        regex::Regex::new(pattern).map_err(anyhow::Error::from)
    } else {
        regex::Regex::new(&format!("(?m){pattern}")).map_err(anyhow::Error::from)
    }
}

/// Remove the first `first` and last `last` lines from the document content.
fn apply_strip_lines(doc: &mut LoaderResult, first: usize, last: usize) {
    if first == 0 && last == 0 {
        return;
    }
    let lines: Vec<&str> = doc.content.lines().collect();
    let total = lines.len();
    let start = first.min(total);
    let end = total.saturating_sub(last).max(start);
    if start >= end {
        doc.content.clear();
        return;
    }
    doc.content = lines[start..end].join("\n");
}

/// Copy non-null fields from `meta` into `doc`, skipping fields already set.
fn merge_first_wins(doc: &mut LoaderResult, meta: metadata::ExtractedMeta) {
    if doc.title.is_none() {
        doc.title = meta.title;
    }
    if doc.author.is_none() {
        doc.author = meta.author;
    }
    if doc.lang.is_none() {
        doc.lang = meta.lang;
    }
    if doc.created_at.is_none() {
        doc.created_at = meta.created;
    }
    if doc.tags.is_none() {
        doc.tags = meta.tags;
    }
    if doc.topic.is_none() {
        doc.topic = meta.topic;
    }
}

/// Set a named metadata field on `doc` if it is currently unset.
// NOTE: metadata::ExtractedMeta has a similar `set_if_empty` method using
// string keys. Kept separate because this operates on LoaderResult + MetadataField enum.
fn set_field_if_empty(doc: &mut LoaderResult, field: MetadataField, value: String) {
    match field {
        MetadataField::Title => {
            if doc.title.is_none() {
                doc.title = Some(value);
            }
        }
        MetadataField::Author => {
            if doc.author.is_none() {
                doc.author = Some(value);
            }
        }
        MetadataField::Lang => {
            if doc.lang.is_none() {
                doc.lang = Some(value);
            }
        }
        MetadataField::Tags => {
            if doc.tags.is_none() {
                doc.tags = Some(value);
            }
        }
        MetadataField::Topic => {
            if doc.topic.is_none() {
                doc.topic = Some(value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::transforms::{ExtractMetadata, ReplaceText, StripLines};

    #[test]
    fn transform_strip_lines() {
        let profile = ProcessingProfile {
            pipeline: vec![Transform::StripLines(StripLines { first: 2, last: 1 })],
            ..Default::default()
        };
        let compiled = CompiledProfile::compile(&profile).unwrap();
        let mut doc = LoaderResult::test_doc("line1\nline2\nline3\nline4\nline5");
        compiled.apply_pipeline(&mut doc);
        assert_eq!(doc.content, "line3\nline4");
    }

    #[test]
    fn transform_replace_text_multiline() {
        let profile = ProcessingProfile {
            pipeline: vec![Transform::ReplaceText(ReplaceText {
                pattern: r"^CONFIDENTIAL.*$".into(),
                replacement: String::new(),
            })],
            ..Default::default()
        };
        let compiled = CompiledProfile::compile(&profile).unwrap();
        let mut doc = LoaderResult::test_doc("public info\nCONFIDENTIAL: secret\nmore public");
        compiled.apply_pipeline(&mut doc);
        assert_eq!(doc.content, "public info\n\nmore public");
    }

    #[test]
    fn transform_extract_metadata_cases() {
        struct Case {
            label: &'static str,
            pipeline: Vec<Transform>,
            content: &'static str,
            preset_author: Option<&'static str>,
            expected_author: Option<&'static str>,
            expected_topic: Option<&'static str>,
        }
        let cases = vec![
            Case {
                label: "basic extraction",
                pipeline: vec![Transform::ExtractMetadata(ExtractMetadata {
                    pattern: r"Author:\s+(.+)".into(),
                    field: MetadataField::Author,
                })],
                content: "Author: Jane Doe\n\nContent.",
                preset_author: None,
                expected_author: Some("Jane Doe"),
                expected_topic: None,
            },
            Case {
                label: "first match wins",
                pipeline: vec![
                    Transform::ExtractMetadata(ExtractMetadata {
                        pattern: r"Author:\s+(.+)".into(),
                        field: MetadataField::Author,
                    }),
                    Transform::ExtractMetadata(ExtractMetadata {
                        pattern: r"Writer:\s+(.+)".into(),
                        field: MetadataField::Author,
                    }),
                ],
                content: "Author: First\nWriter: Second",
                preset_author: None,
                expected_author: Some("First"),
                expected_topic: None,
            },
            Case {
                label: "loader value not overwritten",
                pipeline: vec![Transform::ExtractMetadata(ExtractMetadata {
                    pattern: r"Author:\s+(.+)".into(),
                    field: MetadataField::Author,
                })],
                content: "Author: Pipeline Author",
                preset_author: Some("Loader Author"),
                expected_author: Some("Loader Author"),
                expected_topic: None,
            },
            Case {
                label: "extract to topic field",
                pipeline: vec![Transform::ExtractMetadata(ExtractMetadata {
                    pattern: r"Title:\s+(.+?)(?:\s+\[.+\])?$".into(),
                    field: MetadataField::Topic,
                })],
                content: "Title: The Great Gatsby [Illustrated]\n\nChapter 1\nContent.",
                preset_author: None,
                expected_author: None,
                expected_topic: Some("The Great Gatsby"),
            },
        ];
        for case in cases {
            let profile = ProcessingProfile {
                pipeline: case.pipeline,
                ..Default::default()
            };
            let compiled = CompiledProfile::compile(&profile).unwrap();
            let mut doc = LoaderResult::test_doc(case.content);
            if let Some(a) = case.preset_author {
                doc.author = Some(a.into());
            }
            compiled.apply_pipeline(&mut doc);
            assert_eq!(
                doc.author.as_deref(),
                case.expected_author,
                "case: {}",
                case.label
            );
            assert_eq!(
                doc.topic.as_deref(),
                case.expected_topic,
                "case: {}",
                case.label
            );
        }
    }

    #[test]
    fn transform_extract_builtin() {
        let profile = ProcessingProfile {
            extract: ExtractMode::None,
            pipeline: vec![Transform::ExtractBuiltin],
            ..Default::default()
        };
        let compiled = CompiledProfile::compile(&profile).unwrap();
        let mut doc = LoaderResult::test_doc(
            "---\ntitle: From Frontmatter\nauthor: FM Author\n---\n\nContent.",
        );
        compiled.apply_pipeline(&mut doc);
        assert_eq!(doc.title.as_deref(), Some("From Frontmatter"));
        assert_eq!(doc.author.as_deref(), Some("FM Author"));
    }

    #[test]
    fn transform_pipeline_ordering() {
        let profile = ProcessingProfile {
            pipeline: vec![
                Transform::StripLines(StripLines { first: 1, last: 0 }),
                Transform::ExtractMetadata(ExtractMetadata {
                    pattern: r"Author:\s+(.+)".into(),
                    field: MetadataField::Author,
                }),
            ],
            ..Default::default()
        };
        let compiled = CompiledProfile::compile(&profile).unwrap();
        let mut doc = LoaderResult::test_doc("Author: Wrong (stripped)\nAuthor: Correct");
        compiled.apply_pipeline(&mut doc);
        assert_eq!(doc.author.as_deref(), Some("Correct"));
    }

    #[test]
    fn compiled_drop_sections_normalized() {
        let profile = ProcessingProfile {
            drop_sections: vec!["References".into(), "CHANGELOG".into()],
            ..Default::default()
        };
        let compiled = CompiledProfile::compile(&profile).unwrap();
        assert_eq!(compiled.drop_sections(), &["references", "changelog"]);
    }
}
