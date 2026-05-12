use std::borrow::Cow;
use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Validate;
use crate::config::processing::ProcessingRef;
use crate::util;

/// Default language code for YouTube transcript fetching.
const DEFAULT_YOUTUBE_LANG: &str = "en";

/// Default maximum number of videos to fetch from a YouTube playlist/channel.
const DEFAULT_MAX_VIDEOS: usize = 50;

/// Allowed URL schemes for git sources (validation in config and git loader).
const SAFE_GIT_SCHEMES: &[&str] = &["https", "http", "git", "ssh"];

/// Allowed URL schemes for HTTP/HTTPS sources.
const SAFE_HTTP_SCHEMES: &[&str] = &["https", "http"];

/// Maximum display width for multi-URL source labels.
const MAX_LABEL_WIDTH: usize = 50;

/// Discriminant keys that identify each source type in a YAML block.
const SOURCE_KEYS: &[&str] = &[
    "path", "url", "git", "sitemap", "feed", "s3", "youtube", "maildir", "exec", "mcp",
];

/// Controls how a source is refreshed during incremental ingests.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    /// Smart freshness detection -- mtime+size for local files, ETags and content hashes for remote sources. This is the default.
    #[default]
    Auto,
    /// Always re-fetch and re-process the source, bypassing all freshness checks. Equivalent to `--force` but scoped to a single source.
    Always,
    /// Skip the source on incremental ingests once it has been indexed. Existing documents are protected from stale-document cleanup even if the source becomes unreachable.
    Never,
}

impl UpdateMode {
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn is_default(&self) -> bool {
        *self == Self::Auto
    }
}

// The inner source structs are intentionally not re-exported: callers match on
// `SourceConfig` variants but do not construct them directly. The
// `#[allow(private_interfaces)]` attributes suppress the lint that fires because
// these `pub` structs live in a private module yet appear in the public `SourceConfig` enum.

/// Local file(s) or directory(ies). Supports `glob` patterns. Archives (zip, tar, gz, etc.) are extracted automatically.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(private_interfaces)]
pub struct LocalSource {
    /// Path(s) to files or directories (string or list). Supports `~` and `$VAR` expansion.
    #[serde(deserialize_with = "deserialize_string_or_list")]
    pub path: Vec<String>,
    /// Glob pattern relative to `path`. Only matching files are ingested (default: `"**/*"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// Update mode: auto, always, or never (default: auto).
    #[serde(default, skip_serializing_if = "UpdateMode::is_default")]
    pub update: UpdateMode,
    /// Topic label applied to all chunks from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Comma-separated tags applied to all documents from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    /// Processing override: a preset name (`"code"`) or an inline profile object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing: Option<ProcessingRef>,
}

/// Single HTTP/HTTPS URL, or a list of URLs. Archive content types are detected and extracted.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(private_interfaces)]
pub struct UrlSource {
    /// One or more HTTP/HTTPS URLs to fetch (string or list).
    #[serde(deserialize_with = "deserialize_string_or_list")]
    pub url: Vec<String>,
    /// HTTP headers for requests. Values support `${LORE_*}` env var expansion.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Update mode: auto, always, or never (default: auto).
    #[serde(default, skip_serializing_if = "UpdateMode::is_default")]
    pub update: UpdateMode,
    /// Topic label applied to all chunks from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Comma-separated tags applied to all documents from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    /// Processing override: a preset name or an inline profile object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing: Option<ProcessingRef>,
}

/// Git repository URL(s). Cloned/fetched to a local cache. `ref` selects a branch, tag, or commit.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(private_interfaces)]
pub struct GitSource {
    /// Repository URL(s) (string or list). Allowed schemes: `https`, `http`, `ssh`, `git`.
    #[serde(deserialize_with = "deserialize_string_or_list")]
    pub git: Vec<String>,
    /// Branch, tag, or commit to check out.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// Glob pattern for files to ingest (default: `"**/*.md"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// Update mode: auto, always, or never (default: auto).
    #[serde(default, skip_serializing_if = "UpdateMode::is_default")]
    pub update: UpdateMode,
    /// Topic label applied to all chunks from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Comma-separated tags applied to all documents from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    /// Processing override: a preset name or an inline profile object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing: Option<ProcessingRef>,
}

/// XML sitemap URL(s). Discovers and fetches all listed pages. `include` filters URLs by regex.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(private_interfaces)]
pub struct SitemapSource {
    /// XML sitemap URL(s) (string or list).
    #[serde(deserialize_with = "deserialize_string_or_list")]
    pub sitemap: Vec<String>,
    /// Regex filter applied to discovered URLs. Only matching URLs are fetched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<String>,
    /// HTTP headers for requests. Values support `${LORE_*}` env var expansion.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Update mode: auto, always, or never (default: auto).
    #[serde(default, skip_serializing_if = "UpdateMode::is_default")]
    pub update: UpdateMode,
    /// Topic label applied to all chunks from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Comma-separated tags applied to all documents from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    /// Processing override: a preset name or an inline profile object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing: Option<ProcessingRef>,
}

/// RSS or Atom feed URL(s). Fetches linked articles. `discard: true` removes old entries on update.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(private_interfaces)]
pub struct FeedSource {
    /// RSS/Atom feed URL(s) (string or list).
    #[serde(deserialize_with = "deserialize_string_or_list")]
    pub feed: Vec<String>,
    /// Regex filter applied to entry URLs. Only matching entries are fetched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<String>,
    /// Remove documents from the index when their entries disappear from the feed (default: false).
    #[serde(default)]
    pub discard: bool,
    /// HTTP headers for requests. Values support `${LORE_*}` env var expansion.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Update mode: auto, always, or never (default: auto).
    #[serde(default, skip_serializing_if = "UpdateMode::is_default")]
    pub update: UpdateMode,
    /// Topic label applied to all chunks from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Comma-separated tags applied to all documents from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    /// Processing override: a preset name or an inline profile object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing: Option<ProcessingRef>,
}

/// S3 URI(s) (`s3://bucket/prefix`). Requires the `s3` feature flag. `glob` and `include` filter keys.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(private_interfaces)]
pub struct S3Source {
    /// S3 bucket URI(s) in `s3://bucket/prefix` format (string or list).
    #[serde(deserialize_with = "deserialize_string_or_list")]
    pub s3: Vec<String>,
    /// Glob pattern for objects to ingest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// Regex filter applied to object keys. Only matching objects are fetched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<String>,
    /// Update mode: auto, always, or never (default: auto).
    #[serde(default, skip_serializing_if = "UpdateMode::is_default")]
    pub update: UpdateMode,
    /// Topic label applied to all chunks from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Comma-separated tags applied to all documents from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    /// Processing override: a preset name or an inline profile object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing: Option<ProcessingRef>,
}

/// YouTube video, playlist, or channel URL(s). Fetches transcripts via `yt-dlp`.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(private_interfaces)]
pub struct YoutubeSource {
    /// YouTube video, playlist, or channel URL(s) (string or list).
    #[serde(deserialize_with = "deserialize_string_or_list")]
    pub youtube: Vec<String>,
    /// Transcript language code (default: `"en"`).
    #[serde(default = "default_youtube_lang")]
    pub lang: String,
    /// Regex filter applied to video titles. Only matching videos are processed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<String>,
    /// Maximum videos to fetch from a playlist or channel (default: 50).
    #[serde(default = "default_max_videos")]
    pub max_videos: usize,
    /// Update mode: auto, always, or never (default: auto).
    #[serde(default, skip_serializing_if = "UpdateMode::is_default")]
    pub update: UpdateMode,
    /// Topic label applied to all chunks from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Comma-separated tags applied to all documents from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    /// Processing override: a preset name or an inline profile object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing: Option<ProcessingRef>,
}

/// Maildir directory (or directories). Indexes all messages in `new/` and `cur/`.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(private_interfaces)]
pub struct MaildirSource {
    /// Path(s) to Maildir root directories (string or list). Supports `~` and `$VAR` expansion.
    #[serde(deserialize_with = "deserialize_string_or_list")]
    pub maildir: Vec<String>,
    /// Skip messages with the Trashed (`T`) flag (default: true).
    #[serde(default = "default_true")]
    pub skip_trashed: bool,
    /// Update mode: auto, always, or never (default: auto).
    #[serde(default, skip_serializing_if = "UpdateMode::is_default")]
    pub update: UpdateMode,
    /// Topic label applied to all chunks from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Comma-separated tags applied to all documents from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    /// Processing override: a preset name or an inline profile object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing: Option<ProcessingRef>,
}

/// Output format for exec source stdout.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ExecOutputMode {
    /// Each non-empty stdout line is parsed as a JSON object with required `source` and `content` fields. This is the default.
    #[default]
    Jsonl,
    /// The entire stdout of each command is treated as a single document's content.
    Raw,
}

impl ExecOutputMode {
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn is_default(&self) -> bool {
        *self == Self::Jsonl
    }
}

/// Command(s) to execute. stdout is parsed as JSONL (default) or consumed as raw content.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(private_interfaces)]
pub struct ExecSource {
    /// Shell command(s) to run via `sh -c` (string or list).
    #[serde(deserialize_with = "deserialize_string_or_list")]
    pub exec: Vec<String>,
    /// Output mode: `jsonl` (default) or `raw`. In `raw` mode the entire stdout becomes a single document.
    #[serde(default, skip_serializing_if = "ExecOutputMode::is_default")]
    pub output: ExecOutputMode,
    /// Stable identity key used as the document `source` in `raw` mode. Defaults to the command string when omitted. Ignored in `jsonl` mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_key: Option<String>,
    /// Format hint for the document content (e.g. `"md"`, `"txt"`). In `raw` mode this tells the extractor how to parse the content. In `jsonl` mode it serves as a fallback when a line omits the `format` field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Working directory override (default: config base dir). Supports `~` and `$VAR`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// Extra environment variables passed to the subprocess.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// Per-source timeout in seconds (default: `processing.extraction_timeout_secs`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Update mode: auto, always, or never (default: auto).
    #[serde(default, skip_serializing_if = "UpdateMode::is_default")]
    pub update: UpdateMode,
    /// Topic label applied to all chunks from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Comma-separated tags applied to all documents from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    /// Processing override: a preset name or an inline profile object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing: Option<ProcessingRef>,
}

/// MCP transport protocol.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    /// Spawn the server as a child process and communicate over stdin/stdout.
    #[default]
    Stdio,
    /// Connect to the server over HTTP (Streamable HTTP transport).
    Http,
}

/// A single explicit tool call with name and arguments.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpToolCall {
    /// Tool name to invoke on the upstream MCP server.
    pub name: String,
    /// JSON arguments passed to the tool call (default: `{}`).
    #[serde(default = "default_empty_object")]
    pub args: serde_json::Value,
}

fn default_empty_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// Which resources to read from the upstream MCP server.
#[derive(Debug, Clone, schemars::JsonSchema)]
pub enum McpResources {
    /// Auto-discover and read all resources via `list_resources`.
    All,
    /// Read only the listed resource URIs.
    List(Vec<String>),
}

impl Serialize for McpResources {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::All => serializer.serialize_str("all"),
            Self::List(v) => v.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for McpResources {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = McpResources;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("\"all\" or a list of resource URIs")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                if v.eq_ignore_ascii_case("all") {
                    Ok(McpResources::All)
                } else {
                    Err(serde::de::Error::custom(
                        "expected \"all\" or a list of resource URIs",
                    ))
                }
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                seq: A,
            ) -> Result<Self::Value, A::Error> {
                let v = Vec::deserialize(serde::de::value::SeqAccessDeserializer::new(seq))?;
                Ok(McpResources::List(v))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

/// Upstream MCP server source. Connects to an MCP server and ingests resources and/or tool call results. Requires the `mcp` feature.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(private_interfaces)]
pub struct McpSource {
    /// Server command (stdio) or URL (http).
    pub mcp: String,
    /// Transport protocol: `stdio` (default) or `http`.
    #[serde(default, skip_serializing_if = "is_default_transport")]
    pub transport: McpTransport,
    /// Resource discovery mode: `"all"` to auto-discover, or a list of specific resource URIs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<McpResources>,
    /// Explicit tool calls to invoke on the upstream server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<McpToolCall>,
    /// Extra environment variables passed to the stdio subprocess.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// Bearer token for HTTP transport. Supports `${LORE_*}` env var expansion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Per-session timeout in seconds (default: `processing.extraction_timeout_secs`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Update mode: auto, always, or never (default: auto).
    #[serde(default, skip_serializing_if = "UpdateMode::is_default")]
    pub update: UpdateMode,
    /// Topic label applied to all chunks from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Comma-separated tags applied to all documents from this source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    /// Processing override: a preset name or an inline profile object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing: Option<ProcessingRef>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_default_transport(t: &McpTransport) -> bool {
    *t == McpTransport::Stdio
}

fn default_true() -> bool {
    true
}

/// Configuration for a single ingest source.
#[derive(Debug, Clone)]
pub enum SourceConfig {
    /// Local filesystem source (directory or file glob).
    Local(LocalSource),
    /// HTTP/HTTPS URL source (single URL or list).
    Url(UrlSource),
    /// Git repository source.
    Git(GitSource),
    /// XML sitemap source that discovers URLs to fetch.
    Sitemap(SitemapSource),
    /// RSS/Atom feed source.
    Feed(FeedSource),
    /// AWS S3 bucket source.
    S3(S3Source),
    /// YouTube video/playlist/channel transcript source.
    Youtube(YoutubeSource),
    /// Maildir email store source.
    Maildir(MaildirSource),
    /// Shell command source that reads JSONL from stdout.
    Exec(ExecSource),
    /// Upstream MCP server source.
    Mcp(McpSource),
}

fn default_youtube_lang() -> String {
    DEFAULT_YOUTUBE_LANG.to_owned()
}

fn default_max_videos() -> usize {
    DEFAULT_MAX_VIDEOS
}

/// Compute the Levenshtein (edit) distance between two strings.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    let mut dp: Vec<Vec<usize>> = (0..=m)
        .map(|i| {
            let mut row = vec![0usize; n + 1];
            row[0] = i;
            row
        })
        .collect();
    for (j, cell) in dp[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if a[i - 1] == b[j - 1] {
                dp[i - 1][j - 1]
            } else {
                1 + dp[i - 1][j - 1].min(dp[i - 1][j]).min(dp[i][j - 1])
            };
        }
    }
    dp[m][n]
}

/// Returns the closest key in `SOURCE_KEYS` if within edit distance 2.
fn suggest_source_key(input: &str) -> Option<&'static str> {
    SOURCE_KEYS
        .iter()
        .map(|k| (*k, levenshtein(input, k)))
        .filter(|(_, d)| *d <= 2)
        .min_by_key(|(_, d)| *d)
        .map(|(k, _)| k)
}

impl<'de> serde::Deserialize<'de> for SourceConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value =
            serde_yaml_ng::Value::deserialize(deserializer).map_err(serde::de::Error::custom)?;

        let mapping = value
            .as_mapping()
            .ok_or_else(|| serde::de::Error::custom("source entry must be a YAML mapping"))?;

        let found: Vec<&'static str> = SOURCE_KEYS
            .iter()
            .copied()
            .filter(|k| mapping.contains_key(serde_yaml_ng::Value::String((*k).to_owned())))
            .collect();

        match found.len() {
            0 => {
                let suggestion = mapping
                    .iter()
                    .filter_map(|(k, _)| k.as_str())
                    .find_map(suggest_source_key);
                if let Some(hint) = suggestion {
                    Err(serde::de::Error::custom(format!(
                        "unknown source type; did you mean '{hint}'? \
                         Expected one of: path, url, git, sitemap, feed, s3, youtube, maildir, exec, mcp"
                    )))
                } else {
                    Err(serde::de::Error::custom(
                        "unknown source type; expected one of: \
                         path, url, git, sitemap, feed, s3, youtube, maildir, exec, mcp",
                    ))
                }
            }
            1 => {
                let key = found[0];
                let v = value.clone();
                match key {
                    "path" => serde_yaml_ng::from_value(v)
                        .map(SourceConfig::Local)
                        .map_err(|e| {
                            serde::de::Error::custom(format!("invalid 'path' source: {e}"))
                        }),
                    "url" => serde_yaml_ng::from_value(v)
                        .map(SourceConfig::Url)
                        .map_err(|e| {
                            serde::de::Error::custom(format!("invalid 'url' source: {e}"))
                        }),
                    "git" => serde_yaml_ng::from_value(v)
                        .map(SourceConfig::Git)
                        .map_err(|e| {
                            serde::de::Error::custom(format!("invalid 'git' source: {e}"))
                        }),
                    "sitemap" => serde_yaml_ng::from_value(v)
                        .map(SourceConfig::Sitemap)
                        .map_err(|e| {
                            serde::de::Error::custom(format!("invalid 'sitemap' source: {e}"))
                        }),
                    "feed" => serde_yaml_ng::from_value(v)
                        .map(SourceConfig::Feed)
                        .map_err(|e| {
                            serde::de::Error::custom(format!("invalid 'feed' source: {e}"))
                        }),
                    "s3" => serde_yaml_ng::from_value(v)
                        .map(SourceConfig::S3)
                        .map_err(|e| serde::de::Error::custom(format!("invalid 's3' source: {e}"))),
                    "youtube" => serde_yaml_ng::from_value(v)
                        .map(SourceConfig::Youtube)
                        .map_err(|e| {
                            serde::de::Error::custom(format!("invalid 'youtube' source: {e}"))
                        }),
                    "maildir" => serde_yaml_ng::from_value(v)
                        .map(SourceConfig::Maildir)
                        .map_err(|e| {
                            serde::de::Error::custom(format!("invalid 'maildir' source: {e}"))
                        }),
                    "exec" => serde_yaml_ng::from_value(v)
                        .map(SourceConfig::Exec)
                        .map_err(|e| {
                            serde::de::Error::custom(format!("invalid 'exec' source: {e}"))
                        }),
                    "mcp" => serde_yaml_ng::from_value(v)
                        .map(SourceConfig::Mcp)
                        .map_err(|e| {
                            serde::de::Error::custom(format!("invalid 'mcp' source: {e}"))
                        }),
                    _ => unreachable!(),
                }
            }
            _ => {
                let a = found[0];
                let b = found[1];
                Err(serde::de::Error::custom(format!(
                    "source block has conflicting keys: '{a}' and '{b}'"
                )))
            }
        }
    }
}

/// Build a display label from a list of items, truncating the first item and appending a count suffix when there are multiple.
fn multi_label(items: &[String]) -> Cow<'_, str> {
    let first = items.first().map_or("", std::string::String::as_str);
    if items.len() > 1 {
        let suffix = format!(" (+{} more)", items.len() - 1);
        let max_url = MAX_LABEL_WIDTH.saturating_sub(suffix.len());
        if first.len() > max_url {
            let mut boundary = max_url;
            while boundary > 0 && !first.is_char_boundary(boundary) {
                boundary -= 1;
            }
            let short = &first[..boundary];
            Cow::Owned(format!("{short}...{suffix}"))
        } else {
            Cow::Owned(format!("{first}{suffix}"))
        }
    } else {
        Cow::Borrowed(first)
    }
}

macro_rules! source_field {
    ($self:expr, $field:ident . as_deref()) => {
        match $self {
            Self::Local(s) => s.$field.as_deref(),
            Self::Url(s) => s.$field.as_deref(),
            Self::Git(s) => s.$field.as_deref(),
            Self::Sitemap(s) => s.$field.as_deref(),
            Self::Feed(s) => s.$field.as_deref(),
            Self::S3(s) => s.$field.as_deref(),
            Self::Youtube(s) => s.$field.as_deref(),
            Self::Maildir(s) => s.$field.as_deref(),
            Self::Exec(s) => s.$field.as_deref(),
            Self::Mcp(s) => s.$field.as_deref(),
        }
    };
    ($self:expr, $field:ident . as_ref()) => {
        match $self {
            Self::Local(s) => s.$field.as_ref(),
            Self::Url(s) => s.$field.as_ref(),
            Self::Git(s) => s.$field.as_ref(),
            Self::Sitemap(s) => s.$field.as_ref(),
            Self::Feed(s) => s.$field.as_ref(),
            Self::S3(s) => s.$field.as_ref(),
            Self::Youtube(s) => s.$field.as_ref(),
            Self::Maildir(s) => s.$field.as_ref(),
            Self::Exec(s) => s.$field.as_ref(),
            Self::Mcp(s) => s.$field.as_ref(),
        }
    };
}

impl SourceConfig {
    pub fn label(&self) -> Cow<'_, str> {
        match self {
            Self::Local(s) => multi_label(&s.path),
            Self::Url(s) => multi_label(&s.url),
            Self::Git(s) => multi_label(&s.git),
            Self::Sitemap(s) => multi_label(&s.sitemap),
            Self::Feed(s) => multi_label(&s.feed),
            Self::S3(s) => multi_label(&s.s3),
            Self::Youtube(s) => multi_label(&s.youtube),
            Self::Maildir(s) => multi_label(&s.maildir),
            Self::Exec(s) => multi_label(&s.exec),
            Self::Mcp(s) => Cow::Borrowed(&s.mcp),
        }
    }

    /// The YAML key that identifies this source type (for display).
    pub fn config_key(&self) -> &'static str {
        match self {
            Self::Local(_) => "path",
            Self::Url(_) => "url",
            Self::Git(_) => "git",
            Self::Sitemap(_) => "sitemap",
            Self::Feed(_) => "feed",
            Self::S3(_) => "s3",
            Self::Youtube(_) => "youtube",
            Self::Maildir(_) => "maildir",
            Self::Exec(_) => "exec",
            Self::Mcp(_) => "mcp",
        }
    }

    /// Number of discrete items this source contributes (e.g. URL count).
    /// For types where the count is unknown until discovery, returns 1.
    pub fn item_count(&self) -> usize {
        match self {
            Self::Local(s) => s.path.len(),
            Self::Url(s) => s.url.len(),
            Self::Git(s) => s.git.len(),
            Self::Sitemap(s) => s.sitemap.len(),
            Self::Feed(s) => s.feed.len(),
            Self::S3(s) => s.s3.len(),
            Self::Youtube(s) => s.youtube.len(),
            Self::Maildir(s) => s.maildir.len(),
            Self::Exec(s) => s.exec.len(),
            Self::Mcp(_) => 1,
        }
    }

    pub fn topic(&self) -> Option<&str> {
        source_field!(self, topic.as_deref())
    }

    pub fn tags(&self) -> Option<&str> {
        source_field!(self, tags.as_deref())
    }

    /// Mutable access to the HTTP headers map, if this source type carries one.
    pub fn headers_mut(&mut self) -> Option<&mut HashMap<String, String>> {
        match self {
            Self::Url(s) => Some(&mut s.headers),
            Self::Sitemap(s) => Some(&mut s.headers),
            Self::Feed(s) => Some(&mut s.headers),
            _ => None,
        }
    }

    pub fn processing(&self) -> Option<&ProcessingRef> {
        source_field!(self, processing.as_ref())
    }

    pub fn update(&self) -> UpdateMode {
        match self {
            Self::Local(s) => s.update,
            Self::Url(s) => s.update,
            Self::Git(s) => s.update,
            Self::Sitemap(s) => s.update,
            Self::Feed(s) => s.update,
            Self::S3(s) => s.update,
            Self::Youtube(s) => s.update,
            Self::Maildir(s) => s.update,
            Self::Exec(s) => s.update,
            Self::Mcp(s) => s.update,
        }
    }

    /// Check structural invariants and URL schemes for this source.
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Local(s) => {
                anyhow::ensure!(!s.path.is_empty(), "path list cannot be empty");
                for p in &s.path {
                    if p.contains("://") {
                        let scheme = util::url_scheme(p);
                        anyhow::bail!(
                            "source 'path' value looks like a URL ({scheme:?} scheme). \
                             Did you mean to use 'url:', 'feed:', 'sitemap:', or 'git:' instead?"
                        );
                    }
                }
            }
            Self::Url(s) => {
                anyhow::ensure!(!s.url.is_empty(), "url list cannot be empty");
                for u in &s.url {
                    validate_http_url(u, "url")?;
                }
            }
            Self::Git(s) => {
                anyhow::ensure!(!s.git.is_empty(), "git list cannot be empty");
                for url in &s.git {
                    validate_git_url_config(url)?;
                }
                if let Some(r) = &s.git_ref {
                    validate_git_ref(r).with_context(|| format!("invalid git ref: {r:?}"))?;
                }
            }
            Self::Sitemap(s) => {
                anyhow::ensure!(!s.sitemap.is_empty(), "sitemap list cannot be empty");
                for url in &s.sitemap {
                    validate_http_url(url, "sitemap")?;
                }
                validate_include(s.include.as_deref())?;
            }
            Self::Feed(s) => {
                anyhow::ensure!(!s.feed.is_empty(), "feed list cannot be empty");
                for url in &s.feed {
                    validate_http_url(url, "feed")?;
                }
                validate_include(s.include.as_deref())?;
            }
            Self::S3(s) => {
                anyhow::ensure!(!s.s3.is_empty(), "s3 list cannot be empty");
                for uri in &s.s3 {
                    anyhow::ensure!(
                        uri.starts_with("s3://"),
                        "s3 URI must start with s3:// (got {uri:?})"
                    );
                }
                validate_include(s.include.as_deref())?;
            }
            Self::Youtube(s) => {
                anyhow::ensure!(!s.youtube.is_empty(), "youtube URL list cannot be empty");
                for url in &s.youtube {
                    validate_http_url(url, "youtube")?;
                }
                validate_include(s.include.as_deref())?;
                anyhow::ensure!(s.max_videos > 0, "max_videos must be greater than 0");
            }
            Self::Maildir(s) => {
                anyhow::ensure!(!s.maildir.is_empty(), "maildir list cannot be empty");
                for p in &s.maildir {
                    if p.contains("://") {
                        anyhow::bail!(
                            "maildir path must be a local filesystem path, not a URL (got {p:?})"
                        );
                    }
                }
            }
            Self::Exec(s) => {
                anyhow::ensure!(!s.exec.is_empty(), "exec list cannot be empty");
                for cmd in &s.exec {
                    anyhow::ensure!(!cmd.trim().is_empty(), "exec command must not be blank");
                }
                if let Some(t) = s.timeout_secs {
                    anyhow::ensure!(t > 0, "exec timeout_secs must be greater than 0");
                }
                if s.output == ExecOutputMode::Raw {
                    if let Some(key) = &s.source_key {
                        anyhow::ensure!(
                            !key.trim().is_empty(),
                            "exec source_key must not be blank"
                        );
                    }
                    anyhow::ensure!(
                        s.source_key.is_none() || s.exec.len() == 1,
                        "exec source_key cannot be used with multiple commands in raw mode"
                    );
                }
            }
            Self::Mcp(s) => {
                anyhow::ensure!(
                    !s.mcp.trim().is_empty(),
                    "mcp server command/URL must not be blank"
                );
                anyhow::ensure!(
                    s.resources.is_some() || !s.tools.is_empty(),
                    "mcp source must specify at least one of 'resources' or 'tools'"
                );
                if s.transport == McpTransport::Http {
                    validate_http_url(&s.mcp, "mcp")?;
                }
                for tc in &s.tools {
                    anyhow::ensure!(
                        !tc.name.trim().is_empty(),
                        "mcp tool call name must not be blank"
                    );
                    anyhow::ensure!(
                        tc.args.is_null() || tc.args.is_object(),
                        "mcp tool call args must be a JSON object (got {})",
                        tc.args
                    );
                }
                if let Some(t) = s.timeout_secs {
                    anyhow::ensure!(t > 0, "mcp timeout_secs must be greater than 0");
                }
            }
        }

        Ok(())
    }
}

impl Serialize for SourceConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Local(s) => s.serialize(serializer),
            Self::Url(s) => s.serialize(serializer),
            Self::Git(s) => s.serialize(serializer),
            Self::Sitemap(s) => s.serialize(serializer),
            Self::Feed(s) => s.serialize(serializer),
            Self::S3(s) => s.serialize(serializer),
            Self::Youtube(s) => s.serialize(serializer),
            Self::Maildir(s) => s.serialize(serializer),
            Self::Exec(s) => s.serialize(serializer),
            Self::Mcp(s) => s.serialize(serializer),
        }
    }
}

impl schemars::JsonSchema for SourceConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SourceConfig".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "anyOf": [
                generator.subschema_for::<LocalSource>(),
                generator.subschema_for::<UrlSource>(),
                generator.subschema_for::<GitSource>(),
                generator.subschema_for::<SitemapSource>(),
                generator.subschema_for::<FeedSource>(),
                generator.subschema_for::<S3Source>(),
                generator.subschema_for::<YoutubeSource>(),
                generator.subschema_for::<MaildirSource>(),
                generator.subschema_for::<ExecSource>(),
                generator.subschema_for::<McpSource>(),
            ]
        })
    }
}

impl Validate for SourceConfig {
    fn validate(&self) -> Result<()> {
        SourceConfig::validate(self)
    }
}

/// Deserialize a field that may be either a single string or a YAML list of strings into `Vec<String>`.
fn deserialize_string_or_list<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<String>, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string or list of strings")
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(vec![v.to_owned()])
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
            Vec::deserialize(serde::de::value::SeqAccessDeserializer::new(seq))
        }
    }
    deserializer.deserialize_any(Visitor)
}

/// Git transport protocols that can execute arbitrary commands.
const DANGEROUS_GIT_PREFIXES: &[&str] = &["ext::", "fd::"];

/// Shell-special characters that must not appear in the path component of an
/// SCP-style git URL (e.g. `user@host:path`).
const SHELL_SPECIAL: &[char] = &['$', '`', ';', '&', '|', '(', ')'];

/// Validate a git ref (branch, tag, commit) for injection and safety.
///
/// Rejects empty strings, option injection (`-`), traversal (`..`), and
/// characters outside the safe set `[a-zA-Z0-9._/@-]`.
pub(crate) fn validate_git_ref(git_ref: &str) -> Result<()> {
    anyhow::ensure!(!git_ref.is_empty(), "git ref must not be empty");
    anyhow::ensure!(
        !git_ref.starts_with('-'),
        "git ref must not start with '-' (option injection)"
    );
    anyhow::ensure!(!git_ref.contains(".."), "git ref must not contain '..'");
    anyhow::ensure!(
        git_ref
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-' | '@')),
        "git ref must only contain [a-zA-Z0-9._/@-] characters"
    );
    Ok(())
}

/// Validate a git URL for injection and transport safety.
///
/// Checks performed:
/// * Rejects dangerous git transports (`ext::`, `fd::`)
/// * Rejects URLs starting with `-` (option injection)
/// * For `scheme://` URLs: scheme must be in [`SAFE_GIT_SCHEMES`]
/// * For SCP-style URLs (`user@host:path`): the path component must not
///   contain `..` and must not contain shell-special characters
///   (`$`, backtick, `;`, `&`, `|`, `(`, `)`)
///
/// This function does NOT enforce the SCP structural format (presence of `@`
/// and `:`); that is the responsibility of the caller (config validation).
/// It is safe to call on local paths.
pub(crate) fn validate_git_url(url: &str) -> Result<()> {
    let lower = url.to_lowercase();
    for prefix in DANGEROUS_GIT_PREFIXES {
        anyhow::ensure!(
            !lower.starts_with(prefix),
            "git URL must not use the {prefix} transport (command execution risk)"
        );
    }
    anyhow::ensure!(
        !url.starts_with('-'),
        "git URL must not start with '-' (option injection)"
    );
    if url.contains("://") {
        let scheme = util::url_scheme(url);
        anyhow::ensure!(
            !scheme.is_empty() && SAFE_GIT_SCHEMES.contains(&scheme.as_str()),
            "git must use one of {SAFE_GIT_SCHEMES:?} (got {scheme:?})"
        );
    } else if url.contains('@') && url.contains(':') {
        // SCP-style: user@host:path -- validate the path portion
        let path_part = url.split_once(':').map_or("", |x| x.1);
        anyhow::ensure!(
            !path_part.contains(".."),
            "git SCP path must not contain '..'"
        );
        if let Some(bad) = path_part.chars().find(|c| SHELL_SPECIAL.contains(c)) {
            anyhow::bail!("git SCP path must not contain shell-special character {bad:?}");
        }
    }
    Ok(())
}

/// Validate a git URL at config time: enforces both safety (via
/// [`validate_git_url`]) and format (scheme or SCP syntax or absolute path).
fn validate_git_url_config(url: &str) -> Result<()> {
    validate_git_url(url)?;
    if !url.contains("://") {
        // Allow absolute paths (e.g. /path/to/bare.git) as local git repos.
        if !url.starts_with('/') {
            anyhow::ensure!(
                url.contains(':') && url.contains('@'),
                "git URL must use a supported scheme ({SAFE_GIT_SCHEMES:?}), \
                 an absolute path (/path/to/repo), or SCP format (user@host:path)"
            );
        }
    }
    Ok(())
}

/// Validate that a URL uses an allowed HTTP scheme (http or https) and has a host.
fn validate_http_url(url: &str, label: &str) -> Result<()> {
    let parsed = url::Url::parse(url).with_context(|| format!("{label}: invalid URL: {url}"))?;
    anyhow::ensure!(
        SAFE_HTTP_SCHEMES.contains(&parsed.scheme()),
        "{label}: unsupported scheme in {url:?} (expected http or https)"
    );
    anyhow::ensure!(parsed.host().is_some(), "{label}: URL has no host: {url:?}");
    Ok(())
}

/// Validate that `include` is a valid regex when provided.
fn validate_include(pattern: Option<&str>) -> Result<()> {
    if let Some(pattern) = pattern {
        regex::Regex::new(pattern)
            .with_context(|| format!("'include' is not a valid regex: {pattern}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn youtube_source(urls: &[&str]) -> SourceConfig {
        SourceConfig::Youtube(YoutubeSource {
            youtube: urls.iter().map(|s| (*s).to_owned()).collect(),
            lang: DEFAULT_YOUTUBE_LANG.to_owned(),
            topic: None,
            tags: None,
            include: None,
            max_videos: DEFAULT_MAX_VIDEOS,
            update: UpdateMode::Auto,
            processing: None,
        })
    }

    fn git_source(url: &str) -> SourceConfig {
        SourceConfig::Git(GitSource {
            git: vec![url.to_owned()],
            git_ref: None,
            glob: None,
            update: UpdateMode::Auto,
            topic: None,
            tags: None,
            processing: None,
        })
    }

    #[test]
    fn youtube_validate_rejects_file_scheme() {
        let src = youtube_source(&["file:///etc/passwd"]);
        assert!(
            src.validate().is_err(),
            "youtube source with file:// URL should be rejected"
        );
    }

    #[test]
    fn git_url_scheme_validation() {
        let cases: &[(&str, bool)] = &[
            // Valid schemes
            ("https://github.com/user/repo", true),
            ("http://github.com/user/repo", true),
            ("ssh://git@github.com/user/repo", true),
            ("git://github.com/user/repo", true),
            // SCP-style (valid)
            ("git@github.com:user/repo", true),
            ("git@gitlab.com:org/project.git", true),
            // Invalid schemes
            ("ftp://example.com/repo", false),
            ("file:///etc/passwd", false),
        ];
        for &(url, expect_ok) in cases {
            let result = git_source(url).validate();
            if expect_ok {
                result.unwrap_or_else(|e| panic!("expected {url:?} to be valid, got: {e}"));
            } else {
                assert!(result.is_err(), "expected {url:?} to be rejected");
            }
        }
    }

    #[test]
    fn git_url_injection_prevention() {
        // Dangerous transports (ext::, fd::) -- checked case-insensitively
        let dangerous = ["ext::sh -c evil", "fd::17", "EXT::evil"];
        for url in dangerous {
            let err = git_source(url).validate().unwrap_err().to_string();
            assert!(
                err.contains("transport"),
                "expected dangerous transport {url:?} to be rejected, got: {err}"
            );
        }

        // Option injection via leading dash
        assert!(
            git_source("--upload-pack=evil").validate().is_err(),
            "expected option injection to be rejected"
        );
    }

    #[test]
    fn git_url_scp_special_chars() {
        assert!(
            git_source("github.com:user/repo").validate().is_err(),
            "expected SCP URL without @ to be rejected"
        );

        let scp_special = [
            ("git@host:path$inject", '$'),
            ("git@host:path`inject`", '`'),
            ("git@host:path;rm -rf /", ';'),
            ("git@host:path&background", '&'),
            ("git@host:path|pipe", '|'),
            ("git@host:path(paren", '('),
            ("git@host:path)paren", ')'),
        ];
        for (url, bad_char) in scp_special {
            let err = git_source(url).validate().unwrap_err().to_string();
            assert!(
                err.contains(bad_char),
                "expected error for {url:?} to mention {bad_char:?}, got: {err}"
            );
        }

        let url = "git@host:../../etc/passwd";
        let err = git_source(url).validate().unwrap_err().to_string();
        assert!(
            err.contains(".."),
            "expected error for {url:?} to mention '..', got: {err}"
        );
    }

    #[test]
    fn git_ref_validation() {
        let bad_refs = [
            ("-bad", "option-injection ref should be rejected"),
            ("", "empty ref should be rejected"),
            ("foo..bar", "ref containing '..' should be rejected"),
        ];
        for (bad_ref, msg) in bad_refs {
            let src = SourceConfig::Git(GitSource {
                git: vec!["https://github.com/user/repo".to_owned()],
                git_ref: Some(bad_ref.to_owned()),
                glob: None,
                update: UpdateMode::Auto,
                topic: None,
                tags: None,
                processing: None,
            });
            assert!(src.validate().is_err(), "{msg} (ref={bad_ref:?})");
        }

        let src = SourceConfig::Git(GitSource {
            git: vec!["https://github.com/user/repo".to_owned()],
            git_ref: Some("main".to_owned()),
            glob: None,
            update: UpdateMode::Auto,
            topic: None,
            tags: None,
            processing: None,
        });
        assert!(
            src.validate().is_ok(),
            "valid ref 'main' should be accepted"
        );
    }

    #[test]
    fn source_key_error_messages() {
        let err =
            serde_yaml_ng::from_str::<SourceConfig>("sitmap: https://example.com/sitemap.xml")
                .unwrap_err()
                .to_string();
        assert!(
            err.contains("did you mean 'sitemap'?"),
            "expected suggestion for 'sitmap', got: {err}"
        );

        let err = serde_yaml_ng::from_str::<SourceConfig>("path: ./docs\nurl: https://example.com")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("path") && err.contains("url"),
            "expected both 'path' and 'url' in error, got: {err}"
        );

        let err = serde_yaml_ng::from_str::<SourceConfig>("foobar: something")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("path")
                && err.contains("url")
                && err.contains("git")
                && err.contains("sitemap")
                && err.contains("feed")
                && err.contains("s3")
                && err.contains("youtube")
                && err.contains("maildir")
                && err.contains("exec")
                && err.contains("mcp"),
            "expected all valid keys listed in error, got: {err}"
        );
    }
}
