//! Cursor picking and selection masks for map statics.
//!
//! These operations deliberately share the collector's placement helpers: a
//! pixel can only be selected when it is the same pixel the frame draws.

use openshard_map::map::WorldMap;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_tiles::TileData;

use crate::animate::StaticAnimations;
use crate::atlas::StaticArt;
use crate::camera::{self, Camera, RealPixel, TILE_HEIGHT, TileBounds, WorldPixel};
use crate::cutaway::Cutaway;
use crate::depth;
use crate::sprite::SpriteQuad;

use super::{for_each_static_in, on_screen, place, quad_of};

/// One static of the map, named by where it stands and what it is.
///
/// What [`pick`] answers with. Map furniture has no serial, so its tile,
/// height, and graphic are its useful identity for selecting and drawing it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PickedStatic {
    /// Where it stands.
    pub at: Point,
    /// Its placed graphic (the start of an animation cycle, if animated).
    pub graphic: Graphic,
}

/// Which static of the map the cursor is over, or `None` for none.
///
/// A hit is an opaque texel; when pictures overlap, the topmost drawn one wins.
///
/// Answered as a [`depth::Hit`], because the map's furniture is not the only
/// list the cursor is tested against and the caller has to know which of two
/// answers the frame drew in front — see that type.
#[must_use]
pub fn pick<'a>(
    map: &WorldMap,
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: impl Into<StaticArt<'a>>,
    cutaway: &Cutaway,
    cursor: RealPixel,
) -> Option<depth::Hit<PickedStatic>> {
    pick_with_interior(map, camera, tiledata, animations, atlas, cutaway, cursor, None)
}

/// [`pick`] with the same building-cell gate as the static collector.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn pick_with_interior<'a>(
    map: &WorldMap,
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: impl Into<StaticArt<'a>>,
    cutaway: &Cutaway,
    cursor: RealPixel,
    interior: Option<&crate::interiors::InteriorFrame>,
) -> Option<depth::Hit<PickedStatic>> {
    let atlas = atlas.into();
    let in_view = camera.to_view(camera.pick(cursor));
    let mut hit: Option<depth::Hit<PickedStatic>> = None;
    for_each_static_in(map, pick_bounds(camera, atlas, cursor), |item| {
        let at = Point::new(item.x, item.y, item.z);
        let tile = tiledata.static_tile(item.tile.0);
        if !interior.is_none_or(|frame| frame.shows_static_at(at, tile)) {
            return;
        }
        let graphic = item.tile;
        // Foliage remains pickable even if the collector is fading it.
        let Some(placed) = place(at, graphic, camera, tiledata, animations, atlas, cutaway, None) else {
            return;
        };
        let (Ok(x), Ok(y)) = (
            u16::try_from(in_view.x - placed.at.x as i32),
            u16::try_from(in_view.y - placed.at.y as i32),
        ) else {
            return;
        };
        if !atlas.opaque_at(placed.showing, x, y) {
            return;
        }
        // A later equal-order item is drawn last and therefore wins.
        if hit.is_none_or(|best| placed.order >= best.order) {
            hit = Some(depth::Hit {
                order: placed.order,
                what: PickedStatic { at, graphic },
            });
        }
    });
    hit
}

/// A conservative tile rectangle for statics whose sprite can cover `cursor`.
fn pick_bounds<'a>(camera: &Camera, atlas: impl Into<StaticArt<'a>>, cursor: RealPixel) -> TileBounds {
    let atlas = atlas.into();
    let cursor = camera.pick(cursor);
    let (width, height) = atlas.max_sprite_size();
    let half_width = (i32::from(width) + 1) / 2;
    let height = i32::from(height);
    let points = [
        WorldPixel {
            x: cursor.x - half_width,
            y: cursor.y - height - TILE_HEIGHT / 2,
        },
        WorldPixel {
            x: cursor.x + half_width,
            y: cursor.y + TILE_HEIGHT / 2,
        },
    ];
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_y = i32::MAX;
    let mut max_y = i32::MIN;
    for point in points {
        for z in [i8::MIN, i8::MAX] {
            let (x, y) = camera::unproject(point, z);
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
    }
    TileBounds {
        min_x: min_x - 1,
        max_x: max_x + 1,
        min_y: min_y - 1,
        max_y: max_y + 1,
    }
}

/// The mask quad for a selected static, or an empty list without a selection.
pub fn selected<'a>(
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: impl Into<StaticArt<'a>>,
    cutaway: &Cutaway,
    selection: Option<PickedStatic>,
) -> Vec<SpriteQuad> {
    let atlas = atlas.into();
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
                None,
            )?;
            on_screen(camera, placed.at, &placed.sprite).then(|| {
                quad_of(
                    picked.at,
                    &placed,
                    base,
                    0,
                    crate::occlusion::OwnerId::NONE,
                    crate::impostor::Range::default(),
                )
            })
        })
        .into_iter()
        .collect()
}
