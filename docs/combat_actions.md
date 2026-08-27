# A blow that takes time

A fighter in this engine does not *do* anything. It has a deadline, and when the
deadline arrives damage appears. Everything a player could read as an intention —
the raised axe, the drawn bow, the moment it could still be spoiled — is either
absent or is a picture with no fact behind it.

This is the plan that gives the started action a name, an owner, a reason to end
and something that makes it land. It is the foundation the interesting things stand
on: a polearm that reaches two tiles, a shot loosed at a run, a rider whose blow is
the riding-through, an archer holding a drawn bow on a doorway, and a fight that
tires the people in it. None of those is built here. All of them are unbuildable
until this is.

> Read [`combat.md`](combat.md) first: it is the loop this plan cuts into, and its
> D7 (a server-sent animation is a one-shot with its own clock) is the rule this
> document has to amend. [`archery.md`](archery.md) is the other half — it closed
> range, ammo and the flying arrow, and left the *waiting* untouched by name.

## What the server knows today

Half of it, for half the weapons.

**Melee has a wind-up, and it is real.** `combat::prepare_swings`
(`crates/server/combat/src/lib.rs:893`) runs before and after `swings` in the tick
(`crates/server/world/src/tick.rs:687-690`). For every engaged, reachable
combatant it moves the impact to the end of a full interval, faces the target,
breaks cover, and calls `WorldState::animate_timed`
(`crates/server/state/src/runtime.rs:2531`). That emits the OpenShard extension
`SwingTiming` — `0xBF` subcommand `0xE00B`,
`crates/common/protocol/src/feedback.rs:73` — carrying the whole interval in
milliseconds, because the stock animation packets can only express an eight-bit
per-frame delay. The client keeps it in `Crowd::swing_timings` and loops complete
action cycles across it (`crates/client/app/src/crowd.rs:1249`). So *"a blow began
and will last this long"* already crosses the wire, and already draws.

**Ranged has none, by an explicit `continue`.** `prepare_swings` opts out at
`lib.rs:905-917`:

> A ranged attacker — innate or a wielded bow/crossbow — has no melee wind-up: it
> stands and looses on `volleys`'s own beat instead.

Every gate on a shot — reach, line of sight, ammunition — is tested inside
`volleys` (`lib.rs:624`) on the single tick the arrow leaves. That is the ten
seconds of standing still, exactly.

## The root cause, in one sentence

**The server owns a deadline, not an action.**

The only authoritative value is the scalar `next_swing: WorldTick` inside `Combat`
(`crates/server/state/src/components.rs:2116`). `SwingWindup` (`components.rs:2352`)
is not a process — it is a marker meaning *"this one has already been animated"*,
read once at `lib.rs:818` so `swings` does not animate twice. Four consequences,
each visible in the code:

- **Preconditions are tested twice, at two different moments, and the second test
  has no way to say so.** `prepare_swings` checks reach and sight to start the
  gesture; `swings` checks them again at impact and on failure does a bare
  `continue` (`lib.rs:804`). The player watched a full swing and got neither a blow
  nor a reason.
- **The action has no end.** A target that dies mid-swing is cleared, and the
  stretched animation on the client runs out its promised duration over an empty
  tile. A telegraph that cannot be cancelled is worse than none.
- **The two ends disagree about interruption.** The client cancels a one-shot when
  the body steps (`combat.md`'s D7); the server has never heard of the step and
  lands the blow anyway.
- **Melee and ranged are two admission rules in two tick functions**, so *reach* is
  a constant in one (`MELEE_RANGE`, `melee_reachable` at `lib.rs:1074`) and a
  number in the other (`WeaponData::range`, ranged rows only). A polearm at two
  tiles falls precisely on that seam.

## The model

Three axes, and keeping them apart is the whole design:

| axis | question | example |
|---|---|---|
| **kind** | what the impact *does* | a blow, a shot that spends a round, a breath |
| **trigger** | *when* the impact happens | a clock, riding through the target, a held aim |
| **rules** | what the world does to it *in between* | a run slows the draw, a wound spoils it |

A lance charge is `Swing` with an `OnPass` trigger. Overwatch is `Shot` with a
`Held` one. Neither needs a new kind, and neither is a special case in the tick —
which is what makes them cheap once the axes are separate, and unbuildable while
they are fused into one deadline.

One component, present only while something is happening:

```rust
/// What this combatant is doing right now, and what will make it land.
pub struct CombatAction {
    pub target: Serial,
    pub kind: ActionKind,
    pub trigger: Trigger,
    pub started_at: WorldTick,
    /// Accumulated while the action runs, spent once at the hit roll.
    pub accuracy: i16,
    /// Fatigue owed at the impact, gathered a tick at a time while holding.
    pub owed_stamina: u16,
}

pub enum ActionKind {
    /// A blow, committed to a reach read from the weapon at the commit.
    Swing { reach: TileReach },
    /// A shot from a wielded ranged weapon; the round is already out of the pack.
    Shot { reach: TileReach, nocked: Graphic },
    /// An innate ranged attack — a `RangedAttack` component, a breath weapon.
    /// It carries no ammunition, and that is a difference in kind, not a missing
    /// field.
    Breath { reach: TileReach },
}

pub enum Trigger {
    /// The ordinary swing and the ordinary draw. A rule may push the tick out.
    AtTick { completes_at: WorldTick },
    /// The charge: it lands when movement carries the attacker through the
    /// committed target's reach, and it carries the momentum that did it.
    OnPass { since: WorldTick, expires_at: WorldTick },
    /// The held aim. It lands when the target re-enters reach and sight, and
    /// never on its own. `expires_at` is the arm's endurance, not its timing.
    Held { expires_at: WorldTick },
}
```

`TileReach` is today's `RangedRange` (`crates/server/combat/src/weapons.rs`) with
the word *ranged* taken out of it: once a blow has a reach too, the newtype is
about tiles between two fighters and not about which half of the weapon table it
came from. Ф5 is where it is renamed; until then `Swing` commits the constant.

Three verbs run over it, once per tick, in this order:

**Commit.** No action, engaged, recovery elapsed. Every precondition is tested
*here*: attackable, same facet, within the weapon's reach, sight clear, not
pacified, ammunition present, stamina enough to lift the thing. On success the
round leaves the pack, the opening stamina is spent, the component is inserted, the
fighter faces the target, cover breaks, and the animation goes out.

**Sustain.** An action exists and has not fired. The condition rules for its kind
are applied for this tick (below). A rule may push a clock, add to `accuracy`, add
to `owed_stamina`, or end the action. So may the world: a target that died, logged
out or left the committed reach. A held or charging action is *sustained the same
way* — this is where the cost of standing at full draw accrues.

**Resolve.** The trigger fired: the tick arrived, movement carried the attacker
through, or the held target came back into reach. The hit roll is made with the
accumulated `accuracy`, the owed fatigue is spent, damage or a miss follows, a
`Shot` emits its projectile, the outcome goes to every watcher, and recovery is
scheduled.

`Combat::next_swing` keeps a job, and it is now a narrow one: **when the next
commit may happen.** It is recovery, not a swing. Nothing else reads it.

## Decisions

**D1 — Preconditions are committed at the start, not re-derived at the impact.**
What the fighter promised is frozen into `CombatAction`: the reach, the target, the
round. At the impact only what could have changed is asked — is the target still
alive, still there, still within *the committed* reach. Anything else that fails is
an outcome with a name, never a `continue`.

**D2 — Every action ends, and the end crosses the wire.** `Hit`, `Miss`,
`Interrupted { reason }`. This is the packet the client is missing today: without
it a cancelled telegraph keeps playing. See the wire section — it is one new
subcommand, not two.

**D3 — The arrow is taken at the nock and returned on any end but a shot.** Two
things follow, and they are the whole reason to choose this: *"you have no arrows"*
is said at the start of the draw rather than ten seconds later, and an archer who
was interrupted is not silently robbed. The return goes into the backpack through
`items::give`, the door the beggar's gold already uses. Death, logout and shutdown
all abort the action, so all three return the round; a plan that forgot one of the
three would leak an arrow per interruption, forever.

**D4 — Interruption is a declared table, not a flag on the weapon.** A boolean
*"movement breaks this"* cannot express shooting on the move at a penalty, which is
a thing this shard wants. So a rule is a pair — a condition the server already
knows, and an effect on the running action:

```rust
pub enum ActorCondition {
    /// This tick's step carried `Facing::running`.
    Running,
    /// A step that was not a run.
    Walking,
    /// The fighter is on a mount (`Riding`).
    Mounted,
    /// Took damage since the action began.
    Struck,
    /// Line of sight to the committed target is gone this tick.
    Blinded,
    /// Below `vitals::WINDED_PERCENT` of the stamina pool — the threshold that
    /// already makes a step cost extra (`combat/src/vitals.rs:44`).
    Winded,
}

pub enum ActionEffect {
    /// The action ends as `Interrupted`.
    Break,
    /// The impact is pushed out by this percentage of the remaining time.
    Slow { percent: u16 },
    /// Taken off the hit roll when the action resolves.
    Sway { penalty: i16 },
    /// Fatigue per tick while the condition holds, owed at the impact.
    Drain { stamina: u16 },
}
```

The table is keyed by `ActionKind` and lives in operator settings beside
`combat_era` and `speed_scale_factor` (`crates/common/config/src/lib.rs:92`), so
*"an archer may fire at a walk, sways at a run, and steadies on horseback"* is
configuration and not a branch. A shard that wants the classic rule writes
`Break` on the same row. **No code decides this**, which is the point.

**D5 — Conditions are pushed in from where they are already known, never polled.**
Movement is the exemplar: `motion.rs` already has `request.facing.running` and the
`Riding` lookup in hand at the step (`tick/motion.rs:113-121,194`) and already
calls into combat there — `record_wrestling_step` (`lib.rs:1327`) is the precedent,
line for line. `Struck` arrives at `damage`, the one door all damage passes.
Reading `Heading` in the sustain pass instead would be wrong and subtly so: the run
bit persists in the facing after the step, so a fighter who ran once would sway
forever.

**D6 — The animation stays a picture, and the picture follows the action.** The
client is not given a second authority. `SwingTiming` still says how long, the
outcome packet still says when it stopped, and `Crowd` stops deciding for itself
that a step cancels a one-shot — that decision moves to the server's table, which is
where D4 put it. This amends `combat.md`'s D7 rather than contradicting it: the
one-shot machinery is right, its cancellation rule was in the wrong process.

**D7 — Merging `swings` and `volleys` is a later phase, deliberately.** A blow and
a shot differ at the *impact* (a projectile, a round spent), not in the schedule,
so one pass with a `reach` number is the honest end state — and it is also where a
polearm at two tiles becomes one row of data. It is not the foundation, and doing
both at once would mean debugging the state machine and the merge in the same
change.

**D8 — The impact is not always a clock, and that is a variant rather than a
flag.** A mounted charge lands when the horse carries its rider through the target,
at whatever moment that happens; an overwatching archer lands when something walks
into the doorway. Neither has a duration, so neither can be expressed as a deadline
that the world is allowed to nudge — they are a *different question about when*.
Making `Trigger` an enum keeps one sustain loop for all three; making
`completes_at` an `Option` instead would have meant every reader asking "and what
does absent mean here", which is the case `CLAUDE.local.md`'s `Option` rule exists
to refuse.

Two things follow that the ordinary path never needed. **Movement becomes an input
to combat, not an observer of it** — `OnPass` fires from the step itself, in the
same seam D5 already uses, with the momentum in hand (was it a run, how many tiles
were closed) so a charge can be scored by the speed that delivered it. And **an
untriggered action needs an endurance**: `expires_at` on both `OnPass` and `Held`
is what stops a couched lance from being a permanent property of a rider and an
overwatch from being free. It is the arm's endurance, and D9 is what makes it cost
something before it runs out.

**D9 — Stamina is spent by combat, in the module that already spends it.**
`combat::vitals` owns the step cost, the `Riding` and overload branches, the winded
threshold and both regeneration pulses (`crates/server/combat/src/vitals.rs`).
An action's opening cost and its per-tick `Drain` go there, beside
`spend_step_stamina`, as one more named spender — not into the tick, and not into a
second pool.

One interaction has to be decided here or the numbers become a fight between two
constants nobody can tune: **`regen_stamina` restores a point every 1.5 seconds to
everything below full** (`vitals.rs:110`), so a held draw draining a point a second
would net-drain a third of one. A mobile with a `CombatAction` in sustain is
therefore **excluded from the regeneration pulse** — holding a bow at full draw
does not rest you, which is both the honest physical answer and the one that makes
`Drain` mean what its number says.

## The wire

**Two new packets, and the second one is why the first is needed.** This was one
packet until D8: the beginning already crossed as `SwingTiming`, which carries a
duration, and only the end was missing. But a charge and a held aim *have no
duration*, and the encoding has no room to say so — a zero in that field already
means "forget the timing you were given" (`crowd.rs:1339`). Announcing an armed
action as a zero-length timed one would be a lie in the one place the client is
supposed to read intent.

- **`CombatActionBegan`** — `0xBF` subcommand `OPENSHARD_SUBCOMMANDS + 16`, the
  first free one after `HarvestCompleted` (`feedback.rs:333`). Actor, target, kind,
  and *how it will land*: a duration in milliseconds, or the fact that it is armed
  and waiting. `SwingTiming` stays exactly as it is — harvesting uses it too
  (`skills/src/handlers/harvest.rs:543`) and it is not combat's to repurpose.
- **`CombatActionEnded`** — subcommand `+ 17`. Actor and outcome: `Hit`, `Miss`,
  `Interrupted { reason }`.

Stock clients skip unknown extended commands, so there is no compatibility cost and
no existing packet changes shape.

The player's own preparation bar (Ф4) then reads a pair rather than inferring one:
it fills between the two packets when the action is timed, and shows a held state
when it is armed — which is the picture an overwatching archer needs and a filling
bar could never draw.

## The phases

Ф1–Ф4 are the foundation and are ordered by what a player can see. Ф5–Ф7 each
spend one axis of the model, and every one of them is a phase rather than a
feature only because Ф1 separated the axes.

**Ф1 — the object.** `CombatAction`, the commit/sustain/resolve split, and both
new packets — `Trigger::AtTick` alone, but the enum exists from the first commit
so the wire never has to be revised for the other two. Melee only; behaviour
deliberately unchanged except that the
silent `continue` becomes a named interruption and the client stops an animation it
would have run out. `prepare_swings` and `SwingWindup` are retired into it — the
marker becomes a phase of a real object. *Done when:* a target that dies mid-swing
stops its attacker's animation on the spot; a swing that loses reach says so;
nothing else about a fight looks different.

**Ф2 — the bow draws.** `Shot` commits at the start of the interval: reach, sight
and the nocked round tested there, animation and timing sent there, the arrow loosed
at completion. The visible fix — the archer no longer stands for ten seconds and
then teleports an arrow. *Done when:* an empty quiver refuses at the nock, an
interrupted draw returns its arrow, and a bow at ten tiles is a body drawing a bow
for the whole interval.

**Ф3 — the rules table.** D4 and D5: conditions pushed in from movement and damage,
the effect table in operator settings, defaults chosen so that walking is free,
running sways, and a mount is neutral. *Done when:* shooting on the move works and
its penalty is a config line an operator can change without a rebuild.

**Ф4 — the preparation bar.** The client draws its own pending action, off the
pair from Ф1: filling between `Began` and `Ended` when the action is timed, held
when it is armed, emptied by an interruption.

**Ф5 — the fight costs something.** D9: an opening stamina cost at the commit, a
per-tick `Drain` while sustaining, the owed fatigue spent at the impact, `Winded`
as a condition the table can read, and the regeneration pulse excluding anyone
mid-action. This is where holding a draw becomes a decision rather than a free
option, and it is a precondition for Ф7 being balanceable at all. *Done when:* a
fighter who swings without pause runs down; a held bow tires the archer; and every
number in that sentence is an operator setting.

**Ф6 — reach as data, and one pass.** D7: `reach` on every weapon row rather than
`MELEE_RANGE` in one function and `range` in the other; `swings` and `volleys`
become one. The polearm at two tiles is then a number, and the halberd is content
rather than engineering.

**Ф7 — triggers that are not a clock.** D8's other two variants, and they are one
piece of work rather than two. `OnPass` fires from the movement seam and scores the
charge by the momentum that delivered it — the mounted joust, where the hit is the
riding-through. `Held` fires from the sustain pass when the committed target
returns to reach and sight — overwatch, the drawn bow on a doorway. Both need
`expires_at` honoured and both need Ф5's drain, or an armed fighter is a fighter
who has paid nothing. *Done when:* a rider at a gallop lands a blow by passing, and
an archer can arm a shot at a door and spend it on whatever comes through.

## What this does not cover

- **What a mount does beyond delivering the charge.** `Riding` exists and Ф7 reads
  it, but a mount's effect on reach, on damage, and on being dismounted is not
  decided here.
- **Special moves and combos.** The wrestling opener and combo (`lib.rs:1355-1400`)
  keep working through the new object untouched; giving them phases of their own is
  a separate design.
- **Casting.** Spells have their own timing today and are not folded in here. They
  are the obvious second customer for `CombatAction` — a cast is a timed action with
  a `Struck → Break` rule and a mana cost, which is this model with two words
  changed — and the day a third appears is the day the component should be renamed
  and moved out of combat.
