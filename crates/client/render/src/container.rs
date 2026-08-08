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
use openshard_protocol::wire::Graphic;

use crate::gump::{GumpArt, GumpAtlas, GumpPixel, Picture};

/// Every picture a container window needs packed before it can be laid out.
///
/// Asked for before [`window`] rather than derived inside it for the reason
/// [`crate::gump::art_of`] gives: what is drawn depends on how big the pictures
/// turned out to be, and an atlas grown on the frame *after* the window opened
/// would draw it empty once.
pub fn art_of(gump: Graphic, contents: &[ContainedItem]) -> Vec<GumpArt> {
    let mut wanted = Vec::with_capacity(contents.len() + 1);
    wanted.push(GumpArt::Gump(gump));
    wanted.extend(contents.iter().map(|item| GumpArt::Item(item.graphic)));
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
    let mut pictures = Vec::with_capacity(contents.len() + 1);
    pictures.push(Picture::plain(GumpArt::Gump(gump), at));
    for item in contents {
        pictures.push(
            Picture::plain(
                GumpArt::Item(item.graphic),
                at.offset(GumpPixel::new(item.at.x, item.at.y)),
            )
            .hued(item.hue),
        );
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
    contents.get(index.checked_sub(1)?).copied()
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
        ])
        .expect("three small blocks fit an atlas 2048 on a side")
    }

    fn item(serial: u32, graphic: Graphic, x: i32, y: i32) -> ContainedItem {
        ContainedItem {
            serial: Serial::new(serial).unwrap(),
            graphic,
            amount: 1,
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
