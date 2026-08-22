//! Which facet a map is, and which published version of it a reader holds.
//!
//! One process loads a facet once — through an importer, which is the only kind
//! of thing that can mint revision 1 — and everything downstream borrows what it
//! loaded. See `docs/map/new_map_representation/snapshot.md` for how this began
//! as a revision that could not yet change.
//!
//! It can now: [`MapSnapshot::publish`] applies a [`Patch`] and moves the facet
//! to the revision that patch produces. What keeps a reader from ever seeing
//! half of one is the `&mut` on that call, and the doc there is the argument.

use openshard_protocol::world::Facet;

use crate::map::WorldMap;
use crate::patch::{Patch, PatchError};

/// Which published version of a facet a reader holds.
///
/// Constructed here or read back out of an artifact that recorded one — never
/// invented by a reader. A caller that could mint a revision could also mint
/// agreement with a snapshot it never saw, which is the one thing the field
/// exists to make impossible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MapRevision(u64);

impl MapRevision {
    /// The first revision of a facet loaded from client files.
    pub const INITIAL: Self = Self(1);

    /// The revision an artifact recorded, read back off disk.
    ///
    /// The only way to build one from a number, and deliberately named for its
    /// single caller: a bake's decoder, reconstructing the stamp it wrote so it
    /// can be compared against the snapshot in hand.
    #[must_use]
    pub const fn decoded(value: u64) -> Self {
        Self(value)
    }

    /// The revision that applying one patch to this one produces.
    ///
    /// The third way a revision comes into being, and the only one that is not
    /// a *reading*: a change to the world is what mints a number here, which is
    /// why [`crate::patch::Patch`] stores a parent and derives this rather than
    /// carrying both and risking disagreeing with itself.
    ///
    /// # Panics
    ///
    /// After eighteen quintillion publishes.
    #[must_use]
    pub const fn after(self) -> Self {
        Self(
            self.0
                .checked_add(1)
                .expect("a facet published fewer than 2^64 times"),
        )
    }

    /// The value to write into artifact metadata and diagnostics.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One immutable version of one facet.
///
/// It has one owner per process — `Resources` on the client, the facet's
/// terrain on the server — and is not itself reference counted. Leaf code keeps
/// borrowing a `&WorldMap`; the *caller* is what passes [`MapSnapshot::map`], so the
/// ownership seam is visible at every crossing rather than coerced away.
#[derive(Debug)]
pub struct MapSnapshot {
    facet: Facet,
    revision: MapRevision,
    /// Owned outright, not shared: nothing hands a `WorldMap` out any more. The
    /// shard thread was the last caller that wanted one, and it does not read
    /// the map at all now — see [`MapSnapshot::map`] and the client's
    /// `link::connect`.
    map: WorldMap,
}

impl MapSnapshot {
    /// Publish `map` as the first version of `facet`.
    ///
    /// What keeps "a facet was loaded" and "a facet has an identity and a
    /// revision" the same event. An importer calls this —
    /// `openshard_uofiles::map::load_facet` is the one that reads a UO install
    /// — and so does a test with a scene built by hand.
    ///
    /// **For a world that arrives without a revision**, which is what an import
    /// is. A world that arrives *with* one comes through
    /// [`MapSnapshot::restored`] instead, and between them they are the only
    /// two ways to make a snapshot at all.
    #[must_use]
    pub fn new(facet: Facet, map: WorldMap) -> Self {
        Self {
            facet,
            revision: MapRevision::INITIAL,
            map,
        }
    }

    /// Publish a facet a stored world already gave a revision to.
    ///
    /// [`MapSnapshot::new`]'s other half, and the only other way to make one.
    /// The difference is who decides the revision: `new` is for an importer
    /// minting a first one out of files that carried none, and this is for a
    /// reader of something that *recorded* one — `openshard_basemap` is the
    /// caller, reading a base set back.
    ///
    /// The distinction is the same one [`MapRevision::decoded`] makes, and for
    /// the same reason: a reader that minted its own revision could claim
    /// agreement with a snapshot it never saw. A base set read back is the same
    /// world at the same revision it was written at, and every bake stamped
    /// against that revision stays valid across the round trip — which is the
    /// point of writing one.
    #[must_use]
    pub fn restored(facet: Facet, revision: MapRevision, map: WorldMap) -> Self {
        Self { facet, revision, map }
    }

    /// The facet this snapshot describes.
    ///
    /// A `WorldMap` names only a *size*, and two facets can share one; the number
    /// that resolved the ambiguity at load time survives here instead of being
    /// thrown away.
    #[must_use]
    pub const fn facet(&self) -> Facet {
        self.facet
    }

    /// This snapshot's revision.
    #[must_use]
    pub const fn revision(&self) -> MapRevision {
        self.revision
    }

    /// The immutable decoded map. Leaf readers continue to borrow a `WorldMap`.
    #[must_use]
    pub fn map(&self) -> &WorldMap {
        &self.map
    }

    /// Apply a committed patch and move to the revision it produces.
    ///
    /// **The `&mut` is the atomicity.** `mechanics.md` asks that a new revision
    /// become visible between ticks and never during one, and that no reader
    /// ever see half a change. Both are the borrow checker's here: every reader
    /// holds a `&WorldMap` borrowed from this snapshot, and a `&mut self` cannot be
    /// taken while one of them is alive. There is no window to publish into,
    /// rather than a rule to remember.
    ///
    /// It changes only the tiles the ops name — no chunk is re-cut, nothing is
    /// re-hashed and no facet is rebuilt, which is `plan.md`'s "a publish never
    /// rebuilds a facet" taken literally. What *is* invalidated by it is every
    /// bake over [`crate::patch::Patch::touched_chunks`], and that is direction
    /// D's to notice.
    ///
    /// # Errors
    ///
    /// [`PatchError`] — the patch is for another facet, was made against a
    /// revision this snapshot has left, or disagrees with the world about what
    /// it is changing. On any of them the world is exactly as it was.
    pub fn publish(&mut self, patch: &Patch) -> Result<MapRevision, PatchError> {
        if patch.facet() != self.facet {
            return Err(PatchError::WrongFacet {
                wanted: self.facet,
                found: patch.facet(),
            });
        }
        if patch.parent() != self.revision {
            return Err(PatchError::Conflict {
                holding: self.revision,
                parent: patch.parent(),
            });
        }
        crate::patch::apply(&mut self.map, patch.ops())?;
        self.revision = patch.revision();
        Ok(self.revision)
    }
}

/// So a `MapTerrain` can be parameterised over the snapshot itself.
///
/// Not `Deref`: a snapshot is not a map with extra fields, and letting
/// `snapshot.land(..)` resolve would hide the very seam this phase adds. This
/// impl exists because `MapTerrain<M>` is already generic over `M: AsRef<WorldMap>`,
/// and a holder is what that bound was always asking for.
impl AsRef<WorldMap> for MapSnapshot {
    fn as_ref(&self) -> &WorldMap {
        self.map()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::BlockExtent;
    use crate::map::{LandCell, LandTile};

    #[test]
    fn snapshots_keep_the_facet_that_resolved_an_ambiguous_size() {
        let map = || {
            WorldMap::from_blocks(BlockExtent { wide: 320, down: 256 }, |_, _| LandCell {
                tile: LandTile(0),
                z: 0,
            })
        };
        let malas = MapSnapshot::new(Facet(3), map());
        let ter_mur = MapSnapshot::new(Facet(4), map());

        assert_eq!(
            (malas.map().width(), malas.map().height()),
            (ter_mur.map().width(), ter_mur.map().height())
        );
        assert_ne!(malas.facet(), ter_mur.facet());
        assert_eq!(malas.revision(), MapRevision::INITIAL);
    }

    #[test]
    fn a_decoded_revision_round_trips_through_its_number() {
        assert_eq!(
            MapRevision::decoded(MapRevision::INITIAL.get()),
            MapRevision::INITIAL
        );
    }
}
