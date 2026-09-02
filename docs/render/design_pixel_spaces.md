# Pixels: the inventory, and which grids share a divisor

A living plan, and its own session. The backlog at the end is where the next one
starts.

**All four phases are done, and the normative half of this page now lives in
[`docs/lighting_state.md`](README.md) § *The pixel spaces — the spec***:
the grids with their types, the six rules a person may assume, and the gate
holding each. This page stays as the derivation — the per-site census (P1), the
pair-by-pair commensurability table (P2), and what typing each space turned up
(P3). Read that one to write code; read this one to find out why a row says
what it says.

## The root

**Six grids meet in this renderer and no document lists them.** `docs/camera.md`
D11 names two — the real pixel and the virtual one — and that was the whole
argument it needed at the time. A frame has more than two, they meet inside
single expressions, and the conversions between them are written where they are
used rather than anywhere a person can read them in one sitting.

What that costs is on the record. `docs/parity.md`'s window-parity entry is one
defect, and its whole cause is that **two grids turned out to share a divisor
and nobody knew**: an odd viewport puts the world's centre on a pixel *centre*,
which makes the fragment grid commensurate with the world's integer grid, which
puts a sample exactly on a box's corner, which reaches a tie in
`impostor::meets` that has no right answer. Every step of that is documented in
its own file. The composition is documented nowhere, and the composition is the
bug.

A glossary would not have caught it. **A statement of which pairs are
commensurate would have.**

## The target

One page that answers, without opening a shader:

1. What grids exist, in what units, with what origin.
2. Every conversion between them: which are exact, which round, and which way.
3. **Which pairs share a divisor**, and under what parameters (zoom rung,
   viewport parity, eye fraction) — because a sample landing exactly on a
   discontinuity is the failure this whole plan exists to make predictable.

## What is already known

Collected while chasing the parity defect, and to be checked rather than
trusted — this is the starting list, not the answer.

| Grid | Unit | Type today | Notes |
|---|---|---|---|
| Real (screen) pixel | one physical pixel | none — bare `u32` in `ViewportRect`, `f32` in the shaders' `viewport.size` | what the compositor hands us; the quantum D11 chose |
| Virtual (world) pixel | one pixel of the world at `1:1` | [`WorldPixel`] `i32`, [`ViewPixel`] `i32`, [`WorldPoint`] `f64` | what the world is measured in; `Camera::render_width` counts these |
| Tile | `TILE_WIDTH` = `TILE_HEIGHT` = **44** virtual px | `Point` (`x`, `y`, `z`) | a step in `x` moves *half* a tile on each axis — 22 and 22 |
| `z` step | **4** virtual px (`Z_STEP`) | `i8` inside `Point` | the quantum the wire states a height in |
| Impostor tile space | `z` in **11ths of a tile** (`Z_PER_TILE` = `TILE_WIDTH / Z_STEP`) | bare `f32`/`vec3<f32>` | `impostor::meets`'s `lo`/`hi`; a second `z` unit, related to the first by a constant nobody carries in a type |
| Art texel | one texel of the art file | none | one virtual pixel at `1:1`, `Projection::scale` real pixels magnified |
| Clip space | −1..1 | `vec4<f32>` | the only grid nothing else is measured against |

And the parameters that decide commensurability:

- `Zoom::LADDER` = `[(1,2), (2,3), (3,4), (1,1), (2,1), (3,1), (4,1)]`, `1:1` at
  index 3. Magnifying rungs are whole on purpose (D11); minifying ones are not.
- The viewport's **parity**, per axis, which until `docs/parity.md`'s fix was
  the difference between "no sample is ever on a whole virtual pixel" and
  "every `scale`-th one is".
- The eye's own fraction (`Camera::projection`'s `self.eye.x - rounded.x`),
  which is a multiple of `1/scale` and therefore cannot itself make a
  half-integer whole — worth *stating*, since it is the kind of thing a reader
  assumes rather than checks.

## Phases

### P1 — the census ✅ 2026-08-10

Every conversion site, found rather than remembered: `project`/`unproject`,
`to_view`/`to_view_exact`/`to_world`/`to_screen`, `Projection`, the three
vertex stages' last line, `ray_from`, `light::Z_PER_TILE`'s readers,
`plan.rs`'s `scale`, the atlases' texel arithmetic. One row each: from, to,
exact or rounding, and which rounding.

*Done when:* the table above is filled from the code and not from this page,
and any row this page got wrong is corrected in place with the site that
proves it.

**Done.** Found by grep/Read across the workspace (`mcp__rust-code-mcp` was
indexed but grep on the exact names below was faster and just as complete).

| Site | From → to | Rounding |
|---|---|---|
| [`Camera::project`](../../crates/client/render/src/camera.rs#L200) | tile `Point` → `WorldPixel` | truncate (`as i32`); comment argues truncation == round here because every term stays under 2^24 in `f64` |
| [`Camera::project_exact`](../../crates/client/render/src/camera.rs#L216) | fractional tile `WorldSpot` → `WorldPoint` (`f64`) | exact, linear |
| [`Camera::unproject`](../../crates/client/render/src/camera.rs#L240) | `WorldPixel` + `z: i8` → tile `(i32, i32)` | nearest, via `div_euclid` after re-centring — not truncate-to-origin |
| [`Camera::to_view`](../../crates/client/render/src/camera.rs#L746) | `WorldPixel` → `ViewPixel` | exact int, relative to rounded `self.eye()` |
| [`Camera::to_view_exact`](../../crates/client/render/src/camera.rs#L758) | `WorldPoint` (`f64`) → view pixel (`f32`) | exact, but `eye` itself is the rounded one |
| [`Camera::to_world`](../../crates/client/render/src/camera.rs#L768) | `ViewPixel` → `WorldPixel` | exact int, inverse of `to_view` |
| [`Camera::to_screen`](../../crates/client/render/src/camera.rs#L778) | tile `Point` → `ViewPixel` | `project` + `to_view` composed, inherits `project`'s truncation |
| [`Camera::to_viewport`](../../crates/client/render/src/camera.rs#L790) / [`to_viewport_exact`](../../crates/client/render/src/camera.rs#L802) | view pixel → real viewport pixel (`f32`) | exact, scaled by `zoom.numerator()/zoom.denominator()` |
| [`Projection`](../../crates/client/render/src/camera.rs#L506-L517) | struct: `origin: Vec2`, `scale: f32` = real px per virtual px | — |
| [`Camera::projection`](../../crates/client/render/src/camera.rs#L704) | builds `Projection`; `origin` carries the eye's fractional remainder (`self.eye.x - rounded.x`) explicitly, because the same rounding must land bit-for-bit the same as `to_view`'s | — |
| Vertex stage last line — [`ground.wesl:237`](../../crates/client/render/src/shaders/ground.wesl#L237), [`statics.wesl:226`](../../crates/client/render/src/shaders/statics.wesl#L226), [`mesh_face.wesl:90`](../../crates/client/render/src/shaders/mesh_face.wesl#L90) | virtual/art pixel → real (viewport) pixel → clip space | `floor(viewport.size * 0.5)` — explicit floor, the fix `docs/parity.md`'s window-parity entry is about; then exact linear to NDC. All three shaders end on the identical line by design (comment: "must keep ending on it") |
| [`impostor::ray_from`](../../crates/client/render/src/impostor.rs#L82) | view-plane pixel offsets → tile-space point (`base` already in impostor tile space) | exact, no rounding |
| [`impostor::billboard_at`](../../crates/client/render/src/impostor.rs#L122) | same, billboard variant; `z` via `base - down * Z_PER_TILE / TILE_WIDTH` | exact |
| [`light::Z_PER_TILE`](../../crates/client/render/src/light.rs#L274) | defined as `(TILE_WIDTH / Z_STEP) as f32` = 11 | exact int division of constants |
| Readers of `Z_PER_TILE` | `Point.z` (`Z_STEP` units) ↔ impostor tile-space z | plain `f32` mul/div, no rounding at the site itself — [`impostor.rs:47,126,350`](../../crates/client/render/src/impostor.rs), [`light.rs`](../../crates/client/render/src/light.rs) (9 sites: L2275, L2384, L2489, L2534, L2669, L3092, L3331, L3796, L3800), plus offline oracle tools (`examples/oracle/pathtrace.rs`, `examples/synthetic_stair.rs`) and tests |
| [`plan.rs`'s `scale: u32`](../../crates/client/render/src/plan.rs#L76) | debug-plan pixels per tile — **not** `Projection::scale`, a different number with the same name | — |
| [`Picture::at`](../../crates/client/render/src/plan.rs#L110-L111) | fractional tile coord → plan pixel | truncate (`as i32`) |
| [`plan.rs:400`](../../crates/client/render/src/plan.rs#L400) | wall height (`Z_STEP` units) → plan pixels, via `/ Z_PER_TILE` | truncate (int division) |
| [`LandAtlas::region`](../../crates/client/render/src/atlas.rs#L415-L426) | atlas pixel origin → UV `Region` | exact, no half-texel inset |
| [`TexmapAtlas::pack`](../../crates/client/render/src/atlas.rs#L640-L648) | atlas pixel origin → UV `Region` | exact, **with** half-texel inset (ClassicUO's `CalculateHalfPixelUVs`) — texmap and land atlases disagree on this and both are correct for their own sampling mode |
| [`region_at`](../../crates/client/render/src/atlas.rs#L1896-L1904) (statics/gump atlas) | atlas pixel origin → UV `Region` | exact, no inset |
| [`ViewportRect`](../../crates/client/render/src/blit.rs#L39-L48) | struct, real screen px, `u32` | — |
| [`Camera::render_width`](../../crates/client/render/src/camera.rs#L664) | real viewport width → world/virtual pixel width, via `Zoom::world_pixels` | **ceiling** — the one primary conversion that rounds up rather than down/nearest |
| [`dump::read_rect`](../../crates/client/render/src/dump.rs#L138-L170) | real-pixel `ViewportRect` → byte layout, `COPY_BYTES_PER_ROW_ALIGNMENT`-padded rows | not a grid conversion per se, but the third place that owns the same rectangle (backlog item below) |

**Corrections to "What is already known" above, found while doing the
census:** none of the existing rows were wrong — `TILE_WIDTH = 44`
([`camera.rs:60`](../../crates/client/render/src/camera.rs#L60)), `Z_STEP = 4`
([`camera.rs:70`](../../crates/client/render/src/camera.rs#L70)) and
`Z_PER_TILE = TILE_WIDTH / Z_STEP` all check out as stated. One fact worth
adding: `TILE_WIDTH`/`Z_STEP` are **duplicated as independent local
constants**, not imported, in
[`impostor.rs:51`](../../crates/client/render/src/impostor.rs#L51),
[`facing.rs:224,291`](../../crates/client/render/src/facing.rs#L224) (deliberate,
per its own comment — decouples from the `camera` crate), and in two test
files. No `unproject_exact` exists — only the integer `unproject(at, z)`; the
asymmetry with `project`/`project_exact` is real, not a gap in this page.

### P2 — the commensurability statement ✅ 2026-08-10

For each pair of grids, under which `(rung, parity, fraction)` a point of one
can land exactly on a boundary of the other. This is the deliverable — the rest
is context for it.

*Done when:* the window-parity defect is **derivable** from the table, and the
table says out loud which other pairs are in the same position today.

**Done.** Six grids, fifteen pairs; most are exact by construction because the
constants that relate them are integers chosen to divide evenly. Only one pair
is exposed to sub-pixel sampling at all, and that is the one
[`docs/parity.md`](design_frame_assembly.md)'s window-parity entry is about.

| Pair | Commensurate when | Why |
|---|---|---|
| Tile ↔ World pixel | **always**, at every rung and parity | [`project`](../../crates/client/render/src/camera.rs#L200)/[`project_exact`](../../crates/client/render/src/camera.rs#L216) run before any camera, eye or zoom enters — `HALF_WIDTH = TILE_WIDTH / 2 = 22`, an exact integer, so a tile corner lands on a whole world pixel by construction. This is upstream of the ladder entirely; no `(rung, parity, fraction)` can touch it. |
| Tile `z` ↔ Impostor tile space | **always** | `Z_PER_TILE = TILE_WIDTH / Z_STEP = 44 / 4 = 11`, an exact integer division of two constants ([`light.rs:274`](../../crates/client/render/src/light.rs#L274)). One `Point.z` unit is exactly 11 impostor-space units; there is no rounding to lose. |
| **Fragment (view-plane pixel) ↔ Impostor tile space** | **never**, and that is the point | A fragment is a sample, not an area: [`ray_from`](../../crates/client/render/src/impostor.rs#L112) takes one virtual pixel of `across` to `(1, −1) / TILE_WIDTH` of a tile, so two adjacent samples are `SQRT_2 / TILE_WIDTH` apart in the space `impostor::meets` compares in, and an edge crossing between them is invisible to both. The pair therefore needs a *quantum*, not a rounding tolerance: [`impostor::FRAGMENT`](../../crates/client/render/src/impostor.rs#L94) is that step, and `Meeting::hit` is the one comparison that spends it. Sized wrong, this is visible — under the `1e-4` epsilon that preceded it, a floor's own seam row measured "outside its own box" and was drawn as a fragment with no measurement, which `blit.wesl` lights from every side: [`docs/silhouettes.md`](design_silhouettes.md)'s glowing grid. **Independent of the rung**: the world passes draw at the virtual resolution at every magnification, so a real pixel is `1 / scale` of a fragment and the fragment grid itself does not move. |
| World pixel ↔ View pixel | **always** | [`to_view`](../../crates/client/render/src/camera.rs#L746)/[`to_world`](../../crates/client/render/src/camera.rs#L768) are an exact integer translation — subtract `self.eye()` (already rounded to `WorldPixel`) and add `render_width()/2` (integer division, truncating). An integer lattice translated by an integer offset is still that lattice: no rung, parity or fraction can misalign these two, only shift which world pixel sits at view-pixel `(0,0)`. |
| World point (`f64`, sub-pixel) ↔ View pixel | commensurate **only** when the fractional part is itself zero | [`to_view_exact`](../../crates/client/render/src/camera.rs#L758) is the honest case: a body mid-step is *not* meant to land on a view-pixel boundary, and nothing downstream assumes it does. Not a defect — the one grid pair in this table that is supposed to disagree. |
| **View pixel (art/virtual) ↔ Real (viewport) pixel** | **magnifying rungs** (`scale` = 1, 2, 3, 4 — [`LADDER`](../../crates/client/render/src/camera.rs#L292) indices 3–6): commensurate **only** at an odd viewport extent, before the fix; **never**, at either parity, after it. Minifying rungs (`1/2`, `2/3`, `3/4`): not a point-sampling question at all — see below. | This is [`docs/parity.md`](design_frame_assembly.md)'s window-parity finding in full, restated in this table's terms. All three vertex stages end on `real = (pixel - origin) * scale + viewport.size * 0.5` ([`ground.wesl:237`](../../crates/client/render/src/shaders/ground.wesl#L237) and its two twins). A fragment samples at `i + 0.5`; at an even extent the world coordinate behind it is always a quarter-fraction of a virtual pixel, never whole — commensurate with *nothing*. At an odd extent, before the fix, `size * 0.5` lost its own half-pixel and the centring put a sample exactly on a whole virtual pixel every `scale`-th column: `i ≡ (scale - 1) (mod scale)`, in the exact numbers `docs/parity.md` derived at `4x` — `i ≡ 3 (mod 4)`. A box's own corner sits at a whole virtual pixel by construction (the Tile ↔ World-pixel row above), so this was the only way a primary ray ever passed exactly through one, which is what fed `impostor::meets`'s unresolved tie. The `floor(viewport.size * 0.5)` fix ([`docs/parity.md`](design_frame_assembly.md) §"Repaired where the sampling is") makes every sample sit at a half-integer over `scale` regardless of parity — no integer `scale` divides a half-integer, so this pair is now provably never commensurate at any magnifying rung, closing the case entirely rather than moving it. |
| View pixel ↔ Real pixel, **minifying rungs** | not applicable — no primary sample exists on this path | Below `1:1`, `Camera::minifies()` is true and [`Camera::projection`](../../crates/client/render/src/camera.rs#L704) returns `scale: 1.0`: the world is drawn 1:1 into an oversized image and the *blit's linear sampler* shrinks it ([`camera.rs:686-701`](../../crates/client/render/src/camera.rs#L686-L701)). A linear filter blends across whatever pixels it lands between; there is no point-sample tie to land exactly on a boundary, so the whole commensurability question this page exists to ask does not arise on this path. Worth stating rather than leaving silent, since it looks like the same kind of pair as the row above and is not. |
| Real (viewport) pixel ↔ `Zoom::LADDER` rung | **always inexact except at `1:1`, at magnifying rungs `2/1`–`4/1`** — `render_width = viewport.div_ceil(num) * den` [`camera.rs:337-343`](../../crates/client/render/src/camera.rs#L337-L343) rounds **up**, so a viewport not a multiple of `num` spills a fractional world-pixel column past the edge, clipped. This is a boundary-rounding fact about `render_width` itself, independent of the sampling row above and upstream of it. | Stated because it decides which viewport widths make `render_width()` odd or even — the exact knob the window-parity defect turns on. `render_width` is odd only for specific `(viewport mod num)` residues at each rung; the parity row above is this row's consequence, not a separate coincidence. |
| Art texel ↔ atlas UV region | **inconsistent by atlas, not by rung** | [`LandAtlas::region`](../../crates/client/render/src/atlas.rs#L415-L426) and [`region_at`](../../crates/client/render/src/atlas.rs#L1896-L1904) (statics/gump) divide exactly, no inset; [`TexmapAtlas::pack`](../../crates/client/render/src/atlas.rs#L640-L648) insets by half a texel on every side (ClassicUO's `CalculateHalfPixelUVs`). Both are internally exact — a `Region`'s own corners always land exactly where the code says — but the *convention differs between atlases*, which is a hazard of the same shape as a rung dependency (two callers assuming one rule) even though it has nothing to do with zoom or viewport parity. Flagged rather than merged into the rows above because P3 (below) has to give both conventions a type, not just the one this page started from. |
| Everything ↔ Clip space | **always exact**, linear | The NDC line after the sampled `real` — `real.x/viewport.size.x*2.0-1.0` — is an exact affine map of whatever `real` already is; clip space introduces no rounding of its own. Any commensurability question about this grid reduces to the row that produced `real`. |

**The window-parity defect, derived:** Tile↔WorldPixel is exact (row 1), so a
box's corner is *always* on a whole world pixel. WorldPixel↔ViewPixel is an
exact integer shift (row 3), so that corner is *always* on a whole view pixel
too. The only place a rounding choice enters at all is View pixel↔Real pixel
at a magnifying rung (row 5) — and before the fix, an odd viewport extent made
that one grid pair commensurate on a residue class of columns, which is
exactly the eleven-columns-per-tile pattern `docs/parity.md` measured. Every
other pair in this table was never a candidate, which is the fact P1 makes
checkable rather than argued.

### P3 — the types that are missing ✅ 2026-08-10

The real pixel has no type; the art texel has no type; impostor tile space has
no type and carries a `z` in different units from every other `z` in the
engine. `docs/style.md`'s own newtype rule applies, and the reason to do it
here rather than as a sweep is that P2 will have just named which confusions are
*expressible* — a newtype is worth its cost exactly where two domains meet.

*Done when:* each grid P2 shows can collide has a type that stops the collision
at compile time, or a written reason it does not.

**Done.** All three, and the first of them corrected what this phase thought the
third one was. The art texel is the one grid left untyped, with the reason
written out below — which is what this phase's *done when* asks for, not an
exception to it.

#### The correction: there is no "impostor tile space"

P1's table has a row reading *"Impostor tile space — `z` in 11ths of a tile — a
second `z` unit, related to the first by a constant nobody carries in a type"*.
**That row names the wrong space.** `impostor::VIEW` is `(1, 1, Z_PER_TILE)`,
which means one unit of the impostor's `z` is one unit of `Point.z` — the *same*
quantum the wire states a height in, four virtual pixels, not an eleventh of
anything. `Volume::lo`/`hi`, `ray_from`'s output and `Spot::z` are all in it.
The impostor has no `z` unit of its own; what it has that `Point` does not is
`x` and `y` in **tiles**, and that combination already had a name and a type —
[`WorldSpot`](../../crates/client/render/src/camera.rs#L167).

The second space is one file over and P1 walked past it: **`light.rs`'s tile
space**, where `z` *is* divided by `Z_PER_TILE` so that all three axes share a
unit and a length means something. Every metric the lighting model states lives
there — a distance, a cosine, a beam's axis, a surface's normal — because a
falloff measured with `z` in its own units reaches eleven times as far up as it
does sideways. It was named only in prose, in five separate doc comments, and
carried as `[f32; 3]`.

So the two spaces that actually collide are **world units** (positions) and
**tile space** (metrics), they are one multiplication apart, and both were the
same bare `[f32; 3]`. They met inside single expressions: `flame_points` added a
tile-space offset to a world-units centre, `walk_sun` turned a tile-space
direction into a world-units step, `arrival` fed a hand-written difference to
both `lit_from` and `Beam::lights`. Nothing but the reader told them apart.

#### Done — [`light::TileVec`](../../crates/client/render/src/light.rs#L272)

A newtype for tile space, with [`TileVec::between`] (two world-units points → a
tile-space offset) and [`TileVec::in_world_units`] as its **only two crossings**.
`Z_PER_TILE` now appears in a metric expression exactly twice, in those two
methods, rather than at the eight sites that each had to remember it:
`sample_with`'s cull offset, `walk_sun`'s and `walk_sun_exact`'s step (which
were two copies of one conversion), `flame_points`'s disc and its point, and
`arrival`'s `toward`, plus `impostor::meets`'s `outside`. `Beam::toward`,
`Sun::toward` and `Surface::normal` are typed at the field, so a world-units
vector can no longer be handed to `lit_from` or `Beam::lights` at all.

Two things it deliberately does not do. It has **no normaliser**: the three
places that normalise here guard a different epsilon each (`lit_from` at zero,
`Beam::lights` and `flame_points` at `1e-6`), and folding them into one method
would change three answers to make one type tidier. And `lit_from`'s cosine
stays *written out*, `n.x*t.x/L + n.y*t.y/L + n.z*t.z/L`, rather than going
through `TileVec::dot`: `(n·t)/L`, `n·(t/L)` and that are three roundings of one
number, the shader writes this one, and a cosine landing either side of zero is
a lit pixel or a black one. `scaled` and `divided` are separate methods for the
same reason. The newtype is unwrapped, via `axes()`, at exactly one place — the
uniform `blit.rs` writes.

#### Done — [`camera::RealPoint`](../../crates/client/render/src/camera.rs#L177)

The real pixel, on the side where two grids met in one expression:
`to_viewport_exact` *takes* a fractional view pixel and *returns* a real one,
both were `Vec2`, so feeding it its own output compiled and applied the zoom
twice. It now returns a `RealPoint`, and so do `to_viewport`, `tile_facet`,
`tile_diamond`, `Projection::centre`, `Solid::faces` and `Solid::outline` — the
whole run from the camera to the painter, since every one of those answers on
the display's grid. `solids.rs`'s vertex writer takes one too, which is where
the value stops: a `RealPoint` is unwrapped at the buffer, and nowhere before
it. No `From` in either direction; a `Camera` is the only thing that crosses.

The module doc's claim that *"the one place a real pixel enters is the cursor…
that is why the third space has no type: nothing carries it"* is rewritten
rather than deleted — a paragraph that outlived its fact is worth saying so.

**What typing it turned up:** `Projection::one_to_one` built its virtual
`origin` out of `Projection::centre`, which answers in real pixels. The types
refused it, and the refusal is correct *and* the code was right — 1:1 is exactly
the camera where the two grids are the same grid. The halving is now
[`half_extent`](../../crates/client/render/src/camera.rs#L560), deliberately
spaceless: one rounding, three callers (`centre` in real pixels,
`one_to_one` and `Camera::projection` in virtual ones), each naming its own
space in what it hands back. `Camera::projection` had a fourth copy of that
`/ 2` written out; it reads the shared one now. Gated by
[`one_extent_is_halved_once`](../../crates/client/render/src/camera.rs#L1260),
which also pins the property `Camera::projection`'s comment argues at length and
nothing asserted: at 1:1 `to_view` puts the eye exactly on `projection().origin`,
at odd extents included.

#### Done — [`camera::ViewPoint`](../../crates/client/render/src/camera.rs#L161)

The other half of that pair, and the sweep it needed turned out to be fifteen
sites rather than the whole sprite path. `to_view_exact` and `Projection::origin`
answer in it; `to_viewport_exact` **takes** it, so both ends of the one crossing
this camera performs now name their own grid and the zoom cannot be applied
twice in either direction. Then outward through what that value is:
`statics::stand_on`, `statics::on_screen`, `Placed::at` — which `crate::items`
shares, being the same picture standing the same way — `mobiles::cell_centre`
and `MeshFaceVertex::screen`, whose doc already had to say *"in
`Camera::to_view_exact`'s space"* in prose because nothing said it in types.
[`ViewPoint::of`](../../crates/client/render/src/camera.rs#L192) widens a whole
`ViewPixel`, which is not a crossing: same grid, said to a fraction.

**What typing it turned up.** `debug.rs`'s `middle` was fed to the sweep as a
view point and the compiler refused it — correctly, and the *code* was right:
that `Vec2` is a **tile-space position**, `Vec2::new(x + 0.5, y + 0.5)` being the
middle of tile `(x, y)` on its way to `light::Spot::at`. It is the backlog's
seventh-meaning item, met head-on: the refusal is the first time anything in the
build has distinguished those two spaces, and it took one wrong annotation to
find. Reverted, and left as `Vec2` until that space gets its own name — the
point is that the reader was the only check and now is not.

`stand_on`'s doc said its answer was *"in viewport pixels"*. It never was, for
as long as the answer was a bare `Vec2` and nothing could tell; corrected in
place rather than deleted.

#### Still open

- **`geometry::Rect`.** A sprite's rectangle is a `ViewPoint` and an extent —
  but the same `Rect` is also an atlas rectangle (`text.rs`, `sprite.rs`), a
  gump's place on the surface, and a plan pixel's. Three spaces sharing one
  shape, which is not the crossing P3 set out to stop and does not have the same
  answer: a `Rect<Space>` costs a parameter at every one of those callers to
  refuse a confusion none of them has yet made. Named here so the next reader
  does not rediscover it as a gap.
- **The art texel.** Left untyped on purpose for now. It is real (`Region`'s UV
  arithmetic, `Projection::scale`, every atlas rectangle), and P2's row about it
  found a genuine hazard — the land and statics atlases divide exactly while
  `TexmapAtlas` insets by half a texel — but the confusion is *between two
  conventions of one grid*, not between two grids, so a newtype over the texel
  does not stop it; what would is a type carrying the convention. That belongs
  with [`docs/silhouettes.md`](design_silhouettes.md), which is entirely about this
  grid, rather than being invented here first.

### P4 — the gates ✅ 2026-08-10

An invariant of the form "no primary sample lands on a whole virtual pixel at
any rung, at either parity, at any eye fraction" is a unit test with no GPU in
it: a loop over the ladder and a divisibility assertion. `docs/parity.md`'s fix
is currently held by an argument in a comment; this is where it becomes a gate
that a mutation turns red.

**Done, and the headline gate was already standing.**
[`camera::tests::no_primary_sample_lands_on_a_whole_virtual_pixel`](../../crates/client/render/src/camera.rs#L1148)
is exactly the test this phase describes — it landed as `docs/parity.md` P5's
G1, walks all seven rungs × both parities of both axes × every eye fraction the
quantum can express, asserts the *distance* (`0.5 / scale`, the property) rather
than the absence, counts what it looked at, and carries
[`AN_EYE_ON_A_HALF_PIXEL_REACHES_THE_CORNER`](../../crates/client/render/src/camera.rs#L1116)
as a named exception with a hit-counter so the list cannot quietly cover
nothing. Nothing to add there.

What P2's table left held by argument alone was its *other* rows — the ones that
say "commensurate **always**, by construction". Each of those is a claim about a
constant, and this renderer writes its grid constants down more than once.

| Claim | Gate | Where |
|---|---|---|
| A tile step is a whole number of world pixels (`TILE_WIDTH / 2` exact) — P2 row 1, and therefore the reason a whole virtual pixel *is* a box's corner | [`a_tile_step_is_a_whole_number_of_world_pixels`](../../crates/client/render/tests/grids.rs) | new `tests/grids.rs` |
| One `Point.z` unit is a whole count of tile-space units (`TILE_WIDTH % Z_STEP == 0`) — P2 row 2 | [`a_height_unit_is_a_whole_number_of_tile_space_units`](../../crates/client/render/tests/grids.rs) | same |
| The **shaders'** copies of `TILE_WIDTH`, `Z_PER_TILE`, `Z_STEP` and `HALF_TILE_HEIGHT` are the camera's numbers | [`the_shaders_restate_the_cameras_constants_and_not_their_own`](../../crates/client/render/tests/grids.rs) — reads them back out of `impostor.wesl` / `statics.wesl`'s own source | same |
| `facing.rs`'s deliberately-independent `Z_STEP`, and that its `HALF_TILE_WIDTH` doubles back exactly | [`facing::tests::a_tile_is_the_width_the_camera_draws_one_at`](../../crates/client/render/src/facing.rs#L2138) — its `TILE_WIDTH` was pinned; the other two were not | extended in place |

The shader pins are the ones that were load-bearing and absent: a copy across
the wire has no compiler on either side of it, and a disagreement there does not
fail to build and does not fail to draw — it draws a frame at a different scale
from the one every test on this side asserts about, which is `docs/parity.md`'s
"two pictures rather than one wrong one" exactly. `shader_const` **panics** on a
missing name rather than answering `None`: a renamed constant has not stopped
being a copy, and a helper that shrugged would let the rename read as "nothing
to pin".

*Witnessed by mutation:* `impostor.wesl`'s `TILE_WIDTH` set to `45.0` turns
`the_shaders_restate_the_cameras_constants_and_not_their_own` red, reverted
after.

**Not gated, deliberately.** The atlases' two UV conventions (P2's art-texel
row) — `atlas.rs` already pins the half-texel inset at its own site
([`atlas.rs:2030`](../../crates/client/render/src/atlas.rs#L2030), and the
round-trip below it), and what is *un*gated there is not a number but the
absence of a type carrying which convention a caller is in, which is P3's open
item and `docs/silhouettes.md`'s subject. A test cannot stand in for it.

## Backlog

- 🚩 **The art texel is the one grid with no representation anywhere.** It is
  implicit in every atlas rectangle and in `Projection::scale`, and it is the
  grid `docs/silhouettes.md` is entirely about.
- ✅ **`Z_STEP` and `Z_PER_TILE` are one relationship written twice — resolved
  2026-08-10, by [`light::WorldVec`](../../crates/client/render/src/light.rs).**
  A reader meeting `lo.z`/`hi.z` in the impostor had no way to know which of
  the two `z` units they are in without following the definition — the same
  collision `TileVec` (P3) already fixed one side of. `WorldVec` is `TileVec`'s
  sibling for the *other* space `light.rs`'s own module doc already named in
  prose: `x`/`y` in tiles, `z` in the map's own height units. Threaded through
  `impostor::Volume::lo/hi`, `Meeting::at`/`normal`, `VIEW`, `ray_from`,
  `billboard_at`, `meets`, `nearest`, and `TileVec::between`/`in_world_units`'s
  two crossings — every site P1's census had already found reading `[f32; 3]`
  in this space.

  **`meets`'s own slab test stays array-indexed inside its body, deliberately.**
  It picks an axis (`0`/`1`/`2`) at runtime — `for a in [1, 0]`, a dynamic
  `axis` — which is exactly why `[f32; 3]` was right there and is the one
  place in the module a named `{x, y, z}` struct would fight the algorithm
  rather than clarify it. `WorldVec::array`/`from_array` (`TileVec::axes`'s own
  pattern, one more time) convert at the function's edges — the *signature* is
  typed, the twelve-line body inside it is untouched. No `Index`/`Deref`: the
  house style's reason against `Deref` on newtypes applies the same way to
  `Index` here, and the escape hatch already had a name.
- ✅ **The whole real pixel — the one the cursor arrives on — resolved
  2026-08-10, by [`camera::RealPixel`](../../crates/client/render/src/camera.rs).**
  `RealPoint` was the fraction; `Camera::pick(x: i32, y: i32)` was the pair a
  caller holding a `ViewPixel` could pass by mistake and have it compile —
  exactly the shape `WorldPixel`/`ViewPixel` exist to refuse, and it was not
  only `pick`'s: `mobiles::pick`, `items::pick` and `statics::pick` each took
  the identical bare `cursor: (i32, i32)`, four sites repeating one collision.
  `RealPixel` is now what all four take, what `Camera::zoom_about` anchors on,
  and what `Control` carries as its own cursor — `Control::cursor()`,
  `Control::cursor_moved`, `Drag::cursor`. The app crate's
  `WindowEvent::CursorMoved` handler builds one `RealPixel` from the physical
  position once, and everything downstream — panning, zooming, `ask_to_cursor`,
  `pick_tile`, the three pass-level `pick`s — takes it from there rather than
  re-deriving a pair of `i32`s at each site, which is the event-handling sweep
  the previous note asked for.

  **`Camera::width`/`height`, `image_size` and `ViewportRect` stay bare
  integers, deliberately.** They are extents, not points — a count of real
  pixels rather than a position in real-pixel space — and nothing in this
  crate confuses an extent with the point space it bounds; a `ViewPixel` was
  never at risk of being passed where a width is expected, or the reverse.
  Giving them `RealPixel`'s type would conflate the two kinds of quantity this
  page has kept apart everywhere else (a tile is a size, `WorldSpot` is a
  point, and neither borrows the other's type). Same reasoning P3 already
  wrote down for `geometry::Rect`: a shape shared by several spaces is a
  different problem from two spaces sharing one number, and solving it here
  would be inventing the fix before the problem is the one in front of it.
- 🚩 **`geometry::Vec2` means a *tile-space* position in `light.rs`.** `Light.at`
  and `Spot::at` are `x`, `y` in tiles — `Vec2::new(100.5, 100.5)` is the middle
  of tile 100,100, not a pixel of anything. That is a seventh meaning for the
  type, sitting one file away from `TileVec`, which was introduced in this same
  phase for tile-space *offsets*. The two are the same space's point and vector;
  only one of them has a name. **Sharpened by `ViewPoint`:** the sweep annotated
  `debug.rs`'s tile-space `middle` as a view point and the compiler caught it —
  the first time the build has told those two apart, and evidence the confusion
  is reachable rather than theoretical. A `TilePoint` beside `TileVec` is what
  closes it.
- ✅ **What a `ViewportRect` is measured in when a docked panel has moved it —
  resolved 2026-08-10, by documentation and a gate, no type.** Traced all three
  owners. `Shell::viewport()` (`crates/client/app/src/shell.rs:585-630`) is the
  only production site that ever sets a non-zero origin, and its own doc
  comment already says the right thing: a docked panel shrinks the rect but
  never re-bases it, so `x`/`y` are **window-absolute** physical pixels both
  before and after. `Blit::render` (`blit.rs:659-665`) sets that rect as the
  GPU viewport transform against `target`, which is always the surface — same
  convention. `dump::read_rect` (`dump.rs:138-215`) has no opinion of its own:
  it is a raw offset into whatever `wgpu::Texture` it is handed, window-sized
  or not — its contract is "honour `rect` against `texture`," full stop.
  `Camera` (`camera.rs:685-711`, `control.rs:157-159`) stores no origin at
  all; `lib.rs`'s resize drops `viewport.x`/`.y` on the floor deliberately,
  because the camera's own projection is viewport-local and 0-based —
  `solids::on_screen` reads only `rect.width`/`.height`, never `.x`/`.y`,
  which is the build's own proof that nothing downstream expects an origin
  from the camera.

  So the three never needed to agree on a *type* — they agree on a
  *convention* (`ViewportRect.x/y` is always window-absolute; `Camera` never
  claims to know it), and the risk was that the convention held only where
  every real call site happened to reuse one value, with no test that would
  fail if a caller sent the blit and the readback to unrelated origins. That
  gap is now closed by
  [`a_docked_panels_offset_places_the_same_picture_it_shows_at_the_corner`](../../crates/client/render/tests/dump.rs)
  — the same drawn frame blit once at `(0, 0)` into a target its own size, and
  once at a corner into a bigger "window" texture the way a docked panel
  leaves one, then read back and compared byte for byte. It is the missing
  half of `a_readback_off_the_corner_is_the_same_pixels_shifted`, which only
  ever exercised `read_rect`'s own arithmetic against one texture read twice
  and never called `Blit::render` with a non-zero origin at all.
