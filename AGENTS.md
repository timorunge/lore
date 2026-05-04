# AGENTS.md

Code conventions and style rules for lore. This is the authoritative reference
for AI agents and contributors working on the codebase.

For dev setup, workflow, and recipes see
[docs/contributing.md](./docs/contributing.md).
For architecture see [docs/architecture.md](./docs/architecture.md).

## Build / Lint / Test Commands

<!-- BEGIN GENERATED: make-targets -->
- `make build` -- Build the binary (release, all features)
- `make install` -- Install the binary via cargo install
- `make update` -- Update all Cargo dependencies
- `make fmt` -- Check formatting (fails on diff)
- `make fmt-fix` -- Fix formatting (destructive)
- `make lint-conventions` -- Check code conventions (unwrap, import order, pub visibility)
- `make lint` -- Run clippy (all feature combos)
- `make test-quick` -- Run tests (default features only)
- `make test` -- Run tests (all feature combos)
- `make fuzz` -- Run all fuzz targets for 60 seconds each (requires cargo-fuzz and nightly)
- `make doc` -- Build docs (fails on warnings)
- `make generate-docs` -- Regenerate doc tables from code annotations
- `make check-docs` -- Verify doc tables are up to date
- `make check` -- Run all quality gates (fmt lint-conventions lint doc test check-docs deny)
- `make ci-local` -- Run CI workflow locally via act (ubuntu matrix only)
- `make setup` -- Install git hooks (run once after cloning)
- `make clean` -- Remove build artifacts
- `make help` -- Show this help
<!-- END GENERATED: make-targets -->
- Single test: `cargo test test_name`
- MCP Inspector: `npx @modelcontextprotocol/inspector lore serve`

Zero tolerance for clippy warnings (`-D warnings` treats them as errors).
Run `make check` before every commit.

## Code Style

### Imports

Three groups separated by blank lines:

1. `std::` / standard library
2. External crates (`anyhow`, `serde`, `tokio`, etc.)
3. `crate::` / internal modules

Use `crate::` for intra-crate references, not `super::`. Exception: `mod tests`
blocks may use `super::*`.

### File Organization

If a file needs section-divider banners, it is doing too much. Split into
sub-modules and let the module structure be the organization. Use `mod.rs`
re-exports to keep the public API clean.

One blank line between groups. No comment banners.

### Module Visibility

- `pub` -- public API (re-exports in `lib.rs`)
- `pub(crate)` -- shared across modules within the crate (default for
  cross-module items)
- `pub(super)` -- shared only within parent module
- Private (no qualifier) -- internal to the current module

Start private, widen only when needed. Most structs and functions are
`pub(crate)`.

### Re-exports

Consolidate in `mod.rs` using `pub use` and `pub(crate) use`. Callers import
from the module, not from sub-modules.

## Declaration Ordering

Per file, top to bottom:

1. Module-level doc comment (if needed)
2. `mod` declarations (sub-modules)
3. `use` imports (three groups)
4. Constants (`const`)
5. Type definitions (`struct`, `enum`, type aliases)
6. `impl` blocks (constructor first, then public methods, then private methods)
7. Standalone functions
8. `#[cfg(test)] mod tests { ... }` at the bottom

### CONV-STRUCT: Struct field ordering

Within a struct definition, order fields:

1. Required/identity fields first (`id`, `name`, `source`)
2. Configuration/behavioral fields next
3. Optional/metadata fields last (`Option<T>`)
4. Computed/derived fields at the end

### CONV-ENUM: Enum variant ordering

1. `#[default]` variant first
2. Remaining variants in order of frequency/commonality, or alphabetical
   when no natural ordering exists
3. `as_str()`, `Display`, and match arms follow declaration order

### CONV-IMPL: Impl block ordering

1. Constructor (`new`, `open`, `from_*`) first
2. Public methods next, in logical grouping
3. `pub(crate)` methods next
4. Private methods last
5. Trait impls each in their own `impl` block, after the inherent `impl`

## Naming

- **Modules:** short, lowercase, single-word (`store`, `ingest`, `config`,
  `render`, `cache`)
- **Types:** PascalCase (`Chunk`, `SearchHit`, `IngestConfig`, `OutputMode`)
- **Functions/methods:** snake_case (`add_chunk`, `format_search_results`)
- **Constants:** SCREAMING_SNAKE_CASE (`MAX_PAGE_LIMIT`, `INTERNAL_ERROR`)
- **Enums:** PascalCase variants (`SourceType::Local`, `DocKind::Document`)
- **Variables:** short for short scope (`e`, `s`, `i`), descriptive for long
  (`chunk_count`, `store_path`)
- **Feature flags:** lowercase, hyphenated in Cargo.toml (`tree-sitter`,
  `llm`, `s3`). Some features are Cargo pass-throughs with no source-level
  `#[cfg]` gates (e.g. `tree-sitter`, `ocr`)

## Parameter Ordering

1. Primary data -- the main subject (`store`, `chunks`, `doc`)
2. Arguments/query -- what to do with it (`args`, `query`, `params`)
3. Configuration -- mode, options, flags (`mode: OutputMode`, `limit`, `offset`)
4. Output destination -- writer, formatter (rare in lore)

For search/filter functions: data before filter criteria.

## Error Handling

Primarily `anyhow`, with one exception (`LoreError` in `src/error.rs`
for typed library errors):

- `anyhow::Result` for all fallible functions
- `anyhow::bail!` for early-return errors
- `anyhow::ensure!` for precondition checks
- `.context("message")` or `.with_context(|| format!(...))` for wrapping
- Guard clause pattern: return early, avoid nesting
- Never ignore errors -- no `let _ = fallible_call()`
- MCP tool handlers: log with `tracing::error!` and return `INTERNAL_ERROR`
  string constant
- MCP resource handlers: return `ErrorData::internal_error` for internal
  failures (logged with `tracing::error!`), `ErrorData::resource_not_found`
  for unknown URIs

### unwrap and unsafe policy

- `unwrap()` is permitted only in tests and in `const`/`static` initializers
  where panic is impossible at runtime. In all other production code, use `?`
  or `expect("reason")` with a clear message explaining the invariant.
- `unsafe` requires a `// SAFETY:` comment immediately above the block
  explaining the invariant that makes the usage sound.

## Types and Patterns

- `Option<String>` for optional fields, not empty string sentinels
- `#[serde(skip_serializing_if = "Option::is_none")]` on every optional field
- `#[serde(rename_all = "lowercase")]` on enums with string representations
- `#[serde(deny_unknown_fields)]` on config structs
- `#[derive(Default)]` with `#[default]` attribute on the common variant
- Shared domain types live in `src/types.rs`
- Config types live in `src/config/`
- YAML deserialization uses `serde_yaml_ng` (not `serde_yaml` or `serde_yml`)

## Comment Discipline

ASCII only in all comments (doc comments and inline). Use `--` for em-dashes,
`...` for ellipses, plain quotes, and `-` or `->` for arrows. Non-ASCII is
permitted in string literals and test data, not in comments or documentation.

### Doc comments (`///`)

- Exported symbols: one sentence describing purpose. Multi-sentence only when
  behavior is non-obvious.
- No `# Parameters`, `# Returns`, `# Errors` sections unless genuinely needed
  for complex signatures.
- Unexported functions: one-line summary. Skip only if the name is truly
  self-describing.

### Inline comments (`//`)

- Never narrate the next line of code.
- Never restate a condition.
- Do comment: non-obvious algorithms, performance trade-offs, safety rationale
  for `unsafe`, workarounds with references, "why not the obvious approach".

### Struct fields

- Skip comments on self-describing fields.
- Comment fields where name alone does not convey valid values, units, or
  invariants.

### Test code

- Test function doc comments almost never needed -- the test name documents
  intent.
- No section-divider banners in test code.

## Testing

Two layers:

- **Unit tests** -- `#[cfg(test)] mod tests { ... }` at bottom of source file
- **Integration tests** -- `cli/tests/integration/`, using `assert_cmd`
  and `predicates`

Rules:

- Fixture builders and helpers go at the top of the `mod tests` block, before
  test functions
- Test functions follow the source file's declaration order where practical
- Table-driven tests for formatters, path functions, and similar repetitive
  cases
- No redundant tests -- two tests hitting the same branch should be collapsed
- No tautological tests -- if it can't fail without a compile error, delete it
- `#[allow(dead_code)]` on test helpers only if they serve multiple test
  functions

## Async

- `tokio` runtime throughout
- `tokio::task::spawn_blocking` for CPU-bound work (chunking)
- `tokio::task::block_in_place` for synchronous calls within async context
  (Tantivy commits)
- `tokio::join!` for concurrent independent async operations

## Ingest Pipeline

Four conventions every loader must follow:

### CONV-STREAM: Use Files for file-based sources, Preloaded otherwise

The three-stage pipeline (load -> process -> write) uses bounded `mpsc`
channels for backpressure. Two `SourceItems` variants handle all sources:

**`SourceItems::Files`** -- for sources that produce file paths on disk
(local directories, git checkouts). The pipeline calls
`load_file_or_archive` per path via `buffer_unordered` with bounded
channel backpressure. Use the `source_prefix` and `stamp` fields for
sources that need custom source-key formatting or metadata overrides
(e.g. git uses `source_prefix` for `{url}#{path}` keys and `stamp` for
the HEAD commit etag).

**`SourceItems::Preloaded`** -- for sources where content is loaded
during discovery (maildir, exec, MCP, S3, YouTube, single-file local,
feed). The loader runs `buffer_unordered` internally and returns all
documents in a Vec. The streaming pipeline sends them through the
bounded channel one by one.

Do not add custom `SourceItems` variants for individual source types.
Custom variants that call `suppress_native_stderr()` inside `load_task`
conflict with `indicatif` progress bars (fd 2 redirect makes bar
redraws go to `/dev/null`, corrupting terminal line tracking).

### CONV-FAIL: Propagate structured failures

Every loader that can silently skip documents (parse errors, extraction
panics, timeouts) must collect failures and return them alongside results.
Use `(Vec<LoaderResult>, Vec<FailedDoc>)` return types. For concurrent
streams (`buffer_unordered`), use `Mutex<Vec<FailedDoc>>` to collect
failures across tasks. Wire the list through
`DiscoveredSource.load_failures` so it reaches `IngestResult.failed_docs`
and the CLI displays each failure with its source and reason.

### CONV-ISOLATE: Isolate external code panics

Calls to external libraries that may panic (kreuzberg, mail-parser,
html-to-markdown) must be wrapped in `tokio::task::spawn` to isolate panics
as `JoinError`. Use `suppress_native_stderr()` when the external code may
write to fd 2 (e.g. native C libraries behind kreuzberg). Pattern:

```rust
let task = tokio::task::spawn(async move {
    let guard = suppress_native_stderr();
    let result = tokio::time::timeout(timeout, external_call()).await;
    drop(guard);
    result
});
match task.await {
    Ok(Ok(Ok(result))) => { /* success */ }
    Ok(Ok(Err(e))) => { /* extraction error */ }
    Ok(Err(_)) => { /* timeout */ }
    Err(e) => { /* panic from external code */ }
}
```

**Important:** `suppress_native_stderr()` redirects fd 2 globally via
`dup2`. This conflicts with `indicatif` progress bars which also write
to fd 2. Only use it in code that runs during discovery (before progress
bars are active) or inside `Preloaded` loaders. Never use it inside
`load_task` or any code that runs concurrently with active progress
bars -- the fd 2 redirect causes indicatif to lose track of terminal
lines, corrupting the display.

### CONV-PROGRESS: Drive the progress handle

The CLI shows per-source progress for the fetch, enrich, and index
phases. The loading/discovery phase is intentionally suppressed
(`ProgressHandle::noop()`) to reduce visual noise -- discovery is
typically fast relative to the other phases.

Loaders that accept a `&ProgressHandle` should still drive it (the
handle may be a noop or a real bar depending on context):

1. Call `progress.inc_length(n)` as soon as the total work is known
   (number of entries, commands, resources, etc.).
2. Call `progress.inc(1)` after each unit of work completes.
3. `ProgressHandle::noop()` is free -- use it in tests and internal calls
   where no UI is needed. Never skip the parameter to avoid threading it.

## Feature Gating

- `#[cfg(feature = "x")]` on `mod` declarations, `use` statements, and
  struct fields as appropriate (see `llm` and `s3` for examples)
- `#[cfg(unix)]` / `#[cfg(not(unix))]` pairs for platform-specific code
- Gate optional dependencies, not core logic

## Git Conventions

Conventional Commits: `<type>: <subject>`

Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`

Rules: imperative present tense, no capital first letter, no period, max 72
chars. Use commit body for context when the change is non-obvious.

## Markdown Documentation

- ASCII only -- no emojis or special Unicode
- Use `--` for em-dashes
- Wrap prose at ~80 characters; code blocks and tables exempt
- Separate commands from output in distinct fenced code blocks
- Command blocks use `bash` language tag, no `$` prefix

## See Also

- [docs/contributing.md](./docs/contributing.md) -- dev setup, workflow, recipes
- [docs/architecture.md](./docs/architecture.md) -- module structure, pipeline
  stages
- [docs/configuration.md](./docs/configuration.md) -- config file reference
