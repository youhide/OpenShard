// The soft half of the highlight: the silhouette, spread out.
//
// Two entry points and one geometry — `outline.wgsl`'s four corners from the
// vertex index, drawn over the whole of whatever target is bound, so `uv` means
// "the same place in the world image" in every pass here and in the composite
// that reads the result. That is the only thing keeping the glow registered with
// the picture it is coming off: nothing below knows where the sprite was, only
// that a texel of the mask was or was not part of one.
//
// `seed` takes the `R8Uint` id mask and answers coverage — was anything drawn
// here — at half the mask's resolution. The ids are dropped deliberately: two
// outlined objects need separate *rings* (an id boundary is a ring on both
// sides of itself, see `outline.wgsl`), but two glows that meet should pool
// rather than cut each other out, which is what light does.
//
// `blur` is one Kawase iteration: four bilinear taps on the diagonals at a
// growing offset, ping-ponged between two targets. Three of them at half
// resolution is twelve taps a texel for a spread a Gaussian would want dozens
// for — the trick being that each tap is already the average of four texels, so
// the kernel widens faster than the tap count does.

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

/// What the mask holds where nothing was drawn.
const EMPTY: u32 = 0u;

@fragment
fn seed(in: VertexOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(mask));
    // The bilinear footprint of this texel's centre in the mask: the four texels
    // around it. Half a texel back and then floor, which is what a linear
    // sampler would have taken had the mask been sampleable — it is not, being
    // `Uint`, so the footprint is walked by hand.
    let centre = in.uv * vec2<f32>(size) - 0.5;
    let base = vec2<i32>(floor(centre));
    var found = 0.0;
    for (var dy = 0; dy <= 1; dy = dy + 1) {
        for (var dx = 0; dx <= 1; dx = dx + 1) {
            let probe = base + vec2<i32>(dx, dy);
            if probe.x < 0 || probe.y < 0 || probe.x >= size.x || probe.y >= size.y {
                continue;
            }
            if textureLoad(mask, probe, 0).r != EMPTY {
                found = 1.0;
            }
        }
    }
    // Any of the four, not their average: this is a coverage mask being widened,
    // and the blur below is what turns it into a falloff. Averaging here would
    // put a half-lit fringe on the seed that the first iteration then spreads,
    // which reads as a glow that starts a texel inside the sprite.
    return vec4<f32>(found, found, found, found);
}

// Bindings 1..3 and not 0..2, because both entry points live in one module and
// a binding number may only mean one thing in it. The two pipelines have layouts
// of their own, so the gap costs nothing.
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;

struct Step {
    // The iteration's offset, in the source's own texels. Growing across the
    // chain: a constant offset repeated is a box filter with the same reach
    // every time, and it is the *growth* that makes the falloff smooth.
    offset: vec4<f32>,
};

@group(0) @binding(3) var<uniform> step: Step;

@fragment
fn blur(in: VertexOut) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(source));
    let at = step.offset.x * texel;
    var sum = textureSample(source, source_sampler, in.uv + vec2<f32>(-at.x, -at.y));
    sum = sum + textureSample(source, source_sampler, in.uv + vec2<f32>(at.x, -at.y));
    sum = sum + textureSample(source, source_sampler, in.uv + vec2<f32>(-at.x, at.y));
    sum = sum + textureSample(source, source_sampler, in.uv + vec2<f32>(at.x, at.y));
    return sum * 0.25;
}
