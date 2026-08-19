//! One pixel per tile: the reduction a radar and a facet map are both made of.
//!
//! No GPU, no window, no camera. It is a function of the map and
//! [`RadarColors`], which is what lets the player's radar and `client.md`'s M3b
//! facet map share it — the two differ only in which rectangle of the baked
//! facet texture they show.
//!
//! # The walk is block-major, and that is not a micro-optimisation
//!
//! `Map::statics_at` binary-searches a block per call, and `map.rs:568`'s own
//! doc records what that costs at scale: asked per tile over a frame it was
//! *the largest single phase of the lighting pass*. A radar covering 256 tiles
//! square would ask it sixty-five thousand times.
//!
//! [`Map::statics_in_block`] hands back the whole block as a slice with no
//! search at all, so the walk asks once per block and buckets what it finds —
//! a thousand slice fetches instead of a hundred and thirty thousand searches,
//! for the same answer.
//!
//! # What a tile's colour is
//!
//! Its land tile, overridden by the **highest static standing on it**. Three
//! details that are each a bug if got wrong:
//!
//! - **`Map::statics_at` is not sorted by z.** Its key is `(y, x)` and nothing
//!   else, so "the last one" is not "the highest one" and the comparison is
//!   explicit.
//! - **The comparison against the land is `>=`, not `>`.** A floor tile lies at
//!   the ground's own height, and `>` would draw grass through a marble floor.
//! - **A tile with no colour at all is [`UNKNOWN`], never transparent.** Zero is
//!   how these files spell *absent*, and a transparent radar pixel would punch a
//!   hole through the window it is drawn in rather than reading as unmapped
//!   ground.

use std::collections::BTreeMap;

use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Facet;
use openshard_uofiles::color::Color16;
use openshard_uofiles::map::{BLOCK_SIZE, Map};
use openshard_uofiles::radarcol::RadarColors;

/// What a tile with no colour of its own draws as.
///
/// Deliberately non-zero: `Color16(0)` is *absent* in every one of these files,
/// and a pixel that is absent is a hole. Near-black, so unmapped ground reads as
/// unmapped rather than as a mistake.
pub const UNKNOWN: Color16 = Color16(0x0001);

/// How many tiles a map block is across. Re-exported so a caller sizing a buffer
/// does not reach past this module for it.
pub const BLOCK_TILES: u16 = BLOCK_SIZE as u16;

/// The side of a base radar chunk, in world tiles.
///
/// Sixty-four tiles is eight map blocks.  A chunk therefore never needs to
/// split a block walk, while still being small enough to replace independently
/// when a terrain edit arrives.  Every base chunk has this complete size: the
/// east and south border chunks fill cells beyond the facet with [`UNKNOWN`].
/// That fixed shape is what lets a parent be reduced from exactly four child
/// products without a map-edge special case.
pub const BASE_CHUNK_TILES: u16 = BLOCK_TILES * 8;

/// The number of map blocks along a base chunk edge.
pub const BASE_CHUNK_BLOCKS: u16 = BASE_CHUNK_TILES / BLOCK_TILES;

/// Convert a world tile to the level-zero chunk and local tile that contain it.
///
/// The conversion is floor division and remainder by [`BASE_CHUNK_TILES`]:
/// tile `(64, 0)` is chunk `(1, 0)`, local tile `(0, 0)`, not the final tile
/// of chunk `(0, 0)`.  It deliberately does not clamp to a facet; callers use
/// the same conversion for an out-of-facet request, whose complete chunk
/// carries [`UNKNOWN`] along its east and south borders.
#[must_use]
pub fn world_tile_to_base_chunk(world: (u32, u32)) -> (RadarChunkCoord, (u16, u16)) {
    let side = u32::from(BASE_CHUNK_TILES);
    (
        RadarChunkCoord::new(world.0 / side, world.1 / side),
        ((world.0 % side) as u16, (world.1 % side) as u16),
    )
}

/// One immutable source version of a facet's terrain and statics.
///
/// A revision deliberately cannot be omitted from a [`RadarChunkKey`].  The
/// map/content owner increments it on a mutation; moving a player does not.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct RadarRevision(pub u64);

/// Coordinates of one chunk at one LOD level.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct RadarChunkCoord {
    /// Horizontal chunk coordinate.
    pub x: u32,
    /// Vertical chunk coordinate.
    pub y: u32,
}

impl RadarChunkCoord {
    #[must_use]
    pub const fn new(x: u32, y: u32) -> Self {
        Self { x, y }
    }
}

/// The complete identity of a cached terrain raster.
///
/// At LOD zero, `chunk` addresses [`BASE_CHUNK_TILES`] square world-tile
/// products.  Each higher LOD addresses a product covering twice as much world
/// space in each direction, but still contains the same number of pixels.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct RadarChunkKey {
    facet: Facet,
    lod: u8,
    chunk: RadarChunkCoord,
    revision: RadarRevision,
}

impl RadarChunkKey {
    /// Constructed only by [`RadarCache`], the owner of source revisions.
    ///
    /// Keeping this crate-visible prevents a window or a player marker from
    /// accidentally creating a second, position-keyed terrain cache.
    #[must_use]
    pub(crate) const fn new(facet: Facet, lod: u8, chunk: RadarChunkCoord, revision: RadarRevision) -> Self {
        Self {
            facet,
            lod,
            chunk,
            revision,
        }
    }

    #[must_use]
    pub const fn facet(self) -> Facet {
        self.facet
    }

    #[must_use]
    pub const fn lod(self) -> u8 {
        self.lod
    }

    #[must_use]
    pub const fn chunk(self) -> RadarChunkCoord {
        self.chunk
    }

    #[must_use]
    pub const fn revision(self) -> RadarRevision {
        self.revision
    }

    /// The north-west world tile a base chunk starts at, if it is representable
    /// by the map reader's `u16` coordinates.
    #[must_use]
    pub fn base_origin(self) -> Option<(u16, u16)> {
        if self.lod != 0 {
            return None;
        }
        let x = self.chunk.x.checked_mul(u32::from(BASE_CHUNK_TILES))?;
        let y = self.chunk.y.checked_mul(u32::from(BASE_CHUNK_TILES))?;
        if x > u32::from(u16::MAX) || y > u32::from(u16::MAX) {
            return None;
        }
        Some((x as u16, y as u16))
    }
}

/// An immutable, complete terrain product ready for cache publication.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RadarChunk {
    key: RadarChunkKey,
    pixels: Vec<Color16>,
}

/// The sole owner of ready radar terrain products and their source revisions.
///
/// This belongs with the map/content lifetime, not a minimap window. Closing a
/// window therefore leaves ready chunks intact, while a source revision change
/// makes them unreachable without any player movement or UI event. Production
/// and dirty-parent tracking deliberately arrive in the next cache phase; this
/// type establishes the one authoritative identity they operate on.
#[derive(Debug, Default)]
pub struct RadarCache {
    revisions: BTreeMap<Facet, RadarRevision>,
    ready: BTreeMap<RadarChunkKey, RadarChunk>,
}

impl RadarCache {
    /// The source revision currently authoritative for a facet.
    #[must_use]
    pub fn revision(&self, facet: Facet) -> RadarRevision {
        self.revisions.get(&facet).copied().unwrap_or(RadarRevision(0))
    }

    /// Construct a cache key under the facet's current source revision.
    #[must_use]
    pub fn key(&self, facet: Facet, lod: u8, chunk: RadarChunkCoord) -> RadarChunkKey {
        RadarChunkKey::new(facet, lod, chunk, self.revision(facet))
    }

    /// Adopt a newer revision named by the map/content owner.
    ///
    /// Existing products are retained only as stale, recreatable storage; all
    /// normal lookup is through [`Self::key`] and therefore cannot return one
    /// for the superseded source revision.
    /// Returns `false` for an old or unchanged revision, so a delayed source
    /// notification cannot make an older ready product current again.
    pub fn set_revision(&mut self, facet: Facet, revision: RadarRevision) -> bool {
        if revision <= self.revision(facet) {
            return false;
        }
        self.revisions.insert(facet, revision);
        true
    }

    /// Publish one complete CPU product if it still matches the source.
    ///
    /// A producer that finishes after a terrain/static edit loses this race and
    /// cannot make stale pixels ready again.
    pub fn publish(&mut self, chunk: RadarChunk) -> bool {
        let key = chunk.key();
        if key.revision != self.revision(key.facet) {
            return false;
        }
        self.ready.insert(key, chunk);
        true
    }

    /// The complete product ready for a current cache key.
    #[must_use]
    pub fn get(&self, key: RadarChunkKey) -> Option<&RadarChunk> {
        (key.revision == self.revision(key.facet))
            .then(|| self.ready.get(&key))
            .flatten()
    }

    /// Number of retained complete products, including superseded products
    /// awaiting the cache budget/eviction policy from Phase 2.
    #[must_use]
    pub fn retained_len(&self) -> usize {
        self.ready.len()
    }
}

impl RadarChunk {
    /// Construct a complete fixed-size raster.  A partial chunk is never a
    /// cache value: map borders are represented by [`UNKNOWN`] pixels instead.
    #[must_use]
    pub fn new(key: RadarChunkKey, pixels: Vec<Color16>) -> Option<Self> {
        (pixels.len() == chunk_pixel_count()).then_some(Self { key, pixels })
    }

    #[must_use]
    pub const fn key(&self) -> RadarChunkKey {
        self.key
    }

    #[must_use]
    pub fn pixels(&self) -> &[Color16] {
        &self.pixels
    }
}

/// A terrain sampling request for a minimap draw.
///
/// This is deliberately only a region in world coordinates.  A caller may
/// centre it on a player, but it contains neither a player marker nor a reason
/// to generate or upload terrain; cache selection is solely by chunk key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RadarRegion {
    pub facet: Facet,
    pub lod: u8,
    pub origin: (u32, u32),
    pub extent: (u16, u16),
}

/// Build a level-zero chunk from the authoritative terrain-colour rule.
///
/// `key.lod` must be zero.  Out-of-facet cells, including the fixed-size map
/// border, are [`UNKNOWN`], exactly as [`fill`] specifies.
#[must_use]
pub fn build_base_chunk(map: &Map, colors: &RadarColors, key: RadarChunkKey) -> Option<RadarChunk> {
    let origin = key.base_origin()?;
    let mut pixels = vec![UNKNOWN; chunk_pixel_count()];
    fill(
        map,
        colors,
        origin,
        BASE_CHUNK_TILES,
        BASE_CHUNK_TILES,
        &mut pixels,
    );
    RadarChunk::new(key, pixels)
}

/// Reduce four categorical colours to one categorical parent pixel.
///
/// The most frequent colour wins.  A tie is resolved by the first sample in
/// north-west, north-east, south-west, south-east order.  [`UNKNOWN`] is an
/// ordinary candidate, rather than transparency or black, so an unmapped area
/// remains visibly unmapped at every LOD.  This is intentionally not RGB
/// averaging: radar colours name terrain categories, not light values.
#[must_use]
pub fn reduce_lod_pixel(samples: [Color16; 4]) -> Color16 {
    let mut winner = 0;
    let mut winner_count = 0;
    for candidate in 0..samples.len() {
        let count = samples
            .iter()
            .filter(|&&colour| colour == samples[candidate])
            .count();
        if count > winner_count {
            winner = candidate;
            winner_count = count;
        }
    }
    samples[winner]
}

/// Build one LOD parent from its four complete children.
///
/// Children are ordered north-west, north-east, south-west, south-east.  Their
/// keys must be the direct children of `key`, on the same facet and revision.
/// Refusing a mismatched family prevents a cache from publishing mixed-source
/// terrain after an invalidation.
#[must_use]
pub fn build_lod_parent(key: RadarChunkKey, children: [&RadarChunk; 4]) -> Option<RadarChunk> {
    let child_lod = key.lod.checked_sub(1)?;
    let child_x = key.chunk.x.checked_mul(2)?;
    let child_y = key.chunk.y.checked_mul(2)?;
    let expected = [
        RadarChunkCoord::new(child_x, child_y),
        RadarChunkCoord::new(child_x.checked_add(1)?, child_y),
        RadarChunkCoord::new(child_x, child_y.checked_add(1)?),
        RadarChunkCoord::new(child_x.checked_add(1)?, child_y.checked_add(1)?),
    ];
    if children.iter().zip(expected).any(|(child, chunk)| {
        let child_key = child.key();
        child_key.facet != key.facet
            || child_key.lod != child_lod
            || child_key.chunk != chunk
            || child_key.revision != key.revision
    }) {
        return None;
    }

    let side = usize::from(BASE_CHUNK_TILES);
    let mut pixels = vec![UNKNOWN; chunk_pixel_count()];
    for y in 0..side {
        for x in 0..side {
            let index = |x: usize, y: usize| y * side + x;
            let sample = |x: usize, y: usize| {
                let child = (y / side) * 2 + x / side;
                children[child].pixels[index(x % side, y % side)]
            };
            let source_x = x * 2;
            let source_y = y * 2;
            pixels[index(x, y)] = reduce_lod_pixel([
                sample(source_x, source_y),
                sample(source_x + 1, source_y),
                sample(source_x, source_y + 1),
                sample(source_x + 1, source_y + 1),
            ]);
        }
    }
    RadarChunk::new(key, pixels)
}

const fn chunk_pixel_count() -> usize {
    (BASE_CHUNK_TILES as usize) * (BASE_CHUNK_TILES as usize)
}

/// Bake one colour for every tile in a facet snapshot.
///
/// This is the building block for a radar cache, not a promise that a facet can
/// never change.  Its caller owns the snapshot's revision and uses it to build
/// or rebuild an invalidated LOD level.  Walking only changes which rectangle
/// is sampled; it must not rebuild this raster.  A live marker belongs in a
/// small overlay, not in the cached terrain pixels.
///
/// UO map coordinates and [`fill`] are `u16`; a map outside that format has no
/// representable radar image and returns an empty vector rather than wrapping
/// its dimensions into a different facet.
#[must_use]
pub fn bake(map: &Map, colors: &RadarColors) -> Vec<Color16> {
    let (Ok(width), Ok(height)) = (u16::try_from(map.width()), u16::try_from(map.height())) else {
        return Vec::new();
    };
    let Some(len) = usize::from(width).checked_mul(usize::from(height)) else {
        return Vec::new();
    };
    let mut pixels = vec![UNKNOWN; len];
    fill(map, colors, (0, 0), width, height, &mut pixels);
    pixels
}

/// The colour of one tile.
///
/// The whole rule in one place, so the block walk below and any caller asking
/// about a single tile cannot come to disagree. Off the map is [`UNKNOWN`].
#[must_use]
pub fn tile_color(map: &Map, colors: &RadarColors, x: u16, y: u16) -> Color16 {
    let Some(land) = map.land(x, y) else {
        return UNKNOWN;
    };
    let mut best = colors.land(land.tile);
    let mut best_z = land.z;
    for item in map.statics_at(x, y) {
        if item.z < best_z {
            continue;
        }
        let color = colors.statik(item.tile);
        if color == Color16::TRANSPARENT {
            continue;
        }
        best = color;
        best_z = item.z;
    }
    if best == Color16::TRANSPARENT {
        UNKNOWN
    } else {
        best
    }
}

/// Fill `into` with the colours of a `width` × `height` rectangle of tiles whose
/// north-west corner is `origin`.
///
/// Row-major, `width` per row, so a caller uploading it as a texture needs no
/// stride of its own. `into` must hold `width * height` colours; anything it
/// cannot reach is left alone, which is the safe half of a caller's arithmetic
/// error rather than a panic in a render path.
///
/// See the module header for why this walks blocks rather than tiles.
pub fn fill(
    map: &Map,
    colors: &RadarColors,
    origin: (u16, u16),
    width: u16,
    height: u16,
    into: &mut [Color16],
) {
    let (origin_x, origin_y) = origin;
    // Every tile starts as its land, and the statics are laid over it block by
    // block. Two passes rather than one because the land is a direct lookup and
    // the statics are not: interleaving them would put a binary search back in
    // the inner loop, which is the whole thing this avoids.
    for row in 0..height {
        for column in 0..width {
            let Some(cell) = into.get_mut(usize::from(row) * usize::from(width) + usize::from(column)) else {
                continue;
            };
            let (Some(x), Some(y)) = (origin_x.checked_add(column), origin_y.checked_add(row)) else {
                *cell = UNKNOWN;
                continue;
            };
            *cell = match map.land(x, y) {
                Some(land) => {
                    let color = colors.land(land.tile);
                    if color == Color16::TRANSPARENT {
                        UNKNOWN
                    } else {
                        color
                    }
                }
                None => UNKNOWN,
            };
        }
    }

    // The highest static on each tile, one block at a time. `best_z` starts at
    // the land's own height so a static below the ground is skipped, and a floor
    // *at* it is not — see the module header.
    let mut best_z = vec![i8::MIN; into.len().min(usize::from(width) * usize::from(height))];
    for row in 0..height {
        for column in 0..width {
            let index = usize::from(row) * usize::from(width) + usize::from(column);
            let (Some(z), Some(x), Some(y)) = (
                best_z.get_mut(index),
                origin_x.checked_add(column),
                origin_y.checked_add(row),
            ) else {
                continue;
            };
            if let Some(land) = map.land(x, y) {
                *z = land.z;
            }
        }
    }

    let last_x = origin_x.saturating_add(width.saturating_sub(1));
    let last_y = origin_y.saturating_add(height.saturating_sub(1));
    let (first_block_x, first_block_y) = (origin_x / BLOCK_TILES, origin_y / BLOCK_TILES);
    let (last_block_x, last_block_y) = (last_x / BLOCK_TILES, last_y / BLOCK_TILES);
    for block_x in first_block_x..=last_block_x {
        for block_y in first_block_y..=last_block_y {
            for item in map.statics_in_block(u32::from(block_x), u32::from(block_y)) {
                let (Some(column), Some(row)) = (item.x.checked_sub(origin_x), item.y.checked_sub(origin_y))
                else {
                    continue;
                };
                if column >= width || row >= height {
                    continue;
                }
                let index = usize::from(row) * usize::from(width) + usize::from(column);
                let (Some(cell), Some(z)) = (into.get_mut(index), best_z.get_mut(index)) else {
                    continue;
                };
                if item.z < *z {
                    continue;
                }
                let color = colors.statik(item.tile);
                if color == Color16::TRANSPARENT {
                    continue;
                }
                *cell = color;
                *z = item.z;
            }
        }
    }
}

/// Stamp a marker over a filled buffer, at the tile `column`, `row` in from its
/// north-west corner.
///
/// A cross rather than a pixel, and that is not decoration. At one pixel a tile
/// a single dot is a single pixel — indistinguishable from a lamp post, and
/// invisible against ground of a similar colour. Five pixels in a shape nothing
/// in `radarcol.mul` produces is the smallest thing a person can actually find.
///
/// This is a small bitmap utility for callers that own a transient image.  A
/// [`RadarChunk`] must never be passed here: player and waypoint markers are
/// overlays and do not change cached terrain products.  The arms clip at the
/// edges, so a marker on the first row keeps the pixels that are on the map
/// instead of wrapping to the last.
pub fn mark(into: &mut [Color16], width: u16, height: u16, at: (u16, u16), color: Color16) {
    let (column, row) = at;
    if column >= width || row >= height {
        return;
    }
    let arms = [(0i32, 0i32), (-1, 0), (1, 0), (0, -1), (0, 1)];
    for (dx, dy) in arms {
        let (Ok(x), Ok(y)) = (
            u16::try_from(i32::from(column) + dx),
            u16::try_from(i32::from(row) + dy),
        ) else {
            continue;
        };
        if x >= width || y >= height {
            continue;
        }
        if let Some(cell) = into.get_mut(usize::from(y) * usize::from(width) + usize::from(x)) {
            *cell = color;
        }
    }
}

/// The colour a static of this graphic draws as, or [`UNKNOWN`] when the table
/// has none. Named so a caller drawing a marker over the map uses the same
/// widening as the map itself.
#[must_use]
pub fn static_color(colors: &RadarColors, graphic: Graphic) -> Color16 {
    match colors.statik(graphic) {
        Color16::TRANSPARENT => UNKNOWN,
        color => color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_protocol::wire::Hue;
    use openshard_uofiles::map::{LandCell, LandTile, StaticItem};
    use openshard_uofiles::tiledata::LAND_TILE_COUNT;

    /// Land id 1 is green, land id 2 is blue; static 1 is red, static 2 is
    /// white, static 3 has no colour at all.
    fn colors() -> RadarColors {
        let mut bytes = vec![0u8; LAND_TILE_COUNT * 2];
        bytes[2..4].copy_from_slice(&0x03E0u16.to_le_bytes()); // land 1: green
        bytes[4..6].copy_from_slice(&0x001Fu16.to_le_bytes()); // land 2: blue
        bytes.extend_from_slice(&0u16.to_le_bytes()); // static 0: absent
        bytes.extend_from_slice(&0x7C00u16.to_le_bytes()); // static 1: red
        bytes.extend_from_slice(&0x7FFFu16.to_le_bytes()); // static 2: white
        bytes.extend_from_slice(&0u16.to_le_bytes()); // static 3: absent
        RadarColors::parse(&bytes).expect("a whole table")
    }

    const GREEN: Color16 = Color16(0x03E0);
    const BLUE: Color16 = Color16(0x001F);
    const RED: Color16 = Color16(0x7C00);
    const WHITE: Color16 = Color16(0x7FFF);

    /// A one-block facet, every tile land id 1 at z 0.
    fn a_field() -> Map {
        Map::from_blocks(1, 1, |_, _| LandCell {
            tile: LandTile(1),
            z: 0,
        })
    }

    fn put(map: &mut Map, graphic: u16, x: u16, y: u16, z: i8) {
        map.place_static(StaticItem {
            tile: Graphic(graphic),
            x,
            y,
            z,
            hue: Hue(0),
        });
    }

    #[test]
    fn bake_is_the_whole_facet_once_in_row_major_order() {
        let mut map = a_field();
        put(&mut map, 1, 7, 7, 1);

        let baked = bake(&map, &colors());

        assert_eq!(baked.len(), 64);
        assert_eq!(baked[0], GREEN);
        assert_eq!(baked[63], RED);
    }

    #[test]
    fn world_tiles_use_one_block_aligned_base_chunk_conversion() {
        assert_eq!(BASE_CHUNK_BLOCKS, 8);
        assert_eq!(
            world_tile_to_base_chunk((0, 0)),
            (RadarChunkCoord::new(0, 0), (0, 0))
        );
        assert_eq!(
            world_tile_to_base_chunk((u32::from(BASE_CHUNK_TILES) - 1, 63)),
            (RadarChunkCoord::new(0, 0), (63, 63))
        );
        assert_eq!(
            world_tile_to_base_chunk((u32::from(BASE_CHUNK_TILES), 64)),
            (RadarChunkCoord::new(1, 1), (0, 0))
        );
    }

    #[test]
    fn a_base_chunk_is_block_aligned_and_keeps_the_map_edge_complete() {
        let mut map = a_field();
        put(&mut map, 1, 7, 7, 1);
        let key = RadarChunkKey::new(Facet(0), 0, RadarChunkCoord::new(0, 0), RadarRevision(7));

        let chunk = build_base_chunk(&map, &colors(), key).expect("a level-zero key");
        let mut reference = vec![UNKNOWN; chunk_pixel_count()];
        fill(
            &map,
            &colors(),
            (0, 0),
            BASE_CHUNK_TILES,
            BASE_CHUNK_TILES,
            &mut reference,
        );

        assert_eq!(chunk.key(), key);
        assert_eq!(
            chunk.pixels(),
            reference,
            "the chunk builder is the rectangle walk"
        );
        assert_eq!(chunk.pixels().len(), usize::from(BASE_CHUNK_TILES).pow(2));
        assert_eq!(chunk.pixels()[7 * usize::from(BASE_CHUNK_TILES) + 7], RED);
        assert_eq!(chunk.pixels()[8], UNKNOWN, "the first tile beyond the east edge");
        assert_eq!(
            chunk.pixels()[usize::from(BASE_CHUNK_TILES) * 8],
            UNKNOWN,
            "the first tile beyond the south edge"
        );
    }

    #[test]
    fn a_lod_parent_reduces_its_four_children_without_blending_colours() {
        let revision = RadarRevision(11);
        let child = |x, y, colour| {
            RadarChunk::new(
                RadarChunkKey::new(Facet(0), 0, RadarChunkCoord::new(x, y), revision),
                vec![colour; chunk_pixel_count()],
            )
            .expect("a complete child")
        };
        let northwest = child(0, 0, RED);
        let northeast = child(1, 0, WHITE);
        let southwest = child(0, 1, UNKNOWN);
        let southeast = child(1, 1, BLUE);
        let key = RadarChunkKey::new(Facet(0), 1, RadarChunkCoord::new(0, 0), revision);

        let parent = build_lod_parent(key, [&northwest, &northeast, &southwest, &southeast])
            .expect("four direct children");
        let side = usize::from(BASE_CHUNK_TILES);

        assert_eq!(parent.pixels()[0], RED, "ties prefer the north-west sample");
        assert_eq!(
            parent.pixels()[side - 1],
            WHITE,
            "the north-east child is sampled"
        );
        assert_eq!(parent.pixels()[(side - 1) * side], UNKNOWN, "unknown is retained");
        assert_eq!(
            parent.pixels()[side * side - 1],
            BLUE,
            "the south-east child is sampled"
        );
        assert_eq!(reduce_lod_pixel([RED, WHITE, RED, WHITE]), RED);
    }

    #[test]
    fn lod_reduction_keeps_unknown_and_resolves_all_ties_in_sample_order() {
        assert_eq!(
            reduce_lod_pixel([UNKNOWN, RED, UNKNOWN, WHITE]),
            UNKNOWN,
            "unknown is a categorical map colour, not transparent"
        );
        assert_eq!(
            reduce_lod_pixel([BLUE, RED, WHITE, UNKNOWN]),
            BLUE,
            "four-way ties retain the north-west sample"
        );
    }

    #[test]
    fn cache_keys_and_publication_follow_the_content_revision_not_a_window() {
        let facet = Facet(0);
        let coord = RadarChunkCoord::new(3, 4);
        let mut cache = RadarCache::default();
        let first_key = cache.key(facet, 0, coord);
        let first = RadarChunk::new(first_key, vec![GREEN; chunk_pixel_count()]).expect("a complete chunk");

        assert!(cache.publish(first));
        assert!(cache.get(first_key).is_some());
        assert_eq!(cache.retained_len(), 1);

        assert!(cache.set_revision(facet, RadarRevision(1)));
        let current_key = cache.key(facet, 0, coord);
        assert_ne!(current_key, first_key);
        assert!(cache.get(first_key).is_none(), "old source is not ready");
        assert!(cache.get(current_key).is_none(), "no new product was built yet");

        let late_old =
            RadarChunk::new(first_key, vec![RED; chunk_pixel_count()]).expect("a complete old chunk");
        assert!(!cache.publish(late_old), "an edit rejects a late worker result");
        assert_eq!(cache.retained_len(), 1, "the stale result was not published");
        assert!(
            !cache.set_revision(facet, RadarRevision(0)),
            "a delayed source notification cannot revive the old product"
        );
    }

    #[test]
    fn a_bare_tile_is_its_land() {
        let map = Map::from_blocks(1, 1, |x, _| LandCell {
            tile: LandTile(if x < 4 { 1 } else { 2 }),
            z: 0,
        });
        let colors = colors();

        assert_eq!(tile_color(&map, &colors, 3, 3), GREEN);
        assert_eq!(
            tile_color(&map, &colors, 5, 3),
            BLUE,
            "a different land tile is a different colour, so the land id is the key"
        );
    }

    /// Off the map is unmapped, not transparent — a hole in the window is worse
    /// than a dark tile.
    #[test]
    fn off_the_map_is_unknown() {
        let map = a_field();
        assert_eq!(tile_color(&map, &colors(), 99, 99), UNKNOWN);
    }

    /// **The comparison is `>=`, not `>`.** A floor lies at the ground's own
    /// height, and `>` would draw grass through it.
    #[test]
    fn a_static_at_the_grounds_own_height_covers_it() {
        let mut map = a_field();
        put(&mut map, 1, 2, 2, 0);
        assert_eq!(tile_color(&map, &colors(), 2, 2), RED);
    }

    /// And one below the ground does not.
    #[test]
    fn a_static_under_the_ground_does_not_cover_it() {
        let mut map = a_field();
        put(&mut map, 1, 2, 2, -5);
        assert_eq!(tile_color(&map, &colors(), 2, 2), GREEN);
    }

    /// **`statics_at` is keyed by `(y, x)` and not by z**, so the highest is
    /// picked by comparing rather than by taking the last.
    #[test]
    fn the_highest_static_wins_whatever_order_they_are_in() {
        let mut map = a_field();
        put(&mut map, 2, 4, 4, 20); // white, high
        put(&mut map, 1, 4, 4, 5); // red, low
        assert_eq!(
            tile_color(&map, &colors(), 4, 4),
            WHITE,
            "the lower static was drawn over the higher one"
        );
    }

    /// A static the radar table has no colour for falls through to what is under
    /// it. Zero is *absent* in these files, not black.
    #[test]
    fn a_static_with_no_colour_falls_through() {
        let mut map = a_field();
        put(&mut map, 3, 6, 6, 20);
        assert_eq!(tile_color(&map, &colors(), 6, 6), GREEN);
    }

    /// The rectangle walk agrees with the single-tile rule, tile for tile. They
    /// are two readers of one answer and the block walk is the one that could
    /// drift.
    #[test]
    fn the_block_walk_agrees_with_the_single_tile_rule() {
        let mut map = a_field();
        put(&mut map, 1, 1, 1, 10);
        put(&mut map, 2, 5, 6, 30);
        put(&mut map, 3, 3, 4, 30); // no colour: falls through
        put(&mut map, 1, 7, 0, -1); // below ground: ignored
        let colors = colors();

        let mut pixels = vec![Color16::TRANSPARENT; 64];
        fill(&map, &colors, (0, 0), 8, 8, &mut pixels);

        for y in 0..8u16 {
            for x in 0..8u16 {
                assert_eq!(
                    pixels[usize::from(y) * 8 + usize::from(x)],
                    tile_color(&map, &colors, x, y),
                    "the block walk and the tile rule disagree at ({x}, {y})"
                );
            }
        }
        assert_eq!(pixels[8 + 1], RED, "(1, 1)");
        assert_eq!(pixels[6 * 8 + 5], WHITE, "(5, 6)");
    }

    /// A marker is five pixels, and its arms clip rather than wrap.
    #[test]
    fn a_marker_at_a_corner_keeps_only_the_arms_on_the_map() {
        let mut pixels = vec![Color16::TRANSPARENT; 16];
        mark(&mut pixels, 4, 4, (0, 0), RED);

        assert_eq!(pixels[0], RED, "the centre");
        assert_eq!(pixels[1], RED, "the arm east of it");
        assert_eq!(pixels[4], RED, "and the one south");
        assert_eq!(
            pixels.iter().filter(|&&c| c == RED).count(),
            3,
            "the west and north arms wrapped instead of clipping",
        );
    }

    /// Away from an edge it is the whole cross, and nothing else.
    #[test]
    fn a_marker_in_the_middle_is_a_cross() {
        let mut pixels = vec![Color16::TRANSPARENT; 25];
        mark(&mut pixels, 5, 5, (2, 2), WHITE);

        for index in [12, 11, 13, 7, 17] {
            assert_eq!(pixels[index], WHITE, "pixel {index} is part of the cross");
        }
        assert_eq!(pixels.iter().filter(|&&c| c == WHITE).count(), 5);
    }

    /// Off the buffer entirely is nothing, not a wrapped pixel on the far side.
    #[test]
    fn a_marker_off_the_map_draws_nothing() {
        let mut pixels = vec![Color16::TRANSPARENT; 16];
        mark(&mut pixels, 4, 4, (4, 0), RED);
        mark(&mut pixels, 4, 4, (0, 9), RED);
        assert!(pixels.iter().all(|&c| c == Color16::TRANSPARENT));
    }

    /// A rectangle that runs off the map is filled to its edge and unmapped
    /// past it, rather than short or panicking.
    #[test]
    fn a_rectangle_past_the_edge_is_unmapped_not_missing() {
        let map = a_field();
        let mut pixels = vec![Color16::TRANSPARENT; 16];
        fill(&map, &colors(), (6, 6), 4, 4, &mut pixels);

        assert_eq!(pixels[0], GREEN, "(6, 6) is on the map");
        assert_eq!(pixels[3], UNKNOWN, "(9, 6) is not");
        assert!(
            pixels.iter().all(|&color| color != Color16::TRANSPARENT),
            "a transparent pixel would be a hole in the window"
        );
    }

    /// Nothing is written past the buffer a caller supplied. A render path
    /// should not panic over a caller's arithmetic.
    #[test]
    fn a_buffer_too_small_is_filled_as_far_as_it_goes() {
        let map = a_field();
        let mut pixels = vec![Color16::TRANSPARENT; 4];
        fill(&map, &colors(), (0, 0), 8, 8, &mut pixels);
        assert!(pixels.iter().all(|&color| color == GREEN));
    }
}
