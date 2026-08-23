# 2026-08-23 — R4: statics become one run

[`realtime_map.md`](../realtime_map.md#r4--statics-become-one-run)'s fourth node,
built in one commit: `dd253978` statics become one run. It changes no behaviour
at all — the base-set round trip compares all 29,360,128 tiles byte-identical
after it, which is the whole of the acceptance.

## Where it stands

A facet's statics were a vector per block: 458,752 of them for Felucca, 120,744
with anything in, and the empty three quarters still cost their headers. They are
**one run** now, with a `Vec<u32>` of per-block offsets beside it — block `i`
owns `statics[offsets[i]..offsets[i + 1]]`, which is the CSR layout
[`Chunk`](../../../crates/common/map/src/chunk.rs) has held since it was cut.

| | before | after |
|---|---|---|
| allocations | 120,745 | **2** |
| the statics | 29,068,710 B | 29,068,710 B |
| what describes them | 11,010,048 B of `Vec` headers | 1,835,012 B of offsets |
| **resident** | **40,078,758 B (38.2 MiB)** | **30,903,722 B (29.5 MiB)** |

The accessors are untouched, and that is the point: a block is still contiguous
and still sorted by `(y, x)`, so `statics_at`'s two binary searches,
`statics_in_row`'s contiguous row and `statics_in_block`'s whole slice all still
hand back a `&[StaticItem]`. Nothing outside `openshard-map` names the layout —
the diff is three files, and two of them are the importers.

- **`from_parts` takes the run and a count per block**, which is
  `Chunk::from_parts`' shape: the prefix sum is the type's, so a second decoder
  cannot accumulate it differently from the first, and the counts summed against
  the run's length is what catches a decoder disagreeing with itself. The sort
  stays **per block** — a global sort by `(y, x)` would interleave two blocks
  that share a row.
- **The `.mul` reader pushes straight into the run** as it walks `staidx`, whose
  entry *n* is block *n*. The count per block is measured after the pushes
  rather than predicted from the byte length, so a truncated final entry is not
  counted as one. The intermediate vector-per-block is gone from the loader too.
- **`chunk::assemble` keeps a slice per block**, borrowed from the chunk that
  carried it, and lays the run out in one pass. Chunks arrive in their own order
  and the facet wants the land's; that indirection is what the difference is
  for, and it replaced a `to_vec()` per block.

**`cargo check --workspace --all-targets`, `cargo clippy --workspace
--all-targets` and `cargo fmt --all` are silent.** `cargo test --workspace
--no-fail-fast` ends with the same two red tests in `openshard-state` and no
others — R1's finding, still filed under [*`can_step` does not check the
corner*](../../roadmap.md).

## What the node decided

One thing, and it is against the plan's own text. The plan said `place_static`
as an in-place tail shift **goes**, replaced by a builder; what went is the
*per-block* shift, and the pair stayed with a different cost. The reasoning is
in the node under *What the node decided*, and in short:

- `Vec::insert` on the run **is** "rebuild the block this touches", spelled as
  one move of the tail and no second buffer — and `patch::apply_op` is the only
  engine caller of either.
- The builder's other callers would have been *scenes*, which are one block, and
  `Scene` mutates and reads interleaved by design: it would have paid a
  `build()` seam or a dirty flag for no property.
- The trap the plan was closing — an importer assembling a facet an item at a
  time, which is now quadratic — is closed by `from_parts` instead. Both
  importers hand over a run and a count per block and neither can reach
  `place_static` to do otherwise.

The two other decisions were the counts-not-offsets signature and the per-block
sort, both above.

## What was tested

Two properties the one-run layout newly makes breakable, and both are new tests
in [`map.rs`](../../../crates/common/map/src/map.rs):

- **`an_edit_in_one_block_leaves_every_other_block_where_it_was`.** An edit in
  block 0 with an assertion over the whole facet after it. Miss an offset and
  the neighbour reads a slice one item out of place, which is not a panic — it
  is a wall that is silently the item next to it. The edit is in the *earliest*
  block on purpose: one in the last block passes with the offsets left alone.
- **`from_parts_sorts_each_blocks_own_part_of_the_run`.** Two blocks of one row,
  handed over in file order, which is where a sort that forgot the block
  boundary shows.

A third pins the arithmetic the whole node is about:
**`a_static_is_ten_bytes_in_the_run`** — nine bytes of fields in ten of storage,
so the base layer is that number times a count, and a field added to
`StaticItem` is 2.9 MiB of resident memory per byte.

## What was found

Two things, filed in [`roadmap.md`](../../roadmap.md) under *Backlog from R4*.
Neither blocks R5 or era P.

- **A patch of many ops is now quadratic in the facet.** Each op moves the tail
  of the run; nothing today publishes more than a handful, and the editor
  (direction F) is what will.
- **`from_parts`' grouping is a contract with no oracle.** The counts' sum and
  length are asserted; *which* items a caller put in which block is not, and
  getting it wrong sorts them into the wrong span and is silent everywhere
  after.

## What is next

**R5 — one install, one load**, the last node of era R and the smallest, or
**era P**, which R4 was the last thing gating.

- [**R5**](../realtime_map.md#r5--one-install-one-load) changes what a process
  *holds* rather than what a type is: `openshard-playground` holds two ~150 MiB
  copies of one facet and should hold one.
- [**Era P**](../map_rebuild.md) may start. `Spans` waited on the shape of the
  two layers it is a projection of — what a house contributes to a surface (R3)
  and how the statics are held (R4) — and both have landed. R5 changes neither.

**What would block them:** nothing.
