//! What a thing in the world is made of.
//!
//! # Small, plain, and owned by the rule that needs them
//!
//! Nothing here is a "GameObject". A player is an entity that happens to carry a
//! [`Body`], a [`Position`] and a [`Client`]; an NPC is the same minus the
//! `Client`; a rock is a `Position` and a `Graphic`. What a thing *is* falls out
//! of what it carries, which is the whole reason for an ECS.
//!
//! These are the ones the world itself needs to put a character on screen and
//! move it. Combat's components belong to combat, housing's to housing. A
//! `Components` grab-bag every crate imports from would be an inheritance tree
//! with extra steps.

use std::collections::HashMap;

use openshard_entities::{EntityId, Serial};
use openshard_gateway::ConnectionId;
use openshard_movement::Walker;
use openshard_protocol::{AccessLevel, ClientVersion, Facing, Point, SkillLock};

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
    pub id: u16,
    /// Its colour.
    pub hue: u16,
}

/// The graphic an item is drawn as: its tiledata id and hue.
///
/// The item counterpart of [`Body`]. An entity carries one or the other — a
/// mobile a `Body`, a thing on the ground a `Graphic` — and that is what the
/// interest system reads to decide which packet draws it: `0x78` for a body,
/// `0x1A` for a graphic. Kept in `world` and not in a gameplay crate for the
/// same reason `Body` is: drawing a thing in the world is the world's job, and
/// the crate that owns item *rules* (stacking, decay, containment) builds on
/// this rather than the other way round.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Graphic {
    /// The tiledata id.
    pub id: u16,
    /// Its colour, or 0 for none.
    pub hue: u16,
}

/// How many of a stackable item this entity is: a pile of 500 gold is one entity
/// with `Amount(500)`, not 500 entities.
///
/// Separate from [`Graphic`] because most items are single and storing a `1` on
/// every one of them is a column of ones. An item with no `Amount` is a single.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Amount(pub u16);

/// Marks an item as a container: something other items can be put inside.
///
/// The `gump` is the window the client draws when the container is opened — a
/// backpack, a wooden chest, a bank box each have their own. An item is a
/// container exactly when it carries this; nothing else changes about it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Container {
    /// The gump graphic the client opens for it.
    pub gump: u16,
}

/// Marks an item as being *inside* a container rather than on the ground.
///
/// An item carries either a [`Position`] (on the ground, in the sector grid and
/// on nearby screens) or a `Contained` (in a container, on nobody's ground) —
/// never both. The `x`/`y` are where it sits in the container's gump, not world
/// tiles; `grid` is its slot in the enhanced grid view.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Contained {
    /// The container it is in, by serial.
    pub container: Serial,
    /// Its column in the gump.
    pub x: u16,
    /// Its row in the gump.
    pub y: u16,
    /// Its slot in the grid view.
    pub grid: u8,
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
    pub layer: u8,
}

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

/// A player's quest log — an opaque JSON string the community pack owns.
///
/// The engine neither reads nor understands it: it stores it, persists it with the
/// character, and hands it back to the pack on login. Quests are pack gameplay
/// (the "default in core, customise in the pack" split), so their *shape* is the
/// pack's business; this is just the box it rides home in — the same bargain as an
/// item's `Spellbook` mask or a mobile's saved `effects`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct QuestLog(pub String);

/// The account a player character belongs to.
///
/// Kept out of [`Client`] so that stays `Copy` — this is a heap string, and the
/// only thing that needs it is persistence, turning an entity into a record that
/// remembers whose character it is. An NPC has no account and no `Client`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Account(pub String);

/// Marks an item as script-placed decoration: a sign, a piece of furniture, an
/// ankh — the things a shard adds on top of the static art the client's map
/// already draws.
///
/// It sets the item apart from loose clutter: decoration never decays and cannot
/// be picked up (a town's fittings are not loot), and clearing decoration finds
/// its items by this. Placed through `op_decorate`; the client draws it as an
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
    pub closed: u16,
    /// The graphic drawn while open.
    pub open: u16,
    /// How far the door hops east/west when it swings open.
    pub offset_x: i16,
    /// How far it hops north/south.
    pub offset_y: i16,
    /// Whether the door is currently open.
    pub is_open: bool,
    /// The tick it auto-closes on when open; `0` when shut.
    pub close_at: u64,
}

/// Which spawn region put this mobile here — an index into the world's spawner
/// list.
///
/// The region counts its live creatures by this to know when to refill. A
/// creature dies and is despawned, the component goes with it, the count drops,
/// and the region spawns another. Absent on players and on script- or GM-spawned
/// mobiles, which no region maintains.
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

/// Which facet a mobile is on: 0 Felucca, 1 Trammel, and so on.
///
/// A mobile only ever interacts with others on the same facet — the world keeps
/// a separate map and interest grid per facet — so this is what selects which.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Facet(pub u8);

/// Marks an entity as driven by a person rather than by the server.
///
/// Carries the connection so the world can answer it, and the version so
/// encoders can ask what this particular client understands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Client {
    /// Which connection.
    pub connection: ConnectionId,
    /// What it claims to be. Every feature gate reads this.
    pub version: ClientVersion,
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
    pub level: u8,
    /// The tick the next pulse of damage lands.
    pub next_pulse: u64,
    /// Pulses left before the poison wears off.
    pub pulses_left: u8,
}

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

/// Which stats a stat-modifying effect shifts, and by how much.
///
/// The `kind` names *which* stats (Strength touches str, Bless all three); the
/// signed `offset` carries the magnitude and direction. Returns the delta for
/// `(strength, dexterity, intelligence)`. A debuff simply arrives with a negative
/// `offset` — so the same function undoes a buff by being called with the offset
/// negated, which is exactly how [`StatMod`] reversal works.
#[must_use]
pub fn stat_shift(kind: u8, offset: i16) -> (i16, i16, i16) {
    use effect::*;
    match kind {
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
pub fn is_debuff(kind: u8) -> bool {
    use effect::*;
    matches!(kind, WEAKEN | CLUMSY | FEEBLEMIND | CURSE)
}

/// One timed stat modifier: which effect, how much, and the tick it lifts.
///
/// The `offset` is signed and pre-distributed by [`stat_shift`] from the `kind`;
/// it is kept whole so expiry can reverse *exactly* what was applied, even if the
/// base stat changed underneath it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StatMod {
    /// Which effect — an [`effect`] kind (Strength..Curse).
    pub kind: u8,
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
    /// Which buff — an [`effect`] kind (`NIGHT_SIGHT`..`MAGIC_REFLECT`).
    pub kind: u8,
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

/// What a mobile is trained in: each skill it has, by id, as a value in tenths
/// (so 75.5 is stored as 755, and the skill cap is 1000).
///
/// Sparse on purpose — a mobile knows the handful of skills it has been given,
/// not all fifty-odd at zero. An id it has never trained reads as zero.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Skills {
    values: HashMap<u8, u16>,
    /// How the window trains each skill — `Up` unless the player set an arrow.
    /// Sparse like the values: an untouched skill trains up.
    locks: HashMap<u8, SkillLock>,
}

impl Skills {
    /// The value of `skill`, in tenths; zero if the mobile has never had it.
    pub fn get(&self, skill: u8) -> u16 {
        self.values.get(&skill).copied().unwrap_or(0)
    }

    /// Set `skill` to `value` tenths.
    pub fn set(&mut self, skill: u8, value: u16) {
        self.values.insert(skill, value);
    }

    /// How `skill` is set to train; `Up` unless the player moved its arrow.
    pub fn lock(&self, skill: u8) -> SkillLock {
        self.locks.get(&skill).copied().unwrap_or_default()
    }

    /// Set how `skill` trains — the up/down/lock arrow.
    pub fn set_lock(&mut self, skill: u8, lock: SkillLock) {
        self.locks.insert(skill, lock);
    }

    /// Every trained skill and its lock, for persistence — `(id, value, lock)`,
    /// in no particular order. A skill at zero with a moved arrow still counts,
    /// so a "down" lock the player set is not forgotten.
    pub fn entries(&self) -> impl Iterator<Item = (u8, u16, SkillLock)> + '_ {
        self.values
            .keys()
            .chain(self.locks.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(move |&id| (id, self.get(id), self.lock(id)))
    }
}

/// A spell in progress — the rooted cast delay of the "servuo" cast style. The
/// mobile is committed to `spell` and cannot walk until `complete_at`, the tick
/// the cast resolves; taking damage in the meantime disturbs it if the shard
/// runs with `spell_disturb`. The "sphere" style never sets this — it resolves a
/// cast as it is made.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Casting {
    /// The spell being cast, by id.
    pub spell: u16,
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
    pub sight: u8,
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

/// How a creature relates to the people around it — ServUO's `FightMode`,
/// folded to the three postures that matter: fauna that never fights, the
/// guard-dog that answers force with force, and the monster that starts it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Aggression {
    /// Never fights; runs from whoever hurts it. A deer.
    Passive,
    /// Fights only whoever attacked it first. A guard dog.
    Defensive,
    /// Attacks what it sees first. A monster — and the default, because every
    /// spawn before this knob existed behaved this way.
    #[default]
    Aggressive,
}

impl Aggression {
    /// The wire/config byte: 0 passive, 1 defensive, anything else aggressive.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        match bits {
            0 => Self::Passive,
            1 => Self::Defensive,
            _ => Self::Aggressive,
        }
    }

    /// The byte [`from_bits`](Self::from_bits) reads — what a save writes.
    #[must_use]
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Passive => 0,
            Self::Defensive => 1,
            Self::Aggressive => 2,
        }
    }
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
    pub const fn has(self, spell: u8) -> bool {
        spell < SPELL_COUNT && self.0 & (1u64 << spell) != 0
    }

    /// Add spell `n` (0-based); a no-op past the eighth circle.
    pub fn learn(&mut self, spell: u8) {
        if spell < SPELL_COUNT {
            self.0 |= 1u64 << spell;
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
pub const SPELLBOOK_GRAPHIC: u16 = 0x0EFA;

/// The corpse item graphic. A protocol special case: for item `0x2006` the
/// client reads the `Amount` field as the dead body id, so a corpse draws as the
/// creature it was. A corpse is a container (the loot window) that decays.
pub const CORPSE_GRAPHIC: u16 = 0x2006;

/// The gump the client opens for a corpse — the loot window, not a chest.
pub const CORPSE_GUMP: u16 = 0x0009;

/// The death shroud a fresh ghost wears — item `0x204E` on the outer-torso
/// layer, the grey robe a dead player rises in. ServUO's `deathShroud`.
pub const DEATH_SHROUD_GRAPHIC: u16 = 0x204E;

/// The ghost body a dead player wears — ServUO's `Race.GhostBody`. Female bodies
/// rise as `0x0193`, every other as `0x0192`; the client greys the world once it
/// draws the player in one.
#[must_use]
pub const fn ghost_body(body: u16) -> u16 {
    if body_is_female(body) {
        0x0193
    } else {
        0x0192
    }
}

/// The item graphic of the scroll for a Magery spell, `0-based` — the classic
/// run `0x1F2D..` (Reactive Armor, Clumsy, …), one per spell.
#[must_use]
pub const fn spell_scroll_graphic(spell: u8) -> u16 {
    0x1F2D + spell as u16
}

/// The Magery spell a scroll graphic teaches, if it is a Magery scroll.
#[must_use]
pub const fn scroll_spell(graphic: u16) -> Option<u8> {
    let base = 0x1F2D;
    if graphic >= base && graphic < base + SPELL_COUNT as u16 {
        Some((graphic - base) as u8)
    } else {
        None
    }
}

/// Whether a body knows what a door handle is. The reference rule is
/// "not an animal, not a sea creature"; without body-type tables yet, the
/// human bodies are the safe core of that set.
#[must_use]
pub const fn body_opens_doors(body: u16) -> bool {
    matches!(body, 0x0190..=0x0193 | 0x025D | 0x025E | 0x0260 | 0x0261)
}

/// The item graphic that draws a body as a mount on a rider, for the bodies
/// that can be ridden at all. The classic stable: horses, llama, ostards.
/// `None` is "not rideable", which is what double-click checks first.
#[must_use]
pub const fn mount_item_for(body: u16) -> Option<u16> {
    Some(match body {
        0x00C8 => 0x3E9F, // bay horse
        0x00CC => 0x3EA2, // dark brown horse
        0x00E2 => 0x3EA0, // grey horse
        0x00E4 => 0x3EA1, // tan horse
        0x00DC => 0x3EA6, // llama
        0x00DB => 0x3EA5, // forest ostard
        0x00D2 => 0x3EA3, // desert ostard
        0x00DA => 0x3EA4, // frenzied ostard
        _ => return None,
    })
}

/// The creature body a mount-item graphic stands for — the inverse of
/// [`mount_item_for`]. Persistence saves the worn mount item, not the ridden
/// creature (which lives only while ridden), so restoring a saved ride rebuilds
/// the creature from the item it was drawn as. `None` is "not a mount item".
#[must_use]
pub const fn mount_body_for(item_graphic: u16) -> Option<u16> {
    Some(match item_graphic {
        0x3E9F => 0x00C8, // bay horse
        0x3EA2 => 0x00CC, // dark brown horse
        0x3EA0 => 0x00E2, // grey horse
        0x3EA1 => 0x00E4, // tan horse
        0x3EA6 => 0x00DC, // llama
        0x3EA5 => 0x00DB, // forest ostard
        0x3EA3 => 0x00D2, // desert ostard
        0x3EA4 => 0x00DA, // frenzied ostard
        _ => return None,
    })
}

/// The default name a creature's body gives it — "a chicken", "a horse" —
/// shown on single-click and in the tooltip when a spawn did not name it.
///
/// Creature names are not in any client file the way item names are (those come
/// from tiledata); every emulator holds its own table, ServUO on each
/// `BaseCreature`, Sphere in its chardefs. This is the core default that pack
/// data overrides — the same "default in core, customise in pack" split item
/// names and spells have — so the common Britannia wildlife and dungeon monsters
/// read right out of the box and an unlisted body simply stays nameless rather
/// than wearing a wrong label. Body ids are ServUO's. Expand as needed.
#[must_use]
pub const fn creature_name(body: u16) -> Option<&'static str> {
    Some(match body {
        // Farm and forest animals.
        0x0006 => "a bird",
        0x00C9 => "a cat",
        0x00CA => "an alligator",
        0x00CB => "a pig",
        0x00CD => "a rabbit",
        0x00CF => "a sheep",
        0x00D0 => "a chicken",
        0x00D1 => "a goat",
        0x00D7 => "a giant rat",
        0x00D8 | 0x00E7 => "a cow",
        0x00D9 => "a dog",
        0x00DD => "a walrus",
        0x00EA => "a great hart",
        0x00ED => "a hind",
        0x00EE => "a rat",
        0x0097 => "a dolphin",
        0x0122 => "a boar",
        // Mounts — the stable of [`mount_item_for`].
        0x00C8 | 0x00CC | 0x00E2 | 0x00E4 => "a horse",
        0x00DC => "a llama",
        0x00DB => "a forest ostard",
        0x00D2 => "a desert ostard",
        0x00DA => "a frenzied ostard",
        0x0123 => "a pack horse",
        0x0124 => "a pack llama",
        // Common monsters.
        0x0003 => "a zombie",
        0x0004 => "a gargoyle",
        0x0011 => "an orc",
        0x0012 => "an ettin",
        0x0017 => "a dire wolf",
        0x0019 | 0x001B => "a grey wolf",
        0x001D => "a gorilla",
        0x0023 | 0x0024 => "a lizardman",
        0x002A => "a ratman",
        0x0030 => "a scorpion",
        0x0032 | 0x0038 => "a skeleton",
        0x0034 => "a snake",
        0x0035 | 0x0036 => "a troll",
        0x00A7 => "a brown bear",
        0x00D4 => "a grizzly bear",
        0x00D5 => "a polar bear",
        0x00E1 => "a timber wolf",
        // Undead.
        0x001A => "a spectre",
        0x0018 => "a lich",
        0x004F => "a lich lord",
        0x009A => "a mummy",
        0x0099 => "a ghoul",
        0x0039 => "a bone knight",
        0x0093 => "a skeletal knight",
        0x0094 => "a skeletal mage",
        // Dragons and reptiles.
        0x000C | 0x003B => "a dragon",
        0x003C | 0x003D => "a drake",
        0x003E => "a wyvern",
        0x00B4 | 0x0031 => "a white wyrm",
        0x0096 => "a sea serpent",
        0x0015 => "a giant serpent",
        0x00CE => "a lava lizard",
        // Daemons.
        0x0009 => "a daemon",
        0x004A => "an imp",
        // Elementals.
        0x000F => "a fire elemental",
        0x0010 => "a water elemental",
        0x000D => "an air elemental",
        0x000E => "an earth elemental",
        0x009F => "a blood elemental",
        0x00A3 => "a snow elemental",
        0x00A2 => "a poison elemental",
        // The rest of the common bestiary.
        0x0016 => "a gazer",
        0x001E => "a harpy",
        0x0049 => "a stone harpy",
        0x0001 => "an ogre",
        0x0053 => "an ogre lord",
        0x004B => "a cyclops",
        0x004C => "a titan",
        0x001C => "a giant spider",
        0x009D => "a giant black widow",
        0x002F => "a reaper",
        0x0033 => "a slime",
        0x0007 => "an orc captain",
        0x0046 => "a terathan warrior",
        _ => return None,
    })
}

/// A creature's base sound id — ServUO's `BaseSoundID`, keyed by body like
/// [`creature_name`]. Its attack, hurt and death sounds are fixed offsets from
/// it (`+2`, `+3`, `+4`), so an orc growls and a wolf howls instead of every
/// mobile making the human punch sound. `None` for a human body (which uses the
/// gendered death sounds) and for the passive fauna ServUO leaves silent (a
/// rabbit, a deer). Grow it alongside `creature_name` as bodies are added.
pub const fn creature_base_sound(body: u16) -> Option<u16> {
    Some(match body {
        // Farm and forest animals.
        0x0006 => 0x001B,          // bird
        0x00C9 => 0x0069,          // cat
        0x00CA => 0x0294,          // alligator
        0x00CB | 0x0122 => 0x00C4, // pig, boar
        0x00CF => 0x00D6,          // sheep
        0x00D0 => 0x006E,          // chicken
        0x00D1 => 0x0099,          // goat
        0x00D7 => 0x0188,          // giant rat
        0x00D8 | 0x00E7 => 0x0078, // cow
        0x00D9 => 0x0085,          // dog
        0x00DD => 0x00E0,          // walrus
        0x00EE => 0x00CC,          // rat
        0x0097 => 0x008A,          // dolphin
        // Mounts.
        0x00C8 | 0x00CC | 0x00E2 | 0x00E4 | 0x0123 => 0x00A8, // horse, pack horse
        0x00DC | 0x0124 => 0x03F3,                            // llama, pack llama
        0x00DB | 0x00D2 => 0x0270,                            // forest / desert ostard
        0x00DA => 0x0275,                                     // frenzied ostard
        // Monsters.
        0x0003 => 0x01D7,                            // zombie
        0x0004 => 0x0174,                            // gargoyle
        0x0011 => 0x045A,                            // orc
        0x0012 => 0x016F,                            // ettin
        0x0017 | 0x0019 | 0x001B | 0x00E1 => 0x00E5, // dire / grey / timber wolf
        0x001D => 0x009E,                            // gorilla
        0x0023 | 0x0024 => 0x01A1,                   // lizardman
        0x002A => 0x01B5,                            // ratman
        0x0030 => 0x018D,                            // scorpion
        0x0032 | 0x0038 => 0x048D,                   // skeleton
        0x0034 => 0x00DB,                            // snake
        0x0035 | 0x0036 => 0x01CD,                   // troll
        0x00A7 | 0x00D4 | 0x00D5 => 0x00A3,          // brown / grizzly / polar bear
        // Undead.
        0x001A | 0x0099 => 0x0482,          // spectre / wraith, ghoul
        0x0018 => 0x03E9,                   // lich
        0x004F => 0x019C,                   // lich lord
        0x009A => 0x01D7,                   // mummy
        0x0039 | 0x0093 | 0x0094 => 0x01C3, // bone / skeletal knight and mage
        // Dragons and reptiles — all share the dragon roar.
        0x000C | 0x003B | 0x003C | 0x003D | 0x003E | 0x00B4 | 0x0031 => 0x016A,
        0x0096 => 0x01BF, // sea serpent
        0x0015 => 0x00DB, // giant serpent
        0x00CE => 0x005A, // lava lizard
        // Daemons.
        0x0009 => 0x0165, // daemon
        0x004A => 0x01A6, // imp
        // Elementals.
        0x000F => 0x0346,          // fire
        0x0010 | 0x009F => 0x0116, // water, blood
        0x000D => 0x028F,          // air
        0x000E => 0x010C,          // earth
        0x00A3 | 0x00A2 => 0x0107, // snow, poison
        // The rest of the common bestiary.
        0x0016 => 0x0179,          // gazer
        0x001E | 0x0049 => 0x0192, // harpy, stone harpy
        0x0001 | 0x0053 => 0x01AB, // ogre, ogre lord
        0x004B => 0x025C,          // cyclops
        0x004C => 0x0261,          // titan
        0x001C | 0x009D => 0x0388, // giant spider, giant black widow
        0x002F => 0x01BA,          // reaper
        0x0033 => 0x01C8,          // slime
        0x0007 => 0x045A,          // orc captain (orc sound)
        0x0046 => 0x024D,          // terathan warrior
        _ => return None,
    })
}

/// Whether a body is female — the human death sound splits male from female,
/// ServUO's `m_Female`. The known female bodies: human, elf and gargoyle.
pub const fn body_is_female(body: u16) -> bool {
    matches!(body, 0x0191 | 0x025E | 0x02EF)
}

/// A creature that fights at distance — an archer's bow, a mage's bolt, a
/// dragon's breath, abstracted to what the tick needs: how far it reaches and
/// what kind of hurt it is. The damage amount is the creature's `MeleeDamage`;
/// a ranged creature caught in melee still bites with the same number.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RangedAttack {
    /// How far the attack reaches, in tiles.
    pub range: u8,
    /// The damage type's wire value (see [`DamageType::from_u8`]).
    pub kind: u8,
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
    /// The remaining route, as wire directions (0–7).
    pub steps: Vec<u8>,
    /// The next step to take.
    pub next: usize,
    /// Where the route was aimed; a quarry that strays invalidates it.
    pub goal: Point,
    /// When it was planned, for the repath clock.
    pub planned_at: u64,
}

/// Marks a mobile whose brain is a script's `onTick`, not the built-in one.
///
/// The richer path [`Brain`] leaves room for, now real: the tick's built-in
/// thinking skips a mobile carrying this, and the server calls its `onTick`
/// every tick instead — the per-mobile hook the scripting benchmark sized. A
/// script takes control of a mobile it spawned, then drives it from JavaScript;
/// the built-in `ai` stays out of its way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Scripted;

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
/// same container the bank holds for them everywhere. `next_greet` is the tick it
/// may next greet a passer-by, so it welcomes rather than natters.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Banker {
    /// The earliest tick it may greet someone again.
    pub next_greet: u64,
}

/// A townsperson's AI base — what makes a banker or a vendor *live* rather than
/// stand frozen. The shared part every profession reuses; the profession itself
/// is a marker beside it ([`Banker`], and vendors to come).
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

/// How hard a mobile hits in melee — the base a swing deals before the target's
/// armour takes its cut.
///
/// A mobile-level number for now: a creature's natural blow. Weapon-derived
/// damage is a later refinement that sets this from what the mobile wields.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MeleeDamage {
    /// The blow before resistance.
    pub amount: u16,
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

/// What kind of harm a blow does. Melee is [`Physical`](Self::Physical); a spell
/// picks its element.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum DamageType {
    /// A weapon or a fist.
    #[default]
    Physical,
    /// Fire.
    Fire,
    /// Cold.
    Cold,
    /// Poison.
    Poison,
    /// Energy.
    Energy,
}

impl DamageType {
    /// Read a damage type from a wire byte; anything unknown is physical.
    pub const fn from_u8(byte: u8) -> Self {
        match byte {
            1 => Self::Fire,
            2 => Self::Cold,
            3 => Self::Poison,
            4 => Self::Energy,
            _ => Self::Physical,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_entities::{Registry, SerialKind};
    use openshard_protocol::Direction;

    #[test]
    fn a_player_and_an_npc_differ_only_by_a_component() {
        // The claim the whole ECS rests on. If this ever needs a `kind` field,
        // something has gone wrong.
        let mut registry = Registry::new();
        let (player, _) = registry.spawn_with_serial(SerialKind::Mobile).unwrap();
        let (npc, _) = registry.spawn_with_serial(SerialKind::Mobile).unwrap();

        for entity in [player, npc] {
            registry.insert(entity, Position(Point::new(100, 100, 0)));
            registry.insert(entity, Body { id: 0x0190, hue: 0 });
        }
        registry.insert(
            player,
            Client {
                connection: ConnectionId::from_raw(1),
                version: ClientVersion::TOL,
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
        for body in 0u16..=0x0400 {
            if creature_base_sound(body).is_some() {
                assert!(
                    creature_name(body).is_some(),
                    "body {body:#06x} sounds like a creature but has no name"
                );
            }
        }
        // Spot-checks of the extended table (ServUO's BaseSoundID), and that a
        // human body is in neither — it falls back to the fists/gendered sounds.
        assert_eq!(creature_base_sound(0x001A), Some(0x0482)); // spectre / wraith
        assert_eq!(creature_base_sound(0x000C), Some(0x016A)); // dragon
        assert_eq!(creature_name(0x0009), Some("a daemon"));
        assert_eq!(
            creature_base_sound(0x0190),
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
