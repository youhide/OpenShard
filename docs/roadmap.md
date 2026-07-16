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
- [x] `World::tick` — a fixed 20Hz timestep; commands in, events and packets out
- [x] Core components: `Position`, `Heading`, `Body`, `Name`, `Client`, `Movement`
- [x] Domain events: `PlayerEntered`, `MobileMoved`, `StepRefused`, `PlayerLeft`
- [x] Spatial index — a 64-tile sector grid, Chebyshev range
- [x] Other mobiles: 0x77/0x78/0x1D, and the `seen` set that sends each once
- [x] Character creation (0x00 and 0xF8), not just playing a configured name
- [x] Multiple facets — `[world] facets`, terrain and interest per facet

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

### The tick

`World::tick` is the deterministic half of the boundary the gateway's channel
draws. Commands queue from network tasks and are applied in a fixed order at a
fixed rate; nothing inside a tick awaits, reads a clock or touches a socket.

That is what makes anything that happens *without* a client asking possible at
all — decay, regeneration, an NPC deciding to move. It is also what makes replay
possible: the same commands produce the same world.

Two things worth knowing:

- **`select!` is `biased`** so the tick cannot be starved. Without it a flood of
  packets keeps `recv` ready forever and the world stops simulating under
  exactly the load that needs it most.
- **A late tick does not catch up.** `MissedTickBehavior::Delay`, because running
  several ticks back-to-back turns a hiccup into a stall and a fixed timestep
  into a variable one.

**What is still missing:** persistence. The world is built at start and lost
at stop.

Two players do now see each other. Verified over real TCP, on the real map:
each is drawn on the other's screen exactly once, steps arrive as `0x77`,
walking past 18 tiles sends `0x1D` and walking back re-draws, and a dropped
connection takes the mobile off every screen that had it.

## 4. Persistence

- [x] Persistence queue, drained outside the tick
- [x] SQLite backend — `SqliteStore`, tested
- [x] Save and load accounts and characters
- [x] Serial reservation on load — `Registry::reserve_serial`, for load-on-play
- [x] Crash recovery — the boot load restores the world; a played character
  returns on its saved serial and spot
- [ ] PostgreSQL backend — the same `Store` trait, for operators who want it

Two backends, one choice. A shard runs on SQLite or on PostgreSQL, and which is
the operator's to make: neither is "the production one", and SQLite runs a real
shard perfectly well. Some will want a text file or a Postgres cluster; the
`Store` trait is the seam that lets any of them sit behind the same simulation.

`persistence.database` in the config picks the file; empty keeps the world in
memory, the same bargain as running with no map, and the shard says so. A
logged-out character lives as a row, not an entity: its serial is reserved at
boot so nothing new can take it, and playing it (`0x5D`) spawns it back on that
serial, at its saved position, looking as it did. Characters save as they change
and on logout, through the same journal the tick already feeds.

**Three things it is worth knowing before touching this:**

- **The dirty marks come from the event bus.** Nothing calls `journal.touch()`
  by hand. A system that moves a mobile already emits `MobileMoved`, because
  that is how the client hears about it; persistence reads the same event. There
  is no line to forget.
- **Logout uses `Journal::keep`, not `touch`.** A touch is a promise to read the
  entity at the next save, and the entity is about to be despawned. Logout is
  when a save matters most, so the record is taken before the despawn. There is
  a test with that name.
- **A failed write costs a full sweep, not a rollback.** Re-writing the failed
  snapshot would put everyone back where they were when the write started. The
  world is marked dirty instead and the next save reads it fresh.

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
