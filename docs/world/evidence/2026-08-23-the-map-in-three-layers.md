# 2026-08-23 — nine plans, three eras, and a map with three layers

A documentation session with no code in it. It started as *"which plan is next"*
and turned out to be *"nothing here says what a map **is** at runtime"* — the
storage answer had largely landed, the runtime answer was never written down, and
so the order between nine plans was the order they happened to be written in.

Four commits: `400e8b5e` the consolidation, `dfd18c1c` what a publish does to a
raised tile, `dd1ab7e3` what a search node is, `9bfea62a` one floor to another
over one column.

## Where it stands

Two new documents and one new folder:

| | |
|---|---|
| [`map_rebuild.md`](../../archive/world/map_rebuild.md) | the area's **entry point**: the three layers, the three eras, the decisions taken between the plans, and what each older document becomes |
| [`realtime_map.md`](2026-08-23-era-r-the-map-you-hold.md) | **era R, executable**: what moves where, in which commit, with the DoD and the risk per node |
| [`handoffs/`](.) | this folder. Era S keeps its own in `new_map_representation/handoffs/` |

**The map is three layers** — ground, statics, and the live layer over them,
ordered by how fast each changes — with one invariant that the type system is
meant to carry rather than each bake remembering: **what may be baked is exactly
what is below the live layer.**

**The eras are R → P → S.** R is the runtime map (the tile table out of the file
reader, the live layer joining the type, a house with floors, the statics as one
run, one load per install). P is the search over it —
[`navigation_spans.md`](../design_spans.md), unchanged in substance. S is
[`new_map_representation/`](2026-08-31-the-base-set-track.md), resumed: the live
publish, revisioned bakes, chunks to the client, the editor.

**Nothing was built.** The workspace is untouched by this session; every change
is under `docs/`.

## What was decided

- **The crate rule is struck.** *"`openshard-map` depends on
  `openshard-protocol` and nothing else"* was a proxy for the property that
  matters, and the property replaces it: **the map crate opens no files.** Struck
  in the five places it was written. A world made of tiles that may not name the
  tile table cannot say what it is made of.
- **`TileData` and `stand_surfaces` leave `openshard-uofiles`** — a table is not
  a file, and a rule is not one either. This is phase 3's own argument applied to
  what phase 3 left behind. The tile table gets its own crate; the surfaces walk
  goes to movement, where its consumer is.
- **A house is the live layer, never a patch**, closing the last open row in
  [`mechanics.md`](../design_snapshot.md). Its floors become
  `CoverKind::Stands`, which is the half nobody had built: the only producer of a
  standable cover in the workspace is a ship's plank, so a house's upper storey
  stands on nothing.
- **The base is immutable and everything that changes is a layer over it** —
  which is what makes the CSR statics layout free rather than costly, and what
  makes a publish a rebuild of touched blocks rather than a memmove.
- **Statics: CSR now, packing later.** Two allocations and an immutable base are
  architecture; four bytes a record is an optimisation with an API cost, gated on
  N3's measurement.
- **`plan.md`'s order is superseded past C**, and
  [`navigation_spans.md`](../design_spans.md)'s gate moved from
  `terrain_seam.md` — which closed — to R1 and R2, which reach further into it:
  N1 is built from the two types those nodes move.
- **A raised tile of ground is answered by a rebake, never by an overlay.** A
  ground overlay would have to be *in* the bake for the bake to be right, so it
  is not a live layer at all — it is the base with a slower spelling.

## What was found

Four things, all of them by reading the workspace rather than the plans:

- **🚩 The search's node is a planar tile, and a column with two standing places
  has one slot in the closed set.** A bridge and the ground under it are one
  node. The sharpest form is a *wrong answer*: asked to path from one floor of a
  column to another, `search` flattens both endpoints, finds `start == goal` and
  returns **arrived with an empty route** — so an NPC told to walk to a mobile on
  the bridge above it believes it is already there. Filed as
  [N3b](2026-08-25-the-span-layer.md#n3b--the-node-stops-being-a-tile), which the census
  makes nearly free: `(x, y, span)` with the index zero for 99.4% of the facet,
  twenty-nine bits of the `u32` the key already is.
- **The step rule is asymmetric and the coarse graph cannot say so.** A climb
  reaches `start_top + 2`; a descent is unbounded. But a portal is made only
  where `step_allowed` succeeds in *both* directions, so every ledge a body may
  step off but not back onto is invisible to long routing. N4 builds directed
  edges; N5 goes back to being teleporters rather than hand-declared drops.
- **Direction D is further along than its plan says.** The navigation bake and
  the building flood already carry a `MapRevision` and refuse themselves on a
  mismatch. The occluder measurements are still file-keyed, and the radar cache's
  revision dimension has no production writer at all.
- **Two types are called `LandTile`** — the entry in `uofiles::tiledata` and the
  id in `openshard-map` that indexes it. They have never met because they were in
  different crates; R1 puts them in one, so the id becomes `LandTileId`.

## What is next

**R1, commit 1** — [`realtime_map.md`](2026-08-23-era-r-the-map-you-hold.md#r1--the-table-leaves-the-file-reader).
The new crate and the move, naming nothing outside the two crates it touches;
the ~120 call sites are commit 2 and compiler-led. No behaviour changes and every
mistake is a compile error, which is why it goes first.

**What would block it:** nothing. It needs no client install to write, and the
tests that need one already skip without `OPENSHARD_CLIENT`.

**What not to start:** anything in era P. `Spans` is built from the two types R1
and R2 are moving, and written before them it is written twice — which is the
same wait that plan already served once for node E.

## Left open, deliberately

- **The publish window.** A revision becomes visible before the rebake over its
  touched chunks finishes. Today's rule degrades routing there to flat A\*; the
  alternative is a publish that carries the rebuild and pays the latency. Wants a
  measurement of one region's rebuild against the 96 s whole-facet bake.
- **Whether bodies block.** The client refuses to route through an NPC and the
  shard permits it, on purpose. It is a gameplay decision and the layer carries
  either answer.
- **Which components of a shipped multi are floors.** R3 reads the platform flag;
  a house whose floor that flag does not mark is a `findings.md` entry.
