//! One facet as something that walks holds it: the world, and where a body may
//! stand on it.
//!
//! # The pair that used to be two fields
//!
//! [`SpanIndex`] is a projection of a [`World`]'s base — the ground and the
//! statics, and deliberately not the live layer over them. A world that moves
//! without its bake moving is a shard deciding steps by the heights of a map it
//! no longer holds, so the two have to travel together; until now they did it by
//! agreement. Both ends of the wire held them side by side and said so in a
//! comment — the shard's `FacetState`, the client's `Resources` — and
//! [`Footing::of`](crate::Footing::of) *checked* the pairing at the question,
//! with a panic for the facet that had a map and no bake over it.
//!
//! This is that agreement made into a value. The base is written by three
//! functions here and by nothing else, each of which writes the bake in the same
//! statement, so "a facet with a map and no span bake over it" is a state
//! nothing can spell rather than a state something notices.
//!
//! # Why it is here and not in [`World`]
//!
//! Because `openshard_map` is underneath this crate, and **where a body may
//! stand is a movement rule**: the bake reads [`MAX_STEP_UP`](crate::MAX_STEP_UP)
//! and [`PLAYER_HEIGHT`](crate::PLAYER_HEIGHT), and pushing those down would
//! make the crate that holds the world decide how tall a person is — which is
//! the move `realtime_map.md`'s R2 refused when `Cover::meets` asked for it, and
//! answered with a `Body` argument instead.
//!
//! So the layering is honoured by *wrapping* rather than by moving: a `World` is
//! two layers of one facet, and a `Ground` is that world plus the movement rule
//! baked over it. Nothing hands the inner world back out — a reader that could
//! take the world alone is a reader that could forget the bake again, which is
//! the whole of what this ends.
//!
//! # The tile table stays outside, still
//!
//! One install has one table and several facets, so what a graphic *is* is not a
//! fact about this world — [`World`]'s own doc draws that line and it is
//! unchanged here. The consequence is the one asymmetry in this file: the bake
//! is a statement about the world *and* the table, so the table is an argument
//! to every function that writes it, and a table that arrives after the ground
//! did needs [`Ground::rebake`].

use openshard_map::chunk::{
    Chunk,
    ChunkCoord,
};
use openshard_map::overlay::Overlay;
use openshard_map::patch::{
    Patch,
    PatchError,
    Undo,
};
use openshard_map::snapshot::{
    MapRevision,
    MapSnapshot,
};
use openshard_map::world::{
    ChunksError,
    World,
};
use openshard_tiles::TileData;

use crate::spans::SpanIndex;
use crate::terrain::MapTerrain;

/// One facet's world, and where a body may stand on it.
///
/// **The invariant, in one line:** [`spans`](Self::spans) is `Some` exactly when
/// the world has a base, and it is a bake of *that* base. Both fields are
/// private and every function that writes either — [`new`](Self::new),
/// [`set_base`](Self::set_base), [`rebake`](Self::rebake),
/// [`publish`](Self::publish) and [`undo`](Self::undo) — writes both, so there
/// is no sequence of calls that separates them.
#[derive(Debug)]
pub struct Ground {
    /// The two lower layers and what the live world has laid over them.
    ///
    /// Private, and never handed out whole: every reader of it wants one half
    /// (the snapshot for a bake, the overlay for what is in the way) or wants
    /// the pair *with* the span index, which is [`terrain`](Self::terrain) and
    /// [`Footing::of`](crate::Footing::of).
    world: World,
    /// Where a body may stand on [`world`](Self::world)'s base, baked once.
    ///
    /// A projection of the base alone — a door, a crate and a house floor are
    /// invisible in it by construction — which is why the live layer moving does
    /// not touch it and the base moving always does.
    spans: Option<SpanIndex>,
}

impl Ground {
    /// A facet standing on `base`, with its bake taken and nothing live on it
    /// yet.
    ///
    /// `None` is a world with no map at all: no floor, no walls, every step
    /// allowed. It is what a shard with no client files runs, and it has nothing
    /// to bake.
    #[must_use]
    pub fn new(base: Option<MapSnapshot>, tiles: &TileData) -> Self {
        Self {
            spans: base.as_ref().map(|base| SpanIndex::build(base.map(), tiles)),
            world: World::new(base),
        }
    }

    /// Put ground under this facet, or take it away — and rebake in the same
    /// statement.
    ///
    /// A facet is built before its map is read on both ends: the shard inserts
    /// the facet and then loads it, and a test builds one and then gives it the
    /// scene it is about. This is the seam that arrival goes through, and the
    /// reason it takes the tile table is that the bake is a statement about
    /// both.
    pub fn set_base(&mut self, base: Option<MapSnapshot>, tiles: &TileData) {
        self.spans = base.as_ref().map(|base| SpanIndex::build(base.map(), tiles));
        self.world.set_base(base);
    }

    /// Bring the bake back in step with a tile table that arrived after the
    /// ground did.
    ///
    /// The other order the two can turn up in, and the only one this type cannot
    /// close by itself: the table is the install's, not the facet's, so nothing
    /// here is told when it is replaced. A world builder that takes its tables
    /// and its facets in either order calls this; a facet with no map has
    /// nothing to bake and stays that way.
    pub fn rebake(&mut self, tiles: &TileData) {
        self.spans = self
            .world
            .snapshot()
            .map(|base| SpanIndex::build(base.map(), tiles));
    }

    /// Publish a patch to the ground, and rebake over it in the same statement.
    ///
    /// **The rebake is the reason this method exists.** [`spans`](Self::spans)
    /// is a projection of the base, so a base that moves without it is exactly
    /// the state this type was built to make unspellable — and a patch is the
    /// one thing that moves the base while the shard is running. A caller that
    /// could publish through the world alone would be a caller that could forget
    /// it.
    ///
    /// **It rebakes the chunks the patch touched and no others** —
    /// `navigation_spans.md`'s N8. A facet-wide bake is 109.7 ms on Felucca and
    /// a patch moves one chunk of 7,168, and that number was paid on the tick an
    /// operator typed into. The chunks come from
    /// [`Patch::touched_chunks`](openshard_map::patch::Patch::touched_chunks),
    /// which derives them from the ops rather than carrying a list beside them.
    ///
    /// # Errors
    ///
    /// [`PatchError`], from [`World::publish`] — and on any of them nothing has
    /// moved, so the bake is still the bake of the world in hand.
    pub fn publish(&mut self, patch: &Patch, tiles: &TileData) -> Result<Undo, PatchError> {
        let undo = self.world.publish(patch)?;
        self.rebake_chunks(&patch.touched_chunks(), tiles);
        Ok(undo)
    }

    /// Take back a publish that was never written down, bake and all.
    ///
    /// The other half of [`publish`](Self::publish), and it is local for exactly
    /// the same reason: the inverses touch the tiles the ops did, so the world it
    /// puts back differs from the one baked a moment ago over
    /// [`Undo::touched_chunks`] and nowhere else.
    pub fn undo(&mut self, undo: &Undo, tiles: &TileData) {
        self.world.undo(undo);
        self.rebake_chunks(&undo.touched_chunks(), tiles);
    }

    /// Take squares of ground the other end of the wire has published, and
    /// rebake over them in the same statement.
    ///
    /// [`publish`](Self::publish) is how the *shard* moves its ground and this is
    /// how a client's moves: it holds no patch and no history, only the chunks a
    /// publish notice named and it went and fetched. See
    /// [`World::take_chunks`], which is the half without the bake, and
    /// `docs/map/new_map_representation/to_the_client.md`'s E4.
    ///
    /// **The rebake is this method's reason for existing**, exactly as it is
    /// `publish`'s: the span layer is a projection of the base, and this is the
    /// second thing that moves the base while something is running.
    ///
    /// # Errors
    ///
    /// [`ChunksError`], from [`World::take_chunks`] — and on either of them
    /// nothing has moved, so the bake is still the bake of the world in hand.
    pub fn take_chunks(&mut self, chunks: &[Chunk], tiles: &TileData) -> Result<MapRevision, ChunksError> {
        let revision = self.world.take_chunks(chunks)?;
        // The squares that arrived name themselves, so this end needs no patch
        // to know what moved — which is what makes the window's half of N8 the
        // same call as the shard's.
        let touched: Vec<ChunkCoord> = chunks.iter().map(|chunk| chunk.key().at).collect();
        self.rebake_chunks(&touched, tiles);
        Ok(revision)
    }

    /// Rebake over the chunks that moved, or bake the facet whole where there is
    /// no bake to move.
    ///
    /// The one seam the partial path is reached through, and it is private:
    /// [`SpanIndex::rebake_chunks`] trusts its caller to name every chunk that
    /// changed, and the three writers above are the callers that know. See that
    /// method for the area it takes and why it is a block wider than the chunks.
    fn rebake_chunks(&mut self, chunks: &[ChunkCoord], tiles: &TileData) {
        match (&mut self.spans, self.world.snapshot()) {
            (Some(spans), Some(base)) => spans.rebake_chunks(base.map(), tiles, chunks),
            // A facet with no ground has nothing to bake, and one with ground and
            // no bake is the state this type exists to prevent — either way the
            // whole-facet path is the honest answer rather than a partial bake
            // over a world nothing has baked yet.
            _ => self.rebake(tiles),
        }
    }

    /// The ground, the statics and the revision they are at — and no way from
    /// here to the live layer.
    ///
    /// What a bake over this facet takes. See [`World::snapshot`], which this is
    /// and which says why the absence of the live half is the point.
    #[must_use]
    pub const fn snapshot(&self) -> Option<&MapSnapshot> {
        self.world.snapshot()
    }

    /// What the live world has laid over the ground, as every step decision
    /// reads it.
    #[must_use]
    pub const fn live(&self) -> &Overlay {
        self.world.live()
    }

    /// The live layer, to write.
    ///
    /// It does not disturb the bake, and that is a property rather than an
    /// oversight: the span layer is a projection of the *base*, so a door
    /// flipping and a ship sailing are exactly the changes it does not describe.
    pub const fn live_mut(&mut self) -> &mut Overlay {
        self.world.live_mut()
    }

    /// What the map alone says about this facet, read through the bake over it —
    /// or `None` for a facet with no map.
    ///
    /// The pair a [`MapTerrain`] is, handed out together because that is the
    /// only way it is ever true. `tiles` is the install's table, for the reason
    /// this module's own doc gives.
    #[must_use]
    pub fn terrain<'a>(&'a self, tiles: &'a TileData) -> Option<MapTerrain<'a>> {
        let base = self.world.snapshot()?;
        let index = self
            .spans
            .as_ref()
            .expect("a facet's ground and its bake move together");
        Some(MapTerrain::new(base.map(), tiles, index))
    }
}

#[cfg(test)]
mod tests {
    use openshard_map::grid::{
        BlockExtent,
        Tile,
    };
    use openshard_map::map::{
        LandCell,
        WorldMap,
    };
    use openshard_map::overlay::Cover;
    use openshard_protocol::world::Facet;
    use openshard_tiles::LandTileId;

    use super::*;

    /// One block of flat ground at `z`.
    fn facet(z: i8) -> MapSnapshot {
        MapSnapshot::new(
            Facet(0),
            WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| {
                LandCell {
                    tile: LandTileId(0),
                    z,
                }
            }),
        )
    }

    /// The whole point of the type: there is no way to have one and not the
    /// other, in either direction.
    #[test]
    fn a_bake_exists_exactly_when_the_ground_does() {
        let tiles = TileData::empty();

        let mut ground = Ground::new(None, &tiles);
        assert!(ground.snapshot().is_none());
        assert!(ground.terrain(&tiles).is_none(), "no map, no terrain to read");

        ground.set_base(Some(facet(0)), &tiles);
        assert!(
            ground.terrain(&tiles).is_some(),
            "ground arrived, and its bake with it"
        );

        ground.set_base(None, &tiles);
        assert!(
            ground.terrain(&tiles).is_none(),
            "ground left, and its bake with it"
        );
    }

    /// The failure this type exists to make unspellable: a facet whose map moved
    /// under a bake taken over the old one. The heights are what a step reads,
    /// so the bake has to answer for the map in hand and not the one before it.
    #[test]
    fn replacing_the_ground_replaces_the_bake_over_it() {
        let tiles = TileData::empty();
        let mut ground = Ground::new(Some(facet(0)), &tiles);
        assert_eq!(
            ground
                .terrain(&tiles)
                .expect("it was given a map")
                .ground_z(Tile::new(3, 3)),
            Some(0)
        );

        ground.set_base(Some(facet(20)), &tiles);

        assert_eq!(
            ground
                .terrain(&tiles)
                .expect("it was given a second map")
                .ground_z(Tile::new(3, 3)),
            Some(20),
            "the bake followed the ground it is a projection of"
        );
    }

    /// The same failure, arriving the way it will actually arrive: a patch
    /// published into a running shard. The bake has to move with it, or the
    /// steps a player takes are decided by the heights of a world nobody holds
    /// any more.
    #[test]
    fn a_published_patch_moves_the_bake_with_the_ground() {
        let tiles = TileData::empty();
        let mut ground = Ground::new(Some(facet(0)), &tiles);
        let at = Tile::new(3, 3);
        // Read through the span index rather than off the map: `surface_at` is
        // the bake's own answer, so a stale bake shows up here and nowhere else.
        let read = |ground: &Ground| {
            ground
                .terrain(&tiles)
                .expect("it was given a map")
                .surface_at(at.x, at.y, 30)
        };
        let before = read(&ground);
        assert_ne!(before, Some(30), "the fixture is flat at zero");

        let world = ground.snapshot().expect("it was given a map");
        // A land tile's height is its four corners, and a body stands on their
        // average — so raising one cell raises no tile at all. The four cells
        // that meet at this tile's corners are the edit.
        let raised = |x: u16, y: u16| {
            openshard_map::patch::PatchOp::set_land(
                world.map(),
                x,
                y,
                LandCell {
                    tile: LandTileId(0),
                    z:    30,
                },
            )
            .expect("a tile on the map")
        };
        let patch = Patch::new(
            Facet(0),
            world.revision(),
            openshard_map::patch::PatchAuthor("a test".into()),
            openshard_map::patch::PatchTime(0),
            vec![
                raised(at.x, at.y),
                raised(at.x + 1, at.y),
                raised(at.x, at.y + 1),
                raised(at.x + 1, at.y + 1),
            ],
        );

        let undo = ground.publish(&patch, &tiles).expect("the world in hand");

        assert_eq!(
            read(&ground),
            Some(30),
            "the bake followed the patch, not the map it was taken over"
        );

        ground.undo(&undo, &tiles);

        assert_eq!(read(&ground), before, "and it follows the way back too");
    }

    /// The live layer is orthogonal: writing it leaves the bake alone, because
    /// the bake is a projection of the base and never of what is standing on it.
    #[test]
    fn the_live_layer_moves_without_the_bake() {
        let tiles = TileData::empty();
        let mut ground = Ground::new(Some(facet(0)), &tiles);

        ground
            .live_mut()
            .set(Tile::new(2, 2), vec![Cover::blocking(0, 20)]);

        assert_eq!(ground.live().at(Tile::new(2, 2)).len(), 1);
        assert_eq!(
            ground
                .terrain(&tiles)
                .expect("it still has its map")
                .ground_z(Tile::new(2, 2)),
            Some(0),
            "a crate on a tile is not a change to the ground under it"
        );
    }

    /// The one order this type cannot close by itself — a table that arrives
    /// after the ground — and the seam that closes it.
    #[test]
    fn a_late_tile_table_is_what_rebake_is_for() {
        let tiles = TileData::empty();
        let ground = Ground::new(Some(facet(0)), &tiles);
        assert!(ground.terrain(&tiles).is_some());

        let mut ground = ground;
        ground.rebake(&tiles);
        assert!(
            ground.terrain(&tiles).is_some(),
            "a rebake is not a way to lose the bake"
        );

        let mut mapless = Ground::new(None, &tiles);
        mapless.rebake(&tiles);
        assert!(
            mapless.terrain(&tiles).is_none(),
            "nothing to bake, and it stayed that way"
        );
    }
}
