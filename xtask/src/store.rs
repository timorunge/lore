use std::path::Path;

use anyhow::Result;

use crate::inject;

pub fn inject_all(repo_root: &Path) -> Result<()> {
    println!("store:");

    inject::inject(
        &repo_root.join("docs/architecture.md"),
        &[("schema-search-fields", &render_search_fields())],
    )?;

    Ok(())
}

fn render_search_fields() -> String {
    let mut out = String::new();
    out.push_str("| Field | Boost | Purpose |\n");
    out.push_str("|-------|-------|---------|\n");

    for f in lore::store::SEARCH_FIELDS {
        out.push_str(&format!(
            "| `{}` | {:.1}x | {} |\n",
            f.name, f.boost, f.purpose
        ));
    }

    out
}
