//! Filesystem path helpers and file-listing utilities.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Maximum recursion depth for `dir_size`.
const DIR_SIZE_MAX_DEPTH: usize = 64;

/// Compute the total size of `path` in bytes by recursively walking its contents.
/// Symlinks are not followed and do not contribute to the total.
/// Caps recursion at 64 levels to bound scan time.
pub fn dir_size(path: &Path) -> u64 {
    dir_size_inner(path, 0)
}

/// Recursive helper for [`dir_size`]; `depth` caps recursion.
fn dir_size_inner(path: &Path, depth: usize) -> u64 {
    if depth > DIR_SIZE_MAX_DEPTH {
        return 0;
    }
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_file() {
                total += entry.metadata().map_or(0, |m| m.len());
            } else if ft.is_dir() {
                total += dir_size_inner(&entry.path(), depth + 1);
            }
        }
    }
    total
}

/// Normalize a path by removing redundant `.` and `..` components (no I/O).
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if out
                    .components()
                    .next_back()
                    .is_some_and(|c| c != std::path::Component::ParentDir)
                    && out.pop()
                {
                    // popped a real directory component
                } else {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// Make a path relative to a base directory, with forward slashes.
pub fn relativize_path(path: &Path, base: &Path) -> String {
    let norm_path = normalize_path(path);
    let norm_base = normalize_path(base);
    let rel = norm_path.strip_prefix(&norm_base).unwrap_or(&norm_path);
    let lossy = rel.to_string_lossy();
    #[cfg(windows)]
    {
        lossy.replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        lossy.into_owned()
    }
}

/// Check that a path has no traversal components and is non-empty.
///
/// A `./`-prefixed path (e.g. `./foo/bar`) is permitted: `CurDir` (`.`)
/// components are allowed, only `ParentDir` (`..`), `RootDir` (`/`), and
/// Windows `Prefix` components are rejected.  The check is purely syntactic --
/// it does not perform I/O or resolve symlinks, so a path that passes here may
/// still escape a sandbox if intermediate symlinks point outside it.
pub fn is_lexically_safe_path(path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return false;
    }
    for component in path.components() {
        match component {
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return false,
            _ => {}
        }
    }
    true
}

/// Write data atomically via a temp file + rename.
///
/// On Unix, if the destination file already exists its permissions are preserved on
/// the new file after the rename. New files receive the default `NamedTempFile`
/// permissions (typically 0o600).
///
/// # Errors
///
/// Returns an error if the parent directory cannot be created, the temp file cannot be written, or the rename fails.
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path.parent().context("no parent directory")?;
    std::fs::create_dir_all(parent).context("failed to create parent directory")?;

    // Read existing permissions before writing so we can restore them after rename.
    #[cfg(unix)]
    let existing_permissions = std::fs::metadata(path).ok().map(|m| m.permissions());

    let mut tmp = tempfile::NamedTempFile::new_in(parent).context("failed to create temp file")?;
    std::io::Write::write_all(&mut tmp, data).context("failed to write temp file")?;
    tmp.as_file()
        .sync_all()
        .context("failed to fsync temp file")?;
    tmp.persist(path).context("failed to persist temp file")?;

    // Fsync the parent directory to ensure the rename is durable.
    #[cfg(unix)]
    if let Ok(dir) = std::fs::File::open(parent) {
        dir.sync_all().ok();
    }

    // Restore original file permissions after the atomic rename.
    #[cfg(unix)]
    if let Some(perms) = existing_permissions {
        std::fs::set_permissions(path, perms).context("failed to restore file permissions")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(256))]

        #[test]
        fn prop_is_lexically_safe_path_no_panic(s in ".*") {
            let _ = is_lexically_safe_path(Path::new(&s));
        }
    }

    #[test]
    fn path_functions() {
        for (input, expected) in [
            ("foo/bar.md", true),
            ("./relative", true),
            ("", false),
            ("../escape", false),
            ("/absolute", false),
            ("foo/../../etc/passwd", false),
        ] {
            assert_eq!(
                is_lexically_safe_path(Path::new(input)),
                expected,
                "is_lexically_safe_path({input:?}) should be {expected}"
            );
        }

        for (input, expected) in [
            ("a/./b", "a/b"),
            ("a/b/../c", "a/c"),
            (".", "."),
            ("a/b/c/../../d", "a/d"),
            ("..", ".."),
            ("../..", "../.."),
            ("../../foo", "../../foo"),
            ("a/../..", ".."),
        ] {
            assert_eq!(
                normalize_path(Path::new(input)),
                PathBuf::from(expected),
                "normalize_path({input:?}) should be {expected:?}"
            );
        }
    }

    #[test]
    fn dir_size_depth_limit_does_not_panic() {
        // Create a directory nested deeper than DIR_SIZE_MAX_DEPTH to verify
        // the depth cap is enforced without stack overflow or infinite recursion.
        let tmp = tempfile::tempdir().unwrap();
        let mut current = tmp.path().to_path_buf();
        for _ in 0..70 {
            current = current.join("sub");
            std::fs::create_dir_all(&current).unwrap();
        }
        std::fs::write(current.join("file.txt"), b"hello").unwrap();
        let size = dir_size(tmp.path());
        assert_eq!(size, 0, "file beyond depth cap should not be counted");
    }
}
