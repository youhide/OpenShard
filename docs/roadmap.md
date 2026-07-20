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
- [ ] **Character deletion** (`0x83`) — the packet is in the length table but has
  no handler; the delete button on the character-select screen does nothing. Needs
  the store to forget the row and the serial to stay reserved (a packet in flight
  may still name it).
- [ ] **Store-backed accounts and password hashing** — the login still verifies
  against `DevAccounts` from `openshard.toml`; the store half already exists
  (`AccountRecord`, `Store::accounts`/`put_account`) but is not wired into the
  `Accounts` trait the `LoginServer` reads, and the password is plaintext until a
  slow hash (argon2/bcrypt) lands — `record.rs` and `accounts.rs` both say so.

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
- [x] **The movement check matches the 2D client**, a blend of both references:
  ServUO/RunUO's `GetStartZ`+`Check` for *reach* (a step reaches the top of the
  surface underfoot plus two, not the feet — the fix for slope rubber-band) and
  Sphere's `GetFixPoint` for *selection* (stand on the highest surface in reach,
  not the nearest — the fix for climbing building stairs). See the note below.
- [x] `MobileStatus` (`0x11`) — the status bar, and the only packet carrying
  **stamina**; without it the client sees zero stamina and silently refuses to
  run. Sent on world entry and answered on `0x34`. Versioned 3–6 by
  `status_packet_version` (type 6 is the 121-byte High Seas shape).
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

### The walk check is one part ServUO, one part Sphere

The client draws z it computes itself — the walk ack carries none — so the server
has to land a step on the *same* height the client does or every step
rubber-bands. Neither reference alone matches the 2D client; the working check
takes one half from each.

- **Reach is ServUO's `GetStartZ`+`Check`.** A step reaches `start_top + 2`, where
  `start_top` is the top of the surface the mobile stands on — a sloped land
  tile's highest corner, a stair's full height — not its feet. Reaching from the
  feet (`from_z + 2`) refuses steps up a slope the client took: measured against a
  real facet, that was 10,620 steps around Britain the server blocked and the
  client allowed. Land reachability is the tile's *lowest* corner and you stand at
  its `GetAverageZ` centre, floored toward negative infinity.
- **Selection is Sphere's `GetFixPoint`.** Among the surfaces in reach, stand on
  the **highest**, not — as ServUO's `Check` does — the one nearest the current
  height. A stair tile carries the floor below it and the step above; ServUO's
  nearest-z keeps you on the floor while the client climbs, so building stairs
  "drop" you and you cannot get in. The highest-in-reach rule climbs them.

The two rules agree on bare ground — one surface, so highest *is* nearest — which
is why the ServUO half tested clean on open terrain and the divergence only
surfaced on stacked geometry (stairs, house floors). The whole of it is
`MapTerrain::check` / `start_surface`, ported with the arithmetic audited as
everywhere else.

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
- [x] **Item persistence** — a character's carried inventory (worn gear, the
  backpack and everything nested inside it) and loose ground clutter survive a
  restart. `ItemRecord` is the saved shape; `SCHEMA_VERSION` moved to 2. An
  inventory is saved as a unit — the store replaces everything under an owner
  rather than diffing item by item, walked live for an online character and kept
  at logout like the character record; the ground is a full sweep, decoration
  excluded (a pack re-lays that). On boot the item serials are reserved and ground
  items placed; a returning character re-equips its saved inventory instead of a
  starter backpack. Items keep their serials across a restart so a container's
  contents still point at it.
- [x] **A save is complete, and shutdown flushes it.** Consistency, because it is
  gold and gear: every save writes *every online character* in full — record and
  whole inventory — not only the ones that moved, because picking an item up takes
  no step and so never marks a character dirty; the ground is swept every save, not
  only when someone was active; and a logout re-fills the in-memory
  pending-inventory cache so a **re-login in the same run** re-equips what it
  carried (before the fix it lost the backpack). And the shard **saves on the way
  out**: Ctrl-C, or the gateway stopping, takes one last full snapshot, closes the
  save channel and *awaits* the writer so every queued transaction lands before the
  process exits — unlike the per-tick handoff, because the one moment a lost write
  costs a player real value is the last one.
- [x] **`Stackable` persists, the save interval is a config line, and `.save`
  forces one.** An item's `Stackable` flag is saved (`ItemRecord`, schema v3), so a
  restored gold pile still merges with more rather than losing the flag until
  re-lifted. `persistence.save_seconds` sets the periodic cadence (0 = only shutdown
  and `.save`; a save never stops the world, so this is only how much a crash could
  cost). And a staff **`.save`** (GM+) takes an immediate snapshot and tells every
  player "the world is being saved" — the old shards' announce **without** their
  pause, because OpenShard's snapshot is an instant memcpy, not a synchronous walk
  of the world.
- [x] **Spawn regions persist, timers and all.** A populated area stays populated
  across a restart without re-running `.admin`, and — the point — a rare spawn keeps
  its remaining wait: killed with hours to go, it comes back with those hours ahead
  of it, not popping again the moment the shard is up. `SpawnerRecord` (schema v4)
  saves the region, its creatures and the timer as the **seconds still to wait**,
  not a tick count (which resets at boot) or a wall-clock time (the tick reads no
  clock) — so downtime pauses the timer rather than eating it, the semantics chosen
  for a rare spawn. Registering a region twice replaces it rather than stacking a
  second, and after a restart the regions come from the store, not the pack, so a
  re-populate is not needed and the timers hold.
- [x] **The save is the whole world (schema v5), the Sphere/ServUO model.** Every
  live NPC mobile — townsfolk, vendors with their priced stock, spawner creatures
  with their current wounds and `SpawnedBy` link (`MobileRecord`) — and every
  placed decoration, door open/shut state included (`DecorationRecord`), is swept
  into each snapshot and restored at boot exactly as it stood. A killed creature is
  simply absent from the sweep and stays dead, its region's saved timer counting
  down; nothing re-populates at boot, so a staff `.admin` Populate/Decorate seeds a
  fresh world **once** and the save is the truth thereafter. Both references walk
  every mobile and item to save (ServUO's `World.Save`, Sphere's `CWorld::SaveStage`)
  and never regenerate the world — this reaches the same end without stopping the
  world to do it. A ridden mount in limbo is the one mobile not swept: its ride
  persists through the saddle item on the rider, and `dismount` reconstitutes the
  creature whole.
- [x] **Stats and trained skills persist (schema v6).** A `CharacterRecord` carries
  str/dex/int and every trained skill with its lock arrow; character creation finally
  *applies* the stats and skills the player picked, threaded through
  `Command::Enter` as a `CharacterSheet` — for a new character from the create packet
  and for a played one from the save. The `0x3A` skills window follows a live gain.
- [x] **Active effects persist (schema v7).** Poison and the timed stat buffs are
  saved with their mobile as an `EffectRecord` list on the character or mobile row,
  so a relog cannot wash a debuff off — see the `magic` effects work in §6 for the
  shape (`World::effects_of`/`apply_effects`, the ledger-only restore for buffs).

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
  - [ ] **Stack merge inside a container** — only ground stacks merge today; a
    stack dropped onto an identical pile *inside* a container bounces instead of
    merging (`items/stack.rs` says so). A refinement, not a redesign.
  - [ ] **Partial lift honours the amount everywhere** — the `0x07` amount is
    honoured for ground splits; a partial lift out of a container takes the whole
    item.
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
    mobile as an explicit override, but with no override the pace is now *derived*
    from the wielder's dexterity through Sphere's pre-AoS formula
    (`CResourceCalc.cpp`, era 1): swing tenths = `(15000 · 10) / ((dex + 100) ·
    base)`, wrestling base 50, so a `dex 100` mobile swings every 1.5s and a
    nimbler one sooner. Weapon `base` is still wrestling for everyone — the one
    input left, waiting on weapon tiledata properties.
  - [x] **Resistances and the damage formula.** A swing's damage is no longer
    flat: `melee_blow` takes the attacker's `MeleeDamage` and cuts it by the
    target's `Resistance { physical }`. Both are components a script sets when it
    spawns a mobile (`op_spawn_mobile` grew `damage` and `resistance`), so a
    hard-hitting ogre or an armoured knight is a data change, not a code one — the
    script-first part. Melee is physical; the other damage types arrived with
    `magic`.
  - [x] **Notoriety and criminal flagging.** Mobiles carry a `Notoriety` (the
    enum already in the protocol), drawn as the health-bar colour in every
    `0x78`/`0x77` — the world stopped hardcoding "innocent". A script sets it at
    spawn; an invulnerable (yellow) mobile cannot be attacked. Raising a hand
    against someone blue or green turns the attacker grey — a `CriminalUntil`
    flag, its expiry a tick count like decay, broadcast to every watcher with a
    `0x77`. **And murderer flagging is real** — the red a repeat killer earns. A
    `Murders` count tallies innocents killed (attributed in `swings`, where the
    killer and the blue victim are both known); the fifth turns the killer red for
    good. Unlike the lapsing grey flag it is persistent, so `expire_criminality`
    now restores a mobile's *base* standing — murderer if the tally stands, else
    innocent — rather than always washing it blue. Attribution is *not*
    melee-only: `damage` takes an `attacker`, and a script's `op_damage`/spell
    carries a `by` serial, so a fireball that kills a blue is a murder the same as
    a sword; unattributed damage kills without blame. And old kills fade — a
    `MurderDecay` clock ages one count off at a time, washing a reformed killer
    back to blue once it drops below the threshold. (Sphere's separate short- and
    long-term counts are a finer model this stands in for.)
  - The **typed damage** this once deferred landed with `magic` below: `damage`
    takes a `DamageType` (physical, fire, cold, poison, energy) and cuts it by the
    target's `Resistance` for that type, in the one place all damage passes — melee,
    spell, poison pulse and script alike.
  - [ ] **Weapon properties from tiledata** — weapon-derived swing speed and
    damage (the dexterity half is done; the weapon `base` still needs the item's
    tiledata properties). The seams are in place — `SwingSpeed` and `MeleeDamage`
    are already per-mobile — so it is a fill-in, not a redesign. This is also what
    gates **combat eras 3–5** in the `[gameplay]` config: only 1 (pre-AoS) and
    2 (AoS) are accepted at load until weapons carry real properties.
  - [ ] **Ghosts, corpses and resurrection** — a player who dies today stays
    standing at zero hits (despawning someone connected is worse); the real slice
    is a corpse container holding the gear, a ghost state the living cannot see
    through, and resurrection (the spell is in the table, tagged `Scripted`,
    waiting on exactly this).
  - [ ] **Stamina as a real pool** — the status bar sends `stamina = dexterity`
    so the client will run at all; a real pool spends on running, refills on the
    tick counter like mana, and is the number a war-mode push-through should cost.
  - [ ] **Combat visuals and sounds** — a swing lands with no animation (`0x6E`),
    no blood, no sound (`0x54`); the encoders do not exist yet. Same protocol
    slice as the spell effects below, worth doing together.
- [x] `skills` — usage checks, gain curves
  - [x] **The check and the gain.** A mobile carries `Skills` (a sparse map of id
    → tenths). A script sets one (`op_set_skill`) and uses it against a difficulty
    (`op_use_skill`); the world rolls success on an S-curve of the gap between
    skill and difficulty — ported straight from Sphere's `Calc_GetSCurve`, 50% at
    parity, 75% ten points ahead — and rolls a gain that falls as the skill
    rises. The result comes back as a `SkillUsed` event the server delivers to
    scripts, so the reward — the ore, the pick's turn — is the script's to grant,
    combat's `MobileDied` decoupling again.
  - [x] **A seeded generator in the world.** A roll is randomness inside a tick,
    and the tick must replay. So `Rng` (xorshift64\*) is a plain field the world
    owns, seeded once from a fixed default and advanced only by the tick — two
    identical runs reach the same skill, roll for roll (there is a test that
    asserts exactly this). A live shard that wanted unpredictable rolls seeds
    from the clock and saves the seed; additive, one value.
  - [x] **stats** (str/dex/int), the foundation combat's weapon/dexterity-derived
    numbers were waiting on. A mobile carries `Stats { strength, dexterity,
    intelligence }`; `enter` gives a character the classic 100/100/100 and derives
    its `Hitpoints.max` from strength and `Mana.max` from intelligence, the UO
    identity where those bars *are* the stats. `Command::SetStats` (op `op_set_stats`)
    re-caps both when a stat changes, dragging a current value down under a lowered
    cap and leaving room to heal into a raised one. Dexterity is stored now and
    read next, by the swing speed below.
  - [x] **The skills window on the client** (`0x3A`, ported both ways from ServUO's
    `SkillUpdate`/`SkillChange`). Login sends a player its whole list — every skill
    the client's era knows, trained or not, each with value, base, lock arrow and
    cap (`SkillCaps`-gated for the cap field) — a gain pushes the single-line update
    so an open window follows it live (`SkillRaised`, from the one `roll_skill`
    every gain passes through), and the up/down/lock arrow the player clicks rides
    back as `SkillLockRequest`, stored per skill on `Skills`. Skills and stats
    persist with the character (schema v6, §4), applied from the creation screen or
    the save through the `CharacterSheet`.
  - [ ] **Route client-initiated skill use** (`0x12` type `0x24`, decoded by
    `UseSkillRequest`) — using a skill *from the client* is not wired yet. It wants
    the same script-effect seam casting has, so the effect — the mined ore, the
    picked lock — stays a script's, granted off a `SkillUsed`-style event rather
    than baked into the engine.
  - [ ] **stat gain from skill use** — a skill that trains also nudges its
    governing stat; wants Sphere's per-skill stat map, so it rides with the
    `AdvRate` tables below
  - [ ] Sphere's per-skill `AdvRate` gain tables and the "learn only from a
    challenge" `GainRadius` — data-driven config, a refinement on the flat curve
- [x] `magic` — spells, reagents, casting
  - [x] **Mana, casting, and the effect seam.** A mobile carries `Mana` (spent by
    casting, trickling back on a tick-counter regen). `Command::CastSpell` is the
    gate every spell passes: it checks the mana, rolls the casting skill (through
    the same `roll_skill` a mined ore uses, so casting trains Magery), spends the
    mana, and emits `SpellCast { caster, spell, target, success }`. What the spell
    *does* — a fireball's damage, a heal, a summon — is not here: a script reads
    `SpellCast` and gives it its effect, `MobileDied`'s decoupling a third time.
    `Command::Heal` mends toward the maximum; `op_cast_spell`/`op_heal`/typed
    `op_damage` are the script's hands.
  - [x] **Typed damage and resistances** (the piece combat deferred). `damage`
    now takes a `DamageType` — physical, fire, cold, poison, energy — and cuts it
    by the target's `Resistance` *for that type*, in the one place all damage
    passes through, so a fireball and a sword swing share the door. Melee is
    physical; a spell picks its element.
  - [x] **reagents** — a spell consumes items from a pack. `items` grew the
    container search the deferral named — `count_in_container` and an
    all-or-nothing `take_from_container` — and `cast_spell` grew a second gate
    beside mana: a `Cast` now carries a `pack` and a `(graphic, count)` reagent
    list, and the spell fizzles spending *nothing* unless the pack holds every
    reagent, then consumes them. Reagents-as-data: the script names them per
    spell, the world enforces them. A pack open on a client redraws live too:
    `WorldState` remembers who has each container open (`double_click` records it,
    logout clears it), and a consumed reagent is pushed to those watchers — a
    `0x1D` for an item burned whole, a re-sent `0x25` for a dipped stack.
  - [x] **the client cast path** — a spellbook cast (`0xBF.0x1C`, read from
    ServUO's `PacketHandlers.CastSpell`) decodes to a `RequestCast`. It once
    became a `SpellRequested` event for the script to own the mana and reagents
    (Sphere-scriptpack style); the core spell table below took that over, running
    the whole cast itself, and `SpellRequested` is left dormant behind it. The
    older `0x12` text-command form is a fill-in; a modern client sends the `0xBF`.
  - [x] **The 64-spell core table and the full cast sequence.** All eight circles
    of Magery live in a core table (`magic::spells`, ported from ServUO's
    `SpellInfo` + the classic reagent lists): each spell's circle — which sets its
    mana, cast delay and difficulty — its reagents, what it targets, and its
    *default effect*. `RequestCast` → `World::begin_cast` runs the sequence in the
    core: mana and reagents from the pack, the Magery roll (the same `roll_skill`
    a mined ore uses), the target cursor, and the effect. The core runs the
    archetypes the engine can do — direct and area typed damage, heal, teleport —
    and tags the rest `SpellEffect::Scripted`: they still *cast* fully and emit
    `SpellCast` for the pack to give an effect, the "default in core, customise in
    the pack" split skills has. A pack overrides any spell the same way, off
    `SpellCast`.
  - [x] **Cast style, a `[gameplay]` flag** (the Sphere-vs-ServUO knob asked for).
    `cast_style = "servuo"` roots the caster over a cast delay held in a `Casting`
    component and only then raises the target — moving breaks it, and `spell_disturb`
    decides whether a blow mid-cast fizzles it; `cast_style = "sphere"` resolves
    the cast as it is made, walking. Both threaded from `openshard.toml` into
    `WorldState.gameplay`, never branched on `Era`.
  - [x] **Poison, in the core.** Poison, Cure and Arch Cure run a `Poisoned`
    component that `combat::poison_tick` pulses like decay — typed poison damage
    cut by poison resistance, in the one place all damage passes — with a dose
    scaled from the caster's Magery. This is the first spell effect that is *stateful
    over time* rather than instantaneous, the shape the timed buffs then reuse.
  - [x] **Timed stat buffs — the Bless/Curse family.** Strength, Agility, Cunning
    and Bless, and their opposites Weaken, Clumsy, Feeblemind and Curse, all moved
    from `Scripted` into the core. `magic::apply_stat_buff` folds a Magery-scaled
    offset into the target's `Stats` and the caps that hang off them (str→hits,
    int→mana), a `StatMods` ledger remembers exactly how to give it back, and
    `magic::expire_buffs` lifts it on the tick counter; a recast refreshes its kind
    rather than stacking a second. The stat change redraws the player's status bar
    (`0x11`), the one thing that re-sends str/dex/int. The effect kinds are
    canonical in `state::effect`.
  - [x] **Effects persist (schema v7).** A live effect is saved with its mobile —
    a `Poisoned` or a `StatMods` entry becomes an `EffectRecord { kind, amount,
    remaining }` on the character or mobile row — and restored on login and boot
    alike, so a relog cannot wash a debuff off. The ServUO/Sphere model reached the
    way this engine saves anything: a record, swept whole, not a stopped world.
    Poison restores as the whole component; a buff as its ledger *only* (its shift
    is already folded into the saved stats, so re-applying would double it).
    `World::effects_of`/`apply_effects` are the one seam; every future buff and
    debuff slots into the same `effects` list with no schema change.
  - [ ] **Cast visuals and sounds** — a spell today resolves with no words spoken
    over the caster's head, no particle effect (`0x70`/`0xC0`), no sound (`0x54`)
    and no cast animation; the effect encoders do not exist in `protocol` yet.
    ServUO's `SpellInfo` already carries the power words and each spell's
    effect/sound ids, so this is data the table can grow. The single most visible
    gap when testing with a real client.
  - [ ] **The non-stat magical buffs** — Protection, Reactive Armor, Night Sight,
    Magic Reflection: a different mechanic from the stat family (each modifies a
    behaviour, not a number), but the same `effects`-list persistence once each
    carries a component. The nearest next step, and the smallest.
  - [ ] **Persistent fields** — Fire/Poison/Energy/Paralyze Field and Wall of
    Stone: a timed area that pulses damage or blocks movement, on the tick counter
    like decay. Wants a field entity that the movement obstruction index and the
    damage path both see.
  - [ ] **Summons with a lifetime** — Blade Spirits, Energy Vortex, Summon
    Creature/Daemon: a spawned creature that despawns on its own timer and counts
    against the follower cap the status bar already carries.
  - [ ] **Travel** — Recall, Gate Travel and Mark: needs runes/runebooks (item
    data) and a cross-facet teleport; Gate also a paired physical object either end.
  - [ ] **Dispel, polymorph, resurrection** — each waits on a subsystem of its own
    (summon lifetimes, a body-swap that restores cleanly, the ghost/corpse slice).
  - [ ] **The Poisoning skill for the deadlier doses** — the Magery-cast dose caps
    at greater; the higher poison levels (deadly, lethal) want the Poisoning skill
    to set them.
- [x] `ai` — brains, aggro, wandering
  - [x] **A built-in brain, and room for scripted ones.** A creature spawned with
    a `sight` or `wander` gets a `Brain`, and `think()` gives it a beat every so
    often (not every tick): it notices the nearest player within sight and takes
    a `Combat` aimed at them — so `swings()` attacks it with exactly the machinery
    a player fights with — chases when out of reach, drops a target that dies or
    flees, and drifts when idle. The decision uses the world's `Rng`, so a fight
    replays. Aggro range and wandering are spawn data (`op_spawn_mobile` grew
    `sight`/`wander`), the script-first knobs.
  - [x] **The fully script-driven brain** — the per-mobile `onTick` the scripting
    benchmark sized. A mobile carries a `Scripted` marker; the built-in `think`
    skips anything wearing it, and the server calls that mobile's `onTick` every
    tick instead. A script takes control with `op_control` — which it can only do
    once it knows a serial, so spawning a mobile emits `MobileSpawned`, delivered
    like every other domain event. The built-in `ai` and a scripted brain are the
    two paths, and a mobile is on one or the other, never fought over by both.
  - [x] **Creatures behave like the references say they should.** Movement sees
    the live world: each facet carries an obstruction index of shut doors and
    impassable decoration, and `LiveTerrain` lays it over the map for every walk,
    step and A\* plan — a closed door blocks players and NPCs alike. Aggro needs
    **line of sight** (`Terrain::sight_clear`, a Bresenham ray; windows pass,
    walls and NO_SHOOT statics do not, shut doors are opaque). A chase walks
    naive-step-first, plans once when blocked, follows a **cached `ChasePath`**
    with a 2s repath, and on an impossible route **gives up** — target dropped,
    ~10s standing guard, then back to its life; never the fence-shuffle.
    Humanoids (`body_opens_doors`) open unlocked doors in their way; so do
    townsfolk heading home. Creatures carry an `Aggression` posture (passive
    fauna flee when struck; defensive ones answer the first blow via
    `ai::retaliate`; aggressive ones hunt on sight), break off badly hurt unless
    too big to scare, and step at `gameplay.creature_step_ms` (400 classic — a
    running player outruns a base monster on purpose), each spawn able to
    override its beat.
  - [x] **Ranged creatures volley and kite.** A spawn with `ranged` reach fires
    through `combat::volleys` — typed damage, LOS-gated, sharing the swing timer —
    and keeps its distance at `KITE_GAP` instead of walking into melee.
  - [ ] **Body-type tables** — door-opening and rideability are body-id lists
    until a real table (tiledata or data-driven config) names which bodies have
    hands and which carry a rider.
  - [ ] **Path to a tile *adjacent* to the quarry** rather than onto it — the
    remaining refinement from the A\* work; today a chase plans onto the target's
    own tile and stops one short by the reach check.
- [x] `chat` — speech, journal routing
  - [x] **Speech, heard and answered.** A player says something (`0x03`), and the
    world puts it over their head for everyone within `SPEECH_RANGE` (`0x1C`,
    ported from Sphere's `PacketMessageASCII`) and on the bus as `MobileSpoke`.
    That event is the hook: a script reads the words and answers — a keyword, an
    NPC's line, a command — through `op_say`/`Command::Speak`, and the answer
    goes back out as another `0x1C`. Combat's decoupling for the fourth time; the
    round-trip is tested end to end. This is why the script `Event` and `Command`
    stopped being `Copy`: speech carries an owned `String`, and the bus never
    required `Copy` — only the enums had assumed it.
  - [x] **The Unicode talk packet** (`0xAD`), which is what a modern client
    actually sends when you type — the plain UTF-16 form and the keyword-encoded
    one, ported from Sphere. The classic `0x03` alone left live chat silent for
    every ClassicUO client; this is the fix.
  - [x] **The Unicode reply** (`0xAE`, ported from Sphere's `PacketMessageUNICODE`).
    Speech chooses its encoder by content: pure-ASCII stays on `0x1C`, universally
    understood, but text Latin-1 cannot carry — an accent, a non-Latin script —
    goes out as big-endian UTF-16 `0xAE`, so a player who types "olá" gets the
    accent back intact. A player could only have typed such text through `0xAD` to
    begin with, so the content test doubles as the client-capability one, sidestepping
    that the game connection never states its version.
  - [x] speech *modes* widening or narrowing the range: a whisper (`;`, mode 8)
    carries three tiles, a yell (`!`, mode 9) thirty-one, everything else the
    eighteen-tile screen — Sphere's `DISTANCEWHISPER`/`DISTANCETALK`/`DISTANCEYELL`
    defaults, chosen by the mode byte the client already sends. `speak` picks the
    range; the rest of the path is unchanged.
  - [x] **the guarded staff-command layer** (`.`-prefixed speech, Sphere's
    convention). An account carries an `AccessLevel` — `player`, `gamemaster`,
    `administrator` — set in `[[accounts]]` config (`access = "gm"`), looked up at
    login and carried into the world as an `Access` component, re-derived each
    login so a demotion takes effect and never saved with the character. A game
    master's `.`-prefixed speech is split off in the `Command::Say` handler and
    run as a command instead of reaching anyone's screen; an ordinary player
    saying `.hello` just talks, so there is no leak and no surprise. The commands
    — `.where`, `.go`, `.tele`, `.add`, `.set`, `.admin` — lean on the systems
    that own their rules (`items` spawns, `skills` re-caps the stat) rather than
    reaching into the registry, and answer the actor privately with a `0x1C`
    system line. `.go <x> <y>` jumps to coordinates; `.tele` raises a targeting
    cursor (`0x6C`) and jumps to the tile clicked — Sphere's split, and the
    teleport pushes a `0x20` to the mover's own client so the screen refreshes on
    the spot rather than a step late. The gate lives in the world, not the `gm`
    module, so a command function may assume its caller cleared it. The vocabulary
    grows one verb at a time in `world::gm`.
  - [x] **The `.admin` gump and a pack-driven world.** `.admin` opens a staff-only
    gump (`0xB0`, answered on `0xB1`, re-checked GM+ on the button, not only on
    open) whose buttons populate cities and lay down decoration. The *data* lives
    in the community pack, not the engine: a button emits an `AdminAction` event
    the pack reads, and the pack answers with `op_register_spawner`, `op_decorate`
    and `op_generate_doors` — so spawns and scenery are edited in a hot-reloaded
    script, no rebuild. **Spawners** are tick-maintained regions (`maintain_spawners`):
    a region holds creature templates, a max count and a respawn delay in ticks,
    and a `SpawnedBy` marker lets it refill as its creatures die — replayable, like
    decay. **Decoration** is what a shard adds on top of the map's static art, all
    marked `Decoration` (never decays, never lifts): plain statics (walls, signs,
    furniture), **doors** that toggle open/shut on double-click and swing closed on
    their own (`Door`, a two-graphic-plus-hinge toggle in `items`, auto-closed by
    the tick), and **containers** that open onto a gump (town chests, crates,
    barrels — reusing the `Container` open path, placed empty). The whole of Britain
    is migrated from ServUO's `britain.cfg` and `signs.cfg` (door graphics/offsets
    from its door tables, container gumps from the client's own `containers.cfg`),
    resolved to raw graphics *at pack time* so the engine stays a generic
    toggle/open and knows nothing of door or container families.
  - [x] **Doors generated from the map's own art.** A building's plain wooden shop
    doors are not in the decoration data — they are *implied* by the static door
    frames the client map draws, so the shard generates them: `op_generate_doors`
    scans a region's statics for facing frame posts and drops a functional
    `DarkWoodDoor` into each one- or two-tile gap. This is ServUO's `DoorGenerator`,
    ported (`world::doorgen`) — the same four frame-graphic tables and single/double
    geometry — reusing the statics the engine already parses through a new
    `Terrain::statics_at`. The metal and special doors are placed by name from the
    data; this fills in the ones the map only implies.
  - [x] **The pack is a directory now.** `scripting.main` may point at a folder, not
    just a file: the engine concatenates every `.js` under it (organised by facet
    and place — `felucca/britain/spawns.js`, `deco.js`), `index.js` last, into the
    one script it still evaluates, and hot-reload watches the newest mtime across
    the tree. Data files register into a shared `Pack` namespace under a verb;
    `index.js` wires `onEvent` over it. Deco and spawn are separate files, so a
    shard edits one without touching the other. Still deferred: container **loot
    tables**, door **keys/locks**, sign **text** (a cliloc slice), and the
    furniture/addon *behaviours* (a real armoire versus a scenery one).
  - [x] **Inventory persists.** A character's carried things — worn gear, its
    backpack and everything nested inside — and loose ground clutter now survive a
    restart, not just its position. See §4; this is the foundation a bank and a
    vendor stand on, because a service that forgets your gold on logout is a demo,
    not a service.
  - [x] **Bankers, and a bank box that holds value.** Every character wears a bank
    box (a container on `Layer.Bank`, graphic `0x0E7C`) alongside its backpack, so
    it persists and its contents survive a restart. A `Banker` NPC — a standing,
    named, invulnerable townsperson the pack places once (`op_spawn_mobile` grew a
    `name` and a `banker` flag) — answers the keyword: saying "bank" within twelve
    tiles of one opens your box (the same `0x24`/`0x3C` a double-click sends,
    reused through `items::open_worn_container`), and "balance" counts the gold in
    it. The words are still spoken, so it reads as a request the banker answers.
    And it has life, in its own crate — **`crates/npc`**, so the townsfolk rules do
    not pile into `tick.rs` (the banker logic *moved out* of it). An NPC is
    **dressed** (`op_spawn_mobile` grew an `equipment` list — a robe, hair — worn
    like any gear and drawn in its `0x78`), **named** (a generated personal name and
    the "the banker" title, from the seeded generator so a replay names it the
    same), **stands on the floor** (a spawn drops onto the map's surface at its
    tile, a building's raised floor and all, through a new `Terrain::stand_z`,
    rather than sinking to a given z and reading as inside a wall), **greets** with
    a line chosen fresh each time and by name, turning to face the visitor, and
    **keeps to a home** — an `Npc { home, wander }` base (the part vendors reuse)
    lets it shuffle a couple of tiles near its post rather than stand frozen. The
    AI seam is decide-then-apply, like the creature brain: `npc::live` greets and
    faces itself, and returns the idle steps the tick applies through its
    terrain-checked `step`. This is the first of the living NPCs; **vendors** (buy
    `0x74`/`0x3B`, sell `0x9E`/`0x9F`) reuse the `Npc` base.
  - [x] **Vendors trade.** A `vendor` spawn wears a stock crate a script prices
    (`op_stock` — price and name are item components, so stock is pack data, not
    engine code); double-click opens the classic buy flow (`0x74` contents +
    `0x3B` purchase), and saying "sell" nearby offers the mirror (`0x9E` list,
    `0x9F` sale) at half price. Stock persists with the vendor (§4, schema v5) —
    a restart does not restock the shelf.
  - [x] **Mounts.** Double-click a horse, llama or ostard to ride: the creature
    leaves the world into limbo and a `0x19`-layer saddle item draws the rider
    mounted; double-click yourself to dismount, and the creature is reconstituted
    whole — heading, walker, brain — beside you. The ride persists through the
    saddle item saved with the character, so logging out mounted logs back in
    mounted; the ridden creature itself is the one mobile the world sweep skips.
  - [ ] **Vendor restock timers** — a bought-out shelf refills on a timer, the
    references' economy heartbeat; today stock only changes by trade.
  - [ ] **Locks and keys on doors** — every door opens to any NPC that can work
    handles, and to any player; the pack already names locked doors it cannot yet
    lock. Wants a lock component the obstruction index respects and key items.
  - [ ] **Mounted movement speed at the pace budget** — a mounted rider should
    earn the mounted run rate in `WalkPace`; today the saddle is visual and the
    budget does not know about it.
  - [ ] **Secure trade between players** (`0x6F`) — the drag-onto-a-player trade
    window. The `NewSecureTrade` feature gate exists in `protocol`; the handler
    and the escrow container do not.
  - [x] **A* pathfinding**, so pursuit and homing route *around* walls instead of
    shuffling into them — the thing Sphere does badly. `movement::find_path` is a
    bounded A* over the `Terrain` (the same `can_step` the client's walk uses), with
    a Chebyshev heuristic, a node budget so it can never stall a tick, and a
    corner-cut guard (a diagonal is only taken when both tiles beside it are open,
    so a path never clips a building's edge). It is a pure, dice-free function —
    same map and endpoints, same path — so a replay's monsters keep the same trail.
    The creature chase (`ai::step_toward`) and a townsperson heading back to its
    post both plan through it, falling back to the straight line only when there is
    no map or no route within budget. The path *cache* this once named as a next
    step landed with the creature-behaviour work above (`ChasePath`, a 2s repath);
    adjacent-tile pathing is still open, listed under `ai`.
  - [x] **A name on single-click, a tooltip on hover, a menu on right-click.**
    Clicking a mobile (`0x09`) draws its name over its head for the clicker alone
    — a `0x1C` label in the notoriety colour (ServUO's `Notoriety.Hues`: blue
    innocent … yellow invulnerable), so a banker reads as "the banker" before you
    know to ask. An item labels too now, in the default text hue with its tiledata
    name (Sphere's `addItemName`, "3 gold coins" and all), read through a new
    `Terrain::item_name` beside the `item_blocks`/`item_height` tile accessors.
    That is the classic 2D feel — what a modern client shows on hover, this one
    asks for a click at a time. **And the modern feel is here as well.** AoS object
    tooltips are the "cliloc" system: when the server draws a thing it sends the
    tooltip *revision* (`0xDC`), the client asks for the list (`0xD6` in), and the
    server answers (`0xD6` out) with cliloc numbers the client localizes — a mobile
    is cliloc `1050045` with its name, an item cliloc `1020000 + graphic` (the
    client's own tiledata-name range, so no string travels), pluralised through
    `1050039` for a stack. The revision hash is one value in both packets (Sphere),
    and the whole thing is default-in-core the way names and spells are:
    `WorldState::object_properties` builds the list from components. **Context menus**
    round it out (`0xBF` `0x13` request → `0x14` popup → `0x15` select): a
    container offers Open, a vendor Buy/Sell, any mobile a Paperdoll — each routed
    to the very handler a double-click reaches, so the menu decides *what* and the
    existing rule does *how*. Ported from ServUO's `ObjectPropertyList`/`OPLInfo`/
    `DisplayContextMenu`, cross-checked against Sphere's `PacketPropertyList` and
    `Event_AOSPopupMenuRequest`. Two `[gameplay]` knobs shape it, Sphere's
    `TOOLTIPMODE` made an operator setting: `tooltips` (`"off"` | `"version"` |
    `"full"`) and `context_menus` (bool). With both off no `0xB9` SupportedFeatures
    is sent, so a modern client falls back to the classic single-click label; with
    either on the AoS bit is advertised and ClassicUO turns on hover tooltips and
    context menus. Still on the list: richer per-object menus, the old (`0x01`)
    popup format for pre-6.0 clients, and a tooltip that refreshes mid-life when a
    property changes (names do not, so nothing needs it yet). **Two things a live
    test surfaced landed with this:** a creature with no name given now takes a
    default from its body (`state::creature_name`, ServUO's ids — "a chicken", "a
    horse"), so an unnamed animal or monster reads on single-click and in its
    tooltip, the pack still free to override per spawn; and a mobile's health bar
    (`0xA1`) is sent *on sight*, riding along with its `0x78` the way the tooltip
    revision does, so the bar reads full from the moment you see a thing rather
    than staying an empty frame until the first blow moved it.
- [ ] `housing` — player houses: a multi placed on the map, a door with a real
  lock, decay unless refreshed, friends/co-owners. Wants multis (the client's
  `multi.mul`/UOP format, unread yet), a region concept and the door locks above.
- [ ] `guilds` — membership, titles, the guild notoriety rules (green/orange),
  war declarations. Mostly data plus a notoriety hook; the abstract stub exists
  so the dependency graph already names it.

The bridge is both event- and tick-driven now: the server calls the script's
`onEvent` with each tick's domain events, and the per-mobile `onTick` for every
mobile a script controls (`op_control`, the `Scripted` marker) — the hook the
benchmark priced. The script vocabulary — the events in, the commands out — grows
one gameplay area at a time, each new command mapped in `into_world`.

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
