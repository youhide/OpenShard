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

```
                        Clients
                (ClassicUO / 2D Client)
                           │
                    UO Network Protocol
                           │
                  ┌────────┴────────┐
                  │     gateway     │   accept, framing, encrypt
                  └────────┬────────┘
                           │
                  ┌────────┴────────┐
                  │    protocol     │   encode / decode / version gates
                  └────────┬────────┘
                           │
                  ┌────────┴────────┐
                  │      world      │   the tick, spatial index
                  └────────┬────────┘
                           │
        ┌──────────────────┼──────────────────┐
        │                  │                  │
    entities            events            HTTP API
   (Registry)         (EventBus)         (dashboard)
        │                  │
        │         ┌────────┴────────┐
        │         │                 │
   combat  movement  ai  items  skills  magic  housing  guilds  chat
                           │
                    persistence queue
                           │
                    PostgreSQL / SQLite
```

Dependencies point downward only. `combat` depends on `entities` and `events`,
never on `ai`. Two systems that need to interact do it by emitting and reading
events, not by calling.

## Crates

**Foundation** — implemented.

| Crate | Owns |
|---|---|
| `entities` | `EntityId`, `Serial`, `SparseSet`, `Registry`. Identity and storage. No gameplay. |
| `events` | `Events<E>`, `Cursor<E>`, `EventBus`. Machinery. Defines no game events. |
| `protocol` | `ClientVersion`, `Era`, `Feature`, `FeatureSet`. Packets to come. |

**Stubs** — declared so the dependency graph is visible.

`gateway`, `login`, `world`, `combat`, `movement`, `ai`, `items`, `magic`,
`skills`, `housing`, `guilds`, `chat`, `persistence`, `scripting`, `plugins`,
`metrics`, `config`.

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

## The world (planned)

The entire world is in memory. The database is persistence, never a query
target during gameplay.

```
tick:
  drain network input
  movement
  combat
  ai
  timers
  scripts
  bus.update()
  flush persistence queue
```

The tick is deterministic and single-threaded per world region. Async lives at
the edges — network, database, HTTP — never inside the simulation. That boundary
is what makes replay and debugging tractable.

## Scripting (planned)

Gameplay is not hardcoded. NPCs, items, quests, regions, commands, skills,
crafting and spells are TypeScript, hot-reloadable without a restart.

`deno_core` embeds V8 in-process. QuickJS was considered and rejected — too slow
for hot gameplay code. A Node sidecar was considered and rejected — IPC latency
lands inside the tick.

This is the largest open technical risk in the project. The `ScriptEngine`
boundary should stay narrow enough that the runtime is replaceable.

## Persistence (planned)

```
entity changes → queue → async writer → database
```

Event-sourcing inspired. Autosave configurable. Crash recovery from the log.

The load path is why `Registry::bind_serial` exists: serials come from the save,
not the allocator, and binding one reserves it so nothing fresh collides.

## Non-goals

Reimplementing SphereScript. Parsing `.scp` at runtime. Source compatibility with
Sphere. Legacy save formats. Mimicking Sphere's internals. Being bound by
decisions made for 1999 hardware.
