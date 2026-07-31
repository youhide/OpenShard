# OpenShard

A modern Ultima Online server engine, compatible with the original 2D client and
ClassicUO.

**Not a SphereServer clone.** The goal is the engine SphereServer would likely be
if it were designed today: compatible with the UO *protocol*, and with nothing
else about Sphere.

The gameplay content lives in a second repository, the **OpenShard Community
Pack**.

## This file is an index

It is short on purpose. People work on this at very different levels — one
encoder, one gameplay system, the whole tick — and none of them should have to
read an essay to make a small change correctly. So: a rule that needs a paragraph
to justify itself lives in a doc, and what stays here is the rule. The docs are
the source of truth and they are kept current; a summary in this file would be a
copy that goes stale silently, which is worse than no copy at all.

| | |
|---|---|
| [`docs/style.md`](docs/style.md) | **How code here reads.** Newtypes, `unwrap`, `Option`, errors, comments, tests, determinism. The canon — read it before writing Rust in this repo. |
| [`docs/architecture.md`](docs/architecture.md) | The shape: layers, dependency rules, the crate map, how entities/events/protocol/persistence fit together, and what belongs in which file. |
| [`docs/findings.md`](docs/findings.md) | What the client actually does, and how to read Sphere and ServUO without inheriting their mistakes. Every entry cost a day. |
| [`docs/roadmap.md`](docs/roadmap.md) | The order, and what is built. §6 is gameplay, system by system, with the protocol findings and reference-emulator arguments behind each one. |
| [`docs/client.md`](docs/client.md) | **Our own client**, milestone by milestone: what the protocol is missing in the client's direction, and what has to move out of `server/world` before anything draws. |
| [`docs/client_versions.md`](docs/client_versions.md) | **Which clients exist and which are played.** What changes between versions in the files and on the wire, why server and client must read the same `.mul`, and how to obtain a set legally. |
| [`docs/development.md`](docs/development.md) | The environment: the three commands, a toolchain without root, `target/` bloat, the `Cargo.lock` MSRV pin. |
| **Living plans** — [`connection_state.md`](docs/connection_state.md) (what a connection is, and where its state lives), [`protocol_newtypes.md`](docs/protocol_newtypes.md), [`protocol_rewrite.md`](docs/protocol_rewrite.md) | A multi-session refactor each: the decisions numbered so one can be argued with alone, the steps, and **a backlog of what was found on the way and left undone**. A plan whose steps are all `[x]` is not finished — its backlog is where the next session in that area starts. Reality contradicting one of these is fixed in the same commit as the code. |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | What lands and how: branch, PR, review, merge commit. |

## Code style

[`docs/style.md`](docs/style.md) is the canon and has the reasoning. The rules
themselves:

- **Newtypes, not raw values.** `Serial`, `EntityId`, `Graphic`, `Hue`,
  `AccountName`. Carry the type through the whole call tree; `.0` only where a
  value leaves the domain — packet codec, SQL bind, JSON field.
- **No `From`, no `Into`, no `Deref` on a newtype.** Banned, not discouraged:
  each one hands back exactly the coercion the newtype existed to remove, and
  hands it back invisibly. `Deref` is the worst — it needs *nothing* written at
  the call site, so the signature still says `Serial` while the code below has
  gone back to raw arithmetic, and there is no text to grep for. `.into()` is
  inference-driven, so changing one signature silently retargets every conversion
  in every caller. Open a newtype with `.0`, build it with `Hue(n)` — and where
  the wrapper has an invariant, give both directions a name and keep the field
  private, the way `Serial::new`/`Serial::raw` do. Fine: `Debug`/`Display`,
  comparisons, and `From` between error types (that is what `?` is).
- **`unwrap()` where the invariant already holds.** A `?` that cannot fail still
  has to be read, and it does not stay local — one defensive `Result` makes every
  caller return `Result` until nothing distinguishes an impossible failure from a
  real one. `expect` when the line does not say what the invariant is.
- **`Result` for everything from outside the process.** I/O, the database, the
  client's files, and every byte off the wire. A packet is not an invariant, it
  is a hostile input.
- **`Option` means absent, not unknown.** No target, no container, empty slot —
  yes. "Not loaded yet" — no, and a default (`0`, `""`) is worse, because it
  reads exactly like a value somebody chose.
- **Errors are types.** No `String` errors, no `anyhow` in library crates.
- **Avoid `pub use`.** Import from where the type is declared; a re-export gives
  one type two paths and hides who depends on what.
- **Look for it before writing it.** Extend what exists — the existing code has
  been run and the copy has not.
- **Comments explain why, generously.** Invariants and preconditions above the
  item, in as many lines as it takes. Tests name the behaviour they protect.
- **`unsafe` is denied workspace-wide.** Two mutable borrows into one structure?
  Split a slice — see `Registry::for_each2_mut`.
- **Randomness inside a tick comes from the world's seeded rng; timers are tick
  counts, never wall clocks.** Both are what makes the tick replay.
- **A port cites the function it came from.** Take the numbers, audit the
  arithmetic.

`cargo fmt` is not a matter of taste: `rustfmt.toml` sets `max_width = 110` and
`style_edition = "2024"`, and the nightly-only settings are commented in it
because the toolchain pin is stable and stable rustfmt would warn on every one.

## Architecture invariants

[`docs/architecture.md`](docs/architecture.md) has the reasoning and the crate
map. The rules:

- **No global mutable state.** `Registry` and `EventBus` are plain values the
  world server owns. Nothing is a `static`, nothing is a singleton — which is
  what lets tests spin up worlds freely and will let the simulation shard.
- **Systems emit events; they do not call each other.** Combat does not call the
  guild system, it emits `NpcKilled`.
- **Domain events live with the crate that owns the rule.** `openshard-events` is
  machinery only, or it becomes a hub every crate has to agree on.
- **Gameplay rules live in a domain crate, not in `tick.rs`.** A system is a
  `fn(&mut WorldState)`; `world/tick.rs` is orchestration — command dispatch,
  system order, drain/queue plumbing — and never rules. It reached 8,116 lines
  once by absorbing them. **A file over ~2k lines is overdue for a split** into
  child modules of the owning module. For movement the split is
  decide-then-apply: the crate returns an intent (`ai::think_one -> Option<dir>`)
  and the tick calls `self.step(...)`.
- **The database is never touched inside a tick.** The whole world is in memory;
  `Journal::drain` memcpies what changed at one instant and a task nothing waits
  on writes it. Both other emulators stop the world to save it —
  `crates/server/persistence/src/journal.rs` is the argument for why this one
  does not.
- **Persistence marks dirty from the event bus, not from each mutation.** A
  `touch()` beside every `insert` works and then decays silently. The exception
  is logout: `touch` promises to read the entity later and there will be no
  entity, so `Journal::keep` records it before the despawn.
- **Nothing writes to the world from outside the tick.** Network tasks queue a
  `Command`. Acting on a packet as it arrives would run world code on whatever
  thread Tokio picked, and two clients racing would produce a different world
  depending on which packet won.
- **Protocol logic is sans-io.** Parsing and state machines take bytes and return
  events — see `gateway::Connection` versus `gateway::Server`. What is hard here
  is byte boundaries, and a real socket will not reproduce those on demand.
- **Never branch on `Era` for a protocol decision.** Ask
  `version.supports(Feature::X)`. Features did not land in era-sized batches —
  tooltips at 4.0.0a, stat locks at 4.0.1a, tooltip hashes at 4.0.5a, all inside
  "AoS" — and an era check is wrong silently: the client drops the packet rather
  than complaining. Every boundary lives in `Feature::since`, once.
- **`crates/common/*` is below the server.** `server/*` and `client/*` may depend
  on it and never on each other; anything both ends of the wire agree on lives in
  `crates/common/protocol`. The one place both ends may be named is
  **`crates/e2e/*`**, which ships no code — only tests that need a real client
  and a real shard in one process — and which nothing depends on.

## What the client actually does

Each of these is a day somebody already spent. The argument is in
[`docs/findings.md`](docs/findings.md); do not re-derive them. SphereServer and
ServUO are **read** for observed client behaviour, never copied and never
vendored — if checkouts are available, their paths are in `CLAUDE.local.md`.

- **Never trust a length off the wire.** `frame_client_packet` bounds gateway
  memory; nothing downstream re-checks.
- **The game connection never says what the client is** — the version arrives in
  the login seed, and the auth key is the only thing linking the two sockets. Get
  this wrong and a modern client is sent a 1997 character list and desynchronises
  hundreds of bytes later.
- **The `0x8C` relay and the `0xA8` shard list carry the address in opposite
  orders.** Making the two consistent has broken one of them.
- **The server remembers what is on each client's screen** (`World::seen`); there
  is no "what can you see" packet.
- **Distance is Chebyshev**, `max(|dx|, |dy|)` — the client draws a square.
- **Every visible action plays a sound and an animation**, not just a state
  change. A state-only system passes its test and feels dead in the client.
- **The reference's comments lie; its code does not.** Sphere's IP comments say
  the opposite of what its bytes do, and a tiledata flag means what the engine
  *reads* it for, not what the header calls it. Trace the bytes, and pin a flag's
  value in a test next to the constant.
- **The map is in the `.uop`, not the `.mul`.**
- **No client files in this repository, ever** — they are copyrighted. Tests read
  `OPENSHARD_CLIENT` and skip when it is unset. Never commit a path to anyone's
  machine.
- **A benchmark where nothing moves measures nothing**, and **a statistical test
  needs a companion that says the data is real** — both have produced green,
  meaningless results here.

## Decisions already made

These are settled. Don't relitigate them without being asked.

| Decision | Choice | Why |
|---|---|---|
| Client eras | **Multi-era from day one** | Retrofitting versioning means auditing every packet encoder twice. |
| Scripting runtime | **`deno_core` (V8) embedded** | Real JIT in-process. QuickJS is too slow for hot gameplay code; a Node sidecar puts IPC latency inside the tick. |
| Sphere scriptpack | **One-shot `.scp` → TS/TOML converter** | Keeps years of balance data without a runtime SphereScript parser. A build tool, not an engine feature. |
| First milestone | **Foundation first** (workspace, ECS, events) | Chosen over a login-to-walk vertical slice. |
| Language | Rust + Tokio | |
| Persistence | **SQLite or PostgreSQL**, operator's choice | Same `Store` trait; neither is a tier — SQLite runs a live shard fine. Never queried inside a tick. |
| Tooling | TypeScript, React, Next.js | |
| Licence | **GPL-3.0-only**, plus a §7 exception for script packs | A forked engine gets distributed in this scene and owes its source back. Content loaded into the embedded V8 is a separate work, written down in `LICENSE-EXCEPTION` rather than left to the oldest unsettled question in the GPL. |

## Where things stand

The shard runs. `cargo run -p openshard-server` loads the client's map and takes
clients through login and character creation into a shared, ticking world that
saves itself to SQLite or PostgreSQL without ever pausing. Combat, skills, magic,
crafting, items, NPCs, quests and the creature AI are real systems; `housing`,
`guilds`, `metrics` and `plugins` are stubs — a `Cargo.toml` and a `lib.rs` with a
module doc, so the dependency graph is visible. Gameplay is TypeScript on an
embedded V8, hot-reloaded on save. [`docs/roadmap.md`](docs/roadmap.md) is the
current one.

The workspace is three groups, and the group is part of the path:

- `crates/common/*` — `entities`, `events`, `protocol`, `movement`, `config`,
  `metrics`. Below the server; nothing here knows what a tick is.
- `crates/server/*` — `gateway`, `login`, `world` (the tick, the client file
  formats, the persistence journal), `state` (`WorldState` and the tables two or
  more systems read), `persistence`, `scripting`, `server` (the binary, glue
  only), and the gameplay systems: `chat`, `skills`, `magic`, `combat`, `items`,
  `crafting`, `npc`, `quests`, `ai`.
- `crates/client/*` — `net`: the client's side of the wire. Framing and
  decompression, the login conversation, and a `WorldView` of what the server
  has shown. [`docs/client.md`](docs/client.md) is the plan it is being built
  against.
- `crates/e2e/*` — tests only, and the only crates allowed to name both ends.

## Working on this

```sh
cargo test --workspace          # includes doctests
cargo clippy --workspace --all-targets
cargo fmt --all
```

All three are expected to be silent. They are today; keep them that way. CI runs
exactly these on every pull request, so a red build is one of the three and
nothing subtler. Everything else about the environment — a toolchain without
root, `target/` filling a sandbox, the `Cargo.lock` MSRV pin — is in
[`docs/development.md`](docs/development.md).

**Work lands through a pull request.** `main` is protected: no direct pushes, no
force-push, a review, and a merge commit (squash and rebase are off, so the
branch's history is what lands). Branch from `main`, keep the PR to one subject.

**Commit messages carry no signature.** The message text only — never a
`Co-Authored-By:`, `Claude-Session:`, or any line mentioning Claude, Fable, Opus,
or any model or tool. This holds for every repo (the engine and the Community
Pack alike), and for PR bodies too.

## Non-goals

Reimplementing SphereScript. Parsing `.scp` at runtime. Source compatibility with
Sphere. Legacy save formats. Mimicking Sphere's internals.
