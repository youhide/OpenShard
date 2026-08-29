# Item-tooltip widget prototype

Open [`item-tooltip-widget.html`](item-tooltip-widget.html) in a browser.  It
is a standalone visual and interaction prototype for the reusable item-property
card, intentionally not wired into the crafting-table prototype yet.

The only data API is `renderItemTooltip(model)`.  The model contains a title,
subtitle, optional icon, named property sections, and an optional footer; it
has no serial, recipe index, networking, or craft-window state.  A future
`CraftPane` will adapt a selected recipe's output to that model, while world
items can adapt an OPL to the same model.

The widget has three states demonstrated by the samples: paired stats, a
requirement warning, and free-form text.  It is intentionally pointer-passive:
the host row owns the hover and the card cannot steal a click or wheel event.

This is a visual proof, not production renderer code.  The production port
should keep the data contract and layout, but draw it through the client gump
overlay rather than through the browser DOM.
