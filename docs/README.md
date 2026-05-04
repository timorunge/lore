# Documentation: Finding What You Need

An index of lore's documentation, organized so you can get to the right
page fast.

## What lore Does

lore is a local knowledge base you can search from the terminal or serve
to agents over MCP. Point it at your documents -- local files, URLs, git
repos, feeds, S3, YouTube, email, shell commands, upstream MCP servers --
and it handles extraction,
chunking, and indexing so you can focus on consuming knowledge instead of
figuring out how to read it.

The [project README](../README.md) covers installation and quick-start usage.

## Where to Start

Different starting points for different goals:

**Setting up a knowledge base for the first time?**
Start with the [Hands-on Guide](./hands-on-guide.md) for a step-by-step
walkthrough from install to serving.

**Connecting lore to Claude, Cursor, or another MCP client?**
The [MCP Integration](./mcp-integration.md) guide covers server setup,
tool reference, and multi-KB configurations.

**Tuning how documents are processed?**
The [Configuration](./configuration.md) reference covers every config
option with defaults and examples.

**Wondering what file formats are supported?**
See [Supported Formats](./formats.md) for the full list and how
extraction works.

**Want to see it in action first?**
The [examples/](../examples/) directory has runnable configs covering
local files, git repos, sitemaps, and RSS feeds.

**Want to search from the terminal?**
The [CLI Reference](./cli.md) covers the `search`, `topics`, `docs`,
and `read` commands for terminal-native querying.

**Understanding the internals?**
The [Architecture](./architecture.md) doc covers the ingest pipeline,
store design, serve layer, and key decisions.

**Want to contribute?**
See [Contributing](./contributing.md) for build setup, quality gates, and
how to add new source types or MCP tools.

## Documentation Map

### Getting Started

| Document | What It Covers |
|----------|----------------|
| [Hands-on Guide](./hands-on-guide.md) | Step-by-step walkthrough: install, init, ingest, serve, add sources, multiple knowledge bases. |
| [CLI Reference](./cli.md) | Commands, flags, examples, and exit codes. Covers search, topics, docs, read, and all other commands. |
| [Configuration](./configuration.md) | YAML config format, every setting with its default value, source types, environment variables, and cache management. |

### Integration

| Document | What It Covers |
|----------|----------------|
| [MCP Integration](./mcp-integration.md) | Server setup for Claude/Cursor/other MCP clients, full tool parameter reference, and multi-KB configurations. |
| [Supported Formats](./formats.md) | Document formats, MIME handling, metadata extraction, and how to add custom text extensions. |

### Deep Dives

| Document | What It Covers |
|----------|----------------|
| [Architecture](./architecture.md) | Module structure, ingest pipeline stages, store schema, serve layer, and key design decisions. |
| [Design Philosophy](./design-philosophy.md) | Why BM25, why single binary, two interfaces (CLI and MCP), and other key choices. |
| [Performance](./performance.md) | Benchmarks, tuning tips, chunk size tradeoffs, and index size guidance. |
| [Security](./security.md) | Threat model, SSRF protection, archive limits, input validation, and what lore does not protect against. |
| [Contributing](./contributing.md) | Dev setup, quality gates, how to add source types and MCP tools. |
| [Processing Profiles](./configuration.md#processing-profiles) | Presets, inline overrides, pipeline transforms, and per-source processing configuration. |

### Examples

| Directory | What It Covers |
|-----------|----------------|
| [examples/](../examples/) | Runnable configs and config templates: local files, git repos, sitemaps, RSS feeds, YouTube, LLM enrichment, multi-format, S3. See [examples/README.md](../examples/README.md). |

## Further Reading

- [Project README](../README.md) -- installation, quick-start, feature overview
