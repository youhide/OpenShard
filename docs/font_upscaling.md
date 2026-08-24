# Font upscaling journal

Living context for the `fonts.mul` super-resolution experiment. Update this
file whenever a model, alpha policy, file layout, renderer path, or conclusion
changes.

## Current state — 2026-08-24

The active local direction is contour-only neural upscaling. Full-colour neural
passes, chroma key, and multi-matte all introduced unwanted texture or inferred
detail. In the current pipeline the network sees only a black/white source
silhouette and its luminance becomes the new alpha; original RGB is scaled
separately with nearest-neighbour and is never sent through the network.

Contour-only has been run on font 0 `A` and an eight-glyph stress set through
three unique local models. RGB, alpha-only, soft-alpha, and threshold-128 sheets
exist. This is the first tested direction that changes the jagged silhouette
without inventing surface material.

The official font-specific DeepVecFont SR checkpoint has now also been run on
`O`, `S`, and `a`. It preserves legacy geometry but largely preserves the
staircase too, so it is documented and rejected rather than promoted to a
full-font candidate. StarVector-1B is the remaining local neural image-to-SVG
candidate, but requires a better-provisioned CUDA environment for a responsible
test.

The full font has also been upscaled 4x with both anime and general
Real-ESRGAN models. The anime result was repacked into a separate valid MUL and
parsed back as ten complete faces. Client atlas/runtime work is paused until a
model and alpha policy have been chosen from representative glyph tests.

A word-context experiment is complete. Font 0 `OpenShard` was assembled
from nine real glyphs; its outer silhouette and light-fill mask are processed
separately so the dark outline remains an independent layer. The first word
assembly accidentally contained two opaque white strips below the short glyphs
because of ImageMagick append canvas behaviour. Those initial contact sheets
are retained as debugging evidence but are invalid for model selection.
`input-word-clean.png` is the corrected explicit-coordinate source.

Corrected whole-word results were compared with the same nine glyphs processed
individually. At threshold 128, context changed only 388 of 38,304 RGBA pixels
for anime, 243 for animevideo, and 311 for UltraSharp (about 0.6–1.0%). The
changes are local boundary interactions, not improved letter understanding.
These patch-based local SR models do not gain semantic text context from words.

Three built-in ChatGPT image-edit trials previewed the current OpenAI image API
approach. Tiny 130x37 references produced blocky redesigned letters. A
1024x1024 nearest-enlarged reference produced smooth-looking contours, but
changed glyph design, widths, and spacing. File inspection also found that the
visible checkerboard was baked into RGB and the PNG had no alpha channel,
despite the prompt requesting transparency. Generative editing can create a new
clean font master, but is not a deterministic super-resolution replacement
when exact legacy shapes matter. A direct API test with the explicit
`background: "transparent"` parameter has not yet been run.

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
- `tools/font_contour_upscale.py` — directory-level silhouette export and
  recombination of neural alpha with untouched source RGB.
- `tools/font_deepvecfont_upscale.py` — adapter for DeepVecFont's official
  font-specific 128-to-256 SR generator; accepts white-on-black glyph masks and
  returns soft masks at the requested logical scale.
- `Cargo.toml` and `crates/common/uofiles/Cargo.toml` — direct `png` dependency
  for the offline converter.

Generated experiments live under ignored `font-upscale-artifacts/`. This is
outside `target/`, so `cargo clean` does not remove downloaded weights or
results:

- `font-upscale-artifacts/anime-4x-alpha128/fonts-upscaled-4x.mul` — repacked
  result, 14,055,306 bytes.
- `font-upscale-artifacts/anime-4x-alpha128/before.png` — original render,
  transparent RGBA, 500x246.
- `font-upscale-artifacts/anime-4x-alpha128/after.png` — neural 4x render,
  transparent RGBA, 2000x984.
- `font-upscale-artifacts/anime-4x-alpha128/comparison.png` — original enlarged
  with nearest-neighbour on the left, neural result on the right, both over a
  checkerboard; 4016x984.
- `font-upscale-artifacts/anime-4x-alpha128/glyphs-before/` — 1,319 padded RGBA
  inputs.
- `font-upscale-artifacts/anime-4x-alpha128/glyphs-after/` — 1,319 model outputs.
- `font-upscale-artifacts/general-4x-alpha128/` — equivalent general-model run.
- `font-upscale-artifacts/one-letter-A/` — controlled single-glyph inputs,
  model outputs, transparency studies, and contact sheets.
- `font-upscale-artifacts/runners/` — durable archives, binaries, and weights
  for Real-ESRGAN, waifu2x, Real-CUGAN, and Upscayl UltraSharp.
- `font-upscale-artifacts/word-context-OpenShard/` — word-level two-mask tests,
  corrected word source, individual glyph sources, and ChatGPT edit results.

The original client file is never overwritten and generated client-derived
assets are not committed.

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
The other downloaded runners/models come from
[waifu2x-ncnn-vulkan](https://github.com/nihui/waifu2x-ncnn-vulkan/releases/tag/20250915),
[realcugan-ncnn-vulkan](https://github.com/nihui/realcugan-ncnn-vulkan/releases/tag/20220728),
and [Upscayl's NCNN model directory](https://github.com/upscayl/upscayl/tree/main/resources/models).

[Slice3D](https://github.com/yizhiwang96/Slice3D) is a speculative reference
for this experiment. It will probably produce unusable noise on tiny bitmap
glyphs, but the underlying idea — recovering a clean shape from slices rather
than merely smoothing pixels — is worth keeping in view. It is not a dependency
or a commitment to use its code.

Download/model SHA-256 values recorded in this session:

```text
e5aa6eb131234b87c0c51f82b89390f5e3e642b7b70f2b9bbe95b6a285a40c96  realesrgan.zip
848e0fba55657d34da90b775b8139e9806dc754798b029f95e106ba8850a731f  waifu2x-linux.zip
d745174bd04c0232c89d935b74799311008fda06bea4195f61be5f0f3cc087cb  realcugan-ubuntu.zip
0136ca83686809a8f17f7111f11b951e8db93610e24b7f4137c9ffe4dbc4a806  ultrasharp-4x.param
fb3e279d40d4cddb44db4e684d59e68d0aa39852c8cc14dc3f23ccc7e6eee9c1  ultrasharp-4x.bin
2b8fb6e0ae4d2d85704ca08c119a2f5ea40add4f2ecd512eb7f4cd44b6127ed4  digital-art-4x.param
fe01c269cfd10cdef8e018ab66ebe750cf79c7af4d1f9c16c737e1295229bacc  digital-art-4x.bin
```

- `RealESRGAN_x4plus`: general photographic/mixed-content model; tested on the
  controlled `A` and the complete font.
- `RealESRGAN_x4plus_anime_6B` / NCNN name
  `realesrgan-x4plus-anime`: chosen for the first run because it is trained for
  line art, sharp edges, and flat colour.
- `realesr-general-x4v3`: fast, small candidate for a later fidelity/speed
  comparison.
- `realesr-animevideov3-x4`: tested; more conservative and smoother.
- waifu2x NCNN 20250915, `models-cunet` and anime upconv7: tested; both are
  softer than the Real-ESRGAN line-art result.
- Real-CUGAN NCNN 20220728, `models-se`: tested; the softest result on `A`.
- Upscayl `ultrasharp-4x`: downloaded as NCNN `.param + .bin` and tested through
  the local Real-ESRGAN runner; sharp silhouette, flatter interior.
- Upscayl `digital-art-4x`: downloaded and tested on the representative
  mini-set. Its `.param` and `.bin` hashes are exactly identical to the bundled
  `realesrgan-x4plus-anime` files, so it is an alias, not a fourth unique model.
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

An attempted `realesrnet-x4plus` run produced an invalid black PNG because that
model's `.param/.bin` files are not present in the portable bundle. The file is
retained as failed evidence but excluded from comparisons and MUL imports.

### Controlled one-letter matrix

Input `one-letter-A/input-A-padded.png` is a 14x21 glyph with eight transparent
source pixels on every side, hence a 30x37 RGBA PNG. Each x4 output is 120x148;
reports crop 32 output pixels of padding to compare the same 56x84 glyph area.

The current report contains, in order: nearest-neighbour source,
Real-ESRGAN anime, Real-ESRGAN general, animevideo v3, waifu2x cunet, waifu2x
anime, Real-CUGAN SE, and UltraSharp.

Measured transparency results:

- Real-ESRGAN anime/general/animevideo, Real-CUGAN, and UltraSharp have
  byte-for-byte identical alpha planes because the runner resizes alpha outside
  the neural model;
- waifu2x performs two 2x passes for x4, so its intermediate bicubic rounding is
  slightly different: 1,127 alpha bytes differ, maximum delta 13 and mean
  absolute delta about 0.993;
- at threshold 128, all seven neural configurations still produce exactly the
  same binary mask: 1,915 opaque pixels and zero pixels differing between masks;
- on `A`, thresholds 64/96/128/160 retain respectively
  2,082 / 1,972 / 1,915 / 1,868 opaque pixels.

The dark interior visible inside this `A` is mostly opaque RGB, not transparent
alpha. A model may therefore alter it even when the alpha plane is unchanged.

The representative mini-set in `font-upscale-artifacts/mini-set/` contains
`!`, `.`, `I`, `O`, `8`, a conventional `A`, a sparse gold `A`, and a coloured
`A` from another face. `mini-set-rgb.png` and `mini-set-threshold128.png`
compare source-nearest, Real-ESRGAN anime, animevideo v3, Upscayl Digital Art,
and UltraSharp. It confirms that model ranking depends on glyph type: anime
adds strong bevel/detail, animevideo is conservative but soft, and sparse
glyphs provoke hallucinated dark mass in all tested neural outputs.

### Contrast-colour / chroma-key test

The same input was flattened first onto opaque `#ff00ff`, processed by local
`realesrgan-x4plus-anime`, and filtered with chroma-key fuzz values 5–30%.
This removes the flat background but produces a magenta fringe and visibly
contaminates the light face of the glyph.

A second inference on `#00ff00` allowed a dual-matte alpha estimate from the
difference between the magenta and green outputs. This demonstrated that the
network's response to backgrounds can produce a new, non-bicubic alpha rather
than merely preserve the jagged source mask. The initial two-background result
has 1,770 opaque pixels at threshold 128 versus 1,915 for bicubic source alpha;
187 binary-mask pixels differ.

The known source alpha is therefore only a fidelity baseline, not automatically
the desired result: preserving it also preserves its staircase geometry.

### Multi-matte alpha — current experiment

Eight opaque backgrounds form four complementary pairs:

```text
magenta / green
cyan    / red
yellow  / blue
white   / black
```

The same padded `A` is inferred eight times with local
`realesrgan-x4plus-anime`. Each pair estimates alpha as one minus the normalized
RGB distance between its two outputs. Two aggregations are retained: the mean
of all four estimates and a trimmed mean which drops the per-pixel minimum and
maximum before averaging.

Raw multi-matte alpha obtains a visibly smoother, model-shaped outer silhouette
but also contains internal structure: the network reacts to matte colour inside
the glyph, not only at its boundary. Three constrained variants use the
multi-matte estimate only in a 2/4/6-output-pixel band around the old edge,
forcing deep interior to opaque and far exterior to transparent.

At threshold 128 on the 56x84 `A`:

```text
method                 opaque pixels   pixels different from bicubic mask
bicubic baseline       1915            0
four-pair average      1795            138
trimmed average        1780            151
edge band 2 px         1821            110
edge band 4 px         1809            122
edge band 6 px         1792            139
```

No option is selected yet; the user will decide visually. Soft PNG alpha and
binary MUL alpha must be evaluated separately.

The same eight-background experiment has also been completed for three stress
cases: font 0 `!`, font 0 `O`, and the sparse gold font 5 `A` (24 local neural
inferences). This exposes behaviours hidden by the first `A`:

- raw average/trimmed alpha follows the network's rounded `O` silhouette but
  also reproduces internal response-to-background structure;
- a 2 px boundary band keeps `!` and the glyph interior clean while still
  allowing subpixel edge changes;
- 4/6 px bands increasingly admit the internal multi-matte structure;
- at logical text size the binary variants are much closer than their enlarged
  alpha masks suggest, so both 4x inspection and downsampled presentation are
  retained for the decision.

A second aggregation uses all eight outputs simultaneously as a per-channel
linear regression rather than pairing images. For each channel it subtracts the
mean neural output of the four mattes where that input channel is zero from the
mean of the four where it is one. Expected background response is `1-alpha`, so
the three channel slopes are averaged and subtracted from one:

```text
slope_r = mean(out.r | matte.r=1) - mean(out.r | matte.r=0)
slope_g = mean(out.g | matte.g=1) - mean(out.g | matte.g=0)
slope_b = mean(out.b | matte.b=1) - mean(out.b | matte.b=0)
alpha   = clamp(1 - (slope_r + slope_g + slope_b) / 3, 0, 1)
```

This regression is visibly cleaner inside the glyph than the four-pair average
while retaining the model-shaped outer contour. At threshold 32 it keeps 1,937
pixels for the first `A` versus 1,915 in the bicubic baseline. On the three
stress glyphs its threshold-32 areas are 956 / 2,246 / 907 versus bicubic
956 / 2,172 / 920. It is retained as another candidate, not automatically
selected.

Threshold 128 is not calibrated for the recovered matte: multi-matte alpha is
systematically lower than bicubic alpha. A sweep over 16–128 shows that roughly
32 preserves the original opaque area across these stress cases (the exact
area-matching point varies by glyph). Examples for the raw average:

```text
glyph             old/bicubic area   multi t32   multi t64   multi t128
font 0 !          956                 956         912         875
font 0 O          2172                2247        2113        2049
font 5 sparse A   920                 908         822         655
```

Area matching is only a calibration aid, not a quality criterion. The visual
sheet keeps thresholds 32/64/128 side by side so the user can choose the wanted
contour rather than letting this metric choose it.

Artifacts in `font-upscale-artifacts/multi-matte-mini/`:

- `comparison-soft.png` — 4x soft RGBA;
- `comparison-alpha.png` — soft alpha only;
- `comparison-threshold128.png` — binary 4x MUL candidates;
- `comparison-logical-soft.png` — soft variants filtered to logical size;
- `comparison-logical-threshold128.png` — binary variants filtered to logical
  size.
- `comparison-threshold-calibration.png` — average and 2 px band at thresholds
  32/64/128 beside the old 128 mask.
- `regression-stress-soft.png`, `regression-stress-alpha.png`, and
  `regression-stress-threshold.png` — old, pair-average, eight-background
  regression, and 2 px boundary band on the three stress glyphs.

Artifacts:

- `one-letter-A/chroma-key-comparison.png` — source, ordinary RGBA, one-colour
  key, and dual-matte result;
- `one-letter-A/chroma-key-fuzz.png` — one-colour key at 5–30%;
- `one-letter-A/chroma-dual-alpha.png` — recovered dual-matte alpha.
- `one-letter-A/multi-matte/result-comparison.png` — raw average with thresholds
  96/128/160;
- `one-letter-A/multi-matte/alpha-comparison.png` — all four pair estimates and
  their average;
- `one-letter-A/multi-matte/edge-band-comparison.png` — soft and binary results
  for average/trimmed/boundary-constrained variants;
- `one-letter-A/multi-matte/edge-band-alpha.png` — the corresponding alpha-only
  view.
- `one-letter-A/multi-matte/regression-comparison.png` and
  `regression-alpha.png` — eight-background regression beside earlier methods.

### Contour-only neural alpha — active direction

The input to the network is now only the opaque grayscale source alpha: black
outside, white inside, with the same eight-pixel padding. Model output luminance
is used as the replacement soft alpha. Source RGB is independently enlarged by
nearest-neighbour and combined with that alpha. The model cannot see or alter
glyph colour, shading, highlights, or texture.

Tested unique models are `realesrgan-x4plus-anime`,
`realesr-animevideov3-x4`, and `ultrasharp-4x`. Upscayl Digital Art is omitted
because its weights and graph are byte-identical to the anime model.

On font 0 `A`, threshold-128 opaque areas are:

```text
nearest source silhouette  1920
bicubic alpha at t128      1915
anime contour              1908
animevideo contour         1914
UltraSharp contour         1919
```

Across the eight-glyph stress set, animevideo and UltraSharp generally preserve
silhouette area more closely than anime; anime erodes the tiny dot and sparse
glyph more aggressively. Unlike full-colour inference, none of them introduces
surface texture because RGB never enters the network. Lanczos RGB was also
shown and rejected as too blurry; nearest RGB is the current colour policy.

Artifacts:

- `font-upscale-artifacts/contour-only-A/contour-result-comparison.png`;
- `font-upscale-artifacts/contour-only-A/contour-alpha-comparison.png`;
- `font-upscale-artifacts/contour-only-A/color-resampling-comparison.png`;
- `font-upscale-artifacts/contour-only-mini/contour-mini-soft.png`;
- `font-upscale-artifacts/contour-only-mini/contour-mini-alpha.png`;
- `font-upscale-artifacts/contour-only-mini/contour-mini-threshold128.png`.

### Word context and two boundaries

Font 0 uses exactly two opaque source colours in the tested word: dark outline
`#211821` and light fill `#9C9C9C`. `OpenShard` is assembled at 114x21 with
the original glyph widths and top alignment:

```text
O  x=0   14x21    p  x=14  13x19    e  x=27  12x19
n  x=39  12x19    S  x=51  11x21    h  x=62  13x19
a  x=75  15x19    r  x=90  11x19    d  x=101 13x19
```

Two black/white inputs are derived from the word: the complete opaque
silhouette and an exact mask of the light fill. They are upscaled independently,
the fill mask is clamped to the outer mask, and only the two exact source colours
are restored. Thus neither texture nor colour is generated by the local model.

Important correction: `input-word-padded.png`, the initial word results, and
the initial `word-context-*.png` sheets contain two unintended white strips.
The clean source is `input-word-clean.png`; its padded and enlarged references
are `input-word-padded-clean.png` and `chatgpt-reference-1024-clean.png`.
Corrected local inference and the individual-glyph comparison are in
`context-vs-individual-soft.png` and `context-vs-individual-t128.png`.
`glyph-detail-context-soft.png` enlarges `O`, `S`, and `a` in a seven-column
source/word/isolated comparison for direct visual inspection.

The first corrected fill-mask run also accidentally retained the source alpha,
causing the NCNN runner to resize that channel separately and making the word
look washed out. It was discarded and rerun with an explicitly opaque grayscale
fill mask. The final comparison sheets use only that corrected run.

At threshold 128, whole-word context versus isolated-glyph reconstruction
changes 388 / 38,304 RGBA pixels for anime, 243 / 38,304 for animevideo, and
311 / 38,304 for UltraSharp. Visual inspection agrees with the count: context
does not make the local models understand the word; it only changes a few
boundaries where neighbouring glyphs enter the receptive field.

Current OpenAI web documentation confirms that `gpt-image-2` supports image
edits through `/v1/images/edits`, always processes image inputs at high
fidelity, and advertises transparent PNG output in preview. The installed
imagegen CLI reference still marks `gpt-image-2` transparency unsupported and
recommends `gpt-image-1.5` for native alpha. This version mismatch requires a
real API probe; no API key is present in the current environment.

The output remains generative:
`chatgpt-large-reference-smooth.png` smoothed both boundaries visually but
redesigned some glyph geometry and spacing. Moreover, all three built-in edit
outputs are RGB without alpha; their checkerboards are baked into the image.
They test semantic reconstruction only, not the direct API transparency option.
The two tiny-reference outputs are kept beside the large-reference result to
make prompt/reference-size failures reproducible.

### DeepVecFont font-specific SR

The official [DeepVecFont repository](https://github.com/yizhiwang96/deepvecfont)
was cloned at commit `e1fe3255c876fa0d347018ffda80fe9b8d62ea1c` under
`font-upscale-artifacts/runners/deepvecfont`. Its published 128-to-256 image-SR
checkpoint was downloaded from the authors' Google Drive link:

```text
latest_net_G.pth  217,627,717 bytes
sha256 7c65eb702176898371bc0af4ece7d6e1193701141c56ca9858e1c876845bd59b
latest_net_D.pth   11,057,799 bytes
sha256 6413177ca765e62eacdd7fbc8ed04ac51006a177bb8803a432ed9b298b7c2f87
```

An isolated Python 3.11 environment lives inside the ignored runner directory;
it uses CPU PyTorch 2.13.0. The old checkpoint loads strictly into the official
54,403,457-parameter Pix2Pix U-Net with no missing or unexpected tensors.

DeepVecFont was trained on black TTF glyphs over white 128x128 canvases. The
adapter inverts our masks for inference, centres them in that canvas, runs the
official network, and converts output back to white-foreground masks. Two
occupancy modes were tested on the outer and fill masks of `O`, `S`, and `a`:
`zoom2` maps the network's 2x output directly to 4x logical size, while `zoom4`
shows the network a training-sized glyph and reduces its result to 4x.

Artifacts:

- `font-upscale-artifacts/deepvecfont-glyph-test/deepvecfont-comparison-soft.png`;
- `font-upscale-artifacts/deepvecfont-glyph-test/deepvecfont-comparison-t128.png`;
- `font-upscale-artifacts/deepvecfont-glyph-test/neural-vs-spline-comparison.png`.

Result: DeepVecFont preserves the coarse legacy geometry better than generic
SR, but largely preserves the pixel stair-steps and introduces small defects.
Its training inputs were already high-resolution antialiased TTF renders, so
12-to-21-pixel binary UO glyphs are far outside its learned degradation model.
It is not a candidate for the full font.

At threshold 128, `zoom4` changes 115 / 4,704 exact RGBA pixels on `O`,
73 / 3,696 on `S`, and 55 / 4,560 on `a` relative to nearest-neighbour 4x.
The small counts match the visual result: the model mostly reproduces the old
staircase instead of reconstructing a smoother master. `zoom2` is less stable
(229, 219, and 6 changed pixels respectively).

As a non-neural control, VTracer spline fitting was applied separately to the
same outer/fill masks. It creates smooth alpha, but can destroy the internal
fill topology (clearly visible on `S`). It is retained only as a diagnostic
lower bound and does not satisfy the local-neural requirement.

### Neural raster-to-vector candidate not run here

[StarVector-1B](https://huggingface.co/starvector/starvector-1b-im2svg) is the
most relevant remaining local model found in the survey: it is a 1B-parameter
vision-language image-to-SVG model, was evaluated specifically on SVG-Fonts,
and its authors report a 0.978 DINO score on that split. Unlike the tested SR
models it produces explicit vector code and is trained to recognize fonts.

It was not downloaded in this session. The official checkpoint is 5.15 GB,
the published inference path calls `.cuda()`, this machine exposes no NVIDIA
GPU, and only about 14 GiB of RAM was available with swap already full. The
Hugging Face page also reports no hosted inference provider. A CPU attempt here
would risk memory pressure and would not be a proportionate unattended test.
This is a concrete candidate for a CUDA machine, not a completed experiment.

## Reproduce

With an official NCNN bundle unpacked locally:

```bash
cargo run -p openshard-uofiles --bin openshard-font-upscale -- \
  --client "$OPENSHARD_CLIENT" \
  --upscaler font-upscale-artifacts/runners/realesrgan/realesrgan-ncnn-vulkan \
  --models font-upscale-artifacts/runners/realesrgan/models \
  --out font-upscale-artifacts/anime-4x-alpha128
```

Repack the existing model outputs with another threshold:

```bash
cargo run -p openshard-uofiles --bin openshard-font-upscale -- \
  --client "$OPENSHARD_CLIENT" \
  --upscaler font-upscale-artifacts/runners/realesrgan/realesrgan-ncnn-vulkan \
  --models font-upscale-artifacts/runners/realesrgan/models \
  --out font-upscale-artifacts/anime-4x-alpha128 \
  --alpha-threshold 96 \
  --skip-inference
```

Prepare an opaque black/white silhouette input per exported padded RGBA glyph:

```bash
./tools/font_contour_upscale.py prepare \
  font-upscale-artifacts/anime-4x-alpha128/glyphs-before \
  font-upscale-artifacts/contour-full/mask-inputs
```

Run the selected local NCNN model over `mask-inputs`, then attach its luminance
as alpha to nearest-scaled original RGB:

```bash
./tools/font_contour_upscale.py combine \
  --source-dir font-upscale-artifacts/anime-4x-alpha128/glyphs-before \
  --mask-dir font-upscale-artifacts/contour-full/mask-outputs \
  --out-dir font-upscale-artifacts/contour-full/glyphs-after \
  --scale 4 --rgb-filter nearest
```

`--threshold N` can make alpha binary immediately; without it, soft alpha stays
available for inspection and the MUL converter can apply its own threshold.
Output names and padded dimensions match normal `glyphs-after`, so the existing
converter can import the directory with `--skip-inference`. The tool was
syntax-checked and its eight-glyph output agrees with the independent
ImageMagick prototype to sub-byte luminance-rounding precision.

Validation run after implementation:

```text
cargo test -p openshard-uofiles --lib --bin openshard-font-upscale
135 library tests passed, 1 ignored; 3 converter tests passed.
```

## Plan

- [x] Document and validate the classic font container.
- [x] Add semantic MUL encoding and reject dimensions above 255.
- [x] Export transparency-safe RGBA glyph inputs with colour bleed and padding.
- [x] Run the first line-art model at 4x.
- [x] Repack a separate MUL and parse it back.
- [x] Render transparent before/after PNGs and a same-scale comparison.
- [x] Compare alpha thresholds 64, 96, 128, and 160.
- [x] Compare nearest-neighbour, two Real-ESRGAN models, animevideo, two waifu2x
  models, Real-CUGAN, and UltraSharp on one controlled glyph.
- [x] Test the proposed contrast-colour key and a cleaner dual-matte variant.
- [x] Run a representative mini-set: thin punctuation, narrow stroke, round
  hole, small coloured glyph, and a second face.
- [x] Run eight-background multi-matte on `A`, `!`, `O`, and a sparse glyph.
- [x] Reject RGB neural restoration because it invents unwanted texture.
- [x] Test neural silhouette-only alpha on `A` and eight stress glyphs.
- [x] Implement a reusable directory-level contour preparation/combiner.
- [x] Test word context with separate outer and fill masks.
- [x] Identify and correct the accidental white-strip word assembly artifact.
- [x] Preview generative ChatGPT image editing on tiny and enlarged references.
- [x] Rerun local word-context masks from the corrected word source.
- [x] Compare corrected word-context glyphs with the same glyphs processed alone.
- [x] Conclude that these local SR models do not use word-level semantic context.
- [x] Download and run the official font-specific DeepVecFont SR checkpoint.
- [x] Reject DeepVecFont SR: it preserves coarse stairs and adds small defects.
- [x] Compare a non-neural spline trace as a contour-only control.
- [ ] Test StarVector-1B image-to-SVG on a CUDA-capable machine; do not infer
  suitability from its benchmark alone.
- [ ] Run a direct image-edit API probe once an API key and billed-call approval
  are available; verify actual alpha before judging the picture.
- [ ] Pick the model and alpha threshold from that mini-set, then make the final
  full-font candidate.
- [ ] Resume atlas/runtime integration only after the asset decision.

## Next decision

The local word-context hypothesis is closed: it does not provide semantic text
understanding. The next decision is between (a) an exact-shape local contour
candidate, where animevideo and UltraSharp remain the least destructive, and
(b) a generative OpenAI edit that may make a cleaner new master but can redesign
glyphs. For (b), set an API key and explicitly approve a billed probe; test
native alpha first, then compare and split one word before any full-font work.
