# Supported Document Formats

lore uses [kreuzberg](https://github.com/kreuzberg-dev/kreuzberg) for document
extraction, removing the need to install format-specific tools or know which
parser handles which file type. Content is automatically detected by file
extension and MIME type -- you point lore at your files and it figures out the
rest.

## Formats

Every document is assigned a `kind` based on its file extension and MIME
type. You can filter by kind in search (`--kind`) and document listing
(`--kind`). The four kinds are:

### document (default)

PDF, DOCX, PPTX, ODT, ODP, EPUB, RTF, FictionBook (fb2), XLSX, XLS, ODS,
HTML, Markdown, reStructuredText, Org-mode, LaTeX, OPML, images (PNG,
JPEG, TIFF, BMP, GIF, WebP -- requires `ocr` feature), Apple iWork
(Keynote, Pages, Numbers -- requires `iwork` feature), archives
(<!-- BEGIN GENERATED: format-archive-extensions -->
`zip`, `tar`, `gz`, `bz2`, `xz`, `zst`, `tar.gz`, `tar.bz2`, `tar.xz`, `tar.zip`, `tar.zst`, `tar.zstd`
<!-- END GENERATED: format-archive-extensions -->),
YouTube video transcripts (fetched via `yt-dlp`), and any plain text file
not matched by the other kinds.

### code

Source code files detected by extension:
<!-- BEGIN GENERATED: format-code-extensions -->
`rs`, `py`, `js`, `ts`, `go`, `java`, `c`, `cpp`, `h`, `hpp`, `cs`, `rb`, `sh`, `bash`, `zsh`, `swift`, `kt`, `scala`, `zig`, `lua`, `pl`, `r`, `m`, `php`, `ex`, `erl`, `hs`, `ml`, `clj`, `v`, `sv`, `vhd`, `s`, `asm`, `sql`, `proto`, `thrift`, `graphql`, `tf`, `nix`, `jsx`, `tsx`, `vue`, `svelte`, `css`, `scss`, `less`, `sass`
<!-- END GENERATED: format-code-extensions -->

Also matched by MIME types containing `text/x-`, `x-source`, or `x-script`.
Optionally parsed via tree-sitter (requires `tree-sitter` feature).

### data

Structured data files:
<!-- BEGIN GENERATED: format-data-extensions -->
`json`, `yaml`, `yml`, `toml`, `ini`, `properties`, `csv`, `tsv`, `xml`, `xsd`, `wsdl`, `avro`, `parquet`
<!-- END GENERATED: format-data-extensions -->

### email

Email formats:
<!-- BEGIN GENERATED: format-email-extensions -->
`eml`, `mbox`, `pst`
<!-- END GENERATED: format-email-extensions -->

MSG is also detected via MIME type.

## How it works

1. lore resolves the MIME type from the file extension (with overrides for
   extensions where `mime_guess` returns incorrect types).
2. Binary and rich-text formats (PDF, DOCX, HTML, XML, CSV, etc.) go through
   kreuzberg, which extracts text content and metadata (title, author, language,
   creation date, tags).
3. Some text formats (HTML, XML, CSV, TSV, RST, Org-mode) are routed through
   kreuzberg despite having `text/*` MIME types, because kreuzberg extracts
   structured content better than raw UTF-8 reading. LaTeX (`.tex`) is
   treated as a binary format (`application/x-latex`) and always goes through
   kreuzberg.
4. Remaining plain text files (source code, config files, Markdown, etc.)
   are read as UTF-8 directly. Metadata is extracted from frontmatter
   (YAML `---`, TOML `+++`), key-value headers, and heuristic patterns.
5. Archives are extracted to a temporary directory and each file inside is
   processed individually.

**`extract: kreuzberg` mode** -- For binary-heavy sources where you want
only kreuzberg's document metadata (from PDF/DOCX properties, etc.),
set `extract: kreuzberg` in the source's processing settings. This uses
kreuzberg metadata for binary files and skips builtin text heuristics.
Note: plain text files receive no metadata extraction in this mode
(kreuzberg has no binary metadata for plain text). Pair with
`extract_builtin` or `extract_metadata` pipeline steps if you need
metadata for text files too. See
[Pipeline Transforms](./configuration.md#pipeline-transforms) for
details on these steps.

## Feature flags

<!-- BEGIN GENERATED: compile-features-formats -->
| Flag | What it adds |
|------|--------------|
| `ocr` (default: on) | OCR for scanned PDFs and images. Builds tesseract from source, requires cmake. |
| `llm` | LLM enrichment: per-chunk quality scoring, summaries, and keyword generation via Ollama, Anthropic, OpenAI, or Bedrock. Enriched fields are indexed and boost search relevance. |
| `iwork` | Apple Keynote, Pages, Numbers |
| `tree-sitter` | Language-aware source code parsing |
<!-- END GENERATED: compile-features-formats -->

## Adding custom text extensions

If lore does not recognize a file extension as text, add it to
`processing.text_extensions` in your config:

```yaml
processing:
  text_extensions:
    - mdx
    - astro
    - svelte
```
