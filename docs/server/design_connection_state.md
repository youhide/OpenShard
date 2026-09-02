# A connection: one row in the world, one phase in the binary

What a *connection* is, as built. It is two things on purpose — a row the world
keeps and a phase the binary keeps — and the whole design is about which of them
owns what, so that the two cannot disagree.

The seam is **authentication**. Everything before it is the login conversation
and belongs to `openshard-login`; everything after it is the world's, the
character screen included.

```text
   Authenticated ──> Entering ──> Playing ──> LoggingOut ──> Left
        │                │
        │                └─ Command::Enter is queued, the tick has not applied it
        └─ character screen: list, create, select, delete — all world commands
```

## The two halves

| | who owns it | what it is |
|---|---|---|
| the socket, and whether it is compressed | the binary's `Session` | transport; the compression flag is set once, irreversibly, at the hand-off |
| credentials, auth keys, `0x80`/`0xA0`/`0x8C` | `openshard-login` | not simulation, and never inside a tick — D1 |
| what the world knows about the client | `openshard_state::connection::Connection`, keyed by `ConnectionId` | D2 |
| whether a packet may reach the world | `WorldPhase` on the binary's `Session` | D3, D4 |
| which characters exist, and which is being played | the world's `Roster` and the entities themselves | asked of the fact, never of a copy |

`Connection` is a row and not a component because a connection's lifetime starts
before its character exists and ends after it is gone. Everything the world knew
about a client used to hang off its *entity*, which quietly made "has a
character" the precondition for "can be spoken to":
`WorldState::send_packet` resolves the client version through the player table,
so a connection on the character screen was unreachable from inside a tick, and
unreachable silently.

## Decisions

Numbered so a later session can argue with one without reopening all of them.
The record of how each was arrived at, and what it had to be amended by, is
[`evidence/2026-07-30-the-connection-state-machine.md`](evidence/2026-07-30-the-connection-state-machine.md).

**D1. Login does not move into the world.** Accounts, argon2, auth keys, the
`0x8C` relay and the shard list are not simulation. `Argon2::default()` is 19 MiB
and two passes against a 25 ms `TICK_INTERVAL`; a password check inside a tick
stalls the whole shard for one client's benefit. What moved into the world is
everything *after* the `0x91`.

The hash does not belong on the shard's **task** either, which is not the same
statement: it stays in `openshard-login`, where the credentials are, but the
crate hands it back as work rather than doing it — `LoginServer::handle` returns
an `Outcome`, and `server::verify` runs it on a blocking task. The loop is what
must not wait.

**D2. The world's record of a connection is
`openshard_state::connection::Connection`.** It is deliberately *not* called a
session — the session is the binary's, see D4 — and the two must not share a
name, or a reader has to work out each time which of them is authoritative.

**D3. The phase is an enum, and `Entering` is a state of its own.** It is the
distinction `Option<PlayedCharacter>` could not carry: presence used to be a bool
set as `Command::Enter` was *queued*, while `World::enter` could refuse, so a
session said it was playing, the world had no entity, and the client waited on
"logging into shard" being told nothing. `Entering` is the name of the gap
between asking and arriving; `LoggingOut` is the name of the gap between saying
goodbye and the socket closing.

**D4. The phase lives in the binary and is moved only by the world.**

The first half is forced: the packet router has to decide *now* whether a packet
may reach the world, and the world answers no synchronous question — only
`queue(Command)` in, `drain_*` and the bus out. That rule is why
`World::is_online` was deleted (see [`architecture.md`](../architecture.md)), and
a phase held in `WorldState` would put it straight back by making the router read
the world on every packet.

The second half is what keeps it honest: `Session::enter_world` may only move
`Outside → Entering`, and nothing but a world event moves it further.
`PlayerEntered`, `PlayerLeaving`, `PlayerLeft` and `PlayerRefused` carry the
connection, and `PhaseSync` — cursors drained at the top of `world_tick` — is the
only thing that applies them. The binary's phase is a *projection* of the world's
fact, not a second copy of it: the one direction it can be wrong in is being one
tick behind, and `Entering` is the name of exactly that gap.

Which way the gate falls in each in-between state is not symmetric.
`Entering` may act, because the command queue is ordered and anything queued now
applies after the `Enter` it follows. `LoggingOut` may not, because there is
nothing left for a client to say after "I am leaving".

**D5. The character screen is world commands.** `0xA9`, `0x00`/`0xF8`, `0x83` and
`0x5D` are answered out of a tick, by `world/src/tick/screen.rs`. Two hazards
closed themselves when they moved: `0x83`'s slot indexes the list `0xA9` was
built from — one value in one process, rather than two lists that merely happened
to be ordered alike — and "is this character being played" is asked of the entity
that *is* the fact.

The order this needed is worth keeping written down, because it is why the screen
could not move earlier: until the world owned the saved records it could not
answer `0x5D`, and until the connection row existed it could not answer anybody
who had no entity.

**D6. This does not reopen the decision in `architecture.md`.** That decision —
create, select and delete stay out of `openshard-login` — was about not dragging
`openshard-persistence` (and with it bundled SQLite and `tokio-postgres`) into a
crate whose whole value is that it has neither. Moving them *into the world*,
which already depends on persistence, is the other direction and the objection
does not apply.

**D7. No new crate.** The state is in `openshard-state` beside the rest of the
runtime; the rules are in `openshard-world`. A `crates/server/session` crate was
considered and rejected: it would sit between login and world with a dependency
on both, which is the same seam this design deletes, only with a `Cargo.toml`
around it.

## The row holds what the client is in the middle of

Above the identity fields — `version`, `account`, `access` — every field on
`Connection` is transient state that used to be a map keyed by connection or by
the player's entity, cleared by name in `World::disconnect`. That list was
hand-written, so a map added without a line beside it leaked and nothing caught
it: four of them, the gump tables, had already done exactly that while each one's
own doc comment claimed to be cleared on logout.

A field on the row cannot be forgotten, because removing the row takes it. The
evidence that this is more than a tidy-up is in the field list itself: since the
sweep that put four gump tables there, four more fields have arrived —
`guild_gump`, `healer_gump`, `house_gump`, `craft_catalogue_request` — and none
of them needed a teardown line. `WorldState::forget_connection` is the one place
a connection is let go of, and `disconnect` reads exactly one field off the row
it removed, the cursor, because an item held there is off the ground and in no
container and would be deleted by the row simply ceasing to exist.

**One exception, and it is not an oversight.** `open_containers` is an *inverted*
index, `Serial -> {ConnectionId}`, and every read of it asks "who is watching
this container" as an item inside it changes. On the row it would be a
`HashSet<Serial>` per connection and each of those reads would become a scan of
every connection on the shard — a per-item-change cost that grows with the player
count. So it stays an index on `WorldState`, and `forget_connection` sweeps it.

## What is keyed by a connection rather than by a mobile

Targeting is the case worth naming, because the re-keying looked like a
convenience and was not. Every one of the six sites that raises a targeting
cursor already began by refusing to raise one without a `Client` — a creature has
no cursor to raise — so the connection was what the state was about, and the
entity key made the invariant something to restate at each site instead of
something the type holds. `raise_target` / `take_target` / `has_target` are the
seam and they are total: a mobile with no client gets no cursor and is asked no
questions.

## Where the rest of it is

- The sweep that built this, stage by stage, with the amendments each stage
  forced and the findings it left:
  [`evidence/2026-07-30-the-connection-state-machine.md`](evidence/2026-07-30-the-connection-state-machine.md).
- The phase record of the login area, including the five findings this design
  answered:
  [`evidence/2026-08-24-the-gateway-and-login-phase.md`](evidence/2026-08-24-the-gateway-and-login-phase.md).
- What is still open about connections is ranked in [`README.md`](README.md), and
  nowhere else.
