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
| [`docs/lighting_rebuild.md`](docs/lighting_rebuild.md) | 🚩 **Start here for anything about light.** The lighting we are building instead — deferred shading, art as albedo, shadows by primitive identity. It is the single entry point for eight consolidated plans: what is still live in each, which phase retires or inherits it, and what carries over untouched. |
| **Consolidated into it** — [`lighting.md`](docs/lighting.md), [`lighting_world.md`](docs/lighting_world.md), [`lighting_raymarch.md`](docs/lighting_raymarch.md), [`lighting_geometry.md`](docs/lighting_geometry.md), [`lighting_height.md`](docs/lighting_height.md), [`lighting_reference.md`](docs/lighting_reference.md), [`gbuffer.md`](docs/gbuffer.md), [`world_coordinates.md`](docs/world_coordinates.md) | The record of how each was built and why. Read one when you need its reasoning — not to find out what is left to do. |
| **Living plans** — [`camera.md`](docs/camera.md), [`connection_state.md`](docs/connection_state.md), [`shutdown.md`](docs/shutdown.md), [`outline.md`](docs/outline.md), [`protocol_newtypes.md`](docs/protocol_newtypes.md), [`protocol_rewrite.md`](docs/protocol_rewrite.md) | Multi-session refactors, each with a backlog of what's left undone — that backlog is where the next session starts. |
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
