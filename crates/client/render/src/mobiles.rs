//! Placing a creature's animation frame on the screen.
//!
//! The CPU side, as [`crate::statics`] is for what stands on the ground — and
//! deliberately smaller, because a mobile is not read out of a file the way a
//! static is. The map says where every static is; nothing says where a mobile
//! is except the server, so what arrives here is a list somebody else built.
//!
//! # A mobile is placed from its frame, not from its tile
//!
//! A static sits in the middle of its cell. A mobile's sprite is placed by the
//! two centre offsets stored with the frame — `MobileView.Draw`:
//!
//! ```text
//! x -= flipped ? width - center_x : center_x
//! y -= height + center_y
//! ```
//!
//! against the cell's centre. That asymmetry is what the mirrored facings are
//! for: flipping a picture moves its anchor to the other edge, and using
//! `center_x` for both makes every west-facing creature stand a body's width
//! from where it is.
//!
//! # And between two tiles while it is walking
//!
//! The wire moves a body a whole tile at a time, so a mobile drawn where the
//! last packet put it teleports 44 pixels every step. [`Mobile::drawn`] is the
//! difference: the sub-pixel position the sprite actually hangs at, which
//! mid-step is between two tiles and under an eased rig is behind the tile
//! altogether. Nothing else changes — the *tile* is still where the server put
//! it, which is what everything that is not a pixel keeps asking.
//!
//! **What that position is a function of is deliberately not here.** A step's
//! progress is a clock, an ease is a filter with state, and both belong to the
//! layer that ages what it sees — `client/app`'s `crowd.rs`. This crate reads no
//! clocks (see the crate docs), so a mobile arrives with its position already
//! decided and the renderer places a picture at it. `docs/camera.md` D10 is the
//! argument for the ease living on the body rather than on the eye.

use openshard_protocol::direction::Direction;
use openshard_protocol::wire::{Hue, Layer};
use openshard_protocol::world::Point;
use openshard_uofiles::equipconv::EquipConv;

use crate::atlas::{AnimAtlas, FrameKey};
use crate::camera::{Camera, ViewPixel, WorldPoint};
use crate::cutaway::Cutaway;
use crate::depth;
use crate::follow::Gaze;
use crate::geometry::{Rect, Vec2};
use crate::sprite::SpriteQuad;

/// One creature to draw, as the client knows it.
///
/// Everything here comes from the server — `0x77`, `0x78` and `0x20` — except
/// the frame, which is whatever the animation has advanced to, and
/// [`Mobile::drawn`], which is *history*: only the previous packet says a body
/// moved. Deliberately a plain value rather than a handle into a `WorldView`:
/// this crate renders what it is given and owns no model of the world.
#[derive(Clone, PartialEq, Debug)]
pub struct Mobile {
    /// The tile the server put it on — or, mid-step, the one it is going to.
    ///
    /// Everything that is not a pixel reads this and not [`Mobile::drawn`]: the
    /// height the body is sorted at, and — with [`Mobile::from`] — which tile
    /// depth it sorts at. A step moves the order once, at the boundary, never
    /// by a fraction.
    pub at: Point,
    /// Its body id.
    pub body: u16,
    /// Which animation group is playing.
    pub group: u8,
    /// Which way it is looking.
    ///
    /// The facing, not the stored direction: turning one into the other is
    /// [`openshard_uofiles::anim::facing`], and doing it here rather than in the
    /// caller is what keeps the mirror and the placement from being decided in
    /// two different places.
    pub facing: Direction,
    /// Which frame of that animation.
    pub frame: u16,
    /// The tile it is stepping *off*, while a step is in flight — `None` for a
    /// body standing still.
    ///
    /// Only the depth order reads it, and only through [`depth::mobile_tile`],
    /// which is where the reason is: a sprite mid-step covers both tiles, so it
    /// has to sort at the nearer of them or the ground it is walking off is
    /// drawn over it. Not derived from [`Mobile::drawn`] — an eased body lags
    /// its tile even when it is standing (`docs/camera.md` D10), so the pixel
    /// offset cannot tell "walking" from "settling", and this is a question
    /// about the step and not about the picture.
    pub from: Option<Point>,
    /// Its hue, or [`Hue::NONE`] for none.
    pub hue: Hue,
    /// Where to actually draw it, sub-pixel and with its height kept apart.
    ///
    /// **Not derived from [`Mobile::at`], and that is the point.** A body
    /// between two tiles is part way through a step; a body being eased is
    /// behind its tile by whatever the filter is holding (`docs/camera.md` D10).
    /// Neither is a function of the tile and both are a function of a clock, so
    /// the caller that owns the clock owns this — `client/app`'s `crowd.rs` — and
    /// this crate draws where it is told.
    ///
    /// A [`Gaze`] and not a [`WorldPixel`] because the camera filters it: the
    /// height is kept out of the vertical axis so it can have its own clock, and
    /// the rounding happens once, at the end (D2, D7).
    pub drawn: Gaze,
    /// What it is wearing. Not the item's own wire graphic — its tiledata
    /// `AnimID`, already resolved by the caller (this crate has no tiledata
    /// reference), because that is what a worn item draws with *by default*.
    /// [`collect`] only asks [`EquipConv`] whether this mobile's body wants a
    /// *different* picture for it.
    pub equipment: Vec<EquipmentLayer>,
}

/// One worn item, ready to place: an `AnimID`, not a picture.
///
/// Drawn in this order and no other — there is no paperdoll layer-ordering
/// table in this engine (see [`openshard_protocol::wire::Layer`]'s own doc
/// comment), so items layer in the order the server listed them in, which is
/// usually close to right and not guaranteed to be.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EquipmentLayer {
    /// The body-animation-space graphic this item draws with when
    /// [`EquipConv`] has nothing to say about it — its own tiledata `AnimID`,
    /// not its wire graphic. [`EquipConv::resolve`] is keyed on this same
    /// number, not on the wire graphic either.
    pub graphic: u16,
    /// Its hue, or [`Hue::NONE`] for none.
    pub hue: Hue,
    /// Which slot it is worn on.
    ///
    /// Carried because the *picture* is not enough to draw a body correctly:
    /// the reference skips hair and a beard on the dead ([`worn_graphic`]), and
    /// nothing about a hair graphic says it is hair. It is also the field a
    /// paperdoll's layer ordering needs — see `docs/client.md` — which is why
    /// it is the wire's [`Layer`] and not a smaller local enum: the ordering
    /// table is written against these numbers.
    pub layer: Layer,
}

/// Where a mobile is asking to be looked at.
///
/// One line, and it stays a function rather than a field access because the
/// camera and the sprite must read the *same* number: a body and an eye that
/// round separately disagree by a pixel, and the disagreement is a shimmer
/// nobody can name.
pub fn gaze(mobile: &Mobile) -> Gaze {
    mobile.drawn
}

/// Where a mobile's sprite lands in the world's own pixel space.
///
/// World pixels and not view pixels, and public for it: the camera that follows
/// a body has to follow it *between* tiles too, or the world jumps a tile at a
/// time under a character sliding smoothly across it — and an eye is a
/// [`WorldPoint`], with no camera to convert with.
pub fn world_position(mobile: &Mobile) -> WorldPoint {
    gaze(mobile).eye()
}

/// The same, where the camera puts it in the drawn image, to a fraction.
///
/// **Snapped to the same lattice the eye is on**, which is the half of
/// `docs/camera.md` D11 that is not about the camera. Two things decide where a
/// sprite lands on the display — where the body is and where the eye is — and
/// if only one of them is on the real pixel's grid, the difference is not: the
/// sprite is resampled by a fraction of a texel against a world that is not, so
/// its texels change width as it walks. Under `Rig::HARD` the two are the same
/// number and this is a no-op; under any easing rig, and under D10's eased
/// *body*, they are not.
///
/// Fractional and not a [`ViewPixel`], because a third of a virtual pixel is a
/// whole real one at `3x`: rounding here would put back exactly the quantum the
/// snap above was chosen to keep.
fn cell_centre(mobile: &Mobile, camera: &Camera) -> Vec2 {
    camera.to_view_exact(camera.snap(world_position(mobile)))
}

/// The body, group and stored direction a set of mobiles needs packed.
///
/// Called before building the atlas, like its neighbours: the atlas has to
/// exist before a quad can be given a region. The frame index is absent on
/// purpose — [`AnimAtlas::build`] packs a whole animation at a time, because
/// reading one frame of it costs the same as reading all of them.
///
/// Also yields every equipment layer's *resolved* body-anim triple, so the
/// atlas has what a worn item needs before [`collect`] asks for it — its own
/// `AnimID` ordinarily, or [`EquipConv`]'s override where this body has one.
pub fn needed_animations(mobiles: &[Mobile], equip_conv: &EquipConv) -> Vec<(u16, u8, u8)> {
    let mut wanted: Vec<(u16, u8, u8)> = Vec::new();
    for mobile in mobiles {
        let (direction, _) = openshard_uofiles::anim::facing(mobile.facing);
        // The body the *file* holds, which for a ghost is the living body it is
        // drawn from — see `anim::animation_body`. Packed under that key, so
        // `place` below finds it under the same one.
        wanted.push((
            openshard_uofiles::anim::animation_body(mobile.body),
            mobile.group,
            direction,
        ));
        for layer in &mobile.equipment {
            if let Some((graphic, _)) = worn_graphic(mobile, *layer, equip_conv) {
                wanted.push((graphic, mobile.group, direction));
            }
        }
    }
    wanted.sort_unstable();
    wanted.dedup();
    wanted
}

/// What one worn layer draws with, and what hue — or `None` for a layer that
/// draws nothing on a body at all.
///
/// The `None` is the whole reason this is a function rather than two lines at
/// each call site. An `AnimID` of zero means "this item has no picture in
/// animation space" — a backpack, a ring, anything that only ever shows on a
/// paperdoll — and `MobileView.Draw` guards every layer with
/// `if (item.ItemData.AnimID != 0)` for it. Without that guard the zero is not
/// inert: body 0 is a real creature in `anim.mul`, the first monster in the
/// index, so the layer packs and draws *its* frame under the wearer. It reads
/// as somebody else's legs walking about beside the character, which is
/// exactly what it was found as.
///
/// The other `None` is the dead. A shard dresses a ghost in whatever it was
/// wearing that death does not take off — for a relogged player that is a death
/// shroud, its backpack and *its hair* — and the reference draws neither hair
/// nor beard on a corpse or a ghost:
/// `IsDead && (layer == Layer.Hair || layer == Layer.Beard)` in `MobileView.Draw`.
/// So a ghost is bald under its hood, and this is the one place that decides it,
/// which is what [`EquipmentLayer::layer`] exists for.
///
/// Shared by [`needed_animations`] and [`collect`] because they must agree on
/// every part of this — which layers exist and which graphic each one is under.
/// A layer packed and not drawn is waste; a layer drawn and not packed is a
/// hole.
fn worn_graphic(mobile: &Mobile, layer: EquipmentLayer, equip_conv: &EquipConv) -> Option<(u16, Hue)> {
    if layer.graphic == 0 {
        return None;
    }
    if openshard_uofiles::anim::is_ghost(mobile.body)
        && (layer.layer == Layer::HAIR || layer.layer == Layer::BEARD)
    {
        return None;
    }
    // Keyed on the body the *shard* named and not on the one the animation is
    // read under: `EquipConversions` is indexed by `Mobile.Graphic` in the
    // reference too, and it is a table about what a *kind of creature* wears,
    // not about which file the frames come from.
    let entry = equip_conv.resolve(mobile.body, layer.graphic);
    let graphic = entry.map_or(layer.graphic, |entry| entry.graphic.0);
    // The table's own colour, but only where the item does not carry one: a
    // dyed item keeps its dye.
    let hue = match (layer.hue == Hue::NONE, entry) {
        (true, Some(entry)) => entry.color,
        _ => layer.hue,
    };
    Some((graphic, hue))
}

/// Where a mobile's sprite lands, before hue and hold decide anything about
/// how it is drawn.
///
/// Shared by [`collect`] and [`head_anchor`]: both need the same rectangle —
/// one to draw it, the other to hang a label above it — and computing it twice
/// is two chances for the anchor arithmetic to disagree with itself.
struct Placement {
    rect: Rect,
    region: crate::atlas::Region,
    order: depth::Order,
    /// Which frame of the atlas it is showing. Carried so [`pick`] can ask that
    /// frame's own texels whether the cursor hit the picture, rather than
    /// re-deriving the key from the mobile and drifting from what was drawn.
    key: FrameKey,
    /// Whether the picture is sampled right to left. The atlas holds one
    /// picture for both facings, so a texel test has to undo the flip — see
    /// [`AnimAtlas::opaque_at`].
    mirrored: bool,
}

/// The mobile's frame in the atlas, placed on screen — or `None` if the atlas
/// holds no such frame.
///
/// `body` is the atlas key's body id and not always [`Mobile::body`], for two
/// reasons. An equipment layer's picture lives under its *resolved* body-anim
/// graphic (see [`EquipConv`]), read at the same group, direction and frame as
/// the mobile wearing it — a worn item has no clock of its own. And a body the
/// files hold no animation for is read under the one they do:
/// `openshard_uofiles::anim::animation_body`, which is what draws a ghost. Every
/// caller passes the key it packed with, so this never re-derives either.
fn place(mobile: &Mobile, body: u16, camera: &Camera, atlas: &AnimAtlas) -> Option<Placement> {
    let (direction, mirrored) = openshard_uofiles::anim::facing(mobile.facing);
    let key = FrameKey {
        body,
        group: mobile.group,
        direction,
        frame: mobile.frame,
    };
    let packed = atlas.frame(key)?;

    // Tiles and not the pixels it is between: the order steps once, at the tile
    // boundary, where an interpolated one would have a mobile change sides of a
    // wall in the middle of a step. Which of the two tiles a step is between is
    // `depth::mobile_tile`'s to say.
    let order = depth::Order {
        tile: depth::mobile_tile(mobile.at, mobile.from),
        priority_z: depth::mobile_priority_z(mobile.at.z),
    };
    let at = cell_centre(mobile, camera);
    let (width, height) = (i32::from(packed.sprite.width), i32::from(packed.sprite.height));
    // The anchor moves to the far edge when the picture is flipped, which is
    // `MobileView.Draw`'s `isFlipped ? width - center_x : center_x`.
    let anchor_x = if mirrored {
        width - i32::from(packed.center_x)
    } else {
        i32::from(packed.center_x)
    };
    let region = if mirrored {
        SpriteQuad::mirrored(packed.sprite.region)
    } else {
        packed.sprite.region
    };

    Some(Placement {
        rect: Rect {
            x: at.x - anchor_x as f32,
            y: at.y - (height + i32::from(packed.center_y)) as f32,
            width: width as f32,
            height: height as f32,
        },
        region,
        order,
        key,
        mirrored,
    })
}

/// The quads for every mobile whose frame the atlas holds.
///
/// A mobile the atlas has no frame for is dropped: the client ships no
/// animation for that body and group, or the atlas was built before this one
/// arrived. Both are "nothing to draw", and neither is worth failing a frame
/// over — the alternative is a creature drawn from another creature's picture.
///
/// `cutaway` hides a body on the storey above with that storey. It is the same
/// `max_z` the statics are tested against and deliberately not `no_draw_roofs`:
/// a mobile is never a roof, and the client asks it the one question.
///
/// A mobile's equipment draws over its body for free, without a second depth
/// pass: every layer gets the *same* [`depth::Order`] as the body quad, and
/// the sort below is stable, so pushing the body first and its layers after —
/// in wire order, there being no layer-ordering table here — is what keeps
/// them on top. A layer draws with its own `AnimID`
/// ([`EquipmentLayer::graphic`]) unless [`EquipConv`] overrides it for this
/// body; only a resolved graphic the atlas has no frame for this frame is
/// dropped, the same rule a missing body animation gets.
///
/// `highlight` is the index [`pick`] answered with — the creature the cursor is
/// over, drawn in [`items::HIGHLIGHT_HUE`](crate::items::HIGHLIGHT_HUE) instead
/// of its own hue, body and worn layers alike. An index and not a flag on the
/// mobile, for the reason [`items::collect`](crate::items::collect) takes one:
/// being pointed at is a fact about *this frame* and not about the creature.
pub fn collect(
    mobiles: &[Mobile],
    camera: &Camera,
    atlas: &AnimAtlas,
    cutaway: &Cutaway,
    equip_conv: &EquipConv,
    highlight: Option<usize>,
) -> Vec<SpriteQuad> {
    let mut quads: Vec<(depth::Order, SpriteQuad)> = Vec::new();

    for (index, mobile) in mobiles.iter().enumerate() {
        // The highlight replaces the creature's own hue rather than combining
        // with it, exactly as an item's does — the shader has one hue per
        // sprite and a ramp *replaces* the art's colour.
        let hue = match highlight == Some(index) {
            true => Some(crate::items::HIGHLIGHT_HUE),
            false => None,
        };
        push_quads(mobile, camera, atlas, cutaway, equip_conv, hue, &mut quads);
    }

    // Back to front, and a *stable* sort on the order alone: two bodies on one
    // tile at one height keep the caller's order, which is the order the world
    // view holds them in. The depth test is `LessEqual`, so the later one wins
    // the tie — the client's rule, where a mobile is inserted after whatever is
    // already on the tile at its `PriorityZ`.
    quads.sort_by_key(|(order, _)| *order);
    quads.into_iter().map(|(_, quad)| quad).collect()
}

/// One creature's quads — its body, then everything it is wearing — appended to
/// `out` beside the order they all sort at.
///
/// The one copy of "what a mobile is made of on screen", so [`collect`] and
/// [`outlined`] cannot disagree about it. They must not: the silhouette is grown
/// from these quads and the picture from those, and a ring half a pixel off its
/// creature is the defect this exists to make impossible.
///
/// `hue` overrides both the body's and every layer's own colour when it is
/// given; `None` leaves each with what it arrived wearing.
fn push_quads(
    mobile: &Mobile,
    camera: &Camera,
    atlas: &AnimAtlas,
    cutaway: &Cutaway,
    equip_conv: &EquipConv,
    hue: Option<Hue>,
    out: &mut Vec<(depth::Order, SpriteQuad)>,
) {
    if !cutaway.shows_mobile(mobile.at.z) {
        return;
    }
    // Derived from the camera rather than handed in: it is a function of where
    // the eye is and of nothing else, and an argument would be one more thing
    // the two callers could pass differently.
    let (eye_x, eye_y) = camera.eye_tile();
    let base = depth::base_for(eye_x, eye_y);
    let Some(placement) = place(
        mobile,
        openshard_uofiles::anim::animation_body(mobile.body),
        camera,
        atlas,
    ) else {
        return;
    };
    let order = placement.order;
    out.push((
        order,
        SpriteQuad {
            rect: placement.rect,
            region: placement.region,
            depth: order.to_depth(base),
            hue: u32::from(hue.unwrap_or(mobile.hue).0),
            place: crate::place::Place::of_mobile(mobile.at),
            twin: 0,
            // A billboard is no occluder, so it is exempt from nothing — a
            // creature standing on a walled tile is genuinely in that wall's
            // shadow. `docs/lighting_height.md` phase 3, and the one behaviour
            // change it makes on a real frame.
            owner: u32::from(crate::occlusion::OwnerId::NONE.raw()),
        },
    ));

    for layer in &mobile.equipment {
        let Some((graphic, worn_hue)) = worn_graphic(mobile, *layer, equip_conv) else {
            continue;
        };
        let Some(worn) = place(mobile, graphic, camera, atlas) else {
            continue;
        };
        out.push((
            order,
            SpriteQuad {
                rect: worn.rect,
                region: worn.region,
                depth: order.to_depth(base),
                hue: u32::from(hue.unwrap_or(worn_hue).0),
                // The body's tile, not the sprite's: a hat is lit as the
                // head under it is, and it has no tile of its own.
                place: crate::place::Place::of_mobile(mobile.at),
                twin: 0,
                owner: u32::from(crate::occlusion::OwnerId::NONE.raw()),
            },
        ));
    }
}

/// The quads to draw a silhouette from, for the creature the cursor is over.
///
/// **One creature is one ring, and that is why this returns its layers
/// together.** The silhouette pass numbers *groups*, not quads (see
/// [`SpriteRenderer::render_mask`](crate::renderer::SpriteRenderer::render_mask)):
/// a body and the tunic over it drawn under two ids would have the ring pass
/// find a boundary between them and draw an edge along every seam of the
/// clothing, which is a creature drawn as a diagram of what it is wearing.
///
/// The quads are the *same* quads [`collect`] draws, through the same
/// [`push_quads`], because the silhouette has to land exactly on the picture.
/// The hues are never read: the silhouette shader wants the shape, and the
/// shape is the alpha.
///
/// `highlight` is what [`pick`] answered. `None` comes back empty rather than
/// being a case the caller has to handle.
pub fn outlined(
    mobiles: &[Mobile],
    camera: &Camera,
    atlas: &AnimAtlas,
    cutaway: &Cutaway,
    equip_conv: &EquipConv,
    highlight: Option<usize>,
) -> Vec<SpriteQuad> {
    let mut quads: Vec<(depth::Order, SpriteQuad)> = Vec::new();
    if let Some(mobile) = highlight.and_then(|index| mobiles.get(index)) {
        push_quads(mobile, camera, atlas, cutaway, equip_conv, None, &mut quads);
    }
    quads.into_iter().map(|(_, quad)| quad).collect()
}

/// Which creature the cursor is over: an index into `mobiles`, or `None`.
///
/// [`items::pick`](crate::items::pick)'s rules, against animation frames:
///
/// - **The picture is what is hit, not the tile.** A dragon's sprite covers
///   several tiles' worth of screen and stands on one of them; picking by tile
///   would make most of a creature unclickable and the ground behind it
///   clickable through its body.
/// - **A hit is an opaque texel** — see [`AnimAtlas::opaque_at`]. A creature's
///   frame is mostly empty air, and a box test picks the gap under a rider's
///   arm.
/// - **Equipment counts as the wearer.** A hat is not a thing standing beside
///   the creature; a cursor on it is a cursor on the creature, and it is the
///   *wearer's* index that comes back.
/// - **The topmost drawn wins**, which is the largest [`depth::Order`] and on a
///   tie the later mobile in `mobiles` — what the depth test does to two bodies
///   at one order, see [`collect`]'s sort.
///
/// `cursor` is a viewport pixel, the same pair `winit` reports.
#[must_use]
pub fn pick(
    mobiles: &[Mobile],
    camera: &Camera,
    atlas: &AnimAtlas,
    cutaway: &Cutaway,
    equip_conv: &EquipConv,
    cursor: (i32, i32),
) -> Option<usize> {
    let in_view = camera.to_view(camera.pick(cursor.0, cursor.1));
    let mut hit: Option<(depth::Order, usize)> = None;
    for (index, mobile) in mobiles.iter().enumerate() {
        if !cutaway.shows_mobile(mobile.at.z) {
            continue;
        }
        // The body first, then what it wears: any one of them is the creature.
        let body = openshard_uofiles::anim::animation_body(mobile.body);
        let worn = mobile
            .equipment
            .iter()
            .filter_map(|layer| worn_graphic(mobile, *layer, equip_conv).map(|(graphic, _)| graphic));
        let mut touched = None;
        for graphic in std::iter::once(body).chain(worn) {
            let Some(placement) = place(mobile, graphic, camera, atlas) else {
                continue;
            };
            if !opaque_under(&placement, atlas, in_view) {
                continue;
            }
            touched = Some(placement.order);
            break;
        }
        let Some(order) = touched else {
            continue;
        };
        // `>=`, so a later mobile at the same order takes it: the tie-break is
        // the caller's order, and the one drawn last is the one on top.
        if hit.is_none_or(|(best, _)| order >= best) {
            hit = Some((order, index));
        }
    }
    hit.map(|(_, index)| index)
}

/// Whether a placed frame has a drawn texel under a point of the drawn image.
///
/// The flip is undone here and nowhere else: the atlas holds one picture for
/// both facings, so a mirrored sprite's leftmost *drawn* pixel is its picture's
/// rightmost one. Asked without it, half the creatures on screen are pickable
/// along the silhouette of their own mirror image.
fn opaque_under(placement: &Placement, atlas: &AnimAtlas, in_view: ViewPixel) -> bool {
    // Into the sprite's own pixels. Negative is above or left of it, and
    // `try_from` failing is the whole of that test.
    let (Ok(x), Ok(y)) = (
        u16::try_from(in_view.x - placement.rect.x as i32),
        u16::try_from(in_view.y - placement.rect.y as i32),
    ) else {
        return false;
    };
    let width = placement.rect.width as u16;
    if x >= width {
        return false;
    }
    let texel_x = match placement.mirrored {
        true => width - 1 - x,
        false => x,
    };
    atlas.opaque_at(placement.key, texel_x, y)
}

/// Where a label belongs above this mobile's head, in view pixels — the
/// horizontal centre of its sprite and its topmost edge, or `None` if
/// [`collect`] would draw nothing for it either.
///
/// Not a fixed offset above [`cell_centre`]: a dragon and a rat stand on the
/// same tile centre and hold their heads at wildly different heights, and only
/// the packed frame's own rectangle knows which.
pub fn head_anchor(mobile: &Mobile, camera: &Camera, atlas: &AnimAtlas) -> Option<ViewPixel> {
    let placement = place(
        mobile,
        openshard_uofiles::anim::animation_body(mobile.body),
        camera,
        atlas,
    )?;
    Some(ViewPixel {
        x: (placement.rect.x + placement.rect.width / 2.0).round() as i32,
        y: placement.rect.y.round() as i32,
    })
}

#[cfg(test)]
mod tests {
    use openshard_uofiles::anim::AnimFrame;
    use openshard_uofiles::color::Color16;
    use openshard_uofiles::image::Image;

    use super::*;

    /// A frame packed at a known size, for placing.
    fn atlas(body: u16, direction: u8, width: u16, height: u16, center: (i16, i16)) -> AnimAtlas {
        AnimAtlas::pack([(
            FrameKey {
                body,
                group: 4,
                direction,
                frame: 0,
            },
            AnimFrame {
                center_x: center.0,
                center_y: center.1,
                image: Image::new(
                    width,
                    height,
                    vec![Color16(0x7C00); usize::from(width) * usize::from(height)],
                ),
            },
        )])
        .expect("one frame fits")
    }

    /// A table with no entries — every test below except the one for
    /// equipment itself draws bare bodies.
    fn no_equip() -> EquipConv {
        EquipConv::default()
    }

    /// A worn layer in an ordinary slot — `Layer.InnerTorso`, a shirt.
    ///
    /// The slot is only ever asked about for hair and a beard (see
    /// [`worn_graphic`]), so everything else stands for "a piece of clothing"
    /// and the tests below say so once, here, rather than choosing a number each.
    fn worn_on_torso(graphic: u16, hue: Hue) -> EquipmentLayer {
        EquipmentLayer {
            graphic,
            hue,
            layer: Layer(0x05),
        }
    }

    /// A body standing still on its tile, hue and equipment free.
    fn body_at(x: u16, facing: Direction) -> Mobile {
        Mobile {
            at: Point::new(x, 100, 0),
            body: 400,
            group: 4,
            facing,
            frame: 0,
            from: None,
            hue: Hue::NONE,
            drawn: Gaze::on(Point::new(x, 100, 0)),
            equipment: Vec::new(),
        }
    }

    /// The viewport pixel a point in the drawn image sits at — the inverse of
    /// what [`pick`] undoes, so a test can click on a sprite it has placed.
    fn cursor_over(camera: &Camera, at: Rect, dx: f32, dy: f32) -> (i32, i32) {
        let spot = camera.to_viewport(ViewPixel {
            x: (at.x + dx) as i32,
            y: (at.y + dy) as i32,
        });
        (spot.x as i32, spot.y as i32)
    }

    /// Where a mobile's body lands, asked the way the passes ask it.
    fn placed(mobile: &Mobile, camera: &Camera, atlas: &AnimAtlas) -> Rect {
        place(
            mobile,
            openshard_uofiles::anim::animation_body(mobile.body),
            camera,
            atlas,
        )
        .expect("the frame is packed")
        .rect
    }

    /// A frame with a hole in it: the left half transparent, the right half
    /// drawn. Most animation frames are mostly empty air, which is the whole
    /// reason picking is a texel test.
    fn holed(body: u16, direction: u8, width: u16, height: u16) -> AnimAtlas {
        let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height));
        for _ in 0..height {
            for x in 0..width {
                pixels.push(match x < width / 2 {
                    true => Color16::TRANSPARENT,
                    false => Color16(0x7C00),
                });
            }
        }
        AnimAtlas::pack([(
            FrameKey {
                body,
                group: 4,
                direction,
                frame: 0,
            },
            AnimFrame {
                center_x: 12,
                center_y: -3,
                image: Image::new(width, height, pixels),
            },
        )])
        .expect("one frame fits")
    }

    /// A click on a creature's own pixels picks it, and one on the empty air
    /// inside its rectangle does not.
    #[test]
    fn a_click_on_a_mobile_s_own_pixels_picks_it() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        // Facing 3 is stored direction 0, unflipped.
        let atlas = holed(400, 0, 40, 60);
        let mobile = body_at(100, Direction::SouthEast);
        let at = placed(&mobile, &camera, &atlas);
        let pick_at = |dx, dy| {
            pick(
                std::slice::from_ref(&mobile),
                &camera,
                &atlas,
                &Cutaway::OPEN,
                &no_equip(),
                cursor_over(&camera, at, dx, dy),
            )
        };
        assert_eq!(pick_at(30.0, 30.0), Some(0), "the drawn half was not hit");
        assert_eq!(
            pick_at(5.0, 30.0),
            None,
            "the transparent half was picked — this is a box test, not a texel one"
        );
        assert_eq!(pick_at(-5.0, 30.0), None, "a pixel left of the sprite was picked");
        assert_eq!(pick_at(30.0, 70.0), None, "a pixel below the sprite was picked");
    }

    /// A mirrored facing is picked against the picture as *drawn*, not as
    /// stored. Half the creatures on screen face a mirrored way, and asking the
    /// atlas without undoing the flip makes them pickable along the silhouette
    /// of their own mirror image — clickable where they are transparent and
    /// dead where they are drawn.
    #[test]
    fn a_mirrored_facing_is_picked_through_the_flip() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        // Facings 2 and 4 share stored direction 1; 2 is the flipped one.
        let atlas = holed(400, 1, 40, 60);
        let mobile = body_at(100, Direction::East);
        let at = placed(&mobile, &camera, &atlas);
        let pick_at = |dx| {
            pick(
                std::slice::from_ref(&mobile),
                &camera,
                &atlas,
                &Cutaway::OPEN,
                &no_equip(),
                cursor_over(&camera, at, dx, 30.0),
            )
        };
        // The art's drawn half is its right; flipped, it is drawn on the left.
        assert_eq!(pick_at(5.0), Some(0), "the flipped picture's drawn half");
        assert_eq!(pick_at(30.0), None, "the flipped picture's transparent half");
    }

    /// A cursor on a hat is a cursor on whoever is wearing it — the wearer's
    /// index comes back, not a second object's.
    #[test]
    fn a_click_on_worn_equipment_picks_its_wearer() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        // The body is transparent on its left half and the robe is solid, so a
        // hit on that half can only have come from the layer.
        let mut atlas = holed(400, 0, 40, 60);
        atlas
            .pack_more([(
                FrameKey {
                    body: 7005,
                    group: 4,
                    direction: 0,
                    frame: 0,
                },
                AnimFrame {
                    center_x: 12,
                    center_y: -3,
                    image: Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            )])
            .expect("both frames fit");
        let mobile = Mobile {
            equipment: vec![worn_on_torso(7005, Hue::NONE)],
            ..body_at(100, Direction::SouthEast)
        };
        let at = placed(&mobile, &camera, &atlas);
        assert_eq!(
            pick(
                std::slice::from_ref(&mobile),
                &camera,
                &atlas,
                &Cutaway::OPEN,
                &no_equip(),
                cursor_over(&camera, at, 5.0, 30.0),
            ),
            Some(0),
            "the robe's own pixels did not count as the creature",
        );
    }

    /// Two creatures overlapping on screen: the one drawn on top takes the
    /// click, which is the answer the depth buffer gives the frame.
    #[test]
    fn the_topmost_mobile_wins_an_overlap() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = atlas(400, 0, 44, 120, (22, 0));
        // Given furthest first, which is *not* what decides it: the order is.
        let mobiles = [
            body_at(100, Direction::SouthEast),
            body_at(101, Direction::SouthEast),
        ];
        let near = placed(&mobiles[1], &camera, &atlas);
        assert_eq!(
            pick(
                &mobiles,
                &camera,
                &atlas,
                &Cutaway::OPEN,
                &no_equip(),
                // Inside the near sprite's top strip, over the far one's body.
                cursor_over(&camera, near, 22.0, 10.0),
            ),
            Some(1),
            "the creature behind was picked through the one in front",
        );
    }

    /// The creature the cursor is over is drawn in the highlight ramp — body
    /// and worn layers alike, so a robe is not left in its own colour round a
    /// highlighted body.
    #[test]
    fn the_highlighted_mobile_is_drawn_in_the_highlight_hue() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let mut atlas = atlas(400, 0, 40, 60, (12, -3));
        atlas
            .pack_more([(
                FrameKey {
                    body: 7005,
                    group: 4,
                    direction: 0,
                    frame: 0,
                },
                AnimFrame {
                    center_x: 12,
                    center_y: -3,
                    image: Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            )])
            .expect("both frames fit");
        // Their own hues, so the assertion is "replaced" and not "set".
        let dressed = |x: u16| Mobile {
            hue: Hue(0x03B2),
            equipment: vec![worn_on_torso(7005, Hue(0x0455))],
            ..body_at(x, Direction::SouthEast)
        };
        let quads = collect(
            &[dressed(100), dressed(101)],
            &camera,
            &atlas,
            &Cutaway::OPEN,
            &no_equip(),
            Some(1),
        );
        assert_eq!(quads.len(), 4, "two bodies and a robe each");
        // Back to front, so the nearer creature — index 1 — is the last pair.
        assert_eq!(
            (quads[2].hue, quads[3].hue),
            (
                u32::from(crate::items::HIGHLIGHT_HUE.0),
                u32::from(crate::items::HIGHLIGHT_HUE.0)
            ),
            "the pointed-at creature, robe and all",
        );
        assert_eq!((quads[0].hue, quads[1].hue), (0x03B2, 0x0455), "and nothing else");
    }

    /// The silhouette is the picture's own rectangles, so the ring lands on the
    /// creature rather than beside it. The assertion is the *comparison*: two
    /// numbers here would go on passing if the two paths drifted apart.
    #[test]
    fn the_outline_quads_are_the_drawn_quads() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = atlas(400, 0, 40, 60, (12, -3));
        let mobiles = [
            body_at(100, Direction::SouthEast),
            body_at(101, Direction::SouthEast),
        ];
        let drawn = collect(&mobiles, &camera, &atlas, &Cutaway::OPEN, &no_equip(), None);
        let ringed = outlined(&mobiles, &camera, &atlas, &Cutaway::OPEN, &no_equip(), Some(0));
        assert_eq!(ringed.len(), 1, "one creature, one silhouette");
        // Index 0 is the further creature, which the sort puts first.
        assert_eq!(ringed[0].rect, drawn[0].rect);
        assert_eq!(ringed[0].depth, drawn[0].depth);
        assert!(
            outlined(&mobiles, &camera, &atlas, &Cutaway::OPEN, &no_equip(), None).is_empty(),
            "nothing pointed at is an empty list and not a case to handle",
        );
    }

    /// The placement arithmetic, in numbers rather than in a picture.
    ///
    /// Two ways to get this wrong look identical on a screenshot until a mobile
    /// walks: anchoring by the sprite's own corner instead of its centre, and
    /// anchoring a flipped sprite by the same edge as an unflipped one. The
    /// second is asserted below as the *difference* between the two facings,
    /// which is the only place it shows.
    #[test]
    fn a_mobile_hangs_from_its_frames_centre() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = atlas(400, 0, 40, 60, (12, -3));
        // Facing 3 is the stored direction 0, unflipped.
        let quads = collect(
            &[Mobile {
                at: Point::new(100, 100, 0),
                body: 400,
                group: 4,
                facing: Direction::SouthEast,
                frame: 0,
                from: None,
                hue: Hue::NONE,
                drawn: Gaze::on(Point::new(100, 100, 0)),
                equipment: Vec::new(),
            }],
            &camera,
            &atlas,
            &Cutaway::OPEN,
            &no_equip(),
            None,
        );
        assert_eq!(quads.len(), 1);
        // The camera puts its own tile's centre at (400, 300).
        assert_eq!(quads[0].rect.x, 400.0 - 12.0);
        assert_eq!(quads[0].rect.y, 300.0 - (60.0 - 3.0));
        assert_eq!(quads[0].rect.width, 40.0);
    }

    /// A mirrored facing samples its picture backwards and hangs from the other
    /// edge. Both halves, because either alone draws a creature that faces the
    /// right way and stands in the wrong place — or the reverse.
    #[test]
    fn a_mirrored_facing_flips_the_picture_and_the_anchor() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        // Facings 2 and 4 share stored direction 1; 2 is the flipped one.
        let atlas = atlas(400, 1, 40, 60, (12, -3));
        let quads = |facing: Direction| {
            collect(
                &[Mobile {
                    at: Point::new(100, 100, 0),
                    body: 400,
                    group: 4,
                    facing,
                    frame: 0,
                    from: None,
                    hue: Hue::NONE,
                    drawn: Gaze::on(Point::new(100, 100, 0)),
                    equipment: Vec::new(),
                }],
                &camera,
                &atlas,
                &Cutaway::OPEN,
                &no_equip(),
                None,
            )
        };
        let plain = quads(Direction::South);
        let flipped = quads(Direction::East);
        assert_eq!(plain.len(), 1);
        assert_eq!(flipped.len(), 1);

        assert_eq!(plain[0].rect.x, 400.0 - 12.0);
        assert_eq!(
            flipped[0].rect.x,
            400.0 - (40.0 - 12.0),
            "the anchor is the far edge"
        );
        assert_eq!(plain[0].rect.y, flipped[0].rect.y, "only x is mirrored");
        assert!(flipped[0].region.du < 0.0, "the picture is sampled backwards");
        assert!(plain[0].region.du > 0.0);
    }

    /// A mobile the atlas has no frame for is dropped rather than drawn from
    /// whatever else was packed. Which matters more here than for statics: the
    /// atlas is keyed by four numbers, so a near miss is another creature.
    #[test]
    fn a_mobile_with_no_packed_frame_is_dropped() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = atlas(400, 0, 40, 60, (12, -3));
        let missing = Mobile {
            at: Point::new(100, 100, 0),
            body: 400,
            group: 4,
            facing: Direction::SouthEast,
            // One past the only frame packed.
            frame: 1,
            from: None,
            hue: Hue::NONE,
            drawn: Gaze::on(Point::new(100, 100, 0)),
            equipment: Vec::new(),
        };
        assert!(
            collect(
                std::slice::from_ref(&missing),
                &camera,
                &atlas,
                &Cutaway::OPEN,
                &no_equip(),
                None
            )
            .is_empty()
        );
        // And the same mobile at frame 0 is not, so the drop above is a
        // decision about the frame rather than about the body.
        assert_eq!(
            collect(
                &[Mobile { frame: 0, ..missing }],
                &camera,
                &atlas,
                &Cutaway::OPEN,
                &no_equip(),
                None
            )
            .len(),
            1,
        );
    }

    /// A ghost is packed and looked up under one key: the living body it
    /// borrows its pictures from.
    ///
    /// The two halves have to be asserted together, because either alone is
    /// green while the client draws nothing. `needed_animations` names what the
    /// atlas holds and `collect` asks for what it draws, and the defect this is
    /// written against is the pair disagreeing: `anim.mul` has no block for
    /// body `0x0192` at all, so an atlas packed under the wire's body is an
    /// atlas with nothing in it and the mobile is dropped in silence.
    ///
    /// The same disagreement, one field over, is a body that is drawn and never
    /// animates: a frame count asked under a key nothing was packed under is
    /// zero, and a clock with no frames to choose between stops on the first.
    /// That is `client/app`'s call and it uses this same function.
    #[test]
    fn a_ghost_is_packed_and_drawn_under_the_living_body() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let ghost = Mobile {
            at: Point::new(100, 100, 0),
            // 0x0192, the male ghost: a body the client's files have no
            // animation for.
            body: 402,
            group: 4,
            facing: Direction::SouthEast,
            frame: 0,
            from: None,
            hue: Hue::NONE,
            drawn: Gaze::on(Point::new(100, 100, 0)),
            equipment: Vec::new(),
        };
        assert_eq!(
            needed_animations(std::slice::from_ref(&ghost), &no_equip()),
            vec![(400, 4, 0)],
            "the atlas is asked for the living body",
        );
        // And an atlas holding exactly that draws it.
        assert_eq!(
            collect(
                std::slice::from_ref(&ghost),
                &camera,
                &atlas(400, 0, 40, 60, (12, -3)),
                &Cutaway::OPEN,
                &no_equip(),
                None,
            )
            .len(),
            1,
        );
        // While one packed under the body the shard named holds nothing this
        // asks for — which is what the files themselves look like.
        assert!(
            collect(
                std::slice::from_ref(&ghost),
                &camera,
                &atlas(402, 0, 40, 60, (12, -3)),
                &Cutaway::OPEN,
                &no_equip(),
                None,
            )
            .is_empty()
        );
    }

    /// The label anchor is the top-centre of exactly the rectangle `collect`
    /// draws — computed once and read two ways, not two formulas that happen
    /// to agree today.
    #[test]
    fn head_anchor_is_the_drawn_quads_top_centre() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = atlas(400, 0, 40, 60, (12, -3));
        let mobile = Mobile {
            at: Point::new(100, 100, 0),
            body: 400,
            group: 4,
            facing: Direction::SouthEast,
            frame: 0,
            from: None,
            hue: Hue::NONE,
            drawn: Gaze::on(Point::new(100, 100, 0)),
            equipment: Vec::new(),
        };
        let quads = collect(
            std::slice::from_ref(&mobile),
            &camera,
            &atlas,
            &Cutaway::OPEN,
            &no_equip(),
            None,
        );
        let anchor = head_anchor(&mobile, &camera, &atlas).expect("packed");
        assert_eq!(anchor.x as f32, quads[0].rect.x + quads[0].rect.width / 2.0);
        assert_eq!(anchor.y as f32, quads[0].rect.y);
    }

    /// A mobile the atlas has no frame for gets no anchor either — the same
    /// case `collect` drops, asked the other way.
    #[test]
    fn head_anchor_is_none_for_an_unpacked_frame() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = atlas(400, 0, 40, 60, (12, -3));
        let missing = Mobile {
            at: Point::new(100, 100, 0),
            body: 400,
            group: 4,
            facing: Direction::SouthEast,
            frame: 1,
            from: None,
            hue: Hue::NONE,
            drawn: Gaze::on(Point::new(100, 100, 0)),
            equipment: Vec::new(),
        };
        assert!(head_anchor(&missing, &camera, &atlas).is_none());
    }

    /// A body drawn between two tiles still sorts by the tile it is on.
    ///
    /// Which is what keeps it from changing sides of a wall halfway through a
    /// step — the depth buffer would draw it in front for two frames and behind
    /// for the next two. The same rule covers an eased body, which is drawn
    /// *behind* its tile by however much the filter is holding (D10): the tile
    /// is what the server named and the drawn position is a picture.
    #[test]
    fn where_a_body_is_drawn_does_not_move_its_depth_order() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = atlas(400, 0, 40, 60, (12, -3));
        let on_its_tile = Mobile {
            at: Point::new(101, 100, 0),
            body: 400,
            group: 4,
            facing: Direction::SouthEast,
            frame: 0,
            from: None,
            hue: Hue::NONE,
            drawn: Gaze::on(Point::new(101, 100, 0)),
            equipment: Vec::new(),
        };
        let standing = collect(
            std::slice::from_ref(&on_its_tile),
            &camera,
            &atlas,
            &Cutaway::OPEN,
            &no_equip(),
            None,
        );
        // Half a tile back the way it came: a step east is eleven pixels right
        // and eleven down, so half of one is eleven of each.
        let mid_step = collect(
            &[Mobile {
                drawn: Gaze::on(Point::new(101, 100, 0)).back_towards(Gaze::on(Point::new(100, 100, 0)), 0.5),
                ..on_its_tile
            }],
            &camera,
            &atlas,
            &Cutaway::OPEN,
            &no_equip(),
            None,
        );
        assert_eq!(standing.len(), 1);
        assert_eq!(mid_step.len(), 1);
        assert_eq!(standing[0].depth, mid_step[0].depth, "the order is the tile's");
        assert_eq!(
            mid_step[0].rect.x,
            standing[0].rect.x - 11.0,
            "and the sprite has moved"
        );
        assert_eq!(mid_step[0].rect.y, standing[0].rect.y - 11.0);
    }

    /// A body walking *up* the screen stays in front of the ground it is
    /// stepping off, for the whole step.
    ///
    /// The symptom this is here for is the one a player reports as sinking
    /// through the floor: the sprite spans both tiles for the whole crossing,
    /// and sorting it at the destination — which for a northward step is the
    /// *farther* tile — hands the tile behind it, and everything standing on
    /// that tile, the right to be drawn over the walker. It only shows on four
    /// of the eight headings, which is what makes it read as intermittent.
    #[test]
    fn a_body_stepping_north_stays_in_front_of_the_tile_it_is_leaving() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let (north, _) = openshard_uofiles::anim::facing(Direction::North);
        let atlas = atlas(400, north, 40, 60, (12, -3));
        let base = depth::base_for(100, 100);
        // The ground it is walking off, sorted the way `ground::collect` sorts
        // it: the tile's own depth, and the client's land priority.
        let ground_left_behind = depth::Order {
            tile: 200,
            priority_z: depth::land_priority_z([0; 4]),
        }
        .to_depth(base);

        let walking_north = Mobile {
            at: Point::new(100, 99, 0),
            body: 400,
            group: 4,
            facing: Direction::North,
            frame: 0,
            from: Some(Point::new(100, 100, 0)),
            hue: Hue::NONE,
            drawn: Gaze::on(Point::new(100, 99, 0)).back_towards(Gaze::on(Point::new(100, 100, 0)), 0.5),
            equipment: Vec::new(),
        };
        let quads = collect(
            std::slice::from_ref(&walking_north),
            &camera,
            &atlas,
            &Cutaway::OPEN,
            &no_equip(),
            None,
        );
        assert_eq!(quads.len(), 1);
        assert!(
            quads[0].depth < ground_left_behind,
            "the walker is behind the ground it is stepping off: {} is not nearer than {ground_left_behind}",
            quads[0].depth,
        );

        // And once the step is over the body drops back to its own tile, which
        // is what puts it behind a wall on the tile it just left.
        let arrived = collect(
            &[Mobile {
                from: None,
                drawn: Gaze::on(Point::new(100, 99, 0)),
                ..walking_north
            }],
            &camera,
            &atlas,
            &Cutaway::OPEN,
            &no_equip(),
            None,
        );
        assert!(
            arrived[0].depth > quads[0].depth,
            "the order steps at the boundary"
        );
    }

    /// Every distinct animation a set of mobiles needs, once each.
    #[test]
    fn the_needed_animations_are_deduplicated_by_stored_direction() {
        let mobile = |facing: Direction| Mobile {
            at: Point::new(0, 0, 0),
            body: 400,
            group: 4,
            facing,
            frame: 0,
            from: None,
            hue: Hue::NONE,
            drawn: Gaze::on(Point::new(0, 0, 0)),
            equipment: Vec::new(),
        };
        // East and South share a picture, so they are one animation to read.
        let wanted = needed_animations(
            &[
                mobile(Direction::East),
                mobile(Direction::South),
                mobile(Direction::SouthEast),
            ],
            &no_equip(),
        );
        assert_eq!(wanted, vec![(400, 4, 0), (400, 4, 1)]);
    }

    /// An equipment layer draws over the body, resolved through
    /// [`EquipConv`] to its own body-anim graphic and hued from the entry
    /// when the wire hue is [`Hue::NONE`].
    #[test]
    fn a_worn_item_draws_over_the_body_from_its_resolved_graphic() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let frame = |body: u16| {
            (
                FrameKey {
                    body,
                    group: 4,
                    direction: 0,
                    frame: 0,
                },
                AnimFrame {
                    center_x: 12,
                    center_y: -3,
                    image: Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            )
        };
        // Two bodies in one atlas: the mobile's own (400) and the robe's
        // resolved graphic (7005), the way a real frame would hold both.
        let atlas = AnimAtlas::pack([frame(400), frame(7005)]).expect("both frames fit");

        let equip_conv = EquipConv::parse("400\t7017\t7005\t0\t0\n");
        let mobile = Mobile {
            at: Point::new(100, 100, 0),
            body: 400,
            group: 4,
            facing: Direction::SouthEast,
            frame: 0,
            from: None,
            hue: Hue::NONE,
            drawn: Gaze::on(Point::new(100, 100, 0)),
            equipment: vec![worn_on_torso(7017, Hue::NONE)],
        };
        let quads = collect(&[mobile], &camera, &atlas, &Cutaway::OPEN, &equip_conv, None);
        assert_eq!(quads.len(), 2, "the body and its one worn item");
        assert_eq!(
            quads[0].depth, quads[1].depth,
            "a layer shares its body's depth so the stable sort keeps it on top"
        );
    }

    /// An item whose `AnimID` is zero wears nothing on the body, and is not
    /// packed either.
    ///
    /// Zero is not an id here, it is the absence of one: a backpack and a ring
    /// show on a paperdoll and never on a walking body, and `MobileView.Draw`
    /// skips such a layer outright. Drawn instead, the zero is far from inert —
    /// body 0 is the first *monster* in `anim.mul`, so the layer draws a piece
    /// of a creature under its wearer and walks around with them. Both halves
    /// are asserted, because the atlas request is where the frame would come
    /// from at all.
    #[test]
    fn a_layer_with_no_anim_id_is_neither_packed_nor_drawn() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        // A body at 400 and *something* at 0, so a layer that asked for body 0
        // would have a frame to draw and the test would see it.
        let atlas = AnimAtlas::pack([
            (
                FrameKey {
                    body: 400,
                    group: 4,
                    direction: 0,
                    frame: 0,
                },
                AnimFrame {
                    center_x: 12,
                    center_y: -3,
                    image: Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            ),
            (
                FrameKey {
                    body: 0,
                    group: 4,
                    direction: 0,
                    frame: 0,
                },
                AnimFrame {
                    center_x: 4,
                    center_y: -1,
                    image: Image::new(10, 10, vec![Color16(0x7C00); 10 * 10]),
                },
            ),
        ])
        .expect("both frames fit");
        let mobile = Mobile {
            at: Point::new(100, 100, 0),
            body: 400,
            group: 4,
            facing: Direction::SouthEast,
            frame: 0,
            from: None,
            hue: Hue::NONE,
            drawn: Gaze::on(Point::new(100, 100, 0)),
            equipment: vec![worn_on_torso(0, Hue::NONE)],
        };
        assert_eq!(
            needed_animations(std::slice::from_ref(&mobile), &no_equip()),
            vec![(400, 4, 0)],
            "the body alone; body 0 is not an animation anybody asked for",
        );
        assert_eq!(
            collect(&[mobile], &camera, &atlas, &Cutaway::OPEN, &no_equip(), None).len(),
            1,
            "the body alone, with no monster's frame worn over it",
        );
    }

    /// A ghost is bald, and the shard is not wrong to have dressed it.
    ///
    /// A relogged dead player wears a death shroud, its backpack and its hair —
    /// checked against a real save — and the reference refuses to draw the last
    /// of the three on the dead. The graphic alone cannot say which layer is
    /// hair, which is what [`EquipmentLayer::layer`] is carried for, so this
    /// test gives one body two identical pictures on different slots: only the
    /// slot can tell them apart, and only one of them draws.
    #[test]
    fn a_ghost_wears_no_hair() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        // Body 0x0192 is drawn from the living body two below it, which is what
        // the atlas is packed under — `anim::animation_body`.
        let atlas = AnimAtlas::pack([
            (
                FrameKey {
                    body: 0x0190,
                    group: 4,
                    direction: 0,
                    frame: 0,
                },
                AnimFrame {
                    center_x: 20,
                    center_y: -2,
                    image: Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            ),
            (
                FrameKey {
                    body: 7005,
                    group: 4,
                    direction: 0,
                    frame: 0,
                },
                AnimFrame {
                    center_x: 20,
                    center_y: -2,
                    image: Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            ),
        ])
        .expect("both frames fit");
        let dressed = |body: u16| Mobile {
            body,
            equipment: vec![
                worn_on_torso(7005, Hue::NONE),
                EquipmentLayer {
                    graphic: 7005,
                    hue: Hue::NONE,
                    layer: Layer::HAIR,
                },
            ],
            ..body_at(100, Direction::SouthEast)
        };

        let living = dressed(0x0190);
        assert_eq!(
            collect(&[living], &camera, &atlas, &Cutaway::OPEN, &no_equip(), None).len(),
            3,
            "a living body wears both of them: the body, the shirt and the hair",
        );

        let ghost = dressed(0x0192);
        assert_eq!(
            needed_animations(std::slice::from_ref(&ghost), &no_equip()),
            vec![(0x0190, 4, 0), (7005, 4, 0)],
            "the hair is not packed either — the pack and the draw must agree",
        );
        assert_eq!(
            collect(&[ghost], &camera, &atlas, &Cutaway::OPEN, &no_equip(), None).len(),
            2,
            "and a ghost wears only the shirt: the body, the shirt, no hair",
        );
    }

    /// An item with no `EquipConv` entry draws from its own `AnimID` — the
    /// ordinary case, since a plain shirt has no entry at all and still has
    /// to be drawn.
    #[test]
    fn an_unmapped_item_draws_from_its_own_anim_id() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let frame = |body: u16| {
            (
                FrameKey {
                    body,
                    group: 4,
                    direction: 0,
                    frame: 0,
                },
                AnimFrame {
                    center_x: 12,
                    center_y: -3,
                    image: Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            )
        };
        // The body (400) and the shirt's own AnimID (7017) — no conversion
        // entry for either, the way a plain shirt ships none.
        let atlas = AnimAtlas::pack([frame(400), frame(7017)]).expect("both frames fit");
        let mobile = Mobile {
            at: Point::new(100, 100, 0),
            body: 400,
            group: 4,
            facing: Direction::SouthEast,
            frame: 0,
            from: None,
            hue: Hue::NONE,
            drawn: Gaze::on(Point::new(100, 100, 0)),
            equipment: vec![worn_on_torso(7017, Hue::NONE)],
        };
        let quads = collect(&[mobile], &camera, &atlas, &Cutaway::OPEN, &no_equip(), None);
        assert_eq!(
            quads.len(),
            2,
            "the body and the shirt, drawn from its own AnimID"
        );
    }

    /// An item with no `EquipConv` entry, and no atlas frame packed for its
    /// own `AnimID` either, is dropped rather than drawn wrong — the same
    /// rule a missing body animation gets.
    #[test]
    fn an_unpacked_item_draws_nothing() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = atlas(400, 0, 40, 60, (12, -3));
        let mobile = Mobile {
            at: Point::new(100, 100, 0),
            body: 400,
            group: 4,
            facing: Direction::SouthEast,
            frame: 0,
            from: None,
            hue: Hue::NONE,
            drawn: Gaze::on(Point::new(100, 100, 0)),
            equipment: vec![worn_on_torso(7017, Hue::NONE)],
        };
        let quads = collect(&[mobile], &camera, &atlas, &Cutaway::OPEN, &no_equip(), None);
        assert_eq!(
            quads.len(),
            1,
            "only the body, the item's own AnimID was never packed"
        );
    }
}
