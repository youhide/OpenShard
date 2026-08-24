# The map: the work, and where it touches the code

> **Status: era S.** A0, A and B are built and C's first half with them; D, E, F
> and the rest of C resume after eras R and P. The **Order** section below is
> superseded — see [`map_rebuild.md`](../map_rebuild.md).

The plan behind [`overview.md`](overview.md), with the mechanics in
[`mechanics.md`](mechanics.md). Seven directions and one deferred. The first two
are not features — they are putting the map we already have behind one door, and
everything after them is cheap only if they land first. They have their own
executable plan: [`snapshot.md`](snapshot.md), which grew a third phase in the
doing: **the world is one type in `openshard-map`, and UO's files are an
importer into it.** Every direction below inherits that, and B most of all.

## Who reads the world today

Nothing here is wrong; it is simply six readers with no common owner.

| Reader | Where | What it holds |
|---|---|---|
| Step check, LoS, spawn heights | [`Terrain`](../../../crates/common/movement/src/walk.rs#L43), implemented by [`MapTerrain`](../../../crates/common/movement/src/terrain.rs#L61) | A `WorldMap` and a `TileData`, owned or borrowed |
| The live step | [`LiveTerrain`](../../../crates/server/state/src/obstruct.rs#L140) over [`Obstructions`](../../../crates/server/state/src/obstruct.rs#L58) | Static terrain plus doors, items, boats |
| Long routes | [`NavigationGraph`](../../../crates/common/movement/src/navigation.rs#L28), baked by [`bake.rs`](../../../crates/common/movement/src/bake.rs#L120) | 32×32 regions over one facet, stamped by input files |
| The renderer, cutaway, the building flood | [`BuildingMap`](../../../crates/client/render/src/interiors.rs#L1025), [`occlusion/bake.rs`](../../../crates/client/render/src/occlusion/bake.rs#L400) | Its own walk of the same `WorldMap` |
| The client, everything | [`Resources`](../../../crates/client/app/src/resources.rs#L35), `map: Arc<WorldMap>` | The facet it loaded itself at startup |
| The shard, per facet | [`FacetState`](../../../crates/server/state/src/runtime.rs#L377) | `terrain`, `coarse`, `obstructions`, `boats`, `regions`, `banks` |

Both ends load the same install separately —
[`boot.rs:618`](../../../crates/server/server/src/boot.rs#L618) and
[`lib.rs:461`](../../../crates/client/app/src/lib.rs#L461) — and the world is
whatever those files said.

## A0 — the cell array becomes a type that owns the order

> **Built** — `crates/common/map/src/grid.rs`. Executed on its own, as
> [`snapshot.md`](snapshot.md)'s phase 1; what it left behind is that
> document's [backlog](snapshot.md#what-phase-1-left-behind).

**Goal.** The block order stops being arithmetic five functions each write out,
and becomes one newtype whose whole job is that arithmetic.

The order is column-major in blocks, row-major in cells within a block, and
[`map.rs`'s own header](../../../crates/common/uofiles/src/map.rs#L1) records
why that is dangerous: got backwards, the file still parses, every block is
still 196 bytes, every read lands somewhere plausible, and you find out when a
player walks into an ocean that should be a coastline. It is currently spelled
in five places inside one file:

| Where | What it writes |
|---|---|
| [`from_blocks`](../../../crates/common/map/src/grid.rs#L222) | the triple loop that defines the order |
| [`load_statics`](../../../crates/common/uofiles/src/map.rs#L252) | the **inverse** — `block / blocks_down`, `block % blocks_down` — to recover a block's world origin |
| [`cell_index`](../../../crates/common/map/src/grid.rs#L372) | `(x / 8) * blocks_down + (y / 8)`, then the cell within |
| [`statics_in_row`](../../../crates/common/map/src/map.rs#L331) | `column * rows + y / 8` |
| [`statics_in_block`](../../../crates/common/map/src/map.rs#L368) and [`block_index`](../../../crates/common/map/src/map.rs#L388) | the same formula twice more, in two functions that do not call each other |

- A `LandGrid` newtype over `Vec<LandCell>` holding `width`, `height` and the
  cells, and owning **every** conversion: tile to cell index, tile to block,
  block to linear index, and the inverse a loader needs — the block's world
  origin, which is the one currently open-coded backwards.
- Index domains get their own types rather than travelling as `usize`:
  `BlockCoord`, `BlockIndex`, `CellIndex`. `BlockCoord` is the value that
  already exists three times under three names —
  [`interiors::BlockId`](../../../crates/client/render/src/interiors.rs#L18),
  [`composite::MapBlock`](../../../crates/client/render/src/composite.rs#L56)
  and radar's `RadarChunkCoord` — so this direction decides whether they are
  one type or stay deliberately separate, and says which.
- **Transitions belong to it too.** Stepping to the next tile is `+1` cell
  inside a block and `+blocks_down` blocks across its eastern edge; `+8` cells
  inside a block and `+1` block across its southern one. A rectangle walk that
  asks the grid for its next cell stops re-deriving an index per tile, and —
  the reason this matters beyond tidiness — it makes the walk order a property
  of one iterator rather than of every caller's loop nesting. See the note
  under B on why that order is currently observable in the picture.
- The coupling that is load-bearing and implicit today gets stated: `statics`
  is indexed by **the same** `BlockIndex` as `cells`. Nothing enforces it now.
- **Done when** `blocks_down`, `* blocks_down +` and `% BLOCK_SIZE` appear
  nowhere in `map.rs` outside the newtype, and
  `block_order_is_column_major` is a test of the newtype rather than of
  `WorldMap`.

Nothing outside `uofiles` changes: the linear formula never escaped this
module — readers elsewhere only ever divide or multiply a coordinate by
`BLOCK_SIZE`, which is order-independent. That is what makes this cheap, and
it is why it goes before A rather than inside it.

## A — one world, one door

> Being executed first, on its own: [`snapshot.md`](snapshot.md).

**Goal.** A named, revisioned snapshot that every reader above takes a handle
to, so that later "the world changed" is one event with one meaning.

- Introduce the snapshot as the thing a tick and a frame pin, alongside
  [`FacetState`](../../../crates/server/state/src/runtime.rs#L377) on the server and
  [`Resources::map`](../../../crates/client/app/src/resources.rs#L35) on the client.
  It starts as a wrapper over today's `WorldMap` with a revision on it and no
  patch machinery at all.
- Keep [`Terrain`](../../../crates/common/movement/src/walk.rs#L43) as the query face
  for movement. It already is one, and it already has the two implementations
  that matter. Nothing about a step should learn what a patch is.
- The readers that walk the `WorldMap` directly rather than through `Terrain` —
  the renderer, [`BuildingMap`](../../../crates/client/render/src/interiors.rs#L1025),
  the occluder bake, the minimap — take the snapshot instead of a bare
  `WorldMap`.
- **Done when** the map can only be reached through a snapshot handle, and
  every bake records which revision it was built from.

No format work, no network work, no editor. This direction is worth landing on
its own even if everything below slipped.

## B — our own chunk format, and a UO importer

**Goal.** A world that exists without a UO install.

- The crate exists: [`openshard-map`](../../../crates/common/map/src/lib.rs),
  under `crates/common/` because both ends need it. It already holds the world
  and its snapshot, and has never opened a file. (It also depended on
  `openshard-protocol` and nothing else when this was written; that half is
  struck — see [`map_rebuild.md`](../map_rebuild.md)'s R1.) B fills it with the
  chunk types, the canonical encoding,
  bounds checking and hashing. It knows nothing of sockets, ECS or renderers.
- Entities: `ChunkKey` (facet, chunk x, chunk y — plus a `map_id` only if the
  question in the mechanics table answers yes), `Chunk` (dense land arrays,
  statics grouped by tile), `StaticId`, `Revision`.
- **A chunk reader is a second importer, not a second world.** It builds the
  same [`WorldMap`](../../../crates/common/map/src/map.rs#L75) the `.mul` reader
  does, through the same
  [`from_parts`](../../../crates/common/map/src/map.rs#L191) — which is what
  makes the round-trip below an assertion about *bytes* rather than about two
  parallel representations that happen to agree.
- A CLI that bakes a facet out of a UO install into a base set, reusing
  [`read_facet`](../../../crates/common/uofiles/src/map.rs#L185) as the reader.
- **Done when** an imported facet round-trips byte-identically, and a decoded
  chunk answers the same land and statics as
  [`WorldMap::land`](../../../crates/common/map/src/map.rs#L224) and
  [`WorldMap::statics_at`](../../../crates/common/map/src/map.rs#L305) for
  sampled tiles across Felucca.

Then the server reads the base set instead of the install, and existing
movement, LoS and harvesting tests pass unchanged over the new source. That is
the real acceptance test for B, and it needs no patches to run.

### What Felucca measures, before the layout is chosen

"Statics grouped by tile" above is the shape today's `Vec<Vec<StaticItem>>`
already has, and it is the one thing here that should not be inherited by
default. Measured off the shipped `statics0.mul`:

| | |
|---|---|
| statics | 2,906,871 across 120,744 non-empty blocks |
| per block | median **18**, mean 24, p99 122, max 467 |
| `hue != 0` | **27,743 — 0.95%** |

Three consequences, none of them yet decided:

- **A block is tiny.** At the median it is 18 items. Whatever a chunk holds,
  the per-tile index inside it is an index over about two dozen things.
- **`hue` is dead weight in the common case** — two bytes on 99% of items that
  do not use them.
- **`x` and `y` are stored absolute, and only three bits of each matter** in a
  block. [`load_statics`](../../../crates/common/uofiles/src/map.rs#L252) expands
  them on purpose ("a world coordinate is more use to everyone downstream"),
  which costs four bytes an item and is the difference between a 10-byte record
  and a 4-byte one — 6.4 items per cache line against 16.

A CSR pair — one `Vec<StaticItem>` and a `Vec<u32>` of per-block offsets — is
2 allocations against today's 120,745, and at 4 bytes an item takes the whole
statics layer from 38.2 MiB to about 13.5 MiB. At that density a block's
statics are 72 bytes, one or two cache lines, and the two binary searches
[`statics_at`](../../../crates/common/map/src/map.rs#L305) exists to avoid a
scan with are no longer obviously the cheaper answer. Two costs to weigh
against it: accessors hand back a value rather than a reference, since the
block origin is unpacked at the boundary; and an in-place `place_static`
becomes a tail shift, which wants a builder — and which C's overlay model says
should not be happening to a base array anyway.

**The walk order is observable, and that is a constraint on this.**
[`depth::Order`](../../../crates/client/render/src/depth.rs#L55) is
`{ tile: x + y, priority_z }`, so every tile on one anti-diagonal shares
`tile`; the pre-draw sort is stable, so for two statics on different tiles of
one diagonal at equal `priority_z` the last one walked is the last one drawn
and wins the `LessEqual` depth test. Transposing a rectangle walk to match
whatever order this direction picks is therefore a change to the picture, not
a free optimisation, until `Order` is made total across distinct tiles.

## C — patches, and the resolved snapshot

**Goal.** A change with an author, an order and an undo.

> **Built, except for the client.** The patch model, the log, the offline tool
> and the live publish are all in — see the two handoffs and
> [`mapedit`](../../../crates/server/world/src/mapedit.rs). What the "done"
> below still asks for is the last clause: a patch **visible to a connected
> client**, which is direction E's.
>
> One departure worth reading: the log is a file beside the base set rather than
> a table in `crates/server/persistence`, because the offline bake and the
> editor both have to resolve a world and neither can see a server crate.

- `Patch { parent, ops, author, touched }` and the smallest useful `PatchOp` —
  set land, add static, remove static. Persisted through
  `crates/server/persistence`.
- Resolving: apply in revision order to the touched chunks only, rehash, and
  publish a new snapshot atomically between ticks. A publish never rebuilds a
  facet.
- Revert is a new patch, not a rewritten history.
- **Done when** one `set land` and one `add static` survive a restart, change
  what the server allows a player to walk on, and are visible to a connected
  client.

## D — derived data keyed by revision

**Goal.** Nothing baked outlives the world it was baked from.

- Replace the file stamp in [`bake.rs`](../../../crates/common/movement/src/bake.rs#L22)
  with the source revision, and do the same for the building flood, the occluder
  bake, the minimap cache — see
  [`minimap_lod_plan.md`](../minimap_lod_plan.md), which already asks for exactly
  this key.
- First version may rebuild a whole facet's graph on a publish; the chunk keys
  and revisions must make a local rebuild possible later without a format
  change.
- **Done when** a publish makes every stale bake refuse itself rather than
  answer from the old world.

## E — to the client

**Goal.** Our client draws a world it was given, not one it found on disk.

> **Built, and it has an executable plan of its own:**
> [`to_the_client.md`](to_the_client.md) — five phases, the measurements the
> pipe was chosen off, and what each phase's "done" is. **All five are built**:
> the client's world is a parameter (E0), the wire carries a chunk (E1), a client
> with no map files takes the facet off it (E2), keeps what it was given (E3),
> and is told when the shard's own ground moves under it (E4). What is left is
> that plan's backlog.

- Client-side disk cache keyed by chunk and revision; on connect it offers what
  it holds and receives what is missing or stale; on a publish it is told which
  chunks died.
- The pipe is chosen here and not before — the `0xBF` envelope
  ([`extended.rs:27`](../../../crates/common/protocol/src/extended.rs#L27)) or a
  second stream over [`Dial`](../../../crates/client/net/src/transport.rs#L100).
  **Taken: the `0xBF` envelope**, in the `0xE000` range
  [`access.rs`](../../../crates/common/protocol/src/access.rs#L75) already
  reserved, with the chunk deflated first. The argument is a measurement — a
  deflated chunk of Felucca is at most 16,050 bytes and every one of them fits
  in a packet, which is what retired the case for a second stream.
- **Done when** our client starts with no UO map or statics files present and
  draws the shard's world, including a patch published while it was connected.

## F — the editor

**Goal.** Someone reshapes space and commits.

- Draft patch over a chosen parent, preview built by the same apply path the
  runtime uses — a preview that renders through a different code path is a
  preview of a different world.
- Brushes compile to canonical operations before publishing; publish is one
  transaction — validate, build the touched chunks, store, switch revision,
  invalidate.
- Authority and the conflict path are editor concerns, not map format ones.
- **Committing a house into the base is an editor operation, and only that.**
  A designer builds with the house tool and then publishes the result as
  terrain: the entity's components become base statics in the touched chunks
  and **the entity ceases to exist**. It is one-way, it happens at publish, and
  it is the only path by which a house ever becomes terrain. It is not a
  contradiction of the rule that a *live* house is never baked — that rule
  exists because a live house must be removable in one operation, and a
  committed one is no longer a house. What it needs from this direction: the
  same validate-build-store-switch transaction every other publish uses, and an
  answer to what happens to anything locked down inside it.

## G — residency and size, deferred on purpose

**Not scheduled, and not researched.** Recorded so that A0's newtype and B's
chunk format are shaped without closing the door on it. Today's format is the
one we want; this is about what it must not prevent.

- **The whole facet is resident.** `WorldMap` is about 150 MiB and every reader
  assumes it is all there. A world of chunks held lazily — fetched on approach,
  dropped behind — is the eventual shape, and the thing that makes it cheap
  later is that it stays **behind the same API**: A0's newtype and `Terrain` are
  the two doors, and neither should ever hand out a `&[LandCell]` spanning more
  than one chunk. *What would settle it:* the working set a real session
  touches, against the cost of a miss on the hot path.
- **`cells` should compress well, and nobody has checked.** A facet is largely
  ocean — one land id at one height over enormous runs — so the land layer is
  the obvious candidate for whole-chunk compression at rest, decompressed on
  residency rather than on access. *What would settle it:* the ratio on real
  Felucca chunks, and the decompression cost against the residency budget above.
  It is a *storage* question, not an access one: nothing should ever read a
  compressed cell.

Neither belongs in the first slice, and the reason to write them down now is
the same one this whole track has: a format chosen without knowing they are
coming is a format that will have to be reopened to get them.

## Order

> **Superseded past C.** A0, A, B and C's first half are built, and what is left
> of this track now runs *after* the two eras in
> [`map_rebuild.md`](../map_rebuild.md): the runtime map (R) and the search over
> it (P). D keyed to a revision of a layout R4 is still changing, and an editor
> previewing through an apply path R2 is still shaping, are both written twice if
> they go first. The order below is kept as the reasoning it was.

A0, then A, then B, then C, with D following C closely because a stale bake is
how a changed world lies to a player. A0 is internal to `uofiles` and touches
no reader, so it can land at any time and everything after it is written
against one spelling of the order rather than five. E and F come last and can be reordered against
each other. G is not in the order at all — it is a constraint on A0 and B, not
a step after F. Every step ends with a world that runs; none of them is "replace
the runtime with streaming first and make it correct afterwards".

## First useful slice

The shard and our client both run on a base set imported from facet 0, with no
UO map or static files on the client machine; they agree on sampled land and
statics; one land patch and one static patch publish, survive a restart, change
what the server allows, and reach the connected client. Re-importing the same
facet and re-applying the same patches produces byte-identical chunks.
