use std::path::Path;

use anyhow::Result;

use lore::ingest::loaders::archive::{ARCHIVE_EXTENSIONS, COMPOUND_SUFFIXES};
use lore::ingest::loaders::file::{CODE_EXTENSIONS, DATA_EXTENSIONS, EMAIL_EXTENSIONS};

use crate::inject;

pub fn inject_all(repo_root: &Path) -> Result<()> {
    println!("formats:");

    inject::inject(
        &repo_root.join("docs/formats.md"),
        &[
            ("format-code-extensions", &render_code()),
            ("format-data-extensions", &render_data()),
            ("format-email-extensions", &render_email()),
            ("format-archive-extensions", &render_archives()),
        ],
    )?;

    Ok(())
}

fn join_extensions(exts: &[&str]) -> String {
    exts.iter()
        .map(|e| format!("`{e}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_code() -> String {
    join_extensions(CODE_EXTENSIONS)
}

fn render_data() -> String {
    join_extensions(DATA_EXTENSIONS)
}

fn render_email() -> String {
    join_extensions(EMAIL_EXTENSIONS)
}

fn render_archives() -> String {
    let singles: Vec<&str> = ARCHIVE_EXTENSIONS
        .iter()
        .filter(|e| **e != "tgz" && **e != "zstd")
        .copied()
        .collect();

    let compounds: Vec<String> = COMPOUND_SUFFIXES
        .iter()
        .map(|s| s.trim_start_matches('.').to_owned())
        .collect();

    let mut all: Vec<String> = singles.iter().map(|e| format!("`{e}`")).collect();
    all.extend(compounds.iter().map(|e| format!("`{e}`")));
    all.join(", ")
}
