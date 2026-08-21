# The map: the work, and where it touches the code

The plan behind [`world_map.md`](world_map.md), with the mechanics in
[`world_map_mechanics.md`](world_map_mechanics.md). Six directions. The first
one is not a feature — it is putting the map we already have behind one door,
and everything after it is cheap only if it lands first.

## Who reads the world today

Nothing here is wrong; it is simply six readers with no common owner.

| Reader | Where | What it holds |
|---|---|---|
| Step check, LoS, spawn heights | [`Terrain`](../crates/common/movement/src/walk.rs#L43), implemented by [`MapTerrain`](../crates/common/movement/src/terrain.rs#L61) | A `Map` and a `TileData`, owned or borrowed |
| The live step | [`LiveTerrain`](../crates/server/state/src/obstruct.rs#L140) over [`Obstructions`](../crates/server/state/src/obstruct.rs#L58) | Static terrain plus doors, items, boats |
| Long routes | [`NavigationGraph`](../crates/common/movement/src/navigation.rs#L28), baked by [`bake.rs`](../crates/common/movement/src/bake.rs#L120) | 32×32 regions over one facet, stamped by input files |
| The renderer, cutaway, the building flood | [`BuildingMap`](../crates/client/render/src/interiors.rs#L1025), [`occlusion/bake.rs`](../crates/client/render/src/occlusion/bake.rs#L400) | Its own walk of the same `Map` |
| The client, everything | [`Resources`](../crates/client/app/src/resources.rs#L35), `map: Arc<Map>` | The facet it loaded itself at startup |
| The shard, per facet | [`FacetState`](../crates/server/state/src/runtime.rs#L377) | `terrain`, `coarse`, `obstructions`, `boats`, `regions`, `banks` |

Both ends load the same install separately —
[`boot.rs:618`](../crates/server/server/src/boot.rs#L618) and
[`lib.rs:461`](../crates/client/app/src/lib.rs#L461) — and the world is
whatever those files said.

## A — one world, one door

**Goal.** A named, revisioned snapshot that every reader above takes a handle
to, so that later "the world changed" is one event with one meaning.

- Introduce the snapshot as the thing a tick and a frame pin, alongside
  [`FacetState`](../crates/server/state/src/runtime.rs#L377) on the server and
  [`Resources::map`](../crates/client/app/src/resources.rs#L35) on the client.
  It starts as a wrapper over today's `Map` with a revision on it and no patch
  machinery at all.
- Keep [`Terrain`](../crates/common/movement/src/walk.rs#L43) as the query face
  for movement. It already is one, and it already has the two implementations
  that matter. Nothing about a step should learn what a patch is.
- The readers that walk the `Map` directly rather than through `Terrain` —
  the renderer, [`BuildingMap`](../crates/client/render/src/interiors.rs#L1025),
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
  [`Map::load_facet`](../crates/common/uofiles/src/map.rs#L262) as the reader.
- **Done when** an imported facet round-trips byte-identically, and a decoded
  chunk answers the same land and statics as
  [`Map::land`](../crates/common/uofiles/src/map.rs#L509) and
  [`Map::statics_at`](../crates/common/uofiles/src/map.rs#L568) for sampled
  tiles across Felucca.

Then the server reads the base set instead of the install, and existing
movement, LoS and harvesting tests pass unchanged over the new source. That is
the real acceptance test for B, and it needs no patches to run.

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

- Replace the file stamp in [`bake.rs`](../crates/common/movement/src/bake.rs#L22)
  with the source revision, and do the same for the building flood, the occluder
  bake, the minimap cache — see
  [`minimap_lod_plan.md`](minimap_lod_plan.md), which already asks for exactly
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
  ([`extended.rs:27`](../crates/common/protocol/src/extended.rs#L27)) or a
  second stream over [`Dial`](../crates/client/net/src/transport.rs#L100).
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

A, then B, then C, with D following C closely because a stale bake is how a
changed world lies to a player. E and F come last and can be reordered against
each other. Every step ends with a world that runs; none of them is "replace
the runtime with streaming first and make it correct afterwards".

## First useful slice

The shard and our client both run on a base set imported from facet 0, with no
UO map or static files on the client machine; they agree on sampled land and
statics; one land patch and one static patch publish, survive a restart, change
what the server allows, and reach the connected client. Re-importing the same
facet and re-applying the same patches produces byte-identical chunks.
