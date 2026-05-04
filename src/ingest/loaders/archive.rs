use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::cache;

/// Known archive file extensions (single-part).
pub const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "tar", "tgz", "gz", "bz2", "xz", "zst", "zstd"];

/// Compound suffixes for archive detection (e.g. `.tar.gz`).
pub const COMPOUND_SUFFIXES: &[&str] = &[
    ".tar.gz",
    ".tar.bz2",
    ".tar.xz",
    ".tar.zip",
    ".tar.zst",
    ".tar.zstd",
];

/// MIME types that indicate archive content from HTTP responses.
const ARCHIVE_MIME_TYPES: &[&str] = &[
    "application/zip",
    "application/x-tar",
    "application/gzip",
    "application/x-gzip",
    "application/x-bzip2",
    "application/x-xz",
    "application/x-7z-compressed",
    "application/zstd",
    "application/x-compress",
    "application/x-compressed",
];

/// Maximum directory nesting depth to prevent stack-like exhaustion.
const MAX_COLLECT_DEPTH: usize = 128;

/// Compression codec applied to a tar stream before extraction.
#[derive(Clone, Copy)]
enum TarCompression {
    // None first: uncompressed tar is the default/most common when reading plain `.tar` files.
    None,
    Gz,
    Bz2,
    Xz,
    Zstd,
}

/// Strip MIME type parameters (e.g. "text/html;charset=utf-8" -> "text/html").
fn base_mime(ct: &str) -> &str {
    ct.split(';').next().unwrap_or(ct).trim()
}

/// Return `true` if the path's filename matches a known archive extension or compound suffix.
pub(crate) fn is_archive(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    for suffix in COMPOUND_SUFFIXES {
        if name.ends_with(suffix) {
            return true;
        }
    }

    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            ARCHIVE_EXTENSIONS
                .iter()
                .any(|a| ext.eq_ignore_ascii_case(a))
        })
}

/// Return `true` if the HTTP `Content-Type` header indicates an archive format.
pub(crate) fn is_archive_content_type(ct: Option<&str>) -> bool {
    ct.is_some_and(|ct| {
        let base = base_mime(ct);
        ARCHIVE_MIME_TYPES
            .iter()
            .any(|m| base.eq_ignore_ascii_case(m))
    })
}

/// Detect the archive format from the file path and extract to disk.
/// Returns paths to all extracted files.
///
/// Extracts to `~/.cache/lore/archives/{sha256(path)[..16]}/`.
/// Uses a `.complete` marker with mtime check to skip re-extraction
/// if the archive file is unchanged.
pub(crate) async fn extract(
    archive_path: &Path,
    max_files: usize,
    max_bytes: u64,
) -> Result<Vec<PathBuf>> {
    let archive_path = archive_path.to_path_buf();

    tokio::task::spawn_blocking(move || extract_sync(&archive_path, max_files, max_bytes))
        .await
        .context("archive extraction panicked")?
}

/// Synchronous core of archive extraction, dispatching on format by filename.
fn extract_sync(archive_path: &Path, max_files: usize, max_bytes: u64) -> Result<Vec<PathBuf>> {
    let extract_dir = cache::archive_extract_dir(archive_path)?;
    let complete_marker = extract_dir.join(".complete");

    // Cache check: skip re-extraction if archive is unchanged
    if complete_marker.exists()
        && let (Ok(archive_meta), Ok(marker_meta)) = (
            std::fs::metadata(archive_path),
            std::fs::metadata(&complete_marker),
        )
    {
        let archive_mtime = match archive_meta.modified() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(path = %archive_path.display(), "cannot read mtime: {e}");
                std::time::UNIX_EPOCH
            }
        };
        let marker_mtime = match marker_meta.modified() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(path = %complete_marker.display(), "cannot read mtime: {e}");
                std::time::UNIX_EPOCH
            }
        };
        if marker_mtime >= archive_mtime {
            return Ok(collect_extracted_files(&extract_dir));
        }
    }

    // Clean previous extraction
    if extract_dir.exists() {
        std::fs::remove_dir_all(&extract_dir).with_context(|| {
            format!(
                "failed to clean extraction directory: {}",
                extract_dir.display()
            )
        })?;
    }
    std::fs::create_dir_all(&extract_dir).with_context(|| {
        format!(
            "failed to create extraction directory: {}",
            extract_dir.display()
        )
    })?;

    let name = archive_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    let ext_is = |e: &str| {
        std::path::Path::new(&name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case(e))
    };

    let count = if name.ends_with(".tar.zip") {
        extract_tar_zip(archive_path, &extract_dir, max_files, max_bytes)?
    } else if name.ends_with(".tar.gz") || ext_is("tgz") {
        extract_tar_compressed(
            archive_path,
            &extract_dir,
            max_files,
            max_bytes,
            TarCompression::Gz,
        )?
    } else if name.ends_with(".tar.bz2") {
        extract_tar_compressed(
            archive_path,
            &extract_dir,
            max_files,
            max_bytes,
            TarCompression::Bz2,
        )?
    } else if name.ends_with(".tar.xz") {
        extract_tar_compressed(
            archive_path,
            &extract_dir,
            max_files,
            max_bytes,
            TarCompression::Xz,
        )?
    } else if name.ends_with(".tar.zst") || name.ends_with(".tar.zstd") {
        extract_tar_compressed(
            archive_path,
            &extract_dir,
            max_files,
            max_bytes,
            TarCompression::Zstd,
        )?
    } else if ext_is("zip") {
        extract_zip(archive_path, &extract_dir, max_files, max_bytes)?
    } else if ext_is("tar") {
        extract_tar_compressed(
            archive_path,
            &extract_dir,
            max_files,
            max_bytes,
            TarCompression::None,
        )?
    } else if ext_is("gz") {
        extract_gz_single(archive_path, &extract_dir, max_bytes)?
    } else {
        match extract_zip(archive_path, &extract_dir, max_files, max_bytes) {
            Ok(n) => n,
            Err(_) => extract_tar_compressed(
                archive_path,
                &extract_dir,
                max_files,
                max_bytes,
                TarCompression::Gz,
            )?,
        }
    };

    info!(
        archive = %archive_path.display(),
        files = count,
        "extracted archive contents"
    );

    if let Err(e) = std::fs::write(&complete_marker, b"") {
        warn!(path = %complete_marker.display(), "failed to write archive completion marker: {e}");
    }

    Ok(collect_extracted_files(&extract_dir))
}

/// Collect all files from the extraction directory.
fn collect_extracted_files(extract_dir: &Path) -> Vec<PathBuf> {
    // Pre-allocate a modest capacity. Archives typically contain at least a
    // handful of files; 64 avoids early reallocation for the common case
    // without over-allocating for small archives.
    let mut files = Vec::with_capacity(64);
    collect_files_iterative(extract_dir, &mut files);
    files.sort();
    files
}

/// Iteratively walk `base`, appending regular files to `files` while skipping symlinks.
fn collect_files_iterative(base: &Path, files: &mut Vec<PathBuf>) {
    // Iterative walk with explicit stack to avoid stack overflow on deep nesting.
    let mut stack: Vec<(PathBuf, usize)> = vec![(base.to_path_buf(), 0)];

    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten() {
            // Use DirEntry::file_type() which does NOT follow symlinks,
            // preventing symlink escape out of the extraction directory.
            let Ok(ft) = entry.file_type() else { continue };

            let path = entry.path();

            if ft.is_dir() {
                if depth < MAX_COLLECT_DEPTH {
                    stack.push((path, depth + 1));
                } else {
                    warn!(
                        path = %path.display(),
                        "archive directory nesting exceeds limit of {MAX_COLLECT_DEPTH}, skipping"
                    );
                }
                continue;
            }

            // Skip symlinks entirely -- they could point outside the extract dir
            if ft.is_symlink() {
                warn!(path = %path.display(), "skipping symlink in extracted archive");
                continue;
            }

            // Skip the .complete marker
            if path.file_name().and_then(|n| n.to_str()) == Some(".complete") {
                continue;
            }

            files.push(path);
        }
    }
}

/// Write a single archive entry to disk with size limits.
fn write_limited_entry(
    reader: &mut impl std::io::Read,
    out_path: &Path,
    entry_display: &std::path::Path,
    max_bytes: u64,
    total_bytes: &mut u64,
) -> Result<()> {
    use std::io::Read;
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory: {}", parent.display()))?;
    }
    let remaining = max_bytes - *total_bytes;
    let mut limited = reader.take(remaining.saturating_add(1));
    let mut out_file = std::fs::File::create(out_path)
        .with_context(|| format!("failed to create: {}", out_path.display()))?;
    let written = std::io::copy(&mut limited, &mut out_file)
        .with_context(|| format!("failed to extract: {}", entry_display.display()))?;
    if written > remaining {
        anyhow::bail!("archive exceeds maximum decompressed size limit of {max_bytes} bytes");
    }
    *total_bytes += written;
    Ok(())
}

/// Extract a tar archive from any `Read` source into `dest`, enforcing file and byte limits.
fn extract_tar_from_reader<R: std::io::Read>(
    reader: R,
    dest: &Path,
    max_files: usize,
    max_bytes: u64,
) -> Result<usize> {
    let mut archive = tar::Archive::new(reader);
    let mut count = 0;
    let mut total_bytes: u64 = 0;

    for entry in archive.entries().context("failed to read tar entries")? {
        let mut entry = match entry {
            Ok(e) => e,
            Err(e) => {
                warn!("skipping tar entry: {e}");
                continue;
            }
        };

        let entry_path = entry.path().context("invalid tar entry path")?.into_owned();

        if !crate::util::is_lexically_safe_path(&entry_path) {
            warn!(path = %entry_path.display(), "skipping archive entry with unsafe path");
            continue;
        }

        if entry.header().entry_type().is_dir() {
            let dir = dest.join(&entry_path);
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create directory: {}", dir.display()))?;
            continue;
        }

        if !entry.header().entry_type().is_file() {
            continue;
        }

        if count >= max_files {
            anyhow::bail!("archive exceeds maximum file count limit of {max_files}");
        }

        // Fast-reject using header-declared size (attacker-controlled,
        // but catches obviously oversized entries without I/O).
        let declared_size = entry.header().size().unwrap_or(0);
        if total_bytes.saturating_add(declared_size) >= max_bytes {
            anyhow::bail!("archive exceeds maximum decompressed size limit of {max_bytes} bytes");
        }

        let out_path = dest.join(&entry_path);
        if !out_path.starts_with(dest) {
            warn!(path = %entry_path.display(), "archive entry escapes destination directory");
            continue;
        }
        write_limited_entry(
            &mut entry,
            &out_path,
            &entry_path,
            max_bytes,
            &mut total_bytes,
        )?;
        count += 1;
    }

    Ok(count)
}

/// Open a compressed or uncompressed tar archive and extract it via `extract_tar_from_reader`.
fn extract_tar_compressed(
    archive_path: &Path,
    dest: &Path,
    max_files: usize,
    max_bytes: u64,
    compression: TarCompression,
) -> Result<usize> {
    let file = std::fs::File::open(archive_path)?;
    match compression {
        TarCompression::None => extract_tar_from_reader(file, dest, max_files, max_bytes),
        TarCompression::Gz => extract_tar_from_reader(
            flate2::read::GzDecoder::new(file),
            dest,
            max_files,
            max_bytes,
        ),
        TarCompression::Bz2 => extract_tar_from_reader(
            bzip2::read::BzDecoder::new(file),
            dest,
            max_files,
            max_bytes,
        ),
        TarCompression::Xz => {
            extract_tar_from_reader(xz2::read::XzDecoder::new(file), dest, max_files, max_bytes)
        }
        TarCompression::Zstd => extract_tar_from_reader(
            zstd::stream::read::Decoder::new(file)?,
            dest,
            max_files,
            max_bytes,
        ),
    }
}

/// Extract a zip archive into `dest`, enforcing file count and byte size limits.
fn extract_zip(
    archive_path: &Path,
    dest: &Path,
    max_files: usize,
    max_bytes: u64,
) -> Result<usize> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).context("failed to open zip archive")?;
    let mut count = 0;
    let mut total_bytes: u64 = 0;

    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(e) => {
                warn!("skipping zip entry {i}: {e}");
                continue;
            }
        };

        let entry_path = if let Some(p) = entry.enclosed_name() {
            p.clone()
        } else {
            warn!("skipping zip entry with unsafe name: {:?}", entry.name());
            continue;
        };

        if !crate::util::is_lexically_safe_path(&entry_path) {
            warn!(path = %entry_path.display(), "skipping zip entry with unsafe path");
            continue;
        }

        if entry.is_dir() {
            let dir = dest.join(&entry_path);
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create directory: {}", dir.display()))?;
            continue;
        }

        if count >= max_files {
            anyhow::bail!("archive exceeds maximum file count limit of {max_files}");
        }

        let declared_size = entry.size();
        if total_bytes.saturating_add(declared_size) >= max_bytes {
            anyhow::bail!("archive exceeds maximum decompressed size limit of {max_bytes} bytes");
        }

        let out_path = dest.join(&entry_path);
        if !out_path.starts_with(dest) {
            warn!(path = %entry_path.display(), "archive entry escapes destination directory");
            continue;
        }
        write_limited_entry(
            &mut entry,
            &out_path,
            &entry_path,
            max_bytes,
            &mut total_bytes,
        )?;
        count += 1;
    }

    Ok(count)
}

/// Extract a `.tar.zip` archive by first unzipping the outer zip, then extracting the inner tar.
fn extract_tar_zip(
    archive_path: &Path,
    dest: &Path,
    max_files: usize,
    max_bytes: u64,
) -> Result<usize> {
    let zip_staging = dest.join(".zip_staging");
    std::fs::create_dir_all(&zip_staging)?;
    extract_zip(archive_path, &zip_staging, max_files, max_bytes)?;

    let tar_file = std::fs::read_dir(&zip_staging)?
        .filter_map(std::result::Result::ok)
        .find(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("tar"))
        })
        .map(|e| e.path());

    let result = if let Some(tar_path) = tar_file {
        extract_tar_compressed(&tar_path, dest, max_files, max_bytes, TarCompression::None)
    } else {
        anyhow::bail!("no .tar file found inside .tar.zip archive")
    };
    if let Err(e) = std::fs::remove_dir_all(&zip_staging) {
        warn!(path = %zip_staging.display(), "failed to remove zip staging directory: {e}");
    }
    result
}

/// Extract a bare `.gz` file (not `.tar.gz`) to a single output file in `dest`.
///
/// The output filename is the `.gz` stem with any directory components stripped.
///
/// Edge cases:
/// - **Empty gz file**: `std::io::copy` returns `Ok(0)` (zero bytes written).
///   The function still returns `Ok(1)` to indicate one output file was
///   produced. The output file will exist but be empty. This is intentional --
///   an empty decompressed file is valid and should not be an error.
/// - **Oversized content**: extraction is capped at `max_bytes + 1` via `take()`.
///   If the decompressed content exceeds `max_bytes`, the function returns an error
///   after the bytes are written (and the partial file is left on disk).
fn extract_gz_single(archive_path: &Path, dest: &Path, max_bytes: u64) -> Result<usize> {
    use std::io::Read;

    std::fs::create_dir_all(dest)
        .with_context(|| format!("failed to create extraction directory: {}", dest.display()))?;

    let file = std::fs::File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut limited = decoder.take(max_bytes + 1);

    let stem = archive_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    // Sanitize: strip any directory components so a name like `../../evil` cannot
    // escape the destination directory.
    let safe_stem = Path::new(stem)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let out_path = dest.join(safe_stem);

    let mut out_file = std::fs::File::create(&out_path)?;
    let written = std::io::copy(&mut limited, &mut out_file)?;
    if written > max_bytes {
        anyhow::bail!("gz file exceeds maximum decompressed size limit of {max_bytes} bytes");
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn archive_extraction_cases() {
        let dir = TempDir::new().unwrap();

        let real_gz = dir.path().join("evil.gz");
        let file = std::fs::File::create(&real_gz).unwrap();
        let mut enc = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        std::io::Write::write_all(&mut enc, b"pwned\n").unwrap();
        enc.finish().unwrap();

        let dest = TempDir::new().unwrap();
        let count = extract_gz_single(&real_gz, dest.path(), 1024 * 1024).unwrap();
        assert_eq!(count, 1);
        let out = dest.path().join("evil");
        assert!(out.exists(), "expected decompressed file at {out:?}");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "pwned\n");

        let gz_path = dir.path().join("hello.gz");
        let file = std::fs::File::create(&gz_path).unwrap();
        let mut enc = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        std::io::Write::write_all(&mut enc, b"hello\n").unwrap();
        enc.finish().unwrap();

        let dest = dir.path().join("nested").join("output");
        assert!(!dest.exists(), "dest should not exist before extraction");
        let count = extract_gz_single(&gz_path, &dest, 1024 * 1024).unwrap();
        assert_eq!(count, 1);
        assert!(dest.join("hello").exists());

        let bz2_path = dir.path().join("docs.tar.bz2");
        let file = std::fs::File::create(&bz2_path).unwrap();
        let enc = bzip2::write::BzEncoder::new(file, bzip2::Compression::fast());
        let mut ar = tar::Builder::new(enc);
        let body = b"# Bzip2 Doc\n\nContent compressed with bzip2.\n";
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("docs/bzip2.md").unwrap();
        hdr.set_size(body.len() as u64);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        ar.append(&hdr, &body[..]).unwrap();
        ar.into_inner().unwrap().finish().unwrap();
        let dest_bz2 = TempDir::new().unwrap();
        let count = extract_tar_compressed(
            &bz2_path,
            dest_bz2.path(),
            100,
            1024 * 1024,
            TarCompression::Bz2,
        )
        .unwrap();
        assert_eq!(count, 1);
        let extracted = dest_bz2.path().join("docs/bzip2.md");
        assert!(
            extracted.exists(),
            "expected extracted file at {extracted:?}"
        );
        assert!(
            std::fs::read_to_string(&extracted)
                .unwrap()
                .contains("bzip2")
        );
    }
}
