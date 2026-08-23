//! The ground one body's step is decided against.
//!
//! # Three things, and no fourth
//!
//! A step needs the map, what the live world has laid over it, and which way
//! the shut doors are being read. That is the whole of it, and this is those
//! three carried together because they travel together — through `find_path`,
//! through the detour rule, through every hop a long route is refined by.
//!
//! # Why this is not the trait it replaces
//!
//! There used to be a `Terrain` trait here with six implementors, five of which
//! were an *action over* a terrain rather than a terrain: a mask of what the
//! live world put in the way, a rectangle to stay inside, a memo table, the
//! absence of a map. Each was a kind of terrain because the seam was a trait,
//! and each one being a kind of terrain was then the argument for the seam
//! being a trait. See `docs/map/terrain_seam.md`.
//!
//! This is not that, and three properties are what keep it from becoming that:
//!
//! - **Nothing implements it.** It is a struct with three public fields, so
//!   there is no "another kind of footing" to write.
//! - **It has no methods.** Every rule over it is a free function that reads
//!   its fields — [`can_step`](crate::can_step), [`step_allowed`](crate::step_allowed),
//!   [`find_path`](crate::find_path). A decorator's failure mode was forwarding
//!   nine methods by hand and silently forgetting one; there is nothing here to
//!   forward.
//! - **A caller that wants less takes less.** Baking a navigation graph wants
//!   the bare map and takes a [`MapTerrain`]; a caller that wants to know what
//!   is in the way takes an [`Overlay`]. Only a *step* takes all three, because
//!   only a step needs all three.

use openshard_map::overlay::{Doors, Overlay};
use openshard_tiles::TileData;

use crate::ground::Ground;
use crate::terrain::MapTerrain;

/// The map, the live world over it, and how the doors are read.
///
/// `Copy` and built where it is asked: two pointers, a reference and a byte,
/// with nothing owned and nothing stored. Both ends of the wire already hold
/// the parts — a facet's [`Ground`] and the install's tile table — so this is a
/// view over what the caller has rather than a thing anybody keeps. It is built
/// through [`Footing::of`] wherever a facet's [`Ground`] is what the caller has,
/// and through [`Footing::guide`] where the caller means the bare map the coarse
/// navigation graph was baked over; the bare [`Footing::new`] is left for the one
/// caller that deliberately wants *less* than a facet — a test that is about the
/// overlay and nothing else.
#[derive(Clone, Copy, Debug)]
pub struct Footing<'a> {
    /// The map, or `None` for a world with no map at all: no floor, no walls,
    /// every step allowed and z never changing.
    ///
    /// What a shard with no client files runs, and what a test that is about
    /// the overlay and nothing else asks over. It used to be a type of its own
    /// (`OpenWorld`) and an implementor of the trait, which is how the absence
    /// of a map came to be a kind of map.
    pub map: Option<MapTerrain<'a>>,
    /// What the live world has put on that map.
    pub overlay: &'a Overlay,
    /// Whether a shut door is in the way, or whether this is a route being
    /// planned by somebody who will open it.
    pub doors: Doors,
}

impl<'a> Footing<'a> {
    /// The ground as `doors` reads it.
    #[must_use]
    pub const fn new(map: Option<MapTerrain<'a>>, overlay: &'a Overlay, doors: Doors) -> Self {
        Self { map, overlay, doors }
    }

    /// The ground one facet's [`Ground`] is, read as `doors` reads it.
    ///
    /// **The one composition**, and the reason [`Ground`] exists: the map, what
    /// is laid over it and the bake that says where a body may stand on it come
    /// out of a single value that says they are the same facet, instead of being
    /// three arguments a caller assembled and could assemble wrongly. Every
    /// production site that used to build a footing field by field goes through
    /// here.
    ///
    /// This used to take the bake as a fourth argument and *check* the pairing —
    /// it panicked on a facet with a map and no bake over it, because a facet
    /// like that decides its steps by re-deriving every column from `tiledata`,
    /// six times more expensively, with nothing at all saying so. There is
    /// nothing left to check: the two are one value, and neither can arrive
    /// without the other.
    ///
    /// The tile table is still its own argument, because its scope is different:
    /// one install has one table and several facets, so what a graphic *is* is
    /// not a fact about this world. That asymmetry is the whole of the
    /// signature.
    #[must_use]
    pub fn of(ground: &'a Ground, tiles: &'a TileData, doors: Doors) -> Self {
        Self {
            map: ground.terrain(tiles),
            overlay: ground.live(),
            doors,
        }
    }

    /// The same ground with nothing the live world has put on it — what a coarse
    /// navigation graph is guided and joined by.
    ///
    /// The graph is baked over the bare map (`docs/map/navigation_spans.md`'s
    /// N4), so the corridor it proposes has to be read over the bare map too: a
    /// door that happens to be shut, or a crate somebody dropped, must not be
    /// able to rewrite a route's *topology*. What the live world has to say is
    /// asked of each exact step instead — [`find_long_path`](crate::find_long_path)
    /// takes this and [`of`](Self::of) side by side, and only the second one
    /// approves a step.
    ///
    /// **Both ends of the wire want this value**, and each used to build it for
    /// itself out of an empty overlay it kept alive somewhere: the client's
    /// `world::guide` and the shard's `WorldState::guide`.
    ///
    /// The doors are read [`AsTheyStand`](Doors::AsTheyStand), which is a
    /// statement about nothing: an empty overlay has no door in it, so both
    /// readings of it are the same ground.
    #[must_use]
    pub fn guide(ground: &'a Ground, tiles: &'a TileData) -> Self {
        Self {
            map: ground.terrain(tiles),
            overlay: &NOTHING_PLACED,
            doors: Doors::AsTheyStand,
        }
    }

    /// The same ground, read the other way round.
    ///
    /// A route is planned through shut doors and then walked as they stand, and
    /// both readings are of one facet at one moment — so the pair is made from
    /// one another rather than assembled twice from the parts.
    #[must_use]
    pub const fn reading(self, doors: Doors) -> Self {
        Self { doors, ..self }
    }
}

/// Nothing placed, for a map-only reading to borrow.
///
/// One for the process rather than one per holder, because it is the *absence*
/// of a live layer: a second one would be a second name for the same emptiness,
/// and an empty overlay a caller keeps beside its map is a thing that can be
/// written to by mistake.
static NOTHING_PLACED: std::sync::LazyLock<Overlay> = std::sync::LazyLock::new(Overlay::default);
