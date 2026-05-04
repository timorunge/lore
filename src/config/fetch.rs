use serde::{Deserialize, Serialize};

use crate::config::Validate;
use crate::util::half_cores;

/// Configuration for HTTP fetching behavior.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct FetchConfig {
    /// Seconds between requests to the same host (default: 0.5).
    pub delay: f64,
    /// Maximum parallel HTTP requests (default: half of available cores).
    pub concurrency: usize,
    /// HTTP request timeout in seconds (default: 30.0).
    pub timeout: f64,
    /// Total timeout for streaming downloads in seconds (default: 600).
    pub download_timeout: u64,
    /// Maximum retry attempts for rate-limited (429/503) responses (default: 3).
    pub max_retries: usize,
    /// Honour robots.txt rules (default: true).
    pub respect_robots: bool,
    /// Custom User-Agent header for HTTP requests. When `None`, uses lore's built-in agent string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// Request markdown via Accept header (Cloudflare "Markdown for Agents").
    /// When true, sends `Accept: text/markdown, text/html;q=0.9, */*;q=0.8`.
    pub prefer_markdown: bool,
    /// Cache TTL in seconds; `None` disables caching (default: 3600).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_ttl: Option<f64>,
    /// Maximum HTTP response/download size in MB (default: 50).
    pub max_download_mb: usize,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            delay: 0.5,
            concurrency: half_cores(),
            timeout: 30.0,
            download_timeout: 600,
            max_retries: 3,
            respect_robots: true,
            user_agent: None,
            prefer_markdown: true,
            cache_ttl: Some(3600.0),
            max_download_mb: 50,
        }
    }
}

impl Validate for FetchConfig {
    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.concurrency > 0, "fetch.concurrency must be > 0");
        anyhow::ensure!(
            self.download_timeout > 0,
            "fetch.download_timeout must be > 0"
        );
        anyhow::ensure!(
            self.max_download_mb > 0,
            "fetch.max_download_mb must be > 0"
        );
        anyhow::ensure!(self.timeout > 0.0, "fetch.timeout must be > 0");
        anyhow::ensure!(self.timeout.is_finite(), "fetch.timeout must be finite");
        anyhow::ensure!(self.delay >= 0.0, "fetch.delay must be >= 0");
        anyhow::ensure!(self.delay.is_finite(), "fetch.delay must be finite");
        if let Some(ttl) = self.cache_ttl {
            anyhow::ensure!(ttl >= 0.0, "fetch.cache_ttl must be >= 0");
            anyhow::ensure!(ttl.is_finite(), "fetch.cache_ttl must be finite");
        }
        Ok(())
    }
}
