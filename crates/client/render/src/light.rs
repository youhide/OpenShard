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
//! moment shadows exist at all. `docs/lighting.md` is the argument at length.
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
//! invention rather than port; see `docs/client.md`.
//!
//! # The flicker is on the CPU
//!
//! Two sine terms of incommensurable frequency, per light, sampled once per
//! frame and folded into the intensity that reaches the GPU. On the CPU because
//! a flame's brightness is one number for the whole pool — the shader would
//! recompute it identically for every pixel it touches — and because this crate
//! is not allowed to read a clock, so the time arrives as an argument and there
//! is exactly one place it is used.

use openshard_protocol::direction::Direction;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_uofiles::map::Map;
use openshard_uofiles::tiledata::TileData;

use crate::camera::Camera;
use crate::cutaway::{self, Cutaway};
use crate::facing::Face;
use crate::geometry::Vec2;
use crate::items::GroundItem;
use crate::occlusion::{EDGE_ANY, Occlusion};

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
    pub at: Vec2,
    /// Its height, in the map's own `z` units.
    pub z: f32,
    /// How far its pool reaches, **in tiles**. Nothing beyond this is touched at
    /// all, which is what keeps the shader's loop cheap and the pool a shape
    /// rather than a global tint.
    pub radius: f32,
    /// Its colour, linear, each channel in `0..=1`.
    pub color: [f32; 3],
    /// How brightly it burns at its centre, flicker already folded in. Above
    /// `1.0` is ordinary: a fire blows out the ground it stands on.
    pub intensity: f32,
    /// Which way it throws its light, where it throws it one way at all — see
    /// [`Beam`]. `None` is a fire in the open, which lights every direction
    /// equally, and it is what everything on the map is.
    pub beam: Option<Beam>,
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
/// unit vectors in the same units the distance is in — `x` and `y` in tiles and
/// `z` in tiles as well, which is [`Z_PER_TILE`]'s doing and is what keeps a
/// beam pointing along the ground from lighting the storey above.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Beam {
    /// Where it points, unit length. Built by [`Beam::towards`], which is the
    /// only thing that makes one — a direction of some other length would make
    /// the dot product below mean nothing.
    pub toward: [f32; 3],
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
            toward: [dx / length, dy / length, rise / length],
            cos_half: (degrees.to_radians() / 2.0).cos(),
        }
    }

    /// How much of this beam falls on a spot `offset` away from the flame —
    /// `x` and `y` in tiles, `z` in tiles as well, pointing *from* the flame
    /// *to* the spot.
    ///
    /// `blit.wgsl`'s `cone`, arithmetic for arithmetic, and the parity test of
    /// `docs/lighting.md`'s decision 9 is what says so. The smoothstep is
    /// written out rather than called, because WGSL's built-in and a Rust crate's
    /// are two texts that can disagree and this is one polynomial either way.
    ///
    /// Never zero: [`BEAM_SPILL`] is the floor, and a spot at the flame itself
    /// gets the whole of it — there is no direction from a point to itself, and
    /// the tile a lantern is standing on is not the place to start refusing
    /// light.
    pub fn lights(self, offset: [f32; 3]) -> f32 {
        let length = offset.iter().map(|axis| axis * axis).sum::<f32>().sqrt();
        if length < 1e-6 {
            return 1.0;
        }
        let along = offset
            .iter()
            .zip(self.toward)
            .map(|(axis, toward)| axis / length * toward)
            .sum::<f32>();
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
/// floor behind it. `docs/lighting.md`, decision 12.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Sun {
    /// Which way the sun is, from anywhere: `x` and `y` in tiles and `z` in
    /// tiles as well — the same unit the distance to a flame is in, so that an
    /// elevation of 45° really is one tile up per tile along. Normalised by
    /// [`Sun::towards`], which is the only thing that builds one.
    pub toward: [f32; 3],
    /// Its colour, linear.
    pub color: [f32; 3],
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
            toward: [dx / length, dy / length, rise / length],
            color,
            intensity,
        }
    }

    /// How steeply it climbs per tile along the ground: the slope
    /// [`Sun::towards`] was given back, whatever the direction was normalised to.
    pub fn rise_per_tile(self) -> f32 {
        let horizontal = (self.toward[0] * self.toward[0] + self.toward[1] * self.toward[1]).sqrt();
        match horizontal < 1e-6 {
            true => f32::INFINITY,
            false => self.toward[2] / horizontal,
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

/// The light a place has before anything burns in it: the sky's share, and the
/// floor under it.
///
/// `docs/lighting_world.md`, decision 1. One colour for the whole frame lit the
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
    pub sky: [f32; 3],
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
        sky: [1.0, 1.0, 1.0],
        ground: [0.0, 0.0, 0.0],
    };

    /// The same light with the sky's share folded into the floor: one colour for
    /// every tile, whatever stands over it.
    ///
    /// **The ambient this pass had before the sky field existed**, and the switch
    /// back to it is deliberate rather than a leftover. What a roof does to the
    /// light under it is a whole plan of its own
    /// (`docs/lighting_world.md`), and while the *point* lights are being got
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
    /// `docs/lighting.md`'s decision 9.
    pub fn at(self, sky: u8) -> [f32; 3] {
        let share = f32::from(sky) / f32::from(crate::occlusion::SKY_OPEN);
        let mut lit = self.ground;
        for (channel, sky) in lit.iter_mut().zip(self.sky) {
            *channel += sky * share;
        }
        lit
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
    pub ambient: Ambient,
    /// The flames themselves, nearest first and never more than
    /// [`Lighting::MAX`] of them.
    pub lights: Vec<Light>,
    /// What stands between them and the ground — see [`crate::occlusion`].
    ///
    /// Travels with the lights rather than beside them because it is the same
    /// frame's answer built from the same walk: a grid collected for one camera
    /// and used with another's flames would put shadows where the map has no
    /// walls.
    pub occlusion: Occlusion,
    /// The sun, where there is one — see [`Sun`]. `None` is night, or a frame
    /// that has not been given a sky yet, and costs nothing at all: the shader
    /// never walks a ray for it.
    pub sun: Option<Sun>,
    /// Which of the pass's own values to draw instead of the lit frame — see
    /// [`crate::debug::View`], and `docs/lighting.md`'s decision 8 for why the
    /// diagnostics are branches of this pass rather than a second one.
    ///
    /// Here rather than in [`crate::blit::Frame`] because it is read where the
    /// lights are read, out of the same uniform block, and a second channel into
    /// the same shader is a second thing to keep in step.
    pub view: crate::debug::View,
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
        ambient: Ambient::DAY,
        lights: Vec::new(),
        occlusion: Occlusion::EMPTY,
        sun: None,
        view: crate::debug::View::Lit,
    };

    /// Whether this would change a single pixel.
    ///
    /// The occluders *are* asked about now, and that is decision 1 of
    /// `docs/lighting_world.md` arriving here: a wall with no flame to stop
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
    /// — that is the whole of what it draws.
    pub fn is_identity(&self) -> bool {
        self.lights.is_empty()
            && self.ambient == Ambient::DAY
            && self.occlusion.is_empty()
            && self.view.is_lit()
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
/// Invented here, in the way `docs/lighting_world.md`'s decision 11 says every
/// number in this plan is: held by a scene, not argued into existence.
///
/// **Linear**, like every light quantity in this module since
/// `docs/lighting_rebuild.md`'s phase 1. It was authored as `[0.12, 0.13, 0.18]`
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
    sky: [0.033_105, 0.039_682, 0.078_288],
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
    sky: [0.154_872, 0.147_319, 0.162_647],
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
    pub radius: f32,
    /// Its colour, linear.
    pub color: [f32; 3],
    /// Its brightness at the centre, before the flicker multiplies it.
    pub intensity: f32,
    /// How much the flicker swings that brightness, as a fraction of it. A
    /// candle gutters; a bonfire mostly does not.
    pub flicker: f32,
}

/// A torch, a candle, a lantern: the ordinary flame, and what anything flagged
/// as a light source gets unless it is named below.
const TORCH: Flame = Flame {
    // Six tiles. The reference isometrics light a good deal more than the tile
    // the fire is on — a pool a tile wide reads as a bug, not as a torch.
    radius: 6.0,
    // Linear, authored as `[1.0, 0.72, 0.36]` at `0.95` — [`GROUND_AMBIENT`].
    color: [1.0, 0.477_000, 0.106_539],
    intensity: 0.890_005,
    flicker: 0.10,
};

/// A campfire: wider, brighter, steadier.
const CAMPFIRE: Flame = Flame {
    radius: 9.0,
    // Linear, authored as `[1.0, 0.66, 0.30]` at `1.25`. The intensity is past
    // the range sRGB is defined on, so it carries the curve's exponent alone:
    // `1.25^2.4`. A fire brighter than white is ordinary and is exactly what a
    // tonemap is for.
    color: [1.0, 0.393_123, 0.073_239],
    intensity: 1.708_378,
    flicker: 0.07,
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
pub fn burns(graphic: Graphic, tile: &openshard_uofiles::tiledata::StaticTile) -> bool {
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
const FLAME_LIFT: f32 = Z_PER_TILE / 2.0;

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
const LIGHT_MARGIN_TILES: i32 = CAMPFIRE.radius as i32 + 1;

/// The cells a frame's flames can come from: what is drawn, grown by the reach
/// of the widest pool. See [`LIGHT_MARGIN_TILES`].
///
/// Public because it is the rectangle *the grid is*, and a second caller that
/// wants the same grid must not guess at it: the app's occluder overlay
/// (`docs/lighting.md`, step 14) rebuilds the grid to draw it, and a wireframe
/// over a rectangle the shader did not walk is an instrument that lies about
/// exactly the edge it exists to show.
pub fn lit_tiles(camera: &Camera) -> crate::camera::TileBounds {
    let bounds = camera.visible_tiles();
    crate::camera::TileBounds {
        min_x: bounds.min_x - LIGHT_MARGIN_TILES,
        max_x: bounds.max_x + LIGHT_MARGIN_TILES,
        min_y: bounds.min_y - LIGHT_MARGIN_TILES,
        max_y: bounds.max_y + LIGHT_MARGIN_TILES,
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
// Nine, and every one of them is a different thing the frame knows: the world,
// what the server has put in it, where the eye is, what the client's files say,
// what the frame has cut away, what the sky is doing, when, the pictures, and
// what was built for the last frame. Grouping them into a struct would be one
// more type to keep in step with the call sites for no fewer facts.
#[allow(clippy::too_many_arguments)]
pub fn collect(
    map: &Map,
    items: &[GroundItem],
    camera: &Camera,
    tiledata: &TileData,
    cutaway: &Cutaway,
    ambient: Ambient,
    time: f32,
    atlas: Option<&crate::atlas::StaticAtlas>,
    bake: Option<&mut crate::occlusion::bake::Bake>,
) -> Lighting {
    let bounds = lit_tiles(camera);
    let mut lights = Vec::new();

    crate::statics::for_each_static_in(map, bounds, |item| {
        let tile = tiledata.static_tile(item.tile);
        if !burns(Graphic(item.tile), tile) || !cutaway::shows(cutaway, item.z, tile) {
            return;
        }
        lights.push(place(
            Point::new(item.x, item.y, item.z),
            flame(Graphic(item.tile)),
            time,
        ));
    });

    for item in items {
        let tile = tiledata.static_tile(item.graphic.0);
        if !burns(item.graphic, tile) || !cutaway::shows(cutaway, item.at.z, tile) {
            continue;
        }
        lights.push(place(item.at, flame(item.graphic), time));
    }

    // The grid before the flames are placed, because where a mounted flame burns
    // is a fact about what it is mounted *on* — see `mounted_at`.
    let occlusion = match bake {
        Some(bake) => crate::occlusion::bake::collect(bake, map, items, bounds, tiledata, cutaway, atlas),
        None => crate::occlusion::collect(map, items, bounds, tiledata, cutaway, atlas),
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
        ambient,
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
    }
}

/// How far outside the plane a mounted flame is placed, in tiles.
///
/// Half a tile takes it from its tile's centre to the plane the panel stands on,
/// and [`FACE_EDGE`] more takes it clear of the band the facing test softens
/// over — so the wall it hangs on is lit at full strength rather than at the half
/// a flame lying exactly in the plane would give it.
///
/// The consequence worth stating: it lands on the *next* tile, so the wall it is
/// mounted on stops being the flame's own cell and starts being an ordinary
/// occluder. That is what makes a sconce light the street and not the room behind
/// it, and it is the whole reason this is a move rather than an exemption.
const MOUNTED_CLEARANCE: f32 = 0.5 + FACE_EDGE;

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
/// `docs/lighting.md`'s backlog has carried the shape of this since the first
/// version of the pass — *"a lamp mounted on a wall wants pushing off it, not
/// exempting from it"* — and the grid already holds what it needs. Moving the
/// flame answers both, and it is what let the facing test lose its exemption for
/// a flame standing in a wall's line, which is a whole street long and lit every
/// wall in it.
///
/// A tile with no panel is not moved, and that covers the ordinary cases by
/// construction: a torch on the ground, a lamp post in the street, a brazier in a
/// room. So is a cell whose sides cancel — [`EDGE_ANY`](crate::occlusion::EDGE_ANY),
/// the whole-tile answer for a graphic the art would not name, and a lid — because
/// there is no direction in it to move along and a guess would be a wrong one.
fn mounted_at(at: Vec2, occlusion: &crate::occlusion::Occlusion) -> Vec2 {
    let Some(cell) = occlusion.at(at.x.floor() as i32, at.y.floor() as i32) else {
        return at;
    };
    // Componentwise and not along one normalised direction, so that a flame on a
    // **corner** — two panels, and every building has them — goes clear of both
    // planes rather than half clear of each.
    let toward = |positive: u8, negative: u8| match (cell.edges & positive != 0, cell.edges & negative != 0) {
        (true, false) => MOUNTED_CLEARANCE,
        (false, true) => -MOUNTED_CLEARANCE,
        // Neither side, or both: a lid, a whole-tile occluder, or a tile holding
        // two walls that face away from each other. No direction, no move.
        _ => 0.0,
    };
    Vec2::new(
        at.x + toward(crate::occlusion::EDGE_EAST, crate::occlusion::EDGE_WEST),
        at.y + toward(crate::occlusion::EDGE_SOUTH, crate::occlusion::EDGE_NORTH),
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
        at: Vec2::new(f32::from(at.x) + 0.5, f32::from(at.y) + 0.5),
        z: f32::from(at.z) + FLAME_LIFT,
        radius: flame.radius,
        color: flame.color,
        intensity: flame.intensity * flicker(time, phase_of(at), flame.flicker),
        // Every fire standing in the world burns in every direction. A beam is
        // something a hand does to a flame — see [`carried`].
        beam: None,
    }
}

/// How many cells of the grid one ray may look at.
///
/// `blit.wgsl`'s `MAX_WALK_STEPS`, and the two are one number: [`sample`] is the
/// shader's own arithmetic in Rust and a bound that differed would make the two
/// disagree exactly where a ray is longest. One number for both rays, because
/// [`walk_cells`] is one walk: a pool reaches nine tiles at the widest, a
/// sunbeam's segment runs [`MAX_SUN_TILES`], and a walk visits every cell the
/// segment crosses, which on a diagonal is both axes' worth. It is never actually
/// reached; it exists so that a loop over data cannot be made unbounded by a
/// radius somebody widens later.
pub const MAX_WALK_STEPS: i32 = 72;

/// How far a ray must travel inside an occluding cell for that cell to stop all
/// it can, in tiles. `blit.wgsl`'s `SOFT_CROSSING`.
///
/// The walk knows the length of each cell it crosses, and spending it is what
/// makes a shadow's edge a gradient rather than a step at a tile boundary: a ray
/// that clips a wall tile's corner keeps most of its light, one that crosses the
/// tile squarely keeps none.
///
/// It is not one length. A flame is a body, not a point, so an occluder close to
/// what it shadows draws a sharp edge and a distant one draws a wide penumbra
/// whose width is the flame's own size times `t / (1 - t)` — `t` being how far
/// along the ray the occluder is from the lit end. That is where these three
/// numbers go: [`FLAME_SPREAD`] is the size in tiles, and the bounds keep the
/// ends of the ratio finite. Invented here, the way [`crate::occlusion::PANE`]
/// is — no client file says how big a flame is.
const FLAME_SPREAD: f32 = 1.0;

/// The narrowest a shadow's edge gets: an occluder the fragment is against.
const SOFT_CROSSING_MIN: f32 = 0.05;

/// And the widest, for an occluder almost at the flame.
const SOFT_CROSSING_MAX: f32 = 0.7;

/// Below this, a ray has been stopped: `blit.wgsl`'s early exit, and under a
/// byte's worth of light either way.
const RAY_CUTOFF: f32 = 0.004;

/// How much of a panel a ray pierces at height `z` runs into: `1.0` well inside
/// the span it occupies, `0.0` well outside, and a gradient `tall` `z` units wide
/// across its edges.
///
/// The vertical half of decision 14's penumbra, and all that is left of it: a
/// flame is a body rather than a point, so a ray grazing the top of a wall is
/// dimmed rather than switched.
///
/// The band is centred on the *top* edge and hangs below the bottom one, for the
/// reason `blit.wgsl`'s `pierces` states at length: a wall is based on the ground
/// it stands on and the ray a person looks at runs along that base, so a band
/// centred there would let half of every flame along every wall in the frame.
///
/// `blit.wgsl`'s `pierces`, and the two are one formula.
fn pierces(z: f32, low: f32, high: f32, tall: f32) -> f32 {
    let band = tall.max(1e-3);
    ((z - low + band * 0.5).min(high - z) / band + 0.5).clamp(0.0, 1.0)
}

/// How much of a **lid** is in the way of a ray that runs from `from` to `to` in
/// `z` across one cell: `1.0` where the ray went through the plane and out the
/// other side, `0.0` where it stayed on one side of it, and a gradient between
/// where the flame itself straddles the plane.
///
/// **A lid is a plane and not a slab, and that is the whole of why this is not
/// [`pierces`] and not the length rule beside it.** A floor is `height 0` in
/// `tiledata.mul` — 4,534 of the 4,647 lids over the block of Britain
/// `artscan`'s `column` example reads — so its span is zero deep, and a rule that
/// scales what an occluder stops by how far the ray ran *inside* the span gets
/// zero out of every floor in the world. That is what lit the storey above a
/// torch through its own floorboards; see `scene::storey_over_a_torch`.
///
/// The crossing test is **strict**, and that is the one thing here that has to be
/// argued rather than stated. A ray that runs exactly along the top of a lid — a
/// candle standing on the floor it lights, both at one `z` — has not gone through
/// anything, and a test that counted a touch would put half a floor's shadow
/// across every room lit from inside it. It is [`pierces`]'s asymmetry arriving
/// at the surface that has no thickness for a band to hang under.
///
/// The softness is the flame's own size and is measured at the *flame*: the plane
/// cuts the source, so what gets through is the share of it left on the lit
/// side. `source` is the ray's far end in `z` and `spread` how big the flame is,
/// in tiles — a sunbeam passes `0.0` and gets the hard edge a point source casts.
///
/// **How tall that flame is, though, is [`FLAME_DEPTH`] and not its width.** The
/// two were one number for a day and it lit the storey over every wall sconce in
/// Britain: a sconce burns four or five `z` under the floor above it, and a flame
/// eleven `z` tall — [`FLAME_SPREAD`] of one tile, which is the *lateral*
/// softness of a shadow edge — pokes a tenth of itself through the boards.
///
/// `blit.wgsl`'s `crosses`, and the two are one formula.
fn crosses(entering: f32, leaving: f32, low: f32, high: f32, source: f32, spread: f32) -> f32 {
    let (under, over) = (entering.min(leaving), entering.max(leaving));
    if under >= high || over <= low {
        return 0.0;
    }
    // How far past the lid the flame itself stands, on the side the ray left by.
    let beyond = match leaving >= entering {
        true => source - high,
        false => low - source,
    };
    (beyond / (spread * FLAME_DEPTH).max(1e-3) + 0.5).clamp(0.0, 1.0)
}

/// How tall a flame is, in `z` units, for the one question that asks: how much of
/// it a floor cuts off ([`crosses`]).
///
/// A **quarter of a tile**, and the art is what says so rather than another
/// constant: the projection draws four screen pixels to one `z`
/// ([`crate::camera::Z_STEP`]), and the flame a torch graphic actually has drawn
/// on it is eight or ten pixels tall — two and a half `z`. Half a tile was the
/// first answer here, taken from [`FLAME_LIFT`] because it was the only number
/// in the file about a flame's height, and it is twice what the pictures show.
///
/// What the difference is worth, on the corner of Britain's house at
/// `1509,1635`: a ray passing three quarters of a `z` under the top of the wall
/// beside it keeps `0.31` of its light at half a tile, `0.11` at a quarter and
/// nothing at an eighth. The middle one is the picture; the third would be
/// choosing the number to make one pixel dark.
///
/// Scaled by the caller's `spread` so that a point source stays a point: the sun
/// passes `0.0` and a plane cuts it cleanly, which is what a floor's own shadow
/// on the ground under it is made of.
///
/// **It is what turns a softness in tiles into one in `z`, everywhere.** A
/// penumbra is the size of the source *across the edge it spills over*, and
/// every edge this pass softens vertically — a wall's top, a hole's sill, a
/// lid's plane — is horizontal, so what blurs it is how tall the flame is and
/// not how wide. [`Z_PER_TILE`] did that conversion until a house's corner was
/// measured: a ray passing three quarters of a `z` under the top of a wall kept
/// two fifths of its light, because the band was seven and a half `z` — a flame
/// as tall as it is wide. The lateral softness is unchanged and still
/// [`FLAME_SPREAD`]'s; it is the axis that was being asked the wrong question.
const FLAME_DEPTH: f32 = Z_PER_TILE / 4.0;

/// How far in front of its own plane a **face** pixel is walked from, in tiles.
///
/// `statics.wgsl` places one at `INSIDE` — a hundred-and-twenty-seventh short of
/// the plane — because a fraction of exactly one names the *next* tile and the
/// attachment's tile is what a click selects. That is right for the attachment
/// and wrong for the walk: the pixel is drawn on the plane, the space it is lit
/// from is in front of it, and the floor whose edge meets that plane belongs to
/// the tile in front. Eight thousandths of a tile behind the boundary was enough
/// for a ray to cross a storey's floor *before* reaching the cell that floor is
/// in, which is the bright line a house wore along its floorboards.
///
/// Two steps of the seven-bit fraction, which is what it takes to get past the
/// boundary from `INSIDE` and is a third of a pixel of world. Only the walk moves
/// — the attachment still names the wall's own tile, so picking, the debug views
/// and the wireframe are untouched.
const STAND_OFF: f32 = 2.0 / 127.0;

/// And how far **above** whatever it lies on every point of the world is walked
/// from, in `z`.
///
/// The other half of the same sentence, and the half a lid needs: a plane is
/// crossed rather than travelled through ([`crosses`]), and the test is strict,
/// so a point whose `z` is exactly a floor's lies *in* that floor and a ray from
/// it to a flame below runs along the plane rather than through it. A pixel is
/// drawn on top of the boards, not in them; so is a candle standing on them,
/// which is why this moves the flame's end too.
///
/// Well under one `z` unit — the attachment quantises `z` to whole ones — and
/// well over the last bits of a float.
const ON_TOP: f32 = 1.0 / 128.0;

/// The two ends of a ray, moved off the surfaces they are drawn on: see
/// [`STAND_OFF`] and [`ON_TOP`].
///
/// The flame's end gets the height and not the offset. A mounted flame is
/// already outside the plane its tile names — decision 26's `mounted_at`, which
/// moves it by a good deal more than this — and a flame has no face of its own to
/// be in front of.
fn stand_clear(from: [f32; 3], to: [f32; 3], surface: Surface) -> ([f32; 3], [f32; 3]) {
    let [ahead, across] = match surface.face() {
        Some(face) => face.outward(),
        None => [0.0, 0.0],
    };
    (
        [
            from[0] + ahead * STAND_OFF,
            from[1] + across * STAND_OFF,
            from[2] + ON_TOP,
        ],
        [to[0], to[1], to[2] + ON_TOP],
    )
}

/// Whether a lit point lies **on** a surface: its `z` is inside the span that
/// surface occupies, its two edges included.
///
/// What the exemptions are asked, one surface at a time. "A surface does not
/// shadow itself" needs to know which surface a pixel *is* a point of, and a
/// tile of a two-storey house holds a wall for each storey: `0..20` and
/// `20..40`, two surfaces, and a pixel at `z 25` is on the second. The first is
/// under its feet and occludes it exactly as anybody else's wall would.
///
/// Inclusive at both ends on purpose: a wall's base is the ground it stands on
/// and its top is the cap somebody's floor pixel is lying on, and a pixel is a
/// point of the surface it is drawn from at both.
///
/// **And inclusive by [`ON_TOP`]**, which is the same nudge [`stand_clear`] gave
/// the point and has to be given back here. A pixel of a wall's top cap is at
/// exactly the wall's own `top`; moved a hair above it and asked without the
/// tolerance, it stopped being a point of its own wall and the wall shadowed it —
/// the room's own wall went dark at the one height its cap is drawn at.
///
/// `low`/`high` are the solid's `z` span and **not** its
/// [`bottom`](crate::occlusion::Solid::bottom)/`top`, which is
/// `docs/lighting_height.md` phase 2: each walk hands the span it is entitled to
/// read — [`walk_cells_exact`] the record's own exact one, [`walk_cells_streaming`]
/// the one the GPU can reconstruct off the wire — instead of this deciding for
/// both by rounding.
///
/// `blit.wgsl`'s `on_surface`.
fn on_surface(z: f32, low: f32, high: f32) -> bool {
    z >= low - ON_TOP && z <= high + ON_TOP
}

/// Whether a **lid** is a plane the fragment is drawn *on*, rather than one
/// standing between it and the flame — `docs/lighting_height.md` phase 4's rule,
/// and the whole of the geometry in it.
///
/// > A contact at the ray's origin does not count. A crossing at `t > 0` counts,
/// > whoever owns the solid.
///
/// Two facts, and no tolerance in either.
///
/// **The lid is a plane** — `low == high`, which is what an ordinary floor and a
/// tread's own top both are. Not a special case but the condition that makes "a
/// contact at the origin does not count" and "this primitive does not count" one
/// sentence: a plane is crossed at a single point, so a ray leaving one crosses
/// it at its own origin and nowhere else, and there is no later crossing left for
/// an exemption to swallow. A lid with a real depth is a *slab* — a sloped roof
/// section, [`crate::occlusion::Solid::box_of`]'s own comment — and a ray that
/// descends into one from its own top face genuinely travels through it. That
/// crossing is not at the origin, so it still counts, and this says `false` for
/// it rather than pretending the two shapes are one.
///
/// **And the fragment is drawn at that plane's own height.** `drawn` and not the
/// ray's start: [`stand_clear`]'s [`ON_TOP`] is the *walk's* nudge and not the
/// fragment's, and a question about which surface a fragment **is** has to be
/// asked where the fragment is. Exact equality and not [`on_surface`] for the
/// same reason — the nudge is the only thing there was ever a tolerance for
/// here, and both sides of this are exact on the wire: a lid's `z` is an integer
/// or the fraction [`crate::occlusion::Occlusion::solid_z_bytes`] ships, and a
/// fragment's is the sixteenth [`crate::place::unpacked_height`] hands back.
///
/// **It is not on its own the rule.** [`exemption`] asks it only of a solid that
/// carries the fragment's own owner on the fragment's own cell, and that gate is
/// load-bearing rather than tidiness: a wall's face pixel at exactly the `z` of
/// the floor its wall stands on is drawn at that floor's height too, and
/// exempting *it* is the bright stroke a house wore along its floorboards that
/// [`ON_TOP`] was added to close. Being at a plane's height is not being a point
/// of it; being at it **and owned by it** is.
fn drawn_on(drawn: f32, low: f32, high: f32) -> bool {
    low == high && drawn == low
}

/// A soft interval: `1.0` well inside `low..=high`, `0.0` well outside, and a
/// gradient `band` wide across each edge.
///
/// [`pierces`] with its one asymmetry taken out, and the asymmetry is why this
/// is a second function rather than a call of the first. A wall's *bottom* edge
/// is the ground it stands on and the ray a person looks at runs along it, so
/// that band hangs below rather than straddling. **A hole's edges are in the
/// middle of a surface** and no ray runs along them by construction, so a hole
/// softens the same amount in both directions or it is a hole that has been
/// moved half a penumbra downwards.
///
/// `blit.wgsl`'s `inside`, and the two are one formula.
fn inside(x: f32, low: f32, high: f32, band: f32) -> f32 {
    let band = band.max(1e-3);
    ((x - low).min(high - x) / band + 0.5).clamp(0.0, 1.0)
}

/// Where along a panel's own run a point of it lies, `0.0` to `1.0` across the
/// tile.
///
/// A panel on a north or south side lies in a plane of constant `y`, so what
/// runs along it is `x`; an east or west one is the other way round. That is the
/// whole of the surface's own coordinate system, and it is why an
/// [`Aperture`](crate::occlusion::Aperture) belongs only to a *named* panel: a
/// lid and a body have no run for this to be measured along.
///
/// `blit.wgsl`'s `run_v`.
fn run_v(edges: u8, px: f32, py: f32) -> f32 {
    let along = match edges & (crate::occlusion::EDGE_NORTH | crate::occlusion::EDGE_SOUTH) != 0 {
        true => px,
        false => py,
    };
    along - along.floor()
}

/// How much of a surface is **missing** where a ray goes through it: `1.0` well
/// inside the hole, `0.0` well outside it, and the same penumbra across its
/// edges that the top of a wall gets.
///
/// The two spans are combined with `min` and not with a product, so that the
/// corner of a hole is softened once rather than twice: a point that is halfway
/// into the hole across *and* halfway up is on the diagonal of one corner, and
/// two halves multiplied would make it a quarter of a hole.
///
/// `blit.wgsl`'s `hole`.
fn hole(aperture: Option<crate::occlusion::Aperture>, v: f32, z: f32, wide: f32, tall: f32) -> f32 {
    let Some(hole) = aperture else {
        return 0.0;
    };
    let across = inside(
        v,
        f32::from(hole.near) / crate::occlusion::RUN_STEPS,
        f32::from(hole.far) / crate::occlusion::RUN_STEPS,
        wide,
    );
    across.min(inside(z, hole.bottom as f32, hole.top as f32, tall))
}

/// How much of a surface stands in the way at the point a ray goes through its
/// plane: the span it occupies, less the hole in it.
///
/// The whole of step 21.3 in one line, and the reason it is one line is decision
/// 30.7: a panel was already *pierced at a point* rather than travelled through,
/// so the point was already being computed and a window is what that point is
/// asked about. `cross` is where the ray crosses, in all three — one point and
/// not three loose coordinates, which is what `blit.wgsl`'s own `vec3<f32>`
/// parameter has always been and what both callers already had in hand; `wide`
/// is the penumbra in tiles along the run and `tall` the same number in `z`.
///
/// `low`/`high` are the panel's own `z` span, passed in for the reason
/// [`on_surface`]'s are.
///
/// `blit.wgsl`'s `pierced`.
fn pierced(
    stands: &crate::occlusion::Solid,
    low: f32,
    high: f32,
    cross: [f32; 3],
    wide: f32,
    tall: f32,
) -> f32 {
    let across = pierces(cross[2], low, high, tall);
    match stands.aperture {
        None => across,
        Some(_) => {
            across
                * (1.0
                    - hole(
                        stands.aperture,
                        run_v(stands.edges, cross[0], cross[1]),
                        cross[2],
                        wide,
                        tall,
                    ))
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
/// `docs/lighting_raymarch.md`'s ray-vs-Solid scoping, point 1: an exact
/// test costs a handful of compares, so nothing upstream needs to *guess*
/// whether a corner is worth asking about before asking it — this asks
/// directly and answers exactly, for a box as thin as a panel's own
/// [`crate::occlusion::PANEL_THICKNESS`] slab or as flat as a lid's bare
/// plane. [`walk_cells_streaming`] is what calls it now, mirrored in
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
/// [`walk_cells_streaming`]'s own caller, was tried and reverted.**
/// `docs/lighting_raymarch.md`'s point 4 cutover found a real GPU/CPU
/// disagreement traced to here (see `blit.wgsl`'s own comment for the case),
/// but rescuing the same near-miss on the CPU side clamped `leaves` up to
/// `entered`, collapsing a genuine, if small, interior crossing to a
/// zero-length touch and changing what the surrounding `by_surface` branches
/// computed for it. Because [`walk_cells_exact`]'s `candidate_tiles` probes a
/// wider set of candidate cells than [`walk_cells_streaming`]'s own plain
/// single-axis stepping ever visits — deliberately, session 8's own scoping
/// — a rescued near-miss on a cell only one of the two walks reaches turned
/// a shared, unconditional widening into a *new* disagreement between them,
/// not a fix to one; `walk_cells_streaming_agrees_with_walk_cells_exact_on_
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
/// `docs/lighting_raymarch.md`'s "hard shadows" decision. `ray_vs_solid`'s
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

/// Whether a point stands over a solid's own horizontal footprint.
///
/// [`ray_vs_solid`]'s parallel-axis branch, on the two horizontal axes alone,
/// and deliberately the same rule rather than a second one: a segment that does
/// not move along an axis either sits inside the box's span on that axis for its
/// whole length or misses the box entirely, and both ends count as inside there
/// exactly as they do here.
///
/// **Why the halves are split** instead of the vertical-ray shortcuts simply
/// calling [`ray_vs_solid`]: a vertical ray's *height* answer is [`crosses`]'s,
/// and that one is soft. A flame is a body `spread` deep rather than a point, so
/// a lid a little past the end of the ray still takes part of it, and a lid the
/// ray ends exactly on takes half. `ray_vs_solid` answers the same question
/// hard, and using it whole would erase the penumbra the shortcut's own
/// [`crosses`] call exists to compute — trading one defect for a visible one.
fn over_footprint(at: [f32; 3], solid: &crate::solid::Solid) -> bool {
    at[0] >= solid.min.x as f32
        && at[0] <= solid.max.x as f32
        && at[1] >= solid.min.y as f32
        && at[1] <= solid.max.y as f32
}

/// Which of a cell's sides are **the same wall the lit end is part of**, and
/// therefore must not shadow it.
///
/// `blit.wgsl`'s `own_run` argues it: a wall's face lies *on* the panel it is the
/// face of, so a ray leaving a wall pixel along the wall grazes the panels of the
/// tiles either side of it, and whether that counts as a crossing is decided by
/// the last bits of a float. It drew a thin dark stroke down every tile seam of
/// any wall lit by a lamp standing near it. A run of wall is one surface and no
/// part of a surface shadows another part of it.
///
/// Only the panels on the *same line*: the same row for a north or south face,
/// the same column for an east or west one. A wall tile that also carries the
/// perpendicular face of a corner stops the ray on that face as it always did.
fn own_run(own: u8, cell: (i32, i32), first: (i32, i32)) -> u8 {
    let mut line = 0;
    if cell.1 == first.1 {
        line |= crate::occlusion::EDGE_NORTH | crate::occlusion::EDGE_SOUTH;
    }
    if cell.0 == first.0 {
        line |= crate::occlusion::EDGE_EAST | crate::occlusion::EDGE_WEST;
    }
    own & line
}

/// The end of a ray that is a *surface*: which way it looks, which occluder of
/// its own tile it is a point of, and which tile that is.
///
/// Three facts that only ever travel together — every walk here folds all three
/// into one [`ExemptionContext`] and reads none of them apart — and that must
/// agree: a `surface` off one fragment with an `owner` off another is a
/// combination nothing in the world produces and every walk would answer for.
/// [`Spot`] is the same three beside a position; a walk takes the position
/// separately because [`stand_clear`] has already moved it.
#[derive(Clone, Copy)]
struct LitEnd {
    surface: Surface,
    owner: crate::occlusion::OwnerId,
    tile: (i32, i32),
}

impl LitEnd {
    /// The lit end a [`Spot`] is.
    fn of(spot: Spot) -> Self {
        Self {
            surface: spot.surface,
            owner: spot.owner,
            tile: spot.tile,
        }
    }

    /// A point of nothing on `tile`, looking nowhere in particular — what a
    /// test that is about the geometry alone means.
    #[cfg(test)]
    fn nowhere(tile: (i32, i32)) -> Self {
        Self {
            surface: Surface::Flat,
            owner: crate::occlusion::OwnerId::NONE,
            tile,
        }
    }
}

/// Ray-level facts [`exemption`] needs that do not change per candidate tile
/// or per solid — built once, before [`walk_cells_exact`]/[`walk_cells_streaming`]'s
/// own loop starts, rather than threaded through it argument by argument.
///
/// `owner` is the lit end's own — which occluder of `first` the fragment is a
/// point of, or [`crate::occlusion::OwnerId::NONE`] for a point of none. `spot_z`
/// is the ray's own start `z`, after [`stand_clear`]'s nudge, and `to_z` is the
/// far end's; both are heights, and after `docs/lighting_height.md` phase 3 the
/// only things left that read them are the two questions identity cannot answer
/// — see [`exemption`].
///
/// `drawn_z` is the *same* end's height **before** that nudge, and the two are
/// both here because they answer different questions. `spot_z` is where the ray
/// starts, which is the right thing to ask "did this ray run along that panel";
/// `drawn_z` is where the fragment is, which is the only thing that can answer
/// "is that plane the one I am drawn on" — see [`drawn_on`]. A single field would
/// have to be one or the other, and phase 4's whole defect is a nudge of a
/// hundred-and-twenty-eighth answering the second question as though it were the
/// first.
struct ExemptionContext {
    first: (i32, i32),
    last: (i32, i32),
    skip_last: bool,
    own: u8,
    owner: crate::occlusion::OwnerId,
    spot_z: f32,
    drawn_z: f32,
    to_z: f32,
}

/// Which of this tile's own sides are exempt because [`own_run`] says so, and
/// whether `stands` itself is exempt from occluding this ray at all.
struct Exemption {
    /// A surface does not shadow itself: see [`exemption`]'s own `lit_end` and
    /// `flame_end` for the two cases this covers.
    exempt: bool,
    /// [`own_run`]'s answer for this solid — needed by the caller even when
    /// `exempt` is `false`, since a run does not shadow itself panel by
    /// panel either.
    same_run: u8,
}

/// Whether one solid is exempt from shadowing this ray, and what of a run of
/// wall on its cell is.
///
/// `low`/`high` are the solid's own `z` span, the walk's to choose — see
/// [`on_surface`] — and `owner` is which occluder of its cell this solid is,
/// off the reference the walk followed to reach it.
///
/// # A fragment says which solid it is a point of, rather than being guessed at
///
/// `docs/lighting_height.md` phase 3. `lit_end` used to ask whether the
/// fragment's own height fell inside this solid's span, and take that for "this
/// is the solid the fragment is drawn from". It is a proxy and it fails in both
/// directions:
///
/// - Two things **stacked** on one tile meet at a single plane, and no precision
///   separates the lower one's top from the upper one's base — phases 1 and 2
///   shrank that ambiguity to a quantum of height without removing it, because
///   it is structural.
/// - Two things **side by side** on one tile span the same heights outright, so
///   every fragment of either is inside both spans, and each was exempted from
///   being shadowed by the other while standing squarely in front of it. That is
///   `examples/boxes.rs`'s `pair` scene, three oracles fully red.
///
/// Now the two sides carry the same fact and it is compared:
/// [`crate::occlusion::OwnerId`], unique within a tile, stamped into a solid's
/// own reference by [`crate::occlusion::Builder::finish`] and into the drawn
/// fragment's instance row by the pass that drew it.
/// [`crate::occlusion::OwnerId::same`] and not `==`, since a point of nothing is
/// not a point of the same nothing another solid is.
///
/// # What still reads a height, and why each is not identity's question
///
/// - **`flame_end`.** The far end of the ray is a flame, not a fragment, so
///   there is no owner to compare — a mounted flame stands on a solid nothing
///   drew. That is [`mounted_at`]'s question rather than this one's.
/// - **`same_run`.** A ray leaving a wall pixel *along* the wall grazes the
///   neighbouring tiles' panels, which are different statics and therefore
///   different owners. That is not identity at all, it is one surface cut on a
///   tile boundary, and [`own_run`] is what stands in for it until a scene that
///   can show it exists — `pair` is one tile and cannot.
/// - **[`drawn_on`].** Phase 4, and the one that is not a proxy for identity but
///   a *refinement* of it. Identity is per static and a static is several planes:
///   one [`crate::occlusion::Builder::add`] of a flight pushes a lid and a panel
///   per tread, all carrying one owner, and a fragment is a point of exactly one
///   of them. For a panel that does not matter — a surface does not shadow
///   another part of itself, so the whole static is excused — but a flight's
///   tread tops genuinely shadow each other, so which *plane* the fragment is on
///   has to be said. Nothing on the wire says it (see that function, and
///   `docs/lighting_height.md` phase 4 for what it would cost to), so the height
///   answers it, exactly and for a plane only.
fn exemption(
    ctx: &ExemptionContext,
    cell: (i32, i32),
    stands: &crate::occlusion::Solid,
    owner: crate::occlusion::OwnerId,
    low: f32,
    high: f32,
) -> Exemption {
    let own_cell = cell == ctx.first;
    let same_run = match on_surface(ctx.spot_z, low, high) {
        true => own_run(ctx.own, cell, ctx.first),
        false => 0,
    };
    // The whole of "this surface is one I am a point of": the solid stands on my
    // own cell, and it is the occluder of that cell I was drawn from. No height
    // on either side of it — which is what the two counts in
    // `docs/lighting_height.md`'s phase 3 table are the measurement of.
    let lit_end = own_cell && ctx.owner.same(owner);
    let flame_end = ctx.skip_last && cell == ctx.last && on_surface(ctx.to_z, low, high);
    // **Which of the static's surfaces the fragment is a point of, and not merely
    // which static.** `docs/lighting_height.md` phase 4. An owner is per
    // [`crate::occlusion::Builder::add`] and a static is several solids — a
    // flight pushes a lid and a panel per tread, all carrying one owner — so
    // identity alone excuses a fragment from surfaces that genuinely stand
    // between it and the flame. A flight's own treads shadow each other, and its
    // own risers shadow its tread tops; that is its body, not a self-shadow.
    //
    // Nothing on the wire names the solid (see this function's own doc), so each
    // shape is asked the one exact question the wire *can* answer about it. Both
    // answers are facts the fragment already carries, compared against facts the
    // solid already carries; neither is a tolerance and neither is a height
    // standing in for identity, which is what phase 3 removed.
    let is_mine = match stands.edges {
        // A **lid**: the fragment is drawn at this plane's own height. See
        // [`drawn_on`], which is the whole of phase 4's rule.
        0 => drawn_on(ctx.drawn_z, low, high),
        // A **body** — a whole tile that stands up and whose art would not say
        // which way. It has no face for a fragment to be a point of one of, and a
        // fragment of one carries [`crate::place::Stance::Upright`] for exactly
        // that reason, so identity is the whole of the answer here. That is what
        // `examples/boxes.rs`'s `pair` measured phase 3 against.
        EDGE_ANY => true,
        // A **panel**: the fragment is drawn on the side this one stands on. A
        // static that pushed a named panel gave its fragments a face to carry —
        // [`crate::place::Stance::of`] hands a face to exactly the statics
        // [`crate::occlusion::edges_of`] hands a named edge — so `own` is a fact
        // and not a fallback, and a fragment of a *lid* of the same static
        // carries no side at all and is a point of no panel of it.
        edges => edges & ctx.own != 0,
    };
    let exempt = (lit_end && is_mine) || (stands.edges != 0 && flame_end);
    Exemption { exempt, same_run }
}

/// A point in the world, as the lighting sees one: a fractional tile and a `z`.
///
/// Fractional because that is what the place attachment carries — where in its
/// tile a pixel is, to a hundred-and-twenty-eighth — and a pool is a gradient
/// only because of it. See [`crate::place`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Spot {
    /// Tile coordinates, the fraction being where in the tile the point is.
    pub at: Vec2,
    /// Its height, in the map's own `z` units.
    pub z: f32,
    /// The tile this is a point of — **not** `at.x.floor()`/`at.y.floor()`.
    /// A point legitimately sits on a tile's own far edge (a stair tread's
    /// outer corner, `at.x` exactly whole) and `floor()` there picks whichever
    /// side happens to round down, not the side the geometry actually stands
    /// on. Every caller already knows which tile it means; carrying it here
    /// instead of re-deriving it in [`walk_cells`] is the CPU twin of
    /// `MeshFaceVertex::tile`'s fix to the same class of bug on the GPU side.
    /// `docs/lighting_raymarch.md` step 2.
    pub tile: (i32, i32),
    /// What surface of the world this is a point of.
    ///
    /// The polygon and not the tile: which way it looks, and therefore which
    /// flames can light it and which parts of its own tile can shadow it. It is
    /// what the place attachment's stance carries, per pixel, after
    /// `statics.wgsl` has resolved a corner to the face of the half the fragment
    /// is on. See [`Surface`].
    pub surface: Surface,
    /// **Which occluder of its own tile this point is a point of**, or
    /// [`crate::occlusion::OwnerId::NONE`] for a point of none — the ground, a
    /// mobile, a fixture with no grid behind it.
    ///
    /// `docs/lighting_height.md` phase 3, and the whole of what replaced
    /// `on_surface`'s guess: "is this solid the one I am drawn from" used to be
    /// answered by asking whether this point's height fell inside the solid's
    /// span, which two things stacked on one tile answer identically and two
    /// things side by side on one tile answer wrongly for every pixel. A fragment
    /// knows exactly which solid it belongs to, and this is it saying so.
    ///
    /// Unique within the tile and not within the frame, which is all the
    /// comparison needs: every arm of [`exemption`] that reads it is gated on the
    /// solid being on this point's own cell. See
    /// [`crate::occlusion::Occlusion::owner_at`] for where a caller gets one.
    pub owner: crate::occlusion::OwnerId,
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
            owner: crate::occlusion::OwnerId::NONE,
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
            owner: crate::occlusion::OwnerId::NONE,
        }
    }

    /// The same point, said to be a point of one particular occluder of its own
    /// tile.
    ///
    /// A builder rather than a fourth argument on each of the three constructors:
    /// the surface and the owner are separate facts (a lid's top and a face of
    /// the same static are one owner and two surfaces), and most callers here —
    /// a test about falloff, a probe over open ground — have no occluder to name
    /// and mean [`crate::occlusion::OwnerId::NONE`] exactly.
    pub fn owned_by(self, owner: crate::occlusion::OwnerId) -> Self {
        Self { owner, ..self }
    }

    /// A point of one of a tile's four vertical faces.
    pub fn face(at: Vec2, z: f32, tile: (i32, i32), face: Face) -> Self {
        Self {
            at,
            z,
            tile,
            surface: Surface::Face(face),
            owner: crate::occlusion::OwnerId::NONE,
        }
    }
}

/// What kind of surface a lit point is a point of — the whole of what the
/// lighting asks about a pixel beyond where it is.
///
/// [`crate::place::Stance`] is the same question at the other end of the wire —
/// a corner is resolved to one of its two faces per fragment before the
/// attachment is written, and `docs/gbuffer.md` step 4c gave a mesh face
/// (a tread's top or riser) its own honest tag from this same set besides, so
/// what arrives here is always one of these four fixed normals, never a
/// computed one. `docs/lighting.md` decision 40 tried carrying a fifth,
/// computed case here (`Sloped`, a blended tread normal) before honest
/// per-face geometry existed to make it unnecessary; `docs/gbuffer.md` step 5
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
    /// Which way this surface looks, in tiles — `x` and `y` across the map and
    /// `z` in tiles as well, which is the space a flame's offset is stated in.
    ///
    /// `None` for [`Surface::Upright`], which is a statement about what is *not*
    /// known: a billboard has no side, so every flame that reaches it lights it.
    ///
    /// A **flat** surface looks up, and that is the one this had missing. A wall's
    /// top cap is a flat static, so nothing tested which way it looked and a lamp
    /// standing beside a wall lit its top as fully as one standing over it —
    /// reported from the client as two walls "adding up" at a corner, and it is a
    /// bright diamond where a corner's cap is. `docs/lighting.md`, decision 27.
    pub fn normal(self) -> Option<[f32; 3]> {
        match self {
            Self::Upright => None,
            Self::Flat => Some([0.0, 0.0, 1.0]),
            Self::Face(face) => {
                let [x, y] = face.outward();
                Some([x, y, 0.0])
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

// **`Surface::shadowed_by_own_tile` lived here**, and `docs/lighting_height.md`
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

/// How wide the band is, in tiles, over which a flame passing behind a face
/// stops lighting it. `blit.wgsl`'s `FACE_EDGE`, and the two are one number.
///
/// Not a step, for the reason a beam's rim is not one: a hard edge is what the
/// eye finds first, and a lamp walking past the end of a wall would switch its
/// face off between two frames.
/// `pub` because an oracle has to know how wide the band is to say what it costs.
/// A picture oracle rules on strict geometry — a flame behind a one-sided surface
/// lights it not at all — and the engine's answer inside this band is deliberately
/// not that. Those pixels are neither an agreement nor a defect, so an instrument
/// that cannot name the band either folds a known softening into its residual or,
/// worse, refuses to judge the whole face and reports nothing at all. Sharing the
/// number is not sharing the formula: nothing outside this module computes
/// [`faces`].
pub const FACE_EDGE: f32 = 0.2;

/// How much of a flame `toward` reaches a surface facing `normal` — `1.0` in
/// front of it, `0.0` behind it, and a gradient [`FACE_EDGE`] wide across the
/// plane. Both in tiles, `z` included: a horizontal surface is a surface, and
/// what decides for it is how far *above* its plane the flame is.
///
/// A half-space test and deliberately not a cosine. UO's art is pre-shaded —
/// every wall's picture already has a light in it — so a Lambert term would be a
/// second light fighting the first. What this answers is only whether the flame
/// is on the side the surface looks at.
///
/// `blit.wgsl`'s `faces`, and the two are one formula.
fn faces(normal: [f32; 3], toward: [f32; 3]) -> f32 {
    let along = normal[0] * toward[0] + normal[1] * toward[1] + normal[2] * toward[2];
    (along / FACE_EDGE + 0.5).clamp(0.0, 1.0)
}

/// What took a ray to nothing: not only *where* it was stopped, but *by what*.
///
/// A blamed tile answers "which wall" for exactly as long as a tile holds one
/// thing. A stair's own tile holds six — three tread tops and three risers — and
/// every question worth asking about it ("is this fragment shadowed by its own
/// flight, and by which part of it") reads the same cell whatever the answer
/// turns out to be. A diagnostic that cannot separate those answers cannot be
/// used to choose between the fixes they call for, and choosing between them by
/// reading the code instead is how `docs/lighting_height.md` twice let a
/// plausible attribution stand as a measured cause.
///
/// So the cell stays and the occluder is named beside it. [`Stopper::owner`] is
/// the very fact [`exemption`] compares, so a report carrying it can be read
/// against the fragment's own [`Spot::owner`] with nothing re-derived in
/// between: equal owners on a solid that still stopped the ray says the
/// exemption did not fire, and different owners says it was never entitled to.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Stopper {
    /// The tile it stands on — what [`Reach::stopped_by`] was, alone, before
    /// there was anything beside it.
    pub cell: (i32, i32),
    /// Which occluder of [`Stopper::cell`] this is, off the reference the walk
    /// followed to reach it — the number [`exemption`] compares, not a position
    /// in any list. See [`crate::occlusion::OwnerId`].
    pub owner: crate::occlusion::OwnerId,
    /// Its sides: `0` for a lid, [`crate::occlusion::EDGE_ANY`] for a body,
    /// anything else for a panel.
    ///
    /// The shape rather than the identity, and it is here because "a lid of my
    /// own static" and "a panel of my own static" are two different defects
    /// wearing the same owner — the first is [`exemption`]'s deliberate carve-out
    /// for lids, the second would be identity failing to reach at all.
    pub edges: u8,
    /// The `z` span **the walk that blamed it actually read**: the record's
    /// exact corners from [`walk_cells_exact`], the wire's quantised one from
    /// [`walk_cells_streaming`].
    ///
    /// Deliberately not normalised to one of the two. Which span a walk is
    /// entitled to is the discipline `docs/lighting_height.md` phase 2 states,
    /// and a report that quietly picked the exact one would hide the walk that
    /// read the other.
    pub span: (f32, f32),
}

impl std::fmt::Display for Stopper {
    /// `(100, 100) owner 1, lid z 3.00..3.00` — the cell, the number
    /// [`exemption`] compares, and the shape, in the order a person asks for
    /// them. One formatting, because the flame's report and the sun's both want
    /// exactly this and a second copy would drift.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shape = match self.edges {
            0 => "lid",
            EDGE_ANY => "body",
            _ => "panel",
        };
        write!(
            f,
            "({}, {}) owner {}, {shape} z {:.2}..{:.2}",
            self.cell.0,
            self.cell.1,
            self.owner.raw(),
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
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Reach {
    /// Which of [`Lighting::lights`] this is, by index.
    pub light: usize,
    /// How far the flame is, in tiles, with `z` divided into tiles — the same
    /// three-dimensional distance the falloff uses.
    pub distance: f32,
    /// Whether that distance is inside the flame's radius. `false` means the
    /// spot is outside the pool and nothing else was computed.
    pub within: bool,
    /// How much of the flame survived the walk: `1.0` for an open path, `0.0` for
    /// a wall, and between for a partial occluder. Only meaningful when
    /// [`Reach::within`].
    pub through: f32,
    /// How much of the flame's [`Beam`] falls here, **and how much of it the
    /// surface is turned towards**: `1.0` for a fire that lights every direction
    /// falling on a floor, [`BEAM_SPILL`] for a spot behind a carried lantern, and
    /// `0.0` for a wall's face with the flame behind it.
    ///
    /// The two are one number because they are the same question asked of the two
    /// ends — which way the light points, and which way the surface looks — and a
    /// dark tile that is neither is what the shadow term is for.
    ///
    /// A separate number from [`Reach::through`] and not folded into it, because
    /// the two answer the questions a person asks in the order they ask them:
    /// "is the light pointing at me" comes before "is something in the way", and
    /// a report that gave one number could not tell a spot behind the player
    /// from a spot behind a wall.
    pub cone: f32,
    /// What stopped the ray, where anything did.
    ///
    /// The *first* cell that took the survival to zero and the solid on it that
    /// took most of it — which is the pair worth naming: a ray crossing two walls
    /// is stopped by the first of them and the second is a fact about the map,
    /// not about this pixel. See [`Stopper`].
    pub stopped_by: Option<Stopper>,
    /// What this flame added to the multiplier, linear, per channel.
    pub added: [f32; 3],
}

/// Everything one point of the world receives, and from what.
///
/// [`sample`] is the CPU's copy of `blit.wgsl`'s fragment loop, and the copy
/// exists for two reasons: a test can assert on numbers instead of on pixels,
/// and the client can answer "why is this tile lit" in words. Both are worthless
/// if the copy drifts, so a GPU test runs the real blit over a synthetic place
/// attachment and asserts the two agree — see `docs/lighting.md`, decision 9.
#[derive(Clone, PartialEq, Debug)]
pub struct Sample {
    /// Where this was asked about.
    pub spot: Spot,
    /// What the art at this spot is multiplied by: the ambient plus every
    /// flame's contribution, unclamped. The shader clamps at the end; this does
    /// not, because a value over one is a real answer — it says the spot is
    /// blown out rather than merely lit.
    pub multiplier: [f32; 3],
    /// One entry per flame the frame carried, in the order [`Lighting::lights`]
    /// holds them — including the ones that reached nothing, which is exactly
    /// what a person asking "why is it dark here" needs to see.
    pub reaches: Vec<Reach>,
    /// How much of the sun reached this spot, and what stopped it — `None` where
    /// the frame had no sun at all, which is a different answer from `0.0`.
    pub sun: Option<Reach>,
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
/// **The sentence a reader gets wrong otherwise**, and the report is where they
/// get it wrong rather than the engine: an [`crate::occlusion::OwnerId`] is a
/// number *within a cell*, so a fragment of owner 1 stopped by "owner 1" on a
/// **different** cell has not been stopped by itself — those are two unrelated
/// statics that happen to be their own cells' first. [`exemption`] is not fooled,
/// because every arm of it that reads an owner is gated on `own_cell`; a person
/// reading two equal numbers side by side is, and this session did.
///
/// So the comparison lives here, where both halves are in hand, instead of in
/// [`Stopper`]'s own `Display`, which knows the solid and not the fragment.
fn stands_to(spot: Spot, stopper: Stopper) -> &'static str {
    match stopper.cell == spot.tile {
        false => "another cell, whose owner numbers mean nothing here",
        true if spot.owner.same(stopper.owner) => "THE FRAGMENT'S OWN OCCLUDER",
        true => "another occluder of the fragment's own cell",
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
            write!(f, "  light {}: {:.2} tiles", reach.light, reach.distance)?;
            // In the order the questions are asked: is it near enough, is
            // anything in between, and how much of its beam this spot is in —
            // see [`Reach::cone`], which is the number that says whether a dark
            // tile is behind a wall or behind the character.
            match (reach.within, reach.stopped_by) {
                (false, _) => writeln!(f, ", outside its radius")?,
                (true, Some(stopper)) => {
                    writeln!(f, ", stopped by {stopper} — {}", stands_to(self.spot, stopper))?
                }
                (true, None) => writeln!(
                    f,
                    ", through {:.2}, beam {:.2}, adds {:.3}",
                    reach.through,
                    reach.cone,
                    reach.added.iter().sum::<f32>() / 3.0,
                )?,
            }
        }
        if let Some(sun) = self.sun {
            match sun.stopped_by {
                Some(stopper) => writeln!(
                    f,
                    "  sun: in shadow of {stopper} — {}",
                    stands_to(self.spot, stopper)
                )?,
                None => writeln!(
                    f,
                    "  sun: through {:.2}, adds {:.3}",
                    sun.through,
                    sun.added.iter().sum::<f32>() / 3.0,
                )?,
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

/// [`sample`], through [`walk_cells_exact`] instead of [`walk_cells`].
///
/// A temporary public seam for `docs/lighting_raymarch.md`'s point 3, not a
/// second code path anything real should call: the doc's own oracles
/// (`tests/lighting.rs`'s grid-sweep and fuzz, `tests/frame.rs`'s
/// real-geometry fixtures) run through `sample`, not `walk_cells` directly,
/// so exercising `walk_cells_exact` against them needs its own entry point
/// into the same machinery. It goes away at point 4's cutover, when `sample`
/// itself walks this path and there is only one `sample` to have a seam to.
#[doc(hidden)]
pub fn sample_exact(spot: Spot, lighting: &Lighting) -> Sample {
    sample_with(spot, lighting, walk_exact, walk_sun_exact)
}

fn sample_with(
    spot: Spot,
    lighting: &Lighting,
    walk: impl Fn(Spot, &Light, &Occlusion) -> (f32, Option<Stopper>),
    walk_sun: impl Fn(Spot, Sun, &Occlusion) -> (f32, Option<Stopper>),
) -> Sample {
    // The ambient this *tile* has, and not the frame's: how much of the sky the
    // column over it can see decides how much of the sky term it gets. The tile
    // and not the fractional spot, because the field is a byte a tile — the blur
    // of `docs/lighting_world.md`'s decision 2 is what softens its edges, and a
    // second interpolation here would be a different picture from the shader's.
    let mut multiplier = lighting
        .ambient
        .at(lighting.occlusion.sky_at(spot.tile.0, spot.tile.1));
    let mut reaches = Vec::with_capacity(lighting.lights.len());
    for (index, light) in lighting.lights.iter().enumerate() {
        let offset = [
            light.at.x - spot.at.x,
            light.at.y - spot.at.y,
            (light.z - spot.z) / Z_PER_TILE,
        ];
        let distance = offset.iter().map(|axis| axis * axis).sum::<f32>().sqrt();
        let d = distance / light.radius.max(0.001);
        if d >= 1.0 {
            reaches.push(Reach {
                light: index,
                distance,
                within: false,
                through: 0.0,
                cone: 0.0,
                stopped_by: None,
                added: [0.0; 3],
            });
            continue;
        }
        // Which way the light is pointing, before anything is asked about what
        // stands in the way: a beam that misses this spot has nothing to be
        // stopped by. The offset is from the spot to the flame, and a beam's
        // axis points the other way, so the sign flips here.
        let cone = match light.beam {
            Some(beam) => beam.lights(offset.map(|axis| -axis)),
            None => 1.0,
        };
        // And whether the flame is on the side this surface looks at — geometry
        // and nothing else. `blit.wgsl` argues why there is no longer an
        // exemption for a flame standing in the wall's own line, and
        // [`mounted_at`] is what replaced it.
        let facing = match spot.surface.normal() {
            None => 1.0,
            Some(normal) => faces(normal, offset),
        };
        let (through, stopped_by) = walk(spot, light, &lighting.occlusion);
        let fall = 1.0 - d;
        let added = light
            .color
            .map(|channel| channel * light.intensity * fall * fall * through * cone * facing);
        for (total, channel) in multiplier.iter_mut().zip(added) {
            *total += channel;
        }
        reaches.push(Reach {
            light: index,
            distance,
            within: true,
            through,
            cone: cone * facing,
            stopped_by,
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
            light: lighting.lights.len(),
            distance: f32::INFINITY,
            within: true,
            through,
            // The sun is a direction and not a beam: it lights everything it can
            // see, and there is nothing for a cone to exclude.
            cone: 1.0,
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
/// [`walk_cells`], the same walk a flame's ray takes.
///
/// The spot's own tile is skipped, as it is for a flame, and for the same reason
/// in reverse: a wall's own pixels are on a tile that stops light, and a wall
/// that shadowed itself would be black on the side the sun is on. The far end is
/// *not* skipped — there is no tile there, only a point in the sky.
fn walk_sun(spot: Spot, sun: Sun, occlusion: &Occlusion) -> (f32, Option<Stopper>) {
    let horizontal = (sun.toward[0] * sun.toward[0] + sun.toward[1] * sun.toward[1]).sqrt();
    if horizontal < 1e-6 {
        // Straight overhead: there is no direction to walk along the ground, and
        // the only thing that could shadow the spot is on its own tile — which is
        // exempt. Nothing stops it.
        return (1.0, None);
    }
    // One tile of ground a unit, so `z` climbs by the sun's own slope.
    let step = [
        sun.toward[0] / horizontal,
        sun.toward[1] / horizontal,
        sun.toward[2] / horizontal * Z_PER_TILE,
    ];
    let mut tiles = MAX_SUN_TILES;
    if let (Some(ceiling), true) = (occlusion.tallest(), step[2] > 1e-6) {
        tiles = tiles.min((ceiling as f32 - spot.z) / step[2]);
    }
    if occlusion.tallest().is_none() || tiles <= 0.0 {
        // Nothing in the grid stops anything, or the spot is already above
        // everything that could — either way the ray is in the sky from here.
        return (1.0, None);
    }
    let from = [spot.at.x, spot.at.y, spot.z];
    let to = [
        from[0] + step[0] * tiles,
        from[1] + step[1] * tiles,
        from[2] + step[2] * tiles,
    ];
    // No tile to exempt at the far end, and a point source: the sun subtends half
    // a degree, so its penumbra is the narrowest the walk draws.
    walk_cells_streaming(from, to, LitEnd::of(spot), false, 0.0, occlusion)
}

/// The ray from a spot to a flame: [`walk_cells_streaming`] with a flame's two
/// ends.
///
/// The flame's own tile must not shadow it — a sconce stands *on* a wall — and a
/// flame is a body about a tile across, which is what its penumbra is made of.
/// Those two facts are the whole difference between this ray and the sun's.
fn walk(spot: Spot, light: &Light, occlusion: &Occlusion) -> (f32, Option<Stopper>) {
    walk_cells_streaming(
        [spot.at.x, spot.at.y, spot.z],
        [light.at.x, light.at.y, light.z],
        LitEnd::of(spot),
        true,
        FLAME_SPREAD,
        occlusion,
    )
}

/// One segment of the world, cell by cell: how much of a ray survives it, and
/// what stopped it.
///
/// `blit.wgsl`'s `walk`, including what it leaves out, and **one walk for both
/// the flame and the sun** — see the shader for the argument, and for the
/// measurement that produced it. The ends are the parameters: `skip_last` is the
/// flame's own tile, and `spread` is how big the source is, in tiles. A sunbeam
/// passes `false` and `0.0`.
///
/// Every cell the segment crosses, in order, with the length of each crossing:
/// not a fixed number of samples, which at two tiles apart was one interior
/// point and put every shadow's edge on a tile boundary. What a cell stops is
/// its opacity scaled by how far the ray ran inside it — [`FLAME_SPREAD`] and its
/// two bounds — and by how much of that run was inside the span the tile
/// occupies, so a ray grazing the top of a wall or clipping its corner is dimmed
/// rather than cut.
///
/// The starting cell is always skipped: the tile being lit must not shadow
/// itself, which is what keeps a wall's own face the brightest thing beside a
/// torch.
/// One cell [`dda_walk`]'s DDA visits along the ray, and how the ray leaves
/// it — no [`Occlusion`], no opacity, nothing about what is *in* the cell.
///
/// Split out so the stepping itself — which cell follows which — can be
/// checked against plain numbers instead of a lit scene. This is the exact
/// machinery `docs/lighting_raymarch.md`'s bugs lived in: a tile re-derived
/// from a float that could legitimately sit on its own boundary, and (until
/// [`walk_cells_streaming`]'s cutover removed the need for a corner jump at
/// all) a corner tie that fired on a ray it was never about. See
/// [`dda_walk`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct DdaCell {
    /// The cell this step covers.
    cell: (i32, i32),
    /// Which side the ray leaves this cell by, matching `continues`. `0`
    /// when the ray ends inside this cell instead of crossing on.
    exit: u8,
    /// How far along the whole segment (`0.0..=1.0`) the ray enters and
    /// leaves this cell.
    entered: f32,
    leaves: f32,
    /// Whether the walk continues past this cell — `false` exactly when
    /// `exit == 0`, the ray ending here.
    continues: bool,
}

/// Which cells a straight ray from `from` to `to` visits, in order — plain
/// single-axis DDA, one cell per step, no diagonal jump.
///
/// `tile` is `from`'s own tile, [`Spot::tile`]'s own contract and not
/// `from.floor()`: seeded from the caller's answer rather than re-derived
/// from a float that can legitimately sit on `tile`'s own far edge. Bounded
/// by [`MAX_WALK_STEPS`] cells; a ray that has not reached `to` by then just
/// stops.
///
/// **A walk that never skips a cell is complete by construction** — the
/// textbook reason grid-line rasterisation steps one axis at a time —
/// which is why there is no corner-tie heuristic here at all;
/// [`walk_cells_streaming`]'s own doc comment has the fault-injection
/// discipline that checked this rather than assumed it, for the same shape
/// of stepping restated here as [`DdaCell`]s instead of folded into that
/// function's own loop.
///
/// **Precondition**: `from` and `to` are not the same point in the plane —
/// callers already guard `ground < 1e-6` before this is ever called, and
/// there is no direction to step in for a ray with no length in it.
fn dda_walk(from: Vec2, to: Vec2, tile: (i32, i32)) -> Vec<DdaCell> {
    let delta = [to.x - from.x, to.y - from.y];
    // Which way each axis steps, how much of the whole segment one tile of it
    // is worth, and how far along the segment the first boundary is. An axis
    // the ray does not move along never reaches its boundary, which is what
    // the enormous `t` says.
    let toward = (
        match delta[0] >= 0.0 {
            true => 1,
            false => -1,
        },
        match delta[1] >= 0.0 {
            true => 1,
            false => -1,
        },
    );
    let mut per_tile = [1e30_f32; 2];
    let mut boundary = [1e30_f32; 2];
    for axis in 0..2 {
        if delta[axis].abs() <= 1e-6 {
            continue;
        }
        per_tile[axis] = 1.0 / delta[axis].abs();
        let from = [from.x, from.y][axis];
        // The known tile's own edge, not `from.floor()`: a `from` sitting
        // exactly on this axis' boundary must seed `boundary[axis]` near
        // zero (the ray is already leaving `tile`), and `from.floor()`
        // there can just as well pick the far side and seed a whole tile of
        // slack that was never there.
        let edge = [tile.0, tile.1][axis] as f32;
        let ahead = match delta[axis] >= 0.0 {
            true => edge + 1.0 - from,
            false => from - edge,
        };
        boundary[axis] = ahead * per_tile[axis];
    }

    let mut cells = Vec::new();
    let mut cell = tile;
    let mut entered = 0.0_f32;
    for _ in 0..MAX_WALK_STEPS {
        let next = boundary[0].min(boundary[1]);
        let leaves = next.min(1.0);
        let out_by_x = boundary[0] < boundary[1];
        let exit = match next < 1.0 {
            false => 0,
            true => match (out_by_x, out_by_x && toward.0 > 0 || !out_by_x && toward.1 > 0) {
                (true, true) => crate::occlusion::EDGE_EAST,
                (true, false) => crate::occlusion::EDGE_WEST,
                (false, true) => crate::occlusion::EDGE_SOUTH,
                (false, false) => crate::occlusion::EDGE_NORTH,
            },
        };
        if next >= 1.0 {
            cells.push(DdaCell {
                cell,
                exit: 0,
                entered,
                leaves,
                continues: false,
            });
            break;
        }
        cells.push(DdaCell {
            cell,
            exit,
            entered,
            leaves,
            continues: true,
        });
        // Into the neighbour across whichever boundary is nearer — the
        // nearer boundary only, no diagonal probe.
        match out_by_x {
            true => {
                cell.0 += toward.0;
                boundary[0] += per_tile[0];
            }
            false => {
                cell.1 += toward.1;
                boundary[1] += per_tile[1];
            }
        }
        entered = next;
    }
    cells
}

/// Every tile [`walk_cells_exact`] tests solids from: [`dda_walk`]'s own
/// straight-line cells, plus — unconditionally — the diagonal neighbour
/// crossed at every step.
///
/// `docs/lighting_raymarch.md`'s ray-vs-Solid scoping, point 1's answer,
/// session 8: [`ray_vs_solid`] answers "is a candidate real" exactly, so
/// nothing here needs to guess which corner is worth asking about before
/// asking it — probe both neighbours every time and let the primitive say
/// no. Roughly twice the cells [`dda_walk`] itself visits, not a bounding
/// box: still `O(walk length)`, the order the doc's own cost estimate
/// argued for.
///
/// Deduplicated, in the order the straight walk meets them — a diagonal
/// candidate named at one step can be the very next cell the straight walk
/// reaches on its own, and [`walk_cells_exact`] would double-count a
/// solid on it otherwise.
fn candidate_tiles(from: Vec2, to: Vec2, tile: (i32, i32)) -> Vec<(i32, i32)> {
    let delta = [to.x - from.x, to.y - from.y];
    let toward = (
        if delta[0] >= 0.0 { 1 } else { -1 },
        if delta[1] >= 0.0 { 1 } else { -1 },
    );
    fn push(tiles: &mut Vec<(i32, i32)>, t: (i32, i32)) {
        if !tiles.contains(&t) {
            tiles.push(t);
        }
    }
    let mut tiles = Vec::new();
    for step in dda_walk(from, to, tile) {
        push(&mut tiles, step.cell);
        if step.continues {
            // Both single-axis neighbours of this cell, named at every
            // transition regardless of which one `dda_walk` itself steps
            // into next: an ordinary step already visits one of the two as
            // its very next cell; the other is the diagonal neighbour this
            // walk's own single-axis stepping never reaches on its own.
            // Neither is the cell reached by stepping *both* axes — that
            // one is already `dda_walk`'s own next-or-next-next cell, not a
            // corner candidate at all.
            push(&mut tiles, (step.cell.0 + toward.0, step.cell.1));
            push(&mut tiles, (step.cell.0, step.cell.1 + toward.1));
        }
    }
    tiles
}

/// [`walk_cells`], built on [`ray_vs_solid`] instead of [`dda_walk`]'s own
/// per-cell bookkeeping — `docs/lighting_raymarch.md`'s ray-vs-Solid
/// scoping, point 2. Same signature, same exemption/run/aperture/softness
/// rules, copied rather than re-derived: what changes is where a solid's
/// own crossing interval comes from — an exact box intersection instead
/// of a tile-boundary crossing fraction shared by everything on the cell.
/// Not wired into [`walk`]/[`walk_sun`] yet — see the doc for why the
/// cutover is its own, later step, gated on point 3's agreement pass.
///
/// **`corner_tie`, [`DdaTransition::Corner`] and [`panel_stop`] have no
/// counterpart here, on purpose.** [`candidate_tiles`] probes the diagonal
/// neighbour at every step unconditionally, and [`ray_vs_solid`] answers
/// "does the ray actually touch this solid's box" exactly — nothing here
/// needs the heuristic that used to stand in for that answer, and a
/// solid's own box is tested directly rather than by asking which side of
/// the *tile* a DDA step happened to cross.
///
/// **What is still grouped by tile, not by solid**: `through` is updated
/// once per candidate tile, by the *largest* of what its solids stop — the
/// same `stopped.max(by_surface)` discipline [`walk_cells`] uses, and for
/// the same reason (two panels of one corner are two faces of one wall,
/// crossed once). Each solid still gets its own exact `entered`/`leaves`
/// from [`ray_vs_solid`]; only the accumulation into `through` stays
/// per-tile.
///
/// **One thing tried and reverted, kept here rather than silently
/// dropped**: dropping [`walk_cells`]'s "does either tile-boundary side
/// pierce this body" safety net on an [`EDGE_ANY`] solid, on the theory
/// that an exact box crossing no longer needs a safety net a DDA
/// approximation did. Wrong — [`box_side`]'s scratch fuzz (see the doc's
/// point 3) found `walk_cells_exact` reading a body's corner as almost
/// fully open where `walk_cells` read it as blocked, in the exact shape
/// `walk_cells`'s own comment names: "the pierce is what closes the sliver
/// a ray clipping a corner used to walk through." The safety net was never
/// about DDA imprecision — it is a deliberate choice that a corner reads
/// as opaque, not as proportionally see-through for having been grazed at
/// a narrow angle — so it stays, with [`box_side`] reading which side of
/// the tile a crossing point sits on geometrically instead of carrying it
/// from a DDA step that, for a diagonal-only candidate, never happened.
///
/// **The panel branch does still simplify one thing**: it samples
/// [`pierced`] once, at the crossing's own midpoint, rather than at the
/// two tile-boundary points `walk_cells` used. Those two points could be a
/// whole cell apart; [`ray_vs_solid`]'s own `entered`/`leaves` already
/// bound the ray to the panel's real
/// [`crate::occlusion::PANEL_THICKNESS`]-deep box, so the two ends are
/// close together by construction and one interior sample is enough — the
/// same fuzz that caught the body regression above stayed clean on panels
/// alone.
fn walk_cells_exact(
    from: [f32; 3],
    to: [f32; 3],
    lit: LitEnd,
    skip_last: bool,
    spread: f32,
    occlusion: &Occlusion,
) -> (f32, Option<Stopper>) {
    // Where the fragment is, kept before [`stand_clear`] moves the ray off it —
    // see [`ExemptionContext::drawn_z`].
    let drawn_z = from[2];
    let (from, to) = stand_clear(from, to, lit.surface);
    let first = lit.tile;
    let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let ground = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
    if ground < 1e-6 {
        // Same shortcut `walk_cells` takes, unchanged: no direction to walk
        // in, so only a lid on the one cell can stand between the ends.
        // `cell` rather than `solids_at` for the owner beside each lid — the two
        // follow the same references in the same order, so this enumerates
        // exactly what it always did.
        let mut stopped: f32 = 0.0;
        let mut worst: Option<Stopper> = None;
        for (stands, owner) in occlusion.cell(first.0, first.1) {
            if stands.edges != 0 {
                continue;
            }
            // And only the lids this ray is actually **under**. A tread's top is
            // a lid narrower than its tile, and the main path has asked this
            // since sub-tile footprints landed — this shortcut did not follow,
            // which made a flight's three treads shadow one another from
            // straight above and below. `docs/lighting_height.md`'s backlog, and
            // `a_vertical_ray_is_not_stopped_by_lids_it_is_not_over`.
            if !over_footprint(from, &stands.space) {
                continue;
            }
            let (low, high) = (stands.low(), stands.high());
            // Phase 4's rule, here too. Every solid this loop sees is on the
            // fragment's own cell by construction, so the owner is the whole of
            // what [`exemption`] would add — and a ray going straight up or down
            // off a tread is exactly the ray whose only contact with that tread's
            // own top is the point it started at.
            if lit.owner.same(owner) && drawn_on(drawn_z, low, high) {
                continue;
            }
            let by_surface =
                f32::from(stands.opacity) / 255.0 * crosses(from[2], to[2], low, high, to[2], spread);
            if by_surface > stopped {
                stopped = by_surface;
                worst = Some(Stopper {
                    cell: first,
                    owner,
                    edges: stands.edges,
                    span: (low, high),
                });
            }
        }
        return match stopped >= 1.0 - RAY_CUTOFF {
            // A `stopped` that reaches the cutoff came from some lid, so there is
            // one to name — the invariant this `expect` states rather than hides.
            true => (0.0, Some(worst.expect("a lid took the ray to nothing"))),
            false => (1.0 - stopped, None),
        };
    }
    let last = (to[0].floor() as i32, to[1].floor() as i32);
    let own = match lit.surface.face() {
        Some(face) => crate::occlusion::edges_of(Some(crate::facing::Facing::One(face))),
        None => 0,
    };
    let exemption_ctx = ExemptionContext {
        first,
        last,
        skip_last,
        own,
        owner: lit.owner,
        spot_z: from[2],
        drawn_z,
        to_z: to[2],
    };

    struct Hit<'a> {
        stands: &'a crate::occlusion::Solid,
        /// Which occluder of `cell` this solid is — off the reference the walk
        /// followed to it, not off the solid, since that is where the number
        /// lives. See [`crate::occlusion::OwnerId`].
        owner: crate::occlusion::OwnerId,
        entered: f32,
        leaves: f32,
    }
    let mut by_tile: Vec<((i32, i32), Vec<Hit<'_>>)> = Vec::new();
    let from2 = Vec2::new(from[0], from[1]);
    let to2 = Vec2::new(to[0], to[1]);
    for cell in candidate_tiles(from2, to2, first) {
        let mut here = Vec::new();
        for (stands, owner) in occlusion.cell(cell.0, cell.1) {
            if let Some((entered, leaves)) = ray_vs_solid(from, to, &stands.space) {
                here.push(Hit {
                    stands,
                    owner,
                    entered,
                    leaves,
                });
            }
        }
        if !here.is_empty() {
            by_tile.push((cell, here));
        }
    }
    // The order the ray actually meets them, the same discipline
    // `walk_cells` keeps by walking cell after cell along the DDA — needed
    // for the early cutoff below, and for a blamed tile to mean what
    // `walk_cells`'s does.
    by_tile.sort_by(|(_, a), (_, b)| {
        let ea = a.iter().map(|hit| hit.entered).fold(f32::MAX, f32::min);
        let eb = b.iter().map(|hit| hit.entered).fold(f32::MAX, f32::min);
        ea.total_cmp(&eb)
    });

    let mut through = 1.0;
    for (cell, hits) in by_tile {
        let mut stopped: f32 = 0.0;
        // The solid of this cell that took the most of the ray, kept beside the
        // number it produced. `>` and not `>=`, so a tie names the one the walk
        // met first rather than the last one to equal it.
        let mut worst: Option<Stopper> = None;
        for Hit {
            stands,
            owner,
            entered,
            leaves,
        } in hits
        {
            // The record's own span, exactly — this walk reads `space` for the
            // box it tests, and reading a *rounded* height for the same solid's
            // exemptions and its `crosses` would be two different solids in one
            // iteration. `walk_cells_streaming`'s own copy is the quantised one
            // on purpose; see its doc comment.
            let (low, high) = (stands.low(), stands.high());
            // Same [`exemption`] `walk_cells_streaming` calls — see its own doc.
            let Exemption { exempt, same_run } = exemption(&exemption_ctx, cell, stands, owner, low, high);
            if exempt {
                continue;
            }
            let middle = (entered + leaves) * 0.5;
            let soft =
                (spread * middle / (1.0 - middle).max(1e-3)).clamp(SOFT_CROSSING_MIN, SOFT_CROSSING_MAX);
            let opacity = f32::from(stands.opacity) / 255.0;
            let tall = soft * FLAME_DEPTH;
            let by_surface = match stands.edges {
                0 => {
                    // **Not the lid's own `entered`/`leaves`.** A lid is
                    // flat in `z` (`Solid::box_of`'s `min.z == max.z` for
                    // an ordinary floor), so `ray_vs_solid`'s `z`-slab
                    // narrows both ends to the exact same instant the ray
                    // crosses that one height — correct as a crossing
                    // *point*, but [`crosses`] needs the ray's `z` on each
                    // side of that point to tell "crossed through" from
                    // "never came close," and a from/to that are already
                    // equal answers every comparison in [`crosses`] as
                    // "never." What it needs is the ray's `z` where it
                    // enters and leaves the lid's own real horizontal
                    // footprint, over an unconstrained `z` — `stands.space`
                    // itself, not the whole tile: a tread's own top is a lid
                    // narrower than its tile, and asking a wider box than
                    // the lid actually is would let a ray graze the tile's
                    // corner past the tread's real edge read as "crossed
                    // through" the tread. Was the whole tile until
                    // `docs/lighting_raymarch.md`'s "second bigger idea"
                    // landed — nothing before that could tell a sub-tile
                    // lid's own bounds from its tile's.
                    let footprint = crate::solid::Solid {
                        min: crate::camera::WorldSpot {
                            x: stands.space.min.x,
                            y: stands.space.min.y,
                            z: -1e6,
                        },
                        max: crate::camera::WorldSpot {
                            x: stands.space.max.x,
                            y: stands.space.max.y,
                            z: 1e6,
                        },
                    };
                    let (tile_entered, tile_leaves) =
                        ray_vs_solid(from, to, &footprint).unwrap_or((entered, leaves));
                    let from_z = from[2] + delta[2] * tile_entered;
                    let to_z = from[2] + delta[2] * tile_leaves;
                    opacity * crosses(from_z, to_z, low, high, to[2], spread)
                }
                // A body is a real 3D box and `ray_vs_solid` is an exact
                // slab test — a `Some` here already means the segment
                // genuinely passed through it over `entered..leaves`, so
                // occlusion is the body's own opacity outright. No
                // length-based fade, no per-side `pierces` floor, no
                // widened-corner graze: those existed only to fake a
                // penumbra a point flame does not cast. See `docs/
                // lighting_raymarch.md`'s "hard shadows" decision.
                EDGE_ANY => opacity,
                edges => {
                    if edges & !same_run == 0 {
                        0.0
                    } else {
                        let cross = [
                            from[0] + delta[0] * middle,
                            from[1] + delta[1] * middle,
                            from[2] + delta[2] * middle,
                        ];
                        opacity * pierced(stands, low, high, cross, soft, tall)
                    }
                }
            };
            if by_surface > stopped {
                stopped = by_surface;
                worst = Some(Stopper {
                    cell,
                    owner,
                    edges: stands.edges,
                    span: (low, high),
                });
            }
        }
        through *= 1.0 - stopped;
        if through <= RAY_CUTOFF {
            // `through` was over the cutoff before this cell and is under it
            // after, so `stopped` is above zero and some solid produced it.
            return (
                0.0,
                Some(worst.expect("a cell that trips the cutoff has a solid that did it")),
            );
        }
    }
    (through, None)
}

/// A GPU-shaped reformulation of [`walk_cells_exact`], proven equivalent to
/// it (on ordinary, non-[`crate::occlusion::Builder::add_raw`] geometry) —
/// `docs/lighting_raymarch.md`'s point 4, and [`walk`]/[`walk_sun`]'s own
/// walk since the cutover, mirrored in `blit.wgsl`'s `walk`.
///
/// Returns the blamed tile the same shape [`walk_cells`] used to: the cell
/// being applied at the moment `through` first drops to or under
/// [`RAY_CUTOFF`], `None` when nothing ever did. Free to add — the
/// enumeration below already visits cells strictly in ray order, so the cell
/// that trips the cutoff *is* the first one in ray order that fully blocked
/// it, the same fact [`walk_cells`]'s own `Some(cell)` returns named. Kept
/// rather than dropped because [`Reach::stopped_by`] has real readers
/// (`tests/lighting.rs`'s assertions, `Debug` for [`Sample`],
/// `examples/isolated_scene.rs`) that the cutover must not silently break.
///
/// **Why a second exact walk, rather than porting [`walk_cells_exact`]
/// itself**: `blit.wgsl`'s `walk` returns one `f32`, `through` — nothing
/// downstream reads which tile stopped it, unlike [`Reach::stopped_by`].
/// [`candidate_tiles`]'s `Vec` (dynamic allocation, `O(n²)` dedup via
/// `Vec::contains`) and [`walk_cells_exact`]'s subsequent sort by nearest
/// crossing both exist only to name the *first* blocking tile in ray order —
/// a question nothing downstream of `blit.wgsl`'s `walk` asks. `through` is
/// a product of independent `(1 - stopped)` factors, one per candidate tile,
/// and a product is order-independent (up to float noise, comfortably
/// inside decision 9's ±1/255 tolerance) — so a bounded, bare per-fragment
/// loop can multiply every candidate's contribution in as it is found,
/// without ever collecting or sorting them, provided the enumeration itself
/// never revisits a cell (which would double-count it) or misses one (which
/// would under-occlude).
///
/// **The enumeration is plain single-axis DDA, one cell per step, and
/// nothing else — no diagonal probe, and that was checked rather than
/// assumed.** The backlog's own point 1/2 scoping (session 8) reads as
/// asking for an *unconditional off-axis probe* alongside `dda_walk`'s own
/// corner-jumping — because `dda_walk` still skips straight from one cell to
/// a diagonal neighbour whenever [`corner_tie`] fires, and that skip is
/// exactly what the probe exists to cover for. This function does not keep
/// that skip at all: every transition steps exactly one axis, the nearer
/// boundary, full stop — no [`corner_tie`], no [`panel_stop`], no
/// [`DdaTransition::Corner`]. **A DDA walk that never skips a cell is
/// complete by construction** — the textbook reason grid-line rasterisation
/// steps one axis at a time is that doing so visits every cell a continuous
/// line's interior passes through, with nothing left over for a diagonal
/// probe to add. Checked, not trusted from the argument alone: an earlier
/// draft of this function *did* carry an unconditional off-axis probe
/// (mirroring the backlog's own framing literally), and deliberately
/// disabling it — across the six-point counter-example, an unrestricted
/// single-body fuzz, an unrestricted single-panel fuzz, a fuzz aimed
/// exactly at a two-panel building corner, one fixed ray running the exact
/// diagonal through a shared corner point, and 30,000 cases over a
/// seven-solid room — never once produced a disagreement with
/// [`walk_cells_exact`]. The probe was dead code for this architecture, not
/// a simplification worth keeping "just in case"; see the doc's Session 15
/// entry for the full account of ruling it out rather than assuming it in.
///
/// **The probe came back at the WGSL cutover, for a reason session 15's own
/// fuzzing could never have reached: a second backend to disagree with.**
/// Every fuzz above ran one CPU implementation against itself, so a tie in
/// `boundary[0]`/`boundary[1]` always resolved the same way twice — nothing
/// there could ever expose that a GPU's own division is not guaranteed to
/// resolve the identical tie identically. `docs/lighting_raymarch.md`'s
/// point 4 cutover found exactly that at `a_single_flat_face_beside_an_
/// occluder_agrees_with_light_sample`, and the fix that survived is this
/// function probing the untaken side of every transition — not the walk's
/// own trajectory, which still steps one axis at a time, the nearer
/// boundary only, same as always. A *gated* probe (only near a computed
/// tie) was tried first and made things worse, not better: CPU and GPU do
/// not compute a close-enough `boundary[0] - boundary[1]` to agree on which
/// rays even count as near a tie, so widening the gate only widened the set
/// of rays where one backend probed and the other did not. Unconditional
/// removes the asymmetry instead of sizing it: `candidate_tiles` already
/// names both single-axis neighbours at every transition regardless of any
/// tie, so nothing here costs `walk_cells_exact` agreement that wasn't
/// already priced in.
///
/// **What "exactly" is limited to, and why that limit is deliberate and not
/// a shortcut.** Every solid tested here is reconstructed from `(tile,
/// bottom, top, fraction)` via [`crate::occlusion::Solid::box_from_footprint`]
/// rather than read off [`crate::occlusion::Solid::space`] the way
/// [`walk_cells_exact`] does — because `blit.wgsl` reads exactly that same
/// quantised-to-a-byte fraction, not `space`'s own `f64`s, and this function
/// exists to be a faithful preview of what the shader does, not a better
/// version of it. `docs/lighting_raymarch.md`'s "second bigger idea" (session
/// 14) is what closed the gap this comment used to describe: `Builder::
/// add_raw`'s sub-tile boxes and a climbable static's own treads/risers used
/// to be the one shape neither this function nor `blit.wgsl` could tell apart
/// from a whole tile, because the four-byte upload had no `x`/`y` channel at
/// all. `Occlusion::footprint_bytes` is that channel, landed once a reader
/// existed on both sides — see `docs/lighting_raymarch.md`'s own Handoff log
/// for which session. What is left lossy here is only the byte quantisation itself —
/// `Solid::fraction`'s own `1/255` of a tile — which both backends share and
/// decision 9's own parity tolerance already absorbs.
/// The `z` span [`walk_cells_streaming`] is entitled to read: the one
/// [`crate::occlusion::Occlusion::solid_z_bytes`] carries, and **not**
/// [`crate::occlusion::Solid::low`]/`high`'s exact corners.
///
/// The vertical half of the discipline [`crate::occlusion::Solid::fraction`]
/// already states for the horizontal one: this walk exists to preview exactly
/// what `blit.wgsl` can do, and a CPU reading full precision where the GPU reads
/// a quantised field silently stops being that preview.
/// `docs/lighting_height.md` phase 2.
fn wire_span(stands: &crate::occlusion::Solid) -> (f32, f32) {
    crate::occlusion::Solid::span_from_bytes(stands.z_bytes())
}

fn walk_cells_streaming(
    from: [f32; 3],
    to: [f32; 3],
    lit: LitEnd,
    skip_last: bool,
    spread: f32,
    occlusion: &Occlusion,
) -> (f32, Option<Stopper>) {
    // See [`walk_cells_exact`]'s own copy: the fragment's height, before
    // [`stand_clear`] moves the ray off it.
    let drawn_z = from[2];
    let (from, to) = stand_clear(from, to, lit.surface);
    let first = lit.tile;
    let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let ground = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
    if ground < 1e-6 {
        // Same shortcut [`walk_cells_exact`] takes: no direction to walk in, so
        // only a lid on the one cell can stand between the ends — and only one
        // this ray is under, which is the footprint gate below.
        //
        // Both halves of the box are the **wire's**, not `space`'s, because this
        // walk is the preview of what the GPU reads: [`wire_span`] for the
        // height, since a lid at a fractional `z` is quantised to a byte on the
        // way up, and [`crate::occlusion::Solid::box_from_footprint`] for the
        // horizontal extent, which is quantised the same way. The main path
        // below reconstructs exactly this pair for exactly this reason; a
        // shortcut that read `space` directly would be a second, finer answer to
        // a question the shader cannot ask that precisely.
        let mut stopped: f32 = 0.0;
        let mut worst: Option<Stopper> = None;
        for (stands, owner) in occlusion.cell(first.0, first.1) {
            if stands.edges != 0 {
                continue;
            }
            let (low, high) = wire_span(stands);
            let space =
                crate::occlusion::Solid::box_from_footprint(first.0, first.1, low, high, stands.fraction());
            if !over_footprint(from, &space) {
                continue;
            }
            // Phase 4's rule — [`walk_cells_exact`]'s own copy of this says why.
            if lit.owner.same(owner) && drawn_on(drawn_z, low, high) {
                continue;
            }
            let by_surface =
                f32::from(stands.opacity) / 255.0 * crosses(from[2], to[2], low, high, to[2], spread);
            if by_surface > stopped {
                stopped = by_surface;
                worst = Some(Stopper {
                    cell: first,
                    owner,
                    edges: stands.edges,
                    span: (low, high),
                });
            }
        }
        return match stopped >= 1.0 - RAY_CUTOFF {
            true => (0.0, Some(worst.expect("a lid took the ray to nothing"))),
            false => (1.0 - stopped, None),
        };
    }
    let last = (to[0].floor() as i32, to[1].floor() as i32);
    let own = match lit.surface.face() {
        Some(face) => crate::occlusion::edges_of(Some(crate::facing::Facing::One(face))),
        None => 0,
    };
    let exemption_ctx = ExemptionContext {
        first,
        last,
        skip_last,
        own,
        owner: lit.owner,
        spot_z: from[2],
        drawn_z,
        to_z: to[2],
    };

    let toward = (
        if delta[0] >= 0.0 { 1 } else { -1 },
        if delta[1] >= 0.0 { 1 } else { -1 },
    );
    let mut per_tile = [1e30_f32; 2];
    let mut boundary = [1e30_f32; 2];
    for axis in 0..2 {
        if delta[axis].abs() <= 1e-6 {
            continue;
        }
        per_tile[axis] = 1.0 / delta[axis].abs();
        let coord = [from[0], from[1]][axis];
        let edge = [first.0, first.1][axis] as f32;
        let ahead = match delta[axis] >= 0.0 {
            true => edge + 1.0 - coord,
            false => coord - edge,
        };
        boundary[axis] = ahead * per_tile[axis];
    }

    // One candidate tile's exact occlusion, folded into `through` — every
    // solid on it tested with its own exact `ray_vs_solid` interval rather
    // than a cell-shared one, [`walk_cells_exact`]'s own per-tile block
    // restated over a single cell at a time. Nothing here reads which tile
    // blamed the ray, so this can run in any order and as many times as the
    // caller likes — the enumeration below is what has to visit each
    // relevant cell exactly once, not this.
    // Returns the solid that took the most of the ray on this cell, so the
    // caller can name it where the cutoff trips — the same `>`-not-`>=` tie rule
    // [`walk_cells_exact`] keeps. `None` where nothing on the cell touched it.
    let apply = |cell: (i32, i32), through: &mut f32| -> Option<Stopper> {
        let mut stopped: f32 = 0.0;
        let mut worst: Option<Stopper> = None;
        for (stands, owner) in occlusion.cell(cell.0, cell.1) {
            // The wire's own span, quantised exactly as the upload quantises
            // it — the vertical half of the same discipline `stands.fraction()`
            // below is the horizontal half of. See [`wire_span`].
            let (low, high) = wire_span(stands);
            let space =
                crate::occlusion::Solid::box_from_footprint(cell.0, cell.1, low, high, stands.fraction());
            let Some((entered, leaves)) = ray_vs_solid(from, to, &space) else {
                continue;
            };
            let Exemption { exempt, same_run } = exemption(&exemption_ctx, cell, stands, owner, low, high);
            if exempt {
                continue;
            }
            let middle = (entered + leaves) * 0.5;
            let soft =
                (spread * middle / (1.0 - middle).max(1e-3)).clamp(SOFT_CROSSING_MIN, SOFT_CROSSING_MAX);
            let opacity = f32::from(stands.opacity) / 255.0;
            let tall = soft * FLAME_DEPTH;
            let by_surface = match stands.edges {
                0 => {
                    // Same tile-footprint lookup [`walk_cells_exact`] needs
                    // for the same reason: a lid's own box is flat in `z`,
                    // so its own `entered`/`leaves` collapse to one instant
                    // rather than the before/after pair [`crosses`] needs.
                    // `space` here, not the whole tile — see
                    // `walk_cells_exact`'s own copy of this comment.
                    let footprint = crate::solid::Solid {
                        min: crate::camera::WorldSpot {
                            x: space.min.x,
                            y: space.min.y,
                            z: -1e6,
                        },
                        max: crate::camera::WorldSpot {
                            x: space.max.x,
                            y: space.max.y,
                            z: 1e6,
                        },
                    };
                    let (tile_entered, tile_leaves) =
                        ray_vs_solid(from, to, &footprint).unwrap_or((entered, leaves));
                    let from_z = from[2] + delta[2] * tile_entered;
                    let to_z = from[2] + delta[2] * tile_leaves;
                    opacity * crosses(from_z, to_z, low, high, to[2], spread)
                }
                // Same as `walk_cells_exact`'s own copy: a `Some` from the
                // exact `ray_vs_solid` slab test already means a genuine
                // crossing, so occlusion is the body's own opacity. See
                // `docs/lighting_raymarch.md`'s "hard shadows" decision.
                EDGE_ANY => opacity,
                edges => {
                    if edges & !same_run == 0 {
                        0.0
                    } else {
                        let cross = [
                            from[0] + delta[0] * middle,
                            from[1] + delta[1] * middle,
                            from[2] + delta[2] * middle,
                        ];
                        opacity * pierced(stands, low, high, cross, soft, tall)
                    }
                }
            };
            if by_surface > stopped {
                stopped = by_surface;
                worst = Some(Stopper {
                    cell,
                    owner,
                    edges: stands.edges,
                    span: (low, high),
                });
            }
        }
        *through *= 1.0 - stopped;
        worst
    };

    let mut through = 1.0_f32;
    let mut cell = first;
    for _ in 0..MAX_WALK_STEPS {
        let worst = apply(cell, &mut through);
        if through <= RAY_CUTOFF {
            return (
                0.0,
                Some(worst.expect("a cell that trips the cutoff has a solid that did it")),
            );
        }
        let next = boundary[0].min(boundary[1]);
        if next >= 1.0 {
            break;
        }
        // Plain single-axis DDA drives the walk's own trajectory — the
        // nearer boundary only, never skipping a cell or taking both. See
        // this function's own doc comment for why an unconditional probe of
        // the *trajectory itself* was tried and ruled out: a walk that never
        // skips a cell is complete by construction, for one CPU
        // implementation checked against itself.
        //
        // **The probe below is unconditional for a different reason, and
        // does not change the trajectory.** A first attempt gated it behind
        // `(boundary[0] - boundary[1]).abs() <= TIE_EPSILON` — cheaper, and
        // wrong: CPU and GPU do not compute a close-enough `boundary[0] -
        // boundary[1]` to agree on *which rays count as near a tie*, so one
        // backend would probe a ray the other did not, and the extra
        // occlusion only one side found showed up as a real parity failure
        // (`the_shader_and_light_sample_agree_about_a_wall_that_faces_away`
        // and others, found widening the gate rather than narrowing it —
        // more rays fell on the wrong side of the asymmetry, not fewer).
        // Probing every transition removes the asymmetry instead of trying
        // to size it away: `candidate_tiles` already names both single-axis
        // neighbours at every transition regardless of any tie, so
        // `walk_cells_exact` already tests whatever this probes, and testing
        // the same cell twice on this side changes nothing it could
        // disagree with.
        let out_by_x = boundary[0] < boundary[1];
        let probe = match out_by_x {
            true => (cell.0, cell.1 + toward.1),
            false => (cell.0 + toward.0, cell.1),
        };
        let worst = apply(probe, &mut through);
        if through <= RAY_CUTOFF {
            return (
                0.0,
                Some(worst.expect("a probe that trips the cutoff has a solid that did it")),
            );
        }
        match out_by_x {
            true => {
                cell.0 += toward.0;
                boundary[0] += per_tile[0];
            }
            false => {
                cell.1 += toward.1;
                boundary[1] += per_tile[1];
            }
        }
    }
    (through, None)
}

/// [`walk`], through [`walk_cells_exact`] instead of [`walk_cells`] — for
/// `docs/lighting_raymarch.md`'s point 3 agreement pass, not for anywhere
/// real.
fn walk_exact(spot: Spot, light: &Light, occlusion: &Occlusion) -> (f32, Option<Stopper>) {
    walk_cells_exact(
        [spot.at.x, spot.at.y, spot.z],
        [light.at.x, light.at.y, light.z],
        LitEnd::of(spot),
        true,
        FLAME_SPREAD,
        occlusion,
    )
}

/// [`walk_sun`], through [`walk_cells_exact`] instead of [`walk_cells`] —
/// see [`walk_exact`].
fn walk_sun_exact(spot: Spot, sun: Sun, occlusion: &Occlusion) -> (f32, Option<Stopper>) {
    let horizontal = (sun.toward[0] * sun.toward[0] + sun.toward[1] * sun.toward[1]).sqrt();
    if horizontal < 1e-6 {
        return (1.0, None);
    }
    let step = [
        sun.toward[0] / horizontal,
        sun.toward[1] / horizontal,
        sun.toward[2] / horizontal * Z_PER_TILE,
    ];
    let mut tiles = MAX_SUN_TILES;
    if let (Some(ceiling), true) = (occlusion.tallest(), step[2] > 1e-6) {
        tiles = tiles.min((ceiling as f32 - spot.z) / step[2]);
    }
    if occlusion.tallest().is_none() || tiles <= 0.0 {
        return (1.0, None);
    }
    let from = [spot.at.x, spot.at.y, spot.z];
    let to = [
        from[0] + step[0] * tiles,
        from[1] + step[1] * tiles,
        from[2] + step[2] * tiles,
    ];
    walk_cells_exact(from, to, LitEnd::of(spot), false, 0.0, occlusion)
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
pub fn carried(at: Point, facing: Direction, time: f32) -> Light {
    let (dx, dy) = facing.step();
    Light {
        beam: Some(Beam::towards(dx as f32, dy as f32, 0.0, HELD_BEAM_DEGREES)),
        ..place(at, TORCH, time)
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
    use openshard_protocol::wire::Hue;
    use openshard_uofiles::map::LandCell;
    use openshard_uofiles::tiledata::{StaticTile, TileFlags};

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

    /// A map with ground and nothing standing on it: the statics in these tests
    /// come from the item list, which is the half a test can build without a
    /// client install.
    fn bare() -> Map {
        Map::from_blocks(1, 1, |_, _| LandCell { tile: 0, z: 0 })
    }

    /// A lid stops a ray that goes through it and nothing that runs along it.
    ///
    /// The three cases [`crosses`] exists for, on a floor of the height a real
    /// one has — **zero** — which is the number the rule it replaced could not
    /// answer for. A candle standing on a floor and the floor it lights are at
    /// one `z`, so the ray between them runs exactly along the plane; a test
    /// that only asked about the crossing would pass with a rule that laid half
    /// a floor's shadow across every room lit from inside it.
    ///
    /// The flame's own `z` is the fourth argument, and it is what softens the
    /// answer: a torch a storey below the floor is wholly under it, and what
    /// comes through is nothing.
    #[test]
    fn a_floor_stops_a_ray_through_it_and_not_one_along_it() {
        // A ray from a wall pixel at 25 down to a torch at 5, crossing this
        // cell between 22 and 18: through the floor at 20.
        assert_eq!(crosses(22.0, 18.0, 20.0, 20.0, 5.0, FLAME_SPREAD), 1.0);
        // The same floor, and a flame standing on it: the ray runs along the
        // plane and has gone through nothing.
        assert_eq!(crosses(20.0, 20.0, 20.0, 20.0, 20.0, FLAME_SPREAD), 0.0);
        // And a ray wholly above it — a lamp on the upper storey lighting the
        // upper storey — is not touched by the floor under both of them.
        assert_eq!(crosses(25.0, 23.0, 20.0, 20.0, 26.0, FLAME_SPREAD), 0.0);
        // A flame *in* the plane of the lid is half cut by it: it is a body
        // about a tile across, and half of it is on either side.
        assert!((crosses(22.0, 18.0, 20.0, 20.0, 20.0, FLAME_SPREAD) - 0.5).abs() < 1e-6);
    }

    /// A one-tile-square, fully opaque panel or body, for the small pure
    /// helpers below — the four numbers a scene is actually about, the same
    /// way `occlusion.rs`'s own `stands_at` is, but built directly since that
    /// one is private to `occlusion`'s own test module.
    fn test_solid(bottom: i32, top: i32, edges: u8) -> crate::occlusion::Solid {
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
        }
    }

    /// [`inside`]'s own shape, checked at the three places its doc comment
    /// makes a claim about: the middle (fully in), and each edge (exactly
    /// half, since the band straddles it symmetrically) — the one thing that
    /// tells `inside` apart from [`pierces`], whose band does not straddle.
    #[test]
    fn inside_is_full_at_the_middle_and_half_at_each_edge() {
        assert_eq!(inside(50.0, 0.0, 100.0, 4.0), 1.0);
        assert!((inside(0.0, 0.0, 100.0, 4.0) - 0.5).abs() < 1e-6);
        assert!((inside(100.0, 0.0, 100.0, 4.0) - 0.5).abs() < 1e-6);
        assert_eq!(inside(-10.0, 0.0, 100.0, 4.0), 0.0);
        assert_eq!(inside(110.0, 0.0, 100.0, 4.0), 0.0);
    }

    /// The shape [`inside`]'s own doc comment claims but the example above
    /// does not check on its own: always in `0.0..=1.0`, and symmetric about
    /// the interval's own centre, over arbitrary intervals and positions.
    #[test]
    fn inside_is_clamped_and_symmetric_about_the_intervals_centre() {
        use proptest::prelude::*;

        proptest!(ProptestConfig::with_cases(512), |(
            low in -50.0_f32..50.0,
            width in 0.1_f32..50.0,
            band in 0.01_f32..10.0,
            frac in -0.5_f32..1.5,
        )| {
            let high = low + width;
            let x = low + frac * width;
            let value = inside(x, low, high, band);
            prop_assert!((0.0..=1.0).contains(&value));

            let mirrored = low + high - x;
            let value2 = inside(mirrored, low, high, band);
            prop_assert!(
                (value - value2).abs() < 1e-3,
                "inside({x}) = {value}, inside({mirrored}) = {value2}, should agree by symmetry",
            );
        });
    }

    /// [`pierces`]'s one asymmetry, stated as numbers: the band is centred on
    /// the *top* edge (half blocked exactly there) and hangs below the
    /// bottom one rather than straddling it — so the bottom edge itself is
    /// still fully blocked, and the halfway point is a whole `band / 2`
    /// below it. This is the whole of why `pierces` is a second function
    /// from [`inside`] rather than a call of it, checked directly rather
    /// than trusted from the doc comment's argument.
    #[test]
    fn pierces_centres_its_band_on_the_top_edge_only() {
        assert_eq!(pierces(10.0, 0.0, 20.0, 2.0), 1.0);
        assert!((pierces(20.0, 0.0, 20.0, 2.0) - 0.5).abs() < 1e-6);
        assert_eq!(pierces(30.0, 0.0, 20.0, 2.0), 0.0);
        assert_eq!(pierces(0.0, 0.0, 20.0, 2.0), 1.0);
        assert!((pierces(-1.0, 0.0, 20.0, 2.0) - 0.5).abs() < 1e-6);
    }

    /// Which axis [`run_v`] reads is the edge mask, and the fractional part
    /// is `along - along.floor()` rather than [`f32::fract`] — the two
    /// differ in sign for a negative coordinate, and a wall running through
    /// negative world space (west or north of the map's own origin) is a
    /// real scene, not a corner case invented for this test.
    #[test]
    fn run_v_reads_the_axis_the_edges_name_and_floors_rather_than_fracts() {
        assert!((run_v(crate::occlusion::EDGE_NORTH, 3.75, 9.25) - 0.75).abs() < 1e-6);
        assert!((run_v(crate::occlusion::EDGE_EAST, 3.75, 9.25) - 0.25).abs() < 1e-6);
        // `(-3.25).fract()` is `-0.25` in Rust; the correct run fraction is
        // `0.75`, which is what a floor-based fraction gives and `fract`
        // does not.
        assert!((run_v(crate::occlusion::EDGE_NORTH, -3.25, 0.0) - 0.75).abs() < 1e-6);
    }

    /// [`hole`]'s two claims: nothing without an aperture, and — with one —
    /// the [`inside`]-shaped soft rectangle it documents, checked at a point
    /// deep in both spans and at two points each outside one of them.
    #[test]
    fn hole_is_zero_with_no_aperture_and_the_soft_rectangle_with_one() {
        assert_eq!(hole(None, 0.5, 10.0, 0.1, 0.1), 0.0);

        let aperture = crate::occlusion::Aperture::new(0.25, 0.75, 5, 15);
        assert!((hole(Some(aperture), 0.5, 10.0, 0.05, 1.0) - 1.0).abs() < 1e-3);
        assert!(hole(Some(aperture), 0.9, 10.0, 0.05, 1.0) < 1e-3);
        assert!(hole(Some(aperture), 0.5, 30.0, 0.05, 1.0) < 1e-3);
    }

    /// [`pierced`] with no aperture is exactly [`pierces`] — the surface it
    /// is asked about is solid — and with one, a point deep inside the hole
    /// is open while the same height beside the hole is still stopped by the
    /// wall around it.
    #[test]
    fn pierced_is_pierces_with_no_hole_and_open_where_the_hole_is() {
        let wall = test_solid(0, 20, crate::occlusion::EDGE_NORTH);
        assert_eq!(
            pierced(&wall, wall.low(), wall.high(), [0.5, 0.0, 10.0], 0.05, 2.0),
            pierces(10.0, 0.0, 20.0, 2.0),
        );

        let mut windowed = wall;
        windowed.aperture = Some(crate::occlusion::Aperture::new(0.25, 0.75, 5, 15));
        assert!(pierced(&windowed, 0.0, 20.0, [0.5, 0.0, 10.0], 0.05, 1.0) < 1e-3);
        assert!((pierced(&windowed, 0.0, 20.0, [0.9, 0.0, 10.0], 0.05, 1.0) - 1.0).abs() < 1e-3);
    }

    /// [`own_run`]'s bitmask logic, exhaustively over its four shapes: same
    /// row, same column, neither, and `first` itself (both at once) — the
    /// whole of its finite domain in the two facts that matter (row and
    /// column), so this is exhaustive rather than a sample.
    #[test]
    fn own_run_keeps_only_the_sides_on_the_same_row_or_column_as_the_start() {
        let own = crate::occlusion::EDGE_NORTH | crate::occlusion::EDGE_EAST;
        let first = (5, 5);
        assert_eq!(own_run(own, (8, 5), first), crate::occlusion::EDGE_NORTH);
        assert_eq!(own_run(own, (5, 9), first), crate::occlusion::EDGE_EAST);
        assert_eq!(own_run(own, (8, 9), first), 0);
        assert_eq!(own_run(own, first, first), own);
    }

    /// [`Surface::face`]'s own outward normal is the only thing
    /// [`stand_clear`] nudges by — no face nudges neither axis, and a face
    /// nudges only the axis its own [`Face::outward`] names, never the far
    /// end of the ray.
    #[test]
    fn stand_clear_nudges_only_along_a_faces_own_outward_normal() {
        let from = [10.0_f32, 20.0, 5.0];
        let to = [15.0, 25.0, 8.0];

        let (nudged_from, nudged_to) = stand_clear(from, to, Surface::Upright);
        assert_eq!(nudged_from, [10.0, 20.0, 5.0 + ON_TOP]);
        assert_eq!(nudged_to, [15.0, 25.0, 8.0 + ON_TOP]);

        let (nudged_from, nudged_to) = stand_clear(from, to, Surface::Face(Face::East));
        assert_eq!(nudged_from, [10.0 + STAND_OFF, 20.0, 5.0 + ON_TOP]);
        assert_eq!(nudged_to, [15.0, 25.0, 8.0 + ON_TOP]);
    }

    /// [`on_surface`]'s own inclusiveness, at both ends and by exactly
    /// [`ON_TOP`] — the tolerance [`stand_clear`] gave the point and has to
    /// be given back here, per that function's own doc comment.
    #[test]
    fn on_surface_is_inclusive_of_both_ends_by_exactly_on_top() {
        let wall = test_solid(0, 20, crate::occlusion::EDGE_NORTH);
        let (low, high) = (wall.low(), wall.high());
        assert!(on_surface(0.0, low, high));
        assert!(on_surface(20.0, low, high));
        assert!(on_surface(20.0 + ON_TOP, low, high));
        assert!(!on_surface(20.0 + ON_TOP * 2.0, low, high));
        assert!(!on_surface(0.0 - ON_TOP * 2.0, low, high));
    }

    /// And that the span it is inclusive of is the caller's own, fraction
    /// included — `docs/lighting_height.md` phase 2's whole point on this side.
    ///
    /// A box based half a unit up is the case the plan's control scene
    /// (`OPENSHARD_TREE_H1=3.5`) is made of: rounded to `4`, the bottom half
    /// unit of the box's own faces reads as *below* the box, and every rule that
    /// asks whether a fragment belongs to the thing it was drawn from answers
    /// no for it.
    #[test]
    fn on_surface_reads_a_fractional_span_and_not_a_rounded_one() {
        let box_at_half = crate::occlusion::Solid {
            space: crate::solid::Solid {
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
            opacity: 255,
            edges: crate::occlusion::EDGE_ANY,
            aperture: None,
            roof: false,
            owner: crate::occlusion::Owner::new(3, openshard_protocol::wire::Graphic(0)),
        };
        let (low, high) = (box_at_half.low(), box_at_half.high());
        assert_eq!((low, high), (3.5, 6.5));
        assert!(
            on_surface(3.6, low, high),
            "a fragment of the box's own face, a tenth of a unit up it"
        );
        assert!(
            !on_surface(3.4, low, high),
            "and one below the box entirely, which `bottom()`'s own rounding to 4 could not tell apart"
        );
        // And the same span off the wire, which is what the GPU reads. A half
        // is a whole number of `Solid::Z_STEPS`, so this is exact rather than
        // near — the step being a power of two is what buys that.
        assert_eq!(wire_span(&box_at_half), (3.5, 6.5));
    }

    /// A fragment is exempt from **the occluder it is a point of**, and from no
    /// other on its own cell — even one whose span is exactly the same.
    ///
    /// `docs/lighting_height.md` phase 3, stated at the function the phase is
    /// about. The two solids here are deliberately identical in every geometric
    /// fact `on_surface` could have read: same tile, same span, same kind. That
    /// is what the old test could not tell apart in either direction, and it is
    /// not a corner case — it is two things standing side by side on one tile,
    /// which `examples/boxes.rs`'s `pair` scene draws and which read
    /// 1296/1296, 1248/1248 and 9216/9216 fully wrong before this.
    ///
    /// Mutating `lit_end` back to `on_surface(ctx.spot_z, low, high)` turns the
    /// second assertion red and leaves the first green, which is what says the
    /// two are one property and not a restatement of the same one.
    #[test]
    fn a_fragment_is_exempt_from_its_own_solid_and_from_a_twin_of_it_beside_it() {
        let mine = test_solid(0, 20, crate::occlusion::EDGE_ANY);
        let theirs = test_solid(0, 20, crate::occlusion::EDGE_ANY);
        assert_eq!(
            (mine.low(), mine.high()),
            (theirs.low(), theirs.high()),
            "the scene is only a test of identity while the two spans are equal",
        );
        let (first, elsewhere) = ((100, 100), (101, 100));
        let (ours, other) = (
            crate::occlusion::OwnerId::from_raw(1),
            crate::occlusion::OwnerId::from_raw(2),
        );
        let ctx = ExemptionContext {
            first,
            last: (105, 100),
            skip_last: true,
            own: 0,
            owner: ours,
            // Halfway up both spans, so a height test would answer "mine" for
            // either of them.
            spot_z: 10.0,
            drawn_z: 10.0,
            to_z: 10.0,
        };
        let exempt = |cell, owner| exemption(&ctx, cell, &mine, owner, mine.low(), mine.high()).exempt;
        assert!(
            exempt(first, ours),
            "a fragment is shadowed by the thing it is drawn from"
        );
        assert!(
            !exempt(first, other),
            "the thing beside it, at the same height, is not the thing it is drawn from",
        );
        // And the gate that was always there stays: identity is asked only about
        // the fragment's own cell, which is what lets one byte a tile be enough.
        assert!(
            !exempt(elsewhere, ours),
            "a solid on another tile is never `lit_end`"
        );
        // A fragment of nothing — the ground, a mobile — is exempt from nothing,
        // including from another point of nothing.
        let none = ExemptionContext {
            owner: crate::occlusion::OwnerId::NONE,
            ..ctx
        };
        assert!(
            !exemption(
                &none,
                first,
                &mine,
                crate::occlusion::OwnerId::NONE,
                mine.low(),
                mine.high(),
            )
            .exempt,
            "two absences of an owner read as one owner",
        );
    }

    /// Every authored light value is exactly `srgb_to_linear` of the number a
    /// person chose, and the numbers a person chose are in this test.
    ///
    /// `docs/lighting_rebuild.md` phase 1 moved the multiplication into linear
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

    /// [`faces`]'s own gradient: fully towards the light, fully away, and
    /// exactly edge-on in between — [`FACE_EDGE`]'s own three named points.
    #[test]
    fn faces_is_one_towards_the_light_and_zero_away_from_it() {
        let toward = [1.0_f32, 0.0, 0.0];
        assert_eq!(faces([1.0, 0.0, 0.0], toward), 1.0);
        assert_eq!(faces([-1.0, 0.0, 0.0], toward), 0.0);
        assert!((faces([0.0, 1.0, 0.0], toward) - 0.5).abs() < 1e-6);
    }

    /// The identity is exactly that: the blit has a case where it must not touch
    /// a single byte, and this is what says so.
    #[test]
    fn the_empty_lighting_is_the_identity() {
        assert!(Lighting::NONE.is_identity());
        assert!(
            !Lighting {
                ambient: NIGHT,
                ..Lighting::NONE
            }
            .is_identity()
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
            at: Point::new(100, 100, 0),
            graphic: Graphic(0x0FAE),
            hue: Hue::NONE,
        }];
        let lighting = collect(
            &bare(),
            &items,
            &camera,
            // Flagged, but a *different* graphic.
            &lit(0x0A12),
            &Cutaway::OPEN,
            NIGHT,
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
            at: Point::new(100, 100, 0),
            graphic,
            hue: Hue::NONE,
        }];
        let tiledata = lit(graphic.0);
        let mut camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let mut zoom = camera.zoom();
        loop {
            camera.zoom_about(400, 300, zoom);
            let lighting = collect(
                &bare(),
                &items,
                &camera,
                &tiledata,
                &Cutaway::OPEN,
                NIGHT,
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
            camera.zoom_about(400, 300, zoom);
            let bounds = lit_tiles(&camera);
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
            0.0,
            None,
            None,
        );
        assert_eq!(lighting.occlusion.bounds(), lit_tiles(&camera));
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
        camera.zoom_about(960, 540, zoom);
        let bounds = lit_tiles(&camera);
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
            0.0,
            None,
            None,
        );
        assert!(lighting.lights.is_empty());
    }

    /// **A tread's own top must not be shadowed by the riser it caps.**
    ///
    /// `occlusion::Builder::add`'s climbable branch gives a tread's top the
    /// `Stance::Flat` normal, which is the same one a room's floor gets, and
    /// [`Surface::shadowed_by_own_tile`] was written for that floor: "a floor
    /// pixel on a wall tile is inside the room, and the ray from it to a lamp in
    /// the street crosses the panel its own tile stands on." A tread's top sits
    /// at the exact height its own riser stops at — `top_z == riser.top()` — so
    /// the same rule reads it as a floor the riser walls in, even though the
    /// riser has nothing standing *above* that height to be between the pixel
    /// and anything. Found looking at a real staircase render: every tread top
    /// read dark towards its own riser regardless of where the torch stood.
    #[test]
    fn a_treads_top_is_not_shadowed_by_its_own_riser() {
        use crate::facing::Prism;
        use crate::occlusion::{Builder, Shape};

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
        let top = occlusion
            .solids_at(100, 100)
            .filter(|solid| solid.edges == 0)
            .max_by_key(|solid| solid.top())
            .expect("the climb built three tops");
        let at = Vec2::new(
            ((top.space.min.x + top.space.max.x) / 2.0) as f32,
            ((top.space.min.y + top.space.max.y) / 2.0) as f32,
        );
        let spot = Spot::flat(at, top.top() as f32, (100, 100));

        // East of the stair, level with the top tread — the foot of the flight,
        // which is where a person actually stands a torch.
        let light = Light {
            at: Vec2::new(102.5, 100.5),
            z: top.top() as f32,
            radius: 6.0,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam: None,
        };
        let lighting = Lighting {
            ambient: NIGHT,
            lights: vec![light],
            occlusion,
            sun: None,
            view: crate::debug::View::default(),
        };

        let sample = sample(spot, &lighting);
        assert!(
            sample.reaches[0].through > 0.9,
            "a tread's own top should not be dimmed by the riser it caps: through {}",
            sample.reaches[0].through,
        );
    }

    /// `docs/lighting_height.md` **phase 4**, at the walk rather than at
    /// [`exemption`]: a fragment is not shadowed by the plane it is drawn on, and
    /// **is** shadowed by every other plane of its own static.
    ///
    /// One flight, three treads `1,3,5`, climbing north on one tile — the scene
    /// `examples/synthetic_stair` draws and the face oracle measured. Its six
    /// solids are one [`crate::occlusion::Builder::add`] and therefore one
    /// [`crate::occlusion::OwnerId`], so identity alone cannot tell any of them
    /// from any other, and everything asserted here is about what stands beside
    /// it. Both walks, because a rule one of them has is a parity gap.
    ///
    /// Three rays, and each one kills a different mutation:
    ///
    /// - **Off a tread's own top, steeply down.** The only solid on the line is
    ///   that tread's own lid and the ray leaves its plane, so the only contact
    ///   is at the origin. Red before this phase — [`stand_clear`]'s [`ON_TOP`]
    ///   lifted the fragment a hundred-and-twenty-eighth clear of its own top and
    ///   turned that contact into a crossing, which is what painted 1522 and 1346
    ///   pixels of the middle and top treads black.
    /// - **Off a riser, up and east, over that flight's own bottom tread.** The
    ///   *same kind* of solid — a lid of the fragment's own static — and it must
    ///   still stop the ray, because that crossing is at `t > 0` and well away
    ///   from where the ray started: a lamp above and beyond a staircase genuinely
    ///   cannot see the front of its bottom step. A fix phrased as "a fragment is
    ///   never shadowed by its own static's lid" lights this one, which is the
    ///   whole reason [`drawn_on`] compares a height instead of stopping at the
    ///   owner.
    /// - **Off a tread's top, down and south, into the riser under it.** A
    ///   *panel* of the fragment's own static, and the fragment is a point of no
    ///   panel of it at all: it is flat, and a flat fragment carries no side.
    ///   This one is red without the `edges & own` arm and green with it, while
    ///   the other two are green either way — which is what says the lid half and
    ///   the panel half are two properties rather than one restated. It is also
    ///   the defect the lid was hiding: with the tread tops still black for the
    ///   wrong reason, this ray's answer could not be read off the picture.
    #[test]
    fn a_fragment_is_shadowed_by_every_plane_of_its_own_static_but_the_one_it_is_drawn_on() {
        use crate::facing::Prism;
        use crate::occlusion::{Builder, Shape};

        let stair = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT | TileFlags::CLIMBABLE),
            height: 20,
            ..StaticTile::default()
        };
        // North, so the treads divide the tile up `y`: tread 0 over
        // `100.667..101` capped at `z 1`, tread 1 over `100.333..100.667` at
        // `z 3`, tread 2 over `100..100.333` at `z 5`, and a riser standing on
        // each strip's own low edge.
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
        let owner = occlusion.owner_at(100, 100, 0, graphic);
        assert!(
            !owner.same(crate::occlusion::OwnerId::NONE),
            "the flight has to have an owner for any of this to be about identity"
        );

        let walked = |spot: Spot, at: Vec2, z: f32| {
            let lighting = Lighting {
                ambient: NIGHT,
                lights: vec![Light {
                    at,
                    z,
                    // Wide enough that nothing here is out of reach: this is a
                    // test about what stands in the way, not about falloff.
                    radius: 40.0,
                    color: [1.0, 1.0, 1.0],
                    intensity: 1.0,
                    beam: None,
                }],
                occlusion: occlusion.clone(),
                sun: None,
                view: crate::debug::View::default(),
            };
            let streaming = sample(spot, &lighting).reaches[0].through;
            let exact = sample_exact(spot, &lighting).reaches[0].through;
            (streaming, exact)
        };

        // 1. Off the top tread's own top, down past the flight. Nothing else is
        //    under it: the two lower treads' lids are strips of `y` this ray is
        //    never over, and every riser stands on a `y` it never reaches.
        let on_top = Spot::flat(Vec2::new(100.5, 100.15), 5.0, (100, 100)).owned_by(owner);
        let (streaming, exact) = walked(on_top, Vec2::new(100.6, 100.25), -5.0);
        assert!(
            streaming > 0.99 && exact > 0.99,
            "a tread's own top is a contact at the ray's origin, not a crossing: \
             streaming {streaming}, exact {exact}",
        );

        // 2. The counter-example, and the same lid at `t > 0`: off the bottom
        //    riser's own face, east and up, crossing tread 0's own top a fifth of
        //    the way along and well inside its strip.
        let on_riser = Spot::face(Vec2::new(100.5, 100.99), 0.5, (100, 100), Face::South).owned_by(owner);
        let (streaming, exact) = walked(on_riser, Vec2::new(103.0, 100.5), 5.0);
        assert!(
            streaming < 0.5 && exact < 0.5,
            "the flight's own body is between this riser and a lamp above and beyond it: \
             streaming {streaming}, exact {exact}",
        );

        // 3. The panel half: off the middle tread's top, south and down, straight
        //    into the riser that tread stands against. Same owner, and a surface
        //    the fragment is not a point of.
        let on_middle = Spot::flat(Vec2::new(100.5, 100.4), 3.0, (100, 100)).owned_by(owner);
        let (streaming, exact) = walked(on_middle, Vec2::new(100.5, 101.5), 1.0);
        assert!(
            streaming < 0.5 && exact < 0.5,
            "a flat fragment is a point of no panel, so its own flight's riser stops the ray: \
             streaming {streaming}, exact {exact}",
        );
    }

    /// **A ray with no horizontal run is still only stopped by lids it is
    /// actually under.**
    ///
    /// `docs/lighting_height.md`'s backlog entry, and the reason the ray above
    /// this one is *slanted*: both walks take a shortcut when a ray has no
    /// horizontal run — there is no direction to step in, so only this one cell
    /// can hold anything — and the shortcut applied [`crosses`] to **every** lid
    /// on the cell without asking whether the ray is over that lid at all. The
    /// main path stopped doing that when sub-tile footprints landed; the
    /// shortcut did not follow.
    ///
    /// A flight is exactly where that shows: its three treads are three lids on
    /// one tile, each a *strip* of it, and no point is over more than one of
    /// them. So a fragment on a tread lit from straight above or below was
    /// shadowed by the other two treads — surfaces standing over a part of the
    /// tile it is nowhere near.
    ///
    /// Both directions, because they fail through different lids: from the top
    /// tread downwards the two lower lids are below the fragment and the ray
    /// runs down past them, and from the bottom tread upwards the two higher
    /// lids are above it and the ray runs up past them. A fix that gated only
    /// one end would leave the other reading as a real occlusion.
    ///
    /// Its own tread's lid is not what is being asserted away: that one is
    /// excused by [`drawn_on`] and identity, which is the test above's subject.
    /// Every riser is excused here by having an `edges` at all — the shortcut
    /// has never looked at panels, and a panel stands beside a vertical ray
    /// rather than across it.
    #[test]
    fn a_vertical_ray_is_not_stopped_by_lids_it_is_not_over() {
        use crate::facing::Prism;
        use crate::occlusion::{Builder, Shape};

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
        let owner = occlusion.owner_at(100, 100, 0, graphic);

        let walked = |spot: Spot, at: Vec2, z: f32| {
            let lighting = Lighting {
                ambient: NIGHT,
                lights: vec![Light {
                    at,
                    z,
                    radius: 40.0,
                    color: [1.0, 1.0, 1.0],
                    intensity: 1.0,
                    beam: None,
                }],
                occlusion: occlusion.clone(),
                sun: None,
                view: crate::debug::View::default(),
            };
            (
                sample(spot, &lighting).reaches[0].through,
                sample_exact(spot, &lighting).reaches[0].through,
            )
        };

        // Straight down off the top tread. The flame is directly under the
        // fragment, so the ray's horizontal run is zero by construction rather
        // than by a tolerance — `Spot::flat` carries no outward normal, so
        // `stand_clear` lifts it in `z` alone and cannot nudge it off the line.
        let on_top = Spot::flat(Vec2::new(100.5, 100.15), 5.0, (100, 100)).owned_by(owner);
        let (streaming, exact) = walked(on_top, Vec2::new(100.5, 100.15), -5.0);
        assert!(
            streaming > 0.99 && exact > 0.99,
            "the lower treads are strips of `y` this ray is never over: \
             streaming {streaming}, exact {exact}",
        );

        // And straight up off the bottom tread, where the two lids in question
        // are the ones *above* the fragment.
        let on_bottom = Spot::flat(Vec2::new(100.5, 100.8), 1.0, (100, 100)).owned_by(owner);
        let (streaming, exact) = walked(on_bottom, Vec2::new(100.5, 100.8), 15.0);
        assert!(
            streaming > 0.99 && exact > 0.99,
            "the higher treads are strips of `y` this ray is never over: \
             streaming {streaming}, exact {exact}",
        );
    }

    /// **A wall the flame sits exactly level with can still be skipped whole.**
    ///
    /// `docs/lighting_raymarch.md`'s "A new `walk_cells` miss" backlog entry,
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
        use crate::occlusion::{Builder, Shape};

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
            at: Vec2::new(98.0, 100.0),
            z: 10.0,
            radius: 12.0,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam: None,
        };
        let lighting = Lighting {
            ambient: NIGHT,
            lights: vec![light],
            occlusion,
            sun: None,
            view: crate::debug::View::default(),
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

    /// The pure-geometry echo of
    /// [`a_wall_level_with_the_flame_is_not_skipped_by_a_shallow_ray`]: the
    /// same six spots and the same wall row, but asking [`dda_walk`] alone
    /// whether the cell sequence ever visits `(100, 100)` — no [`Occlusion`],
    /// no [`Lighting`], no `sample`. This is
    /// `docs/lighting_raymarch.md`'s "A new `walk_cells` miss" — a ray
    /// hugging a row's own grid line skipping the row entirely — at the
    /// layer where it actually lives, checked against cell numbers instead
    /// of a lit scene.
    ///
    /// `y 99.9`'s `false` is not a typo: the straight segment from `(102.5,
    /// 99.9)` to `(98.0, 100.0)` never reaches `y >= 100` before its own
    /// endpoint, so the geometrically correct walk never sets foot in the
    /// wall's row at all — see the backlog entry's own correction of the
    /// original six-point table.
    #[test]
    fn the_dda_walk_does_not_skip_the_wall_row_on_a_shallow_ray() {
        for (y, visits_the_wall_row) in [
            (99.9_f32, false),
            (100.1, true),
            (100.2, true),
            (100.3, true),
            (101.0, true),
        ] {
            let tile = (102, y.floor() as i32);
            let steps = dda_walk(Vec2::new(102.5, y), Vec2::new(98.0, 100.0), tile);
            let visited = steps.iter().any(|step| step.cell == (100, 100));
            assert_eq!(
                visited,
                visits_the_wall_row,
                "y {y}: cells were {:?}",
                steps.iter().map(|step| step.cell).collect::<Vec<_>>(),
            );
        }
    }

    /// `docs/lighting_raymarch.md`'s ray-vs-Solid scoping, point 3, over the
    /// three-tread climbable stair
    /// [`a_treads_top_is_not_shadowed_by_its_own_riser`] uses. This is the
    /// scene that found a real bug in [`walk_cells_exact`], not just
    /// another `walk_cells` gap: a lid is flat in `z`
    /// (`Solid::box_of`'s `min.z == max.z`) and a riser is flat in the
    /// climb axis (`Solid::tread_riser_box_of`'s own doc comment: "a
    /// plane, not a strip") — [`ray_vs_solid`]'s slab method correctly
    /// collapses `entered` and `leaves` to the exact same instant on
    /// either one, since a degenerate-thickness box is genuinely crossed
    /// at one point in `t`, not over an interval. [`crosses`] was never
    /// built for that: it reads `entering`/`leaving` as the ray's `z` on
    /// *either side* of a crossing to tell "went through" from "never
    /// close," and a from/to that already collapsed to the same value
    /// answers every comparison in it as "never" — regardless of the real
    /// geometry. `walk_cells_exact` read every lid as fully transparent
    /// before this was caught, `1.0` unconditionally.
    ///
    /// **Fixed by asking a different question for the lid branch**: not
    /// "where does the ray touch this lid's own (degenerate) box" but
    /// "where does the ray enter and leave the *tile's* footprint" — a
    /// second `ray_vs_solid` call against a synthetic box sharing the
    /// tile's `x`/`y` bounds with `z` left unconstrained, giving
    /// `crosses` the before/after pair it actually needs. The same
    /// question `walk_cells`'s own DDA cell entry/exit answered for free;
    /// `walk_cells_exact` has to ask it explicitly since it no longer
    /// walks cells at all.
    ///
    /// This test pins the regression at the exact input the stair scene's
    /// own fuzz found it at, rather than trusting the fix by reasoning
    /// alone: reverting the tile-footprint lookup back to the lid's own
    /// `entered`/`leaves` reproduces `walk_cells_exact` reading fully open
    /// (`1.0`) here, confirmed by hand before this test was written.
    #[test]
    fn walk_cells_exact_does_not_read_every_lid_as_transparent() {
        use crate::facing::Prism;
        use crate::occlusion::{Builder, Shape};

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
        let tile = (from[0].floor() as i32, from[1].floor() as i32);
        let new = walk_cells_exact(from, to, LitEnd::nowhere(tile), true, FLAME_SPREAD, &occlusion);
        assert!(
            new.0 < 0.5,
            "a ray crossing the first tread's own lid should not read as more than half open: \
             through {} (blamed {:?})",
            new.0,
            new.1,
        );
    }

    /// `docs/lighting_raymarch.md`'s ray-vs-Solid scoping, point 3, over the
    /// same stair scene as
    /// [`walk_cells_exact_does_not_read_every_lid_as_transparent`] — a
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
    /// direction, and telling that apart from a `walk_cells_exact` bug
    /// needs the same exemption predicates (`on_surface`, `own_run`,
    /// `flame_end`) evaluated the same way a disagreement-characterising
    /// test would have to duplicate — a real next step, not attempted
    /// here. What this checks instead: `walk_cells_exact` never panics and
    /// never returns a `through` outside `0.0..=1.0` over a broad fuzz of
    /// this richer scene — the lid bug above would have shown up here too,
    /// as values pinned at `1.0` far more often than the geometry allows.
    #[test]
    fn walk_cells_exact_stays_in_range_on_the_stair() {
        use crate::facing::Prism;
        use crate::occlusion::{Builder, Shape};
        use proptest::prelude::*;

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
            let tile = (fx.floor() as i32, fy.floor() as i32);
            let from = [fx, fy, fz];
            let to = [tx, ty, tz];
            let new = walk_cells_exact(from, to, LitEnd::nowhere(tile), true, FLAME_SPREAD, &occlusion);
            prop_assert!((0.0..=1.0).contains(&new.0), "from {from:?} to {to:?}: through {}", new.0);
        });
    }

    /// `docs/lighting_raymarch.md`'s point 4, the same six-point counter-
    /// example this whole track started from — full numeric agreement with
    /// [`walk_cells_exact`]. This is a single whole-tile body
    /// (`Shape::UNREAD`), so [`crate::occlusion::Solid::box_of`]'s
    /// reconstruction is bit-for-bit the solid's own real `space`, and this
    /// is the case [`walk_cells_streaming`]'s own doc comment claims exact
    /// agreement for.
    #[test]
    fn walk_cells_streaming_agrees_with_walk_cells_exact_on_the_six_point_counter_example() {
        use crate::occlusion::{Builder, Shape};

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
            let tile = (102, y.floor() as i32);
            let from = [102.5_f32, y, 10.0];
            let exact =
                walk_cells_exact(from, flame, LitEnd::nowhere(tile), true, FLAME_SPREAD, &occlusion).0;
            let streaming =
                walk_cells_streaming(from, flame, LitEnd::nowhere(tile), true, FLAME_SPREAD, &occlusion).0;
            assert!(
                (exact - streaming).abs() < 1e-4,
                "y {y}: walk_cells_exact through {exact} disagrees with walk_cells_streaming through {streaming}",
            );
        }
    }

    /// `docs/lighting_raymarch.md`'s point 4, over the same single-body wall
    /// scene the six-point counter-example's own occlusion is built from —
    /// **with no corner restriction**, because [`walk_cells_streaming`] has
    /// no corner-jump branch to be restricted away from. Full numeric
    /// agreement with [`walk_cells_exact`] everywhere in the domain.
    #[test]
    fn walk_cells_streaming_agrees_with_walk_cells_exact_on_a_single_body() {
        use crate::occlusion::{Builder, Shape};
        use proptest::prelude::*;

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
            let tile = (fx.floor() as i32, fy.floor() as i32);
            let from = [fx, fy, fz];
            let to = [tx, ty, tz];
            let exact = walk_cells_exact(from, to, LitEnd::nowhere(tile), true, FLAME_SPREAD, &occlusion).0;
            let streaming = walk_cells_streaming(from, to, LitEnd::nowhere(tile), true, FLAME_SPREAD, &occlusion).0;
            prop_assert!(
                (exact - streaming).abs() < 1e-3,
                "from {from:?} to {to:?}: walk_cells_exact {exact} vs walk_cells_streaming {streaming}",
            );
        });
    }

    /// The same claim over a body whose `z` span is **not** a whole number,
    /// which is the case the three tests around this one cannot see at all.
    ///
    /// Every fixture they build goes through `Builder::add` off a `StaticTile`,
    /// so every span in them is a whole `z` and a half — and since
    /// `docs/lighting_height.md` phase 2 the two walks read *different* heights
    /// for one solid on purpose ([`walk_cells_exact`] the record's own `f64`
    /// corners, [`walk_cells_streaming`] the quantised span off the wire, see
    /// [`wire_span`]). On a whole `z` those two are equal by construction, so
    /// their agreement there says nothing about the discipline that keeps them
    /// close anywhere else: the assertion passes on a scene where the thing it
    /// checks cannot differ.
    ///
    /// A base and a top on thirds, well off any step of
    /// [`crate::occlusion::Solid::Z_STEPS`], is what makes the quantisation
    /// actually happen — and the bar stays full numeric agreement, because
    /// half a step of a two-hundred-and-fifty-sixth of a `z` unit is far under
    /// what any of this can be seen through.
    #[test]
    fn walk_cells_streaming_agrees_with_walk_cells_exact_on_a_body_at_a_fractional_z() {
        use crate::occlusion::Builder;
        use proptest::prelude::*;

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
            let tile = (fx.floor() as i32, fy.floor() as i32);
            let from = [fx, fy, fz];
            let to = [tx, ty, tz];
            // **Not every ray can carry this claim, and which cannot is
            // decidable rather than a matter of taste.** The two walks read
            // spans that differ by up to half a step of
            // [`crate::occlusion::Solid::Z_STEPS`] — on purpose, that is the
            // whole subject of this test — and a body's own answer is binary,
            // so wherever the ray's hit is *decided* inside that half step the
            // two must differ by everything rather than by a rounding. The
            // first case proptest found here was exactly that: a ray grazing
            // the box's own bottom-front corner, missing the record's own
            // `1/3` base by three thousandths of `t` and catching the wire's
            // `85/256` one, which is a thousandth of a `z` unit lower.
            //
            // So the guard is the question itself: run the ray against the
            // solid's box with the span pulled a whole quantum in and pushed a
            // whole quantum out, and skip the case when those two disagree
            // about hitting it at all. What is left is every ray whose hit or
            // miss survives the quantisation, and *those* must agree
            // numerically. A tolerance on the input is not a tolerance on the
            // output, and this test asserted the second while meaning the
            // first.
            let solid = occlusion.solids_at(100, 100).next().expect("the fixture's own body");
            let (near, far) = stand_clear(from, to, Surface::Flat);
            let quantum = (1.0 / crate::occlusion::Solid::Z_STEPS) as f32;
            let hits_with = |grown: f32| {
                let mut space = solid.space;
                space.min.z -= f64::from(grown);
                space.max.z += f64::from(grown);
                ray_vs_solid(near, far, &space).is_some()
            };
            prop_assume!(hits_with(-quantum) == hits_with(quantum));

            let exact = walk_cells_exact(from, to, LitEnd::nowhere(tile), true, FLAME_SPREAD, &occlusion).0;
            let streaming = walk_cells_streaming(from, to, LitEnd::nowhere(tile), true, FLAME_SPREAD, &occlusion).0;
            prop_assert!(
                (exact - streaming).abs() < 1e-3,
                "from {from:?} to {to:?}: walk_cells_exact {exact} vs walk_cells_streaming {streaming}",
            );
        });
    }

    /// `docs/lighting_raymarch.md`'s point 4, over a single **panel**
    /// (`Shape::faced`) — the branch [`walk_cells_exact_disagreements_are_
    /// backed_by_ray_vs_solid`]'s own doc comment flags as the one
    /// deliberate simplification (one [`pierced`] sample at the crossing's
    /// own midpoint). A panel's box is `PANEL_THICKNESS`-inset from the
    /// plane but still exactly what [`crate::occlusion::Solid::box_of`]
    /// builds for it, so full numeric agreement is the right bar here too,
    /// not the weaker "stronger answer is backed" claim the `walk_cells`
    /// comparison needed.
    #[test]
    fn walk_cells_streaming_agrees_with_walk_cells_exact_on_a_single_panel() {
        use crate::facing::Facing;
        use crate::occlusion::{Builder, Shape};
        use proptest::prelude::*;

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
            let tile = (fx.floor() as i32, fy.floor() as i32);
            let from = [fx, fy, fz];
            let to = [tx, ty, tz];
            let exact = walk_cells_exact(from, to, LitEnd::nowhere(tile), true, FLAME_SPREAD, &occlusion).0;
            let streaming = walk_cells_streaming(from, to, LitEnd::nowhere(tile), true, FLAME_SPREAD, &occlusion).0;
            prop_assert!(
                (exact - streaming).abs() < 1e-3,
                "from {from:?} to {to:?}: walk_cells_exact {exact} vs walk_cells_streaming {streaming}",
            );
        });
    }

    /// `docs/lighting_raymarch.md`'s point 4, over a small room rather than
    /// one isolated wall — three walled sides, a doorway gap, and a
    /// free-standing body in the open area, seven solids on six different
    /// tiles at once. [`walk_cells_streaming`]'s own doc comment names this
    /// as the densest of the constructions that went looking for a case
    /// where plain single-axis DDA (no diagonal probe) misses a cell a real
    /// ray passes through, and did not find one — this is that construction,
    /// kept as a permanent regression rather than only run once by hand.
    #[test]
    fn walk_cells_streaming_agrees_with_walk_cells_exact_in_a_small_room() {
        use crate::facing::Facing;
        use crate::occlusion::{Builder, Shape};
        use proptest::prelude::*;

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
            let tile = (fx.floor() as i32, fy.floor() as i32);
            let from = [fx, fy, fz];
            let to = [tx, ty, tz];
            let exact = walk_cells_exact(from, to, LitEnd::nowhere(tile), true, FLAME_SPREAD, &occlusion).0;
            let streaming = walk_cells_streaming(from, to, LitEnd::nowhere(tile), true, FLAME_SPREAD, &occlusion).0;
            prop_assert!(
                (exact - streaming).abs() < 1e-3,
                "from {from:?} to {to:?}: walk_cells_exact {exact} vs walk_cells_streaming {streaming}",
            );
        });
    }

    /// `docs/lighting_raymarch.md`'s point 4, over the three-tread climbable
    /// stair — and a **real, new-found boundary of the reconstruction**, not
    /// a smoke test alone.
    ///
    /// **Full agreement with [`walk_cells_exact`] does not hold here, and it
    /// should not — this is a second, independent source of the same gap
    /// session 14 already named, not a new one to chase.** A tread's top
    /// and riser are built by [`crate::occlusion::Solid::tread_top_box_of`]/
    /// [`crate::occlusion::Solid::tread_riser_box_of`] (`Prism::footprint`),
    /// not by [`crate::occlusion::Solid::box_of`] — they are sub-tile strips
    /// along the climb axis. A tread's `edges` is `0`, the same as an
    /// ordinary floor's, so [`walk_cells_streaming`]'s `box_of(tile, 0,
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
    /// itself informative, not a bug to chase.** `walk_cells_exact`'s own
    /// `stopped_by` names the *first* tile in ray order that fully blocked
    /// it; when it found nothing blocking at all (`through == 1.0`,
    /// `stopped_by == None`) there is no blamed tile to fall back to, and
    /// the tile a disagreement actually traces to can be anywhere a tread or
    /// riser's real, precise footprint the ray legitimately misses gets
    /// read by `walk_cells_streaming` as the *whole* tile instead. Building
    /// a sound oracle for that needs the same care session 11's own
    /// multi-solid disagreement oracle took for `exemption` — a real next
    /// step, not attempted here. So this checks what
    /// `walk_cells_exact_stays_in_range_on_the_stair` already checks for the
    /// exact walk itself: never panics, never returns a `through` outside
    /// `0.0..=1.0`, over the same broad fuzz — the lid-transparency and
    /// off-axis-probe-omission bugs either could have had would show up here
    /// as an out-of-range value or a panic, even without a numeric oracle to
    /// compare against.
    #[test]
    fn walk_cells_streaming_stays_in_range_on_the_stair() {
        use crate::facing::Prism;
        use crate::occlusion::{Builder, Shape};
        use proptest::prelude::*;

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
            let tile = (fx.floor() as i32, fy.floor() as i32);
            let from = [fx, fy, fz];
            let to = [tx, ty, tz];
            let streaming = walk_cells_streaming(from, to, LitEnd::nowhere(tile), true, FLAME_SPREAD, &occlusion).0;
            prop_assert!((0.0..=1.0).contains(&streaming), "from {from:?} to {to:?}: through {streaming}");
        });
    }

    /// [`Spot::tile`]'s own contract, checked at the layer that used to get it
    /// wrong: a `from` sitting exactly on its tile's own far edge, in the
    /// direction of travel, must leave that tile at `t` near zero — not carry
    /// a whole tile of slack from a `from.floor()` that could just as well
    /// have picked the near side. `docs/lighting_raymarch.md` step 2's own
    /// "grew by one line" note is this exact seed, `boundary[axis]`, at the
    /// layer it lives at.
    #[test]
    fn a_from_on_its_own_tiles_far_edge_leaves_it_almost_immediately() {
        let tile = (5, 5);
        // `x == 6.0` is tile 5's own far edge (`5..6`), and the ray keeps
        // moving in `+x`, away from tile 5 and never back into it.
        let steps = dda_walk(Vec2::new(6.0, 5.5), Vec2::new(9.0, 5.5), tile);
        assert_eq!(steps[0].cell, tile);
        assert!(
            steps[0].leaves < 1e-3,
            "a from already on tile 5's exit edge should leave it almost at \
             once, not after a whole tile of slack: leaves = {}",
            steps[0].leaves,
        );
        assert_eq!(steps[1].cell, (6, 5));
    }

    /// Everything [`dda_walk`] promises about its own output, checked as
    /// plain numbers over arbitrary rays — no scene, no `Occlusion`, no GPU.
    /// This is the fast net the testability audit in
    /// `docs/lighting_raymarch.md` argues for: the DDA is the piece every
    /// bug in that doc actually lived in, and it is the one piece that was,
    /// until now, only reachable through a rendered or CPU-sampled scene.
    #[test]
    fn dda_walk_visits_a_connected_path_of_cells_starting_at_the_callers_tile() {
        use proptest::prelude::*;

        proptest!(ProptestConfig::with_cases(1024), |(
            tile_x in -20_i32..20,
            tile_y in -20_i32..20,
            frac_x in 0.0_f32..1.0,
            frac_y in 0.0_f32..1.0,
            delta_x in -8.0_f32..8.0,
            delta_y in -8.0_f32..8.0,
        )| {
            // The same `ground < 1e-6` floor callers guard the DDA with —
            // `dda_walk` has no direction to step in below it.
            prop_assume!(delta_x.abs() > 1e-3 || delta_y.abs() > 1e-3);

            let tile = (tile_x, tile_y);
            let from = Vec2::new(tile_x as f32 + frac_x, tile_y as f32 + frac_y);
            let to = Vec2::new(from.x + delta_x, from.y + delta_y);
            let steps = dda_walk(from, to, tile);

            prop_assert!(!steps.is_empty());
            prop_assert_eq!(steps[0].cell, tile);
            prop_assert!(steps.len() <= MAX_WALK_STEPS as usize);

            // Every cell but a genuinely final one has somewhere it goes
            // next, and a final one is exactly the one with no exit side —
            // the two facts a caller leans on to know when to stop reading.
            for step in &steps {
                prop_assert_eq!(step.exit == 0, !step.continues);
            }
            for step in &steps[..steps.len() - 1] {
                prop_assert!(step.continues);
            }

            // Consecutive cells are von-Neumann neighbours: single-axis
            // stepping, one tile at a time, one axis a step — no diagonal
            // jump and nothing this walk does ever skips a cell or stands
            // still.
            for pair in steps.windows(2) {
                let (a, b) = (pair[0].cell, pair[1].cell);
                let (dx, dy) = ((b.0 - a.0).abs(), (b.1 - a.1).abs());
                prop_assert_eq!(dx + dy, 1, "non-adjacent or diagonal step {:?} -> {:?}", a, b);
            }

            // `entered`/`leaves` walk forward along the segment, never
            // backward and never outside `0.0..=1.0`.
            let mut floor = 0.0_f32;
            for step in &steps {
                prop_assert!((0.0..=1.0).contains(&step.entered));
                prop_assert!((0.0..=1.0).contains(&step.leaves));
                prop_assert!(step.leaves + 1e-6 >= step.entered);
                prop_assert!(step.entered + 1e-6 >= floor);
                floor = step.leaves;
            }

            // An axis the ray does not move along is never crossed — the
            // walk stays in `tile`'s own row or column the whole way.
            if delta_y.abs() < 1e-6 {
                for step in &steps {
                    prop_assert_eq!(step.cell.1, tile.1);
                }
            }
            if delta_x.abs() < 1e-6 {
                for step in &steps {
                    prop_assert_eq!(step.cell.0, tile.0);
                }
            }
        });
    }

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
    /// `docs/lighting_raymarch.md`'s ray-vs-Solid scoping, point 1.
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
                    let claims_inside = t >= entered - 1e-4 && t <= leaves + 1e-4;
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
