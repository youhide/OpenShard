# The dev HUD, the desk file, and the third scale

The five tabs, what is written to `client_ui.toml` when the client closes, and
the rule that catches every knob wired to nothing: the `Desk` on `App` is the
file *as it was loaded*, and anything live is read through a getter on the
shell.

Status and what is left are [`README.md`](README.md).

## The dev HUD, and what it remembers

The five floating panels — Camera, Rig, Frames, World, Tile — are **one window
with five tabs**, and what the HUD looked like is written to `client_ui.toml`
beside `openshard.toml` when the client closes. See
`crates/client/app/src/desk.rs`.

This is worth writing down because the *absence* looked like a bug and was not
one: nothing was failing to save, there was nobody to save. `eframe` is the crate
that persists `egui::Memory` and the window's geometry, and this client is bare
`egui` on `winit` and `wgpu` — deliberately, because it owns its own event loop
and its own surface. So the saving is ours, and it is a named struct rather than
a serialized `egui::Memory`: the latter carries every widget id egui happened to
allocate, in a format whose meaning is egui's version, and an upgrade restores
nonsense or nothing.

What is remembered: the tab in front, whether the window is open, where it sits,
egui's `zoom_factor`, and the operating system's window — outer position, inner
size, maximized. A saved frame is only restored if its top-left corner still
lands inside some monitor: a laptop undocked since the last run has a saved frame
that opens the window offscreen, which looks exactly like a client that failed to
start.

**How big this client's own windows draw is a third scale, and its own knob.**
`desk::WindowScale` (`window_scale` in the file, the dev window's Windows tab) is
an upscale on the window art's own pixels, applied on top of the two densities
above — a bag, a doll, a shop, a sheet, the amount picker, all of them together.
It exists because the reference client has no display scaling at all: its windows
are sized in raw art pixels, so on a modern screen a container is a postage stamp
however good the monitor is. One number for every window and not one per kind,
because windows drop items into each other and an icon that changed size in the
player's hand on the way between two of them would be the preview disagreeing
with both. It reaches the surface through `gump::place` and the pointer through
`windows::OwnWindow::local_cursor`, which are the same transform in the two
directions — see `docs/window_components.md`'s window-local coordinates entry.
The shard's hover tooltip and the HUD chat box are outside it: the first is drawn
over the world as well as over a window, and the second has `desk::ChatScale`.

**It is fractional, and that is a decision with a picture attached.** A whole
number draws every art pixel as the same square block. A fraction cannot: this
pass samples with `Nearest`, so 1.5 repeats every other row of texels and leaves
a window's border two pixels thick along part of an edge and one along the rest,
and a background's pieces meet with a seam that opens at some fractions and
closes at others — `gump::Frame::scale`'s warning one rung up, arrived at
deliberately rather than by accident. It is offered anyway because 1.5 is the
size a great many screens actually want, and "this size or twice this size" is
not a scale. Nothing snaps the result back onto whole pixels afterwards:
rounding each *picture* onto the grid while its size stays fractional moves a
window's own pieces relative to each other, which is a button drifting off its
plate — worse than the seam it would tidy. The window's corner is whole, so the
error stays inside one window instead of being reintroduced per picture.

The scale is *egui's* zoom and not the monitor's `scale_factor`, which stays the
platform's business — a file that pinned it would fight the compositor on the
next screen. Ctrl+`+` / Ctrl+`-` / Ctrl+`0` are egui's own shortcuts
(`Options::zoom_with_keyboard`); the status strip shows the number because a
client that reopened at yesterday's zoom and does not say so reads as one that is
rendering at the wrong size.

**A knob read from `App::desk` does nothing, and looks like a knob that is
wired to nothing at all.** The `Desk` on `App` is the file *as it was loaded*;
the one the dev window's widgets move is `Shell::desk`, and the two meet only at
exit, where `event_loop.rs` reads the shell's copy back to save it. So a slider
whose value is read from `self.desk` moves a number nobody draws from and takes
effect on the next launch — which is what `WindowScale` did on the frame it was
first written, and what `Shell::tuning`'s doc had already said about the
lighting. Anything live is read through a getter on the shell
(`Shell::tuning`, `Shell::chat`, `Shell::window_scale`), with the app's copy as
the fallback for a run that has no shell at all.

