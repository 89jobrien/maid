//! Parses `maid` commands and dispatches file organization, previews, undo, and completions.

use clap::{CommandFactory, Parser, Subcommand};
use std::path::PathBuf;

mod config;
mod error;
mod organiser;

use config::Config;
use error::MaidError;

#[derive(Parser)]
#[command(name = "maid", about = "A clean file organiser", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Organise files in a directory
    Run {
        /// Target directory (defaults to configured directories)
        path: Option<PathBuf>,
    },
    /// Preview what would happen without moving files
    Preview {
        /// Target directory (defaults to configured directories)
        path: Option<PathBuf>,
    },
    /// Undo the last organisation
    Undo {
        /// Target directory (defaults to configured directories)
        path: Option<PathBuf>,
    },
    /// Generate a Nushell completion script
    Completions,
}

/// Pairs each target directory with the config that applies to it. An explicit
/// path loads the nearest `maid.toml` at or above it; with no path, each
/// configured directory does the same.
fn targets(path: Option<PathBuf>) -> Result<Vec<(PathBuf, Config)>, MaidError> {
    let dirs = match path {
        Some(p) => vec![p],
        None => Config::load()?.directories,
    };
    dirs.into_iter()
        .map(|dir| {
            let config = Config::load_for(&dir)?;
            Ok((dir, config))
        })
        .collect()
}

fn main() -> Result<(), MaidError> {
    let cli = Cli::parse();

    // Completion generation must not depend on a valid config, so it runs before loading one.
    if let Command::Completions = cli.command {
        clap_complete::generate(
            clap_complete_nushell::Nushell,
            &mut Cli::command(),
            "maid",
            &mut std::io::stdout(),
        );
        return Ok(());
    }

    match cli.command {
        Command::Run { path } => {
            for (dir, config) in targets(path)? {
                println!("==> {}", dir.display());
                let entries = organiser::scan(&dir, &config)?;
                organiser::organise(&dir, &entries, &config)?;
            }
        }
        Command::Preview { path } => {
            for (dir, config) in targets(path)? {
                println!("==> {}", dir.display());
                let entries = organiser::scan(&dir, &config)?;
                organiser::preview(&entries, &dir, &config)?;
            }
        }
        Command::Undo { path } => {
            for (dir, _) in targets(path)? {
                println!("==> {}", dir.display());
                organiser::undo(&dir)?;
            }
        }
        Command::Completions => {}
    }

    Ok(())
}
