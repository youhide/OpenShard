# Archived render documents

Twelve documents describing the lighting engine **that was replaced**. They are
kept for their reasoning, not for their instructions: every one of them was
consolidated into what is now
[`../../render/design_model.md`](../../render/design_model.md), and where one of
them and a live document disagree, the live one is right.

Read one of these to find out *why* a thing was built the way it was, or what
was tried and abandoned. Do not read one to find work.

## The six tracks, each with its own session log

| Document | What it was | Its archive |
|---|---|---|
| [`lighting.md`](lighting.md) | the pass being replaced: a flame a wall can stop | [`lighting_archive.md`](lighting_archive.md) |
| [`lighting_world.md`](lighting_world.md) | ambient and the sky field — most of it survives in the model | [`lighting_world_archive.md`](lighting_world_archive.md) |
| [`lighting_raymarch.md`](lighting_raymarch.md) | boundary precision and CPU/GPU parity of the walk, which survives | [`lighting_raymarch_archive.md`](lighting_raymarch_archive.md) |
| [`lighting_geometry.md`](lighting_geometry.md) | box-to-mesh occluders — never started | [`lighting_geometry_archive.md`](lighting_geometry_archive.md) |
| [`gbuffer.md`](gbuffer.md) | the `place` attachment format, replaced by phase 2 | [`gbuffer_archive.md`](gbuffer_archive.md) |
| [`lighting_height.md`](lighting_height.md) | height as a continuous quantity; its backlog was mostly deleted rather than fixed | — |
| [`world_coordinates.md`](world_coordinates.md) | one metric, half of which became phase 2 | — |

A `*_archive.md` file is a companion, not a duplicate: its parent held the
current state, it holds the arguments and the session-by-session record behind
that state. The pair is the reason both survive the move.

## What the rebuild did to each of them

The mapping the rebuild wrote when it consolidated these seven, kept here rather
than in the live design because it is a statement about *these* documents:

| document | what it was | what happened to it |
|---|---|---|
| [`lighting.md`](lighting.md) | the current system, end to end: place attachment, occlusion grid, ray walk, sun, beams, doors, art measurement | **the thing that was replaced.** Its mechanisms were retired phase by phase; its *content* work survives untouched |
| [`lighting_world.md`](lighting_world.md) | ambient, the sky field, the day curve, tonal response | **mostly survives.** The sky field is ambient occlusion by another name and phase 8 adopts it; the day curve and the tonal response become phase 1's and phase 8's business |
| [`lighting_raymarch.md`](lighting_raymarch.md) | the DDA walk, CPU/GPU parity, the tile-boundary hazard | **survived phases 4–6 as the walk, and phase 6e retired it.** Phase 4 changed what a hit *means* (identity, no bias) and not how cells are stepped; [`design_occluders.md`](../../render/design_occluders.md) deleted the stepping itself, and with it the tile-boundary hazard and the corner tie — a hierarchy has no cell to be on the boundary of. What carries over unchanged is `ray_vs_solid`, which was never about cells: it is an exact slab test in world coordinates and it is what the new traversal ends in |
| [`lighting_geometry.md`](lighting_geometry.md) | box → mesh occluders, never started | **cheaper after phase 4**, which makes primitives addressable by id, and **started at phase 6**: a tread is one body rather than two degenerate surfaces, which is the first time the grid's own shape was chosen for what a *view* ray needs as well as a shadow ray. `facing::Blocks` — an authored list of up to four boxes, written and wired to nothing — is where the generic form continues |
| [`lighting_height.md`](lighting_height.md) | the height track: four landed phases and a long backlog | **the backlog was mostly deleted rather than fixed** — see the mapping below |
| [`../../render/reference/path_tracer.md`](../../render/reference/path_tracer.md) | the path tracer, a third opinion with no shared arithmetic | **became phase 0**, the oracle everything else is judged by. It is live reference, not archive |
| [`gbuffer.md`](gbuffer.md) | the `place` attachment's format, ids, per-face mesh geometry | **phase 2 replaced the format** and inherited every one of its readers. Its open question — how to encode a normal for a non-axis-aligned face — is answered there: an octahedral pair packed as integers into an `R32Uint`, with two bits over for the two answers that are not directions. (`Rg16Snorm`, which the rebuild first named, is not a format wgpu will render to under WebGPU's core set; the plane spent one phase as three floats before it was packed) |
| [`world_coordinates.md`](world_coordinates.md) | a position should carry its own cell; one metric | **half of it is phase 2** (positions as data, `z` in tiles once). The CPU-side type stays its own track |

### What each phase deleted from `lighting_height.md`'s backlog

So that backlog can be read as history rather than as a list of things that may
or may not still matter:

| backlog entry | fate |
|---|---|
| ~~`FACE_EDGE`'s two scales; the flame at a surface's own height~~ | **done, phase 3** — there is no band, and a flame in a surface's own plane is a cosine of zero rather than a half |
| `STAND_OFF`/`ON_TOP` at a grazing corner; the `ON_TOP` twin | **done, phase 4** — there is no nudge |
| risers excused as a group; `flame_end`'s height test; a mobile shadowed by its own wall | **done, phase 4** — identity answers all three |
| `own_run` | **survived phase 4, measured** — a run of wall is N statics, which no identity merges. **Retired at phase 6e**, which is where a run *does* become one solid: [`design_occluders.md`](../../render/design_occluders.md) S3 merges it and S4 deletes the rule, each behind its own measurement |
| the `ground < 1e-6` shortcut ignoring a lid's footprint | **fixed** — it was worth fixing alone, and was |
| `WIDTH_OVERLAP`'s border | **done, phase 6** — there is no second silhouette for a border to reach across |
| the riser penumbra graded over a third of a face | **done, phase 5** — there is no band; a penumbra is eight rays disagreeing |
| the wire's span rounding to nearest; the exact-tangent definition | **phase 4** — a primitive is not a byte range any more |
| `boxes.rs` reading `Unreached` as shadowed; `two_cubes.rs`'s old idiom; the projection idiom stated five times; `mesh::Face`/`facing::Face` colliding | **survive** — instrument work, still worth doing. One of the five spellings of the projection went at phase 6c: `statics.wesl`'s inverse of it is `impostor::ray_from` now, which is a forward ray rather than an unprojection |
| `Occlusion::owner_at`'s linear scan; `selected`/`outlined` stamping `OwnerId::NONE` | **survive**, reshaped by phase 4's ids |
| `tests/cost.rs` measuring three planes of five; `plan::Wall::top` as an `i32`; hand-copies of the third channel | **survive** — the third channel's copies went with the channel, and the other two are still work |

The entries marked *survive* are live work; they are carried in
[`plans/render/lighting/PLAN.md`](../../../plans/render/lighting/PLAN.md) and in
[`../../render/evidence/2026-08-11-lighting-backlog-findings.md`](../../render/evidence/2026-08-11-lighting-backlog-findings.md),
not here.
