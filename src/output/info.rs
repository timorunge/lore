use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::Result;

use crate::fmt::table::format_kv;
use crate::fmt::{format_bytes, format_count, plural, to_json_pretty};
use crate::output::{
    OutputMode, format_formats, format_kinds, format_lang_summary, format_source_types,
    format_topic_table,
};
use crate::store::{StoreInfo, TopicStat};

const TOPIC_DISPLAY_LIMIT: usize = 10;

/// Format store statistics as CLI text, JSON, or MCP text.
///
/// # Errors
///
/// Returns an error if JSON serialization fails (only possible in `Json` output mode).
pub fn format_store_info(
    info: &StoreInfo,
    store_entries: &[(PathBuf, u64)],
    mode: OutputMode,
) -> Result<String> {
    if mode == OutputMode::Json {
        return to_json_pretty(info);
    }

    anyhow::ensure!(!store_entries.is_empty(), "no store entries");

    let mut out = String::new();

    if let Some(ref name) = info.name {
        out.push_str(name);
        out.push('\n');
        if let Some(ref desc) = info.description {
            out.push_str(desc);
            out.push('\n');
        }
        out.push('\n');
    }

    let multi = store_entries.len() > 1;
    let total_size: u64 = store_entries.iter().map(|(_, s)| s).sum();

    let mut kv: Vec<(&str, String)> = Vec::new();

    if multi {
        let stores_val: Vec<String> = store_entries
            .iter()
            .map(|(p, s)| format!("{} ({})", p.display(), format_bytes(*s)))
            .collect();
        kv.push(("Stores", stores_val.join(", ")));
        kv.push(("Total size", format_bytes(total_size)));
    } else {
        kv.push(("Store", store_entries[0].0.display().to_string()));
        kv.push(("Size", format_bytes(store_entries[0].1)));
    }

    if !info.kinds.is_empty() {
        kv.push(("Kinds", format_kinds(&info.kinds)));
    }
    if !info.formats.is_empty() {
        kv.push(("Formats", format_formats(&info.formats)));
    }
    if info.source_types.len() > 1 {
        kv.push(("Sources", format_source_types(&info.source_types)));
    }
    if let Some(summary) = format_lang_summary(&info.languages) {
        kv.push(("Languages", summary));
    }

    kv.push(("Documents", info.documents.to_string()));
    kv.push(("Chunks", info.chunks.to_string()));
    kv.push(("Words", format_count(info.words)));
    if info.documents > 0 {
        let avg_chunks = (info.chunks as f64 / info.documents as f64).round() as usize;
        let avg_words = (info.words as f64 / info.documents as f64).round() as usize;
        let mut avg_val = format!(
            "{avg_chunks} chunk{}, {avg_words} word{}",
            plural(avg_chunks),
            plural(avg_words)
        );
        if info.chunks > 0 {
            let avg_wpc = info.words as f64 / info.chunks as f64;
            w!(avg_val, " ({avg_wpc:.0} words/chunk)");
        }
        kv.push(("Avg/doc", avg_val));
    }

    if let Some(ref ts) = info.created_at {
        kv.push(("Created", ts.clone()));
    }
    if let Some(ref ts) = info.updated_at {
        kv.push(("Updated", ts.clone()));
    }
    if let Some(ref m) = info.last_mode {
        kv.push(("Last mode", m.clone()));
    }

    match info.phrase_search.as_deref() {
        Some("true") => kv.push(("Phrase search", "enabled".to_owned())),
        Some("false") => kv.push(("Phrase search", "disabled".to_owned())),
        _ => {}
    }

    if info.segments > 1 {
        kv.push((
            "Segments",
            format!("{} (run `lore maintain compact`)", info.segments),
        ));
    }

    if let Some(ref v) = info.lore_version {
        kv.push(("Version", format!("lore v{v}")));
    }

    let pairs: Vec<(&str, &str)> = kv.iter().map(|(l, v)| (*l, v.as_str())).collect();
    out.push_str(&format_kv(&pairs, "", mode.width()));

    if !info.topics.is_empty() {
        let mut sorted: Vec<TopicStat> = info.topics.clone();
        sorted.sort_by_key(|t| std::cmp::Reverse(t.chunk_count));
        let page = &sorted[..TOPIC_DISPLAY_LIMIT.min(sorted.len())];
        w!(
            out,
            "\n{} topic{}",
            info.topics.len(),
            plural(info.topics.len())
        );
        if info.topics.len() > TOPIC_DISPLAY_LIMIT {
            w!(
                out,
                ", 1-{}/{}. Use `lore topics` to list all.",
                TOPIC_DISPLAY_LIMIT,
                info.topics.len()
            );
        }
        out.push_str("\n\n");
        let table = format_topic_table(page, mode);
        out.push_str(table.trim_end_matches('\n'));
    }

    Ok(out.trim_end().to_owned())
}
