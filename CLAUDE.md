# OpenShard

A modern Ultima Online server engine, compatible with the original 2D client and
ClassicUO. **Not a SphereServer clone** — the engine SphereServer would likely be
if it were designed today: compatible with the UO *protocol*, and with nothing
else about Sphere. Gameplay content lives in a second repository, the
**OpenShard Community Pack**.

## Docs — read before you touch code

| | |
|---|---|
| [`docs/style.md`](docs/style.md) | How code here reads. Read it before writing Rust in this repo. |
| [`docs/architecture.md`](docs/architecture.md) | The shape: layers, dependency rules, the crate map. |
| [`docs/findings.md`](docs/findings.md) | What the client actually does. Every entry cost a day — don't re-derive them. |
| [`docs/roadmap.md`](docs/roadmap.md) | The order, and what is built. Current status lives here, not in this file. |
| [`docs/client.md`](docs/client.md) | Our own client, milestone by milestone. |
| [`docs/client_versions.md`](docs/client_versions.md) | Which clients exist and which are played. |
| [`docs/development.md`](docs/development.md) | The environment: commands, toolchain, `target/`, MSRV. |
| [`docs/lighting_state.md`](docs/lighting_state.md) | 🚩 **Where the lighting engine stands, in one page** — readiness by subsystem, what is left ranked, the defects a person can see, the normative spec for the pixel spaces, and which of the eleven lighting documents are still live. Read it before opening any of them. |
| [`docs/lighting_pitfalls.md`](docs/lighting_pitfalls.md) | 🚩 **How a lit frame lies, and the order to ask it things in.** A bright line on a dark surface is not evidence of light. The four-rung ladder (`Kind` → the albedo control → `Flames` → `Normal`), the amplifier that makes a facing error read as a drawn artefact, and each pitfall with the wrong verdict it cost. Read it before diagnosing anything a person reported by looking. |
| [`docs/lighting_rebuild.md`](docs/lighting_rebuild.md) | The model itself, phases 0–8, and the backlog every defect is filed in. The lighting we are building instead — deferred shading, art as albedo, shadows by primitive identity. It is the single entry point for eight consolidated plans: what is still live in each, which phase retires or inherits it, and what carries over untouched. |
| **Consolidated into it** — [`lighting.md`](docs/lighting.md), [`lighting_world.md`](docs/lighting_world.md), [`lighting_raymarch.md`](docs/lighting_raymarch.md), [`lighting_geometry.md`](docs/lighting_geometry.md), [`lighting_height.md`](docs/lighting_height.md), [`lighting_reference.md`](docs/lighting_reference.md), [`gbuffer.md`](docs/gbuffer.md), [`world_coordinates.md`](docs/world_coordinates.md) | The record of how each was built and why. Read one when you need its reasoning — not to find out what is left to do. |
| [`docs/occluders.md`](docs/occluders.md) | The occlusion geometry, being rebuilt: one shape per surface, absolute world coordinates, a BVH, and no tile in the answer. `lighting_rebuild.md`'s phase 6e in full — its decisions are **made**, so read it before proposing another. |
| [`docs/parity.md`](docs/parity.md) | 🚩 **One frame, however it was asked for.** A frame is assembled by hand in seven places, so parity between the client and every diagnostic tool is a coincidence rather than a property — read it before adding a caller or trusting a tool's picture. |
| [`docs/footprints.md`](docs/footprints.md) | A static's box is the box the art drew. The other half of `occluders.md`, which put it out of scope by name: that plan changes how many primitives a surface is, this one changes what one primitive's *shape* is — the 31.6% of the world currently given a whole tile because "the art would not say". |
| [`docs/pixels.md`](docs/pixels.md) | Six grids meet in this renderer and no document listed them. Not a glossary — a statement of **which pairs share a divisor**, because a sample landing exactly on a discontinuity is what `parity.md`'s window-parity defect was made of. |
| [`docs/silhouettes.md`](docs/silhouettes.md) | The zigzags. A magnified frame draws its outlines at two resolutions — a box's edge per fragment, the art's alpha per texel — and they meet along one line. First attribute them with a debug view, then decide. |
| [`docs/housing.md`](docs/housing.md) | 🚩 **A house, and the ground it stands on.** The picture is free — every client already owns every house — so what the shard owes is the walls that stop you, the door that knows you, and the decay that takes it away. **H1–H5 built.** H6 is the sixth phase of a five-phase plan, and it is half a correction: three things this document published as decided were never built, and they are one thing — housing and regions never met. |
| [`docs/customisation.md`](docs/customisation.md) | **A house whose shape nobody shipped.** Housing's D7, reverted in full. The picture stops being free for exactly one kind of house, and the load-bearing decision is where a per-house component list lives — because `Terrain::multi_components` cannot hold one, for five reasons, and the fifth is why minting a multi id is not an escape hatch. |
| [`docs/world_map.md`](docs/world_map.md) | **A map we can change.** Why the world cannot be edited today — six readers, none of them owning it — and what has to become true. The entry point of three: mechanics in [`world_map_mechanics.md`](docs/world_map_mechanics.md), the work and the code it touches in [`world_map_plan.md`](docs/world_map_plan.md). |
| [`docs/boats.md`](docs/boats.md) | **A house that moves.** Every hard decision follows from *moves*, not from *boat*. No parent transform — refused on the engine's own evidence, since mounting deletes the mount rather than carrying it. The hull stays out of `Obstructions`, which only ever subtracts and cannot say "there is somewhere to stand here". |
| [`docs/combat.md`](docs/combat.md) | 🚩 **The fight, and what it leaves behind.** War mode, the blow, the bar, the death and the corpse — one plan in six phases, because they are one loop. The server already runs all of it; every gap is at the client's end, and the table at the top is which packet each one is. |
| [`docs/interiors.md`](docs/interiors.md) | **A building, and what it lets you see.** Three asks, one seam — the storey as a person's choice rather than `UpdateMaxDrawZ`'s, a **sealed room as a black area**, and walls cut to the knee. The second needs an index of rooms, and the first is what makes it necessary: with the roof on, a sealed room is already invisible. R0 is a refactor with no feature in it, and every phase after it names which precondition it spends. |
| **Living plans** — [`camera.md`](docs/camera.md), [`connection_state.md`](docs/connection_state.md), [`shutdown.md`](docs/shutdown.md), [`outline.md`](docs/outline.md), [`protocol_newtypes.md`](docs/protocol_newtypes.md), [`protocol_rewrite.md`](docs/protocol_rewrite.md), [`facet_newtype.md`](docs/facet_newtype.md), [`client_window_state.md`](docs/client_window_state.md), [`window_components.md`](docs/window_components.md) | Multi-session refactors, each with a backlog of what's left undone — that backlog is where the next session starts. |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | What lands and how: branch, PR, review, merge commit, commit messages. |

## Working on this

```sh
cargo test --workspace          # includes doctests
cargo clippy --workspace --all-targets
cargo fmt --all
```

All three are expected to be silent — that's what CI runs on every PR.

## Non-goals

Reimplementing SphereScript. Parsing `.scp` at runtime. Source compatibility with
Sphere. Legacy save formats. Mimicking Sphere's internals.
