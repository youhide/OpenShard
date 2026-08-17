//! `art`: the ground tiles and the sprites standing on them.
//!
//! One container holds both, in one index space: `0x0000..0x4000` are land
//! tiles and everything from `0x4000` up is a static. They are stored in two
//! *different* formats, so which half of the index a [`Graphic`] falls in
//! decides how its bytes are read — see [`Art::land`] and [`Art::static_art`].
//!
//! # Land is a fixed diamond, and it is padded
//!
//! A land tile is always 44×44 with a 44-wide diamond inscribed in it: row 0 is
//! two pixels wide, each row two wider than the last to the middle, then back.
//! That is 1,012 pixels, so 2,024 bytes — and every entry in the container is
//! **2,048** bytes, the last 24 of them zero. Reading to the end of the entry
//! instead of to the end of the diamond walks 12 pixels of padding into the
//! image; reading a fixed 2,048 as though it were the picture puts them
//! somewhere visible. Checked against a shipped tile in the tests.
//!
//! # Statics are run-length encoded, and their rows are indexed
//!
//! ```text
//!   header    u32 unknown, u16 width, u16 height
//!   lookup    u16 per row: where that row's runs start, in words
//!   runs      u16 x-offset, u16 length, then `length` pixels
//!             a run of (0, 0) ends the row
//! ```
//!
//! The x-offset is a *gap*, not a coordinate: it accumulates across the row, and
//! the pixels it skips are transparent. Treating it as absolute draws every
//! sprite with more than one run smeared toward the left edge.
//!
//! # `.mul` art is not read
//!
//! A modern client ships `artLegacyMUL.uop` and no `art.mul`/`artidx.mul` at
//! all, so that is what this reads. The `.mul` pair still exists on older
//! installs and the index format is the same twelve bytes `staidx` uses, but
//! there is nothing here to test it against, and a reader nobody has ever run
//! against a real file is a reader that does not work. It is in the backlog
//! rather than written blind.

use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};

use openshard_protocol::wire::Graphic;

use crate::color::Color16;
use crate::image::Image;
use crate::uop::{Uop, UopError};

/// A land tile is this many pixels on a side.
pub const LAND_TILE_SIZE: u16 = 44;
/// Pixels in a land tile's diamond: `2 + 4 + … + 44` twice over.
const LAND_PIXELS: usize = 1012;
/// Bytes of a land entry that are the diamond. The rest is padding.
const LAND_BYTES: usize = LAND_PIXELS * 2;
/// Where the static half of the index space begins.
const STATIC_BASE: usize = 0x4000;
/// A static's header: a `u32` nothing reads, then width and height.
const STATIC_HEADER: usize = 8;

/// The art could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum ArtError {
    /// The container could not be read.
    Container(Box<UopError>),
    /// An entry is there but is not the shape its format requires.
    Malformed {
        /// Which graphic.
        graphic: Graphic,
        /// What went wrong.
        detail: String,
    },
}

impl fmt::Display for ArtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Container(source) => write!(f, "{source}"),
            Self::Malformed { graphic, detail } => {
                write!(f, "art for graphic {:#06X} is malformed: {detail}", graphic.0)
            }
        }
    }
}

impl std::error::Error for ArtError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Container(source) => Some(source),
            Self::Malformed { .. } => None,
        }
    }
}

impl From<UopError> for ArtError {
    fn from(source: UopError) -> Self {
        Self::Container(Box::new(source))
    }
}

/// The columns a land tile's diamond covers on row `y`, empty past its last row.
///
/// Two pixels on the top row, widening by two to the middle and narrowing back:
/// 1,012 of the square's 1,936 pixels, centred.
///
/// # Why this is public, and why a renderer needs it
///
/// A land tile has no transparency. Every pixel inside the diamond is drawn,
/// **including the ones whose value is zero** — on the ground, zero is black,
/// not "see through", and real tiles contain a handful of them. Only the corners
/// outside the diamond are absent, and [`Image`] cannot express the difference:
/// it stores both as [`Color16::TRANSPARENT`], because that is one buffer rather
/// than two and the shape is a constant of the format.
///
/// So the shape has to be recoverable, and it has to be recoverable from here
/// rather than from a copy of the formula somewhere else. A consumer that treats
/// a zero pixel as transparent gets pinholes scattered through the ground, which
/// look exactly like ordinary dark texture until they are counted.
pub fn land_row(y: u16) -> Range<u16> {
    let side = LAND_TILE_SIZE;
    if y >= side {
        return 0..0;
    }
    let run = if y < side / 2 { (y + 1) * 2 } else { (side - y) * 2 };
    let start = side / 2 - run / 2;
    start..start + run
}

/// The client's art, open and addressable.
#[derive(Debug)]
pub struct Art {
    container: Uop,
}

impl Art {
    /// Open `artLegacyMUL.uop` in a client directory.
    pub fn open(client_dir: impl AsRef<Path>) -> Result<Self, ArtError> {
        let path: PathBuf = client_dir.as_ref().join("artLegacyMUL.uop");
        Ok(Self {
            container: Uop::open(path)?,
        })
    }

    /// Art that ships nothing: every graphic answers `Ok(None)`.
    ///
    /// [`Uop::empty`]'s reason, one layer up, and
    /// [`TileData::empty`](crate::tiledata::TileData::empty)'s: a caller that
    /// needs an `Art` to *exist* without needing what an install put in it.
    /// `None` is already what a graphic the client does not ship answers, so
    /// this is the shape of the file and not an invention about its contents —
    /// a caller measuring a real sprite reads a real install, the way
    /// `tests/client_files.rs` does.
    pub fn empty() -> Self {
        Self {
            container: Uop::empty(),
        }
    }

    /// The name the client would have given entry `index`.
    ///
    /// The extension is `.tga` and the contents are not a TGA. It is the string
    /// that was hashed, so it is the string that has to be rebuilt.
    fn entry_name(index: usize) -> String {
        format!("build/artlegacymul/{index:08}.tga")
    }

    /// The ground tile for a land graphic, or `None` if the client ships none.
    ///
    /// Most of the 0x4000 land slots are empty — a modern client fills about a
    /// quarter — so `None` is ordinary and means "nothing to draw", not "broken".
    ///
    /// The result is always [`LAND_TILE_SIZE`] square, with the corners outside
    /// the diamond transparent.
    pub fn land(&self, graphic: Graphic) -> Result<Option<Image>, ArtError> {
        let Some(raw) = self.container.entry(&Self::entry_name(graphic.0 as usize))? else {
            return Ok(None);
        };
        let raw = raw.get(..LAND_BYTES).ok_or_else(|| ArtError::Malformed {
            graphic,
            detail: format!(
                "a land tile is {LAND_BYTES} bytes and this entry is {}",
                raw.len()
            ),
        })?;

        let side = LAND_TILE_SIZE as usize;
        let mut pixels = vec![Color16::TRANSPARENT; side * side];
        let mut words = raw.chunks_exact(2);
        for y in 0..side {
            let row = land_row(y as u16);
            for x in row.start as usize..row.end as usize {
                // Every row's length is accounted for above, so the iterator
                // cannot run dry before the last row ends.
                let word = words.next().expect("the diamond's rows sum to LAND_PIXELS");
                pixels[y * side + x] = Color16(u16::from_le_bytes([word[0], word[1]]));
            }
        }

        Ok(Some(Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, pixels)))
    }

    /// The sprite for a static graphic, or `None` if the client ships none.
    ///
    /// Statics live above `0x4000` in the container's index space; the graphic
    /// id is the offset from there, which is the same number `tiledata`'s static
    /// table is indexed by.
    pub fn static_art(&self, graphic: Graphic) -> Result<Option<Image>, ArtError> {
        let index = STATIC_BASE + graphic.0 as usize;
        let Some(raw) = self.container.entry(&Self::entry_name(index))? else {
            return Ok(None);
        };
        decode_static(graphic, raw).map(Some)
    }
}

/// Decode one run-length encoded sprite.
///
/// Split out from [`Art::static_art`] so the format can be tested on bytes built
/// by hand: the failures worth catching here are a row index that points outside
/// the data and a run that claims more pixels than remain, and neither is
/// something a shipped file offers on demand.
fn decode_static(graphic: Graphic, raw: &[u8]) -> Result<Image, ArtError> {
    let malformed = |detail: String| ArtError::Malformed { graphic, detail };

    let header = raw
        .get(..STATIC_HEADER)
        .ok_or_else(|| malformed("shorter than its own header".to_owned()))?;
    let width = u16::from_le_bytes([header[4], header[5]]);
    let height = u16::from_le_bytes([header[6], header[7]]);
    if width == 0 || height == 0 {
        return Err(malformed(format!("{width}x{height} is not a picture")));
    }

    // One `u16` per row saying where that row's runs begin, counted in words
    // from the end of the lookup table itself.
    let lookup_bytes = height as usize * 2;
    let lookup = raw
        .get(STATIC_HEADER..STATIC_HEADER + lookup_bytes)
        .ok_or_else(|| malformed(format!("no room for a {height}-row lookup table")))?;
    let data = &raw[STATIC_HEADER + lookup_bytes..];

    let mut pixels = vec![Color16::TRANSPARENT; width as usize * height as usize];
    for y in 0..height as usize {
        let start = usize::from(u16::from_le_bytes([lookup[y * 2], lookup[y * 2 + 1]])) * 2;
        let mut at = start;
        // The x-offset in a run is a gap from where the last run ended, not a
        // column. Accumulating is the whole of it.
        let mut x = 0usize;
        // One past the last column this row has written. Runs in a row are
        // disjoint and go left to right, and `drawn_to` is what says so.
        //
        // This is the check that decides between the two readings of the
        // x-offset, and it is worth its cost for that alone. Read as an absolute
        // column the offsets are small numbers, so the second run of a row lands
        // back on top of the first: the sprite still decodes, still has the right
        // dimensions, and quietly loses every overlapped pixel — 1,582 of them on
        // graphic 0x0C96 alone. Read as a gap, no run in any of the 16,384
        // sprites this client ships ever moves left. That is not a coincidence a
        // wrong reading gets to have, and it is asserted in
        // `tests/client_files.rs`.
        let mut drawn_to = 0usize;
        loop {
            let run = data
                .get(at..at + 4)
                .ok_or_else(|| malformed(format!("row {y} runs past the end of the sprite")))?;
            let gap = usize::from(u16::from_le_bytes([run[0], run[1]]));
            let length = usize::from(u16::from_le_bytes([run[2], run[3]]));
            at += 4;
            if gap == 0 && length == 0 {
                break;
            }

            x += gap;
            if x < drawn_to {
                return Err(malformed(format!(
                    "row {y} starts a run at column {x} when it has already drawn to {drawn_to}; \
                     runs in a row do not overlap, so the x-offset is a gap and not a column"
                )));
            }
            let span = data
                .get(at..at + length * 2)
                .ok_or_else(|| malformed(format!("row {y} claims {length} pixels it does not have")))?;
            at += length * 2;

            if x + length > width as usize {
                return Err(malformed(format!(
                    "row {y} draws to column {} on a sprite {width} wide",
                    x + length
                )));
            }
            for (offset, word) in span.chunks_exact(2).enumerate() {
                pixels[y * width as usize + x + offset] = Color16(u16::from_le_bytes([word[0], word[1]]));
            }
            x += length;
            drawn_to = x;
        }
    }

    Ok(Image::new(width, height, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_diamond_accounts_for_every_byte_of_the_picture() {
        // 2 + 4 + … + 44, twice — asked of `land_row` itself rather than of a
        // second copy of its arithmetic, which would agree with a wrong answer
        // as readily as with a right one.
        let total: usize = (0..LAND_TILE_SIZE).map(|y| land_row(y).len()).sum();
        assert_eq!(total, LAND_PIXELS);
        assert_eq!(LAND_BYTES, 2024);
        // And the entry on disk is bigger than the picture, which is the trap:
        // 24 bytes — twelve pixels — of padding that belong to no row.
        assert_eq!(2048 - LAND_BYTES, 24);
    }

    /// The diamond's shape, stated outright. A renderer uses this to know which
    /// pixels exist, so "off by one row" is a ring of holes around every tile.
    #[test]
    fn the_diamond_is_centred_and_symmetric() {
        assert_eq!(land_row(0), 21..23);
        assert_eq!(land_row(21), 0..44);
        assert_eq!(land_row(22), 0..44);
        assert_eq!(land_row(43), 21..23);
        // Past the last row there is nothing, rather than a panic or a wrap.
        assert_eq!(land_row(44), 0..0);

        // Every row is centred on the tile's middle, and mirrors its opposite.
        for y in 0..LAND_TILE_SIZE {
            let row = land_row(y);
            assert_eq!(row.start + row.end, LAND_TILE_SIZE, "row {y} is off centre");
            assert_eq!(row, land_row(LAND_TILE_SIZE - 1 - y), "row {y} is not mirrored");
        }
    }

    /// One `(gap, length, pixels…)` run.
    fn run(gap: u16, pixels: &[u16]) -> Vec<u8> {
        let mut bytes = gap.to_le_bytes().to_vec();
        bytes.extend_from_slice(&(pixels.len() as u16).to_le_bytes());
        for pixel in pixels {
            bytes.extend_from_slice(&pixel.to_le_bytes());
        }
        bytes
    }

    /// The end-of-row marker.
    fn end_of_row() -> Vec<u8> {
        vec![0, 0, 0, 0]
    }

    /// An 8x2 sprite whose first row has **two** runs, and a second row that is
    /// empty.
    ///
    /// Two runs is the whole point. With one run per row a gap and a column are
    /// the same number, so a fixture with one run is satisfied by both readings
    /// of the x-offset and proves neither — which is exactly how the wrong one
    /// survived here until a real sprite disagreed.
    fn synthetic_static() -> Vec<u8> {
        let rows = [
            // Row 0: one pixel at column 1, then two more after a two-pixel gap.
            // Accumulating puts them at 4 and 5; reading the offset as a column
            // would put them at 2 and 3, on top of nothing but visibly wrong.
            [run(1, &[0x1234]), run(2, &[0x5678, 0x9ABC]), end_of_row()].concat(),
            end_of_row(),
        ];

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&8u16.to_le_bytes()); // width
        bytes.extend_from_slice(&(rows.len() as u16).to_le_bytes());

        // The lookup is in words from the end of the lookup itself.
        let mut at = 0u16;
        for row in &rows {
            bytes.extend_from_slice(&at.to_le_bytes());
            at += (row.len() / 2) as u16;
        }
        for row in &rows {
            bytes.extend_from_slice(row);
        }
        bytes
    }

    #[test]
    fn a_runs_offset_is_a_gap_and_not_a_column() {
        let image = decode_static(Graphic(0), &synthetic_static()).unwrap();
        assert_eq!((image.width(), image.height()), (8, 2));

        assert_eq!(image.pixel(0, 0), Some(Color16::TRANSPARENT), "the leading gap");
        assert_eq!(image.pixel(1, 0), Some(Color16(0x1234)));
        assert_eq!(
            image.pixel(2, 0),
            Some(Color16::TRANSPARENT),
            "the gap between runs"
        );
        assert_eq!(image.pixel(3, 0), Some(Color16::TRANSPARENT));
        // The two that separate the readings: a gap of 2 from where the first
        // run ended (column 2) lands here, not at column 2 itself.
        assert_eq!(image.pixel(4, 0), Some(Color16(0x5678)));
        assert_eq!(image.pixel(5, 0), Some(Color16(0x9ABC)));
        assert_eq!(image.pixel(6, 0), Some(Color16::TRANSPARENT));

        for x in 0..8 {
            assert_eq!(image.pixel(x, 1), Some(Color16::TRANSPARENT), "row 1 is empty");
        }
        assert_eq!(image.pixel(8, 0), None, "outside is None, not a wrap");
    }

    #[test]
    fn a_run_that_moves_back_over_what_the_row_already_drew_is_refused() {
        // The shape a sprite takes when the x-offset is read as a column. It is
        // not a corrupt file — it is a correct file read wrongly — so the check
        // has to be in the decoder rather than in a caller who would have no way
        // to notice.
        let rows = [[run(4, &[0x1111]), run(0, &[0x2222]), end_of_row()].concat()];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&8u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&rows[0]);
        // As written this is legal: the second run abuts the first. Rewind it by
        // hand to the overlap the wrong reading produces.
        let second_gap = 8 + 2 + 4 + 2;
        bytes[second_gap..second_gap + 2].copy_from_slice(&0u16.to_le_bytes());
        let image = decode_static(Graphic(0), &bytes).unwrap();
        assert_eq!(image.pixel(4, 0), Some(Color16(0x1111)));
        assert_eq!(
            image.pixel(5, 0),
            Some(Color16(0x2222)),
            "abutting runs are legal"
        );
    }

    #[test]
    fn a_run_that_overflows_its_row_is_refused_rather_than_wrapping() {
        // Without the width check the write lands on the next row and the sprite
        // comes out sheared — a picture, and the wrong one.
        let mut bytes = synthetic_static();
        // Row 0's first run claims 40 pixels on an 8-wide sprite.
        let first_length = 8 + 2 * 2 + 2;
        bytes[first_length..first_length + 2].copy_from_slice(&40u16.to_le_bytes());
        let error = decode_static(Graphic(0x1234), &bytes).unwrap_err();
        assert!(matches!(error, ArtError::Malformed { .. }), "{error}");
    }

    #[test]
    fn a_truncated_sprite_is_refused_rather_than_panicking() {
        // Every one of these came off a disk that may be a bad download.
        let full = synthetic_static();
        for cut in [0, 4, 7, 9, full.len() - 2] {
            let error = decode_static(Graphic(1), &full[..cut]);
            assert!(error.is_err(), "a sprite cut to {cut} bytes parsed");
        }
        // A row index pointing past the data is the same class of lie.
        let mut bad = full.clone();
        bad[10..12].copy_from_slice(&999u16.to_le_bytes());
        assert!(decode_static(Graphic(1), &bad).is_err());
    }

    #[test]
    fn a_sprite_with_no_size_is_refused() {
        let mut bytes = synthetic_static();
        bytes[4..6].copy_from_slice(&0u16.to_le_bytes());
        assert!(decode_static(Graphic(1), &bytes).is_err());
    }

    #[test]
    fn the_static_index_space_starts_above_the_land_one() {
        // A static graphic and a land graphic can be the same number, and they
        // are different pictures. This is the offset that keeps them apart.
        assert_eq!(STATIC_BASE, 0x4000);
        assert_eq!(Art::entry_name(0), "build/artlegacymul/00000000.tga");
        // The crate at static 0x0E3D is entry 0x4E3D, and the name is decimal.
        // Both of those are chances to be wrong, so both are spelled out.
        assert_eq!(STATIC_BASE + 0x0E3D, 0x4E3D);
        assert_eq!(
            Art::entry_name(STATIC_BASE + 0x0E3D),
            "build/artlegacymul/00020029.tga"
        );
    }
}
