# The quest model's remaining half

Four objective kinds, a log, a window, a turn-in and givers that survive a
restart are built. What is deferred is the part of ServUO's model that is about
*structure* rather than about a single quest: chains, a reward the player picks,
and the converter pass that would give the shard content rather than examples.

The model is [`docs/npc/design_quests.md`](../../../docs/npc/design_quests.md),
what the phase found is
[`docs/npc/evidence/2026-08-24-the-parties-and-quests-phase.md`](../../../docs/npc/evidence/2026-08-24-the-parties-and-quests-phase.md),
and what is built across the domain is
[`docs/npc/README.md`](../../../docs/npc/README.md). This page is what is not
built.

## Q1 — a reward the player chooses

Today a turn-in pays everything in the rewards list. ServUO's quest can offer a
*choice*, and the window already has the section for it.

- [ ] A flag on the quest saying the rewards are alternatives.
- [ ] The Rewards page becomes selectable, which is a radio group — the machinery
      the resign dialog already uses.
- [ ] The choice is remembered on the server between the click and the turn-in,
      never carried by the client, for the same reason the *page* is not.

## Q2 — chains

A quest that offers the next one on completion is a field and a rule, not a
system — but the rule has two edges worth writing down before the field exists.

- [ ] `next` on a `QuestDef`, offered by the same giver at the same turn-in.
- [ ] What happens when the next quest's giver is somebody else, which is the
      case the field alone does not answer.
- [ ] What `done_once` means for a chain: finishing a link, or finishing the
      chain.

## Q3 — the two objective kinds that are not built

- [ ] **`ApprenticeObjective`** — reach a skill level. It reads the skill-changed
      event the shard already emits, so it is the cheapest of the two.
- [ ] **The question-and-answer objective**, which needs a dialog the gump does
      not have yet.

## Q4 — the staff force-complete button

The window has the button in the reference. It is one authority check and one
call into the turn-in path — worth doing *with* Q1, because a force-complete on a
quest with a reward choice has to decide which reward it pays.

## Q5 — the converter pass

The engine's model now matches ServUO's, which it did not when the pack-first
version was written. So a converter pass over the reference's own `BaseQuest`
subclasses is possible for the first time, and it is what turns two example
quests into content.

- [ ] Read the subclasses the way the spawn and decoration passes read theirs:
      class name to objective kind, literal fields to data, and anything that
      resolves indirectly is *dropped and reported* rather than guessed.
- [ ] The count of what converted and what did not is the deliverable, the same
      as every other converter pass — a silent skip is what made the creature
      spawns lose a whole camp.

## What has to be decided before Q5 rather than during it

**Which town NPCs are quest givers.** The Felucca converter skips town NPC types
with no vendor class and no shop, which is where the escortables and the
Bard-Mastery knights land today — they are dropped rather than placed as givers.
Deciding that they are `quests`' to claim is what makes Q5 produce a populated
facet instead of a file of quests nobody in the world offers.
