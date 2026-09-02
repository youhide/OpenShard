# How a changeable map works

> **Status: live**, with every open question now closed — the last of them, whether
> a house is a patch or a layer, in [`map_rebuild.md`](../map_rebuild.md)'s R3.

The mechanics behind [`overview.md`](overview.md). This document says what
has to be true and what the real choices are; where a decision is not made yet
it says so and names what would settle it. [`plan.md`](plan.md) is the work
itself.

## Three words, kept apart

- **Base** — the world as imported, immutable. One bake of a UO facet, or one
  generated world. It never changes; that is what makes a change describable.
- **Patch** — one committed unit of change against a known parent revision.
  Durable, ordered, attributable, revertible.
- **Snapshot** — what everything reads: `base + the patches in force`, at one
  revision, immutable while anybody is looking at it.

```text
importer (UO facet, editor, generator)
    -> base            immutable
    -> patches         ordered, durable, attributable
    -> snapshot        immutable, revisioned, what every reader sees
```

A reader never applies patches itself and never sees a half-applied world. The
tick takes a snapshot handle at its start and every step, sight line and route
inside that tick answers from that one revision. A frame does the same. A new
revision becomes visible between ticks, not during one.

## A fourth thing, which is not a fourth word

The three above are all about the *base*. A shard also has a barrel on the
ground, a shut door and a ship's deck, and none of them is a patch. They are the
**live layer**, and *"why not just keep those in the map too"* is asked often
enough — and answered in three different files — to be worth answering here once.

**They are in the map.** [`World`](../../../crates/common/map/src/world.rs) is
`base + live` in one value, which is what
[`map_rebuild.md`](../map_rebuild.md)'s *"the map, in one type"* means: the
ground, the statics, and what the live world has laid over them. A reader takes
one world, not a map and an overlay it remembered to carry together — two
arguments that could be handed different facets without anything noticing, which
is what that type was made to stop.

**What they are not in is the base**, and six things follow from that. The first
five are ours to weigh; the sixth is not.

- **A static has no identity and an item needs one.** A `StaticItem` is a
  graphic, an x, a y, a z and a hue — there is nothing to address. An item is
  picked up by serial (`0x07`), put down by serial (`0x08`) and forgotten by
  serial (`0x1D`).
- **Every drop would be a revision**, and a revision is history: durable,
  attributable, and what a client fetches by. A barrel dropped and picked up five
  seconds later would be two entries in the `.ospatch` log forever, and no
  client's kept world would ever be current.
- **Every drop would be a rebake.** Moving the base invalidates the span index,
  the composited blocks, the radar and the coarse graph — 115 ms and 11.6 s of it
  measured. A dropped barrel costs one hash-map entry today *because* the bake is
  a projection of the two lower layers and cannot see the live one.
- **The layout is a run, not a set.** A facet's statics are one sorted vector
  with a prefix sum of offsets; inserting one item re-offsets the tail.
- **The base is one thing everyone holds, whole. Items are told by interest** —
  eighteen tiles, forgotten on the way out — and carry what a static cannot be
  given: an owner, a container, a weight, a decay timer. A static can only lie on
  a tile; it cannot lie in a backpack.
- **🚩 And the base is ours, which the stock client is not.** A 2D client and
  ClassicUO draw statics from their own `statics0.mul` and never see a byte of
  ours — [`map_rebuild.md`](../map_rebuild.md) puts it plainly, *"the classic 2D
  client is out of it by design: it reads its own files and does not see our
  changes"*, and `.addstatic`'s own doc says the same from the other end. So the
  rule underneath all of the above is one line:

> **Anything that must be visible to a stock client has to be an entity on the
> wire.** The base is what only the shard and a client of ours can see.

**Which is why the decoration is entities**, and it is worth saying because it
looks like a mistake and is not. `data/deco.json` is the largest dataset in the
tree — 18,832 statics, 5,598 containers, 638 doors for one facet — and the
18,832 never move, never decay and cannot be picked up. They are statics in
everything but storage, and they are entities anyway, costing serials, registry
rows, interest sweeps and `0x1A`s, for exactly the reason above.

Committing them into the base is a real option, newly possible — before era S
there was nothing to commit *into* — and it already has a shape in the plan:
[direction F](plan.md#f--the-editor)'s *"committing a house into the base as its
one-way operation"* is the same gesture. What it is not is an optimisation. It
trades away every stock client's view of that furniture, so it is a decision
about **who plays here**, and it belongs to an operator rather than to us.

**One note on where the seam sits**, since the tempting move is to push the live
layer one level further down, into `WorldMap` beside the land and the statics.
The grouping is by *clock*, not by resemblance: land and statics move together —
one patch moves both, one revision stamps both, one chunk carries both — and the
live layer moves when a door swings. Putting it inside `WorldMap` would also put
it inside `MapSnapshot`, and then a bake could reach a door: today it cannot,
because it is handed a snapshot with no field to reach one through. That
invariant is a borrow rather than a rule somebody has to remember, and it is the
thing the current seam buys.

## Chunks

The map is cut into fixed blocks because nobody reloads a facet to move a rock:
a chunk is the unit of loading, caching, invalidation and transfer.

We already store the world close to this shape.
[`WorldMap`](../../../crates/common/map/src/map.rs#L75) holds land block-ordered
and statics per block, each block sorted by the tile its items stand on. A chunk
is that, with an identity and a revision on it.

Two things a chunk had to settle, **both now settled and both built** — see
[`chunk.rs`](../../../crates/common/map/src/chunk.rs), whose module header is the
decision record:

- **Size: 64×64 tiles**, which is eight map blocks square, so no chunk boundary
  ever splits the block the statics are indexed by. It was decided by
  measurement, as this document asked. The base set's *total* size turned out
  to be flat across every candidate — 137 to 151 MiB at 8, 16, 32, 64 and 128
  tiles — so size was not the argument, and what was left was overhead against
  blast radius. UO's own 8×8 loses on overhead and not narrowly: a manifest
  with a hash per chunk is 17.5 MiB, a ninth of the set it indexes, and one
  widest-zoom rectangle pins 625 chunks against 64×64's sixteen. The argument
  the other way is that one wall then rewrites 18 KiB — and
  [`overview.md`](overview.md) refuses that argument by name, since thrift is
  not a goal. Sixty-four is also the grid every artefact derived from terrain
  is already keyed to, so direction D's invalidation is one-to-one. **A house
  is still the sharpest input**: the shipped castle is 3,667 components over
  31×32 tiles and puts 339 into a single 8×8 block, and it stays an argument
  that a flat base array must never be inserted into rather than an argument
  about the size. The numbers are in [`client_today.md`](client_today.md).
- **A static that overhangs a border belongs to the chunk its anchor tile is
  in.** Ownership by anchor was the obvious rule and is now the built one, held
  against a real facet by `a_static_belongs_to_the_chunk_its_anchor_is_in`. It
  forces the second half, which is unchanged: a reader that needs an area pins
  every chunk the area touches and reads owners. Copying a static into the
  neighbour would make removal and hashing ambiguous — see
  [`occluders.md`](../../render/design_occluders.md) and
  [`footprints.md`](../../render/design_footprints.md), which already reason about art
  footprints that cross tiles.

## Patches

A patch is what an editor commits. To be revertible rather than merely applied
it carries: the parent revision it was made against, an ordered list of
operations, who made it and when, and which chunks it touches. **Built** — see
[`patch.rs`](../../../crates/common/map/src/patch.rs), whose module header is
the decision record, and [`patches.rs`](../../../crates/common/basemap/src/patches.rs)
for the log the committed ones live in.

The operation set can start very small — set the land of a tile, add a static,
remove a static — because editor brushes (raise, flatten, smooth, stamp) are
*editor* commands that compile down to those before publishing. That keeps the
diff explainable and the undo exact, and keeps a brush algorithm out of the
world's history. It is those three and nothing else.

Two constraints that are not stylistic, both now settled:

- **A static needs a stable identity.** Two identical rocks can stand on one
  tile at one height. Addressing a static by coordinates and graphic cannot
  tell them apart, so "remove *that* rock" is not expressible. Today a static
  is a position in a block's vector
  ([`StaticItem`](../../../crates/common/map/src/map.rs#L43)), which is not an
  identity that survives an edit. **Closed: the ordinal on its tile, read
  against the patch's parent revision.** What makes it stable is not a field —
  it is the constraint below. A patch is only ever applied to the one world it
  was made against, so "the second static standing here" names exactly one
  thing; and the op carries the item it is taking away, so a patch read against
  the wrong world is refused rather than applied to whatever is in that
  position. That is why `StaticId` needs no bytes in the base format.
- **A patch applies to a parent, and conflicts are refused.** If the world
  moved under an unpublished edit, the editor gets a conflict and makes a new
  patch on the new parent. Silent last-write-wins on terrain would let one
  operator's hillside quietly eat another's. **Closed, and it is the whole
  conflict model**: `MapSnapshot::publish` takes `&mut self` and refuses a
  parent that is not the revision it is holding. The `&mut` is also what makes
  a publish atomic — a reader borrows a `&WorldMap` out of the snapshot, so the
  borrow checker is what stops one from ever seeing half a change, rather than
  a rule about ticks that somebody has to remember.

Two more that the doing settled:

- **Ops are a sequence, not a set**, and each one sees what the ones before it
  did. It is the only reading under which "remove both of these" is one patch.
- **All of a patch or none of it.** An op that cannot apply aborts the patch and
  the ops already applied are undone — and the undo is free, because applying an
  op returns its own inverse. That inverse is what a revert will be built out
  of, which is how "revert is a new patch" stays true without a second apply
  path.

## Caches, and what goes stale

Almost nothing reads the raw world. Between the map and the answer sit baked
things: the navigation graph, the building flood, occluder geometry, minimap
rasters, art measurements. Every one of them is derived data over terrain, and
every one is currently keyed by the *files* it was baked from.

The rule that replaces that: **derived data is keyed by the source revision**,
and a change invalidates the affected chunks plus the neighbours whose answer
crossed into them. A patch touching a thousand chunks costs a thousand chunks
of rebuild, never a facet. A full rebuild is an explicit operation — a new
base, a squash, an import — and never a side effect of publishing an edit.

[`minimap_lod_plan.md`](../minimap_lod_plan.md) already states this contract from
the consumer's side: one cache key of facet, chunk, LOD and source revision.

## Getting it to the client

Whole chunks, as self-contained verifiable blobs. Not a stream of operations —
a client that loses the connection mid-stream would be left in a world that
never existed. It keeps a disk cache, offers what it has, and takes back what
is missing or stale. That negotiation is an optimisation: a matching client
chunk speeds up drawing and local path planning, and never authorises a step.

Which pipe carries it is deliberately a late decision, and the format must not
know:

- **Inside the classic protocol.** The `0xBF` extended envelope already exists
  on both sides ([`extended.rs:27`](../../../crates/common/protocol/src/extended.rs#L27))
  and would carry our own encoding as an opaque body.
- **Beside it.** A second connection for our own traffic, which our client's
  transport already abstracts over
  ([`Dial`](../../../crates/client/net/src/transport.rs#L100)), so a second stream
  costs no protocol violence.

The criterion to pick: whether a chunk fits an envelope the classic stream can
carry without pain, and whether anything else we want (editor authority, asset
packs) wants the same pipe. Whatever wins, the map codec stays a library that
has never heard of sockets.

Art travels separately, addressed by content. A chunk names asset ids; a
missing asset draws an explicit placeholder rather than falling back to
whatever graphic the player's own install has under that number.

## Open, with what would close it

| Question | What settles it |
|---|---|
| ~~Chunk size~~ | **Closed: 64×64 tiles.** Measured on Felucca; the reasoning is above and in [`chunk.rs`](../../../crates/common/map/src/chunk.rs) |
| ~~Whether the address needs a `map_id` above [`Facet`](../../../crates/common/protocol/src/world.rs#L1252)~~ | **Closed: no.** It was conditional on ever running two worlds whose facet numbers collide, and we do not. The encoding carries a version byte, which is what the door back in looks like |
| ~~Who owns a static that overhangs a border~~ | **Closed: the chunk its anchor tile is in**, and nothing is copied into the neighbour |
| ~~Whether a house is a patch to the world or stays an entity overlay~~ | **Closed: an entity overlay — the live layer**, on the density it was always going to be decided by: a castle is 3,667 statics, so a house as a patch is a bulk insert into an immutable base. What that left owing was the half nobody had built — a house's floors were not standable, because `Cover::of_static` only emitted a cover for a *blocking* tile. **Built 2026-08-23**, as R3: see [`map_rebuild.md`](../map_rebuild.md#r3--a-house-is-a-layer-and-it-has-floors). Committing a house *into* the base stays an editor operation and only that, as [direction F](plan.md#f--the-editor) says |
| Land height per tile (UO's model) or per corner | Whichever keeps movement and rendering identical to today until we *mean* to change the geometry |
| Our own material/asset ids vs UO graphic numbers | Whether the first importer is the only importer |
| Where a per-shard asset pack comes from and what may be redistributed | A licensing answer, not a technical one |
| Which validation blocks a publish | Technical validity is mandatory; design rules (reachability, smoothness) are a separate list |
