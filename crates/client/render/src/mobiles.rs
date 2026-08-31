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

use std::rc::Rc;

use openshard_protocol::direction::Direction;
use openshard_protocol::wire::{
    Graphic,
    Hue,
    Layer,
};
use openshard_protocol::world::Point;
use openshard_tiles::AnimId;
use openshard_uofiles::anim::{
    AnimationFrameIndex,
    AnimationGroup,
    BodyKind,
};
use openshard_uofiles::equipconv::EquipConv;

use crate::atlas::{
    AnimAtlas,
    AnimFrameSource,
    AnimationKey,
    AtlasPixel,
    FrameKey,
};
use crate::camera::{
    Camera,
    RealPixel,
    ViewPixel,
    ViewPoint,
    WorldPoint,
};
use crate::cutaway::Cutaway;
use crate::depth;
use crate::follow::Gaze;
use crate::geometry::{
    Rect,
    Vec2,
};
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
    pub at:        Point,
    /// Its body id.
    pub body:      Graphic,
    /// Which animation group is playing.
    pub group:     AnimationGroup,
    /// Which way it is looking.
    ///
    /// The facing, not the stored direction: turning one into the other is
    /// [`openshard_uofiles::anim::facing`], and doing it here rather than in the
    /// caller is what keeps the mirror and the placement from being decided in
    /// two different places.
    pub facing:    Direction,
    /// Which frame of that animation.
    pub frame:     AnimationFrameIndex,
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
    pub from:      Option<Point>,
    /// Whether this is a corpse item lying on the ground.
    ///
    /// A death frame reaches into the tile immediately nearer the camera. Its
    /// anchor remains the corpse item's tile, but the foreground land must not
    /// cut through the body, so its depth key advances one tile.
    pub corpse:    bool,
    /// Its hue, or [`Hue::NONE`] for none.
    pub hue:       Hue,
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
    pub drawn:     Gaze,
    /// What it is wearing. Not the item's own wire graphic — its tiledata
    /// `AnimID`, already resolved by the caller (this crate has no tiledata
    /// reference), because that is what a worn item draws with *by default*.
    /// [`collect`] only asks [`EquipConv`] whether this mobile's body wants a
    /// *different* picture for it.
    /// Immutable while a render-mobile value is used. Frame snapshots clone
    /// this handle rather than allocating and copying every worn layer again.
    /// The client is single-threaded, so `Rc` expresses the intended ownership
    /// without imposing atomic reference-count traffic on every frame.
    pub equipment: Rc<[EquipmentLayer]>,
}

/// A position in the frame's mobile list.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct MobileIndex(usize);

impl MobileIndex {
    pub const fn new(index: usize) -> Self {
        Self(index)
    }
    /// Its position in the mobile list it was picked from.
    pub const fn position(self) -> usize {
        self.0
    }
}

/// One worn item, ready to place: an `AnimID`, not a picture.
///
/// The order this list is in says nothing about the order they are drawn in:
/// that is [`crate::paperdoll::world_order`]'s answer, and [`drawn_layers`] is
/// where this crate asks for it. The wire's own order was what the layers were
/// once painted in, and it is wrong in the ordinary case — a shard lists a
/// cloak before a tunic, and the cloak is then painted over the tunic on a body
/// facing the camera, where it belongs behind the whole of it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EquipmentLayer {
    /// The body-animation-space graphic this item draws with when
    /// [`EquipConv`] has nothing to say about it — its own tiledata `AnimID`,
    /// not its wire graphic. [`EquipConv::resolve`] is keyed on this same
    /// number, not on the wire graphic either.
    ///
    /// [`Layer::MOUNT`] is the exception, and it is one of space rather than of
    /// index: a mount is not drawn on the rider's frame at all, so what this
    /// carries there is the *creature's* body id — read from the same
    /// `anim.idx`, under the mount's own group, by [`mount_of`]. Where that
    /// number comes from and why it is not the file's is `crowd::mount_picture`,
    /// in `client/app`, which is where the wire's list becomes this one.
    pub graphic: AnimId,
    /// Its hue, or [`Hue::NONE`] for none.
    pub hue:     Hue,
    /// Which slot it is worn on.
    ///
    /// Carried because the *picture* is not enough to draw a body correctly:
    /// the reference skips hair and a beard on the dead ([`worn_graphic`]), and
    /// nothing about a hair graphic says it is hair. It is also the field a
    /// paperdoll's layer ordering needs — see `docs/client.md` — which is why
    /// it is the wire's [`Layer`] and not a smaller local enum: the ordering
    /// table is written against these numbers.
    pub layer:   Layer,
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
fn cell_centre(mobile: &Mobile, camera: &Camera) -> ViewPoint {
    camera.to_view_exact(camera.snap(world_position(mobile)))
}

/// One tile unit, in the fixed point [`billboard_offset`] packs a step's
/// fraction at — comfortably finer than a screen pixel resolves (a real pixel
/// is at most `1 / 44` of a tile) and with headroom past a whole tile for an
/// eased body's overshoot.
const BILLBOARD_OFFSET_SCALE: f32 = 4096.0;

/// How far a mobile's drawn position has walked past the tile [`Mobile::at`]
/// names, packed as two fixed-point `i16`s — the low word `x`, the high word
/// `y` — the way `impostor.wesl`'s `billboard_offset` takes it apart.
///
/// **The second anchor.** [`cell_centre`] places the sprite's screen rect at
/// the body's actual, fractional position, but [`crate::place::Place`] has no
/// bits for a fraction — `Place::of_mobile` carries only [`Mobile::at`], the
/// whole tile — and `impostor.wesl`'s `billboard_at` rebuilds a fragment's lit
/// position from *that* tile plus the screen offset the vertex stage handed
/// it. Mid-step the two disagree by up to a tile: the screen rect is where the
/// body really is, the lit position is where its last or next tile is. A
/// person saw this as vertical seams down a walking figure — the light jumps
/// once per screen column because `blit.wesl`'s dither turns each row's sample
/// pattern by an angle that belongs to the position it was lit at, not the one
/// it was drawn at.
///
/// A mobile never draws a corner (its stance is always
/// [`crate::place::Stance::Upright`]), so [`crate::sprite::SpriteQuad::twin`]
/// carries nothing else for one — this is what rides in it instead.
fn billboard_offset(mobile: &Mobile) -> u32 {
    let offset = walked_offset(mobile);
    let pack = |v: f32| -> u32 {
        let fixed = (v * BILLBOARD_OFFSET_SCALE).round();
        let clamped = fixed.clamp(f32::from(i16::MIN), f32::from(i16::MAX));
        (clamped as i16) as u16 as u32
    };
    pack(offset.x) | (pack(offset.y) << 16)
}

/// How far a mobile's drawn position has walked past the tile [`Mobile::at`]
/// names, in tile units — the same fractional lead [`billboard_offset`] packs
/// for the sprite's own lit position, unpacked, for a caller that has no
/// fixed-point word to put it in. [`crate::light::carried`]'s `offset` is
/// this: a flame anchored to `at` alone would sit still for the whole of a
/// step and then jump, four hundred milliseconds late.
pub fn walked_offset(mobile: &Mobile) -> Vec2 {
    let (tile_x, tile_y) = crate::camera::unproject_ground(mobile.drawn.x, mobile.drawn.y);
    Vec2::new(
        (tile_x - f64::from(mobile.at.x)) as f32,
        (tile_y - f64::from(mobile.at.y)) as f32,
    )
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
/// And, for a mounted rider, the mount's own body under its own group — see
/// [`mount_of`], whose key this is not: the mount plays a different group
/// from the rider it is packed alongside.
pub fn needed_animations(mobiles: &[Mobile], equip_conv: &EquipConv) -> Vec<crate::atlas::AnimationKey> {
    let mut wanted: Vec<crate::atlas::AnimationKey> = Vec::new();
    for mobile in mobiles {
        let (direction, _) = openshard_uofiles::anim::facing(mobile.facing);
        // The body the *file* holds, which for a ghost is the living body it is
        // drawn from — see `anim::animation_body`. Packed under that key, so
        // `place` below finds it under the same one.
        wanted.push(crate::atlas::AnimationKey::new(
            openshard_uofiles::anim::animation_body(mobile.body),
            mobile.group,
            direction,
        ));
        for layer in drawn_layers(mobile) {
            if let Some((graphic, _)) = worn_graphic(mobile, layer, equip_conv) {
                wanted.push(crate::atlas::AnimationKey::new(graphic, mobile.group, direction));
            }
        }
        if let Some((mount_body, _, mount_group)) = mount_of(mobile) {
            wanted.push(crate::atlas::AnimationKey::new(
                mount_body,
                mount_group,
                direction,
            ));
        }
    }
    wanted.sort_unstable();
    wanted.dedup();
    wanted
}

/// What a mobile is wearing, back to front — the one answer to "which layers
/// does this body draw, and in what order", and the only list any pass here
/// walks.
///
/// Three passes read it and they must agree: [`needed_animations`] packs what
/// it names, [`push_quads`] draws it, and [`pick`] tests the cursor against it.
/// A layer packed and not drawn is waste; a layer drawn and not packed is a
/// hole; a layer picked and not drawn is a creature you can click on through
/// thin air.
///
/// The order is [`crate::paperdoll::world_order`]'s — the reference's table,
/// not the wire's arrival order. That table also drops two layers outright: the
/// mount, which is a body of its own on the ground rather than a picture on
/// this one (see [`mount_of`], which draws it), and the backpack, which the
/// reference draws on a doll and never on a walking body (`Filter(order,
/// includeBackpack: false, ...)` in `PaperdollOrder.BuildInWorld`).
///
/// Borrows the matching worn layers in their draw order. The fixed order table
/// and the renderer both avoid allocation and never copy an `EquipmentLayer`
/// out of the mobile.
fn drawn_layers<'a>(mobile: &'a Mobile) -> impl Iterator<Item = &'a EquipmentLayer> + 'a {
    let alt_torso =
        openshard_uofiles::anim::is_female(mobile.body) || openshard_uofiles::anim::is_gargoyle(mobile.body);
    crate::paperdoll::world_ordered(&mobile.equipment, alt_torso, mobile.facing)
        .filter_map(move |layer| mobile.equipment.iter().find(|item| item.layer == layer))
}

/// Which mounted rider pose the horse under them is drawn from
/// — [`mount_of`]'s bridge between the rider's own group numbering and the
/// mount's completely different one, since nothing else here ever needs to
/// name a stance apart from the group number that plays it.
enum MountedStance {
    Standing,
    Walking,
    Running,
}

/// The rider's mount, as a second body to draw beneath them — `None` for a
/// mobile that either is not drawn from one of [`BodyKind::Human`]'s mounted
/// poses or, despite that, carries no `Layer::MOUNT` item (a status the wire
/// disagreed with itself about, no worse handled than any other absent frame).
///
/// A mount is never a picture riding the wearer's own frame the way a hat is
/// — [`drawn_layers`] drops `Layer::MOUNT` from that list outright — because
/// it is a whole second animated body. It shares the rider's [`Mobile::frame`]
/// and [`Mobile::facing`] (the two play in lockstep: they are one server-side
/// step, drawn as two sprites) but never the rider's *group* — a horse under
/// a rider sitting the mounted stand is still an animal at its own
/// [`BodyKind::standing`], not at the human numbering's 25.
///
/// Gated on the rider's group and not on the equipment list alone, so the two
/// facts this crate is handed cannot fall out of step: `crowd.rs` sets the
/// mounted group and equips the saddle together (`Tracked::mounted`), but the
/// group is the one that says the atlas actually holds the seated pose, and a
/// horse must not appear under a body drawn standing on its own two feet —
/// mid-dismount, for the one frame between the item leaving and the group
/// catching up, this returns `None` and the horse is simply not drawn.
///
/// No offset is applied to seat the rider higher than [`place`]'s ordinary
/// anchor would put them: every mount this engine currently spawns
/// ([`openshard_protocol::mounts`]) is an ordinary-height animal, and the
/// mounted frames' own baked anchor already seats a rider correctly on one.
/// The reference client's per-species pixel correction
/// (`Mounts.cs`'s `OffsetY`, non-zero only for the unusually tall or short —
/// a unicorn, a tiger) is a backlog entry for whenever such a mount exists
/// here, not a silent approximation now.
fn mount_of(mobile: &Mobile) -> Option<(Graphic, Hue, AnimationGroup)> {
    let human = BodyKind::Human;
    let stance = if Some(mobile.group) == human.standing_mounted() {
        MountedStance::Standing
    } else if Some(mobile.group) == human.walking_mounted() {
        MountedStance::Walking
    } else if Some(mobile.group) == human.running_mounted() {
        MountedStance::Running
    // The four mounted attacks are seated poses too. The rider plays its
    // weapon animation while the mount remains at rest beneath it; rejecting
    // these groups makes the horse vanish for the whole attack.
    } else if matches!(mobile.group.0, 26..=29) {
        MountedStance::Standing
    } else {
        return None;
    };
    let saddle = mobile.equipment.iter().find(|item| item.layer == Layer::MOUNT)?;
    if saddle.graphic == AnimId(0) {
        return None;
    }
    let mount_body = Graphic(saddle.graphic.0);
    let mount_kind = BodyKind::of(mount_body);
    let group = match stance {
        MountedStance::Standing => mount_kind.standing(),
        MountedStance::Walking => mount_kind.walking(),
        MountedStance::Running => mount_kind.running().unwrap_or_else(|| mount_kind.walking()),
    };
    Some((mount_body, saddle.hue, group))
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
fn worn_graphic(mobile: &Mobile, layer: &EquipmentLayer, equip_conv: &EquipConv) -> Option<(Graphic, Hue)> {
    if layer.graphic == AnimId(0) {
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
    let graphic = Graphic(entry.map_or(layer.graphic.0, |entry| entry.graphic.0));
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
    rect:     Rect,
    region:   crate::atlas::Region,
    order:    depth::Order,
    /// Which atlas picture it is showing. Carried so [`pick`] can ask that
    /// picture's own texels whether the cursor hit it, rather than re-deriving
    /// a source from the mobile and drifting from what was drawn.
    source:   AnimFrameSource,
    /// Whether the picture is sampled right to left. The atlas holds one
    /// picture for both facings, so a texel test has to undo the flip — see
    /// [`AnimAtlas::opaque_at`].
    mirrored: bool,
}

/// The mobile's frame in the atlas, placed on screen.
///
/// `body` is the atlas key's body id and not always [`Mobile::body`], for two
/// reasons. An equipment layer's picture lives under its *resolved* body-anim
/// graphic (see [`EquipConv`]), read at the same group, direction and frame as
/// the mobile wearing it — a worn item has no clock of its own. And a body the
/// files hold no animation for is read under the one they do:
/// `openshard_uofiles::anim::animation_body`, which is what draws a ghost. Every
/// caller passes the key it packed with, so this never re-derives either.
///
/// `group` is likewise not always [`Mobile::group`]: every ordinary caller
/// passes that field straight through, but a mounted rider's own mount plays
/// its *own* group — an animal's ordinary [`BodyKind::standing`], never the
/// human numbering the rider is drawn from — while still sharing the rider's
/// frame and facing. See [`mount_of`].
fn place(
    mobile: &Mobile,
    body: Graphic,
    group: AnimationGroup,
    camera: &Camera,
    atlas: &AnimAtlas,
) -> Option<Placement> {
    let (direction, mirrored) = openshard_uofiles::anim::facing(mobile.facing);
    let key = FrameKey::new(AnimationKey::new(body, group, direction), mobile.frame);
    let (packed, source) = atlas.frame_or_fallback(key, mobile.corpse);

    // Tiles and not the pixels it is between: the order steps once, at the tile
    // boundary, where an interpolated one would have a mobile change sides of a
    // wall in the middle of a step. Which of the two tiles a step is between is
    // `depth::mobile_tile`'s to say.
    let order = depth::Order {
        tile:       match mobile.corpse {
            true => depth::corpse_tile(mobile.at),
            false => depth::mobile_tile(mobile.at, mobile.from),
        },
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
            x:      at.x - anchor_x as f32,
            y:      at.y - (height + i32::from(packed.center_y)) as f32,
            width:  width as f32,
            height: height as f32,
        },
        region,
        order,
        source,
        mirrored,
    })
}

/// The quads for every mobile in the frame.
///
/// Where the client files have no matching animation, the body uses the atlas's
/// built-in neutral silhouette. It makes a missing or newly arrived body
/// visible without the more misleading alternative of borrowing another
/// creature's art; corpses receive their own low, lying marker.
///
/// `cutaway` hides a body on the storey above with that storey. It is the same
/// `max_z` the statics are tested against and deliberately not `no_draw_roofs`:
/// a mobile is never a roof, and the client asks it the one question.
///
/// A mobile's equipment draws over its body for free, without a second depth
/// pass: every layer gets the *same* [`depth::Order`] as the body quad, and
/// the sort below is stable, so pushing the body first and its layers after —
/// in [`drawn_layers`]'s order — is what keeps them on top, and what keeps them
/// in the right order among themselves. A layer draws with its own `AnimID`
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
    highlight: Option<MobileIndex>,
) -> Vec<SpriteQuad> {
    collect_with_interior(mobiles, camera, atlas, cutaway, equip_conv, highlight, None)
}

/// [`collect`] with the resolved building-cell gate from the same frame.
pub fn collect_with_interior(
    mobiles: &[Mobile],
    camera: &Camera,
    atlas: &AnimAtlas,
    cutaway: &Cutaway,
    equip_conv: &EquipConv,
    highlight: Option<MobileIndex>,
    interior: Option<&crate::interiors::InteriorFrame>,
) -> Vec<SpriteQuad> {
    let mut quads: Vec<(depth::Order, SpriteQuad)> = Vec::new();

    for (index, mobile) in mobiles.iter().enumerate() {
        if !interior.is_none_or(|frame| frame.shows_at(mobile.at)) {
            continue;
        }
        // The highlight replaces the creature's own hue rather than combining
        // with it, exactly as an item's does — the shader has one hue per
        // sprite and a ramp *replaces* the art's colour.
        let hue = match highlight == Some(MobileIndex::new(index)) {
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
        mobile.group,
        camera,
        atlas,
    ) else {
        return;
    };
    let order = placement.order;
    let billboard = billboard_offset(mobile);
    let body_is_fallback = matches!(placement.source, AnimFrameSource::Fallback { .. });
    // The mount, if this body is drawn mounted — pushed *before* the rider so
    // that the stable sort below keeps it behind: both share `order`, and a
    // tie is won by whichever [`Vec`] position is later. See [`mount_of`] for
    // why this is a whole second body rather than a layer over the rider's
    // own frame.
    if !body_is_fallback {
        if let Some((mount_body, mount_hue, mount_group)) = mount_of(mobile) {
            if let Some(mount) = place(mobile, mount_body, mount_group, camera, atlas)
                .filter(|mount| matches!(mount.source, AnimFrameSource::Frame(_)))
            {
                out.push((
                    order,
                    SpriteQuad {
                        rect:    mount.rect,
                        region:  mount.region,
                        depth:   order.to_depth(base),
                        hue:     u32::from(hue.unwrap_or(mount_hue).0),
                        place:   crate::place::Place::of_mobile(mobile.at),
                        twin:    billboard,
                        owner:   u32::from(crate::occlusion::OwnerId::NONE.raw()),
                        volumes: crate::impostor::Range::default(),
                    },
                ));
            }
        }
    }
    out.push((
        order,
        SpriteQuad {
            rect:    placement.rect,
            region:  placement.region,
            depth:   order.to_depth(base),
            hue:     u32::from(hue.unwrap_or(mobile.hue).0),
            place:   crate::place::Place::of_mobile(mobile.at),
            twin:    billboard,
            // A billboard is no occluder, so it is exempt from nothing — a
            // creature standing on a walled tile is genuinely in that wall's
            // shadow. `docs/lighting_height.md` phase 3, and the one behaviour
            // change it makes on a real frame.
            owner:   u32::from(crate::occlusion::OwnerId::NONE.raw()),
            volumes: crate::impostor::Range::default(),
        },
    ));

    if body_is_fallback {
        // A generic body deliberately has no generic clothing. Rendering a
        // fallback for each missing worn frame would stack several silhouettes
        // on one tile and obscure the useful marker.
        return;
    }

    for layer in drawn_layers(mobile) {
        let Some((graphic, worn_hue)) = worn_graphic(mobile, layer, equip_conv) else {
            continue;
        };
        let Some(worn) = place(mobile, graphic, mobile.group, camera, atlas) else {
            continue;
        };
        if !matches!(worn.source, AnimFrameSource::Frame(_)) {
            continue;
        }
        out.push((
            order,
            SpriteQuad {
                rect:    worn.rect,
                region:  worn.region,
                depth:   order.to_depth(base),
                hue:     u32::from(hue.unwrap_or(worn_hue).0),
                // The body's tile, not the sprite's: a hat is lit as the
                // head under it is, and it has no tile of its own.
                place:   crate::place::Place::of_mobile(mobile.at),
                twin:    billboard,
                owner:   u32::from(crate::occlusion::OwnerId::NONE.raw()),
                volumes: crate::impostor::Range::default(),
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
    highlight: Option<MobileIndex>,
) -> Vec<SpriteQuad> {
    let mut quads: Vec<(depth::Order, SpriteQuad)> = Vec::new();
    if let Some(mobile) = highlight.and_then(|index| mobiles.get(index.position())) {
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
    cursor: RealPixel,
) -> Option<MobileIndex> {
    pick_iter_with_interior(mobiles.iter(), camera, atlas, cutaway, equip_conv, cursor, None)
}

/// Pick from borrowed mobile views without materialising a second owned list.
///
/// The application keeps `(Who, Mobile)` pairs so it can update animation
/// clocks in place. Picking only reads the mobile, and cloning every equipment
/// list just to adapt that pair list to [`pick`] was a needless frame-sized
/// allocation.
#[must_use]
pub fn pick_iter<'a>(
    mobiles: impl Iterator<Item = &'a Mobile>,
    camera: &Camera,
    atlas: &AnimAtlas,
    cutaway: &Cutaway,
    equip_conv: &EquipConv,
    cursor: RealPixel,
) -> Option<MobileIndex> {
    pick_iter_with_interior(mobiles, camera, atlas, cutaway, equip_conv, cursor, None)
}

/// [`pick_iter`] under the same building policy as mobile collection.
#[must_use]
pub fn pick_iter_with_interior<'a>(
    mobiles: impl Iterator<Item = &'a Mobile>,
    camera: &Camera,
    atlas: &AnimAtlas,
    cutaway: &Cutaway,
    equip_conv: &EquipConv,
    cursor: RealPixel,
    interior: Option<&crate::interiors::InteriorFrame>,
) -> Option<MobileIndex> {
    let in_view = camera.to_view(camera.pick(cursor));
    let mut hit: Option<(depth::Order, MobileIndex)> = None;
    for (index, mobile) in mobiles.enumerate() {
        if !cutaway.shows_mobile(mobile.at.z) || !interior.is_none_or(|frame| frame.shows_at(mobile.at)) {
            continue;
        }
        // The body first, then what it wears, then the mount under it, if
        // any: any one of them is the creature — see [`mount_of`]'s own note
        // that a mounted rider is still picked as one body, never two.
        let body = openshard_uofiles::anim::animation_body(mobile.body);
        let mut touched = None;
        let Some(body) = place(mobile, body, mobile.group, camera, atlas) else {
            continue;
        };
        if opaque_under(&body, atlas, in_view) {
            touched = Some(body.order);
        }
        if touched.is_none() && matches!(body.source, AnimFrameSource::Frame(_)) {
            let worn = drawn_layers(mobile).filter_map(|layer| {
                worn_graphic(mobile, layer, equip_conv).map(|(graphic, _)| (graphic, mobile.group))
            });
            let mount = mount_of(mobile).map(|(mount_body, _, mount_group)| (mount_body, mount_group));
            for (graphic, group) in worn.chain(mount) {
                let Some(placement) = place(mobile, graphic, group, camera, atlas) else {
                    continue;
                };
                // `collect` skips a missing equipment or mount frame rather
                // than stacking a second fallback silhouette on the body.
                if !matches!(placement.source, AnimFrameSource::Frame(_))
                    || !opaque_under(&placement, atlas, in_view)
                {
                    continue;
                }
                touched = Some(placement.order);
                break;
            }
        }
        let Some(order) = touched else {
            continue;
        };
        // `>=`, so a later mobile at the same order takes it: the tie-break is
        // the caller's order, and the one drawn last is the one on top.
        if hit.is_none_or(|(best, _)| order >= best) {
            hit = Some((order, MobileIndex::new(index)));
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
    atlas.source_opaque_at(placement.source, AtlasPixel::new(texel_x, y))
}

/// Where a label belongs above this mobile's head, in view pixels — the
/// horizontal centre of its sprite and its topmost edge, or `None` if
/// [`collect`] would draw nothing for it either.
///
/// Not a fixed offset above [`cell_centre`]: a dragon and a rat stand on the
/// same tile centre and hold their heads at wildly different heights, and only
/// the packed frame's own rectangle knows which.
/// Where a mobile's own picture lands on screen, for a caller that needs the
/// rectangle rather than a point on it — [`head_anchor`]'s twin.
///
/// `cutaway::hides_foliage_over` reads this for the player's own body: a
/// tree's leaves fading nobody's screen but this crate has no fade yet, so
/// `docs/combat.md`'s D9 neighbour is a hard cut instead — see
/// [`crate::cutaway`]'s module doc on why foliage was absent until now.
#[must_use]
pub fn screen_rect(mobile: &Mobile, camera: &Camera, atlas: &AnimAtlas) -> Option<Rect> {
    let placement = place(
        mobile,
        openshard_uofiles::anim::animation_body(mobile.body),
        mobile.group,
        camera,
        atlas,
    )?;
    Some(placement.rect)
}

/// The player's actually drawn body pixels in view coordinates.
///
/// A cutaway wall is not selected by two atlas rectangles merely touching: a
/// wall's diagonal band and a body's silhouette both have large transparent
/// corners.  Keeping this small, CPU-side mask beside the body placement lets
/// [`crate::statics`] reject those false candidates before it spends a private
/// G-buffer draw.  Equipment is deliberately absent: a wall hides the body
/// the player controls, while paperdoll layers are visual decoration that can
/// extend far beyond it.
#[derive(Clone, Debug)]
pub struct OpaqueMask {
    rect:   Rect,
    order:  depth::Order,
    width:  u16,
    pixels: Vec<bool>,
}

impl OpaqueMask {
    /// Stable identity of the exact silhouette used by static cutaway tests.
    ///
    /// This is deliberately a fingerprint rather than just its bounding rect:
    /// two body animation frames can share a rectangle while exposing a
    /// different opaque texel to a wall behind them.
    pub fn fingerprint(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut add = |word: u64| {
            hash ^= word;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        };
        for value in [
            self.rect.x.to_bits(),
            self.rect.y.to_bits(),
            self.rect.width.to_bits(),
            self.rect.height.to_bits(),
        ] {
            add(u64::from(value));
        }
        add(self.order.tile as u64);
        add(self.order.priority_z as u64);
        add(u64::from(self.width));
        for pixel in &self.pixels {
            add(u64::from(*pixel));
        }
        hash
    }

    /// The body frame's bounds, retained for the foliage policy.  Foliage is
    /// intentionally a separate, generous canopy rule; see
    /// [`crate::cutaway::hides_foliage_over`].
    #[must_use]
    pub const fn rect(&self) -> Rect {
        self.rect
    }

    /// The depth order of the body this mask belongs to.
    #[must_use]
    pub const fn order(&self) -> depth::Order {
        self.order
    }

    /// Whether this body's and `other`'s *drawn* texels share a viewport
    /// pixel. `other_opaque` receives a texel in `other`'s unmirrored source
    /// image, so the static atlas remains the one authority on its alpha.
    pub fn overlaps_opaque(&self, other: Rect, mut other_opaque: impl FnMut(u16, u16) -> bool) -> bool {
        let left = self.rect.x.max(other.x);
        let top = self.rect.y.max(other.y);
        let right = (self.rect.x + self.rect.width).min(other.x + other.width);
        let bottom = (self.rect.y + self.rect.height).min(other.y + other.height);
        if left >= right || top >= bottom {
            return false;
        }

        // Raster fragments are at pixel centres. These bounds enumerate just
        // the centres within both quads, including a walking body's fractional
        // placement without rounding its rectangle to a different sprite.
        let first_x = (left - 0.5).ceil() as i32;
        let first_y = (top - 0.5).ceil() as i32;
        let last_x = (right - 0.5).ceil() as i32;
        let last_y = (bottom - 0.5).ceil() as i32;
        for y in first_y..last_y {
            let Some(body_y) = texel_at(self.rect.y, self.rect.height, y) else {
                continue;
            };
            for x in first_x..last_x {
                let Some(body_x) = texel_at(self.rect.x, self.rect.width, x) else {
                    continue;
                };
                let body = usize::from(body_y) * usize::from(self.width) + usize::from(body_x);
                if !self.pixels[body] {
                    continue;
                }
                let (Some(other_x), Some(other_y)) = (
                    texel_at(other.x, other.width, x),
                    texel_at(other.y, other.height, y),
                ) else {
                    continue;
                };
                if other_opaque(other_x, other_y) {
                    return true;
                }
            }
        }
        false
    }

    /// A rectangular all-opaque mask for a caller's synthetic fixture.
    #[cfg(test)]
    pub(crate) fn solid(rect: Rect) -> Self {
        let width = rect.width as u16;
        let height = rect.height as u16;
        Self {
            rect,
            order: depth::Order {
                tile:       i32::MIN,
                priority_z: i32::MIN,
            },
            width,
            pixels: vec![true; usize::from(width) * usize::from(height)],
        }
    }
}

/// Build the [`OpaqueMask`] for the body frame [`screen_rect`] would place.
///
/// `None` means the body has no packed frame, the same answer the mobile draw
/// pass and the old rectangle-only candidate path gave.
#[must_use]
pub fn opaque_mask(mobile: &Mobile, camera: &Camera, atlas: &AnimAtlas) -> Option<OpaqueMask> {
    let placement = place(
        mobile,
        openshard_uofiles::anim::animation_body(mobile.body),
        mobile.group,
        camera,
        atlas,
    )?;
    let width = placement.rect.width as u16;
    let height = placement.rect.height as u16;
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height));
    for y in 0..height {
        for x in 0..width {
            let source_x = match placement.mirrored {
                true => width - 1 - x,
                false => x,
            };
            pixels.push(atlas.source_opaque_at(placement.source, AtlasPixel::new(source_x, y)));
        }
    }
    Some(OpaqueMask {
        rect: placement.rect,
        order: placement.order,
        width,
        pixels,
    })
}

/// The source texel whose square contains viewport pixel `pixel`'s centre.
fn texel_at(origin: f32, extent: f32, pixel: i32) -> Option<u16> {
    let texel = ((pixel as f32 + 0.5) - origin).floor() as i32;
    (texel >= 0 && texel < extent as i32).then_some(texel as u16)
}

pub fn head_anchor(mobile: &Mobile, camera: &Camera, atlas: &AnimAtlas) -> Option<ViewPixel> {
    let placement = place(
        mobile,
        openshard_uofiles::anim::animation_body(mobile.body),
        mobile.group,
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
    use openshard_uofiles::anim::{
        AnimFrame,
        AnimationDirection,
    };
    use openshard_uofiles::color::Color16;
    use openshard_uofiles::image::Image;

    use super::*;

    /// A frame packed at a known size, for placing.
    fn atlas(body: u16, direction: u8, width: u16, height: u16, center: (i16, i16)) -> AnimAtlas {
        AnimAtlas::pack([(
            FrameKey::new(
                AnimationKey::new(Graphic(body), AnimationGroup(4), AnimationDirection(direction)),
                AnimationFrameIndex(0),
            ),
            AnimFrame {
                center_x: center.0,
                center_y: center.1,
                image:    Image::new(
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
    fn worn_on_torso(graphic: AnimId, hue: Hue) -> EquipmentLayer {
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
            body: Graphic(400),
            group: AnimationGroup(4),
            facing,
            frame: AnimationFrameIndex(0),
            from: None,
            corpse: false,
            hue: Hue::NONE,
            drawn: Gaze::on(Point::new(x, 100, 0)),
            equipment: Vec::new().into(),
        }
    }

    #[test]
    fn unknown_mobile_and_its_corpse_use_distinct_clickable_silhouettes() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = AnimAtlas::pack([]).expect("the built-in fallback always fits");
        let live = body_at(100, Direction::SouthEast);
        let corpse = Mobile {
            corpse: true,
            ..live.clone()
        };

        let live_rect = screen_rect(&live, &camera, &atlas).expect("unknown mobile has a silhouette");
        let corpse_rect = screen_rect(&corpse, &camera, &atlas).expect("unknown corpse has a silhouette");
        assert!(
            live_rect.height > live_rect.width,
            "the living fallback is an upright marker"
        );
        assert!(
            corpse_rect.width > corpse_rect.height,
            "the corpse fallback is a lying marker"
        );
        assert_eq!(
            collect(
                &[live.clone(), corpse.clone()],
                &camera,
                &atlas,
                &Cutaway::OPEN,
                &no_equip(),
                None,
            )
            .len(),
            2,
            "both missing client animations still produce a quad"
        );
        assert_eq!(
            pick(
                std::slice::from_ref(&live),
                &camera,
                &atlas,
                &Cutaway::OPEN,
                &no_equip(),
                cursor_over(&camera, live_rect, live_rect.width / 2.0, live_rect.height / 2.0),
            ),
            Some(MobileIndex::new(0)),
            "the fallback uses its own opaque pixels for picking"
        );
    }

    /// The viewport pixel a point in the drawn image sits at — the inverse of
    /// what [`pick`] undoes, so a test can click on a sprite it has placed.
    fn cursor_over(camera: &Camera, at: Rect, dx: f32, dy: f32) -> RealPixel {
        let spot = camera.to_viewport(ViewPixel {
            x: (at.x + dx) as i32,
            y: (at.y + dy) as i32,
        });
        RealPixel::new(spot.x as i32, spot.y as i32)
    }

    /// Where a mobile's body lands, asked the way the passes ask it.
    fn placed(mobile: &Mobile, camera: &Camera, atlas: &AnimAtlas) -> Rect {
        place(
            mobile,
            openshard_uofiles::anim::animation_body(mobile.body),
            mobile.group,
            camera,
            atlas,
        )
        .expect("the frame is packed")
        .rect
    }

    /// The cutaway mask is made from the frame's actual alpha, not from its
    /// enclosing rectangle. This is the body half of C3's static/body test.
    #[test]
    fn a_body_mask_keeps_the_frames_transparent_pixels_empty() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let image = Image::new(
            4,
            6,
            (0..24)
                .map(|index| {
                    match index % 4 < 2 {
                        true => Color16::TRANSPARENT,
                        false => Color16(0x7C00),
                    }
                })
                .collect(),
        );
        let atlas = AnimAtlas::pack([(
            FrameKey::new(
                AnimationKey::new(Graphic(400), AnimationGroup(4), AnimationDirection(0)),
                AnimationFrameIndex(0),
            ),
            AnimFrame {
                center_x: 2,
                center_y: 0,
                image,
            },
        )])
        .expect("one body frame fits");
        let mask = opaque_mask(&body_at(100, Direction::SouthEast), &camera, &atlas).expect("packed body");
        let rect = mask.rect();
        let left = Rect { width: 1.0, ..rect };
        let right = Rect {
            x: rect.x + 3.0,
            width: 1.0,
            ..rect
        };
        assert!(
            !mask.overlaps_opaque(left, |_, _| true),
            "the empty half of the body frame became a cutaway mask"
        );
        assert!(
            mask.overlaps_opaque(right, |_, _| true),
            "the drawn half of the body frame was missing from the cutaway mask"
        );
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
            FrameKey::new(
                AnimationKey::new(Graphic(body), AnimationGroup(4), AnimationDirection(direction)),
                AnimationFrameIndex(0),
            ),
            AnimFrame {
                center_x: 12,
                center_y: -3,
                image:    Image::new(width, height, pixels),
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
        assert_eq!(
            pick_at(30.0, 30.0),
            Some(MobileIndex::new(0)),
            "the drawn half was not hit"
        );
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
        assert_eq!(
            pick_at(5.0),
            Some(MobileIndex::new(0)),
            "the flipped picture's drawn half"
        );
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
                FrameKey::new(
                    AnimationKey::new(Graphic(7005), AnimationGroup(4), AnimationDirection(0)),
                    AnimationFrameIndex(0),
                ),
                AnimFrame {
                    center_x: 12,
                    center_y: -3,
                    image:    Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            )])
            .expect("both frames fit");
        let mobile = Mobile {
            equipment: vec![worn_on_torso(AnimId(7005), Hue::NONE)].into(),
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
            Some(MobileIndex::new(0)),
            "the robe's own pixels did not count as the creature",
        );
    }

    /// A mounted rider's saddle equips like any other worn item, but it is not
    /// drawn as one: [`mount_of`] turns it into a whole second body, packed and
    /// drawn under its own group rather than the rider's.
    #[test]
    fn a_mounted_rider_draws_the_horse_beneath_them() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = AnimAtlas::pack([
            (
                // The rider: a human drawn from the mounted stand, 25.
                FrameKey::new(
                    AnimationKey::new(Graphic(400), AnimationGroup(25), AnimationDirection(0)),
                    AnimationFrameIndex(0),
                ),
                AnimFrame {
                    center_x: 12,
                    center_y: -3,
                    image:    Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            ),
            (
                // The mount: an animal body at its own ordinary stand, 2 —
                // never the rider's 25, which the numbering does not share.
                FrameKey::new(
                    AnimationKey::new(Graphic(200), AnimationGroup(2), AnimationDirection(0)),
                    AnimationFrameIndex(0),
                ),
                AnimFrame {
                    center_x: 22,
                    center_y: 0,
                    image:    Image::new(44, 44, vec![Color16(0x7C00); 44 * 44]),
                },
            ),
        ])
        .expect("both frames fit");
        let mobile = Mobile {
            group: AnimationGroup(25),
            equipment: vec![EquipmentLayer {
                graphic: AnimId(200),
                hue:     Hue::NONE,
                layer:   Layer::MOUNT,
            }]
            .into(),
            ..body_at(100, Direction::SouthEast)
        };
        assert_eq!(
            needed_animations(std::slice::from_ref(&mobile), &no_equip()),
            vec![
                AnimationKey::new(Graphic(200), AnimationGroup(2), AnimationDirection(0)),
                AnimationKey::new(Graphic(400), AnimationGroup(25), AnimationDirection(0)),
            ],
            "the mount's own group is packed alongside the rider's",
        );
        let quads = collect(
            std::slice::from_ref(&mobile),
            &camera,
            &atlas,
            &Cutaway::OPEN,
            &no_equip(),
            None,
        );
        assert_eq!(quads.len(), 2, "the horse and its rider, nothing else drawn");
    }

    /// Mounted weapon groups are seated poses, not a dismount. They therefore
    /// keep the mount in the packed and drawn scene for the whole attack.
    #[test]
    fn a_mounted_bow_draws_the_horse_beneath_the_archer() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = AnimAtlas::pack([
            (
                FrameKey::new(
                    AnimationKey::new(Graphic(400), AnimationGroup(27), AnimationDirection(0)),
                    AnimationFrameIndex(0),
                ),
                AnimFrame {
                    center_x: 12,
                    center_y: -3,
                    image:    Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            ),
            (
                FrameKey::new(
                    AnimationKey::new(Graphic(200), AnimationGroup(2), AnimationDirection(0)),
                    AnimationFrameIndex(0),
                ),
                AnimFrame {
                    center_x: 22,
                    center_y: 0,
                    image:    Image::new(44, 44, vec![Color16(0x7C00); 44 * 44]),
                },
            ),
        ])
        .expect("both frames fit");
        let mobile = Mobile {
            group: AnimationGroup(27), // PeopleAnimationGroup.OnmountAttackBow.
            equipment: vec![EquipmentLayer {
                graphic: AnimId(200),
                hue:     Hue::NONE,
                layer:   Layer::MOUNT,
            }]
            .into(),
            ..body_at(100, Direction::SouthEast)
        };

        assert_eq!(
            needed_animations(std::slice::from_ref(&mobile), &no_equip()),
            vec![
                AnimationKey::new(Graphic(200), AnimationGroup(2), AnimationDirection(0)),
                AnimationKey::new(Graphic(400), AnimationGroup(27), AnimationDirection(0)),
            ],
            "the standing horse is packed with the mounted bow pose",
        );
        assert_eq!(
            collect(
                std::slice::from_ref(&mobile),
                &camera,
                &atlas,
                &Cutaway::OPEN,
                &no_equip(),
                None,
            )
            .len(),
            2,
            "the horse remains behind the archer throughout the bow pose",
        );
    }

    /// A rider whose group has not caught up with a dropped saddle — the one
    /// frame between the equipment leaving and [`Crowd::change_to`] landing on
    /// the on-foot stand — draws no horse rather than a stale one.
    #[test]
    fn a_dropped_saddle_with_the_group_not_yet_caught_up_draws_no_horse() {
        let mobile = Mobile {
            group: AnimationGroup(4),
            equipment: vec![EquipmentLayer {
                graphic: AnimId(200),
                hue:     Hue::NONE,
                layer:   Layer::MOUNT,
            }]
            .into(),
            ..body_at(100, Direction::SouthEast)
        };
        assert_eq!(
            needed_animations(std::slice::from_ref(&mobile), &no_equip()),
            vec![AnimationKey::new(
                Graphic(400),
                AnimationGroup(4),
                AnimationDirection(0)
            )],
            "an on-foot stand packs no mount, whatever is still equipped",
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
            Some(MobileIndex::new(1)),
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
                FrameKey::new(
                    AnimationKey::new(Graphic(7005), AnimationGroup(4), AnimationDirection(0)),
                    AnimationFrameIndex(0),
                ),
                AnimFrame {
                    center_x: 12,
                    center_y: -3,
                    image:    Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            )])
            .expect("both frames fit");
        // Their own hues, so the assertion is "replaced" and not "set".
        let dressed = |x: u16| {
            Mobile {
                hue: Hue(0x03B2),
                equipment: vec![worn_on_torso(AnimId(7005), Hue(0x0455))].into(),
                ..body_at(x, Direction::SouthEast)
            }
        };
        let quads = collect(
            &[dressed(100), dressed(101)],
            &camera,
            &atlas,
            &Cutaway::OPEN,
            &no_equip(),
            Some(MobileIndex::new(1)),
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
        let ringed = outlined(
            &mobiles,
            &camera,
            &atlas,
            &Cutaway::OPEN,
            &no_equip(),
            Some(MobileIndex::new(0)),
        );
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
                at:        Point::new(100, 100, 0),
                body:      Graphic(400),
                group:     AnimationGroup(4),
                facing:    Direction::SouthEast,
                frame:     AnimationFrameIndex(0),
                from:      None,
                corpse:    false,
                hue:       Hue::NONE,
                drawn:     Gaze::on(Point::new(100, 100, 0)),
                equipment: Vec::new().into(),
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
                    body: Graphic(400),
                    group: AnimationGroup(4),
                    facing,
                    frame: AnimationFrameIndex(0),
                    from: None,
                    corpse: false,
                    hue: Hue::NONE,
                    drawn: Gaze::on(Point::new(100, 100, 0)),
                    equipment: Vec::new().into(),
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

    /// A mobile the atlas has no frame for gets the neutral silhouette rather
    /// than whatever else was packed. Which matters more here than for statics:
    /// the atlas is keyed by four numbers, so a near miss is another creature.
    #[test]
    fn a_mobile_with_no_packed_frame_gets_a_silhouette() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = atlas(400, 0, 40, 60, (12, -3));
        let missing = Mobile {
            at:        Point::new(100, 100, 0),
            body:      Graphic(400),
            group:     AnimationGroup(4),
            facing:    Direction::SouthEast,
            // One past the only frame packed.
            frame:     AnimationFrameIndex(1),
            from:      None,
            corpse:    false,
            hue:       Hue::NONE,
            drawn:     Gaze::on(Point::new(100, 100, 0)),
            equipment: Vec::new().into(),
        };
        assert_eq!(
            collect(
                std::slice::from_ref(&missing),
                &camera,
                &atlas,
                &Cutaway::OPEN,
                &no_equip(),
                None
            )
            .len(),
            1,
            "a missing frame must not make a server-known mobile disappear"
        );
        // And the same mobile at frame 0 uses its actual packed art, so this
        // is specifically the missing-frame path rather than a body rule.
        assert_eq!(
            collect(
                &[Mobile {
                    frame: AnimationFrameIndex(0),
                    ..missing
                }],
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
    /// body `0x0192` at all, so an atlas packed under the wire's body cannot
    /// show the ghost's real animation and falls back to the neutral marker.
    ///
    /// The same disagreement, one field over, is a body that is drawn and never
    /// animates: a frame count asked under a key nothing was packed under is
    /// zero, and a clock with no frames to choose between stops on the first.
    /// That is `client/app`'s call and it uses this same function.
    #[test]
    fn a_ghost_is_packed_and_drawn_under_the_living_body() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let ghost = Mobile {
            at:        Point::new(100, 100, 0),
            // 0x0192, the male ghost: a body the client's files have no
            // animation for.
            body:      Graphic(402),
            group:     AnimationGroup(4),
            facing:    Direction::SouthEast,
            frame:     AnimationFrameIndex(0),
            from:      None,
            corpse:    false,
            hue:       Hue::NONE,
            drawn:     Gaze::on(Point::new(100, 100, 0)),
            equipment: Vec::new().into(),
        };
        assert_eq!(
            needed_animations(std::slice::from_ref(&ghost), &no_equip()),
            vec![AnimationKey::new(
                Graphic(400),
                AnimationGroup(4),
                AnimationDirection(0)
            )],
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
        assert_eq!(
            collect(
                std::slice::from_ref(&ghost),
                &camera,
                &atlas(402, 0, 40, 60, (12, -3)),
                &Cutaway::OPEN,
                &no_equip(),
                None,
            )
            .len(),
            1,
            "a wrongly keyed atlas is visible as a fallback instead of a missing ghost"
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
            at:        Point::new(100, 100, 0),
            body:      Graphic(400),
            group:     AnimationGroup(4),
            facing:    Direction::SouthEast,
            frame:     AnimationFrameIndex(0),
            from:      None,
            corpse:    false,
            hue:       Hue::NONE,
            drawn:     Gaze::on(Point::new(100, 100, 0)),
            equipment: Vec::new().into(),
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

    /// A mobile the atlas has no frame for gets an anchor above the fallback
    /// too, so speech and combat numbers remain attached to it.
    #[test]
    fn head_anchor_uses_the_fallback_for_an_unpacked_frame() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let atlas = atlas(400, 0, 40, 60, (12, -3));
        let missing = Mobile {
            at:        Point::new(100, 100, 0),
            body:      Graphic(400),
            group:     AnimationGroup(4),
            facing:    Direction::SouthEast,
            frame:     AnimationFrameIndex(1),
            from:      None,
            corpse:    false,
            hue:       Hue::NONE,
            drawn:     Gaze::on(Point::new(100, 100, 0)),
            equipment: Vec::new().into(),
        };
        assert!(head_anchor(&missing, &camera, &atlas).is_some());
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
            at:        Point::new(101, 100, 0),
            body:      Graphic(400),
            group:     AnimationGroup(4),
            facing:    Direction::SouthEast,
            frame:     AnimationFrameIndex(0),
            from:      None,
            corpse:    false,
            hue:       Hue::NONE,
            drawn:     Gaze::on(Point::new(101, 100, 0)),
            equipment: Vec::new().into(),
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
        let atlas = atlas(400, north.index(), 40, 60, (12, -3));
        let base = depth::base_for(100, 100);
        // The ground it is walking off, sorted the way `ground::collect` sorts
        // it: the tile's own depth, and the client's land priority.
        let ground_left_behind = depth::Order {
            tile:       200,
            priority_z: depth::land_priority_z([0; 4]),
        }
        .to_depth(base);

        let walking_north = Mobile {
            at:        Point::new(100, 99, 0),
            body:      Graphic(400),
            group:     AnimationGroup(4),
            facing:    Direction::North,
            frame:     AnimationFrameIndex(0),
            from:      Some(Point::new(100, 100, 0)),
            corpse:    false,
            hue:       Hue::NONE,
            drawn:     Gaze::on(Point::new(100, 99, 0)).back_towards(Gaze::on(Point::new(100, 100, 0)), 0.5),
            equipment: Vec::new().into(),
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
        let mobile = |facing: Direction| {
            Mobile {
                at: Point::new(0, 0, 0),
                body: Graphic(400),
                group: AnimationGroup(4),
                facing,
                frame: AnimationFrameIndex(0),
                from: None,
                corpse: false,
                hue: Hue::NONE,
                drawn: Gaze::on(Point::new(0, 0, 0)),
                equipment: Vec::new().into(),
            }
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
        assert_eq!(
            wanted,
            vec![
                AnimationKey::new(Graphic(400), AnimationGroup(4), AnimationDirection(0)),
                AnimationKey::new(Graphic(400), AnimationGroup(4), AnimationDirection(1))
            ]
        );
    }

    /// An equipment layer draws over the body, resolved through
    /// [`EquipConv`] to its own body-anim graphic and hued from the entry
    /// when the wire hue is [`Hue::NONE`].
    #[test]
    fn a_worn_item_draws_over_the_body_from_its_resolved_graphic() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let frame = |body: u16| {
            (
                FrameKey::new(
                    AnimationKey::new(Graphic(body), AnimationGroup(4), AnimationDirection(0)),
                    AnimationFrameIndex(0),
                ),
                AnimFrame {
                    center_x: 12,
                    center_y: -3,
                    image:    Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            )
        };
        // Two bodies in one atlas: the mobile's own (400) and the robe's
        // resolved graphic (7005), the way a real frame would hold both.
        let atlas = AnimAtlas::pack([frame(400), frame(7005)]).expect("both frames fit");

        let equip_conv = EquipConv::parse("400\t7017\t7005\t0\t0\n");
        let mobile = Mobile {
            at:        Point::new(100, 100, 0),
            body:      Graphic(400),
            group:     AnimationGroup(4),
            facing:    Direction::SouthEast,
            frame:     AnimationFrameIndex(0),
            from:      None,
            corpse:    false,
            hue:       Hue::NONE,
            drawn:     Gaze::on(Point::new(100, 100, 0)),
            equipment: vec![worn_on_torso(AnimId(7017), Hue::NONE)].into(),
        };
        let quads = collect(&[mobile], &camera, &atlas, &Cutaway::OPEN, &equip_conv, None);
        assert_eq!(quads.len(), 2, "the body and its one worn item");
        assert_eq!(
            quads[0].depth, quads[1].depth,
            "a layer shares its body's depth so the stable sort keeps it on top"
        );
    }

    /// The layers are painted in the ordering table's order and not the wire's.
    ///
    /// The defect: a shard lists a cloak after a tunic — the wire's order is
    /// whatever it equipped them in — and a body facing the camera then wears
    /// the cloak painted over the front of the tunic. It belongs behind the
    /// whole body. Told the layers in the wrong order on purpose, and the hues
    /// are how the quads are told apart.
    #[test]
    fn the_worn_layers_are_painted_in_the_tables_order_and_not_the_wires() {
        let camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        let frame = |body: u16| {
            (
                FrameKey::new(
                    AnimationKey::new(Graphic(body), AnimationGroup(4), AnimationDirection(0)),
                    AnimationFrameIndex(0),
                ),
                AnimFrame {
                    center_x: 12,
                    center_y: -3,
                    image:    Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            )
        };
        let atlas = AnimAtlas::pack([frame(400), frame(7017), frame(7018)]).expect("three frames fit");
        let cloak = Hue(11);
        let tunic = Hue(22);
        let mobile = Mobile {
            at:        Point::new(100, 100, 0),
            body:      Graphic(400),
            group:     AnimationGroup(4),
            facing:    Direction::SouthEast,
            frame:     AnimationFrameIndex(0),
            from:      None,
            corpse:    false,
            hue:       Hue::NONE,
            drawn:     Gaze::on(Point::new(100, 100, 0)),
            equipment: vec![
                EquipmentLayer {
                    graphic: AnimId(7017),
                    hue:     tunic,
                    layer:   Layer::TUNIC,
                },
                EquipmentLayer {
                    graphic: AnimId(7018),
                    hue:     cloak,
                    layer:   Layer::CLOAK,
                },
            ]
            .into(),
        };
        let quads = collect(&[mobile], &camera, &atlas, &Cutaway::OPEN, &no_equip(), None);
        assert_eq!(quads.len(), 3, "the body and its two garments");
        assert_eq!(
            quads[1].hue,
            u32::from(cloak.0),
            "the cloak is painted first of the two, facing the camera"
        );
        assert_eq!(quads[2].hue, u32::from(tunic.0));
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
                FrameKey::new(
                    AnimationKey::new(Graphic(400), AnimationGroup(4), AnimationDirection(0)),
                    AnimationFrameIndex(0),
                ),
                AnimFrame {
                    center_x: 12,
                    center_y: -3,
                    image:    Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            ),
            (
                FrameKey::new(
                    AnimationKey::new(Graphic(0), AnimationGroup(4), AnimationDirection(0)),
                    AnimationFrameIndex(0),
                ),
                AnimFrame {
                    center_x: 4,
                    center_y: -1,
                    image:    Image::new(10, 10, vec![Color16(0x7C00); 10 * 10]),
                },
            ),
        ])
        .expect("both frames fit");
        let mobile = Mobile {
            at:        Point::new(100, 100, 0),
            body:      Graphic(400),
            group:     AnimationGroup(4),
            facing:    Direction::SouthEast,
            frame:     AnimationFrameIndex(0),
            from:      None,
            corpse:    false,
            hue:       Hue::NONE,
            drawn:     Gaze::on(Point::new(100, 100, 0)),
            equipment: vec![worn_on_torso(AnimId(0), Hue::NONE)].into(),
        };
        assert_eq!(
            needed_animations(std::slice::from_ref(&mobile), &no_equip()),
            vec![AnimationKey::new(
                Graphic(400),
                AnimationGroup(4),
                AnimationDirection(0)
            )],
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
                FrameKey::new(
                    AnimationKey::new(Graphic(0x0190), AnimationGroup(4), AnimationDirection(0)),
                    AnimationFrameIndex(0),
                ),
                AnimFrame {
                    center_x: 20,
                    center_y: -2,
                    image:    Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            ),
            (
                FrameKey::new(
                    AnimationKey::new(Graphic(7005), AnimationGroup(4), AnimationDirection(0)),
                    AnimationFrameIndex(0),
                ),
                AnimFrame {
                    center_x: 20,
                    center_y: -2,
                    image:    Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            ),
        ])
        .expect("both frames fit");
        let dressed = |body: Graphic| {
            Mobile {
                body,
                equipment: vec![
                    worn_on_torso(AnimId(7005), Hue::NONE),
                    EquipmentLayer {
                        graphic: AnimId(7005),
                        hue:     Hue::NONE,
                        layer:   Layer::HAIR,
                    },
                ]
                .into(),
                ..body_at(100, Direction::SouthEast)
            }
        };

        let living = dressed(Graphic(0x0190));
        assert_eq!(
            collect(&[living], &camera, &atlas, &Cutaway::OPEN, &no_equip(), None).len(),
            3,
            "a living body wears both of them: the body, the shirt and the hair",
        );

        let ghost = dressed(Graphic(0x0192));
        assert_eq!(
            needed_animations(std::slice::from_ref(&ghost), &no_equip()),
            vec![
                AnimationKey::new(Graphic(0x0190), AnimationGroup(4), AnimationDirection(0)),
                AnimationKey::new(Graphic(7005), AnimationGroup(4), AnimationDirection(0)),
            ],
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
                FrameKey::new(
                    AnimationKey::new(Graphic(body), AnimationGroup(4), AnimationDirection(0)),
                    AnimationFrameIndex(0),
                ),
                AnimFrame {
                    center_x: 12,
                    center_y: -3,
                    image:    Image::new(40, 60, vec![Color16(0x7C00); 40 * 60]),
                },
            )
        };
        // The body (400) and the shirt's own AnimID (7017) — no conversion
        // entry for either, the way a plain shirt ships none.
        let atlas = AnimAtlas::pack([frame(400), frame(7017)]).expect("both frames fit");
        let mobile = Mobile {
            at:        Point::new(100, 100, 0),
            body:      Graphic(400),
            group:     AnimationGroup(4),
            facing:    Direction::SouthEast,
            frame:     AnimationFrameIndex(0),
            from:      None,
            corpse:    false,
            hue:       Hue::NONE,
            drawn:     Gaze::on(Point::new(100, 100, 0)),
            equipment: vec![worn_on_torso(AnimId(7017), Hue::NONE)].into(),
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
            at:        Point::new(100, 100, 0),
            body:      Graphic(400),
            group:     AnimationGroup(4),
            facing:    Direction::SouthEast,
            frame:     AnimationFrameIndex(0),
            from:      None,
            corpse:    false,
            hue:       Hue::NONE,
            drawn:     Gaze::on(Point::new(100, 100, 0)),
            equipment: vec![worn_on_torso(AnimId(7017), Hue::NONE)].into(),
        };
        let quads = collect(&[mobile], &camera, &atlas, &Cutaway::OPEN, &no_equip(), None);
        assert_eq!(
            quads.len(),
            1,
            "only the body, the item's own AnimID was never packed"
        );
    }

    /// A body standing still names its own tile with no offset — `twin` packs
    /// to zero, which is also what a static's unused row reads as.
    #[test]
    fn a_standing_body_has_no_billboard_offset() {
        let mobile = body_at(100, Direction::South);
        assert_eq!(billboard_offset(&mobile), 0);
    }

    /// Undoes [`billboard_offset`]'s packing the way `impostor.wesl`'s own
    /// `billboard_offset` does, so this test can pin the two spellings against
    /// each other without a GPU.
    fn unpack_billboard_offset(word: u32) -> (f32, f32) {
        let dx = (word & 0xFFFF) as u16 as i16;
        let dy = (word >> 16) as u16 as i16;
        (
            f32::from(dx) / BILLBOARD_OFFSET_SCALE,
            f32::from(dy) / BILLBOARD_OFFSET_SCALE,
        )
    }

    /// Mid-step, a body's drawn position sits behind the tile it is arriving
    /// at — [`Mobile::at`]'s own doc — and `billboard_offset` has to recover
    /// exactly how far, or `impostor.wesl`'s billboard is lit at the wrong
    /// point of the room. `docs/lighting_rebuild.md` phase 7's own defect.
    #[test]
    fn a_body_half_a_step_in_walks_half_a_tile_short_of_its_offset() {
        let from = Gaze::on(Point::new(100, 100, 0));
        let target = Gaze::on(Point::new(101, 100, 0));
        let mut mobile = body_at(101, Direction::East);
        mobile.drawn = target.back_towards(from, 0.5);

        let (dx, dy) = unpack_billboard_offset(billboard_offset(&mobile));
        assert!((dx - -0.5).abs() < 1e-3, "dx: {dx}");
        assert!(dy.abs() < 1e-3, "dy: {dy}");
    }
}
