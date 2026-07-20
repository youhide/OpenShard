//! `tiledata.mul`: what every tile in the game *is*.
//!
//! Two tables. Land tiles are the ground itself, 0x4000 of them. Static tiles
//! are everything sitting on it — walls, trees, doors — 0x10000 of them. Both
//! carry a flag word saying whether you can walk on it, stand on it, swim in it
//! or climb it, and statics carry a height.
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

/// How many land tiles a client knows about.
pub const LAND_TILE_COUNT: usize = 0x4000;
/// How many static tiles a client knows about.
pub const STATIC_TILE_COUNT: usize = 0x10000;

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
/// A static entry, High Seas. See [`TileData::parse_static`] for the layout.
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

/// What a tile can do, straight from `tiledata.mul`.
///
/// The bits are Sphere's `UFLAG*` in `game/uo_files/uofiles_macros.h`. Only the
/// ones movement needs are named; the rest are on the wire and not our business.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct TileFlags(u64);

impl TileFlags {
    /// UFLAG1_FLOOR: walkable at its base.
    pub const FLOOR: u64 = 0x0000_0001;
    /// UFLAG1_WALL: wall, door or fireplace.
    pub const WALL: u64 = 0x0000_0010;
    /// UFLAG1_NOSHOOT: blocks a straight line — arrows, and sight.
    pub const NO_SHOOT: u64 = 0x0000_0020;
    /// UFLAG1_BLOCK: too big and heavy to walk through.
    pub const BLOCK: u64 = 0x0000_0040;
    /// UFLAG1_WATER: water or wet.
    pub const WATER: u64 = 0x0000_0080;
    /// UFLAG2_PLATFORM: you can stand on top of it.
    pub const PLATFORM: u64 = 0x0000_0200;
    /// UFLAG2_CLIMBABLE: stairs. Sphere halves the height of these.
    pub const CLIMBABLE: u64 = 0x0000_0400;
    /// UFLAG2_WINDOW: an arch or doorway you can walk through.
    pub const WINDOW: u64 = 0x0000_1000;
    /// UFLAG4_DOOR.
    pub const DOOR: u64 = 0x2000_0000;

    /// Wrap a raw flag word.
    pub const fn new(bits: u64) -> Self {
        Self(bits)
    }

    /// The raw word.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Whether every bit in `mask` is set.
    pub const fn has(self, mask: u64) -> bool {
        self.0 & mask != 0
    }

    /// Whether this is water.
    pub const fn is_water(self) -> bool {
        self.has(Self::WATER)
    }

    /// Whether this blocks a walking human.
    pub const fn is_blocking(self) -> bool {
        self.has(Self::BLOCK)
    }

    /// Whether a mobile can stand on top of this.
    pub const fn is_platform(self) -> bool {
        self.has(Self::PLATFORM)
    }

    /// Whether this is stairs.
    pub const fn is_climbable(self) -> bool {
        self.has(Self::CLIMBABLE)
    }
}

impl fmt::Debug for TileFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut names = Vec::new();
        for (mask, name) in [
            (Self::FLOOR, "FLOOR"),
            (Self::WALL, "WALL"),
            (Self::NO_SHOOT, "NO_SHOOT"),
            (Self::BLOCK, "BLOCK"),
            (Self::WATER, "WATER"),
            (Self::PLATFORM, "PLATFORM"),
            (Self::CLIMBABLE, "CLIMBABLE"),
            (Self::WINDOW, "WINDOW"),
            (Self::DOOR, "DOOR"),
        ] {
            if self.has(mask) {
                names.push(name);
            }
        }
        write!(f, "TileFlags(0x{:X}", self.0)?;
        if !names.is_empty() {
            write!(f, " {}", names.join("|"))?;
        }
        f.write_str(")")
    }
}

/// One land tile: the ground.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct LandTile {
    /// What it can do.
    pub flags: TileFlags,
    /// Its name, for logs and tools. Often "NoName".
    pub name: String,
}

/// One static tile: anything standing on the ground.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct StaticTile {
    /// What it can do.
    pub flags: TileFlags,
    /// How tall it is.
    ///
    /// For climbable tiles this is the *full* height; Sphere halves it when
    /// working out where you end up standing. See `MapTerrain`.
    pub height: u8,
    /// 255 means immovable.
    pub weight: u8,
    /// Its name.
    pub name: String,
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

/// Every tile definition the client has.
///
/// `Clone` because it is shared across facets: `tiledata.mul` describes tiles,
/// not a map, so one copy is read and each facet's terrain gets its own.
#[derive(Clone)]
pub struct TileData {
    land: Vec<LandTile>,
    statics: Vec<StaticTile>,
    format: TileDataFormat,
}

impl fmt::Debug for TileData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TileData")
            .field("format", &self.format)
            .field("land", &self.land.len())
            .field("statics", &self.statics.len())
            .finish()
    }
}

impl TileData {
    /// Read `tiledata.mul`, working out its layout from its size.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, TileDataError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|source| TileDataError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&bytes).ok_or_else(|| TileDataError::UnknownFormat {
            path: path.to_owned(),
            size: bytes.len(),
        })
    }

    /// Parse bytes that are already in memory.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        // Newest first. Both are checked rather than assumed, and only one can
        // divide the file exactly.
        let format = [TileDataFormat::HighSeas, TileDataFormat::Legacy]
            .into_iter()
            .find(|format| format.fits(bytes.len()))?;

        let mut land = Vec::with_capacity(LAND_TILE_COUNT);
        for index in 0..LAND_TILE_COUNT {
            land.push(Self::parse_land(bytes, format, index)?);
        }

        // The static table runs to the end of the file. Modern tiledata has
        // 0x10000 entries, but older files stop short — read what is there and
        // pad, so a lookup never panics on a tile this client has not heard of.
        let mut statics = Vec::with_capacity(STATIC_TILE_COUNT);
        for index in 0..STATIC_TILE_COUNT {
            match Self::parse_static(bytes, format, index) {
                Some(tile) => statics.push(tile),
                None => break,
            }
        }
        statics.resize(STATIC_TILE_COUNT, StaticTile::default());

        Some(Self {
            land,
            statics,
            format,
        })
    }

    /// Which layout this file turned out to be in.
    pub const fn format(&self) -> TileDataFormat {
        self.format
    }

    /// A land tile. Total: the index is masked into range.
    ///
    /// Masking rather than returning `Option` because the caller is the map,
    /// every id in it came off disk, and a `None` there would mean an unwalkable
    /// hole rather than an error anyone can act on.
    pub fn land(&self, id: u16) -> &LandTile {
        &self.land[(id as usize) & (LAND_TILE_COUNT - 1)]
    }

    /// A static tile. Total: every `u16` is a valid index.
    pub fn static_tile(&self, id: u16) -> &StaticTile {
        &self.statics[id as usize]
    }

    fn parse_land(bytes: &[u8], format: TileDataFormat, index: usize) -> Option<LandTile> {
        let entry = format.land_entry();
        let offset = (index / GROUP_SIZE) * (GROUP_HEADER + GROUP_SIZE * entry)
            + GROUP_HEADER
            + (index % GROUP_SIZE) * entry;
        let raw = bytes.get(offset..offset + entry)?;

        let flags = read_flags(raw, format);
        // flags, then a u16 texture id, then the name.
        let name_at = format.flag_bytes() + 2;
        Some(LandTile {
            flags,
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
            height: raw[fixed + 12],
            name: read_name(&raw[fixed + 13..]),
        })
    }
}

/// Read the flag word, which is 4 or 8 bytes and always little-endian.
///
/// `tiledata.mul` is little-endian throughout — the *network* is big-endian, the
/// files are not, and mixing the two up is a whole afternoon.
fn read_flags(raw: &[u8], format: TileDataFormat) -> TileFlags {
    match format {
        TileDataFormat::Legacy => {
            TileFlags::new(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]).into())
        }
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

/// Resolve the pluralization markers in a tiledata name, given whether the pile
/// is plural (more than one).
///
/// UO item names carry `%...%` blocks the client normally interprets and the
/// server has to as well when it draws the name itself (a single-click label):
/// left raw, `"bolt%s% of cloth"` reaches the client verbatim. Inside a block a
/// `/` splits the plural form (before it) from the singular (after it), so
/// `%s%` adds an "s" when plural and nothing when singular, and `%ves/f%` gives
/// "…ves" / "…f". Text outside a block is always kept. Ported from Sphere's
/// `CItemBase::GetNamePluralize`.
#[must_use]
pub fn pluralize_name(name: &str, plural: bool) -> String {
    let mut out = String::with_capacity(name.len());
    let mut inside = false;
    // Within a block, the part before a `/` is the plural form. A block with no
    // `/` is a pure plural suffix (`%s%`), kept only when pluralizing.
    let mut is_plural_part = true;
    for ch in name.chars() {
        match ch {
            '%' => {
                inside = !inside;
                is_plural_part = true;
            }
            '/' if inside => is_plural_part = false,
            _ if inside && (plural != is_plural_part) => {}
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pluralize_resolves_the_tiledata_markers() {
        // The reported bug: "bolt%s% of cloth" reaching the client verbatim.
        assert_eq!(pluralize_name("bolt%s% of cloth", false), "bolt of cloth");
        assert_eq!(pluralize_name("bolt%s% of cloth", true), "bolts of cloth");
        // A block with a slash: plural before, singular after.
        assert_eq!(pluralize_name("loa%ves/f%", true), "loaves");
        assert_eq!(pluralize_name("loa%ves/f%", false), "loaf");
        // A name with no markers is untouched either way.
        assert_eq!(pluralize_name("a torch", true), "a torch");
    }

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
        assert!(TileData::parse(&[0u8; 100]).is_none());
    }

    #[test]
    fn land_table_lengths_match_the_arithmetic() {
        assert_eq!(
            TileDataFormat::HighSeas.land_table_len(),
            512 * (4 + 32 * 30)
        );
        assert_eq!(TileDataFormat::Legacy.land_table_len(), 512 * (4 + 32 * 26));
    }

    /// Build a synthetic High Seas tiledata with one known land and static tile.
    fn synthetic() -> Vec<u8> {
        let format = TileDataFormat::HighSeas;
        let mut bytes = vec![0u8; format.land_table_len()];

        // Land tile 0: flags WATER|BLOCK, named "water".
        bytes[4..12].copy_from_slice(&(TileFlags::WATER | TileFlags::BLOCK).to_le_bytes());
        bytes[14..19].copy_from_slice(b"water");

        // One static group: tile 0 is a 20-tall wall.
        let group = GROUP_HEADER + GROUP_SIZE * format.static_entry();
        let base = bytes.len();
        bytes.resize(base + group, 0);
        let entry = base + GROUP_HEADER;
        bytes[entry..entry + 8]
            .copy_from_slice(&(TileFlags::WALL | TileFlags::BLOCK).to_le_bytes());
        bytes[entry + 8] = 255; // weight
        bytes[entry + 20] = 20; // height
        bytes[entry + 21..entry + 32].copy_from_slice(b"wooden wall");
        bytes
    }

    #[test]
    fn parses_a_synthetic_file() {
        let data = TileData::parse(&synthetic()).unwrap();
        assert_eq!(data.format(), TileDataFormat::HighSeas);

        let water = data.land(0);
        assert_eq!(water.name, "water");
        assert!(water.flags.is_water());
        assert!(water.flags.is_blocking());

        let wall = data.static_tile(0);
        assert_eq!(wall.name, "wooden wall", "name at 21, not 20");
        assert_eq!(wall.height, 20, "height at 20, not 19");
        assert_eq!(wall.weight, 255);
        assert!(wall.flags.is_blocking());
    }

    #[test]
    fn a_short_static_table_is_padded_rather_than_panicking() {
        // Older tiledata stops well short of 0x10000. A lookup for a tile this
        // client has never heard of has to answer something, and "nothing there"
        // is the only honest answer.
        let data = TileData::parse(&synthetic()).unwrap();
        assert_eq!(data.static_tile(0xFFFF), &StaticTile::default());
        assert_eq!(data.static_tile(0xFFFF).height, 0);
    }

    #[test]
    fn land_lookups_are_total() {
        // Every id in a map block came off disk and may be anything. A panic
        // here would mean one bad tile takes the shard down.
        let data = TileData::parse(&synthetic()).unwrap();
        for id in [0u16, 1, 0x3FFF, 0x4000, 0xFFFF] {
            let _ = data.land(id);
        }
    }

    #[test]
    fn flags_name_the_bits_sphere_names() {
        // Pinned to uofiles_macros.h. These are not ours to renumber.
        assert_eq!(TileFlags::FLOOR, 0x0000_0001);
        assert_eq!(TileFlags::WALL, 0x0000_0010);
        assert_eq!(TileFlags::BLOCK, 0x0000_0040);
        assert_eq!(TileFlags::WATER, 0x0000_0080);
        assert_eq!(TileFlags::PLATFORM, 0x0000_0200);
        assert_eq!(TileFlags::CLIMBABLE, 0x0000_0400);
        assert_eq!(TileFlags::WINDOW, 0x0000_1000);
        assert_eq!(TileFlags::DOOR, 0x2000_0000);
    }

    #[test]
    fn flags_read_the_way_the_real_files_do() {
        // A water land tile is 0xC0 = BLOCK|WATER.
        let water = TileFlags::new(0xC0);
        assert!(water.is_water());
        assert!(water.is_blocking());
        assert!(!water.is_platform());

        // Grass is zero: no flags at all, and perfectly walkable.
        let grass = TileFlags::new(0);
        assert!(!grass.is_water());
        assert!(!grass.is_blocking());
    }

    #[test]
    fn a_name_stops_at_its_nul() {
        assert_eq!(read_name(b"water\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"), "water");
        assert_eq!(read_name(b"\0garbage"), "");
        assert_eq!(read_name(b"exactly twenty chars"), "exactly twenty chars");
    }
}
