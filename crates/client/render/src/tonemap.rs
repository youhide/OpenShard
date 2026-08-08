//! The colour pipeline, on the CPU: sRGB in, linear throughout, tonemapped out.
//!
//! `shaders/tonemap.wesl`'s twin, and the reason there is a twin at all is the
//! reason every other twin in this crate exists — a test can read what the GPU
//! wrote and say whether it is what the arithmetic says it should be, and a
//! renderer whose output nothing predicts is a renderer whose defects are only
//! ever noticed by eye.
//!
//! `docs/lighting_rebuild.md` phase 1 states why the pipeline changed at all:
//! multiplying stored sRGB bytes by a light value computes an arithmetic that
//! cannot be right or wrong about anything, because a file's `128` is not half
//! of its `255`. Light is a quantity; the bytes are not.

/// How much light one unit of scene radiance is worth on screen, before the
/// curve. `shaders/tonemap.wesl`'s `EXPOSURE`, and the two are one number.
pub const EXPOSURE: f32 = 1.0;

/// sRGB's transfer function, decoded — a byte's worth of stored colour to the
/// linear quantity light multiplies.
///
/// The linear segment below `0.04045` is the standard's, not an approximation of
/// it: `pow` near zero is exactly where two implementations of this drift apart.
pub fn srgb_to_linear(channel: f32) -> f32 {
    match channel <= 0.04045 {
        true => channel / 12.92,
        false => ((channel + 0.055) / 1.055).powf(2.4),
    }
}

/// And back, for the one place a linear quantity becomes a stored byte.
pub fn linear_to_srgb(channel: f32) -> f32 {
    match channel <= 0.003_130_8 {
        true => channel * 12.92,
        false => 1.055 * channel.powf(1.0 / 2.4) - 0.055,
    }
}

/// Narkowicz's fit of the ACES filmic curve, per channel.
///
/// Monotone, and it takes an unbounded radiance into `0..=1` without the flat
/// white disc a clamp leaves in the middle of every pool. Not neutral: `1.0`
/// comes out at about `0.80`, which is what a shoulder is.
pub fn tonemap(radiance: f32) -> f32 {
    let x = radiance * EXPOSURE;
    let mapped = (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14);
    mapped.clamp(0.0, 1.0)
}

/// The whole pipeline: stored art, multiplied by linear light, curved, stored
/// again. What a test predicts a lit pixel to be.
pub fn shade(albedo_srgb: f32, light: f32) -> f32 {
    linear_to_srgb(tonemap(srgb_to_linear(albedo_srgb) * light))
}

/// The same for a whole pixel, in the eight-bit units a frame is read back in.
///
/// Rounding, not truncation, and stated here rather than at each call site: a
/// test that truncates disagrees with the GPU on about half of all values by one
/// step, which reads as a defect of the shader.
pub fn shade_u8(albedo: [u8; 3], light: [f32; 3]) -> [u8; 3] {
    let mut out = [0u8; 3];
    for (channel, (stored, lit)) in out.iter_mut().zip(albedo.into_iter().zip(light)) {
        *channel = (shade(f32::from(stored) / 255.0, lit) * 255.0).round() as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two halves of the transfer function meet, and both ends are fixed
    /// points. A decode that did not return `0` for `0` and `1` for `1` would
    /// tint every frame.
    #[test]
    fn the_transfer_function_round_trips() {
        for step in 0..=255u16 {
            let stored = f32::from(step) / 255.0;
            let back = linear_to_srgb(srgb_to_linear(stored));
            assert!(
                (back - stored).abs() < 1e-5,
                "{stored} decoded and encoded came back as {back}",
            );
        }
    }

    /// Light is a quantity, which is the whole claim of this module: two equal
    /// flames on one spot are twice one flame *in the linear domain*, whatever
    /// the curve then does to the sum.
    ///
    /// Stated on the linear side deliberately. The stored bytes are not twice
    /// each other and never were — asserting that they are is the arithmetic
    /// this phase removed.
    #[test]
    fn two_equal_lights_are_twice_one_in_linear_light() {
        let albedo = srgb_to_linear(0.5);
        let one = albedo * 1.0;
        let two = albedo * 2.0;
        assert!((two - one * 2.0).abs() < 1e-6);
        // And the curve keeps their order without keeping their ratio, which is
        // what a shoulder is for.
        assert!(tonemap(two) > tonemap(one));
        assert!(tonemap(two) < tonemap(one) * 2.0);
    }

    /// The curve is monotone over the whole range a frame can carry. A curve
    /// that dipped anywhere would draw a ring inside a pool that nothing in the
    /// scene puts there.
    #[test]
    fn the_curve_never_turns_back() {
        let mut previous = -1.0;
        for step in 0..=4000u16 {
            let radiance = f32::from(step) / 100.0;
            let value = tonemap(radiance);
            assert!(value >= previous, "the curve fell at {radiance}");
            previous = value;
        }
    }

    /// Black stays black. A pixel the art left black is what makes a lit frame
    /// read as light falling on a scene rather than a film laid over it, and it
    /// is the one value no exposure or curve may move.
    #[test]
    fn black_art_stays_black_under_any_light() {
        for light in [0.0, 1.0, 4.0, 100.0] {
            assert_eq!(shade(0.0, light), 0.0);
        }
    }
}
