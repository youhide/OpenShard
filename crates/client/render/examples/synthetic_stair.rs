//! A climbable static, alone: no client files, no map, no art — just one
//! [`facing::Prism`] on one tile, its own [`occlusion::Occlusion`], and one
//! flame, rendered through the real GPU pipeline
//! (`GroundRenderer`/`MeshFaceRenderer`/`Blit`) and dumped as a picture.
//!
//! `isolated_scene`'s minimal-scene idea without the client dependency: where
//! `isolated_scene` reads a real static's real art and tiledata to find out
//! whether it is climbable, this builds a `Prism` by hand and hands it
//! straight to [`occlusion::Shape::solid`] — the same construction
//! `light.rs`'s own `a_treads_top_is_not_shadowed_by_its_own_riser` test
//! uses. What that buys is a scene with nothing in it to misread: a lamppost,
//! a texture, a second static, all gone, so a shape seen in the picture is a
//! shape this file's own few lines of geometry produced.
//!
//! - `OPENSHARD_STAIR_UP=north|east|south|west` — which side the climb faces.
//!   Default `north`.
//! - `OPENSHARD_STAIR_TREADS=h1,h2,...` — [`facing::Prism::new`]'s own height
//!   profile, each above the static's own base at `z 0`. Default `1,3,5`,
//!   the same modest three-step rise `light.rs`'s own fixtures climb —
//!   `11,13,15` looks like a real staircase only because the real one it was
//!   copied from stands on a `z 10` base; used as absolute heights from `z 0`
//!   here, it renders five times too tall.
//! - `OPENSHARD_STAIR_RUN=n` — how many flights stand side by side, **across**
//!   the climb rather than along it. Default `1`. A run is the only way to ask
//!   about a seam between two *different* statics: abutting treads of two
//!   neighbouring flights sit at the same `z` on either side of a tile
//!   boundary.
//!
//!   What answers that question changed underneath this knob and the sentence
//!   here changed with it. It used to be `own_run`'s same-row/same-column mask,
//!   standing in for an identity that could not fire because two flights are two
//!   statics; `occlusion::merge` retired the mask (`docs/render/design_occluders.md`'s S3b and
//!   S4) by making identity fire after all — the flights of a run are one
//!   `Owner`, so a tread of the run is **one primitive** and both halves of the
//!   seam name it. That is what this knob poses now, and phase 6h is what it
//!   turned out to be about.
//! - `OPENSHARD_LIGHT_AT=dx,dy` — the flame's position, offset from the run's
//!   **last** tile. Default `2.5,1.0`, below the top tread and in front of the
//!   flight.
//!
//!   Off the last tile rather than the first so that the default stands beside
//!   the staircase for any `run` — measured off the first, `+2.5` is *inside*
//!   the third flight of a run of three, and a point source embedded in a solid
//!   is a scene where the light reference and the frame differ everywhere for a
//!   reason that is neither's defect (7,210 judged pixels of one, against 59
//!   with the flame outside). For a single flight the two anchors are the same
//!   tile and nothing moves.
//!
//!   This line used to say that the default "leaves the far tread in its own
//!   riser's shadow", citing `Surface::shadowed_by_own_tile` and decision 32 —
//!   a function `docs/archive/render/lighting_height.md` phase 3 deleted as vacuous. What the
//!   default actually shows is the two shapes phase 4 is about: the far tread
//!   comes out black, and every tread/riser join wears a hard hairline. A
//!   fixture whose own comment expects a bug is a fixture nobody reads as red,
//!   so this says what it is instead — and the oracle below is what says which
//!   of the two, if either, is the renderer being wrong.
//! - `OPENSHARD_LIGHT_Z` / `OPENSHARD_LIGHT_RADIUS` — default `2` and `6`.
//! - `OPENSHARD_FRAME_VIEW=n` — an index into `debug::View::ALL` and not the
//!   enum's own numbering; `9` is `Shadow` and `5` is `Solid`. Default `0`,
//!   `Lit` — mostly uninformative here, since this scene draws no billboard
//!   under the mesh, but the same index every other tool in this crate uses.
//!   See `isolated_scene`'s header for the whole order.
//! - `OPENSHARD_SCENE_ZOOM=n` — notches of `Zoom::scale_up`. Default `3`,
//!   already the ladder's own maximum (`4:1`) from `Zoom::ONE`.
//! - `OPENSHARD_STAIR_PROBE=x,y,z[,surface];…` — one `light::sample` report per
//!   spot, printed beside the picture. World coordinates, not pixels, and
//!   `surface` is `flat` (the default), `upright`, or a face name — a tread top
//!   is `flat` and a riser is the face it climbs away from. Each probe carries
//!   the flight's own owner, so it is the same question a drawn fragment asks;
//!   the report names which solid stopped the ray, which is what tells "my own
//!   riser shadowed me" from "my own tread's lid did".
//! - `OPENSHARD_STAIR_ORACLE=0` — skip the face oracle below.
//!
//! The picture gets one more mark that is not the shader's own output: a lime
//! crosshair on a black backing plate at the flame's own projected position,
//! because "is the light behind the stair or in front of it" is faster to
//! answer by looking than by reading `OPENSHARD_LIGHT_AT` back.
//!
//! # The face oracle
//!
//! `docs/archive/render/lighting_height.md` phase 4, step 1, and it is deliberately built
//! **before** the fix it is meant to judge. Everything known about that defect
//! is a reason to skip this — the shapes are visible, the cause is named, the
//! fix is one predicate — and every number the phase has is a count of pixels
//! this renderer drew, judged by eye against geometry worked out on paper.
//! That is exactly the arrangement that let phases 1 and 2 report a residual
//! for two sessions which turned out to be the instrument.
//!
//! The shape is `examples/boxes.rs`'s own, and the two share it
//! (`examples/oracle/mod.rs`): sweep **every pixel the rendered `place`
//! attachment says a flight's own face drew** — the renderer's answer to whose
//! pixel it is, not a reconstruction of it — read that fragment's own world
//! position back out of the same attachment, ask an independent slab test about
//! *that* point, and lay the answer against the rendered `View::Shadow` pixel.
//! No arithmetic on the oracle's side is shared with `light.rs` or `blit.wesl`.
//!
//! What is new here, and what phase 4 needs, is **which occluder the fragment is
//! excused from**. "My own static never shadows me" is the rule phase 4 must not
//! adopt: a fragment of a riser really is shadowed by its own flight's tread
//! when the flame stands above and beyond the stair, and a flight is one static
//! and one owner. What the engine drops instead is the fragment's own
//! **`SolidId`** — the primitive, not the object — and this drops the same one,
//! named the same way: the `place` attachment says which mesh row drew the
//! pixel, the row says which grid solid it draws, and that solid is what goes.
//! Every other body counts, its own flight's included.
//!
//! For a run of flights a primitive is **wider than a flight**: the same tread of
//! every flight is one owner and one part, so `occlusion::merge` folds the run
//! into one box a tread and a fragment of any flight is a point of all of it.
//! See [`Body::primitive`].
//!
//! That is one body a tread since `docs/render/design_model.md` phase 6; it was a
//! lid and a riser plane before, and for a *plane* dropping the primitive and
//! excusing a contact at the ray's origin are the same sentence, which is how
//! phase 4 first put it. They are two sentences for a body — a ray heading into
//! the box a fragment stands on is a real crossing — and the rule that survived
//! the reshaping is the one about the name.
//!
//! The oracle's own geometry is **re-derived** from the tread profile rather
//! than read off the grid — a check that asked the scene for the scene's own
//! statement of itself would be checking nothing — and then **gated** against
//! the grid body for body, against the drawn mesh face for face, and each face
//! against the body it is a face of, so a divergence between the derivations is
//! a named panic rather than a drift this tool reports as the renderer's fault.
//!
//! Every line carries the pixels compared as well as the disagreements, the
//! total is asserted non-trivial, and the pixels no flame reaches at all are
//! counted apart rather than folded in: a fragment outside every pool is dark
//! because of a radius, and a visibility oracle has no opinion about radii.
//!
//! # And the same oracle as a picture
//!
//! **A count cannot describe a shape.** "316 pixels disagree, banded at 31..32"
//! is a true sentence about a frame and it does not say whether the shadow has
//! the right outline, whether an edge is straight, or whether a step is lit from
//! the side the flame is on. Those are the questions a person actually asks of a
//! render, and the answer to every one of them is a picture.
//!
//! So [`write_reference`] draws the scene **again**, from the geometry: this
//! file's own polygons, rasterised here, lit by [`oracle_visible`]'s own slab
//! test, one hard shadow from a point. Nothing of the renderer is borrowed but
//! the camera, so the two frames land on the same pixels and can be laid over
//! each other. [`write_difference`] is that overlay, in four colours — grey for
//! agreement, red and blue for the two ways of disagreeing about light, and
//! **yellow for a pixel only one of the two drew at all**, which is a
//! disagreement about the *shape* and the class every count here is blind to by
//! construction: a pixel nobody compared is a pixel nobody counted.
//!
//! Both are written beside whatever `OPENSHARD_FRAME_DUMP` names, as
//! `<stem>_reference.png` and `<stem>_difference.png`, along with
//! `<stem>_faces.png`'s map of which plane drew what.
//!
//! The reference is a **point** source and the engine's flame has a size, so a
//! band of grey along every shadow edge is the two disagreeing about softness
//! rather than about geometry. That is why they are written side by side instead
//! of subtracted into a score.
//!
//! ```sh
//! OPENSHARD_FRAME_VIEW=7 OPENSHARD_FRAME_DUMP=/tmp/stair.png \
//!     cargo run --release -p openshard-client-render --example synthetic_stair
//! ```

// This tool uses the readback and the shade decoder; the box scene and the
// reference tracer's comparison next to them belong to `boxes.rs` and to
// `tests/traced.rs`. A shared module every consumer uses all of is a module
// that has stopped being shared.
#[allow(dead_code)]
mod oracle;

use std::path::PathBuf;

use openshard_client_render::blit::{
    Blit,
    ViewportRect,
};
use openshard_client_render::camera::{
    Camera,
    RealPixel,
    WorldSpot,
    Zoom,
    project_exact,
};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::debug::View;
use openshard_client_render::facing::{
    Face,
    Prism,
};
use openshard_client_render::geometry::Vec2;
use openshard_client_render::light::{
    self,
    Light,
    Lighting,
    Surface,
};
use openshard_client_render::mesh_face::{
    MeshFaceRow,
    MeshFaceVertex,
};
use openshard_client_render::occlusion::{
    self as grid,
    Builder,
    Occlusion,
    Owner,
    Shape,
    SolidId,
};
use openshard_client_render::place::{
    Kind,
    Stance,
};
use openshard_client_render::renderer::{
    self,
    GroundRenderer,
    MeshFaceRenderer,
    Target,
};
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_tiles::{
    StaticTile,
    TileFlags,
};
use oracle::{
    Shade,
    dump,
    read_gbuffer,
    segment_clear_of_box,
};

/// The graphic every flight of the run is built from — a real climbable
/// staircase's own number, and the key [`Occlusion::owner_at`] joins on.
const STAIR: Graphic = Graphic(0x0736);

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn env_or(name: &str, default: &str) -> String {
    env_opt(name).unwrap_or_else(|| default.to_string())
}

fn parse_face(spec: &str) -> Face {
    match spec.trim().to_ascii_lowercase().as_str() {
        "north" => Face::North,
        "east" => Face::East,
        "south" => Face::South,
        "west" => Face::West,
        _ => panic!("OPENSHARD_STAIR_UP wants north/east/south/west, got {spec:?}"),
    }
}

fn parse_treads(spec: &str) -> Vec<u8> {
    spec.split(',')
        .map(|s| s.trim().parse().unwrap_or_else(|_| panic!("tread height: {s:?}")))
        .collect()
}

fn parse_pair(spec: &str) -> (f32, f32) {
    let (a, b) = spec
        .split_once(',')
        .unwrap_or_else(|| panic!("wanted `a,b`, got {spec:?}"));
    (
        a.trim().parse().unwrap_or_else(|_| panic!("{a:?}")),
        b.trim().parse().unwrap_or_else(|_| panic!("{b:?}")),
    )
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: openshard_client_render::gbuffer::required_limits(),
        ..Default::default()
    }))
    .ok()
}

/// The side a climber approaches a riser from: the face opposite the one the
/// flight climbs towards.
///
/// Four pairs, stated here for the third time in this crate —
/// `occlusion::opposite` is the same table and is private, and `Prism::mesh`
/// says it a third way, as the negated [`Face::outward`] it hands a riser for a
/// normal. Which is the point rather than an accident: an oracle that borrowed
/// the engine's copy would agree with the engine by construction. The gate in
/// [`gate_against_mesh`] is what keeps this one honest.
fn descends_towards(up: Face) -> Face {
    match up {
        Face::North => Face::South,
        Face::South => Face::North,
        Face::East => Face::West,
        Face::West => Face::East,
    }
}

/// Which axis a flight climbs along: `true` for `y`, which is what a north or
/// south climb runs on.
fn climbs_along_y(up: Face) -> bool {
    matches!(up, Face::North | Face::South)
}

/// Which of a tread's two planes a [`Slab`] is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Part {
    /// The tread's own top: a lid, flat at the tread's own height, covering the
    /// strip of the run that tread spans.
    Top,
    /// The rise between this tread and the one before it — or the flight's own
    /// base, for the first: a panel, degenerate on the climb axis, spanning that
    /// rise in `z`.
    Riser,
}

/// One plane of one flight: the occluder [`Builder::add`] pushes for it and the
/// mesh face [`Prism::mesh`] draws for it, which are the same plane.
///
/// Both of those are the engine's; this is the oracle's own statement of it,
/// re-derived from the tread profile and gated against both.
struct Slab {
    /// Which flight of the run, indexing `flights`.
    flight: usize,
    /// Which tread of that flight, in climb order.
    tread:  usize,
    part:   Part,
    min:    (f64, f64, f64),
    max:    (f64, f64, f64),
}

impl Slab {
    /// Where along this face's own varying axis a point of it sits, `0.0` at the
    /// low end and `1.0` at the high one, saturating outside.
    ///
    /// A riser varies up `z` and a tread's top varies along the climb, and the
    /// two are the axes a defect would band in. Fragments legitimately fall
    /// outside: `Prism::mesh` grows every riser past the treads it meets so the
    /// rasteriser cannot leave a hairline between two coincident edges, and
    /// those pixels are real pixels of this face drawn beyond its own plane's
    /// span — which is why this saturates rather than asserting.
    fn along(&self, point: (f64, f64, f64), up: Face) -> f64 {
        let (value, low, high) = self.varying(point, up);
        match high > low {
            true => ((value - low) / (high - low)).clamp(0.0, 1.0),
            false => 0.0,
        }
    }

    /// Whether a fragment of this face is drawn **beyond the plane's own span**
    /// — past either end of the axis [`Slab::along`] measures.
    ///
    /// This is the hairline down every tread/riser join, named rather than
    /// inferred. `Prism::mesh` grows every riser by `SEAM_OVERLAP` at both `z`
    /// ends so the last-submitted face wins a coincident edge outright instead
    /// of leaving it to a sub-pixel tie — real pixels of a riser, drawn under
    /// the tread it stands on and over the tread it rises to, at a place the
    /// staircase's own body fills. Whatever those pixels are lit as, they are
    /// not lit as the surface they stand in for, and a count that folded them
    /// into the rest of the face could not say whether a seam in the picture is
    /// the lighting's answer or the mesh's own overlap.
    fn beyond_its_plane(&self, point: (f64, f64, f64), up: Face) -> bool {
        let (value, low, high) = self.varying(point, up);
        value < low || value > high
    }

    /// The coordinate this face varies along, with that axis's own span.
    fn varying(&self, point: (f64, f64, f64), up: Face) -> (f64, f64, f64) {
        match self.part {
            Part::Riser => (point.2, self.min.2, self.max.2),
            Part::Top if climbs_along_y(up) => (point.1, self.min.1, self.max.1),
            Part::Top => (point.0, self.min.0, self.max.0),
        }
    }

    /// Which way this plane looks, in the same units a flame's offset is stated
    /// in — `x`/`y` across the map, `z` in tiles.
    ///
    /// **What it is for is refusing to have an opinion.** A one-sided surface
    /// cannot be lit from behind, so for a fragment the flame stands *behind*,
    /// "is anything in the way" is not a question about that fragment's shade at
    /// all — the shade is decided by the facing term before occlusion is ever
    /// asked. Counting such a pixel as a disagreement measures the oracle's own
    /// missing half-space test and calls it the renderer's fault.
    ///
    /// This is exactly the argument `Shade::Unreached` already carries one axis
    /// over: a fragment outside every pool is dark because of a *radius*, and a
    /// visibility oracle has no opinion about radii either. Both classes are
    /// counted apart rather than folded in.
    fn normal(&self, up: Face) -> (f64, f64, f64) {
        match self.part {
            Part::Top => (0.0, 0.0, 1.0),
            Part::Riser => {
                let [x, y] = descends_towards(up).outward();
                (f64::from(x), f64::from(y), 0.0)
            }
        }
    }

    /// How far in front of this plane the flame stands, in tiles, along the
    /// plane's own normal — negative behind it.
    ///
    /// **A diagnostic now and no longer a term.** It was `light::faces`'s own
    /// `along`, the unnormalised dot product the engine divided by `FACE_EDGE`,
    /// and it is what made a single constant mean one width across a wall and
    /// another above a lid. `docs/render/design_model.md` phase 3 normalised that
    /// argument; what is left of this is a *legible* number for a report — "the
    /// flame is a tenth of a tile off this plane" says something a cosine of
    /// `0.04` does not. `z` is divided into tiles first, the way
    /// `light::sample_with` states the offset.
    fn off_plane(&self, point: (f64, f64, f64), flame: (f64, f64, f64), up: Face) -> f64 {
        let normal = self.normal(up);
        let toward = self.toward(point, flame);
        normal.0 * toward.0 + normal.1 * toward.1 + normal.2 * toward.2
    }

    /// The fragment-to-flame offset, in tiles on all three axes.
    fn toward(&self, point: (f64, f64, f64), flame: (f64, f64, f64)) -> (f64, f64, f64) {
        let z_per_tile = f64::from(openshard_client_render::light::Z_PER_TILE);
        (
            flame.0 - point.0,
            flame.1 - point.1,
            (flame.2 - point.2) / z_per_tile,
        )
    }

    /// How squarely this plane looks at the flame — `light::lit_from`'s own
    /// quantity, `max(N · L, 0)`.
    ///
    /// **This is the one term of the five that the reference shares a *shape*
    /// with the engine, and it is deliberate.** The band it replaced was not: a
    /// half-space softened over a fifth of a tile is a stand-in for a BRDF, so an
    /// oracle ruling strictly was measuring the stand-in's price. A cosine is
    /// what a path tracer computes too — the reference is not copying the
    /// engine's arithmetic here, the two have converged on the same physics, and
    /// the third opinion in `tests/traced.rs` is what says so independently.
    fn lit_from(&self, point: (f64, f64, f64), flame: (f64, f64, f64), up: Face) -> f64 {
        let toward = self.toward(point, flame);
        let span = (toward.0 * toward.0 + toward.1 * toward.1 + toward.2 * toward.2).sqrt();
        if span <= 0.0 {
            return 1.0;
        }
        (self.off_plane(point, flame, up) / span).clamp(0.0, 1.0)
    }

    /// Whether the flame is on the side this plane looks at.
    ///
    /// The strict half-space it has always been, said through the shading term
    /// rather than beside it: Lambert is exactly zero behind the plane and
    /// nowhere else, so "the flame is behind this surface" and "this surface
    /// catches nothing" are one fact now and there is no second predicate that
    /// could disagree with the first.
    fn faces(&self, point: (f64, f64, f64), flame: (f64, f64, f64), up: Face) -> bool {
        self.lit_from(point, flame, up) > 0.0
    }

    /// This plane's four corners, in the ring order
    /// [`Prism::mesh`](openshard_client_render::facing::Prism) pushes them.
    ///
    /// A [`Slab`] is stored as a box because that is what a slab test wants; a
    /// *rasteriser* wants a polygon, and for a degenerate box the two are the
    /// same four points said differently. The ring is the mesh's own so that the
    /// reference picture and the rendered one triangulate the same quad the same
    /// way — a fan of `0,1,2 / 0,2,3` splits a quad along one of its two
    /// diagonals, and the two choices differ by a pixel at the corners.
    fn quad(&self) -> [(f64, f64, f64); 4] {
        match self.part {
            Part::Top => {
                [
                    (self.min.0, self.min.1, self.min.2),
                    (self.max.0, self.min.1, self.min.2),
                    (self.max.0, self.max.1, self.min.2),
                    (self.min.0, self.max.1, self.min.2),
                ]
            }
            Part::Riser => {
                [
                    (self.min.0, self.min.1, self.max.2),
                    (self.max.0, self.max.1, self.max.2),
                    (self.max.0, self.max.1, self.min.2),
                    (self.min.0, self.min.1, self.min.2),
                ]
            }
        }
    }

    /// What this face is, for a report: `flight 1 tread 2's riser`.
    fn label(&self) -> String {
        let part = match self.part {
            Part::Top => "top",
            Part::Riser => "riser",
        };
        format!("flight {} tread {}'s {part}", self.flight, self.tread)
    }
}

/// One **tread**, whole: the volume the occlusion grid stands up for it, from
/// the flight's own base to that tread's own height, across the strip of the run
/// that tread spans.
///
/// A [`Slab`] is a *surface* — what `Prism::mesh` draws and what the `place`
/// attachment names — and this is the *solid* a ray travels through. The two
/// were one thing while the grid held a lid and a riser plane per tread; since
/// `docs/render/design_model.md` phase 6 put the body back
/// (`occlusion::Solid::tread_box_of`), a drawn plane is a **face of** one of
/// these rather than an occluder in its own right. This file states both because
/// it checks both: [`gate_against_grid`] holds a body against the grid's own
/// solid, and [`gate_plane_of_body`] holds each plane against the body it is a
/// face of, so the two derivations cannot drift apart quietly.
struct Body {
    /// Which flight of the run, indexing `flights`.
    flight:    usize,
    /// Which tread of that flight, in climb order.
    tread:     usize,
    /// Which **primitive of the grid** this tread is a piece of.
    ///
    /// A tread of one flight stopped being a primitive when `occlusion::merge`
    /// landed (`docs/render/design_occluders.md`'s D2b): the same tread of two flights standing
    /// side by side shares a whole face with its neighbour and agrees in every
    /// other field the merge asks about — one [`Owner`], because an owner is a
    /// `(z, graphic)` and carries no tile, and one [`Part`](grid::Part), because
    /// both are the same tread of the same profile — so the grid folds the run
    /// into **one box a tread**. Two treads of one flight are not folded: they
    /// differ in `Part` and in span.
    ///
    /// So a piece's own primitive is its tread number, and that is a derivation
    /// like every other one in this file rather than a reading: [`gate_against_
    /// grid`] holds the fold against the solids the grid really pushed, corners
    /// and identity both.
    primitive: usize,
    min:       (f64, f64, f64),
    max:       (f64, f64, f64),
}

/// One **primitive of the grid**: a tread of the run, whole — every flight's
/// piece of that tread folded together. See [`Body::primitive`].
struct Primitive {
    /// Which tread of the profile, which is also this primitive's index in the
    /// list [`merged`] hands back.
    tread: usize,
    min:   (f64, f64, f64),
    max:   (f64, f64, f64),
}

/// A corner as an array, so an axis can be an index: the tuples above read
/// better where a corner is written out and worse in a loop over axes.
fn axes(corner: (f64, f64, f64)) -> [f64; 3] {
    [corner.0, corner.1, corner.2]
}

/// Which horizontal axis the flights of a run stand along — **across** the
/// climb, since that is the only arrangement `main` builds and the only one two
/// abutting treads at one height can have.
fn across_axis(up: Face) -> usize {
    match up {
        Face::North | Face::South => 0,
        Face::East | Face::West => 1,
    }
}

/// The pieces folded into the primitives the grid holds — and the fold checked
/// while it happens.
///
/// The union taken here is a union of **point sets**, not a bounding box, and
/// that is the merge's own condition rather than a nicety: `occlusion::merge`
/// joins two boxes only where they share a whole face, so the pieces of one
/// primitive have to agree exactly on the two axes that are not the run's, and
/// tile the run's axis with no gap and no overlap. A fixture that failed either
/// would be folding something the grid did not, and the corner comparison in
/// [`gate_against_grid`] would then be reporting the fixture's own arithmetic.
fn merged(bodies: &[Body], up: Face) -> Vec<Primitive> {
    let across = across_axis(up);
    let count = bodies.iter().map(|body| body.primitive + 1).max().unwrap_or(0);
    (0..count)
        .map(|tread| {
            let pieces: Vec<&Body> = bodies.iter().filter(|body| body.primitive == tread).collect();
            let first = pieces.first().expect("a primitive nothing is a piece of");
            let (mut lo, mut hi) = (axes(first.min), axes(first.max));
            let mut spans = 0.0;
            for piece in &pieces {
                let (min, max) = (axes(piece.min), axes(piece.max));
                for axis in 0..3 {
                    if axis == across {
                        continue;
                    }
                    assert!(
                        (min[axis] - lo[axis]).abs() < 1e-12 && (max[axis] - hi[axis]).abs() < 1e-12,
                        "tread {tread}: flight {}'s piece spans {}..{} on axis {axis} and flight {}'s \
                         spans {}..{}, so the two do not share a whole face and the grid would not \
                         have merged them",
                        piece.flight,
                        min[axis],
                        max[axis],
                        first.flight,
                        lo[axis],
                        hi[axis],
                    );
                }
                lo[across] = lo[across].min(min[across]);
                hi[across] = hi[across].max(max[across]);
                spans += max[across] - min[across];
            }
            assert!(
                (spans - (hi[across] - lo[across])).abs() < 1e-12,
                "tread {tread}: its {} pieces are {spans} wide together and their union is {} wide, \
                 so they leave a gap or overlap and their union is not their union",
                pieces.len(),
                hi[across] - lo[across],
            );
            Primitive {
                tread,
                min: (lo[0], lo[1], lo[2]),
                max: (hi[0], hi[1], hi[2]),
            }
        })
        .collect()
}

/// The tile-relative footprint of one climb-axis span, `lo..=hi` of the run
/// counted from the low side towards `up`.
///
/// [`Prism::footprint`](openshard_client_render::facing::Prism) re-derived, and
/// not because it is `pub(crate)`: it is the arithmetic the occlusion grid and
/// the drawn mesh already **share**, so an oracle that called it would be
/// checking the scene against the scene's own statement of itself. What keeps a
/// re-derivation from becoming a second, quietly different formula is
/// [`gate_against_grid`], which asserts every body built here against the solid
/// the grid really pushed.
fn strip(x: f64, y: f64, up: Face, lo: f64, hi: f64) -> (f64, f64, f64, f64) {
    match up {
        Face::North => (x, x + 1.0, y + 1.0 - hi, y + 1.0 - lo),
        Face::South => (x, x + 1.0, y + lo, y + hi),
        Face::West => (x + 1.0 - hi, x + 1.0 - lo, y, y + 1.0),
        Face::East => (x + lo, x + hi, y, y + 1.0),
    }
}

/// Every plane one flight standing at `stands` is, in the order `Builder::add`
/// pushes them and `Prism::mesh` draws them: a top and then a riser, per tread,
/// in climb order.
///
/// The shared order is what lets a `place` attachment's own row number name a
/// plane — see [`gate_against_mesh`], which asserts it rather than assuming it.
fn flight_slabs(flight: usize, stands: Point, up: Face, treads: &[u8]) -> Vec<Slab> {
    let count = treads.len();
    let (x, y) = (f64::from(stands.x), f64::from(stands.y));
    let base = f64::from(stands.z);
    let mut slabs = Vec::with_capacity(count * 2);
    let mut risen = base;
    for (tread, &height) in treads.iter().enumerate() {
        let top = base + f64::from(height);
        let (lo, hi) = (tread as f64 / count as f64, (tread + 1) as f64 / count as f64);
        let (min_x, max_x, min_y, max_y) = strip(x, y, up, lo, hi);
        slabs.push(Slab {
            flight,
            tread,
            part: Part::Top,
            min: (min_x, min_y, top),
            max: (max_x, max_y, top),
        });
        let (min_x, max_x, min_y, max_y) = strip(x, y, up, lo, lo);
        slabs.push(Slab {
            flight,
            tread,
            part: Part::Riser,
            min: (min_x, min_y, risen),
            max: (max_x, max_y, top),
        });
        risen = top;
    }
    slabs
}

/// Every **body** one flight standing at `stands` is, in the order
/// `Builder::add` pushes them: one a tread, in climb order, each standing from
/// the flight's own base to that tread's height.
///
/// [`strip`] again and deliberately: the footprint of a tread is the same
/// arithmetic whether the question is which plane the mesh drew or which volume
/// stops a ray, and one expression for it here is the same rule this file
/// applies to the engine.
fn flight_bodies(flight: usize, stands: Point, up: Face, treads: &[u8]) -> Vec<Body> {
    let count = treads.len();
    let (x, y) = (f64::from(stands.x), f64::from(stands.y));
    let base = f64::from(stands.z);
    treads
        .iter()
        .enumerate()
        .map(|(tread, &height)| {
            let (lo, hi) = (tread as f64 / count as f64, (tread + 1) as f64 / count as f64);
            let (min_x, max_x, min_y, max_y) = strip(x, y, up, lo, hi);
            Body {
                flight,
                tread,
                // The same tread of every flight is one primitive — see
                // [`Body::primitive`], which is where that is argued.
                primitive: tread,
                min: (min_x, min_y, base),
                max: (max_x, max_y, base + f64::from(height)),
            }
        })
        .collect()
}

/// That the bodies this file derived are the solids the occlusion grid holds —
/// same count, same order, same corners, same kind.
///
/// The gate the re-derivation in [`strip`] is worth having: two statements of
/// one geometry either agree, in which case the oracle is asking about the scene
/// the renderer lit, or they do not, in which case every count below is about a
/// scene nobody built. A drift that is a panic here is a drift that cannot be
/// read as the renderer being wrong.
///
/// It compared *planes* until `docs/render/design_model.md` phase 6, when a tread
/// stopped being a lid and a riser and became one body again. That change is
/// what this assertion is for — it is the first thing that failed after it, in
/// the count, which is exactly the shape of failure a gate on a scene's own
/// geometry is built to have.
///
/// **And it compared one flight's own boxes until `occlusion::merge` landed**,
/// which is the second time this gate was the first thing to fail: a run's
/// flights are one owner and one part a tread, so the grid folds the run into a
/// box a tread and a per-flight derivation is a box a third the width
/// (`docs/render/design_model.md` phase 6i's fourth item — `this oracle says 101,
/// the grid's own solid says 103`, and `OPENSHARD_STAIR_RUN>1` has been unusable
/// since). What it compares now is the fold, [`merged`], and what makes that a
/// statement about the grid rather than about this file is the second half
/// below: every flight of the run has to name the **same** `SolidId` for a
/// tread. A merge that stopped firing would leave the corners agreeing with
/// nothing and that identity red.
fn gate_against_grid(bodies: &[Body], flights: &[Point], up: Face, occlusion: &Occlusion) {
    let primitives = merged(bodies, up);
    let mut at = 0usize;
    for (flight, stands) in flights.iter().enumerate() {
        let solids: Vec<_> = occlusion
            .solids_at(i32::from(stands.x), i32::from(stands.y))
            .collect();
        assert_eq!(
            solids.len(),
            primitives.len(),
            "flight {flight} at ({}, {}): the grid holds {} solids and this oracle derived {} treads",
            stands.x,
            stands.y,
            solids.len(),
            primitives.len(),
        );
        for (primitive, solid) in primitives.iter().zip(&solids) {
            let corners = [
                (primitive.min.0, solid.space.min.x),
                (primitive.max.0, solid.space.max.x),
                (primitive.min.1, solid.space.min.y),
                (primitive.max.1, solid.space.max.y),
                (primitive.min.2, solid.space.min.z),
                (primitive.max.2, solid.space.max.z),
            ];
            for (mine, theirs) in corners {
                assert!(
                    (mine - theirs).abs() < 1e-12,
                    "tread {}, seen from flight {flight}: this oracle says {mine}, the grid's own \
                     solid says {theirs}",
                    primitive.tread,
                );
            }
            // A tread is a **body**, and a body names every edge: `Edges::ANY` is
            // "it stands up and nobody knows which way", which a walk reads as an
            // exact slab test rather than as a lid's crossing or a panel's run
            // mask. The half of the pairing corner equality cannot check — a lid
            // of no height occupies the same corners as a body of no height.
            assert_eq!(
                solid.edges,
                grid::Edges::ANY,
                "tread {}, seen from flight {flight}: the grid's own solid has edges {:#06b}, so it \
                 is not a body",
                primitive.tread,
                solid.edges.raw(),
            );
        }
        at += primitives.len();
    }
    // Every piece this file derived went into a primitive that was compared: the
    // loop above walks the primitives once per flight, so what it cannot notice
    // on its own is a flight whose treads folded into nothing.
    assert_eq!(
        at,
        bodies.len(),
        "{} pieces were derived and {at} went into a compared primitive",
        bodies.len(),
    );

    // **The fold is the grid's and not this file's**, stated as identity: every
    // flight of the run names one `SolidId` for a tread. That is the same
    // question `id_of` answers for the mesh rows below — one owner, since an
    // owner is a `(z, graphic)` and the flights of a run stand at one height
    // under one graphic — and it is what makes a fragment of any flight exempt
    // from the whole run's tread, which is what [`oracle_visible`] drops.
    for primitive in &primitives {
        let part = grid::Part::nth(primitive.tread);
        let named: Vec<Option<SolidId>> = flights
            .iter()
            .map(|stands| {
                occlusion.id_of(
                    i32::from(stands.x),
                    i32::from(stands.y),
                    Owner::new(stands.z, STAIR),
                    part,
                )
            })
            .collect();
        assert!(
            named.iter().all(|id| *id == named[0]) && named[0].is_some(),
            "tread {}: the flights of this run name {named:?}, so the grid did not fold them into \
             one primitive and every count below would be about a staircase nobody built",
            primitive.tread,
        );
    }
}

/// Which body a plane is a face of: **two drawn faces a tread, one body a
/// tread**, in the same order, so the join divides.
///
/// The same arithmetic `statics::push_mesh` does to name a mesh row's solid
/// (`Part::nth(part / 2)`), and for the same reason — `Prism::mesh` emits a top
/// and then the rise below it for each tread while `Builder::add` stands up one
/// body. Assumed nowhere: [`gate_plane_of_body`] is handed the body this picks
/// and its first assertion is that the two name the same tread of the same
/// flight.
fn body_of(plane: usize) -> usize {
    plane / 2
}

/// Which **primitive of the grid** a plane's fragments are points of — what the
/// walk excuses them from, and therefore what [`oracle_visible`] is told to drop.
///
/// Read off the piece rather than computed from `plane` a second time: the piece
/// knows which primitive it was folded into ([`Body::primitive`]), and a formula
/// here would be a third statement of the merge for the other two to drift from.
fn primitive_of(bodies: &[Body], plane: usize) -> usize {
    bodies[body_of(plane)].primitive
}

/// That the plane this file derived for a mesh face really is a **face of** the
/// tread's own body — the join between the two derivations above.
///
/// Without it the surface list and the volume list are two independent
/// statements of one staircase that nothing holds together: the sweep would name
/// a fragment by a plane and ask about visibility with a box, and a drift
/// between the two would read as the renderer being wrong. What is asserted is
/// exactly what "a face of" means for an axis-aligned box, and no more — a top
/// is the body's whole lid; a riser lies in the body's face on the side the
/// flight descends towards, is as wide as the tread, ends at the tread's own
/// height and starts somewhere at or above its floor. Where it starts is the
/// tread below's business and is not re-derived here.
fn gate_plane_of_body(slab: &Slab, body: &Body, up: Face) {
    assert_eq!(
        (slab.flight, slab.tread),
        (body.flight, body.tread),
        "{} was paired with flight {} tread {}'s body",
        slab.label(),
        body.flight,
        body.tread,
    );
    let same = |mine: f64, theirs: f64, what: &str| {
        assert!(
            (mine - theirs).abs() < 1e-12,
            "{}: this oracle's plane puts {what} at {mine}, the tread's own body at {theirs}",
            slab.label(),
        );
    };
    // Across the climb both planes are the body's full width: a tread's top
    // covers its strip of the tile and its riser is as wide as the tread is.
    match climbs_along_y(up) {
        true => {
            same(slab.min.0, body.min.0, "the low x");
            same(slab.max.0, body.max.0, "the high x");
        }
        false => {
            same(slab.min.1, body.min.1, "the low y");
            same(slab.max.1, body.max.1, "the high y");
        }
    }
    match slab.part {
        // The lid: the body's own top face, whole.
        Part::Top => {
            same(slab.min.2, body.max.2, "the plane");
            same(slab.max.2, body.max.2, "the plane");
            match climbs_along_y(up) {
                true => {
                    same(slab.min.1, body.min.1, "the low end of the strip");
                    same(slab.max.1, body.max.1, "the high end of the strip");
                }
                false => {
                    same(slab.min.0, body.min.0, "the low end of the strip");
                    same(slab.max.0, body.max.0, "the high end of the strip");
                }
            }
        }
        // The rise: the exposed part of the body's face on the side the flight
        // descends towards, which is the side the tread below it does not cover.
        Part::Riser => {
            let [dx, dy] = descends_towards(up).outward();
            let (low, high, face) = match climbs_along_y(up) {
                true => {
                    (
                        slab.min.1,
                        slab.max.1,
                        match dy > 0.0 {
                            true => body.max.1,
                            false => body.min.1,
                        },
                    )
                }
                false => {
                    (
                        slab.min.0,
                        slab.max.0,
                        match dx > 0.0 {
                            true => body.max.0,
                            false => body.min.0,
                        },
                    )
                }
            };
            same(low, face, "the plane");
            same(high, face, "the plane");
            same(slab.max.2, body.max.2, "the top of the rise");
            assert!(
                slab.min.2 >= body.min.2 - 1e-12 && slab.min.2 <= slab.max.2,
                "{}: the rise starts at {} and its own tread's body stands from {} to {}",
                slab.label(),
                slab.min.2,
                body.min.2,
                body.max.2,
            );
        }
    }
}

/// That mesh row `id` draws plane `slabs[id]`, which is what lets the `place`
/// attachment's own row number name the plane a pixel is a point of.
///
/// Checked rather than assumed, and against the two things the mesh states
/// independently: the face's own **normal**, which says top or riser and which
/// way a riser looks, and the coordinate of the plane the face **is**.
///
/// That coordinate is the one thing `Prism::mesh`'s two overlaps leave alone. It
/// grows every riser past the treads it meets in `z` and widens every face
/// across the tile, both to keep the rasteriser from leaving a hairline where
/// two faces share an edge — so a mesh face is not the same *rectangle* as its
/// occluder and comparing every corner would be comparing the overlap. The
/// degenerate axis is exact: a top is at its tread's own height and a riser is
/// at its own boundary along the climb.
fn gate_against_mesh(slab: &Slab, face: &openshard_client_render::mesh::Face, up: Face) {
    let stance = Stance::of_normal(face.normal).expect("a stair's own normals are all recognized");
    let (want, plane, of_corner): (Stance, f64, fn(&WorldSpot) -> f64) = match slab.part {
        Part::Top => (Stance::Flat, slab.min.2, |corner| corner.z),
        Part::Riser if climbs_along_y(up) => {
            (Stance::face(descends_towards(up)), slab.min.1, |corner| corner.y)
        }
        Part::Riser => (Stance::face(descends_towards(up)), slab.min.0, |corner| corner.x),
    };
    assert_eq!(stance, want, "{}: the mesh drew a {stance:?}", slab.label());
    for corner in face.vertices() {
        assert!(
            (of_corner(corner) - plane).abs() < 1e-12,
            "{}: the mesh's own corner sits at {}, this oracle's plane at {plane}",
            slab.label(),
            of_corner(corner),
        );
    }
}

/// How much of the flame `point` can see, geometrically: every **body** of every
/// flight, tested by [`segment_clear_of_box`], once per point of the flame's own
/// sphere.
///
/// **A share and not a bool since `docs/render/design_model.md`'s phase 5**, which
/// is where that phase keeps phase 4's own promise. Phase 4 measured 88 pixels of
/// a tread's outer corner that the frame drew shadowed and this oracle called
/// lit, and named them: "the engine's area light against a point source, and
/// phase 5 is where those become comparable." `light::flame_points` is where the
/// engine's rays end, so the two are asked about the same body; the segment test
/// below is still this file's own and shares no arithmetic with any walk.
///
/// `own` is the **solid** the fragment is a point of, and dropping exactly that
/// one is the engine's own rule stated independently: `light.rs` skips an
/// occluder whose `SolidId` is the fragment's (`docs/render/design_model.md` phase
/// 4, `if lit.solid == Some(id) { continue }`). What changed with phase 6 is not
/// the rule but what a solid *is* — a tread used to be a lid and a riser plane
/// and is one body now, so the thing to drop is the body the drawn face belongs
/// to rather than the face itself. The `place` attachment names the mesh row,
/// the row names the grid solid it draws, and this drops that solid; nothing
/// here is guessed from a position.
///
/// It is a rule and not an epsilon either way. A point on a plane crosses it at
/// its own origin and nowhere else, so for the split geometry "a contact at the
/// origin does not count" and "this primitive does not count" were one sentence
/// — `docs/archive/render/lighting_height.md` phase 4 says so. For a body they are two: a ray
/// heading into the box the fragment stands on is a genuine crossing, and both
/// walks are told to ignore it by name.
///
/// Every *other* body counts, its own flight's included, and that is not a
/// detail — it is the counter-example the fix must not break. A ray leaving the
/// front of a bottom step, heading up and away from a flame standing above and
/// beyond the staircase, passes through the tread above it; a staircase's own
/// body is genuinely in the way, and a rule phrased as "a fragment is never
/// shadowed by its own static" would light it. A flight is three bodies and this
/// drops one of them, which is exactly the granularity phase 4 found the owner
/// too coarse for.
///
/// `own` names a **primitive** and not a piece since `occlusion::merge`: a
/// fragment of one flight's third tread is a point of the box the whole run's
/// third tread was folded into, so every flight's piece of that tread goes with
/// it. Dropping only the fragment's own flight's piece would be this file
/// stating a rule the walk does not have, and the pixels it disagreed about
/// would be the neighbour's half of one box shadowing the other half.
///
/// The pieces are what is tested rather than the folded box, and the two are the
/// same test: pieces of one primitive tile it with no gap ([`merged`] asserts
/// exactly that), and a segment meets a union of contiguous boxes exactly when
/// it meets one of them — `occlusion::merge`'s own header, from the other side.
fn oracle_visible(point: (f64, f64, f64), light: (f64, f64, f64), bodies: &[Body], own: usize) -> f64 {
    let spot = light::Spot::at(
        Vec2::new(point.0 as f32, point.1 as f32),
        point.2 as f32,
        (point.0.floor() as i32, point.1.floor() as i32),
    );
    let points = light::flame_points(
        spot,
        [light.0 as f32, light.1 as f32, light.2 as f32],
        openshard_client_render::light::FLAME_RADIUS,
        openshard_client_render::light::ShadowRays::DEFAULT,
    );
    let clear = points
        .iter()
        .filter(|at| {
            let to = (f64::from(at[0]), f64::from(at[1]), f64::from(at[2]));
            bodies
                .iter()
                .filter(|body| body.primitive != own)
                .all(|body| segment_clear_of_box(point, to, body.min, body.max))
        })
        .count();
    clear as f64 / points.count() as f64
}

/// Runs of adjacent non-empty bands, as `(first, past_the_last, points)`.
///
/// A defect that is one band prints as one entry and one spread over a face
/// prints as many, which is the distinction worth reading at a glance and the
/// one a total cannot carry: `docs/archive/render/lighting_height.md` phase 1's own residual
/// was not spread over its face at all, it was a band.
fn runs_of(bands: &[usize]) -> Vec<(usize, usize, usize)> {
    let mut runs = Vec::new();
    let mut band = 0usize;
    while band < bands.len() {
        if bands[band] == 0 {
            band += 1;
            continue;
        }
        let start = band;
        let mut points = 0usize;
        while band < bands.len() && bands[band] > 0 {
            points += bands[band];
            band += 1;
        }
        runs.push((start, band, points));
    }
    runs
}

/// `/tmp/stair.png` and `shadow` to `/tmp/stair_shadow.png` — a second view
/// beside the one that was asked for, rather than over it.
fn beside(path: &std::path::Path, what: &str) -> PathBuf {
    let stem = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let mut named = path.to_path_buf();
    named.set_file_name(match path.extension() {
        Some(extension) => format!("{stem}_{what}.{}", extension.to_string_lossy()),
        None => format!("{stem}_{what}"),
    });
    named
}

/// What [`write_reference`]'s own rasteriser decided about one pixel: which
/// plane covers it, and where in the world that pixel's own fragment sits.
///
/// The pair travels together and is never read apart — a plane without the point
/// on it cannot be shaded, and a point without the plane it belongs to cannot be
/// excused from the right one.
#[derive(Clone, Copy)]
struct Covered {
    /// Which of `slabs` won this pixel.
    plane: usize,
    /// The pixel centre's own world position, interpolated across the covering
    /// triangle. The projection is affine and a plane is planar, so this is
    /// exact rather than approximate — the same property `MeshFaceVertex::world`
    /// rests on.
    at:    (f64, f64, f64),
}

/// **The scene rasterised a second time, from the geometry alone** — which plane
/// covers each pixel and where in the world that pixel's fragment sits.
///
/// Nothing of the renderer is used but the **camera**, deliberately: the two
/// pictures have to land on the same pixels to be laid over each other, and where
/// a fragment projects is not what any of this is about. Everything else is this
/// file's own — [`Slab::quad`]'s polygons rasterised here and the world position
/// of a covered pixel interpolated here.
///
/// Painter order, later face wins, which is `Prism::mesh`'s own submission order
/// and what the mesh pass's `LessEqual` depth does with one depth per static.
///
/// Separate from the two pictures drawn out of it because they differ only in
/// what they *say* about a covered pixel — visibility, or light — and a second
/// copy of a rasteriser is a second set of edges to disagree about.
fn cover(slabs: &[Slab], camera: &Camera, width: u32, height: u32) -> Vec<Option<Covered>> {
    let projection = camera.projection();
    let to_pixel = |corner: (f64, f64, f64)| {
        let screen = camera.to_view_exact(project_exact(WorldSpot {
            x: corner.0,
            y: corner.1,
            z: corner.2,
        }));
        (
            f64::from((screen.x - projection.origin.x) * projection.scale) + f64::from(width) * 0.5,
            f64::from((screen.y - projection.origin.y) * projection.scale) + f64::from(height) * 0.5,
        )
    };
    // Which plane covers each pixel, and where in the world that pixel's own
    // fragment sits. `None` is background — this scene draws nothing else.
    let mut covered: Vec<Option<Covered>> = vec![None; (width * height) as usize];
    for (id, slab) in slabs.iter().enumerate() {
        let world = slab.quad();
        let screen: Vec<(f64, f64)> = world.iter().map(|corner| to_pixel(*corner)).collect();
        for triangle in [[0usize, 1, 2], [0, 2, 3]] {
            let (a, b, c) = (screen[triangle[0]], screen[triangle[1]], screen[triangle[2]]);
            let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
            if area.abs() < 1e-12 {
                // Edge-on: a riser seen exactly along its own plane covers no
                // pixel, which is a real answer and not a case to rescue.
                continue;
            }
            let low_x = a.0.min(b.0).min(c.0).floor().max(0.0) as u32;
            let low_y = a.1.min(b.1).min(c.1).floor().max(0.0) as u32;
            let high_x = (a.0.max(b.0).max(c.0).ceil() as i64).clamp(0, i64::from(width)) as u32;
            let high_y = (a.1.max(b.1).max(c.1).ceil() as i64).clamp(0, i64::from(height)) as u32;
            for y in low_y..high_y {
                for x in low_x..high_x {
                    // The pixel's own centre, which is where the rasteriser
                    // samples and therefore where the fragment the engine lit is.
                    let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                    let w0 = ((b.0 - a.0) * (py - a.1) - (b.1 - a.1) * (px - a.0)) / area;
                    let w1 = ((px - a.0) * (c.1 - a.1) - (py - a.1) * (c.0 - a.0)) / area;
                    if w0 < 0.0 || w1 < 0.0 || w0 + w1 > 1.0 {
                        continue;
                    }
                    // The projection is affine and the face is planar, so the
                    // world position interpolates linearly — the same property
                    // `MeshFaceVertex::world` rests on.
                    let (u, v) = (w1, w0);
                    let corner = |at: usize| world[triangle[at]];
                    let (p, q, r) = (corner(0), corner(1), corner(2));
                    let point = (
                        p.0 + (q.0 - p.0) * u + (r.0 - p.0) * v,
                        p.1 + (q.1 - p.1) * u + (r.1 - p.1) * v,
                        p.2 + (q.2 - p.2) * u + (r.2 - p.2) * v,
                    );
                    covered[(y * width + x) as usize] = Some(Covered {
                        plane: id,
                        at:    point,
                    });
                }
            }
        }
    }
    covered
}

/// **What the geometry says is in shadow**, as a picture: the visibility term
/// alone, drawn over [`cover`]'s own rasterisation.
///
/// Every other check in this file reduces the frame to numbers: so many pixels
/// compared, so many disagreeing, banded so. A number cannot say *what the wrong
/// thing looks like*, and a wrong shape is the thing an eye reads instantly and a
/// counter cannot describe at all — "the shadow on this riser has a staircase
/// edge" is not a quantity. So this draws the answer.
///
/// It is a **point** light and a hard shadow: no penumbra, no falloff, no soft
/// crossing. That is a difference from the engine's picture rather than a defect
/// in either, and it is the reason the two are written side by side instead of
/// subtracted — a band of grey along every shadow edge is the engine's flame
/// having a size, and a *shape* that differs is not.
///
/// It judges a **term** and not the light, which is what
/// [`write_light_reference`] beside it is for.
// Nine: the covered pixels, the geometry as surfaces and as volumes, which way
// it climbs, the flame and its reach, the frame's size, and where to write. A
// struct for any subset would be a second spelling of arguments this one
// function is the only caller of.
#[allow(clippy::too_many_arguments)]
fn write_reference(
    covered: &[Option<Covered>],
    slabs: &[Slab],
    bodies: &[Body],
    up: Face,
    flame: (f64, f64, f64),
    radius: f64,
    width: u32,
    height: u32,
    path: &std::path::Path,
) -> Vec<u8> {
    let z_per_tile = f64::from(openshard_client_render::light::Z_PER_TILE);
    let mut shade = vec![0u8; (width * height * 3) as usize];
    for (pixel, drawn) in covered.iter().enumerate() {
        let colour = match drawn {
            None => [0, 0, 0],
            Some(Covered { plane, at: point }) => {
                let offset = (
                    flame.0 - point.0,
                    flame.1 - point.1,
                    (flame.2 - point.2) / z_per_tile,
                );
                let distance = (offset.0 * offset.0 + offset.1 * offset.1 + offset.2 * offset.2).sqrt();
                let slab = &slabs[*plane];
                match (
                    distance >= radius,
                    slab.faces(*point, flame, up),
                    oracle_visible(*point, flame, bodies, primitive_of(bodies, *plane)) > 0.5,
                ) {
                    // The three answers `Shade` decodes, in the colours
                    // `blit.wesl` writes them, so the two pictures can be read
                    // against each other without a legend — and a fourth this
                    // side has and that frame does not.
                    (true, _, _) => [0, 0, 89],
                    // **The flame is behind this surface**, so its shade is
                    // decided before occlusion is asked and neither picture's
                    // answer here is about geometry. Its own colour rather than
                    // one of the three: see `Slab::faces`.
                    (false, false, _) => [40, 40, 64],
                    (false, true, true) => [255, 255, 255],
                    (false, true, false) => [51, 0, 0],
                }
            }
        };
        shade[pixel * 3..pixel * 3 + 3].copy_from_slice(&colour);
    }
    let mut rgb: Vec<u8> = Vec::with_capacity((width * height * 3) as usize);
    rgb.extend_from_slice(&shade);
    openshard_client_render::png::write(path, width, height, &rgb).expect("writing the reference frame");
    eprintln!("wrote {}", path.display());
    shade
}

/// What the reference says one covered pixel is **lit** to.
#[derive(Clone, Copy)]
struct Lit {
    /// What the flames add there, linear and per channel, before the frame's own
    /// clamp — the quantity `blit.wesl`'s `flames` accumulates and `View::Flames`
    /// writes.
    added:     [f32; 3],
    /// How far the nearest flame stands off this fragment's own plane, in tiles —
    /// [`Slab::off_plane`]. Not part of the answer: it is what says whether the
    /// pixel sits inside the band the engine softens `faces` over, and therefore
    /// whether a disagreement here is a defect or a known difference.
    off_plane: f64,
    /// Which plane and which fragment this is, carried so a disagreement can be
    /// **named**. A count with no address is a count nobody can chase: every
    /// wrong attribution on this track came from reading a number and guessing
    /// which surface it was about.
    covered:   Covered,
}

/// **The light itself, from the geometry** — the oracle this track has never had.
///
/// Every other oracle here judges a *term*: `View::Shadow` is `through` alone and
/// [`write_reference`] draws pure visibility. A term that is multiplied by
/// something before it reaches a pixel can be wrong in ways the pixel never
/// shows, and can be judged wrong where the pixel would not have cared — which is
/// exactly how a missing half-space test stood as this track's largest residual
/// for two sessions. This computes what the engine computes, out of the scene's
/// own parameters:
///
/// `colour × intensity × (1 − d)² × visibility × facing`, summed over the flames,
/// with `d` the three-dimensional distance in tiles over the flame's radius.
///
/// Two of those five are the engine's arithmetic re-derived and three are not:
/// the falloff and the pool are stated in `light.rs`'s own doc and restated here,
/// `visibility` is [`oracle_visible`]'s independent slab test, and `facing` is
/// [`Slab::lit_from`] — geometry, the cosine against the plane's own normal.
///
/// That last one used to be a strict half-space against a *band* on the engine's
/// side, and the difference between the two was a class of its own in
/// [`write_light_difference`] with a price printed under it. Phase 3 deleted the
/// band rather than the class: there is one shading term now and both sides
/// compute it, so those pixels are ordinary agreements and the green is gone.
///
/// What is deliberately **not** here is the flame's own size. The engine's is a
/// body a tile across and this is a point, so every shadow edge differs by a
/// penumbra — and that difference is the question the two pictures exist to ask,
/// not a defect to tune away.
///
/// The scene's `Lighting` is read for its **lights** and nothing else: where a
/// flame stands, how far it reaches and how brightly it burns are the scene's own
/// input, not the renderer's answer. Its occlusion grid is never touched.
// Eight, for the same reason [`write_reference`] takes nine: the covered pixels,
// the geometry as surfaces and as volumes, which way it climbs, the lights, the
// frame's size and where to write. A struct for any subset would be a second
// spelling of arguments this one function is the only caller of.
#[allow(clippy::too_many_arguments)]
fn write_light_reference(
    covered: &[Option<Covered>],
    slabs: &[Slab],
    bodies: &[Body],
    up: Face,
    lights: &[Light],
    width: u32,
    height: u32,
    path: &std::path::Path,
) -> Vec<Option<Lit>> {
    let z_per_tile = f64::from(openshard_client_render::light::Z_PER_TILE);
    assert!(
        lights.iter().all(|light| light.beam.is_none()),
        "the reference has no cone: this scene's flame is a beam, and judging its light \
         would need `Beam::lights` re-derived here as well"
    );
    let lit: Vec<Option<Lit>> = covered
        .iter()
        .map(|drawn| {
            let Covered { plane, at: point } = (*drawn)?;
            let slab = &slabs[plane];
            let mut added = [0.0f32; 3];
            let mut nearest = f64::INFINITY;
            let mut off_plane = 0.0;
            for light in lights {
                let flame = (f64::from(light.at.x), f64::from(light.at.y), f64::from(light.z));
                let offset = (
                    flame.0 - point.0,
                    flame.1 - point.1,
                    (flame.2 - point.2) / z_per_tile,
                );
                let distance = (offset.0 * offset.0 + offset.1 * offset.1 + offset.2 * offset.2).sqrt();
                let d = distance / f64::from(light.radius).max(0.001);
                // The nearest flame's stand-off, by the same `d` the shader's own
                // `nearest` is kept by, so this pairs with what `View::Shadow`
                // reports for the same pixel.
                if d < nearest {
                    nearest = d;
                    off_plane = slab.off_plane(point, flame, up);
                }
                let facing = slab.lit_from(point, flame, up) as f32;
                if d >= 1.0 || facing <= 0.0 {
                    continue;
                }
                // **Scaled by the share, not gated on it** — phase 5. This is
                // the reference the engine's own `View::Flames` is judged
                // against, and the engine multiplies by how much of the flame it
                // can see; a reference that switched at a half would call every
                // penumbra pixel a disagreement of up to half a flame.
                let seen = oracle_visible(point, flame, bodies, primitive_of(bodies, plane)) as f32;
                if seen <= 0.0 {
                    continue;
                }
                let fall = (1.0 - d) as f32;
                for (channel, colour) in added.iter_mut().zip(light.color) {
                    *channel += colour * light.intensity * fall * fall * facing * seen;
                }
            }
            Some(Lit {
                added,
                off_plane,
                covered: Covered { plane, at: point },
            })
        })
        .collect();

    let mut rgb: Vec<u8> = Vec::with_capacity((width * height * 3) as usize);
    for pixel in &lit {
        // The frame's own clamp, so the two pictures are the same quantity in the
        // same units: `View::Flames` clamps to `0..=1` and writes eight bits.
        let colour = pixel.map_or([0, 0, 0], |Lit { added, .. }| {
            added.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8)
        });
        rgb.extend_from_slice(&colour);
    }
    openshard_client_render::png::write(path, width, height, &rgb)
        .expect("writing the light reference frame");
    eprintln!("wrote {}", path.display());
    lit
}

/// Where the rendered `View::Flames` frame and [`write_light_reference`]'s own
/// disagree **about how bright a pixel is**, as a picture and as a price list.
///
/// A pixel here is one of five things, and four of them are not "the renderer is
/// wrong":
///
/// - **grey** — the two agree to `TOLERANCE`, drawn at the brightness they agree
///   on so the scene's own shape stays readable underneath;
/// - **red** — the renderer is brighter than the geometry allows, **blue** —
///   darker, both scaled by how far apart they are;
/// - **magenta** — the engine's own `through` is strictly between nothing and
///   everything, which is its flame having a size. Read off the `View::Shadow`
///   frame rather than guessed at, so it is the engine's own statement of where
///   its penumbra is;
/// - **olive** — the reference is at or over the frame's ceiling, where eight
///   bits cannot say how much brighter one side is;
/// - **yellow** — the two rasterisers gave this pixel to **different planes**, so
///   they are not lighting the same surface and the two numbers are not
///   comparable. Which plane a pixel belongs to is asked of the `place`
///   attachment — the renderer's own answer — and never inferred from this file's
///   own painter order. It cost a wrong reading to learn: at a tread's own top
///   edge the engine draws the **lid** and this file's order draws the **riser**,
///   the lid's normal points up, the flame stood at exactly that lid's height, so
///   the engine's `faces` was `0.5` and the reference's was `1.0` — a clean factor
///   of two that reads exactly like a lighting defect and is a disagreement about
///   whose pixel it is;
/// - **orange / cyan** — only one of the two drew anything at all, [`write_difference`]'s
///   own two classes and the same two colours.
#[allow(clippy::too_many_arguments)]
fn write_light_difference(
    rendered: &[u8],
    reference: &[Option<Lit>],
    shadow: &[u8],
    drawn: &[oracle::Drawn],
    slabs: &[Slab],
    width: u32,
    height: u32,
    path: &std::path::Path,
) {
    /// How far apart the two may be and still be called the same number: two
    /// steps of the eight-bit frame both sides are read out of. One step is the
    /// quantisation itself and the second is the two summations rounding the same
    /// products differently — anything above that is arithmetic, not format.
    const TOLERANCE: f32 = 2.0 / 255.0;

    let mut agreed = 0usize;
    let mut brighter = 0usize;
    let mut darker = 0usize;
    let mut penumbra = 0usize;
    let mut clipped = 0usize;
    let mut disputed = 0usize;
    let mut renderer_alone = 0usize;
    let mut reference_alone = 0usize;
    let mut worst = 0.0f32;
    let mut examples: Vec<(usize, String)> = Vec::new();

    let mut rgb: Vec<u8> = Vec::with_capacity((width * height * 3) as usize);
    for pixel in 0..(width * height) as usize {
        let theirs = [
            f32::from(rendered[pixel * 4]) / 255.0,
            f32::from(rendered[pixel * 4 + 1]) / 255.0,
            f32::from(rendered[pixel * 4 + 2]) / 255.0,
        ];
        // A pixel the renderer left at the cleared background is one it drew
        // nothing on; a flames frame is black where a fragment is lit by no
        // flame, so "drew nothing" has to come from the place attachment's own
        // answer instead — which is what the `kind` byte of the shadow frame
        // carries here, since that view paints every drawn fragment one of three
        // colours and none of them is black.
        let drew_theirs = shadow[pixel * 4..pixel * 4 + 3] != [0, 0, 0];
        let colour = match (drew_theirs, reference[pixel]) {
            (false, None) => [0, 0, 0],
            (true, None) => {
                renderer_alone += 1;
                [235, 140, 0]
            }
            (false, Some(_)) => {
                reference_alone += 1;
                [0, 200, 200]
            }
            (
                true,
                Some(Lit {
                    added,
                    off_plane,
                    covered: Covered { plane, at },
                }),
            ) => {
                let mine = added.map(|channel| channel.clamp(0.0, 1.0));
                let apart = theirs
                    .iter()
                    .zip(mine)
                    .map(|(engine, geometry)| (engine - geometry).abs())
                    .fold(0.0f32, f32::max);
                let signed = theirs[0] - mine[0];
                let through = Shade::of([shadow[pixel * 4], shadow[pixel * 4 + 1], shadow[pixel * 4 + 2]]);
                let soft = matches!(through, Shade::Through(value) if value < 255);
                // Whose pixel the *renderer* says this is. A mesh face's row is
                // addressed through the `MeshFace` sentinel — `place::Stance`'s
                // own doc — so all three of kind, sentinel and row have to match
                // the plane this file's rasteriser chose.
                let texel = &drawn[pixel];
                let same_plane = texel.kind == Kind::Static as u32
                    && texel.stance == Stance::MeshFace as u32
                    && texel.id as usize == plane;
                match (
                    !same_plane,
                    added.iter().any(|channel| *channel >= 1.0),
                    soft,
                    apart <= TOLERANCE,
                ) {
                    // First of all, because every class below is a statement
                    // about one surface and this one says there are two.
                    (true, ..) => {
                        disputed += 1;
                        [200, 200, 0]
                    }
                    (_, true, ..) => {
                        clipped += 1;
                        [140, 140, 0]
                    }
                    (_, _, true, _) => {
                        penumbra += 1;
                        [140, 0, 140]
                    }
                    (_, _, _, true) => {
                        agreed += 1;
                        let value = (theirs[0] * 90.0) as u8;
                        [value, value, value]
                    }
                    _ => {
                        worst = worst.max(apart);
                        // Two of each sign, because the two are opposite defects
                        // and a list that fills up with whichever comes first in
                        // scan order names only one of them.
                        let sign = usize::from(signed > 0.0);
                        if examples.iter().filter(|(had, _)| *had == sign).count() < 2 {
                            let through = match through {
                                Shade::Unreached => "no flame reaches".to_string(),
                                Shade::Blocked => "fully blocked".to_string(),
                                Shade::Through(value) => format!("through {value}/255"),
                            };
                            examples.push((
                                sign,
                                format!(
                                    "  [{}] at ({:.2}, {:.2}, z {:.2}): rendered {:.3}, geometry \
                                     {:.3}, {through}, flame {:.3} tiles off this plane",
                                    slabs[plane].label(),
                                    at.0,
                                    at.1,
                                    at.2,
                                    theirs[0],
                                    mine[0],
                                    off_plane,
                                ),
                            ));
                        }
                        let strength = 100 + (155.0 * (apart * 4.0).min(1.0)) as u8;
                        match signed > 0.0 {
                            true => {
                                brighter += 1;
                                [strength, 40, 40]
                            }
                            false => {
                                darker += 1;
                                [40, 80, strength]
                            }
                        }
                    }
                }
            }
        };
        rgb.extend_from_slice(&colour);
    }
    openshard_client_render::png::write(path, width, height, &rgb)
        .expect("writing the light difference frame");
    eprintln!("wrote {}", path.display());
    eprintln!(
        "light oracle vs rendered View::Flames: {} of {} judged pixels differ by more than \
         {TOLERANCE:.3} ({brighter} rendered brighter, {darker} darker, worst {worst:.3}); \
         set aside: {penumbra} in the engine's own penumbra, \
         {clipped} at the frame's ceiling, {disputed} given to different planes by the two \
         rasterisers, {renderer_alone}/{reference_alone} drawn by one side only",
        brighter + darker,
        agreed + brighter + darker,
    );
    for (_, example) in &examples {
        eprintln!("{example}");
    }
}

/// Where the rendered frame and [`write_reference`]'s own disagree, as a picture.
///
/// Five colours and a count of each. Grey is agreement, and it is deliberately
/// dim so that anything else is the first thing an eye lands on:
///
/// - **red** — the renderer lit a pixel the geometry says is shadowed;
/// - **blue** — the renderer shadowed one the geometry says is lit;
/// - **orange** — only the *renderer* drew anything here;
/// - **cyan** — only the *reference* did.
///
/// The last two are one class of question — a disagreement about the **shape**
/// rather than about the light — and they were one colour until the picture was
/// looked at: an outline a couple of pixels wide runs the whole way round the
/// silhouette on every frame of a flame-height sweep, `1458` pixels of it, which
/// is thirty-five times the largest disagreement the counting oracle beside it
/// ever reports. A single colour says "the two draw different shapes" and cannot
/// say **which of them is the wider one**, and that is the only thing worth
/// knowing about an outline. Two colours answer it by looking.
///
/// This is the class a count of "compared pixels" hides completely, because a
/// pixel nobody compared is a pixel nobody counted — so this one counts, and
/// prints what it counted beside the picture.
fn write_difference(rendered: &[u8], reference: &[u8], width: u32, height: u32, path: &std::path::Path) {
    let mut rgb: Vec<u8> = Vec::with_capacity((width * height * 3) as usize);
    let mut renderer_alone = 0usize;
    let mut reference_alone = 0usize;
    for pixel in 0..(width * height) as usize {
        let theirs = [
            rendered[pixel * 4],
            rendered[pixel * 4 + 1],
            rendered[pixel * 4 + 2],
        ];
        let mine = &reference[pixel * 3..pixel * 3 + 3];
        let drew_theirs = Shade::of(theirs);
        let drew_mine = mine != [0, 0, 0];
        // The flame stands behind this surface, so neither picture's answer
        // here is about geometry — `Slab::faces`. Its own dim colour, and it is
        // never a disagreement.
        let behind = mine == [40, 40, 64];
        let colour = match (theirs == [0, 0, 0], drew_mine) {
            (true, false) => [0, 0, 0],
            (false, false) => {
                renderer_alone += 1;
                [235, 140, 0]
            }
            (true, true) => {
                reference_alone += 1;
                [0, 200, 200]
            }
            _ if behind => [30, 30, 46],
            (false, true) => {
                let lit = drew_theirs.lit();
                let should = mine == [255, 255, 255];
                match (lit, should) {
                    (true, false) => [230, 40, 40],
                    (false, true) => [60, 120, 230],
                    _ => [40, 40, 40],
                }
            }
        };
        rgb.extend_from_slice(&colour);
    }
    openshard_client_render::png::write(path, width, height, &rgb).expect("writing the difference frame");
    eprintln!("wrote {}", path.display());
    eprintln!(
        "difference: {renderer_alone} pixels the renderer drew and the geometry does not cover, \
         {reference_alone} the other way round"
    );
}

/// One colour per plane of the run, over the pixels the `place` attachment says
/// that plane drew — the same field the oracle judges, drawn instead of counted.
///
/// A top and its own riser are deliberately **near** each other in hue and a
/// tread away is far: what is worth seeing here is a face landing where the one
/// beside it should be, and two planes that neighbour in the world reading as two
/// unrelated colours makes every honest boundary as loud as a wrong one. Tops are
/// the bright half of a pair and risers the dim half, so a riser drawn over the
/// tread it stands on — `Prism::mesh`'s `SEAM_OVERLAP`, which is real pixels of a
/// riser under a tread — shows as a dark hairline inside a bright band rather
/// than as another edge.
///
/// Everything that is not one of the run's own faces is black: the ground, the
/// background, and any pixel whose row is outside `slabs`.
fn write_face_map(drawn: &[oracle::Drawn], slabs: &[Slab], width: u32, height: u32, path: &std::path::Path) {
    let mut rgb: Vec<u8> = Vec::with_capacity((width * height * 3) as usize);
    for texel in drawn {
        let of = |id: usize| -> [u8; 3] {
            let slab = &slabs[id];
            // Three primaries by flight, shaded by tread, halved for a riser.
            let step = 255 - (slab.tread as u32 * 60).min(180) as u8;
            let value = match slab.part {
                Part::Top => step,
                Part::Riser => step / 2,
            };
            match slab.flight % 3 {
                0 => [value, value / 4, value / 4],
                1 => [value / 4, value, value / 4],
                _ => [value / 4, value / 4, value],
            }
        };
        let colour = match texel.kind == Kind::Static as u32
            && texel.stance == Stance::MeshFace as u32
            && (texel.id as usize) < slabs.len()
        {
            true => of(texel.id as usize),
            false => [0, 0, 0],
        };
        rgb.extend_from_slice(&colour);
    }
    openshard_client_render::png::write(path, width, height, &rgb).expect("writing the face map");
    eprintln!("wrote {}", path.display());
}

fn main() {
    let (device, queue) = gpu().expect("an adapter");

    let up = parse_face(&env_or("OPENSHARD_STAIR_UP", "north"));
    let treads = parse_treads(&env_or("OPENSHARD_STAIR_TREADS", "1,3,5"));
    let prism = Prism::new(up, &treads).expect("1..=MAX_TREADS heights");
    let run: u16 = env_or("OPENSHARD_STAIR_RUN", "1").parse().expect("a number");
    assert!(run >= 1, "OPENSHARD_STAIR_RUN wants at least one flight");
    let at = Point::new(100, 100, 0);

    let stair = StaticTile {
        flags: TileFlags::new(TileFlags::NO_SHOOT | TileFlags::CLIMBABLE),
        height: 20,
        ..StaticTile::default()
    };
    // Where each flight of the run stands. **Across** the climb, never along
    // it: side by side, every flight's treads meet its neighbour's at a tile
    // boundary *at the same height*, which is the arrangement a wide staircase
    // has and the one question a single flight cannot pose — two abutting
    // treads at one `z` are different statics, therefore different owners, so
    // identity does not answer for them and `own_run`'s mask is what does.
    let flights: Vec<Point> = (0..run)
        .map(|step| {
            match up {
                Face::North | Face::South => Point::new(at.x + step, at.y, at.z),
                Face::East | Face::West => Point::new(at.x, at.y + step, at.z),
            }
        })
        .collect();
    let bounds = openshard_client_render::camera::TileBounds {
        min_x: 90,
        max_x: 110,
        min_y: 90,
        max_y: 110,
    };
    let mut builder = Builder::new(bounds);
    for stands in &flights {
        builder.add(stands.x, stands.y, stands.z, STAIR, &stair, Shape::solid(prism));
    }
    let occlusion = builder.finish(&Cutaway::OPEN);

    for stands in &flights {
        for solid in occlusion.solids_at(i32::from(stands.x), i32::from(stands.y)) {
            eprintln!(
                "solid ({}, {}): x {:.3}..{:.3}, y {:.3}..{:.3}, z {:.1}..{:.1}, edges {:#06b}",
                stands.x,
                stands.y,
                solid.space.min.x,
                solid.space.max.x,
                solid.space.min.y,
                solid.space.max.y,
                solid.space.min.z,
                solid.space.max.z,
                solid.edges.raw(),
            );
        }
    }

    // The oracle's own statement of the same geometry, derived from the profile
    // and immediately held against the grid's — as **volumes**, which is what
    // the grid holds and what a ray travels through. See [`Body`] and [`strip`].
    let bodies: Vec<Body> = flights
        .iter()
        .enumerate()
        .flat_map(|(flight, stands)| flight_bodies(flight, *stands, up, &treads))
        .collect();
    gate_against_grid(&bodies, &flights, up, &occlusion);

    // And as **surfaces**, which is what the mesh draws and what the `place`
    // attachment names a pixel by — each one held against the body it is a face
    // of, so the two derivations cannot say different staircases.
    let slabs: Vec<Slab> = flights
        .iter()
        .enumerate()
        .flat_map(|(flight, stands)| flight_slabs(flight, *stands, up, &treads))
        .collect();
    for (plane, slab) in slabs.iter().enumerate() {
        gate_plane_of_body(slab, &bodies[body_of(plane)], up);
    }

    // A flight is **one owner** of its tile however many treads it was cut into
    // — one `Builder::add` is one owner (`docs/archive/render/lighting_height.md` phase 3), so
    // every face of it carries this one number. What it is *not* is one
    // occluder: `docs/render/design_model.md` phase 4 made the exemption a
    // `SolidId`, owner **and** part, precisely because a tread does shadow the
    // treads below it and an owner alone could not say so.
    //
    // **The flights of a run share that owner**, and this used to say the
    // opposite — "each flight gets its own, which is the whole point of building
    // the run: neighbours are not each other's". An `Owner` is a `(z, graphic)`
    // and carries no tile, so a run standing at one height under one graphic is
    // one owner however wide it is; that is not a detail of this fixture but the
    // condition `occlusion::merge` folds on, and the sentence was wrong from the
    // day the merge landed. What the run still poses, and a single flight cannot,
    // is the two-abutting-statics question — it is now posed as *one* primitive
    // where there were two, which is what `docs/render/design_occluders.md`'s D6 is about and
    // what phase 6h turned out to be.
    let owners: Vec<Owner> = flights
        .iter()
        .map(|stands| {
            let owner = Owner::new(stands.z, STAIR);
            assert!(
                occlusion
                    .id_of(
                        i32::from(stands.x),
                        i32::from(stands.y),
                        owner,
                        grid::Part::nth(0)
                    )
                    .is_some(),
                "the flight at ({}, {}) is not in the grid this tool built",
                stands.x,
                stands.y,
            );
            owner
        })
        .collect();

    let (width, height): (u32, u32) = (512, 512);
    let zoom_notches: u32 = env_or("OPENSHARD_SCENE_ZOOM", "3").parse().expect("a number");
    // On the middle of the run, so a run of three is not half off the frame.
    let mut camera = Camera::new(flights[flights.len() / 2], width, height);
    let mut zoom = Zoom::ONE;
    for _ in 0..zoom_notches {
        zoom = zoom.scale_up();
    }
    camera.zoom_about(RealPixel::new((width / 2) as i32, (height / 2) as i32), zoom);

    const DEPTH: f32 = 0.5;
    let mut vertices: Vec<MeshFaceVertex> = Vec::new();
    let mut rows: Vec<MeshFaceRow> = Vec::new();
    for (flight, stands) in flights.iter().enumerate() {
        let mesh = prism.mesh(i32::from(stands.x), i32::from(stands.y), i32::from(stands.z));
        for (part, face) in mesh.faces().iter().enumerate() {
            // Row `id` draws plane `slabs[id]`: the two lists are built by one
            // pass over the same flights in the same order, and this is where
            // that is checked instead of trusted. Everything the oracle below
            // does rests on it — the id word names a row, and a row has to name
            // a plane for "whose pixel is this" to have an answer.
            let id = rows.len() as u32;
            gate_against_mesh(&slabs[rows.len()], face, up);
            // And which *solid* of the grid this face is, which is what the walk
            // compares a fragment against — `docs/render/design_model.md` phase 4.
            // **Two drawn faces a tread, one solid a tread**: `Prism::mesh`
            // emits a top and then the rise below it for each, and phase 6 put
            // the grid's tread back to one body, so the join divides.
            // `occlusion::tests::a_flight_draws_its_own_solids_in_the_grid_s_own_order`
            // gates the same join on the grid's own side.
            let solid = occlusion
                .id_of(
                    i32::from(stands.x),
                    i32::from(stands.y),
                    owners[flight],
                    grid::Part::nth(part / 2),
                )
                .expect("a flight's own solid per tread");
            rows.push(MeshFaceRow {
                tile:   (stands.x, stands.y),
                stance: Stance::of_normal(face.normal).expect("a stair's own normals are all recognized"),
                solid:  solid.raw(),
            });
            for corner in face.fan() {
                let screen = camera.to_view_exact(project_exact(corner));
                vertices.push(MeshFaceVertex {
                    screen,
                    world: [corner.x as f32, corner.y as f32, corner.z as f32],
                    depth: DEPTH,
                    id,
                    tile: [f32::from(stands.x), f32::from(stands.y)],
                    normal: face.normal,
                    // This tool's own oracle reads visibility off `View::Shadow`,
                    // never brightness — a flat mid-grey is not a claim about
                    // anything it measures.
                    colour: [0.5, 0.5, 0.5],
                });
            }
        }
    }
    eprintln!(
        "{} flights, {} faces, {} vertices",
        flights.len(),
        rows.len(),
        vertices.len()
    );
    for (id, row) in rows.iter().enumerate() {
        let corners: Vec<&MeshFaceVertex> = vertices.iter().filter(|v| v.id == id as u32).collect();
        let (mut minx, mut maxx, mut miny, mut maxy) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for c in &corners {
            minx = minx.min(c.screen.x);
            maxx = maxx.max(c.screen.x);
            miny = miny.min(c.screen.y);
            maxy = maxy.max(c.screen.y);
        }
        eprintln!(
            "face {id}: {}, tile ({}, {}), solid {}, stance {:?}, screen x {minx:.1}..{maxx:.1}, y {miny:.1}..{maxy:.1}",
            slabs[id].label(),
            row.tile.0,
            row.tile.1,
            row.solid,
            row.stance,
        );
    }

    let format = openshard_client_render::blit::WORLD_FORMAT;
    let world = openshard_client_render::blit::world_texture(&device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(&device, width, height);
    let gbuffer_views = gbuffer.views();
    let depth_tex = renderer::depth_texture(&device, width, height);
    let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let land = openshard_client_render::atlas::LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = openshard_client_render::atlas::TexmapAtlas::pack([]).expect("nothing always fits");
    let mut ground_pass = GroundRenderer::new(&device, &queue, format, &land, &texmaps);
    let mut mesh_pass = MeshFaceRenderer::new(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target = Target {
        gbuffer: &gbuffer_views,
        view: &world_view,
        depth: &depth_view,
        width,
        height,
        projection: camera.projection(),
    };
    ground_pass.render(&device, &queue, &mut encoder, target, &[]);
    mesh_pass.render(&device, &queue, &mut encoder, target, &vertices, &rows);
    queue.submit([encoder.finish()]);

    // What the world passes left on each pixel: the renderer's own answer to
    // "whose pixel is this, and where in the world is its fragment". Read once,
    // here, because the blit below neither writes it nor changes it however
    // many views are dumped.
    let drawn = read_gbuffer(&device, &queue, &gbuffer, width, height);

    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let surface_view = surface.create_view(&wgpu::TextureViewDescriptor::default());
    let mut blit = Blit::new(&device, format);

    let (ldx, ldy) = parse_pair(&env_or("OPENSHARD_LIGHT_AT", "2.5,1.0"));
    let light_z: f32 = env_or("OPENSHARD_LIGHT_Z", "2").parse().expect("a number");
    let light_radius: f32 = env_or("OPENSHARD_LIGHT_RADIUS", "6").parse().expect("a number");
    // Off the **last** flight of the run, so the default stands beside the
    // staircase however wide it is rather than inside it — see the header's own
    // entry for this knob. One flight is one tile and the anchor is that tile.
    let lit_from = *flights.last().expect("a run of at least one flight");
    let flame_at = Vec2::new(f32::from(lit_from.x) + ldx, f32::from(lit_from.y) + ldy);
    eprintln!(
        "light: at ({ldx:+}, {ldy:+}) of tile ({}, {}) — ({}, {}), z {light_z}, radius {light_radius}",
        lit_from.x, lit_from.y, flame_at.x, flame_at.y,
    );
    let selected = View::ALL[env_or("OPENSHARD_FRAME_VIEW", "0")
        .parse::<usize>()
        .expect("an index")];
    let mut lighting = Lighting {
        ambient: openshard_client_render::light::NIGHT,
        lights: vec![Light {
            at:        flame_at,
            z:         light_z,
            radius:    light_radius,
            color:     [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam:      None,
        }],
        occlusion,
        sun: None,
        view: selected,
        flame_radius: openshard_client_render::light::FLAME_RADIUS,
        shadow_rays: openshard_client_render::light::ShadowRays::DEFAULT,
        dead: false,
    };
    // The CPU twin of a pixel, on demand. A picture says a fragment came out
    // black; this says *what* took its ray, by name — and after
    // `docs/render/design_model.md` phase 4 the name that decides the answer is the
    // **solid**, so a probe carries one exactly as the mesh rows above do. A
    // probe carrying none would be a point of nothing and would answer a
    // question no pixel of this scene asks.
    //
    // Which solid is found by *geometry* here and not by a part number, because
    // a person types a position: the flight's own solid whose box holds the
    // point. It is printed rather than assumed — a point on the seam between a
    // tread's top and its riser is legitimately in both boxes, and the report
    // says which one the probe took.
    if let Some(spec) = env_opt("OPENSHARD_STAIR_PROBE") {
        for one in spec.split(';').filter(|s| !s.trim().is_empty()) {
            let mut fields = one.split(',');
            let mut number = |what: &str| -> f32 {
                fields
                    .next()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or_else(|| panic!("OPENSHARD_STAIR_PROBE wants {what} in {one:?}"))
            };
            let (px, py, pz) = (number("x"), number("y"), number("z"));
            let surface = match fields.next().map(str::trim) {
                Some("flat") | None => Surface::Flat,
                Some("upright") => Surface::Upright,
                Some(other) => Surface::Face(parse_face(other)),
            };
            // The tile under the point, and *that* tile's own solids. A drawn
            // fragment is a point of a solid standing on the tile its own static
            // stands on, so a probe that borrowed a neighbour's would be asking a
            // question no pixel asks. A point on a tile boundary belongs to
            // whichever side `floor` picks, which is a real ambiguity — so the
            // tile and the solid are both printed rather than assumed.
            let tile = (px.floor() as i32, py.floor() as i32);
            let holds = |space: &openshard_client_render::solid::Solid| {
                f64::from(px) >= space.min.x
                    && f64::from(px) <= space.max.x
                    && f64::from(py) >= space.min.y
                    && f64::from(py) <= space.max.y
                    && f64::from(pz) >= space.min.z
                    && f64::from(pz) <= space.max.z
            };
            let solid = lighting
                .occlusion
                .cell(tile.0, tile.1)
                .find(|(_, stands)| stands.owner == Owner::new(at.z, STAIR) && holds(&stands.space))
                .map(|(id, _)| id);
            let spot = light::Spot {
                at: Vec2::new(px, py),
                z: pz,
                tile,
                surface,
                solid,
            };
            eprint!(
                "probe {surface:?} on ({}, {}) solid {}: {}",
                tile.0,
                tile.1,
                match solid {
                    Some(id) => id.raw().to_string(),
                    None => "none".to_string(),
                },
                light::sample(spot, &lighting),
            );
        }
    }

    // Where the flame itself projects to, marked directly on every picture this
    // tool writes: a number in a log line does not answer "is the light behind
    // the stair or in front of it" nearly as fast as a mark on the frame does.
    let projection = camera.projection();
    let light_screen = camera.to_view_exact(project_exact(WorldSpot {
        x: f64::from(flame_at.x),
        y: f64::from(flame_at.y),
        z: f64::from(light_z),
    }));
    let light_pixel = (
        (light_screen.x - projection.origin.x) * projection.scale + width as f32 * 0.5,
        (light_screen.y - projection.origin.y) * projection.scale + height as f32 * 0.5,
    );
    eprintln!("light pixel: {light_pixel:?}");
    let light_mark = (light_pixel.0.round() as i32, light_pixel.1.round() as i32);

    let dumped = env_opt("OPENSHARD_FRAME_DUMP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("synthetic_stair.png"));
    let oracle_on = env_opt("OPENSHARD_STAIR_ORACLE").as_deref() != Some("0");
    // The view that was asked for, and `Shadow` besides when the oracle needs
    // it — the oracle reads that frame back pixel for pixel, so leaving it out
    // would silently disarm the one check here that does not depend on anybody
    // looking at a picture.
    let mut views = vec![selected];
    if oracle_on && selected != View::Shadow {
        views.push(View::Shadow);
    }
    // And `Flames`, which is what the light oracle judges: the pools' own
    // contribution with the ambient left out and no curve over it, so a byte in
    // that frame is the number the shader added and can be compared with a number
    // rather than with a threshold. `Light` beside it is the same quantity seen
    // through `knee` and with the ambient in, which is what an eye should look at
    // and not what an oracle can subtract.
    if oracle_on && selected != View::Flames {
        views.push(View::Flames);
    }
    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    let dummy_ground_instances = openshard_client_render::blit::dummy_ground_instances(&device);
    let mut shadow_pixels: Vec<u8> = Vec::new();
    let mut flame_pixels: Vec<u8> = Vec::new();
    for view in views {
        lighting.view = view;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        blit.render(
            &device,
            &queue,
            &mut encoder,
            openshard_client_render::blit::Frame {
                target:           &surface_view,
                world:            &world_view,
                gbuffer:          &gbuffer_views,
                face_instances:   &dummy_instances,
                item_instances:   &dummy_instances,
                mobile_instances: &dummy_instances,
                mesh_instances:   mesh_pass.rows_buffer(),
                ground_instances: &dummy_ground_instances,
                zoom:             Zoom::ONE,
                rect:             ViewportRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
            },
            &lighting,
        );
        queue.submit([encoder.finish()]);
        let path = match view == selected {
            true => dumped.clone(),
            false => beside(&dumped, view.name()),
        };
        let pixels = dump(&device, &queue, &surface, width, height, &path, Some(light_mark));
        match view {
            View::Shadow => shadow_pixels = pixels,
            View::Flames => flame_pixels = pixels,
            _ => {}
        }
    }

    // **Which face drew each pixel**, as a picture, beside the frame that was
    // asked for. The oracle below reads exactly this and reports it as counts;
    // a count cannot answer "why is there a white strip down the east side", and
    // that question is asked of a *drawing* rather than of the lighting. One
    // colour a plane, so a face landing where it should not — `Prism::mesh`'s
    // `SEAM_OVERLAP` painting a riser over the tread it stands on, or
    // `WIDTH_OVERLAP` poking a face past its own tile — reads off the picture
    // instead of being argued about.
    //
    // **Above `OPENSHARD_STAIR_ORACLE`'s own early return**, since it answers a
    // question about the drawing and not about the lighting: a sweep asking "does
    // this mesh ever leave a hole" wants the map without paying for a per-pixel
    // visibility oracle it is not reading.
    write_face_map(&drawn, &slabs, width, height, &beside(&dumped, "faces"));

    if !oracle_on {
        return;
    }

    // **Off the light the renderer was handed and not off the offset again.**
    // This was `at + (ldx, ldy)` — a third spelling of the flame's position
    // beside the `Light` above and the crosshair below — and the three agreed
    // only for as long as they were the same expression. Moving the anchor to
    // the run's last tile moved one of them, and the face oracle immediately
    // reported 1,375 pixels of a three-flight run as the renderer's fault: an
    // oracle lighting a scene from somewhere the renderer did not.
    let flame = (f64::from(flame_at.x), f64::from(flame_at.y), f64::from(light_z));
    // The scene drawn again from the geometry, and where the two pictures differ
    // — see `write_reference`, and the module header's own note about why a count
    // cannot describe a shape.
    let covered = cover(&slabs, &camera, width, height);
    let reference = write_reference(
        &covered,
        &slabs,
        &bodies,
        up,
        flame,
        f64::from(light_radius),
        width,
        height,
        &beside(&dumped, "reference"),
    );
    write_difference(
        &shadow_pixels,
        &reference,
        width,
        height,
        &beside(&dumped, "difference"),
    );
    // And the same scene judged as **light** rather than as a visibility term —
    // the two pictures above answer "is anything in the way", which is one factor
    // of five in what a person sees.
    let lit = write_light_reference(
        &covered,
        &slabs,
        &bodies,
        up,
        &lighting.lights,
        width,
        height,
        &beside(&dumped, "reference_light"),
    );
    write_light_difference(
        &flame_pixels,
        &lit,
        &shadow_pixels,
        &drawn,
        &slabs,
        width,
        height,
        &beside(&dumped, "difference_light"),
    );

    // The face oracle. See this module's own doc for what it is and why it
    // comes before the fix rather than after it.
    // How many bands to report a face's disagreements in, up its own varying
    // axis. Not a sampling grid — the sweep is exhaustive over the face's own
    // pixels — only the resolution the "where" line reads at.
    let bands = 32usize;
    let riser_face = descends_towards(up);
    let mut total_compared = 0usize;
    // Split by **sign**, because the two shapes this scene shows are opposite
    // signs on opposite faces and a single total cannot tell them apart: a
    // tread's top rendered darker than the geometry allows is phase 4's own lid,
    // and a riser's own top band rendered lighter is what `STAND_OFF` costs at
    // the corner where that riser meets the tread above it.
    let mut total_too_dark = 0usize;
    let mut total_too_light = 0usize;
    let mut total_unreached = 0usize;
    for (id, slab) in slabs.iter().enumerate() {
        let mut compared = 0usize;
        let mut unreached = 0usize;
        let mut behind_the_flame = 0usize;
        let mut too_dark = 0usize;
        let mut too_light = 0usize;
        // And which of the two walks is out, on every disagreement.
        // `light::sample` is the CPU's own preview of exactly what the shader
        // does (`docs/archive/render/lighting.md` decision 9 holds the two to each other), so a
        // disagreement where it sides with the independent oracle is the
        // *shader* alone being out — a parity gap — and one where it sides with
        // the rendered pixel is the engine's own arithmetic being out, in both
        // implementations at once. Those are opposite next steps, and a count
        // that does not tell them apart names neither.
        let mut shader_alone = 0usize;
        let mut engine_together = 0usize;
        let mut disagreeing_bands = vec![0usize; bands];
        // The seam, counted as its own class rather than left inside the total.
        // See [`Slab::beyond_its_plane`].
        let mut beyond = 0usize;
        let mut beyond_disagreeing = 0usize;
        let mut examples: Vec<String> = Vec::new();
        for (pixel, texel) in drawn.iter().enumerate() {
            // Whose pixel this is, as the renderer wrote it. A mesh face's row
            // is addressed through the `MeshFace` sentinel — `place::Stance`'s
            // own doc — so all three of kind, sentinel and row have to be this
            // face's.
            if texel.kind != Kind::Static as u32
                || texel.stance != Stance::MeshFace as u32
                || texel.id != id as u32
            {
                continue;
            }
            // The fragment's own world position, read whole off the position
            // plane. This is the point the shader lit — exactly, now: the tile
            // and the fraction it used to be added up from were each quantised
            // on the way into the attachment that carried them.
            let point = texel.at;
            let shade = Shade::of([
                shadow_pixels[pixel * 4],
                shadow_pixels[pixel * 4 + 1],
                shadow_pixels[pixel * 4 + 2],
            ]);
            // A fragment outside every pool is dark because of a radius, and
            // this oracle answers about geometry alone. Counted, so that a face
            // that fell out of reach cannot read as a face that agreed.
            if shade == Shade::Unreached {
                unreached += 1;
                continue;
            }
            // And the other class this oracle has no opinion about: a surface
            // the flame stands *behind*. Its shade is decided by the facing term
            // before occlusion is ever asked, so measuring the occlusion term
            // there measures this oracle's own missing half-space test. Counted
            // apart, exactly as `Unreached` is, and for the same reason —
            // `Slab::faces`.
            if !slab.faces(point, flame, up) {
                behind_the_flame += 1;
                continue;
            }
            compared += 1;
            let seam = slab.beyond_its_plane(point, up);
            if seam {
                beyond += 1;
            }
            let rendered_lit = shade.lit();
            let independent = oracle_visible(point, flame, &bodies, primitive_of(&bodies, id)) > 0.5;
            if independent == rendered_lit {
                continue;
            }
            if seam {
                beyond_disagreeing += 1;
            }
            let band = (slab.along(point, up) * bands as f64) as usize;
            disagreeing_bands[band.min(bands - 1)] += 1;
            let surface = match slab.part {
                Part::Top => Surface::Flat,
                Part::Riser => Surface::Face(riser_face),
            };
            // The cell the walk starts from is the mesh face's own tile, off the
            // row this pixel's id names — not `floor` of the position. And so is
            // the solid: the row carries the name of the one it draws, so this
            // probe is a point of exactly what the rendered fragment is a point
            // of, with nothing re-derived. `docs/render/design_model.md` phase 4.
            let row = &rows[texel.id as usize];
            let spot = light::Spot {
                at: Vec2::new(point.0 as f32, point.1 as f32),
                z: point.2 as f32,
                tile: (i32::from(row.tile.0), i32::from(row.tile.1)),
                surface,
                solid: Some(SolidId::new(row.solid)),
            };
            let sampled = light::sample(spot, &lighting);
            let through = sampled
                .reaches
                .first()
                .map_or(0.0, |reach| if reach.within { reach.through } else { 0.0 });
            match (through > 0.5) == independent {
                true => shader_alone += 1,
                false => engine_together += 1,
            }
            match rendered_lit {
                true => too_light += 1,
                false => too_dark += 1,
            }
            if examples.len() < 2 {
                // `Sample`'s own report and not a re-derivation of it: it names
                // the solid that stopped the ray and how that solid stands to
                // this fragment, which is the pair phase 4's instrument exists
                // to print.
                examples.push(format!(
                    "  [{}] independent oracle says {}, rendered says {}\n    {sampled}",
                    slab.label(),
                    if independent { "lit" } else { "shadowed" },
                    if rendered_lit { "lit" } else { "shadowed" },
                ));
            }
        }
        let disagreeing = too_dark + too_light;
        eprintln!(
            "face oracle, {}: {compared} pixels compared, {disagreeing} disagree \
             ({too_dark} rendered too dark, {too_light} too light; {shader_alone} the shader alone, \
             {engine_together} both walks together), {unreached} out of every pool, \
             {behind_the_flame} with the flame behind them",
            slab.label(),
        );
        if beyond > 0 {
            eprintln!(
                "  of which {beyond} are drawn beyond this face's own plane — `Prism::mesh`'s seam \
                 overlap, inside the staircase's own body — and {beyond_disagreeing} of those disagree"
            );
        }
        if disagreeing > 0 {
            let axis = match slab.part {
                Part::Riser => "z",
                Part::Top => "the climb",
            };
            let runs: Vec<String> = runs_of(&disagreeing_bands)
                .into_iter()
                .map(|(start, end, points)| format!("bands {start}..{end} ({points} pixels)"))
                .collect();
            eprintln!("  where, up {axis}: {}", runs.join(", "));
        }
        for example in &examples {
            eprintln!("{example}");
        }
        total_compared += compared;
        total_too_dark += too_dark;
        total_too_light += too_light;
        total_unreached += unreached;
    }
    eprintln!(
        "face oracle vs rendered View::Shadow: {}/{total_compared} drawn face pixels disagree \
         ({total_too_dark} rendered too dark, {total_too_light} too light; {total_unreached} more out \
         of every pool, not compared)",
        total_too_dark + total_too_light,
    );
    assert!(
        total_compared > 100,
        "the face oracle compared only {total_compared} pixels of the flight's own faces — a detector \
         that compares nothing reads exactly like a detector that found nothing"
    );
}
