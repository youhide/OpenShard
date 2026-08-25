//! `hues.mul`: the palettes every recoloured thing in the game is drawn with.
//!
//! A hue is 32 colours — a ramp from dark to light — and everything the server
//! calls a [`Hue`] is an index into this table. Art is stored once, in its own
//! colours, and a shard tints it by naming a row here; that is why a bloody
//! bandage and a clean one are one sprite, and why getting this table wrong
//! recolours the entire world at once rather than one item.
//!
//! # The index is one-based, and nothing says so
//!
//! `Hue(0)` means "draw the art as it was painted" — it is not the first row.
//! `Hue(1)` is. So the lookup is `hue - 1`, and skipping that subtraction shifts
//! every colour in the game by exactly one row: still colours, still plausible,
//! uniformly wrong. See [`Hues::get`].
//!
//! # Layout
//!
//! ```text
//!   group   4-byte header, then 8 entries          708 bytes
//!   entry   32 colours, table start, table end,
//!           20-byte name                            88 bytes
//! ```
//!
//! The header is not read by anything. A modern `hues.mul` is 375 groups —
//! 3,000 hues — and the count comes from the file's length, because there is no
//! field that carries it.

use std::fmt;
use std::path::{Path, PathBuf};

use openshard_protocol::wire::Hue;

use crate::color::Color16;

/// Colours in one hue's ramp.
pub const COLORS_PER_HUE: usize = 32;
/// Hues per group on disk.
const HUES_PER_GROUP: usize = 8;
/// Bytes per hue entry: 32 colours, two table bounds, a 20-byte name.
const ENTRY_BYTES: usize = COLORS_PER_HUE * 2 + 2 + 2 + NAME_BYTES;
/// The name field's width.
const NAME_BYTES: usize = 20;
/// The header before every group of eight. Unused, but it is on disk.
const GROUP_HEADER: usize = 4;
/// Bytes per group.
const GROUP_BYTES: usize = GROUP_HEADER + HUES_PER_GROUP * ENTRY_BYTES;

/// `hues.mul` could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum HuesError {
    /// The file could not be read.
    Read {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The file does not divide into whole groups, so it is not `hues.mul`.
    NotHues {
        /// Which file.
        path: PathBuf,
        /// How big it is.
        size: usize,
    },
}

impl fmt::Display for HuesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::NotHues { path, size } => write!(
                f,
                "{} is {size} bytes, which is not a whole number of {GROUP_BYTES}-byte hue groups; \
                 it is not hues.mul",
                path.display()
            ),
        }
    }
}

impl std::error::Error for HuesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::NotHues { .. } => None,
        }
    }
}

/// One hue: a 32-step ramp and the range of the original palette it replaces.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HueEntry {
    /// The ramp, darkest first.
    pub colors: [Color16; COLORS_PER_HUE],
    /// The first colour of the range this hue was authored to replace.
    ///
    /// Read by tools that recolour art offline. A renderer tinting a sprite at
    /// draw time uses [`colors`](Self::colors) and ignores both bounds.
    pub table_start: Color16,
    /// The last colour of that range.
    pub table_end: Color16,
    /// Its name, for tools. Usually empty.
    pub name: String,
}

/// Every hue the client knows.
#[derive(Clone)]
pub struct Hues {
    entries: Vec<HueEntry>,
}

impl fmt::Debug for Hues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hues").field("hues", &self.entries.len()).finish()
    }
}

impl Hues {
    /// Read `hues.mul`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, HuesError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|source| HuesError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&bytes).ok_or_else(|| HuesError::NotHues {
            path: path.to_owned(),
            size: bytes.len(),
        })
    }

    /// Parse bytes already in memory, or `None` if they are not whole groups.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || !bytes.len().is_multiple_of(GROUP_BYTES) {
            return None;
        }
        let groups = bytes.len() / GROUP_BYTES;

        let mut entries = Vec::with_capacity(groups * HUES_PER_GROUP);
        for group in 0..groups {
            for index in 0..HUES_PER_GROUP {
                let at = group * GROUP_BYTES + GROUP_HEADER + index * ENTRY_BYTES;
                let raw = bytes.get(at..at + ENTRY_BYTES)?;

                let mut colors = [Color16::TRANSPARENT; COLORS_PER_HUE];
                for (slot, word) in colors.iter_mut().zip(raw.chunks_exact(2)) {
                    *slot = Color16(u16::from_le_bytes([word[0], word[1]]));
                }
                let bounds = COLORS_PER_HUE * 2;
                entries.push(HueEntry {
                    colors,
                    table_start: Color16(u16::from_le_bytes([raw[bounds], raw[bounds + 1]])),
                    table_end: Color16(u16::from_le_bytes([raw[bounds + 2], raw[bounds + 3]])),
                    name: read_name(&raw[bounds + 4..]),
                });
            }
        }
        Some(Self { entries })
    }

    /// How many hues this file holds. 3,000 on a modern client.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// The hue a [`Hue`] names, or `None` for "no tint" and for one past the end.
    ///
    /// Two conversions happen here, and both are easy to leave out:
    ///
    /// - **The index is one-based.** `Hue(0)` is not row zero, it is *no hue at
    ///   all* — draw the art in its own colours. Every real hue is `row + 1`.
    /// - **The top two bits are flags, not part of the number.** `0x8000` asks
    ///   for a partial hue (tint only the grey pixels, so skin and metal keep
    ///   their own colour) and it rides along in the same `u16` the index does.
    ///   Left in place it turns hue 1 into hue 32,769 and the lookup misses
    ///   entirely.
    ///
    /// Whether to tint partially is the renderer's decision and this does not
    /// answer it — see [`is_partial`].
    pub fn get(&self, hue: Hue) -> Option<&HueEntry> {
        let row = (hue.0 & HUE_INDEX_MASK).checked_sub(1)?;
        self.entries.get(row as usize)
    }
}

/// The bits of a wire hue that are the index into `hues.mul`.
const HUE_INDEX_MASK: u16 = 0x3FFF;
/// The bit asking for the grey pixels only to be tinted.
const HUE_PARTIAL_FLAG: u16 = 0x8000;

/// Whether this hue asks to tint only the art's grey pixels.
///
/// A partial hue leaves anything already coloured alone, which is how a dyed
/// robe keeps the wearer's skin its own colour. Reading the table does not
/// depend on this; drawing with it does.
pub const fn is_partial(hue: Hue) -> bool {
    hue.0 & HUE_PARTIAL_FLAG != 0
}

/// Ask for the partial form of a real hue.
///
/// A partial flag without an index is not meaningful: [`Hue::NONE`] still
/// means "leave the pixels alone", just as it does on the wire.
pub const fn partial(hue: Hue) -> Hue {
    if hue.0 & HUE_INDEX_MASK == 0 {
        Hue::NONE
    } else {
        Hue(hue.0 | HUE_PARTIAL_FLAG)
    }
}

/// Remove a partial-hue request without changing its palette index.
///
/// `fonts.mul` uses different source palette indexes for ink and contour.
/// Its renderer needs the full ramp for those distinct pixels to stay
/// independently colourable, even when a caller passed a hue with this flag.
pub const fn full(hue: Hue) -> Hue {
    Hue(hue.0 & !HUE_PARTIAL_FLAG)
}

/// Read a NUL-padded name field.
fn read_name(raw: &[u8]) -> String {
    let field = &raw[..raw.len().min(NAME_BYTES)];
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    field[..end].iter().map(|b| *b as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_arithmetic_is_what_the_file_says() {
        assert_eq!(ENTRY_BYTES, 88, "32 colours, two bounds, a 20-byte name");
        assert_eq!(GROUP_BYTES, 708, "a 4-byte header and eight entries");
        // A modern hues.mul is 265,500 bytes, and that is 375 groups exactly.
        assert_eq!(265_500 / GROUP_BYTES, 375);
        assert_eq!(375 * GROUP_BYTES, 265_500);
        assert_eq!(375 * HUES_PER_GROUP, 3000);
    }

    /// One group: hue 0 is a flat ramp of `0x0001`, hue 1 counts upward.
    fn synthetic() -> Vec<u8> {
        let mut bytes = vec![0u8; GROUP_BYTES];
        for index in 0..COLORS_PER_HUE {
            let at = GROUP_HEADER + index * 2;
            bytes[at..at + 2].copy_from_slice(&1u16.to_le_bytes());
        }
        let bounds = GROUP_HEADER + COLORS_PER_HUE * 2;
        bytes[bounds..bounds + 2].copy_from_slice(&0x0001u16.to_le_bytes());
        bytes[bounds + 2..bounds + 4].copy_from_slice(&0x0422u16.to_le_bytes());

        let second = GROUP_HEADER + ENTRY_BYTES;
        for index in 0..COLORS_PER_HUE {
            let at = second + index * 2;
            bytes[at..at + 2].copy_from_slice(&(index as u16 + 6).to_le_bytes());
        }
        let name = second + COLORS_PER_HUE * 2 + 4;
        bytes[name..name + 5].copy_from_slice(b"ochre");
        bytes
    }

    #[test]
    fn parses_a_group() {
        let hues = Hues::parse(&synthetic()).unwrap();
        assert_eq!(hues.count(), HUES_PER_GROUP);

        // Row 0 is Hue(1): the one-based index, from the outside.
        let first = hues.get(Hue(1)).unwrap();
        assert_eq!(first.colors[0], Color16(1));
        assert_eq!(first.colors[COLORS_PER_HUE - 1], Color16(1));
        assert_eq!(first.table_start, Color16(0x0001));
        assert_eq!(first.table_end, Color16(0x0422));
        assert_eq!(first.name, "");

        let second = hues.get(Hue(2)).unwrap();
        assert_eq!(
            second.colors[0],
            Color16(6),
            "the ramp starts where the file says"
        );
        assert_eq!(second.colors[31], Color16(37));
        assert_eq!(second.name, "ochre", "the name is 20 bytes past the bounds");
    }

    #[test]
    fn hue_zero_is_no_hue_rather_than_the_first_row() {
        // The one-off that recolours the whole world. `Hue::NONE` must not
        // resolve to a palette at all, and `Hue(1)` must be the first row.
        let hues = Hues::parse(&synthetic()).unwrap();
        assert!(hues.get(Hue::NONE).is_none());
        assert!(hues.get(Hue(0)).is_none());
        assert_eq!(
            hues.get(Hue(1)).unwrap(),
            &hues.entries[0],
            "Hue(1) is row 0, not row 1"
        );
        assert_eq!(hues.get(Hue(8)).unwrap(), &hues.entries[7]);
    }

    #[test]
    fn the_partial_flag_does_not_reach_the_index() {
        // 0x8000 rides in the same u16 as the number. Left in, it turns every
        // partial hue into a lookup miss and the item draws untinted.
        let hues = Hues::parse(&synthetic()).unwrap();
        let plain = hues.get(Hue(3)).unwrap();
        let partial = hues.get(Hue(3 | HUE_PARTIAL_FLAG)).unwrap();
        assert_eq!(plain, partial, "the flag selects the same palette");

        assert!(is_partial(Hue(3 | HUE_PARTIAL_FLAG)));
        assert!(!is_partial(Hue(3)));
        assert!(!is_partial(Hue::NONE));
    }

    #[test]
    fn making_none_partial_keeps_it_none() {
        assert_eq!(partial(Hue::NONE), Hue::NONE);
        assert_eq!(partial(Hue(3)), Hue(3 | HUE_PARTIAL_FLAG));
        assert_eq!(full(Hue(3 | HUE_PARTIAL_FLAG)), Hue(3));
    }

    #[test]
    fn a_hue_past_the_end_is_none_rather_than_a_panic() {
        // Hue ids come off the wire and out of scripts. One too large is a thing
        // to draw untinted, not a thing to crash on.
        let hues = Hues::parse(&synthetic()).unwrap();
        assert!(hues.get(Hue(HUES_PER_GROUP as u16)).is_some());
        assert!(hues.get(Hue(HUES_PER_GROUP as u16 + 1)).is_none());
        assert!(hues.get(Hue(u16::MAX)).is_none());
        assert!(hues.get(Hue(HUE_INDEX_MASK)).is_none());
    }

    #[test]
    fn a_file_that_is_not_whole_groups_is_refused() {
        assert!(Hues::parse(&[]).is_none());
        assert!(Hues::parse(&[0u8; GROUP_BYTES - 1]).is_none());
        assert!(Hues::parse(&[0u8; GROUP_BYTES + 1]).is_none());
        assert!(Hues::parse(&[0u8; GROUP_BYTES * 2]).is_some());
    }
}
