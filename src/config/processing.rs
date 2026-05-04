use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Validate;
use crate::config::transforms::{ExtractMode, Transform};
use crate::util::half_cores;

/// Per-source processing parameters. Used for presets and inline overrides.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ProcessingProfile {
    /// Metadata extraction mode: auto, builtin, kreuzberg, or none.
    pub extract: ExtractMode,
    /// Maximum characters per chunk (passed to kreuzberg).
    pub max_chunk_chars: usize,
    /// Merge chunks shorter than this into the next chunk.
    pub min_chunk_chars: usize,
    /// Drop chunks under these section headings (case-insensitive).
    pub drop_sections: Vec<String>,
    /// Ordered content/metadata transform pipeline.
    #[serde(default)]
    pub pipeline: Vec<Transform>,
}

impl Default for ProcessingProfile {
    fn default() -> Self {
        Self {
            extract: ExtractMode::Auto,
            max_chunk_chars: 1600,
            min_chunk_chars: 30,
            drop_sections: Vec::new(),
            pipeline: Vec::new(),
        }
    }
}

/// Per-source processing override: a preset name (string) or inline profile (object).
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ProcessingRef {
    /// Name of a preset defined in `processing.presets`.
    Named(String),
    /// Inline profile object with processing parameters.
    Inline(ProcessingProfile),
}

/// Global processing settings and named presets.
///
/// All fields from [`ProcessingProfile`] are duplicated here (rather than
/// using `#[serde(flatten)]`) because `flatten` is incompatible with
/// `#[serde(deny_unknown_fields)]` in serde.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ProcessingConfig {
    /// Metadata extraction mode: auto, builtin, kreuzberg, or none.
    pub extract: ExtractMode,
    /// Maximum characters per chunk (passed to kreuzberg).
    pub max_chunk_chars: usize,
    /// Merge chunks shorter than this into the next chunk.
    pub min_chunk_chars: usize,
    /// Drop chunks under these section headings (case-insensitive).
    pub drop_sections: Vec<String>,
    /// Ordered content/metadata transform pipeline.
    #[serde(default)]
    pub pipeline: Vec<Transform>,

    /// Maximum file size in MB. Files larger than this are skipped.
    pub max_file_mb: usize,
    /// Maximum number of files extracted from a single archive (default: 5,000).
    pub max_archive_files: usize,
    /// Maximum total decompressed size of an archive in MB (default: 2,048 = 2 GB).
    pub max_archive_mb: usize,
    /// Timeout in seconds for file content extraction (default: 120).
    pub extraction_timeout_secs: u64,
    /// Seconds between automatic commits during ingest (default: 30).
    /// Commits persist progress so cancelled ingests can resume.
    /// Set to 0 to commit only at the end.
    pub commit_interval: u64,
    /// Maximum number of parallel chunking threads (default: half of available cores, min 2).
    pub concurrency: usize,
    /// Treat files with these extensions as plain text, bypassing kreuzberg extraction.
    pub text_extensions: Option<Vec<String>>,
    /// Content filtering for document extraction (headers, footers, repeating
    /// text, watermarks). Controls what kreuzberg strips from extracted content.
    #[serde(default)]
    pub content_filter: ContentFilterConfig,

    /// Named processing presets, reusable via per-source `processing: "name"`.
    #[serde(default)]
    pub presets: HashMap<String, ProcessingProfile>,
}

impl ProcessingConfig {
    /// Build the default profile from global config fields.
    pub fn default_profile(&self) -> ProcessingProfile {
        ProcessingProfile {
            extract: self.extract,
            max_chunk_chars: self.max_chunk_chars,
            min_chunk_chars: self.min_chunk_chars,
            drop_sections: self.drop_sections.clone(),
            pipeline: self.pipeline.clone(),
        }
    }

    /// Resolve a per-source `ProcessingRef` into a concrete profile.
    pub fn resolve(&self, r: Option<&ProcessingRef>) -> Result<ProcessingProfile> {
        match r {
            None => Ok(self.default_profile()),
            Some(ProcessingRef::Named(name)) => self
                .presets
                .get(name)
                .cloned()
                .with_context(|| format!("unknown processing preset: {name:?}")),
            Some(ProcessingRef::Inline(p)) => Ok(p.clone()),
        }
    }
}

impl Default for ProcessingConfig {
    fn default() -> Self {
        let profile = ProcessingProfile::default();
        Self {
            extract: profile.extract,
            max_chunk_chars: profile.max_chunk_chars,
            min_chunk_chars: profile.min_chunk_chars,
            drop_sections: profile.drop_sections,
            pipeline: profile.pipeline,
            max_file_mb: 50,
            max_archive_files: 5_000,
            max_archive_mb: 2_048,
            extraction_timeout_secs: 120,
            commit_interval: 30,
            concurrency: half_cores(),
            text_extensions: None,
            content_filter: ContentFilterConfig::default(),
            presets: HashMap::new(),
        }
    }
}

impl Validate for ProcessingConfig {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.max_file_mb > 0, "processing.max_file_mb must be > 0");
        anyhow::ensure!(
            self.max_archive_files > 0,
            "processing.max_archive_files must be > 0"
        );
        anyhow::ensure!(
            self.max_archive_mb > 0,
            "processing.max_archive_mb must be > 0"
        );
        anyhow::ensure!(
            self.extraction_timeout_secs > 0,
            "processing.extraction_timeout_secs must be > 0"
        );
        anyhow::ensure!(self.concurrency > 0, "processing.concurrency must be > 0");

        let defaults = self.default_profile();
        validate_profile(&defaults, "processing")?;

        for (name, preset) in &self.presets {
            validate_profile(preset, &format!("processing.presets.{name}"))?;
        }

        Ok(())
    }
}

/// Bundled processing limits derived from [`ProcessingConfig`].
#[derive(Debug, Clone, Copy)]
pub struct ProcessingLimits {
    /// Maximum file size in bytes; files larger than this are skipped.
    pub max_file_bytes: u64,
    /// Maximum number of files extracted from a single archive.
    pub max_archive_files: usize,
    /// Maximum total decompressed size of an archive in bytes.
    pub max_archive_bytes: u64,
    /// Timeout in seconds for file content extraction.
    pub extraction_timeout_secs: u64,
    /// Content-filter settings forwarded to kreuzberg.
    pub content_filter: ContentFilterConfig,
    /// Maximum number of concurrent loading tasks.
    pub concurrency: usize,
}

impl ProcessingLimits {
    /// Convert a `ProcessingConfig` into resolved byte-level limits for runtime use.
    pub fn from_config(processing: &ProcessingConfig) -> Self {
        Self {
            max_file_bytes: crate::fmt::mb_to_bytes(processing.max_file_mb),
            max_archive_files: processing.max_archive_files,
            max_archive_bytes: crate::fmt::mb_to_bytes(processing.max_archive_mb),
            extraction_timeout_secs: processing.extraction_timeout_secs,
            content_filter: processing.content_filter,
            concurrency: processing.concurrency,
        }
    }
}

/// Controls what "furniture" content kreuzberg strips during extraction.
///
/// When omitted from the config, kreuzberg defaults apply (strip headers,
/// footers, repeating text, and watermarks). Set individual fields to
/// override.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ContentFilterConfig {
    /// Include running headers in extracted text (default: false).
    pub include_headers: bool,
    /// Include running footers in extracted text (default: false).
    pub include_footers: bool,
    /// Strip text that repeats across a supermajority of pages (default: true).
    /// Disable if brand names or repeated headings are incorrectly removed
    /// from PowerPoint-exported PDFs.
    pub strip_repeating_text: bool,
    /// Include watermark text in extracted output (default: false).
    pub include_watermarks: bool,
}

impl Default for ContentFilterConfig {
    fn default() -> Self {
        Self {
            include_headers: false,
            include_footers: false,
            strip_repeating_text: true,
            include_watermarks: false,
        }
    }
}

/// Validate a processing profile's fields and pipeline regexes.
pub(super) fn validate_profile(profile: &ProcessingProfile, label: &str) -> Result<()> {
    anyhow::ensure!(
        profile.max_chunk_chars > 0,
        "{label}: max_chunk_chars must be > 0"
    );
    anyhow::ensure!(
        profile.min_chunk_chars <= profile.max_chunk_chars,
        "{label}: min_chunk_chars ({}) must be <= max_chunk_chars ({})",
        profile.min_chunk_chars,
        profile.max_chunk_chars,
    );
    for (i, step) in profile.pipeline.iter().enumerate() {
        match step {
            Transform::ReplaceText(t) => {
                regex::Regex::new(&t.pattern).with_context(|| {
                    format!("{label}.pipeline[{i}]: invalid regex: {}", t.pattern)
                })?;
            }
            Transform::ExtractMetadata(t) => {
                regex::Regex::new(&t.pattern).with_context(|| {
                    format!("{label}.pipeline[{i}]: invalid regex: {}", t.pattern)
                })?;
            }
            Transform::StripLines(_) | Transform::ExtractBuiltin => {}
        }
    }
    Ok(())
}
