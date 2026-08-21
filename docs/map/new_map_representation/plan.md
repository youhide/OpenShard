# The map: the work, and where it touches the code

The plan behind [`overview.md`](overview.md), with the mechanics in
[`mechanics.md`](mechanics.md). Six directions. The first one is not a feature — it is putting the map we already have behind one door,
and everything after it is cheap only if it lands first.

## Who reads the world today

Nothing here is wrong; it is simply six readers with no common owner.

| Reader | Where | What it holds |
|---|---|---|
| Step check, LoS, spawn heights | [`Terrain`](../../../crates/common/movement/src/walk.rs#L43), implemented by [`MapTerrain`](../../../crates/common/movement/src/terrain.rs#L61) | A `Map` and a `TileData`, owned or borrowed |
| The live step | [`LiveTerrain`](../../../crates/server/state/src/obstruct.rs#L140) over [`Obstructions`](../../../crates/server/state/src/obstruct.rs#L58) | Static terrain plus doors, items, boats |
| Long routes | [`NavigationGraph`](../../../crates/common/movement/src/navigation.rs#L28), baked by [`bake.rs`](../../../crates/common/movement/src/bake.rs#L120) | 32×32 regions over one facet, stamped by input files |
| The renderer, cutaway, the building flood | [`BuildingMap`](../../../crates/client/render/src/interiors.rs#L1025), [`occlusion/bake.rs`](../../../crates/client/render/src/occlusion/bake.rs#L400) | Its own walk of the same `Map` |
| The client, everything | [`Resources`](../../../crates/client/app/src/resources.rs#L35), `map: Arc<Map>` | The facet it loaded itself at startup |
| The shard, per facet | [`FacetState`](../../../crates/server/state/src/runtime.rs#L377) | `terrain`, `coarse`, `obstructions`, `boats`, `regions`, `banks` |

Both ends load the same install separately —
[`boot.rs:618`](../../../crates/server/server/src/boot.rs#L618) and
[`lib.rs:461`](../../../crates/client/app/src/lib.rs#L461) — and the world is
whatever those files said.

## A0 — the cell array becomes a type that owns the order

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
| [`from_blocks`](../../../crates/common/uofiles/src/map.rs#L332) | the triple loop that defines the order |
| [`load_statics`](../../../crates/common/uofiles/src/map.rs#L448) | the **inverse** — `block / blocks_down`, `block % blocks_down` — to recover a block's world origin |
| [`cell_index`](../../../crates/common/uofiles/src/map.rs#L502) | `(x / 8) * blocks_down + (y / 8)`, then the cell within |
| [`statics_in_row`](../../../crates/common/uofiles/src/map.rs#L611) | `column * rows + y / 8` |
| [`statics_in_block`](../../../crates/common/uofiles/src/map.rs#L645) and [`block_index`](../../../crates/common/uofiles/src/map.rs#L654) | the same formula twice more, in two functions that do not call each other |

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
  `block_order_is_column_major` is a test of the newtype rather than of `Map`.

Nothing outside `uofiles` changes: the linear formula never escaped this
module — readers elsewhere only ever divide or multiply a coordinate by
`BLOCK_SIZE`, which is order-independent. That is what makes this cheap, and
it is why it goes before A rather than inside it.

## A — one world, one door

**Goal.** A named, revisioned snapshot that every reader above takes a handle
to, so that later "the world changed" is one event with one meaning.

- Introduce the snapshot as the thing a tick and a frame pin, alongside
  [`FacetState`](../../../crates/server/state/src/runtime.rs#L377) on the server and
  [`Resources::map`](../../../crates/client/app/src/resources.rs#L35) on the client.
  It starts as a wrapper over today's `Map` with a revision on it and no patch
  machinery at all.
- Keep [`Terrain`](../../../crates/common/movement/src/walk.rs#L43) as the query face
  for movement. It already is one, and it already has the two implementations
  that matter. Nothing about a step should learn what a patch is.
- The readers that walk the `Map` directly rather than through `Terrain` —
  the renderer, [`BuildingMap`](../../../crates/client/render/src/interiors.rs#L1025),
  the occluder bake, the minimap — take the snapshot instead of a bare `Map`.
- **Done when** the map can only be reached through a snapshot handle, and
  every bake records which revision it was built from.

No format work, no network work, no editor. This direction is worth landing on
its own even if everything below slipped.

## B — our own chunk format, and a UO importer

**Goal.** A world that exists without a UO install.

- A new crate under `crates/common/` (both ends need it, so the dependency
  invariant puts it there, not under `server/` or `client/`). It owns the chunk
  types, the canonical encoding, bounds checking, hashing. It knows nothing of
  sockets, ECS or renderers.
- Entities: `ChunkKey` (facet, chunk x, chunk y — plus a `map_id` only if the
  question in the mechanics table answers yes), `Chunk` (dense land arrays,
  statics grouped by tile), `StaticId`, `Revision`.
- A CLI that bakes a facet out of a UO install into a base set, reusing
  [`Map::load_facet`](../../../crates/common/uofiles/src/map.rs#L262) as the reader.
- **Done when** an imported facet round-trips byte-identically, and a decoded
  chunk answers the same land and statics as
  [`Map::land`](../../../crates/common/uofiles/src/map.rs#L509) and
  [`Map::statics_at`](../../../crates/common/uofiles/src/map.rs#L568) for sampled
  tiles across Felucca.

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
  block. [`load_statics`](../../../crates/common/uofiles/src/map.rs#L455) expands
  them on purpose ("a world coordinate is more use to everyone downstream"),
  which costs four bytes an item and is the difference between a 10-byte record
  and a 4-byte one — 6.4 items per cache line against 16.

A CSR pair — one `Vec<StaticItem>` and a `Vec<u32>` of per-block offsets — is
2 allocations against today's 120,745, and at 4 bytes an item takes the whole
statics layer from 38.2 MiB to about 13.5 MiB. At that density a block's
statics are 72 bytes, one or two cache lines, and the two binary searches
[`statics_at`](../../../crates/common/uofiles/src/map.rs#L568) exists to avoid a
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

- Client-side disk cache keyed by chunk and revision; on connect it offers what
  it holds and receives what is missing or stale; on a publish it is told which
  chunks died.
- The pipe is chosen here and not before — the `0xBF` envelope
  ([`extended.rs:27`](../../../crates/common/protocol/src/extended.rs#L27)) or a
  second stream over [`Dial`](../../../crates/client/net/src/transport.rs#L100).
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

## Order

A0, then A, then B, then C, with D following C closely because a stale bake is
how a changed world lies to a player. A0 is internal to `uofiles` and touches
no reader, so it can land at any time and everything after it is written
against one spelling of the order rather than five. E and F come last and can be reordered against
each other. Every step ends with a world that runs; none of them is "replace
the runtime with streaming first and make it correct afterwards".

## First useful slice

The shard and our client both run on a base set imported from facet 0, with no
UO map or static files on the client machine; they agree on sampled land and
statics; one land patch and one static patch publish, survive a restart, change
what the server allows, and reach the connected client. Re-importing the same
facet and re-applying the same patches produces byte-identical chunks.
