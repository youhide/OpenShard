# Magic: one gate, one table, and effects that outlive the cast

Every spell on this shard goes through **one gate and one table**. The gate is
where mana, reagents, the spellbook and the skill roll are decided; the table is
sixty-four rows of data that say what each spell costs, what it aims at, what it
says, what it looks like and what it does. Nothing about a particular spell is a
branch in the tick.

The crates, in dependency order — `magic` depends on `combat`, which depends on
`skills`, which depends on `items`, all of them on `state`:

| Where | What lives there |
|---|---|
| `magic::spells` | the 64-row table: circle, reagents, target kind, effect archetype, power words, gesture, art |
| `magic` (crate root) | `cast_spell` / `pay_and_roll`, the mana pool and its regen, the stat and behaviour buffs, paralysis, `SpellCast` |
| `magic::resist` | Resisting Spells — the contest and what a resist takes off |
| `magic::dispel` | the dispel roll, off the summon's own difficulty and focus |
| `magic::travel` | Recall/Mark/Gate permission, the nine public moongates |
| `world::tick::spells` | the *sequence*: the wire, the rooting, the cursor, and running the archetype |
| `world::tick::{fields, gates, dispel}` | the three things a spell leaves standing in the world |
| `state::{effect, summon}` | the canonical effect kinds and what each summon is |

**This document is the model.** What is built and what is open is
[`README.md`](README.md); how it was built, family by family, is
[`evidence/2026-08-24-the-magic-phase.md`](evidence/2026-08-24-the-magic-phase.md);
what is not built is
[`plans/combat/spells/PLAN.md`](../../plans/combat/spells/PLAN.md).

## The cast, in order

A cast is two shapes chosen by one operator knob, and the *order of the
refusals* is the load-bearing part of both. Everything above the reveal costs
nothing; everything below it has already given the caster away.

**Free refusals, in `begin_cast`** — the caster is dead; the spell id is past
the eighth circle; **the spellbook does not hold it** (classic UO: you cast what
you scribed); the travel family's own `CheckCast` (criminal, mid-fight,
overloaded, holding something, a region that forbids it); the summoning family's
follower cap; and a cast already in flight. A spell refused here was never begun,
so it reveals nobody.

**The reveal, and the words.** `break_cover` is called the moment the cast
begins — ServUO's `RevealingAction`, whose last line is `DisruptiveAction`, so
one call both unhides a hidden mage and ends a meditating one's trance. The power
words go out in the same breath, through `chat::speak` in `TalkMode::Spell`, so
a mantra reaches exactly whoever would hear the caster talk. They are said
whether the cast then takes or fizzles: a warning that arrives with the fireball
is not a warning.

**Then one of two shapes**, `gameplay.cast_style`:

- **`sphere` (`Walk`)** — the cast resolves in the same instant it is asked. The
  gesture is given no duration, the caster keeps walking, and there is nothing to
  interrupt.
- **`servuo` (`Stop`)** — a `Casting { spell, complete_at }` component roots the
  caster for the circle's delay, and `animate_timed` holds the gesture for
  exactly that span (the length is stated once and the client keeps the cycles
  alive, rather than a second clock restarting the animation). `advance_casts`
  runs each tick: **disturbances first**, so a cast that is both struck and due
  on one tick breaks rather than lands, and `Protection` gets its roll to hold
  concentration before the blow takes it.

**Payment is at resolution, not up front** — Sphere's model, and three knobs
under it: `reagents` (require them at all), `mana_loss_on_fail`, and
`reagent_loss_on_fail`. `pay_and_roll` checks availability, rolls Magery through
the same `roll_skill_band` a mined ore uses, and only then spends. A successful
cast always spends.

**A targeted spell raises its cursor after it is paid for, not before.** The
world remembers why (`TargetPurpose::Spell { spell, success }`), and the aim
arrives a packet later with its reach re-checked server-side. An `Item`-targeted
spell raises the *object* cursor (`0x6C` type 0) so the client itself refuses
bare ground. A creature with no client cannot aim, so its targeted cast simply
lapses — which is the shape of the gap where creature casting will go.

## What a table row carries

`magic::spells::info(spell)` is the whole of what a spell *is*:

- **the circle**, `SpellCircle` — a newtype that cannot hold anything outside
  1..=8 — which sets mana, cast delay and the difficulty band;
- **the reagents**, as item graphics, all-or-nothing;
- **`SpellTarget`** — `SelfCast`, `Mobile`, `Location` or `Item`. It decides
  which cursor goes up and whether one goes up at all. A row whose spell ignores
  its own aim does not ask for one: a cursor whose answer is discarded is a lie
  the moment the row runs;
- **`SpellEffect`**, the archetype the core runs;
- **the power words**, **the gesture** (`CastDirected` or `CastArea` — twenty
  ServUO action ids collapse to two, because the client's own `Anim2.def`
  replaces `203..=245` with group 16 and `260..=269` with 17), and
- **`SpellArt`** — `Landing { sound, visual }` with ServUO's own per-spell ids,
  or `Silent`. Silent is a decision, not a hole: it is for a spell whose art
  belongs to its *effect* rather than its cast (a recall's two ends, a gate's
  pair, a mark's rune), one the reference itself leaves bare (Earthquake, whose
  noise is everybody it hurts), one with two possible outcomes and room in the
  row for one (all three dispels), and one whose effect is not built yet.

`SpellVisual` names the *placement* rule rather than a picture — bolt,
on-target, at-spot, `Lightning` (whose graphic is the client's own), `Unseen`
(a field whose tiles are its own picture).

## The effect archetypes are a closed list

`SpellEffect` is an enum, and that is the design: the core runs the archetypes
the engine can express, and a spell that needs a subsystem nobody has built is
tagged `Unimplemented` — it still casts fully (words, gesture, mana, reagents,
skill, delay, cursor) and then nothing happens. Such a row is `Silent` too, and
grows its art when it grows its effect.

The archetypes: typed `Damage` and `AreaDamage`, `Heal`, `Poison`, `Cure`,
`AreaCure`, `Teleport`, `StatMod`, `BehaviourBuff`, `Field`, `Summon`,
`Paralyze`, `Resurrect`, `Dispel`, `MassDispel`, `DispelField`, and the travel
family.

**Damage never happens here.** Every archetype that hurts goes through
`combat::damage`, the one door all damage passes, carrying a `DamageType` that
the target's `Resistance` for that type cuts. A fireball and a sword swing share
the door, which is why a reflected hit, a poison pulse, a fire field and a script
`op_damage` all obey the same resistances, the same murder attribution and the
same `Frozen`-lifting rule.

**Mana has one door too**, `WorldState::set_mana`: it writes the component and
sends the owner a `0xA2` in the same breath. The four sites that used to mutate
`Mana` in place across three crates go through it. Only a character sheet being
assembled at login writes directly, because there is no client on the other end
of it yet.

## Effects that outlive the cast

The interesting half of magic is what is still true a minute later. Six shapes,
and they share one save path:

| Shape | Component | Ends by |
|---|---|---|
| poison | `Poisoned` | `combat::poison_tick`, pulsing typed damage |
| stat buff/debuff | `StatMods` — a ledger, at most one entry per kind | `magic::expire_buffs` |
| behaviour buff | `BehaviourBuffs` — Protection, Reactive Armor, Night Sight, Magic Reflection | `magic::expire_behaviour_buffs` |
| paralysis | `Frozen { until }` | `magic::expire_frozen`, **or any blow that lands** |
| a field | `Field` on each ground tile | `World::field_tick` |
| a summon | `Summoned` beside `Pet` | `npc::unsummon` — expiry, death or dispel, one exit |

Three rules hold across all six:

- **A ledger, not a recomputation.** A stat buff folds its Magery-scaled offset
  into `Stats` and the caps that hang off them, and `StatMods` remembers exactly
  how to give it back. A recast refreshes its own kind rather than stacking a
  second.
- **What is timed in ticks is saved; what is transient is not.** A `Poisoned`,
  a `StatMods` entry, a `BehaviourBuffs` entry and a `Frozen` become
  `EffectRecord { kind, amount, remaining }` on the character or mobile row and
  come back on login and on boot — a relog cannot wash a debuff off. A buff is
  restored as its *ledger only*, because its shift is already folded into the
  saved stats and re-applying would double it. Fields, gates and casts in flight
  are **excluded from the save sweep**: restored, a half-minute portal is a
  permanent one whose caster no longer exists. A summon is excluded for exactly
  the same reason and it is the sharper case — restored, a five-minute daemon is
  a permanent one standing as somebody's pet against a follower cap nothing will
  ever free.
- **One seam.** `World::effects_of` / `apply_effects` is where every kind goes in
  and out, so a future buff needs no schema change.

## Resisting, and dispelling: two rolls with the same shape

Both are read sites over data, in tenths of a per-cent so the reference's fifths
and halves stay exact.

**`magic::resist`** — ServUO's pre-AoS `Spell.CheckResisted`. The chance is the
better of two readings of Resisting Spells, halved: a flat `resist / 5` floor,
and a contested one weighing the caster's Magery against the circle, which is
what makes an eighth-circle spell land where a first-circle one would not. **A
resist is not a shield**: it takes a quarter off *whatever the spell was going to
do* — damage for the bolts (each victim of an area blast rolls its own), duration
for Paralyze and for the debuff half of the Bless/Curse family. That is why two
spans had to become spans rather than deadlines. Being cast at is also how the
skill trains, and only above `(1 + circle) * 10 + (1 + circle / 6) * 25` points,
so a grandmaster cannot train on first-circle spam.

**`magic::dispel_chance`** — `0.5 + (Magery - difficulty) / (focus * 2)`, with
`difficulty` and `focus` two columns of `state::summon` beside the ones that say
what a summon *is*. The two ends are the whole design: a blade spirit's
`0.0/20.0` means anyone with any Magery sends it away, while a daemon's
`125.0/45.0` sits *above* a grandmaster's entire skill — what is dearest to call
up is dearest to be rid of. Nothing is trained by the roll: Magery was trained by
the cast that carried it, and the creature's difficulty is its class's rather
than something it learnt. The only question a dispel asks is whether the target
carries `Summoned`; nothing that was not summoned can be dispelled, however
magical it looks.

## Travel is a facet change, and there is one door

`WorldState::move_to` is the single door every relocation goes through —
recall, gate, teleport, a staff jump. Five caches remember where a mobile is and
none is compiler-checked: the traveller's own screen, every watcher's, the old
facet's sector grid, `InRegion` (which carries the facet its id belongs to, since
region 3 on two facets compared equal), and the walk sequence. A relocation that
skipped one of the five is the class of bug this door exists to make impossible.

Permission collapses ServUO's `bool[7,24]` matrix onto two region flags,
`no_teleport` and `no_recall`, and the shape that survives is that the kinds are
**directional**: `RecallFrom` is the only permissive row, so a dungeon nobody may
recall *into* is still one you may recall *out* of. Folding both ends into one
call would make every such region a one-way trap.

A recall rune is a graphic plus a `RuneMark { facet, destination }`, and a blank
rune is one *without* the component — there is no `marked` flag to disagree with
a destination that would mean nothing when false. A gate pair carries no link
field: each gate points at the other's tile, so the link *is* the destination and
there are not two halves to keep honest. The nine city moongates carry no
component at all — their destination is derived from where they stand — and they
are placed **without** an obstruction, because a sealed gate is not a worse gate
but one whose walk-in trigger is dead code.

Walking into a gate is **found, not announced**: `tick/gates.rs` reads this
tick's `MobileMoved`. There are two movement paths and a call beside each is one
to forget, and unlike a position scan it cannot miss somebody who steps on and
off inside one batch of commands.

## Where the knobs are

All of it is `[gameplay]` in `openshard.toml`, never a branch on `Era`:
`cast_style`, `reagents`, `mana_loss_on_fail`, `reagent_loss_on_fail`,
`spell_disturb`, `cross_facet_travel`. The era knobs that magic reads through
combat — `combat_era`, `speed_scale_factor`, `expansion` — are the same ones the
weapon tables read, so a shard has one answer to "which UO is this" rather than
two.
