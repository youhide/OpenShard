<p align="center">
  <img src="docs/logo.png" alt="OpenShard" width="360">
</p>

# OpenShard

An open-source MMORPG engine — a shard and a client of its own — compatible with
the Ultima Online **protocol**, and with nothing else. Not a SphereServer clone:
an attempt at the engine Sphere would be if it were designed today. Rust,
data-oriented, deterministic, observable, and content the compiler checks.

> **Status: a small world lives, and we draw it ourselves.**
> `cargo run -p openshard-playground` puts a shard and our own client in one
> process and logs in. The stock 2D client and ClassicUO stay first-class — the
> server is written to the protocol, never to our client.

See [`docs/README.md`](docs/README.md) for the as-built picture of every area,
[`plans/`](plans/README.md) for what is not built yet, and
[release notes](docs/release_notes.md) for player-facing changes.

## What works today

Honest list. `✅` is shipping and exercised by tests; `🟡` is usable with a named
hole; each line points at the page that says where it actually stands.

### The world and the map

- ✅ **The map is our own format, not a `.mul` reader.** A facet is a base set of
  **64×64-tile chunks** plus a log of patches; one revisioned snapshot is what
  every reader holds. UO's `map*.mul` is *an* importer — 7,168 chunks, 102.6 MiB,
  byte-identical round trip — and a world that never came from an install is the
  point of the split.
- ✅ **Chunks arrive over the wire.** Whole chunks travel to our client deflated
  over `0xBF`/`0xE000`, so an operator's `.setland` reaches a connected screen
  without a restart. Derived data — the span layer, the statics run, the coarse
  navigation graph — *follows* a publish instead of being dropped by it.
- ✅ **Three layers, one type**: ground, statics, and the live layer over them.
  What may be baked is exactly what is below the live layer, and that rule is the
  shape of the value rather than a comment about it.
- ✅ Pathfinding on both ends: a span layer over 29.4 M columns, a coarse graph of
  places with directed portals, A\* over it. → [`docs/world/`](docs/world/README.md)

### The shard

- ✅ Login, character creation, a ticking shared world; walking and running by the
  client's own step rules; interest by sector.
- ✅ **Items**: one canonical location (ground, in a container, worn), stacking,
  weight and capacity, drag and drop, secure trade, doors and keys, mounts,
  chairs, decay. Identity is `ItemKindId + MaterialId`; the `Graphic + Hue` the
  classic client needs is derived from it.
- ✅ **Two indexes, paid on mutation rather than on request.** A recursive **craft
  stock** per container root — so ingots in a bag inside a chest inside the pack
  count as materials, with a ceiling that refuses an adversarial subtree instead
  of stalling the tick — and a permissioned, paginated **house inventory search**
  over everything locked down in a building.
- ✅ **Crafting**: nine trades, 612 recipes, ServUO's odds and gump encoding, a
  workshop scan that reads statics as well as items. Every gate is checked twice,
  the withdrawal plan reserves each pile once, and the output is prepared before
  an ingredient is spent — a craft that fails consumes nothing, not even
  randomness. Smelting, and the material chains end to end: ore → ingots,
  hides → leather, fibre → thread → bolt → cloth, plus the field and the sheep at
  their head. → [`docs/items/`](docs/items/README.md)
- ✅ Combat with real behaviour behind it — line-of-sight aggro, chase, kite,
  flee — skills on a live gain curve, the 64-spell table with timed buffs and
  poison that survive a relog, vendors and bankers, quests, parties, guilds,
  houses and boats.
- ✅ **The whole world saves without pausing** — to SQLite or PostgreSQL, every
  NPC, door, debuff and scribed spellbook — and survives a restart.
- ✅ `GET /metrics` (Prometheus) and `GET /health`, off by default.

### Our own client

`crates/client/` — the wire in the other direction, drawn on `wgpu` with no
engine underneath. → [`docs/client/`](docs/client/README.md),
[`docs/render/`](docs/render/README.md)

- ✅ **Lighting is a deferred model, calibrated against a path tracer that ships
  beside it.** A fragment carries a world position, a measured normal, an albedo
  and the identity of the primitive it is a point of; it is lit by
  `albedo × max(N·L, 0) × colour × intensity × windowed-inverse-square ×
  visibility` over eight stratified samples of a spherical flame, in linear
  radiance, tonemapped once by an ACES fit. Every constant that stood in for a
  missing measurement is deleted, and each deletion was gated by injecting the
  fault. **⬜ The sun and the ambient day curve are not written**, and a mobile's
  normal is still half done.
- ✅ **The camera is off the body.** ClassicUO pins the eye to the body to the
  pixel, so the view inherits every discontinuity of the walk. Ours is a rig with
  a bench: which camera is right is measured against a scripted walk rather than
  argued. 🟡 Several stages of the pipeline are built and empty (the spring, the
  intent, the anchors).
- ✅ **Routing of its own** over this end's reading of the ground, on a thread
  that is not the one drawing: click a door and it is named, click the
  unreachable and the refusal has a reason.
- ✅ The picture: ground stretched over four corner heights, textured statics and
  mobiles, one CPU ordering all three passes share; map-block LOD off the block's
  projected footprint; picking against the picture that was actually drawn.
- ✅ Twelve kinds of window, each owning its state and input behind one router —
  containers, the paperdoll, the skill sheet, the status frame, `0xB0` dialogs,
  and a **typed craft table**: the catalogue and the workbench are structured
  payloads, not a generic gump, with availability derived on the client and
  re-checked on the server for the one recipe chosen.
- ✅ **The global map and the minimap are one raster.** A pyramid of radar chunks
  built straight from the map at any level, a view that picks its own level from
  tiles-per-pixel with hysteresis, a producer budgeted in time and an evicting
  cache — so the facet map zooms from a corner of a street out to the whole of
  Britannia ([`design_radar.md`](docs/world/design_radar.md), `1.25^steps`,
  `steps ∈ -8..=12`) without ever asking for level zero
  of the world, which is 57 MiB of chunks nobody can hold. Levels 2 and coarser,
  for the entire facet, cost 4.8 MiB. The world's own map moves under it: a
  published patch invalidates the chunks it named.
- ✅ Sound and music out of the player's own archive.
- ⬜ **Nothing blends yet** — one missing pass, and five features waiting behind
  it. 🟡 A body whose frames live only in the UOP containers draws nothing.
  🟡 The shop is a shelf: no price column, no Buy button. One session at a time.

**On the browser, honestly.** The renderer was built to WebGL2's ceiling on
purpose — no compute, no storage buffers, instancing through vertex buffers, a
2048 atlas, `async` device requests — because that is cheap to honour from the
first triangle and painful to retrofit, and a client that streams what a scene
needs instead of shipping a three-gigabyte install is worth keeping reachable.
What has to be said next to that: **WASM is a crippled target.** It has no real
threads, and this client already plans a route on a thread that is not the one
drawing. Take that away and the choice is a stall or a stale answer, which is
exactly the lag a detached camera and predicted walk exist to remove. So the
browser stays a *constraint on the design* rather than a promise about a build,
and nobody should read the WebGL2 discipline above as "it runs in a browser
today". It does not.

### Past the protocol, by extension

**We have gone a little past what the UO protocol says, and it should be said
plainly.** The shard streams *its own* map and the statics standing on it, opens
a craft catalogue without a tool in hand, searches a house's inventory, reports
the stages of a combat action, takes a turn as its own request, and edits the
ground while it runs. The reference protocol has a packet for none of that.

**All of it is one mechanism**: `0xBF` subcommands at or above `0xE000` —
twenty-five of them today, and no private packet *id* anywhere. The range is the
whole argument. Every subcommand a shipped client speaks is at or below `0x2B`
and ClassicUO's own private one is `0xBEEF`, so `0xE000` is out of reach of both;
a stock client reads `0xBF`'s length out of the envelope and **skips a subcommand
it does not know**, where a private id would desynchronise its stream for good.
Beyond that, only a client that asked is answered — a stock client never sends
`ChunkRequest`, so nothing but two short notices ever reaches one, and it drops
those.

**What it costs, stated as plainly:** a stock client draws the world on its own
disk. The shard still judges every step against *its* map, so where the two
disagree the stock client is simply refused a step it thought was fine; an
operator's `.setland` never reaches its screen, and a facet that never came out
of an install has nothing there for it to draw at all. Ours sees the shard's
world. That is the line, and it is drawn where a client that ignores an extension
is a client with less in it — never one that breaks.
→ [`design_chunks_to_the_client.md`](docs/world/design_chunks_to_the_client.md)

## Design

- **Everything is an entity.** No inheritance trees. Players, NPCs, items, houses
  and boats differ only by which components they carry.
- **Systems emit events; they do not call each other.** Combat emits
  `MobileDied`; whoever cares reads it. Logging, metrics and replay fall out of
  this rather than being threaded through.
- **The tick is deterministic.** Commands queue, one fixed order applies them,
  randomness comes from a seeded rng the tick owns. Replay the same commands and
  you get the same world.
- **The world lives in memory.** The database is persistence, never a query
  target during a tick — and a save never stops the world.
- **Multi-era from day one.** Code asks what a client *can do*, never what
  version it is.
- **Gameplay is Rust; content is data in this repository.** A rule is a
  `fn(&mut WorldState)` in a domain crate; a table of more than a hundred rows is
  `data/*.json` compiled by a `build.rs`. **There is no scripting language and
  there will not be one** — the embedded V8 that used to hold content is deleted,
  decided in the open on [#7](https://github.com/youhide/OpenShard/issues/7) and
  [#17](https://github.com/youhide/OpenShard/issues/17). A shard needs no second
  repository: point it at a client install and it comes up furnished.
- **No global state, no `unsafe`.**

Read [`docs/architecture.md`](docs/architecture.md) for the reasoning.

## Layout

Three groups, and the direction of dependency is the point: `server` may depend
on `common`, `client` may depend on `common`, and the two never see each other.
Everything both sides of the wire agree on lives in `common/protocol`.

```
crates/
  common/       what both ends agree on
    protocol      versions, feature gates, packets, codec, framing
    entities      ECS: EntityId, Serial, sparse sets, Registry
    map           the world: chunks, patches, snapshots, the live overlay
    basemap       where a base set and a patch log live on disk
    tiles         the tile table, with no file reader in it
    uofiles       the importers: map, art, anim, gump, sound, fonts
    movement      the walk handshake, terrain rules, A* and the coarse graph
    events        double-buffered typed event bus
    commands · config · metrics · pathlog
  server/       the shard
    gateway       sans-io connection + Tokio listener
    login         accounts, auth keys, the login sequence
    state         WorldState: components, sectors, rng, interest, the indexes
    combat        damage, swings, volleys, poison, notoriety, murder counts
    items         locations, containers, stacking, decay, doors, mounts, trade
    crafting      nine trades, 612 recipes, smelting
    magic         the 64-spell table, casting, typed damage, timed buffs
    skills · chat · ai · npc · quests · party · guilds · housing · boats
    world         the tick, the map service, the journal
    persistence   journal, snapshots, SQLite and PostgreSQL stores
    server        the binary — glue only
    plugins       a stub
  client/       our own client, beside the stock one
    net           the client's half of the wire: framing, login, a world view
    model         read models the wire and presentation layers share
    render        the isometric renderer and its lighting
    app           the binary: a window, a surface, a camera
    editor · gump-render · artscan · pathtrace   tools that keep it honest
  e2e/          both ends in one process: playground, shard, egui-capture
```

## Running

```sh
cargo run -p openshard-playground   # a shard in a thread + our client, no port bound
cargo run -p openshard-server       # the shard as a real service on :2593
```

The server's first run writes an `openshard.toml` and starts on `0.0.0.0:2593`
with a dev account of `admin` / `hunter2`.

- **`server.advertise` is not `server.listen`.** It is the address the server
  tells clients to dial, so it defaults to `127.0.0.1` and only works on the
  machine running the shard. Behind NAT it must be your public address. It is the
  single most likely reason a client hangs.
- **`world.client_files`** points at a UO client install. Without one the shard
  still runs, but every step is allowed — players walk through walls and water.
- **`persistence.database`** takes a file path (SQLite) or a `postgres://` URL.
  Neither is a tier; SQLite runs a live shard fine. Empty means in-memory, and
  the shard says so at startup rather than implying it saves.
- **`metrics.listen`** turns on `/metrics` and `/health`. No authentication —
  bind it to loopback.

`cargo run -p openshard-client-app` without `--account` is an **offline map
viewer**, not a client: there is no network in it at all.

## Building

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all
```

All three are expected to be silent, and CI runs exactly these on every pull
request.

## Contributing

Work lands through a pull request against a protected `main`. See
[`CONTRIBUTING.md`](CONTRIBUTING.md) for the flow, [`docs/style.md`](docs/style.md)
for how the code reads, and [`docs/findings.md`](docs/findings.md) for the traps
already paid for.

Questions and project discussion are welcome on
[Discord](https://discord.gg/GKa46DdAG9).

**No Ultima Online client files are in this repository and none ever will be.**
They are copyrighted. Point the engine at whatever install you already have.

## Related projects

Other Rust work on the same client, worth reading before reinventing a wheel:

- [broker0/path_server](https://github.com/broker0/path_server) — a UO server in
  Rust, and [ungine7](https://github.com/broker0/ungine7), the same author's
  later workspace (MIT): packets, protocol detection and encryption, client file
  parsers, world and movement, with example servers, clients and proxies.
- [AngryLawyer/uo-rust-libs](https://github.com/AngryLawyer/uo-rust-libs) —
  libraries for the client's data files (`.mul` / `.uop` art, map, tiledata).
- [hulryung-uo/anima-client](https://github.com/hulryung-uo/anima-client) —
  a UO client written from scratch in Rust, dual-licensed as we are.

## Licence

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option — the Rust ecosystem's convention, and what every crate in the
workspace has always declared. Copyleft was considered and dropped: a shard
operator runs the server rather than distributing it, so the GPL's condition is
never triggered, and it would put `crates/common/protocol` out of reach of every
neighbouring project on this wire.

Unless you state otherwise, any contribution you intentionally submit for
inclusion in this work is dual-licensed as above, with no additional terms.
