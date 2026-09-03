# Operating a shard: what an operator still has no way to do

The `server` domain is built as a *shard* and half-built as a *service*. A shard
starts, runs, saves and stops correctly, and it now publishes what it measures
about itself — `GET /metrics` and `GET /health`, off unless an operator names an
address. What is still missing is every way of *acting* on a shard from outside
the tick: no way to reach in from another process, no way to schedule a stop, and
no gate that notices a dependency arriving under terms this workspace cannot
take.

What is built is [`docs/server/README.md`](../../../docs/server/README.md). This
page is only what is not, and the order to take it in.

## The order, and what each step is waiting for

Cheapest first, and each one is independent of the others unless it says
otherwise.

- [ ] **1. An operator's stop, from inside the world.** A GM command that asks
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

- [ ] **2. Plugin lifecycle, enable and disable.** `crates/server/plugins` is
      declared and empty — the last of the two stub crates, now that
      `crates/common/metrics` is not one. Nothing depends on it and nothing is
      blocked by it; it is on this list because the crate exists and its doc
      promises this page will say when.

- [ ] **3. The administration API.** REST with JWT, so a dashboard or a launcher
      can *do* something to a running shard rather than only read it.

      The read half exists and is deliberately not this: `openshard-metrics`
      publishes numbers over a port with no authentication, and that is the right
      shape for numbers. Authority is a different thing and wants a different
      door. What is worth taking from it is the shape — a `Reading` rendered two
      ways, one socket, one request at a time — and what is worth *not* taking is
      its hand-written HTTP: an API with a dozen routes, bodies and tokens is
      where a framework starts paying for itself.

- [ ] **4. The dashboard and the launcher.** Consumers of (3), and worth starting
      only once it answers something. The map editor is a separate build with a
      plan of its own — [`plans/world/map_editor/PLAN.md`](../../world/map_editor/PLAN.md).

- [ ] **5. A licence gate, and third-party notices on a release.** `cargo-deny`
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
