# Architecture

## The premise

Ultima Online's protocol is a fixed external contract. Two decades of clients
already implement it and none of them will change. Everything else — how the
world is stored, how systems talk, how gameplay is expressed — is ours to
choose.

SphereServer answered those questions in 1999, in C++, for single-core machines,
with a bespoke scripting language. The answers were good for 1999. This project
takes the same contract and answers again.

So: **compatible with the protocol, not with Sphere.** The only thing worth
carrying across from Sphere's source is its record of observed client behaviour —
which client version breaks on which packet. That knowledge is expensive and
Sphere paid for it. Its architecture we can decline.

## Layers

Arrows are dependencies; they only ever point down.

```
   server        the binary: boot, the accept loop, packet dispatch, sessions;
     │           drives login, the script engine and the world around the tick
     │           (login and scripting sit beside it, not below the world)
     ▼
   world         the tick and command queue, the client's file formats
     │           (map/tiledata/UOP), the persistence journal — orchestration
     ▼
   combat  chat  items  skills  magic  ai  npc
     │           the gameplay systems: each a fn(&mut WorldState) in its own
     │           crate, owning its domain events
     ▼
   state         WorldState — registry, bus, sectors, seeded rng, the
     │           drawing/interest substrate, the Gameplay tunables
     ▼
   entities   events   protocol   gateway   movement   persistence   config
                 the foundation: identity/storage, event machinery, the wire,
                 framing, the walk rules, the Store trait. No gameplay.
```

### Dependency rules

- **Dependencies point downward only.** A crate never depends on one above it,
  and there are no cycles. `combat` depends on `state` and `entities`, never on
  `ai`; nothing below `world` knows the tick exists.
- **Systems do not depend on each other.** Two systems that need to interact do
  it by emitting and reading events, not by calling. (The narrow exceptions are
  compositional, not conversational: `ai` builds on `combat`'s components, `npc`
  on `ai` — a layer using the layer below, never a peer calling a peer.)
- **Nothing depends on `world` except the thing that runs it** — the server. A
  crate that wants to know what happened reads events; it does not import the
  tick.
- **Domain events live in the crate that owns the rule** that emits them, and
  `world` re-exports them so consumers see one surface.

## Crates

**Implemented.**

| Crate | Owns |
|---|---|
| `entities` | `EntityId`, `Serial`, `SparseSet`, `Registry`. Identity and storage. No gameplay. |
| `state` | Components, the `Sectors` spatial index, the seeded `Rng`. The world's runtime *data*, below the systems that act on it, so each system can live in its own crate. Knows nothing of *when* state changes. |
| `events` | `Events<E>`, `Cursor<E>`, `EventBus`. Machinery. Defines no game events. |
| `protocol` | Versions, feature gates, the codec, framing, the login and world packets. |
| `gateway` | The sans-io `Connection` and a thin Tokio `Server`. Finds packet boundaries; knows nothing of meaning. |
| `login` | `Accounts`, `AuthKeys`, and the sans-io `LoginServer`. |
| `movement` | The walk handshake, the sequence rules, the pace limiter, and A* (`find_path`). `Terrain` is a trait it does not implement. |
| `config` | TOML, validated at load. |
| `server` | The binary. Glue only: `boot` loads config/store/world, `shard` owns the accept loop and shutdown, `dispatch` turns packets into commands, `session` is per-connection state. |
| `world` | The tick, the client's file formats, `MapTerrain`, and the persistence journal. Owns `WorldState` and drives it. Orchestration, not rules — see the `tick/` layout below. |

**The gameplay systems.** Each is a set of `fn(&mut WorldState)` in its own
crate, owning its domain events:

| Crate | System | Events |
|---|---|---|
| `chat` | `say`/`speak`, speech ranges | `MobileSpoke` |
| `skills` | skill/stat checks, the gain curve, the shared `roll_skill` | `SkillUsed`, `SkillRaised` |
| `magic` | the 64-spell Magery table, `pay_and_roll`/`heal`/`regen_mana`, the timed stat buffs (`apply_stat_buff`/`expire_buffs`) | `SpellCast` |
| `combat` | `damage`/`die`/`swings`/`volleys`/`attack`, poison pulses, criminal flagging, the swing formula | `MobileDamaged`, `MobileDied` |
| `items` | spawn/drag/stack/decay/containers/equip/doors/mounts, one module each | `ItemSpawned` |
| `ai` | the creature brain: LOS aggro, cached-path chase, give-up, kiting, fleeing, retaliation | — |
| `npc` | townsfolk services (banker, vendor buy/sell) and the creature `spawn` rule | `MobileSpawned` |

The drawing/interest substrate they share (`show`, `forget`, `broadcast_move`,
`refresh_around`, `reveal`, `mobile_incoming`, …) lives on `WorldState`, in the
`state` crate below them. `world` keeps the tick that sequences the systems, the
client's file formats, and the persistence journal — the orchestration, not the
rules.

**Stubs** — declared so the dependency graph is visible.

`housing`, `guilds`, `plugins`, `metrics`.

## The shape of a file

`world/src/tick.rs` once reached 8,116 lines by absorbing tests, banker logic,
persistence bridging and door generation inline. That is the cautionary tale
this section exists to prevent repeating.

**A file over ~2k lines is overdue for a split.** The mechanics that make a
split cheap, used by `tick/`, `engine/` (scripting) and the items crate:

- **Child modules of the owning module**, not siblings: `tick.rs` declares
  `mod motion;` and the file lives at `tick/motion.rs`, holding one
  `impl World { … }` block. A child sees the parent's private items, so the
  parent's fields and helpers need no visibility widening; an item a child
  exposes back to the parent or a sibling is `pub(super)`, nothing wider.
- **Tests that read private state stay child modules** (`tick/tests.rs` behind
  `#[cfg(test)] mod tests;`), where parent-module privacy still reaches them.
  They cannot become `tests/` integration tests without widening the API — so
  they don't.
- **A crate's flat API survives a split** with `pub use module::*;` re-exports
  (`items`), so callers never learn the file layout changed.

The `tick/` layout, as the worked example: `command.rs` (the `Command` enum),
`defaults.rs` (tuning constants), `persist.rs` (the journal bridge),
`enter.rs` (character entry), `motion.rs` (`walk`/`step`), `spawners.rs`
(spawn-region upkeep), `decor.rs` (decoration and door generation), `speech.rs`,
`staff.rs`, and the three test files. `tick.rs` itself keeps the `World` struct,
the command router and the tick — orchestration, ~750 lines.

### Where code goes

- A gameplay **rule** → a domain crate, as `fn(&mut WorldState)`.
- Entity assembly, journal bridging, walk/step authority, decoration placement
  → `world/tick/*` (they need the journal, the terrain, or the command queue).
- Wire routing (packet → `Command`) → `server/dispatch`.
- Drawing, interest, packet composition shared by systems → `state`.

### Anti-patterns

Named so a review can point at them:

- **The god file** — a tick that absorbs every new feature inline. Rules go in
  domain crates; the tick sequences them.
- **Gameplay in `state`** — `WorldState` is data plus the shared drawing
  substrate. The moment it grows a rule, every system depends on that rule.
- **Circular crate dependencies** — if two crates need each other, one of them
  is holding an event that belongs on the bus.
- **`Era` branching** — ask `version.supports(Feature::X)`; see Protocol below.
- **Global mutable state** — everything is a plain value a test can build.
- **The database inside a tick** — the journal drains to a task nothing waits
  on; see `persistence/src/journal.rs`.

## Entities

Everything is an entity: players, NPCs, items, houses, boats, projectiles. None
of them are subclasses of each other. What a thing *is* falls out of which
components it carries.

### Two identities

`EntityId` is internal — a generational index, never sent to a client. The
generation is what makes stale handles safe: a corpse remembers its killer, a pet
remembers its owner, and those references outlive the things they point at.
Validating the generation on every lookup turns "use after despawn" from a bug
class into `None`.

`Serial` is the wire identity — a 32-bit value the client uses to address
objects. Mobiles and items come from disjoint numeric ranges because the client
infers the category from the range. That is a protocol constraint, not a design
choice.

Serials are **never recycled**. A client packet already in flight may name a
serial that has since been freed; handing it to a new object lets the client act
on the wrong thing. Both pools are large enough that it does not matter.

### Why sparse sets, not archetypes

Archetype ECS wins when component sets are fixed at spawn and iteration is the
whole workload. Neither holds here. Components churn constantly — an item picked
up loses its world position, an NPC gains and drops a combat target — and every
such change would move a whole row between archetype tables. Sparse sets pay O(1)
for that churn and still iterate a dense array.

If profiling later says otherwise, `Registry`'s public API does not leak the
storage, so it can be replaced.

## Events

Systems do not call each other. Combat does not call the guild system to update
war scores; it emits `NpcKilled` and moves on.

This is not decoration. It is what makes plugins possible without the engine
knowing about them, and what makes logging, metrics, and replay fall out for free
rather than being threaded through every call site.

### Why not callbacks

A subscription model means the bus owns handlers, handlers own state, and
emitting an event runs arbitrary code at an unpredictable point in the tick.
That buys reentrancy, ordering bugs, and a simulation that is no longer
deterministic — which forfeits replay.

Here, `send` pushes to a `Vec`. Reading happens where the reader chooses. Tick
order is whatever the game loop says it is, and the same events replayed produce
the same world.

### The two-tick lifetime

Events live for two ticks, not one. A system that runs *before* the emitter
within a tick still sees the event on the next tick rather than missing it
forever. Without this, system order becomes load-bearing and every reordering is
a potential silent bug. The cost is one extra buffer per event type, swapped and
reused.

Each reader owns a `Cursor`. Reading does not consume — three systems can each
read every `PlayerMove`. The bus holds no subscription state at all.

### Where events are defined

In the crate that owns the rule that emits them. `PlayerLogin` with login,
`NpcKilled` with combat, `HouseCreated` with housing.

Putting them all in `events` would make it a hub every crate must agree on, and
every new event a change to a shared file. The bus is machinery; it should not
know what a house is.

## Protocol

### Multi-era

There is no single "the protocol". A 2.0 client and a 7.0.95 client speak
different dialects. A shard decides which it accepts.

Versioning is modelled first, before any packet, because retrofitting it means
auditing every encoder twice.

### The rule

Gameplay and encoder code asks `version.supports(Feature::X)`. It never compares
version numbers and never branches on `Era`.

Features did not arrive in era-sized batches:

| Feature | Since | Era |
|---|---|---|
| Tooltips | 4.0.0a | AoS |
| Stat locks | 4.0.1a | AoS |
| Silent close dialog | 4.0.4.0 | AoS |
| Tooltip hash | 4.0.5a | AoS |
| New damage packet | 4.0.7a | AoS |

A client at 4.0.3 is "AoS" and wants tooltips and stat locks but not tooltip
hashes. `era == Era::Aos` is wrong for most of the range it covers — and wrong
silently, because the client drops the unexpected packet without complaint.

`Era` is for coarse decisions only: which map set to load, whether housing is
customisable.

Every boundary lives in `Feature::since`, ported from Sphere's `MINCLIVER_*`
table. One table to fix when a boundary turns out to be off by a patch.

## The world

The entire world is in memory. The database is persistence, never a query
target during gameplay.

The real tick, in order (`world/src/tick.rs`):

```
tick:
  apply queued commands          network input, script output — one order
  ai think / npc live            brains decide; the tick applies the steps
  combat                         swings, volleys, criminal/murder expiry, poison
  magic                          buff expiry, mana regen, casts in flight
  items                          decay, doors swinging shut
  spawners                       regions refill their dead
  wire follow-ups                skill window updates, status redraws
  journal.mark_dirty()           from the bus, not from call sites
  bus.update()                   the two-tick swap
  offer_snapshot()               a memcpy handed to the save task, off-tick
```

The systems run in a fixed serial order, not parallel queries — that is the
deliberate price of a deterministic, replayable simulation. The tick is
single-threaded per world region; async lives at the edges — network, database,
HTTP — never inside the simulation. That boundary is what makes replay and
debugging tractable. Randomness inside a tick comes only from the world's seeded
`Rng`, and every timer is a tick count, never a wall clock — a world constructed
twice rolls and expires identically.

## Scripting (spike done)

Gameplay is not hardcoded. NPCs, items, quests, regions, commands, skills,
crafting and spells are TypeScript, hot-reloadable without a restart.

`deno_core` embeds V8 in-process. QuickJS was considered and rejected — too slow
for hot gameplay code. A Node sidecar was considered and rejected — IPC latency
lands inside the tick.

This was the largest open technical risk in the project, and the spike has
retired it. `crates/scripting` embeds one `JsRuntime` in a single V8 isolate
behind [`ScriptEngine`], a four-method trait with nothing V8-shaped in its
signatures — so the runtime stays replaceable. A script is one more consumer of
the same seam every system uses: domain events arrive through `deliver`, the
engine keeps a small read model from them, and a script acts only by enqueuing a
`Command` the tick applies in order. It never writes the world directly. Ops are
declared with `deno_core::extension!` and `#[op2]`, and every op called from a
hook is synchronous — a tick never awaits.

The benchmark is the point: a hook call costs on the order of a couple of hundred
nanoseconds, so ten thousand mobiles each firing a hook per tick spend a low
single-digit-millisecond slice of the 50ms budget. It fits. Numbers and method
are in `docs/roadmap.md` §5.

`ScriptEngine::load` doubles as hot reload — re-evaluating rebinds the hooks in
the live isolate — and `DenoEngine::reload_if_changed` polls a watched file's
mtime so iterating on a hook is save-the-file, not bounce-the-shard.

And it is wired into the running shard. The server (`crates/server/src/scripting.rs`)
owns the engine and drives it around the tick: after `world.tick()` it hands the
tick's domain events to the script and queues the commands the script emits for
the next tick. That keeps a script on the same side of the boundary as a network
task — it never writes the world inside the tick that is running, only enqueues a
command a later tick applies. World and scripting stay ignorant of each other;
the server is the adapter, which is what an adapter is for. `Command::Step` —
server-authoritative movement, terrain the only judge — was the first command a
script could land, and the seam §6 gameplay grew from.

Both hooks the benchmark priced are wired now: `onEvent` receives each tick's
domain events, and the per-mobile `onTick` runs every tick for any mobile a
script controls (`op_control` sets a `Scripted` marker; the built-in brain skips
what wears it, so a mobile is on one brain or the other, never both).

## Persistence

```
events → Journal (dirty marks) → Snapshot (a memcpy at one tick) → Store::save
   the tick's side                  the handover                  a task nothing waits on
```

Implemented, end to end. The journal marks what changed *from the event bus* —
emitting the event is the touch, so no call site can forget persistence exists.
A snapshot is owned values taken at one instant; a `Store` (SQLite, PostgreSQL,
or in-memory for development) writes it on a task the tick never waits for. Both
reference emulators stop the world to save it — ServUO literally broadcasts
"please wait" — and `persistence/src/journal.rs` is the argument for why this
one does not.

The save is the whole world, the Sphere/ServUO model: every character with its
nested inventory, every NPC with its wounds and vendor stock, every decoration
with its door state, every spawn region with its timer, every live effect —
poison, buffs — so a relog or a restart changes nothing a player can see. A
killed creature is simply absent from the next sweep and stays dead.

The load path is why `Registry::bind_serial` exists: serials come from the save,
not the allocator, and binding one reserves it so nothing fresh collides.

## Client files

None are in this repository and none will be. They are copyrighted; the operator
points `world.client_files` at an install they already have.

What is here are readers for the *formats*, and only the formats. The server does
not send map tiles — the client has had them since it was installed. What the
server needs a map for is deciding: how high the ground is, what blocks, what
floats. If the two disagree, the client draws a wall the server lets you walk
through and the player rubber-bands.

Nothing in these parsers is derived from any particular shard's data, and nothing
should be documented as if it were.

## Non-goals

Reimplementing SphereScript. Parsing `.scp` at runtime. Source compatibility with
Sphere. Legacy save formats. Mimicking Sphere's internals. Being bound by
decisions made for 1999 hardware.
