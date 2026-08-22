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

use crate::overlay::{Doors, Overlay};
use crate::terrain::MapTerrain;

/// The map, the live world over it, and how the doors are read.
///
/// `Copy` and built where it is asked: two pointers, a reference and a byte,
/// with nothing owned and nothing stored. Both ends of the wire already hold
/// the parts — the shard as a facet's snapshot, its overlay and the shard's
/// tile table; the client as its resources and its view — so this is a view
/// over what the caller has rather than a thing anybody keeps.
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
