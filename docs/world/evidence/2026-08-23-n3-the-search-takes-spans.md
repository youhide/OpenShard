# 2026-08-23 — N3: the search takes `Spans`

Era P's third node, in one commit. [`navigation_spans.md`](../design_spans.md)'s
N3 is the node where the measurement stops being a benchmark: N1 built the
layer, N2 proved the rule over it answers the same number, and **nothing on the
hot path called it**. Now everything does. **A node expansion is 208 ns where it
was 1,105, a search from Britain's castle is 0.168 ms where it was 0.793, and
every arrival count came back bit-identical to the run
[`terrain_seam.md`](../research/terrain_seam.md#what-one-search-costs) recorded.**

## Where it stands

| facet 0 around (1500, 1900), 10,836 standable tiles, fastest of five | ns |
|---|---:|
| one node expansion, one direction at a time — **what a search did before this** | 1105.5 |
| the same eight answers, landings over the map, work hoisted | 364.1 |
| the same, landings off the bake | 167.1 |
| **`steps_out_of` — what a search does now**, overlay included | **208.1** |
| pure A\* with the terrain taken away | ~183–191 |

All four expansion rows carry the same checksum, which is the only reason the
comparison means anything. The terrain half of a node is now the same size as
A\*'s own machinery rather than seven times it.

**The oracle, over the three origins the plan named**, 37,248 destinations each:

| origin | arrived @400 | arrived @600 | p50 @600 was | p50 @600 now |
|---|---:|---:|---:|---:|
| (1363, 1600, 30) the castle plateau | **4,036** | **4,436** | 0.793 ms | **0.168 ms** |
| (1434, 1699, 2) the bank | **6,138** | **7,389** | 0.851 ms | **0.170 ms** |
| (1500, 1900, 0) open country | **17,458** | **18,093** | 0.570 ms | **0.150 ms** |

**Six arrival counts, six matches to the unit** — and the per-class node
distributions match too: `goal/region` 111/453 and `goal/far` 165/558 from the
bank, unchanged from the recorded table. A faster search that found different
routes would be a different search.

**`cargo check --workspace --all-targets`, `cargo clippy --workspace
--all-targets` and `cargo fmt --all` are silent.** `cargo test --workspace
--no-fail-fast` ends with the same two red tests in `openshard-state` and no
others — R1's finding, still filed under [*`can_step` does not check the
corner*](../../roadmap.md).

## What the node decided

**`start_surface` stays on the map**, which was N3's one open decision. The
plan asked for a measurement and it is **23.3 ns of a 170.8 ns node expansion**
— asked once against the landing half's eight. But the deciding argument turned
out to be a second one the plan had not seen: **`start_surface` is
order-dependent in a way a span list cannot reproduce.** Its loop keeps a
running maximum over the column's statics *in the map file's order* and
accumulates `z_top` over everything that passed on the way, so a climbable with
a low surface and tall art is selected when met first and skipped when a
flatter, higher-surfaced static is met first — and highest-first is the only
order spans are stored in. Baking the start half is not a fourth byte; it is a
second table holding the file's order. Both halves of that are filed.

**The bake is not optional where the map is.** `MapTerrain::new` takes a
`&SpanIndex` as its third argument, so a terrain that would silently re-derive
every column cannot be spelled, and `Footing::of` panics rather than accept a
map without a bake. This is the shape the plan's `Option` would have had, and
the reason it is not one: a fast path a holder can forget is a 6× regression
with nothing saying so.

**It sits beside the world rather than in it** — `FacetState::spans` on the
shard, `Resources::spans` on the client, `Scene::spans` in a fixture — because
`openshard_map` is underneath `openshard_movement` and where a body may stand is
a movement rule, so `World` cannot hold the projection of its own two layers.
Two seams keep the pair honest and there is no third: `FacetState::set_map`
moves the ground and its bake together, and `World::with_tiles` rebakes every
facet already loaded, so the builder's argument order cannot leave a bake over
the empty tile table.

**`steps_out_of` is the primitive, and `step_allowed` is one slot of it.** A
node expansion resolves the tile stepped *off* once and answers each neighbour
once — sixteen landing checks become eight, because the four cardinals a
diagonal asks about as flanks are the four already being asked about as
destinations. `step_allowed(footing, from, dir)` is *defined* as
`steps_out_of(footing, from)[dir]`, so a single-direction caller pays for the
whole expansion. That is deliberate: the alternative is a second copy of the
step rule that can drift from the one a search uses, and a diagonal already
asked about three tiles.

**N2's own oracle had to be re-armed, and that is the shape of every node after
this one.** `span_check`'s coarse half flooded the facet twice — once through
the shipped `step_allowed` and once through a written-out span rule. The moment
`step_allowed` reads the bake, that flood compares the bake against itself and
reports zero differences for the wrong reason. It was caught by the map flood
getting *slower* (5.8 s against N2's 4.2 s — `step_allowed` is now a whole
expansion per direction), which is the sort of tell that is easy to read as
noise. Both sides are written out in the example now, and the oracle holds:
**248,268,125 steps, 0 disagreements; both floods reach 3,747,934 tiles, 0
differing**, map 4.0 s and bake 2.9 s.

**`MapTerrain::check` is now an oracle and nothing else.** No production caller
reaches it — `can_step`, `land_at`, `surface_at` and `predict_step` all read
`Spans::check`. It stays because it is the only statement of the rule *in terms
of the map files*, and `span_check` is 248 million comparisons of one against
the other. Its doc says so, in the imperative, so nobody deletes it as dead.

**The signature the plan wrote is not the signature that landed.**
`find_path(&Spans, &Overlay, …)` was written down in N1 and repeated in N3, and
it cannot be: `obstructed`, `can_fit`, `sight_clear` and `start_surface` are
four rules on the same footing that still need the tile table. So the footing
stayed the carrier and the bake went *inside* the terrain. Recorded in the plan
as a correction rather than quietly.

## What was found

Filed in [`navigation_spans.md`](../design_spans.md)'s *Out of scope, named*.

- **🚩 `start_surface` cannot be baked without baking the map file's order** —
  above. It is the entry point for anybody who returns to the start half.
- **`WorldState::tiles` is a public field, and writing it does not rebake.**
  Both real seams are safe; a direct `state.tiles = table` leaves every facet
  holding a bake over the old table. The repair is a private field behind a
  setter that rebakes, which is the shape `FacetState::obstructions` already has.
- **The interiors bake builds two facet-wide span indexes of its own.**
  `PlanarTopology::bake` and `Buildings::bake` each need a terrain and now build
  a `SpanIndex` for it — 0.07 s each. Threading one through five `bake`
  signatures would put a movement index in the arguments of a wall contour; the
  honest fix is for that bake to take its ground as one value.
- **A test that calls the shipped rule stops testing it the moment the rule
  moves.** Above, and worth carrying to N3b and N4: both will move something
  `span_check` or `coarse_bench` currently reaches through a shipped entry
  point.
- **A `Scene` rebakes on every setter.** The price of `Scene::terrain` taking
  `&self`: nothing in the suite notices, a fixture that grows will.
- **The map and the overlay still disagree about a platform of no thickness.**
  N1's finding, and **N3 could not settle it either** — its oracle is that the
  routes did not change. It is now visible in one place, which is what N3 was
  expected to buy: `walk::landing` consults both layers in six lines. It needs a
  decision, not another node.

## What is next

**Two nodes are open and nothing forces the order between them.**

**[N3b — the node stops being a tile](2026-08-25-the-span-layer.md#n3b--the-node-stops-being-a-tile)**
is the finer defect and the one N3's oracle was written to exclude: a column
with two standing places still gets one slot in `closed`, and *"from this
column's ground floor to its first floor"* still returns success with an empty
route. The key widens to `(x, y, span)` — twenty-nine bits of the `u32` it
already uses — and the four things that follow are enumerated in its section.

**[N4 — regions over spans](2026-08-25-the-span-layer.md#n4--regions-over-spans)** is
the user-visible repair: the coarse graph stops modelling one height per tile,
so a creature can route out of Britain's castle. It needs only N2.

**What would block either:** nothing.
