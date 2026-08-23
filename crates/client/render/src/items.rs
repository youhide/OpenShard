//! Placing what the server has dropped on the ground.
//!
//! The fourth CPU-side collector, and the one that is two of the others put
//! together: an item's *picture* is a static's — the same art, the same atlas,
//! the same "centred on the column, standing on the diamond's bottom vertex" —
//! while its *source* is a mobile's, a list somebody else built out of what
//! arrived on the wire. Nothing in the map says an item is there; a `0x1A` does,
//! and a `0x1D` takes it away again.
//!
//! So the placement is [`crate::statics::stand_on`] rather than a second copy of
//! it, and what is written here is only the part that differs: where the list
//! comes from, and that it is sorted for the depth buffer.
//!
//! # Not the same thing as a static at the same tile
//!
//! Both draw through [`StaticAtlas`], and a client that merged the two lists
//! would be right until an item is picked up: the map's statics are the shard's
//! furniture and never move, while these come and go with every `0x1A`. Two
//! lists, one atlas.

use std::collections::BTreeSet;

use openshard_protocol::items::ItemAmount;
use openshard_protocol::speech::Font;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Point;
use openshard_tiles::TileData;

use crate::animate::StaticAnimations;
use crate::atlas::StaticArt;
#[cfg(test)]
use crate::atlas::StaticAtlas;
use crate::camera::{Camera, RealPixel, ViewPixel};
use crate::cutaway::Cutaway;
use crate::depth;
use crate::sprite::SpriteQuad;
use crate::statics::{Placed, on_screen, place_cutaway, placed_rect, quad_of};

/// One thing lying on the ground, as the client has been told about it.
///
/// A plain value and not a handle into a `WorldView`, for the reason
/// [`Mobile`](crate::mobiles::Mobile) is one: this crate renders what it is
/// given and owns no model of the world.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GroundItem {
    /// Where it lies.
    pub at: Point,
    /// Its graphic, which is a static's graphic: the two share an art file and
    /// therefore an atlas.
    ///
    /// **The one the shard sent**, not the one on screen. A pile of coins is
    /// drawn from a different graphic once there are two of it and again once
    /// there are six — see [`displayed_graphic`] — and [`GroundItem::displayed`]
    /// is where that choice is made, every time this list is drawn, picked or
    /// packed. Storing the chosen one here instead was a real defect: the
    /// client asks `tiledata` about a graphic to decide whether a pile is
    /// counted, and copper's pile art (`0x0EEB`) does not carry the stacking
    /// flag its single coin (`0x0EEA`) does — so the same handful of coppers
    /// was counted in a bag, where the base graphic survives, and silent on
    /// the floor, where it did not.
    pub graphic: Graphic,
    /// Its hue, or [`Hue::NONE`] for none.
    pub hue: Hue,
    /// How many of it there are.
    ///
    /// This used to be deliberately absent, on the grounds that a pile of 500
    /// gold is one sprite and picking that sprite is the caller's question. It
    /// still is — [`displayed_graphic`] runs before a [`GroundItem`] is ever
    /// built. What changed is that a pile is now drawn with its count written
    /// over it, and a number the picture carries is part of the picture: see
    /// [`stack_label`], and [`labels`] for the anchor it hangs from. It also
    /// picks the art, through [`Self::displayed`]. One for everything that is
    /// not a pile at all, which is what the wire's own default is.
    pub amount: ItemAmount,
}

impl GroundItem {
    /// The graphic actually on screen: [`displayed_graphic`] of what the shard
    /// sent and how many there are.
    ///
    /// Everything in this module that draws, places, packs or picks this item
    /// goes through here, so there is one answer to "which picture is this" —
    /// the atlas is grown for it, the sprite is placed from it, and a click is
    /// tested against it. What is deliberately **not** asked through it is
    /// whether the pile is counted at all: that reads the shard's own graphic,
    /// for the reason [`Self::graphic`] states.
    #[must_use]
    pub const fn displayed(&self) -> Graphic {
        displayed_graphic(self.graphic, self.amount)
    }
}

/// The art used for a counted coin stack.
///
/// The wire keeps the currency's base graphic (`0x0EED` for gold) and its
/// amount separate. Like the classic client, choose the two-coin art for a
/// small stack and the pile art once it holds more than five. This belongs at
/// the client boundary: the server retains the base graphic, so identical
/// coins stack regardless of how they are drawn.
#[must_use]
pub const fn displayed_graphic(graphic: Graphic, amount: ItemAmount) -> Graphic {
    match graphic.0 {
        0x0EEA | 0x0EED | 0x0EF0 if amount.0 > 5 => Graphic(graphic.0 + 2),
        0x0EEA | 0x0EED | 0x0EF0 if amount.0 > 1 => Graphic(graphic.0 + 1),
        _ => graphic,
    }
}

/// The face a stack's count is written in.
///
/// `fonts.mul`'s narrow sans-serif face. It stays legible in an icon corner
/// without the serifs that make the ordinary caption face blur into the art.
/// One constant for all three places a count is drawn, for [`stack_label`]'s
/// reason: a pile counted in one face in a bag and another on the floor is
/// the same pile drawn twice.
pub const STACK_COUNT_FONT: Font = Font(9);

/// The digits written on a pile, or `None` for a thing that is drawn without a
/// count.
///
/// **The one rule, for all three places a count is drawn**: an icon in a bag,
/// a pile on the ground, and the pack on the cursor between the two. Which of
/// them draws a number is not three decisions — a pile that is counted in a bag
/// and silent on the floor would be the same pile telling two stories.
///
/// Two conditions, both from the client's own files rather than from a guess at
/// what looks like a heap:
///
/// - **The graphic stacks at all** — `tiledata`'s
///   [`STACKABLE`](openshard_tiles::TileFlags::STACKABLE). A sword
///   the shard sent with an amount is one sword, and writing `2` on it would be
///   inventing a pile out of a field the wire happens to carry.
/// - **There is more than one of it.** A single reagent is a reagent, not a
///   stack of one — which is the same threshold ClassicUO's own stack drawing
///   uses for its offset second sprite (`ItemGump.Draw`).
///
/// **This is not the reference client's picture.** No 2D client writes a count
/// on a pile at all: the classic one puts the number in the name over the item
/// (`NameOverheadGump`, "500 Gold Coins") and nowhere else, and ClassicUO draws
/// the same art a second time five pixels up and left instead. The digits are
/// this client's own addition, which is why the whole rule is stated here
/// rather than cited to a reference that does not have one.
///
/// The number is abbreviated — see [`abbreviated`] — because the widest place
/// it is drawn in is the corner of a 30-pixel icon.
#[must_use]
pub fn stack_label(graphic: Graphic, amount: ItemAmount, tiledata: &TileData) -> Option<String> {
    let stacks = tiledata.static_tile(graphic.0).flags.is_stackable();
    (stacks && amount.0 > 1).then(|| abbreviated(amount))
}

/// A stack's size, short enough to sit in the corner of an icon.
///
/// Three bands, and the number is **truncated** rather than rounded in all of
/// them, so what is written is never more than what is there — a pack reading
/// `1.0k` holds at least a thousand, and one reading `999` is not a thousand
/// rounded down to look like less.
///
/// - Under a thousand: the figure itself. `500`.
/// - Under ten thousand: one decimal. `1234` reads `1.2k`.
/// - Above that: whole thousands. `60000` reads `60k`.
///
/// There is no `m` band and there never can be: an [`ItemAmount`] is a `u16`,
/// so the largest pile the wire can describe is `65535`, which reads `65k`.
#[must_use]
pub fn abbreviated(amount: ItemAmount) -> String {
    let count = u32::from(amount.0);
    match count {
        0..1_000 => count.to_string(),
        1_000..10_000 => format!("{}.{}k", count / 1_000, (count % 1_000) / 100),
        _ => format!("{}k", count / 1_000),
    }
}

/// A position in the frame's ground-item list.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ItemIndex(usize);

impl ItemIndex {
    pub const fn new(index: usize) -> Self {
        Self(index)
    }
    /// Its position in the ground-item list it was picked from.
    pub const fn position(self) -> usize {
        self.0
    }
}

/// The hue an item is drawn in while the cursor is over it.
///
/// ClassicUO's `Constants.HIGHLIGHT_CURRENT_OBJECT_HUE`
/// (`src/ClassicUO.Client/Game/Constants.cs`), and it is a hue rather than an
/// outline because that is the only thing this pass can say: the shader has one
/// `hue` per sprite and a `hues.mul` ramp *replaces* the art's colour (see
/// [`crate::hue`]), so a highlighted barrel is drawn in the highlight ramp
/// entire — which is exactly what the reference does with `partial = false`.
///
/// Full and not partial deliberately. A partial hue leaves anything the art
/// painted in its own colour alone, so a door with a brass handle would be
/// highlighted everywhere except where the eye is looking.
pub const HIGHLIGHT_HUE: Hue = Hue(0x0014);

/// Every distinct graphic a set of items needs packed.
///
/// A `BTreeSet` and not a `Vec`, to be unioned with
/// [`statics::visible_graphics`](crate::statics::visible_graphics) before the
/// atlas is built: one atlas serves both passes, so the two sets are asked for
/// together and packed once.
///
/// An item's whole cycle, for the reason
/// [`statics::graphics_in`](crate::statics::graphics_in) asks for one: a torch
/// dropped on the ground animates exactly as a torch built into the map does,
/// and the atlas has to hold every graphic it will turn into.
pub fn needed_graphics(items: &[GroundItem], animations: &StaticAnimations) -> BTreeSet<Graphic> {
    items
        .iter()
        .flat_map(|item| animations.cycle(item.displayed()))
        .collect()
}

/// The quads for every item whose graphic the atlas holds.
///
/// One the atlas has no sprite for is dropped, exactly as in
/// [`statics::collect`](crate::statics::collect): the client ships no art for
/// it, or the atlas was built before this item arrived. Both are "nothing to
/// draw", and drawing it from a neighbouring graphic would be worse.
///
/// `cutaway` is the same one the statics are tested against, and it is the same
/// test: the client reads a ground item's tiledata row out of the static table,
/// so a barrel on the floor above is hidden with that floor exactly as a wall
/// built into the map is.
///
/// `highlight` is the index [`pick`] answered with, drawn in
/// [`HIGHLIGHT_HUE`] instead of its own — the item the cursor is over. An index
/// and not a flag on the item, because being pointed at is a fact about *this
/// frame* and not about the thing: the caller's list is a projection of what the
/// server said, and writing a highlight into it would put the cursor's position
/// inside the record of the world.
///
/// `occlusion` is this frame's own grid, already built — see
/// [`crate::statics::collect`], which takes it for the same reason and states it.
///
/// `player_mask` makes a server item subject to the same exact opaque-texel
/// candidate rule as map architecture.  A dropped blocking prop is still a
/// part of the world in front of the player, rather than an arbitrary exception
/// merely because the shard sent it after the map file.  Once selected, it is
/// rendered into the private late layer with its own G-buffer data; picking and
/// the opaque world's identity remain on their ordinary path.
// Eight, and every one of them is a different source this frame reads: the list,
// the camera, two tables, the atlas, the cutaway, the pick, the grid and the
// player's body. There is no
// pair among them that belongs in one struct — [`crate::statics::collect`] takes
// the same inputs off the map instead of off a list — so a grouping here would be
// a bag named after the argument count rather than after anything.
#[allow(clippy::too_many_arguments)]
pub fn collect(
    items: &[GroundItem],
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &dyn StaticArt,
    cutaway: &Cutaway,
    highlight: Option<ItemIndex>,
    occlusion: &crate::occlusion::Occlusion,
    player_mask: Option<&crate::mobiles::OpaqueMask>,
) -> crate::statics::StaticGeometry {
    collect_with_fades(
        items,
        camera,
        tiledata,
        animations,
        atlas,
        cutaway,
        highlight,
        occlusion,
        player_mask,
        &mut crate::cutaway::Fades::default(),
    )
}

/// [`collect`] with opacity state retained across frames by the caller.
#[allow(clippy::too_many_arguments)]
pub fn collect_with_fades(
    items: &[GroundItem],
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &dyn StaticArt,
    cutaway: &Cutaway,
    highlight: Option<ItemIndex>,
    occlusion: &crate::occlusion::Occlusion,
    player_mask: Option<&crate::mobiles::OpaqueMask>,
    fades: &mut crate::cutaway::Fades,
) -> crate::statics::StaticGeometry {
    collect_with_fades_with_interior(
        items,
        camera,
        tiledata,
        animations,
        atlas,
        cutaway,
        highlight,
        occlusion,
        player_mask,
        fades,
        None,
    )
}

/// [`collect_with_fades`] with the current building-cell picture gate.
#[allow(clippy::too_many_arguments)]
pub fn collect_with_fades_with_interior(
    items: &[GroundItem],
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &dyn StaticArt,
    cutaway: &Cutaway,
    highlight: Option<ItemIndex>,
    occlusion: &crate::occlusion::Occlusion,
    player_mask: Option<&crate::mobiles::OpaqueMask>,
    fades: &mut crate::cutaway::Fades,
    interior: Option<&crate::interiors::InteriorFrame>,
) -> crate::statics::StaticGeometry {
    let (eye_x, eye_y) = camera.eye_tile();
    let base = depth::base_for(eye_x, eye_y);
    let mut quads: Vec<(depth::Order, SpriteQuad)> = Vec::new();
    let mut cutaway_quads: Vec<(depth::Order, SpriteQuad)> = Vec::new();
    let mut cutaway_boxes = Vec::new();
    // Always empty since `docs/lighting_rebuild.md` phase 6d — see
    // `crate::statics::collect`'s own comment at the same two locals.
    let mesh_vertices = Vec::new();
    let mesh_rows = Vec::new();
    let mut boxes = Vec::new();

    for (index, item) in items.iter().enumerate() {
        if !interior.is_none_or(|frame| frame.shows_at(item.at)) {
            continue;
        }
        let (placed, target) = match place(item, camera, tiledata, animations, atlas, cutaway) {
            Some(placed) => {
                if !on_screen(camera, placed.at, &placed.sprite) {
                    continue;
                }
                let overlaps_body = player_mask.is_some_and(|body| {
                    placed.order > body.order()
                        && body.overlaps_opaque(placed_rect(&placed), |x, y| {
                            atlas.opaque_at(placed.showing, x, y)
                        })
                });
                (
                    placed,
                    if overlaps_body {
                        crate::cutaway::TRANSLUCENT_ALPHA_U8
                    } else {
                        u8::MAX
                    },
                )
            }
            None => {
                // A server decoration on the storey the cutaway removed gets
                // the same late, independently lit treatment as map
                // architecture.  `place_cutaway` still rejects the absolute
                // draw ceiling, so this is not a route to resurrect distant
                // scenery that the frame deliberately omitted.
                let Some(placed) = place_cutaway(
                    item.at,
                    item.displayed(),
                    camera,
                    tiledata,
                    animations,
                    atlas,
                    cutaway,
                ) else {
                    continue;
                };
                if !on_screen(camera, placed.at, &placed.sprite) {
                    continue;
                }
                (placed, 0)
            }
        };
        let alpha = fades.advance(crate::cutaway::FadeKey::item(item.at, item.displayed()), target);
        if alpha == 0 {
            continue;
        }
        let late = alpha != u8::MAX;
        let order = placed.order;
        // The highlight replaces the item's own hue rather than combining with
        // it: one wire hue reaches the shader, and the reference does the same
        // — `ItemView.Draw` overwrites `hue` and clears `partial` when the
        // object is the selected one.
        let hue = match highlight == Some(ItemIndex::new(index)) {
            true => u32::from(HIGHLIGHT_HUE.0),
            false => u32::from(item.hue.0),
        };
        let key = crate::occlusion::Owner::new(item.at.z, item.displayed());
        let owner = occlusion.owner_at(
            i32::from(item.at.x),
            i32::from(item.at.y),
            item.at.z,
            item.displayed(),
        );
        // This item's own boxes, the same way `statics::collect` builds a map
        // static's — phase 6, and an item on the ground is a static that came
        // from the server's list rather than the map's.
        let volumes = crate::statics::push_volumes(
            match late {
                true => &mut cutaway_boxes,
                false => &mut boxes,
            },
            item.at,
            tiledata.static_tile(item.displayed().0),
            &crate::occlusion::shape_of(Some(atlas), item.displayed()),
            key,
            occlusion,
        );
        let quad = quad_of(item.at, &placed, base, hue, owner, volumes).with_opacity(alpha);
        match late {
            true => cutaway_quads.push((order, quad)),
            false => quads.push((order, quad)),
        }
    }

    // Back to front, and a *stable* sort on the order alone: two items on one
    // tile at one `PriorityZ` keep the caller's order, which is by serial —
    // the order the server sent them and so the order the client's own
    // per-tile list holds them in. The depth test is `LessEqual`, so the later
    // one wins the tie; see `renderer::depth_state`.
    quads.sort_by_key(|(order, _)| *order);
    // The late layer is alpha-composited, so preserve its stable
    // back-to-front order just as the map-static collector does.
    cutaway_quads.sort_by_key(|(order, _)| *order);
    crate::statics::StaticGeometry {
        quads: quads.into_iter().map(|(_, quad)| quad).collect(),
        cutaway_quads: cutaway_quads.into_iter().map(|(_, quad)| quad).collect(),
        cutaway_boxes,
        mesh_vertices,
        mesh_rows,
        boxes,
    }
}

/// The quads to draw a silhouette from, for the item the cursor is over.
///
/// A list rather than an `Option` because the pass takes one — and because the
/// day a second thing is highlighted (a target cursor's victim, a container's
/// contents) this is where it is appended, with no change on either side. Their
/// order in the list is the ring identity the silhouette pass numbers them by,
/// so two outlined items that overlap on screen come back ringed separately
/// rather than as one blob; see [`crate::outline`].
///
/// The quads are the *same* quads [`collect`] draws — same placement, same
/// region, same depth, through the same [`quad_of`] — because the silhouette
/// has to land exactly on the picture. The hue is the item's own and is never
/// read: the silhouette shader wants the shape, and the shape is the alpha.
///
/// `highlight` is what [`pick`] answered. `None` is the ordinary frame, and it
/// comes back empty rather than being a case the caller has to handle.
pub fn outlined(
    items: &[GroundItem],
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &dyn StaticArt,
    cutaway: &Cutaway,
    highlight: Option<ItemIndex>,
) -> Vec<SpriteQuad> {
    let (eye_x, eye_y) = camera.eye_tile();
    let base = depth::base_for(eye_x, eye_y);
    highlight
        .and_then(|index| Some((items.get(index.position())?, index)))
        .and_then(|(item, _)| {
            let placed = place(item, camera, tiledata, animations, atlas, cutaway)?;
            match on_screen(camera, placed.at, &placed.sprite) {
                true => Some(quad_of(
                    item.at,
                    &placed,
                    base,
                    u32::from(item.hue.0),
                    crate::occlusion::OwnerId::NONE,
                    // A silhouette is a mask and its colour is never read, so
                    // it is lit by nothing and needs no geometry under it —
                    // the same reason it stamps `OwnerId::NONE` beside this.
                    crate::impostor::Range::default(),
                )),
                false => None,
            }
        })
        .into_iter()
        .collect()
}

/// Place one item, or `None` when there is nothing on screen for it: hidden by
/// the cutaway, or a graphic the atlas holds no art for.
///
/// [`statics::place`](crate::statics::place) and nothing else. A ground item is
/// ordered as a static is, and from the same table — the client reads
/// `tiledata`'s *static* entry for an item's graphic too, so a wall lying on the
/// floor and a wall built into the map sort alike — so the placement is that one
/// rather than a second copy of it. See [`Placed`](crate::statics::Placed) for
/// why there is only one.
fn place(
    item: &GroundItem,
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &dyn StaticArt,
    cutaway: &Cutaway,
) -> Option<Placed> {
    // A dropped item is never a tree: foliage stands in the map's own
    // statics, not in what a player or a shard puts on the ground, so this
    // list never asks `statics::place` to cut one over the player.
    crate::statics::place(
        item.at,
        item.displayed(),
        camera,
        tiledata,
        animations,
        atlas,
        cutaway,
        None,
    )
}

/// Where each counted pile's digits hang, and what they say.
///
/// The [`crate::text::Label`] anchor for every item [`stack_label`] gives a
/// number to: the top edge of the sprite, centred across it — the exact pair
/// [`crate::mobiles::head_anchor`] answers with for a body, and for the same
/// reason. A `Label`'s anchor is its *baseline*, so a line hung from a
/// sprite's top edge stands above the picture rather than across it.
///
/// Placed through the same [`place`] every other question about this list is
/// asked through, so what is not drawn is not counted: a pile the storey cut
/// moved into the late translucent layer keeps no number — the fade is the
/// frame saying "this is behind a floor", and digits at full strength over it
/// would undo that — and one the atlas has no art for yet is silent rather
/// than a number floating over nothing. Off-screen items are dropped here
/// rather than left for the text pass to clip, the way [`collect`] drops
/// them.
///
/// Deliberately not folded into [`collect`]: that function answers in quads
/// against the *static* atlas, and a glyph comes out of the font atlas, which
/// this crate's item pass has never heard of. The caller hangs these on the
/// same overhead list a mobile's speech goes on.
pub fn labels(
    items: &[GroundItem],
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &dyn StaticArt,
    cutaway: &Cutaway,
) -> Vec<(ViewPixel, String)> {
    items
        .iter()
        .filter_map(|item| {
            let text = stack_label(item.graphic, item.amount, tiledata)?;
            let placed = place(item, camera, tiledata, animations, atlas, cutaway)?;
            if !on_screen(camera, placed.at, &placed.sprite) {
                return None;
            }
            let rect = placed_rect(&placed);
            Some((
                ViewPixel {
                    x: (rect.x + rect.width / 2.0).round() as i32,
                    y: rect.y.round() as i32,
                },
                text,
            ))
        })
        .collect()
}

/// Which item the cursor is over: an index into `items`, or `None` for none.
///
/// **The picture is what is hit, not the tile.** A door's leaf is drawn on its
/// own tile and stands two tiles up the screen from it, so a click on the leaf
/// unprojects to the tile *behind* the door — pick by tile and a player can
/// never open the door they are pointing at. That is the same reason the client
/// itself picks against the drawn sprite.
///
/// Two rules, both taken from the frame rather than restated:
///
/// - **A hit is an opaque texel**, not a bounding box — see
///   [`StaticAtlas::opaque_at`]. Static art is mostly empty space, and a box test
///   picks whatever tall thing the cursor is merely *inside*.
/// - **The topmost drawn wins.** That is the largest [`depth::Order`], and on a
///   tie the *later* item in `items`, which is exactly what the depth test does
///   to two sprites at one order — see [`collect`]'s sort.
///
/// `cursor` is a viewport pixel, the same pair `winit` reports and
/// [`Camera::pick`] takes; the zoom is undone here, once.
#[must_use]
pub fn pick(
    items: &[GroundItem],
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &dyn StaticArt,
    cutaway: &Cutaway,
    cursor: RealPixel,
) -> Option<ItemIndex> {
    pick_with_interior(items, camera, tiledata, animations, atlas, cutaway, cursor, None)
}

/// [`pick`] under the same building policy that collected this frame's items.
#[must_use]
pub fn pick_with_interior(
    items: &[GroundItem],
    camera: &Camera,
    tiledata: &TileData,
    animations: &StaticAnimations,
    atlas: &dyn StaticArt,
    cutaway: &Cutaway,
    cursor: RealPixel,
    interior: Option<&crate::interiors::InteriorFrame>,
) -> Option<ItemIndex> {
    let in_view = camera.to_view(camera.pick(cursor));
    let mut hit: Option<(depth::Order, ItemIndex)> = None;
    for (index, item) in items.iter().enumerate() {
        if !interior.is_none_or(|frame| frame.shows_at(item.at)) {
            continue;
        }
        let Some(placed) = place(item, camera, tiledata, animations, atlas, cutaway) else {
            continue;
        };
        // Into the sprite's own pixels. Negative is above or left of it, and
        // `try_from` failing is the whole of that test.
        let (Ok(x), Ok(y)) = (
            u16::try_from(in_view.x - placed.at.x as i32),
            u16::try_from(in_view.y - placed.at.y as i32),
        ) else {
            continue;
        };
        if !atlas.opaque_at(placed.showing, x, y) {
            continue;
        }
        // `>=`, so a later item at the same order takes it: the tie-break is the
        // caller's order, and the one drawn last is the one on top.
        if hit.is_none_or(|(order, _)| placed.order >= order) {
            hit = Some((placed.order, ItemIndex::new(index)));
        }
    }
    hit.map(|(_, index)| index)
}

#[cfg(test)]
mod tests {
    use openshard_uofiles::color::Color16;
    use openshard_uofiles::image::Image;

    use super::*;

    #[test]
    fn coin_stacks_use_the_classic_size_bands() {
        let gold = Graphic(0x0EED);
        assert_eq!(displayed_graphic(gold, ItemAmount(1)), gold);
        assert_eq!(displayed_graphic(gold, ItemAmount(2)), Graphic(0x0EEE));
        assert_eq!(displayed_graphic(gold, ItemAmount(5)), Graphic(0x0EEE));
        assert_eq!(displayed_graphic(gold, ItemAmount(6)), Graphic(0x0EEF));
    }

    /// A table in which `graphic` piles up and nothing else does.
    fn stacking(graphic: Graphic) -> TileData {
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            graphic.0,
            openshard_tiles::StaticTile {
                flags: openshard_tiles::TileFlags::new(openshard_tiles::TileFlags::STACKABLE),
                ..Default::default()
            },
        );
        tiledata
    }

    #[test]
    fn only_a_pile_of_a_stacking_graphic_is_counted() {
        let gold = Graphic(0x0EED);
        let sword = Graphic(0x0F5E);
        let tiledata = stacking(gold);

        assert_eq!(
            stack_label(gold, ItemAmount(500), &tiledata).as_deref(),
            Some("500")
        );
        // One of a stacking thing is one of it, not a stack of one — the same
        // threshold ClassicUO's offset second sprite uses.
        assert_eq!(stack_label(gold, ItemAmount::ONE, &tiledata), None);
        // The wire may carry an amount for anything. A sword is one sword.
        assert_eq!(stack_label(sword, ItemAmount(2), &tiledata), None);
    }

    #[test]
    fn a_count_is_truncated_rather_than_rounded() {
        // Under a thousand is the figure itself.
        assert_eq!(abbreviated(ItemAmount(2)), "2");
        assert_eq!(abbreviated(ItemAmount(999)), "999");
        // One decimal under ten thousand, and downward: `1999` is not `2.0k`,
        // which would claim a thousand more than the pile holds.
        assert_eq!(abbreviated(ItemAmount(1_000)), "1.0k");
        assert_eq!(abbreviated(ItemAmount(1_234)), "1.2k");
        assert_eq!(abbreviated(ItemAmount(1_999)), "1.9k");
        // Whole thousands above it, downward again.
        assert_eq!(abbreviated(ItemAmount(10_000)), "10k");
        assert_eq!(abbreviated(ItemAmount(60_500)), "60k");
        // The largest pile the wire can describe. There is no `m` band because
        // an `ItemAmount` is a `u16` and can never reach one.
        assert_eq!(abbreviated(ItemAmount(u16::MAX)), "65k");
    }

    #[test]
    fn a_non_coin_keeps_its_base_art() {
        assert_eq!(
            displayed_graphic(Graphic(0x0F0E), ItemAmount(100)),
            Graphic(0x0F0E)
        );
    }
    // Where a sprite of this size lands, which the assertions below are stated
    // against: the tests place a cursor over a picture they have placed
    // themselves, and this is the one arithmetic that says where that is.
    use crate::statics::stand_on;

    /// An atlas holding one graphic at a known size.
    fn atlas(graphic: Graphic, width: u16, height: u16) -> StaticAtlas {
        StaticAtlas::pack([(
            graphic,
            Image::new(
                width,
                height,
                vec![Color16(0x7C00); usize::from(width) * usize::from(height)],
            ),
        )])
        .expect("one sprite fits")
    }

    /// An item lands where a static of the same size on the same tile does.
    ///
    /// The assertion is the *comparison* rather than two numbers: the placement
    /// has one copy now, and this is what says the item pass is using it. Two
    /// numbers here would go on passing if a second copy appeared and drifted.
    #[test]
    fn an_item_stands_exactly_where_a_static_of_its_size_would() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0EED);
        let atlas = atlas(graphic, 30, 50);
        let tiledata = TileData::empty();

        let quads = collect(
            &[GroundItem {
                amount: ItemAmount::ONE,
                at: Point::new(100, 100, 0),
                graphic,
                hue: Hue::NONE,
            }],
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            None,
            &crate::occlusion::Occlusion::EMPTY,
            None,
        );
        assert_eq!(quads.quads.len(), 1);
        let sprite = atlas.sprite(graphic).expect("packed");
        let at = stand_on(&camera, Point::new(100, 100, 0), &sprite);
        assert_eq!((quads.quads[0].rect.x, quads.quads[0].rect.y), (at.x, at.y));
        assert_eq!(
            (quads.quads[0].rect.width, quads.quads[0].rect.height),
            (30.0, 50.0)
        );
    }

    /// A pile's count hangs from the top edge of its own picture, centred.
    ///
    /// The assertion is against the placement rather than two numbers, for the
    /// reason the test above states: [`labels`] must be measuring the same
    /// sprite the item pass drew, and a pair of constants here would go on
    /// passing after the two drifted apart. The anchor is a
    /// [`crate::text::Label`]'s baseline, so the top edge is what puts the
    /// digits above the picture rather than across it.
    #[test]
    fn a_pile_is_counted_above_its_own_picture() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0EED);
        let at = Point::new(100, 100, 0);
        let pile = GroundItem {
            amount: ItemAmount(500),
            at,
            graphic,
            hue: Hue::NONE,
        };
        // Packed under the art five hundred coins actually draw as, which is
        // not `graphic` — see `GroundItem::displayed`.
        let atlas = atlas(pile.displayed(), 30, 50);
        let tiledata = stacking(graphic);

        let labelled = labels(
            &[pile],
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
        );

        let sprite = atlas.sprite(pile.displayed()).expect("packed");
        let placed = stand_on(&camera, at, &sprite);
        assert_eq!(
            labelled,
            vec![(
                ViewPixel {
                    x: (placed.x + 30.0 / 2.0).round() as i32,
                    y: placed.y.round() as i32,
                },
                "500".to_owned(),
            )]
        );
    }

    /// Whether a pile is counted is asked of the *shard's* graphic, never of
    /// the art it happens to be drawing as.
    ///
    /// Not a hypothetical: a real `tiledata.mul` marks the single copper coin
    /// `0x0EEA` as stacking and its two pile graphics `0x0EEB`/`0x0EEC` as not,
    /// so a handful of coppers asked about by its drawn art answers "not a
    /// pile" the moment there are two of it — while the same coins in a bag,
    /// where the base graphic survives, answer "yes". That is one pile telling
    /// two stories, and this is the assertion that keeps it from coming back.
    #[test]
    fn a_pile_is_counted_by_the_graphic_the_shard_sent() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let copper = Graphic(0x0EEA);
        let pile_art = displayed_graphic(copper, ItemAmount(50));
        assert_ne!(pile_art, copper, "fifty coins draw as the pile art");

        // The table a client ships: the coin stacks, the pile art it draws as
        // does not.
        let tiledata = stacking(copper);
        assert!(
            !tiledata.static_tile(pile_art.0).flags.is_stackable(),
            "the case this test is about"
        );

        let labelled = labels(
            &[GroundItem {
                amount: ItemAmount(50),
                at: Point::new(100, 100, 0),
                graphic: copper,
                hue: Hue::NONE,
            }],
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas(pile_art, 30, 50),
            &Cutaway::OPEN,
        );
        assert_eq!(
            labelled.into_iter().map(|(_, text)| text).collect::<Vec<_>>(),
            vec!["50".to_owned()]
        );
    }

    /// A pile the frame does not draw is a pile with no number over it.
    ///
    /// Both halves of "not drawn" are the same one question asked through
    /// [`place`], which is why one test covers them: a graphic the atlas holds
    /// no art for, and a pile the storey cut hid. A number floating where the
    /// picture is not is worse than no number.
    #[test]
    fn a_pile_the_frame_does_not_draw_is_not_counted() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0EED);
        let tiledata = stacking(graphic);
        let pile = GroundItem {
            amount: ItemAmount(500),
            at: Point::new(100, 100, 0),
            graphic,
            hue: Hue::NONE,
        };

        // Nothing packed at all: `place` has no sprite to stand on.
        let empty = StaticAtlas::pack([]).expect("an empty atlas packs");
        assert!(
            labels(
                &[pile],
                &camera,
                &tiledata,
                &StaticAnimations::default(),
                &empty,
                &Cutaway::OPEN,
            )
            .is_empty()
        );

        // Packed, but on a storey this frame has cut away: the picture moves
        // to the late translucent layer, where it is a hint of a thing rather
        // than the thing, and a number over it would be the one part of it
        // still drawn at full strength.
        let atlas = atlas(graphic, 30, 50);
        assert!(
            labels(
                &[pile],
                &camera,
                &tiledata,
                &StaticAnimations::default(),
                &atlas,
                &Cutaway {
                    max_z: 0,
                    ..Cutaway::OPEN
                },
            )
            .is_empty()
        );
    }

    /// Height lifts an item off the floor the way it lifts everything else, and
    /// it lifts its depth with it — a coin on a table is in front of the table's
    /// own tile, not behind it.
    #[test]
    fn a_higher_item_is_drawn_higher_and_nearer() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0EED);
        let atlas = atlas(graphic, 30, 50);
        let tiledata = TileData::empty();
        let at = |z: i8| GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(100, 100, z),
            graphic,
            hue: Hue::NONE,
        };
        let floor = collect(
            &[at(0)],
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            None,
            &crate::occlusion::Occlusion::EMPTY,
            None,
        );
        let table = collect(
            &[at(10)],
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            None,
            &crate::occlusion::Occlusion::EMPTY,
            None,
        );
        assert_eq!(
            table.quads[0].rect.y,
            floor.quads[0].rect.y - 40.0,
            "four pixels a unit"
        );
        assert!(table.quads[0].depth < floor.quads[0].depth, "smaller is nearer");
    }

    /// An item whose graphic is not packed is dropped rather than drawn from
    /// whatever else is in the atlas.
    #[test]
    fn an_item_with_no_sprite_is_dropped() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = atlas(Graphic(0x0EED), 30, 50);
        let tiledata = TileData::empty();
        let quads = collect(
            &[GroundItem {
                amount: ItemAmount::ONE,
                at: Point::new(100, 100, 0),
                graphic: Graphic(0x0EEE),
                hue: Hue::NONE,
            }],
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            None,
            &crate::occlusion::Occlusion::EMPTY,
            None,
        );
        assert!(quads.quads.is_empty());
    }

    /// Two items on different tiles come back with the further one first, so a
    /// pass that ignored the depth buffer entirely would still paint them in the
    /// right order.
    #[test]
    fn the_quads_come_back_from_the_back() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0EED);
        let atlas = atlas(graphic, 30, 50);
        let tiledata = TileData::empty();
        let item = |x: u16, y: u16| GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(x, y, 0),
            graphic,
            hue: Hue::NONE,
        };
        // Given nearest first, on purpose.
        let quads = collect(
            &[item(101, 101), item(99, 99)],
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            None,
            &crate::occlusion::Occlusion::EMPTY,
            None,
        );
        assert_eq!(quads.quads.len(), 2);
        assert!(
            quads.quads[0].depth > quads.quads[1].depth,
            "the far one is drawn first"
        );
    }

    /// The viewport pixel a point in the drawn image sits at — the inverse of
    /// what [`pick`] undoes, so a test can click on a sprite it has placed.
    fn cursor_over(camera: &Camera, at: crate::camera::ViewPoint, dx: f32, dy: f32) -> RealPixel {
        let spot = camera.to_viewport(crate::camera::ViewPixel {
            x: (at.x + dx) as i32,
            y: (at.y + dy) as i32,
        });
        RealPixel::new(spot.x as i32, spot.y as i32)
    }

    /// Art with a hole in it: the left half transparent, the right half drawn.
    /// A door's leaf is this shape and so is most static art — which is the
    /// whole reason picking is a texel test and not a rectangle one.
    fn holed(graphic: Graphic, width: u16, height: u16) -> StaticAtlas {
        let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height));
        for _ in 0..height {
            for x in 0..width {
                pixels.push(match x < width / 2 {
                    true => Color16::TRANSPARENT,
                    false => Color16(0x7C00),
                });
            }
        }
        StaticAtlas::pack([(graphic, Image::new(width, height, pixels))]).expect("one sprite fits")
    }

    #[test]
    fn a_click_on_an_item_s_own_pixels_picks_it() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0675);
        let atlas = holed(graphic, 40, 60);
        let tiledata = TileData::empty();
        let item = GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(100, 100, 0),
            graphic,
            hue: Hue::NONE,
        };
        let sprite = atlas.sprite(graphic).expect("packed");
        let at = stand_on(&camera, item.at, &sprite);
        let pick_at = |dx, dy| {
            pick(
                std::slice::from_ref(&item),
                &camera,
                &tiledata,
                &StaticAnimations::default(),
                &atlas,
                &Cutaway::OPEN,
                cursor_over(&camera, at, dx, dy),
            )
        };
        assert_eq!(
            pick_at(30.0, 30.0),
            Some(ItemIndex::new(0)),
            "the drawn half was not hit"
        );
        assert_eq!(
            pick_at(5.0, 30.0),
            None,
            "the transparent half of the picture was picked — this is a box test, not a texel one"
        );
        assert_eq!(pick_at(-5.0, 30.0), None, "a pixel left of the sprite was picked");
        assert_eq!(pick_at(30.0, 70.0), None, "a pixel below the sprite was picked");
    }

    /// Two doors of a shopfront overlap on screen. The one drawn on top is the
    /// one the click gets — the same answer the depth buffer gives the frame,
    /// which is what the player sees.
    #[test]
    fn the_topmost_item_wins_an_overlap() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0675);
        // Tall enough that the nearer tile's sprite covers the further one's.
        let atlas = atlas(graphic, 44, 120);
        let tiledata = TileData::empty();
        let item = |x: u16, y: u16| GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(x, y, 0),
            graphic,
            hue: Hue::NONE,
        };
        // Given furthest first, which is *not* what decides it: the order does.
        let items = [item(100, 100), item(101, 101)];
        let sprite = atlas.sprite(graphic).expect("packed");
        let near = stand_on(&camera, items[1].at, &sprite);
        let found = pick(
            &items,
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            // Inside the near sprite's top strip, which is over the far one's
            // body: both are hit and only one may come back.
            cursor_over(&camera, near, 22.0, 10.0),
        );
        assert_eq!(
            found,
            Some(ItemIndex::new(1)),
            "the door behind was picked through the one in front"
        );
    }

    /// The item the cursor is over is drawn in the highlight ramp, and only
    /// that one. The index is the same one [`pick`] hands back, so what the
    /// player sees lit is what a double-click would use.
    #[test]
    fn the_highlighted_item_is_drawn_in_the_highlight_hue() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0675);
        let atlas = atlas(graphic, 40, 60);
        let tiledata = TileData::empty();
        // Its own hue, so the assertion below is "replaced" and not "set".
        let item = |x: u16| GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(x, 100, 0),
            graphic,
            hue: Hue(0x03B2),
        };
        let items = [item(100), item(101)];
        let quads = collect(
            &items,
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            Some(ItemIndex::new(1)),
            &crate::occlusion::Occlusion::EMPTY,
            None,
        );
        assert_eq!(quads.quads.len(), 2);
        // The pass sorts back to front, so the nearer one — index 1, a tile
        // south-east — is the second quad.
        assert_eq!(
            quads.quads[1].hue,
            u32::from(HIGHLIGHT_HUE.0),
            "the pointed-at item"
        );
        assert_eq!(quads.quads[0].hue, 0x03B2, "and nothing else changed colour");
    }

    /// A server item can be the thing covering the controlled body just as a
    /// map wall can. It takes the late layer only where actual opaque texels
    /// overlap; otherwise it keeps the normal opaque/pickable route.
    #[test]
    fn a_dropped_item_over_the_player_becomes_a_cutaway_row() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0EED);
        let atlas = atlas(graphic, 44, 88);
        let tiledata = TileData::empty();
        let item = GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(100, 100, 0),
            graphic,
            hue: Hue::NONE,
        };
        let screen_at = stand_on(&camera, item.at, &atlas.sprite(graphic).expect("packed item"));
        let body = crate::geometry::Rect {
            x: screen_at.x + 12.0,
            y: screen_at.y + 32.0,
            width: 20.0,
            height: 32.0,
        };
        let body_mask = crate::mobiles::OpaqueMask::solid(body);

        let opaque = collect(
            std::slice::from_ref(&item),
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            None,
            &crate::occlusion::Occlusion::EMPTY,
            None,
        );
        assert_eq!(opaque.quads.len(), 1, "the fixture did not draw its item");
        assert!(opaque.cutaway_quads.is_empty(), "no body means no late item");

        let cutaway = collect(
            std::slice::from_ref(&item),
            &camera,
            &tiledata,
            &StaticAnimations::default(),
            &atlas,
            &Cutaway::OPEN,
            None,
            &crate::occlusion::Occlusion::EMPTY,
            Some(&body_mask),
        );
        assert!(cutaway.quads.is_empty(), "the item stayed in opaque world");
        assert_eq!(cutaway.cutaway_quads.len(), 1, "the item missed the late layer");
        assert_eq!(cutaway.cutaway_quads[0].rect, opaque.quads[0].rect);
        assert_eq!(cutaway.cutaway_quads[0].depth, opaque.quads[0].depth);
    }

    /// The ordinary roof/storey predicate must not make server decorations
    /// vanish outright: the late layer owns that visual transition for both
    /// map statics and dynamic items.
    #[test]
    fn a_dropped_item_hidden_by_the_storey_cut_becomes_a_cutaway_row() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0EED);
        let atlas = atlas(graphic, 44, 88);
        let item = GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(100, 100, 0),
            graphic,
            hue: Hue::NONE,
        };
        let cutaway = collect(
            std::slice::from_ref(&item),
            &camera,
            &TileData::empty(),
            &StaticAnimations::default(),
            &atlas,
            &Cutaway {
                max_z: 0,
                ..Cutaway::OPEN
            },
            None,
            &crate::occlusion::Occlusion::EMPTY,
            None,
        );

        assert!(cutaway.quads.is_empty(), "the hidden item reached opaque world");
        assert_eq!(
            cutaway.cutaway_quads.len(),
            1,
            "the storey cut made a dropped item vanish"
        );
    }

    /// Nothing under the cursor is a legitimate answer and the common one: most
    /// of the screen is ground.
    #[test]
    fn a_click_on_bare_ground_picks_nothing() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let graphic = Graphic(0x0675);
        let atlas = atlas(graphic, 40, 60);
        let tiledata = TileData::empty();
        let item = GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(100, 100, 0),
            graphic,
            hue: Hue::NONE,
        };
        let sprite = atlas.sprite(graphic).expect("packed");
        let at = stand_on(&camera, item.at, &sprite);
        assert_eq!(
            pick(
                std::slice::from_ref(&item),
                &camera,
                &tiledata,
                &StaticAnimations::default(),
                &atlas,
                &Cutaway::OPEN,
                cursor_over(&camera, at, 200.0, 200.0),
            ),
            None
        );
    }

    /// The graphics a list needs, once each, ready to be unioned with the map's.
    #[test]
    fn the_needed_graphics_are_deduplicated() {
        let item = |graphic: u16| GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(0, 0, 0),
            graphic: Graphic(graphic),
            hue: Hue::NONE,
        };
        let wanted = needed_graphics(
            &[item(0x0EED), item(0x0EED), item(0x0EEA)],
            &StaticAnimations::default(),
        );
        assert_eq!(
            wanted.into_iter().collect::<Vec<_>>(),
            vec![Graphic(0x0EEA), Graphic(0x0EED)],
        );
    }
}
