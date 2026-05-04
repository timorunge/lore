use std::path::Path;

use anyhow::{Context, Result};

use crate::inject;

pub fn inject_all(repo_root: &Path) -> Result<()> {
    println!("makefile:");

    let makefile =
        std::fs::read_to_string(repo_root.join("Makefile")).context("failed to read Makefile")?;

    let targets = parse_targets(&makefile);

    inject::inject(
        &repo_root.join("docs/contributing.md"),
        &[("make-targets", &render_table(&targets))],
    )?;

    inject::inject(
        &repo_root.join("AGENTS.md"),
        &[("make-targets", &render_list(&targets))],
    )?;

    Ok(())
}

struct Target {
    name: String,
    description: String,
}

fn parse_targets(makefile: &str) -> Vec<Target> {
    makefile
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("## ")?;
            let (target, desc) = rest.split_once(": ")?;
            Some(Target {
                name: target.to_owned(),
                description: desc.to_owned(),
            })
        })
        .collect()
}

fn render_table(targets: &[Target]) -> String {
    let mut out = String::new();
    out.push_str("| Target | What it does |\n");
    out.push_str("|--------|--------------|\n");
    for t in targets {
        out.push_str(&format!("| `make {}` | {} |\n", t.name, t.description));
    }
    out
}

fn render_list(targets: &[Target]) -> String {
    let mut out = String::new();
    for t in targets {
        out.push_str(&format!("- `make {}` -- {}\n", t.name, t.description));
    }
    out
}
