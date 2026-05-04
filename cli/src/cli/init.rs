use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use lore::fmt::plural;
use lore::{w, wln};

use crate::cli::DOC_EXTENSIONS;

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    ".node_modules",
    "target",
    "build",
    "dist",
    "out",
    "_build",
    ".venv",
    "venv",
    "__pycache__",
    ".tox",
    ".lore",
    ".cache",
    ".tmp",
    "vendor",
    "third_party",
];

const ROOT_DOC_FILES: &[&str] = &[
    "README.md",
    "readme.md",
    "README.rst",
    "README.txt",
    "README.adoc",
    "CHANGELOG.md",
    "CHANGES.md",
    "HISTORY.md",
    "CONTRIBUTING.md",
    "CONTRIBUTING.rst",
];

const MAX_SCAN_DEPTH: usize = 5;
const MIN_DOCS_PER_DIR: usize = 2;

type ExtCounts = HashMap<String, usize>;

/// Create the global user config at the platform-specific config directory.
pub fn init_global() -> Result<()> {
    let config_path = lore::config::global_config_path()
        .context("cannot determine global config directory on this platform")?;

    if config_path.exists() {
        anyhow::bail!(
            "{} already exists -- edit it directly or remove it to re-initialize",
            config_path.display()
        );
    }

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).context("failed to create global config directory")?;
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&config_path)
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    file.write_all(lore::config::global_config_scaffold().as_bytes())
        .with_context(|| format!("failed to write {}", config_path.display()))?;

    let paint = crate::terminal::stderr_painter();
    eprintln!("[{} ] created {}", paint.green("+"), config_path.display());
    eprintln!(
        "[{} ] edit this file to set user-level defaults for all projects",
        paint.blue("i"),
    );
    Ok(())
}

/// Create a `.lore/lore.yaml` config by scanning the project for document sources.
pub fn init(config_override: Option<PathBuf>) -> Result<()> {
    let config_path = config_override.unwrap_or_else(|| PathBuf::from(".lore/lore.yaml"));

    if config_path.exists() {
        anyhow::bail!(
            "{} already exists -- edit it directly or remove it to re-initialize",
            config_path.display()
        );
    }

    if let Some(parent) = config_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).context("failed to create config directory")?;
    }

    let paint = crate::terminal::stderr_painter();
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let is_git_repo = root.join(".git").is_dir();
    let project_name = root.file_name().map_or_else(
        || "my-knowledge-base".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );

    let sources = scan_for_sources(&root);
    let config = build_config(&config_path, &project_name, &sources, is_git_repo)?;

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&config_path)
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    file.write_all(config.as_bytes())
        .with_context(|| format!("failed to write {}", config_path.display()))?;

    eprintln!("[{} ] created {}", paint.green("+"), config_path.display());
    if sources.is_empty() {
        eprintln!("[{} ] no documents detected", paint.blue("i"));
        eprintln!(
            "[{} ] edit {}, then run `lore ingest`",
            paint.blue("i"),
            config_path.display()
        );
    } else {
        eprintln!(
            "[{} ] detected {} source{}",
            paint.green("+"),
            sources.len(),
            plural(sources.len()),
        );
        eprintln!(
            "[{} ] review {}, then run `lore ingest`",
            paint.blue("i"),
            config_path.display()
        );
    }

    write_gitignore(&config_path, &root, is_git_repo, paint);
    Ok(())
}

/// Assemble the YAML config string from detected sources.
fn build_config(
    config_path: &Path,
    project_name: &str,
    sources: &[String],
    is_git_repo: bool,
) -> Result<String> {
    let safe_name = project_name
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "")
        .replace('\n', " ");

    let mut out = String::new();
    writeln!(out, "name: \"{safe_name}\"")?;
    if is_subdir_config(config_path) {
        writeln!(out)?;
        writeln!(out, "base_dir: ..")?;
    }
    writeln!(out)?;
    writeln!(out, "sources:")?;

    if sources.is_empty() {
        write!(
            out,
            "  # No documents detected. Add your sources here:\n  \
             # - path: ./docs\n  \
             #   glob: \"**/*.md\"\n"
        )?;
    } else {
        writeln!(out, "{}", sources.join("\n"))?;
    }

    if is_git_repo {
        append_git_remote(&mut out)?;
    }

    Ok(out)
}

/// Append a commented-out git remote suggestion if origin is configured.
fn append_git_remote(out: &mut String) -> Result<()> {
    // Control character stripping is defense-in-depth against YAML injection.
    if let Ok(output) = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
    {
        let url = String::from_utf8_lossy(&output.stdout)
            .trim()
            .replace(|c: char| c.is_control(), "");
        if !url.is_empty() {
            writeln!(out)?;
            writeln!(out, "  # Git remote detected:")?;
            writeln!(out, "  # - git: {url}")?;
            writeln!(out, "  #   glob: \"**/*.md\"")?;
        }
    }
    Ok(())
}

/// Create `.lore/.gitignore` and hint about root `.gitignore` if needed.
fn write_gitignore(
    config_path: &Path,
    root: &Path,
    is_git_repo: bool,
    paint: lore::fmt::style::Painter,
) {
    if is_subdir_config(config_path) {
        let lore_gitignore = config_path
            .parent()
            .expect("config_path has a parent because is_subdir_config is true")
            .join(".gitignore");
        if !lore_gitignore.exists() {
            match std::fs::write(&lore_gitignore, "store/\n") {
                Ok(()) => eprintln!(
                    "[{} ] created {}",
                    paint.green("+"),
                    lore_gitignore.display()
                ),
                Err(e) => eprintln!(
                    "[{} ] could not write {}: {e}",
                    paint.yellow("-"),
                    lore_gitignore.display()
                ),
            }
        }
    }

    if is_git_repo && !is_subdir_config(config_path) {
        let gitignore = root.join(".gitignore");
        let already_ignored = gitignore.is_file()
            && std::fs::read_to_string(&gitignore)
                .unwrap_or_default()
                .lines()
                .any(|l| {
                    let t = l.trim();
                    t == ".lore/store" || t == ".lore/store/" || t == ".lore/"
                });
        if !already_ignored {
            eprintln!("[{} ] add .lore/store to your .gitignore", paint.blue("i"));
        }
    }
}

/// Return true when the config file lives in a named subdirectory (not at project root).
fn is_subdir_config(config_path: &Path) -> bool {
    !config_path.is_absolute()
        && config_path
            .parent()
            .is_some_and(|p| !p.as_os_str().is_empty() && p != Path::new("."))
}

/// Walk the project tree and return YAML source fragments.
fn scan_for_sources(root: &Path) -> Vec<String> {
    let (dir_counts, root_docs) = walk_project(root);
    let consolidated = consolidate_dirs(dir_counts);
    format_sources(consolidated, root_docs)
}

/// Collect per-directory extension counts and root-level standalone doc files.
fn walk_project(root: &Path) -> (HashMap<PathBuf, ExtCounts>, Vec<String>) {
    let mut dir_counts: HashMap<PathBuf, ExtCounts> = HashMap::new();
    let mut root_docs: Vec<String> = Vec::new();

    let walker = ignore::WalkBuilder::new(root)
        .max_depth(Some(MAX_SCAN_DEPTH))
        .hidden(true)
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                let name = entry.file_name().to_string_lossy();
                return !SKIP_DIRS.contains(&name.as_ref());
            }
            true
        })
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_lowercase(),
            None => continue,
        };
        if !DOC_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }

        if entry.depth() == 1 {
            if let Some(name) = path.file_name().map(|n| n.to_string_lossy())
                && ROOT_DOC_FILES.contains(&name.as_ref())
                && !root_docs.contains(&name.to_string())
            {
                root_docs.push(name.to_string());
            }
            continue;
        }

        let rel = path.strip_prefix(root).unwrap_or(path);
        if let Some(parent) = rel.parent() {
            *dir_counts
                .entry(parent.to_owned())
                .or_default()
                .entry(ext)
                .or_default() += 1;
        }
    }

    (dir_counts, root_docs)
}

/// Accumulate extension counts from `source` into `target`.
fn merge_into(target: &mut ExtCounts, source: ExtCounts) {
    for (ext, count) in source {
        *target.entry(ext).or_default() += count;
    }
}

/// Roll up small dirs, merge siblings, then merge ancestor/descendant overlaps.
fn consolidate_dirs(dir_counts: HashMap<PathBuf, ExtCounts>) -> HashMap<PathBuf, ExtCounts> {
    let mut dirs = roll_up_small_dirs(dir_counts);
    merge_siblings(&mut dirs);
    merge_ancestors(&mut dirs);
    dirs
}

/// Small directories (< MIN_DOCS_PER_DIR files) merge into their parent.
/// Never rolls up to root -- root-level documents are standalone files.
fn roll_up_small_dirs(dir_counts: HashMap<PathBuf, ExtCounts>) -> HashMap<PathBuf, ExtCounts> {
    let mut rolled_up: HashMap<PathBuf, ExtCounts> = HashMap::new();
    for (dir, ext_counts) in dir_counts {
        let total: usize = ext_counts.values().sum();
        let target = if total < MIN_DOCS_PER_DIR {
            let parent = dir
                .parent()
                .map_or(dir.clone(), std::borrow::ToOwned::to_owned);
            if parent.as_os_str().is_empty() {
                continue;
            }
            parent
        } else {
            dir
        };
        merge_into(rolled_up.entry(target).or_default(), ext_counts);
    }
    rolled_up
}

/// When 2+ siblings share a parent, merge them into that parent.
/// Repeats until stable (handles nested sibling groups).
fn merge_siblings(dirs: &mut HashMap<PathBuf, ExtCounts>) {
    loop {
        let mut parent_children: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        for p in dirs.keys() {
            if let Some(parent) = p.parent()
                && !parent.as_os_str().is_empty()
            {
                parent_children
                    .entry(parent.to_owned())
                    .or_default()
                    .push(p.clone());
            }
        }
        let mut did_merge = false;
        for (parent, children) in parent_children {
            if children.len() >= 2 {
                for child in children {
                    if let Some(counts) = dirs.remove(&child) {
                        merge_into(dirs.entry(parent.clone()).or_default(), counts);
                    }
                }
                did_merge = true;
            }
        }
        if !did_merge {
            break;
        }
    }
}

/// When a directory and one of its descendants are both present, merge
/// the descendant into the ancestor.
fn merge_ancestors(dirs: &mut HashMap<PathBuf, ExtCounts>) {
    let mut paths: Vec<PathBuf> = dirs.keys().cloned().collect();
    paths.sort();
    let mut merges: Vec<(PathBuf, PathBuf)> = Vec::new();
    for i in 0..paths.len() {
        for j in 0..i {
            if paths[i].starts_with(&paths[j]) {
                merges.push((paths[i].clone(), paths[j].clone()));
                break;
            }
        }
    }
    for (child, ancestor) in merges {
        if let Some(counts) = dirs.remove(&child) {
            merge_into(dirs.entry(ancestor).or_default(), counts);
        }
    }
}

/// Convert consolidated directory map and root docs into YAML source fragments.
fn format_sources(dirs: HashMap<PathBuf, ExtCounts>, mut root_docs: Vec<String>) -> Vec<String> {
    let mut sorted_dirs: Vec<_> = dirs.into_iter().collect();
    sorted_dirs.sort_by(|a, b| a.0.cmp(&b.0));

    // Merging dirs with identical globs reduces redundant YAML entries.
    let mut by_glob: Vec<(String, Vec<String>)> = Vec::new();
    for (dir, ext_counts) in sorted_dirs {
        let total: usize = ext_counts.values().sum();
        if total < MIN_DOCS_PER_DIR {
            continue;
        }
        let glob = build_glob(&ext_counts);
        let dir_str = dir.display().to_string().replace('\\', "/");
        if let Some(entry) = by_glob.iter_mut().find(|(g, _)| g == &glob) {
            entry.1.push(format!("./{dir_str}"));
        } else {
            by_glob.push((glob, vec![format!("./{dir_str}")]));
        }
    }

    let mut sources = Vec::new();
    for (glob, paths) in by_glob {
        sources.push(format_path_entry(&paths, Some(&glob)));
    }

    root_docs.sort();
    if !root_docs.is_empty() {
        let paths: Vec<String> = root_docs.into_iter().map(|n| format!("./{n}")).collect();
        sources.push(format_path_entry(&paths, None));
    }

    sources
}

/// Format a single YAML source entry with a path (or path list) and optional glob.
fn format_path_entry(paths: &[String], glob: Option<&str>) -> String {
    let mut entry = String::new();
    if paths.len() == 1 {
        w!(entry, "  - path: {}", paths[0]);
    } else {
        entry.push_str("  - path:\n");
        for p in paths {
            wln!(entry, "      - {p}");
        }
        entry.truncate(entry.trim_end().len());
    }
    if let Some(g) = glob {
        w!(entry, "\n    glob: \"{g}\"");
    }
    entry
}

/// Pick a glob pattern that covers the dominant extensions in a directory.
fn build_glob(ext_counts: &ExtCounts) -> String {
    let total: usize = ext_counts.values().sum();
    if total == 0 {
        return "**/*".to_owned();
    }

    let mut exts: Vec<_> = ext_counts.iter().collect();
    exts.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));

    if let Some((top_ext, top_count)) = exts.first()
        && *top_count * 100 / total >= 80
    {
        return format!("**/*.{top_ext}");
    }

    let mut covered = 0;
    let mut selected: Vec<&str> = Vec::new();
    for (ext, count) in &exts {
        selected.push(ext.as_str());
        covered += *count;
        if covered * 100 / total >= 90 {
            break;
        }
    }

    if selected.len() == 1 {
        format!("**/*.{}", selected[0])
    } else if selected.len() <= 5 {
        selected.sort_unstable();
        format!("**/*.{{{}}}", selected.join(","))
    } else {
        "**/*".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::{build_glob, scan_for_sources};

    fn touch(root: &Path, rel_path: &str) {
        let full = root.join(rel_path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("create_dir_all");
        }
        fs::write(&full, b"").expect("write file");
    }

    #[test]
    fn build_glob_cases() {
        let counts: HashMap<String, usize> = HashMap::new();
        assert_eq!(build_glob(&counts), "**/*", "empty map should yield **/*");

        let counts = HashMap::from([("md".to_owned(), 8), ("rst".to_owned(), 2)]);
        assert_eq!(
            build_glob(&counts),
            "**/*.md",
            "80% dominant extension should yield **/*.md"
        );

        let counts = HashMap::from([("md".to_owned(), 5), ("txt".to_owned(), 5)]);
        let glob = build_glob(&counts);
        assert!(
            glob.starts_with("**/*.{"),
            "equal split should yield brace glob, got: {glob}"
        );
        assert!(glob.contains("md"), "brace glob should contain md: {glob}");
        assert!(
            glob.contains("txt"),
            "brace glob should contain txt: {glob}"
        );

        let counts: HashMap<String, usize> = ["md", "rst", "txt", "adoc", "org", "tex", "pdf"]
            .iter()
            .map(|e| ((*e).to_owned(), 1))
            .collect();
        assert_eq!(
            build_glob(&counts),
            "**/*",
            "7 distinct extensions (>5) should yield **/*"
        );

        let counts = HashMap::from([("rst".to_owned(), 5), ("md".to_owned(), 5)]);
        let glob = build_glob(&counts);
        assert!(
            glob.starts_with("**/*.{"),
            "tied extensions should yield brace glob, got: {glob}"
        );
        let inner = glob.trim_start_matches("**/*.{").trim_end_matches('}');
        let parts: Vec<&str> = inner.split(',').collect();
        assert!(
            parts.windows(2).all(|w| w[0] <= w[1]),
            "extensions in brace glob should be alphabetically ordered: {glob}"
        );
    }

    #[test]
    fn scan_for_sources_detects_docs_and_skips_git() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        touch(root, "README.md");
        touch(root, "CHANGELOG.md");

        touch(root, "docs/intro.md");
        touch(root, "docs/guide.md");

        touch(root, "docs/sub1/single.md");
        touch(root, "docs/sub2/other.md");

        touch(root, ".git/objects/foo.md");
        touch(root, ".git/objects/bar.md");

        for i in 0..5 {
            touch(root, &format!("mixed/file{i}.md"));
            touch(root, &format!("mixed/file{i}.rst"));
        }

        let sources = scan_for_sources(root);
        let joined = sources.join("\n");

        assert!(
            joined.contains("README.md"),
            "README.md should be a root doc source: {joined}"
        );
        assert!(
            joined.contains("CHANGELOG.md"),
            "CHANGELOG.md should be a root doc source: {joined}"
        );

        let docs_source = sources
            .iter()
            .find(|s| s.contains("docs") && s.contains("**/*.md"));
        assert!(
            docs_source.is_some(),
            "expected a docs/ source with **/*.md glob: {joined}"
        );

        assert!(
            !joined.contains(".git"),
            ".git contents should be skipped: {joined}"
        );

        let mixed_source = sources
            .iter()
            .find(|s| s.contains("mixed") && s.contains("**/*.{"));
        assert!(
            mixed_source.is_some(),
            "expected mixed/ source with brace glob: {joined}"
        );

        assert_eq!(
            sources.len(),
            3,
            "expected 3 total sources (root docs list + docs/ + mixed/), got: {sources:?}"
        );
    }

    #[test]
    fn init_config_path_cases() {
        // Case 1: root-level path -- no base_dir key written.
        let dir = tempdir().unwrap();
        let root_config = dir.path().join("lore.yaml");
        super::init(Some(root_config.clone())).unwrap();
        let content = fs::read_to_string(&root_config).unwrap();
        assert!(
            !content.contains("base_dir:"),
            "config at project root should not contain 'base_dir:', got:\n{content}"
        );

        // Case 2: nested path with non-existent parent -- dir is created and file exists.
        let dir2 = tempdir().unwrap();
        let nested = dir2.path().join("nested").join("lore.yaml");
        assert!(
            !nested.parent().unwrap().is_dir(),
            "parent dir should not exist before init"
        );
        super::init(Some(nested.clone())).expect("init should create parent dir and write config");
        assert!(
            nested.is_file(),
            "config file should exist after init: {}",
            nested.display()
        );
    }
}
