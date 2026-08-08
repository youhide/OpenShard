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

use std::collections::BTreeSet;

use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_uofiles::map::Map;
use openshard_uofiles::tiledata::TileData;

use crate::animate::StaticAnimations;
use crate::atlas::{Sprite, StaticAtlas};
use crate::camera::{Camera, TILE_HEIGHT, TileBounds};
use crate::cutaway::{self, Cutaway};
use crate::depth;
use crate::geometry::{Rect, Vec2};
use crate::mesh_face::{MeshFaceRow, MeshFaceVertex};
use crate::sprite::SpriteQuad;

/// Where a sprite standing on a tile lands, in viewport pixels.
///
/// The arithmetic in this module's own header, named so that it has one copy:
/// a static read out of the map and an item the server put on the ground are
/// the same picture standing the same way, and the second is
/// [`crate::items`]. Centred on the tile's column, bottom edge on the
/// diamond's bottom vertex — `View.DrawStatic`.
pub fn stand_on(camera: &Camera, at: Point, sprite: &Sprite) -> Vec2 {
    let at = camera.to_screen(at);
    Vec2::new(
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
pub fn on_screen(camera: &Camera, at: Vec2, sprite: &Sprite) -> bool {
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
pub fn visible_graphics(map: &Map, camera: &Camera, animations: &StaticAnimations) -> BTreeSet<Graphic> {
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
    map: &Map,
    bounds: TileBounds,
    animations: &StaticAnimations,
    out: &mut BTreeSet<Graphic>,
) {
    for_each_static_in(map, bounds, |item| {
        out.extend(animations.cycle(Graphic(item.tile)));
    });
}

/// One static of the map, named by where it stands and what it is.
///
/// What [`pick`] answers with, and the only thing this crate can say about a
/// static: the map's furniture has no serial — a serial is an entity's, and
/// these are not entities — so a *reference* to one is its tile, its height and
/// its graphic. That is enough to place its picture again, which is all a
/// selection needs; it is deliberately not enough to ask a shard about, because
/// there is nothing there to ask about.
///
/// Two identical graphics on one tile at one height are one value here and draw
/// one picture, so nothing downstream can tell them apart and nothing needs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PickedStatic {
    /// Where it stands.
    pub at: Point,
    /// Its graphic — the *placed* one, which for an animated static is the
    /// cycle's start rather than the frame on screen. The same value
    /// [`collect`] sorts and looks up tiledata by.
    pub graphic: Graphic,
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
#[derive(Debug, Default)]
pub struct StaticGeometry {
    /// The pictures, back to front — what this function returned before mesh
    /// geometry existed, unchanged.
    pub quads: Vec<SpriteQuad>,
    /// Raw vertices for every visible climbable static's [`crate::mesh::Mesh`],
    /// six per face ([`crate::mesh::Face::fan`]) —
    /// [`crate::renderer::MeshFaceRenderer::render`]'s own input.
    pub mesh_vertices: Vec<MeshFaceVertex>,
    /// One row per face represented in `mesh_vertices`, addressed by a
    /// vertex's own `id`.
    pub mesh_rows: Vec<MeshFaceRow>,
}

/// `occlusion` is **this frame's own grid**, and it has to have been built
/// already: what each row carries beside its picture is the number that grid gave
/// the static it draws ([`crate::occlusion::Occlusion::owner_at`]), which is the
/// join `docs/lighting_height.md` phase 3 pays for. A frame that collected its
/// statics first would be stamping numbers from the frame before it.
pub fn collect(
    map: &Map,
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &StaticAtlas,
    cutaway: &Cutaway,
    occlusion: &crate::occlusion::Occlusion,
) -> StaticGeometry {
    let (eye_x, eye_y) = camera.eye_tile();
    let base = depth::base_for(eye_x, eye_y);
    let mut quads: Vec<(depth::Order, SpriteQuad)> = Vec::new();
    let mut mesh_vertices = Vec::new();
    let mut mesh_rows = Vec::new();

    for_each_static_in(map, camera.visible_tiles(), |item| {
        let at = Point::new(item.x, item.y, item.z);
        let Some(placed) = place(
            at,
            Graphic(item.tile),
            camera,
            tiledata,
            animations,
            atlas,
            cutaway,
        ) else {
            return;
        };
        if !on_screen(camera, placed.at, &placed.sprite) {
            return;
        }
        // The *placed* graphic and not `placed.showing`: the grid keyed its
        // owner off the same one, and an animated static would otherwise change
        // owner every hundred milliseconds. See `occlusion::Owner`.
        let owner = occlusion.owner_at(i32::from(at.x), i32::from(at.y), at.z, Graphic(item.tile));
        let quad = quad_of(at, &placed, base, u32::from(item.hue), owner);
        if let Some(prism) = &placed.prism {
            push_mesh(
                &mut mesh_vertices,
                &mut mesh_rows,
                camera,
                at,
                prism,
                quad.depth,
                owner,
            );
        }
        quads.push((placed.order, quad));
    });

    // Back to front, and a *stable* sort on the order alone: two statics on one
    // tile at one `PriorityZ` keep the order the file has them in, which is the
    // order the client inserted them into its per-tile list and therefore the
    // order it draws them. The depth test is `LessEqual`, so later drawn wins
    // the tie — see `renderer::depth_state`. Sorting by the graphic as well
    // would be just as deterministic and would resolve those ties by an
    // accident of the art's numbering.
    quads.sort_by_key(|(order, _)| *order);
    StaticGeometry {
        quads: quads.into_iter().map(|(_, quad)| quad).collect(),
        mesh_vertices,
        mesh_rows,
    }
}

/// Push one climbable static's honest faces — a top and a riser per tread,
/// [`crate::facing::Prism::mesh`] — as raw, fan-triangulated vertices.
///
/// `depth` is the enclosing [`SpriteQuad`]'s own, reused rather than
/// recomputed: a second depth formula here is a second chance to disagree
/// with the one that already decided this static's pixels
/// (`docs/gbuffer.md` decision 4).
///
/// `pub(crate)` and not private because [`crate::items`] wants the same
/// correction for a placement that came from the server's own list rather
/// than the map's — [`Placed::prism`] is set by [`place`] either way, and a
/// second copy of this loop would be a second place the two could disagree.
///
/// `owner` is the enclosing static's own, and every face of the flight gets it —
/// one `Builder::add` is one owner however many solids it pushed, which is
/// `docs/lighting_height.md` phase 3's first decision seen from the drawing side.
pub(crate) fn push_mesh(
    vertices: &mut Vec<MeshFaceVertex>,
    rows: &mut Vec<MeshFaceRow>,
    camera: &Camera,
    at: Point,
    prism: &crate::facing::Prism,
    depth: f32,
    owner: crate::occlusion::OwnerId,
) {
    let mesh = prism.mesh(i32::from(at.x), i32::from(at.y), i32::from(at.z));
    for face in mesh.faces() {
        let id = rows.len() as u32;
        rows.push(MeshFaceRow {
            tile: (at.x, at.y),
            stance: crate::place::Stance::of_normal(face.normal)
                .expect("Prism::mesh only ever produces normals Stance::of_normal recognizes"),
            owner: u32::from(owner.raw()),
        });
        for corner in face.fan() {
            let screen = camera.to_view_exact(crate::camera::project_exact(corner));
            vertices.push(MeshFaceVertex {
                screen,
                world: [corner.x as f32, corner.y as f32, corner.z as f32],
                depth,
                id,
                tile: [at.x as f32, at.y as f32],
            });
        }
    }
}

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
    pub(crate) at: Vec2,
    /// The atlas entry for the frame it is showing.
    pub(crate) sprite: Sprite,
    /// That frame's graphic, which is what the atlas is keyed by — not the
    /// placed one, which for an animated static is only the cycle's start.
    pub(crate) showing: Graphic,
    /// Which way its picture faces — a rug on the ground is as flat as a floor
    /// built into the map. See [`crate::place::Stance`].
    pub(crate) stance: crate::place::Stance,
    /// The solid this static's art is a picture of, if the client's own
    /// `CLIMBABLE` bit and the atlas's own measurement agree it has one —
    /// see [`crate::atlas::StaticAtlas::prism`]'s own doc for why the bit
    /// comes first. `docs/gbuffer.md` step 4c reads this to build the
    /// static's honest [`crate::mesh::Mesh`]; nothing else uses it yet.
    pub(crate) prism: Option<crate::facing::Prism>,
}

/// Place one static, or `None` when there is nothing on screen for it: hidden by
/// the cutaway, or a graphic the atlas holds no art for.
///
/// `graphic` is the *placed* one and not the frame on screen. It decides the sort
/// and the tiledata lookup: a fire's frames are different art of the same size
/// standing in the same place, and ordering by whichever one is showing would let
/// a stack reshuffle itself every hundred milliseconds.
pub(crate) fn place(
    at: Point,
    graphic: Graphic,
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &StaticAtlas,
    cutaway: &Cutaway,
) -> Option<Placed> {
    let tile = tiledata.static_tile(graphic.0);
    if !cutaway::shows(cutaway, at.z, tile) {
        return None;
    }
    let showing = animations.showing(graphic);
    let sprite = atlas.sprite(showing)?;
    Some(Placed {
        order: depth::Order {
            tile: i32::from(at.x) + i32::from(at.y),
            priority_z: depth::static_priority_z(at.z, tile),
        },
        // The cell's centre, height folded in: `to_screen` already lifts `z` by
        // four pixels a unit, which is the same lift the ground gets.
        at: stand_on(camera, at, &sprite),
        sprite,
        showing,
        // A floor's pixels are spread across its tile, a wall's run along the one
        // edge it stands on, and anything else claims the tile's middle. The
        // tiledata answers the first; the *art* answers the second, measured once
        // when the atlas packed this sprite. See `crate::place::Stance` and
        // `crate::facing`.
        stance: crate::place::Stance::of(tile, sprite.facing),
        // The bit first, the atlas's own measurement second — the same
        // order `occlusion::Builder::add`'s climbable branch already checks
        // in, and the same reason `StaticAtlas::prism`'s own doc gives: a
        // wall can score well against some prism by accident, and only the
        // client's own flag says a static is one at all.
        prism: tile.flags.is_climbable().then(|| atlas.prism(showing)).flatten(),
    })
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
    }
}

/// Which static of the map the cursor is over, or `None` for none.
///
/// [`crate::items::pick`]'s two rules, over the map's furniture rather than over
/// the server's list, and for the same reasons — stated there once:
///
/// - **A hit is an opaque texel**, not a bounding box.
/// - **The topmost drawn wins**, which is the largest [`depth::Order`]; a tie
///   goes to the one the map file has last, which is the one drawn last and so
///   the one on top.
///
/// The cells walked are [`Camera::visible_tiles`]' — the same set [`collect`]
/// draws — so what can be picked is what was drawn, cutaway included: a wall on
/// a storey this frame is not showing is not something the player can have
/// pointed at.
///
/// `cursor` is a viewport pixel, the pair `winit` reports and [`Camera::pick`]
/// takes; the zoom is undone here, once.
#[must_use]
pub fn pick(
    map: &Map,
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &StaticAtlas,
    cutaway: &Cutaway,
    cursor: (i32, i32),
) -> Option<PickedStatic> {
    let in_view = camera.to_view(camera.pick(cursor.0, cursor.1));
    let mut hit: Option<(depth::Order, PickedStatic)> = None;
    for_each_static_in(map, camera.visible_tiles(), |item| {
        let at = Point::new(item.x, item.y, item.z);
        let graphic = Graphic(item.tile);
        let Some(placed) = place(at, graphic, camera, tiledata, animations, atlas, cutaway) else {
            return;
        };
        // Into the sprite's own pixels. Negative is above or left of it, and
        // `try_from` failing is the whole of that test.
        let (Ok(x), Ok(y)) = (
            u16::try_from(in_view.x - placed.at.x as i32),
            u16::try_from(in_view.y - placed.at.y as i32),
        ) else {
            return;
        };
        if !atlas.opaque_at(placed.showing, x, y) {
            return;
        }
        // `>=`, so a later static at the same order takes it: the tie-break is
        // the file's order, which is the order the picture was drawn in.
        if hit.is_none_or(|(order, _)| placed.order >= order) {
            hit = Some((placed.order, PickedStatic { at, graphic }));
        }
    });
    hit.map(|(_, picked)| picked)
}

/// The quads to draw a mask from, for a static that is selected.
///
/// The *same* quads [`collect`] draws — same placement, same region, same depth,
/// through the same [`quad_of`] — because a mask drawn from anything else would
/// shade pixels the picture is not at. See [`crate::items::outlined`], which is
/// this for the server's list and exists for the same reason.
///
/// A list rather than an `Option` because the pass that consumes it takes one,
/// and because the day a second static is selected this is where it is appended.
/// `None` comes back empty rather than being a case the caller has to handle.
///
/// The hue is nought: a mask is a shape, and the shape is the alpha.
pub fn selected(
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &StaticAtlas,
    cutaway: &Cutaway,
    selection: Option<PickedStatic>,
) -> Vec<SpriteQuad> {
    let (eye_x, eye_y) = camera.eye_tile();
    let base = depth::base_for(eye_x, eye_y);
    selection
        .and_then(|picked| {
            let placed = place(
                picked.at,
                picked.graphic,
                camera,
                tiledata,
                animations,
                atlas,
                cutaway,
            )?;
            match on_screen(camera, placed.at, &placed.sprite) {
                true => Some(quad_of(
                    picked.at,
                    &placed,
                    base,
                    0,
                    crate::occlusion::OwnerId::NONE,
                )),
                false => None,
            }
        })
        .into_iter()
        .collect()
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
pub(crate) fn for_each_static_in(
    map: &Map,
    bounds: TileBounds,
    mut each: impl FnMut(&openshard_uofiles::map::StaticItem),
) {
    let Some((xs, ys)) = bounds.clamp_to(map.width(), map.height()) else {
        return;
    };
    // A row at a time and not a tile at a time. The order is the same one — the
    // map hands a row back in ascending `x`, which is what the tile walk did —
    // and the saving is that a row of a block is one binary search rather than
    // eight: **this walk was 0.98ms of the 2.30ms a widest-zoom frame spent
    // building its occlusion grid**, and 35,000 of its 35,000 tile lookups were
    // asked of a map that is mostly open ground. See `Map::statics_in_row`.
    let (from_x, to_x) = (*xs.start(), *xs.end());
    for y in ys {
        for item in map.statics_in_row(y, from_x, to_x) {
            each(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use openshard_uofiles::color::Color16;
    use openshard_uofiles::image::Image;
    use openshard_uofiles::map::{LandCell, StaticItem};

    use super::*;

    /// A map big enough for a camera at (100, 100), with flat ground and nothing
    /// standing on it. Statics are placed by the tests that want them.
    fn field() -> Map {
        Map::from_blocks(16, 16, |_, _| LandCell { tile: 3, z: 0 })
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

    /// The viewport pixel a point in the drawn image sits at — the inverse of
    /// what [`pick`] undoes, so a test can click on a sprite it has placed.
    fn cursor_over(camera: &Camera, at: Vec2, dx: f32, dy: f32) -> (i32, i32) {
        let spot = camera.to_viewport(crate::camera::ViewPixel {
            x: (at.x + dx) as i32,
            y: (at.y + dy) as i32,
        });
        (spot.x as i32, spot.y as i32)
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
            tile: graphic.0,
            x: 100,
            y: 100,
            z: 0,
            hue: 0,
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
                tile: graphic.0,
                x,
                y,
                z: 0,
                hue: 0,
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
            tile: graphic.0,
            x: 100,
            y: 100,
            z: 20,
            hue: 0,
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
            tile: graphic.0,
            x: stands.x,
            y: stands.y,
            z: stands.z,
            hue: 0,
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
        let (x, y) = crate::camera::unproject(camera.pick(cursor.0, cursor.1), 0);
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
            tile: graphic.0,
            x: 101,
            y: 99,
            z: 5,
            hue: 0,
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

    /// The same placement the collector does, without needing a `Map`.
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
        let map = Map::load_facet(&dir, 0).expect("Felucca");
        let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");
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
        let on = |x: f32, y: f32| on_screen(&camera, Vec2::new(x, y), &sprite);

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
        let map = Map::load_facet(&dir, 0).expect("Felucca");
        let art = openshard_uofiles::art::Art::open(&dir).expect("artLegacyMUL.uop");
        let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");

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
        let map = Map::load_facet(&dir, 0).expect("Felucca");
        let art = openshard_uofiles::art::Art::open(&dir).expect("artLegacyMUL.uop");
        let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");

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

    /// [`push_mesh`]'s vertices carry the enclosing static's own tile, and a
    /// stair's footprint reaches at least as far as that tile's own far edge
    /// — [`crate::facing::widen_footprint`] pushes the tile-crossing side
    /// past it by a hair more. `mesh_face.wgsl`'s fragment stage leans on
    /// both: it now subtracts this `tile` from a fragment's exact world
    /// position instead of taking `fract()` of the position alone, because
    /// `fract` of anything at or past a whole tile past `tile` wraps back
    /// toward `0` — reading a corner on this stair's own far edge as sitting
    /// on the *next* tile's near one instead. That was the shadow-raymarch
    /// anomaly `docs/lighting.md` tracks: one fully-lit pixel on an otherwise
    /// evenly-shadowed flat tread, where every neighbouring sample read
    /// partly blocked.
    #[test]
    fn a_stair_s_mesh_vertices_carry_their_tile_and_reach_its_far_edge() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let prism = crate::facing::Prism::new(crate::facing::Face::North, &[5, 5, 5])
            .expect("three treads is a legal profile");
        let at = Point::new(100, 100, 0);
        let mut vertices = Vec::new();
        let mut rows = Vec::new();
        push_mesh(
            &mut vertices,
            &mut rows,
            &camera,
            at,
            &prism,
            0.0,
            crate::occlusion::OwnerId::NONE,
        );

        assert!(!vertices.is_empty(), "a three-tread stair has faces to draw");
        for vertex in &vertices {
            assert_eq!(
                vertex.tile,
                [100.0, 100.0],
                "every corner of a static's mesh should carry the static's own tile"
            );
        }

        let max_offset = vertices
            .iter()
            .flat_map(|v| [v.world[0] - v.tile[0], v.world[1] - v.tile[1]])
            .fold(f32::MIN, f32::max);
        assert!(
            max_offset >= 1.0 - 1e-6,
            "expected some corner at or past the tile's far edge (offset >= 1.0), got {max_offset}",
        );
    }
}
