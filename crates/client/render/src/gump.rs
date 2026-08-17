//! The interface's own pass: `gumpart.mul`'s pictures, drawn over the world.
//!
//! A gump is UO's windowing primitive — a paperdoll, a container, a shopkeeper's
//! list, `.admin`'s menu — and every one of them is built out of pictures from
//! `gumpartLegacyMUL.uop` at pixel coordinates. `openshard_uofiles::gumpart`
//! reads those pictures; this module packs them, places them and draws them.
//!
//! # Why a pass of its own
//!
//! [`SpriteRenderer`](crate::renderer::SpriteRenderer) already draws rectangles
//! of an atlas, and reusing it here was the first thing tried. It does not fit,
//! and the three reasons are the three things an interface is not:
//!
//! - It writes the **G-buffer**, which says what drew a pixel and where that
//!   fragment is so the lighting pass can dim it. A gump stands on no tile.
//!   Handing it [`Place::NOWHERE`](crate::place::Place::NOWHERE) would work and
//!   would still cost three more full-size attachments on the surface, for an
//!   answer nothing reads.
//! - It **tests and writes depth**, against a buffer the world passes filled at
//!   the world's own resolution. The surface is a different size after a
//!   minifying blit, and an attachment has to match its target.
//! - It applies the **camera projection**. A gump does not move when the player
//!   walks, and does not grow when the world is zoomed.
//!
//! What is shared instead is everything that would be a *second answer* if it
//! were copied: [`SpriteQuad`] and its instance layout, the hue lookup (the
//! fragment shader's is `statics.wgsl`'s, line for line), and the shelf packer
//! behind [`GumpAtlas`].
//!
//! # One gump pixel is `scale` real pixels
//!
//! The reference client draws its art one for one with the display and has no
//! display scaling at all, which on a 4K screen makes its windows postage
//! stamps. So a scale exists here, it is a single number applied in the vertex
//! shader, and it is applied to *coordinates and art together* — which is the
//! whole reason the interface had to leave egui to be drawn: egui measures its
//! widgets in points and a bitmap cannot be reinterpreted into them. See
//! `client/app/src/gump.rs` for the decision this replaces.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use openshard_protocol::gump::layout::Element;
use openshard_protocol::gump::{GUMP_WHITE, GumpButton, RawButtonId, RawSwitchId};
use openshard_protocol::speech::Font;

/// A position in a gump's `pictures` list, never a button id or a list index
/// from another window kind.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct PictureIndex(usize);

impl PictureIndex {
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    /// Its position in the gump picture list it was picked from.
    pub const fn position(self) -> usize {
        self.0
    }
}
use openshard_protocol::wire::{Graphic, Hue};
use openshard_uofiles::art::{Art, ArtError};
use openshard_uofiles::gumpart::{GumpError, Gumps};
use openshard_uofiles::image::Image;

use crate::atlas::{AtlasError, Sprite, StaticAtlas};
use crate::geometry::Rect;
use crate::hue::HueRamp;
use crate::renderer::{QUAD, new_static_instance_buffer, upload, write_rows};
use crate::sprite::SpriteQuad;

/// A point in gump space: the reference client's own interface pixels, before
/// [`Frame::scale`] turns them into the display's.
///
/// Its own type and not a [`ViewPixel`](crate::camera::ViewPixel), for the
/// reason that module gives about every other pixel space here: the two are the
/// same two integers and mean different things, and a value that crossed
/// between them without something doing the arithmetic is exactly the silent
/// mistake the newtypes exist to refuse. A gump coordinate comes off the wire
/// in this space and stays in it until the vertex shader.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct GumpPixel {
    /// Pixels from the left edge of the window this gump belongs to.
    pub x: i32,
    /// Pixels from its top edge.
    pub y: i32,
}

impl GumpPixel {
    /// A point at `(x, y)`.
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// This point moved by another's coordinates.
    ///
    /// What turns a layout's coordinate — which is relative to the gump — into
    /// the window's, and the only arithmetic anything does to one of these.
    pub const fn offset(self, by: Self) -> Self {
        Self {
            x: self.x + by.x,
            y: self.y + by.y,
        }
    }
}

/// A texel within one gump picture, before that picture is placed in a window.
///
/// Unlike [`GumpPixel`], this is not a coordinate in the window: it is the
/// local pixel that [`GumpAtlas::opaque_at`] reads. Keeping the two separate
/// makes the subtraction that turns a cursor position into an art position
/// explicit, rather than allowing either pair of axes to be handed to the
/// other API by accident.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct GumpArtPixel {
    /// Columns from the picture's left edge.
    pub x: u16,
    /// Rows from the picture's top edge.
    pub y: u16,
}

impl GumpArtPixel {
    /// A picture-local texel at `(x, y)`.
    pub const fn new(x: u16, y: u16) -> Self {
        Self { x, y }
    }
}

/// A box outside which nothing is drawn and nothing is picked.
///
/// A scissor rectangle, applied on the processor rather than by the pass: a
/// quad that straddles the edge is *cut* — its rectangle shortened and its
/// region moved by the same fraction — which is exact for an axis-aligned
/// sprite and needs no second draw call, no depth and no shader of its own.
///
/// What it is for is a list that scrolls. A window with more rows than it is
/// tall draws the rows at their true positions and hands every one of them the
/// same box; the rows that fall outside it vanish, and the two that straddle the
/// edge are half-drawn, which is what a scrollbar's user expects and what a
/// list stepping a whole row at a time cannot do.
///
/// **Not** [`crate::text::GumpLabel::clip`], which is a different idea with a
/// different rule: that one is `{ croppedtext }`, where a line too long for its
/// plate loses whole *characters* off the end (`FontsLoader.GetTextByWidthASCII`
/// even appends an ellipsis). This one cuts pixels and knows nothing about what
/// it is cutting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Scissor {
    /// The box's top-left corner, in the same gump pixels the pictures are in.
    pub at: GumpPixel,
    /// How wide it is.
    pub width: i32,
    /// How tall it is.
    pub height: i32,
}

impl Scissor {
    /// The same box, moved by `by`.
    ///
    /// What turns a window-local scissor into the screen-space one a pass
    /// draws against: a window's own layout carries its boxes at the origin
    /// (`docs/window_components.md`'s window-local coordinates), and this is
    /// the one step that adds the window's placement back, right beside
    /// [`Picture::offset`] which does the same for the picture the box crops.
    #[must_use]
    pub const fn offset(self, by: GumpPixel) -> Self {
        Self {
            at: self.at.offset(by),
            ..self
        }
    }

    /// Whether a point is inside — the hit test's half of the same box.
    ///
    /// Right and bottom edges are outside, which is the same half-open box
    /// [`crop`](Self::crop) cuts to: a point at `at.x + width` is the first
    /// column not drawn.
    #[must_use]
    pub const fn contains(self, point: GumpPixel) -> bool {
        point.x >= self.at.x
            && point.y >= self.at.y
            && point.x < self.at.x + self.width
            && point.y < self.at.y + self.height
    }

    /// A rectangle and its region cut to this box, or `None` if nothing of it is
    /// inside.
    ///
    /// The region moves by the same *fraction* the rectangle lost, which is what
    /// makes this a cut and not a squeeze: an art pixel stays one art pixel wide
    /// wherever the edge falls.
    #[must_use]
    fn crop(self, rect: Rect, region: crate::atlas::Region) -> Option<(Rect, crate::atlas::Region)> {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return None;
        }
        let left = rect.x.max(self.at.x as f32);
        let top = rect.y.max(self.at.y as f32);
        let right = (rect.x + rect.width).min((self.at.x + self.width) as f32);
        let bottom = (rect.y + rect.height).min((self.at.y + self.height) as f32);
        if right <= left || bottom <= top {
            return None;
        }
        let cut = crate::atlas::Region {
            u: region.u + region.du * (left - rect.x) / rect.width,
            v: region.v + region.dv * (top - rect.y) / rect.height,
            du: region.du * (right - left) / rect.width,
            dv: region.dv * (bottom - top) / rect.height,
        };
        Some((
            Rect {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            },
            cut,
        ))
    }

    /// Every quad cut to this box, dropping the ones wholly outside it.
    ///
    /// The door for the quads a scissor cannot be carried on: a glyph is
    /// collected into the font atlas's own pass, so a window's text is cut here
    /// after the fact rather than per label. Pictures carry theirs on
    /// [`Picture::scissor`] instead, because a picture is also *picked*, and a
    /// hit test reading a different box from the one that drew would answer for
    /// a row that is not on the screen.
    pub fn cut(self, quads: &mut Vec<SpriteQuad>) {
        quads.retain_mut(|quad| match self.crop(quad.rect, quad.region) {
            Some((rect, region)) => {
                quad.rect = rect;
                quad.region = region;
                true
            }
            None => false,
        });
    }
}

/// Which of the client's two art files a picture in a window comes out of.
///
/// A window is not built from one file. Its frame, its buttons and its
/// background are `gumpartLegacyMUL.uop`; the *things in it* are the world's own
/// statics — the icons in a container, and what a `{ tilepic }` names — and
/// those live in `art.mul` beside the ground.
///
/// The two are separate 16-bit index spaces and they **overlap numerically**:
/// `0x003C` is a chest's gump and also an item's graphic. So an atlas holding
/// both cannot be keyed by a bare [`Graphic`] — one of the two pictures would
/// silently answer for the other, and the failure would be a window drawn with
/// the wrong picture rather than an error. This is the key that says which.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum GumpArt {
    /// A picture from `gumpartLegacyMUL.uop`.
    Gump(Graphic),
    /// A static from `art.mul` — an item's icon.
    Item(Graphic),
}

/// The two files a window's pictures are read from.
///
/// Together rather than one at a time because a single window needs both: a
/// container is a gump background with statics laid on it, and an atlas asked to
/// grow for one window has to be able to reach either file.
#[derive(Clone, Copy, Debug)]
pub struct ArtFiles<'a> {
    /// `gumpartLegacyMUL.uop`.
    pub gumps: &'a Gumps,
    /// `art.mul` — the same reader the world's statics come out of.
    pub items: &'a Art,
}

/// Gump art could not be read or could not be packed.
#[derive(Debug)]
#[non_exhaustive]
pub enum GumpAtlasError {
    /// The client's gump container refused a picture.
    Art(GumpError),
    /// The client's static art refused a picture.
    Item(ArtError),
    /// The atlas had no room for it, or it is bigger than the whole atlas.
    Atlas(AtlasError),
}

impl fmt::Display for GumpAtlasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Art(source) => write!(f, "{source}"),
            Self::Item(source) => write!(f, "{source}"),
            Self::Atlas(source) => write!(f, "{source}"),
        }
    }
}

impl std::error::Error for GumpAtlasError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Art(source) => Some(source),
            Self::Item(source) => Some(source),
            Self::Atlas(source) => Some(source),
        }
    }
}

impl From<GumpError> for GumpAtlasError {
    fn from(source: GumpError) -> Self {
        Self::Art(source)
    }
}

impl From<ArtError> for GumpAtlasError {
    fn from(source: ArtError) -> Self {
        Self::Item(source)
    }
}

impl From<AtlasError> for GumpAtlasError {
    fn from(source: AtlasError) -> Self {
        Self::Atlas(source)
    }
}

/// The gump pictures a frame needs, packed into one texture.
///
/// A wrapper around [`StaticAtlas`] rather than a fifth copy of the shelf
/// packer: what a static and a gump have in common is *everything* about
/// packing — an arbitrary rectangle of RGBA with transparency, shelved tallest
/// first, grown into as more is asked for — and the differences are entirely in
/// where the pictures come from and where they end up on the screen, which is
/// this module either side of the wrapper.
///
/// One field of the [`Sprite`] that comes out is meaningless here and is never
/// read: `face`, which asks which edge of a tile a *wall* stands on. A gump
/// stands on nothing. It is computed anyway, once per picture, because the
/// alternative is a packer of our own to avoid one walk over pixels that are
/// already in cache.
#[derive(Debug)]
pub struct GumpAtlas {
    packed: StaticAtlas,
    /// Every picture ever offered, whether or not the client ships art for it,
    /// and the slot the packer under this knows it by.
    ///
    /// Two jobs in one map. The first is the guard [`StaticAtlas`] keeps for the
    /// same reason: a gump the client has no picture for is never packed, so
    /// without this the atlas would try to decode it again on every frame it is
    /// asked for — and decoding a gump is a zlib inflate and a second pass over
    /// the result, not a lookup.
    ///
    /// The second is the reason the value is not `()`. The packer is keyed by a
    /// [`Graphic`] and this atlas holds two index spaces that collide (see
    /// [`GumpArt`]), so what goes down to it is a **slot this atlas handed out**
    /// — a number with no meaning outside these two fields, which is why it
    /// never leaves them.
    slots: BTreeMap<GumpArt, Graphic>,
    /// The next slot to hand out.
    next_slot: u16,
}

impl GumpAtlas {
    /// An atlas holding nothing, ready to be grown into.
    pub fn empty() -> Self {
        Self {
            packed: StaticAtlas::pack(std::iter::empty())
                .expect("packing no pictures cannot overflow an atlas"),
            slots: BTreeMap::new(),
            next_slot: 0,
        }
    }

    /// The slot for a picture, handing out a fresh one if it has none.
    ///
    /// `None` when this atlas has already handed out all 65,536 — which no real
    /// client reaches (`gumpartLegacyMUL.uop` is 5,556 entries and the statics a
    /// window ever shows are in the hundreds) and which is a full atlas long
    /// before it is a full slot space, since the texture is 2048 on a side.
    fn slot(&mut self, art: GumpArt) -> Option<Graphic> {
        if let Some(slot) = self.slots.get(&art) {
            return Some(*slot);
        }
        let slot = Graphic(self.next_slot);
        self.next_slot = self.next_slot.checked_add(1)?;
        self.slots.insert(art, slot);
        Some(slot)
    }

    /// Pack pictures somebody else decoded.
    ///
    /// What a test uses, and the seam a paperdoll's own art comes through: not
    /// every picture a gump window is built from lives in either file this
    /// module reads — an item's paperdoll layer is one of them — and this is
    /// where anything that has been turned into an [`Image`] joins the same
    /// atlas.
    pub fn pack(pictures: impl IntoIterator<Item = (GumpArt, Image)>) -> Result<Self, GumpAtlasError> {
        let mut atlas = Self::empty();
        let mut slotted = Vec::new();
        for (art, image) in pictures {
            let Some(slot) = atlas.slot(art) else {
                return Err(GumpAtlasError::Atlas(AtlasError::Full {
                    wanted: usize::from(u16::MAX) + 2,
                    capacity: usize::from(u16::MAX) + 1,
                }));
            };
            slotted.push((slot, image));
        }
        atlas.packed.pack_more(slotted)?;
        Ok(atlas)
    }

    /// Pack every picture in `wanted` that the client actually ships.
    pub fn build(
        files: ArtFiles<'_>,
        wanted: impl IntoIterator<Item = GumpArt>,
    ) -> Result<Self, GumpAtlasError> {
        let mut atlas = Self::empty();
        atlas.add(files, wanted)?;
        atlas.take_dirty();
        Ok(atlas)
    }

    /// Pack whichever of `wanted` this atlas has not been offered before.
    ///
    /// A picture the client ships no art for is skipped rather than refused: a
    /// layout naming a gump the client has no picture for is the shard's
    /// business, and dropping the window would be a worse answer than drawing
    /// the rest of it. The same goes for an item graphic with no static — a
    /// container holding one draws the bag and an empty space in it.
    pub fn add(
        &mut self,
        files: ArtFiles<'_>,
        wanted: impl IntoIterator<Item = GumpArt>,
    ) -> Result<(), GumpAtlasError> {
        let fresh: Vec<GumpArt> = wanted
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|art| !self.slots.contains_key(art))
            .collect();
        if fresh.is_empty() {
            return Ok(());
        }
        let mut images: Vec<(Graphic, Image)> = Vec::with_capacity(fresh.len());
        for art in &fresh {
            let decoded = match art {
                GumpArt::Gump(graphic) => files.gumps.gump(*graphic)?,
                GumpArt::Item(graphic) => files.items.static_art(*graphic)?,
            };
            // A zero-sized entry is a shipped stub, not a failure — see
            // `gumpart`'s "Some entries really are 0x0" — and packing one would
            // ask the shelf for a rectangle with no area.
            let Some(image) = decoded else { continue };
            if image.width() == 0 || image.height() == 0 {
                continue;
            }
            let Some(slot) = self.slot(*art) else {
                return Err(GumpAtlasError::Atlas(AtlasError::Full {
                    wanted: self.slots.len() + 1,
                    capacity: usize::from(u16::MAX) + 1,
                }));
            };
            images.push((slot, image));
        }
        self.packed.pack_more(images)?;
        // Everything offered is remembered, art or no art: a picture the client
        // does not ship must not be decoded again on the next frame that asks.
        for art in fresh {
            if self.slot(art).is_none() {
                return Err(GumpAtlasError::Atlas(AtlasError::Full {
                    wanted: self.slots.len() + 1,
                    capacity: usize::from(u16::MAX) + 1,
                }));
            }
        }
        Ok(())
    }

    /// The rows written since this was last asked, cleared. Handed straight to
    /// [`GumpRenderer::upload_rows`].
    pub fn take_dirty(&mut self) -> Option<std::ops::Range<u32>> {
        self.packed.take_dirty()
    }

    /// Its pixels, RGBA8 and row-major, ready for `write_texture`.
    pub fn pixels(&self) -> &[u8] {
        self.packed.pixels()
    }

    /// Where a graphic sits and how big it is, or `None` if it is not packed —
    /// either because nothing asked for it yet or because the client ships no
    /// art for it.
    pub fn sprite(&self, art: GumpArt) -> Option<Sprite> {
        self.packed.sprite(*self.slots.get(&art)?)
    }

    /// Whether `pixel` within a graphic's own picture is drawn
    /// rather than transparent.
    ///
    /// The interface's picking, and the same question
    /// [`StaticAtlas::opaque_at`] answers for the world: a gump window is not a
    /// rectangle — a paperdoll's frame has transparent corners, and a click in
    /// one belongs to whatever is behind it.
    pub fn opaque_at(&self, art: GumpArt, pixel: GumpArtPixel) -> bool {
        self.slots
            .get(&art)
            .is_some_and(|slot| self.packed.opaque_at(*slot, pixel.x, pixel.y))
    }

    /// How many pictures landed in it.
    pub fn len(&self) -> usize {
        self.packed.len()
    }

    /// Whether none did.
    pub fn is_empty(&self) -> bool {
        self.packed.is_empty()
    }
}

/// One picture a gump asks for: a graphic, where it goes, and how it fills.
///
/// The unit both [`collect`] and [`resize`] speak in, so that a nine-slice
/// background, a button and a paperdoll layer are one list by the time anything
/// draws them — which is what keeps them in painter's order relative to each
/// other without a sort.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Picture {
    /// Which picture, and which of the two files it comes out of.
    pub graphic: GumpArt,
    /// Its top-left corner, in the window's own gump pixels.
    pub at: GumpPixel,
    /// The hue to tint it with, or [`Hue::NONE`].
    pub hue: Hue,
    /// Opacity applied after the hue.  Ordinary gump art is opaque; a
    /// paperdoll's pending equipment preview is deliberately translucent.
    pub opacity: u8,
    /// The box to repeat it over, or `None` to draw it once at its own size.
    ///
    /// This is `gumppictiled`, and the nine-slice's edges and middle: the art
    /// is a strip a few pixels across and the window is however wide the server
    /// asked for. A box smaller than the art is a *clip*, not a squeeze — the
    /// reference tiles with a scissor rectangle and never scales gump art,
    /// which is what keeps a border's pixels its own size in a window of any
    /// width.
    pub tiled: Option<(i32, i32)>,
    /// The box outside which this picture is neither drawn nor picked, or `None`
    /// to let it stand wherever it was placed.
    ///
    /// On the picture rather than applied to the finished quads because a
    /// picture is *picked* as well as drawn, and [`pick`] walks this list: a hit
    /// test that read a different box — or none — would answer for a row
    /// scrolled out of its own window, which is a click landing on something
    /// nobody can see.
    pub scissor: Option<Scissor>,
}

impl Picture {
    /// A picture drawn once, at its own size, untinted.
    pub const fn plain(graphic: GumpArt, at: GumpPixel) -> Self {
        Self {
            graphic,
            at,
            hue: Hue::NONE,
            opacity: u8::MAX,
            tiled: None,
            scissor: None,
        }
    }

    /// The same picture, drawn and picked only inside `scissor`.
    pub const fn inside(self, scissor: Scissor) -> Self {
        Self {
            scissor: Some(scissor),
            ..self
        }
    }

    /// The same picture, tinted.
    pub const fn hued(self, hue: Hue) -> Self {
        Self { hue, ..self }
    }

    /// The same picture, composited at an opacity below full strength.
    pub const fn translucent(self, opacity: u8) -> Self {
        Self { opacity, ..self }
    }

    /// The same picture, repeated to fill `width` by `height`.
    pub const fn tiled(self, width: i32, height: i32) -> Self {
        Self {
            tiled: Some((width, height)),
            ..self
        }
    }

    /// The same picture, moved by `by`: its own corner and, if it has one,
    /// its scissor box move together — a scissor is only meaningful next to
    /// the picture it crops.
    ///
    /// The one place a window-local picture (`docs/window_components.md`'s
    /// window-local coordinates) becomes a screen-space one: every window
    /// kind lays its pictures out at the origin now, and this is what the
    /// draw pass and the pointer pick both use to add the window's own
    /// placement back, once, rather than baking it into the layout.
    #[must_use]
    pub const fn offset(self, by: GumpPixel) -> Self {
        Self {
            at: self.at.offset(by),
            scissor: match self.scissor {
                Some(scissor) => Some(scissor.offset(by)),
                None => None,
            },
            ..self
        }
    }
}

/// Every picture as quads, in the order given.
///
/// Painter's order and nothing else: this pass has no depth, so the caller's
/// order is what covers what, and a background listed before its buttons is
/// exactly what an interface means by "behind".
///
/// A picture whose graphic is not in the atlas is skipped — see
/// [`GumpAtlas::add`], which is where a missing one was already decided not to
/// be an error — and a tiled box smaller than one repetition is clipped to the
/// box, never squeezed into it.
pub fn collect(pictures: &[Picture], atlas: &GumpAtlas) -> Vec<SpriteQuad> {
    let mut quads = Vec::with_capacity(pictures.len());
    for picture in pictures {
        let Some(sprite) = atlas.sprite(picture.graphic) else {
            continue;
        };
        let (art_width, art_height) = (i32::from(sprite.width), i32::from(sprite.height));
        if art_width <= 0 || art_height <= 0 {
            continue;
        }
        let (width, height) = picture.tiled.unwrap_or((art_width, art_height));
        if width <= 0 || height <= 0 {
            continue;
        }
        let hue = u32::from(picture.hue.0);
        let mut y = 0;
        while y < height {
            let rows = (height - y).min(art_height);
            let mut x = 0;
            while x < width {
                let columns = (width - x).min(art_width);
                let rect = Rect {
                    x: (picture.at.x + x) as f32,
                    y: (picture.at.y + y) as f32,
                    width: columns as f32,
                    height: rows as f32,
                };
                // The clipped part of the region, from the same corner: a
                // half-drawn repetition shows the *left* of the art, which
                // is what a scissor rectangle would have left of it.
                let region = crate::atlas::Region {
                    u: sprite.region.u,
                    v: sprite.region.v,
                    du: sprite.region.du * columns as f32 / art_width as f32,
                    dv: sprite.region.dv * rows as f32 / art_height as f32,
                };
                // Cut per repetition rather than per picture, which is what
                // lets a tiled background be cut too: each repetition is its
                // own quad and meets the edge in its own place.
                let cut = match picture.scissor {
                    Some(scissor) => scissor.crop(rect, region),
                    None => Some((rect, region)),
                };
                let Some((rect, region)) = cut else {
                    x += art_width;
                    continue;
                };
                quads.push(
                    SpriteQuad {
                        rect,
                        region,
                        // No depth test in this pass; see the module docs.
                        depth: 0.0,
                        hue,
                        // A gump stands on no tile and is never lit.
                        place: crate::place::Place::NOWHERE,
                        twin: 0,
                        owner: u32::from(crate::occlusion::OwnerId::NONE.raw()),
                        volumes: crate::impostor::Range::default(),
                    }
                    .with_opacity(picture.opacity),
                );
                x += art_width;
            }
            y += art_height;
        }
    }
    quads
}

/// Which picture in a laid-out window the cursor is over, topmost first.
///
/// The hit test the whole interface is picked by, and deliberately the same walk
/// [`collect`] draws by: a window is a list of pictures in painter's order, so
/// the *last* one drawn is the first one picked, and anything asking a different
/// question of a different list would eventually disagree with what is on the
/// screen. The answer is an index into `pictures`, because what a hit *means* —
/// a window to drag, an item to lift, a button to press — is the caller's and
/// differs per window kind.
///
/// Against opaque texels rather than bounding boxes, for the reason
/// [`GumpAtlas::opaque_at`] exists: gump art is mostly empty space, and a
/// paperdoll's frame is a picture with a large transparent middle that its own
/// doll is drawn into.
///
/// A picture the atlas does not hold is not pickable — it is not drawn either
/// (see [`collect`]), and picking something invisible is the one answer that is
/// certainly wrong. A tiled picture is tested against the box it fills, with the
/// texel taken modulo the art's own size, which is where the repetition puts it.
pub fn pick(pictures: &[Picture], cursor: GumpPixel, atlas: &GumpAtlas) -> Option<PictureIndex> {
    pictures.iter().enumerate().rev().find_map(|(index, picture)| {
        // Outside the box that drew it is outside the picture, whatever its own
        // corner says: half a row is on the screen and the other half is not.
        if picture.scissor.is_some_and(|scissor| !scissor.contains(cursor)) {
            return None;
        }
        let sprite = atlas.sprite(picture.graphic)?;
        let (art_width, art_height) = (i32::from(sprite.width), i32::from(sprite.height));
        if art_width <= 0 || art_height <= 0 {
            return None;
        }
        let (width, height) = picture.tiled.unwrap_or((art_width, art_height));
        let (x, y) = (cursor.x - picture.at.x, cursor.y - picture.at.y);
        if x < 0 || y < 0 || x >= width || y >= height {
            return None;
        }
        atlas
            .opaque_at(
                picture.graphic,
                GumpArtPixel::new((x % art_width) as u16, (y % art_height) as u16),
            )
            .then_some(PictureIndex::new(index))
    })
}

/// A `resizepic`: a window background of any size, out of nine pictures.
///
/// The server names one graphic and a rectangle; the client draws `graphic` to
/// `graphic + 8` as corners, edges and a middle, tiling the six that are not
/// corners. **The nine are not in reading order** — the middle is `+4`, between
/// the left edge at `+3` and the right at `+5` — which is a port of
/// `ResizePic.GetTexture`'s remap (`index >= 8 ? 4 : index >= 4 ? index + 1 :
/// index`) and the one thing here that cannot be guessed from the pictures.
///
/// The four offsets are `ResizePic.DrawInternal`'s, taken as they are written
/// there: the client's own art is not a tidy nine-slice — the top-left and
/// top-right corners of a real background differ in height, and the top edge is
/// shorter than both — so each is the difference the reference nudges a piece
/// by to keep the seam closed. Recomputing them "properly" from the rectangle
/// leaves a one-pixel gap along the top of every window.
///
/// Pieces the client ships no art for are skipped by [`collect`], so a graphic
/// with fewer than nine entries degrades to whatever it does have.
pub fn resize(atlas: &GumpAtlas, graphic: Graphic, at: GumpPixel, width: i32, height: i32) -> Vec<Picture> {
    // The nine, in `DrawInternal`'s own numbering: 0-3 are the top-left corner,
    // top edge, top-right corner and left edge; 4 is the *right* edge; 5-7 are
    // the bottom row; 8 is the middle. `piece` is the remap.
    let piece = |slot: u16| GumpArt::Gump(Graphic(graphic.0.wrapping_add(RESIZE_PIECES[slot as usize])));
    let size = |slot: u16| {
        atlas
            .sprite(piece(slot))
            .map(|sprite| (i32::from(sprite.width), i32::from(sprite.height)))
            .unwrap_or((0, 0))
    };
    let (w0, h0) = size(0);
    let (_w1, h1) = size(1);
    let (w2, h2) = size(2);
    let (w3, _h3) = size(3);
    let (w4, _h4) = size(4);
    let (w5, h5) = size(5);
    let (_w6, h6) = size(6);
    let (w7, h7) = size(7);

    let offset_top = h0.max(h2) - h1;
    let offset_bottom = h5.max(h7) - h6;
    let offset_left = (w0.max(w5) - w2).abs();
    let offset_right = w2.max(w7) - w4;

    let corner = |slot: u16, x: i32, y: i32| Picture::plain(piece(slot), at.offset(GumpPixel::new(x, y)));
    let strip = |slot: u16, x: i32, y: i32, w: i32, h: i32| {
        Picture::plain(piece(slot), at.offset(GumpPixel::new(x, y))).tiled(w, h)
    };

    vec![
        corner(0, 0, 0),
        strip(1, w0, 0, width - w0 - w2, h1),
        corner(2, width - w2, offset_top),
        strip(3, 0, h0, w3, height - h0 - h5),
        strip(4, width - w4, h2, w4, height - h2 - h7),
        corner(5, 0, height - h5),
        strip(6, w5, height - h6 - offset_bottom, width - w5 - w7, h6),
        corner(7, width - w7, height - h7),
        strip(
            8,
            w0,
            h0,
            (width - w0 - w2) + (offset_left + offset_right),
            height - h2 - h7,
        ),
    ]
}

/// Which gump each of [`resize`]'s nine slots actually is, as an offset from the
/// graphic the server named. `ResizePic.GetTexture`'s remap, flattened.
const RESIZE_PIECES: [u16; 9] = [0, 1, 2, 3, 5, 6, 7, 8, 4];

/// Every gump graphic a layout can draw, whichever page is showing.
///
/// What the atlas is grown with before [`window`] runs, and it has to be every
/// page rather than the one being shown: a page flip is a click, and an atlas
/// grown on the frame after it would draw the new page empty for one frame.
///
/// A `{ resizepic }` contributes **nine** graphics, not one — see [`resize`] —
/// and `window` cannot place one until all nine have been packed, because where
/// its edges go is decided by how big its corners turned out to be. That
/// dependency is the reason this exists at all rather than the atlas growing
/// from whatever `window` happened to name.
pub fn art_of(elements: &[Element]) -> BTreeSet<GumpArt> {
    let mut wanted = BTreeSet::new();
    let want = |gump: u32| GumpArt::Gump(Graphic(gump as u16));
    for element in elements {
        match element {
            Element::Background { gump, .. } => {
                for piece in RESIZE_PIECES {
                    wanted.insert(want(u32::from((*gump as u16).wrapping_add(piece))));
                }
            }
            Element::Image { gump, .. } | Element::ImageTiled { gump, .. } => {
                wanted.insert(want(*gump));
            }
            Element::Button { normal, pressed, .. } => {
                wanted.insert(want(*normal));
                wanted.insert(want(*pressed));
            }
            Element::Check(switch) | Element::Radio(switch) => {
                wanted.insert(want(switch.off));
                wanted.insert(want(switch.on));
            }
            // `{ tilepic }` is the one element whose picture is not a gump at
            // all — see `GumpArt`.
            Element::Item { graphic, .. } => {
                wanted.insert(GumpArt::Item(Graphic(*graphic as u16)));
            }
            _ => {}
        }
    }
    wanted
}

/// The face a layout's `{ text }` and `{ croppedtext }` are drawn in.
///
/// **An approximation, and the only one in this module.** The reference draws
/// both through a *Unicode* face — `new Label(text, true, hue, 0, ...)`, where
/// the `true` is `isUnicode` and the `0` is the face — which lives in
/// `unifont.mul`, a file `openshard_uofiles` has no reader for. `fonts.mul`'s
/// face 1 is the nearest thing the client ships: the same face `PaperDollGump`
/// names outright for its own title (see [`crate::paperdoll::NAME_FONT`]), and
/// small enough that a caption laid out for the real one still fits its box.
///
/// What it costs is the character set: `fonts.mul` is single-byte, so a shard
/// that writes a dialog in anything past Latin-1 gets those glyphs skipped
/// rather than drawn — see [`crate::text::collect_gump`]. `docs/client.md`
/// carries the backlog entry.
pub const CAPTION_FONT: Font = Font(1);

/// One line of a window's text, resolved to where it goes.
///
/// The text itself is *not* here: a layout names either a row of the table
/// that arrived beside the packet (`{ text }`, `{ croppedtext }`,
/// `{ htmlgump }`) or a number in the client's own `Cliloc.enu`
/// (`{ xmfhtmlgump }` and kin) — and turning either into a string needs
/// something this crate has never heard of and should not: the whole
/// [`OpenGump`](openshard_client_net::view::OpenGump) for the first, a loaded
/// cliloc table for the second. So this carries only which source and which
/// key, and whoever holds both turns it into glyphs through [`crate::text`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Caption {
    /// Its top-left corner, in the window's own gump pixels.
    pub at: GumpPixel,
    /// The hue the layout asked for.
    pub hue: Hue,
    /// Where the text comes from.
    pub source: CaptionSource,
    /// The box it is clipped to — `{ croppedtext }` — or `None` for anything
    /// else, which overflows rather than clipping.
    pub clip: Option<(i32, i32)>,
}

/// Which table a [`Caption`]'s text is a key into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptionSource {
    /// A row of the gump's own text table — wire-carried, so it exists only
    /// alongside the packet that named it.
    Line(usize),
    /// A number in the client's own `Cliloc.enu`. Nothing travelled for this
    /// one; the shard sent the number on the belief every client already has
    /// the sentence.
    Cliloc(u32),
}

/// What a click on one of a window's pictures means.
///
/// The whole of a `0xB0`'s interactivity, and deliberately a *fourth* list
/// beside the pictures rather than a field on [`Picture`]: what is drawn and
/// what answers the mouse are two different questions about the same list, and
/// only some of the answers are on the wire at all. Two of these four never
/// reach the server — see [`Window::hits`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    /// A reply button. The server hears this one, and the window closes on it.
    Reply(RawButtonId),
    /// A page button: show this page instead. `{ button ... 0 N id }`, which is
    /// answered inside the client and which the server is never told about.
    Page(u32),
    /// A checkbox, which answers for itself.
    Check(RawSwitchId),
    /// A radio, which turns the rest of its group off on the way on. Which
    /// switches *are* its group is the layout's, not this crate's: the wire has
    /// no group id, and the client that sends two of one group set is the one
    /// at fault.
    Radio(RawSwitchId),
}

/// A `{ textentry }`: a box the player types into.
///
/// The one thing in a window that is a rectangle and not a picture, and so the
/// one thing [`pick`] cannot answer about — there is no art to test a texel of.
/// It is picked by its box instead ([`field`]), which is exactly what the
/// reference does with it: an `StbTextBox` is a control with bounds and no
/// texture of its own.
///
/// What is *in* it is not here. A field starts out holding a line of the gump's
/// text table and holds whatever has been typed since, and both of those belong
/// to whoever owns the keyboard — see `client/app`'s `Dialogs`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Field {
    /// Its top-left corner, in the window's own gump pixels.
    pub at: GumpPixel,
    /// How wide and how tall the box is.
    pub size: (i32, i32),
    /// The id its contents come back to the server under.
    pub id: u16,
    /// Which line of the gump's text table it starts out holding.
    pub line: usize,
    /// The hue its text is drawn in, already a wire hue — see [`text_hue`].
    pub hue: Hue,
}

/// Which field the cursor is in, if any.
///
/// A box and not a texel, for the reason [`Field`] gives, and the *last* one
/// wins for [`pick`]'s reason: later in the list is later drawn. Two fields that
/// overlap are a layout the shard should not have sent, and answering with the
/// topmost is the same answer the picture walk would give.
pub fn field(fields: &[Field], cursor: GumpPixel) -> Option<u16> {
    fields.iter().rev().find_map(|field| {
        let (x, y) = (cursor.x - field.at.x, cursor.y - field.at.y);
        (x >= 0 && y >= 0 && x < field.size.0 && y < field.size.1).then_some(field.id)
    })
}

/// A window, laid out: what to draw, what to write on it, and what answers the
/// mouse.
///
/// Three lists and not one. The first two are drawn through two atlases — gump
/// art and a font — and a draw call binds one texture; their order relative to
/// each other is fixed rather than interleaved: every picture, then every
/// caption, which is what the reference does by drawing text controls after the
/// background they sit on. A layout that put a picture *over* a label would
/// draw it under one here, and no gump in the wild does.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Window {
    /// The art, in painter's order.
    pub pictures: Vec<Picture>,
    /// The text on top of it.
    pub captions: Vec<Caption>,
    /// Which pictures answer the mouse, by their index in `pictures`.
    ///
    /// Keyed by index rather than carried on the picture itself, because
    /// [`pick`] answers an index and this is the table that turns one into a
    /// meaning — the two halves of one lookup, and a window whose art and hit
    /// test came from two different walks is exactly what
    /// [`crate::container::pick`]'s docs refuse. A background's nine pieces, a
    /// `{ gumppic }` and a caption are simply not in it.
    pub hits: BTreeMap<PictureIndex, Hit>,
    /// The boxes the player can type into, which are not pictures at all — see
    /// [`Field`].
    pub fields: Vec<Field>,
}

impl Window {
    /// Which button or switch owns `cursor`.
    ///
    /// A gump control's rectangle is interactive even where its bitmap is
    /// transparent. Testing its ink instead made the visible margins of many
    /// buttons dead zones, while the reference's `Button` owns its full bounds.
    #[must_use]
    pub fn hit(&self, cursor: GumpPixel, atlas: &GumpAtlas) -> Option<Hit> {
        self.hits.iter().rev().find_map(|(index, hit)| {
            let picture = self.pictures.get(index.position())?;
            let sprite = atlas.sprite(picture.graphic)?;
            (cursor.x >= picture.at.x
                && cursor.y >= picture.at.y
                && cursor.x < picture.at.x + i32::from(sprite.width)
                && cursor.y < picture.at.y + i32::from(sprite.height))
            .then_some(*hit)
        })
    }
}

/// Lay a parsed layout out at `at`, showing `page`.
///
/// The three pieces of state a window has that the wire does not carry, and
/// each one is why this takes an argument rather than reading the layout alone:
///
/// - `page` — `{ button ... 0 N id }` flips pages inside the client and the
///   server never hears about it. Page `0` is drawn on every page, which is the
///   client's rule and the reason a background declared before the first
///   `{ page 1 }` is on all of them.
/// - `on` — which switches are set. `{ checkbox }` carries its *initial* state
///   and nothing else; after the player has touched one, the layout is stale.
/// - `held` — which button is being pressed right now, drawn in its second
///   picture. Nothing sends this and nothing records it: it is the mouse. It
///   is a [`Hit`] and not a button id because that is what a press *is* here —
///   the same value [`pick`] and [`Window::hits`] answer with, so what is drawn
///   pressed and what the release will act on cannot be two different things.
///
/// `{ checkertrans }` is not drawn. It is a darkened translucent rectangle and
/// this pass has no blending — see [`GumpRenderer`] — so it is left out rather
/// than drawn opaque, which would black out whatever it was meant to shade.
pub fn window(
    elements: &[Element],
    at: GumpPixel,
    page: u32,
    on: &BTreeSet<RawSwitchId>,
    held: Option<Hit>,
    atlas: &GumpAtlas,
) -> Window {
    let mut drawn = Window::default();
    // Everything before the first `{ page }` belongs to page 0 — the layer every
    // page shows — which is where a background and a frame almost always are.
    let mut current = 0;
    let art = |gump: u32| GumpArt::Gump(Graphic(gump as u16));
    for element in elements {
        if let Element::Page(number) = element {
            current = *number;
            continue;
        }
        if current != 0 && current != page {
            continue;
        }
        match element {
            Element::Background {
                x,
                y,
                width,
                height,
                gump,
            } => drawn.pictures.extend(resize(
                atlas,
                Graphic(*gump as u16),
                at.offset(GumpPixel::new(*x, *y)),
                *width,
                *height,
            )),
            Element::Image { x, y, gump, hue } => drawn.pictures.push(
                Picture::plain(art(*gump), at.offset(GumpPixel::new(*x, *y)))
                    .hued(Hue(hue.unwrap_or(0) as u16)),
            ),
            Element::ImageTiled {
                x,
                y,
                width,
                height,
                gump,
            } => drawn
                .pictures
                .push(Picture::plain(art(*gump), at.offset(GumpPixel::new(*x, *y))).tiled(*width, *height)),
            Element::Button {
                x,
                y,
                normal,
                pressed,
                kind,
                page: target,
                id,
            } => {
                // The two kinds are two different ends of a click, and the
                // difference is not cosmetic: one packet leaves and one does
                // not. See [`Hit`].
                let hit = match kind {
                    GumpButton::Page => Hit::Page(*target),
                    GumpButton::Reply => Hit::Reply(*id),
                };
                let face = if held == Some(hit) { *pressed } else { *normal };
                drawn
                    .pictures
                    .push(Picture::plain(art(face), at.offset(GumpPixel::new(*x, *y))));
                drawn
                    .hits
                    .insert(PictureIndex::new(drawn.pictures.len() - 1), hit);
            }
            Element::Check(switch) | Element::Radio(switch) => {
                let set = if on.contains(&switch.id) {
                    switch.on
                } else {
                    switch.off
                };
                drawn.pictures.push(Picture::plain(
                    art(set),
                    at.offset(GumpPixel::new(switch.x, switch.y)),
                ));
                let hit = match element {
                    Element::Radio(_) => Hit::Radio(switch.id),
                    _ => Hit::Check(switch.id),
                };
                drawn
                    .hits
                    .insert(PictureIndex::new(drawn.pictures.len() - 1), hit);
            }
            // A static from the world's art, laid on a window: an item in a
            // container, a reagent on a shopping list, the picture beside a
            // quest's text. The same art the ground draws it with, at its own
            // size and with no world transform on it at all.
            Element::Item { x, y, graphic, hue } => drawn.pictures.push(
                Picture::plain(
                    GumpArt::Item(Graphic(*graphic as u16)),
                    at.offset(GumpPixel::new(*x, *y)),
                )
                .hued(Hue(hue.unwrap_or(0) as u16)),
            ),
            Element::Label { x, y, hue, line } => drawn.captions.push(Caption {
                at: at.offset(GumpPixel::new(*x, *y)),
                hue: text_hue(*hue),
                source: CaptionSource::Line(*line),
                clip: None,
            }),
            Element::CroppedLabel {
                x,
                y,
                width,
                height,
                hue,
                line,
            } => drawn.captions.push(Caption {
                at: at.offset(GumpPixel::new(*x, *y)),
                hue: text_hue(*hue),
                source: CaptionSource::Line(*line),
                clip: Some((*width, *height)),
            }),
            // `{ htmlgump }` — wire-carried text, the same table `{ text }`
            // reads, just longer and wrapped rather than one line. The wire
            // form carries no separate hue (a real client colours it, if at
            // all, with an embedded `<basefont>` tag this pass does not
            // parse), so it draws in the layout's default white rather than
            // untinted — the same fallback `{ xmfhtmlgump }` below takes.
            Element::Html {
                x,
                y,
                width,
                height,
                line,
                ..
            } => drawn.captions.push(Caption {
                at: at.offset(GumpPixel::new(*x, *y)),
                hue: text_hue(GUMP_WHITE),
                source: CaptionSource::Line(*line),
                clip: Some((*width, *height)),
            }),
            // The `xmfhtml*` family — a cliloc number, not a line: nothing
            // travelled on the wire for this one, so resolving it needs the
            // client's own `Cliloc.enu`, which whoever draws `drawn.captions`
            // holds and this crate does not. See [`CaptionSource::Cliloc`].
            Element::Localized {
                x,
                y,
                width,
                height,
                cliloc,
            } => drawn.captions.push(Caption {
                at: at.offset(GumpPixel::new(*x, *y)),
                hue: text_hue(GUMP_WHITE),
                source: CaptionSource::Cliloc(*cliloc),
                clip: Some((*width, *height)),
            }),
            Element::TextEntry {
                x,
                y,
                width,
                height,
                hue,
                entry_id,
                line,
            } => drawn.fields.push(Field {
                at: at.offset(GumpPixel::new(*x, *y)),
                size: (*width, *height),
                id: *entry_id,
                line: *line,
                hue: text_hue(*hue),
            }),
            // Everything else is either state the caller already read (a flag),
            // text this crate cannot lay out yet (html), or the translucent
            // rectangle the module docs above account for.
            _ => {}
        }
    }
    drawn
}

/// The wire hue a layout's text hue means.
///
/// **One more than the number in the layout**, and this is not an off-by-one to
/// be tidied away: a wire hue is one-based, with zero meaning "no tint at all"
/// (see [`openshard_uofiles::hues::Hues::get`]), and a gump's text hue is an
/// index into the same table written the other way round. The reference does
/// exactly this — `UInt16Converter.Parse(parts[3]) + 1` in both `Label`'s and
/// `CroppedText`'s constructors — and reading the layout's number as a wire hue
/// draws every line of every dialog in the colour of the row above the one the
/// shard asked for.
///
/// Only text. A `{ gumppic }`'s hue is already a wire hue and is passed through
/// as it stands, which is the asymmetry the reference has and not one invented
/// here.
fn text_hue(hue: u32) -> Hue {
    Hue((hue as u16).wrapping_add(1))
}

/// Where a gump frame is being drawn, and how big its pixels are.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    /// What to draw into — the surface, after the world has been blitted onto
    /// it and before egui paints anything over it.
    pub target: &'a wgpu::TextureView,
    /// Its width in real pixels.
    pub width: u32,
    /// Its height in real pixels.
    pub height: u32,
    /// Real pixels per gump pixel.
    ///
    /// `1.0` is the reference client exactly. Anything else should be a whole
    /// number: gump art is five-bit pixel art sampled with `Nearest`, and a
    /// fractional scale doubles some of its rows and not others, which shows up
    /// as a border that is two pixels thick along one edge of a window and one
    /// along the other.
    pub scale: f32,
}

/// Bytes of the gump pass's uniform block: the target's size and the scale,
/// rounded up to the sixteen a uniform block is sized in.
const GUMP_UNIFORM_BYTES: u64 = 16;

/// Quads the instance buffer starts out able to hold. Smaller than the world
/// passes' by two orders of magnitude, because it is: a screen full of open
/// windows is a few hundred pictures, and the buffer grows if it is wrong.
const INITIAL_QUADS: u64 = 256;

/// The pass that draws [`collect`]'s quads.
#[derive(Debug)]
pub struct GumpRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    quad: wgpu::Buffer,
    instances: wgpu::Buffer,
    /// Quads the instance buffer can hold before it has to be replaced.
    capacity: u64,
    /// The atlas texture, kept so that it can be grown into rather than
    /// replaced — see [`GumpRenderer::upload_rows`].
    atlas_texture: wgpu::Texture,
}

impl GumpRenderer {
    /// Upload an atlas and build the pipeline for a target of `format`.
    ///
    /// `format` is the **surface's**, not [`blit::WORLD_FORMAT`](crate::blit),
    /// because this pass draws onto the finished frame rather than into the
    /// world image.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        pixels: &[u8],
        hue_ramp: &HueRamp,
    ) -> Self {
        let side = crate::renderer::SPRITE_ATLAS_SIDE;
        let atlas_texture = upload(device, queue, "gump atlas", side, side, pixels);
        let view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let ramp_texture = upload(
            device,
            queue,
            "hue ramp",
            HueRamp::width(),
            hue_ramp.height(),
            hue_ramp.pixels(),
        );
        let ramp_view = ramp_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Nearest and clamped, for the world passes' reason and one more of its
        // own: a scaled gump is magnified pixel art, and any filtering at all
        // turns a one-pixel window border into a grey smear.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gump atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gump screen"),
            size: GUMP_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gumps"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gumps"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&ramp_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gumps"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gump.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gumps"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gumps"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        }],
                    }),
                    // The whole of `SpriteQuad`, of which this pass reads four
                    // fields: the depth and the place ride along unread rather
                    // than the quad having a second layout to keep in step.
                    Some(wgpu::VertexBufferLayout {
                        array_stride: SpriteQuad::STRIDE,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 0,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 8,
                                shader_location: 2,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 16,
                                shader_location: 3,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Uint32,
                                offset: 36,
                                shader_location: 4,
                            },
                        ],
                    }),
                ],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            // None, and that is the point of this pass: the world's depth
            // buffer ordered the world, and the interface is drawn on the
            // result in the order it is listed.
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // No blending: gump art is opaque where it is drawn at all
                    // and the shader discards where it is not, so there is
                    // nothing to average.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let quad = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gump quad"),
            size: std::mem::size_of_val(&QUAD) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut quad_bytes = Vec::with_capacity(QUAD.len() * 4);
        for value in QUAD {
            quad_bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&quad, 0, &quad_bytes);

        Self {
            pipeline,
            bind_group,
            uniforms,
            quad,
            instances: new_static_instance_buffer(device, INITIAL_QUADS),
            capacity: INITIAL_QUADS,
            atlas_texture,
        }
    }

    /// Send a band of the atlas that has grown since it was uploaded — what
    /// [`GumpAtlas::take_dirty`] just handed back.
    pub fn upload_rows(&self, queue: &wgpu::Queue, pixels: &[u8], rows: std::ops::Range<u32>) {
        write_rows(queue, &self.atlas_texture, pixels, rows);
    }

    /// Draw `quads` over what is already in `frame.target`.
    /// Draw `quads` over what is already on the target.
    ///
    /// **Once per frame per renderer.** The instances live in one buffer that
    /// this writes through [`wgpu::Queue::write_buffer`], and a queued write
    /// lands *before* the encoder it was queued beside is submitted — so a
    /// second call in the same frame does not add a second draw, it replaces
    /// the instances the first one was going to draw and leaves that pass
    /// drawing this call's quads instead. Collect everything and call once;
    /// painter's order inside the list is the only order there is anyway, since
    /// this pass has no depth.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame<'_>,
        quads: &[SpriteQuad],
    ) {
        if quads.is_empty() {
            // Nothing to draw and nothing to clear: this pass owns no pixels of
            // its own, so an empty one would only cost a submission — and on the
            // ordinary frame, with no window open, that is every frame.
            return;
        }

        let mut uniform_bytes = Vec::with_capacity(GUMP_UNIFORM_BYTES as usize);
        for value in [frame.width as f32, frame.height as f32, frame.scale, 0.0] {
            uniform_bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.uniforms, 0, &uniform_bytes);

        if quads.len() as u64 > self.capacity {
            self.capacity = (quads.len() as u64).next_power_of_two();
            self.instances = new_static_instance_buffer(device, self.capacity);
        }
        let mut instance_bytes = Vec::with_capacity(quads.len() * SpriteQuad::STRIDE as usize);
        for quad in quads {
            quad.write(&mut instance_bytes);
        }
        queue.write_buffer(&self.instances, 0, &instance_bytes);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gumps"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Loaded: the world is already on this surface.
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.set_vertex_buffer(1, self.instances.slice(..));
        pass.draw(0..4, 0..quads.len() as u32);
    }

    /// Draw one independently ordered interface layer.
    ///
    /// Unlike [`render`](Self::render), this owns an instance buffer for this
    /// one encoder command.  That makes it safe to interleave two atlases in a
    /// frame — a window's art, then its labels, then the next window — without
    /// a later `Queue::write_buffer` replacing the instances an earlier pass
    /// still refers to.
    pub fn render_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame<'_>,
        quads: &[SpriteQuad],
    ) {
        if quads.is_empty() {
            return;
        }
        let mut uniform_bytes = Vec::with_capacity(GUMP_UNIFORM_BYTES as usize);
        for value in [frame.width as f32, frame.height as f32, frame.scale, 0.0] {
            uniform_bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.uniforms, 0, &uniform_bytes);
        let instances = new_static_instance_buffer(device, quads.len() as u64);
        let mut instance_bytes = Vec::with_capacity(quads.len() * SpriteQuad::STRIDE as usize);
        for quad in quads {
            quad.write(&mut instance_bytes);
        }
        queue.write_buffer(&instances, 0, &instance_bytes);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gump layer"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.set_vertex_buffer(1, instances.slice(..));
        pass.draw(0..4, 0..quads.len() as u32);
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::gump::layout::Switch;
    use openshard_uofiles::color::Color16;

    use super::*;

    /// A solid picture of a given size, so that packing and placement can be
    /// tested without a client's files.
    fn block(width: u16, height: u16) -> Image {
        let pixels = vec![Color16(0x7FFF); usize::from(width) * usize::from(height)];
        Image::new(width, height, pixels)
    }

    /// An atlas of gump pictures handed in already decoded — through the same
    /// slot map the real one uses, so a test cannot accidentally look one up by
    /// a key the packer would never have been given.
    fn atlas_of(pictures: impl IntoIterator<Item = (Graphic, Image)>) -> GumpAtlas {
        GumpAtlas::pack(
            pictures
                .into_iter()
                .map(|(graphic, image)| (GumpArt::Gump(graphic), image)),
        )
        .expect("a handful of small blocks fit an atlas 2048 on a side")
    }

    /// A picture with a transparent middle — a frame, as far as picking is
    /// concerned: what a window's own art mostly is.
    fn framed(side: u16, hole: std::ops::Range<u16>) -> Image {
        let mut pixels = vec![Color16(0x7FFF); usize::from(side) * usize::from(side)];
        for y in hole.clone() {
            for x in hole.clone() {
                pixels[usize::from(y) * usize::from(side) + usize::from(x)] = Color16::TRANSPARENT;
            }
        }
        Image::new(side, side, pixels)
    }

    /// A picture cut by a scissor keeps its scale: what is drawn is the part
    /// inside the box, at one screen pixel per art pixel, and the region moves
    /// by exactly the fraction the rectangle lost. A squeeze would keep the
    /// whole art and shrink it, which is the bug this arithmetic exists to
    /// avoid — it is the same rule tiling already follows.
    #[test]
    fn a_scissor_cuts_a_picture_rather_than_squeezing_it() {
        let atlas = atlas_of([(Graphic(1), block(20, 20))]);
        let scissor = Scissor {
            at: GumpPixel::new(100, 110),
            width: 100,
            height: 100,
        };
        let whole = collect(
            &[Picture::plain(
                GumpArt::Gump(Graphic(1)),
                GumpPixel::new(100, 100),
            )],
            &atlas,
        );
        let cut = collect(
            &[Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(100, 100)).inside(scissor)],
            &atlas,
        );
        assert_eq!(cut.len(), 1);
        assert_eq!(cut[0].rect.y, 110.0, "the top ten rows are gone");
        assert_eq!(cut[0].rect.height, 10.0);
        assert_eq!(cut[0].rect.width, whole[0].rect.width, "and nothing else moved");
        // Half the rows dropped from the top means half the region dropped from
        // the top, so the art pixel under a given screen pixel is unchanged.
        assert!((cut[0].region.dv - whole[0].region.dv / 2.0).abs() < 1e-6);
        assert!((cut[0].region.v - (whole[0].region.v + whole[0].region.dv / 2.0)).abs() < 1e-6);
    }

    /// The identity `docs/window_components.md`'s window-local coordinates
    /// depend on: picking a window-local picture list against a *local*
    /// cursor — `client/app`'s `App::window_under_pointer` shape — answers
    /// exactly what picking that same list, each picture moved by the
    /// window's placement with [`Picture::offset`], answers against the
    /// *absolute* cursor — `client/app`'s `draw_gump_windows` shape. The two
    /// call sites add a window's position back in two different ways (one
    /// moves the cursor down, one moves the art up), and this is the proof
    /// neither can silently disagree with the other about which pixel a
    /// click landed on.
    #[test]
    fn offsetting_pictures_and_offsetting_the_cursor_pick_the_same_answer() {
        let atlas = atlas_of([(Graphic(1), block(40, 40)), (Graphic(2), block(10, 10))]);
        // A window's own art, laid out exactly as a pane's `layout` produces
        // it now: at the origin, as if the manager had put the window at
        // `(0, 0)`.
        let local = [
            Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(0, 0)),
            Picture::plain(GumpArt::Gump(Graphic(2)), GumpPixel::new(15, 15)).inside(Scissor {
                at: GumpPixel::new(15, 15),
                width: 10,
                height: 10,
            }),
        ];
        // Where the manager has actually put the window this frame.
        let placed = GumpPixel::new(300, 200);
        let cursor = GumpPixel::new(318, 218);

        // `own_windows.rs`'s way: leave the pictures alone, move the cursor
        // into the window's own space.
        let local_cursor = GumpPixel::new(cursor.x - placed.x, cursor.y - placed.y);
        let picked_by_local_cursor = pick(&local, local_cursor, &atlas);

        // `render_passes.rs`'s way: leave the cursor alone, move every
        // picture (and its scissor) into screen space.
        let screen_space: Vec<Picture> = local.iter().map(|picture| picture.offset(placed)).collect();
        let picked_by_offset_pictures = pick(&screen_space, cursor, &atlas);

        assert_eq!(
            picked_by_local_cursor, picked_by_offset_pictures,
            "the pointer pick and the draw pass must agree on which picture a click landed on"
        );
        assert_eq!(
            picked_by_local_cursor,
            Some(PictureIndex::new(1)),
            "the cursor is inside the smaller picture's scissor box"
        );
    }

    /// A picture wholly outside its box is not drawn at all — the ordinary case
    /// for a list of fifty-eight rows in a window ten tall.
    #[test]
    fn a_picture_outside_its_scissor_is_not_drawn() {
        let atlas = atlas_of([(Graphic(1), block(20, 20))]);
        let scissor = Scissor {
            at: GumpPixel::new(100, 100),
            width: 50,
            height: 50,
        };
        let quads = collect(
            &[Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(100, 400)).inside(scissor)],
            &atlas,
        );
        assert!(quads.is_empty());
    }

    /// The half of the box that matters more: a row scrolled out of its window
    /// is not pickable, or a click lands on something nobody can see. The
    /// picture is *still there*, at its own coordinates — this is the one test
    /// that says the hit test reads the same box the drawing did.
    #[test]
    fn a_scrolled_out_row_is_not_picked_where_it_is_not_drawn() {
        let atlas = atlas_of([(Graphic(1), block(20, 20))]);
        let scissor = Scissor {
            at: GumpPixel::new(100, 100),
            width: 50,
            height: 50,
        };
        let row = [Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(100, 140)).inside(scissor)];
        assert_eq!(
            pick(&row, GumpPixel::new(105, 145), &atlas),
            Some(PictureIndex::new(0)),
            "the part inside the box is the row"
        );
        assert_eq!(
            pick(&row, GumpPixel::new(105, 155), &atlas),
            None,
            "and the part below the box is not, though the picture covers it"
        );
    }

    /// A tiled picture is cut per repetition, because each repetition is its own
    /// quad and meets the edge in its own place — the case a per-picture cut
    /// would get wrong by dropping the whole strip.
    #[test]
    fn a_tiled_picture_is_cut_repetition_by_repetition() {
        let atlas = atlas_of([(Graphic(1), block(10, 10))]);
        let scissor = Scissor {
            at: GumpPixel::new(0, 0),
            width: 25,
            height: 10,
        };
        let quads = collect(
            &[Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(0, 0))
                .tiled(40, 10)
                .inside(scissor)],
            &atlas,
        );
        let widths: Vec<f32> = quads.iter().map(|quad| quad.rect.width).collect();
        assert_eq!(
            widths,
            [10.0, 10.0, 5.0],
            "four repetitions, cut to two and a half"
        );
    }

    /// Glyphs are cut after the fact, not per label: they are collected into the
    /// font atlas's own pass, and nothing picks a letter.
    #[test]
    fn cutting_quads_is_the_same_box_from_the_other_side() {
        let atlas = atlas_of([(Graphic(1), block(20, 20))]);
        let scissor = Scissor {
            at: GumpPixel::new(0, 0),
            width: 100,
            height: 15,
        };
        let mut quads = collect(
            &[
                Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(0, 0)),
                Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(0, 40)),
            ],
            &atlas,
        );
        scissor.cut(&mut quads);
        assert_eq!(quads.len(), 1, "the second is wholly outside");
        assert_eq!(quads[0].rect.height, 15.0, "and the first is cut to the box");
    }

    /// Painter's order is picking order: the last picture drawn over a point is
    /// the one the click belongs to, which is what makes a hat over a frame the
    /// hat and not the frame.
    #[test]
    fn the_last_picture_drawn_over_a_point_is_the_one_picked() {
        let atlas = atlas_of([(Graphic(1), block(40, 40)), (Graphic(2), block(10, 10))]);
        let window = [
            Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(100, 100)),
            Picture::plain(GumpArt::Gump(Graphic(2)), GumpPixel::new(120, 120)),
        ];
        assert_eq!(
            pick(&window, GumpPixel::new(125, 125), &atlas),
            Some(PictureIndex::new(1))
        );
        assert_eq!(
            pick(&window, GumpPixel::new(105, 105), &atlas),
            Some(PictureIndex::new(0)),
            "clear of the second picture, still inside the first"
        );
    }

    /// The reason a window is picked by its pictures and not by a rectangle: a
    /// click through a frame's transparent middle belongs to whatever is drawn
    /// into it, and a click through a hole with nothing behind it belongs to the
    /// world.
    #[test]
    fn a_transparent_texel_is_not_a_hit() {
        let atlas = atlas_of([(Graphic(1), framed(40, 10..30)), (Graphic(2), block(4, 4))]);
        let frame = Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(0, 0));
        assert_eq!(pick(&[frame], GumpPixel::new(20, 20), &atlas), None);
        assert_eq!(
            pick(&[frame], GumpPixel::new(5, 5), &atlas),
            Some(PictureIndex::new(0))
        );

        let doll = Picture::plain(GumpArt::Gump(Graphic(2)), GumpPixel::new(18, 18));
        assert_eq!(
            pick(&[frame, doll], GumpPixel::new(20, 20), &atlas),
            Some(PictureIndex::new(1)),
            "the picture drawn into the hole is what is there"
        );
    }

    /// A picture the atlas does not hold is not drawn (see `collect`) and so is
    /// not pickable either — the two walks answer about the same window.
    #[test]
    fn a_picture_with_no_art_is_not_pickable() {
        let atlas = atlas_of([(Graphic(1), block(4, 4))]);
        let missing = Picture::plain(GumpArt::Gump(Graphic(2)), GumpPixel::default());
        assert_eq!(pick(&[missing], GumpPixel::new(1, 1), &atlas), None);
    }

    /// A tiled picture is picked over the whole box it fills, not over one
    /// repetition of the art — the box is what was drawn.
    #[test]
    fn a_tiled_picture_is_picked_across_its_whole_box() {
        let atlas = atlas_of([(Graphic(1), block(10, 10))]);
        let strip = [Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(0, 0)).tiled(25, 10)];
        assert_eq!(
            pick(&strip, GumpPixel::new(22, 5), &atlas),
            Some(PictureIndex::new(0))
        );
        assert_eq!(
            pick(&strip, GumpPixel::new(26, 5), &atlas),
            None,
            "past the box, where nothing was drawn"
        );
    }

    /// The one thing a picture drawn once must get right: it lands where it was
    /// asked for, at the art's own size and not the window's.
    #[test]
    fn a_plain_picture_is_one_quad_at_its_own_size() {
        let atlas = atlas_of([(Graphic(1), block(20, 10))]);
        let quads = collect(
            &[Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(30, 40))],
            &atlas,
        );
        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].rect.x, 30.0);
        assert_eq!(quads[0].rect.y, 40.0);
        assert_eq!(quads[0].rect.width, 20.0);
        assert_eq!(quads[0].rect.height, 10.0);
    }

    #[test]
    fn a_translucent_picture_keeps_its_requested_alpha() {
        let atlas = atlas_of([(Graphic(1), block(20, 10))]);
        let quads = collect(
            &[Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(0, 0)).translucent(127)],
            &atlas,
        );
        assert_eq!((quads[0].hue >> 16) & 0xFF, 127);
    }

    /// A gump the client ships no art for is skipped, not drawn as a box and not
    /// an error: a layout naming one is the shard's business and the rest of the
    /// window is still worth having.
    #[test]
    fn a_picture_with_no_art_draws_nothing() {
        let atlas = atlas_of([(Graphic(1), block(4, 4))]);
        assert!(
            collect(
                &[Picture::plain(GumpArt::Gump(Graphic(2)), GumpPixel::default())],
                &atlas
            )
            .is_empty()
        );
    }

    /// Tiling repeats and *clips*, never squeezes: the last repetition along
    /// each axis is a fraction of the art, sampled from the same corner.
    #[test]
    fn a_tiled_picture_clips_its_last_repetition() {
        let atlas = atlas_of([(Graphic(1), block(10, 10))]);
        let quads = collect(
            &[Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(0, 0)).tiled(25, 10)],
            &atlas,
        );
        assert_eq!(quads.len(), 3, "two whole repetitions and a clipped one");
        assert_eq!(quads[2].rect.x, 20.0);
        assert_eq!(quads[2].rect.width, 5.0, "clipped to what is left of the box");
        assert_eq!(
            quads[2].region.du,
            quads[0].region.du / 2.0,
            "half the art's columns, from its own left edge"
        );
        assert_eq!(quads[2].region.u, quads[0].region.u);
    }

    /// The nine pieces are not in reading order. This is the port of
    /// `ResizePic.GetTexture`'s remap, and getting it wrong draws a window with
    /// its middle in the right-hand edge — which looks like a packing bug.
    #[test]
    fn a_resizepic_takes_its_middle_from_plus_four() {
        let atlas = atlas_of((0..9).map(|piece| (Graphic(100 + piece), block(8, 8))));
        let pieces = resize(&atlas, Graphic(100), GumpPixel::new(0, 0), 40, 40);
        assert_eq!(pieces.len(), 9);
        assert_eq!(pieces[0].graphic, GumpArt::Gump(Graphic(100)), "top-left corner");
        assert_eq!(
            pieces[4].graphic,
            GumpArt::Gump(Graphic(105)),
            "the right edge, not the middle"
        );
        assert_eq!(
            pieces[8].graphic,
            GumpArt::Gump(Graphic(104)),
            "the middle, last and behind nothing"
        );
        assert_eq!(
            pieces[7].graphic,
            GumpArt::Gump(Graphic(108)),
            "the bottom-right corner is the last piece, not the ninth graphic"
        );
    }

    /// Page 0 is every page, and a numbered page is only its own. The rule the
    /// wire never states and the one a two-page window is unreadable without.
    #[test]
    fn a_layout_draws_page_zero_and_the_page_showing() {
        let atlas = atlas_of([
            (Graphic(10), block(4, 4)),
            (Graphic(11), block(4, 4)),
            (Graphic(12), block(4, 4)),
        ]);
        let elements = vec![
            Element::Image {
                x: 0,
                y: 0,
                gump: 10,
                hue: None,
            },
            Element::Page(1),
            Element::Image {
                x: 0,
                y: 0,
                gump: 11,
                hue: None,
            },
            Element::Page(2),
            Element::Image {
                x: 0,
                y: 0,
                gump: 12,
                hue: None,
            },
        ];
        let showing = window(&elements, GumpPixel::default(), 2, &BTreeSet::new(), None, &atlas);
        let graphics: Vec<GumpArt> = showing.pictures.iter().map(|picture| picture.graphic).collect();
        assert_eq!(
            graphics,
            vec![GumpArt::Gump(Graphic(10)), GumpArt::Gump(Graphic(12))]
        );
    }

    /// A button held down is drawn in its second picture, and only the one that
    /// is held. Nothing on the wire says this — it is the mouse.
    #[test]
    fn a_held_button_draws_its_pressed_art() {
        let atlas = atlas_of((0..4).map(|piece| (Graphic(20 + piece), block(4, 4))));
        let button = |normal, pressed, id| Element::Button {
            x: 0,
            y: 0,
            normal,
            pressed,
            kind: openshard_protocol::gump::GumpButton::Reply,
            page: 0,
            id: RawButtonId(id),
        };
        let elements = vec![button(20, 21, 1), button(22, 23, 2)];
        let showing = window(
            &elements,
            GumpPixel::default(),
            0,
            &BTreeSet::new(),
            Some(Hit::Reply(RawButtonId(2))),
            &atlas,
        );
        let graphics: Vec<GumpArt> = showing.pictures.iter().map(|picture| picture.graphic).collect();
        assert_eq!(
            graphics,
            vec![GumpArt::Gump(Graphic(20)), GumpArt::Gump(Graphic(23))],
            "the untouched button is up and the held one is down"
        );
    }

    /// A switch is drawn from what the *player* has done to it, not from the
    /// `initial` the layout carried: after the first click the layout is stale
    /// and the server is not told until the reply.
    #[test]
    fn a_switch_is_drawn_from_the_set_and_not_from_its_initial() {
        let atlas = atlas_of((0..2).map(|piece| (Graphic(30 + piece), block(4, 4))));
        let elements = vec![Element::Check(Switch {
            x: 0,
            y: 0,
            off: 30,
            on: 31,
            initial: false,
            id: RawSwitchId(7),
        })];
        let mut set = BTreeSet::new();
        set.insert(RawSwitchId(7));
        let showing = window(&elements, GumpPixel::default(), 0, &set, None, &atlas);
        assert_eq!(
            showing.pictures[0].graphic,
            GumpArt::Gump(Graphic(31)),
            "on, despite initial: false"
        );
    }

    /// A background covers the rectangle it was asked for: the corners sit in
    /// the corners, which is the assertion that fails if an offset is dropped.
    #[test]
    fn a_resizepic_puts_its_corners_in_the_corners() {
        let atlas = atlas_of((0..9).map(|piece| (Graphic(100 + piece), block(8, 8))));
        let pieces = resize(&atlas, Graphic(100), GumpPixel::new(5, 5), 40, 30);
        assert_eq!(pieces[0].at, GumpPixel::new(5, 5));
        assert_eq!(pieces[2].at, GumpPixel::new(5 + 40 - 8, 5), "top-right");
        assert_eq!(pieces[5].at, GumpPixel::new(5, 5 + 30 - 8), "bottom-left");
        assert_eq!(
            pieces[7].at,
            GumpPixel::new(5 + 40 - 8, 5 + 30 - 8),
            "bottom-right"
        );
    }

    /// The reason [`GumpArt`] exists at all. The two index spaces overlap, so an
    /// atlas keyed by a bare `Graphic` would answer a container's background
    /// with an item's icon — a window drawn with the wrong picture, and no error
    /// anywhere to notice it by.
    #[test]
    fn a_gump_and_an_item_of_the_same_number_are_two_pictures() {
        let atlas = GumpAtlas::pack([
            (GumpArt::Gump(Graphic(0x003C)), block(20, 10)),
            (GumpArt::Item(Graphic(0x003C)), block(7, 5)),
        ])
        .expect("two small blocks fit an atlas 2048 on a side");

        let background = atlas.sprite(GumpArt::Gump(Graphic(0x003C))).expect("packed");
        let icon = atlas.sprite(GumpArt::Item(Graphic(0x003C))).expect("packed");
        assert_eq!((background.width, background.height), (20, 10));
        assert_eq!((icon.width, icon.height), (7, 5));
        assert_ne!(
            (background.region.u, background.region.v),
            (icon.region.u, icon.region.v),
            "two pictures under one number must not share a region"
        );
    }

    /// A picture the atlas has never been offered has no sprite, whichever file
    /// it would have come from — the same answer, not a panic and not the other
    /// namespace's picture.
    #[test]
    fn art_that_was_never_offered_has_no_sprite() {
        let atlas = GumpAtlas::pack([(GumpArt::Gump(Graphic(1)), block(4, 4))])
            .expect("one small block fits an atlas 2048 on a side");
        assert!(atlas.sprite(GumpArt::Item(Graphic(1))).is_none());
        assert!(atlas.sprite(GumpArt::Gump(Graphic(2))).is_none());
    }

    /// A window's hit table names the pictures a click means something on, and
    /// the two kinds of button are two different meanings: one leaves on the
    /// wire and one never does. A background's nine pieces are in neither.
    #[test]
    fn a_button_says_whether_its_click_reaches_the_server() {
        let atlas = atlas_of((0..9).map(|piece| (Graphic(100 + piece), block(8, 8))));
        let button = |normal, kind, page, id| Element::Button {
            x: 0,
            y: 0,
            normal,
            pressed: normal + 1,
            kind,
            page,
            id: RawButtonId(id),
        };
        let elements = vec![
            Element::Background {
                x: 0,
                y: 0,
                width: 40,
                height: 40,
                gump: 100,
            },
            button(100, openshard_protocol::gump::GumpButton::Reply, 0, 13),
            button(102, openshard_protocol::gump::GumpButton::Page, 2, 0),
        ];
        let showing = window(&elements, GumpPixel::default(), 0, &BTreeSet::new(), None, &atlas);
        assert_eq!(
            showing.pictures.len(),
            11,
            "a background's nine pieces and the two buttons"
        );
        assert_eq!(
            showing.hits.get(&PictureIndex::new(9)),
            Some(&Hit::Reply(RawButtonId(13)))
        );
        assert_eq!(showing.hits.get(&PictureIndex::new(10)), Some(&Hit::Page(2)));
        assert_eq!(
            showing.hits.len(),
            2,
            "no piece of the background answers a click"
        );
    }

    /// A checkbox and a radio are drawn the same way and answered differently:
    /// one turns its neighbours off and the other does not, and the hit is where
    /// that difference is carried.
    #[test]
    fn a_switch_says_which_kind_of_switch_it_is() {
        let atlas = atlas_of((0..2).map(|piece| (Graphic(30 + piece), block(4, 4))));
        let switch = |id| Switch {
            x: 0,
            y: 0,
            off: 30,
            on: 31,
            initial: false,
            id: RawSwitchId(id),
        };
        let elements = vec![Element::Check(switch(1)), Element::Radio(switch(2))];
        let showing = window(&elements, GumpPixel::default(), 0, &BTreeSet::new(), None, &atlas);
        assert_eq!(
            showing.hits.get(&PictureIndex::new(0)),
            Some(&Hit::Check(RawSwitchId(1)))
        );
        assert_eq!(
            showing.hits.get(&PictureIndex::new(1)),
            Some(&Hit::Radio(RawSwitchId(2)))
        );
    }

    /// A layout's text hue is one *less* than the wire hue it means, and reading
    /// it as a wire hue draws every dialog in the colour of the row above the
    /// one the shard asked for. See [`text_hue`].
    #[test]
    fn a_caption_takes_the_hue_the_layout_asked_for_plus_one() {
        let atlas = atlas_of([]);
        let elements = vec![Element::Label {
            x: 3,
            y: 4,
            hue: 0x0480,
            line: 0,
        }];
        let showing = window(
            &elements,
            GumpPixel::new(10, 20),
            0,
            &BTreeSet::new(),
            None,
            &atlas,
        );
        assert_eq!(showing.captions[0].hue, Hue(0x0481));
        assert_eq!(showing.captions[0].at, GumpPixel::new(13, 24));
    }

    /// `{ tilepic }` is the one element whose picture comes out of the world's
    /// art, and it had been dropped on the floor: the layout parsed, and nothing
    /// drew it.
    #[test]
    fn a_tilepic_asks_for_an_item_and_not_a_gump() {
        let elements = [Element::Item {
            x: 44,
            y: 65,
            graphic: 0x0A28,
            hue: Some(0x21),
        }];
        assert_eq!(
            art_of(&elements).into_iter().collect::<Vec<_>>(),
            vec![GumpArt::Item(Graphic(0x0A28))]
        );

        let atlas = atlas_of([]);
        let showing = window(
            &elements,
            GumpPixel::new(10, 20),
            0,
            &BTreeSet::new(),
            None,
            &atlas,
        );
        assert_eq!(showing.pictures.len(), 1);
        assert_eq!(showing.pictures[0].graphic, GumpArt::Item(Graphic(0x0A28)));
        assert_eq!(showing.pictures[0].at, GumpPixel::new(54, 85));
        assert_eq!(showing.pictures[0].hue, Hue(0x21));
    }
}
