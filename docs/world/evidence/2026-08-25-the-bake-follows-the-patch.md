# The bake follows the patch

S3's first artefact, and the one a person waits for.
[`navigation_spans.md`'s N8](2026-08-25-the-span-layer.md#n8--the-bake-follows-a-patch)
in full: a publish now rebakes the chunks it moved instead of the facet they are
in.

## Where it stands

**Built.** Measured by the test that produced the number it replaces,
[`publish_cost`](../../../crates/common/movement/tests/publish_cost.rs), on
Felucca under the profile a `cargo run` builds:

| | before | after |
|---|---:|---:|
| `Ground::publish` — the shard's tick | 109.7 ms | **0.3 ms** |
| `Ground::take_chunks` — the window's event-loop thread | 128.6 ms | **0.4 ms** |

`SpanIndex::rebake_chunks(map, tiles, chunks)` is the whole of it. A block is
rebuilt onto the *end* of the runs and its table repointed, which this layout
allows and `WorldMap`'s statics do not: a `BlockTable` carries its own `base` and
`counts`, where a prefix sum *is* the ordering. What that leaves behind is the
run nothing points at any more, counted in a new `dead` field.

- **The tables do not grow under a session.** A block that had a table keeps its
  slot and has it overwritten; only a block gaining its first static appends one.
- **The garbage rule is N8's, unchanged**: never compact during a session, except
  that dead spans exceeding live ones bake the facet whole and reset the count.
  On a facet-sized bake that is thousands of publishes away; the unit test drives
  it in two, on a fixture with one stored column.
- **The partial path is not public.** `rebake_chunks` is `pub(crate)` and
  `Ground`'s three writers — `publish`, `undo`, `take_chunks` — are its only
  callers, each passing the chunks it already holds. A caller that chose the
  chunks itself is a caller that can leave a stale column behind.
- **`Undo` learned to name its chunks**, beside `Patch`, over one shared function
  in `patch.rs`: an undo is a thing that can be held on its own, and a bake over
  the world it replaced is as stale as one over the world a publish replaced.

**Both ends already go through the door.** `runtime.rs:713` is the shard's
publish and `net_command.rs:273` is the window's `take_chunks`; neither changed.

## What was found in the doing, and is now written into the plan

**The rebaked area is a block wider than the chunks, west and north.** A column's
height is the average of the four cells meeting at its north-west corner
(`land_and_corners` reads `(x, y)` through `(x+1, y+1)`), so a cell that moved is
read by the columns one tile west and north of it — and across a chunk's edge
those live in the block before it. Baking only the edited chunk's blocks leaves a
one-tile seam answering for the world as it was, and **nothing about the edited
chunk itself would show it**. The unit test
`a_stored_column_across_the_chunk_edge_is_rebaked_too` is that failure, and it
was confirmed to fail with the widening removed before it was trusted.

The neighbour is rebuilt whole where only its edge could have moved. A block is
the unit this layer bakes in, and it is a ninth of the work either way.

## The oracles

- **On scenes**, beside `SpanIndex`: a patched facet answers exactly what the
  same facet baked whole answers, column for column, asked as a swimmer so no
  surface is filtered out of the comparison before it is made. Plus the seam, the
  locality (a publish where nothing is stored appends nothing and orphans
  nothing), the table reuse, the garbage rule, and a chunk taken off the wire.
- **On Britannia**,
  [`publish_locality`](../../../crates/common/movement/tests/publish_locality.rs),
  `#[ignore]`d: four edits — one inside a chunk, one on its eastern edge, one on
  its southern, one on the corner — each followed by a duplicated static, then
  148,996 columns around them (60,235 stored) and a stride-31 sweep of the rest
  of the facet, all against a whole bake. This is N8's third "done when", the one
  that is not a number.

## The second artefact, the same session: a block's statics are addressed

`WorldMap` held its statics as one facet-wide run addressed by a **prefix sum**,
which is the same illness one crate down: a block whose item count moved pushed
every static after it and repaired 458,752 offsets. Measured before: 0.02 ms to
**1.3 ms** depending on how much of the facet stood behind the block, and
3.9–5.6 ms for a publish that added an item.

`blocks: Vec<BlockRun>` — `base` and `count` per block — replaces it, and the
three writers become local:

- **A run that kept its length is written where it stands**, which is every edit
  to the *ground*: relocating it would manufacture garbage out of a publish that
  moved no statics at all.
- **A removal closes its own gap** inside the block and drops the count; only an
  **addition** has nowhere to put its item and goes to the end.
- `replace_blocks` no longer rebuilds the span between the first named block and
  the last — there is no span, so scattered blocks cost nothing extra and the
  `splice` is gone.
- Orphaned runs are counted and repacked once they outweigh the live ones, which
  is the span layer's rule verbatim. `static_count` answers what is reachable.

The price is the 1.75 MiB S3 named: 458,752 blocks × 8 bytes against the prefix
sum's 4. The read path is the same two reads it always was.

**Measured now**, by the same test — a publish that adds a static, which is the
one the layout is about: **0.4 ms** on the shard and **0.6 ms** at the window,
where the span half alone used to be 109 ms.

The oracle is `an_edited_facet_holds_what_an_imported_one_does`: a facet grown,
shrunk and replaced into shape holds what the same facet built by `from_parts`
holds, block for block and tile for tile. The byte-identity oracles
(`writing_the_same_facet_twice_writes_the_same_bytes`, the install-versus-base-set
terrain sweep) pass unchanged.

## What is next: the third artefact, and it is the same fix again

**The coarse graph.** `FacetState::publish` still drops the router — 11.6 s to
rebuild whole — and the plan for the local answer is now written down as
[`navigation_graph.md`'s G1](../design_navigation_graph.md#g1--the-graph-follows-a-patch).
Three things it says, so the next session does not re-derive them:

- the graph holds **two** prefix sums (`region_offsets` over regions,
  `edge_offsets` over nodes) and both want the same table this session put in
  twice;
- a `NodeId` is an index that *other regions' edges point at*, so a rebuilt
  region has to keep each surviving place's number — the bake already interns by
  place and throws the map away at `compact`;
- the area is **two rings and a half**: the touched regions grown by a tile,
  their neighbours (a portal is a fact about a border), and edges only in the
  ring beyond. That first ring's tile of growth is this session's seam, one scale
  up.
