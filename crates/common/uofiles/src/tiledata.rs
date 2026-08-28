//! `tiledata.mul`: the file the two tile tables come off.
//!
//! What the tables *are* is [`openshard_tiles`]: this module is the reader, and
//! everything it knows is about bytes — where a group header sits, how wide a
//! flag word is, and which of the two layouts a file is in. It hands back a
//! [`TileData`] and then has nothing further to say about it.
//!
//! # The format changed and the file does not say so
//!
//! High Seas (7.0.9.0) widened the flags field from 4 bytes to 8. Every offset
//! after it moved. There is no version number, no magic — the only way to tell
//! is arithmetic: only one of the two layouts divides the file exactly. Guessing
//! wrong does not fail loudly; it reads the flags of one tile as the name of
//! another and the world becomes quietly unwalkable.

use std::fmt;
use std::path::{Path, PathBuf};

use openshard_tiles::{
    AnimId, LAND_TILE_COUNT, LandTile, STATIC_TILE_COUNT, StaticTile, TextureId, TileData, TileFlags,
};

/// Tiles per group, in both tables. Each group has a 4-byte header.
const GROUP_SIZE: usize = 32;
/// The header before every group of 32. Unused, but it is on disk.
const GROUP_HEADER: usize = 4;

/// A land entry, pre-High-Seas: `u32` flags, `u16` texture, 20-byte name.
const LAND_ENTRY_OLD: usize = 26;
/// A land entry, High Seas: `u64` flags, `u16` texture, 20-byte name.
const LAND_ENTRY_NEW: usize = 30;
/// A static entry, pre-High-Seas.
const STATIC_ENTRY_OLD: usize = 37;
/// A static entry, High Seas. See [`parse_static`] for the layout.
const STATIC_ENTRY_NEW: usize = 41;

/// Which layout `tiledata.mul` is in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TileDataFormat {
    /// Clients before 7.0.9.0: 4-byte flags.
    Legacy,
    /// Clients since 7.0.9.0: 8-byte flags.
    HighSeas,
}

impl TileDataFormat {
    const fn land_entry(self) -> usize {
        match self {
            Self::Legacy => LAND_ENTRY_OLD,
            Self::HighSeas => LAND_ENTRY_NEW,
        }
    }

    const fn static_entry(self) -> usize {
        match self {
            Self::Legacy => STATIC_ENTRY_OLD,
            Self::HighSeas => STATIC_ENTRY_NEW,
        }
    }

    const fn flag_bytes(self) -> usize {
        match self {
            Self::Legacy => 4,
            Self::HighSeas => 8,
        }
    }

    /// How long the land table is in this layout.
    const fn land_table_len(self) -> usize {
        (LAND_TILE_COUNT / GROUP_SIZE) * (GROUP_HEADER + GROUP_SIZE * self.land_entry())
    }

    /// Whether a file of `size` bytes divides exactly in this layout.
    ///
    /// The whole format detection. Both layouts are checked and exactly one
    /// fits; if neither does, the file is not `tiledata.mul`.
    fn fits(self, size: usize) -> bool {
        let land = self.land_table_len();
        let Some(rest) = size.checked_sub(land) else {
            return false;
        };
        let group = GROUP_HEADER + GROUP_SIZE * self.static_entry();
        rest > 0 && rest % group == 0
    }
}

/// `tiledata.mul` could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum TileDataError {
    /// The file could not be read.
    Read {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// Neither layout divides the file exactly, so it is not `tiledata.mul`.
    UnknownFormat {
        /// Which file.
        path: PathBuf,
        /// How big it is.
        size: usize,
    },
}

impl fmt::Display for TileDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::UnknownFormat { path, size } => write!(
                f,
                "{} is {size} bytes, which is neither tiledata layout; it is not tiledata.mul",
                path.display()
            ),
        }
    }
}

impl std::error::Error for TileDataError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::UnknownFormat { .. } => None,
        }
    }
}

/// What one read of `tiledata.mul` yields.
///
/// The table, and which of the two layouts the file turned out to be in. The
/// format is here rather than on [`TileData`] because it is a fact about the
/// *file* and not about a tile: a table built by hand — a test fixture, a shard
/// with no client install — has no layout, and the only caller that wants this
/// one writes it into the boot log.
#[derive(Debug)]
pub struct Reading {
    /// Every tile definition the file defined.
    pub tiles: TileData,
    /// The layout it was in.
    pub format: TileDataFormat,
}

/// Read `tiledata.mul`, working out its layout from its size.
pub fn load(path: impl AsRef<Path>) -> Result<Reading, TileDataError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|source| TileDataError::Read {
        path: path.to_owned(),
        source,
    })?;
    parse(&bytes).ok_or_else(|| TileDataError::UnknownFormat {
        path: path.to_owned(),
        size: bytes.len(),
    })
}

/// Read `tiledata.mul` when the file layout is not needed afterwards.
///
/// Most consumers need the domain table only.  Keeping that common path here
/// means callers do not need to know that the decoder also records a
/// file-format diagnostic in [`Reading`].  Code that reports or otherwise
/// acts on the detected layout should use [`load`] instead.
pub fn load_tiles(path: impl AsRef<Path>) -> Result<TileData, TileDataError> {
    load(path).map(|reading| reading.tiles)
}

/// Parse bytes that are already in memory.
#[must_use]
pub fn parse(bytes: &[u8]) -> Option<Reading> {
    // Newest first. Both are checked rather than assumed, and only one can
    // divide the file exactly.
    let format = [TileDataFormat::HighSeas, TileDataFormat::Legacy]
        .into_iter()
        .find(|format| format.fits(bytes.len()))?;

    let mut land = Vec::with_capacity(LAND_TILE_COUNT);
    for index in 0..LAND_TILE_COUNT {
        land.push(parse_land(bytes, format, index)?);
    }

    // The static table runs to the end of the file. Modern tiledata has
    // 0x10000 entries, but older files stop short — read what is there and
    // pad, so a lookup never panics on a tile this client has not heard of.
    let mut statics = Vec::with_capacity(STATIC_TILE_COUNT);
    for index in 0..STATIC_TILE_COUNT {
        match parse_static(bytes, format, index) {
            Some(tile) => statics.push(tile),
            None => break,
        }
    }
    statics.resize(STATIC_TILE_COUNT, StaticTile::default());

    Some(Reading {
        tiles: TileData::from_tables(land, statics),
        format,
    })
}

fn parse_land(bytes: &[u8], format: TileDataFormat, index: usize) -> Option<LandTile> {
    let entry = format.land_entry();
    let offset = (index / GROUP_SIZE) * (GROUP_HEADER + GROUP_SIZE * entry)
        + GROUP_HEADER
        + (index % GROUP_SIZE) * entry;
    let raw = bytes.get(offset..offset + entry)?;

    let flags = read_flags(raw, format);
    // flags, then a u16 texture id, then the name.
    let texture_at = format.flag_bytes();
    let name_at = texture_at + 2;
    Some(LandTile {
        flags,
        texture: TextureId(u16::from_le_bytes([raw[texture_at], raw[texture_at + 1]])),
        name: read_name(&raw[name_at..]),
    })
}

/// Parse a static entry.
///
/// The layout, from Sphere's `CUOItemTypeRec_HS`:
///
/// ```text
///   0  flags       u64 (u32 before High Seas)
///   8  weight      u8      255 = immovable
///   9  layer       u8
///  10  unknown     u32
///  14  animation   u16
///  16  hue         u16
///  18  light       u16
///  20  height      u8
///  21  name        20 bytes
/// ```
///
/// Height at 20 and name at 21 — one byte out and the height byte appears
/// as the first character of the name, which is exactly how you notice.
fn parse_static(bytes: &[u8], format: TileDataFormat, index: usize) -> Option<StaticTile> {
    let entry = format.static_entry();
    let base = format.land_table_len();
    let offset = base
        + (index / GROUP_SIZE) * (GROUP_HEADER + GROUP_SIZE * entry)
        + GROUP_HEADER
        + (index % GROUP_SIZE) * entry;
    let raw = bytes.get(offset..offset + entry)?;

    let flags = read_flags(raw, format);
    let fixed = format.flag_bytes();
    Some(StaticTile {
        flags,
        weight: raw[fixed],
        layer: raw[fixed + 1],
        anim_id: AnimId(u16::from_le_bytes([raw[fixed + 6], raw[fixed + 7]])),
        height: raw[fixed + 12],
        name: read_name(&raw[fixed + 13..]),
    })
}

/// Read the flag word, which is 4 or 8 bytes and always little-endian.
///
/// `tiledata.mul` is little-endian throughout — the *network* is big-endian, the
/// files are not, and mixing the two up is a whole afternoon.
fn read_flags(raw: &[u8], format: TileDataFormat) -> TileFlags {
    match format {
        TileDataFormat::Legacy => TileFlags::new(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]).into()),
        TileDataFormat::HighSeas => TileFlags::new(u64::from_le_bytes([
            raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
        ])),
    }
}

/// Read a 20-byte NUL-padded name.
fn read_name(raw: &[u8]) -> String {
    let field = &raw[..raw.len().min(20)];
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    field[..end].iter().map(|b| *b as char).collect()
}

#[cfg(test)]
mod tests {
    use openshard_tiles::LandTileId;

    use super::*;

    #[test]
    fn the_two_layouts_are_told_apart_by_arithmetic_alone() {
        // A 7.0.x tiledata.mul. Only the High Seas layout divides it exactly.
        let real = 3_188_736;
        assert!(TileDataFormat::HighSeas.fits(real));
        assert!(!TileDataFormat::Legacy.fits(real));

        // A legacy file: 512 land groups of 26-byte entries, then static groups
        // of 37. There is no flag in the file saying which — this is the whole
        // detection.
        let legacy = TileDataFormat::Legacy.land_table_len() + (GROUP_HEADER + GROUP_SIZE * 37);
        assert!(TileDataFormat::Legacy.fits(legacy));
        assert!(!TileDataFormat::HighSeas.fits(legacy));
    }

    #[test]
    fn a_file_that_is_neither_layout_is_refused() {
        assert!(!TileDataFormat::HighSeas.fits(0));
        assert!(!TileDataFormat::Legacy.fits(0));
        assert!(!TileDataFormat::HighSeas.fits(12));
        // Exactly the land table and no statics: a truncated file.
        assert!(!TileDataFormat::HighSeas.fits(TileDataFormat::HighSeas.land_table_len()));
        assert!(parse(&[0u8; 100]).is_none());
    }

    #[test]
    fn land_table_lengths_match_the_arithmetic() {
        assert_eq!(TileDataFormat::HighSeas.land_table_len(), 512 * (4 + 32 * 30));
        assert_eq!(TileDataFormat::Legacy.land_table_len(), 512 * (4 + 32 * 26));
    }

    /// Build a synthetic High Seas tiledata with one known land and static tile.
    fn synthetic() -> Vec<u8> {
        let format = TileDataFormat::HighSeas;
        let mut bytes = vec![0u8; format.land_table_len()];

        // Land tile 0: flags WATER|BLOCK, texture 0x1234, named "water".
        bytes[4..12].copy_from_slice(&(TileFlags::WATER | TileFlags::BLOCK).to_le_bytes());
        bytes[12..14].copy_from_slice(&0x1234u16.to_le_bytes());
        bytes[14..19].copy_from_slice(b"water");

        // One static group: tile 0 is a 20-tall wall.
        let group = GROUP_HEADER + GROUP_SIZE * format.static_entry();
        let base = bytes.len();
        bytes.resize(base + group, 0);
        let entry = base + GROUP_HEADER;
        bytes[entry..entry + 8].copy_from_slice(&(TileFlags::WALL | TileFlags::BLOCK).to_le_bytes());
        bytes[entry + 8] = 255; // weight
        bytes[entry + 9] = 2; // quality, which for equipment is the layer
        bytes[entry + 14..entry + 16].copy_from_slice(&0xABCDu16.to_le_bytes()); // anim_id
        bytes[entry + 20] = 20; // height
        bytes[entry + 21..entry + 32].copy_from_slice(b"wooden wall");
        bytes
    }

    #[test]
    fn parses_a_synthetic_file() {
        let read = parse(&synthetic()).unwrap();
        assert_eq!(read.format, TileDataFormat::HighSeas);

        let water = read.tiles.land(LandTileId(0));
        assert_eq!(water.name, "water");
        assert!(water.flags.is_water());
        assert!(water.flags.is_blocking());
        // The texture id sits between the flags and the name, and it was read
        // past for this reader's whole life. Reading it one byte out gives a
        // plausible id for every tile in the game and textures the ground with
        // somebody else's terrain — a picture, and the wrong one.
        assert_eq!(
            water.texture,
            TextureId(0x1234),
            "texture id right after the flags"
        );

        let wall = read.tiles.static_tile(0);
        assert_eq!(wall.name, "wooden wall", "name at 21, not 20");
        assert_eq!(wall.height, 20, "height at 20, not 19");
        assert_eq!(wall.weight, 255);
        // The byte after the weight: the quality field, which for a piece of
        // equipment is the paperdoll layer. Pinned here because it sits between two
        // fields that are already read, and an off-by-one would report a plausible
        // layer for every item in the game.
        assert_eq!(wall.layer, 2, "quality/layer at 9, right after the weight");
        // Between the unknown u32 and the hue, on Sphere's own layout — a
        // worn item's *default* drawn graphic, and a different index space
        // from this tile's own art.
        assert_eq!(
            wall.anim_id,
            AnimId(0xABCD),
            "anim id at 14, after the unknown u32"
        );
        assert!(wall.flags.is_blocking());
    }

    #[test]
    fn a_short_static_table_is_padded_rather_than_panicking() {
        // Older tiledata stops well short of 0x10000. A lookup for a tile this
        // client has never heard of has to answer something, and "nothing there"
        // is the only honest answer.
        let read = parse(&synthetic()).unwrap();
        assert_eq!(read.tiles.static_tile(0xFFFF), &StaticTile::default());
        assert_eq!(read.tiles.static_tile(0xFFFF).height, 0);
    }

    #[test]
    fn a_name_stops_at_its_nul() {
        assert_eq!(read_name(b"water\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"), "water");
        assert_eq!(read_name(b"\0garbage"), "");
        assert_eq!(read_name(b"exactly twenty chars"), "exactly twenty chars");
    }
}
