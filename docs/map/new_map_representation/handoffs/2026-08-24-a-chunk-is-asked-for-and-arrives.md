# 2026-08-24 — a chunk is asked for, and arrives

Direction **E**'s second phase. The session before this one made the client's
world a parameter and chose the pipe; this one puts ground *through* it — a
client of ours asks the shard for a square of the world over the game
connection, and the bytes that come back are the bytes the shard would write to
a base set.

Nothing draws it yet. That is E2, and it is deliberately not in this commit: the
two failures — a wire that is wrong, and a client that cannot exist before its
map arrives — must not be in the same session.

## Where it stands

```sh
OPENSHARD_CLIENT="/path/to/Ultima Online Classic" \
    cargo test -p openshard-e2e-shard --test chunks -- --ignored --nocapture
```

Green, in 2.1 s: a real socket, the real login conversation, sixteen chunks
asked for and reassembled, every record compared **byte for byte** against what
`Chunk::of` cuts out of the base set on disk, the whole facet put back through
`chunk::assemble`, and a chunk past the edge refused by name.

### The wire

| | |
|---|---|
| `0xE002 ChunkRequest` | facet, then up to 64 `{x, y}`. Client → server, and **no reference client sends one**, which is the whole of the capability negotiation |
| `0xE003 ChunkData` | facet, position, revision, `fragment`/`fragments`, inflated length, blob. One chunk's canonical record, deflated whole and cut into pieces of at most 8,192 bytes |
| `0xE004 WorldNotice` | facet, blocks wide, blocks down, revision. Sent once on world entry, where `AuthorityNotice` is sent, and the one thing here a client gets without asking |
| `0xE006 ChunkRefused` | facet, position, reason. `NoWorld` or `PastTheEdge` |

`0xE005` is skipped on purpose: it is E4's publish notice, and an id chosen by
which was written first is an id that has to be renumbered later.

### What is new

| | |
|---|---|
| [`protocol/src/chunks.rs`](../../../../crates/common/protocol/src/chunks.rs) | The four packets, `ChunkData::fragments` and `chunks::join` — **one pair, in one crate**, so the two halves of the wire are a round-trip test rather than two functions that agree by inspection |
| `chunks::ChunkAt` · `FacetBlocks` · `WorldRevision` · `InflatedLength` | The wire's own types. `openshard_map`'s `ChunkCoord` and `MapRevision` are the same facts one crate *up* — that crate already imports `Facet` from this one — so they are converted at the seam |
| `chunks::Fragment` | `index` and `count` with a checked constructor and no public fields: "fragment four of two" is a thing a sender can write and now not a thing a reader can hold |
| `PacketReader::u64` · `PacketWriter::u64` | No reference packet has ever carried a field this wide. A revision is one |
| `ServerPacket::{ChunkData, WorldNotice, ChunkRefused}` · `ExtendedRequest::Chunks` | Both directions decode through the one dispatch each already has |
| [`world/src/tick/chunks.rs`](../../../../crates/server/world/src/tick/chunks.rs) | `chunk_request` and `world_notice`. `chunk_answers` is a pure read over the world, which is what makes it testable without a connection |
| `Command::RequestChunks` | Queued like every other packet, so the answer comes out of a tick |
| `WorldView::world` | `authority`'s twin: what the shard said about the world, recorded and nothing more. E2 starts from this field |
| [`world/src/tick/chunks_tests.rs`](../../../../crates/server/world/src/tick/chunks_tests.rs) | Seven, over a fixture facet **nine blocks square** — not a whole number of chunks either way, so three of its four chunks are edge chunks |
| [`e2e/shard/tests/chunks.rs`](../../../../crates/e2e/shard/tests/chunks.rs) | The socket seam, `#[ignore]`d for `map_edit`'s reason: it reads the install's tiledata and bakes a graph |

## What was decided

**Every chunk named is answered exactly once — with its bytes or with a
refusal.** The plan did not foresee this and it is the phase's real decision.
Silence is what `design_details_request` does for a house that is not there, and
it is right *there* because a client that never hears about a house draws no
house. It is wrong here: nothing in this conversation is self-terminating —
no total, no end marker, no timeout that would not also fire on a slow link — so
a client waiting on one chunk that is never coming is a client that never
finishes fetching a facet.

The refusal is a diagnostic in practice: a client fetching `chunks_of` a facet it
was told the size of cannot produce one. That is exactly why it must be *visible*
when it happens rather than looking like a lost packet.

**Two reasons, because they are two facts.** `NoWorld` — this shard holds no
ground for that facet, which is the ordinary state of a shard with no client
files — and `PastTheEdge`. An unknown reason byte is an error rather than a
quiet demotion to the nearest known one, `AccessLevel::from_wire`'s argument.

**One request names at most 64 chunks**, and the decoder refuses a larger count
rather than truncating it. It bounds one *answer* — a megabyte at Felucca's worst
chunk, 111 KiB at its median — not how fast a facet may be fetched, which is the
client's to pace. A count no encoder of ours can write did not come from a client
of ours, and answering half of what was asked for would be the shard inventing a
request.

**The deflate and the fragmenting live in the protocol crate.** They could have
lived on the shard, with the client growing the inverse. They are one pair
instead, and the round trip over them is one test — which is what caught that a
fixture of *pattern* bytes never fragments at all: a real chunk deflates to a
fifth of itself, so a compressible fixture comes back as one packet however long
it is, and the reassembly path the 8,192-byte cap exists to exercise would have
gone untested while the suite stayed green. The fixture is an LCG now, and says
so.

**A facet with no ground sends no `WorldNotice`.** Not a notice of nought blocks
by nought, which would be a world a client could ask for chunks of, described as
though it could. On the client's side that makes `WorldView::world` an
`Option` with one meaning for three causes — no ground, an older shard, somebody
else's shard — and all three mean *there is no world here to ask for*.

**Nothing is cached on the shard.** The cut and the deflate are cheap against the
socket write, and a cache keyed by a world that moves would have to be
invalidated by a publish — which, got wrong, is a client drawing ground the shard
stopped believing in. Direction D's problem.

## What is clean

`cargo check --workspace --all-targets`: silent. `cargo clippy --all-targets` on
protocol, world, server, client-net and e2e-shard: silent. `cargo test` on
protocol (445 + 12), world (644, seven of them new), server, client-net and the
whole non-ignored e2e suite: green. `rustfmt` on every file this session
touched; `cargo fmt --all` was **not** run, because the tree carries several
parallel sessions' work in progress and it would reformat theirs.

**A parallel session committed part of this work mid-session** — `154b88b9`
("upd") swept the tree, protocol half of E1 included. That is expected here and
nothing was undone; what is committed under this handoff's own message is the
rest.

## What is next

**E2 — the client's world comes off the wire**, and it is where the real cost is.
`run` loads the facet before the window exists, and everything after it — `Ground`
and its span bake, the coarse graph, the interiors flood, the camera's opening
`z` — is built from a map that is already there. A world that arrives after login
cannot keep that order. `Resources::map`'s
`expect("a client that got as far as drawing opened a facet")` is the assertion
that has to become a real state.

What E1 leaves for it, ready: `WorldView::world` says how big the facet is and
which revision it is at, `chunks_of` that extent is the list to ask for,
`MAX_CHUNKS` says how to cut the asking into requests, and `join` →
`codec::decode` → `chunk::assemble` is the whole of the client's side. What E1
did *not* build is the bookkeeping over many chunks in flight — `join` takes the
fragments of one chunk and the caller collects them — which is E2's, because only
E2 knows what it wants to do with a progress line.

## Found along the way

**`ServerPacket`'s `one_of_each` does not hold one of each, and its own doc says
it does.** *"So a new variant that lies about its id or length has to be added
here to compile"* — nothing checks it, and **ten of the sixty-two variants are
missing**: `MultiTarget`, `DeathAnimation`, `OpenContainer`, `AddToContainer`,
`DesignRevision`, `PropertyListReply` and all four party packets. Each is
therefore outside `every_packet_frames_to_its_own_length`, the oracle for
`server_packet_length` — the table whose being wrong is a *dropped connection*,
not a dropped packet, and which has already been caught short an id twice
(`0xD6`, `0xD8`). The four this session added are in the list; the ten are filed
in the plan's backlog. A `match` in a helper returning one sample per variant
would make the doc's claim true, where a `vec!` cannot.

**`WorldState::facet_state` panics on a facet nothing loaded, and there is no
accessor that does not.** Right for every caller that got the number off an
entity, wrong for the one that got it off the wire — so `chunk_answers` indexes
`state.facets` directly and says why. Filed.

**The plan said `WorldNotice` was "six bytes of body".** It is seventeen, in a
twenty-two byte packet. Corrected in place; the point it was making — that a
stock client drops it out of hand — is unaffected.
