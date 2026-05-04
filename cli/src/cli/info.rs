use std::path::PathBuf;

use anyhow::Result;

use lore::store::StoreSet;
use lore::util;

/// Gather statistics from a `StoreSet` and format them for display or JSON output.
pub fn info(stores: &StoreSet, json: bool) -> Result<String> {
    let store_info = stores.store_info();
    let store_entries: Vec<(PathBuf, u64)> = stores
        .iter()
        .map(|s| {
            let p = s.path().to_owned();
            let size = util::dir_size(&p);
            (p, size)
        })
        .collect();
    lore::output::format_store_info(
        &store_info,
        &store_entries,
        crate::terminal::output_mode(json),
    )
}
