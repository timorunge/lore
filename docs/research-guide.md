# Research Guide

How to use lore as a local search engine for academic papers, research
notes, and course materials.

## The problem

Researchers accumulate hundreds or thousands of documents across multiple
formats and directories: PDFs from Zotero, Markdown notes in Obsidian,
LaTeX sources, HTML exports from journal sites, scanned papers that need
OCR. There is no unified way to full-text search across all of them
locally without uploading anything to the cloud.

lore solves this by indexing everything into a single BM25 search index
that works entirely offline.

## Quick start with Zotero

Zotero stores PDF attachments in `~/Zotero/storage/<8-char-key>/<file>.pdf`.
lore walks this directory structure natively:

```yaml
sources:
  - path: ~/Zotero/storage
    glob: "**/*.pdf"
    topic: Papers

processing:
  drop_sections: [References, Bibliography, Acknowledgements]
```

```bash
lore ingest
lore search "CRISPR off-target effects"
lore search "attention mechanism" --author "Vaswani"
```

If you use a custom Zotero data directory, adjust the path accordingly
(check Zotero preferences under Advanced > Files and Folders).

### OCR for scanned papers

lore has built-in OCR (tesseract, compiled in). Scanned PDFs are
recognized transparently with no configuration. This means older papers,
book scans, and handwritten-annotation PDFs are all searchable.

### Auto-indexing new papers

Run `lore watch` to monitor the Zotero storage directory for changes.
When you download a new paper in Zotero, it appears in search results
within seconds:

```bash
lore watch
```

## Full research stack

Combine papers, notes, and course materials in one knowledge base:

```yaml
name: Research Knowledge Base
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
  drop_sections:
    - References
    - Bibliography
    - Acknowledgements
    - Acknowledgments
    - Funding
  content_filter:
    strip_repeating_text: true
    include_headers: false
    include_footers: false
```

### Filtering by topic

With topics assigned, search specific subsets of your knowledge base:

```bash
lore search "gradient descent" --topic Papers
lore search "meeting notes" --topic Notes
lore search "lecture 5" --topic Courses
```

### Filtering by format

Find only PDFs, only Markdown notes, or only presentations:

```bash
lore search "neural network" --format pdf
lore search "TODO" --format md
lore search "architecture" --format pptx
```

## Multilingual research

The `store.language` setting selects the stemming language. For a
monolingual corpus, set it directly:

```yaml
store:
  language: german
```

For multilingual corpora, create separate stores and query them together
via federation:

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

Query both simultaneously:

```bash
lore search "quantum entanglement" -c english.yaml -c german.yaml
```

Each store applies its own stemmer (English stemming finds "entangled"
from "entanglement"; German stemming finds "Verschraenkung" from
"Verschraenkungen"), and results are merged by BM25 score.

Supported languages: arabic, danish, dutch, english, finnish, french,
german, greek, hungarian, italian, norwegian, portuguese, romanian,
russian, spanish, swedish, tamil, turkish.

## Processing tips for academic PDFs

### Drop noisy sections

Citation lists dominate academic PDFs. Without filtering, searching for
an author name returns every paper that cites them in the References
section. The `drop_sections` setting removes entire sections by heading:

```yaml
processing:
  drop_sections:
    - References
    - Bibliography
    - Acknowledgements
    - Acknowledgments
    - Funding
    - Appendix
```

### Strip repeating headers and footers

Journal PDFs often repeat the journal name, DOI, or page numbers on
every page. The content filter removes this noise:

```yaml
processing:
  content_filter:
    strip_repeating_text: true
    include_headers: false
    include_footers: false
```

### Adjust chunk size for dense papers

The default chunk size (1600 characters) works well for most documents.
For dense technical papers where you want more context per result,
increase it:

```yaml
processing:
  max_chunk_chars: 2400
```

## Using lore with an AI assistant

Connect lore to Claude Code, Cursor, or any MCP-capable assistant to do
AI-assisted literature review. The agent can search your papers, read
specific documents, and synthesize information across your corpus.

```json
{
  "mcpServers": {
    "papers": {
      "command": "lore",
      "args": ["serve"],
      "env": {
        "LORE_CONFIG": "/absolute/path/to/.lore/lore.yaml"
      }
    }
  }
}
```

Then ask your assistant:

- "What papers in my library discuss attention mechanisms?"
- "Summarize the methodology used in papers about CRISPR"
- "Find contradictions between papers on topic X"
- "What's the consensus on Y across my research corpus?"

See [MCP Integration](mcp-integration.md) for full setup instructions.

## Comparison with alternatives

| Feature | lore | Zotero Search | DEVONthink | Recoll |
|---------|------|---------------|------------|--------|
| Cross-PDF full-text | Yes | Limited | Yes | Yes |
| OCR for scans | Built-in | No | Yes | Via plugin |
| Non-PDF formats | 90+ | No | Yes | Yes |
| Local/offline | Yes | Yes | Yes | Yes |
| Free/open source | MIT | GPL | $199 | GPL |
| macOS/Linux/Windows | All | All | macOS only | Linux (mainly) |
| CLI interface | Yes | No | Limited | Yes |
| AI assistant (MCP) | Yes | No | No | No |
| Watch mode | Yes | No | Yes | Yes |
| Setup complexity | Moderate | Low | Low | High |
