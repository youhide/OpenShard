# What a change costs, and what the world still borrows

> **Status: the plan being executed now.** A0, A, B, C and E are built — the
> shard owns its world, a patch survives a restart, and a publish reaches a
> connected client. What is left of era S is this: **direction
> [D](plan.md#d--derived-data-keyed-by-revision) in full**, plus four things the
> doing of B, C and E left standing and one the track has borrowed since its
> first line. Six nodes, S1 to S6, in order, each with what "done" means.
>
> [`plan.md`](plan.md) holds the directions and their reasoning;
> [`mechanics.md`](mechanics.md) holds the model. This is the executable half,
> the way [`snapshot.md`](snapshot.md) was for A0/A and
> [`to_the_client.md`](to_the_client.md) was for E.

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

What is *not* finished is the price. Today one operator typing `.setland` costs
this:

| What a one-tile publish moves | What it costs | Where |
|---|---|---|
| the span index | **115.4 ms**, on the shard's tick *and* on the client's event-loop thread | [`publish_cost.rs`](../../../crates/common/movement/tests/publish_cost.rs) |
| the coarse graph | **11.6 s** if rebuilt, so it is **dropped outright** | [`mapedit::commit`](../../../crates/server/world/src/mapedit.rs) |
| the client's `WorldMap` | **16.3 ms** of the window's 132, because one block's item count may have moved | [`chunk::apply`](../../../crates/common/map/src/chunk.rs) |
| every product of every **untouched** chunk | unreachable — one revision covers the whole facet | [`radar.md` §10.2](../radar.md) |
| every subsequent boot | the whole log replayed, forever | [`basemap::load`](../../../crates/common/basemap/src/lib.rs) |

And one thing the world still borrows: a base set replaces `map`/`statics`, and
**not** `tiledata.mul` or the multis. A shard with no UO install still cannot
say what a tile *is*.

## Two rules this inherits and does not reopen

- **Nothing here may be answered with a ground overlay.**
  [`map_rebuild.md`](../map_rebuild.md) refuses one on three counts, and the
  third is load-bearing: an overlay of ground would have to be *inside* the bake
  for the bake to be right, so it is not a live layer at all — it is the base
  with a slower spelling.
- **The base is what only the shard and a client of ours can see.** Anything
  that must reach a stock client is an entity on the wire
  ([`mechanics.md`](mechanics.md)). No node below changes that.

## S1 — one version 2, and it carries three things

**Goal.** The file learns enough about itself that S2, S3 and S4 are possible
without touching it again.

`OSBS` version 2, and **one** bump rather than three, because a version byte
names a layout and two migrations for one quarter's worth of work is two
migrations too many:

- **Chunks are stored deflated.** 107,528,650 → 22,363,473 bytes on the same
  content, measured in [`to_the_client.md`](to_the_client.md). It touches
  `write`/`read` alone: the table already makes every chunk independently
  addressable. The client's cache from E3 is the caller that wants it most, and
  it holds a whole uncompressed Felucca per world today.
- **The table grows a hash per chunk.** `fnv1a64` over the chunk's canonical
  bytes, which is the hash
  [`identity_of`](../../../crates/common/basemap/src/lib.rs) already uses over
  the whole set, so no dependency and no second hash function. At 64×64 the
  manifest is **0.27 MiB** — [`chunk.rs`'s own size table](../../../crates/common/map/src/chunk.rs)
  measured it while choosing the chunk size, and the argument that killed a
  manifest at 8×8 (17.5 MiB, a ninth of the set it indexes) does not survive the
  size that was chosen.
- **The header carries a minted world id.** Today `identity_of` hashes the whole
  base set at boot. That is right for E3's question and wrong for S4's: a squash
  rewrites the bytes without changing the world, and every client would refetch a
  facet nothing moved in. So the id is **minted at import** — as the same hash of
  the same bytes, so nothing about E3 changes — written into the header, and
  **carried** by every later rewrite of the file.

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
`write`/`read`, is 22.4 MiB on Felucca, reports the same `WorldId` before and
after a rewrite of its own bytes, and a version 1 file loads to a named error
rather than a plausible world.

## S2 — a product is keyed by the chunk it was built from

**Goal.** A publish invalidates what it moved, and nothing else.

Needs S1. Today every bake and cache that has a revision at all keys on the
*facet's* revision, which one publish moves for all 7,168 chunks at once.
[`radar.md` §10.2](../radar.md) states the debt from the reader's side —
*"a chunk whose content did not change should keep its identity across a facet's
revision"* — and names the shard that gives the path a production writer as the
one who owes it. That is us, as of C's second half.

- `MapSnapshot` holds the per-chunk hashes it was loaded with — 7,168 × 8 B =
  **57 KiB** — and `publish` rehashes **only** the chunks
  [`Patch::touched_chunks`](../../../crates/common/map/src/patch.rs) names.
  `take_chunks` on the client does the same with the chunks it was handed.
- A reader's key stops being `(facet, chunk, facet revision)` and becomes
  `(facet, chunk, chunk hash)`. The radar cache
  ([`client_today.md`](client_today.md) finding 4) and the two bakes that already
  carry a `MapRevision` — the navigation bake and the building flood — are the
  three callers; the occluder measurements, keyed to file mtimes, are the fourth
  and the one with no revision at all today.
- **`invalidate_tile` stays test-only**, and this is the node that makes that
  final rather than pending: a chunk is sixteen thousand tiles, a publish names
  chunks, and a per-tile invalidation would be a second vocabulary for the same
  fact.

**Done when** a publish that touched one chunk leaves the products of the other
7,167 valid — asserted on the radar cache directly, and on a bake by rebuilding
it after a publish and showing that the untouched chunks' entries were reused.

## S3 — a block is replaced where it stands

**Goal.** A publish costs what it moved, on both ends.

Independent of S1 and S2; the most visible of the six, because it is the one a
person waits for. Two artefacts, one cause: the span index and `WorldMap`'s
statics are both facet-wide packed runs addressed by a prefix sum, and a prefix
sum *is* the ordering, so re-laying one block in place moves every run after it.

- **The spans** are [`navigation_spans.md`'s N8](../navigation_spans.md#n8--the-bake-follows-a-patch),
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
  the neighbouring regions whose answer *crossed* into the changed chunk. Until
  this lands, `mapedit::commit` drops the router on every publish, which is
  correct and is not free.

**Done when** a one-chunk publish is O(the chunk) at both ends, measured against
today's numbers by the test that produced them
([`publish_cost.rs`](../../../crates/common/movement/tests/publish_cost.rs)
extended to the client's `apply`), and `mapedit::commit` rebuilds the coarse
graph instead of dropping it.

## S4 — the log is folded

**Goal.** A shard that has been edited for a year boots like one that has not.

Needs S1's minted world id. [`basemap::load`](../../../crates/common/basemap/src/lib.rs)
replays every committed patch on every boot, on the shard and in every offline
bake. [`mechanics.md`](mechanics.md) already calls a full rebuild *"an explicit
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
thing [`overview.md`](overview.md)'s promise names that no direction was ever
written for. A base set replaces `map*.mul` and `statics*.mul`. It does not
replace:

- **`tiledata.mul`** — what a tile *is*: walkable, a surface, a wall, its name.
  It is a table of 16,384 land entries and 65,536 static ones, it is read by
  both ends, and the movement rules are meaningless without it. Same shape of
  work as B: our own file, an importer from the install, and the shard resolving
  it beside the base set.
- **The multis** — a house's component list, which
  [`customisation.md`](../../customisation.md) is already unhappy with for its
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

S1 and S2 are one thought split at the file's edge, and everything about
invalidation is downstream of them. S4 needs S1's world id. S5 and S6 are
independent of all of it.

**S3 may go first**, and should if the editor
([`../editor.md`](../editor.md)) lands before it: a brush is a stream of
publishes, and 115 ms on the tick and another 115 on the window is what a person
feels as the tool being unusable. Nothing in S1 or S2 gets harder for having
waited.
