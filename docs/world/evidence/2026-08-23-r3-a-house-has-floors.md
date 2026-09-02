# 2026-08-23 — R3: a house has floors

[`realtime_map.md`](2026-08-23-era-r-the-map-you-hold.md#r3--a-house-has-floors)'s third node,
built in five commits. Unlike R1 and R2 this one **changes behaviour on
purpose**: it is the era's only node with a feature in it.

Five commits: `e21616ca` an obstacle is a cover, `c485bedf` a platform is two
covers, `ce5c5097` a house has floors, `f5956a3b` you can stand on the second
storey, `0042c08c` one expansion of a multi.

## Where it stands

`Cover::of_static` reads a platform now, and a platform lays **two** covers: the
surface a body on top of it stands on, and the body a mobile beside it walks
into. Two entries rather than one entry with two answers, which is the shape a
ship's plank already had — so `CoverKind::Stands` stays the purely positive arm
and nothing asking what is in the way learns that some of it is floor. `Stands`
gained a `climbable` half, because a stair is stood on half way up and *met at
its base*, and those are two numbers: `Cover::surface` and `Cover::reach`.

`Cover` therefore has three tops, and they are documented as a table where
`crest` is defined:

| | what it is | who asks |
|---|---|---|
| `top` | the body, never empty — a zero-tall wall is still a wall | what is in the way |
| `surface` | where feet go, half way up a climbable | where a body lands |
| `crest` | the art's own extent | how far the **next** step reaches |

Those are ServUO's `itemTop`, `ourZ` and `zTop`, and a staircase needs all three.

Everything downstream follows from the art being read once:

- **The shard's index holds covers.** `Obstacle` was a `z`, a `height` and a
  `door: bool` that were converted into a `Cover` on the way out; it holds the
  cover, and the identity gained a third part — the entity, the z, **and which
  arm it is** — because one component can lay two covers at one z.
  `Obstructions::is_blocked` became `holds_anything`, since a floor is now in
  there and "blocked" would be a lie about an open room.
- **A house's footprint is what the house covers**, not what its walls stop.
  `footprint_of` lays whatever `Cover::of_static` says each component's art lays.
- **`can_step` reads the live layer's surfaces where the map *allows***, not
  only where it refuses. That is `climbed`, and it is how you get upstairs: the
  map answers a house's tiles with the ground underneath, because the house is
  not in the map's files at all.
- **One expansion of a multi.** `Component::placed_at`, in `openshard-uofiles`.

**`cargo check --workspace --all-targets`, `cargo clippy --workspace
--all-targets` and `cargo fmt --all` are silent.** `cargo test --workspace
--no-fail-fast` ends with the same two red tests in `openshard-state` and no
others — R1's finding, still filed under [*`can_step` does not check the
corner*](../../roadmap.md).

## What the node decided

Four questions the plan did not name. All four are in the plan's own node under
*What the node decided*; the reasoning is there.

The one worth repeating here is the **order the flags are read in**. `PLATFORM`
is asked first and `BLOCK` only where it is absent, so a table is a platform and
not a solid twelve-tall body. That is not a preference: `MapTerrain::static_top`
already branches on `is_platform` and never looks at `is_blocking` after, so
reading them the other way round would give one piece of art two heights
depending on which layer asked about it — the map or the overlay — which is the
class of defect this era exists to close.

## The risk it named, and how it held

*A floor laid exactly on the ground it duplicates must not change where a body
stands.* A house's ground floor is a `PLATFORM` of tiledata height zero at
`dz = 0`, and its surface is the same z the land already answers with.

Two things keep it, and both are load-bearing:

- **A platform of no thickness lays no blocking half.** `Cover::top`'s `max(1)`
  is right for a wall — a zero-tall wall is not a wall — and would have put a
  one-unit body on every ground floor in Britannia, sealing each house shut from
  the inside.
- **`climbed` only takes a surface strictly above what the map answered.** So a
  duplicate is a no-op rather than a body lifted a hair.

Tested as `a_ground_floor_laid_on_the_ground_seals_nothing`.

## What was found

Four things, all filed in [`roadmap.md`](../../roadmap.md) under *Backlog from
R3*. None blocks R4 or R5.

- **`aboard` has no reach filter, and now it lets a house in.** Where the map
  refuses a tile outright, `walk.rs`'s `aboard` still takes the *nearest* live
  surface at any distance — a deck's rule, written when a deck was the only
  thing that could be one. A house over open water now lays surfaces there too.
  The fix is a decision rather than a filter, because the case `aboard` exists
  for is a body stepping *down* onto a deck from a mast and reach does not
  describe that.
- **`standing_on` walks the map's start surface a second time**, because
  `map.can_step` computes it internally and throws it away.
- **`Obstructions` is not obstructions any more** — it holds floors. The rename
  touches every server crate and no behaviour here depends on it.
- **A house's placement checks got stricter and nothing measured by how much.**
  The road test and the flat-ground test see a house's *interior* tiles for the
  first time; both are ServUO's rules over the whole plot, and both were only
  ever asked about walls. Worth a placement of each classic multi over the
  shipped decoration data before anyone calls housing finished.

## What is next

**R4 or R5** — independent of each other, and R3 was not a precondition for
either.

- [**R4 — statics become one run**](2026-08-23-era-r-the-map-you-hold.md#r4--statics-become-one-run)
  is measured, bounded, and has its oracle already written.
- [**R5 — one install, one load**](2026-08-23-era-r-the-map-you-hold.md#r5--one-install-one-load)
  is the smallest.

**What would block them:** nothing.

**What not to start:** era P, still, and for the reason
[`map_rebuild.md`](../../archive/world/map_rebuild.md) gives — `Spans` is a projection of the two
layers R is shaping, and R4 changes how the statics are held. R3 has now changed
what a house contributes to a surface, which was the other half of that
argument; **R4 is the only remaining reason to wait.**
