//! `hues.mul`'s 3,000 ramps, packed into one texture a shader can index.
//!
//! # The art is not tinted; it is replaced
//!
//! `get_rgb(color.r, hue)` in `IsometricWorld.fx` is not a multiply. A hued
//! pixel's own colour is thrown away and looked up instead: the atlas already
//! stores each texel's original red channel (widened to eight bits, the same
//! [`Color16::rgb8`](openshard_uofiles::color::Color16::rgb8) every other atlas
//! here uses), and that channel is the index into a 32-step ramp rather than
//! being a colour at all. `statics.wgsl` recovers the original 5-bit index with
//! `round(r * 31.0)` — exact, because the widening it undoes is the same
//! replicate-high-bits expansion `rgb8` performs, and that expansion is a
//! bijection on 0..=31.
//!
//! # What is deliberately not read yet
//!
//! Whether a hue tints only the art's grey pixels is decided here by the hue's
//! own top bit alone — see
//! [`openshard_uofiles::hues::is_partial`]. `tiledata`'s own `PartialHue` flag,
//! which forces the same grey-only behaviour on an item regardless of what the
//! hue on the wire asks for, is not read: nothing in [`crate::atlas`] carries a
//! tiledata reference for a static's sprite, and folding it in means deciding
//! where that lookup happens without a second graphic to test it against.

use std::fmt;

use openshard_protocol::wire::Hue;
use openshard_uofiles::color::{
    Color16,
    Rgb8,
};
use openshard_uofiles::hues::{
    COLORS_PER_HUE,
    Hues,
};

/// Every hue the client knows, packed as a `COLORS_PER_HUE`-wide,
/// `count`-tall RGBA8 grid: row `n` is `Hue(n + 1)`'s ramp.
///
/// Holds its pixels rather than a GPU handle, like every other atlas in this
/// crate: building one needs no device, so a test can assert on the bytes.
pub struct HueRamp {
    height: HueCount,
    /// `COLORS_PER_HUE * height` RGBA8 pixels, row-major.
    pixels: Vec<u8>,
}

/// Number of hue rows packed into a [`HueRamp`].
///
/// This is a row count, not a texture coordinate or a byte length. Keeping
/// that unit distinct inside the atlas prevents the stored height from being
/// accidentally reused as one of the other `u32` dimensions during packing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct HueCount(u32);

impl fmt::Debug for HueRamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HueRamp").field("hues", &self.height).finish()
    }
}

impl HueRamp {
    /// Pack every hue `hues` holds.
    pub fn build(hues: &Hues) -> Self {
        let height = HueCount(hues.count() as u32);
        let mut pixels = vec![0u8; COLORS_PER_HUE * height.0 as usize * 4];
        for row in 0..height.0 {
            // Row `n` is `Hue(n + 1)`: `Hues::get` is one-based, and skipping
            // that offset here would shift every ramp in the texture up by one,
            // exactly the mistake `Hues::get`'s own docs warn about.
            let entry = hues.get(Hue(row as u16 + 1)).expect("row is inside the count");
            for (column, color) in entry.colors.iter().enumerate() {
                write_pixel(&mut pixels, column as u32, row, height.0, *color);
            }
        }
        Self { height, pixels }
    }

    /// The texture's width in pixels. Always [`COLORS_PER_HUE`]: every ramp is
    /// exactly that many steps, so the shader never has to ask.
    pub const fn width() -> u32 {
        COLORS_PER_HUE as u32
    }

    /// The texture's height in pixels: one row per hue this was built from.
    pub fn height(&self) -> u32 {
        self.height.0
    }

    /// Its pixels, RGBA8 and row-major, ready for `write_texture`.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

fn write_pixel(pixels: &mut [u8], column: u32, row: u32, height: u32, color: Color16) {
    let at = ((row * COLORS_PER_HUE as u32 + column) * 4) as usize;
    let Rgb8 { red, green, blue } = color.rgb8();
    pixels[at] = red;
    pixels[at + 1] = green;
    pixels[at + 2] = blue;
    pixels[at + 3] = u8::MAX;
    debug_assert!(row < height);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `hues.mul`'s layout, just enough of it to build one group of eight —
    /// `Hues::parse` refuses anything that is not a whole number of groups, and
    /// there is no public constructor that skips the file format. Hue 1's ramp
    /// is `colors`; the other seven entries in the group are left at zero.
    const GROUP_HEADER: usize = 4;
    const ENTRY_BYTES: usize = COLORS_PER_HUE * 2 + 2 + 2 + 20;
    const HUES_PER_GROUP: usize = 8;

    fn one_group(colors: [Color16; COLORS_PER_HUE]) -> Hues {
        let mut bytes = vec![0u8; GROUP_HEADER + HUES_PER_GROUP * ENTRY_BYTES];
        for (index, color) in colors.iter().enumerate() {
            let at = GROUP_HEADER + index * 2;
            bytes[at..at + 2].copy_from_slice(&color.0.to_le_bytes());
        }
        Hues::parse(&bytes).expect("one whole group")
    }

    #[test]
    fn a_ramp_is_a_row_wide_by_hue_count_tall() {
        let hues = one_group([Color16(0x1F); COLORS_PER_HUE]);
        let ramp = HueRamp::build(&hues);
        assert_eq!(HueRamp::width(), COLORS_PER_HUE as u32);
        assert_eq!(ramp.height(), HUES_PER_GROUP as u32);
        assert_eq!(ramp.pixels().len(), COLORS_PER_HUE * HUES_PER_GROUP * 4);
    }

    #[test]
    fn row_zero_is_hue_one_not_hue_zero() {
        // The one-off `Hues::get`'s own docs warn about: row 0 of the texture
        // has to be `Hue(1)`'s ramp, because `Hue(0)` is "no hue" and is never
        // a row at all — and the second row is `Hue(2)`, the group's next entry,
        // not a repeat of the first.
        let mut colors = [Color16::TRANSPARENT; COLORS_PER_HUE];
        colors[3] = Color16(0b0_11111_00000_00000); // pure red
        let hues = one_group(colors);
        let ramp = HueRamp::build(&hues);

        let pixel_at = |row: u32, column: u32| {
            let at = ((row * COLORS_PER_HUE as u32 + column) * 4) as usize;
            (ramp.pixels()[at], ramp.pixels()[at + 1], ramp.pixels()[at + 2])
        };
        assert_eq!(pixel_at(0, 3), (255, 0, 0), "hue 1's fourth step, widened");
        assert_eq!(pixel_at(0, 0), (0, 0, 0));
        assert_eq!(pixel_at(1, 3), (0, 0, 0), "hue 2 is a different, empty entry");
    }
}
