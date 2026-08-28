//! CLI binary crate for the lore knowledge base.

pub mod cli;
pub mod pager;
// Most progress widgets drive the ingest pipeline; `maintain` uses only the
// step helpers, so the rest is legitimately dead without the `ingest` feature.
#[cfg_attr(not(feature = "ingest"), allow(dead_code))]
pub(crate) mod progress;
pub mod serve;
pub mod terminal;
