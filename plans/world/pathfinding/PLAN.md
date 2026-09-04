# Where a corridor sends a body, and what it costs to ask

> **Scope: P1 to P4, in that order.** Everything here came out of the two
> sessions the route journal recorded on 2026-09-04, and every one of them is a
> defect a player meets rather than a number a bench dislikes.
>
> Status does not live here — it is
> [`docs/world/README.md`](../../../docs/world/README.md), findings 22 and 24
> to 28. The graph itself is
> [`design_navigation_graph.md`](../../../docs/world/design_navigation_graph.md),
> the journal that produced the evidence is
> [`path_journal/done/PLAN.md`](../path_journal/done/PLAN.md), and the reasoning
> the hierarchy was chosen by is
> [`research/coarse_pathfinding.md`](../../../docs/world/research/coarse_pathfinding.md).

## Where this starts

What is built and works: a bounded A\* over places to stand
([`path.rs`](../../../crates/common/movement/src/path.rs)), a coarse graph of
32×32 regions joined by directed portals
([`navigation.rs`](../../../crates/common/movement/src/navigation.rs)), an
endpoint join that floods from a body standing on a runtime floor out to that
static graph (`live_join` — the whole reason a click on a player house's roof
can be answered at all), and two cuts over the route refinement splices
together: `without_loops` for a place stood on twice, and `without_folds` for a
route that comes back within one tile of one.

What the journal then showed is that the *shape of the graph* decides where a
body walks, and the shape is coarser than anybody had looked at:

| what a click met | measured |
|---|---|
| a roof ten tiles away, planned by the corridor | 123 steps against the exact 94 — a walk **nineteen tiles into open field**, away from the house |
| the same after `without_folds` | 95 steps against 94 |
| the region the body stood in | **five nodes, every one of them a corner** |
| one plan, `release` | ~30 ms for the corridor, ~17 ms for the exact search it stood in for |
| one *step* of walking | three plans, 110–124 ms each in the session's own build |

## The rule this is all downstream of

`add_portal` gives a run of `WIDE_PORTAL` (6) crossings or more exactly two
representatives — `run[0]` and `run[len - 1]`. A 32-tile border of open ground
is one such run. So **a region is crossed at its corners and nowhere else**, and
a body in the middle of one pays up to sixteen tiles to reach a crossing. It is
invisible while the bounded search answers, because a short trip never asks the
graph; it becomes a player's report the moment something makes the bounded
search fail for a reason that is not distance — a roof, a cellar, a house with
one door.

---

## P1 — A wide crossing needs more than its two ends

**What is wrong.** The detour above is not a splice artefact and not a live-layer
accident: the corridor was a *single node*, both of its legs were optimal, and
the node was the region's far corner because the near one was under a castle.
`without_folds` cuts the symptom to one step over the exact answer; the cause is
untouched, and the next building placed over a corner produces the next report.

**What to do.** Give a wide run intermediate representatives, so the worst detour
is bounded by the spacing chosen rather than by half a region.

**The three options, and what separates them.**

1. **Every `k`-th crossing of the run.** Simplest, and the detour is bounded by
   `k / 2` outright. Costs nodes on exactly the ground that has the longest runs
   — open country — where they buy the least.
2. **A fixed count per run** (ends plus `n` inside). Bounds the node growth
   instead of the detour, which is the wrong way round for the defect: a facet's
   longest runs are its most open ground and would keep the worst crossings.
3. **Representatives where the border's own cost changes** — the run's ends plus
   wherever the local cost to cross jumps. Fewest nodes for the most information,
   and the only one that needs a measurement before it can even be written.

**The first of those measurements is in, and it argues against doing this
now.** `coarse_bench` grew the reading the defect actually is — the corridor's
route against the shortest one the ground holds, `--exact` — and over the six
distance bands from `(1363, 1600)` on the bare facet the corner rule costs
almost nothing:

| band | detour p50 | p95 | worst |
|---|---|---|---|
| 32 | 1% | 13% | 13% — 42 steps against 37 |
| 64 | 0% | 3% | 3% |
| 128 | 2% | 10% | 10% |
| 256 | 2% | 5% | 5% |
| 512 | 3% | 4% | 4% |
| 1024 | 3% | 4% | 4% |

A corner is up to sixteen tiles out of the way and a long route amortises it;
what does not amortise it is a *short* route, which is the top row and still
only 13%. And the click this track was opened by is now **1%** — 95 steps
against 94 — because `without_folds` takes the detour back out.

So the premise this section was written on looked weaker than it was: on the
bare facet the corner rule is not costing a route. **The gate was one more
reading — over ground with houses on it — and it is in.** `coarse_bench
--houses` lays the same castle finding 25 is about, live, over the bare facet
the graph was baked over, and clicks it from a body standing at four distances.
Every such click fails the bounded search for a reason that is not distance, so
the corridor answers it however near it is; the detour is read after
`without_folds`, exactly as above:

| a body standing | pairs | detour p50 | p95 | worst |
|---|---|---|---|---|
| 16 tiles out | 48 | 8% | **32%** | 35% — 99 steps against 73 |
| 24 tiles out | 48 | 10% | 18% | 20% — 111 against 92 |
| 32 tiles out | 48 | 0% | 3% | 3% |
| 48 tiles out | 48 | 0% | 4% | 4% |

```sh
cargo run --release -p openshard-movement --example coarse_bench -- \
  --client "$OPENSHARD_CLIENT" --houses --rings false
```

**The detour is worst exactly where the player is standing.** The far rings are
cheap for the same reason the bare facet is — a long route amortises a corner —
and the near ring, which is where a body is when it clicks on the building in
front of it, pays a third. All 192 pairs reached the graph (none answered by the
bounded search, none refused by the corridor, none near enough to be refused
without asking), so the reading is about the corridor and nothing else.

**P1 is therefore open**: the gate was a p95 inside a quarter and the ring a
player clicks from is 32%. What is still not settled is *which* of the three
options below, and that is the four numbers named next — not a preference.

**What must be measured before picking.** All three options move the same four
numbers, and nothing here should be chosen by preference:

- node and edge count on facet 0 (today: 71,545 nodes, 416,122 edges over 28,672
  regions) — the artifact's size and `abstract_path`'s open list both scale with
  it;
- bake time, whole-facet and per-chunk (a publish rebakes a ring, and 80 ms is
  the number that made a publish affordable);
- the p95 of `find_long_path` over the facet-0 route set, because more nodes is
  more abstract search and this repair is worthless if it costs what it saves;
- the **detour distribution** itself: over a spread of starts and destinations,
  corridor steps against exact steps. That is the number the defect is, and no
  measurement of the others is a substitute for it. It is now a reading anybody
  can repeat — `coarse_bench --houses` for the ground it is worst on, and the
  ring bands for the ground it must not get worse on.

**Done when.** The detour distribution's p95 is inside a quarter of the exact
answer — the ratio
`a_route_onto_a_castle_roof_does_not_walk_away_from_the_castle` already asserts
for one click — **on the houses reading's near ring**, which is the 32% above
and the only band that fails today, with bake time and long-query p95 no worse
than today's by more than the measurement's own noise.

## P2 — One click, two standing answers

**What is wrong.** Finding 26. The plans of one order run in a rhythm of two and
one: two read the ground with doors as a walking body opens them and route onto
the roof; the third reads it with doors as they stand, fails the roof's live
join, and calls the click `Barred` with a route that stops at the door. Both are
written to the journal, both are drawn green, and the player is shown whichever
was last. This is not the live layer flickering — the destination resolves to
the same height on all three.

**What to decide.** Which reading a *destination* is resolved and refused under.
A body that will open a door on the way has already decided it is not barred;
the preview that says otherwise is answering a question nobody asked. The likely
shape is that the preview borrows the walk's own reading and the `Barred`
refusal is reserved for a door the body will not open — but that is a decision
about what a player is told, and it belongs with whoever owns the cursor.

**Done when.** One order has one answer, and a test asks the same click twice in
the two readings and gets one verdict.

## P3 — Planning off the thread that draws

**What is wrong.** Finding 28. Three plans a step at tens of milliseconds each,
on the frame thread, for as long as anybody is moving. `without_folds` does not
touch it: it shortens the route, not the search that proposed it.

**What makes it more than "spawn a thread".** The search reads two grounds. The
**guide** is the bare facet and never changes. The **live overlay** is rewritten
by the network side as the world arrives, and it is the half that decides
everything a report is ever about. So the question is not where the search runs
but what it is allowed to read, and the three answers are:

1. **A long-lived worker fed changes.** The overlay is mirrored inside the
   worker and the network side pushes edits to it. Cheapest per query and the
   most machinery: two copies of the live layer, and every kind of edit has to
   be expressible as a message.
2. **A worker given an owned snapshot per query.** The frame thread cuts what
   the query can reach — the region around it, at the moment it is asked — and
   hands it over. No shared state at all, and the cost is the cut: it wants
   measuring against the plan it replaces before anything is built on it.
3. **A lock over the live layer.** Least code and the one this repository's
   style refuses by default — a reader that blocks the network thread, or a
   writer that blocks a plan, and neither is visible in a signature.

Latency is not the obstacle and should not be argued as one: a walk holds its
last plan while the next is asked for (that is what the plan cache is), and an
answer that arrives a frame late is a plan from a tile the body has just left —
the case every replan already handles.

**Done when.** A decision is written down with the measurement behind it, and
the walk path's per-step planning is off the frame thread with the frame time to
show for it.

## P4 — The journal says `coarse: false` about a session that had one — ✅ done

Finding 27, and the smallest thing here. The session line is written when the
window opens; the graph a world arriving asks for is baked after that, so a
replay reads "facet 0 WITHOUT a coarse graph" over a session where every plan
used one — the exact fact the field exists to stop a replay guessing at. Write
it when the first line is written rather than when the journal is built.

**Writing it late is only half of it**, which is what the work found: a
session's first click routinely lands *before* the bake finishes, and then the
header on disk is true of the lines under it and wrong about everything after.
An edit cannot fix that — there is no one answer for the file. So the change is
a **fresh `session` line** at the moment the graph arrives, the same shape the
F1 switch already writes for a gap, and one the other way when a facet
replacement drops the graph. A journal still owing its header just tells the
truth in the line it owes, and creates no file for a bake nobody planned a route
through.

On the reading side `read::session_at` answers which session line is in force
for a given line, and `path_replay` asks it for the episode it is about to
replay instead of taking the file's first. Episode numbering is untouched: a
session line opens none and closes none.

**Done when.** A session that bakes at login says `coarse: true`, and
`path_replay` says so too. — Both, in
[`write.rs`](../../../crates/common/pathlog/src/write.rs)'s
`a_graph_that_arrives_after_the_header_writes_a_fresh_session_line` and
`a_graph_that_arrives_before_the_first_line_is_in_the_line_itself`, and
[`read.rs`](../../../crates/common/pathlog/src/read.rs)'s
`the_session_in_force_is_the_last_one_written_before_the_line`.

---

## Order, and why

**P1's own gate first, and it was a measurement rather than a change.** It was
written to be taken first — it is the cause, and everything about where a
corridor may cross is downstream of it — and then its first reading said the
bare facet does not have the problem: 0–3% at the median and 13% at the worst,
with the click that opened this track down to 1%. The one reading that could
still condemn the rule was ground with houses on it, and it did: **32% at the
p95 from sixteen tiles out**, which is where a body stands when it clicks on the
building in front of it. So P1 is open, and what it now needs is the choice
between its three options made on the four numbers above rather than on
preference.

**P4 alongside it** — it is an hour, and it is the instrument every one of these
findings was read with; an instrument that lies is not something to leave lying
about. ✅ Done: the flag changes with a fresh session line, and a replay reads
the one in force for the episode. **P2 next**, because it is a decision more
than a change and the answer is cheap once taken. **P3 last**, deliberately: it is the largest, none of the
others is blocked on it, and it is the one whose number no repair above moves.
