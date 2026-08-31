//! The one pixel format every client file stores colour in.
//!
//! `hues.mul`, `art`, `gumpart` and `texmaps` all store a colour as a `u16` with
//! five bits per channel: `0RRRRRGGGGGBBBBB`. There is no alpha channel — a
//! pixel is either a colour or it is not there — and which of those a zero means
//! depends on what is being drawn, so it is [`Color16::is_transparent`] rather
//! than something baked into the parse.
//!
//! The channel order is not written down anywhere and is easy to get backwards;
//! it is checked against a shipped tile in the tests, which is the only place a
//! guess would show up as anything other than plausible colours.

use std::fmt;

/// A colour as the client's files store it: five bits per channel in a `u16`.
///
/// The raw word is public because it goes back out to a texture upload
/// unchanged, and because a hue table is compared by value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Color16(pub u16);

/// An opaque colour after widening the file format's five-bit channels.
///
/// The renderer uploads these components in RGB order.  It is named instead
/// of returning three anonymous bytes so a caller cannot mistake the value for
/// a coordinate or another byte triple.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb8 {
    pub red:   u8,
    pub green: u8,
    pub blue:  u8,
}

impl Color16 {
    /// The absence of a pixel.
    ///
    /// Zero is "nothing here" in art, where a sprite's runs only cover the
    /// pixels it draws. It is *also* a legitimate black in a hue table, which is
    /// why nothing in this type treats zero as special on its own.
    pub const TRANSPARENT: Self = Self(0);

    /// Bits per channel.
    const CHANNEL_BITS: u16 = 5;
    /// The largest value one channel can hold.
    const CHANNEL_MAX: u16 = (1 << Self::CHANNEL_BITS) - 1;

    /// The red channel, 0..=31.
    pub const fn red(self) -> u8 {
        ((self.0 >> (2 * Self::CHANNEL_BITS)) & Self::CHANNEL_MAX) as u8
    }

    /// The green channel, 0..=31.
    pub const fn green(self) -> u8 {
        ((self.0 >> Self::CHANNEL_BITS) & Self::CHANNEL_MAX) as u8
    }

    /// The blue channel, 0..=31.
    pub const fn blue(self) -> u8 {
        (self.0 & Self::CHANNEL_MAX) as u8
    }

    /// Whether this pixel is absent rather than coloured.
    ///
    /// Only meaningful for art. Callers reading a hue table want the black.
    pub const fn is_transparent(self) -> bool {
        self.0 == 0
    }

    /// The colour as eight bits per channel.
    ///
    /// The expansion is `v << 3 | v >> 2`, not `v << 3`. A plain shift maps the
    /// brightest channel a file can hold to 248 rather than 255, so every white
    /// in the game comes out faintly grey and every gradient stops short of its
    /// own top end — a mistake that looks like a washed-out monitor rather than
    /// like a bug. Replicating the high bits down is the standard 5-to-8 widening
    /// and lands 31 exactly on 255.
    pub const fn rgb8(self) -> Rgb8 {
        Rgb8 {
            red:   widen(self.red()),
            green: widen(self.green()),
            blue:  widen(self.blue()),
        }
    }
}

/// Widen a five-bit channel to eight bits so that the full range maps onto the
/// full range.
const fn widen(channel: u8) -> u8 {
    (channel << 3) | (channel >> 2)
}

impl fmt::Debug for Color16 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Rgb8 { red, green, blue } = self.rgb8();
        write!(f, "Color16(0x{:04X} #{red:02X}{green:02X}{blue:02X})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channels_are_red_green_blue_from_the_top() {
        // Each constant lights one channel and nothing else. Backwards channel
        // order is the mistake this catches, and in a rendered image it looks
        // like a colour scheme somebody chose.
        assert_eq!(Color16(0b0_11111_00000_00000).red(), 31);
        assert_eq!(Color16(0b0_11111_00000_00000).green(), 0);
        assert_eq!(Color16(0b0_11111_00000_00000).blue(), 0);

        assert_eq!(Color16(0b0_00000_11111_00000).green(), 31);
        assert_eq!(Color16(0b0_00000_00000_11111).blue(), 31);

        // The top bit is not a channel. Some readers set it to mean "opaque";
        // nothing here reads it, and it must not leak into red.
        assert_eq!(Color16(0x8000).red(), 0);
    }

    #[test]
    fn the_full_range_widens_to_the_full_range() {
        assert_eq!(widen(0), 0);
        assert_eq!(widen(31), 255, "a plain shift would give 248");
        assert_eq!(widen(16), 132);
        // Monotonic, so a gradient in the file is a gradient on screen.
        for channel in 1..=31u8 {
            assert!(widen(channel) > widen(channel - 1));
        }
    }

    #[test]
    fn transparent_is_only_the_zero_word() {
        assert!(Color16::TRANSPARENT.is_transparent());
        assert!(Color16(0).is_transparent());
        // Pure black with the top bit set is a colour, not an absence.
        assert!(!Color16(0x8000).is_transparent());
        assert!(!Color16(1).is_transparent());
    }
}
