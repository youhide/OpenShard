# The client — milestones M0, M1 and M1a

> A record. It was phase 9 of the roadmap until 2026-09-02; what is open in this
> domain is now ranked in [`client/README.md`](../README.md).

Our own client, starting with the only part that has to exist either way: the
protocol in the direction a client reads it, and a `crates/client/net` that
connects, logs in and walks into the world. The milestones, and what is already
missing for each, are in [`docs/client/README.md`](../README.md).

- [x] M0 — `server_packet_length`, `frame_server_packet`, incremental Huffman,
      and `ServerPacket::decode` for the login set. `ClientPacket::encode` and
      the rest of the decoders land as a milestone needs them.
- [x] M1 — `crates/client/net`: sans-io connection, login state machine,
      `WorldView`, and `crates/e2e` proving a client reaches the world against
      the real shard
- [x] M1a — walking
  - [x] The decoders that fill a `WorldView`: `0x20`, `0x11`, `0x77`, `0x78`,
        `0x1A`, `0x1D`. `WorldView` now holds every other mobile and every
        ground item, not just the player; `0x11` decodes but is not folded in
        — see `docs/client/design_net.md`.
  - [x] `0x02` with its sequence and fastwalk key, `0x22`/`0x21`.
        `client_net::walk::Walk` sends the steps and predicts where they land,
        because a `0x22` carries no position and only this end knows what the
        acked step was asking for. Two rules are shared with the server rather
        than written twice, which is the part that would have desynchronised
        silently: `movement::intend` (a turn is a whole step, and the world
        edge is not a tile) and `movement::StepCounter`, the client half of
        the sequence rule `WalkSequence` enforces — open at zero, skip zero on
        the wrap, back to zero on a `0x21`. `crates/e2e` walks a burst past
        the pace budget on purpose and compares the position the resulting
        `0x21` carries against the one the client derived on its own; the
        refusal is the only packet that ever states the server's own answer.

## The reopening window, and the overlay that replaced the patch

A locally-closed container, paperdoll or dialog reopened itself a beat
later (2026-08-11): `App`'s own copy of `WorldView` learned of the close,
the shard thread's copy did not, and the next packet that changed anything
nearby cloned the still-open copy over it. First patched with
`link::Command::CloseWindow` alone — the shard thread's copy told too, but
two mutable copies of "what is open" kept in step by remembering to write
both, not by construction. Built out the same day into the honest fix:
`App::locally_closed`, a prediction-and-reconciliation overlay mirroring
`link::Body::predicted`/`corrected` one layer down — `App` no longer writes
its own `view` locally at all, closing sets the overlay and sends the
command, and `reconcile_own_windows` (pulled out of `sync_own_windows` so it
is testable without a real `App`) clears an entry only once a fresh
snapshot agrees the subject is gone.
[`client/evidence/2026-08-15-one-owner-for-a-window.md`](2026-08-15-one-owner-for-a-window.md) has the decision record
and the test that reproduces the original bug.
