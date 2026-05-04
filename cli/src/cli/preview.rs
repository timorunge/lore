use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tracing::warn;

use lore::config::transforms::ExtractMode;
use lore::config::{IngestConfig, ProcessingLimits, SourceConfig};
use lore::ingest::chunker::{ChunkAttrs, chunk_markdown_content};
use lore::ingest::loaders;
use lore::ingest::transforms::CompiledProfile;
use lore::ingest::types::LoaderResult;
use lore::output::{self, OutputMode};
use lore::query::Pagination;
use lore::store::DocDetail;
use lore::types::{self, DocMeta};
use lore::util::relativize_path;

use crate::cli::DOC_EXTENSIONS;

/// Options for the `preview` entry point.
pub struct PreviewOptions<'a> {
    pub paths: &'a [PathBuf],
    pub config: Option<&'a IngestConfig>,
    pub pagination: Pagination,
    pub mode: OutputMode,
    pub chunks: usize,
    pub json: bool,
    pub pager: Option<&'a str>,
    pub no_pager: bool,
}

/// Shared state threaded through per-file preview calls.
struct PreviewCtx<'a> {
    cwd: PathBuf,
    mode: OutputMode,
    pagination: Pagination,
    chunks: usize,
    limits: ProcessingLimits,
    extract: ExtractMode,
    compiled: CompiledProfile,
    text_ext: Option<&'a [String]>,
}

/// Preview document processing without writing to the store.
pub async fn preview(opts: PreviewOptions<'_>) -> Result<()> {
    let processing = opts
        .config
        .map(|c| c.processing.clone())
        .unwrap_or_default();
    let profile = opts
        .config
        .map(|c| c.processing.default_profile())
        .unwrap_or_default();
    let compiled = CompiledProfile::compile(&profile)?;

    let paint = crate::terminal::stderr_painter();
    let ctx = PreviewCtx {
        cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        mode: opts.mode,
        pagination: opts.pagination,
        chunks: opts.chunks,
        limits: ProcessingLimits::from_config(&processing),
        extract: profile.extract,
        compiled,
        text_ext: processing.text_extensions.as_deref(),
    };

    let config_paths: Vec<PathBuf>;
    let effective_paths: &[PathBuf] = if opts.paths.is_empty() {
        if let Some(cfg) = opts.config {
            config_paths = cfg
                .sources
                .iter()
                .flat_map(|s| match s {
                    SourceConfig::Local(s) => s.path.iter().map(PathBuf::from).collect(),
                    _ => Vec::new(),
                })
                .collect();
            if config_paths.is_empty() {
                eprintln!(
                    "[{} ] no local sources found in config (preview only supports local paths)",
                    paint.blue("i")
                );
            }
            &config_paths
        } else {
            opts.paths
        }
    } else {
        opts.paths
    };

    let exts = DOC_EXTENSIONS.join(",");
    let dir_glob = format!("**/*.{{{exts}}}");
    let limit = ctx.pagination.limit;
    let mut shown = 0usize;
    let mut out = String::new();
    'outer: for path in effective_paths {
        if path.is_file() {
            if shown >= limit {
                break;
            }
            preview_single_file(path, &ctx, &mut out, &mut shown).await?;
        } else if path.is_dir() {
            let max = Some(limit.saturating_sub(shown));
            let files = loaders::file::list_files(path, &dir_glob, max).await?;
            for file_path in &files {
                if shown >= limit {
                    break 'outer;
                }
                preview_single_file(file_path, &ctx, &mut out, &mut shown).await?;
            }
        } else {
            eprintln!(
                "[{} ] path not found: {}",
                paint.yellow("!"),
                path.display()
            );
        }
    }

    if opts.json {
        print!("{out}");
    } else {
        let capped = lore::fmt::cap_output(&out, crate::terminal::output_width());
        let pager_cmd = crate::pager::resolve_pager(opts.pager, opts.no_pager);
        crate::pager::page_output(&capped, pager_cmd.as_deref())?;
    }
    Ok(())
}

/// Load, process, and preview a single file.
async fn preview_single_file(
    path: &Path,
    ctx: &PreviewCtx<'_>,
    buf: &mut String,
    shown: &mut usize,
) -> Result<()> {
    match loaders::file::read_file(path, None, ctx.text_ext, &ctx.limits, ctx.extract, None).await {
        Ok(mut doc) => {
            ctx.compiled.apply_pipeline(&mut doc);
            doc.source = relativize_path(Path::new(&doc.source), &ctx.cwd);
            doc.source_id = types::source_id(&doc.source);
            let detail = build_detail(&doc, &ctx.compiled);
            format_preview_doc(&detail, ctx.chunks, ctx.mode, buf, shown)?;
        }
        Err(e) => {
            warn!("failed to read {}: {e}", path.display());
        }
    }
    Ok(())
}

/// Format a single document preview into the output buffer.
fn format_preview_doc(
    detail: &DocDetail,
    chunks: usize,
    mode: OutputMode,
    buf: &mut String,
    shown: &mut usize,
) -> Result<()> {
    let formatted = output::format_document(detail, 0, chunks, false, mode, true)?;
    if *shown > 0 && mode != OutputMode::Json {
        writeln!(buf, "\n---\n")?;
    }
    buf.push_str(&formatted);
    *shown += 1;
    Ok(())
}

/// Build a DocDetail from a loaded document by chunking its content.
fn build_detail(doc: &LoaderResult, compiled: &CompiledProfile) -> DocDetail {
    let attrs = ChunkAttrs {
        source_id: doc.source_id.clone(),
        source: Arc::from(doc.source.as_str()),
        origin: doc.origin,
        kind: doc.kind,
        format: doc.format.as_deref().map(Arc::from),
        title: doc.title.as_deref().map(Arc::from),
        author: doc.author.as_deref().map(Arc::from),
        lang: doc.lang.as_deref().map(Arc::from),
        created_at: doc.created_at.as_deref().map(Arc::from),
        tags: doc.tags.as_deref().map(Arc::from),
        topic: doc.topic.as_deref().map(Arc::from),
    };
    let chunks = chunk_markdown_content(
        &doc.content,
        &attrs,
        compiled.drop_sections(),
        compiled.max_chunk_chars,
        compiled.min_chunk_chars,
    );

    let word_count: u64 = chunks
        .iter()
        .map(|c| c.body.split_whitespace().count() as u64)
        .sum();

    let meta = DocMeta {
        source_id: doc.source_id.clone(),
        source: doc.source.clone(),
        origin: doc.origin,
        kind: doc.kind,
        format: doc.format.clone(),
        topic: doc.topic.clone(),
        title: doc.title.clone(),
        author: doc.author.clone(),
        lang: doc.lang.clone(),
        tags: doc.tags.clone(),
        created_at: doc.created_at.clone(),
        updated_at: None,
        avg_llm_quality_score: None,
        llm_summary: None,
        chunk_count: chunks.len() as u64,
        word_count,
    };

    DocDetail { meta, chunks }
}
