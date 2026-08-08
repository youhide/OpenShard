//! The shard binary: start the library and report how it ended.
//!
//! Everything that does anything is in [`openshard_server`]. What is left here
//! is a process: turn on logging, read a command line, run, and turn a failure
//! into an exit code.
//!
//! ```sh
//! cargo run -p openshard-server -- --seed regions:felucca,decorate:felucca,populate:felucca
//! ```

use std::process::ExitCode;

use clap::Parser;
use tracing::error;
use tracing_subscriber::EnvFilter;

/// A shard, and the world it was asked to lay before it starts ticking.
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Admin verbs to send at boot, as if a game master had pressed the buttons:
    /// `--seed regions:felucca,decorate:felucca,populate:felucca`.
    ///
    /// What a verb means is the script pack's to decide — the engine ships no
    /// spawn or decoration data — so the names here are the pack's, not a list
    /// this binary can check. Repeat the flag or comma-separate; either way the
    /// verbs are sent in the order given, which is the order that matters:
    /// regions before what stands in them.
    ///
    /// Sent every run it is passed, with no look at whether the world is already
    /// laid. Seeding a shard that persists to a database twice lays everything
    /// twice.
    #[arg(long, env = "OPENSHARD_SEED", value_delimiter = ',', value_name = "VERB")]
    seed: Vec<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let cli = Cli::parse();

    match openshard_server::run(&cli.seed).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Printed rather than returned as a `Result`: `main` returning `Err`
            // renders it with `Debug`, which for a config error is a wall of
            // struct fields instead of the sentence that says what to fix.
            error!("{error}");
            ExitCode::FAILURE
        }
    }
}
