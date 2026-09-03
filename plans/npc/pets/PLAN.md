# A pet that is kept, not only tamed

Taming resolves, a pet follows, obeys and counts against a follower slot. What a
pet has no notion of is **time**: it cannot be put away, it cannot be fed, and it
cannot become less yours through neglect. Herding — the other half of the
animal-handling skills — is not built either.

What is built is [`docs/npc/README.md`](../../../docs/npc/README.md), the pet's
beat is [`docs/npc/design_brain.md`](../../../docs/npc/design_brain.md), and the
skill that creates one is
[`docs/combat/design_skills.md`](../../../docs/combat/design_skills.md). This
page is what is not built.

## P1 — stabling

A stabled pet is **a pet saved with no position**, which is a shape this engine
already has exactly once: a logged-out character. That is the whole of why this
phase is first — it is the one that decides where such a thing lives.

- [ ] A stablemaster NPC, which is the townsfolk base plus a service, the way the
      banker is: keywords ("stable", "claim"), a per-character list, a cap.
- [ ] A pet leaves the world and its record keeps everything a restore needs —
      body, name, hue, skills, hits, the control master — and no tile.
- [ ] Claiming puts it back beside the owner through the same spawn door
      everything else uses, so a stabled pet and a restored one are not two code
      paths.
- [ ] A schema bump, whose argument is the reader's and not the writer's: an
      older build that opened this database would restore a stabled pet nowhere.

## P2 — feeding, and the loyalty that is pointless without it

Loyalty with nothing that moves it is a number that only ever goes down or only
ever stays, and either way it is a fixture rather than a mechanic. So the two are
one phase.

- [ ] Food as a drop onto the pet, matched against what that body eats.
- [ ] A loyalty value on `Pet`, saved, raised by feeding and lowered on a slow
      tick — the decay shape, so it is replayable and needs no wall clock.
- [ ] Going wild at the floor: the pet loses its master and becomes an ordinary
      creature with its posture back. This is the row that makes the number mean
      something, and it must be the same "stop being controlled" path a summon's
      deadline already takes rather than a second one.

## P3 — Herding

- [ ] The skill, its targeting (a creature, then a destination), and the check.
- [ ] What it moves: a herded creature walks to a place, which is a
      `Goal::Place` route in the brain — the machinery is there and the decision
      to use it is not.

## What has to be decided before P1 rather than during it

**Whether a stabled pet is an entity at all.** A logged-out character is not; it
is a row. If a stabled pet is a row too, then everything about it — its skills,
its loyalty, its name — has to be expressible in the record, and the record is
the thing that has to be got right once. The alternative, an entity parked
somewhere with no position, is what "limbo" already means for a ridden mount, and
that is a precedent worth reading before choosing against it.
