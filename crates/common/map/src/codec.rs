//! One chunk as bytes, and bytes back into one chunk.
//!
//! **Canonical**: one world encodes to exactly one byte string, and a chunk
//! decoded and re-encoded is the same bytes. That is what lets an import be
//! checked by comparing blobs rather than by walking two worlds and asking
//! whether they agree, and it is what makes a content hash mean anything at
//! all.
//!
//! Canonical rests on one property of the layer below: a chunk's statics are in
//! the `(y, x)` stable order [`Map::from_parts`](crate::map::Map::from_parts) imposes, so re-cutting a chunk
//! out of an assembled facet reproduces the order it went in with. Nothing here
//! sorts — if this module had its own sort there would be two of them.
//!
//! # The record
//!
//! ```text
//! header, 24 bytes
//!   0  4  magic "OSMC"
//!   4  1  version
//!   5  1  facet
//!   6  2  chunk x            u16
//!   8  2  chunk y            u16
//!  10  8  revision           u64
//!  18  1  blocks wide, 1..=8
//!  19  1  blocks down, 1..=8
//!  20  4  static count       u32
//! land, 3 bytes a cell, blocks in the chunk's order, cells row-major
//!   0  2  land tile          u16
//!   2  1  height             i8
//! counts, 4 bytes a block, in the chunk's order
//!   0  4  statics in it      u32
//! statics, 6 bytes each, blocks in order and each block in its own order
//!   0  2  graphic            u16
//!   2  1  position in block, y in the high three bits, x in the low three
//!   3  1  height             i8
//!   4  2  hue                u16
//! ```
//!
//! Little-endian, like every other file this workspace writes and unlike the
//! wire, which is UO's and big-endian. A chunk is not a UO packet and there is
//! no reason for it to be shaped like one.
//!
//! # What is not in it, and why
//!
//! - **A hue table.** Only 0.95% of Felucca's statics are hued, so an inline
//!   `u16` is two dead bytes on 99 items in 100 — 5.6 MiB across the facet, on
//!   a base set of 137. A sparse side table would save that and cost a second
//!   resolve on the way in for every item; the four percent is not worth the
//!   second lookup.
//! - **A draw order.** `client_today.md`'s finding 10 asks our own format to
//!   store draw order as a field so the array can be sorted by height instead.
//!   It needs no field: the order the items are written in *is* the draw order,
//!   because the `(y, x)` sort is stable and `client/render`'s `statics::pick`
//!   breaks a tie by taking the last. A height index is a thing a chunk could
//!   gain later; a reordering is what would need the field, and that is a
//!   change to the picture rather than to the format.
//! - **A hash.** Direction E is what needs a blob verified against its name.
//!   The header has a version byte, and a hash is a length-prefixed trailer
//!   when there is something to check it against.

use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Facet;

use crate::chunk::{BLOCKS_PER_CHUNK, Chunk, ChunkCoord, ChunkKey};
use crate::grid::BlockExtent;
use crate::map::{BLOCK_SIZE, CELLS_PER_BLOCK, LandCell, LandTile, StaticItem};
use crate::snapshot::MapRevision;

/// What every chunk starts with, so a blob that is not one says so in four
/// bytes rather than in a plausible facet.
const MAGIC: [u8; 4] = *b"OSMC";

/// The encoding this module writes and the only one it reads.
///
/// A reader that meets a later version refuses rather than guessing: a chunk
/// misread is a world that parses perfectly and is wrong, which is the failure
/// `crate::grid`'s header is about.
const VERSION: u8 = 1;

/// Bytes before the land.
const HEADER_BYTES: usize = 24;
/// Bytes a cell takes: tile `u16`, height `i8`.
const CELL_BYTES: usize = 3;
/// Bytes a per-block static count takes.
const COUNT_BYTES: usize = 4;
/// Bytes a static takes: graphic `u16`, packed position, height `i8`, hue `u16`.
const STATIC_BYTES: usize = 6;

/// A blob is not a chunk, or is not the chunk it claims to be.
///
/// Everything a decoder can refuse. There is no variant for a *plausible* chunk
/// that is wrong — that is what the revision and the key in the header are for,
/// and checking them against what was asked for is the caller's.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// The blob does not start with the magic.
    NotAChunk,
    /// The blob is a chunk of an encoding this build does not read.
    Version {
        /// What it says it is.
        found: u8,
    },
    /// The blob ends before the chunk it describes does.
    Truncated {
        /// How long the header says it is.
        wanted: usize,
        /// How long it is.
        found: usize,
    },
    /// The blob is longer than the chunk it describes.
    ///
    /// Refused rather than ignored: a canonical encoding has exactly one byte
    /// string per chunk, and a tail nothing reads is a place to hide one that
    /// hashes differently and decodes the same.
    Trailing {
        /// How long the header says it is.
        wanted: usize,
        /// How long it is.
        found: usize,
    },
    /// The chunk claims a block extent no chunk can have.
    BadExtent {
        /// What it claims.
        wide: u8,
        /// What it claims.
        down: u8,
    },
    /// The per-block counts do not add up to the header's total.
    CountMismatch {
        /// What the header says.
        wanted: u32,
        /// What the counts come to.
        found: u64,
    },
    /// A static's packed position uses bits that are not a position.
    ///
    /// Three bits of each coordinate is a whole block, so the top two bits of
    /// that byte are zero in every chunk this module writes. A blob that sets
    /// them is not one of ours, and masking them off would turn a foreign
    /// record into a plausible static standing somewhere it never stood.
    BadPosition {
        /// Which static, counted through the whole chunk.
        at: usize,
        /// The byte.
        found: u8,
    },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAChunk => write!(f, "not a map chunk: it does not begin with OSMC"),
            Self::Version { found } => {
                write!(
                    f,
                    "a version {found} chunk, and this build reads version {VERSION}"
                )
            }
            Self::Truncated { wanted, found } => {
                write!(f, "a chunk of {wanted} bytes arrived in {found}")
            }
            Self::Trailing { wanted, found } => {
                write!(
                    f,
                    "a chunk of {wanted} bytes arrived in {found}, with a tail nothing reads"
                )
            }
            Self::BadExtent { wide, down } => write!(
                f,
                "a chunk of {wide}x{down} blocks, where a chunk is at most \
                 {BLOCKS_PER_CHUNK}x{BLOCKS_PER_CHUNK} and at least 1x1"
            ),
            Self::CountMismatch { wanted, found } => write!(
                f,
                "the header says {wanted} statics and the per-block counts come to {found}"
            ),
            Self::BadPosition { at, found } => write!(
                f,
                "static {at} is at {found:#04x}, which is not a position inside a block"
            ),
        }
    }
}

impl std::error::Error for DecodeError {}

/// How long the encoding of a chunk of this shape is.
const fn encoded_len(blocks: usize, statics: usize) -> usize {
    HEADER_BYTES + blocks * (CELLS_PER_BLOCK * CELL_BYTES + COUNT_BYTES) + statics * STATIC_BYTES
}

/// One chunk as its canonical bytes.
#[must_use]
pub fn encode(chunk: &Chunk) -> Vec<u8> {
    let blocks = chunk.extent().count() as usize;
    let mut out = Vec::with_capacity(encoded_len(blocks, chunk.static_count()));

    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(chunk.key().facet.0);
    // The widest facet a client ships is 112 chunks across, so a `u16` holds
    // every chunk coordinate a facet a `u16` tile can reach could have.
    out.extend_from_slice(&(chunk.key().at.x as u16).to_le_bytes());
    out.extend_from_slice(&(chunk.key().at.y as u16).to_le_bytes());
    out.extend_from_slice(&chunk.revision().get().to_le_bytes());
    out.push(chunk.extent().wide as u8);
    out.push(chunk.extent().down as u8);
    out.extend_from_slice(&(chunk.static_count() as u32).to_le_bytes());

    for cell in chunk.land() {
        out.extend_from_slice(&cell.tile.0.to_le_bytes());
        out.push(cell.z as u8);
    }
    for count in chunk.counts() {
        out.extend_from_slice(&count.to_le_bytes());
    }
    for item in chunk.statics() {
        out.extend_from_slice(&item.tile.0.to_le_bytes());
        // Only the position *within the block* is written: which block it is
        // in, the counts above already said. That is the four bytes an absolute
        // coordinate costs, and it is why `StaticItem` can go on carrying a
        // world coordinate everywhere else.
        out.push(pack_position(item.x, item.y));
        out.push(item.z as u8);
        out.extend_from_slice(&item.hue.0.to_le_bytes());
    }

    debug_assert_eq!(out.len(), encoded_len(blocks, chunk.static_count()));
    out
}

/// Bytes back into one chunk.
///
/// Every field is checked against the ones around it before a chunk is built:
/// the length against the shape the header describes, the counts against the
/// header's total, and each packed position against the block it is in. What is
/// *not* checked here is whether this is the chunk the caller wanted — the key
/// and the revision are in the header for the caller to compare, and a decoder
/// that had been told what to expect could not report a cache that answered the
/// wrong chunk.
///
/// # Errors
///
/// [`DecodeError`], one variant per way a blob fails to be a chunk.
pub fn decode(bytes: &[u8]) -> Result<Chunk, DecodeError> {
    if bytes.len() < HEADER_BYTES || bytes[..4] != MAGIC {
        return Err(DecodeError::NotAChunk);
    }
    if bytes[4] != VERSION {
        return Err(DecodeError::Version { found: bytes[4] });
    }

    let key = ChunkKey {
        facet: Facet(bytes[5]),
        at: ChunkCoord {
            x: u32::from(u16::from_le_bytes([bytes[6], bytes[7]])),
            y: u32::from(u16::from_le_bytes([bytes[8], bytes[9]])),
        },
    };
    let revision = MapRevision::decoded(u64::from_le_bytes(bytes[10..18].try_into().expect("eight bytes")));

    let (wide, down) = (bytes[18], bytes[19]);
    let whole = BLOCKS_PER_CHUNK as u8;
    if wide == 0 || down == 0 || wide > whole || down > whole {
        return Err(DecodeError::BadExtent { wide, down });
    }
    let extent = BlockExtent {
        wide: u32::from(wide),
        down: u32::from(down),
    };
    let blocks = extent.count() as usize;

    let statics = u32::from_le_bytes(bytes[20..24].try_into().expect("four bytes"));
    let wanted = encoded_len(blocks, statics as usize);
    if bytes.len() < wanted {
        return Err(DecodeError::Truncated {
            wanted,
            found: bytes.len(),
        });
    }
    if bytes.len() > wanted {
        return Err(DecodeError::Trailing {
            wanted,
            found: bytes.len(),
        });
    }

    let land_from = HEADER_BYTES;
    let counts_from = land_from + blocks * CELLS_PER_BLOCK * CELL_BYTES;
    let statics_from = counts_from + blocks * COUNT_BYTES;

    let land: Vec<LandCell> = bytes[land_from..counts_from]
        .chunks_exact(CELL_BYTES)
        .map(|cell| LandCell {
            tile: LandTile(u16::from_le_bytes([cell[0], cell[1]])),
            z: cell[2] as i8,
        })
        .collect();

    let counts: Vec<u32> = bytes[counts_from..statics_from]
        .chunks_exact(COUNT_BYTES)
        .map(|count| u32::from_le_bytes([count[0], count[1], count[2], count[3]]))
        .collect();
    // In `u64`, so a set of counts that overflows a `u32` is reported rather
    // than wrapping into agreement with a header that says something else.
    let total: u64 = counts.iter().map(|count| u64::from(*count)).sum();
    if total != u64::from(statics) {
        return Err(DecodeError::CountMismatch {
            wanted: statics,
            found: total,
        });
    }

    // Which block each static is in is where it sits in the counts, and its
    // world coordinate is that block's origin plus the packed position. This is
    // the one place the two halves meet, and it is the inverse of what `encode`
    // dropped.
    let origin = key.at.block_origin();
    let mut items = Vec::with_capacity(statics as usize);
    let mut records = bytes[statics_from..].chunks_exact(STATIC_BYTES);
    for (local, count) in extent.blocks().zip(&counts) {
        let block = extent.coord_of(local).expect("a block of this extent");
        let (block_x, block_y) = crate::grid::BlockCoord {
            x: origin.x + block.x,
            y: origin.y + block.y,
        }
        .origin();
        for _ in 0..*count {
            let entry = records.next().expect("the length was checked above");
            let (x, y) = unpack_position(entry[2]).ok_or(DecodeError::BadPosition {
                at: items.len(),
                found: entry[2],
            })?;
            items.push(StaticItem {
                tile: Graphic(u16::from_le_bytes([entry[0], entry[1]])),
                // Back to a world coordinate, which is what every reader
                // downstream of `Map` has always been handed.
                x: (block_x + u32::from(x)) as u16,
                y: (block_y + u32::from(y)) as u16,
                z: entry[3] as i8,
                hue: Hue(u16::from_le_bytes([entry[4], entry[5]])),
            });
        }
    }

    Ok(Chunk::from_parts(key, revision, extent, land, &counts, items))
}

/// A static's position inside its block, `y` in the high three bits.
///
/// `y` first for the same reason [`Map::statics_in_block`](crate::map::Map::statics_in_block)'s order is: a row
/// has to be contiguous, so the row is the more significant half of the key,
/// and packing it the other way round would make the byte's numeric order
/// disagree with the order the items are written in.
const fn pack_position(x: u16, y: u16) -> u8 {
    let x = (x as u32 % BLOCK_SIZE) as u8;
    let y = (y as u32 % BLOCK_SIZE) as u8;
    (y << 3) | x
}

/// The inverse, or `None` for a byte that is not a position.
const fn unpack_position(packed: u8) -> Option<(u8, u8)> {
    match packed >> 6 {
        0 => Some((packed & 0x7, (packed >> 3) & 0x7)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{assemble, chunks_of, fixture};
    use crate::snapshot::MapSnapshot;

    const FACET: Facet = Facet(0);

    fn extent() -> BlockExtent {
        BlockExtent {
            wide: fixture::BLOCKS,
            down: fixture::BLOCKS,
        }
    }

    fn snapshot() -> MapSnapshot {
        MapSnapshot::new(FACET, fixture::map())
    }

    fn cut(snapshot: &MapSnapshot) -> Vec<Chunk> {
        chunks_of(extent())
            .map(|at| Chunk::of(snapshot, at).expect("a chunk of this facet"))
            .collect()
    }

    /// The whole point, in one test: bytes in, the same bytes out, and the
    /// world in between is the world that went in.
    #[test]
    fn a_facet_round_trips_through_its_bytes() {
        let snapshot = snapshot();
        let blobs: Vec<Vec<u8>> = cut(&snapshot).iter().map(encode).collect();

        let decoded: Vec<Chunk> = blobs
            .iter()
            .map(|blob| decode(blob).expect("a chunk we just encoded"))
            .collect();
        let rebuilt = assemble(FACET, extent(), &decoded).expect("a complete set");

        // The world survived...
        let original = snapshot.map();
        assert_eq!(original.static_count(), rebuilt.static_count());
        for y in 0..u16::try_from(fixture::TILES).unwrap() {
            for x in 0..u16::try_from(fixture::TILES).unwrap() {
                assert_eq!(
                    original.land(x, y),
                    rebuilt.land(x, y),
                    "the ground at ({x}, {y})"
                );
                let was: Vec<_> = original.statics_at(x, y).collect();
                let is: Vec<_> = rebuilt.statics_at(x, y).collect();
                assert_eq!(was, is, "the statics at ({x}, {y})");
            }
        }

        // ...and so did the bytes. Cutting the rebuilt facet again has to give
        // the same blobs, which is what canonical means and what lets an import
        // be checked by comparing blobs rather than by walking two worlds.
        let published = MapSnapshot::new(FACET, rebuilt);
        let again: Vec<Vec<u8>> = cut(&published).iter().map(encode).collect();
        assert_eq!(blobs, again);
    }

    #[test]
    fn a_decoded_chunk_is_the_chunk_that_was_encoded() {
        let snapshot = snapshot();
        for chunk in cut(&snapshot) {
            assert_eq!(decode(&encode(&chunk)), Ok(chunk));
        }
    }

    /// A chunk's blob is exactly as long as its shape says, which is what makes
    /// a truncated or padded one detectable rather than merely unlucky.
    #[test]
    fn the_length_is_the_one_the_header_describes() {
        let snapshot = snapshot();
        let chunk = Chunk::of(&snapshot, ChunkCoord { x: 0, y: 0 }).expect("a chunk");
        let blocks = chunk.extent().count() as usize;
        assert_eq!(
            encode(&chunk).len(),
            HEADER_BYTES
                + blocks * CELLS_PER_BLOCK * CELL_BYTES
                + blocks * COUNT_BYTES
                + chunk.static_count() * STATIC_BYTES
        );
    }

    /// An edge chunk is shorter, and that is in its header rather than implied
    /// by a facet size the blob does not carry.
    #[test]
    fn an_edge_chunk_encodes_only_the_blocks_it_has() {
        let snapshot = snapshot();
        let corner = Chunk::of(&snapshot, ChunkCoord { x: 1, y: 1 }).expect("a chunk");
        let blob = encode(&corner);
        assert_eq!((blob[18], blob[19]), (1, 1));
        assert_eq!(decode(&blob).expect("a chunk").extent(), corner.extent());
    }

    #[test]
    fn a_blob_that_is_not_a_chunk_is_refused() {
        assert_eq!(decode(b""), Err(DecodeError::NotAChunk));
        assert_eq!(decode(&[0; HEADER_BYTES]), Err(DecodeError::NotAChunk));
        // The right length and the wrong magic: a plausible blob is still not
        // one of ours.
        let mut blob = vec![0; HEADER_BYTES];
        blob[..4].copy_from_slice(b"OSMD");
        assert_eq!(decode(&blob), Err(DecodeError::NotAChunk));
    }

    #[test]
    fn a_chunk_from_a_later_encoding_is_refused_rather_than_guessed_at() {
        let snapshot = snapshot();
        let mut blob = encode(&Chunk::of(&snapshot, ChunkCoord { x: 0, y: 0 }).expect("a chunk"));
        blob[4] = VERSION + 1;
        assert_eq!(decode(&blob), Err(DecodeError::Version { found: VERSION + 1 }));
    }

    #[test]
    fn a_truncated_chunk_is_refused() {
        let snapshot = snapshot();
        let blob = encode(&Chunk::of(&snapshot, ChunkCoord { x: 0, y: 0 }).expect("a chunk"));
        let wanted = blob.len();
        let short = &blob[..wanted - 1];
        assert_eq!(
            decode(short),
            Err(DecodeError::Truncated {
                wanted,
                found: wanted - 1
            })
        );
    }

    /// A tail nothing reads is a second byte string for one chunk, and a
    /// canonical encoding has exactly one.
    #[test]
    fn a_chunk_with_a_tail_is_refused() {
        let snapshot = snapshot();
        let mut blob = encode(&Chunk::of(&snapshot, ChunkCoord { x: 0, y: 0 }).expect("a chunk"));
        let wanted = blob.len();
        blob.push(0);
        assert_eq!(
            decode(&blob),
            Err(DecodeError::Trailing {
                wanted,
                found: wanted + 1
            })
        );
    }

    #[test]
    fn an_extent_no_chunk_can_have_is_refused() {
        let snapshot = snapshot();
        let blob = encode(&Chunk::of(&snapshot, ChunkCoord { x: 0, y: 0 }).expect("a chunk"));
        for (wide, down) in [(0, 8), (8, 0), (9, 8), (8, 9), (255, 255)] {
            let mut broken = blob.clone();
            broken[18] = wide;
            broken[19] = down;
            assert_eq!(decode(&broken), Err(DecodeError::BadExtent { wide, down }));
        }
    }

    #[test]
    fn counts_that_do_not_add_up_to_the_header_are_refused() {
        let snapshot = snapshot();
        let chunk = Chunk::of(&snapshot, ChunkCoord { x: 0, y: 0 }).expect("a chunk");
        let mut blob = encode(&chunk);
        // One more in the first block than the header says there are anywhere.
        let counts_from = HEADER_BYTES + chunk.extent().count() as usize * CELLS_PER_BLOCK * CELL_BYTES;
        blob[counts_from] += 1;
        assert_eq!(
            decode(&blob),
            Err(DecodeError::CountMismatch {
                wanted: chunk.static_count() as u32,
                found: chunk.static_count() as u64 + 1,
            })
        );
    }

    /// The top two bits of a packed position are zero in every chunk we write.
    /// Masking them off would turn a foreign record into a static standing
    /// somewhere it never stood, which is exactly the kind of plausible wrong
    /// answer this format exists to refuse.
    #[test]
    fn a_position_that_is_not_one_is_refused_rather_than_masked() {
        let snapshot = snapshot();
        let chunk = Chunk::of(&snapshot, ChunkCoord { x: 0, y: 0 }).expect("a chunk");
        let mut blob = encode(&chunk);
        let blocks = chunk.extent().count() as usize;
        let statics_from = HEADER_BYTES + blocks * CELLS_PER_BLOCK * CELL_BYTES + blocks * COUNT_BYTES;
        let at = statics_from + 2;
        blob[at] |= 0x40;
        assert_eq!(
            decode(&blob),
            Err(DecodeError::BadPosition {
                at: 0,
                found: blob[at]
            })
        );
    }

    /// Six bits carry a position inside a block, and every one of the
    /// sixty-four is a position that comes back unchanged.
    #[test]
    fn every_position_inside_a_block_survives_packing() {
        for y in 0..BLOCK_SIZE as u16 {
            for x in 0..BLOCK_SIZE as u16 {
                let packed = pack_position(x, y);
                assert_eq!(unpack_position(packed), Some((x as u8, y as u8)));
            }
        }
        // And a world coordinate packs as its position within its own block.
        assert_eq!(pack_position(4095, 4094), pack_position(7, 6));
    }
}
