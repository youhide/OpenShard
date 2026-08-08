// The `place` attachment's own bit format — what `crate::place::Place::packed`
// writes on the CPU side and what every world pass (`ground.wgsl`,
// `statics.wgsl`, `mesh_face.wgsl`) and every reader (`blit.wgsl`,
// `select.wgsl`) must agree on to draw one shadow correctly.
//
// One `pub const` block instead of five hand-copied ones. Before this file,
// each of those five shader modules carried its own copy of these numbers —
// unavoidable under plain WGSL, since each pass's shader module is its own
// compilation unit and WGSL has no `#include`. A producer that forgot to
// stamp a bit this module's readers branch on decoded as a meaningful-but-
// wrong default instead of failing to compile: `ground.wgsl` never stamping
// `STANCE_FLAT` (session 23, `docs/lighting_raymarch.md`) is what that gap
// looks like when it fires, not a one-off. `crate::place::STANCE_SHIFT`
// states the same numbers once more on the Rust side; nothing but a person
// reading both can check the two still agree — see its own doc comment.
//
// Migrated one pass at a time — `ground.wgsl` first (this module exists
// because of its bug), the others tracked as backlog in
// `docs/lighting_raymarch.md`. A shader not yet migrated still carries its
// own copy of whatever subset of these it needs, unchanged.

// The fourth channel's low two bits: `crate::place::Kind`.
const KIND_MASK: u32 = 3u;
const KIND_NOTHING: u32 = 0u;
const KIND_LAND: u32 = 1u;
const KIND_STATIC: u32 = 2u;
const KIND_MOBILE: u32 = 3u;

// The fourth channel's upper bits, above the kind: a hundred-and-twenty-
// seventh of a tile in each of the two tile-local axes.
const SUB_TILE_MASK: u32 = 127u;
const SUB_TILE: f32 = 127.0;

// How the third channel is divided: `z`'s whole units, offset by 128, in the
// low eight bits of a `u16`; **sixteenths of a unit** in the four above them;
// the stance in the four above those. `crate::place::STANCE_SHIFT` and its
// neighbours are the same numbers, stated once more in Rust.
//
// The fraction is `docs/lighting_height.md` phase 1, and the four bits it
// takes were spare here before it. Until then this channel held `round(raw_z)`
// alone, which is exact on a floor or a lid — a lid *is* at an integer `z` —
// and a lie on anything standing up: height varies continuously down a
// vertical face, and rounding it to the nearest unit turns one surface into a
// staircase of one-unit treads, each lit as though it were a whole unit higher
// or lower than it really is. Sixteenths because a `z` unit is four screen
// pixels at zoom 1, so one step is a quarter of a pixel — under anything a
// frame can show. Eight bits of fraction would have meant moving the stance
// out of the channel entirely, which is real work for precision nothing can
// see; see the plan doc's "Why four bits and not eight".
const PLACE_STANCE_SHIFT: u32 = 12u;
const PLACE_STANCE_MASK: u32 = 15u;
const PLACE_Z_FRAC_SHIFT: u32 = 8u;
const PLACE_Z_FRAC_MASK: u32 = 15u;
const PLACE_Z_MASK: u32 = 255u;

// Steps of that fraction in one `z` unit — `PLACE_Z_FRAC_MASK + 1`, named
// apart from it because every use is arithmetic on an `f32` rather than a mask.
const PLACE_Z_FRAC_STEPS: f32 = 16.0;

// The height a packed third channel holds, whole units and fraction together.
//
// The one place the two fields are put back into a number, so that a reader
// cannot take the whole units and quietly drop the rest — which is exactly
// what every reader did before the fraction existed, and would keep doing
// without a fault, one sixteenth of a unit at a time.
fn unpack_place_z(channel: u32) -> f32 {
    return f32(channel & PLACE_Z_MASK) - 128.0
        + f32((channel >> PLACE_Z_FRAC_SHIFT) & PLACE_Z_FRAC_MASK) / PLACE_Z_FRAC_STEPS;
}

// `crate::place::Stance`, in the four bits above.
const STANCE_UPRIGHT: u32 = 0u;
const STANCE_FLAT: u32 = 1u;
const STANCE_FACE_NORTH: u32 = 2u;
const STANCE_FACE_EAST: u32 = 3u;
const STANCE_FACE_SOUTH: u32 = 4u;
const STANCE_FACE_WEST: u32 = 5u;
// The first of the four corner stances — see `crate::place::Stance::CornerNorthSouth`
// and `STANCE_CORNER` in Rust for the arithmetic that resolves one to a face.
const STANCE_CORNER: u32 = 6u;
// `crate::place::Stance::MeshFace` — not a real stance, a routing sentinel
// the mesh-face pass writes instead of one of the five above.
const STANCE_MESH_FACE: u32 = 10u;

// What `ground.wgsl`/`statics.wgsl`/`mesh_face.wgsl` each built by hand, one
// `vec4<u32>` literal per file, before this: `id` split across the first two
// channels, `z` and `stance` folded into the third, `kind` and the tile-local
// fraction into the fourth. Sharing this closes half of session 23's bug
// class rather than the whole of it — see `docs/lighting_raymarch.md`'s own
// note on the distinction — but it closes the more dangerous half: `stance`
// is a required parameter here, so a producer literally cannot build a
// `place` value without deciding on one. Getting the *value* wrong (passing
// `STANCE_UPRIGHT` where `STANCE_FLAT` was meant) is still possible and still
// a bug, but it is now a line a person reading the call site can see and
// question — not a bit that simply never got OR'd in, invisible until a
// scene happens to tell the two apart.
//
// `raw_z` and `sub` are each pre-clamp: every call site before this clamped
// `z` to `-128.0..127.0` and `sub` to its own tile (statics/mesh_face to
// `0.0..INSIDE`, ground to the full `0.0..1.0`) before folding it in, and
// this repeats both clamps rather than trusting the caller already did —
// harmless where a caller's own range is already inside `0.0..1.0`, and a
// second gate on the one thing every one of the three used to get separately
// right.
//
// **The height is quantised once, in sixteenths, and only then split** into
// the two fields — not clamped to whole units and rounded a second time for
// the remainder. Rounding twice is not the same arithmetic, and it has a
// failure the single rounding cannot have: a remainder that rounds up to a
// full unit carries into the field above, which here is not a higher digit of
// anything but the *stance*. A wall whose fraction happened to land near one
// would have reported itself a floor.
fn pack_place(id: u32, raw_z: f32, stance: u32, kind: u32, sub: vec2<f32>) -> vec4<u32> {
    // The same range the clamp always covered, stated in the units the field
    // now counts in: `-128 .. 127 + 15/16` is everything the two fields can
    // say together, and a height outside it is pinned to the nearer end
    // exactly as it was before.
    let steps = i32(clamp(
        round(raw_z * PLACE_Z_FRAC_STEPS),
        -128.0 * PLACE_Z_FRAC_STEPS,
        128.0 * PLACE_Z_FRAC_STEPS - 1.0,
    ));
    // An **arithmetic** shift, which is what makes this a `floor` and not a
    // truncation: a height of -3.5 is -4 whole units and eight sixteenths, the
    // way `unpack_place_z` adds them back, rather than -3 and minus eight. The
    // mask below is the same statement for the remainder — always positive,
    // always under one unit.
    let z = u32((steps >> 4u) + 128)
        | (u32(steps & i32(PLACE_Z_FRAC_MASK)) << PLACE_Z_FRAC_SHIFT)
        | (stance << PLACE_STANCE_SHIFT);
    let clamped = clamp(sub, vec2<f32>(0.0), vec2<f32>(1.0));
    let packed_sub = (u32(round(clamped.x * SUB_TILE)) << 2u) | (u32(round(clamped.y * SUB_TILE)) << 9u);
    return vec4<u32>(id & 0xFFFFu, id >> 16u, z, kind | packed_sub);
}
