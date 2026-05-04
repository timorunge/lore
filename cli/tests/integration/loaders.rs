use std::fs;
use std::io::Write;

use predicates::prelude::*;
use tempfile::TempDir;

use crate::helpers::*;

fn assert_git_url_rejected(url: &str, expected_stderr: &str) {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("lore.yaml");
    fs::write(
        &config_path,
        format!("name: Git Test\nsources:\n  - git: \"{url}\"\n    glob: \"**/*.md\"\nstore:\n  path: test_index\n"),
    ).unwrap();

    lore()
        .args(["ingest", "--config"])
        .arg(&config_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains(expected_stderr));
}

#[test]
fn local_file_types() {
    let dir = TempDir::new().unwrap();

    // Plain text
    fs::write(
        dir.path().join("notes.txt"),
        "Meeting notes from the architecture review session discussing migration strategy.\n",
    )
    .unwrap();

    // HTML
    fs::write(
        dir.path().join("page.html"),
        "<html><body><h1>HTML Page</h1><p>This is a locally stored HTML page that should be extracted by kreuzberg.</p></body></html>",
    ).unwrap();

    // Single markdown file (not a directory)
    fs::write(
        dir.path().join("standalone.md"),
        "# Standalone\n\nA single file pointed at directly rather than via a directory source.\n",
    )
    .unwrap();

    let config = format!(
        "name: File Types Test\nsources:\n  - path: {0}/notes.txt\n  - path: {0}/page.html\n  - path: {0}/standalone.md\n\
         store:\n  path: test_index\n",
        dir.path().display()
    );

    let config_path = run_ingest(dir.path(), &config);
    assert_store_counts(&config_path, 3, 3);
    assert_search_hit(&config_path, "migration");
    assert_search_hit(&config_path, "HTML");
    assert_search_hit(&config_path, "standalone");
}

#[test]
fn local_glob_filters() {
    let dir = TempDir::new().unwrap();
    let docs = dir.path().join("mixed");
    fs::create_dir_all(&docs).unwrap();

    fs::write(
        docs.join("keep.md"),
        "# Keepable\n\nThis markdown file should be ingested.\n",
    )
    .unwrap();
    fs::write(docs.join("skip.txt"), "Should NOT be ingested.\n").unwrap();
    fs::write(docs.join("skip.json"), r#"{"skip": true}"#).unwrap();

    let config = format!(
        "name: Glob Test\nsources:\n  - path: {}\n    glob: \"**/*.md\"\nstore:\n  path: test_index\n",
        docs.display()
    );

    let config_path = run_ingest(dir.path(), &config);
    assert_store_exact_counts(&config_path, 1, 1);
    assert_docs_listed(&config_path, &["keep.md"]);
}

#[test]
fn local_recursive_walk() {
    let dir = TempDir::new().unwrap();
    let base = dir.path().join("project");
    fs::create_dir_all(base.join("src/components")).unwrap();
    fs::create_dir_all(base.join("docs/guides")).unwrap();

    fs::write(base.join("README.md"), "# Root\n\nTop-level readme.\n").unwrap();
    fs::write(
        base.join("src/components/button.md"),
        "# Button\n\nA reusable button component.\n",
    )
    .unwrap();
    fs::write(
        base.join("docs/guides/quickstart.md"),
        "# Quickstart\n\nGet running in five minutes.\n",
    )
    .unwrap();

    let config = format!(
        "name: Walk Test\nsources:\n  - path: {}\n    glob: \"**/*.md\"\nstore:\n  path: test_index\n",
        base.display()
    );

    let config_path = run_ingest(dir.path(), &config);
    assert_store_counts(&config_path, 3, 2);
    assert_docs_listed(&config_path, &["README.md", "button.md", "quickstart.md"]);
}

#[test]
fn local_multiple_sources_with_topics() {
    let dir = TempDir::new().unwrap();
    let guides = dir.path().join("guides");
    let api = dir.path().join("api");
    fs::create_dir_all(&guides).unwrap();
    fs::create_dir_all(&api).unwrap();

    fs::write(
        guides.join("setup.md"),
        "# Setup\n\nHow to configure the dev environment.\n",
    )
    .unwrap();
    fs::write(
        api.join("endpoints.md"),
        "# Endpoints\n\nREST API docs including auth.\n",
    )
    .unwrap();

    let config = format!(
        "name: Topics Test\nsources:\n  - path: {}\n    glob: \"**/*.md\"\n    topic: Guides\n  \
         - path: {}\n    glob: \"**/*.md\"\n    topic: API\nstore:\n  path: test_index\n",
        guides.display(),
        api.display()
    );

    let config_path = run_ingest(dir.path(), &config);
    assert_store_counts(&config_path, 2, 2);
    assert_topics_listed(&config_path, &["Guides", "API"]);
}

#[test]
fn local_archive_formats() {
    let dir = TempDir::new().unwrap();

    // --- tar.gz ---
    let tar_path = dir.path().join("docs.tar.gz");
    {
        let file = fs::File::create(&tar_path).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut tar = tar::Builder::new(enc);

        for (path, body) in [
            (
                "docs/one.md",
                &b"# Doc One\n\nFirst archived document with enough content.\n"[..],
            ),
            (
                "docs/two.md",
                &b"# Doc Two\n\nSecond archived document on a different topic.\n"[..],
            ),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_path(path).unwrap();
            h.set_size(body.len() as u64);
            h.set_cksum();
            tar.append(&h, body).unwrap();
        }
        tar.finish().unwrap();
    }

    let tar_dir = TempDir::new().unwrap();
    let tar_config = format!(
        "name: Tar Test\nsources:\n  - path: {}\n    glob: \"**/*.md\"\nstore:\n  path: test_index\n",
        tar_path.display()
    );
    let config_path = run_ingest(tar_dir.path(), &tar_config);
    assert_store_counts(&config_path, 2, 2);
    assert_search_hit(&config_path, "archived");

    // --- zip ---
    let zip_path = dir.path().join("bundle.zip");
    {
        let file = fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        for (path, body) in [
            (
                "readme.md",
                &b"# Zipped Readme\n\nLives inside a zip archive.\n"[..],
            ),
            (
                "notes/design.md",
                &b"# Design Notes\n\nArchitectural decisions and trade-offs.\n"[..],
            ),
        ] {
            zip.start_file(path, opts).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
    }

    let zip_dir = TempDir::new().unwrap();
    let zip_config = format!(
        "name: Zip Test\nsources:\n  - path: {}\n    glob: \"**/*.md\"\nstore:\n  path: test_index\n",
        zip_path.display()
    );
    let config_path = run_ingest(zip_dir.path(), &zip_config);
    assert_store_counts(&config_path, 2, 2);
    assert_search_hit(&config_path, "zipped");
}

#[test]
fn git_input_rejected() {
    assert_git_url_rejected("ext::sh -c evil%", "ext::");
    // Leading '-' is caught as option injection before the format check.
    assert_git_url_rejected("--upload-pack=evil", "option injection");
}

#[test]
fn recreate_rebuilds_from_scratch() {
    let dir = TempDir::new().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(&docs).unwrap();

    fs::write(
        docs.join("original.md"),
        "# Original\n\nWill be replaced after recreate.\n",
    )
    .unwrap();

    let config = format!(
        "name: Recreate Test\nsources:\n  - path: {}\n    glob: \"**/*.md\"\nstore:\n  path: test_index\n",
        docs.display()
    );
    let config_path = run_ingest(dir.path(), &config);

    fs::remove_file(docs.join("original.md")).unwrap();
    fs::write(
        docs.join("replacement.md"),
        "# Replacement\n\nOnly doc after recreate.\n",
    )
    .unwrap();

    lore()
        .args(["ingest", "--config"])
        .arg(&config_path)
        .arg("--recreate")
        .assert()
        .success();

    assert_store_counts(&config_path, 1, 1);
    assert_docs_listed(&config_path, &["replacement.md"]);
}

#[test]
fn drop_sections_removes_content() {
    let dir = TempDir::new().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(&docs).unwrap();

    // Use top-level headings so kreuzberg produces separate chunks per section.
    // drop_sections filters by leaf heading -- sections must be separate chunks.
    let filler = "This sentence adds bulk so the section exceeds the chunk limit. ";
    let main_body = filler.repeat(6);
    let ref_body = "Xylophone zeppelin quaternion. ".repeat(6);
    let more_body = filler.repeat(6);

    let doc = format!(
        "# Main Content\n\n{main_body}\n\n\
         # References\n\n{ref_body}\n\n\
         # More Content\n\n{more_body}\n"
    );
    fs::write(docs.join("doc.md"), &doc).unwrap();

    let config = format!(
        "name: Drop Test\nsources:\n  - path: {}\n    glob: \"**/*.md\"\nstore:\n  path: test_index\n\
         processing:\n  max_chunk_chars: 400\n  min_chunk_chars: 20\n  drop_sections:\n    - References\n",
        docs.display()
    );

    let config_path = run_ingest(dir.path(), &config);
    assert_store_counts(&config_path, 1, 2);
    assert_search_hit(&config_path, "sentence bulk");

    // Unique phrase from dropped section must return zero results
    let json = collect_json(
        lore()
            .args(["search", "--config"])
            .arg(&config_path)
            .arg("--json")
            .arg("xylophone zeppelin quaternion"),
    );
    assert_eq!(
        json["total"].as_u64().unwrap(),
        0,
        "dropped section leaked into search results"
    );
}

#[test]
fn custom_text_extensions() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("docs")).unwrap();

    fs::write(
        dir.path().join("docs/page.mdx"),
        "# MDX Page\n\nTreated as plain text via text_extensions config.\n",
    )
    .unwrap();

    let config = format!(
        "name: Ext Test\nsources:\n  - path: {}\nstore:\n  path: test_index\n\
         processing:\n  text_extensions:\n    - mdx\n",
        dir.path().join("docs").display()
    );

    let config_path = run_ingest(dir.path(), &config);
    assert_store_counts(&config_path, 1, 1);
    assert_search_hit(&config_path, "MDX");
}

#[test]
fn per_source_preset_changes_chunk_size() {
    let dir = TempDir::new().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(&docs).unwrap();

    // Write a doc with enough content to produce multiple chunks at 200 chars
    // but only 1 chunk at a large limit
    let long_content = "# Section One\n\n".to_owned()
        + &"This is a long paragraph about software architecture. ".repeat(10)
        + "\n\n# Section Two\n\n"
        + &"This paragraph discusses testing strategies and patterns. ".repeat(10);

    fs::write(docs.join("long.md"), &long_content).unwrap();

    // Use a very small chunk size preset
    let config = format!(
        "name: Preset Test\nsources:\n  - path: {}\n    processing: small\nstore:\n  path: test_index\n\
         processing:\n  max_chunk_chars: 10000\n  presets:\n    small:\n      max_chunk_chars: 200\n      min_chunk_chars: 10\n",
        docs.display()
    );

    let config_path = run_ingest(dir.path(), &config);
    // With 200-char chunks, we should get more than 1 chunk
    assert_store_counts(&config_path, 1, 2);
}

#[test]
fn deleted_files_removed_from_index() {
    let dir = TempDir::new().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(&docs).unwrap();

    fs::write(
        docs.join("keep.md"),
        "# Keep\n\nThis document should survive re-ingest.\n",
    )
    .unwrap();
    fs::write(
        docs.join("delete-me.md"),
        "# Delete Me\n\nThis document will be deleted before the second ingest.\n",
    )
    .unwrap();

    let config = format!(
        "name: Deletion Test\nsources:\n  - path: {}\n    glob: \"**/*.md\"\nstore:\n  path: test_index\n",
        docs.display()
    );

    // Step 1: ingest 2 files, assert 2 docs.
    let config_path = run_ingest(dir.path(), &config);
    assert_store_counts(&config_path, 2, 2);
    assert_search_hit(&config_path, "survive");
    assert_search_hit(&config_path, "deleted before");

    // Step 2: delete 1 file, re-ingest, assert 1 doc remains.
    fs::remove_file(docs.join("delete-me.md")).unwrap();
    run_ingest(dir.path(), &config);
    assert_store_counts(&config_path, 1, 1);
    assert_search_hit(&config_path, "survive");

    // Step 3: delete the last file, re-ingest, assert 0 docs.
    fs::remove_file(docs.join("keep.md")).unwrap();
    run_ingest(dir.path(), &config);
    let json = collect_json(
        lore()
            .args(["info", "--config"])
            .arg(&config_path)
            .arg("--json"),
    );
    assert_eq!(
        json["documents"].as_u64().unwrap(),
        0,
        "index should be empty after all files deleted"
    );
}

#[test]
fn archive_entries_stable_across_reingests() {
    let dir = TempDir::new().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(&docs).unwrap();

    // Create a zip archive with two files.
    let archive_path = docs.join("bundle.zip");
    {
        let file = fs::File::create(&archive_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        zip.start_file("notes/alpha.md", opts).unwrap();
        zip.write_all(b"# Alpha\n\nStable archive entry alpha content.\n")
            .unwrap();
        zip.start_file("notes/beta.md", opts).unwrap();
        zip.write_all(b"# Beta\n\nStable archive entry beta content.\n")
            .unwrap();
        zip.finish().unwrap();
    }

    let config = format!(
        "name: Archive Stability\nsources:\n  - path: {}\nstore:\n  path: test_index\n",
        docs.display()
    );

    // First ingest -- entries should appear.
    let config_path = run_ingest(dir.path(), &config);
    assert_store_counts(&config_path, 2, 2);
    assert_search_hit(&config_path, "alpha");
    assert_search_hit(&config_path, "beta");

    // Second ingest (incremental) -- entries must remain, not be deleted.
    run_ingest(dir.path(), &config);
    assert_store_counts(&config_path, 2, 2);
    assert_search_hit(&config_path, "alpha");
    assert_search_hit(&config_path, "beta");

    // Source paths must reference the archive, not the cache directory.
    let stdout = collect_stdout(
        lore()
            .args(["docs", "--config"])
            .arg(&config_path)
            .arg("--json"),
    );
    assert!(
        stdout.contains("bundle.zip#"),
        "source path should reference archive: {stdout}"
    );
    assert!(
        !stdout.contains("/Caches/") && !stdout.contains("/archives/"),
        "source path should not contain cache path: {stdout}"
    );
}
