# Font upscaling journal

Living context for the `fonts.mul` super-resolution experiment. Update this
file whenever a model, alpha policy, file layout, renderer path, or conclusion
changes.

## Current state — 2026-08-24

Stage 1 is complete: the shipped bitmap font was decoded, upscaled 4x with
`realesrgan-x4plus-anime`, repacked into a separate valid `fonts.mul`, and
rendered to transparent before/after PNGs. The generated MUL parses back as ten
complete faces.

This is not integrated into the client yet. The current 2048x2048 fixed-grid
`FontAtlas` cannot hold this result: its largest original cell is 25 pixels and
its largest 4x cell is 100 pixels. A 100-pixel grid has room for only
`floor(2048 / 100)^2 = 400` glyphs, while the file has 2,240 slots (1,319 with
ink). Atlas packing/paging and the distinction between asset pixels and logical
text size must be solved before the client can select this MUL at runtime.

## Where everything is

Source data on this workstation:

- `.env` sets `OPENSHARD_CLIENT` to
  `/home/sc/t/uo_files/Electronic Arts/Ultima Online Classic`.
- Original file:
  `/home/sc/t/uo_files/Electronic Arts/Ultima Online Classic/fonts.mul`
  (884,909 bytes; 884,766 bytes belong to the ten decoded faces and 143 bytes
  trail them).

Implementation:

- `crates/common/uofiles/src/font.rs` — MUL decoder, glyph replacement, and
  the new semantic encoder.
- `crates/common/uofiles/src/bin/openshard-font-upscale.rs` — PNG export,
  Real-ESRGAN invocation, alpha handling, MUL import, and preview rendering.
- `Cargo.toml` and `crates/common/uofiles/Cargo.toml` — direct `png` dependency
  for the offline converter.

First experiment (generated, under ignored `target/`, so `cargo clean` removes
it):

- `target/font-upscale/anime-4x-alpha128/fonts-upscaled-4x.mul` — repacked
  result, 14,055,306 bytes.
- `target/font-upscale/anime-4x-alpha128/before.png` — original render,
  transparent RGBA, 500x246.
- `target/font-upscale/anime-4x-alpha128/after.png` — neural 4x render,
  transparent RGBA, 2000x984.
- `target/font-upscale/anime-4x-alpha128/comparison.png` — original enlarged
  with nearest-neighbour on the left, neural result on the right, both over a
  checkerboard; 4016x984.
- `target/font-upscale/anime-4x-alpha128/glyphs-before/` — 1,319 padded RGBA
  inputs.
- `target/font-upscale/anime-4x-alpha128/glyphs-after/` — 1,319 model outputs.

The downloaded portable runner and weights currently live only in
`/tmp/openshard-realesrgan.2SK0u0/`. They are not project dependencies or
durable artifacts.

## `fonts.mul` format

There is no index and no offset table. Records are walked front to back:

```text
repeat 10 faces:
    u8 face_header                 // unused
    repeat 224 characters:
        u8 width
        u8 height
        u8 glyph_header            // unused
        u16le pixels[width*height] // row-major
```

The 224 entries are byte characters `0x20..=0xFF`. Pixel colour is Ultima's
16-bit `0RRRRRGGGGGBBBBB` with five bits per channel. A zero word means
transparent for glyphs; the format has no partial alpha. The top bit carries
no colour. The encoder sets it on opaque pixels so an upscaler-produced black
pixel becomes `0x8000` and is not confused with transparent zero.

Width and height are bytes, so neither dimension can exceed 255. This source's
largest glyph is 23x25; the 4x result tops out at 92x100 and still fits.

The reader deliberately stops after ten faces. Repacking writes unused header
bytes as zero and drops the original file's 143 trailing bytes. It preserves
decoded meaning, not unused bytes.

## Transparency pipeline

Transparency is treated as data, not as a black background:

1. Decode each glyph into RGB plus binary alpha (`word == 0` means alpha 0).
2. Extend the nearest opaque RGB colour through transparent pixels (colour
   bleed), while leaving their alpha at zero. This avoids a dark fringe.
3. Add eight transparent source pixels of context around every glyph.
4. Give the RGBA PNG to Real-ESRGAN NCNN. Its implementation runs the model on
   RGB and resizes alpha separately with bicubic interpolation.
5. Crop the padding at the scaled size (32 pixels in the 4x run).
6. Convert alpha back to MUL's binary mask. The first run uses
   `alpha >= 128 => opaque`.
7. Quantize model RGB from 8-bit back to five bits per channel and repack.

The retained `glyphs-after` PNGs allow testing another alpha threshold with
`--skip-inference`; inference does not have to run again.

## What was considered and tried

The initial survey came from
[Local AI Image Upscaling](https://localaimaster.com/blog/ai-image-upscaling-local)
and was checked against the
[official Real-ESRGAN repository](https://github.com/xinntao/Real-ESRGAN) and
[official NCNN/Vulkan runner](https://github.com/xinntao/Real-ESRGAN-ncnn-vulkan).

- `RealESRGAN_x4plus`: general photographic/mixed-content model. Candidate for
  comparison, but not the first choice for small flat-colour glyphs.
- `RealESRGAN_x4plus_anime_6B` / NCNN name
  `realesrgan-x4plus-anime`: chosen for the first run because it is trained for
  line art, sharp edges, and flat colour.
- `realesr-general-x4v3`: fast, small candidate for a later fidelity/speed
  comparison.
- `4x-UltraSharp`: plausible illustration candidate, but not bundled with the
  official portable runner used here.
- SwinIR: restoration/de-JPEG is not the main problem in a lossless bitmap
  font, so it is lower priority.
- GFPGAN and CodeFormer: face restorers, irrelevant to glyphs.

First actual run:

- model: `realesrgan-x4plus-anime`;
- scale: 4x;
- source padding: 8 pixels;
- alpha threshold: 128;
- hardware/backend: AMD Radeon RX 7700 XT through Vulkan/RADV;
- exported/model-processed glyphs: 1,319;
- all 2,240 slots remain in the output; blank glyph rectangles are scaled
  without wasting model inference;
- model pass: about 25 seconds including the command/build wrapper observed in
  this session;
- result: the letter contours are visibly smoother and the ten sample faces
  retain transparent backgrounds and their original colours. Some tiny,
  intentionally sparse faces remain sparse; this is source design, not missing
  alpha.

One setup failure occurred before the run: the freshly unzipped portable binary
did not have its executable bit set. After `chmod u+x`, the same command ran
successfully. No failed inference output was imported.

## Reproduce

With an official NCNN bundle unpacked locally:

```bash
cargo run -p openshard-uofiles --bin openshard-font-upscale -- \
  --client "$OPENSHARD_CLIENT" \
  --upscaler /path/to/realesrgan-ncnn-vulkan \
  --models /path/to/models \
  --out target/font-upscale/anime-4x-alpha128
```

Repack the existing model outputs with another threshold:

```bash
cargo run -p openshard-uofiles --bin openshard-font-upscale -- \
  --client "$OPENSHARD_CLIENT" \
  --upscaler /path/to/realesrgan-ncnn-vulkan \
  --models /path/to/models \
  --out target/font-upscale/anime-4x-alpha128 \
  --alpha-threshold 96 \
  --skip-inference
```

Validation run after implementation:

```text
cargo test -p openshard-uofiles --lib --bin openshard-font-upscale
135 library tests passed, 1 ignored; 2 converter tests passed.
```

## Plan

- [x] Document and validate the classic font container.
- [x] Add semantic MUL encoding and reject dimensions above 255.
- [x] Export transparency-safe RGBA glyph inputs with colour bleed and padding.
- [x] Run the first line-art model at 4x.
- [x] Repack a separate MUL and parse it back.
- [x] Render transparent before/after PNGs and a same-scale comparison.
- [ ] Compare alpha thresholds (suggested: 64, 96, 128, 160) from the retained
  outputs and inspect thin strokes, holes, and punctuation.
- [ ] Compare at least one conservative baseline (nearest or Lanczos) and
  `RealESRGAN_x4plus`; optionally add `realesr-general-x4v3` and UltraSharp.
- [ ] Decide whether the wanted runtime result is truly 4x logical text or 4x
  raster density rendered at the original logical size. The latter needs
  separate logical advance/size from stored image dimensions.
- [ ] Replace the fixed one-cell-size `FontAtlas` with shelf/bin packing or
  paged atlases; verify 2,240 4x slots fit the GPU texture limits.
- [ ] Add an explicit client option for the alternate MUL only after atlas and
  logical sizing are decided. Do not overwrite the installed `fonts.mul`.
- [ ] Capture in-client screenshots at representative UI scale, world zoom,
  and display density before choosing a model as the default asset pipeline.

## Next decision

The cheapest next experiment is an alpha-threshold contact sheet because it
reuses the 1,319 completed neural outputs. The most important engineering step
after that is separating high-density raster pixels from logical glyph metrics;
otherwise loading a 4x MUL simply makes every line four times larger rather
than sharper at the same apparent size.
