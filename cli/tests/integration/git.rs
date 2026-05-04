use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use tempfile::TempDir;

use crate::helpers::*;

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok()
}

fn create_bare_repo_with_files(dir: &Path, files: &[(&str, &str)]) -> PathBuf {
    let bare = dir.join("test.git");
    StdCommand::new("git")
        .args(["init", "--bare"])
        .arg(&bare)
        .output()
        .expect("git init --bare");

    let work = dir.join("work");
    StdCommand::new("git")
        .args(["clone"])
        .arg(&bare)
        .arg(&work)
        .output()
        .expect("git clone");

    StdCommand::new("git")
        .args(["-C"])
        .arg(&work)
        .args(["config", "user.email", "test@test.com"])
        .output()
        .expect("git config email");

    StdCommand::new("git")
        .args(["-C"])
        .arg(&work)
        .args(["config", "user.name", "Test"])
        .output()
        .expect("git config name");

    for (name, content) in files {
        let file_path = work.join(name);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&file_path, content).unwrap();
    }

    StdCommand::new("git")
        .args(["-C"])
        .arg(&work)
        .args(["add", "."])
        .output()
        .expect("git add");

    StdCommand::new("git")
        .args(["-C"])
        .arg(&work)
        .args(["commit", "-m", "initial"])
        .output()
        .expect("git commit");

    StdCommand::new("git")
        .args(["-C"])
        .arg(&work)
        .args(["push"])
        .output()
        .expect("git push");

    bare
}

#[test]
fn git_local_repo_ingested() {
    if !git_available() {
        eprintln!("skipping: git not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let bare = create_bare_repo_with_files(
        dir.path(),
        &[
            (
                "readme.md",
                "# Project Readme\n\nThis project provides a modular ingestion pipeline.\n",
            ),
            (
                "docs/guide.md",
                "# User Guide\n\nFollow these steps to configure the system.\n",
            ),
        ],
    );

    let config_path = dir.path().join("lore.yaml");
    let config = format!(
        "name: git-test\nsources:\n  - git: {}\n    glob: \"**/*.md\"\n    topic: Test\n\
         store:\n  path: test_index\n\
         processing:\n  max_chunk_chars: 800\n  min_chunk_chars: 20\n",
        bare.display()
    );
    std::fs::write(&config_path, config).unwrap();

    lore()
        .args(["ingest", "--config"])
        .arg(&config_path)
        .assert()
        .success();

    assert_store_counts(&config_path, 2, 2);
    assert_search_hit(&config_path, "ingestion pipeline");
    assert_search_hit(&config_path, "configure");
}

#[test]
fn git_glob_filter() {
    if !git_available() {
        eprintln!("skipping: git not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let bare = create_bare_repo_with_files(
        dir.path(),
        &[
            (
                "README.md",
                "# Overview\n\nThis document describes the project overview.\n",
            ),
            (
                "notes/design.md",
                "# Design Notes\n\nArchitectural decisions and trade-offs documented here.\n",
            ),
            (
                "scratch.txt",
                "This plain text file should not be ingested by the glob filter.\n",
            ),
        ],
    );

    let config_path = dir.path().join("lore.yaml");
    let config = format!(
        "name: git-glob-test\nsources:\n  - git: {}\n    glob: \"**/*.md\"\n    topic: Test\n\
         store:\n  path: test_index\n\
         processing:\n  max_chunk_chars: 800\n  min_chunk_chars: 20\n",
        bare.display()
    );
    std::fs::write(&config_path, config).unwrap();

    lore()
        .args(["ingest", "--config"])
        .arg(&config_path)
        .assert()
        .success();

    assert_store_counts(&config_path, 2, 2);
    assert_search_hit(&config_path, "architectural decisions");
}
