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

/// One creature a spawn region may put down. The fields a spawn needs beyond the
/// where — mirrors [`crate::tick::Command::SpawnMobile`] minus the position, which
/// the region supplies.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CreatureTemplate {
    /// The body graphic (a chicken, a skeleton).
    pub body: u16,
    /// Its hue.
    pub hue: u16,
    /// Starting and maximum hit points.
    pub hits: u16,
    /// Health-bar colour: the [`openshard_protocol::Notoriety`] wire value.
    pub notoriety: u8,
    /// Melee damage before the target's resistance.
    pub damage: u16,
    /// Physical resistance, a percentage.
    pub resistance: u8,
    /// Swing cadence in ticks; `0` derives it from dexterity.
    pub swing: u64,
    /// How far it notices a target; `0` for a placid animal.
    pub sight: u8,
    /// Whether it starts fights (2), answers them (1), or only runs (0).
    pub aggression: u8,
    /// Ticks between its beats while hunting; 0 takes the shard default.
    pub beat: u64,
    /// How far its ranged attack reaches, in tiles; 0 fights hand to hand.
    pub ranged: u8,
    /// The ranged attack's damage type wire value.
    pub ranged_kind: u8,
    /// Whether it drifts when idle.
    pub wander: bool,
    /// Trained combat skills, `(skill id, value in tenths)` — what makes a
    /// spawner's monsters roll to hit and scale damage like a player.
    pub skills: Vec<(u8, u16)>,
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
    pub facet: u8,
}

/// A region the tick keeps populated.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Spawner {
    /// Its stable id — the key it persists and is de-duplicated by.
    pub id: u32,
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
    pub next_spawn: u64,
}

impl Spawner {
    /// A region that starts able to spawn immediately.
    pub fn new(
        id: u32,
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
            next_spawn: 0,
        }
    }
}

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
            facet: 0,
        };
        let spawner = Spawner::new(1, area, Vec::new(), 5, 40);
        assert_eq!(spawner.next_spawn, 0, "ready from tick zero");
        assert_eq!(spawner.max_count, 5);
    }
}
