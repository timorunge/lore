use std::fs;

use predicates::prelude::*;
use tempfile::TempDir;

use crate::helpers::{collect_json, lore, run_ingest};

#[test]
fn completions_bash() {
    lore()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("lore"));
}

#[test]
fn completions_zsh() {
    lore()
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("lore"));
}

#[test]
fn completions_fish() {
    lore()
        .args(["completions", "fish"])
        .assert()
        .success()
        .stdout(predicate::str::contains("lore"));
}

#[test]
fn init_creates_config_file() {
    let dir = TempDir::new().unwrap();

    lore()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    let config_path = dir.path().join(".lore").join("lore.yaml");
    assert!(
        config_path.is_file(),
        "lore init should create .lore/lore.yaml, but it does not exist at {}",
        config_path.display()
    );
}

#[test]
fn init_rejects_existing_config() {
    let dir = TempDir::new().unwrap();

    // First init should succeed
    lore()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    // Second init should fail because config already exists
    lore()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn force_flag_reingests_all_documents() {
    let dir = TempDir::new().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(&docs).unwrap();

    fs::write(
        docs.join("doc.md"),
        "# Force Test\n\nContent for forced re-ingestion test.\n",
    )
    .unwrap();

    let config = format!(
        "name: Force Test\nsources:\n  - path: {}\n    glob: \"**/*.md\"\nstore:\n  path: test_index\n",
        docs.display()
    );

    // Initial ingest
    let config_path = run_ingest(dir.path(), &config);

    let before = collect_json(
        lore()
            .args(["info", "--config"])
            .arg(&config_path)
            .arg("--json"),
    );
    let docs_before = before["documents"].as_u64().unwrap();
    let chunks_before = before["chunks"].as_u64().unwrap();
    assert!(docs_before >= 1, "initial ingest should index the document");

    // Force re-ingest should succeed even though documents are unchanged
    lore()
        .args(["ingest", "--config"])
        .arg(&config_path)
        .arg("--force")
        .assert()
        .success();

    let after = collect_json(
        lore()
            .args(["info", "--config"])
            .arg(&config_path)
            .arg("--json"),
    );
    assert_eq!(
        after["documents"].as_u64().unwrap(),
        docs_before,
        "forced re-ingest should preserve the same document count"
    );
    assert_eq!(
        after["chunks"].as_u64().unwrap(),
        chunks_before,
        "forced re-ingest should preserve the same chunk count"
    );
}

#[test]
fn invalid_config_gives_clear_error() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("bad.yaml");

    // Write a config with invalid YAML structure
    fs::write(&config_path, "name: Bad Config\nsources:\n  - path: [\n").unwrap();

    lore()
        .args(["ingest", "--config"])
        .arg(&config_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("failed to parse config"));
}
