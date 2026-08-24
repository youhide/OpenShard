# 2026-08-24 — the search stops asking the clock

The backlog's 🚩 entry, taken: `MAX_SEARCH_TIME` was 50 ms of wall clock read
once per node expansion, inside a tick `architecture.md` calls deterministic. It
is gone, and so is `MAX_LONG_PATH_TIME` — a long query is bounded by a counted
wallet now. The same profile that named the clock named two more things nobody
had looked at, because **nothing had ever profiled the search itself**: every
number this plan quotes about A\* was an arithmetic remainder.

**A search is 26–28% faster and answers exactly what it answered before.**

## Where it stands

### The profile, which is the whole of why this session did what it did ✅

`perf record` over `map_path_probe`, 37,248 destinations from Britain's castle,
release plus line tables (the new `[profile.profiling]` in the workspace
manifest — `release` with `debug = 1`, so a profiler can name a frame in the
code that actually ships).

Three of the eight hottest symbols were not the search's work at all:

| | | |
|---|---|---|
| `__vdso_clock_gettime` + `Timespec::now` | **6.5%** | `MAX_SEARCH_TIME`, once per pop. The only syscall in the loop |
| three `reserve_rehash` | **5.8%** | `cost`, `came_from` and `closed` growing from empty on every search, with the budget known on the way in |
| `BinaryHeap::push` | **3.8%** | a six-field `OpenEntry` whose derived `Ord` is a chain of up to six compares, run ~log₂(600) times a push |

The terrain half — `land_corners`, `SpanIndex::stored`, `Spans::check`,
`landing`, `Spans::ground` — is the other 60%, and **this session did not touch
it**: [`navigation_spans.md`](../navigation_spans.md) closed it with four
measured refusals and asked that it not be reopened without a new reason. There
was no new reason. There was a different half.

### The clock is gone from both searches ✅

**An exact search is bounded by its node budget and nothing else.** 400 or 600
nodes is 0.1–0.25 ms, so a 50 ms deadline was never reachable — it could only
cost the read.

**A long query is bounded by [`Effort`], one counted wallet** that its two region
floods and every refinement pass draw from, because that is the query that is
genuinely many searches. `LONG_PATH_EFFORT` is 100,000 node expansions, and the
number is measured rather than converted: over `coarse_bench`'s six bands from
two origins on facet 0, 87 long queries spend a median of ~1,900 and a worst of
4,377. Converting the old ceiling would have given ~200,000; at ~250 ns a node
the one that shipped is ~25 ms, half of what the clock allowed.

`SearchExit::Deadline` and `LongExit::Deadline` are gone; `LongExit::Spent`
replaces the second. **The difference is the point**: a deadline was a fact about
the machine, and the same query over the same ground now spends the same wallet.

### Three tables became one record ✅

What A\* knows about a place — its cost, the step it was reached by, whether it
is finalised — was three containers keyed by the same `PathNodeKey`. A neighbour
cost four hash lookups of one key (`closed.contains`, `cost.get`, `cost.insert`,
`came_from.insert`) and a pop cost two; a node has eight neighbours.

One `Visit` record, one `entry` per neighbour, one `get_mut` per pop — and the
table is **reserved from the node budget** rather than grown from empty. The
reservation is 2× the budget, and that is a reading rather than a guess:
`map_path_probe` now prints what the table held, and over 149,000 searches from
two origins nothing exceeds **×1.84**.

### And the open list's ordering is one integer ✅

`OpenEntry` was six fields ranked `f`, `h`, Manhattan, then the coordinates for
determinism — which is exactly what comparing one number with those fields laid
out most-significant-first does. It is a `u128` now: `f`, `h` and the Manhattan
distance at 24 bits each, then `x`, `y`, and the height with its sign bit
flipped so an unsigned compare ranks it the way `i8` does.

**The tie-break is preserved by construction**, and the probe proves it rather
than asserting it: the places written down are *identical* to before, so the
search settles on the same route among equally short ones.

## What was decided

**The wallet is read where a search starts, not inside its loop.** `Effort` is
consulted once — `allowance(budget)` is the smaller of the query's remainder and
this search's own budget — and charged once, with what the search finalised. A
limit that cannot change while a search runs is a limit the hot path never looks
at, which is the whole difference between counting and clocking.

**A region flood is not interrupted, it is billed.** `local_costs` is bounded
work by construction — one expansion per place of a 32×32 rectangle — and the
document already said so. So it runs to completion and pays for what it
expanded; the wallet is read after it, where the deadline used to be.

**The bake pays nothing.** `region_costs` is also called from
`NavigationGraph::build`, which has a whole facet's worth of time; what a flood
costs is a query's question.

**The three cost fields clamp rather than wrap.** A value past 24 bits would
otherwise carry into the field above and rank a candidate as the *cheapest* thing
on the list — a search that visibly wanders, not one that crashes. Nothing on a
UO facet comes near: the widest heuristic a 65,536-tile map can produce is 16
bits.

**Diagnostics no longer time what they will not print.** `find_path` took
`Instant::now()` twice per query whether or not `OPENSHARD_PATH_DEBUG` was set.
The clock is behind the same gate as the printing now.

## What is clean

`cargo test --workspace`: **3,546 passed, 0 failed**, 36 ignored — one new test
over the packing, and it earned its place immediately: the first version of
`OpenEntry::place` masked `h` at 32 bits where the field is 24, and the existing
`an_unreachable_goal_is_walked_toward_until_the_ground_runs_out` caught it. The
new test is the direct one — every pair of a sample compared against the tuple
the struct used to be, negative heights included.

`cargo clippy --workspace --all-targets` silent. `rustfmt` on every touched file.

**The oracle is the probe's own answers**, unchanged across all three changes:
4,010 and 4,405 arrivals at the two budgets, 26 and 31 columns reached at another
height, and an identical count of places written down.

| | before | after |
|---|---|---|
| budget 400, p50 | 0.109 ms | **0.080 ms** |
| budget 600, p50 | 0.168 ms | **0.122 ms** |
| per node, budget 600 | ~262 ns | **~189 ns** |

## What is next

| | what would close it |
|---|---|
| **The profile is 76% terrain and 22% A\*** | Which is where [`navigation_spans.md`](../navigation_spans.md) takes over: four measured refusals on the terrain half, and the lever it names is **fewer nodes** rather than a cheaper one. Do not reopen it without a new reason — this session had one for the *other* half and that half is now spent too |
| **The node budgets, 400 and 600, are still unargued** | [`terrain_seam.md`](../terrain_seam.md)'s entry, unchanged: the oracle's data is what they can finally be asked against. A node is 28% cheaper than when they were last discussed, which moves what a budget buys but not what it should be |
| **`LONG_PATH_EFFORT` has one sample behind it** | 87 queries from two origins on one facet. A third origin over denser ground — a city, not a castle — is what would tell whether 23× the worst reading is generous or merely lucky |
| **Two bodies on a deck that moves under them** — still unexamined at both ends | — |

And one this session found and fixed rather than filed:

- **Profiling this tree took three attempts before it took a sample.** `release`
  has no line tables, `perf_event_paranoid` defaults to 2, and
  `perf_event_mlock_kb` defaults to 516 — on which samply fails with a bare
  `mmap failed` and names nothing. The build profile is in the workspace
  manifest and the two sysctls are in
  [`development.md`](../../development.md#profiling-build-profiling-and-set-two-sysctls-first),
  because the first thing the next profiling session does is hit all three.
