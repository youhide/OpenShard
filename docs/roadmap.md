# Roadmap

Order, not dates.

## 0. Foundation — done, unverified

- [x] Cargo workspace, all 20 crates declared
- [x] `entities` — generational `EntityId`, UO `Serial`, sparse-set columns, `Registry`
- [x] `events` — double-buffered `Events<E>`, `Cursor<E>`, `EventBus`
- [x] `protocol` — `ClientVersion`, `Era`, `Feature`, `FeatureSet`
- [ ] **`cargo test --workspace` actually run** — written without a toolchain available

## 1. Protocol

- [ ] `PacketReader` / `PacketWriter` over `bytes`
- [ ] Login sequence: `0xEF` seed, `0x80` login, `0xA8` shard list, `0x8C` relay
- [ ] Compression (server→client Huffman) and login encryption
- [ ] Packet tests against captured dumps from real clients
- [ ] Version negotiation from the `0xBD` seed packet → `FeatureSet` per connection

Version-gate everything from the first packet. Retrofitting is the thing this
crate exists to avoid.

## 2. Gateway and login

- [ ] Tokio listener, one task per connection
- [ ] Framing, backpressure, disconnect handling
- [ ] Account verification (`config`-driven, no DB yet)
- [ ] Shard list and relay to the game server

## 3. World — first vertical slice

The goal: **a ClassicUO client logs in and walks around.**

- [ ] `world` crate: the tick loop, composing `Registry` + `EventBus`
- [ ] Map loading from client MUL/UOP files
- [ ] Spatial index (sectors) and "what can this player see"
- [ ] Core components: `Position`, `Graphic`, `Body`, `Name`
- [ ] `movement`: walk, run, fastwalk rejection
- [ ] Character creation and login into the world

Nothing else. No combat, no items, no database.

## 4. Persistence

- [ ] Persistence queue, drained outside the tick
- [ ] SQLite backend first (dev), PostgreSQL after
- [ ] Save and load accounts and characters
- [ ] Serial reservation on load — `Registry::bind_serial` already handles this
- [ ] Crash recovery

## 5. Scripting

The largest open technical risk. Prove it before building gameplay on top.

- [ ] `deno_core` embedded, one V8 isolate
- [ ] `ScriptEngine` trait — narrow enough that the runtime stays replaceable
- [ ] Entity and event bindings exposed to TypeScript
- [ ] Hot reload without a restart
- [ ] **Benchmark**: script call overhead inside a tick at realistic entity counts

If the numbers do not hold, this is where the design has to change — which is
why it comes before gameplay depends on it.

## 6. Gameplay

Roughly in dependency order, each script-first:

- [ ] `items` — containers, stacking, equipment layers, decay
- [ ] `combat` — swing timers, damage, resistances, notoriety
- [ ] `skills` — usage checks, gain curves
- [ ] `magic` — spells, reagents, casting
- [ ] `ai` — brains, aggro, wandering
- [ ] `chat` — speech, journal routing
- [ ] `housing`, `guilds`

## 7. Scriptpack conversion

- [ ] `tools/cli`: one-shot `.scp` → TS/TOML converter
- [ ] Run it over `Sphere/Scripts-X`, review the output by hand

A build-time tool that runs once, not an engine feature. The output is committed
and edited as normal source afterwards — there is no ongoing `.scp` dependency.

## 8. Operations

- [ ] `config` — TOML, validated at load
- [ ] `metrics` — tracing, Prometheus, health endpoints
- [ ] `plugins` — manifests, lifecycle, enable/disable
- [ ] REST API + JWT
- [ ] `tools/dashboard` — Next.js admin panel
- [ ] `tools/launcher`, `tools/map-editor`

## Later

LLM NPCs, quest generation, GM assistant, Discord integration. All optional, all
after the engine stands on its own.
