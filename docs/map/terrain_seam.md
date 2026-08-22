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

Read off the workspace, not remembered. `impl Terrain for` appears at **37
sites under 27 distinct names**; 31 of those sites are test doubles. These are
the rest:

| | | |
|---|---|---|
| [`MapTerrain`](../../crates/common/movement/src/terrain.rs#L61) | the map and `tiledata.mul` | **a terrain** |
| [`Cluttered`](../../crates/client/app/src/clutter.rs#L306) | the client's live items over it | a **mask** |
| [`LiveTerrain`](../../crates/server/state/src/obstruct.rs#L199) | the server's live items over it | the **same mask** |
| [`CachedTerrain`](../../crates/common/movement/src/cache.rs#L30) | memoises `can_step` for one query | a **memo table** |
| [`InRegion`](../../crates/common/movement/src/navigation.rs#L80) | three lines: refuse a step leaving a rectangle | a **parameter** |
| [`OpenWorld`](../../crates/common/movement/src/walk.rs#L270) | `can_step` returns `Some(to)` | the **absence** of a map |

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
the table. What is left on `FacetState.terrain` is the map, which is D's.

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
| `CachedTerrain` | moves *inside* `search`, which already owns per-query maps for `came_from`; its lifetime was already exactly one query |
| `OpenWorld` | an empty overlay over no map — `Option<&MapTerrain>`, which `LiveTerrain` already carries |

### `MapTerrain` is three borrows, and both ends already hold them

`MapTerrain` is generic over ownership — `M: AsRef<Map>, T: AsRef<TileData>` —
so the server can own its map and the client borrow one. That parameterisation
buys one thing: a terrain with no lifetime, so it can sit in a `Box` in a
struct field. Nothing else asks for it.

And both ends already own precisely its three fields:

| | server | client |
|---|---|---|
| map | `MapSnapshot`, [`boot.rs:660`](../../crates/server/server/src/boot.rs#L660) | `Resources.map: MapSnapshot` |
| tiledata | `Arc<TileData>` | `Resources.tiledata: Arc<TileData>` |
| multis | `Option<Arc<Multis>>` | `Resources.multis: Option<Arc<Multis>>` |

So `MapTerrain<'a> { map: &'a Map, tiles: &'a TileData, multis: Option<&'a Multis> }`
is buildable on either end from fields that already exist, costs three pointers
to construct per query, and makes the type **one name that can be imported**
rather than two unrelated instantiations. That is the collapse; the `AsRef`
bound does not survive it, and does not need to.

### `swimming` is a property of a body, parked on a world

[`MapTerrain::swimming`](../../crates/common/movement/src/terrain.rs#L99) is
set **nowhere in production** — the only caller is `load_client(true)` in
terrain.rs's own tests. It is a per-creature fact (*a boat or a fish says yes*)
living as a field on the per-facet world, which is why no shipping caller can
turn it on: there is one terrain per facet and it would be turned on for
everyone.

Deciding it is part of deciding `MapTerrain`'s shape, so it is decided in the
same node: either it becomes an argument of the query beside `Doors`, or it
goes until there is a swimmer. What it must not do is stay a field.

## What has no incoming edge

The collapse is a graph, not a sequence. Two nodes have nothing before them and
can be worked in either order, or at once:

```
 A. Scene grows what the doubles need ─┐
                                       ├─> C. the doubles become Scenes ─┐
 B. the table leaves the trait ✅ ─────┴──────────────────────────────────┼─> D. FacetState holds data
                                                                          │      MapTerrain is borrows
 0. the facet-0 oracle ───────────────────────────────────────────────────┴─> E. Overlay, and the search
                                                                                 takes explicit types
 F. the coarse graph nobody reads — no edges at all, in or out
```

`Box<dyn Terrain>` is held up in `FacetState` by **tests**, not by production:
fifteen `= Some(Box::new(...))` substitutions across four files, plus three
`= None` in two more, against exactly one production construction at
[`boot.rs:664`](../../crates/server/server/src/boot.rs#L664). That is the edge
`A → C → D`, and it is why D cannot be the first commit however mechanical it
looks.

### Following the compiler works for B and D, and not for E

Deleting a method from a trait and letting `cargo check` enumerate the wreckage
is the right tool for the table (B) and for `FacetState` (D): every site there
is a one-line substitution with a mechanically obvious replacement, and there
are 26 of them.

It is the wrong tool for `Overlay` (E). Removing `can_step` from a trait points
at 38 impl sites and says nothing about the four decisions listed under [the
two that are one](#and-they-do-not-compute-the-same-set) — mobiles, identity,
door opacity, decks. Those are answered by reading, and E is where they are
answered.

## A — `Scene` grows what the doubles need

No incoming edges. Pure addition: nothing breaks while it lands.

[`Scene`](../../crates/common/movement/src/scene.rs) builds a real
`MapTerrain` from hand-placed ground, floors, stairs and walls with no client
files, and its own header is the argument for using it: *"A fixture that
reimplemented the rule would agree with itself and prove nothing."* Most of the
doubles it replaces answer `can_step` with `Some(to)`.

Four additions, read off what the doubles actually do:

- **More than one block.** `SIDE` is 8 and `Scene::flat` builds
  `BlockExtent { wide: 1, down: 1 }`, while `obstruct.rs`'s tests place things
  at (10, 10). `Map::from_blocks` already takes an extent; this is an argument.
- **A named graphic.** `place` mints ids in sequence, so a caller cannot say
  *this* graphic. `Shop` and `harvest`'s `Ground` answer `statics_at` with
  particular ids that domain tables match against.
- **A tiledata row.** `NamedTerrain` exists for `item_name`; `Scene` already
  calls `set_static_tile`, so this is exposing what it does internally.
- **An owned `into_terrain`**, since `terrain()` borrows, and a multi table —
  [`Multis::of`](../../crates/common/uofiles/src/multi.rs#L282) already builds
  one from hand-made values, so it is wiring, not parsing.

**Done when:** a `Scene` can express what each of the seven double types
(`Ground` in three shapes, `Sea`, `Shop`, `BlindTerrain`, `FrameTerrain`,
`NamedTerrain`, `RaisedFloorTerrain`) was written to say.

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
  clearing `FacetState::terrain`. They clear `state.multis` and `state.tiles`
  now, which is what they meant — and the fact that those are two different
  clearings is the defect this node removes.

515 insertions, 642 deletions across 32 files. `cargo test --workspace` and
`cargo fmt --all` are clean, and `clippy` warns in the same ten places it warned
in before.

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

## C — the doubles become Scenes

Needs A. Fifteen substitutions, each one replaced by a `Scene` that builds a
real `MapTerrain`. This is worth doing on its own evidence and would be worth
doing if the rest of this document were abandoned: a double that answers
`can_step` with `Some(to)` is a test that proves the caller compiles.

**Done when:** `facet_state_mut().terrain` is assigned nothing hand-written.

## D — `FacetState` holds data, `MapTerrain` holds borrows

Needs B and C.

[`FacetState::terrain`](../../crates/server/state/src/runtime.rs#L379) is a
`Box<dyn Terrain + Send + Sync>` whose doc comment says the crate *"sits below
the client-file parsers"* — which has not been true since `openshard-state`
gained its `openshard-uofiles` dependency for
[`customisation.md`](../customisation.md)'s C1. It becomes the three things it
already contains: a `MapSnapshot`, an `Arc<TileData>`, an `Option<Arc<Multis>>`
— and an accessor that hands out a `MapTerrain<'_>`.

`MapTerrain` loses its `AsRef` parameters and takes a lifetime. `swimming` is
decided here. `travel.rs`'s `terrain.is_none()` becomes a question about the
map, which is what it was asking.

**Done when:** `grep -rn "dyn Terrain" crates/server` is empty, and every one
of the 26 sites names what it takes.

## E — one `Overlay`, and a search that takes explicit types

Needs D, and needs the oracle below.

`Overlay` lands in `common/movement`: blockers by tile with their z-spans and
their door flag, plus the surfaces a deck adds. `Obstructions` becomes the
server's *builder* for one, `Clutter` the client's. Neither implements
anything. `find_path`, `find_path_toward`, `search`, `step_allowed`,
`corner_open`, `Around::read` and the whole of `navigation.rs` take
`&MapTerrain` and `&Overlay` by name; `InRegion` becomes a bound;
`CachedTerrain` moves inside `search`; `OpenWorld` becomes `None`.

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

### Phase 0 — the oracle, which gates E and nothing else

**E is not landable without it, because "faster" is currently unmeasurable.**
The only routing benchmark on record is synthetic: a 1024×1024 open world where
the hierarchy is *slower* than flat A\* (0.974 ms p95 against 0.803 ms), in
[`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md).
No facet-0 measurement exists at all; that document has carried it as
outstanding since 2026-08-13.

Two probes already exist and neither has a recorded run on a real install:
[`map_path_probe`](../../crates/common/movement/examples/map_path_probe.rs) and
[`coarse_bench`](../../crates/common/movement/examples/coarse_bench.rs).

It gates E because E is the only node that touches the A\* edge. A, B, C and D
change no hot path and are not held behind a client install.

**Done when:** both have a committed facet-0 run with the numbers in this
document — p50/p95/worst per route class, node counts, `TransitionCacheStats`
hit rates.

## F — the graph nobody reads

No edges at all. Unrelated to the seam, found in the same reading, here so it
is not lost.

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

**Done when:** either a test walks a creature a distance flat A\* at budget 400
cannot, or `FacetState.coarse` is gone.

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

**The table half is an import, not a design.** Every crate that reaches
`tiledata` through `Terrain` already compiles `openshard-uofiles`. There is no
seam to mint and no layering rule to weigh.

**`Doors` is the enum.** clutter.rs took that decision with a reason — call
sites four modules apart cannot read a `bool` — and this plan does not reopen
it. The server's `through_doors: bool` is what changes.

**`MapTerrain` becomes three borrows.** Its `AsRef` parameters exist to let one
caller box it with no lifetime, and that caller is `FacetState`, which stops
boxing in D. Both ends already hold the three fields.

**The order is the graph, not the phase number.** A and B have nothing before
them; C needs A; D needs B and C; E needs D and the oracle; F needs nothing.

**No flag day.** An earlier draft proposed migrating through
`&T where T: Terrain + ?Sized`, which `dyn Terrain` satisfies and so breaks no
caller. That was scaffolding for keeping the trait. The nodes are ordered so
each one removes implementors rather than re-typing callers.

## Found in the reading, filed here

Small things this document is the only current record of:

- ~~`Cluttered` does not forward `multi_components` or `land_is_water`~~ — B
  removed the first by removing the question; `land_is_water` is now forwarded
  by nobody on that end, because `Cluttered` never wrote it and the map answers
  for itself.
- ~~No client `MapTerrain` is ever given `with_multis`~~ — `with_multis` is
  gone. The client reads `Resources::multis` directly, which is what it did
  anyway everywhere except through the terrain.
- `MapTerrain::swimming` is dead in production and is a body's property on a
  world's object.
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

**A.** B is done; A is the only node left with nothing before it, and it is what
unblocks C and therefore D. It needs no client install and breaks nothing while
it lands: `Scene` grows four abilities, and only then do the fifteen boxed
doubles have somewhere to go.

Two of B's findings are worth carrying into A and D:

- The doubles that were replaced in B were replaced by **real tables**, not by
  `Scene`s — a `TileData::empty()` with three rows written into it. The ones
  left are about *geometry*, which is what `Scene` is for, so the split between
  the two fixtures is now clean rather than incidental.
- `WorldState.tiles` being `Option` is load-bearing and should stay one: a shard
  with no client files is a real configuration, and the callers differ on what
  they do about it — no weight is harmless, no layer makes every weapon
  one-handed, no multi table refuses to place a house. A default table would
  have hidden all three.
