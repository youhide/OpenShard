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
//! # Unless it is already here
//!
//! E3's half, and it is the same transfer with a different list: a client that
//! kept the ground it was given comes back holding a world at some revision, is
//! told the shard is at another, and asks *what moved* — see [`crate::cache`],
//! which owns the copy on disk, and `openshard_protocol::chunks::ChangesRequest`,
//! which is the question. What comes back then is a handful of chunks rather
//! than a facet, and [`Fetch::over`] is the same state machine pointed at them:
//! the pacing, the checks and the bookkeeping are one implementation, because
//! the difference between "all of it" and "these four" is a list.
//!
//! # Or it moves while it is being drawn
//!
//! E4's half, and it is the same list again with the world one thread further
//! away. A shard that commits a patch says so — `openshard_protocol::chunks::PublishNotice`
//! — and by then this end has handed the facet to the window, so
//! [`Fetch::moved`] ends in the chunks themselves rather than in a world. What
//! they are applied over is the window's, which is the only copy there is.
//!
//! # Or it moves while it is still arriving
//!
//! The three above are a fetch that runs to its end. This is the one that does
//! not: a publish lands while chunks are on the wire, and every answer still
//! coming was cut at a revision the shard has already moved past. See
//! [`Fetch::abandon`], [`Drain`] and [`Restart`] — the fetch stops, what it is
//! still owed is eaten rather than decoded, and what to ask for again is the
//! union of what it was asking about and what the publish named.
//!
//! [`WorldMap`]: openshard_map::map::WorldMap

use openshard_map::chunk::{
    Chunk,
    ChunkCoord,
    ChunkKey,
    assemble,
    chunks_of,
};
use openshard_map::codec;
use openshard_map::grid::BlockExtent;
use openshard_map::snapshot::{
    MapRevision,
    MapSnapshot,
};
use openshard_protocol::chunks::{
    Changes,
    ChunkAt,
    ChunkData,
    ChunkRequest,
    FacetBlocks,
    JoinError,
    MAX_CHUNKS,
    Refusal,
    WorldNotice,
    join,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::world::Facet;
use rustc_hash::{
    FxHashMap,
    FxHashSet,
};

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
        at:     ChunkAt,
        /// Which of the two facts the shard stated.
        reason: Refusal,
    },
    /// A chunk arrived that was never asked for.
    Unasked {
        /// Which facet it claimed.
        facet: Facet,
        /// Which chunk of it.
        at:    ChunkAt,
    },
    /// The fragments of one chunk do not make one blob.
    Join {
        /// Which chunk.
        at:     ChunkAt,
        /// Why.
        source: JoinError,
    },
    /// The blob a chunk's fragments joined into is not a chunk record.
    Record {
        /// Which chunk.
        at:     ChunkAt,
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
    /// A chunk of the world that was asked about arrived at another revision.
    ///
    /// Only a fetch of what moved — [`over`](Fetch::over) a world already held,
    /// or [`moved`](Fetch::moved) for one the window holds — can fail this way,
    /// and it is the reason both name the revision they expect: the list of
    /// chunks to ask for was the difference between two *particular* revisions,
    /// so a publish landing between the answer and the fetch makes it a list of
    /// the wrong chunks. Every other chunk of the facet moved to that new
    /// revision too, and nothing here was told which.
    ///
    /// A whole-facet fetch has no such expectation — `assemble` refuses a set
    /// that straddles a publish, which is the same fact one level down.
    WrongRevision {
        /// Which chunk.
        at:     ChunkAt,
        /// The revision the difference was computed against.
        wanted: MapRevision,
        /// The revision the chunk was cut at.
        found:  MapRevision,
    },
    /// The world a fetch was told to fill in is not the world being described.
    ///
    /// A cache of the right facet at the right revision whose *extent* is not
    /// the one the notice named. It cannot come from [`crate::cache`], which
    /// checks the pair before it hands a world over, so it is here for the
    /// caller that assembles one itself.
    WrongWorld {
        /// What the shard described.
        blocks: FacetBlocks,
        /// What the world in hand is.
        held:   BlockExtent,
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
            Self::TooWide { blocks } => {
                write!(
                    f,
                    "a facet of {}x{} blocks has chunks the wire cannot name",
                    blocks.wide, blocks.down
                )
            }
            Self::Refused { at, reason } => {
                write!(f, "chunk ({}, {}) was refused: {reason}", at.x, at.y)
            }
            Self::Unasked { facet, at } => {
                write!(
                    f,
                    "chunk ({}, {}) of facet {} arrived and nobody asked for it",
                    at.x, at.y, facet.0
                )
            }
            Self::Join { at, source } => {
                write!(f, "chunk ({}, {}) did not arrive whole: {source}", at.x, at.y)
            }
            Self::Record { at, source } => {
                write!(f, "chunk ({}, {}) is not a chunk record: {source}", at.x, at.y)
            }
            Self::WrongChunk { asked, found } => {
                write!(
                    f,
                    "chunk ({}, {}) was asked for and chunk ({}, {}) of facet {} arrived",
                    asked.x, asked.y, found.at.x, found.at.y, found.facet.0
                )
            }
            Self::WrongRevision { at, wanted, found } => {
                write!(
                    f,
                    "chunk ({}, {}) arrived at revision {} and what moved was asked about revision {}",
                    at.x,
                    at.y,
                    found.get(),
                    wanted.get()
                )
            }
            Self::WrongWorld { blocks, held } => {
                write!(
                    f,
                    "a world of {}x{} blocks was kept and the shard describes one of {}x{}",
                    held.wide, held.down, blocks.wide, blocks.down
                )
            }
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
            Self::TooWide { .. }
            | Self::Refused { .. }
            | Self::Unasked { .. }
            | Self::WrongChunk { .. }
            | Self::WrongRevision { .. }
            | Self::WrongWorld { .. } => None,
        }
    }
}

/// What a fetch is filling in.
///
/// The three arms are the three ways a client comes to hold ground, and they
/// differ in one thing each: which chunks are asked for, and what the ones that
/// arrive are put into. Everything between — the pacing, the fragments, the
/// checks — is the same, which is why this is a field of [`Fetch`] rather than
/// three types beside each other.
#[derive(Debug)]
enum Filling {
    /// Nothing was held: every chunk of the facet is coming, and `assemble` is
    /// what turns the set into a world.
    Nothing,
    /// A world already in hand, and only what has moved since is coming.
    ///
    /// The revision is the one the difference was computed against — see
    /// [`FetchError::WrongRevision`], which is what makes it a field rather than
    /// a thing to check afterwards.
    Held {
        /// The world as it was, out of [`crate::cache`].
        world:    MapSnapshot,
        /// The revision every arriving chunk has to carry.
        revision: MapRevision,
    },
    /// What has moved, for a world that is somewhere else.
    ///
    /// E4's arm, and the difference from [`Held`](Self::Held) is *whose* world
    /// it is. A publish reaches a client that is already drawing, and by then the
    /// facet has been handed to the window — a
    /// [`MapSnapshot`](openshard_map::snapshot::MapSnapshot) has one owner per
    /// process by construction, so the thread that owns the socket has nothing
    /// left to apply chunks over. What it hands back is the chunks, and the
    /// window puts them into the world it is drawing from.
    ///
    /// The revision is checked exactly as `Held`'s is, and for the same reason:
    /// the list of chunks is a statement about two particular revisions.
    Loose {
        /// The revision every arriving chunk has to carry.
        revision: MapRevision,
    },
}

/// One facet's ground, arriving.
///
/// Built from the [`WorldNotice`] the shard sends on world entry, which is the
/// one thing a client is told without asking and the one thing it needs before
/// it can ask: the extent is what `assemble` refuses a short set of chunks
/// against, and the chunks to ask for are `chunks_of` it.
///
/// [`Fetch::over`] is the same machine with a shorter list, for a client that
/// kept the last world it was given.
#[derive(Debug)]
pub struct Fetch {
    /// Which facet is being fetched. Every reply is checked against it.
    facet:       Facet,
    /// How big the shard said it is. `assemble`'s second argument, and the only
    /// thing that makes a set of chunks a *whole* facet rather than a narrower
    /// world that parses perfectly.
    extent:      BlockExtent,
    /// Every chunk the facet has, in `chunks_of` order.
    ///
    /// The order is the base set's own, which is not a coincidence: E3's cache
    /// is a base set, and a facet fetched in the order it is written is a facet
    /// that can be written as it arrives.
    wanted:      Vec<ChunkAt>,
    /// How many of [`wanted`](Self::wanted) have been asked for. A cursor rather
    /// than a set, because nothing is ever asked for twice.
    asked:       usize,
    /// Asked for and not yet whole: the fragments each has so far.
    ///
    /// A chunk leaves this map when it is whole, which is what makes "is this
    /// chunk outstanding" the same question as "did anybody ask for it".
    outstanding: FxHashMap<ChunkAt, Vec<ChunkData>>,
    /// Whole, decoded, and checked against what was asked for.
    held:        Vec<Chunk>,
    /// What the chunks are being put into. See [`Filling`].
    filling:     Filling,
}

impl Fetch {
    /// Begin fetching the facet `notice` describes, whole.
    ///
    /// # Errors
    ///
    /// [`FetchError::TooWide`] for an extent whose chunks the wire cannot name.
    pub fn of(notice: WorldNotice) -> Result<Self, FetchError> {
        let extent = extent_of(notice)?;
        let wanted: Vec<ChunkAt> = chunks_of(extent)
            .map(|at| {
                ChunkAt {
                    x: at.x as u16,
                    y: at.y as u16,
                }
            })
            .collect();
        Ok(Self::new(notice.facet, extent, wanted, Filling::Nothing))
    }

    /// Begin fetching only `moved`, over the world already in `held`.
    ///
    /// E3's arm. `revision` is what the shard said it is at — the revision the
    /// list of moved chunks was computed against, and the one every chunk that
    /// arrives has to carry, since a publish in between makes the list name the
    /// wrong squares.
    ///
    /// An empty `moved` is not a fetch: a world that has not moved is answered
    /// before this is called, by keeping the one already held. It is refused
    /// with a panic rather than by finishing instantly, because a caller that
    /// got here with nothing to ask for has skipped that decision.
    ///
    /// # Errors
    ///
    /// [`FetchError::TooWide`] for an extent whose chunks the wire cannot name,
    /// and [`FetchError::WrongWorld`] for a world that is not the size the
    /// notice describes.
    ///
    /// # Panics
    ///
    /// If `moved` is empty.
    pub fn over(
        notice: WorldNotice,
        held: MapSnapshot,
        moved: Vec<ChunkAt>,
        revision: MapRevision,
    ) -> Result<Self, FetchError> {
        assert!(!moved.is_empty(), "a fetch of nothing is a world already held");
        let extent = extent_of(notice)?;
        if held.map().extent() != extent {
            return Err(FetchError::WrongWorld {
                blocks: notice.blocks,
                held:   held.map().extent(),
            });
        }
        Ok(Self::new(
            notice.facet,
            extent,
            moved,
            Filling::Held {
                world: held,
                revision,
            },
        ))
    }

    /// Begin fetching `moved` as chunks, over a world this end does not hold.
    ///
    /// E4's arm: the shard has published a patch and named the chunks it touched,
    /// and the facet those chunks belong to was handed to the window a whole
    /// fetch ago. So this ends in [`Fetched::Chunks`] rather than
    /// in a world — see [`Filling::Loose`], which is where that is argued.
    ///
    /// `revision` is the one the publish named, and every chunk that arrives has
    /// to carry it: a second publish landing between the notice and the fetch
    /// makes this list a list of the wrong squares, exactly as it does for
    /// [`over`](Self::over).
    ///
    /// # Errors
    ///
    /// [`FetchError::TooWide`] for an extent whose chunks the wire cannot name.
    /// There is no `WrongWorld` here, because there is no world in hand to be
    /// the wrong one — the window checks its own when it applies them.
    ///
    /// # Panics
    ///
    /// If `moved` is empty, for [`over`](Self::over)'s reason: a publish that
    /// moved no chunk is not announced, and a caller that got here with nothing
    /// to ask for has skipped a decision.
    pub fn moved(
        notice: WorldNotice,
        moved: Vec<ChunkAt>,
        revision: MapRevision,
    ) -> Result<Self, FetchError> {
        assert!(
            !moved.is_empty(),
            "a fetch of nothing is a world that did not move"
        );
        let extent = extent_of(notice)?;
        Ok(Self::new(
            notice.facet,
            extent,
            moved,
            Filling::Loose { revision },
        ))
    }

    /// The parts every constructor shares.
    fn new(facet: Facet, extent: BlockExtent, wanted: Vec<ChunkAt>, filling: Filling) -> Self {
        Self {
            facet,
            extent,
            outstanding: FxHashMap::default(),
            held: Vec::with_capacity(wanted.len()),
            wanted,
            asked: 0,
            filling,
        }
    }

    /// Which facet this is fetching.
    #[must_use]
    pub const fn facet(&self) -> Facet {
        self.facet
    }

    /// Whether this is what moved rather than the facet.
    ///
    /// What a progress line says about itself: "the ground" and "what moved" are
    /// different news, and four chunks arriving is not the same event as 7,168.
    /// Both of the narrow arms answer `true` — where the world they belong to
    /// lives is not what a person watching wants to be told.
    #[must_use]
    pub const fn is_over_a_world(&self) -> bool {
        matches!(self.filling, Filling::Held { .. } | Filling::Loose { .. })
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
            ServerPacket::ChunkRefused(refused) => {
                Err(FetchError::Refused {
                    at:     refused.at,
                    reason: refused.reason,
                })
            }
            _ => Ok(false),
        }
    }

    /// One fragment, and the chunk it completes if it is the last of them.
    fn fragment(&mut self, data: &ChunkData) -> Result<(), FetchError> {
        let unasked = || {
            FetchError::Unasked {
                facet: data.facet,
                at:    data.at,
            }
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
            at:    ChunkCoord {
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
        // Only for a fetch of what moved, and the asymmetry is the point: there
        // the list of chunks *is* a statement about two revisions, so a chunk
        // from a third one makes the rest of the list wrong. A whole facet has
        // nothing to be wrong about until `assemble` compares the set with
        // itself.
        match self.filling {
            Filling::Nothing => {}
            Filling::Held { revision, .. } | Filling::Loose { revision } => {
                if chunk.revision() != revision {
                    return Err(FetchError::WrongRevision {
                        at:     data.at,
                        wanted: revision,
                        found:  chunk.revision(),
                    });
                }
            }
        }
        self.held.push(chunk);
        Ok(())
    }

    /// What arrived, as the thing the caller asked for.
    ///
    /// A facet goes through the same [`assemble`] a base set is read through,
    /// which is the point: "the client's world" and "the shard's world" are one
    /// code path and not two that agree. A fetch over a world already held ends
    /// in [`apply`](openshard_map::chunk::apply) instead, which is `assemble`'s
    /// other half and ends in the same `WorldMap::from_parts`. A fetch of what
    /// [`moved`](Self::moved) ends in neither, because the world it belongs to is
    /// not on this thread — see [`Fetched`].
    ///
    /// **The revision is the chunks' own and not the notice's.** They are the
    /// same number for a world that held still, and where they differ it is the
    /// chunks that are right: a publish between the notice and the fetch moves
    /// the shard's snapshot, and what arrived is what arrived. `assemble` is
    /// what refuses a set that straddles a publish — half a facet before an edit
    /// and half after is a world that never existed — so by the time this reads
    /// one chunk's field, every chunk carries it. Over a world already held the
    /// same number was checked chunk by chunk on the way in, against the
    /// revision the difference was asked about.
    ///
    /// # Errors
    ///
    /// [`FetchError::Assembly`] for a set of chunks that is not one facet, or
    /// that does not fit the world it is being applied over. It cannot be called
    /// before [`is_complete`](Self::is_complete), which is the only reason a
    /// short set is not one of the ways this fails.
    ///
    /// # Panics
    ///
    /// If no chunk arrived at all, which is a facet of nought chunks: the shard
    /// sends no notice for a facet with no ground, so there is no way to build a
    /// `Fetch` over one.
    pub fn finish(self) -> Result<Fetched, FetchError> {
        let revision = self
            .held
            .first()
            .expect("a facet the shard sent a notice for has at least one chunk")
            .revision();
        match self.filling {
            Filling::Nothing => {
                let map = assemble(self.facet, self.extent, &self.held)
                    .map_err(|source| FetchError::Assembly { source })?;
                Ok(Fetched::World(MapSnapshot::restored(self.facet, revision, map)))
            }
            // The world that was kept, moved to where it was told the shard is:
            // `take_chunks` writes the squares in and re-stamps the revision in
            // one call, so there is no second snapshot to build around a facet
            // that never left this variant.
            Filling::Held { mut world, .. } => {
                world
                    .take_chunks(&self.held)
                    .map_err(|source| FetchError::Assembly { source })?;
                Ok(Fetched::World(world))
            }
            // Every check this would have made on the way to a world has been
            // made already — each chunk is the one that was asked for, and each
            // carries the revision the publish named. What is left is
            // `chunk::apply`, and it belongs to whoever holds the world.
            Filling::Loose { .. } => Ok(Fetched::Chunks(self.held)),
        }
    }

    /// Stop, because the world moved: what is still owed, and what to ask for
    /// again.
    ///
    /// The answer to a publish that lands while ground is arriving. Nothing this
    /// fetch is holding can be used — a chunk that arrived before the publish
    /// was cut at the revision the publish moved past, and one that arrives
    /// after it carries the new number, which is
    /// [`WrongRevision`](FetchError::WrongRevision) for the two narrow arms and
    /// a set `assemble` refuses for the wide one.
    ///
    /// **What cannot simply be dropped is the wire.** The shard answers every
    /// chunk it was asked for exactly once, so the answers to this fetch's last
    /// requests are already coming and nothing in them says which request they
    /// belong to — a restart that asked for the same square would take the
    /// abandoned answer for its own. That is what the [`Drain`] is: the
    /// bookkeeping, without the chunks.
    ///
    /// `published` and `revision` are the publish that caused this, folded in
    /// here rather than left to the caller because a [`Restart`] with no
    /// revision to fetch at is not a thing that should be constructible. Further
    /// publishes go through [`Restart::and`].
    pub fn abandon(self, published: &Changes, revision: MapRevision) -> (Drain, Restart) {
        let drain = Drain {
            facet: self.facet,
            owed:  self
                .outstanding
                .iter()
                .map(|(&at, pieces)| (at, pieces.len()))
                .collect(),
        };
        let (mut moved, whose) = match self.filling {
            // Nothing of this facet is on this side at all, so there is nothing
            // narrower than the whole of it to ask for: the chunks that had
            // already arrived are as abandoned as the ones that had not.
            Filling::Nothing => (Changes::Everything, Whose::Nobodys),
            Filling::Held { world, .. } => (Changes::These(self.wanted), Whose::Ours(world)),
            Filling::Loose { .. } => (Changes::These(self.wanted), Whose::Windows),
        };
        merge(&mut moved, published);
        (
            drain,
            Restart {
                moved,
                revision,
                whose,
            },
        )
    }
}

/// The answers an abandoned [`Fetch`] is still owed.
///
/// A chunk request is answered exactly once and the answer says nothing about
/// which request it belongs to, so a connection that abandoned a fetch and asked
/// again would have two sets of answers on the wire and no way to tell them
/// apart — the same square, at two revisions, and the wrong one might land
/// second. So the abandoned fetch is not dropped, it is turned into this: the
/// count of what it was owed, and nothing else. When [`is_empty`](Self::is_empty)
/// says so, the wire carries no answer to a question this connection has
/// forgotten and the restart can go out.
///
/// **It decodes nothing.** A discarded chunk is bytes to count and not a chunk:
/// the fragments are dropped as they arrive rather than joined, so what a drain
/// costs is one entry per outstanding chunk however big the facet is.
#[derive(Debug)]
pub struct Drain {
    /// The facet the abandoned fetch was about. Only for the log line: what
    /// arrives is eaten whatever facet it names — see [`on_packet`](Self::on_packet).
    facet: Facet,
    /// One entry per chunk asked for and not yet whole, and how many of its
    /// fragments have been seen. A chunk leaves when its last fragment arrives
    /// or when the shard refuses it, which are the only two things the shard
    /// can answer with.
    owed:  FxHashMap<ChunkAt, usize>,
}

impl Drain {
    /// Eat one packet, answering whether it was a chunk packet.
    ///
    /// `false` is everything the abandoned fetch would have handed back, and it
    /// is handed back for the same reason: the ground arrives on the one stream
    /// every other packet does.
    ///
    /// **Every chunk packet is eaten, whatever it names.** Nothing on this
    /// connection has asked for a chunk since the fetch was abandoned, so a
    /// chunk packet is an abandoned answer by construction; and one that is not
    /// — a facet nobody asked about, a square that already completed — is still
    /// nothing the window can be told about. What the bookkeeping recognises is
    /// narrower than what it consumes, on purpose: a drain that ended early
    /// would let the restart's answers race the abandoned ones, which is the
    /// whole thing this exists to prevent.
    pub fn on_packet(&mut self, packet: &ServerPacket) -> bool {
        match packet {
            ServerPacket::ChunkData(data) => {
                if data.facet == self.facet {
                    self.fragment(data);
                }
                true
            }
            // The other half of "answered exactly once", and the half
            // [`Fetch::on_packet`] has no reason to handle: there a refusal ends
            // the fetch, so what it leaves in `outstanding` never matters. Here
            // it is the answer, and a chunk still owed after it would hold the
            // drain open for a packet that is never coming.
            ServerPacket::ChunkRefused(refused) => {
                if refused.facet == self.facet {
                    self.owed.remove(&refused.at);
                }
                true
            }
            _ => false,
        }
    }

    /// One fragment of one abandoned chunk, counted and thrown away.
    fn fragment(&mut self, data: &ChunkData) {
        let Some(seen) = self.owed.get_mut(&data.at) else {
            return;
        };
        *seen += 1;
        // `>=` and not `==`: the count is the sender's, and a drain that held
        // itself open over a shard that sent a fragment twice would stop this
        // connection from ever asking for ground again. `join`'s check is what
        // this end has instead, and a discarded chunk is never joined.
        if *seen >= usize::from(data.fragment.count()) {
            self.owed.remove(&data.at);
        }
    }

    /// Whether every answer has arrived, so the wire is clean.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.owed.is_empty()
    }

    /// How many chunks are still owed — what a progress line says while a
    /// restart is waiting.
    #[must_use]
    pub fn owed(&self) -> usize {
        self.owed.len()
    }
}

/// Whose world an abandoned fetch was filling in.
///
/// [`Filling`] after the fact, and it is a second enum rather than that one
/// because a restart has no fragments, no cursor and no chunks — what survives
/// an abandonment is which of the three kinds of fetch to start again.
#[derive(Debug)]
enum Whose {
    /// Nobody's yet: the facet had not arrived, so a restart takes it whole.
    Nobodys,
    /// This thread's, out of [`crate::cache`]. It comes back with the restart,
    /// still at the revision it was kept at — nothing was applied over it,
    /// because a fetch applies at its end and this one had none.
    Ours(MapSnapshot),
    /// The window's: the facet was handed over a whole fetch ago, so a restart
    /// ends in chunks.
    Windows,
}

/// What to ask for once an abandoned fetch's answers have stopped arriving.
///
/// **The list is a union, and that is the whole rule.** What a fetch was asking
/// about is what moved between the world this end can still show and the
/// revision it was fetching; what a publish names is what moved between that
/// revision and the new one. A square in either list has moved as far as this
/// end is concerned, and nothing will ever name it again — the shard's next
/// notice is about its next patch, not about this one. So the two lists are put
/// together rather than the second replacing the first, and the result is asked
/// for at the newest revision.
///
/// The union can name more chunks than one [`ChangesReply`] could
/// — `MAX_MOVED` bounds a *packet*, and this is a list of things to request in
/// batches of [`MAX_CHUNKS`]. Where it stops being narrower than the facet is
/// the shard's own answer, [`Changes::Everything`], which absorbs everything it
/// is unioned with.
///
/// [`ChangesReply`]: openshard_protocol::chunks::ChangesReply
#[derive(Debug)]
pub struct Restart {
    /// Every square that moved between the world this end can still show and
    /// [`revision`](Self::revision).
    moved:    Changes,
    /// The revision to fetch at: the newest publish's, since each one is the
    /// world as it now stands.
    revision: MapRevision,
    /// Which of the three fetches to start again.
    whose:    Whose,
}

impl Restart {
    /// Fold another publish in: what it names moved as well.
    ///
    /// A second edit while the first one's answers are still draining, and it
    /// costs nothing but a longer list — the fetch has not gone out yet, so
    /// there is no second abandonment and no second drain.
    pub fn and(&mut self, published: &Changes, revision: MapRevision) {
        self.revision = revision;
        merge(&mut self.moved, published);
    }

    /// The fetch to start, once the [`Drain`] beside it is empty.
    ///
    /// # Errors
    ///
    /// [`FetchError::TooWide`], and [`FetchError::WrongWorld`] for the arm that
    /// carries a world — both of them [`Fetch`]'s own, and both about the notice
    /// rather than about anything a publish said.
    pub fn begin(self, notice: WorldNotice) -> Result<Fetch, FetchError> {
        match (self.moved, self.whose) {
            // The shard could not name what moved, or there was nothing on this
            // side to name it against. Either way the facet is taken again, and
            // a world in hand is worth nothing against it: `assemble` builds one
            // out of the chunks alone.
            (Changes::Everything, _) => Fetch::of(notice),
            (Changes::These(moved), Whose::Ours(world)) => Fetch::over(notice, world, moved, self.revision),
            (Changes::These(moved), Whose::Windows) => Fetch::moved(notice, moved, self.revision),
            (Changes::These(_), Whose::Nobodys) => {
                unreachable!("a fetch of the whole facet is abandoned as Everything")
            }
        }
    }
}

/// Put one list of moved squares into another, keeping each square once.
///
/// [`Changes::Everything`] is not a list and absorbs whatever it meets, in both
/// directions: it is the shard saying nothing narrower than the facet is safe to
/// take, and a union with a handful of named squares does not make it safer.
fn merge(into: &mut Changes, other: &Changes) {
    match (&mut *into, other) {
        (Changes::Everything, _) => {}
        (Changes::These(_), Changes::Everything) => *into = Changes::Everything,
        (Changes::These(have), Changes::These(more)) => {
            // A set and not `contains` per square: an operator holding a key
            // down publishes one chunk at a time, and a restart that has been
            // waiting through a hundred of them would be scanning a list that
            // long for each one. It also dedupes `more` against itself, which
            // the wire says is unnecessary and cannot be trusted to be — a
            // square in the list twice is a square asked for twice, and the
            // second answer is one nobody asked for.
            let mut known: FxHashSet<ChunkAt> = have.iter().copied().collect();
            for &at in more {
                if known.insert(at) {
                    have.push(at);
                }
            }
        }
    }
}

/// What a finished [`Fetch`] is.
///
/// Two arms because a fetch has two kinds of caller, and the difference is not
/// which chunks were asked for but **who owns the facet they belong to**. A
/// client with no ground yet, and one catching a kept world up before it draws,
/// both hold the world on the thread that owns the socket, so what they want is
/// the world. A client that is already drawing handed the facet to its window a
/// whole fetch ago — a
/// [`MapSnapshot`](openshard_map::snapshot::MapSnapshot) has one owner per
/// process by construction — so what it wants is the squares, to send across the
/// seam.
///
/// It is an enum rather than a second terminal method so that a caller has to
/// say which it got: the two are not interchangeable, and a `finish` that
/// panicked for the arm it was not built for would be a contract the compiler
/// cannot see.
#[derive(Debug)]
pub enum Fetched {
    /// The facet, assembled or applied.
    World(MapSnapshot),
    /// The chunks alone, every one of them at the revision the fetch was told to
    /// expect.
    Chunks(Vec<Chunk>),
}

impl Fetched {
    /// The world, for the caller that asked for one.
    ///
    /// # Panics
    ///
    /// If this is the chunks of a world somebody else holds, which the caller
    /// that built the fetch already knows it is not.
    #[must_use]
    pub fn world(self) -> MapSnapshot {
        match self {
            Self::World(snapshot) => snapshot,
            Self::Chunks(_) => panic!("a fetch of what moved was asked for its world"),
        }
    }
}

/// The facet's extent, refused before a list of its chunks is built.
///
/// Before `chunks_of` and not after: the extent came off a socket, and a facet
/// claiming a billion blocks would otherwise be a hundred million coordinates
/// allocated on the way to refusing them.
fn extent_of(notice: WorldNotice) -> Result<BlockExtent, FetchError> {
    let extent = BlockExtent {
        wide: notice.blocks.wide,
        down: notice.blocks.down,
    };
    let chunks_wide = extent.wide.div_ceil(openshard_map::chunk::BLOCKS_PER_CHUNK);
    let chunks_down = extent.down.div_ceil(openshard_map::chunk::BLOCKS_PER_CHUNK);
    if u16::try_from(chunks_wide).is_err() || u16::try_from(chunks_down).is_err() {
        return Err(FetchError::TooWide {
            blocks: notice.blocks,
        });
    }
    Ok(extent)
}

#[cfg(test)]
mod tests {
    use openshard_map::chunk::CHUNK_TILES;
    use openshard_map::map::{
        LandCell,
        StaticItem,
        WorldMap,
    };
    use openshard_protocol::chunks::{
        ChunkRefused,
        WorldRevision,
    };
    use openshard_protocol::wire::{
        Graphic,
        Hue,
    };
    use openshard_protocol::world::WorldId;
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
        let mut map = WorldMap::from_blocks(extent(), |x, y| {
            LandCell {
                tile: LandTileId(x.wrapping_mul(7).wrapping_add(y)),
                z:    (x as i32 - y as i32) as i8,
            }
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
        notice_of(FacetBlocks {
            wide: BLOCKS,
            down: BLOCKS,
        })
    }

    /// A notice about a facet of some other size, for the tests that are about
    /// the pacing rather than about the fixture.
    fn notice_of(blocks: FacetBlocks) -> WorldNotice {
        WorldNotice {
            facet: FACET,
            blocks,
            revision: WorldRevision(1),
            world: Some(WorldId(0x0EFA_CE00_0EFA_CE00)),
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
        fetch.finish().expect("a complete set of chunks").world()
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

    /// The same facet with one tile of ground moved and one static added, at
    /// whichever revision the caller says — a publish, as far as this end can
    /// tell.
    fn a_facet_that_moved(revision: MapRevision) -> MapSnapshot {
        let was = a_facet();
        let mut map = WorldMap::from_blocks(extent(), |x, y| {
            was.map()
                .land(x, y)
                .expect("a tile of the facet it was built from")
        });
        for y in 0..u16::try_from(BLOCKS * 8).unwrap() {
            for x in 0..u16::try_from(BLOCKS * 8).unwrap() {
                for item in was.map().statics_at(x, y) {
                    map.place_static(*item);
                }
            }
        }
        map.set_land(
            3,
            4,
            LandCell {
                tile: LandTileId(0x3FF),
                z:    12,
            },
        );
        map.place_static(StaticItem {
            tile: Graphic(0x4321),
            x:    70,
            y:    71,
            z:    3,
            hue:  Hue(9),
        });
        MapSnapshot::restored(FACET, revision, map)
    }

    /// E3's client side without a socket: a world already held, told which two
    /// chunks moved, comes out as the world the shard is holding — including the
    /// two chunks nobody sent.
    #[test]
    fn a_world_already_held_takes_the_chunks_that_moved() {
        let held = a_facet();
        let moved = a_facet_that_moved(a_facet().revision().after());
        let revision = moved.revision();
        let notice = WorldNotice {
            revision: WorldRevision(revision.get()),
            ..notice()
        };

        let mut fetch = Fetch::over(
            notice,
            held,
            vec![ChunkAt { x: 0, y: 0 }, ChunkAt { x: 1, y: 1 }],
            revision,
        )
        .expect("a world the size the notice describes");
        assert!(fetch.is_over_a_world());
        assert_eq!(fetch.wanted(), 2, "what moved, and not the facet");
        while let Some(request) = fetch.next_request() {
            for packet in answers(&moved, &request) {
                assert!(fetch.on_packet(&packet).expect("the shard's own bytes"));
            }
        }
        assert!(fetch.is_complete());

        let arrived = fetch.finish().expect("two chunks of this facet").world();
        assert_eq!(arrived.revision(), revision);
        assert_eq!(arrived.map().static_count(), moved.map().static_count());
        for y in 0..moved.map().height() as u16 {
            for x in 0..moved.map().width() as u16 {
                assert_eq!(
                    arrived.map().land(x, y),
                    moved.map().land(x, y),
                    "the land at ({x}, {y})"
                );
                let there: Vec<StaticItem> = arrived.map().statics_at(x, y).copied().collect();
                let here: Vec<StaticItem> = moved.map().statics_at(x, y).copied().collect();
                assert_eq!(there, here, "the statics at ({x}, {y})");
            }
        }
    }

    /// E4's client side: the same two chunks, fetched for a world that is not on
    /// this thread, come out as chunks — and putting them into the world by hand
    /// is the world the shard is holding.
    ///
    /// The oracle is deliberately the *same* one the test above uses: what
    /// changes between E3's arm and E4's is who owns the facet, and nothing about
    /// what arrives. So `apply` here stands in for the window, and the two tests
    /// have to agree tile for tile.
    #[test]
    fn what_moved_can_be_fetched_for_a_world_this_end_does_not_hold() {
        let held = a_facet();
        let moved = a_facet_that_moved(a_facet().revision().after());
        let revision = moved.revision();
        let notice = WorldNotice {
            revision: WorldRevision(revision.get()),
            ..notice()
        };

        let asked = vec![ChunkAt { x: 0, y: 0 }, ChunkAt { x: 1, y: 1 }];
        let mut fetch = Fetch::moved(notice, asked.clone(), revision).expect("a facet the wire can name");
        assert!(fetch.is_over_a_world(), "what moved, however it is applied");
        assert_eq!(fetch.wanted(), asked.len());
        while let Some(request) = fetch.next_request() {
            for packet in answers(&moved, &request) {
                assert!(fetch.on_packet(&packet).expect("the shard's own bytes"));
            }
        }
        assert!(fetch.is_complete());

        let Fetched::Chunks(chunks) = fetch.finish().expect("two chunks of this facet") else {
            panic!("a fetch of what moved ends in the chunks themselves");
        };
        assert_eq!(chunks.len(), asked.len(), "one chunk per square that moved");
        for chunk in &chunks {
            assert_eq!(chunk.revision(), revision);
        }

        // The window's half, which is `MapSnapshot::take_chunks` and nothing
        // else — `chunk::apply` with the world's own facet and revision
        // bookkeeping around it.
        let mut held = held;
        let applied = held.take_chunks(&chunks).expect("two chunks of this facet");
        assert_eq!(applied, revision);
        let map = held.map();
        assert_eq!(map.static_count(), moved.map().static_count());
        for y in 0..moved.map().height() as u16 {
            for x in 0..moved.map().width() as u16 {
                assert_eq!(map.land(x, y), moved.map().land(x, y), "the land at ({x}, {y})");
                let there: Vec<StaticItem> = map.statics_at(x, y).copied().collect();
                let here: Vec<StaticItem> = moved.map().statics_at(x, y).copied().collect();
                assert_eq!(there, here, "the statics at ({x}, {y})");
            }
        }
    }

    /// A publish landing under E4's own fetch is caught by the same check E3's
    /// is: the chunk arrives at a revision the notice did not name.
    #[test]
    fn a_chunk_from_another_revision_ends_a_fetch_of_what_moved() {
        let asked_about = a_facet().revision().after();
        let later = a_facet_that_moved(asked_about.after());

        let mut fetch = Fetch::moved(notice(), vec![ChunkAt { x: 0, y: 0 }], asked_about)
            .expect("a facet the wire can name");
        let request = fetch.next_request().expect("one chunk to ask for");
        let sent = answers(&later, &request);
        let last = sent.len() - 1;
        for packet in &sent[..last] {
            fetch.on_packet(packet).expect("a fragment of a chunk asked for");
        }
        assert!(matches!(
            fetch.on_packet(&sent[last]),
            Err(FetchError::WrongRevision { wanted, found, .. })
                if wanted == asked_about && found == later.revision()
        ));
    }

    /// A publish that lands between the answer and the fetch is caught by the
    /// revision every chunk carries.
    ///
    /// It has to be: the list of chunks was the difference between two
    /// particular revisions, so a chunk from a third one means other chunks
    /// moved as well and nothing here was told which. The whole-facet fetch has
    /// no equivalent check because `assemble` compares the set with itself.
    #[test]
    fn a_chunk_from_another_revision_ends_a_fetch_over_a_world() {
        let held = a_facet();
        let asked_about = a_facet().revision().after();
        // And then the world moved again, under the answer.
        let later = a_facet_that_moved(asked_about.after());

        let mut fetch = Fetch::over(notice(), held, vec![ChunkAt { x: 0, y: 0 }], asked_about)
            .expect("a world the size the notice describes");
        let request = fetch.next_request().expect("one chunk to ask for");
        let sent = answers(&later, &request);
        let last = sent.len() - 1;
        for packet in &sent[..last] {
            fetch.on_packet(packet).expect("a fragment of a chunk asked for");
        }
        assert!(matches!(
            fetch.on_packet(&sent[last]),
            Err(FetchError::WrongRevision { at, wanted, found })
                if at == ChunkAt { x: 0, y: 0 } && wanted == asked_about && found == later.revision()
        ));
    }

    /// A publish under a running fetch: what the fetch was still owed is eaten
    /// rather than decoded, and the drain is empty when the last of it lands.
    ///
    /// The answers deliberately come from the world *after* the edit — which is
    /// what makes them unusable, and what a drain has no opinion about.
    #[test]
    fn an_abandoned_fetch_eats_the_answers_it_was_still_owed() {
        let snapshot = a_facet();
        let published_at = a_facet().revision().after();
        let mut fetch = Fetch::of(notice()).expect("a facet the wire can name");
        let request = fetch.next_request().expect("a facet has chunks");
        assert_eq!(request.chunks.len(), 4, "the fixture is four chunks");
        // One of the four is whole before the publish lands.
        let first = ChunkRequest {
            facet:  FACET,
            chunks: vec![request.chunks[0]],
        };
        for packet in answers(&snapshot, &first) {
            assert!(fetch.on_packet(&packet).expect("the shard's own bytes"));
        }

        let (mut drain, _restart) =
            fetch.abandon(&Changes::These(vec![ChunkAt { x: 1, y: 1 }]), published_at);
        assert_eq!(drain.owed(), 3, "the answers still on the wire");
        assert!(!drain.is_empty());

        let moved = a_facet_that_moved(published_at);
        let rest = ChunkRequest {
            facet:  FACET,
            chunks: request.chunks[1..].to_vec(),
        };
        for packet in answers(&moved, &rest) {
            assert!(drain.on_packet(&packet), "an answer the abandoned fetch was owed");
        }
        assert!(drain.is_empty(), "the wire is clean and the restart can go out");
        assert!(
            !drain.on_packet(&ServerPacket::LoginComplete(
                openshard_protocol::world::LoginComplete
            )),
            "everything but a chunk packet is still the caller's"
        );
    }

    /// A refusal is an answer, and it has to come out of the drain: a chunk
    /// still owed after one would hold the connection shut for a packet that is
    /// never coming. [`Fetch::on_packet`] has no equivalent — there a refusal
    /// ends the fetch, so what it leaves outstanding never matters.
    #[test]
    fn a_refusal_takes_a_chunk_out_of_a_drain() {
        let mut fetch = Fetch::of(notice()).expect("a facet the wire can name");
        let request = fetch.next_request().expect("a facet has chunks");
        let (mut drain, _restart) = fetch.abandon(&Changes::Everything, a_facet().revision().after());
        assert_eq!(drain.owed(), request.chunks.len());
        for &at in &request.chunks {
            assert!(drain.on_packet(&ServerPacket::ChunkRefused(ChunkRefused {
                facet: FACET,
                at,
                reason: Refusal::NoWorld,
            })));
        }
        assert!(
            drain.is_empty(),
            "a drain that ignored a refusal would never empty"
        );
    }

    /// The restart asks for both lists: what the shard said had moved, and what
    /// the publish that interrupted it said moved as well.
    ///
    /// A square in the first list and not the second has still moved as far as
    /// this end can tell, and nothing will ever name it again — so the answer is
    /// the union, fetched at the revision the publish named. The oracle is the
    /// world the shard is holding after both.
    #[test]
    fn a_restart_asks_for_what_the_answer_and_the_publish_both_named() {
        let held = a_facet();
        let asked_about = a_facet().revision().after();
        let published_at = asked_about.after();
        let moved = a_facet_that_moved(published_at);
        let notice = WorldNotice {
            revision: WorldRevision(published_at.get()),
            ..notice()
        };

        // Told one chunk moved, and a publish naming another lands before a
        // single answer does.
        let fetch = Fetch::over(notice, held, vec![ChunkAt { x: 0, y: 0 }], asked_about)
            .expect("a world the size the notice describes");
        let (drain, restart) = fetch.abandon(&Changes::These(vec![ChunkAt { x: 1, y: 1 }]), published_at);
        assert!(drain.is_empty(), "nothing had been asked for yet");

        let mut fetch = restart
            .begin(notice)
            .expect("a world the size the notice describes");
        assert!(fetch.is_over_a_world());
        assert_eq!(
            fetch.wanted(),
            2,
            "the square the answer named and the one the publish did"
        );
        while let Some(request) = fetch.next_request() {
            for packet in answers(&moved, &request) {
                assert!(fetch.on_packet(&packet).expect("the shard's own bytes"));
            }
        }
        let arrived = fetch.finish().expect("two chunks of this facet").world();

        assert_eq!(arrived.revision(), published_at);
        assert_eq!(arrived.map().static_count(), moved.map().static_count());
        for y in 0..moved.map().height() as u16 {
            for x in 0..moved.map().width() as u16 {
                assert_eq!(
                    arrived.map().land(x, y),
                    moved.map().land(x, y),
                    "the land at ({x}, {y})"
                );
                let there: Vec<StaticItem> = arrived.map().statics_at(x, y).copied().collect();
                let here: Vec<StaticItem> = moved.map().statics_at(x, y).copied().collect();
                assert_eq!(there, here, "the statics at ({x}, {y})");
            }
        }
    }

    /// A publish that cannot name what moved takes the facet again, and the
    /// world that was being filled in goes with it: `assemble` builds one out of
    /// the chunks alone.
    #[test]
    fn a_publish_that_cannot_name_what_moved_takes_the_facet_again() {
        let asked_about = a_facet().revision().after();
        let fetch = Fetch::over(notice(), a_facet(), vec![ChunkAt { x: 0, y: 0 }], asked_about)
            .expect("a world the size the notice describes");
        let (_drain, restart) = fetch.abandon(&Changes::Everything, asked_about.after());
        let fetch = restart.begin(notice()).expect("a facet the wire can name");
        assert!(
            !fetch.is_over_a_world(),
            "the facet, and not a world being filled in"
        );
        assert_eq!(fetch.wanted(), 4, "every chunk of the fixture");
    }

    /// A whole-facet fetch is abandoned as the whole facet: nothing of it is on
    /// this side, so there is nothing narrower to ask for however few squares
    /// the publish names.
    #[test]
    fn an_abandoned_facet_is_taken_whole_and_not_by_the_square() {
        let fetch = Fetch::of(notice()).expect("a facet the wire can name");
        let (_drain, restart) = fetch.abandon(
            &Changes::These(vec![ChunkAt { x: 0, y: 0 }]),
            a_facet().revision().after(),
        );
        let fetch = restart.begin(notice()).expect("a facet the wire can name");
        assert!(!fetch.is_over_a_world());
        assert_eq!(fetch.wanted(), 4, "every chunk of the fixture");
    }

    /// A publish under E4's own fetch: the restart is the same kind of fetch,
    /// so it still ends in the chunks themselves rather than in a world this
    /// end does not hold.
    #[test]
    fn a_restart_for_the_windows_world_still_ends_in_chunks() {
        let published_at = a_facet().revision().after();
        let again = published_at.after();
        let moved = a_facet_that_moved(again);
        let notice = WorldNotice {
            revision: WorldRevision(again.get()),
            ..notice()
        };

        let fetch = Fetch::moved(notice, vec![ChunkAt { x: 0, y: 0 }], published_at)
            .expect("a facet the wire can name");
        let (drain, restart) = fetch.abandon(&Changes::These(vec![ChunkAt { x: 1, y: 1 }]), again);
        assert!(drain.is_empty());

        let mut fetch = restart.begin(notice).expect("a facet the wire can name");
        assert_eq!(fetch.wanted(), 2);
        while let Some(request) = fetch.next_request() {
            for packet in answers(&moved, &request) {
                assert!(fetch.on_packet(&packet).expect("the shard's own bytes"));
            }
        }
        let Fetched::Chunks(chunks) = fetch.finish().expect("two chunks of this facet") else {
            panic!("a fetch for the window's world ends in the chunks themselves");
        };
        assert_eq!(chunks.len(), 2);
        for chunk in &chunks {
            assert_eq!(chunk.revision(), again);
        }
    }

    /// A second edit while the first one's answers are still draining: the list
    /// grows, each square is named once however often a publish repeats it, and
    /// what is fetched is the newest revision.
    #[test]
    fn a_second_publish_grows_the_list_and_names_no_square_twice() {
        let asked_about = a_facet().revision().after();
        let published_at = asked_about.after();
        let again = published_at.after();
        let moved = a_facet_that_moved(again);
        let notice = WorldNotice {
            revision: WorldRevision(again.get()),
            ..notice()
        };

        let fetch = Fetch::over(notice, a_facet(), vec![ChunkAt { x: 0, y: 0 }], asked_about)
            .expect("a world the size the notice describes");
        let (_drain, mut restart) =
            fetch.abandon(&Changes::These(vec![ChunkAt { x: 1, y: 1 }]), published_at);
        // The same square again, and one nobody has named yet.
        restart.and(
            &Changes::These(vec![ChunkAt { x: 1, y: 1 }, ChunkAt { x: 1, y: 0 }]),
            again,
        );

        let mut fetch = restart
            .begin(notice)
            .expect("a world the size the notice describes");
        assert_eq!(fetch.wanted(), 3, "three squares, each named once");
        // Every chunk arrives at the newest revision, which is the check that
        // says `and` moved it: at the older one this is `WrongRevision`.
        while let Some(request) = fetch.next_request() {
            for packet in answers(&moved, &request) {
                assert!(fetch.on_packet(&packet).expect("the shard's own bytes"));
            }
        }
        let arrived = fetch.finish().expect("three chunks of this facet").world();
        assert_eq!(arrived.revision(), again);
    }

    /// A world of another size is refused before a chunk is asked for.
    #[test]
    fn a_world_that_is_not_the_one_described_cannot_be_filled_in() {
        let held = a_facet();
        let elsewhere = notice_of(FacetBlocks {
            wide: BLOCKS + 8,
            down: BLOCKS,
        });
        assert!(matches!(
            Fetch::over(
                elsewhere,
                held,
                vec![ChunkAt { x: 0, y: 0 }],
                MapRevision::INITIAL
            ),
            Err(FetchError::WrongWorld { .. })
        ));
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
            .map(|at| {
                ChunkAt {
                    x: at.x as u16,
                    y: at.y as u16,
                }
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
        let mut fetch = Fetch::of(notice_of(FacetBlocks {
            wide: 17 * openshard_map::chunk::BLOCKS_PER_CHUNK,
            down: 16 * openshard_map::chunk::BLOCKS_PER_CHUNK,
        }))
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
                |_, _| {
                    LandCell {
                        tile: LandTileId(3),
                        z:    0,
                    }
                },
            ),
        );
        let mut fetch = Fetch::of(notice_of(blocks)).expect("a facet the wire can name");

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
        let notice = notice_of(FacetBlocks {
            wide: 8 * 70_000,
            down: 8,
        });
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
