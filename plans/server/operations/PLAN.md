# Operating a shard: what an operator still has no way to do

The `server` domain is built as a *shard* and unbuilt as a *service*. A shard
starts, runs, saves and stops correctly, and an operator watching it has log
lines and nothing else: no metrics, no health endpoint, no way to reach in from
outside the process, no way to schedule a stop, and no gate that notices a
dependency arriving under terms this workspace cannot take.

What is built is [`docs/server/README.md`](../../../docs/server/README.md). This
page is only what is not, and the order to take it in.

## The order, and what each step is waiting for

Cheapest first, and each one is independent of the others unless it says
otherwise.

- [ ] **1. Metrics, tracing and a health endpoint.** `crates/common/metrics` is
      declared and empty; its module doc points here. The shape wanted is in
      [`docs/architecture.md`](../../../docs/architecture.md). Two things are
      already measured and have nowhere to be published: `pace.rs`'s tick-rate
      windows (observed rate, busy share, worst tick and the commands in it) and
      `Unwritten`'s save backlog. Start from those rather than from a blank
      registry — they are the two numbers an operator is currently guessing at.

- [ ] **2. An operator's stop, from inside the world.** A GM command that asks
      for a stop, optionally in N minutes, with the countdown in tick counts and
      the announcements of [`design_shutdown.md`](../../../docs/server/design_shutdown.md)
      D4 along the way.

      The sketch, and the constraint that shapes it: the world must not hold the
      `Shutdown` — nothing writes to the world from outside the tick and the
      world should not reach outside it either — so the command becomes an event
      the shard reads after the tick and turns into `Shutdown::stop()`. That is
      the same shape `report_pace` already uses to stop a shard that has fallen
      behind, so this is a second caller of an existing seam rather than a second
      stop path.

      It is also what turns the shutdown notice from a constant into
      configuration: a message nobody can vary is a string, and this is the first
      thing that could vary it.

- [ ] **3. Plugin lifecycle, enable and disable.** `crates/server/plugins` is
      declared and empty in the same way. Nothing depends on it and nothing is
      blocked by it; it is on this list because the crate exists and its doc
      promises this page will say when.

- [ ] **4. The administration API.** REST with JWT, so a dashboard or a launcher
      can ask a running shard something. Wants (1) first: an admin API whose
      first endpoint is a health check that does not exist yet is two features in
      one commit.

- [ ] **5. The dashboard and the launcher.** Consumers of (4), and worth starting
      only once it answers something. The map editor is a separate build with a
      plan of its own — [`plans/world/map_editor/PLAN.md`](../../world/map_editor/PLAN.md).

- [ ] **6. A licence gate, and third-party notices on a release.** `cargo-deny`
      with a `[licenses]` allow list, beside the commands CI already runs.

      **Re-run the audit before writing the list.** The one on file names
      `cooked-waker` (MPL-2.0) as the tree's only copyleft-only dependency and
      says it arrives through `deno_core` — which was deleted with the scripting
      spike, so the finding may no longer exist at all. The record is
      [`docs/server/evidence/2026-08-24-the-licensing-audit.md`](../../../docs/server/evidence/2026-08-24-the-licensing-audit.md);
      it also names what a distributed binary still owes its recipients, which is
      a notices file rather than a lint.

## What this plan does not carry

- **Progress.** When one of these lands, the row it changes is in
  `docs/server/README.md`'s table, and what the work found is a record in
  `docs/server/evidence/`. This page loses a box and gains nothing.
- **Defects.** Everything wrong with what is already built — the unbounded
  verification queue, the unbounded save await, the two unnamed connection
  phases — is ranked in `docs/server/README.md` § what is open. A defect is not
  waiting for a plan; it is waiting for somebody.
