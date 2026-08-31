# Navigation findings

[Client backlog](README.md) · [Backlog](../README.md) · [Roadmap](../../README.md)

## The route a Ctrl-drag draws, and what is left around it

Built: `steer::plan` reads the ground twice (`steer::Readings` — the map with the
shard's items over it, and the map alone), so a destination sealed off by
something placed is planned *up to* that thing rather than answered "no route";
and where neither reading has a way through, `movement::find_path_toward` plans
as far toward it as the ground goes. **A destination now never asks for a step
this end can already see refused** — the straight-line fallback that used to
shove at a wall until a patience ran out is gone, and every one of those steps
was a `0x21` and a rollback. The walk takes the open half and stands at the
obstacle; the client draws the whole plan green up to it and red past it,
whether or not the terrain overlay is switched on (`App::route_shown`,
`shell::draw_route`). What is left:

- ~~**This end cannot tell a door from a crate.**~~ It can, and now does. The
  fact was already in the tree: `client/render/src/doors.rs` carries ServUO's own
  door families (`data/doors.json`), which is what `clutter.rs` now asks — so a
  blocker is marked `door`, the tiles the shut ones stand on are the list of
  "potentially passable, currently closed", and `Cluttered` reads either as the
  world stands or with every door open. The two readings differ by exactly that
  list, which is what makes the red half of a drawn route mean *a shut door*
  rather than "something the shard placed". The wire needs no new flag.

  **It found a bug on the way in.** `clutter.rs` used to argue that no door state
  had to be tracked, because a door's graphic changes when it swings and only the
  shut leaf is impassable. Measured against the real `tiledata.mul`: all 164 shut
  leaves in the table are impassable, and **so are 132 of the open ones** — so
  this end was refusing to walk through open doors, steps the shard allows. An
  open leaf is now left out of the index entirely.
- **Nothing opens the door.** The classic client's answer to arriving at one is
  the player's double-click, and that is still the whole of ours. A walk that
  ended in front of a shut door could reasonably send the `Use` it already knows
  how to send (`link::Command::Use`) — deliberately not done here: it is a
  gameplay decision (a locked door, a house that is not yours) and not a
  rendering one.
- **The patience is the ordinary one.** A body standing at a shut door is given
  up on after `STUCK_STEPS` beats like any other stalled destination, so a door
  opened more than about a second and a half later needs a fresh click. Holding
  the order longer would want a reason to believe the door is *about* to open,
  which nothing on this end has.
- **A goal that cannot be *stood on*, in a room whose door is shut, walks to the
  wrong side of the building.** `plan`'s middle step needs a full route over the
  bare map to have something to cut, and a tile nothing can stand on (a table, a
  chest, the wall itself) has none — so it falls through to "as close as the real
  ground gets", which is the outside wall nearest the goal rather than the door.
  Clicking the *door* is fine (the doorway is standable with the leaf gone), and
  so is clicking furniture in a room that is open. Fixing it means cutting an
  approach rather than a route — `find_path_toward` over the bare map, cut by the
  real one — which is a third case in `plan` and was not worth the branch until
  somebody hits it.
- ~~**A destination is a tile, so a click on an upper floor walks under it.**~~
  Fixed. A move order is a `Point` now, from the click all the way to the
  arrival test: `Steering::goal`, `CachedPlan`, `RouteCache` and `steer::plan`
  all carry the height, and `plan` no longer plants the body's own z on whatever
  tile it was handed. Which place that height names is one rule in one place —
  `movement::destination_place`, the search's own `goal_node`, made public for
  the arrival test rather than restated beside it.

  **Where the height comes from is the other half.** `App::pick_tile` unprojects
  at the *body's* height, so a roof under the cursor answers with the ground tile
  behind it; a click that landed on a static now takes that static's own place
  (`App::walk_destination`), which is the precedence `target_under_cursor`
  already answers a shard's location cursor with, and the preview
  (`App::route_shown`) asks the same function so the drawn line cannot disagree
  with the walk. Measured on facet 0 before the fix: from `(1375, 1673, 30)` a
  click on the floor at `(1355, 1680, 52)` planned 21 steps to the street *under*
  it; the route up the stairs is 68 steps and the coarse graph answers it.

  **A house somebody built, and which of two picks the frame drew in front.**
  Both settled, in that order. The height used to come off `Hover::static_`
  alone — the *map's* furniture — while a player house's floor is a live item:
  `App::apply_items` expands the multi into `presentation.items` where the view
  becomes a draw list, every piece carrying the house's own serial. So a click on
  an upper storey fell through to the street under the cursor. The arm that fixes
  that could not simply be added, because the item pick and the static pick are
  two independent hit tests and the static one used to be asked *only* where the
  item one came back empty — an unstated "the item always wins" that adding the
  arm would have promoted into a movement order.

  **The reason to prefer one is the frame's own, and it was already computed.**
  Both searches build a `depth::Order` to break ties inside their own list and
  used to drop it on the way out; they answer with it now (`depth::Hit`).
  `App::frame_facts` asks both and `picking::in_front` keeps whichever the frame
  drew in front, giving a tie to the item because the two lists go into one pass
   — the map's statics first, the shard's items after — under a `LessEqual` depth
  test (`renderer::depth_state`). The two orders are comparable because
  `items::place` *is* `statics::place` rather than a copy of it, which
  `a_map_static_and_a_ground_item_are_ordered_by_one_arithmetic` now pins.
  `Hover::item` carries the hit piece's own `at` beside the serial: a house
  repeats one serial down the whole parallel list, so the serial alone could
  never have been looked back up to the storey that was clicked. The walk itself
  needed nothing further — `clutter::fill` already lays each piece's
  `Cover::of_static` into the live overlay the plan is searched over — and the
  preview follows for free, since `App::route_shown` asks `walk_destination`.

  **What is still answered in the old order, deliberately.** Two:

  - **The crowd short-circuits both lists.** A mobile under the cursor is
    answered before either search runs, on the argument that a creature stands
    *on* its tile's clutter and is what a player pointing at a shopkeeper on a
    rug means. `mobile_priority_z` is `z + 1`, so depth agrees with that argument
    wherever it is genuinely about a body and a rug; the one place the two differ
    is a body behind a wall standing on the tile in front, which the frame draws
    over it and the pick hands back regardless. Folding the crowd into
    `in_front` changes what a war-mode click and a target cursor land on, so it
    is a decision of its own rather than a tidy-up of this one.
  - **`App::use_under_cursor` asks its own picks.** A double-click re-runs
    `mobiles::pick`/`items::pick` against the live camera instead of reading the
    hover, so it never consults the map's furniture at all and will use an item
    the frame drew a wall in front of. `App::attack_under_cursor`'s doc already
    says reading the hover is what this should always have been; it is now also
    the one reader left that can disagree with the picture about what is in
    front.
- ~~**The client's flat plan gives up well before the coarse one does, and the
  fallback only runs past 8 tiles.** That same 68-step route costs ~1,600 node
  expansions and `PLAN_BUDGET` is 700, so the flat search exits on budget and
  `Readings::path` falls through to `find_long_path`, which answers it — but only
  because the goal was 20 tiles away. A storey reached by a staircase *inside*
  `COARSE_MIN_DISTANCE` (8 tiles) has no fallback at all: the flat search is the
  whole answer, and a big enough building will run it out. Nothing measured is
  hitting it yet; the fix, when something does, is to let the corridor answer a
  short query whose flat search exited on `Budget` rather than on `Exhausted` —
  the two failures are already told apart in `SearchExit` for exactly this.~~
  Fixed: `Exhausted` inside the threshold remains a final local refusal, while
  `Budget` falls through to the hierarchy on both the client and the shard's AI.
- ~~**A placed multi-house has no navigation graph of its own.** The coarse graph
  is baked from the static map, while a house's floors and stairs arrive in the
  live overlay; consequently an endpoint on an upper storey cannot join the
  coarse graph. The five-storey tower still fits the 700-node local cap (645
  places from its fifth floor to the street), but larger designs turn a simple
  descent into a budget refusal. Build a revisioned per-house storey graph when
  the house is placed or redesigned: nodes are standable `(x, y, z)` surfaces,
  edges are the production step rule's legal transitions, and exterior nodes
  join the static graph at the house's entrances. Keep doors, items and mobiles
  out of that bake and validate every refined edge against the live `Footing`,
  as the existing coarse route does. This is a dynamic navigation graph rather
  than a polygon mesh: UO movement and overlapping storeys are already defined
  on tile-height places.~~ Fixed at the graph seam without a second persistent
  artifact: when an endpoint stands in a column with a live surface,
  `NavigationGraph::live_join` floods directed `(x, y, z)` places until they
  meet the static portals. It keeps the chosen prefix/suffix for refinement,
  and every edge is replayed through the current `Footing`, so doors, items and
  mobiles remain live vetoes. The reverse join enumerates and checks real
  predecessors rather than reversing descent; climbing and dropping therefore
  retain the production step rule's asymmetry. The flood stops 64 steps past
  the first portal and shares `LONG_PATH_EFFORT`, so an isolated live platform
  cannot turn into an unbounded facet walk. Tests cover out of a live upper
  storey, into it, and two floors of one `(x, y)` after the flat budget is spent.
- **The preview replans per frame while a destination is live** — the walk plans
  at most once a step, and drawing from its stored route would blink the line out
  on every mouse-move (see `App::route_shown`). Bounded by `PLAN_BUDGET` and paid
  only while there is something to draw, but an unreachable destination pays up
  to three full-budget searches a frame for the second and a half before the
  order ends. If that ever shows up in a frame time, the fix is to cache the plan
  against (body tile, goal, view generation) — not to give the picture a cheaper
  rule of its own, which is how the two would start disagreeing.

## Backlog: the cache guard at a house never fires on the packet path

`App::entered` (`client/app/src/net_command.rs:452`) decides whether to throw
away the route, plan, terrain and occluder caches by comparing the incoming view
against the one it already holds:

```rust
let items_changed = self.world.authoritative.view
    .as_ref()
    .is_none_or(|old| old.items != view.items);
```

Its comment says why it exists — "invalidating it unconditionally made the same
expensive plan run on every update (and therefore effectively every frame) at a
house" — and on the path it was written for it cannot work. `apply_packet`
(`:327`) does `self.world.authoritative.view.take()`, mutates the view it now
owns, and calls `entered(*view, …)` at `:394`. Inside, the field is `None`, so
`is_none_or` answers **true** unconditionally; the view is only put back at
`:727`, after the check. Every ordinary packet — a stranger's `0x77`, a line of
speech — therefore clears `terrain_cache`, `occluder_cache`, `route_cache`,
`steer.clear_plan_cache()` and `steer.clear_route()`.

Of the three callers only `reproject_item_drag` (`:71`, which clones rather than
takes) and the `0x1B` path (`:154`) compare against anything.

The comparison is also `O(n)` over the item map, which at a castle's roughly
four thousand locked-down items is not free even when it does run — and it
cannot see an item that moved and came back to the same place. Both go away
together if `items_changed` is derived from **what the packet was** rather than
from a map diff: the mutation path already knows it applied a `WorldItem`,
a `Remove` or an `AddToContainer`, and that answer is O(1) and exact.
