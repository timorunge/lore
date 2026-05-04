# CLI Reference

A practical guide to using lore from the command line.

## Usage

```
lore [-c|--config <path>] <command>
```

lore uses subcommands. The `--config` flag is global and accepted before
or after the subcommand. If omitted, lore searches for a config file
automatically (see [Configuration](./configuration.md#how-configuration-works)).

## Commands

### init

Scaffold a `.lore/` directory with a starter config file.

```bash
lore init
```

lore scans the directory tree for files with document extensions (Markdown,
PDF, DOCX, HTML, and others), groups them by directory, and generates sources
with appropriate glob patterns for directories that contain enough matching
files. Root-level files like README.md are added individually. The git remote
origin URL is added as a commented-out source. The knowledge base is named
after the current directory.

If `.lore/lore.yaml` already exists, the command exits with an error.
In git repositories, lore suggests adding `.lore/store` to `.gitignore`.

Use `--global` to create the global user config instead of a project config.
See [Global Config](./configuration.md#global-config) for details.

<!-- BEGIN GENERATED: flags-init -->
| Flag | Description |
|------|-------------|
| `--global` | Create the global user config instead of a project config |
<!-- END GENERATED: flags-init -->

**Example output** (lore's own project):

```yaml
name: lore

sources:
  - path: ./docs
    glob: "**/*.md"

  - path: ./examples
    glob: "**/*.yaml"

  - path: ./README.md

  # Git remote detected:
  # - git: https://github.com/timorunge/lore.git
  #   glob: "**/*.md"
```

### ingest

Build or update the knowledge base.

```bash
# Incremental update -- only new or modified documents
lore ingest

# Drop and rebuild the entire store
lore ingest --recreate

# Show what would change without writing anything
lore ingest --dry-run

# Ingest multiple knowledge bases sequentially
lore ingest -c work.yaml -c vendor.yaml
```

Incremental by default: lore tracks content hashes and only re-processes
documents that changed. Periodic commits (every 30s by default) persist
progress so cancelled ingests can resume. Progress bars show per-source
discovery and processing status. Multi-KB ingest shows per-KB spinners
with an aggregated status line.

<!-- BEGIN GENERATED: flags-ingest -->
| Flag | Description |
|------|-------------|
| `--recreate` | Rebuild the store from scratch, discarding existing data |
| `--dry-run` | Show what would be processed without writing anything |
| `--force` | Re-process all documents, bypassing freshness checks |
| `-q, --quiet` | Suppress progress bars and status messages |
| `--source` | Only process sources whose path or URL contains this substring |
<!-- END GENERATED: flags-ingest -->

Sources with `update: never` in the config are skipped during incremental
ingests. Both `--force` and `--recreate` override this and process all
sources regardless of their update mode. See the
[update mode](configuration.md#update-mode) section for details.

When individual documents fail to process, the source line shows `[! ]`
(yellow) instead of `[+ ]` (green). Successfully processed documents from
that source are still indexed. Up to 20 failures are printed inline after
the summary; the full list is written to a timestamped JSONL file in the
cache logs directory (`lore maintain clean logs` clears old logs):

```
[! ] [1/3] ./docs (path) -- 42 docs, 310 chunks (3 failed)
[! ] 3 document(s) failed:
     docs/broken.pdf    unsupported encryption
     docs/empty.md      no extractable content
     docs/huge.docx     file exceeds size limit
     full log: <cache>/logs/failures-2026-05-04T04-04-00.jsonl
```

### search

Search the knowledge base with free-text queries. Returns
relevance-ranked results with highlighted query matches in titles,
sections, and body snippets. Each result includes a chunk position
so you know where in the document the match occurs -- use
`lore read <source> --offset N` to jump there.

```bash
# Basic search
lore search "authentication"

# Filter by topic, limit results
lore search "auth" --topic Security --limit 5

# JSON output for scripting
lore search "auth" --json | jq '.items[].source'

# Federated search across multiple knowledge bases
lore search "auth" -c work.yaml -c vendor.yaml
```

Topic and author filters use fuzzy matching: exact match, then
case-insensitive, then substring.

<!-- BEGIN GENERATED: flags-search -->
| Flag | Default | Description |
|------|---------|-------------|
| `--limit` | `10` | Maximum results to return (default: 10) |
| `--offset` | `0` | Skip this many results (default: 0) |
| `--source` |  | Filter by source path (substring match) |
| `--topic` |  | Filter by topic name |
| `--author` |  | Filter by author |
| `--lang` |  | Filter by language code |
| `--origin` |  | Filter by source origin (local, url, git, sitemap, feed, s3, youtube, maildir, exec, mcp) |
| `--kind` |  | Filter by document kind (document, code, data, email) |
| `--format` |  | Filter by file format (md, pdf, rs, json, etc.) |
| `--max-per-source` |  | Maximum results per source document |
| `--sort` | `score` | Sort order: score, source, topic (default: score) |
| `--reverse` |  | Reverse the sort order |
| `--json` |  | Output JSON instead of text |
<!-- END GENERATED: flags-search -->

### topics

List topics with document and chunk counts.

```bash
lore topics
lore topics --topic auth
lore topics --author "Jane"
lore topics --origin git
lore topics --sort words
lore topics --kind code --sort chunks
lore topics --json | jq '.items[].name'
```

Topic and author filters use fuzzy matching: exact match, then
case-insensitive, then substring.

<!-- BEGIN GENERATED: flags-topics -->
| Flag | Default | Description |
|------|---------|-------------|
| `--limit` | `20` | Maximum topics to return (default: 20) |
| `--offset` | `0` | Skip this many topics (default: 0) |
| `--topic` |  | Filter by topic name (fuzzy) |
| `--author` |  | Filter by author (fuzzy) |
| `--source` |  | Filter by source path (substring match) |
| `--lang` |  | Filter by language code |
| `--origin` |  | Filter by source origin (local, url, git, sitemap, feed, s3, youtube, maildir, exec, mcp) |
| `--kind` |  | Filter by document kind (document, code, data, email) |
| `--format` |  | Filter by file format (e.g. md, pdf, rs) |
| `--sort` | `name` | Sort order: name, chunks, words (default: name) |
| `--reverse` |  | Reverse the sort order |
| `--json` |  | Output JSON instead of text |
<!-- END GENERATED: flags-topics -->

### docs

List documents in the store with optional filters.

```bash
lore docs
lore docs --topic "API Reference"
lore docs --author "Jane" --lang en
lore docs --json | jq '.items[] | select(.chunk_count > 10)'
```

<!-- BEGIN GENERATED: flags-docs -->
| Flag | Default | Description |
|------|---------|-------------|
| `--limit` | `20` | Maximum documents to return (default: 20) |
| `--offset` | `0` | Skip this many documents (default: 0) |
| `--source` |  | Filter by source path (substring match) |
| `--topic` |  | Filter by topic name |
| `--title` |  | Filter by title (substring match) |
| `--author` |  | Filter by author |
| `--lang` |  | Filter by language code |
| `--origin` |  | Filter by source origin (local, url, git, sitemap, feed, s3, youtube, maildir, exec, mcp) |
| `--kind` |  | Filter by document kind (document, code, data, email) |
| `--format` |  | Filter by file format (md, pdf, rs, json, etc.) |
| `--sort` | `source` | Sort order: source, topic, title, author, chunks, words, quality (default: source) |
| `--reverse` |  | Reverse the sort order |
| `--json` |  | Output JSON instead of text |
<!-- END GENERATED: flags-docs -->

### read

Read a document by source path. Accepts a full path, partial path, or
basename. If multiple documents match, lists candidates for
disambiguation.

```bash
# Full path
lore read docs/guides/auth.md

# Basename (resolves if unambiguous)
lore read auth.md

# Limit chunks shown
lore read docs/auth.md --limit 5

# JSON output
lore read docs/auth.md --json
```

Source resolution follows this order: exact match, suffix match (e.g.
`guides/auth.md` matches `docs/guides/auth.md`), then basename match.
When multiple documents match, lore lists the candidates and exits.

Use `--full` to display the document as continuous text without chunk
separators, ideal for reading in a pager.

For long documents, pipe through an external pager with `--pager` or by
setting `LORE_PAGER` / `PAGER`. When `PAGER=less`, lore sets `LESS=R`
automatically so ANSI colors pass through.

<!-- BEGIN GENERATED: flags-read -->
| Flag | Default | Description |
|------|---------|-------------|
| `--limit` | `50` | Maximum chunks to return (default: 50) |
| `--offset` | `0` | Skip this many chunks (default: 0) |
| `--json` |  | Output JSON instead of text |
| `--full` |  | Show the full document as continuous text without chunk separators |
| `--pager` |  | Pipe output through an external pager (e.g. "less -R", "bat") |
| `--no-pager` |  | Disable the pager even when LORE_PAGER or PAGER is set |
<!-- END GENERATED: flags-read -->

### info

Show store statistics: document count, chunk count, word count, topics,
source types, languages, and timestamps.

```bash
# Uses config to find the store
lore info

# JSON output for CI scripts
lore info --json

# Aggregated info across multiple knowledge bases
lore info -c work.yaml -c vendor.yaml --json
```

<!-- BEGIN GENERATED: flags-info -->
| Flag | Description |
|------|-------------|
| `--json` | Output JSON instead of text |
<!-- END GENERATED: flags-info -->

### serve

Start the MCP server.

```bash
lore serve                                           # stdio (default)
lore serve --transport http                          # streamable HTTP on 127.0.0.1:8080
lore serve --transport http --host 0.0.0.0 --expose  # bind to all interfaces
lore serve --watch                                   # auto re-ingest on local file changes
lore serve -c work.yaml -c vendor.yaml               # federated multi-KB
```

The server exposes six read-only tools to MCP-compatible LLM agents.
It automatically detects store changes from external `lore ingest`
processes and reloads without restarting. Use `--watch` only if you
want the server to monitor source files and re-ingest on its own.
See [MCP Integration](./mcp-integration.md) for the full tool reference
and client setup instructions.

<!-- BEGIN GENERATED: flags-serve -->
| Flag | Default | Description |
|------|---------|-------------|
| `--transport` | `stdio` | Transport protocol: stdio, http (default: stdio) |
| `--host` | `127.0.0.1` | Bind address for HTTP transport (default: 127.0.0.1) |
| `--port` | `8080` | Port for HTTP transport (default: 8080) |
| `--expose` |  | Allow binding to a non-loopback address (required when --host is not localhost) |
| `--token` |  | Bearer token for HTTP authentication (also reads LORE_SERVE_TOKEN env var) |
| `--watch` |  | Watch local sources and re-ingest on file changes inside the server process (the server always auto-reloads when an external `lore ingest` writes changes) |
| `--debounce` | `2` | Seconds to wait after last file-system event before triggering re-ingest (default: 2) |
<!-- END GENERATED: flags-serve -->

**Security note:** When using the HTTP transport, Bearer token authentication
is available via `--token` (or the `LORE_SERVE_TOKEN` env var). Without a
token, any client that can reach the endpoint can query the knowledge base.
When binding to a non-loopback address (e.g. `0.0.0.0`), you must pass
`--expose` explicitly. lore will emit a warning as a reminder to keep the
endpoint private.

<!-- BEGIN GENERATED: mcp-tools-list -->
| Tool | Description |
|------|-------------|
| `lore_info` | Knowledge base overview: stats, topics, languages |
| `lore_list_topics` | List all topics with chunk counts |
| `lore_search` | Full-text search across body, title, sections, and tags |
| `lore_read_topic` | Read all chunks for a given topic |
| `lore_list_docs` | List documents with optional filters |
| `lore_read_doc` | Read a document's chunks sequentially |
<!-- END GENERATED: mcp-tools-list -->

### watch

Watch local source paths and re-ingest automatically when files change.

```bash
# Watch with default 2-second debounce
lore watch

# Custom debounce window
lore watch --debounce 5
```

Runs an initial incremental ingest, then monitors all `path:` sources defined
in the config. After the last file-system event in a burst, waits the debounce
window before triggering a new incremental ingest. Non-local sources are
not watched but are included in each triggered ingest cycle.

Press Ctrl+C to stop. If Ctrl+C arrives during an active ingest, progress
is committed before exiting.

<!-- BEGIN GENERATED: flags-watch -->
| Flag | Default | Description |
|------|---------|-------------|
| `--debounce` | `2` | Seconds to wait after last file-system event before triggering ingest (default: 2) |
| `--interval` |  | Re-ingest all sources every N seconds, including remote sources |
| `--source` |  | Only watch/ingest sources whose path or URL contains this substring |
<!-- END GENERATED: flags-watch -->

Without `--interval`, only local filesystem changes trigger re-ingest.
Remote sources are not monitored.

Poll all sources every hour (no local sources required):

```bash
lore watch --interval 3600
```

Watch local files and poll all sources hourly:

```bash
lore watch --debounce 2 --interval 3600
```

### preview

Preview how documents will be extracted and chunked, without writing to
the store. Works with local `path:` sources only (not URL, git, S3, or
other remote source types). Useful for tuning `max_chunk_chars`, `drop_sections`,
and verifying metadata extraction.

```bash
# Preview first 5 documents in a directory
lore preview ./docs

# Preview more documents
lore preview ./docs --limit 10

# Paginate through results
lore preview ./docs --offset 5 --limit 10

# Use config for processing settings (chunk size, drop_sections, etc.)
lore --config .lore/lore.yaml preview ./docs
```

For each document, preview shows: source path, topic, title, author,
language, creation date, tags, chunk count, section headings, and a
content snippet.

<!-- BEGIN GENERATED: flags-preview -->
| Flag | Default | Description |
|------|---------|-------------|
| `--limit` | `5` | Maximum number of documents to preview (default: 5) |
| `--offset` | `0` | Number of documents to skip before previewing (default: 0) |
| `--chunks` | `3` | Maximum chunks to show per document (default: 3) |
| `--json` |  | Output JSON instead of text (NDJSON: one JSON object per line) |
| `--pager` |  | Pipe output through an external pager (e.g. "less -R", "bat") |
| `--no-pager` |  | Disable the pager even when LORE_PAGER or PAGER is set |
<!-- END GENERATED: flags-preview -->

### enrich

Re-run LLM enrichment on already-indexed documents without re-fetching
content. Requires a build with `--features llm` and an `llm` block in the
config file.

```bash
# Enrich all un-enriched documents
lore enrich

# Only process documents in a specific source path
lore enrich --source docs/api

# Only process documents in a specific topic
lore enrich --topic "Authentication"

# Re-enrich documents that already have LLM data
lore enrich --force
```

For each matching document, `lore enrich` reads the existing chunks from
the index, calls the LLM for topic detection, chunk enrichment (summaries,
tags, quality scores), and a document-level summary, then writes the
enriched chunks back to the index. Content is not re-fetched from the
original source.

By default, documents that already have an `avg_llm_quality_score` are
skipped. Pass `--force` to re-enrich them.

Requires `--features llm` at build time (see
[Configuration](./configuration.md#llm)).

<!-- BEGIN GENERATED: flags-enrich -->
| Flag | Description |
|------|-------------|
| `--source` | Only enrich documents matching this source path substring |
| `--topic` | Only enrich documents in this topic |
| `--force` | Re-enrich documents that already have LLM data |
<!-- END GENERATED: flags-enrich -->

### status

Show what would change on the next ingest without writing anything.
Compares file timestamps and sizes on disk against the stored stamps --
no content is read or hashed.

```bash
# Show added, changed, and deleted local files
lore status

# Also check remote sources for changes (network requests)
lore status --remote

# JSON output for scripting
lore status --json
```

By default only local file sources are checked (via stat). Non-local
sources (URLs, git, S3, feeds, sitemaps, YouTube, exec, MCP) are skipped
and counted in the summary. Use `--remote` to query each remote source for 
changes -- this makes network requests but does not download content or 
modify the store.

<!-- BEGIN GENERATED: flags-status -->
| Flag | Description |
|------|-------------|
| `--json` | Output JSON instead of status lines |
| `--remote` | Check remote sources for changes (network requests) |
<!-- END GENERATED: flags-status -->

### completions

Generate shell completion scripts and print to stdout.

```bash
# zsh (add to fpath)
lore completions zsh > ~/.zfunc/_lore

# bash
lore completions bash > ~/.local/share/bash-completion/completions/lore

# fish
lore completions fish > ~/.config/fish/completions/lore.fish

# PowerShell (add to $PROFILE)
lore completions powershell >> $PROFILE
```

Supported shells: `bash`, `elvish`, `fish`, `powershell`, `zsh`.

### maintain

Store maintenance: consistency checks, repair, optimization, and
cache management.

```bash
lore maintain          # check store consistency (read-only)
lore maintain check    # same as bare `lore maintain`
lore maintain repair   # fix detected issues
lore maintain compact  # merge index segments
lore maintain clean    # clear download cache
```

#### maintain check

Check store consistency (read-only). Detects orphaned chunks, missing
chunks, and count mismatches between the document registry and the search
index. This is the default when no subcommand is given.

```bash
lore maintain check
lore maintain check --json
```

<!-- BEGIN GENERATED: flags-maintain-check -->
| Flag | Description |
|------|-------------|
| `--json` | Output JSON instead of text |
<!-- END GENERATED: flags-maintain-check -->

#### maintain repair

Fix detected consistency issues. Runs check first, then applies fixes.
Orphaned chunks are removed from the index, missing-chunk document entries
are removed from the registry. Count mismatches require
`lore ingest --recreate` to resolve.

```bash
lore maintain repair
lore maintain repair --json
```

<!-- BEGIN GENERATED: flags-maintain-repair -->
| Flag | Description |
|------|-------------|
| `--json` | Output JSON instead of text |
<!-- END GENERATED: flags-maintain-repair -->

#### maintain compact

Merge index segments for lower memory usage and faster reads.

```bash
lore maintain compact
```

#### maintain clean

Clear cached data. The cache is global (shared across all knowledge bases).

```bash
# Clear everything
lore maintain clean

# Clear specific scope
lore maintain clean http      # HTTP response cache
lore maintain clean repos     # cloned git repositories
lore maintain clean archives  # extracted archive contents
lore maintain clean tmp       # temporary files
```

<!-- BEGIN GENERATED: enum-cache-scope -->
| Scope | Description |
|-------|-------------|
| `all` | Clear all cached data (default). |
| `archives` | Extracted archive contents. |
| `http` | HTTP responses with ETag/Last-Modified support. |
| `logs` | Ingest failure logs. |
| `repos` | Bare clones of git repositories. |
| `tmp` | Temporary files (cleaned up automatically). |
<!-- END GENERATED: enum-cache-scope -->

## Global Options

<!-- BEGIN GENERATED: flags-global -->
| Flag | Description |
|------|-------------|
| `-c, --config` | Config file path (repeatable for multi-KB federation; default: .lore/lore.yaml, or LORE_CONFIG) |
<!-- END GENERATED: flags-global -->

## Exit Codes

| Code | Description |
|------|-------------|
| 0 | Success |
| 1 | Runtime error (ingestion failure, missing config, invalid arguments) |

## stdin and stdout

lore follows Unix conventions for standard streams:

- **stdout**: Command output (`search`, `topics`, `docs`, `read`, `info`,
  `status`, `serve` MCP protocol, `preview`)
- **stderr**: Progress bars, status messages, errors, and log output

Progress bars are automatically disabled when stderr is not a terminal.
Use `RUST_LOG` to control log verbosity (e.g. `RUST_LOG=debug`).

See also: [Configuration Reference](./configuration.md),
[MCP Integration](./mcp-integration.md),
[Supported Formats](./formats.md).
