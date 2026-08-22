# How a changeable map works

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

## Chunks

The map is cut into fixed blocks because nobody reloads a facet to move a rock:
a chunk is the unit of loading, caching, invalidation and transfer.

We already store the world close to this shape.
[`Map`](../../../crates/common/map/src/map.rs#L75) holds land block-ordered and
statics per block, each block sorted by the tile its items stand on. A chunk is
that, with an identity and a revision on it.

Two things a chunk must settle, both with a right answer we do not have yet:

- **Size.** UO's own block is 8×8; the first draft guessed 64×64. This is a
  measurement, not an opinion: size of a full base set for Felucca, average
  chunk with its statics, the working set around one screen, and how many
  chunks one editor brush touches. **A house is the sharpest input to it.**
  The shipped castle is 3,667 components over 31×32 tiles and puts **339 into
  a single 8×8 block** — nineteen times Felucca's median block of 18, and near
  its worst natural block of 467. At 8×8 that castle is sixteen chunks and
  moving one wall touches one small chunk; at 64×64 it sits inside one, and
  moving that wall rewrites and retransmits all 4,096 tiles around it. The
  numbers are in [`client_today.md`](client_today.md).
- **Who owns a static that overhangs a border.** A wall anchored in one chunk
  is drawn, walked and lit from the neighbour too. Ownership by anchor tile is
  the obvious rule, and it forces the second half: a reader that needs an area
  pins every chunk the area touches and reads owners. Copying a static into the
  neighbour would make removal and hashing ambiguous, which is the argument
  against it — see [`occluders.md`](../../occluders.md) and
  [`footprints.md`](../../footprints.md), which already reason about art footprints
  that cross tiles.

## Patches

A patch is what an editor commits. To be revertible rather than merely applied
it carries: the parent revision it was made against, an ordered list of
operations, who made it and when, and which chunks it touches.

The operation set can start very small — set the land of a tile, add a static,
remove a static — because editor brushes (raise, flatten, smooth, stamp) are
*editor* commands that compile down to those before publishing. That keeps the
diff explainable and the undo exact, and keeps a brush algorithm out of the
world's history.

Two constraints that are not stylistic:

- **A static needs a stable identity.** Two identical rocks can stand on one
  tile at one height. Addressing a static by coordinates and graphic cannot
  tell them apart, so "remove *that* rock" is not expressible. Today a static
  is a position in a block's vector
  ([`StaticItem`](../../../crates/common/map/src/map.rs#L43)), which is not an
  identity that survives an edit.
- **A patch applies to a parent, and conflicts are refused.** If the world
  moved under an unpublished edit, the editor gets a conflict and makes a new
  patch on the new parent. Silent last-write-wins on terrain would let one
  operator's hillside quietly eat another's.

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
| Chunk size | Measurement on Felucca: base set size, per-chunk statics, screen working set, brush blast radius — with a castle's 339-per-block as the density case |
| Whether a house is a patch to the world or stays an entity overlay | The densest case is decided by this and nothing else: a castle is 3,667 statics, so if a house is a patch, placing one is a bulk insert into the base. An overlay read alongside the base keeps the base immutable, which is what a flat per-chunk layout wants. See [`housing.md`](../../housing.md) and [`customisation.md`](../../customisation.md) |
| Whether the address needs a `map_id` above [`Facet`](../../../crates/common/protocol/src/world.rs#L1252) | Whether we ever run two worlds whose facet numbers collide |
| Land height per tile (UO's model) or per corner | Whichever keeps movement and rendering identical to today until we *mean* to change the geometry |
| Our own material/asset ids vs UO graphic numbers | Whether the first importer is the only importer |
| Where a per-shard asset pack comes from and what may be redistributed | A licensing answer, not a technical one |
| Which validation blocks a publish | Technical validity is mandatory; design rules (reachability, smoothness) are a separate list |
