# The walk: one state, one clock, and what a refusal means

The most-reported class of defect this client has had is a walk that looks
wrong, and almost none of them were in the code that draws. They were two
timelines drifting apart, or two owners of one position disagreeing, or a step
leaving at the moment an input arrived rather than at the moment the walk was
free for one. This document is the rules that came out of fixing them, stated
once so that a caller does not have to re-derive any of them.

The account of how each was found — every measurement, every wrong first cut —
is [`evidence/2026-08-30-the-client-backlog.md`](evidence/2026-08-30-the-client-backlog.md)'s
"joining the window to the wire", and the ownership refactor that follows from
the first rule is
[`evidence/2026-08-27-movement-state-refactor.md`](evidence/2026-08-27-movement-state-refactor.md).
Status and what is open are [`README.md`](README.md).

## The invariant

> **There is one movement state.** The authoritative view, the prediction, the
> transition, the drawn body, the camera target and the HUD's route origin are
> each a projection or a query of it, never a second copy advanced on its own.

`PlayerMotion` is that state: it owns the local prediction, the ordered chain of
pending steps, and the continuous rendered pose. `Crowd` is an animation and
projection consumer — it still owns everybody *else*. An ack retires its matching
pending identity and nothing else; a reject or a relocation snaps to the server's
position and discards the rest of the chain.

The reason this is written as an invariant rather than as a module boundary is
that every violation of it looked like a rendering bug from the outside: the HUD
saying the body had moved while the sprite stood on an earlier tile is two
owners, not a missed frame.

## The picture and the truth are not the same number

An entity has an **authoritative** position — what the server said, plus what
this end has predicted on top — and a **drawn** position. Everything that is not
the picture reads the authoritative one: the depth order, what the body may walk
behind, what the camera calls on screen.

- **A correction never moves the picture directly.** It moves the authoritative
  position at once and puts the difference where the drawing can absorb it.
- **The absorption is bounded.** Past a threshold a correction is a teleport and
  is snapped, because sliding a body across half a facet is a stranger picture
  than the jump it hides. A move of more than one tile is never glided, and a
  rollback — which is one tile, so the rule above does not catch it — goes
  through `Crowd::snap`, which puts the body there and deliberately leaves the
  animation alone: a walker whose third step is refused is still walking.
- **A rollback is not a pace sample.** The gap between a step and its refusal is
  latency; feeding it to the crossing estimate makes the next tile take a
  quarter of a step.

## The pace: what is measured and what is commanded

Nothing on the wire says how fast a creature walks, so for **everybody else** the
crossing is measured: a body already under way crosses each tile in the time the
last crossing took, believed only within half and double the wire's own claim —
outside that band the gap is a body that had stopped or two steps in one burst,
not a pace.

For **our own body it is commanded, not measured** (`Crowd::commanding`). We send
our own steps, so the nominal hold is not an estimate of the walk, it *is* the
walk; measuring it anyway feeds the event loop's wake jitter into the crossing
length, and consecutive gaps jitter in opposite directions, so the estimate came
out worse than the constant it replaced.

`openshard_movement::crossing_left` (`common/movement/pace.rs`, beside the four
rates) is the one function both `crowd::crossing` and `GameMotion::start` read.
Both were the same walk at a walk and not at a gallop, and the difference was a
pause on the tile of up to 17% of a 100ms hold, ten times a second.

**The rate is the hold, never the interval.** `WALK_INTERVAL` and `RUN_INTERVAL`
are anti-speedhack *floors* the shard judges by, deliberately half the real rate;
walking at the floor moves a body twice as fast as the crowd glides it. The four
rates are on foot; a mount's two have nothing here to select them yet, which is
in the README's open list.

## One clock, three asks

Exactly one of "arrows", "heading" or "destination" drives a step at a time
(`Steering::asking`). The keyboard outranks both, and `go_to` and `steer` clear
each other.

- **`Steering::steer(heading)`** — a held right button with no modifier. A
  compass bearing from the body to the cursor, recomputed every move and driven
  exactly like a held arrow key. It has no notion of arrival or of being stuck
  and it never plans.
- **`Steering::go_to(tile)`** — Ctrl-drag. The real move order: a route is
  planned, a refusal replans from where the body actually is.
- **The arrows** — a direction is *held* rather than pressed, and the clock is
  ours. Sending a step from the key event makes the operating system's auto-repeat
  the walking speed, and its fast half is exactly what the shard refuses as a
  speedhack, which reads as the walk stuttering rather than as the client asking
  for too much.

> **An input joins the queue or rebuilds it. A step already begun ticks out.**

That is one rule and not three fixes, because a step that leaves early goes
wrong in three places at once: the glide starts at the tile the *previous* step
ended on and yanks the body forward; the shard's pace budget refuses it; and the
rollback races the steps still in flight, whose acks then arrive for a sequence
this end has forgotten. `Steering::due` is a floor that nothing clears and
`Steering::free` is the one gate every ask goes through. What the queue *is* is
`Steering::take` reading the keys at the moment the step leaves rather than when
they were pressed — one step deep, rebuilt by every press for nothing.

**A deadline is only a cadence if a step was taken at it** (`Steering::walking`).
The next step is armed from the deadline that has just passed rather than from
the wake, which is what stops a late loop accumulating drift — a few
milliseconds a step is a whole tile behind after fifty, and nothing ever gives
it back. But a deadline that came and went with the arrows up is not a cadence,
and measuring from it cuts the glide short after a fresh press. A wake later than
a whole step is a stall, not jitter, and restarts the cadence: those steps are
deliberately not banked.

`steer::LOOKAHEAD` lets a step leave up to one glide interval before its deadline
*while a crossing is under way*, so the prediction is queued before the crossing
ends. The cadence is unmoved — each deadline still chains from the last.

## What this end may refuse for itself

Two rules, and they are the same rule read at two scales.

**`movement::step_allowed(terrain, from, direction)` is the whole question** —
the destination tile is steppable *and* the step cuts no corner. It exists
because `Terrain::can_step` answers for the destination alone, so a diagonal
clipping the corner where a wall ends reads as perfectly open, and the client
asked for it every hold while the server refused every one. `find_path` and
`steer` both ask it; `LiveTerrain::can_step` restates it inline, and that one
duplicate is intended and says so.

**The ground is read twice** (`steer::Readings`): the map with everything the
shard placed over it (`clutter.rs`'s `Cluttered`, which is what every step is
decided against) and the bare map, which is the same world with every door open.
`steer::plan` asks them in that order, so a door with a way round is a longer
walk and never a barred one; only where there is no way through at all does it
plan over the bare map and cut that route at the first step the real ground
refuses. **What the two readings differ by is a list, and the list is doors** —
`clutter.rs` marks each blocker `door` off `client/render`'s own family table, so
a stack of crates is a thing to route around and not a barrier the picture
promises a far side to.

`Cluttered` is the client's twin of the server's `LiveTerrain` and uses the same
predicate and the same z-span, so the two ends agree by construction rather than
by review.

Where neither reading has a way through, the answer is still a walk and never a
shove: `movement::find_path_toward` is the same A\* read the other way — the
reached tile closest to the goal — out of *one* search, so "there is no way" and
"here is how far the way goes" cannot disagree about which tiles were reachable.

**Nothing this end can already see is blocked is sent.** A step the shard is only
going to refuse comes back as a `0x21` and a rollback, which reads as a character
shuddering against a corner. Wedged with nothing to detour onto, the client sends
the *turn* — which the shard accepts and which is the feedback a player pressing
into a wall expects — and then nothing, with the clock still armed so the walk
resumes on its own the moment the way opens.

## Detour, leeway and lean

Not moving is one of the things a body does, so it is a state and not the absence
of one. `movement::detour` is a scene (`Around`), an intent, a three-state
machine (`Detour::Clear` / `Sliding` / `Standing`) and an answer (`Step::Ahead` /
`Aside` / `Stuck`). Its whole input is **four tiles**: where you stand, where you
meant to go, and the two flanks that could take its place — which two is fixed by
the intent, ninety degrees off a blocked cardinal and forty-five off a blocked
diagonal, and no other neighbour can change the answer. That is what makes the
rule enumerable rather than a wall somebody drew and hoped was the interesting
case.

The two flanks are not symmetric, and getting it backwards is not cosmetic: a
wall dead ahead of a held **cardinal** has no diagonal past it at all, because the
corner rule requires both flanking cardinals of a diagonal to be open and the
blocked direction is one of them either way — so the cardinal along the wall's
face is what is offered. A blocked **diagonal** is pinned by a corner rather than
a wall, so the two cardinals it splits into are what is tried. Offering a
diagonal past a wall draws the body slipping through the corner and rubber-banded
back, which is worse than the stand-and-bump it replaced.

**How far a body may be turned off the ask is a preference, and there are exactly
two of them** because the flanks are fixed. `Leeway::Eighth` — the default — is
the 45° turn a blocked diagonal splits onto: a body rounding a corner, always
allowed, because refusing it is a character stopping dead at the edge of a house
it was walking past. `Leeway::Quarter` adds the 90° turn, the only thing a
blocked cardinal has, which puts the body travelling at right angles to what was
asked. So walking straight into a wall stops the body by default, which is what
the classic client does. It is a parameter to `Detour::step` and not a field on
the machine, because a state and a setting must stay two values.

**`movement::Lean` is the sub-sector detail the cursor already carried.** With
both ways round a corner open and nothing in the terrain to prefer either, the
player has already said which way they mean to go — the cursor sits a little to
one side of it — and rounding to one of eight sectors threw that away.
`Clockwise` / `Centred` / `Counter`, from the sign of a cross product, integer
arithmetic so that "squarely on the bearing" is exact rather than a tolerance in
degrees. The tie-breaks run lean, then the remembered flank, then clockwise.

**And the lean is measured on the screen, from where the body is drawn.** A
player pushes the mouse away from their character in the direction they want it
to go, and that is a bearing on a flat picture. That the screen and the grid
agree for today's projection is a coincidence of its numbers, not a property of
the idea. The origin is the body's own projected pixel and not the middle of the
viewport, which is what keeps it meaning the same thing while the eye wanders off
the body.

## The two rings the cursor answers in

- **`DEAD_ZONE`, 10 world pixels.** Inside it a held right button names no
  heading, so a mouse resting over the character stands still instead of walking
  off in whichever of the eight sectors the last pixel of hand tremor landed in.
  Ten is a play number: the *geometry* asks for `22 / cos 22.5° ≈ 23.8`, below
  which a step can carry the body past the cursor, and the radius is set by the
  jitter argument instead — the overshoot is bounded at one tile and self
  correcting, the jitter is unbounded.
- **`TURN_ZONE`, that same 23.8.** Between the two, a held right button asks the
  body to *face* the cursor and covers no ground (`steer::Ask`), so the whole
  band where walking is the wrong answer is the band where the body turns. This
  is the stock 2D client's ring and **not ClassicUO's**, which walks on any
  non-zero offset from the viewport's centre and cannot turn a character on the
  spot at all; the reason to have it is that a player needs to face a door, and
  every other ask a cursor makes also sets the body walking.

## The turn

Turning is a whole step in UO — the mobile turns, moves nowhere, and gets its own
ack — and the shard answers a turn *before* it charges the pace budget, because
spinning on the spot is something clients do. `steer::Turning` is what a turn
costs, defaulting to the reference's `TURN_DELAY` of 80ms, with
`Turning::Immediate` for none. A turn also **records a pace sample**: it is a
step, it just covers no ground, and treating it as no step measured the tile
after every turn across two holds and crossed it at half speed.

## When the two ends disagree: drain, then ask

A `0x21` voids everything in flight and resets both sequences, but the shard owes
one answer per step and those already on the wire are still answered — so an
answer lands for a sequence this end has forgotten. It is not a desync: the wire
delivers in order, so while anything is owed from before the last correction the
next answer is one of *those*. `Walk::draining` counts them and swallows them,
**including a stale rollback**, which is the half with no symptom anybody would
name — applying it rolls the body back a second time, onto a tile it has already
walked away from.

An answer owed to nobody is a real disagreement, and it has a request/response on
the wire rather than a reason to hang up:

1. It sets `Walk::out_of_step`, and while that holds nothing is sent
   (`NotSent::OutOfStep`). It has to: the prediction is a chain of asks the
   server has stopped agreeing with, and an ack carries no position to correct
   it with.
2. One `ResyncRequest` goes out, guarded on the flag not already being set.
3. The shard queues it like any other packet and answers out of a tick: the walk
   sequence back to zero, this client's screen forgotten so it is sent again, and
   a position packet with the truth in it.
4. That snaps the client, which clears the flag, and the walk is free — from a
   fresh sequence on both ends, which is why the step after a resync is not
   refused.

`walk::MAX_IN_FLIGHT` is five, the reference's number, checked first thing in
`Walk::step`. It is not a second pace limit — the shard's budget is the only
judge of how fast a body walks — it is the answer to a shard that has *stopped
answering*, where every further step is another tile of correction when the link
comes back.

## What a person is told

`steer::Refusal` has four members because they send a player four different
places: `Nowhere` (round the wall, or nowhere at all), `TooFar` (walk closer and
click again), `Barred` (open the door) and `NoGraph` (wait — this one goes away
by itself). The honest part is what they map from: the coarse graph's own "no
corridor" is a real claim about the world, while a query giving up — off graph,
out of budget, portals exhausted — is not a claim about anything and is `TooFar`.

Three readers say it, and each is a different one: the drawn route (dashed, with
a cross on the last reachable tile), the journal (one sentence, once per
destination, because a plan is remade every few steps) and the dev strip (for as
long as the order stands).

## The oracle

`crates/client/app/src/dst.rs` runs the whole path — the steering clock, the
prediction, a real shard-side walker and the crowd — on a virtual clock, over a
wire with latency, jitter and a wall in it, and holds the position of the
*sprite* against an oracle.

The oracle is the **intent** timeline: the body leaves the instant the key goes
down and crosses one tile per hold, for ever. It is built from the script of
inputs alone; it is constant velocity and nothing else. Everything under test is
the **event** timeline — when the loop woke, what the wire did — and the claim is
that the second reproduces the first. That is not a tautology: every walking bug
this client has had is a divergence between those two sets of knots.

Two lessons are pinned into the harness itself. **It must sample what the window
samples** — for a while every corridor and ceiling in it held against a code path
the frame builder had stopped reading, which is a green suite measuring nothing.
And **a corridor is blind to a stall**: every measure bounded the body from
above, and a body that arrives, stands, and sets off again is inside all of them,
so `never_stalled` is the floor and is the assertion the original complaint was
actually about.
