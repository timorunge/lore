# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **`exec` raw output mode** -- `output: raw` consumes a command's entire
  stdout as one document instead of parsing it as JSONL, with optional
  `source_key` and `format` fields
- **`exec` output limits** -- `max_output_bytes` (default 10 MiB) bounds
  stdout reads and kills the child process on timeout
- **`lore maintain health`** -- one-screen report of config validity,
  compiled features, LLM client initialization, and store reachability
- **Store provenance** -- `lore.meta` records a chunk-config fingerprint
  and the writing version, and warns when the store was built with
  different settings

### Fixed

- `lore status` no longer reports documents inside archives as deleted on
  every run; members are stored as `{archive}#{member}` but only the
  archive file is walked, so they were never marked as seen
- `serve --watch` no longer reports every source as failed when the MCP
  transport exits before the initial ingest finishes; the failures were
  cancelled filesystem walks during runtime shutdown, not real errors
- Shell scripts and other source files whose extension maps to a MIME
  type kreuzberg rejects (`.sh`, `.sql`, `.ts`, `.pl`, `.php`) now fall
  back to the UTF-8 text probe instead of failing with "Unsupported
  format"
- The featureless build (`--no-default-features`) compiles again:
  ingest-only CLI commands are now gated to match their argument
  definitions
- `lore --version` no longer prints a duplicate `v` prefix
- `make install` passes `--locked`, so installing no longer re-resolves
  the lockfile and reintroduces held-back dependency breakage

### Security

- Updated both workspace lockfiles, clearing RUSTSEC-2026-0258 (h2) at
  the root and seven advisories in the fuzz workspace, which had gone
  unaudited because it declares its own `[workspace]`

## [0.1.0] -- 2026-05-04

Initial release.

### Added

- **10 source types** -- local files, URLs, git repos, XML sitemaps,
  RSS/Atom feeds, Amazon S3 (`--features s3`), YouTube (via `yt-dlp`),
  Maildir email stores, shell commands (`exec`), and upstream MCP
  servers (`--features mcp`)
- **90+ document formats** -- PDF, DOCX, XLSX, HTML, email, EPUB,
  archives, Markdown, Org-mode, LaTeX, source code, and images via
  kreuzberg; OCR for scanned PDFs and images (`--features ocr`, on by
  default); Apple iWork documents (`--features iwork`); language-aware
  source code parsing (`--features tree-sitter`)
- **BM25 full-text search** -- embedded Tantivy index with multi-language
  tokenizer support (18 languages); no external services, no embeddings,
  no GPU
- **CLI** -- `init`, `ingest`, `search`, `topics`, `docs`,
  `read`, `info`, `serve`, `watch`, `preview`, `enrich`,
  `status`, `completions`, `maintain` (with `check`, `repair`, `compact`,
  `clean` subcommands); query and listing commands support `--json`;
  `read --full` for continuous text, `read --pager` for external pager
  support; `status --remote` for remote source change detection;
  global user config via `init --global`;
  shell completions for bash, zsh, fish, elvish, and PowerShell
- **Multi-KB federation** -- repeatable `--config` flag (or
  colon-separated `LORE_CONFIG`) federates queries and ingest across
  multiple knowledge bases
- **MCP server** -- six read-only tools; concrete resources for every
  topic and document with cursor-based pagination; resource templates
  for direct URI construction; stdio and streamable HTTP transports;
  automatic store reload on external ingest; `--watch` for embedded
  re-ingest on local file changes; federated multi-KB via repeated
  `--config`
- **Incremental ingest** -- three-tier change detection (mtime+size,
  ETag/Last-Modified, content hash); periodic commits for resumable
  long-running ingests; graceful shutdown on Ctrl+C / SIGTERM;
  structured failure reporting with JSONL logs
- **Processing profiles** -- global defaults, named presets, and
  per-source overrides for chunk size, extraction mode, section
  dropping, and transform pipelines (strip lines, regex replace,
  metadata extraction)
- **Metadata extraction** -- title, author, language, created date,
  topic, and tags from binary metadata, YAML/TOML frontmatter,
  Org-mode headers, and content heuristics; per-source `tags` config
  with merge semantics
- **LLM enrichment** (`--features llm`) -- topic detection, summaries,
  tags, and quality scoring via Ollama, Anthropic, OpenAI, or AWS
  Bedrock
- **Security** -- SSRF protection (DNS resolver blocks private IP
  ranges), archive bomb protection (file count and size limits), query
  sanitization
- **HTTP caching** -- ETag and Last-Modified round-tripping, per-host
  rate limiting, robots.txt enforcement
- **Install script** (`scripts/install.sh`), cargo-binstall support,
  and pre-built binaries via cargo-dist

[0.1.0]: https://github.com/timorunge/lore/releases/tag/v0.1.0
