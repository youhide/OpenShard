//! Firelight: the pools of warm light a torch, a brazier or a campfire lays on
//! the ground around it.
//!
//! # In the world's own units, not the screen's
//!
//! A light is a tile, a height and a reach in tiles; a fragment is lit according
//! to the tile *its own picture* came from, which the world passes wrote into
//! [`crate::place`]. The screen never enters it. It cannot: the screen folds
//! height into `y`, so a brazier in a cellar lands a few pixels from a lantern
//! on the street above, and a wall's picture stands 44 pixels above the tile it
//! occludes from — which puts the lit face of a wall inside its own shadow the
//! moment shadows exist at all. `docs/archive/render/lighting.md` is the argument at length.
//!
//! # Why it is a pass over the finished image and not a term in three shaders
//!
//! Everything here ends up as a handful of point lights in the *drawn image's*
//! own pixels, applied once by [`crate::blit`] on the way to the surface. The
//! alternative — a light term in `ground.wgsl`, `statics.wgsl` and the mobile
//! pass — is three copies of one formula, three uniform blocks to keep in step,
//! and a frame where a body walking past a fire is lit by a slightly different
//! curve than the flagstone it is standing on. There is nothing a per-object
//! pass would buy: UO's art is flat pictures with no normals, so "lit" means
//! exactly *brighter near the flame*, and where a pixel is on the screen is the
//! whole of what that needs.
//!
//! # What a light is, and what says so
//!
//! [`TileFlags::LIGHT_SOURCE`] — the client's own answer. A graphic burns
//! because `tiledata.mul` says it burns, not because this file holds a list of
//! torch graphics, which would be a list somebody has to maintain against every
//! art patch and would silently miss a shard's custom brazier.
//!
//! What the flag does *not* carry is how big the pool is or what colour it
//! burns: the client reads those from `light.mul`, keyed by a light id this
//! workspace's `uofiles` does not parse yet. Until it does, [`flame`] picks a
//! shape from the graphic — one warm default, and a wider, brighter one for a
//! campfire. That is a deliberate stand-in and it is the one thing here that is
//! invention rather than port; see `docs/client/evidence/2026-08-30-the-client-backlog.md`.
//!
//! # The flicker is on the CPU
//!
//! Two sine terms of incommensurable frequency, per light, sampled once per
//! frame and folded into the intensity that reaches the GPU. On the CPU because
//! a flame's brightness is one number for the whole pool — the shader would
//! recompute it identically for every pixel it touches — and because this crate
//! is not allowed to read a clock, so the time arrives as an argument and there
//! is exactly one place it is used.

use openshard_map::map::WorldMap;
use openshard_protocol::direction::Direction;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_tiles::TileData;

use crate::camera::Camera;
use crate::cutaway::{
    self,
    Cutaway,
};
use crate::facing::Face;
use crate::geometry::Vec2;
use crate::items::GroundItem;
use crate::occlusion::{
    Edges,
    Occlusion,
};

/// One point light, where it stands in the world.
///
/// Tile coordinates and a `z`, not pixels: what a fragment is lit by depends on
/// the tile *it* came from — see [`crate::place`] — and a pool measured on the
/// screen would be a circle drawn over a projection that folds height into `y`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Light {
    /// The tile it burns on, `x` and `y`.
    ///
    /// Floats because the shader compares them against a fragment's tile and
    /// there is nothing to be gained by converting twice; every value here came
    /// from a `u16` and is exact.
    pub at:        Vec2,
    /// Its height, in the map's own `z` units.
    pub z:         f32,
    /// How far its pool reaches, **in tiles**. Nothing beyond this is touched at
    /// all, which is what keeps the shader's loop cheap and the pool a shape
    /// rather than a global tint.
    pub radius:    f32,
    /// Its colour, linear, each channel in `0..=1`.
    pub color:     [f32; 3],
    /// How brightly it burns at its centre, flicker already folded in. Above
    /// `1.0` is ordinary: a fire blows out the ground it stands on.
    pub intensity: f32,
    /// Which way it throws its light, where it throws it one way at all — see
    /// [`Beam`]. `None` is a fire in the open, which lights every direction
    /// equally, and it is what everything on the map is.
    pub beam:      Option<Beam>,
}

/// A flame that lights one direction and not the others: a hooded lantern, or a
/// torch held out in front of a face.
///
/// A cone and not a second radius. Everything else about the light is unchanged
/// — the same falloff, the same three-dimensional distance, the same walk of the
/// grid for what stands in the way — and this multiplies the result by how far
/// inside the cone the lit spot is. That ordering is the whole of why a beam is
/// cheap: a fragment outside the radius never asks about the angle, and one
/// outside the cone never walks the ray.
///
/// Both ends of the cone are cosines rather than angles because the test is a
/// dot product: the direction from the flame to the spot against the axis, both
/// unit vectors in the same units the distance is in — [`TileVec`]'s space,
/// which is what keeps a beam pointing along the ground from lighting the storey
/// above.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Beam {
    /// Where it points, unit length. Built by [`Beam::towards`], which is the
    /// only thing that makes one — a direction of some other length would make
    /// the dot product below mean nothing.
    pub toward:   TileVec,
    /// The cosine of its half-angle: the rim of the cone. A spot whose direction
    /// is below this gets nothing at all.
    pub cos_half: f32,
}

/// How far in from the rim of a beam its edge finishes softening, as a share of
/// the way from the rim to the axis.
///
/// A cone with a hard rim reads as a stencil laid over the scene rather than as
/// light — the eye finds the straight edge immediately, the same way it finds
/// the tile boundary a shadow used to end on. A quarter of the way in is enough
/// to lose it and narrow enough that a sixty-degree beam still looks sixty
/// degrees wide. Invented here, like [`FLAME_SPREAD`] and
/// [`crate::occlusion::PANE`]: no client file has a number for the shape of a
/// lantern's shutter. `blit.wgsl`'s `BEAM_EDGE`, and the two are one number.
const BEAM_EDGE: f32 = 0.25;

/// How much of a beamed flame escapes it in every other direction.
///
/// A hand is not a shutter. What makes a carried torch a beam at all is that the
/// arm holds it out in front and the body is behind it, and neither of those
/// stops the flame from being a flame: the ground at the character's feet is lit,
/// and so is the character. A cone with nothing outside it puts the one thing the
/// player is looking at — their own body — in the only black hole in the frame,
/// which is the opposite of what a light in the hand is for.
///
/// A quarter, so that the beam is still obviously a beam: what is in front is
/// four times what is beside, which reads as a direction at a glance. Invented
/// here like [`BEAM_EDGE`], and `blit.wgsl`'s `BEAM_SPILL` is the same number.
pub const BEAM_SPILL: f32 = 0.25;

impl Beam {
    /// A beam of `degrees` across — the *full* angle, the way a lamp is
    /// described — pointing along `(dx, dy)` with `rise` tiles of climb for
    /// every tile along the ground.
    ///
    /// The full angle and not the half is what a person says out loud, and the
    /// halving belongs at the one place the number is turned into a cosine
    /// rather than at every call site.
    ///
    /// A direction of no length at all is taken as north, for the reason
    /// [`Sun::towards`] takes it as south: a zero axis would make every dot
    /// product zero and the cone would silently become a hemisphere.
    pub fn towards(dx: f32, dy: f32, rise: f32, degrees: f32) -> Self {
        let (dx, dy) = match dx.abs() + dy.abs() < 1e-4 {
            true => (0.0, -1.0),
            false => (dx, dy),
        };
        let length = (dx * dx + dy * dy + rise * rise).sqrt();
        Self {
            toward:   TileVec::new(dx / length, dy / length, rise / length),
            cos_half: (degrees.to_radians() / 2.0).cos(),
        }
    }

    /// How much of this beam falls on a spot `offset` away from the flame — in
    /// [`TileVec`]'s space, pointing *from* the flame *to* the spot.
    ///
    /// `blit.wgsl`'s `cone`, arithmetic for arithmetic, and the parity test of
    /// `docs/archive/render/lighting.md`'s decision 9 is what says so. The smoothstep is
    /// written out rather than called, because WGSL's built-in and a Rust crate's
    /// are two texts that can disagree and this is one polynomial either way.
    ///
    /// Never zero: [`BEAM_SPILL`] is the floor, and a spot at the flame itself
    /// gets the whole of it — there is no direction from a point to itself, and
    /// the tile a lantern is standing on is not the place to start refusing
    /// light.
    pub fn lights(self, offset: TileVec) -> f32 {
        let length = offset.length();
        if length < 1e-6 {
            return 1.0;
        }
        let along = self.toward.dot(offset.divided(length));
        let inner = self.cos_half + (1.0 - self.cos_half) * BEAM_EDGE;
        let t = ((along - self.cos_half) / (inner - self.cos_half).max(1e-6)).clamp(0.0, 1.0);
        BEAM_SPILL + (1.0 - BEAM_SPILL) * t * t * (3.0 - 2.0 * t)
    }
}

/// The sun: one direction for the whole world, and what it does where nothing
/// stands in the way.
///
/// Not a sixty-fifth flame. A flame is a point and the walk to it is bounded by
/// its radius; the sun has no position, so every fragment walks the *same*
/// direction until the ray leaves the grid or is stopped — which is what gives a
/// wall a shadow lying across the street, and a window a bright patch on the
/// floor behind it. `docs/archive/render/lighting.md`, decision 12.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Sun {
    /// Which way the sun is, from anywhere, in [`TileVec`]'s space — the same
    /// unit the distance to a flame is in, so that an elevation of 45° really is
    /// one tile up per tile along. Normalised by [`Sun::towards`], which is the
    /// only thing that builds one.
    pub toward:    TileVec,
    /// Its colour, linear.
    pub color:     [f32; 3],
    /// How much it adds where it reaches. Zero is "no sun", and the blit skips
    /// the walk entirely for it — which is what keeps a frame that has no sun
    /// exactly as cheap as it was before there was one.
    pub intensity: f32,
}

impl Sun {
    /// A sun `rise` tiles up for every tile along `(dx, dy)`.
    ///
    /// The elevation is stated as a slope rather than as an angle because that is
    /// what the walk uses and because a slope is the thing with a picture: `1.0`
    /// is 45°, and a wall twenty units tall — two tiles' worth of `z` — throws
    /// its shadow two tiles.
    ///
    /// A `(dx, dy)` of nothing at all is taken as straight down the `y` axis
    /// rather than left to produce a direction of zero length: a sun with no
    /// azimuth is overhead, and overhead in this projection is a degenerate case
    /// that would silently make every fragment sunlit.
    pub fn towards(dx: f32, dy: f32, rise: f32, color: [f32; 3], intensity: f32) -> Self {
        let (dx, dy) = match dx.abs() + dy.abs() < 1e-4 {
            true => (0.0, -1.0),
            false => (dx, dy),
        };
        let length = (dx * dx + dy * dy + rise * rise).sqrt();
        Self {
            toward: TileVec::new(dx / length, dy / length, rise / length),
            color,
            intensity,
        }
    }

    /// How steeply it climbs per tile along the ground: the slope
    /// [`Sun::towards`] was given back, whatever the direction was normalised to.
    pub fn rise_per_tile(self) -> f32 {
        let horizontal = (self.toward.x * self.toward.x + self.toward.y * self.toward.y).sqrt();
        match horizontal < 1e-6 {
            true => f32::INFINITY,
            false => self.toward.z / horizontal,
        }
    }
}

/// How far along the ground one sunbeam may run, in tiles.
///
/// The bound the ray needs and a flame's does not — see [`Sun`]. What ends a
/// sunbeam is the grid's ceiling: a ray that has climbed above everything in the
/// frame is looking at sky, which for a street of one-storey buildings is two or
/// three tiles out. This is what is left for a sun so low that it never climbs
/// out — a shadow thirty-two tiles long is already longer than any frame, and
/// without it a sunset would be a segment with no end. `blit.wgsl`'s
/// `MAX_SUN_TILES`, and the two are one number.
pub const MAX_SUN_TILES: f32 = 32.0;

/// How many `z` units make one tile's width.
///
/// `TILE_WIDTH / Z_STEP`: a tile is 44 virtual pixels across and one unit of
/// height lifts a sprite four, so eleven units of `z` are one tile of ground.
/// It is what lets a distance have all three axes in one unit, and with it a
/// flame reaches as far up and down as it does sideways — which is what stops a
/// cellar's brazier from lighting the street even where nothing occludes.
pub const Z_PER_TILE: f32 = (crate::camera::TILE_WIDTH / crate::camera::Z_STEP) as f32;

/// A direction or an offset in **tile space**: all three axes in tiles.
///
/// `docs/render/design_pixel_spaces.md` P3, and the grid that phase found genuinely missing a type.
/// Two three-vector spaces meet in this module and nothing but prose told them
/// apart:
///
/// - **world units** — `x` and `y` in tiles, `z` in the map's own height units,
///   which is what [`Light::z`], [`Spot::z`], [`crate::impostor::Volume`] and
///   every position on the wire are stated in. Positions live here.
/// - **tile space** — this. `z` divided by [`Z_PER_TILE`], so all three axes
///   share a unit and a *length* means something. Every metric the lighting
///   model states — a distance, a cosine, a beam's axis, a surface's normal —
///   lives here, because a falloff measured with `z` in its own units would
///   reach eleven times as far up as sideways.
///
/// The two are one multiplication apart, which is exactly why they were
/// confusable: `[f32; 3]` from one appeared in the same expression as `[f32; 3]`
/// from the other — [`flame_points`] adds a tile-space offset to a world-units
/// centre, [`walk_sun`] turns a tile-space direction into a world-units step —
/// and the compiler had nothing to say about it. [`TileVec::between`] and
/// [`TileVec::in_world_units`] are now the only two crossings, so [`Z_PER_TILE`]
/// appears in a metric expression exactly twice rather than at eight sites that
/// each had to remember it.
///
/// Deliberately *not* an all-purpose vector type. It has the operations the
/// lighting metric needs and no normaliser: the three places that normalise here
/// guard a different epsilon each ([`lit_from`] at zero, [`Beam::lights`] and
/// [`flame_points`] at `1e-6`), and folding them into one would change three
/// answers to make one type tidier.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct TileVec {
    /// East, in tiles.
    pub x: f32,
    /// South, in tiles.
    pub y: f32,
    /// Up, **in tiles** — the map's `z` already divided by [`Z_PER_TILE`].
    pub z: f32,
}

impl TileVec {
    /// A vector already stated in this space: a fixed normal, an axis a caller
    /// described in tiles per tile.
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// The offset from one point to another, both in **world units**, expressed
    /// in tile space.
    ///
    /// One of the two crossings, and the direction almost every caller wants:
    /// what the world holds is positions, and what the lighting model asks about
    /// is the vector between two of them.
    pub fn between(from: WorldVec, to: WorldVec) -> Self {
        Self {
            x: to.x - from.x,
            y: to.y - from.y,
            z: (to.z - from.z) / Z_PER_TILE,
        }
    }

    /// Back to world units, for adding to a position or stepping a ray.
    ///
    /// The other crossing. Everything that walks the grid does so in world units,
    /// because that is what the boxes in it are stated in.
    pub fn in_world_units(self) -> WorldVec {
        WorldVec::new(self.x, self.y, self.z * Z_PER_TILE)
    }

    /// The three axes as they stand, for the one place a vector leaves Rust: the
    /// uniform the shader reads. Not a general escape hatch — a caller that
    /// wants arithmetic wants the methods below.
    pub fn axes(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    /// Its length, in tiles. The reason this space exists.
    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    /// The dot product with another vector of the same space.
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    /// The cross product — a vector across both, which is what a disc's two
    /// spanning directions are built from.
    pub fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    /// Every axis multiplied by `k`.
    pub fn scaled(self, k: f32) -> Self {
        Self {
            x: self.x * k,
            y: self.y * k,
            z: self.z * k,
        }
    }

    /// Every axis divided by `k`.
    ///
    /// A sibling of [`TileVec::scaled`] and not sugar for it: `a / k` and
    /// `a * (1 / k)` are two different roundings, and the walks here are compared
    /// against a shader that writes the division.
    pub fn divided(self, k: f32) -> Self {
        Self {
            x: self.x / k,
            y: self.y / k,
            z: self.z / k,
        }
    }

    /// Axis by axis with another vector of the same space.
    pub fn plus(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }

    /// The same vector pointing the other way.
    pub fn negated(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }
}

/// A position or offset in **world units**: `x` and `y` in tiles, `z` in the
/// map's own height units — the other of the two spaces [`TileVec`]'s doc
/// documents, `docs/render/design_pixel_spaces.md` P3.
///
/// This is what every position in the world is stated in: [`Light::z`],
/// [`Spot::z`], [`crate::impostor::Volume`]'s corners, a ray's origin. Before
/// this type the two spaces were both a bare `[f32; 3]`, told apart only by
/// which variable name a reader happened to be looking at — a `lo`/`hi` pair
/// in `impostor.rs` gave no sign of which of the two `z` units it held. The
/// crossings into tile space are [`TileVec::between`] and
/// [`TileVec::in_world_units`]; nothing else converts between the two, so a
/// mismatch is now a type error rather than a wrong answer eleven times too
/// large or too small.
///
/// Deliberately as bare as [`TileVec`]: no arithmetic beyond what a call site
/// has actually needed, and [`WorldVec::array`]/[`WorldVec::from_array`] are
/// the one escape hatch — for the wire, and for the handful of places that
/// index an axis at runtime rather than name it — not a general `Index` impl.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct WorldVec {
    /// East, in tiles.
    pub x: f32,
    /// South, in tiles.
    pub y: f32,
    /// Up, in the map's own height units — [`Z_PER_TILE`] of these make one
    /// tile of [`TileVec::z`].
    pub z: f32,
}

impl WorldVec {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// The three axes as a plain array — for [`crate::impostor::meets`]'s
    /// axis-generic slab test, which needs runtime indexing, and for the wire.
    pub const fn array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    /// The inverse of [`WorldVec::array`].
    pub const fn from_array(a: [f32; 3]) -> Self {
        Self {
            x: a[0],
            y: a[1],
            z: a[2],
        }
    }
}

/// The light a place has before anything burns in it: the sky's share, and the
/// floor under it.
///
/// `docs/archive/render/lighting_world.md`, decision 1. One colour for the whole frame lit the
/// inside of a house exactly as brightly as the street outside it, because
/// nothing in the ambient knew what a roof was — a dungeon was dark only because
/// the server had said the whole world was. Split in two:
///
/// ```text
/// ambient(tile) = sky * sky(tile) + ground
/// ```
///
/// `sky(tile)` is [`crate::occlusion::Occlusion::sky_at`]'s byte, and `ground`
/// is the small, cold floor a windowless cellar still gets — so that a room with
/// no torch in it is deep rather than pure black. An unlit black rectangle is not
/// atmosphere, it is a bug report.
///
/// Both terms are colours and not levels: a sky is blue where a cellar's floor
/// light is bluer still, and a term that was one number could only ever say how
/// *much* light a place has and never what kind.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ambient {
    /// What a tile with an open column above it gets, in full.
    pub sky:    [f32; 3],
    /// What every tile gets, roof or no roof.
    pub ground: [f32; 3],
}

impl Ambient {
    /// Full daylight under an open column and nothing under a lid: the ambient
    /// at which the blit is a copy of the world image.
    ///
    /// Only the *open* half is the identity, which is the whole of decision 1
    /// arriving in the one constant that used to mean "no lighting at all" —
    /// see [`Lighting::is_identity`], which is why it now asks about the grid.
    pub const DAY: Self = Self {
        sky:    [1.0, 1.0, 1.0],
        ground: [0.0, 0.0, 0.0],
    };

    /// The same light with the sky's share folded into the floor: one colour for
    /// every tile, whatever stands over it.
    ///
    /// **The ambient this pass had before the sky field existed**, and the switch
    /// back to it is deliberate rather than a leftover. What a roof does to the
    /// light under it is a whole plan of its own
    /// (`docs/archive/render/lighting_world.md`), and while the *point* lights are being got
    /// right it is a second thing changing every tile of every picture: a pool
    /// that looks wrong indoors is then two questions, and the field answers the
    /// one nobody asked. Flat is also the honest baseline — it is what a shard
    /// with no time of day and no roofs looks like — so a difference between the
    /// two pictures is the field's whole contribution, which is what a person
    /// turning it on wants to see.
    ///
    /// The sum and not either half: the two terms were split out of one colour and
    /// they still add up to it, so a flattened [`NIGHT`] is exactly the night this
    /// had before the split.
    pub fn flattened(self) -> Self {
        let mut ground = self.ground;
        for (channel, sky) in ground.iter_mut().zip(self.sky) {
            *channel += sky;
        }
        Self {
            sky: [0.0; 3],
            ground,
        }
    }

    /// What a tile is multiplied by, given how much of the sky it can see.
    ///
    /// `blit.wgsl` does this same arithmetic per fragment out of the field
    /// plane, and the two are held together by the parity test of
    /// `docs/archive/render/lighting.md`'s decision 9.
    pub fn at(self, sky: u8) -> [f32; 3] {
        let share = f32::from(sky) / f32::from(crate::occlusion::SKY_OPEN);
        let mut lit = self.ground;
        for (channel, sky) in lit.iter_mut().zip(self.sky) {
            *channel += sky * share;
        }
        lit
    }

    /// Whether an open tile already has the full daytime multiplier.
    ///
    /// [`Self::flattened`] spells that same day as a zero sky term and a full
    /// ground term, so comparing this ambient with [`Self::DAY`] would miss the
    /// ordinary client picture. The carried lantern is invisible in either
    /// spelling, and need not make the deferred pass walk its rays.
    pub fn is_full_daylight(self) -> bool {
        self.at(crate::occlusion::SKY_OPEN) == [1.0; 3]
    }
}

/// Everything the blit needs to light a frame.
///
/// [`Lighting::NONE`] is the identity — full ambient, no lights, nothing
/// standing anywhere — and the blit multiplies by exactly `1.0` for it, so a
/// frame test comparing the surface with the world image texel for texel still
/// holds.
#[derive(Clone, PartialEq, Debug)]
pub struct Lighting {
    /// What everything is multiplied by away from any flame — the daylight, or
    /// the lack of it, per tile. [`Ambient::DAY`] over an empty grid is "no
    /// lighting at all".
    pub ambient:      Ambient,
    /// The flames themselves, nearest first and never more than
    /// [`Lighting::MAX`] of them.
    pub lights:       Vec<Light>,
    /// What stands between them and the ground — see [`crate::occlusion`].
    ///
    /// Travels with the lights rather than beside them because it is the same
    /// frame's answer built from the same walk: a grid collected for one camera
    /// and used with another's flames would put shadows where the map has no
    /// walls.
    pub occlusion:    Occlusion,
    /// The sun, where there is one — see [`Sun`]. `None` is night, or a frame
    /// that has not been given a sky yet, and costs nothing at all: the shader
    /// never walks a ray for it.
    pub sun:          Option<Sun>,
    /// Which of the pass's own values to draw instead of the lit frame — see
    /// [`crate::debug::View`], and `docs/archive/render/lighting.md`'s decision 8 for why the
    /// diagnostics are branches of this pass rather than a second one.
    ///
    /// Here rather than in [`crate::blit::Frame`] because it is read where the
    /// lights are read, out of the same uniform block, and a second channel into
    /// the same shader is a second thing to keep in step.
    pub view:         crate::debug::View,
    /// How big every flame in this frame is, in tiles — the radius of the sphere
    /// [`arrival`] casts its [`SHADOW_RAYS`] at.
    ///
    /// **[`FLAME_RADIUS`] is the answer, and this is a field so that a
    /// *comparison* can ask for zero.** A sphere is what a penumbra is made of
    /// and phase 5 is not being undone; but a sphere is also an *estimate* —
    /// eight rays against a reference's sixty-four paths — and a gate laid
    /// against a path tracer then reports that estimate's noise as a
    /// disagreement. On the run-of-flights scene it reported exactly eight
    /// pixels of it, all at a graze six thousandths of a tile deep, and at zero
    /// radius the same comparison is exact on all 252,949 pixels. A knob is what
    /// tells "the walk is wrong" from "the two rulers disagree about a soft
    /// edge"; a constant cannot be asked.
    ///
    /// A frame's, not a flame's: every flame in the world is the same size, and
    /// a per-light radius would be a second meaning for [`Light::radius`], which
    /// is a *reach* and not a size.
    pub flame_radius: f32,
    /// How many rays this frame casts at each flame — [`ShadowRays`], and
    /// [`Tuning::shadow_rays`] is where it comes from.
    ///
    /// A field for the same reason [`Lighting::flame_radius`] is one, and one
    /// more besides: the count is now a *person's* number, and the shader reads
    /// it off this frame's header. Both walks read this field, so a frame asked
    /// about by [`sample`] and drawn by `blit.wgsl` casts the same rays at the
    /// same points — which is what keeps every parity oracle comparing two
    /// answers rather than two sample counts.
    pub shadow_rays:  ShadowRays,
    /// Whether the player's own character is a ghost — `view::Player::dead`,
    /// `0x2C`. `docs/combat/design_fight_loop.md`'s D9: one uniform, one branch, and the whole
    /// lit frame desaturates rather than every quad carrying its own hue.
    pub dead:         bool,
}

impl Lighting {
    /// How many lights one frame may carry.
    ///
    /// A fixed-size uniform array rather than a storage buffer, because the
    /// ceiling this crate draws under is WebGL2 and a storage buffer is not in
    /// it — see the crate docs. Sixty-four is a tavern's worth of candles;
    /// past that [`collect`] keeps the ones nearest the player.
    pub const MAX: usize = 64;

    /// The frame nothing lights: the world image, unchanged.
    pub const NONE: Self = Self {
        ambient:      Ambient::DAY,
        lights:       Vec::new(),
        occlusion:    Occlusion::EMPTY,
        sun:          None,
        view:         crate::debug::View::Lit,
        flame_radius: FLAME_RADIUS,
        shadow_rays:  ShadowRays::DEFAULT,
        dead:         false,
    };

    /// Whether this would change a single pixel.
    ///
    /// The occluders *are* asked about now, and that is decision 1 of
    /// `docs/archive/render/lighting_world.md` arriving here: a wall with no flame to stop
    /// still casts nothing, but a roof takes the sky's share of the ambient away
    /// from the tile under it whether anything burns or not. A grid with
    /// something in it is therefore a frame that may be darker than its world
    /// image, and only an empty one is a copy.
    ///
    /// Put a flame into the frame that no walk of the map could have found: the
    /// one the player is carrying.
    ///
    /// First in the list and never the one dropped. [`collect`] keeps the
    /// [`MAX`](Self::MAX) flames nearest the eye, and the flame *in the eye's own
    /// hand* is the one whose absence would be noticed instantly — a torch that
    /// went out because the player walked into a lit tavern is a worse frame than
    /// one candle at the far end of it going missing.
    pub fn hold(&mut self, light: Light) {
        self.lights.insert(0, light);
        self.lights.truncate(Self::MAX);
    }

    /// A debug view is never the identity, however empty the frame's lighting is
    /// — that is the whole of what it draws. Neither is a ghost's frame: `dead`
    /// desaturates every pixel the world image has, which is the one change
    /// this struct can make without a light, a wall or a view asking for it.
    pub fn is_identity(&self) -> bool {
        self.lights.is_empty()
            && self.ambient == Ambient::DAY
            && self.occlusion.is_empty()
            && self.sun.is_none_or(|sun| sun.intensity <= 0.0)
            && self.view.is_lit()
            && !self.dead
    }

    /// Whether this frame has only a flat ambient term left to apply.
    ///
    /// A roof changes a non-zero sky term even with neither a flame nor the
    /// sun, so this requires that term to have already been flattened. It is
    /// therefore not an identity, but the fragment path need not inspect any
    /// G-buffer attachment or object instance row.
    pub fn is_ambient_only(&self) -> bool {
        self.lights.is_empty()
            && self.sun.is_none_or(|sun| sun.intensity <= 0.0)
            && self.ambient.sky == [0.0; 3]
            && self.view.is_lit()
            && !self.dead
    }
}

/// The floor under the darkness: what a tile with no sky at all still gets.
///
/// Decision 1's `GROUND_AMBIENT`, and it is small and cold on purpose. Small,
/// because the whole of what the split buys is that a room is darker than the
/// road outside it, and a generous floor gives that back. Cold, because it
/// stands in for light that has bounced off a stone floor and a plastered wall
/// rather than for a source — and because a warm floor would take the one hue a
/// flame has to itself.
///
/// Invented here, in the way `docs/archive/render/lighting_world.md`'s decision 11 says every
/// number in this plan is: held by a scene, not argued into existence.
///
/// **Linear**, like every light quantity in this module since
/// `docs/render/design_model.md`'s phase 1. It was authored as `[0.12, 0.13, 0.18]`
/// — how dark the floor *looks* — back when the shader multiplied stored sRGB
/// bytes, and that is what those numbers meant: a fraction of a **displayed**
/// value. Now the multiplication happens in linear radiance, so the authored
/// intent is `srgb_to_linear` of each, which is what these are.
/// `the_authored_light_values_are_their_own_srgb_intent` asserts the pair, so
/// the artistic number is not lost and the two cannot drift.
pub const GROUND_AMBIENT: [f32; 3] = [0.013_412, 0.015_325, 0.027_212];

/// Night, as the reference isometrics draw it: dark, and *cooler* than the art.
///
/// The blue cast is what makes a fire read as warm — with a grey ambient the
/// pool and the dark are the same hue at two brightnesses, which the eye reads
/// as a spotlight rather than as firelight.
///
/// The two terms sum to the `[0.30, 0.33, 0.45]` this was one colour of before
/// the split, so a street at night is exactly as dark as it was and what changed
/// is only what happens indoors.
///
/// Linear, and authored as `sky: [0.20, 0.22, 0.31]`, `ground: [0.10, 0.11,
/// 0.14]` — see [`GROUND_AMBIENT`] for why those are not the numbers here. This
/// is the constant the change is most visible on: `0.20` of a displayed value is
/// a dark street, and `0.20` of *radiance* is a bright overcast afternoon.
pub const NIGHT: Ambient = Ambient {
    sky:    [0.033_105, 0.039_682, 0.078_288],
    ground: [0.010_023, 0.011_645, 0.017_389],
};

/// What a daylit world is lit by *away from the sun*: the sky.
///
/// Well short of white, because with a sun in the frame the sun supplies the
/// rest — an ambient that already lit everything would leave every shadow the
/// sun casts invisible. And well short of black, because a shadow at noon is not
/// a hole: the reference isometrics draw one lit by the sky, and so does this.
///
/// Split like [`NIGHT`] and for the same reason: the two terms sum to the
/// `[0.55, 0.55, 0.62]` a daylit frame had everywhere, so the street is
/// unchanged and the room under the roof is what moved.
/// Linear, authored as `[0.43, 0.42, 0.44]`.
pub const SKYLIGHT: Ambient = Ambient {
    sky:    [0.154_872, 0.147_319, 0.162_647],
    ground: GROUND_AMBIENT,
};

/// The sun this client stands under until there is a time of day on the wire.
///
/// Towards `+x` and one tile up for every tile along — 45°, so a wall twenty
/// units tall throws a shadow two tiles long. Both numbers are placeholders in
/// exactly the way [`flame`] is: what a shard's sky is doing is the shard's to
/// say, and when it does, this is the function that goes and no call site
/// changes.
pub fn midday() -> Sun {
    // Linear, authored as the colour `[1.0, 0.97, 0.88]` at `0.55` — see
    // [`GROUND_AMBIENT`] for why the numbers moved and the constants did not.
    Sun::towards(1.0, 0.0, 1.0, [1.0, 0.933_107, 0.748_414], 0.263_273)
}

/// How one kind of flame burns, before the flicker.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Flame {
    /// The pool's reach, in tiles. The world's own unit: what it lights is a
    /// span of ground, and no zoom changes how much ground that is.
    pub radius:    f32,
    /// Its colour, linear.
    pub color:     [f32; 3],
    /// Its brightness at the centre, before the flicker multiplies it.
    pub intensity: f32,
    /// How much the flicker swings that brightness, as a fraction of it. A
    /// candle gutters; a bonfire mostly does not.
    pub flicker:   f32,
}

/// A torch, a candle, a lantern: the ordinary flame, and what anything flagged
/// as a light source gets unless it is named below.
const TORCH: Flame = Flame {
    // Six tiles. The reference isometrics light a good deal more than the tile
    // the fire is on — a pool a tile wide reads as a bug, not as a torch.
    radius:    6.0,
    // Linear, authored as `[1.0, 0.72, 0.36]` at `0.95` — [`GROUND_AMBIENT`].
    color:     [1.0, 0.477_000, 0.106_539],
    intensity: 0.890_005,
    flicker:   0.10,
};

/// A campfire: wider, brighter, steadier.
const CAMPFIRE: Flame = Flame {
    radius:    9.0,
    // Linear, authored as `[1.0, 0.66, 0.30]` at `1.25`. The intensity is past
    // the range sRGB is defined on, so it carries the curve's exponent alone:
    // `1.25^2.4`. A fire brighter than white is ordinary and is exactly what a
    // tonemap is for.
    color:     [1.0, 0.393_123, 0.073_239],
    intensity: 1.708_378,
    flicker:   0.07,
};

/// The graphics a campfire cycles through.
///
/// `0x0DE3` is the campfire the client draws for a lit camp, and the four after
/// it are the rest of its animation — see `crate::animate`, which is what
/// decides *which* of them is on screen. All five burn the same, so the range
/// is matched rather than the frame.
const CAMPFIRE_GRAPHICS: std::ops::RangeInclusive<u16> = 0x0DE3..=0x0DE7;

/// How a graphic burns.
///
/// The stand-in for `light.mul` described in this module's header: the flag
/// says a graphic is a light and this says what kind, by name where the graphic
/// is one worth naming and by a warm default everywhere else. When `light.mul`
/// is read, this is the function that goes — and its callers do not change.
pub fn flame(graphic: Graphic) -> Flame {
    match CAMPFIRE_GRAPHICS.contains(&graphic.0) {
        true => CAMPFIRE,
        false => TORCH,
    }
}

/// Whether a static is a flame at all: it says it is a light source, **and it is
/// not something light cannot get through**.
///
/// The second half is what stops a city burning at every window. 615 of the
/// install's statics carry `LIGHT_SOURCE` and 80 of the 163 named "window" are
/// among them — `0x0103`, `0x2BBF`, the shutters at `0x2501`, the windowed walls
/// at `0x2B7D`. Every one of them is also a wall: `WALL | BLOCK | WINDOW`, which
/// is an occluder, and [`flame`] answers `TORCH` for any graphic it has no name
/// for. So a street of houses was a street of six-tile warm pools with nothing
/// burning in them, each one standing inside the very panel that then cut it into
/// slices.
///
/// **A window is not an emitter.** It is a hole with glass in it, it is already
/// in the occlusion grid as [`crate::occlusion::PANE`], and what should make it
/// glow is a candle behind it — which is the one thing this pass can already do.
/// The flag on those graphics is the client's way of saying "draw a glow here",
/// and this renderer answers that question with geometry instead.
///
/// Stated as "does it stop light" rather than as a list of window graphics,
/// because that is the property that matters and it is already computed for the
/// grid: a torch, a candle and a brazier stop nothing and burn; a glazed wall
/// stops four fifths and does not. A shard's custom lantern goes on burning for
/// free, and a shard's custom glowing wall stops — which is the conservative
/// direction, a missing pool being easier to see than sixty invented ones.
pub fn burns(graphic: Graphic, tile: &openshard_tiles::StaticTile) -> bool {
    tile.flags.is_light_source() && crate::occlusion::opacity(graphic, tile) == crate::occlusion::CLEAR
}

/// How far above its tile a flame burns, in `z` units.
///
/// A torch's flame is at the top of the sprite and the pool is centred under it,
/// not on the ground the sprite stands on. Half a tile up — [`Z_PER_TILE`] over
/// two — which is where the flame of a waist-high brazier is and close enough
/// for a wall sconce; the sprite's real height is not available here, and asking
/// the atlas for it would tie the lights to whether this frame's art happened to
/// be packed.
///
/// `pub` since phase 3, and for a reason worth stating: [`gather`] adds it to
/// every light the engine builds, so a flame at a tile's own `z` is one nothing
/// in this crate produces. That did not matter while the shading term was a
/// half-space — a flame lying exactly in the ground's plane got the band's own
/// half rather than nothing — and it matters now, because the cosine of a
/// source *in* a surface is zero and a scene that puts one there is asking about
/// a degenerate case rather than about a torch. A test writing `z: 0.0` was
/// stating "on the ground" and meaning "where a fire on the ground burns"; this
/// is the second one, and it is the engine's own number rather than a plausible
/// one chosen beside it.
pub const FLAME_LIFT: f32 = Z_PER_TILE / 2.0;

/// How many tiles beyond the drawn image a flame can still light it from.
///
/// **A light is not culled by where its sprite is.** [`Camera::visible_tiles`]
/// covers the tiles whose *pictures* can land in the frame, widened by a tile
/// for the sprite's own size — which is exactly the wrong rectangle here,
/// because a pool reaches [`CAMPFIRE`]`.radius` past the thing making it. Walked
/// with the drawing bounds, a lamp's pool vanishes the instant the lamp leaves
/// the screen instead of sliding off it, and every edge of the frame pops as the
/// camera pans. Measured on Britain at the widest zoom: 88 light sources stood
/// in the band this constant adds, all of them reaching into the frame and none
/// of them drawn.
///
/// Now that a reach is stated in tiles, the number *is* the widest pool, plus
/// one for the rounding. It is also the margin the occlusion grid is built over:
/// a wall outside it could not shadow anything the frame draws, because no flame
/// inside it reaches that far.
///
/// **A function of the frame's own [`Tuning::reach`]**, and not a constant, for
/// exactly that reason: the sentence above is only true while the widest pool
/// really is `CAMPFIRE.radius` wide. A person who turns the reach up and leaves
/// this where it was gets flames collected from too small a rectangle — a pool
/// that pops in as its own tile enters the margin — over a grid that holds no
/// walls that far out, so the light beyond the old margin falls unshadowed. Both
/// are edges of the frame rather than of the world, which is what makes them a
/// bug and not a look.
///
/// Public beyond [`lit_tiles`] because a caller with no [`Camera`] at all still
/// needs it: a scene tool that windows a database query by a stated radius
/// (`examples/shard/mod.rs`) has to widen that window by the same margin, or a
/// lamp just outside the geometry it pulls is a lamp whose pool the frame draws
/// with nothing making it.
pub fn light_margin_tiles(tuning: &Tuning) -> i32 {
    (CAMPFIRE.radius * tuning.reach).ceil() as i32 + 1
}

/// The cells a frame's flames can come from: what is drawn, grown by the reach
/// of the widest pool. See [`light_margin_tiles`].
///
/// Public because it is the rectangle *the grid is*, and a second caller that
/// wants the same grid must not guess at it: the app's occluder overlay
/// (`docs/archive/render/lighting.md`, step 14) rebuilds the grid to draw it, and a wireframe
/// over a rectangle the shader did not walk is an instrument that lies about
/// exactly the edge it exists to show. Which is also why the tuning is an
/// argument here rather than read from somewhere: the overlay and the frame have
/// to be handed the same one.
pub fn lit_tiles(camera: &Camera, tuning: &Tuning) -> crate::camera::TileBounds {
    let bounds = camera.visible_tiles();
    let margin = light_margin_tiles(tuning);
    crate::camera::TileBounds {
        min_x: bounds.min_x - margin,
        max_x: bounds.max_x + margin,
        min_y: bounds.min_y - margin,
        max_y: bounds.max_y + margin,
    }
}

/// Every flame a frame can see, flickering, with what stands in their way.
///
/// The statics come from the map and the items from what the server has
/// dropped, which is the same pair [`crate::statics`] and [`crate::items`] draw
/// — and they are tested against the same `cutaway`, so a brazier on the storey
/// above the player stops lighting the floor at the instant it stops being
/// drawn. A light that outlived its sprite is a glow with nothing making it.
///
/// The occluders come from the same walk of the same cells, for the same reason
/// in the other direction: a wall the frame did not draw must not darken the
/// street — see [`crate::occlusion`].
///
/// `time` is how long the client has been running, in seconds; only the flicker
/// reads it. It is an argument because this crate does not own a clock, and the
/// caller passes the same sampled instant every other clock in the frame was
/// advanced by.
///
/// `atlas` is where an occluder's *facing* comes from, and it is an `Option`
/// because not every caller has pictures: a built scene has a map and an item
/// list and no art at all. Without it every occluder is the whole tile it was
/// before [`crate::facing`] existed, which is the safe answer and not a broken
/// one — see [`occlusion::collect`](crate::occlusion::collect).
/// `bake` is the blocks of the occlusion grid a caller has already derived, and
/// it is an `Option` for the same reason `atlas` is: not every caller keeps one
/// across frames. `None` builds the grid from nothing, which is the same grid —
/// see [`occlusion::bake`](crate::occlusion::bake), whose first test is that the
/// two are equal.
/// `tuning` is what a person has turned — see [`Tuning`], and its own note for
/// why it is read here rather than applied to the result: the reach is what this
/// walk's rectangle is grown by, so a frame collected without it and scaled
/// afterwards has already lost the flames and the walls outside the old margin.
// Ten, and every one of them is a different thing the frame knows: the world,
// what the server has put in it, where the eye is, what the client's files say,
// what the frame has cut away, what the sky is doing, what the person looking at
// it has turned, when, the pictures, and what was built for the last frame.
// Grouping them into a struct would be one more type to keep in step with the
// call sites for no fewer facts.
#[allow(clippy::too_many_arguments)]
pub fn collect(
    map: &WorldMap,
    items: &[GroundItem],
    camera: &Camera,
    tiledata: &TileData,
    cutaway: &Cutaway,
    ambient: Ambient,
    tuning: &Tuning,
    time: f32,
    atlas: Option<crate::atlas::StaticArt<'_>>,
    bake: Option<&mut crate::occlusion::bake::Bake>,
) -> Lighting {
    collect_with_interior(
        map, items, camera, tiledata, cutaway, ambient, tuning, time, atlas, bake, None,
    )
}

/// [`collect`] with the same room visibility as the geometry passes.
///
/// The cached occlusion bake is intentionally bypassed while this is active:
/// its blocks are complete-map facts and cannot represent a door or floor
/// choice that changes every frame.
#[allow(clippy::too_many_arguments)]
pub fn collect_with_interior(
    map: &WorldMap,
    items: &[GroundItem],
    camera: &Camera,
    tiledata: &TileData,
    cutaway: &Cutaway,
    ambient: Ambient,
    tuning: &Tuning,
    time: f32,
    atlas: Option<crate::atlas::StaticArt<'_>>,
    bake: Option<&mut crate::occlusion::bake::Bake>,
    interior: Option<&crate::interiors::InteriorFrame>,
) -> Lighting {
    let bounds = lit_tiles(camera, tuning);
    let mut lights = Vec::new();

    crate::statics::for_each_static_in(map, bounds, |item| {
        let tile = tiledata.static_tile(item.tile.0);
        if !burns(item.tile, tile)
            || !cutaway::shows(cutaway, item.z, tile)
            || !interior.is_none_or(|frame| frame.shows_static_at(Point::new(item.x, item.y, item.z), tile))
        {
            return;
        }
        lights.push(tuning.applied(place(Point::new(item.x, item.y, item.z), flame(item.tile), time)));
    });

    for item in items {
        let tile = tiledata.static_tile(item.graphic.0);
        if !burns(item.graphic, tile)
            || !cutaway::shows(cutaway, item.at.z, tile)
            || !interior.is_none_or(|frame| frame.shows_at(item.at))
        {
            continue;
        }
        lights.push(tuning.applied(place(item.at, flame(item.graphic), time)));
    }

    // The grid before the flames are placed, because where a mounted flame burns
    // is a fact about what it is mounted *on* — see `mounted_at`.
    let occlusion = match interior {
        Some(_) => {
            crate::occlusion::collect_with_interior(map, items, bounds, tiledata, cutaway, atlas, interior)
        }
        None => {
            match bake {
                Some(bake) => {
                    crate::occlusion::bake::collect(bake, map, items, bounds, tiledata, cutaway, atlas)
                }
                None => crate::occlusion::collect(map, items, bounds, tiledata, cutaway, atlas),
            }
        }
    };
    for light in &mut lights {
        light.at = mounted_at(light.at, &occlusion);
    }

    if lights.len() > Lighting::MAX {
        // Nearest the player first — which is the eye's tile, and at every zoom
        // the middle of what is drawn. A total order and not a partial one: two
        // lights at the same distance keep the order the map gave them, so one
        // frame is not a different sixty-four from the next for a camera that
        // has not moved.
        let (eye_x, eye_y) = camera.eye_tile();
        let eye = Vec2::new(eye_x as f32, eye_y as f32);
        lights.sort_by(|a, b| {
            let key = |light: &Light| {
                let (dx, dy) = (light.at.x - eye.x, light.at.y - eye.y);
                dx * dx + dy * dy
            };
            key(a).total_cmp(&key(b))
        });
        lights.truncate(Lighting::MAX);
    }

    Lighting {
        ambient: tuning.ambient(ambient),
        lights,
        occlusion,
        // No sky here. What the sun is doing is not a property of the tiles this
        // walked — it is one direction for the whole world, and the caller that
        // knows the time of day sets it on the way to the blit.
        sun: None,
        // The ordinary picture. A caller wanting a diagnostic sets the field on
        // the way to the blit: which view is on is a property of the person
        // looking, not of the world walked here.
        view: crate::debug::View::Lit,
        // The two knobs the frame itself carries, rather than ones already spent
        // on the lights above: both are read per fragment, by this walk and by
        // the shader, and neither can be applied to a `Light`.
        flame_radius: tuning.flame_radius,
        shadow_rays: tuning.shadow_rays,
        // Not the geometry's to know either, for `view`'s own reason: a
        // caller wanting the grey screen sets it on the way to the blit —
        // see `Lighting::dead`.
        dead: false,
    }
}

/// How far outside the plane a mounted flame is placed, in tiles.
///
/// Half a tile takes it from its tile's centre to the plane the panel stands on,
/// and a fifth more takes it off that plane — where the cosine against the face
/// it is bolted to is zero along the *whole* face, so the wall it hangs on would
/// come out black from top to bottom however bright the flame is.
///
/// The fifth used to be spelt `FACE_EDGE`, and the number is kept at what that
/// made it on purpose: phase 3 moved the picture through the shading term and
/// through nothing else.
///
/// **`docs/render/design_model.md` phase 4 was to have deleted this and does not**,
/// and the reason is the paragraph above rather than a reluctance: what the plan's
/// "a sconce burns where it is" would mean in practice is a flame at its tile's
/// *centre*, which is behind the plane of the face it is bolted to, where the
/// cosine is zero along the whole face — so every wall carrying a sconce would
/// come out black from top to bottom however bright the flame. This is not a
/// compensation for a missing rule; it is the client's reading of where a
/// wall-mounted static actually hangs, and the map says only which tile. What
/// phase 4 did delete is the *height test* that stood beside it — see the note
/// where `exemption` lived.
///
/// Neutralising [`mounted_at`] turns `a_sconce_lights_the_street_and_not_the_room_
/// behind_it` and `light_runs_along_a_wall_and_stops_across_it` red, which is how
/// that was settled rather than argued. What would retire it honestly is the
/// *art*: a sconce's sprite shows it standing out from the wall, and nothing
/// measures that.
///
/// The consequence worth stating: it lands on the *next* tile, so the wall it is
/// mounted on stops being the flame's own cell and starts being an ordinary
/// occluder. That is what makes a sconce light the street and not the room behind
/// it, and it is the whole reason this is a move rather than an exemption.
const MOUNTED_CLEARANCE: f32 = 0.7;

/// Where a flame standing on a wall tile actually burns: outside the plane its
/// own tile names, on the side the wall's picture is drawn from.
///
/// A sconce, a lamp bracket, a torch in a wall ring — a static whose tile carries
/// a **panel** — is bolted to the *outside* of that panel, and the map does not
/// say so: it says the tile. Left at its tile's centre it is behind the plane of
/// the face it lights, and two things follow that a person can see. Its own wall
/// comes out dark, because a face is one-sided and the flame is behind it. And
/// its own tile is exempt from shadowing it (decisions 3 and 17), so the room on
/// the other side of that wall is lit exactly as brightly as the street.
///
/// `docs/archive/render/lighting.md`'s backlog has carried the shape of this since the first
/// version of the pass — *"a lamp mounted on a wall wants pushing off it, not
/// exempting from it"* — and the grid already holds what it needs. Moving the
/// flame answers both, and it is what let the facing test lose its exemption for
/// a flame standing in a wall's line, which is a whole street long and lit every
/// wall in it.
///
/// A tile with no panel is not moved, and that covers the ordinary cases by
/// construction: a torch on the ground, a lamp post in the street, a brazier in a
/// room. So is a cell whose sides cancel — [`Edges::ANY`](crate::occlusion::Edges::ANY),
/// the whole-tile answer for a graphic the art would not name, and a lid — because
/// there is no direction in it to move along and a guess would be a wrong one.
fn mounted_at(at: Vec2, occlusion: &crate::occlusion::Occlusion) -> Vec2 {
    let Some(cell) = occlusion.at(at.x.floor() as i32, at.y.floor() as i32) else {
        return at;
    };
    // Componentwise and not along one normalised direction, so that a flame on a
    // **corner** — two panels, and every building has them — goes clear of both
    // planes rather than half clear of each.
    let toward = |positive: Edges, negative: Edges| {
        match (cell.edges.contains(positive), cell.edges.contains(negative)) {
            (true, false) => MOUNTED_CLEARANCE,
            (false, true) => -MOUNTED_CLEARANCE,
            // Neither side, or both: a lid, a whole-tile occluder, or a tile holding
            // two walls that face away from each other. No direction, no move.
            _ => 0.0,
        }
    };
    Vec2::new(
        at.x + toward(crate::occlusion::Edges::EAST, crate::occlusion::Edges::WEST),
        at.y + toward(crate::occlusion::Edges::SOUTH, crate::occlusion::Edges::NORTH),
    )
}

/// One flame, from its tile to where it burns: the tile itself, lifted to the
/// height of the flame rather than the ground under it.
///
/// The [`Flame`] and not the [`Graphic`] it came from, because [`carried`] has
/// no graphic at all — nothing on the wire says a hand is holding a torch — and
/// a stand-in graphic passed in only to be looked up again would be a second
/// place the mapping lives.
fn place(at: Point, flame: Flame, time: f32) -> Light {
    Light {
        // The middle of the tile, not its corner: a fragment's own position is
        // fractional now — the world passes write where in its tile a pixel is —
        // and a flame at `(x, y)` exactly would sit on the tile's north corner
        // and light the tile north of it as brightly as its own.
        at:        Vec2::new(f32::from(at.x) + 0.5, f32::from(at.y) + 0.5),
        z:         f32::from(at.z) + FLAME_LIFT,
        radius:    flame.radius,
        color:     flame.color,
        intensity: flame.intensity * flicker(time, phase_of(at), flame.flicker),
        // Every fire standing in the world burns in every direction. A beam is
        // something a hand does to a flame — see [`carried`].
        beam:      None,
    }
}

// **`MAX_WALK_STEPS` stood here, and `docs/render/design_occluders.md`'s S5 deleted it**
// without putting a number in its place — which is a departure from that plan's
// own letter, and the reason is worth the paragraph.
//
// It bounded the **cells** a ray stepped through, at 72, "so that a loop over
// data cannot be made unbounded by a radius somebody widens later". The bound
// was needed because the loop's length was the *ray's*: a longer reach is more
// cells, and nothing about the grid said when to stop.
//
// S5 asks for a node budget in the same role. There is nothing to size. A
// traversal moves to `at + 1` on a hit and to that node's own escape on a miss,
// and **both are strictly greater than `at`** — the tree is laid out depth
// first, so an escape is the end of a subtree that starts at `at`. So the loop
// visits each node at most once and is bounded by the number of nodes the frame
// *has*, which no radius can widen: a reach twice as long walks the same tree.
// The one thing that could break it is a malformed tree — an escape pointing
// backwards, out of a buffer nothing on this side wrote — and [`candidates`]
// stops on exactly that rather than looping, which is a constant-free guard
// where a budget would have been a number to defend.
//
// Measured, so the shape of the loop is not just argued: over the whole suite
// the deepest traversal visits **33 nodes of a 49-node tree**. A budget sized
// off that would have been a number about the fixtures rather than about the
// data.

// **`FLAME_SPREAD`, `SOFT_CROSSING_MIN`, `SOFT_CROSSING_MAX` and `FLAME_DEPTH`
// lived here**, and `docs/render/design_model.md` phase 5 is what deleted all four.
// **A ray is a ray, and a penumbra is what N of them disagreeing about make.**
//
// They were one apparatus: `FLAME_SPREAD` said a flame is a body a tile across,
// the two bounds kept the `t / (1 - t)` ratio finite at both ends of a ray, and
// `FLAME_DEPTH` converted the width that produced into a height, because every
// edge the walk softened vertically is horizontal. Every one of them was a
// number about a *picture of* a penumbra rather than about a flame: the ratio is
// the textbook penumbra formula with the source's own size in it, and the size
// was `1.0` because that is what drew an edge a person liked — the same tile it
// would have been if a flame were a tile across, which it is not.
//
// What replaces the four is [`FLAME_RADIUS`] and eight rays: the flame has a
// size in the one place a size belongs, the walk answers "yes" or "no" the way a
// ray does, and the gradient at a shadow's edge is the share of the flame a
// fragment can still see. `pierces`'s band, `crosses`'s band, `inside`'s band and
// the `spread` parameter every walk threaded went with them.

/// How big a flame is: the radius of the sphere [`arrival`] samples, in tiles.
///
/// **An eighth of a tile, and the art is what says so.** The projection draws
/// four screen pixels to one `z` ([`crate::camera::Z_STEP`]), and the flame a
/// torch graphic actually has drawn on it is eight or ten pixels tall — two and a
/// half `z`, a fifth of a tile, so a radius of an eighth. That measurement is not
/// new: it is the one `FLAME_DEPTH` was taken from, which was `Z_PER_TILE / 4`
/// and is exactly twice this. A height became a radius, and a flame that used to
/// be a pancake — a tile across and a quarter of a tile tall — is a ball.
///
/// **What went with the pancake is `FLAME_SPREAD`'s `1.0`, which was never a
/// size.** It was the numerator of the penumbra's `t / (1 - t)` — the width a
/// person liked at the far end of a ray — and the only reason it was stated in
/// tiles is that the ratio it multiplied is dimensionless. This renderer casts
/// rays at the flame now, so the size is a size: eight times narrower than the
/// number that stood in for one, and every shadow in the frame is correspondingly
/// crisper. `docs/render/design_model.md` phase 5 has the pictures.
///
/// A sphere and not the ellipsoid the two old constants imply, because the
/// reference tracer's `Emitter::Sphere` is a sphere and a penumbra judged against
/// it has to be cast by the same body. In *tile* space, which is where the sphere
/// is round: `z` is divided into tiles before the disc is laid out and multiplied
/// back after, the same metric [`Z_PER_TILE`] gives the falloff.
///
/// `blit.wgsl`'s `FLAME_RADIUS`, and the two are one number.
pub const FLAME_RADIUS: f32 = 0.125;

/// How many rays a fragment casts at each flame.
///
/// Eight, which is the plan's own default and is where a penumbra stops looking
/// like a staircase at the zooms this client draws. It buys a gradient of nine
/// levels across a shadow's edge; a per-fragment rotation of the sample pattern
/// ([`dither`]) is what turns the eight into a continuum rather than eight bands.
///
/// There is no temporal accumulation behind it and deliberately none yet — the
/// moment eight is too noisy or too slow is the moment to add one, and not
/// before. The default of [`ShadowRays`], which is what a frame actually reads:
/// `blit.wgsl` takes the count off the header now rather than off a `const`, so
/// this number is the picture this client draws and no longer the only picture
/// it can draw.
pub const SHADOW_RAYS: usize = 8;

/// How many rays *this frame* casts at each flame — [`SHADOW_RAYS`] unless a
/// person has turned the knob.
///
/// A type and not a bare `u32` because both ends of the range are a real
/// failure: zero rays is a division by zero and a black frame, and a count past
/// [`ShadowRays::MOST`] overruns the array [`flame_points`] fills. Both are
/// clamped in [`ShadowRays::new`], which is the only way to build one, so the
/// walk and the shader can index without asking.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ShadowRays(u32);

impl ShadowRays {
    /// The one unique ray a point source can cast.
    const ONE: Self = Self(1);

    /// What the client draws with unless told otherwise: [`SHADOW_RAYS`].
    pub const DEFAULT: Self = Self(SHADOW_RAYS as u32);

    /// The most a frame may ask for.
    ///
    /// The bound is [`flame_points`]'s array and nothing else — the shader's own
    /// loop has no ceiling at all and would happily walk a thousand. Thirty-two
    /// is four times the default, which is enough for the one thing more rays
    /// are *for*: a person looking at the grain phase 5b put into the brightness
    /// of a fragment standing right beside a flame, and deciding whether the
    /// answer is more rays or a different arrangement. Raising it is one number
    /// here and nothing else.
    pub const MOST: u32 = 32;

    /// A count, clamped into `1..=MOST`. Takes anything, including what a
    /// hand-edited file offers, because that is where hostile numbers come from.
    pub fn new(rays: u32) -> Self {
        Self(rays.clamp(1, Self::MOST))
    }

    /// How many, for a loop.
    pub fn count(self) -> usize {
        self.0 as usize
    }

    /// And for the header the shader reads.
    pub fn raw(self) -> u32 {
        self.0
    }

    /// The samples that can produce distinct answers for a body of `radius`.
    ///
    /// At zero radius every Vogel point is the flame centre, so walking more
    /// than one repeats the identical segment and divides the identical sum by
    /// the repetition count. A non-zero body keeps the person's full setting.
    ///
    /// **Performance regression guard.** This was found with the live client at
    /// close zoom and `flame_radius = 0`, `shadow_rays = 32`, `reach = 4`: one
    /// carried light spent 27–30 ms in the lighting blit. Collapsing the 32
    /// identical BVH walks to this one brought the same pass below 1 ms. Keep
    /// Keep the `a_point_source_walks_the_bvh_once` test beside this rule if the
    /// sampling arrangement changes.
    fn for_radius(self, radius: f32) -> Self {
        match radius <= 0.0 {
            true => Self::ONE,
            false => self,
        }
    }
}

impl Default for ShadowRays {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Where the sun stands and how hard it burns, as the numbers a person turns
/// rather than as the direction the walk wants.
///
/// [`Sun`] carries a *normalised* direction, which is the right thing for a walk
/// and the wrong thing for a slider: two of its three components move together,
/// and a person dragging one of them sideways has changed the elevation as well.
/// This is the pair that does not — an angle around the compass and a slope —
/// and [`SunTuning::sun`] is the one place they become a direction.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SunTuning {
    /// Which way it is, in degrees, measured from `+x` towards `+y`. The map's
    /// own axes and not the screen's: the projection turns them 45°, so a sun at
    /// zero comes from the lower right of the picture.
    pub azimuth_degrees: f32,
    /// How steeply it climbs, in tiles up per tile along — [`Sun::rise_per_tile`],
    /// which is the number this exists to be able to state directly. `1.0` is
    /// 45°.
    pub rise_per_tile:   f32,
    /// Its colour, linear.
    pub color:           [f32; 3],
    /// How much it adds where it reaches. Zero is "no sun" and the shader never
    /// walks a ray for it, which is what `App::sunlit` writes when the sun is
    /// switched off.
    pub intensity:       f32,
}

impl SunTuning {
    /// The sun this client stands under until there is a time of day on the wire:
    /// [`midday`], stated as the two numbers a person turns.
    ///
    /// `the_default_sun_tuning_is_midday` is what holds the pair together — the
    /// constants are here in one spelling and there in another, and nothing but
    /// that test says they are the same sun.
    pub const MIDDAY: Self = Self {
        azimuth_degrees: 0.0,
        rise_per_tile:   1.0,
        color:           [1.0, 0.933_107, 0.748_414],
        intensity:       0.263_273,
    };

    /// The direction the walk wants, built from the two numbers a person turns.
    pub fn sun(self) -> Sun {
        let radians = self.azimuth_degrees.to_radians();
        Sun::towards(
            radians.cos(),
            radians.sin(),
            self.rise_per_tile,
            self.color,
            self.intensity,
        )
    }
}

impl Default for SunTuning {
    fn default() -> Self {
        Self::MIDDAY
    }
}

/// Every number about the light a person may turn while the client is running.
///
/// **A knob is not a second opinion about a constant.** `TORCH`, [`NIGHT`],
/// [`FLAME_RADIUS`] and the rest are what this world's light *is*, measured or
/// authored and argued for where they stand; this is what the person looking at
/// the frame does to them. So all but two fields are plain factors against
/// exactly one of those numbers, `1.0` is the untouched frame everywhere, and
/// [`Tuning::DEFAULT`] is the picture this client drew before there was a menu —
/// which is what makes "put it back" a thing a person can read off the page.
///
/// The two that are not factors say so, because there is nothing sensible to
/// multiply: a flame is not one and a half [`FLAME_RADIUS`]es, it is a size in
/// tiles or it is nothing, and a ray count is a count.
///
/// **Where it is read is where it has to be applied.** Two of these are not
/// cosmetic scalings of the frame's output: [`Tuning::reach`] widens every pool,
/// and the rectangle the occlusion grid is built over is grown by the widest pool
/// ([`lit_tiles`]) — so a reach turned up after the grid was built lights tiles
/// out of a grid that holds no walls for them, and the shadows simply stop at an
/// invisible line. [`Tuning::shadow_rays`] is the same shape of thing in the
/// other direction: it is read by the shader and by [`sample`], and the two
/// answering with different counts is the parity oracle reporting noise as a
/// defect. Hence one struct, threaded through [`collect`], rather than a handful
/// of fields set on the way past.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Tuning {
    /// How big a flame's body is, in tiles: the softness of every shadow it
    /// casts, and the one knob a person means by "hardness". [`FLAME_RADIUS`] is
    /// the world's answer; `0.0` is a point source and a razor edge, and larger
    /// than a tile is a bonfire the size of a room.
    pub flame_radius:    f32,
    /// How many rays each fragment casts at each flame — [`ShadowRays`].
    pub shadow_rays:     ShadowRays,
    /// What every flame's own [`Flame::intensity`] is multiplied by: how hard the
    /// fire burns.
    pub brightness:      f32,
    /// And what its [`Flame::radius`] is: how far the pool reaches, in tiles.
    ///
    /// Read *before* the frame's lights are collected, because [`lit_tiles`] is
    /// grown by it — see the struct's own note.
    pub reach:           f32,
    /// What [`Ambient::sky`] is multiplied by: how bright the open sky is over a
    /// tile that can see it.
    pub sky:             f32,
    /// And [`Ambient::ground`]: the floor under the darkness, which is what a
    /// windowless cellar gets. Turning this to nothing is what makes an unlit
    /// room pure black.
    pub ground:          f32,
    /// Where the sun stands and how hard it burns — [`SunTuning`]. Read only by a
    /// frame that has a sun at all; night never asks. Its own colour —
    /// [`SunTuning::color`] — is a literal and not a factor, for the reason
    /// stated there.
    pub sun:             SunTuning,
    /// A tint the player's own light — [`carried`] — is multiplied through,
    /// channel by channel, by [`Tuning::applied_headlight`]. `[1.0, 1.0, 1.0]`
    /// leaves it whatever colour [`carried`] built it as.
    ///
    /// Kept apart from [`Tuning::lantern_color`] because they answer different
    /// questions: a person turning the street's lanterns blue is not asking to
    /// repaint the torch in their own hand, and [`collect`] never sees this one
    /// at all — see [`Tuning::applied_headlight`]'s own note.
    pub headlight_color: [f32; 3],
    /// A tint every flame [`collect`] finds burning on the map is multiplied
    /// through, channel by channel — every [`TORCH`] and [`CAMPFIRE`], and
    /// never the light in the player's hand. `[1.0, 1.0, 1.0]` is the untouched
    /// frame: [`TORCH`] stays the warm orange it is authored as and
    /// [`CAMPFIRE`] stays its own colour.
    ///
    /// The stand-in this pulls toward is one colour for every lantern in the
    /// world, because `light.mul`'s own per-graphic colour is not on the wire
    /// yet — see this module's header. It is deliberately a factor and not a
    /// literal, unlike [`SunTuning::color`]: a literal here could not leave
    /// [`TORCH`] and [`CAMPFIRE`] their own colours by default, since they do
    /// not share one.
    pub lantern_color:   [f32; 3],
    /// A tint [`Ambient::sky`] and [`Ambient::ground`] are each multiplied
    /// through, on top of [`Tuning::sky`] and [`Tuning::ground`]'s own
    /// brightness — see [`Tuning::ambient`]. `[1.0, 1.0, 1.0]` leaves [`NIGHT`]
    /// its blue and [`SKYLIGHT`] the colour it was authored as.
    pub ambient_color:   [f32; 3],
}

impl Tuning {
    /// The frame this client draws with nothing turned: every factor `1.0`, the
    /// flame the size the art says, eight rays, and [`midday`] overhead.
    pub const DEFAULT: Self = Self {
        flame_radius:    FLAME_RADIUS,
        shadow_rays:     ShadowRays::DEFAULT,
        brightness:      1.0,
        reach:           1.0,
        sky:             1.0,
        ground:          1.0,
        sun:             SunTuning::MIDDAY,
        headlight_color: [1.0, 1.0, 1.0],
        lantern_color:   [1.0, 1.0, 1.0],
        ambient_color:   [1.0, 1.0, 1.0],
    };

    /// The largest any factor may be set to, and the largest a flame may be.
    ///
    /// Not a matter of taste: a reach of eight widens [`lit_tiles`] to eighty
    /// tiles a side, which is the grid, the bake and every walk over it — the
    /// frame stops being drawn at interactive rates well before the number stops
    /// meaning anything. Four is generous and still affordable, and a person who
    /// wants more is asking for a different plan rather than a wider slider.
    pub const MOST: f32 = 4.0;

    /// The same numbers with every one of them inside the domain its field
    /// documents: factors in `0..=`[`Tuning::MOST`], the flame no larger than
    /// that many tiles, and no `NaN` anywhere.
    ///
    /// The door this type has, and the reason it has one is that its fields
    /// arrive from a *file* — `client_ui.ron`, hand-editable on purpose. A
    /// negative brightness is a flame that darkens what it reaches and a `NaN`
    /// radius is a frame with no lit pixels at all; both are silent, and both are
    /// one typo away. Clamping is stated here, in the crate that owns what these
    /// numbers mean, rather than in the deserializer that happens to be first to
    /// see them.
    ///
    /// `f32::clamp` panics on a `NaN`, so a `NaN` becomes the default of its own
    /// field rather than propagating — the same rule `desk::Zoom` follows, and
    /// for the same reason: a light nobody can see is not worth a crash on
    /// startup.
    pub fn clamped(self) -> Self {
        let factor = |value: f32, default: f32| {
            match value.is_nan() {
                true => default,
                false => value.clamp(0.0, Self::MOST),
            }
        };
        Self {
            flame_radius:    factor(self.flame_radius, FLAME_RADIUS),
            shadow_rays:     self.shadow_rays,
            brightness:      factor(self.brightness, 1.0),
            reach:           factor(self.reach, 1.0),
            sky:             factor(self.sky, 1.0),
            ground:          factor(self.ground, 1.0),
            sun:             SunTuning {
                azimuth_degrees: match self.sun.azimuth_degrees.is_nan() {
                    true => SunTuning::MIDDAY.azimuth_degrees,
                    false => self.sun.azimuth_degrees % 360.0,
                },
                rise_per_tile:   factor(self.sun.rise_per_tile, SunTuning::MIDDAY.rise_per_tile),
                color:           std::array::from_fn(|channel| {
                    factor(self.sun.color[channel], SunTuning::MIDDAY.color[channel])
                }),
                intensity:       factor(self.sun.intensity, SunTuning::MIDDAY.intensity),
            },
            headlight_color: std::array::from_fn(|channel| factor(self.headlight_color[channel], 1.0)),
            lantern_color:   std::array::from_fn(|channel| factor(self.lantern_color[channel], 1.0)),
            ambient_color:   std::array::from_fn(|channel| factor(self.ambient_color[channel], 1.0)),
        }
    }

    /// This frame's brightness and reach, with `tint` multiplied into the
    /// flame's own colour channel by channel — the common half of
    /// [`Tuning::applied`] and [`Tuning::applied_headlight`].
    fn tuned(self, light: Light, tint: [f32; 3]) -> Light {
        Light {
            radius: light.radius * self.reach,
            intensity: light.intensity * self.brightness,
            color: std::array::from_fn(|channel| light.color[channel] * tint[channel]),
            ..light
        }
    }

    /// One flame with this frame's brightness, reach and [`Tuning::lantern_color`]
    /// in it.
    ///
    /// Every [`Light`] [`collect`] finds burning on the map goes through here
    /// — not the one in the player's hand, which [`Lighting::hold`] takes
    /// straight from [`Tuning::applied_headlight`] instead, since it was never
    /// collected.
    pub fn applied(self, light: Light) -> Light {
        self.tuned(light, self.lantern_color)
    }

    /// The same brightness and reach, tinted by [`Tuning::headlight_color`]
    /// rather than [`Tuning::lantern_color`] — the player's own light, which
    /// [`collect`] never sees and a person turning the street's lanterns has
    /// not asked to repaint.
    pub fn applied_headlight(self, light: Light) -> Light {
        self.tuned(light, self.headlight_color)
    }

    /// The ambient with this frame's sky and floor brightness and colour in it.
    pub fn ambient(self, ambient: Ambient) -> Ambient {
        Ambient {
            sky:    std::array::from_fn(|channel| {
                ambient.sky[channel] * self.sky * self.ambient_color[channel]
            }),
            ground: std::array::from_fn(|channel| {
                ambient.ground[channel] * self.ground * self.ambient_color[channel]
            }),
        }
    }
}

impl Default for Tuning {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Below this, a ray has been stopped: `blit.wgsl`'s early exit, and under a
/// byte's worth of light either way.
const RAY_CUTOFF: f32 = 0.004;

// **`crosses` stood here, and `docs/render/design_frame_assembly.md`'s P4 step 1 deleted it** — with
// `blit.wesl`'s copy, which is the only way a formula written in two languages
// goes away at all.
//
// It answered whether a **lid** was in the way of a ray, as a crossing of a plane
// rather than a passage through a box: `1.0` where the ray went through and out
// the other side, `0.0` where it stayed on one side. It had to exist because a
// lid *was* a plane — a floor is `height 0` in `tiledata.mul`, 4,534 of the 4,647
// lids over the block of Britain `artscan`'s `column` example reads — so the
// length rule beside it got zero out of every floor in the world, which is what
// lit the storey above a torch through its own floorboards
// (`scene::storey_over_a_torch`). Its strictness was the second half: a ray
// running exactly along the top of a lid — a candle standing on the floor it
// lights, both at one `z` — has not gone through anything, and counting that
// touch would have laid half a floor's shadow across every room lit from inside
// it.
//
// `occlusion::Solid::box_of` gives a lid a span of its own now, so
// `ray_vs_solid` is both halves: a ray from the storey above to the storey below
// genuinely crosses a volume, and a ray in the top face's own plane is excused by
// `on_the_lit_surface` on the same terms a run of wall is — the neighbouring
// floor's extent along the fragment's normal ends exactly on the fragment's own
// plane. `a_floor_stops_a_ray_through_it_and_not_one_along_it` is the same three
// cases asked of the geometry instead.

// **`STAND_OFF`, `ON_TOP` and `stand_clear` lived here**, and
// `docs/render/design_model.md` phase 4 is what deleted them. **The bias is zero.**
//
// They were `2.0 / 127.0` of a tile in front of a face's own plane and
// `1.0 / 128.0` of a `z` above whatever a point lay on, and both numbers came
// off the `place` attachment's byte layout rather than off any statement about
// surfaces: two steps of a seven-bit fraction, and well under the whole `z` unit
// that attachment quantised a height to. What they bought was two things, and
// three earlier phases took the reason for each away:
//
//   - **A ray did not start inside the surface it was drawn on.** That was
//     identity's job all along and it does it exactly now: a fragment names its
//     own solid and the walk skips that one solid, whatever the geometry does at
//     the origin. `exemption`, and the phase's own commit.
//   - **A face pixel was walked from in front of its plane**, because the
//     attachment placed it a hundred-and-twenty-seventh *behind* that plane and a
//     ray from there crossed a neighbouring floor before the walk reached the
//     cell that floor stands in. Phase 2 replaced the packing with the exact
//     position, and `docs/archive/render/lighting_raymarch.md`'s per-solid `ray_vs_solid` gave
//     every solid its own exact interval and footprint — so a crossing is found
//     on the cell the solid is referenced from, whenever along the ray it
//     happens.
//
// And what they cost, measured with the light oracle: up to `0.51` of a channel
// brighter than the geometry allows on the top band of a riser, because a ray
// lifted a hundred-and-twenty-eighth clear of its own surface is a ray that
// escapes the occluders standing at that surface's own height.
//
// `crosses` needed no nudge either, and never did: its crossing test was strict,
// so a ray leaving a lid's plane exactly — `under >= high` — read zero rather
// than half. That strictness was the same sentence [`ON_TOP`] was, spelled in the
// place the geometry is decided instead of in the ray's start — and it is the
// geometry outright now that a lid is a box, which is `crosses`'s own grave note
// below.

// **`on_surface` lived here** and went with its only reader, `same_run` —
// `docs/render/design_occluders.md`'s S4. It asked whether a fragment's `z` lay inside a
// primitive's own span, inclusively and exactly, and what that answered was
// "is the lit end at a height this panel occupies at all", the height half of
// the run mask. Nothing else ever called it: the question a walk asks now is
// whether a candidate's extent *ends on the fragment's own plane*, which is
// [`on_the_lit_surface`], and a span read off the wire rather than rounded is
// held by [`wire_span`] and its own test. `blit.wesl`'s copy went with it.

// **`drawn_on` lived here**, and `docs/render/design_model.md` phase 4 is what
// retired it. It asked whether a lid was a plane the fragment was *drawn at* the
// height of — `low == high && drawn == low` — and [`exemption`] used it to tell
// which of one static's several lids a fragment belonged to, because an
// [`crate::occlusion::OwnerId`] names the static and a flight of steps is one
// static with a lid per tread.
//
// It was exact and it was still a proxy: two lids of one static at one height
// would have answered identically, and the reason none exists is a property of
// how a prism is cut rather than anything the rule knew. What replaced it is the
// primitive's own name — [`crate::occlusion::SolidId`], carried by the fragment
// and compared once — and with it went the `drawn_z` the [`ExemptionContext`]
// carried beside `spot_z` for the sole purpose of asking this question where the
// fragment is rather than where its ray starts.

// **`inside` lived here**, and `docs/render/design_model.md` phase 5 deleted it with
// the rest of the analytic penumbra. It was `pierces` with its one asymmetry
// taken out — a soft interval, a band wide at each edge — and its one caller was
// [`hole`], because a window's edges are in the middle of a surface and no ray
// runs along them by construction, so they softened in both directions where a
// wall's own top softened in one. Both bands are eight rays now, and a plain
// interval is what is left of the question.

/// Which world coordinate runs **along** a panel: a point of it, on the axis its
/// own hole is measured in.
///
/// A panel on a north or south side lies in a plane of constant `y`, so what
/// runs along it is `x`; an east or west one is the other way round. That is the
/// whole of the surface's own coordinate system, and it is why an
/// [`Aperture`](crate::occlusion::Aperture) belongs only to a *named* panel: a
/// lid and a body have no run for this to be measured along.
///
/// **It was `run_v` and it took `along - along.floor()`**, a fraction of the
/// tile the crossing landed in, because the hole's own ends were fractions of a
/// tile — `docs/render/design_occluders.md`'s S6 is where that went. The `floor` was the last
/// one in this pass and it decided the answer twice over: a crossing exactly on
/// a boundary floors into the next tile, and a panel wider than one tile has no
/// single tile for the fraction to be of. The aperture is stated in world
/// coordinates now, so both sides of the comparison are the same kind of number
/// and there is nothing to recover.
///
/// `blit.wesl`'s `along_the_run`.
fn along_the_run(edges: Edges, px: f32, py: f32) -> f32 {
    match edges.contains(Edges::NORTH.union(Edges::SOUTH)) {
        true => px,
        false => py,
    }
}

/// Whether a surface is **missing** where a ray goes through it: `1.0` inside the
/// hole, `0.0` outside it.
///
/// Both spans have to hold, which is what an opening in a wall is: a rectangle,
/// and a ray is through it or through the wall. They were combined with `min` of
/// two soft intervals until phase 5 — deliberately `min` and not a product, so
/// that a point halfway into the hole across *and* halfway up read as one
/// corner's own softening rather than as a quarter of a hole. With no band left
/// on either span the `min` is a conjunction and says so.
///
/// `along` and the hole's own two ends are world coordinates on the same axis
/// since S6, so this is four comparisons of numbers of one kind — where it used
/// to compare a fraction of a tile against a byte over
/// [`RUN_STEPS`](crate::occlusion::RUN_STEPS).
///
/// `blit.wesl`'s `hole`.
fn hole(aperture: Option<crate::occlusion::Aperture>, along: f32, z: f32) -> f32 {
    let Some(hole) = aperture else {
        return 0.0;
    };
    let across = along >= hole.near && along <= hole.far;
    match across && z >= hole.bottom as f32 && z <= hole.top as f32 {
        true => 1.0,
        false => 0.0,
    }
}

/// How much of a surface stands in the way at the point a ray goes through its
/// plane: all of it, less the hole in it.
///
/// The whole of step 21.3 in one line, and the reason it is one line is decision
/// 30.7: a panel was already *pierced at a point* rather than travelled through,
/// so the point was already being computed and a window is what that point is
/// asked about. `cross` is where the ray crosses, in all three — one point and
/// not three loose coordinates, which is what `blit.wgsl`'s own `vec3<f32>`
/// parameter has always been and what both callers already had in hand.
///
/// **The span itself is not asked about any more** — phase 5. The `pierces` call
/// that stood here softened the panel's own top and bottom edges over a band, and
/// its hard half was already answered before it: a caller reaches this only with a
/// `ray_vs_solid` hit against the box `low`/`high` are the `z` bounds of, and
/// `cross` is that crossing's own midpoint, so `cross[2]` is inside the span by
/// construction. What is left is the aperture, which is the only thing on a panel
/// that a ray can miss without missing the panel.
///
/// `blit.wgsl`'s `pierced`.
fn pierced(stands: &crate::occlusion::Solid, cross: [f32; 3]) -> f32 {
    match stands.aperture {
        None => 1.0,
        Some(_) => {
            1.0 - hole(
                stands.aperture,
                along_the_run(stands.edges, cross[0], cross[1]),
                cross[2],
            )
        }
    }
}

/// Where a straight segment enters and leaves an axis-aligned box, as a
/// fraction of the whole segment (`0.0` is `from`, `1.0` is `to`) — the slab
/// method, continuous throughout, with no notion of "which tile" anywhere in
/// it. `blit.wgsl` carries its own copy of the same formula, widened by a
/// small tolerance that has no reason to exist on this side — see that
/// copy's own comment for why.
///
/// `docs/archive/render/lighting_raymarch.md`'s ray-vs-Solid scoping, point 1: an exact
/// test costs a handful of compares, so nothing upstream needs to *guess*
/// whether a corner is worth asking about before asking it — this asks
/// directly and answers exactly, for a box as thin as a panel's own
/// [`crate::occlusion::PANEL_THICKNESS`] slab or as flat as a lid's bare
/// plane. [`walk_the_wire`] is what calls it now, mirrored in
/// `blit.wgsl`'s `walk`.
///
/// `None` when the segment misses the box on at least one axis outright —
/// including a segment that runs parallel to a flat axis (`solid`'s own
/// extent there is a single value, a lid's height or a panel's outer face)
/// without lying exactly in that plane, which is the box's own way of
/// saying "no length of me is on that axis at all". `Some((entered,
/// leaves))` otherwise, both already clamped to `0.0..=1.0` — the box's own
/// extent past either end of the segment does not count.
///
/// A tangent touch — the segment grazing exactly one edge or corner of the
/// box without crossing into it — comes back `Some` with `entered ==
/// leaves`: a real but zero-length crossing, deliberately not folded into
/// `None` here. Whether a caller treats a point-touch as "blocked" is a
/// decision about light and softness, not about the box's own geometry, so
/// it is left for the caller to make rather than made silently in here.
///
/// **Stays exact on purpose — widening this, even scoped to
/// [`walk_the_wire`]'s own caller, was tried and reverted.**
/// `docs/archive/render/lighting_raymarch.md`'s point 4 cutover found a real GPU/CPU
/// disagreement traced to here (see `blit.wgsl`'s own comment for the case),
/// but rescuing the same near-miss on the CPU side clamped `leaves` up to
/// `entered`, collapsing a genuine, if small, interior crossing to a
/// zero-length touch and changing what the surrounding `by_surface` branches
/// computed for it. Because [`walk_the_record`]'s `candidate_tiles` probed a
/// wider set of candidate cells than [`walk_the_wire`]'s own plain
/// single-axis stepping ever visited — deliberately, session 8's own scoping;
/// `docs/render/design_occluders.md`'s S5 has since given the two **one** broad phase, so
/// the asymmetry this paragraph turns on no longer exists — a rescued
/// near-miss on a cell only one of the two walks reached turned
/// a shared, unconditional widening into a *new* disagreement between them,
/// not a fix to one; `walk_the_wire_agrees_with_walk_the_record_on_
/// a_single_body`/`_on_a_single_panel`/`_in_a_small_room` all found real
/// counterexamples within a few hundred proptest cases. The GPU/CPU gap
/// this doc's own point 4 cutover found did not need this function widened
/// at all: CPU's own `f32` rounding already lands on the generous side of
/// the tangent for the case that mattered, so only `blit.wgsl`'s copy needed
/// the tolerance, not this one.
/// Session 20/21's `CORNER_GRAZE`/`CORNER_GAP_SOFTEN`/`ray_vs_body`/
/// `corner_graze_weight`/`axis_window`/`point_box_distance` lived here —
/// widening a body's box near a silhouette corner and tapering the result,
/// to fake a penumbra. Removed: a flame is a point source, and a point
/// source casts a hard shadow everywhere, corners included — see
/// `docs/archive/render/lighting_raymarch.md`'s "hard shadows" decision. `ray_vs_solid`'s
/// own exact hit is the whole test now, for every edge kind.
fn ray_vs_solid(from: [f32; 3], to: [f32; 3], solid: &crate::solid::Solid) -> Option<(f32, f32)> {
    let min = [solid.min.x as f32, solid.min.y as f32, solid.min.z as f32];
    let max = [solid.max.x as f32, solid.max.y as f32, solid.max.z as f32];
    let (mut entered, mut leaves) = (0.0_f32, 1.0_f32);
    for axis in 0..3 {
        let delta = to[axis] - from[axis];
        if delta.abs() <= 1e-9 {
            // Parallel to this axis: every point of the segment sits at the
            // same coordinate, so the box either spans it or the segment
            // misses it for its whole length — nothing a `t` interval can
            // narrow.
            if from[axis] < min[axis] || from[axis] > max[axis] {
                return None;
            }
            continue;
        }
        let (t1, t2) = ((min[axis] - from[axis]) / delta, (max[axis] - from[axis]) / delta);
        let (near, far) = match t1 <= t2 {
            true => (t1, t2),
            false => (t2, t1),
        };
        entered = entered.max(near);
        leaves = leaves.min(far);
        if entered > leaves {
            return None;
        }
    }
    Some((entered, leaves))
}

// **The vertical shortcut stood here, with `over_footprint` its own half, and
// `docs/render/design_occluders.md`'s S4 deleted both.**
//
// A ray with no horizontal run got a branch of its own in both walks and in
// `blit.wesl`: there is no direction to step in, so only the starting cell can
// hold anything. That much was true and is still true — [`dda_walk`] answers a
// ray with no run with exactly that one cell, which is why deleting the branch
// changes nothing about *which* cells are looked at.
//
// What the branch also did was decide **which shapes count**, and there it was a
// second, poorer answer to a question the main path already answers:
//
//   - it skipped every **panel** outright, on the argument that a panel is a
//     plane and a vertical ray lying in a wall's own plane is a graze the branch
//     had no rule for. It has had one since S3 — [`on_the_lit_surface`] is
//     called there too — and a panel is not a plane in the grid but a
//     [`crate::occlusion::PANEL_THICKNESS`]-deep slab a ray can stand inside and
//     run the whole height of. That was a live defect:
//     `a_vertical_ray_meets_what_stands_over_it_whatever_shape_it_is` measures a
//     fragment inside a wall's own thickness, lit from straight overhead through
//     twenty `z` of wall, and the branch handed it the full flame;
//   - it needed `crosses` and `over_footprint` — [`ray_vs_solid`]'s two halves
//     spelled again — because a vertical ray's height answer was once *soft*.
//     Phase 5 made `crosses` a hard crossing test, so the two halves are the
//     whole of `ray_vs_solid` and the split had nothing left to buy;
//   - and it had twice had to grow a gate to stop being a different answer from
//     the main path: once for sub-tile lids, once when a fitted climbable's
//     treads became bodies.
//
// **What licensed the deletion is a census rather than a green suite.** Measured
// 2026-08-09: the whole crate entered this branch **zero times**. A flame is a
// sphere since phase 5 and [`flame_points`] puts no sample at its centre, so a
// flame directly overhead is eight rays each leaning [`FLAME_RADIUS`] out of the
// vertical — and [`walk_sun`] answers an overhead sun before any walk starts. The
// two tests named for the vertical case had stopped sending a vertical ray and
// went on passing. Both set `flame_radius` to zero now, with the control that
// says the rays really are straight, and zero is the one configuration a person
// can still reach it from (`OPENSHARD_FLAME_RADIUS`).

// **`same_run` stood here, and `docs/render/design_occluders.md`'s S4 deleted it.**
//
// What it said was that a run of wall is one surface and no part of a surface
// shadows another part of it — true, and spelled as arithmetic over *cells*: the
// panels of the same row for a north or south face, of the same column for an
// east or west one, at a height the panel occupies. It drew that mask out of
// `own`, the lit face's own edge bit, and the panel arm of both walks skipped a
// candidate whose sides were all inside it.
//
// It is [`on_the_lit_surface`] said less exactly. That rule is a theorem about a
// box and a plane — a candidate whose extent along the fragment's own normal axis
// ends on the fragment's own plane lies wholly behind it, so there is nothing
// there to cross — and it needs no row, no column and no height gate. What kept
// `same_run` alive for three phases was not a case it covered and the theorem did
// not: it was that **three fixtures asked the walk about a fragment that was a
// point of no solid**, so the theorem, which reads the fragment's own box, could
// never fire and the cell arithmetic was the only thing left. The two spots in
// `tests/lighting.rs` and `plan::elevation`'s own rows now name their solid, and
// with them naming it the whole crate is green without this function.
//
// Its going is a *narrowing* and not only a tidy-up: the mask excused a tile's
// **north** panel for a south-facing fragment on the same row, which is a
// different plane and a real occlusion the theorem correctly keeps.

/// Which axis the plane a lit surface lies in runs across, and which end of a box
/// that plane is: `true` for a box's `max` corner, `false` for its `min`.
///
/// [`None`] for [`Surface::Upright`], and that absence is the honest one: a
/// billboard, a tree, a static whose art named no side — nothing about it says
/// which way it looks, so it has no plane to be a point of. See
/// [`on_the_lit_surface`] for what that costs and what answers for it instead.
fn lit_plane(surface: Surface) -> Option<(usize, bool)> {
    match surface {
        Surface::Upright => None,
        // A lid looks up, and the plane it looks out of is its box's `max.z` —
        // the surface the art drew, with the floor's own invented depth hanging
        // below it.
        Surface::Flat => Some((2, true)),
        Surface::Face(face) => {
            let [x, y] = face.outward();
            match x != 0.0 {
                true => Some((0, x > 0.0)),
                false => Some((1, y > 0.0)),
            }
        }
    }
}

/// **`docs/render/design_occluders.md`'s D2, and the whole of the surface exemption**: whether
/// `candidate` is part of the very surface the lit end is a point of, and so must
/// not shadow it.
///
/// The rule, and it is a theorem rather than a tolerance:
///
/// > Skip a candidate exactly when its extent along the fragment's own normal axis
/// > **ends on the fragment's own plane, from behind it** — `max` on that axis for
/// > an outward `+`, `min` for an outward `−`.
///
/// A box is axis-aligned, so each of its faces sits at its own extreme on that
/// axis, and a candidate whose extreme *is* the plane therefore lies wholly inside
/// the closed half-space behind it. A ray with `d·N > 0` leaves that half-space at
/// `t = 0+` and never returns, so **the set this discards is empty of real
/// crossings** — which is exactly the difference between this and a bias. At
/// `d·N = 0` the ray lies in the plane, which is the graze the whole exemption
/// exists for and a set of measure zero besides.
///
/// **`d·N < 0` is not exempt, and the ray's direction is a parameter because of
/// it.** The plan's own statement of D2 left the direction out, on the argument
/// that a flame behind a surface has `N·L = 0` and so cannot light it either way.
/// That is true of the shaded frame and false of the *shadow term*, which the
/// reference path tracer compares directly, with no cosine in the model on either
/// side — and it said so at once: 4,017 interior pixels of `line_scene`, every one
/// a `y = 101` face of the west box with the flame at `y = 98.5` behind it, drawn
/// lit where the tracer had them shadowed by the east box the exemption had just
/// discarded. The box really is behind the plane and the ray really does go in
/// there. So the precondition the proof states is the precondition the code tests,
/// and what was a remark about `N·L` is a comparison of two signs.
///
/// **The plane comes from the fragment's own solid, not from its position.** Both
/// are the same number — `traced.rs`'s
/// `a_face_fragments_own_plane_is_the_primitives_own_number` measured 39,930
/// fragments and found not one off its own face's plane — and taking it from the
/// box means both sides of this comparison are read out of the same list, in the
/// same precision, so the equality is exact by construction rather than by a
/// rasteriser's good behaviour. It costs nothing: the row a fragment carries
/// already names its solid.
///
/// What it replaces, and each of these is a rule that stood in for it: the walk's
/// own `lit.solid == Some(id)` wherever the surface names a side (a solid's own
/// extreme trivially equals itself), and `same_run` entirely — a run of wall is
/// N statics whose panels share one plane, which is what that function's cell
/// arithmetic and height gate were approximating. **`same_run` is gone**, and its
/// grave note above [`lit_plane`] is where its argument is kept:
/// `docs/render/design_occluders.md`'s S4 deleted it once every fixture in the tree could name
/// the solid a fragment is a point of. It does **not** replace identity
/// for [`Surface::Upright`], which has no plane at all: a tree's sprite is excused
/// from its own box by name and by nothing else.
///
/// Both boxes must be read from **one** source — `space` in [`walk_the_record`],
/// [`crate::occlusion::Solid::wire_box`] in [`walk_the_wire`] and the shader
/// — for the same reason those walks each read one box for their geometry: two
/// precisions in one comparison is two surfaces.
fn on_the_lit_surface(
    surface: Surface,
    own: &crate::solid::Solid,
    candidate: &crate::solid::Solid,
    // The ray, as `to - from`. Only its sign along the surface's own axis is read,
    // so the mixed units of a delta — tiles across, `z` units up — cannot matter.
    delta: [f32; 3],
) -> bool {
    let Some((axis, outward)) = lit_plane(surface) else {
        return false;
    };
    // The ray has to be *leaving* the plane. `>= 0` and not `> 0`: a ray lying in
    // the plane is the graze this exists for.
    let along = match outward {
        true => delta[axis],
        false => -delta[axis],
    };
    if along < 0.0 {
        return false;
    }
    let coordinate = |corner: crate::camera::WorldSpot| {
        match axis {
            0 => corner.x,
            1 => corner.y,
            _ => corner.z,
        }
    };
    match outward {
        true => coordinate(candidate.max) == coordinate(own.max),
        false => coordinate(candidate.min) == coordinate(own.min),
    }
}

/// The end of a ray that is a *surface*: which way it looks, which solid of the
/// grid it is a point of, and which tile it stands on.
///
/// Three facts that only ever travel together and that must agree: a `surface`
/// off one fragment with a `solid` off another is a combination nothing in the
/// world produces and every walk would answer for. [`Spot`] is the same three
/// beside a position; a walk takes the position separately, since what it needs is
/// the ray's two ends and this is a property of one of them.
#[derive(Clone, Copy)]
struct LitEnd {
    surface: Surface,
    solid:   Option<crate::occlusion::SolidId>,
}

impl LitEnd {
    /// The lit end a [`Spot`] is.
    ///
    /// **[`Spot::tile`] does not come along**, and `docs/render/design_occluders.md`'s S4 is
    /// why: a walk was the last thing that read it, to arbitrate against its own
    /// start point, and it seeds itself from that point now. The field survives
    /// on [`Spot`] for the one job a tile still has here — `sky_at`, which asks
    /// a question about a column of the map rather than about a ray.
    fn of(spot: Spot) -> Self {
        Self {
            surface: spot.surface,
            solid:   spot.solid,
        }
    }

    /// A point of nothing, looking nowhere in particular — what a test that is
    /// about the geometry alone means.
    #[cfg(test)]
    fn nowhere() -> Self {
        Self {
            surface: Surface::Flat,
            solid:   None,
        }
    }
}

// **`ExemptionContext`, `Exemption` and `exemption` lived here**, and
// `docs/render/design_model.md` phase 4 dissolved all three. What the function did,
// in the end, was two unrelated things at once — decide whether one solid was
// exempt from shadowing the ray, and hand back `same_run`'s mask beside it —
// and the first of those is now one comparison a walk makes inline:
//
//     if lit.solid == Some(id) { continue; }
//
// The context existed because there was a great deal to carry: the fragment's
// height before the nudge and after it, the ray's far end, the last cell, whether
// to skip it. Each of those was a question identity could not answer and each has
// gone with the thing that asked it — `drawn_on` at 4b, `stand_clear` at 4c, and
// `flame_end` here.
//
// **`flame_end` was the flame end's own height test**, and it is what phase 4's
// "`mounted_at`'s height test" names: `skip_last && cell == last &&
// on_surface(to_z, low, high)` — a panel on the cell a flame *ends* in did not
// stop the ray, so that a sconce was not shadowed by the wall it hangs on. What
// made it unnecessary is `mounted_at`, which moves such a flame clear of that
// wall's plane and onto the next tile, so the wall stops being the flame's own
// cell at all. Neutralised, the whole suite stayed green and the light oracle
// stayed at zero on every flame height — which is how it was retired rather than
// argued away. What it covered and nothing now does: a flame standing *inside* a
// whole-tile body, a lantern in a tree's box, which is a wrong box rather than a
// rule the walk owes it.
//
// `skip_last`, the walks' `last`, and `ExemptionContext`'s `to_z` went with it.

/// A point in the world, as the lighting sees one: a fractional tile and a `z`.
///
/// Fractional because that is what the place attachment carries — where in its
/// tile a pixel is, to a hundred-and-twenty-eighth — and a pool is a gradient
/// only because of it. See [`crate::place`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Spot {
    /// Tile coordinates, the fraction being where in the tile the point is.
    pub at:      Vec2,
    /// Its height, in the map's own `z` units.
    pub z:       f32,
    /// The tile this is a point of — **not** `at.x.floor()`/`at.y.floor()`.
    /// A point legitimately sits on a tile's own far edge (a stair tread's
    /// outer corner, `at.x` exactly whole) and `floor()` there picks whichever
    /// side happens to round down, not the side the geometry actually stands
    /// on. Every caller already knows which tile it means; carrying it here
    /// instead of re-deriving it in the walk is the CPU twin of
    /// `MeshFaceVertex::tile`'s fix to the same class of bug on the GPU side.
    /// `docs/archive/render/lighting_raymarch.md` step 2.
    pub tile:    (i32, i32),
    /// What surface of the world this is a point of.
    ///
    /// The polygon and not the tile: which way it looks, and therefore which
    /// flames can light it and which parts of its own tile can shadow it. It is
    /// what the place attachment's stance carries, per pixel, after
    /// `statics.wgsl` has resolved a corner to the face of the half the fragment
    /// is on. See [`Surface`].
    pub surface: Surface,
    /// **Which solid of the grid this point is a point of**, or `None` for a
    /// point of none — the ground, a mobile, a fixture with no grid behind it.
    ///
    /// `docs/render/design_model.md` phase 4, and the whole of what a self-shadow
    /// rule is now: this against the [`crate::occlusion::SolidId`] the walk is
    /// holding, and nothing else. Two things it replaced in turn. A *height*, up
    /// to phase 3 of `docs/archive/render/lighting_height.md` — "is this solid the one I am
    /// drawn from" asked as "does my `z` fall inside its span", which two things
    /// stacked on one tile answer identically and two things side by side answer
    /// wrongly for every pixel. And then an [`crate::occlusion::OwnerId`], the
    /// *static* rather than the piece of it, which cannot tell a flight's second
    /// tread from its third — so a tread was excused from the riser that
    /// genuinely stands between it and the flame, and the height came back as
    /// `drawn_on` to patch it.
    ///
    /// A whole-frame name, not a per-tile one: a solid is referenced from every
    /// cell its box touches, and a fragment of it is a point of it on all of
    /// them. See [`crate::occlusion::Occlusion::id_of`] for where a caller gets
    /// one.
    ///
    /// [`Option`] in the sense the style asks for: a mobile is a point of no
    /// occluder, which is a fact about mobiles and not a measurement nobody took.
    pub solid:   Option<crate::occlusion::SolidId>,
}

impl Spot {
    /// A point of something that stands up and names no side: a tree, a body, a
    /// wall whose art the detector would not read.
    ///
    /// The neutral answer, and what every caller that predates surfaces means:
    /// nothing is known about which way it looks, so every flame that reaches it
    /// lights it. See [`Spot::flat`] and [`Spot::face`] for the two that do know.
    ///
    /// Owned by nothing until [`Spot::owned_by`] says otherwise — the honest
    /// default for the callers that have no grid in hand, and the answer a mobile
    /// and the ground keep.
    pub fn at(at: Vec2, z: f32, tile: (i32, i32)) -> Self {
        Self {
            at,
            z,
            tile,
            surface: Surface::Upright,
            solid: None,
        }
    }

    /// A point of the ground, a floor, a rug, or the top of a wall: a surface
    /// lying in its tile, looking up.
    pub fn flat(at: Vec2, z: f32, tile: (i32, i32)) -> Self {
        Self {
            at,
            z,
            tile,
            surface: Surface::Flat,
            solid: None,
        }
    }

    /// The same point, said to be a point of one particular solid of the grid.
    ///
    /// A builder rather than a fourth argument on each of the three constructors:
    /// the surface and the solid are separate facts (a lid's top and a face of
    /// the same static are two solids and two surfaces, but a *body*'s four sides
    /// are one solid and four surfaces), and most callers here — a test about
    /// falloff, a probe over open ground — have no occluder to name and mean
    /// `None` exactly.
    pub fn part_of(self, solid: crate::occlusion::SolidId) -> Self {
        Self {
            solid: Some(solid),
            ..self
        }
    }

    /// A point of one of a tile's four vertical faces.
    pub fn face(at: Vec2, z: f32, tile: (i32, i32), face: Face) -> Self {
        Self {
            at,
            z,
            tile,
            surface: Surface::Face(face),
            solid: None,
        }
    }
}

/// What kind of surface a lit point is a point of — the whole of what the
/// lighting asks about a pixel beyond where it is.
///
/// [`crate::place::Stance`] is the same question at the other end of the wire —
/// a corner is resolved to one of its two faces per fragment before the
/// attachment is written, and `docs/archive/render/gbuffer.md` step 4c gave a mesh face
/// (a tread's top or riser) its own honest tag from this same set besides, so
/// what arrives here is always one of these four fixed normals, never a
/// computed one. `docs/archive/render/lighting.md` decision 40 tried carrying a fifth,
/// computed case here (`Sloped`, a blended tread normal) before honest
/// per-face geometry existed to make it unnecessary; `docs/archive/render/gbuffer.md` step 5
/// retired it once measuring against decision 40's own reproduction showed
/// the blend was compensating for a fake continuous-ramp sampling of the
/// flight, not for anything the real, decomposed geometry still needed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Surface {
    /// Standing up with nothing known about which way it runs.
    Upright,
    /// Lying in its tile, looking up: the ground, a floor, a rug, the lid on top
    /// of a wall.
    Flat,
    /// One of the tile's four vertical faces.
    Face(Face),
}

impl Surface {
    /// Which way this surface looks, in [`TileVec`]'s space — the same space a
    /// flame's offset is stated in, which is what makes the dot product between
    /// them a cosine.
    ///
    /// `None` for [`Surface::Upright`], which is a statement about what is *not*
    /// known: a billboard has no side, so every flame that reaches it lights it.
    ///
    /// A **flat** surface looks up, and that is the one this had missing. A wall's
    /// top cap is a flat static, so nothing tested which way it looked and a lamp
    /// standing beside a wall lit its top as fully as one standing over it —
    /// reported from the client as two walls "adding up" at a corner, and it is a
    /// bright diamond where a corner's cap is. `docs/archive/render/lighting.md`, decision 27.
    pub fn normal(self) -> Option<TileVec> {
        match self {
            Self::Upright => None,
            Self::Flat => Some(TileVec::new(0.0, 0.0, 1.0)),
            Self::Face(face) => {
                let [x, y] = face.outward();
                Some(TileVec::new(x, y, 0.0))
            }
        }
    }

    /// The face this is, where it is one.
    pub fn face(self) -> Option<Face> {
        match self {
            Self::Face(face) => Some(face),
            _ => None,
        }
    }
}

// **`Surface::shadowed_by_own_tile` lived here**, and `docs/archive/render/lighting_height.md`
// phase 3 is what retired it. It answered "which of a tile's own sides may
// shadow a pixel standing on it" — `edges` for a `Flat` pixel on a tile with
// named panels, zero for everything else — and `exemption` masked its `lit_end`
// arm with it. That arm also required the surface *not* to be `Flat`, so the mask
// it was anded with was zero for every surface that reached it: the conjunct was
// vacuously true, always, and the real restriction was `caps_this` beside it.
// Identity answers both without a tile-wide union: a floor pixel is a point of
// the floor, not of the wall beside it, so the wall shadows it — which is what
// this function was for — and a face pixel is a point of its own panel and of no
// other on the tile, which the mask could not have said.

/// How much of a flame at `toward` reaches a surface facing `normal`:
/// `max(N · L, 0)`, and nothing else.
///
/// Textbook Lambert, with **no wrap, no band and no width knob** — the decision
/// at the top of `docs/render/design_model.md`. The art is declared clean albedo and
/// this renderer is the ordinary one; no term here has the job of arguing with
/// what an artist painted into a sprite. There was a dial in the plan between a
/// half-space and this, and it was closed rather than tuned; the plan parks a
/// stylised BRDF as an experiment of its own, for a day when there are deferred
/// frames worth comparing one against.
///
/// `normal` is a unit vector and `toward` is the fragment-to-flame offset in
/// tiles, `z` included and already divided into them. **It is normalised here**,
/// which is the whole of what phase 3 changed: the same expression fed an
/// unnormalised offset answered about a *distance* along the normal, so one
/// constant meant one width across a wall and quite another above a lid. A cosine
/// has no length in it and the term is the same on every surface.
///
/// `blit.wgsl`'s `lit_from`, and the two are one formula.
fn lit_from(normal: TileVec, toward: TileVec) -> f32 {
    let length = toward.length();
    // A fragment standing exactly on the flame has no direction to be lit from,
    // and every direction is as good an answer as any other. Full is the one that
    // does not put a black dot in the middle of a pool.
    if length <= 0.0 {
        return 1.0;
    }
    // Spelled out with the division inside each term rather than folded into
    // [`TileVec::dot`]: `(n·t)/L`, `n·(t/L)` and this are three roundings of one
    // number, and a cosine landing a bit either side of zero is a lit pixel or a
    // black one. This is the grouping the shader writes and the one the parity
    // test compares against, so it stays written out.
    let cosine = normal.x * toward.x / length + normal.y * toward.y / length + normal.z * toward.z / length;
    cosine.clamp(0.0, 1.0)
}

/// What took a ray to nothing: not only *where* it was stopped, but *by what*.
///
/// A blamed tile answers "which wall" for exactly as long as a tile holds one
/// thing. A stair's own tile holds six — three tread tops and three risers — and
/// every question worth asking about it ("is this fragment shadowed by its own
/// flight, and by which part of it") reads the same cell whatever the answer
/// turns out to be. A diagnostic that cannot separate those answers cannot be
/// used to choose between the fixes they call for, and choosing between them by
/// reading the code instead is how `docs/archive/render/lighting_height.md` twice let a
/// plausible attribution stand as a measured cause.
///
/// So the cell stays and the occluder is named beside it. [`Stopper::solid`] is
/// the very fact [`exemption`] compares, so a report carrying it can be read
/// against the fragment's own [`Spot::solid`] with nothing re-derived in
/// between: the same id on a solid that still stopped the ray says the exemption
/// did not fire, and a different id says it was never entitled to.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Stopper {
    /// The tile it stands on — what [`Reach::stopped_by`] was, alone, before
    /// there was anything beside it.
    pub cell:  (i32, i32),
    /// Which solid of the frame this is, off the reference the walk followed to
    /// reach it — the very name [`exemption`] compares. See
    /// [`crate::occlusion::SolidId`].
    pub solid: crate::occlusion::SolidId,
    /// Its sides: [`crate::occlusion::Edges::NONE`] for a lid,
    /// [`crate::occlusion::Edges::ANY`] for a body, anything else for a panel.
    ///
    /// The shape rather than the identity, and it is here because a report is
    /// read by a person: "a lid stopped me" and "a panel stopped me" are two
    /// different pictures, and an id number is not one a reader can see.
    pub edges: Edges,
    /// The `z` span **the walk that blamed it actually read**: the record's
    /// exact corners from [`walk_the_record`], the wire's quantised one from
    /// [`walk_the_wire`].
    ///
    /// Deliberately not normalised to one of the two. Which span a walk is
    /// entitled to is the discipline `docs/archive/render/lighting_height.md` phase 2 states,
    /// and a report that quietly picked the exact one would hide the walk that
    /// read the other.
    pub span:  (f32, f32),
}

impl std::fmt::Display for Stopper {
    /// `(100, 100) solid 7, lid z 3.00..3.00` — the cell, the name [`exemption`]
    /// compares, and the shape, in the order a person asks for them. One
    /// formatting, because the flame's report and the sun's both want exactly
    /// this and a second copy would drift.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shape = match self.edges {
            Edges::NONE => "lid",
            Edges::ANY => "body",
            _ => "panel",
        };
        write!(
            f,
            "({}, {}) solid {}, {shape} z {:.2}..{:.2}",
            self.cell.0,
            self.cell.1,
            self.solid.raw(),
            self.span.0,
            self.span.1,
        )
    }
}

/// What one flame did to one spot, and why.
///
/// The *why* is the point: a pool that is missing has one of three causes — the
/// flame is too far, the ray was stopped, or the flame was never collected — and
/// a picture cannot tell the first two apart. This does.
/// Which of [`Lighting::lights`] a [`Reach`] is about.
///
/// The sun's own [`Reach`] carries one past the end of `lights` on purpose —
/// see the constructor at the bottom of [`sample_with`] — so this is never
/// mistaken for any other index into a light-shaped list in this module.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LightIdx(usize);

impl LightIdx {
    /// Its position in [`Lighting::lights`].
    ///
    /// The sun deliberately uses the one-past-the-end value, so callers that
    /// use this to index `lights` must do so only for an ordinary flame reach.
    pub const fn position(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Reach {
    /// Which of [`Lighting::lights`] this is, by index.
    pub light:      LightIdx,
    /// How far the flame is, in tiles, with `z` divided into tiles — the same
    /// three-dimensional distance the falloff uses.
    pub distance:   f32,
    /// Whether that distance is inside the flame's radius. `false` means the
    /// spot is outside the pool and nothing else was computed.
    pub within:     bool,
    /// How much of the flame's own body survived the walk: `1.0` for an open
    /// path, `0.0` for a wall, and between for a partial occluder. Only
    /// meaningful when [`Reach::within`].
    ///
    /// **Visibility and nothing else** — no cosine, no falloff, no beam. This is
    /// what `View::Shadow` draws, and it is the one number that separates "the
    /// walk is wrong" from "the cosine is wrong", which is why phase 5b kept it
    /// beside [`Reach::delivered`] rather than folding the two together.
    pub through:    f32,
    /// What the flame actually delivered here, as a share of what it would
    /// deliver to a surface squarely facing it at no distance at all: the mean
    /// over the flame's body of `visibility × max(N · L, 0) × falloff² × beam`.
    ///
    /// [`Reach::added`] is this times the flame's colour and intensity, and that
    /// holds for the sun's own [`Reach`] as well — one invariant, one arithmetic.
    ///
    /// **This is what [`Reach::cone`] was, and it is a different number.** That
    /// field was "how much of the beam falls here, and how squarely the surface
    /// looks at it", both taken at the flame's *centre*; phase 5b took the centre
    /// out of the shading, so there is no single direction left for either term
    /// to be evaluated along. A sum over the body is the honest replacement, and
    /// a report that still printed one cosine would be printing a number the
    /// renderer no longer computes.
    pub delivered:  f32,
    /// What stopped the ray, where anything did.
    ///
    /// The *first* cell that took the survival to zero and the solid on it that
    /// took most of it — which is the pair worth naming: a ray crossing two walls
    /// is stopped by the first of them and the second is a fact about the map,
    /// not about this pixel. See [`Stopper`].
    pub stopped_by: Option<Stopper>,
    /// What this flame added to the multiplier, linear, per channel.
    pub added:      [f32; 3],
}

/// Everything one point of the world receives, and from what.
///
/// [`sample`] is the CPU's copy of `blit.wgsl`'s fragment loop, and the copy
/// exists for two reasons: a test can assert on numbers instead of on pixels,
/// and the client can answer "why is this tile lit" in words. Both are worthless
/// if the copy drifts, so a GPU test runs the real blit over a synthetic place
/// attachment and asserts the two agree — see `docs/archive/render/lighting.md`, decision 9.
#[derive(Clone, PartialEq, Debug)]
pub struct Sample {
    /// Where this was asked about.
    pub spot:       Spot,
    /// What the art at this spot is multiplied by: the ambient plus every
    /// flame's contribution, unclamped. The shader clamps at the end; this does
    /// not, because a value over one is a real answer — it says the spot is
    /// blown out rather than merely lit.
    pub multiplier: [f32; 3],
    /// One entry per flame the frame carried, in the order [`Lighting::lights`]
    /// holds them — including the ones that reached nothing, which is exactly
    /// what a person asking "why is it dark here" needs to see.
    pub reaches:    Vec<Reach>,
    /// How much of the sun reached this spot, and what stopped it — `None` where
    /// the frame had no sun at all, which is a different answer from `0.0`.
    pub sun:        Option<Reach>,
}

impl Sample {
    /// How bright this spot came out, as one number: the mean of the channels.
    ///
    /// For a diagram and for a test that wants "brighter than" rather than a
    /// colour. Deliberately not luma-weighted — this is not a picture, and a
    /// weighting would make a blue ambient and a warm flame incomparable.
    pub fn brightness(&self) -> f32 {
        self.multiplier.iter().sum::<f32>() / 3.0
    }
}

/// How a [`Stopper`] stands to the fragment it stopped, in words.
///
/// **This used to be a warning about reading two numbers side by side**, and the
/// warning is gone with the numbers: an [`crate::occlusion::OwnerId`] was unique
/// within a *cell*, so a fragment of owner 1 stopped by "owner 1" on a different
/// cell had not been stopped by itself — two unrelated statics that happened to
/// be their own cells' first — and a person reading the report was fooled by it,
/// this session's predecessor included. `docs/render/design_model.md` phase 4 made
/// the comparison a [`crate::occlusion::SolidId`], which names one solid in the
/// whole frame, so equal ids *are* the same solid and there is nothing left to
/// qualify.
///
/// It stays a sentence rather than becoming a bare equality because the answer a
/// person wants is which of three situations they are looking at, and one of the
/// three — a fragment that is a point of nothing at all — is not a comparison of
/// ids but the absence of one.
fn stands_to(spot: Spot, stopper: Stopper) -> &'static str {
    match spot.solid {
        None => "a fragment that is a point of no occluder at all",
        solid if solid == Some(stopper.solid) => "THE FRAGMENT'S OWN SOLID",
        _ => "another solid",
    }
}

impl std::fmt::Display for Sample {
    /// The report: the spot, what it came out at, and a line per flame saying
    /// what happened to it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "({:.2}, {:.2}, z {:.1}) -> {:.3} [{:.3} {:.3} {:.3}]",
            self.spot.at.x,
            self.spot.at.y,
            self.spot.z,
            self.brightness(),
            self.multiplier[0],
            self.multiplier[1],
            self.multiplier[2],
        )?;
        for reach in &self.reaches {
            write!(
                f,
                "  light {}: {:.2} tiles",
                reach.light.position(),
                reach.distance
            )?;
            // In the order the questions are asked: is it near enough, is
            // anything in between, and how much of the flame this surface was
            // turned towards in the end — see [`Reach::delivered`], which is the
            // number that says whether a dark tile is behind a wall or facing
            // away from the fire.
            match (reach.within, reach.stopped_by) {
                (false, _) => writeln!(f, ", outside its radius")?,
                (true, Some(stopper)) => {
                    writeln!(f, ", stopped by {stopper} — {}", stands_to(self.spot, stopper))?
                }
                (true, None) => {
                    writeln!(
                        f,
                        ", visible {:.2}, delivers {:.3}, adds {:.3}",
                        reach.through,
                        reach.delivered,
                        reach.added.iter().sum::<f32>() / 3.0,
                    )?
                }
            }
        }
        if let Some(sun) = self.sun {
            match sun.stopped_by {
                Some(stopper) => {
                    writeln!(
                        f,
                        "  sun: in shadow of {stopper} — {}",
                        stands_to(self.spot, stopper)
                    )?
                }
                None => {
                    writeln!(
                        f,
                        "  sun: through {:.2}, adds {:.3}",
                        sun.through,
                        sun.added.iter().sum::<f32>() / 3.0,
                    )?
                }
            }
        }
        Ok(())
    }
}

/// What a frame's lighting does to one spot in the world, with the reasons.
///
/// `blit.wgsl`'s fragment loop, arithmetic for arithmetic: the same
/// three-dimensional distance with `z` in tiles, the same `(1 - d)²` falloff, the
/// same walk of the grid between the spot and each flame. The shader's clamp and
/// its multiply by the art are the two things left out, because neither is about
/// the lighting — see [`Sample::multiplier`].
pub fn sample(spot: Spot, lighting: &Lighting) -> Sample {
    sample_with(spot, lighting, walk, walk_sun)
}

/// [`sample`], through [`walk_the_record`] instead of [`walk_the_wire`].
///
/// A temporary public seam for `docs/archive/render/lighting_raymarch.md`'s point 3, not a
/// second code path anything real should call: the doc's own oracles
/// (`tests/lighting.rs`'s grid-sweep and fuzz, `tests/frame.rs`'s
/// real-geometry fixtures) run through `sample`, not the walk directly,
/// so exercising `walk_the_record` against them needs its own entry point
/// into the same machinery. It goes away at point 4's cutover, when `sample`
/// itself walks this path and there is only one `sample` to have a seam to.
#[doc(hidden)]
pub fn sample_exact(spot: Spot, lighting: &Lighting) -> Sample {
    sample_with(spot, lighting, walk_exact, walk_sun_exact)
}

fn sample_with(
    spot: Spot,
    lighting: &Lighting,
    walk: impl Fn(Spot, [f32; 3], &Occlusion) -> (f32, Option<Stopper>),
    walk_sun: impl Fn(Spot, Sun, &Occlusion) -> (f32, Option<Stopper>),
) -> Sample {
    // The ambient this *tile* has, and not the frame's: how much of the sky the
    // column over it can see decides how much of the sky term it gets. The tile
    // and not the fractional spot, because the field is a byte a tile — the blur
    // of `docs/archive/render/lighting_world.md`'s decision 2 is what softens its edges, and a
    // second interpolation here would be a different picture from the shader's.
    let mut multiplier = lighting
        .ambient
        .at(lighting.occlusion.sky_at(spot.tile.0, spot.tile.1));
    let mut reaches = Vec::with_capacity(lighting.lights.len());
    for (index, light) in lighting.lights.iter().enumerate() {
        let offset = TileVec::between(
            WorldVec::new(spot.at.x, spot.at.y, spot.z),
            WorldVec::new(light.at.x, light.at.y, light.z),
        );
        let distance = offset.length();
        // **The one thing still asked about the flame's centre, and it is
        // therefore conservative.** This is a broad phase: it decides which
        // flames to walk rays for and is forbidden to change the answer. A
        // fragment the centre says is out of reach can be reached by the *near
        // side* of a body that has one, so the near side is what is tested —
        // `docs/render/design_model.md` phase 5b.
        if distance - lighting.flame_radius >= light.radius.max(0.001) {
            reaches.push(Reach {
                light: LightIdx(index),
                distance,
                within: false,
                through: 0.0,
                delivered: 0.0,
                stopped_by: None,
                added: [0.0; 3],
            });
            continue;
        }
        // Eight rays at the flame's own sphere, and what they brought: the
        // cosine, the falloff and the beam are all taken at the point each ray
        // ends on, so there is nothing left out here to multiply the result by.
        // See [`arrival`], and phase 5b for why a flame with a centre drew a
        // wedge of shadow at every join.
        let arrival = arrival(
            spot,
            light,
            &lighting.occlusion,
            lighting.flame_radius,
            lighting.shadow_rays,
            &walk,
        );
        let added = light
            .color
            .map(|channel| channel * light.intensity * arrival.delivered);
        for (total, channel) in multiplier.iter_mut().zip(added) {
            *total += channel;
        }
        reaches.push(Reach {
            light: LightIdx(index),
            distance,
            within: true,
            through: arrival.visible,
            delivered: arrival.delivered,
            stopped_by: arrival.stopped_by,
            added,
        });
    }
    // And the sun, which is one direction rather than a place and therefore not
    // in the loop above: no distance, no falloff, and a walk with no endpoint.
    let sun = lighting.sun.map(|sun| {
        let (through, stopped_by) = walk_sun(spot, sun, &lighting.occlusion);
        let added = sun.color.map(|channel| channel * sun.intensity * through);
        for (total, channel) in multiplier.iter_mut().zip(added) {
            *total += channel;
        }
        Reach {
            // The sun is not one of `lights`, and the index says so by being
            // past the end of it rather than by being a zero somebody might read
            // as "the first flame".
            light: LightIdx(lighting.lights.len()),
            distance: f32::INFINITY,
            within: true,
            through,
            // The sun is a direction and not a beam, and it has no cosine at all
            // yet — `docs/render/design_model.md` phase 8 is where it gets the same
            // BRDF as everything else. So what it delivers *is* what it is
            // visible over, and [`Reach::added`] is that times the colour, which
            // is the same invariant every flame above keeps.
            delivered: through,
            stopped_by,
            added,
        }
    });

    Sample {
        spot,
        multiplier,
        reaches,
        sun,
    }
}

/// The sun's ray from a spot: how much of it arrives, and what stopped it.
///
/// `blit.wgsl`'s `sunlight`. What the sun has instead of a position is a
/// *direction*, so the only thing this does that [`walk`] does not is work out
/// where the segment ends: the point at which the ray leaves the grid's ceiling,
/// because from there on it is looking at sky. Everything after that is
/// [`walk_the_wire`], the same walk a flame's ray takes.
///
/// The spot's own tile is skipped, as it is for a flame, and for the same reason
/// in reverse: a wall's own pixels are on a tile that stops light, and a wall
/// that shadowed itself would be black on the side the sun is on. The far end is
/// *not* skipped — there is no tile there, only a point in the sky.
fn walk_sun(spot: Spot, sun: Sun, occlusion: &Occlusion) -> (f32, Option<Stopper>) {
    let horizontal = (sun.toward.x * sun.toward.x + sun.toward.y * sun.toward.y).sqrt();
    if horizontal < 1e-6 {
        // Straight overhead: there is no direction to walk along the ground, and
        // the only thing that could shadow the spot is on its own tile — which is
        // exempt. Nothing stops it.
        return (1.0, None);
    }
    // One tile of ground a unit, so `z` climbs by the sun's own slope — and back
    // into world units, because that is what the grid this walks is stated in.
    let step = sun.toward.divided(horizontal).in_world_units();
    let mut tiles = MAX_SUN_TILES;
    if let (Some(ceiling), true) = (occlusion.tallest(), step.z > 1e-6) {
        tiles = tiles.min((ceiling as f32 - spot.z) / step.z);
    }
    if occlusion.tallest().is_none() || tiles <= 0.0 {
        // Nothing in the grid stops anything, or the spot is already above
        // everything that could — either way the ray is in the sky from here.
        return (1.0, None);
    }
    let from = WorldVec::new(spot.at.x, spot.at.y, spot.z);
    let to = WorldVec::new(
        from.x + step.x * tiles,
        from.y + step.y * tiles,
        from.z + step.z * tiles,
    );
    // No tile to exempt at the far end, and no source size: the sun subtends half
    // a degree, which is a point at this scale.
    walk_the_wire(from.array(), to.array(), LitEnd::of(spot), occlusion)
}

/// The ray from a spot to a point of a flame: [`walk_the_wire`] with the
/// two ends of it.
///
/// **`at` and not `light.at`**, because phase 5 asks this [`SHADOW_RAYS`] times
/// per flame with that many points of the flame's own sphere — see [`arrival`],
/// which is what callers want.
///
/// There is nothing left in here that a sun's ray does not also have: the
/// `spread` this used to carry was a flame's size standing in for its own
/// penumbra, and the size is in the ray's far end now.
fn walk(spot: Spot, at: [f32; 3], occlusion: &Occlusion) -> (f32, Option<Stopper>) {
    walk_the_wire([spot.at.x, spot.at.y, spot.z], at, LitEnd::of(spot), occlusion)
}

/// A number in `0.0..1.0` that belongs to a point of the world and to no other:
/// the rotation [`arrival`] turns its sample pattern by.
///
/// **World space and integers, so that both backends produce the same number.**
/// The obvious thing — a hash of the pixel — cannot be spelled identically on
/// the CPU side, which has no pixel; and a float hash of the kind shaders
/// usually carry (`fract(sin(dot(…)))`) is exactly the arithmetic two backends
/// are least likely to agree about. This quantises the position to a
/// hundred-and-twenty-eighth of a tile, which is under a screen pixel at 1:1 and
/// well under one at any zoom this client draws, and mixes the three integers
/// with a plain bit-avalanche. It is *stable in the world* rather than on the
/// screen, so a panning camera does not make a penumbra crawl.
///
/// What it is worth: eight rays put nine levels across a shadow's edge, and with
/// one pattern for the whole frame those nine are nine visible bands. Rotating
/// the pattern per fragment spends the same eight rays on a different eight
/// directions each time, which turns the banding into grain — the error is the
/// same size and the eye is far worse at seeing it. `blit.wgsl`'s `dither`.
fn dither(at: [f32; 3]) -> f32 {
    let quantised = |axis: f32| (axis * 128.0).floor() as i32 as u32;
    let mut hash = quantised(at[0]).wrapping_mul(0x8DA6_B343)
        ^ quantised(at[1]).wrapping_mul(0xD816_3841)
        ^ quantised(at[2]).wrapping_mul(0xCB1A_B31F);
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x7FEB_352D);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x846C_A68B);
    hash ^= hash >> 16;
    // The top 24 bits over 2²⁴: a float's own mantissa, so every value this can
    // produce is representable and none of them is `1.0`.
    (hash >> 8) as f32 / 16_777_216.0
}

/// Turns of the sample spiral between one point of a flame and the next.
///
/// The golden angle, `π(3 − √5)`, which is what makes a Vogel spiral of any
/// length evenly spread rather than spoked: no two of the eight fall near one
/// another and the eighth does not land back on the first. Written out rather
/// than computed, so that `blit.wgsl`'s copy is the same bits.
const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// Where the rays of [`arrival`] end: [`SHADOW_RAYS`] points of the sphere a
/// flame at `flame` is, as `spot` sees it.
///
/// A position and not a [`Light`], because that is all it is about: two points
/// and a radius. An oracle holding a flame as three numbers can ask this, which
/// is most of them.
///
/// A Vogel spiral on the disc the sphere presents to the spot — the silhouette,
/// not the surface, which is the same choice `pathtrace::Emitter::Sphere` makes
/// and for the same reason: what a receiver can be occluded from is the disc it
/// sees. `sqrt` of the index spaces them by equal *area* rather than by equal
/// radius, so the middle of the flame is not sampled eight times over, and
/// [`dither`] turns the whole pattern by an angle that belongs to the spot.
///
/// The disc is laid out in **tile space**, `z` divided by [`Z_PER_TILE`], and the
/// offsets multiplied back on the way out: that is the metric the sphere is round
/// in, and the one the falloff and the cosine are already stated in.
///
/// **Public because an oracle needs it.** A detector comparing this renderer's
/// shadows against an independent point-in-box sampler has to be asked about the
/// same *body*, or it is comparing a sphere against a point and reporting the
/// difference as the walk's — see `tests/lighting.rs`'s fuzz, which is where that
/// happened. What it shares with the thing under test is the scene, not the
/// answer.
pub fn flame_points(spot: Spot, flame: [f32; 3], radius: f32, rays: ShadowRays) -> FlamePoints {
    if radius <= 0.0 {
        return FlamePoints {
            points: [flame; ShadowRays::MOST as usize],
            rays,
        };
    }
    let toward = TileVec::between(
        WorldVec::new(spot.at.x, spot.at.y, spot.z),
        WorldVec::from_array(flame),
    );
    let span = toward.length();
    if span < 1e-6 {
        // The spot is inside the flame. Every point of the sphere is as good as
        // any other and none of them has a direction, so every ray is the one to
        // the centre — the ray that has no length.
        return FlamePoints {
            points: [flame; ShadowRays::MOST as usize],
            rays,
        };
    }
    let normal = toward.divided(span);
    // Two directions across the ray, built the textbook branching way: away from
    // whichever axis the ray is most nearly along, so the cross product is never
    // near zero. Any consistent pair does — the pattern is rotated per fragment
    // anyway — and what matters is only that `blit.wgsl` builds the same one.
    let aside = match normal.x.abs() > 0.9 {
        true => TileVec::new(0.0, 1.0, 0.0),
        false => TileVec::new(1.0, 0.0, 0.0),
    };
    let across = aside.cross(normal);
    let across = across.divided(across.length());
    let up = normal.cross(across);

    let phase = dither([spot.at.x, spot.at.y, spot.z]) * std::f32::consts::TAU;
    // Every slot of the array is filled, and only the first `rays` of them are
    // ever read — see [`FlamePoints`]. The spiral's own radius is what the count
    // is in, so the points past it are not the points a shorter walk would have
    // used, and answering with them would be a different flame.
    let points = std::array::from_fn(|ray| {
        let angle = phase + GOLDEN_ANGLE * ray as f32;
        let radius = radius * ((ray as f32 + 0.5) / rays.count() as f32).sqrt();
        let (sin, cos) = angle.sin_cos();
        // The disc is spanned in tile space and the point put back into world
        // units on the way out — the one line that used to carry `Z_PER_TILE` on
        // its `z` alone, with an offset and a position in one expression and
        // nothing but the reader to tell them apart.
        let offset = across
            .scaled(cos)
            .plus(up.scaled(sin))
            .scaled(radius)
            .in_world_units();
        [flame[0] + offset.x, flame[1] + offset.y, flame[2] + offset.z]
    });
    FlamePoints { points, rays }
}

/// The points [`flame_points`] named: a fixed array with a count, and the count
/// is what a reader is allowed to look at.
///
/// A type and not a `Vec`, because this is called once per flame per fragment in
/// the walk that every oracle in the tree runs — an allocation there is the whole
/// cost of the answer. A type and not a bare `[[f32; 3]; MOST]` because the
/// slots past the count hold *a different flame's* sample: the spiral spaces its
/// points by `sqrt(i / rays)`, so the ninth point of an eight-ray flame is not a
/// point of that flame at all, and an array with no count is an invitation to
/// average it in.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FlamePoints {
    points: [[f32; 3]; ShadowRays::MOST as usize],
    rays:   ShadowRays,
}

impl FlamePoints {
    /// The points, in the spiral's own order.
    pub fn iter(&self) -> impl Iterator<Item = [f32; 3]> + '_ {
        self.points[..self.rays.count()].iter().copied()
    }

    /// How many there are — [`ShadowRays::count`] of the flame they are of.
    pub fn count(&self) -> usize {
        self.rays.count()
    }
}

impl IntoIterator for FlamePoints {
    type Item = [f32; 3];
    type IntoIter = std::iter::Take<std::array::IntoIter<[f32; 3], { ShadowRays::MOST as usize }>>;

    fn into_iter(self) -> Self::IntoIter {
        let rays = self.rays.count();
        self.points.into_iter().take(rays)
    }
}

/// What one flame's body sends to one spot, and how much of that body the spot
/// can see — the two answers [`arrival`] returns, and they are different
/// questions.
///
/// `docs/render/design_model.md` phase 5b named the split: a flame is a body for
/// *every* term now, so what it delivers is a sum over its own surface and no
/// longer a visibility share that something outside the loop scales. The share
/// it is visible over survives as its own number because a diagnostic asks for
/// it by name — `View::Shadow` is visibility and nothing else, and it is the one
/// instrument that separates "the walk is wrong" from "the cosine is wrong".
#[derive(Clone, Copy, PartialEq, Debug)]
struct Arrival {
    /// Λ — the mean over the flame's sample points of
    /// `visibility × max(N · L, 0) × falloff² × beam`.
    ///
    /// Everything a flame's colour and intensity are multiplied by, in one
    /// number, because every one of those terms is a function of the *sample
    /// point* and none of them of the flame's centre.
    delivered:  f32,
    /// The share of the flame's body this spot can see: the mean of the same
    /// rays' visibility alone, with no cosine, no falloff and no beam in it.
    visible:    f32,
    /// What took the most from any single ray **that had light to lose**.
    ///
    /// A ray from below the spot's own horizon delivers exactly zero whatever
    /// stands in its way, so naming what stopped it would be blaming an occluder
    /// for a darkness the cosine had already decided. That is the whole of what
    /// phase 5b's wedge was: rays that could not have lit anything, reported as
    /// shadow.
    stopped_by: Option<Stopper>,
}

/// What a flame at `light` delivers to `spot`, and how much of it the spot sees:
/// [`SHADOW_RAYS`] rays at the points of the flame [`flame_points`] names, with
/// **every term of the sum taken at the sample point**.
///
/// **This is `docs/render/design_model.md` phase 5b**, and the construction is not
/// "the cosine moved inside the loop" — it is that *the sample point is the only
/// place a flame has a position*. Visibility, the cosine, the falloff and the
/// beam are all functions of `p`, and `light.at` appears nowhere below except as
/// the centre [`flame_points`] lays its disc around:
///
/// ```text
/// Λ = (1 / N) · Σ_p  V(p) · max(N · L_p, 0) · fall(p)² · cone(p)
/// ```
///
/// What that fixes, and it is exact rather than a mitigation: a lamp lower than
/// the flame's own radius above a floor puts half its sphere **below** that
/// floor's plane. Those rays were traced, and near a join they left the
/// fragment's own primitive and came back blocked — a wedge of shadow on a
/// surface that is flush and continuous. The set of rays a join can block and
/// the set of rays below the horizon are *the same set*, so `max(N · L_p, 0)`
/// removes the wedge entirely instead of dimming it.
///
/// **A sample whose cosine is not positive is not accumulated at all**, and that
/// is an exact skip rather than a tolerance: its contribution is zero whatever
/// stands in its way. The walk still runs for it here, because this is the
/// diagnostic twin and [`Arrival::visible`] is read as a complete answer by every
/// oracle in the tree — `blit.wgsl` skips the ray itself in the lit path, which
/// is where the cost is, and walks every sample in a debug view. The two agree
/// about [`Arrival::delivered`] to the bit either way, which is what makes the
/// shader's skip a cost and not a second answer.
///
/// `blit.wgsl`'s `arrival`, and the two are one arrangement.
fn arrival(
    spot: Spot,
    light: &Light,
    occlusion: &Occlusion,
    // How big the flame is — [`Lighting::flame_radius`], carried down rather
    // than read off the constant so that the shader and this walk can be asked
    // about the same frame.
    radius: f32,
    // And how many rays to cast at it — [`Lighting::shadow_rays`], carried down
    // for exactly the same reason and with a sharper edge: two walks that agree
    // about every rule and sample a different number of points disagree about
    // every soft pixel, and an oracle cannot tell that from a defect.
    rays: ShadowRays,
    walk: impl Fn(Spot, [f32; 3], &Occlusion) -> (f32, Option<Stopper>),
) -> Arrival {
    let rays = rays.for_radius(radius);
    let normal = spot.surface.normal();
    let reach = light.radius.max(0.001);
    let mut delivered = 0.0;
    let mut visible = 0.0;
    let mut worst: Option<(f32, Stopper)> = None;
    for at in flame_points(spot, [light.at.x, light.at.y, light.z], radius, rays) {
        // From the spot to *this point of the flame*, in the one metric — which
        // is what the cosine, the falloff and the beam are all stated in.
        let toward = TileVec::between(
            WorldVec::new(spot.at.x, spot.at.y, spot.z),
            WorldVec::from_array(at),
        );
        let cosine = match normal {
            None => 1.0,
            Some(normal) => lit_from(normal, toward),
        };
        let (through, stopped_by) = walk(spot, at, occlusion);
        visible += through;
        if cosine <= 0.0 {
            continue;
        }
        // The falloff of *this* ray, and it is clamped where the centre's never
        // had to be: the cull below is conservative, so a spot inside the pool by
        // the near edge of the sphere can be past the reach of its far side, and
        // `(1 - d)²` of a negative `1 - d` is a bright ring at the rim.
        let distance = toward.length();
        let fall = (1.0 - distance / reach).max(0.0);
        // And which way the flame is pointing at this point of itself. The offset
        // runs from the spot to the flame and a beam's axis runs the other way,
        // so the sign flips here.
        let cone = match light.beam {
            Some(beam) => beam.lights(toward.negated()),
            None => 1.0,
        };
        delivered += through * cosine * fall * fall * cone;
        if let Some(stopper) = stopped_by {
            if worst.is_none_or(|(lost, _)| through < lost) {
                worst = Some((through, stopper));
            }
        }
    }
    Arrival {
        delivered:  delivered / rays.count() as f32,
        visible:    visible / rays.count() as f32,
        stopped_by: worst.map(|(_, stopper)| stopper),
    }
}

// **`walk_cells`'s own doc comment stood here, orphaned**, and it is deleted
// with the last of the cells it described. The function went at
// `docs/archive/render/lighting_raymarch.md`'s point 4 cutover and the doc outlived it,
// attached to nothing and still promising things no walk has done since: a
// crossing length a cell's opacity is scaled by (phase 5 made every crossing
// hard), `FLAME_SPREAD` and its two bounds (deleted with the pancake flame),
// and a starting cell that is always skipped (phase 4 replaced it with the
// fragment's own solid).
//
// Worth a line rather than a silent removal, because it is the fourth thing on
// this track to be found still describing a rule that had been gone for
// phases — see the fixtures in `docs/render/design_occluders.md`'s S4. A doc comment nothing
// compiles against decays exactly like a test whose subject was taken away.
// **The DDA over the tile grid stood here, and `docs/render/design_occluders.md`'s S5 deleted
// it**: `dda_walk`, `candidate_tiles`, `DdaCell`, the `first` cell both walks
// seeded themselves with by `from.floor()`, and `MAX_WALK_STEPS`, which counted
// its steps. What answers "what might this segment meet" now is
// [`crate::occlusion::bvh::Bvh`] and [`candidates`] below.
//
// What the grid was, in one paragraph, because the shape of it is what the
// replacement has to keep: a ray stepped from cell to cell along the nearer of
// its two axis boundaries, never skipping one, and every solid registered on a
// cell it stepped through was a candidate. `candidate_tiles` added both
// single-axis neighbours at every transition — unconditionally, because CPU and
// GPU do not compute a close-enough `boundary[0] - boundary[1]` to agree on
// which rays are near a tie, so a *gated* probe made the two backends probe
// different rays. A tree has no ties to break: a node is hit or it is not, by
// the same slab test a primitive is, and both backends read one uploaded box.
//
// **Three things the grid got wrong that this is not free of by accident but by
// construction**, and they are `docs/render/design_occluders.md`'s own backlog:
//
//   - **A cell listed a primitive once**, in the cell it was added on, so the
//     first box wider than its own tile would have been invisible to a ray that
//     crossed only the overhang. A leaf holds a primitive whatever its size.
//   - **And listing one from two cells would have double-counted it**, because
//     `through` was multiplied cell after cell. A primitive is under exactly one
//     leaf, so it is applied exactly once — `bvh`'s own
//     `every_primitive_is_named_by_exactly_one_leaf`.
//   - **A `floor()` decided which cell a point on a boundary was in.** That is
//     what took `starting_cell` a session to remove and what made both
//     brute-force oracles wrong for a day (§ *The oracle*). There is no `floor`
//     left in either walk.

/// Every primitive the segment `from`..`to` might meet — a **superset**, which
/// is the whole of what `docs/render/design_occluders.md`'s D4 allows a broad phase to decide.
///
/// What comes back is not "the primitives the ray hits": it is the primitives
/// whose *node* boxes the ray hits, which is more of them, and the answer is the
/// per-primitive rules over that set and nothing else. So every knob in the
/// tree — the leaf size, the split rule, this budget — cannot move a pixel, and
/// the brute-force oracle is what says so rather than this comment.
///
/// **Stackless, and that is the shape rather than an optimisation.** WGSL has no
/// dynamic stack, and a fixed-size array of one would be a cap that silently
/// truncates — the shape `MAX_WALK_STEPS` had. A node the segment misses is one
/// assignment to that node's own escape index, which is where its whole subtree
/// ends; a node it hits steps to `at + 1`, which is that node's first child. A
/// leaf's escape *is* `at + 1`, so the two agree there and the loop needs no
/// case for it.
///
/// Both of those are strictly forward, which is the whole of why there is no
/// budget: see the loop's own comment, and the note where `MAX_WALK_STEPS`
/// stood.
///
/// Mirrored in `blit.wesl`'s `candidates`, and the two are one traversal.
fn candidates<'a>(
    occlusion: &'a Occlusion,
    from: [f32; 3],
    to: [f32; 3],
    mut each: impl FnMut(crate::occlusion::SolidId, &'a crate::occlusion::Solid),
) {
    let bvh = occlusion.bvh();
    let end = bvh.past_the_end();
    let mut at = crate::occlusion::bvh::NodeIdx::ROOT;
    while at < end {
        let node = bvh.node(at);
        // The same slab test a primitive gets, against a box built out of the
        // same `f32` corners — see `bvh::Node::space` for why a node whose
        // corners were the record's `f64` could round inward of a primitive's
        // and lose it.
        let next = match ray_vs_solid(from, to, &node.space) {
            None => node.escape,
            Some(_) => {
                if let Some(leaf) = node.leaf {
                    for id in bvh.primitives(leaf) {
                        each(*id, occlusion.solid(*id));
                    }
                }
                crate::occlusion::bvh::NodeIdx::new(at.depth_first_index() + 1)
            }
        };
        // **The loop's own bound, and there is no constant in it**: a
        // well-formed tree always moves forward, so this visits each node at
        // most once and cannot run longer than the frame's own node count. A
        // tree that does not move forward is malformed — an escape pointing at
        // or behind its own node, which nothing on this side writes and
        // `bvh`'s `a_nodes_escape_is_the_end_of_its_own_subtree` gates — and
        // stopping is the one thing to do with it that is neither a hang nor a
        // number somebody has to size. See the note where `MAX_WALK_STEPS`
        // stood.
        if next <= at {
            break;
        }
        at = next;
    }
}

/// Which tile a ray was stopped on, for the report alone.
///
/// **A report's coordinate and not a rule's**, and the distinction is the whole
/// of `docs/render/design_occluders.md`'s S4: [`Stopper::cell`] is read by a person and by the
/// handful of tests that name a wall by where it stands, and nothing about the
/// light depends on it. So a `floor` here is allowed where one in a walk is not.
///
/// **The middle of the crossing, and no longer the primitive's own low corner**
/// — S3b is what moved it. A merged run of wall is one primitive over four
/// tiles, so its low corner names the end of the run rather than the place the
/// ray met it, and "which wall stopped me" is a question about the meeting. The
/// two answers were the same number for every shape the grid built before the
/// merge, which is why the corner was enough until now.
///
/// The middle rather than where the segment came in: an entry point lies exactly
/// on the box's own face, and a face at a whole coordinate floors into the tile
/// next door. That is § *The oracle*'s own defect, measured there and worth a
/// line here because it was walked into again on the way to this fix.
fn tile_of(at: [f32; 3]) -> (i32, i32) {
    (at[0].floor() as i32, at[1].floor() as i32)
}

/// The shadow rules over one segment and the primitives a tree hands it — what
/// both CPU walks are, with the one thing that differs between them as a
/// parameter.
///
/// **That one thing is which box a primitive is**: the record's exact
/// [`crate::occlusion::Solid::space`] for [`walk_the_record`], and the wire's
/// `f32` [`crate::occlusion::Solid::wire_box`] for [`walk_the_wire`],
/// which exists to be a faithful preview of what `blit.wesl` reads rather than a
/// better version of it. Before `docs/render/design_occluders.md`'s S1 the gap between the two
/// was a quantisation — a box rebuilt from a cell and four bytes — and the two
/// walks were two functions because they genuinely computed different geometry.
/// S1 collapsed the gap to an `f32` rounding, and S5 leaves them differing by
/// exactly this one call, so a second copy would now be two chances for one rule
/// to drift.
///
/// **The accumulation is a product over primitives, and the per-cell `max` is
/// gone with the cell it was about.** `docs/render/design_occluders.md`'s S4 left that
/// deletion blocked on this step, and what arrives with the tree is not the
/// grouping moving to something else but the grouping *disappearing*: what
/// crosses a segment is a volume, and a segment that crosses two volumes is
/// stopped by both. The `max` said "two panels of one corner are one wall,
/// counted once", which is a statement about a **cell** — the only thing that
/// ever grouped them — and a corner's two panels overlap in the square where
/// they meet, so it was also the one arrangement where the two rules differ on
/// real geometry.
///
/// Measured before it went: across the whole suite a second solid of one cell
/// stops a ray 1,359 times and **every one of them is two opaque stoppers**,
/// where `max(1, 1)` and `1 - 0·0` are the same number by arithmetic. So this
/// moves no pixel of anything the crate draws, which is what D4 requires, and
/// the arrangement where it would — two *partial* occluders crossed by one
/// segment — is pinned by
/// `a_segment_through_two_panes_is_dimmed_by_both_of_them` rather than left to
/// whichever rule a later reader assumes.
fn walk_primitives(
    from: [f32; 3],
    to: [f32; 3],
    lit: LitEnd,
    occlusion: &Occlusion,
    box_of: impl Fn(&crate::occlusion::Solid) -> crate::solid::Solid,
) -> (f32, Option<Stopper>) {
    // The box the lit end is a point of, which is where its own plane comes
    // from — through the same `box_of` as every candidate, which is what puts
    // both sides of [`on_the_lit_surface`]'s comparison in one precision. `None`
    // for a fragment of no occluder: the ground and a mobile are points of
    // nothing and are exempt from nothing.
    let own_box = lit.solid.map(|id| box_of(occlusion.solid(id)));
    let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut through = 1.0_f32;
    // The primitive to blame, and **the earliest crossing rather than the
    // largest**, which is the one rule that survives the tree having no ray
    // order in it. `walk_the_record` used to sort its candidate cells by
    // nearest crossing and blame the cell that tripped the cutoff, which was the
    // first blocking cell in ray order; a tree hands its leaves back in its own
    // order, so "first in ray order" has to be computed rather than arrived at.
    // Kept as the `entered` beside the stopper, so the comparison is a number
    // and not a re-derivation.
    let mut worst: Option<(f32, Stopper)> = None;
    candidates(occlusion, from, to, |id, stands| {
        // The box this walk is entitled to read — see this function's own doc.
        let space = box_of(stands);
        let (low, high) = (space.min.z as f32, space.max.z as f32);
        let Some((entered, leaves)) = ray_vs_solid(from, to, &space) else {
            return;
        };
        // **A surface does not shadow itself, and that is the whole rule** —
        // `docs/render/design_model.md` phase 4. `Some(id) == Some(id)` and never
        // `None == None`: a fragment that is a point of no occluder is exempt
        // from nothing.
        if lit.solid == Some(id) {
            return;
        }
        // **And a surface does not shadow itself when it is cut into more than
        // one primitive**, which is the whole of D2 — `docs/render/design_occluders.md`'s S3.
        // A neighbouring box whose extent along this fragment's own normal ends
        // exactly on this fragment's plane lies wholly behind that plane, so
        // there is nothing there for the ray to cross.
        if own_box
            .as_ref()
            .is_some_and(|own| on_the_lit_surface(lit.surface, own, &space, delta))
        {
            return;
        }
        // **A ray that only touches a solid at the point it starts from has not
        // gone through it**, and that is exactly `crosses`'s own strictness said
        // about a box instead of a plane. No epsilon: the interval is `0.0..0.0`,
        // both ends exact numbers off the slab test.
        //
        // The case it is about is a tread's outer corner. A riser is a plane on
        // the climb axis and a tread's lid stops exactly at it, so a fragment on
        // that lip stands *in* the riser's own plane at exactly the riser's top —
        // and every ray it sends anywhere touches the riser's box at `t = 0` and
        // nowhere else. Identity cannot excuse it, because the riser is genuinely
        // a different primitive from the lid. Measured before the rule: 88 pixels
        // of a three-tread flight drawn shadowed where every independent oracle in
        // the tree says lit.
        if entered == 0.0 && leaves == 0.0 {
            return;
        }
        let middle = (entered + leaves) * 0.5;
        // Where the ray is *inside* this primitive, which is what both the run
        // fraction below and the report at the end of this closure mean by "where
        // it crossed". The midpoint and not the entry point: an entry lies on the
        // box's own face, and a face at a whole coordinate floors into the tile
        // next door — § *The oracle*'s own defect, and it is no more welcome in a
        // report than in a rule.
        let cross = [
            from[0] + delta[0] * middle,
            from[1] + delta[1] * middle,
            from[2] + delta[2] * middle,
        ];
        let opacity = f32::from(stands.opacity) / 255.0;
        let by_surface = match stands.edges {
            // A **lid is a body**, and this arm is the body's — `docs/render/design_frame_assembly.md`
            // P4 step 1. A lid used to be a plane in `z` and needed a rule of its
            // own: `crosses` over the segment's two ends, because a hit against
            // a degenerate box says only that the ray touched the plane, and at a
            // corner of the footprint it says that at one `t` where every `z`
            // collapses to one number. A floor leaked one bright point per corner
            // of the grid; reported at `(1492, 1642)`, `z 28`,
            // `docs/render/design_model.md` phase 6i, and cured there by a widening
            // rather than by geometry.
            //
            // [`crate::occlusion::Solid::box_of`] gives a lid a span of its own
            // now, so [`ray_vs_solid`]'s exact slab test answers it the
            // way it answers every other box: a `Some` here means the segment
            // genuinely went from one side of the floor to the other. What
            // `crosses`'s strictness was protecting — a candle standing on the
            // floor it lights, sending every ray from that floor's own plane —
            // is protected by the geometry instead: those rays leave the top face
            // and never enter the box below it, and where the floor is cut into
            // more than one primitive [`on_the_lit_surface`] above is what says
            // so, on the same terms it says it for a run of wall.
            //
            // A body is a real 3D box: occlusion is the primitive's own opacity
            // outright. No length-based fade, no per-side floor, no widened-corner
            // graze: those existed only to fake a penumbra a point flame does not
            // cast. See `docs/archive/render/lighting_raymarch.md`'s "hard shadows" decision.
            Edges::NONE | Edges::ANY => opacity,
            // A panel: a named side, and what stops the ray is whether it crossed
            // the plane *inside* the panel's own drawn extent — [`pierced`], and
            // its hole. There was a gate before it, `edges & !same_run == 0`, and
            // `docs/render/design_occluders.md`'s S4 is where it went: the exemption it spelled
            // is [`on_the_lit_surface`]'s, stated about the fragment's own plane
            // instead of about a row of cells, and it is applied above with the
            // rest of them.
            _ => opacity * pierced(stands, cross),
        };
        if by_surface <= 0.0 {
            return;
        }
        through *= 1.0 - by_surface;
        // `<` and not `<=`, so a tie names the primitive met first in the tree's
        // own order rather than the last one to equal it — the same `>`-not-`>=`
        // discipline the per-cell version kept, said about the ray's `t`.
        if worst.as_ref().is_none_or(|(seen, _)| entered < *seen) {
            worst = Some((
                entered,
                Stopper {
                    cell:  tile_of(cross),
                    solid: id,
                    edges: stands.edges,
                    span:  (low, high),
                },
            ));
        }
    });
    // **The cutoff is applied at the end and no longer exits the loop early**,
    // and that is what makes the blamed primitive independent of the order a
    // tree hands its leaves back in. It cost nothing to give up: the early exit
    // saved cells of a walk whose length was the ray's, where a traversal's cost
    // is the geometry it actually meets. The value is unchanged either way — a
    // product that has already fallen under the cutoff cannot climb back out of
    // it.
    match through <= RAY_CUTOFF {
        true => {
            (
                0.0,
                Some(
                    worst
                        .expect("a ray that trips the cutoff has a primitive that did it")
                        .1,
                ),
            )
        }
        false => (through, None),
    }
}

/// The walk against the **record's** own boxes — `docs/archive/render/lighting_raymarch.md`'s
/// ray-vs-Solid scoping, point 2, and since `docs/render/design_occluders.md`'s S5 one call to
/// [`walk_primitives`] rather than a walk of its own.
///
/// The exact one: [`crate::occlusion::Solid::space`] is what the world built and
/// what a hand-written fixture states, so this is the walk an oracle is compared
/// against and the one a test asserting about geometry means.
///
/// **It was `walk_cells_exact` until S5 finished**, and the rename is the last of
/// that step: there is no cell in either walk any more — a hierarchy over
/// primitives is what answers "what might this segment meet", and what the two
/// still differ by is which *boxes* they read. So the pair is named for that and
/// for nothing else: the record's and [`walk_the_wire`]'s.
fn walk_the_record(
    from: [f32; 3],
    to: [f32; 3],
    lit: LitEnd,
    occlusion: &Occlusion,
) -> (f32, Option<Stopper>) {
    walk_primitives(from, to, lit, occlusion, |stands| stands.space)
}

/// And against the **wire's** — the `f32` a shader will read, which is what
/// makes this a faithful preview of `blit.wesl` rather than a better version of
/// it. [`walk`] and [`walk_sun`]'s own walk.
///
/// **What that costs is an `f32` rounding and nothing else** —
/// `docs/render/design_occluders.md`'s S1. It used to be a quantisation: a solid was
/// reconstructed from `(tile, bottom, top, fraction)`, four bytes of
/// two-hundred-and-fifty-fifths of a tile across and sixteen bits an end up,
/// because the upload folded every primitive back onto a cell. This walk
/// mirrored that fold on purpose, which is why the two CPU walks read different
/// heights for one solid *by design*; with the fold gone the difference collapses
/// to the last bits of a float, which decision 9's own parity tolerance absorbs
/// and which is what D10 says the gap between record and wire is for — measuring,
/// not hiding.
fn walk_the_wire(from: [f32; 3], to: [f32; 3], lit: LitEnd, occlusion: &Occlusion) -> (f32, Option<Stopper>) {
    walk_primitives(from, to, lit, occlusion, |stands| stands.wire_box())
}

/// [`walk`], through [`walk_the_record`] instead of [`walk_the_wire`] — for
/// `docs/archive/render/lighting_raymarch.md`'s point 3 agreement pass, not for anywhere
/// real.
fn walk_exact(spot: Spot, at: [f32; 3], occlusion: &Occlusion) -> (f32, Option<Stopper>) {
    walk_the_record([spot.at.x, spot.at.y, spot.z], at, LitEnd::of(spot), occlusion)
}

/// [`walk_sun`], through [`walk_the_record`] instead of [`walk_the_wire`] —
/// see [`walk_exact`].
fn walk_sun_exact(spot: Spot, sun: Sun, occlusion: &Occlusion) -> (f32, Option<Stopper>) {
    let horizontal = (sun.toward.x * sun.toward.x + sun.toward.y * sun.toward.y).sqrt();
    if horizontal < 1e-6 {
        return (1.0, None);
    }
    let step = sun.toward.divided(horizontal).in_world_units();
    let mut tiles = MAX_SUN_TILES;
    if let (Some(ceiling), true) = (occlusion.tallest(), step.z > 1e-6) {
        tiles = tiles.min((ceiling as f32 - spot.z) / step.z);
    }
    if occlusion.tallest().is_none() || tiles <= 0.0 {
        return (1.0, None);
    }
    let from = WorldVec::new(spot.at.x, spot.at.y, spot.z);
    let to = WorldVec::new(
        from.x + step.x * tiles,
        from.y + step.y * tiles,
        from.z + step.z * tiles,
    );
    walk_the_record(from.array(), to.array(), LitEnd::of(spot), occlusion)
}

/// How wide the flame in a hand throws its light: the full angle, in degrees.
///
/// Sixty is a lamp rather than a searchlight — wide enough that walking is not
/// done down a tube, narrow enough that the direction the character is facing is
/// legible from the picture alone, which is the whole of what a carried light is
/// worth. It is a stand-in in exactly the way [`flame`] is: nothing on the wire
/// says a mobile is holding anything, so this is the client's own guess until
/// the equipment layers are read for a torch.
pub const HELD_BEAM_DEGREES: f32 = 60.0;

/// The flame the player carries: where it burns, which way it points, and how
/// it flickers.
///
/// Not a static and not a ground item, so no walk of the map could produce it —
/// [`Lighting::hold`] is how it gets into a frame. It is a [`TORCH`] in
/// everything but the [`Beam`], which is what makes the difference between a
/// character who glows and a character who is *carrying* something: an
/// omnidirectional pool centred on a body lights the wall behind it exactly as
/// brightly as the one it is walking towards, and the eye reads that as the
/// character being the source rather than the hand being it.
///
/// The axis is level with the ground and not tilted down at it. A torch aimed at
/// the floor two tiles ahead lights that floor beautifully and leaves the top of
/// every wall in front of it outside the cone — with a level axis the pool on the
/// ground is only a little shorter and a wall three tiles off is lit to nearly
/// its full height, which is the picture that says a beam has hit something.
///
/// `offset` is how far past `at`'s tile the body has already walked, in tile
/// units — the same lead [`crate::mobiles::billboard_offset`] carries for the
/// sprite's own lit position, and for the same reason. `at` only changes at
/// the far end of a step, four hundred milliseconds after it starts; a light
/// that read `at` alone would sit still for the whole crossing and then jump,
/// which is the flame following the tile the body left rather than the hand
/// carrying it. Zero for anything that does not walk — every other caller of
/// [`place`] plants a light exactly on its tile.
pub fn carried(at: Point, offset: Vec2, facing: Direction, time: f32) -> Light {
    let (dx, dy) = facing.step();
    let flame = place(at, TORCH, time);
    Light {
        at: Vec2::new(flame.at.x + offset.x, flame.at.y + offset.y),
        beam: Some(Beam::towards(dx as f32, dy as f32, 0.0, HELD_BEAM_DEGREES)),
        ..flame
    }
}

/// A flame's own place in the flicker, so that two torches on one wall do not
/// pulse in step.
///
/// Any spread-out function of the tile would do; this is the ordinary
/// multiply-and-mix, and what matters about it is only that it is deterministic
/// — the same tile flickers the same way in two clients watching one fire.
fn phase_of(at: Point) -> f32 {
    let mixed = u32::from(at.x)
        .wrapping_mul(73_856_093)
        .wrapping_add(u32::from(at.y).wrapping_mul(19_349_663))
        .wrapping_add((at.z as i32 as u32).wrapping_mul(83_492_791));
    // Into `0..2π`, out of the top bits: the low ones of a multiplicative mix
    // are the least stirred.
    (mixed >> 8) as f32 / (1 << 24) as f32 * std::f32::consts::TAU
}

/// The brightness multiplier a flame is at, at `time` seconds.
///
/// Two sines whose frequencies have no common period, so the pattern does not
/// repeat on anything an eye can catch — one sine reads as a pulse, which is
/// what a machine does and not what a fire does. The amplitudes sum to `depth`,
/// so a `depth` of `0.1` swings the brightness by at most a tenth either way
/// and the flame never gutters out.
fn flicker(time: f32, phase: f32, depth: f32) -> f32 {
    let slow = (time * 6.7 + phase).sin();
    let fast = (time * 11.3 + phase * 2.3).sin();
    1.0 + depth * (0.6 * slow + 0.4 * fast)
}

#[cfg(test)]
mod tests {
    use openshard_map::grid::BlockExtent;
    use openshard_map::map::LandCell;
    use openshard_protocol::items::ItemAmount;
    use openshard_protocol::wire::Hue;
    use openshard_tiles::{
        StaticTile,
        TileFlags,
    };

    use super::*;

    /// A tiledata table where exactly one graphic burns.
    fn lit(graphic: u16) -> TileData {
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            graphic,
            StaticTile {
                flags: TileFlags::new(TileFlags::LIGHT_SOURCE),
                ..StaticTile::default()
            },
        );
        tiledata
    }

    /// The default tuning is the client this had before there were knobs: the
    /// two spellings of the sun are one sun.
    ///
    /// [`SunTuning::MIDDAY`] states an azimuth and a slope, [`midday`] states a
    /// direction; nothing but this says they agree, and the day a time of day
    /// arrives on the wire it is what catches one of the two being updated.
    #[test]
    fn the_default_sun_tuning_is_midday() {
        assert_eq!(Tuning::DEFAULT.sun.sun(), midday());
    }

    /// A frame with nothing turned is the frame this drew before the knobs
    /// existed — every factor is the identity on the numbers it multiplies.
    #[test]
    fn the_default_tuning_changes_no_number() {
        let torch = Light {
            at:        Vec2::new(100.5, 100.5),
            z:         FLAME_LIFT,
            radius:    TORCH.radius,
            color:     TORCH.color,
            intensity: TORCH.intensity,
            beam:      None,
        };
        assert_eq!(Tuning::DEFAULT.applied(torch), torch);
        assert_eq!(Tuning::DEFAULT.applied_headlight(torch), torch);
        assert_eq!(Tuning::DEFAULT.ambient(NIGHT), NIGHT);
        assert_eq!(Tuning::DEFAULT.flame_radius, FLAME_RADIUS);
        assert_eq!(Tuning::DEFAULT.shadow_rays.count(), SHADOW_RAYS);
    }

    /// And a turned one is turned everywhere it is read: the flame's own two
    /// numbers, and both halves of the ambient.
    #[test]
    fn a_turned_tuning_reaches_the_flame_and_the_ambient() {
        let tuning = Tuning {
            brightness: 2.0,
            reach: 0.5,
            sky: 0.0,
            ground: 3.0,
            ..Tuning::DEFAULT
        };
        let torch = Light {
            at:        Vec2::new(100.5, 100.5),
            z:         FLAME_LIFT,
            radius:    6.0,
            color:     [1.0, 1.0, 1.0],
            intensity: 0.5,
            beam:      None,
        };
        let turned = tuning.applied(torch);
        assert_eq!(turned.radius, 3.0, "half the reach");
        assert_eq!(turned.intensity, 1.0, "twice the brightness");
        assert_eq!(turned.color, torch.color, "and nothing else moved");
        let ambient = tuning.ambient(Ambient {
            sky:    [0.4, 0.4, 0.4],
            ground: [0.1, 0.1, 0.1],
        });
        assert_eq!(ambient.sky, [0.0; 3], "no sky at all");
        assert_eq!(ambient.ground, [0.3, 0.3, 0.3], "three times the floor");
    }

    /// The lanterns, the headlight and the ambient are three separate dials:
    /// turning one leaves the other two exactly where [`Tuning::DEFAULT`] put
    /// them, because a person painting the street's lanterns has not asked to
    /// repaint their own torch or the sky.
    #[test]
    fn the_headlight_lantern_and_ambient_tints_are_independent() {
        let torch = Light {
            at:        Vec2::new(100.5, 100.5),
            z:         FLAME_LIFT,
            radius:    TORCH.radius,
            color:     [1.0, 1.0, 1.0],
            intensity: TORCH.intensity,
            beam:      None,
        };
        let tuning = Tuning {
            headlight_color: [0.0, 1.0, 0.0],
            lantern_color: [1.0, 0.0, 0.0],
            ambient_color: [0.0, 0.0, 1.0],
            ..Tuning::DEFAULT
        };
        assert_eq!(tuning.applied_headlight(torch).color, [0.0, 1.0, 0.0]);
        assert_eq!(tuning.applied(torch).color, [1.0, 0.0, 0.0]);
        let ambient = tuning.ambient(Ambient {
            sky:    [1.0, 1.0, 1.0],
            ground: [1.0, 1.0, 1.0],
        });
        assert_eq!(ambient.sky, [0.0, 0.0, 1.0]);
        assert_eq!(ambient.ground, [0.0, 0.0, 1.0]);
    }

    /// **The reach is what the grid's rectangle is grown by**, and that is the
    /// whole reason [`Tuning`] is threaded into [`collect`] rather than applied
    /// to what it returns.
    ///
    /// A pool twice as wide over the old margin is a flame that starts lighting
    /// the frame only once its own tile is nearly on screen, over a grid holding
    /// no walls that far out — light with no shadows, ending at a line that
    /// belongs to the camera and not to the world.
    #[test]
    fn a_wider_reach_grows_the_rectangle_the_grid_is_built_over() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let ordinary = lit_tiles(&camera, &Tuning::DEFAULT);
        let wide = lit_tiles(
            &camera,
            &Tuning {
                reach: 2.0,
                ..Tuning::DEFAULT
            },
        );
        assert!(
            wide.min_x < ordinary.min_x && wide.max_x > ordinary.max_x,
            "{wide:?} is not wider than {ordinary:?}",
        );
        // And exactly by the widest pool's own growth, which is what makes this a
        // rule rather than a nudge: nine tiles at the default, eighteen at twice.
        assert_eq!(ordinary.min_x - wide.min_x, CAMPFIRE.radius as i32);
        let narrow = lit_tiles(
            &camera,
            &Tuning {
                reach: 0.0,
                ..Tuning::DEFAULT
            },
        );
        assert_eq!(
            narrow.min_x,
            camera.visible_tiles().min_x - 1,
            "a pool of nothing still leaves the tile of rounding",
        );
    }

    /// A count is clamped at both ends, because both are a broken frame: zero
    /// rays divides by nothing, and more than the array holds is a walk reading
    /// points of a flame nobody sampled.
    #[test]
    fn a_ray_count_is_clamped_to_something_a_walk_survives() {
        assert_eq!(ShadowRays::new(0).count(), 1);
        assert_eq!(ShadowRays::new(9_000).raw(), ShadowRays::MOST);
        assert_eq!(ShadowRays::new(4).count(), 4);
        assert_eq!(ShadowRays::DEFAULT.count(), SHADOW_RAYS);
        assert_eq!(ShadowRays::new(32).for_radius(0.0), ShadowRays::ONE);
        assert_eq!(ShadowRays::new(32).for_radius(0.01).count(), 32);
    }

    /// The performance invariant behind [`ShadowRays::for_radius`], tested at
    /// the actual expensive boundary rather than only as arithmetic on a count:
    /// a point source asks the BVH once even when the frame requests 32 rays.
    #[test]
    fn a_point_source_walks_the_bvh_once() {
        let walks = std::cell::Cell::new(0usize);
        let spot = Spot::flat(Vec2::new(100.5, 100.5), 0.0, (100, 100));
        let light = Light {
            at:        Vec2::new(101.5, 100.5),
            z:         10.0,
            radius:    40.0,
            color:     [1.0; 3],
            intensity: 1.0,
            beam:      None,
        };
        let result = arrival(
            spot,
            &light,
            &Occlusion::EMPTY,
            0.0,
            ShadowRays::new(32),
            |_, _, _| {
                walks.set(walks.get() + 1);
                (0.5, None)
            },
        );

        assert_eq!(walks.get(), 1, "a point source repeated an identical BVH walk");
        assert_eq!(result.visible, 0.5, "the one ray was not averaged as itself");
    }

    /// And the points a flame is sampled at are as many as were asked for — the
    /// array is full either way, and the count is what says which of it is this
    /// flame's.
    #[test]
    fn a_flame_is_sampled_at_as_many_points_as_were_asked_for() {
        let spot = Spot::flat(Vec2::new(100.5, 100.5), 0.0, (100, 100));
        let flame = [103.5, 100.5, FLAME_LIFT];
        for rays in [1u32, 3, 8, ShadowRays::MOST] {
            let points = flame_points(spot, flame, FLAME_RADIUS, ShadowRays::new(rays));
            assert_eq!(points.count(), rays as usize);
            assert_eq!(points.iter().count(), rays as usize);
            assert_eq!(points.into_iter().count(), rays as usize);
        }
        // The spacing is stated in the count, so a shorter walk is a *pattern* of
        // its own and not the first few points of a longer one: every count fills
        // the same disc, out to `radius * sqrt((n - 0.5) / n)` at its widest.
        // Measured in **tile space**, which is the metric the sphere is round in
        // — a spread taken with `z` in its own units is an ellipse's, and reads
        // as two different flames for two counts of one.
        let spread = |points: FlamePoints| {
            points
                .iter()
                .map(|point| {
                    TileVec::between(WorldVec::from_array(flame), WorldVec::from_array(point)).length()
                })
                .fold(0.0_f32, f32::max)
        };
        for rays in [2u32, 4, 8, 16] {
            let widest = FLAME_RADIUS * ((rays as f32 - 0.5) / rays as f32).sqrt();
            let measured = spread(flame_points(spot, flame, FLAME_RADIUS, ShadowRays::new(rays)));
            // A rounding's worth of slack and no more: the two are the same
            // expression evaluated through a sine and a square root, so they
            // agree to a few `f32` bits rather than exactly.
            assert!(
                (measured - widest).abs() < 1e-5,
                "{rays} rays: {measured} is not {widest}",
            );
        }
    }

    /// A hand-edited file is an input, so every number has a door: out of range
    /// comes back at the edge of it, and a `NaN` comes back as the default of
    /// its own field rather than blackening the frame.
    #[test]
    fn hostile_numbers_are_clamped_rather_than_drawn() {
        let clamped = Tuning {
            flame_radius:    f32::NAN,
            shadow_rays:     ShadowRays::DEFAULT,
            brightness:      -1.0,
            reach:           1e9,
            sky:             f32::NAN,
            ground:          0.5,
            sun:             SunTuning {
                azimuth_degrees: 400.0,
                rise_per_tile:   -2.0,
                color:           [f32::NAN; 3],
                intensity:       99.0,
            },
            headlight_color: [-1.0, f32::NAN, 99.0],
            lantern_color:   [-1.0, f32::NAN, 99.0],
            ambient_color:   [-1.0, f32::NAN, 99.0],
        }
        .clamped();
        assert_eq!(clamped.flame_radius, FLAME_RADIUS);
        assert_eq!(clamped.brightness, 0.0);
        assert_eq!(clamped.reach, Tuning::MOST);
        assert_eq!(clamped.sky, 1.0);
        assert_eq!(clamped.ground, 0.5, "and what was already sane is untouched");
        assert_eq!(clamped.sun.azimuth_degrees, 40.0, "the compass wraps");
        assert_eq!(clamped.sun.rise_per_tile, 0.0);
        assert_eq!(clamped.sun.color, SunTuning::MIDDAY.color);
        assert_eq!(clamped.sun.intensity, Tuning::MOST);
        assert_eq!(clamped.headlight_color, [0.0, 1.0, Tuning::MOST]);
        assert_eq!(clamped.lantern_color, [0.0, 1.0, Tuning::MOST]);
        assert_eq!(clamped.ambient_color, [0.0, 1.0, Tuning::MOST]);
    }

    /// A map with ground and nothing standing on it: the statics in these tests
    /// come from the item list, which is the half a test can build without a
    /// client install.
    fn bare() -> WorldMap {
        WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| {
            LandCell {
                tile: openshard_tiles::LandTileId(0),
                z:    0,
            }
        })
    }

    /// A lid stops a ray that goes through it and nothing that runs along it.
    ///
    /// **The three cases `crosses` existed for, asked of the geometry that
    /// retired it** — `docs/render/design_frame_assembly.md`'s P4 step 1. They are the same three and
    /// they are the point of the whole change: a floor of the height a real one
    /// has (**zero** in `tiledata.mul`, so `bottom == top`) is a box spanning
    /// `19..20` now, and [`ray_vs_solid`] answers all three without being told
    /// anything about lids. A candle standing on a floor and the floor it lights
    /// are at one `z`, so the ray between them runs exactly along the top face;
    /// what used to be a strictness argued for in two shading languages is now
    /// the ordinary fact that a ray outside a box does not enter it.
    ///
    /// Read with `walk_primitives`'s own `entered == 0.0 && leaves == 0.0` gate,
    /// which is what turns the grazing case's zero-length touch into "went
    /// through nothing" for the walk as a whole.
    #[test]
    fn a_floor_stops_a_ray_through_it_and_not_one_along_it() {
        let floor = crate::occlusion::Solid::box_of(0, 0, 20, 20, Edges::NONE);
        assert_eq!(floor.max.z, 20.0, "a lid's top is the height it lies at");
        assert_eq!(
            floor.min.z,
            20.0 - crate::occlusion::LID_THICKNESS,
            "and it hangs its own invented depth under that",
        );

        // A ray from a wall pixel at 25 down to a torch at 5, straight through
        // the floor at 20: a real interval inside the box, however thin the box
        // is — which is the whole of what a span buys over a plane.
        let (entered, leaves) =
            ray_vs_solid([0.5, 0.5, 25.0], [0.5, 0.5, 5.0], &floor).expect("it goes through");
        assert!(entered < leaves, "{entered}..{leaves} is a crossing");
        // The same floor, and a flame standing on it: the ray runs along the top
        // face for its whole length, which the slab test reports as a hit — the
        // face is the closed box's own boundary. **What excuses it is
        // [`on_the_lit_surface`] and not a rule about lids**, on exactly the
        // terms it excuses a run of wall: the fragment is a point of a floor
        // whose own `max.z` is this number, the ray does not go below it, so
        // every primitive whose top is that plane lies wholly behind it.
        let along = ray_vs_solid([0.25, 0.25, 20.0], [0.75, 0.75, 20.0], &floor);
        assert_eq!(along, Some((0.0, 1.0)), "it runs along the face, not through");
        assert!(
            on_the_lit_surface(
                Surface::Flat,
                &crate::occlusion::Solid::box_of(1, 1, 20, 20, Edges::NONE),
                &floor,
                [0.5, 0.5, 0.0],
            ),
            "the floor beside it shares that top plane and is exempt",
        );
        // And a ray wholly above it — a lamp on the upper storey lighting the
        // upper storey — misses the floor under both of them outright.
        assert_eq!(ray_vs_solid([0.5, 0.5, 25.0], [0.5, 0.5, 23.0], &floor), None);
    }

    /// A one-tile-square, fully opaque panel or body, for the small pure
    /// helpers below — the four numbers a scene is actually about, the same
    /// way `occlusion.rs`'s own `stands_at` is, but built directly since that
    /// one is private to `occlusion`'s own test module.
    fn test_solid(bottom: i32, top: i32, edges: Edges) -> crate::occlusion::Solid {
        crate::occlusion::Solid {
            space: crate::solid::Solid {
                min: crate::camera::WorldSpot {
                    x: 0.0,
                    y: 0.0,
                    z: f64::from(bottom),
                },
                max: crate::camera::WorldSpot {
                    x: 1.0,
                    y: 1.0,
                    z: f64::from(top),
                },
            },
            opacity: 255,
            edges,
            aperture: None,
            roof: false,
            owner: crate::occlusion::Owner::new(bottom as i8, openshard_protocol::wire::Graphic(0)),
            part: crate::occlusion::Part::ONLY,
        }
    }

    // **Three tests of the analytic penumbra stood here** and went with it at
    // `docs/render/design_model.md` phase 5, along with the two functions they were
    // about. `inside_is_full_at_the_middle_and_half_at_each_edge` and
    // `inside_is_clamped_and_symmetric_about_the_intervals_centre` were `inside`'s
    // band, and `pierces_centres_its_band_on_the_top_edge_only` was the one
    // asymmetry that made `pierces` a second function rather than a call of the
    // first — a wall's bottom edge is the ground it stands on and the ray a person
    // looks at runs along it, so that band hung below rather than straddling.
    // Their subject was a band's own shape, and there is no band; the rule from
    // *How this is judged* is the one that retired them.

    /// Which axis [`along_the_run`] reads is the edge mask, and what it returns
    /// is the coordinate **itself**.
    ///
    /// It took `along - along.floor()` until `docs/render/design_occluders.md`'s S6, and the
    /// case that made the `floor` a deliberate spelling rather than
    /// [`f32::fract`] is still here as the third pair: a wall running through
    /// negative world space is a real scene, `(-3.25).fract()` is `-0.25` and
    /// the run fraction of it was `0.75`. Neither number is asked for now — a
    /// hole's own ends are world coordinates, so both sides of the comparison
    /// are negative together and the sign takes care of itself.
    #[test]
    fn along_the_run_reads_the_axis_the_edges_name() {
        assert_eq!(along_the_run(crate::occlusion::Edges::NORTH, 3.75, 9.25), 3.75);
        assert_eq!(along_the_run(crate::occlusion::Edges::EAST, 3.75, 9.25), 9.25);
        assert_eq!(along_the_run(crate::occlusion::Edges::NORTH, -3.25, 0.0), -3.25);
    }

    /// [`hole`]'s two claims: nothing without an aperture, and — with one — the
    /// rectangle it documents, checked at a point deep in both spans and at two
    /// points each outside one of them.
    ///
    /// The rectangle is a *hard* one since phase 5, so the three points that were
    /// chosen to be well clear of a band are now merely inside and outside, and
    /// the tolerances they were read to are exact equalities. Since S6 the run
    /// pair is stated where the panel stands — the tile at `x = 105` here — and
    /// the point asked about is a world coordinate rather than a fraction.
    #[test]
    fn hole_is_zero_with_no_aperture_and_the_rectangle_with_one() {
        assert_eq!(hole(None, 105.5, 10.0), 0.0);

        let aperture = window_at(105);
        assert_eq!(hole(Some(aperture), 105.5, 10.0), 1.0);
        assert_eq!(hole(Some(aperture), 105.9, 10.0), 0.0);
        assert_eq!(hole(Some(aperture), 105.5, 30.0), 0.0);
    }

    /// The middle half of the tile at `along`, open from `z` 5 to 15 — the
    /// aperture the two tests below ask about, placed the way [`Builder::add`]
    /// places one.
    fn window_at(along: i32) -> crate::occlusion::Aperture {
        crate::occlusion::Aperture::placed(
            0,
            along,
            crate::facing::Hole {
                near:   64,
                far:    191,
                bottom: 5,
                top:    15,
            },
        )
    }

    /// [`pierced`] with no aperture is the whole surface — a ray that reached it
    /// has already crossed the box the panel is — and with one, a point deep
    /// inside the hole is open while the same height beside the hole is still
    /// stopped by the wall around it.
    #[test]
    fn pierced_is_the_whole_surface_with_no_hole_and_open_where_the_hole_is() {
        let wall = test_solid(0, 20, crate::occlusion::Edges::NORTH);
        assert_eq!(pierced(&wall, [105.5, 0.0, 10.0]), 1.0);

        let mut windowed = wall;
        windowed.aperture = Some(window_at(105));
        assert_eq!(pierced(&windowed, [105.5, 0.0, 10.0]), 0.0);
        assert_eq!(pierced(&windowed, [105.9, 0.0, 10.0]), 1.0);
    }

    /// **A window is one window, wherever along the panel the ray crosses** —
    /// `docs/render/design_occluders.md`'s S6, and the gate on the rule that step is.
    ///
    /// Two claims, and neither is expressible while a hole is a fraction of the
    /// tile a *crossing* landed in:
    ///
    /// - **A panel wider than one tile has one hole and not one per tile.** D1
    ///   made such a primitive expressible, `facing::Blocks` will author one and
    ///   the merge would build one the day two windowed pieces could fold; under
    ///   `along - along.floor()` this panel is a wall with a window in every
    ///   tile of itself, which is light through three tiles of stone.
    /// - **A crossing exactly on a tile boundary belongs to the hole that
    ///   reaches it.** `floor` sends such a point into the *next* tile, so a
    ///   window running to the far end of its own tile read as a window at the
    ///   near end of the one beyond — § *The oracle*'s own defect, which is a
    ///   `floor` landing on the wrong side of a whole coordinate, one level up.
    ///
    /// The coordinates are exact in `f32` on purpose: whole tiles and quarters,
    /// so what the assertions read is the rule and not a rounding.
    #[test]
    fn a_windowed_panel_wider_than_a_tile_has_one_window() {
        let mut run = test_solid(0, 20, crate::occlusion::Edges::NORTH);
        // Three tiles of one wall, `x` from 105 to 108 — the shape a merge of
        // three panels makes, and the shape a `Blocks` list can author.
        run.space.min.x = 105.0;
        run.space.max.x = 108.0;
        // A window in the first tile of it, from a quarter of the way along to
        // the far end of that tile.
        run.aperture = Some(crate::occlusion::Aperture {
            near:   105.25,
            far:    106.0,
            bottom: 5,
            top:    15,
        });

        assert_eq!(pierced(&run, [105.5, 0.0, 10.0]), 0.0, "the window is open");
        assert_eq!(
            pierced(&run, [106.0, 0.0, 10.0]),
            0.0,
            "and it is open at its own far end, which is a whole coordinate",
        );
        for along in [106.5, 107.25, 107.5] {
            assert_eq!(
                pierced(&run, [along, 0.0, 10.0]),
                1.0,
                "a second tile of this run is wall, and {along} is a point of it",
            );
        }
    }

    // **`same_run_keeps_only_the_sides_on_the_same_row_or_column_as_the_start`
    // lived here** and went with the function it was exhaustive over —
    // `docs/render/design_occluders.md`'s S4. It was the only thing in the crate that went red
    // when `same_run` was neutralised, which is what said the rule had no case of
    // its own left; see the note at its grave above `lit_plane`.

    // **`stand_clear_nudges_only_along_a_faces_own_outward_normal` lived here**
    // and went with the nudge it was about: `docs/render/design_model.md` phase 4
    // took the bias to zero, and a test whose whole subject is which axis a
    // constant moves a ray along does not survive the constant.

    // **`on_surface_is_inclusive_of_both_ends_and_exact` lived here** and went
    // with `on_surface`, `docs/render/design_occluders.md`'s S4. Its whole subject was that
    // predicate's two ends, and there is no predicate left to be inclusive: what
    // asks whether a fragment belongs to a primitive is now the primitive's own
    // name (`Spot::part_of`) and the plane test beside it.

    /// A primitive's `z` span is the caller's own, fraction included —
    /// `docs/archive/render/lighting_height.md` phase 2's whole point on this side.
    ///
    /// A box based half a unit up is the case the plan's control scene
    /// (`OPENSHARD_TREE_H1=3.5`) is made of: rounded to `4`, the bottom half
    /// unit of the box's own faces reads as *below* the box, and every rule that
    /// asks whether a fragment belongs to the thing it was drawn from answers
    /// no for it.
    ///
    /// It read the span through `on_surface` until S4 deleted that predicate.
    /// The claim never was the predicate's: it is that `Solid::low`/`high` and
    /// [`wire_span`] both answer `3.5` where `bottom()`'s rounding answers `4`,
    /// and both halves are asserted below, exactly.
    #[test]
    fn a_primitives_span_is_its_own_fraction_and_not_a_rounded_one() {
        let box_at_half = crate::occlusion::Solid {
            space:    crate::solid::Solid {
                min: crate::camera::WorldSpot {
                    x: 5.0,
                    y: 5.0,
                    z: 3.5,
                },
                max: crate::camera::WorldSpot {
                    x: 6.0,
                    y: 6.0,
                    z: 6.5,
                },
            },
            opacity:  255,
            edges:    crate::occlusion::Edges::ANY,
            aperture: None,
            roof:     false,
            owner:    crate::occlusion::Owner::new(3, openshard_protocol::wire::Graphic(0)),
            part:     crate::occlusion::Part::ONLY,
        };
        assert_eq!(
            (box_at_half.low(), box_at_half.high()),
            (3.5, 6.5),
            "the span is rounded, so a fragment a tenth of a unit up the box's own \
             face reads as below it — which is what `bottom()`'s own `4` cannot tell \
             apart from a fragment that really is",
        );
        // And the same span off the wire, which is what the GPU reads. A half
        // is a whole number of `Solid::Z_STEPS`, so this is exact rather than
        // near — the step being a power of two is what buys that.
        // `wire_span` was this said as its own function, and it went with
        // `walk_the_wire`'s own body at `docs/render/design_occluders.md`'s S5: the
        // walk reads the wire's whole box now and takes its `z` off that,
        // which is the same two numbers with nothing between them.
        let wire = box_at_half.wire_box();
        assert_eq!((wire.min.z as f32, wire.max.z as f32), (3.5, 6.5));
    }

    // **`a_fragment_is_exempt_from_its_own_solid_and_from_a_twin_of_it_beside_it`
    // lived here**, and it went with the `exemption` function it called.
    //
    // Its subject had shrunk to `Some(id) == Some(id)`. What it was *for* was the
    // scene: two solids on one tile, identical in every geometric fact a height
    // test could read, which is `examples/boxes.rs`'s `pair` and which read
    // 1296/1296, 1248/1248 and 9216/9216 fully wrong before identity existed at
    // all. That scene is still measured, by that oracle; and the two claims the
    // test made that the *walk* can still be asked — a fragment is not shadowed by
    // its own solid, and a fragment that is a point of nothing is shadowed by
    // everything — are the flight test above, whose last ray is the second of
    // them.
    //
    // What it also asserted, and what is worth keeping in words: an
    // `crate::occlusion::OwnerId` was unique within a *cell*, so every arm that
    // read one had to be gated on the solid standing on the fragment's own cell, and
    // the test said so. A `SolidId` names one solid in the whole frame, and a solid
    // whose box crosses a tile boundary is referenced from every cell it touches —
    // a fragment of it is a point of it on all of them, so the gate is not merely
    // unnecessary but wrong.

    /// Every authored light value is exactly `srgb_to_linear` of the number a
    /// person chose, and the numbers a person chose are in this test.
    ///
    /// `docs/render/design_model.md` phase 1 moved the multiplication into linear
    /// radiance, which silently changed what every one of these constants means:
    /// `0.20` of a displayed value is a dark street and `0.20` of radiance is an
    /// overcast afternoon. Converting them is not a tweak and must not read like
    /// one — so the artistic intent stays written down here, beside the
    /// conversion, and a constant nudged by hand to make a picture look right
    /// turns this red rather than quietly redefining what "night" was.
    ///
    /// Intensities above `1.0` are outside sRGB's domain and carry the curve's
    /// exponent alone; the campfire is the only one, and it is checked the same
    /// way.
    #[test]
    fn the_authored_light_values_are_their_own_srgb_intent() {
        let linear = crate::tonemap::srgb_to_linear;
        let same = |got: f32, authored: f32| {
            let want = linear(authored);
            assert!(
                (got - want).abs() < 5e-6,
                "authored {authored} is {want} in linear light, the constant says {got}",
            );
        };
        let all = |got: [f32; 3], authored: [f32; 3]| {
            for (got, authored) in got.into_iter().zip(authored) {
                same(got, authored);
            }
        };
        all(GROUND_AMBIENT, [0.12, 0.13, 0.18]);
        all(NIGHT.sky, [0.20, 0.22, 0.31]);
        all(NIGHT.ground, [0.10, 0.11, 0.14]);
        all(SKYLIGHT.sky, [0.43, 0.42, 0.44]);
        all(TORCH.color, [1.0, 0.72, 0.36]);
        same(TORCH.intensity, 0.95);
        all(CAMPFIRE.color, [1.0, 0.66, 0.30]);
        // Past sRGB's domain: the exponent alone.
        assert!((CAMPFIRE.intensity - 1.25_f32.powf(2.4)).abs() < 5e-6);
        let sun = midday();
        all(sun.color, [1.0, 0.97, 0.88]);
        same(sun.intensity, 0.55);
    }

    /// [`TileVec`]'s two crossings are one conversion, and it is the one the
    /// whole space is defined by.
    ///
    /// `docs/render/design_pixel_spaces.md` P3's gate. The type exists to make world units and tile
    /// space inexpressible in each other's place, and what makes that safe rather
    /// than merely tidy is that the pair round-trips: a mutation that dropped
    /// `Z_PER_TILE` from either method, or applied it the wrong way round, would
    /// leave one of these two assertions standing and not the other.
    #[test]
    fn tile_space_and_world_units_are_one_conversion_apart() {
        // Eleven `z` units are one tile, which is the whole of the space: a
        // vector eleven up and one along is at 45° here and nowhere else.
        let up = TileVec::between(WorldVec::new(0.0, 0.0, 0.0), WorldVec::new(0.0, 0.0, Z_PER_TILE));
        assert_eq!(up.z, 1.0, "eleven `z` units are not one tile: {up:?}");
        let diagonal = TileVec::between(WorldVec::new(0.0, 0.0, 0.0), WorldVec::new(1.0, 0.0, Z_PER_TILE));
        assert_eq!(
            diagonal.length(),
            2.0_f32.sqrt(),
            "a tile along and a tile up is not a 45° vector: {diagonal:?}",
        );
        // And back out again, unchanged. The `z` term is the one that moves, so
        // a point with a `z` of nothing would pass either way round.
        let far = WorldVec::new(1500.0, 1600.0, 37.0);
        let back = TileVec::between(WorldVec::new(0.0, 0.0, 0.0), far).in_world_units();
        let (back, far) = (back.array(), far.array());
        for axis in 0..3 {
            assert!(
                (back[axis] - far[axis]).abs() < 1e-3,
                "axis {axis} did not survive the round trip: {back:?} against {far:?}",
            );
        }
    }

    /// [`lit_from`]'s own gradient: fully towards the light, fully away, and
    /// exactly edge-on in between — the cosine's three named points.
    ///
    /// Edge-on is `0.0` and not a half, which is the whole of "no wrap": a
    /// stylised term would put its floor here, and a surface the flame lies in
    /// the plane of would still be lit.
    #[test]
    fn lit_from_is_one_towards_the_light_and_zero_away_from_it() {
        let toward = TileVec::new(1.0, 0.0, 0.0);
        assert_eq!(lit_from(TileVec::new(1.0, 0.0, 0.0), toward), 1.0);
        assert_eq!(lit_from(TileVec::new(-1.0, 0.0, 0.0), toward), 0.0);
        assert_eq!(lit_from(TileVec::new(0.0, 1.0, 0.0), toward), 0.0);
        // And the curve between them is the cosine itself, not a straight line
        // between the three: sixty degrees off is a half, which is the one point
        // that tells a cosine from a ramp.
        let sixty = lit_from(
            TileVec::new(1.0, 0.0, 0.0),
            TileVec::new(0.5, 0.75_f32.sqrt(), 0.0),
        );
        assert!((sixty - 0.5).abs() < 1e-6, "sixty degrees off came out {sixty}");
    }

    /// **The whole of what phase 3 changed**: the term is an angle and no longer
    /// a distance.
    ///
    /// The same direction at four lengths — a tenth of a tile away and ten tiles
    /// away — used to give four different answers, because the expression divided
    /// `dot(normal, offset)` by a constant *in tiles*. That is where a single
    /// number came to mean ±4 screen pixels across a wall and ±1.1 `z` above a
    /// lid. It is one answer now, and this is what would fail if the
    /// normalisation were dropped again.
    #[test]
    fn the_shading_term_is_an_angle_and_not_a_distance() {
        // Well off the axis, so the cosine is neither zero nor one and a bug that
        // clamped would not be able to hide in the saturated end of the curve.
        let direction = TileVec::new(0.6, 0.8, 0.0);
        let normal = TileVec::new(1.0, 0.0, 0.0);
        let near = lit_from(normal, direction.scaled(0.1));
        for scale in [1.0_f32, 3.0, 10.0] {
            let far = lit_from(normal, direction.scaled(scale));
            assert!(
                (far - near).abs() < 1e-6,
                "the same direction {scale} times further away answered {far} against {near}"
            );
        }
        assert!(
            near > 0.0 && near < 1.0,
            "a saturated {near} would agree with a distance too"
        );
    }

    /// **The cull is conservative, and the silhouette is why that costs nothing
    /// today** — the one term `docs/render/design_model.md`'s phase 5b left at the
    /// flame's centre, and a correction to what that phase expected of it.
    ///
    /// The cull is a **broad phase**: it decides which flames to walk rays for,
    /// and a broad phase that changes the answer is a defect rather than an
    /// optimisation. So it culls on the near side of the body —
    /// `distance - flame_radius >= reach` — because a *sphere* centred a hair past
    /// a spot's reach still has half of itself inside it.
    ///
    /// **And no sample of it is ever nearer than its centre**, which is the first
    /// half of this test and the reason the conservatism currently moves no pixel.
    /// [`flame_points`] samples the disc the sphere *presents* to the spot — the
    /// silhouette, which is the set a receiver can be occluded from — and every
    /// point of that disc is `sqrt(d² + r²)` away. Phase 5b predicted that
    /// tightening the cull to `distance >= reach` would move "pixels at the rim of
    /// every pool"; it moves none, and this is why.
    ///
    /// So the rule is a guard rather than a behaviour, and it is a guard with a
    /// gate: the day a sampler reaches for the *volume* instead of the silhouette,
    /// the first half of this goes red and the conservative form starts earning
    /// its keep. Deleting it and keeping the lemma would be the same picture with
    /// nothing standing between a future sampler and a pool with a bite out of its
    /// rim.
    #[test]
    fn the_cull_is_conservative_and_no_sample_is_nearer_than_the_flames_centre() {
        // Every direction that matters, at three distances: along each axis,
        // diagonally, and steeply up — the branch in `flame_points`'s own basis is
        // on `normal.x`, so a sweep that never crossed it would test one arm.
        for direction in [
            TileVec::new(1.0, 0.0, 0.0),
            TileVec::new(0.0, 1.0, 0.0),
            TileVec::new(0.0, 0.0, 1.0),
            TileVec::new(0.6, 0.8, 0.0),
            TileVec::new(0.3, 0.2, 0.93),
        ] {
            for span in [0.2_f32, 1.0, 7.0] {
                let spot = Spot::at(Vec2::new(100.0, 100.0), 0.0, (100, 100));
                let offset = direction.scaled(span).in_world_units();
                let flame = [100.0 + offset.x, 100.0 + offset.y, offset.z];
                let centre = span * direction.length();
                for point in flame_points(spot, flame, FLAME_RADIUS, ShadowRays::DEFAULT) {
                    let distance =
                        TileVec::between(WorldVec::new(100.0, 100.0, 0.0), WorldVec::from_array(point))
                            .length();
                    assert!(
                        distance >= centre - 1e-5,
                        "a sample at {distance} is nearer than the flame's own centre at {centre}: \
                         the disc is no longer a silhouette, and the cull's conservatism has become \
                         load-bearing"
                    );
                }
            }
        }

        let reach = 1.0_f32;
        let lighting = Lighting {
            ambient:      Ambient {
                sky:    [0.0; 3],
                ground: [0.0; 3],
            },
            lights:       vec![Light {
                at:        Vec2::new(100.5, 100.5),
                z:         0.0,
                radius:    reach,
                color:     [1.0, 1.0, 1.0],
                intensity: 1.0,
                beam:      None,
            }],
            occlusion:    Occlusion::EMPTY,
            sun:          None,
            view:         crate::debug::View::Lit,
            flame_radius: FLAME_RADIUS,
            shadow_rays:  ShadowRays::DEFAULT,
            dead:         false,
        };
        // Along `x` and at the flame's own height, so the distance is the offset
        // and nothing has to be derived. `Spot::at` has no facing, so the cosine
        // is one everywhere and what is being read is the falloff alone.
        let brightness = |east: f32| {
            let at = Vec2::new(100.5 + east, 100.5);
            sample(Spot::at(at, 0.0, (100, 100)), &lighting).brightness()
        };
        let inside = brightness(reach - 0.01);
        let near_edge = brightness(reach + FLAME_RADIUS / 2.0);
        assert!(
            inside > 0.0,
            "a spot inside the centre's own reach is dark: {inside}"
        );
        // And the consequence of the lemma above, stated as the picture: the rays
        // the conservative cull keeps alive all land past the reach, so they
        // deliver exactly nothing. The two culls draw one frame.
        assert_eq!(
            near_edge, 0.0,
            "a spot past the flame's centre but inside its near edge came out lit: some sample is \
             nearer than the centre after all"
        );
    }

    /// The identity is exactly that: the blit has a case where it must not touch
    /// a single byte, and this is what says so.
    #[test]
    fn the_empty_lighting_is_the_identity() {
        assert!(Lighting::NONE.is_identity());
        assert!(
            Lighting {
                ambient: Ambient::DAY.flattened(),
                ..Lighting::NONE
            }
            .is_ambient_only()
        );
        assert!(Ambient::DAY.is_full_daylight());
        assert!(Ambient::DAY.flattened().is_full_daylight());
        assert!(!NIGHT.is_full_daylight());
        assert!(
            !Lighting {
                ambient: NIGHT,
                ..Lighting::NONE
            }
            .is_identity()
        );
        assert!(
            !Lighting {
                sun: Some(midday()),
                ..Lighting::NONE
            }
            .is_identity()
        );
        assert!(
            !Lighting {
                sun: Some(midday()),
                ..Lighting::NONE
            }
            .is_ambient_only()
        );
    }

    /// A dropped torch lights the tile it is on: the pool's centre is where the
    /// camera puts that tile, lifted to where the flame is rather than left on
    /// the ground.
    #[test]
    fn a_lit_item_makes_a_light_over_its_own_tile() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0A12);
        let items = [GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(100, 100, 0),
            graphic,
            hue: Hue::NONE,
        }];
        let lighting = collect(
            &bare(),
            &items,
            &camera,
            &lit(graphic.0),
            &Cutaway::OPEN,
            NIGHT,
            &Tuning::DEFAULT,
            0.0,
            None,
            None,
        );
        assert_eq!(lighting.lights.len(), 1);
        let light = lighting.lights[0];
        assert_eq!(
            (light.at.x, light.at.y),
            (100.5, 100.5),
            "the middle of its own tile"
        );
        assert_eq!(light.z, FLAME_LIFT, "burning above the ground it stands on");
        assert_eq!(light.radius, TORCH.radius, "six tiles, whatever the zoom");
    }

    /// And an item that is not flagged makes none. The flag is the whole test:
    /// a barrel next to a torch must not glow.
    #[test]
    fn an_unflagged_item_makes_no_light() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let items = [GroundItem {
            amount:  ItemAmount::ONE,
            at:      Point::new(100, 100, 0),
            graphic: Graphic(0x0FAE),
            hue:     Hue::NONE,
        }];
        let lighting = collect(
            &bare(),
            &items,
            &camera,
            // Flagged, but a *different* graphic.
            &lit(0x0A12),
            &Cutaway::OPEN,
            NIGHT,
            &Tuning::DEFAULT,
            0.0,
            None,
            None,
        );
        assert!(lighting.lights.is_empty());
    }

    /// A pool covers the same ground at every zoom, and now says so by not
    /// changing at all.
    ///
    /// The bug this was written against — a torch lighting six tiles at 1:1 and
    /// one and a half at 4x — is unexpressible once a reach is in tiles rather
    /// than in pixels of an image whose scale is the zoom. It stays because
    /// "unexpressible" is a claim about the code and this is the thing that
    /// checks it: `collect` walks a camera, and a camera is what used to be
    /// folded into the number.
    #[test]
    fn a_pool_covers_the_same_ground_at_every_zoom() {
        let graphic = Graphic(0x0A12);
        let items = [GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(100, 100, 0),
            graphic,
            hue: Hue::NONE,
        }];
        let tiledata = lit(graphic.0);
        let mut camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let mut zoom = camera.zoom();
        loop {
            camera.zoom_about(crate::camera::RealPixel::new(400, 300), zoom);
            let lighting = collect(
                &bare(),
                &items,
                &camera,
                &tiledata,
                &Cutaway::OPEN,
                NIGHT,
                &Tuning::DEFAULT,
                0.0,
                None,
                None,
            );
            assert_eq!(lighting.lights[0].radius, TORCH.radius, "at {zoom}");
            assert_eq!(lighting.lights[0].at, Vec2::new(100.5, 100.5), "at {zoom}");
            let next = zoom.scale_up();
            if next == zoom {
                break;
            }
            zoom = next;
        }
    }

    /// The flicker stays inside the band its depth promises, and two flames on
    /// two tiles are not at the same point of it.
    #[test]
    fn the_flicker_is_bounded_and_out_of_step() {
        let phase = phase_of(Point::new(100, 100, 0));
        let other = phase_of(Point::new(101, 100, 0));
        assert!((phase - other).abs() > 0.01, "two tiles flicker together");
        for step in 0..2_000 {
            let time = step as f32 * 0.017;
            let value = flicker(time, phase, 0.1);
            assert!((0.9..=1.1).contains(&value), "{value} at {time}");
        }
    }

    /// Every tile a pool could reach the frame from is walked.
    ///
    /// The bug this is written against, and it is the one a screenshot shows:
    /// walked with the *drawing* bounds, a lamp's light vanished the moment the
    /// lamp itself left the screen, so every edge of the frame popped as the
    /// camera panned — worst at the widest zoom, where a frame holds more edges
    /// of more pools. On Britain, 88 light sources stood in the band that was
    /// being skipped.
    ///
    /// Stated as the implication rather than as a margin in tiles: *if* a flame
    /// placed on a tile would light the image, *then* the walk has to visit that
    /// tile. That is checkable without a map, at every zoom, and it stays true
    /// if a wider flame is added later — which a constant compared against a
    /// constant would not.
    #[test]
    fn every_flame_that_can_reach_the_frame_is_walked() {
        let widest = Graphic(*CAMPFIRE_GRAPHICS.start());
        assert_eq!(flame(widest).radius, CAMPFIRE.radius, "the widest pool moved");
        let mut camera = Camera::new(Point::new(500, 500, 0), 800, 600);
        let mut zoom = camera.zoom();
        while !zoom.is_widest() {
            zoom = zoom.scale_down();
        }
        loop {
            camera.zoom_about(crate::camera::RealPixel::new(400, 300), zoom);
            let bounds = lit_tiles(&camera, &Tuning::DEFAULT);
            let drawn = camera.visible_tiles();

            let mut reaching = 0;
            for x in drawn.min_x - 40..=drawn.max_x + 40 {
                for y in drawn.min_y - 40..=drawn.max_y + 40 {
                    // Could a campfire on this tile light any tile the frame
                    // draws? In tiles now, which is the unit the reach is in —
                    // the nearest drawn tile is the one to ask about.
                    let near_x = x.clamp(drawn.min_x, drawn.max_x);
                    let near_y = y.clamp(drawn.min_y, drawn.max_y);
                    let (dx, dy) = ((x - near_x) as f32, (y - near_y) as f32);
                    if (dx * dx + dy * dy).sqrt() >= CAMPFIRE.radius {
                        continue;
                    }
                    reaching += 1;
                    assert!(
                        x >= bounds.min_x && x <= bounds.max_x && y >= bounds.min_y && y <= bounds.max_y,
                        "at {zoom}, a flame on ({x}, {y}) lights the frame and is never walked",
                    );
                }
            }
            // A sweep that found nothing would assert nothing at all, and would
            // stay green for a `lit_tiles` that returned an empty rectangle.
            assert!(
                reaching > 500,
                "at {zoom}, only {reaching} tiles could light the frame"
            );

            let next = zoom.scale_up();
            if next == zoom {
                break;
            }
            zoom = next;
        }
    }

    /// The occluders come back over the same cells the flames were looked for
    /// on, and a wall on one of them is in the grid.
    ///
    /// One rectangle and not two: a grid collected over a smaller region than
    /// the flames were would let a torch light through a wall that is on screen,
    /// and the two walks are written as one call for exactly that reason.
    #[test]
    fn the_occluders_cover_the_cells_the_flames_came_from() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0006);
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            graphic.0,
            StaticTile {
                flags: TileFlags::new(TileFlags::NO_SHOOT),
                height: 20,
                ..StaticTile::default()
            },
        );
        let items = [GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(101, 100, 0),
            graphic,
            hue: Hue::NONE,
        }];
        let lighting = collect(
            &bare(),
            &items,
            &camera,
            &tiledata,
            &Cutaway::OPEN,
            NIGHT,
            &Tuning::DEFAULT,
            0.0,
            None,
            None,
        );
        assert_eq!(lighting.occlusion.bounds(), lit_tiles(&camera, &Tuning::DEFAULT));
        assert!(
            lighting.occlusion.at(101, 100).is_some(),
            "the wall the frame walked past is not in the grid",
        );
    }

    /// The grid a frame uploads stays small enough to upload every frame.
    ///
    /// It is the one *unconditional* cost this pass added: the lights are
    /// walked from the map either way, but the occluders become a texture that
    /// goes to the GPU on every frame whether anything burns or not. Four bytes
    /// a tile over the widest zoom's rectangle, and the number is asserted
    /// rather than assumed because it is the whole of the answer to "does this
    /// cost anything" — a rectangle that grew with the map instead of with the
    /// viewport would be megabytes and nobody would notice until a shard with a
    /// big facet ran it. Measured: 187x187 tiles at the widest zoom on a
    /// 1920x1080 viewport, which is 140KB a frame.
    #[test]
    fn the_grid_a_frame_uploads_is_a_few_tiles_across_and_not_a_map() {
        let mut camera = Camera::new(Point::new(500, 500, 0), 1920, 1080);
        let mut zoom = camera.zoom();
        while !zoom.is_widest() {
            zoom = zoom.scale_down();
        }
        camera.zoom_about(crate::camera::RealPixel::new(960, 540), zoom);
        let bounds = lit_tiles(&camera, &Tuning::DEFAULT);
        let bytes = bounds.width() * bounds.height() * 4;
        assert!(
            bytes < 512 * 1024,
            "the occlusion grid is {}x{} tiles, {bytes} bytes a frame",
            bounds.width(),
            bounds.height(),
        );
    }

    /// A flame the cutaway has taken away takes its light with it: the roof over
    /// the player hides the brazier on it, and a glow with no fire under it is
    /// worse than no glow.
    #[test]
    fn a_hidden_flame_does_not_light() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0A12);
        let items = [GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(100, 100, 40),
            graphic,
            hue: Hue::NONE,
        }];
        let tiledata = lit(graphic.0);
        let lighting = collect(
            &bare(),
            &items,
            &camera,
            &tiledata,
            // Everything at or above z = 20 is cut away.
            &Cutaway {
                max_z: 20,
                ..Cutaway::OPEN
            },
            NIGHT,
            &Tuning::DEFAULT,
            0.0,
            None,
            None,
        );
        assert!(lighting.lights.is_empty());
    }

    /// **A tread's own top must not be shadowed by the tread it is the top of.**
    ///
    /// Found looking at a real staircase render: every tread top read dark
    /// towards its own rise regardless of where the torch stood. The rule that
    /// did it was [`Surface::shadowed_by_own_tile`], written for a room's
    /// floor — "a floor pixel on a wall tile is inside the room, and the ray
    /// from it to a lamp in the street crosses the panel its own tile stands
    /// on" — and a tread's top carries the same flat normal a floor does, so a
    /// surface of the tread's own body read as a wall standing between it and
    /// the light.
    ///
    /// It named a *riser* while a tread was two solids, a lid and a plane; a
    /// tread is one body now (`occlusion::Builder::add`'s climbable branch) and
    /// the claim survived that change **unedited**, which is worth recording:
    /// what it is about is identity — a fragment is not shadowed by the solid it
    /// is a point of — and identity does not care how the shape was cut.
    #[test]
    fn a_treads_top_is_not_shadowed_by_the_tread_it_is_the_top_of() {
        use crate::facing::Prism;
        use crate::occlusion::{
            Builder,
            Shape,
        };

        let stair = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT | TileFlags::CLIMBABLE),
            height: 20,
            ..StaticTile::default()
        };
        let prism = Prism::new(Face::West, &[1, 3, 5]).expect("three treads");
        let mut occlusion = Builder::new(crate::camera::TileBounds {
            min_x: 95,
            max_x: 105,
            min_y: 95,
            max_y: 105,
        });
        occlusion.add(100, 100, 0, Graphic(0x0736), &stair, Shape::solid(prism));
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        // The highest tread's own top, off the built grid rather than
        // recomputed: `footprint`'s own fractions are not this test's subject.
        // Named, not merely found: a fragment of it is a point of *that* solid,
        // which is what phase 4 compares — see `Spot::part_of`.
        let (id, top) = occlusion
            .cell(100, 100)
            .max_by_key(|(_, solid)| solid.top())
            .expect("the climb built three treads");
        let at = Vec2::new(
            ((top.space.min.x + top.space.max.x) / 2.0) as f32,
            ((top.space.min.y + top.space.max.y) / 2.0) as f32,
        );
        let spot = Spot::flat(at, top.top() as f32, (100, 100)).part_of(id);

        // East of the stair, over the top tread's own height by [`FLAME_LIFT`] —
        // a torch standing at the foot of the flight, which is where a person
        // actually stands one, and *not* one whose flame is exactly level with the
        // tread.
        //
        // **The level flame is a graze, and phase 4 is what made it visible.** The
        // riser this tread caps stops at exactly the tread's own height, so a ray
        // running from the tread to a source at that height runs exactly along the
        // riser's top edge — and a flame is a body [`FLAME_DEPTH`] tall, so half of
        // it is above that edge and half below. `crosses`/`pierces` answer `0.5`,
        // which is what the geometry says and not a defect. It read `1.0` while
        // `stand_clear` lifted every ray a hundred-and-twenty-eighth clear of its
        // own surface, and that is the margin this claim was resting on rather than
        // the claim itself: what the test is about is a riser not shadowing the
        // tread it caps *tile-wide*, which is what a real staircase render showed.
        let light = Light {
            at:        Vec2::new(102.5, 100.5),
            z:         top.top() as f32 + FLAME_LIFT,
            radius:    6.0,
            color:     [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam:      None,
        };
        let lighting = Lighting {
            ambient: NIGHT,
            lights: vec![light],
            occlusion,
            sun: None,
            view: crate::debug::View::default(),
            flame_radius: FLAME_RADIUS,
            shadow_rays: ShadowRays::DEFAULT,
            dead: false,
        };

        let sample = sample(spot, &lighting);
        assert!(
            sample.reaches[0].through > 0.9,
            "a tread's own top should not be dimmed by the riser it caps: through {}",
            sample.reaches[0].through,
        );
    }

    /// `docs/render/design_model.md` **phase 4**, at the walk rather than at
    /// [`exemption`]: a fragment is not shadowed by the solid it is a point of,
    /// and **is** shadowed by every other solid of its own static.
    ///
    /// One flight, three treads `1,3,5`, climbing north on one tile — the scene
    /// `examples/synthetic_stair` draws and the face oracle measured. Its six
    /// solids are one [`crate::occlusion::Builder::add`], so they share an
    /// [`crate::occlusion::OwnerId`] and differ in
    /// [`crate::occlusion::Part`]; each fragment here names its own through
    /// [`crate::occlusion::Occlusion::id_of`], which is the whole of what the
    /// phase added. Both walks, because a rule one of them has is a parity gap.
    ///
    /// Three rays, and each one kills a different mutation:
    ///
    /// - **Off a tread's own top, steeply down.** The only solid on the line is
    ///   that tread's own lid and the ray leaves its plane, so the only contact
    ///   is at the origin. Red before `docs/archive/render/lighting_height.md`'s own phase 4 —
    ///   [`stand_clear`]'s [`ON_TOP`] lifted the fragment a
    ///   hundred-and-twenty-eighth clear of its own top and turned that contact
    ///   into a crossing, which is what painted 1522 and 1346 pixels of the
    ///   middle and top treads black.
    /// - **Off a riser, up and east, over that flight's own bottom tread.** The
    ///   *same kind* of solid — a lid of the fragment's own static — and it must
    ///   still stop the ray, because that crossing is at `t > 0` and well away
    ///   from where the ray started: a lamp above and beyond a staircase genuinely
    ///   cannot see the front of its bottom step. A fix phrased as "a fragment is
    ///   never shadowed by its own static" lights this one, and lighting it is
    ///   what an owner-level exemption did.
    /// - **Off a tread's top, down and south, into the riser under it.** A
    ///   riser of the fragment's own static, and a different solid from the lid
    ///   the fragment stands on. Under an owner this needed a rule of its own —
    ///   `edges & own`, "a flat fragment is a point of no panel" — and under an
    ///   id it needs nothing: two ids differ, so the riser occludes, which is
    ///   what a riser standing in a real place does.
    ///
    /// **Mutate the comparison to read the two solids' owners instead of their ids
    /// and the first two go green while the third stays green** — the arrangement
    /// that says the third ray is the one about parts. Mutate it to `true` and the
    /// second goes red.
    ///
    /// **Where the `None` half of it is measured, since it is not here.** A
    /// fragment that is a point of nothing must be exempt from nothing, and this
    /// fixture cannot show it: a flat fragment's own solid is a *lid*, and
    /// `crosses`'s strictness already answers a ray leaving a plane exactly as no
    /// crossing at all; a face fragment's own solid is a *panel*, and `same_run`
    /// masked its own cell's side whatever the fragment carried — that rule is gone
    /// with S4, and what stands in its place ([`on_the_lit_surface`]) reads the
    /// fragment's own box, so it too answers nothing for a fragment that has none.
    /// What does show it
    /// is `tests/lighting.rs`'s `the_face_of_a_wall_is_lit_from_inside_the_room`
    /// and `a_carried_light_lights_the_way_it_is_pointed`, both of which go red
    /// with the comparison forced to `false` — checked by injecting exactly that.
    #[test]
    fn a_fragment_is_shadowed_by_every_solid_of_its_own_static_but_the_one_it_is_a_point_of() {
        use crate::facing::Prism;
        use crate::occlusion::{
            Builder,
            Shape,
        };

        let stair = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT | TileFlags::CLIMBABLE),
            height: 20,
            ..StaticTile::default()
        };
        // North, so the treads divide the tile up `y`: tread 0 is the body over
        // `100.667..101` standing from the ground to `z 1`, tread 1 over
        // `100.333..100.667` to `z 3`, tread 2 over `100..100.333` to `z 5`.
        // Three solids, one a tread — see `occlusion::Builder::add`'s climbable
        // branch for why they are bodies and not a lid and a riser each.
        let prism = Prism::new(Face::North, &[1, 3, 5]).expect("three treads");
        let mut occlusion = Builder::new(crate::camera::TileBounds {
            min_x: 95,
            max_x: 105,
            min_y: 95,
            max_y: 105,
        });
        let graphic = Graphic(0x0736);
        occlusion.add(100, 100, 0, graphic, &stair, Shape::solid(prism));
        let occlusion = occlusion.finish(&Cutaway::OPEN);
        // The flight's three solids by name, in `Builder::add`'s own push
        // order: one a tread, climbing. See [`crate::occlusion::Part`].
        let part = |at: usize| {
            occlusion
                .id_of(
                    100,
                    100,
                    crate::occlusion::Owner::new(0, graphic),
                    crate::occlusion::Part::nth(at),
                )
                .expect("the flight's own three solids")
        };
        let (tread_0, tread_2) = (part(0), part(2));

        let walked = |spot: Spot, at: Vec2, z: f32| {
            let lighting = Lighting {
                ambient:      NIGHT,
                lights:       vec![Light {
                    at,
                    z,
                    // Wide enough that nothing here is out of reach: this is a
                    // test about what stands in the way, not about falloff.
                    radius: 40.0,
                    color: [1.0, 1.0, 1.0],
                    intensity: 1.0,
                    beam: None,
                }],
                occlusion:    occlusion.clone(),
                sun:          None,
                view:         crate::debug::View::default(),
                flame_radius: FLAME_RADIUS,
                shadow_rays:  ShadowRays::DEFAULT,
                dead:         false,
            };
            let streaming = sample(spot, &lighting).reaches[0].through;
            let exact = sample_exact(spot, &lighting).reaches[0].through;
            (streaming, exact)
        };

        // 1. Off the top tread's own top, down past the flight. Nothing else is
        //    under it: the two lower treads are strips of `y` this ray is never
        //    over.
        let on_top = Spot::flat(Vec2::new(100.5, 100.15), 5.0, (100, 100)).part_of(tread_2);
        let (streaming, exact) = walked(on_top, Vec2::new(100.6, 100.25), -5.0);
        assert!(
            streaming > 0.99 && exact > 0.99,
            "a tread's own top is a contact at the ray's origin, not a crossing: \
             streaming {streaming}, exact {exact}",
        );

        // 2. The counter-example, and the same static at `t > 0`: off the front
        //    of the bottom tread — the flight's own south face — towards a lamp
        //    hanging over the middle of the flight. The ray leaves its own solid
        //    at once and walks straight into the next tread up, which is a
        //    different solid of the same static.
        let on_front = Spot::face(Vec2::new(100.5, 101.0), 0.5, (100, 100), Face::South).part_of(tread_0);
        let (streaming, exact) = walked(on_front, Vec2::new(101.0, 100.5), 2.0);
        assert!(
            streaming < 0.5 && exact < 0.5,
            "the tread above is a different solid and stands between this face and the lamp: \
             streaming {streaming}, exact {exact}",
        );

        // 3. And the same claim from the other side of the flight: a fragment on
        //    the bottom tread's own top, lit from level with it to the north, is
        //    shadowed by the two treads climbing away from it. This is what a
        //    staircase does — you cannot see the low step from behind the high
        //    one — and it is the case a single body for the whole flight, or a
        //    tread excused from its neighbours, would both get wrong.
        let on_bottom = Spot::flat(Vec2::new(100.5, 100.8), 1.0, (100, 100)).part_of(tread_0);
        let (streaming, exact) = walked(on_bottom, Vec2::new(100.5, 99.0), 1.0);
        assert!(
            streaming < 0.5 && exact < 0.5,
            "a tread of the fragment's own flight is a different solid, so it stops the ray: \
             streaming {streaming}, exact {exact}",
        );
    }

    /// **A ray with no horizontal run is still only stopped by lids it is
    /// actually under.**
    ///
    /// `docs/archive/render/lighting_height.md`'s backlog entry, and the reason the ray above
    /// this one is *slanted*: both walks take a shortcut when a ray has no
    /// horizontal run — there is no direction to step in, so only this one cell
    /// can hold anything — and the shortcut applied `crosses` to **every** lid
    /// on the cell without asking whether the ray is over that lid at all. The
    /// main path stopped doing that when sub-tile footprints landed; the
    /// shortcut did not follow.
    ///
    /// A flight is exactly where that shows: its three treads are three bodies
    /// on one tile, each a *strip* of it, and no point is over more than one of
    /// them. So a fragment on a tread lit from straight above or below was
    /// shadowed by the other two treads — solids standing over a part of the
    /// tile it is nowhere near.
    ///
    /// Both directions, because they fail through different lids: from the top
    /// tread downwards the two lower lids are below the fragment and the ray
    /// runs down past them, and from the bottom tread upwards the two higher
    /// lids are above it and the ray runs up past them. A fix that gated only
    /// one end would leave the other reading as a real occlusion.
    ///
    /// Its own tread is not what is being asserted away: that one is excused by
    /// identity, which is the test above's subject. What *is* asserted is the
    /// footprint gate, and it carries more weight than it used to: the shortcut
    /// looks at bodies now as well as lids (see its own comment), so a tread it
    /// is not over is a solid it would otherwise stop on outright.
    ///
    /// ⚠ **And this test stopped sending a vertical ray at phase 5, silently.**
    /// A flame became a sphere and [`flame_points`] lays its samples on the disc
    /// the sphere presents — `sqrt((i + 0.5) / n)` of the radius, so **no sample
    /// is the centre**. A flame directly overhead is therefore eight rays each
    /// leaning [`FLAME_RADIUS`] out of the vertical, and the branch this test is
    /// named for was never entered again. Measured on 2026-08-09: the whole crate
    /// runs the shortcut *zero* times. `flame_radius` is `0.0` here for that
    /// reason and the assertion below is the positive control — a fixture that
    /// cannot reach the rule it is about passes for the wrong reason, which is the
    /// same defect as a gate that is green under injection.
    #[test]
    fn a_vertical_ray_is_not_stopped_by_lids_it_is_not_over() {
        use crate::facing::Prism;
        use crate::occlusion::{
            Builder,
            Shape,
        };

        let stair = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT | TileFlags::CLIMBABLE),
            height: 20,
            ..StaticTile::default()
        };
        // The same flight as the test above, and the same divisions: tread 0
        // over `y 100.667..101` capped at `z 1`, tread 1 over `100.333..100.667`
        // at `z 3`, tread 2 over `100..100.333` at `z 5`.
        let prism = Prism::new(Face::North, &[1, 3, 5]).expect("three treads");
        let mut occlusion = Builder::new(crate::camera::TileBounds {
            min_x: 95,
            max_x: 105,
            min_y: 95,
            max_y: 105,
        });
        let graphic = Graphic(0x0736);
        occlusion.add(100, 100, 0, graphic, &stair, Shape::solid(prism));
        let occlusion = occlusion.finish(&Cutaway::OPEN);
        let part = |at: usize| {
            occlusion
                .id_of(
                    100,
                    100,
                    crate::occlusion::Owner::new(0, graphic),
                    crate::occlusion::Part::nth(at),
                )
                .expect("the flight's own three solids")
        };
        let (tread_0, tread_2) = (part(0), part(2));

        let walked = |spot: Spot, at: Vec2, z: f32| {
            let lighting = Lighting {
                ambient:      NIGHT,
                lights:       vec![Light {
                    at,
                    z,
                    radius: 40.0,
                    color: [1.0, 1.0, 1.0],
                    intensity: 1.0,
                    beam: None,
                }],
                occlusion:    occlusion.clone(),
                sun:          None,
                view:         crate::debug::View::default(),
                // A point flame, which is the only thing that sends a ray
                // straight up — see this test's own doc comment.
                flame_radius: 0.0,
                shadow_rays:  ShadowRays::DEFAULT,
                dead:         false,
            };
            // The positive control, and it is the whole reason this test is worth
            // anything: every one of the rays actually walked has to have no
            // horizontal run at all. At `FLAME_RADIUS` none of them does.
            let straight = flame_points(spot, [at.x, at.y, z], lighting.flame_radius, lighting.shadow_rays)
                .iter()
                .all(|point| point[0] == spot.at.x && point[1] == spot.at.y);
            assert!(
                straight,
                "the fixture is not sending a vertical ray, so it cannot be about one",
            );
            (
                sample(spot, &lighting).reaches[0].through,
                sample_exact(spot, &lighting).reaches[0].through,
            )
        };

        // Straight down off the top tread. The flame is directly under the
        // fragment, so the ray's horizontal run is zero by construction rather
        // than by a tolerance — `Spot::flat` carries no outward normal, so
        // `stand_clear` lifts it in `z` alone and cannot nudge it off the line.
        let on_top = Spot::flat(Vec2::new(100.5, 100.15), 5.0, (100, 100)).part_of(tread_2);
        let (streaming, exact) = walked(on_top, Vec2::new(100.5, 100.15), -5.0);
        assert!(
            streaming > 0.99 && exact > 0.99,
            "the lower treads are strips of `y` this ray is never over: \
             streaming {streaming}, exact {exact}",
        );

        // And straight up off the bottom tread, where the two lids in question
        // are the ones *above* the fragment.
        let on_bottom = Spot::flat(Vec2::new(100.5, 100.8), 1.0, (100, 100)).part_of(tread_0);
        let (streaming, exact) = walked(on_bottom, Vec2::new(100.5, 100.8), 15.0);
        assert!(
            streaming > 0.99 && exact > 0.99,
            "the higher treads are strips of `y` this ray is never over: \
             streaming {streaming}, exact {exact}",
        );
    }

    /// **A floor is one surface at the point four of its tiles meet, and a ray
    /// through that point is stopped** — `docs/render/design_model.md` phase 6i.
    ///
    /// Reported by a person playing: a lattice of bright points over a floor at
    /// `(1492, 1642)`, `z 28`, **one to a tile corner** and nothing along the
    /// seams between two tiles. The lattice is the diagnosis. A leak along a
    /// seam is an interval question and would have shown as a line; a leak at a
    /// point is a *degenerate* one, and the only interval this walk had that
    /// collapses to a point at a corner was the one the lid rule was asked
    /// over — the ray's run inside the lid's own horizontal footprint. A ray
    /// through a corner enters and leaves that footprint at one `t`, so both
    /// ends of it carried one `z` and `crosses` answered, honestly, that
    /// nothing had crossed anything over an interval of no length. All four
    /// lids sharing the corner answered alike, which is what left a hole.
    ///
    /// The scene is the smallest thing that can pose it: four floor tiles round
    /// `(101, 101)`, a fragment over one of them and a flame under the tile
    /// diagonally opposite, so the segment passes through the shared corner at
    /// exactly the floor's own height. Both walks, since the leak was in the
    /// rule and not in either walk's traversal, and the shader carries the same
    /// arm again in `blit.wesl`.
    ///
    /// The positive control is the geometry: the assertion below would pass for
    /// a ray that missed the corner by a tile, so the midpoint of the segment is
    /// checked to *be* the corner first. `flame_radius` is `0.0` for the same
    /// reason — eight samples on a disc would put seven of them off the corner,
    /// and the leak this is about is what one exact ray does.
    #[test]
    fn a_ray_through_the_point_four_floor_tiles_share_is_stopped_by_them() {
        use crate::occlusion::{
            Builder,
            Shape,
        };

        // `FLOOR` is the whole of what makes a lid — `occlusion::boxes_of` asks
        // `is_background()` and nothing else, deliberately, so that a floor
        // whose silhouette read as a wall is still a floor.
        let floor = StaticTile {
            flags: TileFlags::new(TileFlags::FLOOR | TileFlags::NO_SHOOT),
            height: 0,
            ..StaticTile::default()
        };
        let mut occlusion = Builder::new(crate::camera::TileBounds {
            min_x: 95,
            max_x: 105,
            min_y: 95,
            max_y: 105,
        });
        // Four tiles round one corner, each its own graphic: `occlusion::merge`
        // folds a floor of one graphic into a single primitive, and a single
        // primitive has no corner shared between two of its pieces to leak at.
        // That the merge would have hidden this defect is worth stating in the
        // fixture rather than discovering later — a real floor is laid out of
        // several graphics and a merged one is the easier case.
        for (at, graphic) in [
            ((100u16, 100u16), 0x0400u16),
            ((101, 100), 0x0401),
            ((100, 101), 0x0402),
            ((101, 101), 0x0403),
        ] {
            occlusion.add(at.0, at.1, 0, Graphic(graphic), &floor, Shape::UNREAD);
        }
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        // Over the middle of the first tile, and the flame under the middle of
        // the one diagonally opposite: the segment's midpoint is the corner all
        // four share, at the floors' own `z`.
        let from = Vec2::new(100.5, 100.5);
        let to = Vec2::new(101.5, 101.5);
        let (above, below) = (10.0f32, -10.0f32);
        assert_eq!(
            [
                (from.x + to.x) * 0.5,
                (from.y + to.y) * 0.5,
                (above + below) * 0.5
            ],
            [101.0, 101.0, 0.0],
            "the fixture's ray does not pass through the corner these tiles share, so it cannot be \
             about one",
        );

        let lighting = Lighting {
            ambient: NIGHT,
            lights: vec![Light {
                at:        to,
                z:         below,
                radius:    40.0,
                color:     [1.0, 1.0, 1.0],
                intensity: 1.0,
                beam:      None,
            }],
            occlusion,
            sun: None,
            view: crate::debug::View::default(),
            // One exact ray — see this test's own doc.
            flame_radius: 0.0,
            shadow_rays: ShadowRays::DEFAULT,
            dead: false,
        };
        // A point of no primitive: the fragment is in the air over the floor, so
        // no exemption is in play and what answers is the lid rule alone.
        let spot = Spot::flat(from, above, (100, 100));
        let streaming = sample(spot, &lighting).reaches[0].through;
        let exact = sample_exact(spot, &lighting).reaches[0].through;
        assert!(
            streaming == 0.0 && exact == 0.0,
            "the ray passes through the point four floors share and reaches the flame anyway: \
             streaming {streaming}, exact {exact}",
        );
    }

    /// **A wall the flame sits exactly level with can still be skipped whole.**
    ///
    /// `docs/archive/render/lighting_raymarch.md`'s "A new `walk_cells` miss" backlog entry,
    /// found rendering a picture rather than sweeping for it and root-caused by
    /// a per-iteration DDA trace. A flame standing exactly on a wall row's own
    /// north edge (`flame.y == wall_tile.y as f32`) makes `corner_tie` balloon
    /// for a query only a fraction of a tile off that row — `per_tile[far] = 1
    /// / |delta[far]|` grows without bound as the ray's far-axis delta shrinks
    /// — and the inflated tie swallows a `boundary[0]` that has nothing to do
    /// with a real corner, stepping the walk diagonally past the entire row the
    /// wall stands on, wall included.
    ///
    /// Expected answers here are worked out from the straight-line geometry
    /// itself (does the segment's continuous path enter the wall's box for
    /// any interior `t`), not copied from the handoff's hand-traced table —
    /// that table's own `y = 99.9` entry turned out to be a second, unrelated
    /// coincidence: the *old* buggy walk took a spurious diagonal corner step
    /// at its very first boundary that happened to land it back in the wall's
    /// row, and it went on to find the wall the ordinary way from there. The
    /// straight segment at `y = 99.9` never actually enters the wall's row —
    /// `y(t) < 100` for every interior `t` — so the geometrically correct
    /// answer is *open*, and a fixed `corner_tie` reports exactly that; a
    /// naive "matches the old table" assertion here would have pinned the old
    /// bug's own coincidence as if it were the spec.
    #[test]
    fn a_wall_level_with_the_flame_is_not_skipped_by_a_shallow_ray() {
        use crate::occlusion::{
            Builder,
            Shape,
        };

        let wall = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT),
            height: 20,
            ..StaticTile::default()
        };
        let mut occlusion = Builder::new(crate::camera::TileBounds {
            min_x: 90,
            max_x: 110,
            min_y: 90,
            max_y: 110,
        });
        occlusion.add(100, 100, 0, Graphic(0x0100), &wall, Shape::UNREAD);
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        let light = Light {
            at:        Vec2::new(98.0, 100.0),
            z:         10.0,
            radius:    12.0,
            color:     [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam:      None,
        };
        let lighting = Lighting {
            ambient: NIGHT,
            lights: vec![light],
            occlusion,
            sun: None,
            view: crate::debug::View::default(),
            flame_radius: FLAME_RADIUS,
            shadow_rays: ShadowRays::DEFAULT,
            dead: false,
        };

        // `blocked`: whether the straight segment from `(102.5, y)` to the
        // flame passes through the wall's box, `x` in `[100, 101]`, for any
        // interior `t` — worked out directly rather than sampled, since the
        // whole box is crossed on one contiguous stretch of `x`.
        for (y, blocked) in [
            (99.9_f32, false),
            (100.1, true),
            (100.2, true),
            (100.3, true),
            (101.0, true),
        ] {
            let tile = (102, y.floor() as i32);
            let spot = Spot::flat(Vec2::new(102.5, y), 10.0, tile);
            let sample = sample(spot, &lighting);
            let reach = sample.reaches[0];
            let through = reach.through;
            assert!(
                blocked == (through <= 0.004),
                "y {y}: expected {} but through is {through} (stopped_by {:?})",
                if blocked { "blocked" } else { "open" },
                reach.stopped_by,
            );
        }
    }

    // **`the_dda_walk_does_not_skip_the_wall_row_on_a_shallow_ray` stood here**,
    // and `docs/render/design_occluders.md`'s S5 took its subject away with the DDA.
    //
    // It was the pure-geometry echo of
    // [`a_wall_level_with_the_flame_is_not_skipped_by_a_shallow_ray`] above,
    // which is still drawn and still the gate: the same six spots and the same
    // wall row, asking `dda_walk` alone whether its cell sequence ever visited
    // `(100, 100)`. That question was worth a test because a *row* was how the
    // grid found a wall, and `docs/archive/render/lighting_raymarch.md`'s "A new `walk_cells`
    // miss" is a ray hugging a row's own grid line and skipping the row.
    //
    // **A tree has no rows to skip.** A primitive is under exactly one leaf
    // whatever line the ray hugs, and what decides whether it is asked about is
    // the slab test against its own node's box — the same test that then decides
    // the primitive itself. There is no second, coarser question left to get
    // wrong, which is the whole of why the grid's own gap cannot recur here.

    /// `docs/archive/render/lighting_raymarch.md`'s ray-vs-Solid scoping, point 3, over the
    /// three-tread climbable stair
    /// [`a_treads_top_is_not_shadowed_by_the_tread_it_is_the_top_of`] uses.
    /// This is the scene that found a real bug in [`walk_the_record`], not
    /// just another `walk_cells` gap: a lid is flat in `z`
    /// (`Solid::box_of`'s `min.z == max.z`), and a flight's treads were two
    /// such degenerate boxes each until they became bodies — [`ray_vs_solid`]'s
    /// slab method correctly
    /// collapses `entered` and `leaves` to the exact same instant on
    /// either one, since a degenerate-thickness box is genuinely crossed
    /// at one point in `t`, not over an interval. `crosses` was never
    /// built for that: it reads `entering`/`leaving` as the ray's `z` on
    /// *either side* of a crossing to tell "went through" from "never
    /// close," and a from/to that already collapsed to the same value
    /// answers every comparison in it as "never" — regardless of the real
    /// geometry. `walk_the_record` read every lid as fully transparent
    /// before this was caught, `1.0` unconditionally.
    ///
    /// **Fixed by asking a different question for the lid branch**: not
    /// "where does the ray touch this lid's own (degenerate) box" but
    /// "where does the ray enter and leave the *tile's* footprint" — a
    /// second `ray_vs_solid` call against a synthetic box sharing the
    /// tile's `x`/`y` bounds with `z` left unconstrained, giving
    /// `crosses` the before/after pair it actually needs. The same
    /// question `walk_cells`'s own DDA cell entry/exit answered for free;
    /// `walk_the_record` has to ask it explicitly since it no longer
    /// walks cells at all.
    ///
    /// This test pins the regression at the exact input the stair scene's
    /// own fuzz found it at, rather than trusting the fix by reasoning
    /// alone: reverting the tile-footprint lookup back to the lid's own
    /// `entered`/`leaves` reproduces `walk_the_record` reading fully open
    /// (`1.0`) here, confirmed by hand before this test was written.
    #[test]
    fn walk_the_record_does_not_read_every_lid_as_transparent() {
        use crate::facing::Prism;
        use crate::occlusion::{
            Builder,
            Shape,
        };

        let stair = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT | TileFlags::CLIMBABLE),
            height: 20,
            ..StaticTile::default()
        };
        let prism = Prism::new(Face::West, &[1, 3, 5]).expect("three treads");
        let mut occlusion = Builder::new(crate::camera::TileBounds {
            min_x: 95,
            max_x: 105,
            min_y: 95,
            max_y: 105,
        });
        occlusion.add(100, 100, 0, Graphic(0x0736), &stair, Shape::solid(prism));
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        let from = [101.26917_f32, 99.877884, 4.255842];
        let to = [100.57816_f32, 100.689926, 0.0];
        let new = walk_the_record(from, to, LitEnd::nowhere(), &occlusion);
        assert!(
            new.0 < 0.5,
            "a ray crossing the first tread's own lid should not read as more than half open: \
             through {} (blamed {:?})",
            new.0,
            new.1,
        );
    }

    /// `docs/archive/render/lighting_raymarch.md`'s ray-vs-Solid scoping, point 3, over the
    /// same stair scene as
    /// [`walk_the_record_does_not_read_every_lid_as_transparent`] — a
    /// smoke test, not a parity oracle.
    ///
    /// **Full numeric agreement with `walk_cells`, or even the weaker
    /// "every disagreement is a real `ray_vs_solid` hit" claim the
    /// single-wall scenes above make, does not hold here, and chasing it
    /// did not fit this session.** Once a tile carries several solids at
    /// different heights and different fractional footprints — three
    /// treads and three risers on one tile, not one body filling it —
    /// `walk_cells`'s own per-*cell* model (one shared `entered`/`leaves`
    /// tested against every solid on the tile, `pierced`'s z-band test
    /// never checking a riser's own `x`/`y` extent at all) can find or
    /// miss occlusion a per-*solid* exact test would not, in either
    /// direction, and telling that apart from a `walk_the_record` bug
    /// needs the same exemption predicates (`on_surface`, `own_run`,
    /// `flame_end`) evaluated the same way a disagreement-characterising
    /// test would have to duplicate — a real next step, not attempted
    /// here. What this checks instead: `walk_the_record` never panics and
    /// never returns a `through` outside `0.0..=1.0` over a broad fuzz of
    /// this richer scene — the lid bug above would have shown up here too,
    /// as values pinned at `1.0` far more often than the geometry allows.
    #[test]
    fn walk_the_record_stays_in_range_on_the_stair() {
        use proptest::prelude::*;

        use crate::facing::Prism;
        use crate::occlusion::{
            Builder,
            Shape,
        };

        let stair = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT | TileFlags::CLIMBABLE),
            height: 20,
            ..StaticTile::default()
        };
        let prism = Prism::new(Face::West, &[1, 3, 5]).expect("three treads");
        let mut occlusion = Builder::new(crate::camera::TileBounds {
            min_x: 95,
            max_x: 105,
            min_y: 95,
            max_y: 105,
        });
        occlusion.add(100, 100, 0, Graphic(0x0736), &stair, Shape::solid(prism));
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        proptest!(ProptestConfig::with_cases(8_000), |(
            fx in 97.0_f32..103.0,
            fy in 97.0_f32..103.0,
            fz in 0.0_f32..6.0,
            tx in 97.0_f32..103.0,
            ty in 97.0_f32..103.0,
            tz in 0.0_f32..6.0,
        )| {
            prop_assume!((fx - tx).abs() > 1e-3 || (fy - ty).abs() > 1e-3);
            let from = [fx, fy, fz];
            let to = [tx, ty, tz];
            let new = walk_the_record(from, to, LitEnd::nowhere(), &occlusion);
            prop_assert!((0.0..=1.0).contains(&new.0), "from {from:?} to {to:?}: through {}", new.0);
        });
    }

    /// `docs/archive/render/lighting_raymarch.md`'s point 4, the same six-point counter-
    /// example this whole track started from — full numeric agreement with
    /// [`walk_the_record`]. This is a single whole-tile body
    /// (`Shape::UNREAD`), so [`crate::occlusion::Solid::box_of`]'s
    /// reconstruction is bit-for-bit the solid's own real `space`, and this
    /// is the case [`walk_the_wire`]'s own doc comment claims exact
    /// agreement for.
    #[test]
    fn walk_the_wire_agrees_with_walk_the_record_on_the_six_point_counter_example() {
        use crate::occlusion::{
            Builder,
            Shape,
        };

        let wall = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT),
            height: 20,
            ..StaticTile::default()
        };
        let mut occlusion = Builder::new(crate::camera::TileBounds {
            min_x: 90,
            max_x: 110,
            min_y: 90,
            max_y: 110,
        });
        occlusion.add(100, 100, 0, Graphic(0x0100), &wall, Shape::UNREAD);
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        let flame = [98.0_f32, 100.0, 10.0];
        for y in [99.9_f32, 100.1, 100.2, 100.3, 101.0] {
            let from = [102.5_f32, y, 10.0];
            let exact = walk_the_record(from, flame, LitEnd::nowhere(), &occlusion).0;
            let streaming = walk_the_wire(from, flame, LitEnd::nowhere(), &occlusion).0;
            assert!(
                (exact - streaming).abs() < 1e-4,
                "y {y}: walk_the_record through {exact} disagrees with walk_the_wire through {streaming}",
            );
        }
    }

    /// `docs/archive/render/lighting_raymarch.md`'s point 4, over the same single-body wall
    /// scene the six-point counter-example's own occlusion is built from —
    /// **with no corner restriction**, because [`walk_the_wire`] has
    /// no corner-jump branch to be restricted away from. Full numeric
    /// agreement with [`walk_the_record`] everywhere in the domain.
    #[test]
    fn walk_the_wire_agrees_with_walk_the_record_on_a_single_body() {
        use proptest::prelude::*;

        use crate::occlusion::{
            Builder,
            Shape,
        };

        let wall = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT),
            height: 20,
            ..StaticTile::default()
        };
        let mut occlusion = Builder::new(crate::camera::TileBounds {
            min_x: 90,
            max_x: 110,
            min_y: 90,
            max_y: 110,
        });
        occlusion.add(100, 100, 0, Graphic(0x0100), &wall, Shape::UNREAD);
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        proptest!(ProptestConfig::with_cases(8_000), |(
            fx in 95.0_f32..105.0,
            fy in 95.0_f32..105.0,
            fz in 0.0_f32..20.0,
            tx in 95.0_f32..105.0,
            ty in 95.0_f32..105.0,
            tz in 0.0_f32..20.0,
        )| {
            prop_assume!((fx - tx).abs() > 1e-3 || (fy - ty).abs() > 1e-3);
            let from = [fx, fy, fz];
            let to = [tx, ty, tz];
            let exact = walk_the_record(from, to, LitEnd::nowhere(), &occlusion).0;
            let streaming = walk_the_wire(from, to, LitEnd::nowhere(), &occlusion).0;
            prop_assert!(
                (exact - streaming).abs() < 1e-3,
                "from {from:?} to {to:?}: walk_the_record {exact} vs walk_the_wire {streaming}",
            );
        });
    }

    // **`a_walk_starts_in_a_cell_its_own_start_point_is_in` stood here**, and
    // `docs/render/design_occluders.md`'s S5 took the cell it was about.
    //
    // It had already outlived one deletion: its subject was `starting_cell`, the
    // arbiter S4 removed, and what was repointed at `dda_walk` afterwards was the
    // half of the claim a DDA still needed — **the cell a walk seeds itself from
    // contains its own start point** — because a walk seeded from a cell the
    // point is not in computes a negative distance to a boundary it has already
    // crossed. That is the 324-pixel leak in the grave note above.
    //
    // **A traversal has no seed at all.** It starts at the root of a tree whose
    // nodes are boxes in world coordinates, so there is no cell to be wrong
    // about, no `floor()` to put a boundary point on the wrong side of one, and
    // nothing left for this to assert. The 11,544 exact-edge ties its domain was
    // built around are not a case any more: a point on two cells' shared boundary
    // was only ever a question because a cell was the unit of lookup.

    /// **The brute-force oracle: [`ray_vs_solid`] against every primitive in
    /// the grid, with no cell in it anywhere.** `docs/render/design_occluders.md`'s
    /// § *The oracle*.
    ///
    /// Not either walk with the traversal taken out — it shares no traversal
    /// with either, which is the whole of what makes it non-circular. Where
    /// `tests/lighting.rs`'s `brute_force_blocked` marches fixed steps and asks
    /// `solids_at` which solids a *tile* holds, this asks the list itself; the
    /// two are different dumbnesses and both are worth having, which is what
    /// that section says.
    ///
    /// Binary, because what S1's fixture asks is whether a ray met a box at all
    /// and a coordinate the wire moved flips exactly that. The touch rule is
    /// the walks' own — a ray that meets a solid only at the point it starts
    /// from has not gone through it.
    ///
    /// Reads [`crate::occlusion::Solid::wire_box`], which is what the streaming
    /// walk and the shader are entitled to; [`walk_the_record`] reads the
    /// record, and the fixture below is where those two are asked to be the
    /// same box.
    fn met_by_brute_force(from: [f32; 3], to: [f32; 3], occlusion: &Occlusion) -> bool {
        (0..occlusion.solid_count()).any(|at| {
            let solid = occlusion.solid(crate::occlusion::SolidId::new(at as u32));
            match ray_vs_solid(from, to, &solid.wire_box()) {
                Some((entered, leaves)) => entered != 0.0 || leaves != 0.0,
                None => false,
            }
        })
    }

    /// **A primitive whose corners are at no fraction of a tile a byte could
    /// name, read the same by both walks and by the brute-force oracle** —
    /// `docs/render/design_occluders.md`'s S1 gate, and the blindness that step is also
    /// fixing: no scene in the tree had such a shape before this one.
    ///
    /// **The coordinates are chosen, not sampled.** The wire used to carry a
    /// primitive as a cell and four bytes of `1/255` of it across, plus sixteen
    /// bits of `1/256` of a `z` unit up; the point such a grid is *maximally*
    /// wrong about is exactly half a step off it, so every face of this box sits
    /// there. That is the fixture stating the defect rather than hoping to trip
    /// over it — a box on thirds would be within a thousandth of the old wire's
    /// answer and this test would pass with the quantisation put back.
    ///
    /// **The rays are aimed at the faces and run parallel to them**, half a
    /// thousandth of a tile to either side. That offset is under the old wire's
    /// own half step (`1/510` of a tile, `1/512` of a `z` unit) and far over an
    /// `f32`'s last bits out here at a hundred tiles, so each pair straddles the
    /// record's own face and nothing else: the ray a hair inside a `min` face is
    /// outside the byte grid's, and the ray a hair outside a `max` face is
    /// inside it. Both directions, because a rounding that moved every face the
    /// same way would be caught by only one of them.
    ///
    /// Parallel and not grazing: a ray that crosses a face at an angle is inside
    /// the box for a hair's length of its path, which is a question about a
    /// sampler's step size. A ray *along* the face is inside for its whole
    /// length or for none of it, and that is a claim about where the face is.
    ///
    /// The box stays inside its own tile, which is deliberate: a primitive wider
    /// than a tile is expressible on the wire now and the *grid* still lists it
    /// on one cell only, so a ray that never enters that cell would miss it.
    /// That is D3's own argument for the hierarchy and S3's to answer — see this
    /// plan's backlog. What is under test here is the wire.
    #[test]
    fn a_primitive_at_no_fraction_a_byte_could_name_reads_the_same_three_ways() {
        use crate::occlusion::Builder;

        // Half a step off the byte grid a footprint used to be measured on, and
        // off the sixteen-bit grid a span used to be measured on. `units` is
        // which step, so the fixture reads as "between step 30 and step 31".
        let across = |units: f64| (units + 0.5) / 255.0;
        let up = |steps: f64| (steps + 0.5) / 256.0 - 128.0;
        let (min_x, max_x) = (100.0 + across(30.0), 100.0 + across(220.0));
        let (min_y, max_y) = (100.0 + across(74.0), 100.0 + across(200.0));
        let (min_z, max_z) = (up(33_049.0), up(35_000.0));

        let mut builder = Builder::new(crate::camera::TileBounds {
            min_x: 95,
            max_x: 105,
            min_y: 95,
            max_y: 105,
        });
        builder.add_raw(
            100,
            100,
            crate::solid::Solid {
                min: crate::camera::WorldSpot {
                    x: min_x,
                    y: min_y,
                    z: min_z,
                },
                max: crate::camera::WorldSpot {
                    x: max_x,
                    y: max_y,
                    z: max_z,
                },
            },
            crate::occlusion::Owner::new(0, openshard_protocol::wire::Graphic(1)),
        );
        let occlusion = builder.finish(&Cutaway::OPEN);

        // A twentieth of the old wire's own half step: inside its blind spot and
        // nowhere near an `f32`'s.
        const OFF: f64 = 0.0005;
        let mid_x = (min_x + max_x) * 0.5;
        let mid_z = (min_z + max_z) * 0.5;
        // `(name, from, to, met)` — a ray along one axis, at a coordinate a hair
        // to one side of one face, and whether the record says it meets the box.
        let along_y = |x: f64, z: f64| ([x as f32, 97.0, z as f32], [x as f32, 103.0, z as f32]);
        let along_x = |y: f64, z: f64| ([97.0, y as f32, z as f32], [103.0, y as f32, z as f32]);
        let mut rays = Vec::new();
        for (face, offset) in [(min_x, OFF), (min_x, -OFF), (max_x, -OFF), (max_x, OFF)] {
            let (from, to) = along_y(face + offset, mid_z);
            rays.push((format!("x = {:.6}", face + offset), from, to));
        }
        for (face, offset) in [(min_y, OFF), (min_y, -OFF), (max_y, -OFF), (max_y, OFF)] {
            let (from, to) = along_x(face + offset, mid_z);
            rays.push((format!("y = {:.6}", face + offset), from, to));
        }
        for (face, offset) in [(min_z, OFF), (min_z, -OFF), (max_z, -OFF), (max_z, OFF)] {
            let (from, to) = along_y(mid_x, face + offset);
            rays.push((format!("z = {:.6}", face + offset), from, to));
        }

        let mut met = 0;
        for (name, from, to) in &rays {
            let lit = LitEnd::nowhere();
            let truth = met_by_brute_force(*from, *to, &occlusion);
            met += usize::from(truth);
            // An opaque body: meeting it takes the whole ray, and missing it
            // takes none. So the two walks' own `through` is the same binary the
            // oracle answers in, and no tolerance is needed to compare them.
            for (walk, through) in [
                ("walk_the_wire", walk_the_wire(*from, *to, lit, &occlusion).0),
                ("walk_the_record", walk_the_record(*from, *to, lit, &occlusion).0),
            ] {
                assert_eq!(
                    through == 0.0,
                    truth,
                    "{walk} on the ray at {name}: it says {}, brute force over every \
                     primitive says {}",
                    if through == 0.0 { "blocked" } else { "open" },
                    if truth { "blocked" } else { "open" },
                );
            }
        }
        // Six of the twelve are inside by construction — one a face — and a
        // fixture where every ray missed would be green for any wire at all.
        assert_eq!(
            met, 6,
            "six of the twelve rays are inside the box the record states; {met} were",
        );
    }

    // **`a_ray_starting_just_past_its_own_tile_is_stopped_by_the_cell_it_is_in`
    // stood here, and it went with `starting_cell`** — `docs/render/design_occluders.md`'s S4.
    //
    // Its whole fixture was a disagreement: a fragment carrying tile `(99, 100)`
    // while standing at `x = 100.0001`, so that a walk seeded from the carried
    // tile was handed a whole tile of slack to a boundary it had already crossed
    // and never looked at the cell it was standing in. With the carried tile
    // gone there is no second number to disagree with the position, and the test
    // was left asserting that a ray inside a wall is stopped by it — true, and
    // true whatever cell the walk starts in, since the ray reaches the wall's
    // own cell within a hair either way.
    //
    // **Measured rather than assumed, because a fixture that has lost its own
    // case goes on passing.** Under the injection that this deletion's licence
    // rests on — seeding a cell the point is not in — it stayed *green*, while
    // three unit tests and fourteen of `tests/lighting.rs` went red. That
    // is the definition of a test that no longer gates its subject, and this
    // track has now found three of them (the two vertical ones are in
    // `docs/render/design_occluders.md`'s S4). What replaces it is
    // `a_walk_starts_in_a_cell_its_own_start_point_is_in`, which asks
    // [`dda_walk`] for its own first cell and does go red under that injection.

    /// The same claim over a body whose `z` span is **not** a whole number,
    /// which is the case the three tests around this one cannot see at all.
    ///
    /// Every fixture they build goes through `Builder::add` off a `StaticTile`,
    /// so every span in them is a whole `z` and a half — and the two walks read
    /// *different* boxes for one solid on purpose ([`walk_the_record`] the
    /// record's own `f64` corners, [`walk_the_wire`] the wire's, see
    /// [`wire_span`]). On a whole `z` those two are equal by construction, so
    /// their agreement there says nothing about the discipline that keeps them
    /// close anywhere else: the assertion passes on a scene where the thing it
    /// checks cannot differ.
    ///
    /// A base and a top on thirds is what makes the wire's rounding actually
    /// happen — and the bar stays full numeric agreement, because the last bits
    /// of an `f32` are far under what any of this can be seen through. It was a
    /// quantisation to a two-hundred-and-fifty-sixth of a `z` unit until
    /// `docs/render/design_occluders.md`'s S1, and this test kept working across that change
    /// because it never named the size of the gap — only that there is one. See
    /// [`a_primitive_at_no_fraction_a_byte_could_name_reads_the_same_three_ways`]
    /// for the fixture that names it.
    #[test]
    fn walk_the_wire_agrees_with_walk_the_record_on_a_body_at_a_fractional_z() {
        use proptest::prelude::*;

        use crate::occlusion::Builder;

        let mut occlusion = Builder::new(crate::camera::TileBounds {
            min_x: 90,
            max_x: 110,
            min_y: 90,
            max_y: 110,
        });
        occlusion.add_raw(
            100,
            100,
            crate::solid::Solid {
                min: crate::camera::WorldSpot {
                    x: 100.0,
                    y: 100.0,
                    z: 1.0 / 3.0,
                },
                max: crate::camera::WorldSpot {
                    x: 101.0,
                    y: 101.0,
                    z: 20.0 - 1.0 / 3.0,
                },
            },
            crate::occlusion::Owner::new(0, openshard_protocol::wire::Graphic(1)),
        );
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        proptest!(ProptestConfig::with_cases(8_000), |(
            fx in 95.0_f32..105.0,
            fy in 95.0_f32..105.0,
            fz in 0.0_f32..20.0,
            tx in 95.0_f32..105.0,
            ty in 95.0_f32..105.0,
            tz in 0.0_f32..20.0,
        )| {
            prop_assume!((fx - tx).abs() > 1e-3 || (fy - ty).abs() > 1e-3);
            let from = [fx, fy, fz];
            let to = [tx, ty, tz];
            // **Not every ray can carry this claim, and which cannot is
            // decidable rather than a matter of taste.** The two walks read
            // boxes that differ by the wire's own `f32` rounding — on purpose,
            // that is the whole subject of this test — and a body's own answer
            // is binary, so wherever the ray's hit is *decided* inside that gap
            // the two must differ by everything rather than by a rounding. The
            // first case proptest found here was exactly that, back when the
            // gap was a quantisation rather than a rounding: a ray grazing the
            // box's own bottom-front corner, missing the record's own `1/3`
            // base and catching the wire's, a thousandth of a `z` unit lower.
            //
            // So the guard is the question itself: run the ray against the
            // solid's own record grown by that gap and shrunk by it, and skip
            // the case when those two disagree about hitting it at all. What is
            // left is every ray whose hit or miss survives the rounding, and
            // *those* must agree numerically. A tolerance on the input is not a
            // tolerance on the output, and this test asserted the second while
            // meaning the first.
            //
            // **The gap is measured off this very box rather than named as a
            // constant** — `docs/render/design_occluders.md`'s S1 took the quantisation away,
            // and what is left has no fixed size: a coordinate an `f32` holds
            // exactly (which is every whole `z` and every half, and therefore
            // most of the world) has a gap of nothing, and the guard skips
            // nothing for it.
            let solid = occlusion.solids_at(100, 100).next().expect("the fixture's own body");
            // The ray as the walks see it, which since phase 4 is the ray as
            // given: `stand_clear` stood here and the bias is zero now.
            let (near, far) = (from, to);
            let wire = solid.wire_box();
            let slack = [
                solid.space.min.x - wire.min.x,
                solid.space.min.y - wire.min.y,
                solid.space.min.z - wire.min.z,
                solid.space.max.x - wire.max.x,
                solid.space.max.y - wire.max.y,
                solid.space.max.z - wire.max.z,
            ]
            .into_iter()
            .fold(0.0_f64, |worst, gap| worst.max(gap.abs()));
            let hits_with = |grown: f64| {
                let mut space = solid.space;
                space.min.x -= grown;
                space.min.y -= grown;
                space.min.z -= grown;
                space.max.x += grown;
                space.max.y += grown;
                space.max.z += grown;
                ray_vs_solid(near, far, &space).is_some()
            };
            prop_assume!(hits_with(-slack) == hits_with(slack));

            let exact = walk_the_record(from, to, LitEnd::nowhere(), &occlusion).0;
            let streaming = walk_the_wire(from, to, LitEnd::nowhere(), &occlusion).0;
            prop_assert!(
                (exact - streaming).abs() < 1e-3,
                "from {from:?} to {to:?}: walk_the_record {exact} vs walk_the_wire {streaming}",
            );
        });
    }

    /// `docs/archive/render/lighting_raymarch.md`'s point 4, over a single **panel**
    /// (`Shape::faced`) — the branch [`walk_the_record_disagreements_are_
    /// backed_by_ray_vs_solid`]'s own doc comment flags as the one
    /// deliberate simplification (one [`pierced`] sample at the crossing's
    /// own midpoint). A panel's box is `PANEL_THICKNESS`-inset from the
    /// plane but still exactly what [`crate::occlusion::Solid::box_of`]
    /// builds for it, so full numeric agreement is the right bar here too,
    /// not the weaker "stronger answer is backed" claim the `walk_cells`
    /// comparison needed.
    #[test]
    fn walk_the_wire_agrees_with_walk_the_record_on_a_single_panel() {
        use proptest::prelude::*;

        use crate::facing::Facing;
        use crate::occlusion::{
            Builder,
            Shape,
        };

        let wall = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT),
            height: 20,
            ..StaticTile::default()
        };
        let mut occlusion = Builder::new(crate::camera::TileBounds {
            min_x: 90,
            max_x: 110,
            min_y: 90,
            max_y: 110,
        });
        occlusion.add(
            100,
            100,
            0,
            Graphic(0x0100),
            &wall,
            Shape::faced(Facing::One(Face::North)),
        );
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        proptest!(ProptestConfig::with_cases(8_000), |(
            fx in 95.0_f32..105.0,
            fy in 95.0_f32..105.0,
            fz in 0.0_f32..20.0,
            tx in 95.0_f32..105.0,
            ty in 95.0_f32..105.0,
            tz in 0.0_f32..20.0,
        )| {
            prop_assume!((fx - tx).abs() > 1e-3 || (fy - ty).abs() > 1e-3);
            let from = [fx, fy, fz];
            let to = [tx, ty, tz];
            let exact = walk_the_record(from, to, LitEnd::nowhere(), &occlusion).0;
            let streaming = walk_the_wire(from, to, LitEnd::nowhere(), &occlusion).0;
            prop_assert!(
                (exact - streaming).abs() < 1e-3,
                "from {from:?} to {to:?}: walk_the_record {exact} vs walk_the_wire {streaming}",
            );
        });
    }

    /// `docs/archive/render/lighting_raymarch.md`'s point 4, over a small room rather than
    /// one isolated wall — three walled sides, a doorway gap, and a
    /// free-standing body in the open area, seven solids on six different
    /// tiles at once. [`walk_the_wire`]'s own doc comment names this
    /// as the densest of the constructions that went looking for a case
    /// where plain single-axis DDA (no diagonal probe) misses a cell a real
    /// ray passes through, and did not find one — this is that construction,
    /// kept as a permanent regression rather than only run once by hand.
    #[test]
    fn walk_the_wire_agrees_with_walk_the_record_in_a_small_room() {
        use proptest::prelude::*;

        use crate::facing::Facing;
        use crate::occlusion::{
            Builder,
            Shape,
        };

        let wall = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT),
            height: 20,
            ..StaticTile::default()
        };
        let mut occlusion = Builder::new(crate::camera::TileBounds {
            min_x: 90,
            max_x: 110,
            min_y: 90,
            max_y: 110,
        });
        occlusion.add(
            100,
            99,
            0,
            Graphic(0x0100),
            &wall,
            Shape::faced(Facing::One(Face::North)),
        );
        occlusion.add(
            101,
            99,
            0,
            Graphic(0x0100),
            &wall,
            Shape::faced(Facing::One(Face::North)),
        );
        occlusion.add(
            99,
            100,
            0,
            Graphic(0x0100),
            &wall,
            Shape::faced(Facing::One(Face::West)),
        );
        occlusion.add(
            99,
            101,
            0,
            Graphic(0x0100),
            &wall,
            Shape::faced(Facing::One(Face::West)),
        );
        occlusion.add(
            102,
            100,
            0,
            Graphic(0x0100),
            &wall,
            Shape::faced(Facing::One(Face::East)),
        );
        occlusion.add(
            100,
            102,
            0,
            Graphic(0x0100),
            &wall,
            Shape::faced(Facing::One(Face::South)),
        );
        occlusion.add(101, 101, 0, Graphic(0x0100), &wall, Shape::UNREAD);
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        proptest!(ProptestConfig::with_cases(30_000), |(
            fx in 98.5_f32..103.5,
            fy in 98.5_f32..103.5,
            fz in 0.0_f32..20.0,
            tx in 98.5_f32..103.5,
            ty in 98.5_f32..103.5,
            tz in 0.0_f32..20.0,
        )| {
            prop_assume!((fx - tx).abs() > 1e-3 || (fy - ty).abs() > 1e-3);
            let from = [fx, fy, fz];
            let to = [tx, ty, tz];
            let exact = walk_the_record(from, to, LitEnd::nowhere(), &occlusion).0;
            let streaming = walk_the_wire(from, to, LitEnd::nowhere(), &occlusion).0;
            prop_assert!(
                (exact - streaming).abs() < 1e-3,
                "from {from:?} to {to:?}: walk_the_record {exact} vs walk_the_wire {streaming}",
            );
        });
    }

    /// `docs/archive/render/lighting_raymarch.md`'s point 4, over the three-tread climbable
    /// stair — and a **real, new-found boundary of the reconstruction**, not
    /// a smoke test alone.
    ///
    /// **Full agreement with [`walk_the_record`] does not hold here, and it
    /// should not — this is a second, independent source of the same gap
    /// session 14 already named, not a new one to chase.** A tread's top
    /// and riser are built by [`crate::occlusion::Solid::tread_top_box_of`]/
    /// [`crate::occlusion::Solid::tread_riser_box_of`] (`Prism::footprint`),
    /// not by [`crate::occlusion::Solid::box_of`] — they are sub-tile strips
    /// along the climb axis. A tread's `edges` is `0`, the same as an
    /// ordinary floor's, so [`walk_the_wire`]'s `box_of(tile, 0,
    /// ...)` reconstruction necessarily comes back the *whole* tile —
    /// correct for a real floor, wrong for a tread that covers a third of
    /// one. **Worth recording explicitly, not left implicit**: the "second
    /// bigger idea" gap session 14 measured against `Builder::add_raw`
    /// boxes is not only about hand-built test scenes — climbable stairs,
    /// already real content, hit the identical limit, by a second,
    /// independent path.
    ///
    /// **An honest attempt at a disagreement-backing oracle here (checking
    /// whether the tile either walk blames has a lossy `box_of`
    /// reconstruction) failed on its very first fuzz run, and the failure is
    /// itself informative, not a bug to chase.** `walk_the_record`'s own
    /// `stopped_by` names the *first* tile in ray order that fully blocked
    /// it; when it found nothing blocking at all (`through == 1.0`,
    /// `stopped_by == None`) there is no blamed tile to fall back to, and
    /// the tile a disagreement actually traces to can be anywhere a tread or
    /// riser's real, precise footprint the ray legitimately misses gets
    /// read by `walk_the_wire` as the *whole* tile instead. Building
    /// a sound oracle for that needs the same care session 11's own
    /// multi-solid disagreement oracle took for `exemption` — a real next
    /// step, not attempted here. So this checks what
    /// `walk_the_record_stays_in_range_on_the_stair` already checks for the
    /// exact walk itself: never panics, never returns a `through` outside
    /// `0.0..=1.0`, over the same broad fuzz — the lid-transparency and
    /// off-axis-probe-omission bugs either could have had would show up here
    /// as an out-of-range value or a panic, even without a numeric oracle to
    /// compare against.
    #[test]
    fn walk_the_wire_stays_in_range_on_the_stair() {
        use proptest::prelude::*;

        use crate::facing::Prism;
        use crate::occlusion::{
            Builder,
            Shape,
        };

        let stair = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT | TileFlags::CLIMBABLE),
            height: 20,
            ..StaticTile::default()
        };
        let prism = Prism::new(Face::West, &[1, 3, 5]).expect("three treads");
        let mut occlusion = Builder::new(crate::camera::TileBounds {
            min_x: 95,
            max_x: 105,
            min_y: 95,
            max_y: 105,
        });
        occlusion.add(100, 100, 0, Graphic(0x0736), &stair, Shape::solid(prism));
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        proptest!(ProptestConfig::with_cases(8_000), |(
            fx in 97.0_f32..103.0,
            fy in 97.0_f32..103.0,
            fz in 0.0_f32..6.0,
            tx in 97.0_f32..103.0,
            ty in 97.0_f32..103.0,
            tz in 0.0_f32..6.0,
        )| {
            prop_assume!((fx - tx).abs() > 1e-3 || (fy - ty).abs() > 1e-3);
            let from = [fx, fy, fz];
            let to = [tx, ty, tz];
            let streaming = walk_the_wire(from, to, LitEnd::nowhere(), &occlusion).0;
            prop_assert!((0.0..=1.0).contains(&streaming), "from {from:?} to {to:?}: through {streaming}");
        });
    }

    // **Two more of the DDA's own tests stood here**, and `docs/render/design_occluders.md`'s
    // S5 took the walk they were about:
    //
    // - `a_from_on_a_boundary_starts_in_the_cell_it_is_heading_into`, which was
    //   the last fixture on this track built entirely out of which of two cells
    //   a point on their shared boundary belongs to. S4's census counted 11,544
    //   of that case without a single answer moving; a tree does not ask the
    //   question at all.
    // - `dda_walk_visits_a_connected_path_of_cells_starting_where_the_ray_does`,
    //   the fast net the testability audit in `docs/archive/render/lighting_raymarch.md` argued
    //   for — every promise the DDA made about its own output, over arbitrary
    //   rays and with no scene: a connected path of von-Neumann neighbours, one
    //   axis a step, `entered`/`leaves` walking forward inside `0.0..=1.0`, and
    //   no more than `MAX_WALK_STEPS` of them.
    //
    // **What inherits that net is `occlusion::bvh`'s own tests**, and it is the
    // same discipline one layer down: the structural claims a traversal leans on
    // — every primitive under exactly one leaf, a node's box holding its whole
    // subtree, an escape index that is the end of that subtree — checked as
    // plain numbers with no scene, no `Occlusion` and no GPU. What is *not*
    // inherited is a claim about the order a ray meets things in, because a
    // traversal has none; what replaces it is that the order cannot matter, and
    // `walk_primitives` states that outright by taking the earliest crossing
    // rather than the first one found.

    /// A unit box built at `(0, 0)`, spanning `0..1` on every axis — the
    /// smallest real box [`ray_vs_solid`]'s hand-computed tests below share,
    /// so a straight line through its middle has fractions worth doing in
    /// one's head.
    fn unit_box() -> crate::solid::Solid {
        crate::solid::Solid {
            min: crate::camera::WorldSpot {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            max: crate::camera::WorldSpot {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
        }
    }

    /// [`ray_vs_solid`]'s ordinary case, checked against hand-computed
    /// fractions rather than trusted from the slab arithmetic: a ray along
    /// `x`, level in `y` and `z`, from `x = -1` to `x = 2` — it enters the
    /// box's `x = 0` face a third of the way along and leaves the `x = 1`
    /// face two thirds of the way along.
    #[test]
    fn ray_vs_solid_finds_the_hand_computed_crossing_of_a_unit_box() {
        let (entered, leaves) =
            ray_vs_solid([-1.0, 0.5, 0.5], [2.0, 0.5, 0.5], &unit_box()).expect("the ray crosses the box");
        assert!((entered - 1.0 / 3.0).abs() < 1e-5, "entered = {entered}");
        assert!((leaves - 2.0 / 3.0).abs() < 1e-5, "leaves = {leaves}");
    }

    /// A ray that never comes near the box misses cleanly.
    #[test]
    fn ray_vs_solid_misses_a_box_it_never_approaches() {
        assert_eq!(ray_vs_solid([-5.0, 5.0, 0.5], [5.0, 5.0, 0.5], &unit_box()), None);
    }

    /// Both ends of the ray already inside the box: the whole segment is a
    /// crossing, `entered` at `0.0` and `leaves` at `1.0` — no length of the
    /// segment is outside the box to clip away.
    #[test]
    fn ray_vs_solid_is_the_whole_segment_when_both_ends_are_inside() {
        let solid = crate::solid::Solid {
            min: crate::camera::WorldSpot {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            max: crate::camera::WorldSpot {
                x: 10.0,
                y: 10.0,
                z: 10.0,
            },
        };
        assert_eq!(
            ray_vs_solid([2.0, 3.0, 4.0], [5.0, 6.0, 7.0], &solid),
            Some((0.0, 1.0)),
        );
    }

    /// A degenerate box flat on `z` — a lid's own shape — still answers
    /// correctly: a ray that crosses the plane finds it at the fraction the
    /// plane's own `z` predicts, and a level ray that never reaches that
    /// height at all misses outright rather than reading as an edge case.
    #[test]
    fn ray_vs_solid_finds_a_flat_lid_only_exactly_on_its_own_plane() {
        let lid = crate::solid::Solid {
            min: crate::camera::WorldSpot {
                x: 0.0,
                y: 0.0,
                z: 20.0,
            },
            max: crate::camera::WorldSpot {
                x: 1.0,
                y: 1.0,
                z: 20.0,
            },
        };
        // Descends from z = 30 to z = 10 through the footprint's own
        // centre: crosses z = 20 exactly halfway.
        let (entered, leaves) =
            ray_vs_solid([0.5, 0.5, 30.0], [0.5, 0.5, 10.0], &lid).expect("crosses the lid's own plane");
        assert!((entered - 0.5).abs() < 1e-5);
        assert!((leaves - 0.5).abs() < 1e-5);

        assert_eq!(ray_vs_solid([0.5, 0.5, 25.0], [0.9, 0.9, 25.0], &lid), None);
    }

    /// A degenerate box narrow on one axis — a panel's own shape,
    /// [`crate::occlusion::PANEL_THICKNESS`] deep — is pierced over exactly
    /// that width of `t`, not the whole tile the panel stands on.
    #[test]
    fn ray_vs_solid_pierces_a_thin_panel_over_exactly_its_own_thickness() {
        let panel = crate::solid::Solid {
            min: crate::camera::WorldSpot {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            max: crate::camera::WorldSpot {
                x: 1.0,
                y: crate::occlusion::PANEL_THICKNESS,
                z: 20.0,
            },
        };
        // Straight along y, from y = -1 to y = 1 — the panel is
        // `PANEL_THICKNESS` of the segment's own 2.0 world units, so it
        // should be pierced for exactly that share of `t`.
        let (entered, leaves) =
            ray_vs_solid([0.5, -1.0, 10.0], [0.5, 1.0, 10.0], &panel).expect("crosses the thin panel");
        assert!((entered - 0.5).abs() < 1e-5);
        let expected_leaves = 0.5 + crate::occlusion::PANEL_THICKNESS as f32 / 2.0;
        assert!(
            (leaves - expected_leaves).abs() < 1e-5,
            "leaves = {leaves}, expected {expected_leaves}",
        );
    }

    /// A ray that only ever grazes one corner of the box — never a length of
    /// its inside — still gets an answer, not `None`: the same corner
    /// [`corner_tie`]'s own tolerance exists to approximate, answered
    /// exactly here instead. `entered` and `leaves` collapse to the same
    /// `t`, a real but zero-length crossing.
    #[test]
    fn ray_vs_solid_returns_a_zero_length_crossing_at_a_tangent_corner() {
        // A diagonal ray through exactly the unit box's own (1, 1) corner in
        // the ground plane, level in z.
        let (entered, leaves) =
            ray_vs_solid([0.0, 2.0, 0.5], [2.0, 0.0, 0.5], &unit_box()).expect("touches the corner");
        assert!(
            (entered - leaves).abs() < 1e-5,
            "entered = {entered}, leaves = {leaves}"
        );
    }

    /// [`ray_vs_solid`]'s own claim, checked against an independent
    /// characterisation rather than trusted from the slab arithmetic: a
    /// point sampled at a given `t` along the segment lies inside the box
    /// exactly when `t` is inside the interval this returns. The same
    /// point-in-box oracle discipline step 4's brute-force sampler already
    /// uses against the whole walk, applied here to the one primitive a
    /// future ray-vs-`Solid` walk would build on —
    /// `docs/archive/render/lighting_raymarch.md`'s ray-vs-Solid scoping, point 1.
    ///
    /// Boxes and segment endpoints are integer-anchored on purpose, so a
    /// random `t` landing within a hair of a face is rare rather than
    /// impossible: right at a face is exactly where a slab intersection and
    /// a naive point-in-box check can disagree by a rounding error smaller
    /// than either is precise to, which is not the disagreement this oracle
    /// is checking for — `near_a_boundary` skips only those samples.
    #[test]
    fn ray_vs_solid_agrees_with_plain_point_in_box_at_every_t() {
        use proptest::prelude::*;

        proptest!(ProptestConfig::with_cases(2048), |(
            min_x in -20_i32..20,
            min_y in -20_i32..20,
            min_z in -20_i32..20,
            size_x in 1_i32..6,
            size_y in 1_i32..6,
            size_z in 1_i32..6,
            from_x in -30.0_f32..30.0,
            from_y in -30.0_f32..30.0,
            from_z in -30.0_f32..30.0,
            to_x in -30.0_f32..30.0,
            to_y in -30.0_f32..30.0,
            to_z in -30.0_f32..30.0,
            t in 0.0_f32..1.0,
        )| {
            let min = [min_x as f32, min_y as f32, min_z as f32];
            let max = [min_x + size_x, min_y + size_y, min_z + size_z].map(|v| v as f32);
            let solid = crate::solid::Solid {
                min: crate::camera::WorldSpot {
                    x: f64::from(min[0]),
                    y: f64::from(min[1]),
                    z: f64::from(min[2]),
                },
                max: crate::camera::WorldSpot {
                    x: f64::from(max[0]),
                    y: f64::from(max[1]),
                    z: f64::from(max[2]),
                },
            };
            let from = [from_x, from_y, from_z];
            let to = [to_x, to_y, to_z];
            prop_assume!((0..3).any(|axis| (to[axis] - from[axis]).abs() > 1e-3));

            let point = [
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
                from[2] + (to[2] - from[2]) * t,
            ];
            let near_a_boundary = (0..3).any(|axis| {
                (point[axis] - min[axis]).abs() < 1e-3 || (point[axis] - max[axis]).abs() < 1e-3
            });
            prop_assume!(!near_a_boundary);
            let inside = (0..3).all(|axis| point[axis] >= min[axis] && point[axis] <= max[axis]);

            match ray_vs_solid(from, to, &solid) {
                Some((entered, leaves)) => {
                    let claims_inside = t >= entered && t <= leaves;
                    prop_assert_eq!(
                        inside, claims_inside,
                        "t {}, point {:?}, box {:?}..{:?}, interval {}..{}",
                        t, point, min, max, entered, leaves,
                    );
                }
                None => prop_assert!(
                    !inside,
                    "t {}, point {:?} reads inside a box the primitive missed entirely",
                    t, point,
                ),
            }
        });
    }
}
