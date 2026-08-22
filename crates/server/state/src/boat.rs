//! Where the ships are, and what standing on one means.
//!
//! # Why this is not `Obstructions`
//!
//! `docs/boats.md`'s B3, and the short version is that **the obstruction index
//! only ever subtracts**. A house's entry says *this tile is closed*, and that
//! is the whole vocabulary. A boat has to say two things at once: the hull is
//! closed, **and the deck is somewhere to stand — at a height, over water that
//! is otherwise not ground at all**. A house never had to add a floor, because
//! its floors sit on land the map already calls walkable. Teaching
//! `Obstructions` to add ground would make it a different structure with a
//! different name.
//!
//! The performance argument points the same way and is the weaker one: moving an
//! N-tile footprint through a `HashMap<(u16, u16), Vec<Obstacle>>` with no
//! translate and no bulk write is 2N hashed vector operations per move, for
//! ever, against a structure whose stated justification is that its contents do
//! not move.
//!
//! # The hot path
//!
//! [`LiveTerrain`](crate::LiveTerrain) consults this on **every step by every
//! mobile**, and its diagonal rule re-enters twice more. So the empty case —
//! which is every facet on every shard that has no boats, and every tile on
//! every facet that does — is [`Boats::is_empty`], a single integer comparison
//! against a length, checked before any hash is computed.
//!
//! That is the shape B3 asked for. What it also asked for is a **measurement
//! rather than an assurance**, and `obstruct`'s `boat_step_cost` is it — an
//! ignored test that prints rather than asserts, because a wall-clock threshold
//! in a suite is a flake generator. Release, 100,000 steps: **1.5ms with no
//! boats and 5.5ms with one moored**, 15ns against 55ns a step. Its own doc
//! comment says why the ratio is the least flattering way to state that and
//! what would make it smaller if it ever needed to be.

use std::collections::HashMap;

use openshard_entities::EntityId;
use openshard_movement::Cover;

/// One tile of one boat, as the step check needs it.
///
/// Not a component list: this is the *answer*, precomputed at placement, and the
/// question is asked ten times a second per mobile. The multi is walked once
/// when the ship is put on the water and never again while it is still.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Plank {
    /// Which boat this tile belongs to.
    pub boat: EntityId,
    /// The z the component sits at — the ship's own z plus the component's `dz`.
    pub z: i8,
    /// What it stands on top of: `z + height` is where a body's feet go.
    pub height: u8,
    /// Whether it stops a body rather than carrying one. A hull blocks; a deck
    /// plank is a floor.
    pub blocks: bool,
}

impl Plank {
    /// Where a body standing on this tile has its feet.
    #[must_use]
    pub const fn surface(self) -> i32 {
        self.z as i32 + self.height as i32
    }

    /// This plank as the overlay states it.
    ///
    /// The projection [`FacetState`](crate::FacetState) maintains. `blocks` is
    /// what picks the arm, and it partitions: a hull stops a body, a deck
    /// carries one, and no plank has ever been both — which is the whole
    /// argument for the overlay's `CoverKind` being an enum rather than a pair
    /// of flags.
    #[must_use]
    pub const fn cover(self) -> Cover {
        match self.blocks {
            true => Cover::blocking(self.z, self.height),
            false => Cover::standing(self.z, self.height),
        }
    }
}

/// Every boat on one facet, indexed by the tiles they cover.
///
/// See the module header for why this is its own structure and not an entry in
/// [`Obstructions`](crate::Obstructions).
#[derive(Clone, Default, Debug)]
pub struct Boats {
    tiles: HashMap<(u16, u16), Vec<Plank>>,
    /// Which tiles each boat put down, so it can be lifted off again without
    /// re-deriving its shape. The reverse index `Obstructions` never had, and
    /// the reason a boat can afford one: there are a handful of ships and
    /// thousands of doors.
    covered: HashMap<EntityId, Vec<(u16, u16)>>,
}

impl Boats {
    /// Whether there is any boat at all on this facet.
    ///
    /// **The hot path's first question.** A length against zero, before any hash
    /// is computed — see the module header.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// How many boats are moored here.
    #[must_use]
    pub fn len(&self) -> usize {
        self.covered.len()
    }

    /// Put a boat's tiles down. Replaces whatever that boat had before, so a
    /// move is one call and never leaves half a hull behind.
    pub fn moor(&mut self, boat: EntityId, planks: impl IntoIterator<Item = ((u16, u16), Plank)>) {
        self.cast_off(boat);
        let mut covered = Vec::new();
        for (tile, plank) in planks {
            self.tiles.entry(tile).or_default().push(plank);
            covered.push(tile);
        }
        covered.sort_unstable();
        covered.dedup();
        self.covered.insert(boat, covered);
    }

    /// Take a boat's tiles back out.
    pub fn cast_off(&mut self, boat: EntityId) {
        let Some(tiles) = self.covered.remove(&boat) else {
            return;
        };
        for tile in tiles {
            let Some(planks) = self.tiles.get_mut(&tile) else {
                continue;
            };
            planks.retain(|plank| plank.boat != boat);
            if planks.is_empty() {
                self.tiles.remove(&tile);
            }
        }
    }

    /// Every tile any boat covers.
    pub fn tiles(&self) -> impl Iterator<Item = (u16, u16)> + '_ {
        self.tiles.keys().copied()
    }

    /// What is at `(x, y)`, if anything.
    #[must_use]
    pub fn at(&self, x: u16, y: u16) -> &[Plank] {
        self.tiles.get(&(x, y)).map_or(&[], Vec::as_slice)
    }

    /// Which tiles a boat is standing on.
    ///
    /// The reverse index read outwards rather than only used by
    /// [`cast_off`](Self::cast_off) — a move needs to ask *who is aboard*, and
    /// the answer is whoever is standing on one of these. Deriving the shape a
    /// second time from the multi would work and would be a second place for it
    /// to be wrong; this is the shape the world is actually indexed by.
    #[must_use]
    pub fn covered_by(&self, boat: EntityId) -> &[(u16, u16)] {
        self.covered.get(&boat).map_or(&[], Vec::as_slice)
    }

    /// The boat covering `(x, y)`, if one does.
    #[must_use]
    pub fn boat_at(&self, x: u16, y: u16) -> Option<EntityId> {
        self.at(x, y).first().map(|plank| plank.boat)
    }

    /// Whether a hull closes `(x, y)` for a body standing at `z`.
    ///
    /// The same vertical-span rule the obstruction index uses: a gunwale at deck
    /// height does not seal the water beneath it.
    #[must_use]
    pub fn hull_blocks(&self, x: u16, y: u16, z: i32) -> bool {
        self.at(x, y)
            .iter()
            .filter(|plank| plank.blocks)
            .any(|plank| z >= plank.z as i32 && z < plank.surface().max(plank.z as i32 + 1))
    }

    /// The deck a body would stand on at `(x, y)`, coming from `near_z`.
    ///
    /// The **positive** half, and the one no existing index can express: over
    /// open water the map says there is nothing to stand on, and this says there
    /// is. The nearest surface to where the body already is, so stepping up onto
    /// a deck from a pier and stepping down onto it from a mast are the same
    /// rule.
    #[must_use]
    pub fn deck_at(&self, x: u16, y: u16, near_z: i32) -> Option<i32> {
        self.at(x, y)
            .iter()
            .filter(|plank| !plank.blocks)
            .map(|plank| plank.surface())
            .min_by_key(|surface| (surface - near_z).abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_entities::Registry;

    fn an_entity() -> EntityId {
        Registry::default().spawn()
    }

    /// Two distinct ids, which two calls to [`an_entity`] would *not* give: a
    /// fresh registry starts counting at the same place, so each would hand back
    /// the same id and a two-boat test would silently be a one-boat test.
    fn two_entities() -> (EntityId, EntityId) {
        let mut registry = Registry::default();
        (registry.spawn(), registry.spawn())
    }

    fn deck(boat: EntityId, z: i8) -> Plank {
        Plank {
            boat,
            z,
            height: 3,
            blocks: false,
        }
    }

    fn hull(boat: EntityId, z: i8) -> Plank {
        Plank {
            boat,
            z,
            height: 10,
            blocks: true,
        }
    }

    #[test]
    fn an_empty_index_answers_before_it_hashes() {
        let boats = Boats::default();
        assert!(boats.is_empty());
        assert_eq!(boats.len(), 0);
        assert!(boats.at(10, 10).is_empty());
        assert!(!boats.hull_blocks(10, 10, 0));
        assert_eq!(boats.deck_at(10, 10, 0), None);
    }

    /// The whole point of the structure: over water the map has nothing to stand
    /// on, and this says there is a floor at a height.
    #[test]
    fn a_deck_is_somewhere_to_stand_and_a_hull_is_not() {
        let ship = an_entity();
        let mut boats = Boats::default();
        boats.moor(ship, [((10, 10), deck(ship, 2)), ((11, 10), hull(ship, 2))]);

        assert_eq!(boats.deck_at(10, 10, 0), Some(5), "the deck plank's top");
        assert!(!boats.hull_blocks(10, 10, 5), "a deck does not block");
        assert!(boats.hull_blocks(11, 10, 5), "the hull does");
        assert_eq!(boats.deck_at(11, 10, 0), None, "and a hull is not a floor");
    }

    /// A gunwale standing at deck height does not seal the water under the ship,
    /// the same vertical-span rule an upper-storey wall follows.
    #[test]
    fn a_hull_closes_its_own_span_and_not_the_water_under_it() {
        let ship = an_entity();
        let mut boats = Boats::default();
        boats.moor(ship, [((11, 10), hull(ship, 20))]);

        assert!(boats.hull_blocks(11, 10, 25));
        assert!(!boats.hull_blocks(11, 10, 0), "the sea beneath is still the sea");
    }

    /// Mooring again replaces: a move is one call, and half a hull left at the
    /// old berth is the failure that would make.
    #[test]
    fn mooring_a_boat_again_leaves_nothing_at_the_old_berth() {
        let ship = an_entity();
        let mut boats = Boats::default();
        boats.moor(ship, [((10, 10), deck(ship, 2))]);
        boats.moor(ship, [((20, 20), deck(ship, 2))]);

        assert!(boats.at(10, 10).is_empty(), "the old berth still has a deck");
        assert_eq!(boats.deck_at(20, 20, 0), Some(5));
        assert_eq!(boats.len(), 1, "and it is one ship, not two");
    }

    /// Two ships side by side are two entries, and taking one away leaves the
    /// other. The reverse index is what makes that cheap and what makes it
    /// correct.
    #[test]
    fn casting_off_one_boat_leaves_the_other() {
        let (first, second) = two_entities();
        let mut boats = Boats::default();
        boats.moor(first, [((10, 10), deck(first, 2))]);
        boats.moor(second, [((10, 10), deck(second, 9)), ((11, 10), deck(second, 9))]);

        boats.cast_off(first);
        assert_eq!(boats.len(), 1);
        assert_eq!(boats.deck_at(10, 10, 0), Some(12), "the second ship's deck");
        assert_eq!(boats.boat_at(11, 10), Some(second));

        boats.cast_off(second);
        assert!(boats.is_empty(), "the tile entry outlived both ships");
    }

    /// The nearest surface to where the body already is, so a step down onto a
    /// deck and a step up onto it read the same rule.
    #[test]
    fn a_body_lands_on_the_deck_nearest_its_own_height() {
        let ship = an_entity();
        let mut boats = Boats::default();
        boats.moor(ship, [((10, 10), deck(ship, 2)), ((10, 10), deck(ship, 40))]);

        assert_eq!(boats.deck_at(10, 10, 0), Some(5), "the main deck from the water");
        assert_eq!(boats.deck_at(10, 10, 50), Some(43), "the crow's nest from above");
    }
}
