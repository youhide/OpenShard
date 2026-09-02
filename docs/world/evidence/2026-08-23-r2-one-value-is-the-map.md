# 2026-08-23 — R2: one value is the map

[`realtime_map.md`](2026-08-23-era-r-the-map-you-hold.md#r2--the-third-layer-joins-the-type)'s
second node, built in the four commits it named plus one it did not: the call
sites, which are their own commit for the reason R1's were. No behaviour changed
anywhere.

Five commits: `6702e72c` the move, `49fa83c8` the call sites, `fbe6588f` `World`
and `Footing::of`, `9fd3f8af` the shard, `b3f5ed97` the client.

## Where it stands

`Overlay`, `Cover`, `CoverKind`, `Doors` and `Tile` are
[`openshard-map`](../../../crates/common/map/src/overlay.rs)'s. `Tile` went into
`grid`, beside `BlockCoord`, whose doc already defined itself as *"a block's
position on the facet — not a tile"*. Every rule that reads one stayed in
`openshard-movement`.

[`World { base: Option<MapSnapshot>, live: Overlay }`](../../../crates/common/map/src/world.rs)
exists, and `Footing::of(&World, &TileData, Doors)` is the one composition over
it. Both ends hold one:

- the shard's `FacetState` where it held a public `map` and a private `overlay`,
  with `refresh` writing through `world.live_mut()` and the four mutators
  untouched;
- the client's `Resources` where it held `map`, with the overlay leaving
  `PresentationWorld` to join it and `clutter::of` becoming `clutter::fill`.

`World::snapshot` is the bake-facing accessor and there is deliberately no
accessor handing out both layers at once, so *a bake cannot see the live world*
is a borrow rather than a rule.

**`cargo check --workspace --all-targets`, `cargo clippy --workspace
--all-targets` and `cargo fmt --all` are silent** — clippy included, which it was
not at the end of R1: the six sites that handoff listed in `client/app` and
`client/render` were cleared by the parallel session that owns those files.
`cargo test --workspace --no-fail-fast` ends with the same two red tests in
`openshard-state` and no others; they are R1's finding, still filed under
[*`can_step` does not check the corner*](2026-08-24-runtime-lookups-and-the-tick.md).

## What the move decided

Three questions the plan did not name. All three are in the plan's own node too,
under *What the move decided*; the reasoning is here.

- **A body's height became an argument.** `Cover::meets` read
  `openshard_movement::PLAYER_HEIGHT`, which the map's crate cannot see. The
  alternatives were to move that constant into `openshard-map` — the crate that
  holds the world deciding how big a person is — or to leave `blocker_at` behind
  in movement, which the plan had already ruled out by naming it a lookup that
  goes with the structure. So the caller supplies it, as a `Body { z, height }`
  and not a second `i32`: a position and a length in the same units side by side
  say nothing about which is which, and this is the hot path of every step.

  It closed a small disagreement on the way. `can_fit` took a height, handed it
  to the map half, and let the overlay half reach for the body constant — so it
  answered about a person whatever it was asked about. Every caller passes a
  person's height today, so nothing changed; what changed is that it now cannot
  drift.

- **The client's `Resources::map` is a method, and holds one `expect`.** `World`'s
  base is optional because a shard with no client files is a real configuration.
  A client is not one — it opened the install to get this far, and `run` fails
  before a `Resources` exists — so the absence is unreachable at that end. The
  choice was one `expect` behind an accessor against forty of them at the
  readers, and the accessor won. **Its cost is a borrow**: a `&self` method
  borrows the struct where the field it replaced borrowed only itself. One call
  site paid it (`window.rs`'s atlas rebuild, which wants `&mut resources.anim`
  beside the map) by hoisting its argument into a local. Filed in
  [the map backlog](2026-08-23-the-world-and-map-backlog.md) with what to do if
  a second one appears.

- **`FacetState::set_map` replaces the public field.** A facet is inserted and
  then loaded on both ends — the tick's loader builds the state before it has
  read a map, and five test fixtures build one and then hand it a scene — so
  something has to give it ground after the fact. What it must not be is a field
  a reader can take without the layer beside it, which is exactly what the old
  `pub map` was.

## What was found

Two things, neither caused by the move:

- **The `Resources` doc had to gain an exception.** It says *"nothing here
  changes because of a packet"*, and now one field does: the live layer. It is
  there because the ground is there, and splitting the two across two structs is
  the arrangement this era exists to end — but the doc had to say so rather than
  quietly stop being true.
- **`openshard-movement`'s root is still thirty `pub use` lines.**
  [`style.md`](../../style.md) asks that a type be imported from the module that
  declares it. That wholesale re-export is how `Tile` and `Overlay` read as
  movement's types from the outside for as long as they did — nine server crates
  imported them from there while only wanting to name a place. R2 removed the
  five that were lying; the same reading applies to the rest, and it is filed in
  [the map backlog](2026-08-23-the-world-and-map-backlog.md).

## What is next

**R3, R4 or R5** — they are independent of each other and R2 was the last thing
any of them waited on.

- [**R3 — a house has floors**](2026-08-23-era-r-the-map-you-hold.md#r3--a-house-has-floors) is the
  one with a feature in it, and the one that can change where a body stands on
  ground it already walks on. Its node names the case to test: a floor laid
  exactly on the ground it duplicates.
- [**R4 — statics become one run**](2026-08-23-era-r-the-map-you-hold.md#r4--statics-become-one-run)
  is measured, bounded, and has its oracle already written.
- [**R5 — one install, one load**](2026-08-23-era-r-the-map-you-hold.md#r5--one-install-one-load)
  is the smallest.

**What would block them:** nothing.

**What not to start:** era P, still. [`map_rebuild.md`](../../archive/world/map_rebuild.md)'s
argument for its order is that `Spans` is a projection of the two layers R is
*still* shaping — R4 changes how the statics are held and R3 changes what a house
contributes to a surface — so a span grid built now is a span grid built twice.
R2 was the node P's *type* waited on, not the era.
