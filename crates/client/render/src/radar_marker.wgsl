// Solid tile-sized quads over ready terrain: a player, and later a waypoint.
// No texture and no atlas — a marker is a colour and a rectangle, and both
// change every frame, which is the whole reason it is an overlay rather than a
// cached product.
struct RadarFrame {
    screen: vec2<f32>,
    scale: f32,
    _pad: f32,
    clip_center: vec2<f32>,
    clip_radius: f32,
    circle: f32,
    map_origin: vec2<f32>,
    map_extent: vec2<f32>,
    rotation: f32,
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
    let source_pixel = origin + corner * extent;
    let local = source_pixel - frame.map_origin - frame.map_extent / 2.0;
    let sin_rotation = sin(frame.rotation);
    let cos_rotation = cos(frame.rotation);
    let pixel = (frame.map_origin + frame.map_extent / 2.0 + vec2<f32>(
        cos_rotation * local.x - sin_rotation * local.y,
        sin_rotation * local.x + cos_rotation * local.y,
    )) * frame.scale;
    var out: VertexOut;
    out.clip = vec4<f32>(pixel.x / frame.screen.x * 2.0 - 1.0, 1.0 - pixel.y / frame.screen.y * 2.0, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    if (frame.circle != 0.0 && distance(in.clip.xy, frame.clip_center) > frame.clip_radius) {
        discard;
    }
    return in.color;
}
