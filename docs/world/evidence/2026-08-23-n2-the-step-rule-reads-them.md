# 2026-08-23 — N2: the step rule reads them

Era P's second node, in one commit: `25f64a28` the step rule reads the spans.
[`navigation_spans.md`](../design_spans.md)'s N2 called itself *"where the
risk in this plan lives"* — the risk being that `MapTerrain::check`'s answer
depends on the column's raw statics in some way a per-span bake cannot carry.
It does not. **248,268,125 steps compared over facet 0, 0 disagreements.**

## Where it stands

[`Spans::check`](../../../crates/common/movement/src/spans.rs) is the rule over
the bake: it walks the target column's stored spans instead of its statics,
highest first, and the first candidate that passes is the answer. `check`
expresses the same choice as a running maximum over the map file's own order, so
a descending walk that stops at the first acceptance is the same number — and on
a tie between the land and a static at one height the two disagree about *which*
surface won and agree about the number, which is the whole of what either
returns.

**Both of N2's oracles were built and both were checked to bite:**

| | |
|---|---:|
| steps compared — every surface × both abilities × eight directions | **248,268,125** |
| …of which landed somewhere | 238,291,149 |
| **disagreements** | **0** |
| flood from (1363, 1600, 30), map rule | 3,747,934 tiles in 4.2 s |
| flood from (1363, 1600, 30), span rule | **3,747,934 tiles** in 2.9 s |
| **tiles reached by one flood and not the other** | **0** |

Both live in the new
[`span_check`](../../../crates/common/movement/examples/span_check.rs) example,
beside `span_index` and for its reason: 29.4 million columns walked twice is
twelve seconds in release and minutes in debug. The suite carries the per-step
half twice — over a box of Britain where an install exists (1.9 M steps, a third
of a second) and over a scene of everything where one does not. **Disabling the
`landCheck` clause fails both of them**, which was checked rather than assumed:
an oracle nobody has seen fail is a decoration.

**What one node expansion's landing half costs**, facet 0 around (1500, 1900),
fastest of five passes over 10,836 standable tiles — `step_cost`'s own rows, all
three with the same checksum:

| | ns per tile |
|---|---:|
| 8 × `step_allowed` — what a search does today | 1070.2 |
| the same, landings computed once, over the map | 366.1 |
| **the same, landings off the bake** | **169.1** |
| pure A\* with the terrain taken away | ~220 |

The landing half is now **under what A\*'s own machinery costs**, which is the
number N3 has to plan against.

**`cargo check --workspace --all-targets`, `cargo clippy --workspace
--all-targets` and `cargo fmt --all` are silent.** `cargo test --workspace
--no-fail-fast` ends with the same two red tests in `openshard-state` and no
others — R1's finding, still filed under [*`can_step` does not check the
corner*](../../roadmap.md). With `OPENSHARD_CLIENT` set, the install-gated tests
in `client/render`, `client/artscan` and `server/state` have reds of their own;
none of them can be this node's, because `SpanIndex` has no caller outside
`spans.rs` and the three `movement` examples.

## What the node decided

**Three clauses became three flags, and every one of them is a property of the
column** rather than of the body or of where it came from. That is the test a
clause had to pass to be baked at all.

- **`GROUND` marks a column's own land.** One span of a column carries it, or
  none where the land is a mountainside. It is what the `landCheck` guard reads
  the tile's *lowest corner* off.
- **`LAND_WINS` carries three of that guard's four conditions.** The fourth,
  `test_top > land_z`, is start-dependent and stays in the query — where it is a
  comparison against the `GROUND` span's `reach_z`. **N2's section budgeted a
  land read for that residue and there is none**: the exception column already
  stores its own ground, which is the thing N1 stored one node before anything
  needed it. And the guard's *first* condition, `land_is_ground`, needs no
  storage either: it is whether the ground span survives the asker's ability
  filter, which is exactly what "water is a surface only to a swimmer" already
  means.
- **`CEILED` says whether `clearance` is a measurement or an absence**, and it
  is a correction to what N1 published. N1 argued a saturated 255 could only
  mean "nothing above", because a base and a `stand_z` are both `i8` and so a
  gap can never exceed 255. The bound is right and the conclusion is off by the
  boundary: a static based at 127 over a surface at −128 *is* a gap of exactly
  255, and it answers differently from an absence for a body needing more than
  255 over its feet — a body that walked in more than 239 above where it is
  landing. The bit closes it, and the byte that had seven spare now has four.

**`check` was not moved off `MapTerrain`.** N2 adds a second implementation and
proves it equal; it does not switch a caller. That is N3's, and doing it here
would have made the oracle compare a rule against itself.

**The example's step is a copy of `step_allowed`'s shape rather than a call to
it.** The corner rule is unchanged, so writing it out is what makes "the two
differ in exactly one place" visible at the place it matters.

## What was found

Filed in [`navigation_spans.md`](../design_spans.md) — the first in N3's own
section, because it is a decision that node has to take before it writes
anything; the rest in *Out of scope, named*.

- **🚩 `start_surface` is not a span, and N3 has to decide what to do about
  it.** N2 moved the *landing* half of a step and left the start half on the
  map, because `start_surface`'s second element is the **crest** — the art's own
  full extent, ServUO's `zTop` — and a span carries neither that nor the tile's
  *highest* corner. For a static the crest is recoverable from what is stored
  (`stand` flat, `2·stand − reach` climbable); for the land it is not, because
  an average and a minimum do not give a maximum. Three ways out are written
  down in N3's section, and the third — leave it on the map — has the numbers on
  its side: `start_surface` is asked once per node expansion against sixteen
  landing checks, and the landing half is already cheaper than A\*.
- **The map and the overlay still disagree about a platform of no thickness.**
  N1's finding, and **N2 could not settle it**: its whole content is that the
  answer did not change, so the one answer it may not change is this one. It
  stays open for N3, which is where both layers are consulted through one
  signature and the disagreement is finally visible in one place.
- **The `clearance` boundary**, above — filed as a correction to N1 rather than
  as a defect, because nothing had read the byte yet.

## What is next

**[N3 — the search takes `Spans`](2026-08-25-the-span-layer.md#n3--the-search-takes-spans).**
`find_path`, `find_path_toward`, `search`, `step_allowed`, `corner_open` and
`Around::read` take `&Spans` and `&Overlay`, and until they do, 169.1 ns is a
number in a benchmark rather than a shard that walks faster. Its oracle is
stricter than N2's — arrivals and node counts **bit-identical** to
[`terrain_seam.md`](../research/terrain_seam.md#what-one-search-costs)'s recorded run —
and the coarse half of it is already written: `span_check`'s two floods cost
three seconds.

**What would block it:** nothing. The `start_surface` decision is N3's to take
and the measurement that settles it is one `step_cost` row.
