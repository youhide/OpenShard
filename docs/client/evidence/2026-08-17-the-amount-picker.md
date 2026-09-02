# The amount picker is a gump, and it is ClassicUO's

The seventh window kind, every coordinate taken from `SplitMenuGump.cs` —
including the arithmetic that is not quite right and is copied anyway — and the
three things it deliberately does not copy.


A Shift-drag out of a pile asks how much to take. That question used to be an
`egui::Window` with a `DragValue` and two buttons on it, anchored to the middle
of the screen; it is the reference client's own `SplitMenuGump` now —
`client/render/src/split.rs` for the layout, `client/app/src/panes/split.rs` for
the gestures — and it is the **seventh window kind**, with a
`WindowSubject::Split`, a `Drawn::Split` and a pane like every other.

Every coordinate is `SplitMenuGump.cs`'s: the frame is `0x085C` (164×74), the
knob is `0x0845` (15×15) on a 105-pixel bar at `(29, 16)` whose trough is
painted into the frame, the button is the three faces `0x085D`/`0x085E`/`0x085F`
at `(102, 37)`, and the number is written at `(29, 42)` in font 1, hue `0x0386`.
The bar's arithmetic is `HSliderBar`'s, including the part that is not quite
right — the value is measured from the trough's left edge without subtracting
half the knob, so the knob's *corner* follows the pointer. That is copied rather
than corrected, because it is what the gesture feels like in the client people
have played.

Three things it does not copy:

- **One number, not a number and a string.** The reference keeps a slider and a
  text box in step across forty lines of `UpdateText`. There is one `u16` here
  and two ways of writing to it: the bar scales it and a typed digit shifts it up
  a decimal place. The state a player is *not* looking at is the one that ends up
  in the packet, so there is only one.
- **The bar's top is the pile less one.** `ItemPress::dragged` has always said a
  split that takes everything is a lift with extra steps, and that rule is older
  than this window. The reference lets the bar reach the whole pile; here it
  reaches `amount - 1`, and the picker opens *at* that top so the button alone
  still means "as much as this gesture allows".
- **It is not dragged out from under the pointer on open.** The reference calls
  `AttemptDragControl` so the fresh gump follows the mouse that is still held
  down. Here it is placed at `pointer - (80, 40)` — the reference's own offset —
  and stays there.

**Escape and Enter stopped being one key.** `panes::Key` had three arms, because
a `{ textentry }` puts itself down either way. A modal is the window where they
are *opposite* answers, so there is a `Key::Cancel` now: Enter sends the number,
Escape dismisses the press the prompt was standing over. The dialog's field
answers both the same way it always did, in one arm.

The answer still travels the way it did — `Windows::prompt` names the presser and
`Input::Answered` is delivered to it by identity — but it no longer arrives a
frame late through `shell::Request`. It is an ordinary `Effect::Answered` on the
frame the button was pressed, which also means the gesture works in a build with
no HUD at all: the old effect was dropped on the floor when `App::shell` was
`None`.

