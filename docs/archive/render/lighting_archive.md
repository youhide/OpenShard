# Lighting: session log, decision reasoning, and backlog archive

Companion to [`lighting.md`](lighting.md) — that file is the current-state
reference (what the pass computes today, its data formats, its measured
costs, its known compromises); this file is the full reasoning behind it:
every decision's argued case and rejected alternatives, every struck-through
withdrawn passage with its own reasoning, the session-by-session narrative
that produced the design, and the complete backlog (including everything
already fixed, kept for the reasoning rather than only the outcome).

Nothing below was rewritten for style — it is `lighting.md`'s old body,
relocated and grouped under headings that mirror the current file's
sections, with decision and step numbers kept exactly as they were so a
reference to "decision 24" or "step 23.1" from another document still
resolves to real text. Where a topic doesn't map cleanly onto one heading
below, the full passage lives under the section its main subject most
belongs to, with a note left for readers arriving from an adjacent topic.

Read [`lighting.md`](lighting.md) first for what is actually true today.
Come here for *why* — what was tried and abandoned, what a report from a
live screenshot actually said, and the numbers behind a claim that changed.

## Overview: what this pass replaced

*(originally `lighting.md`'s "Where it stands" section, and decision 21's
correction to it)*

Every light used to be a **circle in the pixels of the drawn image** — the
flame's tile projected to a screen point, compared against the fragment's
screen position. That arrangement cannot be given walls, for two reasons:

- **The screen folds height into `y`.** A brazier in a cellar and a lantern
  on the street above it are a few pixels apart in the image, so the pool of
  one covers the other. This was `client.md`'s "a flame lights through a
  floor".
- **A wall's sprite stands above the tile it occludes from.** A wall is 44
  pixels of picture rising from a diamond that is at the floor. Whatever
  screen-space mask darkens the ground behind the wall also covers the
  wall's own face — including the face *turned towards the flame*, which is
  the one surface that must obviously be lit. There is no shadow polygon
  that fixes this, because in the image the lit face and the shadowed
  ground are the same pixels of the same sprite; only a per-pixel answer to
  "which tile is this?" separates them.

So the pass moved from the screen into the world, and the shadow came with
it.

**Decision 21: the screen-space glow is a *second layer*, not a thing that
was replaced.** This pass began as a circle in the pixels of the drawn
image — the flame's tile projected to a screen point, compared against the
fragment's screen position — and the passage above is written as though
moving into the world had *replaced* it. That framing is wrong, and it is
worth correcting: the two are different things and a lit frame wants both.

- **The world layer** — everything decisions 1 to 20 are about — answers
  *which surfaces are lit*. It is a multiplier on the art, it knows about
  walls and heights and storeys, and it is what makes a torch inside a
  house not light the street. It cannot draw the flame itself: nothing in
  this renderer draws air, and a fire's own brightness is not a property of
  the ground under it.
- **The screen layer** is the *glare*: a soft radial falloff centred on the
  flame's own sprite, added over the finished picture. It is what the
  reference client draws (`light.mul` sprites blended over the scene) and
  it is the thing a person actually recognises as "a lamp" — the halo
  around the source, which is in the eye and in the air rather than on any
  surface. It was working, and it was the circle the complaint above
  remembers.

The two failures they have are opposite, which is the whole argument for
keeping both. A screen circle alone lights through walls and folds a cellar
into the street — the two reasons this pass moved into the world. A world
multiplier alone has no source in it: the brightest thing in the frame is a
patch of floor, and the flame is a sprite the same brightness as it was in
daylight.

Composed, and in this order: the world layer multiplies the art, and the
glow is added on top of the result. Multiplying by the glow would tint
whatever happens to be drawn there and a black pixel would stay black,
which is exactly what a halo must not do — a lamp glares over the dark
doorway behind it.

What the glow needs that the world layer already has: the flame's *screen*
position, which is the sprite's, not the tile's — `light.rs` places a light
by its tile and the backlog has carried that since the beginning; here it
is the whole point, because a halo half a tile from the burning sprite
reads as a mistake rather than as light. See "Point lights" below for the
full glow design and why it is not yet built.

**Decision 1 (as written). Lighting is computed in world coordinates, not
in screen pixels.** A fragment is lit according to the tile and height of
*the thing drawn there*, not according to where that thing landed in the
image. This is what makes a wall's face lit as the wall's own tile is lit,
and it is what lets a storey below stay dark while the street is not.

### Steps 1–6: the first working pass

- [x] **Step 1. `render/src/occlusion.rs`.** See "The occluding world"
      archive for the full text.
- [x] **Step 2. The `(x, y, z)` attachment.** See "The G-buffer bridge"
      archive for the full text.
- [x] **Step 3. `light.rs` in world coordinates.** See "Point lights"
      archive for the full text.
- [x] **Step 4. `blit.wgsl`.** See "The shadow ray walk" archive for the
      full text.
- [x] **Step 5. Wiring.** `app/src/lib.rs` carries the place attachment
      through the three passes and into the blit; `light::collect` builds
      the grid itself, so no call site grew an argument.
- [x] **Step 6. A picture, and a number.** See "Point lights" archive,
      "found while measuring it", for the full cost breakdown.

## Origin narrative: the mesh-coverage bug, the shadow-raymarch anomaly, and the stair-corner sessions

This section is the chronological session log that used to sit at the top
of `lighting.md`, above the numbered decisions — kept intact because it is
where several still-live design facts (the corner stance, the mounted-flame
fix, the mesh-face tile-attachment fix) were actually found, in the order
they were found, including dead ends. The live continuation of the
mesh/shadow-boundary thread below is
[`lighting_raymarch.md`](lighting_raymarch.md) and
[`lighting_raymarch_archive.md`](lighting_raymarch_archive.md) — this
section is the *origin* of that track, not its current state.

### The shadow-raymarch boundary track split off

**The shadow-raymarch boundary-correctness track split out to its own
document:** [`lighting_raymarch.md`](lighting_raymarch.md). Opened because
the entry below's own thread ("Fixed: the shadow-raymarch anomaly...")
found the same class of bug twice — once on the GPU side
(`mesh_face.wgsl`'s `fract()`), once still open on the CPU side
(`light.rs`'s `walk_cells`/`sample`) — and left a second, unrelated shape
unexplained. Enough sessions' worth of work (fixes, an oracle, tooling)
that it earned its own living plan rather than staying inside this file's
"next session" note.

### A new tool: `synthetic_stair.rs`

**A new tool exists for exactly this class of bug:
[`examples/synthetic_stair.rs`](../../../crates/client/render/examples/synthetic_stair.rs)**
— a climbable static, alone, with no client files, no map and no art: one
hand-built [`facing::Prism`](../../../crates/client/render/src/facing.rs), its
own [`occlusion::Occlusion`](../../../crates/client/render/src/occlusion.rs)
built the same way `light.rs`'s own
`a_treads_top_is_not_shadowed_by_its_own_riser` test builds one, and one
flame, run through the real `GroundRenderer`/`MeshFaceRenderer`/`Blit`
pipeline and dumped as a picture. Built while chasing this section's own
backlog, because a real screenshot has a lamppost, a texture and a second
static in it, and none of those help decide whether a shape in
`View::Shadow` is the bug or the scene. Its own doc comment has the
environment variables; the short version:

```sh
OPENSHARD_FRAME_VIEW=7 OPENSHARD_FRAME_DUMP=/tmp/stair.ppm \
    cargo run --release -p openshard-client-render --example synthetic_stair
```

Three things learned building it, all now load-bearing on the tool's own
defaults:

- **A stair's tread heights are *absolute* from the static's own base at `z
  0`, not relative to the real screenshot's numbers.** The real tread this
  whole track started from reads `z 11, 13, 15` because it stands on a `z
  10` platform; handed straight to `Prism::new` with a base at `z 0`, those
  same three numbers build a rise five times taller than the real one, and
  every proportion in the picture reads wrong. The tool's own default is
  `1,3,5`, matching `light.rs`'s own fixtures.
- **A flame held level with a tread reads the tile as wide open — on
  purpose, not a bug to route around.** `Surface::shadowed_by_own_tile`'s
  exemption (decision 32) means a light at a tread's own height sees past
  every riser on its own tile, so a same-height light is exactly the wrong
  fixture for looking at a shadow: it shows nothing standing between the
  flame and anything. A ground-level flame instead reads almost the whole
  face black, which answers "is anything blocked" but not "how does the
  shadow vary across the face" — no single light height and offset makes
  both a wall and a floor. `OPENSHARD_LIGHT_AT=2.5,1.0`,
  `OPENSHARD_LIGHT_Z=2` (the tool's default) is the least degenerate
  compromise found: the nearer tread lit, the far one in its own riser's
  shadow, an actual line between the two.
- **A thin, nearly-tangent lit strip inside a mostly-shadowed face can
  look, at a coarse nearest-neighbour zoom, like a second shape
  disconnected from the first — it is not, and the way to tell is reading
  raw pixels, not looking harder.** Sampled a suspicious sliver at 3x zoom
  against `View::Kind`'s silhouette pixel for pixel rather than trusting
  the eye a second time: every pixel of it sat inside the mesh's own
  outline, just interleaved with fully-shadowed (`through = 0`,
  indistinguishable from the black background at a glance) pixels a few
  texels away. One real background-side anomaly already exists in this
  track (below) — this was not a second one, and the method that ruled it
  out is worth reusing before writing up a new one.

**Still not caught with the new tool: the CPU-only `light::sample`/
`walk_cells` `floor()` bug below.** Several `OPENSHARD_LIGHT_AT`/`_Z`
combinations were tried against `synthetic_stair`'s `View::Shadow`, looking
for the same before/after contrast the real screenshot's diff showed
(2,948 pixels flipped from a false "fully open" to a correct shadow) —
none of them reproduced a clean, attributable difference between the fixed
and the pre-fix `mesh_face.wgsl` formula on this synthetic geometry. That
does not mean the CPU-side bug is smaller than it looked; it means the
specific light placement that makes the reconstructed-position error
*change the occlusion answer* (not just the number) was not found by hand
in the time spent. Next session: now that the tool exists and the two
working light configurations above are known-good starting points, either
bisect `OPENSHARD_LIGHT_AT`/`_Z` systematically from `2.5,1.0`/`2` rather
than by guessing, or go back to `light.rs:1681`'s `walk_cells` directly
with the profiler this track's own earlier entry already names, which
pinned the CPU bug's existence without needing a picture at all.

### Fixed: the shadow-raymarch anomaly (the mesh-coverage half)

**Fixed: the shadow-raymarch anomaly, and it was one tile of arithmetic on
the wrong side of a `fract()`, not `walk_cells`' own `floor()`.** The chain
below found the real mechanism before touching anything, which is why the
first fix that would have "worked" (nudging the origin a hair back along
the ray) got written down and rejected instead of shipped — see the
counter-example a few paragraphs in, kept for the next time this shape of
bug shows up somewhere the counter-example does not apply.

Once the CPU/GPU parity question below was actually asked of the GPU side
— which the entry originally left as "unconfirmed" — it turned out
`blit.wgsl`'s `walk()` was never the culprit: it reconstructs its ray's
start from a `(tile, sub)` pair, not a bare float, precisely so `floor()`
can't misfire, and `tile` is the mesh face's *own authored* tile, not
re-derived from anything. The bug was one step upstream, in how `sub` got
built in the first place — `mesh_face.wgsl`'s `fs_main`:

```wgsl
let sub = clamp(fract(in.world.xy), vec2<f32>(0.0), vec2<f32>(INSIDE));
```

`fract()` of an exact whole number is `0.0`, not `1.0` — and this tread's
own outer corner sits at exactly `world.x = 1498.0`, a whole number,
because [`Prism::footprint`](../../../crates/client/render/src/facing.rs) holds
every stair's tile-crossing edge at the tile's own unit square. The
`INSIDE = 126/127` constant this crate already had (`scene.rs:868`,
documenting `statics.wgsl`'s own copy) already named this exact class of
hazard for an ordinary wall's face — "a fraction of exactly one lands in
the next tile" — but a mesh face's `sub` was computed from `fract()` alone,
with no access to which tile it actually belonged to, so it could not
apply the same guard the wall path already had. A fragment on this tread's
own far edge read `sub = 0.0`, reconstructed as sitting on the *next* tile
over, and the walk from there never crossed the riser that shadows every
other point on the same face.

**The fix:** give `mesh_face.wgsl` the tile it already knows CPU-side,
instead of asking `fract()` to guess it back from a position that can
legitimately sit on the tile's own far edge.
[`MeshFaceVertex`](../../../crates/client/render/src/mesh_face.rs) grew a `tile:
[f32; 2]` field — [`push_mesh`](../../../crates/client/render/src/statics.rs)
was already carrying `at.x`/`at.y` right where it builds the vertex, so
nothing upstream had to change to supply it — carried through as a new
flat vertex attribute (`mesh_face.wgsl`'s `VertexOut::tile`), and `fs_main`
now computes `sub = clamp(in.world.xy - in.tile, 0.0, INSIDE)` instead of
flooring `world` at all. Verified by re-rendering the exact `View::Shadow`
picture below: the isolated white dash on the tread's face is gone; the
second shape (the white line over empty background) is untouched, which is
the expected result of a fix aimed at the first and not the second — see
below.
[`a_stair_s_mesh_vertices_carry_their_tile_and_reach_its_far_edge`](../../../crates/client/render/src/statics.rs)
pins the fact the fix leans on: every mesh face carries the tile it stands
on, and a stair's own footprint reaches at least as far as that tile's far
edge, not short of it.

**Still open at the time: `light::sample`/`walk_cells`'s own `floor()`
(`light.rs:1681`) has the same class of bug, unfixed, on the CPU-only
path** — the one `isolated_scene`'s profiler exercises, and the one
anything using `light::sample` directly for a diagnostic or a future
gameplay query would inherit. It could not be reached through the render
pipeline itself, which is why fixing `mesh_face.wgsl` alone closed the
live-screenshot defect, but it is real: a bare `Spot` built from a raw
float still has no way to know which tile it is meant to stand on, and the
counter-example below (a west-edge point with a west-side flame) still
applies to it. Left as a decision, not a patch, for whoever next needs
`light::sample` to be trustworthy exactly on a tile's edge — the fix shape
is probably the same one used here: carry the tile alongside the position
instead of re-deriving it. *(This is exactly what
[`lighting_raymarch.md`](lighting_raymarch.md)'s step 2 then did — `Spot`
grew its own `tile` field.)*

**The second shape in the screenshot — the white line over empty
background — is still unexplained at the time this was written.**
Confirmed still present, unchanged, in the post-fix render; it was not
chased this session and may or may not share a cause with the one above.
*(Later root-caused in `lighting_raymarch_archive.md`'s step 5 entries —
not repeated here.)*

### The shadow-raymarch anomaly, first draft (kept for the counter-example)

Everything below is the original write-up of this same defect, kept for
the reasoning it contains — the counter-example in particular is still the
argument against the tempting one-line fix, should this shape of bug turn
up again somewhere the authored tile is not available to hand to the
shader directly.

**A third, different defect, found while screenshot-checking a fix on a
live scene: a shadow-raymarch anomaly, not a mesh-coverage gap.** A
previous fix closed the mesh-coverage leak (measured: gone from the
textured `Lit` picture, still present as 9 unmeasurable-by-eye single
pixels in `View::Place`'s raw channels). What is left on a live screenshot
is a different shape entirely: on the flight's topmost tread — one flat
mesh face, one normal, no adjacent geometry to tie or overlap with —
`View::Shadow` (`OPENSHARD_FRAME_VIEW=7`, an index into `debug::View::ALL`,
**not** the raw `View` discriminant `blit.wgsl` pins — `VIEW_SHADOW` there
is `6`, `ALL[7]` is `Shadow`; this cost a wrong render while writing this
entry) shows an isolated white dash sitting in the middle of an otherwise
uniform grey face, plus a white line running across the black background
where there is no geometry at all. White is `Reach::through == 1.0`, a
fully open path; grey is a partial occluder. A single pixel reading
"nothing in the way" inside a region every neighbour reads as partially
blocked, on a perfectly flat surface with nothing to tie a seam against,
is not a coverage question — it is the occlusion-grid walk (`light.rs`'s
`sample`, decision 9's CPU/GPU parity twin of `blit.wgsl`'s fragment loop)
picking a different answer for two neighbouring rays to the same flame.

Reproduce the picture the same way as below, but read `View::Shadow`:

```sh
OPENSHARD_CLIENT=… \
    OPENSHARD_SCENE_AT=1497,1627,10 OPENSHARD_SCENE_RADIUS=1 OPENSHARD_SCENE_TILES=0x0739 \
    OPENSHARD_SCENE_GROUND=0 OPENSHARD_SCENE_EXTRA=1498,1626,10,2852 \
    OPENSHARD_SCENE_ZOOM=2 OPENSHARD_FRAME_VIEW=7 \
    OPENSHARD_FRAME_DUMP=/tmp/shadow.ppm \
    cargo run --release -p openshard-client-render --example isolated_scene
```

and look at the topmost tread (the flight's uphill end) — the dash sits a
short way in from its outer corner, on the face itself, not on an edge.

**Found: it *is* an edge — the tread's own outer one, and every point on
it, not one speck.** Bisected with the profiler
(`OPENSHARD_SCENE_PROFILE_FACE=flat`, `_FROM=1497.90,1627.20,15`,
`_TO=1498.00,1627.20,15`, `_STEPS=100` — real map coordinates, **not** the
synthetic ones the `solid:` lines print; `run_profile`'s `shift_f` shifts
real into synthetic itself, so feeding it already-synthetic numbers
double-shifts and reads "outside radius" for the whole segment, which cost
the first pass at this): every sample from `t=0.000` to `t=0.990` reads
`stopped at (100, 99)`, unchanged for a hundred steps, and `t=1.000` — `x`
exactly `1498.00`, the tread's own east edge — flips straight to `through
1.000`. No gradient between the two: a knife edge sitting exactly on the
integer, not a trend approaching one, which is what rules out "the corner
genuinely sees around the riser from here" and points at the grid lookup
itself.

**Root cause: `Surface::Flat` gets no boundary nudge, and `Surface::Face`
does.** [`stand_clear`](../../../crates/client/render/src/light.rs) moves a lit
point a hair off the surface it is *the face of* along that face's outward
normal (`STAND_OFF`, decision-worthy already) before `walk_cells` floors it
into an occlusion-grid cell — but that nudge comes from `face.outward()`,
which only exists for `Surface::Face(_)`. `Flat` and `Upright` get `[0.0,
0.0]`: no nudge at all. A flat top face's own edge, at this tread, sits at
`x = 1498.0` for every `y` in its footprint — a whole integer, not a
fraction — and `walk_cells`' `first = (from[0].floor() as i32, from[1].floor()
as i32)` (`light.rs:1681`) puts a point sitting exactly on that line one
cell east of the tread, which carries no riser, so the walk starts there
and never crosses the occluder every interior sample does. This is
`blit.wgsl`'s `walk` too, by the same CPU/GPU parity decision 9 already
named — unconfirmed there yet, but the floor is the same operation.

**The obvious fix — nudge the origin a hair back along the ray, away from
the flame — is not generally correct, and here is the counter-example
before anyone writes it.** This tread's flame sits east of the sampled
edge, so nudging west (back toward where the ray came from) happens to
land in the tread's own cell, cell 100 — but a *west*-edge point (`x`
exactly on its tile's low bound) with a flame also to its *west* would
nudge the same direction the ray already travels, straight into the wrong
neighbour. The sign that makes the first case work is which side of the
boundary the point's own tile is actually on, and `walk_cells` has no way
to know that from a bare `(x, y, z)` and a `Surface` — it was never given
the tile. Fixing this for real means answering that question first, not
picking a nudge sign that happens to work on one tread.

**Where the answer probably lives: the same per-pixel id
[`gbuffer.md`](gbuffer.md) already carries for other reasons.** The
renderer that draws this face already knows which tile's mesh triangle it
came from — that is lost by the time a bare `Spot` reaches `light::sample`.
Widening `Spot`/`walk_cells` to carry (or derive from an id already in
hand) the sample's own owning cell, and using that instead of re-deriving
tile membership from a float that can legitimately sit exactly on a shared
boundary, is the shape of the fix — but it is a decision, not a patch: it
touches `Spot`'s callers on both the CPU debug path and `blit.wgsl`'s
mirror, and decision 9's parity has to hold across both.

The second shape in the screenshot — the white line over empty background
— is still unexplained here too; it was not chased this session, and may
or may not share this cause.

### The corner-split seam, hairlines, and stair shading (earlier sessions still)

Everything below is the session-by-session log leading up to the anomaly
above — kept intact, in its original "everything below is the session
before it" chain, because several of it are the actual origin of design
facts `lighting.md` states as current (the corner stance, the mesh-face
tile attribute, the tread-normal investigation that became `gbuffer.md`'s
own work).

**The hairline from an earlier session is fixed, and it was not the depth
tie that was suspected.** CPU-side, a tread's top and its own riser share
corners built from the exact same `lo`/`hi` arithmetic (`facing.rs`'s
`Prism::mesh`), so those corners are bit-identical in world space before
anything projects them — there was never a tie for the rasteriser to lose
its nerve over. What was actually missing turned out to answer to a direct
measurement, not a picture: `View::Place`'s sub-tile-fraction channels, at
the leak pixels, read `(1, 1)` — the one value `mesh_face.wgsl`'s own
`INSIDE = 126.0/127.0` clamp can never produce, so those pixels were never
touched by the mesh pass at all, only by the sprite billboard drawn under
it. The leak had two shapes, found by dumping `View::Place` over the
repro scene below and counting pixels reading `(1, 1)` in a scratch script
rather than by looking at the picture: a handful of single pixels at
tread/riser ties, and — the one actually visible as a continuous hairline
— full-height, one-column runs along a riser's own *un-shared* side edge,
the same 2.5%-of-the-art gap `best_prism`'s imperfect fit already named
for a wedge finding below. Both are a fitted box not quite reaching the
true art, not a tie.

Fixed in `facing.rs`'s `Prism::mesh`: every riser now grows
[`SEAM_OVERLAP`](../../../crates/client/render/src/facing.rs) (`0.15`, in `z`)
past the tread it meets, and every face grows
[`WIDTH_OVERLAP`](../../../crates/client/render/src/facing.rs) (`0.03`, in
tile-fraction) past the tile-crossing edge `Prism::footprint` holds at the
unit square regardless of `lo`/`hi` — two real overlaps in world space, not
another depth formula, so `docs/archive/render/gbuffer.md` decision 4's argument against a
second depth formula is untouched. Measured on the repro scene below
(count `View::Place` pixels reading `(1, 1)` in the stair's own screen
region): 35 leak pixels before, 9 after, and the two dominant 14-pixel
runs — the ones a person actually sees as hairlines — are down to a single
pixel each. The 9 that are left are isolated single pixels sitting exactly
on a corner, where a width-axis edge and a `z`-axis edge meet: each overlap
closes its own axis, but a corner needs both at once, and neither constant
reaches it alone. Worth trying next: growing the corner along *both* axes
at once rather than raising either constant further — raising
`WIDTH_OVERLAP` to `0.06` alone left the same 9 pixels exactly unchanged,
so the corner is not a matter of degree.

Reproduce with the same scene, `View::Place` (`OPENSHARD_FRAME_VIEW=1`)
this time, not `View::Light` — the sub-tile channels are what actually
named the bug, the picture only showed its shadow:

```sh
OPENSHARD_CLIENT=… \
    OPENSHARD_SCENE_AT=1497,1627,10 OPENSHARD_SCENE_RADIUS=1 OPENSHARD_SCENE_TILES=0x0739 \
    OPENSHARD_SCENE_GROUND=0 OPENSHARD_SCENE_EXTRA=1498,1626,10,2852 \
    OPENSHARD_SCENE_ZOOM=2 OPENSHARD_FRAME_VIEW=1 \
    OPENSHARD_FRAME_DUMP=/tmp/place.ppm \
    cargo run --release -p openshard-client-render --example isolated_scene
```

then load the `.ppm`, take the stair's own screen rectangle, and count
pixels whose red and green channels both read `250` or higher (`sub.x`/
`sub.y` near `1.0` — the packed encoding rounds to `255` at the exact
sentinel, so a small margin catches it without a false positive from an
honestly-lit `0.97`-ish fraction near a real edge).

**Still open at the time, carried over and not re-investigated: the wedge
of stale shading where the fitted box does not reach the true art
silhouette.** `WIDTH_OVERLAP` above almost certainly shrinks it — same
mechanism, same fix — but nobody re-screenshotted `View::Light` after this
fix to check by how much, and `best_prism`'s own score is untouched
(`WIDTH_OVERLAP` is a render-time overlap on top of whichever box the
search already picked, not a change to the search or the score it
reports). If the wedge is fully gone, `PRISM_FITS`'s "which of the 217
misses are stairs the model should fit better" quality work drops in
priority; if it is only smaller, the number is worth re-measuring against
the `0.975` on record.

**A session before that: confirmed live and fixed for real — the
corner-split seam is gone. What is left is two smaller, different residual
artefacts on the same stair.**

Reproduce with the same scene this whole file used, now honest because of
a fix — `View::Light` (`_FRAME_VIEW=5`) shows both far more clearly than
the ordinary `Lit` picture, which is why this is the view to look at
first:

```sh
OPENSHARD_CLIENT=… \
    OPENSHARD_SCENE_AT=1497,1627,10 OPENSHARD_SCENE_RADIUS=1 OPENSHARD_SCENE_TILES=0x0739 \
    OPENSHARD_SCENE_GROUND=0 OPENSHARD_SCENE_EXTRA=1498,1626,10,2852 \
    OPENSHARD_SCENE_ZOOM=2 OPENSHARD_FRAME_VIEW=5 \
    OPENSHARD_FRAME_DUMP=/tmp/light.ppm \
    cargo run --release -p openshard-client-render --example isolated_scene
```

**1. A hairline seam at every tread/riser boundary, running the full length
of the flight — once per tread, repeating down it.** Visible as a thin
bright line exactly on the nosing edge, in both the internal boundary
(between two treads) and the outer silhouette (the flight's own left edge).
Hypothesis, not yet confirmed at the time: `statics::push_mesh` gives
**every face of one static's mesh the same `depth`** — the enclosing
`SpriteQuad`'s own single value, reused rather than recomputed
(`statics.rs`'s own doc on `push_mesh` explains why: "a second depth
formula here is a second chance to disagree"). Two faces that share an edge
(a tread's top and the riser below it) therefore share a depth too, so
`MeshFaceRenderer`'s `LessEqual` test cannot decide a winner *between them*
by geometry — only by triangle draw order, which is `Prism::mesh`'s
`faces()` order and not obviously guaranteed to agree with itself pixel to
pixel along a shared edge. If true, the fix is not a depth formula (the doc
comment's whole argument against one still holds for two faces that are
genuinely at different depths) but something narrower at exactly the
shared edge.

**2. Small triangular/rectangular patches of stale shading, where the
fitted box does not reach the true art silhouette.** Both screenshots this
session took showed one: a wedge of the old (pre-fix, corner-stance)
colour survives inside an otherwise honestly-shaded tread. Consistent with
the number already on record — `best_prism` scores this flight's graphic
`0.975` against `PRISM_FITS`'s `0.9` gate, so ~2.5% of the art's opaque
pixels are outside every face `push_mesh` draws, and `MeshFaceRenderer`
only overwrites the pixels it actually rasterizes (`renderer.rs`'s own
doc: "this pass owns no pixels of its own" for the empty case, and by the
same logic, no pixels outside its triangles for the non-empty one) — so
the sprite pass's own stance answer is what a pixel outside the mesh's
footprint keeps. Two different fixes compete and neither is chosen yet:
tighten `best_prism`'s candidate search so the box gets closer to 1.0
(quality work already flagged as open, under "which of the 217 misses are
stairs the model should fit better"), or give the mismatch itself a
narrower, deliberate answer (extend the mesh's footprint by a fraction of a
pixel, say) rather than relying on the fit ever reaching exactly 1.0.

A useful next step neither of these two had yet: dump `Prism::mesh`'s own
vertices for this graphic next to `StaticAtlas::opaque_at`'s silhouette and
look at where they actually diverge, rather than guessing from the picture
alone.

**A session before that: the "slope `Stance`" diagnosis was wrong, and the
tool that produced it was blind to a fix that already existed.**
`statics::collect` (real map furniture) already built an honest mesh — a
flat top and vertical risers, `facing::Prism::mesh` — for any climbable
static whose art clears `PRISM_FITS`, and `lib.rs` already ran that mesh
pass right after the sprite draw, overwriting the place/depth buffer with
the honest per-face normal for whatever it covers. `place.rs::Stance`
needed no new variant: a staircase is faces at 90°, not a slope, and
`Flat`/`FaceNorth`-family already name exactly that.

The reproduction never exercised any of it. `isolated_scene`'s synthetic
map carries no statics of its own (`WorldMap::from_blocks` never does), so it
re-plays every real map static it pulls as a `GroundItem`, through
`items::collect` — and `items::collect` threw `Placed::prism` away and
never called `push_mesh` or ran a mesh pass at all. Every picture this
file's earlier entries showed of this stair was strictly worse than what
the live client actually draws, because the one tool built to look closely
was the one place the fix could never apply. `OPENSHARD_SCENE_ZOOM` (real
zoom-ladder notches, GPU-nearest, `Zoom::scale_up`) also went in alongside
— a crop blown up afterward with an image tool is a different resample and
was adding its own noise on top.

Fixed: `items::collect` now returns the same `statics::StaticGeometry`
shape `statics::collect` does, building `push_mesh` (now `pub(crate)`)
geometry for any item whose `Placed::prism` is `Some` — one bug fixed
twice, since this is shared by both real callers. `lib.rs` merges the
item-sourced mesh into the same buffer and draw call as the map statics' —
which is a genuine fix for the live client too, not only the tool: a
climbable *item* (a decoration, not map furniture) got no honest mesh
before this, either. `isolated_scene` gained its own `MeshFaceRenderer`
and wires its row buffer into the final blit instead of the dummy.

Re-rendered on the same stair-plus-lamp scene this file has used
throughout: the hard corner-split seam is gone from both the ordinary
`Lit` picture and `View::Light`, replaced by a continuous per-tread
gradient toward the lamp matching an independent `_SOLIDS=lit` ground
truth (below). A faint straight seam remains near the flight's top vertex
in `View::Light` — plausibly the last sliver of the ~2.5% `best_prism`'s
box does not cover (score `0.975` against `PRISM_FITS`'s `0.9`) — small,
and not yet measured or explained at the time.

**Still open at the time, and unrelated: whether what the user sees live
differs from even this fixed picture.** The live client and this
reproduction now agree on this one stair; nobody had yet confirmed the
*live* game showed the same seamless result on the location the user was
actually looking at, or whether "other problems" they described live were
a separate, still-unfound defect.

**A session before that: the user's corner artefact, reproduced in
isolation and traced to a specific, already-known-but-unfixed defect: the
stair's shading, not its occlusion.** A screenshot of a real flight next to
a lamp showed the same X-crossed noise on `View::Height`, `View::Occluders`,
`View::Light`, `View::Shadow` and `View::Reach` alike — different views,
same shape of wrongness, which is the tell that they share one upstream
input rather than five independent bugs.

Reproduction, `examples/isolated_scene.rs` (already built for exactly
this), narrowed to the two stair tiles and the lamp and nothing else:

```sh
OPENSHARD_CLIENT=… \
    OPENSHARD_SCENE_AT=1497,1627,10 OPENSHARD_SCENE_RADIUS=1 OPENSHARD_SCENE_TILES=0x0739 \
    OPENSHARD_SCENE_GROUND=0 OPENSHARD_SCENE_EXTRA=1498,1626,10,2852 \
    OPENSHARD_FRAME_DUMP=/tmp/x.ppm OPENSHARD_FRAME_VIEW=3 \
    cargo run --release -p openshard-client-render --example isolated_scene
```

(`_TILES=0x0739` is the flight's own graphic, `1849`; the lamp is a
decoration, not a map static, so it still has to come in as `_EXTRA` — see
the DB-lookup recipe below, under "The occluding world" backlog.
`_FRAME_VIEW` is an index into `debug::View::ALL`, not the enum's own
discriminant: `3` `Height`, `4` `Occluders`, `5` `Light`, `7` `Shadow`, `8`
`Reach` — the sixth, `0`, is the ordinary `Lit` picture, which shows
nothing wrong to the eye.)

`View::Height`'s ramp (a band every `Z_PER_TILE` = 11 units) wound four or
five times across the flight's own shadowed face alone — a real swing of
40+ `z` units painted across geometry `Occlusion::solids_at` confirms is
only `10..15`, five units, on the same tile. So the picture, not the grid,
is lying about the height, and everything downstream that reads a pixel's
own world height (`Occluders`' red/blue test, `Light`/`Shadow`'s shadow
term, `Reach`'s flame count) inherits the same noise.

That number — a wrong `place` attachment, not a wrong grid — is exactly
this file's own, already-written "A stair is read as a corner of two
walls, and there is no stance for a slope" entry (see "The occluding
world" archive, under "Found on a staircase in Britain"). Its occlusion
half was fixed: `Prism`/`tread_box_of` gave the grid real per-tread boxes,
`tests/prism.rs` scores the flight at `0.975`, and `Occlusion::solids_at`
above is exactly that fix's own output. Its **shading** half was not, and
code at the time still showed it: `place.rs`'s `Stance` had no slope, only
`Flat`/`Upright`/four faces/four corners, so `statics.wgsl` still resolved
this stair's `STANCE_CORNER` art as two *vertical* walls, the same
`in.twin` corner-splitting entry describes, and wrote a per-pixel height
built for a wall's flat plane onto a stepped one. `blit.wgsl`'s
`outward(stance)` inherited the same wrong normal for the same reason.
Confirmed by elimination, not just by reading the code: the grid's own
boxes, drawn with nothing else in the picture (below), were clean.

**A tool for looking at the grid with the sprite's shading bug entirely
out of the picture, built while chasing this.** `examples/isolated_scene.rs`
gained `OPENSHARD_SCENE_SOLIDS=white|lit` — skips every sprite and blade of
ground and draws only `solid::standing`'s boxes (the same list and the same
`solids::SolidsRenderer` the live client's F5 overlay uses) on black.
`white` is one flat colour, for the shape alone; `lit` colours each face by
`light::sample` — four real samples, one per corner in `Solid::faces`'s own
order, blended across the fill by the ordinary vertex interpolation
`solids.wgsl` already does (`solids::FaceColours`), so a gradient shows
instead of a flat tint with a step at every face's edge. Two more knobs
matter together: `OPENSHARD_SCENE_SOLIDS_EDGES=0` drops the outline stroke
(otherwise it can hide a face wrongly showing through), and
`OPENSHARD_SCENE_SOLIDS_OPAQUE=1` swaps the F5 overlay's translucent fill
for a straight overwrite — `solids::Style { edges, opaque }`,
`Style::default()` matching the live overlay exactly,
`SolidsRenderer::render`/`render_lit` take one now instead of two trailing
bools. No depth buffer: opaque occlusion is painter's-algorithm-correct
only as far as `solid::standing`'s existing back-to-front sort is, which
its own doc already flags as fragile once a solid spans more than one
tile.

```sh
OPENSHARD_CLIENT=… \
    OPENSHARD_SCENE_AT=1497,1627,10 OPENSHARD_SCENE_RADIUS=1 OPENSHARD_SCENE_TILES=0x0739 \
    OPENSHARD_SCENE_GROUND=0 OPENSHARD_SCENE_EXTRA=1498,1626,10,2852 \
    OPENSHARD_SCENE_SOLIDS=lit OPENSHARD_SCENE_SOLIDS_EDGES=0 OPENSHARD_SCENE_SOLIDS_OPAQUE=1 \
    OPENSHARD_FRAME_DUMP=/tmp/solids.ppm \
    cargo run --release -p openshard-client-render --example isolated_scene
```

That picture was clean — a smooth gradient down the flight toward the
lamp, no crosshatch anywhere — which is the other half of the elimination
above: whatever was wrong was downstream of the grid, in the sprite's own
shading.

**Left open at the time, and where this was headed next:** a
`scene::staircase` with one flight and nothing else, then a stance for the
shape, minus the occlusion half, which was done. What was still missing
was a slope `Stance`/`Surface`: `place.rs` needed a variant `statics.wgsl`
could resolve a `CLIMBABLE`, prism-fit static to instead of
`STANCE_CORNER`, a per-pixel height formula for it that agreed with the
tread geometry `Prism` already measured (rather than a wall's flat-plane
one), and `blit.wgsl`'s `outward()` taught the same normal. *(This is the
work that became decision 40's steps 4/5, and was later retired in favour
of `gbuffer.md`'s real per-face tread geometry — see "The G-buffer bridge"
archive below.)*

**A session before that: a real staircase render was looked at and named
two defects; both were fixed, and a third thing the second one's own
measurement turned up was left open.**

**Fixed: a tread's own top was shadowed by the riser it stands on.**
`Surface::shadowed_by_own_tile` (decision 28) reads every `Stance::Flat`
pixel on a named-edge tile as the room floor its own doc example
describes — "a floor pixel on a wall tile is inside the room, and the ray
from it to a lamp in the street crosses the panel its own tile stands on."
A tread's top is `Stance::Flat` too (`gbuffer.md`'s honest per-face
normal), and it sits at exactly its own riser's `top()`: nothing of the
riser stands *above* that height, so nothing is between the tread and a
lamp at that height the way a real floor's wall genuinely rises past it.
`light::walk_cells`'s per-solid exemption test gains one more case —
`caps_this`, a flat pixel at or above *this* solid's own top is standing on
it, not behind it — scoped to the one solid at a time rather than to the
tile's whole mask, so a genuine floor at a panel's *bottom* (the ordinary
room case decision 28 was written for) is unaffected. `blit.wgsl`'s `walk`
carries the same test. Reproduced first, before either file was touched:
`light::tests::a_treads_top_is_not_shadowed_by_its_own_riser` builds the
same three-tread `0x0736` fixture `occlusion.rs`'s own test uses, reads
`through 0.513` off the unfixed rule (the riser's own top is exactly the
boundary `pierces()` returns `0.5` at) and `> 0.9` after. All of
`tests/lighting.rs` and `tests/frame.rs` (the Rust/WGSL parity suite among
them) stayed green, so nothing already authored stood on the case this
widens.

**Fixed: the 37.7% of `CLIMBABLE` art the `Prism` model does not fit no
longer takes the wall-corner reading at all.**
`tests/prism.rs`'s `how_much_of_the_climbable_art_the_prism_model_covers`
(`OPENSHARD_CLIENT=… cargo test -p openshard-client-render --test prism
how_much -- --ignored --nocapture`) measured it first: **576 `CLIMBABLE`
pictures the install ships, 359 (62.3%) clear `PRISM_FITS`**, and the other
217 were falling through `Builder::add` into the same code every ordinary
wall uses — `edges_of(shape.facing)`, which reads a stair's base exactly
as the static's own doc comment says it reads a house corner, and
`Solid::box_of`'s `PANEL_THICKNESS` inset then narrowed the panel a fifth
of a tile short of the neighbour it should meet. `Builder::add` now asks
`tile.flags.is_climbable()` a second time, after the fitted-prism branch
and before falling through to `edges_of`, and answers with one whole-tile
body (`EDGE_ANY`, `box_of`'s own un-inset case) rather than a facing-shaped
panel — the same answer step 23.1 later gave every climbable static before
decision 34 taught the grid to read a *fitted* one as its own treads,
restored for the flights the fit still misses. `occlusion.rs`'s own
`a_stair_is_two_faces_per_tread_and_each_ones_height_comes_off_the_art`
held the old two-panel fallback by name; it now asserts the whole-tile
body and its full-width span instead. All of `tests/lighting.rs`,
`tests/frame.rs` and `cargo test --workspace` stayed green.

**Left open: which of the 217 misses are stairs the model should fit
better, and which are a different shape `CLIMBABLE` also covers (a ramp, a
ladder) — nobody has looked at the graphics one by one.** The scores split
into two groups that read as two different causes: some are hundredths
under the gate (`0.898`, `0.897`, `0.895` — plausibly more treads than
`best_prism` searches, or a measurement quirk) and some are far off
(`0.138`, `0.144` — plausibly not a box-of-treads shape at all). This no
longer produces a seam either way — the fallback above is a correct, if
coarse, whole-tile body regardless of which — so improving `PRISM_FITS` or
`best_prism`'s candidate search is a quality question now (a fitted flight
still looks better than a box: real tread silhouettes, no ziggurat), not a
correctness one, and it still wants the graphics looked at before either
number moves.

**A session before that: step 23.5.5's wall-thickness half landed: a panel
is a real, tested slab, not only a drawn one.** `occlusion::PANEL_THICKNESS`
(`0.2`, the number the view used to invent alone) is now the geometry
`Solid::box_of`'s four named edges fatten inward by, the record itself
carries it, `solid::drawn` no longer touches a panel — the box already is
the picture — and `solid::DRAWN_PANEL_THICKNESS` is gone with the split it
existed to name. `light::corner_tie` (`blit.wgsl`'s twin) replaced its bare
`1e-4` with an exact conversion of `PANEL_THICKNESS` into the walk's own
`t`, argued and pinned by a pure unit test. No scene was found, this
session, that the old tolerance fails and the new one catches — the
derivation is correct and harmless, not a demonstrated fix.

**`1509,1635` is not this step's remainder — it is decision 34's own body
footprint, and it hit a real wall of its own.** What decision 34.1
measures is a band in the *screen column* (`fx - fy`, i.e. `u - v`), which
is a **diagonal** stripe of the tile and cannot become an axis-aligned
`occlusion::Solid` box the way a panel's or a tread's can —
`facing::Prism::footprint` only manages it because a climb names a world
axis to be flat on, and a body's silhouette names none. See "The occluding
world" backlog, "A body's footprint... does not actually have a general
shape to learn yet", for the full argument and the candidates nobody has
picked between. This is genuinely open, not merely unauthored — see
`lighting.md`'s Status section.

**The arch is still exactly where it was: `Builder::add` does not read
`Shape::blocks`, and nothing is authored into any table.** That plumbing —
turning an authored `Blocks` list into real solids in the grid, the way
38.2's spill landed ahead of its first user — is unrelated to either
finding above and does not wait on them.

**A session before that: the instrument now has a second, non-trivial
check: three blocks reproduce the three-tread stair `0x0736` almost as
well as its own automatically-derived prism does.** `1822`'s single-tread
case was checked against `block 0 8 0 8 0 5` and landed at 0.977, the same
number `best_prism` gets — a box is a one-tread prism, so that check could
not tell a bug in `blocks_silhouette` from a bug in `prism_silhouette`
sharing one. `1846` can: its best fit is three treads climbing west,
heights 1/3/5 (`best_prism`, 0.975), and hand-placing the matching three
boxes — `x 5..8 z 0..1`, `x 3..5 z 0..3`, `x 0..3 z 0..5`, each the full
`y` — through `tests/author.rs` scores **0.966** against the real art, a
picture whose disagreement is one thin cyan sliver where the prism's
rounded riser beats a box's flat one. `blocks_silhouette` and
`prism_silhouette` agree almost to the pixel on a shape neither shares any
code with the other to draw. Done as a scratch table (`OPENSHARD_TABLE`),
not written into `data/overrides.table` — a stair is already served
correctly by the automatic prism fit, so an authored block row for it
would be exactly the invented-row-nobody-needs the sheet's own header
warns against.

**A joint and an arch are still unauthored, and staying that way until
they can be named rather than guessed.** The DoD's "the two shapes a
person reported as 'something odd happens'" points at a real observation
this file does not record a graphic id for — `(1441, 1692)`'s corner
(`0x0033`) is the *facing* detector's own worked example, decision 17/18's
pierce case, already fixed by a rule that has nothing to do with `blocks`,
and no arch graphic is named anywhere in this plan or in the client's
`tiledata` names that clearly reads as the classic-era case being pointed
at. Inventing a candidate and authoring it into the checked-in sheet is
exactly what the sheet's own comment refuses: "a row invented to exercise
it would be a wrong answer shipped to every shard." Whoever has the report
names the graphic; this had not happened yet.

**Found on the same pass, one session earlier: 23.5's own first bullet
(treads as their own boxes) turned out to be done already, as a side
effect and not by anyone asking for it under this plan's name** — full
text under "The occluding world" archive, step 23's own sub-step 5.

**Earlier session log — the `tests/author.rs` instrument, decision 41's
landing, and the format-3/4 bumps** — see "The art-measurement pipeline"
archive below for the full write-up of `tests/author.rs` landing, and "The
occluding world" archive for decision 41's own text.

**Earlier still: the sun-related step 6/15 measurement narrative, decision
16's discovery, and the very first sessions of this plan** — see the
per-section archives below (Sunlight, Point lights, The art-measurement
pipeline) for that material, which is organized there rather than
repeated in this chronological log a second time.

**Two small things worth pulling out of that same run of "read decision X
and Y first" session recaps, not duplicated by the decisions' own text
elsewhere:**

- **Decisions 26, 27 and 28 were framed, in the session that produced
  them, as three answers to one objection** — worth keeping in the
  objection's own words: *deciding who is lit should be by polygon, not
  by tile.* Every one of the three was a rule that had answered with a
  **tile** where the question was about a **surface** — decision 26's
  flame moved outside its wall's plane rather than merely excused, 27's
  lid gained a normal so it stopped taking every pool from any side, and
  28's self-shadow exemption moved from the tile to the surface.
- **`scene::house_corner`'s own fidelity number, from the session that
  reproduced decision 24's house-corner report in isolation.** The built
  scene — Britain at `(1441, 1692)` with the graphics replaced by three
  synthetic ones — leaked **0.845** of the flame where the real map
  leaked **0.847**. Worth keeping as a general standard for this file's
  own reproductions: *a reproduction that agrees to three digits is a
  reproduction; one that merely looks similar is a second scene.*

Nothing outside this file was ever left half-finished at any of these
waypoints — `main` built, the three commands were silent, and the pictures
came from `tests/cost.rs`'s frame dump, headless, with
`OPENSHARD_FRAME_VIEW=5` for the ones the art had to be thrown away from.

## The G-buffer bridge: the place attachment

This section carries decisions 2, 13, 16, 22, 25 and 27, and steps 2 and 12 — everything about
what a pixel's second attachment carries and how a stance becomes a
normal — plus the backlog entries about the corner report and the
tread-normal investigation that became `gbuffer.md`'s own work.

**Decision 2. The world passes write a second attachment: `(x, y, z)` per
pixel.** *(the shape of this payload is what [`gbuffer.md`](gbuffer.md)
revisits — everything below is still what is built and still true of the
running client, not superseded, just no longer assumed final)*
`Rgba16Uint`, as `(x, y, z + 128, kind)` — the tile the pixel belongs to,
the height it was drawn at, and what kind of thing wrote it. Ground,
statics and mobiles all know these numbers per instance already; none of
them has to compute anything new. A fragment a sprite discarded writes
nothing, so the channel says what is *visible*, which is exactly the
question lighting asks. `kind == 0` is "no world here" — the cleared
background — and takes ambient and no flame.

Why an integer format and not a float one: these are tile indices and a
`z`, and a `u16` holds a coordinate on the largest facet a client ships
(7,168) exactly. `Rgba16Uint` is colour-renderable in WebGL2, which was
the ceiling this crate drew under at the time this decision was written
(later revised to WebGPU — see decision 30.5's "Answered" note under "The
occluding world").

**Decision 13. A sprite says which way its picture faces, and that is
where its pixels are.** The attachment carries where in its tile a pixel
is, and a sprite used to write the middle of the tile for every one of its
pixels — which is right for nothing and wrong for two different reasons. A
*floor* static is a picture of the tile's diamond, so its pixels are
spread across the tile and its height is the tile's; a room's floor
written as one place came out as flat 44-pixel diamonds with a step at
every seam, which is most of what a pool of light was accused of looking
like. A *wall* is a billboard: what runs down its picture is height.

**Across a wall, this decision first said `1/44` of a tile along the
screen's `x - y` axis, and that was wrong.** It survived one commit. That
axis is the horizontal, and no wall runs along it: a wall runs along one
*world* axis, which in this projection is a screen diagonal. Spreading a
wall's pixels sideways puts them along the one direction the wall does not
go, and it looks like it. What replaced it was the tile's middle
everywhere — an honest statement of what was not known — until step 15
measured the axis out of the art. The `1/44` is not half-right; it is the
wrong direction, and it is written down here because the plausible-looking
version is the one somebody re-derives.

Which stance a static has comes from the client first: `TileFlags::FLOOR`
— `UFLAG1_FLOOR` in Sphere, `Background` in ClassicUO — is set on floors,
rugs and roads and on nothing that stands up. Not `PLATFORM`: a table is
`BLOCK | PLATFORM` and is a picture of a table, not of the ground. Then the
art, for which edge a wall stands on. `place::Stance` is the six of them
(at the time; later ten with the corner — decision 25) — flat, four faces,
and "standing but unknown" — and it rides in three bits above the kind in
the instance's place word, never in the attachment, whose fourth channel
is two bits of kind and fourteen of fraction with nothing spare (also
later revised — decision 22 found a second spare channel).

- [x] **Step 2. The `(x, y, z)` attachment.** `ground.wgsl` and
      `statics.wgsl` gain a second output; the quad structs gain the tile
      they are for; the renderer gains the texture and a second colour
      target. The frame tests read it back and assert that a wall's pixel
      names the wall's tile.
- [x] **Step 12. A floor is not a wall.** Decision 13's `place::Stance`: a
      flat static's fraction is the inverse of `camera::project` over the
      pixel's offset from its tile's centre, an upright one's is the
      tile's middle, and the bit comes from `TileFlags::FLOOR`. A room's
      floor stops being flat 44-pixel diamonds with a step at each seam.

**Decision 16. A fraction of exactly one names the next tile, and the walk
believes it.** A wall's face lies *on* the tile boundary, so the honest
fraction for a south face is `y = 1` — and `blit.wgsl` finds a fragment's
cell with `floor(tile + fraction)`, which for that number is the tile
beyond the wall. The walk exempts the fragment's own cell from shadowing
it, precisely so a wall's face is the brightest thing beside a torch; hand
it the neighbour and the wall's own tile stops being exempt, so **every
faced wall is shadowed by the wall it is the face of** and comes out at
ambient. Measured on Britain the first time this was drawn: a run of lit
wall at 249 dropping to the 65 of an unlit night.

So what the attachment carries is the fraction held one step of its own
seven-bit grid inside the tile — a hundred-and-twenty-seventh, 0.35 pixels
of world. `statics.wgsl`'s `INSIDE`. The geometry is still the boundary
and `facing::Face::place_at` still says so; it is the *encoding* that has
to name the tile the wall belongs to, and the two are different questions.
The same clamp covers a floor's outermost pixel, which had the same latent
bug from step 12 and never showed it, because a floor's tile is not an
occluder.

**Decision 22. A wall's face is one-sided, and the stance is what says
so.** Reported from the client: a wall lit from inside a house glows on
the street as though it were made of glass. It is the one fact the
attachment did not carry. A wall's two faces are **one tile, one plane,
one fraction and one height** — everything decisions 2, 13 and 16 write —
so nothing in the frame could tell the street side of a house from the
room side, and a torch in a room lit both equally.
`docs/archive/render/lighting_world.md`'s backlog carried it as "the sun has no facing
either" since the sun arrived (still true — see "Sunlight" below).

Step 15 already measured the answer and threw it away: the *stance* —
which edge of its tile a wall stands on — was used to place a pixel's
fraction and then dropped, because the attachment's fourth channel is two
bits of kind and fourteen of fraction with nothing spare. It is not the
only channel. The **third** is a `z + 128` in the low eight bits of a
`u16`, and the eight above it were empty; the stance rides there now
(`place::STANCE_SHIFT`), and `blit.wgsl` turns it into an outward normal.

Which way is *outward* is not a guess. The art only ever draws the two
faces an isometric camera can see — step 15 measured that too, north and
west being five graphics out of 1197 — so a south face's picture is the
surface turned towards `+y` and an east face's towards `+x`. A flame
behind that plane lights nothing, over a band `FACE_EDGE` wide so that a
lamp walking past the end of a wall does not switch its face off between
two frames.

~~**Except a flame standing in the wall's own line, which is part of that
wall.**~~ **Superseded by decision 26, and it was the wrong half of the
pair.** A lamp mounted on a house sits at its tile's *centre* — behind the
plane of the very face it is bolted to — so testing it blacked out the
wall it hangs on, and the exemption was what kept that from happening.
What the exemption could not tell is a mounted lamp from a **lamp post
standing in the street**: a line is a whole street long, so a post south
of a house was "part of" the east wall three tiles north of it and lit
every face of that run at full strength. Reported from the client, with
coordinates. The answer was the one this paragraph already named and put
in the backlog — place the mounted light outside the plane its tile names
— and once it is placed there, the facing test needs no exemption at all.

**Decision 25. A corner is two faces, and the art says so as plainly as it
says one.**

Decision 3 refused to read an edge off a silhouette, step 15 read one, and
everything since had been written as though the answer were "one face or
nothing". A corner was the *nothing*: `Stance::Upright` in the attachment,
`EDGE_ANY` in the grid, and three separate artefacts following from those
two fallbacks — a flat 44-pixel band between two continuous runs of wall,
both of its faces lit whichever side the flame was on, and a whole-tile
occluder where two panels stand.

The measurement was already being made and then thrown away. `face_of`
proposed each half of the tile's column in turn and refused the graphic
when the *other* half held more than a wall's own thickness — so by the
time it gave up it had measured both halves and found both to be faces.
That refusal was the only thing between here and an answer.

So the halves are read **twice**, and the order is what makes the change
safe:

- **Strictly first**, each half having to be the only face in the
  picture. That is exactly what the module did before, so every graphic it
  read reads the same today — 76% of Britain's walls did not move.
- **Then together**, each half offered the picture on its own. The only
  way through is that both are faces and each was refused for the other,
  which is what a corner is. A face beside a *blob* still fails, because
  the blob is not a face — two failures are not a corner, and that is the
  property the second pass rests on.

**Measured**: 91.9% of the wall statics standing in Britain, against
75.7%; 45.5% of the install's wall art, against 36.3%. 297 corner graphics,
296 of them the east-and-south pair a camera can see and one north-and-west.
`tests/facing.rs` prints all of it and asserts a floor under each, corners
included as their own count rather than as a share — a tail hidden in a
percentage is a tail that can go to zero unnoticed.

**A pixel is resolved to one of the two, in `statics.wgsl`, per fragment.**
Which half of the tile's column the pixel is drawn on is which surface it
is a pixel of; there is nothing else to ask, and nothing else needs
asking. So the attachment carries a single face with a single normal,
`blit.wgsl` is not touched, and `light::sample` has no case for a corner
either. Ten stances need four bits where six needed three, and **no format
changed**: both words had eight or more spare above the stance. The four
corner values are laid out so the two faces come out by arithmetic rather
than by a table — `right = FaceNorth + (offset >> 1)`, `left = FaceSouth +
(offset & 1)` — because the shader does it per fragment, and `place.rs`
pins those two lines in a test.

**In the grid it is two bits, and two bits is the panel path.** Decision
18's `edges` arm already handles a mask with more than one side in it;
what changes is that a corner stops being `EDGE_ANY` and therefore stops
being a *body*. A ray running alongside a corner — down the street it
stands on — crosses neither of its panels and passes, exactly as it does
beside the runs of wall either side of it. A ray from inside the house to
a lamp outside still crosses one of the two and is stopped, which is
decision 24's leak staying shut.

**What it costs is a free-standing solid's other two sides.** A pillar
filling its whole tile reads as a corner, because it *is* one — the same
two faces drawn on the same two edges, and nothing in a silhouette tells a
pillar from the corner of a building. Shading it as two faces is right.
Occluding it as two panels is not quite: a building's corner has its north
and west sides inside the house, where a pillar's are in the open, so a
ray clipping a pillar's far corner now passes where it used to be stopped
by the length rule. See "The occluding world" backlog for what it would
take to fix; it is not a whole-tile answer coming back, because that would
take the street-lighting back with it.

**Decision 26. A mounted flame burns outside the plane its tile names, and
the facing test is geometry with no exceptions in it.**

Reported from the client with the coordinates, which is the shape of
report this pass learned to want: the lamp at `(1441, 1693)`, the corner
tile at `(1441, 1692)`, and *the face leaning towards `(1442, 1692)`* — the
corner's east one — lit when it should not be. The lamp stands at `x =
1441.5` and that face lies in the plane `x = 1442`, so the flame is half a
tile **behind** the surface it was lighting.

What lit it was decision 22's exemption: a flame standing in a wall's own
row or column is part of that wall. That is true of a sconce and false of
everything else standing in a street, and a column is as long as the
street — so one lamp post lit the far side of every wall in its column.
`tests/onsite.rs` says it in one line, because that is the instrument this
report needed: it prints each of a tile's four faces with `through` and
`facing` apart, and a face behind the flame reads `through 1.000, facing
1.000` where a shadowed one reads `through 0.000`. Two numbers, two
different defects, and one of them invisible in any picture of the ground.

The exemption existed for a real reason: a lamp bolted to a wall sits at
its tile's *centre*, which is behind the plane of the face it lights, so
the geometry blacks out the very wall it hangs on. But that is a fact
about **where the flame is**, not about which surfaces it may light — the
map says "this tile" because a tile is all the map has, and the lamp is
really on the outside of the panel. So the flame is moved rather than
excused: `light::mounted_at` puts a flame whose own cell carries a panel
half a tile plus `FACE_EDGE` outside that plane, on the side the wall's
picture is drawn from, componentwise so that a corner's two panels are
both cleared.

Three things fall out of the move, and the second is the one that had
been in this file's backlog since its first version:

- **The wall it hangs on is lit at full strength**, because the flame is
  now in front of the plane by more than the band the facing test softens
  over.
- **A sconce stops lighting the room behind its wall.** The flame lands on
  the *next* tile, so the wall stops being the flame's own cell — which
  decisions 3 and 17 exempt from shadowing it — and becomes an ordinary
  occluder. The oldest known-wrong entry in this file, and the test that
  pinned it (`a_sconce_lights_through_its_own_wall`) became
  `a_sconce_lights_the_street_and_not_the_room_behind_it`.
- **The facing test loses its only exception.** A surface is lit if the
  flame is in front of its own plane, and that is the whole rule. Two
  comparisons less per light per fragment, in both implementations.

A flame on a tile with **no** panel is not moved, and that is what covers
the ordinary cases by construction: a torch on the ground, a brazier in a
room, and the lamp post the report was about. Neither is one whose sides
cancel — a lid, or the whole-tile `EDGE_ANY` of a graphic the art would
not name — because there is no direction in those to move along, and a
guess would be a wrong one.

**Decision 27. A horizontal surface is a surface: it looks up.**

`Stance::Flat` carried no direction, so `blit.wgsl`'s facing test was
skipped for it and a flat pixel took the whole of every pool that reached
it, from any side. Reported from the client as two walls "adding up" at a
corner — a bright diamond wedged between a lit face and a dark one.
Nothing adds; a fragment is lit once. The diamond is the corner's **top
cap**, a `Flat` static at the top of the wall, and it was lit by a lamp
standing two tiles *below* it as fully as one standing over it. Measured
at the reported corner before the change:

```
lid at z 25: through 1.000  facing 1.000
East:        through 1.000  facing 0.000
```

So a lid's normal is `(0, 0, 1)` and the facing test takes the third
component of an offset it already had — `blit.wgsl` computes the flame's
offset with `z` divided into tiles, which is the space the normal is
stated in, so what comes out for a lid is *how far above its plane the
flame is, in tiles*, through the same formula and the same `FACE_EDGE`
band. One `select` in the shader, one arm in `light::Surface::normal`.

**Still a half-space test and deliberately not a cosine.** UO's art is
pre-shaded: every wall's picture already has a light painted into it, so a
Lambert term would be a second light fighting the first. What this answers
is only which side of the surface the flame is on. The backlog carries
that argument since decision 22, and it is what keeps this a rule rather
than a lighting model.

`Spot` stopped carrying `Option<Face>` and started carrying a `Surface` —
flat, one of the four faces, or upright — which is exactly what the
attachment holds per pixel after `statics.wgsl` has resolved a corner.
`Upright` is still "nothing is known, so every flame lights it", and it is
still what a tree, a body and an unread wall get.

### Backlog: found at a house corner in Britain

Four things were reported from one picture — a lamp in the street at
`(1441, 1693)`, against the corner of the house whose corner tile is
`(1441, 1692)`. One of them is closed by decision 24 (see "The shadow ray
walk" archive) and two more by decision 25 above; what is left is the last
entry below. **Three of the four were the same missing fact**: `facing`
refused a corner graphic, so `0x0033` was `EDGE_ANY` in the grid and
`Stance::Upright` in the attachment, and every consequence below follows
from one of those two. It is measured now — `facing_of` answers
`Facing::Corner` — and the entries it closes are struck through with what
the fix turned out to be.

- ~~**A ray at 45° goes through a house corner into the room behind
  it.**~~ Closed by decision 24 (see "The shadow ray walk" archive). What
  is worth keeping of it is the shape of the report: the leak is a
  *stripe*, thinner than a tile and running the diagonal, so a per-tile
  diagram walks straight over it. `tests/onsite.rs` samples at a third of a
  tile for that reason, and that is what made it visible on the map rather
  than only in a built scene.
- ~~**A corner's two faces are lit as one.**~~ Closed by decision 25 above,
  and the estimate under it was right: widening the stance to four bits
  was three constants and no format change. What is worth keeping is the
  shape of the fix, because it is the shape any *pair* of surfaces in one
  picture will want — the corner exists in the instance word and nowhere
  else, and `statics.wgsl` resolves it per fragment, so every reader
  downstream of the world passes still sees one surface with one normal.
- ~~**A corner's pixels all claim the middle of their tile.**~~ Closed by
  the same. A corner's halves now map onto their own edges, so a run of
  wall, its corner and the run going the other way are one continuous
  surface — which is step 15's seam property arriving at the place it is
  most visible.
- **A corner's two faces are lit as one (as written).** `Stance::Upright`
  has no outward normal, so `blit.wgsl`'s `faces` is skipped entirely and
  both of the faces the art draws are as bright as each other — including
  the one turned away from the flame, which the corner itself occludes.
  Decision 22 fixed exactly this for a wall and cannot reach a corner,
  because there is nothing in the attachment to fix it with. What it needs
  is the **corner in the stance** — and the bits are not the obstacle,
  which is worth stating because it looks as though they are. Ten values
  need four bits where six needed three, and the stance rides at bit 16 of
  the instance's second word and at bit 8 of the attachment's third
  channel: both have eight or more spare above it, so widening the mask is
  three constants in `place.rs`, `statics.wgsl` and `blit.wgsl` and no
  format change at all. What the work actually is: `facing::face_of`
  answering a corner instead of refusing one — it has already measured
  both halves by the time it gives up — `Face::place_at` mapping a
  fraction per half, and `outward` choosing between the two by which half
  of the sprite a pixel is on, which the shader has as `across`.
  `occlusion::edges_of` then returns two bits and the pierce path handles
  a two-edge mask already.
- **A corner's pixels all claim the middle of their tile (as written).**
  The same `Stance::Upright`, and the other half of what step 15 gave a
  wall: a faced wall spreads its pixels along the edge it stands on and
  reads as one continuous surface with its neighbours, and a corner
  between two such runs is a flat 44-pixel band with a step at each of its
  two seams. It is the artefact step 15 removed from 76% of Britain's
  walls, still standing in the 24% — and a corner is the place it is most
  visible, because it always has a faced run on both sides of it to be
  compared against.
- ~~**The floor under a wall tile is lit from outside the house.**~~
  Decision 28 (see "The shadow ray walk" archive), and the entry's own
  shape is what was built — exempt a face and an upright, test a flat —
  with one bound it did not name: only a *named* panel, so the tree, the
  post and the barrel keep the answer they had. The pier question is still
  open and is now the only part of it left — see "The shadow ray walk"
  backlog.
- **The floor under a wall tile is lit from outside the house (as
  written).** Neither end of a ray is shadowed by the tile it is on
  (decisions 3 and 17), and the reason is about a *wall's* pixels: its two
  faces are one tile and there is no telling which of them a pixel is on.
  A **ground** pixel on that same tile is not ambiguous at all — it is the
  floor, it is inside, and the ray from it to a lamp in the street crosses
  the panel its own tile stands on. So the corner tile's own square of
  floor comes out fully lit against a dark room, which is the small seam
  on the ground the report ends with. The fix has a shape and it is cheap:
  the attachment already carries the stance, `light::Spot` already carries
  a face, so the exemption can be asked of the *pixel* rather than of the
  tile — exempt a face and an upright, test a flat. What makes it worth
  measuring rather than assuming is that a real floor is often a static in
  the grid itself, and a floor that shadowed the thing standing on it
  would be a worse artefact than the one being removed. The pier entry
  above is the same question from the other side.

### Backlog: found while giving a corner its two faces

- ~~**A lid has no normal, so it takes the whole pool from any side.**~~
  Decision 27 above, and the estimate held: the stance already told a lid
  from a face and the dot product already had a `z` in it.
- **The plan view said `Upright` while its own comment said "flat
  ground".** Found by the change above, which is the point worth keeping:
  a fixture that writes a different attachment from the world pass answers
  about *itself*, and it cost nothing at all until the day a stance meant
  something for a floor. It writes `Stance::Flat` now. Worth a look at
  every other synthetic attachment in the tests for the same reason.
- **A pillar in the open loses two of its four sides.** A solid filling
  its whole tile reads as a corner, which is right about the *picture* and
  half right about the *tile*: a building's corner has its north and west
  sides inside the house and a free-standing pillar's are in the street,
  so a ray clipping a pillar's far corner now passes where the length rule
  used to stop it. The two are one silhouette and no gate can tell them
  apart. What can is the **map**: a corner has a wall on the tile beyond
  each of its two panels and a pillar has open ground on all four sides,
  which `occlusion::collect` is already walking. Until then it is a sliver
  of light past a pillar against a room leaking into a street, and this
  file has taken the second every time.
- ~~**Decision 22's exemption is a whole row or column, and a street lamp
  can stand in one.**~~ Written down as a finding one session and reported
  from the client the next, with the coordinates: decision 26. Worth
  keeping as a note on how it was found, because the entry was written
  from *reading the rule* and the report came from *looking at the frame*,
  and the two arrived at the same line of code from opposite ends.
  `scene::house_corner_named_by_its_art` now stands where Britain does —
  the lamp due south — instead of standing clear of the exemption.
- **A corner is not in the elevation view.** `plan::elevation` unrolls one
  run of one face — `wall.face` is a single `Face` — so the instrument
  that made decisions 22 and 23 visible cannot draw the join a corner
  makes between two runs, which is exactly where a seam artefact would now
  show. It is the same shape as `mark_seams` and wants the run to be a
  list of faces rather than one.
- **A built scene still gets `EDGE_ANY` unless it is handed art.** Three
  scenes now carry silhouettes and the rest do not, so the backlog entry
  about thin coverage of the *panel* path is one scene better and
  otherwise unchanged. `scene::corner_art` is the place a fourth would be
  added.

### Backlog: found while giving a wall a side to be lit from

- **The sun still has no facing.** Decision 22 gives a flame one and the
  sunbeam does not ask: `sunlight` walks the grid and never looks at the
  normal, so every wall in a daylit frame is still lit on the side turned
  away from the sun. It is the same two lines and the same `outward`, and
  it is left because the sun is off by default and every scene that would
  judge it is a firelit one. *(Still true — see "Sunlight" in
  `lighting.md`'s Status.)*
- **The facing is binary where a real surface is a cosine.** What a wall
  gets is `1` in front and `0` behind with a fifth of a tile of gradient
  between, and a real surface lit obliquely gets less than one lit
  head-on. Lambert would be one `dot` more — but UO's art is pre-shaded,
  so a wall's picture *already* has a light in it, and multiplying by a
  second one is a decision that wants a scene rather than a formula.
- **A mobile has no facing either, and it has one on the wire.** A body is
  drawn as a billboard and lit as `Stance::Upright`, so a character walking
  through a pool is lit identically front and back. The direction is
  already parsed — `light::carried` uses it for the beam — so what is
  missing is the will to decide whether a paper-doll sprite should be
  shaded at all.
- **Three bits of the height channel are spent and five are left.** The
  stance took the first three of the eight a `z + 128` leaves free in a
  `u16`. Worth remembering before the next thing wants a channel: step 16's
  aperture asked for one (it went into a separate aperture plane instead —
  see "The occluding world").

### Backlog: the tread-normal investigation (decision 40, retired)

Found while building the treads (step 23.5), continuing straight from "the
user's actual complaint" investigation logged under "The occluding world"
archive below — kept together here because its conclusion is squarely
about the G-buffer's normal, not the occluder record.

**The open question, answered — `faces()`, not the occlusion walk.**
`examples/isolated_scene.rs` grew a profile mode
(`OPENSHARD_SCENE_PROFILE_FACE=north|east|south|west|flat|upright` plus
`_FROM`/`_TO`/`_STEPS`/`_LIGHT`): instead of drawing a frame it walks
`light::sample` along a segment and prints each `light::Reach`'s `through`
and `cone` — the same two numbers the corner investigation already leaned
on, read straight off the production function rather than re-derived. It
also prints `Occlusion::solids_at` for `_AT`'s own tile, so a segment does
not have to be guessed at from a picture — the stair in question
(`1497,1626,10`, filtered to `0x0739`/`0x0738`) came out as one lid
(`z 10..15`, whatever sits under the flight) and three tread strips split
along `y` with heights `1, 3, 5` (`z 15..16`, `15..18`, `15..20`), the low
one nearest south — `up: North`, confirming the reading `tread_box_of`'s
own test already carries.

Sampling `Surface::Face(South)` and `Surface::Face(North)` on the two riser
planes between those strips came back `cone: 0.000` everywhere along their
full height. Not a bug: `place()` puts a flame at its tile's *centre*
(`+0.5`, easy to forget doing this by hand — the first attempt at this did),
and this lamp's centre (`1498.5, 1626.5`) sits almost exactly on this
stair tile's own east edge — a full tile east of the risers' `x` and inside
the middle tread's own `y` span, so both risers' normals (`[0, ±1]`) are
close to perpendicular to the lamp and `faces()` clamps to zero everywhere
on them. The risers cannot be the hard edge here; there is no ramp on them
to be hard, they are simply always dark, which is correct — a step's riser
facing away from the only light nearby *should* be unlit.

Sampling `Surface::Flat` instead — walked across the three tread **tops**
from the low, south one to the high, north one (`(1497.5, 1626.83, 16)` to
`(1497.5, 1626.17, 20)`, 20 steps) — is where the cutoff actually is:
`cone` falls from `0.273` to `0.000` in the first three samples, a climb of
about `0.6` `z` units, and stays at `0.000` for the remaining seventeen —
while `through` is still climbing smoothly past it, reaching `1.000` around
the fifth sample and staying there until the next tread's own occlusion
box interrupts it. So the two hypotheses this investigation set out to
tell apart were told apart: **the facing cutoff is the hard edge, not the
occlusion ramp** — `through` never stops being the smooth, roughly-a-third
-of-a-tile ramp the corner investigation already measured, but `faces()`
gates the lamp off within the first tenth of a tread's climb and holds it
at zero for the rest of the flight, which reads as one lit step and then a
flat, matte run of tread tops above it — the report's "hard line", now
with a name and a place in the code (`FACE_EDGE`, `light.rs`).

**Settled by decision 40, not by widening `FACE_EDGE`.** Widening the
constant for `Surface::Flat` specifically was considered and dropped: it
is a tuning fix for one shape and would make an ordinary floor a full
storey below a lamp bleed light through the widened band it does not have
today. `faces()` itself was never the problem — `along = normal · toward`
is already a genuine physical distance in tiles from the surface's own
plane to the flame (`toward` arrives unnormalised, `light.rs`'s `offset`,
built in `sample`), so `FACE_EDGE` stays the one number it is, "how far
off the plane before you count as behind it," for *any* unit normal. What
was narrow is `Surface`: it could only *hold* three normals — none,
straight up, or one of the four cardinal horizontals — because every panel
and lid built before this step really did look one of those three ways. A
tread top is the first that does not: it is the top face of a box,
honestly horizontal, but the shape it is one step of is a ramp, and reads
to a light standing beside the flight as something other than a floor.

**Decision 40 (as originally written). A surface's normal is a value it
carries, not a tag naming which of three axes it must be — and it stays a
box on the tile grid regardless.** Written down because the alternative
was argued for at length in the same session that found the tread cutoff
(`FACE_EDGE`, above), and the argument deserves the answer on the record
rather than a re-litigation the next time a curved roof or a mountain
comes up.

The case *for* a general triangle mesh — arbitrary vertices, a BVH,
`Solid` becoming index buffers — was that this renderer already builds
real geometry (decision 39's boxes have eight real corners) and will build
more of it, so why cap the shape at three constant normals. The answer is
that every shape this world actually has is already a **box in the tile's
own coordinates** — decision 36 settled that for lids, bodies, treads and
footprints, and there is no graphic in a stock install whose art implies a
wall or a roof that is not one. What decision 36 left as a fixed constant
is the box's *normal*, taken from "which axis-aligned face did the ray
land on" rather than computed from the box's own geometry — and that is
the one thing a box does not have to be degenerate about. A **land tile
already carries four corner heights** (`WorldMap::land_corners`,
`crates/common/uofiles/src/map.rs:670`) and `light::Spot::flat`'s ground
case flattens them to one (`average_corner_z`) for *position* and has
never asked them for a *normal* at all — the slope a mountain's art draws
is real data this pass already reads and already throws away before
lighting sees it.

So the box's shading normal generalises from a fixed three-way tag to a
value computed from whatever vertices the box actually has — a tread's top
tilted by its own rise and run, a land tile's plane fitted to its four
corners — while the box stays exactly what decision 36 made it: anchored
to one tile's cell, found through the same grid, baked once per block
rather than raycast against per frame. That is the whole of what a
triangle mesh was being asked to buy and the whole of what it would have
cost twice over for it: a BVH replacing a grid that is free precisely
because every box in it is tile-aligned, and a ray-plane test becoming a
ray-mesh test that `blit.wgsl` and `light.rs` would each have to get
identically right for decision 9's parity to hold, for content —
arbitrary curvature — that the stock art never draws.

**Decision 35's ordering held, and was the gate on the land half of
this**: land was not in the occlusion grid at all, so a land tile's normal
had nowhere to be read from until it was. The tread half had no such gate
— a stair is a solid already, decision 36's table already lists it —
which is why the stairs were where this was proven first.

**Landed, CPU side — decision 40's steps 1 through 3.** `light::Surface`
gained a fourth case, `Sloped([f32; 3])`, a unit normal carried as a
value; `facing::Prism::tread_normal(index)` computed one tread's normal
from its own rise (the step from the previous tread's own height, or from
the static's base for `index == 0`) over its own run (`1 /
treads().len()`). `examples/isolated_scene.rs`'s profile mode grew a
fourth surface, `OPENSHARD_SCENE_PROFILE_FACE=tread` plus
`_TREAD_UP`/`_TREAD_HEIGHTS`, so the fix could be checked against the exact
scene the previous report came from rather than a fresh one.

**The sign was not obvious and the first guess was wrong.** The physically
tidy derivation — a hillside's own outward normal, leaning *back* over the
low ground it rose from, away from `Prism::up` — read every sample of the
reproduction's walk as `cone: 0.000`, worse than before this landed.
Tilting the other way — *towards* `up`, blending `Surface::Flat`'s `[0, 0,
1]` towards `Surface::Face(up)`'s own horizontal normal as the slope
steepens — took `cone` from the reported `0.273 → 0.000` cliff in three
samples to a smooth `0.715 → 0.584 → 0.453 → 0.321 → 0.190 → 0.059 → 0.000`
decay over six, tracking `through` the way decision 40 asked for. The
justification found afterwards, not before: the lamp this report is about,
and every stairwell fixture like it, sits mounted on the wall a flight
climbs *towards* — at a landing, or the top — not planted at its foot, so a
tread reading partly like the wall it climbs into is the ordinary case,
not the exotic one. Recorded in `facing::Prism::tread_normal`'s own doc,
with the wrong sign's measurement kept there too, since the derivation
that looked right was not — gone along with the function itself once
`gbuffer.md` step 5 retired it; the measurement survives only here now.

**Moved to their own plan, and closed there — not by porting `Sloped`, by
retiring it.** `Surface::Sloped` had no `Stance`/place-attachment encoding:
the tread's normal was real only on the CPU side (`light::sample`, the
profile tool). Chasing where a computed normal could fit in
`place::Stance`'s four spare bits led to the question of whether the
attachment's payload should be shaped that way at all — that question, and
decision 40's steps 4 and 5 as its first concrete case, became
[`gbuffer.md`](gbuffer.md)'s own plan. That plan's step 4c gave every
tread's top and riser real, honest per-face geometry and its own
unblended normal instead of one fixed tag standing in for the whole
flight; step 5 then re-measured this section's own reproduction against
that real geometry and found the hard cliff was a property of the fake
continuous-ramp sampling that went looking for it, not of the treads
themselves. `Surface::Sloped`, `Spot::sloped` and `Prism::tread_normal`
are deleted, not carried forward — see `gbuffer.md` step 5 for the
numbers. `Surface::shadowed_by_own_tile` never had to answer for a
`Sloped` surface as a result.

**Decision 40, reopened (2026-08-07).** The rejection above priced a
general mesh *for every occluder, uniformly*, and that arithmetic still
stands — a box remains free and remains the answer for everything decision
36's table covers. What no longer stands is treating the box as the
*ceiling* rather than the default: a mesh is now wanted for the shape a
box, or decision 41's several composed boxes, genuinely cannot state (a
curved roof, a mountain's slope). This is the "next time a curved roof or
a mountain comes up" this decision's own text named — see
[`lighting_geometry.md`](lighting_geometry.md) for the answer engaged on
the terms this paragraph asked for, not a rewrite of it.

**Decision 35 (as originally written). A sloped surface is deferred, and
the reason is that its consumer does not exist yet.** Four corner heights
instead of two, which is a second texel per surface; and the lid's
crossing test stops being a plane test and becomes a bilinear patch — two
triangles, a ray-plane test each and a containment test, in both
implementations. Worse than the arithmetic is what it reopens: the
strictness of the seam, `on_surface`, and the direction `stand_clear`
nudges a point are all *stated about an axis-aligned plane*, and each of
the three was a defect found the hard way.

And it would not buy the thing it looks like it buys. A roof in this
client is a slab five `z` deep and decision 24 deliberately keeps the
travelled-through rule for it, so a ray at 45° cannot step over it. What is
genuinely sloped in this world is the **land** — four corner heights per
tile — and the land is not in the occlusion grid at all (see "The
occluding world" backlog, "the land itself does not occlude"). So the
order is: land in the grid first, and slopes with it or not at all.

**Reopened (2026-08-05, mid `gbuffer.md` step 4b).** The rejection above
stands as a record of the price, not as a closed question any more:
inclined faces for roofs, land, and future custom geometry are wanted, for
the flexibility they buy. What this reopens is the render side's normal
format, not this decision's own reasoning about the occlusion grid's
crossing test — see [`gbuffer.md`](gbuffer.md)'s "Not settled" list for
where that question now lives (a general per-face normal, not the fixed
axis-aligned set decision 3 there assumed). This file keeps the argument
above as the reason the price was worth writing down; it is no longer the
reason the door stays shut.

## The occluding world: solids, the grid, and the bake

**Decision 3. An occluder is a whole tile, not a wall's edge.** ~~Superseded
by decision 17~~ — what follows is still why it was right at the time, and
its last paragraph is exactly what changed.

**3 (as written).** An occluder is a whole tile, not a wall's edge.
`client.md` proposed projecting each wall static to the segment its base
covers. The map cannot say which segment that is: **nothing in
`tiledata.mul` records which edge of its tile a wall stands on** — that is
only in the shape of the sprite. Guessing it from the art's silhouette is a
subsystem, and a wrong guess opens a corner of a room to the street.

The tile is the honest unit, and it is better than the segment in one way
that matters: a room's wall tiles form a *closed* ring by construction, so
no light leaks out of a corner. It is worse in one way that does not: a
pool stops up to half a tile early. The tile a light stands on never
occludes it — a sconce is a static on the wall's own tile, and a light
that shadowed itself would be dark.

**Decision 4. What stops light is what stops an arrow: `WINDOW |
NO_SHOOT`.** Not `BLOCK`. The two are different questions and the
reference answers them separately: ServUO's `Map.LineOfSight`
(`Server/Map.cs:3040`) tests statics with

```cs
if (t.Z <= pointTop && t.Z + height >= point.Z && (flags & (TileFlag.Window | TileFlag.NoShoot)) != 0)
```

— impassability never enters it. That is the right rule and it is better
than anything invented here: a barrel and a fence are `BLOCK` and you can
see over both, a wall is `NO_SHOOT` and you cannot see through it, and a
shard's custom wall gets it right for free. Reading `BLOCK` instead would
put a shadow behind every crate.

The grid carries an *opacity* byte rather than a flag, and it now carries
three answers rather than two: `NO_SHOOT` stops everything, `WINDOW` stops
a fifth (`occlusion::PANE`), and everything else stops nothing. That is
where this parts company with the reference on purpose — line of sight is
a yes or a no, so a window is a wall in it, and light is a fraction. A
window that stopped light makes a lit room read as a bunker and hides the
one thing a candle is for after dark. The fifth is a guess; there is no
number for it in any client file.

**A static the cutaway has taken away occludes nothing.** The same
`cutaway::shows` test the lights already run: a shadow cast by a wall that
was not drawn is a dark band with nothing making it, which is `client.md`'s
second unsettled question and this is the answer to it.

**Decision 5. An occluder carries the span of heights it occupies.** `z`
from the static and `z + height` from its tiledata entry — `height` being
ServUO's `CalcHeight` (`Server/TileData.cs:112`), which halves a climbable
(`Bridge`) tile the way `movement`'s `platform_surface` already does here.
A ray is stopped by a tile only where it passes *through* that span, so an
upper storey's wall does not shadow the ground floor and a cellar's wall
does not shadow the street. Where a tile holds more than one opaque
static, the span is their union — after the cutaway has already removed
the storeys the player is not on, that is one wall in nearly every real
case, and the union is conservative in the direction that darkens rather
than leaks. *(The "union of one tile's statics" model is later split apart
— see decision 30/step 21 below — but the height span itself, `z` to `z +
height`, is unchanged.)*

**Decision 29. What a cell should hold: panels, not one merged span.**
~~*(the shape of the next format change)*~~ — **superseded in its storage
half by decision 30**, which puts the panels in a list the texel points at
rather than inline in the texel. What it says about *why* a cell needs
more than one surface is why 30 exists, and it is kept for that.

A cell is `(z_bottom, z_top, opacity, PRESENT | edges)` — one `Rgba8Uint`
texel a tile — which is an axis-aligned box with four bits saying which of
its sides are real surfaces. That is already the "polygonal wall" this
pass needs and it is why a corner is a proper object in it: `(1441, 1692),
z 0..=25, sides E|S`. What it cannot say is anything a tile holds
**twice**:

- a lid and a wall on one tile merge into one span — conservative in the
  direction that darkens for the `z` and in the direction that leaks for
  the sides, which is not one direction;
- a window is a hole *in* a panel, so an aperture is a rectangle in that
  panel's own `(v, z)` — step 16, and the thing that makes a real shaft of
  light;
- two walls at different heights on one tile close the gap between them.

So a cell wants a small list of **panels**: a side, a `z` span, an
opacity, an aperture. The grid stays exactly as it is — a uniform grid
over a world whose every surface is tile-aligned *is* the acceleration
structure, and the walk, the per-frame build and the upload do not change
shape. What changes is the texel.

Two things to decide when it is picked up, and neither is decided here:
how many panels a cell may hold before it truncates (two covers a corner,
three covers a corner with a lid, and the tail wants measuring on Britain
rather than guessing), and whether the second plane `Occlusion::field_bytes`
already uploads is where they go. What is **not** on the table is a list
of boxes with an index of its own: the CPU is already thirteen times the
GPU on this pass and the grid build is most of it.

**Decision 30. The occluding world is a baked list of surfaces, indexed by
the tile grid — derived from the art, and overridable by hand.** *(decided,
then built — see steps 21 and 21.5 below)*

The grid was rebuilt **every frame** before this: `occlusion::collect` was
2.0ms of the pass's 3.3ms CPU against 0.31ms on the GPU. A house does not
change between frames; the camera moves. That is the strongest argument
for baking and it is not about freedom or effects — it is the largest
single number in this pass.

What baking with real geometry buys on top of that, and what it does not:

- **Sub-tile holes.** A window is a hole *in* a surface, so a real one
  needs a rectangle in the plane of a panel. Before this, a pane dimmed
  the whole tile, which is a dimmer tile and not a beam.
- **Baked light.** A sky field, an ambient occlusion, a lightmap for the
  static world — computed once per region rather than blurred per frame,
  which is what `docs/archive/render/lighting_world.md` does.
- **A shaft with a shape** (step 17, not built — see `lighting.md`'s
  Status): the mask can come from the opening's own geometry rather than
  from a tile-sized approximation.
- **It does not remove the G-buffer.** What is drawn is a sprite, and a
  sprite's pixels do not lie where a box's faces do — the art has
  thickness, ornament and overhang, 44 pixels of picture on a 22-pixel
  edge. The place attachment stays the bridge from a drawn pixel to a
  world surface, and the stance stays its normal. **Geometry replaces the
  occluder, not the source of normals.** *(the drawn sprite still stays
  the rasteriser's answer — [`gbuffer.md`](gbuffer.md) does not touch
  that. What it reopens is the second half of this sentence: the stance
  stops being the source of normals, and honest per-face geometry — this
  same box — becomes it, which decision 38.3 already called "consulted by
  the light and by the normal, and never by the rasteriser." Read
  together, not in tension.)*
- **It does not remove the measurement, which is the hard half.** A box
  has a window only if something read the hole off the art. Step 16 is
  that, it is the same machinery as `facing::facing_of`, and it comes
  first whatever the storage is.

**30.1 Derived first, authored as an override — and derived *offline*.**
The geometry is measured from the client's own art, so a stock install
gets windows with no assets at all; a shard that ships models overrides by
graphic. The engine must not require content the world does not come with
— and a hand-made mesh per building is thousands of assets, which is a
Community Pack's business. **When** that measurement runs is decision 31,
and the answer is not "in a frame".

**30.2 A surface is a quad with holes.** A plane (one of the tile's four
sides, or a horizontal lid), a `z` span, a span along the run, an opacity,
and up to `K` apertures as `(v, z)` rectangles in the surface's own
coordinates. That is the whole vocabulary the art can be measured into,
and it is what decision 25's corner, decision 27's lid and step 16's
window all are.

**30.3 The index stays the tile grid.** *(and decision 38 keeps it while
changing what it means: a cell's entries become references to solids that
need not lie inside it, so the grid stops being where geometry is stored
and becomes only where it is found)*

A texel becomes `(offset, count)` into the surface list; the DDA walk is
unchanged in shape and iterates a cell's two or three surfaces instead of
reading one merged span. A uniform grid over a world whose every surface
is tile-aligned **is** the acceleration structure — a BVH would put its
build on the CPU, which is the side that is already thirteen times the
GPU.

**30.4 Baked per block, and ~~per storey band~~ by block alone.** *(the
band is gone — decision 33 is why, and it landed before the bake did; the
bake itself is step 21.5 and `occlusion::bake`, and what it turned out to
also need is decision 37)*

The band was here because the cutaway removed the storeys the player is
not on *at the map walk*, which made a built grid one frame's: a cache
keyed by block alone would have been invalidated by walking through a
door, and keyed by band the cutaway could *select* rather than rebuild.
Decision 33 moved the cut to the end, so what a block holds is the same
for every frame and the key is the block. What the server changes — a
door's graphic, a ground item — stays in the per-frame path, which is
small and already exists.

**30.5 No storage buffers.** The ceiling is WebGL2
(`crates/client/render/src/lib.rs`): no compute, no storage buffers. So
the list is a **texture** read with `textureLoad`, and the bake is
CPU-side. This is the constraint that decides the format, and it is
written here because it is the one a session would otherwise design
around for an hour before finding it.

**The ceiling was questioned when decision 38 needed a second indirection,
and it was kept, at the time.** What it actually cost, item by item, was
close to nothing here:

- **Compute shaders** — unused. The bake is on the CPU, per block, and the
  largest thing in it is the paste rather than the build.
- **Atomics and writes from a shader** — unused, for the same reason.
- **Storage buffers** — the only real loss, and in this pass a storage
  buffer is precisely "an array read at a computed index from a fragment
  shader", which a texture already is. `textureLoad` from an integer
  texture plus the address arithmetic is the difference; it is a dozen
  lines, not a millisecond.

So the indirection decision 38 needs is a cost of the *model*, not of the
floor, and it would be paid on WebGPU too. Two things worth writing down
beside that. It is **not a WASM limit** — WASM has no opinion about GPUs;
this is a backend choice, and `wgpu` targets WebGPU as readily. And the
place the floor *would* bite is a GPU-side bake, GPU light culling, or
per-frame variable-length lists — none of which is in this plan.

What was left for a person rather than for this file, at the time: the
sentence in `crates/client/render/src/lib.rs` read as a principle and was
a dated assumption — WebGPU was behind a flag when it was written and was
broadly shipped by the time this was reconsidered. The question under it
was not a graphics question: **is the web still a target?** If it is, this
floor is right and the texture indirection is the price. If it is "one
day, perhaps", saying so plainly is cheaper than carrying the constraint
through every decision and discovering later that it defended nothing.
Keeping *both* backends would be the worst of the three: WGSL has no
preprocessor, so a second fetch path means a generated shader or
`naga-oil`, which is a real cost paid for tidiness.

**Answered.** Asked again while planning [`gbuffer.md`](gbuffer.md): the
web is still a target, but the ceiling this crate is written to is
**WebGPU, not WebGL2** — `crates/client/render/src/lib.rs`'s own module
doc said so first and is the record of it. Compute shaders and storage
buffers are back on the table for anything written from here on — this is
the reasoning behind `lighting.md`'s current statement that shaders now
target WebGPU.

**What this does not do: touch what decision 38 already built.**
`Occlusion`'s texture-folded lookup (`LIST_ROW`, `solid_at`) was not ripped
out when the WebGPU ceiling was confirmed — it is real, tested, running
code, and un-building it is its own piece of work with its own risk, not a
free side effect of a floor changing on paper. It stays exactly as it is,
as a still-valid technique, simply no longer the *mandatory* one for what
gets written next. If it is ever worth simplifying to a plain storage
buffer too, that is a deliberate, separate piece of work to pick up on its
own — not implied by the ceiling change.

**30.6 The truncation is measured, not chosen.** How many surfaces a cell
may hold comes from a distribution printed over Britain, and whatever is
dropped is *logged* rather than silently capped — a grid that quietly
truncates reads as "covered everything" when it did not.

**30.7 The walk's rules carry over untouched.** Decisions 17, 18, 23, 24,
25, 26, 27 and 28 (see "The shadow ray walk" and "The G-buffer bridge"
above) are already stated about *surfaces* — a panel is pierced, a body is
travelled through, a surface does not shadow itself, a face is one-sided,
a lid looks up. That is what several sessions bought and it is why this is
a change of representation rather than a rewrite: the rules do not get
relitigated, and the parity test keeps holding both implementations to
them.

**30.8 A hole is a plane beside the list, not four more channels of it.**
*(decided in step 21.3, and the format is what it decides)*

A `Surface` texel is four `Rgba8Uint` channels and all four are spoken for
— `(z_bottom, z_top, opacity, PRESENT | edges)`. A rectangle needs four
more, so the question is where they live, and there were three answers:

- **Interleave**: two texels a surface, the hole in the second. No new
  binding and no new upload, and it doubles the footprint of *the one
  texture the walk reads in a loop* in order to carry zeros — because a
  hole is what almost nothing has.
- **A third kind of list element**, a texel the count includes and the
  walk skips. Costs nothing when there are no holes, and it makes a cell's
  `count` mean texels rather than surfaces, so `histogram`, the truncation
  cap and decision 30.6's distribution all quietly start counting a
  different thing.
- **A parallel plane** over the same indices, read only where a bit on the
  surface says there is something to read. One more binding, one more
  upload, and the hot loop is untouched.

The third, and the deciding argument is the one this pass makes everywhere
else: **a miss must be cheap.** `HOLED` is a spare bit of a byte that
already had three; the plane is written only when
`Occlusion::any_aperture` is true, so a frame of a map with no measured
window neither lays it out nor sends it; and a surface with no hole costs
one bit test in the shader. The two planes are grown together and never
apart, because they are one list indexed by one number.

**Decision 33. What a ray may cross and what the frame draws are two sets,
and the cut between them is at the end.**

Decision 4 said nothing occludes that was not drawn, and it is still true
of the picture. What was wrong was *where* it was decided: `collect` asked
`cutaway::shows` at the map walk, so what came out of a `Builder` was one
frame's grid — and a per-block cache of one frame's grid is not a cache.
That is the whole of what 30.4's storey band was working around, and the
band would have had to be re-argued the moment a ray was allowed to cross
a storey the frame did not draw.

So the walk builds **what a ray may cross** — every surface standing on
the map inside the rectangle — and `Builder::finish` applies the frame's
`Cutaway` as it packs. Everything above that line is a fact about the map
and can be built once and kept; everything below it is a fact about the
tile the player is standing on and costs one predicate per surface, on a
copy that was already happening.

Three things this decides rather than assumes:

- **The cut needs two facts and a surface now carries both.**
  `Surface::bottom` is the `z` the static stood at, and `Surface::roof` is
  the flag a roof is cut by at any height. Nothing in the walk's rules asks
  either — `roof` exists for this and says so.
- **The rule has one spelling.** `Cutaway::shows_at` is `shows_static` with
  the tiledata row already read, and `shows_static` calls it. A second
  copy of "at or above `max_z` it goes, and a roof goes once the player is
  under one" in the occlusion module would be a second policy, not a
  second caller.
- **The draw ceiling does not move.** The other half of `cutaway::shows` —
  a static past `DRAW_CEILING`, or one the client marks internal — is a
  fact about the static and not about the player, so it stays at the map
  walk as `cutaway::drawn_in_any_frame`. A mountain top a hundred and
  fifty `z` up is drawn in no frame from any tile, and no cache wants it.

What this does **not** decide is whether a ray *should* cross a storey the
frame took away. It stays exactly as it was: the frame's grid is the drawn
set, the sky field is not (`lighting_world.md`'s decision 3), and the two
are as far apart as they have always been. What changed is that the
question is now asked in one place, on one line, over a list that already
exists — so the day light is made to reach the storey above a torch, that
is a change to which set `finish` keeps and nothing else.

**Decision 34. A body has a footprint, and the art can only measure one
axis of it.**

A surface is a plane on an edge, a lid, or **the whole tile**, and the
third is a fallback: `facing_of` refuses a picture it cannot read an edge
in, and what the grid then does is stop light across the entire square.
Measured on Britain's `1509,1635` — the tile a person pointed at because
it was the one lit thing in a dark house — the graphic is `0x00CC`, whose
silhouette occupies **columns 12 to 31 of 44**. Twenty columns of art
became an occluder across the whole tile, standing among neighbours that
are panels on one edge. It over-blocks in every direction at once, and the
view shows it as the odd shape it is.

So a body gets a **footprint**, and the whole of the decision is what can
honestly be measured for one. The projection is what says: world `+x`
moves the screen by `(+22, +22)` and `+y` by `(-22, +22)`, so a sprite's
**column** is `(fx - fy)` and nothing else. The other diagonal, `(fx +
fy)`, is depth — a single picture cannot say how far back a thing goes,
and inventing it would be decision 3's mistake made again.

A footprint is therefore a **band across the tile in the `(fx - fy)` axis,
unbounded along the other** — which is exactly the shape a panel's run
already is, one axis measured and one refused. It is `(near, far)` in
`RUN_STEPS`ths, the same units and the same byte pair a `Hole` carries.

What it costs, and why this is the cheap one of the two:

- **The measurement is a pass that already happens.** `facing_of` scans
  the silhouette by column; the band is the first and last column with a
  pixel in it.
- **The format already fits.** The surface texel is full, but the
  *aperture* plane beside it is `(near, far, bottom, top)` per surface and
  is allocated only when something has a hole. A body's footprint is two
  of those four numbers, so it rides in the same plane under a flag of its
  own, and no texture grows.
- **The walk gains one clip.** A body is travelled through, so what
  changes is the length: the segment inside the cell is clipped to the
  strip. Closed form, exact, a few ALU. The side pierce of decision 24
  moves with it — the sides that stop a ray are the strip's own boundaries
  rather than the tile's.
- **A full-width picture gets no footprint at all.** The band is only
  written down when it is narrower than the tile, so every body in the
  world behaves exactly as it does today unless the art says otherwise.
  That is the direction this file takes at every fork.

**Decision 36. An occluder is a box in the tile's own coordinates, ~~and a
plane where the art cannot say how deep it is~~.** *(the first half stands
and is the reason this decision exists; the second half is withdrawn —
decision 38 is why, and it also takes the "in the tile's own coordinates"
out of it)*

The rules of this grid have grown one shape at a time and each one arrived
the same way: not as a new rule about light, but as a **form the surface
record could not state**, faked with a flag. A corner became two panels
(decision 25). A wall got a hole (21.3). A body got a footprint (34). A
stair is a solid, and its treads are a shape there is still no way to
write down. Five special cases, one after another, and none of them was
ever about how a ray behaves.

So the record becomes a shape: a **box**, `(u0..u1, v0..v1, z0..z1)` in
the tile's own unit square, with an opacity. Everything the grid holds
today is that box with two of its six numbers pinned:

| today | as a box |
|---|---|
| a lid | zero height |
| a body | the whole tile |
| a tread | part of one axis |
| a footprint (decision 34) | part of the other |

And the walk gets **simpler**, which is the argument that matters more
than the tidiness: `blit.wgsl` had three rules — pierce a plane, travel a
span, clip to a strip — and a ray against a box is one slab test, three
pairs of comparisons, closed form. The same box gives the *shading* half
its answer for free: a pixel's normal is the normal of the face it landed
on, which is what `place::Stance`'s nine values were a hand-rolled
enumeration of.

~~**A panel stays a plane, and that is the one thing not folded in.**~~
*(withdrawn — the argument is left standing so that the next person to
reach for it finds it already answered)*

~~A wall's thickness is not in the art — decision 3 is that argument and it
has not changed — so a box for a wall would need a depth somebody
invented. Worse, a zero-thickness box is not the same test as a plane:
"the segment overlaps a slab of width zero" is a numerical coin toss where
"the segment crosses this plane" is exact, and the seam rules (decision
16, `on_surface`, `stand_clear`) are all stated about a plane and each was
a defect found the hard way. Two primitives, then, and the pair is honest
about which one we can measure: a plane where only one axis is known, a
box where the shape was fitted whole.~~

Two things are wrong with it and the second is the interesting one.

**The coin toss is an argument against zero, not against a box.** Give a
wall a thickness of two forty-fourths of a tile and the slab test is
exactly as well-conditioned as the plane test. What the paragraph did was
insist the box be degenerate and then object to the degeneracy.

**And "the art cannot measure it" is a bound on a *detector*, not on a
record.** Decision 3 is right and unchanged: no single sprite says how
deep a wall is, and a detector that invented a depth would be making
decision 3's mistake. But this whole track is the authoring one — decision
31.2's `authored` row already exists and already wins — and the moment a
person can write six numbers, "unmeasurable" stops being a property of the
model and becomes a property of the *fallback*. Provenance is then a
column of the table, not a second primitive in the shader.

So the vocabulary is **one solid**, and a derived one is a solid with one
measured axis and a nominal other. What is still not folded in is the
**hole**: it is a subtraction rather than a body, a box with a bite out of
it is two primitives or one exception, and the exception already works.

What it costs:

- **The surface texel doubles.** Four bytes today (`bottom`, `top`,
  `opacity`, `edges`); a box needs six or seven. The aperture plane
  already exists beside it and decision 34 already plans to put
  `near`/`far` there, so this is a second texel in an existing plane
  rather than a new texture: ~140KB to ~280KB at the widest zoom.
- **A hole is still not a box.** It is a *subtraction*, and it keeps its
  own field. A box with a bite out of it is two primitives or one
  exception, and the exception is the one that already works.
- **Nothing may move in the picture on the way.** The migration is:
  express the four existing kinds as boxes, keep every current test green
  — they are the specification of what must not change — and only then
  let a tread be a box that is a part of a tile. A step that changes both
  the representation and the picture is a step where a difference cannot
  be attributed.

Where this parts company with decision 35: that one deferred *slopes*,
and it is still deferred and still right — a bilinear patch reopens three
rules that each cost a day. A box does not. A flight of steps is
horizontals and verticals, which is exactly what this world is made of,
and the shape that was missing was never a slope.

**Decision 37. What invalidates the bake is the *art*, and the art has a
revision.**

Decision 33 made a `Builder` a fact about the map, and decision 30.4 read
that as "so a block can be built once". Both are true and neither is the
whole of it: a surface is derived from the map **through the atlas** —
which edge a wall stands on, the hole in it, the solid a stair is — and
`occlusion::shape_of` falls back to the whole-tile answer for a graphic
the atlas does not hold.

**An atlas grows.** A graphic the camera has not reached yet is not in it,
so a block baked a second before that graphic was packed holds `EDGE_ANY`
where the atlas can now name a face — and nothing about the baked block
would ever say so. The wall would stay a body for as long as the player
stood still. That is the whole class of bug a cache has that a rebuild
does not, and it is the quiet kind: the picture is a *little* wrong, in a
way that looks like the detector failing rather than like a cache being
stale.

So the fact the bake depends on is given a name and a counter.
`StaticAtlas::revision` counts changes to exactly the three answers
`occlusion::Shape` is made of — a facing, a hole, a prism — and a `Bake`
keeps the revision it was built under and drops **everything** when it
moves. Three things about that shape are deliberate:

- **A counter and not a comparison of contents.** "Has the atlas changed"
  asked of the maps themselves is a scan of a few thousand entries every
  frame, to answer no.
- **Bumped where something is actually packed, not per call.** The app
  offers the atlas every visible graphic on every frame, and a bump per
  *call* would tell the bake its shapes had changed sixty times a second —
  a cache that is cleared every frame is not a cache, and it costs exactly
  what having none costs while looking like it works.
- **Pixels are not in it.** A dirty row is a texture upload and changes no
  geometry.

The map itself is the other input, and it is not versioned: a `Bake` is
one map's, the caller owns that, and this client has one map. That is
stated rather than enforced because the alternative — a map that could
tell you it had changed — would be a facet-wide dirty bit for a case the
client does not have.

**Decision 38. The tile grid is a broadphase index, and a solid is a body
of the world that no cell owns.**

Decision 36 made the record a shape and left it *in the tile's own unit
square*. That last clause is the one carrying the damage, and it is worth
naming what it has cost, because the bill is already in this file: **every
seam here was manufactured by cutting geometry on a tile boundary.** The
spokes of decision 18 were a ray slipping between two panels that meet at
a corner — and there is a corner there only because the wall was cut where
the map's storage happens to be cut. Decision 16's fraction of exactly
one, `on_surface`, the direction `stand_clear` nudges a point: three
rules, three days, all of them about what happens *at a cut*.

So the solid stops being cut. A solid is a box in **world** coordinates
with its own six numbers, and a cell holds **references** to every solid
whose extent touches it.

**38.1 Reference, not clip — and that is the whole of the argument.** A
ray crossing the join tests the same one solid from both cells and gets
the same answer twice; there is no hairline left to slip through, and the
fix is a property of the representation rather than a fourth rule about
seams. A solid overlapping four cells is referenced four times, which
costs four `u16`s. It was cut into four pieces before, which cost four
records *and* the seams between them.

The walk is unchanged in shape: the DDA of decision 14 still steps cells,
a cell still yields a list, and the test is still one slab test. A solid
spanning two cells may be tested twice on one ray; a visited-set that
avoided that would cost more, on a ray of a dozen cells, than the
redundant test it saves. So it is not deduplicated, and the test being
exact is what makes that safe.

**38.2 A solid is anchored, and its reach is *measured* rather than
limited.** The anchor — the tile the static stands on — is the whole of
the invariant a solid needs, because it names the block that owns it. How
far the solid extends past it is nobody's business but the geometry's.

~~A solid may not extend further than one tile beyond its anchor.~~ *(an
invented constant, withdrawn the day it was written. Decision 30.6 says
the shape of the answer: measured, not chosen.)*

What genuinely needs a number is not the model but the **bake**. Blocks
are baked independently (30.4) and a frame pastes the ones it needs, so a
solid anchored in block `A` and reaching into block `B` puts references
into `B`'s cells that only `A` can supply. The frame therefore pastes a
**ring** around the blocks it wants, and the question is how wide the ring
is.

It is measured, and the measurement is free: a solid belongs to a
*graphic*, so the widest reach in the whole world is `max` over the
table's solids and is known before the first block is baked. Zero on a
stock install; one after somebody authors an arch; three if somebody
authors a bridge. Nothing is refused and no graphic is special-cased — a
large solid simply costs what it costs, every frame, and the ring is
exactly as wide as the content made it. This rides on the fact decision 37
already tracks: the table changes, the radius is recomputed and the bake
is dropped, one path.

The bookkeeping is the owner's. A `Baked` block carries, beside its cells,
a small **spill** list of the references reaching outside its own bounds;
the frame pastes the ring's spill and nothing else of it, which for every
block in a stock install is empty.

The one thing that must not happen quietly is a person paying for a reach
they did not intend, so the radius is **logged**: one line saying how many
blocks wide this table makes the ring. A cost that is visible is a cost
somebody can decide about; a silent one is how a frame gets slower for a
reason nobody can name.

**38.3 The pixel's face is the same slab test.** The projection is
orthographic, so "which face of the solid is this drawn pixel on" is a ray
from the camera through the pixel against the same box — the same
arithmetic, the same code, a different origin. That is what gives the
stepped lid of `0x0736` three horizontal treads instead of two vertical
half-walls, and it is why `place::Stance`'s nine hand-enumerated values
become a derived answer instead of a taxonomy to extend. One-sidedness
(decision 22) stops being a rule at the same time: a box's back face is
real, and the artist simply drew no pixels that land on it.

Note what does **not** change: the drawn frame. `statics.wgsl` puts the
sprite where it put it, the G-buffer is still the bridge from a pixel to a
world surface (decision 30's fourth bullet), and the camera has no opinion
about any of this. That is exactly why the freedom is affordable — a
solid is consulted by the light and by the normal, and never by the
rasteriser.

**38.4 The format grows one indirection, and it is the model's cost, not
the floor's.** A cell becomes `(offset, count)` into an **index** plane of
solid ids, and the ids address a **solid** plane. Two textures where there
is one, one more `textureLoad` in the walk. Decision 30.5 carries the
measurement of what WebGL2 costs here, and the answer is that a storage
buffer would be a tidier spelling of the same fetch.

The count of solids goes **down**, not up: a flight of five tiles of
stair is one solid, not five; a run of wall is one per graphic instance
rather than one per cell it crosses. The 18,071 surfaces over 10,212 cells
decision 30.6 measured were largely an artefact of tile-shaped storage,
and that distribution was measured again after the migration rather than
assumed to carry over (see step 23.1's own re-measurement below).

**38.5 Nothing may move in the picture on the way, and the migration is
therefore two steps and not one.** First the ownership changes with the
geometry held still — cells reference solids, every solid is exactly the
box its surface was, every scene and the parity test green. Only then may
a solid be a shape no surface could have been. A step that changes both
where geometry lives and what it is, is a step where a difference cannot
be attributed, and this file has said that twice already for smaller
changes.

**Decision 41. A shape a single climb profile cannot describe gets a
second, independent kind of solid, authored rather than derived — not a
wider `Prism`.** Step 23.4's own instrument needs something to author for
an arch, and `Prism` cannot be it: `Prism::height_at(run)` is a function of
one axis, monotonic by construction — that is exactly what makes it a
*climb*, and exactly what makes it unable to state a post, a gap, and
another post. Widening `Prism` to fit an arch would mean giving a
staircase's own model a discontinuity it has never needed, in exchange for
an escape hatch every future irregular shape would be tempted to squeeze
through the same way.

So: `facing::Block`, a plain axis-aligned box in a graphic's own
tile-local coordinates — `x` and `y` in eighths of the tile, `z` in the
same units `Prism::treads` already uses — and `facing::Blocks`, a fixed
array and a count exactly the shape `Prism` holds its own treads in, so a
`Shape` carrying some is still `Copy` and costs no allocation on the path
from the table to the grid (the reason `Prism` is not a `Vec` in the first
place, stated once for both). `occlusion::Shape` gains a fourth field,
`blocks: Blocks`, beside `prism` rather than folded into it — a graphic
may carry both, because a stair's own base can still misread as a corner
independently of whether some *other* graphic needs an arch's shape.

**Never derived, on purpose, and that is the whole of why it needs no
gate.** `prism` needs `CLIMBABLE` and a score before it is believed,
because `Shape::of` proposes one automatically and an automatic proposal
can be wrong. Nothing proposes a block list automatically — there is no
search over it the way `facing::best_prism` searches prisms, only a
person placing boxes by eye against a silhouette in step 23.4's instrument
— so there is no wrong reading to gate away from. `Builder::add` does not
consume `blocks` yet; per decision 38.5's own discipline, the plumbing
lands before its first user, the same way decision 38.2's spill did.

**The format bumps to four**, and `facing::DETECTOR` does not: nothing
about `facing_of`, `aperture_of` or `prism_of`'s own gates changed, and a
block is never derived, so no old table can describe a rule this session
changed. `arttable.rs` has the grammar (`block x0 x1 y0 y1 z0 z1`, zero or
more, any verdict) and the round trip; the silhouette a person judges a
candidate against is `facing::blocks_silhouette`, drawn the way
`prism_silhouette` is, generalised to let two blocks draw the same column
at different heights — a lintel floating over the gap between two posts,
which a climb profile's always-touches-the-ground assumption cannot.

### Steps: the surface list, the spill, and the solid (steps 1, 21, 22, 23)

- [x] **Step 1. `render/src/occlusion.rs`.** The tile grid of decision 4/5,
      built from the map, the tiledata and the cutaway over the bounds
      `light.rs` already computes. Pure CPU, no GPU types, tested without
      client files: the builder takes occluders one at a time and the map
      walk is the caller.

- [x] **Step 21. The surface list.** Decision 30, **and it was five changes
      rather than one**. They are listed in the order that kept every one of
      them testable on its own, and nothing here waited on anything else:

      1. ✅ **The list and the walk over it, with the union kept.** A cell
         stopped being one merged span and became `(offset, count)` into a
         list of surfaces; both walks iterate a cell's one or two or three.
         The picture did not move, which is the whole point of doing it
         first: a cell maps one-to-one onto surfaces — a lid is one
         horizontal, named sides are a quad each with the same span, and
         `EDGE_ANY` is one **body** rather than four quads, which is why
         the list has two kinds of element exactly as the walk has two
         rules. Every existing test stayed green, the parity tests
         included, so the break it could have made would have landed in
         the plumbing and nowhere else.

         What it is made of: `occlusion::Surface` and `occlusion::Builder`
         — the merge now lives in the builder and only there, which is
         what makes 21.2 a change to one function. `Occlusion::at`
         survives as the **merged view**, folded on demand for the readers
         whose question is genuinely about a tile: the wireframe overlay,
         the plan view, and which way a mounted flame steps out of its own
         cell. Three textures instead of two — the grid is the index
         `(offset & 255, offset >> 8, offset >> 16, count)`, and the list
         is a texture `SURFACE_ROW` wide read with `textureLoad`, which is
         decision 30.5 arriving. A cell's surfaces are combined with
         **`max` and not a product**: two panels on one tile are two faces
         of one corner, and a ray crossing both has gone through one thing
         once.

         And the first number for decision 30.6, off Britain at the widest
         zoom: **10,212 standing cells hold 10,653 surfaces**. Four hundred
         and forty one cells in a city block carry more than one, which
         says what the union has been merging away — and what 21.2 is
         about to multiply.

         The cost, measured on the same frame, is the new baseline rather
         than a comparison: `light::collect` 3.37ms of CPU with
         `occlusion::collect` 2.06ms of it, 0.05ms to lay all three planes
         out as bytes, and on the GPU `copy` 0.181ms, `dark` 0.254ms,
         `night` 0.368ms, `sun` 0.514ms. No like-for-like before-and-after
         was taken — the scene had changed since step 6's numbers, so the
         two were not comparable, and what would want watching is that the
         walk now reads two texels a cell where it read one. Step 21.5 is
         where that is bought back several times over.
      2. ✅ **Split the union.** Two statics on one tile stopped merging
         into one span with one mask. This is the one place the picture
         *had* to change — it is the backlog's "a cell merges a lid and a
         panel into one mask and one span" — so it is its own change with
         its own test, and not smuggled in under a refactor that claimed
         to change nothing.

         The union was wrong in two directions at once and the change
         closes both. For the **span** it was conservative: two walls with
         air between them closed the gap, so a frame carried a band of
         shadow with nothing in the picture casting it. For the **mask**
         it leaked: a floor over a wall tile handed its `z` to the wall's
         span and lost its own lid-ness, so the walk pierced a horizontal
         surface as though it were a vertical panel and travelled through
         nothing — and a pane beside a wall came out opaque across the
         whole tile, because the opacity was a `max` too.

         `occlusion::Builder::add` now decides what a static *is* — a lid,
         a body, or a panel per side its art named — and pushes it.
         Nothing merges. What is left of the fold is `Occlusion::at`, the
         **merged view**, which is unchanged and is what the wireframe,
         the plan view and `light::mounted_at` go on reading: their
         question is genuinely about a tile. A tile's surfaces live in a
         linked list in one arena rather than in a `Vec` a tile, and that
         is a cost decision — 35,000 tiles at the widest zoom would
         otherwise be 35,000 allocations a frame on the side of this pass
         that is already thirteen times the GPU.

         Three tests pin it and each fails on the union: two walls keep
         the air between them
         (`two_occluders_on_one_tile_stop_closing_the_gap_between_them`), a
         lid and a panel keep their spans and their two rules
         (`a_lid_and_a_panel_on_one_tile_are_not_one_surface`), and the
         walk itself passes a ray through the gap
         (`a_ray_through_the_gap_between_two_walls_on_one_tile_passes` —
         built by hand rather than out of a scene, because two statics on
         one tile is the thing a `WorldMap` makes fiddly and a `Builder` makes
         one line). The union was put back for a run to check they were
         red, and they were.

         **The distribution decision 30.6 asked for**, Britain at the
         widest zoom — `tests/cost.rs` prints it now, and
         `Occlusion::histogram` is what it asks:

         ```
           surfaces   cells      share
                  1    5942      58.2%
                  2    2702      26.5%
                  3     759       7.4%
                  4     428       4.2%
                  5     164       1.6%
                6–10     186       1.8%
               11–21      31       0.3%
         ```

         10,212 standing cells hold **18,071** surfaces, against 10,653
         under the union. Nothing was dropped, and the cap is the format's
         own byte rather than a number anybody chose: the worst tile in a
         city is 21, an eighth of what an `(offset, count)` can name.
         `Occlusion::dropped` counts what does not fit and `cost.rs`
         prints it — a grid that quietly truncates reads as "covered
         everything" when it did not.

         **The cost, and it is not free.** On the same frame and the same
         machine as 21.1's numbers: `light::collect` 3.43ms against 3.37,
         the grid 2.19ms against 2.06 — the walk that builds it is
         unchanged and what grew is the list. On the GPU `night` is
         **0.497ms against 0.368**, which is the backlog's "a cell's fetch
         count went from one to `1 + count`" arriving with a count that is
         now 1.77 rather than 1.04. It is still 3% of a 60Hz budget, and
         step 21.5's bake is where the CPU half is bought back.
      3. ✅ **The aperture in the walk, tested on a built scene.** A
         surface got a rectangular hole — `occlusion::Aperture`, a span
         along the run and a span of `z` in the surface's own coordinates
         — and the crossing test asks whether the ray went through it. No
         art was needed, exactly as planned: `StaticAtlas::state_aperture`
         is the seam step 16 fills from a silhouette, and a scene states
         one directly.

         **The change is small because decision 30.7 said it would be.** A
         panel was already *pierced at a point* rather than travelled
         through, so the point was already being computed; what step 21.3
         adds is that the point has two coordinates instead of one and is
         asked about a rectangle. `light::pierced` and `blit.wgsl`'s are
         the whole of it, and everything above them — `own_run`, the
         corner case, the body's second answer, the sun — reaches them
         unchanged.

         Four things were decided along the way, and each is a refusal
         rather than a mechanism:

         - **Only a named panel may have a hole.** A lid is horizontal and
           a body is "it stands up and the art would not say which way",
           so neither has a plane for a rectangle to be stated in.
           `Builder::add` drops one offered to either — decision 3's
           refusal arriving one level down.
         - **A corner carries it on both of its panels.** They are the two
           faces of one picture, so a hole measured off that picture is
           the same window seen from either side, and nothing in a
           silhouette says which half it was in.
         - **The run coordinate is a byte**, `occlusion::RUN_STEPS`, a
           two-hundred-and-fifty-fifth of a tile — finer than the seven
           bits the place attachment carries a *pixel's* fraction in.
           Quantised once, in `Aperture::new`, so that both walks read the
           same byte and divide it by the same number: the parity test is
           exact rather than to a tolerance.
         - **A hole's edges soften symmetrically**, which is why `inside`
           is a second function beside `pierces` rather than a call of it.
           `pierces` hangs its band below the bottom edge because a wall
           is based on the ground and the ray a person looks at runs along
           that base; a hole's edges are in the middle of a surface and no
           ray runs along them, so a band centred there would move the
           hole half a penumbra downwards.

         Held by five tests, each run against the mutation that should
         break it: two aim a ray by hand
         (`a_ray_through_a_hole_in_a_wall_passes_and_one_beside_it_does_not`
         for the run and
         `a_ray_over_a_hole_in_a_wall_is_stopped_by_the_wall_above_it` for
         the height, which is the axis no picture of a floor can ask
         about, because a floor pixel and a flame are both near `z = 0`
         and every ray in that picture crosses at one height). One is the
         scene: `scene::wall_with_a_hole_in_it` is `torch_before_a_wall`
         with the middle tile's graphic swapped for one that carries a
         hole, so the wall either side is the same graphic at the same
         height and a fan that appeared without the hole would be some
         other defect. It asserts the fan is there, that the tiles either
         side are at the ambient exactly, and — measured as the width at
         half the sweep's own peak, because a hole this size is seen
         through a penumbra of about its own width — that it is **wider
         three and a half tiles out than one and a half**. Two are the
         format: `only_a_named_panel_carries_a_hole` and
         `a_hole_is_uploaded_at_its_own_surface_s_index`, the second
         because a shader reading the hole plane at the wrong index would
         draw something everywhere and be wrong only where a window is.
         And the GPU parity test has a sixth fixture,
         `the_shader_and_light_sample_agree_about_a_hole_in_a_wall`, which
         goes red when the shader is made to ignore the hole.

         **The cost was nothing measurable, and the reason is decision
         30.8.** No graphic in any install had an aperture until step 16
         landed, so `any_aperture` was false, the plane was neither laid
         out nor uploaded, and the `HOLED` bit was never set — what a real
         frame pays is one bit test per pierce.
      4. ✅ **The tool, the table and the measured aperture** — steps 20b
         and 16 (see "The art-measurement pipeline" archive for the full
         write-up).
      5. ✅ **Bake it.** Decision 30.4's block cache, and it is **1.22ms to
         0.37ms** on the frame the breakdown below was taken on.

         `crates/client/render/src/occlusion/bake.rs`. A `Bake` holds one
         `Baked` per map block — the surfaces its statics stand and the
         sky they take, in cell coordinates so the same bytes serve a
         frame at any offset — and `bake::collect` assembles a frame by
         pasting the blocks its rectangle overlaps, then does the three
         things that are genuinely per frame: the server's ground items,
         the blur, and the pack with the frame's `Cutaway`. Everything a
         block holds goes through the same `occlusion::place` the
         uncached walk uses, which is now one function rather than two
         copies of a pair of lines.

         **The property it rests on is equality and not similarity**, and
         it is asserted twice. `a_baked_grid_is_the_one_the_walk_builds`
         compares the packed `Occlusion` of a baked frame against a walked
         one on a built town — four blocks, a run of wall crossing a
         block boundary, two statics on one tile, a ground item, and both
         of the cutaway's two cuts — and `tests/cost.rs` makes the same
         comparison on **Britain, every batch**: 25,702 statics over
         187×187 tiles, which is where a read-out that dropped a rim tile
         or reordered a tile's run would show. Both were run against the
         two mutations that should break them (drop the per-tile reverse;
         do not paste the sky) and both go red.

         Equality to the byte is available because nothing about the
         assembly is approximate. A tile's statics all live in its own
         block, so a block's surfaces and its sky are entirely its own;
         within a block the map's order is `(y, x)`, which is the row
         walk's order restricted to that block, so a tile's surfaces
         arrive in the same order either way; and the sky is *assigned*
         rather than multiplied in, because no two blocks share a tile and
         the ground items come after — so the integer rounding of `sky *
         passes / 255` happens in the same sequence in both.

         **What it cost to get right that the plan did not name: decision
         37.** A surface is derived from the map *through the atlas*, and
         an atlas grows — so a block baked before a graphic was packed
         holds the whole-tile fallback for ever. `StaticAtlas::revision`
         is the counter and the `Bake` drops everything when it moves.

         **The numbers**, `what_the_grid_costs_to_build` and
         `tests/cost.rs`, release, Britain at the widest zoom — 187×187
         tiles, 25,702 statics, 17,201 surfaces on 10,212 standing cells:

         ```
         phase                       ms     cumulative
         allocate the builder     0.001      0.001
         walk the map             0.073      0.073
         + shade the sky          0.125      0.199
         + add the surfaces       0.668      0.867
         + blur and pack          0.352      1.220   (`collect` itself)

         camera                      ms     served   built   blocks held
         still                    0.366       9000     600           600
         one tile a frame         0.363       9050     650
         ```

         **The companion is the "served" column and it is asserted, not
         just printed**: a bake that rebuilt every block would cost what
         the walk costs and read identically in a millisecond. A still
         camera serves 600 of 600 after the first frame; a camera moving a
         tile a frame builds about three and a half blocks a frame and
         costs *the same* 0.36ms, which is the reading that decides the
         thing — a widest-zoom frame is 550 blocks and a tile of pan buys
         at most one new column of them.

         What is left in the 0.37ms is the paste (~0.15ms), the blur
         (0.14ms) and the pack (0.08ms). The last two are over the frame's
         rectangle and are per frame whatever is cached, exactly as
         expected; the paste is a copy through `Builder::push`, whose
         per-tile scan is the only thing in it that is not linear. In
         `tests/cost.rs`'s whole-frame reading the grid falls from 1.26ms
         to 0.42ms, against a GPU side of 0.35ms for a night frame — so
         **the CPU half of this pass stopped being the larger one**, which
         is what decision 30 was written to do.

         Two things a cache has that a rebuild does not, both bounded
         rather than argued about: it lets go of the coldest blocks past
         `KEEP_BLOCKS` (4,096, about seven frames of walking, and never a
         block this frame touched — a cache that thrashes is worse than
         none), and it is one map's, which decision 37 states because
         nothing here can check it.

      Read decision 30's micro-decisions before starting: 30.5 decides the
      format (WebGL2 at the time — a texture read with `textureLoad`, not
      a storage buffer), 30.6 decides how many surfaces a cell may hold (a
      distribution printed over Britain, not a guess), and 30.7 is why
      none of the walk's rules are reopened.

      What comes out on the street at the end is a fan: narrow at the
      wall, widening with distance, with the soft edge decision 14's
      penumbra already gives it.

- [ ] **Step 22. A body's footprint.** *(absorbed into step 23 — decision
      38 makes a footprint a solid narrower than its tile, so building this
      first would mean writing a flag into the aperture plane in order to
      delete it two steps later. What survives unchanged is **22.1, the
      measurement**: a derived solid still needs the band, and
      `facing::footprint_of` is where it comes from. 22.2's table row is
      subsumed by the solid verdict of step 23.3. 22.3–22.5 are gone: the
      grid, the walk and the view all learn the general shape instead.)*

      Decision 34, and it is five changes in the order that keeps each one
      testable alone. Nothing here waits on anything outside this list,
      and every step but the last leaves the picture exactly as it is —
      which is the property that makes the last one readable.

      1. **The measurement.** `facing::footprint_of(image) ->
         Option<Footprint>`, beside `facing_of` and off the same one pass
         over the pixels: the first and last column with a pixel in it,
         mapped across the tile's diamond and quantised to `RUN_STEPS`ths.
         `None` for a picture that reaches both corners, which is every
         full-width graphic in the install — so the measurement can only
         narrow the grid and never widen it.

         **Two things to get right and both are cheap to state.** The
         units are counted from the **west** corner (`fx - fy = -1`) to
         the east, because that is left to right across the sprite. And
         the sprite is centred on its tile's column, so the tile's own
         diamond is the middle 44 columns whatever the picture's width — a
         graphic that overhangs is clamped rather than refused, since what
         it covers *of its own tile* is still everything on that side.

         **DoD:** a unit test that builds a synthetic silhouette of a
         stated width and reads the band back; a test that a full-width
         picture measures `None`; and a sweep over the install printing
         how many bodies get a footprint at all, which is the number that
         says whether the rest of this step is worth doing. `0x00CC` —
         columns 12 to 31 of 44 — is the fixture the numbers are checked
         against.
      2. **The table.** `arttable::Shape` gains the footprint, which is a
         format bump (to 3) and a `facing::DETECTOR` bump, for the reason
         the last one was: a table written under the old rules describes
         yesterday's detector exactly and looks perfectly fresh. Authoring
         comes free with it — a person may write a band for a graphic the
         measurement got wrong, and `adopt_authored` already carries it
         over a re-derivation.

         **DoD:** a round trip through the file, and a stale table refused
         rather than half-read.
      3. **The grid.** `occlusion::Surface` gains it, and it rides in the
         **aperture plane** — `(near, far, ., .)` under a flag of its own
         beside `HOLED`, because that plane already exists per surface and
         is allocated only when something is in it. No texture grows and
         no texel widens. `Builder::add` writes one only for a body,
         exactly as it drops a hole offered to a lid.

         **DoD:** `only_a_body_carries_a_footprint`, and the upload test
         that a footprint lands at its own surface's index — the same
         failure mode a hole had, where a shader reading the wrong index
         draws something everywhere and is wrong only where the thing is.
      4. **The walk.** A body is travelled through, so what changes is the
         *length*: the segment inside the cell is clipped to the strip
         `near <= (fx - fy + 1) / 2 <= far`, which is closed form and
         exact. The side pierce of decision 24 moves with it — the sides
         that stop a ray become the strip's own two boundaries rather than
         the tile's four edges. Both implementations, held by the parity
         test.

         **DoD:** a scene — a narrow body in the open with a torch beside
         it — where the ground either side of the strip is lit and the
         strip's own shadow is narrower than a tile; the parity test
         green; and every existing scene unmoved, because none of them has
         a narrow graphic in it.
      5. **The view.** The occluder overlay draws the strip rather than
         the square, which is the step that makes the whole thing visible:
         a body is currently the one kind whose drawn shape and whose
         behaviour are the same wrong answer.

         **DoD:** the tile a person pointed at — Britain's `1509,1635` —
         reads as a narrow violet slab among red panels rather than as a
         full square.

      **What this does not do**, stated so the next session does not go
      looking: it says nothing about depth (`fx + fy`), which no single
      picture can measure; a footprint is one band, not a polygon; and it
      is per *graphic*, so the same picture is the same band on every tile
      it stands on.

- [ ] **Step 23. A solid the world owns.** Decision 38, in six changes.
      23.0 comes first and is not bookkeeping: it is the oracle the rest
      is judged with (see "Solids as drawable geometry" archive for the
      full 23.0 write-up). 23.1 is deliberately invisible, and that is the
      property being bought.

      1. **[x] The ownership, with the geometry held still.**
         `occlusion::Surface` becomes `Solid` — a box in world coordinates
         plus the fields it already has (`opacity`, the hole flag) — and a
         cell holds `(offset, count)` into an index plane of solid ids
         rather than into the solids themselves. ~~Every solid built in
         this step is exactly the box its surface was: a panel is a slab
         of nominal thickness on its edge, a lid is a slab of nominal
         height, a body is its tile.~~ The nominal numbers are chosen so
         that **no test moves** — where a plane test and a slab test can
         differ, the slab is the one that must reproduce the plane's
         answer, and where it cannot, the scene that catches it is the
         finding.

         **DoD:** every scene in `tests/lighting.rs` and `tests/frame.rs`
         unchanged to the byte, the parity test green in both
         implementations, `tests/cost.rs`'s grid assertions re-derived and
         the new distribution of solids-per-cell printed beside 30.6's
         old one. And a bench reading: one indirection in the hot loop is
         the thing most likely to cost something, and it is measured
         rather than argued.

         **What landed, and the two things it decided.**

         **The slab is not stored, and that is the struck-out sentence
         above.** The step as written wanted a panel to become a slab of
         nominal thickness straight away. It cannot, without the record
         telling a lie the whole plan is built to avoid: what a ray is
         tested against in this step is still a *plane* — the walk's
         rules are unchanged, which is the entire DoD — so a thickness in
         the box would be geometry sitting in the field a reader takes for
         geometry with nothing testing it. So a solid's box is what the
         walk crosses: a panel's is its plane, flat on one horizontal
         axis; a lid's is the height it lies at; a body's is its tile,
         which is the one kind that was already a box. The thickness a
         person needs in order to *see* a plane edge-on is the view's, and
         it moved there with its fence intact — `solid::DRAWN_PANEL_THICKNESS`,
         `solid::DRAWN_LID_THICKNESS` and `solid::drawn`, whose only caller
         is a picture. Decision 38 withdrew "a wall must stay a plane
         because a box of zero thickness is a numerical coin toss" on the
         grounds that *with authoring* a thickness is a number a person
         states; nothing had authored one yet (that came in step 23.3),
         so zero was the honest entry and step 23.5 is where a stated one
         arrives with a ray to test it.

         **The kind is carried, not derived** — the real work of this
         step. Deriving it from the box reads well and is wrong on a case
         the map is full of: a static whose `tiledata` height is zero is a
         **body** with a degenerate span, flat in `z` exactly as a floor
         is, so "flat in `z` is a lid" would silently re-kind it and a lid
         is travelled through by a different rule. `Solid::edges` stays,
         with the argument in its doc, and goes in 23.5 when the rules
         that ask it go.

         Built:
         - `occlusion::Solid` — the box (`solid::Solid`) plus `opacity`,
           `edges`, the hole and `roof`; `bottom()`/`top()` come off the
           box, and `Solid::box_of` is the one place a kind becomes
           geometry, so the four call sites in `Builder::add` cannot put a
           panel on the wrong edge one at a time;
         - `occlusion::SolidId`, `Occlusion::ids`, `ids_at`, `solid`,
           `solids_at` — the level between a cell and a solid. Today's ids
           are the identity, because nothing is shared yet, and building
           it anyway is 23.2's own argument: a missing reference is a hole
           in a shadow that looks exactly like a detector failing;
         - the upload is four planes now — `bytes` (the index, unchanged
           to the texel), `id_bytes` (new), `solid_bytes` (was
           `surface_bytes`, unchanged to the texel) and `aperture_bytes`.
           **The box's `x` and `y` are deliberately not uploaded**: the
           walk derives a panel's plane from the cell it is stepping
           through and `edges`, so the two horizontal axes have no reader
           in the shader, and four channels of geometry beside a walk
           that ignores them is how a format grows a field nobody dares
           change. They arrive in 23.5 with a reader;
         - `blit.wgsl`: `solids_at`, `id_at`, `solid_at`, binding 8, and
           the one extra `textureLoad` per solid per cell. `SURFACE_ROW`
           became `LIST_ROW` because three lists are folded by it now;
         - `light::walk_cells` and `panel_stop` read through the same
           level, so the two implementations still mirror each other line
           for line;
         - the views — `solid::standing`, `shell::draw_occluders`, the
           plan view and `artscan`'s `grid` example — read the owned
           solid, and `Surface::solid` is gone.

         **Green:** `cargo test --workspace`, `clippy --all-targets` and
         `fmt` silent; `tests/lighting.rs`'s 31 scenes and `tests/frame.rs`'s
         37 — parity included, which is the one that walks both
         implementations over the same scene — unchanged.

         **The distribution, re-measured**, which the backlog asked for
         and which supersedes 30.6's: over Britain at the widest zoom,
         **10,212 standing cells hold 17,201 solids under 17,201
         references**, nothing dropped, and the tail is short —

         ```
           solids a cell references     cells      share
                              1          6102      59.8%
                              2          2625      25.7%
                              3           773       7.6%
                              4           390       3.8%
                              5           164       1.6%
                              6            80       0.8%
                           7–11           158       1.5%
         ```

         The two totals being **equal** is the fact worth having, not a
         redundancy: it says nothing is shared yet, which is a statement
         about the map's geometry under today's builder rather than about
         this format, and it is the number 38.2's spill will move first.
         `tests/cost.rs` prints both and asserts that references never
         fall below solids — a solid nothing points at is a wall no ray
         can find.

         30.6's old figure was 18,071, and **the difference is not this
         step's**: `Builder::push` dedups on the same predicate over the
         same records (a cell's solids share a tile, so equal boxes are
         equal spans and kinds), so nothing here can change what is
         built. The old number was taken at step 21.2, before a climbable
         static stopped being two panels and became one body. It should
         not be quoted again either way.

         **The cost of the indirection: below what this bench can
         resolve**, and the instrument says so itself. Four runs each way
         at the widest zoom, with and without the id fetch — the ids are
         the identity today, so `solid_at(id_at(i))` against `solid_at(i)`
         is exactly the pass as it was before this step:

         ```
           night, ms a frame     0.639  0.793  0.805  0.830   with the fetch
                                 0.435  0.670  0.702  0.725   without it
           dark, the control     0.385 … 0.621                  neither walks a ray
         ```

         The medians differ by about 0.1ms in the direction one would
         expect, and the sets overlap. What settles it is the control:
         `dark` has no flames, so it walks no ray and reads no solid at
         all, and its own spread over the same eight runs is 0.24ms —
         **wider than the difference being looked for**. So the honest
         reading is a bound rather than a measurement: the fetch costs
         under about a fifth of the pass, on an adapter where the whole
         night pass is 0.8ms against a 16.7ms frame.
      2. **[x] The spill, and the ring's measured radius.** Decision 38.2:
         a `Baked` block gains a spill list, the frame pastes the ring's
         spill, and the ring's width comes from the widest reach in the
         table rather than from a constant. Still no geometry that spills
         — this is the plumbing arriving before its first user, on
         purpose, because a missing reference is a hole in a shadow that
         looks exactly like a detector failing.

         **DoD:** a synthetic solid, authored to overhang, that occludes
         correctly when its anchor's block is *outside* the frame's block
         set — which is the test that fails if the ring is not pasted,
         and it wants a second case at two blocks of reach, because a ring
         that is hardcoded to one passes the first and fails the second. A
         radius that follows the table rather than a constant; the log
         line; and a cost reading showing that a radius of zero costs a
         lookup.

         **What landed, and the one thing the DoD could not yet ask for
         honestly.** `Baked::spill` (`occlusion/bake.rs`) and
         `Solid::footprint` (`occlusion.rs`) are decision 38.1 finished
         for whatever box a solid turns out to have, not only the
         cross-block case 38.2 was written about: every tile a solid's box
         touches besides its own anchor is a spill entry, in absolute map
         coordinates, so `Builder::paste` places it with no translation
         and no case split between "this block was wanted" and "this
         block is only here for its spill" — `Builder::index`'s existing
         clamp is what tells the two apart. `bake::collect_ring` widens
         the block range by a radius and is what `collect` calls with
         `ring_radius(atlas)`; the tests hold the radius themselves and
         author the overhanging solid through `Baked::synthetic`, a
         `#[cfg(test)]` seam, because nothing `Solid::box_of` built at the
         time was wider than one tile — 23.1 left it that way on purpose
         and this step does not move it.

         **`ring_radius` was zero, and it earned that answer rather than
         stating it.** The table had nothing to carry a reach in yet —
         that was 23.3, the next step — so there was no per-graphic
         number to take a `max` over. What this step could honestly build
         was a function that reads the atlas it is handed and finds
         nothing wider than a tile in it, which is what `bake::ring_radius`
         is, and its doc says why in place of the number changing later
         without this comment moving. The alternative — a hardcoded `0`
         with no argument — was rejected as the same invented constant
         decision 38.2 already withdrew once for the ring's width itself;
         a function that ignores what it is handed is not "measured", it
         only reads like it.
      3. **[x] The table carries a solid.** `arttable` gained a third
         verdict and `FORMAT` bumped to 3, with `facing::DETECTOR` bumped
         for the reason the last bump had: a table written under the old
         rules describes yesterday's detector exactly and looks perfectly
         fresh. Derivation is the prism fit that already existed
         (`tests/prism.rs` scores 0.977 and 0.975 on the staircase, against
         0.812 for a wall that is not a prism at all), gated on
         `CLIMBABLE` first and the score second — `Builder::add`,
         `occlusion.rs:1385`. `adopt_authored` carries a hand-written
         solid over a re-derivation, which was already how it worked.

         **Landed as `4ac78dc`, ahead of 23.1 and 23.2 by the clock** —
         this plan's numbering had not yet settled into six sub-steps when
         it was written, and the checkbox went unmarked afterwards purely
         because nobody came back to it, not because anything was
         missing. Every part of the DoD is held: the round trip including
         a multi-box solid and a hand-authored plain one
         (`a_prism_survives_the_round_trip_beside_the_corner_it_was_read_from`),
         a stale table refused rather than half-read
         (`a_table_from_another_format_is_refused`, formats 1 and 2 both
         named), and the defect that mattered — a prism `Shape::of`
         measures is no longer lost through the table, held on both
         sides: `a_stair_is_two_faces_per_tread_and_each_ones_height_comes_off_the_art`
         and `a_climbable_static_occludes_half_its_height` in
         `occlusion.rs`.
      4. **The instrument, which is what makes "by hand" a real mode.**
         See "The art-measurement pipeline" archive for the full
         `tests/author.rs` write-up.
      5. **And now the picture changes.** Treads as their own boxes, a
         wall with a stated thickness, an arch as more than one solid.
         Each of these is its own reading, taken one at a time against a
         scene that isolates it, and each may be reverted alone — which
         is the entire reason 23.1 through 23.4 were built without moving
         a pixel.

         **The first bullet is already done, and not by this plan asking
         for it.** [`gbuffer.md`](gbuffer.md)'s steps 4b and 4c needed
         the same per-tread top-and-riser decomposition for a reason of
         their own — a mesh face to rasterise — and built it into
         `occlusion.rs` before this step got to it by name. The
         staircase at Britain's `(1493, 1639)` already lights as
         horizontal treads rather than as two vertical half-walls;
         `gbuffer.md`'s decision 3 and its step 5 have the measurement.
         What is left of this step is the other two: a wall with a
         stated thickness, and an arch as more than one solid — the
         latter has a format to be authored into since decision 41, but
         nothing authored yet and `Builder::add` does not read
         `Shape::blocks` yet either. Both wait on 23.4's instrument; the
         arch waits on `Builder::add` besides.

         **DoD:** `1509,1635` a narrow slab among red panels rather than
         a full square; and a corner where two walls meet with no light
         through the join, which is decision 18's spokes closed by
         geometry rather than by scaling a crossing's length.

         **The wall-thickness half landed, and `1509,1635` turned out to
         be a different DoD than this bullet's own name.**
         `occlusion::PANEL_THICKNESS` is real geometry now:
         `Solid::box_of`'s four named-edge arms fatten inward by it, the
         record itself (not a view-only copy) carries the slab,
         `solid::drawn` no longer touches a panel at all — the box
         already is the picture — and `DRAWN_PANEL_THICKNESS` is gone,
         closing the backlog entry decision 38's own step 23.1 opened
         when it split the two numbers apart. `1509,1635` (`0x00CC`) is
         **not** this: it is decision 34's own body footprint, folded
         into step 23 under "the grid, the walk and the view all learn
         the general shape instead", and that shape is still unbuilt —
         see "The occluding world" backlog, "found while giving a panel
         real thickness", for why it turned out to be its own open
         question rather than a small remainder of this one.

         **The corner half is where the session's own honesty has to be
         written down.** `light::corner_tie` (`blit.wgsl`'s twin) no
         longer invents a float tolerance: it converts `PANEL_THICKNESS`
         into the same `t` the walk already steps in, by an exact
         derivation (in the function's own doc) rather than an
         approximation, and a pure unit test checks the arithmetic
         independently of the implementation that uses it. **What could
         not be built is a scene that tells the old `1e-4` and the new,
         wider window apart.**
         `a_ray_near_a_corner_and_off_the_exact_diagonal_still_does_not_slip_through`
         (`tests/lighting.rs`) passes under both — checked by hand,
         reverting the derivation to a bare `1e-4` and re-running it —
         because a ray a quarter tile off the exact diagonal already
         takes the *ordinary*, non-corner DDA step into one of the two
         wall cells directly, and a body (`EDGE_ANY`, decision 24) stops
         a ray inside its own cell without any help from the corner
         branch at all. Decision 18's own bug ("the crossing is a hair
         long") was already a precision fix at the exact corner rather
         than a width the map is full of counterexamples for, so the
         honest reading is: **the derivation is a defensible, data-sized
         replacement for an invented constant, kept because it costs
         nothing and a wider tie is never wrong, not a demonstrated fix
         for a leak anyone has shown still exists.** If a future session
         ever needs the old tiny epsilon back for a performance reason,
         that is a real question this paragraph flags rather than hides.
         *(This is the same corner-tie work
         [`lighting_raymarch.md`](lighting_raymarch.md) later continues
         and eventually replaces with a fully derived bound — see that
         document's own archive.)*

         **Still open at the time:** the arch — `Builder::add` still does
         not read `Shape::blocks`, and nothing is authored into any table
         for it — and `1509,1635`'s footprint, its own line in "The
         occluding world" backlog above.

      **What comes out on the street at the end**, once step 23.5's "real
      shape" work is picked up: treads as their own boxes, a wall with a
      stated thickness, an arch as more than one solid — see "The occluding
      world" backlog above for the wall-thickness half that has landed and
      what step 23.5.5 found.

### Backlog: found while giving a panel real thickness (step 23.5.5)

- **A body's footprint (decision 34, folded into step 23 as "the grid, the
  walk and the view all learn the general shape instead") does not
  actually have a general shape to learn yet, and the gap is geometric
  rather than missing code.** What decision 34.1 measures is a band in
  `(fx - fy)` — the screen column a silhouette's first and last drawn
  pixel falls on — and `fx`, `fy` there are `u`, `v`, the tile-local world
  fractions the projection actually uses ("world `+x` moves the screen by
  `(+22,+22)` and `+y` by `(-22,+22)`, so a sprite's column is `(fx - fy)`
  and nothing else"). A band in `u - v` is a **diagonal** stripe of the
  tile, and `occlusion::Solid` is, by `solid.rs`'s own module doc, never
  rotated — "no rotation anywhere in this renderer... and it never will."
  `Prism::footprint` looks like the precedent and is not one: a tread's
  strip is bounded along a *named climb axis* (`up`), which is why its
  `lo..hi` fraction turns into an honest axis-aligned `min_x..max_x`; a
  body has no `up`, only a column, and a column does not pick a world
  axis on its own. So the literal DoD (`1509,1635` reads as a narrow
  slab) cannot be met by extending `Solid::box_of` the way a panel's
  thickness just was, and needs one of: an axis-aligned box that
  conservatively *under*-covers the true diagonal band (loses some real
  occlusion at the band's own corners), a second, non-axis-aligned
  primitive the walk would have to gain a new kind of test for, or a
  different measurement that picks a world axis some other way (the wall
  the body is usually cast against, if `facing_of` reads one nearby, is
  the only candidate looked at and not pursued). Whoever picks this up
  next should read `facing::Prism::footprint` first for exactly why it
  does not generalise. *(Still open — see `lighting.md`'s Status.)*
- **`light::corner_tie`'s new, `PANEL_THICKNESS`-derived width has no scene
  that tells it apart from the old, bare `1e-4` it replaced.** Written up
  in step 23.5.5 itself rather than only here, because it changes what the
  step's own DoD can honestly claim: the derivation is correct arithmetic
  (a pure unit test pins it) and a defensible replacement for an invented
  constant, but nobody has shown a ray that the old tolerance let through
  and the new one stops. The candidate that would prove it — a ray whose
  two boundary crossings differ by more than `1e-4` but less than the new
  width, and that does not *also* get caught by the ordinary single-axis
  step into a neighbouring wall cell — was not found in the time this
  session had. *(This entry's own numerical-precision question is
  exactly [`lighting_raymarch.md`](lighting_raymarch.md)'s subject, and
  its own tie-break work should be read there.)*

### Backlog: found while migrating the ownership (step 23.1)

- **The cost of one fetch in the hot loop is under this bench's noise, and
  the bench says so with its own control.** `tests/cost.rs`'s `dark` case
  walks no ray at all, so it cannot be moved by anything in the walk, and
  its spread over eight runs is 0.24ms — wider than the difference between
  the pass with the id fetch and without it. That is fine for the answer
  23.1 needed (a bound), and it is not fine for the next question of this
  shape, which 23.5 will certainly ask. What a better instrument wants is
  not more runs: it is the same frame timed with a GPU timestamp query
  around the blit alone rather than a wall clock round a submit, and a
  case whose *only* difference is the thing under test.
- **`solid::standing` lists a solid once per cell that references it.**
  38.2's spill has landed and the mechanism is real, but still harmless:
  nothing built at the time produces a box wider than one tile, so nothing
  is referenced twice yet. A solid overhanging four cells, once 23.5
  authors one, will be drawn four times, translucent, and read as four
  weights of colour on one box. The fix is a dedup on the id, and the
  reason it is not written yet is that a view of a *shared* solid also
  wants to say which cells found it, which is a question about what the
  instrument is for.
- **A lid with a span of its own is drawn two `z` deep, not as deep as it
  is.** `solid::drawn` replaces a lid's bottom rather than lowering it to
  reach, which is what step 23.0 drew and is kept to the pixel because
  23.1's whole claim is that no picture moved. A `FLOOR` static with a
  height — a sloped roof section is one — therefore looks thinner in the
  view than the span the walk stops light over. Worth a picture before it
  is changed, and it belongs with 23.5's readings rather than on its own.
- ~~**The walk's rules are keyed on `Surface::edges`, and a box does not
  have one — which is the real shape of step 23.1's work.**~~ Decided in
  23.1: the kind is **carried**. The case that settled it is not the
  abstract one — a static whose `tiledata` height is zero is a body with a
  degenerate span, flat in `z` exactly as a floor is, so deriving would
  re-kind it into a lid and a lid is travelled through by a different
  rule. Written out at `occlusion::Solid`, and it goes in 23.5 with the
  rules that ask it.

### Backlog: found while re-cutting the plan around decision 38 (nothing built that session)

- **A wall's thickness may be measurable after all, and the number is
  already in the tree.** `facing::OVERHANG`'s own doc says it: *a wall is
  a solid with a thickness, and the picture shows that thickness* — the
  far side of the face is a sliver past the tile's centre column, **3.5
  pixels on `0x0100`, 2.5 on `0x0007`** — and the conversion is written
  beside it, `22t` pixels for `t` tiles. That is 0.16 of a tile, derived
  rather than invented, and it means decision 3's "the art cannot say how
  deep a wall is" is too strong: it cannot say from the *outline alone*,
  but this sliver is the depth, projected. The confounder is named in the
  same comment and is real: on a wall low enough to look down on, the
  sliver also contains the **top** surface (8.5 pixels on Britain's
  garden wall), so the measurement is two things added together wherever
  the top is visible. The way to settle it is the instrument, not an
  argument — score a box of thickness `t` against the sprite and take the
  best `t`, exactly as `facing::best_prism` already takes the best prism.
  *(Still open — see `lighting.md`'s Status: "a stated wall thickness
  beyond `PANEL_THICKNESS`'s current nominal value".)*

### Backlog: found on a staircase in Britain

- **A stair is read as a corner of two walls, and there is no stance for a
  slope.** Reported from `(1496, 1641)` and `(1493, 1639)`: a flight of
  stairs is drawn with hard triangles of shadow across it, as if the lit
  surface had been turned inside out. `tests/onsite.rs` at either tile
  names it in one line — the stair graphics `0x071E` (`1822`, height 10)
  and `0x0736` (`1846`, height 5) read `facing Some(Corner { right: East,
  left: South })`, `stance CornerEastSouth`, `opacity 255`, `climbable
  true`. So:
  - **The shading.** `blit.wgsl`'s `outward` gives the right half of
    every step the normal `(1, 0, 0)` and the left half `(0, 1, 0)` — two
    *vertical* walls meeting on the sprite's centre column. A stair's
    surface is neither: it climbs at roughly 45°, and its normal has a
    `z` in it. Every step is therefore lit as a pair of half-tiles turned
    away from whatever the sun and the flames are, and the seam between
    the halves is what the picture shows as a triangle. Nothing in
    `Stance` can say "a slope": the enum is flat, upright, four faces and
    four corners.
  - **The occlusion.** The same verdict puts opaque panels on the tile's
    East and South edges for the stair's whole height, so a staircase
    shadows like a run of wall — including onto its own steps.
  - **The detector cannot see it from the silhouette alone.** A stair's
    base *is* a clean 45° run on both halves, which is exactly what
    `facing_of` asks for, and it stands 40 pixels tall, well over
    `MIN_STANDING`. What tells it apart is not the picture but the
    client's own bit: `TileFlags::CLIMBABLE` (`Bridge`, `0x0400`) is set
    on both graphics and on nothing that is a wall — the same
    order-of-policy argument `Stance::of` already makes for
    `is_background`, one flag over.

  What it needed, in the order it would be built: a `scene::staircase`
  with one flight and nothing else (the plan view is what says whether
  the shading follows the climb), then a stance for the shape, and the
  occlusion side, where a climbable tile should stop being a wall. Sphere
  already halves a climbable tile's height, which is a hint that the
  reference treats this shape as a special case too.

- **And the shape is a box, not a slope.** The first guess above was that
  a stair is an inclined plane and that what the art would have to be
  measured for is which way it climbs. Then the pictures were looked at —
  `tests/artshot.rs` writes any graphic out scaled with the tile's diamond
  stroked over it, and prints the lowest and highest drawn pixel of every
  column. `0x071E` is a **cube ten `z` tall**: its base is the whole
  diamond (21 pixels down to 0 at the centre column and back up, a 1:1
  run, which is the diamond and not a wall's single 45° edge), and its top
  contour is the same diamond raised 42 pixels. `0x0736` is the same box
  with a **stepped lid** — three treads falling away to the west, which
  the column profile shows as three flats in the top contour. Against a
  real wall for contrast, `0x00C8`: base `21…2` across the left half and
  *nothing at all* past column 32. So:
  - The surface that dominates either sprite is the **lid**, and the lid
    is horizontal. It was lit as a vertical wall, which is the whole of
    what the report saw.
  - `facing_of` says `Corner { East, South }` about a box for a reason
    that is not a bug in it: a box's base *is* two 45° runs meeting at
    the south corner, which is exactly the silhouette two walls meeting
    at a corner leave. The detector answers about the two vertical faces
    it can see and there is no third answer in `Facing` for the lid on
    top of them.
  - **The geometry is a profile, extruded.** A height field over the tile
    that varies along one axis and is constant across the other:
    `facing::Prism`, with `up` naming the high side and `treads` the
    profile. A box is the one-tread case. `facing::prism_silhouette` is
    its forward projection, drawn the way `facing::silhouette` draws a
    wall, and every column of it is a vertical run the solid really
    contains rather than a rasterised polygon.
  - **And the fit against the client's own art says the model is right.**
    `tests/prism.rs` scores every candidate prism against a real sprite by
    intersection over union of the two silhouettes, aligned by the bottom
    row and the centre column — no free placement parameter. Measured on
    the staircase this came from:

    | graphic | best prism | agreement | tiledata height | drawn height |
    |---|---|---|---|---|
    | `0x071E` the landing | box, 5 `z` | **0.977** | 10 | 5 |
    | `0x0736` the flight | 3 treads climbing west, to 5 `z` | **0.975** | 5 | 5 |
    | `0x00C8` a plain wall | (control) | 0.812 | 20 | — |

    Two things fall out of that table and neither was expected. **The
    height cannot come from `tiledata`**: the landing states ten and the
    artist drew five, the flight states five and the artist drew five —
    the same field means the full height on one and the drawn height on
    the other, which is the same ambiguity `movement::scene::stair`'s
    "stand half way up it" lives with. The art is the measurement. And
    **the fit alone is not a gate**: a wall that is not a prism at all
    still scores 0.81, so what admits a prism is `CLIMBABLE` first and the
    score second — the order-of-policy `Stance::of` already uses for a
    floor.
  - **The grid believes it now, and the picture is one body per stair.**
    `Builder::add` asks `CLIMBABLE` first and, where the art fitted a
    prism, stands one `EDGE_ANY` surface at the *measured* height instead
    of two opaque panels on the tile's east and south edges. Measured at
    `(1493, 1639)` in Britain: the stair tiles read `edges NESW` where
    they read `-ES-`. A staircase no longer shadows a street like a run
    of wall.
  - **What is left is the treads themselves, and they are decision 36.**
    A tread is a body over *part* of a tile, and `Surface` has no way to
    say "part of": its three kinds are a panel on one edge, a lid, and a
    body over the whole tile. That missing form is the fifth of its kind
    in this file, which is what turned it from a fix into a decision — an
    occluder becomes a box, and a tread is one. Until that lands, a
    flight of steps occludes as a single box the height of its top tread.
    *(and the box turned out not to live in the tile's own coordinates
    either — decision 38, step 23)*
  - ~~**`ArtTable` does not carry a prism.**~~ **Done.** A row is
    `facing`, `hole` and `prism U h…` — the face the climb ends at and one
    height per tread — at `FORMAT` 3, so a format-2 table is refused
    rather than half-read into a world where every staircase is a corner
    of two walls. It rides *beside* the verdict rather than replacing it,
    because the corner is what the wall detector really says about the
    picture and `Builder::add` is the one that picks between them on
    `CLIMBABLE`. A `face` may not carry one, which is the mirror of the
    hole's rule and comes from the same place: `Shape::of` scores prisms
    only against a picture it read as a corner, so a row saying otherwise
    would state what no detector will. `artscan` reports `solids:` beside
    `corners:` and `windows:` — the number that says the seconds a scan
    spends searching bought something, and the one a tightened gate would
    show up in and nowhere else.

### Backlog: found while building the treads (step 23.5, footprint bug and reproduction tooling)

- **A footprint bug from 23.2 that only the real map catches.**
  `Solid::footprint` floored an `EDGE_EAST`/`EDGE_SOUTH` panel's flat
  coordinate straight — correct for `EDGE_NORTH`/`EDGE_WEST`, whose plane
  sits at the tile's own low edge, wrong for the other two, whose plane
  sits at the *far* edge (`x + 1`, `y + 1`, an integer that floors to the
  neighbour). `tests/cost.rs`'s oracle (`cached == grid`) is the only
  thing in the tree that reads a wide-enough real map to hit it — no
  synthetic scene stood a panel exactly on a block boundary. Fixed by
  reading `self.edges` in `footprint`'s degenerate branch; see the
  function's own doc in `occlusion.rs` for the two cases. Found while
  chasing what turned out to be an unrelated question (below), which is
  worth remembering the next time a synthetic-scene suite is all green and
  a real map has not been run through the same oracle.
- **Reproducing one real place headlessly, for the next session — done.**
  No GUI is needed — `tests/cost.rs` already opens a headless `wgpu`
  adapter and can dump any of `debug::View`'s pictures with
  `OPENSHARD_FRAME_DUMP`/`OPENSHARD_FRAME_VIEW`. What was missing was a
  way to point its camera anywhere but the hardcoded `BRITAIN` constant —
  every look at the staircase run at `(1494..=1497, 1626..=1627)` before
  this took a hand edit of `BRITAIN`'s literal and `widest()` →
  `Zoom::ONE` at every call site, run, then `git checkout -- tests/cost.rs`
  to undo it. `OPENSHARD_FRAME_AT=x,y,z` now does that:
  `frame_point_and_zoom` returns `BRITAIN` at `widest()` when unset, and
  the named point at `Zoom::ONE` — close, since naming a place is for
  looking closely at it — when it is. The one assertion that only holds
  at the widest rung (`camera.minifies()`) is skipped when a place is
  named; the rest of the test's assertions (a lit frame, a standing cell,
  a changed pixel) still run and still may panic if the named place has
  nothing lit nearby, which is the honest outcome and not a bug in the
  env var.

  ```sh
  OPENSHARD_CLIENT=… OPENSHARD_FRAME_AT=1495,1627,10 \
      OPENSHARD_FRAME_DUMP=/tmp/lit.ppm OPENSHARD_FRAME_VIEW=0 \
      cargo test --release -p openshard-client-render --test cost -- --ignored --nocapture
  ```
  `OPENSHARD_FRAME_VIEW` is the index into `debug::View::ALL` — `0` is
  `Lit`, `4` is `Occluders`, `5` is `Light`.
- **The flame the user means is usually not a map static.**
  `Solid::footprint`'s own staircase (`1849`/`0x0739`) carries no
  `LIGHT_SOURCE` flag — it is only steps. The wall sconces standing right
  next to it (`0x013A`/`0x013B`) do carry the flag but never burn:
  `light::burns` also requires `occlusion::opacity == CLEAR`, and a
  bracket mounted flush against a wall has `NO_SHOOT`, so it reads as
  wall rather than flame — see `burns`'s own doc for why that is the
  conservative direction and not a bug. What actually lights a place like
  this is usually a **decoration the running shard placed**, which lives
  in `openshard.db`'s `decorations` table (a static-like fixture the
  Community Pack's scripts put down) or `items` (`loc_kind = 0`, something
  dropped), never in the client's own `.mul`/`.uop` — so `map.statics_at`
  cannot see it and neither can a raw-file-only reproduction. Pull it
  straight from the live DB rather than guessing:
  ```sh
  sqlite3 openshard.db "select data from decorations" | python3 -c '
  import sys, json
  for line in sys.stdin:
      d = json.loads(line)
      if d["facet"] == 0 and abs(d["x"] - 1498) <= 2 and abs(d["y"] - 1626) <= 2:
          print(d)'
  ```
  and feed the one result in as a `crate::items::GroundItem` — `at`,
  `graphic`, `hue`, nothing else — passed as `extra_items` everywhere
  `tests/cost.rs` passes `&[]` (three call sites: `light::collect`,
  `occlusion::collect`, `occlusion::bake::collect`). Keep the list to the
  one lamp the question is about; the DB holds hundreds of decorations in
  the same block and every one not in reach of the tile in question is
  noise in the picture and nothing more — pulling the *whole* nearby set
  once (all 217 within 45 tiles, this session) is worth doing exactly
  once, to confirm nothing closer was missed, and then thrown away in
  favour of the one that mattered.
- **Two debug views that looked like the right instrument and were not.**
  `View::Height` draws the *drawn sprite's* own per-pixel world height
  (the `place` attachment `statics.wgsl` writes) — a different mechanism
  entirely from `occlusion::Solid`, so a stair's art reading as one smooth
  ramp there says nothing about whether its occlusion is one box or
  three. `View::Occluders` (`blit.wgsl`'s `merged_at`) reads the tile's
  *merged* span — the union of every solid on it — which by construction
  cannot distinguish one whole-tile body from three tread-strips whose
  union is the same envelope. Neither view answered "did the tread split
  actually happen"; only `Occlusion::solids_at(x, y)`, read directly in
  Rust, did — see the recipe above, minus the `OPENSHARD_FRAME_*` vars,
  plus a loop over `grid.solids_at(tx, ty)`.
- **What that direct read confirmed, and what is still open.**
  `tread_box_of` does what it was built to: tile `(1495, 1627)`'s three
  solids are three `y` strips (`10..=11`, `10..=13`, `10..=15` in `z`,
  each a third of the tile along the climb), the low one nearest south
  and the high one nearest the `up: North` the table measured. A
  screenshot of `View::Light` over this run showed a fine sawtooth along
  the whole flight where a coarser one stood before — eight tiles × up to
  three treads is more edges than eight tiles × one box, and that is the
  geometry working as intended rather than a defect. **What was not
  settled**: whether that finer edge wants a blur radius wider than a
  third of a tile so it reads as a staircase and not static, which is a
  rendering-quality question and not a correctness one — `tests/cost.rs`'s
  oracle is green on the real map with the footprint fix in, and the
  geometry itself is confirmed by direct read rather than by eye.
- **A reusable tool for exactly this, so sampling code does not have to be
  disposable.** `examples/isolated_scene.rs` draws a **synthetic** map
  (`WorldMap::from_blocks`, which never carries statics) and puts back only
  what is asked for, all through environment variables: the real map's
  statics within a stated radius of a stated point (optionally filtered to
  a list of tile IDs), the real ground under them or none at all, and any
  hand-named extra item — a live-shard decoration such as the lamp above,
  in the shape the DB-lookup recipe already produces.
  ```sh
  OPENSHARD_CLIENT=… \
      OPENSHARD_SCENE_AT=1497,1626,10 OPENSHARD_SCENE_TILES=0x0739,0x0738 \
      OPENSHARD_SCENE_GROUND=0 OPENSHARD_SCENE_EXTRA=1498,1626,10,2852 \
      OPENSHARD_FRAME_DUMP=/tmp/corner.ppm OPENSHARD_FRAME_VIEW=5 \
      cargo run --release -p openshard-client-render --example isolated_scene
  ```
  *(The occlusion/tread-normal investigation this tool enabled — "the
  user's actual complaint", and its resolution via decision 40 — is
  archived under "The G-buffer bridge" above, under "the tread-normal
  investigation (decision 40, retired)".)*

### Backlog: found while building the spill (step 23.2)

- **`bake::collect_ring`'s widened range still bakes and caches an empty
  block for every ring tile past the facet's own edge.** `Baked::of`
  answers correctly — `WorldMap::statics_in_block` is empty out of range — but
  a frame in a facet's corner, once a real reach exists, pays a `Bake`
  cache entry for blocks that can never hold anything. Not a bug at
  `radius: 0`, where the widened range is the core range; worth a clamp
  against `map.width()`/`map.height()` in the same change that gives
  `ring_radius` a real number to return.
- **`ring_radius` has nothing to read yet, and it shows in the test:**
  `the_measured_radius_is_zero_until_something_authors_a_reach` asserts a
  constant against an atlas that cannot hold anything else. That test
  earns its place once 23.3 gives a graphic a reach to author — until
  then it is a regression guard against the function being *replaced* by
  a literal, not a measurement of anything.

### Backlog: found while making the instrument honest (step 23.0)

See "Solids as drawable geometry" archive below for the full write-up —
the entries there (the vertex-buffer rebuild cost, the drawn-plane test,
the nominal-thickness naming, `Camera::project`-as-matrix, and the
WebGL2/WebGPU re-examination) belong with the diagnostic view they are
about.

### Backlog: found while baking it (step 21.5)

- **The paste is now the largest single thing in the build, and it is a
  linear scan.** Of the 0.37ms a cached frame spends, roughly 0.15ms is
  `Builder::paste` — pushing 17,201 surfaces into the frame's arena
  through `Builder::push`, which walks the tile's existing list on every
  one of them to drop an exact repeat. Pasting a *baked* block cannot
  produce a repeat: the block was deduplicated when it was built and no
  two blocks share a tile. So the scan is provably dead work on this
  path, and the shape of the fix is a `push` that does not dedup, used
  only by `paste`. It was left undone because "provably" is an argument
  and not a test, and the test that would hold it — a paste that silently
  doubled a tile's surfaces — wants naming before the code is written.
- **A frame indoors is not measured.** Every number in this file is off
  `Cutaway::OPEN`. Decision 33 moved the cut to `finish`, so a frame
  inside a house now *builds* the surfaces it is about to drop — which
  was the cost the bake was meant to pay back, and the bake does pay it
  back, but nobody has put the two side by side. It is one more call in
  `what_the_grid_costs_to_build` with a cutaway that cuts, and it is
  worth having because it is the case where a cached frame and an
  uncached one differ most.
- **`Occlusion::dropped` is double-counted at a block boundary,
  harmlessly.** `paste` adds a block's whole `dropped` count whether or
  not the tile that overflowed is inside the frame's rectangle. The
  number is a diagnostic about the map and the worst tile in Britain
  holds 21 of a cap of 255, so it is not reachable today; it is written
  down because the two implementations of the grid are otherwise equal
  *to the byte*, and this is the one field where "equal" rests on the cap
  never being hit rather than on the arithmetic.

### Backlog: found while building it (the grid, general)

- **The per-tile cap and `dropped` now count the map, not the frame.**
  Decision 33 puts every solid into the builder, so `MAX_SOLIDS_PER_CELL`
  (named `MAX_SURFACES_PER_TILE` when this was written) is reached by
  solids a frame was about to cut away — a tile could in principle drop a
  *drawn* one because undrawn ones filled it, and `Occlusion::dropped`
  counts what the map has rather than what the picture lost. Nothing is
  at risk today: the worst tile in Britain is 11 of 255 (step 23.1's
  distribution), and the distribution was measured under `Cutaway::OPEN`,
  which is the same set the builder now holds. It becomes a real question
  only if the cap is ever lowered to fit a format, and the honest fix
  then is to cut before the cap rather than after — which the builder
  cannot do, because it does not know the frame.
- **The land itself does not occlude.** A hill between a campfire and a
  valley stops nothing: only statics are in the grid. The map has the
  four corner heights for every tile and the grid already carries a span,
  so the shape of the fix is "add the land's own height as an occluder
  with `opacity` scaled by how far the ray is under it" — the reason it is
  not done is that a hillside that cast hard shadows would look worse
  than one that casts none until the falloff and the span are tuned
  against a real scene. *(Still open — see `lighting.md`'s Status.)*
- **Nothing a mobile is standing in casts a shadow.** A body between a
  torch and a wall lights the wall as if it were not there. The reference
  does not shadow mobiles either, so this is a note rather than a defect.
- ~~**The ray is Chebyshev-sampled, one cell a step.**~~ Closed by decision
  18: where the two boundaries land together the walk asks both cells
  that share the corner and then steps diagonally past them, which is the
  supercover answer paid for only on the rays that hit a corner exactly.
- **`Occlusion` is rebuilt and reallocated every frame.** 140KB at the
  widest zoom, and the texture upload beside it. Both want the buffer
  kept between frames — the rectangle only changes size on a zoom step or
  a resize.

### Step 14: the occluder boxes, drawn (the wireframe instrument)

- [x] **Step 14. The occluder boxes, drawn.** The instrument the next two
      steps were judged with, and the answer to "why is there a shadow
      where nothing stands". `Occlusion::boxes` is the iterator over the
      cells that hold something — open tiles are most of a grid and are
      skipped, so a caller spends nothing on them; `Hud::occluders`
      carries the grid beside the terrain overlay under its own
      checkbox; `shell::draw_occluders` takes each cell's two diamonds
      through `Camera::tile_diamond` at the span's clamped ends and
      strokes the twelve edges, coloured from glass to bone by the
      cell's opacity so a `PANE` and a wall are told apart. No GPU pass
      and no new texture: this is arithmetic the camera already does.

      Three decisions inside it, each the same one the shader made and
      therefore not a second policy: the grid is rebuilt over
      `light::lit_tiles` (made public for exactly this) rather than over
      the drawn tiles, so the wireframe covers what the walk covers; the
      `z` span is clamped into an `i8` the way `Occlusion::bytes` clamps
      it, so a box is drawn where the shader believes it is rather than
      where the map says; and it is built from the frame's own
      `Cutaway`, which is handed to `App::hud`.

      **The cost**: one more walk of the map's statics over the lit
      bounds per frame *while the box is ticked*, and twelve strokes per
      standing cell. Off by default, like the terrain overlay, and the
      boxes whose eight corners all fall outside the clip rect are
      dropped before a shape is built — at the widest zoom most of the
      grid is offscreen.

      What it was expected to show first: **a door's shadow is a tile
      wide**, because decision 3's occluder is the whole tile and not
      the leaf — which is the report that started this step. It was
      later superseded as the primary diagnostic by the solids pass
      (step 23.0, see "Solids as drawable geometry"), which draws real
      lit boxes rather than a wireframe — the wireframe stays as the
      lighter-weight, always-available toggle beside it.

### Backlog: found while drawing the boxes (the wireframe instrument)

- **A pier is two thousand occluders, and they are floors.** The first
  frame of the wireframe was Britain's docks, and the grid held **2011
  cells** — one thin slab on every plank, because a floor is exactly what
  you cannot shoot *through* to the storey above and the membership test
  is the shooting flags. It is not wrong; it is what makes the picture
  unreadable, and it is why the view draws only what stands above the
  floor the player is on. What it raises and does not answer: a fragment
  standing on that deck is *inside* one of those cells, and the walk
  exempts the light's own tile (decision 3) but not the fragment's — so
  whether a floor dims the light falling on the thing standing on it is
  an open question with a scene-shaped answer. Nothing in the frame looks
  wrong today, which is exactly why it is written down rather than
  assumed.
- **The overlay walks the map a second time in the same frame.** The HUD
  is built before the world passes and the frame's `Lighting` after them,
  so the wireframe cannot read the grid the shader is about to be given —
  it builds its own from the same bounds, the same cutaway and the same
  map, which is the same answer and twice the walk. Sharing them means
  either building the lighting before the HUD or keeping the last frame's
  grid, and the second is what draws a wireframe a frame behind the
  picture it is a claim about. Worth doing when the frame keeps its
  `Lighting` for another reason; not worth it for this alone.
- **Nothing tests the projection of a box.** `Occlusion::boxes` is pinned
  — the row-major arithmetic is the half that fails silently and it has a
  test — but the eight corners and the twelve edges are held by looking
  at them. A frame test would need an egui context and a painter
  offscreen, which nothing in this workspace does yet; the shape of it,
  if it is ever wanted, is that a box's lid is exactly `(top - bottom) *
  Z_STEP` viewport pixels above its floor.
- **The wireframe shows what stands and not what is missing.** The same
  point `lighting_world.md`'s backlog makes about the sky field, arriving
  here: a roof that is one tile over from where it should be draws a box,
  correctly, one tile over — and the tile it *should* have covered draws
  nothing, which looks exactly like open ground. `View::Sky` on the
  ground is the instrument for that half, and the two want reading
  together rather than one replacing the other.

### Backlog: found while deciding what a cell should hold, and while turning it into a list

- **A cell's fetch count goes from one to `1 + K`.** The walk reads one
  texel a cell today; with decision 30 it reads the index and then each
  of that cell's surfaces, inside the same loop. The GPU has the headroom
  — the whole pass is 0.31ms against a 16ms frame — but it is a real
  change in the shape of the inner loop and it belongs in the measurement
  of step 21 rather than in its surprise.
- **A cell's fetch count went from one to `1 + count`, and the
  measurement it was promised is not a comparison.** The entry above
  asked for it to land in step 21's measurement rather than in its
  surprise, and here it is with a caveat: the cost instrument was run
  *after* the change and there is no before beside it, because the scene
  it walks has changed since step 6's numbers and the two are not like
  for like. The new baseline is in step 21.1. What a comparison would
  have to hold still is the flame count — this run found 7 where step 6
  found 64.
- **The union's own cost is now countable, and it is 441 cells.** 10,212
  standing cells hold 10,653 surfaces over Britain, so all but a few
  hundred tiles are one surface and the list costs almost nothing over
  the merged cell. That is the cheerful reading; the other one is that
  441 cells is what step 21.2 will multiply, and a distribution —
  decision 30.6 — still wants printing rather than a total.
- **The surface texture is padded to a whole row, and the row is 1024.** A
  frame with 12 surfaces uploads 4KB. It does not matter at Britain's
  scale, where the list is ten rows, but it is a floor rather than a cost
  that scales, and a narrow scene pays it every frame. The fix, if one is
  ever wanted, is a row width that is a function of the count rather than
  a constant — which is a second number the shader would have to be told.
- **`Occlusion::at` folds on every call, and `boxes()` calls it per
  tile.** The merged view is derived rather than stored, so the wireframe
  overlay now costs a fold per cell where it used to cost a read. Nothing
  in a frame's hot path asks it, and the overlay is a debug view — but
  `shell.rs` draws it per frame when it is on, and that is the place it
  would show.
- **A tile's surfaces are contiguous, and that is an invariant nothing
  states.** `(offset, count)` names a run, and it only names one because
  `Builder::finish` packs in the index's own order. Nothing outside that
  function could break it today, and nothing outside that function is
  stopped from breaking it either — the list and the index are two
  private `Vec`s that agree by construction and not by type.

### Backlog: found while splitting the union

- **A change that has to move the picture moved no test.** Every test in
  the crate stayed green through step 21.2, which is not reassurance — it
  is the coverage report. A built scene is a `WorldMap` with a handful of
  items on it and almost none of them puts *two* statics on one tile, so
  the whole suite had no opinion about the one thing this step is. The
  three tests that pin it were written for it, and the one that goes
  through the walk builds its grid with a `Builder` rather than with a
  scene, because a `WorldMap` makes "two statics on one tile" fiddly to say.
  The same shape as "a scene has no art, so almost every scene tests the
  whole-tile occluder" (see "The shadow ray walk" backlog): the scenes are
  thin exactly where the format is.
- **The union was put back to check the tests were red**, by hand, for
  one run. Worth writing down because it is the only thing that
  distinguishes a test that pins the new behaviour from a test that pins
  the arithmetic that happens to be there — and two of the three would
  have passed a weaker mutation (merging only surfaces with the same
  mask) that leaves the lid-and-panel case alone.
- **A cell's fetch count is 1.77 now, and the GPU noticed.** `night` went
  from 0.368ms to 0.497ms on the same frame and the same machine, which
  is the first time in this file that a representation change has cost
  something legible. It is still 3% of a 60Hz budget and the CPU is still
  the expensive half by four times — but the backlog entry that asked for
  this to land "in the measurement rather than in the surprise" now has a
  real number in both halves.
- **The tail of the distribution is a shop, and nothing says which one.**
  Ten tiles in a Britain frame hold 21 surfaces. That is a stack of
  floors, walls and roof pieces on one square and it is almost certainly
  right — but the histogram is a count with no coordinate in it, and
  `tests/onsite.rs` is the instrument that would name the tile. Worth
  doing the first time a cap has to be chosen, which is not today.
- **`Occlusion::dropped` is counted and nothing asserts on it.** `cost.rs`
  prints it and a frame that dropped a wall would say so to a person
  reading the output. Nothing fails. The right home for an assertion is
  the bake of step 21.5, where a region is measured once and a truncation
  is permanent rather than one frame's.
- **A surface list makes duplicate suppression a linear scan.**
  `Builder::push` walks the tile's list looking for an exact repeat,
  which is one to three comparisons on 99.9% of tiles and twenty-one on
  ten of them. It is nothing today at 18,000 surfaces a frame; it is the
  shape that stops being nothing when the bake covers a block rather than
  a camera.
- **A tile's surfaces are contiguous, and that is still an invariant
  nothing states.** The entry above from step 21.1 is unchanged by this
  step and is now one function further from being checkable: the arena
  and the heads agree by construction, `finish` is what packs them, and
  nothing has a type saying so.

### Backlog: found while cutting a hole in a wall

- **A surface holds one hole, and decision 30.2 said "up to `K`".** A wall
  with two windows in it is two rectangles in one plane, and `Aperture`
  is a field rather than a list. One covers every window graphic the
  client ships as far as anybody has looked, and the cheap way out if it
  does not is a second surface on the same side with the same span — the
  walk takes the largest of a cell's surfaces, so two panels with two
  holes are not the union of the holes. That is the shape of the
  wrongness, and it wants a measurement (step 16) before it wants a fix.
- **A hole is a fact about a graphic and not about a thing.** Two windows
  of the same graphic in one wall have the same hole, which is right; a
  wall a siege engine knocked a gap in has nowhere to say so. The same
  boundary decision 11 drew for doors (see "Doors" archive) — a flag is
  about a picture, a state is about a thing — and the same answer would
  apply: a per-item override, keyed the way `GroundItem` is.
- **The sky field does not know about holes.** `Builder::shade` multiplies
  a tile's sky by what each static leaves, and a static with a window in
  it leaves exactly what a solid one does — so a room under a glazed roof
  lantern is as dark as one under slate. It is the crude half of decision
  14 meeting the fine half of step 21.3 and losing; the shape of the fix
  is that `shade` scales the opacity by the share of the tile the hole
  covers, which is arithmetic the aperture already carries.
- **Nothing draws the hole.** `Occlusion::at`'s merged view has no
  aperture in it, so the wireframe overlay and `plan::Picture::mark` both
  stroke a holed panel as a solid one — and a fan of light with no hole
  drawn on it is exactly the picture step 19 argued against (see "Testing
  and instrumentation"): "a pool that is the wrong shape and a pool that
  is the right shape behind a wall nobody drew are the same picture until
  the wall is drawn on it". The instrument should gap the panel's stroke
  where the hole is.
- **The `field` plane's second channel is free again.** Its doc predicted
  "the sky today, an aperture and a body's opacity" — and an aperture
  turned out to be a fact about a *surface* rather than about a tile, so
  it went beside the surface list instead (decision 30.8). What that
  plane is for is unchanged and it has one more channel to spend than it
  thought.

### Backlog: an architectural alternative to the block cache, raised while building the spill (step 23.2)

- **Worth arguing rather than losing.** The spill and the ring exist to
  patch a leak that is an artefact of one specific choice — caching the
  occlusion grid by baking it in the map file's own 8×8 blocks
  (`bake.rs`) — rather than being inherent to "what stands between a
  flame and the ground" as a question. If solids were held in a structure
  queried directly by a frame's rectangle instead — an R-tree or a BVH
  over every solid in the facet, built once — there would be no block
  boundary for a solid to leak across, and no spill/ring to build at all.

  Why it was not built that way, as best this can be reconstructed
  without measuring: the shader still needs a **flat per-tile texture**
  (`Occlusion::bytes`/`id_bytes`/`solid_bytes`) to walk in `blit.wgsl`,
  and WGSL has no tree traversal — so a tree only ever helps the CPU
  side, "which solids does this rectangle see", and the rasterisation
  into a flat grid (this step's `Solid::footprint`, in different clothes)
  is still a separate step afterwards either way. Block-based baking also
  happens to align with the file's own I/O chunking
  (`WorldMap::statics_in_block` is a contiguous slice per block for free),
  which a tree built from the same statics would not give up for
  nothing, but would not obviously need either.

  What would settle it rather than argue it: whether a persistent tree,
  queried per frame, actually beats "bake per block, cache blocks, paste
  a ring" on the numbers `tests/cost.rs` already reads — build cost once
  at load, per-frame query cost, and memory, over Britain at the widest
  zoom. If it wins, the honest scope is large: decision 38.1's
  grid-of-references, the cache in `bake::Bake`, and the whole shape of
  the spill this step just built would be replaced rather than extended,
  which is why this stays a backlog entry and not a step under decision
  38 — it is a challenge to decision 30/38 itself, not a thing decision
  38 asks for.

## The shadow ray walk

**Decision 6. The shadow test is a walk along the ray through a tile grid,
in the shader.** One `Rgba8Uint` texture per frame covering the tiles a
flame could reach — `(z_bottom, z_top, opacity, present)` per tile — and a
fragment multiplies the opacities of the cells between it and each flame.
Not a mask rendered per light: one texture is uploaded once for the frame
instead of sixty-four, the shadow edge is exact rather than the resolution
of a mask, and the cost is paid only by fragments that are inside a pool
at all.

This was also claimed to be *cheaper* than what it replaced — every
fragment of the screen ran the old loop over all 64 lights, and here a
fragment outside every radius was said to leave the loop immediately.
**Step 6 measured it and the claim is wrong.** A fragment outside a
light's radius `continue`s to the *next* light; there is no way out of the
loop, so every fragment still runs 64 iterations whatever is on screen,
and those misses are 63% of what the whole pass costs. The saving is real
but it is a smaller one: what a miss skips is the ray walk, not the
iteration. See "Point lights" backlog, "found while measuring it", for the
numbers and the shape of the fix.

**Decision 14. The shadow ray walks the cells it crosses, and spends the
length of each crossing.** ~~Half superseded by decision 18~~ — the length
is what a *body* spends and the walk still spends it there; what a
**panel** does is decided where the ray pierces it, and the paragraph
below about how wide the gradient is now describes only the vertical half
of it. The rest stands, and the first paragraph is why the walk visits
cells at all.

**14 (as written).** Not a fixed number of samples along the segment: at
two tiles apart that was one interior point, so whether a fragment was in
shadow was decided at the resolution of a tile and every shadow in the
frame had a tile's straight side. A grid traversal visits exactly the
cells the ray passes through, and it knows how long each crossing is and
what share of it falls inside the span the tile occupies.

Having the length is what makes the edge a gradient: a ray clipping a wall
tile's corner keeps most of its light, and one grazing the top of a wall
is dimmed rather than switched. How wide that gradient is is not one
number — a flame is a body, not a point, so an occluder against the thing
it shadows draws a sharp edge and a distant one draws a penumbra. Its
width is `FLAME_SPREAD * t / (1 - t)`, `t` being how far along the ray the
occluder is from the lit end: the ordinary similar-triangles answer, for
one division rather than a second ray. It is capped below a tile, because
a wall crossed squarely must stop *all* of the light or rooms leak — which
is the same conservative direction decision 5's union takes.

**Decision 17. An occluder is a panel on a named edge, now that the art
will name one.** Decision 3 made an occluder the whole tile and gave the
reason: nothing in `tiledata.mul` says which edge a wall stands on, and
reading it off the silhouette "is a subsystem", where a wrong guess opens
a corner of a room to the street. **Step 15 built that subsystem**, and it
refuses rather than guesses — so the reason has expired and the cost has
not.

What the whole tile costs is light that travels *alongside* a wall. A
lamp mounted on a house is shadowed by the next tile of its own wall, so
the street it hangs over comes out with a band of darkness that nothing
visible is casting. That is how this was found: a player pointed at
Britain 1439,1692 and `light::sample` answered `stopped at (1440, 1692)` —
a tile that does hold a wall, and whose wall the ray never goes through.

So a cell carries **which sides of its tile are occupied**, four bits, and
a ray is stopped only where it *crosses* one of them. Three things make
that cheap:

- The walk already knows. `boundary.x < boundary.y` is which boundary is
  being crossed and `toward` is the direction, so the side a ray leaves by
  is two comparisons, and the side it enters the next cell by is the
  opposite of it.
- The cell already has room. `Rgba8Uint` is `(z_bottom, z_top, opacity,
  present)` and `present` was a byte holding a bare yes; it is `PRESENT |
  mask` now.
- The face is already measured: `Sprite::face`, once, when the picture is
  packed.

~~**And the flame's own tile stops being exempt.**~~ **Reverted, and the
picture is what reverted it.** The argument was sound and the frame it
produced was not: the flame sits at its tile's centre, which is inside
the panel, so a ray leaving it does cross the wall — and since every lamp
in a city is mounted on a building, the whole of Britain came out with its
walls lit from the inside and **not one pool of light on any street**.
The starburst that motivated the change was mostly decision 18's spokes
and went with them; what was left was a lamp that lights both sides of the
wall it hangs on, which was the defect this file had carried since its
first backlog and was much the smaller of the two — closed later by
decision 26's mounted-flame move (see "The G-buffer bridge" archive).

So decision 3's rule stands: **neither end of a ray is shadowed by the
tile it is on.** The lit end because a wall's two faces are one tile and
there is no telling which of them a pixel is on; the flame's end because a
sconce is mounted *on* a wall. What would answer it properly is knowing
which *side* of its tile a mounted light hangs on — the panel's own side
is in the grid already, and a lamp pushed just outside that plane would
light the street and be stopped by its own wall going the other way. That
is the shape of the fix, and it is what decision 26 later built.

The tile *being lit* stays exempt whatever it holds, and the asymmetry is
the point. A wall's two faces are one tile — the backlog carried that
since the sun arrived — so a pixel's fraction is clamped inside its tile
whichever face it is on, and testing its own panel would darken whichever
face the flame is not behind. There is no telling which that is. The
flame's position is known; the fragment's side is not.

Three answers and not two, and the third is the one to be careful about.
A mask of **all four** is "it stands up and the art would not say" — a
corner, a post, a tree — which is the whole-tile occluder decision 3
started with, so an unread graphic behaves exactly as it did. A mask of
**zero** is a *lid*: something horizontal, whose occlusion is entirely its
`z` span and which no vertical side describes. The client's own `FLOOR`
bit decides that and not the detector, because a floor whose silhouette
happened to read as a wall would otherwise stop three quarters less light
than it does today.

**And it deletes the door problem rather than solving it.** Decision 11
needed a table of which graphics are open leaves, because an open door
has the flags of a shut one. It does not need one now: where the detector
reads both, an open leaf is on the *perpendicular* edge of its tile — 28
pairs out of 28, never once the same axis — so a shut door blocks the
doorway and an open one blocks the side of the tile it swung against, out
of the geometry, with nothing knowing what a door is. Which is what
decision 11 always claimed to be doing — see "Doors" below.

**Decision 18. A panel is *pierced*; a body is *travelled through*. The
length of a crossing is the wrong question for a surface.**

Decision 14 gave every occluding cell one rule: what it stops is its
opacity scaled by how far the ray ran inside it, over a softening width.
That is right for a solid and wrong for a plane, and the wrongness is not
subtle — it is what drew the **spokes**. A thin bright ray fanned out of
every lamp standing near a wall, one per tile corner, straight through
walls with no hole in them.

The mechanism, because it is worth being able to recognise again: a ray
that clips the corner between two panels leaves the first cell
*sideways*, so that cell's own face is never among the sides it crosses;
and it enters the second cell *across the corner*, where the crossing is
a hair long, so `length / soft` rounds to nothing. Two cells, both holding
a wall, and the ray passes both.

A panel is a surface. What it does to a ray is decided where the ray goes
through it: at a point, at a height, once. So the walk asks, for each side
of the cell the ray actually crosses, what height the ray is at *there* —
and the tile's `z` span answers yes or no. There is no length in it.

Three consequences, all of them measured rather than reasoned:

- **All four sides is a body, not four panels.** A mask of `EDGE_ANY`
  means "it stands up and the art would not say", which is the whole-tile
  occluder — and a roof is a lid five `z` deep. Pierce-testing a slab is
  the "stepped over the top of a wall" failure this file already carries,
  arriving from the other side: a 45° ray that enters a roof's cell at 19
  and leaves it at 22 pierces neither side inside the span while passing
  straight through the middle of it. It lit the floor of a sealed house.
  ~~Lids and whole tiles keep decision 14's length; only a cell whose art
  named one, two or three sides is pierced.~~ **Half superseded by
  decision 24**: a whole tile now keeps the length *and* is pierced on the
  sides it is crossed by, the larger answer winning, because "it stands
  up" is what a house corner falls back to and the sliver was leaking. A
  lid is unchanged and keeps the length alone.
- **The penumbra that survives is vertical.** A ray grazing the top of a
  wall is dimmed rather than switched, over a band of the same
  similar-triangles width decision 14 derived. Sideways there is no
  longer a gradient for a named panel: the shadow's edge is where the
  geometry says it is, which is a straight line at any angle rather than
  a staircase on tile boundaries, and that was the actual complaint. The
  band is centred on the *top* edge and hangs below the bottom one,
  because a wall is based on the ground it stands on and the ray a person
  looks at — a torch and a floor, both at `z = 0` — runs exactly along
  that base. Centred there too, every wall in the frame passes half its
  light along the ground: measured at `0.378` against an ambient of
  `0.356` before the line said so.
- **A corner is answered rather than left open.** Where the two boundaries
  land together within `CORNER_TIE`, the ray is crossing the point where
  four tiles meet, and the walk asks *both* of the cells that share it —
  at the height the ray passes through the corner — before stepping
  diagonally past them. That is the supercover walk this pass had wanted
  since its first version, at two extra samples on the rays that hit a
  corner exactly rather than at twice the samples everywhere. It closed
  the diagonal gap
  (`a_ray_slips_between_two_walls_that_touch_at_a_corner` used to pin the
  leak and now pins its absence) and it is also what makes the two
  implementations of decision 9 agree: a ray through a corner is a knife
  edge, and the parity test found a pixel where the CPU stopped and the
  GPU did not. *(The exact numerical shape of this tie-break — `corner_tie`,
  its `PANEL_THICKNESS`-derived width, and every boundary-precision bug
  found in it — is [`lighting_raymarch.md`](lighting_raymarch.md)'s own
  subject, not repeated here.)*

**Decision 23. A wall does not shadow the rest of the wall it is part
of.**

The second thing reported from the same picture: a thin dark stroke down
every tile seam of a wall, appearing only when the lamp is *beside* the
wall rather than in front of it.

Decision 16 is why. A wall's face lies **on** the panel it is the face of,
so a pixel of the face is a point in the plane of its own tile's panel —
and the panels of the tiles either side of it are in that same plane. A
ray from that pixel to a lamp half a tile out from the wall crosses the
plane almost at once, and *where* it crosses is a little further along
the run than the pixel is: for the pixels near the far end of each tile
the crossing lands in the **next** tile, whose panel is a wall. The ray is
stopped by the wall it is standing on the face of. The perpendicular case
is clean because the crossing then lands in the pixel's own tile, which
is exempt — which is exactly the difference the report described.

A run of wall is one surface and no part of a surface shadows another
part of it. So a panel on the same side of its tile as the lit end's own,
on the same *line* — the same row for a north or south face, the same
column for an east or west one — is not an occluder for that ray.
Anything else about that cell still is: a wall tile that also carries the
perpendicular face of a corner stops the ray on that face as it always
did.

The elevation view is what this was found in and it is worth keeping the
order: the artefact is invisible in a plan, because a plan's pixels are
on the *ground* and this is a defect of pixels on a *wall*.

**Decision 24. A thing that stands up is a surface on every side of its
tile, and not only a solid inside it.**

Decision 18 split a cell in two: a *panel* is pierced where the ray goes
through it, a *body* is travelled through and what it stops is scaled by
the length of the crossing. It put `EDGE_ANY` — "it stands up and the art
would not say which way" — with the bodies, and gave the reason: a roof
slab five `z` deep is pierced by neither of its sides at 45° while the ray
passes straight through the middle of it, and pierce-testing it lit the
floor of a sealed house.

That reason is sound and it covers only half of `EDGE_ANY`. The other
half is **every corner of every building in the world**, and it brought
the spoke back.

The picture came in as a lamp in a Britain street throwing a bright seam
at 45° out of a house corner, and the coordinates were `(1441, 1692)`.
What is actually there — `crates/client/render/tests/onsite.rs` prints
it, and this is what that file is for:

| tile | graphic | what the art says | in the grid |
|---|---|---|---|
| `(1440, 1692)` | `0x0037` | a south face | a panel on `S`, `z 0..=25` |
| `(1441, 1691)` | `0x0035` | an east face | a panel on `E`, `z 0..=25` |
| `(1441, 1692)` | `0x0033` | **nothing — a corner** | `EDGE_ANY`, `z 0..=25` |

A ray from inside the house to the lamp arrived at **85% strength**, and
the path is two cells long:

- It enters the last tile of the south run through that tile's **north**
  side and leaves it **eastwards**. It never crosses the panel that tile
  stands on — which is correct, and is decision 17's whole point: it is
  what lets a lamp light the street it hangs over.
- It then clips the corner tile. That tile is faceless, so it is a body,
  so what it stops is `length / soft` — and the sliver is 0.107 of a tile
  against a softening width of 0.7. It stops 15%.

Two cells, both holding wall, and the ray passes both. That is decision
18's own sentence, word for word, arriving in the one branch decision 18
did not change.

So a cell whose mask is `EDGE_ANY` is asked **both** questions and the
larger answer wins: the length it was travelled through, and the sides it
was crossed by, pierced at the height the ray is at there. The length has
to stay, because it is what answers the roof slab; the pierce is what
closes the sliver. A **lid** — mask zero, a floor, a rug, a road — is not
asked, and that is not an oversight: a horizontal surface has no vertical
side for a ray to pierce, and it is the one case where the `z` span really
is the whole of what the cell is.

`max` and not a sum, and the direction matters: nothing that was dark
before becomes lit. The change can only darken, which is the direction
this file has taken at every one of these forks — a missing pool is
easier to see than a room leaking into a street.

**What it costs is the last of the sideways penumbra**, and one test had
to be re-aimed to say so. `the_edge_of_a_shadow_passes_through_the_values_in_between`
swept across the fan out of an open doorway and read a gradient; a built
scene has no art, so every occluder in it was `EDGE_ANY`, and the gradient
it was reading was the length rule — the same softening that let the ray
through the house corner. It is now
`the_edge_of_a_shadow_lands_where_the_geometry_puts_it`, which asserts
what was worth having underneath it: the fan is wider than the doorway by
a *fraction* of a tile, so the edge is neither a tile boundary nor
nothing. Decision 18 already argued why a sideways gradient cannot be
right here — it is measured from the **cell's** boundary and not from the
**surface's** silhouette, so wherever a wall carries on into the next tile
it is wrong in both directions at once.

The penumbra that survives is vertical, it was never measured, and it is
now: `a_ray_grazing_the_top_of_a_wall_is_dimmed_rather_than_switched`.

**Two of the three defects in that report are not this one**, and both
are the same missing fact — that a corner is two faces and the art will
not name them. They are archived under "The G-buffer bridge", "found at a
house corner in Britain", with what each needs. **Decision 25 (see "The
G-buffer bridge" above) is that fact.**

**Decision 26.** See "The G-buffer bridge" archive above for the full
text — the mounted-flame move belongs there, since it is fundamentally
about how the attachment/facing system places a flame, though its effect
is on the walk's own self-shadow exemptions covered here.

**Decision 28. A surface does not shadow itself — which is not the same
as a tile.**

*"Neither end of the ray is shadowed by the tile it is on"* (decisions 3
and 17) was always reaching for this, and the tile was the only handle
available at the time. The reason it gave is a reason about a *face*: a
wall's face lies **on** the panel it is the face of, so that panel cannot
be between it and anything, and a pixel of a wall claims a fraction
clamped inside its tile whichever face it is on. An **upright**
billboard's pixels are inside their tile too.

A **floor** pixel on the same tile is not ambiguous at all. It is the
ground, it is inside the room, and the ray from it to a lamp in the
street crosses the panel its own tile stands on — so a wall tile's own
square of floor came out fully lit against a dark room, which is the seam
on the ground the corner report ended with. It is visible in the plan
view of `scene::sconce_on_wall` as a lit band along the wall's own row,
with the wall's shadow starting only beyond it.

So the exemption is asked of the **surface**: a face and an upright
exempt their own cell, a flat pixel does not. Two things bound it, and
both are the direction this file always takes:

- **Only a named panel.** A mask of all four is "it stands up and the art
  would not say", which is every tree, post and barrel — testing those
  would put a dark square under each of them out of a *fallback* rather
  than out of a measurement. A lid is not asked either: it has no
  vertical side, and the ground standing on a pier's plank is the open
  question the backlog already carries (see "found while drawing the
  boxes" above).
- **The flame's end stays a whole tile**, because decision 26 moved a
  mounted flame outside the plane its tile names, so what is left on that
  tile is not between it and anything.

`own_run` narrowed with it. It took the lit end's whole tile mask and it
now takes the side the pixel **is the face of** — which is what decision
23 says in words: a corner's perpendicular panel is a different surface
and stops the ray as it always did. A pixel that is not a face is part of
no run and gets nothing.

**Decision 32. A lid is a plane, and a plane is crossed rather than
travelled through.**

Decision 24 gave the walk two rules — a panel is *pierced at a point*, a
body is *travelled through* and scaled by the length of the run inside its
span — and a lid was put with the bodies. It reads right and it is wrong
by a number: **a floor is `height 0`**. Over the block of Britain
`artscan`'s `column` example reads, 4,534 of the 4,647 lids are zero deep.
A span of no depth has no length inside it, so `share` came out `0.0` for
every floor in the world and a lid stopped exactly nothing. What a player
sees is a house whose upper storey is lit from under its own floorboards,
the upper wall brightest of all, because a wall's face takes the ray head
on. Reported from the client, reproduced in `scene::storey_over_a_torch`.

So a lid gets the third rule, and it is the one its geometry asks for:
**did the ray get from one side of the plane to the other inside this
cell**. Not a pierce either — a pierce is a point on a *vertical* plane at
a height, and a lid has no height to be pierced at.

Two things about it are decisions rather than arithmetic:

- **The crossing is strict.** A ray that runs exactly along the top of a
  lid — a candle standing on the floor it lights, both at one `z` — has
  gone through nothing. This is `pierces`'s own asymmetry (its band hangs
  *below* the bottom edge because a wall stands on the ground a ray runs
  along) arriving at the surface that has no thickness for a band to hang
  under. Counting a touch would lay half a floor's shadow across every
  room lit from inside it.
- **The softness is the flame's, and it is measured at the flame.** The
  plane cuts the source, so what gets through is the share of the source
  left on the lit side: a flame standing in the plane of a lid is half
  cut by it, one a storey below it is wholly under it. A sunbeam is a
  point source and gets the hard edge a point source casts, which is the
  same `spread` parameter that already tells the two ends apart.

What a floor pixel of an upper storey gets from a torch *below* it is not
this rule's business and never was: decision 27 already refuses it,
because a flat surface looks up and the flame is not on the side it looks
at.

`light::crosses` and `blit.wgsl`'s are one formula, held by the parity
test. Nothing else in the walk's *rules* moved — a body keeps decision
24's length and its second pierce, a panel keeps decision 18's.

**What did move is three exemptions, and the rule stopping light was
worth nothing until all three did.** Each was invisible from the others,
and a scene that read one spot four tiles from the flame would have
passed with any two of them fixed:

- **Neither end's own cell may exempt a lid.** Both exemptions are
  statements about things that *stand up* — a pixel lies on the panel it
  is the face of, a billboard's pixels are inside their own tile, a
  mounted flame burns outside the plane its tile names (decision 26).
  None of that is true of a horizontal plane. A sconce at `z 36` and the
  storey's wall at `z 45` are the **same tile** with the floor at `z 40`
  between them, which is how a real house is built, and both ends were
  exempting it. It costs nothing where the exemptions earn their keep: a
  ray only crosses a plane inside its own cell when the other end is
  nearly straight above or below it.
- **A vertical ray still has one cell to ask.** "Straight up or down: the
  only cells on the line are the exempt ones" was true when the exempt
  cells were exempt in full, and it is the shortcut a torch directly
  under a plank falls through. The walk now asks that one cell's lids
  before returning.
- **Both ends of the ray stand where they are drawn, not inside what they
  are drawn on** (`stand_clear`). A face pixel is walked from a hair in
  *front* of the plane it is the face of, and every point of the world
  from a hair *above* whatever it lies on. The first is geometry the
  attachment cannot carry: `statics.wgsl` keeps a face pixel a
  hundred-and-twenty-seventh short of its own plane, because a fraction
  of exactly one names the next tile and the attachment's tile is what a
  click selects — right for the attachment, wrong for the walk, because
  the floor whose edge meets that plane belongs to the tile in front and
  the ray was crossing it in the wall's own column, which has no plank
  over it. The second is what the strict crossing test costs: a point
  whose `z` is exactly a floor's lies *in* the floor, so the ray runs
  along the plane rather than through it. Strict the test must stay —
  inclusive, it lays half a floor's shadow across every room lit from
  inside it — so the point moves onto the boards instead, and so does the
  flame, because a candle stands on a floor rather than in it. **Neither
  nudge closes the line alone**, which is why they are one change: with
  only the height the ray still crosses the plane a column too early, and
  with only the offset it starts in the plane and runs along it. Only the
  walk moves; picking, the wireframe and the debug views still read the
  wall's own tile.
- **An exemption reaches only as high as the surface it is about**
  (`on_surface`). A tile of a two-storey house carries a wall per storey —
  `0..20` and `20..40`, two surfaces since step 21.2 — and a pixel at `z
  25` lies on the upper one. The lower one is under its feet and occludes
  it exactly as anybody else's wall would. Exempting it let every ray out
  of the room below climb the column of its own wall tile, which is the
  one tile a house's floor never covers. This is decision 28 said with
  the `z` it never had, and it narrows `own_run` by the same argument.

### Steps 4 and 13: the walk built and wired

- [x] **Step 4. `blit.wgsl`.** Reads the attachment and the occlusion
      texture, computes the world distance and the ray's product of
      opacities.
- [x] **Step 13. The ray walks its cells.** Decision 14: a grid traversal
      with the length of each crossing and the share of it inside the
      tile's span, and a penumbra whose width is `FLAME_SPREAD * t / (1 -
      t)`. `light::walk` learns the same walk; the parity test is what
      says they agree.

### Backlog: found while asking why a lamp on a house does not light the street

- ~~**Thin spokes still fan out of a lamp standing against a wall.**~~
  ~~**A ray through the corner between two panels passes between
  them.**~~ Both closed by decision 18, and they were one defect: a panel
  scaled by the length of a crossing passes a ray that clips the corner
  between two of them, through the first sideways and through the second
  over nothing. What is still true of it is that a named panel's shadow
  edge is now exact sideways — a straight line at the angle the geometry
  says, rather than a staircase on tile boundaries — and whether that
  wants softening is a question to ask of a moving picture, not of a
  still.
- ~~**A cell merges a lid and a panel into one mask and one span.**~~
  Closed by step 21.2 (see "The occluding world" archive), and the
  entry's own reading of it was right in both directions: the span
  darkened air the map had nothing standing in, and the mask leaked a
  horizontal surface into the panel path. What it did not predict is the
  third — the *opacity* was a `max` too, so a pane beside a wall was
  opaque across the whole tile. "Two slots a cell" turned out to want 21
  on the worst tile in Britain, which is why the answer was a list and
  not a second slot.
- **`crate::doors` is now deletable.** Decision 17 answers an open door
  out of the geometry, so the ported table earns nothing the edge mask
  does not. It is left in for one reason: 40 of the 104 open leaves are
  graphics `facing` refuses — the wide ones that stick past their own
  tile — and for those the table is still the only thing that knows. When
  that number is measured against the picture rather than against the
  art table, the answer is probably to delete it.
- **The atlas is now an input to the occlusion grid.** `light::collect`
  and `occlusion::collect` take `Option<&StaticAtlas>`, because a facing
  is a property of a picture and only the atlas has pictures. `None` is
  every occluder as a whole tile, which is what a built scene gets and
  what the tests that predate this still assert on. It is also the
  eighth argument of `light::collect`, which is one over what clippy
  likes and is allowed with a note.

### Backlog: found while writing this plan

- ~~**A sconce lights through its own wall.**~~ The oldest entry in this
  backlog, closed by decision 26. It wanted the wall's facing and it got
  it — by way of step 15 measuring one and decision 26 using it to place
  the flame rather than to excuse it.

### Backlog: found while starting again, from the picture rather than from the argument

- ~~**A lamp mounted on a wall wants pushing off it, not exempting from
  it.**~~ Done, decision 26, and the entry's own guess is what was built:
  outside the panel, on the side the picture is drawn from. What it did
  *not* predict is that the same move would let the facing test drop its
  line exemption, which is what the reported defect actually was. The
  **shadow** walk still exempts a flame's own cell (decision 17's
  amendment) — it is just that a mounted flame's own cell is now the
  street rather than the wall, so the exemption stopped mattering where
  it did harm.
- **A scene has no art, so almost every scene tests the whole-tile
  occluder.** Two scenes hand `facing::silhouette` to the grid and the
  rest get `EDGE_ANY`, which after decision 18 is a *different code path*
  — the body, not the panel. The suite is therefore thin exactly where
  the change was: `torch_before_a_wall` is the only picture of a named
  panel's shadow, and the doorway and room scenes all measure the body.
  Giving every wall scene a silhouette would double the coverage for one
  line each, and it would also change what those tests assert, which is
  why it is written down rather than done.

### Backlog: found while asking why the light steps from tile to tile

Decisions 13 and 14 (above) are what came of the first three of this
backlog's items, and these are what was left after:

- ~~**The sun's ray still steps a whole tile at a time.**~~ Done — see
  "Sunlight" below for the full write-up.
- **The penumbra is a width, not an area light.** Decision 14's `t / (1 -
  t)` is the right *shape* off one ray, but it softens by how far the ray
  ran inside the cell rather than by how much of the flame the cell
  hides. Where an opening is a tile wide and the ground is right behind
  it — a doorway — the honest answer is still nearly a hard edge, which
  is what a point light through a one-tile aperture is. Several jittered
  rays to points on a sphere of the flame's size would be the real thing,
  at that many times the walk.
- **`FLAME_SPREAD` and its two bounds are invented**, like
  `occlusion::PANE` and `light::flame`. What holds them is a scene, not a
  file.
- **An upright sprite's fraction is clamped at its tile's edge.** A tree
  is a hundred pixels across and the attachment holds one tile per pixel,
  so the outermost columns of a wide sprite all claim the edge of the
  tile the thing stands on. It is the honest answer available — a
  billboard's pixels are not anywhere in particular — but it means a very
  wide sprite's lighting flattens towards its edges.

### Backlog: found while making a floor stop light (decision 32)

- **A lid is a plane per *tile*, and it has no sub-tile hole.** A gap in a
  floor is a tile with no plank on it — `scene::hole_in_a_floor` — and
  that is what a house's floors are made of, so it is enough for what a
  house does. An `occlusion::Aperture` is still refused to a lid on
  purpose (step 21.3): a hole is a rectangle in a plane, and the run
  coordinate a rectangle would be stated along is a *vertical* panel's. A
  trapdoor would want one, and reading it would want a silhouette
  measured from above, which no art in the install is.
- **The edge of a floor is a hard step at the tile boundary.** The
  crossing test is per cell and a lid fills its cell, so nothing softens
  where the planks stop. It is the same shape decision 18 left the walls
  in — the surviving penumbra is vertical, and the lateral one was
  removed because a cell-local softening is measured from the *cell's*
  boundary rather than from the surface's silhouette. Consistent, and
  worth remembering the first time a shaft through a floor is looked at
  closely.
- ~~**Directly beside a flame, a storey up, the floor still passes
  light.**~~ Closed by the three exemptions decision 32 had to narrow —
  the entry as first written blamed the own-cell rule alone and proposed
  the wrong fix. What it actually took: neither end's cell may exempt a
  **lid**, a vertical ray must still ask the one cell it stands in, and
  an exemption reaches only as high as the surface it is about
  (`on_surface`). The whole row of `scene::storey_over_a_torch` is now
  the ambient to six decimal places, the torch's own tile included.
- ~~**The line at the floor is the strict crossing test, seen.**~~ Closed,
  and by moving the *point* rather than loosening the test — see decision
  32's fourth paragraph and `light::stand_clear`. Reported from a frame as
  a bright stroke along a house's floorboards; `scene::storey_over_a_lit_room`
  is the house it was argued in, and on the real one at `1509,1637` the
  wall's face at the floor's own `z` went from `through 1.00` to `0.09`,
  brightness `0.62` to `0.24` against an ambient of `0.20`.
- **A flame's height is not its width, and for a day it was.** `crosses`
  cuts a source by the plane it straddles, and the band it does that over
  was `FLAME_SPREAD * Z_PER_TILE` — a flame a whole tile tall. A house's
  sconce burns three to five `z` under the floor above it (Britain's at
  `1491,1636` is `z 31` under boards at `40`), so a tenth of every one of
  them was above the plane and the storey over it read `through 0.09` — a
  faint wash on the wall, reported from a frame right after the line at
  the floor was closed. `FLAME_DEPTH` is its own constant now, half a
  tile, which is `FLAME_LIFT`'s number and the only one in the file that
  is about a flame's *height* rather than the lateral softness of what it
  casts. Both houses now read `through 0.00` with the blocking cell
  named. `scene::storey_over_a_lit_room` burns its flame at sconce height
  for this reason: on the ground it would be fourteen `z` under the
  boards, far enough that any band at all would pass.
- ~~**What is left above a wall is the flame's assumed size, not a
  gap.**~~ Mostly closed, and by the same category error one function
  over: a penumbra is the size of the source **across the edge it spills
  over**, and every edge this pass softens vertically — a wall's top, a
  hole's sill, a lid's plane — is horizontal, so what blurs it is how
  tall the flame is. `pierces` was given `soft * Z_PER_TILE`, a flame as
  tall as it is wide, and a ray passing three quarters of a `z` under the
  top of a wall kept two fifths of its light. `FLAME_DEPTH` now does that
  conversion everywhere. On the corner of Britain's house at `1509,1635`,
  over the wall beside it: `through 0.21 -> 0.00` at the wall's own top
  and `0.40 -> 0.11` three `z` above it. The lateral softness is
  untouched and still `FLAME_SPREAD`'s.
  What is left is `0.11` at that one height — `0.267` against an ambient
  of `0.251`, six percent — and it is a real penumbra rather than a leak:
  the flame burns four and a half `z` under the top of a twenty-tall
  wall, so the top of it genuinely clears the edge. Shrinking
  `FLAME_DEPTH` to an eighth of a tile would take it to nothing, and that
  is choosing a constant to make one pixel dark: at a quarter it is what
  the pictures show, four screen pixels to a `z` and a torch's drawn
  flame eight or ten of them.
- ~~**A strip of wall just above the floor line is still lit from the room
  below.**~~ Superseded by the two entries above: measured at the middle
  of a tile, which is not where a face pixel is. The band it reported at
  `z 40..42` is the seam at `z 40` and nothing at `41` and `42`.
  What was left, measured on a real house — the tile at `1490,1635` in
  Britain, a sconce at `z 36.5` on the tile southeast of it: the face
  reads `through 1.00` at `z 40` and `z 42`, and `0.18` from `z 45` up.
  The cause is geometry the map really has: **a house's floor covers the
  room and stops at the wall tile**, so the wall's own square is a
  column with no plank over it, and a ray from a flame near the wall
  crosses `z 40` inside that square rather than over a lid. It is a band
  a few pixels tall against a wall a storey high.
  What would close it is deciding that a wall tile is floored by its
  neighbours — a lid grown one tile into any wall tile that touches one
  at the same `z`. That is a *model* decision and not a bug fix: it
  invents a plank the map does not have, and the same invention would
  darken the street under an overhang. Left as a question for a person
  with the picture in front of them rather than settled here.

## Sunlight

**Decision 12. The sun is a direction; a sunbeam on the floor is the same
walk without an endpoint.** A flame is a point and the walk between a
fragment and it is bounded by the radius. Sunlight has no position: every
fragment walks the *same* direction — an azimuth and an elevation, in tile
units — until the ray leaves the grid or is stopped, and what it produces
is a wall's shadow lying across the street and a bright patch on the
floor behind a window. That patch is the honest form of "sun through a
window" in a tile world, and it is where this starts: a beam in the air
with no lit floor under it looks like a decal.

The beam itself — the visible shaft between the window and the floor — is
not geometry at all here, because nothing in this renderer draws the air.
It is a screen-space pass: the sunlit fragments are a mask, blurred along
the sun's direction *on the screen* and added. That is a second pass and
a separate step, and it only makes sense once the patch it grows out of
is right. *(Still not built — see `lighting.md`'s Status, "the
screen-space shaft for a sunbeam".)*

Two things sunlight needs that firelight did not, and both are why it is
not simply a 65th light:

- **A window has to pass some light.** `occlusion::opacity` is binary
  today and `WINDOW` is opaque, which is right for line of sight and
  wrong for a pane at noon. The byte and the shader's multiply are
  already there; what is missing is the rule, and the rule wants a scene
  to be tuned against. *(Later given a value — `occlusion::PANE` passes
  four fifths, see decision 4 above.)*
- **The ray is long.** A flame's walk is bounded by nine tiles; the sun's
  is bounded by how far a wall can throw a shadow, which at a low
  elevation is the width of the grid. That is a real per-fragment cost on
  every ground pixel of a daylit frame, where firelight's cost was paid
  only inside a pool — so the walk needs a cheaper bound (stop as soon as
  the ray is above the tallest occluder the grid holds) before it is on by
  default. *(Built as `Occlusion::tallest` — see step 11 below.)*

### Steps 11 and 17: sunlight built, the shaft not

- [x] **Step 11. Sunlight on the floor.** Decision 12's directional term:
      a sun direction in the uniform, the same grid walk without an
      endpoint, a wall's shadow on the street and a lit patch behind a
      window. `WINDOW` no longer borrows `NO_SHOOT`'s answer —
      `occlusion::PANE` passes four fifths — and the sun is F8 in the
      app, off by default. Step 6 has since measured it — 0.021ms a
      frame, 9% of the pass — so what keeps it off is no longer the cost
      but the tile-stepped ray the backlog named — and that has since
      been fixed too, so what keeps the sun off by default is now only
      that nothing has asked for it to be on.
- [ ] **Step 17. The shaft.** The screen-space pass of decision 12, over
      the mask step 11 produces — and, once step 16 exists, over the beam
      from a window too. Nothing in this renderer draws air, so a visible
      shaft is a blur of the lit mask along the light's direction *on the
      screen* and nothing else. It only makes sense after the patch it
      grows out of is right. **Not started.**

### Backlog: found while building the observability and the sun

- **A room with no roof is a courtyard, and the sun is right to flood
  it.** The first sunlit scene had four walls and open sky, and at 45°
  the sun clears a two-tile wall in two tiles — so the floor was fully
  lit and the window proved nothing. `scene::sunlit_room_with_window` has
  a roof for exactly this reason, and it is worth remembering when a real
  house looks wrong: ask what the cutaway did to its roof before asking
  what the sun did.
- **The sun has no facing either.** A wall's two faces are one tile, so
  both are lit when either is — the same hole decision 3 leaves for a
  sconce, arriving from the other direction. It is more visible with a
  sun than with a torch, because every wall in the frame has a shaded
  side that is not shaded. *(Still open — see `lighting.md`'s Status.)*
- **`occlusion::PANE` is a guess.** A fifth stopped, from nothing: the
  client has no number for how much light glass passes. It is the one
  value in the pass invented rather than read, along with `light::flame`
  and `light::midday`.
- **The diagram does not draw the sun.** `debug::diagram` marks flames and
  occluders and samples brightness, so a sunlit scene reads as a field of
  `+` with darker tiles in the shadows — legible, but there is no `☀` and
  no arrow saying which way the light comes from.
- **The sun's ray is walked for every ground pixel.** Firelight's cost is
  paid only inside a pool; this one is paid everywhere the sky is
  visible. The ceiling test (`Occlusion::tallest`) makes it two or three
  steps over open ground rather than 32. It is no longer gated on cost;
  what still keeps F8 off by default is that its ray steps a whole tile,
  which the backlog has carried since step 11.
  ~~and the number is still unmeasured~~, and step 6 has now measured it
  at 0.021ms a frame, which is a tenth of what firelight costs on the same
  frame. ~~The cost was never the reason to leave F8 off; the tile-stepped
  ray below is.~~ Both are answered: the walk is a walk now and costs
  0.057ms of the 0.287ms pass. See "found while asking why the light steps
  from tile to tile" (in "The shadow ray walk" archive above) for the full
  write-up of that fix, including the "inverted" window-brightness bug it
  found along the way.

## Point lights: falloff, beam, ambient, and the screen-space glow

Carried from `client.md`'s firelight backlog and still true at the time
this plan was written:

- `light.mul` / `lightidx.mul` are not read; `light::flame` is the
  stand-in.
- Nothing a mobile carries burns — a player holding a torch makes no
  light. *(Later narrowed: the local player's own carried torch does now
  make light — decision 15/step 18 below. No other mobile's does — still
  true, see `lighting.md`'s Status.)*
- The ambient is a key (F10), not a clock.
- A light is placed by its tile, not by its sprite. *(Still true — see
  below.)*

**Decision 7. Distance is three-dimensional, with `z` in tiles.**
`Z_PER_TILE = TILE_WIDTH / Z_STEP = 11`: eleven `z` units is one tile's
width, which is the ratio the projection itself uses. A flame reaches as
far up and down as it reaches sideways, which is what stops a cellar from
lighting the street even where nothing occludes.

**Decision 15. A flame in a hand is a cone, and the hand is not a
shutter.** Everything on the map lights every direction, and a light
carried by a character must not: an omnidirectional pool centred on a
body lights the wall behind it exactly as brightly as the one it is
walking towards, and the eye reads that as the character *glowing* rather
than as the character *carrying* something. So a `Light` may have a
`Beam` — an axis and the cosine of a half-angle — and the pool is
multiplied by how far inside that cone the lit spot is.

A cone and not a second radius, and the ordering is what makes it cheap: a
fragment outside the radius never asks about the angle, and the whole of
a beam is one dot product against a direction the CPU normalised once.
`(0, 0, 0, -1)` is "lights every way" — no cosine is below `-1`, so a fire
standing in the world pays a comparison and never the arithmetic.

Two numbers keep it from reading as a stencil rather than as light. The
rim softens over `BEAM_EDGE` of the way in from it, because a hard edge
is found by the eye instantly — the same complaint the tile-edged shadows
drew. And `BEAM_SPILL` of the flame escapes it in every other direction,
because a hand is not a shutter: the arm holds the torch out in front and
the body is behind it, and neither of those stops a flame from being a
flame. Without the spill the one thing the player is looking at — their
own body, whose pixels are on the flame's own tile and directly above and
below it — is the only black shape in the frame. A quarter, so that what
is in front is four times what is beside and the direction is legible at
a glance.

Where a beam is aimed is the *facing*, which is the one direction this
pass can have for nothing: it is on the wire for every mobile, and the
client already holds it to pick which way a sprite is drawn. That is the
whole of why this arrives before the wall facings of step 15 — the light
knows which way it is pointed even though a wall does not.

**Decision 19. A window is not an emitter.**

615 of the install's statics carry `LIGHT_SOURCE`, and 80 of the 163
named "window" are among them — `0x0103`, `0x2BBF`, the shutters at
`0x2501`, the windowed walls at `0x2B7D`. `light::flame` answers `TORCH`
for any graphic it has no name for, so a street of houses was a street of
six-tile warm pools with nothing burning in them: **64 flames in a
Britain frame, of which seven were fires.** And every one of the other 57
stood *inside a wall*, which is where the whole complaint came from — a
light in a panel lights the panel, and what escapes it is whatever the
geometry lets through. The spokes and the missing pools are the same 57
lights seen from two directions.

Three answers were on offer and this is the one chosen: a window is not a
light. It is a hole with glass in it, it is already in the grid as
`occlusion::PANE`, and what should make it glow is a candle behind it —
which is the one thing this pass can already do. The flag is the
client's way of saying "draw a glow here" and this renderer answers that
with geometry.

Stated as **"a light source that stops light is not a flame"** rather than
as a list of window graphics: the property is the one that matters, it is
already computed for the grid, and a shard's own lantern goes on burning
for free. The conservative direction is the right one here too — a
missing pool is easier to see than sixty invented ones.

**Decision 20. While the point lights are the subject, the ambient holds
still.**

`docs/archive/render/lighting_world.md`'s sky field — a room under a roof darker than
the road outside it, before anything burns — is **off by default** (F6),
and the ambient is one colour per frame again. Not because it is wrong:
because it changes the ambient of *every tile in the frame*, and a pool
that looks wrong indoors is then two questions at once. It is also the
larger thing in a picture — in the light view a city reads as a field of
dark building-shaped blobs with the pools somewhere inside them — so it
hides exactly what a person judging a falloff needs to see.
`light::Ambient::flattened` sums the two terms back into the one they
were split from, so the flat picture is not a lesser version of the
field: it is the frame this pass had before the field existed, which is
what a difference is measured against.

### Steps 3, 18 and 20: world-coordinate lights, the held beam, the glow not built

- [x] **Step 3. `light.rs` in world coordinates.** A `Light` becomes a
      tile, a `z`, a radius in tiles. `place` and `FLAME_LIFT` go; the
      lift becomes `z` units.
- [x] **Step 18. The light in the player's own hand.** Decision 15:
      `light::Beam`, one more `vec4` per light in the uniform, `cone` in
      `blit.wgsl` and `Beam::lights` beside it in Rust, and
      `Lighting::hold` for a flame no walk of the map could have found —
      nothing on the wire says a hand is carrying anything, so
      `light::carried` builds it from the player's tile and facing and
      the app puts it into the frame after the sort. Never the flame
      dropped when a tavern's candles fill the array. F7 in the app, on
      by default, and it does nothing in plain daylight where the whole
      pass is a copy.

      `scene::lantern_in_a_room` is the fixture and it is a room with
      **no torch in it**: the only flame is the carried one, so every
      bright pixel is the beam's. Held to by three tests — the floor and
      the wall ahead against the floor and the wall behind, the rim's
      gradient and its width measured at four tiles out (`4 *
      tan(30°)` ≈ 2.3 tiles), and the GPU parity test over the same
      scene, which is the only parity fixture whose cone is not
      identically one.
- [ ] **Step 20. The glow, as its own layer.** Decision 21 (see
      "Overview" archive above for the full text): the screen-space halo
      around a flame's own sprite, added over the lit frame rather than
      multiplied into it.

      It is a second term in `blit.wgsl` and not a second pass — the
      lights are already in the uniform, and what it needs beside each
      one is where that flame landed **on the screen**, which the CPU
      knows when it collects them and the shader cannot recover from a
      tile. So: one more `vec4` per light, the flame's viewport position
      and the halo's radius in pixels, and an `added` term after the
      multiply at the end of `fs_main`.

      Three things to decide when it is picked up, and none of them is
      decided here:

      - **Whether the halo is occluded at all.** Cheapest is not: glare
        is in the air and a wall between the eye and a lamp still glares
        round it. But a lamp in a sealed cellar would then glow through
        the floor above it, which is the failure the world layer exists
        to prevent — so the honest first cut is probably to gate the
        halo on the *world* term at the flame's own tile, which the pass
        has already computed.
      - **Its falloff, which is not the world layer's.** A halo is a
        glare and falls off much faster than a pool of light; reusing `(1
        - d)²` would draw a second pool over the first and double every
        complaint about flatness.
      - **Where the sprite is.** The flame's screen position is the
        sprite's, and `light::place` gives a tile. "A light is placed by
        its tile, not by its sprite" (above) is a nuisance for the world
        layer and a blocker for this one.

      Off by a key while it is being tuned, like the sun and the sky
      field, and for the same reason: a picture with one thing changed
      in it is the only picture anything can be judged from. **Not
      started.**

### Backlog: found while measuring it (step 6's numbers)

- **The loop has no way out, and the misses are the pass.** Decision 6
  said a fragment outside every radius leaves the loop at once. It does
  not: `blit.wgsl` `continue`s to the next light, so every fragment of
  the screen runs 64 iterations at night whatever is on it, and those
  iterations are 0.135ms of the 0.215ms lighting costs. What a miss skips
  is `reaches` — the ray walk — which is why the *lit* eighth of the
  screen adds only 0.025ms on top. The shape of a fix is a bound the
  whole loop can be skipped against: the lights are already sorted by
  distance from the eye, so a per-frame screen rectangle for the union of
  the pools, or a coarse per-tile light list, would let most fragments do
  one test instead of sixty-four. Worth doing when a frame is short of
  time and not before — the whole pass is 1.3% of a 60Hz budget.
- **The expensive half is the CPU, by thirteen times.** 2.83ms in
  `light::collect` against 0.215ms in the shader, on the same frame.
  Everything argued about this pass so far has been about what a
  fragment does, and a fragment is not where the time is. Three separate
  things want fixing and they have three different fixes — which is why
  `cost.rs` reports them apart rather than as one number.
- **The map's statics over the lit bounds are walked twice a frame.**
  `light::collect` walks them for flames (`for_each_static_in`, 1.27ms of
  the 2.83) and then hands the same bounds, the same map and the same
  cutaway to `occlusion::collect`, which walks them again for the grid
  (1.56ms). Every static is read twice, its tiledata entry looked up
  twice, and its `z` tested twice. One walk with two visitors is the same
  answer for about half the price, and the two are already in one
  function — this is not a design change, it is a loop that was written
  twice.
- **A widest-zoom frame's grid is 187×187 cells and 10,212 of them
  stand.** `Occlusion` is rebuilt and reallocated every frame at 140KB;
  the number under it is 1.56ms, which is what makes that item worth
  doing rather than merely worth writing down.
- **The pass is measured on `Cutaway::OPEN` and no ground items.** A
  player standing inside a house is drawn with storeys removed, which is
  a *smaller* grid and fewer flames, so these numbers are the outdoor
  worst case rather than the average. Nothing here says what a cutaway
  costs, and the cutaway is rebuilt every frame too.

Step 6's own measured picture, for reference — Britain at the widest
zoom, 1920×1080 on screen over a 3840×2160 world image, drawn once and
lit five ways, 64 flames, 10,212 standing cells in a 187×187 grid:

```
  case   ms/frame    ns/pixel     over dark
  copy      0.173       0.084      -23.9%    Lighting::NONE, the pass as a blit
  dark      0.228       0.110         0      the grid and the ambient, no flames
   far      0.363       0.175      +59.6%    the same 64 flames, 1000 tiles away
 night      0.388       0.187      +70.3%    the frame as played
   sun      0.249       0.120       +9.3%    no flames, a midday sun
```

Lighting a night frame cost **0.215ms** of GPU over a plain copy — 1.3% of
a 60Hz budget — at that point in the pass's history. Of that, 0.135ms was
`far`: sixty-four flames that reach nothing, on every fragment. The pools
and their ray walks — the part with all the arithmetic in it — were the
remaining 0.025ms, because only an eighth of the screen is inside any
pool. **The misses cost five times what the light does**, which is
decision 6's claim inverted (see the backlog entry above). The CPU side
at the same point: `light::collect` was 2.83ms, of which
`occlusion::collect` was 1.56ms and laying both planes out as bytes for
the queue was 0.04ms — thirteen times the whole GPU pass, paid on every
frame the camera moves. This baseline was later superseded by the
surface-list and bake numbers under "The occluding world" archive's step
21/21.5 entries — see those for the current shape of the cost.

### Backlog: found while putting a light in the player's hand

- **Only the player carries one.** `light::carried` is built in the app
  from `self.player`, so a second character walking past with a torch
  makes no light at all — and the crowd's mobiles have a facing and a
  tile, which is everything the constructor needs. What is missing is the
  *reason to believe it*: nothing says a given body is holding anything,
  and giving every mobile on screen a beam would light a market square
  from sixty invented torches. *(Still open — see `lighting.md`'s
  Status.)*
- **Nothing on the wire says a hand is holding a torch.** The equipment
  layers are parsed here already (`0x2E` and the paperdoll's items), and
  a torch in `Layer::OneHanded` is exactly the fact this pass is guessing
  at. Until it is read, `App::lantern` is a key that defaults to on —
  which is a client that lights the dark rather than a client that is
  right.
- **`HELD_BEAM_DEGREES`, `BEAM_EDGE` and `BEAM_SPILL` are invented**,
  joining `occlusion::PANE`, `FLAME_SPREAD` and `light::flame`. That is
  now six numbers in this pass that no client file has, and the honest
  way to hold them is one scene each rather than an argument each — which
  is what `scene::lantern_in_a_room` does for the last three.
- **The beam does not move with the sprite's own arm.** A carried flame
  is at the middle of the player's tile, half a tile up, whatever the
  body's animation is doing — so at the instant a step lands, the pool
  jumps a whole tile while the drawn body slides. It is the same "a light
  is placed by its tile, not by its sprite" the backlog already carries,
  arriving where it is most visible, because this is the one light that
  moves every frame.
- **A dark tile now has three causes and the diagram shows one.**
  `light::Reach` grew a `cone`, and `Sample`'s report prints it — but
  `debug::diagram` still draws brightness alone, so "behind the
  character" and "behind a wall" are the same blank cell in the picture.
  The shadow view (`View::Shadow`) has the same hole from the other end:
  it draws what the walk lost and knows nothing about where the light was
  pointed.

### Backlog: found while asking why a house's windows burn

- ~~**Eighty window graphics are flagged `LIGHT_SOURCE`, and every one of
  them is given a torch.**~~ Answered by decision 19, with the third of
  the three answers this entry offered: a light source that stops light
  is not a flame. Britain's 64 flames at the widest zoom are 7. The two
  the entry names and this does not do are still there to be done —
  `light.mul`'s shape by the static's `layer` byte, and the reference's
  rule about something opaque standing over `(x+1, y+1)`.

- **Eighty window graphics are flagged `LIGHT_SOURCE` (as written).** Kept
  because the scan is the evidence and the two unfixed answers are in it.
  Scanned over the client's `tiledata.mul`: 615 statics carry the flag,
  and 80 of the 163 named "window" are among them — `0x0103`, `0x2BBF`,
  the shutters at `0x2501`, the windowed walls at `0x2B7D`. `light::flame`
  answers `TORCH` for any graphic it has no name for, so a street of
  houses is a street of six-tile warm pools with nothing burning in them.
  Two things the reference does that this does not, and either one alone
  fixes it: the light's *shape* comes from `light.mul` indexed by the
  static's `layer` byte (ClassicUO `GameScene.AddLight`,
  `Game/Scenes/GameScene.cs:508` — `light.ID = data.Layer`, and
  `StaticTile::layer` is already parsed here), and a light is dropped
  entirely when something opaque stands over the tile at `(x+1, y+1)`
  above `z + 5` (`GameScene.cs:415`). The third answer, and the one worth
  arguing for: **a window is not an emitter at all.** It is a pane, it is
  already in the occlusion grid, and it should glow because a candle
  behind it does — which is what decision 4's fraction was for.
- **A windowed wall passes four fifths of the light.** The same scan:
  those graphics are `WALL | BLOCK | WINDOW`, and `occlusion::opacity`
  reads `WINDOW` before anything else, so a whole wall tile whose art has
  a window in it stops `PANE` rather than `OPAQUE`. The older ones do not
  carry `NO_SHOOT` either, so nothing rescues them. The pane is the hole
  in the wall, not the wall.

## Doors

**Decision 11. An open door is not a special case — it is a static that
stopped being an occluder.** ~~The client is *told* a door opened: the
item's graphic changes, and the open leaf's graphic is not `NO_SHOOT`.~~
**The second half of that is false, and it was checked against the client
only after somebody looked at a lit doorway.** `tiledata.mul` does not
distinguish an open door from a shut one at all. Measured over ServUO's
own thirteen door families (`Scripts/Items/Functional/Doors.cs`, where
every `BaseDoor` is `base(closed + 2 * facing, closed + 1 + 2 * facing,
…)`, so within a family the even offsets are shut and the odd ones are
open), the flags of the two are **identical in every one of the 104
pairs**: the wooden and metal doors are `NO_SHOOT` open and shut alike,
the gates are clear open and shut alike, the barred doors are `WINDOW`
both ways. 55 of 104 open leaves stop everything.

So today an open door lays a **whole tile of wall across its own
doorway** — a band of shadow with nothing visible casting it, which is
exactly what it looks like on screen, and the more visible for the leaf
beside it being brightly lit.

The intent of this decision stands and the mechanism does not: an open
door must occlude nothing, because decision 3's occluder is a whole tile
and a tile-wide wall in an opening is far more wrong than no occluder at
all. What is missing is the *fact*, and it is not in the client:

- not in `tiledata.mul`, per the measurement above;
- not in how the graphics are laid out — the `DOOR` flag comes in runs of
  1, 2, 4, 6, 7, 8, 11, 12, 13, 16, 20, 24, 29, 32, 80 and 98, so there is
  no parity to read an odd offset off;
- not in the art — "an open leaf's picture is wider than a tile" holds
  for 46 of the 104 pairs and no better, because a door swung to four of
  the eight facings is still 44 across.

Which leaves the door table itself, and it is ServUO's. `render/src/doors.rs`
and `data/doors.json`: the thirteen family bases, sixteen graphics each,
even shut and odd open — and `occlusion::opacity` asks it **before** it
looks at a flag, so an open leaf stops nothing and takes none of the
tile's sky.

Two things follow from where the question is asked. `opacity` takes the
**graphic** now and not only the tiledata entry, which is the general
shape rather than a door-shaped patch: a flag is a fact about a *picture*,
and anything that opens, lifts or breaks — a shutter, a portcullis, a
drawbridge — is a fact about the *thing*. And **a graphic the table does
not know keeps today's behaviour exactly**, so a shard's own door goes on
occluding rather than a wrong guess opening a room to the street; the
same refusal decision 15's detector makes.

Held to by three tests, two of which need a real install. The client is
the oracle a ported table needs: every one of the 208 graphics the table
claims is flagged `DOOR` in `tiledata.mul` bar four, which is the
client's own gap and not a mistyped base. The second asserts the module's
*premise* — that the stopping flags of an open leaf and its shut twin are
identical, 103 of the 104 pairs, the one exception (`0x0683`/`0x0684`)
named rather than tolerated by a percentage. The day that stops being
true, the right move is to delete the table and read the flags, and the
test says so. The third is in the grid: the same `StaticTile` twice, two
graphics, and only the open one leaves no cell.

`server/world/src/doorgen.rs` ports the same `+ 2 * facing` rule for the
doors a shard generates. Two copies, because `client/*` and `server/*`
never depend on each other and a table of art indices is not something
both ends of the wire agree on. If a third reader appears, the table
moves down; the boundary does not.

**Later folded into decision 17's own geometry, and this table's own
usefulness shrank as a result** — see "The shadow ray walk" archive,
decision 17: where the facing detector reads both leaves of a family, the
open one sits on the *perpendicular* edge from the shut one in every
measured pair, so the ordinary panel-edge machinery handles the swing for
free and needs no table at all. The table above stays useful only for the
graphics the facing detector refuses (the wide leaves that overhang their
own tile).

### Backlog: found while asking what an open door does

- ~~**An open door is a tile-wide wall across its own doorway.**~~ Fixed:
  `render/src/doors.rs` and decision 11. What is left of it is the shape
  of the fix rather than the fix — see the two items below.
- **The shading half of a door already works.** 558 graphics carry
  `DOOR`; the ones `facing::facing_of` reads sit on a tile edge as
  squarely as a plain wall — median distance zero, none over two pixels —
  so an open leaf is shaded along the axis it swung to. Only the
  occlusion half was wrong, and decision 17 later closed that half too.
- **The door table is thirteen families and a shard's own door is not in
  it.** `doors::is_open` answers `false` for anything it does not know,
  so a custom door occludes shut or open alike. The right home for that
  fact is the shard — it is what changes the graphic — and the wire does
  not carry it. A pack that ships doors would want to ship the table
  beside them, which is the shape `data/doors.json` is already in.
- **Four of the 208 graphics the table claims are not flagged `DOOR`.**
  `0x0692`, `0x0844`, `0x0846`, `0x0873`. Measured and left alone: they
  sit inside otherwise solid families, so it is the client's gap rather
  than a mistyped base, and nothing reads the `DOOR` flag anyway. Worth a
  look if one of them ever turns up drawn.
- **A pane and a door are the same question asked twice.**
  `occlusion::opacity` now takes the graphic because a flag describes a
  *picture* and an open door is a fact about a *thing*. A shutter, a
  portcullis and a drawbridge are the same shape and none of them is
  handled; what they all want is the client knowing an item's state,
  which is a seam this half of the workspace does not have.

## The art-measurement pipeline

**Decision 31. The art is measured once, off the clock, and the engine
reads a table.**

`facing::facing_of` runs while the atlas packs a sprite — on the frame a
graphic is first seen, on the player's machine. That was right when the
measurement was one pass over 44×80 pixels and there was one of them. It
stops being right twice over: a scroll that introduces four hundred
graphics pays for four hundred of them at once (this file's backlog has
carried it as *"a second walk of pixels the atlas has just copied"*), and
every future measurement is a bigger one — an aperture is a hole to be
found, a corner is two fits, a mesh would be a solve.

So the measurement moves **out of the frame entirely**: a tool reads an
install and writes a table, and the client loads it. The budget goes from
a frame to a minute, and what that buys is not speed but *ambition* — a
runtime pass has to be a scanline trick, and an offline one can do
connected components, fit and cross-check, and print an outlier list for
a person to look at.

**31.1 A tool, not a pass.** The same shape `docs/roadmap.md` already
settled for the Sphere scriptpack: a build tool, not an engine feature. It
runs against an install and writes one table for every graphic it could
read.

**31.2 One table, and a hand-authored entry wins.** Decision 30.1's
"derive first, author later" becomes one artifact rather than two code
paths: an override is a row in the same table, and the tool leaves it
alone. So a shard fixing one wall edits a file rather than patching a
detector.

**31.3 The generated table is not checked in.** It is derived from
copyrighted art, and this repository ships no client files, ever. What is
checked in is the **tool** and the **overrides**; the table is generated
beside the install, into a cache, on the machine that has the files. A
pack that ships its own art may of course check in the table for *its*
art — that is its own content.

**31.4 Staleness is detected, not assumed.** The table records what it
was measured from, and a mismatch re-derives rather than trusting: art
changes between client versions, and a table silently describing a
different install would move every wall's face by a rule nobody could
see. `docs/client_versions.md` is why this is not paranoia.

**31.5 What moves is everything measured from a picture.** Today that is
`facing::facing_of`; tomorrow it is step 16's aperture, and after that
whatever a mesh needs. The runtime keeps a *reader*, and the detector's
own tests keep running against the client exactly as `tests/facing.rs`
does now — the sweep is already the tool, minus the file it writes.

**31.6 The client still works with no table at all.** It derives what it
needs the way it does today and says so in a log line. A missing cache is
a slow first frame, not a shard that will not start — the same
refusal-to-guess this pass takes everywhere else, arriving as a refusal
to *require*.

### Step 15: a wall's facing, measured from its art

- [x] **Step 15. A wall's facing, measured from its art.** Decision 3 is
      right that `tiledata.mul` does not say which edge a wall stands on.
      The *art* does, and what says it is the **base edge** — the lowest
      drawn pixel of each column, which is where the wall meets the
      ground and the one part of a wall's silhouette with no ornament on
      it. Two independent bits come out of it and together they are the
      four faces:

      | face | runs along | occupies | base descends |
      |---|---|---|---|
      | N | `+x` | right half | to the right |
      | E | `+y` | right half | to the left |
      | S | `+x` | left half | to the right |
      | W | `+y` | left half | to the left |

      Verified against the client before a line was written: `0x0100`
      "marble wall" has its mass in columns 18..=43 with the base
      descending left — the east face — and its base lands on the
      predicted `dy = 22 - across` **to the pixel** over the whole
      22-column span. `0x0007` is the south face of the same shape and
      lands the same way. That is the not-circular check the whole step
      rests on.

      Then a pixel maps onto that face instead of onto the tile's middle.
      With `v` along the edge and `(dx, dy)` the offset from the tile's
      centre:

      | face | place | `v` |
      |---|---|---|
      | N | `(v, 0)` | `dx/22` |
      | E | `(1, v)` | `1 - dx/22` |
      | S | `(v, 1)` | `1 + dx/22` |
      | W | `(0, v)` | `-dx/22` |

      And the height is **one line for all six stances**, which is more
      than the plan hoped for: the point of the tile a pixel's picture
      rises from is `(place.x + place.y - 1) * 22` pixels below the
      tile's centre row, so `z = z0 + ((sub.x + sub.y - 1) * 22 - dy) /
      4` covers the four faces, the faceless upright case (where the term
      is zero and this is exactly the old `z0 - dy/4`) and — read with an
      *unclamped* fraction — the flat case too. The formula generalises
      rather than replaces, and `BOTTOM_LIFT` is gone.

      **The point is the seam**, and it is what the frame test asserts:
      the next tile along the run starts its `v` at 0 where this one
      ended at 1, so a row of wall tiles is one continuous surface. Held
      to by two mutations — the run reversed, and the run replaced by a
      constant — each of which fails it.

      Where it went: `render/src/facing.rs` holds `facing_of(&Image) ->
      Option<Facing>` (`face_of` at the time; decision 25 made the
      answer a face *or a corner*) and `silhouette`, the fixture both the
      unit tests and the GPU test are drawn against (`pub` for the reason
      `scene.rs`'s rooms are); `StaticAtlas` calls it once while packing
      and keeps the answer on `Sprite`; `place::Stance` grew from two
      values to six; `statics.wgsl` gained the switch. `light.rs` and
      `blit.wgsl` were not touched — the attachment is their input,
      exactly as planned.

      **What it reads, measured rather than hoped.** 36% of the install's
      3,212 `WALL` graphics, and **76% of the 4,596 wall statics standing
      in Britain** — the second is the number that decides how a frame
      looks, and the two are reported apart because the table is mostly
      things nobody built with. The unread remainder is corners, posts,
      roof slabs flagged `WALL`, and multi-tile buildings shipped as one
      graphic; every one of them keeps today's behaviour exactly.
      `tests/facing.rs` prints both, pins seven named graphics to their
      verdicts, and asserts a floor under each share — a detector with no
      coverage count is a green light for having checked nothing.

      Four things the detector had to be taught by being caught getting
      them wrong, all four by measurement rather than by reasoning:

      - **It only looked at the half it had proposed.** A 106-pixel
        statue read as a north face because its mass, far off to the
        left of any tile edge, was never tested. Every one of the 15
        "north" graphics was that bug.
      - **`SPILL` at six pixels refused most of a city.** A wall is a
        solid and the picture shows its thickness; where it is low
        enough to look down on, its whole top surface is drawn — 8.5
        pixels on `0x0063`, the garden wall Britain is fenced with.
        Twelve reads 76% of the map where six read 40%, and a corner
        still covers the whole other half, 21.5 pixels of it.
      - **A slab is not a wall.** A roof piece has the right 45° base and
        no height above it, so the detector asks that the thing stand up.
      - **A 45° line has the same slope wherever it sits.** Everything
        above measures the base line's *direction*; nothing pinned down
        its *position*, and nothing had to — `statics::stand_on` puts a
        sprite's bottom row on the diamond's bottom vertex, so the edge's
        screen position is fully determined by the face, with no freedom
        at all. `0x0171` is what that bought: a flat diamond drawn eighty
        pixels above its own tile, an awning, whose lower-right side is a
        clean run in the empty right half. It passed every other gate and
        was shaded as a vertical face. The gate is that a base pixel
        lands within three pixels of the edge it claims — and the client
        agrees to the pixel, median zero over 908 graphics. It removed 45
        graphics from the table and **not one instance from Britain**,
        which is what a false-positive gate should do.

      **Doors are not a special case, and it shows.** 558 graphics carry
      `DOOR` and are offered like anything else. The ones read sit on a
      tile edge as squarely as a plain wall — median distance zero, none
      over two pixels — so an open leaf, where the art puts it on an
      edge, is shaded along the axis it actually swung to. The wide open
      leaves, 56 to 106 pixels across, stick past their own tile and die
      on `OVERHANG`. Decision 11 said an open door is a static that
      stopped being an occluder and nothing here knows what a door is;
      this arrives at the same place from the shading side.

      **And the art only ever draws two of the four.** Every wall the
      install ships stands on its tile's `y1` or `x1` edge — the two an
      isometric camera can see the face of; a `y0` or `x0` face is a
      surface turned away from the viewer and there is no picture of one.
      North and west are five graphics and one, out of 1,197. The enum
      keeps all four because the *geometry* has four edges and a detector
      that could not name one could not be caught naming it wrongly.

      **Not for the occlusion grid**, and that has not changed: there a
      wrong guess is a room leaking onto the street, which is what
      decision 3 refuses; here it is shading that looks odd. 76% is the
      number that conversation now starts from — and it is the same key
      that unlocks the sconce lighting through its own wall and the sun
      lighting both faces of one.

### Step 16: the window's aperture, measured off the art

- [x] **Step 16. The window's aperture, measured off the art.** A pane
      passed four fifths of the light *across the whole tile*, which is a
      dimmer tile and not a beam. The hole is in the art — a window
      graphic's silhouette has a transparent gap inside an opaque wall —
      and `facing::aperture_of` reads it: a span of `v` along the face
      and a span of `z`, in the surface's own coordinates.

      **58 pictures out of 39,189, and 56 of them carry the client's own
      `WINDOW` flag** — which is the cross-check worth having, because
      nothing in the detector looks at a flag: the agreement is between a
      silhouette and a table neither half was reading. The other two are
      `0x21FF` and `0x2200`, a ruined wall with a hole knocked in it, and
      they are right too. Weighted by what stands in a city: **85 wall
      statics in Britain** have a window, out of four graphics —
      `0x003C` and `0x003B`, the arched windows of a plaster house, and
      `0x00B9`/`0x00BA`, the same in stone.

      **What is measured is a rectangle in the surface, and the art draws
      an arch.** `0x003C` is a doorway with a flat sill, straight sides
      and a rounded top — two pixels taller in the middle than at its
      ends. So the answer is the **largest rectangle inscribed** in what
      the art left transparent, searched over every sub-run of the
      columns (`O(n²)` over at most 22 of them, which is the sort of
      arithmetic decision 31's budget was bought for). A bounding box
      would let light through stone the artist drew.

      **A measurement is relative and a placement is absolute**, and that
      is the one structural thing this step added: `facing::Hole` is `z`
      above the *static's own base*, because one picture stands on a
      hundred tiles at a hundred heights, and `Aperture::above` is the
      single conversion, called in `Builder::add` where the instance's
      `z` is. `Shape` carries the measurement — one row, one lookup, both
      verdicts — and `Shape::of` is the one function the tool and the
      table-less client both measure through, so the two cannot drift
      into a window that exists only where somebody ran a tool.

      The gates, each of which a real client picture fails: a hole must
      have wall either side of it along the run (`HOLE_MARGIN`), a
      column with two gaps in it is a lattice and not a rectangle, gap
      columns that are not one run of them are two windows, a corner is
      refused outright (nothing in a silhouette says which of its two
      faces a hole is in), and anything under three columns by two `z` is
      a scratch in the art. The refusals are counted, not guessed at: of
      the 244 `WINDOW`-flagged pictures the detector reads a face on, 81
      have no hole drawn at all (the glass is painted opaque — `0x00CB`
      is one), 46 are lattices, and 61 fail a gate.

      Held by nine tests in `facing.rs` — the round trip on all four
      faces, a solid wall with none, the corner's refusal, the margin,
      the scratch, two gaps in a column, two holes along the run, and
      **both directions of the inscribed rectangle** (an arch that keeps
      its width and a chimney that keeps its height) — plus the format's
      round trip, the atlas seam with and without a table, and the
      install sweep, which pins `0x003C`'s four numbers by hand.

### Step 20b: the measuring tool, and the table it writes

- [x] **Step 20b. The measuring tool, and the table it writes.** Decision
      31: the silhouette work leaves the frame. `tests/facing.rs`'s sweep
      was most of it already — it walks an install, reads every `WALL`
      graphic and prints the shares — so what this added is the file,
      the loader, the staleness key and the override merge. Doing it
      *before* step 16 is what lets step 16's measurement be as
      expensive as it needs to be, and it closes the backlog entry about
      the atlas walking the same pixels twice.

      **Where it went**, and the split is the one this workspace already
      has: `client/render` never opens a file (its `Cargo.toml` says so
      and means it), so `render/src/arttable.rs` holds the *type* and its
      text and nothing else, and the new crate `crates/client/artscan`
      holds everything with a path in it — the sweep, the file, and the
      reader `client/app` loads through. The reader lives with the tool
      rather than in the app on purpose: they are the two ends of one
      file, and a client that looked for it somewhere the tool does not
      write is a bug that reads as "the table does nothing".

      **The format is one file and hand-editable**, which is decision
      31.2 rather than a taste: a row is `0x0104 corner E S`, a comment
      is `#`, and an override is the same row with `authored` on the end
      — the tool re-derives everything else and leaves those alone.
      `data/overrides.table` is what this repository ships (decision
      31.3: the tool and the overrides are checked in, the generated
      table never is, because it is derived from copyrighted art), and
      it held **no rows at the time this step landed** — the mechanism
      is held by tests, because a row invented to exercise it would be a
      wrong answer shipped to every shard.

      **An absent row means measured and refused**, and the header's
      `examined` count is what makes that legible: a table that had
      swept the `WALL` graphics alone would be claiming a verdict about
      fifty thousand pictures it never opened. So the sweep offers
      *every* graphic the install ships, which is also exactly what the
      atlas does — the table has to answer the questions the atlas asks,
      not the ones a wall would.

      **Staleness is detected** (31.4): the stamp is the art container's
      name and byte length plus `facing::DETECTOR`, a version to bump
      when a gate in `facing.rs` changes. Two independent halves because
      they fail independently — a different install, and the same
      install read by different rules, the second of which nothing else
      in the file could ever say.

      Measured on a 2D install, `cargo run` with no `--release`:

      ```
      pictures with art: 39189
      read:              6150  (15.7%)
      corners:           4362
        East         824
        East+South   4359
        North        46
        North+West   3
        South        872
        West         46
      ```

      **Four seconds**, which is the number step 16 gets to spend
      against: the budget decision 31 bought is a minute and this is the
      first thing in it. The corner count is the one to look at twice —
      see the backlog below.

      Held by nine tests. Five are the format (a round trip that keeps
      every verdict, a derived refusal that is an absent row, a
      re-derivation that leaves an authored row alone *in both
      directions*, a sheet of overrides handing its rows to a measured
      table, and a stamp that is stale if either half differs); two are
      the seam (`a_packed_sprite_takes_its_surface_from_the_table` and
      the same with no table, which is the fallback decision 31.6
      promises); and two need a real install — every graphic's row
      against a live `facing_of`, and a stale table refused through the
      real reader over the real art file.

### The `tests/author.rs` instrument (step 23 sub-step 4, decision 41's companion)

**The instrument, which is what makes "by hand" a real mode.** Authoring
six numbers per graphic is only tractable with a loop: draw the candidate
solid's silhouette over the real sprite, score the intersection over
union, show where they disagree, edit the row, look again. Half of it
existed before — `tests/artshot.rs` writes a graphic with the tile's
diamond stroked over it, `tests/prism.rs` scores a fit — and what was
missing was the two of them in one run that takes a graphic and a table
and says: here is what you wrote, here is what the artist drew, here is
the difference.

**A second candidate shape joined the first, decision 41 (see "The
occluding world" archive), before this step was taken**: `facing::blocks_silhouette`
draws a `Blocks` list the way `prism_silhouette` draws a `Prism`, so the
instrument has both forward directions to render a candidate with — the
search is automatic for a prism (`best_prism`) and by hand for a block
list, since nothing proposes an arch's boxes the way `best_prism` proposes
a climb.

**Built: `tests/author.rs`.** A graphic and a table in (`OPENSHARD_TABLE`,
the checked-in `overrides.table` by default); it reads whichever verdict
the row carries, draws that candidate's silhouette, and scores it against
the real sprite with `facing::silhouettes_agree` — made `pub` for exactly
this, rather than let the instrument grow its own copy of the alignment
rule `best_prism` already trusts. The picture it writes is the art's own
colours where the two agree and a flat colour for each direction of
disagreement — cyan where the art draws and the row does not claim it,
red the other way round, the worse of the two per `silhouettes_agree`'s
own doc. Checked against the landing, `0x071E`, by hand-typing `block 0 8
0 8 0 5` into a scratch table: **0.977**, the same number decision 41's
own table gets fitting a prism to the same picture, because a box is what
a one-tread prism already is. A deliberately undersized block (`0 4 0 4 0
5`) scores **0.378**, and the picture shows why — the whole diamond
outside the small box reads cyan. With nothing authored for a graphic,
this is `tests/artshot.rs`'s own picture, unchanged.

**DoD, and what was still unmet at the time:** the staircase's two
graphics authored through it, and a joint and an arch — the two shapes a
person reported as "something odd happens" — authored and scored. **The
instrument exists; the authoring did not, at the time.** Nothing was
written into any table yet for any of the four, because placing a box
against a picture is a person's judgement, not a search this session
could run in their place. The number to record once somebody does is how
long one graphic takes them, because that is what says whether the mode
is hundreds of graphics or three. *(As of `lighting.md`'s Status: still
nothing authored into `data/overrides.table`, and `Builder::add` still
does not consume `blocks` at all — see "The occluding world" archive's
"found on a staircase in Britain" for the scratch-table experiments that
have been run against this instrument since.)*

### Backlog: found while measuring a wall's facing out of its art

- **The detector's own coverage is a moving target and only the sweep
  knows it.** 37% of the graphic table, 76% of what Britain is built
  from. Both are printed and both have a floor asserted under them, but
  the floors are *measurements* and not targets — the thing they catch is
  a gate tightened until the feature stops applying, which is what the
  six-pixel `SPILL` did before it was measured.
- **The remaining quarter of Britain's walls has a shape** — and decision
  25 took two thirds of it. The most-built unread graphics were
  `0x00DE`/`0x00DD` (roof slabs carrying `WALL`), `0x0081`/`0x0082`
  (pillars filling a whole tile) and `0x00C8`/`0x00C9`; the pillars read
  as corners now, and what was left is **8.1% of the statics standing in
  Britain**, headed by `0x02D8`, `0x02D3`, `0x02D6` and `0x02D0`. The
  entry's own prediction was right about the shape of the answer: not a
  looser gate but a second *kind* of it. Whatever is left will want a
  third, and it is worth printing the new worst list before guessing at
  one — `tests/facing.rs` does.
- ~~**A corner could be answered rather than refused.**~~ Done, decision
  25 (see "The G-buffer bridge" archive), and the estimate in it was
  exact: four more stances and the rule that a pixel belongs to the face
  on its own half of the picture, which the fragment shader had in
  `across` already.
- **Nothing measures how far a decided face is from the edge except a
  gate.** The check that caught `0x0171` is a pass/fail inside
  `facing_of`; the *median* and the outlier list that made it obvious
  were a throwaway script. A graphic drifting from zero to two pixels
  across a client version is invisible until it crosses three and
  vanishes. The sweep prints two shares and could print this distribution
  for a few lines more.
- **The sweep reads the whole art file to answer a question about 3,212
  graphics.** It takes a couple of seconds, which is fine for an
  `#[ignore]`d test and would not be if it ever moved into CI.
- ~~**`facing_of` is a second walk of pixels the atlas has just
  copied.**~~ Closed by step 20b: with a table beside the install the
  atlas does a lookup, and the walk happens once in a tool that is
  allowed to take four seconds. The entry's own reading of the cost was
  right and incomplete — it is measurable on a scroll that introduces
  four hundred graphics, and the reason it had to go was not the cost but
  the *ceiling*: a measurement that has to fit in a frame can never be a
  search.
- **A wall's *top* surface is shaded as if it were the face.** The pixels
  past the tile's centre column — the thickness `SPILL` allows — clamp to
  the near end of the edge, so the top of a low garden wall is lit as
  though it were the vertical face at that point. Better than one flat
  tile and not right; the top is a horizontal surface and would want the
  flat mapping, which the silhouette can separate (it is the part above
  the base line's own 45°) but nothing does.
- **The frame dump can now be pointed at a debug view.**
  `OPENSHARD_FRAME_VIEW` in `tests/cost.rs`, and it is what made this
  step's measurement possible: a brightness profile across a *drawn*
  wall measures the timbers and the windows, not the lighting.
  `View::Light` throws the art away. Anything about the shape of a pool
  should be judged there.

### Backlog: found while moving the measurement out of the frame

- **The detector is offered 39,189 pictures and calls 4,362 of them
  corners**, where the `WALL` graphics alone hold 297. That is not new
  and not the table's doing — `StaticAtlas::insert` has always asked
  `facing_of` about every graphic it packs, `WALL` or not, and
  `place::Stance::of` reads the client's `FLOOR` bit and then takes
  whatever the art said. What is new is that somebody finally *counted*
  it. A solid filling its own tile reads as a corner because it is one
  shape (see the pillar entry, "The G-buffer bridge" backlog), so a
  crate, a rock and a tree stump are shaded as two vertical faces with a
  normal each. It costs nothing in the occlusion grid — a crate is not
  `NO_SHOOT`, so it is not a cell — and it is pure shading, which is why
  nobody has reported it. Whether it is *wrong* is a question for a
  picture: a barrel lit only on the two sides a camera can see is
  arguably better than one lit flat. The measurement to make is the same
  one `tests/facing.rs` makes for walls — the share of what actually
  stands in a city — for the graphics no `WALL` flag vouches for.
- **A tool that measures pictures has to build a renderer.** `artscan`
  depends on `client/render` for `facing` and `arttable`, and
  `client/render` depends on `wgpu`, so a build of the tool is a build of
  the graphics stack. Nothing about a silhouette needs a GPU. The shape
  of the fix is a crate under both of them — the measurement is a pure
  function of an `Image` and the table is text — and it is not worth
  doing for one tool; it is worth doing the moment a second reader
  appears, which is the same rule `doors.rs` states about its own table.
- **The staleness key cannot see a patch that keeps the file's length.**
  The stamp is `artLegacyMUL.uop`'s name and byte count, which tells two
  *installs* apart and would not notice an art patch that replaced a
  sprite in place. The honest alternatives both cost: a hash of a 150MB
  file every start, or a modification time, which changes on a copy and
  would re-derive for nothing. The install-gated test — every row against
  a live measurement — is what catches it on the machine that has the
  files, and it is the only thing that does.
- **`DETECTOR` is a number somebody has to remember to bump.** Nothing
  enforces it and nothing can: it is a claim about a diff. What makes it
  survivable is the same install-gated test, which compares the table
  against today's rules rather than against the version it says it was
  written by. Worth remembering when step 16 changes `facing.rs`.
- **The table's row grammar has one verdict and step 16 brings a
  second.** A row is a graphic and a facing today. An aperture is a
  rectangle in a surface's own coordinates, which is four more numbers
  and a question about which surface of a corner they belong to — and
  the version gate (`FORMAT`, refused rather than half-read) is what
  keeps a client from answering confidently out of a table written
  before the field existed.
- **Nothing runs the tool for the player.** A first run with no table is
  a client that measures as it packs, which is what it always did, and
  the log line says so — but somebody has to notice the line and run a
  command. The obvious next step is for the client to *write* the table
  when it finds none, which is a four-second stall at startup on one run
  in the lifetime of an install, and it is deliberately not done yet: it
  would put file writing into `client/app`'s startup path on the strength
  of one measurement of one install's size.

### Backlog: found while measuring a window off its own art

- **The leaded window is refused, and it is the biggest thing left here.**
  46 of the install's pictures draw a lattice — mullions across the
  glass, so a column of the sprite has two, three or four transparent
  runs in it rather than one — and the detector refuses the whole
  picture rather than pick one of them or merge them. Four of those 46
  stand in Britain and one of them, `0x000E`, is on twenty walls in the
  tiles the sweep reads. It is the conservative direction (a refused
  window is a solid wall, which is what every wall was until this step)
  but it is the wrong answer for the most ordinary window in the game.
  The shape of a fix, and why it was not done here: a lattice is *mostly*
  hole, so the honest measurement is the largest rectangle over a region
  defined by how much of it is transparent rather than by a single run
  per column — which needs a threshold nobody has measured yet, and a
  threshold invented on the way past is how a detector starts reading
  light through stone. *(Still refused — see `lighting.md`'s Status.)*
- **A second gap anywhere in the picture refuses the first.** The gate is
  per *picture*, not per region: `0x24F6` is a porthole with a small
  second shape beside it, and both are lost. Refusing the picture is
  right when the two are windows; when one is a scratch it throws away a
  real one. What it wants is the same thing the entry above wants — a
  notion of *which region* is the hole — and the two are one piece of
  work.
- **A corner may not have a window.** `aperture_of` refuses a corner
  outright, because a hole would go to both of its panels and there is
  nothing in a silhouette that says which face it was cut into. No
  corner graphic in the install has a hole to lose, so this costs
  nothing today; what would change it is measuring the hole's *columns*
  against the halves, which is the same information the corner's two
  faces are already read from.
- **81 `WINDOW`-flagged pictures have no hole drawn at all.** `0x00CB` is
  one: a solid wall with the glass painted opaque. That is the flag and
  the art disagreeing, and the art wins here on purpose — decision 3's
  refusal, arriving for a second kind of measurement. What it means in a
  frame is that a flagged window with painted glass keeps
  `occlusion::PANE`'s fifth stopped across the whole tile, which is
  exactly the behaviour this step was supposed to replace. It is not a
  defect; it is where the art stops saying anything.
- **The measured `z` is quantised to whole units.** A hole's edge lands
  on a pixel and one unit of `z` is four of them, so a sill measured at
  41 pixels becomes ten and a quarter and is written down as ten. The
  rounding is to nearest and the rectangle it rounds is already the
  inscribed one, so the error is under half a `z` in each direction and
  always at the *edge* of a penumbra the walk softens anyway. Worth
  knowing before anybody reads `Hole`'s numbers as exact.
- **A hole's `near` and `far` are the run of the whole tile, and a window
  is drawn on 22 pixels of it.** So the quantisation the other way is a
  255th of a tile, which is finer than the art can say — `RUN_STEPS` was
  chosen for the walk's own agreement between shader and Rust, and it is
  comfortably finer than this measurement needs. Nothing to do; the
  asymmetry between the two axes is worth not being surprised by.

### Backlog: found while chasing a client that took half a minute to open a window

- **The prism search redrew its candidates once per picture, and that
  cost was paid on the render thread.** `best_prism` scored a graphic
  against 261 candidates and *drew* each one as it went — 129×129 samples
  a silhouette — so every corner the face detector found paid a quarter
  of a million tile samples. The candidate set does not depend on the
  picture, which is the whole of the defect: `artscan` went from **more
  than ten minutes** (it never finished) to **eleven seconds**, and the
  table-less client, which measures as the atlas packs, went from **27
  seconds of black screen before the first frame** to no measurable
  stall. The candidates are drawn once (`facing::candidates`) and a
  candidate whose drawn-pixel count cannot beat the best score already
  found is never walked — an exact bound, `min / max` over the two
  counts, so the answers are identical: `tests/prism.rs` still scores the
  same stairs at 0.977 and 0.975 and the same wall at 0.812.
  Two things worth carrying. **The cost was invisible because it was in
  the fallback**: decision 31.6 says a missing table is a log line and a
  slow first frame, and "slow" silently became "the window does not
  appear" when step 22 added a search to `Shape::of`. A fallback nobody
  times is a fallback that can cost anything. And **the same shape is
  waiting for the thickness search** ("found while re-cutting the plan
  around decision 38", above) proposes — scoring a box of thickness `t`
  per picture is another candidate set that does not depend on the
  picture, and it should be built the same way rather than measured,
  found slow, and fixed again.
- ~~**A table makes the client read stairs as corners, and nothing says
  so.**~~ **Fixed by the same change, and it is the half of it that
  mattered.** The entry recorded that `ArtTable` carried no prism; what
  was not written down is that this was a *behavioural* difference a
  person could turn on by running a tool — run `artscan`, and the
  graphics the atlas would have measured a prism for came back from the
  table with `prism: None`, so the staircase quietly went back to
  occluding like a run of wall while the log line said only how many
  pictures were read. The two honest states named there were "no table"
  and "a table with prisms in it"; the format bump is what removes the
  one in between, since a table written before the third verdict is now
  refused by version rather than read as a set of silent `None`s.
  What is worth carrying out of it: **the measurement was being paid
  twice or not at all, and never once.** A machine with no table paid the
  whole prism search while packing the atlas — the 27 seconds of black
  screen the entry above is about — and a machine with a table paid
  nothing and got the wrong answer. That is the shape decision 31 exists
  to prevent, and it came back the moment a *new* verdict was measured
  without a place in the file to put it. The next detector to land wants
  its row in the grammar in the same commit, not the one after.
  Left open: `tests/install.rs` has floors for faces, corners and windows
  and none for solids, because a floor is a number measured off a real
  install and nobody has run the sweep since format 3. It prints
  `solids:`; the floor goes in the day that print has a number in it.

## Solids as drawable geometry

**Decision 39. The scene is already three-dimensional. What is missing is
a primitive.**

This was written down because the wrong mental model cost an hour of
argument in the session that produced it: asked whether a wall could be
drawn as a solid, the answer given was "that is a mesh pass, a depth
buffer that agrees with a painter's sort, multi-session work" — and every
clause of it was false. The renderer is not a sprite blitter that would
have to *acquire* a third dimension. It is a three-dimensional scene whose
primitives happen to be billboards.

What is already there, item by item, because each one is a thing that
would otherwise be built:

- **World space is the space the lighting reasons in.** Decision 1 moved
  it there and the whole file since is about the consequences. A fragment
  is lit by the tile and height of the thing drawn at it.
- **A per-pixel world position** is written by all three world passes
  (step 2). That is a G-buffer position plane; it was never called one.
- **`Camera::project` is a view-projection matrix** written as integer
  arithmetic. There is no trigonometry in the file, because there is no
  rotation: an orthographic camera at one fixed angle.
- **The depth buffer is hardware**, written and tested by both passes, and
  `crates/client/render/src/depth.rs` is the ordering ported from
  ClassicUO. Draw order between passes was deliberately made not to
  matter.

**39.1 The projection is exact, and the world is anisotropic.** A tile is
44 pixels wide and 22 high on the screen, and one `z` is `Z_STEP` = 4. So
a unit of height is about 0.18 of a unit of ground, and geometry placed in
world coordinates lands in the same pixels the sprite for that tile lands
in — **to the pixel, with nothing fitted**, because the sprite is placed
by the same map.

The trap is on the way in: a solid written with equal axes, a "real
cube", comes out five and a half times the wrong height. The non-uniform
scale is part of the projection and is carried, not corrected.

**39.2 The depth is the client's ordering, not the distance to a camera —
and that is the one place a solid does not simply fit.** `depth::Order`
is `(x + y, priority_z)`, discrete and **per instance**; `statics.wgsl`
says in as many words that deriving it in the shader would be a second
chance to disagree with `depth::Order`. For a sprite that is right. For a
**box spanning several tiles** it is not: one instance depth, several tile
depths underneath it.

Two honest answers, and the first is enough for a long time:

- **Translucent, over the frame, writing no depth.** For looking at
  geometry this is not a compromise but the thing wanted: the wall's
  sprite is visible *inside* the box that claims to contain it, and the
  top face is what makes its thickness legible.
- **Per-fragment depth through the same `Order`.** The fragment knows its
  own world point, so the key is computed from the point rather than from
  a new formula — the rule this file uses everywhere: cite the function
  it came from. This is what a solid occluded by the sprites in front of
  it needs.

**39.3 Three faces, always the same three.** With no rotation, an
axis-aligned box shows exactly `+x`, `+y` and the top; its outline is a
hexagon. So a solids pass is not a mesh pipeline — no index buffers, no
back-face culling, no asset: it is an instanced quad pass of the same
shape as `statics`, six numbers and a colour per instance, the corners
emitted in the vertex shader. Three constant normals shade it for free,
which is what makes the top face read as thickness. And instancing
through vertex buffers is how statics already draw, so decision 30.5's
floor is untouched.

**39.4 Drawing a slope is nearly free, and it is not decision 35.** Under
an affine projection an inclined face is still a parallelogram, and a
stair's prism is a few of them — one more parameter in the same shader.
Decision 35 priced something else entirely: a *sloped surface in the ray
walk*, which is a bilinear patch and reopens the three seam rules. The two
must not be confused, and the good news in the distinction is the order
it allows: a shape can be **looked at** long before the walk can
integrate it, which is the right way round.

**39.5 The billboards stay billboards, and that is the design.** The art
is drawn for this projection and for no other, and the depth must stay
the client's ordering or the picture stops matching the client the
engine exists to serve. So this is not a renderer on its way to being
general — it is a three-dimensional scene with a fixed camera, sprite
primitives, and now a solid one beside them.

**39.6 The pass is not what makes it a solid; the projection is. But a
picture nothing can capture is not an instrument.** Both halves of this
were learned in one sitting, in that order.

The first: `Camera::project` already had a float core waiting to be
named, so `project_exact` takes a place between the tiles, `project` is
it at a whole one, and one test pins the two together over the whole
map. Twelve points through that, three polygons out. The geometry is the
durable half — `render/src/solid.rs` knows nothing about who paints it —
and the first version of 23.0 painted it through the *egui painter*,
beside the wireframe, and looked right.

The second is why that version did not survive the day. **`render` takes
its pictures headless**: `tests/cost.rs` builds Britain at the widest
zoom on a real adapter, times the passes and writes the frame out;
`tests/pictures.rs` does the same for the plan views. A diagnostic drawn
by the client's UI toolkit appears in none of them, and cannot be timed
beside the passes it runs with — so the one number 23.0's DoD asked for
was the one number that arrangement could not produce. A view whose whole
job is to be looked at has to be capturable by the thing that takes the
pictures.

So the solids are `render/src/solids.rs`: two pipelines over one shader —
a triangle list for the faces, a line list for the silhouette — drawn
after the blit, on the surface, translucent, writing no depth. And the
split above is what made that a small change rather than a rewrite:
**the projection stayed in Rust.** The corners arrive already in
viewport pixels from `Solid::faces`, and `solids.wgsl` does the one thing
no CPU can do for it — a pixel into clip space, and the blend. A vertex
shader deriving a box from its two corners would be a second
implementation of the arithmetic every sprite in the frame is placed by,
which is exactly what `statics.wgsl` refuses to do about depth and for
the same reason. The cost of keeping it in Rust is one buffer write a
frame, and it is measured rather than argued.

What the pass still cannot do, stated so it is not rediscovered: a solid
is not **occluded by the sprites in front of it**, because it draws over
the finished picture. That is 39.2's first answer and it is the wanted
one for an instrument; the second answer — per-fragment depth through
`depth::Order` — is what a solid that has stopped being a diagnostic will
need.

**39.7 The lattice is the corners, and the tiles are the centres.** The
one trap the projection had left in it. `project` takes a `Point` and
returns where that tile's diamond is *centred*, so a solid whose extent
is stated in the same numbers — "tile `(x, y)`, from `x` to `x+1`" — is
half a tile off, in both ground axes at once, which on screen is one
clean step down. `WorldSpot` is therefore the corner lattice and
`WorldSpot::centre` is the only place the half lives. Written down
because it is invisible in a wireframe of single tiles and obvious the
moment a box has to contain a sprite.

**39.8 A view of the grid has a second datum, and "this storey" is not
one of its values.** Both views drew what stands above the player's
feet, which is the right answer for a picture of *what shadows you* and
the wrong one for a picture whose subject is geometry: standing in a
room at `z = 0`, the room's own floor and every lid under it are simply
not drawn — and **a hole in a floor and a floor below the cut are the
same picture.** Counting what was hidden does not close that, and the
distinction is worth keeping: a count says *how much* is missing, never
*where*. An instrument that can be wrong in a way indistinguishable from
the defect it is pointed at is the one failure a diagnostic may not
have.

So `solid::Cut` is a switch, with the two values that can be *stated*:
`BelowFeet(z)` — what could shadow somebody standing here, and why a pier
is not 2,011 boxes — and `Nothing`, the whole grid, unreadable in a town
by design. F4 in the client, a pair beside the two checkboxes, and it
governs **both** views, because they are read against each other and two
grids cut differently cannot be compared.

*This storey* is the value a person would reach for most and it is
deliberately absent: it needs a ceiling, and therefore a rule for which
of the four lids over your head is *yours*. Inventing that rule to fill
out an enum would put a third answer into the instrument that no test
could hold. It arrives when a room is a thing the world can name.

The cut is resolved once per frame (`App::solid_cut`) and never stored:
what a person picks is one of two questions and holds across frames,
while the `z` in `BelowFeet` is a fact about the frame it is drawn in.
One join, so no stale height can be kept anywhere and the two views
cannot drift apart.

### Step 23.0: the solid, drawn

**[x] The solid, drawn — in the world, not against a sprite.** Decision
39: a pass that draws a box as a box, in the frame, where it stands.
Translucent and over the world, so the static's own sprite is visible
*inside* the solid that claims to contain it and the top face makes its
thickness legible.

**This comes before the migration and not after it, for the reason this
file gave itself at decision 24: the instrument comes before the
reproduction.** 23.1's whole DoD is "the picture did not move", and what
there was to judge that with at the time was twelve strokes per cell
through the egui painter (`shell::draw_occluders`) — a wireframe that
cannot show a face, a normal, or a solid standing inside another. A
migration judged by an instrument that cannot see the thing being
migrated is a migration whose defects arrive later, attributed to
something else.

It is also the answer to a question no measurement against a sprite can
reach. `tests/prism.rs` scores a shape against the picture it was drawn
from, which is the right check for *is this the shape the artist drew*;
it says nothing about **how the shapes work together** — a wall meeting
a wall, a stair meeting a landing, an arch over a street. That is a fact
about a place and it can only be looked at in one.

Built against **the surfaces of the time** (`Occlusion::boxes` already
yielded what stands), so it needed nothing from 23.1 and survived it
unchanged: after the migration the same six numbers arrive from a solid.
The translucent-over-the-frame choice is decision 39.2's first answer,
and per-fragment depth is left until there is a reason.

**DoD:** the toggle beside the wireframe rather than replacing it — a
wireframe shows what a solid hides; the staircase at `(1493, 1639)` and
the house corner at `(1441, 1692)` looked at and *reported on*, which is
a person's step and not a test's; and a cost reading with the view on at
the widest zoom, because a translucent overlay over a town is overdraw
and the number decides whether it stays a debug view or gets a bound.

**What landed.** Decision 39.6 has the two findings behind the shape of
it: the geometry is the durable half and stayed in Rust, and the pass is
a real one because `render`'s pictures are taken headless and an overlay
drawn by the client's UI toolkit is in none of them.

Built:
- `camera::WorldSpot` and `project_exact` — a place between the tiles, on
  the corner lattice (39.7), with `project` delegating to it and a test
  pinning the two over the whole map;
- `solid::Solid`, `Solid::faces` and `Solid::outline` — the three faces
  in the order `Camera::tile_facet` uses, with a test that a unit
  solid's top *is* its tile's diamond, and the nine lines of the
  silhouette and the star inside it;
- `occlusion::Surface::solid` — the drawing-only nominal thickness
  (`PANEL_THICKNESS` a fifth of a tile, `LID_THICKNESS` two `z`), which
  step 23.1 had to re-decide rather than inherit — and `Surface::stands`,
  which the wireframe used to keep to itself;
- `solid::standing` and `solid::kind_colour` — the one list and the one
  palette both views draw, so that "what is on screen" has one answer;
- `render/src/solids.rs` and `solids.wgsl` — the pass, over the lit
  frame, translucent, no depth. In the app it is fed the frame's *own*
  grid (`Lighting::occlusion`, the list the shader is walking) rather
  than a second walk of the map;
- the toggle: F5, the checkbox beside the wireframe's, and the pass's
  own count of what it drew against what it was handed;
- `--at X,Y` and `--solids` on the offline viewer, and
  `client_app::Opening` behind them: this plan names places, and until
  now the only way to reach one was to walk there with a shard running;
- `tests/cost.rs` draws it over Britain at the widest zoom and times it —
  `OPENSHARD_FRAME_SOLIDS=1` beside `OPENSHARD_FRAME_DUMP`.

**What was seen**, at 1:1 over Britain, in a debug build:
- **The staircase at `(1493, 1639)` is a stepped mass of whole-tile
  violet bodies** — nine of them, each a full tile of solid from the
  ground to its own step's height, so the shape on screen is a ziggurat
  and not a stair. This is step 23.5's headline defect, and it is now a
  picture rather than an argument.
- **A wall's thickness reads.** A run of panels is a ribbon with a
  visible top face, and where two runs meet the joint is legible — which
  is the thing twelve strokes could not show.
- **Ends and corners of wall runs are whole-tile bodies** (violet posts
  standing in red ribbons), not panels. Worth knowing before 23.5 argues
  about corners: some of them are already solid by accident.
- **The tile the staircase descends through carries no solid at all**,
  and the art there is the black opening the client draws for a hole in
  a floor. Correct — nothing stands on it — and worth having seen: it is
  a tile a ray passes through vertically with nothing to stop it, which
  is what a cellar looks like from above.
- **The house's windows and doors are panes** (cyan) standing in the same
  plane as the wall's panels, and they read as glass in a run of brick.
  Nothing here is a defect; it is the first picture of decision 3's
  opacity actually being about a *place*.

**The cost, at the widest zoom, from `tests/cost.rs`: 3.61 ms a frame**,
drawing 3,768 of the grid's 16,729 boxes — the rest are off the edge of
the picture and dropped before a vertex is written. Beside it on the
same frame: the whole lighting pass is 0.34 ms and a plain blit is 0.18.

So the number decides what the DoD said it would, and the answer is **it
stays a debug view**: ten times the pass it is a picture of is fine for
something switched on to answer a question and not fine for anything
else. What it is *not* is the shader — the fill is a translucent quad
over a fifth of the screen — and the honest reading is that most of it is
the frame's own vertex buffer being rebuilt and uploaded, 3.3 MB of it,
because the geometry is on the CPU by choice (39.6). If this ever has to
be cheap, the fix is named by that sentence rather than hunted for: keep
the buffer between frames and rebuild it when the camera moves.

### Backlog: found while making the instrument honest

- **Nothing renders a picture of either view and asserts anything about
  it.** The tests at the time held the geometry a view is built from —
  the plane a panel is drawn on, and that the two cuts are a subset and
  its superset — and `tests/cost.rs` draws Britain with the pass on and
  times it. Between those two there was no test that the pass put a box
  on the screen at all, and `Cut::Nothing` in particular had never been
  drawn by anything but a person. The shape of the answer is the one
  `tests/pictures.rs` already has for the lighting: a small built scene,
  one frame, and a claim about a pixel that a wrong cut or a dropped face
  would move.

### Backlog: found while drawing the solid (step 23.0)

- ~~**Both views filter by `Surface::stands`, so a house's floor is
  invisible from its own floor.**~~ Closed by decision 39.8: `solid::Cut`
  is the second datum, F4 flips it, and it governs both views at once.
  Two values and not three — "this storey" needs a ceiling and therefore
  a rule for which lid is yours, and that rule is not inventable here.
- **The solids pass rebuilds its whole vertex buffer every frame.** 3.3
  MB at the widest zoom, and the 3.61 ms reading above is mostly that
  rather than the fragments. The geometry is on the CPU deliberately
  (39.6) and the fix, if one is ever wanted, is the same one the
  occlusion grid already took in step 21.5: keep the buffer and rebuild
  it when the camera moves. Not worth doing for a view that is off by
  default; worth knowing before anybody concludes the translucent fill is
  expensive.
- ~~**Nothing tests that the solids view and the walk agree.**~~ Closed:
  `a_panel_is_drawn_on_the_plane_its_face_pixels_lie_on` derives the
  plane from `Face::place_at` — what `statics.wgsl` places a face
  fragment with — and asserts the box has a face on it, lies *inside*
  its tile rather than straddling the edge, spans the whole run, and
  carries the span the walk tests. Its companion does the lid (top face
  on its plane, hanging under it) and the body (its whole tile). What is
  still untested is a *thickness*, and that is correct: it is a drawing
  number until step 23.1 makes it geometry.
- ~~**The nominal thicknesses are drawing numbers with no owner.**~~
  Closed by renaming them `DRAWN_PANEL_THICKNESS` and
  `DRAWN_LID_THICKNESS`, with the fence stated in the doc comment: no ray
  is tested against either, the only reader is `Surface::solid`, and
  23.1's thickness is a different number reached by a different
  argument. The collision the entry warned about — two constants with
  one name, one drawn and one tested — is now impossible to make
  silently.
- **`Camera::project` is a matrix, and writing it as one would change no
  pixel.** It is an orthographic view-projection with a fixed rotation,
  spelled as integer arithmetic. `docs/camera.md` is the plan that wants
  this seam — "one pipeline every camera is a parameter set of" — and
  decision 39 is the same fact arriving from the other side. Nothing here
  needs it; it is written down because the two plans should not discover
  it separately.
- **The renderer's own doc line about WebGL2 read as a principle and was
  a dated assumption.** `crates/client/render/src/lib.rs:17` said the
  ceiling was WebGL2 "because the web is a target," written when WebGPU
  was behind a flag. The ceiling was re-examined and kept, at that point
  — decision 30.5 has the measurement — but the sentence was worth
  saying what it is: a floor chosen for a target, with a date on the
  reasoning. The question underneath it was a product question and not a
  graphics one: *is the web still a target?* Answered by decision 30.5's
  own "Answered" note (see "The occluding world" archive): yes, and the
  ceiling moved to WebGPU.
- **Decision 22's one-sidedness and `place::Stance`'s nine values are
  both taxonomies that a solid derives.** Once a pixel's face comes from
  a slab test (38.3), a stance is an answer rather than an enum to
  extend, and "a face is one-sided" is a consequence of where the artist
  put pixels. Neither was worth touching before step 23.5, but both
  should be *removed* there rather than carried alongside the thing that
  replaces them — two ways to answer the same question is how a rule and
  its replacement drift apart. **Picked up by [`gbuffer.md`](gbuffer.md)**,
  for the render side, once decision 40 made the cost of carrying both
  concrete rather than theoretical.
- ~~**30.6's distribution does not survive the migration and must be
  re-measured.**~~ Re-measured under step 23.1: **10,212 cells hold
  17,201 solids under 17,201 references**, nothing dropped, 59.8% of
  standing cells holding one. The old 18,071 is not comparable and is
  not to be quoted; the table and what moved it are with the step (see
  "The occluding world" archive). The *per-cell cap* is still the
  format's own 255 and still nowhere near reached — the worst cell in
  Britain references eleven.
- **`Shape::of`'s prism is still lost through the table.** Fixed by step
  23.3: a client with a table used to read a stair as a corner where a
  client without one measured a solid, which is a table making things
  *worse* and the exact failure mode decision 31.6 was written to avoid.

## Testing and instrumentation

**Decision 8. The debug views are branches of the blit, not a second
pipeline.** Everything an observer of this pass could want to see is
already bound to it: which tile a pixel claims and what drew it (the
place attachment), what stands on that tile (the occlusion grid), how
far every flame is and how much of it survived the walk (the loop
itself). A separate visualiser would be a second copy of that unpacking,
kept in step with this one by hand, and it would answer about *its* copy
of the frame rather than about the frame on the screen. So the mode is
one number in the lighting uniform and a `switch` at the end of
`fs_main`, and what it shows is the very values the lit picture was made
of.

*(the parity test's synthetic attachment is built in decision 2's
current shape; [`gbuffer.md`](gbuffer.md) step 3 has to carry it forward
rather than leave it testing a payload the real passes no longer write —
decision 38.5's two-step discipline, geometry held still before it
changes shape, is written for exactly this kind of migration)*

**Decision 9. The reasons are computed on the CPU, and the shader is
checked against them.** "Why is this tile lit" is a list — this flame,
that far, inside its radius, and the ray died on that cell — and a
picture cannot be a list. `light::sample` is that list, in Rust, from
the same `Lighting` the GPU is given. Which makes one formula exist
twice, and the failure mode of that is specific and nasty: the debugger
diverges from the renderer and then lies exactly when it is believed. So
the shader is not the canon and the CPU is not a sketch — a GPU test
uploads a synthetic place attachment, runs the real blit over it and
asserts the two agree per pixel. The parity test is the reason the
second implementation is allowed to exist at all.

**Decision 10. The scenes are built, not loaded.** A room with a torch
in it is a `WorldMap` of flat ground, a `TileData` where two graphics have
flags, and a list of items — every one of which this workspace can
construct from nothing. That is not a concession to the no-client-files
rule, it is better than a real house: the wall is at a stated tile with
a stated height, so a test can say *which* cell should have stopped the
ray, and a failure prints the room rather than a coordinate.
`render/src/scene.rs` holds them, and they are ordinary `pub` items
rather than `#[cfg(test)]` ones because the GPU tests, the playground and
a future benchmark are all outside the crate.

### Steps 7, 8, 9, 10, 19: the CPU oracle, scenes, debug views, parity, plan/elevation

- [x] **Step 7. `light::sample`, the reasons in Rust.** The shader's loop
      and its ray walk, on the CPU, returning per flame: the distance in
      tiles, whether the fragment is inside the radius, what survived
      the walk, and *which cell* stopped it. Unit-tested on its own
      before anything draws with it.
- [x] **Step 8. `render/src/scene.rs`.** The rooms of decision 10 — a
      closed room, a doorway, a window, a sconce on a wall, a cellar
      under a street, and the diagonal gap the backlog names — each a
      `WorldMap`, a `TileData`, an item list and a camera, plus an ASCII
      diagram of a scene's lighting for the message a failing test
      prints.
- [x] **Step 9. The debug views.** `render/src/debug.rs`'s `View`, one
      field in the lighting uniform, a `switch` at the end of
      `blit.wgsl`, and F11 in the app to cycle them: the place, the
      kind, the height, the occluders, the light alone, the shadow term
      alone, and how many flames reached a fragment.
- [x] **Step 10. The parity test.** A synthetic place attachment
      uploaded to the GPU, the real blit run over it, and every sampled
      pixel compared with `light::sample`. No client files and no art —
      this is about two implementations of one formula, and decision 9
      is what it protects.
- [x] **Step 19. The plan view, and the two dumbest scenes there are.**
      The instrument decisions 18 and 19 were found with, and the one
      this pass did not have: `render/src/plan.rs` draws the **real
      blit** over a synthetic place attachment that says every pixel is
      flat ground on the tile above it, one tile to a square of `scale`
      pixels. The world image is white, so what comes out is the
      multiplier itself — a circle in the world is a circle in the
      picture, a tile is a square, and a wall is a line one can point
      at.

      It is the same seam `tests/frame.rs`'s parity fixture already
      used, lifted out of the tests so that a person looking at a bug
      can get a picture without writing one. Nothing here computes
      lighting: a plan view with its own arithmetic would be a third
      implementation of decision 9's one formula.

      `Picture::mark` strokes **the reasons** over it — every occluding
      cell's panel on the side it stands on, coloured from glass to bone
      by opacity, a lid as a dashed square, the tile grid, each flame
      and the dashed rim of its reach. That is the half without which a
      picture cannot be read: a pool that is the wrong shape and a pool
      that is the right shape behind a wall nobody drew are the same
      picture until the wall is drawn on it.

      And two scenes as dumb as they can be, because every scene this
      file had was a room: `scene::torch_on_open_ground` — one torch,
      nothing else, so a pool that is not a circle here is not a circle
      anywhere — and `scene::torch_before_a_wall`, one straight nine-tile
      wall two tiles from a torch, which is one shadow and nothing else.
      The spokes were visible in the second one at the first attempt.

      `tests/pictures.rs` writes every scene in five views under
      `target/lighting/` (or `OPENSHARD_LIGHT_PICTURES`) and asserts
      what a shape can state: the pool is the same brightness in every
      direction at every distance, it falls off at every step of its
      inner half, it never brightens outwards, and the wall darkens the
      ground behind it and not the ground beside it.

      `View::Flames` came out of the same session and is the sixth view:
      what the flames added, with the ambient subtracted, on black.
      `View::Light` cannot answer "does this pool have a shape" — it
      draws the ambient underneath and bends everything over `KNEE`
      towards white, so a torch's whole falloff is squeezed into the top
      third of the range and reads as a flat bright blob. Take the
      ambient out and the same pool is a gradient from white to black
      with nothing under it.

      ```sh
      cargo test -p openshard-client-render --test pictures -- --nocapture
      magick target/lighting/one-torch-on-open-ground.flames.plan.ppm /tmp/look.png
      ```

      **And the elevation, which is the other half of it.** A plan's
      pixels are on the ground; a wall's are not, and the two defects
      decisions 22 and 23 name are invisible in a plan for exactly that
      reason. `plan::elevation` unrolls one run of wall: across is how
      far along the run, down is height, and each pixel is written into
      the attachment as `statics.wgsl` would write it for that point of
      that face — stance included, or the picture would be lit from
      behind. A seam artefact is then a vertical stroke a person can
      point at, and `mark_seams` says where the joins are. The scene it
      was found in is `scene::wall_run_lit_from_along_it`, and the
      arrangement matters far more than the length: a lamp *along* the
      wall draws the strokes and a lamp in front of it draws none.

### Backlog: found while building the observability (misc instrumentation notes)

- **`View::Flames` is the view a shape is judged in, and nothing says so
  in the client.** F11 cycles eleven views now and their names go to a
  log line. A person who does not already know which one answers their
  question has to walk all eleven.
- **The plan view is not in the client.** `render/src/plan.rs` needs a
  device and a queue and draws into its own texture, so a test can call
  it and the app does not. What the app would want is a key that dumps
  the current frame's `Lighting` as a plan beside the screenshot — the
  same instrument pointed at the world the player is standing in rather
  than at a built room.
