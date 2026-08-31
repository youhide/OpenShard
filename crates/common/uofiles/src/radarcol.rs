//! `radarcol.mul` — one colour per tile, which is the whole of a minimap.
//!
//! A flat table of 16-bit colours in two halves: the first
//! [`LAND_TILE_COUNT`](openshard_tiles::LAND_TILE_COUNT) are land tiles, keyed
//! by land id, and everything after is statics, keyed by `LAND_TILE_COUNT +
//! graphic`. There is no header, no index and no compression — the offset *is*
//! the key.
//!
//! # The split is `tiledata`'s, not a magic number
//!
//! `0x4000` is how many land tiles a client has, which this crate already
//! states once in [`crate::tiledata`]. Writing it again here as a literal would
//! be a second copy of the same fact, and the two would eventually disagree.
//!
//! It is also *checked* rather than assumed: neither reference server reads this
//! file — both are shards and this is a render file — so the split was confirmed
//! against a real install instead. Land `0x03` comes out green, land `0xA8`
//! comes out blue, static `0x0006` (a wall) comes out brown and static `0x0751`
//! (a stair) comes out grey stone. Every one is the colour the thing is, which
//! no other split would produce.
//!
//! # The file is not a fixed size, and assuming one refuses real installs
//!
//! The canonical table is `0x4000 + 0x10000` entries, 163,840 bytes. **The
//! install this was written against is 163,768** — thirty-six entries short,
//! with no padding and no trailing zeroes. It simply stops.
//!
//! So the length is read rather than demanded, the split is a *position* rather
//! than a size, and an id past the end answers [`Color16::TRANSPARENT`] the way
//! an id inside the table with no colour does. A reader that insisted on the
//! canonical size would have rejected the operator's own client, and would have
//! been wrong to.

use std::fmt;
use std::path::{
    Path,
    PathBuf,
};

use openshard_protocol::wire::Graphic;
use openshard_tiles::{
    LAND_TILE_COUNT,
    LandTileId,
};

use crate::color::Color16;

/// `radarcol.mul` could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum RadarColorsError {
    /// The file could not be read.
    Read {
        /// Which file.
        path:   PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The file is not a whole number of 16-bit colours, or it is too short to
    /// hold even the land half.
    NotRadarColors {
        /// Which file.
        path: PathBuf,
        /// How big it is.
        size: usize,
    },
}

impl fmt::Display for RadarColorsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::NotRadarColors { path, size } => {
                write!(
                    f,
                    "{} is {size} bytes, which is not a whole number of 16-bit colours covering the \
                 {LAND_TILE_COUNT} land tiles; it is not radarcol.mul",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for RadarColorsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::NotRadarColors { .. } => None,
        }
    }
}

/// One colour per land tile and per static — the table a minimap is drawn from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RadarColors {
    colors: Vec<Color16>,
}

impl RadarColors {
    /// Read the file at `path`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, RadarColorsError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|source| {
            RadarColorsError::Read {
                path: path.to_path_buf(),
                source,
            }
        })?;
        Self::parse(&bytes).ok_or_else(|| {
            RadarColorsError::NotRadarColors {
                path: path.to_path_buf(),
                size: bytes.len(),
            }
        })
    }

    /// Read a table already in memory.
    ///
    /// Split from [`load`](Self::load) so the format is testable without an
    /// install — this crate's own rule, and the reason `hues.rs` has the same
    /// pair.
    ///
    /// `None` for a file that is not a whole number of colours, or one too short
    /// to hold the land half. A file *longer* than the land half is accepted
    /// whatever its length: see the module header for why the size is not fixed.
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if !bytes.len().is_multiple_of(2) || bytes.len() / 2 < LAND_TILE_COUNT {
            return None;
        }
        Some(Self {
            colors: bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| Color16(u16::from_le_bytes([pair[0], pair[1]])))
                .collect(),
        })
    }

    /// How many colours the table holds, land and statics together.
    #[must_use]
    pub fn len(&self) -> usize {
        self.colors.len()
    }

    /// Whether the table is empty. It never is — [`parse`](Self::parse) refuses
    /// anything shorter than the land half — but clippy asks and a reader
    /// should not have to check `len`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    /// The colour of a land tile.
    ///
    /// Typed rather than indexed by a bare integer, so a static id cannot be
    /// passed where a land id belongs — they are different spaces and the file
    /// keeps them in one array, which is exactly the confusion a newtype is for.
    #[must_use]
    pub fn land(&self, tile: LandTileId) -> Color16 {
        self.colors
            .get(usize::from(tile.0))
            .copied()
            .unwrap_or(Color16::TRANSPARENT)
    }

    /// The colour of a static.
    #[must_use]
    pub fn statik(&self, graphic: Graphic) -> Color16 {
        self.colors
            .get(LAND_TILE_COUNT + usize::from(graphic.0))
            .copied()
            .unwrap_or(Color16::TRANSPARENT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table with `land` land entries and `statics` static ones, each numbered
    /// so a wrong offset reads as a wrong number rather than as a plausible
    /// colour.
    fn table(land: usize, statics: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        for i in 0..land {
            bytes.extend_from_slice(&u16::try_from(i).unwrap_or(0).to_le_bytes());
        }
        for i in 0..statics {
            bytes.extend_from_slice(&(0x8000 | u16::try_from(i).unwrap_or(0)).to_le_bytes());
        }
        bytes
    }

    #[test]
    fn the_split_is_the_land_table_count() {
        let colors = RadarColors::parse(&table(LAND_TILE_COUNT, 16)).expect("a whole table");

        assert_eq!(colors.land(LandTileId(0)), Color16(0));
        assert_eq!(colors.land(LandTileId(3)), Color16(3));
        assert_eq!(
            colors.land(LandTileId(0x3FFF)),
            Color16(0x3FFF),
            "the last land tile"
        );
        assert_eq!(
            colors.statik(Graphic(0)),
            Color16(0x8000),
            "the first static reads the entry after the last land tile"
        );
        assert_eq!(colors.statik(Graphic(5)), Color16(0x8005));
    }

    /// **The file this was written against is thirty-six entries short of the
    /// canonical size**, so a reader that demanded one would refuse a real
    /// install. Anything past the end is absent, not an error.
    #[test]
    fn a_table_shorter_than_the_canonical_size_still_reads() {
        let colors = RadarColors::parse(&table(LAND_TILE_COUNT, 4)).expect("a short table is a table");

        assert_eq!(colors.statik(Graphic(3)), Color16(0x8003), "the last one it has");
        assert_eq!(
            colors.statik(Graphic(4)),
            Color16::TRANSPARENT,
            "one past the end is absent rather than a panic"
        );
        assert_eq!(colors.statik(Graphic(0xFFFF)), Color16::TRANSPARENT);
    }

    /// The land half alone is a legal table — every static is simply absent.
    #[test]
    fn the_land_half_alone_is_enough() {
        let colors = RadarColors::parse(&table(LAND_TILE_COUNT, 0)).expect("the land half is a table");
        assert_eq!(colors.len(), LAND_TILE_COUNT);
        assert_eq!(colors.statik(Graphic(0)), Color16::TRANSPARENT);
    }

    /// Shorter than the land half is not this file. A truncated table would
    /// otherwise read every static as absent and every high land tile too, which
    /// looks like a map of nothing rather than a bad file.
    #[test]
    fn a_file_too_short_for_the_land_half_is_refused() {
        assert!(RadarColors::parse(&table(LAND_TILE_COUNT - 1, 0)).is_none());
        assert!(RadarColors::parse(&[]).is_none());
    }

    /// An odd number of bytes is not a table of 16-bit colours.
    #[test]
    fn an_odd_length_is_refused() {
        let mut bytes = table(LAND_TILE_COUNT, 1);
        bytes.push(0);
        assert!(RadarColors::parse(&bytes).is_none());
    }

    /// Little-endian, like every other colour in these files.
    #[test]
    fn a_colour_is_little_endian() {
        let mut bytes = table(LAND_TILE_COUNT, 0);
        bytes[0] = 0x34;
        bytes[1] = 0x12;
        let colors = RadarColors::parse(&bytes).expect("a whole table");
        assert_eq!(colors.land(LandTileId(0)), Color16(0x1234));
    }
}
