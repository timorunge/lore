use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::TempDir;
#[cfg(feature = "test-support")]
use wiremock::matchers::{method, path};
#[cfg(feature = "test-support")]
use wiremock::{Mock, MockServer, ResponseTemplate};

#[cfg(feature = "test-support")]
pub async fn mount_robots_404(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(404))
        .mount(server)
        .await;
}

#[cfg(feature = "test-support")]
pub fn run_ingest_with_loopback(dir: &Path, config: &str) -> PathBuf {
    let config_path = dir.join("lore.yaml");
    fs::write(&config_path, config).unwrap();

    lore()
        .env("LORE_TEST_ALLOW_LOOPBACK", "1")
        .args(["ingest", "--config"])
        .arg(&config_path)
        .assert()
        .success();

    config_path
}

pub fn lore() -> Command {
    Command::cargo_bin("lore").unwrap()
}

pub fn run_ingest(dir: &Path, config: &str) -> PathBuf {
    let config_path = dir.join("lore.yaml");
    fs::write(&config_path, config).unwrap();

    lore()
        .args(["ingest", "--config"])
        .arg(&config_path)
        .assert()
        .success();

    config_path
}

/// Shared fixture: 3 markdown files with frontmatter, returns (tempdir, config_path).
/// Config sets `topic: Documentation`.
pub fn ingest_fixture() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(&docs).unwrap();

    fs::write(
        docs.join("architecture.md"),
        "# Architecture\n\nThe system uses a modular architecture with separate components \
         for ingestion, storage, and serving. Each module communicates through well-defined \
         interfaces.\n\n## Components\n\nThe main components are the ingest pipeline, the \
         Tantivy backend, and the MCP server.\n",
    )
    .unwrap();

    fs::write(
        docs.join("getting-started.md"),
        "---\ntitle: Getting Started Guide\nauthor: Test Author\nlang: en\ntags:\n  - tutorial\n  - setup\n---\n\n\
         # Getting Started\n\nThis guide helps you set up the project from scratch. Follow \
         these steps to get a working installation.\n\n## Prerequisites\n\nYou need Rust 1.75+ \
         and a Unix-like operating system.\n\n## Installation\n\nRun `cargo install lore` to \
         install the binary.\n",
    )
    .unwrap();

    fs::write(
        docs.join("api-reference.md"),
        "# API Reference\n\nThis document describes the public API surface of the project.\n\n\
         ## Search\n\nThe search endpoint accepts free-text queries and returns ranked results.\n\n\
         ## Topics\n\nTopics group related documents together for browsing.\n",
    )
    .unwrap();

    let config = format!(
        "name: Test KB\ndescription: A test knowledge base.\nsources:\n  - path: {}\n    \
         glob: \"**/*.md\"\n    topic: Documentation\nstore:\n  path: test_index\n\
         processing:\n  max_chunk_chars: 800\n  min_chunk_chars: 20\n",
        docs.display()
    );
    let config_path = run_ingest(dir.path(), &config);
    (dir, config_path)
}

pub fn collect_json(cmd: &mut Command) -> serde_json::Value {
    let out = cmd.assert().success().get_output().stdout.clone();
    serde_json::from_slice(&out).expect("command output should be valid JSON")
}

fn store_counts(config_path: &Path) -> (u64, u64) {
    let json = collect_json(
        lore()
            .args(["info", "--config"])
            .arg(config_path)
            .arg("--json"),
    );
    let documents = json["documents"].as_u64().unwrap_or(0);
    let chunks = json["chunks"].as_u64().unwrap_or(0);
    (documents, chunks)
}

pub fn assert_store_counts(config_path: &Path, min_docs: usize, min_chunks: usize) {
    let (docs, chunks) = store_counts(config_path);
    assert!(
        docs >= min_docs as u64,
        "expected >= {min_docs} documents, got {docs}"
    );
    assert!(
        chunks >= min_chunks as u64,
        "expected >= {min_chunks} chunks, got {chunks}"
    );
}

pub fn assert_store_exact_counts(config_path: &Path, docs: u64, min_chunks: u64) {
    let (actual_docs, actual_chunks) = store_counts(config_path);
    assert_eq!(
        actual_docs, docs,
        "expected exactly {docs} documents, got {actual_docs}"
    );
    assert!(
        actual_chunks >= min_chunks,
        "expected >= {min_chunks} chunks, got {actual_chunks}"
    );
}

pub fn assert_search_hit(config_path: &Path, query: &str) {
    let json = collect_json(
        lore()
            .args(["search", "--config"])
            .arg(config_path)
            .arg("--json")
            .arg(query),
    );
    let items = json["items"].as_array().expect("items should be an array");
    assert!(
        !items.is_empty(),
        "search --json for {query:?} returned zero items"
    );
    let query_lower = query.to_lowercase();
    let first_word = query_lower
        .split_whitespace()
        .next()
        .unwrap_or(query_lower.as_str());
    let body = items[0]["body"]
        .as_str()
        .expect("first item should have a body field");
    assert!(
        body.to_lowercase().contains(first_word),
        "first result body for {query:?} does not contain {first_word:?}: {body}"
    );
}

pub fn collect_stdout(cmd: &mut Command) -> String {
    let out = cmd.assert().success().get_output().stdout.clone();
    String::from_utf8(out).unwrap()
}

pub fn assert_docs_listed(config_path: &Path, substrings: &[&str]) {
    let stdout = collect_stdout(
        lore()
            .args(["docs", "--config"])
            .arg(config_path)
            .arg("--json"),
    );
    for s in substrings {
        assert!(stdout.contains(s), "expected docs output to contain {s:?}");
    }
}

pub fn assert_topics_listed(config_path: &Path, substrings: &[&str]) {
    let stdout = collect_stdout(lore().args(["topics", "--config"]).arg(config_path));
    for s in substrings {
        assert!(
            stdout.contains(s),
            "expected topics output to contain {s:?}"
        );
    }
}
