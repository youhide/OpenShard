// Solid tile-sized quads over ready terrain: a player, and later a waypoint.
// No texture and no atlas — a marker is a colour and a rectangle, and both
// change every frame, which is the whole reason it is an overlay rather than a
// cached product.
struct RadarFrame {
    screen: vec2<f32>,
    scale: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> frame: RadarFrame;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    // Flat: one rectangle is one colour, so interpolating between corners could
    // only produce a colour the marker was never asked to be.
    @location(0) @interpolate(flat) color: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) origin: vec2<f32>,
    @location(2) extent: vec2<f32>,
    @location(3) color: vec4<f32>,
) -> VertexOut {
    let pixel = (origin + corner * extent) * frame.scale;
    var out: VertexOut;
    out.clip = vec4<f32>(pixel.x / frame.screen.x * 2.0 - 1.0, 1.0 - pixel.y / frame.screen.y * 2.0, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return in.color;
}
