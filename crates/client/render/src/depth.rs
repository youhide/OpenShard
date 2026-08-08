//! Which of two things standing on the same screen pixel is in front.
//!
//! Ground and statics are two passes over two atlases with two different quad
//! shapes, so "draw the far one first" is not something a single sort can
//! arrange: the passes run whole, one after the other, and painter's order
//! inside a pass says nothing about the pass next door. A hill would then be
//! covered by every wall behind it, because *all* the ground is drawn before
//! *any* static.
//!
//! So the ordering is a number each quad carries and the depth test enforces.
//! Both passes write it, both test it, and the pass order stops mattering — the
//! same property the shader's flat/slope branch has: it cannot disagree with
//! itself, because there is only one decision.
//!
//! # The number is the client's
//!
//! Ported from ClassicUO, which builds the same ordering as a per-tile linked
//! list: `Chunk.AddGameObject` computes a `PriorityZ` per object and
//! `View.CalculateDepthZ` folds it together with the tile into
//! `(x + y) + (127 + z) * 0.01f`. The two parts are what matter and they are
//! kept apart here — an integer key, sorted by tile depth first and height
//! second — because their float form overflows into the next tile at large `z`
//! and we would inherit a bug we can simply not have.
//!
//! What is *not* ported is `PriorityZ` for anything that does not exist yet:
//! mobiles, corpses, effects and multis all adjust it, and each one belongs
//! with whatever draws it.

use openshard_protocol::world::Point;
use openshard_uofiles::tiledata::StaticTile;

/// Key units one step of tile depth (`x + y`) is worth.
///
/// Big enough that no `PriorityZ` can reach from one tile depth into the next:
/// `z` is an `i8` and the adjustments below move it by at most one, so the whole
/// range is 259 values and this is 512.
const DEPTH_PER_TILE: i32 = 512;

/// How many tiles of depth either side of the camera the buffer resolves.
///
/// The visible span is roughly a hundred tiles, so this is a wide margin rather
/// than a tuned bound: a tile beyond it clamps, which merges its ordering with
/// the frontmost or hindmost tile and is invisible off-screen anyway.
const DEPTH_TILES: i32 = 512;

/// Half the key range one frame's depth buffer covers.
const HALF_RANGE: f32 = (DEPTH_TILES * DEPTH_PER_TILE) as f32;

/// Where a thing sorts, before it is turned into a depth value.
///
/// Two numbers rather than one: the tile it stands on, and how high it stands
/// there. Comparing tuples is what the client's float formula was trying to
/// express and what it loses at the ends of the `z` range.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Order {
    /// `x + y`: how far down the screen the tile is. Larger is nearer.
    pub tile: i32,
    /// The client's `PriorityZ`. Larger is nearer.
    pub priority_z: i32,
}

impl Order {
    /// The depth value a quad writes, in the range clip space wants.
    ///
    /// `base` is the tile depth the camera is centred on, which is what keeps
    /// the whole visible frame in the middle of the range where the resolution
    /// is spent: a facet is 11,264 tiles deep and a depth buffer that had to
    /// span all of it would give neighbouring tiles adjacent representable
    /// values.
    ///
    /// Nearer is *smaller*, because the depth test is `Less` — the ordinary
    /// convention, and the one that makes an untouched depth buffer (1.0) mean
    /// "nothing has been drawn here yet".
    pub fn to_depth(self, base: i32) -> f32 {
        let key = (self.tile - base) * DEPTH_PER_TILE + self.priority_z;
        // A tile beyond the range clamps rather than wrapping into the wrong
        // order: an object that far away is off screen, and a wrong *clamp* is
        // invisible where a wrong *wrap* would draw a distant wall in front of
        // the player.
        (0.5 - key as f32 / (2.0 * HALF_RANGE)).clamp(0.0, 1.0)
    }
}

/// The tile depth a camera centre sits at — the `base` for [`Order::to_depth`].
///
/// `i32` and not `u16`: a free camera is a pixel and not a tile, so the tile it
/// is over comes from [`crate::camera::unproject`] and may be off the map
/// entirely. That is harmless here — the base only says where the middle of the
/// depth range is, and `DEPTH_TILES` is 512 either side of it.
pub fn base_for(x: i32, y: i32) -> i32 {
    x + y
}

/// Where a land tile sorts.
///
/// `Chunk.AddGameObject`, the `Land` arm: a stretched tile takes its average
/// height and a flat one its own, and either way two is subtracted — so ground
/// sits under everything standing on it, including a static at the very same
/// `z`. That subtraction is the whole reason a floor tile and the flagstone
/// static on top of it do not fight.
///
/// `corners` is [`crate::ground::GroundQuad::corners`] order: top, right, left,
/// bottom.
pub fn land_priority_z(corners: [i32; 4]) -> i32 {
    let [top, right, left, bottom] = corners;
    let stretched = corners.iter().any(|z| *z != top);
    let average = if !stretched {
        top
    } else if (top - bottom).abs() <= (left - right).abs() {
        // `>> 1` and not `/ 2`: the client floors towards minus infinity here
        // and the two differ for an odd negative sum, which is every other
        // tile of a dungeon floor.
        (top + bottom) >> 1
    } else {
        (left + right) >> 1
    };
    average - 2
}

/// Where a static sorts on the tile it stands on.
///
/// `Chunk.AddGameObject`'s `default` arm: a background tile drops one below
/// whatever is at its height, and anything with a height of its own rises one
/// above it. Both are what stop a wall from being drawn behind the floor it
/// stands on and a rug from being drawn on top of the chair standing on it.
///
/// `IsMultiMovable` is deliberately absent: nothing here draws multis yet, and
/// the flag is only meaningful for them.
pub fn static_priority_z(z: i8, tile: &StaticTile) -> i32 {
    let mut priority_z = i32::from(z);
    // ClassicUO's `IsBackground` is `TileFlag.Background`, `0x00000001` — the
    // same bit this workspace named `FLOOR` after Sphere's `UFLAG1_FLOOR`. One
    // bit, two names, and the value is what is asserted.
    if tile.flags.has(openshard_uofiles::tiledata::TileFlags::FLOOR) {
        priority_z -= 1;
    }
    if tile.height != 0 {
        priority_z += 1;
    }
    priority_z
}

/// Where a mobile sorts on the tile it stands on.
///
/// `Chunk.AddGameObject`'s `Mobile` arm, which is one line: a mobile rises one
/// above everything at its height. That is what puts a player in front of the
/// floor tile under them and in front of a rug, and behind the wall of the tile
/// in front — the tile part of the ordering, not this, is what does the second.
pub fn mobile_priority_z(z: i8) -> i32 {
    i32::from(z) + 1
}

/// Which tile depth a mobile sorts at, given the one it is walking off.
///
/// # A body between two tiles belongs to the nearer of them
///
/// `View.CalculateDepthZ` is where the client says so, and it says it in a form
/// that hides the rule: the reference keeps `Mobile.X/Y` on the tile a step
/// *started* from until the step finishes, and then eight arms of a `switch`
/// advance `x` and `y` by the direction the sprite is drifting. Read as a whole,
/// the eight arms compute one thing — the larger of the two tiles' `x + y`.
///
/// Which is the only choice that can be right. The sprite spans both tiles for
/// the whole crossing, so whichever tile it sorts behind will be drawn over it;
/// the far tile is the one that is *behind* the body on screen, so sorting there
/// puts the ground the body is walking onto, and every wall standing on it, in
/// front of the walker. Taking the destination unconditionally does exactly that
/// for the four headings that climb the screen — north, west, north-west and the
/// north-east half of the horizontal pair — where the destination is the farther
/// tile. The body sinks into the ground it is stepping off, for as long as the
/// step lasts, and stands up again when it lands.
///
/// The `+ 1` when the two tiles share a depth is the reference's, and it is the
/// two headings that move straight across the screen: north-east and south-west
/// swap a step of `x` for a step of `y`, so both tiles are at one depth and the
/// client lifts the walker one clear of *both*. Nothing else is at that key, so
/// it costs nothing and it stops a body crossing a wall's own row from being
/// swallowed by it.
///
/// `from` is absent when nothing is in flight, which is the still body's case:
/// its own tile, and no adjustment.
pub fn mobile_tile(at: Point, from: Option<Point>) -> i32 {
    let key = |p: Point| i32::from(p.x) + i32::from(p.y);
    let Some(from) = from else {
        return key(at);
    };
    match key(from) == key(at) {
        true => key(at) + 1,
        false => key(from).max(key(at)),
    }
}

#[cfg(test)]
mod tests {
    use openshard_uofiles::tiledata::TileFlags;

    use super::*;

    /// The bit ClassicUO calls `Background` and this workspace calls `FLOOR`.
    /// Pinned beside the code that relies on them being the same bit, because
    /// the two names are the kind of thing a later reader "fixes".
    #[test]
    fn background_and_floor_are_the_same_bit() {
        assert_eq!(TileFlags::FLOOR, 0x0000_0001);
    }

    fn tile(flags: u64, height: u8) -> StaticTile {
        StaticTile {
            flags: TileFlags::new(flags),
            height,
            ..StaticTile::default()
        }
    }

    /// The three cases of the static arm, stated as the client's arithmetic.
    #[test]
    fn a_static_sorts_by_its_height_and_two_flags() {
        assert_eq!(static_priority_z(5, &tile(0, 0)), 5);
        // A wall: it has a height, so it rises above anything lying at its z.
        assert_eq!(static_priority_z(5, &tile(0, 20)), 6);
        // A floor tile: flat and background, so it sinks below them.
        assert_eq!(static_priority_z(5, &tile(TileFlags::FLOOR, 0)), 4);
        // Both at once cancel, which is the client's behaviour and not an
        // accident of ordering here.
        assert_eq!(static_priority_z(5, &tile(TileFlags::FLOOR, 20)), 5);
    }

    /// `Order` — and the depth value it becomes — is a sort key, not a linear
    /// world height: two statics at different `z` on the same tile can fold to
    /// the same `priority_z`, and once they do, `to_depth` cannot tell them
    /// apart either. Nothing downstream can walk this value back to a `z`, and
    /// nothing today tries to — `blit.wgsl` never samples the depth texture,
    /// only `place`. See `docs/gbuffer.md`'s first "Not settled" item, answered
    /// against this.
    #[test]
    fn priority_z_can_collide_for_two_different_world_heights() {
        // A flat, backgroundless static at z=5 and a wall (a nonzero height)
        // one lower at z=4: the wall's own +1 lands it on the same key.
        let flat = static_priority_z(5, &tile(0, 0));
        let wall = static_priority_z(4, &tile(0, 20));
        assert_eq!(flat, wall, "two different world heights folded to one key");

        let same_tile = base_for(100, 100);
        let order_flat = Order {
            tile: same_tile,
            priority_z: flat,
        };
        let order_wall = Order {
            tile: same_tile,
            priority_z: wall,
        };
        assert_eq!(order_flat.to_depth(same_tile), order_wall.to_depth(same_tile));
    }

    /// Ground goes under the things standing on it, and it is the *subtraction*
    /// that puts it there rather than the pass order.
    #[test]
    fn ground_sorts_below_a_static_at_the_same_height() {
        let ground = land_priority_z([10; 4]);
        assert!(ground < static_priority_z(10, &tile(0, 0)));
        // Even a background static, which sinks a level of its own.
        let rug = static_priority_z(10, &tile(TileFlags::FLOOR, 0));
        assert!(ground < rug, "ground is under a rug");
    }

    /// A slope takes the average of its shallower axis, floored. The `>> 1`
    /// against `/ 2` distinction only shows on an odd negative sum, so that is
    /// what is asserted rather than a tidy positive pair.
    #[test]
    fn a_slope_averages_its_shallower_axis() {
        // Top and bottom differ by 2, left and right by 10: the top-bottom
        // axis is the shallower one, so it is the one averaged.
        assert_eq!(land_priority_z([10, 0, 10, 12]), 11 - 2);
        // Left and right differ by 2, top and bottom by 10.
        assert_eq!(land_priority_z([0, 10, 12, 10]), 11 - 2);
        // Odd and negative, on the shallower axis: floored, so -4 and not -3.
        // `(-5 + -2) >> 1` is -4 where `/ 2` would be -3, and a dungeon floor
        // is full of odd negative pairs.
        assert_eq!(land_priority_z([-5, 10, 0, -2]), -4 - 2);
    }

    /// A body mid-step sorts at the nearer of the two tiles it is between, and
    /// the table is `View.CalculateDepthZ`'s eight arms read back as keys.
    ///
    /// The four headings up the screen are the ones this is really about: the
    /// destination is *behind* the walker there, and sorting him at it draws the
    /// tile he is leaving — and every wall standing on it — over him.
    #[test]
    fn a_body_mid_step_sorts_at_the_nearer_of_the_two_tiles() {
        let from = Point::new(100, 100, 0);
        let step = |dx: i32, dy: i32| {
            let at = Point::new((100 + dx) as u16, (100 + dy) as u16, 0);
            mobile_tile(at, Some(from))
        };
        let here = 200;
        // Down the screen: the destination is nearer, and taken at once.
        assert_eq!(step(1, 0), here + 1, "east");
        assert_eq!(step(0, 1), here + 1, "south");
        assert_eq!(step(1, 1), here + 2, "south-east");
        // Up the screen: the tile being left is nearer, and kept for the whole
        // crossing. This is the bug the rule exists for.
        assert_eq!(step(-1, 0), here, "west");
        assert_eq!(step(0, -1), here, "north");
        assert_eq!(step(-1, -1), here, "north-west");
        // Straight across: one tile depth, and the walker is lifted clear of it.
        assert_eq!(step(1, -1), here + 1, "north-east");
        assert_eq!(step(-1, 1), here + 1, "south-west");
    }

    /// A body with nothing in flight is on its own tile, with no lift: the
    /// mid-step rule must not leak into standing, or every idle body would sort
    /// one tile nearer than the ground under it.
    #[test]
    fn a_standing_body_sorts_at_its_own_tile() {
        assert_eq!(mobile_tile(Point::new(100, 100, 0), None), 200);
    }

    /// Nearer is a smaller depth, which is what makes the `Less` test and the
    /// 1.0 clear value mean what they say.
    #[test]
    fn nearer_things_get_a_smaller_depth() {
        let base = base_for(1000, 1000);
        let far = Order {
            tile: 1999,
            priority_z: 0,
        };
        let near = Order {
            tile: 2001,
            priority_z: 0,
        };
        assert!(near.to_depth(base) < far.to_depth(base));

        // And on one tile, height decides the same way.
        let low = Order {
            tile: 2000,
            priority_z: 0,
        };
        let high = Order {
            tile: 2000,
            priority_z: 1,
        };
        assert!(high.to_depth(base) < low.to_depth(base));
    }

    /// Two adjacent keys have to be *representable* as two different depths,
    /// or the ordering this module computes is thrown away by the buffer. A
    /// 24-bit depth buffer resolves about 6e-8, and one key step is 1e-6.
    #[test]
    fn one_step_of_priority_is_well_clear_of_the_buffers_resolution() {
        let base = base_for(1000, 1000);
        let step = Order {
            tile: 2000,
            priority_z: 0,
        }
        .to_depth(base)
            - Order {
                tile: 2000,
                priority_z: 1,
            }
            .to_depth(base);
        assert!(step > 1e-6, "one priority step is only {step}");
        // And every depth this camera produces is inside clip space, which is
        // what the clamp is for.
        for tile in [1000, 2000, 3000] {
            let depth = Order { tile, priority_z: 0 }.to_depth(base);
            assert!((0.0..=1.0).contains(&depth), "{tile} is at {depth}");
        }
    }

    /// The whole point of centring on the camera: a tile far outside the range
    /// clamps instead of wrapping around into the wrong order.
    #[test]
    fn a_tile_beyond_the_range_clamps_rather_than_wrapping() {
        let base = base_for(1000, 1000);
        let behind = Order {
            tile: 0,
            priority_z: 0,
        };
        let ahead = Order {
            tile: 11_000,
            priority_z: 0,
        };
        assert_eq!(behind.to_depth(base), 1.0);
        assert_eq!(ahead.to_depth(base), 0.0);
    }
}
