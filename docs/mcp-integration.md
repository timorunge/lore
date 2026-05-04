# MCP Server Integration

lore serves knowledge bases to LLM agents over the
[Model Context Protocol](https://modelcontextprotocol.io/).
Agents search and retrieve content through six read-only tools without
needing to know anything about the underlying document formats, extraction
tools, or index implementation -- they just consume knowledge.

## End-to-end walkthrough

This walkthrough connects lore to an MCP-capable AI assistant in under
5 minutes. It uses lore's own documentation as the knowledge base -- a
self-referential demo.

### 1. Build the knowledge base

```bash
cd /path/to/lore        # or any project with docs
lore init               # scaffold .lore/lore.yaml
lore ingest             # index everything
lore search "chunking"  # verify it works
```

### 2. Configure your MCP client

Add the following to your MCP client config. The exact location depends
on the client:

| Client | Config file |
|--------|-------------|
| **Claude Code** | `.claude/settings.local.json` or `.mcp.json` in the project root |
| **Claude Desktop** | `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) |
| **Cursor** | `.cursor/mcp.json` in the project root |
| **Windsurf** | `~/.codeium/windsurf/mcp_config.json` |
| **Kiro** | `.kiro/settings/mcp.json` in the project root |
| **Cline** | VS Code settings JSON under `cline.mcpServers` |
| **Continue** | `~/.continue/config.json` |

lore ships a `/lore` skill in [skills/lore/](../skills/lore/) for AI
coding assistants that support skills. Type `/lore` for interactive
setup help.

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

Replace `/path/to/.lore/lore.yaml` with the absolute path to your
config file. Use an absolute path because MCP clients launch the server
as a subprocess whose working directory is unpredictable -- a relative
path will resolve against the wrong directory. See [Setup](#setup) for
alternative configuration approaches, HTTP transport, and
multi-knowledge-base setups.

### 3. Use it

Restart your AI client (or reload MCP servers). The agent now has access
to six `lore_*` tools. Try asking:

- "What document formats does this knowledge base cover?"
- "Search for how authentication works"
- "List all topics and give me a summary of each"

The agent will call `lore_info` to understand the knowledge base, then
`lore_search` or `lore_read_topic` to retrieve content. No prompt
stuffing -- the agent pulls what it needs on demand.

## Setup

### Transports

lore supports two transport modes:

| Transport | Command | Protocol |
|-----------|---------|----------|
| **stdio** (default) | `lore serve` | JSON-RPC over stdin/stdout |
| **Streamable HTTP** | `lore serve --transport http` | HTTP + SSE on `/mcp` |

The server automatically detects store changes made by an external
`lore ingest` process and reloads its caches within seconds -- no restart
or special flags needed. This means you can run `lore serve` in one
terminal and `lore ingest` in another, and the server picks up the new
data automatically.

For a fully self-contained setup, `--watch` adds an embedded file-watcher
that monitors local source files and triggers re-ingest inside the server
process itself:

```bash
lore serve --watch                   # stdio with embedded re-ingest
lore serve --transport http --watch  # HTTP with embedded re-ingest
lore serve --watch --debounce 5      # wait 5s after last change (default: 2)
```

### Stdio

The [walkthrough above](#2-configure-your-mcp-client) shows the
recommended setup using the `LORE_CONFIG` environment variable.
Alternatively, pass the config path as a flag:

```json
{
  "mcpServers": {
    "my-docs": {
      "command": "lore",
      "args": ["--config", "/path/to/lore.yaml", "serve"]
    }
  }
}
```

### Streamable HTTP

Start the server and point your MCP client at the endpoint:

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

This is useful when the MCP client cannot launch subprocesses (web apps,
remote agents) or when you want a single long-running server that multiple
clients can connect to.

<!-- BEGIN GENERATED: flags-serve-http -->
| Flag | Default | Description |
|------|---------|-------------|
| `--host` | `127.0.0.1` | Bind address for HTTP transport (default: 127.0.0.1) |
| `--port` | `8080` | Port for HTTP transport (default: 8080) |
| `--expose` |  | Allow binding to a non-loopback address (required when --host is not localhost) |
<!-- END GENERATED: flags-serve-http -->

```bash
lore serve --transport http                                      # localhost:8080
lore serve --transport http --host 0.0.0.0 --port 3000 --expose  # all interfaces, port 3000
```

`--expose` is required whenever `--host` is set to a non-loopback address
(anything other than `127.0.0.1`, `::1`, or `localhost`). This is a
deliberate safety check: binding to all interfaces exposes the server on
your network. Use `--token` to require Bearer token authentication.

### Multiple knowledge bases

Pass multiple `-c` flags to serve several knowledge bases through a
single MCP server. lore federates them at query time -- agents see one
unified set of tools with merged results.

**Stdio:**

```json
{
  "mcpServers": {
    "my-docs": {
      "command": "lore",
      "args": [
        "serve",
        "-c", "/path/to/project/.lore/lore.yaml",
        "-c", "/path/to/api-ref/.lore/lore.yaml"
      ]
    }
  }
}
```

Or set `LORE_CONFIG` with colon-separated paths:

```json
{
  "mcpServers": {
    "my-docs": {
      "command": "lore",
      "args": ["serve"],
      "env": {
        "LORE_CONFIG": "/path/to/project/.lore/lore.yaml:/path/to/api-ref/.lore/lore.yaml"
      }
    }
  }
}
```

**HTTP:**

```bash
lore serve --transport http -c project.yaml -c api-ref.yaml
```

**Separate instances** -- if you prefer agents to see each knowledge base
as an independent MCP server, configure separate entries instead:

```json
{
  "mcpServers": {
    "project-docs": {
      "command": "lore",
      "args": ["serve"],
      "env": { "LORE_CONFIG": "/path/to/project/.lore/lore.yaml" }
    },
    "api-reference": {
      "command": "lore",
      "args": ["serve"],
      "env": { "LORE_CONFIG": "/path/to/api-ref/.lore/lore.yaml" }
    }
  }
}
```

## Available tools

The server exposes six read-only tools:

### lore_info

Overview of the knowledge base: name, description, document count, chunk
count, topics, languages, and last-updated timestamp (only present when
metadata is available). Takes no parameters. Use this as a first step to
understand what content is available before searching or browsing.

### lore_list_topics

List all topics with chunk counts.

Topic and author filters use fuzzy matching: exact match, then
case-insensitive, then substring.

<!-- BEGIN GENERATED: params-lore_list_topics -->
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `limit` | integer | required | Maximum number of items to return (minimum 1, maximum 500). |
| `offset` | integer | required | Number of items to skip before returning results. |
| `topic` | any | -- | Filter by topic name (fuzzy: exact, case-insensitive, then substring). |
| `author` | any | -- | Filter by author (fuzzy: exact, case-insensitive, then substring). |
| `source` | any | -- | Filter by source path (substring match, case-sensitive). |
| `lang` | any | -- | Filter by language code (e.g. "en", "de"). |
| `origin` | any | -- | Filter by source origin (e.g. "local", "url", "git", "s3"). |
| `kind` | any | -- | Filter by document kind (e.g. "document", "code", "data"). |
| `format` | any | -- | Filter by file format (e.g. "md", "pdf", "rs", "json"). |
| `sort` | string | -- | Sort order for topic listings. |
| `reverse` | any | -- | Reverse the sort order. |
<!-- END GENERATED: params-lore_list_topics -->

### lore_search

Full-text search across document body, title, section headings, and tags.

Topic and author filters use fuzzy matching: exact match, then
case-insensitive, then substring.

<!-- BEGIN GENERATED: params-lore_search -->
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `query` | string | required | Free-text search query. Supports English stemming. |
| `limit` | integer | required | Maximum number of items to return (minimum 1, maximum 500). |
| `offset` | integer | required | Number of items to skip before returning results. |
| `source` | any | -- | Filter by source path (substring match, case-sensitive). |
| `topic` | any | -- | Filter results to a specific topic name. |
| `author` | any | -- | Filter results by author. |
| `lang` | any | -- | Filter results by language code (e.g. "en", "de"). |
| `origin` | any | -- | Filter by source origin (e.g. "local", "url", "git", "s3"). |
| `kind` | any | -- | Filter by document kind (e.g. "document", "code", "data"). |
| `format` | any | -- | Filter by file format (e.g. "md", "pdf", "rs", "json"). |
| `max_per_source` | any | -- | Maximum results per source document for diversity. When set,
over-fetches and filters to ensure diverse sources in results. |
| `sort` | string | -- | Sort order for search results. |
| `reverse` | any | -- | Reverse the sort order. |
<!-- END GENERATED: params-lore_search -->

### lore_read_topic

Read all chunks for a topic. Prefer this over lore_search for complete
coverage.

<!-- BEGIN GENERATED: params-lore_read_topic -->
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `topic` | string | required | Topic name. Resolves via exact match, case-insensitive match, then substring. |
| `limit` | integer | required | Maximum number of items to return (minimum 1, maximum 500). |
| `offset` | integer | required | Number of items to skip before returning results. |
<!-- END GENERATED: params-lore_read_topic -->

### lore_list_docs

List documents with optional filters. Source paths are stable identifiers
for lore_read_doc.

<!-- BEGIN GENERATED: params-lore_list_docs -->
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `limit` | integer | required | Maximum number of items to return (minimum 1, maximum 500). |
| `offset` | integer | required | Number of items to skip before returning results. |
| `source` | any | -- | Filter by source path (substring match, case-sensitive). |
| `topic` | any | -- | Filter by topic name. |
| `title` | any | -- | Filter by title (substring match). |
| `author` | any | -- | Filter by author. |
| `lang` | any | -- | Filter by language code. |
| `origin` | any | -- | Filter by source origin (e.g. "local", "url", "git", "s3"). |
| `kind` | any | -- | Filter by document kind (e.g. "document", "code", "data"). |
| `format` | any | -- | Filter by file format (e.g. "md", "pdf", "rs", "json"). |
| `sort` | string | -- | Sort order for document listings. |
| `reverse` | any | -- | Reverse the sort order. |
<!-- END GENERATED: params-lore_list_docs -->

### lore_read_doc

Read a document's chunks sequentially. Accepts partial paths for convenience.

<!-- BEGIN GENERATED: params-lore_read_doc -->
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `source` | string | required | Full path, partial path, or basename of the document. |
| `limit` | integer | required | Maximum number of items to return (minimum 1, maximum 500). |
| `offset` | integer | required | Number of items to skip before returning results. |
| `full` | boolean | false | Show the full document as continuous text without chunk separators. |
<!-- END GENERATED: params-lore_read_doc -->

## Available resources

The server exposes every topic and document as a concrete MCP resource.
Clients that call `list_resources` discover the full knowledge base
without needing to call a tool first.

| URI pattern | Name | Description | MIME type |
|-------------|------|-------------|-----------|
| `lore://info` | Knowledge Base Info | Document count, topics, languages | `text/plain` |
| `lore://topics/{name}` | Topic: {name} | Doc and chunk counts | `text/markdown` |
| `lore://docs/{source}` | Document title or filename | Topic and chunk count | `text/markdown` |

Resources are returned in a stable order: `lore://info` first, then
topics alphabetically, then documents by source path. Results are
cursor-paginated (200 per page) so large stores work transparently.

The server also advertises two **resource templates** via
`list_resource_templates` for clients that prefer to construct URIs
directly:

| URI template | Description |
|--------------|-------------|
| `lore://topics/{name}` | First 500 chunks for a topic |
| `lore://docs/{path}` | First 500 chunks for a document |

**URI examples:**

- `lore://topics/authentication` -- reads all chunks in the
  `authentication` topic. Topic name resolution follows the same
  fuzzy-match rules as `lore_read_topic` (exact > case-insensitive >
  substring).
- `lore://docs//docs/api.md` -- reads chunks for the document whose
  source path is `/docs/api.md`. Use the full source path as reported by
  `lore_list_docs`; partial paths are also accepted (suffix > basename >
  stem).

Both URIs return up to 500 chunks. For large topics or documents, use
the `lore_read_topic` or `lore_read_doc` tools with `offset` pagination
instead.

## Server instructions

lore generates server instructions at startup from the knowledge base
metadata. These are sent to the MCP client during the protocol handshake
and help the agent understand what content is available and how to search
effectively. The instructions include:

- Knowledge base description (from lore.yaml `description` field)
- Search workflow guidance (topic/document/chunk hierarchy, filter and sort options, fuzzy matching rules)
- Topic list with chunk counts (up to 20 topics)
- Document and chunk totals, last-updated timestamp
