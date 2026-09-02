# The outline: a sprite with an edge drawn round it

The client already says "the cursor is on this" by hue: whatever `items::pick`
answers is drawn in `items::HIGHLIGHT_HUE`, ClassicUO's own
`HIGHLIGHT_CURRENT_OBJECT_HUE`, replacing the art's colour the way a `hues.mul`
ramp does. That is the reference's whole vocabulary for it and it works.

This plan is the **second** way of saying it, wanted alongside the first rather
than instead of it: an **outline** — a hard one-pixel edge round the sprite's
silhouette first, and then the same silhouette blurred into a glow behind it.
Both are built. The two must compose: an item can be hued *and* outlined at once,
which is the first thing this plan decides, and which of them a frame actually
draws is D7's switch.

Written against `crates/client/render/`: `items.rs`, `sprite.rs`, `atlas.rs`,
`statics.wgsl`, `renderer.rs`, `blit.rs`, and `crates/client/app/src/lib.rs`'s
`draw`, where the frame is staged.

## The reference is not UO

ClassicUO does not draw outlines. Its whole per-sprite vocabulary is
`ClassicUO.Renderer/ShaderHueTranslator.cs` — `SHADER_NONE`, `SHADER_HUED`,
`SHADER_PARTIAL_HUED`, `SHADER_SPECTRAL`, `SHADER_SHADOW`, `SHADER_LIGHTS` — a
`byte` picked per draw and carried in the third component of the hue vector.

So the reference is worth exactly two things and nothing else:

- **The mode travels beside the hue, not inside it.** A `byte` of its own, not a
  reserved value of the hue. Copy that.
- **`SHADER_SHADOW`** is the silhouette draw — a sprite rendered as a flat
  shape rather than its own colours. Whatever draws the silhouette here is doing
  what that mode does.

There is a reference for the *effect*, just not in UO: **Fallout 1 and 2 outline
objects**, and they do it by per-pixel edge detection at blit time. The sprites
are 8-bit palettized with index 0 transparent, so the shape is already a bitmask;
the engine walks the object's own buffer and, where a transparent pixel is next
to an opaque one, writes the outline colour's palette index. No polygon, no
second asset, no preprocessing — one extra pass over a rectangle grown by a
pixel, with the colour a per-object property (hostile, party member, the
highlight-items hotkey).

That is D5 step 2 with the GPU taken away. The pipeline below is not a different
idea from theirs; what differs is *where* it runs, and that is decided by two
things Fallout did not have — a zoom, and a packed atlas. See D3 and D4.

Everything else below is ours, and the standard techniques are named in
[Techniques](#techniques-what-to-search-for) rather than invented here.

## The decisions

Numbered so one can be argued with alone. None of them is implemented yet.

### D1 — the outline is a pass, not a flag on the static pass

The tempting version is a bit on `SpriteQuad` and a branch in `statics.wgsl`.
It does not work, for a reason that is geometry rather than taste: **an outline
lives outside the sprite's own quad.** The rectangle is the art's exact size
(`Sprite::width`/`height`, see `statics::stand_on`), so an edge drawn round the
silhouette has nowhere to land.

So: a **pass of its own**, drawing only the highlighted sprites. That is also
the only shape glow can be added to without being rewritten (D5).

**Built, and where it runs is now fixed:** the silhouette pass is
`SpriteRenderer::render_mask`, between the mobile pass and the text pass; the
ring is `outline::Outline::render`, after the blit.

### D1a — the ring does not shine through what is in front of it

This was written as a consequence of D1 ("with the depth test off") and it is
not one, it is a choice, so it is numbered separately.

The silhouette pass **tests the world's depth buffer and does not write it**. So
the mask holds the id of whoever is *visible*, and a barrel behind a shopfront is
ringed only where the barrel can be seen — the ring is occluded exactly as the
picture is. Fallout does the same thing for the same reason, and it falls out for
free rather than costing a pass: the ordering was settled by the passes that drew
the picture, and the mask is the record of that decision.

Depth *write* is off because the ordering must not be settled twice; the text
pass draws at the near plane afterwards and would otherwise punch the mask
through, which is why the mask pass runs before it.

### D2 — no mode field: the list is the numbering

**Withdrawn.** The plan wanted one more `u32` on `SpriteQuad` so a sprite could
say "outline me". It is not needed, and adding it would have touched every
construction site in the crate for a fact about one sprite in a frame.

The silhouette pass takes **only the sprites to be outlined**, as their own list
(`items::outlined`). A quad's identity is then its position in that list, read in
the shader as `instance_index + 1` — zero staying free for "nothing here". No
field, no bits stolen from the hue, and hue and outline compose because they are
two passes over different pixels rather than two flags on one instance.

The ceiling is `outline::MAX_OUTLINED` — 255, the mask being one byte. Past it
the tail is dropped rather than wrapped: an id that wrapped onto another
object's would ring the two as one, silently.

**Amended for creatures: the numbering is per *group*, not per quad.** A ring is
not a sprite. An item is one quad and one ring, but a mobile is a body plus every
layer it is wearing — several quads that must come out under one id, or the ring
pass finds a boundary between the tunic and the arm inside it and draws an edge
along every seam of the clothing, which is a creature drawn as a diagram of what
it has on. So `SpriteRenderer::render_mask` takes `&[&[SpriteQuad]]`, one slice
per ring, and the id rides in a small per-instance buffer of its own rather than
in `instance_index`. Still no field on `SpriteQuad`: that struct is the picture
passes' layout and has no business carrying a highlight's identity. An item is
written `&[&[quad]]` and costs nothing.

### D3 — the atlas has no padding, and it turns out not to matter

**Corrected.** The claim was that *every* outline technique samples neighbouring
texels, so a highlighted barrel would be outlined in whatever art `Shelf::take`
packed beside it. That is true of two techniques and false of the one this plan
chose.

In D5's pipeline the atlas is sampled with the picture pass's own UVs, strictly
inside the sprite's `Region`. The growing happens **in the mask**, where a
neighbour is a neighbouring pixel of the screen and not a neighbouring sprite in
the atlas. So the packing needs no border and no clamp, and the adjacent-sprite
test would be pinning a property nothing depends on.

It comes back the moment either of these is wanted, and both are listed under
[Techniques](#techniques-what-to-search-for):

- **the offset-draw silhouette** (the sprite drawn 4 or 8 times, shifted, as a
  flat colour), which samples the atlas *at an offset* by construction;
- **baking the edge into the art at load time**, which dilates in texel space.

If either is built, the fix is a one-texel transparent border in `Shelf::take`
(`region_at` and `Packed::origin` shift with it) or a clamp to the `Region` in
the shader — and then the adjacent-sprite test this section originally asked for.

### D4 — thickness is one *virtual* pixel, not one screen pixel

**Reversed, deliberately.** The plan argued that a "one pixel" outline drawn in
the world image is four screen pixels at 4x, "which is not what a one-pixel edge
means to anybody looking at it". On reflection the opposite is true for pixel
art: a one-*screen*-pixel hairline round a sprite drawn at 4x is finer than any
edge in the picture it is tracing, and reads as a rendering artefact rather than
as a highlight.

So the mask is the world image's size and the ring is grown in its texels, which
are virtual pixels; the blit's nearest sampler magnifies the ring together with
the art, blockily and in step with it. This is also what makes the pipeline
cheap: no second resolution, no rescaling of a radius, and the mask can share the
world's depth buffer — which it must, since a depth attachment has to match its
colour attachment's size.

The composite still runs **after** the blit, for a different reason than the plan
gave: not resolution but lighting. A ring drawn into the world image would be
multiplied by the night, and a highlight that dims exactly when the picture is
hardest to read is a highlight that stops working.

What this costs is at *minification*, and it is worse than "thinner": the
composite reads the mask with `textureLoad` at the **surface's** resolution, so
below 1:1 the mask is *point-sampled*. At `1/2` only every other texel is ever
looked at, and a one-texel ring does not thin — it loses whichever of its sides
falls on the parity nothing samples. `Ring::for_zoom` widens it to
`denominator / numerator`, two texels at every rung below 1:1, which puts at
least one ring texel in every screen pixel's footprint;
`a_minified_ring_keeps_every_side` pins it, and its companion half pins that the
naive ring really does come back with edges missing.

### D5 — one mask, two effects

The pipeline that makes the pixel outline and the glow the *same* work:

1. **Silhouette into a mask.** A target the size of the world image; every
   sprite to be outlined drawn into it in its own id (D2). This is
   `SHADER_SHADOW`'s draw.
2. **Grow it.** A neighbourhood test on the mask: a fragment is a ring texel of
   object *N* when some neighbour is *N* and the fragment itself is not.
3. **Composite.** That ring, in the ring's colour, alpha-blended over the
   surface.

Steps 2 and 3 are **one pass**, not two: the grow is nine texel loads in the
composite's own fragment shader, so there is no second target and no second
draw. A separate dilation target buys nothing until the blur arrives.

**Built, and the blur has arrived without changing that.** The glow is a chain
*before* the composite rather than a second pass after it — `glow.wgsl` seeds
coverage out of the id mask at half resolution and three Kawase iterations
spread it — and the composite reads the result alongside the mask it was already
reading. What made one pass enough for both halves is the blend state:
**premultiplied alpha**, where `dst = src.rgb + dst * (1 - src.a)`. A fragment
with a colour and no alpha is then pure addition, which is the glow; one with
both is the ordinary blend, which is the ring. Straight alpha would have needed a
second pass with a second blend state, and then a target to read it from.

**The mask holds an id, not a coverage bit**, and that one choice is what keeps
two rings apart. With coverage the second half of the rule reads "and the
fragment is empty", so where two outlined sprites touch there is no ring between
them and the pair comes out ringed as one blob. With ids the boundary between
two outlined objects is a ring on both sides of itself. It costs the same byte
and the same nine taps.

The glow is step 2 with a blur in it and step 3 with additive blending. Nothing
in 1 or 3 changes, which is the whole reason for doing it in this order: **ship
the pixel outline, and the glow is one shader later.**

### D6 — what decides *which* sprites are outlined stays where it is

`items::pick` already answers "which item", `App::world_owns_pointer` already
answers "may the world read the cursor", and `draw` already asks both once per
frame. The outline consumes that answer and adds nothing: no second pick, no
per-item flag, no highlight state kept between frames.

`items::outlined` and `items::collect` build their quads through one
`items::quad_of`, so the silhouette lands on the picture rather than beside it.
Two copies of that arithmetic would be a ring half a pixel off its sprite, and
nothing in either copy would look wrong.

**Creatures are picked and ringed too, through the same shape.** `mobiles::pick`
is `items::pick` against animation frames — the picture and not the tile, an
opaque texel and not a box, the topmost order winning — with two additions of its
own: a worn layer counts as its *wearer* (a cursor on a hat is a cursor on the
creature), and a mirrored facing has its flip undone before the atlas is asked,
since the atlas holds one picture for both and half the creatures on screen face
a mirrored way. `mobiles::outlined` and `mobiles::collect` build their quads
through one `mobiles::push_quads`, for the reason the items' pair share
`quad_of`, and the silhouette goes in as **one group** — see D2's amendment.

Creatures are asked *first* and win the frame: a mobile is sorted above whatever
is lying on its tile, and a player pointing at a shopkeeper standing on a rug
means the shopkeeper. One highlight a frame either way, so an item under a
creature is dropped rather than lit as well. A double-click there uses nothing:
using the barrel *behind* the shopkeeper is the one answer that is certainly
wrong, and what a mobile's own double-click asks for — a paperdoll — waits on
there being a paperdoll to show.

The map's statics stay unpickable, for the reason the picking has: they are not
entities and have no serial to name. That is listed in `docs/client.md`'s M5
backlog and changes nothing here — the pass takes a list of quads and does not
care where they came from.

### D7 — the tile marker and the item highlight are one answer, not two

A tile marker says *the ground a click would walk to*; an item highlight says
*the thing a click would use*. Drawn together over one cursor they are the client
answering the same question twice, and on a littered street the diamond under a
ringed barrel is the wrong half being read.

So one of them wins per frame, and `shell::HighlightTarget` is who: `Auto` — the
item if the pick found one, the tile otherwise — with `Items` and `Tiles` to pin
it. `Hud::hover` stays the *fact* either way, because the panel reads it and the
terrain overlay routes to it; what the mode decides is `Hud::hover_lit`, which is
only whether the marker is drawn.

**The pick moved to the top of the frame for this.** The HUD is laid out before
the world passes run, so a tile marker that decides from the *previous* frame's
pick flickers along every item's edge. `items::pick` is therefore asked once in
the frame's snapshot, beside the camera and the cutaway, and its answer is handed
to all three readers — the hue, the silhouette, and the marker. What that gives
up is one frame of freshness in the *atlas*: the pick now runs before the frame
grows it, so an item that came on screen this very frame has no rectangle to be
pointed at until the next one. A flicker along an edge is seen every time; that
is not.

The style is the second axis and the switch this plan asked for:
`shell::HighlightStyle` is `Hue`, `Outline` or `Both`, and it is `None` on the
pick handed to each pass rather than a mode either pass has to branch on. The
default is `Outline` — the ring and its glow *add* a statement, where the hue
ramp replaces the picture with one.

### D8 — a *held* selection is a wash, and it needs both records

A ring says which thing the cursor is on. What a person editing or debugging a
map wants is a different statement: **which wall, and which tile it stands on**,
held while they read the numbers off the panel. Those are two claims about two
different surfaces, and a ring can make neither of them — an edge round a wall
says nothing about the ground, and the ground has no silhouette of its own to
ring.

So the third form is a **wash**: `select::Select`, one full-screen pass after the
blit, drawn from a click's answer rather than from the cursor's. Cyan at 0.30
over the picked static and 0.18 over its ground, premultiplied so the art shows
through — the same cyan the Tile panel's held-tile diamond already uses, because
a client that said "selected" in two colours would be saying two things.

The pass reads **two records, and it needs both**:

- the **mask**, for what the selected thing is. `statics::selected` hands its
  quad to the same `render_mask` the ring uses, so what is washed is what is
  *visible* of it;
- the **place attachment**, for what the ground under it is — every texel already
  names the tile the pixel came from, so "the tile it stands on" is a comparison
  and costs no second mask.

Neither replaces the other, and the second is the interesting half. A wall's
pixels *cannot* be told from another wall's on the same tile out of the
attachment alone: both write the same tile, the same stance and the same range of
heights. A wash keyed on the tile would light the wall beside the one that was
picked, which is the mutation `tests/select.rs` is built around.

The ground of a tile is **land or a static lying flat in it**, not land alone.
Indoors the land is under a wooden floor and never drawn, so the land-only rule
comes out unshaded in exactly the building being pointed at.

**Its own mask texture, not the ring's.** The ring pass draws an edge round every
id it finds, so a selection sharing that mask would come out ringed *and* washed
— and the two would then be one statement made twice, which is D7's whole
complaint. Two masks, two passes, and the wash goes under the ring: the held
answer is context and the live one has to stay readable crossing it.

## Techniques: what to search for

- **Pixel outline, one pass:** *sprite outline shader alpha dilation*, an 8-tap
  neighbourhood max over alpha. This is D5 step 2 at its simplest.
- **Silhouette by repeated offset draws** (draw the sprite 4 or 8 times, offset
  by a pixel, as a flat colour, then the sprite on top). The cheapest thing that
  works and the one that needs no mask target — but it samples the atlas at an
  offset, so D3 applies to it exactly as much.
- **Thick or uniform outlines:** *jump flood algorithm outline*, *signed
  distance field outline*. Worth it only once a thickness above two pixels or a
  smooth falloff is wanted; the JFA output is also a free distance field for the
  glow.
- **Glow:** *kawase blur*, *dual filter blur*, separable gaussian. Two passes
  over the mask, then additive composite.
- **Stencil-buffer outline** is the other classic answer and is listed for
  completeness: it draws the ring without a mask target, at the cost of a
  stencil attachment on the world pass and a second draw of every highlighted
  sprite.

## Steps

- [x] ~~D2: the mode field on `SpriteQuad`~~ — withdrawn, see D2.
- [x] ~~D3: pick padding or clamping~~ — not needed for this pipeline, see D3.
- [x] D5 step 1: the mask target (`outline::mask_texture`) and the silhouette
      draw (`SpriteRenderer::render_mask`, `silhouette.wgsl`), sized like
      `Screen.world` and resized with it.
- [x] D5 steps 2–3: the neighbourhood test and the composite, in one pass
      (`outline::Outline`, `outline.wgsl`).
- [x] Frame tests. `a_ring_is_drawn_around_a_silhouette_and_not_over_it` pins
      the ring's shape both ways — the border is ringed and the sprite is not —
      and `two_touching_silhouettes_are_ringed_separately` is what an id mask
      buys over a coverage one.
- [x] The switch: hue highlight, outline, or both — `shell::HighlightStyle` on
      the HUD's request, and `shell::HighlightTarget` beside it for the second
      axis D7 found: item or tile. Both are pickers in the Tile window.
- [x] The glow: `Ring::glow` carries a reach and a colour, `glow.wgsl` seeds
      coverage at half resolution and Kawase-spreads it, and the composite adds
      it under the ring. Steps 1 and 3 did not change — the blend state did, see
      D5.
- [ ] Nothing drives the glow from the *shard*: its colour is one uniform for
      the frame, so "hostile red, party blue" — Fallout's own use of the effect
      — needs the mask's id looked up in a small palette. The id is already
      there; what is missing is a table beside `items::outlined` saying what each
      entry means.

## Backlog, in advance

- **Done: mobiles are outlined, through the mobile pass's own `render_mask`.**
  It came out as the second `SpriteRenderer` drawing into the same mask rather
  than a second texture on one pipeline — each renderer owns its atlas, its
  sampler and its uniform block, and sharing them is what guarantees the
  silhouette lands where the picture did. The cost is that **both passes clear
  the mask**, so the mobile one is skipped when nothing is ringed or it would
  erase the items' answer. The day two things are lit at once — a target
  cursor's victim and the thing under the mouse — that clear has to move out of
  `render_mask` into a caller that owns the frame's mask.
- **The pick builds the crowd's drawn list twice a frame.** Once at the top for
  `mobiles::pick` and once below the atlas growth for the passes
  (`App::drawn_now`). They agree because both go through
  `App::advance_to_clocks` over `App::drawn_mobiles`' order, and the second is
  the authority for what is drawn; what is duplicated is a handful of map
  lookups per creature. It becomes wrong the day the two orders can differ —
  a filtered list, a sort — and the fix is to thread the first list through
  rather than to rebuild it.
- **Done: the chain has three links, and the mode no longer hides one.**
  `on_mobile` → `on_item` → `on_static` are asked once a frame, each only where
  the one before found nothing, and `HighlightTarget` is applied *after* them to
  what is lit rather than to whether the pick happens at all. Two things were
  wrong before it: pointing at a house front left the tile marker on the ground
  behind the wall (the marker's rule knew about items and creatures and not about
  the map), and under `Tiles` no pick ran at all — so a player who had pinned the
  highlight to tiles could not select a wall, with nothing on screen to say why.
  The click now reads the *last drawn frame's* answer rather than picking again:
  a click arrives between frames, so the picture it is a click on is the one
  already on the screen, and a second pick would ask a camera that has moved.
- **Done: the selection names one tile.** `App::selected_tile` is the selected
  static's own tile when there is one, and only a click on bare ground
  unprojects the cursor. The two arithmetics answer differently on purpose — a
  wall's picture stands up the screen out of its cell, so the ground under the
  cursor is two cells behind the wall — and showing both at once was the client
  saying "this one" about two places.
  `statics::tests::a_wall_s_tile_is_not_the_tile_under_the_cursor` pins the gap
  in the crate that owns both.
- **Half done: the static-shaped sibling of `items::pick` exists.**
  `statics::pick` walks the visible cells, tests the same opaque texel and
  answers a `PickedStatic` — where it stands and what it is, since the map's
  furniture has no serial to name. It is what D8's wash is picked with, and the
  placement it draws from is now one function (`statics::place`/`quad_of`), used
  by the map's statics, the server's items, the silhouettes and the picks alike.
  What is *not* done is the hover: `HighlightTarget::Auto` still falls back to
  the tile marker when the item pick finds nothing, so pointing at a house front
  still says "you would walk here" — **corrected above**: the marker now goes out
  over a wall, and what is still open is only whether a hovered wall should say
  something *positive* (a ring, a fainter wash) rather than merely silencing the
  ground. That is a thing to look at rather than to argue.
- **The selection is picked against the atlas as it stands at the click.** The
  same caveat `items::pick` carries: a wall whose art this frame has not packed
  yet cannot be clicked on, and is selectable a frame later. It is one frame at a
  camera boundary and has never been seen.
- **Nothing but a click on bare ground clears the wash.** There is no Escape and
  no button; it was not asked for, and a selection that survives until the next
  click is the point. Worth a line on the Tile tab the day somebody has to be
  told how to put it out.
- **A selected static that stops being drawn leaves the wash to the ground
  alone.** The mask comes out empty — the cutaway hid it, or the camera walked
  away — and the pass is skipped entirely (it is gated on the quad list), so the
  ground stops being washed too. That is the honest picture and it is also
  indistinguishable from "nothing is selected"; the panel is what tells the two
  apart, which is why the held static is named there.
- **The mask and the glow's pair are allocated for every frame, whether anything
  is outlined or not.** One byte per world pixel and two RGBA quarter-images,
  cleared each frame by passes that usually draw nothing. Trivial next to the
  world image; worth remembering if the mask grows a channel or the glow grows a
  resolution.
- **Three Kawase iterations are a constant.** `Glow::radius` scales the offsets,
  so a large reach is spread by the same three passes and the falloff coarsens
  as it widens. Nothing above a dozen virtual pixels has been asked for; past
  that the iteration count is what has to grow, and `step_offsets` is where.
- **The glow is not lit and not occluded past what the mask says.** It is added
  after the blit like the ring, so it is un-dimmed at night by design (D4) — but
  it also spills over whatever is in front of the sprite's *neighbourhood*, since
  only the silhouette itself was depth-tested. A barrel behind a shopfront glows
  a little onto the shopfront. Fallout has the same artefact and it reads as
  light rather than as a bug, which is why it is a note and not a step.
- **The click still picks against a camera it reads back from `self.control`**
  (`App::use_under_cursor`), while the highlight picks against the frame's own.
  See `docs/client.md`'s M5 backlog: the outline makes this more visible, since
  what is lit and what is used would then differ by a whole visible ring.
