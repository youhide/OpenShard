# 2026-08-23 — a flood is one thing, and there are no longer three of it

The last finding
[`navigation_spans.md`](../design_spans.md)'s *Out of scope, named* still
carried from N4: **the bake stopped paying eight times over for every neighbour
and every other flood in the tree kept paying it.** It was filed as one line in
two places. It is not one line, and that is the whole of this session.

## Where it stands

**Fixed**, and the fix is a module rather than an edit.

### There were four floods and three of them were copies

A flood over the step rule is how this area proves anything about ground: a
search that refuses is only interesting against an answer to *is there a way at
all*. Four had been written.

| | |
|---|---|
| `NavigationGraph`'s own | `component_labels`, `region_costs`, `region_costs_into` — over *places*, inside one region, and already asking `steps_out_of` once per place since N4 and [the join](2026-08-23-the-join-is-a-flood.md). |
| `Scene::reachable` | every scene fixture's oracle, and the shard's own too — `world/src/tick/tests.rs` asks it whether the ground a router refused is walkable at all. |
| `coarse_bench`'s `land_component` | the whole-facet ground truth a `refused_but_walkable` count is read against. |
| `span_check`'s `flood` | the same traversal again, with the step rule handed in, because that example compares two readings of the landing half. |

The last three were the same twenty lines three times, and each walked the eight
directions one `step_allowed` at a time — the whole expansion asked for eight
times and used once, since `step_allowed` is *defined* as one slot of
`steps_out_of`.

**Repairing the expansion in three places would have left three places**, which
is what the filed line proposed and why it was worth not doing. There is one
flood now — [`reach::Reach`](../../../crates/common/movement/src/reach.rs) —
and the two entry points are the two questions a caller actually has:

- **`Reach::of`** walks the shipped rule: one `steps_out_of` per place popped.
- **`Reach::by`** takes the expansion handed in — eight landings by
  `Direction::to_bits` — for an oracle that must *not* go through the shipped
  rule. Since N3 `step_allowed` reads the span bake, so a flood through it would
  be the bake compared against itself; `span_check` writes both landing rules
  out for that reason and now writes nothing else out.

### What it costs, measured

**A/B on one tree**, facet 0, release, the castle at (1363, 1600, 30) — the
second reading taken by putting the eight-times-over expansion back by hand and
running the same binary again.

| | eight `step_allowed` | one `steps_out_of` |
|---|---|---|
| `coarse_bench` whole-facet flood | 5.1 s | **0.9 s** |
| `span_check`, map rule | 4.4 s | **2.8 s** |
| `span_check`, span rule | 2.6 s | **1.5 s** |

**The answers did not move.** The flood reaches 3,747,934 tiles — the number
recorded in N2 and N4 — from both builds, and `span_check`'s oracle still says
**248,268,125 steps, 0 disagreements; both floods reach 3,747,934 tiles, 0 tiles
differing.**

`span_check`'s halves are the smaller ratio for a reason worth naming: its two
rules are per-direction by construction, so the traversal was never what it paid
for. What it paid for was **the corner rule written once per side** — a diagonal
asking its two flanks again after the rule had already answered for them. That
is folded into one `expansion` now, in the shape `steps_out_of` gives it: every
neighbour resolved once, a diagonal refused where either flank has no landing,
read off answers already in hand. Sixteen landing checks per place became eight,
on both sides, identically.

## What was decided

**Keyed by tile, not by place.** A tile first reached at one height is marked
reached and not explored again at another, so a gallery over a street is one
entry. That under-counts and never over-counts, which is the direction an oracle
wants — a tile it calls reachable really is one. All three copies already did
this; it is stated once now instead of three times. A flood keyed by *place*
over a whole facet is a different structure over a different index, and it is
[N5](2026-08-25-the-span-layer.md#n5--off-mesh-links)'s own first step rather than
something to build before anyone has asked it a question.

**The rectangle is the flood's only edge.** The step rule refuses a landing off
the map by itself, but a footing with **no** map is open ground in every
direction and has no edge to find — so a dense flood over one would run past its
own array. The bound is one comparison per landing and it is in the traversal,
where the three copies each had it implicitly through a map they happened to
have.

**`Scene::reachable` keeps returning a map.** A fixture asserts against the
tiles that are *in* the answer — `contains_key`, `keys().all(…)`, `len()` — and
a dense rectangle is the wrong shape for that. What it stopped owning is the
traversal. The projection is a walk of a scene's own eight-by-eight.

**The interiors floods are not folded in, and that is not an oversight.** The
building flood in [`render/src/interiors.rs`](../../../crates/client/render/src/interiors.rs)
walks a *planar topology* — wall bits between cardinal neighbours, doors, no
heights and no step rule. It shares the word and nothing else.

**`crossings` is not one either.** It asks one direction per place on a border
tile, so `step_allowed` is the right primitive there and the expansion it
resolves is not waste it can avoid — a flood wants all eight, and this wants
one.

**One stale sentence went with it.** `COARSE_MIN_DISTANCE` still said the join
is "one exact search per node of the endpoint's own region", which the previous
handoff had replaced with a flood. The threshold is still real — the region is
walked either way — and the doc says which it is.

## What is clean

`cargo fmt` on every file touched, `cargo clippy -p openshard-movement
--all-targets` silent, and the suites of both crates this reaches:
**`openshard-movement` 144 + 1 + 7 + 5 doctests, `openshard-world` 616.** The
three tests new here are `reach.rs`'s own: a wall is where the flood stops, open
ground is flooded no further than the rectangle it was handed, and a handed-in
rule is the one walked.

Not this work's and unchanged: `cargo clippy --workspace --all-targets` is not
silent on `main` — one `needless_borrow` in `crates/common/uofiles/src/map.rs`
and three in `crates/client/render/tests/traced.rs`, both of which parallel
sessions have open.

## What is next

**No node of [`navigation_spans.md`](../design_spans.md) is open**, and what
is left in *Out of scope, named* is filed observation — with the one exception
below, which arrived from a parallel session while this was being written.

**The previous handoff's one open defect closed while this was being written**,
and by a parallel session rather than by this work: `map: one corner rule, and
the shard walks its creatures by it` (`d96d987a`) put `World::step` and `ai`'s
`probe` on `step_allowed`, so the shard no longer permits a corner cut its own
router refuses to plan.

**The same eight-times-over shape has one instance left**, and it is that
session's filed observation rather than this one's: `items/mounts.rs` resolves
the same stance eight times — eight `can_step` calls — to find somewhere beside
a rider to put a mount down. It is not a flood and `Reach` is not what it wants;
`steps_out_of` is. What has to be decided before the swap is whether the corner
rule belongs there at all, since a mount is *placed* beside its rider rather
than walked there. Filed in
[`navigation_spans.md`](../design_spans.md)'s *Out of scope, named*.

**Where `Reach` gets its sibling** is N5, whose done-when is a flood over the
graph against a flood over `Spans` reaching the same set. That is the
place-keyed one, and the tile-keyed flood is what will say whether the two
disagree at all before anyone builds it.

**What would block it:** nothing.
