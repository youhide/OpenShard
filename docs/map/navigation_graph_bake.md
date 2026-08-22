# Offline bake for the navigation graph

## Why

`NavigationGraph::build` is too expensive to run during startup on a production
facet. A local run against Britannia (`7168x4096`) sampled terrain in 7.8s,
partitioned it into 311,296 regions in 8.4s, and found 1,355,438 portal nodes
in 18.2s before entering the much longer intra-region routing phase. The
in-process shard cannot become ready before that work finishes; the client
would then construct a second, independent copy of the same graph.

The graph is derived solely from static client files and the routing rules. It
therefore belongs in an offline artifact, not on either process's critical
startup path. This follows the same lifecycle as `openshard-art.table`: build
once beside an install, validate at load, and explicitly remake it when stale.

## Outcome

One baked graph exists for every `(client install, facet, graph-format)` tuple.
Both the server and client load that artifact. Neither quietly rebuilds it in a
normal run.

The artifact is a cache for long-distance routing only. The existing live,
bounded pathfinding remains authoritative for every actual step, so doors and
other dynamic obstructions retain their current behaviour.

## Artifact contract

`openshard-movement` gains `navigation::bake`, owning the format and all of
these operations:

- `stamp_of(client_dir, facet)`: inspect the map and tile-data inputs;
- `save(path, graph, stamp)`: atomically write one complete artifact;
- `load(path, expected_stamp)`: validate and return a `NavigationGraph`;
- typed errors for absence, incompatibility, staleness, and corruption.

The default path is beside the client install:

```text
openshard-navigation-<facet>.bin
```

An environment variable and a CLI `--out` option override that path, for
read-only installs.

The binary header contains:

- magic bytes and a format version;
- a routing-algorithm version, bumped whenever `NavigationGraph::build` or its
  movement semantics change;
- facet number and map dimensions;
- the input stamp.

The payload contains the fully constructed graph: regions, tile-to-region
lookup, nodes, per-region node lists, and adjacency lists. Loading it must not
call `Terrain` or run pathfinding.

### Freshness

The stamp covers exactly the static inputs that determine walkability:

- the selected facet's map source (`map<facet>LegacyMUL.uop`, or the legacy map
  and statics inputs selected by `Map::load_facet`);
- `tiledata.mul`;
- map dimensions and selected facet;
- routing-algorithm version.

Metadata/length stamps are sufficient initially, matching `artscan`'s policy:
they reliably distinguish an install revision without making every startup hash
hundreds of megabytes. A stale or unrecognised artifact is never used.

**A facet from a base set stamps different inputs.** When the world comes out of
`openshard-basemap` rather than out of the install, the map and statics files
are no longer what the graph was built from — and they are still sitting there
with their old lengths and mtimes, so stamping them would *pass* and hand a
player a graph of a world it has never seen. `bake::stamp_of_base_set` names the
base set and `tiledata.mul` instead. The `Stamp` also carries the source
revision either way, which is `docs/map/new_map_representation/plan.md`'s
direction D arriving one caller early; D is where the file stamps go away and
the revision becomes the whole key.

## Bake command

Provide a dedicated native CLI, initially in `openshard-movement`:

```sh
OPENSHARD_CLIENT="/path/to/Ultima Online Classic" \
  cargo run --release -p openshard-movement --bin openshard-navigation-bake -- --facet 0
```

Options:

- `--client DIR` (also `OPENSHARD_CLIENT`);
- repeatable `--facet N`, defaulting to `0`;
- `--out FILE` for a one-facet explicit destination;
- `--base-set FILE` for a one-facet build over an OpenShard base set instead of
  the install's map and statics (`--client` is still required, for
  `tiledata.mul`). The artifact defaults to *beside the base set*, and the stamp
  names the base set — see below;
- `--dry-run`, which builds and reports but writes nothing.

The command loads the same `MapTerrain` that runtime used, prints each build
phase, region/node/edge counts, output bytes, and the final path. It writes via
a temporary file in the destination directory, flushes it, then renames it, so
a client never observes a partially written graph.

The development bootstrap documentation should show `artscan` and navigation
bake as separate, explicit preparatory commands.

## Runtime integration

### Server

`boot::load_world` loads and validates the graph for each configured facet
before it calls `World::with_facet`. `World::with_facet` receives the already
loaded `Option<NavigationGraph>` and stores it in `FacetState`; it no longer
calls `NavigationGraph::build` on the ordinary path.

For a shard configured with `world.client_files`, missing, stale, malformed, or
wrong-facet graph data is a startup error by default. The error reports the
artifact path, exact reason, and a ready-to-run bake command.

An explicitly named development-only setting may permit `build_at_startup`, but
it must be opt-in and loudly logged. It is not a fallback that production or
the playground silently enters.

### Client

The client loads the same artifact after it has loaded the facet and tile data.
It stores the returned graph in `Resources::coarse`; it does not invoke
`NavigationGraph::build`. An offline map viewer without a valid graph still
opens, but disables only long-distance route planning and prints the bake
command needed to enable it.

### Playground

The in-process shard validates and loads first. Once ready, the client loads
the same baked file. There are no two independent runtime constructions.

## Error policy

Errors distinguish:

- no artifact at the expected path;
- wrong magic or unsupported format version;
- invalid facet or map dimensions;
- stale input stamp or routing-algorithm version;
- truncated or malformed payload.

Every message includes the path and an actionable bake command. A stale graph
must not be accepted: it can propose corridors inconsistent with the map that
the server authorizes.

## Verification

1. Unit round-trip: build, save, load, then compare dimensions and all graph
   data needed by queries.
2. Route parity: `find_long_path` gives the same result for built and loaded
   graphs over synthetic terrains, including portals and unreachable goals.
3. Rejection tests: wrong version, facet, dimensions, input stamp, and
   truncated/corrupted files.
4. Server and client wiring tests prove valid baked data does not call
   `NavigationGraph::build`.
5. Manual smoke: bake facet 0, then run `cargo run -p openshard-playground`;
   logs must reach shard readiness and client startup without navigation build
   phases.
6. Keep a manual release benchmark recording bake duration, load duration, and
   artifact size for a real install.

### Pre-v4 facet 0 baseline

Measured on 2026-08-12 against a 7168×4096 post-ML Britannia install, release
build, with the bake isolated in a cgroup using `MemoryMax=2G` and
`MemorySwapMax=0`:

- bake: 96.3 seconds, about 1 GiB peak memory;
- graph: 28,672 regions, 140,456 nodes, 2,104,020 directed edges;
- artifact: 265,082,856 bytes;
- debug shard cold load: 2.13 seconds for the graph, 765 MiB peak for the whole
  shard through readiness.

The superseded exact-row-run partition took 603.7 seconds and produced
1,355,438 nodes plus a 513,896,076-byte artifact. Its median region held only
seven cells and 92.3% of regions were one tile wide or tall: it effectively
emitted topology around trees and coastline corners. The bounded 32×32 regions
are the measured fix; obstacles inside a region remain authoritative terrain
but do not emit graph nodes.

Format v4 replaces the artifact's per-tile `RegionId` table with a 1-bit
walkability map and writes nodes, region membership, and adjacency compactly.
Re-run this benchmark before publishing its new size and cold-load figures;
the source tree intentionally carries no client map installation to bake here.

## Implementation order

1. Define binary format, stamp, reader/writer, and movement-crate tests.
2. Add the `openshard-navigation-bake` CLI with atomic writes and reporting.
3. Change server boot and `World::with_facet` to receive baked graphs.
4. Change client resource loading to use baked graphs.
5. Add explicit development-only runtime-build escape hatch, if still useful.
6. Update development documentation and run workspace checks plus real-install
   bake/playground smoke tests.

## Acceptance criteria

- A normal configured shard and `openshard-playground` never build a navigation
  graph at runtime.
- Server and client reject the same stale or invalid artifact deterministically.
- A valid graph is built once offline and loaded by both processes.
- Missing graph data produces a short actionable diagnostic, not an apparently
  hung startup.
