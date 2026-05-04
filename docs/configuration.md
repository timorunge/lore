# Configuration Reference

How to configure lore with a YAML file, and what every setting does.

## How Configuration Works

lore looks for a config file in the following order:

1. `--config` CLI flag (explicit path)
2. `LORE_CONFIG` environment variable
3. Auto-discovery in the current directory: `.lore/lore.yaml`, `.lore/lore.yml`,
   `lore.yaml`, `lore.yml`

Settings are resolved with this precedence (highest wins):

1. `LORE_*` environment variables
2. Project config (lore.yaml)
3. Global user config (`~/.config/lore/config.yaml` or platform equivalent)
4. Built-in defaults

The recommended setup is a `.lore/` directory at the root of your project.
Run `lore init` to scaffold it automatically:

```
my-project/
  .lore/
    lore.yaml  # config
    store/     # created by lore ingest (default store path)
  src/
  docs/
```

Since the config lives inside `.lore/`, all relative paths resolve from
that directory. `lore init` sets `base_dir: ..` so that source paths like
`./docs` resolve from the project root instead of from `.lore/`.

For user-level defaults that apply across all projects (LLM provider, fetch
concurrency, memory tuning), see [Global Config](#global-config).

## Minimal Example

Most setups only need a name and one source. lore uses sensible defaults
for everything else:

```yaml
name: My Docs

base_dir: ..

sources:
  - path: ./docs
    glob: "**/*.md"
```

`base_dir: ..` tells lore to resolve source paths from the project root (one
level up from `.lore/`). This indexes all Markdown files under `docs/`,
chunks them by heading structure, and stores everything in `.lore/store`.

## Complete Reference

All available fields with their default values:

```yaml
name: My Knowledge Base
description: Optional description (shown in lore info and MCP server instructions)

base_dir: ..                          # resolve source paths one level up from .lore/ (default: ".")

sources:
  # Local files or directories
  - path: ./docs
    glob: "**/*.md"                   # default: **/* (all files)
    topic: Architecture               # optional grouping label

  # Single URL
  - url: https://example.com/guide.html
    topic: Guides
    headers:                          # optional HTTP headers (LORE_* vars only)
      Authorization: "Bearer ${LORE_API_TOKEN}"

  # Multiple URLs (same `url` key, list value)
  - url:
      - https://example.com/page1.html
      - https://example.com/page2.html

  # Git repository
  - git: https://github.com/user/repo
    ref: main                         # branch, tag, or commit
    glob: "**/*.md"                   # default: **/*.md

  # XML sitemap (discovers and fetches all listed pages)
  - sitemap: https://example.com/sitemap.xml
    include: "/docs/"                 # regex filter on URLs

  # RSS or Atom feed (fetches linked articles)
  - feed: https://example.com/rss.xml
    include: "/blog/"                 # regex filter on URLs
    discard: false                    # remove entries from index when they disappear from feed (default: false)

  # S3 bucket (requires --features s3)
  - s3: s3://my-bucket/prefix
    glob: "**/*.pdf"
    include: "/reports/202[1-6]"      # regex filter on keys

  # YouTube video or playlist
  - youtube: https://www.youtube.com/watch?v=JZfJTSlhOXM
    lang: en

  # Shell command (reads JSONL from stdout)
  - exec: ./scripts/fetch-docs.sh
    topic: Generated

  # Upstream MCP server (requires --features mcp)
  - mcp: npx -y @internal/wiki-mcp-server
    resources: all
    topic: "internal-wiki"

store:
  path: ./store                       # default: ./store (relative to config file)
  writer_heap_mb: 256                 # default: 256 (Tantivy writer heap in MB)
  phrase_search: false                # default: false (skip token positions; ~18% smaller index)
  language: english                   # default: english (stemmer + stop words language)
  doc_store_cache_blocks: 500         # default: 500 (LRU cache in 16 KB blocks; 500 = 8 MB)

processing:
  max_chunk_chars: 1600               # default: 1600
  min_chunk_chars: 30                 # default: 30
  max_file_mb: 50                     # default: 50 (max local file size in MB)
  max_archive_files: 5000             # default: 5000 (max files from a single archive)
  max_archive_mb: 2048                # default: 2048 (max total MB from a single archive)
  extraction_timeout_secs: 120        # default: 120 (timeout per extraction call)
  concurrency: 4                      # default: half of CPU cores, clamped to [2, 32]
  commit_interval: 30                 # default: 30 (seconds between commits; 0 = only at end)
  drop_sections:                      # headings whose content to discard entirely
    - References
    - See Also
  text_extensions:                    # extra file extensions to treat as plain text
    - mdx
    - astro
  content_filter:                     # controls what kreuzberg strips during extraction
    include_headers: false            # default: false (strip running headers)
    include_footers: false            # default: false (strip running footers)
    strip_repeating_text: true        # default: true (strip cross-page repeated text)
    include_watermarks: false         # default: false (strip watermarks)
  extract: auto                       # default: auto (auto|builtin|kreuzberg|none)
  pipeline: []                        # default: [] (ordered list of transform steps)
  presets:                            # named profiles for per-source overrides
    code:
      extract: none
      max_chunk_chars: 800
      pipeline:
        - type: extract_builtin

fetch:
  delay: 0.5                          # default: 0.5 (seconds between requests per host)
  concurrency: 4                      # default: half of CPU cores, clamped to [2, 32]
  timeout: 30.0                       # default: 30.0 (seconds)
  download_timeout: 600               # default: 600 (total download timeout in seconds)
  max_retries: 3                      # default: 3 (retries on 429/503 responses)
  respect_robots: true                # default: true
  user_agent: "my-bot/1.0"            # default: null (uses lore's built-in agent string)
  prefer_markdown: true               # default: true (request markdown via Accept header)
  cache_ttl: 3600.0                   # default: 3600.0 (seconds, null to disable)
  max_download_mb: 50                 # default: 50 (max HTTP response size in MB)

# Requires --features llm
llm:
  provider: ollama                    # ollama, bedrock, anthropic, or openai
  ollama_model: llama3.2              # default: llama3.2
  ollama_url: http://localhost:11434  # default: http://localhost:11434
  bedrock_model: anthropic.claude-haiku-4-5-20251001-v1:0
  anthropic_model: claude-haiku-4-5-20251001
  openai_model: gpt-4o-mini
  detect_topics:
    enabled: true                     # default: false (must opt in)
    max_tokens: 50                    # default: 50
    max_content_chars: 1500           # default: 1500
    prompt: "Custom system prompt for topic detection..."
  summarize_docs:
    enabled: true                     # default: false (must opt in)
    max_tokens: 150                   # default: 150
    max_content_chars: 4000           # default: 4000
    prompt: "Custom system prompt for document summarization..."
  enrich_chunks:
    enabled: true                     # default: false (must opt in)
    quality_threshold: 0.3            # default: 0.3
    concurrency: 4                    # default: half of CPU cores
    max_tokens: 200                   # default: 200
    max_content_chars: 2000           # default: 2000
    prompt: "Custom system prompt for chunk enrichment..."
```

## Field Reference

### base_dir

Optional. Controls the base directory from which local source `path` values
are resolved. The `base_dir` value itself is resolved relative to the config
file's directory. When omitted, defaults to `"."` (the config file's own
directory).

| Value | Effect |
|-------|--------|
| *(omitted)* | Source paths resolve from the config file's directory. |
| `..` | Source paths resolve from the parent of the config directory. Use when the config is in `.lore/`. |
| `/absolute/path` | Source paths resolve from the given absolute directory. |

`base_dir` affects **local sources only** -- all other source types (URL,
git, sitemap, feed, S3, YouTube, maildir, exec, MCP) are unaffected. The 
`store.path` is always relative to the config file's directory, not `base_dir`.

`lore init` sets `base_dir: ..` automatically when it creates a config in
`.lore/`.

### sources

Each source is a list entry under `sources:`. The type is determined by
which key is present (`path`, `url`, `git`, `sitemap`, `feed`, `s3`,
`youtube`, `maildir`, `exec`, or `mcp`).
All source types accept an optional `topic` field.

All source type keys accept either a single string or a list of strings.

See [Source Types](#source-types) for details on each type.

### store

Controls the Tantivy index location and settings.

<!-- BEGIN GENERATED: config-store -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | string | `"./store"` | Store directory path, relative to the config file (default: `"./store"`). |
| `writer_heap_mb` | int | `256` | Tantivy writer heap size in MB (default: 256). Divided across indexing threads. Larger values reduce segment flushes and speed up bulk ingests at the cost of higher memory use. |
| `phrase_search` | bool | `false` | Enable phrase query support (default: false). When false, uses `WithFreqs` instead of `WithFreqsAndPositions`, reducing index size ~18% and speeding up indexing. Exact phrase queries (`"like this"`) won't work; BM25 ranking is unaffected. Only takes effect on `--recreate` or first-time store creation. |
| `language` | string | `"english"` | Stemming and stop-word language (default: english). Only takes effect on `--recreate` or first-time store creation. |
| `doc_store_cache_blocks` | int | `500` | Tantivy document store LRU cache size in 16 KB blocks (default: 500). Higher values trade memory for faster random-access reads. |
<!-- END GENERATED: config-store -->

### processing

Controls how documents are chunked and processed.

<!-- BEGIN GENERATED: config-processing -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `extract` | string | `"auto"` | Metadata extraction mode: auto, builtin, kreuzberg, or none. |
| `max_chunk_chars` | int | `1600` | Maximum characters per chunk (passed to kreuzberg). |
| `min_chunk_chars` | int | `30` | Merge chunks shorter than this into the next chunk. |
| `drop_sections` | list of strings | `[]` | Drop chunks under these section headings (case-insensitive). |
| `pipeline` | list of objects | `[]` | Ordered content/metadata transform pipeline. |
| `max_file_mb` | int | `50` | Maximum file size in MB. Files larger than this are skipped. |
| `max_archive_files` | int | `5000` | Maximum number of files extracted from a single archive (default: 5,000). |
| `max_archive_mb` | int | `2048` | Maximum total decompressed size of an archive in MB (default: 2,048 = 2 GB). |
| `extraction_timeout_secs` | int | `120` | Timeout in seconds for file content extraction (default: 120). |
| `commit_interval` | int | `30` | Seconds between automatic commits during ingest (default: 30). Commits persist progress so cancelled ingests can resume. Set to 0 to commit only at the end. |
| `concurrency` | int | half cores | Maximum number of parallel chunking threads (default: half of available cores, min 2). |
| `text_extensions` | list of strings | `null` | Treat files with these extensions as plain text, bypassing kreuzberg extraction. |
| `content_filter` | object | see below | Content filtering for document extraction (headers, footers, repeating text, watermarks). Controls what kreuzberg strips from extracted content. |
| `presets` | map | `{}` | Named processing presets, reusable via per-source `processing: "name"`. |
<!-- END GENERATED: config-processing -->

**content_filter** (extraction filtering):

Controls what "furniture" content kreuzberg strips during extraction.
When omitted, kreuzberg defaults apply. Set individual fields to override.

<!-- BEGIN GENERATED: config-content-filter -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `include_headers` | bool | `false` | Include running headers in extracted text (default: false). |
| `include_footers` | bool | `false` | Include running footers in extracted text (default: false). |
| `strip_repeating_text` | bool | `true` | Strip text that repeats across a supermajority of pages (default: true). Disable if brand names or repeated headings are incorrectly removed from PowerPoint-exported PDFs. |
| `include_watermarks` | bool | `false` | Include watermark text in extracted output (default: false). |
<!-- END GENERATED: config-content-filter -->

### Processing Profiles

A processing profile is a per-source override for a subset of processing
settings: `extract`, `max_chunk_chars`, `min_chunk_chars`, `drop_sections`,
and `pipeline`. Profiles let individual sources deviate from
the global `processing` defaults without repeating every field.

**Resolution order**

```
source has `processing: "code"` -> look up in processing.presets
source has `processing: { ... }` -> use inline profile
source has no `processing:`      -> use global defaults
```

**Profile fields**

<!-- BEGIN GENERATED: config-processing-profile -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `extract` | string | `"auto"` | Metadata extraction mode: auto, builtin, kreuzberg, or none. |
| `max_chunk_chars` | int | `1600` | Maximum characters per chunk (passed to kreuzberg). |
| `min_chunk_chars` | int | `30` | Merge chunks shorter than this into the next chunk. |
| `drop_sections` | list of strings | `[]` | Drop chunks under these section headings (case-insensitive). |
| `pipeline` | list of objects | `[]` | Ordered content/metadata transform pipeline. |
<!-- END GENERATED: config-processing-profile -->

Named presets are complete profiles -- unspecified fields use the profile
defaults shown above, not the global `processing` values. The same applies
to inline profile objects. Only the `None` path (no `processing:` key on
the source) inherits the full global `processing` section as defaults.

**Example -- presets and per-source overrides**

```yaml
sources:
  # Uses the "code" preset
  - path: ./src
    glob: "**/*.rs"
    processing: code

  # Inline override -- larger chunks, no extraction
  - path: ./data
    glob: "**/*.csv"
    processing:
      extract: none
      max_chunk_chars: 4000

  # No processing key -- uses global defaults
  - path: ./docs
    glob: "**/*.md"

processing:
  max_chunk_chars: 1600
  min_chunk_chars: 30
  extract: auto

  presets:
    code:
      extract: none
      max_chunk_chars: 800
      min_chunk_chars: 20
      pipeline:
        - type: strip_lines
          first: 5
        - type: extract_builtin

    binary-only:
      extract: kreuzberg
      drop_sections:
        - References
```

### Extraction Modes

The `extract` field controls how metadata (title, author, date, etc.) is
extracted from source files. Text files always have their content read
directly; this setting governs what additional metadata is collected.

<!-- BEGIN GENERATED: enum-extract-mode-detail -->
| Mode | Text files | Binary files |
|------|------------|--------------|
| `auto` | Builtin metadata extraction | Kreuzberg metadata + builtin extraction + merge |
| `builtin` | Builtin metadata extraction only | Builtin metadata extraction only (kreuzberg not called) |
| `kreuzberg` | No metadata extraction | Kreuzberg full extraction (document parsing + metadata) |
| `none` | No metadata extraction | No metadata extraction; use `extract_builtin` in a pipeline for explicit control |
<!-- END GENERATED: enum-extract-mode-detail -->

The default is `auto`, which gives the best trade-off for most knowledge
bases. Use `none` paired with a pipeline `extract_builtin` step when you need
to control exactly when extraction happens relative to other transforms.

### Pipeline Transforms

The `pipeline` field (both global and per-profile) is an ordered list of
transforms applied to each document after loading. Transforms run in
declaration order and can modify content or fill metadata fields.

**Transform types**

<!-- BEGIN GENERATED: enum-transform-types -->
| Type | Fields | Effect |
|------|--------|--------|
| `strip_lines` | `first`, `last` | Strip leading and/or trailing lines from content. |
| `replace_text` | `pattern`, `replacement` | Regex find-and-replace on content. |
| `extract_builtin` | (none) | Run builtin metadata extraction (YAML frontmatter, org-mode, RFC headers, heuristics) on current content. First-wins: only fills empty fields. Useful with `extract: none` to control when extraction runs in the pipeline. |
| `extract_metadata` | `pattern`, `field` | Custom regex metadata extraction. First-wins per field. |
<!-- END GENERATED: enum-transform-types -->

**Example**

```yaml
processing:
  pipeline:
    - type: strip_lines
      first: 3           # remove 3-line file header
      last: 1            # remove trailing blank
    - type: replace_text
      pattern: '\b(TODO|FIXME)\b'
      replacement: "NOTE"
    - type: extract_metadata
      pattern: '^Author:\s*(.+)$'
      field: author
    - type: extract_builtin
```

**Pipeline execution order**

```
loaded content
  |
  +- [loader]  extraction per `extract` mode
  |
  +- [freshness check]  unchanged -> skip
  |
  +- [pipeline]  transforms in declaration order
  |   +- strip_lines      -> modifies content
  |   +- replace_text     -> modifies content
  |   +- extract_builtin  -> fills empty metadata from content
  |   +- extract_metadata -> fills empty metadata via regex
  |
  +- [hash]  blake3 of post-transform content
  |
  +- [chunking]  max_chunk_chars, drop_sections
  |
  +- [chunk merge]  min_chunk_chars
  |
  +- [storage]
```

Note: changing `extract`, `pipeline`, or `presets` config without touching
source files requires `--force` to re-process (bypasses freshness checks
without wiping the store). Use `--recreate` to fully rebuild from scratch.

### fetch

Controls HTTP fetching behavior for URL, sitemap, feed, and git sources.

<!-- BEGIN GENERATED: config-fetch -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `delay` | float | `0.5` | Seconds between requests to the same host (default: 0.5). |
| `concurrency` | int | half cores | Maximum parallel HTTP requests (default: half of available cores). |
| `timeout` | float | `30` | HTTP request timeout in seconds (default: 30.0). |
| `download_timeout` | int | `600` | Total timeout for streaming downloads in seconds (default: 600). |
| `max_retries` | int | `3` | Maximum retry attempts for rate-limited (429/503) responses (default: 3). |
| `respect_robots` | bool | `true` | Honour robots.txt rules (default: true). |
| `user_agent` | string | -- | Custom User-Agent header for HTTP requests. When `None`, uses lore's built-in agent string. |
| `prefer_markdown` | bool | `true` | Request markdown via Accept header (Cloudflare "Markdown for Agents"). When true, sends `Accept: text/markdown, text/html;q=0.9, */*;q=0.8`. |
| `cache_ttl` | float | `3600` | Cache TTL in seconds; `None` disables caching (default: 3600). |
| `max_download_mb` | int | `50` | Maximum HTTP response/download size in MB (default: 50). |
<!-- END GENERATED: config-fetch -->

### llm

Optional. Requires building with `--features llm`. When enabled, lore uses
an LLM to assign topic labels, enrich chunks with summaries, keywords, and
quality scores. All four providers (Ollama, Anthropic, OpenAI, Bedrock) are
included. The `llm` block is used by `lore ingest` (enrichment during
indexing) and `lore enrich` (standalone re-enrichment).

```bash
cargo install --path . --features llm
```

If lore is built without the `llm` feature (the default), any `llm` section
in the config file causes a validation error at startup.

<!-- BEGIN GENERATED: config-llm -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `provider` | string | -- | LLM provider; auto-detected from available credentials when `None`. |
| `bedrock_model` | string | `"anthropic.claude-haiku-4-5-20251001-v1:0"` | Model ID for AWS Bedrock. |
| `anthropic_model` | string | `"claude-haiku-4-5-20251001"` | Model ID for the Anthropic API. |
| `openai_model` | string | `"gpt-4o-mini"` | Model ID for the OpenAI-compatible API. |
| `ollama_model` | string | `"llama3.2"` | Model ID for the local Ollama instance. |
| `ollama_url` | string | `"http://localhost:11434"` | Base URL for the local Ollama instance. Defaults to `http://localhost:11434` when the `llm` block is present. |
| `timeout_secs` | int | `120` | Total request timeout in seconds for LLM API calls (default: 120). |
| `connect_timeout_secs` | int | `10` | TCP connect timeout in seconds for LLM API calls (default: 10). |
<!-- END GENERATED: config-llm -->

**detect_topics** (topic detection):

<!-- BEGIN GENERATED: config-llm-detect-topics -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable LLM topic detection (default: false). |
| `max_tokens` | int | `50` | Maximum tokens for the LLM response (default: 50). |
| `max_content_chars` | int | `1500` | Maximum document characters sent to the LLM (default: 1500). |
| `prompt` | string | -- | Custom system prompt override for topic detection. |
<!-- END GENERATED: config-llm-detect-topics -->

**summarize_docs** (document-level summary):

<!-- BEGIN GENERATED: config-llm-summarize-docs -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable LLM document summarization (default: false). |
| `max_tokens` | int | `150` | Maximum tokens for the LLM response (default: 150). |
| `max_content_chars` | int | `4000` | Maximum document characters sent to the LLM (default: 4000). |
| `prompt` | string | -- | Custom system prompt override for document summarization. |
<!-- END GENERATED: config-llm-summarize-docs -->

**enrich_chunks** (chunk enrichment):

<!-- BEGIN GENERATED: config-llm-enrich-chunks -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable LLM enrichment (summaries, tags, quality scoring) (default: false). |
| `quality_threshold` | float | `0.3` | Minimum quality score (0.0--1.0) for a document to be enriched. |
| `concurrency` | int | half cores | Maximum parallel LLM requests (default: half of available cores). |
| `max_tokens` | int | `200` | Maximum tokens for the LLM response (default: 200). |
| `max_content_chars` | int | `2000` | Maximum document characters sent to the LLM (default: 2000). |
| `prompt` | string | -- | Custom system prompt override for enrichment. |
<!-- END GENERATED: config-llm-enrich-chunks -->

## Source Types

<!-- BEGIN GENERATED: enum-source-types -->
| Type | Key | Description |
|------|-----|-------------|
| Local | `path` | Local file(s) or directory(ies). Supports `glob` patterns. Archives (zip, tar, gz, etc.) are extracted automatically. |
| Git | `git` | Git repository URL(s). Cloned/fetched to a local cache. `ref` selects a branch, tag, or commit. |
| S3 | `s3` | S3 URI(s) (`s3://bucket/prefix`). Requires the `s3` feature flag. `glob` and `include` filter keys. |
| URL | `url` | Single HTTP/HTTPS URL, or a list of URLs. Archive content types are detected and extracted. |
| Sitemap | `sitemap` | XML sitemap URL(s). Discovers and fetches all listed pages. `include` filters URLs by regex. |
| Feed | `feed` | RSS or Atom feed URL(s). Fetches linked articles. `discard: true` removes old entries on update. |
| YouTube | `youtube` | YouTube video, playlist, or channel URL(s). Fetches transcripts via `yt-dlp`. |
| Maildir | `maildir` | Maildir directory (or directories). Indexes all messages in `new/` and `cur/`. |
| Exec | `exec` | Command(s) to execute. stdout is read as JSONL; each line becomes a document. |
| MCP | `mcp` | Upstream MCP server source. Connects to an MCP server and ingests resources and/or tool call results. Requires the `mcp` feature. |
<!-- END GENERATED: enum-source-types -->

Source blocks are identified by their primary key (`path`, `git`, `s3`,
`url`, `sitemap`, `feed`, `youtube`, `maildir`, `exec`, `mcp`).
Misspelled keys produce targeted error messages with suggestions.

### Update mode

Every source type accepts an optional `update` field that controls how it
is refreshed during incremental ingests (`lore ingest` without `--recreate`):

<!-- BEGIN GENERATED: enum-update-mode -->
| Mode | Behavior |
|------|----------|
| `auto` | Smart freshness detection -- mtime+size for local files, ETags and content hashes for remote sources. This is the default. |
| `always` | Always re-fetch and re-process the source, bypassing all freshness checks. Equivalent to `--force` but scoped to a single source. |
| `never` | Skip the source on incremental ingests once it has been indexed. Existing documents are protected from stale-document cleanup even if the source becomes unreachable. |
<!-- END GENERATED: enum-update-mode -->

A source with `update: never` is still processed on its first ingest (when
no documents from it exist in the store yet). The `--force` flag and
`--recreate` both override `update: never`.

```yaml
sources:
  - git: https://github.com/org/archive-repo
    update: never       # index once, then preserve
    topic: Archive

  - feed: https://blog.example.com/feed.xml
    update: always      # always re-fetch the feed
    discard: true
```

### Tags

Lore supports two independent tagging layers. Both are stored in the index
and searchable.

**Document tags** (`tags` field on chunks) come from two places:

1. **Source config** -- set `tags` on any source to apply static labels to
   every document from that source.
2. **Loader-extracted** -- some loaders produce tags automatically (e.g.,
   maildir flags like `seen`, `replied`, `flagged`).

When both are present, they are merged: loader tags appear first, followed by
the config tags (comma-separated). Config tags alone require no LLM.

```yaml
sources:
  - path: ./docs
    tags: "internal, engineering"      # applied to every document

  - maildir: ~/Mail/INBOX
    tags: "inbox"                      # merged with maildir flags
    # a flagged message gets: "seen, flagged, inbox"
```

**LLM tags** (`llm_tags` field on chunks) are keyword tags generated per
chunk by the LLM enrichment pipeline. These are only produced when
`llm.enrich_chunks.enabled` is `true` and an LLM provider is configured.
To disable LLM-generated tags, either omit the `llm` block entirely or set:

```yaml
llm:
  enrich_chunks:
    enabled: false     # default -- no LLM tags or summaries
```

The two layers are independent: you can use config tags without any LLM,
use LLM tags without config tags, or combine both.

### Local source

```yaml
- path: ./docs
  glob: "**/*.md"
  topic: Documentation
  processing: code        # reference a named preset

- path: ./src
  glob: "**/*.rs"
  topic: Source
  processing:             # or an inline profile
    extract: none
    max_chunk_chars: 800
```

```yaml
# multiple paths in one source
- path:
    - ./docs
    - ./guides
  glob: "**/*.md"
  topic: Documentation
```

<!-- BEGIN GENERATED: config-source-local -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | string or list | -- | Path(s) to files or directories (string or list). Supports `~` and `$VAR` expansion. |
| `glob` | string | -- | Glob pattern relative to `path`. Only matching files are ingested (default: `"**/*"`). |
| `update` | string | -- | Update mode: auto, always, or never (default: auto). |
| `topic` | string | -- | Topic label applied to all chunks from this source. |
| `tags` | string | -- | Comma-separated tags applied to all documents from this source. |
| `processing` | string or object | -- | Processing override: a preset name (`"code"`) or an inline profile object. |
<!-- END GENERATED: config-source-local -->

When a path entry points to a single file, that file is ingested directly.
When `path` is a directory, lore walks it respecting `.gitignore` rules
and matches files against the `glob` pattern (default: `**/*`).

Archives (zip, tar, tar.gz, tar.bz2, tar.xz, tar.zst) are automatically
extracted and each file inside is processed individually.

### Git source

```yaml
- git: https://github.com/user/repo
  ref: main
  glob: "**/*.md"
  processing: code          # reference a named preset
```

```yaml
# multiple repositories
- git:
    - https://github.com/user/repo1
    - https://github.com/user/repo2
  glob: "**/*.md"
```

<!-- BEGIN GENERATED: config-source-git -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `git` | string or list | -- | Repository URL(s) (string or list). Allowed schemes: `https`, `http`, `ssh`, `git`. |
| `ref` | string | -- | Branch, tag, or commit to check out. |
| `glob` | string | -- | Glob pattern for files to ingest (default: `"**/*.md"`). |
| `update` | string | -- | Update mode: auto, always, or never (default: auto). |
| `topic` | string | -- | Topic label applied to all chunks from this source. |
| `tags` | string | -- | Comma-separated tags applied to all documents from this source. |
| `processing` | string or object | -- | Processing override: a preset name or an inline profile object. |
<!-- END GENERATED: config-source-git -->

Clones (or fetches updates for) the repository into the global cache.
Only re-processes when HEAD changes. Allowed schemes: `https`, `http`,
`ssh`, `git`.

lore disables interactive credential prompts (`GIT_TERMINAL_PROMPT=0`).
For private repositories or repos on company networks, configure
authentication before running `lore ingest` -- for example via SSH keys,
a Git credential helper, or a personal access token in the URL
(`https://token@host/repo`). Without valid credentials, git will fail
immediately rather than prompt.

### S3 source

```yaml
- s3: s3://my-bucket/docs/
  glob: "**/*.pdf"
  include: "\.pdf$"
  topic: PDFs
  processing:               # inline profile for binary-heavy sources
    extract: kreuzberg
    max_chunk_chars: 2000
```

```yaml
# multiple S3 prefixes
- s3:
    - s3://my-bucket/docs/
    - s3://my-bucket/guides/
  glob: "**/*.pdf"
```

<!-- BEGIN GENERATED: config-source-s3 -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `s3` | string or list | -- | S3 bucket URI(s) in `s3://bucket/prefix` format (string or list). |
| `glob` | string | -- | Glob pattern for objects to ingest. |
| `include` | string | -- | Regex filter applied to object keys. Only matching objects are fetched. |
| `update` | string | -- | Update mode: auto, always, or never (default: auto). |
| `topic` | string | -- | Topic label applied to all chunks from this source. |
| `tags` | string | -- | Comma-separated tags applied to all documents from this source. |
| `processing` | string or object | -- | Processing override: a preset name or an inline profile object. |
<!-- END GENERATED: config-source-s3 -->

Requires the `s3` feature flag. Lists objects under the given prefix,
filters by `glob` and `include` regex, and downloads each object. Uses
standard AWS credential resolution (env vars, profiles, IAM roles).

On update ingests, objects with unchanged ETags are skipped entirely.
Downloaded objects are cached locally with ETag-based freshness checks,
so subsequent runs avoid re-downloading unchanged files.

### URL source

```yaml
- url: https://example.com/guide.html
  topic: Guides
  processing: binary-only   # reference a named preset
```

Multiple URLs use the same `url` key with a list value:

```yaml
- url:
    - https://example.com/page1.html
    - https://example.com/page2.html
  processing:               # inline profile applies to all URLs in the list
    extract: builtin
```

<!-- BEGIN GENERATED: config-source-url -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `url` | string or list | -- | One or more HTTP/HTTPS URLs to fetch (string or list). |
| `headers` | map | `{}` | HTTP headers for requests. Values support `${LORE_*}` env var expansion. |
| `update` | string | -- | Update mode: auto, always, or never (default: auto). |
| `topic` | string | -- | Topic label applied to all chunks from this source. |
| `tags` | string | -- | Comma-separated tags applied to all documents from this source. |
| `processing` | string or object | -- | Processing override: a preset name or an inline profile object. |
<!-- END GENERATED: config-source-url -->

Fetches one or more pages. If the response content type indicates an archive,
the archive is extracted and its contents are indexed.

### HTTP headers (authentication)

URL-based sources (`url`, `sitemap`, `feed`) accept an optional
`headers` map. Header values support `$VAR` and `${VAR}` expansion,
but only for environment variables with the `LORE_` prefix (to prevent
accidental exfiltration of sensitive env vars):

```yaml
- sitemap: https://internal.docs.corp/sitemap.xml
  headers:
    Authorization: "Bearer ${LORE_DOCS_TOKEN}"
    X-Api-Key: "${LORE_API_KEY}"
  topic: internal-docs
```

Headers are sent on every HTTP request for that source (discovery and
content fetching). They are not sent to robots.txt endpoints.

Git and S3 sources do not support `headers` -- git uses the system
credential helper chain (osxkeychain, `gh auth`, SSH keys) and S3 uses
the standard AWS credential chain.

### Sitemap source

```yaml
- sitemap: https://example.com/sitemap.xml
  include: "/docs/"
  processing:
    extract: builtin
```

```yaml
# multiple sitemaps
- sitemap:
    - https://example.com/sitemap.xml
    - https://example.com/docs-sitemap.xml
  include: "/docs/"
```

<!-- BEGIN GENERATED: config-source-sitemap -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `sitemap` | string or list | -- | XML sitemap URL(s) (string or list). |
| `include` | string | -- | Regex filter applied to discovered URLs. Only matching URLs are fetched. |
| `headers` | map | `{}` | HTTP headers for requests. Values support `${LORE_*}` env var expansion. |
| `update` | string | -- | Update mode: auto, always, or never (default: auto). |
| `topic` | string | -- | Topic label applied to all chunks from this source. |
| `tags` | string | -- | Comma-separated tags applied to all documents from this source. |
| `processing` | string or object | -- | Processing override: a preset name or an inline profile object. |
<!-- END GENERATED: config-source-sitemap -->

Discovers all URLs in the sitemap (including nested sitemaps) and fetches
each page. The `include` regex filters which URLs to process.

### Feed source

```yaml
- feed: https://example.com/rss.xml
  include: "/blog/"
  discard: false
  processing: code          # reference a named preset
```

```yaml
# multiple feeds
- feed:
    - https://example.com/rss.xml
    - https://example.com/blog/feed.xml
  include: "/blog/"
```

<!-- BEGIN GENERATED: config-source-feed -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `feed` | string or list | -- | RSS/Atom feed URL(s) (string or list). |
| `include` | string | -- | Regex filter applied to entry URLs. Only matching entries are fetched. |
| `discard` | bool | `false` | Remove documents from the index when their entries disappear from the feed (default: false). |
| `headers` | map | `{}` | HTTP headers for requests. Values support `${LORE_*}` env var expansion. |
| `update` | string | -- | Update mode: auto, always, or never (default: auto). |
| `topic` | string | -- | Topic label applied to all chunks from this source. |
| `tags` | string | -- | Comma-separated tags applied to all documents from this source. |
| `processing` | string or object | -- | Processing override: a preset name or an inline profile object. |
<!-- END GENERATED: config-source-feed -->

Fetches articles linked from an RSS or Atom feed. The `include` regex
filters URLs. When `discard: true`, old entries that disappear from the
feed are removed from the index on the next update ingest.

### YouTube source

```yaml
# single video
- youtube: https://www.youtube.com/watch?v=JZfJTSlhOXM
  lang: en
  topic: DSV1

# multiple videos in one source
- youtube:
    - https://www.youtube.com/watch?v=JZfJTSlhOXM
    - https://www.youtube.com/watch?v=OlQ5Z-xwsP8
  topic: DSV1

# playlist
- youtube: https://www.youtube.com/playlist?list=PLh7VQrI2F_FzuUkyc4X8QFyASymyS8DGP
  lang: en
  max_videos: 20
  topic: Skee Mask
```

<!-- BEGIN GENERATED: config-source-youtube -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `youtube` | string or list | -- | YouTube video, playlist, or channel URL(s) (string or list). |
| `lang` | string | `"en"` | Transcript language code (default: `"en"`). |
| `include` | string | -- | Regex filter applied to video titles. Only matching videos are processed. |
| `max_videos` | int | `50` | Maximum videos to fetch from a playlist or channel (default: 50). |
| `update` | string | -- | Update mode: auto, always, or never (default: auto). |
| `topic` | string | -- | Topic label applied to all chunks from this source. |
| `tags` | string | -- | Comma-separated tags applied to all documents from this source. |
| `processing` | string or object | -- | Processing override: a preset name or an inline profile object. |
<!-- END GENERATED: config-source-youtube -->

Fetches video transcripts from YouTube. Accepts single video URLs,
playlist URLs, and channel URLs (`/@handle`, `/channel/`, `/c/`).

This source type shells out to [yt-dlp](https://github.com/yt-dlp/yt-dlp),
which must be installed and available on `PATH`. lore spawns `yt-dlp` as a
subprocess for metadata retrieval, playlist discovery, and subtitle
downloads -- it does not use YouTube's HTTP APIs directly. This keeps lore
resilient to YouTube API changes, since `yt-dlp` is actively maintained and
handles protocol updates independently.

Subtitle selection prefers manual captions over auto-generated ones, and
matches the requested `lang` with exact match, prefix match (e.g. `en`
matches `en-US`), or falls back to any available language. The downloaded
subtitle XML is parsed into plain text and prepended with a metadata header
(title, author, tags, description excerpt).

On update ingests, already-indexed videos are skipped (by source ID).
Use `--force` to re-fetch all transcripts.

### Maildir source

```yaml
# single maildir
- maildir: ~/Mail/INBOX
  topic: Inbox

# multiple maildirs
- maildir:
    - ~/Mail/INBOX
    - ~/Mail/Archive
  skip_trashed: true
  topic: Email
```

<!-- BEGIN GENERATED: config-source-maildir -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `maildir` | string or list | -- | Path(s) to Maildir root directories (string or list). Supports `~` and `$VAR` expansion. |
| `skip_trashed` | bool | `true` | Skip messages with the Trashed (`T`) flag (default: true). |
| `update` | string | -- | Update mode: auto, always, or never (default: auto). |
| `topic` | string | -- | Topic label applied to all chunks from this source. |
| `tags` | string | -- | Comma-separated tags applied to all documents from this source. |
| `processing` | string or object | -- | Processing override: a preset name or an inline profile object. |
<!-- END GENERATED: config-source-maildir -->

Indexes email messages stored in the [Maildir](https://cr.yp.to/proto/maildir.html)
format. Each directory must contain `cur/` and/or `new/` subdirectories --
`tmp/` is always ignored. Messages are parsed directly with `mail-parser`
(no external dependencies).

Maildir filenames in `cur/` carry a `:2,FLAGS` suffix that changes when
flags change (e.g. message marked as read). The source key strips this
suffix so flag changes alone do not trigger re-indexing. Active flags
(seen, replied, flagged, draft, passed) are stored as comma-separated tags.

Freshness is checked via file mtime and size, same as local sources.

> **Other email formats:** `.eml`, `.mbox`, and `.pst` files are handled
> automatically by the local source type -- just point a `path:` entry at
> the file or directory containing them. The maildir source is only needed
> for the Maildir directory layout (extensionless files in `cur/`/`new/`).

### Exec source

```yaml
# single command
- exec: ./scripts/fetch-notes.sh
  topic: Notes

# multiple commands
- exec:
    - python3 scripts/export-wiki.py
    - ./scripts/fetch-tickets.sh
  topic: Internal

# with options
- exec: ./scripts/fetch.sh
  dir: ..
  timeout_secs: 120
  env:
    API_KEY: "secret"
    BASE_URL: https://api.example.com
  topic: External
```

Runs one or more shell commands via `sh -c` and reads JSONL from stdout.

<!-- BEGIN GENERATED: config-source-exec -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `exec` | string or list | -- | Shell command(s) to run via `sh -c` (string or list). |
| `dir` | string | -- | Working directory override (default: config base dir). Supports `~` and `$VAR`. |
| `env` | map | -- | Extra environment variables passed to the subprocess. |
| `timeout_secs` | int | -- | Per-source timeout in seconds (default: `processing.extraction_timeout_secs`). |
| `update` | string | -- | Update mode: auto, always, or never (default: auto). |
| `topic` | string | -- | Topic label applied to all chunks from this source. |
| `tags` | string | -- | Comma-separated tags applied to all documents from this source. |
| `processing` | string or object | -- | Processing override: a preset name or an inline profile object. |
<!-- END GENERATED: config-source-exec -->

Each non-empty stdout line must be a JSON object with at least `source`
(a stable identity key) and `content` (the document text). Optional fields:
`title`, `author`, `lang`, `created_at`, `tags`, `topic`.

```json
{"source": "notes/2026-01-01", "content": "Year in review...", "title": "Retrospective", "tags": ["annual", "review"]}
{"source": "notes/2026-01-15", "content": "Project kickoff...", "title": "Kickoff", "lang": "en"}
```

Commands run with `stdin` connected to `/dev/null`. Invalid JSON lines and
lines missing `source` or `content` are logged as warnings and skipped. A
non-zero exit code or timeout fails the entire source.

Change detection relies on content hashing: lore always runs the command
but skips re-indexing documents whose content hash is unchanged. Use
`--force` to re-index all exec documents regardless of content.

The `source` value from each JSONL line is used directly as the document
identity key. Use a stable, unique identifier (database primary key, file
path, URL). Changing the `source` value causes lore to treat the document
as new; the old version becomes stale and is removed.

### MCP source

```yaml
# Auto-discover all resources from an upstream MCP server
- mcp: npx -y @internal/wiki-mcp-server
  resources: all
  topic: "internal-wiki"

# Specific resource URIs
- mcp: npx -y @internal/wiki-mcp-server
  resources:
    - "wiki://pages/architecture"
    - "wiki://pages/onboarding"
  topic: "wiki-selected"

# Explicit tool calls
- mcp: npx -y @aws/docs-mcp-server
  tools:
    - name: search_docs
      args: { query: "lambda best practices", limit: 50 }
  topic: "aws-docs"

# HTTP transport with bearer token
- mcp: http://internal-kb.corp:8080/mcp
  transport: http
  token: "${LORE_KB_TOKEN}"
  resources: all
  topic: "corp-kb"

# Lore-to-lore ingestion
- mcp: lore serve --transport stdio -c /path/to/other/.lore/lore.yaml
  resources: all
  topic: "upstream-kb"
  tags: "upstream, imported"
```

Connects to an upstream MCP server and ingests content from resources,
tool call results, or both. Requires the `mcp` feature flag.

<!-- BEGIN GENERATED: config-source-mcp -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mcp` | string | -- | Server command (stdio) or URL (http). |
| `transport` | string | -- | Transport protocol: `stdio` (default) or `http`. |
| `resources` | any | -- | Resource discovery mode: `"all"` to auto-discover, or a list of specific resource URIs. |
| `tools` | list of objects | -- | Explicit tool calls to invoke on the upstream server. |
| `env` | map | -- | Extra environment variables passed to the stdio subprocess. |
| `token` | string | -- | Bearer token for HTTP transport. Supports `${LORE_*}` env var expansion. |
| `timeout_secs` | int | -- | Per-session timeout in seconds (default: `processing.extraction_timeout_secs`). |
| `update` | string | -- | Update mode: auto, always, or never (default: auto). |
| `topic` | string | -- | Topic label applied to all chunks from this source. |
| `tags` | string | -- | Comma-separated tags applied to all documents from this source. |
| `processing` | string or object | -- | Processing override: a preset name or an inline profile object. |
<!-- END GENERATED: config-source-mcp -->

Two ingestion modes are supported:

- **Resource auto-discovery** (`resources: all`): calls `list_resources` on the
  upstream server, then reads every discovered resource. Best for content-oriented
  servers (wikis, documentation) where you want everything indexed.
- **Explicit tool calls** (`tools`): invokes specific tools with user-defined
  arguments. Best for targeted extraction or servers that only expose tools.

Both modes can be combined in a single source block.

The `transport` field selects the MCP transport protocol:
- `stdio` (default): spawns the server command as a child process.
- `http`: connects via Streamable HTTP. Use `token` for Bearer authentication;
  the value supports `${LORE_*}` env var expansion.

Change detection relies on content hashing. On re-ingest, documents whose
content hash is unchanged are skipped automatically. Use `--force` to
re-index all MCP documents regardless of content.

## Path Expansion

Local paths, `store.path`, `base_dir`, and exec `dir` support:

- `~` expands to the user's home directory
- `$VAR` and `${VAR}` expand environment variables

Relative local source paths are resolved relative to `base_dir` (which
itself is resolved relative to the config file's directory). When `base_dir`
is omitted, source paths resolve directly from the config file's directory.

`store.path` always resolves relative to the config file's directory,
regardless of `base_dir`.

## Markdown Content Negotiation

When `fetch.prefer_markdown` is enabled, lore sends
`Accept: text/markdown, text/html;q=0.9, */*;q=0.8` on HTTP requests.
Sites behind Cloudflare with
[Markdown for Agents](https://developers.cloudflare.com/fundamentals/reference/markdown-for-agents/)
respond with pre-converted markdown instead of HTML, typically reducing
content size by ~80%. Sites that do not support this fall back to HTML.

## Global Config

lore supports an optional global config file for user-level defaults that
apply to all projects on this machine. Run `lore init --global` to create
it, or create the file manually.

**Precedence:** global config < project config < environment variables.
Project settings always override global defaults. Environment variables
override both.

**Location:**

| Platform | Path |
|----------|------|
| macOS / Linux | `~/.config/lore/config.yaml` |
| Windows | `%APPDATA%\lore\config.yaml` |

Respects `$XDG_CONFIG_HOME` when set. Override the entire path with
`LORE_GLOBAL_CONFIG` environment variable.

**Allowed fields:**

<!-- BEGIN GENERATED: config-global -->
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `store` | object | -- | Default store tuning parameters. |
| `processing` | object | -- | Default processing parameters. |
| `fetch` | object | -- | Default HTTP fetch behavior. |
| `llm` | object | -- | Default LLM provider and model configuration. |
<!-- END GENERATED: config-global -->

Only machine/user-level blocks are allowed (`fetch`, `store`, `processing`,
`llm`). Project-specific fields (`name`, `description`, `base_dir`,
`sources`) are rejected with a clear error.

**Merge behavior:** within each block, global fields fill in only where
the project config has no value. For example, setting `fetch.concurrency: 4`
globally means every project uses 4 unless its own config sets a different
value.

**Example:**

```yaml
# ~/.config/lore/config.yaml
fetch:
  concurrency: 4
  delay: 1.0

store:
  writer_heap_mb: 512

llm:
  provider: ollama
  ollama_url: http://gpu-box:11434
  detect_topics:
    enabled: true
```

## Environment Variables

Environment variables override config file values after parsing.

| Variable | Config path | Type |
|----------|-------------|------|
| `LORE_CONFIG` | (CLI `--config` flag) | path; colon-separated for multi-KB (use absolute paths in MCP client configs) |
| `LORE_STORE_PATH` | `store.path` | path |
| `LORE_STORE_WRITER_HEAP_MB` | `store.writer_heap_mb` | integer |
| `LORE_STORE_PHRASE_SEARCH` | `store.phrase_search` | bool (`true`/`false`) |
| `LORE_STORE_DOC_STORE_CACHE_BLOCKS` | `store.doc_store_cache_blocks` | integer |
| `LORE_FETCH_DELAY` | `fetch.delay` | float (seconds) |
| `LORE_FETCH_CONCURRENCY` | `fetch.concurrency` | integer |
| `LORE_FETCH_TIMEOUT` | `fetch.timeout` | float (seconds) |
| `LORE_FETCH_CACHE_TTL` | `fetch.cache_ttl` | float (seconds) |
| `LORE_MAX_CHUNK_CHARS` | `processing.max_chunk_chars` | integer |
| `LORE_MIN_CHUNK_CHARS` | `processing.min_chunk_chars` | integer |
| `LORE_MAX_FILE_MB` | `processing.max_file_mb` | integer |
| `LORE_GLOBAL_CONFIG` | (global config path override) | path |
| `LORE_CACHE_DIR` | (cache root directory) | path |
| `RUST_LOG` | (log level filter) | filter string |

## Cache

lore maintains a global cache (shared across all knowledge bases) for HTTP
responses, cloned git repositories, and extracted archives.

Default locations:

| Platform | Path |
|----------|------|
| macOS | `~/Library/Caches/lore` |
| Linux | `~/.cache/lore` |

Override with `LORE_CACHE_DIR`.

Subdirectories:

<!-- BEGIN GENERATED: cache-subdirs -->
| Directory | Contents |
|-----------|----------|
| `archives/` | Extracted archive contents. |
| `http/` | HTTP responses with ETag/Last-Modified support. |
| `logs/` | Ingest failure logs. |
| `repos/` | Bare clones of git repositories. |
| `tmp/` | Temporary files (cleaned up automatically). |
<!-- END GENERATED: cache-subdirs -->

Clear the cache with `lore maintain clean` (or a specific scope: `http`,
`repos`, `archives`, `tmp`).

See also: [CLI Reference](./cli.md), [Supported Formats](./formats.md),
[MCP Integration](./mcp-integration.md).
