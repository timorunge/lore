use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[cfg(feature = "ingest")]
use crate::cli::CacheScope;

/// Sort order for full-text search results.
#[derive(Clone, Copy, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum SearchSortArg {
    Score,
    Source,
    Topic,
}

/// Sort order for topic list results.
#[derive(Clone, Copy, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum TopicSortArg {
    Name,
    Chunks,
    Words,
}

/// Sort order for document list results.
#[derive(Clone, Copy, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum DocSortArg {
    Source,
    Topic,
    Title,
    Author,
    Chunks,
    Words,
    Quality,
}

/// MCP transport protocol for the `serve` subcommand.
#[derive(Clone, Copy, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum TransportArg {
    /// JSON-RPC over stdin/stdout
    Stdio,
    /// Streamable HTTP (includes SSE streaming)
    Http,
}

impl From<SearchSortArg> for lore::query::SearchSort {
    fn from(v: SearchSortArg) -> Self {
        match v {
            SearchSortArg::Score => Self::Score,
            SearchSortArg::Source => Self::Source,
            SearchSortArg::Topic => Self::Topic,
        }
    }
}

impl From<TopicSortArg> for lore::query::TopicSort {
    fn from(v: TopicSortArg) -> Self {
        match v {
            TopicSortArg::Name => Self::Name,
            TopicSortArg::Chunks => Self::Chunks,
            TopicSortArg::Words => Self::Words,
        }
    }
}

impl From<DocSortArg> for lore::query::DocSort {
    fn from(v: DocSortArg) -> Self {
        match v {
            DocSortArg::Source => Self::Source,
            DocSortArg::Topic => Self::Topic,
            DocSortArg::Title => Self::Title,
            DocSortArg::Author => Self::Author,
            DocSortArg::Chunks => Self::Chunks,
            DocSortArg::Words => Self::Words,
            DocSortArg::Quality => Self::Quality,
        }
    }
}

impl From<TransportArg> for crate::serve::Transport {
    fn from(v: TransportArg) -> Self {
        match v {
            TransportArg::Stdio => Self::Stdio,
            TransportArg::Http => Self::Http,
        }
    }
}

/// Top-level CLI parser for the `lore` binary.
#[derive(Parser)]
#[command(
    name = "lore",
    version = lore::VERSION,
    disable_colored_help = true,
    color = clap::ColorChoice::Never,
    about = "Build and search a local knowledge base",
    after_help = "\
Examples:
  lore init                       # Scaffold config from current directory
  lore ingest                     # Build or update the knowledge base
  lore search \"the lore of lore\"  # Search the knowledge base
  lore topics                     # Browse topics and document counts
  lore serve                      # Start MCP server for AI assistants"
)]
pub struct Cli {
    /// Config file path (repeatable for multi-KB federation; default: .lore/lore.yaml, or LORE_CONFIG)
    #[arg(short = 'c', long, global = true, value_name = "PATH", action = clap::ArgAction::Append)]
    pub config: Vec<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

/// Available subcommands for the `lore` CLI.
#[derive(Subcommand)]
pub enum Command {
    /// Scaffold a lore.yaml config by scanning the current directory
    Init {
        /// Create the global user config instead of a project config
        #[arg(long)]
        global: bool,
    },
    /// Build or incrementally update the knowledge base
    #[cfg(feature = "ingest")]
    #[command(long_about = "Build or incrementally update the knowledge base.\n\n\
        Reads sources from the config, fetches documents, chunks content, and \
        builds the full-text search store. On subsequent runs, only changed \
        documents are re-processed. Use --force to bypass freshness checks, \
        or --recreate to rebuild from scratch.")]
    Ingest {
        /// Rebuild the store from scratch, discarding existing data
        #[arg(long)]
        recreate: bool,
        /// Show what would be processed without writing anything
        #[arg(long)]
        dry_run: bool,
        /// Re-process all documents, bypassing freshness checks
        #[arg(long)]
        force: bool,
        /// Suppress progress bars and status messages
        #[arg(long, short = 'q')]
        quiet: bool,
        /// Only process sources whose path or URL contains this substring
        #[arg(long)]
        source: Option<String>,
    },
    /// Full-text search across indexed documents
    #[command(long_about = "Full-text search across indexed documents.\n\n\
        Runs a BM25 query against all chunks. Results ranked by relevance; \
        narrow with topic, author, lang, and other filters.")]
    Search {
        /// Free-text search query
        query: String,
        /// Maximum results to return (default: 10)
        #[arg(
            long,
            default_value = "10",
            hide_default_value = true,
            value_name = "N"
        )]
        limit: usize,
        /// Skip this many results (default: 0)
        #[arg(long, default_value = "0", hide_default_value = true, value_name = "N")]
        offset: usize,
        /// Filter by source path (substring match)
        #[arg(long)]
        source: Option<String>,
        /// Filter by topic name
        #[arg(long)]
        topic: Option<String>,
        /// Filter by author
        #[arg(long)]
        author: Option<String>,
        /// Filter by language code
        #[arg(long)]
        lang: Option<String>,
        /// Filter by source origin (local, url, git, sitemap, feed, s3, youtube, maildir, exec, mcp)
        #[arg(long)]
        origin: Option<String>,
        /// Filter by document kind (document, code, data, email)
        #[arg(long)]
        kind: Option<String>,
        /// Filter by file format (md, pdf, rs, json, etc.)
        #[arg(long)]
        format: Option<String>,
        /// Maximum results per source document
        #[arg(long, value_name = "N")]
        max_per_source: Option<usize>,
        /// Sort order: score, source, topic (default: score)
        #[arg(
            long,
            default_value = "score",
            hide_default_value = true,
            hide_possible_values = true
        )]
        sort: SearchSortArg,
        /// Reverse the sort order
        #[arg(long)]
        reverse: bool,
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// List topics with document and chunk counts
    Topics {
        /// Maximum topics to return (default: 20)
        #[arg(
            long,
            default_value = "20",
            hide_default_value = true,
            value_name = "N"
        )]
        limit: usize,
        /// Skip this many topics (default: 0)
        #[arg(long, default_value = "0", hide_default_value = true, value_name = "N")]
        offset: usize,
        /// Filter by topic name (fuzzy)
        #[arg(long)]
        topic: Option<String>,
        /// Filter by author (fuzzy)
        #[arg(long)]
        author: Option<String>,
        /// Filter by source path (substring match)
        #[arg(long)]
        source: Option<String>,
        /// Filter by language code
        #[arg(long)]
        lang: Option<String>,
        /// Filter by source origin (local, url, git, sitemap, feed, s3, youtube, maildir, exec, mcp)
        #[arg(long)]
        origin: Option<String>,
        /// Filter by document kind (document, code, data, email)
        #[arg(long)]
        kind: Option<String>,
        /// Filter by file format (e.g. md, pdf, rs)
        #[arg(long)]
        format: Option<String>,
        /// Sort order: name, chunks, words (default: name)
        #[arg(
            long,
            default_value = "name",
            hide_default_value = true,
            hide_possible_values = true
        )]
        sort: TopicSortArg,
        /// Reverse the sort order
        #[arg(long)]
        reverse: bool,
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// List indexed documents with optional filters
    Docs {
        /// Maximum documents to return (default: 20)
        #[arg(
            long,
            default_value = "20",
            hide_default_value = true,
            value_name = "N"
        )]
        limit: usize,
        /// Skip this many documents (default: 0)
        #[arg(long, default_value = "0", hide_default_value = true, value_name = "N")]
        offset: usize,
        /// Filter by source path (substring match)
        #[arg(long)]
        source: Option<String>,
        /// Filter by topic name
        #[arg(long)]
        topic: Option<String>,
        /// Filter by title (substring match)
        #[arg(long)]
        title: Option<String>,
        /// Filter by author
        #[arg(long)]
        author: Option<String>,
        /// Filter by language code
        #[arg(long)]
        lang: Option<String>,
        /// Filter by source origin (local, url, git, sitemap, feed, s3, youtube, maildir, exec, mcp)
        #[arg(long)]
        origin: Option<String>,
        /// Filter by document kind (document, code, data, email)
        #[arg(long)]
        kind: Option<String>,
        /// Filter by file format (md, pdf, rs, json, etc.)
        #[arg(long)]
        format: Option<String>,
        /// Sort order: source, topic, title, author, chunks, words, quality (default: source)
        #[arg(
            long,
            default_value = "source",
            hide_default_value = true,
            hide_possible_values = true
        )]
        sort: DocSortArg,
        /// Reverse the sort order
        #[arg(long)]
        reverse: bool,
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Read a specific document by source path
    Read {
        /// Full path, partial path, or basename of the document
        source: String,
        /// Maximum chunks to return (default: 50)
        #[arg(
            long,
            default_value = "50",
            hide_default_value = true,
            value_name = "N"
        )]
        limit: usize,
        /// Skip this many chunks (default: 0)
        #[arg(long, default_value = "0", hide_default_value = true, value_name = "N")]
        offset: usize,
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
        /// Show the full document as continuous text without chunk separators
        #[arg(long)]
        full: bool,
        /// Pipe output through an external pager (e.g. "less -R", "bat")
        #[arg(long, value_name = "CMD")]
        pager: Option<String>,
        /// Disable the pager even when LORE_PAGER or PAGER is set
        #[arg(long)]
        no_pager: bool,
    },
    /// Show store statistics and metadata
    Info {
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Serve the knowledge base over MCP (stdio or HTTP)
    Serve {
        /// Transport protocol: stdio, http (default: stdio)
        #[arg(
            long,
            default_value = "stdio",
            hide_default_value = true,
            hide_possible_values = true
        )]
        transport: TransportArg,
        /// Bind address for HTTP transport (default: 127.0.0.1)
        #[arg(long, default_value = "127.0.0.1", hide_default_value = true)]
        host: String,
        /// Port for HTTP transport (default: 8080)
        #[arg(long, default_value = "8080", hide_default_value = true)]
        port: u16,
        /// Allow binding to a non-loopback address (required when --host is not localhost)
        #[arg(long)]
        expose: bool,
        /// Bearer token for HTTP authentication (also reads LORE_SERVE_TOKEN env var)
        #[arg(long, env = "LORE_SERVE_TOKEN", hide_env = true)]
        token: Option<String>,
        /// Watch local sources and re-ingest on file changes inside the server process
        /// (the server always auto-reloads when an external `lore ingest` writes changes)
        #[arg(long)]
        watch: bool,
        /// Seconds to wait after last file-system event before triggering re-ingest (default: 2)
        #[arg(
            long,
            default_value = "2",
            hide_default_value = true,
            value_name = "SECS"
        )]
        debounce: u64,
    },
    /// Watch local sources and re-ingest on file changes
    #[cfg(feature = "ingest")]
    Watch {
        /// Seconds to wait after last file-system event before triggering ingest (default: 2)
        #[arg(
            long,
            default_value = "2",
            hide_default_value = true,
            value_name = "SECS"
        )]
        debounce: u64,
        /// Re-ingest all sources every N seconds, including remote sources.
        #[arg(long, value_name = "SECS")]
        interval: Option<u64>,
        /// Only watch/ingest sources whose path or URL contains this substring
        #[arg(long)]
        source: Option<String>,
    },
    /// Preview document processing without writing to the store
    #[cfg(feature = "ingest")]
    Preview {
        /// Files or directories to preview (uses config sources if omitted)
        paths: Vec<PathBuf>,
        /// Maximum number of documents to preview (default: 5)
        #[arg(long, default_value = "5", hide_default_value = true, value_name = "N")]
        limit: usize,
        /// Number of documents to skip before previewing (default: 0)
        #[arg(long, default_value = "0", hide_default_value = true, value_name = "N")]
        offset: usize,
        /// Maximum chunks to show per document (default: 3)
        #[arg(long, default_value = "3", hide_default_value = true, value_name = "N")]
        chunks: usize,
        /// Output JSON instead of text (NDJSON: one JSON object per line)
        #[arg(long)]
        json: bool,
        /// Pipe output through an external pager (e.g. "less -R", "bat")
        #[arg(long, value_name = "CMD")]
        pager: Option<String>,
        /// Disable the pager even when LORE_PAGER or PAGER is set
        #[arg(long)]
        no_pager: bool,
    },
    /// Re-run LLM enrichment on stored documents without re-fetching
    #[cfg(feature = "llm")]
    Enrich {
        /// Only enrich documents matching this source path substring.
        #[arg(long)]
        source: Option<String>,
        /// Only enrich documents in this topic.
        #[arg(long)]
        topic: Option<String>,
        /// Re-enrich documents that already have LLM data.
        #[arg(long)]
        force: bool,
    },
    /// Show what changed since last ingest (stat-only, no content reading)
    #[cfg(feature = "ingest")]
    Status {
        /// Output JSON instead of status lines
        #[arg(long)]
        json: bool,
        /// Check remote sources for changes (network requests)
        #[arg(long)]
        remote: bool,
    },
    /// Generate shell completion scripts
    Completions {
        /// Target shell
        shell: clap_complete::Shell,
    },
    /// Store maintenance: consistency checks, optimization, cache
    #[command(subcommand_required = false)]
    Maintain {
        #[command(subcommand)]
        action: Option<MaintainAction>,
    },
    /// Display the kraken splash screen
    #[command(hide = true)]
    Splash {
        /// Loop the animation until interrupted
        #[arg(long = "loop")]
        loop_: bool,
    },
}

/// Subcommands under `lore maintain`.
#[derive(Subcommand)]
pub enum MaintainAction {
    /// Check store consistency (read-only)
    Check {
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Fix detected consistency issues
    Repair {
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Merge index segments for lower memory and faster reads
    Compact,
    /// Clear cached downloads, git repos, or temporary files
    #[cfg(feature = "ingest")]
    Clean {
        /// What to clear: all, http, repos, archives, or tmp (default: all)
        #[arg(default_value = "all", hide_default_value = true)]
        scope: CacheScope,
    },
    /// Print a single-screen health report (config, features, LLM, store)
    Health,
}
