// The ring, from the mask the silhouette pass filled.
//
// Geometry is `blit.wgsl`'s: four corners from the vertex index, and the render
// pass's viewport is the rectangle the UI left free for the world. So this pass
// covers exactly the pixels the blit just wrote, and `uv` means the same thing
// in both — which is what lets a mask texel be found from a surface pixel with
// one multiplication.
//
// The rule is one line and it is the whole of "two rings do not merge":
//
//     a fragment is a ring texel of object N when some neighbour is N and the
//     fragment itself is not.
//
// With a coverage mask — 0 or 1 — the second half reads "and the fragment is
// empty", so where two highlighted sprites touch there is no ring between them
// and the pair comes out ringed as one blob. Carrying the *id* instead costs
// nothing and makes the boundary between two outlined objects a ring on both
// sides of itself, which is what a player looking at two doors of one shopfront
// is asking for.

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    var out: VertexOut;
    out.position = vec4<f32>(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0);
    out.uv = corner;
    return out;
}

@group(0) @binding(0) var mask: texture_2d<u32>;

struct Ring {
    // The mask's size in texels, then the ring's half-width in those texels,
    // then the glow's strength. The mask is the world image's size, so a texel
    // here is a *virtual* pixel — see `outline::Ring::width` for why the
    // thickness is measured in those and not in the surface's own.
    shape: vec4<f32>,
    color: vec4<f32>,
    // The glow's colour in `rgb`; `a` is unused and the strength lives in
    // `shape.w`, so a strength of zero is one comparison rather than a second
    // uniform buffer and a second pipeline.
    glow: vec4<f32>,
};

@group(0) @binding(1) var<uniform> ring: Ring;

// The spread silhouette `glow.wgsl` left behind, at half the mask's resolution
// and sampled linearly: the blur chain drew over the same `uv` rectangle, so
// this is the same place in the world image without any scaling here.
@group(0) @binding(2) var glow: texture_2d<f32>;
@group(0) @binding(3) var glow_sampler: sampler;

// What the mask holds where nothing was drawn.
const EMPTY: u32 = 0u;

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(ring.shape.xy);
    // Floor rather than round: `uv` runs 0..1 across the whole image, so this is
    // the same nearest-texel mapping the magnifying blit does, and the ring is
    // therefore blocky in step with the art rather than half a texel off it.
    let at = vec2<i32>(in.uv * ring.shape.xy);
    if at.x < 0 || at.y < 0 || at.x >= size.x || at.y >= size.y {
        discard;
    }
    let here = textureLoad(mask, at, 0).r;

    // Dense, not eight taps at a distance: a sparse neighbourhood at a radius
    // above one leaves gaps in the ring rather than thickening it. At the
    // default radius of one this *is* the eight-tap neighbourhood, plus the
    // centre — which costs nothing, since a texel is never its own neighbour by
    // the `there != here` test below.
    let step = i32(ring.shape.z);
    var found = EMPTY;
    for (var dy = -step; dy <= step; dy = dy + 1) {
        for (var dx = -step; dx <= step; dx = dx + 1) {
            let probe = at + vec2<i32>(dx, dy);
            if probe.x < 0 || probe.y < 0 || probe.x >= size.x || probe.y >= size.y {
                continue;
            }
            let there = textureLoad(mask, probe, 0).r;
            if there != EMPTY && there != here {
                found = there;
            }
        }
    }
    // The soft half, and it is *added* rather than blended over: light from two
    // sources pools, and a glow that replaced what was under it would be a fog
    // rather than a lamp. The target's blend is premultiplied alpha, so an alpha
    // of zero with a colour in `rgb` is exactly addition — one pass draws both
    // halves, which is what keeps step 3 of `docs/outline.md` D5 unchanged now
    // that step 2 has grown a blur.
    //
    // Nothing inside the silhouette: the glow's job is to say where the thing
    // is, not to repaint it, and an additive wash over the art washes out
    // exactly the picture the player is being pointed at.
    var lit = vec3<f32>(0.0);
    if ring.shape.w > 0.0 && here == EMPTY {
        let spread = textureSample(glow, glow_sampler, in.uv).r;
        lit = ring.glow.rgb * spread * ring.shape.w;
    }

    if found == EMPTY {
        // No ring here, so the fragment is glow or it is nothing. `discard`
        // rather than a black premultiplied zero because the two are the same
        // to the blender and not to a reader.
        if lit.r + lit.g + lit.b <= 0.0 {
            discard;
        }
        return vec4<f32>(lit, 0.0);
    }
    return vec4<f32>(ring.color.rgb * ring.color.a + lit, ring.color.a);
}
