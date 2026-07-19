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

/// How often the world ticks.
///
/// 20Hz. Fast enough that a 200ms walk step lands within a tick of when the
/// client expects it, and slow enough to leave room for everything a tick will
/// eventually do. Not a protocol constant — the client does not know or care.
pub const TICK_INTERVAL: Duration = Duration::from_millis(50);

/// A human male body.
const BODY_HUMAN_MALE: u16 = 0x0190;
/// The graphic, container gump, and paperdoll layer of a starting backpack.
/// Layer 0x15 is UO's `Layer.Backpack`; the gump `0x003C` is the bag window the
/// client draws when it is opened.
const BACKPACK_GRAPHIC: u16 = 0x0E75;
const BACKPACK_GUMP: u16 = 0x003C;
const BACKPACK_LAYER: u8 = 0x15;
/// How far an idle banker may drift from its post before it heads back — a couple
/// of tiles of shuffling near the counter, not a stroll out the door.
const BANKER_WANDER: u8 = 2;
/// The skin hue a character gets when nothing else chose one — the same one
/// Sphere hands a body with no stored colour.
const DEFAULT_HUE: u16 = 0x83EA;
/// Full daylight. The scale runs backwards: 0 is brightest, 0x1F pitch dark.
const LIGHT_DAY: u8 = 0;
/// The facet a new character spawns on, and the world's fallback for a facet it
/// has not loaded. Zero is Felucca.
const DEFAULT_FACET: u8 = 0;
/// The height to use when there is no map to ask. Only the tests still name it;
/// the world reads the flat default through [`WorldState::start_position`].
#[cfg(test)]
const Z_WITHOUT_A_MAP: i8 = 0;
/// Notoriety 0x01 is "innocent" — the blue health bar.
const NOTORIETY_INNOCENT: u8 = 0x01;
/// The facet size used when there is no map. Big enough for anywhere a test
/// puts something; the grid is a `Vec` of empty buckets and costs nothing.
const FACET_WITHOUT_A_MAP: (u32, u32) = (7168, 4096);
/// The strength a character starts with, and so — hit points deriving from
/// strength — its starting hit points. A placeholder for what character creation
/// will set.
const DEFAULT_HITPOINTS: u16 = 100;
/// The intelligence a character starts with, and so its starting mana.
const DEFAULT_MANA: u16 = 100;
/// The dexterity a character starts with.
const DEFAULT_DEXTERITY: u16 = 100;
/// A body's own weight in stones, before anything it carries — Sphere's and
/// ServUO's `BodyWeight`. Sent on the status bar; kept well under the carry cap so
/// the client never thinks it is overloaded and refuses to run.
const BODY_WEIGHT: u16 = 14;
/// The sum of the three stats a character may train to — the classic 225.
const STAT_CAP: u16 = 225;
/// How many pets may follow a character. Only the shape matters until pets do.
const MAX_FOLLOWERS: u8 = 5;

/// The weight a character can carry before it is overloaded, from its strength.
///
/// UO's `40 + floor(3.5 * str)`. Only the *ceiling* is sent on the status bar,
/// and only so the client can see it is not over it; nothing enforces it yet.
const fn max_weight(strength: u16) -> u16 {
    40 + strength * 7 / 2
}
/// Ticks between a brain's beats — half a second at [`TICK_INTERVAL`]. Creatures
/// think in beats, not every tick: it paces their walk and spares the loop from
/// re-deciding a thousand times a second what has not changed.
const AI_THINK_TICKS: u64 = 10;
/// The seed the world's roll generator starts from.
///
/// Fixed, so a fresh world's rolls are reproducible in a test and a replay. A
/// live shard that wanted unpredictable rolls would seed from the clock at
/// startup and save the seed with the world; that is an additive change, and one
/// value, not a redesign.
const DEFAULT_SEED: u64 = 0x0DEE_5340_0000_0001;

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

/// One door in a [`Command::Decorate`] batch. The closed/open graphics and the
/// hinge offset are already resolved by whoever places it (the pack does the
/// door-family arithmetic); the world only stores and toggles.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecorDoor {
    /// The shut graphic.
    pub closed: u16,
    /// The open graphic.
    pub open: u16,
    /// East/west hinge swing.
    pub offset_x: i16,
    /// North/south hinge swing.
    pub offset_y: i16,
    /// Where it sits, shut.
    pub position: Point,
}

/// One container in a [`Command::Decorate`] batch — a town chest or crate that
/// opens onto a gump.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecorContainer {
    /// The item graphic.
    pub graphic: u16,
    /// The gump the client opens for it.
    pub gump: u16,
    /// Its hue, or 0.
    pub hue: u16,
    /// Where.
    pub position: Point,
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
        /// The staff authority the account plays with — what privileged commands
        /// its characters may run. Re-derived from the account each login, never
        /// saved with the character.
        access: AccessLevel,
    },
    /// A client asked to take a step.
    Walk {
        /// Which connection.
        connection: ConnectionId,
        /// The request.
        request: WalkRequest,
    },
    /// A client asked for its own status again — a `0x34`, sent when the paperdoll
    /// opens. The status went out at world entry; this resends it so a paperdoll
    /// opened much later is not stale.
    RequestStatus {
        /// Which connection asked.
        connection: ConnectionId,
    },
    /// A client answered a gump — a `0xB1`. The world routes it to whatever opened
    /// the gump; today that is only the `.admin` menu.
    GumpResponse {
        /// Which connection answered.
        connection: ConnectionId,
        /// The decoded response: which gump, which button, and any fields.
        response: openshard_protocol::GumpResponse,
    },
    /// A client answered a targeting cursor — a `0x6C`. Routed to whatever raised
    /// the cursor; today that is the `.tele` command.
    TargetResponse {
        /// Which connection answered.
        connection: ConnectionId,
        /// The decoded response: what was clicked, or a cancel.
        response: openshard_protocol::TargetResponse,
    },
    /// The script pack registers a spawn region — an area the tick then keeps
    /// populated. See [`crate::spawner`].
    RegisterSpawner {
        /// The region to add.
        spawner: crate::spawner::Spawner,
    },
    /// Remove every spawn region and despawn the creatures they were maintaining —
    /// what the admin menu's "Clear spawns" does.
    ClearSpawners,
    /// Place a batch of decoration: script-added statics — signs, furniture — on
    /// top of the static art the map already draws, plus the interactive kinds:
    /// doors that open on double-click and containers that open onto a gump. See
    /// [`Decoration`], [`Door`] and [`Container`].
    Decorate {
        /// Which facet.
        facet: u8,
        /// The plain statics to place, as `(graphic, hue, position)`.
        statics: Vec<(u16, u16, Point)>,
        /// The doors to place.
        doors: Vec<DecorDoor>,
        /// The containers to place.
        containers: Vec<DecorContainer>,
    },
    /// Remove every script-placed decoration.
    ClearDecorations,
    /// Generate functional doors from the map's static frames in a region — the
    /// shop doors a building's art only implies. See [`crate::doorgen`].
    GenerateDoors {
        /// Which facet.
        facet: u8,
        /// The region's north-west corner and size, in tiles.
        x: u16,
        /// North-west corner y.
        y: u16,
        /// Region width.
        width: u16,
        /// Region height.
        height: u16,
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
    /// The server puts a mobile in the world — a script decree. A creature to
    /// fight, a shopkeeper to stand there: an entity with a body and hit points
    /// but no client driving it.
    SpawnMobile {
        /// The body graphic (a creature id, or a human body).
        body: u16,
        /// Its hue.
        hue: u16,
        /// Its starting and maximum hit points.
        hits: u16,
        /// Its standing — the health-bar colour — as a wire byte (1 innocent, 5
        /// enemy, 7 invulnerable). Zero, or anything unknown, is innocent.
        notoriety: u8,
        /// How hard it hits in melee, before the target's armour.
        damage: u16,
        /// Its physical resistance, 0–100.
        resistance: u8,
        /// Ticks between its swings; 0 takes the default.
        swing: u64,
        /// How far it notices a foe, in tiles; 0 is passive, no brain.
        sight: u8,
        /// Whether it wanders when idle.
        wander: bool,
        /// Where it stands.
        position: Point,
        /// Which facet.
        facet: u8,
        /// A name shown on single-click, if any — a townsperson has one.
        name: Option<String>,
        /// Whether it is a banker (answers "bank").
        banker: bool,
        /// Worn clothing and gear, as `(graphic, layer, hue)` — so an NPC is not
        /// naked. Drawn in its `0x78`.
        equipment: Vec<(u16, u8, u16)>,
    },
    /// Deal damage to a mobile — a script or another mobile's blow.
    Damage {
        /// Whom, by wire serial.
        serial: u32,
        /// How much, before armour.
        amount: u16,
        /// What kind, as a wire byte (0 physical, 1 fire, …). The target's
        /// resistance to that kind takes its cut.
        damage_type: u8,
        /// Who dealt it, by wire serial, or zero for unattributed damage — the
        /// caster a script blames a spell's damage on, so killing a blue with it
        /// is a murder the same as a sword.
        by: u32,
    },
    /// Cast a spell: pay mana, roll the casting skill, and say what happened with
    /// a [`SpellCast`](openshard_magic::SpellCast). The spell's *effect* is a
    /// script's — this is only the mana-and-skill gate every spell passes.
    CastSpell {
        /// The caster's serial.
        serial: u32,
        /// Which spell, by id.
        spell: u16,
        /// The target's serial, or zero for a spell that needs none.
        target: u32,
        /// The mana it costs.
        mana: u16,
        /// The casting difficulty, 0–100.
        difficulty: u16,
        /// The skill it rolls (Magery, and its id is the caller's to name).
        skill: u8,
        /// The container to draw reagents from, or zero for a spell that needs
        /// none. The caster's pack, in the usual case.
        pack: u32,
        /// The reagents the spell consumes, as `(graphic, count)`. All must be in
        /// the pack or the spell fizzles, spending nothing.
        reagents: Vec<(u16, u16)>,
    },
    /// Heal a mobile — a spell's or a script's mending. Raises hit points toward
    /// the maximum and never past it.
    Heal {
        /// Whom.
        serial: u32,
        /// By how much.
        amount: u16,
    },
    /// Set a mobile's stats — a script building a character or a monster.
    /// Strength and intelligence re-cap hit points and mana as they change.
    SetStats {
        /// Whose, by wire serial.
        serial: u32,
        /// Strength.
        strength: u16,
        /// Dexterity.
        dexterity: u16,
        /// Intelligence.
        intelligence: u16,
    },
    /// Set a mobile's skill value — a script configuring a character. `value` is
    /// in tenths, capped at [`SKILL_CAP`](openshard_skills::SKILL_CAP).
    SetSkill {
        /// Whose, by wire serial.
        serial: u32,
        /// Which skill, by id.
        skill: u8,
        /// The value in tenths.
        value: u16,
    },
    /// Use a skill against a difficulty (0–100): roll it, gain from it, and say
    /// what happened with a [`SkillUsed`](openshard_skills::SkillUsed) event.
    UseSkill {
        /// Whose, by wire serial.
        serial: u32,
        /// Which skill, by id.
        skill: u8,
        /// The difficulty, 0–100.
        difficulty: u16,
    },
    /// A client toggled war mode (`0x72`).
    WarMode {
        /// Which connection.
        connection: ConnectionId,
        /// True for war, false for peace.
        war: bool,
    },
    /// A client asked to attack a mobile (`0x05`).
    Attack {
        /// Which connection.
        connection: ConnectionId,
        /// The target's serial.
        target: u32,
    },
    /// A client said something (`0x03`).
    Say {
        /// Which connection.
        connection: ConnectionId,
        /// How it is said (mode byte).
        mode: u8,
        /// The colour.
        hue: u16,
        /// The font.
        font: u16,
        /// The words.
        text: String,
    },
    /// A mobile speaks by decree — a script's NPC, or a keyword answer.
    Speak {
        /// Who, by wire serial.
        serial: u32,
        /// The colour.
        hue: u16,
        /// The words.
        text: String,
    },
    /// A client double-clicked an object (`0x06`) — for now, to open a container.
    DoubleClick {
        /// Which connection.
        connection: ConnectionId,
        /// The object's serial.
        serial: u32,
    },
    /// A client single-clicked something and wants its name (`0x09`).
    SingleClick {
        /// Which connection asked.
        connection: ConnectionId,
        /// The clicked object, by serial.
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
    /// Hand a mobile's brain to the script: the built-in `ai` stops driving it and
    /// its `onTick` takes over. A script controls a creature it spawned.
    Control {
        /// The mobile, by wire serial.
        serial: u32,
    },
    /// A client asked to cast a spell (from its spellbook or a macro). The world
    /// only says it happened, via [`SpellRequested`]; a script does the casting.
    RequestCast {
        /// Which connection asked.
        connection: ConnectionId,
        /// Which spell, zero-based.
        spell: u16,
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
pub(crate) mod tests {
    use super::*;
    use openshard_chat::{MobileSpoke, TALKMODE_WHISPER, TALKMODE_YELL};
    use openshard_combat::{swing_ticks, MobileDied, WRESTLING_SPEED};
    use openshard_events::Cursor;
    use openshard_magic::{SpellCast, MANA_REGEN_TICKS};
    use openshard_movement::WALK_INTERVAL;
    use openshard_protocol::{encode_remove, DROP_TO_GROUND};
    use openshard_skills::SkillUsed;
    use openshard_state::components::{
        Amount, Contained, Container, CriminalUntil, Decays, Equipped, Graphic, MurderDecay,
        Murders, Skills, Stackable,
    };

    pub(super) const START: (u16, u16) = (1363, 1600);

    /// Ticks a bare-handed, default-dexterity mobile waits between swings under
    /// the default rules — the pace the combat tests reckon against. `dex 100`,
    /// wrestling, era 1, scale 15000: thirty ticks.
    const WRESTLING_SWING_TICKS: u64 = swing_ticks(100, WRESTLING_SPEED, 1, 15000);

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
            access: AccessLevel::Player,
        });
        world.tick(now);
        connection
    }

    /// Enter as a game master — the authority the `.`-command tests need.
    pub(super) fn enter_gm(world: &mut World, now: Instant) -> ConnectionId {
        let connection = connection();
        world.queue(Command::Enter {
            connection,
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "Lord British".to_owned(),
            serial: None,
            position: None,
            facet: 0,
            appearance: None,
            access: AccessLevel::GameMaster,
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
        let entity = world.state.players[&connection];
        world.state.registry.insert(entity, Position(point));
        if let Some(Movement(mut walker)) = world.state.registry.get::<Movement>(entity).copied() {
            walker.position = point;
            world.state.registry.insert(entity, Movement(walker));
        }
        let facet = world.state.facet_of(entity);
        world
            .state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, point);
        world.state.refresh_around(entity);
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
        let entity = world.state.players[&connection];
        world.state.registry.serial_of(entity).unwrap().raw()
    }

    #[test]
    fn a_server_step_turns_first_then_moves() {
        // Turn-as-step, server side: the first `Step` in a new direction turns
        // and stays put; the second moves. The same rule a client walk follows,
        // because the clients watching cannot tell who ordered the step.
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        let entity = world.state.players[&connection];
        let serial = serial_of(&world, connection);

        let facing0 = world
            .state
            .registry
            .get::<Heading>(entity)
            .unwrap()
            .0
            .direction;
        let dir = if facing0 == Direction::North {
            Direction::South
        } else {
            Direction::North
        };
        let from = world.state.registry.get::<Position>(entity).unwrap().0;

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
            world.state.registry.get::<Position>(entity).unwrap().0,
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
            world.state.registry.get::<Position>(entity).unwrap().0,
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
        let entity = world.state.players[&connection];
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
            world.state.registry.get::<Position>(entity).unwrap().0,
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
            .state
            .registry
            .query::<Position>()
            .filter(|(entity, _)| world.state.registry.has::<Stackable>(*entity))
            .filter_map(|(entity, _)| world.state.registry.serial_of(entity).map(|s| s.raw()))
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
        // The one spawned test item — never a worn backpack, which every character
        // now carries (an item with a `Graphic`, worn via `Equipped`).
        let (entity, _) = world
            .state
            .registry
            .query::<Graphic>()
            .find(|(entity, _)| !world.state.registry.has::<Equipped>(*entity))
            .expect("a loose item is in the world");
        world.state.registry.serial_of(entity).unwrap().raw()
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
            .state
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
            world.state.registry.has::<Position>(item),
            "the item stays on the ground"
        );
        assert!(world.state.held.is_empty(), "and nothing is on the cursor");
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
            .state
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
            world.state.registry.get::<Position>(item).map(|p| p.0),
            Some(origin),
            "and the item is back where it was lifted"
        );
        assert!(world.state.held.is_empty());
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
            .state
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
            world.state.registry.get::<Position>(item).map(|p| p.0),
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
        // The ground container just spawned — not a worn backpack, which is also a
        // container now that every character has one.
        let (entity, _) = world
            .state
            .registry
            .query::<Container>()
            .find(|(entity, _)| world.state.registry.has::<Position>(*entity))
            .expect("a container is on the ground");
        world.state.registry.serial_of(entity).unwrap().raw()
    }

    /// The serial of the one item that is not a container.
    fn loose_item_serial(world: &World) -> u32 {
        let (entity, _) = world
            .state
            .registry
            .query::<Graphic>()
            .find(|(entity, _)| !world.state.registry.has::<Container>(*entity))
            .expect("a non-container item exists");
        world.state.registry.serial_of(entity).unwrap().raw()
    }

    fn entity(world: &World, serial: u32) -> EntityId {
        world
            .state
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

    /// The serial of the backpack a connection's character is wearing.
    fn backpack_serial(world: &World, connection: ConnectionId) -> u32 {
        let owner = world
            .registry()
            .serial_of(world.state.players[&connection])
            .unwrap();
        world
            .registry()
            .query::<Equipped>()
            .find(|(_, worn)| worn.mobile == owner && worn.layer == BACKPACK_LAYER)
            .and_then(|(item, _)| world.registry().serial_of(item))
            .expect("a character wears a backpack")
            .raw()
    }

    #[test]
    fn entering_the_world_equips_a_backpack_and_tells_the_client() {
        // A fresh character has a bag: worn on the backpack layer, a real
        // container, and named to the client in a 0x78 about itself so the client
        // knows the serial to double-click open.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);

        let pack = backpack_serial(&world, player);
        let pack_entity = entity(&world, pack);
        assert!(
            world.registry().has::<Container>(pack_entity),
            "the bag is a container"
        );
        assert!(
            !world.registry().has::<Position>(pack_entity),
            "a worn bag is off the ground"
        );
        assert!(
            packets_for(&mut world, player).iter().any(|p| p[0] == 0x78),
            "the client is told its own equipment"
        );
    }

    #[test]
    fn double_clicking_your_own_backpack_opens_it() {
        // The bag is worn, not on the ground, so the old ground-only open would
        // have refused it. Your own pack is always in reach.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let pack = backpack_serial(&world, player);
        let _ = packets_for(&mut world, player);

        world.queue(Command::DoubleClick {
            connection: player,
            serial: pack,
        });
        world.tick(now);

        let packets = packets_for(&mut world, player);
        assert!(packets.iter().any(|p| p[0] == 0x24), "the bag gump opens");
        assert!(packets.iter().any(|p| p[0] == 0x3C), "its contents follow");
    }

    #[test]
    fn dropping_an_item_into_your_worn_backpack_stores_it() {
        // The bug the user hit: a worn bag has no `Position`, so the drop-into
        // reach check bounced the item and the client's cursor desynced. The
        // wearer's tile has to stand in for the bag's.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let pack = backpack_serial(&world, player);
        let here = world
            .registry()
            .get::<Position>(world.state.players[&player])
            .unwrap()
            .0;
        spawn_item_at(&mut world, here, now);
        let item_serial = loose_item_serial(&world);
        let item = entity(&world, item_serial);

        world.queue(Command::PickUpItem {
            connection: player,
            serial: item_serial,
            amount: 1,
        });
        world.tick(now);
        world.queue(Command::DropItem {
            connection: player,
            serial: item_serial,
            position: Point::new(0, 0, 0),
            container: pack,
        });
        world.tick(now);

        assert!(
            world.state.registry.has::<Contained>(item),
            "the item is now inside the worn bag"
        );
        assert_eq!(
            world
                .registry()
                .get::<Contained>(item)
                .unwrap()
                .container
                .raw(),
            pack
        );
        assert!(
            !world.state.held.contains_key(&player),
            "and off the cursor, not bounced"
        );
    }

    #[test]
    fn double_clicking_yourself_opens_the_paperdoll() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let serial = world
            .registry()
            .serial_of(world.state.players[&player])
            .unwrap()
            .raw();
        let _ = packets_for(&mut world, player);

        world.queue(Command::DoubleClick {
            connection: player,
            serial,
        });
        world.tick(now);

        assert!(
            packets_for(&mut world, player).iter().any(|p| p[0] == 0x88),
            "double-clicking a mobile opens its paperdoll"
        );
    }

    #[test]
    fn logging_out_despawns_the_backpack() {
        // Equipment is not persisted yet, so it must not outlive its wearer as an
        // orphan equipped on a serial about to be reused.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let pack = backpack_serial(&world, player);
        let pack_entity = entity(&world, pack);

        world.queue(Command::Disconnect { connection: player });
        world.tick(now);

        assert!(
            !world.registry().contains(pack_entity),
            "the bag went with the character"
        );
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
            .state
            .registry
            .get::<Contained>(item)
            .expect("the item is now in a container");
        assert_eq!(contained.container.raw(), container);
        assert_eq!((contained.x, contained.y), (50, 60));
        assert!(
            !world.state.registry.has::<Position>(item),
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
        assert!(world.state.registry.has::<Contained>(item));

        world.queue(Command::PickUpItem {
            connection: player,
            serial: item_serial,
            amount: 1,
        });
        world.tick(now);
        assert!(
            !world.state.registry.has::<Contained>(item),
            "lifting it out drops the containment"
        );
        assert!(
            world.state.held.contains_key(&player),
            "and it is on the cursor"
        );
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
        // The held one is whichever loose item is not the target — not the worn
        // backpack, which is also an item with a graphic now.
        let held_serial = world
            .state
            .registry
            .query::<Graphic>()
            .filter(|(e, _)| !world.state.registry.has::<Equipped>(*e))
            .filter_map(|(e, _)| world.state.registry.serial_of(e).map(|s| s.raw()))
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
            world.state.registry.get::<Position>(held_item).map(|p| p.0),
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
            .state
            .registry
            .query::<Position>()
            .filter(|(entity, _)| {
                world.state.registry.has::<Graphic>(*entity)
                    && !world.state.registry.has::<Container>(*entity)
            })
            .filter_map(|(entity, _)| {
                world
                    .state
                    .registry
                    .serial_of(entity)
                    .map(|s| (entity, s.raw()))
            })
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
            .state
            .registry
            .get::<Equipped>(item)
            .expect("the item is now worn");
        assert_eq!(worn.mobile.raw(), me);
        assert_eq!(worn.layer, LAYER_TORSO);
        // Three worn things now: the torso item, and the backpack and bank box every
        // character is given on entry.
        assert_eq!(world.state.equipment_of(Serial::new(me).unwrap()).len(), 3);
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

        assert!(!world.state.registry.has::<Equipped>(item), "it comes off");
        assert!(
            world.state.held.contains_key(&player),
            "and onto the cursor"
        );
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
            world.state.registry.has::<Position>(second_item),
            "and returns to where it was lifted"
        );
        assert!(!world.state.registry.has::<Equipped>(second_item));
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
            world.state.registry.has::<Position>(held_item),
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
            world.state.registry.get::<Amount>(pile_item).map(|a| a.0),
            Some(150),
            "the amounts add"
        );
        assert!(
            !world.state.registry.contains(loose_item),
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
            world.state.registry.has::<Position>(held_item),
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
        let soon = world.state.ticks + 1;
        world.state.registry.insert(item, Decays { at_tick: soon });
        world.tick(now);

        assert!(
            !world.state.registry.contains(item),
            "the item has rotted away"
        );
        assert!(
            packets_for(&mut world, watcher)
                .iter()
                .any(|p| p == &encode_remove(serial)),
            "and left every screen"
        );
    }

    #[test]
    fn gameplay_config_reaches_the_systems() {
        // The [gameplay] knobs flow through WorldState to the systems: a five-second
        // decay here gives a spawned item a clock of a hundred ticks, not the
        // twenty-minute default's twenty-four thousand.
        let now = Instant::now();
        let gameplay = Gameplay::new(2, 40000, 700, 5, 60, 18, 3, 31);
        let mut world = World::new(START).with_gameplay(gameplay);
        world.queue(Command::SpawnItem {
            graphic: 0x0EED,
            hue: 0,
            amount: 1,
            stackable: false,
            position: Point::new(START.0, START.1, 0),
            facet: 0,
        });
        world.tick(now);

        let serial = loose_item_serial(&world);
        let item = entity(&world, serial);
        let decay = world.state.registry.get::<Decays>(item).unwrap();
        assert!(
            decay.at_tick > world.state.ticks && decay.at_tick <= world.state.ticks + 100,
            "the five-second decay reached mark_decay (at_tick {}, now {})",
            decay.at_tick,
            world.state.ticks
        );
    }

    #[test]
    fn a_container_does_not_decay_even_after_being_moved() {
        // A backpack is a ground item too, but it must not rot — and picking it
        // up and setting it back down must not hand it a decay clock either.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let here = Point::new(START.0, START.1, 0);
        let container = spawn_container_at(&mut world, here, now);
        let container_item = entity(&world, container);
        assert!(
            !world.state.registry.has::<Decays>(container_item),
            "a fresh container has no decay clock"
        );

        world.queue(Command::PickUpItem {
            connection: player,
            serial: container,
            amount: 1,
        });
        world.tick(now);
        world.queue(Command::DropItem {
            connection: player,
            serial: container,
            position: here,
            container: DROP_TO_GROUND,
        });
        world.tick(now);

        assert!(
            world.state.registry.has::<Position>(container_item),
            "back down"
        );
        assert!(
            !world.state.registry.has::<Decays>(container_item),
            "and still no decay clock after moving it"
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
            !world.state.registry.has::<Decays>(item),
            "a held item carries no decay clock"
        );
    }

    #[test]
    fn picking_up_part_of_a_stack_splits_it() {
        // Take 30 of 100: the original keeps its serial and holds the 30 on the
        // cursor, and a new pile of 70 is left on the ground where it was — the
        // way Sphere's UnStackSplit does it.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let here = Point::new(START.0, START.1, 0);
        let pile = spawn_gold(&mut world, here, 100, now);
        let pile_item = entity(&world, pile);
        let _ = packets_for(&mut world, player);

        world.queue(Command::PickUpItem {
            connection: player,
            serial: pile,
            amount: 30,
        });
        world.tick(now);

        // The original, still serial `pile`, is on the cursor holding 30.
        assert!(world.state.held.contains_key(&player));
        assert_eq!(openshard_items::amount_of(&world.state, pile_item), 30);
        assert!(
            !world.state.registry.has::<Position>(pile_item),
            "off the ground"
        );

        // A brand-new pile of 70 sits where the stack was.
        let (leftover, _) = world
            .state
            .registry
            .query::<Position>()
            .find(|(entity, _)| {
                world.state.registry.has::<Stackable>(*entity) && *entity != pile_item
            })
            .expect("a leftover pile on the ground");
        assert_eq!(openshard_items::amount_of(&world.state, leftover), 70);
        assert_ne!(
            world.state.registry.serial_of(leftover).unwrap().raw(),
            pile,
            "the leftover is a new object with a new serial"
        );
        assert!(
            packets_for(&mut world, player).iter().any(|p| p[0] == 0x1A),
            "and the player is drawn the leftover pile"
        );
    }

    #[test]
    fn the_split_portion_keeps_its_serial_and_can_be_dropped() {
        // The reason the original keeps its serial: the client's cursor still
        // names it, so the 0x08 that drops the 30 back matches the held item.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let here = Point::new(START.0, START.1, 0);
        let pile = spawn_gold(&mut world, here, 100, now);
        let pile_item = entity(&world, pile);

        world.queue(Command::PickUpItem {
            connection: player,
            serial: pile,
            amount: 30,
        });
        world.tick(now);
        world.queue(Command::DropItem {
            connection: player,
            serial: pile, // the client drops the same serial it lifted
            position: here,
            container: DROP_TO_GROUND,
        });
        world.tick(now);

        assert!(world.state.held.is_empty(), "the drop landed, not bounced");
        assert!(world.state.registry.has::<Position>(pile_item));
        assert_eq!(openshard_items::amount_of(&world.state, pile_item), 30);
    }

    #[test]
    fn picking_up_a_whole_stack_does_not_split_it() {
        // Asking for the whole amount, or more, lifts the pile itself — no
        // leftover, one object.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let here = Point::new(START.0, START.1, 0);
        let pile = spawn_gold(&mut world, here, 100, now);
        let pile_item = entity(&world, pile);

        world.queue(Command::PickUpItem {
            connection: player,
            serial: pile,
            amount: 100,
        });
        world.tick(now);

        assert_eq!(
            openshard_items::amount_of(&world.state, pile_item),
            100,
            "the whole pile is held"
        );
        assert_eq!(
            world
                .state
                .registry
                .query::<Stackable>()
                .filter(|(entity, _)| world.state.registry.has::<Position>(*entity))
                .count(),
            0,
            "nothing is left on the ground"
        );
    }

    /// Spawn a creature at `point` with `hits` and return its serial. An orange
    /// enemy, no armour — the plain punching bag most combat tests want.
    fn spawn_mobile_at(world: &mut World, point: Point, hits: u16, now: Instant) -> u32 {
        spawn_mobile_full(world, point, hits, 5, combat::SWING_DAMAGE, 0, now)
    }

    /// Spawn a creature with every combat field spelled out, and return its serial.
    fn spawn_mobile_full(
        world: &mut World,
        point: Point,
        hits: u16,
        notoriety: u8,
        damage: u16,
        resistance: u8,
        now: Instant,
    ) -> u32 {
        world.queue(Command::SpawnMobile {
            body: 0x0190,
            hue: 0,
            hits,
            notoriety,
            damage,
            resistance,
            swing: 0, // the default pace
            sight: 0, // passive by default; tests that want a brain set it
            wander: false,
            position: point,
            facet: 0,
            name: None,
            banker: false,
            equipment: Vec::new(),
        });
        world.tick(now);
        // The newest mobile that no client drives — the creature just made.
        world
            .state
            .registry
            .query::<Body>()
            .filter(|(entity, _)| !world.state.registry.has::<Client>(*entity))
            .filter_map(|(entity, _)| world.state.registry.serial_of(entity).map(|s| s.raw()))
            .max()
            .expect("a spawned creature")
    }

    #[test]
    fn a_spawned_creature_is_drawn_to_nearby_players() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let _ = packets_for(&mut world, player);
        let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);

        assert!(
            packets_for(&mut world, player)
                .iter()
                .any(|p| p[0] == 0x78 && mentions(p, mob)),
            "the creature is drawn to the player"
        );
    }

    #[test]
    fn damage_lowers_hits_and_updates_the_bar() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);
        let mob_entity = entity(&world, mob);
        let _ = packets_for(&mut world, player);

        world.queue(Command::Damage {
            serial: mob,
            amount: 20,
            damage_type: 0,
            by: 0,
        });
        world.tick(now);

        assert_eq!(
            world
                .state
                .registry
                .get::<Hitpoints>(mob_entity)
                .map(|h| h.current),
            Some(30),
            "50 minus 20"
        );
        assert!(
            packets_for(&mut world, player).iter().any(|p| p[0] == 0xA1),
            "the health bar is redrawn"
        );
    }

    #[test]
    fn a_creature_dies_at_zero_hits() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 10, now);
        let mob_entity = entity(&world, mob);
        let _ = packets_for(&mut world, player);
        let mut died: Cursor<MobileDied> = world.bus().cursor();

        // Overkill: it dies once, not into the negatives.
        world.queue(Command::Damage {
            serial: mob,
            amount: 100,
            damage_type: 0,
            by: 0,
        });
        world.tick(now);

        assert_eq!(world.bus().read(&mut died).count(), 1, "death is announced");
        assert!(
            !world.state.registry.contains(mob_entity),
            "and the creature is removed"
        );
        assert!(
            packets_for(&mut world, player)
                .iter()
                .any(|p| p == &encode_remove(mob)),
            "and taken off the player's screen"
        );
    }

    #[test]
    fn a_dead_mobile_is_not_killed_again() {
        // A player lies at zero hits without being despawned; a second blow must
        // not announce a second death.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let serial = serial_of(&world, player);
        let mut died: Cursor<MobileDied> = world.bus().cursor();

        world.queue(Command::Damage {
            serial,
            amount: 200,
            damage_type: 0,
            by: 0,
        });
        world.tick(now); // 100 -> 0
        assert_eq!(world.bus().read(&mut died).count(), 1, "the killing blow");

        world.queue(Command::Damage {
            serial,
            amount: 50,
            damage_type: 0,
            by: 0,
        });
        world.tick(now); // already dead
        assert_eq!(
            world.bus().read(&mut died).count(),
            0,
            "a second blow on a corpse announces nothing"
        );
    }

    #[test]
    fn a_player_who_dies_stays_in_the_world() {
        // Ghosts and corpses are a later slice; for now death is announced but a
        // connected player is not yanked out of the world.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let serial = serial_of(&world, player);
        let player_entity = world.state.players[&player];
        let mut died: Cursor<MobileDied> = world.bus().cursor();

        world.queue(Command::Damage {
            serial,
            amount: 500,
            damage_type: 0,
            by: 0,
        });
        world.tick(now);

        assert_eq!(world.bus().read(&mut died).count(), 1, "death is announced");
        assert!(
            world.state.registry.contains(player_entity),
            "but the player is still here"
        );
        assert_eq!(
            world
                .state
                .registry
                .get::<Hitpoints>(player_entity)
                .map(|h| h.current),
            Some(0),
        );
    }

    /// Put a player in war mode, aimed at `target`, in one tick.
    fn engage(world: &mut World, player: ConnectionId, target: u32, now: Instant) {
        world.queue(Command::WarMode {
            connection: player,
            war: true,
        });
        world.queue(Command::Attack {
            connection: player,
            target,
        });
        world.tick(now);
    }

    #[test]
    fn war_mode_and_attack_are_confirmed_to_the_client() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);
        let _ = packets_for(&mut world, player);

        world.queue(Command::WarMode {
            connection: player,
            war: true,
        });
        world.queue(Command::Attack {
            connection: player,
            target: mob,
        });
        world.tick(now);

        let packets = packets_for(&mut world, player);
        assert!(
            packets.iter().any(|p| p == &[0x72, 0x01, 0x00, 0x32, 0x00]),
            "war mode is confirmed"
        );
        assert!(
            packets.iter().any(|p| p[0] == 0xAA && mentions(p, mob)),
            "and the target is set"
        );
    }

    #[test]
    fn a_player_in_war_mode_swings_at_an_adjacent_target() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);
        let mob_entity = entity(&world, mob);
        engage(&mut world, player, mob, now);

        // One swing interval later, a blow has landed.
        for _ in 0..WRESTLING_SWING_TICKS {
            world.tick(now);
        }
        assert!(
            world
                .state
                .registry
                .get::<Hitpoints>(mob_entity)
                .unwrap()
                .current
                < 50,
            "the target has taken damage"
        );
    }

    #[test]
    fn no_swing_without_war_mode() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);
        let mob_entity = entity(&world, mob);

        // Aim, but stay at peace.
        world.queue(Command::Attack {
            connection: player,
            target: mob,
        });
        world.tick(now);
        for _ in 0..(WRESTLING_SWING_TICKS + 1) {
            world.tick(now);
        }
        assert_eq!(
            world
                .state
                .registry
                .get::<Hitpoints>(mob_entity)
                .unwrap()
                .current,
            50,
            "a mobile at peace does not swing"
        );
    }

    #[test]
    fn no_swing_out_of_reach() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        // Well outside melee range, but on screen.
        let mob = spawn_mobile_at(&mut world, Point::new(START.0 + 5, START.1, 0), 50, now);
        let mob_entity = entity(&world, mob);
        engage(&mut world, player, mob, now);
        for _ in 0..(WRESTLING_SWING_TICKS + 1) {
            world.tick(now);
        }
        assert_eq!(
            world
                .state
                .registry
                .get::<Hitpoints>(mob_entity)
                .unwrap()
                .current,
            50,
            "a swing out of reach lands nothing"
        );
    }

    #[test]
    fn a_creatures_notoriety_colours_its_health_bar() {
        // Spawn an orange enemy and read the notoriety byte out of the 0x78 that
        // draws it — the health-bar colour on the wire.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let _ = packets_for(&mut world, player);
        let mob = spawn_mobile_full(
            &mut world,
            Point::new(START.0, START.1, 0),
            50,
            5,
            5,
            0,
            now,
        );

        let drawn = packets_for(&mut world, player)
            .into_iter()
            .find(|p| p[0] == 0x78 && mentions(p, mob))
            .expect("the creature is drawn");
        assert_eq!(drawn[18], 0x05, "the notoriety byte is Enemy/orange");
    }

    #[test]
    fn an_invulnerable_mobile_cannot_be_attacked() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let player_entity = world.state.players[&player];
        // Notoriety 7 is invulnerable — a yellow, untouchable townsperson.
        let mob = spawn_mobile_full(
            &mut world,
            Point::new(START.0, START.1, 0),
            50,
            7,
            5,
            0,
            now,
        );
        let _ = packets_for(&mut world, player);

        world.queue(Command::Attack {
            connection: player,
            target: mob,
        });
        world.tick(now);

        assert_eq!(
            world
                .state
                .registry
                .get::<Combat>(player_entity)
                .unwrap()
                .target,
            None,
            "the attack is refused"
        );
        assert!(
            packets_for(&mut world, player)
                .iter()
                .any(|p| p == &[0xAA, 0, 0, 0, 0]),
            "and the client's target is cleared"
        );
    }

    #[test]
    fn attacking_an_innocent_turns_the_attacker_grey() {
        let now = Instant::now();
        let mut world = world();
        let aggressor = enter(&mut world, now);
        let victim = enter_as(&mut world, ConnectionId::from_raw(2), now);
        let aggressor_entity = world.state.players[&aggressor];
        let aggressor_serial = serial_of(&world, aggressor);
        let victim_serial = serial_of(&world, victim);
        let _ = packets_for(&mut world, victim);

        world.queue(Command::Attack {
            connection: aggressor,
            target: victim_serial,
        });
        world.tick(now);

        assert_eq!(
            world.state.notoriety_of(aggressor_entity),
            Notoriety::Criminal,
            "raising a hand against an innocent is a crime"
        );
        assert!(
            packets_for(&mut world, victim)
                .iter()
                .any(|p| p[0] == 0x77 && mentions(p, aggressor_serial)),
            "and everyone watching sees them turn grey"
        );
    }

    #[test]
    fn five_innocent_kills_turn_the_killer_red() {
        // Murderer flagging: the tally of killed innocents is persistent, and the
        // fifth turns the killer red for good.
        let now = Instant::now();
        let mut world = world();
        let killer = enter(&mut world, now);
        let killer_entity = world.state.players[&killer];

        for kill in 1..=5 {
            // A blue, one-hit victim on the killer's tile.
            let victim = spawn_mobile_full(
                &mut world,
                Point::new(START.0, START.1, 0),
                1,
                Notoriety::Innocent.to_bits(),
                0,
                0,
                now,
            );
            engage(&mut world, killer, victim, now);
            for _ in 0..=WRESTLING_SWING_TICKS {
                world.tick(now);
            }
            assert!(
                world
                    .state
                    .registry
                    .entity_of(Serial::new(victim).unwrap())
                    .is_none(),
                "the innocent is dead"
            );
            if kill < 5 {
                assert_ne!(
                    world.state.notoriety_of(killer_entity),
                    Notoriety::Murderer,
                    "still short of the murder threshold after {kill} kills"
                );
            }
        }

        assert_eq!(
            world.state.notoriety_of(killer_entity),
            Notoriety::Murderer,
            "the fifth innocent killed makes a murderer"
        );
    }

    #[test]
    fn murder_counts_fade_and_wash_the_killer_blue() {
        // The count is persistent, not permanent: old kills age off one at a time,
        // and once the killer drops below the threshold it goes back to innocent.
        let now = Instant::now();
        let mut world = world();
        let killer = enter(&mut world, now);
        let killer_entity = world.state.players[&killer];
        let killer_serial = serial_of(&world, killer);

        for _ in 0..5 {
            let victim = spawn_mobile_full(
                &mut world,
                Point::new(START.0 + 5, START.1, 0),
                1,
                Notoriety::Innocent.to_bits(),
                0,
                0,
                now,
            );
            world.queue(Command::Damage {
                serial: victim,
                amount: 100,
                damage_type: 0,
                by: killer_serial,
            });
            world.tick(now);
        }
        assert_eq!(
            world.state.notoriety_of(killer_entity),
            Notoriety::Murderer,
            "five kills, red"
        );

        // Bring the decay forward rather than run eight hours of ticks: one count
        // fades, dropping to four — below the threshold — and the killer washes
        // blue.
        let soon = world.state.ticks + 1;
        world
            .state
            .registry
            .insert(killer_entity, MurderDecay { at_tick: soon });
        world.tick(now);

        assert_eq!(
            world
                .state
                .registry
                .get::<Murders>(killer_entity)
                .map(|m| m.0),
            Some(4),
            "one murder aged off"
        );
        assert_eq!(
            world.state.notoriety_of(killer_entity),
            Notoriety::Innocent,
            "below the threshold, no longer a murderer"
        );
    }

    #[test]
    fn an_attributed_spell_kill_is_a_murder_too() {
        // Attribution is not melee-only: damage that names its dealer — a script's
        // spell blaming its caster — tallies a murder just as a swing does.
        let now = Instant::now();
        let mut world = world();
        let killer = enter(&mut world, now);
        let killer_entity = world.state.players[&killer];
        let killer_serial = serial_of(&world, killer);

        for _ in 0..5 {
            let victim = spawn_mobile_full(
                &mut world,
                Point::new(START.0 + 5, START.1, 0),
                1,
                Notoriety::Innocent.to_bits(),
                0,
                0,
                now,
            );
            world.queue(Command::Damage {
                serial: victim,
                amount: 100,
                damage_type: 0,
                by: killer_serial,
            });
            world.tick(now);
        }

        assert_eq!(
            world.state.notoriety_of(killer_entity),
            Notoriety::Murderer,
            "five innocents killed by attributed spell damage is murder"
        );
    }

    #[test]
    fn unattributed_damage_kills_without_blame() {
        // The other side of it: damage with no dealer named (a script's raw
        // op_damage, an environmental hazard) kills but pins no murder.
        let now = Instant::now();
        let mut world = world();
        let bystander = enter(&mut world, now);
        let bystander_entity = world.state.players[&bystander];

        for _ in 0..5 {
            let victim = spawn_mobile_full(
                &mut world,
                Point::new(START.0 + 5, START.1, 0),
                1,
                Notoriety::Innocent.to_bits(),
                0,
                0,
                now,
            );
            world.queue(Command::Damage {
                serial: victim,
                amount: 100,
                damage_type: 0,
                by: 0,
            });
            world.tick(now);
        }

        assert_ne!(
            world.state.notoriety_of(bystander_entity),
            Notoriety::Murderer,
            "nobody was blamed for unattributed kills"
        );
    }

    #[test]
    fn attacking_an_enemy_is_not_a_crime() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let player_entity = world.state.players[&player];
        // A plain orange enemy.
        let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);

        world.queue(Command::Attack {
            connection: player,
            target: mob,
        });
        world.tick(now);

        assert_eq!(
            world.state.notoriety_of(player_entity),
            Notoriety::Innocent,
            "attacking what is already an enemy costs no standing"
        );
    }

    #[test]
    fn the_criminal_flag_lifts_when_its_time_runs_out() {
        let now = Instant::now();
        let mut world = world();
        let aggressor = enter(&mut world, now);
        let victim = enter_as(&mut world, ConnectionId::from_raw(2), now);
        let aggressor_entity = world.state.players[&aggressor];
        let victim_serial = serial_of(&world, victim);

        world.queue(Command::Attack {
            connection: aggressor,
            target: victim_serial,
        });
        world.tick(now);
        assert_eq!(
            world.state.notoriety_of(aggressor_entity),
            Notoriety::Criminal
        );

        // Bring the flag's expiry forward rather than run two minutes of ticks.
        let soon = world.state.ticks + 1;
        world
            .state
            .registry
            .insert(aggressor_entity, CriminalUntil { tick: soon });
        world.tick(now);

        assert_eq!(
            world.state.notoriety_of(aggressor_entity),
            Notoriety::Innocent,
            "the flag lifts and they are blue again"
        );
    }

    #[test]
    fn resistance_is_by_damage_type() {
        // Fifty percent fire resistance halves a fireball but does nothing to a
        // sword: resistance is per type, applied in one place for every source.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 100, now);
        let mob_entity = entity(&world, mob);
        world.state.registry.insert(
            mob_entity,
            Resistance {
                fire: 50,
                ..Default::default()
            },
        );
        let _ = packets_for(&mut world, player);

        // 10 fire, halved to 5.
        world.queue(Command::Damage {
            serial: mob,
            amount: 10,
            damage_type: 1, // fire
            by: 0,
        });
        world.tick(now);
        assert_eq!(
            world
                .state
                .registry
                .get::<Hitpoints>(mob_entity)
                .unwrap()
                .current,
            95
        );

        // 10 physical, unresisted.
        world.queue(Command::Damage {
            serial: mob,
            amount: 10,
            damage_type: 0, // physical
            by: 0,
        });
        world.tick(now);
        assert_eq!(
            world
                .state
                .registry
                .get::<Hitpoints>(mob_entity)
                .unwrap()
                .current,
            85
        );
    }

    #[test]
    fn armour_reduces_a_blow() {
        // Same five-damage swing, but the target's 50% physical resistance halves
        // it: two through, not five.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let mob = spawn_mobile_full(
            &mut world,
            Point::new(START.0, START.1, 0),
            50,
            5,
            5,
            50,
            now,
        );
        let mob_entity = entity(&world, mob);
        engage(&mut world, player, mob, now);

        for _ in 0..WRESTLING_SWING_TICKS {
            world.tick(now);
        }
        assert_eq!(
            world
                .state
                .registry
                .get::<Hitpoints>(mob_entity)
                .unwrap()
                .current,
            48,
            "five damage minus half is two"
        );
    }

    #[test]
    fn swing_speed_sets_the_cadence() {
        // A faster swinger lands a blow in fewer ticks than the default interval
        // would allow.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let player_entity = world.state.players[&player];
        world
            .state
            .registry
            .insert(player_entity, SwingSpeed { ticks: 5 });
        let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 100, now);
        let mob_entity = entity(&world, mob);
        engage(&mut world, player, mob, now);

        // Five is fewer than the default interval, but a full fast one.
        const _: () = assert!(5 < WRESTLING_SWING_TICKS);
        for _ in 0..5 {
            world.tick(now);
        }
        assert!(
            world
                .state
                .registry
                .get::<Hitpoints>(mob_entity)
                .unwrap()
                .current
                < 100,
            "the quicker swing has already landed"
        );
    }

    #[test]
    fn a_spawned_creature_derives_its_swing_speed() {
        // Spawned with `swing == 0`, a creature carries no explicit `SwingSpeed`;
        // its pace is derived from dexterity through Sphere's formula — the
        // wrestling default here, since it has no stats set.
        let now = Instant::now();
        let mut world = world();
        enter(&mut world, now);
        let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);
        let mob_entity = entity(&world, mob);
        assert!(
            world.state.registry.get::<SwingSpeed>(mob_entity).is_none(),
            "zero on spawn pins nothing"
        );
        assert_eq!(
            combat::swing_speed(&world.state, mob_entity),
            WRESTLING_SWING_TICKS,
            "and the derived pace is the wrestling default"
        );
    }

    #[test]
    fn dexterity_quickens_the_swing() {
        // Sphere's era-1 formula: a nimbler mobile swings sooner. Raising
        // dexterity above the default shortens the interval `swing_speed` reports.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let player_entity = world.state.players[&player];
        let serial = serial_of(&world, player);

        let slow = combat::swing_speed(&world.state, player_entity);
        world.queue(Command::SetStats {
            serial,
            strength: DEFAULT_HITPOINTS,
            dexterity: 200,
            intelligence: DEFAULT_MANA,
        });
        world.tick(now);
        let fast = combat::swing_speed(&world.state, player_entity);

        assert_eq!(
            slow, WRESTLING_SWING_TICKS,
            "default dexterity, default pace"
        );
        assert!(fast < slow, "more dexterity swings sooner: {fast} < {slow}");
    }

    #[test]
    fn killing_the_target_ends_the_attack() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let player_entity = world.state.players[&player];
        // Eight hits, five a swing: dead on the second.
        let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 8, now);
        let mob_entity = entity(&world, mob);
        engage(&mut world, player, mob, now);

        for _ in 0..(2 * WRESTLING_SWING_TICKS) {
            world.tick(now);
        }
        assert!(
            !world.state.registry.contains(mob_entity),
            "the creature is dead and gone"
        );
        assert_eq!(
            world
                .state
                .registry
                .get::<Combat>(player_entity)
                .unwrap()
                .target,
            None,
            "and the attacker is no longer swinging at it"
        );
    }

    /// A mobile's value in a skill, in tenths.
    fn skill_value(world: &World, entity: EntityId, skill: u8) -> u16 {
        world
            .state
            .registry
            .get::<Skills>(entity)
            .map_or(0, |s| s.get(skill))
    }

    #[test]
    fn setting_a_skill_stores_it() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let entity = world.state.players[&player];
        let serial = serial_of(&world, player);

        world.queue(Command::SetSkill {
            serial,
            skill: 1,
            value: 755,
        });
        world.tick(now);
        assert_eq!(skill_value(&world, entity, 1), 755);
    }

    #[test]
    fn using_a_skill_announces_the_outcome() {
        // A grandmaster (100.0) at a trivial task always succeeds, and the event
        // carries the result for a script to reward.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let serial = serial_of(&world, player);
        world.queue(Command::SetSkill {
            serial,
            skill: 1,
            value: 1000,
        });
        world.tick(now);

        let mut used: Cursor<SkillUsed> = world.bus().cursor();
        world.queue(Command::UseSkill {
            serial,
            skill: 1,
            difficulty: 0,
        });
        world.tick(now);

        let events: Vec<SkillUsed> = world.bus().read(&mut used).copied().collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].skill, 1);
        assert!(events[0].success, "a sure thing succeeds");
    }

    #[test]
    fn a_skill_gains_from_use() {
        // From nothing, thirty percent a use — over fifty tries the value climbs.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let entity = world.state.players[&player];
        let serial = serial_of(&world, player);
        world.queue(Command::SetSkill {
            serial,
            skill: 1,
            value: 0,
        });
        world.tick(now);

        for _ in 0..50 {
            world.queue(Command::UseSkill {
                serial,
                skill: 1,
                difficulty: 0,
            });
            world.tick(now);
        }
        assert!(
            skill_value(&world, entity, 1) > 0,
            "practice taught something"
        );
    }

    #[test]
    fn a_capped_skill_does_not_gain() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let entity = world.state.players[&player];
        let serial = serial_of(&world, player);
        world.queue(Command::SetSkill {
            serial,
            skill: 1,
            value: skills::SKILL_CAP,
        });
        world.tick(now);

        for _ in 0..30 {
            world.queue(Command::UseSkill {
                serial,
                skill: 1,
                difficulty: 0,
            });
            world.tick(now);
        }
        assert_eq!(
            skill_value(&world, entity, 1),
            skills::SKILL_CAP,
            "there is nothing left to learn at the cap"
        );
    }

    #[test]
    fn skill_rolls_are_replayable() {
        // The whole reason the generator lives in the world: the same commands
        // from the same start reach the same skill, roll for roll.
        fn run() -> u16 {
            let now = Instant::now();
            let mut world = world();
            let connection = enter(&mut world, now);
            let serial = serial_of(&world, connection);
            let entity = world.state.players[&connection];
            world.queue(Command::SetSkill {
                serial,
                skill: 3,
                value: 400,
            });
            world.tick(now);
            for _ in 0..40 {
                world.queue(Command::UseSkill {
                    serial,
                    skill: 3,
                    difficulty: 40,
                });
                world.tick(now);
            }
            skill_value(&world, entity, 3)
        }
        assert_eq!(run(), run(), "two identical runs land on the same value");
    }

    #[test]
    fn casting_a_spell_pays_mana_and_announces_it() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let entity = world.state.players[&player];
        let serial = serial_of(&world, player);
        // Grandmaster mage, so the skill roll is a sure thing.
        world.queue(Command::SetSkill {
            serial,
            skill: 1,
            value: 1000,
        });
        world.tick(now);

        let mut cast: Cursor<SpellCast> = world.bus().cursor();
        world.queue(Command::CastSpell {
            serial,
            spell: 5,
            target: 0,
            mana: 10,
            difficulty: 0,
            skill: 1,
            pack: 0,
            reagents: Vec::new(),
        });
        world.tick(now);

        let events: Vec<SpellCast> = world.bus().read(&mut cast).copied().collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].spell, 5);
        assert!(events[0].success, "a mana-full grandmaster casts it");
        assert_eq!(
            world.state.registry.get::<Mana>(entity).unwrap().current,
            90,
            "ten mana is spent"
        );
    }

    #[test]
    fn reagents_are_consumed_on_a_cast_and_a_short_pack_fizzles() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let entity = world.state.players[&player];
        let serial = serial_of(&world, player);
        world.queue(Command::SetSkill {
            serial,
            skill: 1,
            value: 1000,
        });
        world.tick(now);

        // A pack with three of one reagent.
        const REAGENT: u16 = 0x0F7A;
        let pack = spawn_container_at(&mut world, Point::new(START.0, START.1, 0), now);
        let container = openshard_entities::Serial::new(pack).unwrap();
        for _ in 0..3 {
            let (item, _) = world
                .state
                .registry
                .spawn_with_serial(openshard_entities::SerialKind::Item)
                .unwrap();
            world.state.registry.insert(
                item,
                Graphic {
                    id: REAGENT,
                    hue: 0,
                },
            );
            world.state.registry.insert(
                item,
                Contained {
                    container,
                    x: 0,
                    y: 0,
                    grid: 0,
                },
            );
        }

        let spell = |reagents: Vec<(u16, u16)>| Command::CastSpell {
            serial,
            spell: 5,
            target: 0,
            mana: 10,
            difficulty: 0,
            skill: 1,
            pack,
            reagents,
        };
        let mut cast: Cursor<SpellCast> = world.bus().cursor();

        // First cast needs two; the pack has three, so it takes them and casts.
        world.queue(spell(vec![(REAGENT, 2)]));
        world.tick(now);
        let first: Vec<SpellCast> = world.bus().read(&mut cast).copied().collect();
        assert!(first[0].success, "the stocked pack lets it cast");
        assert_eq!(
            openshard_items::count_in_container(&world.state, container, REAGENT),
            1,
            "two of the three reagents were consumed"
        );

        // One left; a second cast needing two fizzles and spends nothing.
        let mana = world.state.registry.get::<Mana>(entity).unwrap().current;
        world.queue(spell(vec![(REAGENT, 2)]));
        world.tick(now);
        let second: Vec<SpellCast> = world.bus().read(&mut cast).copied().collect();
        assert!(!second[0].success, "one reagent left is not enough");
        assert_eq!(
            world.state.registry.get::<Mana>(entity).unwrap().current,
            mana,
            "a fizzle spends no mana"
        );
        assert_eq!(
            openshard_items::count_in_container(&world.state, container, REAGENT),
            1,
            "and consumes no reagent"
        );
    }

    #[test]
    fn consuming_a_reagent_redraws_an_open_pack() {
        // A pack the player has open updates live: a reagent burned out of it
        // vanishes from the gump, a `0x1D` pushed to the watcher.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let serial = serial_of(&world, player);
        world.queue(Command::SetSkill {
            serial,
            skill: 1,
            value: 1000,
        });
        world.tick(now);

        // A container on the player's tile, one reagent inside.
        const REAGENT: u16 = 0x0F7A;
        let pack = spawn_container_at(&mut world, Point::new(START.0, START.1, 0), now);
        let container = openshard_entities::Serial::new(pack).unwrap();
        let (_, item_serial) = world
            .state
            .registry
            .spawn_with_serial(openshard_entities::SerialKind::Item)
            .unwrap();
        let item = world.state.registry.entity_of(item_serial).unwrap();
        world.state.registry.insert(
            item,
            Graphic {
                id: REAGENT,
                hue: 0,
            },
        );
        world.state.registry.insert(
            item,
            Contained {
                container,
                x: 0,
                y: 0,
                grid: 0,
            },
        );

        // Open it, then clear what has been sent so far.
        world.queue(Command::DoubleClick {
            connection: player,
            serial: pack,
        });
        world.tick(now);
        let _ = packets_for(&mut world, player);

        // Cast, burning the reagent out of the open pack.
        world.queue(Command::CastSpell {
            serial,
            spell: 5,
            target: 0,
            mana: 10,
            difficulty: 0,
            skill: 1,
            pack,
            reagents: vec![(REAGENT, 1)],
        });
        world.tick(now);

        assert!(
            packets_for(&mut world, player)
                .iter()
                .any(|p| p == &encode_remove(item_serial.raw())),
            "the watcher is told the reagent left the pack"
        );
    }

    #[test]
    fn a_spell_beyond_the_mana_fizzles() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let entity = world.state.players[&player];
        let serial = serial_of(&world, player);

        let mut cast: Cursor<SpellCast> = world.bus().cursor();
        world.queue(Command::CastSpell {
            serial,
            spell: 1,
            target: 0,
            mana: 200, // more than the 100 on hand
            difficulty: 0,
            skill: 1,
            pack: 0,
            reagents: Vec::new(),
        });
        world.tick(now);

        let events: Vec<SpellCast> = world.bus().read(&mut cast).copied().collect();
        assert!(!events[0].success, "it fizzles");
        assert_eq!(
            world.state.registry.get::<Mana>(entity).unwrap().current,
            100,
            "and no mana is spent on a fizzle"
        );
    }

    #[test]
    fn healing_raises_hits_but_not_past_max() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let entity = world.state.players[&player];
        let serial = serial_of(&world, player);

        world.queue(Command::Damage {
            serial,
            amount: 60,
            damage_type: 0,
            by: 0,
        });
        world.tick(now); // 100 -> 40
        world.queue(Command::Heal {
            serial,
            amount: 1000,
        });
        world.tick(now);

        assert_eq!(
            world
                .state
                .registry
                .get::<Hitpoints>(entity)
                .unwrap()
                .current,
            100,
            "healed to the maximum, no further"
        );
    }

    #[test]
    fn mana_trickles_back() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let entity = world.state.players[&player];
        let serial = serial_of(&world, player);
        world.queue(Command::SetSkill {
            serial,
            skill: 1,
            value: 1000,
        });
        world.tick(now);
        world.queue(Command::CastSpell {
            serial,
            spell: 1,
            target: 0,
            mana: 20,
            difficulty: 0,
            skill: 1,
            pack: 0,
            reagents: Vec::new(),
        });
        world.tick(now);
        let spent = world.state.registry.get::<Mana>(entity).unwrap().current;

        for _ in 0..MANA_REGEN_TICKS {
            world.tick(now);
        }
        assert!(
            world.state.registry.get::<Mana>(entity).unwrap().current > spent,
            "mana came back over time"
        );
    }

    /// Spawn a creature with a brain (sight, wander) and return its serial.
    fn spawn_creature(
        world: &mut World,
        point: Point,
        sight: u8,
        wander: bool,
        now: Instant,
    ) -> u32 {
        world.queue(Command::SpawnMobile {
            body: 0x0190,
            hue: 0,
            hits: 50,
            notoriety: 5,
            damage: combat::SWING_DAMAGE,
            resistance: 0,
            swing: 0,
            sight,
            wander,
            position: point,
            facet: 0,
            name: None,
            banker: false,
            equipment: Vec::new(),
        });
        world.tick(now);
        world
            .state
            .registry
            .query::<Body>()
            .filter(|(entity, _)| !world.state.registry.has::<Client>(*entity))
            .filter_map(|(entity, _)| world.state.registry.serial_of(entity).map(|s| s.raw()))
            .max()
            .expect("a spawned creature")
    }

    #[test]
    fn an_aggressive_creature_attacks_a_nearby_player() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let player_entity = world.state.players[&player];
        // Aggressive, standing on the player's tile.
        spawn_creature(&mut world, Point::new(START.0, START.1, 0), 10, false, now);

        // A beat to notice, a swing interval to strike.
        for _ in 0..(AI_THINK_TICKS + WRESTLING_SWING_TICKS + 2) {
            world.tick(now);
        }
        assert!(
            world
                .state
                .registry
                .get::<Hitpoints>(player_entity)
                .unwrap()
                .current
                < DEFAULT_HITPOINTS,
            "the creature noticed the player and hit them"
        );
    }

    #[test]
    fn an_aggressive_creature_chases_a_player() {
        let now = Instant::now();
        let mut world = world();
        enter(&mut world, now); // a player at START to be chased
        let start = Point::new(START.0 + 4, START.1, 0);
        let mob = spawn_creature(&mut world, start, 10, false, now);
        let mob_entity = entity(&world, mob);

        // Several beats: it turns, then walks toward the player.
        for _ in 0..(5 * AI_THINK_TICKS) {
            world.tick(now);
        }
        assert!(
            world
                .state
                .registry
                .get::<Position>(mob_entity)
                .unwrap()
                .0
                .x
                < start.x,
            "the creature closed the distance"
        );
    }

    #[test]
    fn a_passive_creature_ignores_players() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let player_entity = world.state.players[&player];
        // Sight 0, no wander: no brain at all.
        spawn_creature(&mut world, Point::new(START.0, START.1, 0), 0, false, now);

        for _ in 0..(WRESTLING_SWING_TICKS + AI_THINK_TICKS + 5) {
            world.tick(now);
        }
        assert_eq!(
            world
                .state
                .registry
                .get::<Hitpoints>(player_entity)
                .unwrap()
                .current,
            DEFAULT_HITPOINTS,
            "a passive creature never lifts a finger"
        );
    }

    #[test]
    fn a_wandering_creature_drifts() {
        let now = Instant::now();
        let mut world = world();
        let start = Point::new(START.0, START.1, 0);
        // Wanders, sees nothing to fight.
        let mob = spawn_creature(&mut world, start, 0, true, now);
        let mob_entity = entity(&world, mob);

        for _ in 0..(15 * AI_THINK_TICKS) {
            world.tick(now);
        }
        assert_ne!(
            world.state.registry.get::<Position>(mob_entity).unwrap().0,
            start,
            "given time, a wanderer moves"
        );
    }

    #[test]
    fn stats_recap_hits_and_mana() {
        // Strength caps hit points, intelligence mana; lowering a stat below the
        // current value drags it down.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let entity = world.state.players[&player];
        let serial = serial_of(&world, player);

        world.queue(Command::SetStats {
            serial,
            strength: 60,
            dexterity: 80,
            intelligence: 40,
        });
        world.tick(now);

        let hp = world.state.registry.get::<Hitpoints>(entity).unwrap();
        assert_eq!((hp.current, hp.max), (60, 60), "hits follow strength");
        let mana = world.state.registry.get::<Mana>(entity).unwrap();
        assert_eq!(
            (mana.current, mana.max),
            (40, 40),
            "mana follows intelligence"
        );
        assert_eq!(
            world.state.registry.get::<Stats>(entity).unwrap().dexterity,
            80,
            "and dexterity is stored for what will derive from it"
        );
    }

    #[test]
    fn speech_reaches_nearby_players_and_the_speaker() {
        let now = Instant::now();
        let mut world = world();
        let speaker = enter(&mut world, now);
        let listener = enter_as(&mut world, ConnectionId::from_raw(2), now);
        let _ = packets_for(&mut world, speaker);
        let _ = packets_for(&mut world, listener);

        world.queue(Command::Say {
            connection: speaker,
            mode: 0,
            hue: 0x0384,
            font: 3,
            text: "hail".to_owned(),
        });
        world.tick(now);

        // Drain once — both players' packets came out of the same tick.
        let all: Vec<Outbound> = world.drain_outbound().collect();
        assert!(
            all.iter()
                .any(|o| o.connection == speaker && o.packet[0] == 0x1C),
            "the speaker sees their own words"
        );
        assert!(
            all.iter()
                .any(|o| o.connection == listener && o.packet[0] == 0x1C),
            "and so does the player beside them"
        );
    }

    #[test]
    fn speech_does_not_carry_out_of_earshot() {
        let now = Instant::now();
        let mut world = world();
        let speaker = enter(&mut world, now);
        let listener = enter_as(&mut world, ConnectionId::from_raw(2), now);
        // Move the listener well past speech range.
        teleport(&mut world, listener, Point::new(START.0 + 40, START.1, 0));
        let _ = packets_for(&mut world, listener);

        world.queue(Command::Say {
            connection: speaker,
            mode: 0,
            hue: 0,
            font: 3,
            text: "hail".to_owned(),
        });
        world.tick(now);

        assert!(
            !packets_for(&mut world, listener)
                .iter()
                .any(|p| p[0] == 0x1C),
            "a shout across a field is not heard"
        );
    }

    #[test]
    fn a_whisper_carries_only_to_those_right_beside() {
        // Ten tiles is within normal earshot but far past a whisper's three, so
        // the same listener who would hear a word spoken hears nothing whispered.
        let now = Instant::now();
        let mut world = world();
        let speaker = enter(&mut world, now);
        let listener = enter_as(&mut world, ConnectionId::from_raw(2), now);
        teleport(&mut world, listener, Point::new(START.0 + 10, START.1, 0));
        let _ = packets_for(&mut world, listener);

        world.queue(Command::Say {
            connection: speaker,
            mode: TALKMODE_WHISPER,
            hue: 0,
            font: 3,
            text: "psst".to_owned(),
        });
        world.tick(now);

        assert!(
            !packets_for(&mut world, listener)
                .iter()
                .any(|p| p[0] == 0x1C),
            "a whisper does not reach ten tiles off"
        );
    }

    #[test]
    fn a_yell_carries_past_normal_earshot() {
        // Twenty-five tiles is beyond the normal eighteen but inside a yell's
        // thirty-one, so only shouting reaches this listener.
        let now = Instant::now();
        let mut world = world();
        let speaker = enter(&mut world, now);
        let listener = enter_as(&mut world, ConnectionId::from_raw(2), now);
        teleport(&mut world, listener, Point::new(START.0 + 25, START.1, 0));
        let _ = packets_for(&mut world, listener);

        // Said normally, it does not reach.
        world.queue(Command::Say {
            connection: speaker,
            mode: 0,
            hue: 0,
            font: 3,
            text: "here".to_owned(),
        });
        world.tick(now);
        assert!(
            !packets_for(&mut world, listener)
                .iter()
                .any(|p| p[0] == 0x1C),
            "normal speech stops short of twenty-five tiles"
        );

        // Yelled, it does.
        world.queue(Command::Say {
            connection: speaker,
            mode: TALKMODE_YELL,
            hue: 0,
            font: 3,
            text: "here".to_owned(),
        });
        world.tick(now);
        assert!(
            packets_for(&mut world, listener)
                .iter()
                .any(|p| p[0] == 0x1C),
            "but a yell carries that far"
        );
    }

    #[test]
    fn accented_speech_goes_out_as_unicode() {
        // A Brazilian player types "olá": Latin-1 `0x1C` would lose the accent, so
        // the world reaches for Unicode `0xAE` instead. Pure-ASCII speech (the
        // test above) stays on `0x1C`, universally understood.
        let now = Instant::now();
        let mut world = world();
        let speaker = enter(&mut world, now);

        world.queue(Command::Say {
            connection: speaker,
            mode: 0,
            hue: 0,
            font: 3,
            text: "olá".to_owned(),
        });
        world.tick(now);

        let packets = packets_for(&mut world, speaker);
        assert!(
            packets.iter().any(|p| p[0] == 0xAE),
            "accented speech takes the Unicode path"
        );
        assert!(
            !packets.iter().any(|p| p[0] == 0x1C),
            "and not the ASCII one, which would mangle it"
        );
    }

    #[test]
    fn speaking_puts_the_words_on_the_bus() {
        let now = Instant::now();
        let mut world = world();
        let speaker = enter(&mut world, now);
        let mut spoke: Cursor<MobileSpoke> = world.bus().cursor();

        world.queue(Command::Say {
            connection: speaker,
            mode: 0,
            hue: 0,
            font: 3,
            text: "hello world".to_owned(),
        });
        world.tick(now);

        let events: Vec<MobileSpoke> = world.bus().read(&mut spoke).cloned().collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].text, "hello world");
    }

    fn gm_say(world: &mut World, connection: ConnectionId, text: &str, now: Instant) {
        world.queue(Command::Say {
            connection,
            mode: 0,
            hue: 0,
            font: 3,
            text: text.to_owned(),
        });
        world.tick(now);
    }

    #[test]
    fn a_gm_dot_command_is_run_not_spoken() {
        // `.where` from a game master answers privately and is never put over
        // their head — a command is not speech.
        let now = Instant::now();
        let mut world = world();
        let gm = enter_gm(&mut world, now);
        let _ = packets_for(&mut world, gm);
        let mut spoke: Cursor<MobileSpoke> = world.bus().cursor();

        gm_say(&mut world, gm, ".where", now);

        assert_eq!(
            world.bus().read(&mut spoke).count(),
            0,
            "no one heard a command"
        );
        assert!(
            packets_for(&mut world, gm).iter().any(|p| p[0] == 0x1C),
            "the GM got a private system answer"
        );
    }

    #[test]
    fn a_players_dot_text_is_ordinary_speech() {
        // A non-GM saying ".hello" just talks: no command, no privilege leak, and
        // the words go on the bus like any other speech.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let mut spoke: Cursor<MobileSpoke> = world.bus().cursor();

        gm_say(&mut world, player, ".hello", now);

        let events: Vec<MobileSpoke> = world.bus().read(&mut spoke).cloned().collect();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].text, ".hello",
            "a player's dot-text is spoken verbatim"
        );
    }

    #[test]
    fn dot_save_forces_a_snapshot_and_tells_everyone() {
        // A staff `.save` writes now, without pausing, even with the periodic save
        // turned off — and every player is told it happened.
        let mut world = World::new(START).with_save_every(0);
        let now = Instant::now();
        let gm = enter_gm(&mut world, now);
        let _ = world.drain_saves().count();
        let _ = packets_for(&mut world, gm);

        gm_say(&mut world, gm, ".save", now);

        assert!(
            world.drain_saves().next().is_some(),
            "the save was forced despite the cadence being off"
        );
        assert!(
            packets_for(&mut world, gm)
                .iter()
                .any(|p| { p[0] == 0x1C && String::from_utf8_lossy(p).contains("being saved") }),
            "players were told the world is being saved"
        );
    }

    #[test]
    fn a_gm_can_teleport_add_and_set() {
        let now = Instant::now();
        let mut world = world();
        let gm = enter_gm(&mut world, now);
        let entity = world.state.players[&gm];

        // Teleport by coordinates — Sphere's `.go`.
        gm_say(
            &mut world,
            gm,
            &format!(".go {} {}", START.0 + 5, START.1 + 7),
            now,
        );
        let Position(at) = *world.registry().get::<Position>(entity).unwrap();
        assert_eq!((at.x, at.y), (START.0 + 5, START.1 + 7), "the GM moved");

        // Add an item at the GM's feet — the GM's own screen is drawn the 0x1A.
        let _ = packets_for(&mut world, gm);
        gm_say(&mut world, gm, ".add 0x0eed 5", now);
        assert!(
            packets_for(&mut world, gm).iter().any(|p| p[0] == 0x1A),
            "the spawned item was drawn"
        );

        // Set a stat, through the skills system that owns the cap.
        gm_say(&mut world, gm, ".set str 73", now);
        assert_eq!(world.registry().get::<Stats>(entity).unwrap().strength, 73);
    }

    fn admin_response(connection: ConnectionId, button: u32) -> Command {
        Command::GumpResponse {
            connection,
            response: openshard_protocol::GumpResponse {
                serial: 0,
                gump_id: crate::admin::ADMIN_GUMP,
                button,
                switches: Vec::new(),
                text_entries: Vec::new(),
            },
        }
    }

    #[test]
    fn tele_raises_a_cursor_and_the_click_teleports() {
        let now = Instant::now();
        let mut world = world();
        let gm = enter_gm(&mut world, now);
        let entity = world.state.players[&gm];
        let _ = packets_for(&mut world, gm);

        // `.tele` raises a targeting cursor and does not move the GM yet.
        gm_say(&mut world, gm, ".tele", now);
        assert!(
            packets_for(&mut world, gm).iter().any(|p| p[0] == 0x6C),
            "a targeting cursor is sent"
        );
        let before = *world.registry().get::<Position>(entity).unwrap();
        assert_eq!(
            before.0.x, START.0,
            "the GM has not moved on raising the cursor"
        );

        // The click comes back as a 0x6C response; the GM jumps to the spot.
        let target = Point::new(START.0 + 9, START.1 + 3, before.0.z);
        world.queue(Command::TargetResponse {
            connection: gm,
            response: openshard_protocol::TargetResponse {
                cursor_id: 0,
                serial: 0,
                location: target,
                graphic: 0,
                cancelled: false,
            },
        });
        world.tick(now);
        let Position(at) = *world.registry().get::<Position>(entity).unwrap();
        assert_eq!(
            (at.x, at.y),
            (target.x, target.y),
            "the click teleported the GM"
        );
    }

    #[test]
    fn a_cancelled_tele_does_not_move() {
        let now = Instant::now();
        let mut world = world();
        let gm = enter_gm(&mut world, now);
        let entity = world.state.players[&gm];

        gm_say(&mut world, gm, ".tele", now);
        let before = *world.registry().get::<Position>(entity).unwrap();
        world.queue(Command::TargetResponse {
            connection: gm,
            response: openshard_protocol::TargetResponse {
                cursor_id: 0,
                serial: 0,
                location: Point::new(START.0 + 9, START.1 + 3, before.0.z),
                graphic: 0,
                cancelled: true,
            },
        });
        world.tick(now);
        let after = *world.registry().get::<Position>(entity).unwrap();
        assert_eq!(before.0, after.0, "a right-clicked cursor moves nobody");
    }

    #[test]
    fn admin_opens_a_gump_for_a_game_master() {
        let now = Instant::now();
        let mut world = world();
        let gm = enter_gm(&mut world, now);
        let _ = packets_for(&mut world, gm);

        gm_say(&mut world, gm, ".admin", now);

        assert!(
            packets_for(&mut world, gm).iter().any(|p| p[0] == 0xB0),
            "the admin gump is sent"
        );
    }

    #[test]
    fn an_admin_button_from_a_game_master_is_answered() {
        let now = Instant::now();
        let mut world = world();
        let gm = enter_gm(&mut world, now);
        let _ = packets_for(&mut world, gm);

        world.queue(admin_response(gm, 10)); // Populate Britain
        world.tick(now);

        assert!(
            packets_for(&mut world, gm).iter().any(|p| p[0] == 0x1C),
            "the button is acted on"
        );
    }

    #[test]
    fn decorate_places_statics_and_clear_removes_them() {
        use openshard_state::components::Decoration;
        let now = Instant::now();
        let mut world = world();
        let _gm = enter_gm(&mut world, now);

        world.queue(Command::Decorate {
            facet: 0,
            statics: vec![
                (0x07C1, 0, Point::new(START.0 + 1, START.1, 0)),
                (0x08DA, 0, Point::new(START.0 + 2, START.1, 0)),
            ],
            doors: Vec::new(),
            containers: Vec::new(),
        });
        world.tick(now);
        assert_eq!(
            world.registry().query::<Decoration>().count(),
            2,
            "both decorations were placed"
        );
        // Decoration never decays.
        let decor = world.registry().query::<Decoration>().next().unwrap().0;
        assert!(
            !world.registry().has::<Decays>(decor),
            "decoration does not rot"
        );

        world.queue(Command::ClearDecorations);
        world.tick(now);
        assert_eq!(
            world.registry().query::<Decoration>().count(),
            0,
            "clear removed the decoration"
        );
    }

    #[test]
    fn decoration_cannot_be_picked_up() {
        use openshard_state::components::Decoration;
        let now = Instant::now();
        let mut world = world();
        let gm = enter_gm(&mut world, now);
        world.queue(Command::Decorate {
            facet: 0,
            statics: vec![(0x07C1, 0, Point::new(START.0, START.1, 0))],
            doors: Vec::new(),
            containers: Vec::new(),
        });
        world.tick(now);
        let decor = world.registry().query::<Decoration>().next().unwrap().0;
        let serial = world.registry().serial_of(decor).unwrap().raw();
        let _ = packets_for(&mut world, gm);

        world.queue(Command::PickUpItem {
            connection: gm,
            serial,
            amount: 1,
        });
        world.tick(now);

        assert!(
            !world.state.held.contains_key(&gm),
            "a town's fittings are not loot"
        );
        assert!(
            packets_for(&mut world, gm).iter().any(|p| p[0] == 0x27),
            "the lift is refused with a drag-cancel"
        );
    }

    #[test]
    fn a_door_opens_and_closes_on_double_click() {
        let now = Instant::now();
        let mut world = world();
        let gm = enter_gm(&mut world, now);
        // A metal door one tile from the GM, well within reach.
        let at = Point::new(START.0 + 1, START.1, 0);
        world.queue(Command::Decorate {
            facet: 0,
            statics: Vec::new(),
            doors: vec![DecorDoor {
                closed: 0x0675,
                open: 0x0676,
                offset_x: -1,
                offset_y: 1,
                position: at,
            }],
            containers: Vec::new(),
        });
        world.tick(now);
        let door = world.registry().query::<Door>().next().unwrap().0;
        let serial = world.registry().serial_of(door).unwrap().raw();

        // Double-click opens it: the graphic becomes the open art and it hops by
        // the hinge offset.
        world.queue(Command::DoubleClick {
            connection: gm,
            serial,
        });
        world.tick(now);
        assert_eq!(
            world.registry().get::<Graphic>(door).unwrap().id,
            0x0676,
            "the door drew open"
        );
        assert_eq!(
            world.registry().get::<Position>(door).unwrap().0,
            Point::new(START.0, START.1 + 1, 0),
            "it swung aside by its hinge offset"
        );
        assert!(world.registry().get::<Door>(door).unwrap().is_open);

        // Double-clicking again shuts it and returns it to its frame.
        world.queue(Command::DoubleClick {
            connection: gm,
            serial,
        });
        world.tick(now);
        assert_eq!(world.registry().get::<Graphic>(door).unwrap().id, 0x0675);
        assert_eq!(world.registry().get::<Position>(door).unwrap().0, at);
        assert!(!world.registry().get::<Door>(door).unwrap().is_open);
    }

    #[test]
    fn an_open_door_swings_shut_on_its_own() {
        let now = Instant::now();
        let mut world = world();
        let gm = enter_gm(&mut world, now);
        let at = Point::new(START.0 + 1, START.1, 0);
        world.queue(Command::Decorate {
            facet: 0,
            statics: Vec::new(),
            doors: vec![DecorDoor {
                closed: 0x0675,
                open: 0x0676,
                offset_x: -1,
                offset_y: 1,
                position: at,
            }],
            containers: Vec::new(),
        });
        world.tick(now);
        let door = world.registry().query::<Door>().next().unwrap().0;
        let serial = world.registry().serial_of(door).unwrap().raw();

        world.queue(Command::DoubleClick {
            connection: gm,
            serial,
        });
        world.tick(now);
        assert!(world.registry().get::<Door>(door).unwrap().is_open);

        // Run past the auto-close delay: the door closes itself, untouched.
        let close_at = world.registry().get::<Door>(door).unwrap().close_at;
        while world.state.ticks < close_at {
            world.tick(now);
        }
        assert!(
            !world.registry().get::<Door>(door).unwrap().is_open,
            "the door swung shut on its own"
        );
        assert_eq!(world.registry().get::<Position>(door).unwrap().0, at);
    }

    /// A terrain whose only statics are one west door frame at (100, 100) and one
    /// east frame at (102, 100) — a single-door gap for the generator to fill. The
    /// gap has a surface (a door fits) unless `walled`, which stands in for a solid
    /// wall where nothing fits.
    struct FrameTerrain {
        walled: bool,
    }
    impl Terrain for FrameTerrain {
        fn can_step(&self, _from: Point, to: Point) -> Option<Point> {
            Some(to)
        }
        fn statics_at(&self, x: u16, y: u16, out: &mut Vec<(u16, i8)>) {
            if y == 100 && (x == 100 || x == 102) {
                out.push((0x0007, 0)); // 0x0007 is both a west and an east frame
            }
        }
        fn can_fit(&self, x: u16, y: u16, _z: i32, _height: i32) -> bool {
            !(self.walled && (x, y) == (101, 100))
        }
    }

    fn generate_britain_doors(world: &mut World, now: Instant) {
        world.queue(Command::GenerateDoors {
            facet: 0,
            x: 100,
            y: 100,
            width: 3,
            height: 1,
        });
        world.tick(now);
    }

    #[test]
    fn doors_are_generated_between_static_frames() {
        let now = Instant::now();
        let mut world = world();
        world.state.facet_state_mut(0).terrain = Some(Box::new(FrameTerrain { walled: false }));

        generate_britain_doors(&mut world, now);

        let (entity, door) = world
            .registry()
            .query::<Door>()
            .next()
            .expect("a door was generated");
        assert_eq!(
            world.registry().get::<Position>(entity).unwrap().0,
            Point::new(101, 100, 0),
            "the door fills the gap between the frames"
        );
        // A DarkWoodDoor, WestCW: closed 0x06A5, open 0x06A6, hinge (-1, 1).
        assert_eq!(door.closed, 0x06A5);
        assert_eq!(door.open, 0x06A6);
        assert_eq!((door.offset_x, door.offset_y), (-1, 1));
        assert!(
            world.registry().has::<Decoration>(entity),
            "a generated door is decoration"
        );

        // Running the pass again puts no second door on the same gap.
        generate_britain_doors(&mut world, now);
        assert_eq!(
            world.registry().query::<Door>().count(),
            1,
            "a tile that already has a door is not doored again"
        );
    }

    #[test]
    fn no_door_is_generated_into_a_wall() {
        let now = Instant::now();
        let mut world = world();
        world.state.facet_state_mut(0).terrain = Some(Box::new(FrameTerrain { walled: true }));

        generate_britain_doors(&mut world, now);

        assert_eq!(
            world.registry().query::<Door>().count(),
            0,
            "an obstructed gap is a wall, not a doorway"
        );
    }

    #[test]
    fn a_decoration_container_opens_on_double_click() {
        let now = Instant::now();
        let mut world = world();
        let gm = enter_gm(&mut world, now);
        world.queue(Command::Decorate {
            facet: 0,
            statics: Vec::new(),
            doors: Vec::new(),
            containers: vec![DecorContainer {
                graphic: 0x0E42,
                gump: 0x49,
                hue: 0,
                position: Point::new(START.0 + 1, START.1, 0),
            }],
        });
        world.tick(now);
        // The one container that is decoration — the GM also wears a backpack,
        // which is a container too.
        let chest = world
            .registry()
            .query::<Container>()
            .map(|(entity, _)| entity)
            .find(|&entity| world.registry().has::<Decoration>(entity))
            .expect("a decoration container is on the ground");
        let serial = world.registry().serial_of(chest).unwrap().raw();
        let _ = packets_for(&mut world, gm);

        world.queue(Command::DoubleClick {
            connection: gm,
            serial,
        });
        world.tick(now);
        assert!(
            packets_for(&mut world, gm).iter().any(|p| p[0] == 0x24),
            "the container gump opened"
        );
    }

    #[test]
    fn the_deco_button_emits_the_pack_verb() {
        let now = Instant::now();
        let mut world = world();
        let gm = enter_gm(&mut world, now);
        let mut actions: Cursor<AdminMenuAction> = world.bus().cursor();

        world.queue(admin_response(gm, 20)); // Decorate Britain
        world.tick(now);

        let events: Vec<AdminMenuAction> = world.bus().read(&mut actions).cloned().collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "decorate:britain");
    }

    #[test]
    fn the_populate_button_emits_an_admin_action_for_the_pack() {
        // The engine holds no spawn data now: the button emits a verb the script
        // pack acts on. Here we assert the verb reaches the bus; the pack turning
        // it into spawners is a scripting test.
        let now = Instant::now();
        let mut world = world();
        let gm = enter_gm(&mut world, now);
        let mut actions: Cursor<AdminMenuAction> = world.bus().cursor();

        world.queue(admin_response(gm, 10)); // Populate Britain
        world.tick(now);

        let events: Vec<AdminMenuAction> = world.bus().read(&mut actions).cloned().collect();
        assert_eq!(events.len(), 1, "one admin action was emitted");
        assert_eq!(events[0].action, "populate:britain");
    }

    #[test]
    fn an_admin_button_from_a_non_staff_client_is_ignored() {
        // The gump id is not a secret, so a plain player could forge a 0xB1 for
        // it. The gate must be on the response, not only the .admin that opened it.
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now); // ordinary Player access
        let _ = packets_for(&mut world, player);

        world.queue(admin_response(player, 12)); // Clear
        world.tick(now);

        assert!(
            !packets_for(&mut world, player).iter().any(|p| p[0] == 0x1C),
            "a non-staff forged response does nothing"
        );
    }

    #[test]
    fn a_spawner_fills_to_its_ceiling_and_clear_empties_it() {
        use crate::spawner::{CreatureTemplate, SpawnArea, Spawner};
        let now = Instant::now();
        let mut world = world();
        let creature = CreatureTemplate {
            body: 0x0009,
            hue: 0,
            hits: 10,
            notoriety: 3,
            damage: 0,
            resistance: 0,
            swing: 0,
            sight: 0,
            wander: false,
        };
        let area = SpawnArea {
            x: START.0,
            y: START.1,
            width: 3,
            height: 3,
            facet: 0,
        };
        world.queue(Command::RegisterSpawner {
            spawner: Spawner::new(0, area, vec![creature], 3, 0),
        });

        // One creature per region per pass, so a few ticks fill it to the ceiling
        // and no further.
        for _ in 0..6 {
            world.tick(now);
        }
        assert_eq!(
            world.registry().query::<SpawnedBy>().count(),
            3,
            "the region filled to its ceiling and stopped"
        );

        world.queue(Command::ClearSpawners);
        world.tick(now);
        assert_eq!(
            world.registry().query::<SpawnedBy>().count(),
            0,
            "clear removed the region and its creatures"
        );
    }

    #[test]
    fn a_creature_can_be_made_to_speak() {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);
        let _ = packets_for(&mut world, player);

        world.queue(Command::Speak {
            serial: mob,
            hue: 0,
            text: "grrr".to_owned(),
        });
        world.tick(now);

        assert!(
            packets_for(&mut world, player)
                .iter()
                .any(|p| p[0] == 0x1C && mentions(p, mob)),
            "the player hears the creature the script gave a voice"
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
            access: AccessLevel::Player,
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
            vec![0x1B, 0xBF, 0x20, 0x4F, 0x11, 0x78, 0x55],
            "0x1B first or there is no body; 0x55 last or the client draws early; \
             0x11 status and the 0x78 of the player's own equipment before it, or the \
             client has no stamina and no backpack serial to open"
        );
    }

    #[test]
    fn entering_sends_a_status_with_running_stamina() {
        // The fix for "cannot run": the client reads stamina from the 0x11, and a
        // zero there means walk-only. This is the byte that lets a player run.
        let mut world = world();
        enter(&mut world, Instant::now());

        let status = world
            .drain_outbound()
            .map(|out| out.packet)
            .find(|p| p[0] == 0x11)
            .expect("a status packet on world entry");
        let stamina = u16::from_be_bytes([status[50], status[51]]);
        assert!(
            stamina > 0,
            "stamina is zero; the client will refuse to run"
        );
    }

    #[test]
    fn a_status_request_is_answered_with_a_status() {
        // Opening the paperdoll (0x34) after entry resends the status.
        let mut world = world();
        let connection = enter(&mut world, Instant::now());
        let _ = world.drain_outbound().count();

        world.queue(Command::RequestStatus { connection });
        world.tick(Instant::now());

        assert!(
            packets_for(&mut world, connection)
                .iter()
                .any(|p| p[0] == 0x11),
            "a 0x34 should be answered with a 0x11"
        );
    }

    #[test]
    fn entering_builds_an_entity_out_of_components() {
        let mut world = world();
        enter(&mut world, Instant::now());

        let entity = *world.state.players.values().next().unwrap();
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
            access: AccessLevel::Player,
        });
        world.tick(Instant::now());

        let entity = world.state.players[&connection];
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
        let entity = world.state.players[&connection];
        let body = world.registry().get::<Body>(entity).copied().unwrap();
        assert_eq!(body.id, BODY_HUMAN_MALE);
        assert_eq!(body.hue, DEFAULT_HUE);
    }

    #[test]
    fn a_characters_inventory_survives_a_logout_and_restore() {
        use openshard_entities::SerialKind;

        // A character with something in its backpack logs out; a fresh shard loads
        // the saved items and the same character logs back in to find them.
        let mut home = world();
        let now = Instant::now();
        let conn_a = enter(&mut home, now);
        let entity = home.state.players[&conn_a];
        let char_serial = home.registry().serial_of(entity).unwrap().raw();

        // The backpack it was equipped on entry.
        let (backpack, _) = home
            .registry()
            .query::<Equipped>()
            .find(|(_, worn)| worn.layer == BACKPACK_LAYER)
            .expect("a backpack was equipped");
        let backpack_serial = home.registry().serial_of(backpack).unwrap();

        // A stack of gold inside it.
        let (gold, gold_serial) = home
            .state
            .registry
            .spawn_with_serial(SerialKind::Item)
            .unwrap();
        home.state
            .registry
            .insert(gold, Graphic { id: 0x0EED, hue: 0 });
        home.state.registry.insert(gold, Amount(500));
        home.state.registry.insert(gold, Stackable);
        home.state.registry.insert(
            gold,
            Contained {
                container: backpack_serial,
                x: 40,
                y: 65,
                grid: 0,
            },
        );

        // What persistence would carry: the backpack (worn) and the gold (inside).
        let records = home.inventory_of(entity);
        assert!(
            records
                .iter()
                .any(|r| r.serial == gold_serial.raw() && r.stackable),
            "the gold is saved as stackable"
        );
        assert!(
            records.iter().any(|r| r.serial == backpack_serial.raw()
                && matches!(r.location, ItemLocation::Equipped { .. })),
            "the backpack is saved as worn"
        );
        assert!(
            records.iter().any(|r| r.serial == gold_serial.raw()
                && r.amount == 500
                && matches!(r.location, ItemLocation::Contained { .. })),
            "the gold is saved inside, amount and all"
        );

        // Log out — the character and its items leave the world.
        home.queue(Command::Disconnect { connection: conn_a });
        home.tick(now);

        // A fresh shard: reserve the serials, load the items, play the character.
        let mut shard = world();
        shard.reserve_serial(char_serial);
        shard.restore_items(records);
        let conn_b = connection();
        shard.queue(Command::Enter {
            connection: conn_b,
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "Lord British".to_owned(),
            serial: Some(char_serial),
            position: Some(Point::new(1500, 1000, 0)),
            facet: 0,
            appearance: None,
            access: AccessLevel::Player,
        });
        shard.tick(now);

        // Exactly one backpack (the restored one, not a fresh starter too), with the
        // gold back inside it.
        let backpacks = shard
            .registry()
            .query::<Equipped>()
            .filter(|(_, worn)| worn.mobile.raw() == char_serial && worn.layer == BACKPACK_LAYER)
            .count();
        assert_eq!(
            backpacks, 1,
            "the saved backpack came back, no starter added"
        );
        let gold = shard
            .registry()
            .entity_of(gold_serial)
            .expect("the gold is back on its serial");
        assert_eq!(shard.registry().get::<Amount>(gold).unwrap().0, 500);
        assert!(
            shard.registry().has::<Stackable>(gold),
            "the gold came back stackable, so it still merges with more"
        );
        assert_eq!(
            shard.registry().get::<Contained>(gold).unwrap().container,
            backpack_serial,
            "and back inside the same backpack"
        );
    }

    #[test]
    fn a_relogin_in_the_same_run_keeps_the_inventory() {
        use openshard_entities::SerialKind;

        // The bug the user hit: logging out and back in *without a restart* lost the
        // backpack, because the pending-inventory cache was only filled at boot.
        let mut world = world();
        let now = Instant::now();
        let conn = enter(&mut world, now);
        let entity = world.state.players[&conn];
        let char_serial = world.registry().serial_of(entity).unwrap().raw();
        let (backpack, _) = world
            .registry()
            .query::<Equipped>()
            .find(|(_, w)| w.layer == BACKPACK_LAYER)
            .unwrap();
        let backpack_serial = world.registry().serial_of(backpack).unwrap();
        let (gold, gold_serial) = world
            .state
            .registry
            .spawn_with_serial(SerialKind::Item)
            .unwrap();
        world
            .state
            .registry
            .insert(gold, Graphic { id: 0x0EED, hue: 0 });
        world.state.registry.insert(gold, Amount(300));
        world.state.registry.insert(
            gold,
            Contained {
                container: backpack_serial,
                x: 0,
                y: 0,
                grid: 0,
            },
        );

        // Log out and, in the same world, log the same character back in.
        world.queue(Command::Disconnect { connection: conn });
        world.tick(now);
        let conn = connection();
        world.queue(Command::Enter {
            connection: conn,
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "Lord British".to_owned(),
            serial: Some(char_serial),
            position: Some(Point::new(1500, 1000, 0)),
            facet: 0,
            appearance: None,
            access: AccessLevel::Player,
        });
        world.tick(now);

        let gold = world
            .registry()
            .entity_of(gold_serial)
            .expect("the gold came back on relog");
        assert_eq!(world.registry().get::<Amount>(gold).unwrap().0, 300);
    }

    #[test]
    fn a_spawner_respawn_timer_survives_a_restart() {
        use crate::spawner::{SpawnArea, Spawner};

        // The user's case: a rare spawn on a long timer, killed with time still to
        // wait, must come back with that wait ahead of it — not pop again the moment
        // the shard restarts.
        let mut home = world();
        let area = SpawnArea {
            x: START.0,
            y: START.1,
            width: 1,
            height: 1,
            facet: 0,
        };
        // A 100-second respawn region.
        home.register_spawner(Spawner::new(0, area, vec![], 1, 100 * TICKS_PER_SECOND));
        // Pretend it spawned a while ago and has 60 seconds left to wait.
        home.state.ticks = 5_000;
        home.spawners[0].next_spawn = home.state.ticks + 60 * TICKS_PER_SECOND;

        // What the save carries.
        let records = home.spawner_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].remaining_secs, 60, "sixty seconds still to wait");
        assert_eq!(records[0].respawn_secs, 100);
        assert!(records[0].id > 0, "it was given a real id on registration");

        // Restart: a fresh world, tick counter back at zero, restores the region.
        let mut shard = world();
        shard.restore_spawners(records);
        assert_eq!(shard.spawners.len(), 1);
        assert_eq!(
            shard.spawners[0].next_spawn,
            60 * TICKS_PER_SECOND,
            "the sixty seconds are still ahead of it, not reset to zero"
        );
        assert_eq!(shard.spawners[0].respawn_delay, 100 * TICKS_PER_SECOND);
    }

    #[test]
    fn re_registering_a_region_replaces_it_rather_than_stacking() {
        use crate::spawner::{SpawnArea, Spawner};

        let mut world = world();
        let area = SpawnArea {
            x: 100,
            y: 100,
            width: 5,
            height: 5,
            facet: 0,
        };
        world.register_spawner(Spawner::new(0, area, vec![], 3, 40));
        world.register_spawner(Spawner::new(0, area, vec![], 3, 40));
        assert_eq!(
            world.spawners.len(),
            1,
            "the same region registered twice is one spawner, not two"
        );
    }

    #[test]
    fn a_snapshot_saves_an_idle_online_character_and_the_ground() {
        use openshard_entities::SerialKind;

        // A save must capture an online character's inventory and loose ground items
        // even when nobody moved — an item picked up without a step, gold dropped and
        // left. The old save only ran when the journal was dirty and only walked
        // dirty characters, which is how backpacks and dropped gold went missing.
        let mut world = world();
        let now = Instant::now();
        let conn = enter(&mut world, now);
        let entity = world.state.players[&conn];
        let (backpack, _) = world
            .registry()
            .query::<Equipped>()
            .find(|(_, w)| w.layer == BACKPACK_LAYER)
            .unwrap();
        let backpack_serial = world.registry().serial_of(backpack).unwrap();
        // A backpack item and a loose ground item.
        let (bagged, _) = world
            .state
            .registry
            .spawn_with_serial(SerialKind::Item)
            .unwrap();
        world
            .state
            .registry
            .insert(bagged, Graphic { id: 0x0EED, hue: 0 });
        world.state.registry.insert(
            bagged,
            Contained {
                container: backpack_serial,
                x: 0,
                y: 0,
                grid: 0,
            },
        );
        items::spawn_item(
            &mut world.state,
            0x1BFB,
            0,
            1,
            false,
            Point::new(1365, 1600, 0),
            0,
        );

        // Tick once to settle, draining any snapshots the enter produced, then force
        // a fresh snapshot with no movement in between.
        world.tick(now);
        let _ = world.drain_saves().count();
        world.take_snapshot();

        let snapshot = world.drain_saves().next().expect("a snapshot was taken");
        let owner = world.registry().serial_of(entity).unwrap().raw();
        assert!(
            snapshot.characters.iter().any(|c| c.serial == owner),
            "the idle online character was saved"
        );
        let inv = snapshot
            .inventories
            .iter()
            .find(|inv| inv.owner == owner)
            .expect("its inventory was walked");
        assert!(
            inv.items.iter().any(|i| i.graphic == 0x0EED),
            "the backpack gold is in the saved inventory"
        );
        let ground = snapshot.ground.as_ref().expect("the ground was swept");
        assert!(
            ground.iter().any(|i| i.graphic == 0x1BFB),
            "the loose ground item was saved"
        );
    }

    fn spawn_banker(world: &mut World, at: Point, now: Instant) {
        world.queue(Command::SpawnMobile {
            body: 0x0190,
            hue: 0,
            hits: 100,
            notoriety: 7, // invulnerable
            damage: 0,
            resistance: 0,
            swing: 0,
            sight: 0,
            wander: false,
            position: at,
            facet: 0,
            name: Some("the banker".to_owned()),
            banker: true,
            equipment: Vec::new(),
        });
        world.tick(now);
    }

    fn say(world: &mut World, connection: ConnectionId, text: &str, now: Instant) {
        world.queue(Command::Say {
            connection,
            mode: 0,
            hue: 0,
            font: 3,
            text: text.to_owned(),
        });
        world.tick(now);
    }

    #[test]
    fn entering_the_world_equips_a_bank_box() {
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        let owner = world
            .registry()
            .serial_of(world.state.players[&connection])
            .unwrap();
        assert!(
            world.registry().query::<Equipped>().any(|(item, worn)| {
                worn.mobile == owner
                    && worn.layer == npc::BANK_LAYER
                    && world.registry().has::<Container>(item)
            }),
            "a character wears a bank box on the bank layer"
        );
    }

    #[test]
    fn saying_bank_near_a_banker_opens_the_bank_box() {
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        spawn_banker(&mut world, Point::new(START.0 + 1, START.1, 0), now);
        let _ = packets_for(&mut world, connection);

        say(&mut world, connection, "bank", now);
        assert!(
            packets_for(&mut world, connection)
                .iter()
                .any(|p| p[0] == 0x24),
            "the bank box gump opened"
        );
    }

    #[test]
    fn a_banker_greets_a_nearby_player() {
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        // The banker two tiles off — inside the greet range. Its spawn tick also
        // runs the townsfolk beat, so it greets straight away. The line is one of
        // several, but every one names the visitor.
        spawn_banker(&mut world, Point::new(START.0 + 2, START.1, 0), now);
        let greeted = packets_for(&mut world, connection)
            .iter()
            .any(|p| p[0] == 0x1C && String::from_utf8_lossy(p).contains("Lord British"));
        assert!(greeted, "the banker greeted the nearby player by name");
    }

    #[test]
    fn single_clicking_a_named_mobile_draws_its_name() {
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        spawn_banker(&mut world, Point::new(START.0 + 1, START.1, 0), now);
        let banker = world
            .registry()
            .query::<Banker>()
            .next()
            .map(|(e, _)| e)
            .unwrap();
        let banker_serial = world.registry().serial_of(banker).unwrap().raw();
        let _ = packets_for(&mut world, connection);

        world.queue(Command::SingleClick {
            connection,
            serial: banker_serial,
        });
        world.tick(now);

        // A 0x1C label naming the banker, in the invulnerable (yellow) hue.
        let label = packets_for(&mut world, connection)
            .into_iter()
            .find(|p| p[0] == 0x1C)
            .expect("a name label was sent");
        // hue is at bytes 10..12 of a 0x1C.
        let hue = u16::from_be_bytes([label[10], label[11]]);
        assert_eq!(hue, 0x0035, "the banker's name is drawn yellow");
        assert!(
            String::from_utf8_lossy(&label).contains("the banker"),
            "the label carries the name"
        );
    }

    #[test]
    fn saying_bank_with_no_banker_near_does_nothing() {
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        // A banker, but far out of the 12-tile reach.
        spawn_banker(&mut world, Point::new(START.0 + 40, START.1, 0), now);
        let _ = packets_for(&mut world, connection);

        say(&mut world, connection, "bank", now);
        assert!(
            !packets_for(&mut world, connection)
                .iter()
                .any(|p| p[0] == 0x24),
            "no banker in reach, no bank box"
        );
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
            access: AccessLevel::Player,
        });
        world.tick(Instant::now());

        let entity = world.state.players[&connection];
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
        world.state.facets.insert(
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
            access: AccessLevel::Player,
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

        let a = world.state.players[&here];
        let b = world.state.players[&there];
        assert!(
            !world.state.seen[&a].contains(&b),
            "a mobile on facet 0 must not have drawn one on facet 1"
        );
        assert!(
            !world.state.seen[&b].contains(&a),
            "nor the other way round"
        );
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

        let a = world.state.players[&here];
        let b = world.state.players[&there];
        assert!(
            world.state.seen[&a].contains(&b),
            "same facet, same spot: they see"
        );
        assert!(world.state.seen[&b].contains(&a));
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

        let entity = world.state.players[&connection];
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
        let entity = world.state.players[&connection];
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
    fn a_departing_character_carries_where_it_walked_to() {
        // The re-login rewind bug: the world must hand the server the character's
        // *current* position on logout, so the server's cache tracks the move and
        // a re-login this run spawns it where it left — not where it logged in.
        let mut world = world();
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let entity = world.state.players[&connection];
        let start = world.registry().get::<Position>(entity).unwrap().0;
        let walked_to = Point::new(start.x + 9, start.y + 4, start.z);
        teleport(&mut world, connection, walked_to);

        world.queue(Command::Disconnect { connection });
        world.tick(now);

        let departed: Vec<_> = world.drain_departed().collect();
        assert_eq!(departed.len(), 1, "one character left");
        assert_eq!(
            (departed[0].x, departed[0].y),
            (walked_to.x, walked_to.y),
            "the logout record carries the moved position, not the login one"
        );
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
            access: AccessLevel::Player,
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
    fn an_empty_world_offers_nothing() {
        // No transaction just to say a shard is idle. With nobody online and
        // nothing loose on the ground, a save writes nothing and so is skipped.
        //
        // Note the deliberate change from earlier: an *online* character is now
        // saved every cadence whether or not it moved — picking an item up takes no
        // step, so the dirty set is not a safe basis for saving what someone holds.
        // That safety is worth a small, periodic write per online player; this test
        // guards the other side, that an empty shard still writes nothing.
        let mut world = eager();
        let now = Instant::now();
        for tick in 1..10 {
            world.tick(now + WALK_INTERVAL * tick);
        }
        assert_eq!(world.drain_saves().count(), 0);
    }

    #[test]
    fn an_online_character_is_saved_every_cadence_even_when_idle() {
        // The safety the change above buys: a character that logs in and stands
        // still is still written, so an item it picked up without moving is not lost
        // at the next restart.
        let mut world = eager();
        let now = Instant::now();
        enter(&mut world, now);
        let _ = world.drain_saves().count();
        world.tick(now + WALK_INTERVAL);
        assert!(
            world.drain_saves().next().is_some(),
            "an idle online character is still saved"
        );
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
        let entity = world.state.players[&connection];
        let serial = world.state.registry.serial_of(entity).expect("bound");

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
    use openshard_state::sectors::VIEW_RANGE;

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

        // Bob is drawn his own equipment in a 0x78 about himself now, so count
        // only the ones that are about Alice.
        let alice = world
            .registry()
            .serial_of(world.state.players[&ALICE])
            .unwrap()
            .raw()
            .to_be_bytes();
        let to_bob = packets_for(&mut world, BOB);
        let drawn = to_bob
            .iter()
            .filter(|p| p[0] == 0x78 && p.windows(4).any(|w| w == alice))
            .count();
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
        assert_eq!(world.state.seen.len(), 2);

        world.queue(Command::Disconnect { connection: BOB });
        world.tick(now);

        assert_eq!(world.state.seen.len(), 1, "Bob's screen outlived Bob");
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
        let entity = world.state.players[&alice];

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
