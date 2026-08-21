//! Immutable, revisioned views of one world facet.

use std::ops::Deref;
use std::sync::Arc;

use openshard_protocol::world::Facet;
use openshard_uofiles::map::Map;

/// Which published version of a facet a reader holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MapRevision(u64);

impl MapRevision {
    /// The first revision of a facet loaded from client files.
    pub const INITIAL: Self = Self(1);

    /// The value suitable for artifact metadata and diagnostics.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for MapRevision {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

/// One immutable version of one facet.
#[derive(Debug)]
pub struct MapSnapshot {
    facet: Facet,
    revision: MapRevision,
    map: Arc<Map>,
}

impl MapSnapshot {
    /// Make the first published version of `facet`.
    #[must_use]
    pub fn new(facet: Facet, map: Map) -> Self {
        Self {
            facet,
            revision: MapRevision::INITIAL,
            map: Arc::new(map),
        }
    }

    /// The facet this snapshot describes.
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

    /// Share the immutable map with a concurrent leaf reader.
    #[must_use]
    pub fn shared_map(&self) -> Arc<Map> {
        Arc::clone(&self.map)
    }
}

impl Deref for MapSnapshot {
    type Target = Map;

    fn deref(&self) -> &Self::Target {
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
}
