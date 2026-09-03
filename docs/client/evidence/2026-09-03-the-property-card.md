# The tooltip becomes a property card

The stack of strings at the cursor became one framed, measured, anchored card on
2026-09-03. What the wire half already did is
[`2026-08-15-tooltips.md`](2026-08-15-tooltips.md); this is the half above it —
and, at the end, the four things it deliberately did not do.

## What moved

The old drawing was eleven lines inside `render_passes::draw_gump_windows`: a
`Vec<String>` from `App::hover_tooltip`, one `GumpLabel` per line, offset
down-right from the cursor by a constant, drawn over whatever was behind them.
Nothing measured anything, so nothing could be framed and nothing could be
placed; a hover in the bottom-right corner simply ran off the window.

It is now three pieces with one seam each:

- **`openshard_client_render::tooltip`** — new, and pure. Faces, widths, word
  wrap, the head/body split, the frame, the icon's cell and the four anchors. It
  reads no world, sends no packet, touches no GPU, and every claim in this
  document that is about geometry is a test in it.
- **`app::tooltips`** — the request dedup it already owned, plus the two facts a
  card has between frames: when its content became drawable, and where the
  pointer was then.
- **`render_passes::draw_property_card`** — the three passes, in painter's
  order: plates, icon, text.

`App::hover_tooltip` still returns the answer, and is still the one place that
knows both the pick order and the wire. What it returns is
`tooltips::Presentation` rather than `Vec<String>`.

## Three decisions worth writing down

**The head is positional, and that is not a compromise waiting to be fixed by a
cleverer rule.** The card's head is the title plus the next two lines; the rest
is the body. Which lines *identify* an object and which *describe* it is a fact
the shard knows and `0xD6` does not carry — a property list is a flat sequence,
and the only thing this end can tell about a line is where in that sequence it
came. The alternative on offer is reading "Exceptional" out of resolved text,
which breaks on the first client language or custom shard that spells it
differently. It happens to be exactly right for everything the shard sends
today, because `WorldState::object_properties` writes the name, then the
quality, then the maker, then everything else. When a structured property model
lands, the split moves there and the constant goes.

**Placement is frozen to the pointer the card opened at.** Recomputing the
anchor from the live pointer every frame makes a card near a screen edge flip
between its four candidates as the pointer wanders a pixel. So `Tooltips` reads
the pointer once — on the frame the content first becomes drawable — and the
card is placed by that reading until its subject changes. The *anchor function*
still runs every frame, so a resized window or a newly docked panel re-clamps
the same card without moving it under the cursor.

**The clock starts when there is something to show, not when the pointer
arrived.** A card opens from its title to the whole list after 350 ms. Measured
from the arrival of the list rather than from the hover, an object whose `0xD6`
took a round trip would open the instant it landed, having spent its wait on the
network.

## The primitive the last record asked for already existed

`2026-08-15-tooltips.md` closed with "that needs a filled-rect primitive in the
gump pass, which does not exist yet". It did, by then: `gump::plate` was written
for the chat's channel button and the completer's highlight bar, and
`GumpArt::Solid` is a second one. Nothing new was added; the card's frame is a
plate under a plate — border-sized, then fill inset by a pixel — which is the
cheapest correct frame in a pass that does not blend.

## What is still not right about it

- **The fill is opaque.** The design asks for nearly black and *translucent*.
  The gump pass writes an alpha of one for every quad and has `blend: None` (see
  `gump.wgsl` and `GumpRenderer::new`), so translucency is not available to
  anything drawn through it — the shade is instead the darkest value that still
  reads as interface rather than as a hole cut in the world. A blended gump pass
  would make this the fill the design asked for and nothing else in the module
  would change.
- **A list taller than 60% of the surface is drawn too tall.** The card widens
  to `MAX_WIDTH` first, which takes the same text in fewer rows, and if it still
  does not fit it is drawn whole and over the cap. Dropping the overflow would
  silently tell a player an item has fewer properties than it has. The real
  answer is the pinned inspect pane the plan names, which is not built and is
  deliberately not smuggled into a transient card.
- **Quality tints nothing.** The design allows an item's quality to tint the
  title accent. The only thing that could decide it here is the resolved English
  text, which is what the data contract forbids reading; it becomes possible
  with structured properties.
- **A static still has no card**, which is unchanged and matches the reference:
  the map's own furniture has no serial, so there is nothing to ask about.
