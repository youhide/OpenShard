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
use openshard_map::overlay::{Body, Cover, Covers};
use openshard_tiles::StaticTile;

/// One tile of one boat, as the step check needs it.
///
/// Not a component list: this is the *answer*, precomputed at placement, and the
/// question is asked ten times a second per mobile. The multi is walked once
/// when the ship is put on the water and never again while it is still.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Plank {
    /// Which boat this tile belongs to.
    pub boat: EntityId,
    /// What this piece of the ship lays over its tile, based at the height the
    /// component sits at.
    ///
    /// **[`Covers`] and not a `(z, height, blocks)` triple.** The triple was a
    /// *second reading of the same tiledata row*: it split a component on
    /// `is_blocking()` alone — hull if it stops a body, deck if it does not —
    /// where [`Cover::of_static`] splits on `is_platform()`, which is ServUO's
    /// `(flags & ImpassableSurface) == TileFlag.Surface`
    /// (`Scripts/Services/Pathing/Movement.cs:211`) and what housing,
    /// decoration, the persistence reload and the client all read. On the
    /// shipped fleet the two disagree about **eighty** components, every one of
    /// them art that is neither a platform nor a blocker — a rope, a rudder,
    /// the tiller — which the ship's own reading turned into a floor two under
    /// its own deck. See `openshard-boats`'s `boat_art_survey`.
    ///
    /// A piece of art laying *both* halves is not a contradiction and is why
    /// this is a [`Covers`] rather than one [`Cover`]: a deck plank is a floor
    /// at its top **and** three units of solid wood under it, and it was the
    /// missing second half that let a body stand inside the planking.
    ///
    /// Private, and [`of_art`](Self::of_art) is the only way to fill it: the
    /// whole of this change is that there is one reading of a ship's art, and a
    /// public field is somewhere to put a second.
    covers: Covers,
}

impl Plank {
    /// One piece of a ship, read off the tile table.
    ///
    /// `z` is the ship's own z plus the component's `dz` — the placement's half
    /// of the answer, which the table cannot know.
    #[must_use]
    pub fn of_art(boat: EntityId, art: &StaticTile, z: i8) -> Self {
        Self {
            boat,
            covers: Cover::of_static(art).based_at(z),
        }
    }

    /// What this piece of the ship lays over its tile.
    ///
    /// Read outwards for the projection into the overlay — see
    /// [`covers_at`](crate::obstruct::covers_at) — and by the survey that
    /// prices this reading against the one it replaced.
    #[must_use]
    pub const fn covers(self) -> Covers {
        self.covers
    }

    /// Where a body standing here has its feet, if this piece of the ship is
    /// somewhere to stand at all.
    ///
    /// `None` for a hull, for a mast, and for the ropes that used to answer.
    #[must_use]
    pub fn surface(self) -> Option<i32> {
        self.covers.stands().map(Cover::surface)
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

    /// Whether anything of a ship closes `(x, y)` for a body standing at `z`.
    ///
    /// The same vertical-span rule the obstruction index uses: a gunwale at deck
    /// height does not seal the water beneath it. [`Cover::meets`] is that rule,
    /// borrowed rather than written out a second time — the arithmetic used to
    /// live here as well, and a `height` of zero had to be special-cased at both
    /// ends.
    ///
    /// **A ship's own deck answers here now**, which the name no longer
    /// pretends otherwise about: a plank three units thick is three units of
    /// solid wood with a floor on top, so a body in the water under it is
    /// stopped by the same span that carries the body above it.
    #[must_use]
    pub fn blocks_at(&self, x: u16, y: u16, z: i32) -> bool {
        self.at(x, y)
            .iter()
            .filter_map(|plank| plank.covers.blocks())
            .any(|cover| cover.meets(Body::new(z, 1)))
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
            .filter_map(|plank| plank.surface())
            .min_by_key(|surface| (surface - near_z).abs())
    }

    /// Whether a body standing at `(x, y, z)` has its feet on **`boat`'s** deck.
    ///
    /// The named half of [`deck_at`](Self::deck_at), and the two are not
    /// interchangeable even though the arithmetic under them is nearly the same.
    /// `deck_at` answers a body's question — *what am I standing on* — for which
    /// whose ship it is could not matter less, so it looks at every plank on the
    /// tile. This answers a ship's question — *is this one of mine* — and a ship
    /// that accepts the answer `deck_at` gives sails away with its neighbour's
    /// crew.
    ///
    /// No "nearest surface" here, because there is nothing to be near: the body
    /// is already at `z`, and standing on a plank means standing on its top.
    /// That is exactly what `deck_at(..) == Some(z)` tested, since a surface at
    /// `z` is the unique nearest one to `z`.
    #[must_use]
    pub fn carries(&self, boat: EntityId, x: u16, y: u16, z: i32) -> bool {
        self.at(x, y)
            .iter()
            .filter(|plank| plank.boat == boat)
            .any(|plank| plank.surface() == Some(z))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_entities::Registry;
    use openshard_tiles::{AnimId, StaticTile, TileFlags};

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

    /// One row of the tile table, named field by field.
    ///
    /// The fixture builds a `StaticTile` and reads it rather than asserting a
    /// cover directly, because what these tests are about is the *reading*: a
    /// plank is whatever [`Cover::of_static`] makes of the art, and a fixture
    /// that skipped it would be the second reading this type was just relieved
    /// of.
    fn art(flags: u64, height: u8) -> StaticTile {
        StaticTile {
            flags: TileFlags::new(flags),
            height,
            weight: 255,
            layer: 0,
            anim_id: AnimId(0),
            name: String::from("a fixture"),
        }
    }

    /// A deck plank: a platform three tall, so a body stands at `z + 3` and the
    /// three units of planking under it are solid.
    fn deck(boat: EntityId, z: i8) -> Plank {
        Plank::of_art(boat, &art(TileFlags::PLATFORM, 3), z)
    }

    /// A hull plank: impassable and ten tall, with no floor anywhere on it.
    fn hull(boat: EntityId, z: i8) -> Plank {
        Plank::of_art(boat, &art(TileFlags::WALL | TileFlags::BLOCK, 10), z)
    }

    #[test]
    fn an_empty_index_answers_before_it_hashes() {
        let boats = Boats::default();
        assert!(boats.is_empty());
        assert_eq!(boats.len(), 0);
        assert!(boats.at(10, 10).is_empty());
        assert!(!boats.blocks_at(10, 10, 0));
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
        assert!(
            !boats.blocks_at(10, 10, 5),
            "a body standing on the deck is in its own way"
        );
        assert!(
            boats.blocks_at(10, 10, 3),
            "and the planking it stands on is solid, so nothing stands inside it"
        );
        assert!(boats.blocks_at(11, 10, 5), "the hull does");
        assert_eq!(boats.deck_at(11, 10, 0), None, "and a hull is not a floor");
    }

    /// A gunwale standing at deck height does not seal the water under the ship,
    /// the same vertical-span rule an upper-storey wall follows.
    #[test]
    fn a_hull_closes_its_own_span_and_not_the_water_under_it() {
        let ship = an_entity();
        let mut boats = Boats::default();
        boats.moor(ship, [((11, 10), hull(ship, 20))]);

        assert!(boats.blocks_at(11, 10, 25));
        assert!(!boats.blocks_at(11, 10, 0), "the sea beneath is still the sea");
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

    /// **A deck is not a deck in general.** Two ships sharing a tile is the case
    /// the index already supports, and it is the sharpest statement of the
    /// difference: `deck_at` says "there is a floor here" and `carries` says
    /// whose it is. A ship that sails on the first answer takes the other's crew.
    #[test]
    fn a_deck_belongs_to_one_boat_and_carries_only_for_it() {
        let (first, second) = two_entities();
        let mut boats = Boats::default();
        boats.moor(first, [((10, 10), deck(first, 2))]);
        boats.moor(second, [((10, 10), deck(second, 9))]);

        // A body standing at z 5 is on the first ship's deck and nobody else's,
        // though `deck_at` is happy to call it a floor for either question.
        assert_eq!(
            boats.deck_at(10, 10, 5),
            Some(5),
            "the nearest floor is the lower deck"
        );
        assert!(boats.carries(first, 10, 10, 5));
        assert!(
            !boats.carries(second, 10, 10, 5),
            "the second ship claimed the first's crew"
        );

        // And the upper deck, the other way round.
        assert!(boats.carries(second, 10, 10, 12));
        assert!(!boats.carries(first, 10, 10, 12));

        // Between the two is not standing on either.
        assert!(!boats.carries(first, 10, 10, 8));
        assert!(!boats.carries(second, 10, 10, 8));
    }

    /// A hull is not a floor for its own ship either, which is what keeps a
    /// swimmer at the waterline off the manifest.
    #[test]
    fn a_hull_carries_nobody() {
        let ship = an_entity();
        let mut boats = Boats::default();
        boats.moor(ship, [((11, 10), hull(ship, 20))]);

        assert!(
            !boats.carries(ship, 11, 10, 30),
            "a gunwale is not somewhere to stand"
        );
        assert!(!boats.carries(ship, 11, 10, 20));
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
