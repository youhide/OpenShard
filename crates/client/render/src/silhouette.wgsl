// The silhouette pass: highlighted sprites, as shapes rather than pictures.
//
// The vertex half is `statics.wgsl`'s, line for line, and deliberately so — a
// silhouette that landed one pixel from the sprite it belongs to would ring the
// wrong outline, and the only defence against that is the same arithmetic. What
// differs is the fragment half: instead of the art's colour it writes *which
// object* the texel belongs to, into an `R8Uint` mask.
//
// The id arrives per instance, in a buffer of its own, and zero is left free
// for "nothing here". Per instance and not `instance_index`, because a ring is
// not a sprite: a creature is a body plus every layer it wears, and those must
// share one id or the ring pass finds a boundary between the tunic and the arm
// inside it and draws an edge along every seam. The caller hands this pass only
// what is to be outlined, grouped — see `SpriteRenderer::render_mask`. 255 rings
// at once, against the one the cursor is over today.
//
// The depth buffer is the world's, loaded and tested but not written: the mask
// must hold the id of whoever is *visible*, or a barrel behind a wall would be
// ringed through the wall.

struct Viewport {
    size: vec2<f32>,
    scale: f32,
    _padding: f32,
    origin: vec2<f32>,
    _tail: vec2<f32>,
};

@group(0) @binding(0) var<uniform> viewport: Viewport;
@group(0) @binding(1) var atlas_texture: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) id: u32,
};

@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) origin: vec2<f32>,
    @location(2) size: vec2<f32>,
    @location(3) region: vec4<f32>,
    @location(4) depth: f32,
    @location(5) ring: u32,
) -> VertexOut {
    let pixel = origin + corner * size;
    let real = (pixel - viewport.origin) * viewport.scale + viewport.size * 0.5;
    let ndc = vec2<f32>(
        real.x / viewport.size.x * 2.0 - 1.0,
        1.0 - real.y / viewport.size.y * 2.0,
    );

    var out: VertexOut;
    out.clip = vec4<f32>(ndc, depth, 1.0);
    out.uv = region.xy + corner * region.zw;
    // Given rather than counted: `render_mask` numbers the groups, and zero is
    // reserved for "nothing here".
    out.id = ring;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) u32 {
    // The same alpha cut the picture pass makes, and it has to be the same
    // number: the mask is the shape of what was drawn, so a silhouette that
    // kept a texel the sprite discards would put a ring round a fringe nobody
    // can see.
    let color = textureSample(atlas_texture, atlas_sampler, in.uv);
    if color.a < 0.5 {
        discard;
    }
    return in.id;
}
