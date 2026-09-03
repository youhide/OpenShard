# The server: where it stands

The canon of the `server` domain — `crates/server/server`, `gateway`, `login`,
`persistence` and `state`. This is the shard as a process: the socket, the login
conversation, the loop that drives the tick, what the world remembers between
runs, and the runtime tables the simulation is written against. What the
simulation *does* belongs to the domains beside it — [`world/`](../world/README.md),
`items`, `combat`, `housing`, `npc` — and what it says on the wire belongs to
[`protocol/`](../protocol/README.md).

**One entry point.** This page answers "what does a shard do today" and says
which document holds the reasoning for each line. Where this page and a design
document disagree, the design document is right and this page is stale.

## The one-line answer

**Two loops and a channel between them.** Network tasks turn bytes into events,
one loop turns events into commands and drives the clock, and everything that
would make that loop wait — a disk, an argon2 hash — leaves it and comes back as
something to react to on the same `select!` as a packet.

```text
  gateway tasks ──> ServerEvent ──> [ the shard loop ] ──> Command ──> World::tick
                                           │                              │
                                           ├────────  Outbound  <─────────┤
                                           │                              │
                                           ├──> [ save task ]  <──── Snapshot
                                           │
                                           └──> [ argon2 ] ──> Verdict ──┐
                                           ▲                             │
                                           └─────────────────────────────┘
```

The loop owns neither half it joins. The gateway is a sans-io state machine with
its own tests; the world is a tick with its own. What is in `server/server` is
the wiring, and it is deliberately thin — logic that starts collecting there
belongs in a crate.

## What the area is, by part

| Part | State | What is left | Held by |
|---|---|---|---|
| Sans-io `Connection`: seed handshake, framing, one task per socket, events onto a channel | ✅ shipping | — | the crate's own docs; [`evidence/2026-08-24-the-foundation-phase.md`](evidence/2026-08-24-the-foundation-phase.md) |
| Sans-io `LoginServer`: `0x80` → `0xA8` → `0xA0` → `0x8C` → `0x91`, one-shot expiring auth keys bound to their account | ✅ shipping | — | [`evidence/2026-08-24-the-gateway-and-login-phase.md`](evidence/2026-08-24-the-gateway-and-login-phase.md) |
| Store-backed accounts, argon2 PHC hashes, config `[[accounts]]` seeding only what the store has never seen | ✅ shipping | — | the same |
| argon2 off the loop: the login conversation suspends and the verdict returns on its own arm, bounded by one permit per core | ✅ shipping | the queue behind the permit is unbounded — row 1 | [`design_connection_state.md`](design_connection_state.md) D1 |
| The connection is a row in the world; the phase is the binary's and only the world moves it | ✅ shipping | two in-between states still unnamed — row 4 | [`design_connection_state.md`](design_connection_state.md) |
| The character screen answered out of a tick — `0xA9`, `0x00`/`0xF8`, `0x83`, `0x5D` | ✅ shipping | never watched with a real client — row 12 | the same, D5 |
| One gate instead of thirty: `dispatch_world_packet` is `fn(ClientPacket, ConnectionId) -> Option<Command>` | ✅ shipping | it cannot name what it dropped — row 10 | the same |
| Snapshot inside the tick, write outside it; the whole world saved, dirty marks off the event bus | ✅ shipping | — | [`design_persistence.md`](design_persistence.md) |
| Three backends behind one `Store` enum, picked by what `persistence.database` looks like | ✅ shipping | only SQLite is proved across a stop — row 5 | the same |
| Boot restore order as a signature: characters → items → mobiles | ✅ shipping | — | the same |
| One word stops a shard: `SIGTERM` or Ctrl-C, the outbox drained, the player told, the world on disk before `run_shard` returns | ✅ shipping | the save await is unbounded — row 2 | [`design_shutdown.md`](design_shutdown.md) |
| A tick-pace watchdog: a window behind its declared rate warns, and `[watchdog] tick_behind_windows` of them stops the shard the way Ctrl-C would | ✅ shipping | — | `server/src/pace.rs`'s own docs |
| An operator's stop from inside the world — a GM command with a countdown | ⬜ not built | [`plans/server/operations/PLAN.md`](../../plans/server/operations/PLAN.md) | the same |
| A shard publishes itself: `GET /metrics` and `GET /health` on `[metrics] listen`, off unless an operator names an address | ✅ shipping | no authentication, and none intended — row 14 | [`evidence/2026-09-03-metrics-and-health.md`](evidence/2026-09-03-metrics-and-health.md) |
| One subscriber for every shard binary: `openshard_metrics::logging::install` | ✅ shipping | the client's two binaries still build their own — row 14 | the same |
| Plugin lifecycle; the REST/JWT admin API | ⬜ not built | `crates/server/plugins` is declared and empty | [`plans/server/operations/PLAN.md`](../../plans/server/operations/PLAN.md) |
| Embedded scripting | ⬜ deliberately absent | spiked, proven to fit, and deleted in favour of Rust and data tables | [`evidence/2026-08-24-the-scripting-spike.md`](evidence/2026-08-24-the-scripting-spike.md) |

## What is enforced, and by what

The rule this domain keeps learning is the one an invariants sweep wrote down:
**a type beats a build-time check, and a build-time check beats a test**, in that
order, and the order is about *when the wrongness is visible* — a type is wrong
while the author is still typing, a build script before the artefact exists, a
test after a commit that may already have been pushed. A rule that stays in prose
is invisible to whoever never opens the file, stays green when the code stops
obeying it, and cannot be found by the search that would prove it was broken.

What holds today, and what would notice if it stopped:

- **The restore order is two signatures.** `restore_items` takes what only
  `restore_characters` returns and returns what `restore_mobiles` will not
  compile without. The compiler notices.
- **The database cannot be touched inside a tick.** `Journal::drain` is
  synchronous over the world's own data and `Snapshot` is owned values, so the
  boundary is a type rather than a convention.
- **The teardown chain of a refused connection is walked end to end** by
  `e2e/shard/tests/refused_teardown.rs` — six links, none of them mockable,
  written down in one module doc beside the test. It found link 4 half-missing on
  its first run: the socket's read half stayed open, so the world kept a refused
  character standing there.
- **A stop is asserted, not believed.** `e2e/shard/tests/stop_saves.rs` reopens a
  real SQLite file with `persistence.save_seconds = 0`, so the periodic save
  cannot be what wrote the row.
- **A shard thread that dies in a test fails that test**, with the shard's own
  panic payload rather than a line about it.
- **The tick's declared rate is measured against a clock.** `pace.rs` is the one
  place in the shard allowed to read the wall clock — outside `World::tick`, so
  replay is untouched — and it compares in whole ticks rather than against a
  margin somebody chose by eye.
- **What the endpoint publishes comes off a running shard**, not a fixture.
  `e2e/shard/tests/metrics_endpoint.rs` starts one, waits for it to close a real
  pace window, and scrapes it over a socket: every unit test in
  `openshard-metrics` feeds the registry by hand and would go on passing if
  `run_shard` stopped publishing tomorrow.

Two crate-wide invariants sit above all of that:

- **The world answers no synchronous question.** `queue(Command)` in, `drain_*`
  and the bus out. Everything the binary needs to decide *now* — may this packet
  be queued — is a projection it keeps itself, and the world moves it by event.
- **Nothing slow runs on the loop.** A disk and a password hash both leave it and
  come back as something to react to. What is left inside is arithmetic.

## What is open, ranked

**1. 🚩 An unauthenticated connection can queue a password check, and nothing
bounds how many.** The semaphore bounds how many argon2 hashes run *at once* —
which is the memory bound, and it is the right one — but the rest queue on the
permit, one Tokio task each, and each holds the `CredentialCheck` it is waiting
to run. A shard under a login flood therefore holds a task and a password per
connection the gateway accepted. The bound that is missing is on the number of
unauthenticated connections with a check outstanding.

**2. 🚩 `save_loop` has no bound and `run_shard` awaits it forever.** The
force-exit of `design_shutdown.md` D2 is the mitigation and not the fix: it now
names what the impatience cost — the writes and the rows the save task had not
finished — but a store that never answers is still a shard nobody can stop
politely. The honest shape of a bound is not obvious, because a deadline that
gives up on a slow-but-working Postgres throws away exactly the writes the whole
shutdown tail exists to keep.

**3. 🚩 The files this area was told to split have doubled since anyone
measured.** `world/src/tick/tests.rs` is **24,687 lines** against a record that
called it 12,964 and the stated ~2k rule; `state/src/runtime.rs` is 5,508 against
2,169 and `state/src/components.rs` 3,823 against 2,108. The split is mechanical
and collides with every parallel session, which is why it keeps being deferred —
but the number in the backlog made the debt look half what it is, which is the
argument for doing it in a session that owns the tree outright. Measure with
`wc -l`, not with the last sentence that mentioned it.

**4. Two in-between states are still unnamed.** `Entering` has no clock: the
world can no longer fail to say whether an entry happened, but a `PlayerEntered`
the shard loop never *reads* would still strand the session. And a creation that
enters the world goes `Outside → Playing` on the event, because moving the phase
optimistically would strand a refused creation in `Entering` with no character —
so there is a window in which the gate drops an in-world packet from a connection
that is about to be playing. No client sends one there; the window is real and
has no name.

**5. Only SQLite is proved to be saved on a stop.** PostgreSQL goes through the
same `Store` and the same tail with none of it asserted. The test is written
against the enum and would run against either; what is missing is a server in CI,
which is a decision about CI.

**6. The force-exit is untested, and structurally hard to test.** The second
signal ends the process, so proving it takes a child process — the shard binary
started, signalled twice and its exit status read. What the line *says* is
tested; that the process leaves with code 2 when it says it is not.

**7. `Running` cannot ask for a stop without waiting for it.** `stop` asks and
joins, and the `Shutdown` it holds is private, so no test can reach the window it
would most like to: the shard stopping while its runtime is still alive. A
`Running::ask` that only set the flag would open that window, and is also what a
test of "a client connected at the moment of the stop" needs.

**8. Nothing in this workspace ever runs rustdoc.** `cargo doc --workspace
--no-deps` is not part of any gate, so a doc comment that has quietly stopped
describing the code — a link to an item that was made private, a paragraph left
behind by a function that moved — is invisible to `cargo test`, `clippy` and
`fmt`, which are the whole of CI. Measured 2026-09-03: **292 warnings across 24
crates**, the record that first counted them having said 67. A fourth command
would say so and `-D warnings` would keep it said; the cost is that somebody has
to clear them first, and the cost grows while nothing is watching. Run the
command for the number — the number in a sentence goes stale unwatched, which is
this entry's own history.

**9. `start_cities` is content living in the binary.** Nine towns with
coordinates, filtered by facet, in `server/src/dispatch.rs` next to the packet
translation. Handing it to the world at boot as configuration is right; writing
it there is not, and it is the kind of thing the Community Pack should own.

**10. The gate cannot say which packet it dropped.** Thirty arms could each name
their own; one gate has only the connection. Naming it would mean either
`ClientPacket`'s `Debug` — which carries bodies, so a `0x03` would put the
player's typing in the log — or a per-variant name table, a second list to keep
in step with the enum. Worth revisiting if a real client ever reaches this path,
because today a misbehaving one is an indistinguishable line per packet.

**11. Forty-nine hand-written `ConnectionId::from_raw(n)` in the world's tests,
and the numbers mean nothing.** `2` is "the other player" in a dozen files, `9`
is "the thief", `77` and `7` are one test each. Minted ids come from a band far
above them so the helper cannot collide, but two literals that happen to agree
inside one test is the same silent conflation one layer down. The shape that
works is `interest_tests`' `ALICE`/`BOB`: a test that wants a second player asks
for one by name.

**12. Two things have never been watched with a real client.** The character
screen answers a tick late — `0xA9` used to go back inside the same call that
read the `0x91` and now waits for the next tick, up to 25 ms — and that is
exactly the kind of thing which is fine in theory and a hang in practice. And
compression must not follow the phase: a game socket is Huffman-compressed from
the moment its `0x91` is read, refusal included, so the flag stays in the
binary's transport and is set once, irreversibly.

**13. Smaller, and each is written where it lives.** `DeleteResult` collapses "no
character in that slot" and "that slot is outside the list" into one log line
because the protocol has one answer for both. A character that is being played is
still on file at its logout position — inert, because nothing reads that row in
the meantime, but it is the "exists versus is played" distinction the roster
record still cannot spell. Three test loops resolve a verification by hand, three
copies of the shard's control flow. `DRAIN_ON_STOP` is a constant that belongs
beside `save_every` in `[persistence]` the day an operator finds it wrong. And
`Shard::announce_shutdown` is the only caller of `World::announce`, so that seam
is unproven until a GM broadcast or a scheduled stop is the second.

**14. What the new endpoint leaves open, and one of the three is deliberate.**
The port has **no authentication**, by design: it publishes numbers and nothing
else, and the place authority belongs is the REST/JWT admin API that has not been
built — so the answer today is "bind it to loopback", which the shard says at boot
when it is not. The other two are real: `openshard_tick_age_seconds` makes a
wedged tick loop visible for the first time, but nothing decides how long is too
long — that is an alerting rule the operator writes, and a shard whose tick has
stopped still answers `/health` with a 200. And the client's two binaries still
build their own `tracing` subscriber (`--log` and a jank directive in the
playground, a `warn` default in `openshard-client-app`), so `RUST_LOG` means one
thing to the shard's binaries and another to those.

**15. The licence gate is not written and its audit is stale.** Nothing notices
when a dependency arrives under terms the workspace cannot take; `cargo-deny`
with a `[licenses]` allow list belongs beside the commands CI already runs. The
audit that would seed it names `cooked-waker` as arriving through `deno_core`,
which was deleted with the scripting spike — so it wants re-running before the
gate is written, and a distributed binary still owes its recipients a third-party
notices file.

## The documents

**Design** — the model as built, no status in them:

- [`design_connection_state.md`](design_connection_state.md) — the seam at
  authentication, the row in the world and the phase in the binary, D1–D7, and
  why the row carries what the client is in the middle of.
- [`design_persistence.md`](design_persistence.md) — the snapshot/write split,
  what is saved and when, one `Store` over three backends, and the boot restore
  order as a signature.
- [`design_shutdown.md`](design_shutdown.md) — one word, the fixed order after
  the loop, D1–D7, and what a stop is proved to do.

**Evidence** — measurements and closed records; none of them is a status:

- [`evidence/2026-07-30-the-connection-state-machine.md`](evidence/2026-07-30-the-connection-state-machine.md)
  — the seven stages that moved a connection into the world, and the backlog they
  left.
- [`evidence/2026-07-31-stopping-a-shard.md`](evidence/2026-07-31-stopping-a-shard.md)
  — the six stages of manners around an already-correct stop, and the trap each
  one found.
- [`evidence/2026-07-31-invariants-nothing-enforces.md`](evidence/2026-07-31-invariants-nothing-enforces.md)
  — six rules the code obeyed and could not state, moved up the ladder one at a
  time; one of them turned out not to be a rule, and the character order it was
  hiding was a real bug on two of three backends.
- [`evidence/2026-08-24-the-foundation-phase.md`](evidence/2026-08-24-the-foundation-phase.md)
  — the workspace, the entity store and the event bus.
- [`evidence/2026-08-24-the-gateway-and-login-phase.md`](evidence/2026-08-24-the-gateway-and-login-phase.md)
  — the login sequence, character deletion, store-backed accounts, and the five
  findings the connection work closed.
- [`evidence/2026-08-24-the-persistence-phase.md`](evidence/2026-08-24-the-persistence-phase.md)
  — what each schema version added, why a spawn region's id is its slot, and the
  two things specific to the PostgreSQL backend.
- [`evidence/2026-08-24-the-scripting-spike.md`](evidence/2026-08-24-the-scripting-spike.md)
  — an embedded V8 measured against the tick budget, and why the answer was to
  delete it.
- [`evidence/2026-08-24-the-scriptpack-conversion.md`](evidence/2026-08-24-the-scriptpack-conversion.md)
  — the converter that was dropped with it, and the shape to copy if a `.scp`
  pack is ever converted.
- [`evidence/2026-08-24-the-operations-phase.md`](evidence/2026-08-24-the-operations-phase.md)
  — what the operations phase actually delivered.
- [`evidence/2026-08-24-the-licensing-audit.md`](evidence/2026-08-24-the-licensing-audit.md)
  — the GPL/MIT contradiction, how it was resolved, and the two things it left
  open.
- [`evidence/2026-09-03-metrics-and-health.md`](evidence/2026-09-03-metrics-and-health.md)
  — what a shard now publishes about itself, the four decisions that shaped it,
  and the three questions it deliberately refuses to answer.

**Plans** — what is not built lives outside `docs/`:

- [`plans/server/operations/PLAN.md`](../../plans/server/operations/PLAN.md) —
  the operator's stop, plugin lifecycle, the administration API, the dashboard
  and launcher, and the licence gate.
