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
    encode_login_denied, huffman, CharacterPlay, CreateCharacter, DoubleClick, DropItem,
    EquipItemRequest, GameServerLogin, PickUpItem, Point, StartLocation, WalkRequest,
};
use openshard_world::{Appearance, Command, Map, MapTerrain, TileData, World, TICK_INTERVAL};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

mod scripting;
use scripting::Scripts;

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
/// `persistence.database` picks the backend by what it looks like: a
/// `postgres://` (or `postgresql://`) URL connects to PostgreSQL, anything else
/// is a SQLite file path, and an empty string keeps everything in memory and says
/// so. The two databases are equal choices, not a dev-and-prod pair — SQLite runs
/// a live shard perfectly well, and which one an operator wants is theirs to
/// decide.
///
/// The in-memory mode is a real choice too, not a broken one — the same bargain
/// as running with no map — but a shard that stays quiet about it is one an
/// operator assumes is saving, so it warns.
///
/// Opening the database can fail, and that is fatal: a shard told to persist that
/// cannot is not a shard anyone wants started in memory by surprise, losing
/// everything at the next stop.
async fn open_store(config: &Config) -> Result<Arc<dyn Store>, Box<dyn std::error::Error>> {
    let target = config.persistence.database.trim();
    if target.is_empty() {
        warn!(
            "no database configured: the world is kept in memory and lost at stop. \
             Set persistence.database to a file (SQLite) or a postgres:// URL to keep \
             characters across a restart."
        );
        return Ok(Arc::new(MemoryStore::new()));
    }
    if is_postgres_url(target) {
        // The URL can carry a password, so it is never logged — only that this is
        // the PostgreSQL backend.
        let store = PgStore::connect(target)
            .await
            .map_err(|error| format!("could not connect to PostgreSQL: {error}"))?;
        info!("persisting to PostgreSQL");
        return Ok(Arc::new(store));
    }
    let store = SqliteStore::open(target)
        .map_err(|error| format!("could not open the database at {target:?}: {error}"))?;
    info!(path = target, "persisting to SQLite");
    Ok(Arc::new(store))
}

/// Whether `persistence.database` names a PostgreSQL server rather than a SQLite
/// file. The two `postgres` spellings are the ones libpq itself accepts.
fn is_postgres_url(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.starts_with("postgres://") || lower.starts_with("postgresql://")
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
    // One tile table, shared by every facet: `tiledata.mul` describes tiles, not
    // a map, so it is read once and each facet's terrain gets a copy.
    let tiles = TileData::load(dir.join("tiledata.mul"))?;

    let mut world = World::new(start);
    for &facet in &config.world.facets {
        let map = Map::load_facet(dir, facet)?;
        // The start is only checked against facet 0, where new characters spawn.
        // A start off the map, or in the sea, is worth saying out loud: the shard
        // still runs and every player spawns somewhere useless.
        if facet == 0 {
            match map.land(start.0, start.1) {
                Some(cell) => info!(x = start.0, y = start.1, z = cell.z, "start position"),
                None => warn!(
                    x = start.0,
                    y = start.1,
                    "world.start is off the map; characters will spawn in nowhere"
                ),
            }
        }
        info!(
            facet,
            name = map.facet_name(),
            size = format!("{}x{}", map.width(), map.height()),
            statics = map.static_count(),
            "facet loaded"
        );
        world = world.with_facet(facet, MapTerrain::new(map, tiles.clone()));
    }
    info!(
        facets = config.world.facets.len(),
        tiledata = ?tiles.format(),
        took = ?started.elapsed(),
        "world loaded"
    );
    Ok(world)
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

    let mut accounts = DevAccounts::new();
    for account in &config.accounts {
        accounts = accounts.with_account(&account.name, &account.password);
        for character in &account.characters {
            accounts = accounts.with_character(&account.name, character);
        }
    }

    // Bring the world back from the database: reserve every stored serial so a new
    // character cannot take one, list the stored characters so they show up to
    // play, and keep their records so playing one restores it where it was. This
    // borrows the store; the save task takes ownership after, so the load has to
    // come first.
    for account in &config.accounts {
        let record = AccountRecord {
            name: account.name.clone(),
            credential: account.password.clone(),
        };
        if let Err(error) = store.put_account(&record).await {
            warn!(account = account.name, %error, "could not persist a configured account");
        }
    }
    let mut saved: HashMap<(String, String), CharacterRecord> = HashMap::new();
    match store.characters().await {
        Ok(characters) => {
            for record in characters {
                world.reserve_serial(record.serial);
                let listed = accounts
                    .characters(&record.account)
                    .iter()
                    .any(|entry| entry.name.eq_ignore_ascii_case(&record.name));
                if !listed {
                    accounts = accounts.with_character(&record.account, &record.name);
                }
                saved.insert(
                    (record.account.to_lowercase(), record.name.to_lowercase()),
                    record,
                );
            }
            if !saved.is_empty() {
                info!(
                    characters = saved.len(),
                    "restored the world from the database"
                );
            }
        }
        Err(error) => error!(%error, "could not read saved characters; starting with none"),
    }

    let mut login = LoginServer::new(accounts, &config.server.name, advertised);
    // The character-creation screen needs somewhere to start. Without it the
    // client refuses to create at all — "No city found. Something wrong with the
    // received cities." — because the list it was sent is empty. The list is
    // filtered to the facets this shard loaded, so every city offered is one a
    // player can actually be placed in.
    login.starts = start_cities(
        &config.world.facets,
        (config.world.start.x, config.world.start.y),
    );

    tokio::spawn(save_loop(store, snapshots, failed));

    // The gameplay script, if one is configured. Constructed after the world is
    // built and restored, before the first tick, so its cursors start clean.
    let mut scripts = Scripts::load(&config.scripting.main, &world);

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
                // Feed the script this tick's events and queue its commands for
                // the next one. After the drains, so a command a script emits is
                // applied by a tick and leaves through this same path.
                if let Some(scripts) = scripts.as_mut() {
                    scripts.pump(&mut world);
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
                handle(&mut sessions, &mut login, &mut world, advertised, &saved, event);
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
    saved: &HashMap<(String, String), CharacterRecord>,
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
                    // Character creation crosses the login/world line: it writes
                    // the new character onto the account and then enters the world
                    // with it. Handle it here, where both are in reach — dispatch
                    // sees only the world, and the login state machine is done.
                    if matches!(
                        packet.first().copied(),
                        Some(CreateCharacter::ID_CLASSIC | CreateCharacter::ID_HIGH_SEAS)
                    ) {
                        if !create_character(session, login, world, &packet, id) {
                            sessions.remove(&id);
                        }
                        return;
                    }
                    if !dispatch(session, world, &packet, id, saved) {
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
fn dispatch(
    session: &mut Session,
    world: &mut World,
    packet: &[u8],
    id: ConnectionId,
    saved: &HashMap<(String, String), CharacterRecord>,
) -> bool {
    match packet.first().copied() {
        Some(CharacterPlay::ID) => {
            let Ok(play) = CharacterPlay::decode(packet) else {
                warn!(%id, "malformed 0x5D");
                return false;
            };
            let account = session.login.account().unwrap_or_default().to_owned();
            // A stored character enters on its saved serial, spot and look; one
            // the database has never seen — a config-only character on a fresh
            // shard — enters fresh at the start.
            let key = (account.to_lowercase(), play.name.to_lowercase());
            let record = saved.get(&key);
            let facet = record.map_or(0, |record| record.facet);
            let (serial, position, appearance) = match record {
                Some(record) => (
                    Some(record.serial),
                    Some(Point::new(record.x, record.y, record.z)),
                    Some(Appearance {
                        body: record.body,
                        hue: record.hue,
                    }),
                ),
                None => (None, None, None),
            };
            session.in_world = true;
            world.queue(Command::Enter {
                connection: id,
                version: session.login.version(),
                account,
                name: play.name,
                serial,
                position,
                facet,
                appearance,
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
        Some(PickUpItem::ID) => {
            if !session.in_world {
                return true;
            }
            let Ok(pickup) = PickUpItem::decode(packet) else {
                warn!(%id, "malformed 0x07");
                return false;
            };
            world.queue(Command::PickUpItem {
                connection: id,
                serial: pickup.serial,
                amount: pickup.amount,
            });
            true
        }
        Some(DropItem::ID) => {
            if !session.in_world {
                return true;
            }
            let Ok(drop) = DropItem::decode(packet) else {
                warn!(%id, "malformed 0x08");
                return false;
            };
            world.queue(Command::DropItem {
                connection: id,
                serial: drop.serial,
                position: drop.position,
                container: drop.container,
            });
            true
        }
        Some(DoubleClick::ID) => {
            if !session.in_world {
                return true;
            }
            let Ok(click) = DoubleClick::decode(packet) else {
                warn!(%id, "malformed 0x06");
                return false;
            };
            world.queue(Command::DoubleClick {
                connection: id,
                serial: click.serial,
            });
            true
        }
        Some(EquipItemRequest::ID) => {
            if !session.in_world {
                return true;
            }
            let Ok(equip) = EquipItemRequest::decode(packet) else {
                warn!(%id, "malformed 0x13");
                return false;
            };
            world.queue(Command::EquipItem {
                connection: id,
                item: equip.item,
                layer: equip.layer,
                mobile: equip.mobile,
            });
            true
        }
        _ => true,
    }
}

/// The starting cities offered on the character-creation screen.
///
/// The nine classic towns a new character could wake up in on the original
/// Felucca map — the same list, inns and coordinates RunUO and ServUO have
/// shipped for two decades. Their order is what matters as much as their
/// contents: `start_location` in the create packet is a raw index into this
/// list, so position N here is the city the player picked when they clicked the
/// Nth entry. `create_character` reads the same list back to place the spawn, so
/// the two agree by construction.
///
/// All nine are on facet 0, the only facet a new character starts on, so the
/// list is filtered to the facets this shard actually loaded: offering a city on
/// a facet with no terrain would spawn the player in nowhere. If that leaves it
/// empty — a shard that loaded no facet carrying a starting city — one city at
/// the configured start is kept, because the client refuses an empty list and
/// says so: "No city found. Something wrong with the received cities."
///
/// The description cliloc is left 0: a client older than 7.0.13.0 ignores the
/// field, and a newer one shows the city and inn names either way.
fn start_cities(facets: &[u8], start: (u16, u16)) -> Vec<StartLocation> {
    fn city(area: &str, name: &str, x: i32, y: i32, z: i32) -> StartLocation {
        StartLocation {
            area: area.to_owned(),
            name: name.to_owned(),
            position: (x, y, z),
            map: 0,
            description_cliloc: 0,
        }
    }

    let mut cities: Vec<StartLocation> = [
        city("Yew", "The Empath Abbey", 633, 858, 0),
        city("Minoc", "The Barnacle", 2476, 413, 15),
        city("Britain", "Sweet Dreams Inn", 1496, 1628, 10),
        city("Moonglow", "The Scholars Inn", 4408, 1168, 0),
        city("Trinsic", "The Traveler's Inn", 1845, 2745, 0),
        city("Magincia", "The Great Horns Tavern", 3734, 2222, 20),
        city("Jhelom", "The Mercenary Inn", 1374, 3826, 0),
        city("Skara Brae", "The Falconer's Inn", 618, 2234, 0),
        city("Vesper", "The Ironwood Inn", 2771, 976, 0),
    ]
    .into_iter()
    .filter(|city| facets.contains(&(city.map as u8)))
    .collect();

    if cities.is_empty() {
        cities.push(StartLocation {
            area: "Britannia".to_owned(),
            name: "Britain".to_owned(),
            position: (i32::from(start.0), i32::from(start.1), 0),
            map: i32::from(facets.first().copied().unwrap_or(0)),
            description_cliloc: 0,
        });
    }
    cities
}

/// Create a character on the authenticated account, then enter the world with
/// it — the two halves of what a `0x00`/`0xF8` packet asks for.
///
/// Returns `false` only to drop the connection: a malformed packet, or one with
/// no game login behind it to say whose character this is. A *refused* creation
/// — a full account, an empty or duplicate name — keeps the connection. Sphere
/// answers that with the same `0x82` a login error uses, and the client stays on
/// the creation screen to try again.
fn create_character(
    session: &mut Session,
    login: &mut LoginServer<DevAccounts>,
    world: &mut World,
    packet: &[u8],
    id: ConnectionId,
) -> bool {
    let create = match CreateCharacter::decode(packet) {
        Ok(create) => create,
        Err(error) => {
            warn!(%id, %error, "malformed create-character");
            return false;
        }
    };
    let Some(account) = session.login.account().map(str::to_owned) else {
        warn!(%id, "create-character before a game login");
        return false;
    };

    let name = create.name.trim().to_owned();
    match login.accounts.create_character(&account, &name) {
        Ok(_slot) => info!(%id, account, name, "character created"),
        Err(reason) => {
            warn!(%id, account, name, ?reason, "character creation refused");
            let _ = session.send_packet(encode_login_denied(reason));
            return true;
        }
    }

    // Place the character in the city they picked. `start_location` indexes the
    // very list `start_cities` built and the character-list packet offered, so a
    // valid pick names a real city; only a client sending an out-of-range index
    // falls back to the default facet and a fresh spawn.
    let (facet, position) = match login.starts.get(create.start_location as usize) {
        Some(city) => (
            city.map as u8,
            Some(Point::new(
                city.position.0 as u16,
                city.position.1 as u16,
                city.position.2 as i8,
            )),
        ),
        None => (0, None),
    };

    session.in_world = true;
    world.queue(Command::Enter {
        connection: id,
        version: session.login.version(),
        account,
        name,
        // A brand-new character: a fresh serial, spawned in the chosen city. The
        // tick will journal it, so it is in the database — and in the character
        // list — by the next time the player logs in.
        serial: None,
        position,
        facet,
        appearance: Some(Appearance {
            body: create.body(),
            hue: create.skin_hue,
        }),
    });
    true
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

    #[test]
    fn a_facet_zero_shard_offers_the_classic_towns() {
        // Facet 0 loaded — the normal case — offers the nine classic Felucca
        // cities, every one of them on map 0 with a real, non-origin position.
        let cities = start_cities(&[0], (1363, 1600));
        assert_eq!(cities.len(), 9, "the nine classic starting cities");
        assert!(
            cities.iter().any(|city| city.area == "Britain"),
            "Britain is one of them"
        );
        for city in &cities {
            assert_eq!(city.map, 0, "every classic city is on Felucca");
            assert!(
                city.position.0 > 0 && city.position.1 > 0,
                "a real spot, not the origin"
            );
        }
    }

    #[test]
    fn a_shard_without_facet_zero_still_offers_one_city() {
        // An empty list is what makes ClassicUO refuse to open the creation
        // screen. No classic city lives on a non-zero facet, so a shard that
        // loaded only facet 1 keeps a single fallback at the configured start —
        // on a facet it actually loaded, not facet 0 it did not.
        let cities = start_cities(&[1], (1363, 1600));
        assert_eq!(cities.len(), 1, "never empty");
        assert_eq!(cities[0].position, (1363, 1600, 0));
        assert_eq!(cities[0].map, 1, "on a loaded facet");
    }

    #[test]
    fn start_location_indexes_the_offered_list() {
        // The contract create_character depends on: the byte the client sends is
        // a raw index into exactly this list, so the Nth city is the one picked
        // by clicking the Nth entry. If this order ever shifts, spawns land in
        // the wrong town silently.
        let cities = start_cities(&[0], (1363, 1600));
        assert_eq!(cities[0].area, "Yew");
        assert_eq!(cities[2].area, "Britain");
        assert_eq!(cities[8].area, "Vesper");
    }
}
