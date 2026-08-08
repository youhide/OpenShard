//! A shard and a window, in one process:
//!
//! ```sh
//! cargo run -p openshard-playground -- --client "/path/to/Ultima Online Classic"
//! ```
//!
//! Every option is also an environment variable, so the install can be named
//! once — exported, or written into a `.env` beside the workspace root, which
//! this binary reads before it parses anything. `--help` lists both spellings.
//!
//! One command instead of two, and no network at all: **no port is bound and no
//! socket is opened**. The client dials the shard through
//! [`openshard_e2e_shard::in_process`], which is a pair of in-memory pipes, and
//! closing the window ends both ends.
//!
//! # What that does and does not remove
//!
//! Not the protocol. Both ends run exactly the code they run against ClassicUO
//! — the client's framing and login machine, the relay's second connection,
//! per-write compression, and the gateway's own `client_session_serve`. The
//! transport is a type parameter on either side (`Dial` for the client, any
//! stream for the gateway) and everything above it is untouched, which is the
//! only arrangement where this is worth having: a second implementation that
//! agreed with the first would be the thing that goes quietly out of step.
//!
//! What is gone is the kernel — segment boundaries, resets, and anything about
//! timing that a real network decides. The socket tests in `crates/e2e/shard`
//! cover that and stay where they are.
//!
//! # Whose world it is
//!
//! The shard reads `openshard.toml` if there is one — `--config` names another —
//! and falls back to the shipped default when there is not. That is the
//! difference between a window onto a world and a window onto bare ground: the
//! engine ships no spawn or decoration data at all, so with the default's empty
//! `scripting.main` and no saved world, what draws is the map's own statics and
//! nothing else. No townsfolk, no doors, no shop signs — those are the pack's,
//! and they arrive over the wire like any other item.
//!
//! It follows that this *can* now open the world an operator has saved, and
//! write to it: a config naming a database is that database. Not a second copy.
//! Point it at one a `cargo run -p openshard-server` is already serving and two
//! processes have it open at once.
//!
//! # What this is not
//!
//! Not a shard to serve from: no port is bound, so nothing outside this process
//! can reach it. The `e2e` tests beside this crate are the same arrangement with
//! assertions instead of a window, and they take the shipped default on purpose
//! — a test that read the machine's config would pass on what somebody happened
//! to have configured.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

/// One process, both ends: a shard in a thread and a window logged in to it.
///
/// Each option carries the environment variable it also answers to. The
/// defaults are the stock development shard's own account and character —
/// `openshard_e2e_shard` ships them and this binary does not invent a second
/// set, because a name typed here that the config does not have fails at the
/// character list with nothing to say why.
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// The client install both ends read.
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,

    /// The account to log in as.
    #[arg(short, long, env = "OPENSHARD_ACCOUNT", default_value = openshard_e2e_shard::ACCOUNT)]
    account: String,

    /// The character to play.
    #[arg(long, env = "OPENSHARD_CHARACTER", default_value = openshard_e2e_shard::CHARACTER)]
    character: String,

    /// The shard config to run, when there is one at that path.
    ///
    /// The point of reading it is the pack and the database: a shard with
    /// `scripting.main` empty and no world saved anywhere draws the map and
    /// nothing else — no townsfolk, no doors, no shop signs, because the engine
    /// ships none of that. Missing, this falls back to the shipped default,
    /// which is what a fresh checkout has.
    #[arg(
        long,
        env = "OPENSHARD_CONFIG",
        default_value = "openshard.toml",
        value_name = "FILE"
    )]
    config: PathBuf,

    /// The `tracing` filter, in `RUST_LOG` syntax.
    #[arg(long, env = "RUST_LOG", default_value = "info", value_name = "FILTER")]
    log: String,

    /// Draw overhead speech through this TrueType or OpenType face instead of
    /// `fonts.mul`. See `openshard_client_app`'s own flag of the same name.
    #[arg(long, env = "OPENSHARD_TTF_FONT", value_name = "FILE")]
    ttf_font: Option<PathBuf>,
}

fn main() -> ExitCode {
    // Before the command line is parsed, because what the file holds is the
    // environment those `env =` options fall back to. Exporting the variables,
    // or typing the flags, is the same run.
    openshard_client_app::load_env();
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_new(&cli.log).unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let dir = cli.client;

    // Read before the window opens and before the shard thread starts, so a
    // config with a typo in it is a sentence on the terminal rather than a panic
    // inside a thread nobody is watching.
    let operator = match openshard_e2e_shard::operator_config(&cli.config) {
        Ok(operator) => operator,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    if operator.is_none() {
        eprintln!(
            "no {} beside this directory; running the shipped default, whose world is empty",
            cli.config.display()
        );
    }

    // The shard reads the same install the window does, and that is not a
    // convenience: `world.client_files` is what gives the server a map, and
    // without one every step is allowed at whatever height the client guessed.
    // The client predicts each step's `z` from its own copy of the facet, so two
    // ends reading different files disagree about the ground and the walk turns
    // into a stream of `0x21` rollbacks — which looks like a bug in the client.
    // It costs a second copy of the facet in this process; a playground can
    // afford one, and `docs/client_versions.md` is the standing rule it obeys.
    // Overridden whichever config this is, because the window is the one naming
    // the install and an operator's config may name none.
    let files = dir.to_string_lossy().into_owned();
    let (dial, shard) = openshard_e2e_shard::in_process::spawn(move |stated| {
        let mut config = operator.unwrap_or_else(|| openshard_e2e_shard::stock_config(stated));
        // Both addresses, whichever config this is: nothing binds a port here,
        // but the `0x8C` relay still tells the client where to dial and the
        // client still obeys it. An operator's `advertise` — a LAN address, a
        // public one — would send this window somewhere that is not this
        // process, and the login would end politely and silently.
        config.server.listen = stated;
        config.server.advertise = stated;
        config.world.client_files = files;
        config
    });
    eprintln!(
        "shard up in this process; logging in as {} to play {}",
        cli.account, cli.character
    );

    // On this thread, because `winit` requires the event loop to own the one it
    // was built on. The shard is the one that moved.
    let plan = openshard_e2e_shard::plan_for(&cli.account, &cli.character);
    // Nothing to open on: the shard says where the character stands, and a
    // playground that looked somewhere else would be looking away from the
    // thing it just logged in to play. `--at` is the offline viewer's.
    let code = openshard_client_app::run(
        &dir,
        Some((dial, plan)),
        cli.ttf_font,
        openshard_client_app::Opening::default(),
    );

    // The window is gone, so the shard is asked to stop and waited for. The wait
    // is not a formality any more: with a config naming a database, the last
    // save happens on the way out, and returning before it would lose whatever
    // was played in this window. It is also the same path an operator's Ctrl-C
    // takes, so a stop that hangs or panics shows up here rather than only in
    // production.
    shard.stop();
    code
}
