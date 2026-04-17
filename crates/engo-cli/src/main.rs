use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;

#[derive(Debug, Parser)]
#[command(
    name = "engo",
    about = "Local-first, AI-assisted i18n CLI.",
    version,
    author
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create an engo.toml in the current directory (interactive by default).
    Init(commands::init::Args),
    /// List or apply AI translations for pending units.
    Translate(commands::translate::Args),
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("engo=info,warn")),
        )
        .with_target(false)
        .without_time()
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Init(args) => commands::init::run(args),
        Command::Translate(args) => commands::translate::run(args).await,
    }
}
