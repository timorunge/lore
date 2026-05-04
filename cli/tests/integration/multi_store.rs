use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;

use crate::helpers::{collect_json, collect_stdout, lore, run_ingest};

fn single_doc_store(filename: &str, content: &str, topic: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(&docs).unwrap();
    fs::write(docs.join(filename), content).unwrap();
    let config = format!(
        "sources:\n  - path: {}\n    glob: \"**/*.md\"\n    topic: {topic}\n\
         store:\n  path: test_index\n",
        docs.display()
    );
    let config_path = run_ingest(dir.path(), &config);
    (dir, config_path)
}

fn two_stores() -> (TempDir, PathBuf, TempDir, PathBuf) {
    let (dir1, c1) = single_doc_store(
        "animals.md",
        "# Animals\n\nCats and dogs are popular pets. \
         Domestic animals have lived with humans for thousands of years.",
        "Animals",
    );
    let (dir2, c2) = single_doc_store(
        "rust.md",
        "# Rust\n\nRust is a systems programming language focused on \
         safety and performance. It prevents memory errors at compile time.",
        "Programming",
    );
    (dir1, c1, dir2, c2)
}

fn two_stores_shared_topic() -> (TempDir, PathBuf, TempDir, PathBuf) {
    let (dir1, c1) = single_doc_store(
        "animals.md",
        "# Land Animals\n\nCats and dogs are popular pets. \
         Domestic animals have lived with humans for thousands of years.",
        "Wildlife",
    );
    let (dir2, c2) = single_doc_store(
        "marine.md",
        "# Marine Life\n\nWhales and dolphins live in the ocean. \
         Coral reefs support thousands of species.",
        "Wildlife",
    );
    (dir1, c1, dir2, c2)
}

#[test]
fn multi_store_search_merges_results_from_both_stores() {
    let (_dir1, config1, _dir2, config2) = two_stores();

    let json = collect_json(
        lore()
            .args(["search", "--config"])
            .arg(&config1)
            .args(["--config"])
            .arg(&config2)
            .arg("--json")
            .arg("animals programming"),
    );
    let items = json["items"].as_array().expect("items should be an array");
    assert_eq!(items.len(), 2, "both stores should contribute a hit");

    let topics: Vec<&str> = items.iter().filter_map(|i| i["topic"].as_str()).collect();
    assert!(
        topics.contains(&"Animals"),
        "results should include Animals topic; got: {topics:?}"
    );
    assert!(
        topics.contains(&"Programming"),
        "results should include Programming topic; got: {topics:?}"
    );
}

#[test]
fn multi_store_basic_aggregation() {
    let (_dir1, config1, _dir2, config2) = two_stores();

    let topics_stdout = collect_stdout(
        lore()
            .args(["topics", "--config"])
            .arg(&config1)
            .args(["--config"])
            .arg(&config2),
    );
    assert!(
        topics_stdout.contains("Animals"),
        "topics should include 'Animals' from store1; got: {topics_stdout}"
    );
    assert!(
        topics_stdout.contains("Programming"),
        "topics should include 'Programming' from store2; got: {topics_stdout}"
    );

    let info_json = collect_json(
        lore()
            .args(["info", "--config"])
            .arg(&config1)
            .args(["--config"])
            .arg(&config2)
            .arg("--json"),
    );
    let documents = info_json["documents"].as_u64().unwrap_or(0);
    assert!(
        documents >= 2,
        "federated info should report at least 2 documents (one per store); got: {documents}"
    );
    let chunks = info_json["chunks"].as_u64().unwrap_or(0);
    assert!(
        chunks >= 2,
        "federated info should report at least 2 chunks (one per store); got: {chunks}"
    );

    let docs_stdout = collect_stdout(
        lore()
            .args(["docs", "--config"])
            .arg(&config1)
            .args(["--config"])
            .arg(&config2)
            .arg("--json"),
    );
    assert!(
        docs_stdout.contains("animals.md"),
        "docs should list animals.md from store1; got: {docs_stdout}"
    );
    assert!(
        docs_stdout.contains("rust.md"),
        "docs should list rust.md from store2; got: {docs_stdout}"
    );
}

#[test]
fn multi_store_shared_topic_aggregates_stats() {
    let (_dir1, config1, _dir2, config2) = two_stores_shared_topic();

    let json = collect_json(
        lore()
            .args(["topics", "--topic", "Wildlife", "--config"])
            .arg(&config1)
            .args(["--config"])
            .arg(&config2)
            .arg("--json"),
    );

    let items = json["items"].as_array().expect("items should be an array");
    assert_eq!(items.len(), 1, "should have exactly one Wildlife topic");

    let doc_count = items[0]["doc_count"].as_u64().unwrap_or(0);
    assert_eq!(
        doc_count, 2,
        "shared topic should aggregate docs from both stores; got: {doc_count}"
    );
}

#[test]
fn multi_store_read_from_second_store() {
    let (_dir1, config1, _dir2, config2) = two_stores();

    let stdout = collect_stdout(
        lore()
            .args(["read", "rust.md", "--config"])
            .arg(&config1)
            .args(["--config"])
            .arg(&config2),
    );
    assert!(
        stdout.to_lowercase().contains("rust"),
        "read should resolve a source in the second store; got: {stdout}"
    );
}
