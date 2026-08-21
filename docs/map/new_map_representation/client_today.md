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
6. **`Vec<Vec<StaticItem>>` is 120,744 allocations.** A CSR pair — one
   `Vec<StaticItem>` and a `Vec<u32>` of block offsets — is one allocation,
   10 MiB smaller, better for locality, and trivially mappable or shareable.
   Direction B should not inherit the current shape by default.
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
