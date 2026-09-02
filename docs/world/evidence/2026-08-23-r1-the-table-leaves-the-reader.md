# 2026-08-23 — R1: a table is not a file

[`realtime_map.md`](2026-08-23-era-r-the-map-you-hold.md#r1--the-table-leaves-the-file-reader)'s
first node, built in the four commits it named plus two the plan did not
foresee. No behaviour changed anywhere; every failure the move produced was a
compile error, exactly as the node's risk line said it would be.

Six commits: `3bbb5762` the crate and the move, `5ff64176` an unrelated red gate,
`47c2e94b` the call sites, `56fd0a1f` the id becomes `LandTileId`, `1d67e7a5`
the re-export that followed it by mistake, `18512225` + `616c4733` `surfaces` to
movement.

## Where it stands

`crates/common/tiles`, package **`openshard-tiles`**, **with no dependencies at
all** — which is the property the struck crate rule was a proxy for. It holds
what a tile *is*: `TileData` and its lookups, `LandTile` and `StaticTile`,
`TileFlags`, `LAND_TILE_COUNT` / `STATIC_TILE_COUNT`, `pluralize_name`, and the
ids — `LandTileId`, `AnimId`, `TextureId`.

`openshard_uofiles::tiledata` is the reader and nothing else: the two layouts and
the arithmetic that tells them apart, the group headers, the entry sizes, the
errors, the parse. `openshard-uofiles` exports readers, formats and errors, which
is R1's done-when. The crates that still depend on it depend on it for a
*reader* — multis, art, the map importer.

`stand_surfaces` is `openshard_movement::surfaces` now, a crate-local call from
`MapTerrain::surfaces`.

**`cargo check --workspace --all-targets` and `cargo fmt --all` are silent**,
and clippy is silent on everything this session changed. The other two are not,
and neither was before it started: `cargo test --workspace --no-fail-fast` ends
with two red tests in `openshard-state`, and clippy warns at six sites in
`client/app` and `client/render`. Both are in *What was found*, with what they
are and whose they are.

## What the move decided

Two questions the plan did not name, both settled by the rule it set:

- **`TextureId` moved too.** `LandTile::texture` holds one, so leaving it beside
  the reader of `texmaps.mul` would have made the table depend on a file reader
  — the thing R1 exists to end. `AnimId` was already in `tiledata.rs` for
  exactly this reason and is the precedent: **the table declares the ids its
  entries name**, and the readers of those two files take them as arguments.
- **The layout left `TileData`.** `TileDataFormat` stays in `uofiles` by the
  plan's own table, so the table cannot hold one — and should not: a layout is a
  fact about a *file* and a table built by hand has none. `tiledata::load` and
  `tiledata::parse` are free functions handing back a `Reading { tiles, format }`,
  and the single caller that wanted the format (the boot log) reads it off that.
  `TileData::from_tables` is the one way to build a populated table and asserts
  both lengths, because every lookup on it is total.

## What was found

Three things, none of them caused by the move, all of them found by running the
whole suite:

- **🚩 The suite was red before this session, and the first failure hid two
  more.** `facet_bare_fields` — the gate that keeps a bare `facet: u8` on an
  allowlist — was failing on movement's `span_census` example, which takes a
  `--facet` on its command line exactly as `coarse_bench` next door does and was
  added without the entry. `cargo test` stops at the first failing binary, so
  `openshard-state` was never reached. Fixed in `5ff64176`.
- **🚩 `can_step` does not check the corner.** Behind that gate:
  `a_diagonal_is_refused_when_either_flank_is_blocked` and
  `a_live_terrain_with_no_map_reports_no_water`, both in
  `state/src/obstruct.rs`, both red since node E (`3aef249e`). The first is the
  one that matters — `corner_open` is consulted on the walk path and not from
  `can_step`, so the rule that stops a body slipping past a blocked tile's
  corner applies to a player's `0x02` and not to a caller asking `can_step`
  directly, which is what a server-driven creature does. Filed in
  [the runtime-and-tick record](2026-08-24-runtime-lookups-and-the-tick.md)
  under *`can_step` does not check the corner*,
  with the second (an `unwrap()` on a deliberate `None`).
- **Clippy is not silent at `HEAD`** and was not before this session: six sites
  across `client/app` and `client/render` — `presentation.rs:1932`,
  `world.rs:247`, `render/interiors.rs` (four), `items.rs:609`,
  `statics.rs:1025`, `interior_census.rs:98`. None is in a file this session
  changed for any reason other than an import line; left for the track that owns
  them.

## What is next

**R2 — the third layer joins the type**,
[`realtime_map.md`](2026-08-23-era-r-the-map-you-hold.md#r2--the-third-layer-joins-the-type). It
is the node this whole document set is named for: `Overlay`, `Cover`,
`CoverKind`, `Doors` and `Tile` move to `openshard-map`, `World { base, live }`
is built, and `Footing::of(&World, &TileData, Doors)` becomes the one
composition. R1 was its precondition — `Cover::of_static` can take its
`StaticTile` from `openshard-tiles` now without dragging a file reader anywhere.

**What would block it:** nothing. Its risk is the client's lifetime, as the plan
says, and not the move itself.

**What not to start:** era P still waits on R2, unchanged.
