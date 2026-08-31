//! The world's runtime state: the data every system reads and writes.
//!
//! # Why this crate exists
//!
//! A gameplay system — combat, chat, skills — is a function over the world's
//! state: it reads components, rolls the world's generator, asks who is near a
//! point, and writes the result back. For those functions to live in their own
//! crates (`combat`, `chat`, …) rather than piling into one file, the state they
//! operate on has to sit *below* them in the dependency graph, in a crate they
//! can all depend on without depending on each other or on the tick that
//! sequences them.
//!
//! That is this crate. It owns the vocabulary of world state and nothing about
//! *when* it changes:
//!
//! - [`components`] — what a thing in the world is made of. Position, hit points,
//!   a combat stance, a skill map; a thing's identity is which of these it
//!   carries.
//! - [`Sectors`] — the spatial index that answers "what is near this point",
//!   Chebyshev distance, the square region a UO client draws.
//! - [`Regions`] — the named areas of a facet: which town or dungeon a point is
//!   in, and what holds there (guards, light, music).
//! - [`skill`] — what the fifty-eight skills are: their client ids, their names,
//!   and the per-skill numbers the check and the gain read.
//! - [`harvest`] — what the ground yields to a pick, an axe or a line, and the
//!   per-block banks that make a vein run dry. Data plus the depletion state that
//!   belongs to a patch of map rather than to any entity.
//! - [`weapon`], [`armor`], [`title`] — the same shape for gear and standing: data
//!   keyed by graphic (or by fame and karma) that more than one system above reads,
//!   so it sits below all of them. `combat` turns a weapon row into a swing and a
//!   blow; `skills` reads the same row to answer an Arms Lore question. The rules
//!   stay in the crate that owns them.
//! - [`Rng`] — the seeded generator behind every roll. Deterministic on purpose:
//!   advanced only by the tick, never the OS, so a world replays roll for roll.
//!
//! The tick that drives all this, and the systems that act on it, live above.

pub mod action_rules;
pub mod action_speeds;
pub mod action_stages;
pub mod armor;
pub mod boat;
pub mod components;
pub mod connection;
pub mod craft;
pub mod craft_stock;
pub mod dialogue;
pub mod facet_rules;
pub mod guild;
pub mod harvest;
pub mod house_inventory;
pub mod instrument;
pub mod item_definition;
pub mod item_location;
pub mod obstruct;
pub mod party;
pub mod quest;
pub mod region;
pub mod rng;
pub mod runtime;
pub mod sectors;
pub mod skill;
pub mod spawner;
pub mod tame;
pub mod title;
pub mod weapon;
pub mod world_tick;

pub use boat::{
    Boats,
    Plank,
};
pub use components::{
    Access,
    Account,
    Amount,
    Banker,
    BehaviourBuff,
    BehaviourBuffKind,
    BehaviourBuffs,
    Boat,
    Body,
    BodyType,
    Brain,
    Client,
    Combat,
    Contained,
    Container,
    CorpseBody,
    CriminalUntil,
    DEFAULT_SKILL_CAP,
    Decays,
    Decoration,
    Discorded,
    Door,
    Drawn,
    EMPTY_BOTTLE_GRAPHIC,
    Equipped,
    FIELD_HEIGHT,
    Fame,
    Field,
    FieldKind,
    Frozen,
    Ghost,
    Guard,
    GuildCandidate,
    GuildMember,
    Harvesting,
    Heading,
    HearsGhosts,
    Hidden,
    Hitpoints,
    House,
    HouseDeed,
    HouseDesign,
    HouseDoor,
    HouseSign,
    InRegion,
    Instrument,
    ItemAffix,
    ItemAffixes,
    ItemKind,
    ItemLocation,
    Karma,
    KeyValue,
    LastStatGain,
    Lock,
    LockKind,
    LockedDown,
    MOONGATE_GRAPHIC,
    MOONGATE_REACH,
    Mana,
    Material,
    Meditating,
    MeleeDamage,
    Moongate,
    Movement,
    MurderDecay,
    Murders,
    Name,
    NightHome,
    Npc,
    POISON_POTION_GRAPHIC,
    Pacified,
    PartyCandidate,
    PartyMember,
    PoisonCharges,
    Poisoned,
    Position,
    RECALL_RUNE_GRAPHIC,
    RUNEBOOK_ENTRIES,
    RUNEBOOK_GRAPHIC,
    Resistance,
    RuneMark,
    Runebook,
    RunebookEntry,
    Seated,
    SettledItemLocation,
    SkillCooldown,
    Skills,
    SpawnedBy,
    Stackable,
    Stamina,
    Standing,
    StatEffectKind,
    StatLock,
    StatLocks,
    StatMod,
    StatMods,
    Stats,
    Stealthing,
    SwingSpeed,
    Title,
    Tool,
    TradeWindow,
    Trap,
    TrapKind,
    WrestlingAmbushCooldown,
    WrestlingCombo,
    WrestlingInterceptCooldown,
    WrestlingOpener,
    WrestlingStride,
    effect,
    is_debuff,
    stat_shift,
};
pub use craft_stock::{
    CraftStockError,
    CraftStockPile,
};
pub use dialogue::{
    Dialogue,
    SpeechEntry,
    SpeechTable,
};
pub use guild::{
    Alliance,
    AllianceId,
    Alliances,
    Guild,
    GuildId,
    Guilds,
    Rank,
    Removal,
};
pub use house_inventory::{
    HOUSE_INVENTORY_REBUILD_BUDGET,
    HouseInventoryCursor,
    HouseInventoryError,
    HouseInventoryPage,
    HouseInventoryResult,
    HouseItemIdentity,
    MAX_HOUSE_INVENTORY_PAGE,
    MAX_HOUSE_INVENTORY_SELECTORS,
    house_item_identity,
};
pub use item_definition::{
    ItemDefinition,
    MaterialDefinition,
    has_tag,
    item_definition,
    kind_from_drawn,
    material_definition,
    material_from_legacy_hue,
    material_from_legacy_hue_in_family,
    presentation_of,
    selector_matches,
};
pub use item_location::{
    ContainedItems,
    ItemGraphViolation,
    LocationError,
    PreparedItemRelocation,
    audit_item_graph,
    commit_item_relocation,
    contained_items,
    despawn_item,
    equipped_items,
    establish_item_location,
    item_location,
    prepare_item_relocation,
    recursive_contained_items,
    relocate_item,
    settled_from_origin,
};
pub use obstruct::{
    DOOR_HEIGHT,
    Obstacle,
    Obstructions,
};
pub use openshard_protocol::world::{
    DamageType,
    RangedRange,
};
pub use party::{
    Parties,
    Party,
    PartyId,
};
pub use quest::{
    ObjectiveDef,
    ObjectiveKind,
    QuestDef,
    QuestDefs,
    QuestKey,
    RewardDef,
    RewardKind,
};
pub use region::{
    Region,
    RegionFlags,
    RegionId,
    RegionRect,
    Regions,
};
pub use rng::Rng;
pub use runtime::{
    Action,
    CastStyle,
    CraftGumpContext,
    CraftGumpPage,
    FacetState,
    FacetUndo,
    Gameplay,
    GuildGumpContext,
    GuildPage,
    HeldItem,
    HouseChange,
    HouseGumpContext,
    HouseGumpRow,
    HouseList,
    HouseStorage,
    Origin,
    Outbound,
    QuestGumpContext,
    QuestSection,
    TICKS_PER_SECOND,
    TargetPurpose,
    TooltipMode,
    Trade,
    TradeSide,
    WorldHome,
    WorldState,
};
pub use sectors::{
    Occupant,
    SECTOR_SIZE,
    Sectors,
    VIEW_RANGE,
    distance,
    in_range,
};
pub use skill::{
    SKILL_COUNT,
    SKILLS,
    Skill,
    SkillInfo,
    StatCode,
};
pub use spawner::SpawnerId;
pub use title::{
    award_fame,
    award_karma,
    award_message,
    compute_title,
    titled_name,
};
pub use world_tick::WorldTick;
