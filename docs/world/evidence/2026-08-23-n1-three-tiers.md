# 2026-08-23 — N1: three tiers of standable surface

Era P's first node, built in one commit: `9a6ecb2a` three tiers of standable
surface. It is the layer HPA\* assumes underneath it and this engine never had —
a column as a *list* of places a body can stand rather than one height — and
nothing reads it yet, which is exactly what
[`navigation_spans.md`](../design_spans.md)'s N1 asked for.

## Where it stands

[`spans.rs`](../../../crates/common/movement/src/spans.rs) holds it, and the
structure is the plan's own: a block with no statics is the map's empty block, a
column with no statics is the land grid read live, and everything else is a
stored span list addressed CSR — a per-block base plus a `[u8; 64]` of counts,
indexed by the land's own `BlockIndex` so there is no second block addressing to
keep in step.

| | estimated by the plan | measured |
|---|---:|---:|
| resident | under 20 MB | **16.5 MiB** (17,305,856 B) |
| …block index | 1.8 MB | 1.8 MB (458,752 × `u32`) |
| …count tables | 7.4 MB | 8.2 MB (120,744 × 68 B) |
| …spans | ~9.6 MB | **6.5 MB** (1,635,392 × 4 B) |
| spans stored | ~2.4 M | **1,635,392** |
| build | "less than the census's 3.5 s" | **0.05 s** |

**The equivalence is the whole of the acceptance, and it passed cold:**
`Spans::surfaces(x, y)` returns exactly what
[`stand_surfaces`](../../../crates/common/movement/src/surfaces.rs) returns for
all **29,360,128 columns** of facet 0 and **both abilities** — 0 disagreements,
compared in 2.0 s by the
[`span_index`](../../../crates/common/movement/examples/span_index.rs) example.

The span count came in a third under the estimate because the estimate counted
an exception column as at least one span: a column with statics whose land is
water or mountainside stores no ground span, and a column whose statics are all
walls stores only the ground.

**`cargo check --workspace --all-targets`, `cargo clippy --workspace
--all-targets` and `cargo fmt --all` are silent.** `cargo test --workspace
--no-fail-fast` ends with the same two red tests in `openshard-state` and no
others — R1's finding, still filed under [*`can_step` does not check the
corner*](2026-08-24-runtime-lookups-and-the-tick.md).

## What the node decided

**Two types, where the plan wrote one**, and the middle tier is why rather than
taste. `SpanIndex` is the bake — owned, no lifetimes, built at facet load and
kept beside the map the way `NavigationGraph` is. `Spans` is the view a question
is asked through: the index, the map, and the ability of the asker, built where
it is asked, which is `MapTerrain`'s own shape. A single type would have to
either borrow the map — and then it could not be stored beside it in
`FacetState` — or store a second copy of the ground, which is the one thing the
column tier exists to avoid. `find_path(&Spans, &Overlay, …)` is unchanged as
the shape N3 lands.

**The land surface of an exception column *is* stored**, where a bare column's is
not. That looks like the two tiers disagreeing and is the opposite: a column with
statics has a *headroom* over its ground, and the byte that says so has to sit
beside the height it is a headroom above. It is also what makes `count == 0`
safe to answer from the land — a stored column with no spans at all is one whose
ground is mountainside and whose statics are walls, and the land answers
"nothing" for it too.

**Highest first.** The bake sorts a column descending by `stand_z`, where
`stand_surfaces` is in the map file's own order. The step rule wants the highest
surface within reach (Sphere's `GetFixPoint`), so the order it walks in is the
order it is stored in and the first candidate that passes is the answer.
`MapTerrain::surfaces` documents itself as unordered, so nothing was relying on
the other order.

**`MapTerrain::static_top` became a free function** the bake shares. Two readings
of "how tall is this static" would be a wall the step rule and the layer under it
disagreed about.

## What N2 will need, read in advance

The plan asked for this before the builder was written, because what N1 stores is
decided by what N2 has to prove. `check` reaches its source through `start_z` and
`start_top` — and through **one clause more**:

- **The reach test** is `step_top >= item_top`, and `item_top` is `reach_z`.
- **The obstruction test** is carried by `clearance`, and **exactly**, not
  approximately: a static's base and a span's `stand_z` are both `i8`, so nothing
  can sit more than 255 above a surface, and a `clearance` saturated at 255
  therefore means *nothing above at all*.
- **The ServUO `landCheck` guard** is the clause that reaches further. Three of
  its four conditions are properties of the column; the fourth, `test_top >
  land_z`, is start-dependent and unconditionally true whenever `our_z +
  PLAYER_HEIGHT > land_z`. So it is one flag bit plus, in the residue, a land
  read the query already knows how to make.

**The bit is deliberately not stored yet.** It is a rule, and a rule whose oracle
has not run is a rule written twice. The span's flags byte has seven spare bits
and the layout does not move to gain one.

## What was found

Two things, filed in [`navigation_spans.md`](../design_spans.md)'s *Out of
scope, named*. Neither blocks N2.

- **The count tables are bigger than the spans they address** — 8.2 MB against
  6.5 MB, because only 2.3 M of the 7.7 M columns in a static-bearing block are
  exceptions and the other 71% of every table is a zero. A per-block `u64`
  occupancy mask with counts for the occupied columns only is about 3.3 MB and
  replaces a 64-byte prefix sum with a `count_ones`. Gated on N3's measurement,
  like the packed static record: it is a layout change with a query change in it.
- **The map and the overlay disagree about a platform of no thickness.**
  `MapTerrain::is_obstructed` gives one a body from `base` to `base`, so it is in
  the way of anything *below* it whose head passes the floor; `Cover::of_static`
  lays no blocking half for the same art at all. A floor the map shipped and a
  floor the shard placed answer differently for a body underneath. The bake
  reproduces the map's reading, because that is what N2's oracle compares
  against; which of the two is right is a finding about the step rule.

## What is next

**[N2 — the step rule reads them](2026-08-25-the-span-layer.md#n2--the-step-rule-reads-them).**
`check(x, y, start_z, start_top)` becomes a walk of the target column's spans
rather than of its statics, and the whole of the node is proving it did not
change an answer. Both oracles already exist: per-step agreement against
`step_allowed`, and the whole-facet flood
[`coarse_bench`](../../../crates/common/movement/examples/coarse_bench.rs) uses
as ground truth — the second is the one that would have caught the one-storey
defect.

**What would block it:** nothing.
