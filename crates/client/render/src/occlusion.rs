//! What stands between a flame and the ground it would light.
//!
//! A list of the solids this frame's flames can reach, a list of the references
//! to them, and a grid over the same tiles as the index of those: a cell is
//! `(offset, count)` into the references, a reference names a solid, and a solid
//! is a box saying how much of a ray crossing it survives and between which
//! heights. [`crate::light`] hands all of it to the blit, which walks the cells
//! between a fragment and each flame — see `docs/lighting.md`, decisions 3
//! through 6, decision 30 for why the cell is a list rather than one merged
//! span, and decision 38 for why what a cell holds is a *name*.
//!
//! Nothing appends to an [`Occlusion`]. It is built by a [`Builder`], which is
//! where a tile's occluders are merged, and packed by [`Builder::finish`] — a
//! tile's references have to be contiguous for an `(offset, count)` to name
//! them, and they cannot be while anything can still be added.
//!
//! # Why a tile and not a wall's edge
//!
//! A wall stands on one edge of its tile, and **nothing in `tiledata.mul` says
//! which edge**: that is only in the shape of the sprite. So the occluder here
//! is the whole tile. It costs half a tile of reach at the wall, and it buys a
//! room whose wall tiles are a closed ring by construction — no corner of a
//! house leaks light into the street because two segments failed to meet.
//!
//! # What touches light is what stops an arrow — but not by as much
//!
//! `WINDOW | NO_SHOOT`, and not `BLOCK`. The two are different questions and the
//! reference keeps them apart: ServUO's `Map.LineOfSight` (`Server/Map.cs:3040`)
//! tests a static with `(flags & (TileFlag.Window | TileFlag.NoShoot)) != 0`
//! against the span `t.Z ..= t.Z + CalcHeight`, and impassability never enters
//! it. A barrel and a fence are `BLOCK` and you can see over both; a wall is
//! `NO_SHOOT` and you cannot see through it. Reading `BLOCK` instead would put a
//! shadow behind every crate on the street.
//!
//! Where this parts company with the reference is *how much* each stops. Line of
//! sight is a yes or a no, so a window is a wall in it; light is a fraction, and
//! a window is glass. So the grid carries an opacity byte — [`OPAQUE`] for a
//! wall, [`PANE`] for a window — and the shader multiplies by it either way.
//!
//! # Two sets, and the cut between them is at the end
//!
//! A tile belongs to two of them: **what a ray may cross**, which is a fact
//! about the map, and **what this frame draws**, which is a fact about the tile
//! the player is standing on. [`Builder`] holds the first and [`Builder::finish`]
//! hands back the second, applying the frame's [`Cutaway`] as it packs — see
//! `docs/lighting.md`'s decision 33.
//!
//! What comes out is unchanged: nothing occludes that was not drawn, because a
//! shadow cast by a wall the cutaway took away is a dark band with nothing in the
//! picture making it, which is the worse bug of the two. What changed is *where*
//! that is decided, and it matters for one reason: everything above the cut is
//! the same for every frame standing anywhere, so it can be built once and kept.
//! That is the whole of decision 30.4's cache, and it is why the cut is a filter
//! over a packed list rather than a test at the map walk.
//!
//! # The sky a tile can see
//!
//! The grid answers a second question, and it is the cheapest one it can be
//! asked: *can this tile see straight up*. A tile that cannot does not get the
//! sky's share of the ambient — which is what makes the inside of a house darker
//! than the street outside it with nothing in either. `docs/lighting_world.md`,
//! decisions 1, 2, 3 and 14, and the field is [`Occlusion::sky_at`].
//!
//! Three things about it are not the shadow walk's answers, and each is a
//! decision rather than an accident:
//!
//! - **It ignores the [`Cutaway`].** Standing indoors deletes the roof so that
//!   the player can be seen; if the sky test read the *drawn* statics, walking
//!   through a door would flood the room with noon and the player would carry
//!   daylight into every building. A shadow from a static that is not in the
//!   picture is an artefact; the missing ambient of a roof the player walked
//!   under is the point. So this is the one reader of the walk that does not ask
//!   [`cutaway::shows`].
//! - **It is blurred by a tile.** A raw column test steps from 1 to 0 at the
//!   wall line, and a step is the artefact this whole track exists to remove.
//!   One 3x3 pass over a grid a few hundred tiles across makes the threshold of
//!   an open door brighter than the middle of the room and the eave of a roof
//!   brighter than what is under it. It is not a simulation of anything — it is
//!   the shape the right answer has, for one blur of a small array.
//! - **A pane passes its share.** The column multiplies by what each occluder
//!   leaves, so a glazed roof lets four fifths of the sky through where a slate
//!   one lets none. That is the crude half of decision 14, and it is what keeps
//!   a chapel from reading as a crypt until an aperture arrives.
//!
//! # One plane per answer, beside the cell and not inside it
//!
//! A [`Solid`] is four channels and all four are spoken for, so the sky needed
//! room. It gets a **texture over the grid's rectangle** rather than a
//! wider solid — see [`Occlusion::field_bytes`], whose four channels are the
//! places the answers that are not about *stopping a ray* go: the sky today, an
//! aperture and a body's opacity when `docs/lighting.md`'s step 16 and
//! `docs/lighting_world.md`'s step 8 land. One decision for all three, which is
//! what the plans asked for, and the split is along the line that matters: a
//! solid is what a ray walks through, and this is what a *tile* is, read once
//! per fragment and never in a loop.
//!
//! So a frame uploads four: the index over the camera's rectangle, this field
//! over the same rectangle, and then the two lists — the references and the
//! solids — whose lengths are what the camera happens to be looking at rather
//! than how big it is.

pub mod bake;

use openshard_protocol::wire::Graphic;

use crate::facing::{Face, Facing, Hole};
use openshard_uofiles::map::Map;
use openshard_uofiles::tiledata::{StaticTile, TileData, TileFlags};

use crate::camera::TileBounds;
use crate::cutaway::{self, Cutaway};
use crate::items::GroundItem;

/// A tile that stops light entirely.
pub const OPAQUE: u8 = 255;

/// A tile light crosses untouched.
pub const CLEAR: u8 = 0;

/// Whether a static touches light at all — a wall or a pane, and not a barrel.
///
/// The reference's line of sight test, and it is still the right *membership*
/// question: what is in the grid is what stops an arrow. How much of the light
/// each member stops is [`opacity`]'s answer and no longer the same one. See
/// this module's header for why it is not `BLOCK`.
pub fn stops_light(tile: &StaticTile) -> bool {
    tile.flags.has(TileFlags::WINDOW | TileFlags::NO_SHOOT)
}

/// The four sides of a tile, as bits of a cell's fourth channel — and the
/// difference between an occluder that is a *tile* and one that is a *panel*.
///
/// `docs/lighting.md`'s decision 3 made an occluder a whole tile, because
/// `tiledata.mul` does not say which edge a wall stands on and guessing it from
/// the art was "a subsystem". Step 15 built that subsystem, so this is decision 3
/// revised: where [`crate::facing`] names an edge, the occluder is the panel on
/// that edge and a ray is stopped only where it **crosses** it. A ray running
/// alongside a wall passes, which is what a lamp mounted on a house needs in
/// order to light the street it hangs over.
///
/// A mask of zero is a **lid**: something horizontal, a floor or a roof, whose
/// occlusion is entirely its `z` span and which no vertical edge describes.
/// [`EDGE_ANY`] is "it stands up and nobody knows which way", which is exactly
/// the old whole-tile answer and therefore the safe fallback.
pub const EDGE_NORTH: u8 = 1;
/// The `x1` side. See [`EDGE_NORTH`].
pub const EDGE_EAST: u8 = 2;
/// The `y1` side. See [`EDGE_NORTH`].
pub const EDGE_SOUTH: u8 = 4;
/// The `x0` side. See [`EDGE_NORTH`].
pub const EDGE_WEST: u8 = 8;
/// All four: a thing that stands up whose facing the art would not name.
pub const EDGE_ANY: u8 = EDGE_NORTH | EDGE_EAST | EDGE_SOUTH | EDGE_WEST;
/// How thick a panel is, in tiles: real geometry, inward from the plane its
/// face pixels lie on — the depth [`light::walk_cells`](crate::light::walk_cells)
/// is genuinely stopped by, not only a view's own fattening.
///
/// **Step 23.5, and it withdraws the split [`solid::drawn`](crate::solid::drawn)'s
/// own doc complained about.** Before this, a panel's *record* was the bare
/// plane — step 23.1's "a thickness no ray is tested against may not sit in
/// the field a reader takes for geometry" — and only the *view* fattened it,
/// under its own unrelated name, `solid::DRAWN_PANEL_THICKNESS`. Now one
/// number is the geometry, `Solid::box_of` is the only place it is spent, and
/// the view draws the box exactly as it stands: `solid::drawn` no longer
/// touches a panel at all.
///
/// **What a ray is stopped by has not moved yet.** [`light::pierced`] still
/// samples one point, because the outer face of the slab — the one
/// [`Solid::box_of`] leaves at the tile's own edge — is exactly where the
/// walk's cell-boundary crossing already looks; a panel fattened only
/// *inward* never reaches past where the old flat plane stood. What the real
/// depth buys is at the corner: two panels that meet there now genuinely
/// overlap in a `PANEL_THICKNESS`-wide square instead of touching at a
/// point, and `light::corner_tie` is sized off this number rather than off an
/// arbitrary float tolerance — see it for the argument.
///
/// **0.2, kept from the number the view used to invent alone.** It was
/// already chosen to be *seen* — about nine screen pixels at 1:1 — and
/// nothing here argues for a different one: the art still cannot measure a
/// wall's depth (decision 3), so any number is invented, and this is the one
/// already spent, moved into the geometry it was always standing in for
/// rather than duplicated beside it.
pub const PANEL_THICKNESS: f64 = 0.2;
/// The bit that says a cell holds anything at all.
///
/// Separate from the mask because a lid's mask is legitimately zero, and the
/// shader tests presence before it tests edges. `bytes` writes `PRESENT | mask`
/// and `blit.wgsl` takes the two apart with the same constants.
pub const PRESENT: u8 = 0x80;

/// The bit that says this surface has a hole in it, and therefore that its texel
/// in the aperture plane means something.
///
/// What it buys is that a surface *without* one costs nothing: the walk reads
/// the aperture plane only where this bit is set, so the ordinary wall — which
/// is every wall in the world until step 16 measures a window — pays one bit
/// test and no second fetch. The same shape as every other miss in this pass.
/// See [`Occlusion::aperture_bytes`] and [`Aperture`].
pub const HOLED: u8 = 0x40;

/// The lowest `z` anything on the wire can name, and the highest whole one.
///
/// A map's own `z` is an `i8`, so measuring from [`Z_FLOOR`] makes it fit an
/// unsigned field — but `z + height` is not an `i8`: a 255-tall static based at
/// 100 has a top no field here holds, and a wall that reaches past the top of
/// the world may as well stop there. Named rather than spelled at each place
/// that clamps, so that a span and a hole are pinned to the same ends.
/// `docs/lighting_height.md` phase 2.
///
/// [`Solid::z_bytes`] reaches `Z_CEILING + 255/256` rather than `Z_CEILING`,
/// because it carries sixteen bits an end and not eight; the ceiling named here
/// is the last *whole* unit, which is what the readers that round to one want.
pub const Z_FLOOR: i32 = -128;

/// See [`Z_FLOOR`].
pub const Z_CEILING: i32 = 127;

/// One whole `z` as the byte an [`Aperture`] carries it in: clamped to
/// [`Z_FLOOR`]`..=`[`Z_CEILING`] and offset by 128.
///
/// A hole's own two ends only, since [`Occlusion::solid_z_bytes`] took the
/// span: a hole is measured off the art as whole units, and nothing has asked
/// it for a fraction.
fn z_byte(z: i32) -> u8 {
    (z.clamp(Z_FLOOR, Z_CEILING) + 128) as u8
}

/// The side of the neighbouring tile that touches this one's `side`.
///
/// One line, and it is the whole of how a walk carries an edge across a
/// boundary: the line a ray crosses is one cell's east and the next one's west.
/// `blit.wgsl` has the same function and the parity test is what says so.
pub fn opposite(side: u8) -> u8 {
    match side {
        EDGE_NORTH => EDGE_SOUTH,
        EDGE_SOUTH => EDGE_NORTH,
        EDGE_EAST => EDGE_WEST,
        EDGE_WEST => EDGE_EAST,
        _ => 0,
    }
}

/// The one named edge a compass direction is, as a mask bit.
///
/// Pulled out of [`edges_of`] rather than inlined twice: a tread's riser
/// (`Builder::add`'s climbable branch) names its own edge the same way a
/// facing's does — `opposite` of the climb's `up` — and a second copy of this
/// match would be exactly the kind of drift decision 9 warns about, one file
/// over.
fn edge_of(face: Face) -> u8 {
    match face {
        Face::North => EDGE_NORTH,
        Face::East => EDGE_EAST,
        Face::South => EDGE_SOUTH,
        Face::West => EDGE_WEST,
    }
}

/// Which sides of its tile a static occupies, from what the art said about it.
///
/// `None` — a post, a tree, a graphic no atlas was offered — is [`EDGE_ANY`]:
/// the whole-tile answer, unchanged from before faces existed. A **corner** is
/// two bits, which is the panel path with two panels on it and not a new case —
/// see the `edges` arm of `light::walk_cells` and of `blit.wgsl`'s `walk`. A
/// [`Stance::Flat`](crate::place::Stance) static is not asked at all; see
/// [`Occlusion::add`].
///
/// Two bits and not four is the whole of what decision 25 buys the grid: a ray
/// running *alongside* a corner — down the street the corner stands on — crosses
/// neither of its two panels and passes, exactly as it does beside the runs of
/// wall either side of it, where before it was stopped by a whole-tile occluder.
pub fn edges_of(facing: Option<crate::facing::Facing>) -> u8 {
    let Some(facing) = facing else {
        return EDGE_ANY;
    };
    facing.faces().map(edge_of).fold(0, |mask, side| mask | side)
}

/// What the art said about one graphic's geometry: which edge it stands on, and
/// the hole in it.
///
/// One argument rather than two beside each other, because the two are one
/// answer about one picture and they arrive together: [`crate::facing`] measures
/// the face today and step 16 measures the aperture off the same silhouette, and
/// decision 31.2 makes both of them one row of one table. A caller that has
/// neither passes [`Shape::UNREAD`], which is every built scene and every
/// graphic no atlas held.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Shape {
    /// Which sides of its tile the picture stands on, or `None` for "the art
    /// would not say" — see [`edges_of`].
    pub facing: Option<Facing>,
    /// The hole in it, or `None` for a solid.
    ///
    /// A [`Hole`] and not an [`Aperture`], which is the difference between a
    /// measurement and a placement: the picture is drawn once and stood on a
    /// hundred tiles at a hundred heights, so what the art can say is a rectangle
    /// above the *static's own base*. [`Builder::add`] is where the two meet,
    /// because it is the one place that knows which `z` this instance stands at.
    pub hole: Option<Hole>,
    /// The solid this picture is of, where it is a picture of one.
    ///
    /// A different *kind* of answer from [`Shape::facing`], and the two are not
    /// alternatives by accident: a facing says which edge of the tile a plane
    /// stands on, and a prism says what shape fills the tile. A stair has both
    /// answers — the wall detector reads its base as a corner of two walls,
    /// because a solid's base is pixel for pixel what two walls meeting leave —
    /// and the prism is the true one. See `docs/lighting.md`'s backlog, "found on
    /// a staircase in Britain".
    ///
    /// Which of the two is believed is not decided here: this is what the picture
    /// says, and [`Builder::add`] is where the client's own `CLIMBABLE` bit picks
    /// between them.
    pub prism: Option<crate::facing::Prism>,
    /// A shape [`prism`](Self::prism) cannot describe, authored rather than
    /// derived — an arch's posts and lintel, a gap nothing here states because
    /// it is simply the absence of a block. `docs/lighting.md`'s decision 41 is
    /// the argument for a second, independent kind of solid rather than a wider
    /// `Prism`: a climb profile is monotonic along one axis by construction, and
    /// an arch is not.
    ///
    /// **No detector writes one.** [`Shape::of`] never populates it — there is no
    /// search over it the way [`crate::facing::best_prism`] searches prisms, only
    /// a person placing boxes by eye against a silhouette — so it survives a
    /// re-derivation exactly the way an authored `prism` does, because nothing
    /// but a person's own `author` call ever sets it.
    pub blocks: crate::facing::Blocks,
}

impl Shape {
    /// Nothing was measured: the whole-tile occluder, with no hole in it.
    pub const UNREAD: Self = Self {
        facing: None,
        hole: None,
        prism: None,
        blocks: crate::facing::Blocks::EMPTY,
    };

    /// A graphic whose face the art named and whose hole it did not — which is
    /// every wall in the world but the fifty-eight windows step 16 reads.
    pub fn faced(facing: Facing) -> Self {
        Self {
            facing: Some(facing),
            hole: None,
            prism: None,
            blocks: crate::facing::Blocks::EMPTY,
        }
    }

    /// A graphic the art reads as a solid: a box, or a flight of steps.
    pub fn solid(prism: crate::facing::Prism) -> Self {
        Self {
            facing: None,
            hole: None,
            prism: Some(prism),
            blocks: crate::facing::Blocks::EMPTY,
        }
    }

    /// A graphic a person authored as a list of blocks — an arch, a joint,
    /// anything a single climb profile cannot describe. See [`Shape::blocks`].
    pub fn pieced(blocks: crate::facing::Blocks) -> Self {
        Self {
            facing: None,
            hole: None,
            prism: None,
            blocks,
        }
    }

    /// Everything one picture says about its own geometry, measured off it.
    ///
    /// The two detectors in the order they depend on each other: a hole is a
    /// rectangle *in a face*, so a picture [`facing_of`](crate::facing::facing_of)
    /// would not name is never offered to
    /// [`aperture_of`](crate::facing::aperture_of).
    ///
    /// One function because the two callers must not drift: the tool that writes
    /// the table (`openshard-client-artscan`) and the atlas packing a sprite on a
    /// machine that has no table. Two routes to one answer is how a table and a
    /// client come to disagree about a picture — and the disagreement would look
    /// like a window that exists only where somebody ran a tool.
    ///
    /// It is the expensive one. `docs/lighting.md`'s decision 31 is that it
    /// belongs off the clock; the atlas calls it only where there is no table to
    /// read instead.
    pub fn of(image: &openshard_uofiles::image::Image) -> Self {
        let facing = crate::facing::facing_of(image);
        Self {
            facing,
            hole: facing.and_then(|facing| crate::facing::aperture_of(image, facing)),
            // **Only offered to a picture the wall detector called a corner.**
            // Not a shortcut for the cost, though it saves nearly all of it: a
            // solid's base *is* two 45° runs meeting at the tile's south corner,
            // so every prism the client draws reads as a corner first. A picture
            // that reads as one plain face is a wall standing on one edge, and
            // scoring prisms against it would be asking whether a wall is a box —
            // a question whose best answer is 0.81 and therefore a question worth
            // not asking.
            prism: match facing {
                Some(Facing::Corner { .. }) => crate::facing::prism_of(image),
                _ => None,
            },
            blocks: crate::facing::Blocks::EMPTY,
        }
    }
}

/// How much of a ray crossing a pane of glass is stopped.
///
/// A fifth, which is a guess about glass and not a number from any file — the
/// client has none. What it is *not* is a guess about line of sight: an arrow is
/// stopped by a window and light is not, and `WINDOW` being in the same test as
/// `NO_SHOOT` in the reference is a fact about arrows. A window that stopped
/// light entirely is what makes a lit room read as a bunker, and it is the one
/// thing standing between a candle and the street it should be visible from.
pub const PANE: u8 = 51;

/// How much of a ray crossing this static is stopped, `0..=255`.
///
/// Three answers and not two: a wall stops everything, a pane dims, and
/// everything else — a barrel, a fence, a crate — passes light untouched even
/// where it stops an arrow. The byte was always here for this; what changed is
/// that `WINDOW` no longer borrows `NO_SHOOT`'s answer.
///
/// `NO_SHOOT` wins where a tile carries both. A shard's custom static that is
/// flagged as a solid window is more likely to be a shuttered one than a
/// transparent wall, and the union is the conservative direction — darkening is
/// visible, leaking a room into the street is a bug.
///
/// # Why the graphic, when the flags are right here
///
/// Because an **open door** has the flags of a shut one. `tiledata.mul` gives a
/// door's two leaves identical entries — measured over all 104 of ServUO's
/// open/shut pairs — so a door left to its flags lays a whole tile of wall
/// across its own doorway, which decision 3 makes the coarsest possible wrong
/// answer. [`crate::doors`] is the table that knows, and this is where it is
/// asked, before anything else: a leaf that has swung open stops nothing.
///
/// Which is the general shape and not a door-shaped patch. A flag is a fact
/// about a *picture*, and anything that opens, lifts or breaks is a fact about
/// the *thing*: a shutter, a portcullis, a drawbridge are all this question
/// again. So the argument is the graphic, and the flags are what it falls back
/// on.
pub fn opacity(graphic: Graphic, tile: &StaticTile) -> u8 {
    if crate::doors::is_open(graphic) {
        return CLEAR;
    }
    if tile.flags.has(TileFlags::NO_SHOOT) {
        return OPAQUE;
    }
    match tile.flags.has(TileFlags::WINDOW) {
        true => PANE,
        false => CLEAR,
    }
}

/// How tall a static stands, for the purpose of what it hides.
///
/// ServUO's `ItemData.CalcHeight` (`Server/TileData.cs:112`): a climbable
/// (`Bridge`) tile counts as half its stated height, because that is the height
/// you end up standing at on it. `movement`'s `platform_surface` halves the same
/// number for the same reason.
fn calc_height(tile: &StaticTile) -> i32 {
    let height = i32::from(tile.height);
    match tile.flags.is_climbable() {
        true => height / 2,
        false => height,
    }
}

/// How many parts of a tile one step of the run coordinate is worth.
///
/// A hole's span along its surface is a byte, so a two-hundred-and-fifty-fifth
/// of a tile — 0.17 pixels of world at the projection's 44, which is finer than
/// the seven bits the place attachment carries a *pixel's* own fraction in. The
/// quantisation is deliberate and not a shortcut: it is what makes the two
/// implementations of the walk agree exactly rather than to a tolerance, because
/// both read the same byte and divide it by the same number.
pub const RUN_STEPS: f32 = 255.0;

/// A rectangular hole in a surface, in that surface's own coordinates.
///
/// `docs/lighting.md`'s decision 30.2 and step 21.3: a window is a hole *in* a
/// wall, so a real one is a rectangle in the plane of a panel and not a dimmer
/// tile. What a ray crossing the panel inside this rectangle meets is nothing.
///
/// Two coordinates, and they are the surface's rather than the world's:
///
/// - `near` and `far` run **along** the panel, from the low corner of the axis
///   it runs along to the high one — `x` for a north or south face, `y` for an
///   east or west one — in [`RUN_STEPS`]ths of a tile. A hole from a quarter to
///   three quarters of the way along is `(64, 191)`.
/// - `bottom` and `top` are `z`, in the map's own units and absolute, exactly as
///   [`Surface`]'s are. Not relative to the surface's own base, because every
///   other height in this pass is absolute and one relative number in the middle
///   of them is a conversion somebody will get the sign of wrong.
///
/// **Only a named panel may have one**, and that is a refusal rather than an
/// omission: a lid is horizontal and a body is "it stands up and the art would
/// not say which way", so neither has a plane for a rectangle to be stated in.
/// [`Builder::add`] drops an aperture offered for either.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Aperture {
    /// Where the hole starts along the run, in [`RUN_STEPS`]ths of a tile.
    pub near: u8,
    /// And where it ends. A `far` at or below `near` is a hole of no width,
    /// which stops nothing from being stopped — the same degenerate case a `z`
    /// span of zero already is.
    pub far: u8,
    /// The lowest `z` the hole reaches.
    pub bottom: i32,
    /// And the highest.
    pub top: i32,
}

impl Aperture {
    /// A hole stated as fractions of the tile's run, quantised once here.
    ///
    /// The one place a fraction becomes a byte, so that nothing downstream has
    /// two numbers for one edge: a caller says `0.25`, the grid stores `64`, and
    /// both walks read `64 / 255`.
    pub fn new(near: f32, far: f32, bottom: i32, top: i32) -> Self {
        let step = |v: f32| (v.clamp(0.0, 1.0) * RUN_STEPS).round() as u8;
        Self {
            near: step(near),
            far: step(far),
            bottom,
            top,
        }
    }

    /// The hole the art measured off a picture, placed on the static that is
    /// standing at `base`.
    ///
    /// **The one conversion between the two**, and it is where it is because
    /// `base` is a fact about an *instance*: the same window graphic stands on
    /// the ground floor and on the storey above it, and a measurement that had
    /// been made absolute anywhere earlier would have had to pick one of them.
    /// See [`Hole`].
    pub fn above(base: i32, hole: Hole) -> Self {
        Self {
            near: hole.near,
            far: hole.far,
            bottom: base + i32::from(hole.bottom),
            top: base + i32::from(hole.top),
        }
    }
}

/// One solid the world holds: the box it occupies, and how much of a ray
/// crossing it survives.
///
/// The thing a cell *references* — `docs/lighting.md`'s decision 38, step 23.1 —
/// and **the walk's two rules are still its two kinds**: [`Solid::edges`] naming
/// one side is a *panel*, a ray is stopped where it crosses it; zero is a *lid*
/// and all four a *body*, and a ray is stopped by how far it ran inside the span.
///
/// # The box is the record, and the kind is carried beside it
///
/// [`Solid::space`] is where it stands, in the world's own coordinates, and it is
/// the only geometry: what a view draws and what the walk is tested against come
/// from one record, which is the whole of what step 23.1 buys. What it is *not*
/// yet is a slab — the box of a panel and of a lid is the plane the walk crosses,
/// flat on the axis it has no thickness on, and that is deliberate. A nominal
/// thickness stored here would be a number the walk does not read sitting in the
/// field a reader would take for geometry; the thickness a *drawing* wants is the
/// view's, and [`crate::solid::drawn`] is where it lives.
///
/// [`Solid::edges`] is the kind, carried rather than derived, and that was step
/// 23.1's one deliberate choice. Deriving it from the box — flat in `z` is a lid,
/// flat in `x` or `y` is a panel — reads well and is wrong on a case the map is
/// full of: a static whose `tiledata` height is zero is a **body** with a
/// degenerate span, and it is flat in `z` exactly as a floor is. It would become
/// a lid, silently, and a lid is travelled through by a different rule. The kind
/// goes when step 23.5 replaces the rules that ask it, not before.
///
/// The span is in `z` units — the map's own, not pixels — and it is inclusive of
/// [`Solid::bottom`] and [`Solid::top`]. A wall based at `z = 0` and 20 tall
/// stops a ray passing through `0..=20` and no other, which is what keeps a
/// cellar's wall out of the street and an upper storey's out of the ground floor.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Solid {
    /// Where it stands, in world coordinates: `min` the low corner on all three
    /// axes and `max` the high one.
    ///
    /// Flat on the axis the thing has no extent on — a panel's box is its plane,
    /// a lid's is the height it lies at, a body's is its whole tile. See the type
    /// doc for why nothing nominal is stored here.
    pub space: crate::solid::Solid,
    /// How much of a ray crossing it is stopped.
    pub opacity: u8,
    /// Which side of the tile it stands on: one of [`EDGE_NORTH`],
    /// [`EDGE_EAST`], [`EDGE_SOUTH`], [`EDGE_WEST`] for a panel, `0` for a lid,
    /// or [`EDGE_ANY`] for a body. Never two named sides — a corner is two
    /// panels, which is what the list is for.
    pub edges: u8,
    /// The hole in it, where the art named one — see [`Aperture`], and step 21.3
    /// of `docs/lighting.md`. Indexed by the solid's own place in
    /// [`Occlusion::solid`], which is what the aperture plane is folded to.
    ///
    /// [`Option`] in the sense the style asks for: a surface with no hole is a
    /// solid wall, which is a fact and not a missing measurement. A surface the
    /// detector has not been *offered* is the same `None`, and that is the safe
    /// direction — a wall with no window in it is what every wall in the world
    /// is today.
    pub aperture: Option<Aperture>,
    /// Whether the static this came from is a roof, which is the one thing about
    /// it no rule of the walk asks and [`Builder::finish`] cannot do without.
    ///
    /// A [`Cutaway`] cuts on two facts — a height, which is [`Surface::bottom`],
    /// and roof-ness, which nothing else here needed — and the whole of decision
    /// 33 is that the cut happens when a *frame* is packed rather than when a
    /// surface is built. So the surface has to carry it that far. See
    /// [`Builder::finish`].
    pub roof: bool,
    /// The thing this is a solid **of** — see [`Owner`].
    ///
    /// Every solid one [`Builder::add`] pushes carries the same one, which is
    /// what `docs/lighting_height.md` phase 3 replaces `on_surface`'s guess with:
    /// "is this solid the one the fragment is a point of" stops being a question
    /// about height and becomes a question a fragment and a solid each answer
    /// from the same field.
    ///
    /// Never uploaded. What crosses the wire is [`OwnerId`], the number
    /// [`Builder::finish`] gives this key within its cell.
    pub owner: Owner,
}

impl Solid {
    /// The lowest `z` this solid stops anything at, **rounded to a whole unit**.
    ///
    /// Off the box, because the box is the record. It was integral by
    /// construction in step 23.1 — every span was a static's `z` and its
    /// `tiledata` height, both whole numbers — and the rounding was a formality
    /// spelled anyway against the day something authored a fraction. That day
    /// arrived: [`Builder::add_raw`]'s arbitrary AABB, a mesh face, a slope, a
    /// tread. So this is now a **lossy** view of the record, and the readers it
    /// is left for are the two that genuinely want a whole `z`:
    ///
    /// - the cutaway ([`Solid::drawn`]), which cuts on storeys,
    /// - and the upload's byte ([`Occlusion::solid_bytes`]), where the fraction
    ///   it drops is carried alongside by [`Occlusion::solid_z_bytes`] and put
    ///   back by [`Solid::span_from_bytes`],
    ///
    /// plus [`Occlusion::at`]'s merged view, which is a picture of a tile rather
    /// than a step of a walk. **The walk reads [`Solid::low`]/[`Solid::high`]**,
    /// which is `docs/lighting_height.md` phase 2: a shadow decided from a
    /// rounded occluder is a shadow half a unit out of place, and on a face it
    /// is the difference between a fragment being inside its own solid and
    /// under it.
    pub fn bottom(&self) -> i32 {
        self.space.min.z.round() as i32
    }

    /// And the highest. See [`Solid::bottom`].
    pub fn top(&self) -> i32 {
        self.space.max.z.round() as i32
    }

    /// The lowest `z` this solid stops anything at, exactly: the record's own
    /// corner, in the `f32` every walk works in.
    ///
    /// What [`Solid::bottom`] rounds. Named for what the walks already call it —
    /// `let (low, high) = ...` — and not `bottom_exact`, because the rounded one
    /// is the exception here now, not this.
    pub fn low(&self) -> f32 {
        self.space.min.z as f32
    }

    /// And the highest. See [`Solid::low`].
    pub fn high(&self) -> f32 {
        self.space.max.z as f32
    }

    /// Whether the frame this grid is being packed for draws the static this
    /// solid came from — [`Cutaway::shows_at`], asked about what a solid
    /// kept of it.
    ///
    /// The other half of [`cutaway::shows`] — the draw ceiling — is not asked
    /// here and never will be: it is a fact about the static and the map, so it
    /// is settled where the surface is built, and what is left for a frame to
    /// decide is exactly the cutaway. See [`Builder::finish`].
    fn drawn(&self, cutaway: &Cutaway) -> bool {
        cutaway.shows_at(self.bottom(), self.roof)
    }

    /// Whether a view of the grid drawn for somebody standing at `floor` should
    /// show this solid.
    ///
    /// The first frame of the wireframe overlay was a dock, and it was **2,011
    /// boxes**: a deck plank stops an arrow — a floor is what you cannot shoot
    /// *through* to the storey above — so every tile of the pier is a thin slab
    /// in the grid, and the picture was a red hatch over the whole ground with
    /// the walls somewhere inside it. Nothing about it was wrong and nothing in
    /// it was readable.
    ///
    /// So a view draws what is above the floor the *player* stands on. It is a
    /// datum and not a threshold: the deck underfoot has its top at exactly the
    /// height the body stands at and drops out, a wall beside it is twenty units
    /// tall and stays, the floor of the storey above stays because it is a lid,
    /// and the cellar below drops. Nothing is invented and nothing is tuned.
    ///
    /// What it hides, a view has to count and say — a picture that silently
    /// drops most of a grid reads as a grid with nothing in it, which is the one
    /// failure an instrument may not have. See the backlog entry in
    /// `docs/lighting.md` about a floor that is cut and a hole in a floor
    /// looking identical.
    pub fn stands(&self, floor: i8) -> bool {
        self.space.max.z > f64::from(floor)
    }

    /// The box one occluder standing on tile `(x, y)` is, for a span of `z` and a
    /// kind.
    ///
    /// **The one place a kind becomes geometry**, and it is here rather than at
    /// each of [`Builder::add`]'s four call sites because the four would be four
    /// chances to put a panel on the wrong edge — which decision 39.8's test
    /// caught once already, in the view, where it read as a defect in the map.
    ///
    /// A lid comes out *flat*: its box is the plane the walk crosses. A panel is
    /// a real slab, [`PANEL_THICKNESS`] deep, fattened inward from the plane its
    /// face pixels lie on — see that constant for why the record carries a
    /// number rather than staying flat, the way a lid still does.
    ///
    /// `pub(crate)` since `docs/lighting_raymarch.md`'s point 4:
    /// `light::walk_cells_streaming` reconstructs a solid's box from exactly
    /// `(tile, edges, bottom, top)` rather than reading `Solid::space`
    /// directly, because that is all `blit.wgsl`'s upload format will ever
    /// carry for an ordinary static (no `x`/`y` channel — session 14's
    /// "second bigger idea"). Reusing this rather than re-deriving the same
    /// geometry a second time is the point: for every *ordinary* static this
    /// is bit-for-bit what built the real `space` in the first place, so the
    /// reconstruction is only lossy for `Builder::add_raw`'s sub-tile boxes,
    /// which is the one gap this doc already has a name for.
    pub(crate) fn box_of(x: i32, y: i32, bottom: i32, top: i32, edges: u8) -> crate::solid::Solid {
        use crate::camera::WorldSpot;

        let (x, y) = (f64::from(x), f64::from(y));
        let (mut min, mut max) = (
            WorldSpot {
                x,
                y,
                z: f64::from(bottom),
            },
            WorldSpot {
                x: x + 1.0,
                y: y + 1.0,
                z: f64::from(top),
            },
        );
        match edges {
            // A lid — a floor, a roof, a plank. The whole tile across, at the
            // height it lies at: the walk crosses its top and nothing else, so
            // that is the whole of the box. `bottom` and `top` are both kept
            // because a static with a height is a lid whose span really is deep
            // — a plank is not, a sloped roof section is.
            0 | EDGE_ANY => {}
            // A panel: a slab standing on the named edge, `PANEL_THICKNESS` deep
            // into the tile it stands on and never past it — two walls on the
            // shared edge of neighbouring tiles must not draw one inside the
            // other, which is the same argument `solid::drawn` used to make
            // alone. The outer face stays exactly on the plane `Face::place_at`
            // draws a face pixel on; only the inner one moves.
            EDGE_NORTH => max.y = y + PANEL_THICKNESS,
            EDGE_SOUTH => min.y = y + 1.0 - PANEL_THICKNESS,
            EDGE_WEST => max.x = x + PANEL_THICKNESS,
            EDGE_EAST => min.x = x + 1.0 - PANEL_THICKNESS,
            // A corner is two panels and [`Builder::add`] pushes them one at a
            // time, so more than one named side never reaches here. Whatever it
            // is, it stands up and it is not measured: the whole tile, which is
            // what a body is and what every fallback in this module falls back
            // to.
            _ => {}
        }
        crate::solid::Solid { min, max }
    }

    /// This solid's own horizontal footprint, as a fraction of the tile it
    /// stands on — `(min.x, max.x, min.y, max.y)`, each quantised to a byte.
    ///
    /// Not [`Solid::footprint`] — that answers *which whole tiles* a solid's
    /// plane spills into (decision 38.2's bake-time question); this answers
    /// *where inside one tile* its box actually sits, which is a different
    /// question at a different grain, needed by a different caller.
    ///
    /// What [`Occlusion::footprint_bytes`] uploads and
    /// [`Solid::box_from_footprint`] reconstructs from, and the reason it is
    /// quantised **here**, on the CPU, rather than read as `f64` only at
    /// upload time: `light::walk_cells_streaming` calls this too, and it
    /// exists to preview exactly what the GPU can do — see its own doc
    /// comment. A CPU that read `space` at full precision while the GPU read
    /// a lossy byte would silently stop being that preview and reopen a
    /// parity gap decision 9's suite would have to grow a new tolerance for.
    ///
    /// A solid is always one tile's (`Builder::finish`'s own doc: "nothing
    /// the builder holds is referenced twice"), but **which** tile is not
    /// always `space.min`'s own floor, and reading it as one put a whole class
    /// of occluder on the wrong side of its cell.
    ///
    /// A box with extent on an axis is unambiguous: its `min` is inside its own
    /// tile, so flooring names that tile. A box that is a *plane* on that axis
    /// is not — a plane at a whole coordinate is the boundary **between** two
    /// tiles, and `floor` always picks the far one. That is exactly what a
    /// climbable's first riser is: `Solid::tread_riser_box_of`'s plane for the
    /// tread at the low end of the climb sits on its tile's own far edge, so a
    /// north-climbing flight on `(100, 100)` has a riser at `y == 101.0` —
    /// floored to tile `101`, measured as fraction `0`, and rebuilt by
    /// [`Solid::box_from_footprint`] at `y == 100.0`. **A tile's width away, on
    /// the opposite side of its own cell**, which is where
    /// `light::walk_cells_streaming` and the shader both looked for it. The
    /// front face of every bottom step shadowed nothing, and nothing said so:
    /// `light::walk_cells_exact` reads `space` and was right all along, so the
    /// two walks disagreed on a scene no parity test covers — the agreement
    /// proptests build panels with [`box_of`](Solid::box_of), whose slab is
    /// [`PANEL_THICKNESS`] deep and therefore never degenerate.
    ///
    /// [`Solid::footprint`] already had to decide this and states the rule:
    /// a degenerate axis at a whole coordinate belongs to the tile *below* it
    /// when the solid's own `edges` name that axis's high side. One concept,
    /// and now one spelling of it in both places rather than a fix in one.
    pub fn fraction(&self) -> [u8; 4] {
        // Which tile an axis's span is measured from — `Solid::footprint`'s own
        // `far` rule, and see this function's doc for the class it closes.
        let tile = |min: f64, max: f64, far: bool| match max > min {
            false if far && min.fract() == 0.0 => min.floor() - 1.0,
            _ => min.floor(),
        };
        let tile_x = tile(self.space.min.x, self.space.max.x, self.edges & EDGE_EAST != 0);
        let tile_y = tile(self.space.min.y, self.space.max.y, self.edges & EDGE_SOUTH != 0);
        let frac = |value: f64, tile: f64| (((value - tile).clamp(0.0, 1.0)) * 255.0).round() as u8;
        [
            frac(self.space.min.x, tile_x),
            frac(self.space.max.x, tile_x),
            frac(self.space.min.y, tile_y),
            frac(self.space.max.y, tile_y),
        ]
    }

    /// Steps of a `z` unit [`Solid::z_bytes`] carries one in — `blit.wesl`'s
    /// `SOLID_Z_STEPS`, and the two are one number.
    ///
    /// A power of two, so that a whole `z` — which is what every static off
    /// `tiledata` stands at, and the overwhelmingly common answer — lands on a
    /// step exactly and comes back untouched, and so does every half, quarter
    /// and eighth a hand-authored tread is likely to be at.
    pub const Z_STEPS: f64 = 256.0;

    /// This solid's `z` span as the four bytes [`Occlusion::solid_z_bytes`]
    /// uploads: **each end whole**, sixteen bits of it, in steps of
    /// `1/`[`Solid::Z_STEPS`] and offset by [`Z_FLOOR`] so an unsigned pair of
    /// bytes can carry it — low byte first, `bottom` in the first two channels
    /// and `top` in the second two, `Occlusion::id_bytes`' own byte order.
    ///
    /// **One number an end, not a whole unit and a fraction beside it.** The
    /// first cut of `docs/lighting_height.md` phase 2 split it — the rounded
    /// unit stayed in [`Occlusion::solid_bytes`] and only a signed remainder
    /// came here — to keep that channel meaning what it had always meant for
    /// readers not yet taught about this plane. There were none: after the
    /// phase, `blit.wesl` reads a solid's height through `span_of` and nowhere
    /// else, so the compatibility being preserved had nothing to be compatible
    /// with, and what it cost was a second concept (a fraction *of* something)
    /// and a second clamp for every reader of either to get right.
    ///
    /// Sixteen bits over `Z_FLOOR..` puts the step at a two-hundred-and-fifty-
    /// sixth of a `z` unit — a `z` unit is four screen pixels at zoom 1, so a
    /// step is a sixty-fourth of a pixel — and the range at exactly the
    /// `-128 ..= 127` a map's own `z` lives in, with the top end reaching
    /// `127 + 255/256`. A span past either end is pinned there, which is the
    /// direction an occluder may be wrong in: it stops at least what it really
    /// stops.
    pub fn z_bytes(&self) -> [u8; 4] {
        let raw = |z: f64| {
            let steps = ((z - f64::from(Z_FLOOR)) * Self::Z_STEPS).round();
            steps.clamp(0.0, f64::from(u16::MAX)) as u16
        };
        let (low, high) = (raw(self.space.min.z), raw(self.space.max.z));
        [
            (low & 0xFF) as u8,
            (low >> 8) as u8,
            (high & 0xFF) as u8,
            (high >> 8) as u8,
        ]
    }

    /// The span those bytes say, read back — the Rust twin of `blit.wesl`'s
    /// `span_of`, and the only way anything on either side turns the wire into a
    /// height.
    ///
    /// The result is [`Solid::low`]/[`Solid::high`] to within half a step of
    /// [`Solid::Z_STEPS`], which is the whole of what the wire loses.
    pub fn span_from_bytes(bytes: [u8; 4]) -> (f32, f32) {
        let z = |lo: u8, hi: u8| {
            (f64::from(u16::from(lo) | (u16::from(hi) << 8)) / Self::Z_STEPS + f64::from(Z_FLOOR)) as f32
        };
        (z(bytes[0], bytes[1]), z(bytes[2], bytes[3]))
    }

    /// The box [`Solid::box_of`] would have built if it could read a real
    /// footprint instead of guessing whole-tile-or-panel-strip from `edges`
    /// alone — `docs/lighting_raymarch.md`'s "second bigger idea" landed.
    ///
    /// `footprint` is [`Solid::fraction`]'s own four bytes: for an ordinary
    /// `tiledata`-sourced static this reconstructs the exact same box
    /// `box_of` would (a lid or a body is `(0, 255, 0, 255)`, a panel is
    /// already narrowed by [`PANEL_THICKNESS`] before it is quantised), and
    /// for a [`Builder::add_raw`] sub-tile occluder or a climbable static's
    /// own tread/riser it reconstructs the real footprint neither `box_of`
    /// nor the old four-byte upload could tell apart from a whole tile.
    ///
    /// `low`/`high` are the `z` span the same way — [`Solid::span_from_bytes`]'s
    /// own answer, not [`Solid::bottom`]/[`Solid::top`], since
    /// `docs/lighting_height.md` phase 2: the caller that wants this box is
    /// previewing the GPU, and the GPU has the fraction now.
    pub fn box_from_footprint(
        x: i32,
        y: i32,
        low: f32,
        high: f32,
        footprint: [u8; 4],
    ) -> crate::solid::Solid {
        use crate::camera::WorldSpot;
        let frac = |byte: u8| f64::from(byte) / 255.0;
        crate::solid::Solid {
            min: WorldSpot {
                x: f64::from(x) + frac(footprint[0]),
                y: f64::from(y) + frac(footprint[2]),
                z: f64::from(low),
            },
            max: WorldSpot {
                x: f64::from(x) + frac(footprint[1]),
                y: f64::from(y) + frac(footprint[3]),
                z: f64::from(high),
            },
        }
    }

    /// A tread's **top**: the strip it covers along the climb, flattened to the
    /// plane at its own height — a lid, not a body. This tread's rise is what
    /// [`Solid::tread_riser_box_of`] now covers; folding both into one box the
    /// way step 23.5 did is the whole-body shape step 4b of `docs/gbuffer.md`
    /// replaces, so this box is thin on purpose, not a simplification of it.
    ///
    /// `index`/`count` name which strip the same way
    /// [`Prism::height_at`](crate::facing::Prism::height_at) samples a point in
    /// it — see [`Prism::footprint`](crate::facing::Prism::footprint), moved
    /// there in `gbuffer.md` step 4c so the render side answers the same
    /// question of a tread's strip with the same arithmetic, not a second one.
    fn tread_top_box_of(
        x: i32,
        y: i32,
        top_z: i32,
        up: Face,
        index: usize,
        count: usize,
    ) -> crate::solid::Solid {
        use crate::camera::WorldSpot;
        use crate::facing::Prism;

        let (x, y) = (f64::from(x), f64::from(y));
        let lo = index as f64 / count as f64;
        let hi = (index + 1) as f64 / count as f64;
        let (min_x, max_x, min_y, max_y) = Prism::footprint(x, y, up, lo, hi);
        let z = f64::from(top_z);
        crate::solid::Solid {
            min: WorldSpot {
                x: min_x,
                y: min_y,
                z,
            },
            max: WorldSpot {
                x: max_x,
                y: max_y,
                z,
            },
        }
    }

    /// The **riser** between one tread and the one before it — or the ground,
    /// for the first — the plane at the boundary between their two strips,
    /// facing away from `up`: the side a climber sees approaching from below.
    /// Spans the rise from the previous tread's height (`low_z`) to this one's
    /// (`high_z`); [`Builder::add`] passes the static's own base for the first
    /// tread's `low_z`, since there is no tread before it to rise from.
    ///
    /// The boundary is `index / count` — the same fraction
    /// [`Solid::tread_top_box_of`]'s `lo` is for tread `index` — passed to
    /// [`Prism::footprint`](crate::facing::Prism::footprint) as both `lo` and
    /// `hi`, degenerate on the climb axis: a plane, not a strip.
    fn tread_riser_box_of(
        x: i32,
        y: i32,
        low_z: i32,
        high_z: i32,
        up: Face,
        index: usize,
        count: usize,
    ) -> crate::solid::Solid {
        use crate::camera::WorldSpot;
        use crate::facing::Prism;

        let (x, y) = (f64::from(x), f64::from(y));
        let b = index as f64 / count as f64;
        let (min_x, max_x, min_y, max_y) = Prism::footprint(x, y, up, b, b);
        crate::solid::Solid {
            min: WorldSpot {
                x: min_x,
                y: min_y,
                z: f64::from(low_z),
            },
            max: WorldSpot {
                x: max_x,
                y: max_y,
                z: f64::from(high_z),
            },
        }
    }

    /// Which tiles this solid's box touches, as inclusive ranges on each axis.
    ///
    /// Off [`Solid::space`], not off a kind alone: a panel is flat on the axis
    /// it has no extent on, and flooring that axis recovers the one tile it
    /// stands on — **except** [`Solid::box_of`]'s `EDGE_EAST` and `EDGE_SOUTH`
    /// cases, whose plane sits at the *far* boundary of their own tile
    /// (`x + 1`, `y + 1`, an integer) rather than the near one. Flooring that
    /// integer lands on the neighbour, not the tile the solid was pushed for —
    /// found on the real map (`tests/cost.rs`'s oracle), where a wall's own
    /// east panel spilled into the tile to its east and was referenced twice.
    /// `self.edges` is what tells the two apart; a lid or a body is never
    /// degenerate on either axis, so it never reaches the branch that reads it.
    ///
    /// [`bake`]'s spill (decision 38.2) is the one caller, and it is why this
    /// exists rather than being folded into it: nothing [`Solid::box_of`]
    /// builds today reaches past the tile it was given, so every call this
    /// makes returns that one tile on both axes — the honest state of step
    /// 23.2, and not a case nobody hit. The day a box is wider, this is where
    /// the extra tiles come from, unchanged.
    ///
    /// **A riser's boundary is `far` without being at that integer edge** —
    /// gbuffer.md step 4b's own finding, caught before it shipped rather than
    /// after. [`Solid::tread_riser_box_of`] is degenerate the same way a
    /// `box_of` panel is, and for a tread past the first its boundary
    /// (`index / count`) is a proper fraction, not `x + 1`/`y + 1` — the `-1`
    /// below is only correct where the plane really is the tile's own far
    /// edge, which for a riser is true at `index == 0` and false everywhere
    /// past it. Checking `min.fract() == 0.0` is free for every existing
    /// caller (`box_of`'s planes are always built from whole tile
    /// coordinates, so the fraction is always exactly zero there) and is what
    /// stops a mid-flight riser reading as the tile beside it.
    pub(crate) fn footprint(&self) -> (std::ops::RangeInclusive<i32>, std::ops::RangeInclusive<i32>) {
        // `far` is whether this axis's degenerate plane sits at its tile's
        // high boundary rather than its low one — see the doc above.
        fn axis(min: f64, max: f64, far: bool) -> std::ops::RangeInclusive<i32> {
            let lo = match max > min {
                true => min.floor() as i32,
                false if far && min.fract() == 0.0 => min.floor() as i32 - 1,
                false => min.floor() as i32,
            };
            let hi = if max > min { max.ceil() as i32 - 1 } else { lo };
            lo..=hi
        }
        (
            axis(self.space.min.x, self.space.max.x, self.edges & EDGE_EAST != 0),
            axis(self.space.min.y, self.space.max.y, self.edges & EDGE_SOUTH != 0),
        )
    }
}

/// One tile's worth of occlusion: how much it stops, and between which heights.
///
/// **The merged view**, and no longer what is stored: the union of everything on
/// the tile, folded out of [`Occlusion::solids_at`] for the readers whose
/// question is about a *tile* rather than about a solid — the wireframe
/// overlay, the plan view, the mounted flame's own cell. The walk does not ask
/// it any more.
///
/// The span is in `z` units — the map's own, not pixels — and it is inclusive of
/// `bottom` and `top`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    /// The lowest `z` this tile stops anything at.
    pub bottom: i32,
    /// The highest.
    pub top: i32,
    /// How much of a ray crossing the span is stopped.
    pub opacity: u8,
    /// Which sides of the tile the things standing here occupy — the union over
    /// all of them. A ray is stopped only where it crosses one of these.
    ///
    /// Zero is a **lid** and not "nothing": something horizontal, whose whole
    /// occlusion is the `z` span above. [`EDGE_ANY`] is the old whole-tile
    /// answer and what an unreadable static gets. See [`EDGE_NORTH`].
    pub edges: u8,
}

/// A tile with nothing at all over it: the whole of the sky.
pub const SKY_OPEN: u8 = 255;

/// Where one tile's references are: the index `docs/lighting.md`'s decision 30.3
/// keeps the tile grid as, pointing at [`Occlusion::ids`] since step 23.1.
///
/// A count of zero is open ground, and the offset is then meaningless — a caller
/// reads [`Occlusion::ids_at`], which hands back an empty slice for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Span {
    /// Where this tile's run begins in [`Occlusion::ids`]. Twenty-four bits
    /// of it survive the upload — see [`Occlusion::bytes`].
    offset: u32,
    /// How many solids the tile references. One byte, for the same reason.
    count: u8,
}

/// **The thing that was added**, as the world names it — one
/// [`Builder::add`], one of these, however many [`Solid`]s that call pushes.
///
/// `docs/lighting_height.md` phase 3's first decision. A corner is two panels, a
/// flight of steps is a lid and a riser per tread, and all of them are *one*
/// static standing at one place: a fragment drawn from that static's picture
/// belongs to every one of its solids equally, so identity has to be the thing
/// added and not the box it was cut into. That is also what makes "one static,
/// several solids" a non-question — there is no run to name inside a tile.
///
/// **The key is the world thing and not a walk order**, and both halves of that
/// are load-bearing:
///
/// - Not a counter [`Builder`] hands out. [`bake`] builds a *block's* solids
///   once and pastes them into frame after frame for as long as the atlas
///   revision holds, so a number that depended on the order one frame's walk
///   found things in would be a number from another frame.
/// - Not "the n-th static of this tile" either, tempting as it is at eight bits.
///   The two walks that would have to agree on such an index refuse *different*
///   statics — this side drops `opacity == CLEAR` and everything the cutaway or
///   the draw ceiling hides, the drawing side drops whatever the atlas has no
///   art for — so the two numberings diverge exactly where a tile holds
///   something invisible.
///
/// The tile is not in here: a [`Solid`] is always one tile's
/// ([`Builder::finish`]'s own doc), and every comparison this is made in is
/// inside the fragment's own cell. So what is left to tell two things on one
/// tile apart is the `z` they stand at and the graphic they are — three bytes,
/// and never uploaded: what rides the wire is [`OwnerId`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Owner {
    /// The `z` the static was placed at — its own, not any solid's span.
    pub z: i8,
    /// And its graphic, the *placed* one rather than whichever animation frame
    /// is showing: the two walks read the same field, and an animated static
    /// would otherwise change owner every hundred milliseconds.
    pub graphic: Graphic,
}

impl Owner {
    /// The owner a hand-built scene states for one [`Builder::add_raw`] box.
    ///
    /// There is no `tiledata` behind such a box to derive a key from, and
    /// inventing one inside the builder would be a second identity beside this
    /// one — so the caller says, and this is only the naming.
    pub fn new(z: i8, graphic: Graphic) -> Self {
        Self { z, graphic }
    }
}

/// Which occluder **of this cell** an [`Owner`] is, in one byte: what the wire
/// carries and what a fragment is compared against.
///
/// `docs/lighting_height.md` phase 3's third decision. The comparison is only
/// ever made between a fragment and a solid on the fragment's *own* cell — every
/// arm of `light::exemption` that asks it is gated on `own_cell` — so this has to
/// be unique in a tile and not in a frame, and a tile holds at most
/// [`MAX_SOLIDS_PER_CELL`] of anything. One byte, in the fourth channel of a
/// *reference* ([`Occlusion::id_bytes`]), which a [`SolidId`] leaves free.
///
/// [`OwnerId::NONE`] is zero and numbering starts at one, so "this fragment is
/// not a point of any occluder" — the ground, a mobile, a pass with no grid
/// behind it — matches nothing rather than matching whichever solid happened to
/// be numbered first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OwnerId(u8);

impl OwnerId {
    /// No owner at all: what the ground and a mobile stamp, and what a lookup
    /// that found nothing answers.
    ///
    /// **Matches nothing, including itself**, which is why every comparison goes
    /// through [`OwnerId::same`] rather than `==`. Two fragments that are each a
    /// point of nothing are not a point of the same thing, and a solid is never
    /// this.
    pub const NONE: Self = Self(0);

    /// The `n`th owner of a cell, counting from one — [`Builder::finish`]'s own
    /// numbering, and the only thing that makes one.
    fn nth(at: usize) -> Self {
        Self((at + 1) as u8)
    }

    /// Whether a fragment carrying this is a point of the solid carrying `other`.
    ///
    /// Not `==`: [`OwnerId::NONE`] is the absence of an owner and two absences
    /// are not a match. See that constant.
    pub fn same(self, other: Self) -> bool {
        self != Self::NONE && self == other
    }

    /// The byte the wire carries, for the upload and for the shader's own
    /// comparison.
    pub fn raw(self) -> u8 {
        self.0
    }

    /// And back, for the one reader that gets it from outside — a test or a
    /// tool reading an uploaded plane back.
    pub fn from_raw(raw: u8) -> Self {
        Self(raw)
    }
}

/// Which solid of a frame's list, and **not** which reference of a cell.
///
/// The two were one number until step 23.1 and are now two, which is the whole
/// of decision 38's ownership: a cell holds a run of these, and several cells may
/// hold the same one. A newtype because they are about to be told apart in every
/// loop that reads the grid — the index a cell counts through is a place in
/// [`Occlusion::ids`], and what it finds there is a place in the solids.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SolidId(u32);

impl SolidId {
    /// The id of the `n`th solid of a list.
    pub fn new(at: u32) -> Self {
        Self(at)
    }

    /// Where it points, for the two readers that leave the domain: the upload's
    /// three bytes, and a slice index.
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// How wide the lists are as textures, in texels.
///
/// A list is one dimensional and a texture is not, so all three of them — the
/// ids, the solids, the holes — are folded into rows of this. `blit.wgsl`'s
/// `LIST_ROW`, and the two are one number. A thousand and twenty-four rather
/// than the 2048 WebGL2 guarantees, because the guarantee is the floor and a row
/// that is exactly it leaves no room for the folding to be wrong in only one
/// direction.
pub const LIST_ROW: u32 = 1024;

/// The occluders of one frame: the solids, the references to them, and the tile
/// grid as the index of those.
///
/// Decision 30 — a cell is `(offset, count)`, and the walk iterates a tile's two
/// or three rather than reading one merged span. **Step 23.1 put a level between
/// them**: the run a cell names is a run of [`SolidId`], and the solid is looked
/// up once more. That indirection is the property being bought and not a cost
/// paid for nothing — a solid is a shape the world holds, so a stair reaching
/// over four tiles is one record referenced four times rather than four records
/// that have to agree, and every seam this pass has fought was made by cutting
/// geometry on a tile boundary. Nothing spans a cell yet; decision 38.2's spill
/// is the first thing that will.
///
/// Empty cells are the ordinary case, most of a street being open sky, and
/// [`Occlusion::at`], [`Occlusion::ids_at`] and [`Occlusion::solids_at`] all
/// answer for a tile outside the rectangle without the caller having to know
/// where the edge is.
///
/// Built by a [`Builder`] and immutable afterwards: the merge is the builder's
/// business, and what comes out of it is a list nothing appends to. That is what
/// lets a tile's references be contiguous, which is what an `(offset, count)` is.
#[derive(Clone, PartialEq, Debug)]
pub struct Occlusion {
    bounds: TileBounds,
    /// Row-major over `bounds`, `x` fastest: the order [`Occlusion::bytes`]
    /// uploads and the shader indexes.
    index: Vec<Span>,
    /// Every reference in the frame, the ones of a tile contiguous. The order is
    /// the index's, which is what [`Occlusion::id_bytes`] uploads.
    ids: Vec<SolidId>,
    /// Which occluder of its own cell each of those references is — one
    /// [`OwnerId`] per entry of `ids`, in the same order, and the fourth channel
    /// [`Occlusion::id_bytes`] uploads.
    ///
    /// Beside the references rather than beside the solids because it is a fact
    /// about a *reference*: the number is unique within a cell, and the first
    /// thing to reference one solid from two cells (decision 38.2's spill) gives
    /// it a different number in each. Nothing does today — the two lists are the
    /// same length and one solid is one cell's — which is exactly why the level
    /// is built now rather than after something depends on it being wrong.
    owners: Vec<OwnerId>,
    /// Every solid in the frame, in the order [`Occlusion::solid_bytes`] uploads
    /// and [`SolidId`] names.
    solids: Vec<Solid>,
    /// How much of the sky each tile can see, in the same order as the index —
    /// see this module's header and [`Occlusion::sky_at`].
    ///
    /// A byte and not an `Option`: every tile has an answer, and the answer for
    /// a tile with nothing over it is [`SKY_OPEN`] rather than "absent".
    sky: Vec<u8>,
    /// How many solids did not fit — see [`Occlusion::dropped`].
    dropped: usize,
}

impl Occlusion {
    /// A grid covering no tiles at all, which occludes nothing anywhere.
    ///
    /// A `const` and therefore empty `Vec`s, which allocate nothing: it is
    /// what [`Lighting::NONE`](crate::light::Lighting::NONE) is built from, and
    /// a daylit frame must not pay for a grid it will not read.
    pub const EMPTY: Self = Self {
        bounds: TileBounds {
            min_x: 0,
            max_x: -1,
            min_y: 0,
            max_y: -1,
        },
        index: Vec::new(),
        ids: Vec::new(),
        owners: Vec::new(),
        solids: Vec::new(),
        sky: Vec::new(),
        dropped: 0,
    };

    /// The rectangle of tiles this covers.
    pub fn bounds(&self) -> TileBounds {
        self.bounds
    }

    /// Whether this grid covers no tiles at all — [`Occlusion::EMPTY`], and the
    /// grid a frame with no lighting binds.
    ///
    /// Not "nothing stands in it": a grid over real tiles with no occluder on
    /// any of them still answers [`Occlusion::sky_at`] for every one of them, and
    /// the caller that asks this — [`Lighting::is_identity`](crate::light::Lighting::is_identity)
    /// — is asking whether there is a field to read at all.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// The solids one tile references, in no order the walk depends on — and an
    /// empty slice for open ground and for anything outside the rectangle.
    ///
    /// The **references**, which is what a cell holds since step 23.1 and what
    /// the shader's own loop counts through. A caller that wants the solids and
    /// not their names reads [`Occlusion::solids_at`]; a caller that wants a
    /// solid's *place* in the list — because the hole beside it is indexed by
    /// that place — needs this one.
    pub fn ids_at(&self, x: i32, y: i32) -> &[SolidId] {
        let Some(index) = self.index(x, y) else {
            return &[];
        };
        let span = self.index[index];
        let from = span.offset as usize;
        &self.ids[from..from + usize::from(span.count)]
    }

    /// One solid of the frame's list, by the name a cell holds.
    ///
    /// # Panics
    ///
    /// On an id from another frame's grid. Every [`SolidId`] this crate makes
    /// comes out of [`Occlusion::ids_at`] on the same grid — the walk carries one
    /// from a cell to here and nowhere else — so a stale id is a caller mixing
    /// two frames rather than a value a reader has to defend against.
    pub fn solid(&self, id: SolidId) -> &Solid {
        &self.solids[id.raw() as usize]
    }

    /// Which occluder of its own cell each of that tile's references is, in the
    /// same order [`Occlusion::ids_at`] hands them back.
    ///
    /// The walk reads this beside the solid, since what it compares a fragment
    /// against is the number and not the key — see [`OwnerId`] and
    /// `light::exemption`.
    pub fn owners_at(&self, x: i32, y: i32) -> &[OwnerId] {
        let Some(index) = self.index(x, y) else {
            return &[];
        };
        let span = self.index[index];
        let from = span.offset as usize;
        &self.owners[from..from + usize::from(span.count)]
    }

    /// The solids standing on one tile, each with the [`OwnerId`] its own
    /// reference carries — what both walks iterate.
    ///
    /// A tile carries one lid, or one body, or a panel per side its art named; a
    /// caller combines them itself, and the combination is a rule rather than a
    /// fold — see `light::walk_cells_exact`, which takes the largest and not the
    /// product, because two panels of one wall are one wall.
    ///
    /// The pair and not the solid alone, because the number is a fact about the
    /// *reference*: [`Occlusion::owners`] says why, and a caller that zipped the
    /// two lists itself would be the second place that has to stay in step.
    pub fn cell(&self, x: i32, y: i32) -> impl Iterator<Item = (&Solid, OwnerId)> + '_ {
        self.ids_at(x, y)
            .iter()
            .zip(self.owners_at(x, y))
            .map(|(id, owner)| (self.solid(*id), *owner))
    }

    /// The solids standing on one tile, followed through their references.
    ///
    /// For the readers whose question is about the geometry alone — a picture of
    /// a tile, the tallest thing in a frame. A walk wants [`Occlusion::cell`],
    /// which carries the owner beside each one.
    pub fn solids_at(&self, x: i32, y: i32) -> impl Iterator<Item = &Solid> + '_ {
        self.ids_at(x, y).iter().map(|id| self.solid(*id))
    }

    /// Which occluder of `(x, y)` the static standing at `z` with graphic
    /// `graphic` is — [`OwnerId::NONE`] where this frame's grid has no such
    /// static on that tile.
    ///
    /// **The join `docs/lighting_height.md` phase 3 pays for**, and the one real
    /// cost of the design: the pass that *draws* a static has to learn the number
    /// the *grid* gave it, so a frame's occlusion has to be built before its
    /// statics are collected. A scan of the cell and not a map — a tile holds two
    /// or three solids, not two or three hundred — asked once per drawn static
    /// rather than once per pixel.
    ///
    /// `NONE` rather than an `Option` because the answer is the same one a
    /// fragment with no occluder behind it stamps, and every caller here would
    /// immediately fold an `Option` into exactly that: a static the grid refused
    /// (`opacity == CLEAR`, above the draw ceiling, hidden by the cutaway) is a
    /// static nothing in the walk can be a point of.
    pub fn owner_at(&self, x: i32, y: i32, z: i8, graphic: Graphic) -> OwnerId {
        let key = Owner::new(z, graphic);
        self.cell(x, y)
            .find(|(solid, _)| solid.owner == key)
            .map_or(OwnerId::NONE, |(_, owner)| owner)
    }

    /// What stands on one tile as one box, or `None` for open ground and for
    /// anything outside the rectangle.
    ///
    /// The **merged view** of [`Occlusion::solids_at`] and derived from it on
    /// every call: the union of the spans, the largest opacity and the union of
    /// the sides. For the readers whose question is genuinely about a tile — the
    /// wireframe, the plan view, which way a mounted flame steps out of its own
    /// cell — and not for the walk, which stopped asking it when the list
    /// arrived.
    pub fn at(&self, x: i32, y: i32) -> Option<Cell> {
        let mut solids = self.solids_at(x, y);
        let first = solids.next()?;
        Some(solids.fold(
            Cell {
                bottom: first.bottom(),
                top: first.top(),
                opacity: first.opacity,
                edges: first.edges,
            },
            |cell, solid| Cell {
                bottom: cell.bottom.min(solid.bottom()),
                top: cell.top.max(solid.top()),
                opacity: cell.opacity.max(solid.opacity),
                edges: cell.edges | solid.edges,
            },
        ))
    }

    /// How much of the sky one tile can see: [`SKY_OPEN`] under open air, `0`
    /// under a roof, and between under glass or beside a doorway.
    ///
    /// Open sky outside the rectangle, which is the honest default in the one
    /// direction that matters: the grid is grown by the widest pool's reach, so
    /// a tile outside it is a tile the frame does not draw, and a caller
    /// sampling one is asking about a place this frame knows nothing about.
    /// Answering "dark" there would put a band of night around every frame.
    pub fn sky_at(&self, x: i32, y: i32) -> u8 {
        match self.index(x, y) {
            Some(index) => self.sky[index],
            None => SKY_OPEN,
        }
    }

    /// Every tile something stands on, as `(x, y, cell)` — the grid as the boxes
    /// it is, for whatever wants to draw it.
    ///
    /// Open tiles are skipped: a grid is mostly nothing, and a caller drawing a
    /// box per cell would spend most of its work on cells with no box. The order
    /// is the rectangle's own, row by row, which is [`Occlusion::bytes`]'s and
    /// therefore stable frame to frame for a camera that has not moved.
    pub fn boxes(&self) -> impl Iterator<Item = (i32, i32, Cell)> + '_ {
        let bounds = self.bounds;
        let width = bounds.width();
        (0..self.index.len() as i32).filter_map(move |index| {
            let (x, y) = (bounds.min_x + index % width, bounds.min_y + index / width);
            Some((x, y, self.at(x, y)?))
        })
    }

    /// The second plane the shader reads: `Rgba8Uint`, one texel a tile, in
    /// [`Occlusion::bytes`]'s own order over the same rectangle.
    ///
    /// `(sky, 0, 0, 0)`. The three zeros are not padding, they are the format
    /// being decided once — see this module's header. What a tile *is* goes
    /// here; what a ray passes through stays in [`Occlusion::bytes`].
    pub fn field_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.sky.len() * 4);
        for sky in &self.sky {
            bytes.extend_from_slice(&[*sky, 0, 0, 0]);
        }
        bytes
    }

    /// The highest `z` anything in this grid stops light at, or `None` for a
    /// grid with nothing standing in it.
    ///
    /// What sunlight is bounded by. A flame's ray ends at the flame; the sun's
    /// has no end, so it needs something to stop walking at — and the honest
    /// answer is "as soon as the ray is above everything that could stop it".
    /// One number for the frame rather than a per-cell test, because the walk is
    /// leaving the grid upwards and what it has to beat is the tallest thing
    /// anywhere ahead of it.
    pub fn tallest(&self) -> Option<i32> {
        self.solids.iter().map(Solid::top).max()
    }

    /// How many solids stand in the frame at all — what
    /// [`Occlusion::solid_bytes`] uploads.
    pub fn solid_count(&self) -> usize {
        self.solids.len()
    }

    /// How many *references* to them the cells hold — what
    /// [`Occlusion::id_bytes`] uploads, and the number decision 30.6 has a
    /// distribution of.
    ///
    /// Equal to [`Occlusion::solid_count`] until something spans a cell, and the
    /// pair is worth printing side by side from that day on: the two being equal
    /// is what says *nothing is shared yet*, which is a fact about the map's
    /// geometry rather than about this format.
    pub fn reference_count(&self) -> usize {
        self.ids.len()
    }

    /// How many solids the frame measured and could not store, because their
    /// tile was already holding [`MAX_SOLIDS_PER_CELL`].
    ///
    /// Decision 30.6: a grid that quietly truncates reads as "covered everything"
    /// when it did not, so what is dropped is counted and whoever measures the
    /// grid prints it. A frame that drops anything at all is a frame with a wall
    /// missing from it.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// How many tiles reference how many solids: `histogram()[n]` is the number
    /// of tiles referencing exactly `n`, open ground included at `n = 0`.
    ///
    /// The distribution decision 30.6 asks for rather than the total
    /// [`Occlusion::reference_count`] is — the question is what a *cell* holds,
    /// and a mean over a city answers it with the wrong shape: 10,000 tiles of
    /// one solid and one tile of forty is the case a truncation has to be chosen
    /// against, and a total cannot tell it from 10,000 tiles of one and a bit.
    pub fn histogram(&self) -> Vec<usize> {
        let mut counts = Vec::new();
        for span in &self.index {
            let at = usize::from(span.count);
            if at >= counts.len() {
                counts.resize(at + 1, 0);
            }
            counts[at] += 1;
        }
        counts
    }

    /// Where a tile lives in [`Occlusion::index`].
    fn index(&self, x: i32, y: i32) -> Option<usize> {
        let bounds = self.bounds;
        if x < bounds.min_x || x > bounds.max_x || y < bounds.min_y || y > bounds.max_y {
            return None;
        }
        let (column, row) = (x - bounds.min_x, y - bounds.min_y);
        Some((row * bounds.width() + column) as usize)
    }

    /// The **index** as the texture the shader reads: `Rgba8Uint`, one texel a
    /// tile, row-major from the rectangle's `(min_x, min_y)` corner.
    ///
    /// `(offset & 255, offset >> 8, offset >> 16, count)` — decision 30.3's
    /// `(offset, count)`, with the offset spread over three channels because one
    /// byte holds 255 references and a city block holds thousands. Twenty-four
    /// bits is sixteen million, which is four hundred references on every tile of
    /// the widest frame this renderer draws.
    ///
    /// **What it points at is [`Occlusion::id_bytes`]** since step 23.1, and the
    /// texel is unchanged by that: an offset into one flat list is an offset into
    /// another, and the level the shader gained is a second fetch rather than a
    /// wider index.
    ///
    /// A count of zero is open ground, and it is the whole of the presence test:
    /// the offset of an empty tile is whatever the run before it ended at, and
    /// the shader never reads it. What used to be the `PRESENT` bit of a cell is
    /// now this — and `PRESENT` moved with the span it belongs to, into
    /// [`Occlusion::solid_bytes`].
    pub fn bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.index.len() * 4);
        for span in &self.index {
            bytes.extend_from_slice(&[
                (span.offset & 0xFF) as u8,
                ((span.offset >> 8) & 0xFF) as u8,
                ((span.offset >> 16) & 0xFF) as u8,
                span.count,
            ]);
        }
        bytes
    }

    /// The **references** as the texture the shader reads: `Rgba8Uint`, one texel
    /// a reference, folded into rows [`LIST_ROW`] wide and padded to a whole row.
    ///
    /// `(id & 255, id >> 8, id >> 16, owner)` — a [`SolidId`] spread over three
    /// channels exactly as an offset is in [`Occlusion::bytes`], and for the same
    /// reason.
    ///
    /// **The fourth channel is [`OwnerId`]**, `docs/lighting_height.md` phase 3.
    /// It stayed zero for as long as nothing read it — that comment's own words
    /// were that a channel filled with something plausible would be a field the
    /// walk has to be taught to ignore — and a reader exists now: which occluder
    /// of this cell a solid belongs to is exactly a fact about the *reference*,
    /// not about the solid, so it goes here and needs no plane of its own. Zero
    /// is [`OwnerId::NONE`] and no reference ever writes it: the numbering starts
    /// at one.
    ///
    /// A cell with no solids on it writes no texel here at all: its count is zero
    /// and its offset is never read.
    pub fn id_bytes(&self) -> Vec<u8> {
        let row = LIST_ROW as usize;
        let rows = self.ids.len().div_ceil(row).max(1);
        let mut bytes = Vec::with_capacity(rows * row * 4);
        for (id, owner) in self.ids.iter().zip(&self.owners) {
            let at = id.raw();
            bytes.extend_from_slice(&[
                (at & 0xFF) as u8,
                ((at >> 8) & 0xFF) as u8,
                ((at >> 16) & 0xFF) as u8,
                owner.raw(),
            ]);
        }
        bytes.resize(rows * row * 4, 0);
        bytes
    }

    /// The **solids** as the texture the shader reads: `Rgba8Uint`, one texel a
    /// solid, folded into rows [`LIST_ROW`] wide and padded to a whole row.
    ///
    /// `(0, 0, opacity, PRESENT | edges)`.
    ///
    /// **The first two channels held the span, and hold nothing now.** They were
    /// `(bottom + 128, top + 128)`, a whole `z` a channel — what a cell's texel
    /// was before the list existed, moved down two levels unchanged — until
    /// `docs/lighting_height.md` phase 2 gave the span a plane of its own
    /// ([`Occlusion::solid_z_bytes`]) with sixteen bits an end. Keeping a rounded
    /// copy of a number that now lives somewhere else, and lives there *better*,
    /// is how two answers to one question get built: the shader would have had a
    /// height it could read without the plane, still compiling, still looking
    /// like a height, and wrong by up to half a unit. Zeroed rather than
    /// repurposed — a channel is claimed when a reader for it exists, which is
    /// the rule [`Occlusion::footprint_bytes`] was held to for three sessions.
    ///
    /// **The box's `x` and `y` are not in *this* plane — they were withheld
    /// until a reader existed for them, and now one does.** What used to be
    /// true here — the horizontal axes have no reader in the shader, a panel
    /// on the north side of a cell is the line `y = cell.y`, derived where it
    /// is used, and sending four more channels beside a walk that ignored
    /// them would be exactly how a format grows a field nobody dares change —
    /// held for exactly as long as it stayed true. `docs/lighting_raymarch.md`'s
    /// "second bigger idea": [`Occlusion::footprint_bytes`] is that reader's
    /// own plane, not four more channels of this one, for the same reason
    /// [`Occlusion::aperture_bytes`] is a plane and not four more channels —
    /// this list is what the walk reads cell after cell in a loop, and adding
    /// width to every texel here would widen the one thing the loop actually
    /// pays for on every solid, not just the ones a footprint narrows.
    ///
    /// [`PRESENT`] is still written, and it is still not padding: a lid's mask is
    /// legitimately zero, so a texel of all zeros has to be distinguishable from
    /// a horizontal solid at `z = -128`. What says a *tile* is empty is the
    /// index's count.
    pub fn solid_bytes(&self) -> Vec<u8> {
        let row = LIST_ROW as usize;
        let rows = self.solids.len().div_ceil(row).max(1);
        let mut bytes = Vec::with_capacity(rows * row * 4);
        for solid in &self.solids {
            let holed = match solid.aperture {
                Some(_) => HOLED,
                None => 0,
            };
            bytes.extend_from_slice(&[0, 0, solid.opacity, PRESENT | holed | solid.edges]);
        }
        bytes.resize(rows * row * 4, 0);
        bytes
    }

    /// The **holes** as the texture the shader reads: `Rgba8Uint`, one texel a
    /// solid, in [`Occlusion::solid_bytes`]'s own order and folded the same way.
    ///
    /// `(near, far, bottom + 128, top + 128)` — [`Aperture`], with the `z` offset
    /// and the clamp [`Occlusion::solid_bytes`] uses for the same reason.
    ///
    /// A parallel plane and **not** four more channels of the solid, nor a
    /// second texel interleaved into the list, and the reason is the shape of the
    /// walk: the solid list is what a ray reads cell after cell in a loop, and
    /// a hole is what almost nothing has. Interleaving would halve the list's
    /// texture cache to carry zeros; a plane indexed by the same number is read
    /// only where [`HOLED`] says there is something to read, which is the way
    /// every other miss in this pass is paid for.
    ///
    /// A solid with no hole is four zeros, which is a hole of no width at
    /// `z = -128`: degenerate, and never looked at, because the bit above gates
    /// it.
    pub fn aperture_bytes(&self) -> Vec<u8> {
        let row = LIST_ROW as usize;
        let rows = self.solids.len().div_ceil(row).max(1);
        let mut bytes = Vec::with_capacity(rows * row * 4);
        for solid in &self.solids {
            match solid.aperture {
                Some(hole) => {
                    bytes.extend_from_slice(&[hole.near, hole.far, z_byte(hole.bottom), z_byte(hole.top)])
                }
                None => bytes.extend_from_slice(&[0, 0, 0, 0]),
            }
        }
        bytes.resize(rows * row * 4, 0);
        bytes
    }

    /// Whether any solid in the frame has a hole in it at all.
    ///
    /// What the upload asks before it writes [`Occlusion::aperture_bytes`]: until
    /// step 16 measures a window off the art, no frame of a real map has one, and
    /// a plane of zeros does not need laying out and sending every frame.
    pub fn any_aperture(&self) -> bool {
        self.solids.iter().any(|solid| solid.aperture.is_some())
    }

    /// The **footprints** as the texture the shader reads: `Rgba8Uint`, one
    /// texel a solid, indexed and folded exactly as [`Occlusion::solid_bytes`]
    /// and [`Occlusion::aperture_bytes`] already are.
    ///
    /// `(min.x, max.x, min.y, max.y)`, [`Solid::fraction`]'s own four bytes —
    /// `docs/lighting_raymarch.md`'s "second bigger idea" landed: the box's
    /// horizontal extent, deliberately withheld from [`Occlusion::solid_bytes`]
    /// until a reader existed for it (that comment's own words), now has one
    /// on both sides — `blit.wgsl`'s `box_of` reads this plane instead of
    /// reconstructing a whole tile or a panel strip from `edges` alone.
    ///
    /// Written every frame, unlike [`Occlusion::aperture_bytes`]'s own
    /// hole-gated write: every solid has a footprint (an ordinary lid or body
    /// is simply `(0, 255, 0, 255)`, the whole tile), where almost none has a
    /// hole, so there is no bit here to gate the read on the way
    /// [`HOLED`] gates `apertures`.
    pub fn footprint_bytes(&self) -> Vec<u8> {
        let row = LIST_ROW as usize;
        let rows = self.solids.len().div_ceil(row).max(1);
        let mut bytes = Vec::with_capacity(rows * row * 4);
        for solid in &self.solids {
            bytes.extend_from_slice(&solid.fraction());
        }
        bytes.resize(rows * row * 4, 0);
        bytes
    }

    /// The **heights** as the texture the shader reads: `Rgba8Uint`, one texel a
    /// solid, indexed and folded exactly as [`Occlusion::footprint_bytes`] and
    /// [`Occlusion::solid_bytes`] are.
    ///
    /// [`Solid::z_bytes`]' own four — the whole span, sixteen bits an end, and
    /// the only place a solid's height is on the wire at all.
    /// `docs/lighting_height.md` phase 2: the horizontal half of a solid's box
    /// had been exact to a two-hundred-and-fifty-fifth of a tile since the
    /// footprint plane landed, and the vertical half was a whole unit — a box
    /// based at `z 3.5` was uploaded as one spanning `4`, so the bottom half unit
    /// of its own faces sat *below* its own solid and every rule that asks
    /// whether a fragment is inside the thing it is drawn from answered no.
    ///
    /// A plane of its own and not four more channels of the solid list, for the
    /// reason [`Occlusion::footprint_bytes`] is one: the list is what the walk
    /// reads cell after cell in a loop, and widening its texel is paid for on
    /// every solid a ray meets. Sixteen bits an end would not have fitted there
    /// in any case — which is the same fact from the other side.
    ///
    /// Written every frame, like the footprints and unlike the holes: every
    /// solid has a height, so there is nothing to gate the read on the way
    /// [`HOLED`] gates `apertures`. The row's own padding is zeros, which decodes
    /// as a degenerate span at [`Z_FLOOR`] — never looked at, for the reason the
    /// apertures' padding is not: no id points into it.
    pub fn solid_z_bytes(&self) -> Vec<u8> {
        let row = LIST_ROW as usize;
        let rows = self.solids.len().div_ceil(row).max(1);
        let mut bytes = Vec::with_capacity(rows * row * 4);
        for solid in &self.solids {
            bytes.extend_from_slice(&solid.z_bytes());
        }
        bytes.resize(rows * row * 4, 0);
        bytes
    }
}

/// The end of a tile's list of solids, in [`Builder::heads`] and in
/// [`Builder::arena`]'s links.
const NO_SOLID: u32 = u32::MAX;

/// How many solids one tile may reference: the format's own ceiling and not a
/// number anybody chose.
///
/// A cell's count is one byte — [`Occlusion::bytes`] — so 255 is what an
/// `(offset, count)` can name, and decision 30.6 asks for the real bound to come
/// from a distribution measured over a city rather than from a guess. Until that
/// measurement says otherwise this is the only cap there is, and what it drops is
/// counted rather than silently thrown away: [`Occlusion::dropped`].
pub const MAX_SOLIDS_PER_CELL: usize = 255;

/// Builds one frame's [`Occlusion`]: a solid at a time, packed at the end.
///
/// The grid is written tile by tile — a static at a time, in whatever order the
/// map walk finds them — and what comes out is a packed list nothing appends to.
/// The two are separate types because the two are separate shapes: a tile's
/// references have to be contiguous for an `(offset, count)` to name them, and
/// they cannot be while anything can still be added.
///
/// A tile's solids live in [`Builder::arena`] as a linked list rather than in a
/// `Vec` of their own, and that is a cost decision rather than a taste one: a
/// widest-zoom grid is 35,000 tiles of which 10,000 stand, so a `Vec` a tile
/// would be 35,000 allocations a frame on the side of this pass that is already
/// thirteen times the GPU.
#[derive(Clone, PartialEq, Debug)]
pub struct Builder {
    bounds: TileBounds,
    /// The first solid of each tile, row-major over `bounds`, or
    /// [`NO_SOLID`] for open ground.
    heads: Vec<u32>,
    /// Every solid added this frame, each with the index of the next one on
    /// its own tile.
    ///
    /// A solid a *tile* holds, still, and that is the honest state of step 23.1:
    /// the ownership has moved into [`Occlusion`], where a cell references what
    /// the frame owns, and the builder above it has not been asked to hold a
    /// solid two tiles reference. Decision 38.2's spill is the step that asks.
    arena: Vec<(Solid, u32)>,
    /// How much of the sky each tile can see, in the same order as `heads`.
    sky: Vec<u8>,
    /// How many solids were refused because their tile was already full — see
    /// [`MAX_SOLIDS_PER_CELL`] and [`Occlusion::dropped`].
    dropped: usize,
}

impl Builder {
    /// An empty grid over `bounds`: nothing stops anything, and every tile sees
    /// the whole sky.
    pub fn new(bounds: TileBounds) -> Self {
        let tiles = (bounds.width() * bounds.height()) as usize;
        Self {
            bounds,
            heads: vec![NO_SOLID; tiles],
            arena: Vec::new(),
            sky: vec![SKY_OPEN; tiles],
            dropped: 0,
        }
    }

    /// Add one occluder: the surfaces it *is*, standing beside whatever else is
    /// already on that tile.
    ///
    /// **Nothing is merged here any more**, which is step 21.2 and the one place
    /// in decision 30 where the picture has to change. What the union used to do
    /// was take the widest span, the largest opacity and the union of the sides
    /// over everything on the tile — conservative in the direction that darkens
    /// for the span and in the direction that *leaks* for the sides, which is not
    /// one direction. A floor over a wall tile contributed its `z` to the wall's
    /// span and lost its own lid-ness; two walls with air between them closed the
    /// gap; a pane beside a wall came out opaque across the whole tile.
    ///
    /// Now each of them is its own surface with its own span, its own opacity and
    /// its own rule — a lid is travelled through, a panel is pierced — and what
    /// the walk does with a tile that holds several is take the largest, which is
    /// the rule it already had. [`Occlusion::at`] still folds the union for the
    /// readers whose question is genuinely about a tile.
    ///
    /// An occluder that names two sides — a **corner**, decision 25 — is two
    /// panels and not one surface with two bits, because a ray crossing both has
    /// gone through one wall once and the walk says so by taking the largest.
    ///
    /// An exact repeat of a surface already on the tile is dropped. Two copies of
    /// one plane at one span stop exactly what one does, and the list is what
    /// decision 30.6 is about to count.
    ///
    /// A tile outside the rectangle is dropped rather than clamped: it is a
    /// caller walking wider than it asked the grid for, and folding it onto the
    /// edge would put a wall where the map has none.
    pub fn add(&mut self, x: u16, y: u16, z: i8, graphic: Graphic, tile: &StaticTile, shape: Shape) {
        let opacity = opacity(graphic, tile);
        if opacity == CLEAR {
            return;
        }
        let place = (i32::from(x), i32::from(y));
        let Some(index) = self.index(place.0, place.1) else {
            return;
        };
        let bottom = i32::from(z);
        let top = bottom + calc_height(tile);
        // One `add` is one owner, and every solid below carries it — the two
        // panels of a corner, a flight's tread tops and risers, a body. See
        // [`Owner`], and `docs/lighting_height.md` phase 3.
        let owner = Owner::new(z, graphic);
        // **A climbable static is a solid, and the art says which one.**
        //
        // A stair's base is two 45° runs meeting at the tile's south corner,
        // which is pixel for pixel what two walls meeting at a corner leave — so
        // `facing_of` reads a flight of steps as a corner of a house and the grid
        // used to stand two opaque panels on its east and south edges. A
        // staircase then shadowed a street like a run of wall, and its own treads
        // shadowed each other.
        //
        // The client's own `CLIMBABLE` bit is what admits the other reading, and
        // it is asked *first* for the reason `is_background` is: a fit alone
        // cannot decide it, because the measure scores a plain wall at 0.81
        // against its best prism. See `facing::PRISM_FITS`.
        //
        // The height comes off the art with it. `tiledata` states ten for the
        // landing at `0x071E` and the artist drew five; it states five for the
        // flight at `0x0736` and the artist drew five. The same field means the
        // full height on one and the drawn height on the other — `Sphere` halves
        // it, `movement::scene::stair` stands a walker half way up it — so the
        // measurement is what this believes.
        if let (true, Some(prism)) = (tile.flags.is_climbable(), shape.prism) {
            // Step 23.5: one body per tread, not one body for the whole flight —
            // stopped by *its own tread's* height over *its own tread's* strip,
            // so the shape on screen is the stair rather than a ziggurat to the
            // landing's height.
            //
            // gbuffer.md step 4b: that one body is now two faces, a lid and a
            // panel, not a body a ray travels through. A tread's own top and
            // riser are honest per-face geometry (decision 3) rather than one
            // box standing in for both — the top's normal is `[0,0,1]`, exactly
            // what a lid already is, and the riser's is `up`'s own outward
            // direction, exactly what a named-edge panel already is. Nothing
            // about *how* a lid or a panel stops a ray is new; only the shape
            // fed to `box_of`'s two existing rules changes.
            let treads = prism.treads();
            let mut risen = bottom;
            for (tread, &height) in treads.iter().enumerate() {
                let top_z = bottom + i32::from(height);
                self.push(
                    index,
                    Solid {
                        space: Solid::tread_top_box_of(
                            place.0,
                            place.1,
                            top_z,
                            prism.up(),
                            tread,
                            treads.len(),
                        ),
                        opacity,
                        edges: 0,
                        aperture: None,
                        roof: tile.flags.is_roof(),
                        owner,
                    },
                );
                self.push(
                    index,
                    Solid {
                        space: Solid::tread_riser_box_of(
                            place.0,
                            place.1,
                            risen,
                            top_z,
                            prism.up(),
                            tread,
                            treads.len(),
                        ),
                        opacity,
                        edges: opposite(edge_of(prism.up())),
                        aperture: None,
                        roof: tile.flags.is_roof(),
                        owner,
                    },
                );
                risen = top_z;
            }
            return;
        }
        // **A climbable static the prism search could not fit is still a
        // climbable static, not a wall.** Falling through to `edges_of` would
        // read its silhouette exactly as the doc above says a stair's base
        // reads — a corner of a house — and narrow it by `PANEL_THICKNESS` on
        // two sides for nothing: the flight is already half-height
        // (`calc_height`), so what a panel reading loses is not "this looks
        // like solid stone", it is a seam short of the tile the neighbour
        // occludes from. One whole-tile body, `EDGE_ANY`'s un-inset case of
        // `box_of`, is the same answer step 23.1 gave every climbable static
        // before decision 34 taught the grid to read a *fitted* one as its own
        // treads — this is that answer for the 37.7% the fit still misses.
        // `docs/lighting.md`'s backlog, "measured rather than argued".
        if tile.flags.is_climbable() {
            self.push(
                index,
                Solid {
                    space: Solid::box_of(place.0, place.1, bottom, top, EDGE_ANY),
                    opacity,
                    edges: EDGE_ANY,
                    aperture: None,
                    roof: tile.flags.is_roof(),
                    owner,
                },
            );
            return;
        }
        // A floor or a rug is a **lid**: its occlusion is the `z` it lies at and
        // no vertical side of the tile describes it, so it names no edge.
        // Everything that stands up names the edge the art gave it, or all four
        // where the art would not say — see `edges_of`.
        //
        // The client's own `FLOOR` bit decides which, exactly as it does for
        // `place::Stance`, and asking it here rather than trusting the face to be
        // `None` is deliberate: a floor whose silhouette happened to read as a
        // wall would otherwise be given one edge out of four and stop three
        // quarters less light than it does today.
        let edges = match tile.flags.is_background() {
            true => 0,
            false => edges_of(shape.facing),
        };
        match edges {
            // A lid, or a body: one surface, and the mask is the whole of what
            // says which of the walk's two rules it takes.
            //
            // **And no hole**, whatever was offered. A hole is a rectangle in the
            // plane of a panel, and neither of these two has a plane: a lid is
            // horizontal, and a body is the fallback for a picture whose facing
            // the art would not name — so there is no run for a `near` and a `far`
            // to be measured along. Dropping it is the same refusal decision 3
            // makes about the edge itself; the alternative is a window in a
            // direction nobody measured.
            0 | EDGE_ANY => self.push(
                index,
                Solid {
                    space: Solid::box_of(place.0, place.1, bottom, top, edges),
                    opacity,
                    edges,
                    aperture: None,
                    roof: tile.flags.is_roof(),
                    owner,
                },
            ),
            named => {
                for side in [EDGE_NORTH, EDGE_EAST, EDGE_SOUTH, EDGE_WEST] {
                    if named & side != 0 {
                        self.push(
                            index,
                            Solid {
                                space: Solid::box_of(place.0, place.1, bottom, top, side),
                                opacity,
                                edges: side,
                                // A corner's two panels are two faces of one
                                // picture, and a hole measured off that picture
                                // is a hole in both of them: the same window seen
                                // from the two sides of the tile it is cut into.
                                // There is nothing in a silhouette that would say
                                // which half a hole belonged to, so the honest
                                // answer is the one that does not invent a
                                // difference.
                                //
                                // And the placement happens here, because here
                                // is where `z` is: the art measured a rectangle
                                // above the picture's own base and this static
                                // is standing at one.
                                aperture: shape.hole.map(|hole| Aperture::above(bottom, hole)),
                                roof: tile.flags.is_roof(),
                                owner,
                            },
                        );
                    }
                }
            }
        }
    }

    /// Put one solid on a tile, unless the tile already has it or is full.
    fn push(&mut self, index: usize, solid: Solid) {
        let mut count = 0;
        let mut at = self.heads[index];
        while at != NO_SOLID {
            let (had, next) = self.arena[at as usize];
            if had == solid {
                return;
            }
            count += 1;
            at = next;
        }
        if count >= MAX_SOLIDS_PER_CELL {
            self.dropped += 1;
            return;
        }
        self.arena.push((solid, self.heads[index]));
        self.heads[index] = self.arena.len() as u32 - 1;
    }

    /// One raw occluder: exactly the box given, opaque, with no shape or
    /// height derived from a [`StaticTile`].
    ///
    /// Everything else in this `impl` builds a [`Solid`]'s `space` from
    /// `tiledata` — [`Solid::box_of`] always fills a whole tile for a body or
    /// a thin strip for a panel — because that is what a real static's
    /// footprint always is. A hand-built scene has no such art to read
    /// (`examples/two_cubes.rs`'s own doc: no client files at all) and
    /// sometimes needs a box the tile grid was never asked to produce —
    /// narrower than a tile, or stacked on top of another box rather than
    /// standing beside it. This is that seam: the caller states the exact
    /// AABB and it is stored as one opaque body, in the same tile bucket
    /// [`Builder::push`] already uses for every other occluder, so the walk
    /// finds it exactly the way it finds a wall.
    ///
    /// `owner` is stated by the caller and not derived here, which is
    /// `docs/lighting_height.md` phase 3's own rule: there is no `tiledata`
    /// behind such a box to read a `(z, graphic)` off, and a key the builder
    /// invented would be a second identity beside the one every real static
    /// already has. A scene that stands two boxes on one tile has to tell them
    /// apart itself — see [`Owner`].
    pub fn add_raw(&mut self, x: u16, y: u16, space: crate::solid::Solid, owner: Owner) {
        let Some(index) = self.index(i32::from(x), i32::from(y)) else {
            return;
        };
        self.push(
            index,
            Solid {
                space,
                opacity: OPAQUE,
                edges: EDGE_ANY,
                aperture: None,
                roof: false,
                owner,
            },
        );
    }

    /// Take a tile's sky away, as far as one static standing over it does.
    ///
    /// `floor` is the height of the ground under the tile, and it is what makes
    /// this a *column over the floor* rather than a census of the tile: a
    /// cellar's wall is below the street it stands under and takes none of that
    /// street's sky, which is the same three-dimensional honesty the shadow walk
    /// gets from a surface's span.
    ///
    /// Multiplicative, so two roofs over one tile do not make it darker than
    /// black and a pane under a slate roof is as dark as the slate — and so that
    /// a pane on its own passes its share. Deliberately **not** filtered by the
    /// frame's [`Cutaway`]; the module header says why, and it is the one place
    /// this crate reads the map as it is rather than as it is drawn.
    ///
    /// A tile outside the rectangle is dropped, exactly as [`Builder::add`]
    /// drops one.
    pub fn shade(&mut self, x: u16, y: u16, z: i8, floor: i8, graphic: Graphic, tile: &StaticTile) {
        let opacity = opacity(graphic, tile);
        if opacity == CLEAR {
            return;
        }
        let top = i32::from(z) + calc_height(tile);
        if top < i32::from(floor) {
            return;
        }
        let Some(index) = self.index(i32::from(x), i32::from(y)) else {
            return;
        };
        let passes = u32::from(SKY_OPEN - opacity);
        self.sky[index] = ((u32::from(self.sky[index]) * passes) / u32::from(SKY_OPEN)) as u8;
    }

    /// How much of the sky one tile can see, part-built — [`Occlusion::sky_at`]'s
    /// own answer, asked of the grid before it is packed.
    pub fn sky_at(&self, x: i32, y: i32) -> u8 {
        match self.index(x, y) {
            Some(index) => self.sky[index],
            None => SKY_OPEN,
        }
    }

    /// Soften the sky field by a tile: one 3x3 pass, in place.
    ///
    /// The last thing done to the field and never done twice — [`collect`] calls
    /// it once, after every occluder has been shaded in, because a blur of a
    /// half-built field is a blur of the wrong picture.
    ///
    /// The edge of the rectangle repeats rather than falling off: a tile outside
    /// the grid is open sky by [`Occlusion::sky_at`]'s rule, and averaging that
    /// in would draw a bright rim around the inside of every frame's border —
    /// which is a picture of where the grid ends, not of where the roof does.
    pub fn blur_sky(&mut self) {
        let (width, height) = (self.bounds.width(), self.bounds.height());
        if width <= 0 || height <= 0 {
            return;
        }
        let mut blurred = vec![SKY_OPEN; self.sky.len()];
        for row in 0..height {
            for column in 0..width {
                let mut total = 0_u32;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let x = (column + dx).clamp(0, width - 1);
                        let y = (row + dy).clamp(0, height - 1);
                        total += u32::from(self.sky[(y * width + x) as usize]);
                    }
                }
                blurred[(row * width + column) as usize] = (total / 9) as u8;
            }
        }
        self.sky = blurred;
    }

    /// Pack the grid into the list the walk reads, keeping what this frame draws.
    ///
    /// One pass in the index's own order, so a tile's surfaces come out
    /// contiguous and in the order the tiles are in — which is what makes the
    /// grid's texture and the list's two views of one thing.
    ///
    /// A tile's own solids come out in the order they were added. The walk does
    /// not depend on it — it takes the largest of them — but a stable order is
    /// what lets a test name a slice and a frame dump be compared with the one
    /// before it.
    ///
    /// # One reference per solid, and it is not a placeholder
    ///
    /// The ids this writes run `0, 1, 2, …` and the solid list is in the cells'
    /// own order, because nothing the builder holds is referenced twice: a solid
    /// is still one tile's. So the plane a cell points through is, today, the
    /// identity — and building it anyway is the same argument step 23.2 makes for
    /// the spill, which is that a missing reference is a hole in a shadow that
    /// looks exactly like a detector failing. The level exists, both walks read
    /// it, and the first thing to share a solid changes one function rather than
    /// a format, a shader, two walks and three views.
    ///
    /// # This is where the [`Cutaway`] is applied, and that is decision 33
    ///
    /// A [`Builder`] holds **what a ray may cross**: every surface standing on
    /// the map inside the rectangle, whether or not this frame draws it. The
    /// filter is here, at the one point a *frame's* grid is made, because
    /// everything above it is a fact about the map and the cutaway is a fact
    /// about where the player is standing. That is the whole of what lets one
    /// build serve two frames — which is what decision 30.4's cache is, once its
    /// storey band is gone.
    ///
    /// The test is [`cutaway::shows`]'s own, reconstructed from what a solid
    /// carries: [`Solid::bottom`] is the `z` the static stood at, and
    /// [`Solid::roof`] is the flag. Nothing else in the walk asks either.
    /// # And where a cell's [`OwnerId`]s are handed out
    ///
    /// One per distinct [`Owner`] among the solids this cell *keeps*, counting
    /// from one in the order they come out — so a static the cutaway hid does
    /// not spend a number, and the numbering of a frame is a fact about that
    /// frame's own grid. At most [`MAX_SOLIDS_PER_CELL`] solids a cell means at
    /// most that many owners, which is exactly what a byte above zero holds.
    pub fn finish(self, cutaway: &Cutaway) -> Occlusion {
        let mut index = Vec::with_capacity(self.heads.len());
        let mut solids = Vec::with_capacity(self.arena.len());
        let mut owners: Vec<OwnerId> = Vec::with_capacity(self.arena.len());
        // The cell's owners in the order they were first seen — a `Vec` and a
        // scan rather than a map, because a cell holds two or three solids and
        // the loop is the frame's hot one.
        let mut seen: Vec<Owner> = Vec::new();
        for head in &self.heads {
            let offset = solids.len() as u32;
            // The list is built by pushing at the front, so walking it hands back
            // the newest first; reversing what one tile contributed is what puts
            // the solids in the order the map walk found them.
            let mut at = *head;
            while at != NO_SOLID {
                let (solid, next) = self.arena[at as usize];
                if solid.drawn(cutaway) {
                    solids.push(solid);
                }
                at = next;
            }
            solids[offset as usize..].reverse();
            seen.clear();
            for solid in &solids[offset as usize..] {
                let at = seen
                    .iter()
                    .position(|owner| *owner == solid.owner)
                    .unwrap_or_else(|| {
                        seen.push(solid.owner);
                        seen.len() - 1
                    });
                owners.push(OwnerId::nth(at));
            }
            index.push(Span {
                offset,
                count: (solids.len() as u32 - offset) as u8,
            });
        }
        let ids = (0..solids.len() as u32).map(SolidId::new).collect();
        Occlusion {
            bounds: self.bounds,
            index,
            ids,
            owners,
            solids,
            sky: self.sky,
            dropped: self.dropped,
        }
    }

    /// Where a tile lives in [`Builder::heads`].
    fn index(&self, x: i32, y: i32) -> Option<usize> {
        let bounds = self.bounds;
        if x < bounds.min_x || x > bounds.max_x || y < bounds.min_y || y > bounds.max_y {
            return None;
        }
        let (column, row) = (x - bounds.min_x, y - bounds.min_y);
        Some((row * bounds.width() + column) as usize)
    }
}

/// Everything on `bounds` that stands between a flame and the ground.
///
/// The same two sources the flames themselves come from — the map's statics and
/// the items the server has put on the ground — walked with the same bounds.
/// Both halves matter: a wall is a static, and **a door is an item**, sent by the
/// server and swapped for its open graphic when it is opened. A closed door that
/// let the light through would be the one occluder a player watches change.
///
/// Everything the map has goes into the builder; the frame's [`Cutaway`] is
/// applied once, by [`Builder::finish`], which is decision 33 and is what makes
/// everything above that line a fact about the map. What is refused here is the
/// draw ceiling, which is a fact about the static: a mountain top a hundred and
/// fifty `z` up is not drawn in any frame from any tile, so no frame wants it.
pub fn collect(
    map: &Map,
    items: &[GroundItem],
    bounds: TileBounds,
    tiledata: &TileData,
    cutaway: &Cutaway,
    atlas: Option<&crate::atlas::StaticAtlas>,
) -> Occlusion {
    let mut occlusion = Builder::new(bounds);

    crate::statics::for_each_static_in(map, bounds, |item| {
        place(
            &mut occlusion,
            map,
            tiledata,
            atlas,
            item.x,
            item.y,
            item.z,
            Graphic(item.tile),
        );
    });
    put_items(&mut occlusion, map, items, tiledata, atlas);

    occlusion.blur_sky();
    occlusion.finish(cutaway)
}

/// What one graphic standing at one place contributes to a grid: the sky it
/// takes, and the surfaces it is.
///
/// One function and not two lines written at every walk, because there are three
/// walks now — the map's statics, the server's ground items, and
/// [`bake`]'s block — and "the sky is shaded whatever the cutaway says, the
/// surfaces are refused above the draw ceiling" is the pair that has to be the
/// same in all of them.
///
/// The **draw ceiling** is refused here and the frame's [`Cutaway`] is not: a
/// mountain top a hundred and fifty `z` up is drawn in no frame from any tile, so
/// it is a fact about the static, while what the player is standing under is a
/// fact about the frame and belongs in [`Builder::finish`]. Decision 33.
// Eight: the grid, the three things a shape and a floor are looked up in, and
// the four that say which static this is. A struct for the last four would be
// `StaticItem` with the graphic already unwrapped — which is a fourth spelling of
// a thing that has three.
#[allow(clippy::too_many_arguments)]
fn place(
    grid: &mut Builder,
    map: &Map,
    tiledata: &TileData,
    atlas: Option<&crate::atlas::StaticAtlas>,
    x: u16,
    y: u16,
    z: i8,
    graphic: Graphic,
) {
    let tile = tiledata.static_tile(graphic.0);
    // The ground this tile's column is measured from. Off the map it is zero:
    // there is no floor there and nothing draws, and a static hanging over the
    // void still has to shade something rather than be skipped by an `unwrap`.
    let floor = map.land(x, y).map_or(0, |cell| cell.z);
    grid.shade(x, y, z, floor, graphic, tile);
    if cutaway::drawn_in_any_frame(z, tile) {
        grid.add(x, y, z, graphic, tile, shape_of(atlas, graphic));
    }
}

/// What the art said about one graphic, or the safe fallback where it said
/// nothing.
///
/// Which edge a wall stands on is measured once, when its picture is packed.
/// `None` for the whole atlas is a caller that has no pictures — a built scene, a
/// test — and every occluder is then the whole tile it was before
/// [`crate::facing`] existed. `None` for one graphic is the atlas not holding it,
/// which happens at the rim: the grid is grown by the widest pool's reach and the
/// atlas by what is drawn, and those are not the same rectangle. Both fall back
/// the same way.
///
/// The hole and the prism come off the same lookup and for the same reasons: a
/// graphic the atlas does not hold has neither, which is a solid wall, which is
/// what all but fifty-eight of the install's pictures are.
fn shape_of(atlas: Option<&crate::atlas::StaticAtlas>, graphic: Graphic) -> Shape {
    Shape {
        facing: atlas
            .and_then(|atlas| atlas.sprite(graphic))
            .and_then(|sprite| sprite.facing),
        hole: atlas.and_then(|atlas| atlas.hole(graphic)),
        prism: atlas.and_then(|atlas| atlas.prism(graphic)),
        blocks: crate::facing::Blocks::EMPTY,
    }
}

/// Put the server's ground items into a grid, after the map's own statics.
///
/// Never baked and always per frame: a door is a ground item, and a door
/// changing its graphic changes the one occluder a player watches change. There
/// are a handful of them in a frame against twenty-five thousand statics, so this
/// is the cheap half.
///
/// **After** the statics, which is what keeps a baked grid and a walked one the
/// same list: a tile's surfaces come out in the order they were added, and the
/// items of a tile that also holds a wall must land behind that wall in both.
fn put_items(
    grid: &mut Builder,
    map: &Map,
    items: &[GroundItem],
    tiledata: &TileData,
    atlas: Option<&crate::atlas::StaticAtlas>,
) {
    for item in items {
        place(
            grid,
            map,
            tiledata,
            atlas,
            item.at.x,
            item.at.y,
            item.at.z,
            item.graphic,
        );
    }
}

#[cfg(test)]
mod tests {
    /// A graphic in none of `crate::doors`' families, for the tests here that are
    /// about flags rather than about doors. Zero is below every family base.
    const NOT_A_DOOR: Graphic = Graphic(0);

    use openshard_protocol::wire::{Graphic, Hue};
    use openshard_protocol::world::Point;
    use openshard_uofiles::map::{LandCell, Map};

    use super::*;

    /// A static tile with the flags and height a test is about.
    fn tile(flags: u64, height: u8) -> StaticTile {
        StaticTile {
            flags: TileFlags::new(flags),
            height,
            ..StaticTile::default()
        }
    }

    /// One opaque solid as a test states one: the tile it stands on, the span it
    /// occupies, and its kind.
    ///
    /// The box comes from [`Solid::box_of`] rather than from six corners written
    /// out here, and that is the point: a test that spelled the corners itself
    /// would be a second opinion about where a panel's plane is, and the first
    /// one is the thing under test. What a scene is *about* is the four numbers.
    fn stands_at(x: i32, y: i32, bottom: i32, top: i32, edges: u8) -> Solid {
        Solid {
            space: Solid::box_of(x, y, bottom, top, edges),
            opacity: OPAQUE,
            edges,
            aperture: None,
            roof: false,
            // What a static standing at `bottom` would have been given. The
            // graphic is not a parameter because no test here is about telling
            // two graphics apart on one tile — the tests that are go through
            // `Builder::add`, which derives the key itself.
            owner: Owner::new(bottom as i8, Graphic(0)),
        }
    }

    /// An open door leaves the grid, and the graphic is the only thing that says
    /// so.
    ///
    /// The defect this is the fix for: `tiledata.mul` gives an open leaf the
    /// flags of its shut twin, so a door read by its flags alone lays a whole
    /// tile of wall across its own doorway — and decision 3's occluder being a
    /// tile makes that a band of shadow with nothing visible casting it. The
    /// pair below is the same `StaticTile` twice, which is the point: nothing in
    /// it differs, and the answers do.
    #[test]
    fn an_open_door_stops_nothing_and_its_shut_twin_stops_everything() {
        // `MetalDoor` facing 0, from `crate::doors` — and the flags the client
        // actually gives both of its leaves.
        let (shut, open) = (Graphic(0x0675), Graphic(0x0676));
        let leaf = tile(TileFlags::NO_SHOOT | TileFlags::BLOCK | TileFlags::WALL, 20);
        assert_eq!(opacity(shut, &leaf), OPAQUE, "a shut door is a wall");
        assert_eq!(opacity(open, &leaf), CLEAR, "an open door is a doorway");

        // And the grid keeps no cell for it, which is what the shadow walk reads.
        let mut occlusion = Builder::new(bounds());
        occlusion.add(100, 100, 0, shut, &leaf, Shape::UNREAD);
        occlusion.add(101, 100, 0, open, &leaf, Shape::UNREAD);
        let occlusion = occlusion.finish(&Cutaway::OPEN);
        assert!(occlusion.at(100, 100).is_some(), "the shut leaf left the grid");
        assert_eq!(
            occlusion.at(101, 100),
            None,
            "the open leaf is still a tile of wall across its own doorway",
        );

        // The sky too, and for the same reason: a doorway you can see through is
        // a doorway you can see the sky through. `shade` and `add` reading one
        // `opacity` is what keeps those two from drifting apart.
        let mut occlusion = Builder::new(bounds());
        occlusion.shade(101, 100, 0, 0, open, &leaf);
        assert_eq!(occlusion.sky_at(101, 100), SKY_OPEN, "an open door took the sky");
    }

    /// A rectangle big enough for a few tiles around the origin of a test.
    fn bounds() -> TileBounds {
        TileBounds {
            min_x: 100,
            max_x: 110,
            min_y: 100,
            max_y: 110,
        }
    }

    /// One tile's occluders are numbered from one, **one number per thing that
    /// was added** however many solids that thing turned into — and the number
    /// is what a drawn static can look itself up by.
    ///
    /// `docs/lighting_height.md` phase 3's first and third decisions together.
    /// The corner is the case that says "per added thing" rather than "per
    /// solid": it is one static, two panels, and a fragment of its picture is a
    /// fragment of both — so a numbering that counted solids would make the wall
    /// exempt from one of its own halves and not the other.
    #[test]
    fn a_cell_numbers_each_thing_added_once_however_many_solids_it_became() {
        let wall = tile(TileFlags::NO_SHOOT, 20);
        let corner = Shape {
            facing: Some(Facing::Corner {
                right: Face::East,
                left: Face::South,
            }),
            hole: None,
            prism: None,
            blocks: crate::facing::Blocks::EMPTY,
        };
        let (lower, upper) = (Graphic(0x0006), Graphic(0x0007));
        let mut occlusion = Builder::new(bounds());
        // A corner standing on the ground, and a second storey of the same
        // building above it — two things, four solids, one tile.
        occlusion.add(100, 100, 0, lower, &wall, corner);
        occlusion.add(100, 100, 20, upper, &wall, corner);
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        assert_eq!(occlusion.ids_at(100, 100).len(), 4, "two corners are four panels");
        let numbers: Vec<u8> = occlusion
            .owners_at(100, 100)
            .iter()
            .map(|owner| owner.raw())
            .collect();
        assert_eq!(
            numbers,
            vec![1, 1, 2, 2],
            "a corner's two panels are one owner, and the storey above it is a second",
        );
        // And the join, which is what a drawn static asks: the number for this
        // `(tile, z, graphic)`, not for the n-th solid of the cell.
        assert_eq!(occlusion.owner_at(100, 100, 0, lower).raw(), 1);
        assert_eq!(occlusion.owner_at(100, 100, 20, upper).raw(), 2);
        // The key is the *whole* key. A graphic at the wrong height and a height
        // with the wrong graphic are both misses, which is what keeps the
        // numbering from being "whatever is on this tile".
        assert_eq!(occlusion.owner_at(100, 100, 20, lower), OwnerId::NONE);
        assert_eq!(occlusion.owner_at(100, 100, 0, upper), OwnerId::NONE);
        assert_eq!(
            occlusion.owner_at(101, 100, 0, lower),
            OwnerId::NONE,
            "and a tile the static does not stand on",
        );
    }

    /// The numbering rides in the fourth channel of a **reference**, which is
    /// the one [`Occlusion::id_bytes`] left free until this had a reader.
    ///
    /// Pinned against the shader's own `id_at`, which reads that channel and
    /// which no Rust compiler checks. Zero would decode as [`OwnerId::NONE`] and
    /// exempt every fragment from nothing, which is a picture where every wall
    /// shadows its own face — the failure this format is the fix for, arriving
    /// as a silently-unwritten byte.
    #[test]
    fn the_reference_plane_carries_the_owner_beside_the_solid_it_names() {
        let wall = tile(TileFlags::NO_SHOOT, 20);
        let mut occlusion = Builder::new(bounds());
        occlusion.add(100, 100, 0, Graphic(0x0006), &wall, Shape::UNREAD);
        occlusion.add(100, 100, 0, Graphic(0x0007), &wall, Shape::UNREAD);
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        let bytes = occlusion.id_bytes();
        let owners: Vec<u8> = (0..occlusion.ids_at(100, 100).len())
            .map(|n| bytes[n * 4 + 3])
            .collect();
        assert_eq!(owners, vec![1, 2], "two bodies on one tile, two numbers");
        for (n, id) in occlusion.ids_at(100, 100).iter().enumerate() {
            assert_eq!(
                u32::from(bytes[n * 4])
                    | u32::from(bytes[n * 4 + 1]) << 8
                    | u32::from(bytes[n * 4 + 2]) << 16,
                id.raw(),
                "the owner byte displaced part of the id it stands beside",
            );
        }
    }

    /// A flight of steps is **one** occluder of its tile, however many treads
    /// the art was fitted into.
    ///
    /// The case `docs/lighting_height.md` phase 3's own "one static, several
    /// solids" is about, and the one where a per-solid numbering would be
    /// visibly wrong rather than merely arguable: a tread's own top would be a
    /// different occluder from the riser under it, so a fragment of the flight
    /// would be shadowed by the rest of the flight it is part of.
    #[test]
    fn a_flight_of_steps_is_one_owner_and_not_one_per_tread() {
        let stair = tile(TileFlags::NO_SHOOT | TileFlags::CLIMBABLE, 10);
        let prism = crate::facing::Prism::new(Face::North, &[3, 3, 3]).expect("three treads");
        let mut occlusion = Builder::new(bounds());
        occlusion.add(100, 100, 0, Graphic(0x0736), &stair, Shape::solid(prism));
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        assert_eq!(
            occlusion.ids_at(100, 100).len(),
            6,
            "three treads are a lid and a riser each",
        );
        assert!(
            occlusion.owners_at(100, 100).iter().all(|owner| owner.raw() == 1),
            "the flight's own faces are not one occluder: {:?}",
            occlusion.owners_at(100, 100),
        );
    }

    /// [`OwnerId::NONE`] matches nothing, **including another `NONE`**.
    ///
    /// The property the whole exemption rests on: the ground, a mobile and a
    /// static the grid refused all stamp it, and two of them meeting must not
    /// read as one thing meeting itself. Stated here because `==` on the newtype
    /// would answer the other way and still compile.
    #[test]
    fn no_owner_is_not_the_same_owner_as_no_owner() {
        assert!(!OwnerId::NONE.same(OwnerId::NONE));
        assert!(!OwnerId::nth(0).same(OwnerId::NONE));
        assert!(!OwnerId::NONE.same(OwnerId::nth(0)));
        assert!(OwnerId::nth(0).same(OwnerId::nth(0)));
        assert!(!OwnerId::nth(0).same(OwnerId::nth(1)));
        // And the numbering never produces it, which is what makes the above a
        // fact about the grid rather than about the constant alone.
        assert_ne!(OwnerId::nth(0), OwnerId::NONE);
        assert_eq!(OwnerId::nth(0).raw(), 1, "the first owner of a cell is one");
    }

    /// The rule, said in every direction that matters. A wall stops light; a
    /// pane dims it; a barrel, which is `BLOCK` and nothing else, does not touch
    /// it. Reading impassability instead of the shooting flags is the mistake
    /// this was written against — it would put a shadow behind every crate on
    /// the street — and treating a window as a wall is the one beside it, which
    /// makes a lit room invisible from the road.
    #[test]
    fn a_wall_stops_light_a_pane_dims_it_and_a_barrel_does_not() {
        assert_eq!(opacity(NOT_A_DOOR, &tile(TileFlags::NO_SHOOT, 20)), OPAQUE);
        assert_eq!(opacity(NOT_A_DOOR, &tile(TileFlags::WINDOW, 20)), PANE);
        assert_eq!(opacity(NOT_A_DOOR, &tile(TileFlags::BLOCK, 10)), CLEAR);
        // A real wall carries both, and the rule must not need the pair.
        assert_eq!(
            opacity(NOT_A_DOOR, &tile(TileFlags::NO_SHOOT | TileFlags::BLOCK, 20)),
            OPAQUE
        );
        // And a static flagged as both a window and solid is the solid one: the
        // union darkens, which is the direction that cannot leak a room.
        assert_eq!(
            opacity(NOT_A_DOOR, &tile(TileFlags::NO_SHOOT | TileFlags::WINDOW, 20)),
            OPAQUE
        );
        const { assert!(PANE > CLEAR && PANE < OPAQUE, "a pane is neither open nor a wall") };
    }

    /// A wall occupies the heights it occupies, and the grid says which.
    #[test]
    fn a_wall_carries_the_span_it_stands_in() {
        let mut occlusion = Builder::new(bounds());
        occlusion.add(
            102,
            103,
            5,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT, 20),
            Shape::UNREAD,
        );
        let occlusion = occlusion.finish(&Cutaway::OPEN);
        assert_eq!(
            occlusion.at(102, 103),
            Some(Cell {
                bottom: 5,
                top: 25,
                opacity: OPAQUE,
                edges: EDGE_ANY,
            })
        );
        assert_eq!(occlusion.at(103, 103), None, "its neighbour is open ground");
    }

    /// A corner stands on **two** of its tile's sides, and on the other two it
    /// stands on nothing.
    ///
    /// The grid's half of decision 25. Two bits and not four is what the walk
    /// reads as a panel rather than as a body — see the `edges` arm of
    /// `light::walk_cells` — so a ray crossing the sides the corner does
    /// not stand on passes, exactly as it does beside the runs of wall either
    /// side of it. Before this every corner in the world was `EDGE_ANY`.
    #[test]
    fn a_corner_stands_on_the_two_sides_its_art_named() {
        use crate::facing::{Face, Facing};

        let corner = Facing::Corner {
            right: Face::East,
            left: Face::South,
        };
        assert_eq!(edges_of(Some(corner)), EDGE_EAST | EDGE_SOUTH);
        // And each of the four pairings, so that a mask built from the right
        // half's answer twice would be caught.
        assert_eq!(
            edges_of(Some(Facing::Corner {
                right: Face::North,
                left: Face::West
            })),
            EDGE_NORTH | EDGE_WEST,
        );
        // A plain wall is still one side, and a graphic nothing measured is still
        // the whole tile: neither of those moved.
        assert_eq!(edges_of(Some(Facing::One(Face::South))), EDGE_SOUTH);
        assert_eq!(edges_of(None), EDGE_ANY);

        let mut occlusion = Builder::new(bounds());
        occlusion.add(
            102,
            103,
            0,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT, 20),
            Shape::faced(corner),
        );
        let occlusion = occlusion.finish(&Cutaway::OPEN);
        assert_eq!(
            occlusion.at(102, 103).unwrap().edges,
            EDGE_EAST | EDGE_SOUTH,
            "the cell did not take the corner's two sides",
        );
        // And in the list it is **two solids**, which is the shape decision 30
        // gives a corner: one plane a side, each with the tile's own span. The
        // merged view above is a fold over exactly these.
        assert_eq!(
            occlusion.solids_at(102, 103).copied().collect::<Vec<_>>(),
            [
                stands_at(102, 103, 0, 20, EDGE_EAST),
                stands_at(102, 103, 0, 20, EDGE_SOUTH),
            ],
        );
    }

    /// What a cell is in the list: a lid is one horizontal, a body is one solid,
    /// and a named mask is a quad a side.
    ///
    /// The claim that makes step 21.1 a change of storage and nothing else. A
    /// cell is never a mixture of the two kinds — that is the union in `add`
    /// talking — and the walk's two rules are exactly these two kinds, so a
    /// surface list built any other way would move the picture.
    #[test]
    fn a_cell_becomes_the_surfaces_it_always_meant() {
        let mut occlusion = Builder::new(bounds());
        // A floor: a lid, and one surface naming no side.
        occlusion.add(
            100,
            100,
            10,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT | TileFlags::FLOOR, 0),
            Shape::UNREAD,
        );
        // A graphic nothing measured: a body, and one surface on all four sides
        // rather than four quads. The walk travels *through* it, which is a rule
        // about a solid and not about four planes.
        occlusion.add(
            101,
            100,
            0,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT, 20),
            Shape::UNREAD,
        );
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        assert_eq!(
            occlusion.solids_at(100, 100).copied().collect::<Vec<_>>(),
            [stands_at(100, 100, 10, 10, 0)],
        );
        assert_eq!(
            occlusion.solids_at(101, 100).copied().collect::<Vec<_>>(),
            [stands_at(101, 100, 0, 20, EDGE_ANY)],
        );
        assert_eq!(occlusion.ids_at(102, 100), &[], "open ground stands nothing");
        assert_eq!(occlusion.ids_at(0, 0), &[], "and neither does off the grid");
        assert_eq!(occlusion.solid_count(), 2, "and nothing else got into the list");
        assert_eq!(
            occlusion.reference_count(),
            2,
            "and each of them is referenced by the one cell it stands on",
        );
    }

    /// A hole belongs to a **named panel** and to nothing else, and what is
    /// offered to a lid or a body is dropped.
    ///
    /// Step 21.3's own refusal, and it is the same one decision 3 makes about the
    /// edge itself. A hole is a rectangle in a plane, stated in the run of the
    /// side the surface stands on — so a lid, which is horizontal, and a body,
    /// which is "it stands up and the art would not say which way", have no
    /// coordinate for a `near` and a `far` to mean anything in. Storing one
    /// anyway would put a window in a direction nobody measured.
    ///
    /// And a **corner** carries it on both of its panels: the two are the two
    /// faces of one picture, a hole measured off that picture is the same window
    /// seen from either side of the tile, and nothing in a silhouette says which
    /// half it belonged to.
    #[test]
    fn only_a_named_panel_carries_a_hole() {
        use crate::facing::{Face, Facing};

        // Measured off the picture, so it is a height above the static's base —
        // and every static here stands at `z = 0`, which is what makes the
        // placed `Aperture` below the same four numbers.
        let hole = Hole {
            near: 64,
            far: 191,
            bottom: 0,
            top: 10,
        };
        let placed = Aperture::above(0, hole);
        let wall = tile(TileFlags::NO_SHOOT, 20);
        let mut occlusion = Builder::new(bounds());
        // A named panel keeps it.
        occlusion.add(
            100,
            100,
            0,
            NOT_A_DOOR,
            &wall,
            Shape {
                facing: Some(Facing::One(Face::South)),
                hole: Some(hole),
                prism: None,
                blocks: crate::facing::Blocks::EMPTY,
            },
        );
        // A graphic the art would not name is a body, and drops it.
        occlusion.add(
            101,
            100,
            0,
            NOT_A_DOOR,
            &wall,
            Shape {
                facing: None,
                hole: Some(hole),
                prism: None,
                blocks: crate::facing::Blocks::EMPTY,
            },
        );
        // And a floor is a lid, whatever its silhouette read as.
        occlusion.add(
            102,
            100,
            0,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT | TileFlags::FLOOR, 0),
            Shape {
                facing: Some(Facing::One(Face::South)),
                hole: Some(hole),
                prism: None,
                blocks: crate::facing::Blocks::EMPTY,
            },
        );
        // A corner is two panels and the hole is on both.
        occlusion.add(
            103,
            100,
            0,
            NOT_A_DOOR,
            &wall,
            Shape {
                facing: Some(Facing::Corner {
                    right: Face::East,
                    left: Face::South,
                }),
                hole: Some(hole),
                prism: None,
                blocks: crate::facing::Blocks::EMPTY,
            },
        );
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        let aperture_at = |x, y| {
            occlusion
                .solids_at(x, y)
                .map(|solid| solid.aperture)
                .collect::<Vec<_>>()
        };
        assert_eq!(aperture_at(100, 100)[0], Some(placed));
        assert_eq!(
            aperture_at(101, 100)[0],
            None,
            "a body kept a hole in a plane it does not have",
        );
        assert_eq!(
            aperture_at(102, 100)[0],
            None,
            "a lid kept a hole in a plane it does not have",
        );
        let corner = aperture_at(103, 100);
        assert_eq!(corner.len(), 2);
        assert!(
            corner.iter().all(|hole| *hole == Some(placed)),
            "a corner's two faces are one picture and the hole is in both of them",
        );
    }

    /// The two planes are one list: a hole is at the same index its surface is,
    /// and the `HOLED` bit is what says to look.
    ///
    /// The format, pinned, because it is the one thing here no picture can catch:
    /// a shader reading the hole plane at the wrong index would draw *something*
    /// everywhere and be wrong only where a window is. The bit matters as much as
    /// the bytes — a surface with no hole writes four zeros, which is a hole of no
    /// width at `z = -128`, and only the bit distinguishes that from a real one.
    #[test]
    fn a_hole_is_uploaded_at_its_own_surface_s_index() {
        use crate::facing::{Face, Facing};

        let wall = tile(TileFlags::NO_SHOOT, 20);
        let mut occlusion = Builder::new(bounds());
        // A solid panel first, so that the holed one is not at index zero and an
        // upload that ignored the index would be caught.
        occlusion.add(
            100,
            100,
            0,
            NOT_A_DOOR,
            &wall,
            Shape::faced(Facing::One(Face::South)),
        );
        occlusion.add(
            101,
            100,
            5,
            NOT_A_DOOR,
            &wall,
            Shape {
                facing: Some(Facing::One(Face::East)),
                // Measured a `z` above the picture's base and a nine, on a
                // static standing at five — so the bytes below are the placed
                // rectangle and not the measured one. A conversion that had
                // dropped the base would pass every test that stood its walls on
                // the ground.
                hole: Some(Hole {
                    near: 64,
                    far: 191,
                    bottom: 1,
                    top: 9,
                }),
                prism: None,
                blocks: crate::facing::Blocks::EMPTY,
            },
        );
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        assert!(occlusion.any_aperture());
        let surfaces = occlusion.solid_bytes();
        let holes = occlusion.aperture_bytes();
        assert_eq!(
            surfaces[3] & HOLED,
            0,
            "the solid panel claims a hole, so the shader will read one",
        );
        assert_eq!(&holes[0..4], &[0, 0, 0, 0], "and it has no bytes to read");
        assert_eq!(surfaces[7] & HOLED, HOLED, "the holed panel does not claim one");
        assert_eq!(
            &holes[4..8],
            &[64, 191, 6 + 128, 14 + 128],
            "the hole is not where its surface is",
        );

        // And a grid with no hole in it says so, which is what keeps the plane
        // off the queue on every frame of a map that has none.
        let mut solid = Builder::new(bounds());
        solid.add(
            100,
            100,
            0,
            NOT_A_DOOR,
            &wall,
            Shape::faced(Facing::One(Face::South)),
        );
        assert!(!solid.finish(&Cutaway::OPEN).any_aperture());
    }

    /// **Every solid the builder makes comes back off its own wire where it
    /// was put** — [`Solid::fraction`] out, [`Solid::box_from_footprint`] back,
    /// against the cell the grid really holds it on.
    ///
    /// A round trip and not a spot check, because the class this closes is one
    /// nothing else looks at. `light::walk_cells_exact` reads `space` directly
    /// and is right whatever the bytes say; `light::walk_cells_streaming` and
    /// the shader read only these bytes, and the two walks' agreement proptests
    /// build their panels with [`Solid::box_of`], whose slab is
    /// [`PANEL_THICKNESS`] deep — never a plane, so never able to pose the
    /// question at all. What can is a climbable: `Solid::tread_riser_box_of`
    /// makes a plane, and the first riser's plane sits on its own tile's *far*
    /// edge, at a whole coordinate that `floor` reads as the next tile along.
    ///
    /// All four climb directions, because two of them (`North`'s `y + 1`,
    /// `West`'s `x + 1`) put that plane on the far edge and two put it on the
    /// near one, and a rule that fixed the far case by moving every plane would
    /// break the near one silently.
    #[test]
    fn every_solid_comes_back_off_the_wire_on_the_cell_it_was_put_on() {
        use crate::facing::{Face, Prism};

        let stair = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT | TileFlags::CLIMBABLE),
            height: 20,
            ..StaticTile::default()
        };
        for up in [Face::North, Face::East, Face::South, Face::West] {
            let prism = Prism::new(up, &[1, 3, 5]).expect("three treads");
            let mut builder = Builder::new(bounds());
            builder.add(100, 100, 0, Graphic(0x0736), &stair, Shape::solid(prism));
            let occlusion = builder.finish(&Cutaway::OPEN);
            let solids: Vec<&Solid> = occlusion.solids_at(100, 100).collect();
            assert_eq!(
                solids.len(),
                6,
                "{up:?}: three treads is three tops and three risers"
            );
            for solid in solids {
                let (low, high) = (solid.low(), solid.high());
                let wire = Solid::box_from_footprint(100, 100, low, high, solid.fraction());
                for (mine, theirs, axis) in [
                    (solid.space.min.x, wire.min.x, "min.x"),
                    (solid.space.max.x, wire.max.x, "max.x"),
                    (solid.space.min.y, wire.min.y, "min.y"),
                    (solid.space.max.y, wire.max.y, "max.y"),
                ] {
                    assert!(
                        (mine - theirs).abs() < 1.0 / 255.0,
                        "{up:?}: a solid with edges {:#06b} at {:?}..{:?} came back with {axis} \
                         {theirs} instead of {mine}",
                        solid.edges,
                        solid.space.min,
                        solid.space.max,
                    );
                }
            }
        }
    }

    /// Stairs count as half their height, the way every other reader of this
    /// field here does. A stair that occluded its full height would shadow the
    /// landing it leads to.
    #[test]
    fn a_climbable_static_occludes_half_its_height() {
        let mut occlusion = Builder::new(bounds());
        occlusion.add(
            100,
            100,
            0,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT | TileFlags::CLIMBABLE, 20),
            Shape::UNREAD,
        );
        assert_eq!(occlusion.finish(&Cutaway::OPEN).at(100, 100).unwrap().top, 10);
    }

    /// **A staircase is treads, not a corner of a house and not a ziggurat.**
    ///
    /// The defect this is the fix for, in the terms the grid sees it: a stair's
    /// base is two 45° runs meeting at the tile's south corner, `facing_of` reads
    /// that as `Corner { East, South }` — the same verdict it reaches about two
    /// walls meeting — and the grid stood two opaque panels on the tile's east and
    /// south edges. A flight of steps then shadowed the street like a run of wall.
    /// Step 23.1 fixed that far enough to make one whole-tile body of it, which
    /// step 23.0's own picture then found the next defect in: nine of those,
    /// stacked to the landing's height, is a ziggurat rather than a stair. Step
    /// 23.5 is this test's second half.
    ///
    /// **Step 23.5's second half is this test's history; gbuffer.md step 4b is
    /// what changed since.** A tread used to be one body a ray travels through,
    /// asserted here as `edges == EDGE_ANY`; it is now two faces, a lid for its
    /// top and a panel for the riser below it — decision 3's "seven honest
    /// normals" for this fixture's three-tread stair (six here; the seventh, a
    /// lid static's own top, is a different graphic this test does not build).
    ///
    /// Four things asserted, one per tread. The **shape**: a thin lid at the
    /// tread's own height (`top() == bottom()`), and a panel spanning the rise
    /// from the tread before it. The **height**: each tread's own, off the
    /// picture, where the tile's own field says twenty — which is what a
    /// climbable static's `height` means about half the time. The **footprint**:
    /// each top is its own strip of the tile, climbing west, so the low tread is
    /// the strip nearest east and the high one nearest west — and each riser is
    /// degenerate on that same axis, a plane and not a strip. And the **facing**:
    /// a riser's named edge is `up`'s opposite, `EDGE_EAST` for a climb west,
    /// which is also `Solid::footprint`'s `far` case — exercised here rather
    /// than assumed, since a riser past the first tread sits at a fraction of
    /// the tile, not its true edge (see `footprint`'s own doc). See
    /// `Builder::add`, and `docs/lighting.md`'s backlog.
    #[test]
    fn a_stair_is_two_faces_per_tread_and_each_ones_height_comes_off_the_art() {
        use crate::facing::{Face, Facing, Prism};

        let stair = tile(TileFlags::NO_SHOOT | TileFlags::CLIMBABLE, 20);
        let read_as = |shape| {
            let mut occlusion = Builder::new(bounds());
            occlusion.add(100, 100, 0, NOT_A_DOOR, &stair, shape);
            let finished = occlusion.finish(&Cutaway::OPEN);
            finished.solids_at(100, 100).copied().collect::<Vec<_>>()
        };

        // What the art actually says about `0x0736`: three treads climbing west,
        // one to five `z` — measured in `tests/prism.rs` against the real sprite.
        let prism = Prism::new(Face::West, &[1, 3, 5]).expect("three treads");
        let solids = read_as(Shape::solid(prism));
        assert_eq!(solids.len(), 6, "a top and a riser per tread, not one body");

        let (mut tops, mut risers): (Vec<_>, Vec<_>) = solids.into_iter().partition(|solid| solid.edges == 0);
        assert_eq!(tops.len(), 3, "one lid per tread");
        assert_eq!(risers.len(), 3, "one panel per tread");
        for riser in &risers {
            assert_eq!(
                riser.edges, EDGE_EAST,
                "a riser faces away from `up` (West), which is East"
            );
        }

        tops.sort_by_key(|solid| solid.top());
        let heights: Vec<i32> = tops.iter().map(|solid| solid.top()).collect();
        assert_eq!(
            heights,
            vec![1, 3, 5],
            "each tread's own height, not the tallest for all three"
        );
        for top in &tops {
            assert_eq!(top.top(), top.bottom(), "a top is a plane, not a body");
        }
        // West is a strip of `x`, one third of the tile wide each, and the low
        // tread (height 1) is nearest east — the far side from the climb's `up`.
        let top_by_height = |h: i32| tops.iter().find(|solid| solid.top() == h).unwrap();
        assert!(
            top_by_height(1).space.min.x > top_by_height(5).space.min.x,
            "the lowest tread's strip is nearest east, the highest nearest west",
        );

        // A riser is a plane on the climb axis: `min.x == max.x`, not a strip.
        // Sorted by height, its `bottom()`/`top()` is the rise from the tread
        // before it — or from the static's own base, for the first.
        risers.sort_by_key(|solid| solid.top());
        for riser in &risers {
            assert_eq!(
                riser.space.min.x, riser.space.max.x,
                "a riser is a plane, not a strip"
            );
        }
        let rises: Vec<(i32, i32)> = risers.iter().map(|solid| (solid.bottom(), solid.top())).collect();
        assert_eq!(
            rises,
            vec![(0, 1), (1, 3), (3, 5)],
            "each riser spans the rise from the tread before it, or the ground for the first"
        );

        // And with no prism measured, it is `CLIMBABLE` that decides rather
        // than the facing the art happened to read as: one whole-tile body,
        // not the wall detector's two `PANEL_THICKNESS`-inset panels a corner
        // reading used to fall back to — that reading is exactly the "flight
        // of stairs shadows a street like a run of wall" defect this static's
        // own doc comment names, and it is what left the same seam on every
        // stair the prism search does not fit. Half the stated height either
        // way, because `calc_height` halves it before either branch sees it.
        let corner = read_as(Shape::faced(Facing::Corner {
            right: Face::East,
            left: Face::South,
        }));
        assert_eq!(corner.len(), 1, "one whole-tile body, not two panels");
        assert_eq!(corner[0].edges, EDGE_ANY, "a body, not a named-edge panel");
        assert_eq!(corner[0].top(), 10, "still half of what tiledata states");
        assert_eq!(
            (corner[0].space.min.x, corner[0].space.max.x),
            (100.0, 101.0),
            "full width, not a panel narrowed by PANEL_THICKNESS"
        );
    }

    /// [`Solid::footprint`]'s `-1` adjustment for a "far"-edged degenerate plane
    /// is only correct where that plane sits exactly on the tile's true
    /// boundary — every panel [`Solid::box_of`] builds does, but
    /// [`Solid::tread_riser_box_of`] does not for any tread past the first: its
    /// boundary is a fraction of the tile (`index / count`), and the old,
    /// unconditional `-1` walked it into the neighbouring tile.
    ///
    /// Built directly off `tread_riser_box_of` rather than through `Builder::add`
    /// so the assertion is about `footprint` alone, not the whole climbable
    /// path the test above already covers.
    #[test]
    fn a_mid_flight_risers_footprint_stays_on_its_own_tile() {
        use crate::facing::Face;

        let space = Solid::tread_riser_box_of(100, 100, 1, 3, Face::West, 1, 3);
        let riser = Solid {
            space,
            opacity: OPAQUE,
            edges: opposite(edge_of(Face::West)),
            aperture: None,
            roof: false,
            owner: Owner::new(1, Graphic(0)),
        };
        assert_eq!(
            riser.footprint(),
            (100..=100, 100..=100),
            "a mid-flight riser's fractional boundary is not the tile's true edge"
        );
    }

    /// Two occluders on one tile are **two surfaces**, and the gap between them
    /// stays open.
    ///
    /// Step 21.2, and the one place in decision 30 where the picture had to move.
    /// The union used to take the widest span over everything on a tile, so a wall
    /// at `0..=20` and another at `40..=60` came out as one wall from 0 to 60 and
    /// closed twenty `z` of air — which is a foot of shadow with nothing casting
    /// it, and the same union leaked the *sides* in the other direction.
    ///
    /// The merged view still folds them, and that is deliberate: the readers whose
    /// question is about a tile — the wireframe, the plan view, which way a
    /// mounted flame steps out of its own cell — get the same answer they always
    /// did. What changed is what the *walk* reads.
    #[test]
    fn two_occluders_on_one_tile_stop_closing_the_gap_between_them() {
        let mut occlusion = Builder::new(bounds());
        occlusion.add(
            105,
            105,
            0,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT, 20),
            Shape::UNREAD,
        );
        occlusion.add(
            105,
            105,
            40,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT, 20),
            Shape::UNREAD,
        );
        let occlusion = occlusion.finish(&Cutaway::OPEN);
        assert_eq!(
            occlusion.solids_at(105, 105).copied().collect::<Vec<_>>(),
            [
                // Neither was given a face, so both are the whole tile.
                stands_at(105, 105, 0, 20, EDGE_ANY),
                stands_at(105, 105, 40, 60, EDGE_ANY),
            ],
            "the two walls merged into one, and the air between them with it",
        );
        assert_eq!(
            occlusion.at(105, 105),
            Some(Cell {
                bottom: 0,
                top: 60,
                opacity: OPAQUE,
                edges: EDGE_ANY,
            }),
            "the merged view is what it always was",
        );
    }

    /// A lid and a panel on one tile keep their own spans, their own opacities
    /// and their own **rules**.
    ///
    /// The other half of step 21.2, and the backlog entry it closes: `add` used to
    /// union everything on a tile, so a floor over a wall tile contributed its `z`
    /// to the span and *lost its own lid-ness* — the merged mask was the wall's,
    /// so the walk pierced the floor as though it were a vertical panel and
    /// travelled through nothing. Conservative in the direction that darkens for
    /// the span and in the direction that leaks for the sides, which is not one
    /// direction.
    ///
    /// The two rules are the two masks, so this is also what says the walk sees
    /// them: a lid names no side and is travelled through, a panel names one and
    /// is pierced. See `light::walk_cells`.
    #[test]
    fn a_lid_and_a_panel_on_one_tile_are_not_one_surface() {
        use crate::facing::{Face, Facing};

        let mut occlusion = Builder::new(bounds());
        // A wall on the south side of its tile, twenty `z` tall.
        occlusion.add(
            104,
            104,
            0,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT | TileFlags::WALL, 20),
            Shape::faced(Facing::One(Face::South)),
        );
        // And a glazed roof lying across the same tile at the top of it.
        occlusion.add(
            104,
            104,
            20,
            NOT_A_DOOR,
            &tile(TileFlags::WINDOW | TileFlags::FLOOR, 0),
            Shape::UNREAD,
        );
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        assert_eq!(
            occlusion.solids_at(104, 104).copied().collect::<Vec<_>>(),
            [
                stands_at(104, 104, 0, 20, EDGE_SOUTH),
                Solid {
                    opacity: PANE,
                    ..stands_at(104, 104, 20, 20, 0)
                },
            ],
        );
        // The union's three separate wrongnesses, each of which the merged view
        // still has and the walk no longer reads: the pane was opaque, the lid
        // was a south panel, and the roof's `z` was part of the wall's span.
        assert_eq!(
            occlusion.at(104, 104),
            Some(Cell {
                bottom: 0,
                top: 20,
                opacity: OPAQUE,
                edges: EDGE_SOUTH,
            }),
        );
    }

    /// **A panel *is* the plane a pixel of that face lies on**, and the plane is
    /// derived from [`crate::facing::Face::place_at`] rather than restated here.
    ///
    /// The instrument's own honesty, and the failure it is aimed at is quiet: this
    /// box is what both views draw *and*, since step 23.1, what the world owns, so
    /// a panel on the wrong edge of its tile would look like a wall standing a tile
    /// out of place — which reads as a defect in the *map* rather than in the
    /// picture of it. An instrument that can be wrong in a way indistinguishable
    /// from what it is measuring is worse than no instrument.
    ///
    /// `place_at` is the right other end of the claim because it is what
    /// `statics.wgsl` places a face fragment with: the point it hands back is
    /// where that pixel *is*, and the axis that does not move along the run is
    /// the plane the whole face lies in. That is the plane `light::walk_cells`
    /// pierces when the ray crosses this edge, so a box lying in it is a box the
    /// shader agrees with.
    ///
    /// **Since step 23.5 the record itself carries the thickness**, so there is
    /// one claim rather than two: the *outer* face of the box is the plane
    /// [`crate::facing::Face::place_at`] draws a pixel on, and the inner one is
    /// [`PANEL_THICKNESS`] further in, away from
    /// [`crate::facing::Face::outward`] — never straddling the edge, because two
    /// neighbouring walls drawing one inside the other would make an honest
    /// joint look like a doubled wall. `solid::drawn` is checked here too, and
    /// what it must show is that it no longer moves a panel at all: the record
    /// already is the picture.
    #[test]
    fn a_panel_lies_in_the_plane_its_face_pixels_lie_on() {
        use crate::facing::{Face, Facing};

        let (x, y) = (1500, 1600);
        for face in [Face::North, Face::East, Face::South, Face::West] {
            let stands = Solid {
                edges: edges_of(Some(Facing::One(face))),
                ..stands_at(x, y, 0, 20, edges_of(Some(Facing::One(face))))
            };
            let solid = stands.space;
            // Where the two ends of this face's run are, in the tile's own unit
            // square. The axis they agree on is the one the face is flat in.
            let (near, far) = (face.place_at(0.0), face.place_at(1.0));
            let flat_in_x = (near.0 - far.0).abs() < 1e-6;
            let (axis, plane) = match flat_in_x {
                true => (0, f64::from(x) + f64::from(near.0)),
                false => (1, f64::from(y) + f64::from(near.1)),
            };
            let (min, max) = ([solid.min.x, solid.min.y][axis], [solid.max.x, solid.max.y][axis]);
            // Which of the box's two faces on this axis is the outer one is the
            // face's own outward direction: the same fact `PANEL_THICKNESS`'s
            // own doc argues, now checked on the record rather than the view.
            let outward = f64::from(face.outward()[axis]);
            assert!(
                outward != 0.0,
                "{face:?} does not face along the axis it is flat in"
            );
            let (outer, inner) = match outward > 0.0 {
                true => (max, min),
                false => (min, max),
            };
            assert!(
                (outer - plane).abs() < 1e-9,
                "{face:?}: the outer face should be the plane at {plane}, it is {outer}",
            );
            assert!(
                ((inner - outer) * outward + PANEL_THICKNESS).abs() < 1e-9,
                "{face:?}: the slab should lie {PANEL_THICKNESS} inside its tile, it lies \
                 {} from {outer} to {inner}",
                inner - outer,
            );
            // And across the run it is the whole tile, because a run of wall is
            // one surface: a panel short of its own edge would leave a hairline
            // at every join, which is the class decision 38 exists to kill.
            let along = 1 - axis;
            let (from, to) = (
                [solid.min.x, solid.min.y][along],
                [solid.max.x, solid.max.y][along],
            );
            let corner = f64::from([x, y][along]);
            assert!(
                (from - corner).abs() < 1e-9 && (to - corner - 1.0).abs() < 1e-9,
                "{face:?}: the run should span the whole tile, it spans {from}..{to}",
            );
            assert!(
                (solid.min.z - f64::from(stands.bottom())).abs() < 1e-9
                    && (solid.max.z - f64::from(stands.top())).abs() < 1e-9,
                "{face:?}: the span held is not the span the walk tests",
            );

            // And the drawing: since the record already is a slab, `drawn`
            // leaves a panel's `x` and `y` exactly as they stand.
            let picture = crate::solid::drawn(&stands);
            assert_eq!(
                (picture.min.x, picture.min.y, picture.max.x, picture.max.y),
                (solid.min.x, solid.min.y, solid.max.x, solid.max.y),
                "{face:?}: a panel's box is already the picture, `drawn` moved it",
            );
        }
    }

    /// And the other two kinds: a lid is the plane a ray crosses it at and is
    /// drawn hanging under it, and a body is exactly its tile in both.
    ///
    /// The companion to the test above, and the same argument. A lid is *stopped
    /// at* by [`crate::light`]'s `crosses`, which asks whether the ray got to the
    /// other side of the span — so what a person has to be able to see is the
    /// top, and a slab drawn hanging *above* its own plane would put a floor at
    /// the height of the storey over it. A body is travelled through and its
    /// extent is the tile, which is the one kind that is a box in both.
    #[test]
    fn a_lid_is_drawn_hanging_under_its_plane_and_a_body_is_its_tile() {
        use crate::solid::{DRAWN_LID_THICKNESS, drawn};

        let (x, y) = (1500, 1600);
        // A floor as the map has one: a lid whose span is the height it lies at,
        // which is what `calc_height` gives a `FLOOR` static of no height.
        let flat = stands_at(x, y, 20, 20, 0);
        assert!(
            (flat.space.min.z - 20.0).abs() < 1e-9 && (flat.space.max.z - 20.0).abs() < 1e-9,
            "a lid the world owns is the plane it lies in, it is {:?}",
            flat.space,
        );
        let lid = drawn(&flat);
        assert!(
            (lid.max.z - 20.0).abs() < 1e-9 && (lid.min.z - (20.0 - DRAWN_LID_THICKNESS)).abs() < 1e-9,
            "a lid's top is its plane and it is drawn hanging under it, it is {lid:?}",
        );
        let body = drawn(&stands_at(x, y, 0, 20, EDGE_ANY));
        assert!(
            (body.min.x - f64::from(x)).abs() < 1e-9
                && (body.max.x - f64::from(x) - 1.0).abs() < 1e-9
                && (body.min.y - f64::from(y)).abs() < 1e-9
                && (body.max.y - f64::from(y) - 1.0).abs() < 1e-9
                && (body.min.z).abs() < 1e-9
                && (body.max.z - 20.0).abs() < 1e-9,
            "a body is its whole tile and its whole span, it is {body:?}",
        );
        // Both of them across the whole tile, which is what tells a lid from a
        // panel on screen: a panel is a ribbon, a lid is the tile.
        assert!(
            (lid.min.x - f64::from(x)).abs() < 1e-9 && (lid.max.x - f64::from(x) - 1.0).abs() < 1e-9,
            "a lid should cover its tile, it is {lid:?}",
        );
    }

    /// The same surface twice is one surface.
    ///
    /// Not a merge — the spans are identical, so nothing is being made
    /// conservative — but it is what keeps a tile carrying five copies of one
    /// wall graphic from spending five slots of a count that is one byte wide.
    /// Decision 30.6 is about to measure that count, and a distribution padded
    /// with duplicates would name the wrong bound.
    #[test]
    fn the_same_surface_twice_is_stored_once() {
        let mut occlusion = Builder::new(bounds());
        let wall = tile(TileFlags::NO_SHOOT, 20);
        occlusion.add(106, 106, 0, NOT_A_DOOR, &wall, Shape::UNREAD);
        occlusion.add(106, 106, 0, NOT_A_DOOR, &wall, Shape::UNREAD);
        let occlusion = occlusion.finish(&Cutaway::OPEN);
        assert_eq!(occlusion.solid_count(), 1);
        assert_eq!(occlusion.dropped(), 0, "a duplicate is not a truncation");
    }

    /// A tile past the format's ceiling drops what does not fit — and **counts
    /// it**.
    ///
    /// Decision 30.6: a grid that quietly truncates reads as "covered everything"
    /// when it did not. The count is one byte, so 255 is what an `(offset, count)`
    /// can name; whether the real bound should be smaller is a distribution
    /// measured over a city and not a number chosen here.
    #[test]
    fn a_tile_past_the_ceiling_drops_and_says_how_many() {
        let mut occlusion = Builder::new(bounds());
        // Distinct spans, so nothing is folded away as a duplicate.
        for step in 0..(MAX_SOLIDS_PER_CELL + 3) {
            let z = (step as i32 - 128).clamp(-128, 127) as i8;
            occlusion.add(
                107,
                107,
                z,
                NOT_A_DOOR,
                &tile(TileFlags::NO_SHOOT, 1),
                Shape::UNREAD,
            );
        }
        let occlusion = occlusion.finish(&Cutaway::OPEN);
        assert_eq!(occlusion.ids_at(107, 107).len(), MAX_SOLIDS_PER_CELL);
        assert_eq!(occlusion.dropped(), 3);
    }

    /// The distribution decision 30.6 asks for, over a grid a test can count by
    /// hand: how many tiles hold how many surfaces.
    #[test]
    fn the_histogram_counts_tiles_and_not_surfaces() {
        use crate::facing::{Face, Facing};

        let mut occlusion = Builder::new(bounds());
        occlusion.add(
            100,
            100,
            0,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT, 20),
            Shape::UNREAD,
        );
        occlusion.add(
            101,
            100,
            0,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT, 20),
            Shape::faced(Facing::Corner {
                right: Face::East,
                left: Face::South,
            }),
        );
        let histogram = occlusion.finish(&Cutaway::OPEN).histogram();
        let tiles = (bounds().width() * bounds().height()) as usize;
        assert_eq!(histogram[0], tiles - 2, "every other tile is open ground");
        assert_eq!(histogram[1], 1, "the plain wall");
        assert_eq!(histogram[2], 1, "the corner, which is two panels");
    }

    /// Outside the rectangle is not the edge of it. A caller walking wider than
    /// the grid was built for must lose the occluder rather than fold it onto
    /// the border, where it would be a wall the map does not have.
    #[test]
    fn a_tile_outside_the_bounds_is_dropped_and_not_clamped() {
        let mut occlusion = Builder::new(bounds());
        occlusion.add(
            99,
            100,
            0,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT, 20),
            Shape::UNREAD,
        );
        let occlusion = occlusion.finish(&Cutaway::OPEN);
        assert_eq!(occlusion.at(99, 100), None);
        assert_eq!(occlusion.at(100, 100), None, "and did not land on the edge");
    }

    /// The upload is two textures now: the index in the grid's own order, and
    /// the list it points into.
    ///
    /// The `z` offset and its clamp moved down into the surface with the span
    /// they belong to, and what is left in the grid is `(offset, count)` — so
    /// a tile that stands nothing is a count of zero and the offset beside it is
    /// not read. Getting the three-channel offset backwards would point every
    /// wall at some other wall's span, which is why each byte of it is named
    /// here.
    #[test]
    fn the_bytes_are_the_index_and_the_surfaces_it_points_into() {
        let mut occlusion = Builder::new(TileBounds {
            min_x: 0,
            max_x: 1,
            min_y: 0,
            max_y: 1,
        });
        occlusion.add(
            1,
            0,
            -10,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT, 20),
            Shape::UNREAD,
        );
        occlusion.add(
            0,
            1,
            120,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT, 60),
            Shape::UNREAD,
        );
        let occlusion = occlusion.finish(&Cutaway::OPEN);

        let bytes = occlusion.bytes();
        assert_eq!(bytes.len(), 4 * 4, "one texel a tile");
        assert_eq!(&bytes[0..4], &[0, 0, 0, 0], "(0,0) stands nothing");
        assert_eq!(
            &bytes[4..8],
            &[0, 0, 0, 1],
            "(1,0) is x-fastest, and the first surface"
        );
        assert_eq!(&bytes[8..12], &[1, 0, 0, 1], "(0,1) is the second");
        assert_eq!(
            &bytes[12..16],
            &[2, 0, 0, 0],
            "and (1,1) stands nothing after them"
        );

        // The list is the opacities and the kinds, in the index's order, with the
        // fourth channel `PRESENT` and the edge mask rather than a bare yes:
        // neither of these was given a face, so both are the whole tile. The
        // spans are `solid_z_bytes`' since `docs/lighting_height.md` phase 2, and
        // the first two channels here are zero.
        let whole = PRESENT | EDGE_ANY;
        let surfaces = occlusion.solid_bytes();
        assert_eq!(
            surfaces.len(),
            LIST_ROW as usize * 4,
            "one row, padded — the fold into a texture is `LIST_ROW` wide",
        );
        assert_eq!(&surfaces[0..4], &[0, 0, OPAQUE, whole]);
        assert_eq!(&surfaces[4..8], &[0, 0, OPAQUE, whole]);
        assert_eq!(&surfaces[8..12], &[0, 0, 0, 0], "and nothing follows it");

        // The spans themselves, in the same order and the same rows: `-10..10`,
        // and one that reaches past the top of the world and stops there.
        let heights = occlusion.solid_z_bytes();
        let span = |n: usize| {
            Solid::span_from_bytes([
                heights[n * 4],
                heights[n * 4 + 1],
                heights[n * 4 + 2],
                heights[n * 4 + 3],
            ])
        };
        assert_eq!(span(0), (-10.0, 10.0));
        assert_eq!(
            span(1),
            (120.0, Z_CEILING as f32 + 255.0 / 256.0),
            "reaches past the top of the world and stops there",
        );

        // A lid's mask is zero and it is still present, which is the one thing
        // `PRESENT` exists for — a fourth channel of zero has to mean nothing
        // stands here and nothing else.
        let mut lid = Builder::new(TileBounds {
            min_x: 0,
            max_x: 0,
            min_y: 0,
            max_y: 0,
        });
        lid.add(
            0,
            0,
            20,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT | TileFlags::FLOOR, 0),
            Shape::UNREAD,
        );
        assert_eq!(
            lid.finish(&Cutaway::OPEN).solid_bytes()[3],
            PRESENT,
            "a floor is present with no side of its own"
        );
    }

    /// The height plane beside that list: the span the record actually holds,
    /// carried whole. `docs/lighting_height.md` phase 2.
    ///
    /// The spans are named rather than sampled, because these are the ones that
    /// occur: a **whole** `z`, which every static off `tiledata` stands at and
    /// which must survive untouched or every wall in the world moves off its own
    /// foundation; and a base on a **half**, either side of zero, which is the
    /// plan's control scene exactly and the case a rounded upload put outside its
    /// own solid. The ends of the range are named too — nothing on the wire may
    /// reconstruct a span *narrower* than the record's.
    #[test]
    fn the_height_plane_carries_the_whole_span() {
        let bounds = TileBounds {
            min_x: 5,
            max_x: 5,
            min_y: 5,
            max_y: 5,
        };
        let raw = |low: f64, high: f64| crate::solid::Solid {
            min: crate::camera::WorldSpot {
                x: 5.0,
                y: 5.0,
                z: low,
            },
            max: crate::camera::WorldSpot {
                x: 6.0,
                y: 6.0,
                z: high,
            },
        };
        let mut builder = Builder::new(bounds);
        // Four distinct owners, so the four boxes are four occluders of the cell
        // rather than one repeated — `add_raw` states the key, since a hand-built
        // box has no `tiledata` to derive one from.
        builder.add_raw(5, 5, raw(0.0, 20.0), Owner::new(0, Graphic(1)));
        builder.add_raw(5, 5, raw(3.5, 6.5), Owner::new(3, Graphic(2)));
        builder.add_raw(5, 5, raw(-3.5, -1.0), Owner::new(-4, Graphic(3)));
        // Past both ends of what the wire can name: a cellar below the floor of
        // the world, and a spire through the top of it.
        builder.add_raw(5, 5, raw(-400.0, 400.0), Owner::new(-128, Graphic(4)));
        let occlusion = builder.finish(&Cutaway::OPEN);

        let heights = occlusion.solid_z_bytes();
        assert_eq!(
            heights.len(),
            occlusion.solid_bytes().len(),
            "one texel a solid, folded exactly as the list it stands beside"
        );
        let span = |n: usize| {
            Solid::span_from_bytes([
                heights[n * 4],
                heights[n * 4 + 1],
                heights[n * 4 + 2],
                heights[n * 4 + 3],
            ])
        };
        assert_eq!(span(0), (0.0, 20.0), "a whole `z` survives the wire exactly");
        assert_eq!(span(1), (3.5, 6.5), "and so does a base half a unit up");
        assert_eq!(span(2), (-3.5, -1.0), "and one below zero");
        assert_eq!(
            span(3),
            (Z_FLOOR as f32, Z_CEILING as f32 + 255.0 / 256.0),
            "and one past both ends stops at them, outward — an occluder may stop \
             more than it should, never less"
        );

        // The list's own two `z` channels are zero and stay zero: a rounded copy
        // of a span that lives here in full is a second answer to one question.
        assert_eq!(&occlusion.solid_bytes()[0..2], &[0, 0]);
    }

    /// The boxes are the cells, at the tiles they stand on — the claim the
    /// wireframe is drawn on the strength of. Getting the row-major arithmetic
    /// backwards here draws every wall at its tile's mirror image, which looks
    /// like a camera bug rather than like an index one.
    #[test]
    fn the_boxes_name_the_tiles_they_stand_on() {
        let mut occlusion = Builder::new(TileBounds {
            min_x: 100,
            max_x: 102,
            min_y: 200,
            max_y: 201,
        });
        occlusion.add(
            102,
            200,
            0,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT, 20),
            Shape::UNREAD,
        );
        occlusion.add(
            100,
            201,
            5,
            NOT_A_DOOR,
            &tile(TileFlags::WINDOW, 10),
            Shape::UNREAD,
        );
        let occlusion = occlusion.finish(&Cutaway::OPEN);
        let boxes: Vec<_> = occlusion.boxes().collect();
        assert_eq!(
            boxes,
            vec![
                (
                    102,
                    200,
                    Cell {
                        bottom: 0,
                        top: 20,
                        opacity: OPAQUE,
                        edges: EDGE_ANY,
                    }
                ),
                (
                    100,
                    201,
                    Cell {
                        bottom: 5,
                        top: 15,
                        opacity: PANE,
                        edges: EDGE_ANY,
                    }
                ),
            ],
            "row by row, x fastest, and open tiles are not in it",
        );
        assert_eq!(Occlusion::EMPTY.boxes().count(), 0, "and an empty grid has none");
    }

    /// The column, in every direction it has to be right in: a roof takes the
    /// sky, a pane passes most of it, a barrel takes none, and a wall down in a
    /// cellar takes none of the street's.
    ///
    /// Before the blur, because these are claims about the column test and the
    /// blur is a claim about the neighbourhood — mixing the two would leave
    /// every number here a function of what is on eight other tiles.
    #[test]
    fn the_column_over_a_tile_is_what_takes_its_sky() {
        let mut occlusion = Builder::new(bounds());
        assert_eq!(occlusion.sky_at(100, 100), SKY_OPEN, "nothing built yet");

        occlusion.shade(100, 100, 20, 0, NOT_A_DOOR, &tile(TileFlags::NO_SHOOT, 5));
        assert_eq!(occlusion.sky_at(100, 100), 0, "a roof over the floor");

        occlusion.shade(101, 100, 20, 0, NOT_A_DOOR, &tile(TileFlags::WINDOW, 5));
        assert_eq!(
            occlusion.sky_at(101, 100),
            204,
            "a glazed roof passes four fifths of the sky",
        );

        occlusion.shade(102, 100, 20, 0, NOT_A_DOOR, &tile(TileFlags::BLOCK, 5));
        assert_eq!(occlusion.sky_at(102, 100), SKY_OPEN, "a crate is not a lid");

        // A cellar's wall, twenty tall, standing forty below the street: its top
        // is still under the floor, so the street above it is open sky.
        occlusion.shade(103, 100, -40, 0, NOT_A_DOOR, &tile(TileFlags::NO_SHOOT, 20));
        assert_eq!(occlusion.sky_at(103, 100), SKY_OPEN);

        // And two panes are darker than one: the column multiplies.
        occlusion.shade(101, 100, 30, 0, NOT_A_DOOR, &tile(TileFlags::WINDOW, 5));
        assert!(occlusion.sky_at(101, 100) < 204);

        assert_eq!(occlusion.sky_at(0, 0), SKY_OPEN, "outside the grid is sky");
    }

    /// The blur is a tile wide and it does not brighten the border.
    ///
    /// The second half is the one worth a test: the grid's edge is where the
    /// *frame* ends, not where the roof does, and a blur that averaged in the
    /// open sky outside would draw a bright rim around the inside of every
    /// frame — a picture of the rectangle rather than of the world.
    #[test]
    fn the_blur_spreads_a_tile_and_leaves_the_border_alone() {
        let small = TileBounds {
            min_x: 0,
            max_x: 2,
            min_y: 0,
            max_y: 2,
        };
        let mut occlusion = Builder::new(small);
        for x in 0..=2u16 {
            for y in 0..=2u16 {
                occlusion.shade(x, y, 20, 0, NOT_A_DOOR, &tile(TileFlags::NO_SHOOT, 5));
            }
        }
        occlusion.blur_sky();
        for x in 0..=2 {
            for y in 0..=2 {
                assert_eq!(occlusion.sky_at(x, y), 0, "({x}, {y}) is under the roof");
            }
        }

        // One roofed tile in the middle of open ground: it lifts off zero and
        // its neighbours come down off the sky, which is the doorway's gradient
        // arriving from the other side.
        let mut one = Builder::new(bounds());
        one.shade(105, 105, 20, 0, NOT_A_DOOR, &tile(TileFlags::NO_SHOOT, 5));
        one.blur_sky();
        // Eight open neighbours and itself dark: 255 * 8 / 9.
        assert_eq!(one.sky_at(105, 105), 226);
        assert!(one.sky_at(106, 105) < SKY_OPEN, "the eave shades its neighbour");
        assert_eq!(one.sky_at(107, 105), SKY_OPEN, "and nothing two tiles away");
    }

    /// The sky is read off the map as it is, not as it is drawn.
    ///
    /// `docs/lighting_world.md`'s decision 3, and it is a real inversion of the
    /// rule beside it: the same roof that must stop casting a shadow the moment
    /// the cutaway removes it must go on keeping the daylight out. Otherwise
    /// walking through a door floods the room with noon, and the player carries
    /// daylight into every building they enter.
    #[test]
    fn the_cutaway_takes_a_roof_from_the_eye_and_not_from_the_sky() {
        let map = Map::from_blocks(1, 1, |_, _| LandCell { tile: 0, z: 0 });
        let graphic = Graphic(0x000A);
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(graphic.0, tile(TileFlags::NO_SHOOT, 5));
        // A patch of roof wide enough that the middle of it is roofed on all
        // nine of the tiles the blur reads.
        let items: Vec<GroundItem> = (2..=6u16)
            .flat_map(|x| {
                (2..=6u16).map(move |y| GroundItem {
                    at: Point::new(x, y, 20),
                    graphic,
                    hue: Hue::NONE,
                })
            })
            .collect();
        let bounds = TileBounds {
            min_x: 0,
            max_x: 7,
            min_y: 0,
            max_y: 7,
        };

        let open = collect(&map, &items, bounds, &tiledata, &Cutaway::OPEN, None);
        let cut = collect(
            &map,
            &items,
            bounds,
            &tiledata,
            &Cutaway {
                max_z: 20,
                no_draw_roofs: true,
                ..Cutaway::OPEN
            },
            None,
        );

        assert!(open.at(4, 4).is_some(), "with nothing cut it occludes");
        assert_eq!(cut.at(4, 4), None, "and the cutaway takes it out of the walk");
        assert_eq!(open.sky_at(4, 4), 0, "the roof keeps the sky off the floor");
        assert_eq!(
            cut.sky_at(4, 4),
            open.sky_at(4, 4),
            "the room brightened when the player walked in",
        );
    }

    /// The second plane is the field, in the same order as the cells, with the
    /// three channels the aperture and a body are going to want left at zero.
    #[test]
    fn the_field_bytes_are_the_sky_in_the_cells_own_order() {
        let mut occlusion = Builder::new(TileBounds {
            min_x: 0,
            max_x: 1,
            min_y: 0,
            max_y: 1,
        });
        occlusion.shade(1, 0, 20, 0, NOT_A_DOOR, &tile(TileFlags::NO_SHOOT, 5));
        let bytes = occlusion.finish(&Cutaway::OPEN).field_bytes();
        assert_eq!(bytes.len(), 4 * 4, "one texel a tile, four channels");
        assert_eq!(&bytes[0..4], &[SKY_OPEN, 0, 0, 0], "(0,0) is open sky");
        assert_eq!(&bytes[4..8], &[0, 0, 0, 0], "(1,0) is x-fastest, and roofed");
    }

    /// A wall the cutaway has taken away casts no shadow. The storey above the
    /// player is not drawn, and a dark band under a wall that is not in the
    /// picture is worse than the light leaking.
    #[test]
    fn a_hidden_wall_occludes_nothing() {
        let map = Map::from_blocks(1, 1, |_, _| LandCell { tile: 0, z: 0 });
        let graphic = Graphic(0x0006);
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(graphic.0, tile(TileFlags::NO_SHOOT, 20));
        let items = [GroundItem {
            at: Point::new(4, 4, 40),
            graphic,
            hue: Hue::NONE,
        }];
        let bounds = TileBounds {
            min_x: 0,
            max_x: 7,
            min_y: 0,
            max_y: 7,
        };

        let open = collect(&map, &items, bounds, &tiledata, &Cutaway::OPEN, None);
        assert!(open.at(4, 4).is_some(), "with nothing cut away it occludes");

        let cut = collect(
            &map,
            &items,
            bounds,
            &tiledata,
            &Cutaway {
                max_z: 20,
                ..Cutaway::OPEN
            },
            None,
        );
        assert_eq!(cut.at(4, 4), None);
    }

    /// One walk of the map serves two frames, and that is decision 33.
    ///
    /// The test the *cache* wants and the one the old shape could not pass: the
    /// map is walked once, and two grids with two different cutaways come out of
    /// the same [`Builder`]. Before the cut moved to [`Builder::finish`] the
    /// cutaway was asked at the walk, so a builder was already one frame's — and
    /// a per-block cache of frames is not a cache, which is what 30.4's storey
    /// band was an attempt to work around.
    ///
    /// The two things it holds apart are the two the cutaway cuts on: a **roof**,
    /// which goes at any height once the player is under one, and a **storey**,
    /// which goes by height alone. Both are in one build here, because a builder
    /// that kept only one of them would pass with the surface's `roof` flag
    /// hard-coded either way.
    #[test]
    fn one_walk_of_the_map_serves_two_cutaways() {
        let mut builder = Builder::new(TileBounds {
            min_x: 0,
            max_x: 7,
            min_y: 0,
            max_y: 7,
        });
        // A roof over one tile, and the floor of an upper storey over another.
        // Neither is drawn with the player indoors; both stand in the map.
        builder.add(
            2,
            2,
            20,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT | TileFlags::ROOF, 5),
            Shape::UNREAD,
        );
        builder.add(
            3,
            3,
            40,
            NOT_A_DOOR,
            &tile(TileFlags::NO_SHOOT | TileFlags::FLOOR, 0),
            Shape::UNREAD,
        );
        // A wall on the ground floor: the control, drawn in both frames, so a
        // grid that came out empty for the wrong reason says so.
        builder.add(4, 4, 0, NOT_A_DOOR, &tile(TileFlags::NO_SHOOT, 20), Shape::UNREAD);

        let outdoors = builder.clone().finish(&Cutaway::OPEN);
        let indoors = builder.finish(&Cutaway {
            max_z: 30,
            no_draw_roofs: true,
            ..Cutaway::OPEN
        });

        assert!(outdoors.at(2, 2).is_some(), "the roof is there with nothing cut");
        assert!(outdoors.at(3, 3).is_some(), "and so is the storey's floor");
        assert_eq!(indoors.at(2, 2), None, "the roof came off");
        assert_eq!(indoors.at(3, 3), None, "and the storey above the player with it");
        assert_eq!(
            outdoors.at(4, 4),
            indoors.at(4, 4),
            "and the wall the player is standing beside is in both",
        );
    }

    /// Britain's houses are dark inside and its streets are not.
    ///
    /// The scenes above are built, which is what makes them readable and is also
    /// what makes them unable to answer this: they contain a roof *this crate
    /// placed*, flagged the way this crate assumed a roof is flagged. The whole
    /// column test rests on a real roof being in the grid at all — membership is
    /// `WINDOW | NO_SHOOT`, which is a fact about arrows, and nothing said it was
    /// also a fact about lids. Measured here rather than assumed: every one of
    /// the 203 roof statics over this block of Britain carries `NO_SHOOT`, so the
    /// answer is yes and `TileFlags::ROOF` is not needed for it.
    ///
    /// The classifier is the cutaway, which is the client's own idea of indoors
    /// and was ported from `UpdateMaxDrawZ` — so the two are independent: one
    /// reads the tile the player stands on and the tile a roof draws on, the
    /// other reads the column over each tile. Where they agree, they agree for
    /// two reasons.
    ///
    /// Stated as means over a block and not per tile: the eaves and the
    /// thresholds are *meant* to be in between, and a per-tile assertion would
    /// either forbid the blur or have to name every doorway in Britain.
    ///
    /// Skipped without the client's files.
    #[test]
    fn britains_rooms_are_dark_and_its_streets_are_not() {
        let Some(dir) = std::env::var_os("OPENSHARD_CLIENT").map(std::path::PathBuf::from) else {
            return;
        };
        let map = Map::load_facet(&dir, 0).expect("Felucca");
        let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");
        // The same block of Britain the cutaway's own tests walk: wide enough to
        // hold whole buildings and the streets between them.
        let (from, to) = ((1470u16, 1600u16), (1530u16, 1660u16));
        let bounds = TileBounds {
            min_x: i32::from(from.0),
            max_x: i32::from(to.0),
            min_y: i32::from(from.1),
            max_y: i32::from(to.1),
        };
        let grid = collect(&map, &[], bounds, &tiledata, &Cutaway::OPEN, None);

        let (mut indoors, mut outdoors) = (Vec::new(), Vec::new());
        let (mut roofs, mut roofs_in_the_grid) = (0, 0);
        for y in from.1..=to.1 {
            for x in from.0..=to.0 {
                let Some(land) = map.land(x, y) else { continue };
                let here = openshard_protocol::world::Point::new(x, y, land.z);
                let sky = grid.sky_at(i32::from(x), i32::from(y));
                match Cutaway::at(&map, &tiledata, here, true) == Cutaway::OPEN {
                    true => outdoors.push(sky),
                    false => indoors.push(sky),
                }
                for item in map.statics_at(x, y) {
                    let tile = tiledata.static_tile(item.tile);
                    if !tile.flags.is_roof() {
                        continue;
                    }
                    roofs += 1;
                    roofs_in_the_grid += usize::from(stops_light(tile));
                }
            }
        }

        // A sweep that found nothing would assert nothing at all.
        assert!(indoors.len() > 500, "only {} indoor tiles", indoors.len());
        assert!(outdoors.len() > 500, "only {} outdoor tiles", outdoors.len());
        assert!(roofs > 100, "only {roofs} roof statics over this block");
        assert_eq!(
            roofs_in_the_grid, roofs,
            "a roof is not in the occlusion grid, so no column test can find it",
        );

        let mean = |tiles: &[u8]| tiles.iter().map(|sky| u32::from(*sky)).sum::<u32>() / tiles.len() as u32;
        let (inside, outside) = (mean(&indoors), mean(&outdoors));
        assert!(inside < 64, "Britain's rooms average {inside} of the sky");
        assert!(outside > 200, "Britain's streets average {outside} of the sky");
    }

    /// Where the 2.0ms goes, phase by phase, on the frame `tests/cost.rs` reads
    /// it off.
    ///
    /// **The number decision 30 is built on is a total**, and a total names no
    /// fix. A bake keyed by block and storey band caches the *statics* — the walk
    /// of the map and what each occluder is — and it caches nothing else: a
    /// frame's grid is still a rectangle the camera chose, so the allocation of
    /// it, the blur of its sky field and the pack of its list are per-frame
    /// whatever is cached. If those three are most of the 2.0ms then the bake as
    /// decision 30.4 states it buys a fraction of what it claims, and the shape
    /// of the cache has to change before it is written rather than after.
    ///
    /// So this is the measurement that comes first, and it is cumulative: each
    /// case adds one phase to the one above it, and what a phase costs is the
    /// difference. The fastest of [`RUNS`] is the reading, for the reason
    /// `tests/cost.rs` gives — a minimum is the run the machine did not
    /// interrupt.
    ///
    /// It is the real frame and not a synthetic one: Britain at the widest zoom,
    /// the bounds `light::lit_tiles` gives it, and **the atlas built**, because
    /// without one every occluder falls back to a body and a corner stops being
    /// two panels — which is exactly the half of the list that step 21.2 doubled.
    ///
    /// Ignored, and gated on the client's files: a measurement, not an assertion.
    #[test]
    #[ignore = "a measurement, not an assertion; needs a client"]
    fn what_the_grid_costs_to_build() {
        /// How many times each phase is run. The fastest is the reading.
        const RUNS: u32 = 16;

        let Some(dir) = std::env::var_os("OPENSHARD_CLIENT").map(std::path::PathBuf::from) else {
            eprintln!("no client files: nothing measured");
            return;
        };
        let map = Map::load_facet(&dir, 0).expect("Felucca");
        let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");
        let art = openshard_uofiles::art::Art::open(&dir).expect("artLegacyMUL.uop");

        // The same eye `tests/cost.rs` measures through, so the two sets of
        // numbers are about one frame.
        let mut camera =
            crate::camera::Camera::new(openshard_protocol::world::Point::new(1495, 1629, 0), 1920, 1080);
        let mut zoom = crate::camera::Zoom::ONE;
        while !zoom.is_widest() {
            zoom = zoom.scale_down();
        }
        camera.zoom_about(0, 0, zoom);
        let animations = crate::animate::StaticAnimations::default();
        let atlas = crate::atlas::StaticAtlas::build(
            &art,
            crate::statics::visible_graphics(&map, &camera, &animations),
        )
        .expect("a screen of statics fits");
        let bounds = crate::light::lit_tiles(&camera);

        let shape = |graphic: Graphic| Shape {
            facing: atlas.sprite(graphic).and_then(|s| s.facing),
            hole: atlas.hole(graphic),
            prism: None,
            blocks: crate::facing::Blocks::EMPTY,
        };
        let floor = |x: u16, y: u16| map.land(x, y).map_or(0, |cell| cell.z);

        let fastest = |mut run: Box<dyn FnMut()>| {
            let mut best = std::time::Duration::MAX;
            for _ in 0..RUNS {
                let start = std::time::Instant::now();
                run();
                best = best.min(start.elapsed());
            }
            best
        };

        // Each of these is the one above it plus a phase, so the phase is the
        // difference. `black_box` on what a case produces, because a builder
        // nobody reads is work a release build may delete.
        let empty = fastest(Box::new(|| {
            std::hint::black_box(Builder::new(bounds));
        }));
        let walked = fastest(Box::new(|| {
            let mut count = 0_usize;
            crate::statics::for_each_static_in(&map, bounds, |_| count += 1);
            std::hint::black_box(count);
        }));
        let shaded = fastest(Box::new(|| {
            let mut grid = Builder::new(bounds);
            crate::statics::for_each_static_in(&map, bounds, |item| {
                let tile = tiledata.static_tile(item.tile);
                grid.shade(
                    item.x,
                    item.y,
                    item.z,
                    floor(item.x, item.y),
                    Graphic(item.tile),
                    tile,
                );
            });
            std::hint::black_box(grid.sky_at(bounds.min_x, bounds.min_y));
        }));
        let added = fastest(Box::new(|| {
            let mut grid = Builder::new(bounds);
            crate::statics::for_each_static_in(&map, bounds, |item| {
                let tile = tiledata.static_tile(item.tile);
                grid.shade(
                    item.x,
                    item.y,
                    item.z,
                    floor(item.x, item.y),
                    Graphic(item.tile),
                    tile,
                );
                if cutaway::shows(&Cutaway::OPEN, item.z, tile) {
                    grid.add(
                        item.x,
                        item.y,
                        item.z,
                        Graphic(item.tile),
                        tile,
                        shape(Graphic(item.tile)),
                    );
                }
            });
            std::hint::black_box(grid.sky_at(bounds.min_x, bounds.min_y));
        }));
        let whole = fastest(Box::new(|| {
            std::hint::black_box(
                collect(&map, &[], bounds, &tiledata, &Cutaway::OPEN, Some(&atlas)).dropped(),
            );
        }));

        // And the two tails on their own, built once and timed over a clone, so
        // that the blur and the pack are read apart rather than as one remainder.
        let built = {
            let mut grid = Builder::new(bounds);
            crate::statics::for_each_static_in(&map, bounds, |item| {
                let tile = tiledata.static_tile(item.tile);
                grid.shade(
                    item.x,
                    item.y,
                    item.z,
                    floor(item.x, item.y),
                    Graphic(item.tile),
                    tile,
                );
                if cutaway::shows(&Cutaway::OPEN, item.z, tile) {
                    grid.add(
                        item.x,
                        item.y,
                        item.z,
                        Graphic(item.tile),
                        tile,
                        shape(Graphic(item.tile)),
                    );
                }
            });
            grid
        };
        let blurred = fastest(Box::new(|| {
            let mut grid = built.clone();
            grid.blur_sky();
            std::hint::black_box(grid.sky_at(bounds.min_x, bounds.min_y));
        }));
        let packed = fastest(Box::new(|| {
            std::hint::black_box(built.clone().finish(&Cutaway::OPEN).solid_count());
        }));
        let cloned = fastest(Box::new(|| {
            std::hint::black_box(built.clone().sky_at(bounds.min_x, bounds.min_y));
        }));

        // Step 21.5, and the two states a cache has. A **still** camera is the
        // ceiling — every block it wants is one it holds — and a camera moving a
        // tile a frame is the case that decides whether the thing is worth
        // having: a widest-zoom frame is about 550 blocks and a tile of pan buys
        // at most one new column of them.
        //
        // Both are timed over the same `RUNS` and the fastest is the reading, so
        // both are the *steady* state rather than an average with the first
        // frame's misses folded in. The hits and misses are printed beside them
        // because they are the companion: a bake that rebuilt every block costs
        // what the walk costs and looks identical in a millisecond.
        let cached = |pan: i32| {
            let mut bake = bake::Bake::new();
            let mut best = std::time::Duration::MAX;
            for run in 0..RUNS as i32 {
                let at = TileBounds {
                    min_x: bounds.min_x + run * pan,
                    max_x: bounds.max_x + run * pan,
                    ..bounds
                };
                let start = std::time::Instant::now();
                std::hint::black_box(
                    bake::collect(&mut bake, &map, &[], at, &tiledata, &Cutaway::OPEN, Some(&atlas))
                        .solid_count(),
                );
                best = best.min(start.elapsed());
            }
            (best, bake.served(), bake.len())
        };
        let (still, still_served, still_held) = cached(0);
        let (panning, panning_served, _) = cached(1);

        let grid = collect(&map, &[], bounds, &tiledata, &Cutaway::OPEN, Some(&atlas));
        let mut statics = 0_usize;
        crate::statics::for_each_static_in(&map, bounds, |_| statics += 1);
        let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;

        // The companion every one of these needs: a frame with no statics in it
        // would make every reading below a measurement of an empty rectangle.
        assert!(statics > 10_000, "only {statics} statics in a widest-zoom frame");
        assert!(
            grid.solid_count() > 10_000,
            "only {} surfaces",
            grid.solid_count()
        );

        println!(
            "grid {}x{} = {} tiles, {statics} statics, {} surfaces on {} standing cells\n\
             \n\
             phase                       ms     cumulative\n\
             allocate the builder    {:6.3}     {:6.3}\n\
             walk the map            {:6.3}     {:6.3}\n\
             + shade the sky         {:6.3}     {:6.3}\n\
             + add the surfaces      {:6.3}     {:6.3}\n\
             + blur and pack         {:6.3}     {:6.3}   (`collect` itself)\n\
             \n\
             of that tail, apart:  blur {:.3}ms, pack {:.3}ms, over a clone costing {:.3}ms",
            bounds.width(),
            bounds.height(),
            bounds.width() * bounds.height(),
            grid.solid_count(),
            grid.boxes().count(),
            ms(empty),
            ms(empty),
            ms(walked),
            ms(walked),
            ms(shaded) - ms(walked),
            ms(shaded),
            ms(added) - ms(shaded),
            ms(added),
            ms(whole) - ms(added),
            ms(whole),
            ms(blurred) - ms(cloned),
            ms(packed) - ms(cloned),
            ms(cloned),
        );

        // The companion, and it is not optional: `still` and `panning` would read
        // exactly as they do here if the cache were never hit once.
        assert!(
            still_served.0 > 0,
            "the bake served nothing to a camera that never moved"
        );
        assert!(
            panning_served.1 > still_served.1,
            "a panning camera wanted no block a still one had not already built"
        );
        println!(
            "the same grid out of a bake — `collect` itself is {:6.3}ms\n\
             \n\
             camera                      ms     served   built   blocks held\n\
             still                   {:6.3}   {:8}{:8}{:14}\n\
             one tile a frame        {:6.3}   {:8}{:8}",
            ms(whole),
            ms(still),
            still_served.0,
            still_served.1,
            still_held,
            ms(panning),
            panning_served.0,
            panning_served.1,
        );
    }
}
