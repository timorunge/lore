---
name: lore
description: Sets up and manages lore, a local knowledge base with BM25 full-text search across 90+ document formats. Ingests local files, URLs, git repos, sitemaps, RSS/Atom feeds, S3, YouTube, email (maildir), shell commands (exec), and upstream MCP servers into a searchable store. Serves results via CLI or MCP to LLM agents. Use when the user mentions lore, wants to build a searchable knowledge base, needs to configure lore.yaml, wire up an MCP server for document search, ingest documents, or troubleshoot lore commands.
license: MIT
compatibility: Requires lore binary on PATH. Optional cmake for OCR, yt-dlp for YouTube, Ollama or cloud API for LLM enrichment.
metadata:
  author: timorunge
  version: "0.1.0"
---

# lore

Single-binary local knowledge base. Ingest documents, search from the
terminal, or serve to LLM agents over MCP. No external services, no
embeddings.

## Decision tree

```
What does the user need?
+- Set up a new knowledge base
|  `- Follow "New knowledge base workflow" below
+- Write or modify lore.yaml config
|  `- Read docs/configuration.md
+- Wire up MCP for an agent
|  `- Read docs/mcp-integration.md
+- Search or query an existing store
|  `- See "CLI search" below
+- Troubleshoot
|  `- See "Troubleshooting" below
`- See example configs
   `- Read examples/README.md
```

## New knowledge base workflow

```
Task Progress:
- [ ] Step 1: Initialize config
- [ ] Step 2: Review and customize lore.yaml
- [ ] Step 3: Ingest documents
- [ ] Step 4: Verify the store
- [ ] Step 5: Serve to agents (if needed)
```

**Step 1: Initialize config**

```bash
cd my-project
lore init
```

Scans the directory, detects documentation folders and READMEs, generates
`.lore/lore.yaml` with pre-filled sources.

**Step 2: Review and customize lore.yaml**

Open `.lore/lore.yaml`. Verify source paths, add topics, adjust settings.
Minimal working config:

```yaml
name: my-knowledge-base
sources:
  - path: ./docs
    glob: "**/*.md"
    topic: Documentation
store:
  path: ./store
```

Ten source types are available:

| Key | What it ingests |
|-----|-----------------|
| `path` | Local files or directories |
| `url` | HTTP/HTTPS URLs |
| `git` | Git repositories |
| `sitemap` | XML sitemaps (discovers URLs) |
| `feed` | RSS/Atom feeds |
| `s3` | S3 buckets (requires `--features s3`) |
| `youtube` | YouTube videos/playlists/channels (requires yt-dlp) |
| `maildir` | Maildir email directories |
| `exec` | Shell commands (JSONL output) |
| `mcp` | Upstream MCP servers (requires `--features mcp`) |

All keys accept a single string or a list. Common options: `topic`,
`glob`, `include`, `headers`, `processing`.

For full config reference:
[docs/configuration.md](../../docs/configuration.md)

**Step 3: Ingest documents**

```bash
lore ingest
```

Preview chunking first without writing to the store:

```bash
lore preview ./docs --limit 5
```

**Step 4: Verify the store**

```bash
lore info              # document and chunk counts
lore topics            # list topics
lore search "test"     # try a search
lore docs              # list indexed documents
```

**Step 5: Serve to agents**

Stdio (for MCP clients that launch subprocesses):

```json
{
  "mcpServers": {
    "my-docs": {
      "command": "lore",
      "args": ["serve"],
      "env": { "LORE_CONFIG": "/absolute/path/to/lore.yaml" }
    }
  }
}
```

For HTTP transport, multi-KB setups, tool reference, and resource URIs:
[docs/mcp-integration.md](../../docs/mcp-integration.md)

## CLI commands

<!-- BEGIN GENERATED: cli-commands -->
| Command | Description |
|---------|-------------|
| `lore init` | Scaffold a lore.yaml config by scanning the current directory |
| `lore ingest` | Build or incrementally update the knowledge base |
| `lore search <query>` | Full-text search across indexed documents |
| `lore topics` | List topics with document and chunk counts |
| `lore docs` | List indexed documents with optional filters |
| `lore read <source>` | Read a specific document by source path |
| `lore info` | Show store statistics and metadata |
| `lore serve` | Serve the knowledge base over MCP (stdio or HTTP) |
| `lore watch` | Watch local sources and re-ingest on file changes |
| `lore preview [paths]` | Preview document processing without writing to the store |
| `lore enrich` | Re-run LLM enrichment on stored documents without re-fetching (requires `--features llm`) |
| `lore status` | Show what changed since last ingest (stat-only, no content reading) |
| `lore completions <shell>` | Generate shell completion scripts |
| `lore maintain` | Store maintenance: consistency checks, optimization, cache |
| `lore maintain check` | Check store consistency (read-only) |
| `lore maintain repair` | Fix detected consistency issues |
| `lore maintain compact` | Merge index segments for lower memory and faster reads |
| `lore maintain clean` | Clear cached downloads, git repos, or temporary files |
| `lore maintain health` | Print a single-screen health report (config, features, LLM, store) |
<!-- END GENERATED: cli-commands -->

### Key flags

- `--config <path>` or `LORE_CONFIG` -- config file location (repeatable for multi-KB federation)
- `--json` -- JSON output on all query commands
- `--full` -- show a document as continuous text without chunk separators (`lore read`)
- `--pager <CMD>` / `--no-pager` -- pipe output through an external pager (`lore read`)
- `--limit N`, `--offset N` -- pagination
- `--topic`, `--author`, `--source`, `--lang`, `--origin`, `--kind`,
  `--format` -- search and list filters

### Ingest modes

```bash
lore ingest              # incremental (default, skips unchanged docs)
lore ingest --recreate   # destroy and rebuild from scratch
lore ingest --force      # re-process all documents, keep store
lore ingest --dry-run    # show what would change
lore ingest --source docs  # only process sources matching "docs"
```

## CLI search

```bash
lore search "authentication"
lore search "auth" --topic Security --json
lore search "error" --origin git --format rs --limit 20
lore topics --sort chunks --reverse
lore docs --topic "API Reference"
lore read docs/auth.md
```

Topic and author filters use fuzzy matching: exact, then
case-insensitive, then substring.

## Gotchas

- **`base_dir` resolves from the config file's directory**, not the
  working directory. `lore init` sets `base_dir: ..` because the config
  lives inside `.lore/`. If you move the config, update `base_dir`.
- **`LORE_CONFIG` must be an absolute path** in MCP client configs.
  Relative paths break because the MCP client's working directory is
  unpredictable.
- **Changing `max_chunk_chars`, `language`, or `phrase_search` requires
  `--recreate`**. Incremental ingest skips unchanged documents, so
  existing chunks keep their old sizes. `--force` re-processes content
  but does not change store-level settings like language or phrase search.
- **`deny_unknown_fields` is on by default.** Typos in config keys are
  hard errors, not silent ignores. lore suggests the closest valid key.
- **`store.path` is relative to the config file**, not `base_dir`. A
  config at `.lore/lore.yaml` with `store.path: ./store` writes to
  `.lore/store/`, regardless of `base_dir`.
- **YouTube requires `yt-dlp` on PATH.** lore calls it as a subprocess;
  it is not bundled.

## Troubleshooting

**Config parse errors** -- lore validates strictly
(`deny_unknown_fields`). Typos produce suggestions:

```
unknown source type; did you mean 'sitemap'?
Expected one of: path, url, git, sitemap, feed, s3, youtube, maildir, exec, mcp
```

**Empty search results:**
1. `lore info` -- verify documents are indexed
2. `lore topics` -- check topic names match your filter
3. Try without filters first
4. `lore docs` -- verify expected documents were ingested

**Ingest skipping files** -- incremental ingest skips unchanged
documents. Run `lore maintain check` to verify store consistency.
Use `lore ingest --force` to re-process all, or
`lore ingest --recreate` to rebuild from scratch.

**MCP server not responding:**
- Test `lore serve` standalone first
- Verify `LORE_CONFIG` points to a valid config with an existing store
- For HTTP transport, check the port is not in use

## Reference files

- [docs/configuration.md](../../docs/configuration.md) -- full
  lore.yaml reference: all source types, processing, store, fetch, LLM,
  env vars
- [docs/mcp-integration.md](../../docs/mcp-integration.md) -- MCP
  transports, client configs, all six tools with parameters, resources,
  agent workflow
- [examples/README.md](../../examples/README.md) -- runnable examples
  and config templates: project docs, Gutenberg, LLM, YouTube, sitemap,
  multi-format, authenticated endpoints, S3
