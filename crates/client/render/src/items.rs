//! Placing what the server has dropped on the ground.
//!
//! The fourth CPU-side collector, and the one that is two of the others put
//! together: an item's *picture* is a static's — the same art, the same atlas,
//! the same "centred on the column, standing on the diamond's bottom vertex" —
//! while its *source* is a mobile's, a list somebody else built out of what
//! arrived on the wire. Nothing in the map says an item is there; a `0x1A` does,
//! and a `0x1D` takes it away again.
//!
//! So the placement is [`crate::statics::stand_on`] rather than a second copy of
//! it, and what is written here is only the part that differs: where the list
//! comes from, and that it is sorted for the depth buffer.
//!
//! # Not the same thing as a static at the same tile
//!
//! Both draw through [`StaticAtlas`], and a client that merged the two lists
//! would be right until an item is picked up: the map's statics are the shard's
//! furniture and never move, while these come and go with every `0x1A`. Two
//! lists, one atlas.

use std::collections::BTreeSet;

use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Point;
use openshard_uofiles::tiledata::TileData;

use crate::animate::StaticAnimations;
use crate::atlas::StaticAtlas;
use crate::camera::Camera;
use crate::cutaway::Cutaway;
use crate::depth;
use crate::sprite::SpriteQuad;
use crate::statics::{Placed, on_screen, quad_of};

/// One thing lying on the ground, as the client has been told about it.
///
/// A plain value and not a handle into a `WorldView`, for the reason
/// [`Mobile`](crate::mobiles::Mobile) is one: this crate renders what it is
/// given and owns no model of the world. The stack's *amount* is deliberately
/// absent — a pile of 500 gold is one sprite, and which sprite is the caller's
/// question, not the renderer's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GroundItem {
    /// Where it lies.
    pub at: Point,
    /// Its graphic, which is a static's graphic: the two share an art file and
    /// therefore an atlas.
    pub graphic: Graphic,
    /// Its hue, or [`Hue::NONE`] for none.
    pub hue: Hue,
}

/// The hue an item is drawn in while the cursor is over it.
///
/// ClassicUO's `Constants.HIGHLIGHT_CURRENT_OBJECT_HUE`
/// (`src/ClassicUO.Client/Game/Constants.cs`), and it is a hue rather than an
/// outline because that is the only thing this pass can say: the shader has one
/// `hue` per sprite and a `hues.mul` ramp *replaces* the art's colour (see
/// [`crate::hue`]), so a highlighted barrel is drawn in the highlight ramp
/// entire — which is exactly what the reference does with `partial = false`.
///
/// Full and not partial deliberately. A partial hue leaves anything the art
/// painted in its own colour alone, so a door with a brass handle would be
/// highlighted everywhere except where the eye is looking.
pub const HIGHLIGHT_HUE: Hue = Hue(0x0014);

/// Every distinct graphic a set of items needs packed.
///
/// A `BTreeSet` and not a `Vec`, to be unioned with
/// [`statics::visible_graphics`](crate::statics::visible_graphics) before the
/// atlas is built: one atlas serves both passes, so the two sets are asked for
/// together and packed once.
///
/// An item's whole cycle, for the reason
/// [`statics::graphics_in`](crate::statics::graphics_in) asks for one: a torch
/// dropped on the ground animates exactly as a torch built into the map does,
/// and the atlas has to hold every graphic it will turn into.
pub fn needed_graphics(items: &[GroundItem], animations: &StaticAnimations) -> BTreeSet<Graphic> {
    items
        .iter()
        .flat_map(|item| animations.cycle(item.graphic))
        .collect()
}

/// The quads for every item whose graphic the atlas holds.
///
/// One the atlas has no sprite for is dropped, exactly as in
/// [`statics::collect`](crate::statics::collect): the client ships no art for
/// it, or the atlas was built before this item arrived. Both are "nothing to
/// draw", and drawing it from a neighbouring graphic would be worse.
///
/// `cutaway` is the same one the statics are tested against, and it is the same
/// test: the client reads a ground item's tiledata row out of the static table,
/// so a barrel on the floor above is hidden with that floor exactly as a wall
/// built into the map is.
///
/// `highlight` is the index [`pick`] answered with, drawn in
/// [`HIGHLIGHT_HUE`] instead of its own — the item the cursor is over. An index
/// and not a flag on the item, because being pointed at is a fact about *this
/// frame* and not about the thing: the caller's list is a projection of what the
/// server said, and writing a highlight into it would put the cursor's position
/// inside the record of the world.
///
/// Also the honest mesh for any item that is a climbable, prism-fit static —
/// [`crate::statics::StaticGeometry`], the same shape [`crate::statics::collect`]
/// returns and for the same reason. A stair the server placed rather than the
/// map is exactly as stepped as one built into it, and `Placed::prism` does not
/// know which list its own placement came from — only [`place`] does, and both
/// callers read the same field. Before this, a climbable *item* fell through to
/// the flat corner-stance reading forever, with no honest mesh pass ever built
/// for it: this function threw `placed.prism` away.
///
/// `occlusion` is this frame's own grid, already built — see
/// [`crate::statics::collect`], which takes it for the same reason and states it.
// Eight, and every one of them is a different source this frame reads: the list,
// the camera, two tables, the atlas, the cutaway, the pick, the grid. There is no
// pair among them that belongs in one struct — [`crate::statics::collect`] takes
// the same seven off the map instead of off a list — so a grouping here would be
// a bag named after the argument count rather than after anything.
#[allow(clippy::too_many_arguments)]
pub fn collect(
    items: &[GroundItem],
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &StaticAtlas,
    cutaway: &Cutaway,
    highlight: Option<usize>,
    occlusion: &crate::occlusion::Occlusion,
) -> crate::statics::StaticGeometry {
    let (eye_x, eye_y) = camera.eye_tile();
    let base = depth::base_for(eye_x, eye_y);
    let mut quads: Vec<(depth::Order, SpriteQuad)> = Vec::new();
    let mut mesh_vertices = Vec::new();
    let mut mesh_rows = Vec::new();

    for (index, item) in items.iter().enumerate() {
        let Some(placed) = place(item, camera, tiledata, animations, atlas, cutaway) else {
            continue;
        };
        if !on_screen(camera, placed.at, &placed.sprite) {
            continue;
        }
        let order = placed.order;
        // The highlight replaces the item's own hue rather than combining with
        // it: one wire hue reaches the shader, and the reference does the same
        // — `ItemView.Draw` overwrites `hue` and clears `partial` when the
        // object is the selected one.
        let hue = match highlight == Some(index) {
            true => u32::from(HIGHLIGHT_HUE.0),
            false => u32::from(item.hue.0),
        };
        let owner = occlusion.owner_at(
            i32::from(item.at.x),
            i32::from(item.at.y),
            item.at.z,
            item.graphic,
        );
        let quad = quad_of(item.at, &placed, base, hue, owner);
        if let Some(prism) = &placed.prism {
            crate::statics::push_mesh(
                &mut mesh_vertices,
                &mut mesh_rows,
                camera,
                item.at,
                prism,
                quad.depth,
                owner,
            );
        }
        quads.push((order, quad));
    }

    // Back to front, and a *stable* sort on the order alone: two items on one
    // tile at one `PriorityZ` keep the caller's order, which is by serial —
    // the order the server sent them and so the order the client's own
    // per-tile list holds them in. The depth test is `LessEqual`, so the later
    // one wins the tie; see `renderer::depth_state`.
    quads.sort_by_key(|(order, _)| *order);
    crate::statics::StaticGeometry {
        quads: quads.into_iter().map(|(_, quad)| quad).collect(),
        mesh_vertices,
        mesh_rows,
    }
}

/// The quads to draw a silhouette from, for the item the cursor is over.
///
/// A list rather than an `Option` because the pass takes one — and because the
/// day a second thing is highlighted (a target cursor's victim, a container's
/// contents) this is where it is appended, with no change on either side. Their
/// order in the list is the ring identity the silhouette pass numbers them by,
/// so two outlined items that overlap on screen come back ringed separately
/// rather than as one blob; see [`crate::outline`].
///
/// The quads are the *same* quads [`collect`] draws — same placement, same
/// region, same depth, through the same [`quad_of`] — because the silhouette
/// has to land exactly on the picture. The hue is the item's own and is never
/// read: the silhouette shader wants the shape, and the shape is the alpha.
///
/// `highlight` is what [`pick`] answered. `None` is the ordinary frame, and it
/// comes back empty rather than being a case the caller has to handle.
pub fn outlined(
    items: &[GroundItem],
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &StaticAtlas,
    cutaway: &Cutaway,
    highlight: Option<usize>,
) -> Vec<SpriteQuad> {
    let (eye_x, eye_y) = camera.eye_tile();
    let base = depth::base_for(eye_x, eye_y);
    highlight
        .and_then(|index| Some((items.get(index)?, index)))
        .and_then(|(item, _)| {
            let placed = place(item, camera, tiledata, animations, atlas, cutaway)?;
            match on_screen(camera, placed.at, &placed.sprite) {
                true => Some(quad_of(
                    item.at,
                    &placed,
                    base,
                    u32::from(item.hue.0),
                    crate::occlusion::OwnerId::NONE,
                )),
                false => None,
            }
        })
        .into_iter()
        .collect()
}

/// Place one item, or `None` when there is nothing on screen for it: hidden by
/// the cutaway, or a graphic the atlas holds no art for.
///
/// [`statics::place`](crate::statics::place) and nothing else. A ground item is
/// ordered as a static is, and from the same table — the client reads
/// `tiledata`'s *static* entry for an item's graphic too, so a wall lying on the
/// floor and a wall built into the map sort alike — so the placement is that one
/// rather than a second copy of it. See [`Placed`](crate::statics::Placed) for
/// why there is only one.
fn place(
    item: &GroundItem,
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &StaticAtlas,
    cutaway: &Cutaway,
) -> Option<Placed> {
    crate::statics::place(
        item.at,
        item.graphic,
        camera,
        tiledata,
        animations,
        atlas,
        cutaway,
    )
}

/// Which item the cursor is over: an index into `items`, or `None` for none.
///
/// **The picture is what is hit, not the tile.** A door's leaf is drawn on its
/// own tile and stands two tiles up the screen from it, so a click on the leaf
/// unprojects to the tile *behind* the door — pick by tile and a player can
/// never open the door they are pointing at. That is the same reason the client
/// itself picks against the drawn sprite.
///
/// Two rules, both taken from the frame rather than restated:
///
/// - **A hit is an opaque texel**, not a bounding box — see
///   [`StaticAtlas::opaque_at`]. Static art is mostly empty space, and a box test
///   picks whatever tall thing the cursor is merely *inside*.
/// - **The topmost drawn wins.** That is the largest [`depth::Order`], and on a
///   tie the *later* item in `items`, which is exactly what the depth test does
///   to two sprites at one order — see [`collect`]'s sort.
///
/// `cursor` is a viewport pixel, the same pair `winit` reports and
/// [`Camera::pick`] takes; the zoom is undone here, once.
#[must_use]
pub fn pick(
    items: &[GroundItem],
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &StaticAtlas,
    cutaway: &Cutaway,
    cursor: (i32, i32),
) -> Option<usize> {
    let in_view = camera.to_view(camera.pick(cursor.0, cursor.1));
    let mut hit: Option<(depth::Order, usize)> = None;
    for (index, item) in items.iter().enumerate() {
        let Some(placed) = place(item, camera, tiledata, animations, atlas, cutaway) else {
            continue;
        };
        // Into the sprite's own pixels. Negative is above or left of it, and
        // `try_from` failing is the whole of that test.
        let (Ok(x), Ok(y)) = (
            u16::try_from(in_view.x - placed.at.x as i32),
            u16::try_from(in_view.y - placed.at.y as i32),
        ) else {
            continue;
        };
        if !atlas.opaque_at(placed.showing, x, y) {
            continue;
        }
        // `>=`, so a later item at the same order takes it: the tie-break is the
        // caller's order, and the one drawn last is the one on top.
        if hit.is_none_or(|(order, _)| placed.order >= order) {
            hit = Some((placed.order, index));
        }
    }
    hit.map(|(_, index)| index)
}

#[cfg(test)]
mod tests {
    use openshard_uofiles::color::Color16;
    use openshard_uofiles::image::Image;

    use super::*;
    // Where a sprite of this size lands, which the assertions below are stated
    // against: the tests place a cursor over a picture they have placed
    // themselves, and this is the one arithmetic that says where that is.
    use crate::statics::stand_on;

    /// An atlas holding one graphic at a known size.
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

    /// An item lands where a static of the same size on the same tile does.
    ///
    /// The assertion is the *comparison* rather than two numbers: the placement
    /// has one copy now, and this is what says the item pass is using it. Two
    /// numbers here would go on passing if a second copy appeared and drifted.
    #[test]
    fn an_item_stands_exactly_where_a_static_of_its_size_would() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0EED);
        let atlas = atlas(graphic, 30, 50);
        let tiledata = TileData::empty();

        let quads = collect(
            &[GroundItem {
                at: Point::new(100, 100, 0),
                graphic,
                hue: Hue::NONE,
            }],
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            None,
            &crate::occlusion::Occlusion::EMPTY,
        );
        assert_eq!(quads.quads.len(), 1);
        let sprite = atlas.sprite(graphic).expect("packed");
        let at = stand_on(&camera, Point::new(100, 100, 0), &sprite);
        assert_eq!((quads.quads[0].rect.x, quads.quads[0].rect.y), (at.x, at.y));
        assert_eq!(
            (quads.quads[0].rect.width, quads.quads[0].rect.height),
            (30.0, 50.0)
        );
    }

    /// Height lifts an item off the floor the way it lifts everything else, and
    /// it lifts its depth with it — a coin on a table is in front of the table's
    /// own tile, not behind it.
    #[test]
    fn a_higher_item_is_drawn_higher_and_nearer() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0EED);
        let atlas = atlas(graphic, 30, 50);
        let tiledata = TileData::empty();
        let at = |z: i8| GroundItem {
            at: Point::new(100, 100, z),
            graphic,
            hue: Hue::NONE,
        };
        let floor = collect(
            &[at(0)],
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            None,
            &crate::occlusion::Occlusion::EMPTY,
        );
        let table = collect(
            &[at(10)],
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            None,
            &crate::occlusion::Occlusion::EMPTY,
        );
        assert_eq!(
            table.quads[0].rect.y,
            floor.quads[0].rect.y - 40.0,
            "four pixels a unit"
        );
        assert!(table.quads[0].depth < floor.quads[0].depth, "smaller is nearer");
    }

    /// An item whose graphic is not packed is dropped rather than drawn from
    /// whatever else is in the atlas.
    #[test]
    fn an_item_with_no_sprite_is_dropped() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = atlas(Graphic(0x0EED), 30, 50);
        let tiledata = TileData::empty();
        let quads = collect(
            &[GroundItem {
                at: Point::new(100, 100, 0),
                graphic: Graphic(0x0EEE),
                hue: Hue::NONE,
            }],
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            None,
            &crate::occlusion::Occlusion::EMPTY,
        );
        assert!(quads.quads.is_empty());
    }

    /// Two items on different tiles come back with the further one first, so a
    /// pass that ignored the depth buffer entirely would still paint them in the
    /// right order.
    #[test]
    fn the_quads_come_back_from_the_back() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0EED);
        let atlas = atlas(graphic, 30, 50);
        let tiledata = TileData::empty();
        let item = |x: u16, y: u16| GroundItem {
            at: Point::new(x, y, 0),
            graphic,
            hue: Hue::NONE,
        };
        // Given nearest first, on purpose.
        let quads = collect(
            &[item(101, 101), item(99, 99)],
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            None,
            &crate::occlusion::Occlusion::EMPTY,
        );
        assert_eq!(quads.quads.len(), 2);
        assert!(
            quads.quads[0].depth > quads.quads[1].depth,
            "the far one is drawn first"
        );
    }

    /// The viewport pixel a point in the drawn image sits at — the inverse of
    /// what [`pick`] undoes, so a test can click on a sprite it has placed.
    fn cursor_over(camera: &Camera, at: crate::geometry::Vec2, dx: f32, dy: f32) -> (i32, i32) {
        let spot = camera.to_viewport(crate::camera::ViewPixel {
            x: (at.x + dx) as i32,
            y: (at.y + dy) as i32,
        });
        (spot.x as i32, spot.y as i32)
    }

    /// Art with a hole in it: the left half transparent, the right half drawn.
    /// A door's leaf is this shape and so is most static art — which is the
    /// whole reason picking is a texel test and not a rectangle one.
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

    #[test]
    fn a_click_on_an_item_s_own_pixels_picks_it() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0675);
        let atlas = holed(graphic, 40, 60);
        let tiledata = TileData::empty();
        let item = GroundItem {
            at: Point::new(100, 100, 0),
            graphic,
            hue: Hue::NONE,
        };
        let sprite = atlas.sprite(graphic).expect("packed");
        let at = stand_on(&camera, item.at, &sprite);
        let pick_at = |dx, dy| {
            pick(
                std::slice::from_ref(&item),
                &camera,
                &tiledata,
                &StaticAnimations::default(),
                &atlas,
                &Cutaway::OPEN,
                cursor_over(&camera, at, dx, dy),
            )
        };
        assert_eq!(pick_at(30.0, 30.0), Some(0), "the drawn half was not hit");
        assert_eq!(
            pick_at(5.0, 30.0),
            None,
            "the transparent half of the picture was picked — this is a box test, not a texel one"
        );
        assert_eq!(pick_at(-5.0, 30.0), None, "a pixel left of the sprite was picked");
        assert_eq!(pick_at(30.0, 70.0), None, "a pixel below the sprite was picked");
    }

    /// Two doors of a shopfront overlap on screen. The one drawn on top is the
    /// one the click gets — the same answer the depth buffer gives the frame,
    /// which is what the player sees.
    #[test]
    fn the_topmost_item_wins_an_overlap() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0675);
        // Tall enough that the nearer tile's sprite covers the further one's.
        let atlas = atlas(graphic, 44, 120);
        let tiledata = TileData::empty();
        let item = |x: u16, y: u16| GroundItem {
            at: Point::new(x, y, 0),
            graphic,
            hue: Hue::NONE,
        };
        // Given furthest first, which is *not* what decides it: the order does.
        let items = [item(100, 100), item(101, 101)];
        let sprite = atlas.sprite(graphic).expect("packed");
        let near = stand_on(&camera, items[1].at, &sprite);
        let found = pick(
            &items,
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
            found,
            Some(1),
            "the door behind was picked through the one in front"
        );
    }

    /// The item the cursor is over is drawn in the highlight ramp, and only
    /// that one. The index is the same one [`pick`] hands back, so what the
    /// player sees lit is what a double-click would use.
    #[test]
    fn the_highlighted_item_is_drawn_in_the_highlight_hue() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0675);
        let atlas = atlas(graphic, 40, 60);
        let tiledata = TileData::empty();
        // Its own hue, so the assertion below is "replaced" and not "set".
        let item = |x: u16| GroundItem {
            at: Point::new(x, 100, 0),
            graphic,
            hue: Hue(0x03B2),
        };
        let items = [item(100), item(101)];
        let quads = collect(
            &items,
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            Some(1),
            &crate::occlusion::Occlusion::EMPTY,
        );
        assert_eq!(quads.quads.len(), 2);
        // The pass sorts back to front, so the nearer one — index 1, a tile
        // south-east — is the second quad.
        assert_eq!(
            quads.quads[1].hue,
            u32::from(HIGHLIGHT_HUE.0),
            "the pointed-at item"
        );
        assert_eq!(quads.quads[0].hue, 0x03B2, "and nothing else changed colour");
    }

    /// Nothing under the cursor is a legitimate answer and the common one: most
    /// of the screen is ground.
    #[test]
    fn a_click_on_bare_ground_picks_nothing() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0675);
        let atlas = atlas(graphic, 40, 60);
        let tiledata = TileData::empty();
        let item = GroundItem {
            at: Point::new(100, 100, 0),
            graphic,
            hue: Hue::NONE,
        };
        let sprite = atlas.sprite(graphic).expect("packed");
        let at = stand_on(&camera, item.at, &sprite);
        assert_eq!(
            pick(
                std::slice::from_ref(&item),
                &camera,
                &tiledata,
                &StaticAnimations::default(),
                &atlas,
                &Cutaway::OPEN,
                cursor_over(&camera, at, 200.0, 200.0),
            ),
            None
        );
    }

    /// The graphics a list needs, once each, ready to be unioned with the map's.
    #[test]
    fn the_needed_graphics_are_deduplicated() {
        let item = |graphic: u16| GroundItem {
            at: Point::new(0, 0, 0),
            graphic: Graphic(graphic),
            hue: Hue::NONE,
        };
        let wanted = needed_graphics(
            &[item(0x0EED), item(0x0EED), item(0x0EEA)],
            &StaticAnimations::default(),
        );
        assert_eq!(
            wanted.into_iter().collect::<Vec<_>>(),
            vec![Graphic(0x0EEA), Graphic(0x0EED)],
        );
    }
}
