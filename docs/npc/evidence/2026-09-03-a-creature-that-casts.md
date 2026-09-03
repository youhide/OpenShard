# A creature that casts

The magic domain stopped being one-directional on 2026-09-03. This is what C1 of
[`plans/npc/creature_casting/PLAN.md`](../../../plans/npc/creature_casting/PLAN.md)
turned out to be, the two decisions it had to take first, and the three things it
deliberately left for C2 and C3.

## What was actually missing

Not a spell system. `resolve_cast` and `apply_spell_effect` were already
caster-agnostic; `begin_cast` was the client seam, reading a connection out of
`World::state.players`. What stopped a creature was smaller and more specific
than "no creature spell path", and it was four things:

1. **Nothing to spend and nothing to spend it on.** No `Mana` on a creature, no
   spell list anywhere on the mobile.
2. **The spellbook gate.** `caster_has_spell` looks for a book in a backpack, and
   a creature has neither.
3. **The cursor.** A targeted spell raises one and waits for a client's answer.
   A creature has no client, so — in the code's own words at the time — "its
   targeted cast simply lapses".
4. **The reagent list.** A creature has no pack, so a shard running with reagents
   on would have fizzled every cast.

Each is one rule, and three of the four have a reference behind them: ServUO's
`Spell.ConsumeReagents` returns true for anything that is not a player, and its
`SayMantra` ends with the same test.

## The two decisions

**A creature's spells are spawn data, not a body default.** The alternative was
the shape `creature_name` and `creature_base_sound` already have — a table keyed
by `body`, defaulted in core, overridable by the pack. It was rejected twice
over. A repertoire is not an *identity* the way a name and a howl are: two liches
out of the same body are the same creature called the same thing, and a shard
that wants one to throw flamestrikes and the other magic arrows is authoring
content. And a fourth body-keyed file would grow exactly the drift this domain's
README row 15 is already about. So `mana` and `spells` sit in
`data/spawns.json` beside `ranged` and `sight`, and `build.rs` refuses a creature
that has one without the other — a repertoire with nothing to spend and a pool
with nothing to spend it on are both a caster that never casts.

**The aim rides on `Casting`, beside the scroll.** A player's targeted cast asks
at the *end* of the delay, through the cursor; that is why a mage can start a
fireball before choosing whom to burn. A caster with no client has to aim first,
and the aim then has to survive a delay during which the mark can die — which is
`Casting::scroll`'s argument exactly, made once already for exactly this reason.
So `Casting` gained an `aim`, `None` for a person, and `resolve_cast` grew one
branch: a cast with an aim lands through the same two steps the cursor's answer
takes. Those two steps became `World::land_cast`, which is now the one place a
cast lands and which the cursor's answer calls too — the alternative was a second
copy of "announce it, then apply it", and a second copy is how the announcement
and the effect come to disagree about whether a spell happened.

## What a creature's cast does *not* know

Nothing below `start_cast` was told which kind of caster it has. The spell table,
the roll, the resist, the disturbance, the effect art and the buffs are the ones
that already existed, unchanged. Two rules do ask "is there a person behind this
mobile" — the mantra and the reagents — and both are ServUO's own tests rather
than this engine's inventions. The recovery a creature is held off by is derived
from the spell's own cast delay (`magic::cast_recovery_ticks`), not from a
per-creature knob, so an eighth-circle throw is slower to follow than a magic
arrow because that is a fact about the spell.

## One clause borrowed from C3

C3 owns the cadence, but leaving all of it out would have shipped a visible
defect: a creature that started a cast and then walked out of it on its next
beat, with two systems each believing they owned the mobile for the second in
between. So one clause came early — a creature holding a `Casting` stands — on
the same component and read in the same place a player's rooting is. The rest of
C3 is untouched: LOD may still doze a caster mid-cast, and the determinism claim
has no test of its own.

## What was authored

Three creatures, with ServUO's own numbers: the skeletal mage (200 mana, Magery
57.5, magic arrow / harm / fireball), the lich (290, 75.0, and up to energy
bolt), and the lich lord (475, 95.0, through flamestrike). Their mana follows
ServUO's rule that a creature's pool is its intelligence, and their Magery is the
midpoint of the reference's own band.

## What is still not right about it

- **It throws the strongest thing it can afford**, and that is the whole of the
  choice. Nothing heals, buffs, curses, or teleports away: a healing dragon is
  still impossible and a lich at one hit point fights on rather than escaping.
  Categories are C2, and the plan is explicit that the knob to avoid is a
  per-spell weight table in the spawn data.
- **A self-cast or a location spell in a repertoire is skipped**, silently. It is
  the right answer for now — cast at a foe, a self-cast would land on the caster
  — but it means a pack can author a spell that never fires and get no word about
  it. When categories land, the skip becomes a category with nothing in it, which
  is something the data can be checked against.
- **Magery is not implied by a repertoire.** A creature authored with spells and
  no skill sheet casts at the bottom of the band and fizzles most of what it
  throws. That is the same gate a blow goes through and it is deliberate, but
  nothing in the build refuses the combination the way it refuses mana without
  spells.
