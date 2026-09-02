//! A container's window: one gump picture, with the world's own art laid on it.
//!
//! The simplest window this client draws, and deliberately not a
//! [`layout`](openshard_protocol::gump::layout) at all. A `0xB0` dialog is a
//! program — pages, buttons, switches, text the server wrote — and a container
//! is none of that: the shard sends a `0x24` naming one gump and a `0x3C`
//! listing items with a coordinate each, and the client's whole job is to put
//! the icons where it was told.
//!
//! # Three things this is not
//!
//! - **Not a `resizepic`.** A `0x24` names one graphic and the art's own size
//!   *is* the window's, so there is no rectangle to nine-slice and nothing to
//!   ask a layout engine about. See [`size`].
//! - **Not the item's world sprite.** The icons come from `art.mul`, the same
//!   file the ground draws statics out of, but with no camera, no depth and no
//!   tile: a gump pixel is a gump pixel — see [`GumpArt::Item`].
//! - **Not sorted.** The shard's order is painter's order, which is what the
//!   reference client does: icons in a bag overlap, and re-sorting them here
//!   would put a different one on top than the server's own client shows.

use openshard_protocol::containers::ContainedItem;
use openshard_protocol::items::ItemAmount;
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::Graphic;
use openshard_tiles::TileData;

use crate::atlas::FontAtlas;
use crate::gump::{
    GumpArt,
    GumpAtlas,
    GumpPixel,
    Picture,
};
use crate::items::{
    HIGHLIGHT_HUE,
    displayed_graphic,
};

/// The compact client-side control shown beneath a loot container.
///
/// A container's gump is supplied by the shard and has no spare layout field
/// for client actions. Keeping this control just below that art makes it
/// available for every chest and corpse without guessing at free pixels inside
/// the many different container backgrounds.
///
/// No brackets around the words any more: they were there to make a line of
/// text read as something pressable, back when the text *was* the control.
/// [`ACTION_UP`] is a button, so the caption is free to be a caption.
pub const TAKE_ALL_LABEL: &str = "Take all";
/// Caption for compacting like piles through ordinary client drag packets.
pub const STACK_ALL_LABEL: &str = "Stack all";

/// The face a bag's client-side action is drawn in while nothing is on it,
/// and the face it takes while the pointer is.
///
/// `0x0FA5`/`0x0FA7`, 30×22 — the pair a shard's own `0xB0` dialogs name as
/// `4005`/`4007`, and the only *generic* button the client ships: every
/// button on the paperdoll has its word baked into the art (`0x07D6` is
/// "OPTIONS", `0x07DF` is "SKILLS", and so on through
/// [`crate::paperdoll`]'s six), so none of them can carry a caption of ours.
///
/// This replaces `0x0836`, which was **not a plate at all**. That graphic is
/// the reference skill window's `_bottomComment` — `new GumpPic(25, Height -
/// 85, 0x0836, 0)` — and its 210×19 pixels are a picture of the sentence
/// "Left-click the button before a skill to use the skill. / Skills without
/// buttons are accessed in the world." Every bag in this client drew that
/// sentence under itself, tinted purple when the pointer rested on it. The
/// old note here, and `docs/findings.md`'s entry behind it, reasoned about
/// its *size* and never looked at its pixels.
///
/// Two faces rather than one tint: a button that answers a press by changing
/// picture is what every other button in this client and in the reference
/// does, so the hue [`crate::items::HIGHLIGHT_HUE`] used to do the job with
/// goes back to meaning what it means everywhere else — the icon under the
/// cursor.
pub const ACTION_UP: Graphic = Graphic(0x0FA5);
/// The pressed face — see [`ACTION_UP`].
pub const ACTION_DOWN: Graphic = Graphic(0x0FA7);

/// How far the caption sits from the button's right edge.
const LABEL_GAP: i32 = 4;

/// How far down the caption's own top edge sits, so a line of
/// [`crate::text`]'s face reads as centred against a 22 px button rather than
/// hanging off its top.
const LABEL_DROP: i32 = 4;

/// Which face an action button wears: the pressed one while the pointer is on
/// it.
///
/// One function rather than an `if` at each of the two call sites the old
/// tint had — the layout that draws the picture and the test that pins it.
#[must_use]
pub const fn action_face(lit: bool) -> Graphic {
    match lit {
        true => ACTION_DOWN,
        false => ACTION_UP,
    }
}

/// A rectangular client-side container action.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ActionButton {
    /// Top-left corner in gump pixels.
    pub at:   GumpPixel,
    /// Width and height of the clickable button — [`ACTION_UP`]'s own.
    pub size: (i32, i32),
}

impl ActionButton {
    /// Whether this button owns `cursor`.
    pub fn contains(self, cursor: GumpPixel) -> bool {
        let x = cursor.x - self.at.x;
        let y = cursor.y - self.at.y;
        (0..self.size.0).contains(&x) && (0..self.size.1).contains(&y)
    }

    /// Where its caption starts: clear of the button's right edge, rather
    /// than on top of it.
    ///
    /// The caption used to be written *over* the picture, because the picture
    /// was 210 px of blank-looking art. A real button is 30 px wide and has a
    /// drawing on it, so the text goes beside it — the shape every `0xB0`
    /// dialog the shard sends already has, a `button` and a `text` on the same
    /// row.
    ///
    /// Measured off [`Self::size`] rather than off [`ACTION_UP`]'s own 30×22,
    /// for [`size`]'s reason: the button's width is already a fact this value
    /// carries, and writing the art's size down a second time lets the two
    /// disagree.
    pub const fn label_at(self) -> GumpPixel {
        self.at
            .offset(GumpPixel::new(self.size.0 + LABEL_GAP, LABEL_DROP))
    }
}

/// Every picture a container window needs packed before it can be laid out.
///
/// Asked for before [`window`] rather than derived inside it for the reason
/// [`crate::gump::art_of`] gives: what is drawn depends on how big the pictures
/// turned out to be, and an atlas grown on the frame *after* the window opened
/// would draw it empty once.
pub fn art_of(gump: Graphic, contents: &[ContainedItem]) -> Vec<GumpArt> {
    let mut wanted = Vec::with_capacity(contents.len() + 1);
    wanted.push(GumpArt::Gump(gump));
    wanted.extend(
        contents
            .iter()
            .map(|item| GumpArt::Item(displayed_graphic(item.graphic, item.amount))),
    );
    wanted
}

/// How big the window is: its background art's own size, or `None` when the
/// atlas does not hold that art yet.
///
/// The window has no other size. `None` is not "zero by zero" — a caller that
/// cannot tell how big the window is cannot place it, hit-test it or drag it,
/// and drawing it at the origin would be worse than waiting the one frame the
/// atlas needs.
pub fn size(atlas: &GumpAtlas, gump: Graphic) -> Option<(i32, i32)> {
    let sprite = atlas.sprite(GumpArt::Gump(gump))?;
    Some((i32::from(sprite.width), i32::from(sprite.height)))
}

/// The part of a container gump in which item icons may live.
///
/// These are edges, not a width and height: ClassicUO's `containers.txt`
/// calls the four values `LEFT TOP RIGHT BOTTOM`, and its drop path clamps an
/// icon's top-left corner between the first pair and the second pair less the
/// icon's own size.  A container's background is deliberately larger — its
/// rim, straps, lid and minimizer are window furniture rather than inventory
/// space.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ContentBounds {
    pub left:   i32,
    pub top:    i32,
    pub right:  i32,
    pub bottom: i32,
}

impl ContentBounds {
    const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
}

/// ClassicUO's stock container item bounds, keyed by the gump named in `0x24`.
///
/// Unknown custom gumps use ClassicUO's own fallback. Keeping this table on the
/// client side is important: the protocol sends the gump id and item positions,
/// but never sends this inner rectangle.
#[must_use]
pub const fn content_bounds(gump: Graphic) -> ContentBounds {
    match gump.0 {
        0x0007 => ContentBounds::new(30, 30, 270, 170),
        0x0009 => ContentBounds::new(20, 85, 124, 196),
        0x003C | 0x775E | 0x7760 | 0x7762 | 0x9CE4 | 0x9CE5 | 0x9CE7 => ContentBounds::new(44, 65, 186, 159),
        0x003D => ContentBounds::new(29, 34, 137, 128),
        0x003E => ContentBounds::new(33, 36, 142, 148),
        0x003F => ContentBounds::new(19, 47, 182, 123),
        0x0040 => ContentBounds::new(16, 38, 152, 125),
        0x0041 => ContentBounds::new(40, 30, 139, 123),
        0x0042 | 0x0049 | 0x004A => ContentBounds::new(18, 105, 162, 178),
        0x0043 | 0x004B | 0x266A | 0x266B => ContentBounds::new(16, 51, 184, 124),
        0x0044 => ContentBounds::new(20, 10, 170, 100),
        0x0047 => ContentBounds::new(16, 10, 148, 138),
        0x0048 | 0x0051 => ContentBounds::new(16, 10, 154, 94),
        0x004C => ContentBounds::new(46, 74, 196, 184),
        0x004D => ContentBounds::new(76, 12, 140, 68),
        0x004E | 0x004F => ContentBounds::new(24, 18, 100, 152),
        0x0052 => ContentBounds::new(0, 0, 110, 62),
        0x0102 => ContentBounds::new(35, 10, 190, 95),
        0x0103 => ContentBounds::new(41, 21, 173, 104),
        0x0104..=0x0107 | 0x0109..=0x010E | 0x9CD9 => ContentBounds::new(10, 10, 160, 105),
        0x0108 => ContentBounds::new(10, 10, 160, 105),
        0x0116 => ContentBounds::new(40, 25, 140, 110),
        0x011A | 0x06D3..=0x06D6 => ContentBounds::new(10, 65, 125, 160),
        0x011B | 0x011F => ContentBounds::new(45, 10, 175, 95),
        0x011C => ContentBounds::new(37, 10, 175, 105),
        0x011D => ContentBounds::new(43, 10, 165, 110),
        0x011E => ContentBounds::new(30, 22, 263, 106),
        0x0120 => ContentBounds::new(56, 30, 160, 107),
        0x0121 => ContentBounds::new(77, 32, 162, 107),
        0x0123 => ContentBounds::new(36, 19, 111, 157),
        0x0484 => ContentBounds::new(0, 45, 175, 125),
        0x058E => ContentBounds::new(50, 150, 348, 250),
        0x06E5 | 0x06E6 => ContentBounds::new(66, 74, 306, 520),
        0x06E7 | 0x06E8 | 0x06EA | 0x9CDB | 0x9CDD | 0x9CDF | 0x9CE3 => ContentBounds::new(50, 60, 548, 308),
        0x06E9 => ContentBounds::new(60, 80, 318, 324),
        0x091A => ContentBounds::new(0, 0, 282, 230),
        0x092E => ContentBounds::new(0, 0, 282, 210),
        0x2A63 => ContentBounds::new(60, 33, 460, 348),
        0x4D0C => ContentBounds::new(25, 65, 220, 155),
        0x777A => ContentBounds::new(32, 40, 184, 116),
        _ => ContentBounds::new(44, 65, 186, 159),
    }
}

/// Clamp one icon's top-left coordinate into its container's item rectangle.
///
/// The displayed graphic matters for piles (gold, for example, changes art as
/// its amount grows). When the icon is packed, its far edge is kept inside the
/// right and bottom bounds just as ClassicUO does. Missing art still receives
/// the lower-bound correction, but is not assigned a guessed size.
#[must_use]
pub fn clamp_item_at(
    gump: Graphic,
    graphic: Graphic,
    amount: ItemAmount,
    at: openshard_protocol::gump::GumpPoint,
    atlas: &GumpAtlas,
) -> openshard_protocol::gump::GumpPoint {
    let bounds = content_bounds(gump);
    let mut x = at.x;
    let mut y = at.y;
    if let Some(sprite) = atlas.sprite(GumpArt::Item(displayed_graphic(graphic, amount))) {
        let rightmost = bounds.right - i32::from(sprite.width);
        let bottommost = bounds.bottom - i32::from(sprite.height);
        if x > rightmost {
            x = rightmost;
        }
        if y > bottommost {
            y = bottommost;
        }
    }
    // This order intentionally matches ClassicUO. It also handles an icon
    // larger than the whole content rectangle without `clamp`'s min/max panic.
    if x < bounds.left {
        x = bounds.left;
    }
    if y < bounds.top {
        y = bounds.top;
    }
    openshard_protocol::gump::GumpPoint::new(x, y)
}

/// How big an action button is: [`ACTION_UP`]'s own size, or `None` until it
/// has been packed.
///
/// Both actions share one pair of faces, so both share this — asking the
/// atlas rather than hardcoding 30×22 a second time, for [`size`]'s own
/// reason: a number written down beside the art's real size is a second
/// statement of the same shape, and the two are free to disagree the day the
/// art changes. The pressed face is the same size as the up one, so which of
/// the two is asked does not matter.
fn action_size(atlas: &GumpAtlas) -> Option<(i32, i32)> {
    size(atlas, ACTION_UP)
}

/// The client-owned "Take all" button immediately below a container.
///
/// Its position depends on the container's own art, and its size on the
/// button's — so it is absent until both have been packed.
pub fn take_all_button(atlas: &GumpAtlas, gump: Graphic, at: GumpPixel) -> Option<ActionButton> {
    let (_, height) = size(atlas, gump)?;
    Some(ActionButton {
        at:   at.offset(GumpPixel::new(0, height + 4)),
        size: action_size(atlas)?,
    })
}

/// The compact-piles button below the player's backpack.
pub fn stack_all_button(atlas: &GumpAtlas, gump: Graphic, at: GumpPixel) -> Option<ActionButton> {
    let (_, height) = size(atlas, gump)?;
    Some(ActionButton {
        at:   at.offset(GumpPixel::new(0, height + 4)),
        size: action_size(atlas)?,
    })
}

/// Lay a container out at `at`: the background, then everything in it.
///
/// The item coordinates are the shard's, measured from the background's top
/// left, which is what [`ContainedItem::at`] carries — so the whole window moves
/// by moving `at` and nothing inside it is recomputed.
///
/// An item whose art the client does not ship is skipped by
/// [`collect`](crate::gump::collect), the same way a missing gump is: a bag with
/// one unknown graphic in it draws as the bag and a gap, not as nothing. That is
/// why no atlas is needed here — placing a picture does not depend on how big it
/// turned out to be, and only [`size`] and [`pick`] ever ask.
pub fn window(gump: Graphic, contents: &[ContainedItem], at: GumpPixel) -> Vec<Picture> {
    window_highlighted(gump, contents, at, None)
}

/// Lay out a container, tinting the icon named by `highlighted` while it is
/// under the cursor.  The identity, rather than an index, survives container
/// updates that reorder or remove another item between frames.
pub fn window_highlighted(
    gump: Graphic,
    contents: &[ContainedItem],
    at: GumpPixel,
    highlighted: Option<Serial>,
) -> Vec<Picture> {
    let mut pictures = Vec::with_capacity(contents.len() + 1);
    pictures.push(Picture::plain(GumpArt::Gump(gump), at));
    for item in contents {
        let picture = Picture::plain(
            GumpArt::Item(displayed_graphic(item.graphic, item.amount)),
            at.offset(GumpPixel::new(item.at.x, item.at.y)),
        )
        .hued(if highlighted == Some(item.serial) {
            HIGHLIGHT_HUE
        } else {
            item.hue
        });
        pictures.push(picture);
    }
    pictures
}

/// [`window_highlighted`], with the client-side action button drawn as a real
/// picture rather than left as text with nothing behind it.
///
/// This is the fix for "a press on a bag over a window over another bag is
/// offered a button it cannot see"
/// (`docs/client/evidence/2026-08-17-the-pane-router.md`'s backlog):
/// once the button is a picture in this list, it is exactly as pickable and
/// exactly as occludable as the background and every icon beside it — the
/// same [`crate::gump::pick`] walk that already resolves the rest of this
/// window resolves the button too, by construction, and there is no second
/// rule left to write for it.
///
/// `action` is `(where, which face)` rather than recomputed here: the caller
/// already worked out the button's position to place the caption beside it
/// and to hit-test a press against it, and asking a second time is the exact
/// "one rule, three readers" this window's button has already cost once. The
/// face comes from [`action_face`], so what is drawn and what the pointer is
/// told it is on cannot drift.
pub fn window_with_action(
    gump: Graphic,
    contents: &[ContainedItem],
    at: GumpPixel,
    highlighted: Option<Serial>,
    action: Option<(ActionButton, Graphic)>,
) -> Vec<Picture> {
    let mut pictures = window_highlighted(gump, contents, at, highlighted);
    if let Some((button, face)) = action {
        pictures.push(Picture::plain(GumpArt::Gump(face), button.at));
    }
    pictures
}

/// Where one icon's count sits and what it reads, or `None` for an icon that
/// is drawn without one.
///
/// `at` is the icon's own top-left corner, wherever it is being drawn — a slot
/// in a bag, or the negative grab offset a pack on the cursor is placed at.
/// The three callers this has are the three places a count is drawn, and they
/// share this function rather than each doing the arithmetic: a number in one
/// corner in a bag and another corner on the pointer would be the same pile
/// drawn twice.
///
/// **The bottom-right corner of the icon's own art**, which is where a count
/// has to go: item art hangs its picture anywhere inside its rectangle, and a
/// number written at a fixed offset from the icon's *top* left would sit over
/// the drawing on a tall graphic and in empty space on a short one. Measured
/// against the sprite the atlas actually packed, so the corner is the corner
/// of the picture rather than of a rectangle guessed here.
///
/// Two atlases, because two things are being measured: `gumps` says how big
/// the icon is, and `fonts` how wide the digits are — the count is
/// right-aligned into that corner, so both are needed before it can be placed.
/// An icon the gump atlas has not packed yet gets no number, the same frame's
/// wait [`size`] describes: the count would have nothing to sit in the corner
/// of.
///
/// Whether there is a number at all is [`crate::items::stack_label`]'s one
/// rule, asked here rather than restated.
pub fn amount_label(
    graphic: Graphic,
    amount: ItemAmount,
    at: GumpPixel,
    tiledata: &TileData,
    gumps: &GumpAtlas,
    fonts: &FontAtlas,
) -> Option<(GumpPixel, String)> {
    let text = crate::items::stack_label(graphic, amount, tiledata)?;
    let icon = gumps.sprite(GumpArt::Item(displayed_graphic(graphic, amount)))?;
    // The digits' own box: as wide as the line sets and as tall as one glyph —
    // asked of `0`, since every character in it is a digit or the `.` and `k`
    // `crate::items::abbreviated` may add, and `fonts.mul`'s digits are one
    // height.
    let width = crate::text::gump_width(&text, crate::items::STACK_COUNT_FONT, fonts);
    let height = fonts
        .glyph(crate::items::STACK_COUNT_FONT, b'0')
        .map_or(0, |glyph| i32::from(glyph.height));
    Some((
        at.offset(GumpPixel::new(
            i32::from(icon.width) - width,
            i32::from(icon.height) - height,
        )),
        text,
    ))
}

/// Every counted icon's digits in this window, in the same order [`window`]
/// laid the pictures out.
///
/// The caller turns these into labels of its own, because a glyph comes out of
/// the font atlas and this module has only ever placed pictures. `at` is the
/// window's own corner, the same one [`window`] takes.
pub fn amount_labels(
    contents: &[ContainedItem],
    at: GumpPixel,
    tiledata: &TileData,
    gumps: &GumpAtlas,
    fonts: &FontAtlas,
) -> Vec<(GumpPixel, String)> {
    contents
        .iter()
        .filter_map(|item| {
            amount_label(
                item.graphic,
                item.amount,
                at.offset(GumpPixel::new(item.at.x, item.at.y)),
                tiledata,
                gumps,
                fonts,
            )
        })
        .collect()
}

/// Which item in an open container the cursor is over, if any.
///
/// Topmost first, and against the **picture** rather than its bounding box: item
/// art is mostly empty space, so a box picks whatever tall thing the cursor
/// merely sits inside — the same reason `items::pick` tests
/// [`opaque_at`](GumpAtlas::opaque_at) in the world. Later in the list wins,
/// because later is what was drawn on top.
///
/// Both of those are [`crate::gump::pick`]'s rules, and this is that walk over
/// the list [`window`] laid out — the same list the frame is drawn from, so what
/// is clicked and what is drawn cannot drift the way two separate walks would.
///
/// The background is never picked, which is why the first picture is dropped
/// from the answer rather than never offered: a click on the bag itself is a
/// click on the window, which is the caller's business — dragging it, raising it
/// — and not an item at all.
pub fn pick(
    gump: Graphic,
    contents: &[ContainedItem],
    at: GumpPixel,
    cursor: GumpPixel,
    atlas: &GumpAtlas,
) -> Option<ContainedItem> {
    let pictures = window(gump, contents, at);
    // Index zero is the background, and every one after it is `contents` in
    // order — the layout `window` built a picture at a time.
    let index = crate::gump::pick(&pictures, cursor, atlas)?;
    contents.get(index.position().checked_sub(1)?).copied()
}

#[cfg(test)]
mod tests {
    use openshard_protocol::containers::GridSlot;
    use openshard_protocol::gump::GumpPoint;
    use openshard_protocol::serial::Serial;
    use openshard_protocol::wire::Hue;
    use openshard_uofiles::color::Color16;
    use openshard_uofiles::image::Image;

    use super::*;

    /// A solid block, so that placement and picking can be tested without a
    /// client's files.
    fn block(width: u16, height: u16) -> Image {
        let pixels = vec![Color16(0x7FFF); usize::from(width) * usize::from(height)];
        Image::new(width, height, pixels)
    }

    /// The background of every window in here, and the two icons in it.
    const BAG: Graphic = Graphic(0x003C);
    const CANDLE: Graphic = Graphic(0x0A28);
    const COIN: Graphic = Graphic(0x0EED);
    /// What a coin pile of more than five draws as — see `items::displayed_graphic`.
    const COIN_PILE: Graphic = Graphic(0x0EEF);

    fn atlas() -> GumpAtlas {
        GumpAtlas::pack([
            (GumpArt::Gump(BAG), block(140, 100)),
            (GumpArt::Item(CANDLE), block(10, 20)),
            (GumpArt::Item(COIN), block(8, 8)),
            (GumpArt::Item(COIN_PILE), block(12, 10)),
            (GumpArt::Gump(ACTION_UP), block(30, 22)),
            (GumpArt::Gump(ACTION_DOWN), block(30, 22)),
        ])
        .expect("six small blocks fit an atlas 2048 on a side")
    }

    fn item(serial: u32, graphic: Graphic, x: i32, y: i32) -> ContainedItem {
        ContainedItem {
            serial: Serial::new(serial).unwrap(),
            graphic,
            amount: openshard_protocol::items::ItemAmount(1),
            at: GumpPoint::new(x, y),
            grid: GridSlot(0),
            hue: Hue::NONE,
        }
    }

    /// The window is its art's size and nothing else decides it — there is no
    /// rectangle on the wire to ask.
    #[test]
    fn a_container_window_is_exactly_as_big_as_its_background() {
        assert_eq!(size(&atlas(), BAG), Some((140, 100)));
        assert_eq!(
            size(&atlas(), Graphic(0x9999)),
            None,
            "art nobody packed has no size"
        );
    }

    /// The backpack's visible leather is not all inventory space. ClassicUO's
    /// stock bounds begin at (44, 65), end at (186, 159), and reserve enough
    /// room at the far edges for the icon itself.
    #[test]
    fn backpack_items_stay_inside_the_classic_content_bounds() {
        let atlas = atlas();
        assert_eq!(content_bounds(BAG), ContentBounds::new(44, 65, 186, 159));
        assert_eq!(
            clamp_item_at(BAG, CANDLE, ItemAmount(1), GumpPoint::new(0, 0), &atlas,),
            GumpPoint::new(44, 65),
            "the straps and upper rim are outside the item area"
        );
        assert_eq!(
            clamp_item_at(BAG, CANDLE, ItemAmount(1), GumpPoint::new(999, 999), &atlas,),
            GumpPoint::new(186 - 10, 159 - 20),
            "the whole 10x20 icon, not only its top-left pixel, stays inside"
        );
    }

    /// A pile's displayed art determines its footprint. Six coins draw the
    /// 12x10 pile rather than the base coin's 8x8 sprite.
    #[test]
    fn content_clamping_measures_the_displayed_pile() {
        assert_eq!(
            clamp_item_at(BAG, COIN, ItemAmount(6), GumpPoint::new(999, 999), &atlas(),),
            GumpPoint::new(186 - 12, 159 - 10)
        );
    }

    /// The button's own size, not a magic number written down beside it a
    /// second time — [`action_size`] asks the atlas exactly as [`size`] does
    /// for the window's own background.
    #[test]
    fn the_action_button_is_exactly_as_big_as_its_own_art() {
        assert_eq!(action_size(&atlas()), Some((30, 22)));
        assert_eq!(
            take_all_button(
                &GumpAtlas::pack([(GumpArt::Gump(BAG), block(140, 100))]).unwrap(),
                BAG,
                GumpPixel::new(0, 0)
            ),
            None,
            "the container's own art is packed but the button's is not, so there is nowhere to draw it"
        );
    }

    /// A font of solid blocks, so a count's placement can be measured without
    /// a client's `fonts.mul`. Every digit the assertions below use is five
    /// wide and seven tall.
    fn digits() -> crate::atlas::FontAtlas {
        crate::atlas::FontAtlas::pack((b'0'..=b'9').map(|char| {
            (
                crate::atlas::GlyphKey {
                    font: crate::items::STACK_COUNT_FONT,
                    char,
                },
                Image::new(5, 7, vec![Color16(0x7FFF); 5 * 7]),
            )
        }))
        .expect("ten small blocks fit an atlas 2048 on a side")
    }

    /// A table in which the coin piles up and the candle does not.
    fn stacking() -> openshard_tiles::TileData {
        let mut tiledata = openshard_tiles::TileData::empty();
        tiledata.set_static_tile(
            COIN.0,
            openshard_tiles::StaticTile {
                flags: openshard_tiles::TileFlags::new(openshard_tiles::TileFlags::STACKABLE),
                ..Default::default()
            },
        );
        tiledata
    }

    /// A count sits in the bottom-right corner of the icon's own art, and the
    /// corner is the *sprite's* — measured off the atlas, not off a rectangle
    /// written down here.
    ///
    /// **The corner is the *displayed* art's**, which for a coin pile is not
    /// the graphic the shard sent: 123 gold draws as `0x0EEF`, 12×10 here,
    /// where the single coin `0x0EED` is 8×8. A count measured against the
    /// base graphic would hang off the picture it belongs to by the difference.
    ///
    /// `123` sets 15 wide and 7 tall against the pile's 12×10, so the digits
    /// start 3 pixels left of the icon's left edge and 3 above its bottom —
    /// outside the icon on the left, which is what right-aligning a line wider
    /// than what it labels means and is why the assertion states it rather
    /// than hiding it.
    #[test]
    fn a_count_sits_in_the_corner_of_its_own_icon() {
        let mut coin = item(1, COIN, 30, 40);
        coin.amount = openshard_protocol::items::ItemAmount(123);
        let labels = amount_labels(
            &[coin, item(2, CANDLE, 60, 10)],
            GumpPixel::new(300, 200),
            &stacking(),
            &atlas(),
            &digits(),
        );
        assert_eq!(
            labels,
            vec![(
                GumpPixel::new(300 + 30 + 12 - 15, 200 + 40 + 10 - 7),
                "123".to_owned()
            )],
            "the candle is not a pile, and the coin's count is in its own corner"
        );
    }

    /// An icon whose art has not been packed yet gets no count.
    ///
    /// The same frame's wait `size` describes: the corner a count is placed in
    /// is the sprite's, and there is no sprite to take one from. A number
    /// drawn at the item's coordinate instead would sit in the middle of the
    /// bag until the atlas caught up.
    #[test]
    fn an_unpacked_icon_has_no_count() {
        let mut coin = item(1, COIN, 30, 40);
        coin.amount = openshard_protocol::items::ItemAmount(123);
        let bag_only = GumpAtlas::pack([(GumpArt::Gump(BAG), block(140, 100))]).unwrap();
        assert!(amount_labels(&[coin], GumpPixel::new(0, 0), &stacking(), &bag_only, &digits()).is_empty());
    }

    #[test]
    fn take_all_button_sits_below_the_container_and_owns_its_art() {
        let button = take_all_button(&atlas(), BAG, GumpPixel::new(300, 200))
            .expect("the bag and the button are packed");
        assert_eq!(button.at, GumpPixel::new(300, 304));
        assert_eq!(button.size, (30, 22), "the button's own art, and nothing else");
        assert!(
            button.contains(GumpPixel::new(329, 325)),
            "the far corner of the button"
        );
        assert!(!button.contains(GumpPixel::new(330, 325)));
        assert!(!button.contains(GumpPixel::new(329, 326)));
    }

    /// Both actions share one pair of faces, so the backpack's own button is
    /// exactly the same size as every other bag's — only the caption beside
    /// it differs.
    #[test]
    fn stack_all_button_uses_the_backpacks_action_slot() {
        let button = stack_all_button(&atlas(), BAG, GumpPixel::new(300, 200))
            .expect("the bag and the button are packed");
        assert_eq!(button.at, GumpPixel::new(300, 304));
        assert_eq!(button.size, (30, 22));
    }

    /// The caption clears the button rather than being written over it — the
    /// whole reason a 30 px button can carry a word at all.
    #[test]
    fn the_caption_starts_past_the_buttons_right_edge() {
        let button = take_all_button(&atlas(), BAG, GumpPixel::new(300, 200))
            .expect("the bag and the button are packed");
        assert_eq!(button.label_at(), GumpPixel::new(300 + 30 + 4, 304 + 4));
        assert!(
            !button.contains(button.label_at()),
            "a press on the caption is not a press on the button"
        );
    }

    /// Which face is drawn is a function of one thing, in one place.
    #[test]
    fn the_pressed_face_is_the_lit_one() {
        assert_eq!(action_face(false), ACTION_UP);
        assert_eq!(action_face(true), ACTION_DOWN);
        assert_ne!(ACTION_UP, ACTION_DOWN, "two faces, or there is no press to see");
    }

    /// A window whose button is drawn as a real picture is exactly as
    /// occludable as its background and its icons — the whole of the fix for
    /// "a press on a bag over a window over another bag is offered a button
    /// it cannot see".
    #[test]
    fn an_action_is_a_real_picture_at_the_buttons_own_position() {
        let at = GumpPixel::new(300, 200);
        let button = take_all_button(&atlas(), BAG, at).expect("the bag and the button are packed");
        let pictures = window_with_action(BAG, &[], at, None, Some((button, action_face(true))));
        let drawn = pictures.last().expect("background, then the button");
        assert_eq!(drawn.graphic, GumpArt::Gump(ACTION_DOWN));
        assert_eq!(
            drawn.at, button.at,
            "the same position the press is tested against"
        );
        assert_eq!(
            drawn.hue,
            Hue::NONE,
            "a button says it is pressed by changing picture, not by taking a tint"
        );
    }

    /// A shop's crate has no client-side action at all, and asking for one
    /// draws nothing rather than an empty picture.
    #[test]
    fn no_action_means_no_picture() {
        let at = GumpPixel::new(300, 200);
        let pictures = window_with_action(BAG, &[], at, None, None);
        assert_eq!(pictures.len(), 1, "the background, and nothing else");
    }

    /// Item coordinates are measured from the background's top left, so moving
    /// the window moves everything in it and nothing is recomputed.
    #[test]
    fn everything_in_a_bag_moves_with_the_bag() {
        let contents = [item(0x4000_0002, CANDLE, 20, 30)];
        let pictures = window(BAG, &contents, GumpPixel::new(300, 200));
        assert_eq!(pictures.len(), 2);
        assert_eq!(pictures[0].graphic, GumpArt::Gump(BAG));
        assert_eq!(pictures[0].at, GumpPixel::new(300, 200));
        assert_eq!(pictures[1].graphic, GumpArt::Item(CANDLE));
        assert_eq!(pictures[1].at, GumpPixel::new(320, 230));
    }

    #[test]
    fn a_gold_pile_asks_for_its_pile_art() {
        let mut gold = item(0x4000_0002, COIN, 20, 30);
        gold.amount = openshard_protocol::items::ItemAmount(6);

        assert_eq!(
            art_of(BAG, &[gold]),
            vec![GumpArt::Gump(BAG), GumpArt::Item(Graphic(0x0EEF))]
        );
        assert_eq!(
            window(BAG, &[gold], GumpPixel::new(300, 200))[1].graphic,
            GumpArt::Item(Graphic(0x0EEF)),
        );
    }

    /// The shard's order is painter's order: two icons on the same spot, and the
    /// one listed last is the one drawn on top *and* the one picked.
    #[test]
    fn the_topmost_icon_is_the_one_picked() {
        let contents = [item(0x4000_0002, CANDLE, 20, 30), item(0x4000_0003, COIN, 20, 30)];
        let picked = pick(
            BAG,
            &contents,
            GumpPixel::new(100, 100),
            GumpPixel::new(122, 132),
            &atlas(),
        );
        assert_eq!(picked.map(|item| item.serial), Some(contents[1].serial));
    }

    /// A click past an icon's own pixels is not a click on it, even inside the
    /// window: the background is not an item, and a caller gets `None` to do
    /// what it likes with.
    #[test]
    fn a_click_on_the_bag_itself_picks_nothing() {
        let contents = [item(0x4000_0002, CANDLE, 20, 30)];
        // Inside the window, well clear of the one icon in it.
        assert!(
            pick(
                BAG,
                &contents,
                GumpPixel::new(100, 100),
                GumpPixel::new(200, 180),
                &atlas()
            )
            .is_none()
        );
    }

    /// Everything the window will draw has to be packed before it is laid out,
    /// background included — see `art_of`'s docs for why it cannot be discovered
    /// on the way.
    #[test]
    fn a_window_asks_for_its_background_and_every_icon_in_it() {
        let contents = [item(0x4000_0002, CANDLE, 20, 30), item(0x4000_0003, COIN, 40, 30)];
        assert_eq!(
            art_of(BAG, &contents),
            vec![GumpArt::Gump(BAG), GumpArt::Item(CANDLE), GumpArt::Item(COIN)]
        );
    }
}
