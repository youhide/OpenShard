# 2026-08-24 — the search gets in a hurry

Two commits and a survey. The previous handoff closed the *cheap node* question
and said the lever left is **fewer nodes**; this session took the cheapest way
there, took what was still loose inside `explore`, and — because the question
that opened it was "what does the industry actually do" — priced the rest of the
field against this codebase rather than in the abstract.

**A body's route now reaches a quarter further inside the same node budget.**

## The measurement this starts from

`perf record` over `map_path_probe`, 16,640 destinations from Britain's castle,
the `profiling` profile. Nothing here is inherited: the numbers are today's.

| | |
|---|---|
| `explore` — A\*'s own heap, hash, packing and loop | **18.6%** |
| `land_corners`, `landing`, `Spans::{ground, check}`, `SpanIndex::stored`, `steps_out_of`, `climbed`, `statics_at`, `blocker_at`, `start_surface` | **~76%** |

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
[`terrain_seam.md`](../terrain_seam.md) has had filed as unargued since it was
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

## What the industry does, and what each is worth here

The question this session was opened with. Ranked by what it would buy *this*
codebase, with the reason it is or is not applicable.

### 1. Do not search — precompute the answer

- **Compressed Path Databases** (Botea/Harabor; what wins the Grid-Based Path
  Planning Competition). For every node, the *first move* toward every other
  node, run-length compressed. A query is "read a byte, step, repeat" — no open
  list, no heuristic, microseconds facet-wide. **The endgame, and the one thing
  here that is genuinely orders of magnitude.** The cost is memory and a bake:
  our facet has ~5.4M standing places, and the compression ratio on a game map
  is the whole question. **Nobody has measured it here.**
- **Hierarchical decomposition** (HPA\*, and every RTS since). *We have this* —
  [`navigation_graph.md`](navigation_graph.md), built in N4, read since N7.
- **A connectivity oracle.** "Is there any way at all" answered from precomputed
  components in O(1). We have the components; **the AI does not ask them first**
  — see the backlog below.
- **Contraction hierarchies / hub labelling** (OSRM, road networks). Microsecond
  queries on continental graphs, but they exploit road-network structure —
  highway hierarchy — that a uniform grid does not have. CPD is the grid's
  answer to the same ambition.

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
  behaviour change**, and we already bake a graph to hang it on.
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
**Measured out here, twice**: the terrain half by
[`navigation_spans.md`](navigation_spans.md)'s four refusals, and A\*'s own half
by this session's profile — the heap and the hash are 6% of a run between them,
and our passability cannot be a bitset because it is a property of a step rather
than of a tile.

## What is next

| | what would close it |
|---|---|
| 🚩 **The exact search runs every beat and its route is thrown away** | `ai::plan_step` calls `find_path` each beat and uses `path.first()`. `REPATH_TICKS` (40) governs the *coarse* half only. A validated cached route — walk it, re-plan on a refused step, on the goal moving past a threshold, or on the timer — is the references' own pattern and would cut exact searches by roughly the length of a route |
| 🚩 **A far chase pays a full 400-node local search before the graph is asked** | `plan_step` asks `find_path` first and falls through to `find_long_path` only on refusal. For a destination past `COARSE_MIN_DISTANCE` that the local search will refuse, that is the whole budget spent to learn what the region components already know |
| **Differential heuristics** | §3 above. Bake K landmark distances beside the navigation graph; it is admissible, so the oracle is the existing dump — the routes must not move, only the node counts |
| **Compressed path databases** | §1 above. The one order-of-magnitude idea in the list. Wants a measurement of the compression ratio on facet 0 before it wants a plan |
| **Flow fields for the many-chase-one case** | §4 above. Wants a count first: how many creatures share a goal in a live shard |
| **The node budgets, 400 and 600** | Now partly argued — 5/4 at 400 beats exact at 600 — but the argument was made about *arrivals*, not about what a tick can afford. That second half still wants the shard's own numbers |

## What is clean

`cargo test -p openshard-movement -p openshard-ai -p openshard-client-app`: 154 +
7 + 5 + 393 + 1 passed, 0 failed. `cargo clippy` on all three silent — including
the `too_many_arguments` an eighth parameter earned, which is why `Rigour`
bundles the budget and the weight where they travel together. `rustfmt` on every
touched file.

**Not ours and still there:** `crates/server/boats` and `crates/server/state` do
not compile in the working tree — a parallel session's `boat::Plank` is
mid-change — so `cargo check --workspace` cannot be run to silence today.

One thing to own: the second commit swept up a **pure rename** in
`crates/server/boats/tests/` that a parallel session had left staged in the
index. No content moved with it. `git commit -- <paths>` rather than a bare
`git commit` is the guard, and it was not used.
