//! One facet as something that walks holds it: the world, and where a body may
//! stand on it.
//!
//! # The pair that used to be two fields
//!
//! [`SpanIndex`] is a projection of a facet's base — the ground and the
//! statics, and deliberately not the live layer over them. A world that moves
//! without its bake moving is a shard deciding steps by the heights of a map it
//! no longer holds, so the two have to travel together; until now they did it by
//! agreement. Both ends of the wire held them side by side and said so in a
//! comment — the shard's `FacetState`, the client's `Resources` — and
//! [`Footing::of`](crate::Footing::of) *checked* the pairing at the question,
//! with a panic for the facet that had a map and no bake over it.
//!
//! This is that agreement made into a value. It is [`Bedrock`], the base is
//! written by four functions on it and by nothing else, and each of them writes
//! the bake in the same statement — so "a facet with a map and no span bake over
//! it" is a state nothing can spell rather than a state something notices.
//!
//! # Two clocks, and now they are two fields
//!
//! A facet has two layers and they move on completely different clocks: the base
//! when a patch is published or a chunk arrives, the live layer as doors flip
//! and ships sail. [`Ground`] is the pair, and it holds the slow half **behind
//! an [`Arc`]** — see [`Ground::share`] for the whole of that argument.
//!
//! It used to hold an `openshard_map::world::World` (the base and the live layer
//! together) with the bake beside it, which grouped the two layers that move
//! apart and split the two that cannot. The regrouping is what makes the slow
//! half shareable at all, and it is also the more faithful one: the invariant
//! this module exists for is `base ↔ spans`, and that pair is now a type.
//!
//! # Why it is here and not in `openshard_map`
//!
//! Because that crate is underneath this one, and **where a body may stand is a
//! movement rule**: the bake reads [`MAX_STEP_UP`](crate::MAX_STEP_UP)
//! and [`PLAYER_HEIGHT`](crate::PLAYER_HEIGHT), and pushing those down would
//! make the crate that holds the world decide how tall a person is — which is
//! the move `realtime_map.md`'s R2 refused when `Cover::meets` asked for it, and
//! answered with a `Body` argument instead.
//!
//! So the layering is honoured by *wrapping* rather than by moving: the map
//! crate owns a facet's two layers, and this crate owns the movement rule baked
//! over the lower one. Nothing hands a bare snapshot out with the bake missing —
//! a reader that could take the base alone is a reader that could forget the
//! bake again, which is the whole of what this ends.
//!
//! # The tile table stays outside, still
//!
//! One install has one table and several facets, so what a graphic *is* is not a
//! fact about this world — `openshard_map`'s own docs draw that line and it is
//! unchanged here. The consequence is the one asymmetry in this file: the bake
//! is a statement about the world *and* the table, so the table is an argument
//! to every function that writes it, and a table that arrives after the ground
//! did needs [`Ground::rebake`].

use std::sync::Arc;

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
use openshard_map::world::ChunksError;
use openshard_tiles::TileData;

use crate::spans::SpanIndex;
use crate::terrain::MapTerrain;

/// The half of a facet that does not move while a body walks: the ground, the
/// statics, and where a body may stand on them.
///
/// **The invariant, in one line:** [`spans`](Self::spans) is `Some` exactly when
/// there is a [`base`](Self::base), and it is a bake of *that* base. Both fields
/// are private and every function that writes either writes both, so there is no
/// sequence of calls that separates them.
///
/// **The half that can be shared**, and that is the reason it is a type of its
/// own rather than two fields on [`Ground`]. Nothing in here changes while a
/// body is walking — a door flipping, a ship sailing and a crate being dropped
/// are all the *other* layer — so a thread that plans a route can read it
/// alongside the thread that draws it. See [`Ground::share`].
#[derive(Debug)]
pub struct Bedrock {
    /// The ground and the statics, at some published revision — or `None` for a
    /// facet with no map at all: no floor, no walls, every step allowed.
    base:  Option<MapSnapshot>,
    /// Where a body may stand on [`base`](Self::base), baked once.
    ///
    /// A projection of the base alone — a door, a crate and a house floor are
    /// invisible in it by construction — which is why the live layer moving does
    /// not touch it and the base moving always does.
    spans: Option<SpanIndex>,
}

impl Bedrock {
    /// Bake `base` and hold the pair.
    fn of(base: Option<MapSnapshot>, tiles: &TileData) -> Self {
        Self {
            spans: base.as_ref().map(|base| SpanIndex::build(base.map(), tiles)),
            base,
        }
    }

    /// The ground, the statics and the revision they are at.
    #[must_use]
    pub const fn snapshot(&self) -> Option<&MapSnapshot> {
        self.base.as_ref()
    }

    /// What the map alone says about this facet, read through the bake over it —
    /// or `None` for a facet with no map.
    #[must_use]
    pub fn terrain<'a>(&'a self, tiles: &'a TileData) -> Option<MapTerrain<'a>> {
        let base = self.base.as_ref()?;
        let index = self
            .spans
            .as_ref()
            .expect("a facet's ground and its bake move together");
        Some(MapTerrain::new(base.map(), tiles, index))
    }

    /// Bake the whole facet again — see [`Ground::rebake`], which this is.
    fn rebake(&mut self, tiles: &TileData) {
        self.spans = self.base.as_ref().map(|base| SpanIndex::build(base.map(), tiles));
    }

    /// Rebake over the chunks that moved, or bake the facet whole where there is
    /// no bake to move.
    ///
    /// [`SpanIndex::rebake_chunks`] trusts its caller to name every chunk that
    /// changed, and the three writers below are the callers that know. See that
    /// method for the area it takes and why it is a block wider than the chunks.
    fn rebake_chunks(&mut self, chunks: &[ChunkCoord], tiles: &TileData) {
        match (&mut self.spans, self.base.as_ref()) {
            (Some(spans), Some(base)) => spans.rebake_chunks(base.map(), tiles, chunks),
            // A facet with no ground has nothing to bake, and one with ground and
            // no bake is the state this type exists to prevent — either way the
            // whole-facet path is the honest answer rather than a partial bake
            // over a world nothing has baked yet.
            _ => self.rebake(tiles),
        }
    }

    /// Publish a patch and rebake over the chunks it touched.
    ///
    /// # Errors
    ///
    /// [`PatchError::NoGround`] — this facet has no map to patch at all.
    /// Otherwise [`MapSnapshot::publish`]'s, unchanged, and on any of them
    /// nothing has moved.
    fn publish(&mut self, patch: &Patch, tiles: &TileData) -> Result<Undo, PatchError> {
        let undo = self.base.as_mut().ok_or(PatchError::NoGround)?.publish(patch)?;
        self.rebake_chunks(&patch.touched_chunks(), tiles);
        Ok(undo)
    }

    /// Take a publish back, bake and all.
    fn undo(&mut self, undo: &Undo, tiles: &TileData) {
        self.base
            .as_mut()
            .expect("a facet that published a patch a moment ago still has its ground")
            .undo(undo);
        self.rebake_chunks(&undo.touched_chunks(), tiles);
    }

    /// Take squares of ground the other end of the wire cut, and rebake over
    /// them.
    ///
    /// # Errors
    ///
    /// [`ChunksError`], one variant per way a set of chunks is not a change to
    /// this facet. On either of them nothing has moved.
    fn take_chunks(&mut self, chunks: &[Chunk], tiles: &TileData) -> Result<MapRevision, ChunksError> {
        let revision = self
            .base
            .as_mut()
            .ok_or(ChunksError::NoGround)?
            .take_chunks(chunks)
            .map_err(ChunksError::Applying)?;
        // The squares that arrived name themselves, so this end needs no patch
        // to know what moved — which is what makes the window's half of N8 the
        // same call as the shard's.
        let touched: Vec<ChunkCoord> = chunks.iter().map(|chunk| chunk.key().at).collect();
        self.rebake_chunks(&touched, tiles);
        Ok(revision)
    }
}

/// One facet's world, and where a body may stand on it.
///
/// The two layers of a facet, held as the two things they are: the slow half
/// ([`Bedrock`] — the ground, the statics and the bake over them) shared behind
/// an [`Arc`], and the live half owned outright because it is rewritten as the
/// world arrives.
#[derive(Debug)]
pub struct Ground {
    /// The ground, the statics and the bake — see [`Bedrock`], and
    /// [`share`](Self::share) for why it is behind an `Arc`.
    bedrock: Arc<Bedrock>,
    /// What the live world has laid over it. Empty is the ordinary state of a
    /// freshly loaded facet, not a missing one.
    live:    Overlay,
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
            bedrock: Arc::new(Bedrock::of(base, tiles)),
            live:    Overlay::default(),
        }
    }

    /// A facet somebody else's [`Bedrock`] holds up, with `live` as this
    /// holder's own picture of what is on it.
    ///
    /// **What a thread that plans is given.** The bedrock is shared — the same
    /// map, the same bake, no copy — and the live layer is this holder's own, so
    /// there is nothing here for two threads to write to. See
    /// [`share`](Self::share).
    #[must_use]
    pub const fn shared(bedrock: Arc<Bedrock>, live: Overlay) -> Self {
        Self { bedrock, live }
    }

    /// The slow half of this facet, to hand to a thread that is not this one.
    ///
    /// # Why this is an `Arc`, said out loud
    ///
    /// `docs/style.md` refuses `Arc` by default because it turns ownership from
    /// a place in the code into a question about the run. The exception it
    /// leaves open is a real second thread and a structure large enough that a
    /// copy is absurd, and this is that case on both counts: a client plans
    /// routes off the thread that draws (`plans/world/pathfinding/PLAN.md`'s
    /// P3), and what a plan reads is 117.4 MiB of land, 29.5 MiB of statics and
    /// the span bake over them. It cannot be copied per query and it cannot be
    /// moved, because the thread that draws reads the same map every frame.
    ///
    /// What makes it safe is that it is *read-only while shared*: nothing in a
    /// [`Bedrock`] changes as a body walks. The live layer — the half that does
    /// change — is not in here, and a holder that wants both takes
    /// [`shared`](Self::shared) with a copy of its own.
    ///
    /// # And what the writers below owe it
    ///
    /// **A facet's ground is written while nothing is planning over it.** The
    /// four writers that move the base take the `Arc` back exclusively and say
    /// so; a caller that has handed one out settles whatever is planning first.
    /// [`set_base`](Self::set_base) is the exception and needs nothing: it bakes
    /// a whole new bedrock, so a plan already under way simply finishes over the
    /// facet it started on and the copy it held is dropped with it.
    #[must_use]
    pub fn share(&self) -> Arc<Bedrock> {
        Arc::clone(&self.bedrock)
    }

    /// The bedrock, to write.
    ///
    /// **This is not a lock, and the count is not a synchronisation.** Nothing
    /// here arbitrates between a reader and a writer, because there is never
    /// both at once: the thread that writes a facet is the thread that asks for
    /// plans over it, so it is inside one call or the other and never both — see
    /// `planner.rs`'s header for that timeline. What [`Arc::get_mut`] does is
    /// *check that the call order was kept*, and the share count reads as a
    /// sentence: **one** is "nothing is planning over this facet", **two** is
    /// "a worker is on this map right now". The second is a caller that forgot
    /// to settle first, and it is a panic rather than a wait because waiting
    /// here would hide the mistake at the one seam where it is cheap to see.
    ///
    /// # Panics
    ///
    /// If somebody is planning over this facet — see [`share`](Self::share).
    fn sole(&mut self) -> &mut Bedrock {
        Arc::get_mut(&mut self.bedrock).expect(
            "a facet's ground is written while nothing is planning over it — the holder that shared \
             it settles first; see Ground::share",
        )
    }

    /// Put ground under this facet, or take it away — and rebake in the same
    /// statement.
    ///
    /// A facet is built before its map is read on both ends: the shard inserts
    /// the facet and then loads it, and a test builds one and then gives it the
    /// scene it is about. This is the seam that arrival goes through, and the
    /// reason it takes the tile table is that the bake is a statement about
    /// both.
    ///
    /// **The one writer that does not need the bedrock back.** It bakes a whole
    /// new one, so a route being planned over the facet this replaces goes on
    /// reading the facet it started on and answers about a world that is no
    /// longer there — which is what a replan a moment later is for, and is the
    /// same staleness a plan from the tile a body has just left already has.
    pub fn set_base(&mut self, base: Option<MapSnapshot>, tiles: &TileData) {
        self.bedrock = Arc::new(Bedrock::of(base, tiles));
    }

    /// Bring the bake back in step with a tile table that arrived after the
    /// ground did.
    ///
    /// The other order the two can turn up in, and the only one this type cannot
    /// close by itself: the table is the install's, not the facet's, so nothing
    /// here is told when it is replaced. A world builder that takes its tables
    /// and its facets in either order calls this; a facet with no map has
    /// nothing to bake and stays that way.
    ///
    /// # Panics
    ///
    /// [`sole`](Self::sole)'s.
    pub fn rebake(&mut self, tiles: &TileData) {
        self.sole().rebake(tiles);
    }

    /// Publish a patch to the ground, and rebake over it in the same statement.
    ///
    /// **The rebake is the reason this method exists.** The span bake is a
    /// projection of the base, so a base that moves without it is exactly the
    /// state [`Bedrock`] was built to make unspellable — and a patch is the one
    /// thing that moves the base while the shard is running. A caller that could
    /// publish through the snapshot alone would be a caller that could forget
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
    /// [`PatchError`] — and on any of them nothing has moved, so the bake is
    /// still the bake of the world in hand.
    ///
    /// # Panics
    ///
    /// [`sole`](Self::sole)'s.
    pub fn publish(&mut self, patch: &Patch, tiles: &TileData) -> Result<Undo, PatchError> {
        self.sole().publish(patch, tiles)
    }

    /// Take back a publish that was never written down, bake and all.
    ///
    /// The other half of [`publish`](Self::publish), and it is local for exactly
    /// the same reason: the inverses touch the tiles the ops did, so the world it
    /// puts back differs from the one baked a moment ago over
    /// [`Undo::touched_chunks`] and nowhere else.
    ///
    /// # Panics
    ///
    /// [`sole`](Self::sole)'s.
    pub fn undo(&mut self, undo: &Undo, tiles: &TileData) {
        self.sole().undo(undo, tiles);
    }

    /// Take squares of ground the other end of the wire has published, and
    /// rebake over them in the same statement.
    ///
    /// [`publish`](Self::publish) is how the *shard* moves its ground and this is
    /// how a client's moves: it holds no patch and no history, only the chunks a
    /// publish notice named and it went and fetched. See
    /// `docs/world/design_chunks_to_the_client.md`'s E4.
    ///
    /// **The rebake is this method's reason for existing**, exactly as it is
    /// `publish`'s: the span layer is a projection of the base, and this is the
    /// second thing that moves the base while something is running.
    ///
    /// # Errors
    ///
    /// [`ChunksError`] — and on either of them nothing has moved, so the bake is
    /// still the bake of the world in hand.
    ///
    /// # Panics
    ///
    /// [`sole`](Self::sole)'s.
    pub fn take_chunks(&mut self, chunks: &[Chunk], tiles: &TileData) -> Result<MapRevision, ChunksError> {
        self.sole().take_chunks(chunks, tiles)
    }

    /// The ground, the statics and the revision they are at — and no way from
    /// here to the live layer.
    ///
    /// What a bake over this facet takes. Everything derived from a facet is
    /// stamped with the [`MapRevision`] it was built over and refuses itself on a
    /// mismatch; a bake that could also see a shut door would be recording an
    /// answer no revision describes.
    ///
    /// Not `const` any more, and the reason is the shape of this type rather
    /// than anything about a snapshot: reading through an [`Arc`] is a deref,
    /// and a deref is not something a constant may perform.
    #[must_use]
    pub fn snapshot(&self) -> Option<&MapSnapshot> {
        self.bedrock.snapshot()
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
    ///
    /// It does not disturb the bake, and that is a property rather than an
    /// oversight: the span layer is a projection of the *base*, so a door
    /// flipping and a ship sailing are exactly the changes it does not describe.
    /// It does not disturb a thread that is planning either, for the same
    /// reason — that thread was handed a copy of this layer and reads its own.
    pub const fn live_mut(&mut self) -> &mut Overlay {
        &mut self.live
    }

    /// What the map alone says about this facet, read through the bake over it —
    /// or `None` for a facet with no map.
    ///
    /// The pair a [`MapTerrain`] is, handed out together because that is the
    /// only way it is ever true. `tiles` is the install's table, for the reason
    /// this module's own doc gives.
    #[must_use]
    pub fn terrain<'a>(&'a self, tiles: &'a TileData) -> Option<MapTerrain<'a>> {
        self.bedrock.terrain(tiles)
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

    /// What sharing is *for*: one map read by two holders, and a live layer each
    /// of them owns.
    ///
    /// The thread that plans is the second holder, and this is the whole of its
    /// bargain — the same ground under both, and nothing either of them writes
    /// that the other can see.
    #[test]
    fn a_shared_bedrock_is_one_ground_under_two_live_layers() {
        let tiles = TileData::empty();
        let mut here = Ground::new(Some(facet(0)), &tiles);
        here.live_mut().set(Tile::new(2, 2), vec![Cover::blocking(0, 20)]);

        // What a worker is given: the same bedrock, and a copy of the live layer
        // as it stood when the question was asked.
        let mut elsewhere = Ground::shared(here.share(), here.live().clone());

        assert_eq!(
            elsewhere
                .terrain(&tiles)
                .expect("the shared bedrock has the map")
                .ground_z(Tile::new(3, 3)),
            here.terrain(&tiles)
                .expect("it still has its own")
                .ground_z(Tile::new(3, 3)),
            "one map, read by both"
        );
        assert_eq!(
            elsewhere.live().at(Tile::new(2, 2)).len(),
            1,
            "and the live layer travelled with it"
        );

        // Each writes its own half and neither sees the other's.
        elsewhere
            .live_mut()
            .set(Tile::new(4, 4), vec![Cover::blocking(0, 20)]);
        here.live_mut().clear();
        assert_eq!(elsewhere.live().at(Tile::new(4, 4)).len(), 1);
        assert_eq!(elsewhere.live().at(Tile::new(2, 2)).len(), 1);
        assert!(here.live().is_empty());
    }

    /// The one writer that needs nothing back: a facet replaced under a holder
    /// that is still reading the old one.
    ///
    /// A plan under way goes on answering about the world it started on, which
    /// is the same staleness a plan from the tile a body has just left already
    /// has — and the replan that follows is over the new ground.
    #[test]
    fn replacing_the_ground_leaves_a_sharer_on_the_facet_it_started_on() {
        let tiles = TileData::empty();
        let mut here = Ground::new(Some(facet(0)), &tiles);
        let elsewhere = Ground::shared(here.share(), Overlay::default());

        here.set_base(Some(facet(20)), &tiles);

        assert_eq!(
            here.terrain(&tiles)
                .expect("it was given a second map")
                .ground_z(Tile::new(3, 3)),
            Some(20),
            "the holder moved to the new facet"
        );
        assert_eq!(
            elsewhere
                .terrain(&tiles)
                .expect("it still holds the first")
                .ground_z(Tile::new(3, 3)),
            Some(0),
            "and the one still reading is on the facet it started on"
        );
    }

    /// And the writers that *do* need it back say so rather than quietly copying
    /// a hundred and forty megabytes or, worse, moving ground somebody is
    /// reading.
    ///
    /// The panic is the contract in [`Ground::share`]: a caller that has handed
    /// a bedrock out settles whatever is planning before it writes.
    #[test]
    #[should_panic(expected = "nothing is planning over it")]
    fn the_ground_may_not_be_rebaked_while_somebody_is_planning_over_it() {
        let tiles = TileData::empty();
        let mut ground = Ground::new(Some(facet(0)), &tiles);
        let planning = ground.share();
        ground.rebake(&tiles);
        drop(planning);
    }
}
