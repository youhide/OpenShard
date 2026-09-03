# Finishing the combat action: fatigue, reach, and the watches

The `CombatAction` object is built and shipping — three passes, four packets,
the condition table, the stage walk and the preparation bar. Three phases of the
original seven are not, and each spends one axis of the model that Ф1 separated.
The model is [`docs/combat/design_actions.md`](../../../docs/combat/design_actions.md);
what is built is [`docs/combat/README.md`](../../../docs/combat/README.md); how it
was built is
[`docs/combat/evidence/2026-08-27-the-action-phases.md`](../../../docs/combat/evidence/2026-08-27-the-action-phases.md).

Nothing here blocks anything else in the domain, and the three are in dependency
order: Ф5's fatigue is what makes Ф7's arming balanceable at all.

## Ф5 — the fight costs something

D9. An opening stamina cost at the commit, a per-tick `Drain` while sustaining,
the owed fatigue spent at the impact, `Winded` as a condition the table can read,
and the regeneration pulse excluding anyone mid-action. This is where holding a
draw becomes a decision rather than a free option.

What it needs that does not exist: the `owed_stamina` field on the component
(Ф1 left it off deliberately, because nothing owed any), and `Winded` as a *held*
condition the sustain pass reads rather than an event a seam pushes — which is
the same second seam the `Mounted` finding asks for.

**Done when:** a fighter who swings without pause runs down; a held bow tires the
archer; and every number in that sentence is an operator setting.

**Take the recovery question with it.** `Combat::next_swing` is documented as
"when the next commit may happen" and is as built re-pinned to the impact, so
there is no recovery span at all. Splitting the interval into recovery *then*
action changes how a fight feels — a fighter would stand still between blows —
and it wants a number in operator settings before anyone writes it. Ф5 is the
natural home because the interval already gains a second meaning here.

## Ф6 — reach as data, and one weapon table

D7's second half. `reach` on every weapon row rather than `MELEE_REACH` in one
function and a `range` column on the ranged rows only. `MELEE_REACH` is the last
hard-coded reach in the crate, and the polearm at two tiles falls exactly on that
seam: with the column, a halberd is content rather than engineering.

Two renames belong in the same change, because doing them separately means
touching every call site twice:

- `RangedRange` → `TileReach`. Once a blow has a reach too, the newtype is about
  tiles between two fighters and not about which half of the weapon table it came
  from.
- `release` joins `reach` and the swing base on the weapon row — D10's per-weapon
  release interval, which Ф7 reads.

Two open questions this phase is the right place to answer, both recorded from
the phases that found them: whether a bow should have a melee mode at all (a
player wielding one beside their target now looses rather than clubbing), and
whether `gameplay.action_speed.shot = 64` should become a corrected `old_speed`
column for archery instead of a percentage over the whole kind.

**Done when:** a weapon's reach is a column an operator can read, `MELEE_REACH`
is gone, and a two-tile polearm is a data row with no code change behind it.

## Ф7 — the rest of the watches

The `TargetInSight` slice is built: a targeted bow arms when sight alone is cut,
pays the ready-plus-load share before it can become armed, holds through movement
by either mobile, and releases over the remaining share when sight clears. Its
endurance is ten seconds.

What is left, and it is one piece of work rather than three:

- **`TargetInReach`** — evaluated per armed action against its own target, the
  same shape `TargetInSight` already has.
- **`Contact`** — pushed from the movement seam with the momentum that delivered
  it, so a charge can be scored by the speed that carried it. D5's seam already
  calls into combat at the step.
- **The release delay as a weapon column** — Ф6's, and this phase's reason to
  wait for it.
- **The armed bar's missing number.** `expires_at` crosses the wire and the
  picture spends none of it: a bow about to give out is drawn exactly like one
  just armed. Held rather than filling is the right shape; *nearly out* is a real
  thing to say and nothing says it.

**Done when:** an archer can arm a shot at a doorway and spend it on whoever
steps out, a rider at a gallop lands a blow by passing through, and neither lands
the instant its watch fires.

**A fourth watch is deliberately not in this phase.** A doorway watched with no
target chosen yet — *"anyone hostile enters this square"* — is the first watch
that cannot be answered from the armed action alone, so it needs an index of
armed squares. That index is a design rather than a variant, and it wants writing
before it is built.

## Out of scope, and named so it is not re-proposed

- **What a mount does beyond delivering the charge.** `Riding` exists and Ф7
  reads it, but a mount's effect on reach, on damage, and on being dismounted is
  not decided here.
- **Special moves and combos.** The wrestling opener and combo keep working
  through the action object untouched; giving them phases of their own is a
  separate design.
- **Casting.** Spells have their own timing (`Casting`, `advance_casts`) and are
  not folded in. They are the obvious second customer for `CombatAction` — a cast
  is a timed action with a `Struck → Break` rule and a mana cost, which is this
  model with two words changed — and the day a third appears is the day the
  component should be renamed and moved out of `combat`.
