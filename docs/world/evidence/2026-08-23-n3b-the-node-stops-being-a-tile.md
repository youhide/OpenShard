# 2026-08-23 — N3b: the node stops being a tile

Era P's fourth node, in one commit.
[`navigation_spans.md`](../design_spans.md)'s N3b is the node N3 was written
to make safe: N3 banked a 5.3× search on the promise that **nothing about the
routes changed**, and this is the one node that must change them. A node is now
a *place to stand* — the tile and the height on it — so a column with two floors
gets two slots in `closed`. **178 destinations round Britain that used to report
an arrival are now the refusals they always were, 176 of them enumerated to a
column with more than one surface, and a route from a house's ground floor to
its first floor exists where it used to be answered with success and an empty
route.**

## Where it stands

The same three origins and 37,248 destinations each, run **before and after in
one tree** — the before by putting `HEAD`'s `path.rs` back for one run, which
reproduced every recorded number to the unit:

| origin | arrived @400 | @600 | answers that moved |
|---|---:|---:|---:|
| (1363, 1600, 30) the castle plateau | 4,036 → **4,010** | 4,436 → **4,405** | 26 and 31 |
| (1434, 1699, 2) the bank | 6,138 → **6,091** | 7,389 → **7,315** | 47 and 74 |
| (1500, 1900, 0) open country | 17,458 → **17,458** | 18,093 → **18,093** | **none** |

**Open country is bit-identical**, which is the control: nothing out there
stands on anything. Every one of the 178 that moved lost an arrival and none
gained one, and the probe attributes them itself — **176 are columns with more
than one place to stand**. The two that are not are named below.

**`coarse_bench` reproduces every recorded number to the unit** — 37 of 44
refused-but-walkable from the castle, 5 of 43 from the bank, 0 of 38 from open
country — so the hierarchy is untouched. That was the risk worth checking:
refining a coarse hop now has to arrive at the graph node's own height, which is
`ground_z`, so a hop onto a bridge deck could have started refusing.

**`cargo check --workspace --all-targets`, `cargo clippy --workspace
--all-targets` and `cargo fmt --all` are silent.** `cargo test --workspace
--no-fail-fast` ends with the same two red tests in `openshard-state` and no
others — R1's finding, still filed under [*`can_step` does not check the
corner*](../../roadmap.md).

## What the node decided

**The key is `(x, y, z)` and not `(x, y, span)`, which is a correction to the
plan rather than a choice inside it.** N3b's section asked for a span index in
the twenty-nine spare bits of the `u32` the search already used. That cannot be
the key: **the surfaces a search lands on are not all the map's.** A house's
storey, a ship's deck and a placed stair are the `Overlay`'s, `walk::climbed`
picks them, and none of them has a span — a span is a fact about the map file.
The height is what both layers speak, because a landing *is* a height. Forty
bits of a `u64`, so nothing has to be truncated either. Recorded in the plan as
a correction, and carried to N4 with a warning attached: N4 is about to key a
graph by spans, and the same argument applies to it.

**A destination is a point; a node is a place; resolving between them is half
the node.** Comparing nodes without it would swap one wrong answer for another,
because almost no caller has the exact z of the surface it means — the coarse
graph's nodes carry the land's height under a bridge they mean the deck of, the
probe sweeps a neighbourhood at its *origin's* height, a click carries whatever
the tile was drawn at. `path::goal_node` resolves against what is actually
there, the map's spans and the live world's surfaces together, with the tie
broken by the lower surface so the answer does not depend on which layer was
read first. **The start's own column offers the start's own height**, because a
body standing somewhere is proof that there is somewhere to stand.

**The two destinations that are not multi-span columns are budget, not rule.**
(1403, 1718) and (1402, 1719) from the bank at budget 400: single-surface both,
their goal used to be found on the **400th node**, and it now falls one node
outside the budget because the search spent a node or two on the second height
of a column on the way. Neither is lost at 600, and all 74 lost at 600 are
multi-span. The plan asked for the changed set to be enumerated rather than
counted precisely so that these two would have to be explained, and they are —
`--dump` on the probe writes one line per destination so two runs can be
diffed, which is how they were found.

**The route that could not exist before now does, and it is asserted over a
house rather than argued.** Nothing in UO moves up in place, so a route between
two floors of one column is a **loop** — out, up, and back over the same
`(x, y)`. `a_route_climbs_from_a_villas_ground_floor_to_its_first_floor` plans
one over the same two-storey villa whose steps the rule's own test climbs by
hand, and walks the plan back through `step_allowed` so the search cannot invent
a step nobody may take. Three more in `path.rs` over an overlay-only mezzanine:
the loop, the refusal when the tread is taken away (**the empty-route lie**),
and the goal's height resolving to the nearer of two places.

**One search, and no second copy of anything.** `steps_out_of` is untouched,
`reconstruct` walks the same chain over places instead of tiles, and the popped
entry now *is* the node — so the `came_from` lookup that used to recover "where
was this tile, really" is gone along with the `Point` it stored.

## What was found

Filed in [`navigation_spans.md`](../design_spans.md)'s *Out of scope,
named*.

- **🚩 A node budget is not a tile budget, and 400 was measured against tiles.**
  `budget` bounds finalised *nodes*, and a column with two floors can be
  finalised twice. Measured cost: the two destinations above. Filed rather than
  fixed, because the argument for 400 and 600 is a *time* budget and the
  measurement that would move them is `terrain_seam.md`'s, not this one — but
  whoever revisits those numbers should know the unit changed under them.
- **The probe's `revisits` count is zero over the bare map, and that is the
  sweep rather than the world.** It counts routes that come back to a column at
  a second height — the thing no tile-keyed search could plan — and finds none,
  because the sweep runs with an empty overlay and the map's own multi-span
  columns are bridges you walk along. A house is the shape that produces one.
  Do not read that zero as "no such routes exist".
- **`Overlay::surface_at` breaks a tie by iteration order.** Two surfaces
  equidistant from the height asked about resolve to whichever the `Vec` yields
  first. `goal_node` does not inherit it; the overlay's own resolver still has
  it. One line, in a crate this plan does not otherwise touch.
- **`target/debug/incremental` had reached 80 GB and filled the disk mid-run.**
  Not a map finding, but it stopped this session for a minute and the failure
  mode is worse than it looks: a write on a full disk can leave a source file at
  zero bytes. Deleted; 81 GB back.

## What is next

**[N4 — regions over spans](2026-08-25-the-span-layer.md#n4--regions-over-spans)**,
and nothing else in era P is open before it. It is the user-visible repair and
the one N3b could not reach from inside a fine search: `coarse_bench` still
refuses **37 of 44** walkable destinations from the castle plateau, unchanged to
the unit, because `NavigationGraph::build` samples `ground_z` — the land alone —
once per tile, so the plateau is an island in a graph whose own map says
otherwise. Its two halves are spans as the graph's nodes and **directed edges**,
so a ledge a body may step off but not climb back onto stops being deleted for
not being symmetric. Its done-when is a number the bench already prints.

**Carry the key's lesson into it.** N4 keys a graph by spans, and spans are the
map's surfaces only — the live world's floors have none. The graph is baked from
the bare map and the overlay is applied when a hop is refined, so that is
survivable; it is worth keeping deliberately rather than rediscovering.

**What would block it:** nothing.
