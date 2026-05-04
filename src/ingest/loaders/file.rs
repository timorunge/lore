use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result};
use globset::Glob;
use ignore::WalkBuilder;
use tracing::warn;

use crate::config::{ContentFilterConfig, ExtractMode, ProcessingLimits};
use crate::fmt::format_bytes;
use crate::ingest::metadata::{self, ExtractedMeta};
use crate::ingest::types::LoaderResult;
use crate::types::{DocKind, SourceType};
use crate::util::platform::suppress_native_stderr;

/// Known source-code file extensions.
pub const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "go", "java", "c", "cpp", "h", "hpp", "cs", "rb", "sh", "bash", "zsh",
    "swift", "kt", "scala", "zig", "lua", "pl", "r", "m", "php", "ex", "erl", "hs", "ml", "clj",
    "v", "sv", "vhd", "s", "asm", "sql", "proto", "thrift", "graphql", "tf", "nix", "jsx", "tsx",
    "vue", "svelte", "css", "scss", "less", "sass",
];

/// Known structured data file extensions.
pub const DATA_EXTENSIONS: &[&str] = &[
    "json",
    "yaml",
    "yml",
    "toml",
    "ini",
    "properties",
    "csv",
    "tsv",
    "xml",
    "xsd",
    "wsdl",
    "avro",
    "parquet",
];

/// Known email file extensions.
pub const EMAIL_EXTENSIONS: &[&str] = &["eml", "mbox", "pst"];

/// Build kreuzberg metadata with detected-language fallback.
fn kreuzberg_meta(result: &kreuzberg::ExtractionResult) -> ExtractedMeta {
    let mut meta = metadata::from_kreuzberg(&result.metadata);
    if meta.lang.is_none()
        && let Some(langs) = &result.detected_languages
        && let Some(first) = langs.first()
    {
        meta.lang = crate::util::normalize_language(first);
    }
    meta
}

/// Extract metadata from a kreuzberg result, respecting the extraction mode.
fn extraction_meta(result: &kreuzberg::ExtractionResult, extract: ExtractMode) -> ExtractedMeta {
    match extract {
        ExtractMode::Auto => {
            let binary_meta = kreuzberg_meta(result);
            let text_meta = metadata::extract_metadata(&result.content);
            metadata::merge(binary_meta, text_meta)
        }
        ExtractMode::Builtin => metadata::extract_metadata(&result.content),
        ExtractMode::Kreuzberg => kreuzberg_meta(result),
        ExtractMode::None => ExtractedMeta::default(),
    }
}

/// Build a kreuzberg `ExtractionConfig` with OCR enabled only when the `ocr` feature is active.
fn extraction_config(
    content_filter: ContentFilterConfig,
    cancel_token: kreuzberg::CancellationToken,
) -> kreuzberg::ExtractionConfig {
    let kz_filter = kreuzberg::ContentFilterConfig {
        include_headers: content_filter.include_headers,
        include_footers: content_filter.include_footers,
        strip_repeating_text: content_filter.strip_repeating_text,
        include_watermarks: content_filter.include_watermarks,
    };

    kreuzberg::ExtractionConfig {
        #[cfg(feature = "ocr")]
        ocr: Some(kreuzberg::OcrConfig::default()),
        #[cfg(not(feature = "ocr"))]
        ocr: None,
        output_format: kreuzberg::OutputFormat::Markdown,
        content_filter: Some(kz_filter),
        cancel_token: Some(cancel_token),
        ..kreuzberg::ExtractionConfig::default()
    }
}

/// Heuristically determine whether the file should be read as plain UTF-8 text.
fn is_text_file(path: &Path, bytes: &[u8], extra_extensions: Option<&[String]>) -> bool {
    if let Some(extras) = extra_extensions
        && let Some(ext) = path.extension().and_then(|e| e.to_str())
    {
        let ext_lower = ext.to_lowercase();
        if extras.iter().any(|e| e.eq_ignore_ascii_case(&ext_lower)) {
            return true;
        }
    }

    // Probe up to 32 KB for valid UTF-8 with no null bytes (binary indicator).
    // Walk backwards up to 3 bytes to avoid splitting a multibyte UTF-8 sequence.
    let mut end = bytes.len().min(32 * 1024);
    while end > 0 && end < bytes.len() && bytes[end] & 0b1100_0000 == 0b1000_0000 {
        end -= 1;
    }
    let probe = &bytes[..end];
    std::str::from_utf8(probe).is_ok() && !probe.contains(&0)
}

/// Override map for extensions where mime_guess returns the wrong type
/// or a type kreuzberg doesn't recognize.
fn mime_override(ext: &str) -> Option<&'static str> {
    match ext {
        // mime_guess returns application/vnd.lotus-organizer (wrong)
        "org" => Some("text/x-org"),
        // mime_guess returns application/x-tex; kreuzberg wants application/x-latex
        "tex" => Some("application/x-latex"),
        // Jupyter notebooks: mime_guess doesn't know .ipynb
        "ipynb" => Some("application/x-ipynb+json"),
        // BibTeX: mime_guess doesn't know .bib
        "bib" => Some("application/x-bibtex"),
        // FictionBook: mime_guess doesn't know .fb2
        "fb2" => Some("application/x-fictionbook+xml"),
        // OPML: mime_guess returns text/x-opml; kreuzberg wants application/xml+opml
        "opml" => Some("application/xml+opml"),
        // PST: mime_guess returns application/vnd.ms-outlook (MSG type)
        "pst" => Some("application/vnd.ms-outlook-pst"),
        _ => None,
    }
}

/// Resolve the MIME type for a path using extension overrides then `mime_guess`.
fn resolve_mime_type(path: &Path) -> Option<String> {
    let ext = path.extension().and_then(|e| e.to_str())?;
    if let Some(override_mime) = mime_override(&ext.to_lowercase()) {
        return Some(override_mime.to_owned());
    }
    mime_guess::from_ext(ext)
        .first()
        .map(|m| m.essence_str().to_owned())
}

/// Return `true` for `text/*` MIME types where kreuzberg extracts better than raw UTF-8 reading.
fn kreuzberg_handles_text_mime(mime: &str) -> bool {
    matches!(
        mime,
        "text/html"
            | "text/xml"
            | "text/csv"
            | "text/tab-separated-values"
            | "text/x-rst"
            | "text/x-org"
            | "text/x-tex"
    )
}

/// Auto-detect `DocKind` from MIME type and file extension.
fn detect_doc_kind(mime: Option<&str>, ext: Option<&str>) -> DocKind {
    // Extension is more reliable than MIME for code and data files.
    if let Some(ext) = ext {
        let ext_lower = ext.to_lowercase();
        if CODE_EXTENSIONS.contains(&ext_lower.as_str()) {
            return DocKind::Code;
        }
        if DATA_EXTENSIONS.contains(&ext_lower.as_str()) {
            return DocKind::Data;
        }
        if EMAIL_EXTENSIONS.contains(&ext_lower.as_str()) {
            return DocKind::Email;
        }
    }
    if let Some(mt) = mime {
        if mt.starts_with("text/x-") || mt.contains("x-source") || mt.contains("x-script") {
            return DocKind::Code;
        }
        if mt == "text/csv"
            || mt == "text/tab-separated-values"
            || mt == "application/json"
            || mt.contains("yaml")
            || mt.contains("xml") && !mt.contains("html")
        {
            return DocKind::Data;
        }
        if mt == "message/rfc822" || mt.contains("mbox") || mt.contains("outlook-pst") {
            return DocKind::Email;
        }
    }
    DocKind::Document
}

/// Extract the normalized file format (lowercase extension without dot).
fn detect_format(path: &Path, mime: Option<&str>) -> Option<String> {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        return Some(ext.to_lowercase());
    }
    if let Some(mt) = mime {
        let fmt = match mt {
            "text/html" => "html",
            "text/plain" => "txt",
            "text/csv" => "csv",
            "text/xml" => "xml",
            "application/json" => "json",
            "application/pdf" => "pdf",
            m if m.contains("markdown") => "md",
            _ => "",
        };
        if !fmt.is_empty() {
            return Some(fmt.to_owned());
        }
    }
    None
}

/// Walk `base` for files matching `pattern` (gitignore-aware), capped at `max` entries.
pub async fn list_files(base: &Path, pattern: &str, max: Option<usize>) -> Result<Vec<PathBuf>> {
    let base = base.to_path_buf();
    let pattern = pattern.to_owned();
    tokio::task::spawn_blocking(move || list_files_sync(&base, &pattern, max))
        .await
        .context("list_files panicked")?
}

/// Synchronous file walk used by `list_files`; runs in a blocking thread.
fn list_files_sync(base: &Path, pattern: &str, max: Option<usize>) -> Result<Vec<PathBuf>> {
    let glob_matcher = Glob::new(pattern)
        .with_context(|| format!("invalid glob pattern: {pattern}"))?
        .compile_matcher();

    let mut files = Vec::new();

    for entry in WalkBuilder::new(base)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .follow_links(false)
        .sort_by_file_name(std::cmp::Ord::cmp)
        .build()
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                warn!("walk error: {e}");
                continue;
            }
        };

        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }

        let Ok(rel) = entry.path().strip_prefix(base) else {
            continue;
        };

        if glob_matcher.is_match(rel) {
            files.push(entry.into_path());
            if let Some(m) = max
                && files.len() >= m
            {
                break;
            }
        }
    }

    if max.is_none() {
        files.sort();
    }

    Ok(files)
}

/// Read a single file, extract its text via kreuzberg or raw UTF-8 fallback, and return a `LoaderResult`.
pub async fn read_file(
    path: &Path,
    topic: Option<&str>,
    text_extensions: Option<&[String]>,
    limits: &ProcessingLimits,
    extract: ExtractMode,
    content_type_hint: Option<&str>,
) -> Result<LoaderResult> {
    let meta = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat: {}", path.display()))?;

    anyhow::ensure!(
        meta.len() <= limits.max_file_bytes,
        "file too large ({}, max {}): {}",
        format_bytes(meta.len()),
        format_bytes(limits.max_file_bytes),
        path.display(),
    );

    let raw_bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read: {}", path.display()))?;
    // Strip UTF-8 BOM before kreuzberg sees the bytes.
    let bytes = raw_bytes
        .strip_prefix(b"\xEF\xBB\xBF")
        .unwrap_or(&raw_bytes);

    let cancel_token = kreuzberg::CancellationToken::new();
    let config = extraction_config(limits.content_filter, cancel_token.clone());

    let mime_type: Option<String> = content_type_hint
        .map(|m| m.split(';').next().unwrap_or(m).trim().to_owned())
        .or_else(|| resolve_mime_type(path));

    // Route to kreuzberg or plain text. Kreuzberg handles binary formats and
    // some text-prefixed formats (HTML, XML, CSV, RST) better than raw UTF-8.
    let is_plain_text = match mime_type.as_deref() {
        Some(mt) if kreuzberg_handles_text_mime(mt) => false,
        Some(mt) if mt.starts_with("text/") => true,
        None => is_text_file(path, bytes, text_extensions),
        _ => false,
    };

    let (content, doc_meta) = if is_plain_text {
        let text = match std::str::from_utf8(bytes) {
            Ok(s) => s.to_owned(),
            Err(_) => String::from_utf8_lossy(bytes).into_owned(),
        };
        let doc_meta = match extract {
            ExtractMode::Auto | ExtractMode::Builtin => metadata::extract_metadata(&text),
            ExtractMode::Kreuzberg | ExtractMode::None => ExtractedMeta::default(),
        };
        (text, doc_meta)
    } else {
        // Binary format -- use kreuzberg. If we have a MIME type, pass bytes
        // directly (avoids re-reading from disk). Otherwise let kreuzberg detect.
        // Suppress stderr: pdfium/tesseract C libraries write noise directly to
        // fd 2. Wrapped in tokio::task::spawn to isolate panics from upstream
        // dependencies (e.g. html-to-markdown-rs char boundary bugs).
        let timeout = Duration::from_secs(limits.extraction_timeout_secs);
        let bytes = bytes.to_vec();
        let mt = mime_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_owned());
        let task = tokio::task::spawn(async move {
            let guard = suppress_native_stderr();
            let result =
                tokio::time::timeout(timeout, kreuzberg::extract_bytes(&bytes, &mt, &config)).await;
            drop(guard);
            result
        });
        match task.await {
            Ok(Ok(Ok(result))) => {
                let doc_meta = extraction_meta(&result, extract);
                (result.content, doc_meta)
            }
            Ok(Ok(Err(e))) => {
                anyhow::bail!("extraction failed for {}: {e}", path.display());
            }
            Ok(Err(_)) => {
                cancel_token.cancel();
                anyhow::bail!("extraction timed out for {}", path.display());
            }
            Err(e) => {
                anyhow::bail!("extraction panicked for {}: {e}", path.display());
            }
        }
    };

    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_nanos()).ok());

    let ext = path.extension().and_then(|e| e.to_str());
    let doc_kind = detect_doc_kind(mime_type.as_deref(), ext);
    let format = detect_format(path, mime_type.as_deref());

    let source = path.to_string_lossy().into_owned();
    Ok(LoaderResult {
        source_id: crate::types::source_id(&source),
        source,
        origin: SourceType::Local,
        kind: doc_kind,
        content,
        unchanged: false,
        format,
        topic: topic.map(str::to_owned).or(doc_meta.topic),
        title: if doc_kind == DocKind::Document {
            doc_meta.title
        } else {
            None
        },
        author: doc_meta.author,
        lang: doc_meta.lang,
        created_at: doc_meta.created,
        tags: doc_meta.tags,
        mtime_ns,
        size_bytes: Some(meta.len() as i64),
        etag: None,
        last_modified: None,
        content_hash_override: None,
    })
}
