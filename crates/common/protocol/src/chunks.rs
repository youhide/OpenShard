//! The world itself, over the game connection.
//!
//! Seven subcommands in the [`OPENSHARD_SUBCOMMANDS`] range, and together they
//! are how a client of ours comes to draw the ground *the shard* is standing on
//! rather than the ground on its own disk. `docs/map/new_map_representation/to_the_client.md`
//! is the plan; this is its wire.
//!
//! ```text
//! 0xE002  ChunkRequest   client -> server
//!         facet u8, count u16, then count x { chunk x u16, chunk y u16 }
//!
//! 0xE003  ChunkData      server -> client
//!         facet u8, chunk x u16, chunk y u16, revision u64,
//!         fragment u8, fragments u8, inflated u32, blob ..
//!
//! 0xE004  WorldNotice    server -> client, on world entry
//!         facet u8, blocks wide u32, blocks down u32, revision u64,
//!         world named u8, world id u64
//!
//! 0xE005  PublishNotice  server -> client, when the ground moves
//!         facet u8, revision u64, answer u8,
//!         then for "these" only: count u16, count x { chunk x u16, chunk y u16 }
//!
//! 0xE006  ChunkRefused   server -> client
//!         facet u8, chunk x u16, chunk y u16, reason u8
//!
//! 0xE007  ChangesRequest client -> server
//!         facet u8, revision u64
//!
//! 0xE008  ChangesReply   server -> client
//!         facet u8, revision u64, answer u8,
//!         then for "these" only: count u16, count x { chunk x u16, chunk y u16 }
//! ```
//!
//! Big-endian, like the rest of the wire and unlike the chunk record inside the
//! blob, which is a *file* format and little-endian. The two never meet: this
//! module never looks inside a blob.
//!
//! # Why it is safe to invent these at all
//!
//! [`OPENSHARD_SUBCOMMANDS`] carries the argument in full — every subcommand a
//! shipped client speaks is at or below `0x2B`, a stock client reads `0xBF`'s
//! length out of the envelope and skips a subcommand it does not know, and a
//! private packet *id* would instead desynchronise it for good. The envelope is
//! chosen for what happens when we are wrong.
//!
//! Beyond that, **only a client that asked is answered**. A stock client never
//! sends [`ChunkRequest`], so nothing here but the two notices ever reaches one,
//! and both are a couple of dozen bytes of body it will drop.
//!
//! The two are the exception together and for one reason: a client cannot ask
//! about a world nobody told it about, and it cannot ask what moved unless
//! somebody says something did. [`WorldNotice`] is that sentence at world entry
//! and [`PublishNotice`] is the same sentence afterwards, so there is no
//! connection state that decides who hears them — see `PublishNotice`'s own doc,
//! where the alternative is written down and refused.
//!
//! # A chunk is deflated, and the packet carries the inflated length
//!
//! [`crate::design`]'s shape exactly, for [`crate::design`]'s reason: a receiver
//! that inflates without a bound is a receiver a sender can make allocate
//! anything. The bound rides in every fragment and [`join`] refuses a set whose
//! fragments disagree about it.
//!
//! The measurements that decided it are in the plan and are worth repeating,
//! because they are what retired the argument for a second stream. Felucca's
//! 7,168 chunks are 12,568 bytes each before a single static, mean 15,001 and
//! max 45,382 — so **21.3% of them do not fit in an 18,000-byte packet as they
//! stand**. Deflated at level 6 the same set is median 1,739 bytes, max 16,050,
//! and *none* over the cap; an ocean chunk goes from 12,568 bytes to 56.
//!
//! # Why a fragment cap smaller than a packet
//!
//! Not because a chunk needs it — none of Felucca's does — but because
//! **4.58% of them do at [`FRAGMENT_BYTES`]**, which is 328 chunks a facet. A
//! reassembly path exercised by one chunk in twenty is a path that works; one
//! exercised only by a hypothetical dense generated world is a path that is
//! wrong the first time it runs. The cap also bounds how long a bulk transfer
//! can sit in front of a movement packet, since this rides the one stream
//! everything else does.

use crate::access::OPENSHARD_SUBCOMMANDS;
use crate::codec::{PacketReader, PacketWriter};
use crate::error::DecodeError;
use crate::packet::{DecodePacket, EncodePacket, PacketLength, frame_body};
use crate::version::ClientVersion;
use crate::world::{Facet, WorldId};

/// The largest slice of one chunk's deflated blob a single packet carries.
///
/// See the module header for why this is well below `MAX_PACKET_SIZE` rather
/// than at it.
pub const FRAGMENT_BYTES: usize = 8_192;

/// How many chunks one [`ChunkRequest`] may name.
///
/// A bound on *one answer*, not on how fast a facet may be fetched — pacing the
/// whole 7,168 is the client's business, and it sends as many requests as it
/// likes. Sixty-four is at most a megabyte of reply at Felucca's worst chunk
/// and 111 KiB at its median, and the request itself is 264 bytes.
///
/// The decoder refuses a larger count rather than truncating it: a count no
/// encoder here can write did not come from a client of ours, and answering
/// half of what was asked for would be this end inventing a request.
pub const MAX_CHUNKS: u16 = 64;

/// How hard a chunk's record is packed, which is a property of where it is going.
///
/// **Not a knob.** There are exactly two, one per destination, and both are
/// measured — `openshard-uofiles`'s `base_set_cost` example is what measured
/// them and what any argument about changing one has to re-run. A caller does
/// not choose a number; it says which of the two things it is writing, so a file
/// written by one build cannot be read as a different level by another.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DeflateLevel(u8);

impl DeflateLevel {
    /// One chunk into one packet, at six.
    ///
    /// What the module header's measurements were taken at, and what
    /// [`crate::design`] already uses for a house's planes. Here the *size* is
    /// what matters and the time does not: a chunk is half a millisecond, and it
    /// is sent one at a time to a client that asked.
    pub const PACKET: Self = Self(6);

    /// A whole facet into one file, at one.
    ///
    /// Where the time is what matters, because this is 7,168 chunks in one call
    /// on a thread somebody is waiting on — an import, a client keeping the
    /// ground it was given, a squash. Measured on Felucca, over 107,528,650
    /// bytes of records:
    ///
    /// | level | deflated | of the records | deflating |
    /// |---|---|---|---|
    /// | **1** | **29,698,618** | **27.6%** | **0.50 s** |
    /// | 2 | 25,013,830 | 23.3% | 0.86 s |
    /// | 3 | 23,040,738 | 21.4% | 1.38 s |
    /// | 6 | 22,469,130 | 20.9% | 3.73 s |
    ///
    /// Six buys 6.9 MB of disk for 3.2 seconds on a path a person waits on, and
    /// `overview.md` refuses that trade by name: thrift is not a goal. One still
    /// takes the file from 107.6 MB to 29.8, and a whole base set write from
    /// 4.2 s to 0.56.
    pub const BASE_SET: Self = Self(1);
}

/// Deflate one chunk's canonical record for where it is going.
///
/// **Here rather than at either caller**, because the wire is no longer the only
/// one: `openshard_basemap`'s version 2 stores a base set's chunks deflated
/// through this same function, so the two ends cannot drift into two
/// implementations of one idea. What they do *not* share is the level, and
/// [`DeflateLevel`] is where that is argued.
#[must_use]
pub fn deflate(record: &[u8], level: DeflateLevel) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(record, level.0)
}

/// Inflate one, bounded by the length it declared, and `None` if it is not that.
///
/// The bound is the whole reason an inflated length travels beside a blob — a
/// receiver that inflates without one is a receiver a sender can make allocate
/// anything. The **equality** is the second half of that claim and it is checked
/// here: a stream that stops short of its declared length inflates happily under
/// a limit, and a chunk record short of its own header decodes to a refusal
/// somewhere far away from the blob that caused it.
#[must_use]
pub fn inflate(deflated: &[u8], inflated: InflatedLength) -> Option<Vec<u8>> {
    let record =
        miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(deflated, inflated.0 as usize).ok()?;
    (record.len() == inflated.0 as usize).then_some(record)
}

/// A chunk's position on a facet, as the wire carries it.
///
/// **Not `openshard_map::chunk::ChunkCoord`**, which is the same place in the
/// crate that owns the world — and that crate is *above* this one, since it
/// already imports [`Facet`] from here. So the two are converted at the seam,
/// and the narrowing to `u16` is deliberate rather than incidental: the widest
/// facet a client ships is 112 chunks across, and the chunk record's own header
/// writes the pair as `u16` for the same reason.
///
/// Its components stay bare integers for [`Point`](crate::world::Point)'s
/// reason.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ChunkAt {
    /// Chunk column.
    pub x: u16,
    /// Chunk row.
    pub y: u16,
}

/// How big a facet is, in map blocks.
///
/// [`MapSize`](crate::world::MapSize) is the same question in tiles, and this is
/// not it: what a receiver does with this number is hand it to
/// `openshard_map::chunk::assemble`, which refuses a short set of chunks against
/// it, and that call counts blocks. Deriving one from the other at the wire
/// would put a division in the one place whose whole job is to say how big the
/// world is.
///
/// The base set's own header carries exactly this pair beside exactly this
/// revision, which is not a coincidence: a client's cache *is* a base set.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FacetBlocks {
    /// Blocks from west to east.
    pub wide: u32,
    /// Blocks from north to south.
    pub down: u32,
}

/// A published revision of a facet, as the wire carries it.
///
/// `openshard_map::snapshot::MapRevision` is the domain type, one crate up, and
/// it is built from this with `MapRevision::decoded`. Two types for one number
/// because the crate that owns revisions is above the crate that owns the wire —
/// the same split [`RawSerial`](crate::serial::RawSerial) makes for a serial.
///
/// **This is the *world's* revision, and a chunk's own `revision` field is not
/// the same question.** After a publish every chunk re-cut from the facet
/// carries the new number while only the touched ones changed content, so a
/// cache keyed on a chunk's field throws away 7,167 good chunks per one-tile
/// edit. What [`WorldNotice`] carries is the base set header's.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct WorldRevision(pub u64);

/// How long a chunk's record is once inflated.
///
/// The bound the receiver inflates with, and the only reason it is on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InflatedLength(pub u32);

/// Which piece of a blob a packet carries, and how many pieces there are.
///
/// One value rather than two fields, and with a checked constructor rather than
/// public ones, because the pair has a rule the wire cannot be trusted to
/// respect: a count is never zero and an index is always below it. Both come off
/// a socket, so "fragment 4 of 2" is a thing a sender can say and this is where
/// it stops being sayable.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Fragment {
    index: u8,
    count: u8,
}

impl Fragment {
    /// Piece `index` of `count`, or `None` for a pair that is not one.
    #[must_use]
    pub const fn new(index: u8, count: u8) -> Option<Self> {
        if count == 0 || index >= count {
            return None;
        }
        Some(Self { index, count })
    }

    /// Which piece this is, counting from zero.
    #[must_use]
    pub const fn index(self) -> u8 {
        self.index
    }

    /// How many pieces the whole blob was cut into.
    #[must_use]
    pub const fn count(self) -> u8 {
        self.count
    }

    /// Whether the blob is whole once this one has arrived — which is true of a
    /// blob that was never cut at all.
    #[must_use]
    pub const fn is_last(self) -> bool {
        self.index + 1 == self.count
    }
}

/// `0xBF` subcommand `0xE002` — "send me these chunks of this facet".
///
/// The whole of the capability negotiation, and deliberately not a feature flag:
/// a client that sends one is a client of ours, and no other client sends one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChunkRequest {
    /// Which facet's ground is wanted.
    pub facet: Facet,
    /// Which chunks of it, at most [`MAX_CHUNKS`] of them.
    pub chunks: Vec<ChunkAt>,
}

impl ChunkRequest {
    /// Which `0xBF` this is.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 2;

    /// Read the body, `reader` already past the id, length and subcommand.
    ///
    /// The chunks are not checked against any facet here — whether the shard
    /// *has* that ground is the tick's question, and it is answered with a
    /// [`ChunkRefused`] rather than by a decoder that has no world to ask.
    pub(crate) fn decode_body(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        let facet = Facet(reader.u8()?);
        let count = reader.u16()?;
        if count > MAX_CHUNKS {
            return Err(DecodeError::UnknownValue {
                field: "chunks in one request",
                value: u32::from(count),
            });
        }
        let mut chunks = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            chunks.push(ChunkAt {
                x: reader.u16()?,
                y: reader.u16()?,
            });
        }
        Ok(Self { facet, chunks })
    }

    /// Encode the whole packet. Our own client sends this; the shard only ever
    /// decodes it.
    ///
    /// # Panics
    ///
    /// If more than [`MAX_CHUNKS`] chunks are named. The cap is the protocol's
    /// and the far end refuses one that breaks it, so a caller that could write
    /// one would be building a packet whose only outcome is a closed connection.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let count = u16::try_from(self.chunks.len()).unwrap_or(u16::MAX);
        assert!(
            count <= MAX_CHUNKS,
            "a chunk request names {count} chunks, and {MAX_CHUNKS} is the cap"
        );
        frame_body(0xBF, PacketLength::Variable, |out: &mut PacketWriter| {
            out.u16(Self::SUBCOMMAND);
            out.u8(self.facet.0);
            out.u16(count);
            for at in &self.chunks {
                out.u16(at.x);
                out.u16(at.y);
            }
        })
    }
}

/// `0xBF` subcommand `0xE003` — one fragment of one chunk of the world.
///
/// The blob is `openshard_map::codec`'s canonical chunk record, deflated whole
/// and then cut into pieces of at most [`FRAGMENT_BYTES`]. Nothing here reads
/// it: [`fragments`](Self::fragments) is what cuts one up and [`join`] is what
/// puts it back, and the record's own decoder is a crate away.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChunkData {
    /// Which facet's ground this is.
    pub facet: Facet,
    /// Which chunk of it.
    pub at: ChunkAt,
    /// The revision the chunk was cut at.
    pub revision: WorldRevision,
    /// Which piece of the deflated blob this packet carries.
    pub fragment: Fragment,
    /// How long the record is once inflated — the bound, and the same in every
    /// fragment of one chunk.
    pub inflated: InflatedLength,
    /// This fragment's slice of the deflated blob.
    ///
    /// A buffer and not a number; see [`join`], which is the only thing that
    /// should ever concatenate two of these.
    pub blob: Vec<u8>,
}

impl ChunkData {
    /// Which `0xBF` this is.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 3;

    /// What one fragment costs beyond its slice of the blob.
    ///
    /// The id byte, the envelope's length field, the subcommand, and the
    /// fourteen bytes that say which chunk of which facet at which revision
    /// this is a piece of. Twenty-four bytes against a fragment of up to
    /// [`FRAGMENT_BYTES`], which is what makes fetching a facet a question about
    /// the chunks rather than about the framing.
    pub const OVERHEAD_BYTES: usize = 1 + 2 + 2 + 1 + 2 + 2 + 8 + 1 + 1 + 4;

    /// Cut one chunk's canonical bytes into the packets that carry it.
    ///
    /// Deflate first and fragment after, in that order and not the other way
    /// round: fragments of a chunk deflated in pieces would each carry their own
    /// zlib header and lose the dictionary at every seam, which is most of the
    /// 0.208 the module header quotes.
    ///
    /// # Panics
    ///
    /// If the record needs more than 255 fragments. That is 2,088,960 deflated
    /// bytes for one 64-tile square — roughly ten megabytes of record, or four
    /// hundred statics on every tile of the chunk — so it is a world nobody can
    /// build rather than a case to handle. It panics loudly here instead of
    /// being wrapped into a byte and sent as a different chunk.
    #[must_use]
    pub fn fragments(facet: Facet, at: ChunkAt, revision: WorldRevision, record: &[u8]) -> Vec<Self> {
        let inflated = InflatedLength(
            u32::try_from(record.len()).expect("a chunk record of fewer than four billion bytes"),
        );
        let deflated = deflate(record, DeflateLevel::PACKET);
        // A zlib stream is never empty — it has a header — so `max(1)` is a
        // statement that this function always produces at least one packet
        // rather than a guard against a case that can arise.
        let pieces = deflated.len().div_ceil(FRAGMENT_BYTES).max(1);
        let count = u8::try_from(pieces).expect("a chunk that fits in 255 fragments");

        deflated
            .chunks(FRAGMENT_BYTES)
            .enumerate()
            .map(|(index, slice)| Self {
                facet,
                at,
                revision,
                fragment: Fragment::new(u8::try_from(index).expect("fewer than 255 fragments"), count)
                    .expect("an index below the count it was derived from"),
                inflated,
                blob: slice.to_vec(),
            })
            .collect()
    }

    /// Read the body, `reader` already past the id and the length field.
    fn read(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for chunk data",
                value: u32::from(subcommand),
            });
        }
        let facet = Facet(reader.u8()?);
        let at = ChunkAt {
            x: reader.u16()?,
            y: reader.u16()?,
        };
        let revision = WorldRevision(reader.u64()?);
        let index = reader.u8()?;
        let count = reader.u8()?;
        let Some(fragment) = Fragment::new(index, count) else {
            return Err(DecodeError::UnknownValue {
                field: "chunk fragment index within its count",
                value: (u32::from(index) << 8) | u32::from(count),
            });
        };
        let inflated = InflatedLength(reader.u32()?);
        let blob = reader.bytes(reader.remaining())?.to_vec();
        Ok(Self {
            facet,
            at,
            revision,
            fragment,
            inflated,
            blob,
        })
    }
}

/// Variable, and the only one of these four that is: a fragment is as long as
/// its slice of the blob.
impl EncodePacket for ChunkData {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(Self::SUBCOMMAND);
        out.u8(self.facet.0);
        out.u16(self.at.x);
        out.u16(self.at.y);
        out.u64(self.revision.0);
        out.u8(self.fragment.index());
        out.u8(self.fragment.count());
        out.u32(self.inflated.0);
        out.bytes(&self.blob);
    }
}

impl DecodePacket for ChunkData {
    const ID: u8 = 0xBF;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Self::read(reader)
    }
}

/// A set of fragments does not make one chunk.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum JoinError {
    /// Nothing was handed over.
    Empty,
    /// Two fragments disagree about what they are fragments *of*.
    ///
    /// A chunk assembled out of two chunks' bytes is the half-patched world
    /// `overview.md` refuses whole chunks in order to make unreachable, one
    /// level down.
    NotOneChunk {
        /// Which field disagreed, for the log line.
        field: &'static str,
    },
    /// The indices are not exactly `0..count`, once each.
    Incomplete {
        /// How many fragments the set says there are.
        wanted: u8,
        /// How many distinct indices arrived.
        found: usize,
    },
    /// The joined blob is not a deflate stream of the length it claimed.
    NotDeflated,
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "no fragments at all"),
            Self::NotOneChunk { field } => {
                write!(f, "the fragments disagree about their {field}")
            }
            Self::Incomplete { wanted, found } => {
                write!(f, "{found} of {wanted} fragments arrived")
            }
            Self::NotDeflated => write!(f, "the joined blob did not inflate to the length it declared"),
        }
    }
}

impl std::error::Error for JoinError {}

/// Put one chunk's record back together out of the packets that carried it.
///
/// The inverse of [`ChunkData::fragments`], and here rather than on either end
/// of the wire so that the two are one pair of functions with one round-trip
/// test over them. The fragments may arrive in any order — the stream preserves
/// the sender's, but a caller that collected them into a map has no order to
/// preserve, and sorting sixty-four indices costs nothing next to the inflate.
///
/// What comes back is the chunk *record*, still bytes:
/// `openshard_map::codec::decode` is what turns it into a chunk, and comparing
/// the key it carries against the one that was asked for is the caller's — a
/// cache that answered the wrong chunk is exactly the failure a decoder that had
/// been told what to expect could not report.
///
/// # Errors
///
/// [`JoinError`], one variant per way a set of fragments fails to be one chunk.
pub fn join(fragments: &[ChunkData]) -> Result<Vec<u8>, JoinError> {
    let Some(first) = fragments.first() else {
        return Err(JoinError::Empty);
    };
    for other in &fragments[1..] {
        let field = if other.facet != first.facet {
            "facet"
        } else if other.at != first.at {
            "position"
        } else if other.revision != first.revision {
            "revision"
        } else if other.inflated != first.inflated {
            "inflated length"
        } else if other.fragment.count() != first.fragment.count() {
            "fragment count"
        } else {
            continue;
        };
        return Err(JoinError::NotOneChunk { field });
    }

    // In index order, and each index exactly once: a set holding fragment 1
    // twice and fragment 0 not at all has the right length and is not the chunk.
    let count = first.fragment.count();
    let mut ordered: Vec<Option<&[u8]>> = vec![None; usize::from(count)];
    let mut found = 0;
    for piece in fragments {
        let slot = &mut ordered[usize::from(piece.fragment.index())];
        if slot.is_none() {
            found += 1;
        }
        *slot = Some(&piece.blob);
    }
    if found != usize::from(count) {
        return Err(JoinError::Incomplete { wanted: count, found });
    }

    let mut deflated = Vec::with_capacity(fragments.iter().map(|piece| piece.blob.len()).sum());
    for slice in ordered {
        deflated.extend_from_slice(slice.expect("every slot was filled"));
    }
    // With the length as a limit and as an assertion, which is the whole reason
    // it is on the wire — see [`inflate`].
    inflate(&deflated, first.inflated).ok_or(JoinError::NotDeflated)
}

/// `0xBF` subcommand `0xE004` — "this is the world you are standing in".
///
/// What a client needs *before* it can ask for anything: the facet's extent is
/// what `openshard_map::chunk::assemble` refuses a short set against, and the
/// revision is what a cache is compared with.
///
/// Sent where [`AuthorityNotice`](crate::access::AuthorityNotice) is sent, for
/// the same reason — the world entry is when a connection learns what it is
/// standing in — and it is the one thing in this module a client receives
/// without having asked. A shard whose facet has no ground sends none at all,
/// because a notice saying "nought blocks by nought" would be a world nobody can
/// ask for chunks of, described as though they could.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WorldNotice {
    /// Which facet.
    pub facet: Facet,
    /// How big it is, in blocks.
    pub blocks: FacetBlocks,
    /// Which published revision of it the shard is holding.
    pub revision: WorldRevision,
    /// Which world this ground *is*, when the shard can say.
    ///
    /// `None` is a facet the shard reads out of a UO install: there is no base
    /// set to name it by, and a client must not keep a copy of ground the shard
    /// cannot identify — two installs' Feluccas are two worlds, and nothing here
    /// could tell them apart afterwards. It is the shard's own `WorldHome` being
    /// absent, reaching the client.
    ///
    /// With one, a client files its cache under it and the revision beside it is
    /// then a question about *this* world rather than about a number two shards
    /// both start at 1. See [`WorldId`].
    pub world: Option<WorldId>,
}

impl WorldNotice {
    /// Which `0xBF` this is.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 4;
    /// The whole framed packet: id, length, subcommand, facet, two extents, a
    /// revision, and the world's identity behind a byte saying whether there is
    /// one.
    ///
    /// A presence byte rather than nought standing for "no world": a hash is
    /// free to come out as any of its 2^64 values, and a reserved one would be a
    /// world that silently stopped being cacheable on the day it was imported.
    pub const LENGTH_BYTES: u8 = 31;

    /// Encode the whole packet.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        crate::packet::encode_packet(&self, ClientVersion::new(4, 0, 0, 0))
    }
}

/// Fixed despite living under `0xBF`, [`crate::access::AuthorityNotice`]'s
/// reason exactly: the body never varies, so the constant is written by hand
/// because `frame_body` only back-patches a length for [`PacketLength::Variable`].
impl EncodePacket for WorldNotice {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Fixed(Self::LENGTH_BYTES as u16);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(u16::from(Self::LENGTH_BYTES));
        out.u16(Self::SUBCOMMAND);
        out.u8(self.facet.0);
        out.u32(self.blocks.wide);
        out.u32(self.blocks.down);
        out.u64(self.revision.0);
        out.u8(u8::from(self.world.is_some()));
        out.u64(self.world.map_or(0, |world| world.0));
    }
}

impl DecodePacket for WorldNotice {
    const ID: u8 = 0xBF;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for a world notice",
                value: u32::from(subcommand),
            });
        }
        let facet = Facet(reader.u8()?);
        let blocks = FacetBlocks {
            wide: reader.u32()?,
            down: reader.u32()?,
        };
        let revision = WorldRevision(reader.u64()?);
        // The byte says whether the eight after it mean anything, and it is read
        // as a *flag*: anything but nought is a shard that named its world, and
        // the identity itself is opaque here — this end never computes one.
        let named = reader.u8()? != 0;
        let world = WorldId(reader.u64()?);
        Ok(Self {
            facet,
            blocks,
            revision,
            world: named.then_some(world),
        })
    }
}

/// Why a chunk that was asked for is not coming.
///
/// Both reachable, and they are different facts: one says the shard has no such
/// world, the other that the world it has does not reach that far.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// This shard has no ground for that facet — either it holds no such facet
    /// at all, or the one it holds has no map on it.
    NoWorld,
    /// The facet exists and stops short of that chunk.
    PastTheEdge,
}

impl Refusal {
    /// The byte this reason rides as.
    ///
    /// Written out rather than derived from the discriminant, for
    /// [`AccessLevel::wire`](crate::access::AccessLevel::wire)'s reason: the
    /// order of the variants is not a wire format.
    #[must_use]
    pub const fn wire(self) -> u8 {
        match self {
            Self::NoWorld => 0,
            Self::PastTheEdge => 1,
        }
    }

    /// The reason a byte names, or `None` for one this build has never heard of.
    #[must_use]
    pub const fn from_wire(byte: u8) -> Option<Self> {
        Some(match byte {
            0 => Self::NoWorld,
            1 => Self::PastTheEdge,
            _ => return None,
        })
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoWorld => "this shard has no ground for that facet",
            Self::PastTheEdge => "that chunk is past the edge of the facet",
        })
    }
}

/// `0xBF` subcommand `0xE006` — "that chunk is not coming".
///
/// # Why silence is not the answer
///
/// A client that asked for a chunk the shard cannot cut waits for it, and a
/// client that waits for one packet that will never arrive is a client that
/// never finishes fetching a facet. Nothing else in this conversation is
/// self-terminating: there is no total, no end marker and no timeout that would
/// not also fire on a slow link.
///
/// So every chunk named in a [`ChunkRequest`] is answered exactly once, with
/// either a set of [`ChunkData`] or one of these. It is a diagnostic in
/// practice — a client fetching `chunks_of` a facet it was told the size of
/// cannot produce one — which is precisely why it must be visible when it
/// happens rather than looking like a lost packet.
///
/// `0xE006` and not `0xE005`: that number was left free for the publish notice
/// this plan's last phase adds — [`PublishNotice`], which now has it — because an
/// id chosen by which was written first is an id that has to be renumbered later.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChunkRefused {
    /// Which facet was asked about.
    pub facet: Facet,
    /// Which chunk of it.
    pub at: ChunkAt,
    /// Why it is not coming.
    pub reason: Refusal,
}

impl ChunkRefused {
    /// Which `0xBF` this is.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 6;
    /// The whole framed packet: id, length, subcommand, facet, position, reason.
    pub const LENGTH_BYTES: u8 = 11;

    /// Encode the whole packet.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        crate::packet::encode_packet(&self, ClientVersion::new(4, 0, 0, 0))
    }
}

impl EncodePacket for ChunkRefused {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Fixed(Self::LENGTH_BYTES as u16);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(u16::from(Self::LENGTH_BYTES));
        out.u16(Self::SUBCOMMAND);
        out.u8(self.facet.0);
        out.u16(self.at.x);
        out.u16(self.at.y);
        out.u8(self.reason.wire());
    }
}

impl DecodePacket for ChunkRefused {
    const ID: u8 = 0xBF;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for a chunk refusal",
                value: u32::from(subcommand),
            });
        }
        let facet = Facet(reader.u8()?);
        let at = ChunkAt {
            x: reader.u16()?,
            y: reader.u16()?,
        };
        let byte = reader.u8()?;
        let Some(reason) = Refusal::from_wire(byte) else {
            return Err(DecodeError::UnknownValue {
                field: "chunk refusal reason",
                value: u32::from(byte),
            });
        };
        Ok(Self { facet, at, reason })
    }
}

/// How many chunks one [`ChangesReply`] may name.
///
/// The packet is what decides it: a reply is five bytes of framing and twelve of
/// header, and each chunk named is four more, so 4,096 of them is 16,401 bytes
/// against an 18,000-byte cap. Past that the answer is
/// [`Changes::Everything`] — which is not a compromise but the cheaper truth,
/// since a client that has to be told about 4,097 of Felucca's 7,168 chunks is a
/// client whose cache has stopped being a saving.
///
/// It is deliberately not fragmented the way a chunk record is. A chunk *has* to
/// arrive; a list of what moved has an alternative that is one packet long.
pub const MAX_MOVED: u16 = 4_096;

/// `0xBF` subcommand `0xE007` — "what has moved since this revision?".
///
/// The question a client with a cache asks, and the only one it asks before it
/// knows what to fetch: it holds a world of this facet at some revision, the
/// shard has just said which revision it is at, and the two differ.
///
/// The revision is the client's own, not the shard's. What comes back is
/// [`ChangesReply`], exactly once.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChangesRequest {
    /// Which facet's ground the question is about.
    pub facet: Facet,
    /// The revision the asker already holds.
    pub revision: WorldRevision,
}

impl ChangesRequest {
    /// Which `0xBF` this is.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 7;

    /// Read the body, `reader` already past the id, length and subcommand.
    ///
    /// Nothing is checked against a world here, for [`ChunkRequest`]'s reason:
    /// whether the shard can *answer* about that revision is the tick's
    /// question, and its answer is a [`ChangesReply`] rather than a decoder's
    /// error.
    pub(crate) fn decode_body(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            facet: Facet(reader.u8()?),
            revision: WorldRevision(reader.u64()?),
        })
    }

    /// Encode the whole packet. Our own client sends this; the shard only ever
    /// decodes it.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        frame_body(0xBF, PacketLength::Variable, |out: &mut PacketWriter| {
            out.u16(Self::SUBCOMMAND);
            out.u8(self.facet.0);
            out.u64(self.revision.0);
        })
    }
}

/// What moved between two revisions of one facet.
///
/// Two answers and not a list that is sometimes empty and sometimes everything:
/// "these chunks and no others" and "I cannot tell you, take the facet again"
/// are different facts, and a client acts on them differently. Collapsing the
/// second into a list of all 7,168 would be a shard pretending to know something
/// it does not.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Changes {
    /// Exactly these chunks changed, and no others.
    ///
    /// At most [`MAX_MOVED`] of them, in no particular order and each named once
    /// — the union over the patches committed since the revision asked about.
    /// An empty list is a legal answer and means a world that has not moved.
    These(Vec<ChunkAt>),
    /// The shard cannot say what moved, so nothing short of the whole facet is
    /// safe to take.
    ///
    /// A revision it has no history for — older than the base set, or one it
    /// never published — a log it could not read, or more chunks than
    /// [`MAX_MOVED`]. The client's answer to all four is the same one, which is
    /// why they are one variant and the reason is a log line on the shard.
    ///
    /// In a [`PublishNotice`] only the last of the four can be the reason: the
    /// shard is describing a patch it has just committed, so it knows exactly
    /// what moved and the only thing it can fail to do is name it all in one
    /// packet.
    Everything,
}

impl Changes {
    /// The byte this answer rides as.
    ///
    /// Written out rather than derived from the discriminant, for
    /// [`Refusal::wire`]'s reason: the order of the variants is not a wire
    /// format.
    #[must_use]
    pub const fn wire(&self) -> u8 {
        match self {
            Self::These(_) => 0,
            Self::Everything => 1,
        }
    }
}

/// `0xBF` subcommand `0xE008` — the answer to [`ChangesRequest`], exactly once.
///
/// The revision it carries is the shard's own — the one the chunks it names were
/// cut at, and the one the asker's world is at once it has applied them. A
/// client that fetched them and found a different revision on the wire has a
/// world that moved underneath it mid-answer, which is the same fact
/// `openshard_map::chunk::assemble` refuses a mixed set for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChangesReply {
    /// Which facet this is about.
    pub facet: Facet,
    /// The revision the shard is at now.
    pub revision: WorldRevision,
    /// What moved to get there from the revision that was asked about.
    pub changes: Changes,
}

impl ChangesReply {
    /// Which `0xBF` this is.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 8;

    /// Encode the whole packet.
    ///
    /// # Panics
    ///
    /// If more than [`MAX_MOVED`] chunks are named. The cap is the protocol's
    /// and [`Changes::Everything`] is what a shard says instead, so a caller
    /// that could write one would be building a packet past
    /// [`MAX_PACKET_SIZE`](crate::packet::MAX_PACKET_SIZE).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        crate::packet::encode_packet(self, ClientVersion::new(4, 0, 0, 0))
    }
}

/// Variable: the list is as long as what moved.
impl EncodePacket for ChangesReply {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(Self::SUBCOMMAND);
        write_moved(out, self.facet, self.revision, &self.changes, "a changes reply");
    }
}

impl DecodePacket for ChangesReply {
    const ID: u8 = 0xBF;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for a changes reply",
                value: u32::from(subcommand),
            });
        }
        let (facet, revision, changes) = read_moved(reader, "changes reply answer")?;
        Ok(Self {
            facet,
            revision,
            changes,
        })
    }
}

/// `0xBF` subcommand `0xE005` — "the ground under you has moved, and this is
/// what moved".
///
/// The other half of [`ChangesReply`]: that one is an *answer*, sent once to a
/// client that asked before it had anything to draw; this is an *announcement*,
/// sent to a client that is already drawing when the shard commits a patch. The
/// three fields are the same three facts, so they are one body and two
/// subcommands rather than two encoders that have to agree.
///
/// # Who hears it, and why it is nobody's decision
///
/// Every connection standing on the facet, whatever client it is running. The
/// obvious alternative is to send it only to a client that has asked for
/// chunks — the shard could remember that a connection sent a `0xE002` — and it
/// is wrong for the reason [`WorldNotice`] is sent unasked in the first place:
/// **a client cannot ask about something nobody told it happened.** A client of
/// ours whose kept world was already current asks for nothing at all on the way
/// in, which is `to_the_client.md`'s E3 working exactly as intended, and a
/// subscription built out of "who asked" would leave that client — the one whose
/// cache works best — the one that never hears about an edit.
///
/// So it goes to everyone, and what a client does with it is a client's business:
/// a stock client reads `0xBF`'s length out of the envelope and drops a
/// subcommand it does not know, and a client of ours that opened a facet on its
/// own disk ignores it, because the ground it is drawing is not this shard's to
/// move.
///
/// # It is sent after the log has the patch, never before
///
/// A commit whose log refuses puts the world back — see `openshard_world`'s
/// `mapedit::commit` — and a client told about a revision that was rolled back
/// holds a world that never existed and will ask for chunks of it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PublishNotice {
    /// Which facet moved.
    pub facet: Facet,
    /// The revision it moved to.
    pub revision: WorldRevision,
    /// What moved to get there.
    ///
    /// [`Changes::These`] is the ordinary case and it is the patch's own
    /// [`touched_chunks`](openshard_map::patch::Patch::touched_chunks) — usually
    /// one square. [`Changes::Everything`] is a patch that touched more chunks
    /// than [`MAX_MOVED`], which is the same fact the reply's own `Everything`
    /// carries for its third reason, and it asks the same thing of the client:
    /// take the facet again.
    ///
    /// An empty list is not sent at all: an empty patch moves the revision
    /// without moving a tile, and there is nothing for a client to do about it.
    pub changes: Changes,
}

impl PublishNotice {
    /// Which `0xBF` this is.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 5;

    /// Encode the whole packet.
    ///
    /// # Panics
    ///
    /// If more than [`MAX_MOVED`] chunks are named — [`ChangesReply::encode`]'s
    /// panic, for its reason.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        crate::packet::encode_packet(self, ClientVersion::new(4, 0, 0, 0))
    }
}

/// Variable, for [`ChangesReply`]'s reason: the list is as long as what moved.
impl EncodePacket for PublishNotice {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(Self::SUBCOMMAND);
        write_moved(out, self.facet, self.revision, &self.changes, "a publish notice");
    }
}

impl DecodePacket for PublishNotice {
    const ID: u8 = 0xBF;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for a publish notice",
                value: u32::from(subcommand),
            });
        }
        let (facet, revision, changes) = read_moved(reader, "publish notice answer")?;
        Ok(Self {
            facet,
            revision,
            changes,
        })
    }
}

/// A facet, a revision and what moved between it and the last one — the body
/// [`ChangesReply`] and [`PublishNotice`] share.
///
/// `what` names the packet in the panic, because the cap it enforces is the same
/// cap for both and a caller reading the message wants to know which one it
/// wrote.
fn write_moved(out: &mut PacketWriter, facet: Facet, revision: WorldRevision, changes: &Changes, what: &str) {
    out.u8(facet.0);
    out.u64(revision.0);
    out.u8(changes.wire());
    if let Changes::These(chunks) = changes {
        let count = u16::try_from(chunks.len()).unwrap_or(u16::MAX);
        assert!(
            count <= MAX_MOVED,
            "{what} names {count} chunks, and {MAX_MOVED} is the cap"
        );
        out.u16(count);
        for at in chunks {
            out.u16(at.x);
            out.u16(at.y);
        }
    }
}

/// [`write_moved`] read back. `answer_field` names the packet's own answer byte,
/// so an unknown one says which packet it was unreadable in.
fn read_moved(
    reader: &mut PacketReader<'_>,
    answer_field: &'static str,
) -> Result<(Facet, WorldRevision, Changes), DecodeError> {
    let facet = Facet(reader.u8()?);
    let revision = WorldRevision(reader.u64()?);
    let answer = reader.u8()?;
    let changes = match answer {
        0 => {
            let count = reader.u16()?;
            if count > MAX_MOVED {
                return Err(DecodeError::UnknownValue {
                    field: "chunks named in one packet",
                    value: u32::from(count),
                });
            }
            let mut chunks = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                chunks.push(ChunkAt {
                    x: reader.u16()?,
                    y: reader.u16()?,
                });
            }
            Changes::These(chunks)
        }
        1 => Changes::Everything,
        other => {
            return Err(DecodeError::UnknownValue {
                field: answer_field,
                value: u32::from(other),
            });
        }
    };
    Ok((facet, revision, changes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extended::ExtendedRequest;
    use crate::packet::MAX_PACKET_SIZE;
    use crate::server_packet::{ServerPacket, frame_server_packet};

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    /// A record that does not compress.
    ///
    /// Deliberately unlike a real chunk, which deflates to about a fifth of
    /// itself: a fixture made of pattern bytes comes back as **one** fragment
    /// however long it is, so the reassembly path — the whole reason
    /// [`FRAGMENT_BYTES`] is below the packet cap — would go untested while the
    /// suite stayed green. A cheap LCG makes the deflated length track the raw
    /// one, so a size that ought to fragment does.
    fn a_record(bytes: usize) -> Vec<u8> {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        (0..bytes)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 33) as u8
            })
            .collect()
    }

    /// Every subcommand here is out of reach of every client version's own — all
    /// at or below `0x2B` — and of ClassicUO's private `0xBEEF`, and none of them
    /// collides with another of ours.
    #[test]
    fn the_invented_subcommands_are_nobody_elses() {
        let ours = [
            crate::access::AuthorityNotice::SUBCOMMAND,
            ChunkRequest::SUBCOMMAND,
            ChunkData::SUBCOMMAND,
            WorldNotice::SUBCOMMAND,
            PublishNotice::SUBCOMMAND,
            ChunkRefused::SUBCOMMAND,
            ChangesRequest::SUBCOMMAND,
            ChangesReply::SUBCOMMAND,
        ];
        for one in ours {
            assert!(one > 0x2B, "{one:#06X} is in a real client's range");
            assert_ne!(one, 0xBEEF, "{one:#06X} is ClassicUO's own");
            assert!(one >= OPENSHARD_SUBCOMMANDS, "{one:#06X} is outside our range");
        }
        for (index, one) in ours.iter().enumerate() {
            assert!(
                !ours[index + 1..].contains(one),
                "{one:#06X} is used by two of our own packets"
            );
        }
        // The one this plan's last phase was to take, taken by it: the refusal
        // is still `0xE006` and the publish notice has the number that was held
        // for it, so nothing was renumbered on the way.
        assert_eq!(ChunkRefused::SUBCOMMAND, OPENSHARD_SUBCOMMANDS + 6);
        assert_eq!(PublishNotice::SUBCOMMAND, OPENSHARD_SUBCOMMANDS + 5);
    }

    #[test]
    fn a_request_reads_back_through_the_extended_envelope() {
        let sent = ChunkRequest {
            facet: Facet(3),
            chunks: vec![ChunkAt { x: 0, y: 0 }, ChunkAt { x: 111, y: 63 }],
        };
        let bytes = sent.encode();
        // id, length, subcommand, facet, count, and four bytes a chunk.
        assert_eq!(bytes.len(), 1 + 2 + 2 + 1 + 2 + 2 * 4);
        assert_eq!(&bytes[..5], &[0xBF, 0x00, 0x10, 0xE0, 0x02]);
        assert_eq!(
            ExtendedRequest::decode(&bytes).unwrap(),
            ExtendedRequest::Chunks(sent)
        );
    }

    /// A facet is a named byte, not an enum of the maps the engine happens to
    /// ship today. Encoding and decoding therefore preserve every byte rather
    /// than validating, masking, or narrowing it at the wire seam.
    #[test]
    fn every_facet_byte_round_trips_through_a_request() {
        for raw in u8::MIN..=u8::MAX {
            let sent = ChunkRequest {
                facet: Facet(raw),
                chunks: vec![ChunkAt { x: 17, y: 29 }],
            };
            assert_eq!(
                ExtendedRequest::decode(&sent.encode()).unwrap(),
                ExtendedRequest::Chunks(sent),
                "facet {raw}"
            );
        }
    }

    /// A request naming nothing is legal and means nothing is wanted — a client
    /// with an up-to-date cache asks for no chunks at all, which is E3's own
    /// "done".
    #[test]
    fn a_request_for_no_chunks_is_a_request() {
        let sent = ChunkRequest {
            facet: Facet(0),
            chunks: Vec::new(),
        };
        assert_eq!(
            ExtendedRequest::decode(&sent.encode()).unwrap(),
            ExtendedRequest::Chunks(sent)
        );
    }

    /// Every prefix of a request is refused, and none of them panics: the count
    /// is a length the body has to be checked against, and a decoder that
    /// trusted it would allocate what a client asked it to.
    #[test]
    fn a_request_cut_short_anywhere_is_refused() {
        let full = ChunkRequest {
            facet: Facet(0),
            chunks: vec![ChunkAt { x: 1, y: 2 }, ChunkAt { x: 3, y: 4 }],
        }
        .encode();
        for cut in 0..full.len() {
            assert!(
                ExtendedRequest::decode(&full[..cut]).is_err(),
                "a {cut}-byte request must not decode"
            );
        }
    }

    /// A count over the cap is refused rather than truncated. Our own encoder
    /// cannot write one, so the packet did not come from a client of ours.
    #[test]
    fn a_request_over_the_cap_is_refused() {
        let mut bytes = ChunkRequest {
            facet: Facet(0),
            chunks: vec![ChunkAt { x: 1, y: 2 }],
        }
        .encode();
        // The count sits after the id, the length, the subcommand and the facet.
        bytes[6..8].copy_from_slice(&(MAX_CHUNKS + 1).to_be_bytes());
        assert_eq!(
            ExtendedRequest::decode(&bytes),
            Err(DecodeError::UnknownValue {
                field: "chunks in one request",
                value: u32::from(MAX_CHUNKS) + 1,
            })
        );
    }

    /// The cap and the packet cap agree: the largest request this end can write
    /// is one the framer on the other end will accept.
    #[test]
    fn the_largest_request_fits_in_a_packet() {
        let bytes = ChunkRequest {
            facet: Facet(0),
            chunks: (0..MAX_CHUNKS).map(|n| ChunkAt { x: n, y: n }).collect(),
        }
        .encode();
        assert!(bytes.len() < MAX_PACKET_SIZE, "{} bytes", bytes.len());
        assert!(ExtendedRequest::decode(&bytes).is_ok());
    }

    /// The pair that matters: a record cut into packets and joined back is the
    /// record, and it went over the wire in between.
    #[test]
    fn a_chunk_survives_being_cut_up_and_put_back() {
        for size in [1, 64, 12_568, 45_382, FRAGMENT_BYTES * 3] {
            let record = a_record(size);
            let packets = ChunkData::fragments(
                Facet(2),
                ChunkAt { x: 7, y: 9 },
                WorldRevision(0x0102_0304_0506_0708),
                &record,
            );
            assert!(!packets.is_empty());

            // Through the wire, one packet at a time, exactly as a client sees
            // them: framed, decoded from the id byte, and no other route in.
            let arrived: Vec<ChunkData> = packets
                .iter()
                .map(|packet| {
                    let bytes = ServerPacket::ChunkData(packet.clone()).encode(version());
                    assert_eq!(
                        frame_server_packet(&bytes, version()),
                        Ok(crate::packet::Frame::Complete(bytes.len())),
                    );
                    match ServerPacket::decode(&bytes, version()) {
                        Ok(Some(ServerPacket::ChunkData(data))) => data,
                        other => panic!("a chunk fragment decoded as {other:?}"),
                    }
                })
                .collect();
            assert_eq!(arrived, packets);
            assert_eq!(join(&arrived), Ok(record), "{size} bytes");
        }
    }

    /// No fragment is over the cap, every fragment but the last is *at* it, and
    /// each packet still fits the wire with room over.
    #[test]
    fn the_fragments_are_the_size_the_cap_says() {
        let record = a_record(FRAGMENT_BYTES * 40);
        let packets = ChunkData::fragments(Facet(0), ChunkAt { x: 0, y: 0 }, WorldRevision(1), &record);
        let count = packets.len();
        assert!(count > 1, "this record has to fragment to be worth testing");
        for (index, packet) in packets.iter().enumerate() {
            assert!(packet.blob.len() <= FRAGMENT_BYTES);
            if index + 1 < count {
                assert_eq!(packet.blob.len(), FRAGMENT_BYTES, "fragment {index} is short");
            }
            assert_eq!(usize::from(packet.fragment.index()), index);
            assert_eq!(usize::from(packet.fragment.count()), count);
            assert_eq!(packet.fragment.is_last(), index + 1 == count);
            assert_eq!(packet.inflated, InflatedLength(record.len() as u32));

            let bytes = ServerPacket::ChunkData(packet.clone()).encode(version());
            assert_eq!(
                bytes.len(),
                ChunkData::OVERHEAD_BYTES + packet.blob.len(),
                "the overhead a caller sizes a fetch with has to be the real one"
            );
            assert!(bytes.len() < MAX_PACKET_SIZE);
        }
    }

    /// Order is not what makes a set of fragments one chunk — the indices are.
    #[test]
    fn the_fragments_may_arrive_in_any_order() {
        let record = a_record(FRAGMENT_BYTES * 5);
        let mut packets = ChunkData::fragments(Facet(0), ChunkAt { x: 4, y: 4 }, WorldRevision(9), &record);
        assert!(packets.len() > 2);
        packets.reverse();
        assert_eq!(join(&packets), Ok(record));
    }

    /// Half a chunk is not a chunk, and neither is half of one and half of
    /// another. Each of these would otherwise inflate to *something*.
    #[test]
    fn a_set_that_is_not_one_whole_chunk_is_refused() {
        let record = a_record(FRAGMENT_BYTES * 3);
        let mine = ChunkData::fragments(Facet(0), ChunkAt { x: 1, y: 1 }, WorldRevision(4), &record);
        let theirs = ChunkData::fragments(Facet(0), ChunkAt { x: 2, y: 2 }, WorldRevision(4), &record);

        assert_eq!(join(&[]), Err(JoinError::Empty));
        assert_eq!(
            join(&[mine[0].clone(), theirs[1].clone()]),
            Err(JoinError::NotOneChunk { field: "position" })
        );
        assert_eq!(
            join(&mine[..mine.len() - 1]),
            Err(JoinError::Incomplete {
                wanted: mine.len() as u8,
                found: mine.len() - 1,
            })
        );
        // The right number of fragments and the wrong set of indices: one
        // arrived twice and one never did.
        let mut doubled = mine.clone();
        doubled[1] = mine[0].clone();
        assert_eq!(
            join(&doubled),
            Err(JoinError::Incomplete {
                wanted: mine.len() as u8,
                found: mine.len() - 1,
            })
        );
    }

    /// A blob that is not a deflate stream, and one that inflates past the
    /// length it declared. The second is the one that matters: it is the
    /// allocation a sender would otherwise choose.
    #[test]
    fn a_blob_that_is_not_what_it_says_is_refused() {
        let record = a_record(4_000);
        let mut packets = ChunkData::fragments(Facet(0), ChunkAt { x: 0, y: 0 }, WorldRevision(1), &record);
        assert_eq!(packets.len(), 1, "four kilobytes deflate to one fragment");

        let mut rubbish = packets[0].clone();
        rubbish.blob = vec![0xFF; 32];
        assert_eq!(join(&[rubbish]), Err(JoinError::NotDeflated));

        packets[0].inflated = InflatedLength(record.len() as u32 - 1);
        assert_eq!(join(&packets), Err(JoinError::NotDeflated));
    }

    /// "Fragment four of two" is a thing a sender can write and not a thing a
    /// reader can hold.
    #[test]
    fn a_fragment_outside_its_own_count_is_refused() {
        assert_eq!(Fragment::new(0, 0), None, "a blob is never nought pieces");
        assert_eq!(Fragment::new(2, 2), None);
        assert!(Fragment::new(1, 2).is_some());

        let packet = ChunkData {
            facet: Facet(0),
            at: ChunkAt { x: 0, y: 0 },
            revision: WorldRevision(1),
            fragment: Fragment::new(0, 1).expect("one of one"),
            inflated: InflatedLength(4),
            blob: vec![1, 2, 3],
        };
        let mut bytes = ServerPacket::ChunkData(packet).encode(version());
        // The two fragment bytes, after id, length, subcommand, facet, x, y and
        // the revision.
        let index = 1 + 2 + 2 + 1 + 2 + 2 + 8;
        bytes[index] = 4;
        bytes[index + 1] = 2;
        assert!(ServerPacket::decode(&bytes, version()).is_err());
    }

    /// Both notices: one that names its world and one that cannot.
    ///
    /// The pair together, because the presence byte is the whole difference and
    /// a test of either alone would pass with the byte written as a constant.
    #[test]
    fn a_world_notice_survives_the_wire() {
        for world in [Some(WorldId(0xDEAD_BEEF_FEED_FACE)), None] {
            let sent = WorldNotice {
                facet: Facet(0),
                blocks: FacetBlocks { wide: 896, down: 512 },
                revision: WorldRevision(7),
                world,
            };
            let bytes = sent.encode();
            assert_eq!(bytes.len(), usize::from(WorldNotice::LENGTH_BYTES));
            assert_eq!(bytes[0], 0xBF);
            assert_eq!(u16::from_be_bytes([bytes[3], bytes[4]]), WorldNotice::SUBCOMMAND);
            assert_eq!(
                ServerPacket::decode(&bytes, version()).expect("our own bytes decode"),
                Some(ServerPacket::WorldNotice(sent))
            );
        }
    }

    /// The question a client with a cache asks, and both answers to it.
    #[test]
    fn a_changes_conversation_survives_the_wire() {
        let asked = ChangesRequest {
            facet: Facet(2),
            revision: WorldRevision(41),
        };
        let bytes = asked.encode();
        assert_eq!(
            ExtendedRequest::decode(&bytes).expect("our own bytes decode"),
            ExtendedRequest::Changes(asked)
        );

        for changes in [
            Changes::These(vec![ChunkAt { x: 21, y: 25 }, ChunkAt { x: 0, y: 0 }]),
            // A world that has not moved, which is a list and not `Everything`:
            // "nothing changed" is knowledge, and it is the answer a cache hit
            // one revision behind an empty patch gets.
            Changes::These(Vec::new()),
            Changes::Everything,
        ] {
            let sent = ChangesReply {
                facet: Facet(2),
                revision: WorldRevision(42),
                changes,
            };
            let bytes = sent.encode();
            assert_eq!(
                ServerPacket::decode(&bytes, version()).expect("our own bytes decode"),
                Some(ServerPacket::ChangesReply(sent))
            );
        }
    }

    /// The announcement, which is the same three facts as the answer above and
    /// has to read back as a different packet.
    ///
    /// Both arms, and the empty list with them: a shard does not *send* an empty
    /// publish notice — an empty patch moves nothing a client could fetch — but a
    /// decoder that could not read one would be a decoder with an unreachable
    /// error in it, and the shared body is what makes that worth checking here.
    #[test]
    fn a_publish_notice_survives_the_wire() {
        for changes in [
            Changes::These(vec![ChunkAt { x: 2, y: 2 }]),
            Changes::These(Vec::new()),
            Changes::Everything,
        ] {
            let sent = PublishNotice {
                facet: Facet(0),
                revision: WorldRevision(2),
                changes,
            };
            let bytes = sent.encode();
            assert_eq!(bytes[0], 0xBF);
            assert_eq!(
                u16::from_be_bytes([bytes[3], bytes[4]]),
                PublishNotice::SUBCOMMAND
            );
            assert_eq!(
                ServerPacket::decode(&bytes, version()).expect("our own bytes decode"),
                Some(ServerPacket::PublishNotice(sent))
            );
        }
    }

    /// The two packets that share a body are still two packets: the same three
    /// facts under the other subcommand decode as the other variant.
    ///
    /// What this is against is the one mistake a shared encoder makes possible —
    /// writing one subcommand and reading the other — which no round trip of
    /// either packet alone can see.
    #[test]
    fn the_shared_body_is_still_two_packets() {
        let facet = Facet(1);
        let revision = WorldRevision(7);
        let changes = Changes::These(vec![ChunkAt { x: 3, y: 4 }]);
        let reply = ChangesReply {
            facet,
            revision,
            changes: changes.clone(),
        }
        .encode();
        let notice = PublishNotice {
            facet,
            revision,
            changes,
        }
        .encode();
        assert_eq!(
            reply[5..],
            notice[5..],
            "the bodies are one encoder and have to agree byte for byte"
        );
        assert_ne!(reply[3..5], notice[3..5], "and the subcommands are two");
        assert!(matches!(
            ServerPacket::decode(&reply, version()).expect("our own bytes decode"),
            Some(ServerPacket::ChangesReply(_))
        ));
        assert!(matches!(
            ServerPacket::decode(&notice, version()).expect("our own bytes decode"),
            Some(ServerPacket::PublishNotice(_))
        ));
    }

    /// A reply naming the most chunks it may fits in a packet, and one naming a
    /// chunk more is refused rather than truncated.
    ///
    /// The cap is what makes [`Changes::Everything`] necessary at all, so the
    /// arithmetic behind it is worth pinning rather than trusting.
    #[test]
    fn the_moved_cap_is_what_a_packet_holds() {
        let full = ChangesReply {
            facet: Facet(0),
            revision: WorldRevision(9),
            changes: Changes::These(
                (0..MAX_MOVED)
                    .map(|n| ChunkAt {
                        x: n % 112,
                        y: n / 112,
                    })
                    .collect(),
            ),
        };
        let bytes = full.encode();
        assert!(
            bytes.len() <= MAX_PACKET_SIZE,
            "a full changes reply is {} bytes",
            bytes.len()
        );
        assert_eq!(
            ServerPacket::decode(&bytes, version()).expect("our own bytes decode"),
            Some(ServerPacket::ChangesReply(full))
        );

        // One past the cap, written by hand: no encoder of ours can produce it,
        // which is exactly why the decoder is what has to refuse it.
        let mut bytes = ChangesReply {
            facet: Facet(0),
            revision: WorldRevision(9),
            changes: Changes::These(vec![ChunkAt { x: 1, y: 1 }]),
        }
        .encode();
        // The count, after id, length, subcommand, facet, revision and answer.
        let index = 1 + 2 + 2 + 1 + 8 + 1;
        bytes[index..index + 2].copy_from_slice(&(MAX_MOVED + 1).to_be_bytes());
        assert!(ServerPacket::decode(&bytes, version()).is_err());
    }

    #[test]
    fn a_refusal_survives_the_wire_and_an_unknown_reason_does_not() {
        for reason in [Refusal::NoWorld, Refusal::PastTheEdge] {
            let sent = ChunkRefused {
                facet: Facet(1),
                at: ChunkAt { x: 900, y: 3 },
                reason,
            };
            let bytes = sent.encode();
            assert_eq!(bytes.len(), usize::from(ChunkRefused::LENGTH_BYTES));
            assert_eq!(
                ServerPacket::decode(&bytes, version()).expect("our own bytes decode"),
                Some(ServerPacket::ChunkRefused(sent))
            );
        }

        // A reason this build has never heard of is an error and not a quiet
        // "no world": a client that read an unknown refusal as the nearest one
        // it knows would report the wrong thing to whoever is watching.
        assert_eq!(Refusal::from_wire(2), None);
        let mut bytes = ChunkRefused {
            facet: Facet(1),
            at: ChunkAt { x: 0, y: 0 },
            reason: Refusal::NoWorld,
        }
        .encode();
        *bytes.last_mut().expect("a body") = 2;
        assert!(ServerPacket::decode(&bytes, version()).is_err());
    }
}
