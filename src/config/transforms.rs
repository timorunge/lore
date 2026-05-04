use serde::{Deserialize, Serialize};

/// Metadata extraction mode.
#[derive(
    Debug, Clone, Copy, Deserialize, Serialize, Default, PartialEq, Eq, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ExtractMode {
    /// Text: builtin metadata extraction. Binary: kreuzberg metadata + builtin extraction + merge.
    #[default]
    Auto,
    /// Text: builtin metadata extraction only. Binary: builtin metadata extraction only (kreuzberg not called).
    Builtin,
    /// Text: no metadata extraction. Binary: kreuzberg full extraction (document parsing + metadata).
    Kreuzberg,
    /// Text: no metadata extraction. Binary: no metadata extraction; use `extract_builtin` in a pipeline for explicit control.
    None,
}

/// Strip leading and/or trailing lines from content.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StripLines {
    /// Number of lines to remove from the beginning of the content.
    #[serde(default)]
    pub first: usize,
    /// Number of lines to remove from the end of the content.
    #[serde(default)]
    pub last: usize,
}

/// Regex find-and-replace on content.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceText {
    pub pattern: String,
    #[serde(default)]
    pub replacement: String,
}

/// Custom regex metadata extraction. First-wins per field.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtractMetadata {
    pub pattern: String,
    pub field: MetadataField,
}

/// A single pipeline transform step, applied in declaration order.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Transform {
    /// Strip leading and/or trailing lines from content.
    StripLines(StripLines),
    /// Regex find-and-replace on content.
    ReplaceText(ReplaceText),
    /// Run builtin metadata extraction (YAML frontmatter, org-mode, RFC headers,
    /// heuristics) on current content. First-wins: only fills empty fields.
    /// Useful with `extract: none` to control when extraction runs in the pipeline.
    ExtractBuiltin,
    /// Custom regex metadata extraction. First-wins per field.
    ExtractMetadata(ExtractMetadata),
}

/// Target field for custom regex metadata extraction.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MetadataField {
    Title,
    Author,
    Lang,
    Tags,
    Topic,
}
