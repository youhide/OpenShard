// One instance per resident chunk rectangle. What the frame is — its size and
// its scale — is the same for every one of them and lives in the uniform; where
// a chunk goes, what part of its page it shows and which page that is differ per
// instance, so they travel in the instance buffer. A uniform rewritten between
// draws would not: `Queue::write_buffer` lands before the whole command buffer,
// so every draw in a submission would read the last values written.
struct RadarFrame {
    screen: vec2<f32>,
    scale: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> frame: RadarFrame;
@group(0) @binding(1) var pages: texture_2d_array<f32>;
@group(0) @binding(2) var page_sampler: sampler;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Flat: a layer index is a name, not a quantity, so interpolating it across
    // the quad would sample a different page along the way.
    @location(1) @interpolate(flat) layer: u32,
};

@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) origin: vec2<f32>,
    @location(2) extent: vec2<f32>,
    @location(3) uv_origin: vec2<f32>,
    @location(4) uv_extent: vec2<f32>,
    @location(5) layer: u32,
) -> VertexOut {
    let pixel = (origin + corner * extent) * frame.scale;
    var out: VertexOut;
    out.clip = vec4<f32>(pixel.x / frame.screen.x * 2.0 - 1.0, 1.0 - pixel.y / frame.screen.y * 2.0, 0.0, 1.0);
    out.uv = uv_origin + corner * uv_extent;
    out.layer = layer;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(pages, page_sampler, in.uv, i32(in.layer)).rgb, 1.0);
}
