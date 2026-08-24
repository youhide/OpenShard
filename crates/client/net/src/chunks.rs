//! Fetching a facet's ground over the game connection.
//!
//! The client's half of `openshard_protocol::chunks`, and the bookkeeping E1
//! deliberately left out: that phase built the wire and one chunk's round trip,
//! and said that "the bookkeeping over many chunks in flight" belonged to
//! whoever knew what it wanted a progress line to say. This is it —
//! `docs/map/new_map_representation/to_the_client.md`'s E2.
//!
//! # It is a state machine and not a loop
//!
//! [`Fetch`] holds the list of chunks a facet has, which of them have been asked
//! for, the fragments of the ones still arriving, and the whole ones already
//! decoded. It touches no socket: [`Fetch::next_request`] hands out a packet to
//! send and [`Fetch::on_packet`] takes one that arrived, so the whole of a
//! facet's transfer is testable against a fixture rather than only against a
//! shard. `crates/client/app/src/link.rs` is what drives it, on the thread that
//! owns the connection.
//!
//! # Every chunk is asked for exactly once
//!
//! The shard's rule is that every chunk named in a request is answered exactly
//! once — with its bytes or with a refusal — and this end's is the mirror of it:
//! a chunk moves from `wanted` to `outstanding` when it is asked for and out of
//! `outstanding` when it is whole, and nothing puts it back. So a
//! [`ChunkData`] for a chunk that is not outstanding is not a duplicate to
//! ignore, it is a shard answering something nobody asked, and it is refused by
//! name — [`FetchError::Unasked`].
//!
//! # Why a facet arrives whole
//!
//! `to_the_client.md` takes that decision and it is not reopened here: a client
//! with no map files fetches all 7,168 chunks of Felucca — 21.3 MiB — before it
//! draws. Fetching on approach is direction G's, and [`WorldMap`] is a dense
//! array that cannot answer half a facet today.
//!
//! [`WorldMap`]: openshard_map::map::WorldMap

use openshard_map::chunk::{Chunk, ChunkCoord, ChunkKey, assemble, chunks_of};
use openshard_map::codec;
use openshard_map::grid::BlockExtent;
use openshard_map::snapshot::MapSnapshot;
use openshard_protocol::chunks::{
    ChunkAt, ChunkData, ChunkRequest, FacetBlocks, JoinError, MAX_CHUNKS, Refusal, WorldNotice, join,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::world::Facet;
use rustc_hash::FxHashMap;

/// How many chunks may be outstanding at once.
///
/// **A pacing choice and not a protocol one.** `MAX_CHUNKS` bounds one *answer*;
/// this bounds how much of the facet is in flight, and the two are different
/// questions. One request at a time would be correct and slow: 7,168 chunks in
/// batches of 64 is 112 round trips, which is five and a half seconds of pure
/// waiting on a 50 ms link however fast the bytes move. Four requests deep keeps
/// the pipe full while a reply of about 111 KiB at Felucca's median is on the
/// wire, and costs under a megabyte of half-assembled fragments here.
pub const IN_FLIGHT_CHUNKS: usize = 4 * MAX_CHUNKS as usize;

/// The ground the shard sent is not a facet.
///
/// Every variant is terminal for the fetch: there is nothing this end can do
/// about a refused chunk or a record that will not decode except say which one
/// it was. A client with no ground draws nothing, so the reason has to reach
/// whoever is watching.
#[derive(Debug)]
#[non_exhaustive]
pub enum FetchError {
    /// The facet is bigger than the wire can name.
    ///
    /// A chunk's position rides as a `u16` in both directions, so a facet more
    /// than 65,536 chunks along a side has chunks nobody can ask for. No shard
    /// can honestly describe one — `MapSize` caps a facet at 8,192 blocks, which
    /// is 1,024 chunks — so this is refused before the list of chunks is built
    /// rather than after a hundred million of them have been allocated.
    TooWide {
        /// What the notice claimed.
        blocks: FacetBlocks,
    },
    /// The shard refused a chunk of the facet it had just described.
    ///
    /// A diagnostic in practice and a contradiction in principle: the chunks
    /// asked for are `chunks_of` the extent the shard itself named, so it cannot
    /// produce one that is past its own edge or belongs to a facet it does not
    /// hold. See [`ChunkRefused`](openshard_protocol::chunks::ChunkRefused),
    /// whose whole reason for existing is that this is visible when it happens
    /// instead of looking like a lost packet.
    Refused {
        /// Which chunk.
        at: ChunkAt,
        /// Which of the two facts the shard stated.
        reason: Refusal,
    },
    /// A chunk arrived that was never asked for.
    Unasked {
        /// Which facet it claimed.
        facet: Facet,
        /// Which chunk of it.
        at: ChunkAt,
    },
    /// The fragments of one chunk do not make one blob.
    Join {
        /// Which chunk.
        at: ChunkAt,
        /// Why.
        source: JoinError,
    },
    /// The blob a chunk's fragments joined into is not a chunk record.
    Record {
        /// Which chunk.
        at: ChunkAt,
        /// Why.
        source: codec::DecodeError,
    },
    /// The record that arrived says it is a different chunk than the one asked
    /// for.
    ///
    /// The check [`join`]'s own doc leaves to the caller, and it has to be made
    /// here: a chunk is self-contained and names itself, so nothing downstream
    /// can tell a swapped pair apart from a missing one. `assemble` would refuse
    /// the facet for the blocks the duplicate did not cover and name the wrong
    /// chunk while doing it.
    WrongChunk {
        /// What was asked for.
        asked: ChunkAt,
        /// What the record says it is.
        found: ChunkKey,
    },
    /// The chunks are all here and do not make one facet.
    Assembly {
        /// Why.
        source: openshard_map::chunk::AssemblyError,
    },
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooWide { blocks } => write!(
                f,
                "a facet of {}x{} blocks has chunks the wire cannot name",
                blocks.wide, blocks.down
            ),
            Self::Refused { at, reason } => {
                write!(f, "chunk ({}, {}) was refused: {reason}", at.x, at.y)
            }
            Self::Unasked { facet, at } => write!(
                f,
                "chunk ({}, {}) of facet {} arrived and nobody asked for it",
                at.x, at.y, facet.0
            ),
            Self::Join { at, source } => {
                write!(f, "chunk ({}, {}) did not arrive whole: {source}", at.x, at.y)
            }
            Self::Record { at, source } => {
                write!(f, "chunk ({}, {}) is not a chunk record: {source}", at.x, at.y)
            }
            Self::WrongChunk { asked, found } => write!(
                f,
                "chunk ({}, {}) was asked for and chunk ({}, {}) of facet {} arrived",
                asked.x, asked.y, found.at.x, found.at.y, found.facet.0
            ),
            Self::Assembly { source } => write!(f, "the chunks do not make one facet: {source}"),
        }
    }
}

impl std::error::Error for FetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Join { source, .. } => Some(source),
            Self::Record { source, .. } => Some(source),
            Self::Assembly { source } => Some(source),
            Self::TooWide { .. } | Self::Refused { .. } | Self::Unasked { .. } | Self::WrongChunk { .. } => {
                None
            }
        }
    }
}

/// One facet's ground, arriving.
///
/// Built from the [`WorldNotice`] the shard sends on world entry, which is the
/// one thing a client is told without asking and the one thing it needs before
/// it can ask: the extent is what `assemble` refuses a short set of chunks
/// against, and the chunks to ask for are `chunks_of` it.
#[derive(Debug)]
pub struct Fetch {
    /// Which facet is being fetched. Every reply is checked against it.
    facet: Facet,
    /// How big the shard said it is. `assemble`'s second argument, and the only
    /// thing that makes a set of chunks a *whole* facet rather than a narrower
    /// world that parses perfectly.
    extent: BlockExtent,
    /// Every chunk the facet has, in `chunks_of` order.
    ///
    /// The order is the base set's own, which is not a coincidence: E3's cache
    /// is a base set, and a facet fetched in the order it is written is a facet
    /// that can be written as it arrives.
    wanted: Vec<ChunkAt>,
    /// How many of [`wanted`](Self::wanted) have been asked for. A cursor rather
    /// than a set, because nothing is ever asked for twice.
    asked: usize,
    /// Asked for and not yet whole: the fragments each has so far.
    ///
    /// A chunk leaves this map when it is whole, which is what makes "is this
    /// chunk outstanding" the same question as "did anybody ask for it".
    outstanding: FxHashMap<ChunkAt, Vec<ChunkData>>,
    /// Whole, decoded, and checked against what was asked for.
    held: Vec<Chunk>,
}

impl Fetch {
    /// Begin fetching the facet `notice` describes.
    ///
    /// # Errors
    ///
    /// [`FetchError::TooWide`] for an extent whose chunks the wire cannot name.
    pub fn of(notice: WorldNotice) -> Result<Self, FetchError> {
        let extent = BlockExtent {
            wide: notice.blocks.wide,
            down: notice.blocks.down,
        };
        // Before `chunks_of` and not after: the extent came off a socket, and a
        // facet claiming a billion blocks would otherwise be a hundred million
        // coordinates allocated on the way to refusing them.
        let chunks_wide = extent.wide.div_ceil(openshard_map::chunk::BLOCKS_PER_CHUNK);
        let chunks_down = extent.down.div_ceil(openshard_map::chunk::BLOCKS_PER_CHUNK);
        if u16::try_from(chunks_wide).is_err() || u16::try_from(chunks_down).is_err() {
            return Err(FetchError::TooWide {
                blocks: notice.blocks,
            });
        }
        let wanted: Vec<ChunkAt> = chunks_of(extent)
            .map(|at| ChunkAt {
                x: at.x as u16,
                y: at.y as u16,
            })
            .collect();
        Ok(Self {
            facet: notice.facet,
            extent,
            outstanding: FxHashMap::default(),
            held: Vec::with_capacity(wanted.len()),
            wanted,
            asked: 0,
        })
    }

    /// Which facet this is fetching.
    #[must_use]
    pub const fn facet(&self) -> Facet {
        self.facet
    }

    /// How many chunks the facet has.
    #[must_use]
    pub fn wanted(&self) -> usize {
        self.wanted.len()
    }

    /// How many of them are whole and decoded — the progress line's numerator.
    #[must_use]
    pub fn held(&self) -> usize {
        self.held.len()
    }

    /// Whether every chunk has arrived.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.held.len() == self.wanted.len()
    }

    /// The next request to put on the wire, or `None` while the pipe is as full
    /// as [`IN_FLIGHT_CHUNKS`] allows.
    ///
    /// Call it in a loop until it says `None`: at the start of a fetch that is
    /// four full requests, and after that it is one whenever a request's worth
    /// of chunks has come back. The chunks it names leave `wanted` and become
    /// outstanding in the same call, so a caller that dropped the packet on the
    /// floor would stall rather than ask twice — which is the honest failure,
    /// since a socket that lost a request has lost the connection.
    ///
    /// **A request is full, or it is the last one.** The window is a count of
    /// chunks, so one chunk coming back is room for exactly one going out, and
    /// topping up per chunk would ask for Felucca in four full requests and then
    /// six thousand nine hundred naming one chunk each. Waiting until there is
    /// room for a whole request leaves `IN_FLIGHT_CHUNKS - MAX_CHUNKS` chunks on
    /// the wire while the next one is being asked for, which is what keeps the
    /// pipe from draining; the tail of the facet is the one short request, and
    /// it cannot deadlock because the window only ever empties.
    pub fn next_request(&mut self) -> Option<ChunkRequest> {
        let room = IN_FLIGHT_CHUNKS.saturating_sub(self.outstanding.len());
        let left = self.wanted.len() - self.asked;
        let take = room.min(left).min(MAX_CHUNKS as usize);
        if take == 0 || (take < MAX_CHUNKS as usize && take < left) {
            return None;
        }
        let chunks = self.wanted[self.asked..self.asked + take].to_vec();
        self.asked += take;
        for &at in &chunks {
            self.outstanding.insert(at, Vec::new());
        }
        Some(ChunkRequest {
            facet: self.facet,
            chunks,
        })
    }

    /// Fold one packet in, answering whether it was part of the fetch.
    ///
    /// `false` is everything that is not [`ChunkData`] or
    /// [`ChunkRefused`](openshard_protocol::chunks::ChunkRefused) — the caller's
    /// to deliver as it delivers every other packet. `true` is a packet this
    /// consumed, and nothing above needs to see one: a chunk of ground is not a
    /// fact about the world the way a mobile or an item is, it *is* the world,
    /// and it arrives as one value when the last of it does.
    ///
    /// # Errors
    ///
    /// [`FetchError`]. Every one of them ends the fetch.
    pub fn on_packet(&mut self, packet: &ServerPacket) -> Result<bool, FetchError> {
        match packet {
            ServerPacket::ChunkData(data) => {
                self.fragment(data)?;
                Ok(true)
            }
            ServerPacket::ChunkRefused(refused) => Err(FetchError::Refused {
                at: refused.at,
                reason: refused.reason,
            }),
            _ => Ok(false),
        }
    }

    /// One fragment, and the chunk it completes if it is the last of them.
    fn fragment(&mut self, data: &ChunkData) -> Result<(), FetchError> {
        let unasked = || FetchError::Unasked {
            facet: data.facet,
            at: data.at,
        };
        if data.facet != self.facet {
            return Err(unasked());
        }
        let pieces = self.outstanding.get_mut(&data.at).ok_or_else(unasked)?;
        pieces.push(data.clone());
        // The fragments of one chunk arrive together and in order on one
        // stream, so this is an equality in practice; `join` is what actually
        // decides whether the set is `0..count` once each, and it is what
        // catches the fragment that arrived twice instead of the one that never
        // did.
        if pieces.len() < usize::from(data.fragment.count()) {
            return Ok(());
        }
        let pieces = self
            .outstanding
            .remove(&data.at)
            .expect("the fragments just pushed to");
        let record = join(&pieces).map_err(|source| FetchError::Join { at: data.at, source })?;
        let chunk = codec::decode(&record).map_err(|source| FetchError::Record { at: data.at, source })?;
        let asked = ChunkKey {
            facet: self.facet,
            at: ChunkCoord {
                x: u32::from(data.at.x),
                y: u32::from(data.at.y),
            },
        };
        if chunk.key() != asked {
            return Err(FetchError::WrongChunk {
                asked: data.at,
                found: chunk.key(),
            });
        }
        self.held.push(chunk);
        Ok(())
    }

    /// The facet, out of every chunk of it.
    ///
    /// Through the same [`assemble`] a base set is read through, which is the
    /// point: "the client's world" and "the shard's world" are one code path and
    /// not two that agree.
    ///
    /// **The revision is the chunks' own and not the notice's.** They are the
    /// same number for a world that held still, and where they differ it is the
    /// chunks that are right: a publish between the notice and the fetch moves
    /// the shard's snapshot, and what arrived is what arrived. `assemble` is
    /// what refuses a set that straddles a publish — half a facet before an edit
    /// and half after is a world that never existed — so by the time this reads
    /// one chunk's field, every chunk carries it.
    ///
    /// # Errors
    ///
    /// [`FetchError::Assembly`] for a set of chunks that is not one facet. It
    /// cannot be called before [`is_complete`](Self::is_complete), which is the
    /// only reason a short set is not one of the ways this fails.
    ///
    /// # Panics
    ///
    /// If no chunk arrived at all, which is a facet of nought chunks: the shard
    /// sends no notice for a facet with no ground, so there is no way to build a
    /// `Fetch` over one.
    pub fn finish(self) -> Result<MapSnapshot, FetchError> {
        let revision = self
            .held
            .first()
            .expect("a facet the shard sent a notice for has at least one chunk")
            .revision();
        let map = assemble(self.facet, self.extent, &self.held)
            .map_err(|source| FetchError::Assembly { source })?;
        Ok(MapSnapshot::restored(self.facet, revision, map))
    }
}

#[cfg(test)]
mod tests {
    use openshard_map::chunk::CHUNK_TILES;
    use openshard_map::map::{LandCell, StaticItem, WorldMap};
    use openshard_protocol::chunks::{ChunkRefused, WorldRevision};
    use openshard_protocol::wire::{Graphic, Hue};
    use openshard_tiles::LandTileId;

    use super::*;

    const FACET: Facet = Facet(0);
    /// Nine blocks square: not a whole number of chunks either way, so three of
    /// the four chunks are edge chunks and `chunk_extent`'s clamp is exercised.
    /// The shard's own `chunks_tests` uses the same fixture for the same reason.
    const BLOCKS: u32 = 9;

    fn extent() -> BlockExtent {
        BlockExtent {
            wide: BLOCKS,
            down: BLOCKS,
        }
    }

    /// A facet whose every tile is different, and with statics on both sides of
    /// each chunk seam.
    ///
    /// The heights vary on purpose: a fixture of one flat land id would survive
    /// a fetch that transposed two chunks, and this is the one test where the
    /// chunks are put back together by code that has never seen the shard.
    fn a_facet() -> MapSnapshot {
        let mut map = WorldMap::from_blocks(extent(), |x, y| LandCell {
            tile: LandTileId(x.wrapping_mul(7).wrapping_add(y)),
            z: (x as i32 - y as i32) as i8,
        });
        let seam = CHUNK_TILES as u16;
        for (n, (x, y)) in [
            (0, 0),
            (seam - 1, 3),
            (seam, 3),
            (3, seam - 1),
            (3, seam),
            (70, 70),
        ]
        .into_iter()
        .enumerate()
        {
            map.place_static(StaticItem {
                tile: Graphic(0x4000 + u16::try_from(n).unwrap()),
                x,
                y,
                z: i8::try_from(n).unwrap(),
                hue: Hue(0),
            });
        }
        MapSnapshot::new(FACET, map)
    }

    fn notice() -> WorldNotice {
        WorldNotice {
            facet: FACET,
            blocks: FacetBlocks {
                wide: BLOCKS,
                down: BLOCKS,
            },
            revision: WorldRevision(1),
        }
    }

    /// Every packet the shard would answer one request with, in order.
    ///
    /// `openshard_world`'s `chunk_answers` in miniature, and deliberately not a
    /// call to it: this crate is below the shard and cannot see it, so what is
    /// shared is the pair of functions in `openshard_protocol` that both ends
    /// use — which is the whole reason they live there.
    fn answers(snapshot: &MapSnapshot, request: &ChunkRequest) -> Vec<ServerPacket> {
        let revision = WorldRevision(snapshot.revision().get());
        request
            .chunks
            .iter()
            .flat_map(|&at| {
                let coord = ChunkCoord {
                    x: u32::from(at.x),
                    y: u32::from(at.y),
                };
                let chunk = Chunk::of(snapshot, coord).expect("a chunk of this facet");
                ChunkData::fragments(request.facet, at, revision, &codec::encode(&chunk))
                    .into_iter()
                    .map(ServerPacket::ChunkData)
            })
            .collect()
    }

    /// Drive a whole fetch against `snapshot`, answering every request the way
    /// the shard would.
    fn fetched(snapshot: &MapSnapshot) -> MapSnapshot {
        let mut fetch = Fetch::of(notice()).expect("a facet the wire can name");
        loop {
            let mut asked = 0;
            while let Some(request) = fetch.next_request() {
                asked += request.chunks.len();
                assert!(
                    request.chunks.len() <= MAX_CHUNKS as usize,
                    "a request over the protocol's own cap"
                );
                for packet in answers(snapshot, &request) {
                    assert!(fetch.on_packet(&packet).expect("the shard's own bytes"));
                }
            }
            if asked == 0 {
                break;
            }
        }
        assert!(fetch.is_complete());
        fetch.finish().expect("a complete set of chunks")
    }

    /// The whole of E2's client side, without a socket: a facet cut into chunks,
    /// fragmented, joined, decoded and assembled is the facet.
    #[test]
    fn a_facet_survives_being_fetched() {
        let sent = a_facet();
        let arrived = fetched(&sent);

        assert_eq!(arrived.facet(), sent.facet());
        assert_eq!(arrived.revision(), sent.revision());
        assert_eq!(arrived.map().width(), sent.map().width());
        assert_eq!(arrived.map().height(), sent.map().height());
        assert_eq!(arrived.map().static_count(), sent.map().static_count());
        for y in 0..sent.map().height() as u16 {
            for x in 0..sent.map().width() as u16 {
                assert_eq!(
                    arrived.map().land(x, y),
                    sent.map().land(x, y),
                    "the land at ({x}, {y})"
                );
                let there: Vec<StaticItem> = arrived.map().statics_at(x, y).copied().collect();
                let here: Vec<StaticItem> = sent.map().statics_at(x, y).copied().collect();
                assert_eq!(there, here, "the statics at ({x}, {y})");
            }
        }
    }

    /// Every chunk is asked for exactly once, in `chunks_of` order, and never
    /// more than [`IN_FLIGHT_CHUNKS`] are outstanding at a time.
    #[test]
    fn the_facet_is_asked_for_once_and_in_order() {
        let snapshot = a_facet();
        let mut fetch = Fetch::of(notice()).expect("a facet the wire can name");
        let mut order = Vec::new();
        loop {
            let mut asked = Vec::new();
            while let Some(request) = fetch.next_request() {
                asked.push(request);
            }
            if asked.is_empty() {
                break;
            }
            let outstanding: usize = asked.iter().map(|request| request.chunks.len()).sum();
            assert!(outstanding <= IN_FLIGHT_CHUNKS, "{outstanding} chunks in flight");
            for request in &asked {
                order.extend(request.chunks.iter().copied());
                for packet in answers(&snapshot, request) {
                    fetch.on_packet(&packet).expect("the shard's own bytes");
                }
            }
        }

        let expected: Vec<ChunkAt> = chunks_of(extent())
            .map(|at| ChunkAt {
                x: at.x as u16,
                y: at.y as u16,
            })
            .collect();
        assert_eq!(order, expected);
    }

    /// A refusal ends the fetch and names the chunk. The shard cannot honestly
    /// send one here — the chunks asked for are `chunks_of` the extent it named
    /// — which is exactly why it must not pass unnoticed.
    #[test]
    fn a_refusal_ends_the_fetch() {
        let mut fetch = Fetch::of(notice()).expect("a facet the wire can name");
        let request = fetch.next_request().expect("a facet has chunks");
        let at = request.chunks[0];
        let refused = ServerPacket::ChunkRefused(ChunkRefused {
            facet: FACET,
            at,
            reason: Refusal::PastTheEdge,
        });
        assert!(matches!(
            fetch.on_packet(&refused),
            Err(FetchError::Refused {
                at: refused_at,
                reason: Refusal::PastTheEdge,
            }) if refused_at == at
        ));
    }

    /// A chunk nobody asked for is refused rather than kept. It is how a shard
    /// answering a request it invented would look, and there is no reading of it
    /// that leaves this end holding the facet it asked for.
    #[test]
    fn a_chunk_nobody_asked_for_is_refused() {
        let snapshot = a_facet();
        let mut fetch = Fetch::of(notice()).expect("a facet the wire can name");
        // Nothing has been asked for yet, so every chunk is unasked — including
        // one that is genuinely on the facet.
        let at = ChunkAt { x: 0, y: 0 };
        let chunk = Chunk::of(&snapshot, ChunkCoord { x: 0, y: 0 }).expect("a chunk of this facet");
        let packets = ChunkData::fragments(FACET, at, WorldRevision(1), &codec::encode(&chunk));
        assert!(matches!(
            fetch.on_packet(&ServerPacket::ChunkData(packets[0].clone())),
            Err(FetchError::Unasked { facet: FACET, at: unasked }) if unasked == at
        ));

        // And the same packet on a facet this fetch is not about.
        let mut fetch = Fetch::of(notice()).expect("a facet the wire can name");
        fetch.next_request().expect("a facet has chunks");
        let mut elsewhere = packets[0].clone();
        elsewhere.facet = Facet(3);
        assert!(matches!(
            fetch.on_packet(&ServerPacket::ChunkData(elsewhere)),
            Err(FetchError::Unasked { facet: Facet(3), .. })
        ));
    }

    /// The record that arrived has to be the chunk that was asked for. Nothing
    /// downstream can tell the difference: `assemble` would refuse the facet for
    /// the blocks the duplicate did not cover, and name the innocent chunk.
    #[test]
    fn a_record_that_is_a_different_chunk_is_refused() {
        let snapshot = a_facet();
        let mut fetch = Fetch::of(notice()).expect("a facet the wire can name");
        let request = fetch.next_request().expect("a facet has chunks");
        assert!(request.chunks.len() > 1, "the fixture has four chunks");
        // Chunk (0, 0)'s bytes, sent under the name of the next chunk asked for.
        let chunk = Chunk::of(&snapshot, ChunkCoord { x: 0, y: 0 }).expect("a chunk of this facet");
        let wrong = ChunkData::fragments(FACET, request.chunks[1], WorldRevision(1), &codec::encode(&chunk));
        // Every fragment of it: the record is not read until the last one lands,
        // which is what makes this a check on the *chunk* and not on a packet.
        let last = wrong.len() - 1;
        for packet in &wrong[..last] {
            assert!(
                fetch
                    .on_packet(&ServerPacket::ChunkData(packet.clone()))
                    .expect("a fragment of a chunk that was asked for"),
            );
        }
        assert!(matches!(
            fetch.on_packet(&ServerPacket::ChunkData(wrong[last].clone())),
            Err(FetchError::WrongChunk { asked, found })
                if asked == request.chunks[1] && found.at == ChunkCoord { x: 0, y: 0 }
        ));
    }

    /// A facet bigger than the window: it is asked for a window at a time, in
    /// full requests, and the rest waits for chunks to come back.
    ///
    /// No bytes behind it, deliberately — what is under test is the cursor, and
    /// a chunk that never arrives is exactly what holds the window shut.
    #[test]
    fn no_more_than_the_window_is_ever_in_flight() {
        // 17 x 16 chunks: 272 of them, comfortably past the window.
        let mut fetch = Fetch::of(WorldNotice {
            facet: FACET,
            blocks: FacetBlocks {
                wide: 17 * openshard_map::chunk::BLOCKS_PER_CHUNK,
                down: 16 * openshard_map::chunk::BLOCKS_PER_CHUNK,
            },
            revision: WorldRevision(1),
        })
        .expect("a facet the wire can name");
        assert_eq!(fetch.wanted(), 272);

        let mut asked = 0;
        let mut requests = 0;
        while let Some(request) = fetch.next_request() {
            assert_eq!(
                request.chunks.len(),
                MAX_CHUNKS as usize,
                "a request under the cap while 272 chunks are still unasked"
            );
            asked += request.chunks.len();
            requests += 1;
        }
        assert_eq!(asked, IN_FLIGHT_CHUNKS, "the window, and not the facet");
        assert_eq!(requests, IN_FLIGHT_CHUNKS / MAX_CHUNKS as usize);
        assert_eq!(fetch.held(), 0);
        assert!(!fetch.is_complete());
    }

    /// And what reopens it: a whole request's worth of chunks coming back.
    ///
    /// The tail is the one short request, which is what this ends on — 272
    /// chunks is four full ones and sixteen left over.
    #[test]
    fn a_request_of_chunks_coming_back_makes_room_for_the_next() {
        let blocks = FacetBlocks {
            wide: 17 * openshard_map::chunk::BLOCKS_PER_CHUNK,
            down: 16 * openshard_map::chunk::BLOCKS_PER_CHUNK,
        };
        // Flat and empty: the pacing is what is under test, and every chunk of
        // this facet is the cheapest one that can exist.
        let snapshot = MapSnapshot::new(
            FACET,
            WorldMap::from_blocks(
                BlockExtent {
                    wide: blocks.wide,
                    down: blocks.down,
                },
                |_, _| LandCell {
                    tile: LandTileId(3),
                    z: 0,
                },
            ),
        );
        let mut fetch = Fetch::of(WorldNotice {
            facet: FACET,
            blocks,
            revision: WorldRevision(1),
        })
        .expect("a facet the wire can name");

        let mut window = Vec::new();
        while let Some(request) = fetch.next_request() {
            window.push(request);
        }
        assert!(fetch.next_request().is_none(), "the window is shut");

        // One request's worth of them arrives, and no more.
        for packet in answers(&snapshot, &window[0]) {
            fetch.on_packet(&packet).expect("the shard's own bytes");
        }
        assert_eq!(fetch.held(), MAX_CHUNKS as usize);

        let tail = fetch.next_request().expect("room for the rest of the facet");
        assert_eq!(tail.chunks.len(), 272 - IN_FLIGHT_CHUNKS, "the tail is short");
        assert!(
            fetch.next_request().is_none(),
            "and there is nothing left to ask for"
        );
    }

    /// A facet whose chunks the wire cannot name is refused before the list of
    /// them is built.
    #[test]
    fn a_facet_too_wide_to_name_is_refused() {
        let notice = WorldNotice {
            facet: FACET,
            blocks: FacetBlocks {
                wide: 8 * 70_000,
                down: 8,
            },
            revision: WorldRevision(1),
        };
        assert!(matches!(Fetch::of(notice), Err(FetchError::TooWide { .. })));
    }

    /// A packet that is not part of the fetch is handed back for the caller to
    /// deliver: the ground arrives on the one stream everything else does.
    #[test]
    fn another_packet_is_not_the_fetchs() {
        let mut fetch = Fetch::of(notice()).expect("a facet the wire can name");
        let elsewhere = ServerPacket::LoginComplete(openshard_protocol::world::LoginComplete);
        assert!(
            !fetch.on_packet(&elsewhere).expect("a packet that is not a chunk"),
            "everything but a chunk packet is handed back"
        );
    }
}
