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

> **Both paragraphs are the record of what was here, not of what is.** Ф1 retired
> `prepare_swings` into `commit_actions`, and Ф2 retired `volleys` into the same
> three passes — there is no ranged half of the tick any more. They are left
> standing because the phases below are read against them.

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
came from. Ф6 is where it is renamed; until then `Swing` commits the constant —
which as built is `MELEE_REACH`, the one-tile `RangedRange` beside `MELEE_RANGE`.

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
round leaves the pack, the opening stamina is spent, the component is inserted, the
fighter faces the target, and cover breaks. An ordinary attack commits straight
into `Releasing` and its animation goes out with it; an armed one commits into
`Armed` and draws the arm rather than the stroke.

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

> **As built, it is still the impact tick.** Ф1 changed nothing about the
> schedule, and today there is no recovery to speak of: the next gesture opens
> the instant the previous blow lands and occupies the whole interval, so
> "when the next commit may happen" and "when the next blow is due" are the same
> number, and `commit_actions` re-pins it to the impact exactly as the wind-up
> pass used to. Splitting the interval into recovery *then* action is a real
> change to how a fight feels and is nobody's phase yet — see the backlog.

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

**D3 — The quiver is asked at the nock and the round is taken at the loose.** Two
things follow, and they are the whole reason to choose this: *"you have no arrows"*
is said at the start of the draw rather than ten seconds later, and an archer who
was interrupted is not silently robbed.

> **Amended at Ф2, and this is the mechanism changing rather than the goal.** The
> decision as written took the round at the nock and handed it back on any end but
> a shot — and named the hazard itself: death, logout and shutdown all abort an
> action, and a plan that forgot one of the three would leak an arrow per
> interruption, forever. It could not be built as written. The one door out of a
> `CombatAction` is `WorldState::end_combat_action`, which lives in `state`, and
> `items::give` is unreachable from there — `items` depends on `state`, not the
> other way about — while `skills`'s Peacemaking and Hiding, which also end
> actions through `disengage`, cannot depend on `combat` either (`combat` calls
> `skills` for the hit roll). Every remaining shape was a mailbox: a component or
> a bus event drained by a later pass, plus an explicit drain in `disconnect`
> before the logout snapshot reads the inventory, or the arrow is eaten anyway.
>
> Asking instead of taking reaches both goals with none of that: the refusal is
> still at the nock, and an interrupted archer is not robbed **because nothing was
> ever taken**. The whole leak class the decision warned about stops existing.
> What it costs is one case — a round that leaves the pack *during* the draw
> (dropped, traded, handed away) — and that case is now an end with a name,
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

> **Amended at Ф3, in one place the decision left open: how often a rule fires.**
> A condition is charged **at most once per action**. Written as it stands, a
> pushed condition would fire at every step — and a ten-second draw takes twenty
> of them, so a 25% sway would put a running archer's chance at zero for crossing
> a room, and a `Slow` would push the impact out faster than the wait brings it
> closer, leaving a shot that is never taken. So a rule is a fact *about the
> action* — *it ran*, *it was struck*, which is how `Struck` is worded here
> already — and a `ConditionSet` on the component is what remembers. The
> per-tick spender the enum promises is `Drain`, and it is levied by the sustain
> pass against a condition that *holds* rather than pushed at an event, which is
> the difference that makes Ф5 the phase that can build it.
>
> Two of the enums above are also Ф5's rather than Ф3's, and for the same
> reason: `Winded` is a reading of the stamina pool, and `Drain` needs the
> `owed_stamina` field the component does not carry yet. Ф3 shipped the other
> five conditions and the other three effects.

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
  out of a standing body — which is the bug this whole plan started from, rebuilt
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

## The wire

**Two new packets, and the second one is why the first is needed.** This was one
packet until D8: the beginning already crossed as `SwingTiming`, which carries a
duration, and only the end was missing. But a charge and a held aim *have no
duration*, and the encoding has no room to say so — a zero in that field already
means "forget the timing you were given" (`crowd.rs:1339`). Announcing an armed
action as a zero-length timed one would be a lie in the one place the client is
supposed to read intent.

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

Stock clients skip unknown extended commands, so there is no compatibility cost and
no existing packet changes shape.

The player's own preparation bar (Ф4) then reads a small state machine rather than
inferring one from a duration: a held indicator while armed, a bar filling through
the release, emptied by an interruption. That is the picture an overwatching archer
needs, and a bar that could only fill was never going to draw it.

## The phases

Ф1–Ф4 are the foundation and are ordered by what a player can see. Ф5–Ф7 each
spend one axis of the model, and every one of them is a phase rather than a
feature only because Ф1 separated the axes.

**Ф1 — the object. ✅ Built.** `CombatAction`, the four verbs, and both new
packets. Only `Phase::Releasing` is reachable — nothing arms yet — but the phase
enum and the packet that carries it exist from the first commit, so the wire is
never revised for Ф7. Melee only; behaviour deliberately unchanged except that the
silent `continue` becomes a named interruption and the client stops an animation it
would have run out. `prepare_swings` and `SwingWindup` are retired into it — the
marker becomes a phase of a real object. *Done when:* a target that dies mid-swing
stops its attacker's animation on the spot; a swing that loses reach says so;
nothing else about a fight looks different.

What landed, and the four things worth knowing before reading the code:

- **The three passes are `commit_actions`, `sustain_actions`, `resolve_actions`**
  in `crates/server/combat/src/lib.rs`, called from the tick in the order argued
  above. `swings` and `prepare_swings` are gone; `volleys` is untouched, so a bow
  is still Ф2's problem.
- **`obstruction` is one function now.** Facet, reach and live sight were three
  lines copied into the wind-up pass and into the impact; they are one call
  returning an `InterruptReason`, read at the commit and again every tick the
  action runs. That is where "the second test has no way to say so" was fixed.
- **A concealed opener is an *untelegraphed* action**, not an exception to the
  model: `CombatAction::telegraphed` is false, no wind-up is drawn and no phase
  packet goes out (a wind-up would break cover before the blow), and the stroke
  is animated at the impact. The one behavioural change it brings is that such a
  blow lands on the tick *after* the attack command rather than on it, because
  commit runs last.
- **The end has one door**, `WorldState::end_combat_action`, on the state crate
  rather than in combat — death, Peacemaking, logout and a pet being called off
  all end a fight and none of them can depend on the crate that owns swinging.
  `disengage` goes through it, which is what makes "every action ends" true
  rather than aspirational.

And two known gaps left standing on purpose: `owed_stamina` is not on the
component (nothing owes any until Ф5), and the release verb exists only as the
`Armed` expiry inside sustain, because a watch nobody can satisfy is a stub
rather than a verb. Ф7 is where it becomes one.

**Ф2 — the bow draws. ✅ Built.** `Shot` commits at the start of the interval:
reach, sight and the nocked round tested there, animation and timing sent there,
the arrow loosed at completion. The visible fix — the archer no longer stands for
ten seconds and then teleports an arrow. *Done when:* an empty quiver refuses at
the nock, an interrupted draw costs its archer no arrow, and a bow at ten tiles is
a body drawing a bow for the whole interval.

What landed, and the four things worth knowing before reading the code:

- **`volleys` is gone.** There is one schedule now and three impacts. `ActionKind`
  grew `Shot { reach, nocked, art }` and `Breath { reach, damage, art }`, and the
  three passes ask it four questions — `reach`, `flight`, `round`, `damage_type` —
  where they used to ask which pass they were in. A blow, an arrow and a breath
  differ at `resolve_actions` and nowhere else.
- **A shot fires at any distance inside its reach, arm's length included.** The
  old refusal below `MELEE_RANGE` existed only because the melee pass would
  otherwise strike in the same beat, and there is no melee pass any more; ServUO
  puts no floor under a bow either. What that *removes* is a ranged attacker
  clubbing at point blank, which nothing on this shard ever wanted — an archer
  brain kites out of that tile rather than standing in it.
- **A commit does not turn a shooter toward its mark.** A blow is delivered by the
  body, so the body turns; a shot is delivered down a line. The rule is not
  cosmetic: a step in a direction a mobile is not facing *turns* it instead of
  moving it, so re-aiming an archer at every nock spends the beat it was going to
  escape with, and a kiting brain that beats no faster than the shard re-aims it
  never opens the gap at all. `spawn_archer` in the tick tests already carried
  that finding from the other end, and the first build of this phase reproduced
  it exactly.
- **A shot is announced whichever way the roll goes.** The bolt flew and twanged
  before anyone could know whether it would land, so the flight and the sound go
  out ahead of the hit roll; a blow is the other way about — the thwack *is* the
  sound of landing, and a whiff has a whistle of its own. Wrestling's chain is
  melee's alone: an arrow neither continues one nor breaks one.

And one behaviour that changed on the way past: a pacified archer stops shooting.
`volleys` never read `Pacified` — only the melee pass did — so a bard's calm
silenced swordsmen and left bowmen firing.

**Ф3 — the rules table. ✅ Built.** D4 and D5: conditions pushed in from movement
and damage, the effect table in operator settings, defaults chosen so that walking
is free, running sways, and a mount is neutral. *Done when:* shooting on the move
works and its penalty is a config line an operator can change without a rebuild.

What landed, and the four things worth knowing before reading the code:

- **The table is `crates/server/state/src/action_rules.rs`**, reached through
  `Gameplay::action_rules` like every other operator knob, and written by an
  operator as `[gameplay.action_rules.<kind>]` with the effect's own name:
  `running = { sway = { penalty = 25 } }`, `struck = "break"`. **A row an
  operator writes is the whole row** — a condition left out of it is *no rule*,
  not the shipped default quietly merged back in, because a table that reads one
  way in the file and runs another is the `..Default::default()` hazard wearing
  a different hat.
- **A condition is charged once per action, and this is a decision D4 did not
  make.** A ten-second draw takes twenty steps; a sway charged per step would
  put a running archer's chance at zero for crossing a room, and a `Slow`
  charged per step would push the impact away faster than the wait brings it
  closer, so the shot would never be taken at all. A rule is therefore a *fact
  about the action* — it ran, it was struck, which is how `Struck` was worded to
  begin with — remembered in a `ConditionSet` on the component. The per-tick
  spender in the model is `Drain`, which the sustain pass levies against a held
  condition rather than an event, and it is Ф5's.
- **A cut line is now a row rather than a verdict.** `Blinded` is the one
  condition the sustain pass computes for itself, because losing sight is not an
  event anything pushes; the shipped row breaks the action, which is exactly
  what the pass did with a bare `NoLineOfSight` before there was a table to
  route it through. The other two refusals — another facet, outside the
  committed reach — stay verdicts, because no rule can put a target back.
- **A `Slow` re-announces.** The impact moves, so `next_swing`, the stretched
  animation and the phase packet move with it: a watcher was given an interval
  to stretch a stroke over, and an impact that changed without saying so is the
  same desync the whole model was built to stop.

Two new interrupt reasons cross the wire with it, `Moved` and `Struck`. Walking,
running and riding all end under `Moved`: what a watcher is told is that the
fighter moved, not which of the three it was doing.

**Ф4 — the preparation bar. ✅ Built.** The client draws the pending action off
the pair from Ф1: filling between the commit and the end when the action is
timed, held when it is armed, gone on an interruption. *Done when:* a fighter's
gesture has a bar over it that agrees with the shard's own interval, and an
action that stopped says why instead of vanishing.

What landed, and the four things worth knowing before reading the code:

- **It is over everyone, not only the player.** The plan said *"the player's own
  preparation bar"* and the wire had already decided otherwise: both packets go
  through `broadcast_packet`, so an archer at full draw across the street is a
  fact this client already had and was throwing away. `CombatActionPhase` was
  decoded by the protocol crate and routed nowhere — Ф4 is largely the missing
  `link::Update` arm.
- **The bar has a state beside it, and the state is a second question.** A
  rectangle answers *how far along*; it cannot answer *what of* — a blow and a
  drawn bow fill the same rectangle — nor *what just happened*, which is the
  question a fight actually leaves behind. So the glyph on the left names the
  kind, the filling names the phase, and a word on the right carries the
  outcome, `InterruptReason` and all. The glyphs are drawn rather than written:
  a codepoint out of a font this client does not ship is a box on somebody
  else's machine.
- **What is running and how the last one ended are two fields, not two states.**
  The obvious model — one record, either running or ended — was built first and
  is wrong for a measurable reason: the next gesture opens on the tick the last
  one lands (`next_swing` is still the impact, see the Ф1 backlog), so an
  outcome merely *replaced* by the next commit would be on screen for a single
  frame, and "hit" would be legible only for the final blow of a fight. They are
  remembered independently, and an exchange reads as a bar filling with the
  previous blow's verdict standing beside it.
- **`Crowd::end_action` now ends two things at two moments.** The animation
  keeps its old rule — a hit and a miss run their last frames, because those
  frames *are* the impact — while the *record* ends on every outcome, because a
  blow that landed is a blow nobody is still preparing. That split is the whole
  method.

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

**Ф7 — arming, and the watches.** `Phase::Armed`, the release delay (D10), and all
three watches — one piece of work, not three, because they differ only in which
already-computed fact the sustain pass asks for. `TargetInSight` and
`TargetInReach` are evaluated per armed action against its own target;
`Contact` is pushed from the movement seam with the momentum that delivered it.
Both `expires_at` and Ф5's drain are load-bearing here: an armed fighter who has
paid nothing makes waiting strictly better than fighting. *Done when:* an archer
can arm a shot at a doorway and spend it on whoever steps out, a rider at a gallop
lands a blow by passing through, and neither lands the instant its watch fires.

A fourth watch will be wanted before this is a week old — a doorway watched with no
target chosen yet, which is *"anyone hostile enters this square"* rather than a
question about one committed opponent. It is deliberately not in Ф7: it is the
first watch that cannot be answered from the armed action alone, so it needs an
index of armed squares, and that index is a design rather than a variant.

## Backlog

Found while building Ф1, Ф2, Ф3 and Ф4, and none of it belonged to any of them.

**From Ф4:**

- **The armed bar drops the one number an armed action has.** `expires_at`
  crosses the wire as the phase's own interval and the picture spends none of
  it: a bow about to give out is drawn exactly like one just armed. Held rather
  than filling is the right shape (a creeping bar reads as an impact
  approaching), but *nearly out* is a real thing to say and nothing says it. Ф7
  is where an armed action first exists, and this is its first question.
- **Nothing arms, so half the picture is exercised only by tests.**
  `ActionFill::Armed` is reachable from the wire and unreachable from the shard
  until Ф7. It is built and covered deliberately — the wire was frozen at Ф1 for
  this reason — but nobody has *looked* at a held bar.
- **The bar has no switch.** Every other overlay in this client is behind one
  (`show_sight`, `show_terrain`, the interior index); this draws always, because
  it is the answer to "there are still too many questions about what combat is
  doing". The first person who wants a clean screenshot will want a toggle, and
  the place for it is `GraphicsSettings` beside the others.
- **A word over every head is a word in English.** The outcome labels are
  hard-coded strings in `shell.rs`, which is what every other diagnostic in the
  HUD does; the day this client grows a string table they are on its list.
- **`Abandoned` covers four things and `Moved` covers three, and now something
  reads them.** Both Ф1 and Ф3 wrote down that the first reader would want them
  apart, and predicted it would be this phase. It is: *"disengaged"* and
  *"died"* are one word on screen, and so are *"walked"* and *"rode"*. Splitting
  either costs one byte on the wire and no compatibility.
- **Nothing tests that a bar lands over the right head.** The state machine has
  tests and the palette has tests; the anchoring is `mobiles::head_anchor`,
  shared with the health bar, and untested for both.

**From Ф3:**

- **Nothing cuts a line of sight mid-swing in a test.** `Blinded` is the one
  condition the sustain pass computes rather than receives, and its shipped row
  keeps exactly the behaviour that was there before — but the scene that would
  prove it wants a door or a wall between two fighters, and the tick tests have
  no cheap fixture for one (the wall fixtures they do have belong to housing
  scenes). The table's own unit tests cover the routing; the world does not.
- **`Mounted` is charged at the step, so a rider who stands still is neutral by
  omission rather than by decision.** It is the honest reading of D5 — the step
  is the seam that has the mount in hand — but *"a mount steadies an archer"*
  written as a `Sway { penalty: -10 }` would then only apply to an archer who
  moved, which is not what an operator writing that row would expect. Either the
  condition wants a second seam (a held condition the sustain pass reads, which
  is exactly the shape `Winded` needs in Ф5) or the row wants a name that says
  *riding*, not *mounted*.
- **A condition charged once per action is charged once per action, and a
  fighter can wait it out.** An archer who runs one stride at the start of a
  draw is swayed for the whole draw; an archer who runs the *last* stride is
  swayed exactly as much. Neither is wrong, and the alternative — decaying or
  re-charging — is a number nobody has asked for yet. It is written down because
  the first person to want "sway more the longer you ran" will find this line
  before they find the `ConditionSet`.
- **`resolve_actions` re-reads its actions, and nothing tests why.** Until Ф3
  nothing could end an action *during* the resolve pass, so the pass could walk a
  snapshot; now a blow reaches its victim through `damage`, which pushes `Struck`
  at whatever the victim was doing — under a `struck = "break"` table the victim's
  own blow, due in the same pass, would land from a copy the rules had already
  taken back. The pass re-reads the component and re-asks whether the impact is
  due, which closes it. The scene that would prove it wants two fighters whose
  impacts fall on one tick and a table that says so, and the assertion has to be
  order-independent because which of the two resolves first is a registry
  detail.
- **`InterruptReason::Moved` covers three conditions**, the same shape the
  `Abandoned` note below complains about. It is deliberate here — a watcher is
  being told the fighter moved — but if Ф4's bar ever wants to say *"you cannot
  shoot at a gallop"* it will want the three apart.

**From Ф2:**

- **A shot's reach is a weapon column and a blow's is still a constant.** One
  `ActionKind` holds both now, so the seam is visible in a single `match` instead
  of split across two tick functions — which is exactly Ф6's job and exactly why
  it is cheap now. `MELEE_REACH` is the last hard-coded reach in the crate.
- **A pacified archer stops shooting, and nobody asked for that this phase.** It
  is the right answer — `volleys` simply never read `Pacified` where the melee
  pass did — but it is a live change to what Peacemaking does, and no test covered
  the ranged half before or after.
- **A shot at point blank now fires instead of clubbing.** Faithful to ServUO, and
  the AI never wanted the tile anyway, but a *player* wielding a bow beside their
  target used to swing it and now looses. Whether a bow should have a melee mode
  at all is content, not engineering, and belongs with Ф6's weapon rows.
- **`deliver_affix_poison` now rolls on a landed shot.** A `HitPoison` bow poisons
  what it hits, which is what ServUO does (`BaseRanged.OnHit` *is* `BaseWeapon.OnHit`
  with a flight in front of it). A Poisoning *coating* is still melee's alone,
  because the skill refuses to smear anything but a blade or a point — that is the
  skill's rule and not a branch in combat, which is why there is no branch here.
- **Nothing consumes `Breath`'s `art`, and it is always the arrow graphic.** A
  `RangedAttack` carries a reach and a damage kind but no picture, so a dragon's
  breath crosses the gap drawn as an arrow. The field is on the action so the
  impact stays self-describing; filling it needs a column on `RangedAttack`, which
  is a content question.

**From Ф1:**

- **`Combat::next_swing` is still the impact, not a recovery.** The model calls
  it "when the next commit may happen"; as built it is re-pinned to the impact at
  every commit, because that is what keeps the gesture covering the whole
  interval. Making recovery a real, separate span is a change to how a fight
  feels — a fighter would stand still between blows — and it wants a number in
  operator settings before anyone writes it. Ф5 is the natural home: it is the
  phase where the interval already gains a second meaning.
- **A concealed opener now lands one tick later.** Commit runs last, so an
  untelegraphed action committed on the tick of the attack command resolves on
  the next one. It is 25 ms and no test could see it, but it is a real
  divergence and this is the note that says so rather than letting it be found
  again from the code.
- **Leaving reach mid-swing now cancels the swing.** Under the deadline only the
  impact tick was tested, so a target that stepped out and back landed the blow
  anyway. Sustain ends it instead, and the next commit opens a *full* fresh
  interval — which makes stepping in and out of reach a real defensive move
  nobody balanced. Whether the answer is a grace window or a partial credit is
  Ф3's question, since it is exactly a condition/effect row.
- ~~**`volleys` still clears a target and schedules its own beat**, so the two
  halves of "who is fighting whom" now live in two shapes: an action for melee,
  a deadline for shots.~~ **Closed by Ф2**, which was its whole job: there is one
  shape and one schedule.
- **The tick's swing timing has three call sites and one arithmetic.**
  `animate_timed`, `preview_harvest` and now `CombatAction::wire_phase` each turn
  ticks into milliseconds; the third one is deliberately the only copy in the
  component, but the first two are still two. One `TickMillis` conversion would
  make it impossible for an animation and its phase packet to disagree.
- **`InterruptReason::Abandoned` covers four different things** — disengaged,
  retargeted, died, logged out. It is one byte and splitting it costs nothing on
  the wire; it was left whole because no reader distinguishes them yet, and Ф4's
  bar is the first thing that might.
- **No test drives a stock (non-OpenShard) client past the two new packets.**
  Unknown `0xBF` subcommands are skipped by contract, and the contract is
  believed rather than exercised. A capture-driven test is already on the
  protocol backlog for a different reason and would cover this too.

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
