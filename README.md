# OpenShard

Modern open-source MMORPG server engine compatible with classic Ultima Online
clients.

Compatible with the UO **protocol** — the 2D client and ClassicUO — and with
nothing else. OpenShard is not a SphereServer clone. It is an attempt at the
engine Sphere would likely be if it were designed from scratch today: Rust,
data-oriented, script-first, hot-reloadable, observable.

> **Status: a small world lives.** `cargo run -p openshard-server` loads the
> client's map and takes clients through login and character creation into a
> ticking, shared world. Characters walk and run the same ground the client
> draws (the step rules are the client's own), pick things up, fill backpacks,
> wear clothes, fight creatures that fight back, gain skills, cast, chat, and
> bank with a banker who knows their name. The world saves itself to SQLite or
> PostgreSQL without ever pausing, and survives a restart. Gameplay is
> TypeScript, hot-reloaded on save. See [`docs/roadmap.md`](docs/roadmap.md).

## Design

- **Everything is an entity.** No inheritance trees. Players, NPCs, items,
  houses and boats differ only by which components they carry.
- **Systems emit events; they do not call each other.** Combat emits
  `MobileDied`. Whoever cares reads it. Plugins, logging, metrics and replay
  fall out of this rather than being threaded through.
- **The tick is deterministic.** Commands queue, one fixed order applies them,
  randomness comes from a seeded rng the tick owns. Replay the same commands
  and you get the same world.
- **The world lives in memory.** The database is persistence, never a query
  target during a tick — and a save never stops the world.
- **Multi-era from day one.** Code asks what a client *can do*, never what
  version it is.
- **Gameplay is TypeScript.** Hot reloadable, no restart.
- **No global state, no `unsafe`.**

Read [`docs/architecture.md`](docs/architecture.md) for the reasoning.

## Architecture

Arrows are dependencies; they only point down.

```mermaid
graph TD
    C["clients<br/>(ClassicUO / 2D client)"] -. UO protocol .-> server
    server["server — the binary<br/>boot · accept loop · packet dispatch · sessions"]
    login["login<br/>accounts · auth"]
    scripting["scripting<br/>TypeScript on embedded V8"]
    world["world<br/>the tick · client file formats · persistence journal"]
    systems["gameplay systems — fn(&amp;mut WorldState)<br/>combat · chat · items · skills · magic · ai · npc"]
    state["state<br/>WorldState: registry · event bus · sectors · seeded rng · interest"]
    foundation["entities · events · protocol · gateway · movement · persistence · config"]
    db[("SQLite / PostgreSQL")]

    server --> login
    server --> scripting
    server --> world
    world --> systems
    systems --> state
    state --> foundation
    foundation --> db
```

The tick sequences the systems in a fixed serial order — that is the price of a
deterministic, replayable simulation, and it is paid on purpose. A script is one
more consumer of the same seam every system uses: events in, commands out, never
a direct write to the world.

## Layout

```
crates/
  entities      ECS: EntityId, Serial, sparse sets, Registry
  events        double-buffered typed event bus
  protocol      versions, feature gates, packets, codec, framing
  gateway       sans-io connection + Tokio listener
  login         accounts, auth keys, the whole login sequence
  movement      the walk handshake, terrain rules, A* pathfinding
  state         WorldState: components, sectors, rng, interest
  combat        damage, swings, notoriety, murder counts
  chat          speech in, speech out, speech ranges
  items         containers, drag/drop, stacking, decay, doors
  skills        checks, the gain curve
  magic         casting, mana, typed damage
  ai            creature brains: notice, chase, drift
  npc           townsfolk: bankers, creature spawning
  world         the tick, client map/tiledata formats, the journal
  persistence   journal, snapshots, SQLite and PostgreSQL stores
  scripting     the TypeScript runtime (deno_core)
  config        TOML, validated at load
  server        the binary — glue only
  housing guilds metrics plugins                    stubs, future
tools/
  dashboard launcher map-editor cli                 planned
```

## Running

```sh
cargo run -p openshard-server
```

The first run writes an `openshard.toml` and starts on `0.0.0.0:2593` with a dev
account of `admin` / `hunter2`.

Point `world.client_files` at a UO client install to get a map. Without one the
shard still runs, but every step is allowed — players walk through walls and
across water.

Set `persistence.database` to a file path (SQLite) or a `postgres://` URL to
keep the world across restarts. Neither is a tier — SQLite runs a live shard
fine. Empty means in-memory: a real development mode, and the shard says so at
startup rather than implying it saves.

The one setting worth reading before you touch anything else is
`server.advertise`. It is **not** `server.listen`: it is the address the server
tells clients to dial, so it defaults to `127.0.0.1` and only works on the
machine running the shard. Behind NAT it must be your public address.

## Building

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all
```

All three are expected to be silent.

## Stack

Rust + Tokio. SQLite or PostgreSQL, operator's choice. TypeScript via embedded
V8 (`deno_core`) for gameplay. React and Next.js for tooling.

## Licence

MIT OR Apache-2.0.
