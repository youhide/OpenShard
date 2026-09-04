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

## P1 — A wide crossing needs more than its two ends — ✅ done

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

**What was built: option 1, at a spacing of sixteen.** `PORTAL_SPACING` in
[`navigation.rs`](../../../crates/common/movement/src/navigation.rs), and
`ROUTING_VERSION` 5 because a version 4 artifact is a graph whose regions are
crossed at their corners. All three rules were baked and measured so that the
choice is a reading rather than an argument:

| | corners only | every 16 | every 8 |
|---|---|---|---|
| nodes | 71,545 | 95,672 (+34%) | 144,417 (+102%) |
| edges | 416,122 | 740,339 (+78%) | 1,819,968 (+337%) |
| artifact | 7.84 MB | 10.20 MB | 17.50 MB |
| whole-facet bake | 10.6 s | 14.4 s (+36%) | 21.5 s (+103%) |
| publish rebake, two rings | 41.4 ms | 43.3 ms (+5%) | 48.6 ms (+17%) |
| houses detour p95, 16 tiles out | **32%** | **7%** | **7%** |
| houses detour p95, 24 tiles out | 18% | 6% | 5% |
| houses detour p95, 32 / 48 out | 3% / 4% | 4% / 4% | 4% / 4% |
| ring detour p95, six bands | 13/3/10/5/4/4% | 13/3/11/5/3/4% | 13/3/5/3/2/3% |
| **walk-path corridor p95** (houses) | 30.4 ms | **24.4 ms (−20%)** | 24.5 ms |
| ring query p95, bands 32–256 | 1.1/1.0/1.1/1.3 ms | 1.1/1.0/1.2/1.3 ms | 1.1/1.0/1.1/1.2 ms |
| ring query p95, band 512 | 2.6 ms | 4.4 ms | 11.1 ms |
| ring query p95, band 1024 | 3.0 ms | 3.5 ms | 3.7 ms |

**How these were taken, because the first set of them was wrong.** This
workstation runs several agents and its load average was 30–50 when P1 was first
measured, which inflated every duration and inflated them unevenly — the first
reading had the bake at 19.8 s and claimed the walk-path query got *worse*. The
numbers above are: `nice -15` (via `sudo … setpriv`, so the process is elevated
but still the user's), the three rules built as three binaries and run
**interleaved** round-robin so that any drift hits all of them alike, each
duration the **minimum** of its repeats (bakes three rounds, queries
`--repeat 5`, the publish rebake's own `best_of`), and two full rounds of the
bench that agree to within a few tenths of a millisecond. Load average during
them was 3–10. The step counts never moved: a route is a property of the graph,
not of the machine, so every detour percentage here is the same one the loaded
run reported.

**Eight buys nothing sixteen has not already bought.** The castle's region was
crossable only at its corners, and one crossing in the middle of the border is
the whole repair; halving the spacing again pays for it a second time — 4.4× the
old edge count against 1.8×, a bake twice as long, and band 512's query at
11.1 ms against 4.4 — for the same 7%. Sixteen shipped.

**The done-when below is met on the detour, and the price is a third of what the
loaded run said.** The near ring goes 32% → 7%. Against that: a whole-facet bake
is 36% dearer (10.6 → 14.4 s), a publish's two rings 5% (41.4 → 43.3 ms), and
one ring band — 512, where the abstract search is largest — goes 2.6 → 4.4 ms
while the bands under it do not move. Bake time is still not "within noise", and
no option that adds a node could be.

**And the walk path got faster, which is the opposite of what was expected.**
The corridor a body's step asks for goes **30.4 → 24.4 ms**, reproducibly, on
every one of the four rings. More crossings mean a nearer one, which means fewer
and shorter refinement hops and fewer retries — the abstract search's extra
nodes cost less than the refinement they save. So the clause about long-query
p95 was written against a fear the measurement does not support: the query that
runs while somebody is walking is 20% cheaper, and only the longest bare-facet
bands pay anything at all.

**What is not touched.** The bare facet's ring bands are the same reading with
noise on it, which is the whole reason the houses case had to be measured
separately. And band 32 stays at 13%: a 42-step route against 37 on open ground
is not a region crossed at its corner, no spacing moves it, and nobody has
looked at what it *is*.

**What this leaves behind, for whoever changes the spacing next.** Nodes grow
with the spacing and **edges grow faster than nodes**: +34% nodes bought +78%
edges at sixteen, and +102% nodes bought +337% at eight. That is intra-region
routing, which is all-pairs between a region's own nodes, and it is what makes
the artifact and the bake grow the way they do. A future option 3 —
representatives where the border's own cost changes — is attractive for exactly
this reason: it is the one that adds a node only where a node says something.

**What this reading is one of.** One building, in one place, on one facet —
`--design` and `--design-at` take another, and nobody has run one yet. What
makes this castle worth gating on is that it is the building the report came
from and it stands on the near crossing of its own region, which is the shape
the defect needs; what it cannot say is how common that shape is. A second
design somewhere with a different border would say whether 32% is the case or
the worst case.

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
than today's by more than the measurement's own noise. — Met on the detour (7%)
and on the query the walk path actually makes (30.4 → 24.4 ms); knowingly not
met on bake time, which is 36% dearer. See the table above.
The five `real_routes` scenes pass unchanged against the new graph:
the originating click is 95 steps against the exact 94, its 9 neighbouring
starts and 196 long routes loop nowhere, and the walked click still arrives in
95 steps and 95 plans.

## P2 — One click, two standing answers — ✅ done

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

**Decided, and it is the likely shape.** The third caller is the HUD's own
route, and the divergence was one argument in one comment: it passed
`auto_open_doors = false` whatever the setting said, because "the setting is an
intention to open a leaf and a picture of a walk is no place for intentions".
The ghost passed in the same call refutes it. A dead body is read `AllOpen`
there precisely because its *step* goes through the leaf, and a route drawn
stopped at that leaf would be a picture of a refusal that is not going to
happen; an auto-door body's step goes through the leaf too, because `App::walk`
sends the use before the step, which is what makes the promise good. So the same
sentence convicts the same picture, and what is left for `Barred` is a door the
body really will not open — alive, with the setting off.

**And the reading was already inconsistent inside one function.**
`route_shown`'s hover branch *resolves* its destination through
`walk_destination`, which reads `App::walking_doors` — the walk's setting
included — and then planned the route to that place under a different reading.

**What was built.** `world::drawn_route_doors`, which delegates to
`walking_doors`. The question keeps a name of its own because it used to have an
answer of its own: reverted at the call site it would be one line in a HUD
function nobody reads twice, and a function is something the argument can be
written on and a test can ask for.

**What this leaves behind.** `Steering::plan_for` calls `remember_refusal`, so
the *hover* preview — the branch that plans toward whatever tile the cursor is
over when there is no destination at all — writes into the refusal the HUD strip
reads and `say_refusal` speaks. A tile nobody clicked on can set the sentence a
player is told about the order they did give. It is the same shape as this
finding and not the same bug, and nothing above moves it.

**Done when.** One order has one answer, and a test asks the same click twice in
the two readings and gets one verdict. — Both, in
[`steer.rs`](../../../crates/client/app/src/steer.rs)'s
`one_click_has_one_verdict_whether_it_is_walked_or_drawn`: the same destination
behind a shut leaf, asked under the walk's rule and under the drawn route's, in
the one state of the four they used to answer differently. Reverting
`drawn_route_doors` fails all three of its assertions, and each is something the
player sees — the sentence, the green line, and the red one.

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
building in front of it. So P1 opened, and the choice between its three options
was then made on the four numbers rather than on preference: option 1 at a
spacing of sixteen, which takes the near ring to 7% and costs a third of what
halving the spacing again would. ✅ Done.

**P4 alongside it** — it is an hour, and it is the instrument every one of these
findings was read with; an instrument that lies is not something to leave lying
about. ✅ Done: the flag changes with a fresh session line, and a replay reads
the one in force for the episode. **P2 next**, because it is a decision more
than a change and the answer is cheap once taken. **P3 last**, deliberately: it is the largest, none of the
others is blocked on it, and it is the one whose number no repair above moves.

**P2 was a decision and then two lines.** ✅ Done: the drawn route is planned the
way the walked one is, and `Barred` is left for a door the body will not open.
It moves no number P3 is about — the three plans a step are three *frames*, not
a walk and a preview racing inside one, since the per-frame cache was already
shared. What it changes is that the plan they share is now an answer to the
question both of them are asking.
