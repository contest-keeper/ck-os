//! `ck-agent`: the program on the contest stick.
//!
//! For now it has one command, `apply-config`, which `ck-config.service` runs at every boot.
//! Log lines go to stderr, where the journal collects them.

use std::path::PathBuf;
use std::process::ExitCode;

use ck_agent_config::{Error, Root, apply_basics, load};
use clap::{Parser, Subcommand};

/// The settings file is missing, so the image keeps its defaults. The service treats this
/// differently from a real failure.
const EXIT_NO_SETTINGS: u8 = 3;

#[derive(Parser)]
#[command(
    name = "ck-agent",
    version,
    about = "The Contest Keeper agent on the stick"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read the admin's settings from the EFI partition and apply them to this machine
    ApplyConfig {
        /// The folder that stands for `/` (only for tests; the stick uses `/`)
        #[arg(long, default_value = "/")]
        root: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .without_time()
        .with_ansi(false)
        .init();

    match cli.command {
        Command::ApplyConfig { root } => apply_config(&Root::new(root)),
    }
}

fn apply_config(root: &Root) -> ExitCode {
    let loaded = match load(root) {
        Ok(loaded) => loaded,
        Err(Error::SettingsMissing(path)) => {
            tracing::error!("no settings at {path}; keeping the image defaults");
            return ExitCode::from(EXIT_NO_SETTINGS);
        }
        Err(error) => {
            tracing::error!("cannot use the settings, keeping the image defaults: {error}");
            return ExitCode::FAILURE;
        }
    };
    // The stick id is not secret; the token and the file content are never logged.
    tracing::info!(stick = %loaded.config.stick_id, sha256 = %loaded.sha256, "settings read");

    if let Err(error) = apply_basics(root, &loaded) {
        tracing::error!("applying the settings failed: {error}");
        return ExitCode::FAILURE;
    }
    tracing::info!("settings applied");
    ExitCode::SUCCESS
}
