# What a change costs, and what the world still borrows

> **Scope: S1 to S6, in order** — what era S has left once the representation
> itself works. S1, S2 and S3 have since been built and their sections are kept
> for the reasoning the three that follow inherit; **S4, S5 and S6 are the work
> this plan is open for.** Direction
> [D](../../../docs/world/evidence/2026-08-25-seven-directions.md#d--derived-data-keyed-by-revision)
> is the one S2 and S3 were made of.
>
> Status does not live here — it is
> [`docs/world/README.md`](../../../docs/world/README.md). The directions and
> their reasoning are
> [the seven directions](../../../docs/world/evidence/2026-08-25-seven-directions.md),
> the model is [`design_snapshot.md`](../../../docs/world/design_snapshot.md),
> and the two plans this one succeeds are
> [A0/A](../../../docs/world/evidence/2026-08-25-one-world-one-door.md) and
> [E](../../../docs/world/design_chunks_to_the_client.md).

## Where this starts

The representation itself is finished and works. A facet is one
[base set](../../../crates/common/basemap/src/lib.rs) — `OSBS` version 1, a
26-byte header, a table of offsets and 7,168 canonical chunks on Felucca,
102.6 MiB — with an append-only [`.ospatch`](../../../crates/common/basemap/src/patches.rs)
log beside it. [`MapSnapshot::publish`](../../../crates/common/map/src/snapshot.rs)
takes `&mut self` and refuses a parent that is not the revision it holds, which
is the whole conflict model and also what makes a publish atomic.
[`World`](../../../crates/common/map/src/world.rs) is base plus live layer in one
value. A client of ours takes the facet off the wire and keeps it under a
`WorldId`.

What was *not* finished is the price. One operator typing `.setland` used to cost
this — the first four rows are what S2 and S3 have since answered, and the last
is S4:

| What a one-tile publish moves | What it cost | What it costs now |
|---|---|---|
| the span index | **115.4 ms**, on the shard's tick *and* on the client's event-loop thread | 0.3 / 0.4 ms |
| the coarse graph | **11.6 s** if rebuilt, so it was **dropped outright** | 80 ms |
| the client's `WorldMap` | **16.3 ms** of the window's 132, because one block's item count may have moved | 0.6 ms |
| every product of every **untouched** chunk | unreachable — one revision covers the whole facet | carried |
| every subsequent boot | the whole log replayed, forever | still, until S4 |

Where they are: [`publish_cost.rs`](../../../crates/common/movement/tests/publish_cost.rs)
for the first three, [`radar.md` §10.2](../../../docs/world/design_radar.md) for the fourth and
[`basemap::load`](../../../crates/common/basemap/src/lib.rs) for the last.

And one thing the world still borrows: a base set replaces `map`/`statics`, and
**not** `tiledata.mul` or the multis. A shard with no UO install still cannot
say what a tile *is*.

## Two rules this inherits and does not reopen

- **Nothing here may be answered with a ground overlay.**
  [`map_rebuild.md`](../../../docs/archive/world/map_rebuild.md) refuses one on three counts, and the
  third is load-bearing: an overlay of ground would have to be *inside* the bake
  for the bake to be right, so it is not a live layer at all — it is the base
  with a slower spelling.
- **The base is what only the shard and a client of ours can see.** Anything
  that must reach a stock client is an entity on the wire
  ([`mechanics.md`](../../../docs/world/design_snapshot.md)). No node below changes that.

## S1 — one version 2, and it carries three things

**Goal.** The file learns enough about itself that S2, S3 and S4 are possible
without touching it again.

`OSBS` version 2, and **one** bump rather than three, because a version byte
names a layout and two migrations for one quarter's worth of work is two
migrations too many:

- **Chunks are stored deflated.** 107,528,650 → 29,698,618 bytes on the same
  content. It touches `write`/`read` alone: the table already makes every chunk
  independently addressable. The client's cache from E3 is the caller that wants
  it most, and it holds a whole uncompressed Felucca per world today.
  **At level one, and that is a measurement rather than a default** — the wire's
  level six costs 3.7 s to pack a facet against 0.5, and buys 6.9 MB. The table
  is in
  [`DeflateLevel`](../../../crates/common/protocol/src/chunks.rs), which is where
  the two levels are argued: the wire sends one chunk at a time and cares about
  the packet, the file writes 7,168 at once on a thread somebody is waiting on.
- **The table grows a hash per chunk.** `fnv1a64` over the chunk's canonical
  bytes, which is the hash
  [`identity_of`](../../../crates/common/basemap/src/lib.rs) already uses over
  the whole set, so no dependency and no second hash function. At 64×64 the
  manifest is **0.27 MiB** — [`chunk.rs`'s own size table](../../../crates/common/map/src/chunk.rs)
  measured it while choosing the chunk size, and the argument that killed a
  manifest at 8×8 (17.5 MiB, a ninth of the set it indexes) does not survive the
  size that was chosen. What it is *for* is integrity, the mint below and S4's
  squash; S2 turned out not to need it, and the reasoning is there.
- **The header carries a minted world id.** Today `identity_of` hashes the whole
  base set at boot. That is right for E3's question and wrong for S4's: a squash
  rewrites the bytes without changing the world, and every client would refetch a
  facet nothing moved in. So the id is **minted at import**, written into the
  header, and **carried** by every later rewrite of the file. It is minted over
  the *manifest* rather than over the file, for two reasons that are one: a hash
  of the file cannot be minted from inside the file it goes in, and a hash of the
  chunks' content does not move when a compressor is upgraded under it.

**Decisions, taken here.**

- **The hash is of content, not a revision counter.** A counter is cheaper to
  maintain and loses the property that matters: a re-imported facet that changed
  nothing keeps every derived product and every client's cached chunk. This is
  E3's own reasoning about `WorldId`, applied one level down.
- **The wire record does not change.** `OSMC` in
  [`codec.rs`](../../../crates/common/map/src/codec.rs) stays version 1: the wire
  already deflates a chunk before framing it, and the hash a client needs to
  verify a blob against its name is the one it can compute. What this node adds
  is a hash *at rest*, so the shard need not re-encode a chunk to answer "did
  this one move".
- **Version 1 is refused by name, and re-import is the migration.** A base set is
  a bake of an install or an export of a world; there is no data in it that is
  not reproducible, so a converter would be a second write path with no second
  caller.

**Done when** a version 2 set round-trips byte-identically through
`write`/`read`, is 29.8 MB on Felucca, reports the same `WorldId` before and
after a rewrite of its own bytes, and a version 1 file loads to a named error
rather than a plausible world.

**What it costs, measured** — `openshard-uofiles`'s `base_set_cost` example, on
Felucca, fastest of three, in the dev profile a person plays under:

| | version 1 | version 2 |
|---|---|---|
| the file | 107,528,650 B | 29,842,020 B |
| `write` | ~150 ms | **578 ms** — deflate 435, encode 50, hash 83 |
| `read` | 123 ms | **447 ms** — inflate 291, hash 83, the rest decode and assemble |

Who pays it: an import, which is offline and once; a client keeping the world it
was given, which is once per world and follows a fetch that was *seconds*; and a
boot, where the read is 0.3 s slower and 78 MB lighter off the disk. Nothing on a
tick, and nothing in a frame. The one number that had to be checked is `write`,
because [`link.rs`](../../../crates/client/app/src/link.rs) justified writing the
cache whole with *"the write is a tenth of a second against a fetch that was
seconds"* — at level six it was 4.2 s and that sentence was false; at level one
it is half a second and it holds.

**`openshard-basemap` joined the dev profile's opt-level list** for the reason
`openshard-map` is on it: at `opt-level = 0` its two byte-at-a-time loops made
the same read 830 ms.

## S2 — a product is keyed by the chunk it was built from

**Goal.** A publish invalidates what it moved, and nothing else.

Independent of S1, as it turned out. Today every bake and cache that has a
revision at all keys on the *facet's* revision, which one publish moves for all
7,168 chunks at once. [`radar.md` §10.2](../../../docs/world/design_radar.md) states the debt from the
reader's side — *"a chunk whose content did not change should keep its identity
across a facet's revision"* — and names the shard that gives the path a
production writer as the one who owes it. That is us, as of C's second half.

**The mechanism is a carry, not a second key.** The first draft of this node
keyed a product by the hash of the chunk it was built from, and that was taken
back before it was built, on two counts. It would have made every world loaded
from a UO install hash its whole facet at startup — the playground's world is one
of those — where the alternative costs nothing; and the ladder's coarse products
have no content of their own to hash, so a parent's key would have had to be
*combined* out of its children's rather than simply compared. What replaces it:

- `RadarCache::moved(facet, revision, touched)` — the facet moved, and these are
  the base chunks that changed. Products covering a touched square are dropped
  and marked dirty; **every other product is re-keyed to the new revision**, at
  every level, because a square nothing touched builds the same pixels it already
  holds. The touched list projects up the ladder the way
  [`invalidate_tile`](../../../crates/client/render/src/radar.rs) already
  projects a tile.
- **The key stays, and stays fail-closed.** A facet whose revision moves without
  a `moved` call still leaves every product unreachable, which is exactly what
  happens today. The carry is what a caller who *knows* what changed is allowed
  to claim; nothing is trusted by default.
- The two bakes that carry a `MapRevision` — the navigation bake and the building
  flood — are facet-wide artefacts with no per-chunk products, so they refuse
  themselves on a publish and stay S3's problem. The occluder measurements, keyed
  to file mtimes, are the one reader with no revision at all.
- **`invalidate_tile` stays test-only**, and this is the node that makes that
  final rather than pending: a chunk is sixteen thousand tiles, a publish names
  chunks, and a per-tile invalidation is a second vocabulary for the same fact.

**Done when** a publish that touched one chunk leaves the products of the other
7,167 reachable at the new revision, asserted on the radar cache, and the
client's publish path calls `moved` where it now calls `set_revision`.

## S3 — a block is replaced where it stands

**Goal.** A publish costs what it moved, on both ends.

Independent of S1 and S2; the most visible of the six, because it is the one a
person waits for. Two artefacts, one cause: the span index and `WorldMap`'s
statics are both facet-wide packed runs addressed by a prefix sum, and a prefix
sum *is* the ordering, so re-laying one block in place moves every run after it.

- **The spans** are [the span layer's N8](../../../docs/world/evidence/2026-08-25-the-span-layer.md#n8--the-bake-follows-a-patch),
  whose decision is already taken and is not reopened here: `blocks` names a
  `BlockTable` that carries its own `base` and `counts`, so a rebuilt block's
  spans are **appended and the table repointed** — O(the block) — with the read
  path byte-for-byte unchanged. Garbage rule: never compact during a session,
  except that dead spans exceeding live ones rebakes the facet whole.
- **The statics take the same shape.** R4 built the CSR pair with prefix-sum
  offsets, which is what leaves `chunk::apply` no move but a facet-wide rebuild.
  Giving `WorldMap` a per-block `base`+`count` table costs one extra `u32` a
  block — 458,752 × 4 B = **1.75 MiB** on a 150 MiB world — and costs the read
  path nothing: a prefix sum already reads two entries to bound a block, and a
  table reads two. It buys the same append-and-repoint, and it is the only thing
  between `chunk::apply` and O(the chunks that arrived).
- **The coarse graph is D's own**, and it is the third: `touched_chunks` names
  the 32×32 regions to rebuild, plus the half a naive implementation forgets —
  the neighbouring regions whose answer *crossed* into the changed chunk. It used
  to be dropped on every publish, which was correct and was not free.

**Done when** a one-chunk publish is O(the chunk) at both ends, measured against
today's numbers by the test that produced them
([`publish_cost.rs`](../../../crates/common/movement/tests/publish_cost.rs)
extended to the client's `apply`), and `mapedit::commit` rebuilds the coarse
graph instead of dropping it.

**Built, all three.** Measured by that test on Felucca, one `.setland`, in the
profile a `cargo run` builds:

| | before | after |
|---|---:|---:|
| the span index, on the tick | 109.7 ms | **0.3 ms** |
| the span index, at the window | 128.6 ms | **0.4 ms** |
| `WorldMap`'s statics, a publish that adds one | 3.9–5.6 ms | **0.4 / 0.6 ms** |
| the coarse graph | dropped, 28.0 s to rebuild | **80 ms** |

The graph's is [`navigation_graph.md`'s G1](../../../docs/world/design_navigation_graph.md#g1--the-graph-follows-a-patch),
which holds what the doing of it added and the one thing left in it: 80 ms is the
price of the *chunk*, and the shard knows the tiles.

## S4 — the log is folded

**Goal.** A shard that has been edited for a year boots like one that has not.

Needs S1's minted world id. [`basemap::load`](../../../crates/common/basemap/src/lib.rs)
replays every committed patch on every boot, on the shard and in every offline
bake. [`mechanics.md`](../../../docs/world/design_snapshot.md) already calls a full rebuild *"an explicit
operation — a new base, a squash, an import"*; the squash is the one of the three
nobody built.

- `openshard-map-squash` reads a base set plus its log, writes a new base set at
  **the revision the log ends at** — not at 1, because a revision is what every
  cached chunk, bake stamp and client cache is keyed by — and carries the world
  id forward.
- The log is **archived, not deleted**: `world0.ospatch` becomes
  `world0.ospatch.<revision>` beside the set. The history is the reason a patch
  is attributable; a squash is a load-time optimisation and must not be a way to
  lose the record of who changed what.
- **When** is an operator's call and the tool says so rather than guessing: it
  prints the replay cost of the current log at the top of its help.

**Done when** squashing N patches produces a set whose `load` yields
byte-identical chunks at the same revision under the same `WorldId`, with a load
time that no longer scales with N, and the archived log still reads.

## S5 — revert is a verb

**Goal.** The word the model has used since its first page becomes something an
operator can type.

Every applied op returns its own inverse, `Undo` exists, and
[`patch.rs`](../../../crates/common/map/src/patch.rs) records that *"a revert is
a new patch, not a rewritten history"* — but nothing builds one. There is no
staff verb and no CLI arm.

- A revert is built by **replaying**, not by reading the stored patch backwards,
  and [`apply_op`'s doc](../../../crates/common/map/src/patch.rs) says why:
  `AddStatic`'s inverse names an ordinal that is only knowable once the item is
  in.
- Reverting anything other than the tip is allowed and is **refused honestly**
  when it cannot apply: every op carries what it is taking away, so a world that
  moved under it produces `LandNotAsRecorded` or its static equivalent, naming
  the op. Silent partial reverts are the one outcome that must not exist, and
  the all-or-nothing rule already in `publish` gives it for free.
- Both doors get it, because both doors already exist for a commit: `.revert` as
  a staff verb beside `.setland`, and an arm of `openshard-map-patch`.

**Done when** reverting the last patch reproduces the previous revision's chunk
hashes exactly, reverting a middle patch either applies or refuses naming the
op, and a revert appears in the log as a new patch with its own author.

## S6 — what the world still borrows

**Goal.** The last three files that make a shard need somebody's UO install.

This is the oldest gap in the track and the least visible, because it is the one
thing [`overview.md`](../../../docs/world/research/a_map_we_can_change.md)'s promise names that no direction was ever
written for. A base set replaces `map*.mul` and `statics*.mul`. It does not
replace:

- **`tiledata.mul`** — what a tile *is*: walkable, a surface, a wall, its name.
  It is a table of 16,384 land entries and 65,536 static ones, it is read by
  both ends, and the movement rules are meaningless without it. Same shape of
  work as B: our own file, an importer from the install, and the shard resolving
  it beside the base set.
- **The multis** — a house's component list, which
  [`customisation.md`](../../../docs/customisation.md) is already unhappy with for its
  own reasons. It comes after tiledata and inherits its file.
- **The art** — and this one is not ours to schedule. `mechanics.md` says art
  travels separately, addressed by content, with a missing asset drawing an
  explicit placeholder rather than whatever the player's install has under that
  number. What it needs first is a licensing answer about what may be
  redistributed, which is an operator's question and not a technical one.

**Done when** a shard boots and a client of ours draws and walks a world with no
UO install present at either end, art excepted and named as the exception.

## Kept open, deliberately

| | |
|---|---|
| **Which validation blocks a publish** | Technical validity is mandatory and is what `apply` already enforces. The design list — reachability, smoothness — is **empty on purpose**, and stays empty until somebody names a rule and the world it protects. Nothing goes into the apply path speculatively. |
| **Land height per tile or per corner** | Closed the day we mean to change the geometry, and not before: today's answer is whichever keeps movement and rendering identical. |
| **G — residency and compression at rest** | Still a constraint rather than a step. The offsets table exists so a chunk is a seek and a read away; opening it wants the working set a real session touches, which nobody has measured and nobody has asked for. |
| **A hue side table, a draw-order field** | [`codec.rs`](../../../crates/common/map/src/codec.rs) refused both with numbers. Not reopened by S1's version bump. |

## Order, and the one thing allowed to jump it

**S1 → S2 → S3 → S4 → S5 → S6.**

S1 is built. S2 turned out not to depend on it — the carry needs a list of what
moved and not a hash of what did not — so the only ordering left is S4 behind
S1's world id. S5 and S6 are independent of all of it.

**S1, S2 and S3 are built.** What is left of era S is S4, S5 and S6.

**S2 is closed whole**: `RadarCache::moved(facet, revision, touched)` carries
every product no touched chunk lies under, at every level, and leaves the ones
over the edit retained and stale the way a facet-wide bump leaves all of them —
and the client's publish path calls it. It closes two things the same call was
always going to close — [`radar.md` §10.2](../../../docs/world/design_radar.md) in the cache, and a sweep
that a publish used to finish silently — and makes `invalidate_tile` test-only
for good. The coordinate change from a map chunk to a base radar chunk lives at
that caller, which is the one place holding both types.

**S3 is closed whole**, and the numbers are in the table above: the span bake,
`WorldMap`'s statics and the coarse graph all follow a publish over the chunks it
named. The graph's is the one with something left in it — see
[`navigation_graph.md`'s G1](../../../docs/world/design_navigation_graph.md#g1--the-graph-follows-a-patch),
which says why 80 ms is the price of the chunk rather than of the edit.
