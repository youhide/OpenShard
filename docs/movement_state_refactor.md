# Movement state refactor

## Why this document exists

The client has a reproducible class of movement desynchronisation bugs:
the HUD and logical walk state can show a character moving while the rendered
body remains on an earlier tile. The latest investigation also exposed a
deeper design problem: movement is represented by several partially
independent state machines rather than by one movement core with projections.

This document records the current problems, the intended refactor, and its
implementation status.  The phases below are complete; the acceptance tests
are the executable statement of the ownership rules.

## Implementation status

- [x] Movement facts are split from ordinary mutations, with sequence identity
  carried across the app boundary.
- [x] `PlayerMotion` owns local prediction, its ordered pending chain, and the
  continuous rendered pose.  `Crowd` is an animation/projection consumer.
- [x] HUD, route planning, camera, and frame geometry query named motion
  projections rather than reconstructing player movement from a `Mobile` or
  `Crowd` entry.
- [x] ACK only retires its matching pending identity; it does not restart the
  local trajectory. Reject and relocation snap to the server's position and
  discard the remaining local chain.
- [x] Offline and replay steps use the same core, and trace/DST coverage
  exercises packet ordering, stalled frames, corrections, and random event
  sequences.
- [x] The walk oracle drives the movement core. See below: it did not, and the
  cadence rule that came with `Crowd` was lost in the hand-over.

## The debt the hand-over left, and the gallop that found it

The refactor moved the drawn body from `Crowd` to `PlayerMotion` and said it was
not meant to change movement speed. It changed one thing all the same, and the
thing it changed had no test because the oracle was not looking at it.

**The oracle was measuring the code that no longer draws the body.** `dst.rs`'s
`Sim` read the local pose out of `crowd.drawn_for(me())` while `App` reads it
out of `PlayerMotion::drawn` (`PresentationWorld::project_local_motion`). Every
corridor, ceiling and camera assertion in that file held against a path the
window had stopped using. `Sim` now holds a `PlayerMotion`, predicts through
`accept_local` in the same call `App::step_online` does, folds packets through
`link::fold`, and samples the pose the frame builder samples.

**`Crowd::crossing` did not come across.** A body this client commands used to
be glided over *however long was left until the cadence said it should arrive*;
`GameMotion::start` glided over the nominal hold from the moment the prediction
arrived. Both are the same walk at a walk. At a gallop they are not: the event
loop notices a step's deadline on the display's grid, so the ask leaves up to a
frame late, and a crossing drawn for the full hold from there ends late by the
same amount — a pause on the tile of up to 17% of a 100ms hold, ten times a
second. That is the ragged gallop, and it is why nobody reported a ragged walk.

The rule is now `openshard_movement::crossing_left`, in `common/movement/pace.rs`
beside the four rates, and both `crowd::crossing` and `GameMotion::start` read
it. `GameMotion::since` is what the app's core measures the gap with.

**And a frame of lookahead**, because rescheduling alone cannot remove the pause
— it can only stop it accumulating. `steer::LOOKAHEAD` lets a step leave up to
one glide interval before its deadline *while a crossing is under way*, so the
prediction is queued before the crossing ends and `advance_with_ease` starts it
with the remainder of the same frame. The cadence is unmoved: `next_due` still
chains each deadline from the last. The one visible consequence is written down
in `dst::a_key_released_on_the_deadline_has_already_bought_its_step`.

**What the oracle was missing.** Every measure in `dst.rs` bounded the body from
above — a corridor around the oracle, a ceiling on one frame's ground — and a
body that arrives, stands, and sets off again is inside all of them. `never_stalled`
is the floor, and it is the assertion the complaint was actually about.

Backlog from this pass:

- `Crowd` still runs its own `crossing` for the body it commands, and nothing
  now reads the answer for the local body's position. It is still right for
  everyone else; the commanded branch is dead weight and should go with the
  `Crowd::commanding` seam when something else touches it.
- The turn is exempt from the lookahead by a third boolean on `Steering`
  (`crossing`, beside `walking` and `turned`). Three flags about one clock is a
  state machine written as booleans; the honest shape is one enum naming what
  the deadline belongs to — a crossing, a turn, or a retry that covers no
  ground.
- `dst.rs`'s `Sim` and `MotionKernel` are now two harnesses over the same core.
  `MotionKernel` predates `Sim` having a `PlayerMotion` and no longer earns its
  own existence.

## Observed failure

In the movement trace, one run showed this sequence:

1. A prediction moved the player from `(1434, 1597)` to `(1435, 1596)`.
2. `Crowd` received a transition whose source was `(1434, 1597)` and whose
   drawn position advanced toward the destination.
3. After that transition finished, the logical `prediction` and
   `mobile_at` advanced through more tiles.
4. `stepping_from` became empty and `crowd_drawn` stayed at the previous tile.

The trace also showed frames where `prediction` changed without a logged
command, network prediction, acknowledgement, rejection, or replay event.
That particular file was append-only and did not identify the process on each
line, so multiple runs could be mixed. The diagnostic now writes the process
id on every line. The observation remains important: the current tracing
model made it too easy to mistake one state owner for another.

The double-click on a vendor is likely only the trigger that makes the issue
easy to see. Opening a paperdoll, container, or vendor gump must not alter the
movement state.

## Current state ownership

The same player position is represented in all of these places:

| State | Current role | Problem |
| --- | --- | --- |
| `Walk` in `client/net` | Owns pending protocol steps and the latest local prediction on the network task | Its state is copied into `link::Body`, then copied again on the app thread. |
| `link::Body` | Carries predicted position and `corrected` across the mailbox | It is a snapshot without an explicit step identity or transition identity. |
| `WorldState::prediction` | App-thread copy of predicted tile/facing | A second movement state, separate from the transition that renders it. |
| `WorldView::player.position` | Intended authoritative server view | `apply_mutation` overwrites it with `body.predicted` for every packet, including non-movement packets. |
| `PresentationWorld::player.at` | Render-facing tile/facing | Can disagree with the active `Crowd` transition and is read by steering. |
| `Crowd` | Owns interpolation, animation clock, and drawn position | It is updated separately from the logical prediction. |
| `Steering::goal` and route cache | Owns the movement order and route preview | Its route origin is not the same value used by every other subsystem. |
| Camera follow state | Follows `Crowd`'s drawn body | A centered body can hide world motion and make a logical/render mismatch harder to notice. |

The desired invariant is simpler:

> There is one movement state. Every other value—authoritative view,
> prediction, transition, drawn body, camera target, and HUD route origin—is a
> projection or a query of that state.

## Concrete architectural problems

### 1. Non-movement packets mutate movement authority

`App::apply_mutation` currently folds a packet and then calls
`view.player_stepped(body.predicted.position, body.predicted.facing)`. This is
done for world items, mobiles, vendor/container packets, speech, and other
updates—not only for walk acknowledgements or server relocations.

Consequences:

- `WorldView::player.position` is not purely authoritative data.
- A vendor interaction can look like a movement confirmation in diagnostics.
- The word `server` becomes misleading: the value may be a local prediction
  copied into the view.
- Rebuilding presentation after an unrelated packet can accidentally affect
  movement projections.

Required rule: only a protocol event that changes or confirms the player's
position may update the authoritative movement anchor. A packet about another
part of the world must not touch it.

### 2. The network walk state and app movement state are duplicated

`Walk` already knows the pending step chain, the predicted position, sequence
numbers, and rollback/draining state. The app then stores a reduced copy in
`PredictionState` and separately asks `Crowd` to infer a transition from the
new tile.

There is no single operation that means “accept this step and create the
corresponding render transition.” Instead, several methods must be called in
the right order:

```text
Walk::step
  -> Update::Prediction
  -> PredictionState::apply
  -> Crowd::commanding
  -> Crowd::see
  -> PresentationWorld::player
```

Any missing, duplicated, reordered, or coalesced call can produce a logical
position with no matching visual transition.

### 3. `Crowd` is both a renderer clock and an implicit movement record

`Crowd` is the correct owner of interpolation time, but it is currently also
used to answer what tile the player is “really” leaving via `stepping_from`.
That makes callers reconstruct movement state from an animation structure.

The HUD needed a special fallback from `stepping_from` to
`presentation.player.at`. This is a symptom: route origin should be an
explicit movement query, not a choice between two unrelated stores.

### 4. HUD and steering do not share one movement origin

The route HUD uses the active transition source when available. Steering and
route planning use `presentation.player.at`, which is the predicted/destination
tile. During a glide these are intentionally different tiles, but the policy
for which one represents “where the character is” is distributed across
callers.

The HUD should ask the movement core for a named value, for example:

- `drawn_tile`: interpolated position for rendering;
- `standing_tile`: last reached logical tile;
- `next_step_from`: source of the active transition;
- `route_origin`: the tile from which the next command is planned.

No caller should infer these by inspecting `Mobile`, `Crowd`, or
`PredictionState` directly.

### 5. The mailbox has movement-specific coalescing without movement identity

The update mailbox coalesces consecutive `Prediction` updates but retains
ordered mutations. That can be valid for rendering, but the update type does
not carry a step id, sequence, or transition id that lets the app prove which
prediction a mutation confirms or supersedes.

The protocol walk layer has sequence information internally. The app-facing
movement event should preserve the relevant identity and outcome rather than
passing only a coordinate plus `corrected: bool`.

### 6. Camera locking masks the symptom

The camera follows the drawn body. If it stays centered while the logical body
advances without a matching `Crowd` transition, the world/HUD can move while
the sprite appears stationary. Camera following is not the cause, but it makes
the divergence visually ambiguous.

Diagnostics should report both world-space body position and camera-space
screen position, and the camera should consume the same movement projection as
the renderer.

## Target design

Introduce a client-app movement core, tentatively called `PlayerMotion`.
It should be the only app-thread owner of player movement state after a network
event crosses the mailbox.

Conceptually:

```text
PlayerMotion
├── confirmed: Position + Facing
├── predicted: Position + Facing
├── pending: ordered local steps with protocol identity
├── transition: None | { from, to, started, duration, mode }
├── correction/reconciliation status
└── movement order status (goal, route revision, blocked/stalled)
```

The exact fields can differ, but the ownership rules should not:

1. `accept_local_step` updates prediction and starts/queues the visual
   transition atomically.
2. `accept_walk_ack` retires the matching pending step without creating a
   second transition.
3. `accept_walk_reject` reconciles prediction, clears or rewrites the active
   transition, and reports a correction explicitly.
4. `accept_server_relocation` snaps the movement state and invalidates the
   route/order as appropriate.
5. `advance(dt)` advances interpolation once.
6. `render_state()` returns the body/camera projection.
7. `planning_state()` returns the route origin and logical next-step state.
8. `hud_state()` returns named values for route and movement status.

`WorldView` should remain a record of decoded world facts. Its player position
may be updated only by an actual authoritative player-position event, never by
the generic “apply any packet” path.

The renderer-facing `Mobile` should be rebuilt from `PlayerMotion::render_state`
plus appearance/equipment data. `Crowd` can remain the interpolation
implementation, but it should not be the hidden source of movement truth.

## Refactor order

The refactor should be incremental and keep the client runnable after each
step.

### Phase 1 — make facts and transitions explicit

- Rename the diagnostic `server`/view concepts everywhere they are ambiguous.
- Split `Update::Mutation` handling into movement packets and ordinary world
  packets.
- Remove `player_stepped` from the generic mutation path.
- Add step/ack/reject identity to the app-facing movement event where the
  protocol provides it.
- Keep the current renderer behavior, but add assertions for impossible state
  combinations.

### Phase 2 — introduce `PlayerMotion`

- Move `PredictionState` into `PlayerMotion`.
- Add explicit pending steps and active transition data.
- Make local acceptance and server reconciliation atomic methods.
- Adapt `Crowd` to consume transition commands from `PlayerMotion` rather than
  discovering them from changed coordinates.

### Phase 3 — make all consumers projections

- Make camera follow consume `render_state()`.
- Make route planning consume `planning_state()`.
- Make HUD route/status consume `hud_state()`.
- Remove direct movement reads from `presentation.player.at` and
  `crowd.stepping_from` outside the movement projection adapter.

### Phase 4 — remove duplicate writes and harden tests

- Delete or narrow `PredictionState::set/apply` so movement cannot be changed
  through arbitrary field updates.
- Ensure replay/offline movement uses the same `PlayerMotion` API as online
  movement, with a different event source.
- Add an invariant checker enabled in debug/diagnostic builds.
- Keep the movement trace, but log event id, process id, source, confirmed,
  predicted, transition, rendered tile, and route origin from one snapshot.

## Invariants to assert

At every app-thread boundary:

- A non-movement packet cannot change confirmed or predicted player position.
- `transition == None` implies the rendered body is at the standing/render
  tile, within the configured interpolation tolerance.
- An active transition has `from != to` unless it is an explicit turn event.
- The active transition source and destination are present in the motion core,
  not reconstructed from `Crowd`.
- Every predicted step has one pending protocol identity until acknowledged,
  rejected, or superseded by relocation.
- A rejection cannot leave prediction ahead of the reconciled server position.
- HUD route origin, camera target, and body render state come from the same
  motion snapshot.
- Opening/closing a gump does not change any movement field.

## Acceptance tests

The following scenarios should be automated before the refactor is considered
complete:

1. Double-click a vendor, receive all vendor/container packets, and assert
   that movement state is byte-for-byte unchanged.
2. Accept one step, delay its acknowledgement, and assert that the body
   glides while the HUD route starts at the active transition source.
3. Accept several steps, then reject the oldest one, and assert that all
   prediction, transition, camera, and HUD values reconcile to the rollback.
4. Send unrelated world packets while a step is gliding and assert that they
   do not restart, cancel, or duplicate the transition.
5. Run the same movement sequence through online, offline, and replay sources
   and compare the resulting motion snapshots.
6. Run two clients/processes with one shared trace path and verify that every
   line is attributable to its process and session.

## Non-goals

This refactor is not intended to change movement speed, pathfinding policy,
camera easing, or vendor interaction behavior. Those are separate policy
questions. The goal is to make one movement decision produce one coherent
state and make every consumer read that state consistently.
