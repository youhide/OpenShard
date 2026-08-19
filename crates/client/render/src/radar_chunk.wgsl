struct RadarChunk {
    screen: vec2<f32>,
    origin: vec2<f32>,
    extent: vec2<f32>,
    scale: f32,
    layer: f32,
    uv_origin: vec2<f32>,
    uv_extent: vec2<f32>,
};

@group(0) @binding(0) var<uniform> radar: RadarChunk;
@group(0) @binding(1) var pages: texture_2d_array<f32>;
@group(0) @binding(2) var page_sampler: sampler;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) corner: vec2<f32>) -> VertexOut {
    let pixel = (radar.origin + corner * radar.extent) * radar.scale;
    var out: VertexOut;
    out.clip = vec4<f32>(pixel.x / radar.screen.x * 2.0 - 1.0, 1.0 - pixel.y / radar.screen.y * 2.0, 0.0, 1.0);
    out.uv = radar.uv_origin + corner * radar.uv_extent;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(pages, page_sampler, in.uv, i32(radar.layer)).rgb, 1.0);
}
