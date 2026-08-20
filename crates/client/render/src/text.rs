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
use crate::gump::GumpPixel;
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
                    volumes: crate::impostor::Range::default(),
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
                    volumes: crate::impostor::Range::default(),
                });
            }
            x += i32::from(glyph.advance);
        }
    }
    quads
}

/// One line of overhead speech already resolved to real screen pixels rather
/// than [`Label::anchor`]'s virtual, camera-scaled ones.
///
/// Why a speech bubble needs this and a static or a mobile does not: every
/// world pass's vertex shader carries a quad's *size* through the same
/// `Projection::scale` it carries its *position* through — camera zoom
/// included, `statics.wesl`'s `vs_main` — which is exactly right for a
/// nearest-sampled pixel-art sprite (a whole zoom step puts every texel on
/// exactly that many real pixels, `Zoom`'s own doc) and exactly wrong for an
/// antialiased TrueType glyph: a nine-pixel `a`, already shaded by
/// `openshard_uofiles::ttf_font::EDGE_SHARPEN`, stretched blocky by a `4x`
/// zoom with no filtering to soften the blocks back down. Drawing after the
/// blit instead — through [`crate::gump::GumpRenderer`], the same as
/// [`GumpLabel`] — means the atlas's own resolution reaches the screen
/// untouched, at whatever zoom the camera is on.
///
/// Centred and baseline-anchored, [`Label`]'s own convention and not
/// [`GumpLabel`]'s: a speech bubble hangs from a point over a head, not from
/// a box's corner.
#[derive(Debug)]
pub struct ScreenLabel<'a> {
    /// The point the text hangs *from*, in real screen pixels — [`Label::anchor`]'s
    /// convention, carried over: its baseline, not its top, with every glyph
    /// growing upward from here. The caller's own conversion from a
    /// [`crate::camera::ViewPixel`] — `crate::camera::Camera::to_viewport`,
    /// plus wherever the world's own rect sits on the surface — is what
    /// makes this real rather than virtual; nothing here has a camera to do
    /// that with.
    pub anchor: GumpPixel,
    /// The line itself.
    pub text: &'a str,
    /// The wire hue to tint it with, or [`Hue::NONE`].
    pub hue: Hue,
}

/// [`collect_ttf`]'s screen-space twin — see [`ScreenLabel`]'s doc for why one
/// exists. Identical math, a [`GumpPixel`] anchor instead of a
/// [`crate::camera::ViewPixel`] one, no [`crate::atlas::FontAtlas`] sibling:
/// `fonts.mul` zooms with the world on purpose (its blocky nearest-sampled
/// magnification is the look every other sprite already has), so only a
/// TrueType face ever needs this.
pub fn collect_screen_ttf(labels: &[ScreenLabel<'_>], atlas: &TtfAtlas) -> Vec<SpriteQuad> {
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
                    // No depth test in this pass, the same reason `collect_gump`
                    // has none: it runs after the blit, over the finished
                    // picture.
                    depth: 0.0,
                    hue: u32::from(label.hue.0),
                    place: crate::place::Place::NOWHERE,
                    twin: 0,
                    owner: u32::from(crate::occlusion::OwnerId::NONE.raw()),
                    volumes: crate::impostor::Range::default(),
                });
            }
            x += i32::from(glyph.advance);
        }
    }
    quads
}

/// One line of screen-space text — a chat line, a journal row — already
/// resolved to where it hangs in the interface's own pixels rather than the
/// world's.
///
/// Top-left anchored and not centred, unlike [`Label`]: [`GumpPixel`] is
/// [`crate::gump::Picture`] and [`crate::gump::Caption`]'s own space, and both
/// already anchor at a top-left corner, so this keeps one convention across
/// the gump-pixel family instead of introducing a second.
#[derive(Clone, Copy, Debug)]
pub struct GumpLabel<'a> {
    /// The line's top-left corner, in gump pixels.
    pub at: GumpPixel,
    /// The line itself — see [`Label::text`] for the byte-table contract.
    pub text: &'a str,
    /// Which face to draw it in.
    pub font: Font,
    /// The wire hue to tint it with, or [`Hue::NONE`].
    pub hue: Hue,
    /// The box the line is cropped to, or `None` to let it overflow.
    ///
    /// `{ croppedtext }` and a paperdoll's name plate are the two things that
    /// have one; a chat line and a `{ text }` do not. See [`collect_gump`] for
    /// what cropping means, which is not what wrapping would.
    pub clip: Option<(i32, i32)>,
}

/// How far down a lowercase letter hangs from the top of its line, per face —
/// `FontsLoader._offsetCharTable`.
const LOWERCASE_DROP: [i32; openshard_uofiles::font::FONT_COUNT] = [2, 0, 2, 2, 0, 0, 2, 2, 0, 0];

/// The same for punctuation and digits — `FontsLoader._offsetSymbolTable`. One
/// face lifts them by a pixel, which is why this is signed.
const SYMBOL_DROP: [i32; openshard_uofiles::font::FONT_COUNT] = [1, 0, 1, 1, -1, 0, 1, 1, 0, 0];

/// How far below the top of a line a glyph's own picture starts —
/// `FontsLoader.GetFontOffsetY`.
///
/// A line of `fonts.mul` is drawn from its **top**, and every glyph's picture is
/// only as tall as its own ink: the file carries no metrics beyond that, so what
/// keeps `Toy` from stepping up and down is this table — capitals sit flush with
/// the top of the line and everything else is nudged down a pixel or two, per
/// face. Without it a lowercase `o` is drawn where a capital `O` would be, which
/// reads as a line whose small letters float.
fn glyph_drop(font: Font, char: u8) -> i32 {
    // The one character with an offset of its own, ahead of every rule below.
    if char == 0xB8 {
        return 1;
    }
    // Capitals — Latin and Cyrillic — and the dieresis sit flush with the top.
    if (0x41..=0x5A).contains(&char) || (0xC0..=0xDF).contains(&char) || char == 0xA8 {
        return 0;
    }
    let Some(lowercase) = LOWERCASE_DROP.get(usize::from(font.0)) else {
        // A face past the file's ten, which nothing should ask for.
        return 2;
    };
    match (0x61..=0x7A).contains(&char) {
        true => *lowercase,
        false => SYMBOL_DROP[usize::from(font.0)],
    }
}

/// Every [`GumpLabel`] as quads, glyph by glyph, left to right from `at`.
///
/// [`collect`]'s screen-space twin: no centring, and the line is anchored by its
/// **top** rather than growing upward from a baseline — `fonts.mul` carries no
/// separate baseline, and a top-aligned row is what
/// [`crate::gump::Picture`]'s own corner already means. Within that line each
/// glyph sits where [`glyph_drop`] puts it. A byte the atlas never packed is
/// skipped and does not advance the line, [`collect`]'s contract.
///
/// A [`GumpLabel::clip`] box **crops**, character by character, which is what
/// `{ croppedtext }` means and what the reference's `FontStyle.Cropped` does: a
/// line that does not fit loses its tail, and is never squeezed or wrapped.
/// Height is honoured the same way — a glyph that would hang below the box is
/// where the line stops — so a one-line box stays one line.
pub fn collect_gump(labels: &[GumpLabel<'_>], atlas: &FontAtlas) -> Vec<SpriteQuad> {
    let mut quads = Vec::new();
    for label in labels {
        let mut x = label.at.x;
        for byte in label.text.bytes() {
            let Some(sprite) = atlas.glyph(label.font, byte) else {
                continue;
            };
            let drop = glyph_drop(label.font, byte);
            if let Some((width, height)) = label.clip {
                if x + i32::from(sprite.width) > label.at.x + width
                    || drop + i32::from(sprite.height) > height
                {
                    break;
                }
            }
            if sprite.width > 0 && sprite.height > 0 {
                quads.push(SpriteQuad {
                    rect: Rect {
                        x: x as f32,
                        y: (label.at.y + drop) as f32,
                        width: f32::from(sprite.width),
                        height: f32::from(sprite.height),
                    },
                    region: sprite.region,
                    // No depth test in this pass; see `crate::gump`'s module
                    // docs for why an interface has none.
                    depth: 0.0,
                    hue: u32::from(label.hue.0),
                    place: crate::place::Place::NOWHERE,
                    twin: 0,
                    owner: u32::from(crate::occlusion::OwnerId::NONE.raw()),
                    volumes: crate::impostor::Range::default(),
                });
            }
            x += i32::from(sprite.width);
        }
    }
    quads
}

/// Roughly how far below the top of a line a TrueType face's baseline sits,
/// as a share of the atlas's own rasterization height.
///
/// [`collect_gump`] has [`glyph_drop`]'s table for this because a
/// `fonts.mul` glyph carries no baseline of its own — see that function's
/// doc. A TrueType glyph does, per character (`TtfGlyph::baseline_from_top`),
/// but the *line's* baseline — where every glyph's own offset is measured
/// from — is a face metric this crate never reads (`openshard_uofiles::ttf_font`'s
/// "One face, not ten" doc explains why there is only a pixel height to ask
/// for, not a parsed `hhea` table), so this is a working approximation
/// rather than a measured one. Purely cosmetic: a line a few pixels off its
/// nominal box still reads fine, which is what lets this stand in for a
/// metric nobody has built a reader for yet.
const BASELINE_SHARE: f32 = 0.8;

/// [`collect_gump`]'s TrueType twin, on the same terms [`collect_ttf`] is
/// [`collect`]'s: `char`s and a per-glyph advance instead of `fonts.mul`
/// bytes, and a real per-glyph baseline instead of [`glyph_drop`]'s table.
///
/// In real screen pixels, like [`collect_ttf`]'s `anchor` — **not** gump
/// pixels, unlike every other `GumpLabel` consumer. A `TtfAtlas` rasterizes
/// at real (density-scaled) pixels — see its doc — and this draws every glyph
/// at exactly that resolution, one texel of atlas to one texel of screen: no
/// division by [`crate::gump::GumpRenderer`]'s own scale, and so nothing to
/// round away. An earlier version divided each glyph's width, height, baseline
/// and advance by that scale separately and let the renderer's shader multiply
/// them back — mathematically a no-op, but four independent roundings per
/// glyph, each landing wherever its own fraction happened to fall, which is
/// what made a line's baseline visibly saw-tooth and its edges soft: the
/// classic shrink-then-regrow blur, self-inflicted one glyph at a time. The
/// caller is responsible for placing `label.at` in real pixels to match —
/// scaling the gump-pixel position *once*, not each glyph's own measurements
/// — see `App::draw`'s HUD block for the conversion.
///
/// A `char` the atlas never packed is skipped and does not advance the line,
/// [`collect_gump`]'s contract carried over unchanged.
pub fn collect_gump_ttf(labels: &[GumpLabel<'_>], atlas: &TtfAtlas) -> Vec<SpriteQuad> {
    let baseline_from_top = (atlas.pixel_height() * BASELINE_SHARE).round() as i32;
    let mut quads = Vec::new();
    for label in labels {
        let mut x = label.at.x;
        let baseline = label.at.y + baseline_from_top;
        for ch in label.text.chars() {
            let Some(glyph) = atlas.glyph(ch) else {
                continue;
            };
            let y = baseline - glyph.baseline_from_top;
            if let Some((box_width, box_height)) = label.clip {
                if x - label.at.x + i32::from(glyph.sprite.width) > box_width
                    || y - label.at.y + i32::from(glyph.sprite.height) > box_height
                {
                    break;
                }
            }
            if glyph.sprite.width > 0 && glyph.sprite.height > 0 {
                quads.push(SpriteQuad {
                    rect: Rect {
                        x: x as f32,
                        y: y as f32,
                        width: f32::from(glyph.sprite.width),
                        height: f32::from(glyph.sprite.height),
                    },
                    region: glyph.sprite.region,
                    depth: 0.0,
                    hue: u32::from(label.hue.0),
                    place: crate::place::Place::NOWHERE,
                    twin: 0,
                    owner: u32::from(crate::occlusion::OwnerId::NONE.raw()),
                    volumes: crate::impostor::Range::default(),
                });
            }
            x += i32::from(glyph.advance);
        }
    }
    quads
}

/// [`gump_width`]'s TrueType twin, the same per-glyph advance
/// [`collect_gump_ttf`] walks by, in the same real screen pixels — what
/// places the chat line's caret at a byte offset into a TrueType-drawn line.
pub fn gump_width_ttf(text: &str, atlas: &TtfAtlas) -> i32 {
    text.chars()
        .filter_map(|ch| atlas.glyph(ch))
        .map(|glyph| i32::from(glyph.advance))
        .sum()
}

/// The pixel width of `text` set in `font`, the same per-byte advance
/// [`collect_gump`] walks by — what places a caret at a byte offset into a
/// line rather than always at its end.
pub fn gump_width(text: &str, font: Font, atlas: &FontAtlas) -> i32 {
    text.bytes()
        .filter_map(|byte| atlas.glyph(font, byte))
        .map(|sprite| i32::from(sprite.width))
        .sum()
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

        fn gump_label(at: GumpPixel, text: &str) -> GumpLabel<'_> {
            GumpLabel {
                at,
                text,
                font: Font(0),
                hue: Hue::NONE,
                clip: None,
            }
        }

        /// [`collect_gump_ttf`]'s own version of
        /// `glyphs_are_placed_left_to_right_by_their_advance`.
        #[test]
        fn glyphs_are_placed_left_to_right_by_their_advance_in_gump_space() {
            let atlas = atlas();
            let quads = collect_gump_ttf(&[gump_label(GumpPixel::new(10, 20), "Hi")], &atlas);
            assert_eq!(quads.len(), 2);
            assert_eq!(
                quads[1].rect.x,
                quads[0].rect.x + 7.0,
                "'i' starts 7 past 'H', its advance"
            );
        }

        /// Unlike [`collect_ttf`], which grows a line upward from a baseline
        /// the caller already picked, [`collect_gump_ttf`] is anchored by its
        /// **top** — [`collect_gump`]'s own convention — so two labels at the
        /// same `x` but different `at.y` place their glyphs the same number
        /// of pixels apart as their `at.y` values are, not by their baseline.
        #[test]
        fn the_line_is_anchored_by_its_top_not_a_baseline() {
            let atlas = atlas();
            let top = collect_gump_ttf(&[gump_label(GumpPixel::new(10, 0), "H")], &atlas);
            let lower = collect_gump_ttf(&[gump_label(GumpPixel::new(10, 5), "H")], &atlas);
            assert_eq!(
                lower[0].rect.y,
                top[0].rect.y + 5.0,
                "the box moved, and the glyph with it"
            );
        }

        /// Every glyph measurement is the atlas's own — no division, no
        /// rounding — which is the whole fix `collect_gump_ttf`'s doc walks
        /// through: a rect's width and a neighbour's advance are exactly the
        /// packed sprite's own numbers, unperturbed.
        #[test]
        fn glyph_measurements_are_the_atlas_own_unrounded() {
            let atlas = atlas();
            let quads = collect_gump_ttf(&[gump_label(GumpPixel::new(0, 0), "Hi")], &atlas);
            assert_eq!(quads[0].rect.width, 6.0, "H's own packed width");
            assert_eq!(quads[1].rect.x, 7.0, "H's own advance, exactly");
        }

        /// A `char` the atlas never packed is skipped and leaves no gap — the
        /// screen-space twin of `an_unpacked_character_is_skipped_and_does_not_widen_the_line`.
        #[test]
        fn an_unpacked_character_is_skipped_in_gump_space_too() {
            let atlas = atlas();
            let with_gap = collect_gump_ttf(&[gump_label(GumpPixel::new(10, 20), "H!i")], &atlas);
            let without = collect_gump_ttf(&[gump_label(GumpPixel::new(10, 20), "Hi")], &atlas);
            assert_eq!(with_gap, without, "the missing '!' left no trace");
        }

        /// [`gump_width_ttf`] sums the same advances [`collect_gump_ttf`]
        /// places glyphs by — what a caret computed from it lands on the
        /// right byte offset.
        #[test]
        fn gump_width_ttf_sums_advances() {
            let atlas = atlas();
            assert_eq!(gump_width_ttf("Hi", &atlas), 10, "7 + 3");
        }
    }

    /// The face every claim below is made about — one with a non-zero drop, so
    /// that a lowercase letter landing flush with the top would be visible.
    const GUMP_FACE: Font = Font(1);

    /// A glyph atlas of solid blocks, so that a line's arithmetic can be tested
    /// without a client's `fonts.mul`.
    fn font_of(glyphs: impl IntoIterator<Item = (u8, u16, u16)>) -> FontAtlas {
        FontAtlas::pack(glyphs.into_iter().map(|(char, width, height)| {
            (
                GlyphKey {
                    font: GUMP_FACE,
                    char,
                },
                glyph(width, height),
            )
        }))
        .expect("a handful of small blocks fit an atlas 2048 on a side")
    }

    /// A line is laid out from its top-left corner, one glyph's own width at a
    /// time, and a lowercase letter hangs a pixel or two lower than a capital —
    /// which is the whole of `fonts.mul`'s vertical metrics.
    #[test]
    fn a_line_advances_by_each_glyph_and_drops_the_lowercase() {
        let font = font_of([(b'A', 6, 10), (b'b', 5, 8)]);
        let quads = collect_gump(
            &[GumpLabel {
                at: GumpPixel::new(10, 20),
                hue: Hue::NONE,
                clip: None,
                text: "Ab",
                font: GUMP_FACE,
            }],
            &font,
        );
        assert_eq!(quads.len(), 2);
        assert_eq!(
            (quads[0].rect.x, quads[0].rect.y),
            (10.0, 20.0),
            "a capital is flush"
        );
        assert_eq!(
            (quads[1].rect.x, quads[1].rect.y),
            (16.0, 20.0 + LOWERCASE_DROP[GUMP_FACE.0 as usize] as f32),
            "the next glyph starts where the first ended, and hangs lower"
        );
    }

    /// `{ croppedtext }` crops and never wraps or squeezes: the glyph that would
    /// cross the box's right edge is the last one, and the tail is simply gone.
    #[test]
    fn a_cropped_line_loses_its_tail_at_the_box() {
        let font = font_of([(b'M', 8, 10)]);
        let line = |clip| GumpLabel {
            at: GumpPixel::new(0, 0),
            hue: Hue::NONE,
            clip,
            text: "MMMM",
            font: GUMP_FACE,
        };
        assert_eq!(
            collect_gump(&[line(None)], &font).len(),
            4,
            "nothing to crop against"
        );
        assert_eq!(
            collect_gump(&[line(Some((20, 20)))], &font).len(),
            2,
            "two whole glyphs fit in twenty pixels, and the third is dropped"
        );
        assert!(
            collect_gump(&[line(Some((20, 4)))], &font).is_empty(),
            "a line taller than its box is not drawn at all"
        );
    }

    /// A byte the atlas never packed is skipped and does not advance the line —
    /// there is no glyph to draw and no width to advance by, and a gap would
    /// misalign every character after it.
    #[test]
    fn a_byte_with_no_glyph_leaves_no_gap() {
        let font = font_of([(b'A', 6, 10)]);
        let quads = collect_gump(
            &[GumpLabel {
                at: GumpPixel::default(),
                hue: Hue::NONE,
                clip: None,
                // The middle byte is a UTF-8 lead, which `fonts.mul` has no
                // entry for at all.
                text: "AжA",
                font: GUMP_FACE,
            }],
            &font,
        );
        assert_eq!(quads.len(), 2);
        assert_eq!(quads[1].rect.x, 6.0, "the second A follows the first");
    }
}
