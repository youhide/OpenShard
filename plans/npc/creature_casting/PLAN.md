# A creature that casts

The whole of the magic domain is one-directional: the player casts at the world
and the world never casts back. A lich, a mage-brigand and a healing dragon are
all impossible today, and not because the spells are missing.

What is built across this domain is
[`docs/npc/README.md`](../../../docs/npc/README.md); the brain that would do the
deciding is [`docs/npc/design_brain.md`](../../../docs/npc/design_brain.md), and
the spells are [`docs/combat/design_magic.md`](../../../docs/combat/design_magic.md).
This page is what is not built.

## What is already there, and what is genuinely missing

`crates/server/ai` has no notion of a spell: no mana on a creature, no choice of
spell in `fight_phase`, no cast in the beat. But the cast path itself is
reusable — `World::begin_cast` is the *client* seam (it reads a connection and a
spellbook), and `resolve_cast` / `apply_spell_effect` below it are not. A
creature does not need a new spell system.

So what is missing is the **decision**: which spell, at whom, and how often.

## C1 — a creature has mana, and a spell to spend it on

- [ ] A creature carries `Mana` and a small spell list, both spawn data — the
      same shape `ranged` and `sight` already have, so a pack authors a lich by
      naming spells rather than by writing behaviour.
- [ ] `fight_phase` gains one branch above the melee one: if a spell is
      affordable, in range and off cooldown, cast it instead of closing.
- [ ] The cast goes through the same `resolve_cast` a player's does, so the
      words, the gesture, the resist roll and the effect art are the ones that
      already exist. **Nothing about a spell may learn that a creature cast it.**

## C2 — the choice, which is the part with a decision in it

The reference picks by category rather than by scoring every spell, and the
category is what a shard's content actually wants to author:

- [ ] **Harm** at a quarry in sight.
- [ ] **Heal** at self or a wounded friend, gated on how badly hurt.
- [ ] **Curse / buff**, on a cooldown so a fight is not four beats of buffing.
- [ ] **Escape** — a teleport or an invisibility — under a hit-point floor, which
      is the posture equivalent of `should_flee` and should read off the same
      question rather than a second one.

**The knob to avoid** is a per-spell weight table in the spawn data: it is the
kind of setting that has to be right for every creature and is right for none.
Category plus a spell list per category is the smaller thing that expresses the
same content.

## C3 — the cadence, which is where this breaks the tick if it is wrong

- [ ] A cast has a delay, and a creature's beat is not the delay. A creature that
      starts a cast must be *rooted* the way a player is, on the same `Casting`
      component and the same resolution pass, or two systems will each believe
      they own the mobile for the next second.
- [ ] LOD must not doze a caster mid-cast, which is the same clause that already
      keeps a creature in a fight awake.
- [ ] Determinism: the choice spends the world's seeded `Rng` like every other
      brain decision, so a fight against a mage still replays.

## What has to be decided before C1 rather than during it

**Whether a creature's spell list is data or a body default.** `creature_name`
and `creature_base_sound` are keyed by body and defaulted in core with the pack
free to override; a spell list could be the same, or it could be per-spawn only.
The two answers produce different files, and the choice is the same one the
body-table sweep is already open about — see the domain README's row on the three
tables that share the `body` key.

**And a scroll cast is a player's alone until this lands.** A creature reaches no
double-click, so the scroll path is not a second gap; it is this one.
