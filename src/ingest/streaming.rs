use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use futures::stream::{FuturesUnordered, StreamExt};
#[cfg(feature = "llm")]
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::config::{ExtractMode, SourceConfig};
use crate::ingest::chunker::{ChunkAttrs, chunk_markdown_content};
use crate::ingest::discover::load::{LoadContext, load_file_or_archive, process_download};
use crate::ingest::discover::{DiscoveredSource, SourceItems};
use crate::ingest::pipeline::{
    PeriodicCommit, ProcessContext, build_document_meta, build_stamps_meta, effective_hash,
    is_unchanged,
};
use crate::ingest::transforms::CompiledProfile;
use crate::ingest::types::{FailedDoc, LoaderResult};
#[cfg(feature = "llm")]
use crate::llm::enrich_document;
use crate::net::{FetchRequest, Fetcher};
use crate::types::{Chunk, SourceId, StampsMeta};
use crate::util::progress::ProgressHandle;
use crate::util::relativize_path;

const URL_DOWNLOAD_BATCH: usize = 50;

struct LoadedDoc {
    doc: LoaderResult,
}

enum ProcessedDoc {
    Unchanged,
    Marker {
        doc: LoaderResult,
        content_hash: String,
    },
    Chunked {
        doc: LoaderResult,
        chunks: Vec<Chunk>,
        content_hash: String,
        doc_summary: Option<String>,
    },
    Failed,
}

/// Three-stage streaming pipeline: load -> process -> write.
///
/// The pipeline uses bounded channels that keep all stages saturated
/// continuously. Backpressure propagates naturally: if writing is slow,
/// the process channel fills, which blocks loading.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_streaming_pipeline(
    discovered: DiscoveredSource,
    ctx: &ProcessContext,
    compiled: &CompiledProfile,
    fetcher: &Fetcher,
    observer: &dyn crate::ingest::IngestObserver,
    fetch_token: &ProgressHandle,
    enrich_token: &ProgressHandle,
    index_token: &ProgressHandle,
    doc_sources: &mut HashSet<SourceId>,
    discovered_total: usize,
    config_tags: Option<&str>,
) -> Result<(u64, u64, Vec<FailedDoc>)> {
    let concurrency = ctx.config.processing.concurrency;
    let load_capacity = concurrency * 2;
    let write_capacity = concurrency * 2;

    let (load_tx, load_rx) = mpsc::channel::<LoadedDoc>(load_capacity);
    let (proc_tx, proc_rx) = mpsc::channel::<ProcessedDoc>(write_capacity);

    let shutdown = observer.shutdown_flag();
    let commit_interval = Duration::from_secs(ctx.config.processing.commit_interval);

    let config_tags_owned = config_tags.map(str::to_owned);
    let (load_result, proc_result, write_result) = tokio::join!(
        load_task(
            discovered.items,
            discovered.extras,
            fetcher,
            concurrency,
            compiled.extract,
            fetch_token,
            &ctx.existing_stamps,
            ctx.force,
            config_tags_owned.as_deref(),
            load_tx,
            shutdown,
        ),
        process_task(load_rx, proc_tx, compiled, ctx, enrich_token),
        write_task(
            proc_rx,
            ctx,
            index_token,
            observer,
            commit_interval,
            &ctx.config.sources,
            Some(discovered_total),
        ),
    );

    proc_result?;

    let (seen_ids, mut load_failures) = load_result;
    doc_sources.extend(seen_ids);

    let (docs, chunks, write_failures) = write_result;
    let mut all_failures = discovered.load_failures;
    all_failures.append(&mut load_failures);
    all_failures.extend(write_failures);

    Ok((docs, chunks, all_failures))
}

fn merge_config_tags(doc: &mut LoaderResult, config_tags: Option<&str>) {
    let Some(ct) = config_tags else { return };
    doc.tags = Some(match doc.tags.take() {
        Some(existing) => format!("{existing}, {ct}"),
        None => ct.to_owned(),
    });
}

/// Stage 1: read files / download URLs and send each doc into the load channel.
#[allow(clippy::too_many_arguments)]
async fn load_task(
    items: SourceItems,
    extras: Vec<LoaderResult>,
    fetcher: &Fetcher,
    concurrency: usize,
    extract: ExtractMode,
    fetch_progress: &ProgressHandle,
    existing_stamps: &Arc<HashMap<SourceId, StampsMeta>>,
    force: bool,
    config_tags: Option<&str>,
    tx: mpsc::Sender<LoadedDoc>,
    shutdown: &AtomicBool,
) -> (HashSet<SourceId>, Vec<FailedDoc>) {
    let mut seen = HashSet::new();
    let mut load_failures: Vec<FailedDoc> = Vec::new();

    match items {
        SourceItems::Files {
            paths,
            topic,
            text_extensions,
            limits,
            rewrite_base,
            source_prefix,
            stamp,
        } => {
            let empty: Arc<HashMap<SourceId, StampsMeta>> = Arc::new(HashMap::new());
            let stamps = if force {
                empty
            } else {
                existing_stamps.clone()
            };
            let ctx = Arc::new(LoadContext {
                limits,
                extract,
                fetch_pb: ProgressHandle::noop(),
                existing_stamps: stamps,
                rewrite_base: rewrite_base.clone(),
                topic: topic.clone(),
                text_ext: text_extensions.clone(),
            });

            let mut stream = futures::stream::iter(paths.into_iter().map(|path| {
                let ctx = ctx.clone();
                let display = path.to_string_lossy().into_owned();
                async move { (display, load_file_or_archive(&path, &ctx).await) }
            }))
            .buffer_unordered(concurrency);

            while let Some((path_display, result)) = stream.next().await {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                match result {
                    Ok((mut docs, archive_failures)) => {
                        load_failures.extend(archive_failures);
                        for doc in &mut docs {
                            let rel = if let Some((archive, entry)) = doc.source.split_once('#') {
                                let rewritten = relativize_path(Path::new(archive), &rewrite_base);
                                format!("{rewritten}#{entry}")
                            } else {
                                relativize_path(Path::new(&doc.source), &rewrite_base)
                            };
                            doc.source = match &source_prefix {
                                Some(pfx) => format!("{pfx}#{rel}"),
                                None => rel,
                            };
                            doc.source_id = crate::types::source_id(&doc.source);
                        }
                        if let Some((origin, ref etag, ref last_modified)) = stamp {
                            for doc in &mut docs {
                                doc.origin = origin;
                                doc.etag.clone_from(etag);
                                doc.last_modified.clone_from(last_modified);
                                doc.mtime_ns = None;
                                doc.size_bytes = None;
                            }
                        }
                        for mut doc in docs {
                            seen.insert(doc.source_id.clone());
                            if !doc.unchanged {
                                fetch_progress.inc(1);
                            }
                            merge_config_tags(&mut doc, config_tags);
                            if tx.send(LoadedDoc { doc }).await.is_err() {
                                return (seen, load_failures);
                            }
                        }
                    }
                    Err(e) => {
                        load_failures.push(FailedDoc::new(path_display, format!("{e}")));
                        warn!("failed to read file: {e}");
                    }
                }
            }
        }

        SourceItems::Urls {
            urls,
            source_type,
            topic,
            etags,
            limits,
            headers,
        } => {
            for chunk in urls.chunks(URL_DOWNLOAD_BATCH) {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                let requests: Vec<FetchRequest> = chunk
                    .iter()
                    .map(|url| {
                        let (etag, lm) = etags
                            .get(url.as_str())
                            .map_or((None, None), |(e, l)| (e.as_deref(), l.as_deref()));
                        FetchRequest {
                            url,
                            etag,
                            last_modified: lm,
                            extra_headers: (!headers.is_empty()).then_some(&headers),
                        }
                    })
                    .collect();

                let downloads = fetcher.try_download_all(&requests, fetch_progress).await;

                for (url, dl) in &downloads {
                    match process_download(url, dl, topic.as_deref(), source_type, limits, extract)
                        .await
                    {
                        Ok((docs, archive_failures)) => {
                            load_failures.extend(archive_failures);
                            for mut doc in docs {
                                seen.insert(doc.source_id.clone());
                                merge_config_tags(&mut doc, config_tags);
                                if tx.send(LoadedDoc { doc }).await.is_err() {
                                    return (seen, load_failures);
                                }
                            }
                        }
                        Err(e) => {
                            load_failures.push(FailedDoc::new(url.as_str(), format!("{e}")));
                            warn!(url = url.as_str(), "failed to process download: {e}");
                        }
                    }
                }
            }
        }

        SourceItems::Preloaded(docs) => {
            for mut doc in docs {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                seen.insert(doc.source_id.clone());
                if !doc.unchanged {
                    fetch_progress.inc(1);
                }
                merge_config_tags(&mut doc, config_tags);
                if tx.send(LoadedDoc { doc }).await.is_err() {
                    return (seen, load_failures);
                }
            }
        }
    }

    for mut doc in extras {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        seen.insert(doc.source_id.clone());
        fetch_progress.inc(1);
        merge_config_tags(&mut doc, config_tags);
        if tx.send(LoadedDoc { doc }).await.is_err() {
            return (seen, load_failures);
        }
    }

    (seen, load_failures)
}

/// Bundled context for chunk + enrich work, passed into each async task.
struct ChunkEnrichCtx {
    doc: LoaderResult,
    profile: Arc<CompiledProfile>,
    enrich_pb: ProgressHandle,
    #[cfg(feature = "llm")]
    config: Arc<crate::config::IngestConfig>,
    #[cfg(feature = "llm")]
    enrich_concurrency: usize,
    #[cfg(feature = "llm")]
    sem: Arc<Semaphore>,
    #[cfg(feature = "llm")]
    llm_client: Option<Arc<crate::llm::LlmClient>>,
}

/// Chunk a single document and optionally run LLM enrichment.
///
/// All CPU-bound work (transforms, SHA-256 hash, chunking) runs on the
/// blocking thread pool so the async task only coordinates results.
async fn chunk_and_enrich(cx: ChunkEnrichCtx) -> ProcessedDoc {
    let ChunkEnrichCtx {
        mut doc,
        profile,
        enrich_pb,
        #[cfg(feature = "llm")]
        config,
        #[cfg(feature = "llm")]
        enrich_concurrency,
        #[cfg(feature = "llm")]
        sem,
        #[cfg(feature = "llm")]
        llm_client,
    } = cx;

    enrich_pb.set_prefix(&doc.source);

    let result = tokio::task::spawn_blocking(move || {
        profile.apply_pipeline(&mut doc);
        let hash = effective_hash(&doc);
        let content = std::mem::take(&mut doc.content);
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
            &content,
            &attrs,
            profile.drop_sections(),
            profile.max_chunk_chars,
            profile.min_chunk_chars,
        );
        let preamble = crate::util::truncate_str_ref(&content, 4000).to_owned();
        drop(content);
        (doc, preamble, hash, chunks)
    })
    .await;

    #[allow(unused_mut, unused_variables)]
    let (doc, preamble, hash, mut chunks) = match result {
        Ok(t) => t,
        Err(e) => {
            warn!("chunk task panicked: {e}");
            enrich_pb.inc(1);
            return ProcessedDoc::Failed;
        }
    };

    #[cfg(feature = "llm")]
    let doc_summary: Option<String> = if let Some(ref client) = llm_client {
        let enrichment = enrich_document(
            &mut chunks,
            &preamble,
            doc.topic.is_some(),
            client,
            &config,
            &sem,
            enrich_concurrency,
        )
        .await;
        enrichment.doc_summary
    } else {
        None
    };
    #[cfg(not(feature = "llm"))]
    let doc_summary: Option<String> = None;

    enrich_pb.inc(1);

    ProcessedDoc::Chunked {
        doc,
        chunks,
        content_hash: hash,
        doc_summary,
    }
}

/// Stage 2: apply transforms, chunk, and optionally enrich each loaded doc.
async fn process_task(
    mut rx: mpsc::Receiver<LoadedDoc>,
    tx: mpsc::Sender<ProcessedDoc>,
    profile: &CompiledProfile,
    ctx: &ProcessContext,
    enrich_progress: &ProgressHandle,
) -> Result<()> {
    let max_chunkers = ctx.config.processing.concurrency;
    #[cfg(feature = "llm")]
    let enrich_concurrency = ctx
        .config
        .llm
        .as_ref()
        .map_or(4, |c| c.enrich_chunks.concurrency);
    #[cfg(feature = "llm")]
    let enrich_semaphore = Arc::new(Semaphore::new(enrich_concurrency));
    #[cfg(feature = "llm")]
    let stream_concurrency = max_chunkers.max(enrich_concurrency);
    #[cfg(not(feature = "llm"))]
    let stream_concurrency = max_chunkers;

    let profile = Arc::new(profile.clone());

    let mut in_flight = FuturesUnordered::new();
    let mut rx_done = false;

    loop {
        tokio::select! {
            biased;
            Some(result) = in_flight.next(), if !in_flight.is_empty() => {
                if tx.send(result).await.is_err() {
                    return Ok(());
                }
            }
            loaded = rx.recv(), if !rx_done && in_flight.len() < stream_concurrency => {
                let Some(loaded) = loaded else {
                    rx_done = true;
                    continue;
                };
                let doc = loaded.doc;

                if !ctx.force && doc.unchanged {
                    enrich_progress.inc(1);
                    if tx.send(ProcessedDoc::Unchanged).await.is_err() {
                        return Ok(());
                    }
                    continue;
                }
                if !ctx.force
                    && !ctx.existing_stamps.is_empty()
                    && is_unchanged(&doc, &ctx.existing_stamps)
                {
                    debug!(source = doc.source.as_str(), "unchanged, skipping");
                    enrich_progress.inc(1);
                    if tx.send(ProcessedDoc::Unchanged).await.is_err() {
                        return Ok(());
                    }
                    continue;
                }

                if doc.content.is_empty() && doc.content_hash_override.is_some() {
                    let hash = effective_hash(&doc);
                    enrich_progress.inc(1);
                    if tx.send(ProcessedDoc::Marker { doc, content_hash: hash }).await.is_err() {
                        return Ok(());
                    }
                    continue;
                }

                in_flight.push(chunk_and_enrich(ChunkEnrichCtx {
                    doc,
                    profile: profile.clone(),
                    enrich_pb: enrich_progress.clone(),
                    #[cfg(feature = "llm")]
                    config: ctx.config.clone(),
                    #[cfg(feature = "llm")]
                    enrich_concurrency,
                    #[cfg(feature = "llm")]
                    sem: enrich_semaphore.clone(),
                    #[cfg(feature = "llm")]
                    llm_client: ctx.llm_client.clone(),
                }));
            }
            else => break,
        }
    }

    while let Some(result) = in_flight.next().await {
        if tx.send(result).await.is_err() {
            break;
        }
    }

    Ok(())
}

/// Stage 3: persist processed docs to the store with periodic commits.
async fn write_task(
    rx: mpsc::Receiver<ProcessedDoc>,
    ctx: &ProcessContext,
    index_progress: &ProgressHandle,
    observer: &dyn crate::ingest::IngestObserver,
    commit_interval: Duration,
    sources: &[SourceConfig],
    discovered_total: Option<usize>,
) -> (u64, u64, Vec<FailedDoc>) {
    let mut periodic = PeriodicCommit {
        store: ctx.store.as_ref(),
        interval: commit_interval,
        last: Instant::now(),
        observer,
        sources,
        discovered_total,
    };

    let mut total_docs = 0u64;
    let mut total_chunks = 0u64;
    let mut total_failed: Vec<FailedDoc> = Vec::new();

    let mut rx = rx;
    while let Some(processed) = rx.recv().await {
        match processed {
            ProcessedDoc::Unchanged => {}
            ProcessedDoc::Marker {
                mut doc,
                content_hash,
            } => {
                if !ctx.dry_run {
                    let sid = doc.source_id.clone();
                    let meta = build_document_meta(&mut doc, &[], None);
                    let fm = build_stamps_meta(&mut doc, content_hash);
                    ctx.store.upsert_document(meta);
                    ctx.store.upsert_stamps(sid, fm);
                }
                index_progress.inc(1);
            }
            ProcessedDoc::Chunked {
                mut doc,
                chunks,
                content_hash,
                doc_summary,
            } => {
                if chunks.is_empty() {
                    warn!(
                        source = doc.source.as_str(),
                        "document produced zero chunks after processing"
                    );
                }
                let source_display = doc.source.clone();
                let topic_display = doc.topic.clone();
                let sid = doc.source_id.clone();
                if !ctx.dry_run {
                    let meta = build_document_meta(&mut doc, &chunks, doc_summary);
                    let fm = build_stamps_meta(&mut doc, content_hash);
                    if let Err(e) = ctx.store.replace_document(&sid, &chunks, meta) {
                        warn!(source = source_display.as_str(), "store write failed: {e}");
                        total_failed.push(FailedDoc::new(
                            &source_display,
                            format!("store write failed: {e}"),
                        ));
                        index_progress.inc(1);
                        continue;
                    }
                    ctx.store.upsert_stamps(sid, fm);
                    periodic.maybe_commit();
                }
                total_docs += 1;
                total_chunks += chunks.len() as u64;
                observer.on_document_indexed(&source_display, chunks.len() as u64);
                index_progress.set_prefix(&source_display);
                index_progress.inc(1);
                info!(
                    source = source_display.as_str(),
                    topic = topic_display.as_deref().unwrap_or(""),
                    chunks = chunks.len(),
                    "processed document"
                );
            }
            ProcessedDoc::Failed => {
                total_failed.push(FailedDoc::new("unknown", "chunk/enrich task panicked"));
                index_progress.inc(1);
            }
        }
    }

    (total_docs, total_chunks, total_failed)
}
