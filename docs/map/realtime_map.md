# The map you hold — era R, in order

> **Status: R1 and R2 built; R3, R4 and R5 next, in any order.** The tile table
> has its own crate, `openshard-uofiles` is readers, formats and errors, and one
> `World` is the map on both ends of the wire. Everything below R2 is still a
> plan.

The executable half of [`map_rebuild.md`](map_rebuild.md)'s era R. That document
holds the model and the decisions — three layers, what may be baked, why a house
is a layer and not a patch; **this one holds the work**: what moves where, in
which commit, and what has to still be true afterwards.

The split is the one this track already uses.
[`plan.md`](new_map_representation/plan.md) records intent and
[`snapshot.md`](new_map_representation/snapshot.md) was the plan being executed;
`map_rebuild.md` records the eras and this is the plan being executed now.
Progress does not go in either — it goes in
[`handoffs/`](handoffs/).

## The shape it ends at

```rust
// openshard-map: what the shard and the client each hold, one per facet
struct World { base: Option<MapSnapshot>, live: Overlay }

// openshard-movement: what a query borrows, unchanged from node E
Footing { map: Option<MapTerrain<'a>>, overlay: &'a Overlay, doors: Doors }
Footing::of(&World, &TileData, Doors)      // the one composition
```

Three properties fall out of that and they are the point of the era:

- **A reader takes one value.** `world`, not a map and an overlay a caller
  remembered to carry together.
- **A bake cannot reach the live layer.** It takes `world.snapshot()`, which is
  the ground, the statics and a revision — and has no field to reach a door
  through. [`map_rebuild.md`](map_rebuild.md)'s invariant becomes a borrow rather
  than a rule.
- **The tile table stays outside the world**, because its scope is different: one
  install has one table and several facets. `Footing::of` takes it as its own
  argument, and that is the whole of the asymmetry.

## The order it lands in

R1 and R2 are the two the rest wait on; R3, R4 and R5 are independent of each
other and can land in any order once R2 has.

```
R1. the table leaves the file reader ✔ ──> R2. the third layer joins the type ✔ ──┬─> R3. a house has floors
                                                                                 ├─> R4. statics become one run
                                                                                 └─> R5. one install, one load
```

## R1 — the table leaves the file reader ✔

**Goal.** `openshard-uofiles` reads files and declares nothing.

**Built**, in the four commits below plus two the plan did not foresee, both
recorded under *What the move decided* at the end of this node.

### What moves

A new crate, `crates/common/tiles`, package `openshard-tiles`. The workspace
`members` glob is `crates/*/*`, so only the `[workspace.dependencies]` line is
new.

| moves | stays in `uofiles` |
|---|---|
| `TileData`, and its accessors | `TileDataFormat` — the High Seas widening and the arithmetic that detects it |
| `LandTile` (the **entry**: flags, texture, name), `StaticTile`, `AnimId` | the group headers, the byte offsets, the file constants |
| `TileFlags` | `TileDataError`'s I/O variants |
| `LAND_TILE_COUNT`, `STATIC_TILE_COUNT`, `pluralize_name` | the reader that fills a `TileData` and hands it back |

The line is [`snapshot.md`](new_map_representation/snapshot.md)'s own, quoted
because it decides every doubtful case: *"a constant that describes how many
bytes a thing is on disk has no business in a crate that will one day hold a
world nobody serialised that way."*

### The name collision this ends

There were **two** types called `LandTile`: the entry in `uofiles::tiledata`,
and the *id* in `openshard-map` that indexes it. They had coexisted because they
were in different crates and never met. Both are in
[`openshard-tiles`](../../crates/common/tiles/src/lib.rs) now.

- The **id** moves to `openshard-tiles` and becomes `LandTileId`. An id belongs
  beside the table it indexes, and the pair then reads like the static side
  already does: `Graphic` (an id, on the wire, in `openshard-protocol`) and
  `StaticTile` (its entry).
- The **entry** keeps `LandTile`.

`LandCell.tile` then names `LandTileId`, which is what it always was.

### `surfaces` goes to movement, not to the table

[`surfaces.rs`](../../crates/common/movement/src/surfaces.rs) is neither a file
nor a table: `stand_surfaces` walks a column and answers *where could a body
stand*, which is a movement rule that happens to have been parked beside the
parser. It is also the seed [N1](navigation_spans.md#n1--three-tiers) builds
`Spans` from — its own header says the walk is shared input to movement and the
interior index, and `client/render` already depends on `openshard-movement`, so
the interior index loses nothing.

### The commits

1. **The crate and the move**, naming nothing outside `uofiles`, `openshard-map`
   and the new crate. The tree is left broken for one commit, as phase 3 did
   deliberately: the first commit says what a table *is*, the second is
   mechanical.
2. **The call sites** — around 120 files import `uofiles::tiledata` today, over
   twelve crates. Compiler-led.
3. **`LandTile` the id becomes `LandTileId`.** A rename, and worth its own commit
   so the diff is readable.
4. **`surfaces` to `openshard-movement`.**

All four landed in that order, and the tree was broken for exactly one commit as
the plan said it would be.

**Done when:** no crate depends on `openshard-uofiles` to ask what a graphic is;
`openshard-uofiles` exports readers, formats and errors only; the four checks are
silent. **Met.** The crates that still depend on `openshard-uofiles` depend on it
for a *reader* — multis, art, the map importer — which is the line this node set
out to draw.

**Risk:** low, and it is the reason this goes first. Nothing changes behaviour;
every failure is a compile error. It ran that way: every failure the move
produced was a compile error, and the only red test in the run was a gate that
was already red — movement's `span_census` example takes a `--facet` on its
command line and had never been added to `facet_bare_fields`' allowlist.

### What the move decided

Two questions the plan did not name, both settled by the same rule it set:

- **`TextureId` moved too.** [`LandTile::texture`](../../crates/common/tiles/src/lib.rs)
  holds one, so leaving it beside the reader of `texmaps.mul` would have made
  the table depend on a file reader — the thing R1 exists to end. `AnimId` was
  already in `tiledata.rs` for exactly this reason and is the precedent: **the
  table declares the ids its entries name**, and the readers of those two files
  take them as arguments.
- **The layout left `TileData` with the reader.** `TileDataFormat` stays in
  `uofiles` by the plan's own table, so the table cannot hold one. It is a fact
  about a *file* rather than about a tile — a table built by hand has no layout
  — so `tiledata::load` and `tiledata::parse` hand back a `Reading { tiles,
  format }`, and the single caller that wanted the format writes it into the
  boot log. Both are free functions now: `TileData` is not `uofiles`' type to
  hang a constructor off, and `TileData::from_tables` is the one way to build a
  populated table.

## R2 — the third layer joins the type ✔

**Goal.** One value is the map.

**Built**, in the four commits below plus one the plan did not foresee — the
call sites, which are their own commit for the reason R1's were. What the move
decided is at the end of this node.

### What moves

`Overlay`, `Cover`, `CoverKind`, `Doors` and `Tile` move from
`openshard-movement` to `openshard-map`. They are storage — a span and a kind per
tile — and after R1 `Cover::of_static` can take its `StaticTile` from
`openshard-tiles` without dragging a file reader anywhere.

What does **not** move: every rule that reads one. `step_allowed`, `can_step`,
`corner_open`, the search, and `Footing` itself stay in `openshard-movement`.
`blocker_at`, `blocker_anywhere` and `surface_at` are lookups into the structure
and go with it.

`Tile` landed in [`grid`](../../crates/common/map/src/grid.rs), beside
`BlockCoord`, whose own doc already defined itself as *"a block's position on the
facet — not a tile"*. The pair is now written down rather than implied.

### What is built

- `World { base: Option<MapSnapshot>, live: Overlay }` in `openshard-map`, with
  `world.snapshot()` handing out the base and **no accessor that hands out both
  at once to a bake**. `base` stays an `Option` because a shard with no client
  files is a real configuration — that is `Footing`'s `map: Option<…>` already,
  moved one level down to where it belongs.
- `Footing::of(&World, &TileData, Doors)`, the one composition. Every production
  site that today builds a `Footing` field-by-field calls it instead.
- The server's `FacetState` holds a `World` where it held `map` and `overlay`;
  the four mutators (`block`, `unblock`, `moor`, `cast_off`) write through it and
  `refresh` keeps projecting, exactly as node E left them. **The builders do not
  move** — `Obstructions` and `Boats` are the server's, `clutter::fill` is the
  client's, and neither end learns about the other.
- The client's `Resources` holds a `World`; `clutter::fill` writes its live layer
  rather than returning a value the frame carries beside the map. The client's
  overlay left `PresentationWorld` for it, so `world::footing` takes one struct
  where it took two.

### The commits

1. The move, and `openshard-movement` re-importing from `openshard-map`.
   `6702e72c`.
2. **The call sites**, which the plan did not foresee and which are their own
   commit for the reason R1's were: thirty-eight files and nine server crates
   that only ever wanted to name a place. `49fa83c8`.
3. `World`, `Footing::of`, and the bake-facing accessor. `fbe6588f`.
4. The server: `FacetState` holds one. `9fd3f8af`.
5. The client: `Resources` holds one. `b3f5ed97`.

**Done when:** no production caller carries a map and an overlay as two values; a
bake's argument has no path to the live layer; the movement, boat and housing
test suites pass unchanged, because nothing about a rule changed. **Met.** The
`Footing::new` calls that survive are tests, examples, and the two that
deliberately want *less* than a world: the bake binary and the client's `guide`,
both reading the bare map their coarse graph was built over.

**Risk:** medium, and all of it is in the client's lifetime. The client rebuilds
its live layer per view and throws it away whole; putting it inside a `World` the
frame pins must not turn that into a value kept across frames by accident. It did
not: `clutter::fill` clears and rewrites, which is the same lifecycle assigning a
fresh overlay had, with the allocation kept instead of dropped.

### What the move decided

Three questions the plan did not name:

- **A body's height is the caller's.** `Cover::meets` read `PLAYER_HEIGHT` off
  `openshard-movement`, which the map's crate cannot see — and should not: how
  tall a creature is is a movement rule, and that constant's own comment admits
  it ought to vary by creature and does not. It is a `Body { z, height }`
  argument now, a type rather than a second `i32`, because a position and a
  length in the same units side by side on the hot path of every step say
  nothing about which is which. It also closed a latent disagreement:
  `can_fit` took a height, gave it to the map half and let the overlay half
  reach for the body constant. Every caller passes a person's height, so the
  answer is unchanged.
- **The client's `Resources::map` is a method with one `expect` in it.** `World`'s
  base is an `Option` because a *shard* with no client files is a real
  configuration; a client is not one — it opened the install to get this far —
  so the absence is unreachable at that end, and the alternative was the same
  `expect` written out at forty readers. Its whole-`&self` borrow is the cost:
  where the field it replaced borrowed only itself, this borrows the struct, and
  one call site (`window.rs`'s atlas rebuild, which wants `&mut resources.anim`
  beside the map) had to hoist its argument into a local.
- **`FacetState::set_map` replaces the public field.** A facet is inserted and
  *then* loaded on both ends, so something has to be able to give it ground after
  the fact. What it must not be is a field a reader can take without the layer
  beside it, which is what the old `pub map` was.

## R3 — a house has floors

**Goal.** You can stand on the second storey.

`Cover::of_static` is `tile.flags.is_blocking().then(…)`, so a floor — a platform
that does not block — produces nothing at all.
[`housing::block_footprint`](../../crates/server/housing/src/lib.rs#L871) folds in
only the components whose tiledata says they block, and `Footprint { tile, z,
height }` is a `Cover` with the kind left out.

- `Cover::of_static` grows the `Stands` arm: a platform tile yields
  `CoverKind::Stands` at `z + height`, halved for a climbable, which is the same
  arithmetic `stand_surfaces` applies to a map static. One rule, two sources.
- `housing::Footprint` becomes a `Cover` and stops being a second spelling of it.
- The client's placement path does the same, so the two ends keep agreeing by
  construction — the property node E landed and the thing this must not undo.
- **`net_command::multi_pieces`** ([client/app](../../crates/client/app/src/net_command.rs#L1044))
  is the third expansion of a multi in this workspace, and the picture's. With
  the components producing covers at placement, one expansion feeds both; folding
  it in is part of this node rather than a follow-up, because two expansions of
  one house is exactly the class of defect this document set exists about.

**Done when:** a mobile walks up a placed villa's stairs and stands on its first
floor; a test asserts a floor over open ground is `Stands` and a wall over it is
`Blocks`, on both ends; `grep -rn "CoverKind::Stands" crates` has more than one
producer.

**Risk:** the interesting one. A house's ground floor is currently walkable
*because the map's ground is under it*; adding a `Stands` cover at the same
height must not change where a body stands there. `Overlay::surface_at` picks the
nearest surface to the body, so the case to test is a floor laid exactly on the
ground it duplicates.

**Not in this node:** which of a shipped multi's components are floors is a
question about the tiledata, and if a real house has a floor the platform flag
does not mark, that is a [`findings.md`](../findings.md) entry, not a rule change.

## R4 — statics become one run

**Goal.** The base layer is one immutable run, and what changes is a layer above.

Measured: 2,906,871 statics over 120,744 non-empty blocks — **120,745
allocations, 38.2 MiB**. The CSR pair is already written in
[`chunk.rs`](../../crates/common/map/src/chunk.rs#L154) and was never carried
into `WorldMap`.

- `statics: Vec<Vec<StaticItem>>` becomes a flat `Vec<StaticItem>` and a
  `Vec<u32>` of per-block offsets. Two allocations, ~29.6 MiB, and the accessors
  keep handing back `&[StaticItem]`: the per-block sort by `(y, x)` is unchanged,
  so `statics_at`'s two binary searches and `statics_in_row`'s contiguous row
  survive as they are.
- `place_static` as an in-place tail shift **goes**. A builder assembles the base;
  [`patch::apply_op`](../../crates/common/map/src/patch.rs#L401) rebuilds the
  blocks its ops touched, which is what a publish is already defined to do.
- The **packed four-byte record** is not in this node. It changes accessors from
  handing back a reference to handing back a value, and it waits for
  [N3](navigation_spans.md#n3--the-search-takes-spans)'s measurement to say
  whether the statics are still on a hot path.

**Done when:** two allocations; the base-set round trip is byte-identical and
`openshard-map-import --verify` still compares all 29,360,128 tiles clean; the
resident size is recorded in this document.

## R5 — one install, one load

**Goal.** A process holds one facet, once.

[`boot.rs`](../../crates/server/server/src/boot.rs) and the client's own startup
each load a facet. Under `openshard-playground` that is one process holding two
~150 MiB copies of the same world, and the correctness half is
[`overview.md`](new_map_representation/overview.md)'s opening complaint: the two
ends match *because they opened the same install*, not because either was told
what the world is.

**Done when:** the playground loads a facet once and both ends read that value;
the shard's loader is the only production `load_facet` call.

## What none of this may break

The oracles that already exist, and which every node above runs:

| | |
|---|---|
| the base set round trip | `openshard-map-import --verify` — all 29.4M tiles, 0.6 s |
| movement over both sources | `base_set_terrain`, tens of thousands of sampled places |
| the two ends agreeing | node E's rule: `Cover::of_static` is called by both, and there is no second reading to diverge |
| the navigation artifact | its stamp carries a `MapRevision` and refuses itself on a mismatch |

## Where a session starts

**R3, R4 or R5** — the three are independent of each other and R2 has landed, so
any of them can go first. They are not the same size or the same kind of risk:

- **R3** is the one with a *feature* in it — you can stand on the second storey —
  and the one that can change where a body stands on ground it already walks on.
  Its own node names the case to test.
- **R4** is measured, bounded and has an oracle already written: the base-set
  round trip either stays byte-identical or it does not.
- **R5** is the smallest, and the only one that changes what a process *holds*
  rather than what a type is.

Progress goes in [`handoffs/`](handoffs/), not here.
