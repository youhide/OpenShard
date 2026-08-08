//! `fonts.mul`: the ten bitmap faces overhead speech and the classic client's
//! own UI are drawn in.
//!
//! [`speech::Font`](openshard_protocol::speech::Font) is documented as "an index
//! into `fonts.mul`" and this is that file — the two are the same idea from two
//! sides of the wire, so this reader is keyed by that type rather than by a raw
//! `u8`.
//!
//! # Layout
//!
//! ```text
//!   font     BYTE   header, unused                          x 10
//!     char   BYTE   width
//!            BYTE   height
//!            BYTE   unused
//!            WORD[width * height]   pixels, row-major        x 224
//! ```
//!
//! Ten faces, each holding 224 characters starting at code point
//! [`GLYPH_BASE`] (`0x20`, space) — there is no record for a control code
//! below it at all, so character `c`'s glyph is at table index `c -
//! GLYPH_BASE`, and the file itself is the only thing that says so: nothing
//! in it names the base, and a reader that assumed table index `c` was
//! character `c` directly (this one did, until a real file proved it wrong —
//! see "Checked against a shipped file" below) parses every record without
//! error and reads every character's glyph from thirty-two records later
//! than the one that is actually it, silently.
//!
//! A character's own three-byte header, not a fixed cell, is what says how big
//! its pixels are — so unlike [`crate::hues`], a font cannot be measured by
//! dividing the file's length: it has to be walked front to back, one variable
//! record at a time.
//!
//! # Zero is transparent, the way a static sprite's zero is
//!
//! Glyph pixels are [`Color16`], and a zero word is drawn as nothing rather than
//! as black — the opposite convention from land art and the same one
//! [`crate::art::Art::static_art`] uses. A face is text laid over whatever is
//! behind it, so the alternative would draw every glyph as a solid rectangle.
//!
//! # Checked against a shipped file
//!
//! `tests/client_files.rs::a_real_fonts_mul_parses_to_ten_plausible_faces` runs
//! this parser against a real `fonts.mul` under `OPENSHARD_CLIENT` and pins
//! every face's `'A'` and the overall glyph-size shape — a desync in the
//! walk (see the module doc above) would not fail outright, it would read
//! plausible-looking garbage from the wrong offset, which is what that test
//! is built to catch.

use std::fmt;
use std::path::{Path, PathBuf};

use openshard_protocol::speech::Font;

use crate::color::Color16;
use crate::image::Image;

/// How many faces `fonts.mul` holds.
pub const FONT_COUNT: usize = 10;
/// How many characters each face defines, starting at [`GLYPH_BASE`].
pub const CHARS_PER_FONT: usize = 224;
/// The code point table index 0 is. Not a guess: index `'A' as u8 -
/// GLYPH_BASE` (33) is a capital A's pixels in a real shipped file, and index
/// 0 draws nothing — space, not the null character.
pub const GLYPH_BASE: u8 = 0x20;
/// The byte before each face's characters. Unused.
const FONT_HEADER: usize = 1;
/// A character's own header: width, height, and a byte nothing reads.
const CHAR_HEADER: usize = 3;

/// `fonts.mul` could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum AsciiFontError {
    /// The file could not be read.
    Read {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The file ran out of bytes partway through a face or a glyph.
    Truncated {
        /// Which face, 0-based.
        font: usize,
        /// Which character, if a glyph's header or pixels are what ran short.
        /// `None` means the face itself ended before its header byte.
        char: Option<usize>,
    },
}

impl fmt::Display for AsciiFontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Truncated {
                font,
                char: Some(char),
            } => {
                write!(f, "fonts.mul ends inside font {font} character {char}")
            }
            Self::Truncated { font, char: None } => {
                write!(f, "fonts.mul ends before font {font}'s header")
            }
        }
    }
}

impl std::error::Error for AsciiFontError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Truncated { .. } => None,
        }
    }
}

/// Every face `fonts.mul` defines, decoded.
pub struct AsciiFonts {
    /// `FONT_COUNT` faces, each `CHARS_PER_FONT` glyphs indexed by code point.
    faces: Vec<Vec<Image>>,
}

impl fmt::Debug for AsciiFonts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsciiFonts")
            .field("faces", &self.faces.len())
            .finish()
    }
}

impl AsciiFonts {
    /// Read `fonts.mul` in a client directory.
    pub fn open(client_dir: impl AsRef<Path>) -> Result<Self, AsciiFontError> {
        let path = client_dir.as_ref().join("fonts.mul");
        Self::load(path)
    }

    /// Read a `fonts.mul` at an exact path.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, AsciiFontError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|source| AsciiFontError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&bytes)
    }

    /// Parse bytes already in memory.
    ///
    /// Stops after [`FONT_COUNT`] faces even if more data follows — a shipped
    /// file holds exactly ten, and trusting the file's length over that would
    /// read an eleventh face out of whatever a mod or a truncated download left
    /// behind it.
    pub fn parse(bytes: &[u8]) -> Result<Self, AsciiFontError> {
        let mut faces = Vec::with_capacity(FONT_COUNT);
        let mut at = 0usize;
        for font in 0..FONT_COUNT {
            at += FONT_HEADER;
            if at > bytes.len() {
                return Err(AsciiFontError::Truncated { font, char: None });
            }

            let mut glyphs = Vec::with_capacity(CHARS_PER_FONT);
            for char in 0..CHARS_PER_FONT {
                let header = bytes.get(at..at + CHAR_HEADER).ok_or(AsciiFontError::Truncated {
                    font,
                    char: Some(char),
                })?;
                let (width, height) = (header[0] as usize, header[1] as usize);
                at += CHAR_HEADER;

                let pixel_bytes = width * height * 2;
                let pixels = bytes.get(at..at + pixel_bytes).ok_or(AsciiFontError::Truncated {
                    font,
                    char: Some(char),
                })?;
                at += pixel_bytes;

                let mut colors = Vec::with_capacity(width * height);
                for word in pixels.chunks_exact(2) {
                    colors.push(Color16(u16::from_le_bytes([word[0], word[1]])));
                }
                glyphs.push(Image::new(width as u16, height as u16, colors));
            }
            faces.push(glyphs);
        }
        Ok(Self { faces })
    }

    /// How many faces this reads. Always [`FONT_COUNT`] once parsed.
    pub fn len(&self) -> usize {
        self.faces.len()
    }

    /// Whether nothing parsed. Only true for a value nobody built through
    /// [`parse`](Self::parse) or [`load`](Self::load), both of which fill every
    /// face or fail outright.
    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    /// The glyph for `char` in `font`, or `None` if the font index or the
    /// character falls outside the table.
    ///
    /// A glyph the file defines as zero pixels wide or tall — space,
    /// ordinarily — comes back `Some` with an empty [`Image`] rather than
    /// `None`: the character exists in the table, it simply draws nothing.
    /// `None` means "not in this file at all", which is the case below
    /// [`GLYPH_BASE`] (there is no entry for a control code) and at or past
    /// `GLYPH_BASE + `[`CHARS_PER_FONT`].
    pub fn glyph(&self, font: Font, char: u8) -> Option<&Image> {
        let index = char.checked_sub(GLYPH_BASE)?;
        let face = self.faces.get(font.0 as usize)?;
        face.get(index as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One face: `count` characters, each `width` x `height` with pixel value
    /// `(font_index * 100 + char_index)` so every glyph's origin is identifiable.
    fn synthetic(fonts: usize, width: u8, height: u8) -> Vec<u8> {
        let mut bytes = Vec::new();
        for font in 0..fonts {
            bytes.push(0); // the header byte
            for char in 0..CHARS_PER_FONT {
                bytes.push(width);
                bytes.push(height);
                bytes.push(0); // the unused byte
                let value = (font * 1000 + char) as u16;
                for _ in 0..(width as usize * height as usize) {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        bytes
    }

    #[test]
    fn parses_every_face_and_every_character() {
        let fonts = AsciiFonts::parse(&synthetic(FONT_COUNT, 2, 1)).unwrap();
        assert_eq!(fonts.len(), FONT_COUNT);

        let glyph = fonts.glyph(Font(0), GLYPH_BASE + 0x41).unwrap();
        assert_eq!((glyph.width(), glyph.height()), (2, 1));
        assert_eq!(glyph.pixel(0, 0), Some(Color16(0x41)));

        // A different face and a different character both move the origin, so
        // neither the face index nor the character index can be stuck at zero
        // and still pass.
        let glyph = fonts.glyph(Font(3), GLYPH_BASE).unwrap();
        assert_eq!(glyph.pixel(0, 0), Some(Color16(3000)));
    }

    #[test]
    fn a_zero_sized_glyph_is_some_empty_image_not_none() {
        // Control codes are ordinarily zero-sized in a shipped file, and they
        // are still entries in the table — this is what tells that apart from a
        // character past the end of it, which is genuinely absent. `parse`
        // always expects a full ten faces, so every face here is zero-sized
        // except character 1 of font 0, which carries real pixels so a stray
        // width/height swap would show up as a wrong-sized neighbour.
        let mut bytes = synthetic(FONT_COUNT, 0, 0);
        let char_one = FONT_HEADER + CHAR_HEADER;
        bytes[char_one] = 3; // width
        bytes[char_one + 1] = 2; // height
        let pixels: Vec<u8> = (0..3 * 2).flat_map(|_| 0x1234u16.to_le_bytes()).collect();
        // Splice the pixels in after character 1's header rather than
        // overwriting bytes that belong to what follows — the fixture has to
        // stay the right total length or every offset after it desyncs.
        bytes.splice(char_one + CHAR_HEADER..char_one + CHAR_HEADER, pixels);
        let fonts = AsciiFonts::parse(&bytes).unwrap();

        let empty = fonts.glyph(Font(0), GLYPH_BASE).unwrap();
        assert_eq!((empty.width(), empty.height()), (0, 0));
        let sized = fonts.glyph(Font(0), GLYPH_BASE + 1).unwrap();
        assert_eq!((sized.width(), sized.height()), (3, 2));
        assert_eq!(sized.pixel(2, 1), Some(Color16(0x1234)));
        let after = fonts.glyph(Font(0), GLYPH_BASE + 2).unwrap();
        assert_eq!(
            (after.width(), after.height()),
            (0, 0),
            "the splice did not shift character 2"
        );
    }

    #[test]
    fn a_character_past_the_table_is_none_and_so_is_a_font_past_the_end() {
        let fonts = AsciiFonts::parse(&synthetic(FONT_COUNT, 1, 1)).unwrap();
        assert!(fonts.glyph(Font(0), GLYPH_BASE).is_some());
        assert!(
            fonts
                .glyph(Font(0), GLYPH_BASE + (CHARS_PER_FONT - 1) as u8)
                .is_some(),
            "the last real entry in the table"
        );
        assert!(
            fonts.glyph(Font(FONT_COUNT as u16), GLYPH_BASE).is_none(),
            "one past the last real face"
        );
        assert!(
            fonts.glyph(Font(u16::MAX), GLYPH_BASE).is_none(),
            "far past the end, not a panic"
        );
    }

    #[test]
    fn a_character_below_glyph_base_is_none_not_a_control_code_glyph() {
        // The bug this test was written for: a shipped `fonts.mul` has no
        // record at all for a control code, so asking for one must not wrap
        // around into whatever the file actually stored at that raw table
        // offset — it has to come back `None`, the same answer a character
        // past the end of the table gets.
        let fonts = AsciiFonts::parse(&synthetic(FONT_COUNT, 1, 1)).unwrap();
        for char in 0..GLYPH_BASE {
            assert!(
                fonts.glyph(Font(0), char).is_none(),
                "code point {char:#04X} is below GLYPH_BASE and has no glyph"
            );
        }
    }

    #[test]
    fn a_zero_pixel_is_transparent_like_a_static_sprite_and_not_black_like_land() {
        // `synthetic` values every pixel `font * 1000 + char`, so character 0 of
        // font 0 is the one glyph whose value is genuinely zero — a real word a
        // shipped file could contain, not a fixture-only case.
        let bytes = synthetic(FONT_COUNT, 1, 1);
        let fonts = AsciiFonts::parse(&bytes).unwrap();
        let glyph = fonts.glyph(Font(0), GLYPH_BASE).unwrap();
        assert_eq!(glyph.pixel(0, 0), Some(Color16::TRANSPARENT));
        assert!(Color16::TRANSPARENT.is_transparent());
    }

    #[test]
    fn a_file_cut_off_mid_glyph_is_refused_rather_than_panicking() {
        let full = synthetic(1, 4, 4);
        for cut in [
            0,
            FONT_HEADER,
            FONT_HEADER + 1,
            FONT_HEADER + CHAR_HEADER,
            full.len() - 1,
        ] {
            let error = AsciiFonts::parse(&full[..cut]);
            assert!(error.is_err(), "{cut} bytes of a font parsed");
        }
    }

    #[test]
    fn a_file_longer_than_ten_faces_stops_at_the_tenth() {
        // Trailing bytes are somebody else's problem — a mod's extra data, or a
        // truncated download that happens to land past the boundary — and are
        // not read as an eleventh face.
        let mut bytes = synthetic(FONT_COUNT, 1, 1);
        bytes.extend_from_slice(&[0xFF; 64]);
        let fonts = AsciiFonts::parse(&bytes).unwrap();
        assert_eq!(fonts.len(), FONT_COUNT);
    }
}
