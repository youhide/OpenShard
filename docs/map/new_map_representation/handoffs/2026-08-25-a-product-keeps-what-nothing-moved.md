# A product keeps what nothing moved

S2's cache half, from [`what_a_change_costs.md`](../what_a_change_costs.md). A
publish now costs the radar cache the chunks it touched and nothing else.

## Where it stands

**Built.** `RadarCache::moved(facet, revision, touched)` — the same statement as
`set_revision` plus the list of level-zero chunks whose content changed:

- **Every product with no touched chunk under it is carried** to the new
  revision, at every level of the ladder. One test answers all of them: a
  product at level *n* covers exactly the base chunks whose ancestor *n* levels
  up is its own coordinate.
- **A product over the edit is left where it is** — retained at the revision it
  was built under, so `select_ready` still falls back to it as a complete
  picture — and marked dirty at the new revision up to `SWEEP_LOD`, which is
  `invalidate_tile`'s column for a chunk instead of a tile.
- **Nothing above the ceiling is named at all**, because it does not need to be:
  a parent wants four children at the new revision, three of the four are
  carried, and publishing the one rebuilt child completes the family for
  `build_ready_ancestors` to climb. That is asserted by pixels — the parent's
  north-west quadrant is the carried child's own colour and its south-east is
  what the rebuild put there.
- **The sweep is owed what it was owed.** `drain_sweep` strikes off any key
  older than the facet's revision, and a floor is owed once a session, so before
  the carry a publish part-way through a facet's first sweep quietly finished
  it — a hole that shows up weeks later as backdrop at some zoom, on ground no
  window ever drew at level zero.

`LruBudget::rekey` is the one thing underneath: a carried product keeps its
weight *and* its place in the use clock, because a remove-and-insert would make
a whole facet the most recently used thing in the cache at once, which is the
opposite of what the next eviction pass is trying to read.

**Not built: the caller.** The client's publish path still calls `set_revision`,
in `net_command.rs`, which is the file the editor work is rewriting. The switch
is one line and lands with that tree.

## What was decided, and against what

- **A carry, not a second key.** Settled in the plan before this session and not
  reopened; the code follows it. What it buys, restated from the cache's end: a
  world loaded from a UO install never hashes its facet at startup, and a coarse
  product — which has no content of its own — is compared rather than combined.
- **`set_revision` stays, unchanged and fail-closed.** A caller that moves a
  revision without saying what moved still loses every product. That is what
  makes the carry a *claim* by a caller who knows, rather than a default nobody
  audited. Deleting it and making `touched` mandatory would have made the
  ignorant caller pass an empty list, which means the opposite.
- **A touched product is left stale rather than dropped.** The plan's word was
  "dropped"; dropping the pixels would have opened a hole where today's
  facet-wide bump shows stale terrain, so what it means here is *not carried*.
  The cost of the other reading is a person watching ground turn to backdrop
  under their own edit.
- **The conversion from a map chunk to a base radar chunk stays at the caller.**
  Both are 64 tiles and `openshard_map::chunk`'s header calls that a decision
  rather than a coincidence, so it is a coordinate change — but `moved` is in
  the render crate and the caller is the one place that holds both types.
- **`invalidate_tile` is test-only for good**, which this node was always the one
  to make final. A chunk is sixteen thousand tiles and a publish names chunks;
  what keeps the function is that a tile is the unit the ladder's projection is
  easiest to assert on.

## What is next

- **The one line at the caller**, once the editor tree settles: `set_revision` →
  `moved` in `net_command.rs`, with the chunk coordinate change beside it. That
  is the whole of S2's "done when".
- **S3 — a block is replaced where it stands**, which is independent of both and
  is the one a person waits for. The plan says it may go first, and should if the
  editor lands before it: a brush is a stream of publishes, and 115 ms on the
  tick and another 115 on the window is what a person feels as the tool being
  unusable.

The two bakes that carry a `MapRevision` — the navigation bake and the building
flood — are facet-wide artefacts with no per-chunk products, so they refuse
themselves on a publish and remain S3's problem, unchanged by this.
