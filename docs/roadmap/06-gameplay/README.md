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
benchmark is [`05-scripting.md`](../../server/evidence/2026-08-24-the-scripting-spike.md); the decision is
[`architecture.md`](../../architecture.md) § Scripting.

## Contents

Seven subjects have left this directory, into three migrated domains.

**Items** and **Crafting** are records in `items` —
[`items/evidence/2026-08-24-the-items-phase.md`](../../items/evidence/2026-08-24-the-items-phase.md)
and
[`items/evidence/2026-08-24-the-crafting-phase.md`](../../items/evidence/2026-08-24-the-crafting-phase.md)
— and what is open about them is ranked in
[`items/README.md`](../../items/README.md).

**Combat**, **Skills** and **Magic** are records in `combat` —
[`combat/evidence/2026-08-24-the-combat-phase.md`](../../combat/evidence/2026-08-24-the-combat-phase.md),
[`combat/evidence/2026-08-24-the-skills-phase.md`](../../combat/evidence/2026-08-24-the-skills-phase.md)
and
[`combat/evidence/2026-08-24-the-magic-phase.md`](../../combat/evidence/2026-08-24-the-magic-phase.md)
— and what is open about them is ranked in
[`combat/README.md`](../../combat/README.md).

**Housing** and **Boats and house customisation** are records in `housing` —
[`housing/evidence/2026-08-24-the-housing-phase.md`](../../housing/evidence/2026-08-24-the-housing-phase.md)
and
[`housing/evidence/2026-08-24-the-boats-and-customisation-phase.md`](../../housing/evidence/2026-08-24-the-boats-and-customisation-phase.md)
— and what is open about them is ranked in
[`housing/README.md`](../../housing/README.md).

What is still here:

- [AI](ai.md)
- [Chat and world administration](chat.md)
- [Regions, guards, and the world clock](regions.md)
- [Guilds](guilds.md)
- [Parties and quests](parties-and-quests.md)
