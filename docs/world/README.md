# World: where it stands

The canon of the `world` domain — `common/map`, `common/basemap`, `common/tiles`,
`common/uofiles`, the search half of `common/movement`, and `server/world`. It
also holds the documents of the three readers that live in `client/render`
(the radar raster, the building flood, the roof cutaway), because what they ask
is a question about the map and not about the renderer.

**One entry point.** This page answers "what does this shard's world do today"
and says which document holds the reasoning for each line. It used to be two
pages that both claimed to be the entry — an index and a consolidation — with a
third copy of the status inside each plan. Where this page and a design document
disagree, the design document is right and this page is stale.

The work that is still ahead is in [`plans/world/`](../../plans/README.md), not
here.

## The one-line answer

**The world is our own data, it moves while the shard runs, and every reader
follows it.** A facet is a base set of our chunks plus a log of patches; one
revisioned snapshot is what every reader takes a handle to; an operator's
`.setland` reaches a connected client's screen; and the three artefacts derived
from terrain — the span layer, the statics run, the coarse navigation graph —
are carried over the chunks a publish named instead of being rebuilt or dropped.

**The map is a matryoshka: ground, statics, and the live layer over them.** One
type, held by both ends, and the invariant that makes it one type rather than
three fields:

> **What may be baked is exactly what is below the live layer.** A navigation
> graph, a span grid, a building flood, a minimap raster — every one of them is
> derived from the ground and the statics, and none of them may contain a door, a
> crate, a moored deck or a house. A reader takes the whole map; a bake takes a
> revision of the two layers under the live one.

Which layer a new thing goes in is one question — *must a bake see it?* — and
every answer taken so far is a row in [`design_layers.md`](design_layers.md).

## Readiness, by subsystem

| Subsystem | State | What is left | Held by |
|---|---|---|---|
| The ground: one `LandCell` per column, one owner of the block order | ✅ shipping | — | [`design_snapshot.md`](design_snapshot.md), [the record](evidence/2026-08-25-one-world-one-door.md) |
| One revisioned `MapSnapshot` per facet, every reader a handle | ✅ shipping | — | the same |
| The world is a crate; UO's files are an importer | ✅ shipping | — | the same |
| The statics: one immutable run and a prefix sum — 120,745 allocations → 2, 38.2 → 29.5 MiB | ✅ shipping | the **packed record** (4 bytes an item, ~13.5 MiB) is under its own gate: whether the shift and the unaligned load cost less than the cache lines they save | [era R](evidence/2026-08-23-era-r-the-map-you-hold.md) |
| The live layer inside the type — one `World` on both ends | ✅ shipping | — | era R |
| The tile table out of the file reader — `openshard-tiles`, a crate with no dependencies | ✅ shipping | — | era R |
| A house has floors a body can stand on | ✅ shipping | — | era R, R3 |
| Our own chunk format and a UO importer — 7,168 chunks, 102.6 MiB, byte-identical round trip | ✅ shipping | — | [the seven directions](evidence/2026-08-25-seven-directions.md), B |
| Patches, `publish`, the `.ospatch` log, one resolver both the shard and the bake go through | ✅ shipping | — | direction C |
| A running shard edits its own ground — four staff verbs, the world before the log | ✅ shipping | — | direction C, [`mapedit`](../../crates/server/world/src/mapedit.rs) |
| Whole chunks to our client over `0xBF`/`0xE000`, deflated; a publish reaches a connected screen | ✅ shipping | the untested joining in `link::play`; a world that moved by an empty patch; nothing sweeps an orphaned cache entry | [`design_chunks_to_the_client.md`](design_chunks_to_the_client.md) |
| Derived data follows a publish rather than being dropped by one — spans 0.3 ms, statics 0.4 ms, the graph 80 ms against 28 s | ✅ shipping | 80 ms is the *chunk's* price, not the edit's: the shard holds the tiles and could name a ring half the size, and the client cannot | [`design_navigation_graph.md`](design_navigation_graph.md#g1--the-graph-follows-a-patch) |
| The artifact catches up with the log at boot, so an edited shard restarts | ✅ shipping | — | the same, G2 |
| The span layer: three tiers, 11.2 MiB, baked in 0.05 s, equal to `stand_surfaces` on 29.4 M columns | ✅ shipping | — | [`design_spans.md`](design_spans.md) |
| A search node is a place to stand — 208 ns an expansion where it was 1,105 | ✅ shipping | — | the same |
| The coarse graph over places, with directed portals — `refused_but_walkable` 0 from all five origins | ✅ shipping | — | the same, N4 |
| The shard reads the graph — a creature rounds a town block | ✅ shipping | the *aggressive chase* plans its own route and does not go through it | the same, N7 |
| The span bake follows a patch — 109.7 ms → 0.3 | ✅ shipping | — | the same, N8 |
| Off-mesh links (a teleporter, whatever a flood cannot connect) | ⬜ gated | the flood that would name the content; N5 owns the format slot and nothing else | the same, N5 |
| A span artifact on disk | ⬜ gated, and expected to close as *not needed* | a load-time measurement nobody has asked for | the same, N6 |
| A second HPA\* hierarchy level | ⬜ gated | its own end-to-end p95 over the facet-0 route set. The defect that used to gate it — a one-storey graph — is fixed | [the efficiency record](evidence/2026-08-23-navigation-graph-efficiency.md) |
| The radar: one raster serving both windows, a view picking its own level, an evicting cache, the floor swept once | ✅ shipping | section 10 below — the page array at 2× HiDPI, the `floor`/`round` level rule, and a carry that is O(everything ready) | [`design_radar.md`](design_radar.md) |
| Interiors: the building index, the floor a person is on, a sealed room as a black area | 🟡 in the tree, unrecorded | `Buildings::bake`, `InteriorFrame` and `FloorView` exist in `client/render`; **the document carries no status marks at all**, and R3 (walls at knee height) has nothing in the tree | [`design_interiors.md`](design_interiors.md) |
| The map editor's first usable cut | 🟡 partial | continuous drag strokes, art-composited static preview, smooth, stamps, rebase | [`plans/world/map_editor/PLAN.md`](../../plans/world/map_editor/PLAN.md) |
| A corridor sends a body where the graph lets it cross, and the crossings are corners | ⬜ | P1 to P4: the corner-only crossing, one click with two answers, planning off the frame thread, and the journal's own `coarse` flag | [`plans/world/pathfinding/PLAN.md`](../../plans/world/pathfinding/PLAN.md) |
| Every boot replays the whole log | ⬜ | S4 | [`plans/world/what_a_change_costs/PLAN.md`](../../plans/world/what_a_change_costs/PLAN.md) |
| `revert` is a word no operator can type | ⬜ | S5 | the same |
| `tiledata.mul` and the multis are still the player's install | ⬜ | S6 — a base set replaces `map` and `statics` and neither of those | the same |
| Residency and compression at rest | ⬜ deferred on purpose | the working set a real session touches, which nobody has measured and nobody has asked for | direction G |

## What is open, ranked

Every entry below was a bullet in one of the seventeen backlog sections the plans
in this domain each kept for themselves — thirteen inside the documents this
domain was split out of, and four more the roadmap kept as its own world-and-map
backlog until 2026-09-02. A finding with a defect behind it is a row here; a
finding with nothing behind it stayed where it was measured.

**1. The map and the overlay disagree about a platform of no thickness.**
`MapTerrain::is_obstructed` gives a floor a body from `base` to `base`, so it is
in the way of anything below whose head passes it; `Cover::of_static` lays no
blocking half for the same art at all. So a floor the map shipped and a floor the
shard placed answer differently for a body in the cellar underneath. Three nodes
declined to settle it in a row and each was right to: their oracle was that the
answer did not change. It is now visible in six lines of `walk::landing`, and
what it needs is a decision rather than another node. It is a defect of the
**step rule**, not of the span layer.

**2. The graph is a forest, and nothing in it says which tree a node is in.** A
facet is islands, so `abstract_path` settles every node reachable from the start
before it can refuse a goal on another island — Moonglow **4.1 ms** from
Britain, dearer than the 1,464-step route to Trinsic that *succeeds* in 2.5. One
`u32` per node and a flood at bake time turns each refusal into two loads and a
comparison, and it is the same component label `local_costs` already recovers per
region and throws away.

**3. The shared radar page array is crossed at an ordinary HiDPI scale.** One
`RadarChunkRenderer` serves both windows and holds 1,024 pages; at two physical
pixels to a gump pixel the two views want 1,132 between them, so each frame the
minimap evicts 144 of the facet map's pages and the facet map uploads them again
— 2.3 MiB of page copies a frame for as long as both windows are open. The
cheapest of the three ways out is `round` instead of `floor` in the level rule,
which costs a fraction of a texel of sharpness and a quarter of the array. It
changes what a person sees, so it wants both levels drawn side by side first.

**4. The client's publish path still calls `set_revision`.** One line in
`net_command.rs`, and the carry that replaces it (`RadarCache::moved`) is built
and called from the shard's side. It waits for the editor work that is rewriting
that file rather than landing inside it.

**5. A carry is O(everything ready), and a brush is a stream of publishes.**
`RadarCache::moved` walks every ready key and rebuilds `highest_ready_lod` from
scratch — right for an operator typing `.setland`, wrong for a drag that
publishes once a tile. The ready map is ordered, so the rename is a range rather
than a scan; nothing has measured it.

**6. A patch of many ops is quadratic in the facet, and a brush is a patch of
many ops.** `place_static` and `remove_static` move the tail of the whole run and
every block offset past it, where they used to move the tail of one block — right
for the single op a published patch usually is, wrong for a thousand. Nothing
publishes at that size today and the editor is what will. The fix is a publish
that groups its ops by block and rebuilds each touched block once; the crossover
with "just rebuild the facet" is close enough to be worth measuring first, since
the whole run is 29.5 MiB and one op is a ~30 MiB move.

**7. "The highest static on a tile" is re-derived by linear scan in four
places** — the radar, `MapTerrain`, cutaway, occlusion — because the sort key is
`(y, x)`. It cannot simply become a z-sort: **file order is draw order** and
`statics::pick` breaks ties by taking the last. Our own chunk format is where
the two get separated deliberately, which turns every one of those scans into a
suffix lookup.

**8. The building flood's artifact is 112 MiB of raw `u32`** — one label per
tile, overwhelmingly the exterior's zero, read four bytes at a time on the
startup path. Run-length or a sparse per-block index cuts it by orders of
magnitude.

**9. A column's surfaces are walked in at least two places** — `cutaway::stack`
on the client, `movement`'s `surfaces`/`spawn_z` on the server — with the same
question asked of the same files. Interiors' R1a would be a third caller and is
the moment to decide whether it is one function.

**10. A house's placement checks got stricter and nothing measured by how much.**
`footprint_of` returns an entry for every component that lays a cover, so the
road test and the flat-ground test see a house's *interior* tiles for the first
time — they only ever saw its walls. Both are ServUO's rules over the whole plot
and both are more correct this way, but a plot that was legal before and is
refused now reads to a player as a regression. It wants a pass over the shipped
decoration data placing each classic multi before anybody is told housing is
finished.

**11. `Cluttered::sight_clear` is the map's answer only**, missing the shut-door
half the server has. When it gets its reader, the shared arithmetic wants to
live in `common/movement` once rather than on both ends. `sight_clear`'s own
height blindness is the same shape one layer down: a sight line reads the tiles
it crosses and not the endpoints' columns, so two mobiles on one tile at
different z see each other through a floor.

**12. The plan cache's invalidation boundary is not covered.**
`net_command::entered` keeps the client's plan across mobile-only updates on the
assumption that `WorldView.items` is the complete input to `Cluttered::can_step`.
Enumerate every production update that can alter the predicate and assert the
boundary.

**13. The node budgets, and what a tick can afford.** 400 for server AI and 600
for a client plan were measured against *tiles*, and a node is a place now, so a
column with two floors can be finalised twice. Half the argument exists — a
`Weight::PLANNING` search at 400 arrives at more destinations than an exact one
at 600, for routes 0.2% longer — and the missing half is the shard's own
numbers rather than the probe's.

**14. The radar's 21% tangent margin is three people saying 20.7% left a seam.**
The tests pin the arithmetic and nothing says the seam is gone, because nothing
here can see. Attribute it with a debug view first.

**15. Real-install facet-0 bake/load measurements** inside the dedicated
`MemoryMax=2G` cgroup — artifact size, peak memory, cold-load time, readiness —
have not been re-recorded since the compact graph and component grouping landed.

**16. The counters nobody reads.** `RadarCacheCounters` and `RadarWorkCounters`
are written and unread outside the development HUD; markers on the minimap are
the player and nothing else, and which of party, waypoint and corpse belongs
there is a decision rather than a drawing.

**17. The publish window.** A revision is visible before the rebake over its
touched chunks finishes, and today's rule is that a stale artefact refuses itself
— so routing in those chunks degrades to flat A\* until the rebake lands. The
alternative is to rebuild the touched regions *inside* the publish and pay the
latency. It is a real choice and it should be taken with a measurement of a
single-region rebuild rather than by preference. What made it urgent is gone: a
restart inside the window used to refuse to boot, and boot replays the log's
missed chunks now.

**18. Two whole-facet CPU paths with no caller but their own tests.**
`RadarCache`'s `bake` and `mark` are the whole-map image path, worth keeping only
if something is going to want a whole-map image; the minimap's close affordance
is a provisional `M` and says so in `event_loop.rs`.

**19. The land's fourth byte is 29.4 MB of alignment, and it is bigger than
everything the statics run saved.** A `LandCell` is a `LandTileId` (`u16`) and a
`z` (`i8`) — three bytes of fields in four of storage — and Felucca is 29,360,128
cells, so the land is 117.4 MiB of which 29.4 MB is padding; the whole statics
layer is 29.5 MiB. It is gated on the read staying cheap, and the gate is the
point: the land is handed out as `&[LandCell]` and walked one cell east at a
time on the path that draws every frame, where a block is exactly four cache
lines. A three-byte cell cannot be a slice of anything, so every read becomes an
unaligned load and a shift. What this asks for is a *measurement* — the ground
walk of a widest-zoom frame over a packed cell against the cell we have — and the
same gate governs the packed four-byte static record.

**20. A long query does not report what it spent.** `search_long_path` returns
the route and a `LongExit` and drops `effort.spent()`, so the only node count a
caller can quote for a long destination is the *bounded* search's — which stopped
before the corridor was ever asked for. A route journal writes `explored=700,
long=NoCorridor` and nothing at all about the work the corridor did, and the
budgets for `LONG_PATH_EFFORT` were argued from numbers only the debug print
inside it can see. The wallet already holds the figure; it is one field on the
return.

**21. Three readers of a real facet still open one by hand.** `bake::open_facet`
took the seven that were, and three of the same shape are left, all of them
still on `map::read_facet` and a bare `WorldMap`: the `real_install` fixtures in
`terrain.rs` and `spans.rs` — two spellings of the same fixture, in one crate,
one of which bakes a `SpanIndex` and the other does not — and
`examples/span_index.rs`, which times `SpanIndex::build` on its own and would
have to keep timing it. The two fixtures are a straight conversion; the example
wants the open split into read-and-bake before it can use one.

The three below came out of the **first session the route journal recorded**
(2026-09-04, one click at `(1345, 1894, 88)` from about twenty-five tiles away,
143 plans). They are one report each, and the journal is what separated them:

**22. A destination resolves to two different places in the same second.** The
click named a column whose only *map* surface is the ground at z 0; the live
layer had a surface at z 88 on it. Over the walk, `destination_place` answered
88 on some plans and 0 on others — twice within one millisecond of each other
(lines 125/126, 130/131, 139/140) — so the client alternated between a 24-step
route to the ground and a 95-step route onto the roof. Whatever is at 88 is
therefore *entering and leaving the client's overlay* while the body stands
still, twenty-five tiles away: the same shape as the anchor-only view range this
domain has already been bitten by. Until it settles, no plan for that click is
stable, and the switch is invisible to a player — both routes draw green.

The repair this is getting is not a wider view range but a **memory**: what a
client has been shown stays on its own map after the shard stops sending it,
drawn in grey to say *this was here and may not be now*, and interaction goes on
being refused for it. A destination then stops changing height because a body
walked a tile.

**23. ~~A route to that destination oscillates~~ — closed: refinement spliced a
loop into its own route.** With the goal at z 88, the plan from `(1344, 1919)`
started `NE` — onto `(1345, 1918)` — and the plan from `(1345, 1918)` started
`SW`, back again; the body walked between those two tiles for the rest of the
session, and the stall patience could not see it (`STUCK_STEPS` compares the
body's position, and the body *was* moving).

The cause was not the ground and not the abstract route. **A corridor is a
splice, and each piece of it is optimal only on its own** — the region routes,
the portal crossings, the live join's prefix and suffix — so a query whose start
stands *past* the portal its corridor begins at walks back to that portal, and
the piece after it walks straight over the start again. The plan from
`(1345, 1918)` was literally `SW` then `NE`: one step off the tile and one step
back onto it, then the ninety-four the plan from the neighbour had.
`NavigationGraph::refine` now takes every loop out of the route it assembled
(`without_loops`), which cannot lengthen a route and cannot make an unwalkable
one: standing somewhere twice means the steps between the two visits changed
nothing.

The scene is two tests — `real_routes.rs`'s
`a_route_onto_a_castle_roof_never_visits_a_place_twice`, which lays the session's
own castle (a 2196-component custom design, kept beside the test) over the real
facet and asks the nine starts around that tile; and
`a_click_on_the_castle_roof_is_walked_to_the_end`, which **walks** the click the
way a body meets it: plan, step, plan again from where that step landed, at the
worst cadence there is — a fresh plan on every step, which is what a flickering
castle made the session do.

That second one is the report's own shape and its own oracle. Without the repair
it fails after seven steps, standing on `(1344, 1919)` for the fourth time; with
it the body walks the ninety-four steps onto the roof, in ninety-four plans.
Three of the nine starts looped before and none after, and no two neighbouring
starts plan onto each other any more.

What is *not* closed is the patience: a walk that never arrives and never stands
still is still an order nothing ends. `STUCK_STEPS` measures the wrong thing for
it, and what would measure the right one is the places an order has already
stood on.

**24. A long plan costs 56–130 ms, and there are two or three per step.** Median
61 ms over those 143 plans, every one of them with `explored = 701`: the bounded
search spends its whole budget, fails, and the coarse query then pays for the
corridor. That is the client's own frame budget several times over, on the walk
path, while a player is moving — and the preview and the step ask for it
separately. The node budget bounds the *bounded* search only; nothing bounds
what the fallback costs in milliseconds.

The castle tests above now measure the *other* half of that, and it is the
uncomfortable half: over that same click, an **exact** search arrives in 7,119
nodes and 4.2 ms, and the corridor pays 30.5 ms to return a route of exactly the
same length. Walking the whole click at one plan per step costs **2.1 s of
planning for 94 steps** — around 22 ms a step, on the walk path. So on this destination the hierarchy is seven times the price of
the answer it is standing in for, and the whole of what makes it necessary is a
budget of 700 — a number measured against tiles, which finding 13 is already
about. What that asks for is a measurement of where the crossover really is,
over a spread of destinations rather than one, before either number moves.

The three below came out of the **second session the journal recorded**
(2026-09-04, `path-journal.jsonl` episode 64 of 64: one click at
`(1342, 1893, 88)` — the same castle's roof — from `(1350, 1890, 0)`, ten tiles
away, 122 plans and the window closed before the body arrived):

**25. An open border is crossed at its corners and nowhere else, so a body
walks away from the house it was sent to.** The route onto the roof begins by
walking *south, away from the castle*: `(1350, 1890)` → south along the east
wall to `(1350, 1900)` → south west to `(1344, 1919)`, nineteen tiles past the
castle's south wall → back north east and north west to the door at
`(1341, 1900)`, which was nineteen steps from where it started. Nothing here is
a splice doubling back — finding 23's loop cut has nothing to take out, and both
legs are optimal on their own. The corridor is *one node*, and it is that one:

```text
start  (1350, 1890, 0)  region 13258 [1344..1375] x [1888..1919]
roof   (1342, 1893, 88) region 13257 [1312..1343] x [1888..1919]
corridor: 35228 (1344, 1919, 0)   source 29 + target 94 = 123 steps
```

The start's region has **five nodes and all of them are corners** —
`(1344, 1888)`, `(1375, 1888)`, `(1344, 1919)`, `(1345, 1919)`,
`(1375, 1919)` — because `add_portal` gives a run of `WIDE_PORTAL` (6) or more
crossings exactly two representatives, `run[0]` and `run[len - 1]`. A 32-tile
border of open ground is one such run, so the only places to cross it are its
two ends, and a body in the middle of a region pays up to sixteen tiles to reach
one. That is structural and has nothing to do with any house.

What the castle adds is that it **stands on the near pair**: the roof's live
join reaches no cost at all for `(1344, 1888)` or for its partner
`(1343, 1888)` across the border — the castle covers both — so the north
crossing does not exist for this query and the south corner is the only one
left. Hence 29 steps out to it and 94 back through the door.

Measured on the same scene the castle tests build — the 2196-component design
over facet 0, `Doors::AllOpen` live and the bare map as the guide:

| ask | corridor | exact |
| --- | --- | --- |
| `(1350, 1890, 0)` → roof `(1342, 1893, 88)` | 123 steps, 48.8 ms | 94 steps, 7,037 nodes, 16.6 ms |
| `(1350, 1890, 0)` → door `(1341, 1900, 7)` | 48 steps, 57.7 ms | 19 steps, 63 nodes, 0.1 ms |

**The symptom is repaired: `without_folds` cuts a loop the width of a tile.** A
route that comes back *within one tile* of a place it already stood on, at that
place's own height, has the same nothing between the two visits that an exact
revisit has — and the difference is only that a step is needed to join them,
which is why this costs a search where the loop cut costs none. `refine` runs it
after `without_loops`, charged to the query's own wallet.

Where it is asked is the whole of the price. Over this route the fold is steps
14..44 — thirty steps between two places one tile apart — and re-asking it costs
**2 nodes**; asked the brute-force way instead, every pair of route places within
sixteen tiles, the same answer costs 1,571 searches and 415,271 nodes, six times
the corridor it is repairing. The search stays the oracle rather than the
neighbourhood: a switchback stair folds back the same way and simply fails to
answer shorter, so nothing is spliced, and every step that replaces a fold is one
the search has just approved against the same footing.

The scene is `real_routes.rs`'s
`a_route_onto_a_castle_roof_does_not_walk_away_from_the_castle`, and its oracle
is a ratio rather than a route: a corridor is allowed to be longer than the
exact answer, and half as long again is not that. The click goes from **123
steps to 95** against the exact 94, and its southernmost place from `y = 1919`
to `y = 1905`. The neighbouring castle tests are unchanged — 196 long routes and
9 roof routes still loop nowhere, and the walked click still arrives in 94 steps
and 94 plans.

The cause is untouched. It is the two-representative rule, and
**intermediate representatives on a wide run** bound the detour by the spacing
chosen instead of by half a region, at the price of more nodes on open ground —
a number to measure against the bake and against `abstract_path`, not to pick.

What this is *not* is finding 24's crossover met from the other side: this
destination is only near in a straight line. Ninety-four steps through four
storeys is a long way, the exact search pays 7,037 nodes for it, and no budget
a client can afford every step makes the hierarchy unnecessary here.

**26. Every third plan answers a different question and calls the click
barred.** The plans of that episode run in a fixed rhythm of two and one: two
with `long = Route` onto the roof, then one with `long = NoJoin`, a
`doors_open` probe and `refusal = Barred`, whose route is only the prefix as far
as the castle door. Not the live layer flickering (finding 22) — the
destination resolves to z 88 on all three — but the third caller reading the
same ground with doors as they stand, where the roof's live join reaches no node
of the graph and the answer becomes "the only way through is a shut door". One
click therefore has two standing answers a third of a second apart, and the
player is shown whichever the last one was. Three plans a step at 110–124 ms
each is also the true figure behind finding 24's "two or three".

**27. The journal's `coarse` flag is a startup snapshot, and a login bakes.**
`client/app` writes the session line's `coarse: coarse.is_some()` when the
window opens; the graph a world arriving asks for is baked after that. So this
session's file says `coarse: false` while every plan in it used the graph, and
`path_replay` prints "facet 0 WITHOUT a coarse graph" over a session that had
one — which is exactly the fact the field exists to keep a replay from guessing
wrong about. The flag wants writing when the first line is written, not when the
journal is built.

**Repaired, and the repair is a line rather than an edit.** Writing the header
late is only half of it: a session's first click routinely lands *before* the
bake finishes, and then the flag on disk is honest about the lines under it and
wrong about everything after. So `note_coarse` now writes a **fresh `session`
line** at the moment the graph arrives — the same shape the F1 switch writes for
a gap — and the graph being dropped under a facet replacement writes one the
other way. Nothing before the change is rewritten, because those routes really
were planned without a corridor. A journal that has not written its header yet
still just tells the truth in the line it owes, and creates no file for a bake
nobody planned a route through. On the reading side `read::session_at` answers
which session line is in force for a line, and `path_replay` asks it for the
episode it is about to replay rather than taking the file's first.

**28. The client plans on the thread that draws.** Findings 24 and 26 are two
readings of one number: three plans a step at 110–124 ms in the session's own
build, ~30 ms for the corridor alone in `release`, on every step a body walks.
A step is ~200 ms of walking, so planning is a large and permanent fraction of
the frame while anybody is moving, and none of the repairs above changes that —
`without_folds` shortens the *route*, not the search that proposed it.

Moving it off the frame thread is the obvious answer and it is not free, so what
it waits on is a decision rather than an implementation. The search reads two
grounds: the guide, which is the bare facet and never changes, and the live
overlay, which the network side rewrites as the world arrives. A worker cannot
borrow the second and this repository does not share it behind a lock, so the
shape to argue about is what the worker is *given* — an owned slice of the
overlay around the query, cut on the frame thread — and what it costs to cut
one. The latency it adds is already survivable: a walk holds its last plan while
the next is asked for, which is what the plan cache is, and an answer that
arrives a frame late is a plan from a tile the body has just left, which is the
case refinement already handles on every replan.

Findings 25 to 28 are one track and are held as one:
[`plans/world/pathfinding/PLAN.md`](../../plans/world/pathfinding/PLAN.md) is
P1 to P4 — the corner-only crossing that causes 25, the two readings of one
click in 26, the frame thread in 28, and 27 alongside them because a lying
instrument is not a thing to leave lying about. The order there is argued;
finding 22 stays where it is, because what it waits on is the client's memory of
what it has been shown rather than anything about a search.

Two questions this domain deliberately keeps open, and neither is waiting on
work: **land height per tile or per corner** (closed the day we mean to change
the geometry, and not before) and **which validation blocks a publish**
(technical validity is mandatory and already enforced; the design list —
reachability, smoothness — is empty on purpose until somebody names a rule and
the world it protects).

## Before running anything

**Rebake first.** `ROUTING_VERSION` is 4 and a shard with an older artifact does
not boot — deliberately, because the alternative is a graph that answers with a
one-storey world:

```sh
cargo run --release -p openshard-movement --bin openshard-navigation-bake -- --facet 0
```

It takes 11.7 s on Felucca. A shard that was *edited* and then restarted no
longer needs this: boot replays the log's missed chunks into the artifact and
writes it back.

## The map: which document holds what

**Design — how it works today:**

- [`design_layers.md`](design_layers.md) — 🚩 **which layer does this go in?**
  One question — *must a bake see it?* — and every answer taken so far. Also the
  two rules that read as a contradiction (*"never an overlay"* against *"a house
  is a layer"*) and what actually separates them, and the four different things
  one art id can be. Read it before quoting either rule at a question about the
  other.
- [`design_snapshot.md`](design_snapshot.md) — base, patch, snapshot; what a
  chunk is and why it is 64×64; what goes stale.
- [`design_chunks_to_the_client.md`](design_chunks_to_the_client.md) — the pipe,
  chosen off measurements: the `0xBF` envelope in the `0xE000` range, a chunk
  deflated before it is framed, and the five phases from "the client's world is a
  parameter" to a connected screen changing.
- [`design_spans.md`](design_spans.md) — the span layer: three tiers, a
  four-byte span, what a search node is, and what the overlay carries that a bake
  may not.
- [`design_navigation_graph.md`](design_navigation_graph.md) — the coarse graph:
  regions, components, directed portals, and how it follows a publish instead of
  being dropped by one.
- [`design_radar.md`](design_radar.md) — the radar raster and every window that
  draws it. Sections 1–3 are the record of what was wrong; section 10 is what is
  open.
- [`design_minimap_lod.md`](design_minimap_lod.md) — the raster cache's contract:
  immutable products, ownership, invalidation, bounded production.
- [`design_interiors.md`](design_interiors.md) — the building flood: which cells
  are inside, which floor a person is on, and a sealed room as a black area.

**Reference:**

- [`reference/navigation_artifact.md`](reference/navigation_artifact.md) — the
  baked graph's file: its stamp, its validation, the bake command, and the rule
  that an artifact is named after its world.
- [`reference/path_journal.md`](reference/path_journal.md) — 🚩 **a click that
  walked into a wall**: the journal a session writes under
  `OPENSHARD_PATH_JOURNAL`, the `path_replay` example that re-asks it over the
  real facet, the three verdicts it prints, and how a record becomes a test.
  Deliberately holds no slice of the world — the tile it names is what a test
  builds a door on.

**Research — what was compared and what was rejected:**

- [`research/a_map_we_can_change.md`](research/a_map_we_can_change.md) — the
  want, and the two things that were *not* wanted: thrift, and a baked map on the
  client as a pillar.
- [`research/terrain_seam.md`](research/terrain_seam.md) — six terrains, five of
  which were not terrains. Closed; its node 0 is the facet-0 oracle most numbers
  in this domain come from.
- [`research/coarse_pathfinding.md`](research/coarse_pathfinding.md) — the
  routing design that preceded the graph. Superseded by its own first line.
- [`research/cutaway.md`](research/cutaway.md) — the older, global
  roof-and-height rule the interior policy replaces, with the transparency risks
  it kept explicit.

**Evidence — measurements, phase records and closed handoffs:**

`evidence/` is dated files and no index; `ls` is the index. The ones a session
is most likely to want:

- [`evidence/2026-08-25-the-span-layer.md`](evidence/2026-08-25-the-span-layer.md)
  — nine nodes, what each measured, and the thirty findings they filed. The
  node-expansion question is closed in it, with the four attempts that did not
  move it.
- [`evidence/2026-08-23-era-r-the-map-you-hold.md`](evidence/2026-08-23-era-r-the-map-you-hold.md)
  — the runtime map, node by node: R1 to R4 built and R5 struck, and
  [`evidence/2026-08-23-the-world-and-map-backlog.md`](evidence/2026-08-23-the-world-and-map-backlog.md)
  — the backlog era R filed as it went, which the roadmap kept until this domain
  had a place for it.
- **The world phase, as the roadmap recorded it**, moved here on 2026-09-02:
  [`evidence/2026-08-26-a-client-walks-in-britannia.md`](evidence/2026-08-26-a-client-walks-in-britannia.md)
  (world entry, the file-format traps, and the two rules the walk check takes one
  half of from each reference),
  [`evidence/2026-08-24-mobiles-and-the-shove-rule.md`](evidence/2026-08-24-mobiles-and-the-shove-rule.md)
  (a mobile is an obstacle, and the shove),
  [`evidence/2026-08-24-the-movement-surface-investigation.md`](evidence/2026-08-24-the-movement-surface-investigation.md)
  (the 2026-08-02 pier report: three suspects walked, none of them the cause) and
  [`evidence/2026-08-24-runtime-lookups-and-the-tick.md`](evidence/2026-08-24-runtime-lookups-and-the-tick.md)
  (the corner rule's owner, the sector bucket that became two lists, and what the
  tick guarantees).
- [`evidence/2026-08-31-the-base-set-track.md`](evidence/2026-08-31-the-base-set-track.md)
  — a world of our own, from the first imported chunk to a publish reaching a
  connected client.
- [`evidence/2026-08-25-seven-directions.md`](evidence/2026-08-25-seven-directions.md)
  — the seven directions A0–G with the code each one touches.
- [`evidence/2026-08-25-one-world-one-door.md`](evidence/2026-08-25-one-world-one-door.md)
  — `LandGrid`, the snapshot, and the world becoming a crate.
- [`evidence/2026-08-25-the-clients-map-today.md`](evidence/2026-08-25-the-clients-map-today.md)
  — what the client's map cost when the work started, measured; ten findings,
  seven of them since spent.
- [`evidence/2026-08-23-navigation-graph-efficiency.md`](evidence/2026-08-23-navigation-graph-efficiency.md)
  — the graph's format, its entrances and its transition cache; phases 1, 2 and 4.
- [`evidence/2026-08-22-minimap-lod-cache.md`](evidence/2026-08-22-minimap-lod-cache.md)
  — the raster cache as built, and the four defects it cost.
- fifty-two session records, `YYYY-MM-DD-<slug>.md`, each answering where the
  work stood, what was decided and against which alternative.

**Archive:** [`../archive/world/`](../archive/world/README.md) — the
consolidation this page replaces.

**Neighbours that deliberately did not move:**
[`design_occluders.md`](../render/design_occluders.md) and
[`design_footprints.md`](../render/design_footprints.md) are static geometry for
the lighting rebuild and belong to that document set even though they read the
same statics. [`housing/`](../housing/README.md) — houses, designed houses and
boats — is what gets laid *over* the map.
