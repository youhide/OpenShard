# Six terrains, and one of them is a terrain

The question that produced this document was *why is there a trait here at all,
rather than an explicit reference?* — and the answer is that there should not
be one. `Terrain` has six implementors outside a `mod tests`, and **five of
them are not terrains**. They are actions taken over one: a mask of what the
live world put in the way, a rectangle to stay inside, a memo table, and the
absence of a map. Each was made a *kind of terrain* because the seam was a
trait, and each one being a kind of terrain is then the argument for the seam
being a trait.

The other half of the trait is not terrain either. Nine of its fifteen methods
are a client-file lookup table wearing a terrain's coat: `item_weight(graphic)`
takes no coordinate, reads no cell, and cannot be changed by a placed crate.

So this is not a plan to swap `dyn` for a generic. It is the plan to end up
with `find_path(&MapTerrain, &Overlay, Doors)` — explicit types, imported by
name — and no `Terrain` trait on the search at all.

Track: [`README.md`](README.md) · The map's owner:
[`new_map_representation/snapshot.md`](new_map_representation/snapshot.md) ·
The routing it feeds:
[`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md)

## The six, and what each one actually is

Read off the workspace, not remembered. Thirty types implement `Terrain`;
twenty-four are test doubles. These are the rest:

| | | |
|---|---|---|
| [`MapTerrain`](../../crates/common/movement/src/terrain.rs#L61) | the map and `tiledata.mul` | **a terrain** |
| [`Cluttered`](../../crates/client/app/src/clutter.rs#L306) | the client's live items over it | a **mask** |
| [`LiveTerrain`](../../crates/server/state/src/obstruct.rs#L199) | the server's live items over it | the **same mask** |
| [`CachedTerrain`](../../crates/common/movement/src/cache.rs#L30) | memoises `can_step` for one query | a **memo table** |
| [`InRegion`](../../crates/common/movement/src/navigation.rs#L80) | three lines: refuse a step leaving a rectangle | a **parameter** |
| [`OpenWorld`](../../crates/common/movement/src/walk.rs#L270) | `can_step` returns `Some(to)` | the **absence** of a map |

`InRegion` is the clearest case, because it is short enough to quote whole:

```rust
fn can_step(&self, from: Point, to: Point) -> Option<Point> {
    (self.region.contains(from) && self.region.contains(to))
        .then(|| self.terrain.can_step(from, to))
        .flatten()
}
```

That is a bounding box the graph builder wants the search to respect. Making it
a terrain means every `can_step` on that search pays a vtable hop to ask a
rectangle a question the search could have been told once.

## The two that are one

The client's and the server's live overlays are the same structure written
twice:

```rust
// client/app/src/clutter.rs                 // server/state/src/obstruct.rs
struct Blocker {                             pub struct Obstacle {
    z: i8,                                       pub entity: EntityId,
    height: TileHeight,                          pub door: bool,
    door: bool,                                  pub z: i8,
}                                                pub height: u8,
struct Clutter {                             }
    tiles: HashMap<Tile, Vec<Blocker>>,      pub struct Obstructions {
}                                                tiles: HashMap<(u16, u16), Vec<Obstacle>>,
                                             }
```

[`clutter.rs`](../../crates/client/app/src/clutter.rs)'s own header already
says so — *"this is the client's half of `Obstructions`"* — and gives the
reason the two must agree: same predicate (`item_blocks`), same z-span, *"so
the two ends agree by construction rather than by resemblance"*. They are
agreeing by resemblance. Two copies of a structure whose whole purpose is that
both ends compute the same answer is the shape this repository has a name for.

The one real difference is `EntityId`: the server needs it because
`Obstructions::block` is idempotent *per entity and z* and things get
unblocked, while the client rebuilds the whole index from a view update. That
is a question the shared type has to answer, not a reason for two types.

And `through_doors` is already a `bool` on both sides — the flag, exactly as
the shape of the problem suggests.

## What the shape becomes

```rust
find_path(terrain: &MapTerrain<'_>, over: &Overlay, doors: Doors, ...)
```

Both types named and imported. What happens to the other five:

| | |
|---|---|
| `Cluttered` / `LiveTerrain` | one `Overlay` in `common/movement`; both ends **build** it instead of implementing a trait |
| `through_doors: bool` | a `Doors` argument to the search |
| `InRegion` | a `Option<Region>` bound the search is told once |
| `CachedTerrain` | moves *inside* `search`, which already owns per-query maps for `came_from`; its lifetime was already exactly one query |
| `OpenWorld` | an empty overlay over no map — `Option<&MapTerrain>`, which `LiveTerrain` already carries |

That is five implementors, five `dyn` anchors, and the trait's reason for
existing, all leaving together.

### What does not fit a mask, and is named here

**A deck adds a surface.** [`LiveTerrain::aboard`](../../crates/server/state/src/obstruct.rs#L183)
resolves a step onto a moored ship's deck over water the map says is not
standable. So `Overlay` is not a mask in the sense of "subtract these tiles" —
it has two halves, blockers and surfaces, and the second one is why a bitmask
per tile would have been the wrong first guess. `docs/boats.md`'s B3 already
argued this: the hull stays out of `Obstructions` *because an index that only
subtracts cannot say "there is somewhere to stand here"*.

**`MapTerrain` is generic over ownership**, `M: AsRef<Map>`, so the server can
own its map and the client borrow one. Two references — `MapTerrain<'a> { map:
&'a Map, tiles: &'a TileData }` — is the shape that makes it one concrete type;
it is cheap enough to build per query, and the owner keeps holding the
`MapSnapshot` and the `TileData` it already holds. Whether that is worth doing
is phase 2's call, and it is the one place where an `AsRef` bound might survive
honestly.

## The other half: the table wearing a terrain's coat

Fifteen methods, three unrelated questions:

| | |
|---|---|
| *may a body step here* | `can_step`, `ground_z` — **two**, and that is every pathfinding use. [`path.rs`](../../crates/common/movement/src/path.rs) never calls the trait at all; it goes through `step_allowed`, which calls `can_step`. `navigation.rs` adds `ground_z`. |
| *where is the surface* | `stand_z`, `spawn_z`, `can_fit`, `sight_clear` |
| *what does tiledata say about this graphic* | `item_blocks`, `item_height`, `item_weight`, `item_layer`, `item_name`, `multi_components`, `land_is_water`, `land_tile`, `statics_at` |

The third group travels through `Terrain` for one reason: `server/items`,
`server/crafting` and `server/npc` do not depend on `openshard-uofiles`, and
this is the door that was already open. `server/housing`, `server/world` and
`server/state` **do** depend on it and could ask directly today.

The cost is visible without looking for it. `CachedTerrain` memoises one method
and is obliged to forward fifteen. And `FacetState.terrain` is read from ten
production sites — `items/weight.rs`, `items/capacity.rs`, `items/backpack.rs`,
`housing` (four), `crafting/environment.rs`, `npc/spawn.rs`, `decor.rs`,
`spawners.rs`, `speech.rs`, `gm.rs` — where almost none of them wants a floor.
They want an item's weight, its layer, or a multi's components.

## Phase 0 — the oracle, before anything moves

**Nothing here is landable without it, because "faster" is currently
unmeasurable.** The only routing benchmark on record is synthetic: a 1024×1024
open world where the hierarchy is *slower* than flat A\* (0.974 ms p95 against
0.803 ms), in
[`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md).
No facet-0 measurement exists at all; that document has carried it as
outstanding since 2026-08-13.

Two probes already exist and neither has a recorded run on a real install:
[`map_path_probe`](../../crates/common/movement/examples/map_path_probe.rs) and
[`coarse_bench`](../../crates/common/movement/examples/coarse_bench.rs).

**Done when:** both have a committed facet-0 run with the numbers in this
document — p50/p95/worst per route class, node counts, `TransitionCacheStats`
hit rates. Every phase after this is reported as a delta against it, and a
phase that cannot show one is not finished.

## Phase 1 — one `Overlay`, built by both ends

The load-bearing phase, and the one that makes every later one mechanical.

`Overlay` lands in `common/movement`: blockers by tile with their z-spans and
their door flag, plus the surfaces a deck adds. `Obstructions` becomes the
server's *builder* for one, `Clutter` the client's. Neither implements
anything.

Two decisions this phase takes:

- **Does `Overlay` carry `EntityId`?** The server's mutation API is keyed by
  entity and z; the client has no entities to key by. Either the shared type
  carries an optional owner, or the server keeps its own keyed index and
  *produces* an `Overlay` from it. The second keeps the shared type honest and
  is the default unless building it per tick measures badly — phase 0's numbers
  are what says.
- **Blockers and surfaces are one type or two.** `aboard` is the only surface
  source today (`Boats`), and it is the reason a bitmask is not the answer.

**Done when:** `Cluttered` and `LiveTerrain` are gone as `Terrain`
implementors, both ends build an `Overlay`, and one test asserts the two ends
produce the same overlay for the same world — which is the agreement
`clutter.rs`'s header claims and nothing currently checks.

## Phase 2 — the search takes explicit types

`find_path`, `find_path_toward`, `search`, `step_allowed`, `corner_open`,
`Around::read`, and the whole of `navigation.rs` take `&MapTerrain` and
`&Overlay` by name.

`InRegion` becomes a bound the search is told. `CachedTerrain` moves inside
`search`. `OpenWorld` becomes `Option<&MapTerrain>` being `None`.

Whether `MapTerrain` collapses to a two-reference struct or keeps its `AsRef`
parameters is decided here — see the note above.

**Done when:** `grep -rn "dyn Terrain" crates/common/movement` is empty, and
`path.rs`, `navigation.rs`, `cache.rs` and `detour.rs` name no trait bound at
all.

## Phase 3 — the table stops being a terrain

`item_blocks`, `item_height`, `item_weight`, `item_layer`, `item_name`,
`multi_components` leave `Terrain`. Either `items`, `crafting` and `npc` depend
on `openshard-uofiles` as `housing` already does — in which case there is no
seam to mint — or they get one narrow one. What decides it is
[`architecture.md`](../architecture.md)'s layering, and if the dependency is
allowed there, minting a seam instead is inventing a problem.

**Done when:** no caller reaches a client-file table through `Terrain`.

## Phase 4 — `FacetState` stores data, not an abstraction

[`FacetState::terrain`](../../crates/server/state/src/runtime.rs#L379) is a
`Box<dyn Terrain + Send + Sync>` whose doc comment says the crate *"sits below
the client-file parsers"* — which has not been true since `openshard-state`
gained its `openshard-uofiles` dependency for
[`customisation.md`](../customisation.md)'s C1. It holds the map and the
tiledata it already has, and hands out a `MapTerrain` on request.

What holds the box up in practice is **tests**: about fifteen substitutions
across six files assign a hand-written double into `facet_state_mut().terrain`
— `Ground`, `Sea`, `Shop`, `BlindTerrain`, `FrameTerrain`, `NamedTerrain`,
`RaisedFloorTerrain`. Production builds a facet terrain in exactly one place,
[`boot.rs:664`](../../crates/server/server/src/boot.rs#L664).

Each double is replaced by a [`Scene`](../../crates/common/movement/src/scene.rs),
which builds a real `MapTerrain` from hand-placed ground, floors, stairs and
walls with no client files. Two additions it needs: an owned `into_terrain`
(today's `terrain()` borrows), and a multi table —
[`Multis::of`](../../crates/common/uofiles/src/multi.rs#L282) already builds one
from hand-made values, so it is wiring, not parsing.

This is worth doing on its own evidence, and `scene.rs`'s own header is the
argument: *"A fixture that reimplemented the rule would agree with itself and
prove nothing."* Most of these doubles answer `can_step` with `Some(to)`.

**Done when:** `grep -rn "dyn Terrain" crates` is empty, and `Terrain` is
either gone or is two methods with a named reason to exist.

## Phase 5 — the graph nobody reads

Unrelated to the seam, found in the same reading, here so it is not lost.

[`boot.rs:615`](../../crates/server/server/src/boot.rs#L615) loads the baked
navigation graph, validates its dimensions, and stores it in
`FacetState.coarse`. The only call to
[`coarse_router()`](../../crates/server/state/src/runtime.rs#L422) in the whole
workspace is in a test. Server AI plans with flat
[`find_path`](../../crates/server/ai/src/lib.rs#L79) at a budget of **400**
explored tiles — so a creature cannot route across a town while the artifact
that would let it sits loaded and unread. The client does use it:
[`steer::Ground::path`](../../crates/client/app/src/steer.rs#L331) falls back
past 8 tiles.

Either `step_toward` gains the same fall-back, or `boot.rs` stops paying for
the load, the validation and the resident graph. What it must not stay is what
it is: paid for, validated, unread.

**Done when:** either a test walks a creature a distance flat A\* at budget 400
cannot, or `FacetState.coarse` is gone.

## Decisions, taken here

**The trait goes, not just the `dyn`.** Five of the six implementors are an
action over a terrain rather than a terrain, so making the seam generic would
preserve the mistake with better codegen. A generic's bound *is* a trait; the
choice between `&dyn T` and `&impl T` is about dispatch, and dispatch is not
what is wrong here.

**Explicit types, imported by name.** The dependency rule was the one honest
argument for a trait — `find_path` is in `common/movement`, `LiveTerrain` in
`server/state`, `Cluttered` in `client/app`, and `common` may name neither. An
`Overlay` living in `common` that both ends *build* answers that without
inverting anything: data crosses the boundary, not behaviour.

**Behaviour becomes data, deliberately.** A mask, a flag, a rectangle and a
memo table are values. They were types with a vtable because the seam invited
it, and each one cost a virtual call on every A\* edge for the privilege.

**Phase 0 is not a formality.** The one benchmark on record shows the hierarchy
losing to flat A\* on an open map. A refactor justified by "virtual calls are
slow" that ships without a before and an after is the same kind of claim.

**No flag day, but no `?Sized` scaffolding either.** An earlier draft proposed
migrating through `&T where T: Terrain + ?Sized`, which `dyn Terrain` satisfies
and so breaks no caller. That was scaffolding for keeping the trait. With the
trait leaving, the phases are ordered so each one removes implementors rather
than re-typing callers: phase 1 lands `Overlay` beside the existing impls,
phase 2 switches the search, and only then do the impls come out.

## Out of scope, named

- **The statics layout.** 120,745 allocations and 38.2 MiB where a CSR pair
  would be 2 and ~13.5 MiB —
  [direction B](new_map_representation/plan.md#b--our-own-chunk-format-and-a-uo-importer)'s,
  measured there.
- **Residency.** The whole facet is resident at ~150 MiB on both ends.
  [Direction G](new_map_representation/plan.md#g--residency-and-size-deferred-on-purpose).
- **A second hierarchy level.** Phase 3 of
  [`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md),
  gated on the facet-0 numbers phase 0 here produces.
- **`MAX_SEARCH_TIME` and the node budgets** — 50 ms inside one search, 400 for
  server AI, 600 for a client plan. Phase 0's data is what those can finally be
  asked against; changing them before it exists is guessing.
- **`net_command`'s multi expansion.** The third way entities are laid over the
  map, and the picture's rather than movement's. `Overlay` may end up being
  what merges it, which is
  [`snapshot.md`](new_map_representation/snapshot.md)'s own named successor —
  but this plan does not take the picture on.

## Where a session starts

Phase 0, which needs a client install. Then phase 1, which is where the design
actually lands: everything after it is removing implementors that no longer
have anything to implement.
