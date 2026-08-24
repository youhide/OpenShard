# To the client: the world the shard says it is

> **Direction [E](plan.md#e--to-the-client)'s executable plan.** `plan.md` states
> the direction in five lines and leaves one decision open by name — *"the pipe
> is chosen here and not before"*. It is chosen below, off measurements rather
> than by preference, and the rest of E is cut into phases that each end with a
> client that runs.
>
> Where the work stands is [`handoffs/`](handoffs/), newest last. This document
> holds the intent.

## The one sentence

Our client draws the facet it opened on the player's own disk; the shard edits
its own ground between two ticks; nothing joins those two facts. An operator who
types `.setland 3 40` sees nothing happen and then cannot walk there.

E is the join, and it is only ours. The classic 2D client reads its own files and
is out of this by design — `map_rebuild.md` says so, and nothing here changes it.

## What is already true, and is easy to miss

Three things this direction was expected to invent already exist, and finding
them is most of why the plan below is short.

**The pipe exists.** [`access.rs`](../../../crates/common/protocol/src/access.rs#L75)
defines `OPENSHARD_SUBCOMMANDS = 0xE000` — *"the first `0xBF` subcommand this
engine invented, and where every other one it invents will live"* — with the
argument for why it is safe already written: every subcommand a shipped client
speaks is at or below `0x2B`, ClassicUO's own private one is `0xBEEF`, and a
stock client reads `0xBF`'s length out of the envelope and skips a subcommand it
does not know rather than losing the stream. `AuthorityNotice` is `0xE001` and is
the working precedent.

**Deflating a wire payload is precedent, not invention.**
[`design.rs`](../../../crates/common/protocol/src/design.rs#L544) already
deflates a house's planes into a packet, carries the inflated length beside the
blob, and inflates *with that length as a limit* on the way in. `miniz_oxide` is
a workspace dependency and `openshard-protocol` already uses it, so the chunk
packet costs no new crate.

**The reader exists.** [`chunk::assemble`](../../../crates/common/map/src/chunk.rs)
turns a set of chunks into a `WorldMap`, and `openshard_basemap::read` is a
caller of it. A client that receives chunks assembles them through the same call
the base set reader uses — which is what makes "the client's world" and "the
shard's world" one code path rather than two that agree.

## What Felucca measures

Off the shipped base set (`felucca.osbase`, facet 0, revision 1, 896×512 blocks,
7,168 chunks, 107,528,650 bytes), and every number below is what decides
something further down.

| chunk record | bytes |
|---|---|
| floor (land + counts + header, no statics) | **12,568** |
| median | 12,568 |
| mean | 15,001 |
| p90 · p99 · p999 | 20,500 · 30,286 · 40,012 |
| max | **45,382** |

`MAX_PACKET_SIZE` is [18,000](../../../crates/common/protocol/src/packet.rs#L79),
so **21.3% of Felucca's chunks do not fit in a packet** as they stand. That is
the fact that used to argue for a second stream.

Deflated at level 6, the same 7,168 chunks:

| deflated | bytes |
|---|---|
| whole facet | **22,363,473** — 21.3 MiB, **0.208 of raw** |
| min (an ocean chunk, 12,568 raw) | **56** |
| median | 1,739 |
| mean | 3,119 |
| p90 · p99 · p999 | 6,926 · 10,198 · 13,957 |
| max | **16,050** |
| over 18,000 | **0 chunks** |

And through UO's own Huffman table, which every byte on the game connection pays
(sampled, 194 chunks):

| | of raw |
|---|---|
| raw, Huffman-coded | 0.808 |
| deflated, Huffman-coded | **0.241** |

So Huffman recovers 19% on chunk bytes and *inflates* an already-deflated blob by
15% — which still leaves deflate-then-Huffman at a quarter of the cost of
sending the record raw. The whole facet is **24.6 MiB on the wire** rather than
82.8 MiB, and the empty ocean that is most of a facet costs 56 bytes a chunk
instead of 12,568.

*Reproduce:* the scripts are throwaway — read the base set's own offset table for
the raw sizes, `zlib.compress(blob, 6)` for the deflated ones, and the code
lengths in [`huffman.rs`](../../../crates/common/protocol/src/huffman.rs#L38)
(`CODES[b] & 0xF` is a symbol's width in bits) for the third column. Nothing here
needs a running shard.

## Decisions, taken here

**The pipe is the game connection, in `0xBF`, in the `0xE000` range.** Not a
second stream over [`Dial`](../../../crates/client/net/src/transport.rs#L100).
The argument that made a second stream attractive was the 18,000-byte cap, and
deflate retires it: **no chunk of Felucca exceeds 16,050 bytes compressed**. What
is left is a second port to open, a third method on `Dial` for every
implementation including the in-process one, a second authentication, and two
streams with no order between them — a client could apply a chunk fetched before
a publish after being told the publish happened. One stream has one order.

And the failure modes are not symmetric: a private packet *id* would desynchronise
a stock client that ever received one, because framing has no length rule for it
and [there is no resynchronising a UO stream](../../../crates/client/net/src/connection.rs#L55).
A `0xBF` subcommand it does not know is skipped. The envelope is chosen for what
happens when we are wrong, not for what happens when we are right.

**A chunk is deflated before it is framed, and the packet carries the inflated
length.** `design.rs`'s shape exactly, for `design.rs`'s reason: a receiver that
inflates without a bound is a receiver a sender can make allocate anything.

**One chunk is one blob, cut into fragments of at most 8,192 bytes.** Not because
a chunk needs it — none of Felucca's does — but because *4.58% of them do* at
that cap, which is 328 chunks a facet. A reassembly path exercised by one chunk
in twenty is a path that works; one exercised only by a hypothetical dense
generated world is a path that is wrong the first time it runs. The cap also
bounds how long a bulk transfer can sit in front of a movement packet on the one
stream this direction just chose.

**Every chunk named in a request is answered exactly once — with its bytes, or
with a refusal.** *Taken in E1, and it is the one thing this document did not
foresee.* Silence is what the house-design request does for a house that is not
there, and it is right there because a client that never hears about a house
simply draws no house. It is wrong here: nothing in this conversation is
self-terminating — no total, no end marker, and no timeout that would not also
fire on a slow link — so a client waiting on one chunk that is never coming is a
client that never finishes fetching a facet. Hence `0xE006 ChunkRefused` below,
with two reasons that are two different facts: the shard has no ground for that
facet, or the facet it has stops short of that chunk.

**One request names at most 64 chunks.** *Also E1's.* A bound on one *answer* —
64 is a megabyte at Felucca's worst chunk and 111 KiB at its median — and not on
how fast a facet may be fetched, which is the client's to pace with as many
requests as it likes. The decoder refuses a larger count rather than truncating
it: a count no encoder of ours can write did not come from a client of ours, and
answering half of what was asked for would be the shard inventing a request.

**Whole chunks, never a stream of operations.** Inherited from
[`map_rebuild.md`](../map_rebuild.md#a-tile-of-ground-moved--rebake-and-never-an-overlay)
and not reopened. It costs less than it looks: a publish touches the chunks
`Patch::touched_chunks` names — usually one — and one chunk is a couple of
kilobytes deflated. Sending patches instead would be smaller still and is *not*
worth what it costs: the client would have to hold a world bit-identical to the
shard's for `PatchOp`'s `was` to match, and the recovery from a mismatch is
refetching the chunk anyway. The chunk is the ground truth; there is no second
mechanism to keep honest.

**The client's own world file is a base set, and the flag that names one is the
cache's read path.** E0's `--base-set` is not a stepping stone that gets thrown
away — it is `openshard_basemap::load`, which is what E3's cache is read back
through. One reader, one format, one revision rule, whether an operator put the
file there or the client wrote it.

**A chunk's `revision` field is the revision it was cut at, and the *world's*
revision is the base set header's.** They are not the same question and the
cache depends on the difference: after a publish, every chunk re-cut from the
facet would carry the new number while only the touched ones changed content. A
cache keyed on the chunk's own field would throw away 7,167 good chunks per
edit.

**The facet stays whole and resident.** A client with no map files fetches all
7,168 chunks before it draws — 21.3 MiB, once, and then a cache. Fetching on
approach is [direction G](plan.md#g--residency-and-size-deferred-on-purpose)'s,
and `WorldMap` is a dense array that cannot answer half a facet today. E must not
be the thing that opens G.

## The phases

Each ends with a client that runs. The order is chosen so that the network
arrives *after* the client can already build a world out of chunks — the two
failures are then never in the same session.

### E0 — the client's world becomes a parameter

**Goal.** The client reads the facet from whichever source it was pointed at, and
the install stops being the only answer.

- `--base-set FILE` / `OPENSHARD_BASE_SET` on `openshard-client-app`, and a
  `WorldSource` parameter on [`run`](../../../crates/client/app/src/lib.rs#L430)
  — an enum with two arms and not an `Option<PathBuf>`, because the install is a
  real alternative rather than an absence.
- The install stays required either way: `tiledata.mul`, the art, the multis and
  the hues are not in a base set and are not going to be. What a base set
  replaces is `map0LegacyMUL.uop`, `staidx0.mul` and `statics0.mul`.
- The two derived artifacts follow the world rather than the install:
  `bake::stamp_of_base_set` already exists for the navigation graph;
  `interiors::stamp_of_base_set` is its twin and does not, and
  `openshard-interiors-bake` grows the same `--base-set` the navigation bake
  already has. Both artifacts land beside the base set through `bake::beside`,
  which is the rule `boot.rs` already follows.
- **The resolution stops being written three times.** `boot.rs`'s
  `facet_source`, `openshard-navigation-bake`'s `source` and the client's new one
  are one question — *read a facet from the source named, check the file agrees
  about which facet it is, and say where things derived from it live*. It moves
  into `openshard_movement::bake`, beside the two stamp functions that are its
  only reason to exist; that crate's package already depends on
  `openshard-basemap` and `openshard-uofiles` for its own binary, and every one
  of the three callers already depends on it.

**Done when** the client starts with `map0LegacyMUL.uop`, `staidx0.mul` and
`statics0.mul` moved out of the install, draws Britain over a base set, and walks
— and a test pins that the two sources produce the same world.

### E1 — a chunk is asked for and arrives

**Goal.** The wire carries one chunk, correctly, with nothing drawing it yet.

Four subcommands in the `0xE000` range, in `openshard-protocol` — three of them
foreseen here and the fourth argued for above:

```text
0xE002  ChunkRequest   client -> server
        facet u8, count u16, then count x { chunk x u16, chunk y u16 }

0xE003  ChunkData      server -> client
        facet u8, chunk x u16, chunk y u16, revision u64,
        fragment u8, fragments u8, inflated u32, blob ..

0xE004  WorldNotice    server -> client, on world entry
        facet u8, blocks wide u32, blocks down u32, revision u64

0xE006  ChunkRefused   server -> client
        facet u8, chunk x u16, chunk y u16, reason u8
```

`0xE005` is skipped rather than taken: it is E4's publish notice below, and an id
chosen by which was written first is an id that has to be renumbered later.

- `WorldNotice` is what a client needs *before* it can ask for anything: the
  facet's extent is what `chunk::assemble` refuses a short set against, and the
  revision is what a cache is compared with. It is sent where `AuthorityNotice`
  is sent, for the same reason — the world entry is when a connection learns what
  it is standing in. A facet with no ground sends none at all: a notice of nought
  blocks by nought would be a world a client could ask for chunks of, described
  as though it could.
- **Only a client that asked is answered.** A stock client never sends `0xE002`,
  so nothing about this reaches one. That is the whole capability negotiation and
  it is deliberately not a feature flag.
- The server's side is a reader over the facet it already holds:
  `Chunk::of(snapshot, at)`, `codec::encode`, deflate, fragment. Nothing is
  cached on the shard — the encode is cheap against the socket write, and a cache
  keyed by a world that moves is direction D's problem, not this one's.
- **The deflate and the fragmenting live in `openshard-protocol`, as one pair.**
  `ChunkData::fragments` cuts a record up and `chunks::join` puts it back, so the
  two halves of the wire are one round-trip test rather than a shard function and
  a client function that agree by inspection. Neither looks inside a blob: the
  chunk record is `openshard_map::codec`'s, and that crate is *above* the
  protocol — which is also why `ChunkAt` and `WorldRevision` are the wire's own
  types, converted at the seam from `ChunkCoord` and `MapRevision`.

**Done when** an `e2e` test logs in over a real socket, asks for a named chunk,
reassembles it, and finds it equal to `Chunk::of` over the shard's own snapshot —
and when a client that asks for a chunk outside the facet is refused rather than
answered with something.

### E2 — the client's world comes off the wire

**Goal.** No map files, no base set: the client is told what the world is.

- `WorldSource` grows its third arm, and it is the one that breaks the startup
  order: today [`run`](../../../crates/client/app/src/lib.rs#L460) loads the
  facet before the window exists, and everything after it — `Ground` and its span
  bake, the coarse graph, the interiors flood, the camera's opening `z` — is
  built from a map that is already there. A world that arrives after login cannot
  keep that order, and **this is E's real cost**: the client has to be able to
  exist with `World::new(None)` and grow ground later. `Resources::map`'s
  `expect("a client that got as far as drawing opened a facet")` is the assertion
  that has to become a real state.
- The fetch is the whole facet, in `chunks_of` order, with a progress line —
  21.3 MiB and about 7,168 replies. Assembly is `chunk::assemble`, unchanged.
- The span bake follows (0.07 s); the coarse graph and the interiors flood are
  artifacts of a world this client has no bake of, so they are absent, exactly as
  they are absent today for an install with no artifact beside it.

**Done when** the client runs against a shard with no `map*` or `statics*` files
present at all and draws the same world an install-fed client draws, sampled over
the facet.

### E3 — the client keeps what it was given

**Goal.** The 21.3 MiB is paid once.

- What the client received is written as a base set of ours, under a path derived
  from the shard's identity and the facet, and read back through
  `openshard_basemap::load` — E0's reader.
- On connect, the client compares its cache's revision against `WorldNotice`'s
  and asks only for what has moved since. What moved is the union of
  `Patch::touched_chunks` over the log's records after that revision, which the
  shard can answer without re-reading the world.
- **Open, and E3's to close:** whether the cache is rewritten whole when the
  world moves, or grows an append-only tail of newer chunk blobs that the loader
  applies over the base. Rewriting 102 MiB for a one-tile edit is the cost that
  decides it, and it should be measured rather than assumed.

**Done when** a second run over an unchanged world asks for no chunks at all, and
a second run over a world that moved by one patch asks for exactly the chunks
that patch touched.

### E4 — a publish reaches a connected client

**Goal.** The operator sees the ground move.

```text
0xE005  PublishNotice  server -> client
        facet u8, revision u64,
        count u16, then count x { chunk x u16, chunk y u16 }
```

- Sent from [`mapedit::commit`](../../../crates/server/world/src/mapedit.rs)
  after the log has accepted the patch — never before, because a commit whose log
  refuses puts the world back, and a client told about a revision that was rolled
  back holds a world that never existed.
- The client refetches the named chunks, rebuilds its `WorldMap`'s share of them,
  re-bakes what is derived over them, and drops its coarse graph — the same
  bargain the shard already takes, for the same reason.
- **The live layer is untouched**, exactly as `World::publish` leaves it: a patch
  is a change to the ground and the door standing in the doorway is the shard's
  to move.

**Done when** an operator types `.setland 3 40` and the tile under them changes
colour on a connected client's screen without a reconnect — which is
[direction C's own "done"](plan.md#c--patches-and-the-resolved-snapshot), and the
last clause of it.

## What this must not do

- **Send anything to a client that did not ask.** `WorldNotice` is the one
  exception, and it is seventeen bytes of body in a twenty-two byte packet — a
  stock client reads its length out of the envelope and drops it.
- **Open direction G.** The facet stays whole and resident; a chunk fetched on
  approach is a different plan with a different risk.
- **Grow a second assembly path.** Whatever the client builds a world out of, it
  builds it with `chunk::assemble`.
- **Teach the renderer what a patch is.** A chunk that changed is a `WorldMap`
  that changed, and every reader downstream already takes a
  [`MapSnapshot`](../../../crates/common/map/src/snapshot.rs).

## Backlog

Found while writing this, and each is somebody's.

- **A base set could store its chunks deflated.** 107,528,650 → 22,363,473 bytes
  on the same content, measured above. It is a version 2 of the file and it
  touches `openshard_basemap::write`/`read` alone — the table already makes each
  chunk independently addressable. Not E's, because E's wire format is not the
  file format, but E3's cache is the caller that would want it most.
- ~~**`WorldState::publish` answers `PatchError::NoGround` for a facet that does
  not exist**~~ — carried over from the last handoff and **fixed**: it says
  `expect("an entity's facet is always loaded")` now, which is what every other
  reader of a missing facet in that file says.
- **`World` derives `Default`** ([`world.rs`](../../../crates/common/map/src/world.rs#L45)),
  which `docs/style.md` bans, and `World::new(None)` is the named constructor it
  already has.

Found while building E1:

- ~~**`ServerPacket`'s `one_of_each` does not hold one of each, and its own doc
  says it does**~~ — *"so a new variant that lies about its id or length has to be
  added here to compile"* was false: nothing checked it, and **ten of the
  sixty-two variants were missing** (`MultiTarget`, `DeathAnimation`,
  `OpenContainer`, `AddToContainer`, `DesignRevision`, `PropertyListReply` and
  all four party packets). Every one of them was therefore outside
  `every_packet_frames_to_its_own_length`, which is the oracle for
  `server_packet_length` — the table whose entire job is to be right, and whose
  being wrong is a dropped connection rather than a dropped packet
  (`0xD6` and `0xD8` are both in that table because it was short an id twice).
  **Fixed**, in the two halves the claim needs: an `every_variant!` macro writes
  both the list of variant names and a wildcard-free `match` over them from one
  source, so the compiler refuses a variant missing from the list; and
  `the_fixture_holds_one_of_every_variant` is what then holds the fixture to that
  list. The ten are in it, and the table was right about all ten — which is a
  result and not a formality, because nothing had asked.
- ~~**`WorldState::facet_state` panics on a facet nothing loaded, and there is no
  accessor that does not.**~~ **Fixed**: `facet_state_if_loaded` is the
  `Option`-returning one, `chunk_answers` goes through it instead of indexing
  `state.facets`, and both accessors now say in their docs which question they
  are for — a facet number off an *entity* is an invariant and panics, one off
  the *wire* is an input and is refused. A test asks for chunks on a facet the
  shard never loaded, which nothing covered before.
- **`PacketReader`/`PacketWriter` had no `u64` until this phase.** Added, because
  a map revision is one and splitting it into two dwords at the wire would be a
  second spelling of the same number. Worth noting only because it means no
  reference packet has ever carried a field this wide.

Found while fixing the two above:

- **"Is this facet loaded" has no accessor either, and nine callers ask it by
  hand.** `state.facets.contains_key(&facet)`, each one followed by the same
  `if … { facet } else { default_facet }` — `enter.rs`, `travel.rs`, `decor.rs`
  (twice), `regions.rs` (three times), `gates.rs` (three times), `persist.rs`
  (three times), `npc/spawn.rs`, `items/spawn.rs`, `gm.rs`. `facet_state_if_loaded`
  now covers the *reading* half of that question; the falling-back half is one
  named method — "the facet this number means on this shard" — and it would put
  the rule in one place rather than in eleven `if`s that have to agree.
- **`a_property_list_is_framed_even_though_no_variant_carries_it` is now named
  after something that is no longer true.** A variant does carry `0xD6` —
  `PropertyListReply`, in the fixture since this fix. What has no variant is the
  server-side *builder*, `PropertyList`, which is what the test actually frames,
  and the test is still worth having for exactly that reason: a second writer of
  the same id is a second chance to disagree with the table. Rename, don't
  delete.
