//! The shard binary.
//!
//! Wires the gateway's events to the login server. This is the first thing in
//! the project you can watch work rather than read assertions about: point a
//! ClassicUO at it and it reaches the character list.
//!
//! It is deliberately thin. Everything it does is a few lines of glue over
//! `openshard-gateway` and `openshard-login`, both of which are pure state
//! machines with their own tests. If logic starts collecting here, it belongs in
//! a crate instead.

mod game;

use std::collections::HashMap;
use std::net::SocketAddrV4;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use openshard_config::{Config, DEFAULT_TOML};
use openshard_gateway::{ConnectionId, Event, Server, ServerEvent};
use openshard_login::{DevAccounts, LoginServer, LoginSession, Response};
use openshard_protocol::{CharacterPlay, WalkRequest};
use openshard_world::{Map, MapTerrain, TileData};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::game::{Game, Player};
use tracing_subscriber::EnvFilter;

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

    let game = load_world(&config)?;
    tokio::spawn(server.run());
    run_login(events, &config, advertised, game).await;
    Ok(())
}

/// Load the client's map, if it is configured.
///
/// Blocking, and on purpose: this reads ~110MB and takes a moment, and there is
/// no sense accepting a client before the world it will walk in exists.
fn load_world(config: &Config) -> Result<Game, Box<dyn std::error::Error>> {
    let start = (config.world.start.x, config.world.start.y);
    let dir = config.world.client_files.trim();
    if dir.is_empty() {
        warn!(
            "world.client_files is empty: running with no map. Every step will be allowed — \
             players walk through walls and across water. Set it to a client install."
        );
        return Ok(Game::new(start));
    }

    let dir = Path::new(dir);
    let started = Instant::now();
    let map = Map::load_facet(dir, 0)?;
    let tiles = TileData::load(dir.join("tiledata.mul"))?;
    // A start position off the map, or in the sea, is worth saying out loud:
    // the shard still runs and every player spawns somewhere useless.
    match map.land(start.0, start.1) {
        Some(cell) => info!(x = start.0, y = start.1, z = cell.z, "start position"),
        None => warn!(
            x = start.0,
            y = start.1,
            "world.start is off the map; characters will spawn in nowhere"
        ),
    }
    info!(
        facet = map.facet_name(),
        size = format!("{}x{}", map.width(), map.height()),
        statics = map.static_count(),
        tiledata = ?tiles.format(),
        took = ?started.elapsed(),
        "map loaded"
    );
    Ok(Game::new(start).with_terrain(MapTerrain::new(map, tiles)))
}

/// Load the config, writing the shipped default if there is none.
///
/// A fresh checkout should run. Writing the default rather than baking one in
/// means the first thing a new operator sees is the file they need to edit, with
/// the `advertise` warning in it, instead of a shard that works on their laptop
/// and nowhere else for reasons nobody wrote down.
fn load_config(path: &str) -> Result<Config, Box<dyn std::error::Error>> {
    if !Path::new(path).exists() {
        std::fs::write(path, DEFAULT_TOML)?;
        info!(path, "no config found; wrote the default");
    }
    Ok(Config::load(path)?)
}

/// Drive login for every connection until the gateway stops.
async fn run_login(
    mut events: mpsc::UnboundedReceiver<ServerEvent>,
    config: &Config,
    advertised: SocketAddrV4,
    mut game: Game,
) {
    let mut accounts = DevAccounts::new();
    for account in &config.accounts {
        accounts = accounts.with_account(&account.name, &account.password);
        for character in &account.characters {
            accounts = accounts.with_character(&account.name, character);
        }
    }
    let mut login = LoginServer::new(accounts, &config.server.name, advertised);

    // One session per live connection. Not a global: this task owns it, and the
    // gateway's Disconnected event is what keeps it from growing forever.
    let mut sessions: HashMap<ConnectionId, Session> = HashMap::new();

    while let Some(event) = events.recv().await {
        match event {
            ServerEvent::Connected {
                id,
                address,
                outbox,
            } => {
                info!(%id, %address, "connected");
                sessions.insert(
                    id,
                    Session {
                        login: LoginSession::new(),
                        player: None,
                        outbox,
                    },
                );
            }

            ServerEvent::Received { id, event } => {
                let Some(session) = sessions.get_mut(&id) else {
                    // Disconnected arrived first. Possible: the gateway's tasks
                    // and this loop are not synchronised.
                    continue;
                };

                match event {
                    Event::Seeded(seed) => {
                        session.login.on_seed(seed);
                    }
                    Event::Packet(packet) => {
                        // The world gets first refusal on packets that are
                        // unambiguously its own; everything else is login's.
                        // `LoginServer` ignores what it does not know, so the
                        // order only matters for ids both could claim — and
                        // there are none.
                        if !dispatch(&mut game, session, &packet, id) {
                            sessions.remove(&id);
                            continue;
                        }
                        let response = login.handle(&mut session.login, &packet, Instant::now());
                        if !session.apply(response, id) {
                            sessions.remove(&id);
                        }
                    }
                }
            }

            ServerEvent::Disconnected { id, reason } => {
                match reason {
                    Some(reason) => warn!(%id, %reason, "disconnected"),
                    None => info!(%id, "disconnected"),
                }
                sessions.remove(&id);
            }
        }

        // Reclaim keys from clients that selected a shard and never came back.
        // Cheap, and this is the only loop that runs often enough to bother.
        login.keys.expire(Instant::now());
    }

    error!("the gateway stopped");
}

/// Per-connection state this loop owns.
struct Session {
    login: LoginSession,
    /// Set once the client picks a character. Before that it is still logging in.
    player: Option<Player>,
    outbox: mpsc::UnboundedSender<Vec<u8>>,
}

/// Give the world a look at a packet. Returns `false` if the connection should go.
fn dispatch(game: &mut Game, session: &mut Session, packet: &[u8], id: ConnectionId) -> bool {
    match packet.first().copied() {
        Some(CharacterPlay::ID) => {
            let Ok(play) = CharacterPlay::decode(packet) else {
                warn!(%id, "malformed 0x5D");
                return false;
            };
            let Some((player, reply)) = game.character_play(&play) else {
                error!(%id, "the mobile serial pool is exhausted");
                return false;
            };
            session.player = Some(player);
            session.send_all(reply.packets)
        }
        Some(WalkRequest::ID) => {
            let Some(player) = session.player.as_mut() else {
                // A walk before a character. Not fatal — a stray packet from a
                // client that reconnected — but nothing to act on either.
                debug!(%id, "0x02 before entering the world");
                return true;
            };
            let Ok(request) = WalkRequest::decode(packet) else {
                warn!(%id, "malformed 0x02");
                return false;
            };
            let reply = game.walk(player, request, Instant::now());
            session.send_all(reply.packets)
        }
        _ => true,
    }
}

impl Session {
    /// Send several packets in order. Returns `false` if the client is gone.
    fn send_all(&self, packets: Vec<Vec<u8>>) -> bool {
        packets
            .into_iter()
            .all(|packet| self.outbox.send(packet).is_ok())
    }

    /// Act on a login response. Returns `false` if the connection should go.
    ///
    /// Dropping the outbox is what closes the socket: the gateway's write task
    /// ends when its channel does. There is no separate "close" to forget to
    /// call.
    fn apply(&self, response: Response, id: ConnectionId) -> bool {
        match response {
            Response::Idle => true,
            Response::Send(bytes) => self.outbox.send(bytes).is_ok(),
            Response::SendThenClose(bytes) => {
                let _ = self.outbox.send(bytes);
                false
            }
            Response::Close => {
                warn!(%id, "closing on a protocol error");
                false
            }
        }
    }
}
