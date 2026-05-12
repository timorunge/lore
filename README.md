# lore

A local knowledge base you can search from the terminal or serve to
agents over MCP. lore ingests your documents -- local files, websites,
git repos, feeds, S3, YouTube, email, shell commands, upstream MCP
servers, 90+ formats -- and indexes everything with full-text search.
No external services, no infrastructure to manage. One binary, one
config, one store -- or as many as you need.

Think `man` pages for your projects -- for humans and agents alike.

## Install

Install script (Linux and macOS):

```bash
curl -fsSL https://github.com/timorunge/lore/releases/latest/download/lore-cli-installer.sh | bash
```

Homebrew (macOS and Linux):

```bash
brew install timorunge/tap/lore-cli
```

Or download a pre-built binary directly from the
[releases page](https://github.com/timorunge/lore/releases).

> **Windows:** Pre-built binaries are available for Linux and macOS.
> On Windows, use [WSL](https://learn.microsoft.com/en-us/windows/wsl/)
> (recommended) or build from source with `cargo install` (OCR requires
> cmake and is not supported on MSVC -- omit with `--no-default-features
> --features ingest,mcp`).

Build from source:

```bash
git clone https://github.com/timorunge/lore.git
cd lore
cargo install --path .
```

Optional feature flags:

<!-- BEGIN GENERATED: compile-features -->
| Flag | Default | Description |
|------|---------|-------------|
| `ocr` | on | OCR for scanned PDFs and images (requires cmake) |
| `llm` | off | LLM enrichment (`lore ingest`, `lore enrich`): Ollama, Anthropic, OpenAI, and Bedrock |
| `s3` | off | Amazon S3 source support |
| `mcp` | on | Upstream MCP server ingestion (resources and tool calls) |
| `iwork` | off | Apple iWork documents (Keynote, Pages, Numbers) |
| `tree-sitter` | off | Source code parsing via tree-sitter |
<!-- END GENERATED: compile-features -->

```bash
cargo install --path . --features llm         # with LLM enrichment
cargo install --path . --features llm,s3      # with LLM + S3 support
cargo install --path . --all-features         # all optional features
cargo install --path . --no-default-features  # minimal build (no OCR, no MCP)

# Windows (native, no OCR):
cargo install --path . --no-default-features --features ingest,mcp
```

## Quick start

```bash
cd my-project
lore init                     # generates .lore/lore.yaml from your project
lore ingest                   # build the store
lore search "authentication"  # search from the terminal
lore serve                    # or serve to agents over MCP
```

`lore init` scans your directory, detects documentation folders and
README files, and generates a config with pre-filled sources. Review it,
ingest, and search -- from your terminal or through any MCP client.

### Try it without your own docs

If you cloned the repo, the gutenberg example works out of the box:

```bash
cd examples/gutenberg
lore ingest
lore search "love and death"
```

Without a checkout, fetch the config directly:

```bash
mkdir tryout && cd tryout && mkdir -p .lore
curl -fsSL https://raw.githubusercontent.com/timorunge/lore/main/examples/gutenberg/lore.yaml \
  -o .lore/lore.yaml
lore ingest
lore search "love and death"
```

This fetches classic novels from Project Gutenberg and indexes them --
no local documents required. See [examples/](examples/) for more
ready-to-run configs.

## Use cases

**Give your AI assistant project context** -- index your READMEs, design
docs, ADRs, and runbooks. Connect via MCP. Your AI now knows your
project without stuffing everything into the prompt.

```yaml
sources:
  - path: docs/
    topic: Internal
```

**Offline documentation search** -- index vendor docs, API references,
or internal wikis. Search from the terminal with no browser and no
internet.

```yaml
sources:
  - sitemap: https://docs.example.com/sitemap.xml
    topic: API Reference
```

**Team knowledge base** -- point lore at a shared git repo of documents.
Everyone runs `lore ingest` locally. No server, no SaaS, no vendor
lock-in.

```yaml
sources:
  - git: https://github.com/org/team-docs
    glob: "**/*.md"
    topic: Team
```

**Federated search** -- keep knowledge bases separate but query them
together. Each config has its own store and update schedule; lore merges
results at query time.

```bash
lore search "auth" -c project.yaml -c vendor.yaml
```

## MCP integration

Serve your knowledge base to AI assistants over the
[Model Context Protocol](https://modelcontextprotocol.io/).

Stdio (Claude Code, Kiro, Cursor -- any MCP client that launches
subprocesses):

```json
{
  "mcpServers": {
    "my-docs": {
      "command": "lore",
      "args": ["serve"],
      "env": {
        "LORE_CONFIG": "/path/to/.lore/lore.yaml"
      }
    }
  }
}
```

Streamable HTTP (web apps, remote agents, multiple clients):

```bash
lore serve --transport http --port 8080
```

```json
{
  "mcpServers": {
    "my-docs": {
      "url": "http://localhost:8080/mcp"
    }
  }
}
```

Six read-only tools: `lore_info`, `lore_list_topics`, `lore_search`,
`lore_read_topic`, `lore_list_docs`, and `lore_read_doc`.

MCP resources for clients that support browsing: `lore://info`,
plus one `lore://topics/{name}` and one `lore://docs/{source}` resource
per topic and document. All discoverable via `list_resources`.

Serve multiple knowledge bases through a single MCP server -- pass
multiple `-c` flags and lore federates them at query time:

```bash
lore serve -c project.yaml -c vendor.yaml
```

**`/lore` skill** -- lore ships a `/lore` skill in
[skills/lore/](skills/lore/) for AI coding assistants that support
skills. Type `/lore` for interactive help with setup, config authoring,
and troubleshooting.

See [MCP Integration](docs/mcp-integration.md) for transport options,
multi-KB setups, and the full tool reference.

## Configuration

Each knowledge base is defined by a YAML config file. Mix source types
and assign topics for independent search:

```yaml
name: my-knowledge-base

base_dir: ..

sources:
  # All source keys accept a single string or a list of strings.
  - path: ./docs
    glob: "**/*.md"
    topic: Internal

  - git: https://github.com/org/public-docs
    glob: "**/*.md"
    topic: Public Docs

  - sitemap: https://docs.example.com/sitemap.xml
    include: "/reference/"
    topic: API Reference

  - youtube: https://www.youtube.com/watch?v=JZfJTSlhOXM
    lang: en
    topic: Talks
```

The YouTube source spawns [yt-dlp](https://github.com/yt-dlp/yt-dlp)
as a subprocess to fetch transcripts -- `yt-dlp` must be on `PATH`.
Playlists and channels are also supported.

URL-based sources support custom HTTP headers for authenticated
endpoints. Header values support `${LORE_*}` environment variable
expansion (only variables with the `LORE_` prefix are expanded):

```yaml
  - sitemap: https://internal.corp/sitemap.xml
    headers:
      Authorization: "Bearer ${LORE_DOCS_TOKEN}"
```

Processing profiles let you tune chunking and metadata extraction
per source. Define named presets and reference them inline, or override
inline directly:

```yaml
base_dir: ..

sources:
  - path: docs/                    # uses global defaults
  - path: src/
    processing: code               # uses code preset
  - path: special/
    processing:                    # inline override
      max_chunk_chars: 3000

processing:
  max_chunk_chars: 1600
  presets:
    code:
      extract: none
      max_chunk_chars: 800
```

See the [examples/](examples/) directory for complete, runnable configs
and the [Configuration Reference](docs/configuration.md) for all
options.

## How it works

```
sources                    lore                         consumers
  |                        |                              |
  |   local, URL, git,     |                              |
  |   sitemap, feed, S3,   |                              |
  |   YouTube, maildir,    |                              |
  |   exec, MCP            |                              |
  |----------------------->|                              |
  |                        |  extract text (kreuzberg)    |
  |                        |  extract metadata            |
  |                        |  apply pipeline transforms   |
  |                        |  chunk by structure          |
  |                        |  index (tantivy)             |
  |                        |                              |
  |                        |  lore search (terminal)      |
  |                        |<----- you ----- or --------->|
  |                        |  lore serve (stdio / http)   |
  |                        |<-----------------------------|
  |                        |  search / browse / retrieve  |
  |                        |----------------------------->|
```

Documents flow left to right during `lore ingest`. At query time, you
search from the terminal with `lore search`, or agents query through
six read-only MCP tools via `lore serve`. The store lives in
`.lore/store` by default (configurable via `store.path` or
`LORE_STORE_PATH`) -- no external services.

### What lore does

- **10 source types** -- local files, URLs, git repos, sitemaps, RSS/Atom
  feeds, S3, YouTube (via [yt-dlp](https://github.com/yt-dlp/yt-dlp)),
  maildir, shell commands, and upstream MCP servers
- **90+ document formats** -- PDF, DOCX, XLSX, HTML, email, EPUB,
  archives, Markdown, Org-mode, LaTeX, source code, and
  [more](docs/formats.md) via
  [kreuzberg](https://github.com/kreuzberg-dev/kreuzberg)
- **Metadata extraction** -- title, author, language, created date,
  topic, and tags from binary metadata, frontmatter, Org-mode headers,
  and content heuristics; choose
  `auto`, `builtin`, `kreuzberg`, or `none` per source
- **Smart chunking** -- respects document structure (headings, sections);
  configurable size limits
- **Processing profiles** -- per-source presets and inline overrides for
  chunk size, metadata extraction, and custom transform pipelines
- **Incremental updates** -- only new or modified documents are
  re-processed; periodic commits let you resume cancelled ingests
- **Embedded index** -- full-text search via
  [Tantivy](https://github.com/quickwit-oss/tantivy), stored locally,
  no external database

## Search from the terminal

Query your knowledge base without leaving the terminal:

```bash
lore search "authentication"         # free-text search
lore search "auth" --topic Security  # filter by topic
lore topics                          # list all topics
lore docs                            # list all documents
lore read docs/auth.md               # read a document
lore read docs/auth.md --full        # read as continuous text
lore status                          # show what changed since last ingest
lore completions zsh                 # generate shell completions
```

Query and listing commands support `--json` for piping to `jq` and other tools.
Pass multiple `-c` flags for federated search across knowledge bases. See
[CLI Reference](docs/cli.md) for the full flag reference.

## Documentation

| | |
|---|---|
| [Hands-on Guide](docs/hands-on-guide.md) | Step-by-step from install to serving |
| [CLI Reference](docs/cli.md) | All commands and flags, including search, topics, docs, read |
| [Configuration](docs/configuration.md) | Config file format, source types, env vars |
| [Supported Formats](docs/formats.md) | Document formats and metadata extraction |
| [MCP Integration](docs/mcp-integration.md) | Server setup, tool reference, multi-KB configs |
| [Architecture](docs/architecture.md) | Ingest pipeline, store design, key decisions |
| [Design Philosophy](docs/design-philosophy.md) | Why BM25, why single binary, two interfaces (CLI and MCP) |
| [Performance](docs/performance.md) | Benchmarks, tuning tips, index size guidance |
| [Security](docs/security.md) | Threat model, SSRF protection, input limits |
| [Contributing](docs/contributing.md) | Dev setup, quality gates, extending lore |
| [Examples](examples/) | Runnable configs for common use cases |

## Data responsibility

You are responsible for ensuring you have the right to ingest, index,
and serve the content you configure. When LLM enrichment is enabled,
document content is sent to the configured provider -- for sensitive
data, consider a local provider like Ollama. lore does not phone home
or collect telemetry. See [Security](docs/security.md#data-responsibility)
for details.

## License

MIT -- see [LICENSE](LICENSE).

<details>
<summary>The Lore of Lore</summary>

Every tool needs a mass-appealing origin story these days, so here is ours.

lore is an old lady with tentacles. She hoards knowledge like a kraken hoards
shipwrecks -- reaching into your PDFs, your contracts, your dusty email
archives, pulling out what matters, and indexing it before you even knew you
needed it. She reads everything you give her -- twelve nines of
reliability, one nine more than Amazon promises for not losing your
files. She does not know what downtime means. She claims she was here
before the word was invented -- her git log disagrees. She is part librarian,
part eldritch horror, part *folklore* -- an oral tradition for the digital
age, except she actually remembers things correctly -- until you
`lore ingest --recreate` and wipe her memory clean. She does not hold
grudges. She just re-reads everything -- and fast. You will see progress
bars flicker and vanish before you can read their labels, documents
streaming past like debris in a whirlpool. By the time you glance back
at the terminal, she is already done and waiting for your next question.
Old lady with tentacles, remember? Parallel I/O.

She is not a fancy AI. She does not hallucinate your documents. She was,
however, largely written by one -- but that is a family secret. She uses
BM25 like it is 1994 because it works and because she does not need a GPU
to tell you where you wrote down that one authentication thing three months
ago -- and to quietly surface a memo you never opened that would have saved
you the trouble. She is a single binary -- all tentacles in one body,
hunting alone in the deep. No cloud above her, the sun does not reach
that far down. No dependencies, no entourage, no committee. Just her and
strong opinions about how to slice a shipwreck into readable pieces.

Will she change your life? No. If your problem is 200 Markdown files,
use ripgrep -- you are fine. But when it is PDFs, Word documents,
scanned invoices, HTML exports, and that one spreadsheet Karen from
accounting sent as a .zip and you are too tired to care which tool opens
what -- that is where lore lives. She will find that stupid Q3 number at
11 PM so you do not have to open Excel. As for Karen -- lore says she
was "processed." We do not ask follow-up questions.

Some say she keeps more than one lair. One for the things you wrote,
one for the things sent to you, one for the things you inherited and
never dared open -- each hoard sealed off, each minding its own
business. They do not gossip. But she remembers every single one, and
if you ask the right question she will search them all at once -- one
question, every lair, answers rising from the deep already sorted by
which ones matter most. She calls it federation. We call it unsettling.

Give her your documents. She loves reading them. She loves reading them 
*to* you even more -- come in, sit down, have a cookie -- yes, granny's 
cookies. She baked them. Do not ask what is in them.

You already found her.

</details>

She is still reading.
