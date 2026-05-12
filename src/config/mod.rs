mod expand;
mod fetch;
mod global;
mod llm;
mod processing;
mod source;
mod store;
pub mod transforms;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::util;

pub use expand::expand_path;
use expand::replace_env_vars;
pub use fetch::FetchConfig;
pub use global::{GlobalConfig, global_config_path, global_config_scaffold};
#[cfg(feature = "llm")]
pub(crate) use llm::{DEFAULT_OLLAMA_URL, LlmProvider};
pub use llm::{DetectTopicsConfig, EnrichChunksConfig, LlmConfig, SummarizeDocsConfig};
use processing::validate_profile;
pub use processing::{
    ContentFilterConfig, ProcessingConfig, ProcessingLimits, ProcessingProfile, ProcessingRef,
};
pub use source::{
    ExecOutputMode, ExecSource, FeedSource, GitSource, LocalSource, MaildirSource, McpResources,
    McpSource, McpToolCall, McpTransport, S3Source, SitemapSource, SourceConfig, UpdateMode,
    UrlSource, YoutubeSource,
};
#[cfg(feature = "ingest")]
pub(crate) use source::{validate_git_ref, validate_git_url};
pub use store::StoreConfig;
pub use transforms::{ExtractMode, MetadataField, Transform};

/// A resolved config file ready for use by the CLI, server, or programmatic consumers.
pub struct ResolvedConfig {
    pub config: IngestConfig,
    pub config_path: PathBuf,
}

/// Common interface for config types that can validate their own invariants.
pub(crate) trait Validate {
    fn validate(&self) -> anyhow::Result<()>;
}

/// Top-level configuration parsed from a lore YAML config file.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IngestConfig {
    /// Optional display name for this knowledge base.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional description shown in `lore info` output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Base directory for resolving local source paths.
    /// Resolved relative to the config file's directory. Default: "." (config dir).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<String>,
    #[serde(default)]
    pub sources: Vec<SourceConfig>,
    #[serde(default)]
    pub store: StoreConfig,
    #[serde(default)]
    pub processing: ProcessingConfig,
    #[serde(default)]
    pub fetch: FetchConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmConfig>,
}

impl IngestConfig {
    /// Parse, expand, and validate a config from a YAML file.
    ///
    /// Merges global user-level defaults (if present) under the project
    /// config before deserialization. Precedence: global < project < env vars.
    pub fn from_yaml(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config: {}", path.display()))?;

        let mut config: IngestConfig = if let Some(global_val) = global::load_global_defaults()? {
            let project_val: serde_yaml_ng::Value = serde_yaml_ng::from_str(&content)
                .with_context(|| format!("failed to parse config: {}", path.display()))?;
            let merged = global::merge_config_values(global_val, project_val);
            serde_yaml_ng::from_value(merged)
                .with_context(|| format!("failed to parse config: {}", path.display()))?
        } else {
            serde_yaml_ng::from_str(&content)
                .with_context(|| format!("failed to parse config: {}", path.display()))?
        };

        let config_dir = path.parent().unwrap_or(Path::new("."));

        // Resolve base_dir: defaults to "." (= config_dir). Supports ~, $VAR, absolute paths.
        let root_raw = config.base_dir.as_deref().unwrap_or(".");
        let root_expanded = expand_path(root_raw);
        let root_path = {
            let p = Path::new(&root_expanded);
            if p.is_relative() {
                util::normalize_path(&config_dir.join(p))
            } else {
                p.to_owned()
            }
        };

        // Store path resolves relative to config_dir (internal infrastructure).
        config.store.path = expand_path(&config.store.path);
        for source in &mut config.sources {
            if let SourceConfig::Local(s) = source {
                for p in &mut s.path {
                    *p = expand_path(p);
                    let path = Path::new(p.as_str());
                    if path.is_relative() {
                        *p = root_path.join(path).to_string_lossy().into_owned();
                    }
                }
            }
            if let Some(map) = source.headers_mut() {
                for val in map.values_mut() {
                    *val = replace_env_vars(val, false, Some("LORE_"), None).into_owned();
                }
            }
            if let SourceConfig::Mcp(s) = source
                && let Some(ref mut token) = s.token
            {
                *token = replace_env_vars(token, false, Some("LORE_"), None).into_owned();
            }
        }

        config.apply_env_overrides()?;
        config.validate()?;
        Ok(config)
    }

    /// Resolve `store.path` to an absolute directory path relative to the config file.
    pub fn store_dir(&self, config_path: &Path) -> PathBuf {
        let dir = config_path.parent().unwrap_or(Path::new("."));
        util::normalize_path(&dir.join(&self.store.path))
    }

    /// Apply `LORE_*` environment variable overrides for store path, fetch settings, and processing limits.
    fn apply_env_overrides(&mut self) -> Result<()> {
        if let Ok(v) = std::env::var("LORE_STORE_PATH") {
            self.store.path = expand_path(&v);
        }
        if let Some(v) = parse_env("LORE_FETCH_DELAY")? {
            self.fetch.delay = v;
        }
        if let Some(v) = parse_env("LORE_FETCH_CONCURRENCY")? {
            self.fetch.concurrency = v;
        }
        if let Some(v) = parse_env("LORE_FETCH_TIMEOUT")? {
            self.fetch.timeout = v;
        }
        if let Some(v) = parse_env("LORE_MAX_CHUNK_CHARS")? {
            self.processing.max_chunk_chars = v;
        }
        if let Some(v) = parse_env("LORE_MIN_CHUNK_CHARS")? {
            self.processing.min_chunk_chars = v;
        }
        if let Some(v) = parse_env::<f64>("LORE_FETCH_CACHE_TTL")? {
            self.fetch.cache_ttl = Some(v);
        }
        if let Some(v) = parse_env("LORE_MAX_FILE_MB")? {
            self.processing.max_file_mb = v;
        }
        if let Some(v) = parse_env("LORE_STORE_WRITER_HEAP_MB")? {
            self.store.writer_heap_mb = v;
        }
        if let Some(v) = parse_env::<bool>("LORE_STORE_PHRASE_SEARCH")? {
            self.store.phrase_search = v;
        }
        if let Some(v) = parse_env("LORE_STORE_DOC_STORE_CACHE_BLOCKS")? {
            self.store.doc_store_cache_blocks = v;
        }
        Ok(())
    }
}

impl Validate for IngestConfig {
    fn validate(&self) -> anyhow::Result<()> {
        // Expansion must complete before this point; validate structural invariants first.
        Validate::validate(&self.store)?;
        Validate::validate(&self.processing)?;
        Validate::validate(&self.fetch)?;

        for (i, source) in self.sources.iter().enumerate() {
            Validate::validate(source)
                .with_context(|| format!("sources[{i}] ({}) is invalid", source.label()))?;

            if let SourceConfig::Local(s) = source {
                for p in &s.path {
                    if !Path::new(p).exists() {
                        warn!(path = p.as_str(), "local source path does not exist");
                    }
                }
            }

            if let Some(ProcessingRef::Named(name)) = source.processing() {
                anyhow::ensure!(
                    self.processing.presets.contains_key(name),
                    "sources[{i}] ({}) references unknown preset: {name:?}",
                    source.label(),
                );
            }
            if let Some(ProcessingRef::Inline(p)) = source.processing() {
                validate_profile(p, &format!("sources[{i}]"))?;
            }

            #[cfg(not(feature = "s3"))]
            if matches!(source, SourceConfig::S3(_)) {
                anyhow::bail!(
                    "sources[{i}]: S3 sources require the 's3' feature; \
                     recompile lore with: cargo install lore --features s3"
                );
            }

            #[cfg(not(feature = "mcp"))]
            if matches!(source, SourceConfig::Mcp(_)) {
                anyhow::bail!(
                    "sources[{i}]: MCP sources require the 'mcp' feature; \
                     recompile lore with: cargo install lore --features mcp"
                );
            }
        }

        if let Some(ref llm) = self.llm {
            Validate::validate(llm).context("llm config is invalid")?;
        }

        #[cfg(not(feature = "llm"))]
        if self.llm.is_some() {
            anyhow::bail!(
                "llm config requires the 'llm' feature; recompile lore with: cargo install lore --features llm"
            );
        }

        Ok(())
    }
}

/// Read an environment variable and parse it into `T`; returns `None` if unset, errors if malformed.
fn parse_env<T: std::str::FromStr>(name: &str) -> Result<Option<T>> {
    let Ok(raw) = std::env::var(name) else {
        return Ok(None);
    };
    raw.parse().map(Some).map_err(|_| {
        anyhow::anyhow!(
            "invalid value for {name}: {raw:?} (expected {})",
            std::any::type_name::<T>()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    fn parse_source(yaml: &str) -> Result<SourceConfig> {
        serde_yaml_ng::from_str(yaml).map_err(Into::into)
    }

    #[test]
    fn parse_and_validate_sources() {
        let valid = [
            ("path: test/docs\nglob: '**/*.md'", "Local", "test/docs"),
            (
                "url: https://example.com/doc.md",
                "Url",
                "https://example.com/doc.md",
            ),
            (
                "git: https://github.com/user/repo\nref: main",
                "Git",
                "https://github.com/user/repo",
            ),
            (
                "sitemap: https://example.com/sitemap.xml\ninclude: '/docs/'",
                "Sitemap",
                "https://example.com/sitemap.xml",
            ),
            (
                "feed: https://example.com/rss.xml",
                "Feed",
                "https://example.com/rss.xml",
            ),
            ("s3: s3://my-bucket/prefix", "S3", "s3://my-bucket/prefix"),
        ];
        for (yaml, kind, expected_label) in valid {
            let src = parse_source(yaml).unwrap_or_else(|e| panic!("{kind} parse failed: {e}"));
            src.validate()
                .unwrap_or_else(|e| panic!("{kind} validate failed: {e}"));
            assert_eq!(src.label(), expected_label, "{kind} label mismatch");
        }

        let src = parse_source("url:\n  - https://a.com/1\n  - https://b.com/2").unwrap();
        if let SourceConfig::Url(s) = &src {
            assert_eq!(s.url.len(), 2);
        }
        src.validate().unwrap();

        let src = parse_source("url: https://example.com/doc.md\nheaders:\n  Authorization: \"Bearer token123\"\n  X-Custom: value").unwrap();
        if let SourceConfig::Url(s) = &src {
            assert_eq!(s.headers.get("Authorization").unwrap(), "Bearer token123");
            assert_eq!(s.headers.get("X-Custom").unwrap(), "value");
        }
        src.validate().unwrap();

        let src = parse_source(
            "sitemap: https://example.com/sitemap.xml\nheaders:\n  Cookie: session=abc",
        )
        .unwrap();
        if let SourceConfig::Sitemap(s) = &src {
            assert_eq!(s.headers.get("Cookie").unwrap(), "session=abc");
        }
        src.validate().unwrap();

        let src = parse_source("path: test\ntopic: My Topic").unwrap();
        if let SourceConfig::Local(s) = &src {
            assert_eq!(s.topic.as_deref(), Some("My Topic"));
        }

        let invalid = [
            "s3: my-bucket/prefix",
            "url: ftp://example.com/file",
            "git: ftp://example.com/repo",
            "sitemap: https://example.com/sitemap.xml\ninclude: '[invalid'",
        ];
        for yaml in invalid {
            let src = parse_source(yaml).unwrap();
            assert!(src.validate().is_err(), "expected error for: {yaml}");
        }
    }

    #[test]
    fn processing_resolution() {
        let yaml = r"
name: Test
sources:
  - path: test/docs
    processing: rfc
  - path: test/override
    processing:
      max_chunk_chars: 3000
  - path: test/defaults
processing:
  max_chunk_chars: 1600
  presets:
    rfc:
      max_chunk_chars: 800
      min_chunk_chars: 20
      extract: none
      pipeline:
        - type: strip_lines
          first: 10
        - type: extract_builtin
";
        let config: IngestConfig = serde_yaml_ng::from_str(yaml).unwrap();

        let profile = config
            .processing
            .resolve(config.sources[0].processing())
            .unwrap();
        assert_eq!(profile.max_chunk_chars, 800);
        assert_eq!(profile.min_chunk_chars, 20);
        assert_eq!(profile.extract, ExtractMode::None);
        assert_eq!(profile.pipeline.len(), 2);

        let profile = config
            .processing
            .resolve(config.sources[1].processing())
            .unwrap();
        assert_eq!(profile.max_chunk_chars, 3000);
        assert_eq!(profile.extract, ExtractMode::Auto);

        let profile = config
            .processing
            .resolve(config.sources[2].processing())
            .unwrap();
        assert_eq!(profile.max_chunk_chars, 1600);

        let bad_yaml = "name: T\nsources:\n  - path: test\n    processing: nonexistent\n";
        let bad: IngestConfig = serde_yaml_ng::from_str(bad_yaml).unwrap();
        let result = bad.processing.resolve(bad.sources[0].processing());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("nonexistent"));

        let named = parse_source("path: test\nprocessing: code").unwrap();
        assert!(matches!(named.processing(), Some(ProcessingRef::Named(n)) if n == "code"));
        let inline = parse_source("path: test\nprocessing:\n  max_chunk_chars: 900").unwrap();
        assert!(matches!(
            inline.processing(),
            Some(ProcessingRef::Inline(_))
        ));
        let none = parse_source("path: test").unwrap();
        assert!(none.processing().is_none());
    }

    #[test]
    fn pipeline_transforms() {
        let yaml = r"
name: Test
sources:
  - path: test/docs
processing:
  pipeline:
    - type: strip_lines
      first: 5
      last: 2
    - type: replace_text
      pattern: '(?m)^CONFIDENTIAL.*$'
    - type: extract_builtin
    - type: extract_metadata
      pattern: 'Author:\s+(.+)'
      field: author
";
        let config: IngestConfig = serde_yaml_ng::from_str(yaml).unwrap();
        let p = &config.processing.pipeline;
        assert_eq!(p.len(), 4);
        assert!(matches!(&p[0], Transform::StripLines(s) if s.first == 5 && s.last == 2));
        assert!(matches!(&p[1], Transform::ReplaceText(t) if t.pattern == "(?m)^CONFIDENTIAL.*$"));
        assert!(matches!(&p[2], Transform::ExtractBuiltin));
        assert!(matches!(
            &p[3],
            Transform::ExtractMetadata(t) if t.field == MetadataField::Author
        ));

        let bad_yaml = "name: T\nsources:\n  - path: test\nprocessing:\n  pipeline:\n    - type: replace_text\n      pattern: '[invalid'\n";
        let bad: IngestConfig = serde_yaml_ng::from_str(bad_yaml).unwrap();
        assert!(bad.processing.validate().is_err());
    }

    #[test]
    fn deny_unknown_fields() {
        let source_cases = [
            ("path: test\ntypo_field: oops", "Local"),
            ("url: https://example.com\ntypo_field: oops", "Url"),
            ("git: https://github.com/u/r\ntypo_field: oops", "Git"),
            (
                "sitemap: https://example.com/sitemap.xml\ntypo_field: oops",
                "Sitemap",
            ),
            (
                "feed: https://example.com/rss.xml\ntypo_field: oops",
                "Feed",
            ),
            ("s3: s3://my-bucket/key\ntypo_field: oops", "S3"),
            (
                "youtube: https://youtube.com/watch?v=abc\ntypo_field: oops",
                "Youtube",
            ),
        ];
        for (yaml, kind) in source_cases {
            let result: Result<SourceConfig> = serde_yaml_ng::from_str(yaml).map_err(Into::into);
            assert!(
                result.is_err(),
                "{kind} source should reject unknown field 'typo_field'"
            );
        }

        let yaml = "name: T\nsources:\n  - path: test\nprocessing:\n  typo_field: oops\n";
        let result: Result<IngestConfig> = serde_yaml_ng::from_str(yaml).map_err(Into::into);
        assert!(
            result.is_err(),
            "ProcessingConfig should reject unknown field 'typo_field'"
        );

        // ProcessingProfile in a preset rejects unknown fields
        let yaml = "name: T\nsources:\n  - path: test\nprocessing:\n  presets:\n    mypreset:\n      typo_field: oops\n";
        let result: Result<IngestConfig> = serde_yaml_ng::from_str(yaml).map_err(Into::into);
        assert!(
            result.is_err(),
            "ProcessingProfile preset should reject unknown field 'typo_field'"
        );

        // Inline per-source ProcessingRef rejects unknown fields
        let yaml = "name: T\nsources:\n  - path: test\n    processing:\n      max_chunk_chars: 800\n      typo_field: oops\n";
        let result: Result<IngestConfig> = serde_yaml_ng::from_str(yaml).map_err(Into::into);
        assert!(
            result.is_err(),
            "inline per-source ProcessingRef should reject unknown field 'typo_field'"
        );

        let transform_cases = [
            (
                "name: T\nsources:\n  - path: test\nprocessing:\n  pipeline:\n    - type: strip_lines\n      typo_field: oops\n",
                "strip_lines",
            ),
            (
                "name: T\nsources:\n  - path: test\nprocessing:\n  pipeline:\n    - type: replace_text\n      pattern: foo\n      typo_field: oops\n",
                "replace_text",
            ),
            (
                "name: T\nsources:\n  - path: test\nprocessing:\n  pipeline:\n    - type: extract_metadata\n      pattern: foo\n      field: title\n      typo_field: oops\n",
                "extract_metadata",
            ),
        ];
        for (yaml, step_type) in transform_cases {
            let result: Result<IngestConfig> = serde_yaml_ng::from_str(yaml).map_err(Into::into);
            assert!(
                result.is_err(),
                "{step_type} transform step should reject unknown field 'typo_field'"
            );
        }

        let yaml = "name: T\nsources:\n  - path: test\nfetch:\n  typo_field: oops\n";
        let result: Result<IngestConfig> = serde_yaml_ng::from_str(yaml).map_err(Into::into);
        assert!(
            result.is_err(),
            "FetchConfig should reject unknown field 'typo_field'"
        );
    }

    #[test]
    fn url_empty_list_is_rejected() {
        // An empty URL list should fail validation.
        let src: SourceConfig = serde_yaml_ng::from_str("url: []").unwrap();
        assert!(
            src.validate().is_err(),
            "empty url list should fail validation"
        );
    }

    #[test]
    fn validate_trait_rejects_zero_values() {
        let cfg = FetchConfig {
            concurrency: 0,
            ..Default::default()
        };
        assert!(
            Validate::validate(&cfg).is_err(),
            "FetchConfig with zero concurrency should fail Validate::validate"
        );

        let cfg = ProcessingConfig {
            max_chunk_chars: 0,
            ..Default::default()
        };
        assert!(
            Validate::validate(&cfg).is_err(),
            "ProcessingConfig with max_chunk_chars=0 should fail Validate::validate"
        );

        let cfg = StoreConfig {
            writer_heap_mb: 0,
            ..Default::default()
        };
        assert!(
            Validate::validate(&cfg).is_err(),
            "StoreConfig with writer_heap_mb=0 should fail Validate::validate"
        );
    }

    #[test]
    fn base_dir_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Create project structure: subdir/config.yaml pointing at ../data
        let subdir = root.join("subdir");
        std::fs::create_dir_all(&subdir).unwrap();
        let data_dir = root.join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(data_dir.join("test.md"), b"hello").unwrap();

        // base_dir: .. -- sources resolve from parent of config dir
        let config_path = subdir.join("config.yaml");
        std::fs::write(
            &config_path,
            "base_dir: ..\nsources:\n  - path: ./data\n    glob: '**/*.md'\n",
        )
        .unwrap();
        let cfg = IngestConfig::from_yaml(&config_path).unwrap();
        if let SourceConfig::Local(s) = &cfg.sources[0] {
            let resolved = PathBuf::from(&s.path[0]);
            assert!(
                resolved.ends_with("data"),
                "base_dir: .. should resolve ./data from parent, got: {resolved:?}"
            );
            assert!(
                !s.path[0].contains("subdir"),
                "resolved path should not contain subdir: {}",
                s.path[0]
            );
        } else {
            panic!("expected Local source");
        }

        // root omitted -- sources resolve from config dir (the default)
        std::fs::write(
            &config_path,
            "sources:\n  - path: ./data\n    glob: '**/*.md'\n",
        )
        .unwrap();
        let cfg = IngestConfig::from_yaml(&config_path).unwrap();
        if let SourceConfig::Local(s) = &cfg.sources[0] {
            assert!(
                s.path[0].contains("subdir"),
                "without root, ./data should resolve inside config dir: {}",
                s.path[0]
            );
        } else {
            panic!("expected Local source");
        }

        // root as absolute path
        let abs_root = root.to_str().unwrap().replace('\\', "/");
        std::fs::write(
            &config_path,
            format!("base_dir: {abs_root}\nsources:\n  - path: ./data\n    glob: '**/*.md'\n"),
        )
        .unwrap();
        let cfg = IngestConfig::from_yaml(&config_path).unwrap();
        if let SourceConfig::Local(s) = &cfg.sources[0] {
            let resolved = PathBuf::from(&s.path[0]);
            assert!(
                resolved.ends_with("data"),
                "absolute root should resolve ./data from root, got: {resolved:?}"
            );
        } else {
            panic!("expected Local source");
        }
    }

    #[test]
    fn env_override_store_path_and_numeric_fields() {
        let tmp = tempfile::tempdir().unwrap();
        // Write a minimal config with no sources (sources list is optional).
        let config_path = tmp.path().join("lore.yaml");
        std::fs::write(&config_path, "sources: []\n").unwrap();

        // LORE_STORE_PATH overrides store.path.
        let store_dir = tmp
            .path()
            .join("custom_store")
            .to_string_lossy()
            .into_owned();
        // SAFETY: test-only env mutation; tests run sequentially within this binary.
        unsafe {
            std::env::set_var("LORE_STORE_PATH", &store_dir);
            std::env::set_var("LORE_STORE_WRITER_HEAP_MB", "512");
            std::env::set_var("LORE_MAX_CHUNK_CHARS", "3200");
        }

        let result = IngestConfig::from_yaml(&config_path);

        // SAFETY: same as above.
        unsafe {
            std::env::remove_var("LORE_STORE_PATH");
            std::env::remove_var("LORE_STORE_WRITER_HEAP_MB");
            std::env::remove_var("LORE_MAX_CHUNK_CHARS");
        }

        let cfg = result.expect("config should parse with env overrides");
        assert_eq!(
            cfg.store.path, store_dir,
            "LORE_STORE_PATH should override store.path"
        );
        assert_eq!(
            cfg.store.writer_heap_mb, 512,
            "LORE_STORE_WRITER_HEAP_MB should override store.writer_heap_mb"
        );
        assert_eq!(
            cfg.processing.max_chunk_chars, 3200,
            "LORE_MAX_CHUNK_CHARS should override processing.max_chunk_chars"
        );
    }
}
