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
