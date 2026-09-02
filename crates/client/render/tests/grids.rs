//! **The constants that relate one grid to another, wherever they are written
//! down twice.** `docs/render/design_pixel_spaces.md` P4, the half of it that is not
//! `camera::tests::no_primary_sample_lands_on_a_whole_virtual_pixel`.
//!
//! P2's table says most pairs of grids in this renderer are commensurate *by
//! construction* — a tile corner is a whole world pixel because `TILE_WIDTH / 2`
//! is an integer, and one `Point.z` unit is exactly eleven of `light.rs`'s tile
//! units because `TILE_WIDTH / Z_STEP` divides evenly. Both claims are about
//! constants, and a claim about a constant is only as good as the number staying
//! what the claim was written against.
//!
//! Which would be nothing to test if there were one copy of each. There is not:
//! P1's census found `TILE_WIDTH` and `Z_STEP` restated as independent literals
//! in [`facing`](openshard_client_render::facing) — deliberately, per its own
//! comment, so a function handed nothing but pixels need not be handed a camera
//! — and restated again in the shaders, where the Rust constant cannot reach at
//! all. `docs/render/design_pixel_spaces.md`'s backlog carries this as *"`Z_STEP` and `Z_PER_TILE`
//! are one relationship written twice"*; a copy across the wire is the same
//! hazard with no compiler on either side of it.
//!
//! So the shader's copies are read back out of the shader's own source and
//! pinned. A constant that is renamed or deleted turns this red rather than
//! passing vacuously, which is the difference between a gate and a search.
//!
//! `facing`'s copies are pinned in its own module, where they are visible:
//! `facing::tests::a_tile_is_the_width_the_camera_draws_one_at`.

use openshard_client_render::camera::{
    TILE_HEIGHT,
    TILE_WIDTH,
    Z_STEP,
};
use openshard_client_render::impostor::{
    self,
    Fringe,
};
use openshard_client_render::light::Z_PER_TILE;
use openshard_client_render::occlusion::Edges;

const IMPOSTOR: &str = include_str!("../src/shaders/impostor.wesl");
const STATICS: &str = include_str!("../src/shaders/statics.wesl");

/// One `const NAME: f32 = <number>;` out of a shader's source.
///
/// Panics when the name is absent, and that is the point: a shader that renames
/// its copy of a grid constant has not stopped having one, and a helper that
/// answered `None` here would let the rename read as "nothing to pin".
fn shader_const(source: &str, file: &str, name: &str) -> f32 {
    let prefix = format!("const {name}: f32 = ");
    let line = source
        .lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| {
            panic!(
                "{file} no longer declares `{name}` — if the shader's copy of this grid constant \
                 moved or was renamed, move this pin with it; if it genuinely stopped having one, \
                 say so here and take the case out"
            )
        });
    let value = line[prefix.len()..]
        .split(';')
        .next()
        .unwrap_or_else(|| panic!("{file}'s `{name}` has no `;`: {line}"));
    value
        .trim()
        .parse()
        .unwrap_or_else(|error| panic!("{file}'s `{name}` is not a number: {value:?} ({error})"))
}

/// The same, for a constant a shader declares as a `u32` — `15u` and not `15.0`.
///
/// A second spelling rather than a parameter on [`shader_const`] because the two
/// suffixes are the whole difference and a caller that got the type wrong would
/// otherwise be told the constant is missing, which is the one answer this
/// helper family exists never to give.
fn shader_u32(source: &str, file: &str, name: &str) -> u32 {
    let prefix = format!("const {name}: u32 = ");
    let line = source
        .lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| {
            panic!(
                "{file} no longer declares `{name}` — if the shader's copy of this constant moved \
                 or was renamed, move this pin with it; if it genuinely stopped having one, say so \
                 here and take the case out"
            )
        });
    let value = line[prefix.len()..]
        .split(';')
        .next()
        .unwrap_or_else(|| panic!("{file}'s `{name}` has no `;`: {line}"))
        .trim()
        .trim_end_matches('u');
    value
        .parse()
        .unwrap_or_else(|error| panic!("{file}'s `{name}` is not a number: {value:?} ({error})"))
}

/// **`statics.wesl`'s `EDGES_ANY` is [`Edges::ANY`]'s own four bits.**
///
/// The mask decides which fragments get a facing at all: a box the art named a
/// side of writes the face its view ray met, and a **body** — all four bits,
/// which is `edges_of`'s way of saying *none* — writes no facing, so
/// `blit.wesl` lights it from every side. Getting the number out of step here
/// does not fail to draw. It draws every body with a camera-facing normal again
/// (a lamp black in its own light) or every panel without one (a wall lit
/// through its own back), and both are pictures rather than errors.
///
/// Pinned from the shader's own source for the reason `docs/render/design_pixel_spaces.md` rule 6
/// states: there is no compiler on either side of that wire.
#[test]
fn the_statics_pass_knows_which_mask_means_the_art_named_no_side() {
    assert_eq!(
        shader_u32(STATICS, "statics.wesl", "EDGES_ANY"),
        u32::from(Edges::ANY.raw()),
        "statics.wesl's EDGES_ANY against occlusion::Edges::ANY",
    );
}

/// **The shaders' copies of the grid constants are the camera's numbers.**
///
/// Three of them, and each is a different pair of grids meeting on the GPU:
/// `impostor.wesl`'s `TILE_WIDTH` converts view-plane pixels to tiles in
/// `ray_from`, its `Z_PER_TILE` is `VIEW.z` — the one place a shader states the
/// tile-space metric [`openshard_client_render::light::TileVec`] carries on this
/// side — and `statics.wesl`'s `Z_STEP` lifts a sprite by its own height.
///
/// A disagreement here does not fail to compile and does not fail to draw. It
/// draws a frame at a slightly different scale from the one every test on this
/// side asserts about, which is `docs/render/design_frame_assembly.md`'s subject exactly: two pictures
/// rather than one wrong one.
#[test]
fn the_shaders_restate_the_cameras_constants_and_not_their_own() {
    assert_eq!(
        shader_const(IMPOSTOR, "impostor.wesl", "TILE_WIDTH"),
        TILE_WIDTH as f32,
        "impostor.wesl's tile width against camera::TILE_WIDTH",
    );
    assert_eq!(
        shader_const(IMPOSTOR, "impostor.wesl", "Z_PER_TILE"),
        Z_PER_TILE,
        "impostor.wesl's tile-space metric against light::Z_PER_TILE",
    );
    assert_eq!(
        shader_const(STATICS, "statics.wesl", "Z_STEP"),
        Z_STEP as f32,
        "statics.wesl's height quantum against camera::Z_STEP",
    );
    // **The hit tolerance is a grid constant now**, which is why it is pinned
    // here beside the widths rather than left as a number two files happen to
    // agree on: it is one step of the fragment grid expressed in tiles, so it
    // moves with `TILE_WIDTH` above and with nothing else. The shader states the
    // literal; this is what keeps the literal honest. Compared to seven
    // decimals, which is where an `f32`'s own precision ends — a tighter
    // equality would fail on the printed constant rather than on a disagreement.
    assert!(
        (shader_const(IMPOSTOR, "impostor.wesl", "FRAGMENT") - impostor::FRAGMENT).abs() < 1e-7,
        "impostor.wesl's hit tolerance is {} and impostor::FRAGMENT is {}",
        shader_const(IMPOSTOR, "impostor.wesl", "FRAGMENT"),
        impostor::FRAGMENT,
    );
    // And the same step expressed along the ray's own parameter, which decides
    // the face at a box's top edge rather than whether a pixel is a point of the
    // box at all. Pinned for the same reason and against the same hazard: two
    // files, no compiler between them, and a disagreement draws a different
    // frame rather than failing.
    assert!(
        (shader_const(IMPOSTOR, "impostor.wesl", "RIM") - impostor::FRAGMENT / 3.0_f32.sqrt()).abs() < 1e-7,
        "impostor.wesl's rim is {} and FRAGMENT / sqrt(3) is {}",
        shader_const(IMPOSTOR, "impostor.wesl", "RIM"),
        impostor::FRAGMENT / 3.0_f32.sqrt(),
    );
    assert_eq!(
        shader_const(STATICS, "statics.wesl", "HALF_TILE_HEIGHT"),
        (TILE_HEIGHT / 2) as f32,
        "statics.wesl's half tile against camera::TILE_HEIGHT / 2",
    );
    // **And the fringe switch's three states**, which are a grid constant's
    // problem exactly: a number written in two files with no compiler between
    // them, riding in a uniform block. Getting these out of step does not fail
    // to draw — it draws a *different one of three pictures* than the one the
    // switch says is on the screen, which is the one thing a knob for looking at
    // frames must not do.
    for state in [Fringe::Clamp, Fringe::Discard, Fringe::Volume] {
        let name = format!("FRINGE_{}", state.name().to_uppercase());
        assert_eq!(
            shader_const(IMPOSTOR, "impostor.wesl", &name),
            state as u32 as f32,
            "impostor.wesl's {name} against impostor::Fringe::{state:?}",
        );
    }
}

/// **`Point.z` and tile space are commensurate because the division is exact** —
/// `docs/render/design_pixel_spaces.md` P2's second row, as an assertion rather than a sentence.
///
/// [`Z_PER_TILE`] is *defined* as `TILE_WIDTH / Z_STEP`, so the two can never
/// disagree; what they can do is stop dividing evenly. Then `Z_PER_TILE` is
/// still a number, every expression still compiles, and one `z` unit quietly
/// stops being a whole count of tile-space units — which is the row's claim, and
/// the only thing between it and being wrong is `44 % 4`.
///
/// The eleven is pinned as well, not because eleven is load-bearing on its own
/// but because `impostor.wesl` writes it as a literal and the pin above compares
/// against this.
#[test]
fn a_height_unit_is_a_whole_number_of_tile_space_units() {
    assert_eq!(
        TILE_WIDTH % Z_STEP,
        0,
        "TILE_WIDTH / Z_STEP no longer divides evenly, so one unit of `Point.z` is no longer a \
         whole count of tile-space units — docs/render/design_pixel_spaces.md P2's `Tile z ↔ tile space` row is what \
         stops being true, and `light::TileVec`'s two crossings are where it shows",
    );
    assert_eq!(Z_PER_TILE, (TILE_WIDTH / Z_STEP) as f32);
    assert_eq!(Z_PER_TILE, 11.0);
}

/// **A tile corner is a whole world pixel** — P2's first row, same treatment.
///
/// `project` moves a tile by `HALF_WIDTH = TILE_WIDTH / 2` per step on each
/// axis. That the halving is exact is why the tile grid and the world-pixel grid
/// are commensurate at every rung and parity, upstream of any camera — and it is
/// why a view ray through a whole virtual pixel is a ray through a box's own
/// corner, which is the whole of the window-parity defect. An odd `TILE_WIDTH`
/// would not break the renderer; it would break the *derivation*, silently.
#[test]
fn a_tile_step_is_a_whole_number_of_world_pixels() {
    assert_eq!(TILE_WIDTH % 2, 0, "a tile's width no longer halves exactly");
    assert_eq!(TILE_HEIGHT % 2, 0, "a tile's height no longer halves exactly");
}
