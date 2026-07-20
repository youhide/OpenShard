//! The shard binary.
//!
//! Two loops and a channel between them:
//!
//! ```text
//!   gateway tasks ──> ServerEvent ──> [ this loop ] ──> Command ──> World::tick
//!                                            │                          │
//!                                            ├──────  Outbound  <───────┤
//!                                            │                          │
//!                                            └──> [ save task ]  <──  Snapshot
//!                                                      │
//!                                                    a disk
//! ```
//!
//! This file owns neither half. The gateway is a state machine with its own
//! tests; the world is a tick with its own. What is here is the wiring: read
//! events, decide whether they are login's or the world's, and drive the clock.
//!
//! It is deliberately thin. If logic starts collecting here it belongs in a
//! crate.

use std::collections::HashMap;
use std::net::{SocketAddr, SocketAddrV4};
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use openshard_config::{Config, DEFAULT_TOML};
use openshard_gateway::{ConnectionId, Event, Server, ServerEvent};
use openshard_login::{Accounts, DevAccounts, LoginServer, LoginSession, Response};
use openshard_persistence::{
    AccountRecord, CharacterRecord, MemoryStore, PgStore, Snapshot, SqliteStore, Store,
};
use openshard_protocol::{
    encode_login_denied, huffman, AccessLevel, AttackRequest, CastSpellRequest, CharacterPlay,
    ClientVersion, ContextMenuRequest, ContextMenuSelect, CreateCharacter, DoubleClick, DropItem,
    EquipItemRequest, GameServerLogin, GumpResponse, LookRequest, PickUpItem, Point,
    PropertyQueryRequest, SkillLock, SkillLockRequest, StartLocation, TalkRequest, TargetResponse,
    UnicodeTalkRequest, WalkRequest, WarModeRequest,
};
use openshard_world::{
    Appearance, CharacterSheet, Command, Gameplay, Map, MapTerrain, TileData, World, TICK_INTERVAL,
};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

mod scripting;
use scripting::Scripts;

mod boot;
mod dispatch;
mod session;
mod shard;

use boot::{load_config, load_world, open_store};
use dispatch::{create_character, dispatch, start_cities};
use session::Session;
use shard::run_shard;

/// Where the config lives, relative to the working directory.
const CONFIG_PATH: &str = "openshard.toml";

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    match run().await {
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

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config(CONFIG_PATH)?;

    // The 0x8C relay carries four bytes of address. There is no IPv6 form of it,
    // so a v6 `advertise` cannot be honoured — better to say so at startup than
    // to hand clients an address the packet cannot express.
    let advertised = config.advertise_v4().ok_or(
        "server.advertise is IPv6; the UO relay packet has four bytes for an address \
         and no way to carry one",
    )?;

    let (server, events) = Server::bind(config.server.listen).await?;
    info!(
        shard = config.server.name,
        listen = %config.server.listen,
        advertise = %config.server.advertise,
        accounts = config.accounts.len(),
        "OpenShard starting"
    );
    if advertised.ip().is_loopback() {
        warn!(
            "server.advertise is loopback: only clients on this machine can reach the shard. \
             Set it to the address clients dial."
        );
    }

    let world = load_world(&config)?;
    let store = open_store(&config).await?;
    tokio::spawn(server.run());
    run_shard(events, &config, advertised, world, store).await;
    Ok(())
}
