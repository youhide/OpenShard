//! The ground, handed to a client that asked for it.
//!
//! Direction E's shard side, and it is a *reader*: the world already holds the
//! facet, [`Chunk::of`] already cuts a square out of a published snapshot,
//! [`codec::encode`] already turns one into its canonical bytes, and
//! [`ChunkData::fragments`] already deflates and cuts up a blob. What is here is
//! the join, and one rule.
//!
//! # The rule: every chunk named is answered exactly once
//!
//! With its bytes, or with a [`ChunkRefused`]. Silence is what
//! [`design_details_request`](super::World::design_details_request) does for a
//! house that is not there, and it is right *there* because a client that never
//! learns about a house simply draws no house. It is wrong here: nothing else in
//! this conversation is self-terminating, so a client waiting on one chunk that
//! is never coming is a client that never finishes fetching a facet.
//!
//! # Nothing is cached
//!
//! The cut and the deflate are cheap against the socket write — a median chunk
//! is 1,739 bytes out of 12,568 — and a cache keyed by a world that moves is
//! direction D's problem rather than this one's. A publish would have to
//! invalidate it, and getting that wrong is a client drawing ground the shard
//! stopped believing in.
//!
//! # What moved, and where that is known
//!
//! [`changes_since`](World::changes_since) is E3's half: a client that kept the
//! ground it was given comes back holding a revision, and what it needs is the
//! difference rather than the facet. That difference is **the patch log's** and
//! not the world's — a facet in memory has no memory of which tiles moved to get
//! it here, and the only other way to compute it is to hold both worlds, which
//! is precisely what the client has and the shard does not.
//!
//! So the log is read on the way past. It is one record per committed edit, it
//! is asked once per connection, and the alternative — an index kept in memory
//! and invalidated on publish — is the cache the paragraph above refuses.

use openshard_basemap::patches;
use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_map::chunk::{
    Chunk,
    ChunkCoord,
};
use openshard_map::codec;
use openshard_protocol::chunks::{
    Changes,
    ChangesReply,
    ChunkAt,
    ChunkData,
    ChunkRefused,
    FacetBlocks,
    MAX_MOVED,
    Refusal,
    WorldNotice,
    WorldRevision,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::world::Facet;
use tracing::{
    debug,
    warn,
};

use super::World;

impl World {
    /// What to tell a character entering the world about the ground under it, or
    /// `None` for a facet with no ground at all.
    ///
    /// `None` rather than a notice of nought blocks by nought: a client is meant
    /// to read this and start asking for chunks, and a facet with no map has
    /// none to give. A shard running with no client files is the ordinary case
    /// for that — it says so at boot, and every step on it is allowed.
    ///
    /// The extent is in blocks and comes off the map itself rather than off
    /// [`FacetState::width`](openshard_state::FacetState::width), which is tiles
    /// and is what the `0x1B` tells a stock client. The two describe the same
    /// facet and only one of them is what
    /// [`assemble`](openshard_map::chunk::assemble) refuses a short set of
    /// chunks against.
    pub(super) fn world_notice(&self, entity: EntityId) -> Option<WorldNotice> {
        let facet = self.state.facet_of(entity);
        let state = self.state.facet_state(facet);
        let snapshot = state.ground().snapshot()?;
        let extent = snapshot.map().extent();
        Some(WorldNotice {
            facet,
            blocks: FacetBlocks {
                wide: extent.wide,
                down: extent.down,
            },
            revision: WorldRevision(snapshot.revision().get()),
            // `None` for a facet read out of the install, and it is the same
            // `None` that stops one being edited: a world we do not own has no
            // name we could promise means the same thing tomorrow, so a client
            // must not keep a copy of it. See `WorldHome`.
            world: state.home().map(|home| home.identity),
        })
    }

    /// Answer a client's `0xBF 0xE002` with the ground it asked for.
    pub(super) fn chunk_request(&mut self, connection: ConnectionId, facet: Facet, wanted: &[ChunkAt]) {
        // Built before anything is sent, because cutting a chunk borrows the
        // world and sending one writes to it. That split is also what makes the
        // answer testable without a connection: `chunk_answers` is a pure read.
        let answers = self.chunk_answers(facet, wanted);
        let bytes: usize = answers
            .iter()
            .filter_map(|packet| {
                match packet {
                    ServerPacket::ChunkData(data) => Some(data.blob.len()),
                    _ => None,
                }
            })
            .sum();
        debug!(
            %connection,
            facet = facet.0,
            asked = wanted.len(),
            packets = answers.len(),
            bytes,
            "0xBF 0xE002 chunk request"
        );
        for packet in answers {
            self.state.send_packet(connection, &packet);
        }
    }

    /// What the shard has to say about each chunk asked for, in the order it was
    /// asked.
    ///
    /// One or more [`ServerPacket::ChunkData`] per chunk, or exactly one
    /// [`ServerPacket::ChunkRefused`]. Never nothing — see the module header.
    pub(crate) fn chunk_answers(&self, facet: Facet, wanted: &[ChunkAt]) -> Vec<ServerPacket> {
        // `facet_state_if_loaded` and not `facet_state`: the number came off the
        // wire, and the accessor that panics is documented for facets an entity
        // carries, which are loaded by construction. A client's byte is not.
        let ground = self
            .state
            .facet_state_if_loaded(facet)
            .and_then(|state| state.ground().snapshot());
        let Some(snapshot) = ground else {
            return wanted
                .iter()
                .map(|&at| refuse(facet, at, Refusal::NoWorld))
                .collect();
        };
        // The snapshot's own facet, not the one asked for, would be the same
        // number — the world files a snapshot under the facet it names — but the
        // packet echoes what the client said so a reply can be matched to a
        // request without trusting the shard's bookkeeping.
        let revision = WorldRevision(snapshot.revision().get());

        let mut answers = Vec::with_capacity(wanted.len());
        for &at in wanted {
            let coord = ChunkCoord {
                x: u32::from(at.x),
                y: u32::from(at.y),
            };
            let Some(chunk) = Chunk::of(snapshot, coord) else {
                answers.push(refuse(facet, at, Refusal::PastTheEdge));
                continue;
            };
            answers.extend(
                ChunkData::fragments(facet, at, revision, &codec::encode(&chunk))
                    .into_iter()
                    .map(ServerPacket::ChunkData),
            );
        }
        answers
    }
}

/// One refusal, as the packet it goes out as.
fn refuse(facet: Facet, at: ChunkAt, reason: Refusal) -> ServerPacket {
    ServerPacket::ChunkRefused(ChunkRefused { facet, at, reason })
}

impl World {
    /// Answer a client's `0xBF 0xE007` with what has moved since it last looked.
    pub(super) fn changes_request(&mut self, connection: ConnectionId, facet: Facet, held: WorldRevision) {
        let reply = self.changes_since(facet, held);
        debug!(
            %connection,
            facet = facet.0,
            held = held.0,
            now = reply.revision.0,
            answer = match &reply.changes {
                Changes::These(chunks) => chunks.len() as i64,
                Changes::Everything => -1,
            },
            "0xBF 0xE007 changes request"
        );
        self.state
            .send_packet(connection, &ServerPacket::ChangesReply(reply));
    }

    /// Which chunks a client holding `held` is missing, or that it cannot be
    /// told.
    ///
    /// **The log is the source, not the world.** What moved between two
    /// revisions is exactly the union of the ops committed between them, and the
    /// facet in memory has no memory of which tiles those were — comparing two
    /// worlds would need the old one, which is the client's and not the shard's.
    ///
    /// A pure read, and separate from the send for
    /// [`chunk_answers`](Self::chunk_answers)'s reason: the answer is then
    /// testable without a connection.
    pub(crate) fn changes_since(&self, facet: Facet, held: WorldRevision) -> ChangesReply {
        let ground = self
            .state
            .facet_state_if_loaded(facet)
            .and_then(|state| state.ground().snapshot());
        // No ground at all: there is nothing to be at a revision of, and the
        // client's next move — asking for the facet — is refused per chunk with
        // `NoWorld`, which is where that fact is properly said.
        let Some(snapshot) = ground else {
            return everything(facet, WorldRevision(0));
        };
        let now = WorldRevision(snapshot.revision().get());
        // Before the revisions are compared at all, because on a facet read out
        // of the install a revision carries no information: such a world is
        // always at the first one, however many times the operator has replaced
        // the files under it. There is no log beside it to read, nothing here
        // knows what moved, and a client should not have kept a copy of it in
        // the first place — a facet with no home is sent with no identity.
        let Some(home) = self.state.facet_state(facet).home() else {
            return everything(facet, now);
        };
        if held == now {
            // Knowledge, and not `Everything`: a client that has this world
            // already asks for nothing at all, which is E3's whole point.
            return ChangesReply {
                facet,
                revision: now,
                changes: Changes::These(Vec::new()),
            };
        }
        if held > now {
            // A client holding a revision this shard has never published. Either
            // the world was rebuilt behind it or it is a copy of somebody else's
            // — and both are worlds this one cannot describe a difference to.
            debug!(
                facet = facet.0,
                held = held.0,
                now = now.0,
                "a client is ahead of us"
            );
            return everything(facet, now);
        }
        // Before the base set's own revision is before this world existed: a
        // patch log starts at `base`, so there is no record of how the client
        // got to where it says it is.
        if held.0 < home.base.get() {
            return everything(facet, now);
        }
        let log = patches::log_path(&home.base_set);
        let committed = match patches::read(&log, facet, home.base) {
            Ok(committed) => committed,
            Err(error) => {
                // The shard is still serving the world it loaded; what it has
                // lost is the ability to say what changed in it. `Everything` is
                // the honest answer and the log line is where the reason lives.
                warn!(facet = facet.0, %error, "cannot say what moved: the patch log");
                return everything(facet, now);
            }
        };
        // The world in memory is the base set plus every record of that log, so
        // a log that is a different length than the revision implies is a log
        // that has been written by somebody else since boot. Answering out of it
        // would name the wrong chunks.
        if home.base.get() + committed.len() as u64 != now.0 {
            warn!(
                facet = facet.0,
                records = committed.len(),
                base = home.base.get(),
                now = now.0,
                "the patch log no longer matches the world in memory"
            );
            return everything(facet, now);
        }

        let mut moved: Vec<ChunkAt> = committed
            .iter()
            .filter(|patch| patch.revision().get() > held.0)
            .flat_map(|patch| patch.touched_chunks())
            .map(|at| {
                ChunkAt {
                    x: u16::try_from(at.x).expect("a facet of fewer than 65,536 chunks across"),
                    y: u16::try_from(at.y).expect("a facet of fewer than 65,536 chunks down"),
                }
            })
            .collect();
        moved.sort_unstable();
        moved.dedup();
        // Past the cap the list stops fitting in a packet, and past that point
        // it has also stopped being a saving — see `MAX_MOVED`.
        if moved.len() > usize::from(MAX_MOVED) {
            return everything(facet, now);
        }
        ChangesReply {
            facet,
            revision: now,
            changes: Changes::These(moved),
        }
    }
}

/// "Take the facet again", as the packet it goes out as.
const fn everything(facet: Facet, revision: WorldRevision) -> ChangesReply {
    ChangesReply {
        facet,
        revision,
        changes: Changes::Everything,
    }
}
