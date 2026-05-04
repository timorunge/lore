/// BM25 search field definition: field name, boost weight, and purpose.
pub struct SearchFieldDef {
    pub name: &'static str,
    pub boost: f64,
    pub purpose: &'static str,
}

/// BM25 search fields with their boost weights and human-readable purpose.
pub const SEARCH_FIELDS: &[SearchFieldDef] = &[
    SearchFieldDef {
        name: fields::BODY,
        boost: 1.0,
        purpose: "Chunk content",
    },
    SearchFieldDef {
        name: fields::TITLE,
        boost: 3.0,
        purpose: "Document title",
    },
    SearchFieldDef {
        name: fields::SECTION,
        boost: 2.0,
        purpose: "Section heading",
    },
    SearchFieldDef {
        name: fields::TAGS,
        boost: 2.0,
        purpose: "Document and chunk tags",
    },
    SearchFieldDef {
        name: fields::LLM_SUMMARY,
        boost: 1.5,
        purpose: "LLM-generated summary",
    },
    SearchFieldDef {
        name: fields::LLM_TAGS,
        boost: 1.5,
        purpose: "LLM-generated keywords",
    },
];

/// Tantivy field name constants for the lore store schema.
pub(crate) mod fields {
    pub const CHUNK_ID: &str = "chunk_id";
    pub const SOURCE_ID: &str = "source_id";
    pub const SOURCE: &str = "source";
    pub const ORIGIN: &str = "origin";
    pub const KIND: &str = "kind";
    pub const FORMAT: &str = "format";
    pub const TOPIC: &str = "topic";
    pub const AUTHOR: &str = "author";
    pub const LANG: &str = "lang";
    pub const TAGS: &str = "tags";
    pub const TITLE: &str = "title";
    pub const SECTION: &str = "section";
    pub const BODY: &str = "body";
    pub const CHUNK_INDEX: &str = "chunk_index";
    pub const CREATED_AT: &str = "created_at";
    pub const UPDATED_AT: &str = "updated_at";
    pub const LLM_SUMMARY: &str = "llm_summary";
    pub const LLM_TAGS: &str = "llm_tags";
    pub const LLM_QUALITY_SCORE: &str = "llm_quality_score";
}

use anyhow::{Context, Result};
use tantivy::schema::{
    FAST, Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions, Value,
};
use tantivy::{Index, TantivyDocument};

use crate::types::IndexLanguage;

/// Tokenizer name registered on the index -- referenced in `build_schema()`.
pub(super) const TOKENIZER_NAME: &str = "lore_stem";

/// Resolved Tantivy field handles for the lore schema.
pub(super) struct Fields {
    pub chunk_id: Field,
    pub source_id: Field,
    pub source: Field,
    pub origin: Field,
    pub kind: Field,
    pub format: Field,
    pub topic: Field,
    pub author: Field,
    pub lang: Field,
    pub tags: Field,
    pub title: Field,
    pub section: Field,
    pub body: Field,
    pub chunk_index: Field,
    pub created_at: Field,
    pub updated_at: Field,
    pub llm_summary: Field,
    pub llm_tags: Field,
    pub llm_quality_score: Field,
}

impl Fields {
    /// Resolve all field handles from an existing Tantivy schema.
    pub(super) fn from_schema(schema: &Schema) -> Result<Self> {
        Ok(Self {
            chunk_id: schema
                .get_field(fields::CHUNK_ID)
                .with_context(|| format!("missing field: {}", fields::CHUNK_ID))?,
            source_id: schema
                .get_field(fields::SOURCE_ID)
                .with_context(|| format!("missing field: {}", fields::SOURCE_ID))?,
            source: schema
                .get_field(fields::SOURCE)
                .with_context(|| format!("missing field: {}", fields::SOURCE))?,
            origin: schema
                .get_field(fields::ORIGIN)
                .with_context(|| format!("missing field: {}", fields::ORIGIN))?,
            kind: schema
                .get_field(fields::KIND)
                .with_context(|| format!("missing field: {}", fields::KIND))?,
            format: schema
                .get_field(fields::FORMAT)
                .with_context(|| format!("missing field: {}", fields::FORMAT))?,
            topic: schema
                .get_field(fields::TOPIC)
                .with_context(|| format!("missing field: {}", fields::TOPIC))?,
            author: schema
                .get_field(fields::AUTHOR)
                .with_context(|| format!("missing field: {}", fields::AUTHOR))?,
            lang: schema
                .get_field(fields::LANG)
                .with_context(|| format!("missing field: {}", fields::LANG))?,
            tags: schema
                .get_field(fields::TAGS)
                .with_context(|| format!("missing field: {}", fields::TAGS))?,
            title: schema
                .get_field(fields::TITLE)
                .with_context(|| format!("missing field: {}", fields::TITLE))?,
            section: schema
                .get_field(fields::SECTION)
                .with_context(|| format!("missing field: {}", fields::SECTION))?,
            body: schema
                .get_field(fields::BODY)
                .with_context(|| format!("missing field: {}", fields::BODY))?,
            chunk_index: schema
                .get_field(fields::CHUNK_INDEX)
                .with_context(|| format!("missing field: {}", fields::CHUNK_INDEX))?,
            created_at: schema
                .get_field(fields::CREATED_AT)
                .with_context(|| format!("missing field: {}", fields::CREATED_AT))?,
            updated_at: schema
                .get_field(fields::UPDATED_AT)
                .with_context(|| format!("missing field: {}", fields::UPDATED_AT))?,
            llm_summary: schema
                .get_field(fields::LLM_SUMMARY)
                .with_context(|| format!("missing field: {}", fields::LLM_SUMMARY))?,
            llm_tags: schema
                .get_field(fields::LLM_TAGS)
                .with_context(|| format!("missing field: {}", fields::LLM_TAGS))?,
            llm_quality_score: schema
                .get_field(fields::LLM_QUALITY_SCORE)
                .with_context(|| format!("missing field: {}", fields::LLM_QUALITY_SCORE))?,
        })
    }

    /// Resolve `SEARCH_FIELDS` to Tantivy `Field` handles with boost weights.
    pub(super) fn search_fields(&self) -> Vec<(Field, f64)> {
        SEARCH_FIELDS
            .iter()
            .map(|def| {
                let field = match def.name {
                    fields::BODY => self.body,
                    fields::TITLE => self.title,
                    fields::SECTION => self.section,
                    fields::TAGS => self.tags,
                    fields::LLM_SUMMARY => self.llm_summary,
                    fields::LLM_TAGS => self.llm_tags,
                    _ => panic!("unknown search field: {:?}", def.name),
                };
                (field, def.boost)
            })
            .collect()
    }
}

/// Strip field prefixes from a token. Preserves URLs (`://`) while stripping
/// injections. Loops to handle nested prefixes like `x:title:secret`.
fn strip_field_prefix(mut token: &str) -> &str {
    loop {
        let Some(colon_pos) = token.find(':') else {
            return token;
        };
        if token[colon_pos..].starts_with("://") {
            return token;
        }
        token = &token[colon_pos + 1..];
    }
}

/// Sanitize user queries before passing them to Tantivy's QueryParser.
///
/// Strips field prefixes (`body:`, `source:`, etc.) to prevent field injection
/// and range brackets (`[`, `]`) to prevent range query injection.
///
/// Preserves phrase quotes (`"`), wildcards (`*`, `?`), boost (`^`), fuzzy
/// (`~`), and boolean operators (AND, OR, NOT) -- these are useful Tantivy
/// features that are safe when the QueryParser is locked to the search field
/// set.
pub(crate) fn sanitize_query(q: &str) -> String {
    let stripped: String = q.chars().filter(|&c| c != '[' && c != ']').collect();

    stripped
        .split_whitespace()
        .map(|token| {
            let token = token.trim_start_matches(&['+', '-', '('] as &[char]);
            let token = token.trim_end_matches(')');
            let token = strip_field_prefix(token);
            let token = token.trim_start_matches('(');
            token.to_string()
        })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Extract the first stored text value for `field`, returning an empty string if the field
/// is absent or contains no text value. Use `get_text_opt` when an absent field should be
/// distinguished from an empty string.
pub(crate) fn get_text(doc: &TantivyDocument, field: Field) -> String {
    doc.get_first(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned()
}

/// Extract the first text value for `field`, returning `None` if absent.
pub(crate) fn get_text_opt(doc: &TantivyDocument, field: Field) -> Option<String> {
    doc.get_first(field)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(std::borrow::ToOwned::to_owned)
}

/// Extract the first i64 value for `field`, defaulting to 0 if absent.
pub(crate) fn get_i64(doc: &TantivyDocument, field: Field) -> i64 {
    doc.get_first(field).and_then(|v| v.as_i64()).unwrap_or(0)
}

/// Extract the first f64 value for `field`, returning `None` if absent.
pub(crate) fn get_f64_opt(doc: &TantivyDocument, field: Field) -> Option<f64> {
    doc.get_first(field).and_then(|v| v.as_f64())
}

/// Register the stemming tokenizer used for BM25 search.
///
/// IMPORTANT: Any change to the tokenizer pipeline (filters, language, or
/// tokenizer kind) alters how terms are stored on disk. Existing stores
/// must be rebuilt with `lore ingest --recreate` after any change.
pub(super) fn register_tokenizer(index: &Index, language: IndexLanguage) {
    use tantivy::tokenizer::{
        LowerCaser, RemoveLongFilter, SimpleTokenizer, Stemmer, StopWordFilter, TextAnalyzer,
    };

    let Some(lang) = language.to_tantivy() else {
        // IndexLanguage::None -- whitespace + lowercase only.
        index.tokenizers().register(
            TOKENIZER_NAME,
            TextAnalyzer::builder(SimpleTokenizer::default())
                .filter(RemoveLongFilter::limit(100))
                .filter(LowerCaser)
                .build(),
        );
        return;
    };

    let analyzer = if let Some(sw) = StopWordFilter::new(lang) {
        TextAnalyzer::builder(SimpleTokenizer::default())
            .filter(RemoveLongFilter::limit(100))
            .filter(LowerCaser)
            .filter(sw)
            .filter(Stemmer::new(lang))
            .build()
    } else {
        TextAnalyzer::builder(SimpleTokenizer::default())
            .filter(RemoveLongFilter::limit(100))
            .filter(LowerCaser)
            .filter(Stemmer::new(lang))
            .build()
    };
    index.tokenizers().register(TOKENIZER_NAME, analyzer);
}

/// Build the Tantivy schema for a lore store.
///
/// Existing stores must be rebuilt with `--recreate` if fields or tokenizer change.
pub(super) fn build_schema(phrase_search: bool) -> Schema {
    let mut builder = Schema::builder();

    let index_option = if phrase_search {
        IndexRecordOption::WithFreqsAndPositions
    } else {
        IndexRecordOption::WithFreqs
    };
    let text_opts = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(TOKENIZER_NAME)
                .set_index_option(index_option),
        )
        .set_stored();
    let stored_text = TextOptions::default().set_stored();
    let string_opts = STRING | STORED;

    // STRING | STORED allows exact-match TermQuery lookups on chunk_id.
    builder.add_text_field(fields::CHUNK_ID, string_opts.clone());
    builder.add_text_field(fields::SOURCE_ID, STRING);
    builder.add_text_field(fields::SOURCE, string_opts.clone() | FAST);
    builder.add_text_field(fields::ORIGIN, string_opts.clone());
    builder.add_text_field(fields::KIND, string_opts.clone());
    builder.add_text_field(fields::FORMAT, string_opts.clone());
    builder.add_text_field(fields::TOPIC, string_opts.clone());
    builder.add_text_field(fields::AUTHOR, string_opts.clone());
    builder.add_text_field(fields::LANG, string_opts);
    builder.add_text_field(fields::TAGS, text_opts.clone());
    builder.add_text_field(fields::TITLE, text_opts.clone());
    builder.add_text_field(fields::SECTION, text_opts.clone());
    builder.add_text_field(fields::BODY, text_opts.clone());
    builder.add_i64_field(fields::CHUNK_INDEX, STORED | FAST);
    builder.add_text_field(fields::CREATED_AT, stored_text.clone());
    builder.add_text_field(fields::UPDATED_AT, stored_text);
    builder.add_text_field(fields::LLM_SUMMARY, text_opts.clone());
    builder.add_text_field(fields::LLM_TAGS, text_opts);
    builder.add_f64_field(fields::LLM_QUALITY_SCORE, STORED);

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::sanitize_query;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(256))]

        #[test]
        fn prop_sanitize_query_no_brackets(s in ".*") {
            let result = sanitize_query(&s);
            prop_assert!(!result.contains('['), "output contains '[': {:?}", result);
            prop_assert!(!result.contains(']'), "output contains ']': {:?}", result);
            // URL-like inputs: the "://" scheme marker must survive sanitization.
            if s.contains("://") {
                prop_assert!(result.contains("://"), "URL scheme lost in: {:?} -> {:?}", s, result);
            }
            // Sanitization only removes/trims characters, never adds new ones.
            prop_assert!(result.len() <= s.len(), "result longer than input: {:?} -> {:?}", s, result);
        }
    }

    #[test]
    fn sanitize_query_cases() {
        let cases = [
            // Field prefix stripping
            ("source:secret rust", "secret rust"),
            ("normal query", "normal query"),
            ("topic:foo author:bar baz", "foo bar baz"),
            ("SOURCE:/etc/passwd", "/etc/passwd"),
            ("hash:abc123", "abc123"),
            ("source: rest", "rest"),
            ("body:secret", "secret"),
            ("unknownfield:value", "value"),
            // URL preservation
            ("http://example.com", "http://example.com"),
            ("https://example.com", "https://example.com"),
            ("body:https://evil.com", "https://evil.com"),
            // Parenthesized field prefix bypass
            ("source:(foo)", "foo"),
            ("source:(foo bar)", "foo bar"),
            ("(source:secret)", "secret"),
            ("(hello)", "hello"),
            // +/- operators
            ("+source:secret", "secret"),
            ("-hash:abc123 query", "abc123 query"),
            ("+(source:secret)", "secret"),
            // Nested field prefixes
            ("x:title:secret", "secret"),
            ("body:title:secret", "secret"),
            // Range brackets
            ("[a TO z]", "a TO z"),
            ("foo:[bar TO baz] qux", "bar TO baz qux"),
            // Phrase quotes are preserved
            ("\"exact phrase\"", "\"exact phrase\""),
            ("before \"inside\" after", "before \"inside\" after"),
            // Wildcards are preserved
            ("*", "*"),
            ("*secret", "*secret"),
            ("*rust*", "*rust*"),
            ("ru*st", "ru*st"),
            ("ru?st", "ru?st"),
            ("hel*lo wor?ld", "hel*lo wor?ld"),
            // Boost and fuzzy are preserved
            ("rust^2 programming", "rust^2 programming"),
            ("rust~2", "rust~2"),
        ];
        for (input, expected) in cases {
            assert_eq!(sanitize_query(input), expected, "sanitize_query({input:?})");
        }
    }

    /// CI guard: fails if the Tantivy schema changes accidentally.
    ///
    /// If this test fails, update `EXPECTED` below to the new fingerprint
    /// printed in the failure message.
    #[test]
    fn schema_fingerprint_matches_expected() {
        let schema = super::build_schema(true);
        let mut entries: Vec<String> = schema
            .fields()
            .map(|(_field, entry)| {
                let name = entry.name().to_owned();
                let type_json =
                    serde_json::to_string(entry.field_type()).expect("field type must serialize");
                format!("{name}:{type_json}")
            })
            .collect();
        entries.sort();
        let fingerprint = entries.join("\n");

        let expected = concat!(
            "author:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"basic\",\"fieldnorms\":true,\"tokenizer\":\"raw\"},\"stored\":true,\"fast\":false}}\n",
            "body:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"position\",\"fieldnorms\":true,\"tokenizer\":\"lore_stem\"},\"stored\":true,\"fast\":false}}\n",
            "chunk_id:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"basic\",\"fieldnorms\":true,\"tokenizer\":\"raw\"},\"stored\":true,\"fast\":false}}\n",
            "chunk_index:{\"type\":\"i64\",\"options\":{\"indexed\":false,\"fieldnorms\":false,\"fast\":true,\"stored\":true}}\n",
            "created_at:{\"type\":\"text\",\"options\":{\"stored\":true,\"fast\":false}}\n",
            "format:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"basic\",\"fieldnorms\":true,\"tokenizer\":\"raw\"},\"stored\":true,\"fast\":false}}\n",
            "kind:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"basic\",\"fieldnorms\":true,\"tokenizer\":\"raw\"},\"stored\":true,\"fast\":false}}\n",
            "lang:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"basic\",\"fieldnorms\":true,\"tokenizer\":\"raw\"},\"stored\":true,\"fast\":false}}\n",
            "llm_quality_score:{\"type\":\"f64\",\"options\":{\"indexed\":false,\"fieldnorms\":false,\"fast\":false,\"stored\":true}}\n",
            "llm_summary:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"position\",\"fieldnorms\":true,\"tokenizer\":\"lore_stem\"},\"stored\":true,\"fast\":false}}\n",
            "llm_tags:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"position\",\"fieldnorms\":true,\"tokenizer\":\"lore_stem\"},\"stored\":true,\"fast\":false}}\n",
            "origin:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"basic\",\"fieldnorms\":true,\"tokenizer\":\"raw\"},\"stored\":true,\"fast\":false}}\n",
            "section:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"position\",\"fieldnorms\":true,\"tokenizer\":\"lore_stem\"},\"stored\":true,\"fast\":false}}\n",
            "source:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"basic\",\"fieldnorms\":true,\"tokenizer\":\"raw\"},\"stored\":true,\"fast\":true}}\n",
            "source_id:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"basic\",\"fieldnorms\":true,\"tokenizer\":\"raw\"},\"stored\":false,\"fast\":false}}\n",
            "tags:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"position\",\"fieldnorms\":true,\"tokenizer\":\"lore_stem\"},\"stored\":true,\"fast\":false}}\n",
            "title:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"position\",\"fieldnorms\":true,\"tokenizer\":\"lore_stem\"},\"stored\":true,\"fast\":false}}\n",
            "topic:{\"type\":\"text\",\"options\":{\"indexing\":{\"record\":\"basic\",\"fieldnorms\":true,\"tokenizer\":\"raw\"},\"stored\":true,\"fast\":false}}\n",
            "updated_at:{\"type\":\"text\",\"options\":{\"stored\":true,\"fast\":false}}",
        );

        assert_eq!(
            fingerprint, expected,
            "Schema changed. Update expected fingerprint in this test.\n\
             New fingerprint:\n{fingerprint}"
        );
    }
}
