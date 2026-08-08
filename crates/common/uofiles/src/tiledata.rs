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

use crate::texmaps::TextureId;

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
    /// UFLAG2_WALL2: the second wall bit. ServUO calls it `NoShoot` and uses it
    /// for exactly that — a straight line an arrow or a look does not cross.
    ///
    /// The value is `0x2000`, not `0x20`: `0x20` is `UFLAG1_DAMAGE` (a fire, a
    /// spike), and there is no `UFLAG1_NOSHOOT` in Sphere's header at all. Naming
    /// the damage bit "no shoot" made every brazier opaque and every portcullis
    /// transparent, which is the wrong answer in both directions at once.
    pub const NO_SHOOT: u64 = 0x0000_2000;
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
    /// ClassicUO's `TileFlag.Transparent`. Only the renderer reads it, and only
    /// as one half of the pair that keeps a tile from cutting the roof away
    /// above the player — see `openshard-render`'s `cutaway`.
    pub const TRANSPARENT: u64 = 0x0000_0004;
    /// Drawn at partial alpha whatever else is decided about it: a window pane,
    /// a force field. ClassicUO's `TileFlag.Translucent`.
    pub const TRANSLUCENT: u64 = 0x0000_0008;
    /// Never drawn and never walked on: the client's own marker for a graphic
    /// that exists in the tables and nowhere in the world. ClassicUO drops these
    /// in `AddTileToRenderList` before anything else is asked about them.
    pub const INTERNAL: u64 = 0x0001_0000;
    /// A tree's leaves, a boat's mast — the things that fade when a body walks
    /// behind them. ClassicUO's `TileFlag.Foliage`.
    pub const FOLIAGE: u64 = 0x0002_0000;
    /// A roof tile. This is what makes a building's inside visible at all: the
    /// client stops drawing these once the player is under one.
    ///
    /// `0x1000_0000` — ClassicUO's `TileFlag.Roof`. Sphere's header has no name
    /// for this bit, so ClassicUO is the only reference for it and the value is
    /// pinned in a test beside the constant.
    pub const ROOF: u64 = 0x1000_0000;
    /// The static gives off light: a torch, a candle, a brazier, a lantern.
    ///
    /// `0x0080_0000` — ClassicUO's `TileFlag.LightSource`, read in
    /// `TileDataLoader`'s `IsLight`, and ServUO's `TileFlag.LightSource` at the
    /// same value. It says *that* a graphic burns and nothing about how big or
    /// what colour: the client takes those from `light.mul`, keyed by an id this
    /// reader does not carry yet. See `openshard-client-render`'s `light`, which
    /// picks a flame by graphic until that file is read.
    ///
    /// Pinned in a test beside the constant, because a flag means what the
    /// engine *reads* it for.
    pub const LIGHT_SOURCE: u64 = 0x0080_0000;
    /// The static cycles through graphics on its own: a fire, a torch, a water
    /// wheel. What it cycles through is `animdata.mul` — see
    /// [`crate::animdata`] — and this bit is the only thing that says a graphic
    /// animates at all, since that file has a zeroed entry for everything else.
    ///
    /// `0x0100_0000` in both references that name it: ClassicUO's
    /// `TileFlag.Animation` and ServUO's. Pinned in a test beside the constant,
    /// because a flag means what the engine *reads* it for.
    pub const ANIMATION: u64 = 0x0100_0000;

    /// Wrap a raw flag word.
    pub const fn new(bits: u64) -> Self {
        Self(bits)
    }

    /// The raw word.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Whether *any* bit in `mask` is set.
    ///
    /// Any and not all, which is what a caller passing a pair of alternatives
    /// wants — `has(WINDOW | NO_SHOOT)` is "does this stop an arrow", the pair
    /// ServUO's `Map.LineOfSight` tests together, and no tile carries both.
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

    /// Whether this static plays a cycle of its own. See [`Self::ANIMATION`].
    pub const fn is_animated(self) -> bool {
        self.has(Self::ANIMATION)
    }

    /// Whether this burns, glows or otherwise lights its surroundings. See
    /// [`Self::LIGHT_SOURCE`].
    pub const fn is_light_source(self) -> bool {
        self.has(Self::LIGHT_SOURCE)
    }

    /// Whether this is a roof. See [`Self::ROOF`].
    pub const fn is_roof(self) -> bool {
        self.has(Self::ROOF)
    }

    /// Whether the client never draws this. See [`Self::INTERNAL`].
    pub const fn is_internal(self) -> bool {
        self.has(Self::INTERNAL)
    }

    /// Whether this fades when a body walks behind it. See [`Self::FOLIAGE`].
    pub const fn is_foliage(self) -> bool {
        self.has(Self::FOLIAGE)
    }

    /// Whether this lies flat under whatever stands on it — a floor, a rug.
    ///
    /// ClassicUO calls the bit `Background`; this workspace named it after
    /// Sphere's `UFLAG1_FLOOR`. One bit, two names.
    pub const fn is_background(self) -> bool {
        self.has(Self::FLOOR)
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
            (Self::ANIMATION, "ANIMATION"),
            (Self::LIGHT_SOURCE, "LIGHT_SOURCE"),
            (Self::TRANSPARENT, "TRANSPARENT"),
            (Self::TRANSLUCENT, "TRANSLUCENT"),
            (Self::INTERNAL, "INTERNAL"),
            (Self::FOLIAGE, "FOLIAGE"),
            (Self::ROOF, "ROOF"),
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
    /// Which square texture the ground is stretched over where it slopes.
    ///
    /// Its own index space — see [`crate::texmaps`] — and unrelated to the tile's
    /// art graphic. [`TextureId(0)`](TextureId) is the ordinary "none": entry 0
    /// of `texidx.mul` is empty, and the client draws such a tile flat however
    /// the ground around it stands.
    pub texture: TextureId,
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
    /// Which paperdoll layer a wearable copy of it sits on.
    ///
    /// UO's file documentation calls this field *quality*, and for a piece of
    /// equipment the value is its layer — ServUO reads it exactly that way
    /// (`BaseWeapon`: `Layer = (Layer)ItemData.Quality`), which is how a halberd
    /// knows to take both hands. It was read past for most of this reader's life
    /// because nothing asked; Arms Lore does.
    pub layer: u8,
    /// What a worn copy of it draws as, in the body-animation index space —
    /// a different space from this tile's own art graphic, and read from
    /// `anim.mul`/`AnimAtlas` rather than `art.mul`.
    ///
    /// This is the *default* a worn item draws with — `EquipConv` only
    /// overrides it for the pairs where a body needs a different picture
    /// (a race or gender variant); an ordinary shirt has no such entry and
    /// draws from this field directly. Read past for most of this reader's
    /// life, the same way `layer` was, because nothing asked for it either.
    pub anim_id: u16,
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

impl AsRef<TileData> for TileData {
    fn as_ref(&self) -> &TileData {
        self
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

    /// Every tile, defined and unremarkable: no flags, no height, no name.
    ///
    /// For a caller that needs the *shape* of a tiledata and not the client's —
    /// a renderer test about where a sprite lands, which is decided by the
    /// sprite's own size and not by a flag. It is honest here in a way it would
    /// not be for a test about flags: nothing in it is a guess at what the file
    /// says, because it says nothing at all. Anything asserting on real flags
    /// reads a real install, the way `tests/client_files.rs` does.
    ///
    /// The format is claimed to be the modern one, since there are no records
    /// whose layout could disagree.
    pub fn empty() -> Self {
        Self {
            land: vec![LandTile::default(); LAND_TILE_COUNT],
            statics: vec![StaticTile::default(); STATIC_TILE_COUNT],
            format: TileDataFormat::HighSeas,
        }
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

    /// Put one entry into the table, replacing whatever was there.
    ///
    /// For tests that need a tiledata saying one specific thing — a graphic that
    /// is a light source, a roof, a wall — the way [`TileData::empty`] is for
    /// tests that need it to say nothing. It is `pub` and not `#[cfg(test)]`
    /// because the tests that want it are in other crates: a renderer's test
    /// about what a flag makes it draw cannot read a real install, since this
    /// repository ships no client files.
    ///
    /// Nothing in the engine calls it, and nothing should: what a graphic can do
    /// is the client's file talking, and an entry written over at runtime is a
    /// disagreement between the two ends of the wire about the same graphic.
    pub fn set_static_tile(&mut self, id: u16, tile: StaticTile) {
        self.statics[id as usize] = tile;
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
            anim_id: u16::from_le_bytes([raw[fixed + 6], raw[fixed + 7]]),
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

    /// The bits the renderer reads, pinned against ClassicUO's `TileFlag`.
    ///
    /// None of these appear in Sphere's `uofiles_macros.h` under a name that
    /// says what the client does with them, so ClassicUO is the only reference
    /// and a wrong bit here is silent: the roof simply never lifts, or every
    /// wall is treated as one. The values are `TileDataLoader.cs`'s enum.
    #[test]
    fn the_drawing_flags_are_classicuos_bits() {
        assert_eq!(TileFlags::TRANSPARENT, 0x0000_0004);
        assert_eq!(TileFlags::TRANSLUCENT, 0x0000_0008);
        assert_eq!(TileFlags::INTERNAL, 0x0001_0000);
        assert_eq!(TileFlags::FOLIAGE, 0x0002_0000);
        assert_eq!(TileFlags::ROOF, 0x1000_0000);
        // ClassicUO's `Surface` and `Bridge` are bits this workspace already
        // named after Sphere. Asserted here rather than trusted, because the
        // renderer is about to read them under the client's names: a surface is
        // what a roof may still be drawn as, and a bridge is what
        // `CalculateObjectHeight` halves.
        assert_eq!(TileFlags::PLATFORM, 0x0000_0200, "ClassicUO's Surface");
        assert_eq!(TileFlags::CLIMBABLE, 0x0000_0400, "ClassicUO's Bridge");
        // `TileFlag.LightSource`, which `TileDataLoader.IsLight` reads and
        // ServUO's `TileData` gives the same value. One bit off is a torch that
        // lights nothing and a bookshelf that burns: `0x0040_0000` next door is
        // `Wearable` and `0x0100_0000` above it is `Animation`, and both are
        // set on plenty of graphics that are not on fire.
        assert_eq!(TileFlags::LIGHT_SOURCE, 0x0080_0000);
        assert!(TileFlags::new(TileFlags::LIGHT_SOURCE).is_light_source());
        assert!(!TileFlags::new(TileFlags::ANIMATION).is_light_source());
    }

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
        let data = TileData::parse(&synthetic()).unwrap();
        assert_eq!(data.format(), TileDataFormat::HighSeas);

        let water = data.land(0);
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

        let wall = data.static_tile(0);
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
        assert_eq!(wall.anim_id, 0xABCD, "anim id at 14, after the unknown u32");
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
        assert_eq!(TileFlags::NO_SHOOT, 0x0000_2000);
        assert_eq!(TileFlags::DOOR, 0x2000_0000);
        // The bit next door, and the reason NO_SHOOT is pinned here: 0x20 is
        // UFLAG1_DAMAGE, and naming it "no shoot" is a one-character mistake
        // that silently moves every line-of-sight test one flag to the left.
        assert_ne!(TileFlags::NO_SHOOT, 0x0000_0020);
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
