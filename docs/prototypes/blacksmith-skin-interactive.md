# Interactive blacksmith skin prototype

Open `blacksmith-skin-interactive.html` in a browser from the repository. It
uses the same `blacksmith-skin-kit-v2.png` atlas and coordinates as the offline
renderer, but owns a small local interaction state for design review:

- click a recipe to select it;
- wheel over the list, or click the scrollbar track, to change its scroll
  position;
- click a material's checkbox to preview a missing/available state;
- click either craft button to show the available-material validation result.

The list viewport ends at `x = 220`; the scrollbar starts there, so its hit box
and pixels cannot overlap a recipe row. This is a design prototype only: its
local actions do not contact a shard. The game client already sends ordinary
`GumpReply` messages for the server's current craft gump; the production step
is to give its dedicated `CraftPane` the same hit/state model and skin atlas.

For a local static check, this was rendered in Chromium headless against the
repository path. No external web server or account is required.
