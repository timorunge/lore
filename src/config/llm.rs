use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::Validate;
use crate::util::half_cores;

// Update periodically as new models are released.
const DEFAULT_BEDROCK_MODEL: &str = "anthropic.claude-haiku-4-5-20251001-v1:0";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5-20251001";
const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
const DEFAULT_OLLAMA_MODEL: &str = "llama3.2";
pub(crate) const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

/// LLM-based topic detection settings.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct DetectTopicsConfig {
    /// Enable LLM topic detection (default: false).
    pub enabled: bool,
    /// Maximum tokens for the LLM response (default: 50).
    pub max_tokens: u32,
    /// Maximum document characters sent to the LLM (default: 1500).
    pub max_content_chars: usize,
    /// Custom system prompt override for topic detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

impl Default for DetectTopicsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_tokens: 50,
            max_content_chars: 1500,
            prompt: None,
        }
    }
}

/// LLM-based document-level summary settings.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SummarizeDocsConfig {
    /// Enable LLM document summarization (default: false).
    pub enabled: bool,
    /// Maximum tokens for the LLM response (default: 150).
    pub max_tokens: u32,
    /// Maximum document characters sent to the LLM (default: 4000).
    pub max_content_chars: usize,
    /// Custom system prompt override for document summarization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

impl Default for SummarizeDocsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_tokens: 150,
            max_content_chars: 4000,
            prompt: None,
        }
    }
}

/// LLM chunk enrichment settings (summaries, tags, quality scoring).
///
/// Note: `enabled` is only meaningful when an LLM provider is configured.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct EnrichChunksConfig {
    /// Enable LLM enrichment (summaries, tags, quality scoring) (default: false).
    pub enabled: bool,
    /// Minimum quality score (0.0--1.0) for a document to be enriched.
    pub quality_threshold: f64,
    /// Maximum parallel LLM requests (default: half of available cores).
    pub concurrency: usize,
    /// Maximum tokens for the LLM response (default: 200).
    pub max_tokens: u32,
    /// Maximum document characters sent to the LLM (default: 2000).
    pub max_content_chars: usize,
    /// Custom system prompt override for enrichment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

impl Default for EnrichChunksConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            quality_threshold: 0.3,
            concurrency: half_cores(),
            max_tokens: 200,
            max_content_chars: 2000,
            prompt: None,
        }
    }
}

/// LLM backend provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    /// AWS Bedrock managed inference (requires AWS credentials).
    Bedrock,
    /// Anthropic API (requires `ANTHROPIC_API_KEY`).
    Anthropic,
    /// OpenAI API (requires `OPENAI_API_KEY`).
    Openai,
    /// Locally hosted Ollama instance (no credentials required).
    Ollama,
}

/// LLM provider and model configuration.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct LlmConfig {
    /// LLM provider; auto-detected from available credentials when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<LlmProvider>,
    /// Model ID for AWS Bedrock.
    pub bedrock_model: String,
    /// Model ID for the Anthropic API.
    pub anthropic_model: String,
    /// Model ID for the OpenAI-compatible API.
    pub openai_model: String,
    /// Model ID for the local Ollama instance.
    pub ollama_model: String,
    /// Base URL for the local Ollama instance. Defaults to `http://localhost:11434` when the `llm` block is present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama_url: Option<String>,
    /// Total request timeout in seconds for LLM API calls (default: 120).
    pub timeout_secs: u64,
    /// TCP connect timeout in seconds for LLM API calls (default: 10).
    pub connect_timeout_secs: u64,
    pub detect_topics: DetectTopicsConfig,
    pub summarize_docs: SummarizeDocsConfig,
    pub enrich_chunks: EnrichChunksConfig,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: None,
            bedrock_model: DEFAULT_BEDROCK_MODEL.to_owned(),
            anthropic_model: DEFAULT_ANTHROPIC_MODEL.to_owned(),
            openai_model: DEFAULT_OPENAI_MODEL.to_owned(),
            ollama_model: DEFAULT_OLLAMA_MODEL.to_owned(),
            ollama_url: Some(DEFAULT_OLLAMA_URL.to_owned()),
            timeout_secs: 120,
            connect_timeout_secs: 10,
            detect_topics: DetectTopicsConfig::default(),
            summarize_docs: SummarizeDocsConfig::default(),
            enrich_chunks: EnrichChunksConfig::default(),
        }
    }
}

impl LlmConfig {
    /// Returns true if any LLM enrichment step (topic detection, doc summarization, chunk enrichment) is enabled.
    pub fn has_enrichment(&self) -> bool {
        self.detect_topics.enabled || self.summarize_docs.enabled || self.enrich_chunks.enabled
    }
}

impl Validate for LlmConfig {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.enrich_chunks.quality_threshold),
            "llm.enrich_chunks.quality_threshold must be in [0.0, 1.0], got {}",
            self.enrich_chunks.quality_threshold
        );
        anyhow::ensure!(
            self.enrich_chunks.max_tokens > 0,
            "llm.enrich_chunks.max_tokens must be > 0"
        );
        anyhow::ensure!(
            self.enrich_chunks.max_content_chars > 0,
            "llm.enrich_chunks.max_content_chars must be > 0"
        );
        anyhow::ensure!(
            self.detect_topics.max_tokens > 0,
            "llm.detect_topics.max_tokens must be > 0"
        );
        anyhow::ensure!(
            self.detect_topics.max_content_chars > 0,
            "llm.detect_topics.max_content_chars must be > 0"
        );
        anyhow::ensure!(
            self.summarize_docs.max_tokens > 0,
            "llm.summarize_docs.max_tokens must be > 0"
        );
        anyhow::ensure!(
            self.summarize_docs.max_content_chars > 0,
            "llm.summarize_docs.max_content_chars must be > 0"
        );
        anyhow::ensure!(
            self.enrich_chunks.concurrency > 0,
            "llm.enrich_chunks.concurrency must be > 0"
        );
        anyhow::ensure!(self.timeout_secs > 0, "llm.timeout_secs must be > 0");
        anyhow::ensure!(
            self.connect_timeout_secs > 0,
            "llm.connect_timeout_secs must be > 0"
        );
        if let Some(ref url) = self.ollama_url {
            anyhow::ensure!(
                url.starts_with("http://") || url.starts_with("https://"),
                "llm.ollama_url must start with 'http://' or 'https://', got {url:?}"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::type_complexity)]
    fn validate_llm_config() {
        let cases: &[(f64, bool)] = &[(-0.1, false), (0.0, true), (1.0, true), (1.1, false)];
        for &(value, expect_ok) in cases {
            let mut cfg = LlmConfig::default();
            cfg.enrich_chunks.quality_threshold = value;
            let result = Validate::validate(&cfg);
            assert_eq!(
                result.is_ok(),
                expect_ok,
                "quality_threshold={value}: expected is_ok={expect_ok}"
            );
        }

        let zero_cases: &[(&str, fn(&mut LlmConfig), &str)] = &[
            (
                "enrich.max_tokens",
                |cfg| cfg.enrich_chunks.max_tokens = 0,
                "llm.enrich_chunks.max_tokens",
            ),
            (
                "topic.max_tokens",
                |cfg| cfg.detect_topics.max_tokens = 0,
                "llm.detect_topics.max_tokens",
            ),
            (
                "summary.max_tokens",
                |cfg| cfg.summarize_docs.max_tokens = 0,
                "llm.summarize_docs.max_tokens",
            ),
            (
                "enrich.concurrency",
                |cfg| cfg.enrich_chunks.concurrency = 0,
                "llm.enrich_chunks.concurrency",
            ),
            (
                "enrich.max_content_chars",
                |cfg| cfg.enrich_chunks.max_content_chars = 0,
                "llm.enrich_chunks.max_content_chars",
            ),
            (
                "topic.max_content_chars",
                |cfg| cfg.detect_topics.max_content_chars = 0,
                "llm.detect_topics.max_content_chars",
            ),
            (
                "summary.max_content_chars",
                |cfg| cfg.summarize_docs.max_content_chars = 0,
                "llm.summarize_docs.max_content_chars",
            ),
        ];
        for &(label, set_zero, expected_msg) in zero_cases {
            let mut cfg = LlmConfig::default();
            set_zero(&mut cfg);
            let result = Validate::validate(&cfg);
            assert!(result.is_err(), "{label}: expected validation error");
            let err = result.unwrap_err();
            assert!(
                err.to_string().contains(expected_msg),
                "{label}: error {err:?} should contain {expected_msg:?}"
            );
        }

        let cfg = LlmConfig {
            ollama_url: None,
            ..LlmConfig::default()
        };
        assert!(Validate::validate(&cfg).is_ok());

        let cfg = LlmConfig {
            ollama_url: Some("https://my-ollama.example.com".to_owned()),
            ..LlmConfig::default()
        };
        assert!(Validate::validate(&cfg).is_ok());

        let cfg = LlmConfig {
            ollama_url: Some("ftp://localhost:11434".to_owned()),
            ..LlmConfig::default()
        };
        let err = Validate::validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("llm.ollama_url"));

        let cfg = LlmConfig {
            ollama_url: Some("httpfoo://localhost:11434".to_owned()),
            ..LlmConfig::default()
        };
        let err = Validate::validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("llm.ollama_url"));
    }
}
