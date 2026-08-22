# Every edge is a virtual call

**A design error, not a trade-off.** `Terrain` is the seam between the map and
every question anyone asks it, and it is reached through `dyn` in all 39 places
that name it. Nothing needs that. This document is the plan to remove it —
everywhere, storage included — and to fix the two other things that turned up
in the same reading: a navigation graph the server pays for and never reads,
and a hot path with no measurement on any real facet.

One document rather than three because they are one question with three
answers: *what does it cost to get from the map to a step, and who checked?*

Track: [`README.md`](README.md) · The map's owner:
[`new_map_representation/snapshot.md`](new_map_representation/snapshot.md) ·
The routing it feeds:
[`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md)

## The measured state this starts from

Read off the workspace, not remembered:

| | |
|---|---|
| lines naming `dyn Terrain` | **39**, across `common/movement`, `server/state`, `server/world`, `client/app` |
| `Terrain` implementors | **30** — and only **six** outside a `mod tests`: [`MapTerrain`](../../crates/common/movement/src/terrain.rs#L523), [`Cluttered`](../../crates/client/app/src/clutter.rs#L306), [`LiveTerrain`](../../crates/server/state/src/obstruct.rs#L199), [`CachedTerrain`](../../crates/common/movement/src/cache.rs#L93), [`InRegion`](../../crates/common/movement/src/navigation.rs#L86), [`OpenWorld`](../../crates/common/movement/src/walk.rs#L270) |
| the trait | fifteen methods, **one required** (`can_step`); object-safe today |
| layers on one A\* edge | three — `CachedTerrain` → `Cluttered`/`LiveTerrain` → `MapTerrain` |
| production terrain stacks | **three**: server `LiveTerrain(MapTerrain<MapSnapshot, _>)`, client `CachedTerrain(Cluttered(MapTerrain<&Map, &TileData>))`, bake `MapTerrain` bare |

The last row is the whole argument for monomorphisation being cheap here.
`dyn` buys the right to not know the type, and there are three of them.

### What actually holds the `dyn` in place

Not the free functions — those are `&dyn` because the first caller was, and
nothing since asked. The one anchor with a stated reason is
[`FacetState::terrain`](../../crates/server/state/src/runtime.rs#L379), a
`Box<dyn Terrain + Send + Sync>` whose doc comment says:

> The ground is a [`Terrain`] trait object, not a concrete map: this crate sits
> below the client-file parsers, so it holds the *abstraction* of terrain and
> the world hands it the real thing (a `MapTerrain`) boxed.

**That reason has expired.** `openshard-state` already depends on
`openshard-uofiles` — the edge was added deliberately for `multi::Component`
and is documented in its `Cargo.toml` as
[`docs/customisation.md`](../customisation.md)'s C1. The crate is no longer
below the parsers, so it can name `MapTerrain` outright.

What holds the box up in practice is **tests**: about fifteen substitutions
across six files assign a hand-written double straight into
`facet_state_mut().terrain` — `Ground`, `Sea`, `Shop`, `BlindTerrain`,
`FrameTerrain`, `NamedTerrain`, `RaisedFloorTerrain`. Production constructs a
facet terrain in exactly **one** place,
[`boot.rs:664`](../../crates/server/server/src/boot.rs#L664).

That is worth naming as its own defect, because
[`scene.rs`](../../crates/common/movement/src/scene.rs)'s own header already
argues against those doubles in a different context: *"A fixture that
reimplemented the rule would agree with itself and prove nothing."* A test
terrain answering `can_step` with `Some(to)` is that fixture.

## The migration that does not break a caller

`&dyn Terrain` becomes `&T where T: Terrain + ?Sized`.

`dyn Terrain` satisfies that bound, so **every existing caller keeps compiling
unchanged** while the concrete ones start monomorphising. That is what makes
each phase below landable on its own, in any order after phase 0, without a
flag day. The `?Sized` bound is scaffolding: phase 4 removes the last `dyn`,
and the bound comes off with it.

`Terrain` stays object-safe until then — not as a goal, but because deleting
the property and the users in one commit is the flag day this avoids.

## Phase 0 — the oracle, before anything moves

**Nothing here is landable without this, because "faster" is currently
unmeasurable.** The only routing benchmark on record is synthetic: a 1024×1024
open world where the hierarchy is *slower* than flat A\* (0.974 ms p95 against
0.803 ms) — recorded in
[`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md).
No facet-0 measurement exists at all; that document has been carrying it as an
outstanding item since 2026-08-13.

Two probes already exist and neither has a recorded run on a real install:

- [`map_path_probe`](../../crates/common/movement/examples/map_path_probe.rs) —
  `find_path` and `find_path_toward` over a loaded facet.
- [`coarse_bench`](../../crates/common/movement/examples/coarse_bench.rs) —
  graph build plus long routes.

**Done when:** both have a committed run against facet 0 with the numbers in
this document — p50/p95/worst per route class, node counts, and
`TransitionCacheStats` hit rates — so every phase after this can be reported as
a delta rather than a belief. A phase that cannot show one is not finished.

## Phase 1 — the free functions

The searches themselves: [`path.rs`](../../crates/common/movement/src/path.rs)
(`find_path`, `find_path_toward`, `search` and the two `_until` variants),
[`walk.rs`](../../crates/common/movement/src/walk.rs)'s `step_allowed` and
`corner_open`, [`detour.rs`](../../crates/common/movement/src/detour.rs)'s
`Around::read`, and the whole of
[`navigation.rs`](../../crates/common/movement/src/navigation.rs) — build,
`component_labels`, `portals`, `intra_edges`, `region_costs`, `cross_portal`,
`find_long_path`. That is 30 of the 39 sites.

`find_long_path` takes **two** terrains (`guide` and `terrain`) and they are
genuinely different types, so it takes two parameters.

`InRegion` becomes generic over what it wraps.

**Done when:** no `dyn` in `common/movement` outside `cache.rs`, every caller
untouched, and phase 0's probes re-run.

## Phase 2 — `CachedTerrain`

[`CachedTerrain<'a>`](../../crates/common/movement/src/cache.rs#L30) holds
`&'a dyn Terrain` and is the outermost layer on the client's hot path, so it is
the one that turns a devirtualised search back into a virtual call on every
miss. It becomes `CachedTerrain<'a, T: Terrain + ?Sized>`.

**Done when:** `common/movement` names `dyn` nowhere.

## Phase 3 — `steer::Ground`

[`Ground`](../../crates/client/app/src/steer.rs#L314) holds three terrains —
`real`, `through_doors`, `guide` — and they are three different concrete types,
which is the one place in the workspace where the heterogeneity is real rather
than assumed. Three type parameters, not one.

**Done when:** `client/app` names `dyn Terrain` nowhere.

## Phase 4 — `LiveTerrain`, `FacetState`, and the last `dyn`

The expensive one, and the only one that needs a decision rather than a
rewrite.

- [`LiveTerrain`](../../crates/server/state/src/obstruct.rs#L141) holds
  `Option<&'a (dyn Terrain + Send + Sync)>`; it becomes generic over the map it
  lays obstacles over.
- [`FacetState::terrain`](../../crates/server/state/src/runtime.rs#L379)
  becomes the concrete server terrain. `Send + Sync` were bounds on the *box*
  and leave with it.

The work is not the field — it is the fifteen test doubles that assign into it.
Each is replaced by a real [`Scene`](../../crates/common/movement/src/scene.rs),
which builds a genuine `MapTerrain` from hand-placed ground, floors, stairs and
walls with no client files anywhere. Two things `Scene` does not do yet and
will have to:

- **Hand back an owned terrain.** `Scene::terrain()` returns
  `MapTerrain<&Map, &TileData>`; a `FacetState` field wants
  `MapTerrain<Map, TileData>`. An `into_terrain` is the addition.
- **Carry a multi table.** `boats`' `Sea` double answers `multi_components`.
  [`Multis::of`](../../crates/common/uofiles/src/multi.rs#L282) already builds
  one from hand-made `Multi` values, so this is wiring `with_multis` through
  the scene, not new parsing.

A double whose question a scene genuinely cannot ask is the one thing that
would reopen this phase's shape. **It is expected that none exists** — every
one of them answers a question about ground, statics or tiledata, which is what
a scene is — but the phase is written so that finding one stops it rather than
being worked around with a feature flag on a `#[cfg(test)]` variant, which is
the wrong answer and is named here so nobody reaches for it.

**Done when:** `grep -rn "dyn Terrain" crates` is empty, and the `?Sized`
bounds from phases 1–3 come off in the same commit.

## Phase 5 — the graph nobody reads

Unrelated to dispatch, and found in the same reading, so it is here rather than
lost.

[`boot.rs:615`](../../crates/server/server/src/boot.rs#L615) loads the baked
navigation graph, validates its dimensions against the map, and stores it in
`FacetState.coarse`. The only call to
[`coarse_router()`](../../crates/server/state/src/runtime.rs#L422) in the whole
workspace is in a test. Server AI plans with flat
[`find_path`](../../crates/server/ai/src/lib.rs#L79) at a budget of **400**
explored tiles — so a creature cannot route across a town, while the artifact
that would let it sits loaded and unread. The client, by contrast, does use it:
[`steer::Ground::path`](../../crates/client/app/src/steer.rs#L331) falls back
to `find_long_path` past 8 tiles.

Two honest answers, and the phase picks one on phase 0's numbers:

- **Use it.** `step_toward` gains the same fall-back the client has. The cost
  is that the graph is static — built without doors or placed items — so every
  hop still refines live, exactly as the client's does.
- **Stop loading it.** If server AI has no route class that needs it, the load,
  the validation and the resident graph are all waste, and `boot.rs` should not
  pay them.

What it must not stay is what it is: paid for, validated, and unread. A field
nobody reads is a claim the code makes and does not keep.

**Done when:** either `step_toward` routes through the graph and a test walks a
creature a distance flat A\* at budget 400 cannot, or the server stops loading
it and `FacetState.coarse` is gone.

## Decisions, taken here

**`dyn` goes everywhere, including storage.** Not "on the hot path only". A
generic search over a `LiveTerrain` that still holds `&dyn` for its map has
moved the virtual call inward, not removed it — every `can_step` pays it just
the same. Half of this refactor is worth less than none of it, because it costs
the same churn and leaves the reason for it in place.

**`?Sized` is scaffolding with an end date.** It exists so phases 1–3 do not
break callers, and it comes off in phase 4. Left in permanently it would be a
door back to `dyn` that nobody notices going through.

**Code size is not the objection.** There are three production terrain stacks.
A search monomorphised three ways is three copies of a bounded A\*, not an
explosion — and the fifteen-method vtable it replaces is itself a per-layer
indirection the optimiser cannot see through.

**The test doubles are the migration, not an obstacle to it.** Twenty-four of
the thirty `Terrain` implementors are test types. Most of them answer
`can_step` with `Some(to)` — a terrain that agrees with whatever it is asked,
which proves nothing about the rule under test. Replacing them with scenes is
worth doing on its own evidence, and this plan happens to need it.

**Phase 0 is not optional and not a formality.** The one benchmark on record
shows the hierarchy losing to flat A\* on an open map. A refactor justified by
"virtual calls are slow" that ships without a before and an after is the same
kind of claim.

## Out of scope, named

- **The statics layout.** `Vec<Vec<StaticItem>>` is 120,745 allocations and
  38.2 MiB where a CSR pair would be 2 and ~13.5 MiB —
  [direction B](new_map_representation/plan.md#b--our-own-chunk-format-and-a-uo-importer)'s,
  measured there, not reopened here.
- **Residency.** The whole facet is resident at ~150 MiB on both ends.
  [Direction G](new_map_representation/plan.md#g--residency-and-size-deferred-on-purpose),
  deferred on purpose.
- **A second hierarchy level.** Phase 3 of
  [`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md),
  and explicitly gated on the facet-0 numbers phase 0 here produces. This plan
  supplies the measurement; whether level 2 is justified stays that document's
  question.
- **`MAX_SEARCH_TIME` and the node budgets.** 50 ms inside one search, 400 for
  server AI, 600 for a client plan. Whether those are the right numbers is a
  question phase 0's data can finally be asked, and changing them before it
  exists would be guessing. Named so it is not mistaken for settled.
- **Merging the three ways of laying entities over the map** — `LiveTerrain`,
  `Cluttered`, and `net_command`'s multi expansion. Named as the natural
  successor in
  [`snapshot.md`](new_map_representation/snapshot.md)'s own out-of-scope list,
  and still is.

## Where a session starts

Phase 0. It needs a client install and produces the numbers every later phase
is reported against. Phases 1–3 are independent of each other and of phase 5;
phase 4 is last because it is the one that ends the `?Sized` scaffolding.
