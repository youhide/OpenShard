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
| Every boot replays the whole log | ⬜ | S4 | [`plans/world/what_a_change_costs/PLAN.md`](../../plans/world/what_a_change_costs/PLAN.md) |
| `revert` is a word no operator can type | ⬜ | S5 | the same |
| `tiledata.mul` and the multis are still the player's install | ⬜ | S6 — a base set replaces `map` and `statics` and neither of those | the same |
| Residency and compression at rest | ⬜ deferred on purpose | the working set a real session touches, which nobody has measured and nobody has asked for | direction G |

## What is open, ranked

Every entry below was a bullet in one of the thirteen backlog sections the plans
in this domain each kept for themselves. A finding with a defect behind it is a
row here; a finding with nothing behind it stayed where it was measured.

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

**6. "The highest static on a tile" is re-derived by linear scan in four
places** — the radar, `MapTerrain`, cutaway, occlusion — because the sort key is
`(y, x)`. It cannot simply become a z-sort: **file order is draw order** and
`statics::pick` breaks ties by taking the last. Our own chunk format is where
the two get separated deliberately, which turns every one of those scans into a
suffix lookup.

**7. The building flood's artifact is 112 MiB of raw `u32`** — one label per
tile, overwhelmingly the exterior's zero, read four bytes at a time on the
startup path. Run-length or a sparse per-block index cuts it by orders of
magnitude.

**8. A column's surfaces are walked in at least two places** — `cutaway::stack`
on the client, `movement`'s `surfaces`/`spawn_z` on the server — with the same
question asked of the same files. Interiors' R1a would be a third caller and is
the moment to decide whether it is one function.

**9. `Cluttered::sight_clear` is the map's answer only**, missing the shut-door
half the server has. When it gets its reader, the shared arithmetic wants to
live in `common/movement` once rather than on both ends. `sight_clear`'s own
height blindness is the same shape one layer down: a sight line reads the tiles
it crosses and not the endpoints' columns, so two mobiles on one tile at
different z see each other through a floor.

**10. The plan cache's invalidation boundary is not covered.**
`net_command::entered` keeps the client's plan across mobile-only updates on the
assumption that `WorldView.items` is the complete input to `Cluttered::can_step`.
Enumerate every production update that can alter the predicate and assert the
boundary.

**11. The node budgets, and what a tick can afford.** 400 for server AI and 600
for a client plan were measured against *tiles*, and a node is a place now, so a
column with two floors can be finalised twice. Half the argument exists — a
`Weight::PLANNING` search at 400 arrives at more destinations than an exact one
at 600, for routes 0.2% longer — and the missing half is the shard's own
numbers rather than the probe's.

**12. The radar's 21% tangent margin is three people saying 20.7% left a seam.**
The tests pin the arithmetic and nothing says the seam is gone, because nothing
here can see. Attribute it with a debug view first.

**13. Real-install facet-0 bake/load measurements** inside the dedicated
`MemoryMax=2G` cgroup — artifact size, peak memory, cold-load time, readiness —
have not been re-recorded since the compact graph and component grouping landed.

**14. The counters nobody reads.** `RadarCacheCounters` and `RadarWorkCounters`
are written and unread outside the development HUD; markers on the minimap are
the player and nothing else, and which of party, waypoint and corpse belongs
there is a decision rather than a drawing.

**15. The publish window.** A revision is visible before the rebake over its
touched chunks finishes, and today's rule is that a stale artefact refuses itself
— so routing in those chunks degrades to flat A\* until the rebake lands. The
alternative is to rebuild the touched regions *inside* the publish and pay the
latency. It is a real choice and it should be taken with a measurement of a
single-region rebuild rather than by preference. What made it urgent is gone: a
restart inside the window used to refuse to boot, and boot replays the log's
missed chunks now.

**16. Two whole-facet CPU paths with no caller but their own tests.**
`RadarCache`'s `bake` and `mark` are the whole-map image path, worth keeping only
if something is going to want a whole-map image; the minimap's close affordance
is a provisional `M` and says so in `event_loop.rs`.

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
  — the runtime map, node by node: R1 to R4 built and R5 struck.
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
same statics. [`housing.md`](../housing.md),
[`customisation.md`](../customisation.md) and [`boats.md`](../boats.md) are what
gets laid *over* the map.
