// Where a sprite's pixel is in the world, and which surface it is a pixel of.
//
// `crate::impostor` is the Rust twin and carries the argument at length: the
// projection has one null direction, so every point that draws to a given pixel
// lies on a line in the constant direction `VIEW`, and a fragment is a *slab
// test* rather than an unprojection. `docs/render/design_model.md` phase 6.
//
// **Two spellings, and one thing compares them.** This file and `impostor.rs`
// are the same arithmetic written twice, the way `normal_format.wesl` and
// `gbuffer::pack_normal` are — and as there, what keeps them honest is a frame
// test that renders a sprite and asserts the *word the GPU wrote* equals what
// the Rust side answers for the same boxes, rather than two readers agreeing
// about a picture.
//
// The **selection** rule — which of a static's boxes a fragment belongs to — is
// not here: it needs the storage buffer the boxes live in, which is a pass's own
// binding, so `statics.wesl` spells the loop and this spells one meeting. See
// `impostor::nearest` for the rule that loop implements.

// How many `z` units make one tile.
//
// **The metric, and this is its one spelling in a shader.** `light::Z_PER_TILE`
// is the Rust side of it; `blit.wesl` imports this rather than declaring a
// second, because `docs/render/design_model.md`'s *One metric* is the line half of
// `docs/archive/render/world_coordinates.md` is about.
const Z_PER_TILE: f32 = 11.0;

// A tile's width in virtual pixels — `camera::TILE_WIDTH`, which in this
// projection is also how many pixels a whole tile of `x - y` covers.
const TILE_WIDTH: f32 = 44.0;

// The direction every screen pixel's world line runs in, in world units — tiles
// on `x` and `y`, `z` units on `z`, and exactly `(1, 1, 1)` once `z` is divided
// into tiles.
//
// It points **towards the camera**: a point further along it draws to the same
// pixel and stands in front. So the surface a pixel shows is where the ray
// *leaves* a box, not where it enters one, and the three faces it can ever name
// are `max.x`, `max.y` and `max.z` — `crate::solid::Side`'s own three.
const VIEW: vec3<f32> = vec3<f32>(1.0, 1.0, Z_PER_TILE);

// How far outside a box, in tiles, a meeting may measure and still be a meeting
// rather than a miss: **one fragment of the screen grid**, `SQRT_2 / TILE_WIDTH`
// — the distance `ray_from` moves when `across` changes by one virtual pixel,
// taken diagonally.
//
// `crate::impostor::FRAGMENT` carries the argument and the control that measured
// it. Written here as the number rather than as the division because
// `tests/grids.rs` pins a shader's copy of a grid constant by reading the
// literal, and a copy that drifts from `TILE_WIDTH` above is what that pin is
// for.
const FRAGMENT: f32 = 0.032141216;

// `FRAGMENT` as a step of the ray's own parameter — `impostor::RIM`, and the
// same literal-rather-than-division rule as above so `grids.rs` can pin it. One
// unit of `t` moves the point by `VIEW`, which is `(1, 1, Z_PER_TILE)` in world
// units and so `(1, 1, 1)` in tile space: `t` and tiles differ by `sqrt(3)`.
const RIM: f32 = 0.01855674;

// How many virtual pixels tall one `z` unit draws — `camera::Z_STEP`, written as
// the division of the two constants above so it cannot drift from either.
const Z_STEP: f32 = TILE_WIDTH / Z_PER_TILE;

// Whether a box this tall has side faces a sample could land on: the band they
// draw as is `(hi.z - lo.z) * Z_STEP` virtual pixels, and under one pixel there
// is no fragment centre inside it. `impostor::shows_a_side` carries the
// argument — this was `hi.z > lo.z` while a lid was a plane, and a lid is
// `LID_THICKNESS` deep now, a sixteenth of a pixel.
fn shows_a_side(lo_z: f32, hi_z: f32) -> bool {
    return (hi_z - lo_z) * Z_STEP > 1.0;
}

// Where the view ray through one sprite fragment starts: the point at the
// static's own base height.
//
// `across` is how far the fragment is right of the column its sprite is centred
// on and `down` how far below the row its tile's own centre projects to, both in
// virtual pixels — the two numbers the vertex stage already hands the fragment
// stage. **This is the inverse projection the flat case used to spell**, and
// that is the point rather than a coincidence: a floor's pixel is this ray at
// `t = 0`, and every other case was the same ray met with a plane guessed from a
// stance instead of measured against a box.
fn ray_from(tile: vec2<f32>, base: f32, across: f32, down: f32) -> vec3<f32> {
    // `u - v = across/22` and `u + v = down/22 + 1`, solved. Written over the
    // whole tile width rather than the half so that neither line divides twice.
    return vec3<f32>(
        tile.x + (across + down) / TILE_WIDTH + 0.5,
        tile.y + (down - across) / TILE_WIDTH + 0.5,
        base,
    );
}

// Where a fragment of a **billboard** is: the point of its own view ray on the
// vertical plane the sprite is drawn on.
//
// `docs/render/design_model.md` phase 7, and the half of it that is not a choice.
// A mobile has no volume, so `meets` has nothing to meet and the pass fell back
// to a *point* — the middle of the tile, with the height running down the
// picture — which is the same point for every pixel of a screen row. A figure
// lit flat across, and `blit.wesl`'s `dither` turning the sample pattern by an
// angle that belongs to the position, so an eight-ray estimate lands in
// horizontal bands across it. Both are one cause.
//
// The plane stands through the tile's centre, contains the vertical, and runs
// along the screen's own `x` axis — `(1, -1)` in tiles, which is what `ray_from`
// moves along when only `across` changes. Its normal is therefore `(1, 1, 0)`,
// `VIEW`'s horizontal part: turned towards the camera, which is what a billboard
// is. The height falls out unchanged, `Z_PER_TILE / TILE_WIDTH` being `1 /
// Z_STEP` exactly — what the old point had wrong was `x` and `y` and nothing
// else.
//
// `impostor::billboard_at`, and the two are one formula.
fn billboard_at(tile: vec2<f32>, base: f32, across: f32, down: f32) -> vec3<f32> {
    return vec3<f32>(
        tile.x + 0.5 + across / TILE_WIDTH,
        tile.y + 0.5 - across / TILE_WIDTH,
        base - down * Z_PER_TILE / TILE_WIDTH,
    );
}

// The billboard plane's own normal — `(1, 1, 0)` normalised, `VIEW`'s
// horizontal part. `docs/render/design_model.md` phase 7's "facing the camera"
// candidate; `impostor::billboard_normal`, and the two are one formula.
fn billboard_normal() -> vec3<f32> {
    return normalize(vec3<f32>(1.0, 1.0, 0.0));
}

// The fixed-point scale `crate::mobiles::billboard_offset` packs a step's
// fraction at — the two spellings of one arithmetic, kept equal the way
// `Z_PER_TILE` and `FRAGMENT` above are.
const BILLBOARD_OFFSET_SCALE: f32 = 4096.0;

// How far a mobile's drawn position has walked past the tile its own
// `SpriteQuad::place` carries — two fixed-point `i16`s riding in `twin`, low
// half `x` and high half `y`, the way `crate::mobiles::billboard_offset`
// wrote them.
//
// **Why `billboard_at` needs this at all.** The screen quad is placed at the
// body's real, fractional position (`mobiles::cell_centre`), but the tile
// `billboard_at` is handed is the whole one `Place::of_mobile` carries —
// `Place` has no bits for a fraction. Mid-step the two disagree by up to a
// tile, and without this a walking figure was lit as though it stood on the
// tile it is arriving at rather than where its picture actually is —
// `crate::mobiles::billboard_offset`'s own doc has the account of what that
// looked like on screen.
fn billboard_offset(word: u32) -> vec2<f32> {
    let dx = f32(extractBits(bitcast<i32>(word), 0u, 16u));
    let dy = f32(extractBits(bitcast<i32>(word), 16u, 16u));
    return vec2<f32>(dx, dy) / BILLBOARD_OFFSET_SCALE;
}

// Where the view ray meets one box, and which of its faces that is.
struct Meeting {
    // The point, in world units, clamped into the box.
    at: vec3<f32>,
    // Which way the surface it met looks — one of `+x`, `+y`, `+z`.
    normal: vec3<f32>,
    // How far, in **tiles**, the ray passed outside the box before that clamp
    // pulled it back. At most `FRAGMENT` for a ray this grid can tell from one
    // that went through; `hit` is that comparison spelled once. This is the number
    // `docs/render/design_model.md` phase 6 asks its own "done when" to carry — a
    // sprite overhangs its own volume wherever the fitted box is narrower than
    // the art, and what matters is not that such pixels exist but how far out
    // they are.
    outside: f32,
    // How far along `VIEW` the meeting is, from `ray_from`'s own point. Only the
    // selection loop reads it: the box with the largest `t` is the one in front.
    t: f32,
}

// Whether the ray went through the box at all, as against passing beside it and
// being clamped onto it.
//
// "At all" is a question about this screen's grid: an edge crossing between two
// samples leaves the nearer fragment measuring a fraction of a `FRAGMENT`
// outside a box it is plainly a pixel of, and asking for zero there answers "a
// point of nothing" about a pixel drawn on the surface.
fn hit(met: Meeting) -> bool {
    return met.outside <= FRAGMENT;
}

// Which face this box shows the camera at its largest — the face a miss takes
// under `crate::impostor::Fringe::Volume`, which is a **debug state and not the
// default**: it was written, measured and refused, and it is reachable only
// through that switch so the refusal can be checked by eye.
//
// The score is the face's own projected area and not a preference: the
// projection has one null direction, so a face's area on screen is its world
// area times the cosine to `VIEW`, and for an axis-aligned face those are the
// other two extents and that axis's own `VIEW` component. A panel presents its
// own side, a whole tile three `z` units tall presents its lid. Ties go to `z`,
// then `y`, then `x` — `meets`'s own order. A lid is refused a side by
// `shows_a_side`, the same rule `meets` refuses it one by.
//
// `impostor::presented_face`, and the two are one formula.
fn presented_face(lo: vec3<f32>, hi: vec3<f32>) -> vec3<f32> {
    let extent = hi - lo;
    var face = vec3<f32>(0.0, 0.0, 1.0);
    var most = VIEW.z * extent.x * extent.y;
    if shows_a_side(lo.z, hi.z) {
        let sideways = VIEW.y * extent.z * extent.x;
        if sideways > most {
            most = sideways;
            face = vec3<f32>(0.0, 1.0, 0.0);
        }
        let across = VIEW.x * extent.y * extent.z;
        if across > most {
            most = across;
            face = vec3<f32>(1.0, 0.0, 0.0);
        }
    }
    return face;
}

// The three states of that switch, as the numbers the uniform block carries.
// `crate::impostor::Fringe` is the enum they are the contract with, and
// `grids.rs` reads them back out of this source.
const FRINGE_CLAMP: f32 = 0.0;
const FRINGE_DISCARD: f32 = 1.0;
const FRINGE_VOLUME: f32 = 2.0;

// Where the view ray through `start` meets the box `lo..=hi`.
//
// Always answers. A ray that misses is answered with the point of the box
// nearest it and `Meeting::outside` is what says so — one expression rather than
// a branch, because at tangency the two cases are the same point and a branch
// there would be a seam.
//
// The face is the one whose *exit* comes first. **Ties go to `z`, then `y`, then
// `x`**, which is a decision and not an accident: two exits are equal exactly on
// an edge or a corner, and on a degenerate box — whose `z` slab is a point —
// they are equal over the whole of it. Preferring `z` is what makes such a box
// read as a lid all the way to its own rim. `crate::impostor::meets`.
fn meets(start: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> Meeting {
    // No parallel-axis branch: every component of `VIEW` is positive, so `lo` is
    // always the near end of a slab and `hi` the far one, and no division here
    // can be by zero.
    let far = (hi - start) / VIEW;
    // The `z` face, which every box in this grid has — `Solid::box_of` gives a
    // panel `PANEL_THICKNESS` and everything else a whole tile, so `x` and `y`
    // are never degenerate.
    var exit = far.z;
    var normal = vec3<f32>(0.0, 0.0, 1.0);
    // **A face with no area is not a face.** A box flat in `z` presents `x` and
    // `y` faces that are lines, and the only face of it a ray can meet is its own
    // plane. Both those areas carry the `z` extent as a factor, so one test
    // retires both. Ordering the tie towards `z` used to stand in for this and
    // covered only the *exact* tie: a ray leaving the footprint one rounding
    // before it reaches the plane is a miss, and it was answered with a side
    // face's normal — one pixel per tile corner, a lattice of dots over a roof,
    // each shaded as a wall in the middle of a floor. `impostor::meets` has the
    // whole account, including why no box `Solid::box_of` builds is flat any
    // more (`docs/render/design_frame_assembly.md` P4 step 1) and why the rule stays regardless.
    //
    // **And "no area" is measured against the grid rather than against zero.**
    // A side face draws as a band `(hi.z - lo.z) * Z_STEP` virtual pixels tall,
    // and `LID_THICKNESS` is a sixteenth of one — a face with area that no
    // sample can land on. `impostor::shows_a_side` is the twin.
    if shows_a_side(lo.z, hi.z) {
        if far.y < exit {
            exit = far.y;
            normal = vec3<f32>(0.0, 1.0, 0.0);
        }
        if far.x < exit {
            exit = far.x;
            normal = vec3<f32>(1.0, 0.0, 0.0);
        }
        // **And a side wins only by more than the picture can show.** The same
        // argument as `FRAGMENT` and as `shows_a_side`, on the last thing about
        // this box the grid was not being asked about: a face that beats the lid
        // by less than the distance to the next sample has not beaten it. The
        // band is the box's own top edge, one fragment wide, which along a
        // projected diagonal is the stepped dashed seam a person reports on a
        // run of abutting counters — where the art carries on over the join and
        // a side face's full cosine lands in the middle of a tabletop.
        // `impostor::meets` and `impostor::RIM` carry the whole account. The
        // test is written so that a ray already leaving by the lid re-takes it,
        // which is what it was doing.
        if far.z - exit <= RIM {
            exit = far.z;
            normal = vec3<f32>(0.0, 0.0, 1.0);
        }
    }

    // The exit axis's own coordinate is **stated** and not computed: `exit` is
    // the `t` at which this ray reaches the plane `hi[axis]`, so the point is on
    // that plane by definition, and `start + exit * VIEW` is a division and a
    // multiplication that only round back to it. `VIEW.z` is `Z_PER_TILE` —
    // eleven, and no power of two — so the `z` round trip has nothing exact
    // about it, and a driver contracting the two into an `fma` need not agree
    // with one that does not. What the whole pass downstream leans on is that a
    // fragment is a point *of* the box it names; taking the plane from the
    // number that chose it makes that hold bit for bit rather than to within a
    // rounding. `impostor::meets` is the twin, and
    // `frame.rs`'s `a_sprite_fragment_is_a_point_of_the_primitive_it_names`
    // asserts it as an equality on the strength of this.
    let travelled = start + exit * VIEW;
    let at = select(
        select(vec3<f32>(travelled.x, travelled.y, hi.z), vec3<f32>(travelled.x, hi.y, travelled.z), normal.y > 0.0),
        vec3<f32>(hi.x, travelled.y, travelled.z),
        normal.x > 0.0,
    );
    let clamped = clamp(at, lo, hi);
    // In tiles, which means the `z` term is divided into them: a third of a tile
    // out sideways and a third of a tile out in height are the same miss, and
    // three `z` units are not.
    let miss = at - clamped;

    var met: Meeting;
    met.at = clamped;
    // **A miss keeps this face too**, though a ray that passed beside the box
    // left it by none: what the exit names there is whichever plane the clamp
    // reached first. Giving it `presented_face` instead was written, measured
    // and refused — `impostor::presented_face` carries the numbers — so that
    // rule lives behind `FRINGE_VOLUME` in the pass rather than here.
    met.normal = normal;
    // A distance rather than the slab test's own `enter <= exit` predicate, and
    // the two cannot disagree because this one *is* the clamp above: a ray that
    // goes through the box is clamped by nothing and measures zero.
    met.outside = length(vec3<f32>(miss.x, miss.y, miss.z / Z_PER_TILE));
    met.t = exit;
    return met;
}
