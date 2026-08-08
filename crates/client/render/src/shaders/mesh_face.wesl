// `docs/gbuffer.md` step 4c's render pass: one raw, non-instanced vertex per
// corner of a `crate::mesh::Face`, fan-triangulated on the CPU
// (`Face::fan`). No colour target — the enclosing static's own billboard
// sprite already drew the picture (`statics.wgsl`), and this pass exists
// only to give that same static's pixels a more honest per-face normal in
// `place` than one blended stance (a former `facing::Prism::tread_normal`,
// retired by step 5 once this pass made it unnecessary) could.
//
// Depth is not derived here either, for the same reason `statics.wgsl` says
// so of its own: it is the enclosing static's own `SpriteQuad::depth`,
// carried through unchanged, so this pass's depth test can only tie or
// improve on what the billboard sprite already wrote — never disagree with
// it (`docs/gbuffer.md` decision 4).

import package::place_format::{KIND_STATIC, STANCE_MESH_FACE, pack_place};

struct Viewport {
    // Real pixels per virtual pixel — see `ground.wgsl`, which documents the
    // same three fields at length; every world pass reads them identically.
    size: vec2<f32>,
    scale: f32,
    _padding: f32,
    // The virtual-pixel point that lands in the middle of the target.
    origin: vec2<f32>,
    // Uniform blocks are sized in multiples of 16 bytes.
    _tail: vec2<f32>,
};

@group(0) @binding(0) var<uniform> viewport: Viewport;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    // This vertex's true world position, carried rather than recomputed:
    // linear interpolation across the triangle gives every fragment its own
    // exact world position for free, because the projection is affine and
    // every face is planar — see `crate::mesh_face::MeshFaceVertex::world`.
    @location(0) world: vec3<f32>,
    // This face's row in `blit.wgsl`'s `mesh_instances` — flat, identical for
    // all six vertices one face's fan produces.
    @location(1) @interpolate(flat) id: u32,
    // The tile this face stands on — `crate::mesh_face::MeshFaceRow::tile`
    // again, flat like `id`: every corner of one face agrees on it, and the
    // fragment stage needs it to place a fragment inside its own tile without
    // flooring a position that can legitimately sit on the edge shared with
    // the next one. See `fs_main`'s own comment.
    @location(2) @interpolate(flat) tile: vec2<f32>,
};

@vertex
fn vs_main(
    // This corner in `Camera::to_view_exact`'s space — the same
    // pre-viewport-zoom space `statics.wgsl`'s own `origin` is in.
    @location(0) screen: vec2<f32>,
    @location(1) world: vec3<f32>,
    // Where this face sorts: the enclosing static's own depth, reused whole.
    @location(2) depth: f32,
    @location(3) id: u32,
    @location(4) tile: vec2<f32>,
) -> VertexOut {
    // Virtual pixels to real ones, the same single line `statics.wgsl`'s own
    // vertex stage ends on.
    let real = (screen - viewport.origin) * viewport.scale + viewport.size * 0.5;
    let ndc = vec2<f32>(
        real.x / viewport.size.x * 2.0 - 1.0,
        1.0 - real.y / viewport.size.y * 2.0,
    );

    var out: VertexOut;
    out.clip = vec4<f32>(ndc, depth, 1.0);
    out.world = world;
    out.id = id;
    out.tile = tile;
    return out;
}

// A hundred-and-twenty-seventh of a tile — `crate::place`'s own fraction
// unit, the same one `statics.wgsl` packs into the attachment's low bits.
const INSIDE: f32 = 126.0 / 127.0;

struct FragmentOut {
    @location(0) place: vec4<u32>,
};

@fragment
fn fs_main(in: VertexOut) -> FragmentOut {
    // Where in its tile this fragment is, and how high — both exact, both
    // free: `in.world` already interpolated to this fragment's own true
    // position, so there is no per-stance formula to invert a second time.
    //
    // Subtracted from the face's own known tile, not `fract()`: a face's own
    // edge can sit exactly on a whole number — this tread's outer corner does
    // — and `fract` of a whole number is `0.0`, not `1.0`, so it always
    // resolved that edge to the *next* tile rather than the one the face
    // actually stands on. `docs/lighting.md`'s shadow-raymarch anomaly: one
    // fragment on an otherwise evenly-shadowed flat face reading fully lit,
    // because the walk it fed started from the wrong tile's occluders.
    let sub = clamp(in.world.xy - in.tile, vec2<f32>(0.0), vec2<f32>(INSIDE));

    var out: FragmentOut;
    out.place = pack_place(in.id, in.world.z, STANCE_MESH_FACE, KIND_STATIC, sub);
    return out;
}
