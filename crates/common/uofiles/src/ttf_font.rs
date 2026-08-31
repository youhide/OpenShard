//! A TrueType face for the code points `fonts.mul` never shipped.
//!
//! `fonts.mul`'s ten faces (see [`crate::font`]) are a single-byte CP1251
//! table: they cover Latin and Cyrillic, but not Unicode generally. This
//! rasterizes whichever TrueType face an operator points a shard at for the
//! scripts or typography that table cannot provide — see [`TtfFont::open`].
//! This reader does not choose where a face comes from: an application may
//! embed an openly licensed default or load an operator-selected file.
//!
//! # One face, not ten
//!
//! `fonts.mul`'s ten faces are ten different bitmaps at ten different sizes.
//! This wraps one TrueType face rasterized at whatever pixel height the
//! caller asks for, so text drawn through it does not yet vary by
//! [`openshard_protocol::speech::Font`] the way `fonts.mul` text does — a
//! known simplification, not an oversight, and the trade the "draw the whole
//! line through one renderer or not at all" decision made: see
//! `docs/client.md`.
//!
//! # The grey-pixel convention `fonts.mul` already uses
//!
//! A rasterized glyph is anti-aliased coverage, 0 to 255 per pixel.
//! `crate::font`'s own glyphs are already grey pixels a wire hue replaces —
//! see `statics.wgsl`'s fragment shader, which reads a pixel's red channel as
//! `round(r * 31.0)`, a 5-bit index into the hue ramp, and falls back to the
//! stored colour untouched when no hue applies. Coverage becomes that same
//! index (widened to fill all three channels so the "already coloured, leave
//! it alone" partial-hue path never fires on a glyph), so a TrueType glyph's
//! anti-aliased edge shades through the ramp exactly as a `fonts.mul` glyph's
//! would, and zero coverage is [`Color16::TRANSPARENT`] — nothing to draw.

use std::fmt;
use std::path::{
    Path,
    PathBuf,
};

use fontdue::{
    Font,
    FontSettings,
};

use crate::color::Color16;
use crate::image::Image;

/// How much a coverage byte is pushed away from 50% before it is quantized to
/// a 5-bit grey. `1.0` would leave `fontdue`'s own anti-aliasing untouched;
/// higher pulls a soft edge toward solid ink or nothing at all.
///
/// Every other pixel this engine ever draws — land, statics, mobiles,
/// `fonts.mul`'s own glyphs — is nearest-sampled pixel art with no
/// anti-aliasing at all. `fontdue` rasterizes a general TrueType outline the
/// opposite way, spreading a curved edge over several partially-covered
/// pixels, which reads as a soft smear next to everything hard-edged around
/// it rather than as the crisp small text `fonts.mul` draws. Steepening the
/// coverage curve trades a little of that smoothing for edges closer to the
/// rest of the renderer's look, without going all the way to a 1-bit
/// threshold, which on a curve this small starts dropping whole strokes.
const EDGE_SHARPEN: f32 = 1.4;

/// One coverage byte (0 to 255), quantized to the 5-bit grey
/// [`statics.wgsl`](crate) reads as a hue-ramp index — see the module doc's
/// "grey-pixel convention" note — after [`EDGE_SHARPEN`] steepens its edge.
fn sharpened_grey(byte: u8) -> Color16 {
    let coverage = f32::from(byte) / 255.0;
    let sharpened = ((coverage - 0.5) * EDGE_SHARPEN + 0.5).clamp(0.0, 1.0);
    let five_bit = (sharpened * 31.0).round() as u16;
    Color16((five_bit << 10) | (five_bit << 5) | five_bit)
}

/// A TrueType face could not be read or parsed.
#[derive(Debug)]
pub enum TtfFontError {
    /// The file could not be read.
    Read {
        /// Which file.
        path:   PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The bytes are not a TrueType or OpenType face `fontdue` recognises.
    Parse {
        /// Which file.
        path:   PathBuf,
        /// What `fontdue` said, which is a `&str` rather than an
        /// `std::error::Error` on its side — kept as owned text here so this
        /// type does not borrow from a buffer nothing keeps around it.
        reason: String,
    },
}

impl fmt::Display for TtfFontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Parse { path, reason } => write!(f, "{} is not a TrueType face: {reason}", path.display()),
        }
    }
}

impl std::error::Error for TtfFontError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { .. } => None,
        }
    }
}

/// A rasterized glyph: its pixels, and how to place them.
#[derive(Clone, Debug)]
pub struct TtfGlyph {
    /// The glyph's own ink, cropped to its bounding box — not a fixed cell,
    /// the way a `fonts.mul` character's stored width and height are not one
    /// either.
    pub image:             Image,
    /// How far below the image's top edge the baseline sits, in pixels.
    /// Positive for a descender (a "g", a "y") sitting partly below the
    /// baseline, and it can exceed the image's own height when a glyph draws
    /// nothing (space): there is no ink to crop a box around, but the advance
    /// still has to land the next glyph in the right place.
    pub baseline_from_top: i32,
    /// How far to move the pen before drawing the next character, in pixels,
    /// rounded up: `fonts.mul`'s own per-character width is a whole byte, and
    /// a fractional advance here would place two glyphs a sub-pixel apart on
    /// one side and flush on the other, one line at a time.
    pub advance:           u16,
}

/// The measured vertical grid a TrueType face asks text lines to use.
///
/// `fontdue` obtains these from the face's horizontal metrics; unlike a glyph
/// bitmap, they do not change between an `M`, a `g`, or Cyrillic `ф` at the
/// same size.  Keeping this beside the glyph data removes the former guessed
/// `80%` baseline from UI layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TtfLineMetrics {
    /// Pixels from a line's top to its baseline.
    pub baseline_from_top: i32,
    /// Pixels to advance to the next line, including the face's designed gap.
    pub line_height:       i32,
}

/// An operator-supplied TrueType face, ready to rasterize.
pub struct TtfFont {
    font: Font,
}

impl fmt::Debug for TtfFont {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TtfFont").finish()
    }
}

impl TtfFont {
    /// Parse an already-owned or static TrueType/OpenType face.
    ///
    /// Useful to an application that embeds an openly licensed fallback. The
    /// underlying rasterizer owns the parsed face, so a static byte slice does
    /// not tie this value to an application resource's lifetime.
    pub fn from_bytes(bytes: impl std::ops::Deref<Target = [u8]>) -> Result<Self, String> {
        Font::from_bytes(bytes, FontSettings::default())
            .map(|font| Self { font })
            .map_err(str::to_owned)
    }

    /// Read and parse a TrueType or OpenType face from `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, TtfFontError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|source| {
            TtfFontError::Read {
                path: path.to_owned(),
                source,
            }
        })?;
        Self::from_bytes(bytes).map_err(|reason| {
            TtfFontError::Parse {
                path: path.to_owned(),
                reason,
            }
        })
    }

    /// Rasterize one character at `pixel_height`.
    ///
    /// Never fails: a face that has no glyph for `ch` still has a `.notdef`
    /// glyph, the way a font is required to, and `fontdue` rasterizes that
    /// rather than refusing — the same "draw something rather than nothing"
    /// choice a missing `fonts.mul` byte does *not* get to make (see
    /// `crate::text::collect`'s doc), because there is no table here to fall
    /// silently short of.
    ///
    /// Directly at `pixel_height` — an earlier version asked `fontdue` for
    /// four times that and boxed the result back down, on the theory that a
    /// ~9-pixel letter has too little edge *area* for real antialiasing to
    /// survive. Measured against plain `1x` at the same [`EDGE_SHARPEN`], the
    /// boxed-down version's share of partially-covered pixels came out
    /// 0.61–0.69 against 0.59–0.64 — inside rounding noise, not a real gain:
    /// `fontdue` already computes exact analytic coverage at any size, so
    /// there was no missing sample data for a bigger rasterization to
    /// recover. The look this fixes was `EDGE_SHARPEN` alone.
    #[must_use]
    pub fn glyph(&self, ch: char, pixel_height: f32) -> TtfGlyph {
        let (metrics, coverage) = self.font.rasterize(ch, pixel_height);
        let pixels = coverage.into_iter().map(sharpened_grey).collect();
        let image = Image::new(metrics.width as u16, metrics.height as u16, pixels);
        TtfGlyph {
            image,
            baseline_from_top: baseline_from_top(metrics.height as i32, metrics.ymin),
            advance: metrics.advance_width.ceil() as u16,
        }
    }

    /// The face's real baseline and line step at `pixel_height`.
    ///
    /// A malformed face is allowed to omit horizontal metrics.  In that rare
    /// case a one-em high line with an 80%-down baseline is a safe fallback;
    /// normal OpenType faces take the measured path, and callers never need a
    /// per-character approximation.
    #[must_use]
    pub fn line_metrics(&self, pixel_height: f32) -> TtfLineMetrics {
        let fallback = || {
            TtfLineMetrics {
                baseline_from_top: (pixel_height * 0.8).round() as i32,
                line_height:       pixel_height.round() as i32,
            }
        };
        let Some(metrics) = self.font.horizontal_line_metrics(pixel_height) else {
            return fallback();
        };
        let baseline_from_top = metrics.ascent.ceil() as i32;
        let line_height = metrics.new_line_size.ceil() as i32;
        // A broken metric table must not create a zero-height input field.
        match (baseline_from_top > 0, line_height > 0) {
            (true, true) => {
                TtfLineMetrics {
                    baseline_from_top,
                    line_height,
                }
            }
            _ => fallback(),
        }
    }
}

/// How far below a rasterized glyph's own top edge the baseline sits.
///
/// `ymin` is `fontdue`'s own `Metrics::ymin` — the bitmap's bottom edge,
/// offset from the baseline and signed: negative for a descender that
/// reaches below it, positive for a glyph whose ink stops short of the
/// baseline (an apostrophe, a superscript figure). The top edge therefore
/// sits `ymin + height` pixels above the baseline, so descending that far
/// from the top lands exactly on it — a glyph with no ink at all (space) has
/// zero height and this is just `ymin`, the offset exceeding the (empty)
/// image's height that [`TtfGlyph::baseline_from_top`]'s own doc describes.
///
/// A `height - ymin` here — this function's first shape — agrees with
/// `height + ymin` exactly when `ymin` is zero, which is most capitals and
/// digits, and disagrees by `2 * ymin` everywhere else: a descender floats
/// `2 * |ymin|` pixels too high, an overshoot glyph sinks the same amount too
/// low. Pin this down with the two, not just the zero case, or a future
/// "simplification" back to subtraction reads as correct again.
fn baseline_from_top(height: i32, ymin: i32) -> i32 {
    height + ymin
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A glyph that sits exactly on the baseline — `ymin` zero, most capitals
    /// and digits — has its top edge `height` pixels above it: descending
    /// `baseline_from_top` from the image's top lands on the image's own
    /// bottom row, which is where `ymin == 0` says the baseline is. The one
    /// case where `height - ymin` and `height + ymin` cannot be told apart,
    /// which is exactly why the other two below matter.
    #[test]
    fn a_glyph_flush_with_the_baseline_has_no_offset_to_add() {
        assert_eq!(baseline_from_top(14, 0), 14);
    }

    /// A descender — `ymin` negative, `g`/`y`/Cyrillic `ф` — sinks part of
    /// its own bitmap below the baseline, so the top edge is *closer* to the
    /// baseline than the image is tall: `height + ymin`, less than `height`.
    /// The bug this pins: `height - ymin` computes `height + 4` here, `2 *
    /// |ymin|` too far, which is what put a descender's top edge above where
    /// a non-descending capital's own top edge sits — read at the anchor,
    /// where every glyph draws from `anchor.y - baseline_from_top`, that is
    /// the whole glyph floating `2 * |ymin|` pixels too high.
    #[test]
    fn a_descenders_top_sits_closer_to_the_baseline_than_its_own_height() {
        assert_eq!(baseline_from_top(14, -4), 10, "14 - 4, not 14 + 4");
    }

    /// The opposite shape — `ymin` positive, ink that stops short of the
    /// baseline, like an apostrophe — sits *farther* from the baseline than
    /// the image is tall: `height + ymin`, more than `height`.
    #[test]
    fn a_glyph_that_stops_short_of_the_baseline_sits_farther_than_its_own_height() {
        assert_eq!(baseline_from_top(6, 3), 9, "6 + 3, not 6 - 3");
    }

    /// [`EDGE_SHARPEN`]'s whole job: a faint edge pixel is pushed to fully
    /// transparent, a strong one to fully opaque, and only genuinely
    /// half-covered pixels — the one-texel seam an anti-aliased curve cannot
    /// avoid — keep a partial value. Without this a curved letter reads as a
    /// grey smear next to the rest of the renderer's hard-edged pixel art.
    #[test]
    fn edge_sharpening_snaps_faint_and_strong_coverage_to_the_extremes() {
        assert_eq!(
            sharpened_grey(10).0,
            Color16::TRANSPARENT.0,
            "barely covered: snaps to nothing"
        );
        assert_eq!(
            sharpened_grey(245).red(),
            31,
            "almost fully covered: snaps to solid"
        );
        let midpoint = sharpened_grey(128).red();
        assert!(
            midpoint > 0 && midpoint < 31,
            "a genuinely half-covered pixel is still partial, not snapped: got {midpoint}",
        );
    }

    /// A path with nothing behind it is a read error, not a panic — the same
    /// contract every other reader in this crate gives a missing file.
    #[test]
    fn a_missing_font_file_is_a_read_error() {
        let error = TtfFont::open("/nonexistent/path/does-not-exist.ttf").unwrap_err();
        assert!(matches!(error, TtfFontError::Read { .. }));
    }

    /// Bytes that are not a TrueType face are a parse error, not a panic —
    /// an operator pointing `--ttf-font` at the wrong file gets a sentence
    /// naming the file, not a crash three frames later.
    #[test]
    fn bytes_that_are_not_a_font_are_a_parse_error() {
        let path = std::env::temp_dir().join("openshard-ttf-font-test-not-a-font.bin");
        std::fs::write(&path, b"not a font").expect("scratch file writes");
        let error = TtfFont::open(&path).unwrap_err();
        std::fs::remove_file(&path).ok();
        assert!(matches!(error, TtfFontError::Parse { .. }));
    }

    /// A real face, read from wherever `OPENSHARD_TTF_FONT_TEST` points — set
    /// it to any `.ttf`/`.otf` on hand to exercise actual rasterization.
    /// Skipped, not failed, when it is unset: nothing is bundled with the
    /// engine (see the module doc), so there is no font this crate can assume
    /// exists on the machine running the test.
    fn test_font() -> Option<TtfFont> {
        let path = std::env::var_os("OPENSHARD_TTF_FONT_TEST")?;
        Some(TtfFont::open(path).expect("OPENSHARD_TTF_FONT_TEST names a readable TrueType face"))
    }

    /// A capital letter draws real ink and advances the pen — the two things
    /// every glyph in ordinary text needs, whichever face provides them.
    #[test]
    fn a_latin_letter_rasterizes_to_a_nonempty_glyph() {
        let Some(font) = test_font() else { return };
        let glyph = font.glyph('A', 16.0);
        assert!(glyph.image.width() > 0 && glyph.image.height() > 0);
        assert!(glyph.advance > 0);
        assert!(
            (0..glyph.image.width()).any(|x| {
                (0..glyph.image.height()).any(|y| !glyph.image.pixel(x, y).unwrap().is_transparent())
            }),
            "a capital letter draws at least one pixel",
        );
    }

    /// The whole reason this module exists: `fonts.mul` stops at 0xFF and a
    /// Cyrillic letter has to come from somewhere else.
    #[test]
    fn a_cyrillic_letter_rasterizes_to_a_nonempty_glyph() {
        let Some(font) = test_font() else { return };
        let glyph = font.glyph('Я', 16.0);
        assert!(glyph.image.width() > 0 && glyph.image.height() > 0);
        assert!(glyph.advance > 0);
    }

    /// The UI's line grid is read from the face, not inferred from a nominal
    /// 16px request or one particular glyph.  `g` is included to ensure the
    /// descent has somewhere to live under the common baseline.
    #[test]
    fn a_real_face_exposes_a_positive_baseline_and_line_step() {
        let Some(font) = test_font() else { return };
        let metrics = font.line_metrics(16.0);
        let descender = font.glyph('g', 16.0);
        assert!(metrics.baseline_from_top > 0);
        assert!(metrics.line_height >= metrics.baseline_from_top);
        assert!(
            metrics.line_height >= descender.baseline_from_top,
            "the line must reach the shared baseline even for a descender"
        );
    }

    /// A space draws no ink, but still has to move the pen — a caller that
    /// only ever checked pixel counts would happily "optimise" a zero-width
    /// image into a zero-width advance and collapse every word onto one glyph.
    #[test]
    fn a_space_advances_without_drawing_anything() {
        let Some(font) = test_font() else { return };
        let glyph = font.glyph(' ', 16.0);
        assert_eq!(glyph.image.width(), 0);
        assert!(glyph.advance > 0, "a space still has to move the pen");
    }

    /// Coverage becomes a 5-bit grey with all three channels equal — what
    /// `statics.wgsl` reads as a hue-ramp index, and what makes zero coverage
    /// `Color16::TRANSPARENT` rather than some other value that would draw a
    /// faint box around every glyph.
    #[test]
    fn coverage_becomes_a_grey_pixel_the_hue_shader_can_index() {
        let Some(font) = test_font() else { return };
        let glyph = font.glyph('A', 32.0);
        let mut saw_opaque = false;
        for x in 0..glyph.image.width() {
            for y in 0..glyph.image.height() {
                let pixel = glyph.image.pixel(x, y).unwrap();
                assert_eq!(pixel.red(), pixel.green(), "coverage fills every channel equally");
                assert_eq!(pixel.green(), pixel.blue());
                if pixel.red() == 31 {
                    saw_opaque = true;
                }
            }
        }
        assert!(
            saw_opaque,
            "a 32px capital A has at least one fully-covered pixel"
        );
    }
}
