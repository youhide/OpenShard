# World and map backlog

[Backlog](README.md) · [Roadmap](../README.md)

## Backlog from R2, the live layer joining the map

Found while moving `Overlay` and its friends into `openshard-map`
([`realtime_map.md`](../../map/realtime_map.md)'s R2). None of them blocks R3, R4 or
R5.

- **`Resources::map()` borrows the whole struct where a field borrowed itself.**
  The client's map is behind a method now, because `World`'s base is optional
  for a shard's sake and a client can never be that shard. A `&self` method is
  opaque to the borrow checker's field disjointness, so a caller wanting
  `&mut resources.<anything>` beside the map has to hoist — `window.rs`'s atlas
  rebuild already did. If a second one appears, the answer is a free function
  over `&Resources::ground` rather than another hoist: that borrows one field,
  exactly as the old `resources.map` did. (The field was `world` when this was
  written; the ground and its span bake are one value now — see
  [`Ground`](../../../crates/common/movement/src/ground.rs).)
- **`World` has no way to publish a patch.** `MapSnapshot::publish` wants
  `&mut self` and `World::snapshot` hands out a `&` — as does `Ground`, which
  wraps it and forwards that accessor. Nothing in production
  publishes yet — only `openshard-map`'s own tests do — so the accessor was left
  unwritten rather than guessed at. Era S is what needs it, and what it should
  look like is a question about who is allowed to move a facet's revision, not
  about the borrow.
- **`openshard-movement`'s `lib.rs` is still thirty `pub use` lines.**
  [`style.md`](../../style.md) asks that a type be imported from the module that
  declares it; the crate's root re-exports its own private modules wholesale,
  which is how `Tile` and `Overlay` came to look like movement's types from the
  outside for as long as they did. It is not R2's to fix — R2 removed the five
  that were lying — but the same reading applies to the rest.

  > **Eight now, not thirty**, worn down by the nodes that came after — era P
  > moved the search's own types to the modules that declare them. The reading
  > still applies to the eight; the number in this entry does not.
## Backlog from R3, a house having floors

Found while giving `Cover::of_static` its platform arm and teaching `can_step`
to read the live layer's surfaces
([`realtime_map.md`](../../map/realtime_map.md)'s R3). None of them blocks R4 or R5.

- **~~`aboard` has no reach filter, and now it lets a house in.~~ ✅ Fixed.**
  Where the map refuses a tile outright, `walk.rs`'s `aboard` took the *nearest*
  live surface at any distance — a deck's rule, written when a deck was the only
  thing that could be one. A house built over open water lays surfaces on those
  tiles too, so a body on the shore could step onto whichever storey happened to
  be nearest its own z rather than onto the one it could climb to.

  **The question this entry asked is answered: one rule, two entrances.**
  `aboard` and `climbed` are reached according to whether the *map* had anything
  to say about the tile, so a climb limit on one and not the other made the
  reachability of a storey depend on whether there was water under it. There is
  one limit now, and `Overlay::surface_at` takes the reach as an argument — the
  caller's, because how far a body may climb is a *movement* rule and this is the
  same layering argument that keeps `SpanIndex` out of `openshard-map`.

  **And this entry's own objection to a reach filter does not hold.** It says the
  filter cannot be the fix because `aboard` exists for a body stepping *down*
  onto a deck from a mast. `Cover::reach` of a flat surface is its own height, so
  everything below the body passes the filter at any value: the climb is bounded
  and the descent is untouched. Asserted both ways in
  `boarding_from_open_water_obeys_the_climb_limit`, whose control is the limit
  removed by hand — it then fails at exactly the first assertion.

  **What it cost is two fixtures, and both were asserting a boarding the step
  rule does not permit.** `boats`'s deck stood three above its shore and
  `obstruct`'s five, and both passed only because `aboard` applied no limit; a
  walk climbs at most `MAX_STEP_UP`, which is two. Both now put the deck within a
  step, which leaves what those tests are *about* — the map refusing water, the
  deck overturning that, the hull refusing again — unchanged.

  **What is now visible, and is `boats.md`'s:** this shard has no plank. A UO
  player does not walk aboard over the gunwale — they step on the plank, whose
  `OnMoveOver` sets `from.Location` and teleports them
  (`ServUO/Scripts/Multis/Boats/Plank.cs:136`). So "can a body board a real sloop
  from a real shore" is a question about real deck heights that no test here
  answers, and the honest answer is that boarding is the plank's job and the
  plank is not built.
- **`standing_on` walks the map's start surface a second time.** `map.can_step`
  computes `start_surface(from)` internally and throws it away;
  `climbed` needs the same number to measure reach from, so on any tile with a
  live surface on it the walk happens twice. It is one static loop over one
  tile, and it only runs where the overlay has a surface at the destination —
  but the honest fix is for the map's step check to hand back what it already
  knew, which is a signature change `can_step`'s three callers would all see.
- **`Obstructions` is not obstructions any more.** It holds a house's floors,
  which are the opposite of an obstruction — `is_blocked` had to become
  `holds_anything` for exactly that reason. The type is the *identity* half of
  the overlay (who put this here), and that is what it should be called. Not
  renamed in R3 because the rename touches every server crate and none of R3's
  behaviour depends on it.
- **A house's placement checks got stricter, and nothing measured by how much.**
  `footprint_of` now returns an entry for every component that lays a cover, so
  the road test and the flat-ground test see a house's *interior* tiles for the
  first time — they only ever saw its walls. Both are ServUO's rules over the
  whole plot and both are more correct this way, but a plot that was legal
  before and is refused now would look to a player like a regression. Worth a
  pass over the shipped decoration data with a placement of each classic multi
  before anyone is told housing is finished.
## Backlog from R4, the statics becoming one run

Found while making a facet's statics one run with a per-block offset array
([`realtime_map.md`](../../map/realtime_map.md)'s R4). Nothing here blocks era P.

- **A patch of many ops is now quadratic in the facet.** `place_static` and
  `remove_static` move the tail of the run and every offset past it, where they
  used to move the tail of one block — which is right for the one op a published
  patch usually is, and wrong for a thousand. Nothing publishes at that size
  today; [direction F](../../map/new_map_representation/plan.md#f--the-editor)'s editor
  is what will, and the fix it wants is a publish that groups its ops by block
  and rebuilds each touched block once, rather than an op at a time. Worth
  measuring before designing: the whole run is 29.5 MiB, so an op is a ~30 MiB
  move, and the crossover with "just rebuild the facet" is not far away.
- **`WorldMap::from_parts`' grouping is a contract with no oracle.** It asserts
  that the counts are one per block and that they sum to the run's length —
  neither of which catches a caller that put the *right number* of items in the
  *wrong* block. That sorts them into the wrong span and every lookup after it is
  silently wrong, which is the failure mode this crate's block order has always
  had. Both callers are in-tree and both are tested end to end (the base-set
  round trip and the client-files import), so this is about the third one: a
  debug-only check that every item's coordinates fall in the block its count
  claims would cost one pass over the run at load.
## Backlog: the land's fourth byte is 29.4 MB of alignment

**Bigger than everything R4 saved, and nobody had written it down.** A
[`LandCell`](../../../crates/common/map/src/map.rs) is a `LandTileId` (`u16`) and a
`z` (`i8`) — three bytes of fields in four of storage. Felucca is 29,360,128
cells, so the land is **117.4 MiB of which 29.4 MB is the padding byte**; the
statics layer, after R4, is 29.5 MiB in total. The arithmetic is corroborated by
the measured peak of a facet load: 257 MiB is land 117.4 + statics 29.5 + the
file buffers it was read from, which only adds up with a four-byte cell.

**It is gated on the access staying cheap, and that gate is the point rather
than a caveat.** The land is read as a slice —
[`WorldMap::land_in_block`](../../../crates/common/map/src/map.rs) hands back
`&[LandCell]` and `land_in_row` steps one cell east at a time — and a three-byte
cell cannot be a slice of anything. Every read becomes an unaligned load and a
shift, on the path that draws every frame: the ground walk is the *one* part of
this map whose cache behaviour [`client_today.md`](../../map/new_map_representation/client_today.md)
measured as already good ("a block is 64 cells × 4 B = exactly four cache lines
… the 1997 tiling picked the cache line's size"). **If the unpack costs more
than the 25% of footprint it saves, the answer is no** — the size is worth
having only at unchanged read speed.

So what this finding asks for is a *measurement*, not a change: the ground walk
of a widest-zoom frame over a packed cell against the same walk over the cell we
have. The same gate governs the packed four-byte static record in
[R4](../../map/realtime_map.md#r4--statics-become-one-run), which until now was gated
only on whether the statics are still hot.
