# Superseded: coarse HPA*-style routing

> **Status: superseded — a record.** The first pass at long routes; what shipped
> is described in [`navigation_graph.md`](../design_navigation_graph.md) and what replaces
> the model is [`navigation_spans.md`](../design_spans.md)'s N4. Entry point for
> the area: [`map_rebuild.md`](../../archive/world/map_rebuild.md).

This was the first long-distance routing experiment. It is superseded by the
automatic, topology-derived navigation graph in
[`navigation_graph.md`](../design_navigation_graph.md): the replacement has no fixed
square-cluster decomposition and no multi-level hierarchy.

## Landed — static HPA*-style guide, with live refinement

The first implementation is now in `crates/common/movement/src/coarse.rs` and is wired into the
client's Ctrl-click planner. The server constructs the same graph beside each mapped
`FacetState`, exposes it through `FacetState::coarse_router()`, and deliberately has no long-range
AI consumer yet — chase remains its short-range bounded A*. `cargo run -p openshard-movement
--example coarse_bench --release` is the host-cost probe for build/query tuning.

1. **Diagonals / corner-cutting.** Settled by the "why not the crate" decision below: the coarse
   graph's edges are real `find_path` costs, so corner-cutting is exactly as correct there as it
   already is everywhere else in this codebase. Restated here as an explicit spec, not just
   prose, so it doesn't get renegotiated by accident later.
2. **Doors — walk up and stop, or open it, depending on who's walking.** Settled as two
   caller capabilities:
   - The **client/player** (`steer.rs::plan()`) cannot open a door itself — a human
     double-clicks it. The existing behaviour (walk the open half, stop in front of the shut
     leaf) is already correct and `find_long_path` should preserve it exactly, not invent a
     second one.
   - A **server-side AI walker** (`ai::step_toward` and friends) already has
     `through_doors`/`body_opens_doors` — per [`roadmap/06-gameplay/ai.md`](../../roadmap/06-gameplay/ai.md), humanoids open unlocked doors
     in their way as part of normal movement. Whatever eventually calls `find_long_path`
     server-side needs to route through this existing capability rather than stopping like the
     client does — "if it can open it, it opens it" is a caller-side fact, not something the
     coarse layer should hardcode either way.
   - **Settled:** `find_long_path` takes the *guide terrain* used to join query endpoints and the
     *live terrain* which authorizes each refined step, not a boolean. The client supplies its
     static `MapTerrain` guide plus either real or doors-open live terrain; a future server AI
     does the same with `planning_terrain(through_doors)`. Door capability remains the caller's
     choice and no second policy is added to `movement::coarse`.
3. **Replanning when the world changes (a door opens, a portal appears) — explicitly deferred.**
   Not for this pass. Noted so it isn't lost: a long walk in progress currently has no way to
   notice a shortcut appeared mid-route: nothing here recomputes eagerly. Whatever the
   caller-side retry/replan cadence ends up being (something like `steer.rs`'s existing
   stuck-step/replan-on-refusal idiom, generalized) is a separate design pass, after the above
   two are settled and this stands on its own first.

---

## Context

Today's `movement::find_path`/`find_path_toward` (`crates/common/movement/src/path.rs`) is a
bounded A* — `PATH_BUDGET`/`PLAN_BUDGET` a few hundred tiles — fine for creature chase
(`ai::step_toward`, ~400 budget, chase range is well inside that) and for an ordinary
click-to-walk. It was never meant for a route across a whole facet: its own doc already says
so ("open-world roaming would want caching, not a bigger cap").

We're adding that caching layer: a precomputed coarse connectivity graph that turns "is there a
way, roughly, and which way" into a graph lookup, feeding a handful of waypoints to the
*existing*, exact, live-aware `find_path` for the real walked steps. The one confirmed real
consumer today is the client's own click-to-walk (`crates/client/app/src/steer.rs`'s `plan()`),
which currently gives up on a destination past `PLAN_BUDGET` (600 tiles) and only walks "as
close as the ground allows" (`find_path_toward`). Long clicks working end-to-end is the actual
ask.

### Why not the `hierarchical_pathfinding` crate

We looked at it first (it's the closest match on crates.io: HPA* over a generic grid, MIT,
`tiles_changed()` for incremental updates). Reading its actual API ruled it out:

- `Neighborhood::get_all_neighbors(point, &mut Vec<(usize,usize)>)` receives **only a bare
  coordinate** — no way to know which tile a move is *from*. Corner-cutting — the rule
  `movement::step_allowed`/`corner_open` enforces, that a diagonal may not slip through a corner
  where two blockers meet — cannot be expressed inside it at all, for any neighborhood, built-in
  or custom. Using it would mean either quietly reproducing the exact bug class `docs/client.md`
  already documents once (a terrain with no corner rule believing a diagonal legal, rubber-banded
  by the shard), or crippling the coarse graph to 4-directional-only as a workaround.
  Unmaintained since 2021 (~70 downloads/month) on top of that.
- Writing this ourselves is not much more code than integrating the crate *correctly* would have
  been, and it buys real correctness instead of a workaround: the coarse graph's own edges are
  computed by calling the SAME `find_path`/`step_allowed` that already gets corner-cutting, z,
  and doors right. This is the same call this codebase already made once — `path.rs`'s own A* is
  hand-rolled rather than built on the generic `pathfinding` crate, for the identical reason
  (exact control over the rules this game actually plays by).
- No new dependency, and determinism is no longer a separate concern to test for — it falls out
  of using our own already-deterministic `find_path` as the building block, as long as the graph
  itself avoids raw hash-map iteration order leaking into output (see below).

**Cross-checked against `anima-client`** (`crates/anima-core/src/path/mod.rs`, the pathfinding
of the one reference project in `CLAUDE.local.md`'s links actually built around A*): also
hand-rolled, no third-party crate, deliberately dependency-light for a WASM build, and it
enforces the same both-flanks-open corner rule ours does. It also treats a closed door as
passable during *planning* and reacts to the real closed door only when a route executor reaches
it — the same "plan optimistically, walk pessimistically" split this plan already uses for
`steer.rs::plan()`. No coarse/hierarchical layer there to borrow from (it's architecturally
flat, single A* only) — but independent confirmation that both calls here (hand-roll it, keep
doors open while planning) are the ones a comparable serious project made too.

**Second cross-check, `broker0/path_server`** (also in `CLAUDE.local.md`'s links — a standalone
HTTP pathfinding service a shard can query, `src/world/surveyor.rs::trace_a_star`): same result.
Also a flat, single-resolution A* with no precomputed structure at all — every query re-searches
the raw tile grid live. Hand-rolled, no third-party pathfinding crate, careful about
corner-cutting/multi-z/doors via configurable flags. Grepped the whole repo for
cluster/region/hierarch/navmesh/waypoint — nothing. Between this and `anima-client`, **no UO
ecosystem project in our references does anything hierarchical** — this design has no prior art
to lean on, it's genuinely new ground for this problem space, not a reimplementation of an
existing pattern.

## Doors: "potentially passable but currently closed", for free

Unchanged by dropping the crate — this was always about *what terrain the graph is built from*,
not which library builds it:

- **Server.** `crates/server/state/src/obstruct.rs`'s own doc: "the doorway it stands in is an
  open gap in the statics by construction". A door is a live entity, registered only in
  `Obstructions`, never in `FacetState.terrain` (the static `MapTerrain`; the field is
  `FacetState.map`, a `MapSnapshot`, since [`terrain_seam.md`](terrain_seam.md)'s D) — confirmed
  by search, every production write to it happens exactly once, at facet load. Build the coarse
  graph from the map alone and every doorway is simply open ground to it, no special case.
- **Client.** `crates/client/app/src/clutter.rs` + its call sites in `lib.rs` (e.g. line 2076)
  confirm the same split: `App` holds `self.map`/`self.tiledata` — the bare client files —
  entirely separately from `Clutter`, which is what lays the shard's placed items (doors
  included, arriving as `0x1A` entities, never part of the static art) on top to get `Ground`.
  So `MapTerrain::new(&self.map, &self.tiledata)`, no `Clutter` involved, is already exactly the
  door-open, clutter-free base the coarse graph wants — the same two fields `Clutter::over`
  already takes, nothing new to build.
- **What this doesn't cover, on purpose:** a crate dragged into a doorway is `Obstructions`/
  `Clutter`, not `terrain`/`self.map` — invisible to the coarse graph the same way a shut door
  is. That's fine: the coarse graph only ever proposes a corridor; `find_long_path`'s hop-by-hop
  `find_path` calls are what actually check live ground and refuse to walk through a real
  obstruction, door or crate alike.

## Design

### 1. `movement::coarse` (new module, `crates/common/movement/src/coarse.rs`) — no new dependency

Grounded directly in the original paper (Botea, Müller, Schaeffer, "Near Optimal Hierarchical
Path-Finding", *Journal of Game Development* 1, 2004) — read primary-source, not a secondhand
description — with one correction where blindly copying it would have imported a wrong
assumption about our own game:

- **Chunking.** Partition the facet into fixed, disjoint rectangular clusters — start at 32×32
  tiles, tune against the bench below (the paper swept this too; their sweet spot on 50×50–320×320
  maps was 10×10, not directly transferable to a 7168×4096 facet, but confirms it's a real knob
  worth measuring rather than guessing once).
- **Entrances — the paper's actual, formal definition, not an earlier looser "collapse
  contiguous runs" phrasing.** For each border between two adjacent clusters, an entrance is a
  *maximal obstacle-free segment* along that border: symmetric (a tile and its mirror across the
  border are either both in it or both out), contiguous, and extended in both directions as far
  as those hold. Per entrance, one or two **transitions** (graph nodes): if the entrance's width
  is under a threshold (the paper used 6, tunable), one transition at its midpoint; otherwise
  two, one at each end. So a wide-open border gets two nodes (hug either side), a narrow gap —
  most doorways — gets one.
- **The graph.** Two edge kinds, per the paper: an **inter-edge** links the two transitions of
  one entrance across the border (cost 1, they're adjacent tiles). An **intra-edge** links every
  pair of transitions *within the same cluster*, weighted by the cost of the optimal path
  between them inside it. **Correction to the paper, deliberate:** they weight a diagonal move at
  1.42 (≈√2, an Euclidean-flavoured cost) against 1 for a cardinal — correct for a game where a
  diagonal covers more ground *and* costs more time. Ours doesn't: `WALK_HOLD`/`RUN_HOLD` charge
  every step, diagonal or not, the identical fixed interval (confirmed in `pace.rs`), which is
  the whole reason `path.rs` uses Chebyshev, not Euclidean, as its heuristic. So intra-edge cost
  here is **step count from our own `find_path(terrain, a, b, budget)`**, not the paper's 1/1.42
  weighting — copying their number would quietly bias the coarse graph toward routes that are
  wrong for how this game actually spends time. This also gets corner-cutting right for free,
  since `find_path` already enforces it — no separate correctness argument needed for that part.
  A chunk is small (32×32), so this is cheap: well inside `PATH_BUDGET`.
- **The graph's own storage.** A dense, indexed adjacency structure (`Vec<Vec<(TransitionId,
  u32)>>`), not a raw `HashMap` walked by iteration order — keeps construction and query output
  deterministic by construction, not by luck.
- **Query — `CoarseRouter::waypoints(from, to) -> Option<Vec<Point>>`.** The paper's own
  procedure: temporarily connect `from` to every transition in its own cluster (one local
  `find_path` call per transition, edge weight = that path's length), same for `to`, then run
  ordinary A* over the now-augmented abstract graph between them — same tie-break discipline as
  `path.rs::search`, so this stays deterministic the way that one already is. Short queries
  (`from`/`to` in the same or an adjacent cluster) skip the graph entirely and answer straight
  from `find_path` — the paper notes plain A* already wins for short/direct routes, and their own
  numbers show the overhead of inserting `from`/`to` dominates exactly when the search itself
  would have been cheap. The result is a transition sequence, returned as waypoints — a corridor,
  not a walk order. (The paper also offers optional path-smoothing over the *refined* low-level
  path, for when cluster-crossing produces a visible zigzag; not needed here unless the bench
  shows one, since each hop below is already a full, real `find_path` and not a naive
  cluster-centre-to-centre line.)
- **Dynamic changes — informs, not needed for v1.** The paper's own answer for a topology change
  ("a bridge blows up"): re-derive only the affected cluster's intra-/inter-edges, not the whole
  graph. Direct grounding for open spec item 3 (portal appears / door state affecting the coarse
  graph) when that gets designed — not needed now, since this graph is built once from the
  static base and never touches doors at all (see the Doors section).
- **`find_long_path(terrain, coarse, from, to, hop_budget) -> Option<Vec<Direction>>`** — chains
  the existing `find_path` hop to hop between consecutive waypoints over live `terrain`,
  concatenating directions, cutting at the first hop the live ground actually refuses — the same
  "plan optimistically, walk pessimistically, stop at the real refusal" idiom
  `steer.rs::plan()` already uses for shut doors. Re-derives waypoints from the current position
  on a hop failure rather than giving up outright.

### 2. Server wiring (`crates/server/state/src/runtime.rs`)

- `FacetState` gets `coarse: Option<CoarseRouter>`, built alongside `terrain` at facet load
  (`crates/server/world/src/tick.rs`, next to the existing `terrain: Some(...)` assignment) —
  same lifecycle, same "never touched again" guarantee.
- `FacetState::coarse_router(&self) -> Option<&CoarseRouter>` accessor. **Not** wired into
  `ai::step_toward` — chase/homing is short-range and already fast; nothing there needs this.
  It's there so the capability is real and reachable, not dangling with no consumer at all.

### 3. Client wiring (`crates/client/app/src/steer.rs`)

- `plan()` (line ~1133) currently: try `find_path(ground.real, ...)`, then
  `find_path(ground.through_doors, ...)` cut at the first live refusal, then
  `find_path_toward(ground.real, ...)` as a last resort. Insert the coarse-guided attempt
  **between** the first failure and the `through_doors` fallback: on a destination `find_path`
  can't reach within `PLAN_BUDGET`, ask the client's own `CoarseRouter` for waypoints and walk
  `find_long_path` against `ground.real`. Falls through to the existing
  `through_doors`/`find_path_toward` handling exactly as today if the coarse attempt also comes
  up empty.
- `App` builds `CoarseRouter::build(&MapTerrain::new(&self.map, &self.tiledata), ...)` once, at
  map load, and holds it in a field beside `self.map`/`self.tiledata` themselves.

### Explicitly out of scope

- `ai::step_toward`/creature chase — untouched, doesn't need this.
- `npc`/`quests` — zero current callers of any long-distance path; nothing to wire.
- Live invalidation — not needed given the static-only design; if housing later mutates
  the facet's map directly (it doesn't today — stub crate), this assumption needs
  revisiting then, not now.

## Verification

- `cargo test -p openshard-movement` — new `coarse` module tests: entrance detection on a
  synthetic chunk border (single run vs. two doorways collapsing to one vs. two entrances),
  waypoints on open ground, waypoints routing around a large synthetic wall, a doorway staying
  "open" to the coarse graph while shut in `Obstructions`/`Clutter`, plus a 64-case
  property run over randomized narrow gates at five cluster borders. Each generated route is
  checked against an exhaustive ordinary A* reachability answer, then replayed through
  `step_allowed` so the coarse guide cannot turn into an illegal walk.
- A bench (`crates/common/movement/benches/` or an `examples/` binary, following the project's
  existing "measured, not asserted" convention — see the scripting-hook numbers in
  [`roadmap/05-scripting.md`](../../server/evidence/2026-08-24-the-scripting-spike.md) and the LOD ones in
  [`client/design_lod.md`](../../client/design_lod.md)) comparing `find_long_path` against a plain `find_path` with a raised budget on a large
  synthetic map, to justify `chunk_size` with a number rather than a guess.
- `cargo test -p openshard-state` / `-p openshard-client-app` for the two wiring points.
- `cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`, `cargo fmt --all` (all expected silent per `CLAUDE.md`).
- Manual: `cargo run -p openshard-playground`, click-to-walk a destination further than 600
  tiles away across open ground, confirm the body actually walks the whole way instead of
  stopping at the old budget.
