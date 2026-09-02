//! Crop fields: the patches of farmland a shard keeps standing in cotton.
//!
//! ServUO's shape, one step over from [`spawner`](crate::spawner): a field is an
//! area, a crop it grows, and a ceiling on how many plants stand in it at once.
//! The core keeps it full — a picked plant stops counting immediately, and the
//! field puts another somewhere in its box after a delay. The *data* (which
//! patches of ground, how many plants) is the script pack's; the *machinery*
//! here is the engine's, driven by the tick.
//!
//! Upstream writes these as `<spawning>` blocks on the farm regions of
//! `Regions.xml` rather than as `Spawner` entries, which is why they are a file
//! of their own here: the converter that built `data/spawns.json` reads the
//! spawner map and has never seen an object spawn.
//!
//! Deterministic like the rest of the tick: which tile a plant lands on and
//! which of the crop's arts it wears both draw on the world's seeded
//! [`Rng`](openshard_state::rng::Rng), so a replay grows the same field.

use openshard_state::WorldTick;
use openshard_state::components::CropKind;

/// One field the tick keeps planted.
///
/// No id, unlike a [`Spawner`](crate::spawner::Spawner): a plant carries no tag
/// naming the field that grew it, because it is not saved and there is nothing
/// for a tag to survive. A field counts what stands *inside its box* instead,
/// which is exact as long as no two fields of one crop overlap — an invariant
/// `build.rs` refuses to let the data break.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CropField {
    /// ServUO's region name for it, for the log line and for reading the data.
    pub name:          String,
    /// What grows here.
    pub crop:          CropKind,
    /// The ground it covers.
    pub area:          crate::spawner::SpawnArea,
    /// How many plants it keeps standing.
    pub max_count:     u16,
    /// Ticks to wait after planting one before the next — the regrowth pace.
    pub respawn_delay: u64,
    /// The earliest tick the next plant may go in. Advanced past a planting so a
    /// picked-over field fills at its own pace rather than all at once.
    ///
    /// Live state rather than content, like [`Spawner::next_spawn`]: nothing
    /// writes it in the data, and it is not part of the field's identity.
    ///
    /// [`Spawner::next_spawn`]: crate::spawner::Spawner::next_spawn
    pub next_plant:    WorldTick,
}

impl CropField {
    /// A field that may plant immediately.
    #[must_use]
    pub fn new(
        name: String,
        crop: CropKind,
        area: crate::spawner::SpawnArea,
        max_count: u16,
        respawn_delay: u64,
    ) -> Self {
        Self {
            name,
            crop,
            area,
            max_count,
            respawn_delay,
            next_plant: WorldTick::ZERO,
        }
    }

    /// Whether this is the *same field* as `other` — everything the content
    /// declares, and nothing the engine has since done to it: the ground, the
    /// crop, the ceiling and the pace, but not the live timer. The identity a
    /// re-`populate` de-duplicates on, and the reason a boot that lays the world
    /// twice does not double a field's ceiling.
    #[must_use]
    pub fn is_the_same_field(&self, other: &Self) -> bool {
        self.area == other.area
            && self.crop == other.crop
            && self.max_count == other.max_count
            && self.respawn_delay == other.respawn_delay
    }
}

/// One facet's crop fields, and the admin verb that lays them.
///
/// The shape [`SpawnSet`](crate::spawner::SpawnSet) established: the verb rides
/// with the data rather than being spelled into a `match` in the server.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CropSet {
    /// What the staff menu's button sends: `populate:felucca`.
    pub verb:   String,
    /// The fields.
    pub fields: Vec<CropField>,
}

include!(concat!(env!("OUT_DIR"), "/crops.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_fields_grow_something_and_say_where() {
        let sets = shipped();
        assert_eq!(sets.len(), 1, "one facet ships fields");
        for field in &sets[0].fields {
            assert!(!field.name.is_empty(), "a field with no name reads as a bug");
            assert!(field.max_count > 0);
            assert!(field.respawn_delay > 0);
        }
    }
}
