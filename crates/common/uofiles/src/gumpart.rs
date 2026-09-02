//! `gumpartLegacyMUL.uop`: the images UO's dialog windows are built from.
//!
//! `docs/client/evidence/2026-08-30-the-client-backlog.md`'s backlog called this out as the reason the client draws a
//! placeholder for every `{ gumppic }`: every one of the container's 5,556
//! entries carries compression flag 3, and [`crate::uop::Uop::entry`]
//! deliberately refuses to hand back anything compressed. This module is what
//! turns the flag on and reads the bytes underneath it.
//!
//! # Flag 3 is two passes, not one
//!
//! The obvious guess — "flag 3 means zlib, same as flag 1 elsewhere" — is only
//! half right, and the half that is wrong does not fail loudly: zlib inflates
//! every one of these entries cleanly, to exactly the length the container
//! declares, and the result *looks* like plausible file contents (small
//! integers, plenty of zeros) without being anything a gump reader can use.
//! ClassicUO's `UOFileUop.FillEntries` names flag 3 `CompressionType.ZlibBwt`,
//! and `GumpsLoader.GetGump` runs `BwtDecompress.Decompress` on the zlib
//! output before touching it as a picture. So: zlib first, always producing
//! [`crate::uop::RawEntry::decompressed_length`] bytes, then a second,
//! container-specific pass — [`bwt::decode`] — before there is a width, a
//! height, or a pixel anywhere in the data. Skipping the second pass is what
//! silently produces the huge, plausible-looking-but-wrong-in-every-way
//! numbers this crate's author found the first time through.
//!
//! The name `ZlibBwt` is ClassicUO's, and it overstates the second pass: it is
//! not a Burrows-Wheeler inverse. It is a move-to-front decode feeding a
//! frequency-table-driven run expansion. See [`bwt`] for the algorithm as
//! actually run, cited line for line against `BwtDecompress.cs`.
//!
//! # The picture format, once both passes are undone
//!
//! ```text
//!   header    u32 width, u32 height
//!   lookup    i32 per row: where that row's runs start, in dwords from the
//!             start of this header — the lookup table's own dwords included
//!   runs      u16 colour, u16 run length, repeated until the row's runs
//!             (found via the *next* row's lookup entry, or the end of the
//!             picture for the last row) are exhausted
//! ```
//!
//! This is not [`crate::art`]'s static format twice removed, and the two
//! differences both cost a wrong picture if missed:
//!
//! - **Width and height are not in a fixed-offset header field.** They are
//!   the first two words of the *decoded* data, because nothing outside the
//!   picture stores them — not the UOP entry (its `header_length` is 0 here:
//!   [`crate::uop::Uop::raw_entry`] never takes the "extra bytes in the
//!   container header" path gump art was expected to use, because ClassicUO's
//!   own loader only reads that path `if hasExtra && flag != 3` — and flag is
//!   3 on every entry). Ported from `GumpsLoader.GetGump`, which reads exactly
//!   these two fields off the decoded buffer before anything else.
//! - **A row has no terminator.** A static's runs end at an explicit `(0, 0)`;
//!   a gump's end where the *next* row's lookup entry says the data does, or
//!   at the end of the picture for the last row. There is nothing in a row's
//!   own bytes that says "stop here" — the boundary lives in the neighbour's
//!   lookup entry, which is why decoding row `y` needs `lookup[y + 1]`.
//!
//! Zero is transparent here, the same as a static and *unlike* land ground
//! (see [`crate::art::land_row`]): `GumpsLoader.GetGump` only produces an
//! opaque pixel `if (value != 0)`, leaving a zero pixel's alpha at zero.
//!
//! # Some entries really are 0x0, and that is not an error
//!
//! A shipped `gumpartLegacyMUL.uop` has roughly three dozen entries that
//! decode — cleanly, through both passes, matching every length the container
//! declares — to exactly eight zero bytes: a width/height header of `(0, 0)`
//! and nothing after it. The first one found (index 29, while sweeping the
//! whole container) looked like a decode bug: `decompressed_length` of 16,404
//! bytes collapsing to 8 is a dramatic ratio. It is not a bug. Every one of
//! these entries has the identical shape — 48 compressed bytes, 16,404
//! decompressed, the same handful of nonzero bytes at the front of the zlib
//! output — which is what a deliberately-emitted stub looks like and what a
//! truncated or corrupted one does not: corruption does not reproduce itself
//! bit for bit across three dozen unrelated entries.
//!
//! `GumpsLoader.GetGump` agrees without a special case: its early-out
//! (`entry.Width <= 0 && entry.Height <= 0`) never fires for a flag-3 entry —
//! see the width/height bullet above — so it falls through, decodes `w = h =
//! 0`, allocates a zero-length pixel span, and its per-row loop runs zero
//! times. The `GumpInfo` that comes out the other end has `Width = 0, Height =
//! 0` by ordinary control flow, not a caught error. So this reader treats it
//! the same way [`crate::art`] treats a land or static index the container
//! simply has no entry for: [`Gumps::gump`] returns `Ok(None)`, not
//! `Err`. A width and height that are zero on their *own* — one zero and the
//! other not — is a different, never-observed shape and stays an error: it is
//! not what the real empty entries look like, and guessing it means the same
//! thing would be exactly the kind of guess this module's other corner cases
//! were written to avoid.

use std::fmt;
use std::path::{
    Path,
    PathBuf,
};

use openshard_protocol::wire::Graphic;

use crate::color::Color16;
use crate::image::Image;
use crate::uop::{
    RawEntry,
    Uop,
    UopCompression,
    UopError,
};

mod bwt;

/// The compression flag every entry in a modern `gumpartLegacyMUL.uop`
/// carries: zlib, then [`bwt::decode`] on top. Named for ClassicUO's
/// `CompressionType.ZlibBwt`.
const FLAG_ZLIB_THEN_BWT: UopCompression = UopCompression::ZLIB_THEN_BWT;
/// The flag [`crate::uop::Uop::entry`] treats as "nothing to inflate" —
/// included here so a future container that ships a stored gump (no entry in
/// `gumpartLegacyMUL.uop` has ever done this) is read rather than rejected as
/// a mystery flag.
const FLAG_STORED: UopCompression = UopCompression::STORED;

/// The gump art could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum GumpError {
    /// The container could not be read.
    Container(UopError),
    /// An entry's compression flag is neither of the two this reader has ever
    /// seen a real container use. Not something to guess at: a wrong guess
    /// here would not fail to parse, it would parse into a wrong picture, the
    /// same trap flag 3 itself is.
    UnsupportedCompression {
        /// Which gump.
        graphic: Graphic,
        /// The flag the container recorded.
        flag:    u16,
    },
    /// An entry decoded but is not shaped like a picture.
    Malformed {
        /// Which gump.
        graphic: Graphic,
        /// What went wrong.
        detail:  String,
    },
}

impl fmt::Display for GumpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Container(source) => write!(f, "{source}"),
            Self::UnsupportedCompression { graphic, flag } => {
                write!(
                    f,
                    "gump {:#06X} uses compression flag {flag}, which this reader has never seen a real \
                 container use",
                    graphic.0
                )
            }
            Self::Malformed { graphic, detail } => {
                write!(f, "gump {:#06X} is malformed: {detail}", graphic.0)
            }
        }
    }
}

impl std::error::Error for GumpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Container(source) => Some(source),
            Self::UnsupportedCompression { .. } | Self::Malformed { .. } => None,
        }
    }
}

impl From<UopError> for GumpError {
    fn from(source: UopError) -> Self {
        Self::Container(source)
    }
}

/// The client's gump art, open and addressable.
#[derive(Debug)]
pub struct Gumps {
    container: Uop,
}

impl Gumps {
    /// Open `gumpartLegacyMUL.uop` in a client directory.
    pub fn open(client_dir: impl AsRef<Path>) -> Result<Self, GumpError> {
        let path: PathBuf = client_dir.as_ref().join("gumpartLegacyMUL.uop");
        Ok(Self {
            container: Uop::open(path)?,
        })
    }

    /// The name the client would have given entry `index`. Same `.tga`
    /// extension and same lie about the contents as [`crate::art`]'s — it is
    /// the string that was hashed, not a format.
    fn entry_name(index: usize) -> String {
        format!("build/gumpartlegacymul/{index:08}.tga")
    }

    /// The picture for a gump graphic, or `None` if the client ships none.
    ///
    /// A gump id is a `u16` (the container has room for exactly
    /// [`u16::MAX`] + 1 slots), so [`Graphic`] — already how this crate names
    /// art, tile, and gump ids alike — fits without truncation.
    pub fn gump(&self, graphic: Graphic) -> Result<Option<Image>, GumpError> {
        let Some(raw) = self.container.raw_entry(&Self::entry_name(graphic.0 as usize))? else {
            return Ok(None);
        };
        decode_entry(graphic, &raw)
    }

    /// Whether the client ships a picture for this graphic at all, without
    /// decoding one.
    ///
    /// The question a paperdoll asks before it asks for the picture: a worn
    /// item's gump is its `AnimID` plus a male or a female offset, and the
    /// female half of the file is *sparse* — the reference falls back to the
    /// male picture when the female one is missing rather than drawing nothing
    /// (`PaperDollInteractable.IsAnimExistsInGump`). Answering that with
    /// [`Gumps::gump`] would inflate and decode a whole image to throw it away,
    /// twice per worn layer.
    ///
    /// A container entry that exists but decodes to nothing still answers
    /// `true` here: that is the client's own 0x0 shape, and the caller that
    /// packs it will find out. The distinction the fallback needs is "the
    /// client has no such gump", which is a missing entry.
    pub fn has(&self, graphic: Graphic) -> Result<bool, GumpError> {
        Ok(self
            .container
            .raw_entry(&Self::entry_name(graphic.0 as usize))?
            .is_some())
    }
}

/// Undo whichever of the two passes `raw.compression` says were applied, then
/// decode the picture underneath. `Ok(None)` here is the explicit 0x0 shape
/// documented above, not a lookup miss — a lookup miss never reaches this
/// function, since [`Gumps::gump`] returns before calling it.
fn decode_entry(graphic: Graphic, raw: &RawEntry<'_>) -> Result<Option<Image>, GumpError> {
    let malformed = |detail: String| GumpError::Malformed { graphic, detail };

    let payload = match raw.compression {
        FLAG_STORED => raw.bytes.to_vec(),
        FLAG_ZLIB_THEN_BWT => {
            // Bounded by the length the container itself declares: this is
            // client data, not a proven-good buffer, and an unbounded inflate
            // of a truncated or corrupt entry is an OOM waiting to happen
            // rather than a decode error.
            let inflated =
                miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(raw.bytes, raw.decompressed_length)
                    .map_err(|error| {
                    malformed(format!(
                        "zlib stream did not inflate ({:?} after {} bytes)",
                        error.status,
                        error.output.len()
                    ))
                })?;
            if inflated.len() != raw.decompressed_length {
                return Err(malformed(format!(
                    "zlib produced {} bytes; the container declared {}",
                    inflated.len(),
                    raw.decompressed_length
                )));
            }
            bwt::decode(graphic, &inflated)?
        }
        flag => {
            return Err(GumpError::UnsupportedCompression {
                graphic,
                flag: flag.raw(),
            });
        }
    };

    decode_pixels(graphic, &payload)
}

/// Decode one picture out of bytes that have already had both compression
/// passes undone. Split out from [`decode_entry`] so the row format can be
/// tested on bytes built by hand, the same split [`crate::art::decode_static`]
/// makes for the same reason.
///
/// `Ok(None)` for the explicit `(0, 0)` shape documented in the module docs —
/// a real, deliberately-emitted "no picture here" rather than a decode
/// failure. Any other single zero dimension is `Err`: that shape has never
/// been observed on a real container, and treating it the same as `(0, 0)`
/// would be guessing that it means the same thing.
fn decode_pixels(graphic: Graphic, data: &[u8]) -> Result<Option<Image>, GumpError> {
    let malformed = |detail: String| GumpError::Malformed { graphic, detail };

    // Row offsets below are counted in dwords from here — the start of the
    // width/height header — not from the end of the lookup table. Ported from
    // `GumpsLoader.GetGump`, which captures `start` (its `reader.Position`)
    // *before* reading the lookup table, then seeks to
    // `start + rowLookup[y] * 4` for each row. Counting from the end of the
    // table instead is the same one-hop-too-far mistake `art.rs`'s land tiles
    // made about padding, in the opposite direction: it would read every
    // row `height * 4` bytes too early, and on this format that reads the
    // *previous* row's tail as though it were pixels rather than an
    // out-of-bounds panic, so nothing would say so.
    const HEADER: usize = 8;

    let header = data
        .get(..HEADER)
        .ok_or_else(|| malformed("shorter than its own width/height header".to_owned()))?;
    let width = u32::from_le_bytes(header[0..4].try_into().expect("4-byte slice"));
    let height = u32::from_le_bytes(header[4..8].try_into().expect("4-byte slice"));
    if width == 0 && height == 0 {
        return Ok(None);
    }
    if width == 0 || height == 0 {
        return Err(malformed(format!(
            "{width}x{height} has exactly one dimension zero; every empty entry seen on a real \
             client is 0x0, not this"
        )));
    }
    let width_u16 =
        u16::try_from(width).map_err(|_| malformed(format!("width {width} is too wide for a gump")))?;
    let height_u16 =
        u16::try_from(height).map_err(|_| malformed(format!("height {height} is too tall for a gump")))?;
    let width = width as usize;
    let height = height as usize;

    let lookup_bytes = height * 4;
    let lookup_raw = data
        .get(HEADER..HEADER + lookup_bytes)
        .ok_or_else(|| malformed(format!("no room for a {height}-row lookup table")))?;
    let lookup: Vec<i32> = lookup_raw
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| i32::from_le_bytes(*word))
        .collect();

    // How far the picture runs, in (colour, run) dword-pairs, counted from
    // `HEADER` the same way the lookup table's entries are — the lookup
    // table's own dwords included. This is what bounds the *last* row's
    // data, since nothing in the format marks a row's end the way a static's
    // `(0, 0)` run does; every other row's end is simply the next row's
    // lookup entry.
    let pairs_total = (data.len() - HEADER) / 4;

    let mut pixels = vec![Color16::TRANSPARENT; width * height];
    for y in 0..height {
        let row_start = usize::try_from(lookup[y])
            .map_err(|_| malformed(format!("row {y} has a negative lookup offset")))?;
        let row_end = if y + 1 < height {
            usize::try_from(lookup[y + 1])
                .map_err(|_| malformed(format!("row {} has a negative lookup offset", y + 1)))?
        } else {
            pairs_total
        };
        let pair_count = row_end
            .checked_sub(row_start)
            .ok_or_else(|| malformed(format!("row {y} ends ({row_end}) before it starts ({row_start})")))?;

        let start = HEADER + row_start * 4;
        let row_data = data
            .get(start..start + pair_count * 4)
            .ok_or_else(|| malformed(format!("row {y} runs past the end of the picture")))?;

        let mut x = 0usize;
        for pair in row_data.as_chunks::<4>().0 {
            let color = Color16(u16::from_le_bytes([pair[0], pair[1]]));
            let run = usize::from(u16::from_le_bytes([pair[2], pair[3]]));
            if x + run > width {
                return Err(malformed(format!(
                    "row {y} draws to column {} on a gump {width} wide",
                    x + run
                )));
            }
            pixels[y * width + x..y * width + x + run].fill(color);
            x += run;
        }
    }

    Ok(Some(Image::new(width_u16, height_u16, pixels)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `(colour, run)` pair, little-endian.
    fn run(color: u16, length: u16) -> [u8; 4] {
        let mut bytes = [0u8; 4];
        bytes[0..2].copy_from_slice(&color.to_le_bytes());
        bytes[2..4].copy_from_slice(&length.to_le_bytes());
        bytes
    }

    /// A 4x2 picture with **no run terminator** — the difference from
    /// `art.rs`'s static fixture that this format actually needs tested: row
    /// 0's data ends exactly where row 1's lookup entry says it does, and
    /// there is nothing in row 0's own bytes that would say so if the lookup
    /// table were wrong.
    fn synthetic_picture() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&4u32.to_le_bytes()); // width
        bytes.extend_from_slice(&2u32.to_le_bytes()); // height

        // Row 0: one run of 2 transparent, then 2 opaque. Row 1: 4 opaque.
        // Two runs in row 0 is deliberate, the same reason `art.rs` insists
        // on it for statics: with one run per row nothing distinguishes "the
        // row's data" from "the row's only run", and a lookup-table bug that
        // only shows up with more than one run per row would pass anyway.
        let row0 = [run(0, 2), run(0x1234, 2)].concat();
        let row1 = [run(0x5678, 4)].concat();

        // Lookup entries are dwords from `HEADER` (byte 8), table included:
        // row 0 starts right after the 2-entry (8-byte) table, at dword 2.
        let row0_dwords = 2u32; // the table itself: 2 rows * 4 bytes / 4
        bytes.extend_from_slice(&row0_dwords.to_le_bytes());
        let row1_dwords = row0_dwords + (row0.len() / 4) as u32;
        bytes.extend_from_slice(&row1_dwords.to_le_bytes());

        bytes.extend_from_slice(&row0);
        bytes.extend_from_slice(&row1);
        bytes
    }

    #[test]
    fn a_row_ends_where_the_next_rows_lookup_entry_says_and_not_at_a_marker() {
        let image = decode_pixels(Graphic(0), &synthetic_picture())
            .unwrap()
            .expect("a 4x2 picture is not the empty-stub shape");
        assert_eq!((image.width(), image.height()), (4, 2));

        assert_eq!(image.pixel(0, 0), Some(Color16::TRANSPARENT));
        assert_eq!(image.pixel(1, 0), Some(Color16::TRANSPARENT));
        assert_eq!(image.pixel(2, 0), Some(Color16(0x1234)));
        assert_eq!(image.pixel(3, 0), Some(Color16(0x1234)));

        for x in 0..4 {
            assert_eq!(image.pixel(x, 1), Some(Color16(0x5678)), "row 1 has no gap");
        }
    }

    #[test]
    fn a_run_that_overflows_its_row_is_refused_rather_than_wrapping() {
        let mut bytes = synthetic_picture();
        // Row 0's second run claims 40 pixels on a 4-wide picture.
        let second_run_length_at = 8 + 8 + 4 + 2; // header + lookup + first run + this run's colour
        bytes[second_run_length_at..second_run_length_at + 2].copy_from_slice(&40u16.to_le_bytes());
        let error = decode_pixels(Graphic(0x1234), &bytes).unwrap_err();
        assert!(matches!(error, GumpError::Malformed { .. }), "{error}");
    }

    #[test]
    fn a_picture_with_only_one_dimension_zero_is_refused() {
        // Width 0, height 2 (left alone from `synthetic_picture`) — not the
        // `(0, 0)` shape a real empty entry actually has, so this stays an
        // error rather than being folded into the same `Ok(None)`.
        let mut bytes = synthetic_picture();
        bytes[0..4].copy_from_slice(&0u32.to_le_bytes());
        assert!(decode_pixels(Graphic(1), &bytes).is_err());
    }

    #[test]
    fn a_picture_that_is_exactly_zero_by_zero_is_no_picture_not_an_error() {
        // The shape a real, deliberately-emitted empty gump entry decodes to
        // (see the module docs and `gump_29_is_a_real_empty_entry_not_a_bug`
        // below): a width/height header of `(0, 0)` and nothing else.
        let bytes = [0u8; 8];
        assert_eq!(decode_pixels(Graphic(1), &bytes).unwrap(), None);
    }

    #[test]
    fn a_truncated_picture_is_refused_rather_than_panicking() {
        // Not `full.len() - 2`: the last row's length is never stored
        // explicitly (see the module docs — it runs to the end of the
        // picture), so trimming a couple of bytes off the very end is
        // indistinguishable from "the last row has one fewer pixel of run",
        // and this format has no way to tell those apart. Every cut here
        // instead lands inside the header, the lookup table, or a row that is
        // *not* last, where a shorter file has no legal reading.
        let full = synthetic_picture();
        for cut in [0, 4, 7, 9, 20] {
            assert!(
                decode_pixels(Graphic(1), &full[..cut]).is_err(),
                "cut to {cut} bytes parsed"
            );
        }
    }

    #[test]
    fn a_lookup_table_that_runs_backwards_is_refused() {
        // Row 0 pointing past where row 1 starts is not a picture this format
        // can express — checked explicitly rather than left to wrap a
        // `usize` subtraction, which `checked_sub` is here to catch.
        let mut bytes = synthetic_picture();
        bytes[8..12].copy_from_slice(&999u32.to_le_bytes());
        assert!(decode_pixels(Graphic(1), &bytes).is_err());
    }

    /// Point `OPENSHARD_CLIENT` at a UO client install to run the tests below.
    /// They skip when it is unset, the same rule every other real-file test
    /// in this crate follows.
    fn client_dir() -> Option<PathBuf> {
        let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
        dir.join("gumpartLegacyMUL.uop").exists().then_some(dir)
    }

    #[test]
    fn a_real_containers_entries_are_flag_three_and_this_reader_handles_it() {
        // The claim the whole module rests on, checked against a shipped
        // file rather than assumed from `docs/client/evidence/2026-08-30-the-client-backlog.md`'s backlog note: a
        // modern client really does mark every gump entry flag 3, and this is
        // what says the `UnsupportedCompression` arm of `decode_entry` is not
        // one this client ever exercises.
        let Some(dir) = client_dir() else {
            return;
        };
        let gumps = Gumps::open(&dir).unwrap();

        // Two known icons, decoded and checked on real numbers rather than
        // "it did not error" — an empty or near-empty picture passing this
        // test would be exactly the false green this crate's tests are
        // written to avoid. Both were read by hand off the shipped
        // `gumpartLegacyMUL.uop` (see the session that wrote this module) by
        // running the same zlib-then-bwt-then-rows pipeline independently in
        // Python and cross-checking dimensions and fill fraction before
        // trusting either as a fixture.
        let small = gumps
            .gump(Graphic(3))
            .unwrap()
            .expect("gump 3 ships on a modern client");
        assert_eq!((small.width(), small.height()), (16, 14));
        let small_opaque = small.pixels().iter().filter(|p| !p.is_transparent()).count();
        assert_eq!(small_opaque, 195, "gump 3's opaque pixel count");
        // A pixel from inside the glyph rather than the transparent border,
        // so a reader that decoded *a* picture the right size but the wrong
        // content would still be caught.
        assert!(
            !small.pixel(8, 7).unwrap().is_transparent(),
            "the centre of gump 3 is drawn, not background"
        );

        let solid = gumps
            .gump(Graphic(1))
            .unwrap()
            .expect("gump 1 ships on a modern client");
        assert_eq!((solid.width(), solid.height()), (80, 40));
        assert!(
            solid.pixels().iter().all(|p| !p.is_transparent()),
            "gump 1 is a fully opaque icon"
        );
        assert_eq!(
            solid.pixel(0, 0),
            Some(Color16(0x2008)),
            "gump 1's top-left pixel"
        );
    }

    #[test]
    fn gump_29_is_a_real_empty_entry_not_a_bug() {
        // Found by the exhaustive sweep below: index 29 decodes — cleanly,
        // through both compression passes, matching every length the
        // container declares — to a width/height header of `(0, 0)`. The
        // container really does have an entry for it (compressed_length 48,
        // decompressed_length 16,404), so this is not the "no entry at all"
        // case `gump` also spells `None`; it is a picture that is, itself,
        // deliberately empty. See the module docs for how `GumpsLoader`
        // reaches the same answer without a special case.
        let Some(dir) = client_dir() else {
            return;
        };
        let gumps = Gumps::open(&dir).unwrap();

        assert!(
            gumps
                .container
                .raw_entry(&Gumps::entry_name(29))
                .unwrap()
                .is_some(),
            "gump 29 is a real entry in the container, not an absent index"
        );
        assert_eq!(
            gumps.gump(Graphic(29)).unwrap(),
            None,
            "gump 29 decodes to (0, 0), which is 'no picture', not an error"
        );
    }

    /// Every picture the container ships, decoded — the property two sampled
    /// gumps cannot establish.
    ///
    /// The two above are fixtures: they say *these* bytes come out right. What
    /// they cannot say is that the second decompression pass is right in
    /// general, and that is the pass most likely to be subtly wrong — a
    /// mis-ported move-to-front decode produces plausible-looking output
    /// (small numbers, plenty of zeroes) that inflates to the declared length
    /// and decodes into *a* picture. What catches it is the format checking
    /// itself thousands of times over: a wrong byte stream almost never yields
    /// a lookup table whose rows each fill their width exactly.
    ///
    /// `#[ignore]` because it reads the whole 5,500-entry container: this is a
    /// shared tree, and a sweep belongs to whoever asks for it rather than to
    /// everyone running `cargo test`. Run it with
    /// `cargo test -p openshard-uofiles -- --ignored`.
    #[test]
    #[ignore = "reads every entry of a 5,500-picture container"]
    fn every_picture_the_client_ships_decodes() {
        let Some(dir) = client_dir() else {
            return;
        };
        let gumps = Gumps::open(&dir).unwrap();

        let mut decoded = 0usize;
        let mut container_absent = 0usize;
        let mut empty_stub = 0usize;
        for id in 0..=u16::MAX {
            match gumps.gump(Graphic(id)) {
                // The size is the decode's own claim, so it is worth one
                // assertion: a zero dimension means the rows below it were
                // never read at all, which is how an "everything passed"
                // sweep over nothing would look. `decode_pixels` already
                // turns an exact `(0, 0)` into `None` before this arm ever
                // sees it, so a nonzero pair here is not a tautology.
                Ok(Some(image)) => {
                    assert!(
                        image.width() > 0 && image.height() > 0,
                        "gump {id} decoded to an empty picture"
                    );
                    decoded += 1;
                }
                Ok(None) => {
                    // `None` means two different things on the wire and only
                    // one of them is "this index is unused" — a container
                    // this sparse (roughly a third of `u16::MAX` slots filled)
                    // makes that the common case, so it is not asserted on its
                    // own. What is checked is the *other* reading: an entry
                    // that is present and decodes cleanly to the explicit
                    // `(0, 0)` shape documented in the module docs. Telling
                    // the two apart needs the container directly —
                    // `Gumps::gump` deliberately does not, the same way
                    // `crate::art` does not say why a graphic has no sprite.
                    let present = gumps
                        .container
                        .raw_entry(&Gumps::entry_name(id as usize))
                        .unwrap()
                        .is_some();
                    if present {
                        empty_stub += 1;
                    } else {
                        container_absent += 1;
                    }
                }
                Err(error) => panic!("gump {id} did not decode: {error}"),
            }
        }

        // The counts are asserted, not printed. A sweep that decoded nothing
        // is the false green this is written against — see `docs/style.md` on
        // tests that pass without checking anything.
        assert!(
            decoded > 5_000,
            "only {decoded} pictures decoded ({container_absent} slots empty, {empty_stub} \
             explicit 0x0 stubs): a modern client ships upwards of five thousand"
        );
        // A real container has a handful of these — 34, on the client this
        // was written against — because the shape is genuine, not because
        // this reader is quietly turning real pictures into empty ones. A
        // count anywhere near `decoded` would be exactly that bug, wearing
        // this test's own "expected" branch as cover.
        assert!(
            empty_stub < 200,
            "{empty_stub} entries decoded to an explicit 0x0 — far more than the handful of real \
             stub records this was written against; something is turning real pictures into empty \
             ones instead of reading them"
        );
    }
}
