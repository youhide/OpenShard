# The client's map today, measured

What direction A takes a handle to, stated as facts about the code and numbers
off a real Felucca install (7168×4096, `statics0.mul` = 2,906,871 statics in
120,744 non-empty blocks). [`plan.md`](plan.md) lists *who* reads the world;
this lists *what they read* and what the reading costs, so a decision in
directions A–D can be argued against a measurement rather than an impression.

## The one owner, and its shape

[`Map`](../../../crates/common/uofiles/src/map.rs#L180) is flat, immutable, and
whole in memory:

| Field | Layout | Felucca |
|---|---|---|
| `cells: Vec<LandCell>` | blocks column-major, cells row-major; `LandCell` is 4 bytes | 29,360,128 × 4 B = **112 MiB** |
| `statics: Vec<Vec<StaticItem>>` outer | one `Vec` header per block | 458,752 × 24 B = **10.5 MiB**, of which 338,008 are empty |
| `statics` payload | `StaticItem` is 10 bytes, sorted by `(y, x)`, stably | 2,906,871 × 10 B = **27.7 MiB** in **120,744 separate allocations** |
| **Total** | | **≈ 150 MiB**, peaking near 260 MiB during load (the decompressed UOP is held beside the built `cells`) |

Derived artifacts, as they stand:

| Artifact | In memory | On disk | Keyed by |
|---|---|---|---|
| [`NavigationGraph`](../../../crates/common/movement/src/navigation.rs#L28) | walkable bitset 3.7 MiB + nodes/edges | 8.5 MiB | input file name, size, mtime |
| [`BuildingMap`](../../../crates/client/render/src/interiors.rs#L1025) | `Arc<[u32]>`, one label per tile = **112 MiB** | **112 MiB**, raw `u32`, uncompressed | same |
| `RadarCache` chunks | 8 KiB per 64×64 chunk, **no eviction** | — | `(facet, lod, chunk, revision)` |
| Art table (surfaces) | small | 219 KiB | same |

## Backlog

Ranked by what a person would notice first.

1. **The world map draws at LOD 0.** [`render_passes.rs:636`](../../../crates/client/app/src/render_passes.rs#L636)
   asks for `RadarLod::BASE` across the whole facet to fill a 640×458 window
   where one pixel covers about eleven tiles: 7,168 chunks of 8 KiB, and at the
   queue's eight builds a frame roughly nine hundred frames before it is full.
   `MAX_LOD = 4` is 448×256 — the window — and
   [`build_ready_ancestors`](../../../crates/client/render/src/radar.rs#L1064)
   builds that level and nothing reads it. The pyramid is *reduce-only*: a
   parent exists only once all four children do, so it cannot answer "show the
   whole facet cheaply" at all. A coarse producer that samples `Map` directly
   is the missing piece, not a different LOD request.
2. **An O(facet) scan every frame.** With the world map open,
   [`presentation.rs:2017`](../../../crates/client/app/src/presentation.rs#L2017)
   allocates and sorts a 7,168-element `Vec` of chunk coordinates and probes the
   cache's `BTreeMap` once per element, per frame.
3. **`RadarCache` never evicts.** Its own test says so
   (`"eviction is not implemented yet"`). One pass over the world map retains
   about 75 MiB of chunks for the rest of the run.
4. **The revision dimension has no production writer.** `set_revision` and
   `invalidate_tile` are called only from tests, because the client's `Map`
   cannot change at runtime. It is the right preparation for direction D and it
   must not be mistaken for working invalidation.
5. **The building flood's artifact is 112 MiB of raw `u32`** — one label per
   tile, overwhelmingly zero (the exterior), written and read four bytes at a
   time ([`interiors.rs:240`](../../../crates/client/artscan/src/interiors.rs#L240)),
   which is 29 million bounds-checked reads on the startup path. Run-length or
   a sparse per-block index would cut it by orders of magnitude.
6. **`Vec<Vec<StaticItem>>` is 120,744 allocations.** Sized and weighed under
   [`plan.md`'s direction B](plan.md#what-felucca-measures-before-the-layout-is-chosen):
   a CSR pair takes the statics layer from 38.2 MiB to about 13.5 MiB and from
   120,745 allocations to two.
7. **Both ends load the same install separately** —
   [`boot.rs:618`](../../../crates/server/server/src/boot.rs#L618) and
   [`lib.rs:461`](../../../crates/client/app/src/lib.rs#L461). Under
   `openshard-playground` that is one process holding ~300 MiB of facet twice.
   The handoff already names the correctness half of this; the memory half is
   the same fix.
8. **`Map` does not know its own facet.** `describe_size` names a *size*;
   `Facet` travels separately in every bake stamp and radar key, and the client
   pins [`FACET: u8 = 0`](../../../crates/client/app/src/lib.rs#L245) with a
   single `Arc<Map>`. The Malas/Ter Mur ambiguity is closed at load time and
   then the answer is thrown away. Direction A's snapshot should be keyed per
   facet from the first commit rather than being one handle that later grows a
   facet dimension.
9. **The navigation bake spikes 235 MiB transiently** — `vec![None; cells]` of
   `Option<Point>`, 8 bytes a tile. A walkable bitset plus an `i8` height array
   is 33 MiB for the same information.
10. **"The highest static on a tile" is re-derived by linear scan in four
    places** — the radar, `MapTerrain`, cutaway, occlusion — because the sort
    key is `(y, x)`. It cannot become a z-sort here: file order *is* draw order
    and `statics::pick` breaks ties by taking the last. Our own chunk format
    should separate the two deliberately — store draw order as a field and sort
    by z — which turns every one of those scans into a suffix lookup.

## What a house weighs

Houses do **not** enter `statics` today: one arrives as a single item whose
graphic is `0x4000 | id` and is expanded at draw time through
[`Multis`](../../../crates/common/uofiles/src/multi.rs#L270). They are entities,
not terrain. But they are the density case any future layout has to survive,
so measured off the shipped `multi.mul` (800 multis, 62,177 components):

| Multi | Components | Tiles | Densest 8×8 block |
|---|---|---|---|
| Castle (126/127) | **3,667** | 31×32 | **339** |
| id 5000 | 3,016 | 52×29 | 225 |
| Keep (124/125) | 2,251 | 24×24 | 271 |
| Mean of all 800 | 77.7 | | |

Against Felucca's terrain — median 18 per block, p99 122, max 467 — a castle
puts nineteen times the median into one block, and a *customised* house has a
per-house component list bounded by nothing that ships. Two consequences:

- It is an argument about chunk size, recorded in
  [`mechanics.md`](mechanics.md#chunks): at 8×8 a castle is sixteen chunks and
  a moved wall touches one of them; at 64×64 it is inside one, and that wall
  rewrites 4,096 tiles' worth of chunk.
- It is the argument that a flat base array must never be inserted into.
  Placing a castle into a CSR base would memmove about 11 MiB. That is not a
  reason against the flat layout — it is the flat layout refusing to let a
  house be anything but an overlay, which is the model direction C already
  chose.

## The access pattern, and where it actually hurts

Worth stating because the intuition "a 150 MiB array read through a camera must
thrash" is right about one half and wrong about the other.

`cells[i]` where `i = (block_x * blocks_down + block_y) * 64 + y_local * 8 +
x_local`, `blocks_down = 512` on Felucca:

| step | offset in memory |
|---|---|
| next tile east, same block | +4 B |
| next tile south, same block | +32 B |
| next block south | +256 B |
| **next block east** | **+131,072 B (128 KiB)** |

So the layout is column-major in blocks — southwards is contiguous, eastwards
is a 128 KiB stride — while
[`for_each_static_in`](../../../crates/client/render/src/statics.rs#L1184) and
the ground walk both run **rows, `x` innermost**. The walk is transposed
against the layout.

**The land is fine anyway.** A block is 64 cells × 4 B = 256 B = exactly four
cache lines. Walking a row takes 32 B from each of about 24 blocks; the next
row takes the next 32 B of the same 24. Over eight rows every block is
consumed whole — full line utilisation, and a working set of 6 KiB. A
widest-zoom rectangle is roughly 187×187 tiles (the 35,000 lookups
`for_each_static_in` names), which is 576 blocks and 147 KiB touched out of
112 MiB resident. The 1997 tiling picked the cache line's size.

**The statics are not.** Reaching one block means a 24-byte header out of the
outer array and then a pointer into one of 120,744 separate allocations
scattered across 28 MiB of heap. That is finding 6, and it is the half of the
intuition that is correct.

**And the rectangle is walked three to five times a frame**, independently:
[`statics::collect_in_with_fades`](../../../crates/client/render/src/statics.rs#L547),
[`occlusion::collect_with_interior`](../../../crates/client/render/src/occlusion.rs#L2765) —
whose own doc says it walks *deliberately* rather than using the block bake —
[`light::collect_with_interior`](../../../crates/client/render/src/light.rs#L976),
plus `statics::pick` on cursor movement and the ground's own land walk. Same
576 blocks, same binary searches, from scratch each time. That is a larger
lever than the layout, and no direction in the plan currently owns it.
