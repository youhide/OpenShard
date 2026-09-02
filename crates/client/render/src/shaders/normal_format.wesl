// Which way a fragment's surface looks, in one `u32` — the packing of the
// G-buffer's normal plane, `crate::gbuffer::NORMAL_FORMAT`.
//
// Every world pass (`ground.wgsl`, `statics.wgsl`, `mesh_face.wgsl`) writes one
// of these and `blit.wgsl` reads it, exactly as `place_format.wesl` is written
// by all three and read by two. `crate::gbuffer::pack_normal` and
// `unpack_normal` are the Rust twins, for the two things on that side that
// build a G-buffer without drawing a frame.
//
// # Why a word and not three floats
//
// The plane was `Rgba32Float` — sixteen bytes a fragment for a unit vector —
// and twelve of those were the whole of what stood between this client and
// WebGPU's guaranteed `maxColorAttachmentBytesPerSample` of 32. See
// `crate::gbuffer::required_limits`, which no longer asks for anything above
// the floor. `docs/render/design_model.md`'s own backlog is where this was named,
// and its phase 6 is why it had to happen first: the target layout there is
// position, normal, albedo and ids at exactly 32, and the normal is the only
// term of that sum that was not already the size it has to be.
//
// # The mapping
//
// Octahedral, the ordinary kind: project the unit sphere onto the octahedron
// `|x| + |y| + |z| = 1`, then unfold its lower half outwards into the square
// `[-1, 1]²`. Two numbers instead of three, with no pole and no seam that a
// direction in this renderer ever crosses — the alternative, storing `x` and
// `y` and recovering `z` from them, cannot state a downward-facing surface at
// all.
//
// **The span is even on purpose.** Each axis is quantised to `NORMAL_AXIS_SPAN`
// steps rather than to the `32767` its fifteen bits would allow, so that zero
// lands exactly on a code (`16383`) instead of half a step off one. Nearly
// every normal this renderer writes is cardinal — every wall face, every lid,
// every level tile — and an odd span would have moved all of them by a
// ten-thousandth and left nothing round-tripping exactly.
//
// # The two bits above them
//
// Thirty bits of direction and two of what kind of answer this is, because
// there are three:
//
// - `NORMAL_DRAWN` clear — **nothing drew this pixel**. The clear value is
//   zero, so an untouched texel is this and no producer can spell it.
// - `NORMAL_DRAWN` set, `NORMAL_FACING` clear — **something drew it and it has
//   no side**: a billboard, a tree, a wall whose art named no edge. Every flame
//   that reaches such a fragment lights it, and `blit.wgsl` tests exactly this
//   bit for that.
// - both set — a direction, in the thirty bits below.
//
// The pair is kept in this plane rather than inferred from the id word beside
// it (a `KIND_NOTHING` pixel is the first, a `STANCE_UPRIGHT` one the second)
// so that the plane still means something read on its own — which is how
// `VIEW_NORMAL` and every test that copies it back read it.

// Fifteen bits an axis, and the mask that takes one back out.
const NORMAL_AXIS_BITS: u32 = 15u;
const NORMAL_AXIS_MASK: u32 = 32767u;

// The number of steps an axis is quantised into — even, see above, and one
// short of what the mask allows. `crate::gbuffer::NORMAL_AXIS_SPAN`.
const NORMAL_AXIS_SPAN: f32 = 32766.0;

// A fragment was drawn here at all, and it has a facing. See the file's own
// header for the three states these two spell.
const NORMAL_FACING: u32 = 0x40000000u;
const NORMAL_DRAWN: u32 = 0x80000000u;

// Which side of zero a number is on, counting zero as the positive side.
//
// **Not `sign`**, which answers `0` at zero — the octahedron's fold multiplies
// by this, and a zero there collapses a whole edge of the square onto the
// origin. Written the same way on the Rust side, where `f32::signum` has the
// opposite problem: it answers `1` for `-0.0`'s neighbourhood but is a
// different function again, and two spellings of a fold is how the halves of
// this file stop agreeing.
fn axis_sign(value: f32) -> f32 {
    return select(-1.0, 1.0, value >= 0.0);
}

// One word from one direction. A zero vector is "no facing" and is the one
// input that is not a direction — see the header's three states.
fn pack_normal(normal: vec3<f32>) -> u32 {
    let sum = abs(normal.x) + abs(normal.y) + abs(normal.z);
    if sum == 0.0 {
        return NORMAL_DRAWN;
    }
    // Onto the octahedron, which is what makes the two components below add up
    // to a known total and therefore recoverable from each other.
    let on_octahedron = normal / sum;
    var oct = on_octahedron.xy;
    if on_octahedron.z < 0.0 {
        // The lower half, folded outwards into the corners of the square.
        oct = vec2<f32>(
            (1.0 - abs(on_octahedron.y)) * axis_sign(on_octahedron.x),
            (1.0 - abs(on_octahedron.x)) * axis_sign(on_octahedron.y),
        );
    }
    // Clamped before the scale rather than after: the fold above is exact in
    // principle and a rounding error of one bit at a corner would otherwise
    // produce a code past the mask, which wraps rather than saturates.
    let steps = round((clamp(oct, vec2<f32>(-1.0), vec2<f32>(1.0)) * 0.5 + vec2<f32>(0.5)) * NORMAL_AXIS_SPAN);
    return NORMAL_DRAWN
        | NORMAL_FACING
        | (u32(steps.x) & NORMAL_AXIS_MASK)
        | ((u32(steps.y) & NORMAL_AXIS_MASK) << NORMAL_AXIS_BITS);
}

// And back: the unit vector a word names, or a zero vector for a fragment with
// no facing. A word nothing drew answers zero as well — `blit.wgsl` has already
// passed such a pixel through untouched by the time it would ask.
fn unpack_normal(word: u32) -> vec3<f32> {
    if (word & NORMAL_FACING) == 0u {
        return vec3<f32>(0.0);
    }
    let steps = vec2<f32>(
        f32(word & NORMAL_AXIS_MASK),
        f32((word >> NORMAL_AXIS_BITS) & NORMAL_AXIS_MASK),
    );
    let oct = steps / NORMAL_AXIS_SPAN * 2.0 - vec2<f32>(1.0);
    // The third component is what the octahedron's own equation leaves: it is
    // negative exactly on the folded half, which is how the fold is undone
    // without a bit saying it happened.
    var normal = vec3<f32>(oct, 1.0 - abs(oct.x) - abs(oct.y));
    if normal.z < 0.0 {
        normal = vec3<f32>(
            (1.0 - abs(normal.y)) * axis_sign(normal.x),
            (1.0 - abs(normal.x)) * axis_sign(normal.y),
            normal.z,
        );
    }
    return normalize(normal);
}
