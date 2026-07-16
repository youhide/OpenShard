# Roadmap

Order, not dates.

## 0. Foundation — done

- [x] Cargo workspace, all 20 crates declared
- [x] `entities` — generational `EntityId`, UO `Serial`, sparse-set columns, `Registry`
- [x] `events` — double-buffered `Events<E>`, `Cursor<E>`, `EventBus`
- [x] `protocol` — `ClientVersion`, `Era`, `Feature`, `FeatureSet`
- [x] `cargo test --workspace` green: 125 tests, clippy clean, fmt clean

## 1. Protocol — mostly done

- [x] `PacketReader` / `PacketWriter` — std only, every read fallible
- [x] Client packet length table ported from Sphere's `receive.h` (70 packets)
- [x] `frame_client_packet` — split a TCP stream into packets
- [x] Seed handshake state: old 4-byte form, new `0xEF` form, lone-`0xEF` segment
- [x] Login sequence: `0x80`, `0x82`, `0xA8`, `0xA0`, `0x8C`, `0x91`, `0xA9`
- [x] `0xBD` client version report → `ClientVersion` → `FeatureSet`
- [x] Server→client Huffman compression (Sphere's "golden key" table)
- [ ] Login encryption — see below
- [ ] Packet tests against captured dumps from real clients

Version-gate everything from the first packet. Retrofitting is the thing this
crate exists to avoid.

The codec deliberately has no dependencies — not even `bytes`. Keeping the
foundation crates dependency-free is what lets them build in environments where
crates.io is unreachable.

### Login encryption is deliberately deferred

Sphere ships `sphereCrypt.ini`: a per-client-version key table for the login
stream, and separate game-stream encryption. It is a real lift and it buys
nothing — the keys are extracted from the client binary, so anyone can read the
stream. It is obfuscation, not security.

ClassicUO connects with encryption off, which is what freeshards use in
practice. So: support unencrypted first, get a client on screen, and revisit
only if a real client turns up that cannot be configured without it. Do not
mistake this for a security feature when it lands.

## 2. Gateway and login — done

- [x] Sans-io `Connection`: handshake then framing, no async, no sockets
- [x] Tokio listener, one task per connection, events onto a channel
- [x] Disconnect handling; every protocol violation is fatal
- [x] `Accounts` trait + `DevAccounts` in-memory store
- [x] Sans-io `LoginServer`: 0x80 → 0xA8 → 0xA0 → 0x8C → 0x91 → 0xA9
- [x] Auth key issued at relay, one-shot, expiring, bound to its account
- [x] `crates/server` — a binary that runs and reaches a character list
- [x] `config` — TOML, validated at load; accounts and addresses come from it
- [x] A fresh checkout writes a default `openshard.toml` and runs

`config` refuses to start on a wildcard `advertise` rather than accepting it and
failing silently for every remote client. That check is the reason the crate
exists; parsing TOML is three lines of serde.

The connection logic is a pure state machine on purpose. Everything hard about a
gateway is byte boundaries — a seed split across three segments, four packets in
one read — and a real socket will not reproduce those on demand. As a state
machine each one is a deterministic test with no ports and no sleeps.

`Server` hands events to a channel rather than calling back. A callback would run
world code inside a network task, on whatever thread Tokio picked, whenever bytes
arrived. The channel is where async stops and the tick begins.

## 3. World — the slice walks; the world does not exist

The goal was **a client logs in and walks around**, and it does.

- [x] `Direction` / `Facing` — steps ported verbatim from Sphere's `sm_Moves`
- [x] World entry: 0x5D, 0x1B, 0xBF.0x08, 0x20, 0x4F, 0x55
- [x] `movement`: the walk handshake, turning as a step, the world edge
- [x] `WalkSequence` — 0 means fresh, 255 wraps to 1, a reject resets both ends
- [x] `crates/server/src/game.rs` — a player enters and walks, over real TCP
- [ ] `world` crate: the tick loop, composing `Registry` + `EventBus`
- [ ] Map loading from client MUL/UOP files
- [ ] `Terrain` for real: heights, blocking, statics
- [ ] Spatial index (sectors) and "what can this player see"
- [ ] Core components: `Position`, `Graphic`, `Body`, `Name`
- [ ] Character creation (0x00), not just playing a configured name

**What the slice actually proves.** The packets are right and the walk handshake
is right. It does not prove there is a world: `movement::OpenWorld` allows every
step, so a player walks across water, through walls and off cliffs. `game.rs`
holds a `Registry` with a serial in it and no components at all.

That is deliberate. The handshake is protocol — finishable now, and pinned. The
map is a project of its own, and `Terrain` is the seam it lands on without
touching any of the above. What `game.rs` must not do is grow: the moment it
wants components, the `world` crate is what it wants.

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

- [x] `config` — TOML, validated at load
- [ ] `metrics` — tracing, Prometheus, health endpoints
- [ ] `plugins` — manifests, lifecycle, enable/disable
- [ ] REST API + JWT
- [ ] `tools/dashboard` — Next.js admin panel
- [ ] `tools/launcher`, `tools/map-editor`

## Later

LLM NPCs, quest generation, GM assistant, Discord integration. All optional, all
after the engine stands on its own.
