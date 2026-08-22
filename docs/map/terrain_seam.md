# Six terrains, and one of them is a terrain

The question that produced this document was *why is there a trait here at all,
rather than an explicit reference?* — and the answer is that there should not
be one. `Terrain` has six implementors outside a test, and **five of them are
not terrains**. They are actions taken over one: a mask of what the live world
put in the way, a rectangle to stay inside, a memo table, and the absence of a
map. Each was made a *kind of terrain* because the seam was a trait, and each
one being a kind of terrain is then the argument for the seam being a trait.

The other half of the trait was not terrain either. Six of its fifteen methods
were a client-file lookup table wearing a terrain's coat: `item_weight(graphic)`
takes no coordinate, reads no cell, and cannot be changed by a placed crate.
**Those six are gone** — node B below — and the trait is nine map questions.

So this is not a plan to swap `dyn` for a generic. It is the plan to end up
with `find_path(&MapTerrain, &Overlay, Doors)` — explicit types, imported by
name — and no `Terrain` trait on the search at all. **Whoever needs the map
takes the map. Whoever needs the table takes the table.** The result is a
directed graph with no seam in the middle of it, and the collapse is ordered by
that graph rather than by phases: [what has no incoming
edge](#what-has-no-incoming-edge) is where a session starts.

Track: [`README.md`](README.md) · The map's owner:
[`new_map_representation/snapshot.md`](new_map_representation/snapshot.md) ·
The routing it feeds:
[`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md)

## The six, and what each one actually is

Read off the workspace, not remembered. `impl Terrain for` appeared at **37
sites under 27 distinct names** when this was written; 31 of those sites were
test doubles. B and C between them took it to **17 sites under 16 names**, and D
to **15 under 14** — the last two doubles were `openshard-state`'s own, and
`LiveTerrain` taking a named map is what stopped them compiling. Every one left
in a test is local to the crate whose own rule it exercises, and none is handed
to a `FacetState`, which no longer has anywhere to put one. These six are the
rest:

| | | |
|---|---|---|
| [`MapTerrain`](../../crates/common/movement/src/terrain.rs#L64) | the map and `tiledata.mul` | **a terrain** |
| [`Cluttered`](../../crates/client/app/src/clutter.rs#L285) | the client's live items over it | a **mask** |
| [`LiveTerrain`](../../crates/server/state/src/obstruct.rs#L140) | the server's live items over it | the **same mask** |
| [`CachedTerrain`](../../crates/common/movement/src/cache.rs#L30) | memoises `can_step` for one query | a **memo table** |
| [`InRegion`](../../crates/common/movement/src/navigation.rs#L81) | three lines: refuse a step leaving a rectangle | a **parameter** |
| [`OpenWorld`](../../crates/common/movement/src/walk.rs#L211) | `can_step` returns `Some(to)` | the **absence** of a map |

`InRegion` is the clearest case, because it is short enough to quote whole:

```rust
fn can_step(&self, from: Point, to: Point) -> Option<Point> {
    (self.region.contains(from) && self.region.contains(to))
        .then(|| self.terrain.can_step(from, to))
        .flatten()
}
```

That is a bounding box the graph builder wants the search to respect. Making it
a terrain means every `can_step` on that search pays a vtable hop to ask a
rectangle a question the search could have been told once.

### A decorator that forgets a method is the bug this seam ships

It has already happened once, on the side that matters. `LiveTerrain`'s own
comment records it — *"Every one of these used to fall through to the trait's
default, so a caller holding a `LiveTerrain` was told that nothing blocks,
everything is flat, no static has a name or a weight, and no multi exists"*
([obstruct.rs:296](../../crates/server/state/src/obstruct.rs#L296)). It was
fixed by writing seven forwarding methods by hand.

**The client had it too, and B is what removed it.** `Cluttered` forwarded
thirteen of the fifteen; `multi_components` and `land_is_water` fell into the
trait's defaults, so a caller asking that overlay about a multi was told there
was none. Nothing on that end asked yet — the same sentence the server's comment
uses about its own version — so it was latent rather than live.

Two further silent holes were found in the same reading, and left with the same
change: [`Resources`](../../crates/client/app/src/resources.rs#L78) holds
`multis: Option<Arc<Multis>>`, and **not one of the client's four
`MapTerrain::new` sites called `with_multis`** — only `boot.rs` did. So a client
`Terrain` answered "no such multi" twice over, by two independent omissions
neither of which a compiler could see.

Six of the fifteen no longer exist, so six of the fifteen can no longer be
forgotten. The remaining nine still can: `Cluttered` writes eight forwards by
hand and skips `land_is_water`, which is the same shape of hole one method
wide. **Only E closes the class**, by making the overlay a value nobody has to
forward at all.

## The two that are one

The client's and the server's live overlays are the same structure written
twice:

```rust
// client/app/src/clutter.rs                 // server/state/src/obstruct.rs
struct Blocker {                             pub struct Obstacle {
    z: i8,                                       pub entity: EntityId,
    height: TileHeight,                          pub door: bool,
    door: bool,                                  pub z: i8,
}                                                pub height: u8,
struct Clutter {                             }
    tiles: HashMap<Tile, Vec<Blocker>>,      pub struct Obstructions {
}                                                tiles: HashMap<(u16, u16), Vec<Obstacle>>,
                                             }
```

[`clutter.rs`](../../crates/client/app/src/clutter.rs)'s own header already
says so — *"this is the client's half of `Obstructions`"* — and gives the
reason the two must agree: same predicate (`item_blocks`), same z-span, *"so
the two ends agree by construction rather than by resemblance"*. They are
agreeing by resemblance.

### And they do not compute the same set

Which is the thing a shared type has to reconcile, and it is more than
`EntityId`:

| | client `Clutter` | server `Obstructions` |
|---|---|---|
| mobiles | **in the index**, `PLAYER_HEIGHT` tall | not in it at all |
| identity | none — rebuilt whole from the view | `EntityId`, because `block` is idempotent per entity *and* z |
| key | `Tile` | `(u16, u16)` |
| height | `TileHeight` newtype, `span()` applies `max(1)` | raw `u8`, `max(1)` applied at the read |
| body height | `openshard_movement::PLAYER_HEIGHT` | a private `MOBILE_HEIGHT: i32 = 16` copied by hand |
| doors-open | [`enum Doors`](../../crates/client/app/src/clutter.rs#L277) on the view | `through_doors: bool` on `LiveTerrain` |
| sight | door opacity **not** applied (owed, and said so) | shut door is opaque |
| surfaces | none | `aboard` — a deck over water |

Two of these decide phase work rather than decorate it. **Mobiles** mean
"both ends build the same overlay" is false as stated and has to become "the
same *items* produce the same blockers". **`Doors`** is not already a `bool` on
both sides, and clutter.rs argues against making it one: *"A `bool` would do
and is exactly what this must not be: the two are read at call sites four
modules apart"*. So the flag the search is told is that enum, and the server's
`bool` becomes it — not the other way round.

The rest are a merge, not a design: `MOBILE_HEIGHT` is a hand-kept copy of a
constant the shared crate already exports and leaves with the move.

## What each caller actually wants

The census the collapse was planned from, taken **before B**. `FacetState.terrain`
was read at **26 production sites in 16 files** — 27 with `runtime.rs`'s own
design-detail encoder, which the first grep missed — and what they asked for
split four ways:

| what it wants | who |
|---|---|
| **the table only** — a graphic's weight, layer, name, components | `items/weight` (×2), `items/capacity`, `items/backpack`, `items/equip`, `skills/appraise` (×2), `tick/speech`, `housing/design` |
| **the map only** — statics, land, a height, a fit | `crafting/environment`, `skills/harvest`, `tick/decor` (×3), `tick/spawners`, `npc/spawn` |
| **both** | `housing/lib` (×4), `boats/lib` (×3), `world/gm` (×2) |
| **neither** — only whether a map exists | [`tick/travel.rs:220`](../../crates/server/world/src/tick/travel.rs#L220), which calls `terrain.is_none()` |

Seven files out of sixteen had never wanted a floor. One wanted a boolean.

**B settled the first row and half the third.** Those callers read
`WorldState::tiles` and `WorldState::multi_components` now, and nine of them
stopped taking a `facet` or a `near: EntityId` they only ever used to walk to
the table. What was left on `FacetState.terrain` was the map, and D made the
field say so: it is `FacetState.map`, a `MapSnapshot`.

### The dependency argument for the seam is already spent

The third group travels through `Terrain` because `server/items`,
`server/crafting`, `server/npc` and `server/skills` do not *name*
`openshard-uofiles`. They all depend on `openshard-movement` **and**
`openshard-state`, and both of those depend on `openshard-uofiles` — so the
crate is already in every one of their build graphs, and naming it directly
adds nothing at all.

`openshard-movement`, where the trait itself lives, is the same story: it
depends on `openshard-uofiles` and returns `openshard_uofiles::multi::Component`
straight out of a trait method. The trait's own doc claims *"this crate should
know about none of them"*
([walk.rs:39](../../crates/common/movement/src/walk.rs#L39)). It has not been
true for some time.

**There is no layer here to protect.** What the table half needs is not a
decision about layering; it is an import.

## The fifteen methods, split three ways

| | | |
|---|---|---|
| **the table** — a graphic, no coordinate, no cell | `item_blocks`, `item_height`, `item_weight`, `item_layer`, `item_name`, `multi_components` | **6** |
| **the map** — a coordinate, and the map answers | `can_step`, `ground_z`, `land_tile`, `statics_at`, `stand_z`, `spawn_z`, `can_fit`, `land_is_water`, `sight_clear` | **9** |
| of those, **the ones an overlay changes** | `can_step`, `can_fit`, `sight_clear` | **3** |

The third row is the one that sizes `Overlay`. `LiveTerrain` overrides exactly
those three (and `spawn_z`, which is only `None`-handling); `Cluttered`
overrides two. Everything else in both is forwarding. So the shared overlay
answers *does something block this step*, *does a body fit here*, *is sight
blocked* — plus **surfaces**, which is the one thing a mask cannot express:
[`LiveTerrain::aboard`](../../crates/server/state/src/obstruct.rs#L183)
resolves a step onto a moored ship's deck over water the map calls unstandable.
`docs/boats.md`'s B3 already argued it — the hull stays out of `Obstructions`
*because an index that only subtracts cannot say "there is somewhere to stand
here"*.

The pathfinder uses two of the fifteen.
[`path.rs`](../../crates/common/movement/src/path.rs) never calls the trait at
all — it goes through `step_allowed`, which calls `can_step`; `navigation.rs`
adds `ground_z`.

The cost of the other thirteen is visible without looking for it.
`CachedTerrain` memoises one method and is obliged to forward fourteen.

## What the shape becomes

```rust
find_path(terrain: &MapTerrain<'_>, over: &Overlay, doors: Doors, ...)
```

Both types named and imported. What happens to the other five:

| | |
|---|---|
| `Cluttered` / `LiveTerrain` | one `Overlay` in `common/movement`; both ends **build** it instead of implementing a trait |
| `Doors` | an argument to the search — the client's enum, not the server's bool |
| `InRegion` | an `Option<Region>` bound the search is told once |
| `CachedTerrain` | **deleted.** An earlier draft moved it *inside* `search`, on the reasoning that its lifetime was already one query. [0 measured it](#-cachedterrain-costs-about-5-and-buys-nothing): 50.6% hit rate, ~5% slower than the calls it memoises |
| `OpenWorld` | an empty overlay over no map — `Option<MapTerrain<'_>>`, which `LiveTerrain` carries since D |

### `MapTerrain` is three borrows, and both ends already hold them ✅

**Done in D**, and one field smaller than this section predicted — B had already
taken `multis` off the terrain, so what landed is two borrows and a flag.

`MapTerrain` used to be generic over ownership — `M: AsRef<WorldMap>, T:
AsRef<TileData>` — so the server could own its map and the client borrow one.
That parameterisation bought one thing: a terrain with no lifetime, so it could
sit in a `Box` in a struct field. Nothing else asked for it.

And both ends already owned precisely its fields:

| | server | client |
|---|---|---|
| map | `MapSnapshot`, [`boot.rs:660`](../../crates/server/server/src/boot.rs#L660) | `Resources.map: MapSnapshot` |
| tiledata | `Arc<TileData>` | `Resources.tiledata: Arc<TileData>` |
| ~~multis~~ | ~~`Option<Arc<Multis>>`~~ — B removed the question | ~~`Resources.multis`~~ |

So `MapTerrain<'a> { map: &'a WorldMap, tiles: &'a TileData, swimming: bool }`
is buildable on either end from fields that already exist, costs two pointers to
construct per query, and makes the type **one name that can be imported** rather
than two unrelated instantiations. The `AsRef` bound did not survive it, and
neither did the three `AsRef` impls that existed only to satisfy it.

### `swimming` is a property of a body, parked on a world ✅

**Resolved in D by moving what it sits on**, not by moving the field. See
[D's own section](#swimming-is-an-argument-now-because-the-thing-it-sits-on-is-the-query).

The finding was that [`MapTerrain::swimming`](../../crates/common/movement/src/terrain.rs)
is set **nowhere in production** — the only caller was `load_client(true)` in
terrain.rs's own tests. It is a per-creature fact (*a boat or a fish says yes*)
that was living as a field on the per-facet world, which is why no shipping
caller could turn it on: there was one terrain per facet, and turning it on
turned it on for everyone standing there.

A `MapTerrain` is built per question now, so the field is on the query and
`.swimming(true)` scopes to the one asker — which is what "an argument beside
`Doors`" was asking for. Still unused in production, because the shard has no
swimmer yet; no longer unusable, which was the defect.

## What has no incoming edge

The collapse is a graph, not a sequence. **0, A, B, C and D are done, and every
edge into E is met** — it is the last node of the seam and nothing is waiting on
anything:

```
 A. Scene grows what the doubles need ✅ ─┐
                                          ├─> C. the doubles become Scenes ✅ ─┐
 B. the table leaves the trait ✅ ────────┴────────────────────────────────────┼─> D. FacetState holds data ✅
                                                                               │      MapTerrain is borrows
 0. the facet-0 oracle ✅ ─────────────────────────────────────────────────────┴─> E. Overlay, and the search
                                                                                      takes explicit types
 F. the coarse graph nobody reads ─── needs the one-storey defect 0 found
```

F had no edges at all when this was written. It has one now, and 0 is what put
it there: the graph the server would be wired to
[disagrees with its own map on raised ground](#-the-coarse-graph-is-a-one-storey-model-of-a-two-storey-world).

`Box<dyn Terrain>` **was** held up in `FacetState` by tests rather than by
production: fifteen `= Some(Box::new(...))` substitutions across four files when
this was written, against exactly one production construction at
[`boot.rs:664`](../../crates/server/server/src/boot.rs#L664). That was the edge
`A → C → D`, and it is why D could not be the first commit however mechanical it
looked. B deleted four of the doubles and C converted the other eight, so by the
time D ran there was a `MapTerrain` at every site and the box was all that was
left to take away.

### Following the compiler works for B and D, and not for E

Deleting a method from a trait and letting `cargo check` enumerate the wreckage
is the right tool for the table (B) and for `FacetState` (D): every site there
is a one-line substitution with a mechanically obvious replacement, and there
are 26 of them. D bore this out — the whole node was compiler-led, and the two
things it turned up that the plan had not predicted (`can_fit`'s two shapes, and
`openshard-state`'s own last two doubles) were both *found by the compiler*
rather than by reading.

It is the wrong tool for `Overlay` (E). Removing `can_step` from a trait points
at 38 impl sites and says nothing about the four decisions listed under [the
two that are one](#and-they-do-not-compute-the-same-set) — mobiles, identity,
door opacity, decks. Those are answered by reading, and E is where they are
answered.

## A — `Scene` grows what the doubles need ✅

**Done.** No incoming edges, and pure addition: nothing broke while it landed.

[`Scene`](../../crates/common/movement/src/scene.rs) builds a real
`MapTerrain` from hand-placed ground, floors, stairs and walls with no client
files, and its own header is the argument for using it: *"A fixture that
reimplemented the rule would agree with itself and prove nothing."* Most of the
doubles it replaces answer `can_step` with `Some(to)`.

What it grew, read off what the doubles actually do:

- **A size.** `Scene::flat_over(BlockExtent, z)` builds as many blocks as asked
  and `Scene::flat_holding(x, y, z)` builds enough to hold one coordinate, which
  is the size a fixture actually knows — (10, 10) for a house, (102, 100) for a
  door frame. `Scene::flat` is one block still, and `picture` draws whatever the
  scene turned out to be.
- **A named graphic.** `Scene::art(graphic, flags, height)` declares what an id
  *is* and `Scene::put` places copies of it. The mint-an-id path is untouched
  and still right for geometry; identity is what `Shop`, `harvest`'s `Ground`
  and `FrameTerrain` need, because a domain table matches the id.
- **A land id, and what it can do.** `Scene::land`, `Scene::land_everywhere` and
  `Scene::land_art` — the last one needed a new
  [`TileData::set_land_tile`](../../crates/common/uofiles/src/tiledata.rs),
  `set_static_tile`'s missing other half. **That absence is why three doubles
  exist**: water and impassable ground are flags on a land row, so a fixture
  that could not write one had to override `land_is_water` or `can_fit`
  instead — a fixture agreeing with itself, which is the thing this whole seam
  is against.
- **An owned terrain.** `Scene::into_terrain` hands out a
  `MapTerrain<WorldMap, Arc<TileData>>`, which has no lifetime and therefore
  fits in `FacetState`'s box; `Scene::into_shard` hands out that *and* the
  `Arc`, so the shard's `WorldState.tiles` and the ground under it are one
  table. A fixture that built them separately could stand a house on a wall the
  ground had never heard of.

The multi table stayed out, and B is why: a multi is no longer asked of the
ground at all. A fixture that needs one writes `Multis::of` into `state.multis`,
which is where the shard keeps it — the same two lines it already writes.

Six tests pin the new abilities, each through the real rule rather than through
the fixture: the far coordinate is on the map and the one past the corner is
not; a named graphic comes back under its own id and blocks; a road id is what
`land_tile` answers; water is a flag that only a swimmer stands on; ground
flagged impassable leaves a raised floor as the only surface, out of a step's
reach and inside a placement's; and `into_shard`'s two holders answer alike.

One latent defect fell out: `Scene::ground` wrote `LandTile(0)` back while
moving a tile's height, so naming ground and then sloping it would silently
un-name it. Height and identity are two facts about one cell now.

**Done when** ~~a `Scene` can express what each of the seven remaining doubles
was written to say~~ — it can. `Ground` in two shapes is land ids plus a static;
`Sea` in two is a water flag with a shore row, swimming where the double allowed
every step; `Shop` is `art`/`put` over the square; `FrameTerrain` is two frames
at their real coordinates with a wall for `walled`; `RaisedFloorTerrain` is the
test above outright; `BlindTerrain` is a wall between two people. Doing it is C.

## B — the table leaves the trait ✅

**Done.** No incoming edges, and no benchmark: nothing here was on the A\* edge.
The table's callers are picking an item up, placing decoration, labelling a
click.

What landed, and where it differs from what this section planned:

- `WorldState` holds `tiles: Option<Arc<TileData>>` and
  `multis: Option<Arc<Multis>>` — **one pair for the shard, not one per facet**,
  which is what they always were. `boot.rs` used to *clone the whole tile table
  into every facet's terrain*; it now reads it once behind an `Arc`.
- The two methods with a rule in them moved to the file's own reader:
  `TileData::item_weight` (tiledata's `255` is *immovable*, so it weighs
  nothing) and `TileData::item_name` (`"NoName"` and the empty pad mean *no
  name*). `Multis::components` answers the third. The other four are field
  reads at the call site, which is what `Clutter::of` already did.
- **Nine call sites stopped asking which facet an item was on.** `item_weight`,
  `item_layer` and `item_name` were reached through `facet_of(entity)` →
  `facets[facet]` → `terrain` → the table, so five functions carried a `facet`
  or a `near: EntityId` parameter that existed only to complete that walk.
  `sign_spot`, `tiles_of`, `footprint_of`, `doorstep`, `initial_foundation`,
  `planks_of`, `appraise::item_name` and `tiledata_layer` all lost one.
- `MapTerrain` lost its `multis` field and `with_multis`: nothing asks a
  *terrain* what a house is made of any more. It is map, tiledata and
  `swimming` — which is exactly what its name claims.
- Eight test doubles stopped answering table questions. Five of them
  (`housing`'s and `boats`' `Ground`/`Sea`, `world`'s three) now hand the state
  a real `TileData` and a real `Multis`, built by hand — `TileData::empty()`
  plus `set_static_tile`, `Multis::of`. `NamedTerrain` is gone outright.
- Two housing tests that said "a shard with no client files" expressed it by
  clearing `FacetState::terrain`. They set `state.multis` and `state.tiles` to
  empty tables now, which is what they meant — and the fact that those were two
  different clearings is the defect this node removes.

- Four of the fifteen boxed doubles became **exactly the trait defaults** and
  were deleted rather than rewritten: once `item_blocks` and friends were gone,
  all they said was `can_step -> Some(to)` and `can_fit -> true`, which is what
  `FacetState::terrain = None` already answers through `OpenWorld`. Seven boxed
  substitutions are left, and `impl Terrain for` is down from 38 sites to 33.

`cargo test --workspace` and `cargo fmt --all` are clean, and `clippy` warns in
the same ten places it warned in before.

### One `Arc` is gone and one is not, and the difference is the box

**Both are gone now — D took the second one.** Kept here because the *reason*
the second one existed is the clearest single statement of what the box was
costing, and it was written before the box came out.

`multis` was owned outright from B: after `MapTerrain` lost its copy,
`WorldState` was the only thing on the shard holding a multi table, so there was
nothing to share it with and the `Arc` was vestigial the moment it was written.

`tiles` kept one, and it had **exactly one other holder**: the
`MapTerrain<MapSnapshot, Arc<TileData>>` boxed inside every `FacetState`. That
box was the whole reason. `MapTerrain`'s `AsRef` parameters existed so it could
own its inputs and therefore have no lifetime and therefore fit in a `Box` in a
struct field — and the `Arc` was what stopped that ownership from being a copy
of the table per facet.

So the `Arc` was not a decision anyone took about tiledata. **It was D's price,
paid in advance.** `FacetState` holds a `MapSnapshot`, `WorldState` owns the
`TileData`, and a `MapTerrain<'_>` built per query borrows both out of the same
`&WorldState` — no self-reference, no sharing, no `Arc`, and no `AsRef` bound
either. Worth naming, because "why is there an `Arc` here" had a better answer
than "because two things hold it", and the better answer is what predicted
which line to delete.

### What it was, for the record

`item_blocks`, `item_height`, `item_weight`, `item_layer`, `item_name` and
`multi_components` come off `Terrain`. Five of the six are one-liners over
`TileData::static_tile`; the two with policy in them (`item_weight`'s
`255 => 0`, `item_name`'s `"NoName" => None`) keep it, wherever they land.

The consumers name `openshard-uofiles` directly, because [it is already in
their build graph](#the-dependency-argument-for-the-seam-is-already-spent) and
minting a seam instead would be inventing a problem.

What this alone removes: six methods from the trait, and therefore **eighteen
forwarding method bodies** across `CachedTerrain`, `Cluttered` and
`LiveTerrain` — including the two `Cluttered` never wrote.

**Done when:** no caller reaches a client-file table through `Terrain`, and the
trait is nine methods.

## C — the doubles become Scenes ✅

**Done.** All eight replaced by a `Scene` that builds a real `MapTerrain` through
[`Scene::into_shard`](#a--scene-grows-what-the-doubles-need-) — `boats`' and
`persistence_tests`' `Sea`, `harvest_tests`' and `housing`'s `Ground`,
`crafting_tests`' `Shop`, and `tick/tests.rs`'s `FrameTerrain` and
`RaisedFloorTerrain`. `BlindTerrain` has no replacement, because the rule it
stood for cannot exist; see below.

`grep -rn "\.terrain = " crates` is six sites and every one of them is either a
`MapTerrain` out of a scene or `None`. The `Box` around them is all that is left,
and it is D's.

It was worth doing on its own evidence, which is the count: **one engine defect
fixed, one test deleted as fiction, and three silent holes named.** A double that
answers `can_step` with `Some(to)` is a test that proves the caller compiles.

### What they cost, and what they were hiding

**A double answers questions its subject never asks, and being wrong there is
free.** `boats`' `Sea` had four methods; `check_berth` reads one of them.
`land_tile` and `can_fit` were dead, which is why the disagreement this document
predicted — a real `MapTerrain::can_fit` says *true* over water where the double
said `y == 0` — turned out to be unobservable. That is the seam's defect in a
third form: not a decorator that forgot a method
([`LiveTerrain`](#a-decorator-that-forgets-a-method-is-the-bug-this-seam-ships)),
not a decorator that inherited a wrong default (`Cluttered`), but a double
answering into the void. The count that matters is not "how many methods does
the double implement" but "how many does its caller reach".

**A double can hold a world that cannot exist.** `persistence_tests`' `Sea` said
every tile is water *and* every step is allowed. Water is a surface only a
swimmer stands on, so as a scene it needs a jetty at `START` for the character
who launches the ship — the contradiction becomes a line of fixture instead of a
rule quietly overridden.

**🚩 `housing::check_ground` refused every house with an upper storey, and the
double is why nobody knew.** `can_fit` requires a *surface* at the z it is asked
about, and `check_ground` asked it at each component's own z — so a wall twenty
units up is standing on thin air and the whole placement is `BadGround`. Every
villa, keep and two-storey shop, on any real map, everywhere. The fixture
answered `can_fit` with a boolean the test set to `true`, so the check had never
once been run against ground. ServUO gates the same question the same way — its
`hasSurface` is only ever set for a component at `addTile.Z == 0`
([`HousePlacement.cs:174`](/home/sc/t/ServUO/Scripts/Multis/HousePlacement.cs)) —
and `check_region` four lines above already carried the doctrine in its own
header: *"the house's `z`, once, and never the component's"*. Fixed, with two
tests that fail without it.

**Left behind by that fix:** ServUO's rule two for the *upper* components — a
roof driven into a hillside over a tile the house has no ground-level wall on.
`can_fit` at the house's z covers every tile that does have one. Closing the
rest needs a terrain question that is *"is anything in this body"* without
*"and is there a surface"*, which `MapTerrain::is_obstructed` already is and the
`Terrain` trait does not carry. **Do not add it to the trait** — E is where
`check_ground` stops going through one.

**A facet-sized scene is free if you pave it by id.** `Scene::flat_holding`
lays land id `0` everywhere, so `land_art(0, flags)` makes the whole facet water
or void with no pass over its cells. `land_everywhere` costs a pass and is still
cheap — thirteen harvest tests over a 1416×1656 scene run in 0.41 s — but the id
trick is the one to reach for when the whole square is one kind of ground.

### 🚩 `BlindTerrain` stood for a rule that cannot exist

The last double had no replacement, and finding out why is the clearest single
argument in this document for what a real fixture buys.

[`line_tiles`](../../crates/common/movement/src/walk.rs#L175) returns the tiles
strictly *between* two points. **Between neighbours there are none**, so no map
can make a sight line between adjacent tiles anything but clear — a wall is a
whole tile, and standing behind one puts you two tiles away.

Two tests said otherwise, by holding a `sight_clear` that answered `false` from
anywhere to anywhere:

- `a_vendor_behind_a_wall_will_not_sell` is a real rule with the wrong geometry.
  `TRADE_RANGE` is 4, so the vendor moves two tiles out with a wall on the tile
  between, and the test now fails without that wall — which it did not before,
  because before it was asserting that its own double returned `false`.
- `no_melee_swing_through_an_adjacent_wall` is **deleted**. `MELEE_RANGE` is 1,
  so there is no geometry that makes it true, and the gate it claimed to
  exercise — [`combat/src/lib.rs:791`](../../crates/server/combat/src/lib.rs#L791),
  under a comment reading *"Adjacent tiles can still be separated by a closed
  door or wall"* — **cannot fire**. The check is left standing; the comment is
  wrong about why.

**And the hole underneath both: a sight line has no height.** `sight_clear`
walks the tiles between and reads the statics on them, so two mobiles on the
*same* tile at different z — one on a shop's ground floor and one on the storey
above — see each other through the floor. Sphere reads the platform bit in its
own LoS for exactly this case, and this port reads it only on the tiles it
crosses. That is a real defect with a player-visible consequence (buy from the
vendor upstairs, shoot the one downstairs), and it is what would make the melee
gate above live. It needs the endpoints' own columns examined, which is a change
to what a sight line *is* rather than to who asks for one — so it is filed here
rather than fixed in passing.

## D — `FacetState` holds data, `MapTerrain` holds borrows ✅

**Done.** `grep -rn "dyn Terrain" crates/server` is empty, and no facet holds a
terrain at all: [`FacetState`](../../crates/server/state/src/runtime.rs#L386) is
`map: Option<MapSnapshot>`, the shard owns the tile table outright, and a
`MapTerrain<'_>` is three words built at the question and dropped after it.

The field used to be a `Box<dyn Terrain + Send + Sync>` whose doc said the crate
*"sits below the client-file parsers"* — untrue since `openshard-state` gained
its `openshard-uofiles` dependency for
[`customisation.md`](../customisation.md)'s C1. What the box was really buying is
in [the `Arc` section](#one-arc-is-gone-and-one-is-not-and-the-difference-is-the-box):
a type with no lifetime, at the price of an `AsRef` bound, an `Arc`, and a
trait object in the middle of every step.

What landed, in the order the field's three parts came apart:

- **`MapTerrain<'a>` is `{ map: &'a WorldMap, tiles: &'a TileData, swimming }`,
  `Copy`,** and its `AsRef` parameters are gone. **Three `AsRef` impls went with
  them** — `WorldMap for WorldMap`, `TileData for TileData`, and
  `MapSnapshot`'s, whose own doc said it existed *"because `MapTerrain<M>` is
  already generic over `M: AsRef<WorldMap>`"*. Nothing else in the workspace was
  using any of the three: the bound was their only consumer, so the bound going
  took the impls with it.
- **The accessors moved from `FacetState` to `WorldState`**, because that is
  where the other half is. `state.facet_state(facet).live_terrain()` is
  `state.live_terrain(facet)`, and the new
  [`WorldState::map_terrain(facet)`](../../crates/server/state/src/runtime.rs)
  is what "the bare map, if this facet has one" now spells — nine call sites
  that used to write `.terrain.as_deref()` or an `and_then` chain over the
  option.
- **`WorldState.tiles` is a `TileData`.** The `Arc` had exactly one other
  holder, the box, and went with it. `boot.rs` reads the file once and moves it
  in; the log line that reported the format reads it before the move rather
  than after.
- **`LiveTerrain` holds an `Option<MapTerrain<'_>>`** instead of an
  `Option<&dyn Terrain>` — the same nine hand-written forwards, over a named
  type.
- **`travel.rs`'s `terrain.is_none()` is `map_terrain(facet).is_none()`.** It
  was asking whether there is ground to have an opinion about, and now says so.
- **`Scene::into_shard(facet)` hands out `(MapSnapshot, TileData)`** — what a
  shard actually holds. `Scene::into_terrain` is deleted: its only caller was
  `into_shard` itself, and a scene has nothing to own a terrain *for* any more.
- **`World::with_terrain` is `World::with_map`**, and `with_facet` takes a
  snapshot with a `debug_assert` that it is the facet it is being filed under.

### `swimming` is an argument now, because the thing it sits on is the query

The [section above](#swimming-is-a-property-of-a-body-parked-on-a-world) asked
for one of two outcomes: an argument of the query, or deletion. It is the first,
and the change that made it so is `MapTerrain` no longer being stored anywhere.

The complaint was never the field — it was *whose* field it was: one terrain per
facet meant turning swimming on turned it on for everyone on that facet. A
`MapTerrain` is now built per question out of a `&WorldState`, so `.swimming(true)`
on it is scoped to exactly the one asker, which is what "an argument beside
`Doors`" was asking for. `WorldState::map_terrain` hands out the walker's view;
a caller with a fish asks the fish's.

It is still dead in production — nothing on the shard has a swimmer to ask for
yet — but it is no longer *unusable*, which is what the original finding was
about. `terrain.rs`'s fixture now hands out both views of one install
(`Install::terrain` and `Install::swimming`), and the water test asks the same
map twice rather than loading it twice.

### Two things fell out that were not in the plan

**`MapTerrain::can_fit` had two shapes, and the inherent one shadowed the
trait's.** The inherent took `(x, y, z, height)` and the trait `(tile, z,
height)`, same name, different arity — so a caller with a concrete `MapTerrain`
silently reached a *different function* than the same line reached through the
trait. Two callers had already worked around it without naming it:
`picking_query.rs` writes `openshard_movement::Terrain::can_fit(..)` in full,
and `terrain.rs` wrote `MapTerrain::can_fit(self, tile.x, tile.y, ..)` inside
its own trait impl. Both now have one shape, and D made this urgent rather than
tidy: every caller that used to hold a `&dyn Terrain` now holds a `MapTerrain`,
which is exactly the switch that changes which one they get.

**`openshard-state`'s last two test doubles became scenes.** `LiveTerrain` takes
a named map, so `Charted` and `Sea` could not stay — and `Sea` was the last
`can_step` that answered from an integer comparison rather than from a map. The
`boat_step_cost` measurement recorded in that file was taken *over that double*,
and its own doc says so: *"`Sea::can_step` below is a single integer comparison,
so the boat lookup is very nearly the whole of the measured work."* That caveat
is now obsolete and the recorded 15ns/55ns numbers are stale — the baseline is a
real `MapTerrain::can_step` reading a real map. **The re-run is owed**, and it is
one `--ignored` test.

**Done when:** ~~`grep -rn "dyn Terrain" crates/server` is empty, every one of
the remaining sites names what it takes, and the `Arc` around the tile table is
gone with the box that needed it~~ — all three.

## 0 — the oracle ✅

**Done.** Both probes have a committed facet-0 run, and the numbers are below.
They were asked two questions — *what does a search cost* and *is the hierarchy
worth it* — and answered a third nobody asked: **the coarse graph is a
one-storey model of a two-storey world**, and from a raised place it refuses
routes to ground a body can walk to.

Neither probe could answer as it stood. `map_path_probe` reported a mean and a
maximum over one pass, with no node counts and no classes; `coarse_bench` had no
map in it at all — it was a 1024×1024 open plain, which is the one world where a
coarse graph can only lose. What both became is [below](#what-the-probes-had-to-become).

### What one search costs

Three origins on Felucca, each a 193×193 square of destinations around it —
37,248 per sweep, every tile in reach asked for as a destination, which is what
click-to-walk actually does. Each search is repeated three times and the fastest
kept, and the bare and cached readings of one destination are taken back to
back. **Node counts came back bit-identical between runs; wall clock did not**,
which is why the timings here are minima and why the nodes are the currency to
argue in.

| origin | | arrived @400 | arrived @600 | p50 @600 | p95 @600 | worst @600 |
|---|---|---|---|---|---|---|
| (1363, 1600, **30**) | Britain, the castle plateau | 4,036 (10.8%) | 4,436 (11.9%) | 0.793 ms | 1.011 ms | 4.790 ms |
| (1434, 1699, 2) | Britain, the bank — a dense street | 6,138 (16.5%) | 7,389 (19.8%) | 0.851 ms | 1.111 ms | 6.374 ms |
| (1500, 1900, 0) | open country south-east of it | 17,458 (46.9%) | 18,093 (48.6%) | 0.570 ms | 1.067 ms | 6.482 ms |

**The budget is what says no, not the map.** Four destinations in five are
refused from a town street and one in two from open ground, and every one of
those refusals is a `Budget` exit — the open list still had somewhere to go.
`Exhausted` — the map itself running out — does not appear once in 111,744
destinations. And the budget buys badly: +50% nodes is **+9.9%** more arrivals
at the castle, +20.4% at the bank, **+3.6%** in the country. Half again as much
searching, for a twentieth more answers on open ground.

Per class, at budget 600 from the bank (`n`, p50 ms, nodes p50/p95):

| class | n | p50 | nodes p50 | nodes p95 |
|---|---|---|---|---|
| `goal/near` (≤8 tiles) | 271 | 0.006 ms | 7 | 22 |
| `goal/region` (≤32) | 2,504 | 0.136 ms | 111 | 453 |
| `goal/far` (>32) | 4,614 | 0.230 ms | 165 | 558 |
| `budget/near` | 17 | 0.832 ms | 601 | 601 |
| `budget/region` | 1,432 | 0.819 ms | 601 | 601 |
| `budget/far` | 28,410 | 0.888 ms | 601 | 601 |

**A route that arrives is cheap and a refusal is not.** The whole cost of
pathfinding on this shard is the refusals: an arriving route within one
navigation region costs 111 nodes and 0.14 ms, and every refusal costs the
budget outright. `budget/near` is the click-on-a-wall case — a destination
eight tiles away whose own tile nothing stands on — and it costs the same 601
nodes as a route across the map, because A\* cannot know the goal is unstandable
until it has looked everywhere else.

**`find_path_toward` is free.** It is the same search read for the other
question, so the `approach` column is filled for every refusal at no extra cost.
The old probe re-ran its twenty slowest destinations through it and reported
that as a separate measurement; it was measuring one search twice.

### 🚩 `CachedTerrain` costs about 5% and buys nothing

The memo table hits **50.6%** of its lookups and is *slower than not having it*,
on every origin and in every class:

| origin | budget | bare p50 | cached p50 | |
|---|---|---|---|---|
| castle | 400 | 0.509 ms | 0.534 ms | +4.9% |
| castle | 600 | 0.793 ms | 0.821 ms | +3.5% |
| bank | 400 | 0.541 ms | 0.548 ms | +1.3% |
| bank | 600 | 0.851 ms | 0.847 ms | −0.5% |
| country | 400 | 0.360 ms | 0.402 ms | +11.7% |
| country | 600 | 0.570 ms | 0.677 ms | +18.8% |

Half of every search's `can_step` calls are answered from the table, and the
answer still arrives no sooner. What that measures is that
`MapTerrain::can_step` is **cheaper than the `HashMap<Point, Directions>` probe
that stands in for it** — a map block read and a walk over one tile's statics,
against a hash of a three-field point and a `RefCell` borrow. The cache was
written when the terrain behind it was a trait object; a direct call to a
`Copy` struct is not the thing it was built to avoid.

**This changes E.** [E](#e--one-overlay-and-a-search-that-takes-explicit-types)
says `CachedTerrain` *moves inside* `search`, on the reasoning that its lifetime
was already one query. Its lifetime is not the finding — its **cost** is. The
measurement says delete it, and the only reason to keep the type at all is the
`stats()` a probe reads. That is one decision E no longer has to take by
reading, and it takes eight hand-written forwarding
method bodies out of the workspace rather than moving them somewhere better.

### 🚩 A\* is not what a search spends its time on

The question this section exists to answer was put the other way round — *A\*
should be faster than this; is it slow at walking the map?* — and it is, but not
where the guess pointed.
[`step_cost`](../../crates/common/movement/examples/step_cost.rs) splits one
step on facet 0, over open ground, fastest of five passes:

| | ns |
|---|---:|
| `WorldMap::land` | 1.6 |
| `WorldMap::statics_at` — two `partition_point`s and a pointer chase | 15.4 |
| `MapTerrain::ground_z` | 11.8 |
| `MapTerrain::surface_at` — the *landing* half of a step | 52.4 |
| **`MapTerrain::can_step`** | **86.1** |
| **eight `step_allowed` — one whole node expansion** | **1462.3** |
| the same eight answers, with the shared work hoisted | 509.4 |

**601 nodes × 1462 ns is 0.88 ms, against a measured p50 of 0.79–0.89 ms.** The
binary heap, the two `FxHashMap`s and the closed set are inside the noise of
zero. There is no A\* to speed up: a search *is* its terrain queries, sixteen
`can_step` per node — four cardinals, and four diagonals that each also pay for
their two flanks.

**And the binary searches are 2% of it.** `statics_at` is 15.4 ns and a
`can_step` makes two of them, so 31 of 86 ns is finding the statics; the other
55 is **deriving the step rule from them again** — walking the tile's statics,
a `tiledata` lookup per static for its flags, the platform/climbable arithmetic,
and four land corners. Making the lookup free would leave 84% of the cost
standing. The rule is recomputed from raw statics on every query, and that is
the expense.

Two thirds of the rest is redundancy rather than work. `can_step(from, to)`
recomputes `start_surface(from)` on every call and all sixteen calls in one
expansion share `from`; the diagonals re-ask about flanks that are themselves
neighbours already being asked about. Hoisting both is **2.87×** with a
bit-identical checksum — which is what the last row measures, and is
deliberately **not** proposed. It is a local repair to a query that should not
be happening during a search at all.

### The first storey was never built

Both defects this node found are one omission seen from two sides. The graph
[models one height per tile](#-the-coarse-graph-is-a-one-storey-model-of-a-two-storey-world);
the search [re-derives the step rule per query](#-a-is-not-what-a-search-spends-its-time-on).
What is missing between them is the thing every navigation stack has underneath:
**a baked traversability representation that the search reads and never
questions.**

`NavigationGraph` is HPA\* — Botea, Müller and Schaeffer, cited by
[`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md)'s
own grounding section — and HPA\* is an abstraction *over a cheap grid*. It
assumes the layer below is fast and already knows what a floor is. That layer
does not exist here: the second storey was built on the ground.

The reference implementation of the layer that is missing is **Recast &
Detour** (zlib; Unreal's navigation is built on it, and Unity's NavMesh baking
descends from it). What matters here is not the polygons but two of its stages:

- **`rcCompactHeightfield`.** Each column `(x, y)` holds a *list of spans* —
  open vertical intervals — rather than one height. That is the multi-storey
  answer, and it is why a navmesh has never had our castle-plateau defect: a
  raised courtyard is its own span, not a disagreement with the land under it.
- **Off-mesh connections.** Explicit links for what the geometry does not imply:
  ladders, jumps, doors, teleports. A stair between two spans falls out of the
  spans themselves; anything that does not is *declared* rather than inferred.

For a tile world the same shape is much smaller than a navmesh: per `(x, y)` a
short list of standable surfaces, and per surface an **eight-bit neighbour mask**
plus, where the neighbour sits on a different span, its index. A node expansion
is then one record load and eight bit tests — single-digit nanoseconds against
1462 — and it is *baked*, so the rule is evaluated once per surface instead of
sixteen times per node forever.

**This is not E's, and it is not a change to be folded into E.** E takes
`find_path(&MapTerrain, &Overlay, Doors)`; what this says is that a search
should not take a `MapTerrain` at all, and that `Overlay` — the live half — is
exactly the part that must stay a query because it changes between ticks. The
split survives; what it is layered over is what changes. It wants its own plan,
and this section is the measurement that plan starts from.

### 🚩 The coarse graph is a one-storey model of a two-storey world

`coarse_bench` now loads the shard's own baked artifact — facet 0 is **28,672
regions, 85,310 nodes, 567,412 edges** — samples eight destinations per distance
band spread around a ring, and asks both routers for each. It also **floods the
facet once from the origin** with the real walk (`step_allowed`, carrying the
height each step resolves to), which is the ground truth every refusal is read
against: 3,747,934 tiles, 12.8% of the facet. A refusal on a tile the flood
never reached is the map's answer and the right one. A refusal on a tile the
flood *did* reach is the router's own.

From open country the router is right about everything:

| band | flat routed | coarse routed | flat p50 | coarse p50 | refused but walkable |
|---|---|---|---|---|---|
| 32 | 8/8 | 8/8 | 0.095 ms | 0.634 ms | 0 |
| 64 | 6/6 | 6/6 | 0.107 ms | 0.674 ms | 0 |
| 128 | 2/6 | 6/6 | 0.709 ms | 1.117 ms | 0 |
| 256 | 1/7 | 6/7 | 0.862 ms | 3.509 ms | 0 |
| 512 | 0/8 | 7/8 | 0.794 ms | 3.102 ms | 0 |
| 1024 | 0/7 | 5/7 | 0.926 ms | 6.119 ms | 0 |

**That reverses the only routing verdict on record.** The synthetic run in
[`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md) had
the hierarchy *slower* than flat A\* — 0.974 ms p95 against 0.803 ms — and it was
right about its own world and wrong about this one. On an open plain flat A\*
walks a straight line and the corridor adds portals to a route that never needed
them. On a facet, from 128 tiles out, **flat A\* answers 3 of 28 and the coarse
router answers 24 of 28**, at three to seven times the cost of a refusal it
would otherwise hand back. The comparison the old bench made was not a bad
measurement; it was a measurement of the wrong world, and `--synthetic` keeps it
so the pair can be read together.

And from the castle plateau the router is wrong about nearly everything:

| origin | z | sampled | walkable | **refused but walkable** |
|---|---|---|---|---|
| (1363, 1600) Britain castle | **30** | 45 | 44 | **37** |
| (1434, 1699) Britain bank | 2 | 45 | 43 | 5 |
| (1828, 2745) Trinsic | 0 | 42 | 36 | 1 |
| (600, 2100) Skara Brae | 0 | 35 | 15 | 0 |
| (1500, 1900) open country | 0 | 42 | 38 | 0 |

From the castle, every destination past 128 tiles is refused and every one of
them is walkable — 29 out of 29. It is not the town: the bank is a hundred tiles
away,
in denser statics, and disagrees with the flood five times out of 45. **It is
the height**, and the mechanism is in the build rather than in the search:

```rust
let near = terrain.ground_z(tile).unwrap_or(0);      // navigation.rs, build
let point = Point::new(x, y, near);
points[…] = terrain.can_step(point, point);
```

One sampled height per tile, and that height is
[`ground_z`](../../crates/common/movement/src/terrain.rs#L541) — which is
`average_land_z`, **the land alone, with the statics on it ignored**. So the
graph is a model of the terrain surface and of nothing standing on it: the
castle's plateau is land at z=30, the city around it is land at z=0, and the
stairs and bridges a body actually climbs between them are statics the sampler
never saw. `can_step` refuses a thirty-unit cliff, the flood fill inside
`component_labels` therefore never leaves the plateau, and the plateau becomes
an island in a graph whose own map says otherwise.

Every consequence of that follows: a building's upper floor, a bridge over a
river, a walkway over a street, and any raised courtyard is either absent from
the graph or — where the land under it happens to be standable too — fused with
whatever is *underneath* it, which is the same fault pointing the other way.
It is the same z-blindness [`sight_clear` has](#-blindterrain-stood-for-a-rule-that-cannot-exist),
in the other subsystem, found the same way.

**This is a precondition for [F](#f--the-graph-nobody-reads), and F is where it
is filed.** Teaching `step_toward` to use the graph before this is fixed would
give creatures a router that believes Britain's castle is unreachable.

### What the probes had to become

Six changes, each of which was load-bearing for a number above:

- **[`search_path`](../../crates/common/movement/src/path.rs)** — one public
  reading of the search `find_path` and `find_path_toward` already run, handing
  back `explored` and a `SearchExit` alongside the route. The node budgets are
  argued in nodes, and until this existed nothing outside the crate could count
  one. The two entry points are unchanged and still drop it; `PathSearch`'s doc
  says why a body should.
- **A refusal has five names.** `find_long_path` answered every failure with one
  string, `unreachable_or_live_refinement`, which is four repairs wearing one
  word. It is now `OffGraph`, `NoJoin`, `NoCorridor`, `NoLocalRoute` and
  `PortalsExhausted` — and the whole diagnosis above is one tally of them:
  **38 `NoCorridor` out of 45**, which ruled out the deadline, live refinement
  and the endpoint joins in a single run and left the graph itself.
- **Min-of-three, interleaved.** Two consecutive runs of the same sweep measured
  40.6 s and 65.5 s of wall clock, worst-case 43 ms against 15 ms, with every
  node count bit-identical. This workstation cannot be asked for a tail. The
  minimum of three repeats, with bare and cached taken back to back per
  destination so drift moves both, brought the worst case to 3.7 ms and made the
  5% cache result readable at all.
- **The flood.** Without ground truth every `NoCorridor` is ambiguous, because
  an island across a bay has no land route and refusing one is correct. 7 s of
  breadth-first walking is what turns 38 refusals into 37 defects.
- **A ring is sampled at eight anchors**, one per eighth, with the first
  standable tile forward from each. Taking the first eight standable tiles
  instead — which is what it did first — returned eight *adjacent* tiles on one
  side of the ring: eight readings of very nearly one route, reported as a
  distribution.
- **An origin nothing stands on is an error.** (1300, 1624) is such a spot, and
  the probe reported it as a tidy `arrived=0` over 4,224 destinations in 3 ms
  rather than as a mistake.

**Done when:** ~~both have a committed facet-0 run with the numbers in this
document — p50/p95/worst per route class, node counts, `TransitionCacheStats`
hit rates~~ — all four, above.

## E — one `Overlay`, and a search that takes explicit types

**Both its edges are met**: D landed, and [the oracle](#0--the-oracle-) above is
the measurement it was held behind. It is the last node of the seam.

`Overlay` lands in `common/movement`: blockers by tile with their z-spans and
their door flag, plus the surfaces a deck adds. `Obstructions` becomes the
server's *builder* for one, `Clutter` the client's. Neither implements
anything. `find_path`, `find_path_toward`, `search`, `step_allowed`,
`corner_open`, `Around::read` and the whole of `navigation.rs` take
`&MapTerrain` and `&Overlay` by name; `InRegion` becomes a bound;
**`CachedTerrain` is deleted rather than moved** — [the oracle measured
it](#-cachedterrain-costs-about-5-and-buys-nothing) at a 50.6% hit rate and
about 5% *slower* than the calls it memoises; `OpenWorld` becomes `None`.

Three decisions this node takes, all of them from [the table
above](#and-they-do-not-compute-the-same-set):

- **Mobiles.** The client's index has them, the server's does not. Either they
  are a category the shared type carries and the server leaves empty, or they
  stay out and the client keeps a second pass. Whichever, the agreement test is
  about *items*.
- **Identity.** `Obstructions::block` is idempotent per entity *and* z and
  things get unblocked; the client rebuilds whole. Either the shared type
  carries an owner, or the server keeps its keyed index and *produces* an
  `Overlay` from it. A third shape is worth measuring before either: the
  server's index *owns* an `Overlay` and its `block`/`unblock` maintain it, so
  nothing is rebuilt per tick.
- **Blockers and surfaces are one type or two.** `aboard` is the only surface
  source today (`Boats`), and it is the reason a bitmask is not the answer.

**Done when:** `grep -rn "dyn Terrain" crates` is empty, `Terrain` is gone, and
one test asserts the two ends produce the same blockers for the same items —
which is the agreement `clutter.rs`'s header claims and nothing currently
checks.

## F — the graph nobody reads

Unrelated to the seam, found in the same reading, here so it is not lost. It had
no edges at all until [the oracle](#0--the-oracle-) gave it one.

[`boot.rs:615`](../../crates/server/server/src/boot.rs#L615) loads the baked
navigation graph, validates its dimensions, and stores it in
`FacetState.coarse`. The only call to
[`coarse_router()`](../../crates/server/state/src/runtime.rs#L422) in the whole
workspace is in a test. Server AI plans with flat
[`find_path`](../../crates/server/ai/src/lib.rs#L79) at a budget of **400**
explored tiles — so a creature cannot route across a town while the artifact
that would let it sits loaded and unread. The client does use it, through its
own `Resources.coarse`:
[`steer::Ground::path`](../../crates/client/app/src/steer.rs#L331) falls back
past 8 tiles.

Either `step_toward` gains the same fall-back, or `boot.rs` stops paying for
the load, the validation and the resident graph. What it must not stay is what
it is: paid for, validated, unread.

### 🚩 And the artifact is wrong before anyone reads it

The oracle measured what wiring it up would buy, and the answer has two halves.

**It is worth wiring up.** On open ground the coarse router answers 24 of 28
destinations past 128 tiles where flat A\* at budget 600 answers 3 — which is
exactly the "cannot route across a town" this node is about, in numbers.

**And it must be fixed first.** From a raised origin it refuses 29 of 29
walkable destinations past 128 tiles, because
[`NavigationGraph::build` samples one height per tile and that height is the
land alone](#-the-coarse-graph-is-a-one-storey-model-of-a-two-storey-world).
Every floor, bridge and stair a static provides is invisible to it, so Britain's
castle plateau is an island in a graph whose own map says it is not. Teaching
`step_toward` to plan with that would give creatures a router that is confidently
wrong about the middle of the largest city on the facet — which is worse than
the flat search it replaces, because the flat search at least fails honestly.

The fix is a change of sampled height, not of structure: `ground_z` is
`average_land_z`, and [`stand_z`](../../crates/common/movement/src/terrain.rs#L562)
is the same question asked of the surfaces. What it costs is that a tile stops
having *one* answer — a gallery over a street is two places — which is a change
to what a graph node **is**, and is why this is filed rather than done in
passing.

**Done when:** either a test walks a creature a distance flat A\* at budget 400
cannot — over ground the flood says is walkable, from a raised origin as well as
a flat one — or `FacetState.coarse` is gone.

## Decisions, taken here

**The trait goes, not just the `dyn`.** Five of the six implementors are an
action over a terrain rather than a terrain, so making the seam generic would
preserve the mistake with better codegen. A generic's bound *is* a trait; the
choice between `&dyn T` and `&impl T` is about dispatch, and dispatch is not
what is wrong here.

**Explicit types, imported by name.** The dependency rule was the one honest
argument for a trait — `find_path` is in `common/movement`, `LiveTerrain` in
`server/state`, `Cluttered` in `client/app`, and `common` may name neither. An
`Overlay` living in `common` that both ends *build* answers that without
inverting anything: data crosses the boundary, not behaviour.

**Behaviour becomes data, deliberately.** A mask, a flag, a rectangle and a
memo table are values. They were types with a vtable because the seam invited
it, and each one cost a virtual call on every A\* edge for the privilege.

**The memo table is deleted rather than relocated, and a measurement is why.**
Three drafts of this document moved `CachedTerrain` somewhere better on the
strength of an argument about its *lifetime*. [0](#-cachedterrain-costs-about-5-and-buys-nothing)
measured its *cost* instead and found it about 5% slower than the calls it
stands in for, at a 50.6% hit rate — because a `HashMap<Point, Directions>`
probe is dearer than `MapTerrain::can_step`, which reads one map block and walks
one tile's statics. It was written against a trait object and outlived the
reason for it. **A cache whose hit is slower than its miss is not a cache**, and
no argument about where it should live was ever going to say so.

**Nodes are the currency, not milliseconds.** Every node count in 0 came back
bit-identical between runs on the same map; two consecutive runs of one sweep
disagreed by 60% on wall clock and by 3× on the worst case. So a claim about
cost is made in nodes and a claim about time is made from a minimum of repeats,
and a probe that reports only a mean and a maximum — which is what
`map_path_probe` did — reports mostly the machine.

**The table half is an import, not a design.** Every crate that reaches
`tiledata` through `Terrain` already compiles `openshard-uofiles`. There is no
seam to mint and no layering rule to weigh.

**`Doors` is the enum.** clutter.rs took that decision with a reason — call
sites four modules apart cannot read a `bool` — and this plan does not reopen
it. The server's `through_doors: bool` is what changes.

**`MapTerrain` becomes three borrows.** Its `AsRef` parameters existed to let
one caller box it with no lifetime, and that caller was `FacetState`, which
stopped boxing in D. Both ends already held the fields. Landed as two borrows
and a flag, because B had already taken `multis` off it.

**The accessor lives where both halves do.** `live_terrain` and its siblings are
`WorldState` methods rather than `FacetState` ones, and that follows from the
map and the table being owned by different things: a facet cannot hand out a
terrain it only has half of. This is the one place D changed a call shape rather
than a type — `state.facet_state(f).live_terrain()` became
`state.live_terrain(f)` at fifteen sites.

**There is always a table.** `WorldState.tiles` and `.multis` are total:
`TileData` and `Multis`, never `Option` and — since D — never behind an `Arc`
either. A shard with no client files holds
`TileData::empty()` and `Multis::default()` — which is not a stand-in for the
file but *the file saying nothing*, and it gives every caller back exactly the
answer it used to compute for itself: weight nothing, layer zero, no name, no
components. An earlier draft of this document argued the opposite, that the
`Option` was load-bearing because the three callers differ in what they do about
it. They do differ — and each still says so, at its own lookup, in one line
instead of a `map_or` around it. What the `Option` actually bought was a second
way to say "no client files", which is the very defect B removed at the other
end: two spellings of one state that nobody keeps in step. One consequence is
visible and intended: `design_detail_packet` used to send nothing at all with no
tiledata and now sends a house whose every component is a floor, because that is
what a table saying nothing says about heights.

**The order is the graph, not the phase number.** A and B have nothing before
them; C needs A; D needs B and C; E needs D and the oracle; F needs nothing.

**A method that shadows a trait method must not change shape.** `MapTerrain`
carried an inherent `can_fit(x, y, z, height)` beside the trait's
`can_fit(tile, z, height)`; inherent wins, so which function a call reached
depended on whether the caller's variable was concrete or a trait object. Two
sites had already routed around it by hand. Both are now `(tile, z, height)`.
D is what made this load-bearing rather than cosmetic: it converted every
`&dyn Terrain` on the server into a `MapTerrain`, which is exactly the switch
that flips which one a caller gets.

**No flag day.** An earlier draft proposed migrating through
`&T where T: Terrain + ?Sized`, which `dyn Terrain` satisfies and so breaks no
caller. That was scaffolding for keeping the trait. The nodes are ordered so
each one removes implementors rather than re-typing callers.

## Found in the reading, filed here

Small things this document is the only current record of:

- ~~`WorldState.tiles` is behind an `Arc` only because a facet's terrain is
  boxed~~ — D removed the box and the `Arc` with it; the reasoning is kept
  [above](#one-arc-is-gone-and-one-is-not-and-the-difference-is-the-box) because
  it is what predicted the deletion.
- ~~`Cluttered` does not forward `multi_components` or `land_is_water`~~ — B
  removed the first by removing the question; `land_is_water` is now forwarded
  by nobody on that end, because `Cluttered` never wrote it and the map answers
  for itself.
- ~~No client `MapTerrain` is ever given `with_multis`~~ — `with_multis` is
  gone. The client reads `Resources::multis` directly, which is what it did
  anyway everywhere except through the terrain.
- ~~`MapTerrain::swimming` is dead in production and is a body's property on a
  world's object~~ — still dead, no longer parked on the world: a `MapTerrain`
  is per-query now, so the flag is scoped to one asker. See
  [D](#swimming-is-an-argument-now-because-the-thing-it-sits-on-is-the-query).
- ~~`boat_step_cost`'s recorded numbers are stale~~ — **re-run, 2026-08-22.**
  The 15ns/55ns of 2026-08-16 was taken with a `Sea` double whose `can_step` was
  one integer comparison, and D replaced it with a real `Scene`. Against a real
  `MapTerrain::can_step` it reads **110ns with no boats and 123ns with one
  moored** — the moored ship costs 13ns and **12%**, where the same probe over
  the double said 267%. Its own doc predicted exactly that (*"a small fraction
  rather than a multiple"*), and both readings are kept in `obstruct.rs` because
  the pair is the point: one probe, two baselines, and only one of them a world.
- 🚩 **A sight line has no height and neither does the navigation graph.**
  [`sight_clear`](#-blindterrain-stood-for-a-rule-that-cannot-exist) reads the
  statics on the tiles it crosses and not the endpoints' own columns, so two
  mobiles on one tile at different z see each other through a floor.
  [`NavigationGraph::build`](#-the-coarse-graph-is-a-one-storey-model-of-a-two-storey-world)
  samples one height per tile and that height is the land alone, so a static
  floor is not somewhere the graph can route. **These are one defect in two
  subsystems** — the world is modelled as a height field by things that walk
  over a world of stacked surfaces — and they were found independently, a
  session apart, which is the argument for naming the class rather than the two
  cases.
- 🚩 **The search's cost is entirely the terrain's, and mostly a rule being
  re-derived.** One node expansion is 1,462 ns and A\*'s own machinery is
  within noise of zero;
  [the breakdown](#-a-is-not-what-a-search-spends-its-time-on) puts 2% on the
  statics lookup's binary searches and the rest on evaluating the step rule from
  raw statics, sixteen times per node. Two thirds of that is redundancy — the
  tile stepped *off* is recomputed per call, and the diagonals re-ask about
  their flanks — and hoisting both is 2.87× with an identical checksum. Left
  unhoisted on purpose: it is a local repair to a query that should be a table
  lookup. See [the first storey](#the-first-storey-was-never-built).
- **`MapTerrain::start_surface` is public now**, because it is the other half of
  the already-public `check` and a probe cannot split a step without it.
- **A single `None` was hiding four repairs.** `find_long_path` answered
  every failure with `unreachable_or_live_refinement`, which is a deadline, a
  missing endpoint join, a missing corridor and exhausted refinement wearing one
  word. Split into five named exits by 0, and a tally of them is the whole of
  the diagnosis above — the wrong verdict was available and cheap without it.
- **`server/world/src/terrain.rs` is a `pub use` and a single test.** Its whole
  body is `pub use openshard_movement::{MAX_STEP_UP, MapTerrain, PLAYER_HEIGHT}`
  plus one test that needs `openshard-state`'s layer constants. The re-export is
  against [`style.md`](../style.md) — the same finding as `movement/lib.rs`'s
  below — and its callers can name `openshard-movement`, which every one of them
  already depends on.
- `obstruct.rs`'s `MOBILE_HEIGHT: i32 = 16` duplicates
  `openshard_movement::PLAYER_HEIGHT`, which the client imports in the same
  place for the same purpose.
- `event_loop.rs` and `ui_command.rs` each build three terrains per input and
  repeat the same `if self.auto_open_doors` selection — two of the three differ
  only by a flag, and the third is the first one's inner map.
- `openshard-movement` re-exports through `pub use` in
  [`lib.rs`](../../crates/common/movement/src/lib.rs#L63), against
  [`style.md`](../style.md).

## Out of scope, named

- **The statics layout.** 120,745 allocations and 38.2 MiB where a CSR pair
  would be 2 and ~13.5 MiB —
  [direction B](new_map_representation/plan.md#b--our-own-chunk-format-and-a-uo-importer)'s,
  measured there.
- **Residency.** The whole facet is resident at ~150 MiB on both ends.
  [Direction G](new_map_representation/plan.md#g--residency-and-size-deferred-on-purpose).
- **A second hierarchy level.** Phase 3 of
  [`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md),
  gated on the facet-0 numbers the oracle produces.
- **`MAX_SEARCH_TIME` and the node budgets** — 50 ms inside one search, 400 for
  server AI, 600 for a client plan. The oracle's data is what those can finally
  be asked against; changing them before it exists is guessing.
- **`net_command`'s multi expansion.** The third way entities are laid over the
  map, and the picture's rather than movement's. `Overlay` may end up being
  what merges it, which is
  [`snapshot.md`](new_map_representation/snapshot.md)'s own named successor —
  but this plan does not take the picture on.

## Where a session starts

**E — because it is the last node of the seam and nothing is in front of it any
more.**

0, A, B, C and D are done. E's four decisions are answered by reading rather
than by the compiler, and [the oracle](#0--the-oracle-) has since answered a
fifth that the [section on E](#e--one-overlay-and-a-search-that-takes-explicit-types)
used to carry as a move rather than a choice: `CachedTerrain` is deleted, not
relocated. What is left to decide there is mobiles, identity, and whether
blockers and surfaces are one type or two.

**F is the other node a session can take, and it is no longer the free one.**
The coarse graph the server loads, validates and never reads is still
independent of the seam — but 0 found that
[the artifact is wrong before anyone reads it](#-and-the-artifact-is-wrong-before-anyone-reads-it),
so the node now has a defect in front of its feature. Fixing what height the
graph is built from is the first half of it; wiring `step_toward` to the result
is the second.

**Both measurements this section used to owe are paid.** The probes have their
facet-0 runs, above; `boat_step_cost`'s 15ns/55ns — measured against a double D
deleted — has been re-run against a real `MapTerrain` and reads **110ns/123ns**,
which is the correction its own doc comment predicted.

### What D left behind for E

- `LiveTerrain` and `Cluttered` are still two hand-written decorators over the
  same nine methods, and `Cluttered` still skips `land_is_water`. D gave them a
  named map to forward *to*; only E stops them forwarding.
- `housing::check_ground` still needs *"is anything in this body"* without
  *"and is there a surface"* — [C's leftover](#c--the-doubles-become-scenes-).
  `MapTerrain::is_obstructed` is that question and is private to the map; E is
  where `check_ground` stops going through a trait and can be handed one.
- `CachedTerrain`, `InRegion` and `OpenWorld` are untouched: all three are on
  the search side, which D did not open.

### And what it carried in from B

- A scene hands out [`into_shard`](#a--scene-grows-what-the-doubles-need-)'s
  pair — since D, a `MapSnapshot` and a `TileData` — so every converted test
  sets `state.tiles` from the same table its ground reads. C did that eight
  times, which is why D changed a type rather than hunting for disagreements.
- ~~`WorldState.tiles` being `Option` is load-bearing and should stay one~~ —
  reversed and done. Both tables are total; see [There is always a
  table](#decisions-taken-here). A shard with no client files is still a real
  configuration, and it is now spelled the same way everywhere: empty tables,
  not absent ones.

### The map track's C2 went with it

`new_map_representation/`'s live publish — an edit taking effect in a running
shard between two ticks — was blocked on exactly this field: `with_facet` used
to box a `MapTerrain<MapSnapshot, _>` as a trait object, so the shard held its
snapshot *inside* one and had nothing to call `publish` on. `FacetState.map` is
a `MapSnapshot` now and `facet_state_mut` reaches it, so the `&mut` that
[`MapSnapshot::publish`](../../crates/common/map/src/snapshot.rs) needs — the
one that makes a publish atomic by construction — is available on the shard.
That is the precondition, not the feature: **who** calls it, and where in the
tick, is C2's own.
