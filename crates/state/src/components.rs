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

use openshard_entities::Serial;
use openshard_gateway::ConnectionId;
use openshard_movement::Walker;
use openshard_protocol::{AccessLevel, ClientVersion, Facing, Point};

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
