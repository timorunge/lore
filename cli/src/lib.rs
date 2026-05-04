//! CLI binary crate for the lore knowledge base.

pub mod cli;
pub mod pager;
#[cfg(feature = "ingest")]
pub(crate) mod progress;
pub mod serve;
pub mod terminal;
