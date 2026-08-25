# Text sizes: a real font size, not a scale

Every piece of text this client draws through a TrueType face gets a **size in
pixels** — a real, fractional one, rasterized at that size — rather than a
multiplier applied to something else. This document is the plan that gets it
there, and the record of what was decided along the way.

## What was wrong with a scale

Three different things called themselves a "scale" and only one of them had to:

- **`desk::TtfScale`** — a multiplier (×0.5 … ×3.0) on a base of 16 pixels. The
  rasterization underneath it was always honest (`fontdue` takes a fractional
  pixel height and shades the outline analytically), so this was a real size
  wearing a multiplier's clothes: a player who wants 13-pixel text has to work
  out that this is ×0.8125.
- **`desk::ChatScale`** — an integer upscale of *finished quads*, because
  `fonts.mul` is a bitmap face with no continuous size to ask for. This one is
  a scale because it has to be, and it stays.
- **`desk::WindowScale`** — how much bigger than its own art a window draws.
  A window's caption is drawn through it too, so at 2× the glyphs are stretched
  to twice their rasterized size instead of being *rasterized twice as large*.

And one hard limit underneath all three: a `TtfAtlas` baked **one** pixel
height for the whole client, so there was no way to say "the count on a pile is
smaller than a spoken line" — the atlas had one size and every caller shared
it. Changing that size threw the whole atlas away and re-packed it
(`Screen::sync_ttf_scale`), which is why the size was a slider you nudged
rather than a number per kind of text.

## The decisions

**D1 — A size is a value, not a factor.** `TextSize(f32)`, in pixels,
fractional, clamped to a sane range once on the way in (the way `Zoom::new` and
every other desk knob already clamps). It is what reaches the rasterizer, and
nothing multiplies it on the way to a quad.

**D1a — The size is an argument to the draw, not a field on every label.**
`collect_ttf`, `collect_screen_ttf`, `collect_gump_ttf` and `gump_width_ttf`
take a `TextSize`; `Label`, `GumpLabel` and `ScreenLabel` are untouched. A call
draws one role, and a caller with two roles in one list makes two calls — which
is what the overhead list does, since a pile's count and a spoken line hang off
the same walk. The alternative was a fifth field on three label types and the
forty-four places that build them, for a value every one of them would have
copied from the same place.

**D2 — One atlas, keyed by `(char, TextSize)`.** Not an atlas per size: one
texture, one bind group, one pass, one upload path — everything that already
exists keeps working, and a second size costs a few more shelves rather than a
second `GumpRenderer`. `TtfAtlas::glyph(ch, size)` and `add(font, size, chars)`.

A consequence worth stating: **changing a size no longer throws the atlas
away.** `Screen::sync_ttf_scale` and `build_ttf`'s re-bake go away with it; the
new size's glyphs simply pack in beside the old ones the first frame they are
asked for. What replaces the re-bake is a reset on `AtlasError::Full`, since a
slider dragged across thirty sizes packs thirty alphabets: the atlas empties
itself, marks its whole texture dirty, and the frame after that re-packs
whatever is actually on screen.

**D3 — Sizes are per role, and a role is what the text *is*.** Four of them,
each a real pixel size in `client_ui.toml`:

| role | what it is | default |
|---|---|---|
| `speech` | a line over a head, and the HUD chat box | 16.0 |
| `window` | captions inside this client's own windows | 14.0 |
| `tooltip` | the shard's hover text | 14.0 |
| `stack_count` | the digits written on a pile | 11.0 |

Not one size with per-role offsets: an offset is a scale again, and the whole
point is that a person can say "eleven pixels" and get eleven pixels.

**D4 — Density multiplies the size, never the quad.** The real size handed to
the atlas is `role × window.scale_factor()`, plus `× WindowScale` for text
drawn inside a window. Both are *inputs to the rasterizer*: a caption in a 2×
window is rasterized at twice the pixels, not stretched to twice the size. The
glyph quads themselves are then placed 1:1 — positions still move with the
window's magnification, sizes do not.

**D5 — `ChatScale` stays an integer, and stays `fonts.mul`-only.** A bitmap
face has no other honest answer. It disappears from the picture entirely when a
TrueType face is loaded, which is already true today.

**D6 — With a face loaded, windows and tooltips draw through it too.** They are
`fonts.mul` today whatever `--ttf-font` says, which is the last place a size
cannot be asked for. `fonts.mul` remains the whole picture when no face is
loaded.

**D7 — A line height is measured ink/metrics, never its nominal request.** A
bitmap face uses the visible height of its actual `M` glyph plus two pixels of
air; `fonts.mul` cell padding does not become leading. A TrueType face uses
its `ascent`, `descent` and `line_gap` from `fontdue`, retained in the atlas at
each raster size. The same result feeds draw and hit-test paths, so selecting a
different F1 face cannot make the chat control's box drift from its glyphs.

## Phases

**All four are built** (2026-08-20). What each was:

- **P1 — the atlas and the size.** `TextSize`; `TtfAtlas` keyed by
  `(char, size)`; `add`/`glyph`/`collect_ttf`/`collect_screen_ttf`/
  `collect_gump_ttf`/`gump_width_ttf` take one. Existing callers pass the one
  size they use today, so nothing changes on screen. `sync_ttf_scale` and the
  re-bake go; `TtfAtlas::reset` arrives with the `Full` policy above.
- **P2 — the knobs.** `desk::FontSizes` with D3's four fields; `TtfScale`
  retires. The Chat tab's slider becomes pixels. An old `ttf_scale` in a
  `client_ui.toml` is ignored rather than migrated — it is a multiplier of a
  base this file no longer has.
- **P3 — windows and tooltips through the face.** D6, and D4's `WindowScale`
  half with it, since a window's caption is where the two meet.
- **P4 — the pile's count in its own size.** `stack_count`, which is the
  request this plan came out of: a number over a pile has to be smaller than a
  line of speech, and until P1 there was no way to say so.

## What it came out as

One thing worth recording, because it is not what the phases predicted: the
re-bake is *gone* rather than made cheaper. `Screen::sync_ttf_scale` and
`build_ttf`'s pixel height went with it — an atlas keyed by `(char, size)` has
nothing to re-bake, so moving a size slider now packs the glyphs actually on
screen at the new size and leaves the old ones where they are. The one thing
that empties it is `TtfAtlas::add_or_reset` meeting a full texture, which a
long drag across sizes can do; the cost is one frame of text drawn from stale
regions, stated on that method.

The other is that `window_text` in `render_passes.rs` is now the single place
any window caption, tooltip or cursor count is drawn, whichever face is
running — which is what made D4 expressible at all: the bitmap path magnifies
the finished quad and the TrueType path folds the same magnification into the
size, and those two sentences are three lines apart in one function rather
than in two subsystems that would drift.

## Backlog

- **A size per role, and no size per *window*.** A player who wants a bigger
  shop but not a bigger paperdoll has `WindowScale`, which moves both. Nothing
  here changes that, and nothing here makes it harder.
- **`fonts.mul` has no role table.** The bitmap path still picks its face by
  hand at each call site (`Font(1)` in four places). Worth folding into the
  same role table as the sizes, so "which face, at what size" is one answer per
  role rather than two answers in two places.
- **Cross-family visual calibration.** F1 exposes an exact raster size for a
  configured TrueType face and an exact `fonts.mul` face for bitmap text. Line
  baselines and rows are now measured, but an operator who wants one *particular*
  TTF family's capitals to match one bitmap face's capitals still chooses that
  visual size deliberately; there is no universal ratio between unrelated
  typefaces.
- **A window that is magnified re-rasterizes its captions.** `WindowScale` is
  folded into the size (D4), so dragging that slider asks for a new size per
  step — which is correct and is also the drag most likely to fill the atlas
  and trip `add_or_reset`. Worth a coarser quantisation of the *product* if it
  ever shows.
