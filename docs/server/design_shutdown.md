# Stopping a shard: one word, heard everywhere

How a shard stops, as built. There is exactly one stop — a `gateway::Shutdown`
cloned into the accept loop, into every connection task and into the tick — and
`run_shard` returns only once the world is on disk.

The mechanism is the easy half. The rest of this is the manners: a stop that is
correct and still costs somebody something is not a stop anyone wants.

## The order after the loop is fixed

Announce → flush the outbound queue → drop the sessions → end every trade → take
the last full snapshot → await the save task.

It is welded in `Shard::announce_shutdown`, which announces and flushes as one
call, because anything inserted between the announcement and the flush drops the
announcement — silently, in one line.

| | how it behaves |
|---|---|
| how a stop is asked for | Ctrl-C or `SIGTERM`; one day a GM command |
| a save that will not finish | a second signal exits with code 2 and a line naming what was abandoned |
| bytes in flight at the stop | drained, under `DRAIN_ON_STOP` (2 s) |
| what the player sees | a system line, then the hang-up |
| a dead shard thread in a test | a failed test, with the shard's own panic payload |
| "the stop saved the world" | asserted against a reopened SQLite file |

## Decisions

Numbered so a later session can argue with one without reopening all of them.
The record of how each was arrived at — the stages, what each one had to be
amended by, and what it left open — is
[`evidence/2026-07-31-stopping-a-shard.md`](evidence/2026-07-31-stopping-a-shard.md).

**D1. Signals are watched in one function, and `cfg(unix)` lives inside it.**
`SIGTERM` and Ctrl-C mean the same thing to this process, so they end at the same
`Shutdown::stop()`; what differs is only how they are heard. Keeping the `cfg`
inside `stop.rs` keeps `run()` a straight read and keeps the non-unix build from
being a second arrangement nobody exercises.

Installation is a separate, synchronous step (`stop::install`) rather than the
first line of the spawned task. Until the handler is installed, `SIGTERM`'s
default disposition kills the process, so `spawn(watch(..))` followed by anything
that could signal is a window in which the shard dies instead of stopping. The
streams are also held across the first signal: one created fresh for the second
wait would be deaf to a signal delivered between them.

`SIGHUP` is deliberately not a stop and not anything else. It conventionally
means "reload your config", this shard cannot reload one, and mapping it to a
stop would surprise an operator whose terminal closed.

**D2. The second signal is a force-exit, not a second polite stop.** A stop
awaits the save task, and the whole point of that task is that it may be slow — a
wedged Postgres, a disk that has gone away. Without a second door the operator's
only escape is `SIGKILL`, which is indistinguishable to them from the shard
having hung on its own. So: the first signal asks, the second exits with code 2
and a line naming how much was abandoned. `Unwritten` counts the snapshots handed
to the save task and the rows inside them, so the line says what the impatience
cost rather than the one thing the operator already knows.

*Arguable*, and the counter-case is an operator who fat-fingers Ctrl-C twice and
loses the save they were trying to take. That is why the first signal's line says
what the second one will do.

**D3. A stop stops reading immediately and drains what is already written.**
The two halves of a connection are not symmetric at a stop. A packet *read* after
the stop is work queued for a tick that will not run — worse than useless,
because it can still mutate the session it passes through. A packet already in
the *outbox* is something the world decided to say while it was still the
authority, and the client is entitled to it.

The drain is bounded, because it cannot depend on the world being well: the
writer ends when every `OutboxTx` is dropped, which is the tick dropping its
sessions, which is the thing that might be broken in the first place. The
deadline turns "the shard did not stop" into "the shard stopped rudely".

**D4. The goodbye is a plain system line, not a protocol invention.** The client
already draws `SpokenMessage` from the system serial in system hue, and
`WorldState::system_message` already sends one; `World::announce` walks
`WorldState::players` and sends one to each. No new packet, no new era question,
no `Feature::since`.

The text is a constant. A message nobody can vary is a string, not a setting; it
becomes configuration on the day there is an operator command to schedule a stop
and therefore something to vary it *with*.

**D5. The hang-up is the tick's; the connection's deadline is only a backstop.**
Both ends can close a connection, and if that is not settled somewhere it will be
settled differently by each of them. The world is the one that knows it has
finished talking, so *it* hangs up, by dropping its sessions after the flush. The
connection task's bounded wait exists for the case where that never happens.

**D6. The flush is welded to the announcement.** See the order above: it is one
call for a reason, and the test that pins it asserts the *order*, not the
presence of the line.

**D7. A shard thread that panicked is a failure, not a diagnostic.**
`std::thread::panicking()` distinguishes the two cases: unwinding already, so
print; not unwinding, so `resume_unwind` with the shard's own payload, which is
what makes a test report what actually failed rather than that something did.

## What a stop is proved to do

Each of these is a test rather than a claim, and the ones that were checked to
fail are the ones worth trusting:

- a `SIGTERM` flips the `Shutdown` (`stop.rs`, unix only — it signals itself);
- a stop drains what the world queued before hanging up (`gateway`);
- the announcement reaches the player *before* the close, and a connection with
  no entity is told nothing and simply hung up on (`crates/e2e`, and
  `World::announce`'s own test for the negative half);
- a gate that is stopping serves nobody: no id minted, a closed stream, nothing
  on the channel;
- a shard thread that panicked fails the test that owned it;
- and the claim the whole tail exists for — that `run_shard` returns only once
  the world is on disk — is asserted by reopening a real SQLite file with
  `persistence.save_seconds = 0`, so the periodic save cannot be what wrote the
  row.

Two things are *not* proved and are named here rather than assumed. The
force-exit of D2 ends the process, so proving it needs a child process and the
exit status read — the out-of-process test this repository has otherwise avoided.
And only SQLite is proved to be saved on a stop: PostgreSQL goes through the same
`Store` and the same tail with none of it asserted, because there is no server in
CI.

## Where the rest of it is

- The stages, the backlog, and the traps each stage found:
  [`evidence/2026-07-31-stopping-a-shard.md`](evidence/2026-07-31-stopping-a-shard.md).
- What the client does with the announcement it hears:
  [`client/design_net.md`](../client/design_net.md) → "Stopping is one word, and
  everything hears it".
- What is still open — including the operator's stop from inside the world — is
  ranked in [`README.md`](README.md).
