# OpenShard

Modern open-source MMORPG server engine compatible with classic Ultima Online
clients.

Compatible with the UO **protocol** — the 2D client and ClassicUO — and with
nothing else. OpenShard is not a SphereServer clone. It is an attempt at the
engine Sphere would likely be if it were designed from scratch today: Rust,
multi-core, data-oriented, script-first, hot-reloadable, observable.

> **Status: early, but it runs.** `cargo run -p openshard-server` listens on
> 2593 and takes a client through login to a character list. There is no world
> behind it yet. See [`docs/roadmap.md`](docs/roadmap.md).

## Design

- **Everything is an entity.** No inheritance trees. Players, NPCs, items,
  houses and boats differ only by which components they carry.
- **Systems emit events; they do not call each other.** Combat emits
  `NpcKilled`. Whoever cares reads it. Plugins, logging, metrics and replay fall
  out of this rather than being threaded through.
- **The world lives in memory.** The database is persistence, never a query
  target during a tick.
- **Multi-era from day one.** Code asks what a client *can do*, never what
  version it is.
- **Gameplay is TypeScript.** Hot reloadable, no restart.
- **No global state, no `unsafe`.**

Read [`docs/architecture.md`](docs/architecture.md) for the reasoning.

## Layout

```
crates/
  entities      ECS: EntityId, Serial, Registry          implemented
  events        double-buffered typed event bus          implemented
  protocol      client versions, feature gates, packets  versioning only
  gateway       sans-io connection + Tokio listener      implemented
  login         accounts, auth keys, the whole sequence  implemented
  config        TOML, validated at load                  implemented
  server        the binary                               implemented
  world         the tick loop and spatial index          stub
  combat movement ai items magic skills housing
  guilds chat persistence scripting plugins
  metrics                                                stubs
tools/
  dashboard launcher map-editor cli                      planned
```

## Running

```sh
cargo run -p openshard-server
```

The first run writes an `openshard.toml` and starts on `0.0.0.0:2593` with a dev
account of `admin` / `hunter2`.

The one setting worth reading before you touch anything else is
`server.advertise`. It is **not** `server.listen`: it is the address the server
tells clients to dial, so it defaults to `127.0.0.1` and only works on the
machine running the shard. Behind NAT it must be your public address.

## Building

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

## Stack

Rust + Tokio. PostgreSQL in production, SQLite in development. TypeScript via
embedded V8 (`deno_core`) for gameplay. React and Next.js for tooling.

## Licence

MIT OR Apache-2.0.
