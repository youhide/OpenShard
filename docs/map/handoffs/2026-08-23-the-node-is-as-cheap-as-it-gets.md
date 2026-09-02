# 2026-08-23 — the node is as cheap as it is going to get

[`navigation_spans.md`](../navigation_spans.md)'s **baked adjacency**, which the
previous handoff named as *the next useful measurement*. It is measured, and it
closes as **declined** — together with two smaller ideas around it and one the
measurement raised on its own. Nothing was built into the search, and that is
the outcome rather than a shortfall: **the node-expansion question is closed.**

## Where it stands

**Measured, declined, recorded.** Three commits, and the third corrects the
first two.

### What a mask could ever have attacked

A neighbour mask hangs on a span, and **92% of the facet's columns have no
span**. So the number that decides the entry is not the saving but the
*population*, and nothing here could ask for it: `step_cost` sampled a square
and timed it whole. It splits by tier now, through `SpanIndex::stores` —
`column_count` for a single column, and in that family deliberately.

Two origins, release, `--repeat 25`, least of three runs:

| | open country (1500, 1900) | the castle (1363, 1600) |
|---|---|---|
| starts on a stored column | 16.0% | 46.3% |
| expansion, whole node | 198.9 ns | 210.5 ns |
| expansion, stored start | **170.4 ns** | 225.4 ns |
| expansion, bare start | 193.9 ns | 184.1 ns |
| the floor — landings free | 31.7 ns | 42.6 ns |
| landings off the bake | 155.4 ns | 171.6 ns |
| the same, all eight on one column | 137.6 ns | 151.0 ns |
| of 8 neighbours, refused from a stored column | 12% | 20% |

**Four ideas, and the run refuses each for its own reason:**

| | |
|---|---|
| **the premise** | Recast's is that the columns with geometry are the expensive ones. In open country a stored start is **170.4 ns against bare land's 193.9** — the bake *beats* the land grid, because a span carries its height where a bare column derives one from four corner reads. Over 84% of the sample the premise is not weakened but **backwards**. It is dearer only at the castle, where the run being walked is a wall's. |
| **the rejection mask** | One bit per direction, no heights, 1.6 MB — the cheap, census-approved half. It can only save the reads it *refuses*, and a stored column refuses 12–20% of its eight neighbours: one read of eight, on 16–46% of expansions. **Under 2% of a node.** |
| **the full record** | A mask plus eight landing heights is ~9 bytes on a 4-byte span: **~15 MB against the bake's 11.2 MiB**, more than doubling it, to make 16% of open-country and 46% of castle expansions ~4× cheaper. Weighted against a whole node: **5% and 19%**. Nothing asks for 5%, and the 19% is the case the coarse graph already routes around. |
| **the locality hoist** | Raised by the floor row and refuted by the same run — see below. |

### The floor, and the idea it raised and killed

The new **floor** row is an expansion with the landings *free*: 31.7 / 42.6 ns
against 198.9 / 210.5. So the landing half is ~155–172 ns for eight neighbours,
~20 ns each, where `surface_at` measured alone on one column is 12.4.

The obvious reading of that gap is locality — eight neighbours are eight walks
of the same addressing chain (`extent().index_of`, `blocks`, `tables`, the
occupancy word, the prefix sum) for tiles that share a block whenever the node
is not on a block edge. That is **N3's `Stance` hoist one tier down**, and it
costs no bytes, so it looked like the better find.

**It is not.** *All eight on one column* is those eight lookups aimed at one
column — same `check`, same tier, same arithmetic, only the addressing hot and
shared — and it is **137.6 against 155.4** in open country and **151.0 against
171.6** at the castle. The same one eighth, in two places that share nothing
else. A hoist threading a resolved block through `MapTerrain::check` into
`Spans::check` buys ~9% of an expansion and ~4% of a node, across the seam N3
and the terrain work spent two sessions making narrow.

**The landing half is not addressing. It is ~19 ns of rule, eight times** —
which is what N1's three tiers already brought it down to.

### 🚩 The default repeat count was lying, and it published a number

The first two commits took `step_cost` at its default five passes on a machine
at **load average 33 on 24 cores**. Every row is the least of `--repeat` passes,
which is the right estimator under load — the fastest pass is the least
disturbed — but only once there are enough passes for one to run clean. At five
the rows moved 30% run to run, and one of them read *stable*: open country's two
tiers at 204.5 against 204.6, published as **the two tiers cost the same**.

At `--repeat 25` it does not reproduce. The stored tier is 170.4 against 193.9,
and the entry's argument gets **stronger** — but it was published wrong first,
and the correction is its own commit rather than a quiet edit.

**The discipline went into `step_cost`'s own module doc, not into this
handoff**, because the next person to quote a row from it is the one it is for:
raise `--repeat` on a busy machine, take three runs, quote the least, and put
the repeat count beside any number kept.

## What was decided

**Baked adjacency is closed as *measured and declined*, not deferred.** The
distinction matters: a deferred node waits for a trigger, and this one has had
its trigger fire. The plan's own words were "if a node expansion ever has to get
cheaper again", and the answer recorded is that *there is no cheap way left to
make one cheaper* — not that nobody got round to it.

**The node-expansion question itself is closed with it, and a session should not
re-open it without a new reason.** A node is ~200–210 ns of terrain against
~225 ns of A\*'s own heap and hash. Five attacks on the terrain half are now
priced in one run of `step_cost` — the full record, the rejection mask, the
locality hoist, and the dense `average_land_z` before them — and **none moves a
node by a tenth**. The lever that is left is not a cheaper node but **fewer
nodes**, which is the coarse graph: built in N4, read by the shard since N7,
and its endpoint join made a flood two handoffs ago.

**Nothing was added to the search, and nothing was rebaked.** `Span` is four
bytes as it was, `SpanIndex` is 11,713,607 B as it was, spans are not
serialised, and `ROUTING_VERSION` stays 4. The only production change is one
`#[must_use]` predicate.

## What is clean

`cargo check --workspace --all-targets` and `rustfmt` on both touched files.
`openshard-movement`: 144 + 1 + 7 + 5 doctests. `cargo clippy -p
openshard-movement --all-targets` is silent.

**Not ours and still there**, exactly as the previous handoff filed them in
[`roadmap.md`](../../roadmap.md): a `needless_borrow` in
`common/uofiles/src/map.rs`, three borrowed expressions in
`client/render/tests/traced.rs`, and a 640-byte enum variant in
`client/app/src/link.rs`.

## What is next

**Nothing in [`navigation_spans.md`](../navigation_spans.md) is open**, and
*Out of scope, named* now holds one fewer entry with anything to do in it. N5
and N6 remain **gated rather than queued**, as the last four handoffs left them:
N5's content is empty until a flood says what the spans cannot connect, and N6
waits for a number nobody has asked for.

**Era P has nothing queued in it.** What the map area has next is era S's
[direction C, second half](../new_map_representation/plan.md#c--patches-and-the-resolved-snapshot) —
a live publish: an edit taking effect in a running shard between two ticks and
reaching a connected client. Its precondition landed with
[`terrain_seam.md`](../terrain_seam.md)'s D; what is open is **who calls it and
where in the tick**.

**One thing this session filed rather than fixed**, in
[`roadmap.md`](../../roadmap.md): `step_cost`'s `expand` helper is a second copy
of the diagonal flank rule, and three of its rows now go through it. It is the
instrument the plan measures with, so a drift between it and `steps_out_of`
would be a wrong number rather than a failing test — the same class as
[`parity.md`](../../render/design_frame_assembly.md)'s frame assembled by hand in seven places.

**What would block it:** nothing.
