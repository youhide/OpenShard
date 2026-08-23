//! One facet, whole: the map, and what the live world has laid over it.
//!
//! # One value, and what falls out of it
//!
//! Both ends of the wire held these two beside each other and carried them
//! together by hand — the shard as a `FacetState`'s snapshot and its overlay,
//! the client as its resources and a field on the frame's picture. Every reader
//! that wanted both took two arguments, and every one of them could be handed a
//! map and an overlay of *different facets* without anything noticing.
//!
//! Three properties come out of putting them in one value, and they are the
//! whole reason for it:
//!
//! - **A reader takes one value.** `world`, not a map and an overlay a caller
//!   remembered to carry together.
//! - **A bake cannot reach the live layer.** It takes [`World::snapshot`], which
//!   is the ground, the statics and a revision — and has no field to reach a
//!   door through. `docs/map/map_rebuild.md`'s invariant is a borrow rather than
//!   a rule anybody has to remember, and there is deliberately no accessor that
//!   hands out both halves at once.
//! - **The tile table stays outside.** Its scope is different: one install has
//!   one table and several facets, so what a graphic *is* is not a fact about
//!   this world. `openshard_movement::Footing::of` takes it as its own argument,
//!   and that is the whole of the asymmetry.
//!
//! # Why the base is optional and the live layer is not
//!
//! A shard with no client files is a real configuration — no floor, no walls,
//! every step allowed — and it is what `Footing`'s `map: Option<…>` already
//! said, one level up from where it belonged. A world with *nothing live on it*
//! is not a second kind of world, though: it is an empty overlay, which every
//! reader already handles because a facet starts that way.

use crate::overlay::Overlay;
use crate::snapshot::MapSnapshot;

/// One facet as its owner holds it.
///
/// **One per facet, and the thing a reader is handed.** The two layers move on
/// completely different clocks — the base changes when a patch is published, the
/// live layer as doors flip and ships sail — which is why they are two fields
/// and not one structure. What they share is a facet, and that is what this type
/// is: the statement that these two describe the same ground.
#[derive(Debug, Default)]
pub struct World {
    /// The ground and the statics, at some published revision — or `None` for a
    /// world with no map at all.
    base: Option<MapSnapshot>,
    /// What the live world has put on that ground. Empty is the ordinary state
    /// of a freshly loaded facet, not a missing one.
    live: Overlay,
}

impl World {
    /// A world standing on `base`, with nothing live on it yet.
    #[must_use]
    pub fn new(base: Option<MapSnapshot>) -> Self {
        Self {
            base,
            live: Overlay::default(),
        }
    }

    /// The ground, the statics and the revision they are at — **and no way from
    /// here to the live layer.**
    ///
    /// What a bake takes. Everything derived from a facet is stamped with the
    /// [`MapRevision`](crate::snapshot::MapRevision) it was built over and
    /// refuses itself on a mismatch; a bake that could also see a shut door
    /// would be recording an answer no revision describes.
    #[must_use]
    pub const fn snapshot(&self) -> Option<&MapSnapshot> {
        self.base.as_ref()
    }

    /// Put ground under this world, or take it away.
    ///
    /// A facet is built before its map is read on both ends — the shard inserts
    /// the facet and then loads it, and a test builds one and then gives it the
    /// scene it is about — so this is the seam that used to be a public field.
    pub fn set_base(&mut self, base: Option<MapSnapshot>) {
        self.base = base;
    }

    /// What the live world has laid over the ground, as every step decision
    /// reads it.
    #[must_use]
    pub const fn live(&self) -> &Overlay {
        &self.live
    }

    /// The live layer, to write.
    ///
    /// The owner of the indexes behind it is what comes here: the shard's facet
    /// projecting one tile at a time as a door flips, and the client replacing
    /// the whole picture when the shard sends it a new one.
    pub const fn live_mut(&mut self) -> &mut Overlay {
        &mut self.live
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{BlockExtent, Tile};
    use crate::map::LandCell;
    use crate::map::WorldMap;
    use crate::overlay::Cover;
    use openshard_protocol::world::Facet;
    use openshard_tiles::LandTileId;

    fn facet() -> MapSnapshot {
        MapSnapshot::new(
            Facet(0),
            WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
                tile: LandTileId(0),
                z: 0,
            }),
        )
    }

    /// A world with no client files behind it is a world, not a missing one —
    /// and the live layer works over it exactly the same.
    #[test]
    fn a_world_with_no_map_still_carries_what_was_put_on_it() {
        let mut world = World::new(None);
        assert!(world.snapshot().is_none());
        assert!(world.live().is_empty());

        world
            .live_mut()
            .set(Tile::new(3, 4), vec![Cover::blocking(0, 20)]);
        assert_eq!(world.live().at(Tile::new(3, 4)).len(), 1);
        assert!(world.snapshot().is_none(), "nothing gave it ground");
    }

    /// The two layers are independent: giving a world its ground does not
    /// disturb what is standing on it, which is what a facet loaded after the
    /// entities on it depends on.
    #[test]
    fn ground_arrives_without_clearing_what_is_on_it() {
        let mut world = World::new(None);
        world.live_mut().set(Tile::new(1, 1), vec![Cover::door(0, 20)]);

        world.set_base(Some(facet()));

        assert!(world.snapshot().is_some());
        assert!(
            world
                .live()
                .at(Tile::new(1, 1))
                .first()
                .is_some_and(|cover| cover.is_door()),
            "the door was on the facet before the facet had ground"
        );
    }
}
