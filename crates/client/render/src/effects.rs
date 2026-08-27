//! A flying arrow or bolt: one ephemeral sprite crossing a straight line in
//! continuous world space — the one sprite in this renderer that is not
//! tile-snapped, because a projectile has no tile to snap to until it lands.
//!
//! It rides the static atlas exactly as a ground item does (`0x0F42` and
//! `0x1BFE` are ordinary item graphics), but skips [`crate::items::collect`]'s
//! whole occlusion-aware walk: there are at most a handful of these on screen
//! at once, none is an occluder, and none is a thing a click can land on
//! ([`Place::NOWHERE`]).

use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;

use crate::atlas::StaticArt;
use crate::camera::{Camera, WorldSpot, project_exact};
use crate::depth;
use crate::geometry::{Rect, Vec2};
use crate::occlusion::OwnerId;
use crate::place::Place;
use crate::sprite::SpriteQuad;

/// One frame's snapshot of an in-flight effect. The clock that turns elapsed
/// time into `progress` is presentation state's own — this is only ever asked
/// to draw the instant it is given.
#[derive(Clone, Copy, Debug)]
pub struct FlyingArrow {
    /// The shot's own sprite.
    pub art: Graphic,
    /// Where it left from.
    pub from: Point,
    /// Where it is bound.
    pub to: Point,
    /// `0.0` at `from`, `1.0` once it has arrived.
    pub progress: f32,
}

/// Build one quad per in-flight effect, positioned by linearly interpolating
/// between its two endpoints in world space rather than snapping to a tile.
/// An effect whose art the atlas has not packed is dropped, the same "nothing
/// to draw" a mobile with no frame gets.
pub fn collect<'a>(
    effects: &[FlyingArrow],
    camera: &Camera,
    atlas: impl Into<StaticArt<'a>>,
) -> Vec<SpriteQuad> {
    let atlas = atlas.into();
    let (eye_x, eye_y) = camera.eye_tile();
    let base = depth::base_for(eye_x, eye_y);
    effects
        .iter()
        .filter_map(|effect| quad_for(effect, camera, atlas, base))
        .collect()
}

fn quad_for(effect: &FlyingArrow, camera: &Camera, atlas: StaticArt<'_>, base: i32) -> Option<SpriteQuad> {
    let sprite = atlas.paged_sprite(effect.art)?;
    let from = WorldSpot::centre(effect.from);
    let to = WorldSpot::centre(effect.to);
    let t = f64::from(effect.progress.clamp(0.0, 1.0));
    let spot = WorldSpot {
        x: from.x + (to.x - from.x) * t,
        y: from.y + (to.y - from.y) * t,
        z: from.z + (to.z - from.z) * t,
    };
    // `stand_on`'s own arithmetic (`statics.rs`), against a continuous point
    // instead of a tile's projected centre — a static sprite has no anchor of
    // its own, only its width and height, so this is the whole convention.
    let at = camera.to_view_exact(camera.snap(project_exact(spot)));
    let rotation = flight_angle(from, to);
    let size = rotated_bounds(
        f32::from(sprite.sprite.width),
        f32::from(sprite.sprite.height),
        rotation,
    );
    // The unrotated sprite's centre is at `(at.x, at.y + HALF_TILE_HEIGHT -
    // height / 2)`. Keep that exact centre, then grow the quad to the rotated
    // picture's bounds; otherwise aiming the same arrow in another direction
    // makes it visibly orbit its flight path.
    let centre = Vec2::new(
        at.x,
        at.y + (crate::camera::TILE_HEIGHT / 2) as f32 - f32::from(sprite.sprite.height) / 2.0,
    );
    let order = depth::Order {
        tile: spot.x.floor() as i32 + spot.y.floor() as i32,
        // A shot in the air rises one above the ground under it, the same
        // reason a mobile does (`depth::mobile_priority_z`) — it is a thing
        // standing over the tile, not a marking on it.
        priority_z: depth::mobile_priority_z(spot.z.round() as i8),
    };
    Some(
        SpriteQuad {
            rect: Rect {
                x: centre.x - size.x / 2.0,
                y: centre.y - size.y / 2.0,
                width: size.x,
                height: size.y,
            },
            region: sprite.sprite.region,
            depth: order.to_depth(base),
            hue: 0,
            place: Place::NOWHERE,
            twin: 0,
            owner: u32::from(OwnerId::NONE.raw()),
            volumes: crate::impostor::Range { offset: 0, count: 0 },
        }
        .with_static_atlas_page(sprite.page)
        .with_screen_rotation(rotation),
    )
}

/// Clockwise screen-space rotation that turns this art's left-facing resting
/// direction toward the shot. `project_exact` is deliberately used rather
/// than world `x`/`y`: one tile east and one tile south have different
/// directions on the isometric screen.
fn flight_angle(from: WorldSpot, to: WorldSpot) -> f32 {
    let from = project_exact(from);
    let to = project_exact(to);
    let dx = (to.x - from.x) as f32;
    let dy = (to.y - from.y) as f32;
    if dx == 0.0 && dy == 0.0 {
        return 0.0;
    }
    // This is ClassicUO's `atan2(-offset.Y, -offset.X)`. The arrow art rests
    // pointing left, and positive angles rotate clockwise in screen axes.
    (-dy).atan2(-dx)
}

/// The smallest axis-aligned screen rectangle containing a `width` by
/// `height` picture after a clockwise turn.
fn rotated_bounds(width: f32, height: f32, radians_clockwise: f32) -> Vec2 {
    let (sin, cos) = radians_clockwise.sin_cos();
    Vec2::new(
        width * cos.abs() + height * sin.abs(),
        width * sin.abs() + height * cos.abs(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flight_angle_uses_the_isometric_screen_vector() {
        let from = WorldSpot::centre(Point::new(10, 10, 0));
        let east = WorldSpot::centre(Point::new(11, 10, 0));
        let south = WorldSpot::centre(Point::new(10, 11, 0));

        assert!((flight_angle(from, east) + 3.0 * std::f32::consts::FRAC_PI_4).abs() < 0.000_1);
        assert!((flight_angle(from, south) + std::f32::consts::FRAC_PI_4).abs() < 0.000_1);
    }

    #[test]
    fn rotated_bounds_keep_the_picture_inside_its_quad() {
        let straight = rotated_bounds(10.0, 4.0, 0.0);
        assert_eq!(straight, Vec2::new(10.0, 4.0));

        let quarter = rotated_bounds(10.0, 4.0, std::f32::consts::FRAC_PI_2);
        assert!((quarter.x - 4.0).abs() < 0.000_1);
        assert!((quarter.y - 10.0).abs() < 0.000_1);
    }
}
