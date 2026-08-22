# A map we can change

We want a world we can edit. The shard should be able to move a coastline,
raise ground, put a wall up or knock one down, and have every connected player
see it — with nobody editing files on their own machine.

Everything else is a consequence of that map being large. Nothing is a
consequence of anything else, and keeping that true is the point of splitting
this track into three documents: the want is here, the mechanics are in
[`mechanics.md`](mechanics.md), and the work with the code it touches is in
[`plan.md`](plan.md).

## What is wrong now

The world is the player's UO install, and only that:

- **Both ends read the files separately and agree by luck.** The shard loads a
  facet at boot ([`boot.rs:618`](../../../crates/server/server/src/boot.rs#L618)) and
  our client loads one for itself
  ([`lib.rs:461`](../../../crates/client/app/src/lib.rs#L461)). They match because
  they opened the same install, not because either was told what the world is.
- **Nothing in the engine can change the ground.**
  [`Map::set_land`](../../../crates/common/map/src/map.rs#L256) and
  [`Map::place_static`](../../../crates/common/map/src/map.rs#L285) exist, and
  every caller is a test fixture or a bake. There is no path from "an operator
  changed the world" to "a player sees it", because a change would have to end
  up in the client's own `map0.mul`.
- **Everything derived is keyed to the files, not to the world.** The
  navigation bake stamps input file names, sizes and mtimes
  ([`bake.rs:22`](../../../crates/common/movement/src/bake.rs#L22)); the building
  flood, the occluder measurements and the minimap cache are baked off the same
  install. Change the ground and none of them would know they had gone stale.
- **A change has no identity.** No author, no revision, no order, nothing to
  revert. The only unit of change we have today is "someone shipped different
  files".

What the shard *can* already do is put things *on* the world — houses, items,
doors, boats — as entities with serials and their own persistence. That half
works. It is the world underneath them that is frozen.

## What we want to be true

- The map is our data. UO's files become one **importer** — a way to create a
  starting world, not the runtime source. Our client needs no UO install.
- There is **one** world everybody reads: the renderer, the step check, the
  pathfinder, the building flood, the minimap. Not one reader each.
- A change is a **unit** with an author, an order and an undo, laid over an
  untouched original — not a rewritten world.
- The server stays the authority. A client's copy of the world is for drawing
  and for guessing its own next step, never for permission.
- An unmodified 2D client keeps reading its own files and simply does not see
  our changes. That is accepted; the changed world is for our client.

## Where this is going

A world editor: someone reshapes space and commits. The commit travels as a
patch, lands on top of the original map, and every reader above sees the new
world at a known revision.

Two things that were *not* wanted, said out loud because the first draft of
this plan assumed both:

- **Thrift is not a goal.** The classic format was designed when bandwidth was
  scarce. Whole self-contained chunks over a cache that sometimes re-fetches
  too much beat any delta scheme that can leave a client half-patched.
- **A baked map on the client is a nicety, not a pillar.** With an honest cache
  and background download the player sees the same world. If a shipped pack
  turns out to be easier, we ship one.
