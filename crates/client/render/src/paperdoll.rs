//! A paperdoll: one body picture with everything the mobile is wearing laid
//! over it, in the order the reference client draws them.
//!
//! [`crate::container`]'s twin, and deliberately built out of the same pieces —
//! a list of [`Picture`]s at an origin the caller owns, packed into the same
//! [`GumpAtlas`](crate::gump::GumpAtlas). What a paperdoll adds to a container is not a window kind, it
//! is two questions a bag never has to ask:
//!
//! # 1. Which picture a worn item is, which is a third index space
//!
//! An item has three graphics, and they are not related by arithmetic. Its
//! **wire graphic** is what the shard sends and what `art.mul` draws in a bag.
//! Its **`AnimID`** (`tiledata`) is what `anim.mul` draws on a walking body —
//! that is [`crate::mobiles`]'s space, and [`EquipmentLayer::graphic`] already
//! holds it. Its **paperdoll gump** is that same `AnimID` offset into
//! `gumpart` by [`MALE_GUMP_OFFSET`] or [`FEMALE_GUMP_OFFSET`], which is a
//! different picture of the same shirt drawn from a different file.
//!
//! So [`crate::mobiles`]'s resolver cannot be reused, and neither can its
//! `EquipConv` lookup: the table has *two* override columns, one per space, and
//! a paperdoll reads the one the animation ignores. See [`gump_of`], which is
//! `PaperDollInteractable.GetAnimID` in full.
//!
//! # 2. What order they go in, which is not the layer numbering
//!
//! On a walking body the layers rarely overlap wrongly and the wire's own order
//! is tolerable. A paperdoll is one flat picture where every layer overlaps
//! every other, so the order is a table: [`order`], ported from
//! `PaperdollOrder.Build` — three base tables chosen by what is worn on the
//! arms and torso, then a handful of per-graphic reorder rules. The backpack is
//! not in it at all; it is drawn last, outside the pass, which is what
//! [`window`] does with it.
//!
//! # What this does not do yet
//!
//! **Covering.** The reference also *hides* layers an outer garment fully
//! occludes (`MobileView.IsCovered`) — shoes under plate legs, arms under a
//! closed robe. Every arm of that function keys on the item's **wire graphic**,
//! which is the one graphic [`EquipmentLayer`] does not carry, so it is a
//! backlog entry in `docs/client.md` and not a silent approximation here. Its
//! absence draws a layer that should have been hidden, which is a garment
//! poking out from under a robe — visible, and not a hole.
//!
//! **Buttons**, and picking a worn item out of the picture. Both are M5's, and
//! both want the item's serial, which this end reaches through the mobile.

use openshard_protocol::wire::{Graphic, Hue, Layer};
use openshard_uofiles::equipconv::EquipConv;
use openshard_uofiles::gumpart::Gumps;

use crate::gump::{GumpArt, GumpPixel, Picture};
use crate::mobiles::EquipmentLayer;

/// Whose paperdoll a window is, which the frame is chosen by.
///
/// The wire never says it: a `0x88` carries a serial, and whether that serial is
/// this client's own is a question only the client can answer. The reference
/// asks it in exactly one place — `LocalSerial == World.Player` in
/// `PaperDollGump.BuildGump` — and the answer decides which background picture
/// is drawn, because one of the two has room down its right-hand side for the
/// buttons a player gets over their own doll and nobody gets over a stranger's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Whose {
    /// This client's own character.
    Own,
    /// Anybody else — an NPC, another player.
    Another,
}

/// The frame around a paperdoll: `0x07D0` for our own, `0x07D1` for anyone
/// else's.
///
/// `PaperDollGump.BuildGump`, whose expression is `0x07d0 + (LocalSerial ==
/// World.Player ? 0 : 1)` — written out here as two named pictures, because the
/// arithmetic is a coincidence of how the file is laid out and not a rule.
///
/// The frame is the *window*: its size is the window's size and its pixels are
/// what a click is tested against, which is why the body no longer is — see
/// [`BODY_AT`].
pub fn frame(whose: Whose) -> Graphic {
    match whose {
        Whose::Own => Graphic(0x07D0),
        Whose::Another => Graphic(0x07D1),
    }
}

/// Where the body picture sits inside the frame.
///
/// `new PaperDollInteractable(8, 19, ...)` in `PaperDollGump.BuildGump`: the
/// doll is a control placed on the background, and every layer over it shares
/// the one origin, so this offset applies once to the whole stack and never
/// per garment.
pub const BODY_AT: GumpPixel = GumpPixel::new(8, 19);

/// What a male body's worn items are offset by to reach their gump.
///
/// `Constants.MALE_GUMP_OFFSET`. Deliberately a plain number and not a table:
/// the whole of `gumpart`'s equipment range is `AnimID` plus one of these two.
pub const MALE_GUMP_OFFSET: u16 = 50_000;

/// The same for a female body — `Constants.FEMALE_GUMP_OFFSET`.
///
/// The female half of the file is *sparse*, which is what [`gump_of`]'s
/// fallback is for: a garment with no female picture is drawn from the male
/// one rather than not drawn at all.
pub const FEMALE_GUMP_OFFSET: u16 = 60_000;

/// One mobile, as much of it as a paperdoll draws.
///
/// Three fields and no serial: a paperdoll is a picture of a body, and *whose*
/// body it is, is the caller's business — the window is filed by serial, the
/// picture is not. Built from what the client already knows about the mobile,
/// which is the point of `0x88` carrying no equipment (see
/// `WorldView::paperdolls`).
#[derive(Clone, Copy, Debug)]
pub struct Wearer<'a> {
    /// Its body id, as the shard named it.
    pub body: u16,
    /// Its hue, or [`Hue::NONE`] — the body picture's, not the equipment's.
    pub hue: Hue,
    /// What it is wearing: `AnimID`s and layers, exactly as
    /// [`crate::mobiles::Mobile::equipment`] holds them, so that one resolution
    /// out of `tiledata` serves the world sprite and the paperdoll alike.
    pub equipment: &'a [EquipmentLayer],
}

/// How many layers the ordering tables cover — `PaperdollOrder.N`, the layers
/// `0x00..=0x18`, sentinel included.
const LAYERS: usize = 0x19;

/// The layer byte that is not a layer: `Layer.Invalid`, which the base tables
/// carry at index zero so that a layer's index in the table and its number stay
/// separate ideas. Dropped by [`order`].
const SENTINEL: Layer = Layer(0x00);

/// Arms drawn late — `PaperdollOrder.T1`.
const T1: [Layer; LAYERS] = [
    SENTINEL,
    Layer::CLOAK,
    Layer::SHIRT,
    Layer::PANTS,
    Layer::SHOES,
    Layer::LEGS,
    Layer::TORSO,
    Layer::TUNIC,
    Layer::RING,
    Layer::BRACELET,
    Layer::FACE,
    Layer::ARMS,
    Layer::GLOVES,
    Layer::SKIRT,
    Layer::ROBE,
    Layer::WAIST,
    Layer::NECKLACE,
    Layer::HAIR,
    Layer::BEARD,
    Layer::EARRINGS,
    Layer::HELMET,
    Layer::ONE_HANDED,
    Layer::TWO_HANDED,
    Layer::BACKPACK,
    Layer::TALISMAN,
];

/// The default: arms early — `PaperdollOrder.T2`.
const T2: [Layer; LAYERS] = [
    SENTINEL,
    Layer::CLOAK,
    Layer::SHIRT,
    Layer::PANTS,
    Layer::SHOES,
    Layer::LEGS,
    Layer::ARMS,
    Layer::TORSO,
    Layer::TUNIC,
    Layer::RING,
    Layer::BRACELET,
    Layer::FACE,
    Layer::GLOVES,
    Layer::SKIRT,
    Layer::ROBE,
    Layer::WAIST,
    Layer::NECKLACE,
    Layer::HAIR,
    Layer::BEARD,
    Layer::EARRINGS,
    Layer::HELMET,
    Layer::ONE_HANDED,
    Layer::TWO_HANDED,
    Layer::BACKPACK,
    Layer::TALISMAN,
];

/// Torso pulled to the front — `PaperdollOrder.T3`, the chest-under order a
/// female or gargoyle body takes for the handful of torso graphics that need
/// it.
const T3: [Layer; LAYERS] = [
    SENTINEL,
    Layer::CLOAK,
    Layer::TORSO,
    Layer::SHIRT,
    Layer::PANTS,
    Layer::SHOES,
    Layer::LEGS,
    Layer::TUNIC,
    Layer::RING,
    Layer::BRACELET,
    Layer::FACE,
    Layer::ARMS,
    Layer::GLOVES,
    Layer::SKIRT,
    Layer::ROBE,
    Layer::WAIST,
    Layer::NECKLACE,
    Layer::HAIR,
    Layer::BEARD,
    Layer::EARRINGS,
    Layer::HELMET,
    Layer::ONE_HANDED,
    Layer::TWO_HANDED,
    Layer::BACKPACK,
    Layer::TALISMAN,
];

/// What is worn on each layer, by `AnimID`, for the tables to be keyed on.
///
/// Zero is "nothing worn there", which is also what an item with no `AnimID`
/// means — a ring, a talisman: something the paperdoll draws nothing for. The
/// reorder rules read this array and never the equipment list, exactly as
/// `PaperdollOrder.GraphicsFromEntity` builds it, because every rule is a
/// question about one layer's graphic.
fn by_layer(equipment: &[EquipmentLayer]) -> [u16; LAYERS] {
    let mut graphics = [0u16; LAYERS];
    for item in equipment {
        let index = usize::from(item.layer.0);
        if index < LAYERS {
            graphics[index] = item.graphic;
        }
    }
    graphics
}

/// The layers a paperdoll draws, back to front — later is painted on top.
///
/// `PaperdollOrder.Build` followed by its `Filter`: the sentinel, the mount and
/// the backpack are dropped, the first because it is not a layer and the other
/// two because a paperdoll draws neither in the pass (a mount is a body on the
/// ground, and the backpack goes last and outside — see [`window`]).
///
/// `alt_torso` is the reference's own gate on the third table: a female or a
/// gargoyle body. It is a parameter rather than read from a body id here so
/// that the table stays a pure function of what is worn — which is what makes
/// it testable without a client's files.
///
/// Every layer is in the answer whether or not anything is worn on it. The list
/// is an *order*, not a plan: what is worn decides which order, and [`window`]
/// is what skips the empty slots.
pub fn order(equipment: &[EquipmentLayer], alt_torso: bool) -> Vec<Layer> {
    let graphic = by_layer(equipment);
    let mut order = base_table(&graphic, alt_torso);
    reorder(&mut order, &graphic);
    order
        .into_iter()
        .filter(|layer| *layer != SENTINEL && *layer != Layer::MOUNT && *layer != Layer::BACKPACK)
        .collect()
}

/// Which of the three base tables this outfit starts from.
///
/// The arms check wins outright — a match there locks the arms-late table and
/// the torso is never consulted, which is the reference's own control flow and
/// not an optimisation of it.
fn base_table(graphic: &[u16; LAYERS], alt_torso: bool) -> [Layer; LAYERS] {
    let arms = graphic[usize::from(Layer::ARMS.0)];
    let arms_alternate = match arms < 0x3D0 {
        true => arms == 0x3CF || arms == 0x210 || arms == 0x3B3,
        false => arms == 0x3DD,
    };
    if arms_alternate {
        return T1;
    }
    let torso = u32::from(graphic[usize::from(Layer::TORSO.0)]);
    if torso == 0x21A {
        return T1;
    }
    // `torso - 0x399 < 5` in the reference, on an unsigned widening — so a
    // torso below 0x399 wraps to something enormous and does not match. The
    // five graphics are the ones the chest-under order was written for.
    if torso.wrapping_sub(0x399) < 5 && alt_torso {
        return T3;
    }
    T2
}

/// The per-graphic reorder rules, in the reference's order and with its early
/// exit.
///
/// Each one is a specific garment whose art was drawn expecting a different
/// neighbour — a quiver that has to sit over a robe, a pair of boots the
/// leggings tuck into. There is no principle to extract: this is a list of
/// exceptions the client ships, and reordering the list changes the answer.
fn reorder(order: &mut [Layer; LAYERS], graphic: &[u16; LAYERS]) {
    let at = |layer: Layer| graphic[usize::from(layer.0)];

    // A shirt under these leggings: the leggings go where the shirt is.
    if at(Layer::SHIRT) != 0 && at(Layer::PANTS) == 0x398 {
        move_to(order, Layer::PANTS, Layer::SHIRT);
    }

    let pants = u32::from(at(Layer::PANTS));
    let mut pants_settled = false;
    if pants < 0x201 {
        if pants == 0x200 || pants == 0x1EB || pants == 0x1FA {
            // Boots the leggings tuck into: shoes first, whichever way the
            // table had them.
            if let (Some(shoes), Some(pants)) = (index_of(order, Layer::SHOES), index_of(order, Layer::PANTS))
            {
                if pants < shoes {
                    order.swap(shoes, pants);
                }
            }
        }
    } else if pants.wrapping_sub(0x513) < 2 {
        if at(Layer::SHOES) != 0 {
            move_to(order, Layer::PANTS, Layer::SHOES);
        }
        pants_settled = true;
    }

    if !pants_settled && at(Layer::SHOES) != 0 && at(Layer::PANTS) == 0x3E4 {
        move_to(order, Layer::PANTS, Layer::SHOES);
    }

    // The surcoat, which is worn over the belt, and over the neck line of the
    // four robes it was cut to go with.
    if at(Layer::TUNIC) == 0x238 {
        move_after(order, Layer::TUNIC, Layer::WAIST);
        let robe = at(Layer::ROBE);
        if matches!(robe, 0x4E8..=0x4EB | 0x5E2..=0x5E5) {
            move_to(order, Layer::ROBE, Layer::NECKLACE);
        }
    }

    // The quiver, which is worn on the cloak layer and belongs over the robe.
    if matches!(at(Layer::CLOAK), 0x380 | 0x5F3) {
        move_after(order, Layer::CLOAK, Layer::ROBE);
    }

    let helmet = u32::from(at(Layer::HELMET));
    if helmet < 0x202 {
        let neck = at(Layer::NECKLACE);
        if (helmet == 0x201 || helmet == 0x1A9) && (neck == 0x1C8 || (0x1D7..0x1D9).contains(&neck)) {
            move_after(order, Layer::NECKLACE, Layer::HELMET);
            // The reference returns here, and it matters: the helmet rule below
            // is skipped, not merely unmatched.
        }
    } else if helmet.wrapping_sub(0x5E9) < 2
        && at(Layer::ROBE) != 0
        && u32::from(at(Layer::ROBE)).wrapping_sub(0x5E2) < 4
    {
        move_to(order, Layer::ROBE, Layer::HELMET);
    }
}

/// Where a layer sits in an order, or `None` — which cannot happen for a layer
/// the tables carry, and is answered rather than asserted because the tables
/// are data and a rule naming a layer none of them holds should reorder
/// nothing.
fn index_of(order: &[Layer; LAYERS], layer: Layer) -> Option<usize> {
    order.iter().position(|entry| *entry == layer)
}

/// Move `layer` so that it lands on `onto`'s index, sliding everything between
/// them along — the reference's `MoveTo`, which is a memmove and not a swap.
fn move_to(order: &mut [Layer; LAYERS], layer: Layer, onto: Layer) {
    let (Some(from), Some(to)) = (index_of(order, layer), index_of(order, onto)) else {
        return;
    };
    slide(order, from, to);
}

/// The same, landing immediately *after* `behind` — the reference's
/// `MoveAfter`. Moving something forwards lands it on `behind`'s own index,
/// because everything between has already slid back one.
fn move_after(order: &mut [Layer; LAYERS], layer: Layer, behind: Layer) {
    let (Some(from), Some(to)) = (index_of(order, layer), index_of(order, behind)) else {
        return;
    };
    match to < from {
        true => slide(order, from, to + 1),
        false => slide(order, from, to),
    }
}

/// Take the entry at `from` out and put it back at `to`, in place.
fn slide(order: &mut [Layer; LAYERS], from: usize, to: usize) {
    if from == to {
        return;
    }
    // A rotation *is* the memmove the reference writes out by hand: rotating
    // the span between the two ends carries the moved entry to the far end of
    // it and slides everything else one place the other way.
    match to < from {
        true => order[to..=from].rotate_right(1),
        false => order[from..=to].rotate_left(1),
    }
}

/// The base picture — the body itself, and the hue it is drawn in.
///
/// The hue is answered here rather than taken from the mobile because one body
/// overrides it: `0x03DB`, the "player as an object" body, is drawn as an
/// ordinary male doll in a fixed hue. Everything else keeps its own.
///
/// A body this does not know is drawn male, which is what the reference falls
/// through to — a paperdoll of a creature is a rough thing, and no paperdoll at
/// all would be worse.
pub fn body_gump(body: u16, hue: Hue) -> (Graphic, Hue) {
    match body {
        0x0191 | 0x0193 => (Graphic(0x000D), hue),
        0x025D => (Graphic(0x000E), hue),
        0x025E => (Graphic(0x000F), hue),
        0x029A | 0x02B6 => (Graphic(0x029A), hue),
        0x029B | 0x02B7 => (Graphic(0x0299), hue),
        0x04E5 => (Graphic(0xC835), hue),
        // The one body whose hue is not its own.
        0x03DB => (Graphic(0x000C), Hue(0x03EA)),
        _ => match openshard_uofiles::anim::is_female(body) {
            true => (Graphic(0x000D), hue),
            false => (Graphic(0x000C), hue),
        },
    }
}

/// Which gump a worn item draws as on this body — `GetAnimID`, in full bar the
/// two tables this engine does not read.
///
/// Three steps, in the reference's order:
///
/// 1. The dead gargoyle's shroud, which is a different `AnimID` on a paperdoll
///    than on the ground. Applied unconditionally where the reference gates it
///    on a 7.0 client, because the two bodies it names did not exist before
///    one.
/// 2. `EquipConv`'s **gump** column, which is a different override from the
///    animation one and is read here and nowhere else. A value already past
///    [`MALE_GUMP_OFFSET`] is a gump id the table stated outright — the offset
///    is taken back off, because step 3 puts one on.
/// 3. The offset itself, male or female, with the fallback that makes the
///    sparse female half of the file work: no female picture means the male one
///    (`IsAnimExistsInGump`).
///
/// Not ported: `ConvertBodyIfNeeded` (`Bodyconv.def`, which this crate does not
/// read — the same gap `crate::mobiles`'s resolver has) and the `tileart.uop`
/// appearance table, which is a modern-client file nothing here opens. Both
/// answer the same question for bodies this engine does not send yet.
pub fn gump_of(body: u16, anim_id: u16, female: bool, equip_conv: &EquipConv, gumps: &Gumps) -> Graphic {
    // The dead gargoyle's shroud.
    let anim_id = match anim_id == 0x03CA && matches!(body, 0x02B6 | 0x02B7) {
        true => 0x0223,
        false => anim_id,
    };
    let anim_id = match equip_conv.resolve(body, anim_id) {
        Some(entry) if entry.gump.0 > MALE_GUMP_OFFSET => match entry.gump.0 >= FEMALE_GUMP_OFFSET {
            true => entry.gump.0 - FEMALE_GUMP_OFFSET,
            false => entry.gump.0 - MALE_GUMP_OFFSET,
        },
        Some(entry) => entry.gump.0,
        None => anim_id,
    };

    let wanted = match female {
        true => FEMALE_GUMP_OFFSET,
        false => MALE_GUMP_OFFSET,
    };
    let picture = |offset: u16| Graphic(anim_id.wrapping_add(offset));
    // A read that fails is a missing picture as far as this is concerned: the
    // fallback is the whole point, and a paperdoll is not the place to refuse
    // to draw because a container entry could not be looked up.
    if gumps.has(picture(wanted)).unwrap_or(false) {
        return picture(wanted);
    }
    let other = match female {
        true => MALE_GUMP_OFFSET,
        false => FEMALE_GUMP_OFFSET,
    };
    match gumps.has(picture(other)).unwrap_or(false) {
        true => picture(other),
        // Neither exists. The reference logs and draws the offset picture
        // anyway; so does this, and `collect` skips what the atlas has no
        // sprite for — one missing garment rather than no paperdoll.
        false => picture(wanted),
    }
}

// There is no `size` here, deliberately, and [`crate::container::size`] has
// lost its caller to the same decision: a window is not a rectangle at all. It
// is the list of pictures [`window`] lays out, and what the pointer is tested
// against is that list — [`crate::gump::pick`], over what was drawn — so
// nothing ever has to ask how big the window is or which of its pictures is the
// background. That is what lets a hat drawn past the edge of the frame belong
// to the window and a click through the frame's transparent corner fall to the
// world, neither of which a size can express.

/// Lay a paperdoll out at `at`: the frame, then the body inside it, then every
/// worn layer over that, then the backpack.
///
/// `wearer` is `None` for a mobile this client has never been told the body of —
/// which happens when a shard opens a paperdoll for someone it has not revealed.
/// The frame is drawn anyway, deliberately: the window is open, and a window
/// with no picture is one the pointer cannot find and the player cannot close.
/// What is missing is the doll, and it appears on the frame the `0x77` arrives
/// on.
///
/// Every picture inside the frame is drawn at one origin, [`BODY_AT`] past the
/// window's — a paperdoll layer's art is a full-size picture with its own
/// transparency, not a sprite with a position, which is why nothing here has a
/// per-item coordinate the way [`crate::container::window`] does.
///
/// A layer with no `AnimID` draws nothing: zero is the absence of a picture,
/// which is what a ring or a talisman has. Hair and a beard on a ghost draw
/// nothing either, for the reason [`crate::mobiles`]'s resolver has — the
/// reference refuses both on the dead, and a bald ghost is the shard's own
/// answer read the way its client reads it.
///
/// The backpack is last and outside the order, exactly as the reference draws
/// it: it hangs off the side of the picture and nothing is ever drawn over it.
pub fn window(
    wearer: Option<&Wearer<'_>>,
    whose: Whose,
    equip_conv: &EquipConv,
    gumps: &Gumps,
    at: GumpPixel,
) -> Vec<Picture> {
    let mut pictures = vec![Picture::plain(GumpArt::Gump(frame(whose)), at)];
    let Some(wearer) = wearer else {
        return pictures;
    };
    let at = GumpPixel::new(at.x + BODY_AT.x, at.y + BODY_AT.y);

    let female = openshard_uofiles::anim::is_female(wearer.body);
    let ghost = openshard_uofiles::anim::is_ghost(wearer.body);
    let alt_torso = female || openshard_uofiles::anim::is_gargoyle(wearer.body);

    let (body, hue) = body_gump(wearer.body, wearer.hue);
    pictures.push(Picture::plain(GumpArt::Gump(body), at).hued(hue));

    let worn = |layer: Layer| -> Option<&EquipmentLayer> {
        wearer
            .equipment
            .iter()
            .find(|item| item.layer == layer && item.graphic != 0)
    };
    let draw = |pictures: &mut Vec<Picture>, item: &EquipmentLayer| {
        let graphic = gump_of(wearer.body, item.graphic, female, equip_conv, gumps);
        pictures.push(Picture::plain(GumpArt::Gump(graphic), at).hued(item.hue));
    };

    for layer in order(wearer.equipment, alt_torso) {
        if ghost && (layer == Layer::HAIR || layer == Layer::BEARD) {
            continue;
        }
        if let Some(item) = worn(layer) {
            draw(&mut pictures, item);
        }
    }
    if let Some(item) = worn(Layer::BACKPACK) {
        draw(&mut pictures, item);
    }
    pictures
}

/// Everything a paperdoll needs packed before it is drawn.
///
/// [`crate::container::art_of`]'s twin in purpose and not in shape: a
/// container's list can be built from the wire alone, and a paperdoll's cannot
/// — which picture a layer is, is the answer to [`gump_of`], so asking for the
/// list *is* laying the window out. Hence a list of [`Picture`]s in and the art
/// out, rather than two walks that could disagree about what was drawn.
pub fn art_of(pictures: &[Picture]) -> Vec<GumpArt> {
    pictures.iter().map(|picture| picture.graphic).collect()
}

#[cfg(test)]
mod tests {
    use openshard_uofiles::equipconv::EquipConv;

    use super::*;

    /// One worn item.
    fn worn(layer: Layer, graphic: u16) -> EquipmentLayer {
        EquipmentLayer {
            graphic,
            hue: Hue::NONE,
            layer,
        }
    }

    /// Where a layer lands in an order, for the assertions below to compare.
    fn at(order: &[Layer], layer: Layer) -> usize {
        order
            .iter()
            .position(|entry| *entry == layer)
            .expect("every layer the tables carry is in every order")
    }

    /// The default table, and the two things every order must be true of: the
    /// sentinel is not a layer, and neither the mount nor the backpack is drawn
    /// in the pass.
    #[test]
    fn an_order_is_every_layer_but_the_three_that_are_not_drawn() {
        let order = order(&[], false);
        assert_eq!(order.len(), LAYERS - 2, "the sentinel and the backpack are gone");
        assert!(!order.contains(&SENTINEL));
        assert!(!order.contains(&Layer::BACKPACK));
        assert!(!order.contains(&Layer::MOUNT));
    }

    /// Decision 3 in `docs/client.md`, and the reason [`EquipmentLayer`] carries
    /// its layer at all: a female body draws the torso *under* the shirt for
    /// the chest graphics the alternate table was written for, and a male one
    /// draws it over. Same outfit, two orders.
    #[test]
    fn a_female_body_draws_the_torso_where_a_male_one_does_not() {
        let outfit = [worn(Layer::TORSO, 0x399), worn(Layer::SHIRT, 0x100)];

        let male = order(&outfit, false);
        assert!(
            at(&male, Layer::SHIRT) < at(&male, Layer::TORSO),
            "the default table puts the shirt first"
        );

        let female = order(&outfit, true);
        assert!(
            at(&female, Layer::TORSO) < at(&female, Layer::SHIRT),
            "the chest-under table does not"
        );
    }

    /// And the gate is the *graphic*, not the body: the same female body with a
    /// torso the alternate table was not written for keeps the default order.
    #[test]
    fn the_chest_under_table_is_chosen_by_the_torso_graphic_too() {
        let outfit = [worn(Layer::TORSO, 0x500), worn(Layer::SHIRT, 0x100)];
        let female = order(&outfit, true);
        assert!(at(&female, Layer::SHIRT) < at(&female, Layer::TORSO));
    }

    /// The arms check wins outright — a match locks the arms-late table, and
    /// the torso is never consulted even on a body that would have chosen the
    /// chest-under one.
    #[test]
    fn arms_that_draw_late_lock_the_table_before_the_torso_is_asked() {
        let outfit = [worn(Layer::ARMS, 0x210), worn(Layer::TORSO, 0x399)];
        let order = order(&outfit, true);
        assert!(
            at(&order, Layer::TORSO) < at(&order, Layer::ARMS),
            "T1 draws the arms after the torso"
        );
        assert!(
            at(&order, Layer::CLOAK) < at(&order, Layer::TORSO),
            "and it is T1, not T3, which would have pulled the torso to the front"
        );
    }

    /// The quiver, which is worn on the cloak layer: a plain cloak is drawn
    /// under the robe and a quiver over it.
    #[test]
    fn a_quiver_is_drawn_over_the_robe_and_a_cloak_under_it() {
        let plain = order(&[worn(Layer::CLOAK, 0x100), worn(Layer::ROBE, 0x200)], false);
        assert!(at(&plain, Layer::CLOAK) < at(&plain, Layer::ROBE));

        let quiver = order(&[worn(Layer::CLOAK, 0x380), worn(Layer::ROBE, 0x200)], false);
        assert!(
            at(&quiver, Layer::ROBE) < at(&quiver, Layer::CLOAK),
            "the quiver fix moves the cloak layer after the robe"
        );
    }

    /// The helmet rule's early exit: a helm that covers the neck moves the neck
    /// item after it *and stops*, so the robe rule below it never runs. Both
    /// rules are armed here, and only the first may fire.
    #[test]
    fn the_helmet_rule_stops_the_rules_after_it() {
        let outfit = [
            worn(Layer::HELMET, 0x201),
            worn(Layer::NECKLACE, 0x1C8),
            worn(Layer::ROBE, 0x5E2),
        ];
        let order = order(&outfit, false);
        assert!(at(&order, Layer::HELMET) < at(&order, Layer::NECKLACE));
        assert!(
            at(&order, Layer::ROBE) < at(&order, Layer::HELMET),
            "the robe stayed where the table put it"
        );
    }

    /// `MoveAfter` is a memmove, not a swap: everything between the two ends
    /// slides, and nothing else changes relative order. Pinned on the quiver
    /// rule, which is the one that moves a layer forwards.
    #[test]
    fn moving_a_layer_slides_the_rest_rather_than_swapping() {
        let before = order(&[worn(Layer::ROBE, 0x200)], false);
        let after = order(&[worn(Layer::CLOAK, 0x380), worn(Layer::ROBE, 0x200)], false);

        let others: Vec<Layer> = before.iter().copied().filter(|l| *l != Layer::CLOAK).collect();
        let moved: Vec<Layer> = after.iter().copied().filter(|l| *l != Layer::CLOAK).collect();
        assert_eq!(others, moved, "only the cloak moved");
    }

    /// Decision 4: the body picture is chosen from the body id, and one body
    /// carries a hue that is not its own.
    #[test]
    fn the_body_picture_is_the_body_id_and_one_of_them_brings_a_hue() {
        assert_eq!(body_gump(0x0190, Hue(7)), (Graphic(0x000C), Hue(7)));
        assert_eq!(body_gump(0x0191, Hue(7)), (Graphic(0x000D), Hue(7)));
        assert_eq!(
            body_gump(0x03DB, Hue(7)),
            (Graphic(0x000C), Hue(0x03EA)),
            "the one body drawn in a hue of the client's choosing"
        );
        assert_eq!(
            body_gump(0x0031, Hue(7)),
            (Graphic(0x000C), Hue(7)),
            "a body nobody has a doll for is drawn male, not skipped"
        );
    }

    /// The gump column is read, and it is read as the *paperdoll's* override:
    /// the animation column of the same row says something else and must not
    /// reach this side. No `Gumps` here — the file is the fallback's business,
    /// and this is about which number the offset is applied to.
    #[test]
    fn the_equipconv_gump_column_overrides_the_anim_id_a_paperdoll_draws() {
        let table = EquipConv::parse("400\t100\t200\t300\t0\n");
        let entry = table.resolve(400, 100).expect("the row above");
        assert_eq!(entry.graphic, Graphic(200), "the animation's override");
        assert_eq!(entry.gump, Graphic(300), "the paperdoll's, and they differ");
    }
}
