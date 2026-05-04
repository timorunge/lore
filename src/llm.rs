//! LLM integration for document enrichment.

use std::sync::Arc;

use anyhow::{Context, Result};
use futures::stream::StreamExt;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use crate::config::{IngestConfig, LlmConfig, LlmProvider};
use crate::types::Chunk;

const TOPIC_PROMPT: &str = "\
You are a document classifier. Given the beginning of a document, \
respond with a short, descriptive topic title (2-6 words) that captures \
the main subject. Respond with ONLY the title, nothing else.";

const SUMMARY_PROMPT: &str = "\
You are a document summarizer. Given the beginning of a document, \
respond with a single concise sentence (under 30 words) that captures \
the document's main topic and purpose. Respond with ONLY the sentence, \
nothing else.";

const ENRICH_PROMPT: &str = "\
You are a document analyst. Given a text chunk from a knowledge base, \
respond with a JSON object containing:\n\
- \"llm_summary\": A 1-2 sentence summary of the chunk content.\n\
- \"llm_tags\": A comma-separated list of 3-8 relevant keywords/tags.\n\
- \"llm_quality_score\": A float from 0.0 to 1.0 indicating how useful this \
  chunk is as reference material (0 = boilerplate/noise, 1 = highly informative).\n\n\
Respond with ONLY the JSON object, no other text.";

/// Maximum LLM response body size (1 MB).
const MAX_LLM_RESPONSE_BYTES: usize = 1024 * 1024;

/// LLM-generated per-chunk enrichment fields.
#[derive(Debug, Default)]
pub(crate) struct EnrichmentResult {
    /// One-to-two sentence summary of the chunk content.
    pub summary: Option<String>,
    /// Comma-separated list of keywords or tags for the chunk.
    pub tags: Option<String>,
    /// Quality score in [0.0, 1.0] indicating chunk usefulness as reference material.
    pub quality: Option<f64>,
}

/// Bundled parameters for an OpenAI-compatible chat completion request.
struct OpenAiRequest<'a> {
    url: &'a str,
    model: &'a str,
    auth: Option<&'a str>,
    content: &'a str,
    system: &'a str,
    max_tokens: u32,
    provider: &'a str,
}

/// Stateful client for calling LLM provider APIs (Anthropic, OpenAI, Ollama, Bedrock).
pub struct LlmClient {
    /// LLM configuration (provider selection, model names, prompt overrides).
    config: LlmConfig,
    /// HTTP client for LLM API calls. No SSRF protection -- LLM endpoints
    /// come from trusted operator config, not from fetched content.
    http: reqwest::Client,
    /// Lazily initialised AWS Bedrock runtime client.
    bedrock: tokio::sync::OnceCell<aws_sdk_bedrockruntime::Client>,
}

/// LLM-generated document-level enrichment output from the full pipeline.
pub struct DocumentEnrichment {
    pub doc_summary: Option<String>,
}

impl LlmClient {
    /// Create a new client from the given LLM config, building the shared HTTP client.
    pub fn new(config: &LlmConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .connect_timeout(std::time::Duration::from_secs(config.connect_timeout_secs))
            .build()
            .context("failed to build LLM HTTP client")?;
        Ok(Self {
            config: config.clone(),
            http,
            bedrock: tokio::sync::OnceCell::new(),
        })
    }

    /// Detect a topic name for a document using LLM.
    pub async fn detect_topic(&self, content: &str) -> Option<String> {
        self.call_simple(
            content,
            self.config.detect_topics.max_content_chars,
            self.config.detect_topics.prompt.as_deref(),
            TOPIC_PROMPT,
            self.config.detect_topics.max_tokens,
        )
        .await
    }

    /// Generate a one-sentence summary of the entire document.
    pub async fn summarize_document(&self, content: &str) -> Option<String> {
        self.call_simple(
            content,
            self.config.summarize_docs.max_content_chars,
            self.config.summarize_docs.prompt.as_deref(),
            SUMMARY_PROMPT,
            self.config.summarize_docs.max_tokens,
        )
        .await
    }

    /// Enrich a chunk with summary, keywords, and quality score.
    pub(crate) async fn enrich_chunk(&self, text: &str) -> EnrichmentResult {
        let max_chars = self.config.enrich_chunks.max_content_chars;
        let truncated = crate::util::truncate_str_ref(text, max_chars);

        let prompt = self
            .config
            .enrich_chunks
            .prompt
            .as_deref()
            .unwrap_or(ENRICH_PROMPT);

        let Some(response) = self
            .call_llm(truncated, prompt, self.config.enrich_chunks.max_tokens)
            .await
        else {
            return EnrichmentResult::default();
        };

        let Some(json) = extract_json(&response) else {
            let snippet = crate::util::truncate_str_ref(text, 80);
            warn!(snippet, "failed to parse enrichment JSON response");
            return EnrichmentResult::default();
        };

        EnrichmentResult {
            summary: json
                .get("llm_summary")
                .and_then(|v| v.as_str())
                .map(std::borrow::ToOwned::to_owned),
            tags: json
                .get("llm_tags")
                .and_then(|v| v.as_str())
                .map(std::borrow::ToOwned::to_owned),
            quality: json
                .get("llm_quality_score")
                .and_then(serde_json::Value::as_f64)
                .map(|v| v.clamp(0.0, 1.0)),
        }
    }

    /// Run a simple single-prompt LLM task, returning None on failure or empty result.
    async fn call_simple(
        &self,
        content: &str,
        max_content_chars: usize,
        prompt: Option<&str>,
        default_prompt: &str,
        max_tokens: u32,
    ) -> Option<String> {
        let truncated = crate::util::truncate_str_ref(content, max_content_chars);
        let prompt = prompt.unwrap_or(default_prompt);
        let result = self.call_llm(truncated, prompt, max_tokens).await?;
        let trimmed = result.trim().to_owned();
        (!trimmed.is_empty()).then_some(trimmed)
    }

    /// Build an OpenAI-compatible chat completion request body.
    fn openai_body(model: &str, content: &str, system: &str, max_tokens: u32) -> serde_json::Value {
        serde_json::json!({
            "model": model,
            "max_completion_tokens": max_tokens,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": content}
            ]
        })
    }

    /// Dispatch an LLM call, trying each configured provider in order and returning the first success.
    async fn call_llm(&self, content: &str, system: &str, max_tokens: u32) -> Option<String> {
        let wrapped = format!("<document>\n{content}\n</document>");
        let content = wrapped.as_str();

        let providers: Vec<LlmProvider> = match self.config.provider {
            Some(p) => vec![p],
            None => {
                // Auto-detect: try providers that require API keys first (fast
                // failure if key is missing), then Ollama (local), then Bedrock
                // (AWS credential chain lookup can be slow when not configured).
                vec![
                    LlmProvider::Anthropic,
                    LlmProvider::Openai,
                    LlmProvider::Ollama,
                    LlmProvider::Bedrock,
                ]
            }
        };

        let single_provider = providers.len() == 1;

        for p in providers {
            let result = match p {
                LlmProvider::Bedrock => self.call_bedrock(content, system, max_tokens).await,
                LlmProvider::Anthropic => self.call_anthropic(content, system, max_tokens).await,
                LlmProvider::Openai => self.call_openai(content, system, max_tokens).await,
                LlmProvider::Ollama => self.call_ollama(content, system, max_tokens).await,
            };
            match result {
                Ok(Some(text)) => return Some(text),
                Ok(None) => {
                    debug!(provider = ?p, "LLM returned no content");
                }
                Err(e) => {
                    if is_auth_error(&e) {
                        warn!(
                            provider = ?p,
                            error = %e,
                            "LLM authentication failed -- check your API key or credentials",
                        );
                        if single_provider {
                            return None;
                        }
                    } else {
                        debug!(provider = ?p, "LLM call failed: {e}");
                    }
                }
            }
        }

        warn!("all LLM providers failed");
        None
    }

    /// Send a prompt to AWS Bedrock via the model-agnostic Converse API.
    async fn call_bedrock(
        &self,
        content: &str,
        system: &str,
        max_tokens: u32,
    ) -> Result<Option<String>> {
        use aws_sdk_bedrockruntime::types::{
            ContentBlock, ConversationRole, InferenceConfiguration, Message, SystemContentBlock,
        };

        let client = self.bedrock_client().await;

        let resp = client
            .converse()
            .model_id(&self.config.bedrock_model)
            .system(SystemContentBlock::Text(system.to_owned()))
            .messages(
                Message::builder()
                    .role(ConversationRole::User)
                    .content(ContentBlock::Text(content.to_owned()))
                    .build()
                    .context("failed to build Bedrock message")?,
            )
            .inference_config(
                InferenceConfiguration::builder()
                    .max_tokens(max_tokens as i32)
                    .build(),
            )
            .send()
            .await
            .context("Bedrock converse failed")?;

        let text = resp
            .output()
            .and_then(|o| o.as_message().ok())
            .and_then(|m| m.content().first())
            .and_then(|block| block.as_text().ok())
            .cloned();
        Ok(text)
    }

    /// Send a prompt to the Anthropic Messages API.
    async fn call_anthropic(
        &self,
        content: &str,
        system: &str,
        max_tokens: u32,
    ) -> Result<Option<String>> {
        // Read the API key at call time so rotation takes effect without restart.
        let api_key = std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY not set")?;

        let body = serde_json::json!({
            "model": self.config.anthropic_model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": [
                {"role": "user", "content": content}
            ]
        });

        let resp = self
            .http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Anthropic API request failed")?;

        let resp_body = check_llm_response(resp, "Anthropic").await?;
        Ok(extract_anthropic_text(&resp_body))
    }

    /// Send a prompt to the OpenAI chat completions API.
    async fn call_openai(
        &self,
        content: &str,
        system: &str,
        max_tokens: u32,
    ) -> Result<Option<String>> {
        // Read the API key at call time so rotation takes effect without restart.
        let api_key = std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY not set")?;
        self.call_openai_compatible(OpenAiRequest {
            url: "https://api.openai.com/v1/chat/completions",
            model: &self.config.openai_model,
            auth: Some(&format!("Bearer {api_key}")),
            content,
            system,
            max_tokens,
            provider: "OpenAI",
        })
        .await
    }

    /// Send a prompt to a local Ollama instance via its OpenAI-compatible endpoint.
    async fn call_ollama(
        &self,
        content: &str,
        system: &str,
        max_tokens: u32,
    ) -> Result<Option<String>> {
        let raw_url = self
            .config
            .ollama_url
            .as_deref()
            .unwrap_or(crate::config::DEFAULT_OLLAMA_URL);
        let base_url = raw_url.trim_end_matches('/');
        anyhow::ensure!(
            base_url.starts_with("http://") || base_url.starts_with("https://"),
            "Ollama URL must start with http:// or https://, got: {base_url:?}"
        );
        self.call_openai_compatible(OpenAiRequest {
            url: &format!("{base_url}/v1/chat/completions"),
            model: &self.config.ollama_model,
            auth: None,
            content,
            system,
            max_tokens,
            provider: "Ollama",
        })
        .await
    }

    /// Shared implementation for OpenAI-compatible chat completion APIs.
    async fn call_openai_compatible(&self, req: OpenAiRequest<'_>) -> Result<Option<String>> {
        let mut http_req = self
            .http
            .post(req.url)
            .header("content-type", "application/json")
            .json(&Self::openai_body(
                req.model,
                req.content,
                req.system,
                req.max_tokens,
            ));
        if let Some(token) = req.auth {
            http_req = http_req.header("Authorization", token);
        }
        let resp = http_req
            .send()
            .await
            .with_context(|| format!("{} API request failed", req.provider))?;
        Ok(extract_openai_text(
            &check_llm_response(resp, req.provider).await?,
        ))
    }

    /// Lazily initialise and return the AWS Bedrock runtime client.
    async fn bedrock_client(&self) -> &aws_sdk_bedrockruntime::Client {
        self.bedrock
            .get_or_init(|| async {
                let region = std::env::var("AWS_REGION")
                    .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
                    .unwrap_or_else(|_| "us-east-1".to_owned());

                let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                    .region(aws_sdk_bedrockruntime::config::Region::new(region))
                    .load()
                    .await;

                aws_sdk_bedrockruntime::Client::new(&sdk_config)
            })
            .await
    }
}

/// Extract the first JSON object from `text`.
///
/// Strips markdown code fences when present, then tries a direct parse.
/// Falls back to a brace-scan that finds the outermost `{...}` span.
///
/// # Known limitation
///
/// The brace-scan fallback is fooled by `}` inside JSON string values.
/// For example, `{"key": "}"}` causes the scan to stop at the `}` inside
/// the string value rather than at the closing brace of the object, producing
/// a parse error or an incorrectly truncated result.  The direct-parse path
/// is not affected by this limitation.
fn extract_json(text: &str) -> Option<serde_json::Value> {
    let cleaned = text
        .trim()
        .strip_prefix("```json")
        .or_else(|| text.trim().strip_prefix("```"))
        .map(str::trim)
        .and_then(|s| s.strip_suffix("```"))
        .map_or(text.trim(), str::trim);

    if let Ok(val) = serde_json::from_str(cleaned) {
        return Some(val);
    }

    if let Some(start) = cleaned.find('{')
        && let Some(end) = cleaned.rfind('}')
        && end > start
        && let Ok(val) = serde_json::from_str(&cleaned[start..=end])
    {
        return Some(val);
    }

    None
}

/// Parse and size-check a raw LLM response body into a JSON value.
fn parse_llm_response(bytes: &[u8], provider: &str) -> Result<serde_json::Value> {
    anyhow::ensure!(
        bytes.len() <= MAX_LLM_RESPONSE_BYTES,
        "{provider} response too large: {} bytes",
        bytes.len()
    );
    serde_json::from_slice(bytes).with_context(|| format!("failed to parse {provider} response"))
}

/// Assert HTTP success, then parse the response body as JSON.
async fn check_llm_response(resp: reqwest::Response, provider: &str) -> Result<serde_json::Value> {
    let status = resp.status();
    let resp_bytes = resp.bytes().await?;
    anyhow::ensure!(
        status.is_success(),
        "{provider} API returned {status}: {}",
        String::from_utf8_lossy(&resp_bytes[..resp_bytes.len().min(512)])
    );
    parse_llm_response(&resp_bytes, provider)
}

/// Extract the text content from an Anthropic Messages API response body.
fn extract_anthropic_text(body: &serde_json::Value) -> Option<String> {
    body.get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("text"))
        .and_then(|t| t.as_str())
        .map(std::borrow::ToOwned::to_owned)
}

/// Extract the text content from an OpenAI-compatible chat completions response body.
fn extract_openai_text(body: &serde_json::Value) -> Option<String> {
    body.get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|t| t.as_str())
        .map(std::borrow::ToOwned::to_owned)
}

/// Returns `true` when `err` looks like an authentication/authorisation failure.
///
/// Matches HTTP 401/403 status codes embedded in the error message by
/// [`check_llm_response`] (e.g. `"Anthropic API returned 401 Unauthorized: ..."`)
/// and missing API-key errors raised before the HTTP call
/// (e.g. `"ANTHROPIC_API_KEY not set"`).
fn is_auth_error(err: &anyhow::Error) -> bool {
    let msg = format!("{err:#}");
    // Match HTTP 401/403 only as whole status codes, not as substrings of
    // larger numbers (e.g. port 4013).  Accepted prefixes come from:
    //   - check_llm_response: "... returned 401 Unauthorized: ..."  -> " 401"
    //   - reqwest status display: "status: 401"
    //   - some error formatters: "HTTP 401"
    msg.contains(" 401")
        || msg.contains(" 403")
        || msg.contains("status: 401")
        || msg.contains("status: 403")
        || msg.contains("HTTP 401")
        || msg.contains("HTTP 403")
        || msg.contains("_KEY not set")
}

/// Run the full per-document LLM pipeline: topic detection, chunk enrichment,
/// and document summarization.
pub async fn enrich_document(
    chunks: &mut Vec<Chunk>,
    content: &str,
    has_topic: bool,
    client: &LlmClient,
    config: &IngestConfig,
    semaphore: &Semaphore,
    concurrency: usize,
) -> DocumentEnrichment {
    let Some(llm_config) = &config.llm else {
        return DocumentEnrichment { doc_summary: None };
    };

    if llm_config.detect_topics.enabled
        && !has_topic
        && let Some(topic) = client.detect_topic(content).await
    {
        let topic: std::sync::Arc<str> = Arc::from(topic);
        for chunk in chunks.iter_mut() {
            chunk.topic = Some(topic.clone());
        }
    }

    enrich_chunks(chunks, client, config, semaphore, concurrency).await;

    let doc_summary = if llm_config.summarize_docs.enabled {
        client.summarize_document(content).await
    } else {
        None
    };

    DocumentEnrichment { doc_summary }
}

/// Enrich all chunks in a document batch via concurrent LLM calls, filtering by quality threshold.
pub async fn enrich_chunks(
    chunks: &mut Vec<Chunk>,
    llm_client: &LlmClient,
    config: &IngestConfig,
    semaphore: &Semaphore,
    concurrency: usize,
) {
    let llm_config = match &config.llm {
        Some(c) if c.enrich_chunks.enabled => c,
        _ => return,
    };

    let quality_threshold = llm_config.enrich_chunks.quality_threshold;

    let handles: Vec<_> = chunks
        .iter()
        .map(|chunk| {
            let body = chunk.body.clone();
            async move {
                let _permit = semaphore.acquire().await.expect(
                    "invariant: enrich semaphore is never closed while pipeline is running",
                );
                llm_client.enrich_chunk(&body).await
            }
        })
        .collect();

    let results: Vec<_> = futures::stream::iter(handles)
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;

    if !results.is_empty()
        && results
            .iter()
            .all(|r| r.summary.is_none() && r.tags.is_none() && r.quality.is_none())
    {
        warn!(
            "all enrichment calls failed for this batch (no LLM summaries, tags, or quality scores produced)"
        );
    }

    *chunks = chunks
        .drain(..)
        .zip(results)
        .filter_map(|(mut chunk, r)| {
            if r.quality.is_some_and(|q| q < quality_threshold) {
                return None;
            }
            chunk.llm_summary.clone_from(&r.summary);
            chunk.llm_tags.clone_from(&r.tags);
            chunk.llm_quality_score = r.quality;
            Some(chunk)
        })
        .collect();
}
