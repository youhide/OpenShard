// The occlusion grid's solids, drawn over a finished picture.
//
// `docs/lighting.md` step 23.0 and decision 39. There is deliberately **no
// projection in this file**: the corners arrive already in viewport pixels,
// because the arithmetic that puts them there is `Solid::faces` in Rust, where
// it is tested against `Camera::tile_diamond` and where the corner-versus-centre
// lattice of decision 39.7 was found. A vertex shader that projected a box
// itself would be a second chance to disagree with the projection every sprite
// in the frame is placed by — `statics.wgsl` says the same thing about depth,
// and for the same reason.
//
// So all this does is what no CPU can do for it: turn a pixel of the target into
// clip space, and blend.

struct Screen {
    // The target's size in real pixels, and the rest is padding: a uniform block
    // is a multiple of 16 bytes.
    size: vec4<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) tint: vec4<f32>,
};

@vertex
fn vs_main(@location(0) at: vec2<f32>, @location(1) tint: vec4<f32>) -> VertexOut {
    var out: VertexOut;
    // Pixels to clip space: `y` flips, because a target's pixels count down from
    // the top and clip space counts up from the middle.
    out.position = vec4<f32>(
        at.x / screen.size.x * 2.0 - 1.0,
        1.0 - at.y / screen.size.y * 2.0,
        // Zero, and no depth attachment: this pass draws over a picture that has
        // already decided what covers what. A solid spanning several tiles has
        // one instance depth and several tile depths underneath it, which is
        // decision 39.2 — the answer taken here is the first of its two, and it
        // is translucency rather than a depth this pass would have to invent.
        0.0,
        1.0,
    );
    out.tint = tint;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Premultiplied, like the ring pass: the target's blend is
    // `dst = src.rgb + dst * (1 - src.a)`, so a colour scaled by its own alpha is
    // the ordinary blend and the art shows through by whatever is left. That
    // show-through is the whole point of the view — a solid that hid the sprite
    // it claims to contain would report nothing about it.
    return vec4<f32>(in.tint.rgb * in.tint.a, in.tint.a);
}
