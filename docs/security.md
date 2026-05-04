# Security

How lore handles untrusted input and what it does not protect against.

## Reporting Vulnerabilities

Report security vulnerabilities through
[GitHub Security Advisories](https://github.com/timorunge/lore/security/advisories/new).
Do not open a public issue for security reports.

## Threat Model

lore ingests content from untrusted sources: local files, HTTP URLs, git
repositories, XML sitemaps, RSS feeds, S3 objects, YouTube videos, Maildir
email stores, shell command output, and upstream MCP servers. It processes
binary documents (PDFs, Office files, archives) and serves results over
MCP. Its security design focuses on safe handling of that untrusted input.

### What lore protects against

**SSRF (Server-Side Request Forgery).** HTTP requests use a custom DNS
resolver (`SafeResolver`) that filters out private and loopback IP addresses
at connect time, preventing DNS rebinding attacks. The resolver blocks:
loopback, unspecified (`0.0.0.0/8`), private (RFC 1918), link-local, CGNAT
(`100.64.0.0/10`), IETF protocol assignments (`192.0.0.0/24`), TEST-NET-1/2/3,
benchmarking (`198.18.0.0/15`), reserved (`240.0.0.0/4`), broadcast, and
multicast for IPv4; and loopback, unspecified, link-local (`fe80::/10`),
unique-local (`fc00::/7`), Teredo (`2001::/32`), 6to4 with private destinations
(`2002::/16`), NAT64 (`64:ff9b::/96`, `64:ff9b:1::/48`), documentation
(`2001:db8::/32`), benchmarking (`2001:2::/48`), ORCHIDv2 (`2001:20::/28`),
IPv4-mapped (`::ffff:0:0/96`), and multicast for IPv6. This applies to all
HTTP fetching:
URLs, sitemaps, feeds, and download requests. LLM provider connections
(Ollama, OpenAI, Anthropic, Bedrock) use a separate HTTP client without
SSRF filtering, since their endpoints come from trusted operator config.

**Archive bombs.** Archive extraction (zip, tar, gz, bz2, xz, zst) enforces
both a file count limit (default 5,000, configurable as
`processing.max_archive_files`) and a total size limit (default 2 GB,
configurable as `processing.max_archive_mb`). Path traversal is blocked: entries
with `..` components or absolute paths are rejected.

**Large file exhaustion.** File sizes are checked before reading. The
`processing.max_file_mb` config (default 50 MB) caps individual file sizes. HTTP
downloads enforce `Content-Length` pre-checks and running byte counters
against `fetch.max_download_mb`.

**Git URL injection.** Git source URLs are validated against an allowlist
of safe schemes (`https`, `http`, `ssh`, `git`). Git refs are validated
to prevent shell injection through crafted ref names.

**Config path traversal.** The `LORE_CACHE_DIR` environment variable
rejects paths containing `..` components and falls back to the default
cache location.

**MCP error leakage.** Server errors are logged with full details
(`tracing::error!`) but clients receive only a generic "Internal error --
check server logs." message. This prevents leaking file paths, stack
traces, or internal state to MCP clients.

**Predictable temp files.** Atomic file writes use `tempfile::NamedTempFile`
(random names in the target directory) instead of predictable `.tmp`
suffixes, preventing symlink attacks on the store sidecar write path.

**Unbounded MCP responses.** All paginated MCP tools enforce
`MAX_PAGE_LIMIT = 500` via Pagination deserialization, preventing clients
from requesting unbounded result sets.

### What lore does not protect against

**Malicious document content.** lore extracts text from binary formats
using kreuzberg (which wraps pdfium, calamine, and other parsers). A
specially crafted PDF or Office document could exploit vulnerabilities
in these parsers. Binary format extraction runs with timeouts (default 120s,
configurable as `processing.extraction_timeout_secs`) to contain hangs, but
this is mitigation, not prevention. Plain text files bypass kreuzberg and
these protections.

**Denial of service via CPU.** Processing a very large number of documents,
or documents that produce pathologically large text output, may consume
significant CPU and memory. The `processing.max_file_mb` limit and archive
extraction caps provide partial mitigation.

For very large archives, consider downloading and unpacking with native OS
tools (curl, tar, unzip) and pointing lore at the extracted directory instead
of having lore fetch and extract directly.

**Trust store integrity.** lore trusts the system's TLS certificates for
HTTPS connections. It does not verify that the trust store has not been
tampered with.

**LLM prompt injection.** When LLM enrichment is enabled, document content
is sent to the configured LLM provider. Malicious content could attempt
prompt injection. Document content is wrapped in `<document>` delimiters,
but this is not a security boundary.

## Dependencies

Security-relevant dependencies:

- `reqwest` -- HTTP client with TLS (rustls or native-tls)
- `kreuzberg` -- document extraction (wraps pdfium, calamine, etc.)
- `tantivy` -- full-text search engine (local, no network)
- `rmcp` -- MCP server framework (stdio and streamable HTTP transports)
- `axum` -- HTTP server (used by the streamable HTTP transport)
- `tempfile` -- secure temporary file creation
- `texting_robots` -- robots.txt parsing

All dependencies are tracked in `Cargo.lock`.

## Data Responsibility

lore is a tool that processes and indexes content you point it at. You are
responsible for:

- **Data ownership and licensing.** Ensure you have the right to ingest,
  index, and serve the content you configure as sources. This includes
  local files, web content fetched via URLs/sitemaps/feeds, git
  repositories, S3 objects, YouTube videos, email (maildir), shell
  command output (exec), and upstream MCP servers.

- **Compliance with terms of service.** Crawling websites, fetching RSS
  feeds, and downloading content may be subject to the source's terms of
  service. lore respects `robots.txt` by default, but robots.txt compliance
  does not guarantee legal permission.

- **LLM data transmission.** When LLM enrichment is enabled, document
  content is sent to the configured provider (Ollama, Bedrock, Anthropic,
  or OpenAI). You are responsible for understanding and accepting the data
  handling policies of your chosen provider. For sensitive or proprietary
  content, consider using a local provider like Ollama that keeps data on
  your own infrastructure.

- **Indexed content exposure.** Content in the store is served to any MCP
  client that connects to `lore serve`. When using the HTTP transport, the
  server is network-accessible -- bind to `127.0.0.1` (the default) to
  restrict access to localhost, or use a reverse proxy with authentication
  for remote access. Ensure that the content in your knowledge base is
  appropriate for the agents and users that will query it.

- **Personal data.** lore does not filter or redact personal data from
  ingested documents. If your sources contain personally identifiable
  information, you are responsible for compliance with applicable data
  protection regulations.

lore does not phone home, collect telemetry, or transmit data anywhere
except to explicitly configured LLM providers and the content sources
you define.

See also: [Architecture](./architecture.md),
[Configuration](./configuration.md).
