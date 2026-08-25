# The three layers, and what goes in which

> **The matryoshka's routing table.** [`map_rebuild.md`](map_rebuild.md#the-map-in-one-type)
> states the three layers and owns them; this page is the one question that
> decides which of them a thing belongs to, and the table of every answer taken
> so far. Read it when you have a thing and do not know where it goes — or when
> two rules in this folder look like they contradict each other, because two of
> them do and section 3 is why they do not.

## 1. The question

> **Must a bake see it?**

That is the whole rule, and it is the invariant `map_rebuild.md` states one line
under the layer table:

> *What may be baked is exactly what is below the live layer.*

A navigation graph, a span grid, a building flood, a minimap raster — every one
of them is derived from the ground and the statics, and none of them may contain
a door, a crate, a moored deck or a house. So:

- **Yes, a bake must see it** → it belongs to the **ground** or the **statics**.
  It moves by a published `Patch`, it moves the revision, and it invalidates
  every bake over `Patch::touched_chunks`. It is not free and it is not meant
  to be.
- **No, a bake must not see it** → it belongs to the **live layer**. It moves
  between two ticks, it is not part of any revision, and it costs a bake
  nothing at all.

The question is **not** "is it a static?" and not "is it made of static art".
Half the things in the live layer are static art — a house's walls are
`Component.graphic`, *"the static's art id"*. What decides is whether a route
planned an hour ago is allowed to be wrong about it.

## 2. Every answer taken so far

| you have | layer | how it gets there | who draws it | what it costs |
|---|---|---|---|---|
| a raised or retextured land tile | **ground** | `PatchOp::SetLand` → `MapSnapshot::publish` → new revision → the chunk goes to connected clients | the client, out of its own copy of the facet | the rebake |
| a static an operator placed or removed | **statics** | `PatchOp::AddStatic` / `RemoveStatic`, same path | the same chunk | the rebake |
| a door, a crate, a corpse, any item on the ground | **live** | an entity with a serial → `Obstructions` → [`obstruct::project`](../../crates/server/state/src/obstruct.rs) | the client, from the packet about the item | nothing |
| a ship's deck and planks | **live** | `Boats` → the same `project` | the client, from the multi | nothing |
| a house of a shipped shape | **live** | `HouseRecord` (serial, multi id, position, facet, owner); components are resolved at placement — see [`housing.md`](../housing.md) D2 | **the client, by itself**, resolving the multi id against its own `multi.mul` | nothing |
| a house of a customised shape | **live**, and on the entity | `HouseDesign { components, revision }` in `openshard-state`, sent as `0xD8` — see [`customisation.md`](../customisation.md) | the client, from the list we send it: **this is the one row where the picture stops being free** | nothing |

The last two rows are why a castle — 3,667 components over 31×32 tiles — costs a
bake nothing. That density is also the evidence [R3](map_rebuild.md#r3--a-house-is-a-layer-and-it-has-floors)
closed the question on: a house as a patch is a bulk insert into an immutable
base, and *"the flat layout refuses to let a house be anything but an overlay"*.

## 3. Two sentences that read as a contradiction

They are both in this folder, one page apart:

- [*"A tile of ground moved — rebake, and never an overlay"*](map_rebuild.md#a-tile-of-ground-moved--rebake-and-never-an-overlay)
- [*"a house is a layer"*](map_rebuild.md#r3--a-house-is-a-layer-and-it-has-floors)

The first forbids an overlay; the second is one. Quoting either at a question the
other one answers is the mistake this page exists to prevent, and it has been
made.

**What separates them is section 1 and nothing else.** A ground overlay would
have to be *in* the bake for the bake to be right — so it is not a live layer at
all, it is the base with a slower spelling, and it would put two sources of truth
for a column's height on the hottest path there is. A house must **not** be in
the bake. Same mechanism, opposite verdicts, because the verdict was never about
the mechanism.

## 4. The live layer only ever adds

`Cover` is a span and a kind: something in the way, or somewhere to stand.
[`overlay.rs`](../../crates/common/map/src/overlay.rs)'s header lists the three
questions it answers and notes that a deck — *somewhere to stand the map does not
know about* — is **the only positive thing there**.

There is no third kind, and there is deliberately no way to say *"ignore what the
map has here"*. So:

- **A wall can be added to a tile the map left open.** That is a house.
- **A surface can be added above what the map answers.** That is a deck, or a
  second storey.
- **Nothing in the base can be taken away.** A static the importer laid down is
  removed by a patch, a revision and a rebake, or it is not removed.

That monotonicity is not an omission to be filled in later. A subtractive live
layer would let a route be planned over a wall that the bake still contains,
which is the same two-sources-of-truth failure section 3 refuses for ground.

## 5. One art id, four forms

The same `Graphic` can reach a player through four different things, and only the
first is in a bake:

| form | where it lives | addressed by | in a bake |
|---|---|---|---|
| a base static | `WorldMap.statics`, one CSR run over the facet's blocks | a block of the facet | **yes** |
| a multi component | `multi.mul`, which both ends already own | a multi id | no |
| a design component | `HouseDesign` on the house entity | the house's serial | no |
| a `Cover` | `Overlay`, keyed by tile | a tile | no |

The fourth carries **no graphic at all** — `Cover` is *"no graphic, no serial, no
entity"* — and that is the point rather than a limitation: the live layer holds
what physics needs, because both ends already know the art from somewhere else.
The client draws a house from its own `multi.mul` and an item from the packet
about that item; neither needs the overlay to tell it what a thing looks like.

## 6. What each route costs

Measured on the shipped Felucca — 458,752 blocks, 2,906,871 statics — because
the numbers are what keep this document from being a matter of taste.

| | |
|---|---|
| put something in the **live layer** | **0** — no bake is over it |
| `WorldMap::replace_blocks`, no block's item count changed | 0.1 ms |
| `WorldMap::replace_blocks`, a count changed | 0.02 ms at the end of the statics run to 1.3 ms at its start, plus 7 ms once per world for the reallocation `from_parts`' `shrink_to_fit` makes unavoidable for the first *added* item |
| `SpanIndex::build` — the rebake, paid at **both** ends | **55 ms** |
| the coarse graph, after N4 | 11.7 s |

**So the cost of a patch is the rebake, not the write.** Anyone reaching for a
cheaper way to get statics into the base is optimising 0.1 ms next to 55 ms. The
work that would pay is a bake with a seam smaller than a facet —
[`navigation_spans.md`](navigation_spans.md)'s N8 — and it is queued there, not
here.
