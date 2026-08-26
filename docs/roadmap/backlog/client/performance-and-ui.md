# Performance and UI findings

[Client backlog](README.md) · [Backlog](../README.md) · [Roadmap](../../README.md)

## Backlog from the frame-cost instrumentation

The `frames` panel measured a frame with a clock on the event-loop thread, which
can only see half of one. `queue.submit` returns without waiting, so
`Frame::scene` stopped when the *encoding* did and every pass was still ahead;
the device's work reappeared a frame later inside `get_current_texture`, where
`Frame::wait` recorded it under a comment calling it "the pacer working". Under
`PresentMode::Fifo` a saturated GPU and a client asleep on vsync were therefore
the same reading, and the panel could not tell them apart.
`crates/client/app/src/profile.rs` closes that: a timestamp query around each
pass in `App::draw`, a `gpu` row and curve beside `ui`/`world`/`waited`, and a
`puffin` sink on `OPENSHARD_PUFFIN` for the CPU flamegraph. What is left:

- **`Frame::wait` is still one number for two facts.** The `gpu` row now says
  which fact it is, but it says so *beside* the field rather than in it — a
  reader has to do the comparison, and the panel does it for them in a sentence.
  Splitting the acquire stall into "the display held the last frame" and "the
  swapchain had no image because we did" would need something `wgpu` does not
  expose today, which is why it is a sentence and not a field.
- **The GPU number is two or three frames old and is recorded against the
  current one.** Right for a standing cost and wrong for a spike: a repack's own
  frame and the `gpu` reading beside it are not the same frame. The ring would
  have to carry a frame index for the two to be joined up, and nothing yet needs
  it.
- **The scopes are closed by hand.** `profile::begin`/`profile::end` rather than
  the RAII scope `wgpu-profiler` offers, because the guard borrows the encoder
  and every pass in `App::draw` would gain a block. A forgotten `end` is caught
  by `end_frame` and logged, so it is loud — but a scope guard would make it
  impossible, and `App::draw` is overdue a split into per-pass functions that
  would make the block free.
- **Nothing times the CPU below `draw`.** The `puffin` scopes are one span for
  the whole draw. The interesting divisions — `frame::assemble`, the atlas
  growth, `light::collect`, `occlusion::bake` — each want a `profile_scope!`,
  and that is the change that makes the flamegraph worth opening at all.
- **`PresentMode::Fifo` is not switchable at runtime.** Unmasking the true frame
  ceiling means editing `App::create_window` and rebuilding. A flag would make
  "is this vsync or is this cost" a ten-second question; it is currently a
  recompile.
- **The lighting pass is measured twice, by two harnesses that cannot be
  compared.** `crates/client/render/tests/cost.rs` batches it offline with
  `poll(Wait)` and divides down; this measures it in the frame as played. Both
  are right and neither validates the other — the offline one runs the pass
  `REPEATS` times back to back, which is a different cache state from one pass
  among a dozen others.

## Lighting architecture after the point-source regression

The close-zoom house case exposed the present cost model rather than a cost of
the house: the full-screen blit shades every output pixel, then for every light
and every soft-shadow sample walks the occluder BVH. With the carried light at
`reach = 4`, `flame_radius = 0` and `shadow_rays = 32`, all 32 samples were the
same ray; collapsing that exact point source to one walk took `blit: lighting`
from 27–30 ms to 0.86–0.91 ms in the live `--jank-log` reproduction. That fix is
an algebraic specialisation, not a new lighting architecture. Finite-radius
lights and scenes with several lights still have the old scaling. In order of
likely leverage, what is left:

- [ ] **Prototype a per-light shadow/visibility representation.** Build
      visibility once for a light, then let the full-screen resolve sample that
      result instead of walking the world BVH from every covered fragment. The
      prototype must preserve the current 2.5D occluder, translucency,
      finite-radius penumbra and CPU/GPU parity cases; compare image output and
      GPU time against the direct walker at near and far zoom before choosing
      its resolution or representation. This is the structural change most
      likely to help one large soft light, including the house reproduction.
- [ ] **Cache visibility for static light/occluder pairs.** Key invalidation on
      the light transform and the occlusion generation; moving carried lights
      remain dynamic. Do this only after visibility is an explicit product of
      the previous item — caching the current per-fragment walk would add state
      without removing its dominant work.
- [ ] **Bound each point light's screen work.** Derive a conservative screen
      rectangle or light volume and shade only pixels its reach can affect,
      retaining a full-frame ambient/tonemap resolve. Prove the bounds at every
      supported camera rotation, zoom, elevation and frame edge so a pool never
      clips. This helps small and off-screen lights; it will not rescue the
      carried light while that light covers most of a close-zoom frame.
- [ ] **Bin visible lights into screen tiles (tiled/clustered lighting).** Give
      each screen tile only the lights whose conservative bounds overlap it,
      rather than making every fragment consider the whole frame's light array.
      Measure list construction, storage and divergence as well as shader time.
      This targets scenes with several local lights and is deliberately behind
      per-light visibility for the one-large-light case.
- [ ] **Split the monolithic resolve into specialised pipelines.** Keep the
      already-cheap identity and flat-ambient routes out of the full lighting
      shader, and separate point-light, sunlight and diagnostic variants where
      measurements show branch or register pressure. Pipeline selection must
      be made from frame-level facts and retain a single shared definition of
      the lighting equations; otherwise this exchanges GPU cost for shader
      drift. Record compiled-shader statistics where the backend exposes them.
- [ ] **Investigate adaptive or temporal soft shadows last.** Vary samples by
      penumbra/error and reuse stable visibility between frames only with a
      deterministic quality ceiling, explicit history invalidation and tests
      for moving lights, camera motion and changing occluders. This may reduce
      finite-radius cost, but unlike the point-source collapse it is an
      approximation and must not silently become the default quality knob.

Every item is gated by the live close-zoom house capture plus a multi-light,
finite-radius capture in `--jank-log`. Report lighting-pass GPU time and image
parity separately: a lower whole-frame number under FIFO is not sufficient
evidence, and neither is the point-source case by itself.

## The party left egui: a yes/no plate and a manifest — backlog

The two party windows were the last of this client's own interface drawn as
`egui::Window`s over the gump layer, and both are gump windows now: the
invitation is `crates/client/render/src/confirm.rs` on the reference's own
`0x0816` question plate (`panes::confirm`), and the roster is
`crates/client/render/src/party.rs` on the `0x0A28` manifest (`panes::party`).
Both are reconciled from the view — `party.invited_by` and `party.members` — the
way a `0xB0` dialog is, so neither has an openness kept anywhere but in
`Windows::own_windows`. `Link::accept_party`, `decline_party`, `add_to_party`
and `remove_from_party` are gone with them: a pane names `Effect::Net` and never
holds a `Link`. What is left:

- ~~**Three window kinds now carry the same `hit()`.**~~ **Closed.**
  `gump::pick_hit` is that one function, generic over the `Hit` type and over
  whether the caller's index-to-meaning table is a `BTreeMap` or a `Vec` —
  `gump::Window::hit`, `confirm::Window::hit` and `party::Window::hit` are all
  now one line each, calling it.
- **A party member is named by serial, in both windows.** No packet in this
  path carries a name — a `0x78` invitation does not, and the `0xBF 0x06`
  roster does not — so both draw `0x0000002A`. The names this client *does*
  have arrive by single click and by tooltip (`view.paperdolls`, the `0xD6`
  cache), and neither is consulted: a lookup that answered "not yet" for most
  rows would be worse than a number that is always right. Worth revisiting
  when the tooltip cache is keyed for this.
- **Two controls on the reference's manifest have no packet here.** The
  per-member *Tell* buttons address one member and `Outgoing::PartySay` only
  addresses the whole party; the loot-type toggle needs a party-loot request
  `Outgoing` has no arm for. Both are left off the plate rather than drawn dead
  — see the module docs.
- **A question is not modal, and the reference's is.** `QuestionGump` is
  `IsModal = true`; this one is an ordinary window because z-order is the
  manager's (decision 2 in `window_components.md`) and "nothing under me may be
  clicked" would be a second z-order policy living in a pane. If a question ever
  needs to be answered before anything else, that is a manager-level rule and a
  field on `Windows`, not a pane's.
- **Both windows cascade like a bag.** The reference centres its question plate
  on the screen; `reconcile_own_windows` has never been told the surface size
  and deliberately is not. This is the backlog entry every window kind already
  shares — nothing remembers where it was left — and the question plate is the
  one kind where the reference's own answer is *not* "wherever you last put it".

## The keyboard has an owner now — backlog

`Tab` used to enter war mode exactly once per launch. egui's
`egui_wants_keyboard_input` is literally "some widget has the focus", `Tab` is
what hands out that focus, so the first press entered war mode *and* focused a
button in the dev desk — and from the next frame egui claimed every key, war
mode, `Enter` and the arrows included. A self-arming trap: the key that broke
the keyboard was the key that could no longer be pressed.

`crates/client/app/src/keyboard.rs` is the layer that replaced the implicit
ladder of early `return`s inside `App::window_event`: `Owner` names who a
keystroke belongs to (speech line, pane field, world), `Edit` is the binding
table for a line being typed and `Hotkey` the world's own, all with tests that
need no window. egui is handed no `Tab` at all (`egui_may_see`) and may claim the
keyboard only while a text field inside it has the focus
(`Shell::holds_keyboard`) — of which this client has none, every box a player
types into being drawn by `chat.rs` or `panes.rs`.

The speech line completes staff commands as they are typed, from
`openshard-commands` — one table the world dispatches on *and* the client
offers, so a command that runs is a command that is offered and the two cannot
drift: `gm::run` matches `StaffCommand` exhaustively. `Tab` takes the highlight,
arrows move it, `Escape` puts the popup away before the line, and past the
command word the popup becomes the usage hint — and it offers only what this
character's authority lets it run, which the shard says once on world entry.
The channel is a button on the input line, with `Shift+Tab` beside it.

The five entries this backlog was left with are all closed, and what each of
them turned into is worth keeping, because the next thing here will be built on
one of them:

- [x] **The channel is a button, not a chord.** `chat::channel_button` draws it
      at the left end of the input line, on a plate, whether or not the line is
      open; a left click cycles it, ahead of the window layer and the world
      because the chat is drawn over both (`App::press_channel_button`). Its box
      comes out of two functions — `channel_button` and `channel_width` — that
      the frame and the pointer both call, which is `docs/parity.md`'s rule in
      the one place a player can feel it being broken. `Shift+Tab` stays, beside
      it rather than instead of it: a hand already typing should not have to
      leave the keyboard.
- [x] **The world's own hotkeys are a table.** `keyboard::Hotkey` names each of
      the nineteen and `Hotkey::key` says which key it is on; `Hotkey::of` is
      answered *out of* that one table rather than by a second `match`, so a
      forward and a backward reading cannot disagree. `event_loop.rs` is left
      with the doing. The arrows, `Tab` and `Escape` are deliberately not in it
      — two are held rather than pressed and one belongs to the window layer —
      and that is written down on the type.
- [x] **The completer offers only what the shard would run.**
      `openshard_protocol::access::AuthorityNotice` is this engine's own `0xBF`
      subcommand (`0xE001`, in a reserved range no client version and no
      ClassicUO uses), sent once on world entry, and it carries the account's
      `AccessLevel`. The client keeps it on the view and hands it to
      `StaffCommand::matching`, which offers a player nothing — the usage hint
      past the command word included. The threshold itself is
      `StaffCommand::AUTHORITY`, and `WorldState::staff_authority` compares
      against the same constant, so the gate and the completer cannot drift.
      `crates/e2e/shard/tests/staff_authority.rs` is both ends on one wire.
- [x] **The popup's highlight is a plate.** `gump::plate` is the rectangle
      primitive the pass had none of: a quad with no region at all — `du` and
      `dv` zero, which no packed sprite can be — whose `u` carries a `Shade` the
      shader paints through the hue's own ramp. No atlas entry, so it works in
      all three of this pass's uses (gump art, `fonts.mul`, a TrueType face),
      and the chat's furniture is drawn with it.
- [x] **The chat block is cut to the window.** `chat::room_above` answers how
      many rows fit between the input line and the top of the surface, and the
      popup is served first because it is the one a keystroke is moving; the
      journal takes what is left. `Offer::rows` takes that number as a hard cap
      and spends one of its rows on the "… n more" count rather than adding a row
      to it.

What this left behind:

- **The caret is still a glyph.** `gump::plate` now exists, so the `|` the chat
  draws could be a one-pixel bar — which is what a caret is. Not done with the
  rest because it is a *look* rather than a defect, and the width of a caret is a
  decision nobody has argued yet.
- **Nothing draws the bindings.** `Hotkey::key` is the half a key-bindings window
  reads, and there is no such window; the table is rebindable-*ready* and not
  rebindable. What is missing is a place to put it and a file to keep it in
  (`desk::Desk` is the obvious home, `client_ui.toml` the obvious file).
- **The authority notice is sent once and never again.** Right today — an
  account's level does not move while a character is in the world, and `.gm`
  moves the staff *mode* rather than the authority — but a shard that ever grows
  a `.setaccess` would have to send it again, and nothing would notice that it
  had not.
- **A plate is opaque.** The gump pass does no blending, so the chat's furniture
  covers the world under it rather than tinting it. That is the right first
  answer (a highlight has to be readable) and the wrong final one for a chat
  backdrop, which wants to be a wash. Blending is a pipeline decision for the
  whole pass, not a plate's.

Two defects were found on the way and fixed rather than filed, both in code the
work had to touch anyway:

- **The gump pass ran an untinted translucent picture through the hue ramp.**
  `SpriteQuad::hue` carries more than the wire hue — `with_opacity` writes a byte
  into bits 16-23 — and `gump.wgsl` asked whether the whole word was nonzero, so
  a picture with an opacity and no tint took the lookup at index zero, whose row
  is `-1`. An out-of-bounds `textureLoad` answers with zeros: the paperdoll's
  pending-equipment preview drew black. The shader now tests the index bits, and
  `crates/client/render/tests/gump.rs` pins it on a ramp built to fail if the
  lookup runs at all.
- **The chat's caret ignored `desk::ChatScale` on the `fonts.mul` path.**
  `text::gump_width` measures the font's own pixels and `scaled_gump_quads` draws
  them magnified, so an anchor placed at the unmagnified width put the caret a
  fraction of the way along the line it was measuring — at the default scale of
  two, halfway back through what had been typed.
