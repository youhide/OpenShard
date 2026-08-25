//! The client binary: a command line, read into [`openshard_client_app::run`].
//!
//! ```sh
//! cargo run -p openshard-client-app -- --client "/path/to/Ultima Online Classic"
//! ```
//!
//! Every option is also an environment variable — the same `OPENSHARD_*` names
//! as before — and a `.env` beside the workspace root is read before the command
//! line is parsed, so an install can be named once and never again. `--help`
//! lists both spellings.
//!
//! With an account it logs in to `--server` (or the default port on this
//! machine); without one it is an offline map viewer. Everything else — the
//! window, the wire, the world — is in the library beside this file, so that a
//! caller with a shard of its own can start the same client without an
//! environment or a command line at all. `crates/e2e/playground` is that caller.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use openshard_client_app::WorldSource;
use openshard_client_net::session::{Pick, Plan};
use openshard_client_net::transport::Tcp;
use openshard_map::grid::Tile;
use openshard_protocol::identity::{RawAccountName, RawPlaintextPassword};
use tracing_subscriber::EnvFilter;

/// Where a shard is, when one is asked for and no address is given.
const DEFAULT_SHARD: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 2593);

/// A reproducible diagnostic path that is injected into the client itself.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Scenario {
    /// Zoom out to the first map-composite tier and pan across block boundaries.
    LodSweep,
    /// Hold maximum zoom and audit the static atlas for delayed corruption.
    AtlasSoak,
    /// Zoom out from the default view, then hold still for delayed LOD churn.
    ZoomSoak,
    /// As `zoom-soak`, but ignore server world updates after the injected zoom.
    ZoomSoakFreezeServer,
    /// Zoom out on the real shard connection and periodically audit its live
    /// server-driven frames against direct LOD0 rendering.
    LiveOracle,
}

/// A window on a client install, and a shard to play if one was asked for.
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// The client install to read.
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,

    /// Take the ground from a base set of ours instead of the install's map.
    ///
    /// The same file `world.base_sets` names in a shard's `openshard.toml`, made
    /// by `openshard-map-import`, and the patch log beside it is read with it —
    /// so a client and a shard pointed at one base set draw and enforce the same
    /// revision of the same world. `--client` is still required: what a base set
    /// replaces is `map0LegacyMUL.uop`, `staidx0.mul` and `statics0.mul`, and
    /// the art, the hues, the multis and `tiledata.mul` are the install's either
    /// way.
    ///
    /// The navigation graph and the interiors flood are then read from beside
    /// the base set rather than from the install, because they are derived from
    /// this world and not from that one.
    #[arg(long, env = "OPENSHARD_BASE_SET", value_name = "FILE")]
    base_set: Option<PathBuf>,

    /// Take the ground from the shard, and not from any file on this machine.
    ///
    /// `docs/map/new_map_representation/to_the_client.md`'s E2. The client
    /// starts with no facet at all, is told on world entry how big the one it is
    /// standing in is, asks for every chunk of it, and assembles the world out of
    /// what arrives. `map0LegacyMUL.uop`, `staidx0.mul` and `statics0.mul` need
    /// not exist; `--client` is still required, because the art, the hues, the
    /// multis and `tiledata.mul` are not on the wire and are not going to be.
    ///
    /// It needs an `--account`: a viewer with no shard has nobody to ask.
    ///
    /// **It wins over `--base-set`**, and says so on the way past. That is not
    /// clap's `conflicts_with`, which was tried and is wrong here: a base set
    /// also arrives from `OPENSHARD_BASE_SET`, so a `.env` naming one turned
    /// this flag into an error about an argument nobody typed. A world does come
    /// from one place, and the one a person typed on the command line is it.
    ///
    /// No environment variable of its own, like `--solids` and unlike
    /// `--base-set`: this says what a *run* is doing rather than where this
    /// machine keeps its files, and a `.env` that quietly took the ground off
    /// the wire would be a client that stopped reading the install without
    /// anybody typing anything.
    #[arg(long)]
    world_from_shard: bool,

    /// The account to log in as. Without one this is an offline map viewer.
    #[arg(short, long, env = "OPENSHARD_ACCOUNT")]
    account: Option<String>,

    /// Its password.
    ///
    /// Defaulted rather than required, because a shard in development accepts
    /// whatever it is given and asking for one nobody set would turn a map
    /// viewer into an error.
    #[arg(short, long, env = "OPENSHARD_PASSWORD", default_value = "")]
    password: String,

    /// The shard to connect to.
    #[arg(short, long, env = "OPENSHARD_SERVER", default_value_t = DEFAULT_SHARD, value_name = "ADDR:PORT")]
    server: SocketAddrV4,

    /// The character to play. Without one, the first on the account.
    #[arg(long, env = "OPENSHARD_CHARACTER")]
    character: Option<String>,

    /// Draw overhead speech through this TrueType or OpenType face instead of
    /// `fonts.mul`.
    ///
    /// `fonts.mul` is CP1251 and therefore covers Cyrillic, but cannot cover
    /// Unicode generally or offer another typeface. A shard that needs those
    /// chooses this `.ttf`/`.otf` on the local machine. Nothing is bundled — see
    /// `openshard_uofiles::ttf_font`'s doc for why. Unset draws the classic
    /// client's own bitmap faces, unchanged; there is no mixing the two within
    /// one line — see `openshard_client_render::text::collect_ttf`'s doc for
    /// why.
    #[arg(long, env = "OPENSHARD_TTF_FONT", value_name = "FILE")]
    ttf_font: Option<PathBuf>,

    /// Open the camera on this tile instead of the default one.
    ///
    /// `X,Y` in the facet's own coordinates, which is how every plan in `docs/`
    /// names a place — "the staircase at 1493,1639". Offline it is simply where
    /// the viewer looks; logged in it only moves the eye, and the first thing
    /// that relocks the camera on the character (Home) undoes it.
    #[arg(long, value_name = "X,Y", value_parser = tile)]
    at: Option<Tile>,

    /// Draw the lighting's occlusion grid as solids from the first frame.
    ///
    /// The same view F5 toggles — `docs/lighting.md` step 23.0. As a flag
    /// because a picture of a place is taken with a command line and not with a
    /// hand on a checkbox, and because the two together (`--at` and this) are
    /// what make one reproducible.
    #[arg(long)]
    solids: bool,

    /// Run a deterministic presentation diagnostic inside the client instead
    /// of relying on desktop input events.
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
}

/// `X,Y` off the command line, as a tile.
///
/// A `Result` because this is outside the process — the same rule the wire's
/// bytes follow — and the message is what a person sees, so it names the form
/// rather than the parser's own complaint.
fn tile(text: &str) -> Result<Tile, String> {
    let (x, y) = text
        .split_once(',')
        .ok_or_else(|| format!("expected a tile as X,Y, got {text:?}"))?;
    let read = |part: &str, axis: &str| {
        part.trim()
            .parse::<u16>()
            .map_err(|error| format!("{axis} of {text:?}: {error}"))
    };
    Ok(Tile::new(read(x, "the x")?, read(y, "the y")?))
}

/// The login this run was asked to make, if it was asked for one.
///
/// The account is what decides: a client with no account has nobody to log in
/// as, and asking for a password nobody typed would be worse than drawing the
/// map on its own.
fn plan(cli: &Cli) -> Option<Plan> {
    let account = cli.account.clone()?;
    Some(Plan {
        account: RawAccountName(account),
        password: RawPlaintextPassword(cli.password.clone()),
        shard: Pick::First,
        character: cli.character.clone().map_or(Pick::First, Pick::Named),
    })
}

fn main() -> ExitCode {
    // Before the command line is parsed, because what the file holds is the
    // environment those `env =` options fall back to.
    openshard_client_app::load_env();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")))
        .init();
    let cli = Cli::parse();

    // A real client on a real network. `Tcp` is where the address goes: past
    // this line nothing knows what a socket is.
    let shard = plan(&cli).map(|plan| {
        eprintln!("logging in to {}", cli.server);
        (Tcp::at(cli.server), plan)
    });
    let opening = openshard_client_app::Opening {
        at: cli.at,
        solids: cli.solids,
        stall_on_update: None,
        scenario: cli.scenario.map(|scenario| match scenario {
            Scenario::LodSweep => openshard_client_app::Scenario::LodSweep,
            Scenario::AtlasSoak => openshard_client_app::Scenario::AtlasSoak,
            Scenario::ZoomSoak => openshard_client_app::Scenario::ZoomSoak,
            Scenario::ZoomSoakFreezeServer => openshard_client_app::Scenario::ZoomSoakFreezeServer,
            Scenario::LiveOracle => openshard_client_app::Scenario::LiveOracle,
        }),
    };
    // Where the ground comes out of. `Install` is the arm every run before base
    // sets existed took, and it is a source rather than the absence of one; the
    // third names no file at all.
    //
    // Both at once is not refused, it is *decided* and then said: a base set
    // arrives from `OPENSHARD_BASE_SET` as readily as from the command line, so
    // refusing the pair turns a `.env` into an error message about an argument
    // nobody typed. The flag wins because somebody typed it, and the line below
    // is what stops that from being a guess the client made silently.
    let world = match (cli.world_from_shard, cli.base_set.as_deref()) {
        (true, Some(base_set)) => {
            eprintln!(
                "--world-from-shard: the ground comes from the shard, and not from {}",
                base_set.display()
            );
            WorldSource::Shard
        }
        (true, None) => WorldSource::Shard,
        (false, Some(base_set)) => WorldSource::BaseSet(base_set),
        (false, None) => WorldSource::Install,
    };
    openshard_client_app::run(&cli.client, world, shard, cli.ttf_font, opening)
}
