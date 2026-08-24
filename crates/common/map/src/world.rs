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

use crate::chunk::{AssemblyError, Chunk};
use crate::overlay::Overlay;
use crate::patch::{Patch, PatchError, Undo};
use crate::snapshot::{MapRevision, MapSnapshot};

/// One facet as its owner holds it.
///
/// **One per facet, and the thing a reader is handed.** The two layers move on
/// completely different clocks — the base changes when a patch is published, the
/// live layer as doors flip and ships sail — which is why they are two fields
/// and not one structure. What they share is a facet, and that is what this type
/// is: the statement that these two describe the same ground.
#[derive(Debug)]
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

    /// Publish a patch to this world's base, and leave the live layer alone.
    ///
    /// **The live layer is untouched, and that is the model rather than an
    /// omission.** A patch is a change to the *ground*; a door standing in a
    /// doorway that a patch just deleted is still a door standing there, and
    /// what it means is a question for whoever put it there. The two layers move
    /// on different clocks, which is what this type is for.
    ///
    /// # Errors
    ///
    /// [`PatchError::NoGround`] — this facet has no map to patch at all.
    /// Otherwise [`MapSnapshot::publish`]'s, unchanged.
    pub fn publish(&mut self, patch: &Patch) -> Result<Undo, PatchError> {
        self.base.as_mut().ok_or(PatchError::NoGround)?.publish(patch)
    }

    /// Take back a publish that was never written down. See
    /// [`MapSnapshot::undo`], which this is.
    ///
    /// # Panics
    ///
    /// If the ground went away between the publish and the undo, which nothing
    /// can do while the caller holds the `&mut` that published.
    pub fn undo(&mut self, undo: &Undo) {
        self.base
            .as_mut()
            .expect("a world that published a patch a moment ago still has its ground")
            .undo(undo);
    }

    /// Take squares of ground somebody else cut, and leave the live layer alone.
    ///
    /// [`publish`](Self::publish)'s counterpart on the *other* end of the wire.
    /// A shard moves its ground by applying a patch it has the whole history for;
    /// a client is handed the chunks that changed and has no patch at all — see
    /// `docs/map/new_map_representation/to_the_client.md`, which argues why whole
    /// chunks travel rather than operations.
    ///
    /// The revision it hands back is the chunks' own, and it is the world's
    /// afterwards. **It is not checked against the revision this world was at**:
    /// what the chunks are a difference *from* is a question the caller asked the
    /// shard and this one cannot re-ask — see
    /// [`chunk::apply`](crate::chunk::apply), which draws the same line one level
    /// down.
    ///
    /// The live layer is untouched for [`publish`](Self::publish)'s reason,
    /// unchanged: a patch is a change to the ground, and what stands on it is
    /// whoever put it there's business.
    ///
    /// # Errors
    ///
    /// [`ChunksError`], one variant per way a set of chunks is not a change to
    /// this world. On either of them nothing has moved.
    ///
    /// # Panics
    ///
    /// If `chunks` is empty — [`chunk::apply`](crate::chunk::apply)'s panic, for
    /// its reason: a world that did not move is a case answered before this is
    /// called.
    pub fn take_chunks(&mut self, chunks: &[Chunk]) -> Result<MapRevision, ChunksError> {
        self.base
            .as_mut()
            .ok_or(ChunksError::NoGround)?
            .take_chunks(chunks)
            .map_err(ChunksError::Applying)
    }
}

/// Why a set of chunks is not a change to a world.
///
/// Both variants leave the world exactly where it was: the first never reaches
/// the map at all, and the second is [`chunk::apply`](crate::chunk::apply)
/// refusing before it hands anything back.
#[derive(Debug)]
#[non_exhaustive]
pub enum ChunksError {
    /// There is no map here for them to go into.
    ///
    /// A world with no base is a shard running with no client files, and a
    /// client that has not been given a facet yet — see this module's header.
    /// Chunks of a facet nobody holds are not a facet.
    NoGround,
    /// The chunks and the world do not describe the same ground.
    Applying(AssemblyError),
}

impl std::fmt::Display for ChunksError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoGround => f.write_str("there is no ground here to put chunks into"),
            Self::Applying(source) => write!(f, "the chunks are not of this world: {source}"),
        }
    }
}

impl std::error::Error for ChunksError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoGround => None,
            Self::Applying(source) => Some(source),
        }
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

    /// A chunk the other end of the wire published replaces the ground under
    /// what is standing on it, and nothing else.
    ///
    /// The live layer is the assertion that matters: a patch is a change to the
    /// ground, and a door in a doorway is still a door. It is the same rule
    /// [`World::publish`] keeps, arrived at from the client's side.
    #[test]
    fn chunks_from_the_wire_move_the_ground_and_not_what_is_on_it() {
        let mut world = World::new(Some(facet()));
        world.live_mut().set(Tile::new(1, 1), vec![Cover::door(0, 20)]);
        let was = world.snapshot().expect("it was given ground").revision();

        // The same one-block facet, one tile of it moved, cut at the next
        // revision — which is exactly what a shard would send after a `.setland`.
        let mut moved = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTileId(0),
            z: 0,
        });
        moved.set_land(
            3,
            4,
            LandCell {
                tile: LandTileId(9),
                z: 40,
            },
        );
        let published = MapSnapshot::restored(Facet(0), was.after(), moved);
        let chunk = crate::chunk::Chunk::of(&published, crate::chunk::ChunkCoord { x: 0, y: 0 })
            .expect("the one chunk this facet has");

        let now = world.take_chunks(&[chunk]).expect("a chunk of this facet");
        assert_eq!(now, was.after(), "the revision is the chunk's own");
        assert_eq!(
            world.snapshot().expect("it still has ground").map().land(3, 4),
            Some(LandCell {
                tile: LandTileId(9),
                z: 40
            }),
        );
        assert!(
            world
                .live()
                .at(Tile::new(1, 1))
                .first()
                .is_some_and(|cover| cover.is_door()),
            "the ground moved under the door and the door stayed"
        );
    }

    /// A world with no map has nothing for chunks to go into, and says so rather
    /// than growing one out of them.
    ///
    /// A facet built out of whatever arrived would be a facet whose extent came
    /// off the wire — `assemble` is the call that takes an extent and refuses a
    /// short set against it, and this is not that call.
    #[test]
    fn chunks_are_refused_by_a_world_with_no_ground() {
        let published = facet();
        let chunk = crate::chunk::Chunk::of(&published, crate::chunk::ChunkCoord { x: 0, y: 0 })
            .expect("the one chunk this facet has");
        let mut world = World::new(None);
        assert!(matches!(world.take_chunks(&[chunk]), Err(ChunksError::NoGround)));
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
