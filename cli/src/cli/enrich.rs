use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use indicatif::MultiProgress;
use tokio::sync::Semaphore;

use lore::config::{IngestConfig, LlmConfig};
use lore::fmt::{format_bytes, format_elapsed, plural};
use lore::llm::{LlmClient, enrich_document};
use lore::store::Store;
use lore::util::platform;
use lore::util::progress::ProgressHandle;

use crate::cli::LinePrefix;
use crate::progress::{self, SourceBars};
use crate::terminal;

/// Re-run LLM enrichment on indexed documents without re-fetching content.
pub async fn enrich(
    config: &IngestConfig,
    config_path: &Path,
    source_filter: Option<&str>,
    topic_filter: Option<&str>,
    force: bool,
    prefix: &LinePrefix,
) -> Result<()> {
    let llm_config = config.llm.as_ref().context("no llm block in config")?;
    ensure!(
        llm_config.has_enrichment(),
        "all llm enrichment features are disabled"
    );

    let store_dir = config.store_dir(config_path);
    if !store_dir.is_dir() {
        bail!("knowledge base not found; run `lore ingest` first");
    }

    let paint = terminal::stderr_painter();
    let mp = MultiProgress::new();
    let pfx = prefix.to_string();

    let mut features: Vec<&str> = Vec::new();
    if llm_config.detect_topics.enabled {
        features.push("topics");
    }
    if llm_config.summarize_docs.enabled {
        features.push("summaries");
    }
    if llm_config.enrich_chunks.enabled {
        features.push("chunks");
    }
    let feature_str = if features.is_empty() {
        String::new()
    } else {
        format!(", features: {}", features.join("+"))
    };

    progress::mp_println(
        &mp,
        format!("{prefix}[{} ] enriching{feature_str}", paint.purple(".")),
    );

    let store = Store::open(
        &store_dir,
        config.store.phrase_search,
        config.store.writer_heap_mb,
        config.store.language,
        config.store.doc_store_cache_blocks,
    )
    .context("failed to open store")?;
    let llm_client = LlmClient::new(llm_config)?;

    let step = progress::add_step(&mp, &pfx, "scanning for candidates...");
    let mut candidates =
        find_enrichment_candidates(&store, source_filter, topic_filter, force, llm_config);
    candidates.sort_unstable();
    let n = candidates.len();
    progress::finish_step(
        &mp,
        &step,
        &pfx,
        &format!("{n} document{} to enrich", plural(n)),
    );

    if candidates.is_empty() {
        return Ok(());
    }

    let total = n as u64;
    let _stdin_guard = lore::util::platform::SuppressStdin::new();

    let term_width = terminal::output_width();
    let bars = SourceBars::new(mp.clone(), term_width);
    let label = format!("enriching {n} document{}", plural(n));
    bars.add_active(&label);
    let enrich_pb = bars.create_progress(0, "enriching");
    enrich_pb.set_length(total);

    let shutdown = Arc::new(AtomicBool::new(false));
    crate::cli::spawn_signal_handlers(&shutdown, &mp);

    let enrich_start = Instant::now();

    let (enriched, total_chunks) = process_enrichment_batch(
        &candidates,
        &store,
        &llm_client,
        config,
        llm_config,
        &shutdown,
        &enrich_pb,
    )
    .await?;

    let elapsed = enrich_start.elapsed();
    let interrupted = shutdown.load(Ordering::SeqCst);

    store.commit().context("failed to commit store")?;

    bars.finish_and_clear_all();

    if interrupted {
        progress::mp_println(
            &mp,
            format!(
                "{prefix}[{} ] interrupted after {enriched} document{}",
                paint.red("x"),
                plural(enriched),
            ),
        );
        progress::mp_println(&mp, format!("{prefix}hint: resume with `lore enrich`"));
    } else {
        progress::mp_println(
            &mp,
            format!(
                "{prefix}[{} ] enriched {enriched} document{}, {total_chunks} chunk{}",
                paint.green("+"),
                plural(enriched),
                plural(total_chunks),
            ),
        );

        let index_size = lore::util::dir_size(&store_dir);
        let index_size_str = if index_size > 0 {
            format!(" ({})", format_bytes(index_size))
        } else {
            String::new()
        };
        progress::mp_println(
            &mp,
            format!(
                "{prefix}[{} ] {enriched} document{}, {total_chunks} chunk{} in {} -> {}{}",
                paint.blue("i"),
                plural(enriched),
                plural(total_chunks),
                format_elapsed(elapsed),
                store_dir.display(),
                index_size_str,
            ),
        );
        if enriched > 0 && elapsed.as_secs() > 0 {
            let docs_per_sec = enriched as f64 / elapsed.as_secs_f64();
            let chunks_per_sec = total_chunks as f64 / elapsed.as_secs_f64();
            let (peak_rss, cpu_time) = platform::resource_usage();
            progress::mp_println(
                &mp,
                format!(
                    "{prefix}[{} ] {docs_per_sec:.0} docs/s, {chunks_per_sec:.0} chunks/s, peak mem {}, cpu {}",
                    paint.purple("."),
                    format_bytes(peak_rss),
                    format_elapsed(cpu_time),
                ),
            );
        }
    }

    Ok(())
}

/// Select documents that need enrichment based on filters and LLM config state.
fn find_enrichment_candidates(
    store: &Store,
    source_filter: Option<&str>,
    topic_filter: Option<&str>,
    force: bool,
    llm_config: &LlmConfig,
) -> Vec<String> {
    let all_docs = store.get_all_documents();
    let topic_filter_lc = topic_filter.map(str::to_lowercase);

    all_docs
        .iter()
        .filter(|(_, doc)| {
            if source_filter.is_some_and(|f| !doc.source.contains(f)) {
                return false;
            }
            if let Some(ref topic_lc) = topic_filter_lc {
                let matches = doc
                    .topic
                    .as_deref()
                    .is_some_and(|t| t.to_lowercase().contains(topic_lc.as_str()));
                if !matches {
                    return false;
                }
            }
            if !force {
                let needs_topics = llm_config.detect_topics.enabled && doc.topic.is_none();
                let needs_summary = llm_config.summarize_docs.enabled && doc.llm_summary.is_none();
                let needs_chunks =
                    llm_config.enrich_chunks.enabled && doc.avg_llm_quality_score.is_none();
                if !needs_topics && !needs_summary && !needs_chunks {
                    return false;
                }
            }
            true
        })
        .map(|(id, _)| id.as_str().to_owned())
        .collect()
}

/// Process each candidate document through LLM enrichment, returning counts.
async fn process_enrichment_batch(
    candidates: &[String],
    store: &Store,
    llm_client: &LlmClient,
    config: &IngestConfig,
    llm_config: &LlmConfig,
    shutdown: &AtomicBool,
    pb: &ProgressHandle,
) -> Result<(usize, usize)> {
    let concurrency = llm_config.enrich_chunks.concurrency;
    let semaphore = Semaphore::new(concurrency);
    let commit_interval = Duration::from_secs(config.processing.commit_interval);

    let mut enriched = 0usize;
    let mut total_chunks = 0usize;
    let mut last_commit = Instant::now();

    for source_id in candidates {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        let Some(detail) = store
            .get_document(source_id)
            .with_context(|| format!("failed to read document {source_id}"))?
        else {
            pb.inc(1);
            continue;
        };

        let meta = detail.meta;
        let mut chunks = detail.chunks;

        if chunks.is_empty() {
            pb.inc(1);
            continue;
        }

        let content: String = chunks
            .iter()
            .map(|c| c.body.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        let enrichment = enrich_document(
            &mut chunks,
            &content,
            meta.topic.is_some(),
            llm_client,
            config,
            &semaphore,
            concurrency,
        )
        .await;

        let mut updated_meta = meta;
        updated_meta.apply_chunks(&chunks, enrichment.doc_summary);

        total_chunks += chunks.len();
        store
            .replace_document(source_id, &chunks, updated_meta)
            .with_context(|| format!("failed to write document {source_id}"))?;

        enriched += 1;
        pb.inc(1);

        if !commit_interval.is_zero() && last_commit.elapsed() >= commit_interval {
            store.commit_index().context("periodic commit failed")?;
            last_commit = Instant::now();
        }
    }

    Ok((enriched, total_chunks))
}
