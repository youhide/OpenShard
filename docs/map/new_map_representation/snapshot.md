# One world, one door

The executable plan for directions **A0** and **A** of
[`plan.md`](plan.md) — and nothing else in this track. It is worth landing on
its own: no format, no patches, no network, no editor, and every test in the
workspace passes at the end of each phase.

Parent: [`plan.md`](plan.md) · Track: [`README.md`](README.md) · The measured
state it starts from: [`client_today.md`](client_today.md)

## What this is

Two changes that together make the world a thing with an owner.

1. **A0** — the block order stops being arithmetic written out five times and
   becomes one type whose whole job is that arithmetic. Internal to
   `openshard-uofiles`; no reader outside it changes. **Built.**
2. **A** — the map stops being something a caller loads for itself and becomes
   a revisioned snapshot every reader takes a handle to.

Neither adds a feature. That is the point: everything after them in the track
is cheap only if they land first, and if the rest of the track slipped these
would still have been worth doing.

## Phase 1 — `LandGrid` — **built**

Landed as [`crates/common/uofiles/src/grid.rs`](../../../crates/common/uofiles/src/grid.rs).
What it turned out to owe beyond this section is under
[What phase 1 left behind](#what-phase-1-left-behind).

### The problem, stated

The land is block-ordered: blocks column-major, cells row-major within a block.
[`map.rs`'s header](../../../crates/common/uofiles/src/map.rs#L1) records why
that is dangerous — got backwards, the file still parses, every block is still
196 bytes, every read lands somewhere plausible, and you find out when a player
walks into an ocean that should be a coastline.

It is written out five times inside one file, listed in
[`plan.md`'s A0](plan.md#a0--the-cell-array-becomes-a-type-that-owns-the-order),
including two verbatim copies in functions that do not call each other
([`cell_index`](../../../crates/common/uofiles/src/map.rs#L502) and
[`block_index`](../../../crates/common/uofiles/src/map.rs#L654)) and one
inverted ([`load_statics`](../../../crates/common/uofiles/src/map.rs#L448)).

### What to build

A new module `crates/common/uofiles/src/grid.rs`. `map.rs` imports from it; no
`pub use` — [`style.md`](../../style.md)'s rule is that a type is imported from
where it is declared.

```rust
/// The land of one facet, in the block order the files are in.
pub struct LandGrid { /* width, height, cells */ }

/// A block's position on the facet — not a tile, and not a radar chunk.
pub struct BlockCoord { /* x, y in blocks */ }

/// A block's position in the linear array. Derived; never built by a caller.
pub struct BlockIndex(u32);

/// A cell's position in the linear array. Derived; never built by a caller.
pub struct CellIndex(u32);
```

`LandGrid` owns, and is the only thing that knows:

| | |
|---|---|
| construction | the triple loop now in [`from_blocks`](../../../crates/common/uofiles/src/map.rs#L332), and the byte walk in [`from_bytes`](../../../crates/common/uofiles/src/map.rs#L380) |
| tile → cell | today's `cell_index` |
| tile → block | today's `block_index` |
| block → linear | the `block_x * blocks_down + block_y` written four times |
| **linear → world origin** | the inverse `load_statics` open-codes backwards |
| read and write | `get(x, y)`, `set(x, y, cell)`, `block(BlockIndex) -> &[LandCell]` |
| **transitions** | the next cell east or south, and whether that crossed a block edge |

### Transitions, and why they are not decoration

Stepping east is `+1` cell inside a block and `+blocks_down` blocks across its
eastern edge; south is `+8` cells inside a block and `+1` block across its
southern one. A rectangle walk that asks the grid for its next cell stops
re-deriving an index per tile — but the reason this matters is the second one:
**it makes the walk order a property of one iterator rather than of every
caller's loop nesting.**

That is currently observable in the picture.
[`depth::Order`](../../../crates/client/render/src/depth.rs#L55) is
`{ tile: x + y, priority_z }`, so every tile on one anti-diagonal shares `tile`
and the pre-draw sort is stable: for two statics on different tiles of one
diagonal at equal `priority_z`, the last one walked is the last one drawn. A
grid-owned iterator does not fix that — it puts the order in one place where a
later direction can fix it.

### Decisions, taken here

**`BlockCoord` is one type, and a radar chunk is not it.**
[`interiors::BlockId`](../../../crates/client/render/src/interiors.rs#L18) and
[`composite::MapBlock`](../../../crates/client/render/src/composite.rs#L56) are
the same value under two names — an 8×8 map block's coordinate — and become
`BlockCoord`. `RadarChunkCoord` does **not**: a radar chunk is 64 tiles square
(`BASE_CHUNK_TILES`), so it addresses a different grid, and collapsing the two
is exactly the confusion [`pixels.md`](../../pixels.md) exists to prevent.

**`BlockIndex` and `CellIndex` have private fields and an accessor**, unlike
`LandTile(pub u16)`. They are *derived* values: a caller constructing one by
hand is the precise bug the type is there to prevent, where a `LandTile` is
read straight off the wire or the file and has to be constructible.

**Statics do not move in this phase.** `Map::statics` stays
`Vec<Vec<StaticItem>>`; changing it is
[direction B](plan.md#what-felucca-measures-before-the-layout-is-chosen). What
this phase *does* owe it is stating the coupling that is load-bearing and
implicit today: **`statics` is indexed by the same `BlockIndex` as the cells.**
Nothing enforces that now.

**`Map`'s public API does not change.** `land`, `statics_at`, `statics_in_row`,
`statics_in_block`, `land_corners`, `average_land_z` keep their signatures and
their behaviour. That is what makes this phase landable without touching a
reader.

### Done when

- `blocks_down`, `* blocks_down +` and `% BLOCK_SIZE` appear nowhere in
  `map.rs` outside `LandGrid`.
- `block_order_is_column_major` and `cells_within_a_block_are_row_major` are
  tests of `LandGrid` rather than of `Map`.
- A test asserts the round trip `origin_of(index_of(block_of(x, y)))` lands on
  the block's north-west tile — the inverse `load_statics` gets wrong silently.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets` and
  `cargo fmt --all` are silent.

## Phase 2 — the snapshot

### What to build

A new crate `crates/common/map` (`openshard-map`) — both ends need it, so the
dependency rule puts it under `common/`. It is the crate
[direction B](plan.md#b--our-own-chunk-format-and-a-uo-importer) later fills
with the chunk format; creating it here means B does not also have to move
things.

```rust
/// One immutable version of one facet.
pub struct MapSnapshot { /* facet, revision, map */ }

/// Which version of a facet this is. Bumped by whoever publishes a change.
pub struct MapRevision(u64);
```

`MapSnapshot` has one owner — `Resources` on the client, the facet terrain on
the server. It owns the decoded map and is not itself reference counted.
Leaf code keeps borrowing `&Map` through `MapSnapshot::map()`; no ordinary
reader owns a `Map`.

### Decisions, taken here

**A snapshot holds one facet, and knows which.** This closes
[`client_today.md`](client_today.md)'s finding 8 — `Map` today names only a
*size*, `describe_size` cannot tell Malas from Ter Mur, and the facet number
that resolved the ambiguity at load time is then thrown away. It also stops the
client's single-map, `FACET: u8 = 0` shape from being baked in: a second facet
is a second snapshot, looked up by `Facet`, rather than a reopening.

**This phase changes owners, not signatures.** `MapSnapshot::map() -> &Map`
exists, and a leaf function that needs the land keeps taking `&Map` — the
*caller* passes `snapshot.map()`. There are **78 `&Map`/`Arc<Map>` signature
sites across 16 files**; converting them all would be churn with no reader
better off, and would turn a landable phase into a sweep. What must change is
every place that **owns or loads** one:

| Where | Today |
|---|---|
| [`Resources::map`](../../../crates/client/app/src/resources.rs#L37) | `Map` loaded by [`lib.rs:461`](../../../crates/client/app/src/lib.rs#L461) |
| [`FacetState`](../../../crates/server/state/src/runtime.rs#L377) | its `terrain`, from [`boot.rs:618`](../../../crates/server/server/src/boot.rs#L618) |
| [`link::connect`](../../../crates/client/app/src/link.rs#L535) | reads the map to predict a step across the thread |
| the three bakes | navigation, the building flood, the occluder table |

The invariant that makes it real: **`Map::load_facet` is called in exactly one
place per process, and that place produces a `MapSnapshot`.** Everything else
borrows its map from that owner.

**The network thread is transport, not a map reader.** It emits decoded packet
mutations; the event-loop owner applies them to its `WorldView` and its `Walk`.
It also receives an already encoded step packet to send. Consequently the
prediction, including the terrain lookup and the step sequence, stays beside
`MapSnapshot`; no `Arc<Map>` crosses into `link::connect`.

**Both ends stop agreeing by luck.** The opening handoff names this: the client
and the server load the same install independently. This phase does not merge
the two processes' loads — they are two processes — but it makes each of them
have *one* answer with a revision on it, which is the precondition for later
saying they are the same answer.

**A bake gains a revision field; it does not lose its mtime stamp.** Navigation
([`bake.rs`](../../../crates/common/movement/src/bake.rs#L120)), the building
flood ([`artscan/interiors.rs`](../../../crates/client/artscan/src/interiors.rs#L240))
and the art table each key on input file name, size and mtime. This phase adds
"which revision was this built from" beside that. Replacing the mtime key is
[direction D](plan.md#d--derived-data-keyed-by-revision), and a session that
does it here has taken on D as well.

**MapRevision starts at 1 and never moves in this phase.** Nothing publishes yet.
A revision that cannot change is still worth having: it is what a bake records,
and it is the field C later makes mean something.

### Done when

- No production code outside `openshard-map` calls `Map::load_facet`.
- The client `Resources` and the server facet terrain each own a `MapSnapshot`,
  and reach their terrain through it.
- `link::connect` receives neither a `MapSnapshot` nor a map handle: it sends
  prepared packets and returns decoded mutations for the owner to apply.
- Every one of the three bakes records the revision it was built from, and
  refuses a snapshot whose revision does not match — alongside, not instead of,
  its existing staleness check.
- A test asserts a snapshot knows its own facet, and that Malas and Ter Mur —
  the same block count — produce snapshots that disagree about which they are.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets` and
  `cargo fmt --all` are silent.

## What phase 1 left behind

Written down as it was found, and none of it blocks phase 2.

- **The `BlockCoord` collapse is decided, not done.** Phase 1's decision stands
  — [`interiors::BlockId`](../../../crates/client/render/src/interiors.rs#L18)
  and [`composite::MapBlock`](../../../crates/client/render/src/composite.rs#L45)
  are the same value under two names and become
  [`BlockCoord`](../../../crates/common/uofiles/src/grid.rs) — but converting
  them is a reader change, and [`plan.md`'s A0](plan.md#a0--the-cell-array-becomes-a-type-that-owns-the-order)
  says in as many words that nothing outside `uofiles` changes there. They are
  still two types, still `u32` and `u16` respectively. Whoever collapses them
  should take `RadarChunkCoord` off the table in the same breath, because the
  reason it stays separate is the one thing a reader of that diff will ask.
- **The land and the statics share an index that is still two arrays.**
  `Map::statics` is now documented as being addressed by the same `BlockIndex`
  as the cells, and every subscript of it goes through
  `LandGrid::index_of` — which is as close to enforcement as two parallel
  `Vec`s get. A type that cannot express the mismatch is
  [direction B](plan.md#b--our-own-chunk-format-and-a-uo-importer)'s to build.
- **The transitions have no caller yet.** `east_of`, `south_of` and
  `cells_in_row` are tested against a fresh `cell_index` per tile, and nothing
  walks them: every rectangle walk in the workspace is `client/render`'s, which
  is outside A0's reach. The first caller is what turns the walk order into a
  property of one iterator — which is the half of the point that
  [`depth::Order`](../../../crates/client/render/src/depth.rs#L55)'s
  anti-diagonal tie is waiting on.
- **A block column is an eight-wide strip, and that is now written down.** The
  two orders compose: `block_y * 64 + y_local * 8 + x_local == y * 8 + x_local`,
  so a whole block column is one row-major image eight tiles wide. It is why
  stepping south is `+8` on *every* tile rather than only inside a block, and it
  is the whole of the `CellIndex → (x, y)` inverse. `grid.rs`'s module header
  derives it and `the_two_orders_compose_into_a_strip` holds it against the
  plain spelling; direction B should know it before choosing a chunk layout,
  because it is the property that would be lost.
- **`Map::from_blocks` still takes two bare `u32`s.** A facet's extent in blocks
  has no type, where a block's *position* now does. Small, and it is the kind of
  thing B will want anyway.

## Out of scope, named

Written down because each is a thing a session might reasonably drift into.

- **The statics layout.** Direction B. Phase 1 states the `BlockIndex`
  coupling and leaves `Vec<Vec<StaticItem>>` alone.
- **Replacing the bakes' mtime key.** Direction D.
- **Lazy chunk residency and compressing the land.**
  [Direction G](plan.md#g--residency-and-size-deferred-on-purpose), deferred on
  purpose. What this plan owes it is only that neither door — `LandGrid` or
  `Terrain` — ever hands out a slice spanning more than one block.
- **One composer for the base and the entities over it.** `LiveTerrain` on the
  server, `Cluttered` on the client for the step, and `net_command`'s multi
  expansion for the picture are three ways of laying the world's contents over
  the map. Merging them is the natural successor to this plan and is
  deliberately not in it: this is about the map having one owner, and that is
  about entities laid over it — a second question, and one that would double
  the diff.
- **The client-side defects in [`client_today.md`](client_today.md)** — the
  world map's LOD, the per-frame facet scan, the radar cache's missing
  eviction. None of them is blocked by this plan and none of them blocks it.

## Where a session starts

Phase 1, in `crates/common/uofiles/src/map.rs`. It touches one crate, no
reader outside it, and the five spellings of the block order are listed with
line numbers in [`plan.md`'s A0 table](plan.md#a0--the-cell-array-becomes-a-type-that-owns-the-order).
