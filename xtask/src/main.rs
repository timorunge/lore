mod cli;
mod config;
mod enums;
mod features;
mod formats;
mod inject;
mod makefile;
mod mcp;
mod schema;
mod store;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Regenerate all doc tables from code annotations
    GenerateDocs,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::GenerateDocs => generate_docs(),
    }
}

fn generate_docs() -> Result<()> {
    let repo_root = repo_root();

    cli::inject_all(&repo_root)?;
    mcp::inject_all(&repo_root)?;
    config::inject_all(&repo_root)?;
    enums::inject_all(&repo_root)?;
    features::inject_all(&repo_root)?;
    formats::inject_all(&repo_root)?;
    makefile::inject_all(&repo_root)?;
    store::inject_all(&repo_root)?;

    println!("generate-docs: done");
    Ok(())
}

fn repo_root() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    PathBuf::from(manifest_dir)
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

use std::path::Path;
