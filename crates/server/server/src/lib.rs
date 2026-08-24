//! The shard, as a library.
//!
//! Two loops and a channel between them:
//!
//! ```text
//!   gateway tasks ──> ServerEvent ──> [ this loop ] ──> Command ──> World::tick
//!                                            │                          │
//!                                            ├──────  Outbound  <───────┤
//!                                            │                          │
//!                                            ├──> [ save task ]  <──  Snapshot
//!                                            │           │
//!                                            │         a disk
//!                                            │
//!                                            ├──> [ argon2 ] ──> Verdict ──┐
//!                                            └─────────────────────────────┘
//! ```
//!
//! Everything that leaves this loop leaves because waiting for it here would
//! stop the world: a disk that is slow, a hash that is slow on purpose. Both
//! come back as something to react to, on the same `select!` as a packet.
//!
//! This crate owns neither half. The gateway is a state machine with its own
//! tests; the world is a tick with its own. What is here is the wiring: read
//! events, decide whether they are login's or the world's, and drive the clock.
//!
//! It is deliberately thin. If logic starts collecting here it belongs in a
//! crate.
//!
//! # Why a library and not only a binary
//!
//! [`shard::run_shard`] is the whole shard, and a test that wants to *be a
//! client* needs one running. Out of process that means building a binary,
//! writing a config file, guessing a port and waiting for a log line; in
//! process it means calling a function. So the wiring is a library, `main.rs`
//! is the few lines that start it, and `crates/e2e` logs in over a real socket
//! against the same code an operator runs.
//!
//! # The `use` list below is load-bearing
//!
//! Every module here opens with `use super::*`, so the crate root is where
//! their shared imports live. Removing one because this file does not name it
//! breaks a module that does.

use std::collections::HashMap;
use std::net::{SocketAddr, SocketAddrV4};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use openshard_config::{Config, DEFAULT_TOML};
use openshard_gateway::{
    ClientGatewayServer, ConnectionId, Event, OutboxTx, Packet, PacketError, ServerEvent, ServerEventRx,
    Shutdown, VersionTx,
};
use openshard_login::{DevAccounts, LoginServer, LoginSession, Outcome, Response};
use openshard_persistence::{AccountRecord, PgStore, Snapshot, SqliteStore, Store};
use openshard_protocol::client_packet::ClientPacket;
use openshard_protocol::encoded::EncodedSubcommand;
use openshard_protocol::extended::ExtendedRequest;
use openshard_protocol::login::{ClientLoginDecodeError, LoginStagePacket, StartLocation};
use openshard_protocol::mobile::StatusQueryKind;
use openshard_protocol::trade::SecureTradeAction;
use openshard_protocol::wire::ClilocId;
use openshard_protocol::world::{Facet, Point};
use openshard_protocol::{access::AccessLevel, huffman};
use openshard_state::facet_rules::FacetRules;
use openshard_world::{
    AdminMenuAction, Command, PlayerEntered, PlayerLeaving, PlayerLeft, PlayerRefused, RestoredCharacters,
    RestoredItems, StatLock, TICK_INTERVAL, World,
};
use tokio::sync::{Semaphore, mpsc};
use tracing::{debug, error, info, warn};

pub mod boot;
pub mod shard;
pub mod stop;

mod content;
mod dispatch;
mod session;
#[cfg(test)]
mod testing;
mod verify;

use boot::{load_config, load_world, open_store};
use dispatch::dispatch_world_packet;
use session::{PhaseSync, Session, Sessions};
use shard::{Reins, run_shard};
use verify::{Verdict, Verifier};

/// Where the config lives, relative to the working directory.
pub const CONFIG_PATH: &str = "openshard.toml";

/// Load the config, open the store, bind the port, and serve until asked to stop.
///
/// The binary's whole body, kept here so that what an operator starts and what
/// a test could start are the same code.
///
/// `seed` is the admin verbs this run was asked to send itself before the first
/// tick — `--seed` on the command line. Empty is the normal case: a shard whose
/// world is already laid, or one an operator will lay through `.admin`.
///
/// Returns once the world has been saved: [`run_shard`] does not return before
/// that, and neither does this.
pub async fn run(seed: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config(CONFIG_PATH)?;

    // One stop for the whole process — the listener, every connection, and the
    // tick. It is made here rather than inside any of them, because a shard that
    // stopped in pieces would be a shard whose parts each decided when to go.
    // The tally beside it is what the save task owes the disk; the two are one
    // value because they go to the same two places — see [`Reins`].
    let reins = Reins::new();

    let (gateway_server, events) = ClientGatewayServer::bind(config.server.listen, reins.shutdown()).await?;
    info!(
        shard = config.server.name,
        listen = %config.server.listen,
        advertise = %config.server.advertise,
        accounts = config.accounts.len(),
        "OpenShard starting"
    );
    if config.server.advertise.ip().is_loopback() {
        warn!(
            "server.advertise is loopback: only clients on this machine can reach the shard. \
             Set it to the address clients dial."
        );
    }

    let world = load_world(&config)?;
    let store = open_store(&config).await?;

    // A signal is the operator's way to ask, and this is the only place the
    // process listens for one: the shard loop watches the same `Shutdown` a test
    // would use, so a stop is one thing that happens rather than two paths that
    // have to agree. Installing the handlers here rather than inside the spawned
    // task is deliberate — see [`stop::install`]; until they are installed a
    // `SIGTERM` kills the shard instead of stopping it.
    match stop::install() {
        Ok(signals) => {
            tokio::spawn(stop::watch(signals, reins.clone()));
        }
        Err(error) => {
            error!(%error, "cannot listen for stop signals; this shard will only stop when killed")
        }
    }

    tokio::spawn(gateway_server.run());
    run_shard(events, &config, world, store, reins, seed).await;

    Ok(())
}
