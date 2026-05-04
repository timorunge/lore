# Contributing

How to set up a development environment, run the quality gates, and submit
changes.

## Prerequisites

You need:

- **Rust** stable toolchain (install via [rustup](https://rustup.rs/))
- **cmake** -- required when building with the `ocr` feature (the default).
  Install via your system package manager (`brew install cmake`,
  `apt install cmake`, etc.)

Optional, depending on which features you build:

- **AWS credentials** -- only needed for the `s3` and `bedrock` features
  at runtime, not at compile time

## Getting Started

```bash
git clone https://github.com/timorunge/lore.git
cd lore
make setup
cargo build
cargo run -- --help
```

`make setup` installs a pre-commit hook that runs `make fmt`,
`make lint-conventions`, and `make lint` before every commit. Run it once
after cloning. The hook keeps the tree clean without requiring a full test
run on every commit (tests run in CI). If you need to land a WIP commit
quickly, `git commit --no-verify` skips the hook, but use it sparingly.

The default build includes the `ocr` feature, which requires cmake. To build
without it:

```bash
cargo build --no-default-features
```

To build with LLM enrichment (topic detection, summaries, quality scoring):

```bash
cargo build --features llm
```

## Feature Flags

<!-- BEGIN GENERATED: compile-features-contributing -->
| Feature | What it adds | Extra dep |
|---------|--------------|-----------|
| `ocr` (default) | OCR for scanned PDFs and images (requires cmake) | cmake |
| `llm` | LLM enrichment (`lore ingest`, `lore enrich`): Ollama, Anthropic, OpenAI, and Bedrock | AWS SDK (Bedrock) |
| `s3` | Amazon S3 source support | AWS SDK crates |
| `mcp` (default) | Upstream MCP server ingestion (resources and tool calls) | rmcp |
| `iwork` | Apple iWork documents (Keynote, Pages, Numbers) | none |
| `tree-sitter` | Source code parsing via tree-sitter | none |
| `test-support` | Exposes `store_test_support` helpers for integration tests | none |
<!-- END GENERATED: compile-features-contributing -->

Build with specific features:

```bash
cargo build --features llm,s3
cargo build --no-default-features --features iwork
```

The `llm` feature enables LLM enrichment with all four providers (Ollama,
Anthropic, OpenAI, Bedrock). The `s3` feature adds S3 sources. Both are off
by default.

## Development Workflow

### 1. Make your changes

See [Architecture](./architecture.md) for the full module structure. The short
version: `src/ingest/` handles the ingestion pipeline, `cli/src/serve/`
is the MCP server, `src/store/` is the search store, and `cli/src/main.rs`
is the CLI entry point.

### 2. Run the quality gates

The project includes a `Makefile` that combines all quality gates into a
single command:

```bash
make check
```

This runs formatting (`fmt`), convention linting (`lint-conventions`),
clippy (`lint`, across all feature combinations), rustdoc warnings (`doc`),
tests (`test`), and doc table verification (`check-docs`) in sequence. It
is the recommended way to validate changes before committing.

Individual targets are also available:

<!-- BEGIN GENERATED: make-targets -->
| Target | What it does |
|--------|--------------|
| `make build` | Build the binary (release, all features) |
| `make install` | Install the binary via cargo install |
| `make update` | Update all Cargo dependencies |
| `make fmt` | Check formatting (fails on diff) |
| `make fmt-fix` | Fix formatting (destructive) |
| `make lint-conventions` | Check code conventions (unwrap, import order, pub visibility) |
| `make lint` | Run clippy (all feature combos) |
| `make test-quick` | Run tests (default features only) |
| `make test` | Run tests (all feature combos) |
| `make fuzz` | Run all fuzz targets for 60 seconds each (requires cargo-fuzz and nightly) |
| `make doc` | Build docs (fails on warnings) |
| `make generate-docs` | Regenerate doc tables from code annotations |
| `make check-docs` | Verify doc tables are up to date |
| `make check` | Run all quality gates (fmt lint-conventions lint doc test check-docs deny) |
| `make ci-local` | Run CI workflow locally via act (ubuntu matrix only) |
| `make setup` | Install git hooks (run once after cloning) |
| `make clean` | Remove build artifacts |
| `make help` | Show this help |
<!-- END GENERATED: make-targets -->

All checks must pass. Zero tolerance for clippy warnings (`-D warnings` treats
them as errors, matching CI).

You can also run the underlying cargo commands directly:

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

### 3. Commit

Use [Conventional Commits](https://www.conventionalcommits.org/) format:
`<type>: <subject>`. Valid prefixes are `feat:`, `fix:`, `refactor:`,
`docs:`, `test:`, and `chore:`. Keep the subject under 72 characters and
use the commit body for context when the change is non-obvious.

Examples:

```
feat: add sitemap loader with nested sitemap support
fix: skip stale documents when ETag header is missing
refactor: extract batch size constant from discover
```

### 4. Open a pull request

Push your branch and open a PR against `main`. Make sure the three quality
gates above pass locally before pushing -- CI runs the same checks.

## Testing

Every test earns its keep. Test count is not a proxy for quality. A test
that does not catch a bug that no other test catches is overhead.

### Philosophy

Two layers:

- **Unit tests** -- inline `#[cfg(test)]` modules next to the code they
  test. Use for known input/output pairs, edge cases (empty input, zero,
  boundary conditions), error paths, and structural facts.
- **Integration tests** -- in `cli/tests/integration/`. Use `assert_cmd` and `predicates`
  to exercise the CLI end-to-end. These require a working binary build.

When adding a loader or MCP tool, add at least one test covering the happy
path and one covering malformed input.

### What good looks like

A well-tested component has:

1. Unit tests for specific scenarios, edge cases, and error paths
2. No redundancy -- two tests hitting the same branch with trivially
   different inputs should be collapsed into one (or a table-driven test)
3. Integration tests at component boundaries where unit tests cannot
   reach (CLI behavior, config parsing from YAML strings, end-to-end
   ingest)

### Anti-patterns to avoid

1. **Multiple tests for the same branch** -- two functions exercise the
   same `if`/`match` arm with inputs that differ only in ways the branch
   does not inspect. Keep the most representative input, remove the rest.
2. **Tautological tests** -- assertions guaranteed true by type safety or
   struct initialization. If the test cannot fail without a compile error,
   delete it.
3. **Testing stdlib behavior** -- assertions that would only fail if Rust's
   standard library had a bug, not project code.
4. **Duplicate fixture, split assertions** -- two functions build identical
   fixtures but assert different fields. Merge into one function or use a
   table-driven test.
5. **Test-only code outside `#[cfg(test)]`** -- functions used only by
   tests must be gated with `#[cfg(test)]`. Do not leave `pub(crate)`
   functions in production code that exist solely to support tests.

### File structure

- Unit tests go in a `#[cfg(test)] mod tests { ... }` block at the bottom
  of the source file they test. Not in a separate file.
- Integration tests go in `cli/tests/integration/` as separate files.
- Test helpers, fixture builders, and shared constants go at the top of
  the `mod tests` block, before the test functions.
- Test functions should follow the source file's declaration order where
  practical.
- No section-divider banners (`// --------`) in test code. Use blank
  lines between groups.
- Mark helper functions with `#[allow(dead_code)]` only if they serve
  multiple test functions. Prefer inlining one-off setup.

### Running tests

```bash
make check             # all quality gates (fmt, lint, doc, test)
make test              # tests across all feature combinations
make test-quick        # tests with default features only (faster)
cargo test test_name   # run a single test by name
```

## Fuzz Testing

Fuzz targets live in `fuzz/fuzz_targets/`. They use
[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (backed by libFuzzer)
and require the Rust nightly toolchain.

### Setup

```bash
rustup toolchain install nightly
cargo install cargo-fuzz
```

### Running fuzz targets

Run all targets for 60 seconds each:

```bash
make fuzz
```

Or run a single target manually:

```bash
cargo +nightly fuzz run fuzz_sanitize_query
cargo +nightly fuzz run fuzz_visible_width
cargo +nightly fuzz run fuzz_config_parse
```

Pass `-max_total_time=N` to limit the run to `N` seconds, or omit it to run
until you interrupt with Ctrl-C.

### Available targets

| Target | What it fuzzes |
|--------|----------------|
| `fuzz_chunker` | `chunk_markdown_content` -- Markdown chunker (not included in `make fuzz`, run manually) |
| `fuzz_config_parse` | `IngestConfig` YAML deserialization |
| `fuzz_sanitize_query` | `sanitize_query` -- Tantivy query sanitizer |
| `fuzz_visible_width` | `visible_width` -- ANSI-aware visible width counter |

### Reproducing a crash

When cargo-fuzz finds a crash it writes an artifact to
`fuzz/artifacts/<target>/`. Reproduce it with:

```bash
cargo +nightly fuzz run <target> fuzz/artifacts/<target>/crash-<hash>
```

## MCP Inspector

MCP Inspector is useful for debugging the lore MCP server during development.

```bash
npx @modelcontextprotocol/inspector lore serve                   # stdio
npx @modelcontextprotocol/inspector lore serve --transport http  # streamable HTTP
```

This opens a web UI where you can invoke MCP tools manually and inspect
their inputs and outputs in real time.

### Custom config with the inspector

The inspector has its own `--config` flag which collides with lore's
`--config`. Passing `--config` directly causes the inspector to parse
the YAML file as its own JSON config, producing errors like
`"# Gutenber"... is not valid JSON`.

Use an inspector config file instead:

```bash
npx @modelcontextprotocol/inspector --config inspector.json
```

Where `inspector.json` contains:

```json
{
  "mcpServers": {
    "lore": {
      "command": "./target/release/lore",
      "args": ["--config", "examples/gutenberg/bench.yaml", "serve"]
    }
  }
}
```

## How to Add a New Source Type

1. **Add a loader** in `src/ingest/loaders/`. Create a new file (e.g.,
   `src/ingest/loaders/mytype.rs`) that returns
   `Result<(Vec<LoaderResult>, Vec<FailedDoc>)>` where `FailedDoc` (from
   `src/ingest/types.rs`) captures the source identifier and a human-readable
   reason for each failure. Use `Mutex<Vec<FailedDoc>>` for concurrent
   streams. Export it from `src/ingest/loaders/mod.rs`. For file-based sources
   that produce paths on disk, return those paths and let the pipeline's
   `SourceItems::Files` handle loading -- see the git loader for this pattern.

2. **Add a `SourceConfig` variant** in `src/config/source.rs`. The variant holds
   whatever configuration the new source needs (URL, path, credentials, etc.).
   Add the new discriminant key (the required field name) to the `SOURCE_KEYS`
   constant in `src/config/source.rs` so the two-pass deserializer recognises it
   and can suggest it in typo-correction messages.

3. **Wire it into discover** in `src/ingest/discover/`. Add a match arm in
   `discover_source()` that maps the new `SourceConfig` variant to a
   `DiscoveredSource`. For file-based sources, return `SourceItems::Files`.
   For everything else, return
   `DiscoveredSource::preloaded_with_failures(docs, failures)`.

4. **Propagate failures** via
   `DiscoveredSource::preloaded_with_failures(docs, failures)`. The
   `Vec<FailedDoc>` list flows through `streaming.rs` into
   `IngestResult.failed_docs`. The CLI prints each failure inline (up to 20)
   and writes the full list to a JSONL log in the cache directory. Sources
   with failures show `[! ]` (yellow) instead of `[+ ]` (green).

5. **Wrap external calls safely**. If the loader calls external libraries
   (kreuzberg, subprocess, native code), wrap calls in `tokio::task::spawn`
   for panic isolation and `suppress_native_stderr()` for stderr suppression.
   See the maildir loader (`src/ingest/loaders/maildir.rs`) for the canonical
   pattern, and `AGENTS.md` CONV-ISOLATE for the full template.

6. **Document it** in `docs/configuration.md` under the `sources` section.

If the new source requires optional dependencies (e.g., a cloud SDK), gate
it behind a new feature flag following the pattern of the `s3` feature in
`Cargo.toml`.

## How to Add a New MCP Tool

All MCP tool implementations live in `cli/src/serve/` (module with
`mod.rs` and `tools.rs`) on the `LoreServer` struct.

1. **Add the handler method** to `LoreServer`. The method signature must be
   compatible with the `rmcp` macro -- look at the existing six tools for
   the pattern (input struct, `Result<CallToolResult, McpError>` return type).

2. **Define input/output types** if needed. Input structs go in the handler
   method or inline. Structured output types belong in `cli/src/serve/tools.rs`.

3. **Register the tool** by adding the `#[tool]` attribute and including the
   new method in the `#[tool(tool_box)]` impl block on `LoreServer`.

4. **Format responses** using helpers in `cli/src/serve/tools.rs`. All tool
   responses are markdown strings. Keep response size bounded -- check the
   `MAX_PAGE_LIMIT` constant in `src/query.rs`.

5. **Add a test** in the `#[cfg(test)]` block in `cli/src/serve/tools.rs`
   covering the new tool with a mock store.

## How to Add a New MCP Resource

MCP resource handling lives in `cli/src/serve/resources.rs`. Resources
are exposed via three `ServerHandler` methods on `LoreServer`
(`list_resources`, `list_resource_templates`, `read_resource`) in
`cli/src/serve/mod.rs`.

**Static resources** (a single, fixed URI):

1. Add a `const` for the URI in `resources.rs`.
2. Extend the `ListResourcesResult` in `list_resources()` with a new
   `RawResource::new(URI, name).with_description(...).no_annotation()` entry.
3. Add a match arm in `read_resource()` for the new URI that calls the
   appropriate search or store function and returns a `ReadResourceResult`.
4. Add a unit test covering the happy path and unknown-URI error.

**URI-template resources** (parameterized URIs):

1. Add a `const` for the URI prefix (e.g. `lore://mytype/`) in
   `resources.rs`.
2. Add the template to `list_resource_templates()` using
   `RawResourceTemplate::new(template, name).no_annotation()`.
3. Add a `strip_prefix` match arm in `read_resource()` that extracts
   the parameter and calls the appropriate search function.
4. Add a unit test covering the happy path, parameter extraction, and
   unknown-URI error.

Keep response sizes bounded -- prefer `Pagination::new(500, 0)` for the
initial fetch and document any per-resource caps. Log internal failures
with `tracing::error!` before returning
`ErrorData::internal_error(...)`.

## Code Style

- `make lint` must pass with zero issues (runs clippy across all feature
  combinations)
- `cargo fmt --check` must pass
- Use `anyhow::Result` for fallible functions, `anyhow::bail!` / `anyhow::Context`
  for error propagation with context
- Prefer `Option<X>` over verbose patterns; use `unwrap_or_else`, `map`,
  and `?` rather than explicit `match` on `None`
- Feature-gate any code that pulls in optional dependencies

## Documentation Generation

Many tables in the documentation are auto-generated from code to prevent
drift. The system lives in `xtask/src/` and is invoked via:

```bash
make generate-docs  # regenerate all tables
make check-docs     # verify tables are up to date (CI gate)
```

### How it works

Doc files use HTML comment markers to delimit generated regions:

```html
<!-- BEGIN GENERATED: marker-id -->
| ... generated table ... |
<!-- END GENERATED: marker-id -->
```

Each xtask module registers `(marker-id, content)` pairs. The `inject.rs`
engine replaces content between matching markers. Running twice in a row
produces "up-to-date" for all files (idempotent).

### Modules

| Module | Data source | Generates |
|--------|-------------|-----------|
| `cli.rs` | clap metadata | CLI flag tables, command summary |
| `config.rs` | `schemars::JsonSchema` on config structs | Config field reference tables |
| `mcp.rs` | `schemars::JsonSchema` + const array | MCP parameter tables, tools list |
| `enums.rs` | `schemars::JsonSchema` on enums | Enum variant tables (UpdateMode, ExtractMode, Transform, etc.) |
| `features.rs` | const array | Feature flag tables (README, formats, contributing) |
| `makefile.rs` | `## target: description` Makefile comments | Make targets table |
| `store.rs` | const array | Search field boost table |

### Adding a new generated table

1. Add a render function in the appropriate xtask module (or create a new
   one and register it in `main.rs`)
2. Add `<!-- BEGIN/END GENERATED: marker-id -->` markers in the target doc
   file with the current hand-written content between them
3. Run `make generate-docs` -- the content is replaced with the generated
   version
4. Run again to verify idempotency ("up-to-date")

Prefer schema-derived tables (`schemars::JsonSchema` on the Rust type) over
const arrays in xtask. Schema-derived tables stay in sync automatically;
const arrays need manual updates when the source code changes.

See also: [Architecture](./architecture.md),
[Configuration](./configuration.md),
[CLI Reference](./cli.md), [AGENTS.md](../AGENTS.md) (code conventions).
