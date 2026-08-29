# Gump preview renderer

`openshard-gump-render` turns a small RON scene into a PNG using the client
renderer’s own gump atlas, text layout and `resizepic` implementation. It also
composes project-owned RGBA skin atlases, including cropped controls and
nine-slice panels. It is an offline design-review tool: it opens no window and
needs no GPU, so an agent can write a scene, render it, inspect the PNG and
iterate from a screenshot or sketch.

```sh
cargo run -p openshard-gump-render -- \
  crates/client/gump-render/examples/admin-panel.ron \
  --client "$OPENSHARD_CLIENT" --out /tmp/admin-panel.png --scale 2
```

`OPENSHARD_CLIENT` may replace `--client`. It is required only for classic UO
art or bitmap labels. The scene’s `width` and `height` are
gump pixels; `--scale` uses nearest-neighbour enlargement only for inspection.
The output path defaults to the scene path with `.png` in place of `.ron`.

## Scene format

The file is RON. Project skin elements are painter-ordered; classic UO art is
drawn over that skin and text is last, so captions remain legible.

```ron
(
    width: 360,
    height: 270,
    background: (r: 64, g: 0, b: 96),
    elements: [
        Resize(gump: 5054, x: 0, y: 0, width: 360, height: 270),
        Tile(gump: 2624, x: 20, y: 45, width: 160, height: 100),
        Gump(gump: 4005, x: 30, y: 56),
        Item(graphic: 0x0E75, x: 250, y: 150),
        Label(x: 66, y: 58, text: "Populate Felucca", font: 1),
    ],
)
```

An optional `backdrop: Some("path/to/panel.png")` is a project-owned RGB/RGBA PNG
drawn below the declarative gump layers. It must exactly match the scene's
logical dimensions; this makes a reviewed visual prototype reproducible while
the reusable 9-slice art is being prepared.

- `Gump` draws native-size art from `gumpartLegacyMUL.uop`.
- `Item` draws a static icon from `artLegacyMUL.uop`.
- `Tile` repeats an art source without rescaling it.
- `Resize` is the client’s nine-piece `resizepic`, including its non-obvious
  piece order and seam offsets.
- `Label` uses the client’s bitmap font; it defaults to face `1`.
- `Asset` copies an RGBA atlas rectangle at native size. It is intended for
  fixed controls such as a scroll track or icon.
- `ScaledAsset` copies an RGBA atlas rectangle into a fixed target size using
  nearest-neighbour sampling. It is used by the blacksmith scenes for
  independent empty and checked checkbox sprites.
- `NineSlice` keeps its four source corners at 1:1 and changes only the edge
  and centre regions. Its optional `tile: true` repeats those regions for
  deliberately seamless art; the default stretches a painterly centre once.
- `Text` rasterizes a project-owned TrueType face and supports UTF-8, including
  Cyrillic, without `--client`.

The blacksmith skin proof is completely self-contained — it needs neither a UO
installation nor a backdrop:

```sh
cargo run -p openshard-gump-render -- \
  crates/client/gump-render/examples/blacksmith-skin-compact.ron \
  --out docs/prototypes/blacksmith-skin-compact.png --scale 1

cargo run -p openshard-gump-render -- \
  crates/client/gump-render/examples/blacksmith-skin-wide.ron \
  --out docs/prototypes/blacksmith-skin-wide.png --scale 1
```

The source rectangles, fixed insets and control-state mapping live beside the
atlas in `assets/ui/crafting/blacksmith-skin-kit-v2.md`.

For interaction review without a running shard, open
`docs/prototypes/blacksmith-skin-interactive.html`. Its canvas uses the same
atlas and corrected list viewport, while click/wheel behaviour is documented in
`docs/prototypes/blacksmith-skin-interactive.md`.

The preview deliberately uses untinted art in its first version. It is for
checking composition, pixel alignment, frame choice and text placement; hue
and interactive controls remain client/runtime responsibilities until their
design needs a preview surface too.
