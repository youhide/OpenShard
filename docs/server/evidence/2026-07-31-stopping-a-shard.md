# Stopping a shard: one word, heard everywhere — and what it owes the player

> **This is a record.** It is the living plan that ran from 2026-07-31 through
> S6, kept as it was written. Its decisions are restated as built in
> [`../design_shutdown.md`](../design_shutdown.md) — where the two differ, the
> design is right — S7 is
> [`plans/server/operations/PLAN.md`](../../../plans/server/operations/PLAN.md),
> and what is still open out of the backlog below is ranked in
> [`../README.md`](../README.md), which is the only status page for this domain.

Living plan. The stop itself landed: a `gateway::Shutdown` is cloned into the
accept loop, every connection task and the tick, and `run_shard` returns only
once the world is on disk. What was written down here is the four ways that stop
was still not what it claimed to be, and the order to fix them in. S1 through S6
are done; S7 — an operator's stop from inside the world — is what is left, and
the backlog at the bottom is where the next session in this area starts.

As with [`connection_state.md`](2026-07-30-the-connection-state-machine.md): when reality contradicts a
decision here, change this file in the same commit that changes the code.

## Why

The mechanism is right and the manners are missing. Each of these is a way a
correct stop still costs somebody something.

1. **Only Ctrl-C is listened for.** `run()` wires `tokio::signal::ctrl_c` and
   nothing else. A shard under systemd — which stops a unit with `SIGTERM` — is
   *killed*, not asked, and loses everything since the last save cadence. That
   is precisely the loss the save-on-stop path was built to prevent, so the
   feature is currently absent in the one deployment that matters most.
2. **Bytes queued at the instant of the stop are dropped.** The connection task
   aborts its writer, so whatever the world had already handed the outbox never
   reaches the wire. Nothing a player can lose depends on it *today* — which is
   the trap: it is the transport for (3), so it has to be fixed first or the
   next item cannot work at all.
3. **The player is told nothing.** A clean, deliberate stop looks from the
   client exactly like the shard crashing: the screen freezes and the connection
   drops. Every other visible action in this engine plays a sound and says
   something (`CLAUDE.md`, "what the client actually does"); the shard's own
   departure is the one event that says nothing.
4. **A shard thread that dies during a test's teardown is a printed line.**
   `Running::halt` reports a panicked join with `eprintln!` because panicking
   inside `Drop` while another panic is unwinding aborts the process. Right
   instinct, wrong resolution: a test whose shard died should fail, and there is
   a standard way to have both.

## The shape this works toward

| | today | after |
|---|---|---|
| how a stop is asked for | Ctrl-C | Ctrl-C, `SIGTERM`, and one day a GM command |
| a save that will not finish | `SIGKILL`, and the operator guesses | a second signal exits loudly, saying what was lost |
| bytes in flight at the stop | dropped | drained, under a deadline |
| what the player sees | the connection dies | a system line, then the hang-up |
| a dead shard thread in a test | a line on stderr | a failed test |
| "the stop saved the world" | asserted nowhere | an end-to-end test that reopens the store |

## Decisions

Numbered so a later session can argue with one without reopening all of them.

**D1. Signals are watched in one function, and `cfg(unix)` lives inside it.**
`SIGTERM` and Ctrl-C mean the same thing to this process, so they end at the same
`Shutdown::stop()`; what differs is only how they are heard. Keeping the `cfg`
inside one small `stop.rs` keeps `run()` a straight read and keeps the
non-unix build from being a second arrangement nobody exercises.

`SIGHUP` is deliberately not a stop and not anything else. It conventionally
means "reload your config", this shard cannot reload one, and mapping it to a
stop would surprise an operator whose terminal closed.

**D2. The second signal is a force-exit, not a second polite stop.** A stop
awaits the save task, and the whole point of that task is that it may be slow —
a wedged Postgres, a disk that has gone away. Today the operator's only escape is
`SIGKILL`, which is indistinguishable to them from the shard having hung on its
own. So: the first signal asks, the second exits with a loud line naming how many
writes were abandoned and a non-zero code. Two deliberate signals is a clear
instruction; anything more patient would be pretending the choice is ours.

*Arguable.* The counter-case is that an operator who fat-fingers Ctrl-C twice
loses the save they were trying to take. The line must therefore say what it is
about to do the *first* time, so the second is informed.

**D3. A stop stops reading immediately and drains what is already written.**
The two halves of a connection are not symmetric at a stop. A packet *read*
after the stop is work queued for a tick that will not run — worse than useless,
because it can still mutate the session it passes through. A packet already in
the *outbox* is something the world decided to say while it was still the
authority, and the client is entitled to it.

The drain is bounded, because it cannot depend on the world being well: the
writer ends when every `OutboxTx` is dropped, which is the tick dropping its
sessions, which is the thing that might be broken in the first place. A deadline
turns "the shard did not stop" into "the shard stopped rudely".

**D4. The goodbye is a plain system line, not a protocol invention.** The client
already draws `SpokenMessage` from the system serial in system hue, and
`WorldState::system_message` already sends one. A shutdown notice is that, to
every entry in `WorldState::players`. No new packet, no new era question, no
`Feature::since`.

The text is a constant for now and becomes config the day there is an operator
command to schedule a stop (S7) — a message nobody can vary is not a setting,
it is a string.

**D5. The hang-up is the tick's; the connection's deadline is only a backstop.**
Both ends can close a connection, and if that is not settled somewhere it will be
settled differently by each of them. The world is the one that knows it has
finished talking, so *it* hangs up, by dropping its sessions after the flush. The
connection task's bounded wait exists for the case where that never happens.

**D6. The order after the loop is fixed, and the flush is welded to the
announcement.** Announce → flush the outbound queue → drop the sessions → end
every trade → last full snapshot → await the save task. Anything inserted
between the announcement and the flush drops the announcement, silently, and the
test in S3 exists because that is a one-line mistake.

**D7. A shard thread that panicked is a failure, not a diagnostic.**
`std::thread::panicking()` distinguishes the two cases `Running::halt` currently
conflates: unwinding already, so print; not unwinding, so `resume_unwind` and
let the payload reach the test harness.

## Steps

Each is a pull request. S2 must precede S3 — the notice needs a wire to travel
on. The rest are independent.

- [x] **S1. `SIGTERM` stops a shard, and a second signal exits it.**
      `crates/server/server/src/stop.rs`: `install() -> io::Result<Signals>` and
      `watch(signals, shutdown)`, replacing the inline `ctrl_c` task in `run()`.
      On unix `Signals` holds an installed `SIGINT` and `SIGTERM` stream and
      selects between them; elsewhere it is Ctrl-C alone. After the first, it
      keeps waiting, and the second exits with `2` — see D2, including the line
      the first one prints.

      Installation is a separate, synchronous step rather than the first line of
      the spawned task, which is a change from how this step was first written.
      Until the handler is installed, `SIGTERM`'s default disposition kills the
      process, so `spawn(watch(..))` followed by anything that could signal is a
      window in which the shard dies instead of stopping — in the binary as well
      as in the test. The two streams are also held across the first signal: one
      created fresh for the second wait would be deaf to a signal delivered
      between them.
      **DoD (met):** `stop::tests::a_sigterm_asks_the_shard_to_stop`, unix-only,
      sends itself `SIGTERM` (`kill -TERM` through `std::process`, so no new
      dependency) and sees the `Shutdown` flip inside a deadline. It installs
      before it signals, and says why.

- [x] **S2. A stop drains the outbox before it hangs up.** In
      `client_session_serve`, the shutdown arm stops reading and awaits the write
      task under `DRAIN_ON_STOP` (a constant beside it, 2 s) instead of aborting
      it; the abort stays as what happens when the deadline passes, and is
      harmless after a drain that finished.
      **DoD (met):** `a_stop_drains_what_the_world_queued_before_hanging_up`
      queues on the outbox *after* `stop()` — which is the order a shutdown
      really happens in, the world hearing the stop before it says anything — and
      reads the bytes, then the zero read. Checked to fail without the drain
      (`early eof`), which is the point of writing it first.

      Note for S3: `a_stop_hangs_up_on_a_client_that_is_already_connected` holds
      its outbox for the whole test and so now takes the full `DRAIN_ON_STOP`
      before hanging up. That is the deadline path working, and it is written
      down in the test.

- [x] **S3. The world says why.** `World::announce(&str)` beside
      `cancel_all_trades` in `world/src/tick/persist.rs` — walk
      `WorldState::players`, `system_message` each — and in `run_shard`, after
      the loop: `Shard::announce_shutdown`, which announces and flushes as one
      call, *then* the destructuring `let Shard { mut world, saves, .. }`, which
      is what drops the sessions. The flush is `Shard::flush_outbound`, lifted
      out of `tick` rather than copied, so there is one loop that sends and not
      two to keep in step.
      **DoD (met):** `a_stop_tells_the_player_before_it_hangs_up` in
      `crates/e2e/shard/tests/in_process.rs` reads events until the close and
      asserts the line was among them — so the assertion is the *order*, not the
      presence. Checked to fail with the flush moved before the announcement,
      which is the one-line mistake it exists for.

      **It also needed a decoder.** Our own client could not read `0x1C`:
      `ServerPacket::decode` had no arm for it, so the notice arrived as
      `Event::Undecoded` and the test could only have asserted on raw bytes. A
      shard announcing something its own client cannot read is not a feature, so
      `DecodePacket for SpokenMessage` is part of this step, with the two
      sentinels (`0xFFFFFFFF` speaker, `0xFFFF` graphic) folded back to `None`
      where the encoder folds them out.

- [x] **S4. `Running` raises what it currently prints.** The
      `std::thread::panicking()` guard of D7: unwinding already, so `eprintln!`;
      not unwinding, so `std::panic::resume_unwind(payload)` — the shard's own
      payload, not a message about it, so the test reports what actually failed.
      **DoD (met):** `a_shard_thread_that_panicked_fails_the_test` in
      `crates/e2e/shard/src/lib.rs` builds a `Running` over a thread that panics
      — the fields are private to the crate, so this is the one place it can be
      done — and `#[should_panic(expected = ...)]` on `stop()` names the thread's
      message, which is what pins the payload travelling rather than a panic
      merely happening.

      The not-double-panicking half is not tested: a test that panics inside a
      panic aborts the runner rather than failing, so there is nothing to assert
      on. What makes it safe is that `halt` is idempotent — the unwind out of
      `stop` leaves the handle already taken, so the `Drop` on the way out joins
      nothing — and that argument lives in the comment on `halt`.

- [x] **S5. A gate that has closed does not spawn onto a dead runtime.**
      `InProcess` is `Clone` and outlives its `Running`, so a dial after the stop
      reaches a `tokio::runtime::Handle` whose runtime is gone. `Gate::serve`
      returns early when `is_stopping()`, dropping the stream so the caller sees a
      closed pipe. Its return type is now `Option<ConnectionId>` — `None` means
      "not served", and no id is minted for a session that will never exist.

      **What `Handle::spawn` does there, checked rather than assumed:** it does
      not panic and does not hang. The future is dropped without ever being
      polled and the `JoinHandle` resolves to `JoinError::Cancelled`. So the
      stream *was* already being closed — by dropping the task that owned it,
      silently and by accident. The refusal makes the same outcome deliberate,
      and covers the case the accident does not: a gate that is stopping while
      its runtime is still alive, where a client would get a whole login
      conversation whose events go onto a channel the tick has stopped draining.
      The accept loop takes the same answer as its `biased` select does, for the
      stop that lands between the two lines.
      **DoD (met):** `a_gate_that_is_stopping_serves_nobody` in
      `crates/server/gateway/src/server.rs` — no id, a closed stream, and no
      `Connected` on the channel. Checked to fail without the guard.

      ⚠️ The e2e half, `dialling_a_shard_that_has_stopped_gets_a_closed_pipe`,
      **passed before the change too**, and is kept knowing that: the cancelled
      task closes the pipe, so the client sees the same thing either way. It pins
      the caller-visible contract — a dial after a stop ends rather than hangs —
      and the unit test beside the code is what pins the mechanism.

- [x] **S6. Prove that a stop saves.** "`run_shard` returns only once the world
      is on disk" is the claim the whole shutdown tail exists for, and nothing
      asserted it: every other stop test runs with no database, where the store is
      a `MemoryStore` and a save that never happened looks exactly like one that
      did. `crates/e2e/shard/tests/stop_saves.rs` — log in, take one acked step,
      stop, reopen the file, find the character at the tile it walked to.

      Two things make it an assertion rather than a coincidence.
      `persistence.save_seconds = 0`, so the periodic save cannot be what wrote
      the row — otherwise the test would pass on a slow machine for the wrong
      reason. And the reader is `SqliteStore` directly rather than
      `boot::open_store`, so the claim is about the bytes on the disk and not
      about the shard's own opener agreeing with itself.
      **DoD (met):** the test, with `std::env::temp_dir` plus the pid for the
      path and a `Scratch` guard that removes the file on the way in as well as
      out — a leftover from a killed run would otherwise be a database that
      already had the character in it. Checked to fail with the snapshot moved
      below `drop(saves)`: the sends land on a closed channel, the `let _ =`
      swallows them, and the character is not in the database at all.

- [ ] **S7. Later: an operator's stop, from inside the world.** A GM command that
      asks for a stop, optionally in N minutes, with the countdown as tick counts
      and the announcements of D4 along the way. The sketch: the world must not
      hold the `Shutdown` — nothing writes to the world from outside the tick and
      the world should not reach outside it either — so the command becomes an
      event the shard reads after the tick and turns into `Shutdown::stop()`.
      S1 through S6 are what make this safe to add rather than a second stop path.

## Backlog, found on the way

- **`DRAIN_ON_STOP` is a constant, not a setting.** Right until an operator has a
  shard where it is wrong; the number belongs beside `save_every` in
  `[persistence]` if it ever moves.
- **`save_loop` has no bound and `run_shard` awaits it forever.** D2's
  force-exit is the mitigation, not the fix. It now says *how much* was
  abandoned — `Unwritten` counts what the save task has been handed and has not
  finished writing, and the second signal's line names the writes and the rows —
  but a store that never returns still leaves a shard nobody can stop politely.
  What is missing is a bound on the await itself, and the honest shape of one is
  not obvious: a deadline that gives up on a slow-but-working Postgres would
  throw away exactly the writes this whole tail exists to keep.
- **The force-exit of D2 is untested, and structurally hard to test.** The second
  signal ends the process, so proving it takes a child process — the shard binary
  started, signalled twice, and its exit status read — which is the out-of-process
  test this repository has otherwise avoided. Worth doing once the binary has any
  other reason to be driven from a test; not worth building the harness for alone.
  What the line *says* is testable and tested — see `Unwritten` in `shard.rs`;
  what is not is that the process leaves with code 2 when it says it.
- ~~**The client decodes `0x1C` and keeps nothing.**~~ `WorldView::journal` now
  holds what was said, oldest first and capped at `JOURNAL_LINES` — a bound
  rather than a `Vec`, because the client this is for stays logged in and
  nothing ever asks it to forget. Two decisions worth knowing: the whole `0x1C`
  is kept rather than a trimmed line, so there is no second type to reconcile;
  and a `0x1B` restart replaces everything *except* the journal, because a
  restart says what is on screen is stale and unsays nothing that was said.
  That last one is `a_restart_replaces_the_world_and_unsays_nothing`, checked to
  fail. **Drawing it is still M4** in [`client/design_windows.md`](../../client/design_windows.md) — this is the
  record, not the window.
- **`Shard::announce_shutdown` is the only caller of `World::announce`.** A GM
  broadcast is the obvious second, and S7's countdown is the third; until one of
  them exists the method is a one-use seam and its shape is unproven.
- **`Running` cannot ask for a stop without waiting for it.** `stop` asks and
  joins, and there is no way to get the first without the second — the `Shutdown`
  it holds is private. That is why S5's e2e test cannot reach the window it
  actually cares about, the one where the shard is stopping and its runtime is
  still alive; by the time a test can dial, the thread is already gone and the
  answer arrives for a second reason. A `Running::ask` that only sets the flag
  would make that window reachable, and would also be what a test of "a client
  connected at the moment of the stop" needs.
- **Only SQLite is proved to be saved on a stop.** S6 opens a file, and the
  PostgreSQL backend goes through the same `Store` and the same tail with none of
  it asserted — a shard whose operator chose Postgres has the claim on trust. The
  test is written against the trait and would run against either; what is missing
  is a way to have a server in CI, which is a decision about CI and not about this
  plan.
- **A stop mid-`Entering` is pinned in the world and not end to end.** That a
  connection with no entity is told nothing — it gets the hang-up and no line —
  is now a test beside `World::announce`
  (`a_shutdown_notice_reaches_the_world_and_nobody_on_the_way_into_it`), with
  the character list asserted on the way past so the negative half cannot pass
  for a connection the world could not address at all. What is still missing is
  the timing: a *real* client whose `0x5D` is queued when the stop lands. That
  needs `Running::ask` below, and even then the window is a race rather than a
  state a test can hold still.
- ~~**`tests::connection()` hands back the same id every time.**~~ Fixed in the
  shape the entry sketched: a counter, `connection()` minting a fresh id each
  call, and the tests that need a *known* id keeping `enter_as`. Two decisions
  worth knowing. The counter is **thread-local**, not a process-wide atomic, so
  the sequence a test sees is its own in a parallel run — but nothing may depend
  on the values, because `--test-threads=1` runs every test on one thread and
  shares the counter; uniqueness is what is promised, and it holds either way.
  And minted ids come from a band far above every id these tests write by hand
  (`MINTED_CONNECTIONS`, `1 << 20`; the largest literal is the `1000` loner in
  `interest_tests`), because the common scene is an `enter` beside an
  `enter_as(.., from_raw(2), ..)` and a minted id that landed on a literal would
  put the two back into one connection — invisibly this time, with the helper
  looking like it was working.
  `entering_twice_through_the_helper_is_two_players_and_not_one` pins the
  mechanism and the consequence (two ids, and two players in the world), and
  `a_minted_connection_is_never_one_a_test_wrote_by_hand` pins the gap rather
  than the constant. Checked to fail with the counter held at one value.

  **One test in the crate was living off the conflation.**
  `entering_twice_on_one_connection_is_ignored` said its subject with two bare
  `enter`s, so "one connection" was an accident of the helper rather than
  something the test stated; it now names the id with `enter_as`. It was the only
  one, out of thirty-odd call sites — the rest bind what `connection()` hands
  back to a local and never asked for the id twice.
- **Forty-nine hand-written `ConnectionId::from_raw(n)` in the world's tests,**
  and the numbers mean nothing: `2` is "the other player" in a dozen files, `9`
  is "the thief" in `region_tests`, `77` and `7` are one test each.
  `MINTED_CONNECTIONS` makes them safe against the helper, but they are not safe
  against *each other* — two literals that happen to agree inside one test is the
  same silent conflation, one layer down, and `interest_tests` already names its
  two `ALICE`/`BOB` because reading them as numbers did not work. The shape is
  that pair, shared: a test that wants a second player asks for one by name.
- **Nothing tests that the playground boots** — carried over from
  [`client/README.md`](../../client/README.md), and now with one more thing to get wrong, since the
  playground stops its shard after the window closes.
- ~~**`run_shard` takes six arguments, and the sixth is one every test passes
  blind.**~~ Done, in the shape the entry sketched: `Reins` in `shard.rs` beside
  `Unwritten` — the stop and the tally as one "what the outside world holds of
  this shard", `Reins::new` for a caller that owns neither yet and `Reins::over`
  for one that already made the `Shutdown` (the binary, which binds the gateway
  with it, and the e2e harness, whose `Running` keeps it). `run_shard` takes
  five arguments and `stop::watch` two, and no test constructs a tally to
  satisfy a signature.
  `reins_over_a_stop_hold_that_stop_and_not_a_new_one` pins the one thing
  `over` can get wrong — a `Shutdown::new()` inside it would compile and leave
  every caller holding a word the shard cannot hear.
- ~~**`packets_for` drains the whole outbound queue and filters.**~~ Fixed: it
  partitions instead, handing back the connection asked about and writing the
  rest of the queue back in order, so two calls in a row are two answers. The
  546 tests that used it as a drain did not notice, which is the measure of how
  invisible the trap was.
  `asking_what_one_connection_was_sent_leaves_the_other_its_own` in
  `world/src/tick/tests.rs` pins both halves — bob's answer survives alice's
  question, and alice's is asserted non-empty first so the surviving half cannot
  pass in a world where nothing reaches anybody. Checked to fail with the
  write-back dropped.

## Status

S1 through S4 are in: a shard under systemd is asked rather than killed, an
operator with a wedged save has a way out that is not `SIGKILL`, what the world
queues on its way out reaches the wire, a player is told why their screen is
about to go, a shard thread that dies during a test's teardown fails that test
instead of printing at it, a gate that has been asked to stop refuses rather
than spawning onto a runtime that is going away, and the claim the whole tail
exists for — that `run_shard` returns only once the world is on disk — is
finally asserted against a real file rather than believed.

What is left is S7, an operator's stop from inside the world, and the backlog
above — four of whose entries are closed: `run_shard`'s argument list is one
`Reins` shorter, `packets_for` answers about one connection without emptying the
world, `tests::connection()` mints a connection rather than naming the same one
forever, and what a shard says on its way out is written down by the client that
hears it. The oldest thing in it is now the unbounded `save_loop`: D2's force-exit
finally names what it costs — the writes and the rows the save task had not
finished — but a store that never answers is still a shard that cannot be
stopped politely. The commit that created this plan is the one that landed the stop
itself; [`docs/client/design_net.md`](../../client/design_net.md) → "Stopping is one word, and everything
hears it" is the design it is built on, and
[`roadmap/08-operations.md`](2026-08-24-the-operations-phase.md) points here rather than
repeating the list.
