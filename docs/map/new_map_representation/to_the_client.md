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
  **The graph did not stay absent, and could not** — see the last entry below.

**Done when** the client runs against a shard with no `map*` or `statics*` files
present at all and draws the same world an install-fed client draws, sampled over
the facet.

**Taken in E2, and each is a thing this document did not foresee:**

- **The third arm is the *client's* enum and not
  `openshard_movement::bake::WorldSource`'s.** The shard's boot and both bake
  binaries share that type, and none of them can ever take a `Shard` arm — a
  shard has no shard to ask. So the client owns a three-armed enum of its own and
  converts at the seam, which is the one place the two have to agree.
- **The window keeps its ground behind a gate, not behind an ordering.** The
  other arrangement was available and is the obvious one: hold the first
  `Update::World` back until the facet is here, and the window never exists
  without ground. It is refused because the packets that keep arriving during a
  21 MiB fetch have to go somewhere, and the only two places are the shard
  thread's own unbounded buffer or the bounded mailbox the window drains — which
  the window cannot drain while it is blocked on a value that thread is holding
  back. So the gap between "there is a world" and "there is ground under it" is
  real, and `Resources::grounded` is checked at the two doors that can reach the
  map: the frame, and the window's own events.
- **The fetch is a state machine in `client/net` and the loop is in `link.rs`.**
  `Fetch::next_request` hands out a packet and `Fetch::on_packet` takes one, so a
  whole facet's transfer is testable against a fixture with no socket in it.
- **A request is full or it is the last one.** The in-flight window is a count of
  chunks, so one chunk coming back is room for exactly one going out, and topping
  up per chunk would ask for Felucca in four full requests and then 6,912 naming
  one chunk each. Waiting for room for a whole request leaves 192 chunks on the
  wire while the next one goes out.
- **A chunk that arrives is checked against the chunk that was asked for.** The
  check `join`'s own doc leaves to the caller, and it has to be made: a chunk is
  self-contained and names itself, so a swapped pair is indistinguishable
  downstream from a missing one — `assemble` would refuse the facet for the
  blocks the duplicate did not cover and name the innocent chunk.
- **Every failure of the fetch ends the connection.** A client told to take the
  shard's ground and given something that is not a facet has nothing to draw and
  no second source, so the honest thing is one line saying which chunk and why.
- **🚩 "No coarse graph" is not a thing a player can be asked to live with.**
  This section wrote the absent graph down as a cost like the interiors flood,
  and it is not the same kind of cost at all: the flood is a diagnostic, and the
  graph is *how a click gets out of a building*. Measured on Felucca from an
  upper storey at (1340, 1676): of the 1,681 places in a 41×41 square around it,
  the shipped plan reaches **895 with the graph and 415 without**, because the
  bounded 600-node search cannot see round a house. A person playing E2 reported
  it as the pathfinder not computing, which is exactly what it is.
  **What it wanted was already here.** E3 keeps the world as a base set of ours,
  and a base set is a world an artifact can be stamped against — so
  `Update::Ground` carries the kept file's path and the client takes the graph up
  from beside it: load it, or build it off the frame loop (11 s on this facet,
  once per world) and keep it there. It arrives as `Update::Navigation`.
  This is also what made an artifact's name have to say **which world** it is a
  bake of: a client's world and a shard's base set share a working directory —
  see [`navigation_graph_bake.md`](../navigation_graph_bake.md).

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

Two more subcommands, and one field on E1's notice:

```text
0xE004  WorldNotice    server -> client, on world entry
        ... as E1, and then: world named u8, world id u64

0xE007  ChangesRequest client -> server
        facet u8, revision u64

0xE008  ChangesReply   server -> client
        facet u8, revision u64, answer u8,
        then for "these" only: count u16, count x { chunk x u16, chunk y u16 }
```

**Done when** a second run over an unchanged world asks for no chunks at all, and
a second run over a world that moved by one patch asks for exactly the chunks
that patch touched.

**Taken in E3, and the first of them is the open question above:**

- **It is rewritten whole.** Measured on the shipped Felucca rather than argued:
  `openshard_basemap::write` is **0.10–0.13 s** for 7,168 chunks and 102.6 MiB,
  and the flush behind it does not register. A tail would save a tenth of a
  second per edit and cost a version 2 of the file format, a second read path and
  a compaction rule. (The read is 0.12–0.19 s, which is the number E3 is *for*:
  that against seconds of fetch.)
- **A cache is filed under the world and not under the shard.** The address
  dialled is the obvious key and it is wrong twice — our own playground dials
  nothing, and a shard that re-imports its facet serves a different world at the
  same address whose first revision is 1 again. So the shard names its world in
  the notice (`WorldId`, a hash of the base set's bytes taken at boot), the name
  goes in the file name, and a facet the shard *cannot* name is not kept at all.
- **The cache is read on the shard thread, after the notice.** This document
  guessed that a cache hit would be `BaseSet` with a path the client chose; it
  cannot be, because the path is named by an identity that arrives after login.
  E2's startup order is unchanged — what a cache changes is only how long the
  window waits for ground.
- **What moved is the log's answer, computed per request** — `0xE007` and
  `0xE008`, with `Changes::Everything` as the one answer to four different facts
  the client acts on identically, and a cap of 4,096 chunks because that is what
  a packet holds. Nothing is cached on the shard, for the reason its own chunk
  reader gives.
- **An empty list is knowledge**: a world that moved by an empty patch has not
  moved, and saying `Everything` would send a client to fetch what it has.
- **Every chunk of an incremental fetch is checked against the revision the
  difference was asked about**, because the list is a statement about two
  particular revisions and a publish in between makes it a list of the wrong
  squares.
- **`chunk::apply` is `assemble`'s other half.** It rebuilt the facet at first,
  on the grounds that a block's statics are one run in a facet-wide vector so a
  chunk whose item count changed moves every static after it — there being no
  splice that is not a copy of the tail. **Reversed on the measurement**: the
  tail copy is a quarter of the rebuild, because what the rebuild added to it
  was the land no splice touches and a re-sort of every block on the facet. It
  now writes the squares in, through `WorldMap::replace_blocks`. See the backlog
  entry, which has the numbers. E4 is its other caller.

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

**Taken in E4:**

- **The notice goes to everyone standing on the facet**, and there is no
  subscription. The obvious alternative is to remember which connections have
  sent a `0xE002` and tell only those, and it is wrong for the reason
  `WorldNotice` is sent unasked in the first place: *a client cannot ask about
  something nobody told it happened*. E3's best case — a kept world already at
  the shard's revision — asks for **nothing at all** on the way in, so a
  subscription built out of "who asked" would leave the client whose cache works
  best the one that never hears about an edit. A stock client drops the
  subcommand, and a client of ours drawing its own disk's facet ignores it.
- **The body is `ChangesReply`'s**, so the two are one encoder and two
  subcommands. They are the same three facts — a facet, the revision it is at,
  and what moved to get there — said for two different reasons, and `Everything`
  means the same thing in both: *more than a packet can name*, take the facet
  again. That is the only one of the reply's four reasons a notice can have.
- **The chunks cross the seam, not the world.** By the time a publish reaches a
  client that is drawing, the facet belongs to the *window* — a `MapSnapshot` has
  one owner per process — so the thread that owns the socket has nothing to apply
  them over. `Fetch::moved` therefore ends in `Fetched::Chunks`, and `finish`
  answers with an enum rather than panicking for the arm it was not built for.
  `World::take_chunks` is the far end, and `Ground::take_chunks` wraps it with
  the span rebake in the same statement, exactly as `publish` does.
- **The kept file is left at the revision it was written at.** Rewriting it would
  mean the world coming back across that seam to be written from, and what it
  saves is one small fetch on the next connection — which is exactly the
  mechanism E3 built and this client runs anyway. The next start asks what moved,
  is told these same chunks, and writes the file then.
- **The invalidation is by block, except the radar, which is by revision** — and
  that asymmetry is the caches' own. The composited pictures are keyed by where
  they are, so what is dropped is named by the blocks the chunks cover; the
  radar's products carry the source revision in their key, so naming the new one
  makes every one of them unreachable at once while the stale-exact path keeps a
  minimap from blinking empty. `RadarCache` was built with that field and *no
  writer for it* — "this path has no production writer today, the client's
  `WorldMap` cannot change at runtime" — and this is the writer it was waiting
  for.
- **The coarse graph is dropped and not rebuilt.** Eleven seconds of flood, the
  same trade the shard makes when it publishes and the same answer it gives its
  operator: long routes are the bounded search until the client reconnects, which
  is when a graph is looked for beside the kept world again.
- **A publish that lands while ground is still arriving ends the connection.**
  The answers to the fetch in flight are on the wire at the revision the publish
  has just moved past, and nothing tells them apart from the answers a second
  fetch would ask for. So it ends *here*, naming the publish, rather than seconds
  later inside `assemble` naming a mixed set of chunks. Draining the abandoned
  fetch is what would make this a recovery, and it is in the backlog.

## What this must not do

- **Send anything to a client that did not ask.** The two *notices* are the
  exception and they are one exception, taken for one reason: a client cannot ask
  about a world nobody told it about, and it cannot ask what moved unless
  somebody says something did. `WorldNotice` is seventeen bytes of body in a
  twenty-two byte packet and `PublishNotice` is twenty-two for a one-chunk edit —
  a stock client reads the length out of the envelope and drops both.
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
- ~~**`World` derives `Default`**~~ ([`world.rs`](../../../crates/common/map/src/world.rs)),
  which `docs/style.md` bans, and `World::new(None)` is the named constructor it
  already has. **Gone**, taken off by another session — the type carries `Debug`
  and nothing else now.

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

Found while building E2:

- **`link::play`'s own wiring has no test, and it is the one seam E2 added that
  does not.** The pieces under it are covered — `Fetch` against a fixture in
  `client/net`, the whole loop against a real shard in `e2e/shard`'s
  `chunks.rs` — but the thirty lines in `play` that join them (the `continue`
  that keeps a chunk packet off the mailbox, the order of `Update::World` and
  `Update::Ground`, the four error returns) are checked only by running the
  playground. The obstacle is honest and structural: `link` is private to
  `openshard-client-app` and that crate cannot see a shard, while
  `openshard-e2e-shard` can see both ends and cannot see `link`. Either the
  module goes public for a test, or `e2e/playground` grows a `tests/` that drives
  `connect` — the second is the smaller change and the playground already links
  both.
- ~~**`world_of_ours` is written twice, in `map_edit.rs` and in `chunks.rs`.**~~
  The same twenty-five lines — write a base set, bake a graph beside it, hand
  back the path — differing only in whether statics are placed. E2 generalised
  `chunks.rs`'s copy by a `blocks` argument rather than adding a third; the lift
  is a `tests/common/mod.rs` those two share, which is dev-only and costs the
  `openshard-e2e-shard` library nothing. **Lifted**, along with `config_over`,
  `install`, `scratch` and `say_and_hear` and the three constants under them —
  see the E3 entry below, which is the same finding one caller worse.
- ~~**A fetch that straddles a publish fails the whole facet.**~~ `assemble`
  refuses `MixedRevisions`, which is right — half a world before an edit and half
  after is a world that never existed — but the client's answer to it was to end
  the connection. **Fixed** by drain-and-restart; the entry E4 left is where it
  is written up.
- **The window is blank for the length of the fetch and only the terminal says
  why.** 21.3 MiB of Felucca is seconds, and what a person sees is an unpainted
  surface while `the ground: 4096 of 7168 chunks` scrolls past in a console they
  may not be looking at. E3 makes the common case instant, so this may never be
  worth a picture; if it is, the state already exists — `Resources::grounded` is
  false and `WorldState::connection` is the strip that says what the connection
  is doing.

Found while building E3:

- **`link::play`'s wiring is now larger and still has no test.** E2's finding,
  one phase worse: the three-way decision between keeping, asking and fetching is
  `decide` in `link.rs`, and the packet loop grew a state for waiting on the
  answer. The e2e test writes the same three branches out by hand, so what is
  untested is the joining — which is where a state machine goes wrong. The fix is
  unchanged: a `tests/` in `e2e/playground`, which already links both ends.
- **A world that moved by an empty patch keeps its old revision in the cache.**
  The client is told nothing changed, keeps the world it has, and does not
  re-stamp it — so it asks the same question on the next connection and is told
  the same thing. One packet, on a world nobody will ever publish; the fix is a
  `MapSnapshot` that can be re-stamped without being rebuilt. **Half of that
  exists now**: `MapSnapshot::take_chunks` moves the world and its number in one
  call without rebuilding a facet to carry the new one. What is still missing is
  the *empty* case, which never reaches that call — a world told `These([])` has
  nothing to apply, so nothing re-stamps it. It wants the same door with no
  chunks in it, and `apply`'s "applying no chunks is not a change" is the
  sentence to reconsider when it is written.
  **What the other half costs, before anybody writes it: the graph.** A bake
  carries the revision it was built from and `bake::load` refuses one whose stamp
  names another number — `incompatible_stale_and_corrupt_files_are_distinct`
  moves nothing but the number and reads the refusal back. So the
  re-stamp only *pays* if the file is re-stamped too, and a file at revision *n*
  + 1 with a graph beside it stamped *n* is a graph this client drops: eleven
  seconds of flood, or a session with no long routes, traded against a 22-byte
  request and its reply once per connection. The behaviour that is here is the
  better one until the stamp can say "the same world, renumbered" — and
  `Patch::new` already says how rare the case is: *"an empty `ops` … does
  invalidate every bake over the facet, so an editor should not publish one"*.
- ~~**Nothing sweeps an orphaned world.**~~ A shard that re-imports its facet
  leaves the client's old copy behind under the old identity, and 102 MiB is not
  nothing. The names of every world a client has kept are in one directory, so
  what is missing is a rule about how many to keep rather than a mechanism.
  **The rule is `cache::KEPT_PER_FACET`, and it is two.** On every write each
  facet's worlds are ranked and the tail goes, taking everything named after it —
  the navigation graph, which `bake::artifact_path` names after the world's file
  stem, and any `.osbase.writing` a torn write left. Ranked by when each was last
  *used* rather than last written, because `cache::read` now stamps the file it
  read: a world that is already at the shard's revision is never rewritten, so the
  other clock would have let go of the one cache that pays for itself on every
  connection. Two and not one because a person who plays two shards should not
  re-fetch a facet on every start; per *facet* and not per directory because a
  shard has six of them and walking between them must not evict that shard's own
  ground.
- **The cache directory is the working directory and nothing can move it.** No
  flag and no environment variable, because `client_ui.toml` sets that precedent
  and nobody has asked. A read-only checkout is the case that changes it, and
  `bake::artifact_path`'s `OPENSHARD_NAVIGATION` is the shape it would take.
- ~~**`world_of_ours` is now written twice and a half**~~, since `e2e/shard`'s
  `chunks.rs` grew a copy of `map_edit.rs`'s `say_and_hear` as well. Same lift,
  one caller worse. **Lifted** into
  [`e2e/shard/tests/common/mod.rs`](../../../crates/e2e/shard/tests/common/mod.rs):
  `world_of_ours` with the `blocks` argument E2 gave one copy and the statics the
  other placed, plus `config_over`, `install`, `scratch`, `say_and_hear` and the
  three constants under them. A third test that boots a shard on a world of ours
  is now `mod common;` and nothing else. A `tests/` module and not the
  `openshard-e2e-shard` library, which is the half worth recording: everything in
  it reads an install, bakes a graph or drives a socket on a timeout, and none of
  that belongs in a crate a non-test caller can link.

Found while accepting E3 — by running the playground, which is what E2 and E3
both left owed:

- ~~**`App::create_window` reads the map, and it is neither of the two doors
  `Resources::grounded` is checked at.**~~ **Fixed.** It packs the atlases for
  the frame that has not happened yet, and it packed them out of
  `wanted_now` → `Resources::map` — so
  `cargo run -p openshard-playground -- --world-from-shard` panicked at the
  window, every time, before a single chunk had landed. It is E2's defect and
  not E3's: the gate went on the frame and on the window's events, and the
  *third* reader ran once before either of them existed. It packs nothing when
  there is no ground now, and leaves `graphics.covered` unset, which is already
  what tells `ready_atlases` to grow over the whole lit rectangle on the first
  frame that has one — the same work, one frame later.
- **This is what an untested startup path costs, and the entry above it named the
  fix a phase ago.** Two handoffs recorded "not run: the playground itself" and
  both moved on; the panic was on the first line of the first acceptance. A
  `tests/` in `e2e/playground` would not have caught *this* one — it is a GPU
  path and a test has no window — but the same absence is why nothing between
  `run` and the first frame is exercised by anything except a person watching.
  The cheap half is worth naming separately: **the startup order is only ever
  proven by starting**, so a phase that changes it owes the run, and neither of
  the two that did paid it.

Found while building E4:

- ~~**A fetch that straddles a publish is ended and not recovered.**~~ The
  finding E2 left, E3 handed on and E4 could not close — **fixed**, in the shape
  it asked for. `Fetch::abandon` turns the fetch in flight into two values: a
  [`Drain`](../../../crates/client/net/src/chunks.rs), which eats the answers the
  shard still owes without decoding one of them, and a `Restart`, which is what
  to ask for once it is owed nothing. Nothing goes out on the wire in between,
  because the shard answers a chunk exactly once and an answer does not say which
  request it belongs to.
  **What to ask for again is a union**, and that is the decision the fix turns
  on: what the fetch was asking about is what moved between the world this end
  can still show and the revision it was fetching, and what the publish names is
  what moved from there — so a square in either list has moved as far as this end
  is concerned, and nothing will ever name it again. The three arms restart as
  themselves: a whole facet is taken whole (`Changes::Everything` absorbs
  whatever it is unioned with), a kept world comes back out of the fetch and is
  filled in, and the window's world still ends in chunks.
  The fourth case is the one the entry named separately and it needed no drain:
  **a publish that lands while a client is *asking* what moved** leaves no list
  to union, so the stale reply — recognised by the revision it carries against
  the newest the shard has announced — is answered by asking the question again.
  One request in flight at a time, and no state.
- **A publish costs the window a whole facet's rebuild** — ~~and nobody has
  measured it~~. **Measured now**, by
  [`publish_cost`](../../../crates/common/movement/tests/publish_cost.rs), and
  the measurement found something else first: *the cost nobody could see was the
  build profile*. `[profile.dev.package."*"]` reaches dependencies and not
  workspace members, so `openshard-map`, `openshard-movement` and
  `openshard-tiles` were compiled at `opt-level = 0` in every `cargo run`, and
  one `.setland` on Felucca was **1.17 s on the shard's tick and 1.29 s on the
  window's event-loop thread** — two and a half seconds of stall for one tile.
  The root `Cargo.toml` now names the three, which takes both halves to ~0.13 s.
  What is left after that is the real entry, and it is small enough to rank
  honestly: `SpanIndex::build` is 115 ms of the window's 132 and `chunk::apply`
  is the other 16, both still on the event-loop thread. The span half now has a
  node of its own —
  [`navigation_spans.md`](../navigation_spans.md#n8--the-bake-follows-a-patch)'s
  N8, queued rather than gated. ~~**The `chunk::apply` half stays here**~~ —
  **done 2026-08-25.** It was smaller than it looked and for the reason this
  entry named: `WorldMap::offsets` is a prefix sum, so only a chunk whose
  *static count changed* forces the facet-wide re-offsetting, and `.setland`
  never changes one. What the splice turned out to save was larger than that,
  though, and the rebuild's own argument is where it was hiding. "There is no
  splice that is not a copy of the tail" is true and it is not the cost: the
  rebuild *added* to that tail copy 117 MiB of land no splice touches and a
  re-sort of all 458,752 blocks rather than the sixty-four that arrived. On the
  shipped Felucca, against **15.3 ms and a second 150 MiB facet resident**:
  **0.1 ms** for a set that changed no block's item count — every edit to the
  ground, which is the equal-count case this entry named — and **3.9–5.6 ms**
  for one that did, most of it the reallocation `from_parts`' own
  `shrink_to_fit` makes unavoidable for the first item added to a facet.
  `WorldMap::replace_blocks` is the primitive: one span from the first replaced
  block to the last, one memmove, one pass over the offsets, and the land
  written where it stood. `MapSnapshot::take_chunks` is the door beside
  `publish`, and it re-stamps the revision without the facet being rebuilt to
  carry a new one — which is half of the empty-patch entry above, granted by the
  same call. The window's hitch is `SpanIndex::build` and nothing else now.
  The general shape is worth keeping twice: **"it is slow" is a claim about a
  binary, and the profile that built it is the first thing to ask about**,
  before any algorithm is blamed — and **an argument for a cost is not a
  measurement of it**, which is how a 4× saving sat behind a sentence that was
  true.
- ~~**A quarantined composite block is never un-quarantined.**~~ `CompositeCache`
  permanently marks a block that `FlatGroundBlock::inspect` refused, on the
  stated grounds that "map terrain is immutable for the lifetime of this cache" —
  which is exactly what E4 stops being true. A publish that flattens a block
  leaves it on the direct path for the rest of the session, which is slower and
  not wrong; the reverse is handled, since the invalidation drops the composite
  and the next preparation rejects it again. What is missing is one line of API,
  `rejected.retain(…)` under the same block invalidation. **Done**, at all three
  doors — `invalidate_block`, `invalidate_blocks` and `clear`, which is the arm a
  replaced facet takes. The one thing it needed that the entry did not name is an
  *order*: `quarantine` invalidates the block on its way in, so recording the
  verdict now happens after that call rather than before it, or quarantining
  would undo itself. A test asserts that first, since it is the way this fix
  fails silently.
- **The client never takes up an interiors bake for a world off the wire**, so
  E4's invalidation has nothing to say about one. `Update::Ground` takes up the
  navigation graph and not the interiors flood — the artifact is looked for only
  in `run`, from an install path — so `Resources::interiors` is `None` for the
  whole of a `--world-from-shard` session. Worth writing down because the day it
  is taken up, it is a third thing a publish makes stale.
- **`link::play`'s wiring has grown a third decision and still has no test.** E2
  and E3 both recorded this; E4 adds the publish arm — which is the one that runs
  *while the connection is doing something else*, and therefore the one whose
  ordering against a fetch in flight is hardest to get right by reading. Same
  fix, one caller worse: a `tests/` in `e2e/playground`, which links both ends.

Found while closing the fetch that straddles a publish
([handoff](handoffs/2026-08-25-a-fetch-survives-the-ground-moving.md)):

- **A whole-facet fetch restarts from zero on every publish.** A client with no
  kept world fetches 7,168 chunks over seconds, and a publish at any point in
  that abandons every one of them — a chunk that already arrived carries the old
  revision and `assemble` refuses a mixed set, even though a chunk the publish
  did *not* name has identical content at both. So an operator publishing faster
  than a facet arrives can keep a first-time client from ever finishing. What
  would retire it is a way for `assemble` to take a set whose revisions differ
  but whose content is known not to — the shard already computes which chunks a
  revision left untouched (`changes_since`), and an arriving chunk's own
  `revision` field cannot express it.
- **A fetch applies at its end and never as chunks arrive, and `Fetch::abandon`
  now depends on it.** `Filling::Held` hands the kept world back untouched, which
  is only right because `finish` is where `take_chunks` happens. A streaming
  apply — an obvious optimisation for a client watching a blank window — would
  make that world half a revision ahead of its own number.
- **`link::play`'s wiring has a fourth decision now.** The publish arm is a
  four-way match on what the connection is in the middle of, and two of its arms
  are new *states* rather than new packets. Same fix, four handoffs old.

Found while sweeping the four entries above:

- **`CompositeCache::latest_quarantine` outlives the quarantine it names.** Now
  that a block can be un-quarantined, a field dump can say `quarantine_count=0`
  and `latest_quarantine=Some(block …)` in the same breath — and the HUD line in
  `shell.rs` reads the second as if it were current. `clear` drops it, since a
  replaced facet makes every verdict meaningless at once; the block-wise doors do
  not, because the field is documented as *the most recent safety decision* and
  that is a thing that happened rather than a thing that is true. Which of the two
  it should be is the question: a decision this cache has since gone back on is
  worth keeping in a dump and worth *not* showing on a strip, and those want two
  different fields.
