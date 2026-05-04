# Design Philosophy

Why lore is built the way it is.

## The Core Idea

Knowledge lives in many formats -- PDFs, Word documents, HTML pages,
spreadsheets, email archives, Org-mode files, source code, scanned images.
Consuming that knowledge is harder than it should be. Some formats are
binary and require specific tools to read. Some tools are proprietary or
platform-specific. OCR needs its own stack. Even when you can open a file,
searching across a collection of mixed formats is a different problem
entirely. There are tools for each piece -- pdftotext, pandoc, tika,
tesseract -- but stitching them together into something searchable is its
own project.

lore exists to remove that complexity. It handles extraction, parsing, and
indexing so that users, agents, and integrations can focus on *consuming*
knowledge instead of figuring out *how to consume* it. Point lore at your
documents, run ingest, and search -- from the terminal, through scripts,
or via MCP.

lore is a knowledge base, not a reader or a document viewer. It indexes
content for search and references the original files, which remain the
single source of truth. What lore provides is a way to find and retrieve
the right content without needing to know which format it lives in or which
tool can open it.

Most retrieval setups for LLM agents add further complexity on top: embedding
models, vector databases, and a managed pipeline to keep them in sync. lore
closes the gap between "I have documents" and "my agent can search them"
with a single binary and no external services.

## BM25 as Default Search

BM25 is deterministic, needs no model, and its failure modes are predictable.
For structured reference material -- documentation, policies, API references,
wikis -- BM25 retrieval is competitive with vector search in practice and
requires no additional setup. It is the default and the only search mode
that ships with a zero-dependency build.

## Single Binary, Zero Runtime Dependencies

lore compiles to a single static binary with no runtime requirements: no
Python interpreter, no Docker daemon, no cloud account.

## Two Interfaces, Same Store

lore exposes every knowledge base through two interfaces: CLI for humans,
MCP for agents. Both are read-only, both use the same store methods,
both support pagination. The same knowledge base (or federated set of
knowledge bases) serves both audiences without separate tooling or
configuration.

The CLI is pipe-friendly (`--json`, `--limit`, `--offset`) so it composes
with standard Unix tools. `lore search auth --json | jq` works the way
you expect.

For agents, lore serves the store over the Model Context Protocol rather
than a REST API or custom SDK, integrating directly into Claude, Cursor,
and any MCP client without adapters. The default transport is stdio -- the
same pipe-based convention Unix tools have used for fifty+ years. A
streamable HTTP transport (`--transport http`) is available for web-based
clients, remote agents, or multi-client setups. All six tools are
read-only, making `lore serve` safe to expose without write-access
concerns.

The MCP server also exposes Resources alongside Tools. Resources let
clients browse the knowledge base content directly -- `lore://info` for
an overview, `lore://topics/{name}` for a topic's chunks, and
`lore://docs/{path}` for a document's chunks. Tools and resources share
the same store and search logic; which interface an agent uses is a matter
of preference and client capability.

## Multiple Source Types

lore ingests from local files, single URLs, URL lists, git repos, sitemaps,
RSS/Atom feeds, S3 buckets, YouTube videos/playlists/channels, Maildir
email stores, shell commands, and upstream MCP servers. Each source type
has its own freshness tracking (mtime, ETags, Last-Modified, HEAD commit
hashes, content hashes, source ID presence) so incremental updates skip
unchanged documents. Glob patterns and regex filters narrow what gets
ingested from any source.

The YouTube source type demonstrates lore's approach to external CLI
dependencies: rather than reimplementing YouTube's protocol (which changes
frequently), lore spawns `yt-dlp` as a subprocess and parses its output.
The `exec` source generalises this pattern: any shell command that writes
JSONL to stdout becomes a source, turning custom scripts and CLI tools
into first-class inputs.

## kreuzberg for Extraction

The format problem is central to what lore solves: users should not need to
know whether a file is a PDF, a Word document, or a scanned image in order
to search its content. Rather than building a format-specific extraction
toolchain or requiring users to install a collection of tools (pdftotext,
pandoc, tika, tesseract) each with their own setup requirements and failure
modes, lore delegates to kreuzberg -- a single library that handles 90+
formats including PDF, DOCX, HTML, email, EPUB, and more. Archives (zip,
tar, gz, etc.) are extracted by lore itself; each file inside is then
processed through kreuzberg individually. One extraction library means one
set of failure modes and one upgrade path.

## Tantivy as Embedded Index

The index runs in-process with no external server -- no Elasticsearch
cluster, no SQLite FTS workarounds, no external process to manage. lore
uses Tantivy, a Rust-native full-text search library, and stores the index
as a plain directory next to your config. You can copy it, back it up, or
ship it with your project.

## Binary Sidecars for Metadata

Document-level metadata is stored in bitcode sidecar files alongside the
Tantivy index rather than inside it. This decouples metadata from the search
schema and survives schema migrations without reindexing. Three sidecar
files serve different audiences:

- **`lore.docs`** -- display and aggregation metadata (titles, topics, chunk
  counts, quality scores). Loaded lazily on first access.
- **`lore.stamps`** -- change-detection state (content hash, ETag,
  Last-Modified, mtime, file size). Only loaded by commands that need
  change detection (`ingest`, `watch`, `status`, `serve`); read-only
  query commands skip it entirely.
- **`lore.meta`** -- store-level configuration (name, description, version
  strings, timestamps). Small, loaded eagerly at startup.

Separating change-detection stamps from document metadata means read-only
query commands never load change-detection state, and `lore search` skips all 
sidecars entirely -- topic and author resolution use Tantivy's term dictionary
instead of scanning lore.docs. bitcode gives ~2x faster deserialization than
JSON and ~37% smaller files. Use `lore info` and `lore docs` to inspect
store contents.

## Separate Knowledge Bases over Monolithic Store

lore encourages separate knowledge base instances per domain rather than
one large store. Different topics have different update cadences and access
patterns; separate instances give a clean ingest boundary and prevent
cross-contamination of relevance scores. When an agent needs multiple
domains, lore federates them at query time: pass multiple `--config` flags
(or set `LORE_CONFIG` with colon-separated paths) and lore merges results
across all knowledge bases. `lore serve -c a.yaml -c b.yaml` exposes one
MCP server backed by multiple KBs, with per-KB names and descriptions in
the server instructions.

In practice, most knowledge bases are local directories that already have
some structure -- a `docs/` folder, a wiki checkout, a collection of PDFs
sorted into subfolders. lore leans into that: directory layout becomes
topic structure, headings become section labels, and `lore init` scaffolds
a config from whatever it finds.

## Incremental Ingest

lore tracks freshness using file modification times for local files, ETags
and Last-Modified headers for HTTP sources, and HEAD commit hashes for git
repos -- and skips documents that have not changed. Periodic store commits
during a long ingest mean a cancelled run can resume rather than restart,
which matters for large knowledge bases.

## Defensive Ingestion

lore treats all ingested content as untrusted input. Archives, HTTP
downloads, sitemaps, and search queries all have hard limits and
sanitization to prevent abuse. See [Security](./security.md) for the
full threat model and specific protections.

## Concurrency Without Saturation

lore defaults to half the available CPU cores for parallel work, clamped
between 2 and 32. This leaves headroom for the OS, other processes, and
the Tantivy writer. The limit is configurable via `processing.concurrency`.
See [Architecture](./architecture.md) for how the streaming pipeline
works internally.

## Minimal by Default, Optional Cloud and LLM

The default build has no cloud dependencies, no LLM code, and no external
service requirements. LLM enrichment (summaries, tags, topic detection) and
S3 sources are compile-time opt-in via feature flags. The pipeline degrades
gracefully: if a feature is not compiled in or not configured, it is simply
skipped. This keeps the dependency surface minimal for the common case --
a local knowledge base with BM25 search.

See also: [Architecture](./architecture.md),
[Configuration](./configuration.md),
[MCP Integration](./mcp-integration.md).
