# Blacksmith skin kit v2

`blacksmith-skin-kit-v2.png` is an RGBA source atlas, not a screenshot placed
behind the UI. The preview scenes refer to its regions directly, so replacing
one component cannot accidentally change another one.

| Component | Source rectangle | Fixed inset | Behaviour |
| --- | --- | --- | --- |
| dark panel | `894,62 290×266` | `13,13,13,13` | nine-slice; pane background |
| parchment panel | `884,353 300×285` | `13,13,13,13` | nine-slice; recipe detail |
| dark row | `60,690 286×97` | `10,10,10,10` | nine-slice; normal list state |
| gold row | `347,688 298×100` | `10,10,10,10` | nine-slice; selected list state |
| light row | `652,687 258×103` | `10,10,10,10` | nine-slice; fulfilled resource |
| red row | `910,687 275×103` | `10,10,10,10` | nine-slice; missing resource |
| scroll track | `121,827 84×385` | n/a | fixed native asset; thumb overlays it |
| scroll thumb | `347,852 85×208` | n/a | fixed native asset; its `y` expresses scroll position |
| checkbox, empty | `568,891 98×96` | n/a | native state sprite |
| checkbox, checked | `771,887 105×98` | n/a | native state sprite |
| action buttons | `543,1070 319×116`, `875,1068 332×121` | `12,12,12,12` | nine-slice; disabled and gold actions |

The renderer’s `NineSlice` copies corners 1:1. Its `tile: true` mode repeats
edge and centre segments for deliberately seamless art; the checked-in scenes
use the default stretched centre because this generated, painterly first-pass
atlas is not seamless. In both modes bevel widths and metal corner decoration
stay fixed. The next asset pass will export a small seamless wood/parchment
centre tile, then turn tiling on for those regions. In an in-game
implementation these rectangles become sprite metadata in the client texture
atlas; their states are selected from typed crafting state, not inferred from
pixels.

The checked-in proof scenes are:

- `crates/client/gump-render/examples/blacksmith-skin-compact.ron` — 1024×640
- `crates/client/gump-render/examples/blacksmith-skin-wide.ron` — 1440×860

Both compose the exact same kit, including an independently positioned scroll
thumb and distinct empty/checked checkboxes.

## Reserved item-slot art

`item-slot-frames-v1.png` is a standalone normal/hover ornamental slot frame,
kept for a later skin pass. The current interactive prototype deliberately
uses a plain egui-style rectangle instead, so item readability and hover
behaviour can be reviewed without committing to decorative chrome.
