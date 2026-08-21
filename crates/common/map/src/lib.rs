//! Immutable, revisioned views of one world facet.
//!
//! One process loads a facet once, here, and everything downstream borrows what
//! it loaded. That is the whole of this crate today: no format, no patches, no
//! publisher. See `docs/map/new_map_representation/snapshot.md` for why a
//! revision that cannot yet change is still worth carrying.

use std::path::Path;

use openshard_protocol::world::Facet;
use openshard_uofiles::map::{Map, MapError};

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
/// borrowing a `&Map`; the *caller* is what passes [`MapSnapshot::map`], so the
/// ownership seam is visible at every crossing rather than coerced away.
#[derive(Debug)]
pub struct MapSnapshot {
    facet: Facet,
    revision: MapRevision,
    /// Owned outright, not shared: nothing hands a `Map` out any more. The
    /// shard thread was the last caller that wanted one, and it does not read
    /// the map at all now — see [`crate::MapSnapshot::map`] and the client's
    /// `link::connect`.
    map: Map,
}

impl MapSnapshot {
    /// Load `facet` from a client install as its first published version.
    ///
    /// The one door: `Map::load_facet` is not called by production code outside
    /// this crate, so every loaded facet arrives knowing which facet it is and
    /// which revision it stands at.
    pub fn load_facet(client_dir: impl AsRef<Path>, facet: Facet) -> Result<Self, MapError> {
        // `facet.0` unwrapped here and nowhere above: the file name and the
        // `FACET_SHAPES` subscript are the two places the number itself is the
        // value, and both are inside `uofiles`.
        Ok(Self::new(facet, Map::load_facet(client_dir, facet.0)?))
    }

    /// Make the first published version of `facet` from a map already in memory.
    ///
    /// For a scene built by hand and for a test; an install goes through
    /// [`MapSnapshot::load_facet`].
    #[must_use]
    pub fn new(facet: Facet, map: Map) -> Self {
        Self {
            facet,
            revision: MapRevision::INITIAL,
            map,
        }
    }

    /// The facet this snapshot describes.
    ///
    /// A `Map` names only a *size*, and two facets can share one; the number
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

    /// The immutable decoded map. Leaf readers continue to borrow a `Map`.
    #[must_use]
    pub fn map(&self) -> &Map {
        &self.map
    }
}

/// So a `MapTerrain` can be parameterised over the snapshot itself.
///
/// Not `Deref`: a snapshot is not a map with extra fields, and letting
/// `snapshot.land(..)` resolve would hide the very seam this phase adds. This
/// impl exists because `MapTerrain<M>` is already generic over `M: AsRef<Map>`,
/// and a holder is what that bound was always asking for.
impl AsRef<Map> for MapSnapshot {
    fn as_ref(&self) -> &Map {
        self.map()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_uofiles::map::{LandCell, LandTile};

    #[test]
    fn snapshots_keep_the_facet_that_resolved_an_ambiguous_size() {
        let map = || {
            Map::from_blocks(320, 256, |_, _| LandCell {
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
