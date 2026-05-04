use std::collections::HashMap;
use std::sync::Arc;

use kreuzberg::chunking::{ChunkerType, ChunkingConfig, chunk_text};
use tracing::warn;

use crate::types::{Chunk, DocKind, SourceId, SourceType, chunk_id};

/// Threshold for pre-splitting large documents at top-level headings.
/// Documents larger than this are split into sections before kreuzberg
/// chunking, avoiding kreuzberg's O(n^2) heading-map cost on huge inputs.
const PRESPLIT_THRESHOLD: usize = 512 * 1024; // 512 KB

/// Metadata attributes shared across all chunks produced from a single document.
/// Fields use `Arc<str>` so each chunk can clone them via refcount bump.
pub struct ChunkAttrs {
    pub source_id: SourceId,
    pub source: Arc<str>,
    pub origin: SourceType,
    pub kind: DocKind,
    pub format: Option<Arc<str>>,
    pub title: Option<Arc<str>>,
    pub author: Option<Arc<str>>,
    pub lang: Option<Arc<str>>,
    pub created_at: Option<Arc<str>>,
    pub tags: Option<Arc<str>>,
    pub topic: Option<Arc<str>>,
}

/// Lowercase a heading list for case-insensitive matching.
pub(crate) fn resolve_headings(headings: &[String]) -> Vec<String> {
    headings.iter().map(|s| s.to_lowercase()).collect()
}

/// CPU-bound chunking: splits `content` into indexed chunks using the given attributes.
pub fn chunk_markdown_content(
    content: &str,
    attrs: &ChunkAttrs,
    drop_set: &[String],
    max_chunk_chars: usize,
    min_chunk_chars: usize,
) -> Vec<Chunk> {
    // Strip UTF-8 BOM (\u{FEFF}) before trimming -- plain-text files from
    // sources like Project Gutenberg often start with a BOM, which is not
    // whitespace and is not stripped by str::trim().
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    let text = content.trim_matches(|c: char| c == '\n' || c == '\r');
    if text.is_empty() {
        return Vec::new();
    }

    if text.len() > PRESPLIT_THRESHOLD {
        let sections = presplit_at_headings(text);
        if sections.len() > 1 {
            let mut chunks = Vec::with_capacity(sections.len() * 4);
            for section in &sections {
                let section_chunks = chunk_section(
                    section.body,
                    attrs,
                    None, // kreuzberg finds headings from body content; passing heading_path here would duplicate them
                    drop_set,
                    max_chunk_chars,
                    min_chunk_chars,
                );
                for mut c in section_chunks {
                    c.chunk_index = chunks.len() as i64;
                    chunks.push(c);
                }
            }
            return chunks;
        }
    }

    chunk_section(
        text,
        attrs,
        None,
        drop_set,
        max_chunk_chars,
        min_chunk_chars,
    )
}

/// Fast check for whether text contains markdown headings (`# ` at line start).
/// Only scans the first 256 KB as a performance heuristic.
fn has_markdown_headings(text: &str) -> bool {
    if text.starts_with("# ") || text.starts_with("## ") {
        return true;
    }
    // Scan first 256 KB -- large enough to catch headings after long preambles
    // while still avoiding a full scan of multi-MB documents.
    let scan = crate::util::truncate_str_ref(text, 256 * 1024);
    scan.contains("\n# ") || scan.contains("\n## ")
}

/// A contiguous text section used as the unit for independent chunking.
struct PreSection<'a> {
    body: &'a str,
}

/// Split a large document at top-level (`# `) headings so each section
/// can be chunked independently with much lower cost.
///
/// Only H1 headings are used as split points (not H2+).  Splitting at H2
/// would produce many more, smaller sections, increasing the risk that a
/// kreuzberg chunk spans a section boundary in ways that corrupt heading
/// context.  H1 sections are large enough to make the O(n^2) cost of
/// kreuzberg's heading map prohibitive while still being small enough to
/// chunk efficiently; H2+ sections rarely exceed the threshold on their own.
fn presplit_at_headings(text: &str) -> Vec<PreSection<'_>> {
    let mut sections = Vec::new();
    let mut section_start = 0;
    let bytes = text.as_bytes();
    let mut fence: Option<(u8, usize)> = None;

    // Check if the document starts with a fence (no preceding newline).
    if let Some(&ch) = bytes.first()
        && (ch == b'`' || ch == b'~')
    {
        let len = bytes.iter().take_while(|&&b| b == ch).count();
        if len >= 3 {
            fence = Some((ch, len));
        }
    }

    for i in 0..bytes.len() {
        if bytes[i] != b'\n' {
            continue;
        }

        if let Some(&fence_ch) = bytes.get(i + 1)
            && (fence_ch == b'`' || fence_ch == b'~')
        {
            let run_len = bytes[i + 1..]
                .iter()
                .take_while(|&&b| b == fence_ch)
                .count();
            if run_len >= 3 {
                match fence {
                    Some((open_ch, open_len)) if open_ch == fence_ch && run_len >= open_len => {
                        fence = None; // close
                    }
                    None => {
                        fence = Some((fence_ch, run_len)); // open
                    }
                    _ => {} // different char or shorter run -- not a close
                }
                continue;
            }
        }

        if fence.is_some() {
            continue;
        }

        if bytes.get(i + 1) == Some(&b'#') && bytes.get(i + 2) == Some(&b' ') {
            if i > section_start {
                sections.push(PreSection {
                    body: &text[section_start..i],
                });
            }
            section_start = i + 1; // include the "# heading" line
            if !text.is_char_boundary(section_start) {
                warn!("pre-split at non-char-boundary offset {section_start}, returning unsplit");
                return vec![PreSection { body: text }];
            }
        }
    }

    if section_start < text.len() {
        sections.push(PreSection {
            body: &text[section_start..],
        });
    }

    sections
}

/// Run kreuzberg chunking on a single section, applying drop-set and size filters.
fn chunk_section(
    text: &str,
    attrs: &ChunkAttrs,
    parent_heading: Option<&str>,
    drop_set: &[String],
    max_chunk_chars: usize,
    min_chunk_chars: usize,
) -> Vec<Chunk> {
    // Callers (chunk_markdown_content, presplit path) already strip leading/
    // trailing newlines before calling here, so no redundant trim is needed.
    if text.is_empty() {
        return Vec::new();
    }

    let chunker_type = if has_markdown_headings(text) {
        ChunkerType::Markdown
    } else {
        ChunkerType::Text
    };
    let kz_config = ChunkingConfig::new(max_chunk_chars, 0, false)
        .with_chunker_type(chunker_type)
        .with_prepend_heading_context(false);

    let result = match chunk_text(text, &kz_config, None) {
        Ok(r) => r,
        Err(e) => {
            warn!(
                source = attrs.source_id.as_str(),
                "kreuzberg chunking failed: {e}"
            );
            return vec![make_chunk_raw(
                attrs,
                text,
                parent_heading.filter(|s| !s.is_empty()).map(Arc::from),
                0,
            )];
        }
    };

    let mut chunks: Vec<Chunk> = Vec::with_capacity(result.chunks.len());
    let mut section_cache: HashMap<String, Arc<str>> = HashMap::new();
    let mut section_buf = String::new();
    let mut carry_start: Option<usize> = None;
    for kz_chunk in &result.chunks {
        let headings = kz_chunk
            .metadata
            .heading_context
            .as_ref()
            .map(|ctx| ctx.headings.as_slice())
            .unwrap_or_default();

        let dominated_by_drop = headings
            .iter()
            .any(|h| drop_set.iter().any(|d| d.eq_ignore_ascii_case(&h.text)));
        if dominated_by_drop {
            carry_start = None;
            continue;
        }

        section_buf.clear();
        if let Some(ph) = parent_heading.filter(|s| !s.is_empty()) {
            section_buf.push_str(ph);
        }
        for h in headings {
            if !section_buf.is_empty() {
                section_buf.push_str(" > ");
            }
            section_buf.push_str(&h.text);
        }
        let section_arc: Option<Arc<str>> = if section_buf.is_empty() {
            None
        } else {
            Some(
                section_cache
                    .entry(std::mem::take(&mut section_buf))
                    .or_insert_with_key(|k| Arc::from(k.as_str()))
                    .clone(),
            )
        };

        let chunk_content = kz_chunk
            .content
            .trim_matches(|c: char| c == '\n' || c == '\r');

        if chunk_content.len() < min_chunk_chars {
            if carry_start.is_none() {
                carry_start = Some(kz_chunk.metadata.byte_start);
            }
            continue;
        }

        let body_start = carry_start.take().unwrap_or(kz_chunk.metadata.byte_start);
        let body = text[body_start..kz_chunk.metadata.byte_end]
            .trim_matches(|c: char| c == '\n' || c == '\r');

        chunks.push(make_chunk_raw(
            attrs,
            body,
            section_arc,
            chunks.len() as i64,
        ));
    }

    chunks
}

fn make_chunk_raw(
    attrs: &ChunkAttrs,
    body: &str,
    section: Option<Arc<str>>,
    position: i64,
) -> Chunk {
    Chunk {
        chunk_id: chunk_id(&attrs.source_id, body),
        source_id: attrs.source_id.clone(),
        source: attrs.source.clone(),
        origin: attrs.origin,
        kind: attrs.kind,
        body: body.to_owned(),
        chunk_index: position,
        section,
        format: attrs.format.clone(),
        topic: attrs.topic.clone(),
        author: attrs.author.clone(),
        lang: attrs.lang.clone(),
        tags: attrs.tags.clone(),
        title: attrs.title.clone(),
        created_at: attrs.created_at.clone(),
        updated_at: None,
        llm_summary: None,
        llm_tags: None,
        llm_quality_score: None,
    }
}

#[cfg(fuzzing)]
#[doc(hidden)]
pub fn fuzz_entry(content: &str) {
    let attrs = ChunkAttrs {
        source_id: SourceId::from("fuzz"),
        source: Arc::from("fuzz.md"),
        origin: crate::types::SourceType::Local,
        kind: crate::types::DocKind::Document,
        format: Some(Arc::from("md")),
        title: None,
        author: None,
        lang: None,
        created_at: None,
        tags: None,
        topic: Some(Arc::from("fuzz")),
    };
    drop(chunk_markdown_content(content, &attrs, &[], 1024, 0));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProcessingProfile;
    use crate::ingest::types::LoaderResult;

    fn chunk_markdown(
        doc: &LoaderResult,
        topic_name: &str,
        profile: &ProcessingProfile,
    ) -> Vec<Chunk> {
        let drop_set = resolve_headings(&profile.drop_sections);
        let attrs = ChunkAttrs {
            source_id: doc.source_id.clone(),
            source: Arc::from(doc.source.as_str()),
            origin: doc.origin,
            kind: doc.kind,
            format: doc.format.as_deref().map(Arc::from),
            title: doc.title.as_deref().map(Arc::from),
            author: doc.author.as_deref().map(Arc::from),
            lang: doc.lang.as_deref().map(Arc::from),
            created_at: doc.created_at.as_deref().map(Arc::from),
            tags: doc.tags.as_deref().map(Arc::from),
            topic: Some(Arc::from(topic_name)),
        };
        chunk_markdown_content(
            &doc.content,
            &attrs,
            &drop_set,
            profile.max_chunk_chars,
            profile.min_chunk_chars,
        )
    }

    #[test]
    fn chunking_plain_text() {
        let doc = LoaderResult::test_doc("This is a paragraph.\n\nThis is another paragraph.");
        let chunks = chunk_markdown(&doc, "test", &ProcessingProfile::default());
        assert!(
            !chunks.is_empty(),
            "should produce at least one chunk from two paragraphs"
        );
        let combined: String = chunks
            .iter()
            .map(|c| c.body.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            combined.contains("paragraph"),
            "combined body should contain 'paragraph'"
        );
        assert_eq!(chunks[0].origin, crate::types::SourceType::Local);
        assert_eq!(chunks[0].topic.as_deref(), Some("test"));
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.chunk_index, i as i64, "chunk_index mismatch at {i}");
        }
    }

    #[test]
    fn chunking_header_splitting() {
        // Flat H1/H1 split: two top-level sections must yield distinct section labels.
        let cfg = ProcessingProfile {
            min_chunk_chars: 0,
            max_chunk_chars: 300,
            ..Default::default()
        };
        let flat = concat!(
            "# First\n\n",
            "Content one with enough text to form its own chunk. ",
            "Adding more words to ensure kreuzberg will split this into a separate section. ",
            "We need quite a lot of words here to push past the chunk size threshold.\n\n",
            "# Second\n\n",
            "Content two with enough text to form its own chunk. ",
            "Similarly adding more words so kreuzberg produces a second distinct chunk. ",
            "Padding the section to guarantee it exceeds the maximum chunk character limit.",
        );
        let chunks = chunk_markdown(&LoaderResult::test_doc(flat), "test", &cfg);
        assert!(
            chunks.len() >= 2,
            "expected >=2 chunks, got {}",
            chunks.len()
        );
        assert!(
            chunks
                .iter()
                .any(|c| c.section.as_deref().unwrap_or("").contains("First"))
        );
        assert!(
            chunks
                .iter()
                .any(|c| c.section.as_deref().unwrap_or("").contains("Second"))
        );

        // Nested H1/H2/H3: grandchild chunk must carry the full heading path.
        let nested = concat!(
            "# Parent\n\n",
            "Parent text with enough content to form its own chunk. ",
            "Adding more words to make sure kreuzberg produces separate chunks per section.\n\n",
            "## Child\n\n",
            "Child text with enough content to form its own chunk. ",
            "We need substantial content here to trigger separate chunk creation.\n\n",
            "### Grandchild\n\n",
            "Grandchild text with enough content to form its own chunk. ",
            "More text here to ensure kreuzberg creates a separate chunk for this section.",
        );
        let ncfg = ProcessingProfile {
            min_chunk_chars: 0,
            max_chunk_chars: 200,
            ..Default::default()
        };
        let nchunks = chunk_markdown(&LoaderResult::test_doc(nested), "test", &ncfg);
        assert!(!nchunks.is_empty());
        let grandchild = nchunks
            .iter()
            .find(|c| c.body.contains("Grandchild text"))
            .expect("should have chunk with Grandchild text");
        let grandchild_section = grandchild.section.as_deref().unwrap_or("");
        assert!(
            grandchild_section.contains("Parent"),
            "section: {grandchild_section}"
        );
        assert!(
            grandchild_section.contains("Grandchild"),
            "section: {grandchild_section}"
        );
    }

    #[test]
    fn chunking_drop_sections() {
        let dcfg = ProcessingProfile {
            min_chunk_chars: 0,
            max_chunk_chars: 300,
            drop_sections: vec!["references".to_owned()],
            ..Default::default()
        };

        // Direct drop: the dropped section must not appear; a sibling section must survive.
        let flat_drop = concat!(
            "# References\n\n",
            "Some refs that should be dropped because references is boilerplate. ",
            "Adding enough words here to make this section large enough that kreuzberg ",
            "treats it as a real separate chunk when splitting the document.\n\n",
            "# Real\n\n",
            "Real content that must be kept in the output. Adding enough words here ",
            "to make this section large enough that kreuzberg treats it as a real ",
            "separate chunk and does not merge it with the preceding section.",
        );
        let flat_chunks = chunk_markdown(&LoaderResult::test_doc(flat_drop), "test", &dcfg);
        assert!(
            !flat_chunks.iter().any(|c| c
                .section
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains("references")),
            "references section should be dropped, sections: {:?}",
            flat_chunks
                .iter()
                .map(|c| c.section.as_deref().unwrap_or(""))
                .collect::<Vec<_>>()
        );
        assert!(flat_chunks.iter().any(|c| c.body.contains("Real content")));

        // Transitive drop: sub-sections under a dropped parent must also be dropped.
        let nested_drop = concat!(
            "# References\n\n",
            "References intro text with enough words to form a chunk on its own. ",
            "We need the section to be substantial for kreuzberg to split it.\n\n",
            "## Sub-section\n\n",
            "Dropped sub content that must not appear in results. ",
            "Adding more words to ensure kreuzberg creates a real chunk here.\n\n",
            "# Keep\n\n",
            "Keep content that must appear in results. ",
            "Adding more words to ensure kreuzberg creates a real chunk for this section.",
        );
        let nested_chunks = chunk_markdown(&LoaderResult::test_doc(nested_drop), "test", &dcfg);
        assert!(
            !nested_chunks
                .iter()
                .any(|c| c.body.contains("Dropped sub content")),
            "sub-section under dropped parent must also be dropped"
        );
        assert!(
            nested_chunks
                .iter()
                .any(|c| c.body.contains("Keep content"))
        );
    }

    #[test]
    fn chunking_presplit_unclosed_fence() {
        let preamble = "a ".repeat(300 * 1024 / 2); // ~300 KB of padding before the fence
        let unclosed_fence_content =
            format!("{preamble}\n\n```\n# This heading is inside an unclosed fence\ncode\n");
        let sections = presplit_at_headings(&unclosed_fence_content);
        assert_eq!(
            sections.len(),
            1,
            "unclosed fence must suppress splits: got {} sections",
            sections.len()
        );
    }

    #[test]
    fn chunking_fenced_pseudoheader_ignored() {
        let fenced = concat!(
            "# Real\n\n",
            "Content with enough words to form a real chunk on its own right here.\n\n",
            "```\n# Not a header\ncode\n```\n\n",
            "# Another\n\n",
            "More content with enough words to form a real chunk on its own right here.",
        );
        let cfg = ProcessingProfile {
            min_chunk_chars: 0,
            max_chunk_chars: 200,
            ..Default::default()
        };
        let fchunks = chunk_markdown(&LoaderResult::test_doc(fenced), "test", &cfg);
        assert!(!fchunks.is_empty(), "expected chunks from fenced content");
        assert!(
            !fchunks
                .iter()
                .any(|c| c.section.as_deref().unwrap_or("").contains("Not a header")),
            "fenced pseudo-header must not become a section, sections: {:?}",
            fchunks
                .iter()
                .map(|c| c.section.as_deref().unwrap_or(""))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn chunking_bom_stripped() {
        let bom_content =
            "\u{FEFF}# Title\n\nBody text that is long enough to produce at least one chunk from kreuzberg without hitting the minimum character threshold.".to_owned();
        let bcfg = ProcessingProfile {
            min_chunk_chars: 0,
            ..Default::default()
        };
        let bchunks = chunk_markdown(&LoaderResult::test_doc(&bom_content), "test", &bcfg);
        assert!(!bchunks.is_empty());
        assert!(
            !bchunks.iter().any(|c| c.body.contains('\u{FEFF}')),
            "BOM must be stripped from all chunk bodies"
        );
        let combined: String = bchunks
            .iter()
            .map(|c| c.body.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            combined.contains("Body text"),
            "content after BOM should be preserved: {combined}"
        );
    }

    #[test]
    fn chunking_presplit_no_duplicate_heading() {
        let repeated = "x ".repeat(30);
        let presplit_content = format!("# My Section\n\n{repeated}\n\n{repeated}\n\n{repeated}");
        let pcfg = ProcessingProfile {
            min_chunk_chars: 0,
            max_chunk_chars: 50,
            ..Default::default()
        };
        let pchunks = chunk_markdown(&LoaderResult::test_doc(&presplit_content), "test", &pcfg);
        assert!(!pchunks.is_empty(), "should produce chunks");
        for chunk in &pchunks {
            let section = chunk.section.as_deref().unwrap_or("");
            assert!(
                !section.contains("My Section > My Section"),
                "section path must not duplicate heading: {section}"
            );
            assert!(
                section.contains("My Section"),
                "every chunk should be under My Section: {section}"
            );
        }
    }
}
