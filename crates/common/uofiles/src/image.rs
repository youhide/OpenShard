//! A decoded picture, in the one shape every art format here ends up in.
//!
//! Three of the client's containers hold pixels — [`crate::art`], the texture
//! maps in [`crate::texmaps`], and the gump art that is not read yet — and they
//! store them three different ways: a fixed diamond, a raw square, run-length
//! encoded rows. What comes out is the same thing in every case, so it lives
//! here rather than in whichever reader happened to be written first.

use std::fmt;

use crate::color::Color16;

/// The smallest rectangle containing every non-transparent pixel in an
/// [`Image`].
///
/// Kept in image-local coordinates: a font glyph uses it to measure real ink,
/// while an atlas image can use the same answer without pretending its empty
/// margins belong to a screen coordinate system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InkBounds {
    pub x:      u16,
    pub y:      u16,
    pub width:  u16,
    pub height: u16,
}

/// A decoded sprite: a rectangle of pixels, transparent where nothing is drawn.
///
/// Row-major, `width * height` long. Transparency is [`Color16::TRANSPARENT`]
/// rather than a mask, because that is how the file stores it and a renderer
/// wants one buffer, not two.
///
/// Whether a zero pixel is *absent* or *black* is the format's business and not
/// this type's: a land diamond has no transparency at all and a static sprite is
/// mostly transparency. See [`crate::art::land_row`] for the one case where the
/// shape has to be recovered from outside the pixels.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    width:  u16,
    height: u16,
    pixels: Vec<Color16>,
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let drawn = self.pixels.iter().filter(|p| !p.is_transparent()).count();
        f.debug_struct("Image")
            .field("size", &format!("{}x{}", self.width, self.height))
            .field("drawn", &drawn)
            .finish()
    }
}

impl Image {
    /// Build one from pixels somebody already decoded.
    ///
    /// # Panics
    ///
    /// If `pixels` is not exactly `width * height` long. That is a caller's
    /// arithmetic and not a file's contents — every reader in this crate has
    /// already validated its input by the time it gets here, and a `Result`
    /// would only push an impossible case out to every one of them.
    pub fn new(width: u16, height: u16, pixels: Vec<Color16>) -> Self {
        assert_eq!(
            pixels.len(),
            usize::from(width) * usize::from(height),
            "an image's pixels have to fill its rectangle exactly",
        );
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Its width in pixels.
    pub const fn width(&self) -> u16 {
        self.width
    }

    /// Its height in pixels.
    pub const fn height(&self) -> u16 {
        self.height
    }

    /// Every pixel, row-major from the top-left.
    pub fn pixels(&self) -> &[Color16] {
        &self.pixels
    }

    /// One pixel, or `None` outside the image.
    pub fn pixel(&self, x: u16, y: u16) -> Option<Color16> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.pixels
            .get(y as usize * self.width as usize + x as usize)
            .copied()
    }

    /// The bounds of visible ink, excluding transparent padding.
    #[must_use]
    pub fn ink_bounds(&self) -> Option<InkBounds> {
        let mut left = self.width;
        let mut top = self.height;
        let mut right = 0;
        let mut bottom = 0;
        let mut any = false;
        for y in 0..self.height {
            for x in 0..self.width {
                if self.pixel(x, y).is_some_and(|pixel| !pixel.is_transparent()) {
                    left = left.min(x);
                    top = top.min(y);
                    right = right.max(x);
                    bottom = bottom.max(y);
                    any = true;
                }
            }
        }
        if !any {
            return None;
        }
        Some(InkBounds {
            x:      left,
            y:      top,
            width:  right - left + 1,
            height: bottom - top + 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pixel_outside_the_rectangle_is_none_and_not_a_wrap() {
        let image = Image::new(
            2,
            3,
            vec![
                Color16(1),
                Color16(2),
                Color16(3),
                Color16(4),
                Color16(5),
                Color16(6),
            ],
        );
        assert_eq!(image.pixel(1, 2), Some(Color16(6)));
        // The trap this catches: `y * width + x` happily addresses (2, 0) as
        // (0, 1), so a missing bounds check reads the next row rather than
        // saying there is nothing there.
        assert_eq!(image.pixel(2, 0), None);
        assert_eq!(image.pixel(0, 3), None);
    }

    #[test]
    #[should_panic(expected = "fill its rectangle")]
    fn pixels_that_do_not_fill_the_rectangle_are_a_bug_and_not_an_image() {
        let _ = Image::new(4, 4, vec![Color16(0); 15]);
    }

    #[test]
    fn ink_bounds_excludes_the_transparent_margin() {
        let image = Image::new(
            5,
            4,
            vec![
                Color16(0),
                Color16(0),
                Color16(0),
                Color16(0),
                Color16(0), // row 0
                Color16(0),
                Color16(7),
                Color16(0),
                Color16(0),
                Color16(0), // row 1
                Color16(0),
                Color16(0),
                Color16(9),
                Color16(0),
                Color16(0), // row 2
                Color16(0),
                Color16(0),
                Color16(0),
                Color16(0),
                Color16(0), // row 3
            ],
        );
        assert_eq!(
            image.ink_bounds(),
            Some(InkBounds {
                x:      1,
                y:      1,
                width:  2,
                height: 2,
            })
        );
    }
}
