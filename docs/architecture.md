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
| `state` | Components, the `Sectors` spatial index, the `Regions` index of named areas, the seeded `Rng`, and the tables two or more systems read (`weapon`, `armor`, `harvest`, `craft`, `title`). The world's runtime *data*, below the systems that act on it, so each system can live in its own crate. Knows nothing of *when* state changes. |
| `events` | `Events<E>`, `Cursor<E>`, `EventBus`. Machinery. Defines no game events. |
| `protocol` | Versions, feature gates, the codec, framing, the login and world packets. |
| `gateway` | The sans-io `Connection`, and a thin Tokio adapter over it: `Gate` serves one stream, `ClientGatewayServer` is that plus a listener. Finds packet boundaries; knows nothing of meaning, and nothing of where a stream came from. |
| `login` | `Accounts`, `AuthKeys`, and the sans-io `LoginServer`. |
| `movement` | The walk handshake, the sequence rules, the pace limiter, and A* (`find_path`). `Terrain` is a trait it does not implement. |
| `config` | TOML, validated at load. |
| `server` | The shard: glue only — `boot` loads config/store/world, `shard` owns the accept loop and shutdown, `dispatch` turns packets into commands, `session` is per-connection state. A library with a four-line binary on top, so a test can *start a shard* by calling `run_shard` instead of building one out of process. |
| `client/net` | The client's side of the wire: framing, decompression, the login conversation as a sans-io state machine, a `WorldView` of what the server has shown, and `Dial` — how a connection is opened, of which `Tcp` is one answer. The mirror of `gateway` + `login`, and it depends on neither. See [`client.md`](client.md). |
| `world` | The tick, the client's file formats, `MapTerrain`, and the persistence journal. Owns `WorldState` and drives it. Orchestration, not rules — see the `tick/` layout below. |

**The gameplay systems.** Each is a set of `fn(&mut WorldState)` in its own
crate, owning its domain events:

| Crate | System | Events |
|---|---|---|
| `chat` | `say`/`speak`, speech ranges | `MobileSpoke` |
| `skills` | the skill table's rules: the band check, the gain curve, stat gain, the usable skills | `SkillUsed`, `SkillChanged`, `SkillRequested` |
| `magic` | the 64-spell Magery table, `pay_and_roll`/`heal`/`regen_mana`, the timed stat buffs (`apply_stat_buff`/`expire_buffs`) | `SpellCast` |
| `combat` | `damage`/`die`/`attack`, the three action passes (`commit`/`sustain`/`resolve`), poison pulses, criminal flagging, the swing formula | `MobileDamaged`, `MobileDied` |
| `items` | spawn/drag/stack/decay/containers/equip/doors/mounts/trade, one module each | `ItemSpawned` |
| `ai` | the creature brain: LOS aggro, cached-path chase, give-up, kiting, fleeing, retaliation | — |
| `npc` | townsfolk: generated appearance and names, the greet/face/wander beat, the keyword answers, banker and vendor services, the town guards, and the creature `spawn` rule | `MobileSpawned` |
| `crafting` | the five craft systems and their recipes, the chance curve, the workshop scan, ore smelting, and the craft window | `ItemCrafted` |
| `quests` | the quest model, the objectives, and the quest gump | `QuestAccepted`, `QuestObjectiveUpdated`, `QuestCompleted`, … |
| `housing` | placement and its refusals, the sign and the deed, locks and lockdowns, co-owners and friends and bans, decay, and the house design | — |
| `boats` | a ship as a multi on the water: the berth, the `Boats` tile index, and mooring | — |
| `guilds` | the guild, its five ranks, membership and titles | — |
| `party` | the party, its invitations and its channel | — |

The drawing/interest substrate they share (`show`, `forget`, `broadcast_move`,
`refresh_around`, `reveal`, `mobile_incoming`, …) lives on `WorldState`, in the
`state` crate below them. `world` keeps the tick that sequences the systems, the
client's file formats, and the persistence journal — the orchestration, not the
rules.

**Stubs** — declared so the dependency graph is visible.

`plugins`, `metrics`.

**`crates/e2e/*` — tests, and the exception that proves the direction rule.**

`server/*` and `client/*` never depend on each other: the wire is the only thing
they agree on, and that is what keeps the protocol crate honest. But "a client
can log in to a shard" needs both in one process, and hanging it off either side
would make that side depend on the other — a dev-dependency is still a
dependency, and it is the direction, not the profile, that the rule is about.

So the seam tests live outside both, in crates that ship no code and that
nothing depends on. Only what cannot be tested on one side belongs there: the
framing, the login machine and the tick all have better tests of their own —
pure state machines, no ports, no timing. What `e2e` is for is that two correct
ends actually agree, and it earned its place on the first run by catching a
client that assumed one compressed block was one packet.

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
(spawn-region upkeep), `decor.rs` (decoration and door generation),
`regions.rs` (the region crossing), `ambient.rs` (the world clock and light),
`speech.rs`, `staff.rs`, and the test files. `tick.rs` itself keeps the `World` struct,
the command router and the tick — orchestration, ~750 lines.

### A big table is data, and lives in `data/*.json`

The other way a file gets long is not logic at all. Five files of craft recipes
were 16,106 lines; a body-type table was 469 lines inside a 2,782-line
`components.rs`; 250 NPC names were most of `names.rs`. None of it is code —
it is ported reference data (ServUO's `Def*.cs`, `Data/bodyTable.cfg`,
`SkillInfo.Table`) that happens to be spelled in Rust syntax, and it drowns the
few hundred lines around it that a person actually reads.

**A table of more than a hundred rows belongs in `crates/<group>/<crate>/data/`
as JSON, with a `build.rs` that emits the `const` before the crate compiles.**
Four exist — `crafting`, `state`, `npc`, `world` — and are the pattern:

- **A table is not always spelled as a `const`.** `creature_name` was 91 lines
  of `match` arms and is data by every test that matters: a key, a value, and no
  control flow. Look for the long run of near-identical lines, not for the
  `const` keyword.
- **The generated code keeps the shape the hand-written code had.** A searched
  table stays a `const` slice; `creature_name` stays a `const fn` over a
  `match`, because the compiler turns a dense integer `match` into a jump and a
  search over a slice could not be `const fn`. Moving the data is not licence to
  change the lookup.
- **The generated tables stay `const`.** Two of `state`'s are binary-searched on
  the tick path. Nothing is parsed or allocated at startup, and a caller still
  reads `&'static [Recipe]`; the file it comes from is the only thing that
  changed.
- **Errors move from runtime to build time.** `deny_unknown_fields` on every
  row makes a misspelt key a build failure, and a `Skill::` variant that does
  not exist will not compile. What a runtime load would report on the first
  craft of the day, this reports before the crate builds.
- **Invariants the data must satisfy are the script's job, not the data's.**
  `build.rs` sorts `BODY_TYPES` by id, because `body_type` binary-searches it and
  a table sorted by hand decays the first time somebody appends a row. The same
  script asserts there is no duplicate id — the case a binary search would answer
  arbitrarily, and a `match` would answer with whichever arm came first, so a
  creature quietly wears another one's name.
- **A hundred rows is the threshold, and the other side of it is a shared
  module.** The thirty-row mount table came *back* out of `state/data/` when the
  client turned out to need it too — a saddle on the wire is an item and the
  thing drawn under the rider is a creature, and neither end can derive the other.
  It is `openshard_protocol::mounts` now: small enough to read as source, and in
  the crate both ends of the wire already share. The invariants did not go with
  the `build.rs` — they became the module's own tests, which is what the rule
  above is actually asking for.
- **Prose stays in the source.** The doc comments for the generated items live
  in `build.rs`, not in the JSON: a data file is a poor place to explain why
  ServUO's `StatTotal` sums the *undivided* scales.
- **Repetition is factored in the data, not in the emitted source.**
  `world/data/spawns.json` names its 193 distinct creatures once and lets 1,430
  spawn regions refer to them by name; `build.rs` resolves the references, so
  the emitted code has one table and 8,338 indices into it rather than 8,338
  struct literals. A name with no definition is a build failure that says which
  region asked for it. What the *runtime* sees is unchanged — the resolution
  happens at build time, and `Spawner` never learns the names exist.
- **Converting is verified by round-tripping, or by behaviour.** Every table
  that could be was dumped out of the *compiled* tables rather than parsed out
  of the source text, and the regenerated `const`s dump back to byte-identical
  JSON. Where the layout necessarily changed — the `match` arms, whose grouping
  the generator re-derives — the check is the stronger one: `creature_name` and
  `creature_base_sound` were called for all 65,536 body ids before and after,
  and the snapshots compared.

What is *not* worth moving: `state`'s `WEAPONS` and `ARMOR`, `magic`'s `MAGERY`,
`harvest`'s `ORES`. Each is already one row per line with a constructor function
and the item named in a trailing comment, and that alignment is the readable
part — JSON would lose it and save nothing. The line is roughly a hundred rows,
or the point where the comments stop carrying meaning.

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
- **The table in the source file** — a few hundred rows of ported reference data
  spelled as Rust literals, drowning the code around it. It goes in `data/`.
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

### Item ownership and atomic mutations

`ItemLocation` is the canonical ownership edge. `Position`, `Contained`,
`Equipped`, cursor state, and `ContainedItems` are synchronous projections of
that edge; readers revalidate indexed candidates against it. Container lookup
therefore costs the named container's candidate count, never an iteration over
the world, and restore rebuilds the index through the ordinary establish door
instead of persisting derived membership.

Compound quantity/ownership changes use domain-specific prepare/commit values.
Prepare validates capacity, identity, amount, source revision, and every needed
allocation without publishing gameplay state. Commit has no ordinary failure
branch. Crafting combines a deterministic `WithdrawalPlan` with a prepared
output placement, so an allocation or capacity refusal spends neither inputs
nor tool state and emits no durable craft event.

Recursive backpack craft stock is paid for on canonical mutation and bounded
at 125 descendants. Dense `CraftKey` totals answer catalogue context directly;
ordered piles are revalidated only for the selected authoritative recipe.
Multiple pile changes in one prepared withdrawal suppress intermediate stock
rebuilds and publish one final root revision. House inventory uses a separate
permissioned, epoch-invalidated projection rebuilt under a fixed tick budget;
its Ctrl+I search is read-only. Crafting from house boxes is deliberately not a
side effect of search and remains disabled under the settled `SearchOnly`
policy.

The tick admits at most 256 command work units and 32 coalesced catalogue opens.
Unadmitted work remains FIFO for a later tick; a gameplay mutation is never
paused halfway. The complete contract, limits, release measurements, and
property model live in [`item_transactions_plan.md`](item_transactions_plan.md).

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

## Sessions and the character registry

A character is two different things, and most of the awkwardness in the shard
loop came from the two not being told apart.

- **It exists.** A name on an account, listed on the character screen, there
  whether or not anybody is logged in.
- **It is present.** An entity in the running world, with a serial, a position
  and hit points, there only while somebody is playing it.

So the operations are not symmetric, and it is worth writing the table out
because reading it settles every question below:

| | exists | present |
|---|---|---|
| Create (`0x00`/`0xF8`) | create | instantiate |
| Select (`0x5D`) | — | instantiate |
| Logout (`0xD1`) | — | de-instantiate |
| Delete (`0x83`) | delete | refuse if present |

Character *select* is pure instantiation, which is why it falls out as a single
`Command::Enter` and always did. Create is creation glued to instantiation, and
delete is deletion plus a question about presence — which is why those two are
the ones that kept wanting to reach into the world.

### Two owners, and the boundary between them

*This section said three, and named the binary as the owner of two of them, until
[`connection_state.md`](connection_state.md) S4 and S5 moved them into the world.
What is below is where they are.*

- **Credentials live in `openshard-login`**: a password, a ban, an access level —
  what a *login* is about. Nothing about a character, which is what keeps that
  crate down to `protocol` + `getrandom` + `argon2` and testable as a sequence of
  byte slices.
- **Existence and state both live in the world's roster**
  (`world/src/tick/roster.rs`): the account's characters, in the slot order `0xA9`
  shows and `0x83` indexes, each carrying `Option<CharacterRecord>` — where that
  character was when last seen, or `None` for one that exists and that nothing has
  described yet. They were two lists before, one per crate, and the world's half
  could not see the other.
- **Presence lives in the entity**: a character is being played if it is in the
  world, which is the fact itself rather than a table about it. The binary keeps a
  `WorldPhase` per connection, but only to decide synchronously whether a packet
  may be queued — see below.

The boundary is authentication, and it is one question: **is this the login
conversation, or is it the world's?** The login crate ends at
`Command::Authenticated`, and the character screen — list, create, select, delete
— is answered out of a tick like everything else. What used to argue for the
binary owning it was that select needs the saved record, and pulling
`openshard-persistence` (with bundled SQLite and `tokio-postgres`) into the login
crate would cost that crate its whole value. Moving the screen *into the world*,
which already depends on persistence, is the other direction, and that objection
does not apply.

### Presence is asked of the entity, never of a table about it

*Also reversed by S5, and it is worth keeping the old answer visible because the
reasoning that produced it was sound and still incomplete.* "Is this character
being played?" used to be answered by scanning the session table, because the
world could only be asked about a *serial*, and the only route to a serial from
the character screen was the roster — which a character created during this run
was not in until it logged out. Asking the world therefore skipped the check for
exactly the character most likely to be online, and deleted it out from under the
connection playing it.

What changed is the route, not the argument: the roster is the world's and holds
the account's characters by name, so the world resolves the name itself and looks
for the entity. The question is asked of the thing that *is* the fact, and
`Sessions::is_playing` is gone.

### The world mints serials; the roster only remembers them

A serial is durable — it is what every packet ever sent about a character
referred to, so a restored character must come back on the one it was saved
under. It is tempting to conclude that the registry should therefore own it, and
that `World::reserve_serial` is an inversion showing through. It is not. Items,
NPCs and decoration are persisted and reserve their serials at boot in exactly
the same way; players are not a special case, and serials are one space
(`SerialKind`) shared by all of them. Splitting a player range out would put two
allocators over one pool to fix nothing.

`Command::Enter` says which of the two it is by name rather than by an absent
serial: `Character::Saved` asks the world to play whatever is on file for that
account and name, and `Character::Fresh` carries what a creation chose. The
serial is never on the command at all now — it is on the roster row, which the
world holds.

### Where the login conversation ends

`LoginSession` is a state machine over a client that dials **twice** — the login
socket (`0x80`/`0xA0`, ending in a relay and a key) and the game socket
(`0x91`, ending in the character list). Which socket this is cannot be known
until the first packet and never changes after, so it is a branch of the state
enum, not a flag beside it. Two things follow, and both used to be fields that
had to be kept in step by hand:

- **The account rides in the state**, so there is no `Option<AccountName>`
  meaning "not known yet". It is deliberately readable only on the game socket:
  the question `LoginSession::account` answers is *whose character may this
  connection play*, and a `0x5D` arriving on a login socket that only got as far
  as the shard list must find nothing.
- **A game socket is compressed from the moment the `0x91` is read**, refusal or
  not — Sphere sets `CONNECT_GAME` during the crypt handshake, before the
  password is looked at. Every path out of the game-login handler returns a
  `Game(..)` state, so no new refusal can forget it.

## The world

The entire world is in memory. The database is persistence, never a query
target during gameplay.

The real tick, in order (`world/src/tick.rs`):

```
tick:
  apply queued commands          network input, script output — one order
  ai think / npc live            brains decide; the tick applies the steps
  combat                         sustain/resolve/commit, criminal/murder expiry, poison
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

## Scripting (spiked, then deleted)

**The rest of this section is the record of an answer that was replaced.**
Settled on [#7](https://github.com/youhide/OpenShard/issues/7) and
[#17](https://github.com/youhide/OpenShard/issues/17): this project has no
scripting language. Gameplay *data* is `data/*.json` compiled by a `build.rs` —
what § "A big table is data" already required of the craft recipes and the skill
table — and gameplay *logic* is systems in the domain crates, as
`fn(&mut WorldState)`. All of it is in the tree, and `crates/server/scripting` is
gone. Two costs were accepted in the open when it was decided: writing content
requires Rust, and hot reload of logic goes away.

**What replaced the seam.** `server::content` is the one place content reaches
the world: `boot()` for what is simply true of the shard, `verb()` for what an
operator lays from the staff menu. Both return `Vec<Command>` and queue nothing,
which is what let every dataset be moved under a test comparing its commands
against the pack's — the migration's whole method, and the reason none of it
had to be taken on trust.

The reason the pack was 98.6% data is the reason it went: a spawn table and a
decoration table are the same kind of thing as a recipe table, and one of the two
was in a second repository behind a V8 for no principled reason. What follows is
why that V8 was chosen and what the spike proved — kept, because the decision
above is what overturned it.

The line has moved once already, and it is worth naming where: a system whose
*window the client draws* — the quest log, the vendor gump, the spellbook — has
to be the core's, because the client reaches it through packets a script cannot
answer, and because its state has to survive a restart the script's memory does
not. So `crates/server/quests` owns the quest model and the gump, and the quests
themselves are content — `crates/server/state/data/quests.json` today, the pack
before it. That is the same "default in core, customise in the pack" split as
`magic::spells` and `Pack.loot`, and the rule of thumb it produced is: **if a
binding must outlive a reboot, it is a saved component, not a map in a script.**

`deno_core` embeds V8 in-process. QuickJS was considered and rejected — too slow
for hot gameplay code. A Node sidecar was considered and rejected — IPC latency
lands inside the tick.

This was the largest open technical risk in the project, and the spike has
retired it. `crates/server/scripting` embeds one `JsRuntime` in a single V8 isolate
behind [`ScriptEngine`], a four-method trait with nothing V8-shaped in its
signatures — so the runtime stays replaceable. A script is one more consumer of
the same seam every system uses: domain events arrive through `deliver`, the
engine keeps a small read model from them, and a script acts only by enqueuing a
`Command` the tick applies in order. It never writes the world directly. Ops are
declared with `deno_core::extension!` and `#[op2]`, and every op called from a
hook is synchronous — a tick never awaits.

The benchmark is the point: a hook call costs on the order of a couple of hundred
nanoseconds, so ten thousand mobiles each firing a hook per tick spend a
single-digit-millisecond slice of the 25ms budget. It fits. Numbers and method
are in `docs/roadmap.md` §5.

`ScriptEngine::load` doubles as hot reload — re-evaluating rebinds the hooks in
the live isolate — and `DenoEngine::reload_if_changed` polls a watched file's
mtime so iterating on a hook is save-the-file, not bounce-the-shard.

And it is wired into the running shard. The server (`crates/server/server/src/scripting.rs`)
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
