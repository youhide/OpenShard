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

use crate::items::{HIGHLIGHT_HUE, displayed_graphic};
use openshard_protocol::containers::ContainedItem;
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::{Graphic, Hue};

use crate::gump::{GumpArt, GumpAtlas, GumpPixel, Picture};

/// The compact client-side control shown beneath a loot container.
///
/// A container's gump is supplied by the shard and has no spare layout field
/// for client actions. Keeping this control just below that art makes it
/// available for every chest and corpse without guessing at free pixels inside
/// the many different container backgrounds.
pub const TAKE_ALL_LABEL: &str = "[ Take all ]";
/// Caption for compacting like piles through ordinary client drag packets.
pub const STACK_ALL_LABEL: &str = "[ Stack all ]";

/// The plate a bag's client-side caption is written over.
///
/// Reused rather than synthesised: `docs/findings.md` recorded that no
/// *small, tightly-fitting* shipped gump matches the plate's old 72×18/80×18
/// box, and the project's resolution was to stop asking for a tight fit and
/// reuse a plate that is already shipped and already used this exact way —
/// `crates/client/render/src/skills.rs`'s own `TOTAL_PLATE`, the plate its
/// skill total is written beside. Same graphic, same role, confirmed here
/// the same way it was confirmed there: `cargo run -p openshard-uofiles
/// --example gump_probe -- 0x0836`, 210×19.
///
/// Generously sized on purpose (`docs/window_components.md`'s backlog entry
/// this closes) — a caption a third that wide has room to spare, which is
/// the point: fitting the art was never worth chasing, only covering the
/// text with real pixels was.
pub const PLATE_BACKGROUND: Graphic = Graphic(0x0836);

/// A rectangular client-side container action.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ActionButton {
    /// Top-left corner in gump pixels.
    pub at: GumpPixel,
    /// Width and height of the clickable plate.
    pub size: (i32, i32),
}

impl ActionButton {
    /// Whether this button owns `cursor`.
    pub fn contains(self, cursor: GumpPixel) -> bool {
        let x = cursor.x - self.at.x;
        let y = cursor.y - self.at.y;
        (0..self.size.0).contains(&x) && (0..self.size.1).contains(&y)
    }

    /// Where its caption starts.
    pub const fn label_at(self) -> GumpPixel {
        self.at.offset(GumpPixel::new(3, 2))
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

/// How big the plate is: [`PLATE_BACKGROUND`]'s own size, or `None` until it
/// has been packed.
///
/// Both plates share one graphic, so both share this — asking the atlas
/// rather than hardcoding 210×19 a second time, for [`size`]'s own reason: a
/// number written down beside the art's real size is a second statement of
/// the same shape, and the two are free to disagree the day the art changes.
fn plate_size(atlas: &GumpAtlas) -> Option<(i32, i32)> {
    size(atlas, PLATE_BACKGROUND)
}

/// The client-owned "Take all" plate immediately below a container.
///
/// Its position depends on the container's own art, and its size on the
/// plate's — so it is absent until both have been packed.
pub fn take_all_button(atlas: &GumpAtlas, gump: Graphic, at: GumpPixel) -> Option<ActionButton> {
    let (_, height) = size(atlas, gump)?;
    Some(ActionButton {
        at: at.offset(GumpPixel::new(0, height + 4)),
        size: plate_size(atlas)?,
    })
}

/// The compact-piles plate below the player's backpack.
pub fn stack_all_button(atlas: &GumpAtlas, gump: Graphic, at: GumpPixel) -> Option<ActionButton> {
    let (_, height) = size(atlas, gump)?;
    Some(ActionButton {
        at: at.offset(GumpPixel::new(0, height + 4)),
        size: plate_size(atlas)?,
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

/// [`window_highlighted`], with the client-side plate drawn as a real
/// picture rather than left as text with nothing behind it.
///
/// This is the fix for "a press on a bag over a window over another bag is
/// offered a plate it cannot see" (`docs/window_components.md`'s backlog):
/// once the plate is a picture in this list, it is exactly as pickable and
/// exactly as occludable as the background and every icon beside it — the
/// same [`crate::gump::pick`] walk that already resolves the rest of this
/// window resolves the plate too, by construction, and there is no second
/// rule left to write for it.
///
/// `plate` is `(where, what tint)` rather than recomputed here: the caller
/// already worked out the button's position to draw the caption over it and
/// to hit-test a press against it, and asking a second time is the exact
/// "one rule, three readers" this window's plate has already cost once.
pub fn window_with_plate(
    gump: Graphic,
    contents: &[ContainedItem],
    at: GumpPixel,
    highlighted: Option<Serial>,
    plate: Option<(ActionButton, Hue)>,
) -> Vec<Picture> {
    let mut pictures = window_highlighted(gump, contents, at, highlighted);
    if let Some((button, hue)) = plate {
        pictures.push(Picture::plain(GumpArt::Gump(PLATE_BACKGROUND), button.at).hued(hue));
    }
    pictures
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

    fn atlas() -> GumpAtlas {
        GumpAtlas::pack([
            (GumpArt::Gump(BAG), block(140, 100)),
            (GumpArt::Item(CANDLE), block(10, 20)),
            (GumpArt::Item(COIN), block(8, 8)),
            (GumpArt::Gump(PLATE_BACKGROUND), block(210, 19)),
        ])
        .expect("four small blocks fit an atlas 2048 on a side")
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

    /// The plate's own size, not a magic number written down beside it a
    /// second time — [`plate_size`] asks the atlas exactly as [`size`] does
    /// for the window's own background.
    #[test]
    fn the_plate_is_exactly_as_big_as_its_own_art() {
        assert_eq!(plate_size(&atlas()), Some((210, 19)));
        assert_eq!(
            take_all_button(
                &GumpAtlas::pack([(GumpArt::Gump(BAG), block(140, 100))]).unwrap(),
                BAG,
                GumpPixel::new(0, 0)
            ),
            None,
            "the container's own art is packed but the plate's is not, so there is nowhere to draw it"
        );
    }

    #[test]
    fn take_all_button_sits_below_the_container_and_owns_its_plate() {
        let button = take_all_button(&atlas(), BAG, GumpPixel::new(300, 200))
            .expect("the bag and the plate are packed");
        assert_eq!(button.at, GumpPixel::new(300, 304));
        assert_eq!(
            button.size,
            (210, 19),
            "the plate's own size, generous over the caption"
        );
        assert!(
            button.contains(GumpPixel::new(509, 322)),
            "the far corner of the plate"
        );
        assert!(!button.contains(GumpPixel::new(510, 322)));
        assert!(!button.contains(GumpPixel::new(509, 323)));
    }

    /// Both plates share one graphic now, so the backpack's own is exactly
    /// the same size as every other bag's — only the caption on it differs.
    #[test]
    fn stack_all_button_uses_the_backpacks_action_slot() {
        let button = stack_all_button(&atlas(), BAG, GumpPixel::new(300, 200))
            .expect("the bag and the plate are packed");
        assert_eq!(button.at, GumpPixel::new(300, 304));
        assert_eq!(button.size, (210, 19));
    }

    /// A window whose plate is drawn as a real picture is exactly as
    /// occludable as its background and its icons — the whole of the fix for
    /// "a press on a bag over a window over another bag is offered a plate
    /// it cannot see".
    #[test]
    fn a_plate_is_a_real_picture_at_the_buttons_own_position() {
        let at = GumpPixel::new(300, 200);
        let button = take_all_button(&atlas(), BAG, at).expect("the bag and the plate are packed");
        let pictures = window_with_plate(BAG, &[], at, None, Some((button, HIGHLIGHT_HUE)));
        let plate = pictures.last().expect("background, then the plate");
        assert_eq!(plate.graphic, GumpArt::Gump(PLATE_BACKGROUND));
        assert_eq!(
            plate.at, button.at,
            "the same position the caption is written over"
        );
        assert_eq!(plate.hue, HIGHLIGHT_HUE, "tinted while the pointer is on it");
    }

    /// A shop's crate has no client-side plate at all, and asking for one
    /// draws nothing rather than an empty picture.
    #[test]
    fn no_plate_means_no_picture() {
        let at = GumpPixel::new(300, 200);
        let pictures = window_with_plate(BAG, &[], at, None, None);
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
