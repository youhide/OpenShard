//! The hover's **property card**: what it is made of, how big it is, and where
//! it sits relative to the pointer.
//!
//! An object property list is information about one game object, not a string
//! that happens to be near the pointer. This module is the half that decides
//! what that information looks like: it is handed lines somebody else already
//! resolved and answers with measured rectangles, an optional icon and placed
//! labels. It reads no world, sends no packet and touches no GPU — see
//! `plans/client/tooltips/PLAN.md`, whose fourth implementation boundary is
//! exactly this seam, and `crate::gump` for the pass the answer is drawn
//! through.
//!
//! # Why the card is measured before it is placed
//!
//! Because where it goes depends on how big it is. A card anchored first and
//! measured afterwards can only discover that it runs off the lower-right
//! corner once it is already drawn there, which is what every hover near the
//! edge of the window looks like. So the order here is fixed: wrap the lines to
//! a width, add up the height, and only then try the four anchors around the
//! cursor — see [`anchor`].
//!
//! # Why the fill is opaque
//!
//! The design asks for a nearly-black *translucent* fill. The gump pass does no
//! blending at all — see [`crate::gump::plate`], and `gump.wgsl`, which returns
//! an alpha of one for every quad — so translucency is not available to
//! anything drawn through it. The fill below is therefore the darkest value
//! that still reads as a piece of interface rather than as a hole cut in the
//! world, which is the same argument `crate::gump::plate`'s own doc makes and
//! the same one the chat's plates are set by. A blended gump pass would let
//! this become what the design asked for; nothing else here would change.

use std::fmt;

use openshard_protocol::speech::Font;
use openshard_protocol::wire::{
    Graphic,
    Hue,
};

use crate::atlas::{
    FontAtlas,
    TextSize,
    TtfAtlas,
};
use crate::geometry::Rect;
use crate::gump::{
    GumpAtlas,
    GumpPixel,
    ItemCell,
    Picture,
    Shade,
    plate,
};
use crate::sprite::SpriteQuad;
use crate::text::{
    GumpLabel,
    gump_width,
    gump_width_ttf,
};

/// The face the title is set in — the same one every window caption in this
/// client uses, at the tooltip's own size.
///
/// It is `fonts.mul`'s face 1, which is also [`BODY_FONT`] below. The two are
/// named separately because they answer different questions — "how is a heading
/// drawn" and "how is a property drawn" — and a card that stops distinguishing
/// them the moment one of the two changes would be a card nobody could restyle.
pub const TITLE_FONT: Font = crate::gump::CAPTION_FONT;

/// The face the properties are set in — the face this client's hover text has
/// always been drawn in.
pub const BODY_FONT: Font = Font(1);

/// The ink every line of the card is written in.
///
/// One hue for the title and the properties both. The design allows an item's
/// quality to tint the title, and deliberately nothing else: a property line
/// tinted by quality is a property line that is unreadable at some hues. That
/// tint is **not** applied here, because the only thing that could decide it is
/// the resolved English text, and reading meaning out of resolved text is what
/// the data contract forbids (`plans/client/tooltips/PLAN.md`, "Data
/// contract"). It becomes possible when the shard sends structured properties;
/// until then the title is told apart by its place and its face, not by colour.
const INK: Hue = Hue::LABEL;

/// How dark the card's fill is, on [`Shade`]'s zero-to-one scale.
///
/// Near black, and not black: see the module docs for why this cannot be the
/// translucent fill the design asks for, and why the darkest usable value is
/// still short of the end of the ramp.
const FILL_SHADE: f32 = 0.10;

/// And the one-pixel border and the rule under the head, which are the same
/// muted grey.
const BORDER_SHADE: f32 = 0.45;

/// Clear space inside the border, on every edge.
const PADDING: i32 = 8;

/// The gap above and below the rule that divides the head from the body.
const HEAD_GAP: i32 = 4;

/// How wide a card prefers to be, in gump pixels; how narrow it may shrink for
/// a short list; and how wide it may grow to keep a long one within its height.
const PREFERRED_WIDTH: i32 = 280;
const MIN_WIDTH: i32 = 220;
const MAX_WIDTH: i32 = 360;

/// The side of the square the item's icon is fitted into, and the air left
/// around it inside that square.
const ICON: i32 = 36;
const ICON_PADDING: i32 = 2;

/// How far the card is held off the pointer, so that the hotspot never sits on
/// the first line it is asking about.
const POINTER_GAP: GumpPixel = GumpPixel::new(14, 18);

/// How close to the edge of the game surface a card may be placed.
const SCREEN_MARGIN: i32 = 12;

/// The share of the game surface a card's height aims to stay within.
const HEIGHT_SHARE: f32 = 0.6;

/// How many lines after the title may stand in the head — the quality, the
/// maker, the count on a pile.
///
/// **A display rule, not a reading of the text.** Which lines identify an
/// object and which describe it is a fact the shard knows and the wire does not
/// carry: an object property list is a flat sequence, and the only thing this
/// client can tell about a line is where in that sequence it came. So the head
/// is the title plus the next two lines, always, and the body is the rest —
/// stable, cheap, and wrong for nothing a shard sends today, because the server
/// writes the name, then the quality, then the maker, then everything else
/// (`WorldState::object_properties`). Deciding it by parsing "Exceptional" out
/// of a resolved string would break on the first client language or custom
/// shard that spells it differently, which is exactly what the data contract
/// rules out. When the shard sends a structured property model the boundary
/// moves there and this constant goes.
const HEADER_LINES: usize = 2;

/// How many lines a card shows before it has been hovered long enough to open.
///
/// The title and the first property: enough to answer "what is this" in the
/// glance a player spends on most objects, and short enough that a pointer
/// crossing a full bag does not paper the screen over.
const COMPACT_LINES: usize = 2;

/// How much of the card is drawn.
///
/// The compact card is what a pointer passing over an object gets; the detailed
/// one is what it becomes when the pointer stays. Both are drawn from the same
/// cached property list and neither asks the shard anything — the expansion is
/// a change of mind about what to show, not a second question.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// The title and the first property line.
    Compact,
    /// Every property the shard sent.
    Detail,
}

/// The two `fonts.mul` faces a card is set in.
///
/// Handed in rather than read from the constants above so that one caller — the
/// F1 classic-face override — can replace both without this module learning
/// what that setting is. A TrueType card ignores them: that path has one
/// family, and [`Measure::TrueType`] never looks at a face.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Faces {
    pub title: Font,
    pub body:  Font,
}

impl Faces {
    /// The client's own two faces, which is what every card that is not
    /// overriding the face is drawn in.
    #[must_use]
    pub const fn client() -> Self {
        Self {
            title: TITLE_FONT,
            body:  BODY_FONT,
        }
    }
}

/// How text is measured, which is the one thing a card cannot work out for
/// itself.
///
/// An enum over the two faces this client can be running and not a trait
/// object: there are exactly two, both are known here, and the difference
/// between them is arithmetic rather than behaviour. Both arms answer in **gump
/// pixels**, which is the space every other number in this module is in —
/// `fonts.mul` measures in its own pixels and is drawn `magnify` times bigger,
/// a TrueType face is rasterized at the display's density and measures in real
/// ones, and neither of those is the card's space.
pub enum Measure<'a> {
    /// `fonts.mul`, drawn `magnify` times its own size.
    Bitmap { atlas: &'a FontAtlas, magnify: f32 },
    /// A TrueType face at role size `size`, rasterized for a display of
    /// `density`.
    ///
    /// The atlas must already hold the card's characters: an unpacked glyph
    /// measures zero (`crate::text::gump_width_ttf`), so a card measured before
    /// its glyphs were packed would be sized off an empty string. See
    /// [`Measure::packed`] for the size to pack at.
    TrueType {
        atlas:   &'a TtfAtlas,
        size:    TextSize,
        density: f32,
    },
}

impl fmt::Debug for Measure<'_> {
    /// The numbers and not the atlas: a glyph atlas is megabytes of packed
    /// pixels, and none of it says anything about how a card was measured.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bitmap { magnify, .. } => write!(f, "Bitmap {{ magnify: {magnify} }}"),
            Self::TrueType { size, density, .. } => {
                write!(f, "TrueType {{ size: {size:?}, density: {density} }}")
            }
        }
    }
}

impl Measure<'_> {
    /// The size a TrueType role of `size` is actually rasterized at on a
    /// display of `density`.
    ///
    /// An associated function rather than a method because it is needed
    /// *before* a [`Measure::TrueType`] can exist: its atlas has to hold the
    /// card's glyphs already, and this is the size to pack them at. It is the
    /// same product the draw pass asks the atlas for, so measuring and drawing
    /// cannot end up looking at two different sets of glyphs.
    #[must_use]
    pub fn packed(size: TextSize, density: f32) -> TextSize {
        size.scaled(density)
    }

    /// How wide `text` is drawn in `font`, in gump pixels.
    #[must_use]
    pub fn width(&self, text: &str, font: Font) -> i32 {
        match self {
            Self::Bitmap { atlas, magnify } => {
                (gump_width(text, font, atlas) as f32 * magnify).round() as i32
            }
            Self::TrueType { atlas, size, density } => {
                // Real pixels back to gump ones, once and here — the same
                // division `chat::channel_width` does for the same reason.
                (gump_width_ttf(text, atlas, Self::packed(*size, *density)) as f32 / density).round() as i32
            }
        }
    }

    /// The vertical step between two lines set in `font`, in gump pixels.
    ///
    /// Read off the face rather than written down: `fonts.mul` holds ten faces
    /// of different heights, and a number fixed here would be wrong the moment
    /// a card was drawn in another one. A capital `M`'s actual *ink* height is
    /// the measure — transparent padding in a glyph cell does not count — plus
    /// two pixels so consecutive lines do not touch. A TrueType face has a
    /// requested size instead, and the step is that size plus the same two
    /// pixels of air. The bitmap fallback is only reachable with a font atlas
    /// that packed no `M`, which is a broken `fonts.mul` rather than a case to
    /// be right about.
    #[must_use]
    pub fn line_step(&self, font: Font) -> i32 {
        match self {
            Self::Bitmap { atlas, magnify } => {
                (atlas
                    .glyph_ink_height(font, b'M')
                    .map_or(16, |height| i32::from(height) + 2) as f32
                    * magnify)
                    .round() as i32
            }
            Self::TrueType { size, .. } => size.pixels().round() as i32 + 2,
        }
    }
}

/// The part of the window a card is allowed to stand in, in gump pixels.
///
/// Not the whole surface: a docked panel takes its space before the world is
/// drawn, and a card anchored to the surface would be placed underneath one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Surface {
    pub at:     GumpPixel,
    pub width:  i32,
    pub height: i32,
}

/// What a card is made of: the object's resolved property list, its icon, and
/// how much of the list to show.
#[derive(Debug)]
pub struct Content<'a> {
    /// The property list as text, first line first — the title is `lines[0]`.
    /// Empty is a real answer and means "this object says nothing", which is
    /// not the same as there being no object under the pointer.
    pub lines:   &'a [String],
    /// The art to draw beside the title, when the object has one. A mobile
    /// does not, and reserves no blank column for it.
    pub graphic: Option<Graphic>,
    /// Whether the pointer has stayed long enough for the whole list.
    pub phase:   Phase,
}

/// One laid-out row of a card, in the same gump pixels the surface is in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CardLine {
    pub at:   GumpPixel,
    pub text: String,
    pub font: Font,
    pub hue:  Hue,
}

/// A measured, wrapped and placed property card.
///
/// Every coordinate is absolute and unmagnified: a card belongs to no window,
/// so `crate::desk::WindowScale` has nothing to say about it and
/// [`crate::gump::place`] has nothing left to do. What remains is the display's
/// own density, which the pass applies in its shader.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Card {
    /// The card's top-left corner.
    pub at:      GumpPixel,
    pub width:   i32,
    pub height:  i32,
    /// The item's art, fitted into its square, or `None` for an object with no
    /// suitable picture.
    pub icon:    Option<Picture>,
    /// Every row, head first.
    pub lines:   Vec<CardLine>,
    /// Where the rule between head and body is drawn, or `None` when the card
    /// has no body to divide off.
    pub divider: Option<i32>,
}

impl Card {
    /// The card's own furniture: the border, the fill inside it, and the rule
    /// under the head.
    ///
    /// In painter's order, which is the only order this pass has: the border is
    /// a plate the size of the whole card and the fill is a second one inset by
    /// a pixel, so the frame is what is left showing round the edge. Two quads
    /// rather than four edges, for the reason there is no blending — an opaque
    /// fill covers what it is drawn over outright, and the cheapest correct
    /// frame is therefore a plate under a plate.
    #[must_use]
    pub fn plates(&self) -> Vec<SpriteQuad> {
        let rect = |x: i32, y: i32, width: i32, height: i32| {
            Rect {
                x:      x as f32,
                y:      y as f32,
                width:  width as f32,
                height: height as f32,
            }
        };
        let mut quads = vec![
            plate(
                rect(self.at.x, self.at.y, self.width, self.height),
                Hue::NONE,
                Shade::new(BORDER_SHADE),
            ),
            plate(
                rect(self.at.x + 1, self.at.y + 1, self.width - 2, self.height - 2),
                Hue::NONE,
                Shade::new(FILL_SHADE),
            ),
        ];
        if let Some(y) = self.divider {
            quads.push(plate(
                rect(self.at.x + PADDING, y, self.width - 2 * PADDING, 1),
                Hue::NONE,
                Shade::new(BORDER_SHADE),
            ));
        }
        quads
    }

    /// Every row as a label the text passes can draw, already placed.
    #[must_use]
    pub fn labels(&self) -> Vec<GumpLabel<'_>> {
        self.lines
            .iter()
            .map(|line| {
                GumpLabel {
                    at:   line.at,
                    text: &line.text,
                    font: line.font,
                    hue:  line.hue,
                    // Wrapped, never cropped: a property the card cut in half
                    // is a property the player is being lied to about. See
                    // [`wrap`].
                    clip: None,
                }
            })
            .collect()
    }
}

/// Lay `content` out and place it around `cursor`, or answer `None` when there
/// is nothing to draw.
///
/// `icons` is the atlas the item's art was packed into; an object whose picture
/// is not in it simply gets no icon, exactly as [`crate::gump::collect`] draws
/// no picture for one.
///
/// # How wide it comes out
///
/// [`PREFERRED_WIDTH`] is where a card wants to be. A list whose longest line
/// needs less than that shrinks to fit it, down to [`MIN_WIDTH`] and no
/// further — a card that hugged a three-letter name would be a label again.
/// A list that needs more wraps at the preferred width instead of growing, so
/// that ordinary cards are all the same width. The one thing that widens a card
/// is height: if wrapping at the preferred width makes it taller than
/// [`HEIGHT_SHARE`] of the surface, it is laid out again at [`MAX_WIDTH`],
/// where the same text takes fewer rows.
///
/// A list that is *still* taller than that share is drawn whole and too tall.
/// Dropping the overflow would be silently telling the player an item has
/// fewer properties than it has, which is the one outcome the design rules
/// out; the real answer is a pinned inspect pane, and it is not this.
#[must_use]
pub fn card(
    content: Content<'_>,
    faces: Faces,
    measure: &Measure<'_>,
    icons: &GumpAtlas,
    cursor: GumpPixel,
    surface: Surface,
) -> Option<Card> {
    let shown: Vec<&str> = match content.phase {
        Phase::Compact => content.lines.iter().take(COMPACT_LINES),
        Phase::Detail => content.lines.iter().take(usize::MAX),
    }
    .map(String::as_str)
    .collect();
    if shown.is_empty() {
        return None;
    }
    // Fitted into its square before anything is measured, because whether there
    // is an icon at all is what the head's left edge is: an object whose art is
    // not packed reserves no column for it.
    let icon = content.graphic.and_then(|graphic| {
        ItemCell::new(GumpPixel::new(PADDING, PADDING), ICON, ICON)
            .padded(ICON_PADDING)
            .picture(icons, graphic)
    });
    let chrome = 2 * PADDING + icon.map_or(0, |_| ICON + PADDING);
    let natural = shown
        .iter()
        .enumerate()
        .map(|(row, text)| {
            let font = font_of(row, faces);
            // Only the head is indented past the icon; a body line starts at
            // the card's own padding, so it needs less room for the same text.
            let indent = match row < 1 + HEADER_LINES {
                true => chrome,
                false => 2 * PADDING,
            };
            indent + measure.width(text, font)
        })
        .max()
        .expect("a non-empty list has a widest line");
    let width = match natural <= PREFERRED_WIDTH {
        true => natural.max(MIN_WIDTH),
        false => PREFERRED_WIDTH,
    };
    let mut laid = lay_out(&shown, faces, measure, icon.is_some(), width);
    let cap = (surface.height as f32 * HEIGHT_SHARE) as i32;
    if laid.height > cap && width < MAX_WIDTH {
        laid = lay_out(&shown, faces, measure, icon.is_some(), MAX_WIDTH);
    }
    let at = anchor(cursor, laid.width, laid.height, surface);
    Some(Card {
        at,
        width: laid.width,
        height: laid.height,
        icon: icon.map(|icon| {
            Picture {
                at: icon.at.offset(at),
                ..icon
            }
        }),
        lines: laid
            .lines
            .into_iter()
            .map(|line| {
                CardLine {
                    at: line.at.offset(at),
                    ..line
                }
            })
            .collect(),
        divider: laid.divider.map(|y| y + at.y),
    })
}

/// Where a card of this size goes, given where the pointer is.
///
/// Four anchors around the cursor, tried in this order: down-right, down-left,
/// up-right, up-left. The first one that is wholly inside `surface` — less
/// [`SCREEN_MARGIN`] on every side — wins; when none is, the least-overflowing
/// one is clamped into the surface instead. Down-right first because that is
/// where a pointer's own hotspot leaves room, and the flips are what keep a
/// card at the bottom-right corner of the window on the screen.
///
/// One `min_by_key` covers both rules: a candidate that fits overflows by zero,
/// `min_by_key` keeps the **first** of several equal minimums, and so the
/// preference order above is the tie-break for free.
#[must_use]
pub fn anchor(cursor: GumpPixel, width: i32, height: i32, surface: Surface) -> GumpPixel {
    let min_x = surface.at.x + SCREEN_MARGIN;
    let min_y = surface.at.y + SCREEN_MARGIN;
    let max_x = surface.at.x + surface.width - SCREEN_MARGIN - width;
    let max_y = surface.at.y + surface.height - SCREEN_MARGIN - height;
    let right = cursor.x + POINTER_GAP.x;
    let left = cursor.x - POINTER_GAP.x - width;
    let below = cursor.y + POINTER_GAP.y;
    let above = cursor.y - POINTER_GAP.y - height;
    let candidates = [
        GumpPixel::new(right, below),
        GumpPixel::new(left, below),
        GumpPixel::new(right, above),
        GumpPixel::new(left, above),
    ];
    let overflow = |at: GumpPixel| {
        (min_x - at.x).max(0) + (at.x - max_x).max(0) + (min_y - at.y).max(0) + (at.y - max_y).max(0)
    };
    let best = candidates
        .into_iter()
        .min_by_key(|at| overflow(*at))
        .expect("four candidates");
    // `max_x` is below `min_x` for a card wider than the surface it is being
    // clamped into, and `clamp` panics on an inverted range. The card is then
    // pinned to the near edge, which is the most of it that can be read.
    GumpPixel::new(
        best.x.clamp(min_x, max_x.max(min_x)),
        best.y.clamp(min_y, max_y.max(min_y)),
    )
}

/// Which face row `row` of the shown list is set in.
const fn font_of(row: usize, faces: Faces) -> Font {
    match row {
        0 => faces.title,
        _ => faces.body,
    }
}

/// A card laid out at one width, in its own coordinates with its corner at the
/// origin.
struct Laid {
    lines:   Vec<CardLine>,
    divider: Option<i32>,
    width:   i32,
    height:  i32,
}

/// Wrap every row into `width` and stack them, head over body.
///
/// The returned width is what the rows actually took rather than the `width`
/// they were wrapped to, floored at [`MIN_WIDTH`]: wrapping decides where the
/// text breaks, and a card left as wide as the box it was wrapped in would have
/// an empty margin on the right of every short list.
fn lay_out(shown: &[&str], faces: Faces, measure: &Measure<'_>, has_icon: bool, width: i32) -> Laid {
    let head_left = PADDING
        + match has_icon {
            true => ICON + PADDING,
            false => 0,
        };
    let head_width = width - head_left - PADDING;
    let body_width = width - 2 * PADDING;
    let head_rows = shown.len().min(1 + HEADER_LINES);
    let mut lines: Vec<CardLine> = Vec::new();
    let mut widest_head = 0;
    let mut widest_body = 0;
    let mut y = PADDING;
    for (row, text) in shown[..head_rows].iter().enumerate() {
        let font = font_of(row, faces);
        let step = measure.line_step(font);
        for piece in wrap(text, font, measure, head_width) {
            widest_head = widest_head.max(measure.width(&piece, font));
            lines.push(CardLine {
                at: GumpPixel::new(head_left, y),
                text: piece,
                font,
                hue: INK,
            });
            y += step;
        }
    }
    // The head is at least as tall as the icon beside it, or a one-line name
    // would leave the picture hanging out of the bottom of the card.
    if has_icon {
        y = y.max(PADDING + ICON);
    }
    let mut divider = None;
    if head_rows < shown.len() {
        divider = Some(y + HEAD_GAP);
        y += HEAD_GAP + 1 + HEAD_GAP;
        let font = faces.body;
        let step = measure.line_step(font);
        for text in &shown[head_rows..] {
            for piece in wrap(text, font, measure, body_width) {
                widest_body = widest_body.max(measure.width(&piece, font));
                lines.push(CardLine {
                    at: GumpPixel::new(PADDING, y),
                    text: piece,
                    font,
                    hue: INK,
                });
                y += step;
            }
        }
    }
    let needed = (head_left + widest_head + PADDING).max(2 * PADDING + widest_body);
    Laid {
        lines,
        divider,
        // `min` after `max` rather than `clamp`: a surface narrower than
        // `MIN_WIDTH` would invert the range and `clamp` panics on one.
        width: needed.max(MIN_WIDTH).min(width),
        height: y + PADDING,
    }
}

/// Break `text` into rows no wider than `width`, at word boundaries.
///
/// A word wider than the whole row is broken where it stops fitting rather than
/// left to run past the border: nothing here crops, so an unbroken one would be
/// drawn outside the card. `text` is expected to be a resolved property line,
/// which `tooltips::lines` has already trimmed and never hands over blank; runs
/// of internal whitespace collapse to one space, which is what a wrapped
/// paragraph means by them.
fn wrap(text: &str, font: Font, measure: &Measure<'_>, width: i32) -> Vec<String> {
    if width <= 0 {
        return vec![text.to_owned()];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = match line.is_empty() {
            true => word.to_owned(),
            false => format!("{line} {word}"),
        };
        if measure.width(&candidate, font) <= width {
            line = candidate;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        let mut rest = word;
        while measure.width(rest, font) > width {
            let cut = break_at(rest, font, measure, width);
            // Not one character fits: the row is narrower than a single glyph,
            // which is a card too small to have been laid out at all. Leave the
            // rest whole rather than loop forever over an empty prefix.
            if cut == 0 {
                break;
            }
            lines.push(rest[..cut].to_owned());
            rest = &rest[cut..];
        }
        line = rest.to_owned();
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The byte offset of the longest prefix of `word` that fits in `width`, or
/// zero when not even its first character does.
fn break_at(word: &str, font: Font, measure: &Measure<'_>, width: i32) -> usize {
    let mut fits = 0;
    for (index, char) in word.char_indices() {
        let end = index + char.len_utf8();
        if measure.width(&word[..end], font) > width {
            break;
        }
        fits = end;
    }
    fits
}

#[cfg(test)]
mod tests {
    use openshard_uofiles::color::Color16;
    use openshard_uofiles::image::Image;

    use super::*;
    use crate::atlas::GlyphKey;

    /// Every glyph the same width, so that a card's arithmetic can be stated
    /// exactly: a row of `n` characters is `n * GLYPH` gump pixels wide, and one
    /// line is `INK_HEIGHT + 2` tall.
    const GLYPH: i32 = 6;
    const INK_HEIGHT: u16 = 10;

    fn font_atlas() -> FontAtlas {
        let glyphs = (0x20_u8..0x7F).map(|char| {
            (
                GlyphKey {
                    font: BODY_FONT,
                    char,
                },
                Image::new(
                    GLYPH as u16,
                    INK_HEIGHT,
                    vec![Color16(0x7FFF); GLYPH as usize * usize::from(INK_HEIGHT)],
                ),
            )
        });
        FontAtlas::pack(glyphs).expect("one small block per printable byte fits an atlas")
    }

    /// The step this atlas's lines are drawn at.
    const STEP: i32 = INK_HEIGHT as i32 + 2;

    fn surface() -> Surface {
        Surface {
            at:     GumpPixel::new(0, 0),
            width:  1024,
            height: 768,
        }
    }

    fn lines(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|text| (*text).to_owned()).collect()
    }

    fn card_of(atlas: &FontAtlas, texts: &[String], phase: Phase, cursor: GumpPixel) -> Option<Card> {
        card(
            Content {
                lines: texts,
                graphic: None,
                phase,
            },
            Faces::client(),
            &Measure::Bitmap { atlas, magnify: 1.0 },
            &GumpAtlas::empty(),
            cursor,
            surface(),
        )
    }

    /// The card the design is about: a title, the head, a rule, and the
    /// properties under it — from one property list, in one pass.
    #[test]
    fn a_full_list_becomes_a_head_a_rule_and_a_body() {
        let atlas = font_atlas();
        let texts = lines(&[
            "a longsword",
            "Exceptional",
            "crafted by Alys",
            "Slayer: ogre",
            "Damage bonus",
        ]);
        let card = card_of(&atlas, &texts, Phase::Detail, GumpPixel::new(100, 100))
            .expect("a list with lines draws a card");
        assert_eq!(
            card.lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            vec![
                "a longsword",
                "Exceptional",
                "crafted by Alys",
                "Slayer: ogre",
                "Damage bonus",
            ],
            "every property, in the order the shard wrote them"
        );
        assert_eq!(card.lines[0].font, TITLE_FONT, "the first line is the title");
        let divider = card
            .divider
            .expect("a list with a body has a rule under its head");
        let head_bottom = card.lines[2].at.y + STEP;
        assert_eq!(divider, head_bottom + HEAD_GAP);
        assert_eq!(
            card.lines[3].at.y,
            divider + 1 + HEAD_GAP,
            "the body starts below the rule"
        );
        assert_eq!(
            card.lines[3].at.x,
            card.at.x + PADDING,
            "and at the card's own edge rather than the head's indent"
        );
    }

    /// A short hover answers "what is this" and no more; staying opens the rest.
    /// Both are drawn from the same list — the phase changes what is shown, not
    /// what was asked for.
    #[test]
    fn the_compact_card_is_the_title_and_one_property() {
        let atlas = font_atlas();
        let texts = lines(&["a longsword", "Exceptional", "crafted by Alys", "Slayer: ogre"]);
        let compact = card_of(&atlas, &texts, Phase::Compact, GumpPixel::new(100, 100))
            .expect("a list with lines draws a card");
        assert_eq!(
            compact
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            vec!["a longsword", "Exceptional"]
        );
        assert_eq!(compact.divider, None, "nothing to divide off yet");
        let detail = card_of(&atlas, &texts, Phase::Detail, GumpPixel::new(100, 100))
            .expect("a list with lines draws a card");
        assert_eq!(detail.lines.len(), 4);
        assert!(detail.height > compact.height, "opening it makes it taller");
    }

    /// An object that says nothing draws no card — the case a missing cliloc
    /// table produces, where the alternative is a frame with nothing in it.
    #[test]
    fn a_list_with_no_lines_draws_nothing() {
        let atlas = font_atlas();
        assert!(card_of(&atlas, &[], Phase::Detail, GumpPixel::new(100, 100)).is_none());
    }

    /// The defect the whole placement half exists to prevent: a card that runs
    /// off the screen because it was anchored before it was measured.
    #[test]
    fn a_card_at_every_corner_stays_on_the_surface() {
        let atlas = font_atlas();
        let texts = lines(&[
            "a longsword",
            "Exceptional",
            "crafted by Alys",
            "Slayer: ogre (+25% damage)",
            "Damage bonus: +3 to +7",
        ]);
        let surface = surface();
        for corner in [
            GumpPixel::new(0, 0),
            GumpPixel::new(surface.width - 1, 0),
            GumpPixel::new(0, surface.height - 1),
            GumpPixel::new(surface.width - 1, surface.height - 1),
            GumpPixel::new(surface.width / 2, surface.height - 1),
        ] {
            let card = card_of(&atlas, &texts, Phase::Detail, corner).expect("a card");
            assert!(
                card.at.x >= surface.at.x + SCREEN_MARGIN
                    && card.at.y >= surface.at.y + SCREEN_MARGIN
                    && card.at.x + card.width <= surface.at.x + surface.width - SCREEN_MARGIN
                    && card.at.y + card.height <= surface.at.y + surface.height - SCREEN_MARGIN,
                "a {}x{} card at {corner:?} landed at {:?}",
                card.width,
                card.height,
                card.at
            );
        }
    }

    /// Down-right unless it does not fit, and then the flip that does — which
    /// is what keeps the object itself visible rather than covered.
    #[test]
    fn the_four_anchors_are_tried_in_order() {
        let surface = Surface {
            at:     GumpPixel::new(0, 0),
            width:  400,
            height: 300,
        };
        assert_eq!(
            anchor(GumpPixel::new(100, 100), 200, 100, surface),
            GumpPixel::new(114, 118),
            "down-right when it fits"
        );
        assert_eq!(
            anchor(GumpPixel::new(300, 100), 200, 100, surface),
            GumpPixel::new(86, 118),
            "down-left at the right edge"
        );
        assert_eq!(
            anchor(GumpPixel::new(100, 250), 200, 100, surface),
            GumpPixel::new(114, 132),
            "up-right at the bottom"
        );
        assert_eq!(
            anchor(GumpPixel::new(300, 250), 200, 100, surface),
            GumpPixel::new(86, 132),
            "up-left in the corner"
        );
    }

    /// A card bigger than the space it is placed in is pinned to the near edge
    /// rather than panicking on an inverted clamp.
    #[test]
    fn a_card_larger_than_its_surface_is_pinned_to_the_margin() {
        let surface = Surface {
            at:     GumpPixel::new(40, 20),
            width:  100,
            height: 80,
        };
        assert_eq!(
            anchor(GumpPixel::new(60, 40), 300, 300, surface),
            GumpPixel::new(52, 32),
            "the surface's own corner plus the margin"
        );
    }

    /// The card offsets its origin into the surface it was given, so a docked
    /// panel's viewport is honoured rather than the whole window.
    #[test]
    fn placement_is_relative_to_the_given_surface() {
        assert_eq!(
            anchor(
                GumpPixel::new(0, 0),
                200,
                100,
                Surface {
                    at:     GumpPixel::new(300, 50),
                    width:  400,
                    height: 300,
                },
            ),
            GumpPixel::new(312, 62),
            "clamped to the viewport's corner, not the window's"
        );
    }

    /// A property longer than the card breaks between words, and every piece of
    /// it is still drawn: cropping would tell the player the item has a shorter
    /// property than it has.
    #[test]
    fn a_long_property_wraps_and_keeps_every_word() {
        let atlas = font_atlas();
        let long = "slayer of every ogre and ettin and troll that ever walked the shard at once";
        let texts = lines(&["a longsword", "Exceptional", "crafted by Alys", long]);
        let card = card_of(&atlas, &texts, Phase::Detail, GumpPixel::new(100, 100)).expect("a card");
        let body: Vec<&str> = card.lines[3..].iter().map(|line| line.text.as_str()).collect();
        assert!(body.len() > 1, "it did not fit on one row: {body:?}");
        assert_eq!(body.join(" "), long, "every word survived the wrap");
        for row in &card.lines {
            let width = GLYPH * row.text.chars().count() as i32;
            assert!(
                row.at.x + width <= card.at.x + card.width - PADDING,
                "row {:?} runs past the card's padding",
                row.text
            );
        }
    }

    /// A word with no break in it is cut where it stops fitting rather than
    /// drawn out through the border — there is no cropping in this pass.
    #[test]
    fn a_word_wider_than_the_card_is_broken() {
        let atlas = font_atlas();
        let word = "a".repeat(200);
        let texts = lines(&["a longsword", "Exceptional", "crafted by Alys", &word]);
        let card = card_of(&atlas, &texts, Phase::Detail, GumpPixel::new(100, 100)).expect("a card");
        let body: Vec<&str> = card.lines[3..].iter().map(|line| line.text.as_str()).collect();
        assert!(body.len() > 1, "one unbreakable word stayed on one row");
        assert_eq!(body.concat(), word, "and not a character of it was dropped");
    }

    /// The three widths are a range and not a single number: a short list
    /// shrinks to its own content but never below the floor, and a long one
    /// wraps at the preferred width rather than growing without limit.
    #[test]
    fn the_card_shrinks_to_its_content_but_not_below_the_floor() {
        let atlas = font_atlas();
        let short = lines(&["cup"]);
        let card = card_of(&atlas, &short, Phase::Detail, GumpPixel::new(100, 100)).expect("a card");
        assert_eq!(card.width, MIN_WIDTH, "a three-letter name is still a card");

        let wide = lines(&[&"a".repeat(100), &"b".repeat(100)]);
        let card = card_of(&atlas, &wide, Phase::Detail, GumpPixel::new(100, 100)).expect("a card");
        assert!(
            card.width <= PREFERRED_WIDTH,
            "a long list wraps rather than growing: {}",
            card.width
        );
    }

    /// The frame is a plate under a plate, so the border is what shows round
    /// the fill — and the rule is drawn over both.
    #[test]
    fn the_furniture_is_a_border_a_fill_and_a_rule() {
        let atlas = font_atlas();
        let texts = lines(&["a longsword", "Exceptional", "crafted by Alys", "Slayer: ogre"]);
        let card = card_of(&atlas, &texts, Phase::Detail, GumpPixel::new(100, 100)).expect("a card");
        let plates = card.plates();
        assert_eq!(plates.len(), 3);
        assert_eq!(plates[0].rect.x, card.at.x as f32);
        assert_eq!(plates[0].rect.width, card.width as f32);
        assert_eq!(plates[1].rect.x, card.at.x as f32 + 1.0);
        assert_eq!(
            plates[1].rect.width,
            card.width as f32 - 2.0,
            "the fill leaves one pixel of border showing"
        );
        assert_eq!(plates[2].rect.height, 1.0, "the rule is one pixel");

        let plain =
            card_of(&atlas, &lines(&["cup"]), Phase::Detail, GumpPixel::new(100, 100)).expect("a card");
        assert_eq!(plain.plates().len(), 2, "no body, no rule");
    }
}
