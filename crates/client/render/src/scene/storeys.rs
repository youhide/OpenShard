//! Multi-level lighting fixtures: cellars, floor planes, and rooms above rooms.

use openshard_protocol::world::Point;

use super::{
    CENTRE,
    FLOOR,
    ROOM_HALF,
    Scene,
    SceneTile,
    TORCH,
    WALL,
    WALL_EAST,
    WALL_HEIGHT,
    empty,
    room_wall_tiles,
};
use crate::atlas::StaticAtlas;
use crate::items::GroundItem;

/// How far below street level the cellar torch burns.
pub const CELLAR_DEPTH: i8 = -(7 * 11);

/// A torch far below an otherwise empty street.
pub fn cellar_under_street() -> Scene {
    let mut scene = empty("a torch in a cellar under an empty street");
    scene.items.push(GroundItem {
        amount:  openshard_protocol::items::ItemAmount::ONE,
        at:      Point::new(CENTRE.x, CENTRE.y, CELLAR_DEPTH),
        graphic: TORCH,
        hue:     openshard_protocol::wire::Hue::NONE,
    });
    scene
}

/// Ground-floor torch location in the two-storey fixtures.
pub const STOREY_TORCH: SceneTile = SceneTile::new(CENTRE.x - 2, CENTRE.y);
/// Upper-storey observation point in the two-storey fixtures.
pub const STOREY_SPOT: SceneTile = SceneTile::new(CENTRE.x + 2, CENTRE.y);
/// Height of that upper-storey observation point.
pub const STOREY_Z: f32 = WALL_HEIGHT as f32 + 5.0;

/// A two-storey house with a torch on the ground floor.
pub fn storey_over_a_torch() -> Scene {
    floored("a torch on the ground floor of a two-storey house", None)
}

/// The omitted floor tile in [`hole_in_a_floor`].
pub const FLOOR_HOLE: SceneTile = SceneTile::new(CENTRE.x + 1, CENTRE.y);

/// The same two-storey house with one floor plank missing.
pub fn hole_in_a_floor() -> Scene {
    floored("a two-storey house with a plank missing", Some(FLOOR_HOLE))
}

fn floored(name: &'static str, hole: Option<SceneTile>) -> Scene {
    let (cx, cy) = (CENTRE.x, CENTRE.y);
    let mut scene = empty(name);
    for tile in room_wall_tiles() {
        scene = scene.with(tile, WALL).with_at(tile, WALL_HEIGHT as i8, WALL);
    }
    for x in cx - ROOM_HALF + 1..=cx + ROOM_HALF - 1 {
        for y in cy - ROOM_HALF + 1..=cy + ROOM_HALF - 1 {
            if hole != Some(SceneTile::new(x, y)) {
                scene = scene.with_at(SceneTile::new(x, y), WALL_HEIGHT as i8, FLOOR);
            }
        }
    }
    scene.with(STOREY_TORCH, TORCH)
}

/// The wall whose face the split-level room fixture reads.
pub const STOREY_WALL: SceneTile = CENTRE;
/// The inset sub-tile coordinate used by the sprite shader for a face pixel.
pub const INSIDE: f32 = 126.0 / 127.0;
/// Torch tile in the lower, lit room.
pub const LIT_ROOM_TORCH: SceneTile = SceneTile::new(CENTRE.x + 2, CENTRE.y);
/// Height of that torch: a wall sconce under the floor.
pub const LIT_ROOM_SCONCE: i8 = 10;

/// A lit room with a second storey, used to test light at the floor seam.
pub fn storey_over_a_lit_room() -> Scene {
    let (cx, cy) = (CENTRE.x, CENTRE.y);
    let mut scene = empty("a lit room with a second storey over it");
    for y in cy - ROOM_HALF..=cy + ROOM_HALF {
        scene = scene.with(SceneTile::new(cx, y), WALL_EAST).with_at(
            SceneTile::new(cx, y),
            WALL_HEIGHT as i8,
            WALL_EAST,
        );
        for x in cx + 1..=cx + ROOM_HALF {
            scene = scene.with_at(SceneTile::new(x, y), WALL_HEIGHT as i8, FLOOR);
        }
    }
    scene.art = Some(
        StaticAtlas::pack([(
            WALL_EAST,
            crate::facing::silhouette(crate::facing::Face::East, WALL_HEIGHT.into()),
        )])
        .expect("one silhouette fits"),
    );
    scene.with_at(LIT_ROOM_TORCH, LIT_ROOM_SCONCE, TORCH)
}
