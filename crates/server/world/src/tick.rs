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

use std::collections::{
    BTreeMap,
    HashMap,
    HashSet,
};
use std::time::{
    Duration,
    Instant,
};

use openshard_ai as ai;
use openshard_chat as chat;
use openshard_combat as combat;
use openshard_crafting as crafting;
use openshard_entities::{
    EntityId,
    Registry,
};
use openshard_events::{
    Cursor,
    EventBus,
};
use openshard_gateway::ConnectionId;
use openshard_items as items;
use openshard_magic as magic;
use openshard_map::grid::Tile;
use openshard_map::overlay::Doors;
use openshard_map::snapshot::MapSnapshot;
use openshard_movement::{
    Walk,
    Walker,
};
use openshard_npc as npc;
use openshard_persistence::{
    CharacterRecord,
    DecorationRecord,
    DoorState,
    Inventory,
    ItemLocation,
    ItemRecord,
    Journal,
    MobileRecord,
    SCHEMA_VERSION,
    Snapshot,
};
use openshard_protocol::access::{
    AccessLevel,
    AuthorityNotice,
};
use openshard_protocol::containers::UseRequest;
use openshard_protocol::context::{
    ContextMenu,
    ContextMenuEntry,
};
use openshard_protocol::direction::{
    Direction,
    Facing,
};
use openshard_protocol::feature::Feature;
use openshard_protocol::gump::{
    ButtonId,
    CloseGump,
    GumpDisplay,
    GumpId,
    GumpKey,
    GumpPoint,
    GumpResponse,
};
use openshard_protocol::identity::{
    AccountName,
    CharacterName,
};
use openshard_protocol::login::{
    SupportedFeatures,
    encode_supported_features,
};
use openshard_protocol::mobile::{
    MobileStatus,
    Notoriety,
    Stat,
    StatLockBits,
    Vitals,
};
use openshard_protocol::serial::{
    RawSerial,
    Serial,
    SerialKind,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::speech::{
    Font,
    RawFont,
    RawTalkMode,
    SpokenMessage,
    TalkMode,
};
use openshard_protocol::version::ClientVersion;
use openshard_protocol::wire::{
    Graphic,
    Hue,
    Layer,
    RawHue,
    RawLayer,
};
use openshard_protocol::world::{
    DeathStatus,
    Facet,
    Light,
    LightLevel,
    LoginComplete,
    LogoutAck,
    MapChange,
    MapSize,
    PlayerStart,
    PlayerUpdate,
    Point,
    SeasonChange,
    Sight,
    TurnRequest,
    WalkAck,
    WalkReject,
    WalkRequest,
};
use openshard_quests as quests;
use openshard_skills as skills;
use openshard_state::components::{
    Access,
    Account,
    Amount,
    Body,
    Brain,
    Client,
    Combat,
    Contained,
    Container,
    DamageType,
    Decoration,
    Door,
    Drawn,
    Equipped,
    Ghost,
    Heading,
    Healer,
    Hitpoints,
    LastStep,
    Mana,
    MeleeDamage,
    Movement,
    Name,
    Position,
    Resistance,
    Ridden,
    Riding,
    SpawnedBy,
    Spellbook,
    Stackable,
    Stamina,
    Stats,
    Vendor,
};
use openshard_state::facet_rules::FacetRules;
use openshard_state::rng::Rng;
use openshard_state::sectors::Sectors;
use openshard_state::{
    FacetState,
    Gameplay,
    ItemLocation as LiveItemLocation,
    Outbound,
    TICKS_PER_SECOND,
    TooltipMode,
    WorldHome,
    WorldState,
    establish_item_location,
    kind_from_drawn,
    presentation_of,
    relocate_item,
};
use tracing::{
    debug,
    info,
    warn,
};

use crate::events::{
    AdminMenuAction,
    CorpseCreated,
    MobileMoved,
    MobileTurned,
    PlayerEntered,
    PlayerLeaving,
    PlayerLeft,
    PlayerRefused,
    RefusedEntry,
    RefusedReason,
    RegionChanged,
    StepRefused,
};
use crate::{
    doorgen,
    gm,
};

mod ambient;
mod chunks;
mod command;
mod context;
mod death;
mod decor;
mod defaults;
mod enter;
mod fields;
mod gates;
/// The world's *own* guild code — what crosses the persistence door. The rules
/// are `openshard_guilds`, spelled out in full at its call sites so the two are
/// never mistaken for each other.
mod guilds;
mod healer;
mod houses;
mod motion;
mod party;
mod persist;
mod regions;
mod roster;
pub mod screen;
mod shipped_items;
mod skills_wire;
mod spawners;
mod speech;
mod spells;
mod staff;
mod status;
mod traps;
mod travel;
mod wake;

use command::StoredCharacter;
pub use command::{
    Appearance,
    Character,
    CharacterSheet,
    Command,
    DecorContainer,
    DecorDoor,
    Entering,
    FreshCharacter,
};
use defaults::*;
pub use defaults::{
    SAVE_EVERY_TICKS,
    TICK_INTERVAL,
};

/// Deterministic command work reserved for one simulation tick.
pub const MAX_COMMAND_WORK_PER_TICK: usize = 256;
/// Compact catalogue contexts admitted per tick after per-connection coalescing.
pub const MAX_CATALOGUE_OPENS_PER_TICK: usize = 32;
pub use persist::{
    RestoredCharacters,
    RestoredItems,
};
use roster::Roster;

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
    state:               WorldState,
    /// What has changed since the last save.
    journal:             Journal,
    /// How often to offer a snapshot, in ticks. Zero never saves.
    save_every:          u64,
    /// Snapshots the tick has taken and nobody has collected yet.
    saves:               Vec<Snapshot>,
    /// Where every stored character was when it was last seen: seeded from the
    /// store at boot, and rewritten by every logout. It is what a re-login reads
    /// to come back where it left rather than where it stood at boot, and the
    /// store cannot answer that — its copy is written by a task nobody waits for,
    /// which a fast re-login can beat. See [`Roster`].
    roster:              Roster,
    /// What the character screen offers beside the characters: the starting
    /// cities, and the two client-capability masks. Configuration, handed over at
    /// boot — see [`CharacterScreen`](screen::CharacterScreen).
    screen:              screen::CharacterScreen,
    /// Read to find out what to mark dirty. See `mark_dirty`.
    entered:             Cursor<PlayerEntered>,
    /// Read to find out what to mark dirty. See `mark_dirty`.
    moved:               Cursor<MobileMoved>,
    /// The same moves, read to notice who stepped onto a gate. A cursor of its
    /// own, not a second read of `moved`: each consumer needs every event, and
    /// two of them sharing one cursor means whichever runs first eats the other's.
    gated:               Cursor<MobileMoved>,
    /// What combat reported hit, for the AI's retaliation.
    damaged:             Cursor<openshard_combat::MobileDamaged>,
    /// Poisoners who fumbled a dose onto themselves, for the tick to apply.
    fumbled:             Cursor<openshard_skills::PoisonedSelf>,
    /// Beggars who were given something, for the tick to put in their pack.
    begged:              Cursor<openshard_skills::Begged>,
    /// Instruments that played their last tune, for the tick to remove.
    spent_instruments:   Cursor<openshard_skills::InstrumentSpent>,
    /// Read to find out what to mark dirty. See `mark_dirty`.
    turned:              Cursor<MobileTurned>,
    /// Skill gains this tick, to push the single-line `0x3A` update to the owner.
    changed:             Cursor<openshard_skills::SkillChanged>,
    /// Damage this tick, read to disturb a spell mid-cast (the `spell_disturb`
    /// rule); a separate cursor from `damaged`, which the AI reads for its own.
    disturbed:           Cursor<openshard_combat::MobileDamaged>,
    /// Deaths this tick, read by `reap` to lay a corpse where a creature fell.
    dead:                Cursor<openshard_combat::MobileDied>,
    /// Deaths this tick again, read to credit a quest's "slay N". A second cursor
    /// on the same event rather than a shared read: `reap` and the quest tally
    /// want the whole list independently, and a cursor is consumed by reading.
    slain:               Cursor<openshard_combat::MobileDied>,
    /// Region crossings this tick, read to set the guards on a murderer who has
    /// just walked into a town.
    crossed:             Cursor<RegionChanged>,
    /// Commands waiting for the next tick.
    inbox:               Vec<Command>,
    /// The leaves a connection has just opened this tick.
    ///
    /// A diagonal past a double doorway asks both of its shut leaves to open,
    /// before its walk request. Linked leaves open as one doorway, so the
    /// second `0x06` is the same automatic action, not an instruction to shut
    /// the pair again. This is tick-local on purpose: a later deliberate
    /// double-click still closes an open door normally.
    opened_door_leaves:  HashSet<(ConnectionId, Serial)>,
    /// The spawn regions the tick keeps populated. Laid by the `populate:` verb,
    /// maintained here, and persisted — a populated area stays populated across a
    /// restart, and a rare spawn keeps its remaining respawn wait.
    ///
    /// **A region's id is its index here**, and nothing else may assign one. That
    /// is the invariant every creature's [`SpawnedBy`] rides on: the tag holds the
    /// id, the tick counts a region's live members by it, and the save writes it
    /// out. See [`register_spawner`](World::register_spawner).
    ///
    /// [`SpawnedBy`]: openshard_state::components::SpawnedBy
    spawners:            Vec<crate::spawner::Spawner>,
    /// Saved inventories waiting for their owners to log in, keyed by character
    /// serial. Loaded from the store at boot by [`restore_inventory`]; a character
    /// entering takes its own and equips it, once.
    ///
    /// [`restore_inventory`]: World::restore_inventory
    pending_inventories: HashMap<Serial, Vec<ItemRecord>>,
    // The status, light and music a connection was last told about used to be
    // three maps here, keyed by connection and cleared by name in `disconnect`.
    // They are fields on the connection's row now — see
    // `openshard_state::connection::Connection` — because a map cleared by name is
    // a map the next one added beside it can be left out of.
    /// Where the world clock started, in UO minutes — restored at boot so a
    /// restart does not put the world back at midnight. See `tick/ambient.rs`.
    clock_base:          u64,
    /// The sector each player was last seen in, the remembered half of the wake
    /// diff. A change means someone has walked into a block of the map that may
    /// be asleep. See `tick/wake.rs`.
    player_sectors:      HashMap<EntityId, (u8, usize)>,
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
    pub fn new(start: Tile) -> Self {
        // Always at least the default facet, so there is somewhere to stand even
        // with no map loaded — the same no-map mode the shard has always had.
        let mut facets = BTreeMap::new();
        facets.insert(
            Facet(DEFAULT_FACET),
            FacetState::new(
                None,
                None,
                FACET_WITHOUT_A_MAP.0,
                FACET_WITHOUT_A_MAP.1,
                FacetRules::classic(Facet(DEFAULT_FACET)),
                // No map, so no world of ours behind it either: there is nothing
                // to patch and nowhere to write a patch down.
                None,
                // No map, so nothing to bake — the table this would be baked
                // over is the empty one `WorldState` starts with, and
                // `with_tiles` rebakes every facet that has ground when the
                // real one arrives.
                &openshard_tiles::TileData::empty(),
            ),
        );
        Self {
            state:               WorldState::new(
                facets,
                Facet(DEFAULT_FACET),
                // An empty table rather than no table: a shard with no client
                // files is one whose tiledata says nothing about every graphic,
                // and saying it once here is what keeps every reader from having
                // to decide what "no table" means. `with_tiles` replaces both.
                openshard_tiles::TileData::empty(),
                openshard_uofiles::multi::Multis::default(),
                start,
                DEFAULT_SEED,
            ),
            journal:             Journal::new(),
            save_every:          SAVE_EVERY_TICKS,
            saves:               Vec::new(),
            roster:              Roster::new(),
            screen:              screen::CharacterScreen::default(),
            entered:             Cursor::default(),
            moved:               Cursor::default(),
            gated:               Cursor::default(),
            damaged:             Cursor::default(),
            fumbled:             Cursor::default(),
            begged:              Cursor::default(),
            spent_instruments:   Cursor::default(),
            turned:              Cursor::default(),
            changed:             Cursor::default(),
            disturbed:           Cursor::default(),
            dead:                Cursor::default(),
            slain:               Cursor::default(),
            crossed:             Cursor::default(),
            inbox:               Vec::new(),
            opened_door_leaves:  HashSet::new(),
            spawners:            Vec::new(),
            pending_inventories: HashMap::new(),
            clock_base:          0,
            player_sectors:      HashMap::new(),
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

    /// Whether the next tick will offer the ordinary periodic snapshot.
    ///
    /// The shard asks before driving that tick so it can put a notice on the
    /// wire before snapshot construction occupies the simulation thread. This
    /// deliberately says nothing about a staff `.save`: that request arrives
    /// while the tick is applying commands, so it cannot be known beforehand.
    #[must_use]
    pub const fn periodic_save_due_next_tick(&self) -> bool {
        self.save_every != 0 && (self.state.ticks.raw() + 1).is_multiple_of(self.save_every)
    }

    /// Set the tunable gameplay rules. The server builds these from the
    /// `[gameplay]` config; a test or the default takes [`Gameplay::default`],
    /// the pre-AoS numbers the systems were written with.
    #[must_use]
    pub const fn with_gameplay(mut self, gameplay: Gameplay) -> Self {
        self.state.gameplay = gameplay;
        self
    }

    /// Set what the character screen offers: the starting cities, and the two
    /// client-capability masks the `0xA9` and `0xB9` carry.
    ///
    /// The server builds these from `[gameplay]` and `[world] facets`; a test
    /// takes the default, which offers no city — nothing in a test creates a
    /// character through the screen, and a city list that came from nowhere would
    /// be a fixture pretending to be configuration.
    #[must_use]
    pub fn with_character_screen(mut self, screen: screen::CharacterScreen) -> Self {
        self.screen = screen;
        self
    }

    /// Start a fresh world's rolls from `seed` instead of the engine's default.
    ///
    /// What `world.seed` in the config reaches. For a world with a save behind it
    /// this is the wrong door — use [`with_rng_state`] and continue the stream the
    /// save was taken from.
    ///
    /// [`with_rng_state`]: World::with_rng_state
    #[must_use]
    pub const fn with_seed(mut self, seed: u64) -> Self {
        self.state.rng = Rng::new(seed);
        self
    }

    /// Resume the roll stream at the point a save recorded.
    ///
    /// The counterpart of [`rng_state`], and the reason the pair exists rather
    /// than one `with_seed`: the two callers mean different things. A fresh world
    /// is *seeded*; a restored one is *resumed*, and seeding it instead would deal
    /// the previous run's rolls again — see [`Rng::state`].
    ///
    /// [`rng_state`]: World::rng_state
    #[must_use]
    pub const fn with_rng_state(mut self, state: u64) -> Self {
        self.state.rng = Rng::new(state);
        self
    }

    /// Where the roll stream has got to, for the save.
    #[must_use]
    pub const fn rng_state(&self) -> u64 {
        self.state.rng.state()
    }

    /// Give the shard the client's static tables: what every graphic is, and what
    /// every multi is made of.
    ///
    /// One pair for the whole shard rather than one per facet, because that is
    /// what they are — an install has one `tiledata.mul`, and a house does not
    /// change shape between Felucca and Trammel. A shard left without them keeps
    /// the empty pair [`World::new`] gave it and runs with no encumbrance, no
    /// layers, no names and no houses, which is the same bargain a shard with no
    /// map makes about walking.
    ///
    /// Both are taken by value and neither is an `Option`: an install whose
    /// multis could not be read hands over [`Multis::default`], which is a table
    /// that knows about no houses — the same thing said as data instead of as an
    /// absence.
    ///
    /// [`Multis::default`]: openshard_uofiles::multi::Multis::default
    #[must_use]
    pub fn with_tiles(
        mut self,
        tiles: openshard_tiles::TileData,
        multis: openshard_uofiles::multi::Multis,
    ) -> Self {
        // `set_tiles` and not a field write: every facet already loaded is
        // holding a span bake over the *old* table, and rebaking there is what
        // makes the builder's order not matter here.
        self.state.set_tiles(tiles);
        self.state.multis = multis;
        self
    }

    /// Give the shard its operator-imported custom house designs.
    ///
    /// They are keyed by their JSON file stem, rather than synthetic multi ids:
    /// an imported design has no entry in `multi.mul` and must remain distinct
    /// from every classic house the client installation owns.
    #[must_use]
    pub fn with_house_templates(
        mut self,
        templates: std::collections::BTreeMap<String, Vec<openshard_uofiles::multi::Component>>,
    ) -> Self {
        self.state.house_templates = templates;
        self
    }

    /// Give the default facet a map, under the ruleset its number ran in retail.
    ///
    /// A map and no home: a caller handing over a snapshot it built itself — a
    /// test, the playground — has no base set behind it, so the facet is one
    /// nothing can commit a patch to.
    pub fn with_map(self, map: MapSnapshot) -> Self {
        let facet = self.state.default_facet;
        self.with_facet(facet, map, None, FacetRules::classic(facet), None)
    }

    /// Load `map` and its already-baked coarse router as facet `facet`, under
    /// `rules`.
    ///
    /// The facet is named here as well as carried by the snapshot, because this
    /// is the key the world files it under and a caller loading Malas into slot
    /// three should say so once, out loud. The ruleset is named for the same
    /// reason and not folded into that: a caller doing exactly that — Malas into
    /// slot three — is precisely the one whose facet number no longer says what
    /// its rules are, so [`FacetRules::classic`] is offered rather than applied.
    ///
    /// `home` is where that world lives on disk, and it is `Some` exactly for a
    /// facet read out of a base set of ours — the one kind that can be edited
    /// while the shard runs. See [`WorldHome`].
    pub fn with_facet(
        mut self,
        facet: Facet,
        map: MapSnapshot,
        coarse: Option<openshard_movement::NavigationGraph>,
        rules: FacetRules,
        home: Option<WorldHome>,
    ) -> Self {
        debug_assert_eq!(map.facet(), facet, "a snapshot loaded into another facet's slot");
        let (width, height) = (map.map().width(), map.map().height());
        debug_assert!(
            coarse
                .as_ref()
                .is_none_or(|graph| graph.dimensions() == (width, height))
        );
        self.state.facets.insert(
            facet,
            FacetState::new(Some(map), coarse, width, height, rules, home, self.state.tiles()),
        );
        self
    }

    /// Bring a facet's coarse graph in step with ground it never saw move, and
    /// hand it back so the caller can write it down.
    ///
    /// Boot's door to [`WorldState::catch_up`], and the reason the facet is
    /// loaded *before* it is asked: the rebake reads this facet's span index and
    /// its map, and both of those are things [`with_facet`](Self::with_facet)
    /// built. A caller that carried the graph forward outside the world would
    /// have to bake a second span index over the same facet to do it.
    pub fn catch_up(
        &mut self,
        facet: Facet,
        chunks: &[openshard_map::chunk::ChunkCoord],
    ) -> Option<&openshard_movement::NavigationGraph> {
        self.state.catch_up(facet, chunks)
    }

    /// The default facet's spatial index.
    pub fn sectors(&self) -> &Sectors {
        self.state.facets[&self.state.default_facet].sectors()
    }

    /// The event bus, for reading what happened.
    pub const fn bus(&self) -> &EventBus {
        &self.state.bus
    }

    /// Send an admin verb nobody clicked — see [`crate::admin::seed`].
    ///
    /// Called between the script host being loaded and the first tick, which is
    /// the one window where the cursors exist and no tick has retired anything
    /// yet: the bus keeps an event for a tick past the `update` that follows it,
    /// so a verb sent here is read by the first delivery and not a tick late.
    pub fn seed(&mut self, action: &str) {
        crate::admin::seed(&mut self.state, action);
    }

    /// Everything in the world.
    pub const fn registry(&self) -> &Registry {
        &self.state.registry
    }

    /// How many ticks have run.
    pub const fn ticks(&self) -> openshard_state::WorldTick {
        self.state.ticks
    }

    /// How many people are in the world.
    pub fn player_count(&self) -> usize {
        self.state.players.len()
    }

    /// The wire serial of everyone connected. For a benchmark that wants to walk
    /// them; a shard addresses players by connection, not by serial.
    pub fn player_serials(&self) -> Vec<Serial> {
        self.state
            .players
            .values()
            .filter_map(|&entity| self.state.registry.serial_of(entity))
            .collect()
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

    /// How many commands are waiting for the next tick.
    ///
    /// The only way anything outside can see that a packet became work. A test
    /// that a gate *refused* a packet has nothing else to look at: the whole
    /// assertion is that nothing happened, and every other observation of the
    /// world — the outbox, the players, the events — is downstream of a tick that
    /// would have had nothing to apply either way.
    pub fn queued(&self) -> usize {
        self.inbox.len()
    }

    /// Value-free names of the commands the next tick will apply.
    ///
    /// The shard watchdog snapshots this immediately before calling [`tick`].
    /// Returning names rather than commands keeps player text and bulky content
    /// batches out of logs while still saying what occupied a slow tick.
    pub fn queued_command_kinds(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.inbox.iter().map(Command::kind)
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

    /// Put a character on an account's list without recording anything about it.
    ///
    /// For the config-seeded characters at boot: `[[accounts]] characters` names
    /// characters that exist and that nothing has ever saved, which is exactly
    /// what the roster's `None` record means.
    ///
    /// Safe to call before or after [`restore_characters`](Self::restore_characters):
    /// the enrolment is idempotent, an entry already on the list is not touched,
    /// and a record describes an entry however late the enrolment was — including
    /// the character's spelling, which the config names but does not get to
    /// change. The order decides the slot and nothing else.
    pub fn enrol_character(&mut self, account: &AccountName, name: &CharacterName) {
        self.roster.enrol(account, name);
    }

    /// An account's characters, in the slot order the `0xA9` list shows and
    /// `0x83` indexes.
    pub fn characters(&self, account: &AccountName) -> Vec<openshard_protocol::login::CharacterEntry> {
        self.roster.characters(account)
    }

    /// Delete a character.
    ///
    /// Everything the world has under that name goes: its place on the account's
    /// list, the record a re-login would read, the store row, and the inventory
    /// waiting under its serial. A character with nothing recorded — created this
    /// run and never logged out — leaves the list all the same; there is simply
    /// nothing saved anywhere to clean up after it, which is what the early
    /// return below skips.
    ///
    /// The serial is *not* unbound — a packet in flight may still name it — so
    /// `reserve_serial` keeps it out of circulation for the rest of the run.
    fn delete_character(&mut self, account: &AccountName, name: &CharacterName) {
        let Some(record) = self.roster.forget(account, name) else {
            return;
        };
        self.journal.forget_serial(record.serial);
        // Drop the fast-relogin inventory cache: the character is gone, not
        // coming back this run.
        self.pending_inventories.remove(&record.serial);
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
        self.opened_door_leaves.clear();

        // Take the whole inbox. A command queued *during* a tick belongs to the
        // next one — otherwise a system that queues work could starve the loop,
        // and the tick's length would depend on what happened in it.
        let commands = std::mem::take(&mut self.inbox);
        let mut last_catalogue_open = HashMap::new();
        for (index, command) in commands.iter().enumerate() {
            if let Command::OpenCraftCatalogue { connection } = command {
                last_catalogue_open.insert(*connection, index);
            }
        }
        let mut deferred = Vec::new();
        let mut command_work = 0usize;
        let mut catalogue_opens = 0usize;
        let mut catalogue_coalesced = 0usize;
        for (index, command) in commands.into_iter().enumerate() {
            if command_work == MAX_COMMAND_WORK_PER_TICK {
                deferred.push(command);
                continue;
            }
            if let Command::OpenCraftCatalogue { connection } = &command {
                if last_catalogue_open.get(connection) != Some(&index) {
                    catalogue_coalesced += 1;
                    continue;
                }
                if catalogue_opens == MAX_CATALOGUE_OPENS_PER_TICK {
                    deferred.push(command);
                    continue;
                }
                catalogue_opens += 1;
            }
            command_work += 1;
            self.apply(command, now);
        }
        if !deferred.is_empty() || catalogue_coalesced != 0 {
            tracing::debug!(
                metric = "item_transaction.command_budget",
                command_work,
                catalogue_opens,
                catalogue_coalesced,
                deferred = deferred.len(),
                "bounded command lanes deferred or coalesced work"
            );
        }
        deferred.append(&mut self.inbox);
        self.inbox = deferred;

        // What time it is, once, before anything asks. Derived from the tick
        // counter (see `tick/ambient.rs`), never a wall clock, so a routine and a
        // shop's opening hours replay like everything else.
        self.refresh_hour();

        // Before anything beats: has a player walked into a part of the map that
        // was asleep? A dozing mobile is not woken by anyone arriving unless
        // something tells it, and waiting out a sixteen-second doze is what "the
        // NPCs only start acting when I get close" looks like. See `tick/wake.rs`.
        self.sector_wakes();

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
        // The three verbs of a combat action, and every fighter goes through
        // them — a swordsman, an archer and a thing that breathes fire differ at
        // the impact and nowhere in the schedule. Apply the world to what is
        // already running — ending, with a reason on the wire, whatever the
        // world has spoiled — then land what has reached its impact, then start
        // what a ready fighter promises, telling the client the exact
        // server-owned interval to that impact.
        //
        // Committing *last* is what makes a fight continuous: a blow that lands
        // this tick opens its next gesture in the same tick, so the animation
        // covers the whole interval instead of starting a tick late and leaving
        // a beat of dead air in every single swing.
        combat::sustain_actions(&mut self.state);
        combat::resolve_actions(&mut self.state);
        combat::commit_actions(&mut self.state);
        // A poisoner who fumbled a dose onto themselves. `skills` decides it and
        // says so; applying poison is combat's one door, and this is the tick
        // closing the gap — the decide-then-apply split again.
        let fumbles: Vec<openshard_skills::PoisonedSelf> =
            self.state.bus.read(&mut self.fumbled).copied().collect();
        let ticks = self.state.ticks;
        for fumble in fumbles {
            combat::apply_poison(&mut self.state, fumble.serial, fumble.level, ticks);
        }
        // And a beggar who talked somebody out of some coin. Same shape: `skills`
        // decides, and the crate that owns backpacks pays.
        let begged: Vec<openshard_skills::Begged> = self.state.bus.read(&mut self.begged).copied().collect();
        for beg in begged {
            let Some(serial) = self.state.registry.serial_of(beg.entity) else {
                continue;
            };
            let Some(backpack) = items::backpack_of(&self.state, serial) else {
                continue;
            };
            let outcome = items::give(&mut self.state, backpack, items::GOLD_GRAPHIC, Hue(0), beg.gold);
            if !outcome.is_complete() {
                self.state.system_message(
                    beg.entity,
                    &format!(
                        "Only {} of {} begged gold reached your backpack.",
                        outcome.given, beg.gold
                    ),
                );
            }
            // ServUO's `Begging` plays the amount-sensitive gold drop sound at
            // the beggar after the coins have reached their pack.
            self.state.play_sound(
                beg.entity,
                items::drop_sound(
                    items::GOLD_GRAPHIC,
                    u16::try_from(outcome.given).unwrap_or(u16::MAX),
                    openshard_protocol::wire::SoundId(0x0048),
                ),
            );
        }
        combat::expire_criminality(&mut self.state);
        combat::decay_murders(&mut self.state);
        combat::poison_tick(&mut self.state);
        // Pulse and expire persistent fields — fire burns, poison seeps, walls hold
        // — before `reap`, so a field kill lays its corpse this tick.
        self.field_tick();
        // Close the gates whose half-minute is up, and take through anyone who
        // stepped onto one this tick. Before `reap` for the same reason a field
        // is: what happens on arrival happens now, not next tick.
        self.expire_gates();
        self.gate_crossings();
        // Lift the stat buffs whose time is up, and redraw the bar for any player
        // whose stats just changed back — the decide-then-apply split again.
        let now = self.state.ticks;
        for entity in magic::expire_buffs(&mut self.state, now) {
            if let Some(serial) = self.state.registry.serial_of(entity) {
                self.refresh_status_of(serial);
            }
        }
        // Lift the behaviour buffs whose time is up. Night Sight restores the
        // ambient light on its way out; the rest just stop being read.
        for (entity, kind) in magic::expire_behaviour_buffs(&mut self.state, now) {
            if kind == openshard_state::BehaviourBuffKind::NIGHT_SIGHT {
                if let Some(serial) = self.state.registry.serial_of(entity) {
                    self.send_light(serial, LIGHT_DAY);
                }
            }
        }
        // Thaw the paralyzed whose time is up, and tell a player it can move again.
        for entity in magic::expire_frozen(&mut self.state, now) {
            self.notify_self(entity, "You are no longer frozen.");
        }
        skills::expire_ghost_contact(&mut self.state);
        skills::expire_songs(&mut self.state);
        self.finish_bandages();
        // Swing every pick, axe and line whose beat has come, and remove the tools
        // that broke doing it — the `InstrumentSpent` split once more, since a
        // worn-out pickaxe is `items`' to make gone.
        for worn in skills::advance_harvests(&mut self.state) {
            if let Some(serial) = self.state.registry.serial_of(worn.tool) {
                items::consume(&mut self.state, serial, 0);
            }
        }
        // And every hammer, saw and pestle in flight. After the harvest for no
        // reason but reading order: the two are independent, and a craft's
        // materials are already in a pack before its first beat.
        crafting::advance_crafts(&mut self.state);
        // An instrument that played its last tune. `skills` decides, `items`
        // removes — the same split the poison fumble and the beggar's coin use.
        let spent: Vec<openshard_skills::InstrumentSpent> = self
            .state
            .bus
            .read(&mut self.spent_instruments)
            .copied()
            .collect();
        for gone in spent {
            if let Some(serial) = self.state.registry.serial_of(gone.item) {
                items::consume(&mut self.state, serial, 0);
            }
        }
        magic::regen_mana(&mut self.state);
        combat::regen_stamina(&mut self.state);
        combat::regen_hits(&mut self.state);
        // Finish or break the ServUO-style casts whose delay is up or whose
        // caster was struck; the Sphere style resolves in `begin_cast` and never
        // reaches here.
        self.advance_casts();
        // Credit this tick's kills against any "slay N" objective. Before `reap`,
        // which is only ordering hygiene — the event outlives both — but it keeps
        // the quest tally reading a world where the body is still standing.
        let slain: Vec<openshard_combat::MobileDied> =
            self.state.bus.read(&mut self.slain).cloned().collect();
        quests::advance_slay(&mut self.state, &slain);
        // Lay a corpse where each creature fell this tick — after every source of
        // death (a swing, a volley, poison, a spell, a command) has had its turn.
        self.reap();
        items::decay(&mut self.state);
        self.collapse_houses();
        self.state
            .advance_house_inventory_rebuilds(openshard_state::HOUSE_INVENTORY_REBUILD_BUDGET);
        self.sail_boats();
        items::close_doors(&mut self.state);
        // End any trade whose two parties have walked apart, died or logged out,
        // and untick both boxes if the goods moved after somebody agreed to them.
        // Found rather than announced: ServUO does this from the `Location`
        // setter, which is a call beside every one of this engine's five movers.
        items::validate_trades(&mut self.state);
        self.maintain_spawners();
        // Notice who walked into a town or out of a dungeon: the crossing emits
        // its event and starts the region's music. Before the guards read it, and
        // before the light pass, which the crossing can change.
        self.region_crossings();
        // A murderer who walks into a guarded town is hunted without anyone
        // having to call — ServUO's `GuardedRegion.OnEnter`.
        self.guard_crossings();
        npc::expire_guards(&mut self.state);

        // Follow this tick's skill gains on any open window. Before `update`
        // retires the events, like `mark_dirty`.
        self.send_skill_updates();
        // The sun moved, or somebody walked into a cave. One pass, both reasons,
        // and only the players whose level actually changed are told.
        self.refresh_light();
        // The clock reached a new six-hour weather quarter. This is separate
        // from the light diff because weather is a whole visual state rather
        // than a brightness level, and it changes even at noon.
        self.refresh_weather();
        // And follow what a player is carrying: gold spent, loot lifted, armour
        // worn. Diffed against what was last sent, so a still player costs nothing.
        self.refresh_statuses();
        // The quest passes that *look* rather than being told: what a player is
        // carrying against their obtain objectives, where an escorted NPC has got
        // to, and the clocks on timed quests. All three are diffing passes for the
        // same reason `refresh_statuses` is — a call beside every mutation is a
        // call someone eventually forgets.
        if self.state.ticks.is_multiple_of(quests::OBTAIN_EVERY_TICKS) {
            quests::refresh_obtain(&mut self.state);
        }
        for (serial, direction) in quests::advance_escorts(&mut self.state) {
            self.step(serial, direction);
        }
        quests::tick_timers(&mut self.state);
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

    /// Whether a townsperson of this trade is already posted to this tile.
    ///
    /// The de-duplication a placed NPC has and a spawner's creature does not.
    ///
    /// # Why the trade, and why the *post*
    ///
    /// The body is no key: four hundred townsfolk share body 400. And the tile it
    /// is **standing** on is no key either — a townsperson drifts around its
    /// counter between beats, and at night walks home — so a check against
    /// `Position` misses whoever had wandered a step, which on a restored shard
    /// was half of them. [`Npc::home`](openshard_state::components::Npc) is the
    /// tile it was *placed* on and does not move, which is the thing the content
    /// actually names.
    ///
    /// **`x` and `y` only.** A spawn is dropped onto the ground, so an NPC placed
    /// at `z: 0` is posted at whatever height the terrain there turned out to be —
    /// on Felucca that is `-2` more often than not. Comparing the z would compare
    /// what the content asked for against what the world decided, which never
    /// matches, and the whole check would silently pass everybody through.
    fn townsperson_already_stands(&self, facet: Facet, at: Point, title: &str) -> bool {
        self.state
            .registry
            .query::<openshard_state::components::Npc>()
            .any(|(entity, npc)| {
                (npc.home.x, npc.home.y) == (at.x, at.y)
                    && self.state.facet_of(entity) == facet
                    && self
                        .state
                        .registry
                        .get::<openshard_state::components::Title>(entity)
                        .is_some_and(|worn| worn.0 == title)
            })
    }

    fn spawn_mobile(&mut self, command: Command) {
        let Command::SpawnMobile {
            body,
            hue,
            hits,
            notoriety,
            damage,
            resistance,
            swing,
            sight,
            aggression,
            beat,
            wander,
            ranged,
            ranged_kind,
            position,
            facet,
            name,
            title,
            shoe,
            fame,
            karma,
            night_home,
            banker,
            vendor,
            healer,
            equipment,
            skills,
            stock,
            escort_to,
            quests: offers,
        } = command
        else {
            unreachable!("spawn_mobile is called only for Command::SpawnMobile");
        };

        // A placed townsperson is skipped if one of its trade already stands
        // on the tile. Placement is *not* saved as a thing that can be re-run —
        // the mobiles themselves are — so without this, seeding `populate:` on
        // a restored shard put a second banker inside the first, and pressing
        // the button twice did the same. Only titled mobiles: a spawner's
        // creature has no title, lands on a tile the rng picked, and two of them
        // sharing one is ordinary.
        if title
            .as_deref()
            .is_some_and(|title| self.townsperson_already_stands(facet, position, title))
        {
            return;
        }
        let spawned = npc::spawn(
            &mut self.state,
            npc::SpawnSpec {
                body,
                hue,
                hits,
                notoriety,
                damage,
                resistance,
                swing,
                sight,
                aggression,
                beat,
                wander,
                ranged,
                ranged_kind,
                position,
                facet,
                name,
                title,
                shoe: npc::ShoeType::from_bits(shoe),
                fame,
                karma,
                night_home,
                banker,
                vendor,
                healer,
                equipment,
                skills,
            },
        );
        // Both were a second command keyed by serial, and the serial did not
        // exist until this returned — which is what the tile-keyed rendezvous
        // the script pack was working around. See `Command::SpawnMobile`.
        if let Some(entity) = spawned {
            if let Some(serial) = self.state.registry.serial_of(entity) {
                if !stock.is_empty() {
                    npc::stock(&mut self.state, serial, stock);
                }
                let mut offers = offers;
                if let Some(destination) = escort_to {
                    quests::make_escortable(&mut self.state, serial, destination);
                    // An escort *is* a quest: the offer, the log entry and the
                    // reward all come from one. Without this it would follow
                    // whoever double-clicked it, with nothing to accept or
                    // refuse.
                    offers.push(openshard_state::QuestKey::new("escort"));
                }
                if !offers.is_empty() {
                    quests::bind_giver(&mut self.state, serial, offers);
                }
            }
        }
    }

    fn clicked_entities(&self, connection: ConnectionId, serial: Serial) -> Option<(EntityId, EntityId)> {
        Some((
            *self.state.players.get(&connection)?,
            self.state.registry.entity_of(serial)?,
        ))
    }

    fn handle_double_click(&mut self, connection: ConnectionId, request: UseRequest) {
        match request {
            // Bit 31 is the client's *paperdoll request* — the login-time
            // paperdoll open, the paperdoll macro — and it is only that:
            // ServUO's `UseReq` routes it straight to `OnPaperdollRequest`,
            // never to `Use`. Treating both alike was the bug where relogging
            // mounted dismounted you a breath later: the client's paperdoll-open
            // read as a self-double-click. `DoubleClick::interpret` is what
            // takes the two apart, and it did so before this command was queued.
            UseRequest::Paperdoll(raw) => {
                debug!(serial = format!("0x{:08X}", raw.0), "paperdoll request");
                if let Some(serial) = raw.validate() {
                    items::paperdoll_request(&mut self.state, connection, serial);
                }
            }
            UseRequest::Use(raw) => self.handle_item_use(connection, raw),
        }
    }

    fn handle_item_use(&mut self, connection: ConnectionId, raw: RawSerial) {
        debug!(serial = format!("0x{:08X}", raw.0), "double-click");
        // A click on nothing is silence: `0`, `0xFFFFFFFF` and anything
        // past the item pool address no object, and the client is owed
        // no answer for asking.
        let Some(serial) = raw.validate() else {
            return;
        };

        // ServUO asks `CheckAlive` before it dispatches a use, so
        // every new double-click begins closed to the dead. This
        // shard has one deliberate exception: its healer click is
        // an extra resurrection path (ServUO offers on movement).
        if let Some((player, target)) = self.clicked_entities(connection, serial) {
            if self.state.registry.has::<Ghost>(player) {
                if self.state.registry.has::<Healer>(target) {
                    self.click_healer(player, target);
                } else {
                    self.state.system_message(player, items::DEAD_HANDS);
                }
                return;
            }
        }

        self.notify_mobile_use(connection, serial);
        let snoop_refused = self.snooping_refused(connection, serial);
        let engine_window = self.open_engine_window(connection, serial);
        if !engine_window && !snoop_refused && !npc::open_shop(&mut self.state, connection, serial) {
            self.use_ordinary_item(connection, serial);
        }
    }

    fn notify_mobile_use(&mut self, connection: ConnectionId, serial: Serial) {
        // Every double-clicked mobile reaches the rules layered over it,
        // whatever the engine itself then does with the click.
        items::mobile_used(&mut self.state, connection, serial);
        if let Some((player, target)) = self.clicked_entities(connection, serial) {
            quests::talk_to(&mut self.state, player, target);
        }
        if let Some((player, target)) = self.clicked_entities(connection, serial) {
            self.click_healer(player, target);
        }
        // A trapped chest goes off before it opens — and then opens anyway.
        if let Some((player, target)) = self.clicked_entities(connection, serial) {
            self.spring_trap(player, target);
        }
    }

    fn snooping_refused(&mut self, connection: ConnectionId, serial: Serial) -> bool {
        let Some((player, target)) = self.clicked_entities(connection, serial) else {
            return false;
        };
        if !self.state.registry.has::<Container>(target)
            || !matches!(
                openshard_state::item_location(&self.state, target),
                Some(LiveItemLocation::Settled(
                    openshard_state::SettledItemLocation::Contained(_)
                ))
            )
        {
            return false;
        }
        // A failed peek keeps the gump shut, and every peek costs karma.
        !skills::snooping(&mut self.state, player, target)
    }

    fn open_engine_window(&mut self, connection: ConnectionId, serial: Serial) -> bool {
        let Some((player, target)) = self.clicked_entities(connection, serial) else {
            return false;
        };
        // Gates and runebooks own engine windows; neither may fall through as
        // a bare `ItemUsed` event.
        self.click_gate(player, target) || self.click_runebook(player, target)
    }

    fn use_ordinary_item(&mut self, connection: ConnectionId, serial: Serial) {
        // `App::open_door_ahead` sends a use for every shut leaf a diagonal has
        // to pass. Do not toggle a linked doorway shut on the second request in
        // that automatic batch.
        let already_opened = self.opened_door_leaves.contains(&(connection, serial));
        let opened = match self.state.registry.entity_of(serial) {
            Some(door) => {
                self.state
                    .registry
                    .get::<Door>(door)
                    .filter(|door| !door.is_open)
                    .map(|door| (serial, door.link))
            }
            None => None,
        };
        let equipped_weapon = if !already_opened {
            let equipped_weapon = items::double_click(&mut self.state, connection, serial);
            if let Some((leaf, Some(link))) = opened.filter(|_| {
                self.state
                    .registry
                    .entity_of(serial)
                    .and_then(|door| self.state.registry.get::<Door>(door))
                    .is_some_and(|door| door.is_open)
            }) {
                self.opened_door_leaves.insert((connection, leaf));
                self.opened_door_leaves.insert((connection, link));
            }
            equipped_weapon
        } else {
            false
        };

        // Core item skills and shipped item behaviours run only after the pack
        // has seen the ordinary `ItemUsed` event.
        if !equipped_weapon {
            if let Some((player, item)) = self.clicked_entities(connection, serial) {
                self.use_item_skill(player, item);
                self.use_shipped_item(player, item);
            }
        }
    }

    fn apply(&mut self, command: Command, now: Instant) {
        match command {
            Command::Authenticated {
                connection,
                version,
                account,
                access,
            } => self.authenticated(connection, version, account, access),
            Command::CreateCharacter { connection, create } => self.create_character(connection, create),
            Command::PlayCharacter { connection, name } => self.play_character(connection, name),
            Command::Enter(entering) => self.enter(entering),
            Command::Walk { connection, request } => self.walk(connection, request, now),
            Command::Turn { connection, request } => self.turn(connection, request),
            Command::RequestStatus { connection } => {
                if let Some(&entity) = self.state.players.get(&connection) {
                    self.send_status(connection, entity);
                }
            }
            Command::Resync { connection } => {
                let Some(&entity) = self.state.players.get(&connection) else {
                    debug!(%connection, "0x22 from a connection with no character");
                    return;
                };
                // Worth a line in the log: a client only asks when the walk
                // handshake has broken down, and the two ends disagreeing about a
                // sequence is a thing to know about rather than a routine event.
                debug!(%connection, "resync asked for: the walk fell out of step");
                self.state.resync(entity);
            }
            Command::LogoutRequest { connection } => {
                // Say yes and stop. The client closes the connection itself, and
                // the disconnect path saves and despawns as it does for any other
                // way of leaving — there is no second logout rule here.
                self.state
                    .send_packet(connection, &ServerPacket::LogoutAck(LogoutAck));
                // But say it out loud as well, because the character stays in the
                // world until the socket closes and something has to know that
                // this connection is on its way out. The shard loop stops
                // accepting in-world packets from it; the world itself changes
                // nothing, which is why this is an event and not a rule.
                self.state.bus.send(PlayerLeaving { connection });
            }
            Command::RequestSkills { connection } => {
                if let Some(&entity) = self.state.players.get(&connection) {
                    self.send_skills(connection, entity);
                }
            }
            Command::GumpResponse { connection, response } => self.handle_gump_response(connection, response),
            Command::TargetResponse { connection, response } => self.handle_target(connection, response),
            Command::RegisterSpawner { spawner } => self.register_spawner(spawner),
            Command::ClearSpawners => self.clear_spawners(),
            Command::RegisterRegions { facet, regions } => self.register_regions(facet, regions),
            Command::ClearRegions { facet } => self.clear_regions(facet),
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
            Command::Step { serial, direction } => self.step(serial, Direction::from_bits(direction)),
            Command::SpawnItem {
                graphic,
                hue,
                amount,
                stackable,
                position,
                facet,
            } => {
                let drawn = Drawn { id: graphic, hue };
                if let Some((kind, material)) = kind_from_drawn(drawn) {
                    items::spawn_item_kind(
                        &mut self.state,
                        kind,
                        material,
                        amount,
                        stackable,
                        position,
                        facet,
                    );
                } else {
                    items::spawn_item(&mut self.state, graphic, hue, amount, stackable, position, facet);
                }
            }
            Command::SpawnContainer {
                graphic,
                gump,
                hue,
                position,
                facet,
            } => items::spawn_container(&mut self.state, graphic, gump, hue, position, facet),
            command @ Command::SpawnMobile { .. } => self.spawn_mobile(command),
            Command::Damage {
                serial,
                amount,
                damage_type,
                by,
            } => {
                combat::damage(
                    &mut self.state,
                    serial,
                    amount,
                    DamageType::from_u8(damage_type),
                    by,
                )
            }
            Command::CastSpell {
                serial,
                spell,
                target,
                mana,
                min_skill,
                max_skill,
                skill,
                pack,
                reagents,
            } => {
                magic::cast_spell(
                    &mut self.state,
                    magic::Cast {
                        serial,
                        spell,
                        target,
                        mana,
                        skill_band: openshard_skills::SkillBand::new(min_skill, max_skill),
                        skill: magic::SkillId::new(skill),
                        pack,
                        reagents: &reagents,
                    },
                )
            }
            Command::Heal { serial, amount } => magic::heal(&mut self.state, serial, amount),
            Command::SetStats {
                serial,
                strength,
                dexterity,
                intelligence,
            } => {
                skills::set_stats(
                    &mut self.state,
                    serial,
                    Stats {
                        strength,
                        dexterity,
                        intelligence,
                    },
                )
            }
            Command::SetSkill { serial, skill, value } => {
                skills::set_skill(&mut self.state, serial, skill, value)
            }
            Command::SetWeapon {
                serial,
                speed,
                min,
                max,
            } => items::set_weapon(&mut self.state, serial, speed, min, max),
            Command::SetPoison {
                serial,
                level,
                charges,
            } => items::set_poison(&mut self.state, serial, level, charges),
            Command::UseSkill {
                serial,
                skill,
                min_skill,
                max_skill,
            } => skills::use_skill(&mut self.state, serial, skill, min_skill, max_skill),
            Command::SetSkillLock {
                connection,
                skill,
                lock,
            } => self.set_skill_lock(connection, skill, lock),
            Command::UseSkillButton { connection, skill } => {
                if let Some(&player) = self.state.players.get(&connection) {
                    skills::use_skill_button(&mut self.state, player, skill);
                }
            }
            Command::OpenCraftCatalogue { connection } => {
                if let Some(&player) = self.state.players.get(&connection) {
                    crafting::open_catalogue(&mut self.state, player);
                }
            }
            Command::HouseInventory { connection, request } => self.house_inventory(connection, request),
            Command::SetStatLock {
                connection,
                stat,
                lock,
            } => self.set_stat_lock(connection, stat, lock),
            Command::WarMode { connection, war } => combat::war_mode(&mut self.state, connection, war),
            Command::Attack { connection, target } => combat::attack(&mut self.state, connection, target),
            Command::Say {
                connection,
                mode,
                hue,
                font,
                text,
            } => self.say(connection, mode, hue, font, text),
            Command::Speak { serial, hue, text } => {
                if let Some(entity) = self.state.registry.entity_of(serial) {
                    chat::speak(
                        &mut self.state,
                        entity,
                        TalkMode::Regular,
                        hue,
                        Font::DEFAULT,
                        &text,
                    );
                }
            }
            Command::DoubleClick { connection, request } => self.handle_double_click(connection, request),
            Command::SingleClick { connection, serial } => self.single_click(connection, serial),
            Command::QueryProperties { connection, serials } => self.query_properties(connection, &serials),
            Command::ContextMenuRequest { connection, serial } => {
                self.context_menu_request(connection, serial);
            }
            Command::DesignDetails { connection, serial } => {
                self.design_details_request(connection, serial);
            }
            Command::RequestChunks {
                connection,
                facet,
                chunks,
            } => self.chunk_request(connection, facet, &chunks),
            Command::RequestChanges {
                connection,
                facet,
                revision,
            } => self.changes_request(connection, facet, revision),
            Command::CommitMapEdit { connection, request } => {
                crate::mapedit::request(&mut self.state, connection, &request);
            }
            Command::ContextMenuSelect {
                connection,
                serial,
                index,
            } => self.context_menu_select(connection, serial, index),
            Command::Party { connection, request } => self.party_request(connection, &request),
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
                destination,
            } => items::drop_item(&mut self.state, connection, serial, destination),
            Command::TradeAction {
                connection,
                container,
                accepted,
            } => items::set_accepted(&mut self.state, connection, container, accepted),
            Command::TradeCancel {
                connection,
                container,
            } => items::cancel_by_container(&mut self.state, connection, container),
            Command::Disconnect { connection } => self.disconnect(connection),
            Command::DeleteCharacter { connection, slot } => self.delete_character_at(connection, slot),
            Command::ShowGump {
                serial,
                gump_id,
                at,
                layout,
                lines,
            } => self.show_gump(serial, gump_id, at, &layout, &lines),
            Command::RegisterNpcSpeech { trades } => {
                let count = trades.len();
                self.state.dialogue.set_tables(trades.into_iter().collect());
                debug!(count, "townsfolk speech registered");
            }
            Command::RegisterQuests { quests } => {
                let count = quests.len();
                self.state.quests.set(quests);
                debug!(count, "quest definitions registered");
            }
            Command::BindQuestGiver { serial, keys } => {
                quests::bind_giver(&mut self.state, serial, keys);
            }
            Command::MakeEscortable { serial, destination } => {
                quests::make_escortable(&mut self.state, serial, destination);
            }
            Command::GuildWindowRequest { connection } => {
                openshard_guilds::open(&mut self.state, connection);
            }
            Command::QuestLogRequest { connection } => {
                quests::open_log(&mut self.state, connection);
            }
            Command::CloseGump { serial, gump_id } => self.close_gump(serial, gump_id),
            Command::Message { serial, text } => {
                if let Some(entity) = self.state.registry.entity_of(serial) {
                    self.state.system_message(entity, &text);
                }
            }
            Command::PlaySound { serial, sound } => {
                if let Some(entity) = self.state.registry.entity_of(serial) {
                    self.state.play_sound_to(entity, sound);
                }
            }
            Command::GiveItem {
                serial,
                graphic,
                hue,
                amount,
                stackable,
            } => self.give_item(serial, graphic, hue, amount, stackable),
            Command::GiveItemKind {
                serial,
                item_kind,
                material,
                amount,
                stackable,
            } => {
                items::give_kind_to_backpack(&mut self.state, serial, item_kind, material, amount, stackable);
            }
            Command::TakeItem {
                serial,
                graphic,
                amount,
            } => self.take_item(serial, graphic, amount),
            Command::TakeItemKind {
                serial,
                item_kind,
                material,
                amount,
            } => self.take_item_kind(serial, item_kind, material, amount),
            Command::RequestCast { connection, spell } => self.begin_cast(connection, spell),
            Command::StockVendor { serial, stock } => {
                npc::stock(&mut self.state, serial, stock);
            }
            Command::AddLoot {
                container,
                graphic,
                hue,
                amount,
                stackable,
            } => self.add_loot(container, graphic, hue, amount, stackable),
            Command::AddLootKind {
                container,
                item_kind,
                material,
                amount,
                stackable,
            } => self.add_loot_kind(container, item_kind, material, amount, stackable),
            Command::ConsumeItem { serial, amount } => {
                items::consume(&mut self.state, serial, amount);
            }
            Command::Buy {
                connection,
                vendor,
                purchases,
            } => npc::buy(&mut self.state, connection, vendor, &purchases),
            Command::Sell {
                connection,
                vendor,
                sales,
            } => npc::sell(&mut self.state, connection, vendor, &sales),
        }
    }

    fn house_inventory(
        &mut self,
        connection: ConnectionId,
        request: openshard_protocol::house_inventory::HouseInventoryRequest,
    ) {
        use openshard_protocol::house_inventory::{
            HouseInventoryRefusal as Refusal,
            HouseInventoryReply as Reply,
            HouseInventoryRow,
        };

        let Some(&player) = self.state.players.get(&connection) else {
            self.state.send_packet(
                connection,
                &ServerPacket::HouseInventory(Reply::Refused {
                    reason:        Refusal::NotInHouse,
                    current_epoch: 0,
                }),
            );
            return;
        };
        let reply = match request {
            openshard_protocol::house_inventory::HouseInventoryRequest::Search {
                expected_epoch,
                selectors,
                after,
                limit,
            } => {
                match openshard_housing::inventory::search(
                    &self.state,
                    player,
                    expected_epoch,
                    &selectors,
                    after,
                    usize::from(limit),
                ) {
                    Ok(page) => {
                        Reply::Page {
                            epoch: page.epoch,
                            rows:  page
                                .rows
                                .into_iter()
                                .map(|row| {
                                    HouseInventoryRow {
                                        identity:        row.identity,
                                        aggregate_total: row.aggregate_total,
                                        root:            row.root,
                                        root_total:      row.root_total,
                                        first_pile:      row.first_pile,
                                        pile_count:      u32::try_from(row.pile_count).unwrap_or(u32::MAX),
                                    }
                                })
                                .collect(),
                            next:  page.next,
                        }
                    }
                    Err(openshard_housing::inventory::SearchRefusal::NotInAHouse) => {
                        Reply::Refused {
                            reason:        Refusal::NotInHouse,
                            current_epoch: 0,
                        }
                    }
                    Err(openshard_housing::inventory::SearchRefusal::Banned) => {
                        Reply::Refused {
                            reason:        Refusal::Banned,
                            current_epoch: 0,
                        }
                    }
                    Err(openshard_housing::inventory::SearchRefusal::Index(error)) => {
                        match error {
                            openshard_state::HouseInventoryError::EmptySelectors
                            | openshard_state::HouseInventoryError::TooManySelectors
                            | openshard_state::HouseInventoryError::InvalidPageSize => {
                                Reply::Refused {
                                    reason:        Refusal::InvalidRequest,
                                    current_epoch: 0,
                                }
                            }
                            openshard_state::HouseInventoryError::Unavailable { epoch } => {
                                Reply::Refused {
                                    reason:        Refusal::Unavailable,
                                    current_epoch: epoch,
                                }
                            }
                            openshard_state::HouseInventoryError::StaleEpoch { current } => {
                                Reply::Refused {
                                    reason:        Refusal::Stale,
                                    current_epoch: current,
                                }
                            }
                        }
                    }
                }
            }
            openshard_protocol::house_inventory::HouseInventoryRequest::Resolve {
                epoch,
                identity,
                root,
                item,
            } => {
                if openshard_housing::inventory::resolve(&self.state, player, epoch, identity, root, item)
                    .is_some()
                {
                    // A result grants no mutation right. Revalidation above is
                    // followed only by the ordinary checked container-open door.
                    let _ = items::double_click(&mut self.state, connection, root);
                    Reply::Resolved { epoch, root, item }
                } else {
                    Reply::Refused {
                        reason:        Refusal::NotFound,
                        current_epoch: epoch,
                    }
                }
            }
        };
        self.state
            .send_packet(connection, &ServerPacket::HouseInventory(reply));
    }

    /// Give every brain due a beat. The deciding is [`ai::think_one`]'s; the world
    /// only applies the one thing a brain cannot do itself — a step — since it
    /// owns movement. A creature that gets a `Combat` from the brain is fought by
    /// `combat::swings` exactly as a player would be.
    fn think(&mut self) {
        // Violence answered first: whoever was struck since the last tick turns
        // on its attacker (or turns tail), before any beat is spent.
        let blows: Vec<openshard_combat::MobileDamaged> =
            self.state.bus.read(&mut self.damaged).copied().collect();
        if !blows.is_empty() {
            ai::retaliate(&mut self.state, &blows);
            combat::retaliate_players(&mut self.state, &blows);
        }
        let now = self.state.ticks;
        let thinkers: Vec<EntityId> = self
            .state
            .registry
            .query::<Brain>()
            .filter(|(_, brain)| now >= brain.next_think)
            .map(|(entity, _)| entity)
            .collect();
        for creature in thinkers {
            // A ridden mount is out of the world; its legs are the rider's.
            if self.state.registry.has::<Ridden>(creature) {
                continue;
            }
            // Level of detail: a creature no player is near need not pay for the
            // full decision (line of sight, target scan, pathfinding) this beat.
            // One already engaged keeps simulating — a fight must not freeze
            // because the player stepped a tile out of range — otherwise it
            // dozes, its next think pushed out by `lod_idle_factor`. Because
            // `lod_radius` sits above the largest sight, "no player near" implies
            // "no player in sight", so nothing is acquired or chased by skipping.
            if self.state.gameplay.lod {
                let engaged = self
                    .state
                    .registry
                    .get::<Combat>(creature)
                    .and_then(|combat| combat.target())
                    .is_some();
                if !engaged {
                    let facet = self.state.facet_of(creature);
                    let pos = self.state.registry.get::<Position>(creature).map(|p| p.0);
                    let near = pos.is_some_and(|p| {
                        self.state
                            .any_player_near(p, self.state.gameplay.lod_radius, facet)
                    });
                    if !near {
                        let base = self.brain_beat(creature);
                        let doze = base * self.state.gameplay.lod_idle_factor;
                        let armed = openshard_npc::next_beat(&mut self.state.rng, now, doze);
                        if let Some(brain) = self.state.registry.get_mut::<Brain>(creature) {
                            brain.next_think = armed;
                        }
                        continue;
                    }
                }
            }
            // A pet does not decide anything: it carries out its last order, which
            // is a different beat from a wild brain's and takes the place of it.
            let step = if self
                .state
                .registry
                .has::<openshard_state::components::Pet>(creature)
            {
                ai::pet_beat(&mut self.state, creature)
            } else {
                ai::think_one(&mut self.state, creature)
            };
            if let Some(dir) = step {
                if let Some(serial) = self.state.registry.serial_of(creature) {
                    self.step(serial, dir);
                }
            }
            // A hunter re-beats at its own pace (or the shard's); idle life
            // ambles at half speed. Engagement is read after the think, so the
            // beat that acquired a target already quickens.
            let engaged = self
                .state
                .registry
                .get::<Combat>(creature)
                .and_then(|combat| combat.target())
                .is_some();
            let base = self.brain_beat(creature);
            let interval = if engaged { base } else { base * 2 };
            let armed = openshard_npc::next_beat(&mut self.state.rng, now, interval);
            if let Some(brain) = self.state.registry.get_mut::<Brain>(creature) {
                brain.next_think = armed;
            }
        }
    }

    /// How long this creature waits between decisions: its own pace if the spawn
    /// pinned one, else the shard's `creature_step_ticks`.
    fn brain_beat(&self, creature: EntityId) -> u64 {
        let own = self
            .state
            .registry
            .get::<Brain>(creature)
            .map_or(0, |brain| brain.beat_ticks);
        if own > 0 {
            own
        } else {
            self.state.gameplay.creature_step_ticks.max(1)
        }
    }

    /// Send a content-built gump to a mobile's client — the counterpart
    /// of the admin menu's own `GumpDisplay`. Silent if the serial names
    /// no mobile, or it has no client to draw on.
    fn show_gump(&mut self, serial: Serial, gump_id: GumpId, at: GumpPoint, layout: &str, lines: &[String]) {
        let Some(entity) = self.state.registry.entity_of(serial) else {
            return;
        };
        let Some(&Client { connection, .. }) = self.state.registry.get::<Client>(entity) else {
            return;
        };
        let packet = ServerPacket::GumpDisplay(GumpDisplay {
            serial: GumpKey::on(serial),
            gump_id,
            at,
            layout: layout.to_owned(),
            lines: lines.to_vec(),
        });
        self.state.send_packet(connection, &packet);
    }

    /// Close an open dialog on a player's client. Silent if the serial names no
    /// mobile, or it has no client to close anything on.
    fn close_gump(&mut self, serial: Serial, gump_id: GumpId) {
        let Some(entity) = self.state.registry.entity_of(serial) else {
            return;
        };
        let Some(&Client { connection, .. }) = self.state.registry.get::<Client>(entity) else {
            return;
        };
        let packet = ServerPacket::CloseGump(CloseGump {
            gump_id,
            button: ButtonId::CLOSE_BOX,
        });
        self.state.send_packet(connection, &packet);
    }

    /// Drop an item into a player's backpack — a quest reward. Merges onto a like
    /// pile when `stackable` (gold), else a discrete piece. Silent if the serial
    /// names no mobile or it wears no backpack. Registered presentation is
    /// immediately projected back to semantic identity; arbitrary client art
    /// remains an explicit legacy reward.
    fn give_item(&mut self, serial: Serial, graphic: Graphic, hue: Hue, amount: u16, stackable: bool) {
        let drawn = Drawn { id: graphic, hue };
        if let Some((kind, material)) = kind_from_drawn(drawn) {
            items::give_kind_to_backpack(&mut self.state, serial, kind, material, amount, stackable);
        } else {
            items::give_to_backpack(&mut self.state, serial, graphic, hue, amount, stackable);
        }
    }

    /// Take up to `amount` of a graphic from a player's backpack — all-or-nothing,
    /// so a collect quest either completes cleanly or takes nothing. Reports the
    /// result with an [`ItemsTaken`](crate::ItemsTaken) event the pack reads next
    /// tick. Nothing (and `taken: 0`) if the serial names no mobile or it wears no
    /// backpack.
    fn take_item(&mut self, serial: Serial, graphic: Graphic, amount: u16) {
        let taken = items::take_from_backpack(&mut self.state, serial, graphic, amount);
        self.state.bus.send(openshard_items::ItemsTaken {
            player: serial,
            graphic,
            item_kind: None,
            material: None,
            taken,
        });
    }

    /// Semantic form of [`Self::take_item`]. It keeps the audited legacy seam
    /// while the save migration is in flight, but never lets an unrelated item
    /// with matching art satisfy a typed quest objective.
    fn take_item_kind(
        &mut self,
        serial: Serial,
        item_kind: openshard_protocol::item_kind::ItemKindId,
        material: Option<openshard_protocol::item_kind::MaterialId>,
        amount: u16,
    ) {
        let Some(drawn) = presentation_of(item_kind, material) else {
            self.state.bus.send(openshard_items::ItemsTaken {
                player: serial,
                graphic: Graphic(0),
                item_kind: Some(item_kind),
                material,
                taken: 0,
            });
            return;
        };
        let taken = items::take_from_backpack_identity_or_legacy(
            &mut self.state,
            serial,
            item_kind,
            material,
            drawn,
            amount,
        );
        self.state.bus.send(openshard_items::ItemsTaken {
            player: serial,
            graphic: drawn.id,
            item_kind: Some(item_kind),
            material,
            taken,
        });
    }

    fn disconnect(&mut self, connection: ConnectionId) {
        // Release a held item while the connection row still exists: the item
        // edge and the cursor reverse projection are one transition now.
        //
        // One `remove` for everything the connection was in the middle of — what
        // it was last told about the light, the music and its own numbers goes
        // silently, which is right: a connection id can be reused, and a reconnect
        // inheriting the last one's remembered light would sit in daylight inside
        // a cave. Only the cursor needs an answer, because an item on it is
        // nowhere — off the ground and out of any container — until something puts
        // it back.
        let held = self.state.held_of(connection);
        if let Some(held) = held {
            items::restore(&mut self.state, held);
        }
        // The world's own row for this client can now go. Unconditional,
        // because a connection that never picked a character has one of these
        // and nothing else below.
        self.state.forget_connection(connection);

        let Some(entity) = self.state.players.remove(&connection) else {
            return;
        };
        // And which sector it was standing in, or the map would keep a row per
        // character that has ever logged in. Someone logging back on reads as a
        // fresh arrival, which is what wakes the ground under them.
        self.forget_sector(entity);
        // A rider logs out *still mounted*: the ride persists. The saddle rides
        // along in the saved inventory below, and `restore_inventory` rebuilds the
        // ridden creature from it on relogin, so the character comes back on
        // horseback where every other emulator would have dropped them on foot.
        // The transient creature itself is despawned once the inventory has
        // captured the saddle that stands for it (below).
        // End any trade it was in, *before* the record and inventory are read
        // below: cancelling puts both sides' offerings back in their own packs,
        // and a trade escrow is deliberately not saved, so an item still sitting
        // in one when the sweep runs is an item nobody gets back.
        items::cancel_for(&mut self.state, entity);
        // And leave any party, while the entity still exists to be removed from
        // one. A party is not saved and its members are online by construction,
        // so a serial left in one after the despawn below names nobody — see
        // `openshard_party::on_logout`.
        openshard_party::on_logout(&mut self.state, entity);
        let serial = self.state.registry.serial_of(entity);
        let facet = self.state.facet_of(entity);

        // Save before despawning, and not by marking it dirty: a `touch` is a
        // promise to read the entity at the next save, and in a moment there
        // will be no entity to read. Logging out is when a save matters most —
        // it is the only moment a player's whole session is at stake — so the
        // record is taken at the one instant it still can be.
        if let Some(record) = Self::record_of(&self.state.registry, entity, self.state.ticks) {
            // The journal copy is for the store; the roster copy is what a
            // re-login reads, because it can arrive before the deferred store
            // save has landed. Written here, at the same instant, so the two
            // cannot describe different logouts.
            self.roster.remember(record.clone());
            // The carried inventory, walked now for the same reason as the record:
            // in a moment the items are despawned with the character and there is
            // nothing left to read. Two copies, for two readers: the journal's for
            // the store, and `pending_inventories` so a re-login *this run* re-equips
            // it — the same fast-relogin path the departed record cache serves, and
            // without it a relog before the next save loses everything carried.
            let items = self.inventory_of(entity);
            self.pending_inventories.insert(record.serial, items.clone());
            self.journal.keep_inventory(Inventory {
                owner: record.serial,
                items,
            });
            self.journal.keep(record);
        }

        // The ridden creature lived only in limbo; the saddle that rebuilds it is
        // now safely in the saved inventory, so drop the creature (the saddle item
        // itself goes with the character's belongings below).
        if let Some(&Riding { mount, .. }) = self.state.registry.get::<Riding>(entity) {
            self.state.registry.despawn(mount);
        }

        // Take it off every screen *before* despawning: once the entity is gone
        // its serial is released and there is nothing left to tell anyone about.
        if let Some(serial) = serial {
            for watcher in self.state.watchers_of(entity) {
                self.state.forget(watcher, entity, serial);
            }
        }
        self.state.seen.remove(&entity);
        self.state.unplace(facet, entity);
        // The character's worn items — its backpack and whatever is in it — are
        // not saved yet, so they go with it rather than orphaning on a serial that
        // is about to be released and reused.
        if let Some(serial) = serial {
            items::despawn_belongings(&mut self.state, serial);
        }
        self.state.registry.despawn(entity);

        if let Some(serial) = serial {
            self.state.bus.send(PlayerLeft {
                connection,
                entity,
                serial,
            });
            info!(%serial, "left the world");
        }
    }

    /// Say that a connection asked to enter and did not.
    ///
    /// Every caller is a failure path inside [`enter`](Self::enter) that used to
    /// end in a bare `return`. Emitting rather than answering the client directly:
    /// what to *do* about it — close the socket, log it, count it — is the shard
    /// loop's, and the world's job ends at saying so.
    fn refuse_entry(&mut self, connection: ConnectionId, reason: RefusedEntry) {
        self.state.bus.send(PlayerRefused { connection, reason });
    }
}

#[cfg(test)]
mod chunks_tests;
#[cfg(test)]
mod crafting_tests;
#[cfg(test)]
mod harvest_tests;
#[cfg(test)]
mod interest_tests;
#[cfg(test)]
mod mapedit_tests;
#[cfg(test)]
mod persistence_tests;
#[cfg(test)]
mod quest_tests;
#[cfg(test)]
mod region_tests;
#[cfg(test)]
mod skills_tests;
#[cfg(test)]
mod status_tests;
#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod trade_tests;
#[cfg(test)]
mod travel_tests;
