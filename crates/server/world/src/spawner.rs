//! Spawn regions: the thing that keeps a patch of the world populated.
//!
//! Sphere's shape, and ServUO's: a region is an area, a set of creatures it may
//! hold, and a ceiling on how many. The core keeps it full — when a creature dies
//! or wanders off and the count drops below the ceiling, the region spawns
//! another after a delay. The *data* (which areas, which creatures) is the script
//! pack's; the *machinery* here is the engine's, driven by the tick, so a shard
//! stays alive without anything asking it to.
//!
//! It is deterministic like everything in the tick: the pick of creature and the
//! pick of tile both draw on the world's seeded [`Rng`](openshard_state::rng::Rng),
//! so a replay repopulates identically.

use openshard_protocol::mobile::Notoriety;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::{Aggression, DamageType, Facet, PhysicalResistance, RangedRange, Sight};
use openshard_state::{Skill, SpawnerId, WorldTick};

/// One creature a spawn region may put down. The fields a spawn needs beyond the
/// where — mirrors [`crate::tick::Command::SpawnMobile`] minus the position, which
/// the region supplies.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CreatureTemplate {
    /// The body graphic (a chicken, a skeleton).
    pub body: Graphic,
    /// Its hue.
    pub hue: Hue,
    /// Starting and maximum hit points.
    pub hits: u16,
    /// Health-bar colour: the [`openshard_protocol::Notoriety`] wire value.
    pub notoriety: Notoriety,
    /// Melee damage before the target's resistance.
    pub damage: u16,
    /// Physical resistance, a percentage.
    pub resistance: PhysicalResistance,
    /// How widely known it is — what its killer inherits.
    pub fame: i32,
    /// Which way it is known. **Negative is evil**, so killing it earns karma.
    pub karma: i32,
    /// Swing cadence in ticks; `0` derives it from dexterity.
    pub swing: u64,
    /// How far it notices a target; `0` for a placid animal.
    pub sight: Sight,
    /// Whether it starts fights (2), answers them (1), or only runs (0).
    pub aggression: Aggression,
    /// Ticks between its beats while hunting; 0 takes the shard default.
    pub beat: u64,
    /// Its optional ranged attack reach.
    pub ranged: Option<RangedRange>,
    /// The ranged attack's damage type.
    pub ranged_kind: DamageType,
    /// Whether it drifts when idle.
    pub wander: bool,
    /// Trained combat skills, `(skill id, value in tenths)` — what makes a
    /// spawner's monsters roll to hit and scale damage like a player.
    pub skills: Vec<(Skill, u16)>,
}

/// The box a region spawns within: a top-left tile, a size, and a facet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpawnArea {
    /// West edge.
    pub x: u16,
    /// North edge.
    pub y: u16,
    /// Width in tiles; a spawn lands somewhere in `x .. x + width`.
    pub width: u16,
    /// Height in tiles.
    pub height: u16,
    /// Which facet.
    pub facet: Facet,
}

/// A region the tick keeps populated.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Spawner {
    /// Its id — **its index in the world's spawner list**, assigned by
    /// [`World::register_spawner`](crate::World) and by nothing else. It is the
    /// key its creatures point at ([`SpawnedBy`]), the key the tick counts them
    /// by, and the key it persists under.
    ///
    /// Not what a re-`populate` de-duplicates on; that is
    /// [`Spawner::is_the_same_region`]. The value a caller passes to
    /// [`Spawner::new`] is a placeholder the world overwrites.
    ///
    /// [`SpawnedBy`]: openshard_state::components::SpawnedBy
    pub id: SpawnerId,
    /// Where it spawns.
    pub area: SpawnArea,
    /// The creatures it may put down; each spawn picks one at random.
    pub creatures: Vec<CreatureTemplate>,
    /// The most live creatures it keeps.
    pub max_count: u16,
    /// Ticks to wait after a spawn before the next one — the respawn pace.
    pub respawn_delay: u64,
    /// The earliest tick the next spawn may happen. Advanced past a spawn so a
    /// region refills at its own pace, not all at once. Persisted as the *seconds*
    /// still to wait, so a rare spawn's timer survives a restart (see the tick's
    /// `spawner_records`).
    pub next_spawn: WorldTick,
}

impl Spawner {
    /// Whether this is the *same region* as `other` — everything the content
    /// declares, and nothing the engine assigned: the box, the creatures, the
    /// ceiling and the pace, but not the id and not the live timer.
    ///
    /// This is the identity a re-`populate` de-duplicates on. The box alone is not
    /// it: Britannia's regions overlap, and two regions over one box with different
    /// creature lists are two regions, not one laid twice. See
    /// [`World::register_spawner`](crate::World).
    #[must_use]
    pub fn is_the_same_region(&self, other: &Self) -> bool {
        self.area == other.area
            && self.max_count == other.max_count
            && self.respawn_delay == other.respawn_delay
            && self.creatures == other.creatures
    }

    /// A region that starts able to spawn immediately.
    pub fn new(
        id: SpawnerId,
        area: SpawnArea,
        creatures: Vec<CreatureTemplate>,
        max_count: u16,
        respawn_delay: u64,
    ) -> Self {
        Self {
            id,
            area,
            creatures,
            max_count,
            respawn_delay,
            next_spawn: WorldTick::ZERO,
        }
    }
}

/// One facet's spawn regions, and the admin verb that lays them.
///
/// The shape [`RegionSet`](openshard_state::region::RegionSet) established: the
/// verb rides with the data rather than being spelled into a `match` in the
/// server, so adding a facet is a file and a row in [`crate::admin`]'s menu.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SpawnSet {
    /// What the staff menu's button sends: `populate:felucca`.
    pub verb: String,
    /// The regions, each with the placeholder id `register_spawner` overwrites.
    pub spawners: Vec<Spawner>,
}

include!(concat!(env!("OUT_DIR"), "/spawns.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_spawner_can_spawn_at_once() {
        let area = SpawnArea {
            x: 1,
            y: 2,
            width: 3,
            height: 3,
            facet: Facet(0),
        };
        let spawner = Spawner::new(SpawnerId(1), area, Vec::new(), 5, 40);
        assert_eq!(spawner.next_spawn, WorldTick::ZERO, "ready from tick zero");
        assert_eq!(spawner.max_count, 5);
    }
}
