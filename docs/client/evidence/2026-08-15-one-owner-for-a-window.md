# The client's window state: one owner, not two

Living plan for a client-side refactor. It starts from a bug fixed on
2026-08-11 — a paperdoll, container or dialog closed on screen and reopened
itself a second or so later — and the plan is what closing that bug the
honest way would take, versus the patch that actually shipped.

As with [`protocol_newtypes.md`](../../protocol_newtypes.md): when reality
contradicts a decision here, change this file in the same commit that changes
the code.

## Why

A window close is deliberately client-only. No packet carries it — the
reference client does not send one either, `App::close_window`'s own doc
says so, and it is correct as protocol behaviour. But "client-only" turned
out to mean something narrower than intended: only *one of two* mutable
copies of the fact "what does this player currently have open" heard about
it.

| | `App::view` (`crates/client/app/src/lib.rs`) | `view` (`crates/client/app/src/link.rs`) |
|---|---|---|
| owner | the window / event-loop thread | the shard thread, one per connection |
| written by | server packets (via `entered`), *and* local closes (`close_window`, `answer_gump`) | server packets only (`fold`), until 2026-08-11 |
| read by | `sync_own_windows`, every frame's draw | `snapshot()`, on every packet that changes anything |

Both are `openshard_client_net::view::WorldView`. The link thread's copy is
the one every `Update::World` is cloned *from*, whole, not as a diff —
`App::entered` then overwrites its own copy with that clone outright
(`self.view = Some(Box::new(view.clone()))`). So a fact the link thread's
copy does not have is a fact the next snapshot erases from `App`'s copy too,
however recently `App` learned it locally. A closed paperdoll is exactly
that fact: `App` marked it closed, the link thread's copy still called it
open, and the next packet that changed *anything nearby* — an NPC's own
step was enough, no server message about the paperdoll at all — cloned the
still-open copy over the closed one. The window reopened itself with no
packet logged for it, which is what made the bug hard to place: nothing
server-side was wrong, and the two client-side copies never once got a
chance to be compared, only overwritten in one direction.

**What shipped instead of the honest fix.** `link::Command::CloseWindow` /
`link::CloseTarget`: a command that, like every other `Command` variant,
crosses the channel but not the wire, and the link thread applies it to its
own `view` in the `commands.recv()` arm — the same `paperdoll_closed` /
`container_closed` / `gump_closed` methods `App` already called on its own
copy. `App::close_window` and `App::answer_gump` now call both. It is a
correct patch — closing a container, a paperdoll or a dialog reaches both
copies now, and the reopen is gone — and it is a patch: the fix is "remember
to write to both," which is exactly the invariant nothing enforces. The next
local-only fact this client learns about a window — and there is at least
one already, see the backlog below — gets to fail the same way once before
somebody remembers this file.

## The shape this works toward

**There should be one mutable `WorldView`, not two kept in step by
convention.** The candidates:

**Option A — the link thread is the only writer; `App` reads a snapshot and
predicts nothing.** `App` stops calling `paperdoll_closed`/etc. on its own
copy at all; a close is *only* a `Command`, and `App`'s picture of "what is
open" updates on the next `Update::World`, same as every other fact it
learns. Simplest to state, and it introduces a visible lag: the window a
player just closed stays drawn for up to one round trip through the
channel — which, same-process, is a handful of commands' width, not a
network RTT, but it is a frame or more of a window the player just told to
go away still being there to click through.

**Option B — `App`'s copy is a prediction, reconciled the way `Walk` and
`Body` already are.** This client already has the shape for "we know
something locally before the authoritative side confirms it" —
`link::Body::predicted` / `corrected`, `client/net`'s `walk` module — built
for exactly the same problem one layer down: the body's position, not a
window's openness. A close would set a local, provisional "not open"
overlay (`App::locally_closed: HashSet<WindowSubject>`, say) *and* send the
`Command`; `sync_own_windows` would check the overlay beside `view.paperdolls`
when deciding what is wanted; the overlay entry would clear the moment a
fresh snapshot agrees the window is gone, the same reconciliation `Body`
does on every corrected step. No visible lag, and the duplication moves from
"two functions that must independently stay in sync" to "one overlay type
with a documented reconciliation rule," which is the same trade the walk
handshake already made once.

Neither is chosen yet — see Decisions. Both retire the CloseWindow patch's
actual defect, which is not that it is wrong, but that it is *invisible*:
nothing stops a third window kind from being added with only one of the two
copies told about its close, the way this one nearly was.

## Decisions

**D1. Option B.** `link::Body` already carries this exact shape one layer
down: `Walk::predicted` is drawn ahead of the server's word, `Body::corrected`
is the flag that says the guess was wrong and the server's word replaced it,
and the reconciliation happens the moment a fresh snapshot disagrees — never
by a second call site remembering to write the same fact twice.
`link::Command::CloseWindow` is a `Command` already, same as a step; what it
is missing is `Body`'s other half, the local overlay that lets `App` draw
"closed" before the round trip confirms it.

Option A was the alternative and is rejected. It is less code, but the frame
or more of staleness it costs is not free the way it looked at first: it is
the exact user-visible symptom — a window the player just told to go away
still being there to click through — that this plan exists to remove, just
made deliberate and bounded instead of an unbounded reopen. Trading "closes
sometimes reopen" for "closes always lag by a frame" is progress, but it is
not the fix; a client-only fact should not have to wait on the shard thread
to be true on screen, any more than a predicted step should wait on a `0x22`
to move. Option B costs one new overlay type; the project has already paid
that cost once, for the same reason, in `walk`.

So: `App` gains a local-close overlay (`locally_closed: HashSet<WindowSubject>`,
per the sketch above), set by `close_window`/`answer_gump` alongside sending
the `Command`, checked by `sync_own_windows` beside `view.paperdolls` etc.,
and cleared the moment a fresh `Update::World` agrees the subject is gone —
mirroring `Folded::corrected`'s role in `link.rs`, not inventing a new
reconciliation rule for it.

**D2. `App`'s copy is not the source of truth for anything, in either
option.** Whichever shape wins, drawing code reads `App::view` because it is
where the last-known picture lives, never because `App` is where a fact
about the world is *decided*. The link thread's `view` — or, if D1 lands on
Option A, the snapshot it last sent — is what a disagreement is checked
against.

## Steps

Nothing beyond the immediate patch is built. Each of these is a session on
its own.

- [x] **S0. Stop the reopen.** Done, 2026-08-11. `link::Command::CloseWindow`
      / `link::CloseTarget`, applied from `App::close_window` and
      `App::answer_gump`. This is the patch the rest of this plan proposes to
      retire, not extend — a fourth window kind should not get a third
      call site added to it while this plan is open.
- [x] **S1. Pick A or B.** Done, 2026-08-11. Option B — see D1.
- [x] **S2. Build it, and delete the patch.** Done, 2026-08-11. `App` gains
      `locally_closed: HashSet<WindowSubject>` (`crates/client/app/src/lib.rs`);
      `close_window` and `answer_gump` insert into it instead of calling
      `paperdoll_closed`/`container_closed`/`gump_closed` on `self.view`
      directly — that copy is never written locally again, per D2. The
      membership logic `sync_own_windows` ran inline was pulled out to a free
      function, `reconcile_own_windows`, so it is exercisable without a real
      `App` (which needs client asset files to construct at all — the same
      reason `dst.rs` mirrors `App`'s walk loop instead of driving it). It
      reconciles the overlay against `view` first — an entry survives only
      until `view` itself agrees the subject is gone, the moment
      `Folded::corrected` would clear a mispredicted step — then treats an
      overlaid subject as closed in both the drop pass and the *reopen* pass:
      the dialog "wanted" loop had the same latent bug as the paperdoll one
      that shipped, since `answer_gump` never removed its subject from
      `own_windows` either — caught only once this was in one place to see.
      `App::close_window` and `App::answer_gump` each call one thing per
      close now, not two.
- [x] **S3. A test that would have caught the original bug.** Done,
      2026-08-11. `a_closed_paperdoll_does_not_reopen_on_an_unrelated_world_change`
      in `crates/client/app/src/lib.rs`'s test module: opens a paperdoll, closes
      it, then reconciles against the *same*, unchanged `view` — standing in
      for the snapshot a `0x20` or an NPC's step would clone from the link
      thread's still-unaware copy — and asserts the window stays closed and
      the overlay survives; then reconciles once more against a `view` with
      the entry actually gone, and asserts the overlay clears.

## Backlog

- **Skills is the one window kind `WorldView` cannot answer for at all.**
  `WindowSubject::Skills` closes by clearing `App::skills`/`held_skill`
  directly — there is no `view.skills_closed` to call because the tree is
  never server state to begin with (`close_window`'s own comment: "the
  skills stay where they are, the way a paperdoll's equipment does"). Not a
  bug of this shape — there is only one copy of that fact, so it cannot
  disagree with itself — but worth naming here so whoever does S1/S2 does
  not go looking for a `WorldView` method that was never supposed to exist.
- **`reconcile_own_windows` re-derives `wanted` from `view.paperdolls`/
  `containers` every frame** (`crates/client/app/src/lib.rs`). Cheap today —
  the maps are small — but it is now also walking `locally_closed` on every
  call; still a `HashSet` lookup per entry, not a scan, so this is unchanged
  in shape, just worth re-confirming if either set ever stops being small.

- 🚩 **The direct writes are not gone, and this file said they were.** Found
  2026-08-15, while chasing five doc comments that named a
  `link::Command::CloseWindow` no reader could find — the variant is genuinely
  retired, and those comments were the whole of what was stale about *it*. What
  they led to is this: `App::apply_close_window`
  (`crates/client/app/src/net_command.rs`) still exists and still calls
  `paperdoll_closed` / `container_closed` / `gump_closed` on `App`'s own view,
  from three call sites in `own_windows.rs`, each one line above the
  `locally_closed.insert` that was supposed to replace it. So the fact is
  written twice, in two places, by convention — which is the exact shape S2 was
  written to end, arrived at from the other direction.

  **It is not currently a bug, and the reason is worth writing down before
  somebody relies on it.** The reopen this plan exists about needed the link
  thread's copy to be cloned over `App`'s, and that happens **once**: `snapshot`
  is called at world entry and nowhere else in the steady state
  (`crates/client/app/src/link.rs`), everything after being an
  `Update::Mutation` folded into `App`'s own copy. The two copies therefore no
  longer race. Whoever makes the link thread publish a second snapshot brings
  the original bug back with it, and this note is what says so.

  Untangling it is a session: the direct write is load-bearing for three window
  kinds (it is what lets `reconcile_own_windows` drop the overlay entry) and
  absent for the fourth — `WindowSubject::Vendor` sets only the overlay — so
  the two paths are not interchangeable today and cannot simply be deleted.

## Status

**S0 through S3 have landed, and one claim this section made about them was
wrong** — see the last Backlog entry. The overlay is built and a test exercises
the exact scenario the original bug report used; what is *not* true, and was
stated here as though it were, is that the `CloseWindow`/`CloseTarget` patch's
direct writes to `App`'s own `view` are gone. The `Command` half is gone. The
writes are still there.

This plan exists because the patch that fixed the visible bug was flagged,
correctly, as a shape rather than a cause — see
[the client milestones](2026-08-24-the-client-milestones.md), which point back
here for what to do about it.
