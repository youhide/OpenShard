# The bake follows the patch

S3's first artefact, and the one a person waits for.
[`navigation_spans.md`'s N8](../navigation_spans.md#n8--the-bake-follows-a-patch)
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

## What is next, in S3

The span layer was 115 ms of it at both ends and is now 0.3. What is left of
[`what_a_change_costs.md`](../new_map_representation/what_a_change_costs.md)'s S3
is the other two, and they are not one problem:

- **`WorldMap`'s statics** — the same shape and the same fix: a per-block
  `base`+`count` table in place of the prefix sum, 1.75 MiB on a 150 MiB world,
  which is the only thing between `chunk::apply` and O(the chunks that arrived).
  A `.setland` moves no statics so it does not show in the number above; a
  publish that *adds* an item is 3.9–5.6 ms.
- **The coarse graph**, direction D's own and the 11.6 s one: `mapedit::commit`
  drops the router on every publish, which is correct and is not free.
  `touched_chunks` names the 32×32 regions to rebuild, plus the half a naive
  implementation forgets — the neighbours whose answer *crossed* into the changed
  chunk. The seam this handoff found is that same half, one scale down.
