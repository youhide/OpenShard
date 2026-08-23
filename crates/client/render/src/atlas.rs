//! Land sprites, packed into one texture. Twice: once for the flat art and once
//! for the textures a slope is stretched over.
//!
//! A draw call can bind one texture, and a screen of ground touches a few
//! hundred different graphics, so they go into a grid: every land tile is
//! exactly 44x44, which makes the packing a division rather than a bin-packing
//! problem, and makes a slot's position something a test can state outright.
//!
//! [`TexmapAtlas`] is the same idea one step less regular, because a texture map
//! is 64 or 128 on a side. Both atlases are keyed by the *land graphic* even
//! though the texture is looked up through `tiledata` by a different id, so a
//! quad asks both of them the same question.
//!
//! The atlases are built for a set of graphics, not for the whole file. A modern
//! client ships about 4,244 land tiles and the container is 155MB; what is on
//! screen is a fraction of that, and the browser is the reason the difference
//! matters rather than an optimisation nobody asked for.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use openshard_protocol::feedback::AnimationFrameCount;
use openshard_protocol::speech::Font;
use openshard_protocol::wire::Graphic;
use openshard_tiles::TileData;
use openshard_uofiles::anim::{Anim, AnimError, AnimFrame, AnimationFrameIndex};
use openshard_uofiles::art::{Art, ArtError, LAND_TILE_SIZE, land_row};
use openshard_uofiles::color::Rgb8;
use openshard_uofiles::font::{AsciiFonts, FONT_COUNT};
use openshard_uofiles::image::Image;
use openshard_uofiles::texmaps::{TexMapError, TexMaps};
use openshard_uofiles::ttf_font::{TtfFont, TtfGlyph};

/// The atlas texture's side, in pixels.
///
/// Not larger, and not negotiable: WebGL2 only guarantees `MAX_TEXTURE_SIZE` of
/// 2048, so a 4096 atlas would work on this machine and fail on a phone. At 44
/// pixels a tile that is 46 columns of 46 rows, or 2,116 slots.
const ATLAS_SIDE: u32 = 2048;

/// Slots per row, and per column.
const SLOTS_PER_ROW: u32 = ATLAS_SIDE / LAND_TILE_SIZE as u32;

/// How many graphics one atlas can hold.
pub const CAPACITY: usize = (SLOTS_PER_ROW * SLOTS_PER_ROW) as usize;

/// What can go wrong building one.
#[derive(Debug)]
pub enum AtlasError {
    /// More pictures than the atlas holds.
    ///
    /// Not a "grow the texture" case: the cap is the web's, so the fix is to
    /// build an atlas per region, or to evict. Failing loudly is the point.
    Full {
        /// How many were asked for.
        wanted: usize,
        /// How many would have fitted, at best.
        capacity: usize,
    },
    /// More immutable static-atlas pages were needed than the client permits.
    ///
    /// This is deliberately distinct from [`Full`](Self::Full): a page filling
    /// is normal for [`StaticAtlasPages`], while reaching this limit is the
    /// memory policy that keeps an unbounded walk from retaining every static
    /// picture it has ever passed.
    PageLimit {
        /// Pages needed to hold the requested pictures.
        wanted: usize,
        /// Pages this atlas family is allowed to retain.
        limit: usize,
    },
    /// A sprite is bigger than the whole atlas, so no packing could hold it.
    ///
    /// Separate from [`Full`](Self::Full) because it is not a capacity problem:
    /// the tallest static a client ships is around 250 pixels, so a sprite over
    /// 2048 means the art was decoded wrongly rather than that too much was
    /// asked for.
    Oversized {
        /// Which graphic.
        graphic: Graphic,
        /// How wide it claims to be.
        width: u16,
        /// How tall.
        height: u16,
    },
    /// A rasterized TrueType glyph is bigger than the whole atlas.
    ///
    /// [`Oversized`](Self::Oversized) is keyed by a wire `Graphic`, which a
    /// Unicode code point is not — a face rasterized at a sane pixel height
    /// never approaches 2048 pixels, so this means the caller asked for an
    /// implausible size rather than that a real character is this shape.
    OversizedGlyph {
        /// The character.
        char: char,
        /// How wide it rasterized to.
        width: u16,
        /// How tall.
        height: u16,
    },
    /// The art container refused a graphic.
    Art(ArtError),
    /// The animation files refused a body.
    Anim(AnimError),
    /// The texture maps refused a texture.
    TexMaps(TexMapError),
}

impl fmt::Display for AtlasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full { wanted, capacity } => {
                write!(f, "{wanted} pictures do not fit in an atlas of {capacity}")
            }
            Self::PageLimit { wanted, limit } => {
                write!(f, "{wanted} static atlas pages exceed the limit of {limit}")
            }
            Self::Oversized {
                graphic,
                width,
                height,
            } => write!(
                f,
                "{graphic:?} is {width}x{height}, which does not fit an atlas {ATLAS_SIDE} on a side",
            ),
            Self::OversizedGlyph { char, width, height } => write!(
                f,
                "{char:?} rasterized to {width}x{height}, which does not fit an atlas {ATLAS_SIDE} on a side",
            ),
            Self::Art(source) => write!(f, "reading land art: {source}"),
            Self::Anim(source) => write!(f, "reading an animation: {source}"),
            Self::TexMaps(source) => write!(f, "reading a land texture: {source}"),
        }
    }
}

impl std::error::Error for AtlasError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Full { .. }
            | Self::PageLimit { .. }
            | Self::Oversized { .. }
            | Self::OversizedGlyph { .. } => None,
            Self::Art(source) => Some(source),
            Self::Anim(source) => Some(source),
            Self::TexMaps(source) => Some(source),
        }
    }
}

impl From<ArtError> for AtlasError {
    fn from(source: ArtError) -> Self {
        Self::Art(source)
    }
}

impl From<AnimError> for AtlasError {
    fn from(source: AnimError) -> Self {
        Self::Anim(source)
    }
}

impl From<TexMapError> for AtlasError {
    fn from(source: TexMapError) -> Self {
        Self::TexMaps(source)
    }
}

/// Which rows of an atlas have been written since a renderer last read it.
///
/// Every atlas here grows rather than being rebuilt — see [`LandAtlas::add`] —
/// and a growth writes a few sprites into a 16MB texture. Re-uploading the whole
/// thing for that is the cost this exists to avoid, and rows are the unit
/// because `write_texture` wants a contiguous slice of the source: a band is one
/// call over one sub-slice of `pixels`, where a set of scattered rectangles
/// would be one call each.
///
/// Kept as a bounding band rather than a list, which over-covers when two
/// distant rows are touched in one growth and is exactly right for the ordinary
/// case: every allocator here fills downwards, so a growth's rows are adjacent.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Dirty {
    /// `[top, bottom)`, and `None` when nothing has been written.
    band: Option<(u32, u32)>,
}

impl Dirty {
    /// Record that `height` rows starting at `top` were written.
    fn mark(&mut self, top: u32, height: u32) {
        // A zero-height write touches nothing, and folding it in would widen the
        // band to a row nobody wrote.
        if height == 0 {
            return;
        }
        let (top, bottom) = (top, top + height);
        self.band = Some(match self.band {
            Some((was_top, was_bottom)) => (was_top.min(top), was_bottom.max(bottom)),
            None => (top, bottom),
        });
    }

    /// The band, cleared. `None` when nothing has changed.
    fn take(&mut self) -> Option<std::ops::Range<u32>> {
        self.band.take().map(|(top, bottom)| top..bottom)
    }
}

/// Where in the atlas a graphic's sprite sits, in texture coordinates.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Region {
    /// Left edge, 0..1.
    pub u: f32,
    /// Top edge, 0..1.
    pub v: f32,
    /// Width, 0..1.
    pub du: f32,
    /// Height, 0..1.
    pub dv: f32,
}

/// Land art, packed and ready to upload.
///
/// Holds its pixels rather than a GPU handle: this crate does not decide when a
/// texture is created, and a test wants to read the pixels without a device.
pub struct LandAtlas {
    /// Where each graphic sits, indexed by the graphic itself.
    ///
    /// **Dense, and that is a frame-rate decision rather than a style one.**
    /// [`Self::region`] is asked once per visible land tile — 26,732 of them at
    /// the widest zoom — and a `BTreeMap` answers each of those from a node the
    /// cache has long since evicted, in a loop that is already missing on the
    /// map itself. One indexed load costs a miss at worst and none at all for
    /// the handful of graphics a street repeats. `Graphic` is a `u16`, so the
    /// table is `u16::MAX + 1` entries — 1.3 MB beside the 16 MB of pixels this
    /// same atlas already holds.
    ///
    /// The [`Region`] is stored rather than the slot, because deriving it costs
    /// four divisions that do not change once a graphic is packed.
    regions: Box<[Option<Region>]>,
    /// How many slots are spoken for — the next free one, and the count
    /// `regions` cannot give without a scan.
    packed: u32,
    /// Every graphic ever offered to this atlas, whether or not it packed.
    ///
    /// Not the same set as `slots`, and the difference is the point: three
    /// quarters of the land index ships no art, so a graphic that is genuinely
    /// absent would otherwise be looked up in a 155MB container on every frame
    /// it is on screen — and would answer "not packed" for ever, which is a
    /// question that never stops being asked.
    asked: BTreeSet<Graphic>,
    /// `ATLAS_SIDE * ATLAS_SIDE` RGBA8 pixels, row-major.
    pixels: Vec<u8>,
    dirty: Dirty,
}

impl fmt::Debug for LandAtlas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LandAtlas")
            .field("graphics", &self.packed)
            .field("side", &ATLAS_SIDE)
            .finish()
    }
}

impl LandAtlas {
    /// Pack every graphic in `wanted` that the client actually ships.
    ///
    /// A graphic the client has no art for is skipped, not an error: three
    /// quarters of the land index is genuinely empty, and a map referring to an
    /// empty slot is the file's business, not a failure to draw.
    pub fn build(art: &Art, wanted: impl IntoIterator<Item = Graphic>) -> Result<Self, AtlasError> {
        let mut atlas = Self::empty();
        atlas.add(art, wanted)?;
        // Handed to a renderer whole, so nothing is outstanding: see
        // [`LandAtlas::take_dirty`].
        atlas.dirty.take();
        Ok(atlas)
    }

    /// How many graphics in `wanted` this atlas has not been asked for before.
    pub fn newly_requested(&self, wanted: impl IntoIterator<Item = Graphic>) -> usize {
        wanted
            .into_iter()
            .filter(|graphic| !self.asked.contains(graphic))
            .count()
    }

    /// An atlas holding nothing, ready to be grown into.
    fn empty() -> Self {
        let side = ATLAS_SIDE as usize;
        Self {
            regions: vec![None; GRAPHIC_SLOTS].into_boxed_slice(),
            packed: 0,
            asked: BTreeSet::new(),
            pixels: vec![0u8; side * side * 4],
            dirty: Dirty::default(),
        }
    }

    /// Pack whichever of `wanted` this atlas has not been offered before.
    ///
    /// The alternative to rebuilding, and the reason the atlases are not thrown
    /// away every time the camera walks a tile: one new graphic at the edge of
    /// the view used to re-read and re-pack every atlas and recreate every
    /// pipeline behind them, which is a hitch every few tiles during a scroll —
    /// and a scroll is exactly the thing that keeps introducing graphics.
    ///
    /// A graphic already offered is skipped without touching the art container,
    /// packed or not. What is written is recorded in the dirty band, so the
    /// upload that follows is the rows that changed rather than 16MB.
    ///
    /// On [`AtlasError::Full`] the atlas keeps whatever fitted and is no longer
    /// consistent with what its caller asked for: the caller is expected to
    /// throw it away and build one for what is on screen now, which is what
    /// makes "full" recoverable rather than terminal.
    pub fn add(&mut self, art: &Art, wanted: impl IntoIterator<Item = Graphic>) -> Result<(), AtlasError> {
        // Sorted and deduplicated, so the same input always produces the same
        // atlas — a frame that changes because a `HashSet` iterated differently
        // is not a frame a test can assert on.
        let wanted: BTreeSet<Graphic> = wanted.into_iter().collect();
        let fresh: Vec<Graphic> = wanted
            .into_iter()
            .filter(|graphic| !self.asked.contains(graphic))
            .collect();
        if fresh.is_empty() {
            return Ok(());
        }
        let mut images = Vec::with_capacity(fresh.len());
        for graphic in &fresh {
            if let Some(image) = art.land(*graphic)? {
                images.push((*graphic, image));
            }
        }
        self.insert(images)?;
        self.asked.extend(fresh);
        Ok(())
    }

    /// The rows written since this was last asked, cleared.
    ///
    /// `None` when nothing has changed, which is the ordinary frame: a camera
    /// standing still introduces no graphics. Pairs with
    /// [`GroundRenderer::upload_changes`](crate::renderer::GroundRenderer::upload_changes),
    /// and a freshly built atlas reports nothing because whoever binds one
    /// uploads it whole.
    pub fn take_dirty(&mut self) -> Option<std::ops::Range<u32>> {
        self.dirty.take()
    }

    /// Pack sprites somebody else decoded.
    ///
    /// What [`LandAtlas::build`] does once it has read the art, and the only way
    /// in that does not need a client install: a test can hand this a picture it
    /// chose and then assert on the pixels the frame comes back with. Every
    /// sprite is expected to be [`LAND_TILE_SIZE`] square, and only the diamond
    /// inside it is copied.
    pub fn pack(images: impl IntoIterator<Item = (Graphic, Image)>) -> Result<Self, AtlasError> {
        let mut atlas = Self::empty();
        let images: Vec<(Graphic, Image)> = images.into_iter().collect();
        atlas.asked.extend(images.iter().map(|(graphic, _)| *graphic));
        atlas.insert(images)?;
        atlas.dirty.take();
        Ok(atlas)
    }

    /// Pack more sprites somebody else decoded into an atlas that already holds
    /// some.
    ///
    /// [`pack`](Self::pack) grown rather than rebuilt, and the way in that needs
    /// no client install: a test can assert that an atlas built in two steps is
    /// the atlas built in one, which is the property the frame depends on and
    /// the one a shelf or a grid can quietly lose.
    pub fn pack_more(
        &mut self,
        images: impl IntoIterator<Item = (Graphic, Image)>,
    ) -> Result<(), AtlasError> {
        let images: Vec<(Graphic, Image)> = images.into_iter().collect();
        self.asked.extend(images.iter().map(|(graphic, _)| *graphic));
        self.insert(images)
    }

    /// Write pictures into the free slots, marking the rows they land in.
    ///
    /// A graphic already packed is skipped: the caller filtered by what was
    /// *asked*, which is the larger set, and packing one twice would spend a
    /// slot and leave the older region pointing at pixels nothing samples.
    fn insert(&mut self, images: impl IntoIterator<Item = (Graphic, Image)>) -> Result<(), AtlasError> {
        let images: BTreeMap<Graphic, Image> = images.into_iter().collect();
        if self.packed as usize + images.len() > CAPACITY {
            return Err(AtlasError::Full {
                wanted: self.packed as usize + images.len(),
                capacity: CAPACITY,
            });
        }

        let side = ATLAS_SIDE as usize;

        for (graphic, image) in images {
            // The grid is the format's constant, so a sprite of another size is
            // a caller's mistake rather than a file's. Said here because the
            // copy below indexes by `land_row`, which only stays inside a 44
            // square.
            assert_eq!(
                (image.width(), image.height()),
                (LAND_TILE_SIZE, LAND_TILE_SIZE),
                "a land sprite is always {LAND_TILE_SIZE} square",
            );
            if self.regions[graphic.0 as usize].is_some() {
                continue;
            }
            let slot = self.packed;
            let (origin_x, origin_y) = slot_origin(slot);
            self.dirty.mark(origin_y, u32::from(LAND_TILE_SIZE));

            for y in 0..image.height() {
                // The diamond, not the colours, is what says which pixels exist.
                // Ground has no transparency: a zero pixel inside the diamond is
                // black, and real tiles contain a few. Reading the shape out of
                // the colours instead punches pinholes through the ground that
                // look like dark texture until something counts them.
                for x in land_row(y) {
                    // `pixel` is `None` only outside the image, and `land_row`
                    // stays inside it.
                    let color = image.pixel(x, y).unwrap();
                    let at =
                        ((origin_y + u32::from(y)) as usize * side + (origin_x + u32::from(x)) as usize) * 4;
                    let Rgb8 { red, green, blue } = color.rgb8();
                    self.pixels[at] = red;
                    self.pixels[at + 1] = green;
                    self.pixels[at + 2] = blue;
                    self.pixels[at + 3] = u8::MAX;
                }
            }
            self.regions[graphic.0 as usize] = Some(region_of_slot(slot));
            self.packed += 1;
        }

        Ok(())
    }

    /// The atlas texture's side in pixels. Square.
    pub const fn side() -> u32 {
        ATLAS_SIDE
    }

    /// Its pixels, RGBA8 and row-major, ready for `write_texture`.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// How many graphics landed in it.
    pub fn len(&self) -> usize {
        self.packed as usize
    }

    /// Whether nothing did.
    pub fn is_empty(&self) -> bool {
        self.packed == 0
    }

    /// Where a graphic sits, or `None` if the client ships no art for it.
    ///
    /// One indexed load, deliberately — see [`Self::regions`].
    pub fn region(&self, graphic: Graphic) -> Option<Region> {
        self.regions[graphic.0 as usize]
    }
}

/// One entry per [`Graphic`], which is a `u16`.
///
/// The whole index rather than the land range, so no caller has to know where
/// that range ends: the table is a rounding error beside an atlas's pixels, and
/// a bound nobody can be wrong about is worth more than the bytes.
const GRAPHIC_SLOTS: usize = u16::MAX as usize + 1;

/// The top-left pixel of a slot. Row-major, which is why slot 0 is the origin.
fn slot_origin(slot: u32) -> (u32, u32) {
    let tile = LAND_TILE_SIZE as u32;
    ((slot % SLOTS_PER_ROW) * tile, (slot / SLOTS_PER_ROW) * tile)
}

/// The normalised rectangle a land slot occupies.
///
/// Computed once, when the graphic is packed, rather than on every frame that
/// draws the tile: a slot never moves, so these four divisions have exactly one
/// answer per graphic for the life of the atlas.
fn region_of_slot(slot: u32) -> Region {
    let (x, y) = slot_origin(slot);
    let side = ATLAS_SIDE as f32;
    let tile = LAND_TILE_SIZE as f32;
    Region {
        u: x as f32 / side,
        v: y as f32 / side,
        du: tile / side,
        dv: tile / side,
    }
}

/// The texture-map atlas's grid is this many pixels on a side.
///
/// The smaller of the two sizes `texmaps.mul` holds. A 128 texture takes a 2x2
/// block of cells, which is what makes this a grid at all rather than a
/// bin-packing problem: every texture is a whole number of cells.
const TEXMAP_CELL: u32 = 64;

/// Cells across the texture atlas, and down.
const TEXMAP_CELLS_PER_ROW: u32 = ATLAS_SIDE / TEXMAP_CELL;

/// Cells one texture atlas holds. A 64 texture takes one and a 128 takes four,
/// so the number of *textures* that fit depends on what they are.
pub const TEXMAP_CELLS: usize = (TEXMAP_CELLS_PER_ROW * TEXMAP_CELLS_PER_ROW) as usize;

/// The square textures a sloped tile is stretched over, packed into one texture.
///
/// Keyed by the *land graphic*, not by the texture id: the id is `tiledata`'s
/// business and resolving it here means a quad asks this and [`LandAtlas`] the
/// same question. Two graphics sharing a texture id therefore hold two copies of
/// it, which costs a cell each and keeps the lookup one map deep.
pub struct TexmapAtlas {
    /// Where each graphic's texture sits, indexed by the graphic.
    ///
    /// Dense for the same reason [`LandAtlas::regions`] is, and asked in the
    /// same loop: a ground quad reads both on every visible tile.
    regions: Box<[Option<Region>]>,
    /// How many of `regions` are filled — the count a scan would otherwise
    /// have to find.
    packed: u32,
    /// Every land graphic ever offered, whether or not it had a texture.
    ///
    /// The ordinary case is that it did not — the client ships 4,116 textures
    /// for 16,384 slots — so without this every flat tile on screen would send
    /// `tiledata` and `texmaps.mul` the same question on every frame, for ever.
    asked: BTreeSet<Graphic>,
    /// Which cells are spoken for, kept between growths: an atlas that forgot
    /// this would hand the next texture a cell it had already filled.
    grid: CellGrid,
    /// `ATLAS_SIDE * ATLAS_SIDE` RGBA8 pixels, row-major.
    pixels: Vec<u8>,
    dirty: Dirty,
}

impl fmt::Debug for TexmapAtlas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TexmapAtlas")
            .field("graphics", &self.packed)
            .field("side", &ATLAS_SIDE)
            .finish()
    }
}

impl TexmapAtlas {
    /// Pack the texture of every graphic in `wanted` that has one.
    ///
    /// A land graphic with no texture is skipped rather than refused, and it is
    /// the ordinary case: the client ships 4,116 textures for 16,384 slots, and
    /// a tile without one is drawn from its flat art however the ground stands.
    pub fn build(
        texmaps: &TexMaps,
        tiledata: &TileData,
        wanted: impl IntoIterator<Item = Graphic>,
    ) -> Result<Self, AtlasError> {
        let mut atlas = Self::empty();
        atlas.add(texmaps, tiledata, wanted)?;
        atlas.dirty.take();
        Ok(atlas)
    }

    /// How many land-graphic requests have not reached this atlas before.
    pub fn newly_requested(&self, wanted: impl IntoIterator<Item = Graphic>) -> usize {
        wanted
            .into_iter()
            .filter(|graphic| !self.asked.contains(graphic))
            .count()
    }

    /// An atlas holding nothing, ready to be grown into.
    fn empty() -> Self {
        let side = ATLAS_SIDE as usize;
        Self {
            regions: vec![None; GRAPHIC_SLOTS].into_boxed_slice(),
            packed: 0,
            asked: BTreeSet::new(),
            grid: CellGrid::new(),
            pixels: vec![0u8; side * side * 4],
            dirty: Dirty::default(),
        }
    }

    /// Pack the texture of whichever of `wanted` this atlas has not been
    /// offered before. The land atlas's [`add`](LandAtlas::add), for the other
    /// half of a ground quad — and a quad asks the two the same question, so
    /// they are grown from the same set on the same frame or a slope is drawn
    /// from art the tile next to it was textured with.
    pub fn add(
        &mut self,
        texmaps: &TexMaps,
        tiledata: &TileData,
        wanted: impl IntoIterator<Item = Graphic>,
    ) -> Result<(), AtlasError> {
        // Sorted and deduplicated for the same reason as the land atlas: the
        // same input has to produce the same atlas, byte for byte.
        let wanted: BTreeSet<Graphic> = wanted.into_iter().collect();
        let fresh: Vec<Graphic> = wanted
            .into_iter()
            .filter(|graphic| !self.asked.contains(graphic))
            .collect();
        if fresh.is_empty() {
            return Ok(());
        }
        let mut images = Vec::new();
        for graphic in &fresh {
            // The indirection this whole atlas exists to follow: a land graphic
            // names a `tiledata` entry, and that entry names a texture.
            let id = tiledata.land(graphic.0).texture;
            if let Some(image) = texmaps.texture(id)? {
                images.push((*graphic, image));
            }
        }
        self.insert(images)?;
        self.asked.extend(fresh);
        Ok(())
    }

    /// The rows written since this was last asked, cleared. See
    /// [`LandAtlas::take_dirty`].
    pub fn take_dirty(&mut self) -> Option<std::ops::Range<u32>> {
        self.dirty.take()
    }

    /// Pack textures somebody else decoded, largest first.
    ///
    /// Largest first is what keeps the grid simple: a 128 needs a free 2x2 block
    /// and the 64s would otherwise have scattered themselves through every one
    /// of them. Deterministic given the same input, which the frame tests rely
    /// on.
    pub fn pack(images: impl IntoIterator<Item = (Graphic, Image)>) -> Result<Self, AtlasError> {
        let mut atlas = Self::empty();
        atlas.pack_more(images)?;
        atlas.dirty.take();
        Ok(atlas)
    }

    /// Pack more textures into an atlas that already holds some. The texture
    /// half of [`LandAtlas::pack_more`], and it is grown in the same breath:
    /// a ground quad samples both.
    pub fn pack_more(
        &mut self,
        images: impl IntoIterator<Item = (Graphic, Image)>,
    ) -> Result<(), AtlasError> {
        let images: Vec<(Graphic, Image)> = images.into_iter().collect();
        self.asked.extend(images.iter().map(|(graphic, _)| *graphic));
        self.insert(images)
    }

    /// Write textures into the free cells, marking the rows they land in.
    ///
    /// Largest first *within one growth*, which is all a first-fit grid needs to
    /// stay correct: a later 128 that finds no free 2x2 block among the cells an
    /// earlier growth's 64s left is a few wasted cells, not a wrong picture. A
    /// single [`pack`](Self::pack) therefore still lays the atlas out exactly as
    /// it always did, byte for byte, which is what the frame tests assert on.
    fn insert(&mut self, images: impl IntoIterator<Item = (Graphic, Image)>) -> Result<(), AtlasError> {
        let images: BTreeMap<Graphic, Image> = images.into_iter().collect();
        let mut order: Vec<(Graphic, Image)> = images.into_iter().collect();
        // Ties broken by graphic, which `BTreeMap` already ordered them by, and
        // `sort_by_key` is stable — so this is a total order and not a
        // "whichever the sort happened to visit first".
        order.sort_by_key(|(_, image)| std::cmp::Reverse(image.width()));

        let side = ATLAS_SIDE as usize;

        let wanted = self.packed as usize + order.len();
        for (graphic, image) in order {
            if self.regions[graphic.0 as usize].is_some() {
                continue;
            }
            // Square, and a whole number of cells: both are the format's, and
            // `texmaps` has already refused anything else.
            assert_eq!(image.width(), image.height(), "a texture map is square");
            let span = image.width() as u32 / TEXMAP_CELL;
            assert!(
                span >= 1 && u32::from(image.width()) % TEXMAP_CELL == 0,
                "a {}-pixel texture is not a whole number of {TEXMAP_CELL}-pixel cells",
                image.width(),
            );
            let Some((cell_x, cell_y)) = self.grid.take(span) else {
                return Err(AtlasError::Full {
                    wanted,
                    capacity: TEXMAP_CELLS,
                });
            };
            let (origin_x, origin_y) = (cell_x * TEXMAP_CELL, cell_y * TEXMAP_CELL);
            self.dirty.mark(origin_y, u32::from(image.height()));

            // The whole square, corner to corner. A texture has no transparency
            // and no shape to recover: unlike a land sprite, every pixel of it
            // is drawn, zero words included.
            for y in 0..image.height() {
                for x in 0..image.width() {
                    let color = image.pixel(x, y).expect("inside the image");
                    let at =
                        ((origin_y + u32::from(y)) as usize * side + (origin_x + u32::from(x)) as usize) * 4;
                    let Rgb8 { red, green, blue } = color.rgb8();
                    self.pixels[at] = red;
                    self.pixels[at + 1] = green;
                    self.pixels[at + 2] = blue;
                    self.pixels[at + 3] = u8::MAX;
                }
            }

            // Half a texel in on every side, which is ClassicUO's
            // `CalculateHalfPixelUVs` and is not a nicety. A sloped quad's corner
            // texture coordinates are the region's own edges, and an edge is the
            // boundary *between* two texels: at `u + du` the sample lands on the
            // first texel of whatever was packed next door. Inset, the four
            // corners sample texel centres — 0.5 and side-0.5 — so the picture is
            // its own and nothing bleeds along the two far edges of every tile.
            let atlas = ATLAS_SIDE as f32;
            let half = 0.5 / atlas;
            self.regions[graphic.0 as usize] = Some(Region {
                u: origin_x as f32 / atlas + half,
                v: origin_y as f32 / atlas + half,
                du: f32::from(image.width()) / atlas - 2.0 * half,
                dv: f32::from(image.height()) / atlas - 2.0 * half,
            });
            self.packed += 1;
        }

        Ok(())
    }

    /// The atlas texture's side in pixels. Square, and the same as the land
    /// atlas's — one constant, one ceiling.
    pub const fn side() -> u32 {
        ATLAS_SIDE
    }

    /// Its pixels, RGBA8 and row-major, ready for `write_texture`.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// How many graphics landed in it.
    pub fn len(&self) -> usize {
        self.packed as usize
    }

    /// Whether nothing did.
    pub fn is_empty(&self) -> bool {
        self.packed == 0
    }

    /// Where to sample a graphic's texture, or `None` if it has none.
    ///
    /// Where to *sample*, not where it sits: the region is half a texel inside
    /// the texture on every side, because a quad's corners sample the region's
    /// edges and an edge belongs to two texels. See [`TexmapAtlas::pack`].
    ///
    /// `None` is the common answer and means "draw this tile from its art",
    /// which is what the client does with a tile whose texture is missing — see
    /// `ground.wgsl`.
    /// One indexed load, deliberately — see [`Self::regions`].
    pub fn region(&self, graphic: Graphic) -> Option<Region> {
        self.regions[graphic.0 as usize]
    }
}

/// One packed static sprite: where it is, and how big it is.
///
/// The size travels with the region because a static's quad *is* its sprite —
/// unlike ground, whose quad is 44 square whatever the art holds — so whoever
/// places the quad needs the pixels, and reading them back out of a normalised
/// region is a multiplication that can disagree with the one that produced it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Sprite {
    /// Where to sample it.
    pub region: Region,
    /// Its width in pixels.
    pub width: u16,
    /// Its height in pixels.
    pub height: u16,
    /// Which edges of its tile this picture stands on, if the art says.
    ///
    /// Only [`StaticAtlas`] ever measures it. A body's animation frame and a
    /// letter of a font share this type and are `None` by construction: a mobile
    /// stands in the middle of its tile and turns, and a glyph is not a thing
    /// standing in the street.
    ///
    /// Measured once, here, while the sprite is packed — the answer is a
    /// property of the picture and a city repeats its pictures thousands of
    /// times, so a per-quad measurement would be the same walk over the same
    /// pixels a few thousand times a frame. See [`crate::facing`], and
    /// [`crate::place::Stance::of`] for what is done with it: `None` is a post,
    /// a tree, or a wall the detector would not guess at, and every one of those
    /// keeps the behaviour it had before faces existed. A **corner** is two
    /// faces and is one of the answers — see [`crate::facing::Facing`].
    ///
    /// It rides on [`Sprite`] rather than in a table beside the atlas for the
    /// reason the size does: whoever places the quad has this in hand already,
    /// and a second lookup keyed by graphic is a second chance to answer about a
    /// picture that is not the one being drawn — an animated static shows a
    /// different graphic every few frames.
    pub facing: Option<crate::facing::Facing>,
}

/// A stable index of one immutable static-atlas page.
///
/// It is intentionally not an index into a GPU texture array. Work 4 can use
/// an array where the adapter permits it or issue bounded page batches where it
/// does not; the CPU-side page identity is the same in both cases.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct StaticAtlasPage(pub u8);

/// The page and ordinary sprite data a paged static lookup returns.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PagedSprite {
    /// Texture page to bind before sampling `sprite.region`.
    pub page: StaticAtlasPage,
    /// The page-local sprite placement and geometry facts.
    pub sprite: Sprite,
}

/// The concrete source of static art for a world operation.
///
/// The renderer has exactly two sources: an ordinary single-page atlas in
/// tests and tools, and the bounded page family the client owns.  Keeping that
/// choice visible avoids a trait object that suggests third-party atlas
/// implementations are a supported extension point.
#[derive(Clone, Copy, Debug)]
pub enum StaticArt<'a> {
    /// One atlas page. Its sprites all belong to page zero.
    Single(&'a StaticAtlas),
    /// The client's bounded family of texture pages.
    Pages(&'a StaticAtlasPages),
}

impl<'a> From<&'a StaticAtlas> for StaticArt<'a> {
    fn from(atlas: &'a StaticAtlas) -> Self {
        Self::Single(atlas)
    }
}

impl<'a> From<&'a StaticAtlasPages> for StaticArt<'a> {
    fn from(atlas: &'a StaticAtlasPages) -> Self {
        Self::Pages(atlas)
    }
}

impl StaticArt<'_> {
    /// The page-local sprite to draw for `graphic`.
    pub fn paged_sprite(self, graphic: Graphic) -> Option<PagedSprite> {
        match self {
            Self::Single(atlas) => atlas.sprite(graphic).map(|sprite| PagedSprite {
                page: StaticAtlasPage(0),
                sprite,
            }),
            Self::Pages(atlas) => atlas.sprite(graphic),
        }
    }

    /// Whether an image texel is opaque.
    pub fn opaque_at(self, graphic: Graphic, x: u16, y: u16) -> bool {
        match self {
            Self::Single(atlas) => atlas.opaque_at(graphic, x, y),
            Self::Pages(atlas) => atlas.opaque_at(graphic, x, y),
        }
    }

    /// The measured hole in a graphic, if any.
    pub fn hole(self, graphic: Graphic) -> Option<crate::facing::Hole> {
        match self {
            Self::Single(atlas) => atlas.hole(graphic),
            Self::Pages(atlas) => atlas.hole(graphic),
        }
    }

    /// The measured prism in a graphic, if any.
    pub fn prism(self, graphic: Graphic) -> Option<crate::facing::Prism> {
        match self {
            Self::Single(atlas) => atlas.prism(graphic),
            Self::Pages(atlas) => atlas.prism(graphic),
        }
    }

    /// The measured footprint in a graphic, if any.
    pub fn footprint(self, graphic: Graphic) -> Option<crate::facing::Footprint> {
        match self {
            Self::Single(atlas) => atlas.footprint(graphic),
            Self::Pages(atlas) => atlas.footprint(graphic),
        }
    }

    /// The revision consumers cache their shape facts under.
    pub fn revision(self) -> u64 {
        match self {
            Self::Single(atlas) => atlas.revision(),
            Self::Pages(atlas) => atlas.revision(),
        }
    }

    /// Largest sprite dimensions across the source.
    pub fn max_sprite_size(self) -> (u16, u16) {
        match self {
            Self::Single(atlas) => atlas.max_sprite_size(),
            Self::Pages(atlas) => atlas.max_sprite_size(),
        }
    }

    /// Total packed graphics across all pages.
    pub fn len(self) -> usize {
        match self {
            Self::Single(atlas) => atlas.len(),
            Self::Pages(atlas) => atlas.len(),
        }
    }

    /// Whether the source packed no graphics at all.
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}

/// Maximum retained static pages in the first paged path.
///
/// Each page is a 2048×2048 RGBA8 texture (16 MiB), so eight pages retain at
/// most 128 MiB of texture pixels. The bound is chosen before renderer support
/// exists: it makes the future page-batch path finite on WebGL2 as well as on
/// native WebGPU, and turns a pathological map into a named limit rather than
/// an unbounded GPU-memory leak.
pub const MAX_STATIC_ATLAS_PAGES: usize = 8;

/// Static art, packed into one texture.
///
/// The third atlas and the first irregular one: a static sprite is any size from
/// a 2x2 pebble to a 250-pixel tree, so neither the land grid nor the texture
/// map's cells apply. Shelf packing is what fits — sort by height, fill a row,
/// start the next one below the tallest sprite in it — which is not optimal and
/// does not need to be: a screen of Britain holds a few hundred distinct
/// graphics and the waste is a few percent of one 2048 texture.
///
/// Keyed by the *static* graphic, which is `tiledata`'s static index and the
/// number a `map`'s static item carries. That is a different index space from
/// the land graphic [`LandAtlas`] is keyed by, and the two overlap numerically —
/// which is exactly why they are separate atlases rather than one with a prefix.
pub struct StaticAtlas {
    sprites: BTreeMap<Graphic, Packed>,
    /// The largest packed sprite in each direction. Picking uses this as a
    /// conservative screen-space margin, so it need not walk every visible
    /// static just to find one under the cursor.
    max_sprite: (u16, u16),
    /// The hole in each graphic that has one — see [`Hole`](crate::facing::Hole).
    ///
    /// Beside the sprites rather than on [`Sprite`], which is the opposite of
    /// where [`Sprite::facing`] lives, and the difference is who asks. A facing
    /// is read by whoever *places the quad* — `place::Stance`, per instance, on
    /// the hot path — so it rides on the thing that is already in hand. A hole is
    /// read once per graphic by [`crate::occlusion::collect`] and by nothing that
    /// draws, so a map keyed by graphic costs the drawing path nothing and keeps
    /// four bytes off every sprite of every atlas, most of which are letters.
    ///
    /// Fifty-eight entries on a 2D install and nearly all of them windows — see
    /// [`aperture_of`](crate::facing::aperture_of), which is what fills it, and
    /// [`StaticAtlas::state_hole`] for a scene that states one instead.
    holes: BTreeMap<Graphic, crate::facing::Hole>,
    /// The solid each graphic that is one is a picture of — see
    /// [`Prism`](crate::facing::Prism).
    ///
    /// Beside the atlas rather than on [`Sprite`] for the reason the holes are:
    /// what reads it is the occlusion grid, which asks about a *graphic* while it
    /// walks the map, and nothing that places a quad has any use for it. A stair
    /// is a rarity in an install — 576 climbable statics of 39,189 pictures — so a
    /// map is the right shape for it and a field on every sprite is not.
    prisms: BTreeMap<Graphic, crate::facing::Prism>,
    /// The horizontal box each graphic's own base edge states, where the
    /// picture is one and nothing else already answered for it — see
    /// [`Footprint`](crate::facing::Footprint) and `docs/footprints.md`'s S3.
    ///
    /// Beside the atlas for the same reason the holes and the prisms are: what
    /// reads it is [`crate::occlusion::boxes_of`], walking the map, and no
    /// per-quad drawing path has any use for it.
    footprints: BTreeMap<Graphic, crate::facing::Footprint>,
    /// What was measured off this install's art before the client started, or
    /// `None` for an atlas that has to measure as it packs.
    ///
    /// **The whole of `docs/lighting.md`'s decision 31 at the seam it arrives
    /// through.** With a table, packing a graphic is a lookup; without one it is
    /// a second walk of the pixels [`copy_sprite`] has just copied, which is
    /// what this did on every frame that introduced a graphic. A client with no
    /// table still works and says so in a log line — decision 31.6 — which is
    /// why this is an `Option` and not a required argument.
    ///
    /// Owned rather than borrowed: an atlas outlives the frame it was built in
    /// and is rebuilt from scratch when it fills up, and a lifetime on
    /// [`StaticAtlas`] would travel into every renderer that holds one. It is a
    /// few thousand rows.
    table: Option<crate::arttable::ArtTable>,
    /// Every graphic ever offered, whether or not the client ships art for it.
    ///
    /// The one that most needed writing down: "does the atlas hold everything
    /// on screen" answered *no* for ever whenever one visible static had no
    /// art, because a graphic that cannot be packed is never packed — so one
    /// such tile repacked every atlas on every frame. Asking each graphic once
    /// is what makes the question terminate.
    asked: BTreeSet<Graphic>,
    /// How many times the answers [`StaticAtlas::sprite`], [`StaticAtlas::hole`]
    /// and [`StaticAtlas::prism`] give have changed — see
    /// [`StaticAtlas::revision`].
    revision: u64,
    /// Where the next sprite goes, kept between growths.
    shelf: Shelf,
    /// `ATLAS_SIDE * ATLAS_SIDE` RGBA8 pixels, row-major.
    pixels: Vec<u8>,
    dirty: Dirty,
}

impl fmt::Debug for StaticAtlas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticAtlas")
            .field("graphics", &self.sprites.len())
            .field("side", &ATLAS_SIDE)
            .finish()
    }
}

impl StaticAtlas {
    /// Pack every graphic in `wanted` that the client actually ships.
    ///
    /// A graphic with no art is skipped rather than refused, the same way the
    /// land atlas skips an empty land slot: a map naming a static the client has
    /// no picture for is the file's business.
    pub fn build(art: &Art, wanted: impl IntoIterator<Item = Graphic>) -> Result<Self, AtlasError> {
        Self::build_from(art, wanted, None)
    }

    /// The same, reading each graphic's surface out of a table measured off the
    /// clock instead of measuring it here.
    ///
    /// `docs/lighting.md`'s decision 31: what the client does per frame is a
    /// lookup, and the measurement happened in a tool with a budget of a minute.
    /// `None` is the client that has no table — it measures as it packs, exactly
    /// as it did before one existed.
    ///
    /// The table travels into the atlas rather than into this call alone because
    /// [`add`](Self::add) packs too: a scroll introduces graphics for the rest of
    /// the session, and an atlas that read the table once at startup would go
    /// back to measuring the moment the camera moved.
    pub fn build_from(
        art: &Art,
        wanted: impl IntoIterator<Item = Graphic>,
        table: Option<crate::arttable::ArtTable>,
    ) -> Result<Self, AtlasError> {
        let mut atlas = Self::empty();
        atlas.table = table;
        atlas.add(art, wanted)?;
        atlas.dirty.take();
        Ok(atlas)
    }

    /// An atlas holding nothing, ready to be grown into.
    fn empty() -> Self {
        let side = ATLAS_SIDE as usize;
        Self {
            sprites: BTreeMap::new(),
            max_sprite: (0, 0),
            holes: BTreeMap::new(),
            prisms: BTreeMap::new(),
            footprints: BTreeMap::new(),
            table: None,
            asked: BTreeSet::new(),
            revision: 0,
            shelf: Shelf::default(),
            pixels: vec![0u8; side * side * 4],
            dirty: Dirty::default(),
        }
    }

    /// Pack whichever of `wanted` this atlas has not been offered before.
    ///
    /// [`LandAtlas::add`] for the sprites standing on the ground, and the one
    /// that actually costs something: a screen of Britain holds a few hundred
    /// distinct static graphics and walking 136 tiles introduces four hundred
    /// more, so this is the growth a scroll spends its time on.
    pub fn add(&mut self, art: &Art, wanted: impl IntoIterator<Item = Graphic>) -> Result<(), AtlasError> {
        let wanted: BTreeSet<Graphic> = wanted.into_iter().collect();
        let fresh: Vec<Graphic> = wanted
            .into_iter()
            .filter(|graphic| !self.asked.contains(graphic))
            .collect();
        if fresh.is_empty() {
            return Ok(());
        }
        let mut images = Vec::with_capacity(fresh.len());
        for graphic in &fresh {
            if let Some(image) = art.static_art(*graphic)? {
                images.push((*graphic, image));
            }
        }
        self.insert(images)?;
        self.asked.extend(fresh);
        Ok(())
    }

    /// The rows written since this was last asked, cleared. See
    /// [`LandAtlas::take_dirty`].
    pub fn take_dirty(&mut self) -> Option<std::ops::Range<u32>> {
        self.dirty.take()
    }

    /// Pack sprites somebody else decoded, tallest first.
    ///
    /// Tallest first is what makes a shelf worth using at all: rows started by a
    /// short sprite waste the whole difference under every tall one that lands
    /// beside it. Deterministic given the same input — same order in, same
    /// pixels out — which the frame tests depend on.
    pub fn pack(images: impl IntoIterator<Item = (Graphic, Image)>) -> Result<Self, AtlasError> {
        Self::pack_from(images, None)
    }

    /// The same, reading each picture's surface out of a table instead of
    /// measuring it — [`build_from`](Self::build_from) for pictures somebody else
    /// decoded.
    ///
    /// It exists for the same reason [`pack`](Self::pack) does beside
    /// [`build`](Self::build): the readers that hand this atlas pictures directly
    /// are the tests and the built scenes, and a seam that only the file-reading
    /// path went through would be a seam no test could stand on.
    pub fn pack_from(
        images: impl IntoIterator<Item = (Graphic, Image)>,
        table: Option<crate::arttable::ArtTable>,
    ) -> Result<Self, AtlasError> {
        let mut atlas = Self::empty();
        atlas.table = table;
        atlas.pack_more(images)?;
        atlas.dirty.take();
        Ok(atlas)
    }

    /// Pack more sprites into an atlas that already holds some. See
    /// [`LandAtlas::pack_more`].
    pub fn pack_more(
        &mut self,
        images: impl IntoIterator<Item = (Graphic, Image)>,
    ) -> Result<(), AtlasError> {
        let images: Vec<(Graphic, Image)> = images.into_iter().collect();
        self.asked.extend(images.iter().map(|(graphic, _)| *graphic));
        self.insert(images)
    }

    /// Shelve pictures beside what is already packed, marking the rows written.
    ///
    /// Tallest first *within one growth*: a shelf sorted across the whole atlas
    /// is not something a growing atlas can have, and the cost is waste rather
    /// than error — a short growth starts a row that a later tall sprite cannot
    /// share. One [`pack`](Self::pack) still lays out exactly as it always did.
    fn insert(&mut self, images: impl IntoIterator<Item = (Graphic, Image)>) -> Result<(), AtlasError> {
        // Deduplicated and ordered by graphic first, so the sort below is a
        // total order rather than "whichever the caller happened to yield
        // first" — `sort_by_key` is stable and the tie-break is the graphic.
        let images: BTreeMap<Graphic, Image> = images.into_iter().collect();
        let wanted = self.sprites.len() + images.len();
        let mut order: Vec<(Graphic, Image)> = images.into_iter().collect();
        order.sort_by_key(|(_, image)| std::cmp::Reverse(image.height()));

        for (graphic, image) in order {
            if self.sprites.contains_key(&graphic) {
                continue;
            }
            // A growth that actually packs something is what a reader keyed on
            // `revision` has to notice, and a growth that packs nothing must not
            // look like one: the app offers this atlas every visible graphic on
            // every frame, so bumping per *call* would tell an occlusion bake its
            // shapes had changed sixty times a second. See `StaticAtlas::revision`.
            self.revision += 1;
            let (width, height) = (image.width(), image.height());
            // A sprite wider or taller than the whole atlas cannot be packed at
            // any offset. The client ships nothing near it — the tallest art is
            // around 250 pixels — so this is a corrupt-file case rather than a
            // capacity one, and it says which graphic.
            if u32::from(width) > ATLAS_SIDE || u32::from(height) > ATLAS_SIDE {
                return Err(AtlasError::Oversized {
                    graphic,
                    width,
                    height,
                });
            }
            let Some((origin_x, origin_y)) = self.shelf.take(u32::from(width), u32::from(height)) else {
                return Err(AtlasError::Full {
                    wanted,
                    capacity: self.sprites.len(),
                });
            };
            self.dirty.mark(origin_y, u32::from(height));

            // Every pixel, transparent ones included: a static sprite genuinely
            // has transparency — it is a picture with a shape, not a diamond
            // with a known one — and the alpha channel is what the fragment
            // shader discards on.
            //
            // Zero *is* absent here, which is the opposite of the rule for land
            // art and is the client's own: `ArtLoader.ReadStaticArt` writes a
            // run's pixel only `if (val != 0)`, leaving the rest of the buffer
            // at zero alpha. So a zero inside a run and a column no run covered
            // are the same thing to the client, and `Color16::TRANSPARENT` for
            // both loses nothing.
            copy_sprite(&mut self.pixels, &image, origin_x, origin_y);

            // A lookup where there is a table, and the two measurements over the
            // pixels just copied where there is not — see `Sprite::facing` and
            // `self.table` for why the walk is no longer the only answer, and
            // `crate::facing::aperture_of` for the second one.
            //
            // A table's *absent* row is a graphic it measured and refused, so
            // this does not fall back to measuring on a miss: doing that would
            // put the frame cost back on precisely the graphics the tool already
            // decided nothing can be said about, which is most of them.
            let shape = match &self.table {
                Some(table) => table.shape(graphic),
                None => crate::occlusion::Shape::of(&image),
            };
            if let Some(hole) = shape.hole {
                self.holes.insert(graphic, hole);
            }
            if let Some(prism) = shape.prism {
                self.prisms.insert(graphic, prism);
            }
            if let Some(footprint) = shape.footprint {
                self.footprints.insert(graphic, footprint);
            }

            self.sprites.insert(
                graphic,
                Packed {
                    sprite: Sprite {
                        region: region_at(origin_x, origin_y, width, height),
                        width,
                        height,
                        facing: shape.facing,
                    },
                    origin: (origin_x, origin_y),
                },
            );
            self.max_sprite.0 = self.max_sprite.0.max(width);
            self.max_sprite.1 = self.max_sprite.1.max(height);
        }

        Ok(())
    }

    /// How many images at the front of a growth this page can accept without
    /// changing it.
    ///
    /// [`StaticAtlasPages`] uses this before it touches an active page. A failed
    /// `insert` would otherwise leave a partially changed page and make the
    /// remaining images ambiguous; a preflight keeps a sealed page byte-for-byte
    /// stable while its successors are allocated.
    fn fitting_prefix(&self, images: &[(Graphic, Image)]) -> Result<usize, AtlasError> {
        let mut shelf = self.shelf.clone();
        for (index, (graphic, image)) in images.iter().enumerate() {
            let (width, height) = (image.width(), image.height());
            if u32::from(width) > ATLAS_SIDE || u32::from(height) > ATLAS_SIDE {
                return Err(AtlasError::Oversized {
                    graphic: *graphic,
                    width,
                    height,
                });
            }
            if shelf.take(u32::from(width), u32::from(height)).is_none() {
                return Ok(index);
            }
        }
        Ok(images.len())
    }

    /// The atlas texture's side in pixels. Square, and the same as the others'.
    pub const fn side() -> u32 {
        ATLAS_SIDE
    }

    /// Its pixels, RGBA8 and row-major, ready for `write_texture`.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// How many graphics landed in it.
    pub fn len(&self) -> usize {
        self.sprites.len()
    }

    /// How many of `wanted` have not been offered to this atlas before.
    ///
    /// This is deliberately about requests rather than packed sprites: an
    /// absent art file is still remembered in `asked`, otherwise it would be
    /// read again on every frame. Overflow diagnostics need the same boundary
    /// to say how much genuinely new work caused the fill.
    pub fn newly_requested(&self, wanted: impl IntoIterator<Item = Graphic>) -> usize {
        wanted
            .into_iter()
            .filter(|graphic| !self.asked.contains(graphic))
            .count()
    }

    /// Largest packed sprite dimensions, in the image's pixels.
    ///
    /// The empty atlas answers `(0, 0)`: with no art, a static cannot be
    /// picked, and a caller's one-tile safety margin still leaves a valid
    /// search rectangle.
    pub fn max_sprite_size(&self) -> (u16, u16) {
        self.max_sprite
    }

    /// Whether nothing did.
    pub fn is_empty(&self) -> bool {
        self.sprites.is_empty()
    }

    /// Where a graphic sits and how big it is, or `None` if it is not packed.
    pub fn sprite(&self, graphic: Graphic) -> Option<Sprite> {
        self.sprites.get(&graphic).map(|packed| packed.sprite)
    }

    /// The hole in a graphic's surface, or `None` for a solid — which is all but
    /// fifty-eight of a real install's pictures. See [`Hole`](crate::facing::Hole).
    pub fn hole(&self, graphic: Graphic) -> Option<crate::facing::Hole> {
        self.holes.get(&graphic).copied()
    }

    /// The solid a graphic is a picture of, or `None` where no prism fits it.
    ///
    /// Whether it is *believed* is not settled here: a wall scores 0.81 against
    /// its best prism, so what admits one is the client's own `CLIMBABLE` bit
    /// first — see [`Builder::add`](crate::occlusion::Builder::add).
    pub fn prism(&self, graphic: Graphic) -> Option<crate::facing::Prism> {
        self.prisms.get(&graphic).copied()
    }

    /// The horizontal box a graphic's own base edge states, or `None` where
    /// nothing was measured — either because a face or a corner already named
    /// which edge it stands on, or because the base is not two 45° runs at all.
    /// See [`Footprint`](crate::facing::Footprint).
    pub fn footprint(&self, graphic: Graphic) -> Option<crate::facing::Footprint> {
        self.footprints.get(&graphic).copied()
    }

    /// Say what solid a graphic is, without measuring one.
    ///
    /// The pair to [`StaticAtlas::state_hole`], and for the same reason: a built
    /// scene names the shape it wants to reason about rather than drawing a
    /// staircase convincing enough to be measured.
    pub fn state_prism(&mut self, graphic: Graphic, prism: crate::facing::Prism) {
        self.prisms.insert(graphic, prism);
        self.revision += 1;
    }

    /// Say what hole a graphic has, without measuring one.
    ///
    /// What a built scene uses: `docs/lighting.md`'s step 21.3 is the *mechanism*
    /// — a hole in the walk, tested on a scene that states one — and step 16 is
    /// the measurement that reads it off a real window's silhouette. The two are
    /// deliberately independent, and this method is the line between them: a
    /// scene names the hole it wants to reason about instead of drawing a window
    /// convincing enough to be measured.
    ///
    /// It does not need the graphic to be packed. A scene that states a hole in a
    /// wall it also draws will pack it; one that only wants the occluder need
    /// not, and refusing here would make the order of two unrelated calls matter.
    pub fn state_hole(&mut self, graphic: Graphic, hole: crate::facing::Hole) {
        self.holes.insert(graphic, hole);
        self.revision += 1;
    }

    /// Take back every footprint this atlas measured, standing each of those
    /// pictures back on the whole tile.
    ///
    /// The opposite direction to [`state_prism`](Self::state_prism) and
    /// [`state_hole`](Self::state_hole), and it exists for the same kind of
    /// caller: a scene that wants to reason about a shape rather than about
    /// whatever the art happens to say. Here the shape wanted is the one that
    /// shipped *before* `docs/footprints.md`'s S3 — the whole tile
    /// [`occlusion::shape_of`](crate::occlusion::shape_of) falls back to — so
    /// that one run of a tool can draw a place both ways and a person can put
    /// the two pictures beside each other. `tests/lid.rs` states the same
    /// counterfactual by hand, one call to `boxes_of` at a time; this is it for
    /// a whole frame, which is what a *shadow* needs.
    ///
    /// Nothing narrower is offered on purpose: a per-graphic version would be a
    /// second policy about which pictures deserve a footprint, and the one that
    /// decides that is [`crate::facing::footprint_of`].
    pub fn forget_footprints(&mut self) {
        self.footprints.clear();
        self.revision += 1;
    }

    /// How many times what this atlas says about a graphic's *shape* has changed.
    ///
    /// Monotonic, and it counts four answers and no others:
    /// [`sprite`](Self::sprite)'s facing, [`hole`](Self::hole),
    /// [`prism`](Self::prism) and [`footprint`](Self::footprint) — which are
    /// exactly what [`occlusion::Shape`](crate::occlusion::Shape) is made of
    /// (`blocks` excepted: no detector writes one), and therefore exactly what
    /// a grid derived from this atlas depends on.
    ///
    /// It exists for [`occlusion::Bake`](crate::occlusion::Bake), and the failure
    /// it prevents is the quiet one. An atlas *grows*: a graphic the camera has
    /// not reached yet is not in it, so a block baked before that graphic was
    /// packed holds the whole-tile fallback, and nothing about the baked block
    /// would ever say it was built from a poorer answer than the one available
    /// now. A wall would stay a body for as long as the player stood still. So
    /// the bake keeps the revision it was built under and throws the cache away
    /// when it moves, which is a handful of times in a session and never in a
    /// steady frame.
    ///
    /// Pixels are deliberately not in it. A dirty row is a texture upload and
    /// changes no geometry.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether the pixel at `(x, y)` *within* a graphic's own picture is drawn
    /// rather than transparent — the alpha the fragment shader discards on, read
    /// back on the CPU.
    ///
    /// This is what makes picking hit the *picture* and not its bounding box. A
    /// static's box is mostly empty — a door's leaf is a slim diagonal in a tall
    /// rectangle, and two of them in a shopfront overlap boxes without ever
    /// overlapping a pixel — so a box test picks a door the player is pointing
    /// past. The shader draws a texel exactly when its alpha is non-zero (see
    /// [`copy_sprite`], where zero *is* absent, the client's own rule), and this
    /// asks the same texel the same question.
    ///
    /// `false` for a graphic that is not packed and for a coordinate outside the
    /// picture: neither is a pixel the player can have clicked on.
    pub fn opaque_at(&self, graphic: Graphic, x: u16, y: u16) -> bool {
        let Some(packed) = self.sprites.get(&graphic) else {
            return false;
        };
        if x >= packed.sprite.width || y >= packed.sprite.height {
            return false;
        }
        let side = ATLAS_SIDE as usize;
        let (origin_x, origin_y) = packed.origin;
        let at = ((origin_y as usize + usize::from(y)) * side + origin_x as usize + usize::from(x)) * 4;
        self.pixels[at + 3] != 0
    }
}

/// One picture in the static atlas: what to draw it with, and where its pixels
/// actually are.
///
/// The origin is kept beside the region rather than recovered from it. A region
/// is normalised — the whole reason [`Sprite`] carries its size in pixels as
/// well — and multiplying `u * side` back to an integer is a second answer to a
/// question that already has one, off by a texel wherever the rounding falls the
/// other way. Reading the *wrong* texel is exactly the bug picking cannot
/// afford, since a one-pixel miss along a sprite's edge is invisible until a
/// player is pointing at something and nothing happens.
struct Packed {
    sprite: Sprite,
    /// Its top-left corner in atlas pixels.
    origin: (u32, u32),
}

/// A dirty row band belonging to one [`StaticAtlasPages`] texture page.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DirtyStaticAtlasPage {
    /// Texture page whose rows changed.
    pub page: StaticAtlasPage,
    /// Pixel rows in that page, half-open.
    pub rows: std::ops::Range<u32>,
}

/// Static art spread over a bounded sequence of immutable texture pages.
///
/// Each page uses the established 2048px shelf format. While a page can still
/// accept a growth it behaves like [`StaticAtlas`]; as soon as the next ordered
/// image does not fit, that page is sealed forever and the remainder goes into
/// a fresh page. No visible graphic is evicted or decoded a second time merely
/// because an older page filled.
///
/// The production static renderer consumes [`StaticAtlasPage`] on
/// [`PagedSprite`] as a bounded page-batch binding. [`StaticAtlas`] remains a
/// one-page baseline for tests and embeddings that do not need paging.
pub struct StaticAtlasPages {
    pages: Vec<StaticAtlas>,
    /// Which page holds each graphic, indexed by the graphic.
    ///
    /// Dense for the same reason [`LandAtlas::regions`] is, and it earns it
    /// harder: a map static is asked this question four times over on the frame
    /// it is drawn — [`Self::sprite`], [`Self::prism`], [`Self::footprint`] and
    /// [`Self::opaque_at`] each resolve the page for themselves — and a
    /// far-zoom frame draws seven thousand of them.
    page_of: Box<[Option<StaticAtlasPage>]>,
    /// How many of `page_of` are filled.
    packed: u32,
    /// Requests belong to the family rather than to one page: absent art must
    /// not be re-read after a later page is allocated.
    asked: BTreeSet<Graphic>,
    table: Option<crate::arttable::ArtTable>,
    page_limit: usize,
    revision: u64,
}

impl fmt::Debug for StaticAtlasPages {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticAtlasPages")
            .field("pages", &self.pages.len())
            .field("graphics", &self.packed)
            .field("page_limit", &self.page_limit)
            .finish()
    }
}

impl StaticAtlasPages {
    /// Pack pictures into the default bounded family of pages.
    pub fn pack(images: impl IntoIterator<Item = (Graphic, Image)>) -> Result<Self, AtlasError> {
        Self::pack_with_limit(images, MAX_STATIC_ATLAS_PAGES)
    }

    /// Same as [`pack`](Self::pack), with a smaller bound available to tests
    /// and to a memory-constrained embedding.
    pub fn pack_with_limit(
        images: impl IntoIterator<Item = (Graphic, Image)>,
        page_limit: usize,
    ) -> Result<Self, AtlasError> {
        Self::pack_from_with_limit(images, None, page_limit)
    }

    /// Pack decoded pictures while reusing a precomputed art-surface table.
    pub fn pack_from(
        images: impl IntoIterator<Item = (Graphic, Image)>,
        table: Option<crate::arttable::ArtTable>,
    ) -> Result<Self, AtlasError> {
        Self::pack_from_with_limit(images, table, MAX_STATIC_ATLAS_PAGES)
    }

    /// Page-limited version of [`pack_from`](Self::pack_from).
    pub fn pack_from_with_limit(
        images: impl IntoIterator<Item = (Graphic, Image)>,
        table: Option<crate::arttable::ArtTable>,
        page_limit: usize,
    ) -> Result<Self, AtlasError> {
        let mut atlas = Self::empty_with_limit(table, page_limit)?;
        atlas.pack_more(images)?;
        // A caller that constructs pages hands each initial texture to its
        // renderer whole, exactly as [`StaticAtlas::pack`] does.
        atlas.take_dirty();
        Ok(atlas)
    }

    /// Read and pack every available static image in `wanted`.
    pub fn build(art: &Art, wanted: impl IntoIterator<Item = Graphic>) -> Result<Self, AtlasError> {
        Self::build_from(art, wanted, None)
    }

    /// [`build`](Self::build) with a precomputed art-surface table.
    pub fn build_from(
        art: &Art,
        wanted: impl IntoIterator<Item = Graphic>,
        table: Option<crate::arttable::ArtTable>,
    ) -> Result<Self, AtlasError> {
        let mut atlas = Self::empty_with_limit(table, MAX_STATIC_ATLAS_PAGES)?;
        atlas.add(art, wanted)?;
        atlas.take_dirty();
        Ok(atlas)
    }

    fn empty_with_limit(
        table: Option<crate::arttable::ArtTable>,
        page_limit: usize,
    ) -> Result<Self, AtlasError> {
        // `StaticAtlasPage` is intentionally compact enough to travel with an
        // instance, so do not accept a limit it cannot name.
        if page_limit == 0 || page_limit > usize::from(u8::MAX) + 1 {
            return Err(AtlasError::PageLimit {
                wanted: 1,
                limit: page_limit,
            });
        }
        let mut first = StaticAtlas::empty();
        first.table = table.clone();
        Ok(Self {
            pages: vec![first],
            page_of: vec![None; GRAPHIC_SLOTS].into_boxed_slice(),
            packed: 0,
            asked: BTreeSet::new(),
            table,
            page_limit,
            revision: 0,
        })
    }

    /// Add static-file images the family has not already been asked for.
    pub fn add(&mut self, art: &Art, wanted: impl IntoIterator<Item = Graphic>) -> Result<(), AtlasError> {
        let fresh: Vec<Graphic> = wanted
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|graphic| !self.asked.contains(graphic))
            .collect();
        if fresh.is_empty() {
            return Ok(());
        }
        let mut images = Vec::with_capacity(fresh.len());
        for graphic in &fresh {
            if let Some(image) = art.static_art(*graphic)? {
                images.push((*graphic, image));
            }
        }
        self.insert_fresh(images)?;
        self.asked.extend(fresh);
        Ok(())
    }

    /// Add already-decoded pictures, without rebuilding or moving prior pages.
    pub fn pack_more(
        &mut self,
        images: impl IntoIterator<Item = (Graphic, Image)>,
    ) -> Result<(), AtlasError> {
        let images: BTreeMap<Graphic, Image> = images.into_iter().collect();
        self.asked.extend(images.keys().copied());
        self.insert_fresh(images.into_iter().collect())
    }

    fn insert_fresh(&mut self, images: Vec<(Graphic, Image)>) -> Result<(), AtlasError> {
        // Same total order as `StaticAtlas::insert`: reproducing it means a
        // preflight and the subsequent page insertion cannot disagree.
        let mut pending: Vec<(Graphic, Image)> = images
            .into_iter()
            .filter(|(graphic, _)| self.page_of[graphic.0 as usize].is_none())
            .collect();
        pending.sort_by_key(|(_, image)| std::cmp::Reverse(image.height()));

        while !pending.is_empty() {
            let fit = self
                .pages
                .last()
                .expect("a paged atlas always has a first page")
                .fitting_prefix(&pending)?;
            if fit == 0 {
                if self.pages.len() == self.page_limit {
                    return Err(AtlasError::PageLimit {
                        wanted: self.pages.len() + 1,
                        limit: self.page_limit,
                    });
                }
                let mut page = StaticAtlas::empty();
                page.table = self.table.clone();
                self.pages.push(page);
                continue;
            }

            let packed: Vec<(Graphic, Image)> = pending.drain(..fit).collect();
            let page_index = self.pages.len() - 1;
            self.pages[page_index].pack_more(packed.iter().cloned())?;
            let page = StaticAtlasPage(page_index as u8);
            for (graphic, _) in packed {
                self.page_of[graphic.0 as usize] = Some(page);
                self.packed += 1;
                self.revision += 1;
            }
        }
        Ok(())
    }

    /// How many texture pages are currently retained, including an empty first
    /// page before any static art is discovered.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The configured hard page limit.
    pub fn page_limit(&self) -> usize {
        self.page_limit
    }

    /// Total packed graphics across all pages.
    pub fn len(&self) -> usize {
        self.packed as usize
    }

    /// Whether no graphic has been packed.
    pub fn is_empty(&self) -> bool {
        self.packed == 0
    }

    /// The per-page atlas a renderer will turn into one texture and bind group.
    pub fn page(&self, page: StaticAtlasPage) -> Option<&StaticAtlas> {
        self.pages.get(usize::from(page.0))
    }

    /// The page and sprite data for a graphic, or `None` when it has no art.
    pub fn sprite(&self, graphic: Graphic) -> Option<PagedSprite> {
        let page = self.page_of[graphic.0 as usize]?;
        Some(PagedSprite {
            page,
            sprite: self.page(page)?.sprite(graphic)?,
        })
    }

    /// Dirty bands from every page that changed since the prior call.
    pub fn take_dirty(&mut self) -> Vec<DirtyStaticAtlasPage> {
        self.pages
            .iter_mut()
            .enumerate()
            .filter_map(|(index, atlas)| {
                atlas.take_dirty().map(|rows| DirtyStaticAtlasPage {
                    page: StaticAtlasPage(index as u8),
                    rows,
                })
            })
            .collect()
    }

    /// How many graphics in `wanted` have never been offered to this family.
    pub fn newly_requested(&self, wanted: impl IntoIterator<Item = Graphic>) -> usize {
        wanted
            .into_iter()
            .filter(|graphic| !self.asked.contains(graphic))
            .count()
    }

    /// The largest sprite across all pages, for the same conservative picking
    /// margin an ordinary [`StaticAtlas`] exposes.
    pub fn max_sprite_size(&self) -> (u16, u16) {
        self.pages.iter().fold((0, 0), |max, page| {
            let size = page.max_sprite_size();
            (max.0.max(size.0), max.1.max(size.1))
        })
    }

    /// A monotonic family revision for consumers whose cached geometry depends
    /// on static shape facts. Pages never move a packed graphic, so this only
    /// changes when a newly packed graphic makes an answer available.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The CPU picking question, delegated to the page holding the graphic.
    pub fn opaque_at(&self, graphic: Graphic, x: u16, y: u16) -> bool {
        self.page_of[graphic.0 as usize]
            .and_then(|page| self.page(page))
            .is_some_and(|page| page.opaque_at(graphic, x, y))
    }

    /// The measured hole, if the graphic's page supplied one.
    pub fn hole(&self, graphic: Graphic) -> Option<crate::facing::Hole> {
        self.page_of[graphic.0 as usize]
            .and_then(|page| self.page(page))
            .and_then(|page| page.hole(graphic))
    }

    /// The measured prism, if the graphic's page supplied one.
    pub fn prism(&self, graphic: Graphic) -> Option<crate::facing::Prism> {
        self.page_of[graphic.0 as usize]
            .and_then(|page| self.page(page))
            .and_then(|page| page.prism(graphic))
    }

    /// The measured horizontal footprint, if the graphic's page supplied one.
    pub fn footprint(&self, graphic: Graphic) -> Option<crate::facing::Footprint> {
        self.page_of[graphic.0 as usize]
            .and_then(|page| self.page(page))
            .and_then(|page| page.footprint(graphic))
    }
}

/// The client-file key for a whole animation, re-exported where the atlas uses
/// it. The reader and renderer must agree on this triple; keeping one shared
/// type avoids each side accepting the same three unlabelled integers.
pub use openshard_uofiles::anim::AnimationKey;

/// Which picture of an animation this is.
///
/// A body alone is not a sprite: it is a body, an action, a facing and a moment
/// in that action, and the file is indexed by exactly that tuple. Carried as one
/// value so an atlas can be keyed by it — and so that a caller cannot pass the
/// group where the direction goes, which the file would answer with somebody
/// else's frames rather than with nothing.
///
/// The direction is the *stored* one, 0 to 4: the other three facings are
/// mirrors of these and share their pictures, so they share an atlas entry too.
/// Mirroring is [`SpriteQuad::mirrored`](crate::sprite::SpriteQuad::mirrored)
/// and it happens where the quad is built.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct FrameKey {
    /// The animation this picture belongs to.
    pub animation: AnimationKey,
    /// Which frame of that animation.
    pub frame: AnimationFrameIndex,
}

impl FrameKey {
    #[must_use]
    pub const fn new(animation: AnimationKey, frame: AnimationFrameIndex) -> Self {
        Self { animation, frame }
    }
}

/// One packed animation frame: where it is, how big, and where the feet are.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PackedFrame {
    /// Where to sample it, and how big it is.
    pub sprite: Sprite,
    /// The frame's own centre offsets, carried through unchanged.
    ///
    /// They are not the middle of the picture and they are not the atlas's
    /// business: a walking frame leans, and the lean lives in these two numbers
    /// rather than in the pixels. See [`AnimFrame`].
    pub center_x: i16,
    /// The vertical half of the same pair.
    pub center_y: i16,
    /// Its top-left corner in atlas pixels.
    ///
    /// Private, and kept beside the region rather than recovered from it, for
    /// the reason [`Packed::origin`] is: a region is normalised, and
    /// multiplying `u * side` back to an integer is a second answer to a
    /// question that already has one. [`AnimAtlas::opaque_at`] reads the texel
    /// picking is decided by, and a one-pixel miss there is a creature that
    /// cannot be pointed at along its own edge.
    origin: (u32, u32),
}

/// Animation frames, packed into one texture.
///
/// The same shelf packing [`StaticAtlas`] uses, keyed by [`FrameKey`] instead of
/// a graphic. Separate from the statics atlas rather than sharing one: a screen
/// holds a few hundred static graphics and a handful of mobiles, they are
/// rebuilt on completely different triggers — the camera moving against a
/// creature turning — and a draw call binds one texture either way.
pub struct AnimAtlas {
    frames: BTreeMap<FrameKey, PackedFrame>,
    /// Every body-group-direction ever offered, animated or not.
    ///
    /// Keyed by the triple and not by [`FrameKey`], because a triple is what a
    /// caller asks for and what the file answers in one read. Most of the index
    /// is empty, so without this a creature whose group the client ships no
    /// animation for would seek `anim.mul` once a frame for ever.
    asked: BTreeSet<AnimationKey>,
    /// Where the next frame goes, kept between growths.
    shelf: Shelf,
    /// `ATLAS_SIDE * ATLAS_SIDE` RGBA8 pixels, row-major.
    pixels: Vec<u8>,
    dirty: Dirty,
}

impl fmt::Debug for AnimAtlas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnimAtlas")
            .field("frames", &self.frames.len())
            .field("side", &ATLAS_SIDE)
            .finish()
    }
}

impl AnimAtlas {
    /// Read and pack every frame of the bodies, groups and directions asked for.
    ///
    /// `wanted` is body-group-direction triples, each of which brings its whole
    /// animation: a caller that wanted one frame of a walk would still have to
    /// read the entry the others are in, so packing them all costs nothing but
    /// atlas space and saves re-reading 195MB the moment the frame advances.
    ///
    /// A triple the client ships no animation for is skipped, not refused. Most
    /// of the index is empty — see [`Anim`] — and a body without a group is the
    /// ordinary case rather than a failure.
    pub fn build(
        anim: &mut Anim,
        wanted: impl IntoIterator<Item = AnimationKey>,
    ) -> Result<Self, AtlasError> {
        let mut atlas = Self::empty();
        atlas.add(anim, wanted)?;
        atlas.dirty.take();
        Ok(atlas)
    }

    /// How many animation groups in `wanted` have not been requested before.
    pub fn newly_requested(&self, wanted: impl IntoIterator<Item = AnimationKey>) -> usize {
        wanted
            .into_iter()
            .filter(|animation| !self.asked.contains(animation))
            .count()
    }

    /// An atlas holding nothing, ready to be grown into.
    fn empty() -> Self {
        let side = ATLAS_SIDE as usize;
        Self {
            frames: BTreeMap::new(),
            asked: BTreeSet::new(),
            shelf: Shelf::default(),
            pixels: vec![0u8; side * side * 4],
            dirty: Dirty::default(),
        }
    }

    /// Read and pack whichever triples this atlas has not been offered before.
    ///
    /// [`LandAtlas::add`] for the mobiles, and its trigger is a different one:
    /// this atlas goes stale when a creature *turns* or a new one walks into
    /// view, not when the camera moves. A body that arrives mid-frame therefore
    /// costs one seek into `anim.mul` and a few hundred rows of upload, where it
    /// used to cost re-reading every animation on screen.
    pub fn add(
        &mut self,
        anim: &mut Anim,
        wanted: impl IntoIterator<Item = AnimationKey>,
    ) -> Result<(), AtlasError> {
        // Sorted and deduplicated, so the same request always packs the same
        // atlas — the frame tests depend on it, and so does not re-reading a
        // body twice because the caller listed it twice.
        let wanted: BTreeSet<AnimationKey> = wanted.into_iter().collect();
        let fresh: Vec<AnimationKey> = wanted
            .into_iter()
            .filter(|triple| !self.asked.contains(triple))
            .collect();
        if fresh.is_empty() {
            return Ok(());
        }
        let mut images = Vec::new();
        for animation in fresh.iter().copied() {
            let Some(frames) = anim.frames(animation)? else {
                continue;
            };
            for (index, frame) in frames.into_iter().enumerate() {
                // A blank frame is a real thing in these files, and it packs to
                // nothing: an empty picture has no pixels to copy and no region
                // worth handing back.
                if frame.image.width() == 0 || frame.image.height() == 0 {
                    continue;
                }
                images.push((FrameKey::new(animation, AnimationFrameIndex(index as u16)), frame));
            }
        }
        self.insert(images)?;
        self.asked.extend(fresh);
        Ok(())
    }

    /// The rows written since this was last asked, cleared. See
    /// [`LandAtlas::take_dirty`].
    pub fn take_dirty(&mut self) -> Option<std::ops::Range<u32>> {
        self.dirty.take()
    }

    /// Pack frames somebody else decoded.
    ///
    /// The way in that needs no client install, exactly as
    /// [`StaticAtlas::pack`] is: a test hands this the pictures it chose and
    /// then asserts on the pixels the frame comes back with.
    pub fn pack(frames: impl IntoIterator<Item = (FrameKey, AnimFrame)>) -> Result<Self, AtlasError> {
        let mut atlas = Self::empty();
        let frames: Vec<(FrameKey, AnimFrame)> = frames.into_iter().collect();
        atlas.asked.extend(frames.iter().map(|(key, _)| key.animation));
        atlas.insert(frames)?;
        atlas.dirty.take();
        Ok(atlas)
    }

    /// Pack more frames into an atlas that already holds some. See
    /// [`LandAtlas::pack_more`].
    pub fn pack_more(
        &mut self,
        frames: impl IntoIterator<Item = (FrameKey, AnimFrame)>,
    ) -> Result<(), AtlasError> {
        let frames: Vec<(FrameKey, AnimFrame)> = frames.into_iter().collect();
        self.asked.extend(frames.iter().map(|(key, _)| key.animation));
        self.insert(frames)
    }

    /// Shelve frames beside what is already packed, marking the rows written.
    ///
    /// Tallest first within one growth, for the reason
    /// [`StaticAtlas::insert`] is.
    fn insert(&mut self, frames: impl IntoIterator<Item = (FrameKey, AnimFrame)>) -> Result<(), AtlasError> {
        let frames: BTreeMap<FrameKey, AnimFrame> = frames.into_iter().collect();
        let wanted = self.frames.len() + frames.len();
        let mut order: Vec<(FrameKey, AnimFrame)> = frames.into_iter().collect();
        order.sort_by_key(|(_, frame)| std::cmp::Reverse(frame.image.height()));

        for (key, frame) in order {
            if self.frames.contains_key(&key) {
                continue;
            }
            let image = &frame.image;
            let (width, height) = (image.width(), image.height());
            if u32::from(width) > ATLAS_SIDE || u32::from(height) > ATLAS_SIDE {
                return Err(AtlasError::Oversized {
                    // Reported as the body, which is the only part of the key
                    // a `Graphic` can carry and the part worth naming.
                    graphic: key.animation.body,
                    width,
                    height,
                });
            }
            let Some((origin_x, origin_y)) = self.shelf.take(u32::from(width), u32::from(height)) else {
                return Err(AtlasError::Full {
                    wanted,
                    capacity: self.frames.len(),
                });
            };
            self.dirty.mark(origin_y, u32::from(height));

            copy_sprite(&mut self.pixels, image, origin_x, origin_y);
            self.frames.insert(
                key,
                PackedFrame {
                    sprite: Sprite {
                        region: region_at(origin_x, origin_y, width, height),
                        width,
                        height,
                        // A body is not a wall: it stands in the middle of its
                        // tile and turns, and no edge of the cell is its own.
                        facing: None,
                    },
                    center_x: frame.center_x,
                    center_y: frame.center_y,
                    origin: (origin_x, origin_y),
                },
            );
        }

        Ok(())
    }

    /// The atlas texture's side in pixels. Square, like every other atlas here.
    pub const fn side() -> u32 {
        ATLAS_SIDE
    }

    /// Its pixels, RGBA8 and row-major, ready for `write_texture`.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// How many frames landed in it.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether nothing did.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// One frame, or `None` if it was never packed.
    pub fn frame(&self, key: FrameKey) -> Option<PackedFrame> {
        self.frames.get(&key).copied()
    }

    /// Whether the pixel at `(x, y)` *within* a frame's own picture is drawn
    /// rather than transparent — [`StaticAtlas::opaque_at`]'s question, asked of
    /// an animation frame.
    ///
    /// Same reason and same answer: a creature's frame is a tall rectangle with
    /// a thin body in it, and a box test picks whatever the cursor is merely
    /// *inside* — the empty air beside a dragon's wing, or the gap between a
    /// rider's legs. `x` is in the frame's own left-to-right pixels, so a
    /// mirrored facing is the caller's to undo before asking: the atlas holds
    /// one picture for both, and the flip lives where the quad is built.
    ///
    /// `false` for a frame that is not packed and for a coordinate outside the
    /// picture.
    pub fn opaque_at(&self, key: FrameKey, x: u16, y: u16) -> bool {
        let Some(packed) = self.frames.get(&key) else {
            return false;
        };
        if x >= packed.sprite.width || y >= packed.sprite.height {
            return false;
        }
        let side = ATLAS_SIDE as usize;
        let (origin_x, origin_y) = packed.origin;
        let at = ((origin_y as usize + usize::from(y)) * side + origin_x as usize + usize::from(x)) * 4;
        self.pixels[at + 3] != 0
    }

    /// How many frames a body's animation has, as packed.
    ///
    /// What a caller needs to advance one: the count is the animation's, not a
    /// constant, and asking the atlas rather than remembering it is what keeps
    /// "frame 7 of a 6-frame walk" from being expressible.
    pub fn frame_count(&self, animation: AnimationKey) -> AnimationFrameCount {
        let first = FrameKey::new(animation, AnimationFrameIndex(0));
        let last = FrameKey::new(animation, AnimationFrameIndex(u16::MAX));
        AnimationFrameCount(self.frames.range(first..=last).count() as u16)
    }
}

/// One glyph's slot: `fonts.mul`'s own face and character, not a `Graphic`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct GlyphKey {
    /// Which of the ten faces.
    pub font: Font,
    /// The character, direct — `fonts.mul` is a 224-entry table starting at
    /// code point 0, so this is the byte itself and not an index into
    /// anything.
    pub char: u8,
}

/// `fonts.mul`'s glyphs, packed into one texture.
///
/// A fixed grid like [`LandAtlas`], not a shelf like [`StaticAtlas`]: a glyph
/// is a few pixels and a bin packer's bookkeeping would cost more than the
/// waste it saves. Unlike the land grid, the cell size is not the format's
/// constant — a glyph's own three-byte header says its size, and the biggest
/// one packed decides the cell every other glyph sits inside, corner to
/// corner. Keyed by [`GlyphKey`] rather than the land atlas's `Graphic`,
/// because a character is not a graphic and the two id spaces have nothing to
/// do with each other.
pub struct FontAtlas {
    cell: u32,
    sprites: BTreeMap<GlyphKey, Sprite>,
    /// `ATLAS_SIDE * ATLAS_SIDE` RGBA8 pixels, row-major.
    pixels: Vec<u8>,
}

impl fmt::Debug for FontAtlas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FontAtlas")
            .field("glyphs", &self.sprites.len())
            .field("cell", &self.cell)
            .field("side", &ATLAS_SIDE)
            .finish()
    }
}

impl FontAtlas {
    /// Pack every glyph every face defines.
    ///
    /// Unlike the other atlases, nothing here is asked for by what is on
    /// screen: a speech line can hold any character `fonts.mul` defines, so
    /// there is no "visible set" to build for and the whole file — ten faces
    /// of 224 glyphs each, all of them a few pixels — is packed once and kept.
    /// A control-code glyph, zero-sized in a shipped file, is skipped: it has
    /// nothing to draw and nothing to pack.
    pub fn build(fonts: &AsciiFonts) -> Result<Self, AtlasError> {
        let mut images = Vec::new();
        for font in 0..FONT_COUNT as u16 {
            for char in 0..=u8::MAX {
                let Some(image) = fonts.glyph(Font(font), char) else {
                    continue;
                };
                if image.width() == 0 || image.height() == 0 {
                    continue;
                }
                images.push((
                    GlyphKey {
                        font: Font(font),
                        char,
                    },
                    image.clone(),
                ));
            }
        }
        Self::pack(images)
    }

    /// Pack glyphs somebody else decoded.
    ///
    /// The way in that needs no client install, exactly as
    /// [`StaticAtlas::pack`] is.
    pub fn pack(images: impl IntoIterator<Item = (GlyphKey, Image)>) -> Result<Self, AtlasError> {
        let images: BTreeMap<GlyphKey, Image> = images.into_iter().collect();
        // Every glyph fits inside a square this large — the tallest and the
        // widest packed, whichever is bigger. A grid needs one number for both
        // axes, or the arithmetic below would have to track two.
        let cell = images
            .values()
            .map(|image| u32::from(image.width()).max(u32::from(image.height())))
            .max()
            .unwrap_or(1)
            .max(1);
        let cells_per_row = ATLAS_SIDE / cell;
        let capacity = (cells_per_row * cells_per_row) as usize;
        if images.len() > capacity {
            return Err(AtlasError::Full {
                wanted: images.len(),
                capacity,
            });
        }

        let side = ATLAS_SIDE as usize;
        let mut pixels = vec![0u8; side * side * 4];
        let mut sprites = BTreeMap::new();

        for (slot, (key, image)) in images.into_iter().enumerate() {
            let (width, height) = (image.width(), image.height());
            let origin_x = (slot as u32 % cells_per_row) * cell;
            let origin_y = (slot as u32 / cells_per_row) * cell;
            copy_sprite(&mut pixels, &image, origin_x, origin_y);
            sprites.insert(
                key,
                Sprite {
                    region: region_at(origin_x, origin_y, width, height),
                    width,
                    height,
                    facing: None,
                },
            );
        }

        Ok(Self {
            cell,
            sprites,
            pixels,
        })
    }

    /// The atlas texture's side in pixels. Square, like every other atlas here.
    pub const fn side() -> u32 {
        ATLAS_SIDE
    }

    /// Its pixels, RGBA8 and row-major, ready for `write_texture`.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// How many glyphs landed in it.
    pub fn len(&self) -> usize {
        self.sprites.len()
    }

    /// Whether nothing did.
    pub fn is_empty(&self) -> bool {
        self.sprites.is_empty()
    }

    /// Where a character sits and how big it is, or `None` if that face or
    /// character was never packed — a zero-sized control code, or a byte past
    /// `fonts.mul`'s 224-entry table.
    pub fn glyph(&self, font: Font, char: u8) -> Option<Sprite> {
        self.sprites.get(&GlyphKey { font, char }).copied()
    }
}

/// How big a line of TrueType text is drawn: a real size in pixels, and a
/// fractional one.
///
/// **Not a factor.** `fontdue` rasterizes an outline at whatever pixel height
/// it is asked for, analytically, so there is nothing here that has to land on
/// a whole number and nothing that multiplies a finished quad — see
/// `docs/text_sizes.md`, whose whole subject this type is. A caller says
/// eleven pixels and gets eleven pixels.
///
/// Ordered and hashed by the bits of a **finite, positive** `f32`, which
/// [`TextSize::new`] is what guarantees: for those, the IEEE bit pattern
/// compares in the same order the number does, so a `BTreeMap` keyed by this
/// is keyed by size in the ordinary sense. That is the whole reason the
/// clamping constructor is the only way in.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TextSize(u32);

impl TextSize {
    /// Smaller than this is not text any more — the smallest of `fonts.mul`'s
    /// own faces is eight pixels tall, and a face rasterized below that has
    /// glyphs that are one or two rows of grey.
    pub const MIN: f32 = 6.0;
    /// Bigger than this is a title card rather than a client's text, and the
    /// atlas is 2048 on a side: a full alphabet at 96 pixels already asks for
    /// a fair share of it, and several sizes have to live there at once.
    pub const MAX: f32 = 96.0;

    /// Clamp into the range and keep it. Takes anything, including what a
    /// hand-edited file offers — `NaN` lands on [`TextSize::MIN`] rather than
    /// poisoning every comparison this is the key of.
    #[must_use]
    pub fn new(pixels: f32) -> Self {
        let clamped = match pixels.is_nan() {
            true => Self::MIN,
            false => pixels.clamp(Self::MIN, Self::MAX),
        };
        Self(clamped.to_bits())
    }

    /// The size itself, in pixels.
    #[must_use]
    pub fn pixels(self) -> f32 {
        f32::from_bits(self.0)
    }

    /// This size on a display of `density` — a dense screen wants the *glyph*
    /// rasterized bigger, not the quad stretched.
    ///
    /// The one place a size is multiplied by anything, and it is
    /// `docs/text_sizes.md`'s D4: the product is what reaches the rasterizer,
    /// so what comes out is a real glyph at the real size rather than a
    /// smaller one enlarged. `winit`'s `scale_factor` is one such density; a
    /// window's own magnification is another, and a caption inside a magnified
    /// window passes both.
    #[must_use]
    pub fn scaled(self, density: f32) -> Self {
        Self::new(self.pixels() * density)
    }
}

/// One rasterized TrueType glyph, packed and ready to place.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TtfSprite {
    /// Where it sits, and how big it is. A glyph with no ink — a space — packs
    /// to a zero-sized [`Sprite`] with nothing copied into the texture, the
    /// way [`SpriteQuad`](crate::sprite::SpriteQuad) already treats a zero
    /// width or height as nothing to draw.
    pub sprite: Sprite,
    /// How far below the glyph's own top edge the baseline sits, in pixels.
    /// See [`openshard_uofiles::ttf_font::TtfGlyph::baseline_from_top`].
    pub baseline_from_top: i32,
    /// How far to move the pen afterwards, in pixels.
    pub advance: u16,
}

/// A TrueType face's glyphs, packed into one texture and grown on demand.
///
/// Unlike [`FontAtlas`], there is no fixed table to pack once: a TrueType face
/// answers for any Unicode code point, so building "every glyph" up front has
/// no upper bound. This is a shelf instead — the same packer
/// [`StaticAtlas`] uses — keyed by the character itself and grown the first
/// time each one is asked for, the way [`StaticAtlas::add`] grows for graphics
/// newly on screen.
///
/// A space — real ink-free glyphs generally — is still inserted, with a
/// zero-sized [`Sprite`] and a real [`TtfSprite::advance`]: unlike
/// [`FontAtlas`], which skips a zero-sized glyph entirely (see
/// [`FontAtlas::build`]) because `fonts.mul` gives it no way to tell "no ink"
/// from "not packed", the caller can tell the two apart here, and
/// `crate::text::collect_ttf` needs the advance to land the next character in
/// the right place — see its doc for what happens to a byte this atlas never
/// packed.
pub struct TtfAtlas {
    /// Every glyph packed so far, keyed by the character **and the size it was
    /// rasterized at**.
    ///
    /// One face, many sizes: `openshard_uofiles::ttf_font`'s "One face, not
    /// ten" note is about *faces* — `fonts.mul` has ten of them and a
    /// TrueType file is one — and says nothing about size, which an outline
    /// answers for continuously. Keying by the pair is what lets a pile's
    /// count be smaller than a spoken line without a second texture, a second
    /// bind group and a second pass; see `docs/text_sizes.md`'s D2.
    sprites: BTreeMap<(char, TextSize), TtfSprite>,
    /// Every (character, size) ever asked for, whether or not it drew ink.
    /// Same purpose as the other atlases' `asked` sets: a character with no
    /// glyph — impossible for a TrueType face, which always has `.notdef` —
    /// would otherwise be rasterized once per frame forever.
    asked: BTreeSet<(char, TextSize)>,
    shelf: Shelf,
    /// `ATLAS_SIDE * ATLAS_SIDE` RGBA8 pixels, row-major.
    pixels: Vec<u8>,
    dirty: Dirty,
}

impl fmt::Debug for TtfAtlas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TtfAtlas")
            .field("glyphs", &self.sprites.len())
            .field("sizes", &self.sizes().count())
            .field("side", &ATLAS_SIDE)
            .finish()
    }
}

impl TtfAtlas {
    /// An atlas holding nothing, ready to be grown into.
    #[must_use]
    pub fn empty() -> Self {
        let side = ATLAS_SIDE as usize;
        Self {
            sprites: BTreeMap::new(),
            asked: BTreeSet::new(),
            shelf: Shelf::default(),
            pixels: vec![0u8; side * side * 4],
            dirty: Dirty::default(),
        }
    }

    /// Every size this atlas currently holds a glyph at, smallest first.
    ///
    /// For a caller deciding whether it has grown too many of them, and for
    /// [`fmt::Debug`]. Nothing draws from it.
    pub fn sizes(&self) -> impl Iterator<Item = TextSize> {
        // Through a set rather than a dedup of the walk: the map is ordered by
        // `(char, size)`, so one size's entries are scattered across every
        // character rather than adjacent. A handful of sizes, so the set costs
        // nothing worth measuring.
        self.sprites
            .keys()
            .map(|&(_, size)| size)
            .collect::<BTreeSet<_>>()
            .into_iter()
    }

    /// Rasterize and pack whichever of `wanted` this atlas has not been
    /// offered at `size` before. [`StaticAtlas::add`], for characters instead
    /// of graphics — and for one size of them, since the same character at two
    /// sizes is two glyphs.
    pub fn add(
        &mut self,
        font: &TtfFont,
        size: TextSize,
        wanted: impl IntoIterator<Item = char>,
    ) -> Result<(), AtlasError> {
        let wanted: BTreeSet<char> = wanted.into_iter().collect();
        let fresh: Vec<char> = wanted
            .into_iter()
            .filter(|&ch| !self.asked.contains(&(ch, size)))
            .collect();
        if fresh.is_empty() {
            return Ok(());
        }
        let glyphs: Vec<((char, TextSize), TtfGlyph)> = fresh
            .iter()
            .map(|&ch| ((ch, size), font.glyph(ch, size.pixels())))
            .collect();
        self.insert(glyphs)?;
        self.asked.extend(fresh.into_iter().map(|ch| (ch, size)));
        Ok(())
    }

    /// [`TtfAtlas::add`], emptying the atlas and trying once more when it is
    /// full.
    ///
    /// The way every caller should grow one, and the reason [`TtfAtlas::reset`]
    /// exists: an atlas keyed by size fills up in ordinary use — a slider
    /// dragged from 8 pixels to 30 asks for twenty-odd alphabets — where one
    /// baked at a single size never could. What it costs when it fires is one
    /// frame of text drawn from stale regions: quads collected *earlier in the
    /// same frame*, at another size, were measured against the old shelves and
    /// the texture underneath them has just been overwritten. A frame of
    /// scrambled captions once in a drag, against a client that stops drawing
    /// text until something changes — the alternative — is the easy trade.
    pub fn add_or_reset(
        &mut self,
        font: &TtfFont,
        size: TextSize,
        wanted: impl IntoIterator<Item = char>,
    ) -> Result<(), AtlasError> {
        let wanted: Vec<char> = wanted.into_iter().collect();
        match self.add(font, size, wanted.iter().copied()) {
            Err(AtlasError::Full { .. }) => {
                self.reset();
                self.add(font, size, wanted)
            }
            other => other,
        }
    }

    /// Empty the atlas, keeping its texture: every glyph goes, and the whole
    /// picture is marked dirty so the upload overwrites what was there.
    ///
    /// The answer to [`AtlasError::Full`], which a sized atlas can reach the
    /// ordinary way a fixed-size one cannot: a slider dragged from 8 pixels to
    /// 30 asks for twenty-odd alphabets, and all but the last of them are dead
    /// the moment the drag moves on. Emptying costs the frame after it a
    /// re-pack of whatever is actually on screen — a few dozen glyphs — which
    /// is what the old re-bake on every size change cost *every* time.
    pub fn reset(&mut self) {
        self.sprites.clear();
        self.asked.clear();
        self.shelf = Shelf::default();
        self.pixels.fill(0);
        self.dirty.mark(0, ATLAS_SIDE);
    }

    /// The rows written since this was last asked, cleared. See
    /// [`LandAtlas::take_dirty`].
    pub fn take_dirty(&mut self) -> Option<std::ops::Range<u32>> {
        self.dirty.take()
    }

    /// Pack glyphs somebody else rasterized.
    ///
    /// The way in that needs no bundled font at all — a test hands this the
    /// glyphs it chose and asserts on the pixels the frame comes back with,
    /// exactly as [`StaticAtlas::pack`] does for graphics.
    pub fn pack(
        glyphs: impl IntoIterator<Item = (char, TtfGlyph)>,
        size: TextSize,
    ) -> Result<Self, AtlasError> {
        let mut atlas = Self::empty();
        let glyphs: Vec<((char, TextSize), TtfGlyph)> = glyphs
            .into_iter()
            .map(|(ch, glyph)| ((ch, size), glyph))
            .collect();
        atlas.asked.extend(glyphs.iter().map(|(key, _)| *key));
        atlas.insert(glyphs)?;
        atlas.dirty.take();
        Ok(atlas)
    }

    /// Shelve glyphs beside what is already packed, marking the rows written.
    ///
    /// Tallest first, for the reason [`StaticAtlas::insert`] is — and a glyph
    /// with no ink sorts to the very end, where its zero height means it never
    /// starts a row for anything else to waste space under.
    fn insert(
        &mut self,
        glyphs: impl IntoIterator<Item = ((char, TextSize), TtfGlyph)>,
    ) -> Result<(), AtlasError> {
        let glyphs: BTreeMap<(char, TextSize), TtfGlyph> = glyphs.into_iter().collect();
        let wanted = self.sprites.len() + glyphs.len();
        let mut order: Vec<((char, TextSize), TtfGlyph)> = glyphs.into_iter().collect();
        order.sort_by_key(|(_, glyph)| std::cmp::Reverse(glyph.image.height()));

        for (key, glyph) in order {
            let ch = key.0;
            if self.sprites.contains_key(&key) {
                continue;
            }
            let (width, height) = (glyph.image.width(), glyph.image.height());
            // No ink — a space, ordinarily. Nothing to shelve or copy, but the
            // advance still has to be kept: see the type doc for why this is
            // inserted rather than left for `glyph()` to answer `None`.
            if width == 0 || height == 0 {
                self.sprites.insert(
                    key,
                    TtfSprite {
                        sprite: Sprite {
                            region: Region {
                                u: 0.0,
                                v: 0.0,
                                du: 0.0,
                                dv: 0.0,
                            },
                            width: 0,
                            height: 0,
                            facing: None,
                        },
                        baseline_from_top: glyph.baseline_from_top,
                        advance: glyph.advance,
                    },
                );
                continue;
            }
            if u32::from(width) > ATLAS_SIDE || u32::from(height) > ATLAS_SIDE {
                return Err(AtlasError::OversizedGlyph {
                    char: ch,
                    width,
                    height,
                });
            }
            let Some((origin_x, origin_y)) = self.shelf.take(u32::from(width), u32::from(height)) else {
                return Err(AtlasError::Full {
                    wanted,
                    capacity: self.sprites.len(),
                });
            };
            self.dirty.mark(origin_y, u32::from(height));
            copy_sprite(&mut self.pixels, &glyph.image, origin_x, origin_y);
            self.sprites.insert(
                key,
                TtfSprite {
                    sprite: Sprite {
                        region: region_at(origin_x, origin_y, width, height),
                        width,
                        height,
                        // A letter is not a thing standing in the street at all.
                        facing: None,
                    },
                    baseline_from_top: glyph.baseline_from_top,
                    advance: glyph.advance,
                },
            );
        }

        Ok(())
    }

    /// The atlas texture's side in pixels. Square, like every other atlas here.
    pub const fn side() -> u32 {
        ATLAS_SIDE
    }

    /// Its pixels, RGBA8 and row-major, ready for `write_texture`.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// How many characters landed in it.
    pub fn len(&self) -> usize {
        self.sprites.len()
    }

    /// Whether nothing did.
    pub fn is_empty(&self) -> bool {
        self.sprites.is_empty()
    }

    /// A character's packed glyph **at one size**, or `None` if that pair was
    /// never asked for.
    ///
    /// The size is part of the question, not a property of the atlas: the same
    /// character at eleven pixels and at sixteen is two glyphs, and answering
    /// with whichever one happens to be packed would draw a pile's count in a
    /// spoken line's size the moment somebody had spoken.
    pub fn glyph(&self, ch: char, size: TextSize) -> Option<TtfSprite> {
        self.sprites.get(&(ch, size)).copied()
    }
}

/// Copy a whole picture into an atlas, alpha from the file's own zeroes.
///
/// Shared by the two irregular atlases because they mean the same thing by a
/// transparent pixel: absent. Ground is the exception — there a zero is black —
/// and that copy stays in [`LandAtlas::pack`] where the diamond's shape is.
fn copy_sprite(pixels: &mut [u8], image: &Image, origin_x: u32, origin_y: u32) {
    let side = ATLAS_SIDE as usize;
    for y in 0..image.height() {
        for x in 0..image.width() {
            let color = image.pixel(x, y).expect("inside the image");
            let at = ((origin_y + u32::from(y)) as usize * side + (origin_x + u32::from(x)) as usize) * 4;
            let Rgb8 { red, green, blue } = color.rgb8();
            pixels[at] = red;
            pixels[at + 1] = green;
            pixels[at + 2] = blue;
            pixels[at + 3] = if color.is_transparent() { 0 } else { u8::MAX };
        }
    }
}

/// The region a picture packed at a pixel origin occupies.
fn region_at(origin_x: u32, origin_y: u32, width: u16, height: u16) -> Region {
    let atlas = ATLAS_SIDE as f32;
    Region {
        u: origin_x as f32 / atlas,
        v: origin_y as f32 / atlas,
        du: f32::from(width) / atlas,
        dv: f32::from(height) / atlas,
    }
}

/// A shelf packer: rows of sprites, each row as tall as its tallest member.
///
/// Deliberately not a general bin packer. Fed tallest-first — which
/// [`StaticAtlas::pack`] guarantees — a shelf's waste is bounded by the height
/// difference *within* a row, and sorted input keeps that small. A better
/// packer would buy a few percent of one texture and cost a data structure
/// nobody can check by hand.
#[derive(Clone, Default)]
struct Shelf {
    /// Where the current row starts, from the top of the atlas.
    top: u32,
    /// How far along the current row is filled.
    used: u32,
    /// How tall the current row is, which the next one starts below.
    height: u32,
}

impl Shelf {
    /// Take a `width` x `height` box, or `None` when the atlas is full.
    fn take(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        if self.used + width > ATLAS_SIDE {
            // The row is full: drop to the next one. Its height is this row's,
            // which is the tallest sprite that landed in it.
            self.top += self.height;
            self.used = 0;
            self.height = 0;
        }
        if self.top + height > ATLAS_SIDE {
            return None;
        }
        let at = (self.used, self.top);
        self.used += width;
        // Tallest-first means this is only ever set by the row's first sprite,
        // but a caller that fed us unsorted input would still get a correct
        // atlas rather than an overlapping one.
        self.height = self.height.max(height);
        Some(at)
    }
}

/// Which cells of the texture atlas are spoken for.
///
/// A first-fit scan rather than a running index, because the two sizes cannot
/// share one: a 128 needs four cells that form a square, and after one of those
/// the next free *cell* and the next free *block* are different places.
struct CellGrid {
    taken: Vec<bool>,
}

impl CellGrid {
    fn new() -> Self {
        Self {
            taken: vec![false; TEXMAP_CELLS],
        }
    }

    /// Take the first free `span` x `span` block, top-left first, or `None` when
    /// the atlas is full.
    fn take(&mut self, span: u32) -> Option<(u32, u32)> {
        let per_row = TEXMAP_CELLS_PER_ROW;
        for y in 0..per_row.saturating_sub(span - 1) {
            'block: for x in 0..per_row.saturating_sub(span - 1) {
                for dy in 0..span {
                    for dx in 0..span {
                        if self.taken[((y + dy) * per_row + x + dx) as usize] {
                            continue 'block;
                        }
                    }
                }
                for dy in 0..span {
                    for dx in 0..span {
                        self.taken[((y + dy) * per_row + x + dx) as usize] = true;
                    }
                }
                return Some((x, y));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use openshard_uofiles::color::Color16;

    use super::*;

    #[test]
    fn slots_fill_a_row_before_starting_the_next() {
        assert_eq!(slot_origin(0), (0, 0));
        assert_eq!(slot_origin(1), (44, 0));
        assert_eq!(slot_origin(SLOTS_PER_ROW - 1), (44 * (SLOTS_PER_ROW - 1), 0));
        assert_eq!(slot_origin(SLOTS_PER_ROW), (0, 44));
    }

    /// The last slot has to end inside the texture. Off by one here is a sprite
    /// wrapping to the far side of the atlas, which looks like corrupt art.
    #[test]
    fn the_last_slot_fits() {
        let (x, y) = slot_origin(CAPACITY as u32 - 1);
        assert!(x + LAND_TILE_SIZE as u32 <= ATLAS_SIDE);
        assert!(y + LAND_TILE_SIZE as u32 <= ATLAS_SIDE);
    }

    /// A square of one colour, at a side the texture atlas accepts.
    fn texture(side: u16, color: Color16) -> Image {
        Image::new(side, side, vec![color; usize::from(side) * usize::from(side)])
    }

    /// The whole reason the texture atlas is not the land atlas: two sizes in
    /// one grid, and a big one must not be cut in half by a small one that was
    /// packed into the middle of it.
    #[test]
    fn a_large_texture_gets_a_whole_block_and_a_small_one_stays_out_of_it() {
        let atlas = TexmapAtlas::pack([
            (Graphic(1), texture(64, Color16(0x1F))),
            (Graphic(2), texture(128, Color16(0x7C00))),
        ])
        .expect("two textures fit");

        let big = atlas.region(Graphic(2)).expect("packed");
        let small = atlas.region(Graphic(1)).expect("packed");
        let atlas_side = ATLAS_SIDE as f32;
        // A texture of `n` pixels spans the centres of `n` texels, which is
        // `n - 1` apart: the half-texel inset, stated in pixels.
        assert_eq!(
            big.du * atlas_side,
            127.0,
            "a 128 texture spans 128 texel centres"
        );
        assert_eq!(small.du * atlas_side, 63.0);

        // Largest first, so the 128 owns the corner and the 64 is beside it
        // rather than inside it.
        assert_eq!((big.u * atlas_side, big.v * atlas_side), (0.5, 0.5));
        assert!(
            small.u * atlas_side >= 128.0 || small.v * atlas_side >= 128.0,
            "the 64 landed at ({}, {}), inside the block the 128 took",
            small.u * atlas_side,
            small.v * atlas_side,
        );
    }

    /// The inset samples the texture's own first and last texel, and nothing
    /// beyond them. Without it a quad's far corners sample the neighbour packed
    /// next door — a one-texel fringe along two edges of every sloped tile,
    /// which reads as terrain and is somebody else's.
    #[test]
    fn a_regions_corners_sample_the_first_and_last_texel_of_its_own_texture() {
        let atlas = TexmapAtlas::pack([
            (Graphic(1), texture(64, Color16(1))),
            (Graphic(2), texture(64, Color16(2))),
        ])
        .expect("two textures fit");
        let region = atlas.region(Graphic(1)).expect("packed");
        let side = ATLAS_SIDE as f32;

        // What the shader computes at the quad's two extreme corners, in texels.
        let first = region.u * side;
        let last = (region.u + region.du) * side;
        assert_eq!(first.floor(), 0.0);
        assert_eq!(last.floor(), 63.0, "the far corner is not the neighbour's texel");
    }

    /// Every region has to be inside the texture and disjoint from every other,
    /// which the grid gives by construction — and which one wrong bound in
    /// `CellGrid::take` would take away without changing anything visible until
    /// two terrains started sharing a texel.
    #[test]
    fn packed_textures_never_overlap_and_never_leave_the_atlas() {
        // Enough of both sizes to fill several rows, and interleaved so the
        // allocator cannot be right by accident of ordering.
        let images: Vec<(Graphic, Image)> = (0..200u16)
            .map(|i| {
                let side = if i % 3 == 0 { 128 } else { 64 };
                (Graphic(i), texture(side, Color16(i | 1)))
            })
            .collect();
        let atlas = TexmapAtlas::pack(images.clone()).expect("200 textures fit in 1024 cells");

        let mut claimed = vec![None; TEXMAP_CELLS];
        for (graphic, image) in images {
            let region = atlas.region(graphic).expect("packed");
            let side = ATLAS_SIDE as f32;
            // Back out the half-texel inset: the texel the region starts on is
            // the pixel the texture was packed at.
            let (x, y) = ((region.u * side) as u32, (region.v * side) as u32);
            assert_eq!(region.du * side + 1.0, f32::from(image.width()));
            assert!(x + u32::from(image.width()) <= ATLAS_SIDE);
            assert!(y + u32::from(image.height()) <= ATLAS_SIDE);
            assert_eq!((x % TEXMAP_CELL, y % TEXMAP_CELL), (0, 0), "off the cell grid");

            for cy in 0..u32::from(image.height()) / TEXMAP_CELL {
                for cx in 0..u32::from(image.width()) / TEXMAP_CELL {
                    let cell =
                        ((y / TEXMAP_CELL + cy) * TEXMAP_CELLS_PER_ROW + x / TEXMAP_CELL + cx) as usize;
                    assert_eq!(claimed[cell], None, "{graphic:?} overlaps {:?}", claimed[cell]);
                    claimed[cell] = Some(graphic);
                }
            }
        }
    }

    /// A rectangle of one colour, for the shelf packer's tests.
    fn sprite(width: u16, height: u16) -> Image {
        Image::new(
            width,
            height,
            vec![Color16(0x1F); usize::from(width) * usize::from(height)],
        )
    }

    /// The property the shelf packer exists to give: every sprite gets its own
    /// pixels. Overlap here is two statics sharing a picture, which reads as
    /// corrupt art rather than as a packing bug.
    #[test]
    fn packed_sprites_never_overlap_and_never_leave_the_atlas() {
        // Sizes that do not divide the atlas evenly, so a row's leftover is
        // never zero and the wrap to the next shelf is exercised.
        let images: Vec<(Graphic, Image)> = (0..300u16)
            .map(|i| (Graphic(i), sprite(30 + i % 7, 20 + i % 11)))
            .collect();
        let atlas = StaticAtlas::pack(images.clone()).expect("300 small sprites fit");

        let side = ATLAS_SIDE as usize;
        let mut claimed = vec![None; side * side];
        for (graphic, image) in images {
            let packed = atlas.sprite(graphic).expect("packed");
            assert_eq!((packed.width, packed.height), (image.width(), image.height()));
            let x = (packed.region.u * ATLAS_SIDE as f32) as usize;
            let y = (packed.region.v * ATLAS_SIDE as f32) as usize;
            assert!(
                x + usize::from(packed.width) <= side,
                "{graphic:?} runs off the right"
            );
            assert!(
                y + usize::from(packed.height) <= side,
                "{graphic:?} runs off the bottom"
            );
            for row in y..y + usize::from(packed.height) {
                for column in x..x + usize::from(packed.width) {
                    let cell = &mut claimed[row * side + column];
                    assert_eq!(*cell, None, "{graphic:?} overlaps {cell:?}");
                    *cell = Some(graphic);
                }
            }
        }
    }

    /// A static's shape is its alpha, and the alpha comes from the file's zero
    /// pixels. The land atlas does the opposite — there a zero is black — so
    /// this is the one place the two rules meet, and getting it backwards
    /// either punches holes through solid art or draws every sprite's bounding
    /// box as a black rectangle.
    #[test]
    fn a_zero_pixel_is_absent_and_everything_else_is_opaque() {
        let mut pixels = vec![Color16(0x7C00); 4];
        pixels[1] = Color16::TRANSPARENT;
        let atlas = StaticAtlas::pack([(Graphic(1), Image::new(2, 2, pixels))]).expect("fits");
        let packed = atlas.sprite(Graphic(1)).expect("packed");
        let x = (packed.region.u * ATLAS_SIDE as f32) as usize;
        let y = (packed.region.v * ATLAS_SIDE as f32) as usize;
        let alpha = |column: usize, row: usize| {
            atlas.pixels()[((y + row) * ATLAS_SIDE as usize + x + column) * 4 + 3]
        };
        assert_eq!(alpha(0, 0), u8::MAX);
        assert_eq!(alpha(1, 0), 0, "a zero pixel is the sprite's shape, not a colour");
        assert_eq!(alpha(0, 1), u8::MAX);
    }

    /// Tallest first, or a shelf wastes the difference under every tall sprite
    /// that lands beside a short one. Stated as "the tall one is on the first
    /// row", which is the observable consequence.
    #[test]
    fn the_tallest_sprite_starts_the_first_shelf() {
        let atlas = StaticAtlas::pack([
            (Graphic(1), sprite(40, 20)),
            (Graphic(2), sprite(40, 200)),
            (Graphic(3), sprite(40, 60)),
        ])
        .expect("three sprites fit");
        assert_eq!(atlas.sprite(Graphic(2)).expect("packed").region.v, 0.0);
        // And the shorter two share that row rather than starting their own.
        assert_eq!(atlas.sprite(Graphic(3)).expect("packed").region.v, 0.0);
        assert_eq!(atlas.sprite(Graphic(1)).expect("packed").region.v, 0.0);
    }

    /// **The table answers, and the detector is not asked** — the seam
    /// `docs/lighting.md`'s decision 31 arrives through.
    ///
    /// The fixture is a picture the detector reads confidently: a silhouette of
    /// an east face, which `facing::facing_of` names on its own and which every
    /// other test in this workspace relies on it naming. Packed against a table
    /// that says something else, the sprite carries what the *table* said —
    /// including the one row only a person can write, which is that nothing may
    /// be read off this picture at all.
    ///
    /// Both arms matter, and the second is the one a weaker implementation
    /// passes: an atlas that consulted the table and fell back to measuring on a
    /// miss would get the first assertion right and put the detector's answer
    /// back for the second, which is exactly the override a shard writes when the
    /// detector is wrong about its wall.
    #[test]
    fn a_packed_sprite_takes_its_surface_from_the_table() {
        use crate::arttable::{ArtTable, Stamp};
        use crate::facing::{Face, Facing};

        let wall = crate::facing::silhouette(Face::East, 80);
        assert_eq!(
            crate::facing::facing_of(&wall),
            Some(Facing::One(Face::East)),
            "the fixture is one the detector reads, or this test is about nothing",
        );

        let mut table = ArtTable::measured(Stamp {
            art: "artLegacyMUL.uop".to_string(),
            bytes: 1,
            detector: crate::facing::DETECTOR,
        });
        table.author(
            Graphic(1),
            crate::occlusion::Shape::faced(Facing::One(Face::South)),
        );
        table.author(Graphic(2), crate::occlusion::Shape::UNREAD);
        // And a row that says this picture has a window in it, which the picture
        // does not: the hole travels by the same lookup the face does.
        table.author(
            Graphic(3),
            crate::occlusion::Shape {
                facing: Some(Facing::One(Face::East)),
                hole: Some(WINDOW),
                prism: None,
                blocks: crate::facing::Blocks::EMPTY,
                footprint: None,
            },
        );
        // And a fourth that says this picture is a bookcase's own slab —
        // `docs/footprints.md`'s S3: the footprint travels by the same lookup
        // as the hole and the prism, none of it read off this east-facing wall.
        let footprint =
            crate::facing::Footprint::new(crate::facing::Span::new(0, 4), crate::facing::Span::new(3, 8))
                .expect("a narrower box");
        table.author(
            Graphic(4),
            crate::occlusion::Shape {
                facing: None,
                hole: None,
                prism: None,
                blocks: crate::facing::Blocks::EMPTY,
                footprint: Some(footprint),
            },
        );

        let atlas = StaticAtlas::pack_from(
            [
                (Graphic(1), wall.clone()),
                (Graphic(2), wall.clone()),
                (Graphic(3), wall.clone()),
                (Graphic(4), wall),
            ],
            Some(table),
        )
        .expect("four sprites fit");
        assert_eq!(
            atlas.sprite(Graphic(1)).expect("packed").facing,
            Some(Facing::One(Face::South)),
            "the table's row, not the picture's",
        );
        assert_eq!(
            atlas.sprite(Graphic(2)).expect("packed").facing,
            None,
            "a row a person wrote to say nothing may be read off this picture",
        );
        assert_eq!(atlas.hole(Graphic(3)), Some(WINDOW), "the table's hole");
        assert_eq!(atlas.hole(Graphic(1)), None, "and a row with none has none");
        assert_eq!(
            atlas.footprint(Graphic(4)),
            Some(footprint),
            "the table's footprint"
        );
        assert_eq!(
            atlas.footprint(Graphic(1)),
            None,
            "a face already named leaves no footprint to read",
        );
    }

    /// `0x003C`'s hole, which is what the detector reads off the client's own
    /// window — see the sweep in `openshard-client-artscan`.
    const WINDOW: crate::facing::Hole = crate::facing::Hole {
        near: 93,
        far: 185,
        bottom: 10,
        top: 15,
    };

    /// And with no table at all, the picture is measured as it always was —
    /// which is the client decision 31.6 promises: a slow first frame rather
    /// than a shard that will not start.
    ///
    /// **Both measurements**, because they are one answer about one picture and a
    /// fallback that read the face and not the hole would be a client where
    /// windows exist only on machines somebody ran a tool on.
    #[test]
    fn a_packed_sprite_with_no_table_is_measured_as_it_is_packed() {
        use crate::facing::{Face, Facing};

        let atlas = StaticAtlas::pack([
            (Graphic(1), crate::facing::silhouette(Face::East, 80)),
            (Graphic(2), crate::facing::pierced(Face::East, 80, WINDOW)),
        ])
        .expect("two sprites fit");
        assert_eq!(
            atlas.sprite(Graphic(1)).expect("packed").facing,
            Some(Facing::One(Face::East)),
        );
        assert_eq!(atlas.hole(Graphic(1)), None, "a solid wall has no window");
        assert_eq!(
            atlas.sprite(Graphic(2)).expect("packed").facing,
            Some(Facing::One(Face::East)),
            "a wall with a window in it is still a wall",
        );
        assert_eq!(atlas.hole(Graphic(2)), Some(WINDOW));
    }

    /// A sprite bigger than the atlas is its own error, because it is not a
    /// capacity problem: no packing of any kind could place it.
    #[test]
    fn a_sprite_larger_than_the_atlas_says_which_one_it_was() {
        let huge = Image::new(
            1,
            ATLAS_SIDE as u16 + 1,
            vec![Color16(1); ATLAS_SIDE as usize + 1],
        );
        assert!(matches!(
            StaticAtlas::pack([(Graphic(7), huge)]),
            Err(AtlasError::Oversized {
                graphic: Graphic(7),
                ..
            })
        ));
    }

    /// More textures than cells is an error rather than a silent drop: a tile
    /// whose texture quietly vanished is drawn from its art, which looks like
    /// terrain and is the wrong terrain.
    #[test]
    fn an_atlas_that_cannot_hold_them_all_says_so() {
        let images: Vec<(Graphic, Image)> = (0..TEXMAP_CELLS as u16 + 1)
            .map(|i| (Graphic(i), texture(64, Color16(1))))
            .collect();
        assert!(matches!(TexmapAtlas::pack(images), Err(AtlasError::Full { .. })));
    }

    /// A glyph, sized like a real one and coloured so it is not accidentally
    /// transparent. `fonts.mul`'s own pixels are grey, but nothing packed here
    /// reads the colour back — only the size and the region matter.
    fn glyph(width: u16, height: u16, gray: u8) -> Image {
        let word = u16::from(gray) << 10 | u16::from(gray) << 5 | u16::from(gray);
        Image::new(
            width,
            height,
            vec![Color16(word); usize::from(width) * usize::from(height)],
        )
    }

    /// The grid's cell is the biggest glyph packed, and two different sizes
    /// both come back their own size rather than the cell's.
    #[test]
    fn the_cell_is_the_tallest_or_widest_glyph_and_a_small_glyph_keeps_its_own_size() {
        let atlas = FontAtlas::pack([
            (
                GlyphKey {
                    font: Font(0),
                    char: b'A',
                },
                glyph(8, 12, 0x1F),
            ),
            (
                GlyphKey {
                    font: Font(0),
                    char: b'i',
                },
                glyph(3, 12, 0x2F),
            ),
        ])
        .expect("two glyphs fit");
        assert_eq!(atlas.cell, 12);

        let wide = atlas.glyph(Font(0), b'A').expect("packed");
        assert_eq!((wide.width, wide.height), (8, 12));
        let narrow = atlas.glyph(Font(0), b'i').expect("packed");
        assert_eq!((narrow.width, narrow.height), (3, 12));
        // Neither samples the other: the grid gave each its own cell.
        assert_ne!((wide.region.u, wide.region.v), (narrow.region.u, narrow.region.v));
    }

    /// The same character in two different faces is two different glyphs, not
    /// one shared by both — `GlyphKey` carries the face for exactly this.
    #[test]
    fn the_same_character_in_two_faces_is_two_glyphs() {
        let atlas = FontAtlas::pack([
            (
                GlyphKey {
                    font: Font(0),
                    char: b'A',
                },
                glyph(4, 4, 0x10),
            ),
            (
                GlyphKey {
                    font: Font(1),
                    char: b'A',
                },
                glyph(4, 4, 0x20),
            ),
        ])
        .expect("two glyphs fit");
        let face0 = atlas.glyph(Font(0), b'A').expect("packed");
        let face1 = atlas.glyph(Font(1), b'A').expect("packed");
        assert_ne!((face0.region.u, face0.region.v), (face1.region.u, face1.region.v));
    }

    /// A character never packed — the font index is wrong, or the byte was
    /// never in the input — is `None` rather than another glyph's picture.
    #[test]
    fn an_unpacked_character_is_none() {
        let atlas = FontAtlas::pack([(
            GlyphKey {
                font: Font(0),
                char: b'A',
            },
            glyph(4, 4, 0x10),
        )])
        .expect("one glyph fits");
        assert!(atlas.glyph(Font(0), b'B').is_none());
        assert!(atlas.glyph(Font(1), b'A').is_none());
    }

    /// `FontAtlas::build` walks a real [`AsciiFonts`] rather than a hand-built
    /// list, and skips the zero-sized control-code glyphs the way
    /// [`AnimAtlas::build`] skips a blank animation frame.
    #[test]
    fn build_packs_every_real_glyph_and_skips_the_zero_sized_ones() {
        let mut bytes = Vec::new();
        let a_index = usize::from(b'A' - openshard_uofiles::font::GLYPH_BASE);
        for _font in 0..FONT_COUNT {
            bytes.push(0); // the font header byte
            for char in 0..openshard_uofiles::font::CHARS_PER_FONT {
                if char == a_index {
                    bytes.push(2);
                    bytes.push(2);
                    bytes.push(0);
                    bytes.extend_from_slice(&[0xFFu8, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
                } else {
                    // Every other character is a zero-sized control code.
                    bytes.push(0);
                    bytes.push(0);
                    bytes.push(0);
                }
            }
        }
        let fonts = AsciiFonts::parse(&bytes).expect("a whole set of faces");
        let atlas = FontAtlas::build(&fonts).expect("packs");
        // One real glyph per face, and nothing for the zero-sized rest.
        assert_eq!(atlas.len(), FONT_COUNT);
        assert!(atlas.glyph(Font(0), b'A').is_some());
        assert!(atlas.glyph(Font(0), b'B').is_none(), "zero-sized, not packed");
    }

    /// The size every TrueType test below packs and reads at — one of them,
    /// since what those tests are about is the packing rather than the size.
    /// The one that is about two sizes says so.
    fn size() -> TextSize {
        TextSize::new(16.0)
    }

    /// A synthetic rasterized glyph: `width`x`height` of one grey level, the
    /// baseline `baseline_from_top` pixels down from the top, advancing the
    /// pen by `advance`.
    fn ttf_glyph(width: u16, height: u16, baseline_from_top: i32, advance: u16) -> TtfGlyph {
        TtfGlyph {
            image: Image::new(
                width,
                height,
                vec![Color16(0x1F); usize::from(width) * usize::from(height)],
            ),
            baseline_from_top,
            advance,
        }
    }

    /// A packed glyph reports back exactly the advance and baseline it was
    /// rasterized with — `TtfAtlas` only relocates pixels, it does not touch
    /// the placement numbers `crate::text::collect_ttf` depends on.
    #[test]
    fn a_packed_ttf_glyph_keeps_its_advance_and_baseline() {
        let atlas = TtfAtlas::pack([('A', ttf_glyph(10, 14, 11, 12))], size()).expect("one glyph fits");
        let glyph = atlas.glyph('A', size()).expect("packed");
        assert_eq!((glyph.sprite.width, glyph.sprite.height), (10, 14));
        assert_eq!(glyph.baseline_from_top, 11);
        assert_eq!(glyph.advance, 12);
    }

    /// A space has no ink to pack, but a caller still needs to move the pen
    /// past it — the whole reason [`TtfAtlas`] inserts a zero-sized entry
    /// instead of leaving it out the way [`FontAtlas`] leaves out a zero-sized
    /// `fonts.mul` glyph.
    #[test]
    fn a_glyph_with_no_ink_is_still_packed_with_its_advance() {
        let atlas = TtfAtlas::pack([(' ', ttf_glyph(0, 0, 0, 6))], size()).expect("packs");
        let glyph = atlas.glyph(' ', size()).expect("a space is packed, just empty");
        assert_eq!((glyph.sprite.width, glyph.sprite.height), (0, 0));
        assert_eq!(glyph.advance, 6);
    }

    /// A character nobody asked for answers `None`, not a made-up glyph — the
    /// same contract [`FontAtlas::glyph`] and [`StaticAtlas::sprite`] give.
    #[test]
    fn an_unpacked_ttf_character_is_none() {
        let atlas = TtfAtlas::pack([('A', ttf_glyph(10, 14, 11, 12))], size()).expect("packs");
        assert!(atlas.glyph('Z', size()).is_none());
    }

    /// The property every shelf-packed atlas needs: two glyphs never claim the
    /// same pixels, whatever order they were handed in.
    #[test]
    fn packed_ttf_glyphs_never_overlap_and_never_leave_the_atlas() {
        let glyphs: Vec<(char, TtfGlyph)> = ('a'..='z')
            .enumerate()
            .map(|(i, ch)| (ch, ttf_glyph(6 + (i as u16) % 5, 10 + (i as u16) % 7, 8, 8)))
            .collect();
        let atlas = TtfAtlas::pack(glyphs.clone(), size()).expect("26 small glyphs fit");

        let side = ATLAS_SIDE as usize;
        let mut claimed = vec![None; side * side];
        for (ch, glyph) in glyphs {
            let packed = atlas.glyph(ch, size()).expect("packed");
            assert_eq!(
                (packed.sprite.width, packed.sprite.height),
                (glyph.image.width(), glyph.image.height())
            );
            let x = (packed.sprite.region.u * ATLAS_SIDE as f32) as usize;
            let y = (packed.sprite.region.v * ATLAS_SIDE as f32) as usize;
            for row in y..y + usize::from(packed.sprite.height) {
                for column in x..x + usize::from(packed.sprite.width) {
                    let cell = &mut claimed[row * side + column];
                    assert_eq!(*cell, None, "{ch:?} overlaps {cell:?}");
                    *cell = Some(ch);
                }
            }
        }
    }

    /// The same character at two sizes is two glyphs, and each answers with
    /// its own.
    ///
    /// The whole of `docs/text_sizes.md`'s D2, asserted: before the atlas was
    /// keyed by `(char, size)` there was one glyph per character and a second
    /// size had nowhere to go — which is why a pile's count was drawn in a
    /// spoken line's size.
    #[test]
    fn one_character_at_two_sizes_is_two_glyphs() {
        let small = TextSize::new(11.0);
        let large = TextSize::new(22.0);
        let mut atlas = TtfAtlas::pack([('A', ttf_glyph(6, 8, 7, 7))], small).expect("packs");
        atlas
            .insert([(('A', large), ttf_glyph(12, 16, 14, 13))])
            .expect("a second size fits beside the first");

        assert_eq!(atlas.glyph('A', small).expect("packed").advance, 7);
        assert_eq!(atlas.glyph('A', large).expect("packed").advance, 13);
        assert_eq!(
            atlas.glyph('A', TextSize::new(16.0)),
            None,
            "a size nobody packed is not answered with a neighbour's glyph"
        );
        assert_eq!(atlas.sizes().collect::<Vec<_>>(), vec![small, large]);
    }

    /// A size is what a person wrote, clamped only where it stops being text.
    #[test]
    fn a_text_size_is_pixels_and_orders_by_size() {
        assert_eq!(TextSize::new(13.5).pixels(), 13.5);
        assert_eq!(TextSize::new(0.0).pixels(), TextSize::MIN);
        assert_eq!(TextSize::new(4000.0).pixels(), TextSize::MAX);
        // `NaN` cannot be allowed through: this is a `BTreeMap` key, and one
        // `NaN` in an ordered map is a comparison that answers `false` to
        // everything it is asked.
        assert_eq!(TextSize::new(f32::NAN).pixels(), TextSize::MIN);
        // Ordered by size, which is what makes the bit pattern a legitimate
        // key: `sizes()` promises smallest first.
        assert!(TextSize::new(11.0) < TextSize::new(11.5));
        assert!(TextSize::new(11.5) < TextSize::new(96.0));
    }

    /// A density multiplies the size rather than the finished glyph —
    /// `docs/text_sizes.md`'s D4.
    #[test]
    fn a_density_is_folded_into_the_size() {
        assert_eq!(TextSize::new(11.0).scaled(2.0).pixels(), 22.0);
        assert_eq!(TextSize::new(13.0).scaled(1.5).pixels(), 19.5);
    }

    /// A full atlas empties itself and takes the glyphs that would not fit.
    ///
    /// The policy `TtfAtlas::add_or_reset` exists for: an atlas keyed by size
    /// fills up in ordinary use, and a client that answered a full atlas by
    /// drawing no text ever again would be one bad drag from silence.
    #[test]
    fn a_full_atlas_empties_itself_rather_than_refusing() {
        let mut atlas = TtfAtlas::empty();
        // One glyph the size of the whole texture: nothing fits beside it,
        // above it or below it, so the next one asked for is refused.
        let huge = TextSize::new(96.0);
        atlas
            .insert([(('A', huge), ttf_glyph(ATLAS_SIDE as u16, ATLAS_SIDE as u16, 8, 8))])
            .expect("one fits exactly");
        assert!(
            atlas
                .insert([(('B', huge), ttf_glyph(ATLAS_SIDE as u16, ATLAS_SIDE as u16, 8, 8))])
                .is_err(),
            "the test's premise: the atlas is full"
        );

        atlas.reset();
        assert!(atlas.is_empty());
        assert_eq!(atlas.glyph('A', huge), None);
        atlas
            .insert([(('B', huge), ttf_glyph(ATLAS_SIDE as u16, ATLAS_SIDE as u16, 8, 8))])
            .expect("the emptied atlas takes what would not fit before");
        assert!(atlas.glyph('B', huge).is_some());
    }

    /// A real face, read from wherever `OPENSHARD_TTF_FONT_TEST` points.
    /// Skipped, not failed, when it is unset: nothing is bundled with the
    /// engine (see `openshard_uofiles::ttf_font`'s doc), so there is no font
    /// this crate can assume exists on the machine running the test.
    fn test_ttf_font() -> Option<TtfFont> {
        let path = std::env::var_os("OPENSHARD_TTF_FONT_TEST")?;
        Some(TtfFont::open(path).expect("OPENSHARD_TTF_FONT_TEST names a readable TrueType face"))
    }

    /// A character already packed is not rasterized again — the reason
    /// [`TtfAtlas::add`] keeps its own `asked` set rather than calling
    /// [`TtfFont::glyph`] unconditionally, the way [`StaticAtlas::add`] does
    /// for graphics it already offered.
    #[test]
    fn a_character_already_packed_is_not_rasterized_again() {
        let Some(font) = test_ttf_font() else { return };
        let mut atlas = TtfAtlas::empty();
        atlas.add(&font, size(), ['H', 'i']).expect("packs");
        let before = atlas.glyph('H', size()).expect("packed");

        // Asking again, alongside something new, must not disturb 'H' — if it
        // were rasterized and re-inserted it would still look the same here,
        // which is exactly why the real regression this guards is `len()`
        // growing every time a line repeats a letter, not a pixel changing.
        atlas.add(&font, size(), ['H', '!']).expect("packs");
        let after = atlas.glyph('H', size()).expect("still packed");
        assert_eq!(before, after);
        assert_eq!(atlas.len(), 3, "'H', 'i' and '!' — not 'H' twice");
    }
}

/// Growing an atlas rather than rebuilding it, which is what a scroll does
/// several hundred times a minute.
///
/// The tests here are all about the difference between the two, because that is
/// the only thing the change could have broken: what an atlas *holds* is easy to
/// check and what it holds after being grown twice is not.
#[cfg(test)]
mod growth_tests {
    use openshard_uofiles::color::Color16;

    use super::*;

    fn land(color: u16) -> Image {
        Image::new(
            LAND_TILE_SIZE,
            LAND_TILE_SIZE,
            vec![Color16(color); usize::from(LAND_TILE_SIZE) * usize::from(LAND_TILE_SIZE)],
        )
    }

    fn sprite(width: u16, height: u16, color: u16) -> Image {
        Image::new(
            width,
            height,
            vec![Color16(color); usize::from(width) * usize::from(height)],
        )
    }

    /// The property everything downstream rests on: an atlas grown in pieces is
    /// the atlas built in one go, byte for byte.
    ///
    /// Stated on the land grid because the land grid can promise it — slots are
    /// handed out in insertion order — and it is the one atlas a ground quad
    /// samples by a region computed from a slot number. A layout that drifted
    /// between the two paths would draw one terrain with another's picture,
    /// which reads as a seasonal variant rather than as a bug.
    #[test]
    fn a_land_atlas_grown_in_two_steps_is_the_one_built_in_one() {
        let all: Vec<(Graphic, Image)> = (0..8u16).map(|i| (Graphic(i), land(i * 37 + 1))).collect();
        let whole = LandAtlas::pack(all.clone()).expect("eight tiles fit");

        let mut grown = LandAtlas::pack(all[..3].to_vec()).expect("three tiles fit");
        grown.pack_more(all[3..].to_vec()).expect("five more fit");

        assert_eq!(grown.len(), whole.len());
        for (graphic, _) in &all {
            assert_eq!(grown.region(*graphic), whole.region(*graphic), "{graphic:?}");
        }
        assert_eq!(
            grown.pixels(),
            whole.pixels(),
            "the atlases differ in their pixels"
        );
    }

    /// A graphic offered twice is packed once. Not an optimisation: packing it
    /// again would spend a second slot and leave the first region pointing at
    /// pixels nothing samples, so an atlas would fill up at the rate the camera
    /// re-asks rather than at the rate it discovers.
    #[test]
    fn offering_a_graphic_twice_packs_it_once() {
        let mut atlas = LandAtlas::pack([(Graphic(7), land(1))]).expect("one tile fits");
        let before = atlas.region(Graphic(7));
        atlas
            .pack_more([(Graphic(7), land(2)), (Graphic(8), land(3))])
            .expect("one new tile fits");

        assert_eq!(atlas.len(), 2, "the repeat took a slot");
        assert_eq!(atlas.region(Graphic(7)), before, "the repeat moved it");
    }

    /// Only the rows that were written come back, and they are the rows the
    /// sprite actually landed in.
    ///
    /// This is what makes a growth cost a band instead of the 16MB texture, and
    /// getting it *narrow* would be the dangerous direction: an upload short of
    /// what was packed leaves a sprite as whatever was in the texture before,
    /// which on a fresh atlas is transparent — a graphic that silently does not
    /// draw.
    #[test]
    fn the_dirty_band_covers_exactly_the_rows_written() {
        // A row of the land grid is 44 tall and holds `SLOTS_PER_ROW` tiles.
        let first: Vec<(Graphic, Image)> = (0..SLOTS_PER_ROW as u16)
            .map(|i| (Graphic(i), land(i + 1)))
            .collect();
        let mut atlas = LandAtlas::pack(first).expect("one full row fits");
        assert_eq!(
            atlas.take_dirty(),
            None,
            "a freshly built atlas is uploaded whole"
        );

        // The next graphic starts the second row.
        atlas
            .pack_more([(Graphic(1000), land(9))])
            .expect("one more tile fits");
        let tile = LAND_TILE_SIZE as u32;
        assert_eq!(atlas.take_dirty(), Some(tile..tile * 2));
        assert_eq!(atlas.take_dirty(), None, "taking it twice uploads it twice");
    }

    /// Nothing new means nothing to upload, which is the ordinary frame — a
    /// camera standing still must not be sending 16MB to the device.
    #[test]
    fn a_growth_that_adds_nothing_is_not_dirty() {
        let mut atlas = StaticAtlas::pack([(Graphic(3), sprite(10, 10, 1))]).expect("fits");
        atlas.pack_more([(Graphic(3), sprite(10, 10, 2))]).expect("fits");
        assert_eq!(atlas.take_dirty(), None);
    }

    /// The shelf and the cell grid keep their state between growths. Without
    /// it the second growth starts at the top of the atlas again and packs one
    /// sprite over another, which is the one failure here that produces a
    /// *picture* rather than an error.
    #[test]
    fn a_second_growth_does_not_reuse_the_first_one_s_space() {
        let mut atlas = StaticAtlas::pack([(Graphic(1), sprite(40, 40, 1))]).expect("fits");
        atlas.pack_more([(Graphic(2), sprite(40, 40, 2))]).expect("fits");
        atlas.pack_more([(Graphic(3), sprite(40, 40, 3))]).expect("fits");

        let regions: Vec<Region> = [1u16, 2, 3]
            .into_iter()
            .map(|graphic| atlas.sprite(Graphic(graphic)).expect("packed").region)
            .collect();
        for (i, one) in regions.iter().enumerate() {
            for other in &regions[i + 1..] {
                assert_ne!((one.u, one.v), (other.u, other.v), "two sprites at one origin");
            }
        }
    }

    /// A graphic that was offered is never offered again, packed or not.
    ///
    /// The bug this closes had nothing to do with speed: "does the atlas hold
    /// everything on screen" answered *no* for ever when one visible static had
    /// no art, because a graphic with no picture is never packed — so a single
    /// such tile rebuilt every atlas on every frame, for as long as it was in
    /// view. Asking is what terminates, not packing.
    #[test]
    fn a_graphic_with_no_picture_is_still_only_asked_once() {
        let mut atlas = StaticAtlas::pack([(Graphic(1), sprite(8, 8, 1))]).expect("fits");
        // What `add` does for a graphic the art container answered `None` for:
        // it is recorded and no image is packed.
        atlas.asked.insert(Graphic(99));

        assert!(atlas.sprite(Graphic(99)).is_none(), "nothing was packed for it");
        assert!(
            atlas.asked.contains(&Graphic(99)),
            "a graphic with no art has to stay asked, or the question never ends",
        );
    }

    /// Pages split only when an ordered picture cannot fit the active shelf;
    /// after that split, the earlier page is sealed and a later growth can only
    /// touch the newer one. This is the no-eviction invariant Work 4 will draw.
    #[test]
    fn a_sealed_static_page_never_changes_when_a_later_page_grows() {
        // One of these occupies a 2048px-wide, 1025px-tall shelf, leaving no
        // vertical room for a second. The test gets a page boundary without
        // relying on a count-based capacity that shelf packing does not have.
        let tall = |color| sprite(ATLAS_SIDE as u16, 1025, color);
        let mut atlas = StaticAtlasPages::pack_with_limit([(Graphic(1), tall(1)), (Graphic(2), tall(2))], 2)
            .expect("two pages fit under the test limit");
        assert_eq!(atlas.page_count(), 2);
        assert_eq!(
            atlas.sprite(Graphic(1)).expect("first picture").page,
            StaticAtlasPage(0)
        );
        assert_eq!(
            atlas.sprite(Graphic(2)).expect("second picture").page,
            StaticAtlasPage(1)
        );
        let first_page = atlas
            .page(StaticAtlasPage(0))
            .expect("first page")
            .pixels()
            .to_vec();

        atlas
            .pack_more([(Graphic(3), sprite(10, 10, 3))])
            .expect("the active second page still has room");

        assert_eq!(
            atlas
                .page(StaticAtlasPage(0))
                .expect("sealed first page")
                .pixels(),
            first_page,
            "growing page one rewrote the sealed page zero"
        );
        assert_eq!(
            atlas.sprite(Graphic(3)).expect("late picture").page,
            StaticAtlasPage(1)
        );
        assert_eq!(
            atlas.take_dirty(),
            vec![DirtyStaticAtlasPage {
                page: StaticAtlasPage(1),
                rows: 1025..1035,
            }],
            "only the new page's changed rows are uploaded"
        );
    }

    /// The policy is bounded rather than an accidental cache of every graphic
    /// ever walked past. Reaching it preserves the completed pages untouched.
    #[test]
    fn a_static_page_limit_keeps_existing_pages_and_names_the_limit() {
        let tall = |color| sprite(ATLAS_SIDE as u16, 1025, color);
        let mut atlas = StaticAtlasPages::pack_with_limit([(Graphic(1), tall(1)), (Graphic(2), tall(2))], 2)
            .expect("two pages fit under the test limit");
        let first_page = atlas
            .page(StaticAtlasPage(0))
            .expect("first page")
            .pixels()
            .to_vec();

        assert!(matches!(
            atlas.pack_more([(Graphic(3), tall(3))]),
            Err(AtlasError::PageLimit { wanted: 3, limit: 2 })
        ));
        assert_eq!(atlas.page_count(), 2);
        assert!(
            atlas.sprite(Graphic(3)).is_none(),
            "a limited page did not partly pack"
        );
        assert_eq!(
            atlas.page(StaticAtlasPage(0)).expect("first page").pixels(),
            first_page,
            "the limit rewrote an already complete page"
        );
    }
}
