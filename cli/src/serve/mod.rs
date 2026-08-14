mod resources;
mod tools;

use std::fmt::Write as _;
use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ListResourceTemplatesResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
    ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use tracing::info;

use lore::config::ResolvedConfig;
use lore::fmt::{format_count, format_version, plural};
use lore::net::is_loopback;
use lore::output::{format_formats, format_kinds, format_lang_summary, format_source_types};
use lore::store::{self, StoreSet};
use lore::{w, wln};

/// Maximum number of topics to include in the server instructions preview.
const TOPIC_PREVIEW_CAP: usize = 20;

const TOOL_INSTRUCTIONS: &str = "\
The knowledge base is organized as: topics > documents > chunks.

## How to search effectively
- Start with lore_info for an overview of topics, document count, and languages.
- Full-text BM25 search with English stemming: \"install\" matches \"installed\", \"installer\".
- Use lore_search for exploratory queries; lore_read_topic for systematic topic coverage (paginate with offset for large topics).
- Narrow results with: topic, author, lang, source, origin, kind, format, max_per_source.
- Sort with: sort (score/source/topic for search; name/chunks/words for topics; source/topic/title/author/chunks/words/quality for docs), reverse.
- topic and author resolve fuzzy (exact > case-insensitive > substring). source matches by substring. lang, origin, kind, format match exact (format is case-insensitive).
- Explore: lore_info -> lore_list_topics -> lore_search or lore_read_topic.
- Deep read: lore_list_docs -> lore_read_doc.";

/// MCP transport protocol.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum Transport {
    /// JSON-RPC over stdin/stdout
    #[default]
    Stdio,
    /// Streamable HTTP (includes SSE streaming)
    Http,
}

/// Shared metadata that can be refreshed after re-ingest without replacing
/// the `Store` itself.  The Tantivy reader auto-reloads via
/// `ReloadPolicy::OnCommitWithDelay`; this struct covers the computed caches
/// (store info, server instructions) that sit on top of the reader.
pub(crate) struct SharedMeta {
    pub(crate) info: store::StoreInfo,
    pub(crate) instructions: String,
    pub(crate) resources: Vec<rmcp::model::Resource>,
}

impl SharedMeta {
    /// Build shared metadata by computing store info and server instructions.
    fn from_store(stores: &StoreSet) -> Self {
        let info = stores.store_info();
        let instructions = build_instructions(&info);
        let resources = resources::build_resource_list(stores, &info);
        Self {
            info,
            instructions,
            resources,
        }
    }
}

/// MCP server that exposes a lore knowledge base.
pub(crate) struct LoreServer {
    pub(super) store: Arc<StoreSet>,
    tool_router: ToolRouter<Self>,
    meta: Arc<std::sync::RwLock<SharedMeta>>,
}

impl LoreServer {
    #[cfg(test)]
    pub(super) fn from_store(store: store::Store) -> Self {
        let stores = Arc::new(StoreSet::single(store));
        let meta = Arc::new(std::sync::RwLock::new(SharedMeta::from_store(&stores)));
        Self {
            store: stores,
            tool_router: Self::tool_router(),
            meta,
        }
    }

    /// Construct a server with a pre-built store set and shared metadata handle.
    fn from_store_with_meta(
        store: Arc<StoreSet>,
        meta: Arc<std::sync::RwLock<SharedMeta>>,
    ) -> Self {
        Self {
            store,
            tool_router: Self::tool_router(),
            meta,
        }
    }

    /// Return a snapshot of the cached store info, recovering from a poisoned lock.
    pub(super) fn cached_info(&self) -> store::StoreInfo {
        self.meta
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .info
            .clone()
    }
}

impl ServerHandler for LoreServer {
    /// Return server info with capabilities and the current knowledge-base instructions.
    fn get_info(&self) -> ServerInfo {
        let instructions = self
            .meta
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .instructions
            .clone();
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_instructions(instructions)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: self.tool_router.list_all(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        self.tool_router.get(name).cloned()
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let offset = request
            .and_then(|r| r.cursor)
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        let meta = self
            .meta
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(resources::page_resources(&meta.resources, offset))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(resources::list_resource_templates())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let info = self.cached_info();
        resources::read_resource(&self.store, &info, &request.uri).map(Into::into)
    }
}

/// Options for `run()`.
pub struct ServeOptions<'a> {
    pub configs: &'a [ResolvedConfig],
    pub transport: Transport,
    pub host: &'a str,
    pub port: u16,
    pub expose: bool,
    pub token: Option<String>,
    pub watch: bool,
    pub watch_debounce: u64,
}

/// Format a plain-text knowledge base overview optimized for LLM agent consumption.
pub(crate) fn format_store_overview(
    info: &store::StoreInfo,
    topic_overflow: impl Fn(usize) -> String,
) -> String {
    let mut out = String::new();

    if let Some(ref name) = info.name {
        out.push_str(name);
        out.push('\n');
        if let Some(ref desc) = info.description {
            out.push_str(desc);
            out.push('\n');
        }
        out.push('\n');
    }

    if !info.kinds.is_empty() {
        wln!(out, "Kinds: {}", format_kinds(&info.kinds));
    }
    if info.source_types.len() > 1 {
        wln!(out, "Sources: {}", format_source_types(&info.source_types));
    }
    if !info.formats.is_empty() {
        wln!(out, "Formats: {}", format_formats(&info.formats));
    }
    if let Some(summary) = format_lang_summary(&info.languages) {
        wln!(out, "Languages: {summary}");
    }

    wln!(out, "Documents: {}", info.documents);
    wln!(out, "Chunks: {}", info.chunks);
    wln!(out, "Words: {}", format_count(info.words));
    if info.documents > 0 {
        let avg_chunks = (info.chunks as f64 / info.documents as f64).round() as usize;
        let avg_words = (info.words as f64 / info.documents as f64).round() as usize;
        let mut avg_val = format!(
            "{avg_chunks} chunk{}, {avg_words} word{}",
            plural(avg_chunks),
            plural(avg_words)
        );
        if info.chunks > 0 {
            let avg_wpc = info.words as f64 / info.chunks as f64;
            w!(avg_val, " ({avg_wpc:.0} words/chunk)");
        }
        wln!(out, "Avg/doc: {avg_val}");
    }

    if let Some(ref ts) = info.created_at {
        wln!(out, "Created: {ts}");
    }
    if let Some(ref ts) = info.updated_at {
        wln!(out, "Updated: {ts}");
    }
    if let Some(ref m) = info.last_mode {
        wln!(out, "Last mode: {m}");
    }

    match info.phrase_search.as_deref() {
        Some("true") => wln!(out, "Phrase search: enabled"),
        Some("false") => wln!(out, "Phrase search: disabled"),
        _ => {}
    }

    if let Some(ref lang) = info.language
        && lang != "english"
    {
        wln!(out, "Language: {lang}");
    }

    if let Some(ref v) = info.lore_version {
        wln!(out, "Version: {}", format_version(v));
    }

    if !info.topics.is_empty() {
        format_topics_section(&mut out, &info.topics, &topic_overflow);
    }

    out.trim_end().to_owned()
}

/// Render the topic listing section of the store overview.
fn format_topics_section(
    out: &mut String,
    topics: &[store::TopicStat],
    topic_overflow: &impl Fn(usize) -> String,
) {
    const TOPIC_DISPLAY_LIMIT: usize = 10;

    let mut sorted: Vec<_> = topics.to_vec();
    sorted.sort_by_key(|t| std::cmp::Reverse(t.chunk_count));
    let total = sorted.len();
    let page = &sorted[..TOPIC_DISPLAY_LIMIT.min(total)];
    w!(out, "\n{total} topic{}", plural(total));
    if total > TOPIC_DISPLAY_LIMIT {
        w!(out, ", 1-{TOPIC_DISPLAY_LIMIT}/{total}. ");
        out.push_str(&topic_overflow(total));
    }
    out.push('\n');
    for s in page {
        w!(out, "- {}: {} doc", s.name, s.doc_count);
        if s.word_count > 0 {
            w!(out, ", {} words", s.word_count);
        }
        wln!(out, ", {} chunks", s.chunk_count);
    }
}

/// Build the server instructions string from pre-computed store metadata.
fn build_instructions(info: &store::StoreInfo) -> String {
    let mut parts = Vec::new();

    if let Some(ref desc) = info.description {
        parts.push(desc.clone());
    } else {
        parts.push("lore provides read-only access to a pre-built knowledge base.".to_owned());
    }

    parts.push(TOOL_INSTRUCTIONS.to_owned());

    if !info.topics.is_empty() {
        let topic_entries: Vec<String> = info
            .topics
            .iter()
            .take(TOPIC_PREVIEW_CAP)
            .map(|s| {
                format!(
                    "{} ({} chunk{})",
                    s.name,
                    s.chunk_count,
                    plural(s.chunk_count)
                )
            })
            .collect();
        let mut line = format!(
            "{} topic{}: {}",
            info.topics.len(),
            plural(info.topics.len()),
            topic_entries.join(", ")
        );
        if info.topics.len() > TOPIC_PREVIEW_CAP {
            w!(
                line,
                ". 1-{TOPIC_PREVIEW_CAP}/{}. Use lore_list_topics to list all",
                info.topics.len()
            );
        }
        line.push('.');
        parts.push(line);
    }

    let mut meta_parts = Vec::new();
    meta_parts.push(format!(
        "{} document{}",
        info.documents,
        plural(info.documents)
    ));
    meta_parts.push(format!(
        "{} chunk{} total",
        info.chunks,
        plural(info.chunks)
    ));
    if info.phrase_search.as_deref() == Some("false") {
        meta_parts.push("phrase search disabled".to_owned());
    }
    if let Some(ref ts) = info.updated_at {
        meta_parts.push(format!("updated {ts}"));
    }
    parts.push(meta_parts.join(" | "));

    parts.join("\n\n")
}

#[cfg(feature = "ingest")]
struct ServeWatchObserver {
    stores: Arc<StoreSet>,
    meta: Arc<std::sync::RwLock<SharedMeta>>,
}

#[cfg(feature = "ingest")]
impl lore::ingest::watch::WatchObserver for ServeWatchObserver {
    fn on_watching(&self, path: &std::path::Path) {
        info!(path = %path.display(), "watching for changes");
    }
    fn on_watch_error(&self, path: &std::path::Path, error: &notify::Error) {
        tracing::warn!(path = %path.display(), %error, "skipping watch path");
    }
    fn on_mode(&self, has_watcher: bool, interval_secs: Option<u64>, debounce_secs: u64) {
        info!(
            has_watcher,
            ?interval_secs,
            debounce_secs,
            "watch mode active"
        );
    }
    fn on_stopping(&self) {
        info!("watch loop stopping");
    }
    fn on_cycle_ok(&self, result: &lore::ingest::IngestResult) {
        if result.documents == 0 && result.failed_docs.is_empty() {
            tracing::debug!(elapsed = ?result.elapsed, "up to date");
        } else {
            info!(
                documents = result.documents,
                chunks = result.chunks,
                elapsed = ?result.elapsed,
                "ingest cycle complete"
            );
        }
        if !result.failed_docs.is_empty() {
            tracing::warn!(
                failed = result.failed_docs.len(),
                "documents failed during ingest"
            );
        }
    }
    fn on_cycle_complete(&self) {
        self.stores.first_store().reload_sidecars();
        refresh_meta(&self.stores, &self.meta);
    }
    fn on_cycle_error(&self, context: &str, error: &anyhow::Error) {
        tracing::error!(%error, "{context}");
    }
}

/// Start the MCP server on the given transport.
pub async fn run(opts: ServeOptions<'_>) -> Result<()> {
    let ServeOptions {
        configs,
        transport,
        host,
        port,
        expose,
        token,
        watch,
        #[cfg(feature = "ingest")]
        watch_debounce,
        #[cfg(not(feature = "ingest"))]
            watch_debounce: _,
    } = opts;

    anyhow::ensure!(!configs.is_empty(), "at least one config is required");

    if transport != Transport::Stdio && !is_loopback(host) {
        if !expose {
            anyhow::bail!("non-loopback host requires --expose flag");
        }
        if token.is_none() {
            let paint = crate::terminal::stderr_painter();
            eprintln!(
                "[{} ] serving on {host} without authentication; \
                 consider --token <SECRET> for access control",
                paint.yellow("!")
            );
        }
    }

    let stores = Arc::new(crate::cli::open_stores(configs)?);
    let meta = Arc::new(std::sync::RwLock::new(SharedMeta::from_store(&stores)));

    spawn_store_watcher(&stores, &meta);

    #[cfg(feature = "ingest")]
    if watch {
        for rc in configs {
            spawn_watch_loop(
                rc.config.clone(),
                rc.config_path.clone(),
                &stores,
                &meta,
                watch_debounce,
            );
        }
    }
    #[cfg(not(feature = "ingest"))]
    if watch {
        anyhow::bail!("--watch requires the `ingest` feature");
    }

    match transport {
        Transport::Stdio => run_stdio(stores, meta).await,
        Transport::Http => run_http(stores, meta, host, port, token).await,
    }
}

#[cfg(feature = "ingest")]
fn spawn_watch_loop(
    config: lore::config::IngestConfig,
    config_path: std::path::PathBuf,
    stores: &Arc<StoreSet>,
    meta: &Arc<std::sync::RwLock<SharedMeta>>,
    debounce_secs: u64,
) {
    let observer = Box::new(ServeWatchObserver {
        stores: Arc::clone(stores),
        meta: Arc::clone(meta),
    });
    tokio::spawn(async move {
        if let Err(e) = lore::ingest::watch::watch(
            config,
            config_path,
            debounce_secs,
            None,
            None,
            observer,
            Arc::new(lore::ingest::QuietIngestObserver::new(Arc::new(
                std::sync::atomic::AtomicBool::new(false),
            ))),
        )
        .await
        {
            tracing::error!(error = %e, "watch loop failed");
        }
    });
}

/// Spawn a background task that watches store directories for changes written
/// by an external `lore ingest` process and reloads the in-memory caches.
fn spawn_store_watcher(stores: &Arc<StoreSet>, meta: &Arc<std::sync::RwLock<SharedMeta>>) {
    use notify::{RecursiveMode, Watcher};
    use std::time::Duration;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher =
        match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok() {
                tx.send(()).ok();
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("failed to create store watcher: {e}");
                return;
            }
        };

    for store in stores.iter() {
        if let Err(e) = watcher.watch(store.path(), RecursiveMode::Recursive) {
            tracing::warn!(path = %store.path().display(), "failed to watch store: {e}");
        }
    }

    let stores = Arc::clone(stores);
    let meta = Arc::clone(meta);
    tokio::spawn(async move {
        let _watcher = watcher;
        let debounce = Duration::from_secs(1);

        while rx.recv().await.is_some() {
            loop {
                tokio::select! {
                    () = tokio::time::sleep(debounce) => break,
                    msg = rx.recv() => {
                        if msg.is_none() { return; }
                    }
                }
            }
            for store in stores.iter() {
                store.reload_sidecars();
            }
            refresh_meta(&stores, &meta);
            tracing::debug!("store change detected, caches reloaded");
        }
    });
}

/// Recompute cached store info and server instructions after a re-ingest cycle.
fn refresh_meta(stores: &StoreSet, meta: &std::sync::RwLock<SharedMeta>) {
    let mut m = meta
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *m = SharedMeta::from_store(stores);
}

/// Start the MCP server using JSON-RPC over stdin/stdout.
async fn run_stdio(stores: Arc<StoreSet>, meta: Arc<std::sync::RwLock<SharedMeta>>) -> Result<()> {
    let server = LoreServer::from_store_with_meta(stores, meta);

    if let Some(ref name) = server.cached_info().name {
        info!(name = name.as_str(), "starting MCP server (stdio)");
    } else {
        info!("starting MCP server (stdio)");
    }

    let (stdin, stdout) = rmcp::transport::stdio();
    let Ok(handle) = server.serve((stdin, stdout)).await else {
        anyhow::bail!(
            "no MCP client connected on stdin -- \
             use an MCP client or switch to --transport http"
        )
    };
    handle
        .waiting()
        .await
        .map_err(|_| anyhow::anyhow!("MCP client disconnected"))?;
    Ok(())
}

/// Compare two byte slices in constant time to prevent timing side-channels.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Start the MCP server using Streamable HTTP transport.
async fn run_http(
    stores: Arc<StoreSet>,
    meta: Arc<std::sync::RwLock<SharedMeta>>,
    host: &str,
    port: u16,
    token: Option<String>,
) -> Result<()> {
    use axum::response::IntoResponse;
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    use tokio_util::sync::CancellationToken;

    if let Some(ref name) = stores.store_info().name {
        info!(
            name = name.as_str(),
            transport = "http",
            "starting MCP server"
        );
    } else {
        info!(transport = "http", "starting MCP server");
    }

    let ct = CancellationToken::new();
    let http_config =
        StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token());

    let service = StreamableHttpService::new(
        move || {
            let server = LoreServer::from_store_with_meta(stores.clone(), meta.clone());
            Ok::<_, std::io::Error>(server)
        },
        Arc::new(LocalSessionManager::default()),
        http_config,
    );

    let app = axum::Router::new().nest_service("/mcp", service);
    let app = if let Some(ref token) = token {
        let expected = format!("Bearer {token}");
        app.layer(axum::middleware::from_fn(
            move |req: axum::extract::Request, next: axum::middleware::Next| {
                let expected = expected.clone();
                async move {
                    let auth = req
                        .headers()
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|v| v.to_str().ok());
                    if auth.is_some_and(|a| constant_time_eq(a.as_bytes(), expected.as_bytes())) {
                        next.run(req).await
                    } else {
                        axum::http::StatusCode::UNAUTHORIZED.into_response()
                    }
                }
            },
        ))
    } else {
        app
    };
    let bind = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("failed to bind to {bind}"))?;

    tracing::info!(url = format!("http://{bind}/mcp"), "MCP server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.ok();
            tracing::info!("shutting down");
            ct.cancel();
        })
        .await
        .context("server error")?;

    Ok(())
}

#[cfg(test)]
pub(crate) fn make_test_server() -> (LoreServer, tempfile::TempDir) {
    use lore::store::test_helpers::{temp_store, test_chunk, test_meta};

    let (store, dir) = temp_store();
    store
        .insert_chunks(&[
            test_chunk("/docs/intro.md", "introduction to the system", "intro", 0),
            test_chunk("/docs/api.md", "API reference for the service", "api", 0),
            test_chunk(
                "/docs/api.md",
                "second API chunk with more detail",
                "api",
                1,
            ),
        ])
        .unwrap();
    store.upsert_document(test_meta("/docs/intro.md", "intro"));
    store.upsert_document(test_meta("/docs/api.md", "api"));
    store.commit().unwrap();
    let server = LoreServer::from_store(store);
    (server, dir)
}
