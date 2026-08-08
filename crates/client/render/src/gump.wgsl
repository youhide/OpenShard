// The gump pass: the interface, in its own pixels, over a finished frame.
//
// The same instance layout as `statics.wgsl` — one `crate::sprite::SpriteQuad`
// — and deliberately none of its arithmetic. Three things are gone, and each is
// gone for a reason a gump makes obvious:
//
// - **No projection.** A gump does not live in the world, so there is no
//   camera, no origin and no zoom to apply. What there is instead is one `ui`
//   scale, which turns a gump pixel into however many real pixels this display
//   wants it to be — see `crate::gump::Frame::scale`.
// - **No depth.** This runs after the blit with no depth attachment, and what
//   covers what is the order the quads arrive in. A window over a window is
//   painter's order, which is what every interface has always been.
// - **No place attachment.** A gump is not standing on a tile, so there is
//   nothing for the lighting pass to read and nothing for it to dim: an
//   interface that went dark at night would be unreadable exactly when the
//   picture already is.
//
// What it keeps is the hue lookup, byte for byte: `hues.mul` tints a gump the
// same way it tints a wall, and two copies of that would be two chances to
// disagree about a ramp.

struct Screen {
    // The target's size in real pixels.
    size: vec2<f32>,
    // Real pixels per gump pixel. `1.0` is the reference client's own scale,
    // where its art is one for one with the display and its windows are
    // postage stamps on anything modern.
    scale: f32,
    // Uniform blocks are sized in multiples of 16 bytes.
    _padding: f32,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(0) @binding(1) var atlas_texture: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;
// `hues.mul`'s ramps, exactly as the world passes bind them — see `crate::hue`.
@group(0) @binding(3) var hue_ramp: texture_2d<f32>;
@group(0) @binding(4) var hue_sampler: sampler;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) hue: u32,
};

@vertex
fn vs_main(
    // The unit quad: (0,0) top-left to (1,1) bottom-right.
    @location(0) corner: vec2<f32>,
    // Per instance: the picture's top-left corner, in gump pixels.
    @location(1) origin: vec2<f32>,
    // Per instance: its size in gump pixels.
    @location(2) size: vec2<f32>,
    // Per instance: where it sits in the atlas, as (u, v, du, dv).
    @location(3) region: vec4<f32>,
    // Per instance: the wire hue, or 0 for none.
    @location(4) hue: u32,
) -> VertexOut {
    let pixel = (origin + corner * size) * screen.scale;

    // Pixels to clip space; `y` flips because the interface counts down from
    // the top of the window and clip space counts up from the middle.
    let ndc = vec2<f32>(
        pixel.x / screen.size.x * 2.0 - 1.0,
        1.0 - pixel.y / screen.size.y * 2.0,
    );

    var out: VertexOut;
    // Depth zero on every quad: this pass has no depth attachment and no test,
    // so the value only has to be inside the clip volume.
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = region.xy + corner * region.zw;
    out.hue = hue;
    return out;
}

// The bits of a wire hue that are the index into `hues.mul`, and the bit asking
// for a partial (grey-pixels-only) tint. `statics.wgsl`'s pair, and the same
// port: `openshard_uofiles::hues`.
const HUE_INDEX_MASK: u32 = 0x3FFFu;
const HUE_PARTIAL_FLAG: u32 = 0x8000u;

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let color = textureSample(atlas_texture, atlas_sampler, in.uv);

    // Gump art is a picture with a shape, and zero is transparent in it — see
    // `openshard_uofiles::gumpart`. Discarding rather than blending is what
    // lets a window's own transparent corners show the world through them
    // without this pass caring which order two windows were drawn in.
    if color.a < 0.5 {
        discard;
    }

    var rgb = color.rgb;
    if in.hue != 0u {
        let partial = (in.hue & HUE_PARTIAL_FLAG) != 0u;
        if !partial || (color.r == color.g && color.g == color.b) {
            let index = i32(round(color.r * 31.0));
            let row = i32((in.hue & HUE_INDEX_MASK) - 1u);
            rgb = textureLoad(hue_ramp, vec2<i32>(index, row), 0).rgb;
        }
    }

    return vec4<f32>(rgb, 1.0);
}
