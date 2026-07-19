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
use openshard_persistence::{
    CharacterRecord, Inventory, ItemLocation, ItemRecord, Journal, Snapshot, SCHEMA_VERSION,
};
use openshard_protocol::{
    encode_light_level, encode_login_complete, encode_map_change, encode_message, encode_walk_ack,
    encode_walk_reject, AccessLevel, ClientVersion, Direction, Facing, MobileStatus, Notoriety,
    PlayerStart, PlayerUpdate, Point, WalkRequest, DEFAULT_MAP_HEIGHT, DEFAULT_MAP_WIDTH,
    LABEL_MODE,
};
use tracing::{debug, info, warn};

use openshard_state::components::{
    Access, Account, Amount, Banker, Body, Brain, Client, Combat, Contained, Container, DamageType,
    Decoration, Door, Equipped, Facet, Graphic, Heading, Hitpoints, Mana, MeleeDamage, Movement,
    Name, Npc, Position, Resistance, Scripted, SpawnedBy, Stackable, Stats, SwingSpeed,
};
use openshard_state::rng::Rng;
use openshard_state::sectors::Sectors;
use openshard_state::{FacetState, Gameplay, Outbound, WorldState, TICKS_PER_SECOND};

use openshard_ai as ai;
use openshard_chat as chat;
use openshard_combat as combat;
use openshard_items as items;
use openshard_magic as magic;
use openshard_npc as npc;
use openshard_skills as skills;

use crate::doorgen;
use crate::events::{
    AdminMenuAction, MobileMoved, MobileSpawned, MobileTurned, PlayerEntered, PlayerLeft,
    RefusedReason, SpellRequested, StepRefused,
};
use crate::gm;
use crate::terrain::MapTerrain;

mod command;
mod defaults;

pub use command::{Appearance, Command, DecorContainer, DecorDoor};
use defaults::*;
pub use defaults::{SAVE_EVERY_TICKS, TICK_INTERVAL};

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
    access: AccessLevel,
}

/// Everything [`World::spawn_mobile`] needs — a plain bundle, so the one function
/// that makes a creature takes one argument instead of eleven.
struct SpawnMobile {
    body: u16,
    hue: u16,
    hits: u16,
    notoriety: u8,
    damage: u16,
    resistance: u8,
    swing: u64,
    sight: u8,
    wander: bool,
    position: Point,
    facet: u8,
    /// A name the client shows on single-click, if any. Townsfolk have one.
    name: Option<String>,
    /// Whether this mobile is a banker — it answers "bank".
    banker: bool,
    /// Worn clothing and gear, `(graphic, layer, hue)` — so it is not naked.
    equipment: Vec<(u16, u8, u16)>,
}

// `Outbound`, `FacetState`, `HeldItem` and `Origin` are the world's runtime
// state, moved down into `openshard-state` with `WorldState` so the systems can
// live in their own crates. Imported at the top of the file.

/// The world: the runtime state plus the tick that drives it and the journal
/// that saves it.
///
/// The gameplay state — registry, bus, facets, who-sees-what — lives in
/// [`WorldState`], one level down, so systems can operate on it from their own
/// crates. What stays here is what a system never touches: the persistence
/// journal, the save cadence, and the command queue the tick drains. A plain
/// value: nothing is a static, and a test builds as many as it likes.
pub struct World {
    /// The runtime state every gameplay system reads and writes.
    state: WorldState,
    /// What has changed since the last save.
    journal: Journal,
    /// How often to offer a snapshot, in ticks. Zero never saves.
    save_every: u64,
    /// Snapshots the tick has taken and nobody has collected yet.
    saves: Vec<Snapshot>,
    /// Characters that left this tick, with the state they left in. The server
    /// drains these to keep its in-memory character list current, so a re-login in
    /// the same run finds the character where it logged out — not where it was at
    /// boot. The store gets the same record through the journal; this is the
    /// immediate copy, because a re-login can beat the next deferred save.
    departed: Vec<CharacterRecord>,
    /// Read to find out what to mark dirty. See `mark_dirty`.
    entered: Cursor<PlayerEntered>,
    /// Read to find out what to mark dirty. See `mark_dirty`.
    moved: Cursor<MobileMoved>,
    /// Read to find out what to mark dirty. See `mark_dirty`.
    turned: Cursor<MobileTurned>,
    /// Commands waiting for the next tick.
    inbox: Vec<Command>,
    /// The spawn regions the tick keeps populated. Registered by the script pack,
    /// maintained here, and persisted — a populated area stays populated across a
    /// restart, and a rare spawn keeps its remaining respawn wait.
    spawners: Vec<crate::spawner::Spawner>,
    /// The next id to hand a newly registered spawner. Bumped past every id loaded
    /// from the store so a fresh registration never collides with a restored one.
    next_spawner_id: u32,
    /// Saved inventories waiting for their owners to log in, keyed by character
    /// serial. Loaded from the store at boot by [`restore_inventory`]; a character
    /// entering takes its own and equips it, once.
    ///
    /// [`restore_inventory`]: World::restore_inventory
    pending_inventories: HashMap<u32, Vec<ItemRecord>>,
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World")
            .field("state", &self.state)
            .field("unsaved", &self.journal.len())
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
            state: WorldState {
                registry: Registry::new(),
                bus: EventBus::new(),
                facets,
                default_facet: DEFAULT_FACET,
                players: HashMap::new(),
                seen: HashMap::new(),
                held: HashMap::new(),
                start,
                rng: Rng::new(DEFAULT_SEED),
                ticks: 0,
                outbox: Vec::new(),
                open_containers: HashMap::new(),
                pending_targets: HashMap::new(),
                gameplay: Gameplay::default(),
                save_requested: false,
            },
            journal: Journal::new(),
            save_every: SAVE_EVERY_TICKS,
            saves: Vec::new(),
            departed: Vec::new(),
            entered: Cursor::default(),
            moved: Cursor::default(),
            turned: Cursor::default(),
            inbox: Vec::new(),
            spawners: Vec::new(),
            next_spawner_id: 1,
            pending_inventories: HashMap::new(),
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

    /// How often to save, in *seconds* — what the operator sets in the config. `0`
    /// keeps the periodic save off (only shutdown and a staff `.save` write). The
    /// world owns the tick rate, so the conversion lives here rather than in the
    /// server.
    pub const fn with_save_seconds(self, seconds: u64) -> Self {
        self.with_save_every(seconds.saturating_mul(TICKS_PER_SECOND))
    }

    /// Set the tunable gameplay rules. The server builds these from the
    /// `[gameplay]` config; a test or the default takes [`Gameplay::default`],
    /// the pre-AoS numbers the systems were written with.
    #[must_use]
    pub const fn with_gameplay(mut self, gameplay: Gameplay) -> Self {
        self.state.gameplay = gameplay;
        self
    }

    /// Give the default facet a map.
    pub fn with_terrain(self, terrain: MapTerrain) -> Self {
        let facet = self.state.default_facet;
        self.with_facet(facet, terrain)
    }

    /// Load `terrain` as facet `facet`, its interest grid sized to the map.
    pub fn with_facet(mut self, facet: u8, terrain: MapTerrain) -> Self {
        let sectors = Sectors::new(terrain.map().width(), terrain.map().height());
        // Boxed as `dyn Terrain`: the state crate holds the abstraction, and the
        // world supplies the concrete map here.
        self.state.facets.insert(
            facet,
            FacetState {
                terrain: Some(Box::new(terrain) as Box<dyn Terrain + Send + Sync>),
                sectors,
            },
        );
        self
    }

    /// The default facet's spatial index.
    pub fn sectors(&self) -> &Sectors {
        &self.state.facets[&self.state.default_facet].sectors
    }

    /// The event bus, for reading what happened.
    pub const fn bus(&self) -> &EventBus {
        &self.state.bus
    }

    /// Everything in the world.
    pub const fn registry(&self) -> &Registry {
        &self.state.registry
    }

    /// How many ticks have run.
    pub const fn ticks(&self) -> u64 {
        self.state.ticks
    }

    /// How many people are in the world.
    pub fn player_count(&self) -> usize {
        self.state.players.len()
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
        self.state.outbox.drain(..)
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

    /// Take the records of characters that logged out since the last call.
    ///
    /// The server keeps an in-memory character list — where each stored character
    /// was, so playing one spawns it back at its spot — seeded from the store at
    /// boot. Without this it would go stale the moment a character moved and logged
    /// out, and a re-login in the same run would rewind to the boot position. These
    /// are the fresh records to fold in; the store gets the same data through the
    /// journal, but a re-login can beat that deferred write, so this is the copy
    /// that closes the gap.
    pub fn drain_departed(&mut self) -> std::vec::Drain<'_, CharacterRecord> {
        self.departed.drain(..)
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
            .state
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
        self.state.ticks += 1;

        // Take the whole inbox. A command queued *during* a tick belongs to the
        // next one — otherwise a system that queues work could starve the loop,
        // and the tick's length would depend on what happened in it.
        let commands = std::mem::take(&mut self.inbox);
        for command in commands {
            self.apply(command, now);
        }

        // Strike whatever swings are due, lift any criminal flags that have run
        // out, then rot away what has lain on the ground too long. All after the
        // commands and all driven by the tick counter, so a fight, a flag and a
        // decay are as replayable as everything else.
        self.think();
        // The townsfolk beat: `npc::live` greets and faces on its own and hands
        // back the idle steps it wants, which the tick applies through `step` —
        // the same decide-then-apply split the creature brain uses.
        for (serial, direction) in npc::live(&mut self.state) {
            self.step(serial, direction);
        }
        combat::swings(&mut self.state);
        combat::expire_criminality(&mut self.state);
        combat::decay_murders(&mut self.state);
        magic::regen_mana(&mut self.state);
        items::decay(&mut self.state);
        items::close_doors(&mut self.state);
        self.maintain_spawners();

        // Before the bus retires anything: what happened is what needs saving,
        // and reading it after `update` would read it a tick late.
        self.mark_dirty();
        // A staff `.save` this tick forces a snapshot now; otherwise the cadence
        // decides. Either way the world never pauses — the snapshot is instant.
        if std::mem::take(&mut self.state.save_requested) {
            self.take_snapshot();
        } else {
            self.offer_snapshot();
        }

        // Retire the oldest events. Once per tick, after every system, so that
        // "one tick" means the same thing for every event type.
        self.state.bus.update();
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
        changed.extend(
            self.state
                .bus
                .read(&mut self.entered)
                .map(|event| event.entity),
        );
        changed.extend(
            self.state
                .bus
                .read(&mut self.moved)
                .map(|event| event.entity),
        );
        changed.extend(
            self.state
                .bus
                .read(&mut self.turned)
                .map(|event| event.entity),
        );
        for entity in changed {
            self.journal.touch(entity);
        }
    }

    /// Every `save_every` ticks, hand what changed to whoever is collecting.
    fn offer_snapshot(&mut self) {
        if self.save_every == 0 || !self.state.ticks.is_multiple_of(self.save_every) {
            return;
        }
        self.take_snapshot();
    }

    /// Take a snapshot now, whatever the cadence says.
    ///
    /// For shutdown, for a GM save command, and for tests that would rather not
    /// tick four hundred times to see one row.
    pub fn take_snapshot(&mut self) {
        let ticks = self.state.ticks;

        // Start from the journal's logged-out records, their kept inventories, and
        // deletions. Dirty *online*-character records are dropped (the `|_| None`)
        // because every online character is saved in full below regardless — an
        // item picked up without a step never marks the character dirty, so the
        // dirty set is not a safe basis for saving what a character holds.
        let mut snapshot = self.journal.drain(ticks, |_| None).unwrap_or(Snapshot {
            tick: ticks,
            schema: SCHEMA_VERSION,
            characters: Vec::new(),
            removed: Vec::new(),
            inventories: Vec::new(),
            ground: None,
            spawners: None,
        });

        // Every online character, whole: its record and its entire carried
        // inventory — worn gear, backpack, bank box and everything nested. A save is
        // a complete picture of who is here and what they hold, so nothing of value
        // depends on whether its owner happened to move this tick.
        let online: Vec<EntityId> = self.state.players.values().copied().collect();
        for entity in online {
            if let Some(record) = Self::record_of(&self.state.registry, entity) {
                let owner = record.serial;
                snapshot.characters.push(record);
                snapshot.inventories.push(Inventory {
                    owner,
                    items: self.inventory_of(entity),
                });
            }
        }

        // The whole ground, every save — decoration excluded. Dropped loot and
        // stray items persist whether or not anyone was active this tick.
        snapshot.ground = Some(self.ground_items());
        // And every spawn region with its timer, so populated areas stay populated
        // across a restart and a rare spawn's wait is not reset.
        snapshot.spawners = Some(self.spawner_records());

        // Skip only a genuinely empty save, so a quiet, empty shard queues nothing.
        let ground_empty = snapshot.ground.as_ref().is_none_or(Vec::is_empty);
        let spawners_empty = snapshot.spawners.as_ref().is_none_or(Vec::is_empty);
        if snapshot.characters.is_empty()
            && snapshot.removed.is_empty()
            && ground_empty
            && spawners_empty
        {
            return;
        }
        debug!(tick = ticks, rows = snapshot.len(), "snapshot taken");
        self.saves.push(snapshot);
    }

    /// Every item a character is carrying — worn, and inside anything worn, at any
    /// depth — as saveable records owned by that character.
    ///
    /// A breadth-first walk: the worn items first, then the contents of every
    /// container found, and their containers in turn. `owner` is the character on
    /// every record however deep, because that is the key a store replaces a whole
    /// inventory by.
    fn inventory_of(&self, entity: EntityId) -> Vec<ItemRecord> {
        let registry = &self.state.registry;
        let Some(owner) = registry.serial_of(entity) else {
            return Vec::new();
        };
        let owner_raw = owner.raw();
        let mut records = Vec::new();
        let mut containers: Vec<Serial> = Vec::new();

        for (item, worn) in registry.query::<Equipped>() {
            if worn.mobile != owner {
                continue;
            }
            let location = ItemLocation::Equipped {
                mobile: owner_raw,
                layer: worn.layer,
            };
            if let Some(record) = Self::item_record(registry, item, owner_raw, location) {
                if record.container_gump.is_some() {
                    if let Some(serial) = registry.serial_of(item) {
                        containers.push(serial);
                    }
                }
                records.push(record);
            }
        }

        while let Some(container) = containers.pop() {
            for (item, held) in registry.query::<Contained>() {
                if held.container != container {
                    continue;
                }
                let location = ItemLocation::Contained {
                    container: container.raw(),
                    x: held.x,
                    y: held.y,
                    grid: held.grid,
                };
                if let Some(record) = Self::item_record(registry, item, owner_raw, location) {
                    if record.container_gump.is_some() {
                        if let Some(serial) = registry.serial_of(item) {
                            containers.push(serial);
                        }
                    }
                    records.push(record);
                }
            }
        }
        records
    }

    /// Every loose item on the ground — the dropped and the spawned, but not the
    /// [`Decoration`] a pack re-places and not a mobile — as ownerless records.
    fn ground_items(&self) -> Vec<ItemRecord> {
        let registry = &self.state.registry;
        let mut records = Vec::new();
        for (item, Position(at)) in registry.query::<Position>() {
            // A drawable thing on the ground: a graphic, not a mobile (which carries
            // a Body), and not decoration (which the pack owns and re-lays).
            if !registry.has::<Graphic>(item)
                || registry.has::<Body>(item)
                || registry.has::<Decoration>(item)
            {
                continue;
            }
            let facet = self.state.facet_of(item);
            let location = ItemLocation::Ground {
                facet,
                x: at.x,
                y: at.y,
                z: at.z,
            };
            if let Some(record) = Self::item_record(registry, item, 0, location) {
                records.push(record);
            }
        }
        records
    }

    /// Turn one item entity into a saveable record, or `None` if it is not a
    /// drawable item (no graphic or no serial).
    fn item_record(
        registry: &Registry,
        item: EntityId,
        owner: u32,
        location: ItemLocation,
    ) -> Option<ItemRecord> {
        let serial = registry.serial_of(item)?;
        let graphic = registry.get::<Graphic>(item)?;
        let amount = registry.get::<Amount>(item).map_or(1, |a| a.0);
        let container_gump = registry.get::<Container>(item).map(|c| c.gump);
        Some(ItemRecord {
            serial: serial.raw(),
            owner,
            graphic: graphic.id,
            hue: graphic.hue,
            amount,
            stackable: registry.has::<Stackable>(item),
            container_gump,
            location,
        })
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
            self.state.registry.reserve_serial(serial);
        }
    }

    /// Bring saved items back from the store at boot.
    ///
    /// Reserves every item's serial so a live spawn cannot take it, places the
    /// loose ground items now, and files each character's carried items away by
    /// owner for [`enter`](Self::enter) to equip when that character logs in. Call
    /// once, after the map is loaded and before anyone connects.
    pub fn restore_items(&mut self, records: Vec<ItemRecord>) {
        for record in &records {
            self.reserve_serial(record.serial);
        }
        for record in records {
            if record.owner == 0 {
                self.place_ground_item(&record);
            } else {
                self.pending_inventories
                    .entry(record.owner)
                    .or_default()
                    .push(record);
            }
        }
    }

    /// Put one restored item on the ground, bound to its saved serial.
    fn place_ground_item(&mut self, record: &ItemRecord) {
        let ItemLocation::Ground { facet, x, y, z } = record.location else {
            return;
        };
        let Some(serial) = Serial::new(record.serial) else {
            return;
        };
        let facet = if self.state.facets.contains_key(&facet) {
            facet
        } else {
            self.state.default_facet
        };
        let entity = self.state.registry.spawn();
        if self.state.registry.bind_serial(entity, serial).is_err() {
            self.state.registry.despawn(entity);
            return;
        }
        let position = Point::new(x, y, z);
        self.state.registry.insert(
            entity,
            Graphic {
                id: record.graphic,
                hue: record.hue,
            },
        );
        self.state.registry.insert(entity, Position(position));
        self.state.registry.insert(entity, Facet(facet));
        if record.amount > 1 {
            self.state.registry.insert(entity, Amount(record.amount));
        }
        if record.stackable {
            self.state.registry.insert(entity, Stackable);
        }
        if let Some(gump) = record.container_gump {
            self.state.registry.insert(entity, Container { gump });
        }
        // Loose clutter resumes rotting; a container does not (mark_decay skips it).
        items::mark_decay(&mut self.state, entity);
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, position);
    }

    /// Equip a logging-in character's saved inventory, if any is waiting.
    ///
    /// Two passes so nesting resolves whatever order the records are in: first
    /// spawn every item bound to its saved serial with its graphic and container
    /// mark, then place each — worn on the mobile, or inside the container its
    /// record names, now that every container entity exists. Returns whether an
    /// inventory was restored, so [`enter`](Self::enter) knows not to hand out a
    /// starter backpack.
    fn restore_inventory(&mut self, owner: u32) -> bool {
        let Some(records) = self.pending_inventories.remove(&owner) else {
            return false;
        };
        // Pass one: the entities, so a container exists before its contents point
        // at it.
        for record in &records {
            let Some(serial) = Serial::new(record.serial) else {
                continue;
            };
            let entity = self.state.registry.spawn();
            if self.state.registry.bind_serial(entity, serial).is_err() {
                self.state.registry.despawn(entity);
                continue;
            }
            self.state.registry.insert(
                entity,
                Graphic {
                    id: record.graphic,
                    hue: record.hue,
                },
            );
            if record.amount > 1 {
                self.state.registry.insert(entity, Amount(record.amount));
            }
            if record.stackable {
                self.state.registry.insert(entity, Stackable);
            }
            if let Some(gump) = record.container_gump {
                self.state.registry.insert(entity, Container { gump });
            }
        }
        // Pass two: where each item goes.
        for record in &records {
            let Some(entity) =
                Serial::new(record.serial).and_then(|s| self.state.registry.entity_of(s))
            else {
                continue;
            };
            match record.location {
                ItemLocation::Equipped { mobile, layer } => {
                    if let Some(mobile) = Serial::new(mobile) {
                        self.state
                            .registry
                            .insert(entity, Equipped { mobile, layer });
                    }
                }
                ItemLocation::Contained {
                    container,
                    x,
                    y,
                    grid,
                } => {
                    if let Some(container) = Serial::new(container) {
                        self.state.registry.insert(
                            entity,
                            Contained {
                                container,
                                x,
                                y,
                                grid,
                            },
                        );
                    }
                }
                // An owned item is never on the ground; ignore a stray one rather
                // than drop it into the world at 0,0.
                ItemLocation::Ground { .. } => {}
            }
        }
        true
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
                access,
            } => self.enter(Entering {
                connection,
                version,
                account,
                name,
                serial,
                position,
                facet,
                appearance,
                access,
            }),
            Command::Walk {
                connection,
                request,
            } => self.walk(connection, request, now),
            Command::RequestStatus { connection } => {
                if let Some(&entity) = self.state.players.get(&connection) {
                    self.send_status(connection, entity);
                }
            }
            Command::GumpResponse {
                connection,
                response,
            } => self.handle_admin_gump(connection, response),
            Command::TargetResponse {
                connection,
                response,
            } => self.handle_target(connection, response),
            Command::RegisterSpawner { spawner } => self.register_spawner(spawner),
            Command::ClearSpawners => self.clear_spawners(),
            Command::Decorate {
                facet,
                statics,
                doors,
                containers,
            } => self.decorate(facet, &statics, &doors, &containers),
            Command::GenerateDoors {
                facet,
                x,
                y,
                width,
                height,
            } => self.generate_doors(facet, x, y, width, height),
            Command::ClearDecorations => self.clear_decorations(),
            Command::Step { serial, direction } => self.step(serial, direction),
            Command::SpawnItem {
                graphic,
                hue,
                amount,
                stackable,
                position,
                facet,
            } => {
                items::spawn_item(
                    &mut self.state,
                    graphic,
                    hue,
                    amount,
                    stackable,
                    position,
                    facet,
                );
            }
            Command::SpawnContainer {
                graphic,
                gump,
                hue,
                position,
                facet,
            } => items::spawn_container(&mut self.state, graphic, gump, hue, position, facet),
            Command::SpawnMobile {
                body,
                hue,
                hits,
                notoriety,
                damage,
                resistance,
                swing,
                sight,
                wander,
                position,
                facet,
                name,
                banker,
                equipment,
            } => {
                self.spawn_mobile(SpawnMobile {
                    body,
                    hue,
                    hits,
                    notoriety,
                    damage,
                    resistance,
                    swing,
                    sight,
                    wander,
                    position,
                    facet,
                    name,
                    banker,
                    equipment,
                });
            }
            Command::Damage {
                serial,
                amount,
                damage_type,
                by,
            } => combat::damage(
                &mut self.state,
                serial,
                amount,
                DamageType::from_u8(damage_type),
                openshard_entities::Serial::new(by),
            ),
            Command::CastSpell {
                serial,
                spell,
                target,
                mana,
                difficulty,
                skill,
                pack,
                reagents,
            } => magic::cast_spell(
                &mut self.state,
                magic::Cast {
                    serial,
                    spell,
                    target,
                    mana,
                    difficulty,
                    skill,
                    pack,
                    reagents: &reagents,
                },
            ),
            Command::Heal { serial, amount } => magic::heal(&mut self.state, serial, amount),
            Command::SetStats {
                serial,
                strength,
                dexterity,
                intelligence,
            } => skills::set_stats(&mut self.state, serial, strength, dexterity, intelligence),
            Command::SetSkill {
                serial,
                skill,
                value,
            } => skills::set_skill(&mut self.state, serial, skill, value),
            Command::UseSkill {
                serial,
                skill,
                difficulty,
            } => skills::use_skill(&mut self.state, serial, skill, difficulty),
            Command::WarMode { connection, war } => {
                combat::war_mode(&mut self.state, connection, war)
            }
            Command::Attack { connection, target } => {
                combat::attack(&mut self.state, connection, target)
            }
            Command::Say {
                connection,
                mode,
                hue,
                font,
                text,
            } => self.say(connection, mode, hue, font, text),
            Command::Speak { serial, hue, text } => {
                if let Some(entity) =
                    Serial::new(serial).and_then(|s| self.state.registry.entity_of(s))
                {
                    chat::speak(&mut self.state, entity, 0, hue, chat::DEFAULT_FONT, &text);
                }
            }
            Command::DoubleClick { connection, serial } => {
                items::double_click(&mut self.state, connection, serial)
            }
            Command::SingleClick { connection, serial } => self.single_click(connection, serial),
            Command::EquipItem {
                connection,
                item,
                layer,
                mobile,
            } => items::equip_item(&mut self.state, connection, item, layer, mobile),
            Command::PickUpItem {
                connection,
                serial,
                amount,
            } => items::pick_up(&mut self.state, connection, serial, amount),
            Command::DropItem {
                connection,
                serial,
                position,
                container,
            } => items::drop_item(&mut self.state, connection, serial, position, container),
            Command::Disconnect { connection } => self.disconnect(connection),
            Command::Control { serial } => self.control(serial),
            Command::RequestCast { connection, spell } => self.request_cast(connection, spell),
        }
    }

    /// The facet a mobile is on, or the default if it carries none.
    ///
    /// Always a facet the world actually has: [`enter`](Self::enter) clamps an
    /// unloaded facet to the default before it ever reaches a `Facet` component,
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
            access,
        } = entering;
        if self.state.players.contains_key(&connection) {
            warn!(%connection, "already in the world");
            return;
        }

        // A character can only stand on a facet the shard loaded. An unloaded one
        // — a save from a shard that had more facets, say — falls back to the
        // default rather than leaving the character nowhere.
        let facet = if self.state.facets.contains_key(&facet) {
            facet
        } else {
            warn!(%connection, facet, "unloaded facet; falling back to the default");
            self.state.default_facet
        };

        // A stored character comes back on the serial it was saved under; a new
        // one takes a fresh serial from the pool. The saved serial was reserved
        // at boot (see `World::reserve_serial`), so binding it here cannot collide.
        let (entity, serial) = match serial.and_then(Serial::new) {
            Some(saved) => {
                let entity = self.state.registry.spawn();
                if let Err(error) = self.state.registry.bind_serial(entity, saved) {
                    warn!(%connection, ?error, "could not restore the saved serial");
                    self.state.registry.despawn(entity);
                    return;
                }
                (entity, saved)
            }
            None => match self.state.registry.spawn_with_serial(SerialKind::Mobile) {
                Ok(pair) => pair,
                Err(_) => {
                    warn!(%connection, "the mobile serial pool is exhausted");
                    return;
                }
            },
        };

        // A loaded character spawns exactly where it was saved, its own z
        // included; a fresh one takes the world's configured start on its facet.
        let position = position.unwrap_or_else(|| self.state.start_position(facet));
        let facing = Facing::walking(Direction::South);
        // A created or loaded character brings its body and hue; without one it
        // falls back to the default.
        let body = Body {
            id: appearance.map_or(BODY_HUMAN_MALE, |look| look.body),
            hue: appearance.map_or(DEFAULT_HUE, |look| look.hue),
        };

        self.state.registry.insert(entity, Position(position));
        self.state.registry.insert(entity, Heading(facing));
        self.state.registry.insert(entity, body);
        self.state.registry.insert(entity, Name(name.clone()));
        self.state.registry.insert(entity, Account(account));
        self.state.registry.insert(entity, Facet(facet));
        // The account's authority, re-derived each login and never saved with the
        // character — so it is what the GM command gate reads.
        self.state.registry.insert(entity, Access(access));
        // Strength caps hit points, intelligence caps mana — the first derived
        // numbers. Character creation will choose the stats; until it does, the
        // defaults reproduce the flat hundreds the world had before.
        self.state.registry.insert(
            entity,
            Stats {
                strength: DEFAULT_HITPOINTS,
                dexterity: DEFAULT_DEXTERITY,
                intelligence: DEFAULT_MANA,
            },
        );
        self.state.registry.insert(
            entity,
            Hitpoints {
                current: DEFAULT_HITPOINTS,
                max: DEFAULT_HITPOINTS,
            },
        );
        self.state.registry.insert(
            entity,
            Mana {
                current: DEFAULT_MANA,
                max: DEFAULT_MANA,
            },
        );
        self.state.registry.insert(entity, Combat::default());
        self.state.registry.insert(entity, Notoriety::Innocent);
        self.state.registry.insert(
            entity,
            MeleeDamage {
                amount: combat::SWING_DAMAGE,
            },
        );
        self.state.registry.insert(entity, Resistance::default());
        // No explicit `SwingSpeed`: a player swings at the pace their dexterity
        // dictates, through `swing_speed`.
        self.state
            .registry
            .insert(entity, Movement(Walker::new(position, facing)));
        self.state.registry.insert(
            entity,
            Client {
                connection,
                version,
            },
        );
        self.state.players.insert(connection, entity);
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, position);
        self.state.seen.insert(entity, HashSet::new());

        // Bring back what this character was carrying, if the store had it. A
        // returning character re-equips its saved backpack, bank box and gear; a
        // new one has nothing waiting.
        let restored = self.restore_inventory(serial.raw());

        // Every character wears a backpack. Without it the paperdoll's bag is dead
        // and there is nowhere to put anything picked up. Equipped before the
        // packets go out so it rides in the `0x78` that tells the client — and
        // everyone watching — what this mobile is wearing. A returning character's
        // backpack came back with its inventory; only a character that restored
        // none — a brand-new one, or one whose save predates item persistence —
        // gets a fresh starter bag.
        let has_backpack = self
            .state
            .registry
            .query::<Equipped>()
            .any(|(_, worn)| worn.mobile == serial && worn.layer == BACKPACK_LAYER);
        if !restored || !has_backpack {
            items::equip_new_container(
                &mut self.state,
                serial,
                BACKPACK_GRAPHIC,
                BACKPACK_GUMP,
                0,
                BACKPACK_LAYER,
            );
        }

        // And a bank box, on the bank layer. Like the backpack it is worn, so it
        // persists with the character and its contents survive a restart — which is
        // what makes a bank worth anything. A returning character's came back with
        // its saved inventory; a new one gets an empty one.
        let has_bank = self
            .state
            .registry
            .query::<Equipped>()
            .any(|(_, worn)| worn.mobile == serial && worn.layer == npc::BANK_LAYER);
        if !has_bank {
            items::equip_new_container(
                &mut self.state,
                serial,
                npc::BANK_GRAPHIC,
                npc::BANK_GUMP,
                0,
                npc::BANK_LAYER,
            );
        }

        // The order is the client's, not ours. 0x1B must come first — until it
        // lands there is no body to attach anything to — and 0x55 must come
        // last, because it is what tells the client to start drawing. What is
        // between can be reordered; the two ends cannot.
        self.state.send(
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
        self.state.send(connection, encode_map_change(facet));
        self.state.send(
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
        self.state.send(connection, encode_light_level(LIGHT_DAY));
        // The status bar, stamina and all. Without it the client believes it has
        // zero stamina and refuses to run — see `MobileStatus`. Sent before the
        // login-complete that starts the client drawing, so the numbers are there
        // the moment the paperdoll can be opened.
        self.send_status(connection, entity);
        // The player's own `0x78`, so its client learns its equipment — and the
        // serial of the backpack it must be able to double-click open. The client
        // draws its body from `0x1B`, but its worn items come from here; `reveal`
        // sends this mobile to *others*, never to itself, so this is the one place
        // it hears about its own paperdoll.
        if let Some(mine) = self.state.mobile_incoming(entity) {
            self.state.send(connection, mine.encode(version));
        }
        self.state.send(connection, encode_login_complete());

        self.state.bus.send(PlayerEntered {
            entity,
            serial,
            position,
        });
        info!(%serial, name, position = %position, "in world");

        // Draw whoever is already here, and draw this one for them. Both
        // directions, because arriving is symmetric: the newcomer has an empty
        // screen and everyone nearby has a gap where it now stands.
        self.state.refresh_around(entity);
    }

    /// Send a player its own `0x11` status — the paperdoll numbers, and the only
    /// packet that carries stamina. A client with no status believes it has zero
    /// stamina and will only ever walk, so this goes out on world entry and again
    /// whenever the client asks (`0x34`). Reads the mobile's own components;
    /// stamina tracks dexterity, as it does in UO, until a stamina system exists.
    fn send_status(&mut self, connection: ConnectionId, entity: EntityId) {
        let Some(Client { version, .. }) = self.state.registry.get::<Client>(entity).copied()
        else {
            return;
        };
        let Some(serial) = self.state.registry.serial_of(entity) else {
            return;
        };
        let name = self
            .state
            .registry
            .get::<Name>(entity)
            .map_or_else(String::new, |n| n.0.clone());
        let stats = self.state.registry.get::<Stats>(entity).copied();
        let hits = self.state.registry.get::<Hitpoints>(entity).copied();
        let mana = self.state.registry.get::<Mana>(entity).copied();
        let (strength, dexterity, intelligence) = stats
            .map_or((DEFAULT_HITPOINTS, DEFAULT_DEXTERITY, DEFAULT_MANA), |s| {
                (s.strength, s.dexterity, s.intelligence)
            });
        let (hits_now, hits_max) = hits.map_or((DEFAULT_HITPOINTS, DEFAULT_HITPOINTS), |h| {
            (h.current, h.max)
        });
        let (mana_now, mana_max) =
            mana.map_or((DEFAULT_MANA, DEFAULT_MANA), |m| (m.current, m.max));

        let status = MobileStatus {
            serial: serial.raw(),
            name,
            hits: hits_now,
            hits_max,
            female: false,
            strength,
            dexterity,
            intelligence,
            // Stamina is dexterity's pool and starts full — anything less than max
            // here needlessly slows the first run out of the gate.
            stamina: dexterity,
            stamina_max: dexterity,
            mana: mana_now,
            mana_max,
            gold: 0,
            armor: 0,
            // A body's own weight, well under the cap: an overloaded client will
            // not run either, so this is deliberately light until an inventory
            // weight system replaces it.
            weight: BODY_WEIGHT,
            max_weight: max_weight(strength),
            stat_cap: STAT_CAP,
            followers: 0,
            followers_max: MAX_FOLLOWERS,
        };
        self.state.send(connection, status.encode(version));
    }

    /// A player's speech, with staff commands split off the front. A
    /// `.`-prefixed line from a game master runs as a command and never reaches
    /// anyone's screen; from an ordinary player it is just speech, so a player can
    /// still say ".hello" out loud. The authority gate lives here, not in `gm`,
    /// so the command module can assume a call is already cleared.
    fn say(&mut self, connection: ConnectionId, mode: u8, hue: u16, font: u16, text: String) {
        if let Some(rest) = text.strip_prefix(gm::COMMAND_PREFIX) {
            if let Some(&actor) = self.state.players.get(&connection) {
                let is_gm = self
                    .state
                    .registry
                    .get::<Access>(actor)
                    .is_some_and(|access| access.0 >= AccessLevel::GameMaster);
                if is_gm {
                    gm::run(&mut self.state, actor, rest);
                    return;
                }
            }
        }
        chat::say(&mut self.state, connection, mode, hue, font, &text);

        // Townsperson services triggered by keyword: saying "bank" near a banker
        // opens your bank box. The words were still spoken above, so it reads as a
        // request the banker answers, not a hidden command.
        if let Some(&actor) = self.state.players.get(&connection) {
            npc::banker_keywords(&mut self.state, connection, actor, &text);
        }
    }

    /// Answer a single-click (`0x09`): draw the clicked mobile's name over its
    /// head, seen only by the asker, in its notoriety colour.
    ///
    /// Mobiles with a name only — a townsperson, a player. A nameless creature and
    /// a plain item say nothing rather than a blank label; item names wait on a
    /// tiledata name lookup.
    fn single_click(&mut self, connection: ConnectionId, serial: u32) {
        let Some(target) = Serial::new(serial).and_then(|s| self.state.registry.entity_of(s))
        else {
            return;
        };
        let Some(name) = self.state.registry.get::<Name>(target) else {
            return;
        };
        let name = name.0.clone();
        let Some(body) = self.state.registry.get::<Body>(target).map(|b| b.id) else {
            return;
        };
        let hue = self
            .state
            .registry
            .get::<Notoriety>(target)
            .copied()
            .unwrap_or(Notoriety::Innocent)
            .name_hue();
        // The object's own serial makes the client draw the text over it; an empty
        // speaker name and the label mode make it a name tag, not speech.
        let packet = encode_message(serial, body, LABEL_MODE, hue, 3, "", &name);
        self.state.send(connection, packet);
    }

    fn walk(&mut self, connection: ConnectionId, request: WalkRequest, now: Instant) {
        let Some(&entity) = self.state.players.get(&connection) else {
            // A walk before a character. Not fatal — a stray packet from a
            // client that reconnected — but nothing to act on either.
            debug!(%connection, "0x02 from a connection with no character");
            return;
        };
        let Some(serial) = self.state.registry.serial_of(entity) else {
            return;
        };
        let Some(Movement(mut walker)) = self.state.registry.get::<Movement>(entity).copied()
        else {
            return;
        };

        let facet = self.state.facet_of(entity);
        let was = walker.position;
        let out_of_sequence = walker.sequence.is_fresh() && request.sequence != 0;
        let outcome = match &self.state.facet_state(facet).terrain {
            Some(terrain) => walker.request(request, terrain.as_ref(), now),
            None => walker.request(request, &OpenWorld, now),
        };
        self.state.registry.insert(entity, Movement(walker));

        match outcome {
            Walk::Moved { position, facing } => {
                self.state.registry.insert(entity, Position(position));
                self.state.registry.insert(entity, Heading(facing));
                // The index is a second copy of the position; this is the line
                // that keeps it honest.
                self.state
                    .facet_state_mut(facet)
                    .sectors
                    .insert(entity, position);
                self.state.send(
                    connection,
                    encode_walk_ack(request.sequence, NOTORIETY_INNOCENT),
                );
                self.state.bus.send(MobileMoved {
                    entity,
                    serial,
                    from: was,
                    to: position,
                    facing,
                });
                self.state.refresh_around(entity);
            }
            Walk::Turned { facing } => {
                self.state.registry.insert(entity, Heading(facing));
                self.state.send(
                    connection,
                    encode_walk_ack(request.sequence, NOTORIETY_INNOCENT),
                );
                self.state.bus.send(MobileTurned {
                    entity,
                    serial,
                    facing,
                });
                // A turn moves nobody, but it changes what everyone watching
                // draws — the client animates a facing it is told about.
                self.state.broadcast_move(entity);
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
                self.state.send(
                    connection,
                    encode_walk_reject(request.sequence, walker.position, walker.facing),
                );
                self.state.bus.send(StepRefused {
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
        let Some(entity) = self.state.registry.entity_of(serial) else {
            return;
        };
        let Some(Movement(mut walker)) = self.state.registry.get::<Movement>(entity).copied()
        else {
            return;
        };
        let direction = Direction::from_bits(direction);
        let facet = self.state.facet_of(entity);
        let was = walker.position;

        // Turn-as-step: a mobile not yet facing this way turns and stays put.
        if walker.facing.direction != direction {
            let facing = Facing::walking(direction);
            walker.facing = facing;
            self.state.registry.insert(entity, Movement(walker));
            self.state.registry.insert(entity, Heading(facing));
            self.state.bus.send(MobileTurned {
                entity,
                serial,
                facing,
            });
            self.state.broadcast_move(entity);
            return;
        }

        let Some(target) = step_from(walker.position, direction) else {
            // Off the edge of the coordinate space — nowhere to go, and no client
            // to snap back, so it is simply refused.
            self.state.bus.send(StepRefused {
                entity,
                serial,
                reason: RefusedReason::Blocked,
            });
            return;
        };
        let landed = match &self.state.facet_state(facet).terrain {
            Some(terrain) => terrain.can_step(walker.position, target),
            None => OpenWorld.can_step(walker.position, target),
        };
        let Some(landed) = landed else {
            self.state.bus.send(StepRefused {
                entity,
                serial,
                reason: RefusedReason::Blocked,
            });
            return;
        };

        let facing = Facing::walking(direction);
        walker.position = landed;
        walker.facing = facing;
        self.state.registry.insert(entity, Movement(walker));
        self.state.registry.insert(entity, Position(landed));
        self.state.registry.insert(entity, Heading(facing));
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, landed);
        self.state.bus.send(MobileMoved {
            entity,
            serial,
            from: was,
            to: landed,
            facing,
        });
        self.state.refresh_around(entity);
    }

    /// Put a mobile in the world. See [`Command::SpawnMobile`].
    ///
    /// The same bundle a player is built from — a body, a position, a facing, a
    /// walker, hit points — minus the [`Client`]. That absence is the whole
    /// difference between a creature and a person; everything that draws or moves
    /// a mobile already treats "has a client" as the question, so a spawned one
    /// falls out of the machinery already there.
    #[allow(clippy::too_many_arguments)]
    fn spawn_mobile(&mut self, spec: SpawnMobile) -> Option<EntityId> {
        let SpawnMobile {
            body,
            hue,
            hits,
            notoriety,
            damage,
            resistance,
            swing,
            sight,
            wander,
            position,
            facet,
            name,
            banker,
            equipment,
        } = spec;
        let facet = if self.state.facets.contains_key(&facet) {
            facet
        } else {
            warn!(facet, "unloaded facet; spawning the mobile on the default");
            self.state.default_facet
        };
        // Drop the mobile onto the ground, the way a client's spawner does: the
        // pack gives x/y and a rough height, and the floor it stands on — the top
        // of the static surface there, a building's raised floor and all — is the
        // map's to say. Without this a banker sinks to the given z and reads as
        // "inside a wall".
        let position = match self
            .state
            .facet_state(facet)
            .terrain
            .as_ref()
            .and_then(|t| t.stand_z(position.x, position.y, i32::from(position.z)))
            .and_then(|z| i8::try_from(z).ok())
        {
            Some(z) => Point::new(position.x, position.y, z),
            None => position,
        };
        let (entity, serial) = match self.state.registry.spawn_with_serial(SerialKind::Mobile) {
            Ok(pair) => pair,
            Err(error) => {
                warn!(?error, "out of mobile serials; not spawning");
                return None;
            }
        };
        let hits = hits.max(1);
        let facing = Facing::walking(Direction::South);
        self.state.registry.insert(entity, Body { id: body, hue });
        self.state.registry.insert(entity, Position(position));
        self.state.registry.insert(entity, Heading(facing));
        self.state.registry.insert(entity, Facet(facet));
        self.state.registry.insert(
            entity,
            Hitpoints {
                current: hits,
                max: hits,
            },
        );
        self.state
            .registry
            .insert(entity, Notoriety::from_bits(notoriety));
        self.state
            .registry
            .insert(entity, MeleeDamage { amount: damage });
        self.state.registry.insert(
            entity,
            Resistance {
                physical: resistance.min(100),
                ..Default::default()
            },
        );
        // Zero means "derive from dexterity", so a script that does not care about
        // pace names no number and gets the wrestling formula. A non-zero value
        // pins an exact cadence — a special creature that ignores its stats.
        if swing != 0 {
            self.state
                .registry
                .insert(entity, SwingSpeed { ticks: swing });
        }
        // A brain only for a creature that needs one — something that hunts or
        // wanders. A pure prop (a shopkeeper standing still) gets none and never
        // enters `think`. `Combat` it earns when it first picks a fight.
        if sight > 0 || wander {
            self.state.registry.insert(
                entity,
                Brain {
                    sight,
                    wander,
                    next_think: 0,
                },
            );
        }
        // A banker earns a generated name and title ("Rowena the banker") when the
        // spawn did not name it, the townsperson AI base (so it greets, faces and
        // keeps near its post), and the service mark that answers "bank".
        let name = if banker && name.is_none() {
            Some(npc::banker_name(&mut self.state.rng))
        } else {
            name
        };
        if let Some(name) = name {
            self.state.registry.insert(entity, Name(name));
        }
        if banker {
            self.state.registry.insert(entity, Banker { next_greet: 0 });
            self.state.registry.insert(
                entity,
                Npc {
                    home: position,
                    wander: BANKER_WANDER,
                    next_beat: 0,
                },
            );
        }
        // Dress it before the reveal, so the clothing rides in the `0x78` that
        // draws it — a naked banker is a bug that looks like nudity.
        for (graphic, layer, item_hue) in equipment {
            items::equip_worn_item(&mut self.state, serial, graphic, item_hue, layer);
        }
        self.state
            .registry
            .insert(entity, Movement(Walker::new(position, facing)));
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, position);
        self.state.reveal(entity);
        // Say who and where, so a script can take control of it: the mobile
        // counterpart of `PlayerEntered`, and how `op_control` learns a serial.
        self.state.bus.send(MobileSpawned {
            entity,
            serial,
            position,
        });
        debug!(%serial, body, "mobile spawned");
        Some(entity)
    }

    /// Act on a targeting cursor's answer. Looks up what the cursor was raised for
    /// and, if the click was not cancelled, does it. A cancel just clears the
    /// pending target.
    fn handle_target(
        &mut self,
        connection: ConnectionId,
        response: openshard_protocol::TargetResponse,
    ) {
        let Some(&actor) = self.state.players.get(&connection) else {
            return;
        };
        let Some(purpose) = self.state.pending_targets.remove(&actor) else {
            return; // no cursor was up for this mobile
        };
        if response.cancelled {
            return;
        }
        match purpose {
            openshard_state::TargetPurpose::Teleport => {
                crate::gm::teleport_to(&mut self.state, actor, response.location);
            }
        }
    }

    /// Act on an admin-gump button. The gump crate reads the response and gates it
    /// (game-master only); the acting — registering or clearing spawn regions,
    /// which only the tick can touch — is here.
    fn handle_admin_gump(
        &mut self,
        connection: ConnectionId,
        response: openshard_protocol::GumpResponse,
    ) {
        let Some((actor, verb)) = crate::admin::button_action(&self.state, connection, &response)
        else {
            return;
        };
        // The engine holds no spawn data: it emits the verb, and the script pack —
        // where a shard's spawns are edited without a rebuild — decides what it
        // means, registering regions through `op_register_spawner` or clearing them.
        if let Some(serial) = self.state.registry.serial_of(actor) {
            self.state.bus.send(AdminMenuAction {
                serial,
                action: verb.to_owned(),
            });
        }
        gm::notify(&mut self.state, actor, &format!("Admin: {verb}."));
    }

    /// Keep every spawn region at its ceiling. Once per tick, but cheap: a region
    /// not yet due to respawn is a single counter check, and only a due one that
    /// is short a creature does the work of counting and spawning. One creature
    /// per region per pass, so a wiped region refills at its own pace rather than
    /// snapping back full in a tick. Deterministic — the picks draw on the world's
    /// seeded rng, so a replay repopulates the same.
    fn maintain_spawners(&mut self) {
        let now = self.state.ticks;
        for index in 0..self.spawners.len() {
            if now < self.spawners[index].next_spawn {
                continue;
            }
            let id = index as u32;
            let live = self
                .state
                .registry
                .query::<SpawnedBy>()
                .filter(|(_, owner)| owner.0 == id)
                .count() as u16;
            let spawner = &self.spawners[index];
            if spawner.creatures.is_empty() || live >= spawner.max_count {
                continue;
            }

            // Pick a creature and a tile with the tick's rng.
            let area = spawner.area;
            let which = self.state.rng.below(spawner.creatures.len() as u32) as usize;
            let creature = spawner.creatures[which].clone();
            let delay = spawner.respawn_delay;
            let facet = area.facet;
            let dx = self.state.rng.below(u32::from(area.width.max(1)));
            let dy = self.state.rng.below(u32::from(area.height.max(1)));
            let x = area.x.wrapping_add(dx as u16);
            let y = area.y.wrapping_add(dy as u16);

            // Stand it on the ground the client will compute, or a flat default
            // where there is no map.
            let z = self
                .state
                .facet_state(facet)
                .terrain
                .as_ref()
                .and_then(|terrain| terrain.ground_z(x, y))
                .unwrap_or(0);

            if let Some(entity) = self.spawn_mobile(SpawnMobile {
                body: creature.body,
                hue: creature.hue,
                hits: creature.hits,
                notoriety: creature.notoriety,
                damage: creature.damage,
                resistance: creature.resistance,
                swing: creature.swing,
                sight: creature.sight,
                wander: creature.wander,
                position: Point::new(x, y, z),
                facet,
                // A maintained spawn is a monster or an animal, never a named
                // townsperson; those are placed once, not respawned.
                name: None,
                banker: false,
                equipment: Vec::new(),
            }) {
                self.state.registry.insert(entity, SpawnedBy(id));
            }
            self.spawners[index].next_spawn = now + delay;
        }
    }

    /// Drop every spawn region and despawn the creatures they were maintaining —
    /// Register a spawn region, giving it a fresh id and replacing any earlier one
    /// over the same box. Re-running the pack's "populate" does not stack a second
    /// spawner on a region — it re-places it, with a reset timer — and after a
    /// restart the regions come from the store, not from here, so their timers hold.
    fn register_spawner(&mut self, mut spawner: crate::spawner::Spawner) {
        self.spawners.retain(|s| s.area != spawner.area);
        spawner.id = self.next_spawner_id;
        self.next_spawner_id += 1;
        self.spawners.push(spawner);
    }

    /// The spawn regions as saveable records. The live timer is a tick count; it is
    /// saved as the *seconds still to wait*, so it means the same after a restart
    /// resets the tick counter — a rare spawn killed with hours left comes back with
    /// those hours ahead of it, and downtime does not spend them.
    fn spawner_records(&self) -> Vec<openshard_persistence::SpawnerRecord> {
        let now = self.state.ticks;
        self.spawners
            .iter()
            .map(|s| openshard_persistence::SpawnerRecord {
                id: s.id,
                facet: s.area.facet,
                x: s.area.x,
                y: s.area.y,
                width: s.area.width,
                height: s.area.height,
                max_count: s.max_count,
                respawn_secs: s.respawn_delay / TICKS_PER_SECOND,
                remaining_secs: s.next_spawn.saturating_sub(now) / TICKS_PER_SECOND,
                creatures: s
                    .creatures
                    .iter()
                    .map(|c| openshard_persistence::CreatureData {
                        body: c.body,
                        hue: c.hue,
                        hits: c.hits,
                        notoriety: c.notoriety,
                        damage: c.damage,
                        resistance: c.resistance,
                        swing: c.swing,
                        sight: c.sight,
                        wander: c.wander,
                    })
                    .collect(),
            })
            .collect()
    }

    /// Re-create the spawn regions from saved records at boot. The remaining-wait
    /// seconds become a tick offset from now (the tick counter is zero at boot), so
    /// the timer resumes where it stood; downtime is not counted against it. Call
    /// once, before anyone connects.
    pub fn restore_spawners(&mut self, records: Vec<openshard_persistence::SpawnerRecord>) {
        let now = self.state.ticks;
        for record in records {
            self.next_spawner_id = self.next_spawner_id.max(record.id + 1);
            let area = crate::spawner::SpawnArea {
                x: record.x,
                y: record.y,
                width: record.width,
                height: record.height,
                facet: record.facet,
            };
            let creatures = record
                .creatures
                .into_iter()
                .map(|c| crate::spawner::CreatureTemplate {
                    body: c.body,
                    hue: c.hue,
                    hits: c.hits,
                    notoriety: c.notoriety,
                    damage: c.damage,
                    resistance: c.resistance,
                    swing: c.swing,
                    sight: c.sight,
                    wander: c.wander,
                })
                .collect();
            let mut spawner = crate::spawner::Spawner::new(
                record.id,
                area,
                creatures,
                record.max_count,
                record.respawn_secs * TICKS_PER_SECOND,
            );
            spawner.next_spawn = now + record.remaining_secs * TICKS_PER_SECOND;
            self.spawners.push(spawner);
        }
    }

    /// "Clear spawns". A creature belongs to a region by its [`SpawnedBy`]; taking
    /// it off every screen before despawning, so no client is left drawing a ghost.
    fn clear_spawners(&mut self) {
        self.spawners.clear();
        let owned: Vec<EntityId> = self
            .state
            .registry
            .query::<SpawnedBy>()
            .map(|(entity, _)| entity)
            .collect();
        for entity in owned {
            let serial = self.state.registry.serial_of(entity);
            let facet = self.state.facet_of(entity);
            if let Some(serial) = serial {
                for watcher in self.state.watchers_of(entity) {
                    self.state.forget(watcher, entity, serial);
                }
            }
            self.state.facet_state_mut(facet).sectors.remove(entity);
            self.state.registry.despawn(entity);
        }
    }

    /// Place a batch of decoration: script-added statics the shard puts on top of
    /// the map's art, plus the interactive kinds — doors and containers. Each is an
    /// item — a `Graphic` and a `Position`, drawn to clients through the same
    /// `0x1A`/interest path as any item — but marked [`Decoration`], so it never
    /// decays and cannot be picked up. A door also carries [`Door`] (toggled by
    /// double-click) and a container [`Container`] (opened by double-click). See
    /// [`crate::gm`] and `items::pick_up`.
    fn decorate(
        &mut self,
        facet: u8,
        statics: &[(u16, u16, Point)],
        doors: &[DecorDoor],
        containers: &[DecorContainer],
    ) {
        let facet = if self.state.facets.contains_key(&facet) {
            facet
        } else {
            self.state.default_facet
        };
        // A closure that spawns one decoration item at a tile and reveals it,
        // returning the entity so the caller can hang a `Door` or `Container` on
        // it. `None` when the serial pool is empty.
        for &(graphic, hue, position) in statics {
            if self
                .place_decoration(facet, graphic, hue, position)
                .is_none()
            {
                return;
            }
        }
        for door in doors {
            let Some(entity) = self.place_decoration(facet, door.closed, 0, door.position) else {
                return;
            };
            self.state.registry.insert(
                entity,
                Door {
                    closed: door.closed,
                    open: door.open,
                    offset_x: door.offset_x,
                    offset_y: door.offset_y,
                    is_open: false,
                    close_at: 0,
                },
            );
        }
        for container in containers {
            let Some(entity) =
                self.place_decoration(facet, container.graphic, container.hue, container.position)
            else {
                return;
            };
            self.state.registry.insert(
                entity,
                Container {
                    gump: container.gump,
                },
            );
        }
    }

    /// Spawn one decoration item — a `Graphic`, `Position`, `Facet` and the
    /// [`Decoration`] marker — index it and draw it to everyone in range. Returns
    /// its entity, or `None` if the item-serial pool is empty (the caller stops the
    /// batch).
    fn place_decoration(
        &mut self,
        facet: u8,
        graphic: u16,
        hue: u16,
        position: Point,
    ) -> Option<EntityId> {
        let Ok((entity, _serial)) = self.state.registry.spawn_with_serial(SerialKind::Item) else {
            warn!("out of item serials; stopping decoration");
            return None;
        };
        self.state
            .registry
            .insert(entity, Graphic { id: graphic, hue });
        self.state.registry.insert(entity, Position(position));
        self.state.registry.insert(entity, Facet(facet));
        self.state.registry.insert(entity, Decoration);
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, position);
        self.state.reveal(entity);
        Some(entity)
    }

    /// Generate functional doors from the map's static door frames in a region.
    ///
    /// ServUO's `DoorGenerator`, ported (see [`crate::doorgen`]): where a west
    /// frame faces an east frame across a one- or two-tile gap — or a north faces a
    /// south — a `DarkWoodDoor` (single) or a linked pair (double) is dropped into
    /// the gap, so a building's implied shop door becomes one that opens. Reading
    /// the terrain and placing entities cannot overlap borrows, so the scan
    /// collects every placement first and lays them down after.
    fn generate_doors(&mut self, facet: u8, x: u16, y: u16, width: u16, height: u16) {
        let facet = if self.state.facets.contains_key(&facet) {
            facet
        } else {
            self.state.default_facet
        };

        // Tiles that already hold a door — the named metal/special doors placed
        // from decoration data, and doors generated earlier in this same pass. A
        // generated door never lands on one of these, which is what stops the bank
        // door being doubled and a doorway being filled twice.
        let door_entities: Vec<EntityId> = self
            .state
            .registry
            .query::<Door>()
            .map(|(entity, _)| entity)
            .collect();
        let mut occupied: HashSet<(u16, u16)> = HashSet::new();
        for entity in door_entities {
            if self.state.facet_of(entity) == facet {
                if let Some(&Position(p)) = self.state.registry.get::<Position>(entity) {
                    occupied.insert((p.x, p.y));
                }
            }
        }

        // (closed, open, offset_x, offset_y, where-it-sits-closed).
        let mut placements: Vec<(u16, u16, i16, i16, Point)> = Vec::new();
        {
            let Some(terrain) = self.state.facet_state(facet).terrain.as_ref() else {
                warn!(facet, "no map on this facet; no doors to generate");
                return;
            };
            // Is there a frame of the given side at (tx, ty) sharing height z?
            let frame_at = |tx: u16, ty: u16, tz: i8, pred: fn(u16) -> bool| -> bool {
                let mut here = Vec::new();
                terrain.statics_at(tx, ty, &mut here);
                here.iter().any(|&(id, z)| z == tz && pred(id))
            };
            // Place a door in the gap, but only if a door actually fits there — an
            // open doorway with a floor, not a solid wall or thin air — and it is
            // not already doored. `can_fit` is ServUO's `CanFit` guard (16 tall);
            // the `occupied` set is our own de-dup.
            let mut try_place = |gap: Point, door: (u16, u16, i16, i16)| {
                let key = (gap.x, gap.y);
                if occupied.contains(&key) || !terrain.can_fit(gap.x, gap.y, i32::from(gap.z), 16) {
                    return;
                }
                occupied.insert(key);
                let (c, o, ox, oy) = door;
                placements.push((c, o, ox, oy, gap));
            };
            let east = |vx: u16| vx.checked_add(2);
            let mut here = Vec::new();
            for ry in 0..height {
                for rx in 0..width {
                    let (Some(vx), Some(vy)) = (x.checked_add(rx), y.checked_add(ry)) else {
                        continue;
                    };
                    here.clear();
                    terrain.statics_at(vx, vy, &mut here);
                    for &(id, z) in &here {
                        if doorgen::is_west_frame(id) {
                            // A single door: one gap tile to an east frame two away.
                            if east(vx).is_some_and(|e| frame_at(e, vy, z, doorgen::is_east_frame))
                            {
                                try_place(
                                    Point::new(vx + 1, vy, z),
                                    doorgen::GenFacing::WestCw.door(),
                                );
                            } else if vx
                                .checked_add(3)
                                .is_some_and(|e| frame_at(e, vy, z, doorgen::is_east_frame))
                            {
                                // A double door fills the two-tile gap.
                                try_place(
                                    Point::new(vx + 1, vy, z),
                                    doorgen::GenFacing::WestCw.door(),
                                );
                                try_place(
                                    Point::new(vx + 2, vy, z),
                                    doorgen::GenFacing::EastCcw.door(),
                                );
                            }
                        } else if doorgen::is_north_frame(id) {
                            if vy
                                .checked_add(2)
                                .is_some_and(|s| frame_at(vx, s, z, doorgen::is_south_frame))
                            {
                                try_place(
                                    Point::new(vx, vy + 1, z),
                                    doorgen::GenFacing::SouthCw.door(),
                                );
                            } else if vy
                                .checked_add(3)
                                .is_some_and(|s| frame_at(vx, s, z, doorgen::is_south_frame))
                            {
                                try_place(
                                    Point::new(vx, vy + 1, z),
                                    doorgen::GenFacing::NorthCcw.door(),
                                );
                                try_place(
                                    Point::new(vx, vy + 2, z),
                                    doorgen::GenFacing::SouthCw.door(),
                                );
                            }
                        }
                    }
                }
            }
        }

        let count = placements.len();
        for (closed, open, offset_x, offset_y, position) in placements {
            if let Some(entity) = self.place_decoration(facet, closed, 0, position) {
                self.state.registry.insert(
                    entity,
                    Door {
                        closed,
                        open,
                        offset_x,
                        offset_y,
                        is_open: false,
                        close_at: 0,
                    },
                );
            }
        }
        debug!(facet, count, "generated doors from static frames");
    }

    /// Remove every script-placed decoration — "Clear deco".
    fn clear_decorations(&mut self) {
        let placed: Vec<EntityId> = self
            .state
            .registry
            .query::<Decoration>()
            .map(|(entity, _)| entity)
            .collect();
        for entity in placed {
            let serial = self.state.registry.serial_of(entity);
            let facet = self.state.facet_of(entity);
            if let Some(serial) = serial {
                for watcher in self.state.watchers_of(entity) {
                    self.state.forget(watcher, entity, serial);
                }
            }
            self.state.facet_state_mut(facet).sectors.remove(entity);
            self.state.registry.despawn(entity);
        }
    }

    /// Give every brain due a beat. The deciding is [`ai::think_one`]'s; the world
    /// only applies the one thing a brain cannot do itself — a step — since it
    /// owns movement. A creature that gets a `Combat` from the brain is fought by
    /// `combat::swings` exactly as a player would be.
    fn think(&mut self) {
        let now = self.state.ticks;
        let thinkers: Vec<EntityId> = self
            .state
            .registry
            .query::<Brain>()
            .filter(|(_, brain)| now >= brain.next_think)
            .map(|(entity, _)| entity)
            .collect();
        for creature in thinkers {
            // A script-controlled mobile is driven by its `onTick`, not here; the
            // built-in brain stays out of its way.
            if self.state.registry.has::<Scripted>(creature) {
                continue;
            }
            if let Some(dir) = ai::think_one(&mut self.state, creature) {
                if let Some(serial) = self.state.registry.serial_of(creature) {
                    self.step(serial.raw(), dir);
                }
            }
            if let Some(brain) = self.state.registry.get_mut::<Brain>(creature) {
                brain.next_think = now + AI_THINK_TICKS;
            }
        }
    }

    /// The wire serials of every mobile a script has taken control of. The server
    /// reads this each tick and calls each one's `onTick`.
    #[must_use]
    pub fn scripted(&self) -> Vec<Serial> {
        self.state
            .registry
            .query::<Scripted>()
            .filter_map(|(entity, _)| self.state.registry.serial_of(entity))
            .collect()
    }

    /// Hand a mobile's brain to the script: it stops thinking on its own and its
    /// `onTick` drives it instead. See [`Command::Control`].
    fn control(&mut self, serial: u32) {
        if let Some(entity) = Serial::new(serial).and_then(|s| self.state.registry.entity_of(s)) {
            self.state.registry.insert(entity, Scripted);
        }
    }

    /// A client asked to cast a spell: say so on the bus for a script to act on.
    /// The world does not cast — it does not know what the spell costs or does.
    /// See [`Command::RequestCast`].
    fn request_cast(&mut self, connection: ConnectionId, spell: u16) {
        let Some(&entity) = self.state.players.get(&connection) else {
            return;
        };
        let Some(serial) = self.state.registry.serial_of(entity) else {
            return;
        };
        self.state.bus.send(SpellRequested {
            entity,
            serial,
            spell,
        });
    }

    fn disconnect(&mut self, connection: ConnectionId) {
        // A client that logs out mid-drag would otherwise leave its item nowhere —
        // off the ground and out of any container, on a cursor that is gone. Put
        // it back where it was.
        if let Some(held) = self.state.held.remove(&connection) {
            items::restore(&mut self.state, held);
        }
        // Forget any containers it had open; a gone connection watches nothing.
        self.state.open_containers.retain(|_, watchers| {
            watchers.remove(&connection);
            !watchers.is_empty()
        });

        let Some(entity) = self.state.players.remove(&connection) else {
            return;
        };
        // Forget any targeting cursor it had up: a gone mobile clicks nothing.
        self.state.pending_targets.remove(&entity);
        let serial = self.state.registry.serial_of(entity);
        let facet = self.state.facet_of(entity);

        // Save before despawning, and not by marking it dirty: a `touch` is a
        // promise to read the entity at the next save, and in a moment there
        // will be no entity to read. Logging out is when a save matters most —
        // it is the only moment a player's whole session is at stake — so the
        // record is taken at the one instant it still can be.
        if let Some(record) = Self::record_of(&self.state.registry, entity) {
            // The journal copy is for the store; the departed copy is for the
            // server's in-memory character list, which a re-login reads before the
            // deferred store save has necessarily landed.
            self.departed.push(record.clone());
            // The carried inventory, walked now for the same reason as the record:
            // in a moment the items are despawned with the character and there is
            // nothing left to read. Two copies, for two readers: the journal's for
            // the store, and `pending_inventories` so a re-login *this run* re-equips
            // it — the same fast-relogin path the departed record cache serves, and
            // without it a relog before the next save loses everything carried.
            let items = self.inventory_of(entity);
            self.pending_inventories
                .insert(record.serial, items.clone());
            self.journal.keep_inventory(Inventory {
                owner: record.serial,
                items,
            });
            self.journal.keep(record);
        }

        // Take it off every screen *before* despawning: once the entity is gone
        // its serial is released and there is nothing left to tell anyone about.
        if let Some(serial) = serial {
            for watcher in self.state.watchers_of(entity) {
                self.state.forget(watcher, entity, serial);
            }
        }
        self.state.seen.remove(&entity);
        self.state.facet_state_mut(facet).sectors.remove(entity);
        // The character's worn items — its backpack and whatever is in it — are
        // not saved yet, so they go with it rather than orphaning on a serial that
        // is about to be released and reused.
        if let Some(serial) = serial {
            items::despawn_belongings(&mut self.state, serial);
        }
        self.state.registry.despawn(entity);

        if let Some(serial) = serial {
            self.state.bus.send(PlayerLeft { entity, serial });
            info!(%serial, "left the world");
        }
    }
}

#[cfg(test)]
mod interest_tests;
#[cfg(test)]
mod persistence_tests;
#[cfg(test)]
pub(crate) mod tests;
