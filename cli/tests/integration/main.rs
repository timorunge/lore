// Suites that build their fixture store via the `lore ingest` command need the
// `ingest` feature: without it the subcommand does not exist and every test in
// them fails with "unrecognized subcommand 'ingest'". They were previously
// unguarded but never ran featureless, because `--workspace --no-default-features`
// unified features with xtask and silently enabled `ingest` anyway.
#[cfg(feature = "test-support")]
mod feed;
#[cfg(feature = "test-support")]
mod git;
// Most helpers build a store via `lore ingest`, so they are unused when the
// suites that call them are compiled out.
#[cfg_attr(not(feature = "ingest"), allow(dead_code))]
mod helpers;
#[cfg(feature = "ingest")]
mod ingest;
#[cfg(feature = "ingest")]
mod loaders;
#[cfg(feature = "ingest")]
mod maintain;
#[cfg(feature = "ingest")]
mod multi_store;
#[cfg(feature = "ingest")]
mod query;
#[cfg(feature = "test-support")]
mod sitemap;
mod smoke;
#[cfg(feature = "test-support")]
mod url;
