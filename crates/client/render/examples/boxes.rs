//! A general lit-pipeline scene of hand-built boxes: no client files, no
//! map, no tile-shaped footprints required — a generalisation of
//! `examples/two_cubes.rs`'s own "through the real lit pipeline" section
//! (see that module's doc for why it is `GroundRenderer`/`MeshFaceRenderer`/
//! `Blit` and not `SolidsRenderer`) to any number of boxes, each an
//! arbitrary axis-aligned span rather than always a whole tile.
//!
//! `two_cubes.rs`'s own two boxes both come from `occlusion::Builder::add`,
//! which only ever produces a whole-tile body or an edge panel — a real
//! static's footprint always is one of those two shapes, because that is
//! what `tiledata` states. A scene with no static at all is not bound by
//! that: this tool needs boxes narrower than a tile, and boxes stacked on
//! top of each other rather than standing side by side, and neither shape
//! exists in `tiledata`. `occlusion::Builder::add_raw` is the seam that
//! makes that honest instead of worked around — it stores exactly the AABB
//! given, in the same tile bucket every other occluder uses, so the walk
//! finds it exactly the way it finds a wall. Each [`BoxSpec`] here states
//! both its footprint and the one tile bucket that owns it, and both the
//! occluder and the visible mesh (`box_mesh`, copied from `two_cubes.rs`'s
//! own — see that module's doc for why it is built from exact corners and
//! not two `facing::Prism`s) are built from that same [`BoxSpec`], so the
//! two can never disagree the way `two_cubes.rs`'s session 13 bug did.
//!
//! - `OPENSHARD_BOXES_SCENE=tree|pair|line|stair|flat` — which scene to build.
//!   `flat` is the odd one and has no boxes at all: flat ground and one flame,
//!   which is the only scene where the two *shaded* pictures can be laid beside
//!   each other — see "The two shaded pictures" below, and [`scene_flat`].
//!   Default
//!   `tree`: two boxes on one tile, the lower a half-tile footprint, the
//!   upper a third-tile footprint standing directly on top of it — small on
//!   purpose, to see whether a shape that narrow still throws a shadow with
//!   the outline its own silhouette suggests. `pair` puts two boxes of one
//!   height *side by side* on one tile with the flame beyond the second, which
//!   is `docs/lighting_height.md` phase 3's own fixture: see [`scene_pair`].
//!   `line` is `two_cubes.rs`'s own default shape (two whole-tile boxes,
//!   offset `1,0` — due east, a straight line rather than a diagonal) at a
//!   shorter height, built here only so both scenes go through one tool with
//!   one set of knobs. `stair` is `synthetic_stair.rs`'s own default flight
//!   restated as boxes, which is what lets the reference tracer see it at
//!   all: see [`scene_stair`].
//! - `OPENSHARD_SCENE_ZOOM=n` — notches of `Zoom::scale_up`, from `Zoom::ONE`.
//!   Default `3` for both scenes, which is **the top of the ladder**:
//!   `camera::LADDER` has three rungs above 1:1 and `scale_up` stops at the
//!   last, so 3 and 7 and 70 are all 4:1 and all the same picture. `tree`'s
//!   own default said `7` for a while and meant nothing by it — a knob whose
//!   value cannot be read back out of the frame is a knob that has to say so.
//! - `OPENSHARD_LIGHT_AT=dx,dy` / `OPENSHARD_LIGHT_Z` / `OPENSHARD_LIGHT_RADIUS`
//!   — same meaning as `two_cubes.rs`'s own, offset from the scene's first
//!   box's own tile. Both scenes' defaults put the flame up and to the
//!   boxes' `+x` side, close enough that the boxes and their own shadow sit
//!   inside the torch's own reach rather than the whole canvas — `NIGHT`
//!   ambient means most of a 512×512 frame around a single torch is dark
//!   regardless, and that is a fact about a torch at night, not a bug. Picked
//!   by looking at the rendered picture and not by arithmetic
//!   (`two_cubes.rs`'s own session 13 lesson), so treat either default as a
//!   starting point to override rather than a fact about which way is
//!   "screen right".
//! - `OPENSHARD_FLAME_RADIUS=t` — how big the flame's own **body** is, in tiles,
//!   which is what decides how soft a shadow's edge is. Not `_LIGHT_RADIUS`,
//!   which is how far the pool *reaches*; the two were one word for a while and
//!   the confusion is worth spelling out at the knob. Default is
//!   `light::FLAME_RADIUS`, an eighth of a tile, off the art. **Zero is a point
//!   source and a razor edge** — the same knob `tests/traced.rs`'s gates use to
//!   ask the walk a question with no estimate in it — and the reference emitter
//!   follows it, so a run at zero compares two hard shadows rather than a hard
//!   one against a soft one. See [`flame_radius`].
//! - `OPENSHARD_TREE_H1`/`_H2`/`_W1`/`_W2` — the `tree` scene's own two
//!   heights and two footprint widths (tile fractions), default `3`/`3`/
//!   `0.5`/`0.33333`. For pushing the shapes further apart than the default
//!   — a wider gap between the two boxes' own silhouettes is easier to read
//!   a shadow's own edge against. **These four defaults, this scene, and this
//!   tool's own zoom and flame defaults are the reference scene**
//!   (`docs/lighting.md`, "Testing and instrumentation"), and the oracle
//!   counts recorded there are what they produce: override one for a single
//!   run to ask a question, but editing a default here silently retires every
//!   recorded number.
//! - `OPENSHARD_FRAME_DUMP=/tmp/x` — base path; writes `<path>_lit.png` and
//!   `<path>_shadow.png` beside it, both marked with a lime crosshair at the
//!   flame's own projected position (`synthetic_stair.rs`'s own trick).
//! - `OPENSHARD_BOXES_PROBE=x,y[,radius]` — two scanlines through one pixel:
//!   which surface each renderer says is there and **where in the world** it is.
//!   The question a dumped picture cannot answer, since a body's lid and its
//!   riser are one colour. `tools/mask_probe.py` is the rest of that reading.
//!
//! # The oracle
//!
//! A rendered picture answers "does this look right" and nothing sharper —
//! session 14's own account (`docs/lighting_raymarch.md`) of chasing three
//! reported artefacts by eye through several renders is the argument against
//! trusting that alone twice. `oracle_visible`/[`segment_clear_of_box`] is a
//! second, deliberately independent answer: a bare point-light-vs-AABB
//! visibility test, the textbook slab method, written fresh rather than
//! calling the engine's own private `light::ray_vs_solid` — reusing the
//! thing under test to check the thing under test proves nothing about
//! either. Runs by default (`OPENSHARD_BOXES_ORACLE=0` to skip it), over a
//! grid of each box's own top, next to what the engine's own CPU walk
//! (`light::sample`, the same arithmetic `blit.wgsl` runs, held to it by the
//! parity suite `docs/lighting_raymarch.md` calls decision 9) says for the
//! same points, written as one side-by-side comparison picture per box
//! (`<path>_oracle_box<N>.png`: oracle | engine | signed diff) plus a
//! disagreement count on stderr. `OPENSHARD_BOXES_ORACLE_EXACT=1` swaps the
//! engine side to `light::sample_exact` (`walk_the_record`, the ray-vs-Solid
//! primitive session 8-11 built) instead of `light::sample`'s own
//! `walk_cells` — this is how session 14 found that `walk_cells`'s `Edges::ANY`
//! body arm (`light.rs:2269`) tests a candidate tile's `z`-span alone and
//! never its `x`/`y` footprint, so a body narrower than its own tile (every
//! box `occlusion::Builder::add_raw` can build, none `Builder::add` ever
//! could) shadows as if it filled the whole tile: `tree`'s default scene
//! disagreed with the oracle on 3027 of 9216 sampled points of the lower
//! box's own top through `walk_cells`, 480 through `walk_the_record` (the
//! remainder is the soft edge of a real penumbra against the oracle's own
//! hard step, not a further bug). **`walk_the_record` is not wired into
//! `blit.wgsl`, and wiring it would not be enough on its own even if it
//! were**: `Occlusion::solid_bytes` (`occlusion.rs:1259`), what the GPU
//! actually reads a solid's shape from, uploads four bytes a solid —
//! `(z_bottom, z_top, opacity, edges)` — no `x`/`y` at all, because every
//! real static's footprint is already implied by which tile bucket it is in
//! plus its own edges. See the plan doc's own backlog entry for where this
//! goes next.
//!
//! **That last paragraph is the state of session 14 and no longer the tree's.**
//! `Occlusion::solid_bytes` is gone: a primitive is one struct in a storage
//! buffer with its own six absolute coordinates
//! (`Occlusion::primitive_bytes`), so the shader reads a body's `x`/`y` outright
//! and `walk_cells`'s own arm reads the same box. `docs/occluders.md`'s S1. The
//! account above is kept because it is how the gap was *found*, which is the one
//! thing a later reader cannot re-derive.
//!
//! A second oracle, next to it, sweeps the *ground* immediately beside the
//! boxes (`OPENSHARD_BOXES_GROUND_ORACLE=0` to skip it) — the same
//! independent slab test, this time compared against the rendered
//! `View::Shadow` frame read back pixel for pixel, because the ground has no
//! "own top" to ask `light::sample` about directly.
//!
//! A third, `docs/lighting_height.md`'s own phase 0
//! (`OPENSHARD_BOXES_FACE_ORACLE=0` to skip it), closes the gap the first two
//! cannot see at all: both sample a *flat* surface, where an integer height is
//! exact by construction (a lid is at an integer `z`, the ground is at
//! `z = 0`). The defect that doc traces lives on a *vertical* face, where
//! height varies continuously down the wall and `pack_place` rounded it to the
//! nearest unit (phase 1 has since given it a fraction). It sweeps **every
//! pixel the rendered `place` attachment says a box's own `east` or `south`
//! face drew** (`box_mesh` never builds the other two, since an isometric
//! camera never sees them), reads that fragment's own world position back out
//! of the same attachment, and lays the independent slab test's answer about
//! *that* point against the rendered `View::Shadow` pixel.
//!
//! Both halves of that are the renderer's own answer rather than a
//! reconstruction of it, and both replaced a shape that guessed:
//!
//! - **Whose pixel it is.** A world point projected to a pixel lands on
//!   whatever the depth test left there — the ground half a pixel under a
//!   face's base, a nearer box, a box's own top. The attachment names the
//!   instance row that drew each pixel; the old shape re-derived every face's
//!   screen quad instead and was blind to the ground pass entirely, which was
//!   212 of the 278 disagreements the `tree` scene used to report.
//! - **Which point it is.** A pixel's fragment sits at the pixel's centre and
//!   the attachment quantises what it carries (a hundred-and-twenty-eighth of
//!   a tile, a sixteenth of a `z` unit), so a sample point that skipped both
//!   is a fragment the rasteriser could not produce.
//!
//! Every reported line carries the pixels drawn and the disagreements, and the
//! total is asserted non-trivial — a detector that compares nothing reads like
//! one that found nothing. Each face also reports **where** up its own height
//! the disagreements sat, as runs of bands, and **which walk is out**:
//! `light::sample` siding with the independent oracle means the shader alone,
//! siding with the rendered pixel means the engine's own arithmetic in both
//! implementations at once. Those are opposite next steps.
//!
//! # The reference tracer
//!
//! A fourth check, and the only one that is not a point query: the whole scene
//! rendered again by `openshard-client-pathtrace`, a Monte Carlo path tracer
//! with no dependency on this crate and **no notion of a tile anywhere in it**.
//! The three oracles above are independent *arithmetic* answering one question
//! at a time about a point somebody chose; this is an independent *renderer*,
//! and a defect that can only be stated in the shadow walk's own vocabulary —
//! cells, boundaries, stances, exemptions — cannot be reproduced in it by
//! construction rather than by coverage. It is also a third party: where
//! `light::sample` and `blit.wesl` disagree, both are copies of one formula and
//! neither can arbitrate.
//!
//! The same comparison is a test — `tests/traced.rs`, which builds the `line`
//! scene offscreen and asserts on it under `cargo test`. The *judging* is one
//! implementation, `oracle::pathtrace`, shared by both; what is here is what a
//! tool adds to it, which is the pictures and the knobs.
//!
//! It runs by default (`OPENSHARD_BOXES_PATHTRACE=0` to skip) in the
//! *degenerate* mode — a point emitter, one path a pixel, no bounces — where
//! the estimator collapses to one deterministic visibility test and the two
//! pictures must agree. `<path>_pathtrace.png` is the frame's own shadow
//! decision beside the tracer's, grey where a pixel was not compared, and then
//! a third strip of where the two differ — red where the engine lit a pixel the
//! tracer shadowed, blue the other way round, black everywhere the comparison
//! judged nothing. Two shadow masks side by side hide a few hundred
//! disagreeing pixels perfectly well; the third strip is where to look.
//!
//! **A dotted line along a shadow's own edge is expected and is not a finding.**
//! One picture puts an edge on a rasteriser's fill rule and the other on an
//! analytic intersection, so they disagree about the pixel the edge lands in and
//! about nothing else — the report counts those separately as "on an edge". What
//! the strip is for is a *filled* patch: that is a shadow in one renderer and not
//! in the other, which is the whole errand. A pixel
//! is compared only where both agree which surface is there, the surface faces
//! the flame, and neither picture has a shadow edge in its own eight-
//! neighbourhood — the three splits, and why each is not a shadow, are in
//! `docs/lighting_reference.md`.
//!
//! `OPENSHARD_BOXES_PATHTRACE_SAMPLES=n` (with `_BOUNCES`, `_EMITTER`,
//! `_EXPOSURE`) additionally renders the *full* mode to
//! `<path>_pathtrace_full.png`: a spherical emitter, a cosine term, indirect
//! light, ambient occlusion. That one is compared against nothing on purpose —
//! none of what it adds exists in the renderer, so every pixel would
//! "disagree". It is there to be looked at.
//!
//! # The two shaded pictures
//!
//! `<path>_lit_vs_traced.png` is the *brightness* comparison, which is a
//! different thing from the shadow masks above: the engine's own `View::Lit`
//! frame, the tracer's exact render of the same scene in the same light model,
//! and the difference between them per channel, amplified `8×`. Both encoded by
//! `tonemap::encode`, so what is left to differ is the light and not the way it
//! was written to a byte. `docs/lighting_rebuild.md`'s phase 0.
//!
//! **`OPENSHARD_BOXES_SCENE=flat` is the scene it means something on.** No
//! boxes: one flame, flat ground, nothing in the way, and no ambient — so no
//! silhouette, no invented body albedo, no shadow ray, and no term one side has
//! and the other does not. Everything else it could differ by is gone, which
//! leaves falloff, intensity and colour handling. `tests/traced.rs` runs that
//! same scene as a gate.
//!
//! ```sh
//! OPENSHARD_BOXES_SCENE=flat OPENSHARD_SCENE_ZOOM=0 OPENSHARD_FRAME_DUMP=/tmp/flat \
//!     cargo run -p openshard-client-render --example boxes
//! ```
//!
//! ```sh
//! OPENSHARD_FRAME_DUMP=/tmp/tree OPENSHARD_BOXES_SCENE=tree \
//!     cargo run --release -p openshard-client-render --example boxes
//! ```

// The judging module is shared with `tests/traced.rs`, and each side reaches for
// a different part of it: `pathtrace::shading` is a gate's comparison and this
// tool has no dump for it yet. Not every rule needs both readers on the day it is
// written — what the sharing is for is that there is only ever one of it.
#[allow(dead_code)]
mod oracle;

use std::path::PathBuf;

// The reference tracer, under short names because this file's own `light`,
// `Light` and `camera` are already the renderer's. Aliased at the import and
// never re-exported: `pt_light::Light` says which crate's light it is at every
// use, which in a file whose whole subject is comparing two of them is the
// distinction that matters most.
use openshard_client_pathtrace::light as pt_light;
use openshard_client_pathtrace::trace as pt_trace;
use openshard_client_render::atlas::{
    LandAtlas,
    TexmapAtlas,
};
use openshard_client_render::blit::Frame as BlitFrame;
use openshard_client_render::camera::{
    Camera,
    RealPixel,
    WorldSpot,
    Zoom,
    project_exact,
};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::debug::View;
use openshard_client_render::depth;
use openshard_client_render::facing::Face as WallFace;
use openshard_client_render::geometry::Vec2;
use openshard_client_render::light::{
    self,
    Light,
    Lighting,
    NIGHT,
};
use openshard_client_render::mesh_face::{
    MeshFaceRow,
    MeshFaceVertex,
};
use openshard_client_render::occlusion::{
    Builder,
    Part,
    SolidId,
};
use openshard_client_render::place::Stance;
use openshard_client_render::renderer::{
    self,
    GroundRenderer,
    MeshFaceRenderer,
    Target,
};
use openshard_map::grid::BlockExtent;
use openshard_tiles::{
    StaticTile,
    TileFlags,
};
use oracle::boxes::{
    BoxSpec,
    box_mesh,
    box_owner,
};
use oracle::{
    Shade,
    dump,
    read_gbuffer,
    segment_clear_of_box,
};

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn env_or(name: &str, default: &str) -> String {
    env_opt(name).unwrap_or_else(|| default.to_string())
}

fn parse_pair_f32(spec: &str) -> (f32, f32) {
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

/// A "christmas tree": one tile, two boxes stacked on it, each narrower than
/// the one under it. The lower is a half-tile footprint, the upper a
/// third-tile footprint standing exactly on the lower's own top — the two
/// shapes `OPENSHARD_BOXES_SCENE`'s own doc says `occlusion::Builder::add`
/// cannot produce, which is the whole reason this tool exists.
fn scene_tree() -> Vec<BoxSpec> {
    let (tx, ty) = (100u16, 100u16);
    let (cx, cy) = (f64::from(tx) + 0.5, f64::from(ty) + 0.5);
    let h1: f64 = env_or("OPENSHARD_TREE_H1", "3").parse().expect("a number");
    let h2: f64 = env_or("OPENSHARD_TREE_H2", "3").parse().expect("a number");
    let w1: f64 = env_or("OPENSHARD_TREE_W1", "0.5").parse().expect("a number");
    let w2: f64 = env_or("OPENSHARD_TREE_W2", "0.33333").parse().expect("a number");
    vec![
        BoxSpec {
            tile:    (tx, ty),
            min:     (cx - w1 / 2.0, cy - w1 / 2.0, 0.0),
            max:     (cx + w1 / 2.0, cy + w1 / 2.0, h1),
            graphic: 0,
        },
        BoxSpec {
            tile:    (tx, ty),
            min:     (cx - w2 / 2.0, cy - w2 / 2.0, h1),
            max:     (cx + w2 / 2.0, cy + w2 / 2.0, h1 + h2),
            graphic: 1,
        },
    ]
}

/// Two boxes **side by side on one tile, spanning the same heights**, with the
/// flame beyond the second along the line through both.
///
/// The scene `docs/lighting_height.md`'s phase 3 is measured against, and it
/// exists because the `tree` scene cannot show that phase's defect at all:
/// `tree` stacks its boxes, so the two `z` spans meet at a single plane and a
/// fragment of one is inside the other's span for exactly one quantum of
/// height. Here every fragment of either box's faces is inside *both* spans,
/// which is what `exemption` reads as "this solid is the one the fragment is a
/// point of" — so the second box, standing squarely between the first and the
/// flame, is exempted from shadowing the very face it covers. `on_surface`
/// answers a question about height and is asked a question about identity;
/// this is the scene where the two answers differ for every pixel rather than
/// for a band.
///
/// The two are set on the tile's own diagonal so that neither covers the other
/// on screen — they share a tile and therefore a `depth::Order`, and a tie
/// there goes to whichever was pushed later, which would leave the first box's
/// own east face with almost no pixels for an oracle to sweep.
fn scene_pair() -> Vec<BoxSpec> {
    let (tx, ty) = (100u16, 100u16);
    let w: f64 = env_or("OPENSHARD_PAIR_W", "0.3").parse().expect("a number");
    let h: f64 = env_or("OPENSHARD_PAIR_H", "3").parse().expect("a number");
    let (x0, y0) = (f64::from(tx), f64::from(ty));
    vec![
        // The far one, north-west along the tile's diagonal: the face under
        // test is its own `east`.
        BoxSpec {
            tile:    (tx, ty),
            min:     (x0 + 0.05, y0 + 0.65, 0.0),
            max:     (x0 + 0.05 + w, y0 + 0.65 + w, h),
            graphic: 0,
        },
        // And the near one, south-east, standing between it and the flame.
        BoxSpec {
            tile:    (tx, ty),
            min:     (x0 + 0.65, y0 + 0.05, 0.0),
            max:     (x0 + 0.65 + w, y0 + 0.05 + w, h),
            graphic: 1,
        },
    ]
}

/// Two whole-tile boxes in a straight line due east — `two_cubes.rs`'s own
/// default shape (`OPENSHARD_CUBE_OFFSET`'s default `1,1`) offset `1,0`
/// instead, at a shorter height than that tool's own default `11`.
fn scene_line() -> Vec<BoxSpec> {
    let (ax, ay) = (100u16, 100u16);
    let (bx, by) = (101u16, 100u16);
    let h = 4.0;
    vec![
        BoxSpec {
            tile:    (ax, ay),
            min:     (f64::from(ax), f64::from(ay), 0.0),
            max:     (f64::from(ax) + 1.0, f64::from(ay) + 1.0, h),
            graphic: 0,
        },
        BoxSpec {
            tile:    (bx, by),
            min:     (f64::from(bx), f64::from(by), 0.0),
            max:     (f64::from(bx) + 1.0, f64::from(by) + 1.0, h),
            // Two graphics and not one: these are two statics standing in a
            // line, and `occlusion::merge` would fold one graphic's run into a
            // single primitive — which is a scene of its own and not this one.
            graphic: 1,
        },
    ]
}

/// `synthetic_stair.rs`'s own default flight, restated as boxes: one tile, a
/// three-tread climb towards `north`, each tread a full-width strip a third of
/// a tile deep standing on the static's own base at `z 0`.
///
/// The heights and the strip layout are not invented here — they are
/// `facing::Prism::new(Face::North, &[1, 3, 5])`'s own, read off
/// `Prism::footprint`: `up` names the *high* side, so the run climbs from the
/// `+y` edge towards `-y`, and tread `i` of `n` occupies the strip
/// `[i/n, (i+1)/n]` of it. A box per tread and not a prism, because that is the
/// one shape the reference tracer can see: `openshard-client-pathtrace` has an
/// axis-aligned box and nothing else, and a stepped prism *is* a stack of them.
///
/// What it buys is the phase 4 question — a tread standing in its own riser's
/// shadow, a hairline along every tread/riser join — asked of the one renderer
/// that has no tiles, no owners and no self-occlusion exemption to get it wrong
/// with. `synthetic_stair.rs` asks it of a geometric oracle that shares this
/// tool's own camera; this asks it of a path tracer that shares no arithmetic
/// at all.
///
/// The three boxes meet exactly at their shared strip boundaries, which is
/// deliberate: it is where a riser and the tread below it join, and a gap or an
/// overlap there would be a light leak this scene exists to look for.
///
/// **They are returned top tread first, so the near one is painted last.** The
/// boxes share a tile and therefore a `depth::Order`, and a tie there goes to
/// whichever was pushed later, so the *last* box painted is the one that wins.
/// Every tread's own `+y` face is a full-height quad ([`box_mesh`] gives a
/// `Solid` three faces and knows nothing about what abuts it), and the part of
/// it below the tread in front is *interior to the union* — real geometry that
/// no camera can see. Painting the near treads last is what buries it, exactly
/// as an isometric painter's order should. In climb order instead, a tread's
/// buried riser is drawn over the tread in front of it, and the reference tracer
/// reports it: 3,784 pixels of "the frame draws box 2's south face, the tracer
/// sees body 1", none of them on a silhouette. The tracer was right and the
/// order was what was wrong — which is the first thing this scene found.
/// **`OPENSHARD_STAIR_RUN=n` stands `n` flights side by side**, across the climb
/// rather than along it — `examples/synthetic_stair`'s own knob, spelled the same
/// way so that a person pointing the two tools at "the same scene" gets the same
/// scene. One flight cannot pose the question `docs/occluders.md` is about: a run
/// is what puts two treads of *different statics* at one height either side of a
/// tile boundary, sharing a whole face.
///
/// The flights are laid out flight by flight, and within a flight top tread
/// first, so the paint order above holds inside each of them — a flight's own
/// buried risers are what that order buries, and a neighbouring flight is beside
/// it rather than in front of it.
fn scene_stair() -> Vec<BoxSpec> {
    let (tx, ty) = (100u16, 100u16);
    let treads: Vec<f64> = env_or("OPENSHARD_STAIR_TREADS", "1,3,5")
        .split(',')
        .map(|h| h.trim().parse().expect("a number"))
        .collect();
    assert!(!treads.is_empty(), "a flight with no treads is not a scene");
    let run: u16 = env_or("OPENSHARD_STAIR_RUN", "1").parse().expect("a count");
    assert!(run >= 1, "a run of no flights is not a scene");
    let n = treads.len() as f64;
    // How many graphics a flight uses up, so the next flight's treads carry
    // numbers of their own — see the `graphic` below.
    let per_flight = treads.len() as u16;
    let y0 = f64::from(ty);
    (0..run)
        .flat_map(|flight| {
            let x0 = f64::from(tx) + f64::from(flight);
            treads.iter().enumerate().rev().map(move |(i, &h)| {
                // `Prism::footprint`'s own `Face::North` branch, for the run
                // `[i/n, (i+1)/n]`: the low tread sits at the `+y` edge, which is
                // also the near one, which is why the climb order is the paint
                // order here.
                let (lo, hi) = (i as f64 / n, (i as f64 + 1.0) / n);
                BoxSpec {
                    tile:    (tx + flight, ty),
                    min:     (x0, y0 + 1.0 - hi, 0.0),
                    max:     (x0 + 1.0, y0 + 1.0 - lo, h),
                    // One graphic per tread of per flight: every tread of this
                    // scene is its own static, so a landing continuous across
                    // the run stays three primitives — which is the geometry
                    // `docs/occluders.md`'s D2 is argued on.
                    graphic: flight * per_flight + i as u16,
                }
            })
        })
        .collect()
}

/// The tile every scene here is built around, and what stands in for a box's
/// own tile in the one scene that has no boxes.
///
/// Each `scene_*` above picks its own `(100, 100)` by hand; this is that number
/// named, so that the flame — which is authored as an offset from *the scene's
/// tile* — has something to be an offset from when there is nothing standing on
/// it.
const ANCHOR: (u16, u16) = (100, 100);

/// Nothing at all: flat ground, and the flame above it.
///
/// **`docs/lighting_rebuild.md`'s phase 0 scene, and the emptiness is the
/// point.** The comparison that phase asks for is the engine's shaded frame
/// beside the path tracer's, and every other scene here has four things in it
/// that differ for reasons which are not about light: a box's albedo is invented
/// on the reference's side and absent on the engine's (`mesh_face.wesl` writes
/// no colour at all — phase 6), and a box's silhouette is a rasteriser's on one
/// side and an analytic intersection's on the other.
///
/// Take the boxes away and what is left is one surface whose colour both sides
/// read from the same art, lit by one flame, with nothing in the way. What can
/// still differ is falloff, intensity and colour handling — which is exactly
/// what phase 0's own "done when" is about, and nothing else.
fn scene_flat() -> Vec<BoxSpec> {
    Vec::new()
}

/// How big the flame is for this run, in tiles — `light::Lighting::flame_radius`,
/// and `OPENSHARD_FLAME_RADIUS` is the knob.
///
/// **Read here and nowhere else, because four things have to agree about it.**
/// The frame's own `Lighting`, [`oracle_visible`]'s sample points, and the
/// reference tracer's emitter are three renderings of one body, and a run where
/// they disagree reports the difference as the walk's — which is exactly what
/// happened the hour the radius became a field and one call site had not
/// followed (`tests/traced.rs`'s own note: forty-seven pixels).
///
/// `0` is a point source and a hard shadow: no penumbra on any side, and the
/// reference goes exact rather than sampled. The default is
/// [`light::FLAME_RADIUS`], which is what a frame draws.
fn flame_radius() -> f32 {
    env_opt("OPENSHARD_FLAME_RADIUS").map_or(openshard_client_render::light::FLAME_RADIUS, |value| {
        value
            .parse()
            .expect("OPENSHARD_FLAME_RADIUS is a number of tiles")
    })
}

/// How much of `light` `point` can see, geometrically — every box but `skip` (the
/// one `point` itself rests on, which must not shadow itself) tested by
/// [`segment_clear_of_box`], once per point of the flame.
///
/// **A share and not a bool, and `docs/lighting_rebuild.md`'s phase 4 predicted
/// exactly this.** It answered about the flame's *centre* until phase 5, and its
/// disagreements with the frame were reported that phase as "the engine's area
/// light against a point source, and phase 5 is where those become comparable".
/// This is where. `light::flame_points` names where the engine's rays end, so the
/// two are asked about the same body; the segment test is still this file's own
/// and shares no arithmetic with any walk.
///
/// The radius is [`flame_radius`] and not the constant, for the reason that
/// function exists: a run that asked the frame for a point source and this oracle
/// for a sphere would report the whole penumbra as a disagreement.
fn oracle_visible(point: (f64, f64, f64), light: (f64, f64, f64), boxes: &[BoxSpec], skip: usize) -> f64 {
    let spot = light::Spot::at(
        Vec2::new(point.0 as f32, point.1 as f32),
        point.2 as f32,
        (point.0.floor() as i32, point.1.floor() as i32),
    );
    let points = light::flame_points(
        spot,
        [light.0 as f32, light.1 as f32, light.2 as f32],
        flame_radius(),
        light::ShadowRays::DEFAULT,
    );
    let clear = points
        .iter()
        .filter(|at| {
            let to = (f64::from(at[0]), f64::from(at[1]), f64::from(at[2]));
            boxes
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != skip)
                .all(|(_, b)| segment_clear_of_box(point, to, b.min, b.max))
        })
        .count();
    clear as f64 / points.count() as f64
}

/// A flat grayscale raster, `sampler` called once per pixel with the world
/// `(x, y)` a top-down orthographic view over `min..max` puts there — no
/// relation to the scene's own isometric camera, on purpose: reading this
/// image takes no more of the renderer's own arithmetic on trust than
/// [`oracle_visible`] already does.
fn raster_top_down(
    side: u32,
    min: (f64, f64),
    max: (f64, f64),
    sampler: impl Fn(f64, f64) -> f32,
) -> Vec<u8> {
    let mut pixels = vec![0u8; (side * side * 3) as usize];
    for row in 0..side {
        for col in 0..side {
            let u = (col as f64 + 0.5) / f64::from(side);
            let v = (row as f64 + 0.5) / f64::from(side);
            let x = min.0 + u * (max.0 - min.0);
            let y = min.1 + v * (max.1 - min.1);
            let value = (sampler(x, y).clamp(0.0, 1.0) * 255.0).round() as u8;
            let at = ((row * side + col) * 3) as usize;
            pixels[at] = value;
            pixels[at + 1] = value;
            pixels[at + 2] = value;
        }
    }
    pixels
}

/// Three side-by-side [`raster_top_down`] strips — oracle, engine, and their
/// signed difference (red where the engine is darker than the oracle says it
/// should be, cyan where it is lighter) — as one picture, so the two sit at the
/// same scale without a second tool to align them.
fn write_oracle_comparison(path: &std::path::Path, side: u32, oracle: &[u8], engine: &[u8]) {
    let mut diff = vec![0u8; (side * side * 3) as usize];
    for i in 0..(side * side) as usize {
        let (o, e) = (i32::from(oracle[i * 3]), i32::from(engine[i * 3]));
        let d = e - o;
        if d < 0 {
            diff[i * 3] = d.unsigned_abs().min(255) as u8;
        } else {
            diff[i * 3 + 1] = d.min(255) as u8;
            diff[i * 3 + 2] = d.min(255) as u8;
        }
    }
    openshard_client_render::png::write_strips(path, side, side, &[oracle, engine, &diff])
        .expect("writing the oracle comparison");
    eprintln!("wrote {}", path.display());
}

fn main() {
    let (device, queue) = gpu().expect("an adapter");

    let scene_name = env_or("OPENSHARD_BOXES_SCENE", "tree");
    let boxes = match scene_name.as_str() {
        "tree" => scene_tree(),
        "pair" => scene_pair(),
        "line" => scene_line(),
        "stair" => scene_stair(),
        "flat" => scene_flat(),
        other => {
            panic!("unknown OPENSHARD_BOXES_SCENE {other:?}, wanted tree, pair, line, stair or flat")
        }
    };
    eprintln!("scene {scene_name:?}: {} boxes", boxes.len());
    for (i, b) in boxes.iter().enumerate() {
        eprintln!(
            "box {i}: tile {:?}, x {:.3}..{:.3} y {:.3}..{:.3} z {:.3}..{:.3}",
            b.tile, b.min.0, b.max.0, b.min.1, b.max.1, b.min.2, b.max.2
        );
    }

    // The tile the scene stands on — its own boxes', or [`ANCHOR`] where it has
    // none. Both the grid's bounds and the flame's place are stated relative to
    // it, so a scene with nothing standing in it still has a somewhere.
    let anchor = boxes.first().map_or(ANCHOR, |b| b.tile);
    let min_tx = boxes.iter().map(|b| b.tile.0).min().unwrap_or(anchor.0);
    let max_tx = boxes.iter().map(|b| b.tile.0).max().unwrap_or(anchor.0);
    let min_ty = boxes.iter().map(|b| b.tile.1).min().unwrap_or(anchor.1);
    let max_ty = boxes.iter().map(|b| b.tile.1).max().unwrap_or(anchor.1);
    let bounds = openshard_client_render::camera::TileBounds {
        min_x: i32::from(min_tx) - 5,
        max_x: i32::from(max_tx) + 5,
        min_y: i32::from(min_ty) - 5,
        max_y: i32::from(max_ty) + 5,
    };

    // NO_SHOOT so a box occludes light at all (`occlusion::opacity`'s own
    // doc: a graphic's own flags decide it, not the shape). `height` here is
    // only what `depth::static_priority_z` reads off it (whether the box has
    // any height at all); the occluder's real span comes from `add_raw`'s
    // own `space`, not from this.
    let cube_tile = StaticTile {
        flags: TileFlags::new(TileFlags::NO_SHOOT),
        height: 1,
        ..StaticTile::default()
    };

    let mut builder = Builder::new(bounds);
    for b in boxes.iter() {
        builder.add_raw(b.tile.0, b.tile.1, b.solid(), box_owner(b));
    }
    let occlusion = builder.finish(&Cutaway::OPEN);
    // Which *solid* of the grid each box is — what a fragment of that box has to
    // carry for `exemption` to know it is a point of it, and what `pair` is
    // entirely about: two boxes on one tile, so two names, where the height test
    // they replaced cannot tell them apart at all. `add_raw` pushes exactly one
    // solid a box, so `Part::ONLY` is the piece. `docs/lighting_rebuild.md` phase
    // 4; it was the box's `OwnerId` until then.
    let solids: Vec<SolidId> = boxes
        .iter()
        .enumerate()
        .map(|(index, b)| {
            occlusion
                .id_of(i32::from(b.tile.0), i32::from(b.tile.1), box_owner(b), Part::ONLY)
                .unwrap_or_else(|| {
                    panic!(
                        "box {index} is not in the grid this tool just built — every oracle \
                         below would then be measuring a scene with one box missing"
                    )
                })
        })
        .collect();
    eprintln!("solids: {:?}", solids.iter().map(|s| s.raw()).collect::<Vec<_>>());

    let (width, height_px): (u32, u32) = (512, 512);
    // Three notches is the top of `camera::LADDER` — 4:1, the closest this
    // crate's own wheel goes — and both scenes want it: a whole-tile box fills
    // the fixed 512×512 canvas comfortably there and a half-tile one is still
    // readable. `tree`'s default read `7` until it was noticed that
    // `Zoom::scale_up` stops at the last rung, so the four extra notches were
    // four calls that returned the same value: the frame at 3 and at 7 is the
    // same frame, oracle counts included, which is how it was checked rather
    // than argued.
    let zoom_notches: u32 = env_or("OPENSHARD_SCENE_ZOOM", "3").parse().expect("a number");
    let centre_x = (i32::from(min_tx) + i32::from(max_tx)) / 2;
    let centre_y = (i32::from(min_ty) + i32::from(max_ty)) / 2;
    let mut camera = Camera::new(
        openshard_protocol::world::Point::new(centre_x as u16, centre_y as u16, 0),
        width,
        height_px,
    );
    let mut zoom = Zoom::ONE;
    for _ in 0..zoom_notches {
        zoom = zoom.scale_up();
    }
    camera.zoom_about(RealPixel::new((width / 2) as i32, (height_px / 2) as i32), zoom);

    let projection = camera.projection();
    // Where a world position lands in this frame, in real pixels — stated once.
    //
    // Three places below need it (a face's own screen ring, the flame's
    // crosshair, and the reference tracer's camera) and each used to spell it
    // out again. Three copies of one composition is the shape that drifts, and
    // here it would drift in the worst possible direction: the tracer's whole
    // claim is that it looks at the same scene through the same camera, and a
    // fourth hand-written copy of this is exactly how it would quietly stop.
    let to_pixel = |at: WorldSpot| -> (f64, f64) {
        let screen = camera.to_view_exact(project_exact(at));
        (
            f64::from((screen.x - projection.origin.x) * projection.scale + width as f32 * 0.5),
            f64::from((screen.y - projection.origin.y) * projection.scale + height_px as f32 * 0.5),
        )
    };

    let base_tile = depth::base_for(centre_x, centre_y);
    let mut rows: Vec<MeshFaceRow> = Vec::new();
    let mut vertices: Vec<MeshFaceVertex> = Vec::new();
    // Which row each box's each face was pushed as, kept while it is pushed
    // rather than re-derived from `rows.len()` arithmetic later: it is what the
    // face oracle compares the rendered `place` attachment's own id against, so
    // "this pixel is box 2's south face" is the renderer's answer and not this
    // tool's guess about the order it built its own list in.
    let mut face_rows: Vec<(usize, Stance, u32)> = Vec::new();
    for (box_index, b) in boxes.iter().enumerate() {
        let solid = b.solid();
        let d = depth::Order {
            tile:       i32::from(b.tile.0) + i32::from(b.tile.1),
            priority_z: depth::static_priority_z(solid.min.z.round() as i8, &cube_tile),
        }
        .to_depth(base_tile);
        let mesh = box_mesh(solid);
        for face in mesh.faces() {
            let id = rows.len() as u32;
            let stance = Stance::of_normal(face.normal).expect("a box face's own axis-aligned normal");
            face_rows.push((box_index, stance, id));
            rows.push(MeshFaceRow {
                tile: (b.tile.0, b.tile.1),
                stance,
                // Every face of one box carries that box's own solid — a raw box
                // is one solid however many faces are drawn over it.
                solid: solids[box_index].raw(),
            });
            for corner in face.fan() {
                let screen = camera.to_view_exact(project_exact(corner));
                vertices.push(MeshFaceVertex {
                    screen,
                    world: [corner.x as f32, corner.y as f32, corner.z as f32],
                    depth: d,
                    id,
                    tile: [f32::from(b.tile.0), f32::from(b.tile.1)],
                    normal: face.normal,
                    // What the reference tracer is told every box is worth —
                    // `oracle::pathtrace::Albedos::INVENTED.body` — so the two
                    // pictures start from one authored number rather than two.
                    // `oracle::body_albedo` reads this back off the frame below,
                    // which is what makes "the same albedo on both sides" a
                    // measurement and not a second author agreeing with the first.
                    colour: oracle::pathtrace::Albedos::INVENTED.body.map(|c| c as f32),
                });
            }
        }
    }
    eprintln!("{} mesh faces, {} vertices", rows.len(), vertices.len());
    // Where each face lands in *pixels*, not in the view space `screen` above
    // is in: "what is this patch in the picture" is a question about pixels,
    // and answering it by re-deriving the projection by hand outside the tool
    // is exactly the arithmetic-instead-of-looking `two_cubes.rs`'s own
    // session 13 lesson warns about. Same conversion `light_pixel` below uses.
    {
        let vertex_pixel = |v: &MeshFaceVertex| {
            to_pixel(WorldSpot {
                x: f64::from(v.world[0]),
                y: f64::from(v.world[1]),
                z: f64::from(v.world[2]),
            })
        };
        for (id, row) in rows.iter().enumerate() {
            let (mut minx, mut maxx, mut miny, mut maxy) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
            for corner in vertices.iter().filter(|v| v.id == id as u32) {
                let (px, py) = vertex_pixel(corner);
                minx = minx.min(px);
                maxx = maxx.max(px);
                miny = miny.min(py);
                maxy = maxy.max(py);
            }
            // The corners themselves and not only the box around them: which
            // face owns a given patch of the picture is a question a bounding
            // box cannot answer here, because all six of these overlap on
            // screen — the upper box's own south face sits inside the lower
            // box's own south face's box.
            let ring: Vec<String> = vertices
                .iter()
                .filter(|v| v.id == id as u32)
                .map(|v| {
                    let (px, py) = vertex_pixel(v);
                    format!("({px:.1},{py:.1})")
                })
                .collect();
            eprintln!(
                "face {id}: tile {:?}, stance {:?}, pixels x {minx:.1}..{maxx:.1}, y {miny:.1}..{maxy:.1}, ring {}",
                row.tile,
                row.stance,
                ring.join(" "),
            );
        }
    }

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let base = env_opt("OPENSHARD_FRAME_DUMP").unwrap_or_else(|| "boxes".to_string());

    let world = openshard_client_render::blit::world_texture(&device, width, height_px);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(&device, width, height_px);
    let gbuffer_views = gbuffer.views();
    let depth_tex = renderer::depth_texture(&device, width, height_px);
    let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

    // A floor for a shadow to fall on — `two_cubes.rs`'s own hand-built land
    // tile, repeated over the same bounds the occlusion grid covers.
    const FLOOR_TILE: openshard_tiles::LandTileId = openshard_tiles::LandTileId(3);
    let floor_pixel = openshard_uofiles::color::Color16((20 << 10) | (20 << 5) | 20);
    let floor_image = openshard_uofiles::image::Image::new(
        openshard_uofiles::art::LAND_TILE_SIZE,
        openshard_uofiles::art::LAND_TILE_SIZE,
        vec![floor_pixel; usize::from(openshard_uofiles::art::LAND_TILE_SIZE).pow(2)],
    );
    let blocks = (bounds.max_x as u32).div_ceil(openshard_map::map::BLOCK_SIZE) + 1;
    let synthetic_map = openshard_map::map::WorldMap::from_blocks(
        BlockExtent {
            wide: blocks,
            down: blocks,
        },
        |_x, _y| {
            openshard_map::map::LandCell {
                tile: FLOOR_TILE,
                z:    0,
            }
        },
    );
    let land = LandAtlas::pack([(FLOOR_TILE, floor_image)]).expect("one flat tile always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let ground_quads =
        openshard_client_render::ground::collect(&synthetic_map, &camera, &land, &texmaps, &Cutaway::OPEN);
    eprintln!("{} ground quads", ground_quads.len());

    let mut ground_pass = GroundRenderer::new(&device, &queue, format, &land, &texmaps);
    let mut mesh_pass = MeshFaceRenderer::new(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target = Target {
        gbuffer: &gbuffer_views,
        view: &world_view,
        depth: &depth_view,
        width,
        height: height_px,
        projection: camera.projection(),
    };
    ground_pass.render(&device, &queue, &mut encoder, target, &ground_quads);
    mesh_pass.render(&device, &queue, &mut encoder, target, &vertices, &rows);
    queue.submit([encoder.finish()]);

    // What the world passes actually left on each pixel. Read once, here,
    // because it is the *world* passes' output and the blit below neither
    // writes it nor changes it however many views are dumped — and every oracle
    // in this tool needs it to know whether the pixel it is about to read is a
    // pixel of the surface it is asking about. See [`Drawn`].
    let drawn = read_gbuffer(&device, &queue, &gbuffer, width, height_px);

    // Picked by looking at a rendered frame, the same way the zoom default
    // above was: a radius wide enough to light both boxes and their own
    // shadow without pulling the whole 512×512 canvas into the torch's
    // reach, which is a fact about a torch at night and not a bug (`NIGHT`
    // ambient below is deliberate — see `light.rs`'s own doc).
    let (default_ldx, default_ldy, default_z, default_radius) = match scene_name.as_str() {
        "tree" => (1.5, -1.0, "6", "6"),
        // `pair`'s flame is not picked to look good, it is picked to make one
        // question sharp: it stands on the line through both boxes' own
        // centres, beyond the near one, at half their height. So every ray from
        // the far box's `east` face to it runs nearly level and squarely
        // through the near box, and any pixel of that face the frame draws lit
        // is a pixel the near box was exempted from shadowing.
        "pair" => (2.0, -1.0, "1.5", "6"),
        // `synthetic_stair.rs`'s own default flame, verbatim (`OPENSHARD_LIGHT_AT`
        // `2.5,1.0`, `_Z` 2, `_RADIUS` 6) and offset from the same tile — the
        // flight's first tread. Low and in front of the climb, which is what puts
        // the far tread in shadow and draws a hairline on every tread/riser join.
        // Two tools rendering one scene under two flames would be two scenes.
        "stair" => (2.5, 1.0, "2", "6"),
        // `flat` puts the flame over the middle of its own tile and low, so the
        // pool is a disc on open ground with its brightest point in the frame:
        // the falloff curve is what this scene is a picture of, and a flame off
        // to one side would show most of one flank of it.
        "flat" => (0.5, 0.5, "3", "8"),
        _ => (2.5, -1.5, "6", "8"),
    };
    let (ldx, ldy) = env_opt("OPENSHARD_LIGHT_AT")
        .map(|s| parse_pair_f32(&s))
        .unwrap_or((default_ldx, default_ldy));
    let light_z: f32 = env_or("OPENSHARD_LIGHT_Z", default_z).parse().expect("a number");
    let light_radius: f32 = env_or("OPENSHARD_LIGHT_RADIUS", default_radius)
        .parse()
        .expect("a number");
    eprintln!("light: at ({ldx:+}, {ldy:+}) of the scene's own tile, z {light_z}, radius {light_radius}");
    // What everything is multiplied by away from the flame.
    //
    // `NIGHT` everywhere but on the scene whose whole errand is a brightness
    // comparison. The reference tracer has **no ambient term at all** — a
    // degenerate render is direct light and nothing else — so an ambient here is
    // a constant the engine's picture has and the tracer's does not, added to
    // every pixel of a comparison that is trying to measure a falloff curve. It
    // is not subtractable afterwards either: the sum goes through a tonemap, and
    // a curve is not linear in what was put into it.
    //
    // Giving the tracer an ambient instead would be worse — it would be this
    // renderer's own ambient model, restated inside the thing that checks this
    // renderer, which is the one shape an oracle may not have.
    let ambient = match scene_name.as_str() {
        "flat" => {
            light::Ambient {
                sky:    [0.0; 3],
                ground: [0.0; 3],
            }
        }
        _ => NIGHT,
    };
    let mut lighting = Lighting {
        ambient,
        lights: vec![Light {
            at:        Vec2::new(f32::from(anchor.0) + ldx, f32::from(anchor.1) + ldy),
            z:         light_z,
            radius:    light_radius,
            color:     [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam:      None,
        }],
        occlusion,
        sun: None,
        view: View::Lit,
        flame_radius: flame_radius(),
        shadow_rays: openshard_client_render::light::ShadowRays::DEFAULT,
        dead: false,
    };
    // Where the flame itself is, in the one place every consumer of it reads:
    // the crosshair on the dumped frames, the two visibility oracles, and the
    // reference tracer's own emitter all take their light from here.
    let light_at = WorldSpot {
        x: f64::from(anchor.0) + f64::from(ldx),
        y: f64::from(anchor.1) + f64::from(ldy),
        z: f64::from(light_z),
    };
    // The same place as three plain numbers, which is what the oracles below
    // take. One definition, so a scene knob can never move the flame for the
    // picture and not for what checks it.
    let light_point = (light_at.x, light_at.y, light_at.z);
    let light_pixel = to_pixel(light_at);
    let light_mark = (light_pixel.0.round() as i32, light_pixel.1.round() as i32);
    eprintln!("light pixel: {light_mark:?}");

    // The oracle: for each box's own top, an independent (`oracle_visible`,
    // no shared code with `light::sample`/`walk_cells`/`blit.wgsl`) answer
    // for whether the light can see each point of it at all, laid next to
    // what the engine's own CPU walk (`light::sample`, the same arithmetic
    // `blit.wgsl` runs, held to it by a parity test) says — a disagreement
    // here is a real defect, not a rendering nuance, because nothing about
    // either side depends on how a pixel projects to the screen.
    // A scene with no boxes has no box tops and no box faces, so the two oracles
    // that sweep them are skipped rather than run over nothing — and *said*,
    // because a detector that quietly compared nothing reads exactly like one
    // that found nothing, which is the rule their own non-triviality assertions
    // exist to enforce.
    if boxes.is_empty() {
        eprintln!("scene {scene_name:?} has no boxes: the box-top and face oracles are not run");
    }
    if !boxes.is_empty() && env_opt("OPENSHARD_BOXES_ORACLE").as_deref() != Some("0") {
        for (index, b) in boxes.iter().enumerate() {
            let side = 96u32;
            let z = b.max.2;
            let oracle = raster_top_down(
                side,
                (b.min.0, b.min.1),
                (b.max.0, b.max.1),
                // The share of the flame this point can see, drawn as a grey:
                // the picture beside it is the engine's own `through`, which
                // is the same quantity, so a soft edge now appears on both.
                |x, y| oracle_visible((x, y, z), light_point, &boxes, index) as f32,
            );
            let engine = raster_top_down(side, (b.min.0, b.min.1), (b.max.0, b.max.1), |x, y| {
                // A point of *this box's own top*, which is what the solid id
                // says: the box it is on must not shadow it, and any other box
                // on the same tile must. Before identity that was read off the
                // height, and on `pair` — where both boxes span the same
                // heights — it exempted the wrong one.
                let spot = light::Spot::flat(
                    Vec2::new(x as f32, y as f32),
                    z as f32,
                    (i32::from(b.tile.0), i32::from(b.tile.1)),
                )
                .part_of(solids[index]);
                let sampler = match env_opt("OPENSHARD_BOXES_ORACLE_EXACT").as_deref() {
                    Some("1") => light::sample_exact,
                    _ => light::sample,
                };
                sampler(spot, &lighting)
                    .reaches
                    .first()
                    .map_or(0.0, |reach| if reach.within { reach.through } else { 0.0 })
            });
            let mut mismatches = 0usize;
            for i in 0..(side * side) as usize {
                let (o, e) = (oracle[i * 3], engine[i * 3]);
                let says_lit = o > 200;
                let reads_lit = e > 128;
                if says_lit != reads_lit {
                    mismatches += 1;
                }
            }
            eprintln!(
                "oracle vs engine, box {index}'s own top: {mismatches}/{} pixels disagree on lit-or-not",
                side * side
            );
            write_oracle_comparison(
                std::path::Path::new(&format!("{base}_oracle_box{index}.png")),
                side,
                &oracle,
                &engine,
            );
        }
    }

    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    let mut blit = openshard_client_render::blit::Blit::new(&device, format);
    // Which `debug::View`s to dump, by `View::name()`. `View::Shadow` is
    // appended whether or not it was asked for: the ground oracle below reads
    // the rendered shadow frame back, so dropping it would silently disarm the
    // one check in this tool that does not depend on anyone looking at a
    // picture.
    let mut views: Vec<View> = env_or("OPENSHARD_BOXES_VIEWS", "lit,shadow")
        .split(',')
        .map(|name| {
            let name = name.trim();
            [
                View::Lit,
                View::Place,
                View::Kind,
                View::Height,
                View::Occluders,
                View::Light,
                View::Shadow,
                View::Reach,
                View::Sun,
                View::Sky,
                View::Flames,
            ]
            .into_iter()
            .find(|view| view.name() == name)
            .unwrap_or_else(|| panic!("unknown OPENSHARD_BOXES_VIEWS entry {name:?}"))
        })
        .collect();
    if !views.contains(&View::Shadow) {
        views.push(View::Shadow);
    }

    let mut shadow_pixels: Vec<u8> = Vec::new();
    // And the shaded frame itself, which the brightness comparison is *about*.
    // Kept from the same dump loop as the shadow view and for the same reason:
    // a second render of the same view is a second frame, and the whole claim of
    // the comparison is that it is looking at the picture a person looked at.
    let mut lit_pixels: Vec<u8> = Vec::new();
    // `View::Lit` and `View::Shadow` answer "does it look right" and "what did
    // the walk say"; when a patch in either one is unaccounted for, the
    // question is which surface owns those pixels at all, and that is
    // `View::Kind`/`View::Height`/`View::Place`'s own job (`debug.rs`'s doc on
    // each). Naming them by hand beats dumping all eleven every run.
    for view in views {
        lighting.view = view;
        let surface = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("surface"),
            size: wgpu::Extent3d {
                width,
                height: height_px,
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
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        blit.render(
            &device,
            &queue,
            &mut encoder,
            BlitFrame {
                target:           &surface_view,
                world:            &world_view,
                gbuffer:          &gbuffer_views,
                face_instances:   &dummy_instances,
                item_instances:   &dummy_instances,
                mobile_instances: &dummy_instances,
                mesh_instances:   mesh_pass.rows_buffer(),
                ground_instances: ground_pass.instances_buffer(),
                zoom:             Zoom::ONE,
                rect:             openshard_client_render::blit::ViewportRect {
                    x: 0,
                    y: 0,
                    width,
                    height: height_px,
                },
            },
            &lighting,
        );
        queue.submit([encoder.finish()]);
        let pixels = dump(
            &device,
            &queue,
            &surface,
            width,
            height_px,
            &PathBuf::from(format!("{base}_{}.png", view.name())),
            Some(light_mark),
        );
        // The crosshair `dump` draws is on the *file*, not on what it returns —
        // see its own doc — so what these two keep is the frame as the shader
        // wrote it. A comparison against a picture with a marker painted into it
        // would report the marker.
        if view == View::Shadow {
            shadow_pixels = pixels;
        } else if view == View::Lit {
            lit_pixels = pixels;
        }
    }

    // A ground-plane companion to the box-top oracle above: that one only ever
    // asks about the tops of the boxes, so it structurally cannot see the bug
    // `docs/lighting_raymarch.md`'s backlog names ("A live CPU/GPU disagreement
    // on `boxes.rs`'s `tree` scene") — the ground immediately beside a box's own
    // base.
    //
    // Same shape as the face oracle below, and for the same two reasons: it
    // sweeps **every pixel the rendered `place` attachment says the ground
    // drew**, and asks about **that fragment's own world position**, read back
    // out of the same attachment. Both replaced guesses that cost it most of
    // its own reported disagreements:
    //
    // - Standing outside every box's footprint is not enough to be looking at
    //   the ground. A box's picture *rises* out of its footprint, so the ground
    //   behind one is drawn over by that box's own faces, and reading those
    //   pixels as the ground's was the bulk of its "rendered too dark" points.
    // - The GPU never lights the point a top-down grid names: `ground.wgsl`
    //   quantises a fragment's tile-local fraction to `SUB_TILE = 127` levels,
    //   and the fragment under a pixel sits at the pixel's own centre, which is
    //   up to half a pixel from wherever a sampled world point projected. This
    //   used to repeat the quantisation by hand on its own `(x, y)` — which is
    //   the right idea applied to the wrong point.
    //
    // The independent answer is `segment_clear_of_box`, the same no-shared-
    // arithmetic slab test the box-top check already trusts, and `light::sample`
    // says which walk is out on every disagreement.
    type Mismatch = (f64, f64, f32, (u8, u8, u8));
    if env_opt("OPENSHARD_BOXES_GROUND_ORACLE").as_deref() != Some("0") {
        let mut sampled = 0usize;
        // Ground pixels no flame's radius covers. Reported rather than silently
        // dropped: "this many pixels were not compared, and why" is half of what
        // makes a count of disagreements mean anything.
        let mut unreached = 0usize;
        let mut mismatches = 0usize;
        let mut too_dark = 0usize;
        let mut too_light = 0usize;
        let mut shader_alone = 0usize;
        let mut engine_together = 0usize;
        let mut examples_too_dark: Vec<Mismatch> = Vec::new();
        let mut examples_too_light: Vec<Mismatch> = Vec::new();
        for (pixel, texel) in drawn.iter().enumerate() {
            if texel.kind != openshard_client_render::place::Kind::Land as u32 {
                continue;
            }
            // The fragment's own point, read whole off the position plane — not
            // a tile looked up in a row plus a fraction and a height taken out
            // of two `u16`s and added back to it. A hillside's pixels each
            // carry their own interpolated height, and this is the point the
            // shader lit.
            let (x, y, z) = texel.at;
            let offset = pixel * 4;
            // **A fragment no flame reaches is not a fragment this oracle has an
            // opinion about.** `oracle_visible` is pure geometry and knows
            // nothing of a torch's range, so comparing it against a pixel the
            // shader left dark *because of the radius* counts the radius as a
            // disagreement. `Shade` exists to make that distinction available
            // and `Shade::lit`'s own doc says a caller that must not count it
            // has to match on the variant — this is that caller.
            //
            // It went unnoticed while every scene here had its flame reaching
            // the whole canvas. The `flat` scene is the first where it does not,
            // and this reported 67,728 of 262,144 ground pixels "rendered too
            // dark" — every one of them simply out of the torch's reach.
            let shade = Shade::of([
                shadow_pixels[offset],
                shadow_pixels[offset + 1],
                shadow_pixels[offset + 2],
            ]);
            if shade == Shade::Unreached {
                unreached += 1;
                continue;
            }
            sampled += 1;
            let gpu_lit = shade.lit();
            let independent = oracle_visible((x, y, z), light_point, &boxes, usize::MAX) > 0.5;
            if independent != gpu_lit {
                mismatches += 1;
                // The ground is no occluder, so it owns nothing and is exempt
                // from nothing — `Spot::flat`'s own default.
                // The cell the walk starts from is the tile the ground quad this
                // pixel names stands on — the row, not `floor` of the position,
                // which is the class of bug `walk`'s own comment records.
                let tile = ground_quads[texel.id as usize].place;
                let spot = light::Spot::flat(
                    Vec2::new(x as f32, y as f32),
                    z as f32,
                    (i32::from(tile.x), i32::from(tile.y)),
                );
                let through = light::sample(spot, &lighting)
                    .reaches
                    .first()
                    .map_or(0.0, |reach| if reach.within { reach.through } else { 0.0 });
                match (through > 0.5) == independent {
                    true => shader_alone += 1,
                    false => engine_together += 1,
                }
                let rgb = (
                    shadow_pixels[offset],
                    shadow_pixels[offset + 1],
                    shadow_pixels[offset + 2],
                );
                if gpu_lit {
                    // Oracle says shadowed, picture says lit.
                    too_light += 1;
                    if examples_too_light.len() < 8 {
                        examples_too_light.push((x, y, through, rgb));
                    }
                } else {
                    // Oracle says lit, picture says shadowed.
                    too_dark += 1;
                    if examples_too_dark.len() < 8 {
                        examples_too_dark.push((x, y, through, rgb));
                    }
                }
            }
        }
        eprintln!(
            "ground oracle vs rendered View::Shadow: {mismatches}/{sampled} drawn ground pixels disagree \
             ({too_dark} rendered too dark, {too_light} rendered too light; {shader_alone} the shader \
             alone, {engine_together} both walks together); {unreached} more are outside the flame's \
             own radius and are not compared"
        );
        assert!(
            sampled > 100,
            "the ground oracle found only {sampled} pixels of ground — a detector that compares nothing \
             reads exactly like a detector that found nothing"
        );
        for (label, examples) in [
            ("too dark", &examples_too_dark),
            ("too light", &examples_too_light),
        ] {
            for (x, y, cpu_through, rgb) in examples {
                eprintln!(
                    "  [{label}] ({x:.3}, {y:.3}): light::sample through={cpu_through:.3}, pixel rgb={rgb:?}",
                );
            }
        }
    }

    // The face oracle: `docs/lighting_height.md`'s own phase 0. Neither
    // oracle above can see this bug's class — the box-top oracle samples only
    // a box's own flat top, where an integer height is exact (a lid *is* at
    // an integer `z`); the ground oracle samples the ground, `z = 0`,
    // likewise exact. The defect lives on a vertical face, where height
    // varies continuously and `pack_place` rounded it to the nearest unit.
    //
    // **It sweeps pixels and not world points.** Every pixel of the frame the
    // rendered `place` attachment says a box's own vertical face drew — the
    // renderer's answer, not a reconstruction of it — is one comparison: the
    // fragment's own world position is read back out of that attachment
    // (`Drawn`), the independent `segment_clear_of_box` is asked about *that*
    // point, and the answer is laid against the rendered `View::Shadow` pixel.
    // No arithmetic here is shared with `light.rs` or `blit.wesl`.
    //
    // Both halves of that are corrections of an earlier shape that gridded
    // world points over each face and projected them, and both were worth
    // hundreds of phantom disagreements on the `tree` scene:
    //
    // - **Whose pixel is it.** A projected sample lands on whatever the depth
    //   test left there — the ground half a pixel under a face's base, a
    //   nearer box's face, a box's own top. The old shape answered that by
    //   re-deriving every face's screen quad and running a point-in-quad test
    //   with a hand-rolled painter's-order tie-break, which was blind to the
    //   ground pass entirely (212 of `tree`'s own 278 disagreements were the
    //   ground, correctly shadowed, read as though it were the face's).
    // - **Which point is it.** The pixel's own fragment sits at the pixel's
    //   centre, and the attachment quantises what it carries — a
    //   hundred-and-twenty-seventh of a tile, a sixteenth of a `z` unit. A
    //   sample point that skipped both is a fragment the rasteriser could not
    //   produce, and near a grazing corner the difference decides the answer.
    //   The ground oracle already knew this and quantised by hand; reading the
    //   attachment is the same statement, exactly, and for every axis at once.
    //
    // A face with no pixels at all is not a pass: every line below carries
    // sampled and disagreeing, and the total is asserted non-trivial — a
    // detector that silently compares nothing reads exactly like a detector
    // that found nothing.
    if !boxes.is_empty() && env_opt("OPENSHARD_BOXES_FACE_ORACLE").as_deref() != Some("0") {
        // How many bands to report a face's disagreements in, up its own
        // height. Not a sampling grid any more — the sweep is exhaustive over
        // the face's pixels — only the resolution the "where" line reads at.
        let bands = 64usize;
        let mut total_sampled = 0usize;
        let mut total_disagreeing = 0usize;
        for (index, b) in boxes.iter().enumerate() {
            for (face, label) in [(WallFace::East, "east"), (WallFace::South, "south")] {
                // Which row this face was drawn as. The attachment names a row,
                // so this is what "the pixel is this face's" compares against —
                // and a face that was never pushed would be a scene this tool
                // cannot ask about at all, which is a panic and not a skip.
                let own_row = face_rows
                    .iter()
                    .find(|(box_index, stance, _)| *box_index == index && *stance == Stance::face(face))
                    .map(|(_, _, id)| *id)
                    .expect("every box pushes an east and a south face");
                let mut sampled = 0usize;
                // Pixels of this face outside every flame's radius, which this
                // oracle has no opinion about — see the ground oracle above.
                let mut unreached = 0usize;
                let mut disagreeing = 0usize;
                // And which of the two walks is out, on every disagreement.
                // `light::sample` is the CPU's own preview of exactly what the
                // shader does (`docs/lighting.md` decision 9 holds the two to
                // each other), so a disagreement where it sides with the
                // independent oracle is the *shader* alone being out — a parity
                // gap — and one where it sides with the rendered pixel is the
                // engine's own arithmetic being out, in both implementations at
                // once. Those are opposite next steps, and a count that does not
                // tell them apart names neither.
                let mut shader_alone = 0usize;
                let mut engine_together = 0usize;
                let mut examples: Vec<String> = Vec::new();
                // Where up the face each disagreement sat, one counter a band.
                // A total alone says a phase moved the number; it cannot say
                // whether what is left is the same defect made smaller or a
                // different one that was always there, and those want opposite
                // next steps. `docs/lighting_height.md` phase 1's own residual
                // is the case in point: it is not spread over the face at all,
                // it is one band, which is a shape the count could never have
                // shown.
                let mut disagreeing_bands = vec![0usize; bands];
                for (pixel, texel) in drawn.iter().enumerate() {
                    // Whose pixel this is, as the renderer wrote it. A mesh
                    // face's row is addressed through the `MeshFace` sentinel
                    // — `place::Stance::MeshFace`'s own doc — so all three of
                    // kind, sentinel and row have to be this face's.
                    if texel.kind != openshard_client_render::place::Kind::Static as u32
                        || texel.stance != Stance::MeshFace as u32
                        || texel.id != own_row
                    {
                        continue;
                    }
                    // The fragment's own world position, read whole off the
                    // position plane. This is the point the shader lit.
                    let (x, y, z) = texel.at;
                    // Out of every flame's reach is not shadowed — the ground
                    // oracle above carries the argument, and this is the same
                    // comparison against the same picture.
                    let shade = Shade::of([
                        shadow_pixels[pixel * 4],
                        shadow_pixels[pixel * 4 + 1],
                        shadow_pixels[pixel * 4 + 2],
                    ]);
                    if shade == Shade::Unreached {
                        unreached += 1;
                        continue;
                    }
                    sampled += 1;
                    let gpu_lit = shade.lit();
                    let independent = oracle_visible((x, y, z), light_point, &boxes, index) > 0.5;
                    if independent != gpu_lit {
                        disagreeing += 1;
                        let up = ((z - b.min.2) / (b.max.2 - b.min.2) * bands as f64) as usize;
                        disagreeing_bands[up.min(bands - 1)] += 1;
                        let spot = light::Spot::face(
                            Vec2::new(x as f32, y as f32),
                            z as f32,
                            (i32::from(b.tile.0), i32::from(b.tile.1)),
                            face,
                        )
                        .part_of(solids[index]);
                        let through = light::sample(spot, &lighting)
                            .reaches
                            .first()
                            .map_or(0.0, |reach| if reach.within { reach.through } else { 0.0 });
                        match (through > 0.5) == independent {
                            true => shader_alone += 1,
                            false => engine_together += 1,
                        }
                        if examples.len() < 8 {
                            examples.push(format!(
                                "  [box {index} {label}] ({x:.3}, {y:.3}, {z:.3}): independent oracle \
                                 says {}, rendered says {}, light::sample through={through:.3}",
                                if independent { "lit" } else { "shadowed" },
                                if gpu_lit { "lit" } else { "shadowed" },
                            ));
                        }
                    }
                }
                eprintln!(
                    "face oracle, box {index}'s own {label} face: {sampled} pixels drawn, \
                     {disagreeing} disagree ({shader_alone} the shader alone, {engine_together} both \
                     walks together); {unreached} outside the flame's radius, not compared",
                );
                if disagreeing > 0 {
                    // Runs of adjacent bands rather than a band apiece: a defect
                    // that is a band prints as one entry, and one that is spread
                    // over the face prints as many, which is the distinction
                    // worth reading at a glance. Band 0 is the foot of the face.
                    let mut runs: Vec<String> = Vec::new();
                    let mut band = 0usize;
                    while band < disagreeing_bands.len() {
                        if disagreeing_bands[band] == 0 {
                            band += 1;
                            continue;
                        }
                        let start = band;
                        let mut points = 0usize;
                        while band < disagreeing_bands.len() && disagreeing_bands[band] > 0 {
                            points += disagreeing_bands[band];
                            band += 1;
                        }
                        let low = b.min.2 + start as f64 / bands as f64 * (b.max.2 - b.min.2);
                        let high = b.min.2 + band as f64 / bands as f64 * (b.max.2 - b.min.2);
                        runs.push(format!(
                            "bands {start}..{band} (z {low:.2}..{high:.2}, {points} pixels)"
                        ));
                    }
                    eprintln!("  where: {}", runs.join(", "));
                }
                for example in &examples {
                    eprintln!("{example}");
                }
                total_sampled += sampled;
                total_disagreeing += disagreeing;
            }
        }
        eprintln!(
            "face oracle vs rendered View::Shadow: {total_disagreeing}/{total_sampled} drawn face pixels \
             disagree"
        );
        assert!(
            total_sampled > 100,
            "the face oracle found only {total_sampled} pixels of the boxes' own vertical faces — a \
             detector that compares nothing reads exactly like a detector that found nothing"
        );
    }

    // The reference tracer: the same scene rendered again, by something with no
    // idea what a tile is. See this module's own "# The reference tracer".
    if env_opt("OPENSHARD_BOXES_PATHTRACE").as_deref() != Some("0") {
        // The art itself, as the world passes left it — the albedo half of the
        // shaded comparison, and the one number in this whole tool that has to
        // come off the picture rather than out of a scene description.
        let world_pixels = oracle::read_surface(&device, &queue, &world, width, height_px);
        pathtrace_comparison(PathtraceInputs {
            boxes: &boxes,
            light_at,
            light_radius: f64::from(light_radius),
            to_pixel: &to_pixel,
            width,
            height: height_px,
            drawn: &drawn,
            shadow_pixels: &shadow_pixels,
            face_rows: &face_rows,
            base: &base,
            flame: &lighting.lights[0],
            albedos: oracle::pathtrace::Albedos {
                ground: oracle::ground_albedo(&drawn, &world_pixels),
                // Measured since `docs/lighting_rebuild.md` phase 6d gave the
                // mesh pass a colour target — except on `scene_flat`, whose
                // whole point is that it has no boxes, so there is nothing on
                // the engine's side to read one off.
                body:   match boxes.is_empty() {
                    true => oracle::pathtrace::Albedos::INVENTED.body,
                    false => oracle::body_albedo(&drawn, &world_pixels),
                },
            },
            lit_pixels: &lit_pixels,
        });
    }
}

/// Everything [`pathtrace_comparison`] needs, in one struct because a function
/// of ten positional arguments is a function whose call site nobody can read.
struct PathtraceInputs<'a> {
    boxes:         &'a [BoxSpec],
    light_at:      WorldSpot,
    light_radius:  f64,
    /// The frame's own world-to-pixel map — the *renderer's*, handed over as a
    /// black box for [`Parallel::measure`] to recover. This is the one thing the
    /// tracer takes from this crate, and taking it as values rather than as a
    /// formula is what stops the reference camera from drifting into being
    /// nobody's camera.
    to_pixel:      &'a dyn Fn(WorldSpot) -> (f64, f64),
    width:         u32,
    height:        u32,
    drawn:         &'a [oracle::Drawn],
    shadow_pixels: &'a [u8],
    face_rows:     &'a [(usize, Stance, u32)],
    base:          &'a str,
    /// The flame this scene is actually lit by — the engine's own `Light`, so
    /// the reference emits what the renderer emits rather than a brightness
    /// picked to make its own picture readable.
    flame:         &'a Light,
    /// What the surfaces reflect. The ground's is read off the frame the world
    /// passes just drew; see [`ground_albedo`].
    albedos:       oracle::pathtrace::Albedos,
    /// The engine's own lit frame, `RGBA8`, for the shaded comparison — the
    /// picture `docs/lighting_rebuild.md`'s phase 0 is *about*. Empty when the
    /// run did not dump `View::Lit`, and the comparison then says so rather
    /// than drawing half of itself.
    lit_pixels:    &'a [u8],
}

/// Render the scene a second time with [`openshard_client_pathtrace`], lay the
/// two pictures beside each other, and count where they disagree.
///
/// The judging itself is `oracle::pathtrace`, shared with `tests/traced.rs` —
/// see that module's own doc for why it is the part that must not be
/// duplicated. What is here is what a *tool* adds to it: the pictures, the
/// knobs, and the full mode.
fn pathtrace_comparison(inputs: PathtraceInputs<'_>) {
    let PathtraceInputs {
        boxes,
        light_at,
        light_radius,
        to_pixel,
        width,
        height,
        drawn,
        shadow_pixels,
        face_rows,
        base,
        flame,
        albedos,
        lit_pixels,
    } = inputs;
    let size = pt_trace::ImageSize::new(width, height);

    // **The engine's own sphere where there is something to cast a shadow, and a
    // point where there is not** — the same choice `tests/traced.rs`'s two gates
    // make, for the same two reasons. A reference holding a point where the frame
    // holds a body reports the whole penumbra as a disagreement; and a scene with
    // no occluder in it has no penumbra to report, so the sixty-four samples buy
    // nothing there but a slower picture.
    //
    // And a point where the *frame* was asked for one, which is the third reader
    // of [`flame_radius`]: at `OPENSHARD_FLAME_RADIUS=0` the engine casts eight
    // rays at one place and a sphere on this side would be the only soft edge in
    // the comparison.
    let body = match boxes.is_empty() || flame_radius() == 0.0 {
        true => oracle::pathtrace::Body::Point,
        false => {
            oracle::pathtrace::Body::Sphere {
                radius:  f64::from(flame_radius()),
                samples: match oracle::pathtrace::ENGINE_FLAME {
                    oracle::pathtrace::Body::Sphere { samples, .. } => samples,
                    oracle::pathtrace::Body::Point => 1,
                },
            }
        }
    };
    let mirror = oracle::pathtrace::Mirror::of(oracle::pathtrace::Mirrored {
        boxes,
        light_at,
        light_radius,
        colour: flame.color.map(f64::from),
        intensity: f64::from(flame.intensity),
        albedos,
        body,
        to_pixel,
    });

    // **Visibility in one model and brightness in another, and phase 3 is what
    // split them.**
    //
    // `Brdf::Flat` still describes how the engine decides *whether* a pixel is
    // lit: a fragment's own body does not occlude it, which the shipped walk
    // states as an exemption and `docs/lighting_rebuild.md`'s phase 4 will state
    // as identity. So the visibility comparison — the whole of `compare` — stays
    // here, and `physical` beside it still measures how many pixels the choice of
    // model decides.
    //
    // How *bright* a lit pixel is, though, is a cosine now. The shaded picture is
    // rendered in `Brdf::Lambert` for that reason, out of a second mirror whose
    // flame carries `LAMBERT_PI`: the engine's diffuse term has no `1/π` and the
    // reference's does, and the intensity is where the two conventions meet.
    let exact = mirror.render(pt_trace::Brdf::Flat, oracle::pathtrace::FIRST_SEED, size);
    let physical = mirror.render(pt_trace::Brdf::Lambert, oracle::pathtrace::FIRST_SEED, size);
    let shaded = oracle::pathtrace::Mirror::of(oracle::pathtrace::Mirrored {
        boxes,
        light_at,
        light_radius,
        colour: flame.color.map(f64::from),
        intensity: f64::from(flame.intensity) * oracle::pathtrace::LAMBERT_PI,
        albedos,
        body,
        to_pixel,
    })
    .render(pt_trace::Brdf::Lambert, oracle::pathtrace::FIRST_SEED, size);
    let verdict = oracle::pathtrace::compare(
        &exact,
        &physical,
        oracle::pathtrace::Frame {
            size,
            drawn,
            picture: shadow_pixels,
            face_rows,
        },
    );
    eprint!("{}", verdict.report());

    // `OPENSHARD_BOXES_PROBE=x,y[,radius]` — two scanlines through one pixel,
    // each side's surface and the **world point** of it. `tools/mask_probe.py`
    // reads the dumped strips and can say which body a region is; what a picture
    // cannot say is which *face*, because a body's lid and its riser are one
    // colour. Guessing that from the shape of a region is how a shadow boundary
    // gets attributed to the wrong surface, and it has been.
    if let Some(spec) = env_opt("OPENSHARD_BOXES_PROBE") {
        let numbers: Vec<u32> = spec
            .split(',')
            .map(|n| n.trim().parse().expect("OPENSHARD_BOXES_PROBE is x,y[,radius]"))
            .collect();
        eprint!(
            "{}",
            oracle::pathtrace::probe(
                &exact,
                oracle::pathtrace::Frame {
                    size,
                    drawn,
                    picture: shadow_pixels,
                    face_rows,
                },
                pt_trace::ImagePixel::new(numbers[0], numbers[1]),
                numbers.get(2).copied().unwrap_or(6),
            )
        );
    }

    // And the *shape* of the soft edge, which the verdict above cannot see: it
    // reads both pictures as one bit a pixel, and a penumbra is exactly the
    // region where that bit is arbitrary. `docs/lighting_rebuild.md` phase 5.
    // Only where there is a penumbra to look at: `penumbra` says so by panicking,
    // which is right for a gate and wrong for a tool a person points at a scene
    // with no occluder in it.
    if !boxes.is_empty() {
        let allowed = oracle::pathtrace::PENUMBRA_ALLOWED;
        let soft = oracle::pathtrace::penumbra(
            &exact,
            &mirror.render(pt_trace::Brdf::Flat, oracle::pathtrace::SECOND_SEED, size),
            oracle::pathtrace::Frame {
                size,
                drawn,
                picture: shadow_pixels,
                face_rows,
            },
            allowed,
        );
        eprint!("{}", soft.report(allowed));
    }

    // The picture: the frame's own shadow decision, the tracer's, where they
    // differ, and why the pixels nobody compared were not compared. What each
    // strip means is `Verdict::strips`'s own doc — shared with `tests/traced.rs`
    // for the same reason the judging is, and the fourth strip is there because
    // its absence cost a session.
    let strips = verdict.strips();
    let strips: Vec<&[u8]> = strips.iter().map(Vec::as_slice).collect();
    write_strips(
        std::path::Path::new(&format!("{base}_pathtrace.png")),
        width,
        height,
        &strips,
    );

    // **The two shaded pictures, side by side.** `docs/lighting_rebuild.md`'s
    // phase 0: the instrument every later phase is judged by is a picture beside
    // the tracer's, looked at by a person — and until this, the tool wrote the
    // engine's frame and the tracer's as two files and laid only their *shadow
    // masks* beside each other.
    //
    // The engine's own `View::Lit` frame, the tracer's exact render of the same
    // scene in the same light model, and the difference between them per
    // channel. Both encoded by `tonemap::encode`, which is the second half of
    // the curve `blit.wesl` ends with: two pictures encoded by two spellings
    // would differ by the spellings.
    shaded_comparison(&shaded, lit_pixels, &verdict, width, height, base);

    // And the full mode, on request: a real Monte Carlo render of the same
    // scene, with a spherical emitter, a cosine term and bounced light. It is
    // deliberately not compared against anything — none of what it adds exists
    // in the renderer, so every pixel would "disagree". It is here to be looked
    // at.
    let samples: u32 = env_or("OPENSHARD_BOXES_PATHTRACE_SAMPLES", "0")
        .parse()
        .expect("a number");
    if samples == 0 {
        return;
    }
    // `0` is the point emitter, not a sphere of no radius. The two draw the same
    // sample — `Light::sample`'s own `Sphere` branch spreads by `radius * √u`,
    // which is the centre when the radius is nought — but only one of them
    // *says* so: `exact_in_samples` reports `Some(1)` for a point and `None` for
    // any sphere, and that is what `Image::is_exact` reads. A knob whose zero
    // produces an exact picture the image itself calls inexact is a knob that
    // has to be spelled, and this is where the spelling belongs: the tracer's
    // own enum already has both shapes.
    //
    // What it buys is the one picture neither mode had — hard shadows *with* a
    // cosine, bounced light and a sky. Degenerate mode is a bit per pixel and
    // full mode was always soft, so "which of these edges is the penumbra"
    // could only be answered by rendering the same frame at two radii.
    let radius: f64 = env_or("OPENSHARD_BOXES_PATHTRACE_EMITTER", "0.35")
        .parse()
        .expect("a number");
    let soft = pt_light::Light {
        emitter: match radius {
            0.0 => pt_light::Emitter::Point,
            radius => pt_light::Emitter::Sphere { radius },
        },
        ..mirror.flame
    };
    let bounces: u32 = env_or("OPENSHARD_BOXES_PATHTRACE_BOUNCES", "2")
        .parse()
        .expect("a number");
    let emitter = match soft.emitter {
        pt_light::Emitter::Point => "a point emitter, hard shadows".to_string(),
        pt_light::Emitter::Sphere { radius } => format!("a sphere of {radius} tiles"),
    };
    eprintln!("path tracer, full mode: {samples} samples, {bounces} bounces, {emitter} — this takes a while");
    let full = pt_trace::render(
        &mirror.scene,
        &mirror.camera,
        &[soft],
        &pt_trace::Settings {
            samples,
            bounces,
            // A dim sky, so a bounce that escapes the scene brings something
            // back and ambient occlusion has a lit background to occlude.
            sky: [0.05, 0.06, 0.09],
            seed: 1,
            // Physics, and not the engine's model: this picture is the one
            // showing what the shipped model does not contain, and a cosine is
            // most of that.
            ..pt_trace::Settings::degenerate()
        },
        size,
    );
    let exposure: f64 = env_or("OPENSHARD_BOXES_PATHTRACE_EXPOSURE", "1.0")
        .parse()
        .expect("a number");
    let mut pixels = vec![0u8; (width * height * 3) as usize];
    for (pixel, traced) in full.pixels.iter().enumerate() {
        // Linear radiance to something a viewer shows, through **the renderer's
        // own curve** — `tonemap::encode`, the second half of what `blit.wesl`
        // ends every lit frame with. This spelled the sRGB transfer function out
        // by hand until phase 0, with a `clamp` where the tonemap's shoulder is,
        // so the reference and the frame took two different paths out of linear
        // light and a picture of one could not be laid beside a picture of the
        // other. `docs/lighting_rebuild.md` phase 1's own rule: nothing in this
        // crate spells these curves a second time.
        //
        // The exposure knob stays a knob, and stays *outside* the curve: it says
        // how much light this scene has, which is a property of the scene and
        // not of how a viewer shows it.
        let encoded = openshard_client_render::tonemap::encode_u8(
            traced.radiance.map(|channel| (channel * exposure) as f32),
        );
        pixels[pixel * 3..pixel * 3 + 3].copy_from_slice(&encoded);
    }
    write_strips(
        std::path::Path::new(&format!("{base}_pathtrace_full.png")),
        width,
        height,
        &[&pixels],
    );
}

/// The engine's shaded frame, the tracer's, and the difference — one picture,
/// and a line of numbers under it.
///
/// **`docs/lighting_rebuild.md`'s phase 0 instrument.** Not a gate: it prints
/// and draws, and a person decides. The gate with the same subject is
/// `tests/traced.rs`'s brightness test, which runs the flat scene where the two
/// sides can be held to a quantisation and asserts on it.
///
/// Which pixels are compared is [`oracle::pathtrace::compare`]'s own answer,
/// taken off the verdict rather than decided again here: a pixel counts when
/// both renderers agree what surface is there. A second rule for it would be a
/// second opinion about what the picture is of.
///
/// The difference strip is per channel and **amplified**, because that is what
/// makes it useful: an agreement to within a step or two of eight bits is the
/// success case, and drawn honestly it is a black rectangle indistinguishable
/// from a comparison that ran on nothing. The scale is printed beside it.
fn shaded_comparison(
    traced: &pt_trace::Image,
    lit: &[u8],
    verdict: &oracle::pathtrace::Verdict,
    width: u32,
    height: u32,
    base: &str,
) {
    if lit.is_empty() {
        eprintln!(
            "no `View::Lit` frame was dumped, so the shaded comparison has only one side — add \
             `lit` to OPENSHARD_BOXES_VIEWS"
        );
        return;
    }
    // How much a difference of one eight-bit step is drawn as. Eight, so a
    // single step of quantisation is a visible dark grey and four steps — which
    // is a real disagreement about light — is already bright.
    const AMPLIFY: u16 = 8;

    let mut ours = vec![0u8; (width * height * 3) as usize];
    let mut theirs = vec![0u8; (width * height * 3) as usize];
    let mut difference = vec![0u8; (width * height * 3) as usize];
    let (mut compared, mut worst, mut total) = (0usize, 0u16, 0u64);
    // How many compared pixels sit within each step of the other picture, so the
    // report says *how* close rather than only how far the worst one is.
    let mut within = [0usize; 4];
    for pixel in 0..(width * height) as usize {
        let engine = [lit[pixel * 4], lit[pixel * 4 + 1], lit[pixel * 4 + 2]];
        let reference = openshard_client_render::tonemap::encode_u8(
            traced.pixels[pixel].radiance.map(|channel| channel as f32),
        );
        ours[pixel * 3..pixel * 3 + 3].copy_from_slice(&engine);
        theirs[pixel * 3..pixel * 3 + 3].copy_from_slice(&reference);
        if verdict.engine_lit[pixel].is_none() {
            continue;
        }
        compared += 1;
        let mut apart = 0u16;
        for channel in 0..3 {
            let step = engine[channel].abs_diff(reference[channel]);
            apart = apart.max(u16::from(step));
            difference[pixel * 3 + channel] = (u16::from(step) * AMPLIFY).min(255) as u8;
        }
        worst = worst.max(apart);
        total += u64::from(apart);
        for (steps, count) in within.iter_mut().enumerate() {
            *count += usize::from(apart <= steps as u16);
        }
    }
    assert!(
        compared > 0,
        "the shaded comparison had no pixel where both renderers agree what surface is there — it \
         is measuring nothing, which reads exactly like measuring an agreement"
    );
    eprintln!(
        "shaded frame vs path tracer: {compared} pixels compared, worst channel {worst} steps of \
         255, mean {:.2}; within 0/1/2/3 steps: {}/{}/{}/{}",
        total as f64 / compared as f64,
        within[0],
        within[1],
        within[2],
        within[3],
    );
    eprintln!("  the difference strip is amplified {AMPLIFY}× — one step of eight bits draws as {AMPLIFY}");
    write_strips(
        std::path::Path::new(&format!("{base}_lit_vs_traced.png")),
        width,
        height,
        &[&ours, &theirs, &difference],
    );
}

/// [`openshard_client_render::png::write_strips`], with this tool's own "say
/// what you wrote" on the end of it.
fn write_strips(path: &std::path::Path, width: u32, height: u32, strips: &[&[u8]]) {
    openshard_client_render::png::write_strips(path, width, height, strips).expect("writing the comparison");
    eprintln!("wrote {}", path.display());
}
