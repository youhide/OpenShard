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
    // How bright a solid rectangle is, or below zero for a quad that is a
    // picture and samples the atlas like every other one — see
    // `crate::gump::plate`. Flat because it is a property of the instance and
    // not of the corner: a plate is one colour, which is what makes it a plate.
    @location(2) @interpolate(flat) shade: f32,
};

// A region with no extent at all. No packed sprite can have one — the packer
// only ever hands out rectangles it put at least one texel into — so this is
// free to mean something else, and what it means is "there is no picture here":
// the quad is a solid rectangle and its `region.x` is the shade rather than a
// texture coordinate. See `crate::gump::plate` for the CPU half.
fn is_plate(region: vec4<f32>) -> bool {
    return region.z == 0.0 && region.w == 0.0;
}

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
    // A plate's `uv` above is its own corner four times over and is never read;
    // this is what the fragment shader reads instead.
    out.shade = select(-1.0, region.x, is_plate(region));
    return out;
}

// The bits of a wire hue that are the index into `hues.mul`, and the bit asking
// for a partial (grey-pixels-only) tint. `statics.wgsl`'s pair, and the same
// port: `openshard_uofiles::hues`.
const HUE_INDEX_MASK: u32 = 0x3FFFu;
const HUE_PARTIAL_FLAG: u32 = 0x8000u;

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // The solid rectangle, before the atlas is touched: there is nothing to
    // sample, nothing to discard on, and the hue — if there is one — is read at
    // the shade's own rung of the ramp, which is exactly what a grey texel of
    // that brightness would have picked below. A plate and a picture painted the
    // same grey therefore come out the same colour, which is what keeps this one
    // hue lookup and not two.
    if in.shade >= 0.0 {
        let index_bits = in.hue & HUE_INDEX_MASK;
        var rgb = vec3<f32>(in.shade);
        if index_bits != 0u {
            let index = i32(round(in.shade * 31.0));
            rgb = textureLoad(hue_ramp, vec2<i32>(index, i32(index_bits - 1u)), 0).rgb;
        }
        return vec4<f32>(rgb, 1.0);
    }

    let color = textureSample(atlas_texture, atlas_sampler, in.uv);

    // Gump art is a picture with a shape, and zero is transparent in it — see
    // `openshard_uofiles::gumpart`. Discarding rather than blending is what
    // lets a window's own transparent corners show the world through them
    // without this pass caring which order two windows were drawn in.
    if color.a < 0.5 {
        discard;
    }

    var rgb = color.rgb;
    // **The index bits and not the whole word.** `SpriteQuad::hue` carries more
    // than the wire hue: `with_opacity` writes an opacity into bits 16-23 and
    // `with_static_atlas_page` a page into 24-31, neither of which this pass
    // reads. Testing the word meant an untinted picture with an opacity — a
    // paperdoll's pending-equipment preview is one, `paperdoll::draw` — took this
    // branch with an index of zero, whose row is `-1`: an out-of-bounds
    // `textureLoad`, which WGSL answers with zeros, so the preview drew black.
    let index_bits = in.hue & HUE_INDEX_MASK;
    if index_bits != 0u {
        let partial = (in.hue & HUE_PARTIAL_FLAG) != 0u;
        if !partial || (color.r == color.g && color.g == color.b) {
            let index = i32(round(color.r * 31.0));
            rgb = textureLoad(hue_ramp, vec2<i32>(index, i32(index_bits - 1u)), 0).rgb;
        }
    }

    return vec4<f32>(rgb, 1.0);
}
