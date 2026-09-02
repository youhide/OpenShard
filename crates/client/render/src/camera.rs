//! Where a tile lands on the screen, and how to get back.
//!
//! UO's world is a square grid seen from a fixed diagonal, so the projection is
//! two multiplications and no matrix at all. Every number here is the client's,
//! and the client's numbers are not a choice we get to make: a tile is 44 pixels
//! across, a step in `x` moves half a tile right and half a tile down, a step in
//! `y` moves half a tile left and half a tile down, and a unit of height lifts
//! the tile four pixels. Change one of them and the art stops meeting itself at
//! the seams — which is visible only as a shimmer along the diagonals, not as an
//! error, so the values are pinned by tests rather than trusted.
//!
//! # Three spaces, and a type for two of them
//!
//! - **Tile space** — [`Point`], the server's, and the only one that goes on the
//!   wire.
//! - **World pixels** — [`WorldPixel`], what [`project`] returns. The origin is
//!   tile `(0, 0, 0)`, it is unbounded in both directions, and no camera is in
//!   it at all.
//! - **View pixels** — [`ViewPixel`], where a thing lands in the image the world
//!   is drawn into. Origin at its top-left.
//!
//! The two pixel spaces are the same two `i32`s and two different meanings, so
//! they are two types: adding a zoom to one of them while the other still means
//! the first is the kind of mistake a shared type cannot catch. Neither gets
//! `From` or `Into` — the only thing allowed to move between them is a
//! [`Camera`], and a conversion that needs a camera is a method.
//!
//! # Two pixel sizes, and where the zoom is
//!
//! The client's art fixes a pixel size and the display has one of its own; they
//! are the same only at 1:1. `docs/camera.md` D11 calls the first **virtual**
//! and the second **real**, and the rule it settles is that motion is continuous
//! and the one rounding is to the real pixel — because a scroll that stepped a
//! whole *virtual* pixel would step `zoom` real ones, which is a world moving in
//! jumps coarser than the screen it is on.
//!
//! [`WorldPixel`] and [`ViewPixel`] are both virtual, and everything this crate
//! builds is measured in them: every quad, every atlas region and every
//! pixel-exact assertion is about the art's own grid and says the same thing at
//! every magnification. The zoom enters once, in [`Projection`], which the three
//! world passes apply in their last two lines of vertex shader — so a magnified
//! world is drawn at the display's resolution rather than drawn small and blown
//! up.
//!
//! Below 1:1 there is nothing to win that way: several virtual pixels land on
//! one real one, which is what a filter is for and not what a transform is, so
//! the world is drawn 1:1 into an image larger than the viewport and the blit's
//! linear sampler shrinks it. [`Camera::minifies`] is the branch, and it is the
//! only one.
//!
//! The third space is the **real** pixel, and it has a type for the fraction of
//! one: [`RealPoint`]. It was once true that nothing carried it — that the only
//! place a real pixel entered was the cursor, and it left in the same call, with
//! [`Camera::pick`] taking one and handing back a [`WorldPixel`]. It stopped
//! being true and the paragraph that said so outlived the fact: this camera
//! answers in real pixels from [`Camera::to_viewport`],
//! [`Camera::to_viewport_exact`], [`Camera::tile_facet`] and
//! [`Projection::centre`], and every one of them used to answer in the same bare
//! [`Vec2`](crate::geometry::Vec2) that [`Camera::to_view_exact`] and
//! [`Projection::origin`] answer in.
//! Which meant [`Camera::to_viewport_exact`] — a function that *takes* a view
//! pixel and *returns* a real one — compiled when fed its own output, silently
//! applying the zoom twice.
//!
//! The *view* side of that pair is [`ViewPoint`], and it was bare for one
//! release longer than the real one because it does not stop where the camera
//! does: it is what every quad and every sprite placement in this crate is
//! measured in, so typing it was a sweep through that path rather than a change
//! to this file. Both ends of [`Camera::to_viewport_exact`] now name their own
//! grid, which is the crossing that was silent.
//!
//! What is still bare, deliberately, is [`Rect`](crate::geometry::Rect): a
//! sprite's rectangle is a [`ViewPoint`] and an extent, but the same type is
//! also an atlas rectangle and a gump's place on the surface, and those are
//! three spaces sharing one shape. `docs/render/design_pixel_spaces.md` P3 carries that half.

use openshard_protocol::world::Point;

/// A land tile's sprite is this wide. Statics vary; the ground never does.
pub const TILE_WIDTH: i32 = 44;

/// And this tall. The diamond fills the square corner to corner.
pub const TILE_HEIGHT: i32 = 44;

/// Half a tile: the screen distance one step in `x` or `y` covers on each axis.
const HALF_WIDTH: i32 = TILE_WIDTH / 2;
const HALF_HEIGHT: i32 = TILE_HEIGHT / 2;

/// Pixels a single unit of `z` lifts a tile up the screen.
pub const Z_STEP: i32 = 4;

/// The tallest lift `z` can produce, in pixels.
///
/// `z` is an `i8`, so the whole range is 255 units, and a tile at the bottom of
/// a dungeon and one on a mountain differ by a thousand pixels of screen space.
/// This is the slack [`Camera::visible_tiles`] has to allow for, because a tile
/// whose *ground position* is below the viewport can still be drawn inside it.
const MAX_Z_LIFT: i32 = 128 * Z_STEP;

/// A position in the world's own pixel space.
///
/// The origin is tile `(0, 0, 0)` and it is unbounded in both directions: `x`
/// goes negative for anything east of the north corner. `y` grows downwards, as
/// it does in every window system and in the art.
///
/// It is the camera's job to turn this into somewhere in an image.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct WorldPixel {
    /// Rightwards.
    pub x: i32,
    /// Downwards.
    pub y: i32,
}

/// The same space as [`WorldPixel`], to a fraction of one.
///
/// What every position in this client actually is before anything rounds it: a
/// body mid-step, an eased sprite, an eye converging on one. [`WorldPixel`] is
/// what comes out of the quantiser, and `docs/camera.md` D11 is the rule for
/// where that quantiser sits — the fraction it keeps is a fraction of a
/// *virtual* pixel, because at `3x` a third of one is a whole pixel of the
/// display and rounding it away is the judder the whole decision is about.
///
/// `f64` and not `f32`, and it is the rounding that decides it rather than any
/// filter: the far corner of a 7,168-tile facet is 157,000 virtual pixels out,
/// where an `f32` resolves to about a hundredth of a pixel — fine for a
/// smoother, and a hundred times the margin at which two roundings of the same
/// position disagree. The eye has to land on the pixel the sprite landed on.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct WorldPoint {
    /// Rightwards.
    pub x: f64,
    /// Downwards.
    pub y: f64,
}

impl WorldPoint {
    /// Rounded to a whole virtual pixel.
    ///
    /// The *coarse* quantiser, and not what the screen is given: what a display
    /// can show is [`Camera::snap`], which is finer than this at every
    /// magnification above 1:1. This is for the things that reason about the
    /// world rather than about a frame — which tile the eye is over, which tiles
    /// are on screen — where the fraction it drops cannot change the answer.
    pub fn pixel(self) -> WorldPixel {
        WorldPixel {
            x: self.x.round() as i32,
            y: self.y.round() as i32,
        }
    }
}

/// A position in the image the world is drawn into, from its top-left corner.
///
/// Outside it is ordinary and not an error — most of what a camera projects
/// lands off the edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ViewPixel {
    /// Rightwards.
    pub x: i32,
    /// Downwards.
    pub y: i32,
}

/// The same space as [`ViewPixel`], to a fraction of one.
///
/// What [`Camera::to_view_exact`] answers in, what [`Projection::origin`] is
/// measured in, and what every sprite this crate places is positioned in — the
/// **art's own grid**, one unit per virtual pixel, which is what makes a quad
/// comparable to the art file texel for texel and is why the pixel-exact tests
/// assert about this space and not about the display's.
///
/// Fractional because two things reach it that are not on the lattice: a body
/// mid-step, which is [`WorldPoint`]'s whole subject, and the eye's own
/// remainder, which rides in [`Projection::origin`] rather than in any quad.
/// Everything the *map* holds lands on a whole one by construction — a tile
/// projects to a whole [`WorldPixel`] and [`Camera::to_view`] is an integer
/// translation of that — which is `docs/render/design_pixel_spaces.md` P2's first two rows and the
/// reason a whole value here is a box's own corner.
///
/// `f32` and not [`WorldPoint`]'s `f64`: the world's own space is 157,000 pixels
/// across at the map's far corner, and this one is a viewport, a few thousand at
/// most.
///
/// No `From` from [`RealPoint`] and none to it — [`Camera::to_viewport_exact`]
/// is the only crossing, because the zoom and the eye's fraction are what the
/// crossing consists of. [`ViewPoint::of`] widens a whole [`ViewPixel`] into
/// this, which is not a crossing at all: it is the same grid, said to a
/// fraction.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct ViewPoint {
    /// Rightwards.
    pub x: f32,
    /// Downwards.
    pub y: f32,
}

impl ViewPoint {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// A whole view pixel, said to a fraction. Exact, and the same space.
    pub fn of(pixel: ViewPixel) -> Self {
        Self::new(pixel.x as f32, pixel.y as f32)
    }
}

/// A place on the display's own grid, to a fraction of one of its pixels.
///
/// The **real** pixel of `docs/camera.md` D11 — what the compositor hands us and
/// what a painter drawing over the world has to answer in. Every other pixel
/// space in this crate is *virtual*, the art's own grid, and the two are the
/// same number only at 1:1; a [`Zoom`] is exactly the ratio between them.
///
/// Measured from the corner of the rect the world is drawn into, which is the
/// same corner [`ViewPixel`] measures from and is **not** the surface's corner
/// when a docked panel has moved the viewport — a caller painting onto the whole
/// surface adds the rect's own origin, once, itself.
///
/// Fractional because the things that reach this space are not on the display's
/// lattice either: a highlight round a slab a fifth of a tile thick has corners
/// between the virtual pixels before the zoom multiplies them, and rounding each
/// one on its own bends a plane. `f32` and not [`WorldPoint`]'s `f64`: this is a
/// position inside one viewport, a few thousand pixels wide at most, where `f32`
/// resolves to a ten-thousandth of a pixel.
///
/// No `From` or `Into` from any virtual space, for [`ViewPixel`]'s reason: the
/// only thing allowed to move a point between the two is a [`Camera`], because
/// the zoom and the eye's own fraction are what the move consists of.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct RealPoint {
    /// Rightwards.
    pub x: f32,
    /// Downwards.
    pub y: f32,
}

impl RealPoint {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// The same space as [`RealPoint`], to a whole pixel — what the compositor
/// hands us and what the mouse is reported in.
///
/// The one place this arrives is the cursor: `winit` reports a physical
/// position and [`Camera::pick`] takes it whole, because a display has no
/// sub-pixel cursor to lose. It is a pair of `i32`s for the same reason
/// [`WorldPixel`] and [`ViewPixel`] are — a signed offset from the viewport's
/// own corner is ordinary, not an error, on a cursor that has strayed past an
/// edge — and it is its own type for their reason too: [`Camera::pick`] used
/// to take `(x: i32, y: i32)` directly, which is exactly the shape a caller
/// holding a [`ViewPixel`] could pass by mistake and have it compile.
///
/// No `From` or `Into` from any other space — the same rule [`RealPoint`]
/// states, for the same reason: the zoom and the eye's own fraction are what
/// [`Camera::pick`] spends to leave this space, and nothing here is exact
/// without a [`Camera`] to ask.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RealPixel {
    /// Rightwards.
    pub x: i32,
    /// Downwards.
    pub y: i32,
}

impl RealPixel {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// A place in the world's own coordinates, between the tiles as well as on them.
///
/// The world the map states is a lattice — a tile is a whole `x`, a whole `y`
/// and a whole `z` — but the *geometry* standing in it is not: a wall of stated
/// thickness has faces a fifth of a tile apart, and a solid spanning several
/// tiles has corners on none of them. This is that place, in the same units the
/// map uses, so a number here is read the way a number in `docs/archive/render/lighting.md` is
/// read: `x` and `y` in tiles, `z` in the map's own height units.
///
/// **The lattice is the tiles' corners and not their centres**, which is the one
/// thing here that has to be got right on the way in. Tile `(x, y)` is the
/// square `x..x+1` by `y..y+1`, so its four corners are whole numbers and its
/// centre is a half — the other way round from [`Point`], where the whole number
/// *is* the centre and [`project`] measures from there. Choosing corners is what
/// makes a solid's extent read the same way the map states one: a body fills its
/// tile, `x..x+1`, rather than reaching half a tile out of it in each direction.
/// [`WorldSpot::centre`] is the bridge, and it is the only place the half lives.
///
/// `f64` for [`WorldPoint`]'s reason — the far corner of the map is 157,000
/// virtual pixels out, where an `f32` has resolved to about a hundredth of a
/// pixel and two roundings of one position can already disagree.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct WorldSpot {
    /// East, in tiles.
    pub x: f64,
    /// South, in tiles.
    pub y: f64,
    /// Up, in the map's height units.
    pub z: f64,
}

impl WorldSpot {
    /// The middle of the tile a [`Point`] names — the place [`project`]
    /// projects it to.
    ///
    /// The half-tile on each ground axis is the corner lattice meeting the
    /// centre lattice, and nothing but `z` crosses unchanged: heights are
    /// measured from the same zero in both.
    pub fn centre(point: Point) -> WorldSpot {
        WorldSpot {
            x: f64::from(point.x) + 0.5,
            y: f64::from(point.y) + 0.5,
            z: f64::from(point.z),
        }
    }
}

/// Where the centre of a tile's diamond falls in world pixel space.
///
/// [`project_exact`] at a whole lattice point, and it delegates to it rather
/// than repeating the arithmetic: the two are the same projection or geometry
/// placed between the tiles lands somewhere a sprite would not. The delegation
/// costs nothing in accuracy — every term is an integer under 2^24 in `f64`, so
/// the truncation on the way back is exact, and the test beside this pins that
/// against the whole map.
pub fn project(point: Point) -> WorldPixel {
    let at = project_exact(WorldSpot::centre(point));
    WorldPixel {
        x: at.x as i32,
        y: at.y as i32,
    }
}

/// The same projection for a place that is not a tile: [`project`]'s float core.
///
/// One arithmetic for the lattice and for everything standing on it. The
/// anisotropy is the part to keep in mind on the way in — a step of one in `x`
/// is 22 pixels across and 22 down, and a step of one in `z` is 4 up, so a solid
/// authored with equal numbers on the three axes is five and a half times too
/// tall. That scale is part of the projection and is carried, never corrected;
/// see `docs/archive/render/lighting.md` decision 39.1.
pub fn project_exact(at: WorldSpot) -> WorldPoint {
    WorldPoint {
        x: (at.x - at.y) * f64::from(HALF_WIDTH),
        // The half tile subtracted here is [`WorldSpot`]'s corner lattice, not a
        // fudge: at the corner `(x, y)` the sum `x + y` is one less than at the
        // centre of tile `(x, y)`, so without it every solid would be drawn half
        // a tile down the screen from the sprite it is meant to contain.
        y: (at.x + at.y - 1.0) * f64::from(HALF_HEIGHT) - at.z * f64::from(Z_STEP),
    }
}

/// Which tile a world pixel falls on, given the height to read it at.
///
/// The named inverse of [`project`], and exact: `project` is a linear map with
/// determinant `2 * HALF_WIDTH * HALF_HEIGHT`, so `x` and `y` come back out of
/// `x - y` and `x + y` with nothing lost. `z` has to be supplied because the
/// projection folds it into the vertical axis — a pixel on the screen is a whole
/// column of tiles at different heights, which is the entire difficulty of
/// picking and is why this takes the height rather than guessing one.
///
/// Tiles come back as `i32` and not `u16` for the same reason [`TileBounds`]
/// holds `i32`: world pixel space is unbounded, so a pixel north of the map's
/// corner has a negative tile, and clamping here would invent one. The caller
/// knows its map; this knows arithmetic.
pub fn unproject(at: WorldPixel, z: i8) -> (i32, i32) {
    // Undo the lift first, and the rest is `u = x - y`, `v = x + y` scaled by a
    // half tile: `a + b` is `44x` and `b - a` is `44y`.
    let a = at.x;
    let b = at.y + i32::from(z) * Z_STEP;
    // Rounded to the nearest tile rather than floored, so a pixel one short of a
    // centre names the tile it is nearly on. `div_euclid` because the numerator
    // is negative across half the map and truncation would round towards the
    // origin from one side and away from it on the other.
    (
        (a + b + HALF_WIDTH).div_euclid(TILE_WIDTH),
        (b - a + HALF_HEIGHT).div_euclid(TILE_HEIGHT),
    )
}

/// Which fractional tile a world pixel at `z = 0` names — [`unproject`]'s exact
/// counterpart, kept to a fraction rather than rounded to the tile it lands
/// nearest, and in the same units as a [`Point`]'s own `x`/`y`: a body
/// standing on tile `(100, 100)` reads back as `(100.0, 100.0)`, not the
/// `(100.5, 100.5)` [`project_exact`] would answer for its *centre* — the
/// `-0.5` [`WorldSpot::centre`] adds is undone here so a caller can subtract a
/// [`Point`] straight off the result.
///
/// [`crate::follow::Gaze`]'s `x` and `y` are this same plane before its own
/// `lift` channel folds the height back in, so this is how a walking
/// [`crate::mobiles::Mobile`] recovers how far past its own
/// [`crate::mobiles::Mobile::at`] tile its drawn position has actually
/// gotten — [`crate::mobiles::billboard_offset`] is the one caller.
pub fn unproject_ground(x: f64, y: f64) -> (f64, f64) {
    let sum = y / f64::from(HALF_HEIGHT) + 1.0;
    let diff = x / f64::from(HALF_WIDTH);
    ((sum + diff) * 0.5 - 0.5, (sum - diff) * 0.5 - 0.5)
}

/// How far the world is magnified, as an exact ratio.
///
/// A fraction from a fixed ladder and not an `f32`, for three reasons and the
/// third decides it: [`Camera`] is `Copy + Eq` and several tests compare
/// cameras, which a float field takes away; the offscreen target's size has to
/// come out the same integer every frame or the world is reallocated on rounding
/// noise; and a ladder is what a wheel notch wants anyway.
///
/// The numerator and denominator are private and the only way along the ladder
/// is [`Zoom::scale_up`] and [`Zoom::scale_down`], so a zoom off the end of it
/// is not expressible.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Zoom {
    /// An index into [`LADDER`]. The value itself is never arithmetic.
    step: u8,
}

/// Every zoom the wheel can reach, magnifying left to right.
///
/// Below 1 the world is minified and the offscreen target grows, which is what
/// [`Camera::render_width`] and the GPU's texture limit have to agree about.
///
/// **Whole above 1:1, fractional below it**, and the asymmetry is the point
/// rather than an oversight — `docs/camera.md` D11. Magnifying, the world is
/// drawn at the display's own resolution with `nearest` sampling, so a *whole*
/// magnification puts every texel on exactly that many real pixels and a whole
/// pixel of camera movement translates the picture; at `4/3` the texel widths
/// alternate 1, 2, 1, 2 and the pattern crawls as the camera moves, which is a
/// shimmer no placement of the quantiser fixes. It used to have `4/3` and `3/2`
/// and they are gone: a coarse ladder of exact rungs reads better than a fine
/// ladder of rungs that crawl.
///
/// Minifying, the same argument does not apply and the fractional rungs stay.
/// Several virtual pixels land on one real one there, which is a filter's job
/// and not a transform's, and the blit's linear sampler is that filter — so
/// `2/3` is no worse behaved than `1/2`, and the three of them are what makes
/// zooming out feel like a slider rather than a switch.
const LADDER: [(u32, u32); 7] = [(1, 2), (2, 3), (3, 4), (1, 1), (2, 1), (3, 1), (4, 1)];

/// Where `1:1` sits in [`LADDER`].
const ONE_STEP: u8 = 3;

impl Zoom {
    /// One world pixel to one viewport pixel.
    pub const ONE: Self = Self { step: ONE_STEP };

    /// The magnification's numerator: viewport pixels per `den` world pixels.
    pub const fn numerator(self) -> u32 {
        LADDER[self.step as usize].0
    }

    /// Its denominator.
    pub const fn denominator(self) -> u32 {
        LADDER[self.step as usize].1
    }

    /// One notch in, stopping at the top of the ladder.
    pub const fn scale_up(self) -> Self {
        let step = if self.step + 1 < LADDER.len() as u8 {
            self.step + 1
        } else {
            self.step
        };
        Self { step }
    }

    /// One notch out, stopping at the bottom.
    pub const fn scale_down(self) -> Self {
        let step = if self.step > 0 { self.step - 1 } else { self.step };
        Self { step }
    }

    /// Whether this is the widest view the ladder offers.
    pub const fn is_widest(self) -> bool {
        self.step == 0
    }

    /// How many world pixels a run of `viewport` pixels covers, rounded up.
    ///
    /// Up, because the offscreen image is blitted at exactly this ratio and a
    /// short image would leave a strip of the viewport undrawn. Rounding up
    /// instead spills a fraction of a pixel past the edges, where it is clipped.
    const fn world_pixels(self, viewport: u32) -> u32 {
        let (num, den) = LADDER[self.step as usize];
        // At least one pixel: a zero-sized viewport is a minimised window, not
        // an error, and a texture of zero width is.
        let pixels = viewport.div_ceil(num).saturating_mul(den);
        if pixels == 0 { 1 } else { pixels }
    }
}

impl std::fmt::Display for Zoom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (num, den) = LADDER[self.step as usize];
        if den == 1 {
            write!(f, "{num}x")
        } else {
            write!(f, "{num}/{den}x")
        }
    }
}

/// The tiles a viewport could show, as an inclusive rectangle in tile space.
///
/// Deliberately a rectangle and not a set: the visible region is a diamond, so
/// this over-covers by roughly half. Drawing a few hundred extra tiles costs
/// less than being clever about it, and *under*-covering is a hole in the world
/// that appears only at one camera angle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TileBounds {
    /// Lowest `x`, inclusive. May be negative: the caller clamps to its map.
    pub min_x: i32,
    /// Highest `x`, inclusive.
    pub max_x: i32,
    /// Lowest `y`, inclusive.
    pub min_y: i32,
    /// Highest `y`, inclusive.
    pub max_y: i32,
}

impl TileBounds {
    /// How many tiles across, counting both edges.
    ///
    /// Never negative: a rectangle whose `max` is below its `min` is empty, and
    /// a caller sizing a buffer from this would otherwise be sizing it from a
    /// negative number.
    pub fn width(self) -> i32 {
        (self.max_x - self.min_x + 1).max(0)
    }

    /// How many tiles down, counting both edges. See [`TileBounds::width`].
    pub fn height(self) -> i32 {
        (self.max_y - self.min_y + 1).max(0)
    }

    /// The same rectangle with everything outside a map of this size removed.
    ///
    /// `None` when nothing is left, which happens for a camera looking off the
    /// edge of a small facet — not an error, just an empty frame.
    ///
    /// Shared by the ground and the statics deliberately: they walk the same
    /// cells, and two copies of "clamp the bounds to the map" is two chances to
    /// draw a static on a tile whose ground was dropped.
    pub fn clamp_to(
        self,
        width: u32,
        height: u32,
    ) -> Option<(std::ops::RangeInclusive<u16>, std::ops::RangeInclusive<u16>)> {
        if width == 0 || height == 0 {
            return None;
        }
        let min_x = self.min_x.max(0) as u32;
        let min_y = self.min_y.max(0) as u32;
        let max_x = (self.max_x.max(0) as u32).min(width - 1);
        let max_y = (self.max_y.max(0) as u32).min(height - 1);
        if min_x > max_x || min_y > max_y {
            return None;
        }
        // Every bound fits a `u16` because it was clamped to the map's size, and
        // no facet is wider than 7,168 tiles.
        Some((min_x as u16..=max_x as u16, min_y as u16..=max_y as u16))
    }

    /// The parts of this rectangle that `covered` does not already contain.
    ///
    /// Up to four rectangles — a band above, a band below, and what is left of
    /// the rows between them on either side — and none at all when `covered`
    /// contains this one, which is the ordinary frame.
    ///
    /// This is what makes the atlases' growth proportional to the camera's
    /// *movement* rather than to the viewport. Asking "does the atlas hold
    /// every graphic on screen" walks nine thousand cells at 1080p on every
    /// frame to answer a question about the edge the camera just crossed; a step
    /// of one tile crosses one row of it. The invariant that makes the answer
    /// sound is positional and belongs to the caller: every cell inside
    /// `covered` has already been offered to the atlas, so a cell outside it is
    /// the only place a graphic can be new.
    ///
    /// Saturating, because tile space is unbounded in both directions here — see
    /// this type's own note — and `covered.min_x - 1` at `i32::MIN` is not a
    /// rectangle anybody asked for.
    pub fn difference(self, covered: TileBounds) -> [Option<TileBounds>; 4] {
        // Disjoint: nothing to subtract, and doing the arithmetic anyway would
        // produce the two bands *and* the two sides of an empty middle.
        if self.max_x < covered.min_x
            || self.min_x > covered.max_x
            || self.max_y < covered.min_y
            || self.min_y > covered.max_y
        {
            return [Some(self), None, None, None];
        }

        let rect = |min_x: i32, max_x: i32, min_y: i32, max_y: i32| {
            (min_x <= max_x && min_y <= max_y).then_some(TileBounds {
                min_x,
                max_x,
                min_y,
                max_y,
            })
        };
        // The rows the two rectangles share, which are the only ones with a left
        // and a right piece: above and below them the whole width is uncovered.
        let (from, to) = (self.min_y.max(covered.min_y), self.max_y.min(covered.max_y));
        [
            rect(
                self.min_x,
                self.max_x,
                self.min_y,
                self.max_y.min(covered.min_y.saturating_sub(1)),
            ),
            rect(
                self.min_x,
                self.max_x,
                self.min_y.max(covered.max_y.saturating_add(1)),
                self.max_y,
            ),
            rect(
                self.min_x,
                self.max_x.min(covered.min_x.saturating_sub(1)),
                from,
                to,
            ),
            rect(
                self.min_x.max(covered.max_x.saturating_add(1)),
                self.max_x,
                from,
                to,
            ),
        ]
    }
}

/// How the drawn image lands on the pixels a display actually has.
///
/// The one place the two pixel sizes of `docs/camera.md` D11 meet, and the
/// reason it is a value rather than three arguments: the three world passes all
/// need the same answer, and a pass that computed its own would draw a correct
/// frame at a different scale from its neighbours — which is not a wrong picture,
/// it is two pictures.
///
/// Every world pass reads it the same way, and this is the whole of the
/// arithmetic:
///
/// ```text
/// real = (virtual - origin) * scale + rect / 2
/// ```
///
/// `virtual` is a [`ViewPixel`] — the art's own grid, which is what every quad
/// this crate builds is measured in and what every pixel-exact test asserts
/// about. Only this last step knows what a zoom is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Projection {
    /// The point of the drawn image that lands in the middle of the target, in
    /// virtual pixels.
    ///
    /// Fractional, and that is the point: the eye is quantised to a *real*
    /// pixel, so at `3x` it carries thirds of a virtual one, and the remainder
    /// has nowhere else to go. Rounding it here would put the quantum back where
    /// D11 took it from.
    pub origin: ViewPoint,
    /// Real pixels per virtual pixel.
    pub scale:  f32,
}

/// Half an extent, floored, as a float — in whichever pixel space the extent
/// was in.
///
/// The one copy of the halving [`Projection::centre`], [`Projection::one_to_one`]
/// and [`Camera::projection`] all need, and the float image of the integer one
/// [`Camera::to_view`] and [`Camera::pick`] do. Spaceless on purpose: it is the
/// same rounding of the same number whether the extent is real pixels or virtual
/// ones, and the callers above differ in exactly which — so the space belongs on
/// what each of them hands back, not in here. `docs/render/design_frame_assembly.md`'s window-parity
/// entry is what the floor is for, and [`Projection::centre`] carries the
/// account.
fn half_extent(width: u32, height: u32) -> (f32, f32) {
    ((width / 2) as f32, (height / 2) as f32)
}

impl Projection {
    /// Where the middle of the target lands, in the target's own real pixels.
    ///
    /// **The three world vertex stages' `floor(viewport.size * 0.5)`, on this
    /// side of the wire, and the only copy of it here.** `ground.wesl` carries
    /// the account of why the floor is there; the short of it is that an image
    /// with an *odd* extent has a pixel in its middle rather than a join, and a
    /// world centred on that pixel puts every primary sample on a whole virtual
    /// pixel at some column — where a box's own corner is, and where
    /// [`crate::impostor::meets`] answers the tie with a face that has no area.
    /// `docs/render/design_frame_assembly.md`'s window-parity entry is the whole story.
    ///
    /// Integer division *is* that floor, and it is the same rounding
    /// [`Camera::to_view`] and [`Camera::pick`] have always done — which is the
    /// second reason this is one function: three roundings of one number have to
    /// be one rounding, and they were not, at an odd extent, for as long as
    /// [`Camera::to_viewport_exact`] halved a float.
    ///
    /// Gated by [`crate::camera::tests::no_primary_sample_lands_on_a_whole_virtual_pixel`]
    /// on this side and by `tests/parity.rs`'s odd-extent case on the shader's.
    pub fn centre(width: u32, height: u32) -> RealPoint {
        let half = half_extent(width, height);
        RealPoint::new(half.0, half.1)
    }

    /// The world drawn 1:1 into an image of this size, eye in the middle.
    ///
    /// What a frame test wants, and what the minifying path hands the passes:
    /// there is no magnification in either, and in the second one the scaling is
    /// the blit's.
    pub fn one_to_one(width: u32, height: u32) -> Self {
        // Halved as an integer for the reason `Camera::projection` gives at
        // length: `to_view` halves the extent the same way, and two roundings of
        // one number have to be one rounding.
        //
        // [`half_extent`] and not [`Projection::centre`], though the two are the
        // same arithmetic: `origin` is in *virtual* pixels and `centre` answers
        // in real ones, and here — and only here — the extent handed in is both,
        // because `scale` is 1. The shared rounding is what has to be one
        // function; the space is the caller's, so the two entry points differ by
        // which type they hand back and nothing else.
        let half = half_extent(width, height);
        Self {
            origin: ViewPoint::new(half.0, half.1),
            scale:  1.0,
        }
    }
}

/// What the view is looking at, how magnified, and how big the viewport is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Camera {
    /// Where the middle of the viewport looks.
    ///
    /// Pixels and not a tile: a tile is 44 pixels across and a drag is one pixel
    /// at a time.
    ///
    /// **On the real pixel's lattice**, which is `docs/camera.md` D11 and is not
    /// the same as whole virtual pixels — it is whole virtual pixels only at
    /// 1:1. At `3x` this holds thirds, because a third of a virtual pixel is a
    /// whole pixel of the display and an eye that could not express one would
    /// move the world in threes. [`Camera::snap`] is the quantiser and
    /// [`Camera::look_at`] is where it is applied, so nothing else has to
    /// remember; what the type still refuses is a *free* fraction, which would
    /// put every sprite on a half-texel boundary for half of all camera
    /// positions.
    ///
    /// Private, because "where the camera looks" is the one piece of state two
    /// writers fight over: the thing that pins it to the player, and the thing
    /// that pans it. [`Camera::look_at`] is the one door — and it takes a point
    /// rather than a tile because everything upstream of it has one: a body
    /// mid-step is between two tiles, and naming the tile throws away the part
    /// of the answer the whole glide exists to produce.
    eye:        WorldPoint,
    zoom:       Zoom,
    /// The viewport's width in *physical* pixels — the rect the UI leaves free,
    /// which is not the window.
    pub width:  u32,
    /// Its height, likewise.
    pub height: u32,
}

impl Camera {
    /// A camera on a tile, unmagnified, for a viewport of this size.
    pub fn new(center: Point, width: u32, height: u32) -> Self {
        let at = project(center);
        Self {
            eye: WorldPoint {
                x: f64::from(at.x),
                y: f64::from(at.y),
            },
            zoom: Zoom::ONE,
            width,
            height,
        }
    }

    /// Where the middle of the viewport looks, rounded to a virtual pixel.
    ///
    /// What everything that reasons about the *world* wants — which tiles are on
    /// screen, which tile the depth order is centred on, how far a drag moved.
    /// The fraction it drops is a fraction of a virtual pixel and never more, so
    /// no answer here changes by dropping it. [`Camera::eye_at`] is the one that
    /// keeps it, and only the projection needs that.
    pub fn eye(&self) -> WorldPixel {
        self.eye.pixel()
    }

    /// Where it looks, on the real pixel's lattice.
    pub fn eye_at(&self) -> WorldPoint {
        self.eye
    }

    /// How much of a virtual pixel one real pixel is.
    ///
    /// The quantum, and the whole of what D11 changed: `1` at 1:1, a third at
    /// `3x`, and two above `1/2` — where a real pixel is *coarser* than a
    /// virtual one and the lattice is the sparser of the two, which is honest
    /// rather than a rounding error. Nothing finer than this can be shown, so
    /// nothing finer than this is stored.
    pub fn quantum(&self) -> f64 {
        f64::from(self.zoom.denominator()) / f64::from(self.zoom.numerator())
    }

    /// The nearest position the display can actually distinguish.
    ///
    /// Public because the eye is not the only thing that has to sit on this
    /// lattice: a body drawn between two of its points would be resampled
    /// against a world that is not, which is a sprite whose texels change width
    /// as it walks. See [`crate::mobiles::place`].
    pub fn snap(&self, at: WorldPoint) -> WorldPoint {
        let quantum = self.quantum();
        WorldPoint {
            x: (at.x / quantum).round() * quantum,
            y: (at.y / quantum).round() * quantum,
        }
    }

    /// Look at a point, on the nearest real pixel to it.
    ///
    /// The one door into the eye, and the one place the quantiser runs: a caller
    /// that had to remember to snap is a caller that will not, and an eye off
    /// the lattice is a whole frame resampled by a fraction of a texel.
    pub fn look_at(&mut self, at: WorldPoint) {
        self.eye = self.snap(at);
    }

    /// Look at a whole virtual pixel, which is on the lattice at every zoom.
    pub fn look_at_pixel(&mut self, eye: WorldPixel) {
        self.eye = WorldPoint {
            x: f64::from(eye.x),
            y: f64::from(eye.y),
        };
    }

    /// The tile the eye is over, read at ground level.
    ///
    /// What [`crate::depth`] wants for its `base`: the ordering is centred on
    /// the camera so the visible frame sits where the depth buffer has
    /// resolution to spare, and `z` does not matter to that at all — it moves
    /// the answer by a tile or two out of a margin of five hundred.
    pub fn eye_tile(&self) -> (i32, i32) {
        unproject(self.eye(), 0)
    }

    /// The magnification.
    pub fn zoom(&self) -> Zoom {
        self.zoom
    }

    /// How much world the viewport shows across, in virtual pixels.
    ///
    /// Bigger than the viewport when minifying, smaller when magnifying. It is
    /// what the world is *measured* in — [`Camera::visible_tiles`] covers it and
    /// [`Camera::projection`] centres on half of it — and it is the image's size
    /// in real pixels only on the minifying path, which is the one place the two
    /// are the same number. [`Camera::image_size`] is the other question.
    pub fn render_width(&self) -> u32 {
        self.zoom.world_pixels(self.width)
    }

    /// And down.
    pub fn render_height(&self) -> u32 {
        self.zoom.world_pixels(self.height)
    }

    /// Whether the world image is coarser than the screen it ends up on.
    ///
    /// Only when minifying, and it decides the *whole* of what the zoom does.
    /// Magnified, the world is drawn at the display's own resolution and the
    /// magnification rides in [`Projection::scale`], so the image is already the
    /// size of the rect it goes into and the blit that carries it there is a
    /// copy. Minified, the image stays in the world's own pixels and the blit
    /// shrinks it, which is where a filter belongs: several virtual pixels
    /// landing on one real one is exactly the case `nearest` cannot answer.
    ///
    /// See `docs/camera.md` D11 for why the magnifying case cannot be left to
    /// the blit — the short of it is that an image of virtual resolution cannot
    /// express an offset of one real pixel, wherever the fraction is kept.
    pub fn minifies(&self) -> bool {
        self.zoom.numerator() < self.zoom.denominator()
    }

    /// The size of the image the world is drawn into, in real pixels.
    ///
    /// The viewport's own size when magnifying — the world is drawn at the
    /// display's resolution and the blit is a copy — and the world's own extent
    /// when minifying, which is larger than the viewport and is what the blit
    /// shrinks.
    pub fn image_size(&self) -> (u32, u32) {
        if self.minifies() {
            return (self.render_width(), self.render_height());
        }
        (self.width, self.height)
    }

    /// How that image's real pixels are reached from the world's virtual ones.
    pub fn projection(&self) -> Projection {
        // The middle of the drawn image, in its own virtual pixels. `to_view`
        // puts the eye exactly here, so subtracting it hands the passes an
        // offset from the eye — and the ceiling `render_width` applies cancels
        // out between the two rather than shifting the world half a pixel.
        //
        // Halved as an integer and *then* widened, because `to_view` halves it
        // as an integer too: at an odd extent the two disagree by half a virtual
        // pixel, which the scale turns into half of `zoom` real ones — a world
        // sitting a pixel and a half off centre at 3x, on some viewport widths
        // and not others. The two roundings have to be the same rounding, not
        // merely the same formula.
        //
        // And the eye's own fraction rides here, which is the whole of what
        // makes the world move a real pixel at a time. `to_view` measures from
        // the *rounded* eye, so what is left over is added once, to the point
        // the target is centred on, instead of to every quad.
        let rounded = self.eye();
        // `half_extent` and not a second `/ 2` written here: the same halving
        // `Projection::centre` does, so the two cannot drift apart.
        let half = half_extent(self.render_width(), self.render_height());
        let origin = ViewPoint::new(
            half.0 + (self.eye.x - f64::from(rounded.x)) as f32,
            half.1 + (self.eye.y - f64::from(rounded.y)) as f32,
        );
        if self.minifies() {
            // 1:1 into an image of virtual resolution, which the blit then
            // shrinks. The passes cannot tell this apart from no zoom at all,
            // and that is the point.
            return Projection { origin, scale: 1.0 };
        }
        Projection {
            origin,
            scale: self.zoom.numerator() as f32 / self.zoom.denominator() as f32,
        }
    }

    /// Where a world pixel lands in the drawn image.
    ///
    /// Measured from the eye *rounded*, and the fraction it therefore drops is
    /// put back by [`Camera::projection`] — once, on the origin, rather than on
    /// every quad. That is what keeps this integral: the quads a pass builds are
    /// on the art's grid and stay comparable to the files texel for texel, and
    /// the sub-pixel offset is a property of the frame rather than of any tile
    /// in it.
    pub fn to_view(&self, at: WorldPixel) -> ViewPixel {
        let eye = self.eye();
        ViewPixel {
            x: at.x - eye.x + self.render_width() as i32 / 2,
            y: at.y - eye.y + self.render_height() as i32 / 2,
        }
    }

    /// The same for something that is not on a whole virtual pixel.
    ///
    /// A body mid-step, and nothing else so far: everything the map holds is on
    /// a tile, and a tile projects to a whole pixel by construction.
    pub fn to_view_exact(&self, at: WorldPoint) -> ViewPoint {
        let eye = self.eye();
        ViewPoint::new(
            (at.x - f64::from(eye.x)) as f32 + (self.render_width() as i32 / 2) as f32,
            (at.y - f64::from(eye.y)) as f32 + (self.render_height() as i32 / 2) as f32,
        )
    }

    /// And back. The exact inverse of [`Camera::to_view`] — integers throughout,
    /// because the zoom is not in either of them.
    pub fn to_world(&self, at: ViewPixel) -> WorldPixel {
        let eye = self.eye();
        WorldPixel {
            x: at.x - self.render_width() as i32 / 2 + eye.x,
            y: at.y - self.render_height() as i32 / 2 + eye.y,
        }
    }

    /// Where a tile's centre falls in the drawn image, in pixels from its
    /// top-left corner. Outside it is ordinary and not an error.
    pub fn to_screen(&self, point: Point) -> ViewPixel {
        self.to_view(project(point))
    }

    /// Where a drawn-image pixel lands in the viewport, after the blit's scale.
    ///
    /// [`Camera::to_view`] stops at the offscreen image, which is drawn 1:1
    /// and then blitted into the viewport at exactly the ratio
    /// [`Zoom::world_pixels`] used to size it — so the inverse of that ratio is
    /// what carries a render-space point the rest of the way to a pixel a
    /// painter can use. `f32` because a highlight is drawn at whatever
    /// magnification the blit lands on, not on a texel grid.
    pub fn to_viewport(&self, at: ViewPixel) -> RealPoint {
        self.to_viewport_exact(ViewPoint::of(at))
    }

    /// The same for a render-space point that is not on a whole virtual pixel.
    ///
    /// [`Camera::to_viewport`]'s core, and the reason it is split out: geometry
    /// that is not on the tile lattice — a slab a fifth of a tile thick — has
    /// corners between the virtual pixels, and rounding each one to reach the
    /// viewport would put a face's two ends on different fractions of the same
    /// plane. The integer entry point is this one at whole coordinates, so there
    /// is no second projection to disagree with.
    ///
    /// `at` is a [`ViewPoint`] and the answer is a [`RealPoint`], and the two
    /// being different types is the point: this function **is** the zoom, so
    /// feeding it its own output applies the zoom twice — which compiled, for as
    /// long as both sides were a bare [`Vec2`](crate::geometry::Vec2), and would
    /// put a highlight `zoom`
    /// times further from the middle of the viewport than the world it is drawn
    /// over.
    pub fn to_viewport_exact(&self, at: ViewPoint) -> RealPoint {
        // From `projection`'s origin and not from half the extent, so the eye's
        // sub-virtual-pixel offset is in here too. Without it this lands where
        // the world *would* be if the camera were on a whole virtual pixel,
        // which at `4x` is up to four real pixels from where the world actually
        // is — a tile highlight that slides off its tile as the camera moves,
        // and a gate for the drag that reads correct while the picture is not.
        let projection = self.projection();
        // The zoom's own ratio and not `projection.scale`: minifying, the passes
        // draw at 1:1 and it is the blit that shrinks, so the scale that reaches
        // the viewport is the same number on both paths even though the one in
        // the transform is not.
        let scale = self.zoom.numerator() as f32 / self.zoom.denominator() as f32;
        // [`Projection::centre`] and not `width / 2.0`, for the reason that
        // function is: the passes centre the world on `floor(size / 2)`, and a
        // painter centring it on `size / 2` puts its highlight half a *real*
        // pixel off the world it is drawn over at every odd extent — the finest
        // offset a display can show, on the axis nothing here ever varied. It is
        // also what [`Camera::pick`] has always done, so the two directions are
        // now one rounding rather than two.
        let centre = Projection::centre(self.width, self.height);
        RealPoint::new(
            (at.x - projection.origin.x) * scale + centre.x,
            (at.y - projection.origin.y) * scale + centre.y,
        )
    }

    /// The four corners of a tile's diamond, in viewport pixels — top, right,
    /// bottom, left.
    ///
    /// Read off the same square every ground quad is drawn from: the art is
    /// 44 pixels on a side in render space regardless of zoom, and only the
    /// blit in [`Camera::to_viewport`] scales it, so the offsets below are
    /// taken before that conversion and not after.
    pub fn tile_diamond(&self, point: Point) -> [RealPoint; 4] {
        self.tile_facet(point, [point.z; 4])
    }

    /// The same four corners with each one at its *own* height — the sloped
    /// quad the ground pass actually draws.
    ///
    /// `corners` are absolute heights in the diamond's own order: top, right,
    /// bottom, left, which is `(x, y)`, `(x+1, y)`, `(x+1, y+1)`, `(x, y+1)`.
    /// Note that this is **not** [`WorldMap::land_corners`] order — that one reads
    /// top, right, *left*, bottom — so a caller passing land heights straight
    /// through gets a bow tie. The reorder is the caller's because only the
    /// caller knows whether the surface it is describing is land at all: a
    /// pier's planks are flat whatever the water under them does.
    ///
    /// A flat facet is [`Camera::tile_diamond`], which is this with one height
    /// four times — one arithmetic, so a marker on level ground and a marker on
    /// a hillside cannot land on different pixels for any reason but the slope.
    ///
    /// The lift is a *difference* from `point.z` because [`Camera::to_screen`]
    /// has already applied that one to the centre.
    pub fn tile_facet(&self, point: Point, corners: [i8; 4]) -> [RealPoint; 4] {
        let centre = self.to_screen(point);
        let half = TILE_WIDTH / 2;
        // Up the screen as the corner rises, by the same `Z_STEP` `project`
        // uses — the corner has to land where the ground vertex under it does.
        let lift = |z: i8| (i32::from(z) - i32::from(point.z)) * Z_STEP;
        [
            (0, -half, corners[0]),
            (half, 0, corners[1]),
            (0, half, corners[2]),
            (-half, 0, corners[3]),
        ]
        .map(|(dx, dy, z)| {
            self.to_viewport(ViewPixel {
                x: centre.x + dx,
                y: centre.y + dy - lift(z),
            })
        })
    }

    /// What world pixel the cursor is over, given where it is in the viewport.
    ///
    /// The one place a *viewport* pixel is spoken about, and it does not escape:
    /// the zoom is undone here and a world pixel comes out. Lossy in the
    /// magnifying direction by construction — several viewport pixels share one
    /// world pixel at zoom 4 — which is the honest answer, since the world has
    /// no finer position to name.
    pub fn pick(&self, at: RealPixel) -> WorldPixel {
        let den = self.zoom.denominator() as i32;
        let num = self.zoom.numerator() as i32;
        // About the centre, because that is where the blit is anchored: the
        // offscreen image is drawn over the viewport rect whole, so the two
        // centres coincide whatever the rounding did to the edges.
        let dx = (at.x - self.width as i32 / 2) * den / num;
        let dy = (at.y - self.height as i32 / 2) * den / num;
        let eye = self.eye();
        WorldPixel {
            x: eye.x + dx,
            y: eye.y + dy,
        }
    }

    /// Change the magnification, keeping whatever is under the cursor there.
    ///
    /// The whole reason the inverse exists. Hold `pick(cursor)` fixed across the
    /// change and solve for the new eye — one line, and it is the difference
    /// between a camera that feels placed and one that feels shoved.
    pub fn zoom_about(&mut self, at: RealPixel, zoom: Zoom) {
        let before = self.pick(at);
        self.zoom = zoom;
        let after = self.pick(at);
        // Snapped to the *new* zoom's lattice, and by `look_at` rather than by
        // hand: a third of a pixel is on the lattice at 3x and is not at 2x, so
        // an eye carried across a rung unchanged would sit between two real
        // pixels for as long as nobody moved it.
        self.look_at(WorldPoint {
            x: self.eye.x + f64::from(before.x - after.x),
            y: self.eye.y + f64::from(before.y - after.y),
        });
    }

    /// Every tile that could land inside the drawn image, over-covered.
    ///
    /// The inverse of [`project`] is exact — see [`unproject`] — but only for a
    /// known `z`, and `z` is what is stored per tile and therefore unknown until
    /// the tile is read. So the vertical span is widened by the whole range `z`
    /// can lift a tile through, and by one tile for the sprite's own size. The
    /// result is a superset, which is the safe direction.
    ///
    /// Zoomed out this covers more, because the image it is covering *is*
    /// bigger: nothing here reads the zoom, only the size it produced.
    pub fn visible_tiles(&self) -> TileBounds {
        let half_w = self.render_width() as i32 / 2;
        let half_h = self.render_height() as i32 / 2;

        // The image's rectangle in world pixel space, grown by a tile so a
        // diamond straddling the edge still counts, and by the `z` range in
        // *both* directions. Both, because `z` is signed and the two cases look
        // nothing alike: a mountain lifts a tile whose ground position is below
        // the viewport up into it, and a dungeon floor drops a tile from above
        // the viewport down into it. Widening only downwards passes every test
        // written at `z = 0` and loses a band of ground the moment the ground
        // goes negative.
        let eye = self.eye();
        let left = eye.x - half_w - TILE_WIDTH;
        let right = eye.x + half_w + TILE_WIDTH;
        let top = eye.y - half_h - TILE_HEIGHT - MAX_Z_LIFT;
        let bottom = eye.y + half_h + TILE_HEIGHT + MAX_Z_LIFT;

        // `u = x - y` and `v = x + y`, in tiles. Dividing rounds towards zero,
        // which shrinks the range on the negative side, so each bound is pushed
        // out by one rather than reasoned about.
        let u_min = left / HALF_WIDTH - 1;
        let u_max = right / HALF_WIDTH + 1;
        let v_min = top / HALF_HEIGHT - 1;
        let v_max = bottom / HALF_HEIGHT + 1;

        // `x = (u + v) / 2`, `y = (v - u) / 2`, each extreme taken from the
        // corner of the `(u, v)` rectangle that maximises it.
        TileBounds {
            min_x: (u_min + v_min).div_euclid(2),
            max_x: (u_max + v_max).div_euclid(2) + 1,
            min_y: (v_min - u_max).div_euclid(2),
            max_y: (v_max - u_min).div_euclid(2) + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The arithmetic `ground.wgsl` and `statics.wgsl` both end on, in Rust.
    ///
    /// A copy of two lines of shader, and worth it: everything below is an
    /// assertion about what lands where on a display, and the alternative is a
    /// GPU, an atlas and the client's files to say anything at all about it. It
    /// is one expression, it is written out in `Projection`'s own doc comment,
    /// and the frame tests are what keep the two honest.
    fn real(projection: Projection, target: (u32, u32), at: ViewPixel) -> RealPoint {
        let centre = Projection::centre(target.0, target.1);
        RealPoint::new(
            (at.x as f32 - projection.origin.x) * projection.scale + centre.x,
            (at.y as f32 - projection.origin.y) * projection.scale + centre.y,
        )
    }

    /// **The rungs where the window-parity repair does not reach, named.**
    ///
    /// The floor in the three vertex stages centres the world on a pixel *join*
    /// at every extent, which makes a sample sit at a half-integer over `scale`
    /// — and no integer `scale` divides a half-integer. That proof has a premise
    /// nobody wrote down: it is about the extent, and the eye contributes a
    /// fraction of its own to the same sum. The eye is snapped to
    /// [`Camera::quantum`], `denominator / numerator`, and at `2/3x` that is
    /// `1.5` — so half of all camera positions there put the eye exactly half a
    /// virtual pixel off, the sum comes out whole, and the ray goes through the
    /// box's corner again.
    ///
    /// Only this one rung. Magnifying, the quantum is `1 / scale` and every
    /// fraction is `m / scale`, which is the case the proof covers; at `1/2x`
    /// the quantum is `2` and the fraction is always zero; at `3/4x` it is
    /// `4/3`, whose fractions are thirds and never a half.
    ///
    /// Recorded rather than repaired, because repairing it is a decision about
    /// *motion* and not about centring — dropping the eye's fraction at the
    /// minifying rungs costs a third of a real pixel of smoothness there, which
    /// is `docs/camera.md` D11's own subject. `docs/render/design_frame_assembly.md`'s backlog carries
    /// it; this constant is what keeps the gate above from being green about it
    /// by accident.
    const AN_EYE_ON_A_HALF_PIXEL_REACHES_THE_CORNER: [&str; 1] = ["2/3x"];

    /// **No primary sample can land on a whole virtual pixel** — `docs/render/design_frame_assembly.md`
    /// P5's G1, and the arithmetic half of the window-parity repair.
    ///
    /// The defect it states the absence of: a fragment samples at `i + 0.5` real
    /// pixels, and the world coordinate behind that sample is
    /// `(i + 0.5 - centre) / scale + origin`. A box's own corner sits at a whole
    /// virtual pixel by construction, so a sample that lands on one is a view ray
    /// passing exactly through a box's vertical edge — where `impostor::meets`'s
    /// tie answers `+Y` and draws a one-pixel green line down every `+X` wall.
    /// That is what the client showed and no tool ever did, because every tool's
    /// viewport has been even.
    ///
    /// With [`Projection::centre`] flooring, the whole of `centre` and the whole
    /// part of `origin` are integers and cancel, leaving `(i + m + 0.5) / scale`
    /// for an integer `m` — a half-integer over an integer scale, which is never
    /// a whole number. So the assertion is not "no case found": with the eye on a
    /// whole virtual pixel the closest any sample comes to one is **exactly**
    /// `0.5 / scale`, which is the property rather than the symptom, and the
    /// minimum is reported rather than merely bounded.
    ///
    /// Every rung of the ladder, both parities of both axes, and every eye
    /// fraction the quantum can express — the inputs whose *unanimity* hid this,
    /// varied here on purpose. And varying the last of them is what found
    /// [`AN_EYE_ON_A_HALF_PIXEL_REACHES_THE_CORNER`] below: the repair is a
    /// statement about the *extent*, and the eye has a fraction of its own.
    ///
    /// **Witness it by mutation:** make [`Projection::centre`] halve as a float
    /// and the odd extents turn red at once. It gates this side's copy of the
    /// shader's last line; `tests/parity.rs`'s odd-extent case gates the shader.
    #[test]
    fn no_primary_sample_lands_on_a_whole_virtual_pixel() {
        let mut zoom = Zoom::ONE;
        while !zoom.is_widest() {
            zoom = zoom.scale_down();
        }
        let mut rungs = 0;
        let mut cases = 0;
        // How often the recorded exception was actually met. A list of rungs
        // nothing reached would be a list nobody could tell from a repair.
        let mut reached_the_corner = 0;
        loop {
            for (width, height) in [(900, 700), (901, 700), (900, 701), (901, 701)] {
                let mut camera = Camera::new(Point::new(1501, 1659, 0), width, height);
                camera.zoom = zoom;
                // Every eye the display can distinguish, which is the fraction
                // that reaches `Projection::origin`: whole virtual pixels at 1:1,
                // thirds at 3x, and only even ones at 1/2x.
                let quantum = camera.quantum();
                let base = camera.eye_at();
                for step in 0..i64::from(zoom.numerator()) {
                    camera.look_at(WorldPoint {
                        x: base.x + step as f64 * quantum,
                        y: base.y + step as f64 * quantum,
                    });
                    let projection = camera.projection();
                    let (image_width, image_height) = camera.image_size();
                    let centre = Projection::centre(image_width, image_height);
                    let scale = f64::from(projection.scale);

                    for (extent, middle, origin) in [
                        (image_width, centre.x, projection.origin.x),
                        (image_height, centre.y, projection.origin.y),
                    ] {
                        // A sample every real pixel across the image, and the
                        // *smallest* distance any of them reaches — a detector
                        // that reported only "none was zero" would be reporting
                        // about its own tolerance.
                        let mut nearest = f64::INFINITY;
                        for pixel in 0..extent {
                            let virtual_pixel =
                                (f64::from(pixel) + 0.5 - f64::from(middle)) / scale + f64::from(origin);
                            nearest = nearest.min((virtual_pixel - virtual_pixel.round()).abs());
                            cases += 1;
                        }
                        // The eye's own fraction, which rides in `origin` beside
                        // the centre and is the other half of where a sample
                        // lands. Whole at 1:1, a third at 3x — and a *half* at
                        // `2/3x`, which is the one rung the centring cannot
                        // answer for. See the constant below.
                        let fraction = f64::from(origin) - f64::from(origin).round();
                        if nearest < 1e-6 {
                            // **The recorded exception, and nothing else may
                            // take shelter in it.** A rung not on the list turns
                            // this red, which is what makes the list a statement
                            // rather than a tolerance.
                            assert!(
                                AN_EYE_ON_A_HALF_PIXEL_REACHES_THE_CORNER
                                    .contains(&zoom.to_string().as_str()),
                                "at {zoom}, {width}x{height}, eye step {step}, eye fraction \
                                 {fraction}: a sample lands on a whole virtual pixel, which is a \
                                 view ray through a box's own vertical corner — docs/render/design_frame_assembly.md's \
                                 window-parity entry, on a rung nothing has recorded it for",
                            );
                            assert!(
                                (fraction.abs() - 0.5).abs() < 1e-6,
                                "at {zoom}: the recorded exception is an eye on a half pixel, and \
                                 this sample landed whole with the eye at {fraction}",
                            );
                            reached_the_corner += 1;
                            continue;
                        }
                        // **The property**, where it holds: with the eye's own
                        // fraction a multiple of `1 / scale` — every magnifying
                        // rung, and a whole-pixel eye on the others — the
                        // centring alone decides how close a sample comes, and
                        // that is exactly half a real pixel's worth of virtual
                        // one. `f32`'s noise is the only slack: the eye's
                        // fraction is a third at 3x, exact in neither width.
                        if scale > 1.0 || fraction.abs() < 1e-6 {
                            let want = 0.5 / scale;
                            assert!(
                                (nearest - want).abs() < 1e-5,
                                "at {zoom}, {width}x{height}, eye fraction {fraction}: the nearest \
                                 sample came {nearest} of a virtual pixel from a whole one, against \
                                 {want}",
                            );
                        }
                    }
                }
            }
            rungs += 1;
            let next = zoom.scale_up();
            if next == zoom {
                break;
            }
            zoom = next;
        }
        assert_eq!(rungs, LADDER.len(), "every rung of the ladder was walked");
        // What was counted, said out loud: a sweep that silently covered one
        // column would satisfy every assertion above.
        assert!(cases > 100_000, "only {cases} samples were looked at");
        // And the exception is a case that happens, not a case that is allowed:
        // if the eye at `2/3x` stops reaching a box's corner, this is what says
        // so instead of the list quietly covering nothing.
        assert!(
            reached_the_corner > 0,
            "no eye reached a box's own corner on any rung, so \
             AN_EYE_ON_A_HALF_PIXEL_REACHES_THE_CORNER now names a defect that is gone — take it \
             out, here and in docs/render/design_frame_assembly.md's backlog",
        );
    }

    /// The three halvings of one extent are one halving.
    ///
    /// [`Projection::centre`] answers in real pixels, [`Projection::one_to_one`]
    /// and [`Camera::projection`] put the same number in `origin` in virtual
    /// ones, and [`Camera::to_view`] does it a third time in integers. They now
    /// share [`half_extent`], and this is what says so from outside: a floor
    /// written twice is a floor that drifts at exactly the odd extents
    /// `docs/render/design_frame_assembly.md`'s window-parity entry is about, and every extent below is
    /// odd on at least one axis for that reason.
    ///
    /// The 1:1 row is the crossing the types otherwise refuse — the one camera at
    /// which a real pixel and a virtual one are the same pixel, which is why
    /// `one_to_one` may build a view-space `origin` out of the same arithmetic
    /// `centre` reads as real.
    #[test]
    fn one_extent_is_halved_once() {
        for (width, height) in [(900, 700), (901, 700), (900, 701), (901, 701), (1, 1)] {
            let centre = Projection::centre(width, height);
            let origin = Projection::one_to_one(width, height).origin;
            assert_eq!(
                (centre.x, centre.y),
                (origin.x, origin.y),
                "at {width}x{height}: the real centre and the 1:1 virtual origin are one number",
            );

            // And the camera's own two, at 1:1, where `to_view` puts the eye
            // exactly on `projection().origin` — the property the comment in
            // `Camera::projection` argues and nothing asserted.
            let camera = Camera::new(Point::new(1501, 1659, 0), width, height);
            let eye = camera.to_view(camera.eye());
            let origin = camera.projection().origin;
            assert_eq!(
                (eye.x as f32, eye.y as f32),
                (origin.x, origin.y),
                "at {width}x{height}: `to_view`'s integer halving and `projection`'s float one",
            );
        }
    }

    /// [`project`] now goes through [`project_exact`], and this is the gate on
    /// that: the integer arithmetic it used to do, written out once more here,
    /// against the float path over the map's whole extent and the whole of `z`.
    ///
    /// Not a tautology and not a formality — the delegation is only free because
    /// every term stays an exact integer in `f64` (the largest is 7,168 × 22,
    /// well inside 2^53) and because [`WorldSpot`]'s corner lattice is half a
    /// tile off the centre one. Either claim failing shows up here, and the
    /// second one shows up as exactly 22 pixels.
    #[test]
    fn the_float_projection_is_the_integer_one_at_a_whole_tile() {
        // The corners of the map, the middle, and a tile whose `x + y` is odd on
        // purpose: the half tile lives in that sum.
        for (x, y) in [
            (0, 0),
            (7167, 7167),
            (0, 7167),
            (7167, 0),
            (1500, 1601),
            (4096, 4096),
        ] {
            for z in [i8::MIN, -50, 0, 1, 27, i8::MAX] {
                let point = Point::new(x, y, z);
                let want = WorldPixel {
                    x: (i32::from(x) - i32::from(y)) * HALF_WIDTH,
                    y: (i32::from(x) + i32::from(y)) * HALF_HEIGHT - i32::from(z) * Z_STEP,
                };
                assert_eq!(project(point), want, "at {point:?}");
            }
        }
    }

    /// And the other direction of the same seam: the tile a [`Point`] names is
    /// the *centre* of the square its four corners are whole numbers at, so the
    /// corner and the centre differ by half a tile on each ground axis and by
    /// nothing at all in `z`.
    #[test]
    fn a_tile_centre_sits_half_a_tile_from_its_own_corner() {
        let point = Point::new(1500, 1600, 12);
        let centre = project_exact(WorldSpot::centre(point));
        let corner = project_exact(WorldSpot {
            x: f64::from(point.x),
            y: f64::from(point.y),
            z: f64::from(point.z),
        });
        // Straight up the screen by half a tile's height: `(x - y)` is unchanged
        // by adding a half to both, and `(x + y)` gains one.
        assert_eq!(centre.x, corner.x);
        assert_eq!(centre.y - corner.y, f64::from(HALF_HEIGHT));
    }

    /// Unmagnified, the whole of D11 is a no-op: a virtual pixel is a real one,
    /// and the pixel a quad lands on is the one `to_view` named.
    #[test]
    fn at_one_to_one_the_projection_is_the_identity() {
        let camera = Camera::new(Point::new(300, 300, 0), 800, 600);
        assert_eq!(
            camera.image_size(),
            (camera.render_width(), camera.render_height())
        );
        let projection = camera.projection();
        assert_eq!(projection.scale, 1.0);
        for point in [Point::new(300, 300, 0), Point::new(305, 297, 12)] {
            let view = camera.to_screen(point);
            let at = real(projection, camera.image_size(), view);
            assert_eq!((at.x, at.y), (view.x as f32, view.y as f32));
        }
    }

    /// Magnified, the image is the viewport's own size — the world is drawn at
    /// the display's resolution rather than at a fraction of it and blown up —
    /// and one virtual pixel of separation is exactly `zoom` real ones.
    ///
    /// The second half is the gate D11 names: a texel that is not `zoom` real
    /// pixels wide is a texel the magnification resampled, which is the artefact
    /// the whole arrangement exists to avoid.
    #[test]
    fn magnified_a_virtual_pixel_is_exactly_zoom_real_ones() {
        for rungs in 1..=5 {
            let mut camera = Camera::new(Point::new(300, 300, 0), 800, 600);
            let mut zoom = Zoom::ONE;
            for _ in 0..rungs {
                zoom = zoom.scale_up();
            }
            camera.zoom_about(RealPixel::new(400, 300), zoom);
            assert!(!camera.minifies());
            assert_eq!(camera.image_size(), (800, 600), "the viewport's own resolution");

            let projection = camera.projection();
            let eye = camera.to_view(camera.eye());
            let from = real(projection, camera.image_size(), eye);
            let to = real(
                projection,
                camera.image_size(),
                ViewPixel {
                    x: eye.x + 1,
                    y: eye.y + 1,
                },
            );
            let expected = zoom.numerator() as f32 / zoom.denominator() as f32;
            // Exact at a whole magnification, and within a float's noise at a
            // fractional one — where the promise is weaker anyway, because a
            // texel of `4/3` real pixels cannot be a whole number of them
            // however the arithmetic is done. That is the shimmer D11 gives as
            // its reason for the ladder ending up integral, and it is measured
            // here rather than asserted away.
            let (dx, dy) = (to.x - from.x, to.y - from.y);
            if zoom.denominator() == 1 {
                assert_eq!((dx, dy), (expected, expected), "at {zoom}");
            } else {
                assert!((dx - expected).abs() < 1e-4, "at {zoom}: {dx} against {expected}");
                assert!((dy - expected).abs() < 1e-4, "at {zoom}: {dy} against {expected}");
            }
        }
    }

    /// Minified, nothing moves into the transform: the passes draw 1:1 into an
    /// image larger than the viewport and the blit's linear sampler shrinks it,
    /// which is the one direction a filter is the right answer.
    #[test]
    fn minified_the_image_is_the_worlds_extent_and_the_scale_is_one() {
        let mut camera = Camera::new(Point::new(300, 300, 0), 800, 600);
        camera.zoom_about(RealPixel::new(400, 300), Zoom::ONE.scale_down());
        assert!(camera.minifies());
        assert_eq!(camera.projection().scale, 1.0);
        assert_eq!(
            camera.image_size(),
            (camera.render_width(), camera.render_height())
        );
        assert!(camera.render_width() > 800, "more world across than viewport");
    }

    /// The eye lands in the middle of the image, at every rung of the ladder.
    ///
    /// One assertion and it covers both paths: it is what makes the two centres
    /// coincide, which is the premise `Camera::pick` states out loud and the
    /// reason a zoom about the middle does not move the world.
    #[test]
    fn the_eye_is_in_the_middle_whatever_the_zoom() {
        let mut camera = Camera::new(Point::new(300, 300, 0), 800, 600);
        let mut zoom = Zoom::ONE;
        loop {
            let down = zoom.scale_down();
            if down == zoom {
                break;
            }
            zoom = down;
        }
        loop {
            camera.zoom_about(RealPixel::new(400, 300), zoom);
            let (width, height) = camera.image_size();
            let middle = real(camera.projection(), (width, height), camera.to_view(camera.eye()));
            assert_eq!(
                (middle.x, middle.y),
                (width as f32 / 2.0, height as f32 / 2.0),
                "the eye is off centre at {zoom}",
            );
            let up = zoom.scale_up();
            if up == zoom {
                break;
            }
            zoom = up;
        }
    }

    /// The four numbers the whole projection is made of. If these move, the art
    /// no longer tiles, so they are written out rather than derived.
    #[test]
    fn a_step_moves_half_a_tile_on_each_axis() {
        assert_eq!(project(Point::new(0, 0, 0)), WorldPixel { x: 0, y: 0 });
        // East: right and down.
        assert_eq!(project(Point::new(1, 0, 0)), WorldPixel { x: 22, y: 22 });
        // South: left and down.
        assert_eq!(project(Point::new(0, 1, 0)), WorldPixel { x: -22, y: 22 });
        // Both: straight down one full tile, and back to the same column.
        assert_eq!(project(Point::new(1, 1, 0)), WorldPixel { x: 0, y: 44 });
    }

    #[test]
    fn height_lifts_four_pixels_per_unit() {
        assert_eq!(project(Point::new(0, 0, 10)).y, -40);
        assert_eq!(project(Point::new(0, 0, -10)).y, 40);
        // And never sideways: a cliff would shear otherwise.
        assert_eq!(
            project(Point::new(5, 3, 100)).x,
            project(Point::new(5, 3, -100)).x
        );
    }

    /// The inverse is an inverse — over the whole `z` range, because that is the
    /// axis it has to be told about and therefore the one that can be wired up
    /// wrongly and still pass at `z = 0`.
    #[test]
    fn unproject_undoes_project() {
        for x in [0u16, 1, 2, 511, 1495, 6143] {
            for y in [0u16, 1, 3, 512, 1629, 4095] {
                for z in [i8::MIN, -37, -1, 0, 1, 44, i8::MAX] {
                    let point = Point::new(x, y, z);
                    assert_eq!(
                        unproject(project(point), z),
                        (i32::from(x), i32::from(y)),
                        "{point} did not come back",
                    );
                }
            }
        }
    }

    /// [`unproject_ground`] is [`project`]'s exact inverse at `z = 0`, fraction
    /// and all — a standing body's own tile comes back whole, which is the
    /// property [`crate::mobiles::billboard_offset`] leans on to recover how
    /// far a walking body's drawn position sits past it.
    #[test]
    fn unproject_ground_undoes_project_at_a_fraction() {
        for (x, y) in [
            (0.0, 0.0),
            (0.5, 0.5),
            (1495.25, 6143.75),
            (0.1, 0.9),
            (-3.0, -3.0),
        ] {
            let spot = WorldSpot { x, y, z: 0.0 };
            let projected = project_exact(spot);
            let (tx, ty) = unproject_ground(projected.x, projected.y);
            // `project_exact` reads a *spot*, half a tile off `Point`'s own
            // lattice — `WorldSpot::centre`'s `+0.5` — and `unproject_ground`
            // undoes exactly that, so the round trip lands half a tile short
            // of the spot it started from.
            assert!((tx - (x - 0.5)).abs() < 1e-9, "x: {tx} vs {}", x - 0.5);
            assert!((ty - (y - 0.5)).abs() < 1e-9, "y: {ty} vs {}", y - 0.5);
        }

        // The natural form: a standing tile comes back whole. `z = 0` only —
        // `unproject_ground` reads the same plane [`crate::follow::Gaze`]
        // keeps `z` out of, so a nonzero one belongs in the caller's own
        // `lift` channel, not folded into `y` the way `project` would.
        for point in [Point::new(0, 0, 0), Point::new(1495, 6143, 0)] {
            let world = project(point);
            let (tx, ty) = unproject_ground(f64::from(world.x), f64::from(world.y));
            assert!((tx - f64::from(point.x)).abs() < 1e-9, "x: {tx}");
            assert!((ty - f64::from(point.y)).abs() < 1e-9, "y: {ty}");
        }
    }

    /// A pixel that is not a tile centre names the tile it is nearest, and the
    /// north-west of the map is where truncation would have named a different
    /// one — which is the whole reason for `div_euclid`.
    #[test]
    fn unproject_rounds_to_the_nearest_tile_on_both_sides_of_the_origin() {
        // A few pixels either side of a centre still name that tile.
        let centre = project(Point::new(100, 100, 0));
        for (dx, dy) in [(0, 0), (5, 0), (-5, 0), (0, 5), (0, -5), (-10, -10)] {
            let near = WorldPixel {
                x: centre.x + dx,
                y: centre.y + dy,
            };
            assert_eq!(unproject(near, 0), (100, 100), "{near:?}");
        }
        // North of tile (0, 0) is a negative tile, and it is reported as one
        // rather than clamped into the map.
        let above = WorldPixel { x: 0, y: -44 };
        assert_eq!(unproject(above, 0), (-1, -1));
    }

    /// A facet's corners stand where the ground pass lifts its vertices to, and
    /// a level one is the diamond exactly.
    ///
    /// The second half is what makes the first safe to rely on: the marker on a
    /// hillside and the marker on a floor come off one arithmetic, so the only
    /// thing that can move a corner is the slope.
    #[test]
    fn a_facet_lifts_each_corner_by_its_own_height() {
        let camera = Camera::new(Point::new(1000, 1000, 0), 800, 600);
        let point = Point::new(1000, 1000, 0);
        assert_eq!(camera.tile_facet(point, [0; 4]), camera.tile_diamond(point));
        // One corner up by four units is four `Z_STEP`s up the screen, and the
        // other three do not move.
        let raised = camera.tile_facet(point, [4, 0, 0, 0]);
        let level = camera.tile_diamond(point);
        assert_eq!(raised[0].y, level[0].y - (4 * Z_STEP) as f32);
        assert_eq!(raised[0].x, level[0].x);
        assert_eq!(raised[1..], level[1..]);
        // The lift is a difference from the point's own height, so a facet whose
        // corners all sit at `z` is the diamond at `z` wherever that is read
        // from — a tile does not shift because the height it was named by did.
        assert_eq!(
            camera.tile_facet(Point::new(1000, 1000, 7), [11; 4]),
            camera.tile_diamond(Point::new(1000, 1000, 11)),
        );
    }

    #[test]
    fn the_camera_puts_its_own_tile_in_the_middle() {
        let camera = Camera::new(Point::new(1000, 1000, 5), 800, 600);
        assert_eq!(
            camera.to_screen(Point::new(1000, 1000, 5)),
            ViewPixel { x: 400, y: 300 }
        );
    }

    /// The rule the whole camera hangs on. Checked at every rung, because the
    /// image's size changes with the zoom and a half that used the viewport's
    /// would agree with the other half only at 1:1.
    #[test]
    fn to_world_is_the_inverse_of_to_view_at_every_zoom() {
        let mut camera = Camera::new(Point::new(1495, 1629, 0), 1024, 768);
        let mut zoom = Zoom::ONE;
        while !zoom.is_widest() {
            zoom = zoom.scale_down();
        }
        loop {
            camera.zoom = zoom;
            for at in [
                WorldPixel { x: 0, y: 0 },
                camera.eye(),
                WorldPixel { x: -12_345, y: 6 },
                WorldPixel {
                    x: 100_000,
                    y: -70_000,
                },
            ] {
                assert_eq!(camera.to_world(camera.to_view(at)), at, "at {zoom}");
            }
            let next = zoom.scale_up();
            if next == zoom {
                break;
            }
            zoom = next;
        }
    }

    /// The ladder is walked, not indexed: nothing outside it is reachable.
    #[test]
    fn the_zoom_ladder_has_two_ends() {
        let mut zoom = Zoom::ONE;
        for _ in 0..20 {
            zoom = zoom.scale_up();
        }
        assert_eq!((zoom.numerator(), zoom.denominator()), (4, 1));
        assert!(!zoom.is_widest());
        for _ in 0..20 {
            zoom = zoom.scale_down();
        }
        assert_eq!((zoom.numerator(), zoom.denominator()), (1, 2));
        assert!(zoom.is_widest());
    }

    /// Zoomed out the offscreen image is bigger than the viewport, zoomed in
    /// smaller, and never short — a short image leaves a strip of the viewport
    /// with nothing blitted into it.
    #[test]
    fn the_drawn_image_covers_the_viewport_at_every_zoom() {
        let mut camera = Camera::new(Point::new(1000, 1000, 0), 1024, 768);
        let mut zoom = Zoom::ONE;
        while !zoom.is_widest() {
            zoom = zoom.scale_down();
        }
        loop {
            camera.zoom = zoom;
            let (num, den) = (zoom.numerator(), zoom.denominator());
            assert!(
                camera.render_width() * num >= camera.width * den,
                "{zoom} leaves {}px short of {}",
                camera.render_width(),
                camera.width,
            );
            // And not wastefully long: one world pixel of slack at most.
            assert!(
                camera.render_width() * num < (camera.width + num) * den,
                "{zoom} overshoots"
            );
            let next = zoom.scale_up();
            if next == zoom {
                break;
            }
            zoom = next;
        }
    }

    /// Zooming holds the cursor still. Not exactly — a world pixel is coarser
    /// than a viewport pixel when magnified, so the answer is only as precise as
    /// the space it is expressed in — but within one world pixel at every rung,
    /// which is what "feels placed" means.
    #[test]
    fn zooming_keeps_what_is_under_the_cursor_under_it() {
        let mut camera = Camera::new(Point::new(1495, 1629, 0), 1024, 768);
        let cursor = RealPixel::new(200, 700);
        let before = camera.pick(cursor);
        let mut zoom = camera.zoom();
        for _ in 0..8 {
            zoom = zoom.scale_up();
            camera.zoom_about(cursor, zoom);
            let after = camera.pick(cursor);
            assert!(
                (after.x - before.x).abs() <= 1 && (after.y - before.y).abs() <= 1,
                "{zoom}: {before:?} drifted to {after:?}",
            );
        }
    }

    /// The property that matters: `visible_tiles` may over-cover, but it may
    /// never miss. Anything `to_screen` puts inside the image has to be in the
    /// bounds — checked by walking tiles and projecting them, which is the other
    /// formula, so agreement is evidence and not a restatement.
    ///
    /// Re-run at every rung of the ladder, because the image the bounds cover
    /// grows with it.
    #[test]
    fn every_tile_that_lands_on_screen_is_inside_the_bounds() {
        let mut camera = Camera::new(Point::new(1000, 1000, 0), 800, 600);
        let mut zoom = Zoom::ONE;
        while !zoom.is_widest() {
            zoom = zoom.scale_down();
        }
        loop {
            camera.zoom = zoom;
            let bounds = camera.visible_tiles();
            let (width, height) = (camera.render_width() as i32, camera.render_height() as i32);

            let mut on_screen = 0;
            for x in 900..1100u16 {
                for y in 900..1100u16 {
                    for z in [-120i8, -10, 0, 10, 120] {
                        let point = Point::new(x, y, z);
                        let at = camera.to_screen(point);
                        if at.x < 0 || at.x >= width || at.y < 0 || at.y >= height {
                            continue;
                        }
                        on_screen += 1;
                        assert!(
                            i32::from(x) >= bounds.min_x
                                && i32::from(x) <= bounds.max_x
                                && i32::from(y) >= bounds.min_y
                                && i32::from(y) <= bounds.max_y,
                            "at {zoom}, {point} lands at {at:?} but {bounds:?} excludes it",
                        );
                    }
                }
            }

            // A pass with nothing on screen would assert nothing at all, and
            // would stay green through any change to either formula. The floor
            // is the image's own area in tiles — one diamond covers
            // `TILE_WIDTH * HALF_HEIGHT` pixels — because a magnified image
            // genuinely holds fewer tiles and a constant here would either fail
            // at 4x or assert nothing at 1/2x.
            let floor = width as i64 * height as i64 / (TILE_WIDTH * HALF_HEIGHT) as i64;
            assert!(
                i64::from(on_screen) > floor,
                "at {zoom}, only {on_screen} tiles landed on screen, against {floor}",
            );
            let next = zoom.scale_up();
            if next == zoom {
                break;
            }
            zoom = next;
        }
    }

    /// And the over-covering has to stay bounded, or "superset" becomes an
    /// excuse for drawing the map.
    ///
    /// A constant would not do it: zoomed out the image is four times the area,
    /// so the bound has to be a statement about the image's size rather than a
    /// number that happens to hold at 1:1. This is that statement, derived from
    /// the formula's own terms — the `u` and `v` spans it computes, converted to
    /// a square of tiles — with a tile of slack per bound for the roundings.
    #[test]
    fn the_bounds_do_not_grow_faster_than_the_image() {
        let mut camera = Camera::new(Point::new(1000, 1000, 0), 800, 600);
        let mut zoom = Zoom::ONE;
        while !zoom.is_widest() {
            zoom = zoom.scale_down();
        }
        loop {
            camera.zoom = zoom;
            let bounds = camera.visible_tiles();
            let tiles = (bounds.max_x - bounds.min_x + 1) as i64 * (bounds.max_y - bounds.min_y + 1) as i64;

            let u_span = (camera.render_width() as i64 + 2 * TILE_WIDTH as i64) / HALF_WIDTH as i64 + 4;
            let v_span = (camera.render_height() as i64 + 2 * TILE_HEIGHT as i64 + 2 * MAX_Z_LIFT as i64)
                / HALF_HEIGHT as i64
                + 4;
            let side = (u_span + v_span) / 2 + 4;
            assert!(
                tiles <= side * side,
                "at {zoom}, {bounds:?} covers {tiles} tiles against {}",
                side * side,
            );
            let next = zoom.scale_up();
            if next == zoom {
                break;
            }
            zoom = next;
        }
    }
}

#[cfg(test)]
mod difference_tests {
    use super::TileBounds;

    fn bounds(min_x: i32, max_x: i32, min_y: i32, max_y: i32) -> TileBounds {
        TileBounds {
            min_x,
            max_x,
            min_y,
            max_y,
        }
    }

    /// Every cell in a rectangle, as a set a test can compare.
    fn cells(rect: TileBounds) -> std::collections::BTreeSet<(i32, i32)> {
        let mut out = std::collections::BTreeSet::new();
        for y in rect.min_y..=rect.max_y {
            for x in rect.min_x..=rect.max_x {
                out.insert((x, y));
            }
        }
        out
    }

    /// The property the whole band walk rests on, stated over every pair of
    /// rectangles in a small window: the pieces are exactly the cells of the
    /// first that the second does not hold, and no cell appears twice.
    ///
    /// Both halves matter and they fail differently. A piece that is *missing*
    /// is a graphic never offered to the atlas, which draws nothing where it
    /// should have drawn a wall — silently, and only along one edge, and only
    /// for the camera direction that produced it. A cell counted *twice* is
    /// merely work done twice, which nothing would ever notice; asserting it
    /// anyway is what keeps a lazily-widened rectangle from becoming the
    /// "walk the whole viewport" this exists to replace.
    #[test]
    fn the_pieces_are_exactly_what_is_not_covered() {
        let range = -2..=2;
        for min_x in range.clone() {
            for max_x in min_x..=2 {
                for min_y in range.clone() {
                    for max_y in min_y..=2 {
                        let want = bounds(min_x, max_x, min_y, max_y);
                        for cx in range.clone() {
                            for cy in range.clone() {
                                let covered = bounds(cx, cx + 1, cy, cy + 2);
                                let pieces = want.difference(covered);

                                let mut union = std::collections::BTreeSet::new();
                                let mut total = 0;
                                for piece in pieces.into_iter().flatten() {
                                    let piece = cells(piece);
                                    total += piece.len();
                                    union.extend(piece);
                                }
                                assert_eq!(union.len(), total, "{want:?} minus {covered:?} overlaps itself");

                                let expected: std::collections::BTreeSet<(i32, i32)> =
                                    cells(want).difference(&cells(covered)).copied().collect();
                                assert_eq!(union, expected, "{want:?} minus {covered:?}");
                            }
                        }
                    }
                }
            }
        }
    }

    /// A camera that has not moved asks for nothing, which is the frame this
    /// whole arrangement exists to make free.
    #[test]
    fn a_rectangle_inside_the_covered_one_has_no_pieces() {
        let covered = bounds(0, 100, 0, 100);
        assert!(
            covered
                .difference(covered)
                .into_iter()
                .all(|piece| piece.is_none())
        );
        let inside = bounds(10, 20, 10, 20);
        assert!(
            inside
                .difference(covered)
                .into_iter()
                .all(|piece| piece.is_none())
        );
    }

    /// A step of one tile is one row, not a viewport. The number is the whole
    /// point of the band walk, so it is asserted rather than described.
    #[test]
    fn a_step_of_one_tile_uncovers_one_row() {
        let covered = bounds(0, 99, 0, 99);
        let moved = bounds(0, 99, 1, 100);
        let cells: usize = moved
            .difference(covered)
            .into_iter()
            .flatten()
            .map(|piece| cells(piece).len())
            .sum();
        assert_eq!(cells, 100, "one row of a 100-wide rectangle");
    }

    /// Nothing in common: the whole rectangle is new. A teleport, or a facet
    /// wide enough that the camera left everything it knew.
    #[test]
    fn a_disjoint_rectangle_is_uncovered_whole() {
        let covered = bounds(0, 10, 0, 10);
        let elsewhere = bounds(100, 110, 100, 110);
        let pieces: Vec<TileBounds> = elsewhere.difference(covered).into_iter().flatten().collect();
        assert_eq!(pieces, vec![elsewhere]);
    }

    /// The saturating arithmetic, at the edge that would wrap without it.
    #[test]
    fn the_extremes_do_not_wrap() {
        let covered = bounds(i32::MIN, i32::MAX, i32::MIN, i32::MAX);
        let want = bounds(-5, 5, -5, 5);
        assert!(want.difference(covered).into_iter().all(|piece| piece.is_none()));
    }
}
