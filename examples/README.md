# Examples

Each directory contains a self-contained `lore.yaml` config that works out
of the box. Run any example from its directory:

```bash
cd examples/lore
lore ingest
lore serve
```

Or from the repo root:

```bash
lore --config examples/lore/lore.yaml ingest
```

Use `preview` to inspect how documents are chunked before ingesting:

```bash
cd examples/lore
lore preview ../../docs
```

## Examples

### lore

**Source types:** `path`, `git`

Indexes lore's own documentation from the local checkout alongside
kreuzberg (the upstream document extraction library) fetched via git.
Topics separate the two projects, demonstrating how a single knowledge
base can combine local paths, external git repositories, and topic-based
document separation.

Requires a local checkout of the lore repository.

### sitemap

**Source types:** `sitemap`

Public documentation fetched via XML sitemap. lore discovers all pages
listed in the sitemap, fetches them, and indexes their content. Includes
fetch settings for polite crawling (rate limiting, robots.txt).

```bash
cd examples/sitemap
lore ingest
lore search "attributes"
```

### gutenberg

**Source types:** `feed`, `url`

Ten classic novels fetched directly from Project Gutenberg as plain-text
downloads, plus a daily RSS feed of recently added or updated ebooks.
Uses pipeline transforms to strip Gutenberg boilerplate and extract the
book title as the topic. Works out of the box -- no local files needed.

```bash
cd examples/gutenberg
lore ingest
lore search "whale"
```

A dedicated `bench.yaml` config is provided for performance benchmarking
(see [Performance](../docs/performance.md)). To run it, download the full
Gutenberg plain-text archive (~11 GB compressed, 76K+ books) and ingest:

```bash
curl -O https://www.gutenberg.org/cache/epub/feeds/txt-files.tar.zip
mkdir -p ~/gutenberg && unzip txt-files.tar.zip && tar xf txt-files.tar -C ~/gutenberg
lore --config examples/gutenberg/bench.yaml ingest --recreate
```

### youtube

**Source types:** `youtube`

Educational YouTube videos demonstrating transcript ingestion. lore
spawns `yt-dlp` to fetch captions and indexes the full transcript text.

Requires `yt-dlp` on PATH:

```bash
cd examples/youtube
lore ingest
lore search "neural network"
```

### exec

**Source types:** `exec`

Demonstrates the `exec` source type by running a Python script that emits
JSONL documents to stdout. The sample dataset is a small kraken-themed
knowledge base -- a nod to the sea monster that guards all lore beneath
the deep. No external dependencies or credentials needed.

```bash
cd examples/exec
lore ingest
lore search "kraken"
```

### zotero

**Source types:** `path`

Research paper library managed by Zotero. Indexes all PDFs from Zotero's
local storage directory, including scanned papers via built-in OCR.
Processing settings strip citation lists (References, Bibliography) to
keep search results focused on paper content.

Requires a local Zotero installation with the default data directory:

```bash
cd examples/zotero
lore ingest
lore search "attention mechanism"
```

---

## Config Templates

The configs below require credentials, local files, or non-default
feature flags and cannot run out of the box. Use them as starting
templates.

### LLM enrichment

**Source types:** `path`
**Features:** `llm`

Local markdown articles enriched by a local Ollama instance.
Demonstrates LLM-powered topic detection, chunk quality scoring,
and keyword/summary generation.

The `llm` feature is not compiled in by default. Build with the flag
enabled, start Ollama, and pull the model:

```bash
cargo install --path . --features llm
ollama pull llama3.2
ollama serve                           # if not already running
cd examples/llm
lore ingest
```

After ingesting, LLM-generated summaries, keywords, and quality scores
are visible in JSON output:

```bash
lore search "memory safety" --json
```

Low-quality chunks (boilerplate, noise) are automatically filtered out
based on the `quality_threshold` setting.

### Research stack (Zotero + notes + course materials)

Combined research knowledge base indexing Zotero papers, Obsidian notes,
and course materials. The `drop_sections` setting removes citation lists
and boilerplate to keep search results focused on substance.

```yaml
name: research
description: Papers, notes, and course materials

sources:
  - path: ~/Zotero/storage
    glob: "**/*.pdf"
    topic: Papers

  - path: ~/Documents/ObsidianVault
    glob: "**/*.md"
    topic: Notes

  - path: ~/Documents/courses
    glob: "**/*.{pdf,pptx,docx}"
    topic: Courses

store:
  path: ./store

processing:
  drop_sections: [References, Bibliography, Acknowledgements, Funding]
  content_filter:
    strip_repeating_text: true
    include_headers: false
    include_footers: false
```

### Multilingual research (federated)

Separate stores per language, queried together via federation. Each store
uses its own stemmer for accurate full-text search. Query both with
`lore search "query" -c english.yaml -c german.yaml`.

```yaml
# english.yaml
sources:
  - path: ~/papers/english
    glob: "**/*.pdf"
store:
  path: ./store-en
  language: english
```

```yaml
# german.yaml
sources:
  - path: ~/papers/german
    glob: "**/*.pdf"
store:
  path: ./store-de
  language: german
```

### Multi-format knowledge base

Mixed document types with per-source processing presets:

```yaml
name: team-knowledge
description: PDFs, emails, spreadsheets, and wiki exports

sources:
  - path: ~/Documents/policies
    glob: "**/*.pdf"
    topic: Policies

  - path: ~/Documents/contracts
    glob: "**/*.{pdf,docx}"
    topic: Contracts

  - path: ~/mail-export
    glob: "**/*.eml"
    topic: Email
    processing: email

  - url:
      - https://wiki.internal.co/export/team-a.html
      - https://wiki.internal.co/export/team-b.html
    topic: Wiki
    headers:
      Authorization: "Bearer ${LORE_WIKI_TOKEN}"

store:
  path: ./store

processing:
  max_chunk_chars: 2000
  presets:
    email:
      max_chunk_chars: 800
      extract: builtin
```

### Authenticated sitemap with rate limiting

```yaml
name: internal-docs
description: Internal documentation behind auth

sources:
  - sitemap: https://docs.internal.co/sitemap.xml
    include: "/api/|/guides/"
    topic: Internal Docs
    headers:
      Authorization: "Bearer ${LORE_DOCS_TOKEN}"

store:
  path: ./store

fetch:
  concurrency: 2
  delay: 1.0
  timeout: 60.0
  respect_robots: true
```

### S3 bucket

Requires `--features s3` and AWS credentials via the default provider
chain.

```yaml
name: reports
description: Monthly reports stored in S3

sources:
  - s3: s3://company-reports/2024/
    glob: "**/*.pdf"
    topic: Reports

  - s3: s3://company-data/exports/
    include: "\.xlsx$"
    topic: Data Exports

store:
  path: ./store
```
