# 2026-08-24 — the search gets in a hurry

Three commits and a survey. The previous handoff closed the *cheap node* question
and said the lever left is **fewer nodes**; this session took the cheapest way
there, took what was still loose inside `explore`, priced the rest of the field
against this codebase rather than in the abstract — and then, asked what is left
in the 76% that is terrain, found that the four refusals guarding it were all
about *baking more data* and none of them about the land grid's own arithmetic.

**A body's route reaches a quarter further inside the same node budget, and the
node it spends is 2.3% cheaper on top of the 2–6% the packing gave.**

## The measurement this starts from

`perf record` over `map_path_probe`, 16,640 destinations from Britain's castle,
the `profiling` profile. Nothing here is inherited: the numbers are today's.

| | |
|---|---|
| `explore` — A\*'s own heap, hash, packing and loop | **18.6%** |
| the terrain half, below | **~76%** |

| | |
|---|---|
| `WorldMap::land_corners` | 17.7% |
| `walk::landing` | 13.1% |
| `Spans::ground` | 10.4% |
| `SpanIndex::stored` | 9.1% |
| `Spans::check` | 7.5% |
| `walk::climbed` | 4.4% |
| `walk::steps_out_of` | 4.2% |
| `Overlay::blocker_at` | 3.8% |
| `WorldMap::statics_at` | 3.1% |
| `MapTerrain::start_surface` | 2.6% |

`land_corners` at the top of that list is the thread the last section of this
handoff pulls on.

And inside `explore`, nothing dominates: hashbrown is ~2.8% of the whole
program, `BinaryHeap` ~3.3%, and the rest is straight-line loop. **That is a
measured refusal of the two obvious micro-ideas** — a bucket/dial queue for the
open list and a flat array for `visited` — because a *perfect* heap and a
*perfect* table together are worth about 6% of a run.

The probe's own shape says the same thing louder: at radius 96 from the castle,
**30,679 of 37,248 destinations exhaust the budget and never arrive**. The cost
is not the node. The cost is the search that spends all of them for nothing.

## What was done

### `explore` was packing the same node twice ✅

Four things, all answer-preserving:

- The open entry's bottom forty bits **are** `PathNodeKey` with the height's sign
  bit flipped. A push was packing the coordinates the key had just packed, and a
  pop unpacked them to pack them again. The entry carries the key now; the pop's
  goal test is a `u64` compare and only the node actually expanded is unpacked
  back into a point.
- The region bound was an `Option<Region>` read once per **neighbour**, eight
  times a node, `None` in every search but the corridor's — 1.9% of the whole
  probe. It is opened once a node.
- The neighbour loop indexed by `Direction::to_bits` an array already in
  `Direction::ALL`'s order: a bounds check for a pairing a zip states.
- `heuristic` and `manhattan` took each coordinate's difference twice. One
  `estimate` over one pair of deltas.

**74,496 searches dumped per destination and compared byte for byte: identical.**
Interleaved against the pre-change binary, 2–6% faster.

### And then it was taught to be in a hurry ✅

`Weight` — how far a search may over-trust its estimate, as a ratio and not a
float because the tick is replayable. `Weight::PLANNING` is **5/4**, measured
over 33,280 destinations from two origins at both shipped budgets:

| | castle, 400 | castle, 600 | open country, 400 | open country, 600 |
|---|---|---|---|---|
| destinations reached | **+24.7%** | **+23.3%** | +6.8% | +3.0% |
| total route length | +0.20% | +0.32% | +0.19% | +0.27% |
| routes longer at all | 195 of 2,828 | 352 of 3,223 | 613 of 10,143 | 845 of 10,516 |
| the worst one | +2 steps | +4 steps | +3 steps | +3 steps |
| arrivals **lost** | 0 | 0 | 0 | 0 |

**5/4 at budget 400 arrives at more destinations than the exact search at 600** —
3,527 against 3,223 — at two thirds of the worst-case cost. That is the first
real thing anyone has been able to say about the two node budgets, which
[`terrain_seam.md`](../research/terrain_seam.md) has had filed as unargued since it was
written.

The two origins answer different halves and both are needed: at the castle the
**budget** refuses and a weight spends it better; in open country the **map**
refuses — water, cliff — and the weight saturates by 9/8 with nothing left to
reach. 3/2 buys +36% for +0.34%; 2/1 buys +49% for +2.36% and a route twelve
steps over the shortest. 5/4 is the last rung where no single route is stretched
past a step or two.

**The weight is named at the call.** A body's own route — the AI's chase, the
client's click, every hop a corridor refines into — is `Weight::PLANNING`.
Anything that has to *compare* two answers is `Weight::EXACT`: the graph's baked
edge costs, the probe, and every test that means "the shortest". A baked cost is
a statement about the facet, and a corridor picks between hops by comparing them,
so it may not be the length of a route that merely happened to be short enough.

### And the terrain half turned out to have arithmetic left in it ✅

**The four refusals were all about baking more data.** The full adjacency
record, the rejection mask, the dense `average_land_z`, the locality hoist
across the span seam — every one of them proposed *storing* something, and every
one was priced and declined. None of them was about what the land grid already
does, and `land_corners` is the hottest symbol in the whole profile at **17.7%**.

It read four cells with four full derivations. A cell index is a bounds check,
two remainders, a multiply by the block column's height and two shifts — and a
block is row-major inside, so the four cells a tile's corners are made of sit at
`+0`, `+1`, `+8`, `+9` of one another whenever the tile is not on its block's
eastern or southern edge. **That is 76.6% of a facet**, `(7/8)²`.

**No edge of the facet reaches the fast path, and that is a consequence rather
than a check**: a facet is a whole number of blocks, so its last row and column
are exactly the ones the guard refuses. Which is why `corner_quad` returns four
cells rather than four options — every one of them is on the facet.

The other half is a read that was simply made twice. `Spans::ground` — the tier
that answers 92% of columns — asked `land(x, y)` for the graphic and then
`land_corners(x, y)`, which reads `(x, y)` again for its own `own`.
`land_and_corners` hands back both from one walk. The bake's column builder was
doing it twice over: four reads where one does.

`step_cost`, `--repeat 25`, least of three runs, open country:

| | before | after | |
|---|---|---|---|
| `surface_at`, one column | 12.5 ns | **10.5** | −16% |
| landings off the bake | 157.8 ns | **145.4** | −7.9% |
| all eight on one column | 139.2 ns | **127.0** | −8.8% |
| expansion from a stored column | 178.2 ns | **166.2** | −6.7% |
| **`steps_out_of`, a whole node** | **201.1 ns** | **196.4** | **−2.3%** |
| `map.land`, one read | 1.2 ns | 1.2 | — |
| landings over the map | 373.1 ns | 374.9 | — |

**The last two rows are the control**, and they are what makes the rest a reading
rather than drift: one read cannot get cheaper, and *landings over the map* does
not go through `Spans` at all. Both stayed put while everything through the
changed path fell. Answers dumped and compared byte for byte: identical.

**And the same run explains a refusal that had never been explained.** A dense
`average_land_z` table was measured and declined, and it could not have helped
whatever it cost: `ground` needs the corners' **minimum** as well as their
average — `reach_z` is what a step has to climb to — and an average table does
not carry it. To replace `land_corners` a table would have to hold both, which
is 2 bytes over 29.4M columns, **58.8 MB**.

## What the industry does, and what each is worth here

The question this session was opened with. Ranked by what it would buy *this*
codebase, with the reason it is or is not applicable.

### 1. Do not search — precompute the answer, and why the whole facet cannot

**Everything in this family is one idea with one obstacle.** The idea: answer
the query from a table instead of a search. The obstacle: the table a query
wants is *the world joined against the world*, and the join does not survive a
facet.

The number that decides it is measured — `span_census` over facet 0, which is
7168 × 4096:

| | |
|---|---|
| columns | 29,360,128 |
| **standing places a walker has** | **7,986,741** |
| of them, the land's own surface | 7,704,411 (96.5%) |

So `N ≈ 8.0 × 10⁶`, and `N²` is **6.4 × 10¹³**.

- **Compressed Path Databases** (Botea; Strasser/Harabor/Botea — what wins the
  Grid-Based Path Planning Competition). For every source, the *first move*
  toward every target, run-length compressed. A query is "read a byte, step,
  repeat" — no open list, no heuristic. **It is the fastest thing in the
  literature and it does not fit here**, and the arithmetic is the whole
  argument rather than a caveat:

  - *Storage.* RLE is what makes it tractable at all: for a fixed source the
    first move is spatially coherent — whole quarters of the map leave by the
    same door — so a row of 8M entries compresses to a few hundred runs. But it
    compresses to **O(N × runs)**, not to O(N). At 8.0M sources and a
    generous 300 runs of 4 bytes, that is **~10 GB**. The published databases
    are tens to hundreds of MB on GPPC maps that hold 10⁴–10⁵ walkable cells;
    we are two to three orders of magnitude past them.
  - *Build.* One single-source search per source: **N searches over an N-node
    graph**. At this tree's measured ~200 ns a node expansion, 6.4 × 10¹³
    expansions is on the order of **10⁵ CPU-hours**. There is no compression
    for that — RLE shrinks the answer, not the work of finding it.

  **Where it is still alive is per cluster**, which is the standard reading: a
  32×32 region holds ~270 standing places, so all-pairs inside one is ~73,000
  entries and trivial. That is what `local_costs` already computes, on the fly,
  per query.
- **Hierarchical decomposition** (HPA\*, and every RTS since). *We have this* —
  [`navigation_graph.md`](../design_navigation_graph.md), built in N4, read since N7 —
  **and the obstacle above is precisely why it exists.** Hierarchy is not a
  cheaper table; it is the trick for never building the big one. Pay all-pairs
  only among a small distinguished set — portals here, contracted nodes in a
  contraction hierarchy, access nodes in transit-node routing, landmarks in
  ALT — where |P| ≪ N, and reach the nearest member with a short local search.
  `|P|² + N` is a table; `N²` is not.
- **Contraction hierarchies / hub labelling** (OSRM, road networks). The same
  escape, tuned to a graph with a highway structure a uniform grid does not
  have. Worth reading for the escape rather than for the method.
- **A connectivity oracle.** The cheapest member of the family and the only one
  that is genuinely O(1) storage per place: "is there any way at all", answered
  from precomputed components. We have the components; **the AI does not ask
  them first** — see the backlog below.

**What this leaves is the linear precomputation**, which is why
[differential heuristics](#3-search-the-same-graph-expand-fewer-nodes) below is
ranked where it is: `K × N` and not `N²`. Even that wants a granularity
decision — 8.0M places × 8 landmarks × 2 bytes is 128 MB beside a facet that is
already ~150 MB resident — so the landmark distances probably want to hang on
regions rather than on places, and a region-level bound has to be shown to be a
true lower bound before it may be used as one.

### 2. Do not search a grid — change what a node is

- **Navmesh** (Recast/Detour; Unreal, Unity, most of AAA). A polygon soup
  instead of tiles: a search expands tens of nodes where a grid expands
  thousands. **This is the honest answer to "how do modern games make it fast",
  and it is the least applicable to us**: UO's world *is* a grid, and our step
  rule is per-tile with heights, climb limits and one-way ledges. The idea —
  fewer, bigger nodes — is what the coarse graph already borrows.
- **Jump Point Search / JPS+** (Harabor & Grastien 2011). For uniform-cost
  octile grids: prune symmetric paths and jump along straight runs without
  putting the intermediate tiles on the open list. 10–30× fewer expansions, and
  JPS+ precomputes the jump distances for several more. **It does not apply
  here, and the reason is worth writing down**: JPS assumes passability is a
  property of a *tile*. Ours is a property of a *step* — `steps_out_of` reads
  the height being stepped off, the climb limit, the corner rule, the doors and
  the live bodies — so a "jump" would have to re-derive z along the run, and the
  pruning proof (that if a path exists, a canonical one exists) fails wherever
  two equal-length routes differ in whether the climb is legal. **Canonical
  orderings** (Sturtevant & Rabin, GDC 2016) are JPS's pruning without the
  jumping, and they fail for exactly the same reason.

### 3. Search the same graph, expand fewer nodes

- **Weighted A\*.** ✅ Done this session. The cheapest change in the whole list
  and worth a quarter of the map.
- **Differential heuristics / ALT** (landmarks and the triangle inequality).
  Bake the distance from K landmarks to every place; take
  `h = max over L of |d(L, goal) − d(L, here)|`. Far more informed than
  Chebyshev *precisely where Chebyshev is worst* — around a wall the frontier is
  currently flooding — and it stays **admissible**, so routes remain shortest.
  **The strongest idea in this document that is both applicable and free of a
  behaviour change**, and we already bake a graph to hang it on. It is also the
  only precomputation here that is **linear** — `K × N` where §1's table is
  `N²` — which is the whole reason it survives a facet and CPD does not. The
  granularity question §1 ends on is its first decision.
- **Bidirectional search.** ~2× on average. **Blocked by a real property of this
  world**: our step rule is not symmetric — a body drops off a ledge it cannot
  climb — so the backward search needs its own reverse rule, which is a second
  copy of the thing `navigation_spans.md` spent two sessions making single.
- **Beam search, A\*ε, optimistic search.** Bounded-suboptimal families around
  the same trade weighted A\* makes; nothing here beats 5/4's ratio of gain to
  risk without giving up the bound.

### 4. Search less often

- **Plan once, walk it, re-plan on failure.** ServUO's `PathFollower`, and
  practically every MMO. **We do this for the coarse half and not for the exact
  one** — the backlog entry below.
- **D\* Lite / LPA\*.** Repair the previous search when the world changed a
  little, instead of redoing it. Robotics' answer to dynamic obstacles; a real
  fit for "a crate was dropped in the doorway", a poor one for "the quarry
  moved", which is most of what a chase is.
- **Flow fields** (Supreme Commander 2, Planetary Annihilation). One Dijkstra
  *from the goal* gives a direction per tile that every unit reads for free.
  **Directly applicable to the shape a UO shard actually has**: many creatures
  chasing one player is many searches to one goal, and it is one flood.
- **Time-slicing across agents** — N plans a tick, the rest queue. Standard RTS
  hygiene, and the thing that makes a budget a *scheduling* decision rather than
  a per-search one.

### 5. Make the node itself cheaper

Bitset grids (one bit a tile, 64 tiles a word — the trick that makes JPS's
scanning fast), packed node records, arrays instead of hash maps, bucket or
radix queues for small integer costs, SIMD over the eight neighbours.

**A\*'s own half is measured out** by this session's profile: the heap and the
hash are 6% of a run between them, and our passability cannot be a bitset
because it is a property of a step rather than of a tile.

**The terrain half is not quite as closed as it read.**
[`navigation_spans.md`](2026-08-25-the-span-layer.md)'s four refusals are sound and stand,
but every one of them asked *should we store more* — and this session found 2.3%
of a node expansion in what the land grid already computes, by deriving one block
address instead of four. The distinction is worth carrying forward: **"is there a
cheaper table" is answered, "is this arithmetic doing work twice" was never
asked.** What is left of it is in the table below.

## What is next

| | what would close it |
|---|---|
| 🚩 **The exact search runs every beat and its route is thrown away** | `ai::plan_step` calls `find_path` each beat and uses `path.first()`. `REPATH_TICKS` (40) governs the *coarse* half only. A validated cached route — walk it, re-plan on a refused step, on the goal moving past a threshold, or on the timer — is the references' own pattern and would cut exact searches by roughly the length of a route |
| 🚩 **A far chase pays a full 400-node local search before the graph is asked** | `plan_step` asks `find_path` first and falls through to `find_long_path` only on refusal. For a destination past `COARSE_MIN_DISTANCE` that the local search will refuse, that is the whole budget spent to learn what the region components already know |
| **Differential heuristics** | §3 above. Bake K landmark distances beside the navigation graph; it is admissible, so the oracle is the existing dump — the routes must not move, only the node counts |
| ~~**Compressed path databases**~~ | **Refused on arithmetic, §1 above.** `N` is 7,986,741 measured, so the table is `N²` = 6.4 × 10¹³: ~10 GB after run-length compression and ~10⁵ CPU-hours to build, against published databases of tens of MB on maps two to three orders smaller. Alive only per cluster, which `local_costs` already is |
| **Flow fields for the many-chase-one case** | §4 above. Wants a count first: how many creatures share a goal in a live shard |
| **The node budgets, 400 and 600** | Now partly argued — 5/4 at 400 beats exact at 600 — but the argument was made about *arrivals*, not about what a tick can afford. That second half still wants the shard's own numbers |
| **One corner block for a whole expansion** | The corners of a node's nine tiles are **16 distinct cells** and about forty are read. `corner_quad` took the redundancy *inside* one tile; this would take it across the eight. Derived at 10–14% of an expansion and **not measured**, and the ceiling is known: *all eight on one column* against *landings off the bake* is 127.0 against 145.4, so everything locality can ever give is ~13% of the landing half. It also threads an array through `landing` → `Spans::check` → `ground`, which is the seam N3 and the terrain work spent two sessions narrowing — so it wants a measurement before it wants a patch |

## What is clean

`cargo test -p openshard-map -p openshard-movement -p openshard-ai -p
openshard-client-app`: 82 + 154 + 7 + 5 + 393 + 1 passed, 0 failed. `cargo
clippy` on all of them silent — including the `too_many_arguments` an eighth
parameter earned, which is why `Rigour` bundles the budget and the weight where
they travel together. `rustfmt` on every touched file.

**The oracle across all four commits is the probe's own dump**, byte for byte:
37,248 destinations at both budgets, `arrived=4010` and `arrived=4405`
throughout. Three of the four commits had to leave it untouched and did; the
fourth is the one that moves routes on purpose, and what it moves is measured in
the table above rather than asserted.

**Not ours and still there:** `crates/server/boats` and `crates/server/state` do
not compile in the working tree — a parallel session's `boat::Plank` is
mid-change — and `client/render`'s `frame` test wants a `DirtyRows::start` that
is mid-change too. So `cargo check --workspace` cannot be run to silence today;
every crate this session touched was checked on its own, `client/render`'s
library included.

One thing to own: the second commit swept up a **pure rename** in
`crates/server/boats/tests/` that a parallel session had left staged in the
index. No content moved with it. `git commit -- <paths>` rather than a bare
`git commit` is the guard, and it was not used; the two commits after it use it.
