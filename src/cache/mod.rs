pub(crate) mod http;

use std::path::{Path, PathBuf};

use anyhow::Result;

const MARKER_FILE: &str = ".lore.cache";
const MARKER_SALT: &str = "lore-cache-v1";

/// Selects which portion of the cache to operate on.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CacheScope {
    /// Clear all cached data (default).
    All,
    /// Extracted archive contents.
    Archives,
    /// HTTP responses with ETag/Last-Modified support.
    Http,
    /// Ingest failure logs.
    Logs,
    /// Bare clones of git repositories.
    Repos,
    /// Temporary files (cleaned up automatically).
    Tmp,
}

/// Individual scopes (everything except `All`).
const SCOPES: &[CacheScope] = &[
    CacheScope::Archives,
    CacheScope::Http,
    CacheScope::Logs,
    CacheScope::Repos,
    CacheScope::Tmp,
];

impl std::fmt::Display for CacheScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::All => "all",
            Self::Archives => "archives",
            Self::Http => "http",
            Self::Logs => "logs",
            Self::Repos => "repos",
            Self::Tmp => "tmp",
        })
    }
}

impl std::str::FromStr for CacheScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "all" => Ok(Self::All),
            "archives" => Ok(Self::Archives),
            "http" => Ok(Self::Http),
            "logs" => Ok(Self::Logs),
            "repos" => Ok(Self::Repos),
            "tmp" => Ok(Self::Tmp),
            _ => Err(format!(
                "invalid cache scope '{s}': expected one of all, archives, http, logs, repos, tmp"
            )),
        }
    }
}

impl CacheScope {
    const ARCHIVES_DIR: &'static str = "archives";
    const HTTP_DIR: &'static str = "http";
    const LOGS_DIR: &'static str = "logs";
    const REPOS_DIR: &'static str = "repos";
    const TMP_DIR: &'static str = "tmp";

    /// Return the subdirectory name for this scope, or `None` for `All`.
    fn subdir(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Archives => Some(Self::ARCHIVES_DIR),
            Self::Http => Some(Self::HTTP_DIR),
            Self::Logs => Some(Self::LOGS_DIR),
            Self::Repos => Some(Self::REPOS_DIR),
            Self::Tmp => Some(Self::TMP_DIR),
        }
    }
}

/// Return the cache root directory (~/.cache/lore or $LORE_CACHE_DIR).
fn cache_root() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("LORE_CACHE_DIR") {
        let p = PathBuf::from(&dir);
        let depth = p
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .count();
        let has_parent_dir = p
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir));
        if has_parent_dir {
            tracing::warn!(
                "LORE_CACHE_DIR contains '..' components, ignoring and using default cache dir"
            );
        } else {
            anyhow::ensure!(
                !p.is_relative(),
                "LORE_CACHE_DIR must be an absolute path, got: {}",
                p.display()
            );
            // Reject paths that are too shallow (e.g. "/", "/etc") to prevent
            // accidental cache operations on sensitive system directories.
            anyhow::ensure!(
                depth >= 3,
                "LORE_CACHE_DIR path is too shallow: {}",
                p.display()
            );
            return Ok(p);
        }
    }
    Ok(dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("lore"))
}

/// Return a persistent subdirectory under the cache root, creating it if needed.
fn cache_dir(parts: &[&str]) -> Result<PathBuf> {
    let mut p = cache_root()?;
    crate::util::ensure_marker(&p, MARKER_FILE, MARKER_SALT)?;
    for part in parts {
        p.push(part);
    }
    std::fs::create_dir_all(&p)?;
    Ok(p)
}

/// Clean up all temp directories in the tmp cache scope.
pub(crate) fn cleanup_tmp() -> usize {
    clear_cache(CacheScope::Tmp).map_or_else(
        |e| {
            tracing::warn!("failed to clean tmp cache: {e}");
            0
        },
        |(count, _)| count,
    )
}

/// Return an HTTP cache directory.
pub(crate) fn http_cache_dir() -> Result<PathBuf> {
    cache_dir(&[CacheScope::HTTP_DIR])
}

/// Return the logs directory under the cache root, creating it if needed.
///
/// # Errors
///
/// Returns an error if the cache root cannot be determined or the directory cannot be created.
pub fn logs_dir() -> Result<PathBuf> {
    cache_dir(&[CacheScope::LOGS_DIR])
}

/// Return the git repo cache path for a given URL.
pub(crate) fn repo_cache_path(repo_url: &str) -> Result<PathBuf> {
    let hash = &crate::util::blake3_hex(repo_url)[..16];
    cache_dir(&[CacheScope::REPOS_DIR, hash])
}

/// Clear cached data. Returns `(items_removed, bytes_freed)`.
///
/// # Errors
///
/// Returns an error if the cache root cannot be determined or deletion fails.
pub fn clear_cache(scope: CacheScope) -> Result<(usize, u64)> {
    clear_cache_at(&cache_root()?, scope)
}

/// Clear cached entries under `root` for the given scope; returns `(items_removed, bytes_freed)`.
fn clear_cache_at(root: &Path, scope: CacheScope) -> Result<(usize, u64)> {
    // Canonicalize the path to resolve symlinks. The marker contains a keyed
    // hash of the directory path at write time. By canonicalizing first we
    // ensure that:
    //  - The checks and the deletion happen on the same resolved path.
    //  - A symlink whose target path differs from the marker path (i.e. the
    //    symlink was redirected after the marker was written) is rejected.
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    // Safety: refuse to remove paths with <= 2 components (e.g. "/" or "/home")
    anyhow::ensure!(
        canonical.components().count() > 2,
        "refusing to clear cache at suspiciously short path: {}",
        canonical.display()
    );

    // Safety: require a valid marker file that lore writes on first use.
    // The marker contains a keyed hash of the absolute path, so a file
    // copied from another directory or hand-created won't pass.
    // Using the canonical path prevents a symlink-swap attack: if root is
    // a symlink whose target was swapped to a different directory after the
    // marker was written, the canonical path won't match the stored hash.
    anyhow::ensure!(
        crate::util::verify_marker(&canonical, MARKER_FILE, MARKER_SALT),
        "refusing to clear cache: {} is not a lore cache directory (missing or invalid {} marker)",
        canonical.display(),
        MARKER_FILE,
    );

    let scopes: &[CacheScope] = match scope {
        CacheScope::All => SCOPES,
        _ => std::slice::from_ref(&scope),
    };

    let mut count = 0usize;
    let mut bytes = 0u64;
    for s in scopes {
        let dir = canonical.join(s.subdir().expect("SCOPES never contains CacheScope::All"));
        let entries: Vec<_> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd.filter_map(std::result::Result::ok).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let path = entry.path();
            if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                bytes += crate::util::dir_size(&path);
                std::fs::remove_dir_all(&path)?;
            } else {
                bytes += entry.metadata().map_or(0, |m| m.len());
                std::fs::remove_file(&path)?;
            }
            count += 1;
        }
    }

    Ok((count, bytes))
}

/// Return a cache directory for extracted archive contents.
/// Each archive gets a unique directory based on the SHA-256 of its path.
pub(crate) fn archive_extract_dir(archive_path: &Path) -> Result<PathBuf> {
    let key = crate::util::blake3_hex(&archive_path.to_string_lossy());
    cache_dir(&[CacheScope::ARCHIVES_DIR, &key[..16]])
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    #[test]
    fn cache_clear_safety() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("not-a-cache").join("foo").join("bar");
        std::fs::create_dir_all(&root).unwrap();
        let result = clear_cache_at(&root, CacheScope::All);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not a lore cache directory"),
            "expected 'not a lore cache directory' error for unmarked dir"
        );

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("fake-cache").join("sub");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(MARKER_FILE), "not-the-right-hash").unwrap();
        let result = clear_cache_at(&root, CacheScope::All);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not a lore cache directory"),
            "expected 'not a lore cache directory' error for wrong marker content"
        );

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("my-cache").join("subdir");
        std::fs::create_dir_all(root.join("http")).unwrap();
        crate::util::ensure_marker(&root, MARKER_FILE, MARKER_SALT).unwrap();
        std::fs::write(root.join("http").join("test.txt"), "data").unwrap();
        let test_file = root.join("http").join("test.txt");
        assert!(test_file.exists(), "test file should exist before clear");
        let result = clear_cache_at(&root, CacheScope::Http);
        assert!(result.is_ok(), "expected Ok for valid marked directory");
        let (count, bytes) = result.unwrap();
        assert!(count > 0, "expected count > 0 after clearing data");
        assert!(bytes > 0, "expected bytes > 0 after clearing data");
        assert!(
            !test_file.exists(),
            "test file should be deleted after cache clear"
        );

        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            let cache_dir = dir.path().join("real-cache").join("a").join("b");
            std::fs::create_dir_all(&cache_dir).unwrap();
            crate::util::ensure_marker(&cache_dir, MARKER_FILE, MARKER_SALT).unwrap();
            let target_dir = dir.path().join("sensitive").join("a").join("b");
            std::fs::create_dir_all(&target_dir).unwrap();
            let symlink_path = dir.path().join("symlink-cache");
            std::os::unix::fs::symlink(&target_dir, &symlink_path).unwrap();
            let result = clear_cache_at(&symlink_path, CacheScope::All);
            assert!(
                result.is_err(),
                "symlink pointing to unmarked directory should be rejected"
            );
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("not a lore cache directory"),
                "expected 'not a lore cache directory' error for symlink bypass"
            );
        }
    }

    #[test]
    #[serial]
    fn cache_root_validation() {
        #[cfg(unix)]
        let shallow_paths: &[&str] = &["/", "/tmp"];
        #[cfg(windows)]
        let shallow_paths: &[&str] = &["C:\\", "C:\\tmp"];
        for shallow in shallow_paths {
            // SAFETY: test-only mutation; no other threads touch this env var.
            unsafe { std::env::set_var("LORE_CACHE_DIR", shallow) };
            let result = cache_root();
            assert!(
                result.is_err(),
                "expected error for shallow LORE_CACHE_DIR={shallow}"
            );
            assert!(
                result.unwrap_err().to_string().contains("too shallow"),
                "expected 'too shallow' error message"
            );
        }

        // SAFETY: test-only mutation; no other threads touch this env var.
        unsafe { std::env::set_var("LORE_CACHE_DIR", "a/b/c") };
        let result = cache_root();
        assert!(
            result.is_err(),
            "expected error for relative LORE_CACHE_DIR=a/b/c"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must be an absolute path"),
            "expected 'must be an absolute path' error message for relative path"
        );

        // Clean up env var so it doesn't leak into subsequent sub-cases.
        // SAFETY: test-only mutation; no other threads touch this env var.
        unsafe { std::env::remove_var("LORE_CACHE_DIR") };

        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a").join("b").join("c");
        // SAFETY: test-only mutation; no other threads touch this env var.
        unsafe { std::env::set_var("LORE_CACHE_DIR", &deep) };
        let result = cache_root();
        // Clean up before asserting so the env var is always removed.
        // SAFETY: test-only mutation; no other threads touch this env var.
        unsafe { std::env::remove_var("LORE_CACHE_DIR") };
        assert!(result.is_ok(), "expected Ok for valid deep LORE_CACHE_DIR");
        assert_eq!(result.unwrap(), deep);
    }

    #[test]
    fn cache_scope_round_trip_and_rejection() {
        let all_scopes = [
            CacheScope::All,
            CacheScope::Archives,
            CacheScope::Http,
            CacheScope::Logs,
            CacheScope::Repos,
            CacheScope::Tmp,
        ];
        for scope in all_scopes {
            let s = scope.to_string();
            let parsed: CacheScope = s.parse().expect("round-trip parse failed");
            assert_eq!(parsed.to_string(), s, "round-trip mismatch for {s}");
        }

        let result = "invalid".parse::<CacheScope>();
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("invalid cache scope"),
            "unexpected error: {msg}"
        );
    }
}
