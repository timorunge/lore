use std::fs;

use predicates::prelude::*;
use tempfile::TempDir;

use crate::helpers::*;

#[test]
fn ingest_dry_run() {
    let dir = TempDir::new().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(&docs).unwrap();

    fs::write(
        docs.join("test.md"),
        "# Test\n\nContent for dry run test.\n",
    )
    .unwrap();

    let config_path = dir.path().join("lore.yaml");
    let config = format!(
        "name: Dry Run Test\nsources:\n  - path: {}\n    glob: \"**/*.md\"\nstore:\n  path: test_index\n",
        docs.display()
    );
    fs::write(&config_path, config).unwrap();

    lore()
        .args(["ingest", "--config"])
        .arg(&config_path)
        .arg("--dry-run")
        .assert()
        .success()
        .stderr(predicate::str::contains("dry run"));

    // Dry run should not create any indexed documents
    let json = collect_json(
        lore()
            .args(["info", "--config"])
            .arg(&config_path)
            .arg("--json"),
    );
    assert_eq!(
        json["documents"].as_u64().unwrap(),
        0,
        "dry run should index 0 documents"
    );
    assert_eq!(
        json["chunks"].as_u64().unwrap(),
        0,
        "dry run should index 0 chunks"
    );
}
