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
- [x] Starting cities — the nine classic Felucca towns, filtered to the loaded
  facets; a new character spawns in the one it picked
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
- [x] PostgreSQL backend — `PgStore`, the same `Store` trait, tested against a
  live server

Two backends, one choice. A shard runs on SQLite or on PostgreSQL, and which is
the operator's to make: neither is "the production one", and SQLite runs a real
shard perfectly well. Some will want a text file or a Postgres cluster; the
`Store` trait is the seam that lets any of them sit behind the same simulation.

`persistence.database` picks the backend by what it looks like: a `postgres://`
URL connects to PostgreSQL, anything else is a SQLite file path, and empty keeps
the world in memory — the same bargain as running with no map, and the shard says
so. A logged-out character lives as a row, not an entity: its serial is reserved
at boot so nothing new can take it, and playing it (`0x5D`) spawns it back on that
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

**Two things specific to the PostgreSQL backend:**

- **It connects with `NoTls`.** Enough for a database on the same host or a
  trusted network, which is where a first backend earns its keep. An encryptor is
  a later, additive change and does not touch the shape — `PgStore` is one
  connection behind an async mutex, the same shape as SQLite's, because a
  transaction borrows the client and saves are off the tick either way.
- **`tokio-postgres` used to be pinned, and no longer is.** From 0.7.13 it pulls
  a crypto stack (RustCrypto 0.11, `rand` 0.10) that wanted Rust 1.85 — above the
  1.82 MSRV of the time — so the lock held it at 0.7.12. The scripting spike (§5)
  raised the MSRV to 1.88, which cleared the constraint, and the pin was dropped;
  the crate floats on `"0.7"` again. See the `Cargo.lock` note in `CLAUDE.md`.

## 5. Scripting — spike done

The largest open technical risk. Proven before building gameplay on top, and it
holds. The engine is `crates/scripting`; `engine.rs` explains the seam.

- [x] `deno_core` embedded, one V8 isolate — `DenoEngine`, one `JsRuntime`
- [x] `ScriptEngine` trait — four methods, nothing V8-shaped in a signature, so
  the runtime stays replaceable
- [x] Entity and event bindings exposed to TypeScript — domain events in through
  `deliver`, a read model a hook reads through `op_position`, commands out
  through `op_move`; ops declared with `extension!` and `#[op2]`, all synchronous
- [x] Hot reload without a restart — `load` rebinds the hooks in the live
  isolate; `reload_if_changed` polls a watched file's mtime
- [x] **Benchmark** — `examples/benchmark.rs`, numbers below

### The numbers

The question was whether a per-entity hook fits the tick. The budget is
`TICK_INTERVAL`: **50ms at 20Hz**. Measured on an Apple-silicon dev machine, V8
hosted in a Tokio runtime, release build, warmed up so the JIT has tiered the
hook. `cargo run -p openshard-scripting --example benchmark --release`.

| Hook | per call | 10k mobiles/tick | share of a 50ms tick |
|---|---|---|---|
| empty (`onTick(){}`) — pure Rust↔V8 crossing | ~170 ns | ~1.7 ms | ~3% |
| read + maybe move — `op_position`, then conditionally `op_move` | ~490 ns | ~4.9 ms | ~10% |

The realistic hook — the one a gameplay rule looks like: read the mobile's tile
through an op, decide, and on a condition enqueue a step — costs about half a
microsecond a call. Ten thousand mobiles each firing it every tick spend roughly
a tenth of the budget. **It fits, with room.**

Two honest caveats. The ceiling is *script* time only; a real tick also moves
mobiles, runs interest management and writes packets, so the script share is a
slice of the 50ms, not all of it — the per-call nanoseconds are the number that
travels, not the "calls per tick" ceiling. And the crossing cost is per call, so
a design that calls one hook over a batch of entities will always beat one that
crosses per entity; that is a knob for §6, not a problem for the spike.

The design does not have to change. Gameplay can depend on it.

## 6. Gameplay

Roughly in dependency order, each script-first:

- [x] **The script is wired into the tick.** The bridge §5 deferred: the server
  owns a `DenoEngine`, delivers each tick's domain events to it, and queues the
  commands it emits for the next tick. `scripting.main` in the config names the
  script; empty runs scriptless, the same bargain as an empty map. A script acts
  through `Command::Step` — server-authoritative movement, no client sequence or
  pace, terrain the only judge — which is the first thing a script command lands
  on. `crates/server/src/scripting.rs` is the whole seam.
- [x] `items` — containers, stacking, equipment layers, decay
  - [x] **On the ground and visible.** A script drops an item
    (`op_spawn_item` → `Command::SpawnItem`) and every client in range is sent
    the `0x1A` that draws it; walking up to one draws it, walking away sends the
    `0x1D`, exactly as for a mobile. Items are entities like anything else — a
    `Graphic` and a `Position`, drawn through the same `seen`/interest machinery
    as bodies. A stack carries an `Amount`. The `WorldItem` (`0x1A`) encoder is
    ported from Sphere's `PacketItemWorld`, flag bits and all.
  - [x] **Pick up and drop** (`0x07`/`0x08`). The client's own item loop: lift
    an item onto the cursor and set it back on the ground. The world holds it in
    limbo — off the sector grid, off every screen but the picker's — and
    remembers where it came from, so a drop out of reach or a logout mid-drag
    bounces it back rather than losing it. A refused lift or drop is a `0x27`
    drag-cancel with a reason. Server-authoritative reach (`ITEM_REACH`), no
    trust in the client's claim. Ground-to-ground only; dropping *into* a
    container is the next slice, and it bounces for now.
  - [x] **Containers** (`0x06` open, `0x24`/`0x3C`/`0x25`). A container is an
    item that also carries a `Container` (its gump); items inside carry a
    `Contained` and no `Position` — the two are exclusive, on the ground *or* in
    a container, never both. Double-click opens it (`0x24` + the `0x3C` contents
    list); dropping onto its serial puts the item inside (`Contained` + a `0x25`
    to the open gump); lifting a contained item drops the containment. A drop
    onto a non-container, or out of reach, bounces to origin — and origin is now
    "the ground *or* the container it was in", so a cancelled drag always undoes
    cleanly. Live updates go to the acting client only; a second viewer re-opens
    to refresh (a noted limitation, not a bug). The `0x24`/`0x25`/`0x3C` version
    seams (High Seas type word, `ItemGrid` grid byte) are gated on `Feature`, not
    era.
  - [x] **Equipment layers** (`0x13` wear, `0x2E` equipped). A worn item carries
    an `Equipped { mobile, layer }` and no `Position`/`Contained` — the third and
    last place an item can be, all three exclusive. Dragging an item onto a
    paperdoll (`0x13`) wears it: the layer is checked free, the wearer reachable,
    and a `0x2E` goes to everyone who can see the mobile. A newcomer sees a
    dressed mobile because the `0x78` now lists what it wears (it sent an empty
    list before). Lifting a worn item takes it off. A held item's origin is now
    "ground, container, *or* mobile", so every cancelled drag still undoes to
    exactly where it came from.
  - [x] **Stacking, split and decay.** A `Stackable` item merges with an
    identical pile (same graphic and hue) dropped onto it — amounts sum, clamped,
    the dragged one despawns, the survivor is redrawn past the `seen` set.
    Picking up part of a pile splits it: the `0x07` amount is honoured, and —
    read out of Sphere's `CItem::UnStackSplit` rather than guessed — the original
    keeps its serial and holds the taken amount on the cursor while a new dupe is
    left on the ground with the remainder, so the client's cursor and its drop
    still name the same object. Ground items carry a `Decays { at_tick }` and rot
    when the tick counter reaches it; lifting, containing or wearing takes the
    clock off, and `decay()` reads only its own counter, no wall clock.
    Containers do not decay with their contents inside.
- [x] `combat` — swing timers, damage, resistances, notoriety
  - [x] **Hit points, damage and death.** Mobiles carry `Hitpoints`; scripts
    spawn creatures (`op_spawn_mobile` → `Command::SpawnMobile`, an entity with a
    body and no client, drawn through the same interest machinery as a player)
    and damage them (`op_damage` → `Command::Damage`). A blow lowers hits and
    redraws the `0xA1` bar — the mobile itself sees the real numbers, everyone
    else a percentage, so a stranger's exact health never crosses the wire. At
    zero it emits `MobileDied`, which the server delivers to scripts, so loot,
    notoriety and quests hang off death without combat knowing they exist — the
    "systems emit, they do not call" rule made concrete. A creature is removed on
    death; a player stays (ghosts and corpses are a later slice).
  - [x] **The interactive layer.** A player toggles war mode (`0x72`, echoed
    back settled) and picks a target (`0x05` → `0xAA`); a `Combat` component
    holds the stance, the target and the next-swing tick. `swings()` runs each
    tick: a combatant in war mode with a target within `MELEE_RANGE` on the same
    facet strikes when its timer is up, out of reach it waits with its timer
    unspent, and a killed target ends the attack. The timer is a tick count, like
    decay — no clock in the tick. A `SwingSpeed` component sets the cadence per
    mobile, script-set at spawn; the stand-in for what UO derives from a weapon's
    speed and the wielder's dexterity, neither of which exists yet.
  - [x] **Resistances and the damage formula.** A swing's damage is no longer
    flat: `melee_blow` takes the attacker's `MeleeDamage` and cuts it by the
    target's `Resistance { physical }`. Both are components a script sets when it
    spawns a mobile (`op_spawn_mobile` grew `damage` and `resistance`), so a
    hard-hitting ogre or an armoured knight is a data change, not a code one — the
    script-first part. Physical only for now; the other damage types land with
    magic.
  - [x] **Notoriety and criminal flagging.** Mobiles carry a `Notoriety` (the
    enum already in the protocol), drawn as the health-bar colour in every
    `0x78`/`0x77` — the world stopped hardcoding "innocent". A script sets it at
    spawn; an invulnerable (yellow) mobile cannot be attacked. Raising a hand
    against someone blue or green turns the attacker grey — a `CriminalUntil`
    flag, its expiry a tick count like decay, broadcast to every watcher with a
    `0x77`. **Murderer** flagging (the red a repeat killer earns) is deferred: it
    needs a persistent count of whom you have killed, which is a karma/reputation
    system with nothing yet to hang it on.
  - Deferred, on purpose, because each waits on something not built: **the other
    damage types** (fire, cold, poison, energy) want a source of typed damage,
    which is spells (`magic`); **weapon- and dexterity-derived swing speed and
    damage** want stats and weapon properties; **murderer/karma** wants a
    reputation store. The seams are in place — `Resistance` has room for more
    types, `SwingSpeed` and `MeleeDamage` are already per-mobile — so each is a
    fill-in, not a redesign.
- [ ] `skills` — usage checks, gain curves
- [ ] `magic` — spells, reagents, casting
- [ ] `ai` — brains, aggro, wandering
- [ ] `chat` — speech, journal routing
- [ ] `housing`, `guilds`

The bridge is event-driven today: the server calls the script's `onEvent`, not a
per-mobile `onTick`. The per-entity hook the benchmark measured is what `ai`
(wandering, aggro) will want, and wiring it is a server-loop change when that
lands — the engine already supports it. The script vocabulary — the events in,
the commands out — grows one gameplay area at a time, each new command mapped in
`into_world`.

The balance data comes from the SphereServer scriptpack (`Scripts-X`): `items/`,
`skills/`, `spells/`, `npcs/`, `crafting/`. Numbers taken, arithmetic audited —
the same bargain as everywhere else Sphere is read.

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
