//! Turning a line of speech into glyph quads.
//!
//! A label is not a sprite: it is several of them, one per character, laid
//! left to right and centred on a single point rather than placed at a
//! top-left corner. That is the only thing this module adds over
//! [`crate::sprite::SpriteQuad`] — a glyph's own quad is exactly as ordinary as
//! a static's once its position is decided.
//!
//! # A character's own width is its advance
//!
//! `fonts.mul` has no kerning table and nothing here invents one: the file's
//! per-character width *is* the distance to the next character, the same way
//! ClassicUO's own `MULTIFontData` walks a string.

use openshard_protocol::speech::Font;
use openshard_protocol::wire::Hue;

use crate::atlas::{FontAtlas, TtfAtlas};
use crate::camera::ViewPixel;
use crate::geometry::Rect;
use crate::sprite::SpriteQuad;

/// One line of overhead text, already resolved to where it hangs.
///
/// Not a [`crate::mobiles::Mobile`] and a message together: matching a
/// [`openshard_protocol::speech::SpokenMessage`] to whoever is drawing it, and
/// deciding how long it stays up, is the caller's business — this only lays
/// bytes into quads once somewhere to put them has been decided.
#[derive(Debug)]
pub struct Label<'a> {
    /// The point the text hangs *from*, in view pixels: its baseline, not its
    /// top. Every glyph grows upward from here rather than down, which is what
    /// makes this the point to hand [`crate::mobiles::head_anchor`]'s own
    /// result to directly — that is the mobile's top edge, and text drawn
    /// downward from it would draw over the sprite's own head instead of
    /// above it.
    pub anchor: ViewPixel,
    /// The line itself. `fonts.mul` is a single-byte table of
    /// [`openshard_uofiles::font::CHARS_PER_FONT`] entries starting at code
    /// point 0, so this is read as bytes rather than as `char`s — a UTF-8
    /// multi-byte sequence has no entry in the table any more than a raw
    /// high byte would, and both are silently skipped rather than boxed.
    pub text: &'a str,
    /// Which face to draw it in.
    pub font: Font,
    /// The wire hue to tint it with, or [`Hue::NONE`].
    pub hue: Hue,
    /// Where it sorts. See [`crate::depth`].
    pub depth: f32,
}

/// Every label as quads, glyph by glyph.
///
/// A byte the atlas never packed — punctuation past `fonts.mul`'s table, a
/// UTF-8 lead or continuation byte — is skipped and does not advance the line:
/// there is no fallback glyph to draw in its place, and leaving a gap the
/// width of a box nobody drew would misalign every character after it for a
/// character that was never going to be there either way.
pub fn collect(labels: &[Label<'_>], atlas: &FontAtlas) -> Vec<SpriteQuad> {
    let mut quads = Vec::new();
    for label in labels {
        let glyphs: Vec<_> = label
            .text
            .bytes()
            .filter_map(|byte| atlas.glyph(label.font, byte))
            .collect();
        let total_width: i32 = glyphs.iter().map(|sprite| i32::from(sprite.width)).sum();
        let mut x = label.anchor.x - total_width / 2;
        for sprite in glyphs {
            if sprite.width > 0 && sprite.height > 0 {
                quads.push(SpriteQuad {
                    rect: Rect {
                        x: x as f32,
                        y: (label.anchor.y - i32::from(sprite.height)) as f32,
                        width: f32::from(sprite.width),
                        height: f32::from(sprite.height),
                    },
                    region: sprite.region,
                    depth: label.depth,
                    hue: u32::from(label.hue.0),
                    // Letters over a head are a message, not a thing standing
                    // in the street: no place, and so never dimmed by night.
                    place: crate::place::Place::NOWHERE,
                    twin: 0,
                    owner: u32::from(crate::occlusion::OwnerId::NONE.raw()),
                });
            }
            x += i32::from(sprite.width);
        }
    }
    quads
}

/// Every label as quads, drawn through a [`TtfAtlas`] instead of `fonts.mul`.
///
/// [`collect`]'s TrueType twin, kept a separate function rather than one
/// generalised over both atlases: the two walk different units — bytes with
/// no bearing to a `fonts.mul` glyph, `char`s with a baseline offset a
/// TrueType glyph cannot do without (see
/// [`TtfSprite::baseline_from_top`](crate::atlas::TtfSprite::baseline_from_top))
/// — and threading one abstraction through both would cost more than the
/// dozen lines it would save.
///
/// `label.font` is not read: a [`TtfAtlas`] rasterizes one face at one pixel
/// size, not `fonts.mul`'s ten, so there is nothing here for it to select —
/// see `openshard_uofiles::ttf_font`'s "One face, not ten" note.
///
/// A `char` the atlas never packed is skipped and does not advance the
/// line, the same contract [`collect`] gives a byte outside `fonts.mul`'s
/// table — but a `TtfAtlas` packs whatever it is asked to grow with (see
/// [`TtfAtlas::add`](crate::atlas::TtfAtlas::add)), so in practice this only
/// fires for a character nobody grew the atlas for yet.
pub fn collect_ttf(labels: &[Label<'_>], atlas: &TtfAtlas) -> Vec<SpriteQuad> {
    let mut quads = Vec::new();
    for label in labels {
        let glyphs: Vec<_> = label.text.chars().filter_map(|ch| atlas.glyph(ch)).collect();
        let total_width: i32 = glyphs.iter().map(|glyph| i32::from(glyph.advance)).sum();
        let mut x = label.anchor.x - total_width / 2;
        for glyph in glyphs {
            if glyph.sprite.width > 0 && glyph.sprite.height > 0 {
                quads.push(SpriteQuad {
                    rect: Rect {
                        x: x as f32,
                        y: (label.anchor.y - glyph.baseline_from_top) as f32,
                        width: f32::from(glyph.sprite.width),
                        height: f32::from(glyph.sprite.height),
                    },
                    region: glyph.sprite.region,
                    depth: label.depth,
                    hue: u32::from(label.hue.0),
                    place: crate::place::Place::NOWHERE,
                    twin: 0,
                    owner: u32::from(crate::occlusion::OwnerId::NONE.raw()),
                });
            }
            x += i32::from(glyph.advance);
        }
    }
    quads
}

#[cfg(test)]
mod tests {
    use crate::atlas::GlyphKey;
    use openshard_uofiles::color::Color16;
    use openshard_uofiles::image::Image;

    use super::*;

    fn glyph(width: u16, height: u16) -> Image {
        Image::new(
            width,
            height,
            vec![Color16(0x7FFF); usize::from(width) * usize::from(height)],
        )
    }

    fn atlas() -> FontAtlas {
        FontAtlas::pack([
            (
                GlyphKey {
                    font: Font(0),
                    char: b'H',
                },
                glyph(6, 10),
            ),
            (
                GlyphKey {
                    font: Font(0),
                    char: b'i',
                },
                glyph(2, 10),
            ),
        ])
        .expect("two glyphs fit")
    }

    /// A line's quads read left to right in the order the string does, each
    /// starting where the one before it ended — `fonts.mul` carries no kerning
    /// table, so the character's own width is the whole of the advance.
    #[test]
    fn glyphs_are_placed_left_to_right_by_their_own_width() {
        let atlas = atlas();
        let quads = collect(
            &[Label {
                anchor: ViewPixel { x: 100, y: 50 },
                text: "Hi",
                font: Font(0),
                hue: Hue::NONE,
                depth: 0.5,
            }],
            &atlas,
        );
        assert_eq!(quads.len(), 2);
        assert_eq!(
            quads[1].rect.x,
            quads[0].rect.x + quads[0].rect.width,
            "i starts where H ends"
        );
        assert_eq!(quads[0].rect.y, 40.0, "grows upward from the anchor, not down");
        assert_eq!(quads[1].rect.y, 40.0);
    }

    /// The whole line is centred on its anchor, not left-aligned to it: two
    /// glyphs of width 6 and 2 span 8, so the line starts 4 pixels either side
    /// of the anchor.
    #[test]
    fn the_line_is_centred_on_its_anchor() {
        let atlas = atlas();
        let quads = collect(
            &[Label {
                anchor: ViewPixel { x: 100, y: 50 },
                text: "Hi",
                font: Font(0),
                hue: Hue::NONE,
                depth: 0.5,
            }],
            &atlas,
        );
        assert_eq!(quads[0].rect.x, 96.0, "100 - 8/2");
        assert_eq!(quads[1].rect.x, 102.0);
    }

    /// A byte the atlas has no glyph for — here, everything but 'H' and 'i' —
    /// is skipped rather than drawn as a gap or a placeholder box.
    #[test]
    fn an_unknown_byte_is_skipped_and_does_not_widen_the_line() {
        let atlas = atlas();
        let with_gap = collect(
            &[Label {
                anchor: ViewPixel { x: 100, y: 50 },
                text: "H!i",
                font: Font(0),
                hue: Hue::NONE,
                depth: 0.5,
            }],
            &atlas,
        );
        let without = collect(
            &[Label {
                anchor: ViewPixel { x: 100, y: 50 },
                text: "Hi",
                font: Font(0),
                hue: Hue::NONE,
                depth: 0.5,
            }],
            &atlas,
        );
        assert_eq!(with_gap.len(), 2);
        assert_eq!(with_gap, without, "the missing '!' left no trace");
    }

    /// Two labels are two independent lines: the second's placement does not
    /// drift because of anything the first one drew.
    #[test]
    fn two_labels_are_placed_independently() {
        let atlas = atlas();
        let quads = collect(
            &[
                Label {
                    anchor: ViewPixel { x: 100, y: 50 },
                    text: "H",
                    font: Font(0),
                    hue: Hue::NONE,
                    depth: 0.5,
                },
                Label {
                    anchor: ViewPixel { x: 300, y: 80 },
                    text: "i",
                    font: Font(0),
                    hue: Hue::NONE,
                    depth: 0.5,
                },
            ],
            &atlas,
        );
        assert_eq!(quads.len(), 2);
        assert_eq!(quads[1].rect.x, 300.0 - 1.0, "300 - 2/2");
        assert_eq!(quads[1].rect.y, 70.0, "80 - the glyph's own height");
    }

    mod ttf {
        use openshard_uofiles::ttf_font::TtfGlyph;

        use super::*;
        use crate::atlas::TtfAtlas;

        /// A synthetic rasterized glyph, the same shape
        /// `atlas::tests::ttf_glyph` builds — a rectangle of one grey level,
        /// its baseline `baseline_from_top` pixels down from its top, and an
        /// `advance` that need not equal its own width.
        fn glyph(width: u16, height: u16, baseline_from_top: i32, advance: u16) -> TtfGlyph {
            TtfGlyph {
                image: Image::new(
                    width,
                    height,
                    vec![Color16(0x7FFF); usize::from(width) * usize::from(height)],
                ),
                baseline_from_top,
                advance,
            }
        }

        fn atlas() -> TtfAtlas {
            TtfAtlas::pack([('H', glyph(6, 10, 10, 7)), ('i', glyph(2, 10, 10, 3))], 16.0)
                .expect("two glyphs fit")
        }

        /// `label.font` never enters into it: `collect_ttf` draws through
        /// whatever the atlas packed, and a face id `fonts.mul` would read is
        /// simply not part of this atlas's key.
        fn label(anchor: ViewPixel, text: &str) -> Label<'_> {
            Label {
                anchor,
                text,
                font: Font(0),
                hue: Hue::NONE,
                depth: 0.5,
            }
        }

        /// Glyphs read left to right, each starting where the one before it's
        /// *advance* ended — not its bitmap width, which a TrueType glyph's
        /// own ink need not fill.
        #[test]
        fn glyphs_are_placed_left_to_right_by_their_advance() {
            let atlas = atlas();
            let quads = collect_ttf(&[label(ViewPixel { x: 100, y: 50 }, "Hi")], &atlas);
            assert_eq!(quads.len(), 2);
            assert_eq!(
                quads[1].rect.x,
                quads[0].rect.x + 7.0,
                "'i' starts 7 past 'H', its advance"
            );
        }

        /// A glyph's top sits `baseline_from_top` pixels above the anchor,
        /// not the bitmap's own height above it — the correction `collect`
        /// does not need because `fonts.mul`'s glyphs have no separate
        /// baseline.
        #[test]
        fn a_glyphs_top_is_offset_by_its_own_baseline_not_its_height() {
            let atlas = atlas();
            let quads = collect_ttf(&[label(ViewPixel { x: 100, y: 50 }, "H")], &atlas);
            assert_eq!(quads[0].rect.y, 40.0, "50 - baseline_from_top (10)");
        }

        /// The whole line centres on its anchor by the same rule `collect`
        /// uses, measured in advances rather than bitmap widths: "Hi" spans
        /// 7 + 3 = 10, so it starts 5 pixels either side of the anchor.
        #[test]
        fn the_line_is_centred_on_its_anchor_by_total_advance() {
            let atlas = atlas();
            let quads = collect_ttf(&[label(ViewPixel { x: 100, y: 50 }, "Hi")], &atlas);
            assert_eq!(quads[0].rect.x, 95.0, "100 - 10/2");
        }

        /// A character the atlas never packed is skipped and leaves no gap —
        /// [`collect`]'s contract for a byte outside `fonts.mul`'s table,
        /// carried over unchanged.
        #[test]
        fn an_unpacked_character_is_skipped_and_does_not_widen_the_line() {
            let atlas = atlas();
            let with_gap = collect_ttf(&[label(ViewPixel { x: 100, y: 50 }, "H!i")], &atlas);
            let without = collect_ttf(&[label(ViewPixel { x: 100, y: 50 }, "Hi")], &atlas);
            assert_eq!(with_gap, without, "the missing '!' left no trace");
        }
    }
}
