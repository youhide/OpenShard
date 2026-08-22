//! What a thing in the world is made of.
//!
//! # Small, plain, and owned by the rule that needs them
//!
//! Nothing here is a "GameObject". A player is an entity that happens to carry a
//! [`Body`], a [`Position`] and a [`Client`]; an NPC is the same minus the
//! `Client`; a rock is a `Position` and a `Drawn`. What a thing *is* falls out
//! of what it carries, which is the whole reason for an ECS.
//!
//! These are the ones the world itself needs to put a character on screen and
//! move it. Combat's components belong to combat, housing's to housing. A
//! `Components` grab-bag every crate imports from would be an inheritance tree
//! with extra steps.

use std::collections::HashMap;

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_movement::Walker;
use openshard_protocol::casting::SpellId;
use openshard_protocol::containers::GridSlot;
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::identity::AccountName;
use openshard_protocol::serial::Serial;
use openshard_protocol::skill::SkillLock;
use openshard_protocol::wire::{Graphic, Hue, Layer, SoundId};

use crate::skill::Skill;
pub use openshard_protocol::items::CORPSE_GRAPHIC;
pub use openshard_protocol::world::{Aggression, DamageType, PoisonLevel, RangedRange};
use openshard_protocol::world::{Facet, Point, Sight};
use openshard_protocol::{
    access::AccessLevel,
    direction::{Direction, Facing},
};

/// Where a mobile or item is.
///
/// Separate from [`Walker`] because most things that have a position never walk:
/// a tree, a corpse, a chest. Giving them a walk sequence and a pace budget
/// would be storage spent on a question nobody asks.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Position(pub Point);

/// Which way something faces.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Heading(pub Facing);

/// The graphic a mobile is drawn as.
///
/// UO calls this the "body". 0x0190 is a human male, 0x0191 a human female;
/// everything else is a creature.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Body {
    /// The body graphic id.
    pub id: Graphic,
    /// Its colour.
    pub hue: Hue,
}

/// The graphic an item is drawn as: its tiledata id and hue.
///
/// The item counterpart of [`Body`]. An entity carries one or the other — a
/// mobile a `Body`, a thing on the ground a `Drawn` — and that is what the
/// interest system reads to decide which packet draws it: `0x78` for a body,
/// `0x1A` for a graphic. Kept in `world` and not in a gameplay crate for the
/// same reason `Body` is: drawing a thing in the world is the world's job, and
/// the crate that owns item *rules* (stacking, decay, containment) builds on
/// this rather than the other way round.
///
/// Named `Drawn` and not `Graphic` because [`Graphic`] is the wire type this
/// component is *made of*. While both were called `Graphic` the collision cost
/// three spellings of one conversion across the server — a full path here, an
/// `as WireGraphic` import in four crates — and every one of them was a place a
/// reader had to work out which of the two was meant.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Drawn {
    /// The tiledata id.
    pub id: Graphic,
    /// Its colour, or [`Hue`]`(0)` for none.
    pub hue: Hue,
}

/// How many of a stackable item this entity is: a pile of 500 gold is one entity
/// with `Amount(500)`, not 500 entities.
///
/// Separate from [`Drawn`] because most items are single and storing a `1` on
/// every one of them is a column of ones. An item with no `Amount` is a single.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Amount(pub u16);

/// What a corpse marker (`0x2006`) is a picture of: which body, lying which way.
///
/// UO puts the body graphic in the same `0x1A` wire word as a stack size, but it
/// is not a quantity: the client uses it to choose the deceased body's animation.
/// Keeping it separate from [`Amount`] prevents corpse body ids from entering
/// stack, weight, and tooltip rules.
///
/// The facing is here rather than in the [`Corpse`] story because it belongs to
/// the *picture*: the client draws the last frame of that body's death group for
/// one direction, so a body and no direction is half a corpse. It is the dead
/// mobile's own heading, taken the moment it fell.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CorpseBody {
    /// The mobile graphic the corpse represents.
    pub body: Graphic,
    /// Which way it fell — the heading it died with.
    pub facing: Direction,
}

/// Marks an item as a container: something other items can be put inside.
///
/// The `gump` is the window the client draws when the container is opened — a
/// backpack, a wooden chest, a bank box each have their own. An item is a
/// container exactly when it carries this; nothing else changes about it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Container {
    /// The gump graphic the client opens for it.
    ///
    /// A [`Graphic`] and not a bare `u16` for the reason the type's own doc
    /// gives: gump art indexes the same `art.mul` as everything else the client
    /// draws, so a container's window art is the same kind of id as the item's.
    pub gump: Graphic,
}

/// Marks an item as being *inside* a container rather than on the ground.
///
/// An item carries either a [`Position`] (on the ground, in the sector grid and
/// on nearby screens) or a `Contained` (in a container, on nobody's ground) —
/// never both.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Contained {
    /// The container it is in, by serial.
    pub container: Serial,
    /// Where its icon sits inside the container's gump art.
    ///
    /// A [`GumpPoint`] and not a loose pair: these are gump pixels, not world
    /// tiles, and half a position is not a smaller one — it is an icon in the
    /// wrong place. The same type the packet built from this carries
    /// (`containers::ContainedItem::position`), so the two no longer disagree
    /// about what space they are in.
    pub position: GumpPoint,
    /// Its slot in the enhanced client's grid view.
    pub grid: GridSlot,
}

/// Marks an item as *worn* by a mobile, at a layer.
///
/// The third and last place an item can be, alongside [`Position`] (the ground)
/// and [`Contained`] (a container) — and exclusive with both. A layer holds at
/// most one item: a right hand has one weapon, a torso one shirt. Which layer an
/// item belongs on comes from its tiledata; the client proposes it and the
/// server checks the slot is free.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Equipped {
    /// The mobile wearing it.
    pub mobile: Serial,
    /// Which layer it sits on.
    ///
    /// The wire type, not a byte: a layer is the client's own numbering, this
    /// is the only component whose value goes out unaltered in two packets
    /// (`0x2E` and the `0x78` outfit list), and every rule that reads it — what
    /// a corpse keeps, what armour counts, what may not be lifted — is naming a
    /// slot rather than doing arithmetic. `docs/protocol_newtypes.md` N4.
    pub layer: Layer,
}

/// Marks a container as one half of a secure trade window.
///
/// A trade escrow is an ordinary [`Container`] worn on a layer no player can
/// reach, which is what makes reach, dropping in and lifting out work with no
/// new machinery. This marker is the one fact three places have to know, rather
/// than a magic layer number written down three times:
///
/// - it is **not drawn** — `WorldState::equipment_of` skips it, or every onlooker
///   sees a mystery box hanging off both traders' paperdolls;
/// - it is **not saved** — the inventory sweep skips it and everything in it,
///   for the reason a spell field is skipped: a trade is transient, and a
///   restored one would be an escrow nobody can ever close;
/// - it **cannot be lifted**, which is ServUO's `CheckLift` refusing outright.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TradeWindow;

/// Marks an item as one that stacks: two of them of the same graphic and hue
/// are one pile, not two objects.
///
/// A marker, not a rule engine. Gold, arrows and reagents carry it; a sword does
/// not, which is why dropping a sword on a sword leaves two swords. Whether a
/// graphic stacks is really a tiledata fact, but keeping it an explicit component
/// set at spawn keeps the rule where a script can see it — the §6 way.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Stackable;

/// When an item on the ground will rot away, as a tick number.
///
/// A tick count and not an `Instant` on purpose: the tick already counts itself,
/// so decay is checked against the world's tick counter and stays as
/// deterministic and replayable as everything else the tick does — no clock read
/// inside it. An item carries this only while it is on the ground; lifting it,
/// putting it in a container or wearing it takes the clock off it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Decays {
    /// The tick at or after which it rots.
    pub at_tick: u64,
}

/// What something is called.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Name(pub String);

/// One quest a character has taken, and how far along it is.
///
/// `progress` runs parallel to the definition's objective list — one count per
/// objective, in the same order. Positional, like ServUO's own save, which is why
/// **reordering a quest's objectives invalidates saved progress on it**; adding
/// one to the end is safe.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QuestState {
    /// Which quest, by the pack's key.
    pub key: String,
    /// How far each objective has got.
    pub progress: Vec<u16>,
    /// Ticks left on each timed objective; `0` on the untimed ones.
    pub seconds_left: Vec<u32>,
    /// Whether a timer ran out on it. A failed quest stays in the log, in red,
    /// until it is resigned — ServUO shows it rather than removing it, so the
    /// player finds out why it stopped counting.
    pub failed: bool,
    /// Who gave it, so the turn-in knows where to go back to. `None` once that
    /// mobile is gone.
    pub giver: Option<Serial>,
}

/// A quest a character has finished, and when they may take it again.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DoneQuest {
    /// Which quest, by the pack's key.
    pub key: String,
    /// The tick it may be offered again at. [`u64::MAX`] never.
    pub restart_at: u64,
}

/// A player's quest log: what they are doing, and what they have done.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct QuestLog {
    /// Quests in progress, newest last. The gump lists them newest first.
    pub active: Vec<QuestState>,
    /// Quests finished, with their cooldowns.
    pub done: Vec<DoneQuest>,
}

impl QuestLog {
    /// The state of an active quest, if it is one.
    #[must_use]
    pub fn active_quest(&self, key: &str) -> Option<&QuestState> {
        self.active.iter().find(|quest| quest.key == key)
    }

    /// The state of an active quest, to change.
    pub fn active_quest_mut(&mut self, key: &str) -> Option<&mut QuestState> {
        self.active.iter_mut().find(|quest| quest.key == key)
    }
}

/// An NPC that offers quests — ServUO's `MondainQuester`, as a component.
///
/// The binding lives on the mobile and is **saved with it**, which is the whole
/// point: the script that placed the NPC knows it is a giver only during the run
/// that placed it, so a binding held anywhere else is lost at the first restart
/// and the NPC goes quietly inert.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QuestGiver {
    /// Which quests it may offer, by key, in preference order.
    pub keys: Vec<String>,
}

/// An NPC that can be escorted somewhere — ServUO's `BaseEscortable`.
///
/// Saved with the mobile for the same reason [`QuestGiver`] is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Escortable {
    /// The region it wants to reach, by name. Empty means "wherever the escorter's
    /// quest says", picked when the escort is accepted.
    pub destination: String,
    /// Who is leading it, once someone is.
    pub escorter: Option<Serial>,
    /// The last tick its escorter was within sight. An escortable left behind
    /// gives up rather than following a ghost across the map.
    pub last_seen: u64,
}

/// The account a player character belongs to.
///
/// Kept out of [`Client`] so that stays `Copy` — this is a heap string, and the
/// only thing that needs it is persistence, turning an entity into a record that
/// remembers whose character it is. An NPC has no account and no `Client`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Account(pub AccountName);

/// Marks an item as script-placed decoration: a sign, a piece of furniture, an
/// ankh — the things a shard adds on top of the static art the client's map
/// already draws.
///
/// It sets the item apart from loose clutter: decoration never decays and cannot
/// be picked up (a town's fittings are not loot), and clearing decoration finds
/// its items by this. Placed through `Command::Decorate`; the client draws it as an
/// ordinary `0x1A` item.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Decoration;

/// Marks an item as a door: a decoration that opens and closes on double-click.
///
/// A UO door is two graphics and a small position shift. Closed it draws
/// `closed`; opened it draws `open` (always `closed + 1` in the client's art) and
/// hops one tile off its frame by `(offset_x, offset_y)` — the hinge swing. The
/// same double-click toggles it back. `open_at` is the tick the door auto-closes
/// on, mirroring the real client's self-closing door; `0` means it is shut.
///
/// The graphic and offset are the client's, computed once from ServUO's door
/// tables when the pack places the door, so the engine stays a generic toggle and
/// knows nothing about door *families*.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Door {
    /// The graphic drawn while shut.
    pub closed: Graphic,
    /// The graphic drawn while open.
    pub open: Graphic,
    /// How far the door hops east/west when it swings open.
    pub offset_x: i16,
    /// How far it hops north/south.
    pub offset_y: i16,
    /// Whether the door is currently open.
    pub is_open: bool,
    /// The tick it auto-closes on when open; `0` when shut.
    pub close_at: u64,
}

/// How widely known a mobile is — ServUO's `Mobile.Fame`, `0..=32000`.
///
/// Earned by killing things, and by killing *famous* things in particular: a creature
/// gives up its own fame. Half of what a character's title is computed from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Fame(pub i32);

/// Which way a mobile is known — ServUO's `Mobile.Karma`, `-32000..=32000`.
///
/// Killing something evil earns karma; killing something innocent loses it. The other
/// half of the title, and the axis a creature's own notoriety colour is derived from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Karma(pub i32);

/// A lock on a door or a container — ServUO's `ILockable`: `Locked` plus a
/// `KeyValue` that says which key fits.
///
/// # A lock is a refusal, not a second kind of door
///
/// Everything about a locked door is the same as an unlocked one — the graphic, the
/// offset, the auto-close, the obstruction it registers while shut. The only
/// difference is that the thing which would open it does not. So this is a marker
/// beside [`Door`] rather than a field inside it, and the two places that open a
/// door consult it: a player's double-click (answered with cliloc 502503, "That is
/// locked.") and the AI's decree, which is what stops a townsperson strolling
/// through a locked shopfront on its way home.
///
/// `key_value` is ServUO's: a key fits when its own value matches, and `0` is a lock
/// no key in the world opens — a set-piece door, not a mistake.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Lock {
    /// Which key opens it. `0` fits no key.
    pub key_value: u32,
    /// The Lockpicking a thief needs before the lock will even be tried, in tenths
    /// — ServUO's `LockLevel`. Zero is a lock anybody may attempt.
    pub required_skill: u16,
    /// The skill at which it is no challenge at all, in tenths — ServUO's
    /// `MaxLockLevel`, the top of the band a pick is rolled against.
    pub max_skill: u16,
}

/// A key, and what it opens — ServUO's `Key.KeyValue`.
///
/// Using a key raises a target cursor; clicking a [`Lock`] whose `key_value` matches
/// turns it. The value and not the item is what matters, so a copied key works and a
/// key to another door does not.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyValue(pub u32);

/// Which spawn region put this mobile here — the region's id, which *is* its
/// index in the world's spawner list. The two are one number by construction;
/// nothing hands a region an id from anywhere else.
///
/// The region counts its live creatures by this to know when to refill. A
/// creature dies and is despawned, the component goes with it, the count drops,
/// and the region spawns another. Absent on players and on script- or GM-spawned
/// mobiles, which no region maintains.
///
/// It is saved with the creature and restored with it, so a region comes back
/// knowing which of its creatures are still alive. That is the whole reason the id
/// may not be a counter of its own: the tag written last week is read against the
/// list rebuilt this morning, and only a slot survives that trip.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SpawnedBy(pub u32);

/// A mobile's staff authority — what privileged commands it may run.
///
/// Set on world entry from the account's configured level, not saved with the
/// character: authority is a property of who is logged in, re-derived each login,
/// so a demoted account loses it the next time it plays. A mobile with no `Access`
/// is a [`AccessLevel::Player`], the same as the default the level carries.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Access(pub AccessLevel);

/// A mobile that is *acting* as staff right now — Sphere's `PRIV_GM` flag.
///
/// The other half of the split [`Access`] starts: the level says who *may*
/// command, this says who is currently held to none of the game's rules. A staff
/// account gets it at login and `.gm` takes it off, so a game master can walk
/// under a player's rules — tiring, blind to ghosts — without giving up the
/// commands that let them switch back. Never saved: like [`Access`], it is
/// derived from the account, not from the character.
///
/// Every in-game exemption reads it through
/// [`WorldState::is_staff`](crate::WorldState::is_staff); nothing should test
/// `Access` for one, or the two halves drift apart.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Staff;

/// Marks an entity as driven by a person rather than by the server.
///
/// Carries the connection so the world can answer it — and nothing else. What
/// the client *is* lives on the connection's own row
/// ([`session::Session`](crate::session::Session)), not here: a version held on
/// the entity is a version that does not exist until a character does, which is
/// what made a connection on the character screen unaddressable. Ask
/// [`WorldState::version_of`](crate::WorldState::version_of) with the connection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Client {
    /// Which connection.
    pub connection: ConnectionId,
}

/// A mobile's three stats: strength, dexterity, intelligence.
///
/// The numbers everything derived hangs off. Strength sets how many hit points a
/// mobile can have, intelligence how much mana; dexterity will pace its swings
/// and its stamina once those derive rather than sit as constants. A script sets
/// them (character creation, a monster's build); the maxima follow.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Stats {
    /// Raw might — the cap on hit points.
    pub strength: u16,
    /// Quickness — the cap on stamina, and the pace of a swing, to come.
    pub dexterity: u16,
    /// Wits — the cap on mana.
    pub intelligence: u16,
}

/// A mobile's hit points: how much it has, and how much it can have.
///
/// The thing combat spends. A mobile is alive while `current > 0` and dead at
/// zero. Only mobiles carry it — an item on the ground has no health to lose.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Hitpoints {
    /// How much it has now.
    pub current: u16,
    /// The most it can have.
    pub max: u16,
}

/// Marks a mobile as temporarily a criminal: grey, and freely attackable,
/// until the tick it wears off.
///
/// The consequence of an aggressive act on someone blue — the flag that stops a
/// player attacking innocents in a town with no cost. A tick number, like
/// [`Decays`]; when the tick counter passes it the mobile goes back to innocent
/// (or to murderer, if it has become one — see [`Murders`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CriminalUntil {
    /// The tick the flag lifts.
    pub tick: u64,
}

/// A mobile that cannot move until its tick — paralysis, from the Paralyze spell
/// or a Paralyze Field. The one hard rule of paralysis: the walk and the step both
/// refuse while it holds; a blow lifts it (damage wakes you); it lapses on the tick
/// counter. Casting and swinging are *not* barred (the classic engine leaves those
/// to the client), only movement. A tick number, like [`CriminalUntil`], so it
/// replays.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Frozen {
    /// The tick the mobile can move again.
    pub until: u64,
}

/// Poison working through a mobile: its strength, the tick its next pulse lands,
/// and how many pulses remain before it clears. Tick counts, never a clock — a
/// poisoned fight replays like decay and the criminal flag — so `poison_tick`
/// reads only the world's counter.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Poisoned {
    /// The poison level, 0 (lesser) .. 4 (lethal) — sets the damage per pulse.
    pub level: PoisonLevel,
    /// The tick the next pulse of damage lands.
    pub next_pulse: u64,
    /// Pulses left before the poison wears off.
    pub pulses_left: u8,
}

/// Poison an *item* carries: a dose in a bottle, or a coating on a blade.
///
/// One component for both because they are the same fact — how strong the poison is
/// and how much of it is left — and what an item can *do* with it is decided by
/// what the item is, exactly as ServUO decides (`targeted is BasePoisonPotion`
/// against `targeted is BaseWeapon`). A potion holds one dose; a blade the Poisoning
/// skill has coated holds `18 - level * 2`, spent a charge per landed blow.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PoisonCharges {
    /// The poison level, 0 (lesser) .. 4 (lethal) — the same scale [`Poisoned`] uses.
    pub level: PoisonLevel,
    /// Doses left. Zero means spent, and a spent coating is removed rather than
    /// kept at zero, so this is never `0` on a live component.
    pub charges: u16,
}

/// A musical instrument, and how many tunes are left in it — ServUO's
/// `BaseInstrument.UsesRemaining`.
///
/// The bard skills all need one in the pack, and each attempt spends a use. Which
/// *sounds* it makes is a property of the class, so it lives in the core table
/// keyed by graphic; how worn this particular one is lives here.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Instrument {
    /// Tunes left. At zero the instrument plays its last and is gone.
    pub uses_left: u16,
}

/// A harvesting tool, and how many swings are left in it — ServUO's
/// `BaseHarvestTool.UsesRemaining`.
///
/// The sibling of [`Instrument`], and the same interface in ServUO
/// (`IUsesRemaining`): which *system* a tool drives is a property of its class and
/// lives in the core table ([`crate::harvest::tool_data`]), how worn this
/// particular pickaxe is lives here. At zero the tool breaks and is gone.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tool {
    /// Swings left.
    pub uses_left: u16,
}

/// A harvest in progress — ServUO's `HarvestTimer`.
///
/// The one gathering fact that is genuinely stateful, and the reason it is a
/// component rather than a local: a swing takes several beats, and between them
/// the harvester can walk away, the vein can be emptied by somebody else, or the
/// shard can tick a hundred times. Every field but the target is answered by the
/// tick counter, like [`Decays`] and a swing timer, so a harvest replays.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Harvesting {
    /// The tool being swung. Re-checked each beat: a pickaxe dropped mid-swing
    /// mines nothing.
    pub tool: EntityId,
    /// The tile being worked.
    pub at: Point,
    /// Which system this is, so the beat needs no second lookup.
    pub kind: crate::harvest::HarvestKind,
    /// The tile id, as [`crate::harvest::tile_key`] matched it — kept so the beat
    /// can confirm the ground has not changed under the swing.
    pub tile: Graphic,
    /// Beats still to come. The last one delivers.
    pub beats_left: u16,
    /// The tick the next beat falls on.
    pub next_beat: u64,
    /// The tick this beat's *sound* falls on, or [`u64::MAX`] once it has played.
    ///
    /// A second clock rather than one, because ServUO gives the swing and the
    /// noise it makes different delays (`EffectDelay` against `EffectSoundDelay`):
    /// a pick is raised, and the chink comes most of a second later. Collapsing
    /// them makes a miner sound like a metronome.
    pub next_sound: u64,
}

/// A craft in progress — ServUO's `CraftItem.InternalTimer`.
///
/// The sibling of [`Harvesting`], and stateful for the same reason: a craft takes
/// a beat or several, and in between the crafter can walk away from the forge,
/// hand the ingots to a friend, or wear the tongs out on something else. Every
/// gate is re-checked on the last beat rather than trusted from the first, which
/// is why the recipe is held as a pair of indices and not as a resolved plan.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Crafting {
    /// Which craft system, by its index in the core table.
    pub system: u8,
    /// Which of that system's recipes, by index.
    pub recipe: u16,
    /// The tool in hand. Re-checked each beat: tongs dropped mid-craft make
    /// nothing.
    pub tool: EntityId,
    /// Which material off the system's axis — the ore or wood the player chose in
    /// the gump.
    pub sub_res: u8,
    /// Beats still to come. The last one resolves.
    pub beats_left: u8,
    /// The tick the next beat falls on.
    pub next_beat: u64,
}

/// How well a crafted item came out — ServUO's `IQuality.Quality`.
///
/// Only ever present on an *exceptional* piece: an ordinary item carries no
/// component at all, which is what keeps the column the size of the handful of
/// masterpieces on a shard rather than the size of every item in it.
///
/// Read where it matters and folded into nothing — the armour rating derives it
/// at the read site the way a weapon's speed derives from what is on the hand, so
/// a fine breastplate coming off leaves no bookkeeping behind.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Quality {
    /// Whether it is exceptional. A field rather than a bare marker because
    /// ServUO's scale has a low grade too, and a shard that wants it should widen
    /// this rather than add a second component.
    pub exceptional: bool,
}

/// Who made it — ServUO's `ICraftable.Crafter`, the maker's mark.
///
/// A **name and not a serial**, for the reason [`Corpse`]'s killer is one: the
/// smith logs out, retires, or is deleted, and the sword outlives all three. A
/// serial would leave "crafted by (nobody)" on every good blade on the shard.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CraftedBy(pub String);

/// A mobile a bard has calmed — ServUO's `BaseCreature.BardPacified`.
///
/// It does not swing and it does not pick fights while this holds, which is read at
/// combat's and the AI's own decision points rather than folded into either. A tick
/// count, like every other expiry.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Pacified {
    /// The tick the calm lifts.
    pub until: u64,
}

/// A mobile a bard has put out of tune — ServUO's Discordance.
///
/// `penalty` is a percentage taken off everything the target is good at. It is read
/// in exactly one place, `skills::skill_value`, which is what every other system
/// asks when it wants to know how good somebody is — so a discorded creature hits
/// worse, resists worse and casts worse without any of those three knowing what a
/// lute is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Discorded {
    /// How much worse at everything, as a percentage.
    pub penalty: u16,
    /// The tick the song wears off.
    pub until: u64,
}

/// What a trap on a container does when it goes off — ServUO's `TrapType`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TrapKind {
    /// A flash and a jolt: damage to whoever is standing at the lid.
    Magic,
    /// A blast: the heaviest damage, and it reaches three tiles.
    Explosion,
    /// A dart in the flesh — physical damage.
    Dart,
    /// A noxious green cloud: poison rather than damage.
    Poison,
}

/// A trap on a container: what it does, how hard it hits, and how hard it is to
/// take off — ServUO's `TrapableContainer` fields (`TrapType`, `TrapPower`,
/// `TrapLevel`).
///
/// It springs when the container is opened by anyone but staff, and Remove Trap is
/// the skill that takes it off. Both halves matter: without the trigger a trap is a
/// decoration, and without the disarm it is a wall.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Trap {
    /// What it does.
    pub kind: TrapKind,
    /// How hard it hits when `level` is zero, and the difficulty Remove Trap is
    /// rolled against either way (`power .. power + 10`).
    pub power: u16,
    /// The chest's level, which scales the damage instead of `power` when set.
    pub level: u8,
}

/// The item graphic every poison potion shares — `0x0F0A`, ServUO's
/// `BasePoisonPotion : base(0xF0A, effect)`.
///
/// All four strengths are the same bottle: which poison one holds is on the item
/// (a [`PoisonCharges`]), not in its graphic, which is why the core cannot key
/// poison off a table the way it keys a weapon's damage.
pub const POISON_POTION_GRAPHIC: Graphic = Graphic(0x0F0A);

/// The empty bottle a used potion leaves behind — ServUO hands one back on every
/// `Consume`.
pub const EMPTY_BOTTLE_GRAPHIC: Graphic = Graphic(0x0F0E);

/// What a persistent field does — the behaviour a field-tile entity carries.
///
/// A spell lays a row of ground tiles that either pulse harm or bar the way, on
/// the tick counter like [`Poisoned`] and decay.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FieldKind {
    /// Fire Field — pulses fire damage to whoever stands on it; not a wall.
    Fire,
    /// Poison Field — poisons whoever stands on it; not a wall.
    Poison,
    /// Energy Field — an impassable wall; no damage.
    Energy,
    /// Wall of Stone — an impassable wall; no damage.
    Stone,
    /// Paralyze Field — freezes whoever walks onto it ([`Frozen`](super::Frozen));
    /// not a wall, because you must be able to step on to be caught.
    Paralyze,
}

impl FieldKind {
    /// Whether a mobile cannot walk onto this field — a wall (Energy, Stone), not
    /// a hazard you cross and are caught by (Fire, Poison, Paralyze).
    #[must_use]
    pub fn blocks(self) -> bool {
        matches!(self, Self::Energy | Self::Stone)
    }

    /// Whether this field acts on whoever stands on it each cadence (damage for
    /// Fire/Poison, a freeze for Paralyze) — as opposed to a passive wall.
    #[must_use]
    pub fn pulses(self) -> bool {
        matches!(self, Self::Fire | Self::Poison | Self::Paralyze)
    }
}

/// One tile of a persistent field — a ground entity that pulses harm or blocks the
/// way until its tick comes. The field counterpart of [`Poisoned`]: `next_pulse`
/// and `expires_at` are tick counts, so a field replays like decay.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Field {
    /// What the field does.
    pub kind: FieldKind,
    /// Who laid it — a Fire Field's damage is credited to the caster, so a field
    /// kill counts.
    pub caster: Serial,
    /// The tick the next pulse of harm lands (Fire, Poison); unused for a wall.
    pub next_pulse: u64,
    /// The tick the tile vanishes.
    pub expires_at: u64,
    /// Whether the tile is registered in the obstruction index (Energy, Stone).
    pub blocks: bool,
}

/// The z-span a wall-like field tile occupies in the obstruction index — tall
/// enough that a mobile's own span always intersects it, so it bars the way like a
/// shut door.
pub const FIELD_HEIGHT: u8 = 20;

/// The kind tag on a saved effect and a live [`StatMod`], canonical across the
/// engine.
///
/// One numbering, shared by everything that reads or writes an effect: the
/// persistence [`EffectRecord`](openshard_persistence) stores this `u8`, `magic`
/// tags a [`StatMod`] with it, and the world's save/restore translates the two.
/// Poison (`0`) is the odd one out — its live form is [`Poisoned`], not a
/// `StatMod` — but it shares the numbering so one effects list carries the lot.
/// The order is frozen: a saved `4` must always mean Bless, or old saves rot.
pub mod effect {
    /// Poison — [`Poisoned`](super::Poisoned), not a stat modifier.
    pub const POISON: u8 = 0;
    /// Strength: `+str`.
    pub const STRENGTH: u8 = 1;
    /// Agility: `+dex`.
    pub const AGILITY: u8 = 2;
    /// Cunning: `+int`.
    pub const CUNNING: u8 = 3;
    /// Bless: `+` all three.
    pub const BLESS: u8 = 4;
    /// Weaken: `-str`.
    pub const WEAKEN: u8 = 5;
    /// Clumsy: `-dex`.
    pub const CLUMSY: u8 = 6;
    /// Feeblemind: `-int`.
    pub const FEEBLEMIND: u8 = 7;
    /// Curse: `-` all three.
    pub const CURSE: u8 = 8;
    /// Night Sight — a personal light override, not a stat. See
    /// [`BehaviourBuffs`](super::BehaviourBuffs).
    pub const NIGHT_SIGHT: u8 = 9;
    /// Protection — a chance a blow does not break concentration mid-cast.
    pub const PROTECTION: u8 = 10;
    /// Reactive Armor — a share of melee physical damage reflected to the attacker.
    pub const REACTIVE_ARMOR: u8 = 11;
    /// Magic Reflection — bounces the next offensive spell back at its caster.
    pub const MAGIC_REFLECT: u8 = 12;
    /// Paralyze — a [`Frozen`](super::Frozen) mobile that cannot move until it lifts.
    pub const PARALYZE: u8 = 13;
}

/// Valid kinds for a live stat modifier. Persistence keeps the raw tag and
/// uses `from_u8` at its boundary.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StatEffectKind(u8);

impl StatEffectKind {
    pub const STRENGTH: Self = Self(effect::STRENGTH);
    pub const AGILITY: Self = Self(effect::AGILITY);
    pub const CUNNING: Self = Self(effect::CUNNING);
    pub const BLESS: Self = Self(effect::BLESS);
    pub const WEAKEN: Self = Self(effect::WEAKEN);
    pub const CLUMSY: Self = Self(effect::CLUMSY);
    pub const FEEBLEMIND: Self = Self(effect::FEEBLEMIND);
    pub const CURSE: Self = Self(effect::CURSE);
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self.0
    }
    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            effect::STRENGTH
            | effect::AGILITY
            | effect::CUNNING
            | effect::BLESS
            | effect::WEAKEN
            | effect::CLUMSY
            | effect::FEEBLEMIND
            | effect::CURSE => Some(Self(value)),
            _ => None,
        }
    }
}

/// The kind of a timed magical buff that changes behaviour rather than stats.
///
/// Its raw value is stable in saved effect records, but live code must not mix
/// it with a stat-modifier or any unrelated `u8`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BehaviourBuffKind(u8);

impl BehaviourBuffKind {
    pub const NIGHT_SIGHT: Self = Self(effect::NIGHT_SIGHT);
    pub const PROTECTION: Self = Self(effect::PROTECTION);
    pub const REACTIVE_ARMOR: Self = Self(effect::REACTIVE_ARMOR);
    pub const MAGIC_REFLECT: Self = Self(effect::MAGIC_REFLECT);

    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self.0
    }

    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            effect::NIGHT_SIGHT | effect::PROTECTION | effect::REACTIVE_ARMOR | effect::MAGIC_REFLECT => {
                Some(Self(value))
            }
            _ => None,
        }
    }
}

/// Which stats a stat-modifying effect shifts, and by how much.
///
/// The `kind` names *which* stats (Strength touches str, Bless all three); the
/// signed `offset` carries the magnitude and direction. Returns the delta for
/// `(strength, dexterity, intelligence)`. A debuff simply arrives with a negative
/// `offset` — so the same function undoes a buff by being called with the offset
/// negated, which is exactly how [`StatMod`] reversal works.
#[must_use]
pub fn stat_shift(kind: StatEffectKind, offset: i16) -> (i16, i16, i16) {
    use effect::*;
    match kind.as_u8() {
        STRENGTH | WEAKEN => (offset, 0, 0),
        AGILITY | CLUMSY => (0, offset, 0),
        CUNNING | FEEBLEMIND => (0, 0, offset),
        BLESS | CURSE => (offset, offset, offset),
        _ => (0, 0, 0),
    }
}

/// Whether an effect kind lowers a stat rather than raising it — the sign the
/// caster gives its magnitude.
#[must_use]
pub fn is_debuff(kind: StatEffectKind) -> bool {
    use effect::*;
    matches!(kind.as_u8(), WEAKEN | CLUMSY | FEEBLEMIND | CURSE)
}

/// One timed stat modifier: which effect, how much, and the tick it lifts.
///
/// The `offset` is signed and pre-distributed by [`stat_shift`] from the `kind`;
/// it is kept whole so expiry can reverse *exactly* what was applied, even if the
/// base stat changed underneath it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StatMod {
    /// Which effect — an [`effect`] kind (Strength..Curse).
    pub kind: StatEffectKind,
    /// The signed magnitude applied to each stat the kind selects.
    pub offset: i16,
    /// The tick it wears off.
    pub expires_at: u64,
}

/// The stat modifiers working through a mobile — the Bless/Curse family.
///
/// A mobile can carry several at once (Bless stacked over Strength); re-casting
/// one kind refreshes its entry rather than stacking a duplicate. The shift is
/// folded into the live [`Stats`] (and the derived [`Hitpoints`]/[`Mana`] maxima)
/// when the effect lands, so everything that reads a stat sees the buffed value;
/// this component is the ledger that says how much to give back, and when. Tick
/// counts, like every other timed effect, so a buffed fight replays.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct StatMods {
    /// The active modifiers, at most one per kind.
    pub active: Vec<StatMod>,
}

/// One timed behaviour buff — a spell that changes *how* a mobile acts rather than
/// a stat number: Night Sight, Protection, Reactive Armor, Magic Reflection.
///
/// Unlike a [`StatMod`], nothing is folded into a stat, so there is nothing to
/// back out on expiry — the buff simply stops being read at its decision point.
/// The `amount` carries what that point needs (a Protection chance, a Reactive
/// Armor reflect percent); it is unused for the markers (Night Sight, Magic
/// Reflect). Tick counts, like every timed effect, so it replays.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BehaviourBuff {
    /// Which behaviour the buff changes.
    pub kind: BehaviourBuffKind,
    /// The magnitude the buff's decision point reads (chance, reflect percent),
    /// or `0` for a bare marker.
    pub amount: i16,
    /// The tick it wears off.
    pub expires_at: u64,
}

/// The behaviour buffs working through a mobile — the non-stat magical family.
///
/// The sibling of [`StatMods`] for effects that modify a behaviour, not a stat:
/// at most one entry per kind, a recast refreshes rather than stacks, and each
/// entry rides the same saved effects list. Read at the point the behaviour is
/// decided — Reactive Armor in the damage door, Protection at cast disturbance,
/// Magic Reflection where a spell resolves.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct BehaviourBuffs {
    /// The active buffs, at most one per kind.
    pub active: Vec<BehaviourBuff>,
}

/// How many innocents a mobile has killed — the tally that turns it red.
///
/// The deeper standing [`CriminalUntil`] left for later: a persistent count, not
/// a lapsing timer. Once it passes the murder threshold the mobile is a murderer;
/// the grey criminal flag comes and goes, this only fades slowly, one kill at a
/// time, on a [`MurderDecay`] clock.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Murders(pub u16);

/// When a mobile's murder count next drops by one.
///
/// A tick number, like [`Decays`]: old kills age off rather than staying forever,
/// so a reformed killer eventually washes blue again. One count fades per fire,
/// and the clock reschedules until the tally is empty. (Sphere's separate
/// short-term and long-term counts are a finer model this stands in for.)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MurderDecay {
    /// The tick the next count fades.
    pub at_tick: u64,
}

/// The ceiling one skill trains to when nothing has raised or lowered it, in
/// tenths — 100.0. ServUO's per-`Skill` `m_Cap` default.
pub const DEFAULT_SKILL_CAP: u16 = 1000;

/// What a mobile is trained in: each skill it has, by id, as a value in tenths
/// (so 75.5 is stored as 755, and the skill cap is 1000).
///
/// Sparse on purpose — a mobile knows the handful of skills it has been given,
/// not all fifty-odd at zero. An id it has never trained reads as zero.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Skills {
    values: HashMap<Skill, u16>,
    /// How the window trains each skill — `Up` unless the player set an arrow.
    /// Sparse like the values: an untouched skill trains up.
    locks: HashMap<Skill, SkillLock>,
    /// The ceiling on each skill, in tenths. Sparse like the rest: an untouched
    /// skill caps at [`DEFAULT_SKILL_CAP`]. Per-skill and not one shard-wide
    /// number because the gain chance reads *this* skill's headroom, and because
    /// a reward or a profession raises one skill's ceiling alone.
    caps: HashMap<Skill, u16>,
}

impl Skills {
    /// The value of `skill`, in tenths; zero if the mobile has never had it.
    pub fn get(&self, skill: Skill) -> u16 {
        self.values.get(&skill).copied().unwrap_or(0)
    }

    /// Set `skill` to `value` tenths.
    pub fn set(&mut self, skill: Skill, value: u16) {
        self.values.insert(skill, value);
    }

    /// How `skill` is set to train; `Up` unless the player moved its arrow.
    pub fn lock(&self, skill: Skill) -> SkillLock {
        self.locks.get(&skill).copied().unwrap_or_default()
    }

    /// Set how `skill` trains — the up/down/lock arrow.
    pub fn set_lock(&mut self, skill: Skill, lock: SkillLock) {
        self.locks.insert(skill, lock);
    }

    /// The ceiling on `skill`, in tenths; [`DEFAULT_SKILL_CAP`] unless one was set.
    pub fn cap(&self, skill: Skill) -> u16 {
        self.caps.get(&skill).copied().unwrap_or(DEFAULT_SKILL_CAP)
    }

    /// Set the ceiling on `skill`, in tenths.
    pub fn set_cap(&mut self, skill: Skill, cap: u16) {
        self.caps.insert(skill, cap);
    }

    /// Everything trained, added up, in tenths — ServUO's `Skills.Total`, the
    /// number the total cap is weighed against and the gain chance reads.
    ///
    /// Summed on demand rather than kept as a running field: a mirror updated
    /// beside every `set` is one more thing to forget, and the map holds a
    /// handful of entries, not fifty-eight.
    pub fn total(&self) -> u32 {
        self.values.values().map(|&v| u32::from(v)).sum()
    }

    /// Every trained skill and its lock, for persistence — `(skill, value,
    /// lock)`, in no particular order. A skill at zero with a moved arrow
    /// still counts, so a "down" lock the player set is not forgotten.
    pub fn entries(&self) -> impl Iterator<Item = (Skill, u16, SkillLock)> + '_ {
        self.ids()
            .map(move |skill| (skill, self.get(skill), self.lock(skill)))
    }

    /// Every skill this mobile has a value, a lock or a cap for, ascending by
    /// id. The one place the three sparse maps are unioned.
    pub fn ids(&self) -> impl Iterator<Item = Skill> + '_ {
        self.values
            .keys()
            .chain(self.locks.keys())
            .chain(self.caps.keys())
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
    }
}

/// How a stat is set to train — ServUO's `StatLockType`, the arrows on the
/// paperdoll's status bar. The mirror of [`SkillLock`] for strength, dexterity
/// and intelligence, and read by the same gain path: a skill that trains nudges
/// its governing stat only where that stat's arrow points up.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum StatLock {
    /// Train up on use — the default.
    #[default]
    Up,
    /// Give ground, so another stat can rise past the total cap.
    Down,
    /// Held fixed.
    Locked,
}

impl StatLock {
    /// The wire bits — two per stat inside the `0xBF 0x19` lock byte.
    #[must_use]
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Up => 0,
            Self::Down => 1,
            Self::Locked => 2,
        }
    }

    /// From the wire byte. ServUO's handler folds anything above 2 to `Up`.
    ///
    /// Kept for the *saved* byte: a stat lock is one column in the character
    /// record, and a save written by an older build may hold anything. Traffic
    /// goes through [`to_wire`](Self::to_wire)/[`from_wire`](Self::from_wire).
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Down,
            2 => Self::Locked,
            _ => Self::Up,
        }
    }

    /// The same arrow as the protocol names it.
    ///
    /// The wire has one three-way arrow type and this crate has two users of it
    /// — skills store [`SkillLock`] directly, stats have their own enum because
    /// their gain path is separate — so the two are bridged here, by name and in
    /// both directions rather than through a `From` nobody can grep for.
    #[must_use]
    pub const fn to_wire(self) -> SkillLock {
        match self {
            Self::Up => SkillLock::Up,
            Self::Down => SkillLock::Down,
            Self::Locked => SkillLock::Locked,
        }
    }

    /// The arrow a client asked for, as this crate names it.
    #[must_use]
    pub const fn from_wire(lock: SkillLock) -> Self {
        match lock {
            SkillLock::Up => Self::Up,
            SkillLock::Down => Self::Down,
            SkillLock::Locked => Self::Locked,
        }
    }
}

/// When a mobile may next use a skill from the window.
///
/// ServUO's `Mobile.NextSkillTime`, as a tick count like every other timer here.
/// One clock for all skills, not one per skill: the classic client's window is a
/// list of buttons, and holding any of them down is the thing being prevented.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SkillCooldown {
    /// The tick the next use is allowed on.
    pub until: u64,
}

/// Which way each of a mobile's three stats trains.
///
/// All `Up` by default, so a mobile that has never been told otherwise behaves
/// like every character does on a fresh shard.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct StatLocks {
    /// Strength's arrow.
    pub strength: StatLock,
    /// Dexterity's arrow.
    pub dexterity: StatLock,
    /// Intelligence's arrow.
    pub intelligence: StatLock,
}

/// When each stat last went up, as a tick count.
///
/// ServUO's `LastStrGain`/`LastDexGain`/`LastIntGain` — a per-stat cooldown so a
/// flurry of skill uses cannot pour points into one stat. A tick count and not a
/// clock, like [`Decays`] and [`CriminalUntil`], so it replays.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct LastStatGain {
    /// The tick strength last rose.
    pub strength: u64,
    /// The tick dexterity last rose.
    pub dexterity: u64,
    /// The tick intelligence last rose.
    pub intelligence: u64,
}

/// A living mobile that can hear the dead, until `until` — ServUO's
/// `Mobile.CanHearGhosts`, which Spirit Speak turns on for a while.
///
/// It gates **hearing only**, never drawing: a ghost stays invisible to the living
/// however much Spirit Speak they have, and the point of the classic skill is that
/// you catch a voice with nobody there. So the two questions are two predicates —
/// [`WorldState::can_see_mobile`] and [`WorldState::can_hear_mobile`] — and only the
/// second consults this.
///
/// A tick count, like every other expiry in the engine, and deliberately **not**
/// saved: fifteen seconds to three minutes puts it in the same class as a cast in
/// flight or a field on the ground.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct HearsGhosts {
    /// The tick the contact fades.
    pub until: u64,
}

/// A creature that can be tamed, and what it takes — ServUO's `BaseCreature`
/// `Tamable`/`MinTameSkill`/`ControlSlots`.
///
/// Data about the *kind*, which is why the core keeps a table of it keyed by body
/// ([`crate::tame`]) and a spawn may override it: a shard's pack decides what walks
/// in its woods, and the engine decides what a horse is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tamable {
    /// The Animal Taming needed even to try, in tenths — ServUO's `MinTameSkill`.
    pub min_skill: u16,
    /// How much of a tamer's following it takes up, in slots.
    pub slots: openshard_protocol::world::FollowerSlots,
}

/// What a tamed creature is doing, and for whom — ServUO's `ControlOrder`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PetOrder {
    /// Walk at the owner's heel.
    Follow,
    /// Come here, then stand.
    Come,
    /// Stay where you are.
    Stay,
    /// Stand watch and answer anything that strikes the owner.
    Guard,
    /// Kill what the owner named.
    Attack,
    /// Stop whatever you were doing.
    Stop,
}

/// A tamed creature: whose it is, and what it was last told.
///
/// The pet's *brain* reads this and decides a step, exactly as a wild creature's
/// does — a pet is not a second kind of mobile, it is a creature with an owner and
/// an order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Pet {
    /// Whose it is, by wire serial — a serial rather than an entity because the
    /// owner logs out and comes back while the pet stands where it was.
    pub owner: Serial,
    /// How many follower slots it fills.
    pub slots: openshard_protocol::world::FollowerSlots,
    /// What it was last told to do.
    pub order: PetOrder,
    /// Whom that order was about, for Attack.
    pub order_target: Option<Serial>,
}

/// A mobile nobody can see — ServUO's `Mobile.Hidden`.
///
/// The marker the whole stealth family hangs off. It is read in exactly one place,
/// [`WorldState::can_see_mobile`], which is the same choke point `Ghost` uses and
/// the reason a hidden mobile is drawn to nobody without a single draw site knowing
/// what hiding is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Hidden;

/// A hidden mobile that may move without being seen, for a few steps — ServUO's
/// `AllowedStealthSteps`.
///
/// Hiding alone is broken by the first step; Stealth buys `value / 10` of them
/// (pre-AoS), counted down by the movement paths. When they run out the next step
/// breaks cover, which is what makes the skill a budget rather than a switch.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Stealthing {
    /// Steps left before the next one gives you away.
    pub steps_left: u16,
}

/// A healer part way through a bandage — ServUO's `BandageContext`.
///
/// The one skill in the engine whose *duration* is the mechanic: it takes seconds,
/// the patient can be hurt again meanwhile, and it finishes on the tick counter
/// like everything else that waits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Bandaging {
    /// Who is being patched up.
    pub patient: EntityId,
    /// The tick the work is done.
    pub done_at: u64,
}

/// A mobile sitting in a meditative trance — ServUO's `Mobile.Meditating`.
///
/// A marker, not a timer: a trance has no duration and ends when something breaks
/// it, which is any *disruptive* action (the same set that reveals someone hidden).
/// While it holds, mana comes back twice as fast — see the mana regen rate, which
/// reads this at the moment it decides, with nothing folded in and nothing to undo.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Meditating;

/// A spell in progress — the rooted cast delay of the "servuo" cast style. The
/// mobile is committed to `spell` and cannot walk until `complete_at`, the tick
/// the cast resolves; taking damage in the meantime disturbs it if the shard
/// runs with `spell_disturb`. The "sphere" style never sets this — it resolves a
/// cast as it is made.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Casting {
    /// The spell being cast, by id.
    pub spell: SpellId,
    /// The tick the cast finishes and resolves.
    pub complete_at: u64,
}

/// Marks a mobile as run by the server rather than a person: it has a brain.
///
/// The built-in brain, deliberately simple — notice a nearby foe, chase it,
/// swing (through the same `Combat` a player uses); wander when there is nothing
/// to fight. What it *is* is a couple of knobs a script sets at spawn, so an
/// aggressive ogre and a placid deer differ by data, not code. A brain a script
/// drives itself — a per-tick hook, which the scripting benchmark exists to make
/// affordable — is the richer path this leaves room for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Brain {
    /// How far, in tiles, it notices a foe. Zero never picks a fight.
    pub sight: Sight,
    /// Whether it drifts around when it has nothing to fight.
    pub wander: bool,
    /// The tick it next gets to act — brains think in beats, not every tick.
    pub next_think: u64,
    /// Standing watch until this tick after a chase found no way through —
    /// the give-up both reference emulators use instead of wall-shuffling.
    /// Zero means not guarding.
    pub guard_until: u64,
    /// Whether it opens a shut door in its way rather than treating it as
    /// wall. Humanoids do; animals do not — ServUO's `CanOpenDoors`.
    pub opens_doors: bool,
    /// Whether it starts fights, only answers them, or only runs.
    pub aggression: Aggression,
    /// Ticks between its beats while hunting; `0` takes the shard's default
    /// (`Gameplay::creature_step_ticks`). Idle, it ambles at twice this.
    pub beat_ticks: u64,
}

/// A Magery spellbook's contents: a bit per spell, bit `n` set when the book
/// holds spell `n` (0-based, the same numbering `magic::info` uses). A spellbook
/// is an ordinary item (graphic [`SPELLBOOK_GRAPHIC`]) that also carries this;
/// double-clicking it sends the client the mask (`0xBF 0x1B`), dropping a scroll
/// on it sets a bit, and casting checks one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Spellbook(pub u64);

impl Spellbook {
    /// Whether the book holds spell `n` (0-based).
    #[must_use]
    pub const fn has(self, spell: SpellId) -> bool {
        spell.0 < SPELL_COUNT as u16 && self.0 & (1u64 << spell.0) != 0
    }

    /// Add spell `n` (0-based); a no-op past the eighth circle.
    pub fn learn(&mut self, spell: SpellId) {
        if spell.0 < SPELL_COUNT as u16 {
            self.0 |= 1u64 << spell.0;
        }
    }

    /// Every Magery spell — the "full" book the mage sells for testing.
    #[must_use]
    pub const fn full() -> Self {
        Self(u64::MAX) // all 64 bits; the client reads only the first 64 spells
    }
}

/// The 64 Magery spells, first through eighth circle.
pub const SPELL_COUNT: u8 = 64;

/// A Magery spellbook's item graphic.
pub const SPELLBOOK_GRAPHIC: Graphic = Graphic(0x0EFA);

/// A recall rune's item graphic — ServUO's `RecallRune`.
pub const RECALL_RUNE_GRAPHIC: Graphic = Graphic(0x1F14);

/// A runebook's item graphic — ServUO's `Runebook`, whose constructor defaults
/// to this id.
pub const RUNEBOOK_GRAPHIC: Graphic = Graphic(0x22C5);

/// Where a recall rune points, once the Mark spell has written it.
///
/// A rune with no `RuneMark` is a blank one, which is what makes the component's
/// absence the answer to "is this marked" — there is no `marked: bool` to keep
/// honest beside a destination that means nothing when it is false.
///
/// The facet is part of the destination and not a detail: a rune is an object,
/// it can be carried anywhere, and a rune marked in Britain and read in Ilshenar
/// has to still mean Britain.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RuneMark {
    /// Which facet the destination is on.
    pub facet: Facet,
    /// The tile the rune was marked on.
    pub destination: Point,
}

/// One destination bound into a [`Runebook`].
///
/// Carries its own description rather than pointing at the rune it came from,
/// because the rune is consumed when it is bound — ServUO deletes it — so there
/// would be nothing left to ask.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RunebookEntry {
    /// Which facet the destination is on.
    pub facet: Facet,
    /// The tile bound.
    pub destination: Point,
    /// What to call it in the window — the region's name where there is one.
    pub description: String,
}

/// A book of up to [`RUNEBOOK_ENTRIES`] destinations, and the charges that let it
/// cast to them on its own — ServUO's `Runebook`.
///
/// Not `Copy`, unlike nearly every other component here: it owns its entries and
/// their names. The bus has never required `Copy` — only the enums assumed it —
/// and a component is under no such rule at all.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Runebook {
    /// The destinations bound, in the order they were added.
    pub entries: Vec<RunebookEntry>,
    /// Charges left, each good for one free Recall from the book itself.
    pub charges: u8,
    /// The ceiling recharging fills to — set when the book is made.
    pub max_charges: u8,
    /// Which entry the Recall spell takes when aimed at the book rather than at
    /// a row, if any.
    pub default_entry: Option<u8>,
    /// The tick the book may be opened again — ServUO's `NextUse`.
    ///
    /// Not saved: it is a couple of seconds long, and a restart re-arming it at
    /// zero errs in the generous direction.
    pub next_use: u64,
}

/// How many destinations one runebook holds — ServUO's `Runebook.MaxEntries`.
pub const RUNEBOOK_ENTRIES: usize = 16;

/// A moongate's item graphic — ServUO's `Moongate` and `PublicMoongate` alike.
pub const MOONGATE_GRAPHIC: Graphic = Graphic(0x0F6C);

/// A gate on the ground, and where stepping into it leads.
///
/// Covers both kinds, which differ only in `expires_at`: a Gate Travel spell
/// lays a pair that close after half a minute, and a city moongate stands
/// forever. The pair needs no link field — each gate points at the other's tile,
/// so the link *is* the destination and there are not two halves to keep honest.
///
/// A timed gate is transient, like a cast in flight, and is deliberately left
/// out of the save sweep: restored, it would be a permanent portal whose caster
/// no longer exists. ServUO deletes its own on deserialise for the same reason.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Moongate {
    /// Which facet the far end is on.
    pub facet: Facet,
    /// The tile it leads to.
    pub destination: Point,
    /// The tick it closes, or `None` for one that never does.
    pub expires_at: Option<u64>,
}

/// How tall a gate stands, for the reach test on a double-click. ServUO's
/// `Moongate.OnDoubleClick` wants range 1.
pub const MOONGATE_REACH: u32 = 1;

/// The gump the client opens for a corpse — the loot window, not a chest.
pub const CORPSE_GUMP: Graphic = Graphic(0x0009);

/// What a corpse remembers about how it came to be one — ServUO's `Corpse` fields
/// (`Owner`, `Killer`, `m_Forensicist`, `Looters`).
///
/// A corpse is already a container item with a graphic, a name and a decay clock;
/// this is the part only Forensic Evaluation reads, and it is on the corpse rather
/// than in a side table for the reason every other fact about an item is: the item
/// is swept whole by the save, so the story survives a restart with it.
///
/// The killer and the looters are kept as **names**, not serials. ServUO holds live
/// `Mobile` references and reads `.Name` when the corpse is examined, which cannot
/// answer once the killer has logged out — and a corpse outliving its killer's
/// session is the ordinary case, not the corner one.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Corpse {
    /// Who this was.
    pub owner: String,
    /// Who struck the killing blow, if anybody did. An unattributed death (a fall
    /// into a fire field with no caster, say) leaves `None`, which Forensics reads
    /// out as ServUO's "no one".
    pub killer: Option<String>,
    /// The first forensicist to read it, so a second one is told the work is done —
    /// ServUO's `m_Forensicist`, which it sets on the first successful examination.
    pub examined_by: Option<String>,
    /// Everyone who has taken something off it, in the order they did.
    pub looters: Vec<String>,
}

/// The death shroud a fresh ghost wears — item `0x204E` on the outer-torso
/// layer, the grey robe a dead player rises in. ServUO's `deathShroud`.
pub const DEATH_SHROUD_GRAPHIC: Graphic = Graphic(0x204E);

/// The ghost body a dead player wears — ServUO's `Race.GhostBody`. Female bodies
/// rise as `0x0193`, every other as `0x0192`; the client greys the world once it
/// draws the player in one.
#[must_use]
pub const fn ghost_body(body: Graphic) -> Graphic {
    if body_is_female(body) {
        Graphic(0x0193)
    } else {
        Graphic(0x0192)
    }
}

/// The item graphic of the scroll for a Magery spell, `0-based` — the classic
/// run `0x1F2D..` (Reactive Armor, Clumsy, …), one per spell.
#[must_use]
pub const fn spell_scroll_graphic(spell: SpellId) -> u16 {
    0x1F2D + spell.0
}

/// The Magery spell a scroll graphic teaches, if it is a Magery scroll.
#[must_use]
pub const fn scroll_spell(graphic: Graphic) -> Option<SpellId> {
    // Opened once, so the scroll table below stays terse.
    let graphic = graphic.0;
    let base = 0x1F2D;
    if graphic >= base && graphic < base + SPELL_COUNT as u16 {
        Some(SpellId(graphic - base))
    } else {
        None
    }
}

/// What kind of thing a body is — ServUO's `BodyType`, from `Data/bodyTable.cfg`.
///
/// The table this reads replaced two hand-kept body-id lists (which bodies open doors,
/// which can be ridden). Both were "the safe core of the set" rather than the set, and
/// a list you have to remember to extend is one that goes stale silently.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum BodyType {
    /// Not in the table: ServUO's `BodyType.Empty`, and the default.
    #[default]
    Empty,
    /// A monster — an orc, a lich, a dragon. Has hands, near enough.
    Monster,
    /// A sea creature. Cannot leave the water, cannot work a handle.
    Sea,
    /// An animal. Four legs and no thumbs.
    Animal,
    /// A person.
    Human,
}

// The body-type and mount tables are `data/body_types.json` and
// `data/mounts.json`; `build.rs` sorts them by id and emits the `const`s, with
// their documentation, before this crate compiles. Both are searched on the
// tick path, so both stay `const` slices rather than a map built at startup.
include!(concat!(env!("OUT_DIR"), "/body_types.rs"));
include!(concat!(env!("OUT_DIR"), "/mounts.rs"));

/// The type ServUO gives this body, or [`BodyType::Empty`] for one it does not list.
///
/// A binary search over a sorted table, so it is cheap enough for the tick paths that
/// ask it about every creature in range.
#[must_use]
pub fn body_type(body: Graphic) -> BodyType {
    match BODY_TYPES.binary_search_by_key(&body.0, |&(id, _)| id) {
        Ok(index) => BODY_TYPES[index].1,
        Err(_) => BodyType::Empty,
    }
}

/// Whether a body knows what a door handle is.
///
/// ServUO's `BaseCreature.CanOpenDoors`, exactly: `!Body.IsAnimal && !Body.IsSea`. So
/// an orc follows you through a door and a wolf does not, and a body the table does not
/// list is assumed to have hands — which is ServUO's answer too, since an unlisted body
/// is `BodyType.Empty` and neither of the two things the rule excludes.
///
/// This was a list of eight human body ids, described in its own comment as a stand-in
/// "without body-type tables yet". The whole monster half of Britannia was shut out by
/// a closed door it could have opened.
#[must_use]
pub fn body_opens_doors(body: Graphic) -> bool {
    !matches!(body_type(body), BodyType::Animal | BodyType::Sea)
}

/// The item graphic that draws a body as a mount on a rider, for the bodies that can be
/// ridden at all. `None` is "not rideable", which is what double-click checks first.
///
/// Ported from ServUO's `BaseMount` subclasses — the `base(name, bodyID, itemID, …)`
/// each one passes, plus the alternating body/item arrays a class that rolls between
/// several looks keeps (`Horse` is one of four). Thirty bodies, against the eight the
/// hand-kept list had.
#[must_use]
pub fn mount_item_for(body: Graphic) -> Option<Graphic> {
    MOUNTS
        .binary_search_by_key(&body.0, |&(id, _)| id)
        .ok()
        .map(|index| Graphic(MOUNTS[index].1))
}

/// The creature body a mount-item graphic stands for — the inverse of
/// [`mount_item_for`]. Persistence saves the worn mount item, not the ridden
/// creature (which lives only while ridden), so restoring a saved ride rebuilds
/// the creature from the item it was drawn as. `None` is "not a mount item".
///
/// Derived from the one [`MOUNTS`] table rather than written out again: two
/// hand-kept halves of one mapping is how a saved ride comes back as the wrong
/// animal.
#[must_use]
pub fn mount_body_for(item_graphic: Graphic) -> Option<Graphic> {
    MOUNTS
        .iter()
        .find(|&&(_, item)| item == item_graphic.0)
        .map(|&(body, _)| Graphic(body))
}

// The two creature tables are `data/creature_names.json` and
// `data/creature_sounds.json`; `build.rs` emits both functions, with their
// documentation, before this crate compiles. They stay `const fn` over a
// `match` rather than a search over a slice, and the script rejects a body id
// listed twice — the second arm would be unreachable and the first would
// quietly answer for it, which is how a creature wears another one's name.
include!(concat!(env!("OUT_DIR"), "/creature_names.rs"));
include!(concat!(env!("OUT_DIR"), "/creature_sounds.rs"));

/// Whether a body is female — the human death sound splits male from female,
/// ServUO's `m_Female`. The known female bodies: human, elf and gargoyle.
pub const fn body_is_female(body: Graphic) -> bool {
    matches!(body.0, 0x0191 | 0x025E | 0x02EF)
}

/// A creature that fights at distance — an archer's bow, a mage's bolt, a
/// dragon's breath, abstracted to what the tick needs: how far it reaches and
/// what kind of hurt it is. The damage amount is the creature's `MeleeDamage`;
/// a ranged creature caught in melee still bites with the same number.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RangedAttack {
    /// How far the attack reaches, in tiles.
    pub range: RangedRange,
    /// The kind of damage the attack deals.
    pub kind: DamageType,
}

/// Marks a townsperson as a shopkeeper: it answers double-click with a buy
/// gump and "sell" with an offer list. Its goods live in a container worn on
/// its stock layer, priced item by item with [`Price`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Vendor;

/// What a vendor charges per unit for a stock item. Selling pays half.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Price(pub u32);

/// A mobile being ridden: off every screen and every sector, alive in the
/// registry, waiting for the dismount that puts it back on the ground.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Ridden {
    /// Who sits on it.
    pub rider: EntityId,
}

/// A mobile riding a mount.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Riding {
    /// The creature underneath, held out of the world until dismount.
    pub mount: EntityId,
    /// The mount item worn on the mount layer — what the client draws.
    pub item: EntityId,
}

/// The cached route of a chase, followed a step per beat.
///
/// Replanning A* from scratch every beat is what the old brain did, and it is
/// both wasteful and the direct cause of wall-hugging: a plan that fails one
/// beat was retried identically the next. A route is planned once, followed
/// until it goes stale — the quarry moved, the route ran out, or two seconds
/// passed (the references' repath cadence) — and replanned then.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChasePath {
    /// The remaining route.
    pub steps: Vec<Direction>,
    /// The next step to take.
    pub next: usize,
    /// Where the route was aimed; a quarry that strays invalidates it.
    pub goal: Point,
    /// When it was planned, for the repath clock.
    pub planned_at: u64,
}

/// Which guild a mobile belongs to, and the title it wears inside it.
///
/// A component rather than a list on the [`Guild`](crate::Guild), because the
/// question asked most often is "what guild is *this* mobile in", once per
/// watcher per drawn mobile — see
/// [`notoriety_toward`](crate::WorldState::notoriety_toward). A roster is the
/// rarer direction and is a scan.
///
/// Saved with the mobile: a guild that survived a restart with no members would
/// be a guild nobody could leave.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GuildMember {
    /// Which guild.
    pub guild: crate::guild::GuildId,
    /// The title shown before the name, if the guild gave it one — "Master of
    /// Arms". Empty for a member the guild has not named.
    ///
    /// **Not the [`rank`](Self::rank), and the two are easy to confuse** because
    /// a rank's name is a word a guild would plausibly type into the title
    /// field. A title is free text a leader chose and the engine only clips; a
    /// rank is one of five and is what every permission is decided by. ServUO
    /// keeps them apart the same way (`Mobile.GuildTitle` beside
    /// `PlayerMobile.GuildRank`), and a guild is free to title its Warlord
    /// "Emissary" if it likes.
    pub title: String,
    /// Where they stand in it, and what they are therefore allowed to do.
    ///
    /// Defaults to [`Rank::Ronin`](crate::guild::Rank::Ronin), which is also
    /// what a newcomer joins as — see that type.
    pub rank: crate::guild::Rank,
}

/// A mobile that has been asked to join a guild and has not yet answered.
///
/// On the *candidate*, not on the guild, for the same reason [`GuildMember`] is:
/// the question asked is "has this player been invited", and it is asked of one
/// player at a time. ServUO keeps a `Candidates` list on the guild and reaches
/// the same answer by scanning it.
///
/// One at a time. A second invitation replaces the first rather than queueing,
/// because there is one answer — "yes" — and a queue would make it ambiguous
/// which guild it was to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GuildCandidate {
    /// Which guild asked.
    pub guild: crate::guild::GuildId,
}

/// Which party a mobile is in, and whether it may loot their corpse.
///
/// The **reverse index**, not the roster: the order is on the wire and lives in
/// [`Party::members`](crate::Party). This answers "which party is this mobile
/// in", which is the question asked once per line of party chat and once per
/// corpse.
///
/// Not saved. A party does not survive a restart — see [`crate::party`] for why
/// that is the reference's behaviour and not an omission — so unlike
/// [`GuildMember`] this component is built at run time and never restored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PartyMember {
    /// Which party, by its leader's serial.
    pub party: crate::party::PartyId,
    /// Whether the rest of the party may take from this member's corpse.
    ///
    /// Off by default, which is ServUO's `PartyMemberInfo` — a player has to
    /// say so, and the packet that says it is `0xBF 0x06 0x06`.
    pub can_loot: bool,
}

/// A mobile that has been asked into a party and has not yet answered.
///
/// [`GuildCandidate`]'s twin, and it holds the same shape for the same reason:
/// the question asked is "has this player been invited", of one player at a
/// time. The difference is that a party also keeps its own list — see
/// [`Party::candidates`](crate::Party) — because the capacity rule has to count
/// invitations that are still out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PartyCandidate {
    /// Whose party asked.
    pub party: crate::party::PartyId,
}

/// Marks a player who has died and walks as a ghost: greyed, silent to the
/// living, waiting on resurrection.
///
/// Only players become ghosts — a creature is reaped into a corpse and gone. The
/// world draws a ghost only to other ghosts and to staff
/// (`WorldState::can_see_mobile`), so the living see an empty tile where a dead
/// player stands. A ghost wears the [`ghost_body`] and a death shroud in place of
/// its living body; resurrection lifts the marker and restores both. The living
/// `body` it rose from is remembered here — the ghost body hides it, and without
/// it a raised player would rise the wrong colour or race, and a relogged one
/// could never be brought back at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ghost {
    /// The living body to restore on resurrection.
    pub body: Body,
}

/// Marks a mobile as a banker: a townsperson who opens your bank box when you ask,
/// and greets those who come near.
///
/// The service, not the person — the graphic, name and standing-still are ordinary
/// mobile data a spawn sets; this is the one bit that makes saying "bank" near it
/// do something. A player within reach of any banker gets their own bank box, the
/// same container the bank holds for them everywhere.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Banker;

/// Marks a mobile as a healer: a townsperson who offers a ghost that comes near
/// or double-clicks it a free resurrection, no spell or bandage needed.
///
/// The service, not the person — same shape as [`Banker`]. Unlike a spell's or a
/// bandage's resurrection (a tenth of max hit points, so the raised are not one
/// blow from dying again), a healer's is ServUO's `BaseHealer.OfferResurrection`:
/// full hit points, because the price here is walking to a healer in town rather
/// than surviving the fight that killed you.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Healer;

/// The trade a townsperson plies, in ServUO's form — "the blacksmith", "the
/// banker". The `Title` beside a `BaseVendor`'s `Name`.
///
/// # Why it is a component and not just part of the name
///
/// It is a *key*. Three separate rules look a townsperson's trade up: the outfit
/// generated at spawn, the personal name put in front of it, and — every time
/// anyone speaks nearby — the keyword table that decides what it answers. A trade
/// that lived only inside the `Name` string would have to be parsed back out of
/// it, and one that lived only in the spawn call that placed it would be lost at
/// the first restart, which is exactly how the quest givers went inert (see
/// `MobileRecord::quest_giver`). So it is saved with the mobile.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Title(pub String);

/// A vendor's shelf as it was first stocked, and when it next refills.
///
/// ServUO's `BaseVendor.Restock`, which tops each `IBuyItemInfo` back up to its
/// original amount on `OnRestock`, checked when the shop is opened
/// (`DelayRestock`, an hour). Without it a bought-out shelf stays bought out for the
/// life of the shard, which is what this engine did.
///
/// The original amounts have to be *remembered*, not recomputed: the crate's live
/// contents are what is left, and there is nothing else to compare them against. So
/// the list is kept whole on the vendor and saved with it — a restock timer that
/// forgot its shelf at every restart would be a slower version of the same bug.
///
/// `at` is a tick count, like [`Decays`] and every other timer here, so a shard's
/// economy replays.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Restock {
    /// The tick at or after which the next shop-open refills the shelf.
    pub at: u64,
    /// What the shelf holds when full.
    pub lines: Vec<StockRecord>,
}

/// One line of a vendor's full shelf, inside a [`Restock`].
///
/// The price and the label are part of it, not just the count: a line that sold out
/// entirely leaves no item behind to copy them from, so a restock that only
/// remembered graphics would put nameless goods back on the shelf at a price of one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StockRecord {
    /// The goods' graphic.
    pub graphic: Graphic,
    /// Their hue.
    pub hue: Hue,
    /// How many the shelf holds when full.
    pub amount: u16,
    /// What one unit costs.
    pub price: u32,
    /// The label the client shows.
    pub name: String,
}

/// Where a townsperson sleeps, for the optional daily routine.
///
/// Read only when [`Gameplay::npc_schedule`](crate::Gameplay::npc_schedule) is on;
/// without it an NPC keeps to its post around the clock, which is what both
/// references do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NightHome(pub Point);

/// A townsperson's AI base — what makes a townsperson *live* rather than stand
/// frozen. The shared part every trade reuses; the trade itself is a [`Title`]
/// beside it, and a service a marker like [`Banker`].
///
/// It keeps to a home: the tile it was placed on, and how far it may drift. A
/// beat every so often lets it greet a passer-by, turn to face them, or take an
/// idle step back toward where it belongs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Npc {
    /// The tile it belongs at — a shop counter, a bank.
    pub home: Point,
    /// How many tiles it may stray from `home`; `0` stands perfectly still.
    pub wander: u8,
    /// The tick it next gets a beat.
    pub next_beat: u64,
    /// The earliest tick it may greet or bark again, so it welcomes rather than
    /// natters. It sat on [`Banker`] while bankers were the only townsfolk that
    /// spoke; every trade greets now, so it belongs on the base.
    pub next_greet: u64,
}

/// A mobile's fighting state: whether it is in war mode, whom it is attacking,
/// and when it may next swing.
///
/// Players carry it from the moment they enter; a creature gets one when it
/// starts fighting (which is an `ai` question, not here). `next_swing` is a tick
/// number, like [`Decays`], so the swing timer is checked against the tick
/// counter and never a clock.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Combat {
    /// Whether swings are allowed at all.
    pub warmode: bool,
    /// The mobile being attacked, if any.
    pub target: Option<Serial>,
    /// The tick at or after which the next swing may land.
    pub next_swing: u64,
}

/// A hidden wrestler's next strike is an ambush rather than an ordinary swing.
///
/// This is deliberately a short-lived, target-bound fact.  Hiding does not make
/// every later punch stronger: it only arms the first legal blow after the
/// attacker commits to somebody.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WrestlingOpener {
    /// The mobile the concealed fighter committed to.
    pub target: Serial,
    /// The last tick at which the opening remains armed.
    pub expires_at: u64,
}

/// The recovery that stops repeatedly hiding from producing an unlimited stream
/// of ambushes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WrestlingAmbushCooldown {
    /// The tick at which another ambush may be armed.
    pub until: u64,
}

/// Consecutive unarmed hits against one mobile.
///
/// The count is reset by a miss, a target change, or a pause, so the third hit
/// rewards staying on an opponent rather than merely owning the skill.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WrestlingCombo {
    /// The opponent the sequence belongs to.
    pub target: Serial,
    /// Successful hits already landed in this sequence (one or two).
    pub hits: u8,
    /// The last tick on which the next hit still continues the sequence.
    pub expires_at: u64,
}

/// Recent footwork available to an unarmed fighter.
///
/// Movement records this independently of combat; attacking consumes it only
/// when it earns a first-contact swing, so running cannot accelerate every hit.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WrestlingStride {
    /// Steps made inside the current short window.
    pub steps: u8,
    /// The tick at which the stored steps expire.
    pub expires_at: u64,
}

/// The recovery after using recent footwork to intercept a new target.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WrestlingInterceptCooldown {
    /// The tick at which another intercept can be armed.
    pub until: u64,
}

/// How hard a mobile hits in melee — the base a swing deals before the target's
/// armour takes its cut.
///
/// A mobile-level number: a creature's natural blow, or a script's pin. A player
/// carries none and derives the blow from the weapon wielded (`combat::melee_blow`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MeleeDamage {
    /// The blow before resistance.
    pub amount: u16,
}

/// A per-item weapon override — the pack's magic sword. Placed on a *weapon item*,
/// its speed and damage replace what the core weapon table gives that graphic
/// (`combat::equipped_weapon` reads it first); the weapon's skill still comes from
/// the base graphic. Era-independent: the same numbers whichever combat era runs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Weapon {
    /// Ticks-formula speed base (Sphere's weapon `base`); higher swings faster.
    pub speed: u16,
    /// Minimum damage before resistance.
    pub min: u16,
    /// Maximum damage before resistance.
    pub max: u16,
}

/// How many steps a mobile has taken — ServUO's `PlayerMobile.StepsTaken`, and
/// only ever read modulo the stride between stamina points (`combat::spend_step_stamina`).
///
/// Not saved: a fresh count after a restart costs a player at most one point of
/// stamina, and a saved one would be a column that means nothing to anything else.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Steps(pub u32);

/// A per-item armour override — the pack's enchanted breastplate. Placed on a
/// *worn armour item*, its rating replaces what the core armour table gives that
/// graphic (`combat::armor` reads it first); where the piece sits on the body
/// still comes from the layer it is worn on. Era-independent, like [`Weapon`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Armor {
    /// The piece's rating before body coverage — ServUO's `ArmorBase`.
    pub rating: u16,
}

/// How many ticks a mobile waits between swings.
///
/// One number stands in for what UO derives from a weapon's speed and the
/// wielder's dexterity — neither of which exists yet (there are no stats, and a
/// weapon has no speed). Making it a component a script sets is the honest
/// halfway house: swing speed is data now, and the derivation slots in later
/// without moving where the number is read.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SwingSpeed {
    /// Ticks between blows.
    pub ticks: u64,
}

/// A mobile's armour: how much of each kind of blow it shrugs off, as a
/// percentage. Zero everywhere is no protection.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Resistance {
    /// Percent of physical damage absorbed, 0–100.
    pub physical: u8,
    /// Percent of fire damage absorbed.
    pub fire: u8,
    /// Percent of cold damage absorbed.
    pub cold: u8,
    /// Percent of poison damage absorbed.
    pub poison: u8,
    /// Percent of energy damage absorbed.
    pub energy: u8,
}

impl Resistance {
    /// The percentage that resists `kind` of damage, capped at 100.
    pub fn against(&self, kind: DamageType) -> u8 {
        let value = match kind {
            DamageType::Physical => self.physical,
            DamageType::Fire => self.fire,
            DamageType::Cold => self.cold,
            DamageType::Poison => self.poison,
            DamageType::Energy => self.energy,
        };
        value.min(100)
    }
}

/// A mobile's mana: what casting spends, and how much it can hold.
///
/// The hit-points of magic. A spell that costs more than `current` fizzles; a
/// cast draws it down; it trickles back over time. Only mobiles that cast carry
/// it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Mana {
    /// What it has now.
    pub current: u16,
    /// The most it can have.
    pub max: u16,
}

/// A mobile's stamina: the pool the client reads run-eligibility from, and how
/// much it can hold.
///
/// `max` is dexterity — the UO identity, where the stamina bar *is* dexterity —
/// so a dexterity change re-caps it the way strength re-caps hit points. It
/// trickles back over time like [`Mana`]. Unencumbered foot movement does not
/// spend it in the classic (pre-AoS) era — running is free on open ground — so
/// the pool sits full in normal play; its consumers are combat, being struck,
/// and moving overweight or mounted, which land later. The client refuses to run
/// at zero, so a real pool is what a future push-through mechanic spends against.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Stamina {
    /// What it has now.
    pub current: u16,
    /// The most it can have — dexterity.
    pub max: u16,
}

/// A mobile that can walk: its position, facing, sequence and pace.
///
/// Wraps [`Walker`] rather than replacing [`Position`]: the walk state and the
/// coordinate are asked for by different code at different times, and the tick
/// keeps them in step.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Movement(pub Walker);

/// The region a mobile was last seen in — the remembered half of the crossing
/// diff.
///
/// The world does not call "you have left Britain" beside every step. It keeps
/// this, and one pass compares it against the region under the mobile's feet; a
/// difference is the crossing. Same shape as the status bar's snapshot, and for
/// the same reason: a line beside every mutation is the thing that decays the
/// moment a new mover forgets to write it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InRegion {
    /// Which facet's list [`region`](Self::region) indexes.
    ///
    /// An id alone is not an answer. Each facet numbers its own regions from
    /// zero, so region 3 in Felucca and region 3 in Ilshenar compare equal —
    /// and a traveller crossing between them would look to the diff like
    /// somebody who had not moved: no `RegionChanged`, no music, no guards.
    pub facet: Facet,
    /// The region's id on that facet, or `None` out in the wilds.
    pub region: Option<crate::RegionId>,
}

/// A town guard, summoned to execute someone and gone soon after.
///
/// Not a creature with a life: ServUO's guard is a sentence, and this marker is
/// what says so — the tick it vanishes on, and nothing else. There is no target
/// on it because there is no pursuit; a guard strikes in the moment it arrives.
/// A mobile wearing it is also exempt from earning a murder count,
/// because killing the guilty is its whole purpose (ServUO clears the guard's
/// `Criminal`/`Kills` on every beat, which is the same statement).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Guard {
    /// The tick it despawns, its work done.
    pub until: u64,
}

/// A player's house: one entity that draws as a hundred statics.
///
/// A ship on the water.
///
/// A [`House`]'s twin and deliberately not a variant of it: the two share the
/// fact that they are one entity drawn as a multi, and share nothing else. A
/// house has an access list, an age and an allowance; a boat has a heading and
/// a tiller. Folding them together would mean every reader asking which kind it
/// had before it could ask anything useful.
///
/// The **components are not here**, for [`House`]'s reason and more strongly: a
/// boat's shape is a pure function of its multi id with no designed case at all,
/// so it is exactly what that rule was written for. Where they *are* is
/// [`Boats`](crate::Boats), which holds the derived answer — hull or deck, at
/// what height — rather than the components themselves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Boat {
    /// Which multi it is, `0x4000` below the graphic on the wire.
    pub multi: u16,
    /// Who owns it.
    pub owner: openshard_protocol::serial::Serial,
}

/// A ship holding a course: it is under way, and this is where to.
///
/// Absent on a moored ship, which is why it is its own component rather than an
/// `Option` on [`Boat`]: "is anything sailing" is then a query over a sparse set
/// that is empty on every shard with no ship under way, and the tick's boat pass
/// costs nothing on all of them.
///
/// **Not saved, and that is a decision.** A shard that comes back up finds its
/// fleet at anchor. Persisting a course would mean a ship sailing on through a
/// restart with nobody at the tiller and nobody aboard — the manifest is derived
/// per move, so the crew logged out at the last berth and the ship would leave
/// without them. Stopping is what the reference does too: `BaseBoat` writes its
/// facing but not its motion.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sailing {
    /// Which way, one step at a time.
    pub direction: openshard_protocol::direction::Direction,
    /// The tick this ship may next take a step on — the cadence gate, in the
    /// same units as [`WorldState::ticks`](crate::WorldState::ticks).
    pub next: u64,
    /// How many ticks apart its steps are. Stored rather than recomputed,
    /// because `next` has already passed by the time a step is taken and the
    /// interval cannot be read back out of it.
    pub every: u64,
}

/// Beside a [`Position`] and a [`Drawn`] whose graphic is `0x4000 | multi`, so
/// everything that already walks items — the sector index, the save, the `0x1A`
/// that draws it — works on a house unchanged. What makes it a house is this
/// component, not a table of its own.
///
/// The **components are not here**. They are a pure function of the multi id and
/// they live in the client's files, which is where they are read from at
/// placement — see [`WorldState::multis`](crate::WorldState::multis). Copying them
/// onto the entity would be storing a copy of a file every client already has,
/// and the copy would go stale the day the operator updates their install.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct House {
    /// Which multi it is, `0x4000` below the graphic on the wire.
    pub multi: u16,
    /// Who owns it. A house always has an owner; demolition is what happens when
    /// it would not.
    pub owner: openshard_protocol::serial::Serial,
    /// Everyone trusted with the house short of owning it: they may lock things
    /// down, open every secure, and use the door.
    pub co_owners: std::collections::BTreeSet<openshard_protocol::serial::Serial>,
    /// Everyone who may come in and use the door, and nothing else.
    pub friends: std::collections::BTreeSet<openshard_protocol::serial::Serial>,
    /// Everyone turned away at it.
    ///
    /// A separate list rather than a flag on the other two, because a ban is not
    /// the absence of friendship: a stranger is neither, and the difference is
    /// what the door says when it refuses.
    pub bans: std::collections::BTreeSet<openshard_protocol::serial::Serial>,
    /// How many ticks the house has stood unrefreshed.
    ///
    /// A tick count and not a wall clock, which is D6. An **accumulator** and not
    /// a deadline, unlike every other timer in this engine
    /// ([`Decays`], [`MurderDecay`]), and the difference is that this one has to
    /// cross a restart: the tick counter is not saved — the world's clock is in
    /// UO minutes and `WorldState::ticks` starts at zero every boot — so a
    /// deadline written as an absolute tick would mean nothing on the way back
    /// in, and every house on the shard would come up freshly refreshed.
    ///
    /// Counted up by the decay sweep, zeroed by a refresh, saved as it stands.
    pub age: u64,
    /// How many items may be locked down here, secures included.
    ///
    /// **Computed at placement and stored**, which is D2's own rule one level
    /// up: it is derived from the multi's footprint, and the footprint is a
    /// client-file fact this crate must never reach for. Storing the number
    /// keeps the ceiling askable by anything holding a `House` — the drop path
    /// in `openshard-items` is the one that needs it, and it has no terrain in
    /// hand. ServUO stores its own `MaxLockDowns` on `BaseHouse` for the same
    /// reason and saves it.
    ///
    /// Zero on a house placed by a shard with no client files: nothing can be
    /// locked down in a house whose size this shard cannot know.
    pub lockdowns: u32,
}

impl House {
    /// How many items may sit inside the secures, between them.
    ///
    /// ServUO's AoS table has this at exactly twice the lockdown count on every
    /// one of its rows, so it is derived rather than stored — one number to
    /// compute at placement and one that cannot fall out of step with it.
    #[must_use]
    pub const fn storage(&self) -> u32 {
        self.lockdowns * 2
    }
}

/// Where somebody stands with a house.
///
/// One question and not four booleans, because the reference's are **nested** —
/// a co-owner is a friend, an owner is a co-owner — and four independent answers
/// are four chances to ask the wrong one. See ServUO's `IsFriend`, which is
/// `IsCoOwner(m) || Friends.Contains(m)`, and `IsCoOwner`, which is `IsOwner(m)
/// || ...`.
///
/// # Why it is here and not only in the housing crate
///
/// Because a *door* has to ask it. The double-click dispatch lives in
/// `openshard-items`, which does not depend on `openshard-housing` and should
/// not — a door is not a housing concept. This is [`Guild`](crate::Guild)'s split
/// exactly: the *rules* (trusting, banning, the limits) are the system crate's,
/// and the *question* a wire path has to answer lives on the component. See
/// [`WorldState::notoriety_toward`](crate::WorldState::notoriety_toward), which
/// is here for the same reason.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Standing {
    /// Turned away at the door.
    ///
    /// **Lowest**, so an `Ord` comparison means "at least this trusted" and a
    /// ban is never that.
    Banned,
    /// Neither friend nor enemy. May knock, may not enter.
    Stranger,
    /// May come in and use the door.
    Friend,
    /// Everything but giving the house away.
    CoOwner,
    /// The house is theirs.
    Owner,
}

impl Standing {
    /// The number it is saved as.
    ///
    /// Written out rather than derived from the discriminant, because the
    /// discriminant is an ordering decision — `Banned` is lowest so that a
    /// comparison reads "at least this trusted" — and reordering it must not
    /// silently turn every saved secure into a different access level.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Banned => 0,
            Self::Stranger => 1,
            Self::Friend => 2,
            Self::CoOwner => 3,
            Self::Owner => 4,
        }
    }

    /// Read one back, or `None` for a number this engine did not write.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Banned),
            1 => Some(Self::Stranger),
            2 => Some(Self::Friend),
            3 => Some(Self::CoOwner),
            4 => Some(Self::Owner),
            _ => None,
        }
    }

    /// What to call it on a screen.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Banned => "banned",
            Self::Stranger => "a stranger here",
            Self::Friend => "a friend of this house",
            Self::CoOwner => "a co-owner",
            Self::Owner => "the owner",
        }
    }
}

impl House {
    /// What `who` is to this house.
    ///
    /// The order the checks are made in is the rule: owner first, so nothing can
    /// demote them, then the ban, then the trusted lists. A banned co-owner is
    /// **banned** — the ban is the newer decision and the reference's
    /// `HasAccess` reads it that way, which is what makes "ban them" a usable
    /// answer to a co-owner who has turned.
    ///
    /// `staff` is the caller's, because whether a mobile holds the authority is
    /// [`WorldState::is_staff`](crate::WorldState::is_staff)'s to answer and this
    /// takes no world.
    #[must_use]
    pub fn standing_of(&self, who: openshard_protocol::serial::Serial, staff: bool) -> Standing {
        if who == self.owner {
            return Standing::Owner;
        }
        // Staff walk in anywhere, and are never banned. ServUO's own first branch.
        if staff {
            return Standing::CoOwner;
        }
        if self.bans.contains(&who) {
            return Standing::Banned;
        }
        if self.co_owners.contains(&who) {
            return Standing::CoOwner;
        }
        if self.friends.contains(&who) {
            return Standing::Friend;
        }
        Standing::Stranger
    }
}

/// A door that belongs to a house.
///
/// On the *door*, naming the house, and not a list on the house naming its
/// doors: a door is asked about one at a time, by a double-click that already
/// has the door in hand, and a list would be a second copy of the same fact to
/// keep in step through every demolition.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HouseDoor {
    /// Which house, by its item serial.
    pub house: openshard_protocol::serial::Serial,
}

/// A house whose shape nobody shipped: the components it is made of, made on
/// this shard.
///
/// # Why it cannot live where every other house's shape lives
///
/// A classic house's components come from
/// [`WorldState::multis`](crate::WorldState::multis), keyed by a `u16` and
/// borrowed out of a table fixed at boot. A design is per
/// *house* — two houses on one foundation id have two designs and one key — and
/// it is world state, which that seam is documented as deliberately not being.
/// So it is a component, and the three readers of a house's shape take it as a
/// parameter instead. See `docs/customisation.md`'s D1 and C1.
///
/// # What is never saved, said accurately
///
/// Housing's rule is that components are never saved, because a multi's shape is
/// a pure function of its id and a copy goes stale the day the operator updates
/// their install. A design has no file behind it, so the rule as written cannot
/// cover it — and does not need to be abandoned, only stated precisely: **what
/// is never saved is a copy of something the client's files already state.** A
/// design says nothing they say. It *is* the original, with nothing to go stale
/// against, so it is saved.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HouseDesign {
    /// What the house is made of, in the shape a multi's own list has.
    pub components: Vec<openshard_uofiles::multi::Component>,
    /// Bumped on every commit, so a client can cache the design by
    /// `(serial, revision)` and ask for the whole thing only when what it holds
    /// is stale. Not an optimisation: without it every client walking into an
    /// area re-fetches every design in it, on every approach.
    pub revision: u32,
}

/// The sign standing outside a house — the thing you double-click to see who
/// owns it and to change who may come in.
///
/// On the *sign*, naming the house, for [`HouseDoor`]'s reason and one more: the
/// sign is derived from the house rather than owned by it. It is not saved — a
/// restore rebuilds it from the [`House`] record — so a back-pointer from the
/// house would be a serial that meant one sign before the restart and another
/// after.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HouseSign {
    /// Which house, by its item serial.
    pub house: openshard_protocol::serial::Serial,
}

/// An item pinned inside a house: a **lockdown**, and a **secure** if it names
/// an access level.
///
/// # One component and not two
///
/// A secure *is* a lockdown — it cannot be lifted either, releasing one works on
/// both, and both count against the same allowance. Two components would be two
/// facts that must agree about all three, and the reference's own model is this
/// one: `BaseHouse.Release` takes a secure off the secures list and the item goes
/// back to loose in a single step.
///
/// # Why the access level is a `Standing`
///
/// ServUO's `SecureLevel` is `Owner`, `CoOwners`, `Friends`, `Anyone`, which is
/// the trusted half of [`Standing`] with a fourth name for its bottom.
/// `Standing::Stranger` *is* "anyone", so the enum already had the four and did
/// not need a fifth type beside it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LockedDown {
    /// Which house, by its item serial.
    pub house: openshard_protocol::serial::Serial,
    /// The least standing that may open it, if this is a secure container.
    /// `None` for a plain lockdown, which is not a container and opens for
    /// nobody.
    pub secure: Option<Standing>,
}

/// A deed: the item a house is placed from.
///
/// It carries the multi rather than the house carrying the deed, because the
/// deed is what exists first and the only thing it has to know is which building
/// it becomes. Spent on a successful placement and kept on a refused one — a
/// player who picked a bad spot has lost nothing but a click.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HouseDeed {
    /// Which house it builds.
    pub multi: openshard_protocol::wire::MultiId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_table_is_sorted_so_the_search_finds_things() {
        // Both lookups binary-search. An out-of-order entry does not fail loudly — it
        // silently answers `Empty` for a body that is in the table, and an ogre stops
        // opening doors for no visible reason.
        assert!(
            BODY_TYPES.windows(2).all(|w| w[0].0 < w[1].0),
            "BODY_TYPES must be sorted and unique"
        );
        assert!(
            MOUNTS.windows(2).all(|w| w[0].0 < w[1].0),
            "MOUNTS must be sorted and unique"
        );
    }

    #[test]
    fn doors_open_to_hands_and_not_to_paws() {
        // ServUO's `CanOpenDoors`: `!Body.IsAnimal && !Body.IsSea`. The eight-body list
        // this replaced shut out every monster in Britannia — an orc could not follow
        // you through a door it plainly has hands for.
        assert!(body_opens_doors(Graphic(0x0190)), "a man");
        assert!(body_opens_doors(Graphic(0x0191)), "a woman");
        assert!(body_opens_doors(Graphic(0x0011)), "an orc");
        assert!(!body_opens_doors(Graphic(0x00C9)), "a cat");
        assert!(!body_opens_doors(Graphic(0x00E2)), "a horse");
        // An unlisted body is `BodyType::Empty` — neither animal nor sea — so it has
        // hands, which is ServUO's answer too.
        assert_eq!(body_type(Graphic(0xFFFE)), BodyType::Empty);
        assert!(body_opens_doors(Graphic(0xFFFE)));
    }

    #[test]
    fn the_body_types_are_servuos() {
        assert_eq!(body_type(Graphic(0x0190)), BodyType::Human);
        assert_eq!(body_type(Graphic(0x00E2)), BodyType::Animal);
        assert_eq!(body_type(Graphic(0x0011)), BodyType::Monster);
    }

    #[test]
    fn every_horse_colour_is_rideable_and_round_trips() {
        // The hand-kept list had eight mounts; ServUO has thirty, and four of them are
        // the one `Horse` class rolling between colours — which the first scrape missed
        // entirely, because the colours live in an array and not in the constructor.
        for (body, item) in [
            (0x00C8, 0x3E9F),
            (0x00CC, 0x3EA2),
            (0x00E2, 0x3EA0),
            (0x00E4, 0x3EA1),
            (0x00DC, 0x3EA6),
        ] {
            let (body, item) = (Graphic(body), Graphic(item));
            assert_eq!(mount_item_for(body), Some(item), "body {:#06x}", body.0);
            assert_eq!(mount_body_for(item), Some(body), "item {:#06x}", item.0);
        }
        assert_eq!(mount_item_for(Graphic(0x0190)), None, "a person is not a mount");
        assert!(MOUNTS.len() >= 25, "{} mounts", MOUNTS.len());
    }

    #[test]
    fn no_two_mounts_share_one_item_graphic() {
        // `mount_body_for` is the inverse of one table now, and an inverse only exists
        // if the mapping is one to one — otherwise a saved ride comes back as whichever
        // animal the search happened to reach first.
        let mut items: Vec<u16> = MOUNTS.iter().map(|&(_, item)| item).collect();
        items.sort_unstable();
        let before = items.len();
        items.dedup();
        assert_eq!(before, items.len(), "a mount item graphic is used twice");
    }

    use openshard_entities::Registry;
    use openshard_protocol::direction::Direction;
    use openshard_protocol::serial::SerialKind;

    #[test]
    fn a_player_and_an_npc_differ_only_by_a_component() {
        // The claim the whole ECS rests on. If this ever needs a `kind` field,
        // something has gone wrong.
        let mut registry = Registry::new();
        let (player, _) = registry.spawn_with_serial(SerialKind::Mobile).unwrap();
        let (npc, _) = registry.spawn_with_serial(SerialKind::Mobile).unwrap();

        for entity in [player, npc] {
            registry.insert(entity, Position(Point::new(100, 100, 0)));
            registry.insert(
                entity,
                Body {
                    id: openshard_protocol::wire::Graphic(0x0190),
                    hue: openshard_protocol::wire::Hue(0),
                },
            );
        }
        registry.insert(
            player,
            Client {
                connection: ConnectionId::from_raw(1),
            },
        );

        assert!(registry.has::<Client>(player));
        assert!(!registry.has::<Client>(npc), "an NPC has no connection");
        assert_eq!(registry.count::<Position>(), 2, "both are somewhere");
    }

    #[test]
    fn every_sounded_creature_is_also_named() {
        // The two bestiary tables cover the same creatures: a body that growls has
        // a name to show on single-click too. Names may outrun sounds — passive
        // fauna (a rabbit, a deer) are named but silent — but never the reverse.
        for body in (0u16..=0x0400).map(Graphic) {
            if creature_base_sound(body).is_some() {
                assert!(
                    creature_name(body).is_some(),
                    "body {:#06x} sounds like a creature but has no name",
                    body.0
                );
            }
        }
        // Spot-checks of the extended table (ServUO's BaseSoundID), and that a
        // human body is in neither — it falls back to the fists/gendered sounds.
        assert_eq!(creature_base_sound(Graphic(0x001A)), Some(SoundId(0x0482))); // spectre / wraith
        assert_eq!(creature_base_sound(Graphic(0x000C)), Some(SoundId(0x016A))); // dragon
        assert_eq!(creature_name(Graphic(0x0009)), Some("a daemon"));
        assert_eq!(
            creature_base_sound(Graphic(0x0190)),
            None,
            "a human is not a creature-sound body"
        );
    }

    #[test]
    fn a_rock_has_a_position_and_no_walk_state() {
        // Most things that have a position never walk. Storing a sequence and a
        // pace budget on every tree would be storage for a question nobody asks.
        let mut registry = Registry::new();
        let (rock, _) = registry.spawn_with_serial(SerialKind::Item).unwrap();
        registry.insert(rock, Position(Point::new(50, 50, 10)));

        assert!(registry.has::<Position>(rock));
        assert!(!registry.has::<Movement>(rock));
    }

    #[test]
    fn a_query_finds_every_mobile_that_can_walk() {
        let mut registry = Registry::new();
        let mut walkers = 0;
        for index in 0..10u16 {
            let (entity, _) = registry.spawn_with_serial(SerialKind::Mobile).unwrap();
            registry.insert(entity, Position(Point::new(index, 0, 0)));
            // Only the even ones move.
            if index % 2 == 0 {
                registry.insert(
                    entity,
                    Movement(Walker::new(
                        Point::new(index, 0, 0),
                        Facing::walking(Direction::North),
                    )),
                );
                walkers += 1;
            }
        }
        assert_eq!(registry.count::<Movement>(), walkers);
        assert_eq!(registry.count::<Position>(), 10);
    }
}
