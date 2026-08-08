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

use clap::Parser;
use openshard_client_net::session::{Pick, Plan};
use openshard_client_net::transport::Tcp;
use openshard_protocol::identity::{RawAccountName, RawPlaintextPassword};

/// Where a shard is, when one is asked for and no address is given.
const DEFAULT_SHARD: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 2593);

/// A window on a client install, and a shard to play if one was asked for.
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// The client install to read.
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,

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
    /// `fonts.mul` only defines Latin text and a handful of symbols — no
    /// Cyrillic, no anything past `0xFF` — so a shard whose players type in
    /// one of those scripts needs this set, to a `.ttf`/`.otf` on this
    /// machine. Nothing is bundled with the engine — see
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
    at: Option<(u16, u16)>,

    /// Draw the lighting's occlusion grid as solids from the first frame.
    ///
    /// The same view F5 toggles — `docs/lighting.md` step 23.0. As a flag
    /// because a picture of a place is taken with a command line and not with a
    /// hand on a checkbox, and because the two together (`--at` and this) are
    /// what make one reproducible.
    #[arg(long)]
    solids: bool,
}

/// `X,Y` off the command line, as a tile.
///
/// A `Result` because this is outside the process — the same rule the wire's
/// bytes follow — and the message is what a person sees, so it names the form
/// rather than the parser's own complaint.
fn tile(text: &str) -> Result<(u16, u16), String> {
    let (x, y) = text
        .split_once(',')
        .ok_or_else(|| format!("expected a tile as X,Y, got {text:?}"))?;
    let read = |part: &str, axis: &str| {
        part.trim()
            .parse::<u16>()
            .map_err(|error| format!("{axis} of {text:?}: {error}"))
    };
    Ok((read(x, "the x")?, read(y, "the y")?))
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
    };
    openshard_client_app::run(&cli.client, shard, cli.ttf_font, opening)
}
