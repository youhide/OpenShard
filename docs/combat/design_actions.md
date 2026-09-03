# A blow that takes time

An action in this engine is an **object with an owner, a reason to end and
something that makes it land** — not a deadline with damage on the far side of
it. The raised axe, the drawn bow, the moment it could still be spoiled: each is
a fact the shard holds and announces, rather than a picture the client invents.

This is the model. It is the foundation the interesting things stand on: a
polearm that reaches two tiles, a shot loosed at a run, a rider whose blow is
the riding-through, an archer holding a drawn bow on a doorway, and a fight that
tires the people in it.

> Read [`design_fight_loop.md`](design_fight_loop.md) first: it is the loop this
> model cuts into, and its D7 (a server-sent animation is a one-shot with its own
> clock) is the rule D6 below amends.
> [`evidence/2026-08-27-the-ranged-shot.md`](evidence/2026-08-27-the-ranged-shot.md)
> is the other half — it closed range, ammo and the flying arrow, and left the
> *waiting* untouched by name.

**Status is not here.** What is built and what is open is
[`README.md`](README.md); how it was built, and the deadline it replaced, is
[`evidence/2026-08-27-the-action-phases.md`](evidence/2026-08-27-the-action-phases.md),
whose `Ф`-numbers several comments in the tree cite; what is not built is
[`plans/combat/actions/PLAN.md`](../../plans/combat/actions/PLAN.md).

## The model

Three axes, and keeping them apart is the whole design:

| axis | question | example |
|---|---|---|
| **kind** | what the impact *does* | a blow, a shot that spends a round, a breath |
| **watch** | what *releases* it | nothing (release at once), a target stepping out of cover, a rider passing through |
| **rules** | what the world does to it *in between* | a run slows the draw, a wound spoils it |

**Every action releases on a clock. The watch decides when that clock starts.**
An ordinary swing starts it at the commit; an overwatching archer starts it when
something walks out from behind the wall. There is no variant of an action that
resolves the instant its condition is met — the release is a real interval, always,
and D10 is why.

So a charge is not a kind and not a trigger: it is `Swing` with `Watch::Contact`
and a short release. An archer covering a doorway is `Shot` with
`Watch::TargetInSight` and the same release the bow always had. Neither is a
special case in the tick.

One component, present only while something is happening:

```rust
/// What this combatant is doing right now, and what will make it land.
pub struct CombatAction {
    pub target: Serial,
    pub kind: ActionKind,
    pub phase: Phase,
    pub started_at: WorldTick,
    /// Accumulated while the action runs, spent once at the hit roll.
    pub accuracy: i16,
    /// Fatigue owed at the impact, gathered a tick at a time while holding.
    pub owed_stamina: u16,
}

pub enum ActionKind {
    /// A blow, committed to a reach read from the weapon at the commit.
    Swing { reach: TileReach },
    /// A shot from a wielded ranged weapon. `nocked` is the round it spends at
    /// the loose and `art` what crosses the gap — both frozen off the weapon it
    /// was holding at the commit, so a bow swapped mid-draw changes neither.
    Shot { reach: TileReach, nocked: Graphic, art: Graphic },
    /// An innate ranged attack — a `RangedAttack` component, a breath weapon.
    /// It carries no ammunition, and that is a difference in kind, not a missing
    /// field. `damage` is the other thing a breath does not share with an arrow.
    Breath { reach: TileReach, damage: DamageType, art: Graphic },
}

pub enum Phase {
    /// Drawing toward a held action. The watch cannot release before ready_at.
    Arming { watch: Watch, ready_at: WorldTick, expires_at: WorldTick },
    /// Ready and waiting on the world. `expires_at` is the arm's endurance, not
    /// its timing: an action that is never released ends when it runs out.
    Armed { watch: Watch, expires_at: WorldTick },
    /// Released. The impact lands at this tick, and a rule may still push it.
    Releasing { impact: WorldTick },
}

/// What the world has to do for an armed action to be released.
///
/// Each is a fact the server already computes at a seam it already runs. This
/// is deliberately a closed enum and not a predicate language: a watch that
/// cannot be named here is a watch nobody can cost, and the tick has to be able
/// to answer all of them in bounded time.
pub enum Watch {
    /// The committed target becomes visible again — steps out from behind the
    /// wall, the tree, the door that just opened.
    TargetInSight,
    /// The committed target comes within the action's reach.
    TargetInReach,
    /// Movement carries the attacker through the target's reach, at whatever
    /// speed it was travelling: the joust, the passing blow.
    Contact,
}
```

`TileReach` is today's `RangedRange` (`crates/server/combat/src/weapons.rs`) with
the word *ranged* taken out of it: once a blow has a reach too, the newtype is
about tiles between two fighters and not about which half of the weapon table it
came from. **The rename has not happened**, so a `Swing` commits the constant —
`MELEE_REACH`, the one-tile `RangedRange` beside `MELEE_RANGE` — and the plan
owns the change.

Four verbs run over it, once per tick each. **Built order: sustain, resolve,
commit** — release is folded into sustain until something arms. Commit runs
*last* on purpose, and the reason is a measured one rather than a preference: a
blow that resolves this tick has to open its next gesture in the same tick, or
every single swing starts a tick late and the animation covers one tick less
than the interval it is stretched to. The old code said the same thing by
running its wind-up pass twice, before and after the blow.

**Commit.** No action, engaged, recovery elapsed. Every precondition is tested
*here*: attackable, same facet, within the weapon's reach, sight clear, not
pacified, ammunition present, stamina enough to lift the thing. On success the
round leaves the pack, the opening stamina is spent, the component is inserted, a
standing fighter faces the target, and cover breaks. A shot committed during an
accepted step keeps that step's facing instead. An ordinary attack commits straight
into `Releasing` and its animation goes out with it; an armed one commits into
`Armed` and draws the arm rather than the stroke.

The first armed admission is built for a bow whose already selected target is
inside its committed reach but out of sight: the cut line is not a refusal for
that shot, it is `Armed { watch: TargetInSight }`. The target remains specific —
this does not scan a doorway for any hostile mobile — and the quiver is checked at
the nock but spent only when the released arrow actually flies.

**Sustain.** An action exists and has not landed. The condition rules for its kind
are applied for this tick. A rule may push the impact, add to `accuracy`, add to
`owed_stamina`, or end the action. So may the world: a target that died, logged out
or left the committed reach. **Both phases are sustained the same way** — an armed
fighter is spending, is interruptible, and can be spoiled, which is what stops
"wait for the perfect moment" from being a free option.

**Release.** An `Armed` action whose watch is satisfied this tick becomes
`Releasing { impact: now + release }`, and *this* is where the stroke's animation
and its timing go out. A watch that never fires ends the action at `expires_at`.

**Resolve.** `now >= impact`. The hit roll is made with the accumulated `accuracy`,
the owed fatigue is spent, damage or a miss follows, a `Shot` emits its projectile,
the outcome goes to every watcher, and recovery is scheduled.

`Combat::next_swing` keeps a job, and it is now a narrow one: **when the next
commit may happen.** It is recovery, not a swing. Nothing else reads it.

> **As built, it is still the impact tick.** Today there is no recovery to speak
> of: the next gesture opens the instant the previous blow lands and occupies
> the whole interval, so "when the next commit may happen" and "when the next
> blow is due" are the same number, and `commit_actions` re-pins it to the impact
> exactly as the wind-up pass used to. Splitting the interval into recovery
> *then* action is a real change to how a fight feels and is the plan's.

## Decisions

**D1 — Preconditions are committed at the start, not re-derived at the impact.**
What the fighter promised is frozen into `CombatAction`: the reach, the target, the
round. At the impact only what could have changed is asked — is the target still
alive, still there, still within *the committed* reach. Anything else that fails is
an outcome with a name, never a `continue`.

**D2 — Every action ends, and the end crosses the wire.** `Hit`, `Miss`,
`Interrupted { reason }`. This is the packet a cancelled telegraph needs: without
it the picture keeps playing. See the wire section — it is one new subcommand,
not two.

**D3 — The quiver is asked at the nock and the round is taken at the loose.** Two
things follow, and they are the whole reason to choose this: *"you have no arrows"*
is said at the start of the draw rather than ten seconds later, and an archer who
was interrupted is not silently robbed.

> **Amended once, and this is the mechanism changing rather than the goal.** The
> decision as first written took the round at the nock and handed it back on any
> end but a shot — and named the hazard itself: death, logout and shutdown all
> abort an action, and a plan that forgot one of the three would leak an arrow
> per interruption, forever. It could not be built as written. The one door out
> of a `CombatAction` is `WorldState::end_combat_action`, which lives in `state`,
> and `items::give` is unreachable from there — `items` depends on `state`, not
> the other way about — while `skills`'s Peacemaking and Hiding, which also end
> actions through `disengage`, cannot depend on `combat` either (`combat` calls
> `skills` for the hit roll). Every remaining shape was a mailbox: a component or
> a bus event drained by a later pass, plus an explicit drain in `disconnect`
> before the logout snapshot reads the inventory, or the arrow is eaten anyway.
>
> Asking instead of taking reaches both goals with none of that: the refusal is
> still at the nock, and an interrupted archer is not robbed **because nothing was
> ever taken**. The whole leak class the decision warned about stops existing.
> What it costs is one case — a round that leaves the pack *during* the draw
> (dropped, traded, handed away) — and that case is an end with a name,
> `Interrupted(NoAmmo)`, rather than a silent misfire.

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

> **Amended in one place the decision left open: how often a rule fires.**
> A condition is charged **at most once per action**. Written as it first stood,
> a pushed condition would fire at every step — and a ten-second draw takes
> twenty of them, so a 25% sway would put a running archer's chance at zero for
> crossing a room, and a `Slow` would push the impact out faster than the wait
> brings it closer, leaving a shot that is never taken. So a rule is a fact
> *about the action* — *it ran*, *it was struck*, which is how `Struck` is worded
> here already — and a `ConditionSet` on the component is what remembers. The
> per-tick spender the enum promises is `Drain`, and it is levied by the sustain
> pass against a condition that *holds* rather than pushed at an event.
>
> Two of the enums above are **not built**: `Winded` is a reading of the stamina
> pool, and `Drain` needs the `owed_stamina` field the component does not carry
> yet. The other five conditions and the other three effects are shipping.

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
outcome packet still says when it stopped, and `Crowd` does not decide for itself
that a step cancels a one-shot — that decision is the server's table, which is
where D4 put it. This amends
[`design_fight_loop.md`](design_fight_loop.md)'s D7 rather than contradicting it:
the one-shot machinery is right, its cancellation rule was in the wrong process.

**D7 — Merging `swings` and `volleys` was a later phase, deliberately.** A blow and
a shot differ at the *impact* (a projectile, a round spent), not in the schedule,
so one pass with a `reach` number is the honest end state — and it is also where a
polearm at two tiles becomes one row of data. It is not the foundation, and doing
both at once would have meant debugging the state machine and the merge in the same
change. The two passes are one; the `reach` column is still the plan's.

**D8 — What starts the clock is a separate question from the clock, and the answers
are a closed list.** A rider's blow lands because the horse carried him through; an
archer's because the target stepped out of the doorway; and there will be more of
these than either of us can list today — that is precisely the argument for a
`Watch` the tick can enumerate rather than a general predicate. Three things follow
from making it an enum instead of a callback or a script hook:

- **Every watch is a fact the server already computes.** `TargetInSight` is
  `sight_clear`, which the commit already calls; `TargetInReach` is `in_range`;
  `Contact` is pushed from the step. Adding a watch means naming an existing seam,
  not inventing a new source of truth.
- **The cost stays bounded and is bounded by the right thing.** The sustain pass
  evaluates one watch per *armed action*, and the number of armed actions is the
  number of fighters who chose to arm — not a function of world size. A watch that
  cannot be answered that way does not get added.
- **Movement becomes an input to combat, not an observer of it.** `Contact` fires
  from the step itself, in the seam D5 already uses, with the momentum in hand
  (was it a run, how many tiles were closed), so a charge can be scored by the
  speed that delivered it.

And an armed action needs an endurance: `expires_at` is what stops a couched lance
from being a permanent property of a rider and an overwatch from being free. D9 is
what makes it cost something *before* it runs out.

**D10 — The release is an interval, never an instant, and it is per weapon.** Even
a watch that fires on the perfect tick does not put an arrow in anyone: an armed
action becomes `Releasing` and lands `release` ticks later. Three separate reasons,
and any one of them would be enough:

- **Nothing can be drawn in zero time.** An impact resolved on the tick its
  condition was met gives the client no frames for the loose, and the arrow appears
  out of a standing body — which is the bug this whole model started from, rebuilt
  in a new place.
- **The rules still bite during it.** The release window is the interval in which a
  spoiling wound or a stumble can still take the shot away, so an armed archer is
  fast but not instantaneous, and a defender has something to do about it.
- **It is the number that balances arming at all.** Without it, waiting is strictly
  better than shooting: a held shot would beat an equal opponent's timed one every
  time, for free. With it, arming trades the opening delay for a shorter release
  and pays fatigue for the privilege.

`release` sits on the weapon row beside `reach` and the swing base — a crossbow
takes longer to get away than a bow — and a fighter with no armed capability never
reads it, because their commit goes straight to `Releasing` with the full interval.

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

**D11 — A commit that cannot happen is a state with a name, not a quiet
`continue`.** D1 said this and made it true at exactly one end. At the impact a
failed precondition became an outcome with a reason on the wire; at the *commit*
the pass went on declining in silence, every tick, for as long as the obstacle
lasted. The two ends were never the same rule, and the difference is invisible in
the code — one is a `continue` inside a loop that ends actions, the other a
`continue` inside a loop that starts them.

What that costs is not subtle once seen. An archer whose quarry steps round a
corner produces **nothing**: no packet, no word, no picture, until the corner is
gone. From a player's seat that is a shard that has stopped working, and it was
reported as exactly that. So a refusal is a component while it holds
(`Balked`) and an edge on the wire in both directions — said when it begins,
said when it lifts, silent in between. A fighter held off by a wall costs two
packets for as long as the wall stands, not forty a second.

It is deliberately **not** an outcome and **not** a phase. An outcome is a thing
that happened and fades; a phase belongs to an action that exists. This is a
standing condition of a fighter with no action, and the client holds it with no
clock at all — the shard says when it is over, which is the only thing that can
know.

**D12 — Where a draw ends and an aim begins is the shard's, and it crosses the
wire.** A bar answers *how far along* and cannot answer *how far along what*: a
bow coming up, a bow bending and a bow held at full draw fill the same rectangle.
The four stretches — `Ready`, `Load`, `Aim`, `Release` — are named neutrally
because the same four fit a blow, a shot and a breath; the word each is *drawn* as
belongs to the kind.

The boundaries are an operator setting (`gameplay.action_stages`, keyed by kind)
for the reason the rules table is one: *"an archer spends half the interval drawing
and a third of it holding"* is a shard's choice. Which means the client cannot
compute it. A picture that read *"past 60% is aiming"* off its own percentage would
be stating a fact nobody gave it, and would be wrong on every shard that retuned
the shares — the same invention D6 keeps the client out of. So the server walks the
stages and announces each transition, and **a stage never goes backwards**: a
`Slow` lowers the fraction elapsed, and a fighter who has drawn a bow has not
un-drawn it.

> **Amended in the one place playing it found wrong: `Aim` is not a share of the
> interval.** Three shares and a remainder made *aiming* a stretch of a released
> action — and a released action is not aiming at anything, its impact is coming
> whether or not somebody waits for it. Given a third of the interval, what it
> drew was a bow already bent with nothing happening: a delay with no cause, and
> it was reported as exactly that. **Aiming is holding**, and the only thing on
> this shard that holds is a `Phase::Armed`. So the shares are two and a
> remainder (`ready`, `load`, and the release), the walk returns
> `Ready → Load → Release` and can no longer return `Aim` at all, and `Aim` is
> entered in exactly one place: the sustain pass, for an armed action. Everything
> else about D12 stands — the boundaries are still the shard's, still an operator
> setting, still announced rather than derived. There is one fewer of them, and
> the one that left was the one no fighter was doing.

## The wire

**Four packets.** This was one packet until D8: the beginning already crossed as
`SwingTiming`, which carries a duration, and only the end was missing. But a
charge and a held aim *have no duration*, and the encoding has no room to say so —
a zero in that field already means "forget the timing you were given"
(`crowd.rs:1339`). Announcing an armed action as a zero-length timed one would be
a lie in the one place the client is supposed to read intent.

- **`CombatActionPhase`** — `0xBF` subcommand `OPENSHARD_SUBCOMMANDS + 16`, the
  first free one after `HarvestCompleted` (`feedback.rs:333`). Actor, target, kind,
  and the phase it just entered: *armed*, with the endurance it will hold for, or
  *releasing*, with the milliseconds to the impact. It is sent on the commit and
  again on the release, because those are two things a watcher can see and the
  second is not implied by the first — an armed archer who looses is a different
  picture from an armed archer, and the client is told rather than guessing from
  the arrival of an animation. `SwingTiming` stays exactly as it is: harvesting
  uses it too (`skills/src/handlers/harvest.rs:543`) and it is not combat's to
  repurpose.
- **`CombatActionEnded`** — subcommand `+ 17`. Actor and outcome: `Hit`, `Miss`,
  `Interrupted { reason }`, `Expired`.
- **`CombatActionBalked`** — subcommand `+ 18`, D11's. Actor and one byte:
  either what is in the way or a zero meaning the way is clear. The byte shares
  its numbering with an interruption's reason, which is free — a reason is never
  written as `0`, because that is already the filler an outcome that is not an
  interruption writes. Sent on the edge in both directions and never in between.
- **`CombatActionStage`** — subcommand `+ 19`, D12's. Actor and which of the four
  stretches it just entered — three of which a released action can be in, the
  fourth (`Aim`) reserved for an armed one; see D12's amendment. The byte carries
  all four and always has, so nothing on the wire changed when the share table
  lost one. Deliberately not folded into `CombatActionPhase`,
  which carries the interval a bar is measured against: a stage changes *inside*
  that interval, and re-sending the phase to say so would restart the client's
  clock and reset the very bar the stage annotates.

Stock clients skip unknown extended commands, so there is no compatibility cost and
no existing packet changes shape.

The preparation bar then reads a small state machine rather than inferring one
from a duration: a held indicator while armed, a bar filling through the release,
emptied by an interruption. That is the picture an overwatching archer needs, and
a bar that could only fill was never going to draw it.
