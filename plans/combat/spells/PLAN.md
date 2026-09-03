# The spells that are not built, and the one thing that never casts

Magery casts, costs, rolls, resists, persists and dispels. Fourteen of the
sixty-four rows are `SpellEffect::Unimplemented` — they cast fully and then
nothing happens — and one whole *caster* is missing: nothing but a player ever
casts anything.

The model is [`docs/combat/design_magic.md`](../../../docs/combat/design_magic.md);
what is built is [`docs/combat/README.md`](../../../docs/combat/README.md); the
family-by-family record is
[`docs/combat/evidence/2026-08-24-the-magic-phase.md`](../../../docs/combat/evidence/2026-08-24-the-magic-phase.md).

The order below is cheapest-first, and the first two items are cheap for the same
reason: the subsystems they need already exist and nobody has written the arm.

## 1. The eleven arms that need no new subsystem

Each is a `SpellEffect` variant and an arm in `apply_spell_effect`, against
machinery that is already in the tree:

| Spell | What it already has |
|---|---|
| Create Food | spawn into the pack — `items::give` |
| Mana Drain, Mana Vampire | `Mana` and the one `set_mana` door |
| Arch Protection, Mass Curse | the area sweep plus both buff appliers |
| Invisibility, Reveal | `Hidden`, `break_cover`, `refresh_around` |
| Magic Lock, Unlock | the lock component and its two skill levels |
| Magic Trap, Untrap | `Trap`/`TrapKind` and `world::tick::traps` |

Each also needs its `SpellArt` row filled — a `Silent` row on an
`Unimplemented` spell is a placeholder, not a decision, and the two change
together.

**Done when:** no row in the table is `Unimplemented` for want of an arm rather
than for want of a subsystem, and each new arm plays ServUO's own sound and
particle for that spell.

## 2. A scroll casts

A Magery scroll can be dragged onto a spellbook to learn its spell, and that is
all it does: double-clicking one casts nothing. Classic UO casts from the scroll
itself — at the circle's difficulty less one, without reagents, consuming the
scroll. That is the piece that makes a scroll worth buying for a mage who cannot
yet hold the circle.

It comes in through the double-click seam that Healing, Lockpicking and the
harvest tools already use, and it is a second entry into `begin_cast` with two
values overridden rather than a second cast path.

**Done when:** a scroll of a spell the caster's book does not hold still casts
it, once, and the scroll is gone.

## 3. Something other than a player casts

`crates/server/ai` has no notion of a spell: no mana on a creature, no choice of
spell in `fight_phase`, no cast in the beat. A lich, a mage-brigand and a healing
dragon are all impossible, so the whole of magic is one-directional — the player
casts at the world and the world never casts back.

**The cast path is already reusable and this is the argument for doing it here
rather than in `ai`.** `begin_cast` is a client seam (it reads a connection, says
the mantra to a player's audience, and raises a *cursor*), but `resolve_cast` and
`apply_spell_effect` are not. What is missing is the *decision* — which spell, at
whom, how often — plus the two things a creature has not got:

- a mana pool worth reading on a creature, and a spawn column that sets it;
- a way to aim without a cursor. A targeted cast by a creature currently
  **lapses**, because `resolve_cast` raises a cursor only for an entity with a
  `Client`. The honest fix is for a brain to supply the aim at the point the
  cursor would have gone up, not for the creature to be given a fake client.

This is where the domain boundary is worth stating: the *choice* is `ai`'s and
the *cast* is `magic`'s, and the seam between them is one function that takes a
caster, a spell and an already-resolved target.

**Done when:** a spawn row can give a creature a spell list, a lich casts it, and
a fight against one replays from the seed like any other.

## 4. The three that are genuinely blocked

- **Polymorph** waits on a body-swap subsystem that restores cleanly — the same
  problem a mount solves by deleting the thing it carries, which is not an answer
  here.
- **Incognito** is the same subsystem with a name and a hue on top.
- **Telekinesis** wants a use-at-range model that nothing else in the engine has.

Each of these is a design before it is a phase. They are listed so that "why is
this still `Unimplemented`" has an answer that is not "nobody got to it".

## 5. Named deferrals, carried from the families that landed

Small, each independently doable, and every one of them was written down by the
slice that chose to skip it:

- **Area art is played once at the aimed spot**, where ServUO strikes every
  mobile Chain Lightning catches and throws a fireball at each one Meteor Swarm
  does.
- **The hand particle at a cast's start** (`LeftHandEffect`/`RightHandEffect`)
  wants the `0xC7` particle packet this engine does not send.
- **The mantra in the caster's own speech hue** — the client's chosen hue passes
  through `chat::say` and is never stored, so there is nothing to read back.
- **The AoS (era 2) resist-swap variants** of Protection, Reactive Armor and the
  paralysis family.
- **A day/night cycle for Night Sight to fight.** The personal-light packet is
  sent and restored correctly; there is nothing yet for it to be brighter than.
- **Blade Spirits and Energy Vortex are summoned controlled**, where the
  reference summons them free — which is why on OSI they famously turn on the
  mage. Reproducing it wants a hostility model the engine has not got: the
  acquire phase only ever acquires *players*, so an uncontrolled spirit would
  hunt the caster and walk past an orc.
- **Summon Creature's beasts share one stat block** — nine bodies over one modest
  woodland animal, where the reference draws eighteen classes with their own
  numbers. It needs a per-body stat table, not eighteen invented rows.
- **The field row's 300 ms stagger and per-tile `stand_z` on slopes.**
- **Travel's remainder:** Sacred Journey (decoded and ignored), the moon-phase
  gates, red/young restrictions, ship-mark runes, an `op_place_moongate` for the
  pack, and a tooltip that refreshes mid-life — a marked rune is the first thing
  in the world whose *name* changes under the player.
- **A dispel's line of sight** ("Target can not be seen", cliloc 500237),
  unchecked here as it is everywhere else in this engine; ServUO's
  `SummonMaster == from || CheckHSequence` gate; and `IsAnimatedDead`, which
  waits on a necromancy that does not exist here.
- **Barring a cast while paralyzed.** Classic pre-AoS paralysis is move-only,
  which is what is built; whether this shard wants the stricter rule is a
  decision nobody has taken.
- **The Poisoning skill for the deadlier doses.** A Magery-cast dose caps at
  greater; deadly and lethal want the skill to set them.
