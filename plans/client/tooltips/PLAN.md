# The tooltip becomes a property card

> **Built on 2026-09-03.** Everything below is what shipped, except where
> [`docs/client/evidence/2026-09-03-the-property-card.md`](../../../docs/client/evidence/2026-09-03-the-property-card.md)
> says otherwise — it holds the four things this page asks for that the client
> deliberately does not do, the largest being that the fill is opaque because
> the gump pass does not blend. The page is kept as written rather than rewritten
> into the past tense: it is the argument the code was built from, and the
> evidence file is the difference.

What the client drew before this landed is what the first paragraph below calls
the thing to replace: an unframed stack of text at the cursor, from an
`App::hover_tooltip` that returns `Vec<String>`. The protocol half is built and
is not in scope here —
`0xDC` announces a revision, `0xD6` supplies the list, `Tooltips` deduplicates by
`(Serial, revision)` and `WorldView::tooltips` is the cache. What is not built is
everything between that cache and the screen.

What is built in this area is [`docs/client/README.md`](../../../docs/client/README.md);
the record of how the current tooltip landed is
[`docs/client/evidence/2026-08-15-tooltips.md`](../../../docs/client/evidence/2026-08-15-tooltips.md).
This page is only what is not.

## Outcome

An object property list (OPL) is information about one game object, not a
string that happens to be near the pointer.  The client presents it as one
readable **property card**.  A short hover gives the player a quick answer;
remaining on the object opens the complete list of properties without a click
or a new request.  The same card works for an item in the world, a container,
a paperdoll, a vendor list, and a mobile.

This replaces the current unframed stack of text at the cursor.  The protocol
and its cache stay the source of truth: `0xDC` announces a revision, `0xD6`
supplies an OPL, and the hover is still the only thing that asks for it.

## Interaction

| Moment | What the player sees | Network behaviour |
| --- | --- | --- |
| Pointer enters an object | Nothing until an OPL is already cached or arrives.  A card must never flash an empty loading box. | At most one `0xD6` for that serial and revision. |
| OPL arrives | The title and first property line appear immediately. | No extra request. |
| Pointer remains for 350 ms after content is available | The card expands to all properties. | No extra request. |
| Pointer moves to another object, leaves the game surface, or starts a drag | The card closes immediately. | The cached OPL remains reusable. |
| A newer OPL revision arrives while the card is open | Keep the last complete card visible, mark it as updating only if the next frame needs it, then replace its contents atomically. | One request for the new revision, never one per frame. |

There is deliberately no click binding in the first version.  Left click is
selection/attack/drag and right click steers; taking either gesture for an
inspection card would make ordinary play worse.  A pinned inspect window can
be added later as an explicit pane, once a player has a reason to compare two
objects.  It must not be smuggled into the transient tooltip.

## Card layout

```
  +------------------------------------+
  |  [ icon ]  Exceptional longsword   |
  |            crafted by Alys         |
  |------------------------------------|
  |  Slayer: ogre (+25% damage)        |
  |  Damage bonus: +3 to +7            |
  |  Hit poison: level 2, 15.0%        |
  +------------------------------------+
```

- The card has a nearly black, translucent fill, a one-pixel muted border,
  8 gump pixels of inner padding, and a four-pixel gap between title and
  body.  It is visually a game UI element, not an egui panel.
- The first resolved OPL line is the title.  The next one or two identifying
  lines (quality, maker, stack amount) form the header; the remainder are the
  body.  Until the server supplies semantic metadata this is an intentional,
  stable *display rule*, rather than English-string parsing.
- The icon is optional.  Items use their drawn graphic when it is available;
  mobiles and objects with no suitable art reserve no blank icon column.
- Title uses the existing window-label face at the tooltip size; body uses the
  existing tooltip face.  Quality may tint only the small title accent, never
  the property text, so every property remains readable against every hue.
- Lines wrap at word boundaries.  The card's preferred width is 280 gump
  pixels; it may shrink to 220 or grow to 360.  Its height is content-driven,
  capped at 60% of the game surface.  A future property list that exceeds this
  cap needs a pinned inspect pane; silently dropping stats is not acceptable.

The card is an overlay, drawn after all gump windows and the held-item count.
It does not take pointer ownership, does not block the world, and cannot be
clipped by the bag or paperdoll whose object it describes.

## Placement

The layout engine measures the final card before drawing it, then tries four
anchors around the cursor in this order: down-right, down-left, up-right,
up-left.  Each candidate has a 14-by-18 gump-pixel pointer gap and a 12-pixel
screen margin.  It selects the first wholly visible candidate; when none fits,
it clamps the least-overflowing one to the game surface.  This keeps the item
visible, avoids a card running off the lower-right corner, and works equally
for icons in windows and objects in the world.

The card never follows microscopic pointer motion while its subject remains
the same; it recomputes placement only when the subject, content, or usable
surface changes.  That prevents edge-flipping flicker.

## Data contract

`PropertyListReply` remains the client/server contract for the first version.
`Tooltips` continues to deduplicate questions by `(Serial, revision)`, and
`WorldView::tooltips` remains the cache.  The presentation layer receives a
typed value such as:

```rust
struct TooltipPresentation<'a> {
    serial: Serial,
    graphic: Option<Graphic>,
    entries: &'a [PropertyEntry],
    phase: TooltipPhase,
}
```

It is important that this replaces `Vec<String>` only at the presentation
boundary.  A bare list of strings loses the serial (needed for stable hover
timing), the item graphic, and the distinction between an empty OPL and no
tooltip yet.

The server should keep adding properties to `WorldState::object_properties`.
The current name, stack amount, exceptional quality, crafter, and affixes all
appear in the card automatically.  Do **not** derive categories or values by
parsing resolved text: text changes with the client's language and custom
shards.  When the game needs a two-column stat grid, resistance bars, comparison
arrows, or filtering, add a separate structured `ItemProperty` model alongside
the OPL and localise it at the edge.  OPL stays the compatibility fallback and
the authoritative free-form text channel.

Static map art has no serial and consequently has no OPL; it keeps the current
no-tooltip behaviour.  In `tooltips = "off"` mode the classic single-click
name path remains unchanged.

## Implementation boundaries

1. Make `tooltips.rs` own only timing and presentation state (`subject`,
   `content-ready-at`, and compact/detail phase); retain its existing request
   deduplication tests.
2. Change `App::hover_tooltip` in `picking_query.rs` to return the typed
   presentation value rather than rendered strings.  It is the one place that
   knows subject pick order and may issue `query_properties`.
3. Add a small colour-rectangle overlay primitive to `client/render`, then draw
   fill and border before the card's existing text pass in `render_passes.rs`.
   Do not route the overlay through egui or fake it from a stretched gump art.
4. Put measuring, wrapping, and anchor choice in a pure layout module.  The
   renderer receives already-measured rectangles and labels; it does not decide
   semantics or inspect the world.

## Acceptance checks

- Hovering an item in a bag and an item in the world produces the same card
  from the same OPL, above every open window.
- Holding the pointer over a stale object sends one query; receiving a changed
  revision permits exactly one new query.
- An object at every screen edge leaves the card completely on screen whenever
  its measured size fits the surface.
- A delayed reply never creates a blank or incorrectly attributed card after
  the pointer has moved to another serial.
- A newer reply replaces all visible lines in one frame; old and new property
  lists never interleave.
- With no cliloc table, no card is drawn; raw cliloc numbers are never exposed.

