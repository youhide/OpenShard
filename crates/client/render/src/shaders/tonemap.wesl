// The colour pipeline: sRGB in, linear throughout, tonemapped out.
//
// `docs/lighting_rebuild.md` phase 1, and it is the phase everything else waits
// on rather than an improvement on its own. Light is a *quantity*: two equal
// flames on one spot are twice one flame, and a surface at half the distance is
// four times as bright. None of that is true of the numbers stored in an image
// file — a PNG's `128` is not half of its `255`, it is about a fifth — so a
// renderer that multiplies stored bytes by a light value is computing an
// arithmetic nobody can check. It cannot agree with a path tracer, with a
// photograph, or with a second implementation of itself, because there is
// nothing it is trying to be right *about*.
//
// So: decode the art out of sRGB before it is multiplied by anything, do every
// sum in linear radiance, and encode once at the end.
//
// `crate::tonemap` states all four of these on the Rust side and a test asserts
// the two agree to within the eight bits a frame is written in. Nothing else in
// this crate may spell these curves a second time.

// How much light one unit of scene radiance is worth on screen, before the
// curve. One: the flame intensities in `light.rs` were tuned against the old
// clamped pipeline and re-tuning them is not this phase's business. It is a
// separate knob from the curve on purpose — exposure moves the whole picture,
// the curve decides what happens to the part of it that no longer fits.
const EXPOSURE: f32 = 1.0;

// sRGB's own transfer function, decoded. The linear segment below `0.04045` is
// not a nicety: `pow` near zero is where the two implementations of this drift,
// and the standard says the curve is a line there.
fn srgb_to_linear(color: vec3<f32>) -> vec3<f32> {
    let low = color / 12.92;
    let high = pow((color + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, color <= vec3<f32>(0.04045));
}

fn linear_to_srgb(color: vec3<f32>) -> vec3<f32> {
    let low = color * 12.92;
    let high = 1.055 * pow(color, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, color <= vec3<f32>(0.0031308));
}

// Narkowicz's fit of the ACES filmic curve: five constants, monotone, and it
// takes an unbounded radiance to `0..=1` without the flat white disc a clamp
// leaves in the middle of every pool.
//
// Chosen over the `knee` it replaces because a knee is a curve with no reference
// behind it — it was fitted to make one debug view readable — and this one is
// what every other renderer means by "tonemapped", which is the whole point of
// being comparable to one. It is not neutral: `1.0` comes out at `0.80`, so a
// white surface under full light is no longer paper-white. That is what a
// shoulder is.
fn tonemap(color: vec3<f32>) -> vec3<f32> {
    let x = color * EXPOSURE;
    let mapped = (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14);
    return clamp(mapped, vec3<f32>(0.0), vec3<f32>(1.0));
}

// The whole pipeline in one call, which is what a caller should reach for:
// decoded art, multiplied by linear light, curved, and encoded back.
fn shade(albedo_srgb: vec3<f32>, light: vec3<f32>) -> vec3<f32> {
    return linear_to_srgb(tonemap(srgb_to_linear(albedo_srgb) * light));
}
