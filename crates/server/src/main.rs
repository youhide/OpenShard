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
use openshard_login::{DevAccounts, LoginServer, LoginSession, Response};
use openshard_persistence::{MemoryStore, Snapshot, Store};
use openshard_protocol::{huffman, CharacterPlay, GameServerLogin, WalkRequest};
use openshard_world::{Command, Map, MapTerrain, TileData, World, TICK_INTERVAL};
use std::sync::Arc;
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
    let store = open_store();
    tokio::spawn(server.run());
    run_shard(events, &config, advertised, world, store).await;
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

/// Where the world is kept.
///
/// Nowhere, for now. The save path is wired end to end and the store at the end
/// of it holds everything in memory, so a restart loses the world exactly as it
/// did before — the difference is that the tick, the journal and the save task
/// are the ones that will be there when a real database is, and they are being
/// exercised rather than waiting to be written.
///
/// The warning is not decoration. A shard that says nothing here is a shard an
/// operator assumes is saving.
fn open_store() -> Arc<dyn Store> {
    warn!(
        "no database yet: the world is kept in memory and lost at stop. \
         Characters will not survive a restart."
    );
    Arc::new(MemoryStore::new())
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

/// Write snapshots, forever, on a task nothing waits for.
///
/// # This is the only place that touches a disk
///
/// And it is deliberately somewhere the tick cannot reach. The world hands over
/// owned values and moves on; whatever happens here — a slow disk, a lock, a
/// database in another country — happens to this task and to nothing else. A
/// shard whose store is wedged saves late. It does not lag, and it does not stop
/// letting people play.
///
/// # A failed write is reported, not retried here
///
/// Retrying from here would write the same stale snapshot at a world that has
/// moved on. The failure goes back to the shard loop, which asks the world for a
/// full sweep — see `World::resweep`. The cost of a failure is a fat save, and
/// the recovery reads the world as it is now rather than as it was.
async fn save_loop(
    store: Arc<dyn Store>,
    mut snapshots: mpsc::UnboundedReceiver<Snapshot>,
    failures: mpsc::UnboundedSender<()>,
) {
    while let Some(snapshot) = snapshots.recv().await {
        let rows = snapshot.len();
        let started = Instant::now();
        match store.save(&snapshot).await {
            Ok(()) => debug!(
                tick = snapshot.tick,
                rows,
                took = ?started.elapsed(),
                "saved"
            ),
            Err(error) => {
                error!(tick = snapshot.tick, rows, %error, "save failed; the next one will be a full sweep");
                // If the shard loop is gone there is nobody to sweep and nothing
                // to do about it. The `let _` is that, not carelessness.
                let _ = failures.send(());
            }
        }
    }
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
    store: Arc<dyn Store>,
) {
    let (saves, snapshots) = mpsc::unbounded_channel();
    let (failed, mut failures) = mpsc::unbounded_channel();
    tokio::spawn(save_loop(store, snapshots, failed));

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
                        // A connection reaches the world only after its game
                        // login, so this is always a game connection and every
                        // packet leaves compressed. `send_packet` gates on the
                        // flag anyway, so it stays correct if that ever changes.
                        let _ = session.send_packet(out.packet);
                    }
                }
                // Handed off, not awaited. The tick's job here is to stop
                // holding the only copy.
                for snapshot in world.drain_saves() {
                    let _ = saves.send(snapshot);
                }
            }

            // Before `events`: a store that is failing is worth hearing about
            // ahead of the next packet, and there is never a queue of these.
            Some(()) = failures.recv() => {
                warn!("a save failed; marking the world for a full sweep");
                world.resweep();
            }

            event = events.recv() => {
                let Some(event) = event else {
                    error!("the gateway stopped");
                    return;
                };
                handle(&mut sessions, &mut login, &mut world, advertised, event);
            }
        }

        // Reclaim keys from clients that selected a shard and never came back.
        login.keys.expire(Instant::now());
    }
}

/// Whether the relay is about to send this client somewhere it cannot get back
/// from.
///
/// True when `advertise` is loopback and the client is not on this machine: the
/// relay will tell it to dial `127.0.0.1`, it will reach its own loopback, find
/// nothing, and give up.
///
/// # Why this is worth catching here and not only at startup
///
/// The startup warning fires before anyone has connected, and it scrolls away.
/// By the time the mistake is *made* — the moment a client that is not on this
/// machine picks the shard — the warning is a hundred lines up, and what the
/// operator is looking at is a client stuck on "logging into shard" and a server
/// log that says nothing is wrong.
///
/// And nothing here *is* wrong, which is the whole difficulty. This end sends a
/// perfectly good packet and never sees a second connection, because the failure
/// happens somewhere it cannot observe. This is the last moment the shard can
/// still see both addresses at once and say what is about to happen.
fn relay_is_unreachable(client: SocketAddr, advertised: SocketAddrV4) -> bool {
    advertised.ip().is_loopback() && !client.ip().is_loopback()
}

fn handle(
    sessions: &mut HashMap<ConnectionId, Session>,
    login: &mut LoginServer<DevAccounts>,
    world: &mut World,
    advertised: SocketAddrV4,
    event: ServerEvent,
) {
    match event {
        ServerEvent::Connected {
            id,
            address,
            outbox,
        } => {
            info!(%id, %address, "connected");
            if relay_is_unreachable(address, advertised) {
                error!(
                    client = %address,
                    %advertised,
                    "this client is not on this machine and server.advertise is loopback. \
                     When it picks the shard it will be told to dial {advertised} — its own \
                     loopback — and will hang on \"logging into shard\" until it times out. \
                     Set server.advertise to the address this client can reach."
                );
            }
            sessions.insert(
                id,
                Session {
                    login: LoginSession::new(),
                    in_world: false,
                    game: false,
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
                    // The game login is the seam Sphere calls CONNECT_GAME: from
                    // here on, this connection's every server->client packet is
                    // Huffman-compressed — starting with the character list this
                    // very packet triggers. Set the flag before the reply is
                    // built so that reply goes out compressed.
                    if packet.first().copied() == Some(GameServerLogin::ID) {
                        session.game = true;
                    }
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
    /// Whether this is a game-server connection, whose every server-to-client
    /// packet is Huffman-compressed.
    ///
    /// The UO login connection is uncompressed; the game connection compresses
    /// everything from the character list on. This mirrors Sphere's
    /// `CONNECT_GAME`, which it sets during the game socket's crypt handshake —
    /// before the character list is sent — so the list and all world traffic go
    /// out compressed. Here the seam is the `0x91` game login: see the flag being
    /// set in `handle`.
    game: bool,
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
            Response::Send(bytes) => self.send_packet(bytes),
            Response::SendThenClose(bytes) => {
                let _ = self.send_packet(bytes);
                false
            }
            Response::Close => {
                warn!(%id, "closing on a protocol error");
                false
            }
        }
    }

    /// Send one server-to-client packet, compressing it on a game connection.
    ///
    /// The login connection sends plain bytes; the game connection Huffman-
    /// compresses every packet, each one independently — terminator and all —
    /// exactly as Sphere's `CNetworkOutput` does for `CONNECT_GAME`. Skip this
    /// and ClassicUO, which decompresses the game stream unconditionally, decodes
    /// the raw bytes through its Huffman tree, produces plausible garbage for a
    /// while, and then desyncs on a fabricated packet id far downstream —
    /// surfacing as `need more data ID: 0E ...` hundreds of bytes in, looking
    /// nothing like a compression problem.
    fn send_packet(&self, bytes: Vec<u8>) -> bool {
        let bytes = if self.game {
            huffman::compress(&bytes)
        } else {
            bytes
        };
        self.outbox.send(bytes).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(address: &str) -> SocketAddr {
        address.parse().expect("a client address")
    }

    fn advertise(address: &str) -> SocketAddrV4 {
        address.parse().expect("an advertised address")
    }

    #[test]
    fn a_loopback_advertise_is_unreachable_to_a_client_on_the_network() {
        // The bug this exists for: the client dials its own loopback and hangs
        // on "logging into shard" while this end sees one connection, a clean
        // disconnect, and nothing to explain either.
        assert!(relay_is_unreachable(
            client("192.168.11.163:51606"),
            advertise("127.0.0.1:2593")
        ));
    }

    #[test]
    fn a_loopback_advertise_is_fine_for_a_client_on_this_machine() {
        // And this is why the shard does not simply refuse to start on a
        // loopback advertise: a developer with the client on their own desk is
        // the common case, and it works.
        assert!(!relay_is_unreachable(
            client("127.0.0.1:51606"),
            advertise("127.0.0.1:2593")
        ));
    }

    #[test]
    fn a_real_advertise_is_fine_for_anyone() {
        for address in ["127.0.0.1:51606", "192.168.11.163:51606", "8.8.8.8:51606"] {
            assert!(
                !relay_is_unreachable(client(address), advertise("192.168.11.10:2593")),
                "{address} should be able to reach an advertised LAN address"
            );
        }
    }

    #[test]
    fn an_ipv6_loopback_client_is_still_on_this_machine() {
        // `::1` is loopback and the obvious check — comparing against the string
        // "127.0.0.1", or against Ipv4Addr::LOCALHOST — misses it, and fires a
        // scary error at a developer whose client happens to have resolved
        // localhost to v6.
        assert!(!relay_is_unreachable(
            client("[::1]:51606"),
            advertise("127.0.0.1:2593")
        ));
    }

    fn session(game: bool) -> (Session, mpsc::UnboundedReceiver<Vec<u8>>) {
        let (outbox, wire) = mpsc::unbounded_channel();
        (
            Session {
                login: LoginSession::new(),
                in_world: false,
                game,
                outbox,
            },
            wire,
        )
    }

    #[test]
    fn a_game_connection_compresses_and_a_login_one_does_not() {
        // The whole bug. ClassicUO Huffman-decodes every packet on the game
        // connection; send one raw and it decodes garbage and desyncs later on a
        // fabricated id ("need more data ID: 0E ..."). A character-list-shaped
        // packet, since 0xA9 is the first thing the game connection ever sends.
        let packet = vec![0xA9u8, 0x00, 0x08, 0x05, b'L', b'o', b'r', b'd'];

        let (game, mut wire) = session(true);
        assert!(game.send_packet(packet.clone()));
        let on_wire = wire.try_recv().expect("a packet was sent");
        assert_ne!(on_wire, packet, "a game packet must not leave raw");
        assert_eq!(
            huffman::decompress(&on_wire).expect("valid stream"),
            packet,
            "and the client must get its bytes back"
        );

        let (login, mut wire) = session(false);
        assert!(login.send_packet(packet.clone()));
        assert_eq!(
            wire.try_recv().expect("a packet was sent"),
            packet,
            "the login connection is never compressed"
        );
    }
}
