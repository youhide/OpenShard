# 6. Gameplay

> Open work and follow-up findings from this phase are tracked in the
> [consolidated backlog](../backlog/README.md).

Roughly in dependency order, each script-first:

- [x] **The script is wired into the tick.** The bridge §5 deferred: the server
  owns a `DenoEngine`, delivers each tick's domain events to it, and queues the
  commands it emits for the next tick. `scripting.main` in the config names the
  script; empty runs scriptless, the same bargain as an empty map. A script acts
  through `Command::Step` — server-authoritative movement, no client sequence or
  pace, terrain the only judge — which is the first thing a script command lands
  on. `crates/server/server/src/scripting.rs` is the whole seam.

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
