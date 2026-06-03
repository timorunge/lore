/// CLI argument definitions (shared between binary and xtask).
pub mod args;
/// Background animation during long ingest operations.
#[cfg(feature = "ingest")]
pub(crate) mod drift;
/// LLM enrichment of already-indexed documents.
#[cfg(feature = "llm")]
pub mod enrich;
/// Store statistics display.
pub mod info;
/// Multi-KB ingest orchestrator.
#[cfg(feature = "ingest")]
pub mod ingest;
/// Project initialization and config file generation.
pub mod init;
/// Store integrity checking and maintenance.
#[cfg(feature = "ingest")]
pub mod maintain;
/// Document preview and chunking inspection.
#[cfg(feature = "ingest")]
pub mod preview;
/// Single-document reading with pager support.
pub mod read;
/// Cinematic kraken splash screen animation.
pub mod splash;
/// Diff-based status checking for local and remote sources.
#[cfg(feature = "ingest")]
pub mod status;
/// File-system watch mode for continuous incremental ingest.
#[cfg(feature = "ingest")]
pub mod watch;

use std::fmt;
use std::path::{Path, PathBuf};
#[cfg(feature = "ingest")]
use std::sync::Arc;
#[cfg(feature = "ingest")]
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
#[cfg(feature = "ingest")]
use indicatif::{MultiProgress, ProgressDrawTarget};

pub use config::ResolvedConfig;
#[cfg(feature = "ingest")]
pub use lore::cache::{CacheScope, clear_cache};
use lore::config;
use lore::fmt::style;
use lore::store::{self, StoreSet};
#[cfg(feature = "ingest")]
use lore::util::platform::write_to_real_stderr;

/// Config file names searched in order when no explicit path is given.
const CONFIG_CANDIDATES: &[&str] = &[".lore/lore.yaml", ".lore/lore.yml", "lore.yaml", "lore.yml"];

/// File extensions that lore recognizes as documents worth indexing.
///
/// Shared across CLI subcommands (init scanning, preview filtering, etc.).
pub(crate) const DOC_EXTENSIONS: &[&str] = &[
    "md", "mdx", "rst", "txt", "adoc", "org", "tex", "pdf", "docx", "doc", "odt", "rtf", "html",
    "htm", "xml", "xlsx", "xls", "ods", "csv", "pptx", "odp", "epub", "fb2", "eml", "msg", "ipynb",
    "json", "yaml", "yml", "toml",
];

/// Dimmed `[label] ` prefix for multi-KB output. Emits nothing in single-config mode.
#[derive(Clone)]
pub struct LinePrefix(Option<String>);

impl LinePrefix {
    pub(crate) fn none() -> Self {
        Self(None)
    }

    pub(crate) fn new(label: &str, width: usize, paint: style::Painter) -> Self {
        Self(Some(format!(
            "{} ",
            paint.dim(&format!("[{label:width$}]"))
        )))
    }

    #[cfg(feature = "ingest")]
    pub(crate) fn is_some(&self) -> bool {
        self.0.is_some()
    }
}

impl fmt::Display for LinePrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref s) = self.0 {
            f.write_str(s)
        } else {
            Ok(())
        }
    }
}

/// Resolve one or more config paths from -c flags, LORE_CONFIG env var
/// (colon-separated), or filesystem discovery. Returns at least one path.
pub(crate) fn resolve_configs(cli_configs: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    if !cli_configs.is_empty() {
        return cli_configs
            .into_iter()
            .map(|p| {
                let s = p.to_string_lossy();
                Ok(PathBuf::from(config::expand_path(&s)))
            })
            .collect();
    }
    if let Ok(env_val) = std::env::var("LORE_CONFIG")
        && !env_val.is_empty()
    {
        let paths: Vec<PathBuf> = env_val
            .split(':')
            .filter(|s| !s.is_empty())
            .map(|s| PathBuf::from(config::expand_path(s)))
            .collect();
        if !paths.is_empty() {
            return Ok(paths);
        }
    }
    for name in CONFIG_CANDIDATES {
        let path = PathBuf::from(name);
        if path.is_file() {
            return Ok(vec![path]);
        }
    }
    anyhow::bail!("no config found -- run `lore init` to create one, or pass --config <path>")
}

/// Parse all resolved config files into `ResolvedConfig` entries.
pub fn resolve_all_configs(cli_configs: Vec<PathBuf>) -> Result<Vec<ResolvedConfig>> {
    let paths = resolve_configs(cli_configs)?;
    paths
        .into_iter()
        .map(|config_path| {
            let cfg = config::load_config_with_hints(&config_path)?;
            Ok(ResolvedConfig {
                config: cfg,
                config_path,
            })
        })
        .collect()
}

/// Open all stores from resolved configs as a federated `StoreSet`.
pub(crate) fn open_stores(configs: &[ResolvedConfig]) -> Result<StoreSet> {
    let stores: Vec<store::Store> = configs
        .iter()
        .map(|rc| {
            let store_path = rc.config.store_dir(&rc.config_path);
            if !store_path.is_dir() {
                return Err(lore::error::LoreError::StoreNotFound { path: store_path }.into());
            }
            store::Store::open_readonly(&store_path).context("failed to open store")
        })
        .collect::<Result<Vec<_>>>()?;

    StoreSet::new(stores)
}

/// Resolve configs and open all stores as a federated `StoreSet`.
pub fn resolve_stores(cli_configs: Vec<PathBuf>) -> Result<StoreSet> {
    let configs = resolve_all_configs(cli_configs)?;
    open_stores(&configs)
}

/// Derive a human-readable label for a config (for multi-config output).
pub fn config_label(name: Option<&String>, config_path: &Path) -> String {
    name.cloned().unwrap_or_else(|| {
        config_path.file_stem().map_or_else(
            || config_path.display().to_string(),
            |s| s.to_string_lossy().into_owned(),
        )
    })
}

/// Build per-config `LinePrefix` instances for multi-config output.
/// Single-config runs get `LinePrefix::none()`.
pub fn make_prefixes(configs: &[ResolvedConfig]) -> Vec<LinePrefix> {
    if configs.len() <= 1 {
        return vec![LinePrefix::none(); configs.len()];
    }
    let paint = crate::terminal::stderr_painter();
    let labels: Vec<String> = configs
        .iter()
        .map(|rc| config_label(rc.config.name.as_ref(), &rc.config_path))
        .collect();
    let width = labels.iter().map(String::len).max().unwrap_or(0);
    labels
        .iter()
        .map(|l| LinePrefix::new(l, width, paint))
        .collect()
}

/// Run a fallible operation for each resolved config, collecting errors.
pub fn run_per_config(
    cli_configs: Vec<PathBuf>,
    error_summary: &str,
    mut f: impl FnMut(&ResolvedConfig, &LinePrefix) -> Result<()>,
) -> Result<()> {
    let configs = resolve_all_configs(cli_configs)?;
    let prefixes = make_prefixes(&configs);
    let paint = crate::terminal::stderr_painter();
    let mut had_error = false;
    for (rc, pfx) in configs.iter().zip(&prefixes) {
        if let Err(e) = f(rc, pfx) {
            let label = config_label(rc.config.name.as_ref(), &rc.config_path);
            eprintln!("{pfx}[{} ] {label}: {e:#}", paint.red("-"));
            had_error = true;
        }
    }
    if had_error {
        anyhow::bail!("{error_summary}");
    }
    Ok(())
}

/// Async variant of `run_per_config`.
pub async fn run_per_config_async(
    cli_configs: Vec<PathBuf>,
    error_summary: &str,
    mut f: impl AsyncFnMut(&ResolvedConfig, &LinePrefix) -> Result<()>,
) -> Result<()> {
    let configs = resolve_all_configs(cli_configs)?;
    let prefixes = make_prefixes(&configs);
    let paint = crate::terminal::stderr_painter();
    let mut had_error = false;
    for (rc, pfx) in configs.iter().zip(&prefixes) {
        if let Err(e) = f(rc, pfx).await {
            let label = config_label(rc.config.name.as_ref(), &rc.config_path);
            eprintln!("{pfx}[{} ] {label}: {e:#}", paint.red("-"));
            had_error = true;
        }
    }
    if had_error {
        anyhow::bail!("{error_summary}");
    }
    Ok(())
}

/// Spawn ctrl-c and SIGTERM handlers that set a shutdown flag and hide progress bars.
#[cfg(feature = "ingest")]
pub(crate) fn spawn_signal_handlers(shutdown: &Arc<AtomicBool>, mp: &MultiProgress) {
    let paint = crate::terminal::stderr_painter();
    let interrupt_msg = format!(
        "\n[{} ] interrupted, committing progress...\n",
        paint.yellow("!")
    );
    {
        let flag = shutdown.clone();
        let mp_hide = mp.clone();
        let msg = interrupt_msg.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            flag.store(true, Ordering::SeqCst);
            mp_hide.set_draw_target(ProgressDrawTarget::hidden());
            write_to_real_stderr(&msg);
            tokio::signal::ctrl_c().await.ok();
            write_to_real_stderr(&format!(
                "[{} ] forced exit, progress may be lost\n",
                paint.red("x")
            ));
            lore::util::platform::silence_stderr_for_exit();
            std::process::exit(130);
        });
    }
    #[cfg(unix)]
    {
        let flag = shutdown.clone();
        let mp_hide = mp.clone();
        let msg = interrupt_msg;
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let mut sigterm =
                signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
            sigterm.recv().await;
            flag.store(true, Ordering::SeqCst);
            mp_hide.set_draw_target(ProgressDrawTarget::hidden());
            write_to_real_stderr(&msg);
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            lore::util::platform::silence_stderr_for_exit();
            std::process::exit(130);
        });
    }
}
