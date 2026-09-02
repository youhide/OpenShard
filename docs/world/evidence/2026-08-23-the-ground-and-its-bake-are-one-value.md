# 2026-08-23 — The ground and its bake are one value

Not a node of [`navigation_spans.md`](../design_spans.md) — a repair of what
three of its nodes filed. The span bake is a projection of a facet's base, so a
base that moves without it is a shard deciding steps by the heights of a map it
no longer holds. Until now the two travelled **by agreement**: both ends held
them in adjacent fields, both fields carried a comment saying they had to agree,
and `Footing::of` *checked* the agreement at the question with a panic for the
facet that had a map and no bake over it. They are one value now, and the panic
is gone rather than moved.

One commit: `1627508f`.

## Where it stands

[`openshard_movement::ground::Ground`](../../../crates/common/movement/src/ground.rs)
is a `World` and the `SpanIndex` over its base. Both fields private; the three
functions that write either — `new`, `set_base`, `rebake` — write **both in the
same statement**, so *a facet with a map and no span bake over it* is a state
nothing can spell.

- `Footing::of(ground, tiles, doors)` — three arguments where it took four, and
  its `# Panics` section is deleted rather than reworded.
- `FacetState` loses `world` and `spans` for one `ground`, and hands out
  `ground()` where it handed out `world()` and `span_index()`. `set_map` and
  `rebake` forward; nothing else on the facet can reach either half.
- `WorldState::map_terrain` is `ground().terrain(&self.tiles)` — one call, where
  it was a snapshot lookup and a bake lookup that could in principle disagree.
- The client's `Resources` loses `spans` the same way, and builds its `Ground` at
  the point it used to bake the index — the snapshot goes *in*, and the loaders
  below it read a borrow back out.

**`cargo check`, `cargo clippy --all-targets` and `cargo fmt --all` are silent on
every crate this touches** (`openshard-movement`, `-state`, `-housing`, `-world`,
`-boats`, `-server`, `-client-app`). `cargo test` on the same set is green except
the two long-standing red tests in `openshard-state` —
`obstruct::tests::a_diagonal_is_refused_when_either_flank_is_blocked` and
`a_live_terrain_with_no_map_reports_no_water`, R1's finding, still filed under
[*`can_step` does not check the corner*](2026-08-24-runtime-lookups-and-the-tick.md).

⚠ `cargo check --workspace --all-targets` is **not** silent, and none of it is
this: `openshard-client-render`'s tests and examples do not compile against a
parallel session's in-flight `StaticArt` change (`occlusion::shape_of` and
`occlusion::collect` now take a `StaticArt<'_>` where the callers pass
`&StaticAtlas`). Its library compiles, which is why everything downstream of it
does.

## What the repair decided

**It wraps the world rather than moving the bake down into it.** The plan said
twice — in `FacetState::spans`' doc and in `Footing::of`'s — that a `SpanIndex`
would live inside [`World`](../../../crates/common/map/src/world.rs) if it could,
and that it cannot because `openshard_map` is underneath `openshard_movement` and
where a body may stand is a movement rule. That argument still holds and is the
reason the fix is a wrapper: the bake reads `MAX_STEP_UP` and `PLAYER_HEIGHT`, so
pushing it down would make the crate that holds the world decide how tall a
person is — which is exactly the move
[`realtime_map.md`](2026-08-23-era-r-the-map-you-hold.md)'s R2 refused when `Cover::meets` asked
for it, and answered with a `Body` argument instead. A wrapper honours the
layering without paying that.

**Nothing hands the inner world back out.** `Ground` forwards `snapshot()`,
`live()` and `live_mut()` and offers `terrain(tiles)`; there is no `world()`. A
reader that could take the `World` alone is a reader that could forget the bake
again, which is the whole of what this ends — and after the sweep nobody wanted
one: the three holders of a `World` in the workspace were the shard's facet, the
client's resources, and `Footing::of`'s parameter.

**The tile table stays outside, and that is now the only asymmetry.** One install
has one table and several facets, so what a graphic *is* is not a fact about a
world — `World`'s own doc draws that line and this keeps it. The consequence is
that the bake is a statement about the world *and* the table, so the table is an
argument to all three writers, and a table arriving after the ground still needs
`Ground::rebake` (`World::with_tiles`, the one caller).

**The one `expect` is inside, not at the readers.** `Ground::terrain` returns
`None` for a facet with no map and expects the bake for a facet that has one.
That expect is unreachable by construction from three functions in one file,
which is the trade this type is: one assertion nobody can reach, against a panic
in `Footing::of` that every caller could.

## What was found

Filed in [`navigation_spans.md`](../design_spans.md)'s *Out of scope, named*,
beside the two findings this repair half-closes.

- **🚩 `WorldState::tiles` is now the only way left to hold a stale bake.** The
  ground can no longer move out from under its bake, so what remains is the third
  input: a direct `state.tiles = table` leaves every facet holding a bake over the
  old table. Six sites write it, all fixtures, all harmless today (they assign the
  table they just baked from); the repair is a setter that rebakes every loaded
  facet — `World::with_tiles`' own body, which already does it. What makes it a
  sweep rather than a one-liner is the **sixty-seven** sites that *read* the
  field.
- **A second `Ground` already exists in `client/app`, and it is the misnamed
  one.** [`steer::Ground`](../../../crates/client/app/src/steer.rs) is a pair of
  `Footing`s — the same map read twice, once with the doors shut and once open —
  which is a *reading*, not ground. No file imports both today and nothing
  collides at the compiler, but one crate now spells two different ideas with one
  word. If either moves, that one should: `Readings` says what it is.
- **The interiors bake's finding now has its value.** N3 filed that
  `PlanarTopology::bake` and `Buildings::bake` each build a facet-wide
  `SpanIndex` of their own, 0.07 s each inside a bake that already walks the
  facet, and that *"the honest fix is for the interiors bake to take the ground
  it is baking over as one value"*. That value exists now and its
  `terrain(tiles)` is exactly what those two build for themselves. What is left
  is the signature sweep — five bakes in `interiors.rs`, plus `artscan` and the
  examples that call them — and it was left because it is a client-render change
  and that crate is where the parallel session's in-flight work is.

## What is next

Nothing here blocks anything, and this blocks nothing.
[`navigation_spans.md`](../design_spans.md)'s own next is **N7 — the server
reads the graph**, which is where a player meets N4.

If someone picks up one of the three findings above, the `WorldState::tiles`
setter is the one with a defect behind it; the other two are tidying.
