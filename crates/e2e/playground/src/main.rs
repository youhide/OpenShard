//! A shard and a window, in one process:
//!
//! ```sh
//! cargo run -p openshard-playground -- --client "/path/to/Ultima Online Classic"
//! ```
//!
//! To exercise client mailbox backpressure with ordinary moving-mobile traffic,
//! add `--mailbox-load --stall-app-ms 5000`. This is opt-in and replaces the
//! playground's script only for that process; it does not alter a configured
//! world's database.
//!
//! To reproduce static-atlas exhaustion, add `--atlas-scroll`. It has the
//! in-process shard drive the logged-in player around an expanding square, so
//! the normal player-follow camera crosses fresh map tiles without hand input.
//! Leave it running until the jank log reports
//! `atlas_overflowed=Some("statics")`.
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
//! difference between a window onto a world and a window onto bare ground: an
//! unseeded shard with no saved world draws the map's own statics and nothing
//! else. No townsfolk, no doors, no shop signs — those are laid by the admin
//! verbs, which this binary takes too
//! (`--seed populate:felucca,decorate:felucca`), and they arrive over the wire
//! like any other item. A world with a database behind it needs no seed: it
//! restores what it holds. It also lays nothing *new*, so content that has grown
//! since it was populated — a fixed dataset, a region the engine used to drop —
//! arrives on a seed or on the staff menu's Populate, and never on a restart.
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
use std::time::Duration;

use clap::Parser;
use clap::ValueEnum;
use tracing_subscriber::EnvFilter;

/// A fresh trace from the last playground run. `target/` keeps the diagnostic
/// out of source control while leaving it beside the command that produced it.
const JANK_LOG: &str = "target/openshard-playground-jank.log";

/// Reproducible presentation scenarios the playground can hold while writing
/// its jank trace. Keeping this local makes the integration runner's command
/// line independent of the standalone client's diagnostic CLI.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Scenario {
    /// Open the live craft catalogue as soon as the window is ready. Intended
    /// for deterministic egui captures, not for gameplay automation.
    CraftCatalogue,
    /// Zoom out from the default view, then hold still for LOD profiling.
    ZoomSoak,
    /// Zoom out and then pan across map blocks, without desktop input.
    ///
    /// The moving companion to `ZoomSoak`, and the one a frame-rate claim
    /// needs: a standing camera lets every geometry cache in the client hit,
    /// so it measures the frame a player sees only while they are not
    /// playing.
    LodSweep,
}

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
    /// The point of reading it is the seed and the database: a shard that was
    /// never seeded and has no world saved anywhere draws the map and nothing
    /// else — no townsfolk, no doors, no shop signs, because nothing has laid
    /// them yet. Missing, this falls back to the shipped default,
    /// which is what a fresh checkout has.
    #[arg(
        long,
        env = "OPENSHARD_CONFIG",
        default_value = "openshard.toml",
        value_name = "FILE"
    )]
    config: PathBuf,

    /// Admin verbs to lay before the first tick, comma-separated:
    /// `--seed populate:felucca,decorate:felucca`. The server binary's flag, and
    /// the only way a world that has never been populated gets its spawn regions,
    /// its townsfolk and its doors — a database restores what it holds and lays
    /// nothing new, so a content fix that adds regions needs this (or the staff
    /// menu's Populate) once, not a restart.
    ///
    /// Laying twice is safe: every verb is idempotent, an already-standing region
    /// keeps its timer, and a townsperson is not placed on top of itself.
    #[arg(long, env = "OPENSHARD_SEED", value_delimiter = ',', value_name = "VERBS")]
    seed: Vec<String>,

    /// The `tracing` filter, in `RUST_LOG` syntax.
    #[arg(long, env = "RUST_LOG", default_value = "info", value_name = "FILTER")]
    log: String,

    /// Write a frame-jank trace to `target/openshard-playground-jank.log`.
    ///
    /// This is deliberately opt-in: detailed diagnostics and their I/O can
    /// distort the responsiveness of the in-process playground they measure.
    #[arg(long, env = "OPENSHARD_JANK_LOG")]
    jank_log: bool,

    /// Pause the App event loop once immediately after it enters the world.
    ///
    /// A diagnostic only: the in-process shard keeps sending while the App is
    /// paused, so the ordered-update mailbox can demonstrate its backpressure.
    #[arg(long, env = "OPENSHARD_STALL_APP_MS", value_name = "MS")]
    stall_app_ms: Option<u64>,

    /// Draw overhead speech through this TrueType or OpenType face instead of
    /// `fonts.mul`. See `openshard_client_app`'s own flag of the same name.
    #[arg(long, env = "OPENSHARD_TTF_FONT", value_name = "FILE")]
    ttf_font: Option<PathBuf>,

    /// Run a deterministic presentation scenario after the window opens.
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,

    /// Give the window its ground over the connection instead of off the disk.
    ///
    /// `docs/map/new_map_representation/to_the_client.md`'s E2, and the reason
    /// it is worth having here rather than only on `openshard-client-app`: this
    /// is the one launcher where both ends are in one process, so a world that
    /// arrives wrong arrives wrong beside the world it was cut from.
    ///
    /// It changes only the *window*. The shard still reads whatever the config
    /// and `window_base_set` point it at — it has to, since what the window
    /// fetches is the shard's own facet — so this is the two ends reading one
    /// world by construction rather than by both being pointed at one file.
    #[arg(long)]
    world_from_shard: bool,
}

fn main() -> ExitCode {
    // Before the command line is parsed, because what the file holds is the
    // environment those `env =` options fall back to. Exporting the variables,
    // or typing the flags, is the same run.
    openshard_client_app::load_env();
    let cli = Cli::parse();

    let mut filter = EnvFilter::try_new(&cli.log).unwrap_or_else(|_| EnvFilter::new("info"));
    if !cli.jank_log {
        filter = filter.add_directive(
            "jank=off"
                .parse()
                .expect("the built-in jank logging directive is valid"),
        );
    }
    tracing_subscriber::fmt().with_env_filter(filter).init();
    if cli.jank_log {
        if let Err(error) = openshard_client_app::start_jank_log(JANK_LOG.as_ref()) {
            eprintln!("opening {JANK_LOG}: {error}");
            return ExitCode::FAILURE;
        }
        eprintln!("jank frames over 16 ms will be written to {JANK_LOG}");
    }

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

    // Which world the shard is about to run on, read out of the config before it
    // moves into the closure. The window takes the same one: two ends of one
    // process reading two different worlds is the disagreement this playground
    // exists to make impossible, and a base set is a *different world* from the
    // install it was imported from the moment one patch is committed to the log
    // beside it. See `docs/map/new_map_representation/to_the_client.md`.
    let base_set = openshard_e2e_shard::window_base_set(operator.as_ref());
    if let Some(base_set) = &base_set {
        match cli.world_from_shard {
            // The shard still reads it — it has to, since what the window
            // fetches is the shard's own facet. What changed is only which end
            // opens the file.
            true => eprintln!(
                "the shard reads facet 0 from {}, and the window asks the shard for it",
                base_set.display()
            ),
            false => eprintln!("both ends read facet 0 from {}", base_set.display()),
        }
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
    let (dial, shard) = openshard_e2e_shard::in_process::spawn(
        move |stated| {
            let mut config = operator.unwrap_or_else(|| openshard_e2e_shard::stock_config(stated));
            // Both addresses, whichever config this is: nothing binds a port
            // here, but the `0x8C` relay still tells the client where to dial
            // and the client still obeys it. An operator's `advertise` — a LAN
            // address, a public one — would send this window somewhere that is
            // not this process, and the login would end politely and silently.
            config.server.listen = stated;
            config.server.advertise = stated;
            config.world.client_files = files;
            config
        },
        cli.seed.clone(),
    );
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
        // The flag wins over the config's base set, and the line above already
        // said which file that was — see `openshard-client-app`'s own `main`,
        // where the precedence is argued.
        match (cli.world_from_shard, base_set.as_deref()) {
            (true, _) => openshard_client_app::WorldSource::Shard,
            (false, Some(base_set)) => openshard_client_app::WorldSource::BaseSet(base_set),
            (false, None) => openshard_client_app::WorldSource::Install,
        },
        Some((dial, plan)),
        cli.ttf_font,
        openshard_client_app::Opening {
            at: None,
            solids: false,
            stall_on_update: cli.stall_app_ms.map(Duration::from_millis),
            scenario: cli.scenario.map(|scenario| match scenario {
                Scenario::CraftCatalogue => openshard_client_app::Scenario::CraftCatalogue,
                Scenario::ZoomSoak => openshard_client_app::Scenario::ZoomSoak,
                Scenario::LodSweep => openshard_client_app::Scenario::LodSweep,
            }),
        },
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
