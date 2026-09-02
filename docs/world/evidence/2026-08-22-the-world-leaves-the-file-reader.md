# 2026-08-22 — the world leaves the file reader

Third session of the day. It started as a question — is `MapSnapshot` the single
type everything reads? — and the answer surfaced a thing the track had said in
its first line and never built: **the map is our data, and UO's files are one
importer.** `WorldMap` was the single representation of the world, and it was
declared inside `openshard-uofiles`, whose stated job is reading the client's
files.

Landed as [`snapshot.md`](2026-08-25-one-world-one-door.md)'s **phase 3**, in two commits.

## Where it stands

`openshard-map` is the world. It depends on `openshard-protocol` and nothing
else, no function in it takes a path, and it has three modules: `grid` (the
block order, moved whole and unchanged), `map` (`WorldMap`, `LandCell`,
`LandTile`, `StaticItem`), and `snapshot` (`MapSnapshot`, `MapRevision` — what
used to be its `lib.rs`).

`uofiles::map` is the importer: `.mul`, `.uop`, `staidx`, `MapError`, the byte
constants, and the facet-shape table. Free functions, not methods on a type it
no longer owns. `uofiles` now depends on `openshard-map` — the reverse of
before — because `surfaces::stand_surfaces` and `radarcol` *read* the world.

Both loading doors kept their meaning and changed their names:

| Was | Is | Who calls it |
|---|---|---|
| `MapSnapshot::load_facet` | `uofiles::map::load_facet` | production: `boot.rs`, the client's `lib.rs`, both bakes |
| `WorldMap::load_facet` | `uofiles::map::read_facet` | tests, examples, diagnostics — as before |

`MapSnapshot::new` is now the only constructor in `openshard-map`, so "a facet
was loaded" and "a facet has an identity and a revision" cannot come apart.

`cargo check --workspace --all-targets`, `cargo test --workspace` and
`cargo fmt --all` are silent. Clippy's ten warnings are the interiors track's,
in files this work only edited an import line of.

## What was decided

**The `.mul` reader stays in `uofiles`; only the world moved.** The alternative
was moving `map.rs` whole — simpler, one commit, and `openshard-map` would then
have been "the world type, plus a UO reader". That is the shape the track is
trying to leave. Cost of the split, paid: `WorldMap` needed a public
construction path, and `MapSnapshot::load_facet` had to move to the importer, so
a file-reading crate is what publishes revision 1. That is honest — an importer
is exactly the thing that mints a first revision.

**The line is grid geometry against file bytes.** `BLOCK_SIZE` and
`CELLS_PER_BLOCK` describe the grid and went with it; `BLOCK_BYTES`,
`BLOCK_HEADER`, `CELL_BYTES`, `STAIDX_ENTRY`, `STATIC_BYTES` describe a file and
stayed. Deciding per-constant rather than per-name is what keeps a byte count
out of a crate that will hold a world nobody serialised that way.

**`WorldMap::from_parts` owns the sort, and the decoder lost it.** The per-block
sort by tile was the loader's, and it is the invariant `statics_at` and
`statics_in_row` binary-search over — so a decoder that skipped it would not
fail, it would make every later lookup quietly find nothing. Moving it into the
type is what makes direction B's chunk reader safe *by construction*: it cannot
get the order wrong differently from the `.mul` reader, because it does not do
it. `from_parts` also panics on a statics array whose length disagrees with the
block count, which the field's own doc comment says nothing enforced before.

**The tree was left broken for one commit, deliberately.** The move landed
first (`facadfdd`, does not build), the ~50 files naming the old paths second
(`cb0d8358`). Asked for, and worth repeating for a move of this shape: the first
commit is the decision and reads as one, the second is mechanical and reads as
one.

**Direction B is a second importer, not a second world.** Written into
[`plan.md`](2026-08-25-seven-directions.md)'s B: a chunk reader builds the same `WorldMap` through
the same `from_parts`, which turns B's round-trip acceptance test into an
assertion about *bytes* rather than about two parallel representations that
agree by inspection.

## What is next

Direction B, unchanged. Two things it now inherits that it did not have before:
the crate to put the chunk types in already exists and is already the world, and
`from_parts` is the door its decoder comes through.

The two smaller items the previous handoff left open are still open: the typed
extent for `WorldMap::from_blocks` (deferred to B on the plan's own words) and
the `Command::Send` newtype once `link.rs` is quiet.

## Found along the way

**A parallel session's uncommitted rename rode along.** `grid.rs` had an
uncommitted `Cells` → `TerrainCells` in the working tree at session start; it
moved with the file and is in `facadfdd`. One stale doc reference to the old
name inside it was fixed. Nothing else of that session's was touched.

**`WorldMap` is the world under the entities, not the whole world a player
sees.** Houses, items, doors and boats are laid over it as entities; the
client's `steer.rs` reads `cluttered` — the map with the shard's live items over
it — while `predict_step` read the bare `WorldMap`. Phase 2 put both halves on
one side of the thread boundary; making them one rule is still nobody's task,
and it is worth naming before direction C starts publishing.
