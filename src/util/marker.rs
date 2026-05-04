//! Sentinel marker files for safe destructive operations.

use std::path::Path;

use anyhow::Result;

use crate::util::{atomic_write, blake3_hex};

/// Write a sentinel marker so destructive operations can verify directory ownership.
///
/// # Durability note
///
/// `atomic_write` uses a rename to make the marker file visible atomically, but
/// does **not** fsync the parent directory.  On a crash between the rename and a
/// subsequent directory fsync the directory entry may not be durable.  For the
/// purposes of this marker (protecting against accidental deletion of a wrong
/// directory) this is acceptable: a crash here is no worse than the marker never
/// being written.  If hard durability is required, callers must fsync the
/// parent directory after this function returns.
pub fn ensure_marker(root: &Path, file_name: &str, salt: &str) -> Result<()> {
    // `canonicalize` resolves symlinks so the hash is stable across relative-path
    // variations.  The fallback to the raw path (when the directory does not yet
    // exist or I/O fails) weakens symlink-swap protection: an attacker who can
    // atomically replace the directory with a symlink between the fallback and the
    // marker write could redirect destructive operations.  This is acceptable because
    // the caller is responsible for ensuring the directory exists before ingestion.
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let marker = canonical.join(file_name);
    let expected = blake3_hex(&format!("{salt}:{}", canonical.display()));
    let current = std::fs::read_to_string(&marker).unwrap_or_default();
    if current.trim() != expected {
        atomic_write(&marker, expected.as_bytes())?;
    }
    Ok(())
}

/// Check that a sentinel marker exists and contains the correct token.
pub fn verify_marker(root: &Path, file_name: &str, salt: &str) -> bool {
    // See `ensure_marker` for the caveat about the canonicalize fallback weakening
    // symlink-swap protection when the directory is not yet resolvable.
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let marker = canonical.join(file_name);
    let Ok(content) = std::fs::read_to_string(&marker) else {
        return false;
    };
    content.trim() == blake3_hex(&format!("{salt}:{}", canonical.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_verification() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let root = dir.path();
        ensure_marker(root, ".lore_marker", "mysalt").expect("ensure_marker should succeed");
        assert!(
            verify_marker(root, ".lore_marker", "mysalt"),
            "verify_marker should return true after ensure_marker with same salt"
        );

        let dir = tempfile::tempdir().expect("create tempdir");
        let root = dir.path();
        ensure_marker(root, ".lore_marker", "salt-a").expect("ensure_marker should succeed");
        assert!(
            !verify_marker(root, ".lore_marker", "salt-b"),
            "verify_marker should return false when salt differs"
        );

        let dir = tempfile::tempdir().expect("create tempdir");
        let root = dir.path();
        assert!(
            !verify_marker(root, ".lore_marker", "anysalt"),
            "verify_marker should return false when marker file is absent"
        );
    }
}
