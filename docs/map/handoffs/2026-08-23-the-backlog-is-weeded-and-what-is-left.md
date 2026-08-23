# 2026-08-23 — the backlog is weeded, and what is left of it is a list

A session with no node in it. The eras are R → P → S; R and P are closed and era
S's live publish is the next *plan*, but the ask here was the backlog first —
"there is a lot in it" — so this is the pass over it. Three commits, and the
whole of what they contain is **four entries that described work already done,
one defect refuted, one defect fixed, and one document corrected because it
rested on the refuted one.**

The rest of the backlog is catalogued at the bottom, ranked, with what would
close each entry. That list is what this handoff is *for*.

## Where it stands

### The eras, unchanged

R and P are closed and nothing here reopened them. `Span` is four bytes,
`SpanIndex` is 11,713,607 B, `ROUTING_VERSION` is 4, nothing was rebaked and
nothing was serialised. **The only production change this session is
[`walk::aboard`](../../../crates/common/movement/src/walk.rs) applying the climb
limit**, plus the signature change that let it — see below.

### 🚩 Four backlog entries were describing work already finished

Read before trusting any entry in [`roadmap.md`](../../roadmap.md): a backlog
nobody re-reads decays, and this one had decayed in four places at once.

| the entry | what is actually true |
|---|---|
| *"`can_step` does not check the corner, and two obstruct tests are red"* | Both green. Closed by the corner-rule repair — `step_allowed` owns the corner, which is the "same rule for both callers" the entry asked for |
| *"the re-run is owed"* — `boat_step_cost`, in [`terrain_seam.md`](../terrain_seam.md) | Run 2026-08-22. 110 ns against 123 ns a step; a moored ship costs 13 ns and 12%, where the reading over the `Sea` double said 267% |
| *"ten clippy sites"* | None of the ten remain. Five findings in three files do, and none are ours: `uofiles/src/map.rs`, `render/tests/traced.rs` ×3, `client/app/src/link.rs` |
| *"movement's `lib.rs` is still thirty `pub use` lines"* | Eight. The reading still applies to the eight; the number does not |

### 🚩 One unmeasured premise had spread to three documents

The pier-and-bridge entry has said since 2026-08-02 that `MapTerrain::check`'s
ServUO `landCheck` guard discards a deck and drops the walker onto the land
below it. It held itself back from a fix pending a repro. **The repro was never
run, and the claim propagated anyway** — into `boats.md`'s B5, and into the
attribution of a real player report.

Two `#[ignore]` surveys now live in
[`terrain.rs`](../../../crates/common/movement/src/terrain.rs), neither an
assertion — an assertion over a facet's worth of shipped art is an assertion
about the art.

**`land_check_survey`**, over the whole of facet 0, as a walker and again as a
swimmer:

| | walker | swimmer |
|---|---|---|
| the guard discards a platform | 2,381 | 2,385 |
| of those, climbable (pier/bridge) | 596 | 599 |
| **and the body lands below what was discarded** | **0** | **0** |
| the tile is refused outright instead | 722 | 722 |
| of those, walled *by the guard* | 242 (71 climbable) | 242 (71 climbable) |

**The fall cannot happen, and the guard's own third condition is why.**
`landCenter > ourZ` fires only where the land is *higher* than the deck, so
discarding the deck moves a body **up** onto the land. What the guard actually
costs is 242 tiles a body cannot enter — an invisible wall, not a fall — and that
is **parity**: the port is character-for-character ServUO's
(`Scripts/Services/Pathing/Movement.cs:238`).

**`predicted_step_survey`** closes the other mechanism that could put a player
under a deck — the client drawing them there, since a `0x22` carries no position.
Only steps the shard *allows* count: a refusal is a `0x21`, which carries z and
corrects the client.

| | |
|---|---|
| permitted steps off a bridge or pier | 224,950 |
| client and shard disagree at all | 77 (0.03%) |
| **client draws the body lower** | **0** |

**And a boat's deck could never have been the thing discarded**, which is what
retires `boats.md`'s version. The guard is inside `check`'s loop over the *map's*
statics; a moored ship is a `Cover` in the overlay, reached through `aboard` and
`climbed`, which never see it.

### `aboard` had no climb limit, and now has the same one as `climbed` ✅

`aboard` answers where the map refuses a tile — open water, mostly — and it took
the nearest live surface **at any height whatever**. `climbed`, reached when the
map *did* answer, bounds the climb by `MAX_STEP_UP`. So the two entrances to the
live layer applied different rules and which one a tile got was decided by
whether there was water under it: a house over the sea was boardable from the
shore at whatever storey happened to be nearest the body's feet.

`roadmap.md` filed this under R3 and doubted a reach filter could be the fix,
because `aboard` exists for a body stepping *down* from a mast. **The objection
does not hold**: `Cover::reach` of a flat surface is its own height, so
everything below the body passes at any value — the climb is bounded, the descent
is untouched. `boarding_from_open_water_obeys_the_climb_limit` asserts both
halves, and its control is the limit removed by hand, where it fails at exactly
the first assertion.

## What was decided

**`Overlay::surface_at` takes the reach as an argument.** How far a body may
climb is a *movement* rule — the same argument that keeps `SpanIndex` in
`openshard-movement` and out of `openshard-map` — so the map crate does not read
`MAX_STEP_UP`. What it keeps is the choice among what qualifies, which is the
half that must not be written twice. Its one production caller was `aboard`.

**`can_fit` stopped going through it.** It asks whether a surface sits at exactly
one height, which is a placement and not a step; it says so directly now. The two
spellings agree — a surface exactly at `z` is the unique nearest — but only one
says what is being asked.

**Two fixtures moved, and both had been asserting a boarding the step rule does
not permit.** `boats`'s deck stood three above its shore and `obstruct`'s five,
passing only because no limit was applied. Both are within a step now. What those
tests are *about* — the map refusing water, the deck overturning it, the hull
refusing again — is unchanged by the height.

**`boats.md`'s conclusion stands and its reason does not.** The `swimming` flag
stays off and B4 does not move. But the reason recorded there was a fall this
session measured at zero, so it is now marked as an open question rather than a
settled one — and the outcome is still reachable by a route the survey cannot
see: with swimming on, `check` answers water with its `land_center` instead of
refusing, so a body that cannot climb to the deck stands on the sea. **Not
measured**, needs the overlay.

**The pier report keeps its entry, and the entry now says where not to look.**
The 2026-08-02 report is real and its cause is unknown. Both surveys walk the
bare map with no overlay at all, which is what leaves the suspects below.

## What is clean

`cargo test --workspace`: **3,502 passed, 0 failed, 36 ignored.**
`cargo clippy --workspace --all-targets`: five findings, in three files, none
this session's — `uofiles/src/map.rs` (a needless borrow),
`render/tests/traced.rs` (three borrowed expressions), `client/app/src/link.rs`
(a 640-byte enum variant). `rustfmt` on every touched file.

## What is next — the backlog, ranked

Nothing here is a plan node; these are the entries the eras do not own. Ranked
by what a person would notice, then by what a session would enjoy.

### A player can see these

| | what would close it |
|---|---|
| **A mobile is not an obstacle.** Nothing registers a mobile in `Obstructions`, so a player walks through a standing NPC, a guard does not hold a doorway, and `find_path` plans through a crowd | The method is already chosen in the entry: ask `Sectors`, which is already the authority from tile to entity and already kept honest by the step itself — *not* a second copy of `Position`. Three rules come with it and none are in the code: the dead do not block, a mobile may always step *off* its own tile, and staff walk through bodies |
| **The world map draws at LOD 0** — 7,168 chunks of 8 KiB to fill a window where one pixel is eleven tiles, ~900 frames before it is full. Beside it: `RadarCache` never evicts (~75 MiB a run) and an O(facet) scan every frame | A coarse producer that samples `WorldMap` directly. The pyramid is reduce-only — a parent exists only once all four children do — so it cannot answer "the whole facet, cheaply" at all, and this is not a different LOD request |
| **The real cause of the pier report.** Three suspects left, in order: a boat moored at a pier reaching `aboard` (the overlay neither survey has); a multi-step drift, where each step is right and the sequence is not; arriving rather than walking — a login, spawn, gate or teleport, which reach `spawn_z` and not `check` | A shard-side sweep with a live overlay. The first suspect is the cheapest to try and the likeliest |
| **`aboard`'s neighbour: this shard has no plank.** A UO player does not walk aboard over the gunwale — they step on the plank and its `OnMoveOver` teleports them (`ServUO/Scripts/Multis/Boats/Plank.cs:136`). So whether a real sloop is boardable from a real shore is a question no test answers | `boats.md`'s own phase. Worth measuring first: what a shipped boat multi's deck actually stands at over water |

### Nobody has taken these decisions

| | what would settle it |
|---|---|
| **A platform of no thickness: the map and the overlay disagree.** `MapTerrain::is_obstructed` gives one a body from `base` to `base`, so it is in the way of anything below whose head passes the floor; `Cover::of_static` lays no blocking half at all. A cellar under a shipped floor and the same cellar under a built one answer differently | A decision, not a node. N2 and N3 could not settle it by construction — their oracles are "the answers did not change", and changing which reading wins *is* a change to the answers |
| **Do bodies block?** The client refuses to route through an NPC, the shard permits it, and both ends have said so in comments for as long as both indexes existed | Whoever owns "may I walk into somebody". The layer carries either answer unchanged. Note this is the *routing* half of the mobile-obstacle entry above |
| **The publish window** — a revision is visible before the rebake over its touched chunks finishes | A measurement of one region's rebuild against the 96 s whole-facet bake. Era S's, and the live publish will ask it first |
| **Why `swimming` may not be turned on**, now that the recorded reason is refuted | The unmeasured route above: a body that cannot climb to a deck standing on the sea under its own ship. Needs a survey with an overlay in it |

### The instruments lie, and the plans quote them

| | what would close it |
|---|---|
| **`step_cost`'s `expand` is a second copy of the diagonal flank rule**, and four of its rows go through it — including the *floor* and *all eight on one column* rows the baked-adjacency decision rests on. A change to the flank rule leaves the example measuring the old rule and passing | `steps_out_of` growing a seam the example can substitute into, so there is one flank rule and the harness borrows it |
| **A bench's default is a claim about the machine it was written on.** `step_cost --repeat` defaulted to five, which at load average 33 moved rows 30% run to run and published a reading twenty-five passes do not reproduce. **The other examples were not audited** | `coarse_bench` already does it right — it prints `repeat={}` in its header. `map_path_probe` has the flag and does not print it; `span_index` and `span_census` quote a bake time with no repeat at all, and the plans keep that time |
| **Nothing checks an intra-doc link.** Two `[`Ground::…`]` links to fields that had stopped existing survived every `cargo test`, `clippy` and CI run | Either the workspace lint table gains `rustdoc::broken_intra_doc_links` and `cargo doc` joins CI, or every `[`Type::member`]` here is prose with brackets round it |

### Waiting on a measurement, not on a decision

| | the gate |
|---|---|
| **The land's fourth byte is 29.4 MB of padding** — bigger than everything R4 saved | Not size: the land is read as a slice and the ground walk is the one part of this map whose cache behaviour was measured as *already good*. The ground walk of a widest-zoom frame, packed against unpacked. If the unpack costs more than the 25% it saves, the answer is no |
| **The packed four-byte static record** | The same gate, plus whether the statics are still on a hot path now spans exist |
| **`Sectors::nearby` is linear in a bucket**, and a castle's ~4,000 lockdowns sit in one or two. It is asked per NPC per tick by AI sight, and again by guards, pets, chat, area spells, quests and the broadcast audience | Split a bucket into mobiles and items and let the caller say which it means — almost every caller wants mobiles. AI sight then compares ten rows instead of four thousand. **This gets worse, not better, once the mobile-obstacle entry lands the recommended way**: that puts a second per-step reader on the same lookup |
| **The building flood's artifact is 112 MiB of raw `u32`**, overwhelmingly zero, read four bytes at a time on the startup path — 29 million bounds-checked reads | Run-length or a sparse per-block index |
| **The navigation bake spikes 235 MiB transiently** — `vec![None; cells]` of `Option<Point>` | A walkable bitset plus an `i8` height array is 33 MiB for the same information |
| **`navigation_graph_efficiency_plan.md`'s phase 3** — a second hierarchy level. The N4 gate is spent | Its own end-to-end p95 |
| **The node budget of 400 was measured in tiles**, and a node is a place to stand now, so the same 400 buys marginally less ground | The argument for 400 and 600 is a *time* budget, so the measurement that moves them is `terrain_seam.md`'s, not the span plan's |

### Structure, and none of it blocks anything

| | |
|---|---|
| **"The highest static on a tile" is re-derived by linear scan in four places** — the radar, `MapTerrain`, cutaway, occlusion. It cannot become a z-sort: file order *is* draw order. Our own chunk format should store draw order as a field and sort by z, which turns every scan into a suffix lookup |
| **`Obstructions` is not obstructions.** It holds a house's floors, which are the opposite; `is_blocked` had to become `holds_anything` for exactly that reason. It is the *identity* half of the overlay, and that is what it should be called. The rename touches every server crate |
| **`standing_on` walks the map's start surface a second time.** `map.can_step` computes `start_surface(from)` internally and throws it away. The honest fix is a signature change all three callers see |
| **`WorldMap::from_parts`' grouping is a contract with no oracle.** It catches the wrong *count* in a block, not the right count in the *wrong* block — which sorts items into the wrong span and is silently wrong forever. A debug-only check costs one pass at load |
| **A patch of many ops is quadratic in the facet.** `place_static` moves the tail of the whole 29.5 MiB run. Right for the one op a published patch usually is; wrong for a thousand, which is direction F's editor. Wants a publish that groups ops by block |
| **A `Scene` rebakes on every setter**, walking `land_kinds`'s 16,384 ids each time. Nothing in the suite is slow enough to notice yet |
| **`Resources::map()` borrows the whole struct** where a field borrowed itself. If a second caller needs `&mut` beside it, the answer is a free function over `&Resources::ground`, not another hoist |
| **A house's placement checks got stricter and nothing measured by how much.** `footprint_of` now returns every cover-laying component, so the road and flat-ground tests see a house's *interior* for the first time. A plot legal before and refused now reads to a player as a regression |
| **`a_creature_routes_past_its_exact_budget_over_the_coarse_graph` is load-sensitive by construction** — `MAX_LONG_PATH_TIME` is 50 ms of wall clock and one miss anywhere along the walk ends it far from the goal. A deadline the caller names would take a clock out of an assertion |
| **`openshard-movement`'s `lib.rs` is eight `pub use` lines**, re-exporting its own private modules |
| **Five clippy findings in three files**, none ours, listed under *What is clean* |

## Where a session starts

**Either the top of that first table, or era S.** They are independent: the live
publish is a *plan* node with a design question in front of it — who calls
`MapSnapshot::publish` and where in the tick — and the backlog above is repair
work that needs no plan at all.

If it is the backlog: **the mobile obstacle is the one a player notices first**,
its method is already chosen, and it drags `Sectors::nearby` in behind it — which
is the one performance entry here with a caller-side reason to happen now rather
than later.

**What would block it:** nothing.
