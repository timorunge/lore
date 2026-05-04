use predicates::prelude::*;
use tempfile::TempDir;

use crate::helpers::*;

#[test]
fn maintain_check_healthy_store() {
    let (_dir, config_path) = ingest_fixture();

    let json = collect_json(
        lore()
            .args(["maintain", "check", "--config"])
            .arg(&config_path)
            .arg("--json"),
    );
    assert_eq!(json["issues"].as_array().unwrap().len(), 0);
    assert_eq!(json["fixed"].as_u64().unwrap(), 0);
}

#[test]
fn maintain_check_missing_store() {
    let dir = TempDir::new().unwrap();
    let bad_config = dir.path().join("bad.yaml");
    std::fs::write(
        &bad_config,
        "sources: []\nstore:\n  path: nonexistent_index\n",
    )
    .unwrap();

    lore()
        .args(["maintain", "check", "--config"])
        .arg(&bad_config)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no store found"));
}

#[test]
fn maintain_repair_orphaned_chunks() {
    let (dir, config_path) = ingest_fixture();
    let store = dir.path().join("test_index");

    let mut docs_json = lore::store_test_support::read_store_docs_json(&store);
    let docs = docs_json.as_array_mut().expect("documents array");
    assert!(!docs.is_empty(), "at least one document");
    docs.remove(0);
    lore::store_test_support::write_store_docs_json(&store, docs_json);

    // Check: should detect orphaned chunks
    let report = collect_json(
        lore()
            .args(["maintain", "check", "--config"])
            .arg(&config_path)
            .arg("--json"),
    );
    let issues = report["issues"].as_array().expect("issues array");
    assert!(
        !issues.is_empty(),
        "check should detect orphaned chunks before repair"
    );
    let has_orphan = issues
        .iter()
        .any(|i| i["kind"].as_str() == Some("orphaned_chunks"));
    assert!(has_orphan, "should have at least one orphaned_chunks issue");

    // Repair: should clean up orphaned chunks
    let report_fix = collect_json(
        lore()
            .args(["maintain", "repair", "--config"])
            .arg(&config_path)
            .arg("--json"),
    );
    let fixed = report_fix["fixed"].as_u64().expect("fixed count");
    assert!(fixed > 0, "repair should report at least one fix");

    // Check again: should be clean now
    let report_clean = collect_json(
        lore()
            .args(["maintain", "check", "--config"])
            .arg(&config_path)
            .arg("--json"),
    );
    assert_eq!(
        report_clean["issues"].as_array().unwrap().len(),
        0,
        "store should be clean after repair"
    );
    assert_eq!(
        report_clean["fixed"].as_u64().unwrap(),
        0,
        "no additional fixes needed"
    );
}
