# Architecture

How lore is structured internally, and how the pieces connect.

## Module Structure

```
lore (library crate):
  src/
    cache/         Global cache directory management and cleanup
    config/        YAML config, path expansion, validation
    fmt/           Text formatting utilities (wrap, table, style)
    ingest/        Discovery, loading, chunking, metadata, transforms
    net/           HTTP client, SSRF protection, robots.txt enforcement
    output/        CLI, JSON, and MCP output formatting
    store/         Tantivy index, document store, search implementation
    util/          Generic helpers (hashing, formatting, filesystem, time)
    error.rs       Public error types (LoreError)
    lib.rs         Crate root (module declarations)
    llm.rs         LLM integration for document enrichment (feature-gated)
    query.rs       Search, topic, and document query API
    types.rs       Shared domain types (Chunk, DocMeta, SourceId, SourceType, DocKind)

cli/ (binary crate, package "lore"):
  src/
    cli/           CLI subcommands (init, ingest, search, topics, docs, read, info, serve, watch, preview, enrich, status, completions, maintain)
    serve/         MCP server (tools, resources, transports)
    pager.rs       External pager support (less, bat)
    progress.rs    indicatif progress bar styles
    terminal.rs    Terminal detection (TTY, width, color)
    main.rs        CLI entry point (clap)
```

## Ingest Pipeline

Data flows in one direction through a multi-stage pipeline:

```
config
  |
  v
discover          load                transform             chunk & index
  |                 |                     |                      |
  v                 v                     v                      v
SourceConfig -> SourceItems ------> LoaderResult --------> Chunk[] -> Store
  (discover)    (loaders)              (pipeline)           (chunker)  (tantivy)
                    |                     |
                    +-- extract mode      +-- transforms
                        (auto/builtin/       (strip_lines, replace_text,
                         kreuzberg/none)      drop_sections, ...)
```

### Stage 1: Discovery

`discover_source()` maps each `SourceConfig` variant to a `DiscoveredSource`:

<!-- BEGIN GENERATED: source-discovery -->
| Source type | Discovery |
|-------------|-----------|
| Local | Walk directory, match glob, return file paths |
| Git | Clone/fetch repo, check HEAD against stored hash, return file paths |
| S3 | List objects under prefix, filter by glob/regex, download inline |
| URL | Build URL list with existing ETags for conditional fetching |
| Sitemap | Fetch and parse XML (including nested sitemaps), return URL list |
| Feed | Fetch RSS/Atom XML, extract entry links, return URL list |
| YouTube | Parse URL, discover video IDs (playlist/channel via `yt-dlp --flat-playlist`), preload transcripts |
| Maildir | Walk cur/ and new/ subdirectories, parse RFC 2822 messages with mail-parser |
| Exec | Run shell command(s) via `sh -c`, read JSONL from stdout, one document per line |
| MCP | Connect to upstream MCP server, auto-discover resources and/or call tools, read text content |
<!-- END GENERATED: source-discovery -->

### Stage 2: Loading

Documents stream through loading individually via bounded channels:

- **Files**: Parallel `read_file()` per path. kreuzberg extracts text from
  binary formats (PDF, DOCX, HTML, etc.). Archives are detected and extracted
  first. Text files go through kreuzberg for structured text formats (HTML,
  CSV, RST, Org) and fall back to raw UTF-8 for source code and plain text.
- **URLs**: `try_download_all()` fetches files to disk with ETag/Last-Modified
  support. Downloaded files are checked for archive content types, extracted
  if needed, then processed through `read_file()`.

Loading and processing overlap via the streaming pipeline: each loaded
document flows directly into the processing stage as it completes,
without waiting for a full batch.

### Stage 3: Processing

`process_documents()` handles each `LoaderResult`:

1. **Freshness check** -- skip unchanged documents (mtime + size for files,
   ETag/Last-Modified for URLs, HEAD commit for git, content hash fallback)
2. **Pipeline transforms** -- the profile's transform steps run on the loaded
   content after the freshness check. Steps execute in declaration order and
   each receives the output of the previous step. Available operations:
   `strip_lines` (remove N lines from start/end), `replace_text` (regex
   find-and-replace), `extract_builtin` (run builtin metadata extraction on
   the current content), and `extract_metadata` (custom regex capture into a
   metadata field). Transforms run after the freshness check so that
   unchanged source files are skipped before any transform work.
3. **Metadata extraction** -- title, author, language, created date, topic, 
   tags are assembled with **first-wins** semantics. Loader-phase extraction 
   runs during `read_file` (Stage 2) per the `extract` mode: kreuzberg metadata
   for binary files, builtin text extraction (YAML/TOML frontmatter,
   Org-mode keywords, key-value headers, heuristics) for text files. Each
   field is set by the first source that provides a non-empty value; later
   sources cannot overwrite it. Pipeline `extract_builtin` and
   `extract_metadata` steps in Stage 3 can fill remaining empty fields but
   cannot overwrite loader-populated values.
4. **Topic** -- extracted from frontmatter/headers via the standard metadata
   pipeline, or set explicitly via the source config `topic` field.
   Optionally overridden by LLM
5. **Chunking** -- `chunk_markdown_content()` via `spawn_blocking` (CPU-bound,
   parallelized up to `concurrency`). Splits by heading structure, respects
   `max_chunk_chars`, merges chunks shorter than `min_chunk_chars` into the
   next chunk, drops configured sections
6. **LLM enrichment** (optional) -- topic detection, document-level summary,
   per-chunk summary/tags/quality score; chunks below `quality_threshold`
   are discarded. Enrichment is also available standalone via `lore enrich`,
   which re-runs this step on already-indexed documents without re-fetching.
7. **Store write** -- for updated documents, replace old chunks and metadata;
   for new documents, insert chunks and upsert metadata

**Extraction modes** (set via the processing profile, forwarded to
`load_range`):

<!-- BEGIN GENERATED: enum-extract-mode -->
| Mode | Behaviour |
|------|-----------|
| `auto` | Text: builtin metadata extraction. Binary: kreuzberg metadata + builtin extraction + merge. |
| `builtin` | Text: builtin metadata extraction only. Binary: builtin metadata extraction only (kreuzberg not called). |
| `kreuzberg` | Text: no metadata extraction. Binary: kreuzberg full extraction (document parsing + metadata). |
| `none` | Text: no metadata extraction. Binary: no metadata extraction; use `extract_builtin` in a pipeline for explicit control. |
<!-- END GENERATED: enum-extract-mode -->

### Processing Profiles

A processing profile bundles extraction and transformation settings that apply
uniformly to all documents from a source.

**Resolution flow** (mutually exclusive):

- Source has no `processing:` key -> use the global `processing` section
  as defaults
- Source has `processing: "name"` -> look up the named preset from
  `processing.presets`
- Source has `processing: { ... }` -> use the inline profile object directly

Named presets and inline profiles are complete, self-contained profiles --
they do not inherit from or merge with the global `processing` values.
Unspecified fields use `ProcessingProfile` struct defaults
(e.g. `max_chunk_chars: 1600`).

The resolved profile is compiled once per source into a `CompiledProfile`,
which holds pre-compiled regexes (avoiding repeated compilation per document)
and lowercased heading strings (for case-insensitive section filtering).

**Threading through the pipeline:**

- `process_source` resolves and compiles the profile before dispatching to
  loaders
- The extract mode (see Stage 2) is extracted from the compiled profile and
  forwarded to `load_range`, so loaders know whether to call kreuzberg, use
  the built-in extractor, or skip extraction entirely
- The full `CompiledProfile` is passed to `process_documents`, which applies
  the pipeline transforms and section-drop rules during Stage 3

### Stage 4: Commit

Periodic commits (every `commit_interval` seconds) persist progress so
cancelled ingests can resume. After all sources are processed, stale
documents (sources not seen in this run) are removed, and a final commit
flushes everything.

### Multi-KB Ingest

When multiple `--config` flags are passed, `cli::ingest` dispatches to a
multi-KB orchestrator instead of the single-config path. The orchestrator
shows per-KB spinners (waiting -> active -> done/error) and a status bar
showing running totals. KBs are ingested sequentially; each KB's result
updates the shared status line. The library's `IngestObserver` trait
mediates all progress reporting -- the CLI implements it with indicatif
progress bars, while the library itself has no terminal dependencies. A
shared `Arc<AtomicBool>` shutdown flag allows Ctrl+C / SIGTERM to commit
in-progress work and exit cleanly.

## Serve Layer (MCP)

`serve()` opens the store (or a federated `StoreSet` when multiple
`--config` flags are given) and binds a `LoreServer` via RMCP. Two
transport modes are supported:

```
stdio:  stdin (MCP requests)  -> LoreServer -> StoreSet (read-only) -> stdout (MCP responses)
http:   HTTP POST/GET on /mcp -> LoreServer -> StoreSet (read-only) -> HTTP/SSE responses
```

In HTTP mode, `StreamableHttpService` (from RMCP) manages sessions and
creates a `LoreServer` per session. All sessions share a single
`Arc<StoreSet>` for zero-copy read access. `StoreSet` federates queries
across multiple stores transparently -- single-store mode has no overhead.

The server always watches its store directories for filesystem changes.
When an external `lore ingest` writes new data, the server detects the
change (via `notify`), debounces for 1 second, and calls
`reload_sidecars()` to swap the in-memory document and stamp maps.

When `--watch` is passed, an additional embedded file-watcher monitors
the original source files and triggers incremental re-ingest inside the
server process itself.

The server generates tool instructions from the knowledge base name,
description, topic list, and statistics. This helps LLM agents understand
what content is available.

All six tools are read-only and idempotent. Results are formatted as
markdown strings. A hard `MAX_PAGE_LIMIT = 500` cap prevents unbounded
responses. Error details are logged server-side; clients receive generic
error messages.

In addition to tools, the server exposes MCP Resources: a static
`lore://info` resource and two URI templates (`lore://topics/{name}`,
`lore://docs/{path}`). Resources let MCP clients browse and read knowledge
base content without invoking tools. The resource and tool handlers share
the same underlying search functions and store instance. Resource handling
lives in `cli/src/serve/resources.rs`; the manual `ServerHandler` impl in
`cli/src/serve/mod.rs` delegates to both the `ToolRouter` and the resource
functions.

## Store

The `Store` struct in `src/store/mod.rs` owns the Tantivy index and
the in-memory document/metadata maps.

### Schema

Full-text BM25 fields with configurable stemming and field-level boosts:

<!-- BEGIN GENERATED: schema-search-fields -->
| Field | Boost | Purpose |
|-------|-------|---------|
| `body` | 1.0x | Chunk content |
| `title` | 3.0x | Document title |
| `section` | 2.0x | Section heading |
| `tags` | 2.0x | Document and chunk tags |
| `llm_summary` | 1.5x | LLM-generated summary |
| `llm_tags` | 1.5x | LLM-generated keywords |
<!-- END GENERATED: schema-search-fields -->

Exact-match filter fields (untokenized STRING): `topic`, `author`, `lang`,
`source`, `source_id`, `origin`, `kind`, `format`.

Stored-only fields (not indexed for search): `chunk_id` (deterministic
content hash for stable references), `chunk_index` (i64, also FAST),
`created_at`, `updated_at`.

### Persistence

Four layers:

- **Tantivy index** (`{store}/tantivy/`) -- segment files for full-text search
- **Document sidecar** (`lore.docs`) -- document metadata as a bitcode-encoded
  `Vec<DocMeta>`, loaded lazily on first access via `OnceLock`
- **Stamps sidecar** (`lore.stamps`) -- per-document change-detection
  state as a bitcode-encoded `Vec<(SourceId, StampsMeta)>`, loaded lazily
  via `OnceLock`. Contains content hash, ETag, Last-Modified, mtime, and
  file size. Only loaded during ingest; all other commands skip it.
- **Metadata sidecar** (`lore.meta`) -- small key-value map (store name,
  version strings, timestamps), loaded eagerly at `Store::open`

All sidecar files are persisted via atomic temp-file-then-rename writes.
The sidecars live outside Tantivy because document-level metadata (topic,
title, chunk count) and change-detection state need to survive restarts
without a full index scan.

Lazy loading means `lore search` (the hot path) never reads `lore.docs` or
`lore.stamps` at all. Topic and author resolution use Tantivy's term
dictionary directly, so even `--topic` and `--author` filters avoid loading
lore.docs. bitcode encoding is ~2x faster to parse and ~37% smaller than
the equivalent JSON.

### Search

BM25 ranking with `QueryParser` and field boosts. Filters (topic, author,
lang, origin, kind, format) are composed as `BooleanQuery` with `Must`
term clauses. Source filtering is a post-fetch substring match. Source
diversity (`max_per_source`) is implemented by over-fetching
up to `MAX_SEARCH_LIMIT = 1000` results (or `limit + offset` when
score-sorted with no source filter) then retaining up to N per source. Results can be sorted by score (default),
source, or topic, and optionally reversed.

Topic, author, and source resolution use Tantivy's STRING field term
dictionaries to enumerate distinct values without loading lore.docs. The
fuzzy matching algorithm applies: exact > case-insensitive > substring
(shortest wins, alphabetical tie-break). Source path resolution uses:
exact > suffix > basename > stem.

**Cross-store BM25 scores.** Tantivy has no multi-index reader, so
`StoreSet` merges results by comparing BM25 scores across stores.
BM25's IDF component is corpus-relative, so scores from stores with
substantially different sizes are not directly comparable. In practice
this is acceptable: multi-KB usage typically combines distinct domains,
and the BM25 signal dominates cross-store artifacts.

## Key Design Decisions

**kreuzberg for extraction.** A single library handles 90+ formats including
PDF, DOCX, HTML, email, and archives. lore adds a plain-text fallback and
metadata extraction layer on top.

**Tantivy for search.** Embedded, no external process, fast BM25 with
stemming. No external service or model required for the default search path.

**Binary sidecars for metadata.** Document-level metadata (topic assignments,
chunk counts, quality scores) and change-detection state (content hash,
mtime, ETag) are stored in separate bitcode sidecar files: `lore.docs` for
display/aggregation metadata, `lore.stamps` for ingest-only
change-detection, and `lore.meta` for store-level configuration. This
separation means non-ingest commands never load change-detection data,
and search never loads either sidecar. Use `lore info` and `lore docs`
to inspect store contents.

**Streaming pipeline.** Documents flow through loading, processing, and
indexing stages connected by bounded channels. Each document moves to the
next stage as soon as it completes, hiding network and I/O latency behind
CPU-bound chunking work.

**Defensive ingestion.** Archives are byte-counted during extraction (not
trusted from headers). HTTP/S3 downloads stream to disk with size caps.
Search queries are sanitized to prevent field-prefix injection. Sitemap
discovery caps fan-out at 500 sub-sitemaps and 50,000 URLs.

**RwLock over Mutex for read-heavy paths.** The document and metadata maps
use `RwLock` so concurrent read requests (CLI queries, MCP tools) do not
serialize. Writes only happen during ingest.

**Feature-gated cloud dependencies.** S3 support and the Bedrock LLM
provider pull in large AWS SDK crates. They are compile-time opt-in
(`--features s3,llm`) so the default build has no cloud dependencies.
The Ollama, Anthropic, and OpenAI providers use reqwest (already a
dependency) and compile unconditionally.

See also: [Configuration](./configuration.md), [CLI Reference](./cli.md),
[Supported Formats](./formats.md).
