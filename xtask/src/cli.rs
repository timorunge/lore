use std::path::Path;

use anyhow::Result;
use clap::CommandFactory;

use crate::inject;

/// Regenerate CLI flag tables in documentation files from clap metadata.
pub fn inject_all(repo_root: &Path) -> Result<()> {
    println!("cli:");

    let cli_md = repo_root.join("docs/cli.md");
    let mut replacements: Vec<(String, String)> = Vec::new();

    let app = lore_cli::cli::args::Cli::command();

    for subcmd in app.get_subcommands() {
        if subcmd.is_hide_set() {
            continue;
        }
        let name = subcmd.get_name();
        let table = render_flags_table(subcmd);
        if !table.is_empty() {
            replacements.push((format!("flags-{name}"), table));
        }
        for nested in subcmd.get_subcommands() {
            let nested_name = nested.get_name();
            if nested_name == "help" {
                continue;
            }
            let table = render_flags_table(nested);
            if !table.is_empty() {
                replacements.push((format!("flags-{name}-{nested_name}"), table));
            }
        }
    }

    // Global flags (--config)
    replacements.push(("flags-global".to_owned(), render_global_flags(&app)));

    let refs: Vec<(&str, &str)> = replacements
        .iter()
        .map(|(id, content)| (id.as_str(), content.as_str()))
        .collect();
    inject::inject(&cli_md, &refs)?;

    // SKILL.md command summary
    let skill_md = repo_root.join("skills/lore/SKILL.md");
    let commands_table = build_command_summary_table(&app);
    inject::inject(&skill_md, &[("cli-commands", commands_table.as_str())])?;

    // HTTP serve flags subset for mcp-integration.md
    let mcp_md = repo_root.join("docs/mcp-integration.md");
    let serve_cmd = app.find_subcommand("serve").expect("serve subcommand");
    let http_flags_table = render_filtered_flags(serve_cmd, &["host", "port", "expose"]);
    inject::inject(&mcp_md, &[("flags-serve-http", &http_flags_table)])?;

    Ok(())
}

fn render_flags_table(subcmd: &clap::Command) -> String {
    let flags: Vec<_> = subcmd
        .get_arguments()
        .filter(|a| a.get_long().is_some() && !a.is_hide_set())
        .collect();

    if flags.is_empty() {
        return String::new();
    }

    let has_defaults = flags.iter().any(|a| !a.get_default_values().is_empty());

    if has_defaults {
        render_3col(&flags)
    } else {
        render_2col(&flags)
    }
}

fn render_3col(flags: &[&clap::Arg]) -> String {
    let mut out = String::new();
    out.push_str("| Flag | Default | Description |\n");
    out.push_str("|------|---------|-------------|\n");
    for arg in flags {
        let flag = format_flag(arg);
        let default = arg
            .get_default_values()
            .first()
            .map(|v| format!("`{}`", v.to_string_lossy()))
            .unwrap_or_default();
        let desc = arg.get_help().map(|h| h.to_string()).unwrap_or_default();
        out.push_str(&format!("| `{flag}` | {default} | {desc} |\n"));
    }
    out
}

fn render_2col(flags: &[&clap::Arg]) -> String {
    let mut out = String::new();
    out.push_str("| Flag | Description |\n");
    out.push_str("|------|-------------|\n");
    for arg in flags {
        let flag = format_flag(arg);
        let desc = arg.get_help().map(|h| h.to_string()).unwrap_or_default();
        out.push_str(&format!("| `{flag}` | {desc} |\n"));
    }
    out
}

fn render_global_flags(app: &clap::Command) -> String {
    let flags: Vec<_> = app
        .get_arguments()
        .filter(|a| a.get_long().is_some() && !a.is_hide_set() && a.is_global_set())
        .collect();

    render_2col(&flags)
}

fn render_filtered_flags(subcmd: &clap::Command, include: &[&str]) -> String {
    let flags: Vec<_> = subcmd
        .get_arguments()
        .filter(|a| a.get_long().is_some_and(|long| include.contains(&long)))
        .collect();

    if flags.is_empty() {
        return String::new();
    }

    render_3col(&flags)
}

fn format_flag(arg: &clap::Arg) -> String {
    let long = arg.get_long().map(|l| format!("--{l}")).unwrap_or_default();
    if let Some(c) = arg.get_short() {
        format!("-{c}, {long}")
    } else {
        long
    }
}

fn build_command_summary_table(app: &clap::Command) -> String {
    let mut out = String::new();
    out.push_str("| Command | Description |\n");
    out.push_str("|---------|-------------|\n");

    for subcmd in app.get_subcommands() {
        if subcmd.is_hide_set() {
            continue;
        }
        let name = subcmd.get_name();

        let cmd_display = match name {
            "search" => "`lore search <query>`".to_owned(),
            "read" => "`lore read <source>`".to_owned(),
            "preview" => "`lore preview [paths]`".to_owned(),
            "completions" => "`lore completions <shell>`".to_owned(),
            _ => format!("`lore {name}`"),
        };

        let mut about = subcmd
            .get_about()
            .map(|s| s.to_string())
            .unwrap_or_default();

        if name == "enrich" && !about.contains("--features") {
            about.push_str(" (requires `--features llm`)");
        }

        out.push_str(&format!("| {cmd_display} | {about} |\n"));

        for nested in subcmd.get_subcommands() {
            let nested_name = nested.get_name();
            if nested_name == "help" {
                continue;
            }
            let nested_display = format!("`lore {name} {nested_name}`");
            let nested_about = nested
                .get_about()
                .map(|s| s.to_string())
                .unwrap_or_default();
            out.push_str(&format!("| {nested_display} | {nested_about} |\n"));
        }
    }
    out
}
