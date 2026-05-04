use std::path::Path;

use anyhow::{Context, Result, bail};

/// Replace content between matching `<!-- BEGIN GENERATED: {id} -->` /
/// `<!-- END GENERATED: {id} -->` marker pairs with the registered replacement.
pub fn inject(path: &Path, replacements: &[(&str, &str)]) -> Result<()> {
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let mut output = String::with_capacity(original.len() + 512);
    let mut remaining = original.as_str();
    let mut touched = 0usize;

    let begin_prefix = "<!-- BEGIN GENERATED: ";

    while !remaining.is_empty() {
        if let Some(begin_pos) = remaining.find(begin_prefix) {
            output.push_str(&remaining[..begin_pos]);
            let after_begin = &remaining[begin_pos..];

            let end_of_begin_line = after_begin
                .find('\n')
                .with_context(|| format!("unclosed BEGIN marker in {}", path.display()))?;
            let begin_line = &after_begin[..end_of_begin_line];
            let id = begin_line
                .strip_prefix(begin_prefix)
                .and_then(|s| s.strip_suffix(" -->"))
                .with_context(|| format!("malformed BEGIN marker: {begin_line}"))?
                .trim();

            let end_tag = format!("<!-- END GENERATED: {id} -->");
            let end_pos = after_begin
                .find(end_tag.as_str())
                .with_context(|| format!("no END marker for id={id:?} in {}", path.display()))?;

            let replacement = replacements
                .iter()
                .find(|(rid, _)| *rid == id)
                .map(|(_, c)| *c);

            if let Some(new_content) = replacement {
                // begin line + \n + new content + \n + end tag
                output.push_str(&after_begin[..end_of_begin_line + 1]);
                output.push_str(new_content);
                if !new_content.is_empty() && !new_content.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str(&end_tag);
                touched += 1;
            } else {
                // Marker belongs to a different module; preserve as-is.
                output.push_str(&after_begin[..end_pos + end_tag.len()]);
            }

            remaining = &after_begin[end_pos + end_tag.len()..];
        } else {
            output.push_str(remaining);
            break;
        }
    }

    for (id, _) in replacements {
        if !original.contains(&format!("<!-- BEGIN GENERATED: {id} -->")) {
            bail!("marker id={id:?} not found in {}", path.display());
        }
    }

    if touched > 0 && output != original {
        std::fs::write(path, &output)
            .with_context(|| format!("failed to write {}", path.display()))?;
        println!("  updated: {} ({touched} markers)", path.display());
    } else if touched > 0 {
        println!("  up-to-date: {}", path.display());
    }
    Ok(())
}
