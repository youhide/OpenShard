# Client architecture handoff

This is a long-running refactor track for `crates/client`.

## Current checkpoint

The current implementation checkpoint is:

```text
7ebdac0 exercise staged update mailbox
```

The status-window and `own_windows` module changes described below are working
tree changes on top of this checkpoint; no new checkpoint commit has been made.

The checkpoint's working tree also contains the following uncommitted client
track work:

- an online window waits for its first complete `WorldView` before presenting;
- staged-mailbox backpressure is reported once per stall episode and re-armed
  when the App drains it;
- health-bar facts moved from the egui shell into `diagnostics.rs`;
- `openshard-playground --mailbox-load --stall-app-ms 5000` provides an opt-in
  live moving-crowd exercise, with a unit test for its script.
- `PresentationWorld::advance` now advances crowd, static-animation and flame
  clocks together. `advance_presentation_to` owns both the measured interval
  and `last_advance`, and is used by update delivery, frame advancement and
  offline movement before those paths mutate presentation state.
- `diagnostics.rs` now owns the remaining read-only dev-inspection snapshots:
  the complete `Hud` and its one-frame `Pick`. `shell.rs` only adapts those
  facts to egui and returns `Request`; `HighlightTarget` and `HighlightStyle`
  live with their `GraphicsSettings` owner rather than the UI adapter.

Preserve unrelated concurrent changes, especially the server and protocol
work visible in `git status`. `AGENTS.md` is also untracked user guidance, not
part of this client-track change.

Relevant earlier checkpoints:

```text
bcd0b68 separate client presentation boundaries
e49f7e3 avoid allocating mobile layer order
2ca72c5 borrow drawn equipment layers
61a4356 update client architecture handoff
3e33a6c separate client presentation projection
e6e0c82 separate client prediction state
cf06c8b isolate authoritative client world
505e4ed stage client frame delivery
5d1cf8c trim frame copies and isolate update reducer
ffa66ce keep local window mutations on app thread
1e48536 consolidate world mutations in client reducer
4dd8bad refactor client world update ownership
```

Validation currently passes:

```text
cargo check -p openshard-client-app
cargo test -p openshard-client-app --lib
cargo test -p openshard-client-net --lib
cargo test -p openshard-client-render --lib
```

Result: 174 app tests passed, 2 ignored; 77 net tests passed; 483 render
tests passed, 1 ignored. `git diff --check` also passes. The touched Rust files
pass `rustfmt --check`; a workspace-wide `cargo fmt --check` is currently
affected by unrelated worktree formatting changes and must not be used as
evidence for this client track.

## Architecture agreed with the owner

- `WorldView` has one owner: the client event-loop/App model.
- No `Arc<WorldView>` and no shared mutable world ownership.
- The network thread decodes protocol and maintains wire/walk state only.
- Network updates are applied in the App reducer.
- Frame processing is staged:

  ```text
  receive event
    -> mutate authoritative model
    -> rebuild presentation projection
    -> read-only frame snapshot
    -> render
  ```

- Local UI mutations remain on the App thread. `CloseWindow` is not a network
  command anymore.
- `event_loop.rs` should remain a platform dispatcher, not a gameplay or world
  reducer.

## What has been changed

- `link.rs` no longer imports `winit`.
- `Update` distinguishes initial world state, server mutations and local
  movement prediction.
- `App::on_update` owns cross-thread update orchestration.
- `WorldView` is moved into the reducer, mutated, projected and moved back;
  there is no per-update `WorldView` clone.
- Mobile picking uses `mobiles::pick_iter`, avoiding a second owned
  `Vec<Mobile>` solely to adapt `(Who, Mobile)` pairs to the picker.
- Render-mobile equipment is `Rc<[EquipmentLayer]>`, so a frame snapshot
  retains immutable equipment rather than allocating and copying its layers.
- Cross-thread delivery is staged: ordered updates are bounded (with socket
  backpressure), consecutive predictions coalesce, and winit receives one
  wake-up per pending batch.
- `WorldState` names its three state kinds directly: `authoritative`,
  `prediction` and `presentation`.

## Follow-on: UI, frame, and outgoing-wire boundaries

- `app/src/diagnostics.rs` owns read-only inspection DTOs: the complete `Hud`,
  its one-frame `Pick`, `PickedTile`, selection results, terrain overlay, route,
  and health bars. `picking.rs` and `picking_query.rs` depend on that module
  instead of `shell`, so a future non-egui inspector can use the same answers.
  The egui adapter alone resolves notoriety to its health-bar colour.
- `presentation.rs` names the staged snapshot as `PreparedFrame`. It freezes
  the camera and `FrameFacts`; `publish_frame_picks` is the one explicit bridge
  that records the current picture's identities for the next input event.
  Rendering still reads no live camera or input after that boundary.
- `client/net/src/action.rs` owns `Outgoing` and `GumpReply`. Ordinary
  post-login intentions are encoded there, with the session's player serial
  supplied only at encoding time. `app/link.rs` retains the thread, mailbox,
  and `Walk` branch; walking is deliberately not an `Outgoing` action because
  its sequence and prediction state belong to `Walk`.
- The old narrow packet encoder modules (`talk`, `doll`, `combat`, `skill`,
  `interact`) remain available for now. `Outgoing` is the single path used by
  the app, so their public surface can be reviewed separately rather than
  bundled into this boundary change.
- `view::Status` holds the non-positional `0x11` facts for the player's own
  character. Its hit points intentionally remain in `Player::hits`, the same
  field the `0xA1` health-bar update refreshes, so the status window and the
  overhead line cannot disagree. `WindowSubject::Status` is local UI state:
  the Status button opens it and a reply only refreshes its authoritative
  contents. The status frame lives in `client/render/src/status.rs`.
  `own_windows/{sync,paperdoll,skills}.rs` now own their corresponding
  behavior, leaving `own_windows.rs` with generic window interaction.
- The mailbox has a deterministic stalled-window regression exercise: after
  256 ordered updates and 10,000 predictions, the next ordered update remains
  blocked until the frame drains, and that frame receives every ordered update
  plus only the latest prediction. `openshard-playground --mailbox-load
  --stall-app-ms 5000` starts an opt-in moving crowd after entry, so an
  in-process run can exercise the live path through ordinary `MobileIncoming`
  and `MobileUpdate` traffic without replaying whole-region resync snapshots.
  A 2026-08-12 stock in-process run reached the 256-update limit during the
  five-second App stall and logged socket backpressure before resuming; the
  subsequent refilled batch logged one new line, confirming that drain re-arms
  reporting for a later stall. It is still a controlled workload, not a
  production capacity number.
- A connected client now leaves its surface untouched until it has received
  the first complete `WorldView`. The offline viewer still draws its diagnostic
  placeholder at `START`; an online client never briefly shows that placeholder
  while login packets are in flight, then switches directly to the server's
  position on entry.
- A 2026-08-12 local `openshard-playground` smoke run loaded the configured
  client files and logged into its in-process shard successfully. An earlier run emitted repeated
  Vulkan `vkAcquireNextImageKHR` fence-validation errors. This was the
  non-Windows acquire-fence defect fixed upstream in wgpu #9918, so the
  workspace temporarily pins `e904d2eac` through `[patch.crates-io]`; the
  post-patch smoke run entered the world without the validation error. Remove
  that pin once the backport reaches crates.io. It remains separate from
  mailbox-capacity tuning.
- The presentation-clock handoff is now explicit. A network delivery and a
  frame both call `advance_presentation_to`, which derives one elapsed interval
  from `last_advance`, advances every presentation clock, then records the new
  instant. The offline movement path uses it as well before it timestamps a
  local step. The `world` regression verifies that an update-time interval is
  retained by both static animation and the flame clock, rather than being
  lost when the next frame sees almost no elapsed time.

## Frame ownership and allocation result

The frame-level equipment copy has been removed: a `Mobile` clone now only
increments the single-threaded `Rc` handle. `drawn_layers` borrows matching
`EquipmentLayer` values directly from that slice. Its internal
`paperdoll::world_ordered` path keeps the bounded layer order in a fixed array,
so the renderer neither allocates nor copies worn layers. `paperdoll::world_order`
still returns a `Vec<Layer>` where the public paperdoll API requires ownership.

Other clones are either small/local or semantically required by protocol state
updates (paperdoll, skills, container contents, login plan). Atlas rebuild
copies are on a rare eviction path. Do not reintroduce `Arc<WorldView>`.

## Next work items

1. Exercise the staged mailbox against sustained production-like stalled-window
   traffic before tuning its ordered-update capacity. Start from the
   reproducible `openshard-playground --mailbox-load --stall-app-ms 5000`
   diagnostic, then collect a real externally observed traffic profile. The
   controlled in-process run and headless regression establish the mechanism,
   not a production capacity number.
2. Keep the three state boundaries explicit as new fields are added:

   ```text
   authoritative world state
   prediction state
   presentation projection
   ```

3. Keep commits small and run the client app check/tests after each stage.

## Important caution

The repository may contain unrelated user changes when work resumes. Inspect
`git status` first and preserve them. Avoid broad mechanical rewrites of the
large render files until the mobile ownership design is settled.
