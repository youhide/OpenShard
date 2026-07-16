//! The shard binary.
//!
//! Two loops and a channel between them:
//!
//! ```text
//!   gateway tasks ──> ServerEvent ──> [ this loop ] ──> Command ──> World::tick
//!                                            │                          │
//!                                            └──────  Outbound  <───────┘
//! ```
//!
//! This file owns neither half. The gateway is a state machine with its own
//! tests; the world is a tick with its own. What is here is the wiring: read
//! events, decide whether they are login's or the world's, and drive the clock.
//!
//! It is deliberately thin. If logic starts collecting here it belongs in a
//! crate.

use std::collections::HashMap;
use std::net::SocketAddrV4;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use openshard_config::{Config, DEFAULT_TOML};
use openshard_gateway::{ConnectionId, Event, Server, ServerEvent};
use openshard_login::{DevAccounts, LoginServer, LoginSession, Response};
use openshard_protocol::{CharacterPlay, WalkRequest};
use openshard_world::{Command, Map, MapTerrain, TileData, World, TICK_INTERVAL};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
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

    let world = load_world(&config)?;
    tokio::spawn(server.run());
    run_shard(events, &config, advertised, world).await;
    Ok(())
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

/// Load the client's map, if it is configured.
///
/// Blocking, and on purpose: this reads over a hundred megabytes and takes a
/// moment, and there is no sense accepting a client before the world it will
/// walk in exists.
fn load_world(config: &Config) -> Result<World, Box<dyn std::error::Error>> {
    let start = (config.world.start.x, config.world.start.y);
    let dir = config.world.client_files.trim();
    if dir.is_empty() {
        warn!(
            "world.client_files is empty: running with no map. Every step will be allowed — \
             players walk through walls and across water. Set it to a client install."
        );
        return Ok(World::new(start));
    }

    let dir = Path::new(dir);
    let started = Instant::now();
    let map = Map::load_facet(dir, 0)?;
    let tiles = TileData::load(dir.join("tiledata.mul"))?;

    // A start position off the map, or in the sea, is worth saying out loud: the
    // shard still runs and every player spawns somewhere useless.
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
    Ok(World::new(start).with_terrain(MapTerrain::new(map, tiles)))
}

/// Drive login and the world until the gateway stops.
///
/// One task owns both. That is not a limitation: the world is deliberately
/// single-threaded — a deterministic tick is the whole point — and login is a
/// state machine that does no work worth parallelising. Async lives in the
/// gateway's tasks, on the far side of the channel.
async fn run_shard(
    mut events: mpsc::UnboundedReceiver<ServerEvent>,
    config: &Config,
    advertised: SocketAddrV4,
    mut world: World,
) {
    let mut accounts = DevAccounts::new();
    for account in &config.accounts {
        accounts = accounts.with_account(&account.name, &account.password);
        for character in &account.characters {
            accounts = accounts.with_character(&account.name, character);
        }
    }
    let mut login = LoginServer::new(accounts, &config.server.name, advertised);
    let mut sessions: HashMap<ConnectionId, Session> = HashMap::new();
    let mut ticker = tokio::time::interval(TICK_INTERVAL);
    // A tick that ran late must not try to catch up by running several in a row:
    // that turns a hiccup into a stall, and a fixed timestep into a variable one.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            // Biased so the tick cannot be starved by a busy network. Without
            // this, a flood of packets would keep `recv` ready forever and the
            // world would stop simulating under exactly the load that needs it.
            biased;

            _ = ticker.tick() => {
                world.tick(Instant::now());
                for out in world.drain_outbound() {
                    if let Some(session) = sessions.get(&out.connection) {
                        let _ = session.outbox.send(out.packet);
                    }
                }
            }

            event = events.recv() => {
                let Some(event) = event else {
                    error!("the gateway stopped");
                    return;
                };
                handle(&mut sessions, &mut login, &mut world, event);
            }
        }

        // Reclaim keys from clients that selected a shard and never came back.
        login.keys.expire(Instant::now());
    }
}

fn handle(
    sessions: &mut HashMap<ConnectionId, Session>,
    login: &mut LoginServer<DevAccounts>,
    world: &mut World,
    event: ServerEvent,
) {
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
                    in_world: false,
                    outbox,
                },
            );
        }

        ServerEvent::Received { id, event } => {
            let Some(session) = sessions.get_mut(&id) else {
                // Disconnected arrived first. Possible: the gateway's tasks and
                // this loop are not synchronised.
                return;
            };
            match event {
                Event::Seeded(seed) => session.login.on_seed(seed),
                Event::Packet(packet) => {
                    if !dispatch(session, world, &packet, id) {
                        sessions.remove(&id);
                        return;
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
            // The world learns on its own schedule. It owns the entity and the
            // serial, and tearing them down from here would be a write to the
            // world from outside the tick.
            world.queue(Command::Disconnect { connection: id });
            sessions.remove(&id);
        }
    }
}

/// Turn a packet the world cares about into a command. `false` closes.
///
/// Nothing here answers the client. Every reply comes out of a tick, which is
/// what keeps the two ends in one order.
fn dispatch(session: &mut Session, world: &mut World, packet: &[u8], id: ConnectionId) -> bool {
    match packet.first().copied() {
        Some(CharacterPlay::ID) => {
            let Ok(play) = CharacterPlay::decode(packet) else {
                warn!(%id, "malformed 0x5D");
                return false;
            };
            session.in_world = true;
            world.queue(Command::Enter {
                connection: id,
                version: session.login.version(),
                name: play.name,
            });
            true
        }
        Some(WalkRequest::ID) => {
            if !session.in_world {
                debug!(%id, "0x02 before entering the world");
                return true;
            }
            let Ok(request) = WalkRequest::decode(packet) else {
                warn!(%id, "malformed 0x02");
                return false;
            };
            world.queue(Command::Walk {
                connection: id,
                request,
            });
            true
        }
        _ => true,
    }
}

/// Per-connection state this loop owns.
struct Session {
    login: LoginSession,
    /// Whether a character has been asked for. The world owns the entity; this
    /// is only enough to know a `0x02` is worth queueing.
    in_world: bool,
    outbox: mpsc::UnboundedSender<Vec<u8>>,
}

impl Session {
    /// Act on a login response. Returns `false` if the connection should go.
    ///
    /// Dropping the outbox is what closes the socket: the gateway's write task
    /// ends when its channel does. There is no separate "close" to forget.
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
