# 8. Operations

> Open work and follow-up findings from this phase are tracked in the
> [consolidated backlog](../../plans/roadmap/PLAN.md).

- [x] `config` — TOML, validated at load

## Stopping a shard — the mechanism and the manners are done

A shard stops on one `gateway::Shutdown`, cloned into the accept loop, every
connection task and the tick; `run_shard` returns only once the last snapshot is
written. The design and the order of events are in
[`docs/client/design_net.md`](../client/design_net.md), under "Stopping is one word".

**The manners are a plan of its own: [`docs/shutdown.md`](../shutdown.md), S1–S6,
all in.** `SIGTERM` asks rather than kills, so a shard under systemd saves; a
second signal is a force-exit for an operator whose store has wedged; bytes
already queued reach the wire before the hang-up; the player is told why; a gate
that has been asked to stop serves nobody; a shard thread that dies in a test
fails that test; and the claim the whole tail exists for is asserted against a
real SQLite file rather than believed.

What is left there is S7 — an operator's stop from inside the world, a GM command
with a countdown — and the plan's own backlog, which is where the next session in
this area starts.
