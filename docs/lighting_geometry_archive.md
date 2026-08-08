# The occluding primitive: box or mesh — archive

The reasoning, arguments, and session record behind
[`lighting_geometry.md`](lighting_geometry.md). Organized under headings
mirroring that document's own.

## Why this document exists

It exists because three living docs —
[`lighting.md`](lighting.md), [`lighting_world.md`](lighting_world.md),
[`lighting_raymarch.md`](lighting_raymarch.md) — and
[`gbuffer.md`](gbuffer.md) each built a subsystem on top of one shared,
mostly-unstated assumption: the thing a ray stops at is an axis-aligned box.
One of the four wrote that assumption down as a decision and argued for it
at length — `lighting.md`'s former decision 40, "written down because the
alternative was argued for at length... and the argument deserves the answer
on the record rather than a re-litigation the next time a curved roof or a
mountain comes up." That sentence named the session this document was
written in. This document is the answer decision 40 asked for, not a
rewrite of the paragraph it is answering — the reasoning in decision 40
stood, quoted and engaged with below, because the box it argued for was not
going away; only its status as the ceiling was.

`gbuffer.md`'s own "Not settled" list had already half-reopened the same
question from the *shading* side (its own note: "`docs/lighting.md`
decision 35's rejection of sloped roofs... is no longer settled," dated
2026-08-05, two sessions before this one) and had deliberately fenced
itself off from the *occlusion* side: "purely a step 4c/5 question —
nothing about... the occlusion-grid work." That fence is what this document
took down. Nobody had connected the two halves before this session, which
was most of why the direction read as three contradictory tracks rather
than one that had already started moving.

## What already supports a mesh occluder — the full argument

Four things already leaned toward a mesh, none of them written with this
document in mind:

- **Decision 38 (`lighting.md`) already decoupled a solid from the tile
  grid.** It is a box in *world* coordinates with its own six numbers,
  referenced — not owned — by every cell it touches (38.1). The walk
  already treats a solid as something a cell holds a reference to, not
  something whose shape a cell's own boundary constrains. Generalising
  `occlusion::Solid.space` (`occlusion.rs:563`, today `crate::solid::Solid`,
  one box) from a single variant to box-or-mesh costs the bookkeeping
  decision 38 already pays, not new bookkeeping.
- **Decision 41 (`lighting.md`, `facing::Blocks`) already generalised past a
  single box once, for a cheaper reason.** Several authored axis-aligned
  boxes per graphic, for a shape `Prism`'s one-axis climb profile can't
  describe (an arch: a lintel floating over a gap two posts don't touch). It
  is the existence proof that "compose several boxes by hand" already had a
  home in this table (`arttable.rs`'s `block x0 x1 y0 y1 z0 z1` grammar),
  and it is very likely the *right* answer for a good fraction of what looks
  like it needs a mesh — a jagged silhouette is boxes at some resolution;
  true curvature is not, at any resolution worth authoring. The line between
  "reach for decision 41" and "reach for a mesh" is decision 1 below.
- **Decision 31.2's authored table (`lighting.md`) already lets a
  hand-authored entry win over a derived one.** A mesh is data nothing can
  derive from a flat 2D sprite — decision 3's argument, still correct and
  untouched — so it was always going to need authoring rather than
  detection, exactly the path decision 41 already walked for `Blocks`. The
  table already has the "authored wins" row; a mesh is a new payload in an
  existing column.
- **`crates/client/render/src/mesh.rs`'s own doc comment already said this,
  unprompted**, written for step 4c's tread geometry: "a sloped roof, or any
  future custom geometry, builds its own `Mesh` the same way, and whatever
  walks one draws every `Face` alike." `MAX_FACE_VERTICES`/`MAX_MESH_FACES`
  are stated as caps to raise "against its own shape," not ceilings. The
  code was already built expecting this question; the decision log is what
  had fallen behind it.

## What changes — the reopened decisions, in full

**1. `lighting.md` decision 40 is reversed: a general mesh is wanted, and
the box stays the default rather than the ceiling.** Decision 40 priced a
mesh *for every occluder*, uniformly — a BVH replacing a grid that is free
because every box in it is tile-aligned, and a ray-plane test becoming a
ray-mesh test `blit.wgsl` and `light.rs` would each have to get identically
right — and found that not worth paying for content that is, at the time it
was written, entirely boxes. That arithmetic is correct and nobody is asking
to pay it uniformly. The ask is a mesh *only where a box, or decision 41's
several composed boxes, cannot state the shape* — exactly the "authored,
only where it's needed, everything else stays the cheap derived case"
discipline decision 31 already runs the rest of this table by. Decision 40's
conclusion doesn't survive rephrasing the question it answered; its cost
accounting does, and is inherited by decision 3 below rather than argued
again.

**2. `occlusion::Solid.space` (`occlusion.rs:563`) grows a second shape.**
Today it is one box, `crate::solid::Solid`. It needs a mesh-backed variant —
whether that is an enum on `Solid` itself, or the box staying the fast path
with a mesh addressed the way decision 38.4 already indexes solids
indirectly (an offset into a second table), is a design question, not a
decided one. What is decided is the shape of the answer: the box's own
fields (`opacity`, `edges`, the aperture) stay exactly what they are for a
box, and whatever a mesh variant needs is additive, not a reshaping of what
already works.

**3. `ray_vs_solid` (`light.rs:1160`, the GPU copy in `blit.wgsl`) gets a
mesh sibling, paid twice, on purpose.** This is decision 40's own named
cost, restated as accepted rather than avoided. Decision 9's CPU/GPU parity
discipline — an independent oracle and the shader hand-derive the same
formula and a test catches drift — is exactly the tool this repo already
has for this problem, proven on the box test across
[`lighting_raymarch.md`](lighting_raymarch.md)'s twenty-some sessions. It is
asked to do the same job for a mesh test, built together, never one side
first and the other "to be ported" — the failure mode
[`lighting_raymarch.md`](lighting_raymarch.md)'s own point 1 already carries
a live, unresolved instance of for the box case, worth reading before
assuming a mesh test will be easier to keep in sync.

**4. `solids.wgsl`'s debug view (`lighting.md` decision 39.3) loses its "no
index buffers" property for a mesh occluder.** 39.3 is not wrong for a box —
three constant normals, six numbers and a colour, and it stays exactly that
for a box. A mesh occluder drawn in the same diagnostic needs a real
triangle list, which is the "mesh pipeline" 39.3 said this deliberately
wasn't, for a box. Casualty, not a contradiction: the diagnostic grows a
second draw path for the rarer shape, the same way decision 30's occlusion
field already carries more than one kind of record without either
pretending to be the other.

**5. `gbuffer.md`'s "Not settled" fence between shading and occlusion comes
down — it was never really two questions, only asked in that order.** A
general per-face normal for *shading* a slope (`gbuffer.md`'s own
reopening) is strictly smaller than a general *shape for a ray to stop at*
— the smaller question doesn't even need this document, since decision 36
already generalises a box's normal from its own vertices (a tread's tilted
top, a land tile's fitted plane) without a mesh. What forces the fence down
is content whose silhouette isn't a box at all: a curved roof's shadow is
not the shadow of its bounding box, and that is an occlusion question the
shading-side reopening was never going to answer by itself. One primitive,
one place both halves are decided together from here.

## Not reopened — checked and left standing

- Decision 39.5 (`lighting.md`, billboards stay billboards) — mobiles and
  characters, out of scope: their art is 2D sprite frames, not skeletal
  geometry, and replacing that is a different, much larger undertaking (the
  closest real precedent anyone here has looked at is Iris2's Granny-model
  loader, GPL, read-only, never vendored) that nobody asked for in this
  session. If it is wanted later it is its own track, not a rider on this
  one.
- Decisions 39.1/39.2/39.4 (`lighting.md`: the projection, the client's
  depth ordering, a slope drawn as a parallelogram) — about *how a box is
  drawn on screen*, orthogonal to what shape the occluder underneath it is.
- Decision 38's own reasoning (world-coordinate anchoring, reference rather
  than clip) — this is what a mesh inherits unchanged, not what it
  disturbs.
- Decision 41 (`facing::Blocks`) — stays the cheaper answer for an irregular
  but axis-aligned silhouette; a mesh is for what even several boxes can't
  state. Neither replaces the other.
- Decision 31's whole measurement pipeline (the atlas, the derive-then-
  override table) — a mesh is a new payload in the authored column, not a
  new column or a new mechanism.
- `lighting_world.md`'s field plane (sky/aperture/body, its own decision
  1) — a property of a tile, computed by walking whatever occludes it.
  Box-or-mesh underneath doesn't change what the field stores, only what
  the walk that fills it tests against — flagged as unproven rather than
  safe (see the live doc's Status).

## Notes on process

- **Numbering.** This document's reopened decisions were numbered fresh
  (1-5) rather than continuing `lighting.md`'s sequence, because they lived
  in a different file and were cross-referenced by `file:decision` rather
  than a shared counter — the convention every other living plan here
  already used for referring to another doc's decisions.
- **What was surveyed and found not to need reopening is listed above, on
  purpose** ("Not reopened — checked and left standing"), so this archive
  itself is the record that the other ~35 decisions across the four
  original docs were looked at rather than silently skipped — the
  difference between "reopened everything relevant" and "reopened
  everything," which was the ask.

## Handoff log

One entry per session, newest first.

### Session 1 — this document written; decision 40 reopened and answered, not re-litigated

Started from the user naming, across several sessions, a direction (general
geometry for statics/terrain) that the decision log had recorded arguing
against (`lighting.md` decision 40) in the same file that, two sessions
before this one, had already half-reversed itself on the adjacent question
from the shading side (`gbuffer.md`'s decision-35 reopening, 2026-08-05) —
read as three contradictory tracks because nobody had connected the two
reopenings or engaged decision 40 on its own terms. Surveyed `lighting.md`
decisions 30-41, `lighting_world.md`'s decision 1, `gbuffer.md`'s "Not
settled" list, and the current `occlusion::Solid`/`mesh.rs`/`ray_vs_solid`
code before writing anything, specifically to answer decision 40's own
request — "the argument deserves the answer on the record" — rather than
overwrite the paragraph. No code changed this session; this was direction
and the decisions it unblocked, not an implementation. Next: the mesh
variant's design.

### Session 2 — the whole lighting documentation set rewritten into current-state doc plus archive, this document included

The repo owner judged the decision-log format itself unreadable as a
"where do things stand" reference across all five lighting-family docs, not
just this one, and asked for a full split: each live doc trimmed to a
textbook-style technical reference (current state only, present tense, no
decision numbering, no argued alternatives), with all reasoning and session
narrative moved to a sibling `_archive.md`. `lighting_raymarch.md` had
already been split once this same session, less strictly, as the first
attempt at the pattern; `lighting.md` and `gbuffer.md` were rewritten by
dedicated agents; `lighting_world.md` and `lighting_raymarch.md` (a second,
stricter pass) were rewritten in the same round. This document (originally
written whole, as decisions with reasoning inline, in session 1 above) was
split the same way: `lighting_geometry.md` now carries only the current
direction and what changes, and this archive carries the full argument.
Nothing about the direction itself changed — this was a format pass, not a
reopening of anything decided in session 1.
