//! Turning the statics on a patch of map into the quads that draw them.
//!
//! The CPU side, the way [`crate::ground`] is for land: read what stands on the
//! visible cells, place each sprite, look it up in the atlas, and give it the
//! depth that decides what it hides. No GPU type appears here.
//!
//! # A static is not a tile
//!
//! Ground is 44x44 whatever the art holds and its quad is the diamond. A static
//! is a picture of any size that stands *on* a tile, and where it goes is the
//! client's arithmetic rather than ours: the sprite is centred on the tile's
//! column and its bottom edge sits at the diamond's bottom vertex, so a tall
//! tree hangs up the screen out of a 44-pixel cell. `View.DrawStatic` writes
//! that as `x -= (width >> 1) - 22` and `y -= height - 44` against a screen
//! position that is the cell's top-left corner — the same two numbers, said
//! from the corner instead of the centre.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use openshard_map::map::WorldMap;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_tiles::TileData;

use crate::animate::StaticAnimations;
#[cfg(test)]
use crate::atlas::StaticAtlas;
use crate::atlas::{Sprite, StaticArt, StaticAtlasPage};
use crate::camera::{Camera, TILE_HEIGHT, TileBounds, ViewPoint};
use crate::cutaway::{self, Cutaway};
use crate::depth;
use crate::geometry::Rect;
use crate::mesh_face::{MeshFaceRow, MeshFaceVertex};
use crate::sprite::SpriteQuad;

mod picking;
pub use picking::{PickedStatic, pick, pick_with_interior, selected};

/// Where a sprite standing on a tile lands, as a [`ViewPoint`] — the drawn
/// image's own grid, before any zoom. The doc here said "viewport pixels" for
/// as long as the answer was a bare `Vec2`; it never was, and the type says so
/// now.
///
/// The arithmetic in this module's own header, named so that it has one copy:
/// a static read out of the map and an item the server put on the ground are
/// the same picture standing the same way, and the second is
/// [`crate::items`]. Centred on the tile's column, bottom edge on the
/// diamond's bottom vertex — `View.DrawStatic`.
pub fn stand_on(camera: &Camera, at: Point, sprite: &Sprite) -> ViewPoint {
    let at = camera.to_screen(at);
    ViewPoint::new(
        // `>> 1` and not `/ 2.0`: an odd-width sprite lands half a pixel off
        // centre in the client too, and rounding it the other way shifts every
        // one of them against the ground.
        (at.x - (i32::from(sprite.width) >> 1)) as f32,
        (at.y + TILE_HEIGHT / 2 - i32::from(sprite.height)) as f32,
    )
}

/// Whether a sprite placed here touches the drawn image at all.
///
/// `AddTileToRenderList` rejects an object whose screen position falls outside
/// `_minPixel`/`_maxPixel` before it asks anything else about it, and the
/// reason there is a reject at all is [`for_each_static_in`]'s: the cells walked
/// are widened by the whole `z` range in both directions, which is 512 pixels
/// either way, so a screenful of tiles is walked with a frame of cells around
/// it that cannot draw anything.
///
/// The client tests the cell's own corner against bounds grown by a tile. This
/// tests the sprite's actual rectangle, which is the same question asked
/// exactly — a 250-pixel tree hangs five tiles up the screen out of its own
/// cell, and a margin is what the client needs because it is testing a point
/// that is not where the picture is.
pub fn on_screen(camera: &Camera, at: ViewPoint, sprite: &Sprite) -> bool {
    at.x + f32::from(sprite.width) > 0.0
        && at.x < camera.render_width() as f32
        && at.y + f32::from(sprite.height) > 0.0
        && at.y < camera.render_height() as f32
}

/// Every distinct static graphic standing on the cells the camera can see.
///
/// Called before building the atlas, for the same reason
/// [`ground::visible_graphics`](crate::ground::visible_graphics) is: a quad
/// cannot be given a region until the atlas holding it exists.
pub fn visible_graphics(map: &WorldMap, camera: &Camera, animations: &StaticAnimations) -> BTreeSet<Graphic> {
    let mut seen = BTreeSet::new();
    graphics_in(map, camera.visible_tiles(), animations, &mut seen);
    seen
}

/// Every distinct static graphic standing on the cells of one rectangle, added
/// to `out`. [`ground::graphics_in`](crate::ground::graphics_in) for the sprites
/// rather than the ground, and it exists for the same reason: an atlas grows by
/// the band the camera crossed, not by the viewport it is looking at.
///
/// An animated static contributes its **whole cycle** and not the graphic it is
/// showing — see [`StaticAnimations::cycle`]. Offering the current one instead
/// packs less and grows the atlas every time a fire ticks over, which is a band
/// of rows uploaded to the GPU on whichever frame that happened to be.
pub fn graphics_in(
    map: &WorldMap,
    bounds: TileBounds,
    animations: &StaticAnimations,
    out: &mut BTreeSet<Graphic>,
) {
    for_each_static_in(map, bounds, |item| {
        out.extend(animations.cycle(item.tile));
    });
}

/// The quads for every visible static.
///
/// A graphic the atlas does not hold is dropped — the client ships no art for
/// it, or the atlas was built for a different camera — which is the same
/// "nothing to draw here" the ground makes of a missing land sprite.
///
/// The order they come back in does not decide what covers what: every quad
/// carries its own depth and the pass tests it. They are sorted anyway, back to
/// front, so that the same camera produces the same buffer byte for byte —
/// which is what the frame tests assert on, and what a `HashMap` slipped in
/// later would quietly take away.
///
/// `cutaway` is what the frame has decided not to draw — the roof over the
/// player and the storey above them. It is a parameter and not a lookup because
/// it is one answer per frame: it is read from the tile the player is standing
/// on and every quad in the frame is tested against the same three numbers. See
/// [`crate::cutaway`].
/// Everything [`collect`] gathers about the statics on screen: the pictures
/// to draw, and the honest per-face geometry `docs/gbuffer.md` step 4c's
/// mesh pass draws over some of them.
///
/// One walk builds both — [`for_each_static_in`]'s own doc is why a second
/// walk asking the same question of the same statics would be two answers to
/// "which statics is this frame about" rather than one.
#[derive(Clone, Debug, Default)]
pub struct StaticGeometry {
    /// The pictures, back to front — what this function returned before mesh
    /// geometry existed, unchanged.
    pub quads: Vec<SpriteQuad>,
    /// Pictures that would otherwise cover the player's body. They are drawn
    /// into a private G-buffer, then lit and alpha-composited over the opaque
    /// world, so the wall keeps its own surface data without replacing the
    /// body's answer.
    pub cutaway_quads: Vec<SpriteQuad>,
    /// The cutaway pictures' own volume list. It is separate from [`Self::boxes`]
    /// because the two static passes bind and index their rows independently.
    pub cutaway_boxes: Vec<crate::impostor::Volume>,
    /// Raw vertices for every visible climbable static's [`crate::mesh::Mesh`],
    /// six per face ([`crate::mesh::Face::fan`]) —
    /// [`crate::renderer::MeshFaceRenderer::render`]'s own input.
    pub mesh_vertices: Vec<MeshFaceVertex>,
    /// One row per face represented in `mesh_vertices`, addressed by a
    /// vertex's own `id`.
    pub mesh_rows: Vec<MeshFaceRow>,
    /// Every box every drawn static stands as, each quad naming its own run of
    /// them through [`SpriteQuad::volumes`] —
    /// `docs/lighting_rebuild.md` phase 6.
    ///
    /// One flat list for the frame rather than a list per static, because it
    /// becomes one storage buffer: a range into it is two words on a row the
    /// instance buffer was already padding out to.
    pub boxes: Vec<crate::impostor::Volume>,
}

/// CPU time spent by [`collect_with_fades_profiled`] inside the map-static
/// collector. Kept separate from server items: they share a renderer pass, but
/// they are built from different sources and need separate profiling answers.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CollectCosts {
    /// Placement, culling, volume construction and collection of map statics.
    pub walk: Duration,
    /// The two stable back-to-front sorts after collection.
    pub sort: Duration,
    /// Whether an animated map static was visible to this collection.
    pub animated: bool,
}

/// The opaque geometry left after the two picture lists have been spent.
///
/// `sprite::split_corners` consumes [`StaticGeometry::quads`] while the app
/// carries [`StaticGeometry::cutaway_quads`] and `cutaway_boxes` to their
/// private, deferred layer. An emptied [`StaticGeometry`] would claim no
/// statics drew to any later reader; this type carries only the three opaque
/// geometry fields that actually remain.
#[derive(Debug, Default)]
pub struct StaticMesh {
    /// See [`StaticGeometry::mesh_vertices`].
    pub mesh_vertices: Vec<MeshFaceVertex>,
    /// See [`StaticGeometry::mesh_rows`].
    pub mesh_rows: Vec<MeshFaceRow>,
    /// See [`StaticGeometry::boxes`].
    pub boxes: Vec<crate::impostor::Volume>,
}

impl StaticGeometry {
    /// Move only a second producer's private cutaway layer into this one.
    ///
    /// The opaque rows intentionally remain separate in the client: immutable
    /// map rows can be replaced by a block composite while server items still
    /// need current-frame ids.  Cutaway rows share one private G-buffer,
    /// however, so they must be joined before the one cutaway render call.
    pub fn absorb_cutaway(&mut self, mut other: Self) {
        let cutaway_boxes = self.cutaway_boxes.len() as u32;
        self.cutaway_quads
            .extend(other.cutaway_quads.drain(..).map(|mut quad| {
                if quad.volumes.count != 0 {
                    quad.volumes.offset += cutaway_boxes;
                }
                quad
            }));
        self.cutaway_boxes.append(&mut other.cutaway_boxes);
        self.cutaway_quads
            .sort_by(|back, front| front.depth.total_cmp(&back.depth));
    }

    /// Append another frame's-worth of statics to this one, as one set.
    ///
    /// The map's furniture and the server's dropped items are two [`collect`]s
    /// of the same shape drawn by one pass, so the two have to become one list
    /// — and **three list pairs here are addressed by index**, so
    /// appending is not `Vec::extend` three times. A quad names its boxes by an
    /// offset into `boxes`, and a mesh vertex names its row by an index into
    /// `mesh_rows`; both are relative to the list they were built against, and
    /// both start at zero in the second one.
    ///
    /// **The mesh half was a live defect** — `docs/lighting_rebuild.md` phase 6c
    /// found it while wiring the first half — and it needs a climbable *item* to
    /// show, which is why nothing had: an item with a prism drew its faces
    /// against whichever of the map's rows its own numbering happened to land
    /// on, so the tile and the solid a fragment reported were another static's.
    /// One place does the join now, and it does both.
    pub fn absorb(&mut self, other: Self) {
        let boxes = self.boxes.len() as u32;
        let cutaway_boxes = self.cutaway_boxes.len() as u32;
        let rows = self.mesh_rows.len() as u32;
        self.quads.extend(other.quads.into_iter().map(|mut quad| {
            // An empty range keeps its own offset rather than being moved: it
            // addresses nothing and `offset + 0` past the end of the list is
            // not a place to point at.
            if quad.volumes.count != 0 {
                quad.volumes.offset += boxes;
            }
            quad
        }));
        self.mesh_vertices
            .extend(other.mesh_vertices.into_iter().map(|mut vertex| {
                vertex.id += rows;
                vertex
            }));
        self.mesh_rows.extend(other.mesh_rows);
        self.boxes.extend(other.boxes);
        self.cutaway_quads
            .extend(other.cutaway_quads.into_iter().map(|mut quad| {
                if quad.volumes.count != 0 {
                    quad.volumes.offset += cutaway_boxes;
                }
                quad
            }));
        self.cutaway_boxes.extend(other.cutaway_boxes);
        // Unlike the opaque pass, the private cutaway layer is later
        // alpha-composited, so its row order is observable.  Each collector
        // has already made its own stable back-to-front order, but map statics
        // and server items can interleave.  Keep equal-depth rows in collector
        // order — map first, then the server item that the ordinary pass also
        // draws afterwards — while restoring one order across both sources.
        self.cutaway_quads
            .sort_by(|back, front| front.depth.total_cmp(&back.depth));
    }
}

/// `occlusion` is **this frame's own grid**, and it has to have been built
/// already: what each row carries beside its picture is the number that grid gave
/// the static it draws ([`crate::occlusion::Occlusion::owner_at`]), which is the
/// join `docs/lighting_height.md` phase 3 pays for. A frame that collected its
/// statics first would be stamping numbers from the frame before it.
#[allow(clippy::too_many_arguments)]
pub fn collect<'a>(
    map: &WorldMap,
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: impl Into<StaticArt<'a>>,
    cutaway: &Cutaway,
    occlusion: &crate::occlusion::Occlusion,
    player_rect: Option<Rect>,
    player_mask: Option<&crate::mobiles::OpaqueMask>,
) -> StaticGeometry {
    collect_in(
        map,
        camera,
        camera.visible_tiles(),
        tiledata,
        animations,
        atlas,
        cutaway,
        occlusion,
        player_rect,
        player_mask,
    )
}

/// Map-static geometry on one caller-selected tile rectangle.
///
/// This is the immutable-map counterpart to [`crate::ground::collect_in`].
/// A block-composite producer gives it exactly one map block, while the LOD 0
/// renderer continues to use [`collect`] and its camera-visible rectangle.
/// Server items have their separate [`crate::items`] collector and cannot enter
/// this result.
#[allow(clippy::too_many_arguments)]
pub fn collect_in<'a>(
    map: &WorldMap,
    camera: &Camera,
    bounds: TileBounds,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: impl Into<StaticArt<'a>>,
    cutaway: &Cutaway,
    occlusion: &crate::occlusion::Occlusion,
    player_rect: Option<Rect>,
    player_mask: Option<&crate::mobiles::OpaqueMask>,
) -> StaticGeometry {
    collect_in_with_fades(
        map,
        camera,
        bounds,
        tiledata,
        animations,
        atlas,
        cutaway,
        occlusion,
        player_rect,
        player_mask,
        &mut crate::cutaway::Fades::default(),
    )
}

/// [`collect`] with opacity state retained across frames by the caller.
#[allow(clippy::too_many_arguments)]
pub fn collect_with_fades<'a>(
    map: &WorldMap,
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: impl Into<StaticArt<'a>>,
    cutaway: &Cutaway,
    occlusion: &crate::occlusion::Occlusion,
    player_rect: Option<Rect>,
    player_mask: Option<&crate::mobiles::OpaqueMask>,
    fades: &mut crate::cutaway::Fades,
) -> StaticGeometry {
    collect_in_with_fades(
        map,
        camera,
        camera.visible_tiles(),
        tiledata,
        animations,
        atlas,
        cutaway,
        occlusion,
        player_rect,
        player_mask,
        fades,
    )
}

/// [`collect_in`] retaining the caller's cutaway fade state.
#[allow(clippy::too_many_arguments)]
pub fn collect_in_with_fades<'a>(
    map: &WorldMap,
    camera: &Camera,
    bounds: TileBounds,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: impl Into<StaticArt<'a>>,
    cutaway: &Cutaway,
    occlusion: &crate::occlusion::Occlusion,
    player_rect: Option<Rect>,
    player_mask: Option<&crate::mobiles::OpaqueMask>,
    fades: &mut crate::cutaway::Fades,
) -> StaticGeometry {
    collect_in_with_fades_profiled(
        map,
        camera,
        bounds,
        tiledata,
        animations,
        atlas,
        cutaway,
        occlusion,
        player_rect,
        player_mask,
        fades,
    )
    .0
}

/// [`collect_with_fades`], with the expensive map-static phases measured for
/// the app's jank log.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub(crate) fn collect_with_fades_profiled<'a>(
    map: &WorldMap,
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: impl Into<StaticArt<'a>>,
    cutaway: &Cutaway,
    occlusion: &crate::occlusion::Occlusion,
    player_rect: Option<Rect>,
    player_mask: Option<&crate::mobiles::OpaqueMask>,
    fades: &mut crate::cutaway::Fades,
) -> (StaticGeometry, CollectCosts) {
    collect_with_fades_profiled_with_interior(
        map,
        camera,
        tiledata,
        animations,
        atlas,
        cutaway,
        occlusion,
        player_rect,
        player_mask,
        fades,
        None,
    )
}

/// [`collect_with_fades_profiled`] with the building picture gate for this
/// frame. Kept separate so the tools' ordinary assembly retains its exact
/// historical inputs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_with_fades_profiled_with_interior<'a>(
    map: &WorldMap,
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: impl Into<StaticArt<'a>>,
    cutaway: &Cutaway,
    occlusion: &crate::occlusion::Occlusion,
    player_rect: Option<Rect>,
    player_mask: Option<&crate::mobiles::OpaqueMask>,
    fades: &mut crate::cutaway::Fades,
    interior: Option<&crate::interiors::InteriorFrame>,
) -> (StaticGeometry, CollectCosts) {
    collect_in_with_fades_profiled_with_interior(
        map,
        camera,
        camera.visible_tiles(),
        tiledata,
        animations,
        atlas,
        cutaway,
        occlusion,
        player_rect,
        player_mask,
        fades,
        interior,
    )
}

/// [`collect_in_with_fades`], with map-static costs kept for the jank log.
#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_in_with_fades_profiled<'a>(
    map: &WorldMap,
    camera: &Camera,
    bounds: TileBounds,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: impl Into<StaticArt<'a>>,
    cutaway: &Cutaway,
    occlusion: &crate::occlusion::Occlusion,
    player_rect: Option<Rect>,
    player_mask: Option<&crate::mobiles::OpaqueMask>,
    fades: &mut crate::cutaway::Fades,
) -> (StaticGeometry, CollectCosts) {
    collect_in_with_fades_profiled_with_interior(
        map,
        camera,
        bounds,
        tiledata,
        animations,
        atlas,
        cutaway,
        occlusion,
        player_rect,
        player_mask,
        fades,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn collect_in_with_fades_profiled_with_interior<'a>(
    map: &WorldMap,
    camera: &Camera,
    bounds: TileBounds,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: impl Into<StaticArt<'a>>,
    cutaway: &Cutaway,
    occlusion: &crate::occlusion::Occlusion,
    player_rect: Option<Rect>,
    player_mask: Option<&crate::mobiles::OpaqueMask>,
    fades: &mut crate::cutaway::Fades,
    interior: Option<&crate::interiors::InteriorFrame>,
) -> (StaticGeometry, CollectCosts) {
    let atlas = atlas.into();
    let (eye_x, eye_y) = camera.eye_tile();
    let base = depth::base_for(eye_x, eye_y);
    let mut quads: Vec<(depth::Order, SpriteQuad)> = Vec::new();
    let mut cutaway_quads: Vec<(depth::Order, SpriteQuad)> = Vec::new();
    // A canopy may contribute several graphics. Advance its persistent state
    // once per frame, then reuse that value for every member.
    let mut foliage_alpha = BTreeMap::new();
    let mut cutaway_boxes = Vec::new();
    // Always empty since `docs/lighting_rebuild.md` phase 6d: a real static's
    // position and normal come from the impostor meeting `boxes` below, and
    // nothing here pushes a mesh face for it any more. `StaticGeometry` still
    // carries the two fields — [`crate::renderer::MeshFaceRenderer`]'s own
    // pass still runs, into whatever these are, and the four hand-built
    // diagnostic scenes are what fill the same fields with real faces.
    let mesh_vertices = Vec::new();
    let mesh_rows = Vec::new();
    let mut boxes = Vec::new();
    let mut animated = false;

    let walk_started = Instant::now();
    for_each_static_in(map, bounds, |item| {
        animated |= animations.is_animated(item.tile);
        let at = Point::new(item.x, item.y, item.z);
        let tile = tiledata.static_tile(item.tile.0);
        if !interior.is_none_or(|frame| frame.shows_static_at(at, tile)) {
            return;
        }
        let is_foliage = tile.flags.is_foliage();
        let Some(placed) = place(
            at,
            item.tile,
            camera,
            tiledata,
            animations,
            atlas,
            cutaway,
            // Foliage uses the canopy rule below. Passing the body rectangle
            // into `place` would hard-cut it before the shared fade can move
            // all members of the canopy into the late layer.
            player_rect.filter(|_| !is_foliage),
        ) else {
            // The normal placement deliberately rejects a roof or upper
            // storey the cutaway hides. Keep that policy for the opaque pass,
            // but give it a late translucent row instead of making it vanish.
            let Some(placed) = place_cutaway(at, item.tile, camera, tiledata, animations, atlas, cutaway)
            else {
                return;
            };
            if on_screen(camera, placed.at, &placed.sprite) {
                let fade_key = match is_foliage {
                    true => crate::cutaway::FadeKey::foliage(at),
                    false => crate::cutaway::FadeKey::static_(at, item.tile),
                };
                let alpha = match is_foliage {
                    true => *foliage_alpha
                        .entry(fade_key)
                        .or_insert_with(|| fades.advance(fade_key, 0)),
                    false => fades.advance(fade_key, 0),
                };
                if alpha == 0 {
                    return;
                }
                let key = crate::occlusion::Owner::new(at.z, item.tile);
                let owner = occlusion.owner_at(i32::from(at.x), i32::from(at.y), at.z, item.tile);
                let volumes = push_volumes(
                    &mut cutaway_boxes,
                    at,
                    tiledata.static_tile(item.tile.0),
                    &crate::occlusion::shape_of(Some(atlas), item.tile),
                    key,
                    occlusion,
                );
                cutaway_quads.push((
                    placed.order,
                    quad_of(at, &placed, base, u32::from(item.hue.0), owner, volumes).with_opacity(alpha),
                ));
            }
            return;
        };
        if !on_screen(camera, placed.at, &placed.sprite) {
            return;
        }
        // The rectangle is only a cheap outer bound. A cutaway row needs one
        // pixel where both the wall and the player's body are actually opaque;
        // otherwise the empty corners of a diagonal wall or silhouette would
        // make an unrelated wall translucent. The private layer still tests
        // against opaque depth after mobiles, so a wall behind the body writes
        // nothing while one in front blends later.
        let target = if is_foliage {
            if player_rect.is_some_and(|body| cutaway::hides_foliage_over(body, placed_rect(&placed))) {
                crate::cutaway::FOLIAGE_ALPHA_U8
            } else {
                u8::MAX
            }
        } else if player_mask.is_some_and(|body| {
            placed.order > body.order()
                && body.overlaps_opaque(placed_rect(&placed), |x, y| atlas.opaque_at(placed.showing, x, y))
        }) {
            crate::cutaway::TRANSLUCENT_ALPHA_U8
        } else {
            u8::MAX
        };
        let fade_key = match is_foliage {
            true => crate::cutaway::FadeKey::foliage(at),
            false => crate::cutaway::FadeKey::static_(at, item.tile),
        };
        let alpha = match is_foliage {
            true => *foliage_alpha
                .entry(fade_key)
                .or_insert_with(|| fades.advance(fade_key, target)),
            false => fades.advance(fade_key, target),
        };
        if alpha != u8::MAX {
            let key = crate::occlusion::Owner::new(at.z, item.tile);
            let owner = occlusion.owner_at(i32::from(at.x), i32::from(at.y), at.z, item.tile);
            let volumes = push_volumes(
                &mut cutaway_boxes,
                at,
                tiledata.static_tile(item.tile.0),
                &crate::occlusion::shape_of(Some(atlas), item.tile),
                key,
                occlusion,
            );
            cutaway_quads.push((
                placed.order,
                quad_of(at, &placed, base, u32::from(item.hue.0), owner, volumes).with_opacity(alpha),
            ));
            return;
        }
        // The *placed* graphic and not `placed.showing`: the grid keyed its
        // owner off the same one, and an animated static would otherwise change
        // owner every hundred milliseconds. See `occlusion::Owner`.
        let key = crate::occlusion::Owner::new(at.z, item.tile);
        let owner = occlusion.owner_at(i32::from(at.x), i32::from(at.y), at.z, item.tile);
        // The boxes this static's own pixels will be met against — phase 6, and
        // built in the same walk as everything else about this static for
        // `for_each_static_in`'s own reason. The tile and the shape are the two
        // the *grid* is built from, asked here of the same tiledata and the same
        // atlas, so that what a fragment is met against is what a shadow ray
        // crosses and not a second reading of the art.
        let volumes = push_volumes(
            &mut boxes,
            at,
            tiledata.static_tile(item.tile.0),
            &crate::occlusion::shape_of(Some(atlas), item.tile),
            key,
            occlusion,
        );
        let quad = quad_of(at, &placed, base, u32::from(item.hue.0), owner, volumes);
        quads.push((placed.order, quad));
    });

    let walk = walk_started.elapsed();
    let sort_started = Instant::now();
    // Back to front, and a *stable* sort on the order alone: two statics on one
    // tile at one `PriorityZ` keep the order the file has them in, which is the
    // order the client inserted them into its per-tile list and therefore the
    // order it draws them. The depth test is `LessEqual`, so later drawn wins
    // the tie — see `renderer::depth_state`. Sorting by the graphic as well
    // would be just as deterministic and would resolve those ties by an
    // accident of the art's numbering.
    quads.sort_by_key(|(order, _)| *order);
    // Alpha composition is order-dependent. `Order` ascending is already the
    // renderer's back-to-front order, so the same stable sort is the one source
    // over needs for two translucent statics that overlap.
    cutaway_quads.sort_by_key(|(order, _)| *order);
    let sort = sort_started.elapsed();
    (
        StaticGeometry {
            quads: quads.into_iter().map(|(_, quad)| quad).collect(),
            cutaway_quads: cutaway_quads.into_iter().map(|(_, quad)| quad).collect(),
            cutaway_boxes,
            mesh_vertices,
            mesh_rows,
            boxes,
        },
        CollectCosts { walk, sort, animated },
    )
}

/// The boxes one drawn static stands as, appended to `out`, and the range they
/// occupy in it.
///
/// `docs/lighting_rebuild.md` phase 6's own join, and the reason a fragment can
/// be met against geometry without a second draw: every sprite instance carries
/// a range of [`crate::impostor::Volume`]s that are **its own**, so the shader
/// meeting a view ray with them cannot reach a neighbour's shape. Where two
/// silhouettes disagree there is therefore no pixel belonging to neither — the
/// thing `facing::WIDTH_OVERLAP` grows a mesh to cover — only a pixel of *this*
/// static that fell some measured distance outside *this* static's volume.
///
/// **The static's own shape, and the grid only for the name.**
/// [`crate::occlusion::boxes_of`] says what a thing standing here is — a wall's
/// panel, a floor's lid, a body's tile, a flight's treads — and
/// [`crate::occlusion::Occlusion::id_of`] says which solid of *this frame's grid*
/// each of those is, or nothing where the grid holds none.
///
/// **Not the grid's own boxes, which is what this asked for until phase 6c and
/// which was wrong for most of a frame.** `Builder::add` answers two questions at
/// once and only one of them is about shape: it refuses outright anything the
/// tiledata does not mark `NO_SHOOT` or `WINDOW`, so a floorboard, a rug, a fence
/// and about half the walls of a Britain street stand as **no box at all**.
/// Measured on one real place at radius 6: nineteen of thirty-nine drawn
/// pictures, twelve of them south-facing walls. Read through the grid, every one
/// of those became a billboard — the middle of its tile, no facing, lit from
/// every side — which is a worse answer than the stance it replaced and would
/// have undone `docs/lighting.md`'s decision 27 for every wall cap in the world.
/// A pane of glass has a shape whether or not it casts a shadow.
///
/// **But the grid's own box wherever the grid has one** — `docs/occluders.md`'s
/// D6, which that plan wrote down and did not do. Since S3b a run of coplanar
/// pieces with one [`crate::occlusion::Owner`] is folded into **one** primitive,
/// and the shapes [`crate::occlusion::boxes_of`] answers with are still one per
/// *tile*. Two adjacent statics of one staircase therefore stood as two boxes
/// with a face buried between them — a face the merged solid does not have — and
/// a fragment met against it read as a surface looking east where its neighbours
/// looked south. It was excused from shadow by the very solid it was buried in
/// (one merged primitive is one id), so it came out **fully lit**: a bright,
/// one-pixel vertical stroke at every seam between two abutting statics, once a
/// tile, which is what a person looking at a lit staircase called garbage on the
/// vertical joins. Position and normal stop being able to jump at a tile edge
/// when there is no edge in the volume, which is the sentence D6 is.
///
/// The fallback is what keeps 6c's own fix: a picture the grid refused has no
/// merged box to take, so it keeps its own — that is the `None` arm, and the
/// paragraph above is why it cannot become "read everything through the grid".
///
/// The join is by [`crate::occlusion::Part`], and it is what makes the lookup
/// possible at all: both sides walk the shapes in the order the grid pushed
/// them, so the `n`th here is the `n`th there. A `NOBODY` name is the honest
/// answer for a shape the grid refused — the fragment is *somewhere*, and it is
/// a point of nothing the shadow walk can be asked to exempt.
///
/// An empty range is left only for a picture with no shape at all, which
/// `boxes_of` never produces today; the shader's own no-volume case is what
/// [`crate::place::Stance::Upright`] has always meant, and a mobile is what
/// reaches it.
pub(crate) fn push_volumes(
    out: &mut Vec<crate::impostor::Volume>,
    at: Point,
    tile: &openshard_tiles::StaticTile,
    shape: &crate::occlusion::Shape,
    owner: crate::occlusion::Owner,
    occlusion: &crate::occlusion::Occlusion,
) -> crate::impostor::Range {
    let offset = out.len() as u32;
    let (x, y) = (i32::from(at.x), i32::from(at.y));
    // **What the art named, which is not what `boxes_of` occludes with.** The
    // shader asks this mask one question — did the art name a face of this box
    // at all — and `boxes_of`'s own mask cannot answer it: on a **climbable** it
    // is `Edges::ANY` by override, chosen to pick the slab test a solid takes,
    // and read as "the art named none" that put every fitted staircase in the
    // class of pictures with no facing. A flight's treads and risers are planes
    // `facing::Prism` measured off the picture. See `occlusion::named_edges`,
    // which is the expression `boxes_of` starts from, and
    // `docs/lighting_rebuild.md`'s backlog for the frame this was found on.
    let named_edges = crate::occlusion::named_edges(tile, shape);
    crate::occlusion::boxes_of(x, y, at.z, tile, shape, |part, _occluding, space| {
        let named = occlusion.id_of(x, y, owner, part);
        // The grid's own primitive where there is one — merged, and therefore
        // continuous across every tile this piece runs over. See the doc above.
        let space = match named {
            Some(id) => occlusion.solid(id).space,
            None => space,
        };
        // **And not from the merged solid either.** A run's own `Edges` is the
        // union `Cell` folds a tile's solids into, and what the shader asks is
        // whether *this piece's* art named a side — see
        // `crate::impostor::Volume::edges`. The question is about the picture
        // rather than about the run, which is why it is answered off the
        // graphic's own `Shape` above and not off anything this closure is
        // handed.
        out.push(crate::impostor::Volume::of(&space, named_edges, named));
    });
    crate::impostor::Range {
        offset,
        count: out.len() as u32 - offset,
    }
}

// **`MeshSink` and `push_mesh` lived here** and went with the last thing that
// called either: `docs/lighting_rebuild.md` phase 6d took the mesh pass off
// real statics, and `push_mesh` was `pub(crate)` for exactly two callers, both
// inside this crate — `statics::collect` and `items::collect` — because a
// third, external one (`examples/*.rs`, `tests/*.rs`) cannot see a `pub(crate)`
// item at all. Once both went, its only caller left was its own unit test,
// which went with it — see the note at its grave in `mod tests`. What replaced
// it, for a real static, is the impostor meeting `push_volumes`'s own boxes;
// what still builds a `MeshFaceRow`/`MeshFaceVertex` pair by hand is the four
// hand-built diagnostic scenes, each its own copy for the reason above.

/// One placed picture: where it lands, which frame it is showing, and where it
/// sorts.
///
/// The one copy of the arithmetic, so that nothing which draws a static — the
/// map's own furniture, an item the server dropped, a silhouette, a pick — can
/// answer those three questions differently. They must not: what a click hits is
/// *the picture on the screen*, and a placement written a second time is one that
/// drifts from the drawing one — the click lands a tile away and nothing in
/// either copy looks wrong.
///
/// `pub(crate)` and not private because [`crate::items`] is the same picture
/// standing the same way, differing only in where the list came from.
pub(crate) struct Placed {
    /// Where it sorts against everything else drawn this frame.
    pub(crate) order: depth::Order,
    /// Its top-left corner in the drawn image.
    pub(crate) at: ViewPoint,
    /// The atlas entry for the frame it is showing.
    pub(crate) sprite: Sprite,
    /// Atlas page that holds `sprite.region`; page zero is the legacy atlas.
    pub(crate) page: StaticAtlasPage,
    /// That frame's graphic, which is what the atlas is keyed by — not the
    /// placed one, which for an animated static is only the cycle's start.
    pub(crate) showing: Graphic,
    /// Which way its picture faces — a rug on the ground is as flat as a floor
    /// built into the map. See [`crate::place::Stance`].
    pub(crate) stance: crate::place::Stance,
}

/// Place one static, or `None` when there is nothing on screen for it: hidden by
/// the cutaway, or a graphic the atlas holds no art for.
///
/// `graphic` is the *placed* one and not the frame on screen. It decides the sort
/// and the tiledata lookup: a fire's frames are different art of the same size
/// standing in the same place, and ordering by whichever one is showing would let
/// a stack reshuffle itself every hundred milliseconds.
#[allow(clippy::too_many_arguments)]
pub(crate) fn place<'a>(
    at: Point,
    graphic: Graphic,
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: impl Into<StaticArt<'a>>,
    cutaway: &Cutaway,
    _player_rect: Option<Rect>,
) -> Option<Placed> {
    let atlas = atlas.into();
    let tile = tiledata.static_tile(graphic.0);
    if !cutaway::shows(cutaway, at.z, tile) {
        return None;
    }
    let showing = animations.showing(graphic);
    let packed = atlas.paged_sprite(showing)?;
    let sprite = packed.sprite;
    let screen_at = stand_on(camera, at, &sprite);
    // Foliage is classified by the collector, where all graphics of one
    // canopy can share a fade key. Placement itself must remain available for
    // that late row (and for picking), so it never hard-cuts foliage here.
    Some(Placed {
        order: depth::Order {
            tile: i32::from(at.x) + i32::from(at.y),
            priority_z: depth::static_priority_z(at.z, tile),
        },
        // The cell's centre, height folded in: `to_screen` already lifts `z` by
        // four pixels a unit, which is the same lift the ground gets.
        at: screen_at,
        sprite,
        page: packed.page,
        showing,
        // A floor's pixels are spread across its tile, a wall's run along the one
        // edge it stands on, and anything else claims the tile's middle. The
        // tiledata answers the first; the *art* answers the second, measured once
        // when the atlas packed this sprite. See `crate::place::Stance` and
        // `crate::facing`.
        stance: crate::place::Stance::of(tile, sprite.facing),
    })
}

/// Place a world sprite the frame's cutaway would ordinarily remove.
///
/// The opaque collectors for map statics and server items must continue to
/// exclude these rows: they do not write a depth or G-buffer answer, and a
/// hidden roof must not begin to occlude light merely because it is now faintly
/// visible. The late cutaway pass consumes the returned placement after bodies
/// are drawn and blends it over that already settled picture instead.
#[allow(clippy::too_many_arguments)]
pub(crate) fn place_cutaway<'a>(
    at: Point,
    graphic: Graphic,
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: impl Into<StaticArt<'a>>,
    cutaway: &Cutaway,
) -> Option<Placed> {
    let atlas = atlas.into();
    let tile = tiledata.static_tile(graphic.0);
    // The drawing ceiling and the internal flag are absolute rejects. Only a
    // thing this frame's cutaway hid belongs in the translucent list.
    if !cutaway::drawn_in_any_frame(at.z, tile) || cutaway.shows_static(at.z, tile) {
        return None;
    }
    let showing = animations.showing(graphic);
    let packed = atlas.paged_sprite(showing)?;
    let sprite = packed.sprite;
    let screen_at = stand_on(camera, at, &sprite);
    Some(Placed {
        order: depth::Order {
            tile: i32::from(at.x) + i32::from(at.y),
            priority_z: depth::static_priority_z(at.z, tile),
        },
        at: screen_at,
        sprite,
        page: packed.page,
        showing,
        stance: crate::place::Stance::of(tile, sprite.facing),
    })
}

/// The exact rectangle the sprite pass rasterises in viewport pixels.
///
/// The CPU uses it only to shortlist cutaway candidates. The alpha test and
/// depth comparison remain GPU decisions in the pass that actually blends.
pub(crate) fn placed_rect(placed: &Placed) -> Rect {
    Rect {
        x: placed.at.x,
        y: placed.at.y,
        width: f32::from(placed.sprite.width),
        height: f32::from(placed.sprite.height),
    }
}

/// One placed picture as an instance the sprite passes can draw.
///
/// `hue` is a parameter rather than read off anything here because the same
/// placement is drawn in three hues: the thing's own, the highlight ramp, and —
/// for a silhouette, where the colour is never read — whatever the caller had.
///
/// `owner` is which occluder of `at`'s tile this static is in the frame's own
/// occlusion grid — [`crate::occlusion::Occlusion::owner_at`], and
/// [`SpriteQuad::owner`] for what reads it.
/// [`OwnerId::NONE`](crate::occlusion::OwnerId::NONE) from the passes that draw
/// a picture nothing is lit from: a silhouette, a selection mask.
pub(crate) fn quad_of(
    at: Point,
    placed: &Placed,
    base: i32,
    hue: u32,
    owner: crate::occlusion::OwnerId,
    volumes: crate::impostor::Range,
) -> SpriteQuad {
    SpriteQuad {
        rect: Rect {
            x: placed.at.x,
            y: placed.at.y,
            width: f32::from(placed.sprite.width),
            height: f32::from(placed.sprite.height),
        },
        region: placed.sprite.region,
        place: crate::place::Place {
            stance: placed.stance,
            ..crate::place::Place::of_static(at)
        },
        depth: placed.order.to_depth(base),
        hue,
        // Set for a corner static by `crate::sprite::split_corners`, once
        // this row's final index among a frame's other corners is known —
        // not here, where it is not.
        twin: 0,
        owner: u32::from(owner.raw()),
        volumes,
    }
    .with_static_atlas_page(placed.page)
}

/// Walk every static on the visible cells, calling back for each.
///
/// The cells are the ones the ground walks — the same clamped rectangle — and
/// that is not quite the same set as "every static whose sprite touches the
/// viewport": a tree is 250 pixels tall and stands up to five tiles further
/// down the screen than its own cell. [`Camera::visible_tiles`] already widens
/// by the whole `z` range in both directions, which is 512 pixels either way,
/// so the sprites are covered by a margin that exists for another reason. Said
/// here because it is a dependency between two modules and not an accident.
/// `pub(crate)` for [`crate::light`], which walks the same cells to find what
/// on them burns: one walk written twice would be two answers to "which statics
/// is this frame about", and the lights would drift from the sprites making
/// them.
pub fn for_each_static_in(
    map: &WorldMap,
    bounds: TileBounds,
    mut each: impl FnMut(&openshard_map::map::StaticItem),
) {
    let Some((xs, ys)) = bounds.clamp_to(map.width(), map.height()) else {
        return;
    };
    // A row at a time and not a tile at a time. The order is the same one — the
    // map hands a row back in ascending `x`, which is what the tile walk did —
    // and the saving is that a row of a block is one binary search rather than
    // eight: **this walk was 0.98ms of the 2.30ms a widest-zoom frame spent
    // building its occlusion grid**, and 35,000 of its 35,000 tile lookups were
    // asked of a map that is mostly open ground. See `WorldMap::statics_in_row`.
    let (from_x, to_x) = (*xs.start(), *xs.end());
    for y in ys {
        for item in map.statics_in_row(y, from_x, to_x) {
            each(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::atlas::StaticAtlasPages;
    use crate::camera::RealPixel;

    use openshard_map::grid::BlockExtent;
    use openshard_map::map::{LandCell, StaticItem};
    use openshard_protocol::wire::Hue;
    use openshard_tiles::LandTileId;
    use openshard_uofiles::color::Color16;
    use openshard_uofiles::image::Image;

    use super::*;

    /// A map big enough for a camera at (100, 100), with flat ground and nothing
    /// standing on it. Statics are placed by the tests that want them.
    fn field() -> WorldMap {
        WorldMap::from_blocks(BlockExtent { wide: 16, down: 16 }, |_, _| LandCell {
            tile: LandTileId(3),
            z: 0,
        })
    }

    /// The rectangle every occlusion fixture below stands in.
    fn grid_bounds() -> crate::camera::TileBounds {
        crate::camera::TileBounds {
            min_x: 98,
            max_x: 104,
            min_y: 98,
            max_y: 104,
        }
    }

    /// An atlas holding one graphic, drawn solid at a known size.
    fn atlas(graphic: Graphic, width: u16, height: u16) -> StaticAtlas {
        StaticAtlas::pack([(
            graphic,
            Image::new(
                width,
                height,
                vec![Color16(0x7C00); usize::from(width) * usize::from(height)],
            ),
        )])
        .expect("one sprite fits")
    }

    /// A tree over the player's head is cut so the body under it stays in
    /// view, and only when the two pictures actually share a pixel — a
    /// player's own rectangle standing well clear of the canopy still sees
    /// it, and a non-foliage static of the same size and position is never
    /// cut at all. The simplified hard cut this workspace chose over
    /// `ApplyFoliageTransparency`'s fade — see `cutaway::hides_foliage_over`.
    #[test]
    fn foliage_over_the_player_is_cut_and_nothing_else_is() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0D45);
        let atlas = atlas(graphic, 44, 88);
        let animations = StaticAnimations::default();
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            graphic.0,
            openshard_tiles::StaticTile {
                flags: openshard_tiles::TileFlags::new(openshard_tiles::TileFlags::FOLIAGE),
                ..Default::default()
            },
        );
        let at = Point::new(100, 100, 0);
        let cutaway = Cutaway::OPEN;
        let sprite = atlas.sprite(graphic).expect("packed");
        let screen_at = stand_on(&camera, at, &sprite);
        let over = Rect {
            x: screen_at.x,
            y: screen_at.y,
            width: 44.0,
            height: 88.0,
        };
        let elsewhere = Rect {
            x: screen_at.x + 1000.0,
            y: screen_at.y + 1000.0,
            width: 44.0,
            height: 88.0,
        };

        assert!(
            super::place(
                at,
                graphic,
                &camera,
                &tiledata,
                &animations,
                &atlas,
                &cutaway,
                None
            )
            .is_some(),
            "no player rectangle at all draws it, same as today"
        );
        assert!(
            super::place(
                at,
                graphic,
                &camera,
                &tiledata,
                &animations,
                &atlas,
                &cutaway,
                Some(elsewhere)
            )
            .is_some(),
            "a player standing well clear of the canopy still sees it"
        );
        assert!(
            super::place(
                at,
                graphic,
                &camera,
                &tiledata,
                &animations,
                &atlas,
                &cutaway,
                Some(over)
            )
            .is_some(),
            "the late collector needs the placed canopy to apply its fade"
        );

        let mut not_foliage = TileData::empty();
        not_foliage.set_static_tile(graphic.0, openshard_tiles::StaticTile::default());
        assert!(
            super::place(
                at,
                graphic,
                &camera,
                &not_foliage,
                &animations,
                &atlas,
                &cutaway,
                Some(over),
            )
            .is_some(),
            "the same overlap draws a non-foliage static: only foliage is cut"
        );
    }

    /// A static covering the player's on-screen body moves out of the opaque
    /// list, not out of the frame. The late renderer is what turns this row
    /// into alpha blending; keeping the split as a CPU test makes the feature's
    /// selection rule independent of a GPU or client install.
    #[test]
    fn a_wall_over_the_player_becomes_a_cutaway_row() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0006);
        let atlas = atlas(graphic, 44, 88);
        let animations = StaticAnimations::default();
        let tiledata = TileData::empty();
        let at = Point::new(100, 100, 0);
        let mut map = field();
        map.place_static(StaticItem {
            tile: graphic,
            x: at.x,
            y: at.y,
            z: at.z,
            hue: Hue(0),
        });
        let sprite = atlas.sprite(graphic).expect("packed wall");
        let screen_at = stand_on(&camera, at, &sprite);
        let player = Rect {
            x: screen_at.x + 12.0,
            y: screen_at.y + 32.0,
            width: 20.0,
            height: 32.0,
        };
        let player_mask = crate::mobiles::OpaqueMask::solid(player);

        let ordinary = collect(
            &map,
            &camera,
            &tiledata,
            &animations,
            &atlas,
            &Cutaway::OPEN,
            &crate::occlusion::Occlusion::EMPTY,
            None,
            None,
        );
        assert_eq!(ordinary.quads.len(), 1, "the fixture did not draw its wall");
        assert!(ordinary.cutaway_quads.is_empty(), "no body means no cutaway");

        let cutaway = collect(
            &map,
            &camera,
            &tiledata,
            &animations,
            &atlas,
            &Cutaway::OPEN,
            &crate::occlusion::Occlusion::EMPTY,
            Some(player),
            Some(&player_mask),
        );
        assert!(cutaway.quads.is_empty(), "the wall was still in the opaque pass");
        assert_eq!(
            cutaway.cutaway_quads.len(),
            1,
            "the wall did not reach the cutaway layer"
        );
        assert_eq!(cutaway.cutaway_quads[0].rect, ordinary.quads[0].rect);
        assert_eq!(cutaway.cutaway_quads[0].depth, ordinary.quads[0].depth);

        // The original storey cut has the same destination: it is no longer
        // an opaque row, but it remains a faint picture instead of vanishing.
        let above = collect(
            &map,
            &camera,
            &tiledata,
            &animations,
            &atlas,
            &Cutaway {
                max_z: 0,
                ..Cutaway::OPEN
            },
            &crate::occlusion::Occlusion::EMPTY,
            None,
            None,
        );
        assert!(above.quads.is_empty(), "the cut static reached the opaque pass");
        assert_eq!(
            above.cutaway_quads.len(),
            1,
            "the cut static disappeared outright"
        );
    }

    /// Art with a hole in it: the left half transparent, the right half drawn.
    /// Most static art is this shape — a wall's picture is a diagonal band in a
    /// rectangle — which is the whole reason picking is a texel test.
    fn holed(graphic: Graphic, width: u16, height: u16) -> StaticAtlas {
        let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height));
        for _ in 0..height {
            for x in 0..width {
                pixels.push(match x < width / 2 {
                    true => Color16::TRANSPARENT,
                    false => Color16(0x7C00),
                });
            }
        }
        StaticAtlas::pack([(graphic, Image::new(width, height, pixels))]).expect("one sprite fits")
    }

    /// Rectangles only bound the candidate search. A wall's atlas allocation
    /// can overlap the body while every texel in that overlap is transparent;
    /// C3 must leave that wall on the opaque path rather than fading it for air.
    #[test]
    fn a_transparent_static_corner_does_not_trigger_cutaway() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0007);
        let atlas = holed(graphic, 44, 88);
        let animations = StaticAnimations::default();
        let tiledata = TileData::empty();
        let at = Point::new(100, 100, 0);
        let mut map = field();
        map.place_static(StaticItem {
            tile: graphic,
            x: at.x,
            y: at.y,
            z: at.z,
            hue: Hue(0),
        });
        let screen_at = stand_on(&camera, at, &atlas.sprite(graphic).expect("packed wall"));
        let empty_corner = Rect {
            x: screen_at.x + 2.0,
            y: screen_at.y + 32.0,
            width: 12.0,
            height: 32.0,
        };
        let mask = crate::mobiles::OpaqueMask::solid(empty_corner);
        let geometry = collect(
            &map,
            &camera,
            &tiledata,
            &animations,
            &atlas,
            &Cutaway::OPEN,
            &crate::occlusion::Occlusion::EMPTY,
            Some(empty_corner),
            Some(&mask),
        );
        assert_eq!(
            geometry.quads.len(),
            1,
            "transparent pixels moved a wall out of opaque world"
        );
        assert!(
            geometry.cutaway_quads.is_empty(),
            "transparent corners became cutaway candidates"
        );
    }

    /// The viewport pixel a point in the drawn image sits at — the inverse of
    /// what [`pick`] undoes, so a test can click on a sprite it has placed.
    fn cursor_over(camera: &Camera, at: ViewPoint, dx: f32, dy: f32) -> RealPixel {
        let spot = camera.to_viewport(crate::camera::ViewPixel {
            x: (at.x + dx) as i32,
            y: (at.y + dy) as i32,
        });
        RealPixel::new(spot.x as i32, spot.y as i32)
    }

    /// A click on a wall's own pixels picks that wall, and a click through the
    /// transparent half of its picture picks nothing.
    ///
    /// The second assertion is the one worth having: a box test passes the first
    /// and fails this, and a box test is what selecting a wall by its rectangle
    /// would be — the cursor a tile away from any wall, inside the empty corner
    /// of its art, selecting it.
    #[test]
    fn a_click_on_a_wall_s_own_pixels_picks_it() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0006);
        let atlas = holed(graphic, 44, 60);
        let tiledata = TileData::empty();
        let mut map = field();
        map.place_static(StaticItem {
            tile: graphic,
            x: 100,
            y: 100,
            z: 0,
            hue: Hue(0),
        });
        let sprite = atlas.sprite(graphic).expect("packed");
        let at = stand_on(&camera, Point::new(100, 100, 0), &sprite);
        let pick_at = |dx, dy| {
            pick(
                &map,
                &camera,
                &tiledata,
                &StaticAnimations::default(),
                &atlas,
                &Cutaway::OPEN,
                cursor_over(&camera, at, dx, dy),
            )
        };
        assert_eq!(
            pick_at(30.0, 30.0),
            Some(PickedStatic {
                at: Point::new(100, 100, 0),
                graphic,
            }),
            "the drawn half was not hit",
        );
        assert_eq!(
            pick_at(5.0, 30.0),
            None,
            "the transparent half of the picture was picked — this is a box test, not a texel one",
        );
        assert_eq!(pick_at(-5.0, 30.0), None, "a pixel left of the sprite was picked");
        assert_eq!(pick_at(30.0, 70.0), None, "a pixel below the sprite was picked");
    }

    /// Two walls of one building overlap on screen. The one drawn on top is the
    /// one the click gets — the same answer the depth buffer gives the frame,
    /// which is what the player sees.
    #[test]
    fn the_topmost_wall_wins_an_overlap() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0006);
        // Tall enough that the nearer tile's sprite covers the further one's.
        let atlas = atlas(graphic, 44, 120);
        let tiledata = TileData::empty();
        let mut map = field();
        for (x, y) in [(100, 100), (101, 101)] {
            map.place_static(StaticItem {
                tile: graphic,
                x,
                y,
                z: 0,
                hue: Hue(0),
            });
        }
        let sprite = atlas.sprite(graphic).expect("packed");
        let near = stand_on(&camera, Point::new(101, 101, 0), &sprite);
        let found = pick(
            &map,
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            // Inside the near sprite's top strip, which is over the far one's
            // body: both are hit and only one may come back.
            cursor_over(&camera, near, 22.0, 10.0),
        );
        assert_eq!(
            found.map(|picked| picked.at),
            Some(Point::new(101, 101, 0)),
            "the wall behind was picked through the one in front",
        );
    }

    /// A wall the cutaway is not drawing cannot be pointed at.
    ///
    /// The pick asks the same question the collector does, so a roof the frame
    /// has taken away is not something the player can select through the hole it
    /// left. Without this the client would hand back a wall that is not on the
    /// screen — and then wash it, which draws nothing and reads as a broken
    /// selection.
    #[test]
    fn a_wall_the_cutaway_hides_is_not_picked() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0006);
        let atlas = atlas(graphic, 44, 60);
        let tiledata = TileData::empty();
        let mut map = field();
        map.place_static(StaticItem {
            tile: graphic,
            x: 100,
            y: 100,
            z: 20,
            hue: Hue(0),
        });
        let sprite = atlas.sprite(graphic).expect("packed");
        let at = stand_on(&camera, Point::new(100, 100, 20), &sprite);
        let cursor = cursor_over(&camera, at, 22.0, 30.0);
        let ask = |cutaway: &Cutaway| {
            pick(
                &map,
                &camera,
                &tiledata,
                &StaticAnimations::default(),
                &atlas,
                cutaway,
                cursor,
            )
        };
        assert!(ask(&Cutaway::OPEN).is_some(), "the scene proves nothing");
        // Everything at or above the storey's floor is taken out of the frame.
        let indoors = Cutaway {
            max_z: 10,
            ..Cutaway::OPEN
        };
        assert_eq!(ask(&indoors), None, "a wall this frame did not draw was picked");
    }

    /// The tile a wall stands on is **not** the tile the cursor unprojects to,
    /// and the difference is tiles rather than pixels.
    ///
    /// This is the defect the client shipped for one commit: a click on a wall
    /// washed the wall and put the held-tile marker on the ground *under the
    /// cursor*, which for a picture standing up the screen out of its own cell is
    /// two cells behind it. Both answers were right about their own question and
    /// the client was showing them as one.
    ///
    /// Pinned here, in the crate that owns both arithmetics, because the app is
    /// where they are chosen between and a comment there cannot fail. What it
    /// says is only "these two disagree, by this much" — which is the whole
    /// reason a selection has to name the *static's* tile and never the pick's.
    #[test]
    fn a_wall_s_tile_is_not_the_tile_under_the_cursor() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0006);
        // 44 wide by 88 tall, which is an ordinary wall of the client's art:
        // one tile across and two tiles of height up the screen.
        let atlas = atlas(graphic, 44, 88);
        let tiledata = TileData::empty();
        let mut map = field();
        let stands = Point::new(100, 100, 0);
        map.place_static(StaticItem {
            tile: graphic,
            x: stands.x,
            y: stands.y,
            z: stands.z,
            hue: Hue(0),
        });
        let sprite = atlas.sprite(graphic).expect("packed");
        let at = stand_on(&camera, stands, &sprite);
        // Halfway up the wall's face, which is where a person clicking on a wall
        // puts the cursor.
        let cursor = cursor_over(&camera, at, 22.0, 30.0);

        let picked = pick(
            &map,
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            cursor,
        )
        .expect("the cursor is on the wall's own pixels");
        assert_eq!(picked.at, stands, "the wall is on the tile it was placed on");

        // And the other arithmetic: the ground the cursor points at, read at the
        // ground's own height, which is what `App::pick_tile` resolves.
        let (x, y) = crate::camera::unproject(camera.pick(cursor), 0);
        assert_ne!(
            (x, y),
            (i32::from(stands.x), i32::from(stands.y)),
            "the two arithmetics agreed: this test can no longer say what it is for",
        );
        // Down the screen is north-west in tile space, so the ground under a
        // cursor halfway up a wall is *behind* the wall in both axes.
        assert!(
            x < i32::from(stands.x) && y < i32::from(stands.y),
            "the ground under the cursor came out at {x}, {y}",
        );
    }

    /// The quad the wash is drawn from is the quad the picture was drawn from.
    ///
    /// Stated as a comparison rather than as coordinates: the two are one
    /// arithmetic now, and this is what says the selection pass is using it. Two
    /// numbers here would go on passing if a second copy appeared and drifted —
    /// and a mask half a pixel off its sprite is a wash with a bright fringe
    /// down one side of the wall.
    #[test]
    fn a_selected_wall_s_quad_is_the_one_the_frame_drew() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0006);
        let atlas = atlas(graphic, 44, 60);
        let tiledata = TileData::empty();
        let mut map = field();
        map.place_static(StaticItem {
            tile: graphic,
            x: 101,
            y: 99,
            z: 5,
            hue: Hue(0),
        });
        let animations = StaticAnimations::default();
        let drawn = collect(
            &map,
            &camera,
            &tiledata,
            &animations,
            &atlas,
            &Cutaway::OPEN,
            &crate::occlusion::Occlusion::EMPTY,
            None,
            None,
        )
        .quads;
        assert_eq!(drawn.len(), 1);
        let picked = PickedStatic {
            at: Point::new(101, 99, 5),
            graphic,
        };
        let washed = selected(
            &camera,
            &tiledata,
            &animations,
            &atlas,
            &Cutaway::OPEN,
            Some(picked),
        );
        assert_eq!(washed.len(), 1);
        assert_eq!(washed[0].rect, drawn[0].rect);
        assert_eq!(washed[0].region, drawn[0].region);
        assert_eq!(washed[0].depth, drawn[0].depth);
        assert_eq!(washed[0].place, drawn[0].place);
        assert!(
            selected(&camera, &tiledata, &animations, &atlas, &Cutaway::OPEN, None).is_empty(),
            "nothing selected is an empty list, not a quad nobody asked for",
        );
    }

    /// Picking and its silhouette are CPU paths, but they still have to carry
    /// the page identity the renderer binds.  This fixture forces the selected
    /// wall onto page one, so a legacy-only lookup cannot accidentally keep
    /// passing through page zero.
    #[test]
    fn a_page_one_static_is_picked_and_selected_from_its_own_atlas_page() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let filler = Graphic(0x0100);
        let boundary = Graphic(0x0101);
        let graphic = Graphic(0x0102);
        let tall = |color| Image::new(2048, 1025, vec![color; 2048 * 1025]);
        let mut atlas = StaticAtlasPages::pack_with_limit([(filler, tall(Color16(0x001F)))], 2)
            .expect("the first page fits");
        atlas
            .pack_more([
                (boundary, tall(Color16(0x03E0))),
                (graphic, Image::new(44, 60, vec![Color16(0x7C00); 44 * 60])),
            ])
            .expect("the selected wall starts page one");
        assert_eq!(atlas.page_count(), 2, "the fixture must cross a page boundary");
        let sprite = atlas.sprite(graphic).expect("the selected wall was packed");
        assert_eq!(
            sprite.page,
            StaticAtlasPage(1),
            "the selected wall is not on page one"
        );

        let tiledata = TileData::empty();
        let animations = StaticAnimations::default();
        let at = Point::new(100, 100, 0);
        let mut map = field();
        map.place_static(StaticItem {
            tile: graphic,
            x: at.x,
            y: at.y,
            z: at.z,
            hue: Hue(0),
        });
        let screen_at = stand_on(&camera, at, &sprite.sprite);
        let cursor = cursor_over(&camera, screen_at, 22.0, 30.0);
        let picked = pick(
            &map,
            &camera,
            &tiledata,
            &animations,
            &atlas,
            &Cutaway::OPEN,
            cursor,
        );
        assert_eq!(picked, Some(PickedStatic { at, graphic }));

        let mask = selected(&camera, &tiledata, &animations, &atlas, &Cutaway::OPEN, picked);
        assert_eq!(mask.len(), 1, "the page-one selection has one silhouette quad");
        assert_eq!(mask[0].static_atlas_page(), StaticAtlasPage(1));
        assert_eq!(
            mask[0].region, sprite.sprite.region,
            "the selection samples the page-one region"
        );
    }

    /// Where a sprite of a given size lands on a given tile, stated in numbers
    /// rather than by drawing it.
    ///
    /// This is the arithmetic the whole layer rests on, and it is the kind that
    /// looks right at a glance in either of two wrong forms — centred on the
    /// cell instead of standing on it, or standing on the cell's *top*. Both
    /// draw a plausible town and put every wall half a tile out of place.
    #[test]
    fn a_sprite_stands_centred_on_the_bottom_of_its_cell() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let at = camera.to_screen(Point::new(100, 100, 0));
        assert_eq!((at.x, at.y), (400, 300), "the camera centres its own tile");

        // A 44x44 sprite — a floor tile — covers the cell exactly: the same
        // square the ground's flat art is drawn in.
        let (x, y) = place(&camera, Point::new(100, 100, 0), 44, 44);
        assert_eq!((x, y), (400 - 22, 300 + 22 - 44));

        // A tall, narrow sprite hangs up the screen from the same bottom edge.
        let (x, y) = place(&camera, Point::new(100, 100, 0), 30, 120);
        assert_eq!((x, y), (400 - 15, 300 + 22 - 120));

        // And height lifts it four pixels a unit, exactly as it lifts ground.
        let (_, lifted) = place(&camera, Point::new(100, 100, 10), 44, 44);
        assert_eq!(lifted, 300 + 22 - 44 - 40);
    }

    /// The same placement the collector does, without needing a `WorldMap`.
    fn place(camera: &Camera, point: Point, width: u16, height: u16) -> (i32, i32) {
        let at = camera.to_screen(point);
        (
            at.x - (i32::from(width) >> 1),
            at.y + TILE_HEIGHT / 2 - i32::from(height),
        )
    }

    /// The contract between the animation clock and the atlas, on a real town:
    /// every graphic a static will *show* over a whole cycle is one the atlas was
    /// *offered*.
    ///
    /// Breaking it does not fail loudly — [`collect`] drops a graphic the atlas
    /// has no sprite for, exactly as it does for art the client does not ship —
    /// so a fire would simply vanish for five frames out of six and come back.
    ///
    /// The scene is checked for having something to prove first. A view of
    /// Britain with no animated statics in it would pass this in silence, which
    /// is the false green this repository keeps rediscovering.
    ///
    /// **What it does and does not catch, measured rather than assumed.** It
    /// catches the wiring: `graphics_in` offering the frame on screen instead of
    /// the cycle fails it. It does *not* catch a cycle that is short by one
    /// frame, and that was checked by mutation rather than reasoned about — the
    /// offer is a union over everything on screen, and a fire's neighbours cycle
    /// through the same six graphics, so a frame this static did not ask for was
    /// packed on its neighbour's behalf. The per-graphic property that has no
    /// union to hide in lives beside the clock, in
    /// [`animate`](crate::animate)'s own tests, and both of those do fail on
    /// that mutation. This one is the integration: that the two ends are
    /// connected on a real map.
    #[test]
    fn britain_offers_the_atlas_every_frame_its_fires_will_show() {
        use crate::animate::{FRAME_STEP, StaticAnimations};
        use openshard_uofiles::animdata::AnimData;

        let Some(dir) = std::env::var_os("OPENSHARD_CLIENT").map(std::path::PathBuf::from) else {
            return;
        };
        let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
        let tiledata =
            openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");
        let animdata = AnimData::load(&dir).expect("animdata.mul");
        let mut animations = StaticAnimations::build(&animdata, &tiledata);

        // The forge and the smithy east of the bank, which is where Britain
        // keeps its fires.
        let camera = Camera::new(Point::new(1420, 1683, 0), 768, 512);
        let offered = visible_graphics(&map, &camera, &animations);

        // The scene has something to say. Counted over the *placed* graphics, so
        // this is "there are animated statics on screen" and not "the offer is
        // bigger than the placed set", which the offer is by construction.
        let mut placed = BTreeSet::new();
        graphics_in(
            &map,
            camera.visible_tiles(),
            &StaticAnimations::default(),
            &mut placed,
        );
        let animating = placed
            .iter()
            .filter(|graphic| tiledata.static_tile(graphic.0).flags.is_animated())
            .count();
        assert!(
            animating > 0,
            "nothing on this screen animates: the test proves nothing"
        );
        assert!(
            offered.len() > placed.len(),
            "the cycles added no graphics at all"
        );

        let art = openshard_uofiles::art::Art::open(&dir).expect("artLegacyMUL.uop");
        let atlas = StaticAtlas::build(&art, offered.iter().copied()).expect("a screen of statics fits");
        // Ten seconds, which is longer than the slowest cycle in the file. The
        // count of quads must not move: a graphic that was shown and not packed
        // is a sprite that silently stops being drawn.
        let first = collect(
            &map,
            &camera,
            &tiledata,
            &animations,
            &atlas,
            &Cutaway::OPEN,
            &crate::occlusion::Occlusion::EMPTY,
            None,
            None,
        )
        .quads
        .len();
        assert!(first > 300, "only {first} statics on screen");
        for step in 1..=100 {
            animations.advance(FRAME_STEP);
            let now = collect(
                &map,
                &camera,
                &tiledata,
                &animations,
                &atlas,
                &Cutaway::OPEN,
                &crate::occlusion::Occlusion::EMPTY,
                None,
                None,
            )
            .quads
            .len();
            assert_eq!(
                now, first,
                "a static vanished {step} steps in: shown but never packed"
            );
        }
    }

    /// The four edges of the screen reject, and a pixel either side of each one
    /// is the difference.
    ///
    /// Stated as a boundary rather than as "far away is out": the whole risk in
    /// a cull is that it is one sprite too eager, and a test that places things
    /// a hundred pixels off screen passes with any of the four comparisons
    /// written the wrong way round.
    #[test]
    fn a_sprite_one_pixel_onto_the_screen_is_kept() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let sprite = Sprite {
            width: 30,
            height: 50,
            region: crate::atlas::Region {
                u: 0.0,
                v: 0.0,
                du: 0.0,
                dv: 0.0,
            },
            facing: None,
        };
        let on = |x: f32, y: f32| on_screen(&camera, ViewPoint::new(x, y), &sprite);

        // Off the left: the sprite's right edge is at x + 30, so -30 is the
        // first placement with nothing on screen and -29 is the last with one
        // column of it showing.
        assert!(!on(-30.0, 300.0));
        assert!(on(-29.0, 300.0));
        // Off the right: 800 is the first column past the image.
        assert!(!on(800.0, 300.0));
        assert!(on(799.0, 300.0));
        // Above, where a 250-pixel tree hangs out of its own cell.
        assert!(!on(400.0, -50.0));
        assert!(on(400.0, -49.0));
        // And below.
        assert!(!on(400.0, 600.0));
        assert!(on(400.0, 599.0));
    }

    /// Standing under a roof in Britain draws fewer statics than standing
    /// outside it, and the picture is not empty either way.
    ///
    /// The integration the unit tests in [`crate::cutaway`] cannot do: those
    /// assert what a `Cutaway` decides, this asserts that the collector asks it
    /// — which is a line that can be deleted with every one of them still
    /// green.
    ///
    /// Skipped without the client's files.
    #[test]
    fn a_cutaway_takes_statics_out_of_the_frame() {
        let Some(dir) = std::env::var_os("OPENSHARD_CLIENT").map(std::path::PathBuf::from) else {
            return;
        };
        let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
        let art = openshard_uofiles::art::Art::open(&dir).expect("artLegacyMUL.uop");
        let tiledata =
            openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");

        let camera = Camera::new(Point::new(1495, 1629, 0), 768, 512);
        let animations = StaticAnimations::default();
        let wanted = visible_graphics(&map, &camera, &animations);
        let atlas = StaticAtlas::build(&art, wanted).expect("a screen of statics fits");
        let open = collect(
            &map,
            &camera,
            &tiledata,
            &animations,
            &atlas,
            &Cutaway::OPEN,
            &crate::occlusion::Occlusion::EMPTY,
            None,
            None,
        )
        .quads
        .len();

        // A tile in this quarter of Britain that is under something. Found
        // rather than named, for the reason `cutaway`'s own map test searches:
        // a coordinate written down here is one more thing to be wrong about.
        let indoors = (1620..1640u16)
            .flat_map(|y| (1485..1505u16).map(move |x| (x, y)))
            .find_map(|(x, y)| {
                let z = map.land(x, y)?.z;
                let cutaway = Cutaway::at(&map, &tiledata, Point::new(x, y, z), true);
                (cutaway != Cutaway::OPEN).then_some(cutaway)
            })
            .expect("something in Britain is under a roof");
        let cut = collect(
            &map,
            &camera,
            &tiledata,
            &animations,
            &atlas,
            &indoors,
            &crate::occlusion::Occlusion::EMPTY,
            None,
            None,
        )
        .quads
        .len();

        assert!(cut < open, "the cutaway removed nothing: {cut} of {open}");
        assert!(cut > 0, "the cutaway removed the whole town");
    }

    /// On a real town, the quads come back sorted and every depth agrees with
    /// the sort — the ordering the depth buffer will enforce is the ordering
    /// the collector believes in.
    ///
    /// Two things could break independently here: the sort key, and the
    /// arithmetic that turns it into a depth. Asserting that the depths are
    /// non-increasing across a sorted list is what ties them together, and it
    /// is measured on Britain rather than on a fixture because a fixture's
    /// stack of statics would be one this module's own understanding wrote.
    ///
    /// Skipped without the client's files, like everything else needing a map.
    #[test]
    fn britains_statics_come_back_sorted_from_the_back() {
        let Some(dir) = std::env::var_os("OPENSHARD_CLIENT").map(std::path::PathBuf::from) else {
            return;
        };
        let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
        let art = openshard_uofiles::art::Art::open(&dir).expect("artLegacyMUL.uop");
        let tiledata =
            openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");

        // Britain by the bank: buildings, walls, floors and signs, which is
        // what makes the ordering worth checking here rather than in a field.
        let camera = Camera::new(Point::new(1495, 1629, 0), 768, 512);
        let wanted = visible_graphics(&map, &camera, &StaticAnimations::default());
        assert!(
            wanted.len() > 50,
            "only {} static graphics in the middle of Britain",
            wanted.len(),
        );
        let atlas = StaticAtlas::build(&art, wanted).expect("a screen of statics fits");
        let quads = collect(
            &map,
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            &crate::occlusion::Occlusion::EMPTY,
            None,
            None,
        )
        .quads;
        assert!(quads.len() > 500, "only {} statics on screen", quads.len());

        let mut previous = f32::INFINITY;
        for quad in &quads {
            assert!(
                quad.depth <= previous,
                "a quad at depth {} came after one at {previous}: the sort and the depth disagree",
                quad.depth,
            );
            previous = quad.depth;
        }

        // And the frame actually spans depths, or the assertion above is
        // satisfied by every static sharing one.
        let nearest = quads.last().expect("not empty").depth;
        let furthest = quads.first().expect("not empty").depth;
        assert!(
            furthest - nearest > 1e-4,
            "every static came back at the same depth ({nearest})",
        );
    }

    // **`a_stair_s_mesh_vertices_carry_their_tile_and_reach_its_far_edge` lived
    // here** and went with `push_mesh`, `docs/lighting_rebuild.md` phase 6d:
    // `push_mesh` was `pub(crate)` for exactly two callers, both inside this
    // crate — `statics::collect` and `items::collect` — and the phase removed
    // both, since a real static's position and normal come from the impostor
    // meeting its boxes now, not from a second draw over its own sprite. What
    // this test asserted (a corner carries its static's own tile, and a
    // stair's footprint reaches at least as far as that tile's far edge) is
    // the same claim the four hand-built diagnostic scenes make about their
    // own, separately-built `MeshFaceVertex` lists — `examples/boxes.rs`'s and
    // `examples/synthetic_stair.rs`'s own tests among them — and those did not
    // move.

    /// A tiledata entry with the flags and height a test wants —
    /// `occlusion::tests::tile`'s own shape, which cannot be shared across two
    /// test modules and is two lines.
    fn static_tile(flags: u64, height: u8) -> openshard_tiles::StaticTile {
        openshard_tiles::StaticTile {
            flags: openshard_tiles::TileFlags::new(flags),
            height,
            ..openshard_tiles::StaticTile::default()
        }
    }

    /// A three-tread flight climbing north on tile `(100, 100)`, standing at
    /// `z = 0` — the scene every stair defect in this crate is found on, and the
    /// one `docs/lighting_rebuild.md`'s backlog wants turned into a constructor.
    fn flight() -> (crate::facing::Prism, openshard_tiles::StaticTile) {
        (
            crate::facing::Prism::new(crate::facing::Face::North, &[1, 3, 5])
                .expect("three treads is a legal profile"),
            static_tile(
                openshard_tiles::TileFlags::CLIMBABLE
                    | openshard_tiles::TileFlags::BLOCK
                    | openshard_tiles::TileFlags::NO_SHOOT,
                5,
            ),
        )
    }

    /// A flight stands as one box a tread, each a real volume from the static's
    /// own base to that tread's height — and the impostor's list is the grid's,
    /// copied rather than rebuilt.
    #[test]
    fn a_flight_stands_as_one_volume_a_tread() {
        let (prism, tile) = flight();
        let graphic = Graphic(0x0736);
        let mut builder = crate::occlusion::Builder::new(grid_bounds());
        builder.add(
            100,
            100,
            0,
            graphic,
            &tile,
            crate::occlusion::Shape {
                prism: Some(prism),
                ..crate::occlusion::Shape::UNREAD
            },
        );
        let occlusion = builder.finish(&Cutaway::OPEN);
        let owner = crate::occlusion::Owner::new(0, graphic);

        let mut boxes = Vec::new();
        let shape = crate::occlusion::Shape {
            prism: Some(prism),
            ..crate::occlusion::Shape::UNREAD
        };
        let range = push_volumes(
            &mut boxes,
            Point::new(100, 100, 0),
            &tile,
            &shape,
            owner,
            &occlusion,
        );
        assert_eq!(
            range,
            crate::impostor::Range { offset: 0, count: 3 },
            "three treads, three solids, three boxes"
        );

        // Every box is the space of the solid it names — the whole claim of the
        // function now that there is one shape rather than two.
        for volume in &boxes {
            let solid = occlusion.solid(volume.solid.expect("fixture volume has a solid"));
            assert_eq!(volume.lo.x, solid.space.min.x as f32);
            assert_eq!(volume.lo.y, solid.space.min.y as f32);
            assert_eq!(volume.lo.z, solid.space.min.z as f32);
            assert_eq!(volume.hi.y, solid.space.max.y as f32);
            assert_eq!(volume.hi.z, solid.space.max.z as f32);
        }

        // And they are volumes rather than the surfaces the grid used to hold:
        // every one stands from the static's own base up to its tread's height,
        // and the three heights are the profile.
        assert!(
            boxes.iter().all(|volume| volume.lo.z == 0.0),
            "a tread that does not reach the ground is a surface, not a volume: {boxes:?}"
        );
        let mut tops: Vec<f32> = boxes.iter().map(|volume| volume.hi.z).collect();
        tops.sort_by(f32::total_cmp);
        assert_eq!(tops, [1.0, 3.0, 5.0], "the profile, as three solid heights");

        // And none of them reaches past the tile the static stands on. `==` on
        // the axis crossing the climb, where `Prism::footprint` returns the
        // tile's own integers untouched: a hair either way would be
        // `WIDTH_OVERLAP` come back.
        for volume in &boxes {
            assert_eq!(
                (volume.lo.x, volume.hi.x),
                (100.0, 101.0),
                "a box reached past its tile across the climb: {volume:?}"
            );
        }
    }

    /// **A flight's boxes carry the facing its art named; a body's carry none.**
    ///
    /// The one claim that separates the two questions one mask was answering.
    /// `occlusion::boxes_of` hands a tread `Edges::ANY` on purpose — it picks
    /// the exact slab test a solid takes over a lid's crossing test — and
    /// `statics.wesl` reads `Edges::ANY` in this field as *the art named no
    /// face* and writes no facing at all. Filled from `boxes_of`, the two
    /// sentences met on every staircase in the world: a person reported one at
    /// Britain's `(1454, 1728)` with no shading of its own, lit from every side.
    ///
    /// So the fixture is the pair, and it is the pair that makes it a claim
    /// rather than a reading: **the same tile, the same flags, the same
    /// prism** — only the `facing` the art measured differs. Restore
    /// `boxes_of`'s own mask in `push_volumes` and the first half goes red while
    /// the second stays green, which is the direction that matters: the rule may
    /// not be "a climbable keeps its faces", it has to be "the *art's* answer is
    /// the one this field carries".
    #[test]
    fn a_flights_volumes_name_the_faces_its_art_named_and_a_bodys_name_none() {
        let (prism, tile) = flight();
        let graphic = Graphic(0x0736);
        // What the detector reads off a real flight, and what the art table
        // holds for the stairs the defect was reported on: a stair's base is
        // pixel for pixel what two walls meeting at a corner leave. See
        // `occlusion::boxes_of`'s own doc.
        let corner = crate::facing::Facing::Corner {
            right: crate::facing::Face::East,
            left: crate::facing::Face::South,
        };
        let volumes_of = |shape: crate::occlusion::Shape| {
            let mut builder = crate::occlusion::Builder::new(grid_bounds());
            builder.add(100, 100, 0, graphic, &tile, shape);
            let occlusion = builder.finish(&Cutaway::OPEN);
            let mut boxes = Vec::new();
            push_volumes(
                &mut boxes,
                Point::new(100, 100, 0),
                &tile,
                &shape,
                crate::occlusion::Owner::new(0, graphic),
                &occlusion,
            );
            boxes
        };

        let fitted = volumes_of(crate::occlusion::Shape {
            facing: Some(corner),
            prism: Some(prism),
            ..crate::occlusion::Shape::UNREAD
        });
        assert_eq!(fitted.len(), 3, "three treads, three boxes");
        for volume in &fitted {
            assert_eq!(
                volume.edges,
                crate::occlusion::Edges::EAST | crate::occlusion::Edges::SOUTH,
                "a tread carries what the art named, not what the grid occludes \
                 it with: {volume:?}"
            );
            assert_ne!(
                volume.edges,
                crate::occlusion::Edges::ANY,
                "`Edges::ANY` here is `statics.wesl`'s own \"no facing\", and this \
                 picture named a corner: {volume:?}"
            );
        }

        // The other half, and the reason the first is not just "a climbable is
        // special": a picture the detector would not read is a **body**, and a
        // body genuinely has no face to give a fragment. The prism is still
        // here, so what moved is the art's answer and nothing else.
        let unread = volumes_of(crate::occlusion::Shape {
            prism: Some(prism),
            ..crate::occlusion::Shape::UNREAD
        });
        assert_eq!(unread.len(), 3, "three treads either way");
        for volume in &unread {
            assert_eq!(
                volume.edges,
                crate::occlusion::Edges::ANY,
                "the art named nothing, and this field is where that is said: {volume:?}"
            );
        }
    }

    /// A wall's volume and the grid's own box for it are the same box, because
    /// both come out of `occlusion::boxes_of` — and its `SolidId` is the grid's,
    /// which is what the shadow walk compares.
    #[test]
    fn anything_that_is_not_a_fitted_climbable_stands_as_the_grid_s_own_boxes() {
        let graphic = Graphic(0x0006);
        let tile = static_tile(
            openshard_tiles::TileFlags::WALL
                | openshard_tiles::TileFlags::BLOCK
                | openshard_tiles::TileFlags::NO_SHOOT,
            20,
        );
        let mut builder = crate::occlusion::Builder::new(grid_bounds());
        builder.add(100, 100, 0, graphic, &tile, crate::occlusion::Shape::UNREAD);
        let occlusion = builder.finish(&Cutaway::OPEN);
        let owner = crate::occlusion::Owner::new(0, graphic);

        let mut boxes = Vec::new();
        let range = push_volumes(
            &mut boxes,
            Point::new(100, 100, 0),
            &tile,
            &crate::occlusion::Shape::UNREAD,
            owner,
            &occlusion,
        );

        let grid: Vec<_> = occlusion.pieces_of(100, 100, owner).collect();
        assert_eq!(range.count as usize, grid.len());
        assert!(!grid.is_empty(), "the fixture should stand something up");
        for (volume, (id, solid)) in boxes.iter().zip(&grid) {
            assert_eq!(volume.lo.x, solid.space.min.x as f32);
            assert_eq!(volume.hi.z, solid.space.max.z as f32);
            assert_eq!(volume.solid, Some(*id));
        }
    }

    /// **A static the grid refused still has a shape**, and it is the same shape
    /// — only the name is missing.
    ///
    /// The phase 6c claim, and the one this test asserted the *opposite* of
    /// until then. `Builder::add` refuses everything the tiledata does not mark
    /// `NO_SHOOT` or `WINDOW`, which on one real place at radius 6 was nineteen
    /// of thirty-nine drawn pictures — twelve of them south-facing walls. An
    /// empty range for those is a billboard: the middle of the tile, no facing,
    /// lit from every side. The fixture is deliberately a *wall* rather than a
    /// curiosity, because that is what the measurement found.
    #[test]
    fn a_static_the_grid_refused_still_stands_as_its_own_shape() {
        let graphic = Graphic(0x0006);
        // A wall the grid will not hold: no `NO_SHOOT`, no `WINDOW`, so
        // `occlusion::opacity` answers `CLEAR` and `Builder::add` returns before
        // it pushes anything.
        let tile = static_tile(openshard_tiles::TileFlags::WALL, 20);
        let mut builder = crate::occlusion::Builder::new(grid_bounds());
        builder.add(100, 100, 0, graphic, &tile, crate::occlusion::Shape::UNREAD);
        let occlusion = builder.finish(&Cutaway::OPEN);
        let owner = crate::occlusion::Owner::new(0, graphic);
        assert_eq!(
            occlusion.pieces_of(100, 100, owner).count(),
            0,
            "the fixture should be a static the grid refuses",
        );

        let mut boxes = Vec::new();
        let range = push_volumes(
            &mut boxes,
            Point::new(100, 100, 0),
            &tile,
            &crate::occlusion::Shape::UNREAD,
            owner,
            &occlusion,
        );
        assert_eq!(range.count, 1, "a refused wall is still one body");
        assert_eq!(
            (boxes[0].lo, boxes[0].hi),
            (
                crate::light::WorldVec::new(100.0, 100.0, 0.0),
                crate::light::WorldVec::new(101.0, 101.0, 20.0),
            ),
            "and it is the box the tiledata's own height gives it",
        );
        // And it is a point of nothing, which is the honest name for a shape no
        // shadow ray will ever meet.
        assert_eq!(
            boxes[0].solid, None,
            "a shape the grid refused cannot name a solid of it",
        );
    }
}
