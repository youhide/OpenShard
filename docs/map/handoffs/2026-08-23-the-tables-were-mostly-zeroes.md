# 2026-08-23 — the count tables were mostly zeroes

[`navigation_spans.md`](../navigation_spans.md)'s first filed finding, taken two
sessions after it was filed the second time: **the span bake spent half of
itself addressing columns that own nothing.** No node of the plan is involved —
this is an *Out of scope, named* entry with no defect under it, and it is the
one that section named as the next thing to try.

## Where it stands

**Built.** `SpanIndex`'s per-block addressing is an occupancy mask, the counts
are packed, and nothing else about the layer moved.

### What the tables were

A block that holds any static at all carried a `BlockTable` of a `u32` base and
a `[u8; 64]` — one count per cell, whether or not that cell had anything to
count. N1 chose it deliberately: 64 bytes is a cache line, and a byte-wise
prefix sum over one line is cheaper than a 256-byte table of offsets. N1 also
filed what it costs and estimated the waste at 71% from the census.

**The bake counts it exactly, and it is worse than the estimate: 1,388,743 of
the 7,727,616 cells those 120,744 tables address own a run.** 82% of every table
was a zero, and the tables were 8.2 MB against the 6.5 MB of spans they address
— the addressing was bigger than the thing addressed.

### What it is now

| | |
|---|---|
| `occupied: u64` | one bit per cell, beside the base — sixteen bytes a block instead of sixty-eight |
| `SpanIndex::counts` | one byte per *set* bit, one facet-wide run, blocks in table order and columns in ascending cell |
| the lookup | `count_ones` on the mask below the cell gives the rank; the prefix sum runs over the occupied columns before this one, not over all sixty-four |

**A column with nothing stored returns on the bit test** — the mask is in the
same sixteen bytes the base is, so it costs one word and one comparison, and it
is 82% of the columns of a block with statics in it. That is the half of the
repair the byte count does not show.

**A column whose statics leave nothing to stand on gets no bit and no count** —
a wall on a mountainside. Not for correctness: a stored zero sums to the same
offset and yields the same empty slice. It is the population the counts are
sized by, which is the whole point.

## What it is measured against

**`step_cost` and `span_index` on facet 0**, release, two runs of each build
agreeing to the tenth. Both builds were taken on *today's* working tree rather
than against yesterday's numbers, because a parallel session has `walk.rs` and
`scene.rs` open and the landing path runs through them — the only difference
between the two measurements is `spans.rs`.

| | dense tables | mask |
|---|---|---|
| the addressing | 8.21 MB | **3.32 MB** |
| the whole bake | 16,603,552 B (15.8 MiB) | **11,713,607 B (11.2 MiB)** |
| a landing off the bake | 179.4 / 180.6 ns | **158.3 / 158.6 ns** |
| `steps_out_of`, a whole node | 218.0 / 219.1 ns | **200.3 / 200.7 ns** |

**Smaller *and* fewer bytes read**, which is exactly what the finding predicted
it would be. Per column of the facet the bake is 0.40 bytes where it was 0.57.

**The answers did not move.** 1,635,392 spans before and after, and
`span_index`'s whole-facet oracle — `Spans::surfaces` against `stand_surfaces`
on every column for both abilities — reports **0 disagreements over 29,360,128
columns**, the same as the dense build. The bake still takes 0.07 s.

**No rebake is owed.** Spans are not serialised — N6 is the node that would
change that and it is still gated — so `ROUTING_VERSION` is untouched and the
navigation artifact is unaffected.

## The done-whens, and their controls

**`the_rank_is_over_occupied_columns_and_not_over_cells`** (`spans.rs`). Three
columns of one block, the last of them in the block's last cell, with two
surfaces on it so a misread is visible as a height rather than as a length.

*The control* is `rank` taken over cells instead of over set bits — one line.
The test fails with `range end index 28 out of range for slice of length 3`,
which is cell 63 reading sixty-one bytes past a three-byte run, and nothing
earlier fails.

**`a_column_whose_statics_leave_nothing_to_stand_on_owns_no_run`** (`spans.rs`).
A wall and a floor over impassable land: the block has a table, the wall's
column has no bit, and the floor's column still reads its own run. It asserts
the *size* — `column_count()` is 1 of the two cells with statics — because the
addressing would survive a stored zero and the size would not.

**The whole-facet oracle is the real done-when**, and it is the one that was
already there: `span_index` is N1's own equivalence proof and it is unchanged.

## What is clean

`rustfmt` on both touched files, `cargo clippy -p openshard-movement
--all-targets` silent, and the crate's suite green: 144 + 1 + 7 + 5 doctests.
`cargo check -p openshard-state -p openshard-world --all-targets` builds.

`openshard-uofiles`'s one `needless_borrow` is still there and is still not
ours — the previous handoff filed it, and the file is open in another session.

## What is next

**Nothing here follows from this.** The finding is closed and what it pointed
at next — the packed static record — is under its own gate, which is
[`new_map_representation`](../new_map_representation/)'s direction B rather than
this plan's.

**What the finding leaves as the new frontier**, if a node expansion ever has to
get cheaper again: the landing half is now ~158 ns of a ~200 ns expansion, and
A\*'s own machinery is ~220 ns a node. The two are the same size, so the next
useful measurement is the one the plan already names — *baked adjacency*, an
8-bit neighbour mask per span, which attacks the half that is no longer the
larger one.

**What would block it:** nothing, but nothing asks for it either.
