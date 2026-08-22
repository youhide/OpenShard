//! What the live world lays over the map.
//!
//! # Why the map is not enough, said once
//!
//! [`MapTerrain`](crate::MapTerrain) reads the client's files — land and static
//! art — and nothing else. A door is an *entity*: the doorway it stands in is an
//! open gap in the statics by construction, and the leaf that closes it lives in
//! the shard's registry. A barrel is an entity. A ship is an entity, and it is
//! the one that can put ground where the map says there is none.
//!
//! Both ends of the wire had to say that, and until this module existed both
//! said it themselves: `openshard-state`'s `Obstructions` and `client/app`'s
//! `Clutter`, two indexes of the same fact that agreed only by resemblance —
//! each with its own blocker struct, its own span arithmetic and its own body
//! height. This is the one type they now both build, so a step refused here is
//! refused there for the same reason rather than for a similar one.
//!
//! # An overlay is not a terrain
//!
//! It answers three questions and none of them is "may I walk here":
//!
//! - **Is anything in the way of a body standing at this height?** — the span
//!   overlap, which is what lets a wall on an upper floor leave the ground floor
//!   open.
//! - **Is there somewhere to stand the map does not know about?** — a deck.
//!   The only *positive* thing here, and the reason this is not a bitmask.
//! - **Was it a door?** — because a route that stops at a door means "go and
//!   open it" and a route that stops at a crate means "there is no way through".
//!
//! Who *put* a cover here is not one of them. The server knows — a door is an
//! entity it can open, a plank belongs to a ship somebody is standing on — and
//! keeps that in its own indexes, because the client has no identity to offer:
//! a `GroundItem` is a position, a graphic, a hue and an amount. An owner field
//! here would be a hole one end fills with a lie. See
//! `docs/map/terrain_seam.md`'s node E.

use rustc_hash::FxHashMap;

use openshard_uofiles::tiledata::StaticTile;

use crate::terrain::PLAYER_HEIGHT;
use crate::walk::Tile;

/// Which of the two readings of the same ground a question is asked under.
///
/// A `bool` would do and is exactly what this must not be: the two are read at
/// call sites several crates apart, and "true" there says nothing about which
/// way round it is. Both ends had this distinction already — the server as
/// `LiveTerrain::through_doors`, the client as a private enum of this name —
/// which is two spellings of one idea and the argument for it living here.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Doors {
    /// Shut is shut: what a step is actually allowed by, and the only reading a
    /// step that reaches the wire may use.
    #[default]
    AsTheyStand,
    /// Every shut door stands open: what a *route* may be planned through, by a
    /// body that intends to open its way along it. Never a walkability answer —
    /// a mobile walked into a door on this reading is refused by the shard.
    AllOpen,
}

impl Doors {
    /// The reading a body plans under, given whether it can work a latch.
    ///
    /// The one place a `bool` legitimately becomes one of these: whether a
    /// creature opens doors is a fact about the creature, held as a flag on its
    /// brain, and this is the seam where that fact turns into a way of reading
    /// the ground. Everywhere else the enum is what travels.
    #[must_use]
    pub const fn for_opener(opens_doors: bool) -> Self {
        match opens_doors {
            true => Self::AllOpen,
            false => Self::AsTheyStand,
        }
    }
}

/// What one entry over one tile does to a body.
///
/// An enum and not a pair of flags, because the world already says the two are
/// exclusive: a ship's `Plank` carries `blocks` and is read through two filters
/// that partition it — the hull half stops a body, the deck half carries one —
/// and no placed item has ever been both.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoverKind {
    /// In the way.
    Blocks {
        /// A shut door: in the way now, and not in the way at all to a mobile
        /// that will open it. What [`Doors::AllOpen`] leaves out, and the whole
        /// of what "potentially passable" means here.
        door: bool,
    },
    /// Somewhere to stand that the map does not have — a deck over open water.
    ///
    /// **The one thing that can overrule the map's refusal.** Open water is not
    /// ground, so the map answers "nothing to stand on" — correctly, right up
    /// until a ship is moored there. An index that only ever subtracted could
    /// not say this, which is why boats were a structure of their own before
    /// this type existed.
    Stands,
}

/// One thing the live world has put on one tile.
///
/// No graphic, no serial, no entity — see the module header. What a reader
/// needs is where its body starts, how far up it reaches, and which of the two
/// things above it does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cover {
    /// The base z its body sits at.
    pub z: i8,
    /// How tall it is: its body spans `[z, z + height)`.
    ///
    /// Zero-height art still occupies its own tile — `tiledata.mul` gives
    /// plenty of impassable art a height of zero, and a flat span would overlap
    /// nothing and block nowhere, which reads exactly like the bug this type
    /// exists to fix. [`Cover::top`] is where that is fixed, once.
    pub height: u8,
    /// What it does to a body.
    pub kind: CoverKind,
}

impl Cover {
    /// Something in the way, at `z`, `height` tall.
    #[must_use]
    pub const fn blocking(z: i8, height: u8) -> Self {
        Self {
            z,
            height,
            kind: CoverKind::Blocks { door: false },
        }
    }

    /// A shut door — in the way, and openable.
    #[must_use]
    pub const fn door(z: i8, height: u8) -> Self {
        Self {
            z,
            height,
            kind: CoverKind::Blocks { door: true },
        }
    }

    /// A floor to stand on at `z + height`, carrying rather than stopping.
    #[must_use]
    pub const fn standing(z: i8, height: u8) -> Self {
        Self {
            z,
            height,
            kind: CoverKind::Stands,
        }
    }

    /// The cover a placed item lays over its own tile, from the client-file
    /// entry that says what its art is — or `None` where it lays none.
    ///
    /// **The one rule both ends of the wire call**, and the reason the two
    /// agree by construction rather than by resemblance. It used to be written
    /// twice: `world::tick::decor::place_decoration` filtered on
    /// `flags.is_blocking()` and took `tile.height`, and `client/app`'s
    /// `clutter::of` did the same three lines a crate away. Same predicate,
    /// same span, two places to change one of them.
    ///
    /// **Not a door**, whichever way round that is. Which leaves are doors is
    /// not a property of the tiledata — the shard knows because it made the
    /// entity, and the client knows from `client/render`'s ported door table —
    /// so the caller refines this with [`Cover::door`] where it knows better.
    #[must_use]
    pub fn of_static(tile: &StaticTile) -> Option<Self> {
        tile.flags.is_blocking().then(|| Self::blocking_at(tile.height))
    }

    /// A blocker `height` tall whose base is the caller's to fill in.
    const fn blocking_at(height: u8) -> Self {
        Self {
            z: 0,
            height,
            kind: CoverKind::Blocks { door: false },
        }
    }

    /// The same cover, based at `z`.
    ///
    /// [`of_static`](Self::of_static) reads a table, which knows a height and
    /// not a position; this is where the placement supplies the other half.
    #[must_use]
    pub const fn based_at(self, z: i8) -> Self {
        Self { z, ..self }
    }

    /// Whether this is a door somebody could open.
    #[must_use]
    pub const fn is_door(self) -> bool {
        matches!(self.kind, CoverKind::Blocks { door: true })
    }

    /// The bottom of its body.
    #[must_use]
    pub const fn bottom(self) -> i32 {
        self.z as i32
    }

    /// One past the top of its body. Never equal to [`bottom`](Self::bottom):
    /// see [`Cover::height`].
    #[must_use]
    pub const fn top(self) -> i32 {
        self.bottom() + if self.height == 0 { 1 } else { self.height as i32 }
    }

    /// Where a body standing on this has its feet.
    ///
    /// Not [`top`](Self::top): the `max(1)` there is about a body of zero
    /// height still occupying its tile, and a *surface* of zero height is at
    /// its own base — the deck is where the plank is, not one z above it.
    #[must_use]
    pub const fn surface(self) -> i32 {
        self.bottom() + self.height as i32
    }

    /// Whether a body whose feet are at `stand_z` has this in its way.
    ///
    /// The body spans `[stand_z, stand_z + PLAYER_HEIGHT)` and this spans
    /// `[bottom, top)`; they are in the way of each other when the two overlap.
    /// The z-span and not the tile, so a crate on a building's upper floor
    /// leaves the ground floor beneath it open.
    #[must_use]
    pub const fn meets(self, stand_z: i32) -> bool {
        self.bottom() < stand_z + PLAYER_HEIGHT && stand_z < self.top()
    }
}

/// Everything the live world has laid over one facet's map, by tile.
///
/// **One per facet, owned by whoever owns the facet, and read by every step
/// decision.** Not built per query: a search asks about hundreds of tiles for
/// one click, and a structure assembled for each of them would be the answer
/// bought a hundred times over. The server keeps this in step from its own
/// indexes as doors flip and ships move; the client rebuilds it whole whenever
/// the shard sends it a new picture of the ground.
#[derive(Clone, Default, Debug)]
pub struct Overlay {
    tiles: FxHashMap<Tile, Vec<Cover>>,
}

impl Overlay {
    /// Whether the live world has put anything anywhere on this facet.
    ///
    /// **The hot path's first question.** A length against zero, before any
    /// hash is computed — a shard with no doors, no decor and no ships asks
    /// this once per step instead of hashing a tile that cannot be there.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// How many tiles hold anything at all.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// Everything on `tile`.
    #[must_use]
    pub fn at(&self, tile: Tile) -> &[Cover] {
        self.tiles.get(&tile).map_or(&[], Vec::as_slice)
    }

    /// Replace everything on `tile`.
    ///
    /// **The only mutator, and deliberately whole-tile.** A cover here has no
    /// identity, so there is nothing to address a finer edit to; the owner that
    /// does have one recomputes its tile and hands the result over. That is
    /// what keeps this type honest for both ends — the client, which has no
    /// owners at all and sets a whole picture, and the server, whose indexes
    /// key by entity and project one tile at a time.
    ///
    /// An empty list removes the tile, so [`is_empty`](Self::is_empty) stays
    /// the question it claims to be.
    pub fn set(&mut self, tile: Tile, covers: Vec<Cover>) {
        match covers.is_empty() {
            true => {
                self.tiles.remove(&tile);
            }
            false => {
                self.tiles.insert(tile, covers);
            }
        }
    }

    /// Forget everything, keeping the allocation.
    ///
    /// What a whole-picture rebuild starts with — the client's, which throws the
    /// previous view away rather than diffing it.
    pub fn clear(&mut self) {
        self.tiles.clear();
    }

    /// The first thing in the way of a body standing at `stand_z` on `tile`,
    /// under this reading of the doors.
    ///
    /// What a step asks. `None` is "nothing here stops you", which is not the
    /// same as "you may stand here" — the map answers that.
    #[must_use]
    pub fn blocker_at(&self, tile: Tile, stand_z: i32, doors: Doors) -> Option<Cover> {
        self.at(tile)
            .iter()
            .copied()
            .find(|cover| cover.blocks_body(stand_z, doors))
    }

    /// The first thing in the way on `tile` at *any* height.
    ///
    /// Where the height genuinely does not enter it: a sight line, which a shut
    /// door stops wherever it hangs, and the question "is there a door on this
    /// tile" a body asks before deciding to open one.
    #[must_use]
    pub fn blocker_anywhere(&self, tile: Tile) -> Option<Cover> {
        self.at(tile)
            .iter()
            .copied()
            .find(|cover| matches!(cover.kind, CoverKind::Blocks { .. }))
    }

    /// The surface a body coming from `near_z` would stand on at `tile`, if the
    /// live world put one there.
    ///
    /// The nearest one to where the body already is, so stepping up onto a deck
    /// from a pier and stepping down onto it from a mast are the same rule.
    #[must_use]
    pub fn surface_at(&self, tile: Tile, near_z: i32) -> Option<i32> {
        self.at(tile)
            .iter()
            .filter(|cover| cover.kind == CoverKind::Stands)
            .map(|cover| cover.surface())
            .min_by_key(|surface| (surface - near_z).abs())
    }
}

impl Cover {
    /// Whether this stops a body standing at `stand_z`, under `doors`.
    ///
    /// Private, because the two halves are not separable: a door left open by
    /// the reading is not "a blocker that does not block", it is nothing at
    /// all, and a caller that saw it as the former would report a doorway as
    /// obstructed.
    const fn blocks_body(self, stand_z: i32, doors: Doors) -> bool {
        match self.kind {
            CoverKind::Blocks { door: true } if matches!(doors, Doors::AllOpen) => false,
            CoverKind::Blocks { .. } => self.meets(stand_z),
            CoverKind::Stands => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_uofiles::tiledata::TileFlags;

    const HERE: Tile = Tile::new(100, 100);

    /// The whole of what the z-span is for: a wall on an upper floor is not a
    /// sealed ground floor. Registering both used to be the only way to say it
    /// on the server and a separate arithmetic on the client.
    #[test]
    fn a_wall_upstairs_leaves_the_floor_below_open() {
        let mut overlay = Overlay::default();
        overlay.set(HERE, vec![Cover::blocking(20, 20)]);

        assert!(overlay.blocker_at(HERE, 0, Doors::AsTheyStand).is_none());
        assert!(overlay.blocker_at(HERE, 25, Doors::AsTheyStand).is_some());
        assert!(overlay.blocker_at(HERE, 60, Doors::AsTheyStand).is_none());
    }

    /// Impassable art with a tiledata height of zero still occupies its tile.
    /// A flat span would overlap nothing and block nowhere, which reads exactly
    /// like the bug this type exists to fix.
    #[test]
    fn zero_height_art_still_occupies_its_own_tile() {
        let mut overlay = Overlay::default();
        overlay.set(HERE, vec![Cover::blocking(0, 0)]);

        assert!(overlay.blocker_at(HERE, 0, Doors::AsTheyStand).is_some());
        // And nothing above it: a flat blocker is one z tall, not infinite.
        assert!(overlay.blocker_at(HERE, 1, Doors::AsTheyStand).is_none());
    }

    /// The two readings, on the same tile, differing only in the door.
    #[test]
    fn a_plan_walks_through_a_door_and_not_through_a_crate() {
        let mut overlay = Overlay::default();
        overlay.set(HERE, vec![Cover::door(0, 20)]);
        assert!(overlay.blocker_at(HERE, 0, Doors::AsTheyStand).is_some());
        assert!(overlay.blocker_at(HERE, 0, Doors::AllOpen).is_none());

        // A crate dragged into the doorway is still there once the door swings:
        // opening it does not move the crate.
        overlay.set(HERE, vec![Cover::door(0, 20), Cover::blocking(0, 12)]);
        assert!(overlay.blocker_at(HERE, 0, Doors::AllOpen).is_some());
    }

    /// A deck is somewhere to stand and not something in the way, and the same
    /// tile can hold both: a crate lashed to the deck.
    #[test]
    fn a_deck_carries_and_a_hull_stops() {
        let mut overlay = Overlay::default();
        overlay.set(HERE, vec![Cover::standing(-2, 5)]);

        assert_eq!(overlay.surface_at(HERE, 0), Some(3));
        assert!(
            overlay.blocker_at(HERE, 3, Doors::AsTheyStand).is_none(),
            "a body standing on the deck is not blocked by the deck"
        );

        overlay.set(HERE, vec![Cover::standing(-2, 5), Cover::blocking(3, 12)]);
        assert_eq!(overlay.surface_at(HERE, 0), Some(3), "the crate is not a surface");
        assert!(overlay.blocker_at(HERE, 3, Doors::AsTheyStand).is_some());
    }

    /// A gunwale at deck height seals neither the deck nor the water under the
    /// ship — the case the hull rule was written for, restated as a span.
    ///
    /// This is the one behaviour folding boats into this type changed: `Boats`
    /// asked whether the body's *feet* were inside the plank, and everything
    /// else asks whether the body's *span* meets it. The span test is the
    /// stricter of the two, so it is the one that had to be checked against the
    /// case the looser one was written for.
    #[test]
    fn a_hull_ends_where_its_own_deck_begins() {
        let mut overlay = Overlay::default();
        // A plank at z = -2, five tall: its deck is at 3, and a hull piece of
        // the same shape spans [-2, 3).
        overlay.set(HERE, vec![Cover::standing(-2, 5), Cover::blocking(-2, 5)]);

        assert!(
            overlay.blocker_at(HERE, 3, Doors::AsTheyStand).is_none(),
            "the hull sealed the deck standing on top of it"
        );
        assert!(
            overlay.blocker_at(HERE, -2, Doors::AsTheyStand).is_some(),
            "the water inside the hull is not somewhere to swim"
        );
    }

    /// The nearest deck to where the body already is: a mast's crow's nest and
    /// the deck under it are two surfaces on one tile, and which one a body
    /// lands on is which one it is coming from.
    #[test]
    fn the_nearest_deck_is_the_one_stepped_onto() {
        let mut overlay = Overlay::default();
        overlay.set(HERE, vec![Cover::standing(-2, 5), Cover::standing(40, 2)]);

        assert_eq!(overlay.surface_at(HERE, 0), Some(3));
        assert_eq!(overlay.surface_at(HERE, 50), Some(42));
    }

    /// Emptying a tile empties the overlay, so the hot path's first question
    /// stays the question it claims to be.
    #[test]
    fn a_tile_set_empty_leaves_nothing_behind() {
        let mut overlay = Overlay::default();
        assert!(overlay.is_empty());
        overlay.set(HERE, vec![Cover::blocking(0, 20)]);
        assert!(!overlay.is_empty());
        overlay.set(HERE, Vec::new());
        assert!(overlay.is_empty(), "an emptied tile is still hashed");
    }

    /// The rule both ends of the wire lay a placed item's cover with.
    ///
    /// This is the whole of the agreement `clutter.rs`'s header claims and that
    /// nothing checked before node E: the shard's `place_decoration` and the
    /// client's `clutter::of` both call [`Cover::of_static`], so a step refused
    /// at one end is refused at the other for the same reason rather than for a
    /// similar one. What is asserted here is the contract they share.
    #[test]
    fn a_placed_item_covers_its_tile_exactly_when_its_art_blocks() {
        let barrel = StaticTile {
            flags: TileFlags::new(TileFlags::BLOCK),
            height: 12,
            ..StaticTile::default()
        };
        assert_eq!(
            Cover::of_static(&barrel).map(|cover| cover.based_at(-3)),
            Some(Cover::blocking(-3, 12)),
            "the span is the art's own height, based where the item was placed"
        );

        let rug = StaticTile {
            flags: TileFlags::new(0),
            height: 0,
            ..StaticTile::default()
        };
        assert_eq!(
            Cover::of_static(&rug),
            None,
            "art that does not block covers nothing"
        );

        // Impassable art with a tiledata height of zero is common, and it still
        // occupies its tile — see `Cover::height`.
        let flat = StaticTile {
            flags: TileFlags::new(TileFlags::BLOCK),
            height: 0,
            ..StaticTile::default()
        };
        let flat = Cover::of_static(&flat).expect("it blocks").based_at(0);
        assert!(flat.meets(0));
        assert!(!flat.meets(1));
    }

    /// Sight and door-detection ask about the tile and not about a height.
    #[test]
    fn a_door_is_found_whatever_height_it_hangs_at() {
        let mut overlay = Overlay::default();
        overlay.set(HERE, vec![Cover::standing(-2, 5), Cover::door(80, 20)]);

        assert!(overlay.blocker_at(HERE, 0, Doors::AsTheyStand).is_none());
        assert!(
            overlay.blocker_anywhere(HERE).is_some_and(Cover::is_door),
            "a door three storeys up is still a door on this tile"
        );
    }
}
