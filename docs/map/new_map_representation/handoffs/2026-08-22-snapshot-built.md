# 2026-08-22 — the snapshot is built, both phases of it

## Where it stands

[`snapshot.md`](../snapshot.md) is done. Both of its phases are merged, and the
plan has no unbuilt section left in it — only its two "what phase N left
behind" backlogs and its out-of-scope list.

**A0 — the block order got a type.** `251fdc7f`. It is
[`crates/common/uofiles/src/grid.rs`](../../../../crates/common/uofiles/src/grid.rs):
`LandGrid` with `BlockCoord`, `BlockIndex` and `CellIndex` beside it. `map.rs`
no longer spells the order anywhere — not the four `block_x * blocks_down`, not
the two verbatim copies in `cell_index` and `block_index`, and not the inverse
`load_statics` had backwards. `Map`'s public API is unchanged, which is what
made it land without touching a reader.

**A — the map got one owner with a revision on it.** `f4e563ea` and `6944e9d2`,
with `71d4589d` writing down why the second was necessary.
[`crates/common/map`](../../../../crates/common/map/src/lib.rs) holds
`MapSnapshot` and `MapRevision`; `Resources` on the client and `FacetState` on
the server each own one; `Map::load_facet` has one production caller and it is
inside `openshard-map`. Both *map-derived* bakes — navigation and the building
flood — record the revision they were built from and refuse a snapshot that
disagrees, alongside their existing mtime staleness check rather than instead of
it. `link::connect` is transport: it takes bytes already framed for the wire and
returns decoded mutations for the event-loop owner to apply, and the step
prediction moved back beside the snapshot.

Gates, as run at the end of this session on a tree that also carries another
session's uncommitted `client/app` edits: `cargo check --workspace
--all-targets` is silent, `cargo test --workspace` passes, and
`cargo clippy --workspace --all-targets` carries the same eleven warnings
[`snapshot.md`](../snapshot.md) named — eight in `client/render`'s interiors
work plus its `interior_census` example, two in `client/app` — none of them from
this track, and clearing them belongs to whoever finishes interiors.

## What was decided

Only the decisions a later session could reopen by accident; the rest are in
[`snapshot.md`](../snapshot.md) with their reasoning.

- **The prediction sits with the snapshot's owner, not on the socket thread** —
  against three alternatives that all exist only because it was across a thread
  boundary: a second channel republishing the handle on every publish, an
  `ArcSwap`/`RwLock` the socket thread can block on, or a height grid rebuilt
  per revision. An `Arc<Map>` handed over at login stays memory-safe forever and
  silently keeps the revision it was handed, which under
  [direction C](../plan.md#c--patches-and-the-resolved-snapshot) is worse than a
  race: it never crashes and never warns.
- **A stamp asks for the revision in hand, never supplies `INITIAL` itself.** A
  stamp that filled in its own default would compare a constant with a constant.
- **The revision guard is beside the mtime key, not instead of it.** Replacing
  the key is [direction D](../plan.md#d--derived-data-keyed-by-revision), and a
  session that does it here has taken on D.
- **The art table gets no `MapRevision`.** It never reads a map, so the field
  would lie and a guard reading it could not fire for the right reason. Whoever
  adds a third *map-derived* artifact inherits the bullet; the art table does
  not.
- **`MapSnapshot` has no `Deref`.** It was tried, and removed: it defeats the
  phase's own point, because a seam the compiler crosses for the caller is a
  seam that is not there. `AsRef<Map>` stays for one reason — `MapTerrain<M>` is
  generic over it.
- **`BlockCoord` is one type and `RadarChunkCoord` is not it.** A radar chunk is
  64 tiles square; collapsing the two is the confusion
  [`pixels.md`](../../../pixels.md) exists to prevent.
- **`MapRevision` starts at 1 and nothing moves it yet.** The guard is tested by
  handing `load` a stamp one revision ahead; nothing in a running shard can
  produce that disagreement until C publishes.

## What is next

By [`plan.md`'s order](../plan.md#order), direction B — our own chunk format and
a UO importer. Nothing blocks it. Three things it should know before it picks a
layout:

- **A block column is an eight-wide row-major strip.** The two orders compose:
  `block_y * 64 + y_local * 8 + x_local == y * 8 + x_local`. `grid.rs`'s header
  derives it and `the_two_orders_compose_into_a_strip` holds it. It is the
  property a chunk layout would lose.
- **The land and the statics are still two parallel `Vec`s sharing one
  `BlockIndex`.** Every subscript goes through `LandGrid::index_of`, which is as
  close to enforcement as parallel vectors get. A type that cannot express the
  mismatch is B's to build.
- **The `facet: u8` gate breaks by existing.**
  `crates/common/protocol/tests/facet_bare_fields.rs` is an allowlist keyed by
  exact per-file counts, so any new binary that reads `--facet` from argv turns
  `cargo test --workspace` red until it is listed. B adds an importer. It had
  already been red since 2026-08-20 for exactly this reason, on a file this
  track never touched.

The first of the smaller things is done in the same session: `interiors::BlockId`
and `composite::MapBlock` are one `BlockCoord`, `RadarChunkCoord` stayed
separate, and `MapBlockBounds` widened to `u32` behind it.

What is left of that list, each landable alone and each already decided
in [`snapshot.md`](../snapshot.md)'s left-behind lists: give `LandGrid`'s transitions their first caller, which is what turns
the walk order into a property of one iterator and is the half of A0's point
[`depth::Order`](../../../../crates/client/render/src/depth.rs)'s anti-diagonal
tie is waiting on; give `Map::from_blocks` a typed extent; and give
`Command::Send` a newtype saying its bytes are already framed for the wire.

## Found while collapsing the two block types

The grids in this workspace share arithmetic and share no code. Four of them
now: the 8×8 map block (`BLOCK_SIZE`, and `BlockCoord` owns its conversions),
the 64-tile radar chunk (`BASE_CHUNK_TILES`, with `RadarChunkCoord` beside it),
the 64-tile server sector (`SECTOR_SIZE`), and the block *bounds* rectangle in
`client/render`.

- **A sector has no coordinate type at all.**
  [`Sectors::sector_of`](../../../../crates/server/state/src/sectors.rs) answers
  in a bare `usize`, and its buckets are indexed `sector_x * down + sector_y` by
  hand — the same column-major linear index `LandGrid::index_of` owns, written
  out again. Its own comment says it copied the map's order deliberately so that
  two orders would not sit in one crate; the copy is the part worth removing.
- **Sector and radar chunk are both 64 tiles for unrelated reasons** — the radar
  from `BLOCK_TILES * 8`, the sector from Sphere's `SECTORSIZE_DEFAULT`, pinned
  by `VIEW_RANGE`. Two grids that agree by coincidence are exactly what
  [`pixels.md`](../../../pixels.md) is about, and a shared *number* would be the
  wrong way to unify them.
- **`radar.rs` still open-codes tile → block** (`origin_x / BLOCK_TILES`) where
  `BlockCoord::containing` is the same arithmetic.

The shape that would settle it is shared arithmetic behind *distinct* types — a
grid parameterised by its tile side, from which `BlockCoord`, `RadarChunkCoord`
and a future `SectorCoord` come out non-interchangeable. One coordinate type for
all three is the thing this track has already refused twice, and for the same
reason both times.
