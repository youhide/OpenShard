# 2026-08-23 — the filed observations are taken, and one of them was a constructor

[`navigation_spans.md`](../design_spans.md)'s *Out of scope, named* had four
entries left with something to do in them — no defects, by the previous
handoff's own account, but four places where the code and what this repo says
about itself had drifted apart. All four are taken.

## Where it stands

**Four fixed**, in the order they were listed, each with its own commit.

### The tile table is private, and five literals become one constructor

`WorldState::tiles` was public. Every facet's span bake is a statement about
that table as much as about its ground, so `state.tiles = table` left the shard
deciding steps by the heights of a world it no longer had — and since
[`Ground`](../../../crates/common/movement/src/ground.rs) closed the other half,
it was the last way to hold a bake describing neither world in hand.

The field is private, read through `tiles()` and replaced through `set_tiles`,
which is where `World::with_tiles`'s rebake loop moved. The write and the rebake
are one call.

**What that cost is the part the finding had not seen.** A struct with one
private field cannot be written as a literal outside its own module, and
`WorldState` was written as one in **five** places — `World::new` and a fixture
each in `party`, `guilds`, `boats`, `housing` — every one naming all twenty-four
fields, so a field added here had to be added in five places or nowhere.
`WorldState::new` replaced them: facets, default facet, tiles, multis, start and
a seed, everything else starting empty. **The four fixtures shed twenty imports
between them**, which is the measure of how much of each literal was ceremony.

*Done when:* `a_late_tile_table_rebakes_every_facet` — **two** facets over an
empty table, each with a wall the table cannot see, and a `set_tiles` that has to
reach both. The control is the loop deleted by hand, where it fails at the first
facet; one facet would have passed a rebake that only ever touched the default.

### A dismount asks the step rule once, and takes its corner rule

Putting a mount down looked for somewhere beside the rider with eight `can_step`
calls, each re-deriving the tile being stepped off. `steps_out_of` is those eight
answers for the price of one.

**The rule was the decision, not the swap.** A mount is *placed* beside its
rider, not walked there, so the corner rule is not obviously owed. It is taken
anyway: every step the shard permits has carried it since `World::step` went
through `step_allowed`, so a horse put down through a cut stands where nothing
could have walked it, and where the same rule can refuse to walk it out again.
"Nowhere beside the rider" was already an answer this code had — under the rider
— and it is the better of the two.

**What fell out of it, and it is worth knowing before reading that loop:** with
the corner rule in it, a diagonal is never what it picks. A legal diagonal needs
both flanking cardinals to be steppable and both come earlier in
`Direction::to_bits` order, so the choice is the first open cardinal or the
rider's own tile.

*Done when:* `a_dismount_does_not_put_a_horse_through_a_corner` — a rider boxed
in by seven crates whose one open neighbour is a corner cut. Reverted to
`can_step` the horse stands on the diagonal, at (1364, 1601) against
(1363, 1600). The control is in the test: the northern crate taken away, where
the same dismount uses it.

### The roof threshold follows a step, not a landing

`advance_cutaway` moved the cutaway source when the move was "locally known to
be possible", and asked `can_step` — so once the shard started refusing a corner
cut, the two disagreed by exactly that: a direction held into a building corner
moved the threshold for a step about to be rubber-banded.

The guard is a function of its own now, `cutaway_follows`, **which is what makes
it testable at all** — the threshold is otherwise reachable only through a packet
fold on a live `App`. It also has to say what a step *is*, since `step_allowed`
takes a direction where `can_step` took two points: a move that is not one step —
the body already standing where the threshold is, a z that changed under it, a
gate, a push — is answered *yes* rather than measured, because a threshold left
behind hides the body the cutaway exists to reveal.

*Done when:* `the_cutaway_does_not_follow_a_corner_cut`, with the two
not-a-step cases asserted beside it. Reverted, it fails at the first assertion.

### The interiors bake takes the terrain it reads

`PlanarTopology::bake` and `Buildings::bake` each built a facet-wide `SpanIndex`
to get a terrain, 0.07 s apiece inside a bake that already walks the facet, and
the client built a third at startup.

**The value that carries all three is `MapTerrain`, not `Ground`** — which is
where this differs from what the finding proposed. A bake does not want to own a
facet; it wants the map, the table, and the index it reads them through.
`Ground::terrain(tiles)` is what *produces* one, and taking it made the
signatures **shorter**: every `map: &WorldMap, tiledata: &TileData` pair in
`interiors.rs` — twenty of them, across the block bake, the room bake, the
stitch, the building bake, the wall helpers and the `Index` cache — is one
`terrain: &MapTerrain<'_>`, and the two `SpanIndex::build` calls went with them.

Each caller hands over the bake it already has: the client's is
`Resources::terrain`, its `Ground`'s own — the third index it used to build —
while `artscan` and the census example build one apiece per run instead of one
per bake.

**No timing was taken.** What is claimed is two builds removed and the client's
reused, not a number. The oracle is that the answers did not move.

### And the pair of readings stops calling itself ground

`steer::Ground` is two `Footing`s — the same map as the doors stand and again
with them open. It is `Readings` now. Nothing else moved; the name was the whole
of the defect, and it is the rename the finding itself asked for.

## What is clean

`cargo check --workspace --all-targets` and `cargo fmt --all` are silent.
Suites: `openshard-state` (131 + 1), `openshard-world` (617),
`openshard-client-render` (658), `openshard-client-app` (382), plus
`openshard-items`, `openshard-housing`, `openshard-boats`, `openshard-party`,
`openshard-guilds`, `openshard-skills`.

**`cargo clippy --workspace --all-targets` is not silent, and none of it is
this session's**: a needless borrow in `common/uofiles/src/map.rs`, three
borrowed expressions in `client/render/tests/traced.rs`, and a 640-byte enum
variant difference in `client/app/src/link.rs`. The first four are a parallel
session's open files. Filed in
[the engineering findings](../../client/evidence/2026-08-27-engineering-follow-up-findings.md).

## Three things worth carrying

**A parallel session committed this work's first half.** `bdaac0af` ("upd")
swept up the in-flight `WorldState::tiles` edits before they were finished, so
that repair is spread over two commits and only the second has a message. It is
the expected hazard of a shared tree and cost nothing here; it is recorded
because anybody reading `227a3e1c` alone will find the field already private.

**Nothing checks an intra-doc link.** `steer.rs` carried `[`Ground::real`]` and
`[`Ground::through_doors`]`, two fields that stopped existing when the pair of
terrains became one `Footing`, and both survived every test and lint run since:
`cargo doc` is not one of the three commands and
`rustdoc::broken_intra_doc_links` is a rustdoc lint. Filed in
[the engineering findings](../../client/evidence/2026-08-27-engineering-follow-up-findings.md).

**One `openshard-world` run went red and did not reproduce.** It happened on the
first run after a workspace rebuild and passed on every run after; the panic
text was not captured, so it is **not** attributed. The suite has a known
load-sensitive test —
`a_creature_routes_past_its_exact_budget_over_the_coarse_graph`, which reads 50
ms of wall clock per plan — and that is the likely one, but likely is not
settled. Whoever sees it again should keep the panic line.

## What is next

**Nothing in [`navigation_spans.md`](../design_spans.md) is open**, and
*Out of scope, named* now holds only entries with nothing to do: the map and the
overlay disagreeing about a platform of no thickness (a decision, not a node),
`start_surface` not being bakeable without the file's order, the `Scene` that
rebakes on every setter, a dense `average_land_z`, baked adjacency,
`sight_clear`'s height blindness, and the statics layout — each already pointing
at the plan that owns it.

**N5 and N6 remain gated rather than queued**, exactly as the last three
handoffs left them: N5's content is empty until a flood says what the spans
cannot connect, and N6 waits for a number nobody has asked for.

**What would block it:** nothing.
