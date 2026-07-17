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

use openshard_entities::Serial;
use openshard_gateway::ConnectionId;
use openshard_movement::Walker;
use openshard_protocol::{ClientVersion, Facing, Point};

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
/// so decay is checked against [`World::ticks`](crate::World::ticks) and stays as
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
