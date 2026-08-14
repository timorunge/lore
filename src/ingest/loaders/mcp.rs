use std::time::Duration;

use anyhow::{Context, Result};
use rmcp::model::{CallToolRequestParams, ReadResourceRequestParams, ResourceContents};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use tracing::{debug, info, warn};

use crate::config::{McpResources, McpSource, McpTransport};
use crate::ingest::types::{FailedDoc, LoaderResult};
use crate::types::SourceType;
use crate::util::progress::ProgressHandle;

fn server_label(mcp: &str) -> String {
    let trimmed = mcp.split_whitespace().next().unwrap_or(mcp);
    if trimmed.len() > 40 {
        format!("{}...", &trimmed[..37])
    } else {
        trimmed.to_owned()
    }
}

fn resource_source_key(server: &str, uri: &str) -> String {
    format!("mcp://{server}/resource/{uri}")
}

fn tool_source_key(server: &str, name: &str, idx: usize) -> String {
    format!("mcp://{server}/tool/{name}/{idx}")
}

fn extract_resource_text(contents: &[ResourceContents]) -> String {
    contents
        .iter()
        .filter_map(|c| match c {
            ResourceContents::TextResourceContents { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn make_doc(source: String, content: String, topic: Option<&str>) -> LoaderResult {
    LoaderResult {
        source_id: crate::types::source_id(&source),
        source,
        origin: SourceType::Mcp,
        kind: crate::types::DocKind::default(),
        content,
        unchanged: false,
        format: None,
        topic: topic.map(str::to_owned),
        title: None,
        author: None,
        lang: None,
        created_at: None,
        tags: None,
        mtime_ns: None,
        size_bytes: None,
        etag: None,
        last_modified: None,
        content_hash_override: None,
    }
}

pub(crate) async fn run_mcp(
    s: &McpSource,
    topic: Option<&str>,
    timeout_secs: u64,
    progress: &ProgressHandle,
) -> Result<(Vec<LoaderResult>, Vec<FailedDoc>)> {
    let timeout = Duration::from_secs(timeout_secs);
    let label = server_label(&s.mcp);

    progress.inc_length(1);
    let result = tokio::time::timeout(timeout, run_session(s, topic, &label, progress)).await;
    match result {
        Ok(inner) => inner,
        Err(_) => anyhow::bail!("MCP session to {label:?} timed out after {timeout_secs}s"),
    }
}

async fn run_session(
    s: &McpSource,
    topic: Option<&str>,
    label: &str,
    progress: &ProgressHandle,
) -> Result<(Vec<LoaderResult>, Vec<FailedDoc>)> {
    let service = connect(s).await?;
    let peer = service.peer();
    progress.inc(1);
    info!(server = label, "connected to upstream MCP server");

    let mut docs = Vec::new();
    let mut failures = Vec::new();

    if let Some(ref resources) = s.resources {
        let uris = match resources {
            McpResources::All => {
                let all = peer
                    .list_all_resources()
                    .await
                    .context("failed to list resources from upstream MCP server")?;
                debug!(server = label, count = all.len(), "discovered resources");
                all.into_iter().map(|r| r.uri.clone()).collect::<Vec<_>>()
            }
            McpResources::List(list) => list.clone(),
        };

        progress.inc_length(uris.len() as u64);
        for uri in &uris {
            match peer
                .read_resource(ReadResourceRequestParams::new(uri.clone()))
                .await
            {
                Ok(result) => {
                    let text = extract_resource_text(&result.contents);
                    if text.is_empty() {
                        debug!(server = label, uri, "resource returned no text content");
                        continue;
                    }
                    let source = resource_source_key(label, uri);
                    docs.push(make_doc(source, text, topic));
                }
                Err(e) => {
                    let source = resource_source_key(label, uri);
                    failures.push(FailedDoc::new(
                        source,
                        format!("failed to read resource: {e}"),
                    ));
                    warn!(server = label, uri, "failed to read resource: {e}");
                }
            }
            progress.inc(1);
        }
    }

    progress.inc_length(s.tools.len() as u64);
    for (idx, tc) in s.tools.iter().enumerate() {
        let params = if tc.args.is_null() || !tc.args.is_object() {
            CallToolRequestParams::new(tc.name.clone())
        } else {
            let obj = tc
                .args
                .as_object()
                .expect("checked is_object above")
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            CallToolRequestParams::new(tc.name.clone()).with_arguments(obj)
        };

        match peer.call_tool(params).await {
            Ok(result) => {
                if result.is_error.unwrap_or(false) {
                    let err_text = result
                        .content
                        .iter()
                        .filter_map(|c| c.as_text())
                        .map(|t| t.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let source = tool_source_key(label, &tc.name, idx);
                    failures.push(FailedDoc::new(
                        source,
                        format!("tool returned error: {err_text}"),
                    ));
                    warn!(
                        server = label,
                        tool = tc.name.as_str(),
                        "tool returned error: {err_text}"
                    );
                    continue;
                }
                let text = result
                    .content
                    .iter()
                    .filter_map(|c| c.as_text())
                    .map(|t| t.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if text.is_empty() {
                    debug!(
                        server = label,
                        tool = tc.name.as_str(),
                        "tool returned no text content"
                    );
                    continue;
                }
                let source = tool_source_key(label, &tc.name, idx);
                docs.push(make_doc(source, text, topic));
            }
            Err(e) => {
                let source = tool_source_key(label, &tc.name, idx);
                failures.push(FailedDoc::new(source, format!("tool call failed: {e}")));
                warn!(
                    server = label,
                    tool = tc.name.as_str(),
                    "tool call failed: {e}"
                );
            }
        }
        progress.inc(1);
    }

    if let Err(e) = service.cancel().await {
        debug!(server = label, "MCP shutdown error (non-fatal): {e}");
    }

    Ok((docs, failures))
}

async fn connect(s: &McpSource) -> Result<RunningService<RoleClient, ()>> {
    match s.transport {
        McpTransport::Stdio => connect_stdio(s).await,
        McpTransport::Http => connect_http(s).await,
    }
}

async fn connect_stdio(s: &McpSource) -> Result<RunningService<RoleClient, ()>> {
    let parts: Vec<&str> = s.mcp.split_whitespace().collect();
    anyhow::ensure!(!parts.is_empty(), "mcp command is empty");

    let mut cmd = tokio::process::Command::new(parts[0]);
    cmd.args(&parts[1..]);
    for (k, v) in &s.env {
        cmd.env(k, v);
    }

    let (transport, _stderr) = rmcp::transport::TokioChildProcess::builder(cmd)
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("failed to spawn MCP stdio subprocess")?;
    ().serve(transport)
        .await
        .context("failed to initialize MCP client session (stdio)")
}

async fn connect_http(s: &McpSource) -> Result<RunningService<RoleClient, ()>> {
    use rmcp::transport::streamable_http_client::{
        StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    };

    let mut config = StreamableHttpClientTransportConfig::with_uri(s.mcp.as_str());
    if let Some(ref token) = s.token {
        config = config.auth_header(format!("Bearer {token}"));
    }

    let transport = StreamableHttpClientTransport::<reqwest::Client>::from_config(config);

    ().serve(transport)
        .await
        .context("failed to initialize MCP client session (HTTP)")
}
