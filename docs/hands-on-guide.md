# Hands-On Guide

A practical walkthrough for building a knowledge base with lore, searching
it from the terminal, and serving it to agents over MCP. lore handles
extraction and indexing across document formats so you can search your
knowledge without worrying about which tool opens which file. Every command
in this guide can be run in a real terminal against a real project directory.

## Prerequisites

- lore installed (see [Install](../README.md#install) for all options)
- A project directory with some documentation (Markdown files work well for a
  first pass)

To build with LLM enrichment (topic detection, summaries, quality scoring):

```bash
cargo install --path . --features llm
```

See the [README](../README.md#install) for the full list of feature flags.

For tab completion in your shell:

```bash
lore completions zsh > ~/.zfunc/_lore                                    # zsh
lore completions bash > ~/.local/share/bash-completion/completions/lore  # bash
lore completions fish > ~/.config/fish/completions/lore.fish             # fish
```

## Initialize a knowledge base

Navigate to the project you want to index:

```bash
cd ~/work/my-project
lore init
```

lore scans your directory, detects documentation folders and README files, and
writes a starter config to `.lore/lore.yaml`:

```
[+ ] created .lore/lore.yaml
[+ ] detected 2 sources
[i ] review .lore/lore.yaml, then run `lore ingest`
[i ] add .lore/store to your .gitignore
```

The `.lore/store/` directory does not exist yet -- it is created when you run
`lore ingest`.

If a `.lore/lore.yaml` already exists, `lore init` exits with an error rather
than overwriting it.

## Review the generated config

Open `.lore/lore.yaml`:

```yaml
name: my-project

base_dir: ..

sources:
  - path: ./docs
    glob: "**/*.md"

  - path: ./README.md

  # Git remote detected:
  # - git: https://github.com/user/my-project.git
  #   glob: "**/*.md"
```

Key fields:

- `name` -- the label for this knowledge base, shown by `lore info` and in
  MCP server instructions. Change it to something descriptive.

- `base_dir` -- the base directory for resolving local source paths. Since
  the config lives in `.lore/`, `base_dir: ..` makes paths like `./docs`
  resolve from the project root instead of from `.lore/`.

- `sources` -- a list of document sources. `lore init` only generates `path`
  sources (local files and directories), but you can manually add other
  source types too: `url`, `git`, `sitemap`, `feed`, `s3`, `youtube`,
  `maildir`, `exec`, or `mcp`. The type is determined by which key is
  present. Each source can also carry optional `topic` and `tags` fields.

- `path` -- a local file or directory. When pointing at a directory, `glob`
  controls which files are matched. The default glob is `**/*` (all files).

- The commented-out `git` source is a suggestion based on the detected remote
  URL. Uncomment and adjust it if you want to index the repository itself.

If you want to drop sections like "See Also" or "References" from the store,
add a `processing` block:

```yaml
processing:
  drop_sections:
    - See Also
    - References
    - Changelog
```

## Run your first ingest

```bash
lore ingest
```

lore discovers and processes each source, then writes the store to
`.lore/store/`:

```
[. ] ingesting "my-project" (2 sources, mode: recreate)
[+ ] [1/2] ./docs (path) -- 23 docs, 187 chunks
[+ ] [2/2] ./README.md (path) -- 1 doc, 4 chunks
[i ] 24 documents, 191 chunks in 1.0s -> .lore/store (1.4 MB)
[. ] 24 docs/s, 191 chunks/s, peak mem 53.4 MB, cpu 1.8s
```

lore extracts text from each document, splits it into chunks that respect
heading boundaries, extracts metadata (title, author, language, created 
date, topic, tags), and commits the store to disk.

If some documents fail to process, the source line shows `[! ]` instead of
`[+ ]`. Successfully processed documents are still indexed. Failures are
printed inline and written to a JSONL log in the cache directory:

```
[! ] [1/2] ./docs (path) -- 42 docs, 310 chunks (3 failed)
[! ] 3 document(s) failed:
     docs/broken.pdf    unsupported encryption
     docs/empty.md      no extractable content
     docs/huge.docx     file exceeds size limit
     full log: <cache>/logs/failures-2026-05-04T04-04-00.jsonl
```

Clear old failure logs with `lore maintain clean logs`.

To rebuild the store from scratch (discarding all existing data):

```bash
lore ingest --recreate
```

`--recreate` drops all existing store data before ingesting. Use it when you
have changed store-level settings (`language`, `phrase_search`) or want a
clean slate. For less destructive alternatives, see `lore maintain check`
(diagnose issues) and `lore ingest --force` (re-process without wiping).

To see what would change without writing anything:

```bash
lore ingest --dry-run
```

## Enrich with LLM

If you built lore with `--features llm` and have an `llm` block in your
config, you can add or refresh LLM enrichment (topic labels, per-chunk
summaries, keyword tags, quality scores) on already-indexed documents
without re-fetching content from the source:

```bash
lore enrich
```

By default, documents that already have enrichment data are skipped. To
re-run enrichment on everything:

```bash
lore enrich --force
```

To limit enrichment to a specific source path or topic:

```bash
lore enrich --source docs/api
lore enrich --topic "Authentication"
```

The `--source` filter is also available on `ingest` and `watch` to narrow
which configured sources are processed:

```bash
lore ingest --source docs/api
lore watch --source docs
```

The filter matches by substring against each source's path or URL.

Enrichment reads the existing chunks from the store, calls the configured
LLM, and writes the updated data back. It does not re-fetch or re-chunk
the original documents.

## Preview documents before ingesting

`lore preview` shows how documents will be extracted and chunked without
writing to the store. Use it to tune `max_chunk_chars`, `drop_sections`, or
any other processing setting before committing to a full ingest.

```bash
lore preview ./docs
```

Each document shows its title, source path, metadata fields, and chunk
content:

```
Architecture Overview
  docs/architecture.md
  Title:   Architecture Overview
  Kind:    document
  Format:  md
  Origin:  local

[1] Architecture
    This document describes the high-level architecture of the system.
    The main components are the API gateway, the processing pipeline,
    and the storage layer...

API Reference
  docs/api-reference.md
  Title:   API Reference
  Kind:    document
  Format:  md
  Origin:  local

[1] Authentication
    The API uses Bearer token authentication. Every request must
    include an Authorization header...
```

By default, preview shows up to 3 chunks per document. Use `--chunks` to
adjust:

```bash
lore preview ./docs --chunks 1    # header + first chunk only
lore preview ./docs --chunks 999  # show all chunks
```

Preview does not write to the store. Run it as many times as you need.
Output can be piped through a pager with `--pager "less -R"` or by setting
the `LORE_PAGER` environment variable.

For flag details (`--limit`, `--offset`, `--chunks`, `--json`, `--pager`,
`--config`), see [CLI Reference](./cli.md).

## Inspect the knowledge base

After ingesting, check what is in the store:

```bash
lore info
```

```
my-project

Store:          .lore/store
Size:           1.4 MB
Kinds:          24 documents
Formats:        md (24)
Sources:        24 local
Languages:      en (24)
Documents:      24
Chunks:         191
Words:          43.2K
Avg/doc:        8 chunks, 1800 words (225 words/chunk)
Created:        2026-05-04T03:21:09Z
Updated:        2026-05-04T03:21:09Z
Last mode:      recreate
Phrase search:  disabled
Version:        lore v0.1.0
```

When topics are configured, a topics block appears below with chunk
counts per topic.

## Search from the terminal

lore includes terminal-native search commands that work directly
against the store:

```bash
lore search "architecture"
```

```
2 results for "architecture", 1-2/2.

[0] Architecture Overview  (4.32, #1/4)
    docs/architecture.md
    Architecture
    The main components are the API gateway, the processing
    pipeline, and the storage layer...

[1] Design Decisions  (3.87, #2/6)
    docs/design-decisions.md
    Overview
    Key architectural decisions and their rationale...
```

Browse topics and documents:

```bash
lore topics
lore docs --topic "Project Docs"
lore read docs/architecture.md
lore read docs/architecture.md --full  # continuous text, no chunk separators
```

Use `--pager` to pipe long output through an external pager like `less -R`
or `bat`. Use `--no-pager` to disable it when `LORE_PAGER` or `PAGER` is set.

Query and listing commands support `--json` for scripting:

```bash
lore search "auth" --json | jq '.items[].source'
lore docs --json | jq '.items[] | select(.chunk_count > 10)'
```

`lore serve` exposes the knowledge base to MCP clients and agents.
The server automatically picks up changes when you run `lore ingest`
separately -- no restart or flags needed. Use `--watch` only if you
want the server to monitor source files and re-ingest on its own.
For the full flag reference, see [CLI Reference](./cli.md).

## Serve over MCP

Start the MCP server:

```bash
lore serve                               # stdio (default, for MCP clients that launch subprocesses)
lore serve --transport http --port 8080  # streamable HTTP on localhost:8080/mcp
```

### Stdio (subprocess)

The default stdio transport listens on stdin and writes to stdout. Configure
your MCP client to launch it as a subprocess:

```json
{
  "mcpServers": {
    "my-project": {
      "command": "lore",
      "args": ["serve"],
      "env": {
        "LORE_CONFIG": "/absolute/path/to/my-project/.lore/lore.yaml"
      }
    }
  }
}
```

Use an absolute path for `LORE_CONFIG` -- MCP clients launch the server as a
subprocess, so relative paths resolve from the client's working directory, not
yours. `~` expansion works in both `LORE_CONFIG` and the `--config` flag.

### Streamable HTTP

For clients that connect over the network (web apps, remote agents, or
multiple concurrent clients):

```bash
lore serve --transport http
```

Point your MCP client at `http://localhost:8080/mcp` (or configure
`--host` / `--port` as needed).

See [MCP Integration](./mcp-integration.md) for the `--config` flag
alternative, multi-instance setups, and the full tool reference.

## Add a second source

Suppose you want to add the CNCF Cloud Native Glossary from its public git
repository. Open `.lore/lore.yaml` and add a new source:

```yaml
name: my-project
description: Internal project docs plus CNCF cloud-native terminology.

base_dir: ..

sources:
  - path: ./docs
    glob: "**/*.md"
    topic: Project Docs

  - path: ./README.md
    topic: Project Docs

  - git: https://github.com/cncf/glossary.git
    glob: "content/en/[!_]*.md"
    topic: CNCF Glossary
```

Two changes from the original config:

- Each source now has a `topic` field. Topics let agents filter results with
  `lore_search`'s `topic` parameter and browse by topic with `lore_read_topic`.
- A `description` field was added. lore surfaces this in `lore info` and
  in MCP server instructions so both humans and agents know what the
  knowledge base contains.

Now run ingest again:

```bash
lore ingest
```

```
[. ] ingesting "my-project" (3 sources, mode: update)
[+ ] [1/3] ./docs (path)
[+ ] [2/3] ./README.md (path)
[+ ] [3/3] https://github.com/cncf/glossary.git (git) -- 214 docs, 891 chunks
[i ] 214 documents, 891 chunks in 9.1s -> .lore/store (8.7 MB)
```

lore clones the glossary repository into the global cache
(`~/Library/Caches/lore/repos/` on macOS, `~/.cache/lore/repos/` on Linux),
then processes only the matching Markdown files. The local `docs/` and
`README.md` sources are unchanged and report zero new documents.

To add an RSS feed instead (for example, the AWS Architecture Blog):

```yaml
  - feed: https://aws.amazon.com/blogs/architecture/feed/
    include: "/blogs/architecture/"
    topic: AWS Architecture Blog
```

The `include` field is a regex that filters which URLs from the feed are
fetched. Without it, lore fetches every link in the feed.

To add a YouTube talk or playlist:

```yaml
  - youtube: https://www.youtube.com/watch?v=wgdBVIX9ifA
    lang: en
    topic: Architecture Talks
```

lore spawns [yt-dlp](https://github.com/yt-dlp/yt-dlp) as a background
process to fetch video metadata and subtitles -- `yt-dlp` must be
installed and on your `PATH`. For playlists and channels, use
`max_videos` to cap the number of videos processed:

```yaml
  - youtube: https://www.youtube.com/@fireship
    lang: en
    max_videos: 10
    topic: Fireship
```

### Indexing email from Maildir

If you store email locally in Maildir format, add a maildir source:

```yaml
sources:
  - maildir: ~/Mail/INBOX
    topic: Inbox

  # or multiple directories
  - maildir:
      - ~/Mail/INBOX
      - ~/Mail/Archive
    skip_trashed: true
    topic: Email
```

Each path must be a Maildir root with `cur/` and/or `new/` subdirectories.
Messages are parsed directly -- no external tools required. Subject becomes
the document title, From becomes the author, and Maildir flags (seen,
replied, flagged, etc.) are stored as tags.

> **Tip:** `.eml`, `.mbox`, and `.pst` files work out of the box with a
> regular `path:` source -- the maildir source is only needed for the
> Maildir directory layout where files have no extension.

### Index data from a custom script

The `exec` source runs a shell command and reads JSONL from stdout. This
is useful when your data lives in a database, API, or format that lore
doesn't support natively.

Create a script that outputs one JSON object per line:

```python
#!/usr/bin/env python3
import json

docs = [
    {"source": "kb/getting-started", "content": "To get started...", "title": "Getting Started"},
    {"source": "kb/faq",             "content": "Frequently asked...", "title": "FAQ"},
]
for doc in docs:
    print(json.dumps(doc))
```

Then reference it in your config:

```yaml
sources:
  - exec: python3 scripts/export-kb.py
    topic: Knowledge Base
    timeout_secs: 30
```

Each line must have at least `source` (a stable identity key) and
`content` (text to index). Optional fields: `title`, `author`, `lang`,
`tags`, `format`, `kind`. lore always re-runs the command but skips
re-indexing documents whose content hash hasn't changed.

## Customizing Processing

lore's processing pipeline lets you control how documents are extracted,
chunked, and transformed -- either globally, through named presets, or on a
per-source basis.

### Presets

Define a named processing profile once and reference it from any number of
sources. This avoids repeating the same settings for every source that needs
the same treatment:

```yaml
sources:
  - path: src/
    processing: code

processing:
  presets:
    code:
      extract: none
      max_chunk_chars: 800
```

The `extract: none` value tells lore to skip kreuzberg extraction and read the
file as plain text. `max_chunk_chars: 800` keeps code chunks tight.

### Inline overrides

For a one-off tweak on a single source, set `processing` directly on that
source instead of defining a preset:

```yaml
sources:
  - path: vendor/rfcs/
    processing:
      max_chunk_chars: 2000
      pipeline:
        - type: strip_lines
          first: 10
```

Inline settings take precedence over any global `processing` block and do not
affect other sources.

### Pipeline transforms

The `pipeline` key is an ordered list of transform steps applied to each
document's text before chunking. Transforms run top to bottom:

```yaml
processing:
  extract: none
  pipeline:
    - type: strip_lines
      first: 5
    - type: replace_text
      pattern: '(?m)^CONFIDENTIAL.*$'
    - type: extract_builtin
    - type: extract_metadata
      pattern: 'Author:\s+(.+)'
      field: author
```

- `strip_lines` removes the first `first` lines and/or the last `last` lines
  -- useful for boilerplate headers or footers in vendor documents.
- `replace_text` removes or substitutes text matching a regex pattern.
- `extract_builtin` runs lore's built-in metadata heuristics after any prior
  transforms have cleaned up the text.
- `extract_metadata` captures the first capture group and stores it as a
  metadata field (here, `author`).

Presets and inline overrides can both include a `pipeline` key. The pipeline
in an inline override replaces the preset's pipeline entirely -- there is no
merging of pipeline steps.

### Preview

Use `lore preview` to verify that your chunking and pipeline settings produce
the output you expect before committing a full ingest:

```bash
lore preview ./vendor/rfcs/
```

You can test different presets by temporarily changing the `processing` field
on a source and running `lore preview` again. It does not write to the store,
so there is no cost to iterating. For flag details, see
[CLI Reference](./cli.md).

## Run an incremental update

lore tracks a content hash for every document. On subsequent ingests, only
documents that have changed are re-processed.

Run ingest after a day or two:

```bash
lore ingest
```

```
[. ] ingesting "my-project" (3 sources, mode: update)
[+ ] [1/3] ./docs (path)
[+ ] [2/3] ./README.md (path)
[+ ] [3/3] https://github.com/cncf/glossary.git (git)
[i ] 0 documents, 0 chunks in 0.3s -> .lore/store (8.7 MB)
```

Nothing changed, so nothing was re-indexed. If a few docs changed:

```
[. ] ingesting "my-project" (3 sources, mode: update)
[+ ] [1/3] ./docs (path) -- 2 docs, 17 chunks
[+ ] [2/3] ./README.md (path)
[+ ] [3/3] https://github.com/cncf/glossary.git (git) -- 3 docs, 14 chunks
[i ] 5 documents, 31 chunks in 1.1s -> .lore/store (8.9 MB)
```

lore commits progress to disk every 30 seconds during ingest (configurable
with `processing.commit_interval`). If a long ingest is cancelled mid-run,
the next `lore ingest` resumes from the last commit rather than starting over.

To re-process all documents without wiping the store:

```bash
lore ingest --force
```

To check and repair store consistency:

```bash
lore maintain check    # diagnose issues (read-only)
lore maintain repair   # fix detected issues
lore maintain compact  # merge segments for faster reads
```

If store-level settings changed (`language`, `phrase_search`) or you want a
clean slate, rebuild from scratch:

```bash
lore ingest --recreate
```

To check whether remote sources have new content without re-ingesting:

```bash
lore status --remote
```

This makes lightweight network requests (HEAD, ls-remote, feed/sitemap
XML) and reports added, changed, or deleted items per source.

To keep remote sources (URLs, feeds, git repos) fresh automatically:

```bash
lore watch --interval 3600
```

This re-ingests all sources every hour while also watching local files for
immediate changes.

## Set up multiple knowledge bases

lore supports one knowledge base per config file. For separate domains --
say, internal project docs and a vendor documentation archive -- keep them
in separate directories, each with their own `.lore/` directory.

Run `lore ingest` explicitly -- manually, via cron, or in CI -- to update
each knowledge base. To re-ingest automatically whenever local files change,
use `lore watch` instead (see [CLI Reference](./cli.md#watch)).

### Directory layout

```
~/work/
  my-project/
    .lore/
      lore.yaml
      store/
    docs/
    src/

  vendor-kb/
    .lore/
      lore.yaml
      store/
```

### When to split vs. combine

Put sources in the same config when they are part of the same conceptual
domain and you want a single search to span them. Split into separate configs
when the domains are distinct -- this keeps each index focused (better
relevance) and avoids pulling irrelevant content into the agent's context
window. Separate configs also let different teams update their knowledge
bases on independent schedules.

When you need to search or serve multiple knowledge bases together, pass
multiple `--config` flags:

```bash
lore search "auth" -c ~/work/my-project/.lore/lore.yaml -c ~/work/vendor-kb/.lore/lore.yaml
lore serve -c ~/work/my-project/.lore/lore.yaml -c ~/work/vendor-kb/.lore/lore.yaml
```

Or set `LORE_CONFIG` with colon-separated paths:

```bash
LORE_CONFIG=~/work/my-project/.lore/lore.yaml:~/work/vendor-kb/.lore/lore.yaml lore serve
```

All read commands (search, topics, docs, read, info) merge results across
all knowledge bases. Write commands (ingest, watch, enrich, maintain)
run per-KB with labeled output.

For MCP setup with multiple knowledge bases, see
[MCP Integration](./mcp-integration.md#multiple-knowledge-bases).

### Updating each knowledge base independently

Run `lore ingest` for each config using `LORE_CONFIG` or the `--config` flag.
`~` expansion works in both:

```bash
lore -c ~/work/my-project/.lore/lore.yaml ingest
lore -c ~/work/vendor-kb/.lore/lore.yaml ingest
```

Or ingest all at once:

```bash
lore ingest -c ~/work/my-project/.lore/lore.yaml -c ~/work/vendor-kb/.lore/lore.yaml
```

## Working with large datasets

For most knowledge bases -- a few hundred to a few thousand documents -- the
defaults work well and ingest completes in seconds. Once the initial ingest is
done, search returns results in milliseconds and incremental updates only
re-process changed documents.

If you are indexing tens of thousands of documents (large monorepos, archival
collections, crawled sites), the initial ingest becomes the bottleneck. Three
settings make the biggest difference:

```yaml
store:
  writer_heap_mb: 2048   # reduce segment flushes (default: 256)

processing:
  max_chunk_chars: 5000  # fewer, larger chunks (default: 1600)
  concurrency: 24        # saturate available cores (default: half)
```

These reduce wall time by 70%+ on a 76K-document corpus. See
[Performance Tuning](./performance.md) for the full benchmark, internal
pipeline details, and tuning rationale.

After initial ingest, adding or updating documents is fast regardless of index
size -- lore skips unchanged documents via content hashing, and the optimized
single-segment index keeps search latency in the low milliseconds. The heavy
work only happens once.

See also: [CLI Reference](./cli.md), [Configuration](./configuration.md),
[MCP Integration](./mcp-integration.md), [Examples](../examples/).
