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

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_map::chunk::{Chunk, ChunkCoord};
use openshard_map::codec;
use openshard_protocol::chunks::{
    ChunkAt, ChunkData, ChunkRefused, FacetBlocks, Refusal, WorldNotice, WorldRevision,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::world::Facet;
use tracing::debug;

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
        let snapshot = self.state.facet_state(facet).ground().snapshot()?;
        let extent = snapshot.map().extent();
        Some(WorldNotice {
            facet,
            blocks: FacetBlocks {
                wide: extent.wide,
                down: extent.down,
            },
            revision: WorldRevision(snapshot.revision().get()),
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
            .filter_map(|packet| match packet {
                ServerPacket::ChunkData(data) => Some(data.blob.len()),
                _ => None,
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
        // `facets.get` and not `facet_state`: the number came off the wire, and
        // the accessor that indexes is documented for facets an entity carries,
        // which are loaded by construction. A client's byte is not.
        let ground = self
            .state
            .facets
            .get(&facet)
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
