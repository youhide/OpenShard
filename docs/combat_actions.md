# A blow that takes time

A fighter in this engine does not *do* anything. It has a deadline, and when the
deadline arrives damage appears. Everything a player could read as an intention —
the raised axe, the drawn bow, the moment it could still be spoiled — is either
absent or is a picture with no fact behind it.

This is the plan that gives the started action a name, a duration, an owner and an
end. It is the foundation the interesting things stand on: a polearm that reaches
two tiles, a shot loosed at a run, an archer holding a drawn bow on a doorway. None
of those is built here. All of them are unbuildable until this is.

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

One component, present only while something is happening:

```rust
/// What this combatant is doing right now, and when it lands.
pub struct CombatAction {
    pub target: Serial,
    pub kind: ActionKind,
    pub started_at: WorldTick,
    /// Moves: a condition rule may push the impact out (see the rules table).
    pub completes_at: WorldTick,
    /// Accumulated while the action runs, spent once at the hit roll.
    pub accuracy: i16,
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
```

`TileReach` is today's `RangedRange` (`crates/server/combat/src/weapons.rs`) with
the word *ranged* taken out of it: once a blow has a reach too, the newtype is
about tiles between two fighters and not about which half of the weapon table it
came from. Ф5 is where it is renamed; until then `Swing` commits the constant.

Three verbs run over it, once per tick, in this order:

**Commit.** No action, engaged, recovery elapsed. Every precondition is tested
*here*: attackable, same facet, within the weapon's reach, sight clear, not
pacified, ammunition present. On success the round leaves the pack, the component
is inserted, the fighter faces the target, cover breaks, and the animation goes out
with its `SwingTiming` — melee and ranged alike.

**Sustain.** An action exists and has not completed. The condition rules for its
kind are applied for this tick (below). A rule may push `completes_at`, add to
`accuracy`, or end the action. So may the world: a target that died, logged out or
left the committed reach.

**Resolve.** `now >= completes_at`. The hit roll is made with the accumulated
`accuracy`, damage or a miss follows, a `Shot` emits its projectile, the outcome
goes to every watcher, and recovery is scheduled.

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
}

pub enum ActionEffect {
    /// The action ends as `Interrupted`.
    Break,
    /// The impact is pushed out by this percentage of the remaining time.
    Slow { percent: u16 },
    /// Taken off the hit roll when the action resolves.
    Sway { penalty: i16 },
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

## The wire

**One new packet.** The beginning already crosses: `SwingTiming` carries the
duration, and the client draws from it. What has never existed is the end.

`CombatActionEnded` — `0xBF` subcommand `OPENSHARD_SUBCOMMANDS + 16`, the first
free one after `HarvestCompleted` (`feedback.rs:333`). It carries the actor's
serial and the outcome. Stock clients skip unknown extended commands, so there is
no compatibility cost, and no existing packet changes shape.

The player's own preparation bar (Ф4) needs nothing further on the wire: the client
already knows its own serial, already receives its own `SwingTiming`, and already
holds `view::Player::attacking` from the `0xAA`.

## The phases

**Ф1 — the object.** `CombatAction`, the commit/sustain/resolve split, and
`CombatActionEnded`. Melee only; behaviour deliberately unchanged except that the
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

**Ф4 — the preparation bar.** The client draws its own pending action: a bar
filling from the animation's start to its promised impact, emptied by an
interruption. Client-side only, off facts it already holds.

**Ф5 — reach as data, and one pass.** D7: `reach` on every weapon row rather than
`MELEE_RANGE` in one function and `range` in the other; `swings` and `volleys`
become one. The polearm at two tiles is then a number, and the halberd is content
rather than engineering.

**Ф6 — the hold.** An action that reaches full draw and *waits* — for a tick, for a
target to enter its reach, for a door to open. Overwatch is a fourth verb between
sustain and resolve, and it is buildable only because the first five phases made
"an action in progress" a thing that exists.

## What this does not cover

- **Special moves, combos and stamina costs.** The wrestling opener and combo
  (`lib.rs:1355-1400`) keep working through the new object untouched; giving them
  phases of their own is a separate design.
- **Casting.** Spells have their own timing today and are not folded in here. They
  are the obvious second customer for `CombatAction`, and the day a third appears
  is the day the component should be renamed and moved out of combat.
- **Mounted combat rules beyond the condition row.** `Riding` exists; what a mount
  does to reach, to damage or to being dismounted is not decided here.
