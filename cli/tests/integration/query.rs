use predicates::prelude::*;
use tempfile::TempDir;

use crate::helpers::*;

#[test]
fn search_json_fields() {
    let (_dir, config_path) = ingest_fixture();

    let json = collect_json(
        lore()
            .args(["search", "--config"])
            .arg(&config_path)
            .arg("--json")
            .arg("architecture"),
    );
    let results = json["items"].as_array().expect("items array");
    assert!(!results.is_empty(), "expected at least one search result");

    for result in results {
        let chunk_id = result["chunk_id"]
            .as_str()
            .expect("chunk_id should be a string");
        assert_eq!(chunk_id.len(), 32, "chunk_id should be 32 hex chars");

        let chunk_score = result["score"].as_f64().expect("score should be a number");
        assert!(chunk_score > 0.0, "BM25 score should be positive");
    }

    let has_snippet = results.iter().any(|r| !r["snippet"].is_null());
    assert!(
        has_snippet,
        "at least one search result should include a snippet"
    );
}

#[test]
fn search_with_topic_filter() {
    let (_dir, config_path) = ingest_fixture();

    let assert = lore()
        .args(["search", "--config"])
        .arg(&config_path)
        .arg("architecture")
        .args(["--topic", "Documentation"])
        .arg("--json")
        .assert()
        .success()
        .stderr(predicate::str::contains("hint: narrow with").not());

    let out = assert.get_output().stdout.clone();
    let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let results = json["items"].as_array().unwrap();
    assert!(!results.is_empty());
    for r in results {
        assert_eq!(r["topic"].as_str().unwrap(), "Documentation");
    }
}

#[test]
fn search_max_per_source() {
    let (_dir, config_path) = ingest_fixture();

    let json = collect_json(
        lore()
            .args(["search", "--config"])
            .arg(&config_path)
            .args(["--max-per-source", "1"])
            .arg("--json")
            .arg("project"),
    );
    let results = json["items"].as_array().unwrap();

    let mut per_source: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for r in results {
        let source = r["source_id"].as_str().unwrap();
        *per_source.entry(source).or_default() += 1;
    }
    for (source, count) in &per_source {
        assert!(
            *count <= 1,
            "source {source} has {count} results, expected <= 1 with max_per_source=1"
        );
    }
}

#[test]
fn info_json_completeness() {
    let (_dir, config_path) = ingest_fixture();

    let json = collect_json(
        lore()
            .args(["info", "--config"])
            .arg(&config_path)
            .arg("--json"),
    );

    assert!(json["documents"].as_u64().unwrap() > 0);
    assert!(json["chunks"].as_u64().unwrap() > 0);
    assert!(json["words"].as_u64().unwrap() > 0);

    let source_types = json["source_types"]
        .as_object()
        .expect("source_types object");
    assert!(source_types.contains_key("local"));

    let langs = json["languages"]
        .as_object()
        .expect("languages should be an object");
    assert!(
        langs.contains_key("en"),
        "languages should contain 'en' from frontmatter fixture, got: {langs:?}"
    );

    let avg = json["avg_words_per_chunk"]
        .as_f64()
        .expect("avg_words_per_chunk should be a number");
    assert!(
        avg > 0.0,
        "avg_words_per_chunk should be positive, got: {avg}"
    );
}

#[test]
fn read_command() {
    let (_dir, config_path) = ingest_fixture();

    let json = collect_json(
        lore()
            .args(["read", "--config"])
            .arg(&config_path)
            .arg("architecture.md")
            .args(["--limit", "1"])
            .arg("--json"),
    );
    assert_eq!(json["chunks"].as_array().unwrap().len(), 1);

    // Partial path resolution
    lore()
        .args(["read", "--config"])
        .arg(&config_path)
        .arg("architecture.md")
        .assert()
        .success()
        .stdout(predicate::str::contains("modular"));

    // Not found
    lore()
        .args(["read", "--config"])
        .arg(&config_path)
        .arg("nonexistent.md")
        .assert()
        .failure()
        .stderr(predicate::str::contains("document not found"));
}

#[test]
fn docs_filters() {
    let (_dir, config_path) = ingest_fixture();

    // By title
    let json = collect_json(
        lore()
            .args(["docs", "--config"])
            .arg(&config_path)
            .args(["--title", "Getting Started"])
            .arg("--json"),
    );
    assert_eq!(json["items"].as_array().unwrap().len(), 1);

    // By author
    let json = collect_json(
        lore()
            .args(["docs", "--config"])
            .arg(&config_path)
            .args(["--author", "Test Author"])
            .arg("--json"),
    );
    assert_eq!(json["items"].as_array().unwrap().len(), 1);
}

#[test]
fn topics_json_format() {
    let (_dir, config_path) = ingest_fixture();

    let json = collect_json(
        lore()
            .args(["topics", "--config"])
            .arg(&config_path)
            .arg("--json"),
    );
    let topics = json["items"].as_array().expect("items array");
    assert!(!topics.is_empty());
    assert_eq!(
        topics[0]["name"].as_str(),
        Some("Documentation"),
        "first topic name should be 'Documentation'"
    );
    assert!(
        topics[0]["chunk_count"].as_u64().unwrap() > 0,
        "first topic chunk_count should be > 0"
    );
}

#[test]
fn search_pagination() {
    let (_dir, config_path) = ingest_fixture();

    let json0 = collect_json(
        lore()
            .args(["search", "--config"])
            .arg(&config_path)
            .args(["--offset", "0", "--limit", "1"])
            .arg("--json")
            .arg("project"),
    );
    let results0 = json0["items"].as_array().expect("items array page 0");
    assert_eq!(results0.len(), 1, "limit=1 should return exactly 1 result");

    let json1 = collect_json(
        lore()
            .args(["search", "--config"])
            .arg(&config_path)
            .args(["--offset", "1", "--limit", "1"])
            .arg("--json")
            .arg("project"),
    );
    let results1 = json1["items"].as_array().expect("items array page 1");
    assert_eq!(
        results1.len(),
        1,
        "limit=1 offset=1 should return exactly 1 result"
    );

    // The two results should be different chunks
    let chunk_id0 = results0[0]["chunk_id"].as_str().expect("chunk_id page 0");
    let chunk_id1 = results1[0]["chunk_id"].as_str().expect("chunk_id page 1");
    assert_ne!(
        chunk_id0, chunk_id1,
        "paginated results should return different chunks"
    );

    // The rank field should reflect the offset
    let rank0 = results0[0]["rank"].as_i64().unwrap_or(-1);
    let rank1 = results1[0]["rank"].as_i64().unwrap_or(-1);
    assert_eq!(rank0, 0, "first result rank should be 0");
    assert_eq!(rank1, 1, "second page result rank should be 1");
}

#[test]
fn search_bm25_stemming() {
    let (_dir, config_path) = ingest_fixture();

    let json = collect_json(
        lore()
            .args(["search", "--config"])
            .arg(&config_path)
            .arg("--json")
            .arg("installed"),
    );
    let results = json["items"].as_array().expect("items array");
    assert!(
        !results.is_empty(),
        "stemming: 'installed' should match docs containing 'install'"
    );

    let json = collect_json(
        lore()
            .args(["search", "--config"])
            .arg(&config_path)
            .arg("--json")
            .arg("install"),
    );
    let results = json["items"].as_array().expect("items array");
    assert!(
        !results.is_empty(),
        "stemming: 'install' should match docs containing 'installation'"
    );
}

#[test]
fn query_commands_fail_on_missing_store() {
    let dir = TempDir::new().unwrap();
    let bad_config = dir.path().join("bad.yaml");
    std::fs::write(&bad_config, "sources: []\nstore:\n  path: nonexistent\n").unwrap();

    lore()
        .args(["search", "--config"])
        .arg(&bad_config)
        .arg("anything")
        .assert()
        .failure()
        .stderr(predicate::str::contains("store not found"));

    lore()
        .args(["info", "--config"])
        .arg(&bad_config)
        .assert()
        .failure()
        .stderr(predicate::str::contains("store not found"));

    lore()
        .args(["docs", "--config"])
        .arg(&bad_config)
        .assert()
        .failure()
        .stderr(predicate::str::contains("store not found"));

    lore()
        .args(["topics", "--config"])
        .arg(&bad_config)
        .assert()
        .failure()
        .stderr(predicate::str::contains("store not found"));

    lore()
        .args(["read", "--config"])
        .arg(&bad_config)
        .arg("anything.md")
        .assert()
        .failure()
        .stderr(predicate::str::contains("store not found"));
}

#[test]
fn search_filter_cases() {
    let (_dir, config_path) = ingest_fixture();

    // (flag, value, query, field, expected_value)
    let cases: &[(&str, &str, &str, &str, &str)] = &[
        (
            "--source",
            "architecture",
            "system",
            "source",
            "architecture",
        ),
        ("--lang", "en", "started", "lang", "en"),
    ];

    for (flag, value, query, field, expected) in cases {
        let json = collect_json(
            lore()
                .args(["search", "--config"])
                .arg(&config_path)
                .args([flag, value])
                .arg("--json")
                .arg(query),
        );
        let results = json["items"].as_array().unwrap();
        assert!(
            !results.is_empty(),
            "{flag}={value}: expected at least one result for query '{query}'"
        );
        for r in results {
            let actual = r[*field].as_str().unwrap();
            assert!(
                actual.contains(expected),
                "{flag}={value}: {field} should contain '{expected}', got: {actual}"
            );
        }
    }
}

#[test]
fn search_sort_and_reverse() {
    let (_dir, config_path) = ingest_fixture();

    let json_asc = collect_json(
        lore()
            .args(["search", "--config"])
            .arg(&config_path)
            .args(["--sort", "source"])
            .arg("--json")
            .arg("project"),
    );
    let asc = json_asc["items"].as_array().unwrap();
    assert!(asc.len() >= 2, "need >= 2 results to test sort");
    for w in asc.windows(2) {
        let s0 = w[0]["source"].as_str().unwrap();
        let s1 = w[1]["source"].as_str().unwrap();
        assert!(s0 <= s1, "expected ascending source order: {s0} > {s1}");
    }

    let json_desc = collect_json(
        lore()
            .args(["search", "--config"])
            .arg(&config_path)
            .args(["--sort", "source", "--reverse"])
            .arg("--json")
            .arg("project"),
    );
    let desc = json_desc["items"].as_array().unwrap();
    for w in desc.windows(2) {
        let s0 = w[0]["source"].as_str().unwrap();
        let s1 = w[1]["source"].as_str().unwrap();
        assert!(s0 >= s1, "expected descending source order: {s0} < {s1}");
    }
}

#[test]
fn diff_detects_added_modified_deleted() {
    let dir = TempDir::new().unwrap();
    let docs = dir.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();

    std::fs::write(docs.join("keep.md"), "# Keep\n\nThis file stays.\n").unwrap();
    std::fs::write(docs.join("change.md"), "# Change\n\nOriginal content.\n").unwrap();
    std::fs::write(
        docs.join("remove.md"),
        "# Remove\n\nThis will be deleted.\n",
    )
    .unwrap();

    let config = format!(
        "sources:\n  - path: {}\n    glob: \"**/*.md\"\nstore:\n  path: test_index\n",
        docs.display()
    );
    let config_path = run_ingest(dir.path(), &config);

    // diff on clean state should show no changes
    let json = collect_json(
        lore()
            .args(["status", "--config"])
            .arg(&config_path)
            .arg("--json"),
    );
    let entries = json["local"]
        .as_array()
        .expect("diff --json returns local array");
    assert!(
        entries.is_empty(),
        "clean diff should have zero entries, got: {entries:?}"
    );

    // Modify a file, delete another, add a new one
    std::fs::write(docs.join("change.md"), "# Change\n\nModified content.\n").unwrap();
    std::fs::remove_file(docs.join("remove.md")).unwrap();
    std::fs::write(docs.join("new.md"), "# New\n\nBrand new file.\n").unwrap();

    let json = collect_json(
        lore()
            .args(["status", "--config"])
            .arg(&config_path)
            .arg("--json"),
    );
    let entries = json["local"]
        .as_array()
        .expect("diff --json returns local array");
    assert!(!entries.is_empty(), "diff should detect changes");

    let statuses: Vec<(&str, &str)> = entries
        .iter()
        .map(|e| (e["source"].as_str().unwrap(), e["status"].as_str().unwrap()))
        .collect();

    assert!(
        statuses
            .iter()
            .any(|(s, st)| s.contains("new.md") && *st == "added"),
        "should detect new.md as added: {statuses:?}"
    );
    assert!(
        statuses
            .iter()
            .any(|(s, st)| s.contains("remove.md") && *st == "deleted"),
        "should detect remove.md as deleted: {statuses:?}"
    );
    assert!(
        statuses
            .iter()
            .any(|(s, st)| s.contains("change.md") && *st == "changed"),
        "should detect change.md as changed: {statuses:?}"
    );
}
