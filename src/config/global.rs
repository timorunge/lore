use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_yaml_ng::{Mapping, Value};
use tracing::debug;

use crate::config::fetch::FetchConfig;
use crate::config::llm::LlmConfig;
use crate::config::processing::ProcessingConfig;
use crate::config::store::StoreConfig;

/// User-level defaults loaded from `~/.config/lore/config.yaml` (or platform equivalent).
///
/// Only machine/user-level settings are allowed here. Project-specific fields
/// (`name`, `description`, `base_dir`, `sources`) must live in the project config.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GlobalConfig {
    /// Default store tuning parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<StoreConfig>,
    /// Default processing parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing: Option<ProcessingConfig>,
    /// Default HTTP fetch behavior.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetch: Option<FetchConfig>,
    /// Default LLM provider and model configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmConfig>,
}

/// Return the path to the global config file.
///
/// Checks `LORE_GLOBAL_CONFIG` env var first, then falls back to
/// `config_dir() / "lore" / "config.yaml"` (`~/.config` on Unix,
/// `%APPDATA%` on Windows).
pub fn global_config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LORE_GLOBAL_CONFIG") {
        let expanded = crate::config::expand_path(&p);
        return Some(PathBuf::from(expanded));
    }
    crate::util::config_dir().map(|d| d.join("lore").join("config.yaml"))
}

/// Load the global config file as a raw YAML `Value` for merging.
///
/// Returns `Ok(None)` if the file does not exist (backward compatible).
/// Validates the file against [`GlobalConfig`] to produce clear error messages
/// before returning the raw value.
pub(crate) fn load_global_defaults() -> Result<Option<Value>> {
    let Some(path) = global_config_path() else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }

    debug!(path = %path.display(), "loading global config");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read global config: {}", path.display()))?;

    if content.trim().is_empty() {
        return Ok(None);
    }

    // Parse as typed struct first for validation and clear error messages.
    serde_yaml_ng::from_str::<GlobalConfig>(&content)
        .with_context(|| format!("failed to parse global config: {}", path.display()))?;

    // Re-parse as raw Value for merging.
    let value: Value = serde_yaml_ng::from_str(&content)
        .with_context(|| format!("failed to parse global config: {}", path.display()))?;

    Ok(Some(value))
}

/// Merge global defaults under project config values.
///
/// For each top-level block (`fetch`, `store`, `processing`, `llm`):
/// - If the project does not have the block, the global block is used wholesale.
/// - If both have the block as mappings, global fields fill in only where the
///   project has no value (project always wins).
/// - Project-only keys (`name`, `description`, `base_dir`, `sources`) are never
///   touched by this merge.
pub(crate) fn merge_config_values(global: Value, project: Value) -> Value {
    let Value::Mapping(global_map) = global else {
        return project;
    };
    let Value::Mapping(mut project_map) = project else {
        return Value::Mapping(Mapping::new());
    };

    for (key, global_val) in global_map {
        if !is_mergeable_key(&key) {
            continue;
        }
        match project_map.entry(key) {
            serde_yaml_ng::mapping::Entry::Vacant(entry) => {
                entry.insert(global_val);
            }
            serde_yaml_ng::mapping::Entry::Occupied(mut entry) => {
                if let (Value::Mapping(global_sub), Value::Mapping(project_sub)) =
                    (global_val, entry.get_mut())
                {
                    merge_mapping_shallow(global_sub, project_sub);
                }
            }
        }
    }

    Value::Mapping(project_map)
}

/// Only these top-level keys are merged from the global config.
fn is_mergeable_key(key: &Value) -> bool {
    matches!(key.as_str(), Some("fetch" | "store" | "processing" | "llm"))
}

/// Shallow merge: copy global fields into project only where absent.
fn merge_mapping_shallow(global: Mapping, project: &mut Mapping) {
    for (key, val) in global {
        if !project.contains_key(&key) {
            project.insert(key, val);
        }
    }
}

/// Scaffold content for `lore init --global`.
pub fn global_config_scaffold() -> &'static str {
    "\
# lore global config
# User-level defaults applied to all projects on this machine.
# Project configs override these values; LORE_* env vars override both.
#
# Location: this file
# Docs: https://github.com/timorunge/lore/blob/main/docs/configuration.md

# fetch:
#   delay: 0.5
#   concurrency: 4
#   timeout: 30.0
#   respect_robots: true
#   user_agent: \"my-bot/1.0\"
#   prefer_markdown: true
#   cache_ttl: 3600.0
#   max_download_mb: 50

# store:
#   writer_heap_mb: 256
#   doc_store_cache_blocks: 500

# processing:
#   concurrency: 4
#   max_file_mb: 50
#   max_chunk_chars: 1600
#   min_chunk_chars: 30
#   extraction_timeout_secs: 120

# llm:
#   provider: ollama
#   ollama_url: http://localhost:11434
#   ollama_model: llama3.2
#   detect_topics:
#     enabled: false
#   summarize_docs:
#     enabled: false
#   enrich_chunks:
#     enabled: false
"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml_value(s: &str) -> Value {
        serde_yaml_ng::from_str(s).unwrap()
    }

    #[test]
    fn merge_global_fills_missing_block() {
        let global = yaml_value("fetch:\n  concurrency: 8\n  max_retries: 5\n");
        let project = yaml_value("name: test\nsources: []\n");
        let merged = merge_config_values(global, project);
        let m = merged.as_mapping().unwrap();
        let fetch = m.get("fetch").unwrap().as_mapping().unwrap();
        assert_eq!(fetch.get("concurrency").unwrap().as_u64().unwrap(), 8);
        assert_eq!(fetch.get("max_retries").unwrap().as_u64().unwrap(), 5);
        assert_eq!(m.get("name").unwrap().as_str().unwrap(), "test");
    }

    #[test]
    fn merge_project_wins_field_level() {
        let global = yaml_value("fetch:\n  concurrency: 8\n  max_retries: 5\n");
        let project = yaml_value("fetch:\n  concurrency: 2\n");
        let merged = merge_config_values(global, project);
        let fetch = merged
            .as_mapping()
            .unwrap()
            .get("fetch")
            .unwrap()
            .as_mapping()
            .unwrap();
        assert_eq!(
            fetch.get("concurrency").unwrap().as_u64().unwrap(),
            2,
            "project value should win"
        );
        assert_eq!(
            fetch.get("max_retries").unwrap().as_u64().unwrap(),
            5,
            "global value should fill in missing field"
        );
    }

    #[test]
    fn merge_skips_non_mergeable_keys() {
        let global = yaml_value("name: global-name\nfetch:\n  concurrency: 8\n");
        let project = yaml_value("name: project-name\nsources: []\n");
        let merged = merge_config_values(global, project);
        let m = merged.as_mapping().unwrap();
        assert_eq!(
            m.get("name").unwrap().as_str().unwrap(),
            "project-name",
            "global 'name' should not overwrite project 'name'"
        );
        assert!(
            m.get("fetch").is_some(),
            "mergeable key 'fetch' should still be copied from global"
        );
    }

    #[test]
    fn merge_global_fills_fetch_from_global() {
        let global = yaml_value("fetch:\n  delay: 2.0\n");
        let project = yaml_value("name: project-name\nsources: []\n");
        let merged = merge_config_values(global, project);
        let m = merged.as_mapping().unwrap();
        assert!(
            m.get("fetch").is_some(),
            "fetch should be filled from global"
        );
    }

    #[test]
    fn merge_llm_block_wholesale() {
        let global = yaml_value("llm:\n  provider: ollama\n  ollama_model: llama3.2\n");
        let project = yaml_value("sources: []\n");
        let merged = merge_config_values(global, project);
        let llm = merged
            .as_mapping()
            .unwrap()
            .get("llm")
            .unwrap()
            .as_mapping()
            .unwrap();
        assert_eq!(llm.get("provider").unwrap().as_str().unwrap(), "ollama");
        assert_eq!(
            llm.get("ollama_model").unwrap().as_str().unwrap(),
            "llama3.2"
        );
    }

    #[test]
    fn merge_empty_global() {
        let global = yaml_value("{}");
        let project = yaml_value("name: test\nsources: []\nfetch:\n  delay: 1.0\n");
        let merged = merge_config_values(global, project.clone());
        assert_eq!(merged, project);
    }

    #[test]
    fn global_config_rejects_sources() {
        let yaml = "sources:\n  - path: test\n";
        let result = serde_yaml_ng::from_str::<GlobalConfig>(yaml);
        assert!(result.is_err(), "global config should reject 'sources'");
    }

    #[test]
    fn global_config_rejects_name() {
        let yaml = "name: oops\n";
        let result = serde_yaml_ng::from_str::<GlobalConfig>(yaml);
        assert!(result.is_err(), "global config should reject 'name'");
    }

    #[test]
    fn global_config_accepts_valid_blocks() {
        let yaml = "fetch:\n  delay: 1.0\nstore:\n  writer_heap_mb: 512\n";
        let cfg: GlobalConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(cfg.fetch.is_some());
        assert!(cfg.store.is_some());
        assert!(cfg.processing.is_none());
        assert!(cfg.llm.is_none());
    }

    #[test]
    fn global_config_path_env_override() {
        let original = std::env::var("LORE_GLOBAL_CONFIG").ok();
        let test_path = std::env::temp_dir().join("my-lore-config.yaml");
        // SAFETY: test-only env mutation; tests run sequentially within this binary.
        unsafe { std::env::set_var("LORE_GLOBAL_CONFIG", &test_path) };
        let path = global_config_path();
        if let Some(ref orig) = original {
            // SAFETY: test-only env mutation.
            unsafe { std::env::set_var("LORE_GLOBAL_CONFIG", orig) };
        } else {
            // SAFETY: test-only env mutation.
            unsafe { std::env::remove_var("LORE_GLOBAL_CONFIG") };
        }
        assert_eq!(path, Some(test_path));
    }
}
