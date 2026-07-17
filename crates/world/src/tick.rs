//! The simulation loop.
//!
//! # Why there is a tick at all
//!
//! Everything so far answers a packet: a client asks to walk, the server says
//! yes. That works right up until something has to happen *without* a client
//! asking — an item decaying, a wound healing, an NPC deciding to move. There is
//! nowhere to put any of it in a request/response server.
//!
//! The tick is that place. It is also what makes the simulation deterministic:
//! commands arrive from network tasks on whatever thread at whatever moment,
//! queue up, and are applied in a fixed order at a fixed rate. Replay the same
//! commands and you get the same world.
//!
//! # The boundary
//!
//! ```text
//!   network tasks          the tick               network tasks
//!   ─────────────>  [ commands ]  ─────────>  [ outbound packets ]
//!        async         drained in order            async again
//! ```
//!
//! The gateway already draws half of this line by handing events to a channel
//! rather than calling back. This is the other half: nothing inside
//! [`World::tick`] awaits, reads a clock, or touches a socket.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use openshard_entities::{EntityId, Registry, Serial, SerialKind};
use openshard_events::{Cursor, EventBus};
use openshard_gateway::ConnectionId;
use openshard_movement::{step_from, OpenWorld, Terrain, Walk, Walker};
use openshard_persistence::{CharacterRecord, Journal, Snapshot};
use openshard_protocol::{
    encode_add_to_container, encode_container_contents, encode_drag_cancel, encode_equip,
    encode_light_level, encode_login_complete, encode_map_change, encode_open_container,
    encode_remove, encode_walk_ack, encode_walk_reject, ClientVersion, ContainedItem, Direction,
    DragCancelReason, Equipment, Facing, MobileIncoming, MobileMove, Notoriety, PlayerStart,
    PlayerUpdate, Point, WalkRequest, WorldItem, DEFAULT_MAP_HEIGHT, DEFAULT_MAP_WIDTH,
    DROP_TO_GROUND,
};
use tracing::{debug, info, warn};

use crate::components::{
    Account, Amount, Body, Client, Contained, Container, Decays, Equipped, Facet, Graphic, Heading,
    Movement, Name, Position, Stackable,
};
use crate::events::{
    ItemSpawned, MobileMoved, MobileTurned, PlayerEntered, PlayerLeft, RefusedReason, StepRefused,
};
use crate::sectors::{in_range, Sectors, VIEW_RANGE};
use crate::terrain::MapTerrain;

/// How often the world ticks.
///
/// 20Hz. Fast enough that a 200ms walk step lands within a tick of when the
/// client expects it, and slow enough to leave room for everything a tick will
/// eventually do. Not a protocol constant — the client does not know or care.
pub const TICK_INTERVAL: Duration = Duration::from_millis(50);

/// A human male body.
const BODY_HUMAN_MALE: u16 = 0x0190;
/// The skin hue a character gets when nothing else chose one — the same one
/// Sphere hands a body with no stored colour.
const DEFAULT_HUE: u16 = 0x83EA;
/// Full daylight. The scale runs backwards: 0 is brightest, 0x1F pitch dark.
const LIGHT_DAY: u8 = 0;
/// The facet a new character spawns on, and the world's fallback for a facet it
/// has not loaded. Zero is Felucca.
const DEFAULT_FACET: u8 = 0;
/// The height to use when there is no map to ask.
const Z_WITHOUT_A_MAP: i8 = 0;
/// Notoriety 0x01 is "innocent" — the blue health bar.
const NOTORIETY_INNOCENT: u8 = 0x01;
/// The facet size used when there is no map. Big enough for anywhere a test
/// puts something; the grid is a `Vec` of empty buckets and costs nothing.
const FACET_WITHOUT_A_MAP: (u32, u32) = (7168, 4096);
/// How close, in tiles (Chebyshev), a player must be to lift an item off the
/// ground or set one down.
///
/// Sphere reaches two; a third forgives the diagonal the cursor is shown on. A
/// starting number and an auditable one, not a rule carved anywhere — reach is
/// exactly the sort of thing a shard retunes.
const ITEM_REACH: u32 = 3;
/// The highest layer an item can be worn on.
///
/// Layers 1–25 are the body: hands, armour, clothing, the mount. Higher numbers
/// are the backpack, the bank box and other slots that are not "worn" and cannot
/// be equipped by dragging an item onto them.
const MAX_WEARABLE_LAYER: u8 = 25;
/// How many ticks an item lies on the ground before it decays.
///
/// Twenty minutes at [`TICK_INTERVAL`] — Sphere's default `DECAY_TIME` is fifteen
/// to thirty depending on the item; one number stands in for the table until
/// per-item decay is script-driven. A starting value, tuned per shard, and the
/// kind of thing §6 hands to scripts.
const DECAY_TICKS: u64 = 20 * 60 * 20;

/// How often the world offers a snapshot to persistence, in ticks.
///
/// Twenty seconds at [`TICK_INTERVAL`]. Sphere's default world save is ten
/// minutes, which is ten minutes of play a crash can cost; that number is from
/// an era when a save walked the entire world and blocked while it did. This one
/// writes what changed, on another task, so it can afford to be frequent.
///
/// In ticks and not a `Duration` on purpose. A shard that has fallen behind
/// should save less often, not spend its shortfall on the disk.
pub const SAVE_EVERY_TICKS: u64 = 400;

/// How a freshly created character looks: its body graphic and hue.
///
/// [`Command::Enter`] carries this when the client just made the character and
/// chose it. Playing an existing one carries `None`, and the world falls back to
/// its default body until persistence can supply the stored appearance.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Appearance {
    /// The body graphic id.
    pub body: u16,
    /// The skin hue.
    pub hue: u16,
}

/// Something for the world to do, from outside the world.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Command {
    /// A client picked a character.
    Enter {
        /// Which connection.
        connection: ConnectionId,
        /// What the client claims to be.
        version: ClientVersion,
        /// The account the character belongs to. Saved with the character so a
        /// load knows whose it is.
        account: String,
        /// The character's name.
        name: String,
        /// The saved wire serial, when a stored character is being played. `None`
        /// creates a fresh one — a character being made for the first time. A
        /// played character must come back with the serial it was saved under,
        /// because that serial is what every packet ever sent about it referred to.
        serial: Option<u32>,
        /// Where to spawn, when a stored character is loaded at its saved spot.
        /// `None` uses the world's configured start — a newly created character,
        /// or a played one from before positions were stored.
        position: Option<Point>,
        /// Which facet to spawn on: a stored character's saved one, or the
        /// world's default for a new character. An unloaded facet falls back to
        /// the default.
        facet: u8,
        /// How the character looks: chosen at creation, or restored from the save.
        /// `None` falls back to the default body.
        appearance: Option<Appearance>,
    },
    /// A client asked to take a step.
    Walk {
        /// Which connection.
        connection: ConnectionId,
        /// The request.
        request: WalkRequest,
    },
    /// The server moves a mobile one step — a script or AI decree, not a client
    /// request.
    ///
    /// Server-authoritative, so unlike [`Walk`](Self::Walk) there is no walk
    /// sequence to keep in step and no pace budget to spend: those exist to catch
    /// a *client* lying about how fast it moves, and the server is not lying to
    /// itself. Only the terrain gets a say. Turning is the step, exactly as it is
    /// for a client — a mobile not yet facing `direction` turns to face it and
    /// stays put, and the next `Step` moves it — because the clients watching
    /// animate the turn and the move the same way whoever ordered it.
    Step {
        /// Which mobile, by wire serial.
        serial: u32,
        /// Which way: the low three bits of a facing byte (0 N, clockwise).
        direction: u8,
    },
    /// The server puts an item on the ground — a script decree.
    ///
    /// Creates a new item entity on its own serial and draws it for everyone who
    /// can see the tile. The item's *rules* — whether it stacks, when it decays,
    /// what it does when used — are not here; this is only "a thing now lies at
    /// this spot", the item counterpart of a mobile entering.
    SpawnItem {
        /// The tiledata graphic id.
        graphic: u16,
        /// Its hue, or 0 for none.
        hue: u16,
        /// How many, for a stackable item; 0 or 1 is a single.
        amount: u16,
        /// Whether it merges with an identical pile when dropped onto one.
        stackable: bool,
        /// Where it lies.
        position: Point,
        /// Which facet.
        facet: u8,
    },
    /// The server puts a container on the ground — a script decree, like
    /// [`SpawnItem`](Self::SpawnItem) but the thing can hold others.
    SpawnContainer {
        /// The tiledata graphic id (a chest, a backpack).
        graphic: u16,
        /// The gump the client opens when it is double-clicked.
        gump: u16,
        /// Its hue, or 0 for none.
        hue: u16,
        /// Where it lies.
        position: Point,
        /// Which facet.
        facet: u8,
    },
    /// A client double-clicked an object (`0x06`) — for now, to open a container.
    DoubleClick {
        /// Which connection.
        connection: ConnectionId,
        /// The object's serial.
        serial: u32,
    },
    /// A client asked to wear the item on its cursor (`0x13`).
    EquipItem {
        /// Which connection.
        connection: ConnectionId,
        /// The item to wear.
        item: u32,
        /// The layer to wear it on.
        layer: u8,
        /// The mobile to wear it — usually the player's own.
        mobile: u32,
    },
    /// A client asked to pick an item up onto its cursor (`0x07`).
    PickUpItem {
        /// Which connection.
        connection: ConnectionId,
        /// The item's serial.
        serial: u32,
        /// How many of a stack to lift. Ignored for now — the whole item is
        /// lifted; splitting a pile is a stacking concern (§6 items).
        amount: u16,
    },
    /// A client asked to put the item on its cursor down (`0x08`).
    DropItem {
        /// Which connection.
        connection: ConnectionId,
        /// The item's serial, as the client names it.
        serial: u32,
        /// Where, for a ground drop.
        position: Point,
        /// Where it is going: a container serial, a mobile to equip on, or
        /// [`DROP_TO_GROUND`](openshard_protocol::DROP_TO_GROUND). Only ground
        /// drops are handled yet; anything else is bounced.
        container: u32,
    },
    /// A connection went away.
    Disconnect {
        /// Which connection.
        connection: ConnectionId,
    },
}

/// Everything [`World::enter`] needs: who is entering, and as what. A plain
/// bundle so the one function that puts a character in the world takes one
/// argument instead of seven.
struct Entering {
    connection: ConnectionId,
    version: ClientVersion,
    account: String,
    name: String,
    serial: Option<u32>,
    position: Option<Point>,
    facet: u8,
    appearance: Option<Appearance>,
}

/// Bytes for a connection, produced by a tick.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Outbound {
    /// Who to send to.
    pub connection: ConnectionId,
    /// What to send.
    pub packet: Vec<u8>,
}

/// One facet: its ground, and who is near what on it.
///
/// The world keeps one of these per loaded facet. Two mobiles on different
/// facets never share a sector grid, so they never see each other and never
/// block each other — the isolation is a property of the data structure, not a
/// check anyone has to remember to write.
struct FacetState {
    terrain: Option<MapTerrain>,
    sectors: Sectors,
}

/// An item on a cursor: the entity, and where it was lifted from.
///
/// The origin is the whole reason to remember more than the entity. A drag that
/// is refused — dropped out of reach, into nothing — has to put the item back
/// exactly where it was, and by then it is off the ground (and out of any
/// container) with no place of its own to return to.
#[derive(Clone, Copy, Debug)]
struct HeldItem {
    entity: EntityId,
    origin: Origin,
}

/// Where a held item came from, so a cancelled drag can put it back.
#[derive(Clone, Copy, Debug)]
enum Origin {
    /// It was on the ground.
    Ground { position: Point, facet: u8 },
    /// It was inside a container.
    Container(Contained),
    /// It was worn by a mobile.
    Worn(Equipped),
}

/// The world.
///
/// Owns the registry, the bus and the map. A plain value: nothing here is a
/// static, and a test builds as many as it likes.
pub struct World {
    registry: Registry,
    bus: EventBus,
    /// The loaded facets, each with its own ground and interest grid, keyed by
    /// facet number. There is always at least the default one.
    facets: BTreeMap<u8, FacetState>,
    /// The facet a new character spawns on, and the one the world falls back to
    /// for anything asking for a facet it does not have.
    default_facet: u8,
    /// Which entity a connection is driving.
    players: HashMap<ConnectionId, EntityId>,
    /// What each player's client currently has on screen.
    ///
    /// The server has to remember, because the client never says. There is no
    /// "what can you see" packet — only "draw this" and "forget that" — so the
    /// only way to send a mobile exactly once is to know what was sent before.
    seen: HashMap<EntityId, HashSet<EntityId>>,
    /// The item each connection is dragging on its cursor, and where it was so a
    /// cancelled drag can put it back. An item here is off the ground and out of
    /// everyone's [`seen`](Self::seen) — in limbo until a `0x08` lands it.
    held: HashMap<ConnectionId, HeldItem>,
    /// Where new characters appear. The height comes from the map.
    start: (u16, u16),
    /// What has changed since the last save.
    journal: Journal,
    /// How often to offer a snapshot, in ticks. Zero never saves.
    save_every: u64,
    /// Snapshots the tick has taken and nobody has collected yet.
    saves: Vec<Snapshot>,
    /// Read to find out what to mark dirty. See `mark_dirty`.
    entered: Cursor<PlayerEntered>,
    /// Read to find out what to mark dirty. See `mark_dirty`.
    moved: Cursor<MobileMoved>,
    /// Read to find out what to mark dirty. See `mark_dirty`.
    turned: Cursor<MobileTurned>,
    /// Commands waiting for the next tick.
    inbox: Vec<Command>,
    /// Packets the last tick produced.
    outbox: Vec<Outbound>,
    /// How many ticks have run.
    ticks: u64,
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World")
            .field("ticks", &self.ticks)
            .field("entities", &self.registry.len())
            .field("players", &self.players.len())
            .field("facets", &self.facets.len())
            .finish()
    }
}

impl World {
    /// An empty world with no map, spawning at `start`.
    pub fn new(start: (u16, u16)) -> Self {
        // Always at least the default facet, so there is somewhere to stand even
        // with no map loaded — the same no-map mode the shard has always had.
        let mut facets = BTreeMap::new();
        facets.insert(
            DEFAULT_FACET,
            FacetState {
                terrain: None,
                sectors: Sectors::new(FACET_WITHOUT_A_MAP.0, FACET_WITHOUT_A_MAP.1),
            },
        );
        Self {
            registry: Registry::new(),
            bus: EventBus::new(),
            facets,
            default_facet: DEFAULT_FACET,
            players: HashMap::new(),
            seen: HashMap::new(),
            held: HashMap::new(),
            start,
            journal: Journal::new(),
            save_every: SAVE_EVERY_TICKS,
            saves: Vec::new(),
            entered: Cursor::default(),
            moved: Cursor::default(),
            turned: Cursor::default(),
            inbox: Vec::new(),
            outbox: Vec::new(),
            ticks: 0,
        }
    }

    /// How often to offer a snapshot, in ticks. Zero never saves.
    ///
    /// Zero is a real mode and not a broken one: the shard already runs with no
    /// map, and running with nothing to save to is the same bargain. What it
    /// must not do is pretend — a world with nowhere to write is a world that
    /// says so, not one that keeps a journal nobody ever collects.
    pub const fn with_save_every(mut self, ticks: u64) -> Self {
        self.save_every = ticks;
        self
    }

    /// Give the default facet a map.
    pub fn with_terrain(self, terrain: MapTerrain) -> Self {
        let facet = self.default_facet;
        self.with_facet(facet, terrain)
    }

    /// Load `terrain` as facet `facet`, its interest grid sized to the map.
    pub fn with_facet(mut self, facet: u8, terrain: MapTerrain) -> Self {
        let sectors = Sectors::new(terrain.map().width(), terrain.map().height());
        self.facets.insert(
            facet,
            FacetState {
                terrain: Some(terrain),
                sectors,
            },
        );
        self
    }

    /// The default facet's spatial index.
    pub fn sectors(&self) -> &Sectors {
        &self.facets[&self.default_facet].sectors
    }

    /// The event bus, for reading what happened.
    pub const fn bus(&self) -> &EventBus {
        &self.bus
    }

    /// Everything in the world.
    pub const fn registry(&self) -> &Registry {
        &self.registry
    }

    /// How many ticks have run.
    pub const fn ticks(&self) -> u64 {
        self.ticks
    }

    /// How many people are in the world.
    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    /// Queue a command for the next tick.
    ///
    /// Never acts immediately. That is the whole point: a command that took
    /// effect the moment it arrived would run world code on a network thread at
    /// an arbitrary point in the tick, and two clients racing would produce a
    /// different world depending on which packet won.
    pub fn queue(&mut self, command: Command) {
        self.inbox.push(command);
    }

    /// Take the packets the last tick produced.
    pub fn drain_outbound(&mut self) -> std::vec::Drain<'_, Outbound> {
        self.outbox.drain(..)
    }

    /// Take the snapshots the tick has offered to persistence.
    ///
    /// The same shape as [`drain_outbound`](Self::drain_outbound), and for the
    /// same reason: the world produces owned values and never waits for anyone
    /// to take them. What the caller does with a snapshot — write it, queue it,
    /// drop it — is not the tick's problem, and the tick is not slower for the
    /// answer being "write it to a disk in Frankfurt".
    pub fn drain_saves(&mut self) -> std::vec::Drain<'_, Snapshot> {
        self.saves.drain(..)
    }

    /// How many entities are waiting to be saved.
    pub fn unsaved(&self) -> usize {
        self.journal.len()
    }

    /// Mark everything as needing saving, whatever the tracking thinks.
    ///
    /// # This is what a failed save costs
    ///
    /// The precise answer is to remember which entities were in the snapshot
    /// that failed and mark those. The reason not to is that it means the world
    /// tracking in-flight writes — a map of tick to entities, a message back per
    /// success, and a leak the first time a store neither succeeds nor fails.
    /// That is real bookkeeping on the common path to make the rare path cheap.
    ///
    /// So the rare path is expensive instead: a save that failed makes the next
    /// one a full sweep. It is more rows than necessary, it is always correct
    /// whatever was lost, and it costs nothing at all when nothing goes wrong.
    ///
    /// Also for shutdown, where "everything" is the only right answer.
    pub fn resweep(&mut self) {
        let characters: Vec<EntityId> = self
            .registry
            .query::<Name>()
            .map(|(entity, _)| entity)
            .collect();
        self.journal.touch_all(characters);
    }

    /// Run one tick.
    ///
    /// `now` is a parameter, like everywhere else on this path: a tick that read
    /// the clock could not be replayed, and a simulation that cannot be replayed
    /// cannot be debugged from a log.
    pub fn tick(&mut self, now: Instant) {
        self.ticks += 1;

        // Take the whole inbox. A command queued *during* a tick belongs to the
        // next one — otherwise a system that queues work could starve the loop,
        // and the tick's length would depend on what happened in it.
        let commands = std::mem::take(&mut self.inbox);
        for command in commands {
            self.apply(command, now);
        }

        // Rot away what has lain on the ground too long. After the commands, so
        // an item dropped this tick is not aged by the same tick that placed it.
        self.decay();

        // Before the bus retires anything: what happened is what needs saving,
        // and reading it after `update` would read it a tick late.
        self.mark_dirty();
        self.offer_snapshot();

        // Retire the oldest events. Once per tick, after every system, so that
        // "one tick" means the same thing for every event type.
        self.bus.update();
    }

    // -- persistence -------------------------------------------------------

    /// Mark what changed, from what the tick said happened.
    ///
    /// # Why this reads the bus instead of being called from each mutation
    ///
    /// The obvious version is a `journal.touch(entity)` next to every
    /// `registry.insert`. It works, and it decays: the day someone adds a system
    /// that moves a mobile — a teleport, a knockback, a script — they have to
    /// know that persistence exists and remember a line that nothing will fail
    /// without. The bug is silent, it survives every test that does not restart
    /// the shard, and it looks like the disk lost something.
    ///
    /// Emitting the event *is* the touch. A system that moves a mobile already
    /// has to say so, because that is how the client hears about it, and the
    /// same event now also means "and write it down". There is nothing left to
    /// forget.
    fn mark_dirty(&mut self) {
        // Collected first: `read` borrows the bus, and the journal is a
        // different field but the iterator holds the borrow across the loop.
        let mut changed: Vec<EntityId> = Vec::new();
        changed.extend(self.bus.read(&mut self.entered).map(|event| event.entity));
        changed.extend(self.bus.read(&mut self.moved).map(|event| event.entity));
        changed.extend(self.bus.read(&mut self.turned).map(|event| event.entity));
        for entity in changed {
            self.journal.touch(entity);
        }
    }

    /// Every `save_every` ticks, hand what changed to whoever is collecting.
    fn offer_snapshot(&mut self) {
        if self.save_every == 0 || !self.ticks.is_multiple_of(self.save_every) {
            return;
        }
        self.take_snapshot();
    }

    /// Take a snapshot now, whatever the cadence says.
    ///
    /// For shutdown, for a GM save command, and for tests that would rather not
    /// tick four hundred times to see one row.
    pub fn take_snapshot(&mut self) {
        let ticks = self.ticks;
        // The borrow split: `drain` needs the journal mutably and the closure
        // needs the registry, so the registry is taken out of `self` by
        // reference first. The closure is called inside the tick and reads
        // memory only — this is the "consistent picture at one instant" the
        // snapshot promises.
        let registry = &self.registry;
        let snapshot = self
            .journal
            .drain(ticks, |entity| Self::record_of(registry, entity));
        if let Some(snapshot) = snapshot {
            debug!(tick = ticks, rows = snapshot.len(), "snapshot taken");
            self.saves.push(snapshot);
        }
    }

    /// What a character looks like on disk.
    ///
    /// `None` for anything that is not a character, which is not an error: the
    /// journal tracks entities and the world will hold more than people.
    fn record_of(registry: &Registry, entity: EntityId) -> Option<CharacterRecord> {
        let serial = registry.serial_of(entity)?;
        let position = registry.get::<Position>(entity)?.0;
        let heading = registry.get::<Heading>(entity)?.0;
        let body = registry.get::<Body>(entity)?;
        let name = registry.get::<Name>(entity)?;
        // No account means this is not a player character — an NPC, say — so it
        // is not a `CharacterRecord`. Returning `None` drops it from the save,
        // which is the honest answer.
        let account = registry.get::<Account>(entity)?;
        let facet = registry.get::<Facet>(entity).map_or(DEFAULT_FACET, |f| f.0);
        Some(CharacterRecord {
            serial: serial.raw(),
            account: account.0.clone(),
            name: name.0.clone(),
            body: body.id,
            hue: body.hue,
            facet,
            x: position.x,
            y: position.y,
            z: position.z,
            facing: heading.to_bits(),
        })
    }

    /// Reserve a serial read from persistence so a fresh spawn never takes it.
    ///
    /// A logged-out character is not in the world — it is a row in the database —
    /// but its serial is still spoken for. Call this at boot for every stored
    /// character, before anyone can create a new one. Values outside the serial
    /// range are ignored: a corrupt row should not stop the shard from starting.
    pub fn reserve_serial(&mut self, raw: u32) {
        if let Some(serial) = Serial::new(raw) {
            self.registry.reserve_serial(serial);
        }
    }

    fn apply(&mut self, command: Command, now: Instant) {
        match command {
            Command::Enter {
                connection,
                version,
                account,
                name,
                serial,
                position,
                facet,
                appearance,
            } => self.enter(Entering {
                connection,
                version,
                account,
                name,
                serial,
                position,
                facet,
                appearance,
            }),
            Command::Walk {
                connection,
                request,
            } => self.walk(connection, request, now),
            Command::Step { serial, direction } => self.step(serial, direction),
            Command::SpawnItem {
                graphic,
                hue,
                amount,
                stackable,
                position,
                facet,
            } => {
                self.spawn_item(graphic, hue, amount, stackable, position, facet);
            }
            Command::SpawnContainer {
                graphic,
                gump,
                hue,
                position,
                facet,
            } => self.spawn_container(graphic, gump, hue, position, facet),
            Command::DoubleClick { connection, serial } => self.double_click(connection, serial),
            Command::EquipItem {
                connection,
                item,
                layer,
                mobile,
            } => self.equip_item(connection, item, layer, mobile),
            Command::PickUpItem {
                connection,
                serial,
                amount,
            } => self.pick_up(connection, serial, amount),
            Command::DropItem {
                connection,
                serial,
                position,
                container,
            } => self.drop_item(connection, serial, position, container),
            Command::Disconnect { connection } => self.disconnect(connection),
        }
    }

    /// The facet a mobile is on, or the default if it carries none.
    ///
    /// Always a facet the world actually has: [`enter`](Self::enter) clamps an
    /// unloaded facet to the default before it ever reaches a `Facet` component,
    /// so callers can index `self.facets` with the result.
    fn facet_of(&self, entity: EntityId) -> u8 {
        self.registry
            .get::<Facet>(entity)
            .map_or(self.default_facet, |facet| facet.0)
    }

    /// The state of a facet the world is known to have.
    fn facet_state(&self, facet: u8) -> &FacetState {
        &self.facets[&facet]
    }

    /// The same, mutably. Panics only on a facet no entity should carry —
    /// `facet_of` and `enter` keep every live entity on a loaded facet.
    fn facet_state_mut(&mut self, facet: u8) -> &mut FacetState {
        self.facets
            .get_mut(&facet)
            .expect("an entity's facet is always loaded")
    }

    /// Where a character appears on `facet`: the configured x and y, at that
    /// facet's height.
    ///
    /// The `z` is read from the map rather than configured. A second source of
    /// truth that disagrees by three units leaves a character unable to take a
    /// single step — every one is more than a two-unit climb — with nothing in
    /// the log to explain it.
    fn start_position(&self, facet: u8) -> Point {
        let (x, y) = self.start;
        let z = self
            .facets
            .get(&facet)
            .and_then(|state| state.terrain.as_ref())
            .and_then(|terrain| terrain.map().land(x, y))
            .map_or(Z_WITHOUT_A_MAP, |cell| cell.z);
        Point::new(x, y, z)
    }

    fn enter(&mut self, entering: Entering) {
        let Entering {
            connection,
            version,
            account,
            name,
            serial,
            position,
            facet,
            appearance,
        } = entering;
        if self.players.contains_key(&connection) {
            warn!(%connection, "already in the world");
            return;
        }

        // A character can only stand on a facet the shard loaded. An unloaded one
        // — a save from a shard that had more facets, say — falls back to the
        // default rather than leaving the character nowhere.
        let facet = if self.facets.contains_key(&facet) {
            facet
        } else {
            warn!(%connection, facet, "unloaded facet; falling back to the default");
            self.default_facet
        };

        // A stored character comes back on the serial it was saved under; a new
        // one takes a fresh serial from the pool. The saved serial was reserved
        // at boot (see `World::reserve_serial`), so binding it here cannot collide.
        let (entity, serial) = match serial.and_then(Serial::new) {
            Some(saved) => {
                let entity = self.registry.spawn();
                if let Err(error) = self.registry.bind_serial(entity, saved) {
                    warn!(%connection, ?error, "could not restore the saved serial");
                    self.registry.despawn(entity);
                    return;
                }
                (entity, saved)
            }
            None => match self.registry.spawn_with_serial(SerialKind::Mobile) {
                Ok(pair) => pair,
                Err(_) => {
                    warn!(%connection, "the mobile serial pool is exhausted");
                    return;
                }
            },
        };

        // A loaded character spawns exactly where it was saved, its own z
        // included; a fresh one takes the world's configured start on its facet.
        let position = position.unwrap_or_else(|| self.start_position(facet));
        let facing = Facing::walking(Direction::South);
        // A created or loaded character brings its body and hue; without one it
        // falls back to the default.
        let body = Body {
            id: appearance.map_or(BODY_HUMAN_MALE, |look| look.body),
            hue: appearance.map_or(DEFAULT_HUE, |look| look.hue),
        };

        self.registry.insert(entity, Position(position));
        self.registry.insert(entity, Heading(facing));
        self.registry.insert(entity, body);
        self.registry.insert(entity, Name(name.clone()));
        self.registry.insert(entity, Account(account));
        self.registry.insert(entity, Facet(facet));
        self.registry
            .insert(entity, Movement(Walker::new(position, facing)));
        self.registry.insert(
            entity,
            Client {
                connection,
                version,
            },
        );
        self.players.insert(connection, entity);
        self.facet_state_mut(facet).sectors.insert(entity, position);
        self.seen.insert(entity, HashSet::new());

        // The order is the client's, not ours. 0x1B must come first — until it
        // lands there is no body to attach anything to — and 0x55 must come
        // last, because it is what tells the client to start drawing. What is
        // between can be reordered; the two ends cannot.
        self.send(
            connection,
            PlayerStart {
                serial: serial.raw(),
                body: body.id,
                position,
                facing,
                map_width: DEFAULT_MAP_WIDTH,
                map_height: DEFAULT_MAP_HEIGHT,
            }
            .encode(),
        );
        self.send(connection, encode_map_change(facet));
        self.send(
            connection,
            PlayerUpdate {
                serial: serial.raw(),
                body: body.id,
                hue: body.hue,
                flags: 0,
                position,
                facing,
            }
            .encode(),
        );
        self.send(connection, encode_light_level(LIGHT_DAY));
        self.send(connection, encode_login_complete());

        self.bus.send(PlayerEntered {
            entity,
            serial,
            position,
        });
        info!(%serial, name, position = %position, "in world");

        // Draw whoever is already here, and draw this one for them. Both
        // directions, because arriving is symmetric: the newcomer has an empty
        // screen and everyone nearby has a gap where it now stands.
        self.refresh_around(entity);
    }

    fn walk(&mut self, connection: ConnectionId, request: WalkRequest, now: Instant) {
        let Some(&entity) = self.players.get(&connection) else {
            // A walk before a character. Not fatal — a stray packet from a
            // client that reconnected — but nothing to act on either.
            debug!(%connection, "0x02 from a connection with no character");
            return;
        };
        let Some(serial) = self.registry.serial_of(entity) else {
            return;
        };
        let Some(Movement(mut walker)) = self.registry.get::<Movement>(entity).copied() else {
            return;
        };

        let facet = self.facet_of(entity);
        let was = walker.position;
        let out_of_sequence = walker.sequence.is_fresh() && request.sequence != 0;
        let outcome = match &self.facet_state(facet).terrain {
            Some(terrain) => walker.request(request, terrain, now),
            None => walker.request(request, &OpenWorld, now),
        };
        self.registry.insert(entity, Movement(walker));

        match outcome {
            Walk::Moved { position, facing } => {
                self.registry.insert(entity, Position(position));
                self.registry.insert(entity, Heading(facing));
                // The index is a second copy of the position; this is the line
                // that keeps it honest.
                self.facet_state_mut(facet).sectors.insert(entity, position);
                self.send(
                    connection,
                    encode_walk_ack(request.sequence, NOTORIETY_INNOCENT),
                );
                self.bus.send(MobileMoved {
                    entity,
                    serial,
                    from: was,
                    to: position,
                    facing,
                });
                self.refresh_around(entity);
            }
            Walk::Turned { facing } => {
                self.registry.insert(entity, Heading(facing));
                self.send(
                    connection,
                    encode_walk_ack(request.sequence, NOTORIETY_INNOCENT),
                );
                self.bus.send(MobileTurned {
                    entity,
                    serial,
                    facing,
                });
                // A turn moves nobody, but it changes what everyone watching
                // draws — the client animates a facing it is told about.
                self.broadcast_move(entity);
            }
            Walk::Refused => {
                // Which of the three it was is not something `Walk` says, and
                // teaching it to would put the reasons in the wrong crate. The
                // sequence is checked before anything else, so a fresh walker
                // with a non-zero sequence can only have failed that; past it,
                // the pace and the terrain are the two left and this cannot yet
                // tell them apart. Better a coarse reason than a wrong one.
                let reason = if out_of_sequence {
                    RefusedReason::OutOfSequence
                } else {
                    RefusedReason::Blocked
                };
                self.send(
                    connection,
                    encode_walk_reject(request.sequence, walker.position, walker.facing),
                );
                self.bus.send(StepRefused {
                    entity,
                    serial,
                    reason,
                });
                debug!(%serial, ?reason, "step refused");
            }
        }
    }

    /// Move a mobile one step by server decree. See [`Command::Step`].
    ///
    /// Shares the interest-management tail with [`walk`](Self::walk) —
    /// [`refresh_around`](Self::refresh_around) and
    /// [`broadcast_move`](Self::broadcast_move) — because a mobile the server
    /// moved has to appear on the same screens, and leave the same ones, as a
    /// mobile that walked itself. What it does not share is the client half:
    /// there is no `0x22`/`0x21` ack, because there may be no client, and the
    /// mobile might be an NPC nobody is driving.
    fn step(&mut self, serial: u32, direction: u8) {
        let Some(serial) = Serial::new(serial) else {
            return;
        };
        let Some(entity) = self.registry.entity_of(serial) else {
            return;
        };
        let Some(Movement(mut walker)) = self.registry.get::<Movement>(entity).copied() else {
            return;
        };
        let direction = Direction::from_bits(direction);
        let facet = self.facet_of(entity);
        let was = walker.position;

        // Turn-as-step: a mobile not yet facing this way turns and stays put.
        if walker.facing.direction != direction {
            let facing = Facing::walking(direction);
            walker.facing = facing;
            self.registry.insert(entity, Movement(walker));
            self.registry.insert(entity, Heading(facing));
            self.bus.send(MobileTurned {
                entity,
                serial,
                facing,
            });
            self.broadcast_move(entity);
            return;
        }

        let Some(target) = step_from(walker.position, direction) else {
            // Off the edge of the coordinate space — nowhere to go, and no client
            // to snap back, so it is simply refused.
            self.bus.send(StepRefused {
                entity,
                serial,
                reason: RefusedReason::Blocked,
            });
            return;
        };
        let landed = match &self.facet_state(facet).terrain {
            Some(terrain) => terrain.can_step(walker.position, target),
            None => OpenWorld.can_step(walker.position, target),
        };
        let Some(landed) = landed else {
            self.bus.send(StepRefused {
                entity,
                serial,
                reason: RefusedReason::Blocked,
            });
            return;
        };

        let facing = Facing::walking(direction);
        walker.position = landed;
        walker.facing = facing;
        self.registry.insert(entity, Movement(walker));
        self.registry.insert(entity, Position(landed));
        self.registry.insert(entity, Heading(facing));
        self.facet_state_mut(facet).sectors.insert(entity, landed);
        self.bus.send(MobileMoved {
            entity,
            serial,
            from: was,
            to: landed,
            facing,
        });
        self.refresh_around(entity);
    }

    /// Put an item on the ground. See [`Command::SpawnItem`].
    ///
    /// Returns the entity so [`spawn_container`](Self::spawn_container) can make
    /// the same thing and then say it holds others.
    fn spawn_item(
        &mut self,
        graphic: u16,
        hue: u16,
        amount: u16,
        stackable: bool,
        position: Point,
        facet: u8,
    ) -> Option<EntityId> {
        let facet = if self.facets.contains_key(&facet) {
            facet
        } else {
            warn!(facet, "unloaded facet; spawning the item on the default");
            self.default_facet
        };
        let (entity, serial) = match self.registry.spawn_with_serial(SerialKind::Item) {
            Ok(pair) => pair,
            Err(error) => {
                warn!(?error, "out of item serials; not spawning");
                return None;
            }
        };
        self.registry.insert(entity, Graphic { id: graphic, hue });
        self.registry.insert(entity, Position(position));
        self.registry.insert(entity, Facet(facet));
        // Only a real stack carries an amount; a single item stays a bare graphic.
        if amount > 1 {
            self.registry.insert(entity, Amount(amount));
        }
        if stackable {
            self.registry.insert(entity, Stackable);
        }
        self.mark_decay(entity);
        self.facet_state_mut(facet).sectors.insert(entity, position);
        self.bus.send(ItemSpawned {
            entity,
            serial,
            position,
        });
        self.reveal_item(entity);
        debug!(%serial, graphic, position = %position, "item on the ground");
        Some(entity)
    }

    /// Put a container on the ground. See [`Command::SpawnContainer`].
    ///
    /// A container is an ordinary ground item that also carries a [`Container`],
    /// which is the only thing that makes it openable. So it is spawned exactly
    /// like one and then marked.
    fn spawn_container(&mut self, graphic: u16, gump: u16, hue: u16, position: Point, facet: u8) {
        if let Some(entity) = self.spawn_item(graphic, hue, 1, false, position, facet) {
            self.registry.insert(entity, Container { gump });
            // A container does not rot with its contents inside it; only loose
            // ground clutter decays.
            self.registry.remove::<Decays>(entity);
        }
    }

    /// Set an item's decay clock: it rots [`DECAY_TICKS`] from now. Every item on
    /// the ground has one; every item off it has none.
    fn mark_decay(&mut self, item: EntityId) {
        self.registry.insert(
            item,
            Decays {
                at_tick: self.ticks + DECAY_TICKS,
            },
        );
    }

    /// Open a container onto a client's screen. See [`Command::DoubleClick`].
    ///
    /// Only containers do anything yet — a double-click on anything else is
    /// ignored rather than answered, because "use" for a door or a food is a
    /// later rule and a wrong guess is worse than silence.
    fn double_click(&mut self, connection: ConnectionId, serial: u32) {
        let Some(&player) = self.players.get(&connection) else {
            return;
        };
        let Some(item_serial) = Serial::new(serial) else {
            return;
        };
        let Some(item) = self.registry.entity_of(item_serial) else {
            return;
        };
        let Some(&Container { gump }) = self.registry.get::<Container>(item) else {
            return;
        };
        // The container has to be in reach on the ground. Nesting — opening one
        // out of another already open — is a later refinement.
        let Some(&Position(item_pos)) = self.registry.get::<Position>(item) else {
            return;
        };
        let Some(&Position(player_pos)) = self.registry.get::<Position>(player) else {
            return;
        };
        if self.facet_of(item) != self.facet_of(player)
            || !in_range(item_pos, player_pos, ITEM_REACH)
        {
            return;
        }
        let Some(&Client { version, .. }) = self.registry.get::<Client>(player) else {
            return;
        };

        let contents = self.contents_of(item_serial);
        self.send(connection, encode_open_container(serial, gump, version));
        self.send(
            connection,
            encode_container_contents(serial, &contents, version),
        );
        debug!(%item_serial, items = contents.len(), "container opened");
    }

    /// Everything inside a container, as the wire records `0x3C`/`0x25` need.
    fn contents_of(&self, container: Serial) -> Vec<ContainedItem> {
        self.registry
            .query::<Contained>()
            .filter(|(_, held)| held.container == container)
            .filter_map(|(entity, _)| self.contained_record(entity))
            .collect()
    }

    /// How many items a container already holds — the next free grid slot.
    fn item_count(&self, container: Serial) -> u8 {
        self.registry
            .query::<Contained>()
            .filter(|(_, held)| held.container == container)
            .count()
            .min(u8::MAX as usize) as u8
    }

    /// Wear a client's held item on a mobile. See [`Command::EquipItem`].
    fn equip_item(&mut self, connection: ConnectionId, item: u32, layer: u8, mobile: u32) {
        // Equipping is a *drop* of the dragged item, so there has to be one, and
        // it has to be the item named.
        let Some(held) = self.held.get(&connection).copied() else {
            return;
        };
        if self.registry.serial_of(held.entity) != Serial::new(item) {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        }
        if layer == 0 || layer > MAX_WEARABLE_LAYER {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        }
        let (Some(wearer_serial), Some(wearer)) = (
            Serial::new(mobile),
            Serial::new(mobile).and_then(|s| self.registry.entity_of(s)),
        ) else {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        };
        // Only a mobile wears things, and only within reach of the player.
        let Some(&player) = self.players.get(&connection) else {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        };
        let (Some(&Position(wearer_pos)), Some(&Position(player_pos))) = (
            self.registry.get::<Position>(wearer),
            self.registry.get::<Position>(player),
        ) else {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        };
        if !self.registry.has::<Body>(wearer) {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        }
        if self.facet_of(wearer) != self.facet_of(player)
            || !in_range(wearer_pos, player_pos, ITEM_REACH)
        {
            self.bounce(connection, held, DragCancelReason::OutOfRange);
            return;
        }
        // A layer holds one thing.
        if self.layer_taken(wearer_serial, layer) {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        }

        self.held.remove(&connection);
        self.registry.insert(
            held.entity,
            Equipped {
                mobile: wearer_serial,
                layer,
            },
        );
        self.broadcast_equip(held.entity, wearer);
        debug!(item, layer, "equipped");
    }

    /// Whether a mobile already wears something on a layer.
    fn layer_taken(&self, mobile: Serial, layer: u8) -> bool {
        self.registry
            .query::<Equipped>()
            .any(|(_, worn)| worn.mobile == mobile && worn.layer == layer)
    }

    /// Tell everyone who can see `mobile`, and the mobile itself if it is a
    /// player, that it is now wearing `item` — a `0x2E` each.
    fn broadcast_equip(&mut self, item: EntityId, mobile: EntityId) {
        let Some(packet) = self.equip_packet(item) else {
            return;
        };
        for watcher in self.equip_audience(mobile) {
            if let Some(&Client { connection, .. }) = self.registry.get::<Client>(watcher) {
                self.outbox.push(Outbound {
                    connection,
                    packet: packet.clone(),
                });
            }
        }
    }

    /// Everyone who should hear about a change to `mobile`'s outfit: those who
    /// can see it, and the mobile itself.
    fn equip_audience(&self, mobile: EntityId) -> Vec<EntityId> {
        let mut audience = self.watchers_of(mobile);
        audience.push(mobile);
        audience
    }

    /// Build the `0x2E` for a worn item.
    fn equip_packet(&self, item: EntityId) -> Option<Vec<u8>> {
        let serial = self.registry.serial_of(item)?;
        let Equipped { mobile, layer } = *self.registry.get::<Equipped>(item)?;
        let Graphic { id, hue } = *self.registry.get::<Graphic>(item)?;
        Some(encode_equip(serial.raw(), id, layer, mobile.raw(), hue))
    }

    /// Draw a freshly placed item for every player who can see its tile.
    ///
    /// The item's own half of [`refresh_around`](Self::refresh_around): an item
    /// does not move, so it never runs that, but the players around it still need
    /// telling once, when it appears.
    fn reveal_item(&mut self, item: EntityId) {
        let facet = self.facet_of(item);
        let sectors = &self.facet_state(facet).sectors;
        let Some(centre) = sectors.position_of(item) else {
            return;
        };
        let watchers: Vec<EntityId> = sectors
            .nearby(centre, VIEW_RANGE)
            .map(|(id, _)| id)
            .filter(|id| *id != item)
            .collect();
        for watcher in watchers {
            self.show(watcher, item);
        }
    }

    /// Lift an item onto a client's cursor. See [`Command::PickUpItem`].
    fn pick_up(&mut self, connection: ConnectionId, serial: u32, _amount: u16) {
        let Some(&player) = self.players.get(&connection) else {
            return;
        };
        if self.held.contains_key(&connection) {
            self.reject_drag(connection, DragCancelReason::AlreadyHolding);
            return;
        }
        let Some(item_serial) = Serial::new(serial) else {
            self.reject_drag(connection, DragCancelReason::CannotLift);
            return;
        };
        let Some(item) = self.registry.entity_of(item_serial) else {
            self.reject_drag(connection, DragCancelReason::CannotLift);
            return;
        };
        // Only a thing with a graphic is an item. A mobile has none, so this
        // rejects trying to pick up a person.
        if !self.registry.has::<Graphic>(item) {
            self.reject_drag(connection, DragCancelReason::CannotLift);
            return;
        }

        // Where it is now decides how it is lifted and where a cancelled drag
        // will put it back.
        if let Some(&Position(item_pos)) = self.registry.get::<Position>(item) {
            let Some(&Position(player_pos)) = self.registry.get::<Position>(player) else {
                return;
            };
            let facet = self.facet_of(item);
            if facet != self.facet_of(player) || !in_range(item_pos, player_pos, ITEM_REACH) {
                self.reject_drag(connection, DragCancelReason::OutOfRange);
                return;
            }
            // Off the sector grid, off every screen but the picker's — whose own
            // client already put it on the cursor, so a 0x1D there would fight it.
            self.facet_state_mut(facet).sectors.remove(item);
            for watcher in self.watchers_of(item) {
                if watcher == player {
                    if let Some(seen) = self.seen.get_mut(&player) {
                        seen.remove(&item);
                    }
                } else {
                    self.forget(watcher, item, item_serial);
                }
            }
            self.registry.remove::<Position>(item);
            // Off the ground, off the decay clock.
            self.registry.remove::<Decays>(item);
            self.held.insert(
                connection,
                HeldItem {
                    entity: item,
                    origin: Origin::Ground {
                        position: item_pos,
                        facet,
                    },
                },
            );
        } else if let Some(&contained) = self.registry.get::<Contained>(item) {
            // Out of a container. The client with the gump open removes it from
            // the gump itself; the server just drops the containment.
            self.registry.remove::<Contained>(item);
            self.held.insert(
                connection,
                HeldItem {
                    entity: item,
                    origin: Origin::Container(contained),
                },
            );
        } else if let Some(&worn) = self.registry.get::<Equipped>(item) {
            // Off a mobile. The picker's own client drags it off the paperdoll;
            // everyone else watching the mobile is told to forget it, because
            // they knew it only as part of that mobile.
            self.registry.remove::<Equipped>(item);
            if let Some(mobile) = self.registry.entity_of(worn.mobile) {
                for watcher in self.equip_audience(mobile) {
                    if watcher == player {
                        continue;
                    }
                    if let Some(&Client { connection: to, .. }) =
                        self.registry.get::<Client>(watcher)
                    {
                        self.outbox.push(Outbound {
                            connection: to,
                            packet: encode_remove(item_serial.raw()),
                        });
                    }
                }
            }
            self.held.insert(
                connection,
                HeldItem {
                    entity: item,
                    origin: Origin::Worn(worn),
                },
            );
        } else {
            // Neither on the ground nor in a container: already on a cursor, or
            // nowhere. Nothing to lift.
            self.reject_drag(connection, DragCancelReason::CannotLift);
            return;
        }
        debug!(%item_serial, "lifted onto the cursor");
    }

    /// Put a client's held item down. See [`Command::DropItem`].
    fn drop_item(
        &mut self,
        connection: ConnectionId,
        serial: u32,
        position: Point,
        container: u32,
    ) {
        let Some(held) = self.held.get(&connection).copied() else {
            // Nothing on the cursor — a stray 0x08, nothing to bounce.
            return;
        };
        // The serial has to be the thing actually held; a mismatch is a confused
        // client, and the safe answer is to give it back what it was holding.
        if self.registry.serial_of(held.entity) != Serial::new(serial) {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        }

        if container != DROP_TO_GROUND {
            self.drop_onto_item(connection, held, position, container);
            return;
        }

        // Onto the ground: within reach of the player, on the player's facet.
        let Some(&player) = self.players.get(&connection) else {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        };
        let Some(&Position(player_pos)) = self.registry.get::<Position>(player) else {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        };
        if !in_range(position, player_pos, ITEM_REACH) {
            self.bounce(connection, held, DragCancelReason::OutOfRange);
            return;
        }

        self.held.remove(&connection);
        self.place_on_ground(held.entity, position, self.facet_of(player));
        debug!(serial, "dropped on the ground");
    }

    /// Put a held item into a container. See [`Command::DropItem`].
    fn drop_into_container(
        &mut self,
        connection: ConnectionId,
        held: HeldItem,
        position: Point,
        container: u32,
    ) {
        let Some(container_serial) = Serial::new(container) else {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        };
        let Some(container_entity) = self.registry.entity_of(container_serial) else {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        };
        if !self.registry.has::<Container>(container_entity) {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        }
        let Some(&player) = self.players.get(&connection) else {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        };
        // The container has to be a reachable one on the ground. Dropping into a
        // container that is itself inside another is a later refinement.
        let Some(&Position(container_pos)) = self.registry.get::<Position>(container_entity) else {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        };
        let Some(&Position(player_pos)) = self.registry.get::<Position>(player) else {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        };
        if self.facet_of(container_entity) != self.facet_of(player)
            || !in_range(container_pos, player_pos, ITEM_REACH)
        {
            self.bounce(connection, held, DragCancelReason::OutOfRange);
            return;
        }

        // In it goes. The drop's `x`/`y` are gump coordinates, not world tiles.
        let grid = self.item_count(container_serial);
        self.held.remove(&connection);
        self.registry.insert(
            held.entity,
            Contained {
                container: container_serial,
                x: position.x,
                y: position.y,
                grid,
            },
        );
        // Tell the client, whose gump is open, that the item is now inside.
        if let (Some(&Client { version, .. }), Some(record)) = (
            self.registry.get::<Client>(player),
            self.contained_record(held.entity),
        ) {
            self.send(
                connection,
                encode_add_to_container(record, container, version),
            );
        }
        debug!(container, "dropped into a container");
    }

    /// A drop onto another item: into it if it is a container, merged with it if
    /// it is an identical stack, refused otherwise.
    fn drop_onto_item(
        &mut self,
        connection: ConnectionId,
        held: HeldItem,
        position: Point,
        target_serial: u32,
    ) {
        let target = Serial::new(target_serial).and_then(|s| self.registry.entity_of(s));
        match target {
            Some(target) if self.registry.has::<Container>(target) => {
                self.drop_into_container(connection, held, position, target_serial);
            }
            Some(target) if self.can_stack(held.entity, target) => {
                self.merge_onto(connection, held, target);
            }
            _ => self.bounce(connection, held, DragCancelReason::Other),
        }
    }

    /// Whether two items are one pile waiting to happen: both stackable, same
    /// graphic and hue, and not the same entity.
    fn can_stack(&self, a: EntityId, b: EntityId) -> bool {
        a != b
            && self.registry.has::<Stackable>(a)
            && self.registry.has::<Stackable>(b)
            && self.registry.get::<Graphic>(a) == self.registry.get::<Graphic>(b)
    }

    /// Merge a held stack onto a stack on the ground. See [`can_stack`](Self::can_stack).
    fn merge_onto(&mut self, connection: ConnectionId, held: HeldItem, target: EntityId) {
        // Only ground stacks merge for now; merging onto a stack inside a
        // container is a later refinement, and until then it bounces.
        let Some(&Position(target_pos)) = self.registry.get::<Position>(target) else {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        };
        let Some(&player) = self.players.get(&connection) else {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        };
        let Some(&Position(player_pos)) = self.registry.get::<Position>(player) else {
            self.bounce(connection, held, DragCancelReason::Other);
            return;
        };
        if self.facet_of(target) != self.facet_of(player)
            || !in_range(target_pos, player_pos, ITEM_REACH)
        {
            self.bounce(connection, held, DragCancelReason::OutOfRange);
            return;
        }

        // Sum, clamped: a pile cannot count past what its amount word can hold.
        let total = self
            .amount_of(held.entity)
            .saturating_add(self.amount_of(target));
        self.registry.insert(target, Amount(total));
        self.held.remove(&connection);
        // The dragged stack is gone into the other; it was on a cursor, on
        // nobody's ground, so despawning it needs no packet.
        self.registry.despawn(held.entity);
        self.redraw_ground_item(target);
        debug!(total, "stacks merged");
    }

    /// How many an item is: its [`Amount`], or one if it has none.
    fn amount_of(&self, item: EntityId) -> u16 {
        self.registry.get::<Amount>(item).map_or(1, |a| a.0)
    }

    /// Re-send a ground item to everyone already watching it — for when its
    /// amount changed and the `seen` set would otherwise suppress the redraw.
    fn redraw_ground_item(&mut self, item: EntityId) {
        for watcher in self.watchers_of(item) {
            let Some(&Client {
                connection,
                version,
            }) = self.registry.get::<Client>(watcher)
            else {
                continue;
            };
            if let Some(packet) = self.draw_packet(item, version) {
                self.outbox.push(Outbound { connection, packet });
            }
        }
    }

    /// Remove every ground item whose decay tick has arrived. Runs each tick,
    /// against [`ticks`](Self::ticks), so it reads no clock.
    fn decay(&mut self) {
        let now = self.ticks;
        let expired: Vec<EntityId> = self
            .registry
            .query::<Decays>()
            .filter(|(_, decays)| decays.at_tick <= now)
            .map(|(entity, _)| entity)
            .collect();
        for item in expired {
            let Some(serial) = self.registry.serial_of(item) else {
                continue;
            };
            let facet = self.facet_of(item);
            for watcher in self.watchers_of(item) {
                self.forget(watcher, item, serial);
            }
            self.facet_state_mut(facet).sectors.remove(item);
            self.registry.despawn(item);
            debug!(%serial, "decayed");
        }
    }

    /// Put a held item back where it was lifted and tell the client the drag is
    /// off, so it stops showing the item on the cursor.
    fn bounce(&mut self, connection: ConnectionId, held: HeldItem, reason: DragCancelReason) {
        self.held.remove(&connection);
        self.restore(held);
        self.reject_drag(connection, reason);
    }

    /// Put a held item back exactly where it came from — the ground it lay on or
    /// the container it was in.
    fn restore(&mut self, held: HeldItem) {
        match held.origin {
            Origin::Ground { position, facet } => {
                self.place_on_ground(held.entity, position, facet);
            }
            Origin::Container(contained) => {
                self.registry.insert(held.entity, contained);
            }
            Origin::Worn(worn) => {
                self.registry.insert(held.entity, worn);
                // Back on the mobile, and back on every screen that shows it.
                if let Some(mobile) = self.registry.entity_of(worn.mobile) {
                    self.broadcast_equip(held.entity, mobile);
                }
            }
        }
    }

    /// Build the `0x25`/`0x3C` record for one contained item.
    fn contained_record(&self, entity: EntityId) -> Option<ContainedItem> {
        let serial = self.registry.serial_of(entity)?;
        let Contained { x, y, grid, .. } = *self.registry.get::<Contained>(entity)?;
        let Graphic { id, hue } = *self.registry.get::<Graphic>(entity)?;
        let amount = self.registry.get::<Amount>(entity).map_or(1, |a| a.0);
        Some(ContainedItem {
            serial: serial.raw(),
            graphic: id,
            amount,
            x,
            y,
            grid,
            hue,
        })
    }

    /// Send a `0x27`, cancelling whatever drag the client thinks it has.
    fn reject_drag(&mut self, connection: ConnectionId, reason: DragCancelReason) {
        self.send(connection, encode_drag_cancel(reason));
    }

    /// Land an item on the ground at `position` and draw it for everyone in range.
    fn place_on_ground(&mut self, item: EntityId, position: Point, facet: u8) {
        self.registry.insert(item, Position(position));
        self.registry.insert(item, Facet(facet));
        // Back on the ground, back on the decay clock.
        self.mark_decay(item);
        self.facet_state_mut(facet).sectors.insert(item, position);
        self.reveal_item(item);
    }

    fn disconnect(&mut self, connection: ConnectionId) {
        // A client that logs out mid-drag would otherwise leave its item nowhere —
        // off the ground and out of any container, on a cursor that is gone. Put
        // it back where it was.
        if let Some(held) = self.held.remove(&connection) {
            self.restore(held);
        }

        let Some(entity) = self.players.remove(&connection) else {
            return;
        };
        let serial = self.registry.serial_of(entity);
        let facet = self.facet_of(entity);

        // Save before despawning, and not by marking it dirty: a `touch` is a
        // promise to read the entity at the next save, and in a moment there
        // will be no entity to read. Logging out is when a save matters most —
        // it is the only moment a player's whole session is at stake — so the
        // record is taken at the one instant it still can be.
        if let Some(record) = Self::record_of(&self.registry, entity) {
            self.journal.keep(record);
        }

        // Take it off every screen *before* despawning: once the entity is gone
        // its serial is released and there is nothing left to tell anyone about.
        if let Some(serial) = serial {
            for watcher in self.watchers_of(entity) {
                self.forget(watcher, entity, serial);
            }
        }
        self.seen.remove(&entity);
        self.facet_state_mut(facet).sectors.remove(entity);
        self.registry.despawn(entity);

        if let Some(serial) = serial {
            self.bus.send(PlayerLeft { entity, serial });
            info!(%serial, "left the world");
        }
    }

    // -- interest management ----------------------------------------------

    /// Every player who currently has `entity` on screen.
    fn watchers_of(&self, entity: EntityId) -> Vec<EntityId> {
        self.seen
            .iter()
            .filter(|(watcher, seen)| **watcher != entity && seen.contains(&entity))
            .map(|(watcher, _)| *watcher)
            .collect()
    }

    /// Bring `entity`'s neighbourhood up to date, both ways.
    ///
    /// Whoever it can see, and whoever can see it. Both, because visibility is
    /// symmetric here and doing one direction leaves the other end with a mobile
    /// that walked away and never left the screen.
    fn refresh_around(&mut self, entity: EntityId) {
        // Only this entity's facet: two mobiles on different facets share no
        // sector grid, so a lookup here never turns up anyone on another one.
        let facet = self.facet_of(entity);
        let sectors = &self.facet_state(facet).sectors;
        let Some(centre) = sectors.position_of(entity) else {
            return;
        };

        // Collect first. The lookup borrows the index and the sends borrow
        // `self` mutably, and more importantly a `Vec` here is what makes the
        // set of neighbours a snapshot rather than something that shifts while
        // it is walked.
        let neighbours: Vec<EntityId> = sectors
            .nearby(centre, VIEW_RANGE)
            .map(|(id, _)| id)
            .filter(|id| *id != entity)
            .collect();

        for other in &neighbours {
            self.show(entity, *other);
            self.show(*other, entity);
        }

        // Anything this one used to see and no longer can. `nearby` says who is
        // close; only the remembered set says who *was*.
        let gone: Vec<EntityId> = self
            .seen
            .get(&entity)
            .map(|seen| {
                seen.iter()
                    .filter(|id| !neighbours.contains(id))
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        for other in gone {
            if let Some(serial) = self.registry.serial_of(other) {
                self.forget(entity, other, serial);
            }
        }

        // And anyone who used to see this one and no longer can.
        for watcher in self.watchers_of(entity) {
            if !neighbours.contains(&watcher) {
                if let Some(serial) = self.registry.serial_of(entity) {
                    self.forget(watcher, entity, serial);
                }
            }
        }

        self.broadcast_move(entity);
    }

    /// Tell everyone already watching `entity` that it moved.
    ///
    /// Only those who already have it: someone seeing it for the first time gets
    /// a `0x78` from [`World::show`], and a `0x77` for a mobile the client has
    /// never heard of is ignored.
    fn broadcast_move(&mut self, entity: EntityId) {
        let Some(packet) = self.mobile_move(entity) else {
            return;
        };
        for watcher in self.watchers_of(entity) {
            let Some(&Client {
                connection,
                version,
            }) = self.registry.get::<Client>(watcher)
            else {
                continue;
            };
            self.outbox.push(Outbound {
                connection,
                packet: packet.encode(version),
            });
        }
    }

    /// Draw `other` for `watcher`, if it is not already on screen.
    fn show(&mut self, watcher: EntityId, other: EntityId) {
        // Only players have screens. An NPC "seeing" someone is an AI question,
        // and it does not belong in the packet path.
        let Some(&Client {
            connection,
            version,
        }) = self.registry.get::<Client>(watcher)
        else {
            return;
        };
        if self
            .seen
            .get(&watcher)
            .is_some_and(|seen| seen.contains(&other))
        {
            return;
        }
        let Some(packet) = self.draw_packet(other, version) else {
            return;
        };
        self.seen.entry(watcher).or_default().insert(other);
        self.outbox.push(Outbound { connection, packet });
    }

    /// The packet that draws `entity` on a client, or `None` for something not
    /// drawable. A mobile is a `0x78`, an item a `0x1A` — the interest system
    /// does not care which, only that there is one packet per thing on screen.
    fn draw_packet(&self, entity: EntityId, version: ClientVersion) -> Option<Vec<u8>> {
        if self.registry.has::<Body>(entity) {
            Some(self.mobile_incoming(entity)?.encode(version))
        } else if self.registry.has::<Graphic>(entity) {
            Some(self.world_item(entity)?.encode())
        } else {
            None
        }
    }

    /// Build a `0x1A` for an entity, if it is a drawable item.
    fn world_item(&self, entity: EntityId) -> Option<WorldItem> {
        let serial = self.registry.serial_of(entity)?;
        let Graphic { id, hue } = *self.registry.get::<Graphic>(entity)?;
        let Position(position) = *self.registry.get::<Position>(entity)?;
        // No `Amount` means a single. The encoder treats 1 and absent the same.
        let amount = self.registry.get::<Amount>(entity).map_or(1, |a| a.0);
        Some(WorldItem {
            serial: serial.raw(),
            graphic: id,
            amount,
            position,
            hue,
        })
    }

    /// Take `other` off `watcher`'s screen.
    fn forget(&mut self, watcher: EntityId, other: EntityId, serial: openshard_entities::Serial) {
        if let Some(seen) = self.seen.get_mut(&watcher) {
            if !seen.remove(&other) {
                return;
            }
        } else {
            return;
        }
        if let Some(&Client { connection, .. }) = self.registry.get::<Client>(watcher) {
            self.outbox.push(Outbound {
                connection,
                packet: encode_remove(serial.raw()),
            });
        }
    }

    /// Build a 0x78 for an entity, if it is a drawable mobile.
    fn mobile_incoming(&self, entity: EntityId) -> Option<MobileIncoming> {
        let serial = self.registry.serial_of(entity)?;
        let Position(position) = *self.registry.get::<Position>(entity)?;
        let Heading(facing) = *self.registry.get::<Heading>(entity)?;
        let body = *self.registry.get::<Body>(entity)?;
        Some(MobileIncoming {
            serial: serial.raw(),
            body: body.id,
            position,
            facing,
            hue: body.hue,
            flags: 0,
            notoriety: Notoriety::Innocent,
            equipment: self.equipment_of(serial),
        })
    }

    /// What a mobile is wearing, as the `0x78` equipment list.
    fn equipment_of(&self, mobile: Serial) -> Vec<Equipment> {
        self.registry
            .query::<Equipped>()
            .filter(|(_, worn)| worn.mobile == mobile)
            .filter_map(|(item, worn)| {
                let serial = self.registry.serial_of(item)?;
                let Graphic { id, hue } = *self.registry.get::<Graphic>(item)?;
                Some(Equipment {
                    serial: serial.raw(),
                    graphic: id,
                    layer: worn.layer,
                    hue,
                })
            })
            .collect()
    }

    /// Build a 0x77 for an entity.
    fn mobile_move(&self, entity: EntityId) -> Option<MobileMove> {
        let serial = self.registry.serial_of(entity)?;
        let Position(position) = *self.registry.get::<Position>(entity)?;
        let Heading(facing) = *self.registry.get::<Heading>(entity)?;
        let body = *self.registry.get::<Body>(entity)?;
        Some(MobileMove {
            serial: serial.raw(),
            body: body.id,
            position,
            facing,
            hue: body.hue,
            flags: 0,
            notoriety: Notoriety::Innocent,
        })
    }

    fn send(&mut self, connection: ConnectionId, packet: Vec<u8>) {
        self.outbox.push(Outbound { connection, packet });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use openshard_events::Cursor;
    use openshard_movement::WALK_INTERVAL;

    pub(super) const START: (u16, u16) = (1363, 1600);

    pub(super) fn world() -> World {
        World::new(START)
    }

    pub(super) fn connection() -> ConnectionId {
        ConnectionId::from_raw(1)
    }

    pub(super) fn enter(world: &mut World, now: Instant) -> ConnectionId {
        enter_as(world, connection(), now)
    }

    pub(super) fn enter_as(
        world: &mut World,
        connection: ConnectionId,
        now: Instant,
    ) -> ConnectionId {
        world.queue(Command::Enter {
            connection,
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "Lord British".to_owned(),
            serial: None,
            position: None,
            facet: 0,
            appearance: None,
        });
        world.tick(now);
        connection
    }

    /// Every packet the last tick produced for one connection.
    pub(super) fn packets_for(world: &mut World, connection: ConnectionId) -> Vec<Vec<u8>> {
        world
            .drain_outbound()
            .filter(|out| out.connection == connection)
            .map(|out| out.packet)
            .collect()
    }

    /// Put an entity somewhere directly, as if it had walked there.
    pub(super) fn teleport(world: &mut World, connection: ConnectionId, point: Point) {
        let entity = world.players[&connection];
        world.registry.insert(entity, Position(point));
        if let Some(Movement(mut walker)) = world.registry.get::<Movement>(entity).copied() {
            walker.position = point;
            world.registry.insert(entity, Movement(walker));
        }
        let facet = world.facet_of(entity);
        world.facet_state_mut(facet).sectors.insert(entity, point);
        world.refresh_around(entity);
    }

    pub(super) fn walk(sequence: u8, direction: Direction) -> WalkRequest {
        WalkRequest {
            facing: Facing::walking(direction),
            sequence,
            fastwalk_key: 0,
        }
    }

    /// The serial the world gave the character a connection is driving.
    fn serial_of(world: &World, connection: ConnectionId) -> u32 {
        let entity = world.players[&connection];
        world.registry.serial_of(entity).unwrap().raw()
    }

    #[test]
    fn a_server_step_turns_first_then_moves() {
        // Turn-as-step, server side: the first `Step` in a new direction turns
        // and stays put; the second moves. The same rule a client walk follows,
        // because the clients watching cannot tell who ordered the step.
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        let entity = world.players[&connection];
        let serial = serial_of(&world, connection);

        let facing0 = world.registry.get::<Heading>(entity).unwrap().0.direction;
        let dir = if facing0 == Direction::North {
            Direction::South
        } else {
            Direction::North
        };
        let from = world.registry.get::<Position>(entity).unwrap().0;

        let mut moved: Cursor<MobileMoved> = world.bus().cursor();
        let mut turned: Cursor<MobileTurned> = world.bus().cursor();

        world.queue(Command::Step {
            serial,
            direction: dir.to_bits(),
        });
        world.tick(now);
        assert_eq!(world.bus().read(&mut turned).count(), 1, "first step turns");
        assert_eq!(world.bus().read(&mut moved).count(), 0, "and does not move");
        assert_eq!(
            world.registry.get::<Position>(entity).unwrap().0,
            from,
            "still on the same tile"
        );

        world.queue(Command::Step {
            serial,
            direction: dir.to_bits(),
        });
        world.tick(now);
        let moves: Vec<MobileMoved> = world.bus().read(&mut moved).copied().collect();
        assert_eq!(moves.len(), 1, "second step moves");
        assert_eq!(moves[0].from, from);
        assert_eq!(moves[0].to, step_from(from, dir).unwrap());
        assert_eq!(
            world.registry.get::<Position>(entity).unwrap().0,
            step_from(from, dir).unwrap(),
        );
    }

    #[test]
    fn a_server_step_for_an_unknown_serial_is_a_no_op() {
        // A script can name a serial that has logged out between the event and
        // the command it queued in response. That is a miss, not a crash.
        let now = Instant::now();
        let mut world = world();
        enter(&mut world, now);
        let mut moved: Cursor<MobileMoved> = world.bus().cursor();
        world.queue(Command::Step {
            serial: 0x4000_0001,
            direction: 0,
        });
        world.tick(now);
        assert_eq!(world.bus().read(&mut moved).count(), 0);
    }

    #[test]
    fn a_server_step_off_the_edge_is_refused_not_a_wrap() {
        // Stepping north from y=0 has no landing tile. Refuse it — the mobile
        // must not wrap to the far side of the map.
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        let entity = world.players[&connection];
        let serial = serial_of(&world, connection);
        teleport(&mut world, connection, Point::new(0, 0, 0));

        let mut refused: Cursor<StepRefused> = world.bus().cursor();
        // Twice: the first may only turn to face north, the second attempts it.
        for _ in 0..2 {
            world.queue(Command::Step {
                serial,
                direction: Direction::North.to_bits(),
            });
            world.tick(now);
        }
        assert!(
            world.bus().read(&mut refused).count() >= 1,
            "a step off the edge is refused"
        );
        assert_eq!(
            world.registry.get::<Position>(entity).unwrap().0,
            Point::new(0, 0, 0),
            "and it did not move"
        );
    }

    /// The graphic of a gold coin — a real item id, used only so the tests read
    /// like the thing they describe.
    const GOLD: u16 = 0x0EED;

    fn spawn_item_at(world: &mut World, point: Point, now: Instant) {
        world.queue(Command::SpawnItem {
            graphic: GOLD,
            hue: 0,
            amount: 1,
            stackable: false,
            position: point,
            facet: 0,
        });
        world.tick(now);
    }

    /// Spawn a stackable pile of `amount` gold and return its serial.
    fn spawn_gold(world: &mut World, point: Point, amount: u16, now: Instant) -> u32 {
        world.queue(Command::SpawnItem {
            graphic: GOLD,
            hue: 0,
            amount,
            stackable: true,
            position: point,
            facet: 0,
        });
        world.tick(now);
        // The newest ground item, by serial.
        world
            .registry
            .query::<Position>()
            .filter(|(entity, _)| world.registry.has::<Stackable>(*entity))
            .filter_map(|(entity, _)| world.registry.serial_of(entity).map(|s| s.raw()))
            .max()
            .expect("the gold was spawned")
    }

    #[test]
    fn a_spawned_item_is_drawn_to_a_player_in_range() {
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        let _ = packets_for(&mut world, connection); // the login burst

        spawn_item_at(&mut world, Point::new(START.0, START.1, 0), now);

        let packets = packets_for(&mut world, connection);
        assert!(
            packets.iter().any(|p| p[0] == 0x1A),
            "the player standing on the tile is told about the item"
        );
    }

    #[test]
    fn an_item_out_of_range_is_not_drawn() {
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        let _ = packets_for(&mut world, connection);

        // Well past the view range.
        spawn_item_at(&mut world, Point::new(START.0 + 50, START.1, 0), now);

        let packets = packets_for(&mut world, connection);
        assert!(
            !packets.iter().any(|p| p[0] == 0x1A),
            "an item across the map is not drawn"
        );
    }

    #[test]
    fn walking_into_range_draws_an_item_and_out_of_range_forgets_it() {
        // The seen set at work, for items: an item is drawn exactly once when it
        // comes into range and removed with 0x1D when it leaves.
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);

        // Put the player far away and the item back at the start, out of range.
        teleport(&mut world, connection, Point::new(START.0 + 50, START.1, 0));
        spawn_item_at(&mut world, Point::new(START.0, START.1, 0), now);
        let _ = packets_for(&mut world, connection);

        // Come into range: the item is drawn.
        teleport(&mut world, connection, Point::new(START.0, START.1, 0));
        let arriving = packets_for(&mut world, connection);
        assert!(
            arriving.iter().any(|p| p[0] == 0x1A),
            "walking up to the item draws it"
        );

        // Leave again: the item is taken off the screen with 0x1D.
        teleport(&mut world, connection, Point::new(START.0 + 50, START.1, 0));
        let leaving = packets_for(&mut world, connection);
        assert!(
            leaving.iter().any(|p| p[0] == 0x1D),
            "walking away forgets the item"
        );
    }

    #[test]
    fn a_stacked_item_keeps_its_amount_when_drawn() {
        // A pile of gold is one entity with an amount, and the amount rides the
        // 0x1A that draws it — the packet sets the serial's top bit for it.
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        let _ = packets_for(&mut world, connection);

        world.queue(Command::SpawnItem {
            graphic: GOLD,
            hue: 0,
            amount: 500,
            stackable: false,
            position: Point::new(START.0, START.1, 0),
            facet: 0,
        });
        world.tick(now);

        let packets = packets_for(&mut world, connection);
        let item = packets
            .iter()
            .find(|p| p[0] == 0x1A)
            .expect("the item was drawn");
        // The amount bit on the serial says a stack; a single item would not set it.
        assert_ne!(item[3] & 0x80, 0, "the stack sets the amount flag");
    }

    /// The serial of the one item in the world.
    fn only_item_serial(world: &World) -> u32 {
        let (entity, _) = world
            .registry
            .query::<Graphic>()
            .next()
            .expect("an item is in the world");
        world.registry.serial_of(entity).unwrap().raw()
    }

    #[test]
    fn picking_up_then_dropping_moves_an_item_on_everyone_elses_screen() {
        // Two players on the same tile, an item between them. When one lifts it,
        // the other's client is told to forget it (0x1D); when it is set back
        // down, the other is told to draw it again (0x1A).
        let now = Instant::now();
        let mut world = world();
        let picker = enter(&mut world, now);
        let watcher = enter_as(&mut world, ConnectionId::from_raw(2), now);
        spawn_item_at(&mut world, Point::new(START.0, START.1, 0), now);
        let _ = packets_for(&mut world, picker);
        let _ = packets_for(&mut world, watcher);
        let serial = only_item_serial(&world);

        world.queue(Command::PickUpItem {
            connection: picker,
            serial,
            amount: 1,
        });
        world.tick(now);
        assert!(
            packets_for(&mut world, watcher)
                .iter()
                .any(|p| p[0] == 0x1D),
            "the other player is told to forget the lifted item"
        );

        world.queue(Command::DropItem {
            connection: picker,
            serial,
            position: Point::new(START.0, START.1, 0),
            container: DROP_TO_GROUND,
        });
        world.tick(now);
        assert!(
            packets_for(&mut world, watcher)
                .iter()
                .any(|p| p[0] == 0x1A),
            "and to draw it again where it was dropped"
        );
    }

    #[test]
    fn picking_up_out_of_reach_is_rejected_and_leaves_the_item() {
        let now = Instant::now();
        let mut world = world();
        let picker = enter(&mut world, now);
        spawn_item_at(&mut world, Point::new(START.0 + 20, START.1, 0), now);
        let _ = packets_for(&mut world, picker);
        let serial = only_item_serial(&world);
        let item = world
            .registry
            .entity_of(Serial::new(serial).unwrap())
            .unwrap();

        world.queue(Command::PickUpItem {
            connection: picker,
            serial,
            amount: 1,
        });
        world.tick(now);

        assert!(
            packets_for(&mut world, picker)
                .iter()
                .any(|p| p == &[0x27, 0x01]),
            "the client is told the item is out of range"
        );
        assert!(
            world.registry.has::<Position>(item),
            "the item stays on the ground"
        );
        assert!(world.held.is_empty(), "and nothing is on the cursor");
    }

    #[test]
    fn dropping_out_of_reach_bounces_the_item_back_to_where_it_was() {
        let now = Instant::now();
        let mut world = world();
        let picker = enter(&mut world, now);
        let origin = Point::new(START.0, START.1, 0);
        spawn_item_at(&mut world, origin, now);
        let serial = only_item_serial(&world);
        let item = world
            .registry
            .entity_of(Serial::new(serial).unwrap())
            .unwrap();

        world.queue(Command::PickUpItem {
            connection: picker,
            serial,
            amount: 1,
        });
        world.tick(now);
        let _ = packets_for(&mut world, picker);

        // Drop it far from the player: refused, and put back where it started.
        world.queue(Command::DropItem {
            connection: picker,
            serial,
            position: Point::new(START.0 + 40, START.1, 0),
            container: DROP_TO_GROUND,
        });
        world.tick(now);

        assert!(
            packets_for(&mut world, picker).iter().any(|p| p[0] == 0x27),
            "the drag is cancelled"
        );
        assert_eq!(
            world.registry.get::<Position>(item).map(|p| p.0),
            Some(origin),
            "and the item is back where it was lifted"
        );
        assert!(world.held.is_empty());
    }

    #[test]
    fn logging_out_while_holding_an_item_returns_it_to_the_ground() {
        let now = Instant::now();
        let mut world = world();
        let picker = enter(&mut world, now);
        let watcher = enter_as(&mut world, ConnectionId::from_raw(2), now);
        let origin = Point::new(START.0, START.1, 0);
        spawn_item_at(&mut world, origin, now);
        let serial = only_item_serial(&world);
        let item = world
            .registry
            .entity_of(Serial::new(serial).unwrap())
            .unwrap();

        world.queue(Command::PickUpItem {
            connection: picker,
            serial,
            amount: 1,
        });
        world.tick(now);
        let _ = packets_for(&mut world, watcher);

        world.queue(Command::Disconnect { connection: picker });
        world.tick(now);

        assert_eq!(
            world.registry.get::<Position>(item).map(|p| p.0),
            Some(origin),
            "the item is back on the ground, not lost with the cursor"
        );
        assert!(
            packets_for(&mut world, watcher)
                .iter()
                .any(|p| p[0] == 0x1A),
            "and the player still online sees it reappear"
        );
    }

    #[test]
    fn you_cannot_pick_up_a_mobile() {
        // A body has no `Graphic`, so lifting one is refused rather than yanking
        // a person onto the cursor.
        let now = Instant::now();
        let mut world = world();
        let picker = enter(&mut world, now);
        let other = enter_as(&mut world, ConnectionId::from_raw(2), now);
        let mobile_serial = serial_of(&world, other);
        let _ = packets_for(&mut world, picker);

        world.queue(Command::PickUpItem {
            connection: picker,
            serial: mobile_serial,
            amount: 1,
        });
        world.tick(now);
        assert!(
            packets_for(&mut world, picker)
                .iter()
                .any(|p| p == &[0x27, 0x00]),
            "cannot-lift is the reason"
        );
    }

    /// A backpack graphic and its gump.
    const BACKPACK: u16 = 0x0E75;
    const BACKPACK_GUMP: u16 = 0x003C;

    fn spawn_container_at(world: &mut World, point: Point, now: Instant) -> u32 {
        world.queue(Command::SpawnContainer {
            graphic: BACKPACK,
            gump: BACKPACK_GUMP,
            hue: 0,
            position: point,
            facet: 0,
        });
        world.tick(now);
        let (entity, _) = world
            .registry
            .query::<Container>()
            .next()
            .expect("a container is in the world");
        world.registry.serial_of(entity).unwrap().raw()
    }

    /// The serial of the one item that is not a container.
    fn loose_item_serial(world: &World) -> u32 {
        let (entity, _) = world
            .registry
            .query::<Graphic>()
            .find(|(entity, _)| !world.registry.has::<Container>(*entity))
            .expect("a non-container item exists");
        world.registry.serial_of(entity).unwrap().raw()
    }

    fn entity(world: &World, serial: u32) -> EntityId {
        world
            .registry
            .entity_of(Serial::new(serial).unwrap())
            .unwrap()
    }

    #[test]
    fn double_clicking_a_container_opens_it() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let container = spawn_container_at(&mut world, Point::new(START.0, START.1, 0), now);
        let _ = packets_for(&mut world, player);

        world.queue(Command::DoubleClick {
            connection: player,
            serial: container,
        });
        world.tick(now);

        let packets = packets_for(&mut world, player);
        assert!(packets.iter().any(|p| p[0] == 0x24), "the gump opens");
        assert!(packets.iter().any(|p| p[0] == 0x3C), "the contents follow");
    }

    #[test]
    fn dropping_an_item_into_a_container_puts_it_inside() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let here = Point::new(START.0, START.1, 0);
        let container = spawn_container_at(&mut world, here, now);
        spawn_item_at(&mut world, here, now);
        let item_serial = loose_item_serial(&world);
        let item = entity(&world, item_serial);
        let _ = packets_for(&mut world, player);

        world.queue(Command::PickUpItem {
            connection: player,
            serial: item_serial,
            amount: 1,
        });
        world.tick(now);
        world.queue(Command::DropItem {
            connection: player,
            serial: item_serial,
            position: Point::new(50, 60, 0), // gump coordinates, not tiles
            container,
        });
        world.tick(now);

        let contained = world
            .registry
            .get::<Contained>(item)
            .expect("the item is now in a container");
        assert_eq!(contained.container.raw(), container);
        assert_eq!((contained.x, contained.y), (50, 60));
        assert!(
            !world.registry.has::<Position>(item),
            "and no longer on the ground"
        );
        assert!(
            packets_for(&mut world, player).iter().any(|p| p[0] == 0x25),
            "the client is told the item went in"
        );
    }

    #[test]
    fn an_opened_container_lists_what_was_put_in_it() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let here = Point::new(START.0, START.1, 0);
        let container = spawn_container_at(&mut world, here, now);
        spawn_item_at(&mut world, here, now);
        let item_serial = loose_item_serial(&world);

        // Put the item in, then open the container and read the count.
        world.queue(Command::PickUpItem {
            connection: player,
            serial: item_serial,
            amount: 1,
        });
        world.tick(now);
        world.queue(Command::DropItem {
            connection: player,
            serial: item_serial,
            position: Point::new(50, 60, 0),
            container,
        });
        world.tick(now);
        let _ = packets_for(&mut world, player);

        world.queue(Command::DoubleClick {
            connection: player,
            serial: container,
        });
        world.tick(now);

        let contents = packets_for(&mut world, player)
            .into_iter()
            .find(|p| p[0] == 0x3C)
            .expect("a contents packet");
        assert_eq!(
            u16::from_be_bytes([contents[3], contents[4]]),
            1,
            "the one item is listed"
        );
    }

    #[test]
    fn picking_an_item_out_of_a_container_holds_it() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let here = Point::new(START.0, START.1, 0);
        let container = spawn_container_at(&mut world, here, now);
        spawn_item_at(&mut world, here, now);
        let item_serial = loose_item_serial(&world);
        let item = entity(&world, item_serial);

        // In, then straight back out.
        for _ in 0..1 {
            world.queue(Command::PickUpItem {
                connection: player,
                serial: item_serial,
                amount: 1,
            });
            world.tick(now);
            world.queue(Command::DropItem {
                connection: player,
                serial: item_serial,
                position: Point::new(50, 60, 0),
                container,
            });
            world.tick(now);
        }
        assert!(world.registry.has::<Contained>(item));

        world.queue(Command::PickUpItem {
            connection: player,
            serial: item_serial,
            amount: 1,
        });
        world.tick(now);
        assert!(
            !world.registry.has::<Contained>(item),
            "lifting it out drops the containment"
        );
        assert!(world.held.contains_key(&player), "and it is on the cursor");
    }

    #[test]
    fn dropping_into_something_that_is_not_a_container_bounces() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let here = Point::new(START.0, START.1, 0);
        // Two plain items: one to hold, one to (wrongly) drop onto.
        spawn_item_at(&mut world, here, now);
        let target = loose_item_serial(&world);
        world.queue(Command::SpawnItem {
            graphic: GOLD,
            hue: 0,
            amount: 1,
            stackable: false,
            position: here,
            facet: 0,
        });
        world.tick(now);
        // The held one is whichever item is not the target.
        let held_serial = world
            .registry
            .query::<Graphic>()
            .filter_map(|(e, _)| world.registry.serial_of(e).map(|s| s.raw()))
            .find(|s| *s != target)
            .unwrap();
        let held_item = entity(&world, held_serial);

        world.queue(Command::PickUpItem {
            connection: player,
            serial: held_serial,
            amount: 1,
        });
        world.tick(now);
        let origin = Point::new(START.0, START.1, 0);
        let _ = packets_for(&mut world, player);

        world.queue(Command::DropItem {
            connection: player,
            serial: held_serial,
            position: Point::new(0, 0, 0),
            container: target, // a real item, but not a container
        });
        world.tick(now);

        assert!(
            packets_for(&mut world, player).iter().any(|p| p[0] == 0x27),
            "the drag is cancelled"
        );
        assert_eq!(
            world.registry.get::<Position>(held_item).map(|p| p.0),
            Some(origin),
            "and the item is back on the ground where it was"
        );
    }

    /// Whether a 4-byte serial appears anywhere in a packet's body.
    fn mentions(packet: &[u8], serial: u32) -> bool {
        packet.windows(4).any(|w| w == serial.to_be_bytes())
    }

    /// Spawn a ground item at the player's feet and pick it up. Returns the item
    /// it just made — the newest one, by serial, so earlier items in the world do
    /// not confuse it.
    fn take_loose_item(
        world: &mut World,
        connection: ConnectionId,
        now: Instant,
    ) -> (u32, EntityId) {
        spawn_item_at(world, Point::new(START.0, START.1, 0), now);
        let (item, serial) = world
            .registry
            .query::<Position>()
            .filter(|(entity, _)| {
                world.registry.has::<Graphic>(*entity) && !world.registry.has::<Container>(*entity)
            })
            .filter_map(|(entity, _)| world.registry.serial_of(entity).map(|s| (entity, s.raw())))
            .max_by_key(|(_, serial)| *serial)
            .expect("a ground item to lift");
        world.queue(Command::PickUpItem {
            connection,
            serial,
            amount: 1,
        });
        world.tick(now);
        (serial, item)
    }

    /// A plausible armour layer.
    const LAYER_TORSO: u8 = 5;

    #[test]
    fn equipping_a_held_item_dresses_the_mobile() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let me = serial_of(&world, player);
        let (item_serial, item) = take_loose_item(&mut world, player, now);
        let _ = packets_for(&mut world, player);

        world.queue(Command::EquipItem {
            connection: player,
            item: item_serial,
            layer: LAYER_TORSO,
            mobile: me,
        });
        world.tick(now);

        let worn = world
            .registry
            .get::<Equipped>(item)
            .expect("the item is now worn");
        assert_eq!(worn.mobile.raw(), me);
        assert_eq!(worn.layer, LAYER_TORSO);
        assert_eq!(world.equipment_of(Serial::new(me).unwrap()).len(), 1);
        assert!(
            packets_for(&mut world, player).iter().any(|p| p[0] == 0x2E),
            "the wearer is told they put it on"
        );
    }

    #[test]
    fn a_newcomer_sees_a_dressed_mobile_in_its_0x78() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let me = serial_of(&world, player);
        let (item_serial, _) = take_loose_item(&mut world, player, now);
        world.queue(Command::EquipItem {
            connection: player,
            item: item_serial,
            layer: LAYER_TORSO,
            mobile: me,
        });
        world.tick(now);

        // A second player walks up and is drawn the first, now dressed.
        let newcomer = enter_as(&mut world, ConnectionId::from_raw(2), now);
        let drawn = packets_for(&mut world, newcomer)
            .into_iter()
            .find(|p| p[0] == 0x78 && mentions(p, me))
            .expect("the dressed mobile is drawn");
        assert!(
            mentions(&drawn, item_serial),
            "the worn item rides along in the 0x78"
        );
    }

    #[test]
    fn unequipping_lifts_the_item_off_and_forgets_it_for_others() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let watcher = enter_as(&mut world, ConnectionId::from_raw(2), now);
        let me = serial_of(&world, player);
        let (item_serial, item) = take_loose_item(&mut world, player, now);
        world.queue(Command::EquipItem {
            connection: player,
            item: item_serial,
            layer: LAYER_TORSO,
            mobile: me,
        });
        world.tick(now);
        let _ = packets_for(&mut world, watcher);

        world.queue(Command::PickUpItem {
            connection: player,
            serial: item_serial,
            amount: 1,
        });
        world.tick(now);

        assert!(!world.registry.has::<Equipped>(item), "it comes off");
        assert!(world.held.contains_key(&player), "and onto the cursor");
        assert!(
            packets_for(&mut world, watcher)
                .iter()
                .any(|p| p == &encode_remove(item_serial)),
            "the other player is told to forget it"
        );
    }

    #[test]
    fn a_layer_holds_only_one_item() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let me = serial_of(&world, player);

        // First item onto the torso.
        let (first, _) = take_loose_item(&mut world, player, now);
        world.queue(Command::EquipItem {
            connection: player,
            item: first,
            layer: LAYER_TORSO,
            mobile: me,
        });
        world.tick(now);

        // Second item, same layer: refused, and it bounces back to the ground.
        let (second, second_item) = take_loose_item(&mut world, player, now);
        let _ = packets_for(&mut world, player);
        world.queue(Command::EquipItem {
            connection: player,
            item: second,
            layer: LAYER_TORSO,
            mobile: me,
        });
        world.tick(now);

        assert!(
            packets_for(&mut world, player).iter().any(|p| p[0] == 0x27),
            "the second is refused"
        );
        assert!(
            world.registry.has::<Position>(second_item),
            "and returns to where it was lifted"
        );
        assert!(!world.registry.has::<Equipped>(second_item));
    }

    #[test]
    fn you_cannot_equip_onto_something_that_is_not_a_mobile() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        // A second ground item to (wrongly) equip onto.
        spawn_item_at(&mut world, Point::new(START.0, START.1, 0), now);
        let target = loose_item_serial(&world);
        let (held, held_item) = take_loose_item(&mut world, player, now);
        let _ = packets_for(&mut world, player);

        world.queue(Command::EquipItem {
            connection: player,
            item: held,
            layer: LAYER_TORSO,
            mobile: target, // an item, not a mobile
        });
        world.tick(now);

        assert!(
            packets_for(&mut world, player).iter().any(|p| p[0] == 0x27),
            "refused"
        );
        assert!(
            world.registry.has::<Position>(held_item),
            "and bounced back"
        );
    }

    #[test]
    fn dropping_a_stack_onto_an_identical_one_merges_them() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let here = Point::new(START.0, START.1, 0);
        let pile = spawn_gold(&mut world, here, 100, now);
        let loose = spawn_gold(&mut world, here, 50, now);
        let pile_item = entity(&world, pile);
        let loose_item = entity(&world, loose);
        let _ = packets_for(&mut world, player);

        // Lift the small pile and drop it onto the big one.
        world.queue(Command::PickUpItem {
            connection: player,
            serial: loose,
            amount: 50,
        });
        world.tick(now);
        world.queue(Command::DropItem {
            connection: player,
            serial: loose,
            position: here,
            container: pile, // dropping onto the other stack
        });
        world.tick(now);

        assert_eq!(
            world.registry.get::<Amount>(pile_item).map(|a| a.0),
            Some(150),
            "the amounts add"
        );
        assert!(
            !world.registry.contains(loose_item),
            "and the dropped pile is gone"
        );
        assert!(
            packets_for(&mut world, player).iter().any(|p| p[0] == 0x1A),
            "the surviving pile is redrawn with its new amount"
        );
    }

    #[test]
    fn a_non_stackable_item_does_not_merge() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let here = Point::new(START.0, START.1, 0);
        // Two plain (non-stackable) items.
        spawn_item_at(&mut world, here, now);
        let target = loose_item_serial(&world);
        let (held, held_item) = take_loose_item(&mut world, player, now);
        let _ = packets_for(&mut world, player);

        world.queue(Command::DropItem {
            connection: player,
            serial: held,
            position: here,
            container: target,
        });
        world.tick(now);

        assert!(
            packets_for(&mut world, player).iter().any(|p| p[0] == 0x27),
            "dropping one onto the other is refused"
        );
        assert!(
            world.registry.has::<Position>(held_item),
            "and it bounces back to the ground"
        );
    }

    #[test]
    fn a_ground_item_decays_after_its_time() {
        let now = Instant::now();
        let mut world = world();
        let watcher = enter(&mut world, now);
        spawn_item_at(&mut world, Point::new(START.0, START.1, 0), now);
        let serial = loose_item_serial(&world);
        let item = entity(&world, serial);
        let _ = packets_for(&mut world, watcher);

        // Bring the decay forward rather than run twenty minutes of ticks.
        let soon = world.ticks + 1;
        world.registry.insert(item, Decays { at_tick: soon });
        world.tick(now);

        assert!(!world.registry.contains(item), "the item has rotted away");
        assert!(
            packets_for(&mut world, watcher)
                .iter()
                .any(|p| p == &encode_remove(serial)),
            "and left every screen"
        );
    }

    #[test]
    fn an_item_off_the_ground_does_not_decay() {
        // Lifting an item takes the decay clock off it: a stack on a cursor, in a
        // pack or worn does not rot.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let (_, item) = take_loose_item(&mut world, player, now);
        assert!(
            !world.registry.has::<Decays>(item),
            "a held item carries no decay clock"
        );
    }

    #[test]
    fn a_command_does_nothing_until_the_tick() {
        // The whole boundary. If queueing acted immediately, world code would run
        // on a network thread at an arbitrary point, and two clients racing would
        // produce a different world depending on which packet won.
        let mut world = world();
        world.queue(Command::Enter {
            connection: connection(),
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "Lord British".to_owned(),
            serial: None,
            position: None,
            facet: 0,
            appearance: None,
        });

        assert_eq!(world.player_count(), 0, "queued, not applied");
        assert_eq!(world.drain_outbound().count(), 0, "and nothing sent");

        world.tick(Instant::now());
        assert_eq!(world.player_count(), 1);
    }

    #[test]
    fn entering_sends_the_sequence_the_client_needs() {
        let mut world = world();
        enter(&mut world, Instant::now());

        let ids: Vec<u8> = world.drain_outbound().map(|out| out.packet[0]).collect();
        assert_eq!(
            ids,
            vec![0x1B, 0xBF, 0x20, 0x4F, 0x55],
            "0x1B first or there is no body; 0x55 last or the client draws early"
        );
    }

    #[test]
    fn entering_builds_an_entity_out_of_components() {
        let mut world = world();
        enter(&mut world, Instant::now());

        let entity = *world.players.values().next().unwrap();
        assert!(world.registry().has::<Position>(entity));
        assert!(world.registry().has::<Body>(entity));
        assert!(world.registry().has::<Name>(entity));
        assert!(world.registry().has::<Movement>(entity), "a player walks");
        assert!(
            world.registry().has::<Client>(entity),
            "and has a connection"
        );
        assert!(world.registry().serial_of(entity).is_some());
    }

    #[test]
    fn a_created_character_enters_with_its_chosen_body() {
        // Character creation carries the body and hue the player picked; the
        // world must spawn that rather than its default human male.
        let mut world = world();
        let connection = connection();
        world.queue(Command::Enter {
            connection,
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "Nyx".to_owned(),
            serial: None,
            position: None,
            facet: 0,
            appearance: Some(Appearance {
                body: 0x025E,
                hue: 0x0430,
            }),
        });
        world.tick(Instant::now());

        let entity = world.players[&connection];
        let body = world.registry().get::<Body>(entity).copied().unwrap();
        assert_eq!(body.id, 0x025E, "the elf-female body the client chose");
        assert_eq!(body.hue, 0x0430);

        // And 0x1B tells the client the same body.
        let start = packets_for(&mut world, connection)
            .into_iter()
            .find(|packet| packet[0] == 0x1B)
            .expect("a PlayerStart");
        assert_eq!(
            &start[9..11],
            &0x025Eu16.to_be_bytes(),
            "0x1B carries the chosen body"
        );
    }

    #[test]
    fn a_played_character_keeps_the_default_body() {
        // The `None` path: playing an existing character has no appearance yet,
        // so the world uses its default and does not send a body of zero.
        let mut world = world();
        let connection = enter(&mut world, Instant::now());
        let entity = world.players[&connection];
        let body = world.registry().get::<Body>(entity).copied().unwrap();
        assert_eq!(body.id, BODY_HUMAN_MALE);
        assert_eq!(body.hue, DEFAULT_HUE);
    }

    #[test]
    fn a_loaded_character_returns_on_its_saved_serial_and_spot() {
        // Load-on-play: a stored character is played with its saved serial and
        // position, and must come back exactly there — not at the start point,
        // and not on a fresh serial that would orphan every reference to it.
        let mut world = world();
        let connection = connection();
        world.reserve_serial(0x0000_0202);
        world.queue(Command::Enter {
            connection,
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "Lord British".to_owned(),
            serial: Some(0x0000_0202),
            position: Some(Point::new(1500, 1000, -5)),
            facet: 0,
            appearance: Some(Appearance {
                body: 0x0191,
                hue: 0x83EA,
            }),
        });
        world.tick(Instant::now());

        let entity = world.players[&connection];
        assert_eq!(
            world.registry().serial_of(entity).unwrap().raw(),
            0x0000_0202,
            "it kept its saved serial"
        );
        assert_eq!(
            world.registry().get::<Position>(entity).unwrap().0,
            Point::new(1500, 1000, -5),
            "and its saved spot, z and all"
        );
    }

    #[test]
    fn a_saved_character_remembers_whose_it_is() {
        // The other half: `record_of` fills the account from the entity, so a
        // saved character can be tied back to its owner on load. A blank account
        // here is what left every loaded character ownerless before.
        let mut world = world();
        enter(&mut world, Instant::now());
        world.take_snapshot();
        let snapshot = world
            .drain_saves()
            .next()
            .expect("entering the world is a change worth saving");
        assert_eq!(snapshot.characters[0].account, "admin");
        assert_eq!(snapshot.characters[0].name, "Lord British");
    }

    /// Register a mapless facet, so a test can populate more than one without
    /// client files. Its interest grid is the same no-map size facet 0 uses.
    fn add_empty_facet(world: &mut World, facet: u8) {
        world.facets.insert(
            facet,
            FacetState {
                terrain: None,
                sectors: Sectors::new(FACET_WITHOUT_A_MAP.0, FACET_WITHOUT_A_MAP.1),
            },
        );
    }

    fn enter_on_facet(world: &mut World, connection: ConnectionId, facet: u8, now: Instant) {
        world.queue(Command::Enter {
            connection,
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "P".to_owned(),
            serial: None,
            position: None,
            facet,
            appearance: None,
        });
        world.tick(now);
    }

    #[test]
    fn two_facets_do_not_see_each_other() {
        // The whole point of a per-facet interest grid: two mobiles standing on
        // the very same coordinates, one on Felucca and one on Trammel, share no
        // screen. If this ever fails, someone reached for a single global grid.
        let mut world = world();
        add_empty_facet(&mut world, 1);
        let now = Instant::now();
        let here = ConnectionId::from_raw(1);
        let there = ConnectionId::from_raw(2);
        enter_on_facet(&mut world, here, 0, now);
        enter_on_facet(&mut world, there, 1, now);

        let a = world.players[&here];
        let b = world.players[&there];
        assert!(
            !world.seen[&a].contains(&b),
            "a mobile on facet 0 must not have drawn one on facet 1"
        );
        assert!(!world.seen[&b].contains(&a), "nor the other way round");
    }

    #[test]
    fn one_facet_at_the_same_spot_does_see() {
        // The control: the isolation above is facet-specific, not a bug that
        // hides everyone. Same coordinates, same facet — they see each other.
        let mut world = world();
        let now = Instant::now();
        let here = ConnectionId::from_raw(1);
        let there = ConnectionId::from_raw(2);
        enter_on_facet(&mut world, here, 0, now);
        enter_on_facet(&mut world, there, 0, now);

        let a = world.players[&here];
        let b = world.players[&there];
        assert!(
            world.seen[&a].contains(&b),
            "same facet, same spot: they see"
        );
        assert!(world.seen[&b].contains(&a));
    }

    #[test]
    fn entering_twice_on_one_connection_is_ignored() {
        let mut world = world();
        let now = Instant::now();
        enter(&mut world, now);
        enter(&mut world, now);
        assert_eq!(world.player_count(), 1);
    }

    #[test]
    fn walking_moves_the_position_component_too() {
        // Two places hold a position — `Position` and the `Movement`'s walker —
        // and a system that reads one while the other has moved is a rubber-band
        // bug. The tick is what keeps them in step.
        let mut world = world();
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let _ = world.drain_outbound().count();

        world.queue(Command::Walk {
            connection,
            request: walk(0, Direction::South),
        });
        world.tick(now);

        let entity = world.players[&connection];
        let Position(position) = *world.registry().get::<Position>(entity).unwrap();
        let Movement(walker) = *world.registry().get::<Movement>(entity).unwrap();
        assert_eq!(position, walker.position, "the two must not drift apart");
        assert_eq!(position, Point::new(START.0, START.1 + 1, Z_WITHOUT_A_MAP));
    }

    #[test]
    fn walking_emits_an_event_and_acks() {
        let mut world = world();
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let _ = world.drain_outbound().count();
        let mut moves: Cursor<MobileMoved> = world.bus().cursor();

        world.queue(Command::Walk {
            connection,
            request: walk(0, Direction::South),
        });
        world.tick(now);

        let sent: Vec<Vec<u8>> = world.drain_outbound().map(|out| out.packet).collect();
        assert_eq!(sent, vec![vec![0x22, 0, NOTORIETY_INNOCENT]]);

        let moved: Vec<_> = world.bus().read(&mut moves).copied().collect();
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].from, Point::new(START.0, START.1, Z_WITHOUT_A_MAP));
        assert_eq!(
            moved[0].to,
            Point::new(START.0, START.1 + 1, Z_WITHOUT_A_MAP)
        );
    }

    #[test]
    fn turning_emits_a_turn_not_a_move() {
        // A listener that cares where things are should not have to filter out
        // events where nothing went anywhere.
        let mut world = world();
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let mut moves: Cursor<MobileMoved> = world.bus().cursor();
        let mut turns: Cursor<MobileTurned> = world.bus().cursor();

        // Spawned facing south; ask for north.
        world.queue(Command::Walk {
            connection,
            request: walk(0, Direction::North),
        });
        world.tick(now);

        assert_eq!(world.bus().read(&mut moves).count(), 0, "nothing moved");
        assert_eq!(world.bus().read(&mut turns).count(), 1);
    }

    #[test]
    fn an_out_of_sequence_step_says_so() {
        let mut world = world();
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let mut refused: Cursor<StepRefused> = world.bus().cursor();

        world.queue(Command::Walk {
            connection,
            request: walk(9, Direction::South),
        });
        world.tick(now);

        let events: Vec<_> = world.bus().read(&mut refused).copied().collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].reason, RefusedReason::OutOfSequence);
    }

    #[test]
    fn a_flood_is_refused_and_says_so() {
        // The pace, through the tick. Every step in one instant is a speedhack.
        let mut world = world();
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let _ = world.drain_outbound().count();

        for sequence in 0..200u8 {
            world.queue(Command::Walk {
                connection,
                request: walk(sequence, Direction::South),
            });
        }
        world.tick(now);

        let rejects = world
            .drain_outbound()
            .filter(|out| out.packet[0] == 0x21)
            .count();
        assert!(rejects > 150, "only {rejects} of 200 instant steps refused");
    }

    #[test]
    fn an_honest_walker_is_never_refused_across_ticks() {
        let mut world = world();
        let start = Instant::now();
        let connection = enter(&mut world, start);
        let _ = world.drain_outbound().count();

        let mut sequence = 0u8;
        for step in 0..200u32 {
            let now = start + WALK_INTERVAL * step;
            world.queue(Command::Walk {
                connection,
                request: walk(sequence, Direction::South),
            });
            world.tick(now);
            let refused = world
                .drain_outbound()
                .filter(|out| out.packet[0] == 0x21)
                .count();
            assert_eq!(refused, 0, "step {step} refused");
            sequence = if sequence == u8::MAX { 1 } else { sequence + 1 };
        }
    }

    #[test]
    fn a_walk_from_a_connection_with_no_character_is_ignored() {
        let mut world = world();
        world.queue(Command::Walk {
            connection: connection(),
            request: walk(0, Direction::South),
        });
        world.tick(Instant::now());
        assert_eq!(world.drain_outbound().count(), 0);
    }

    #[test]
    fn disconnecting_releases_the_entity_and_its_serial() {
        let mut world = world();
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let entity = world.players[&connection];
        let serial = world.registry().serial_of(entity).unwrap();

        let mut left: Cursor<PlayerLeft> = world.bus().cursor();
        world.queue(Command::Disconnect { connection });
        world.tick(now);

        assert_eq!(world.player_count(), 0);
        assert!(!world.registry().contains(entity));
        assert_eq!(
            world.registry().entity_of(serial),
            None,
            "a dead serial resolves to nothing"
        );
        assert_eq!(world.bus().read(&mut left).count(), 1);
    }

    #[test]
    fn disconnecting_a_connection_that_never_entered_is_harmless() {
        let mut world = world();
        world.queue(Command::Disconnect {
            connection: connection(),
        });
        world.tick(Instant::now());
    }

    #[test]
    fn a_command_queued_during_a_tick_waits_for_the_next_one() {
        // The inbox is taken whole. Otherwise a system that queues work could
        // starve the loop, and a tick's length would depend on what happened in
        // it — which is the end of a fixed timestep.
        let mut world = world();
        let now = Instant::now();
        world.tick(now);
        let before = world.ticks();

        world.queue(Command::Enter {
            connection: connection(),
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "a".to_owned(),
            serial: None,
            position: None,
            facet: 0,
            appearance: None,
        });
        assert_eq!(world.player_count(), 0);
        world.tick(now);
        assert_eq!(world.ticks(), before + 1);
        assert_eq!(world.player_count(), 1);
    }

    #[test]
    fn an_empty_tick_is_cheap_and_harmless() {
        let mut world = world();
        let now = Instant::now();
        for _ in 0..1000 {
            world.tick(now);
        }
        assert_eq!(world.ticks(), 1000);
        assert!(world.registry().is_empty());
    }

    #[test]
    fn a_reader_that_polls_once_a_tick_never_misses_an_event() {
        // The property that matters, and the reason the bus is double-buffered.
        // A system reading once per tick sees everything, whatever order the
        // systems ran in — including one that polled *before* the emitter within
        // the same tick, which is what this simulates: the cursor is taken before
        // the tick that emits.
        let mut world = world();
        let now = Instant::now();
        let mut entered: Cursor<PlayerEntered> = world.bus().cursor();

        enter(&mut world, now);
        assert_eq!(world.bus().read(&mut entered).count(), 1);
    }

    #[test]
    fn an_event_is_gone_a_tick_after_the_one_that_emitted_it() {
        // The lifetime, stated as it actually is. `tick` calls `bus.update()` at
        // its end, so the emitting tick already spends one of the event's two
        // buffers: it is readable after that tick, and gone after the next.
        //
        // That is not a bug, and the guarantee still holds — a reader polling
        // once per tick has a full tick to see it. But "events live two ticks"
        // is off by one if you count from outside, and this is where you find
        // that out.
        let mut world = world();
        let now = Instant::now();
        enter(&mut world, now);

        let mut after_emit: Cursor<PlayerEntered> = world.bus().cursor();
        assert_eq!(
            world.bus().read(&mut after_emit).count(),
            1,
            "readable after the tick that emitted it"
        );

        world.tick(now);
        let mut a_tick_later: Cursor<PlayerEntered> = world.bus().cursor();
        assert_eq!(
            world.bus().read(&mut a_tick_later).count(),
            0,
            "and gone after the next"
        );
    }

    #[test]
    fn the_tick_interval_is_not_a_protocol_constant() {
        // 20Hz is ours to change. The client neither knows nor cares; it only
        // sees acks. Worth stating because the 200ms walk interval *is* the
        // client's, and the two are easy to confuse.
        assert_eq!(TICK_INTERVAL.as_millis(), 50);
        assert!(
            TICK_INTERVAL < WALK_INTERVAL,
            "a step must not span two ticks"
        );
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::tests::{enter, enter_as, walk, START};
    use super::*;
    use openshard_gateway::ConnectionId;
    use openshard_movement::WALK_INTERVAL;

    /// A world that saves every tick, so a test does not have to run four
    /// hundred of them to see one row.
    fn eager() -> World {
        World::new(START).with_save_every(1)
    }

    /// Take `count` steps, and return the tick time afterwards.
    ///
    /// The extra request is not a typo: a character spawns facing south, and the
    /// first request in any other direction turns rather than steps. A test that
    /// sends one request per step is a test that is off by one.
    fn steps(
        world: &mut World,
        connection: ConnectionId,
        direction: Direction,
        count: u32,
        start: Instant,
    ) -> Instant {
        let mut now = start;
        for request in 0..=count {
            now += WALK_INTERVAL;
            world.queue(Command::Walk {
                connection,
                request: walk(request as u8, direction),
            });
            world.tick(now);
        }
        now
    }

    fn only_snapshot(world: &mut World) -> Option<Snapshot> {
        let mut saves: Vec<_> = world.drain_saves().collect();
        assert!(saves.len() <= 1, "one tick, one snapshot");
        saves.pop()
    }

    #[test]
    fn entering_the_world_is_worth_saving() {
        let mut world = eager();
        let now = Instant::now();
        enter(&mut world, now);

        let snapshot = only_snapshot(&mut world).expect("a new character is a change");
        assert_eq!(snapshot.characters.len(), 1);
        assert_eq!(snapshot.characters[0].name, "Lord British");
        assert_eq!(snapshot.characters[0].x, START.0);
    }

    #[test]
    fn a_quiet_world_offers_nothing() {
        // The reason the tick offers an Option and not an empty snapshot. A
        // shard where nobody is doing anything must not queue a transaction
        // twenty times a second to say so.
        let mut world = eager();
        let now = Instant::now();
        enter(&mut world, now);
        let _ = world.drain_saves();

        for tick in 1..10 {
            world.tick(now + WALK_INTERVAL * tick);
        }
        assert_eq!(world.drain_saves().count(), 0);
    }

    #[test]
    fn walking_marks_the_character_without_anyone_remembering_to() {
        // The point of reading the bus. Nothing in `walk` mentions the journal:
        // the step is saved because the step was announced.
        let mut world = World::new(START).with_save_every(0);
        let now = Instant::now();
        let connection = enter(&mut world, now);

        let _ = steps(&mut world, connection, Direction::North, 1, now);
        world.take_snapshot();

        let snapshot = only_snapshot(&mut world).expect("a step is a change");
        assert_eq!(snapshot.characters.len(), 1);
        assert_eq!(
            snapshot.characters[0].y,
            START.1 - 1,
            "the snapshot must hold where the step went, not where it started"
        );
    }

    #[test]
    fn turning_is_worth_saving_too() {
        // A turn moves nobody, and a character that logs in facing the wrong way
        // is a small bug that is invisible until someone looks for it.
        let mut world = eager();
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let _ = world.drain_saves();

        // One request, one tick: a character spawns facing south, so the first
        // request east turns and goes nowhere.
        world.queue(Command::Walk {
            connection,
            request: walk(0, Direction::East),
        });
        world.tick(now + WALK_INTERVAL);

        let snapshot = only_snapshot(&mut world).expect("a turn is a change");
        assert_eq!(snapshot.characters[0].x, START.0, "a turn moves nobody");
        assert_eq!(
            snapshot.characters[0].facing,
            Facing::walking(Direction::East).to_bits()
        );
    }

    #[test]
    fn logging_out_saves_where_the_player_actually_stopped() {
        // The test `keep` exists for, and the one a `touch` cannot pass: by the
        // next save the entity is despawned and there is nothing left to read.
        // Getting this wrong loses the whole session and looks like a disk fault.
        let mut world = World::new(START).with_save_every(0);
        let now = Instant::now();
        let connection = enter(&mut world, now);

        let now = steps(&mut world, connection, Direction::North, 2, now);

        world.queue(Command::Disconnect { connection });
        world.tick(now + WALK_INTERVAL);
        assert_eq!(world.player_count(), 0, "and the entity is gone");

        world.take_snapshot();
        let snapshot = only_snapshot(&mut world).expect("a session is worth saving");
        assert_eq!(snapshot.characters.len(), 1);
        assert_eq!(
            snapshot.characters[0].y,
            START.1 - 2,
            "two steps north is where the player stopped"
        );
    }

    #[test]
    fn logging_out_does_not_delete_the_character() {
        // Disconnecting is not deleting. The entity goes; the character stays.
        let mut world = World::new(START).with_save_every(0);
        let now = Instant::now();
        let connection = enter(&mut world, now);
        world.queue(Command::Disconnect { connection });
        world.tick(now + WALK_INTERVAL);

        world.take_snapshot();
        let snapshot = only_snapshot(&mut world).expect("a change");
        assert!(
            snapshot.removed.is_empty(),
            "a logout must not queue a deletion"
        );
    }

    #[test]
    fn a_world_with_nowhere_to_save_keeps_no_journal_anyone_waits_on() {
        // save_every = 0 is a real mode. What it must not do is quietly grow a
        // journal forever, which is a leak that looks like a working shard.
        let mut world = World::new(START).with_save_every(0);
        let now = Instant::now();
        let connection = enter(&mut world, now);
        steps(&mut world, connection, Direction::North, 4, now);
        assert_eq!(world.drain_saves().count(), 0, "nothing was offered");
        assert!(world.unsaved() > 0, "but it is still tracked, and takeable");

        // And a caller that asks explicitly gets it all.
        world.take_snapshot();
        assert_eq!(
            only_snapshot(&mut world)
                .expect("a change")
                .characters
                .len(),
            1
        );
        assert_eq!(world.unsaved(), 0);
    }

    #[test]
    fn the_snapshot_arrives_on_the_cadence_and_not_before() {
        let mut world = World::new(START).with_save_every(4);
        let now = Instant::now();
        let connection = enter(&mut world, now);

        // enter() ran tick 1. Ticks 2 and 3 offer nothing; tick 4 does.
        world.queue(Command::Walk {
            connection,
            request: walk(0, Direction::North),
        });
        world.tick(now + WALK_INTERVAL);
        assert_eq!(world.drain_saves().count(), 0, "tick 2 is not a save tick");
        world.tick(now + WALK_INTERVAL * 2);
        assert_eq!(world.drain_saves().count(), 0, "nor tick 3");
        world.tick(now + WALK_INTERVAL * 3);
        assert_eq!(world.drain_saves().count(), 1, "tick 4 is");
    }

    #[test]
    fn thirty_steps_in_one_save_window_are_one_row() {
        // What the dirty set buys: a save proportional to activity, not to how
        // chatty the activity was.
        let mut world = World::new(START).with_save_every(0);
        let now = Instant::now();
        let connection = enter(&mut world, now);

        steps(&mut world, connection, Direction::North, 20, now);
        world.take_snapshot();
        let snapshot = only_snapshot(&mut world).expect("a change");
        assert_eq!(snapshot.characters.len(), 1, "one character, one row");
    }

    #[test]
    fn a_failed_save_is_retried_with_fresh_data_and_not_the_old_snapshot() {
        // Re-writing the failed snapshot would put the character back where it
        // was when the write began, which is a rollback nobody asked for. The
        // sweep re-reads instead.
        let mut world = World::new(START).with_save_every(0);
        let now = Instant::now();
        let connection = enter(&mut world, now);

        world.take_snapshot();
        let first = only_snapshot(&mut world).expect("a change");
        assert_eq!(first.characters[0].y, START.1);
        assert_eq!(world.unsaved(), 0, "the journal was drained");

        // The store said no.
        world.resweep();

        // And the world kept ticking while the write was failing.
        steps(&mut world, connection, Direction::North, 1, now);

        world.take_snapshot();
        let retry = only_snapshot(&mut world).expect("swept");
        assert_eq!(
            retry.characters[0].y,
            START.1 - 1,
            "the retry must write where the character is now, not where it was"
        );
    }

    #[test]
    fn a_sweep_finds_characters_nothing_has_touched() {
        // The escape hatch has to actually escape: a character that has done
        // nothing since the last save is not dirty, and a sweep must still find
        // it. Otherwise "always correct" is only true for people who moved.
        let mut world = World::new(START).with_save_every(0);
        let now = Instant::now();
        enter_as(&mut world, ConnectionId::from_raw(1), now);
        enter_as(&mut world, ConnectionId::from_raw(2), now);

        world.take_snapshot();
        let _ = world.drain_saves();
        assert_eq!(world.unsaved(), 0, "nobody is dirty");

        world.resweep();
        world.take_snapshot();
        let snapshot = only_snapshot(&mut world).expect("a sweep is a change");
        assert_eq!(snapshot.characters.len(), 2, "including the idle one");
    }

    #[test]
    fn two_players_are_two_rows_in_one_snapshot() {
        // The consistency promise: one drain, one instant, everyone in it.
        let mut world = World::new(START).with_save_every(0);
        let now = Instant::now();
        enter_as(&mut world, ConnectionId::from_raw(1), now);
        enter_as(&mut world, ConnectionId::from_raw(2), now);

        world.take_snapshot();
        let snapshot = only_snapshot(&mut world).expect("a change");
        assert_eq!(snapshot.characters.len(), 2);
        let serials: HashSet<u32> = snapshot.characters.iter().map(|c| c.serial).collect();
        assert_eq!(serials.len(), 2, "and two distinct serials");
    }

    #[test]
    fn a_saved_serial_is_the_one_the_client_was_told() {
        // The serial is on the wire and in every packet the client has been
        // sent. A character that comes back under a different one is a different
        // character with the same name.
        let mut world = World::new(START).with_save_every(0);
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let entity = world.players[&connection];
        let serial = world.registry.serial_of(entity).expect("bound");

        world.take_snapshot();
        let snapshot = only_snapshot(&mut world).expect("a change");
        assert_eq!(snapshot.characters[0].serial, serial.raw());
    }
}

#[cfg(test)]
mod interest_tests {
    use super::tests::*;
    use super::*;
    use openshard_movement::WALK_INTERVAL;

    const ALICE: ConnectionId = ConnectionId::from_raw(1);
    const BOB: ConnectionId = ConnectionId::from_raw(2);

    #[test]
    fn two_players_in_the_same_place_see_each_other() {
        // The thing this whole crate has been missing.
        let mut world = World::new(START);
        let now = Instant::now();

        enter_as(&mut world, ALICE, now);
        let _ = world.drain_outbound().count();

        enter_as(&mut world, BOB, now);
        let to_alice = packets_for(&mut world, ALICE);
        assert!(
            to_alice.iter().any(|p| p[0] == 0x78),
            "Alice was never told Bob arrived"
        );
    }

    #[test]
    fn a_newcomer_is_told_about_everyone_already_here() {
        // The other direction, and the one that is easy to forget: arriving is
        // symmetric. Bob's screen starts empty and Alice is already standing
        // there.
        let mut world = World::new(START);
        let now = Instant::now();

        enter_as(&mut world, ALICE, now);
        enter_as(&mut world, BOB, now);

        let to_bob = packets_for(&mut world, BOB);
        let drawn = to_bob.iter().filter(|p| p[0] == 0x78).count();
        assert_eq!(drawn, 1, "Bob should be drawn Alice, exactly once");
    }

    #[test]
    fn a_mobile_is_drawn_once_however_much_it_walks() {
        // The reason the server remembers what it sent. Without `seen`, every
        // step would redraw the mobile from scratch and the client would flicker.
        let mut world = World::new(START);
        let now = Instant::now();
        enter_as(&mut world, ALICE, now);
        enter_as(&mut world, BOB, now);
        let _ = world.drain_outbound().count();

        let mut drawn = 0;
        let mut moved = 0;
        for step in 1..=5u32 {
            world.queue(Command::Walk {
                connection: BOB,
                request: WalkRequest {
                    facing: Facing::walking(Direction::South),
                    sequence: (step - 1) as u8,
                    fastwalk_key: 0,
                },
            });
            world.tick(now + WALK_INTERVAL * step);
            for packet in packets_for(&mut world, ALICE) {
                match packet[0] {
                    0x78 => drawn += 1,
                    0x77 => moved += 1,
                    _ => {}
                }
            }
        }
        assert_eq!(drawn, 0, "Bob was redrawn mid-walk");
        assert!(moved > 0, "Alice never saw Bob move");
    }

    #[test]
    fn walking_out_of_range_removes_the_mobile() {
        let mut world = World::new(START);
        let now = Instant::now();
        enter_as(&mut world, ALICE, now);
        enter_as(&mut world, BOB, now);
        let _ = world.drain_outbound().count();

        // Well past the view range.
        teleport(
            &mut world,
            BOB,
            Point::new(START.0 + VIEW_RANGE as u16 + 5, START.1, Z_WITHOUT_A_MAP),
        );

        let to_alice = packets_for(&mut world, ALICE);
        assert!(
            to_alice.iter().any(|p| p[0] == 0x1D),
            "Bob walked away and stayed on Alice's screen forever"
        );
    }

    #[test]
    fn walking_back_into_range_draws_it_again() {
        let mut world = World::new(START);
        let now = Instant::now();
        enter_as(&mut world, ALICE, now);
        enter_as(&mut world, BOB, now);

        let far = Point::new(START.0 + VIEW_RANGE as u16 + 5, START.1, Z_WITHOUT_A_MAP);
        teleport(&mut world, BOB, far);
        let _ = world.drain_outbound().count();

        teleport(
            &mut world,
            BOB,
            Point::new(START.0, START.1, Z_WITHOUT_A_MAP),
        );
        let to_alice = packets_for(&mut world, ALICE);
        assert!(
            to_alice.iter().any(|p| p[0] == 0x78),
            "Bob came back and was never redrawn"
        );
    }

    #[test]
    fn removal_is_sent_once_not_every_tick() {
        // `forget` returning early when nothing was removed is what stops a
        // 0x1D per tick for a mobile that left a minute ago.
        let mut world = World::new(START);
        let now = Instant::now();
        enter_as(&mut world, ALICE, now);
        enter_as(&mut world, BOB, now);

        let far = Point::new(START.0 + VIEW_RANGE as u16 + 5, START.1, Z_WITHOUT_A_MAP);
        teleport(&mut world, BOB, far);
        let _ = world.drain_outbound().count();

        // Move again, still out of range.
        teleport(&mut world, BOB, Point::new(far.x + 1, far.y, far.z));
        let removes = packets_for(&mut world, ALICE)
            .iter()
            .filter(|p| p[0] == 0x1D)
            .count();
        assert_eq!(removes, 0, "a second removal for a mobile already gone");
    }

    #[test]
    fn a_player_is_never_sent_itself() {
        // Sphere's own comment: 0x77 cannot move the receiving client's
        // character. Sending one is invisible and puts the two ends a tile apart.
        let mut world = World::new(START);
        let now = Instant::now();
        enter_as(&mut world, ALICE, now);
        let _ = world.drain_outbound().count();

        world.queue(Command::Walk {
            connection: ALICE,
            request: WalkRequest {
                facing: Facing::walking(Direction::South),
                sequence: 0,
                fastwalk_key: 0,
            },
        });
        world.tick(now);

        let ids: Vec<u8> = packets_for(&mut world, ALICE)
            .iter()
            .map(|p| p[0])
            .collect();
        assert!(!ids.contains(&0x78), "Alice was drawn to herself");
        assert!(
            !ids.contains(&0x77),
            "Alice was moved for herself; 0x20 does that"
        );
    }

    #[test]
    fn leaving_takes_the_mobile_off_every_screen() {
        let mut world = World::new(START);
        let now = Instant::now();
        enter_as(&mut world, ALICE, now);
        enter_as(&mut world, BOB, now);
        let _ = world.drain_outbound().count();

        world.queue(Command::Disconnect { connection: BOB });
        world.tick(now);

        let to_alice = packets_for(&mut world, ALICE);
        assert!(
            to_alice.iter().any(|p| p[0] == 0x1D),
            "Bob logged out and stayed on Alice's screen"
        );
    }

    #[test]
    fn leaving_removes_the_watcher_bookkeeping_too() {
        // A `seen` set that outlives its player is a slow leak: every login
        // leaves one behind and `watchers_of` walks them all forever.
        let mut world = World::new(START);
        let now = Instant::now();
        enter_as(&mut world, ALICE, now);
        enter_as(&mut world, BOB, now);
        assert_eq!(world.seen.len(), 2);

        world.queue(Command::Disconnect { connection: BOB });
        world.tick(now);

        assert_eq!(world.seen.len(), 1, "Bob's screen outlived Bob");
        assert_eq!(
            world.sectors().len(),
            1,
            "and so did his place in the index"
        );
    }

    #[test]
    fn the_index_never_disagrees_with_the_position() {
        // Two copies of where something is. The tick is what keeps them in step,
        // and this is the assertion that says so.
        let mut world = World::new(START);
        let start = Instant::now();
        let alice = enter_as(&mut world, ALICE, start);
        let entity = world.players[&alice];

        for step in 1..=50u32 {
            world.queue(Command::Walk {
                connection: alice,
                request: WalkRequest {
                    facing: Facing::walking(Direction::South),
                    sequence: (step - 1) as u8,
                    fastwalk_key: 0,
                },
            });
            world.tick(start + WALK_INTERVAL * step);

            let Position(position) = *world.registry().get::<Position>(entity).unwrap();
            assert_eq!(
                world.sectors().position_of(entity),
                Some(position),
                "the index drifted from the component at step {step}"
            );
        }
    }

    #[test]
    fn two_hundred_players_in_one_place_do_not_stop_the_tick() {
        // Not a benchmark — a shape check. Everyone sees everyone here, so the
        // work really is quadratic in the crowd; what the index buys is that a
        // crowd in Britain costs nothing to a player in Vesper.
        let mut world = World::new(START);
        let now = Instant::now();
        for id in 0..200u64 {
            enter_as(&mut world, ConnectionId::from_raw(id + 1), now);
        }
        assert_eq!(world.player_count(), 200);
        let _ = world.drain_outbound().count();

        // One far away: its refresh must not touch the crowd at all.
        let loner = ConnectionId::from_raw(1000);
        enter_as(&mut world, loner, now);
        teleport(&mut world, loner, Point::new(6000, 3000, Z_WITHOUT_A_MAP));
        let _ = world.drain_outbound().count();

        teleport(&mut world, loner, Point::new(6001, 3000, Z_WITHOUT_A_MAP));
        assert_eq!(
            world.drain_outbound().count(),
            0,
            "a step in Vesper sent packets to a crowd in Britain"
        );
    }
}
