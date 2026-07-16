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

## 3. World — a client walks in Britannia

- [x] `Direction` / `Facing` — steps ported verbatim from Sphere's `sm_Moves`
- [x] World entry: 0x5D, 0x1B, 0xBF.0x08, 0x20, 0x4F, 0x55
- [x] `movement`: the walk handshake, turning as a step, the world edge
- [x] `WalkSequence` — 0 means fresh, 255 wraps to 1, a reject resets both ends
- [x] `tiledata.mul` — both layouts, told apart by arithmetic
- [x] UOP containers — the map is in `map0LegacyMUL.uop`, not `map0.mul`
- [x] `map*.mul` / `statics*.mul` — column-major blocks, 2.9M statics
- [x] `MapTerrain` — real heights, walls, water, the two-unit step limit
- [x] `WalkPace` — a token bucket; a client can no longer walk as fast as it sends
- [ ] `world` crate: the tick loop, composing `Registry` + `EventBus`
- [ ] Spatial index (sectors) and "what can this player see"
- [ ] Core components: `Position`, `Graphic`, `Body`, `Name`
- [ ] Character creation (0x00), not just playing a configured name
- [ ] Multiple facets; only map0 is wired up

**Three things about the client file formats that are not written down
anywhere**, each of which parses cleanly and produces a plausible, wrong world
if guessed:

- **`map0.mul` may be a stub.** It can be 90MB of zeroes, at exactly the right
  size. The real map is `map0LegacyMUL.uop`. Reading the stub raises no error
  and yields a flat, empty, perfectly smooth world.
- **UOP entries need not be in index order.** Sorting by file offset — the
  obvious shortcut — scrambles the map. The entries are named by a 64-bit hash
  and it has to be computed.
- **The UOP hash packs its halves `(b << 32) | c`.** Jenkins' own signature is
  `hashlittle2(key, len, &pc, &pb)`, so `(c << 32) | b` is the natural reading.
  It matches zero entries.

### The pace limiter takes Sphere's numbers and not its arithmetic

The intervals are Sphere's — 200ms on foot, 100ms running — and those are worth
having: two decades of tuning against real clients.

The arithmetic is ours. Sphere's `Event_Walking` keeps a running average in
milliseconds and clamps it against `WALKBUFFER`, which defaults to `15` — a
duration compared against what its own docs call a count of "points". Read
literally, a normal walker sits at a balance of 15ms and one early step puts it
at `15 - 200 = -185`, refused instantly, with none of the burst tolerance the
buffer exists to give. Either the constant means something undocumented or the
check does not do what it says. `movement::WalkPace` is a token bucket instead:
the same intent, stated plainly.

**What is still missing:** the tick. `game.rs` answers packets as they arrive
rather than simulating anything, and it is the placeholder the world crate grows
out of. Until it exists there is no place for anything that happens *without* a
client asking — decay, regeneration, an NPC deciding to move.

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
- [ ] Run it over a scriptpack, review the output by hand

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

## A note on client files

None are in this repository and none will be: they are copyrighted and not ours
to redistribute. `world.client_files` points at an install the operator already
has. Tests that need one read `OPENSHARD_CLIENT` and skip when it is unset.

What this project contains is readers for the *formats*. Nothing is derived from
any particular shard's data, and nothing should be documented as if it were.
