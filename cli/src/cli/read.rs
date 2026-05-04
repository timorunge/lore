use anyhow::Result;

use lore::output::OutputMode;
use lore::query::Pagination;
use lore::store::StoreSet;

/// Options for the `read` entry point.
pub struct ReadOptions<'a> {
    pub stores: StoreSet,
    pub source: String,
    pub pagination: Pagination,
    pub full: bool,
    pub mode: OutputMode,
    pub json: bool,
    pub pager: Option<&'a str>,
    pub no_pager: bool,
}

/// Read a single document from the store and display it.
pub fn read(opts: ReadOptions<'_>) -> Result<()> {
    let output = lore::query::get_document(
        &opts.stores,
        lore::query::ReadArgs {
            source: opts.source,
            pagination: opts.pagination,
            full: opts.full,
        },
        opts.mode,
    )?;
    if opts.json {
        println!("{output}");
    } else {
        let capped = lore::fmt::cap_output(&output, crate::terminal::output_width());
        let pager_cmd = crate::pager::resolve_pager(opts.pager, opts.no_pager);
        crate::pager::page_output(&capped, pager_cmd.as_deref())?;
    }
    Ok(())
}
