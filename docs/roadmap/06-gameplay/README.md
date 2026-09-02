# 6. Gameplay

> Open work and follow-up findings from this phase are tracked in the
> [consolidated backlog](../../../plans/roadmap/PLAN.md).

Roughly in dependency order.

**This phase was written script-first, and it is not any more.** It used to open
with a bridge: the server owned a `DenoEngine`, delivered each tick's domain
events to it and queued the commands it emitted for the next tick, with
`scripting.main` in the config naming the script. That whole seam — the
`openshard-scripting` crate, the bridge beside it, `deno_core` and the
`[scripting]` config section — was **deleted**, and the pages below still carry
sentences written when it existed.

What replaced it is data, not a runtime: `crates/*/data/*.json` compiled by a
`build.rs` and laid by `server::content`, plus the two item behaviours that were
scripts as `world::tick::shipped_items`. Each dataset moved under a test that
compared its `Command`s against the pack's. The record of the spike and its
benchmark is [`05-scripting.md`](../05-scripting.md); the decision is
[`architecture.md`](../../architecture.md) § Scripting.

## Contents

- [Items](items.md)
- [Combat](combat.md)
- [Skills](skills.md)
- [Crafting](crafting.md)
- [Magic](magic.md)
- [AI](ai.md)
- [Chat and world administration](chat.md)
- [Regions, guards, and the world clock](regions.md)
- [Housing](housing.md)
- [Boats and house customisation](boats-and-customisation.md)
- [Guilds](guilds.md)
- [Parties and quests](parties-and-quests.md)
