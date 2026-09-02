# 2026-08-23 — a body is in the way of the plan too

The backlog's own recommendation, taken:
[the weeded backlog](2026-08-23-the-backlog-is-weeded-and-what-is-left.md) said
the mobile obstacle is the one a player notices first, that its method was
already chosen, and that nothing blocked it. All three held. What the entry did
not know is that **half of it was already built, and the half that was missing
was the worse half.**

## Where it stands

### 🚩 The entry was the fifth stale one, and its first sentence was false

`WorldState::mobile_occupies` had been refusing a step onto an occupied tile
since 2026-08-14, at `tick/motion.rs`'s two step paths — a player's `0x02` and
the shard's own decree. So "a player walks through a standing NPC" had not been
true for nine days.

What *was* true is the sentence after it, and it is worse than a walk-through:
**the step knew about bodies and the plan did not.** A creature whose quarry
stood behind a bystander walked into the bystander, was refused by a check
bolted on after `can_step` had already answered, and re-decided the same
direction on the next beat. It butted into the same shoulder until something
else moved. A crate in the same place worked fine, because a crate is in the
overlay and the route was planned around it.

### The rule is one rule now ✅

`Footing` has a **fourth field**. [`Bodies`](../../../crates/common/movement/src/footing.rs)
is a borrowed, tile-sorted slice of feet, and `walk::landing` asks it **last**,
at the height the body would arrive at — the order ServUO's `Check` uses
(`Movement.cs:344`), and it has to be last because which bodies are in the way
depends on where this one would stand.

Being inside `landing` is the whole of the change:

| asks `landing` | so it now sees a body |
|---|---|
| `can_step` | the destination tile |
| `steps_out_of` | all eight, **flanks included** — a body cannot be slipped past at a corner |
| `step_allowed` | one slot of `steps_out_of`, so by construction |
| `find_path`, `find_long_path`'s refinement | every node it expands |

### `Bodies` is built at the question and thrown away

The same bargain `MapTerrain` makes. `WorldState::crowd_near` reads the sector
grid — already the authority from tile to entity, already kept honest by the
step itself — filters it to bodies that block, sorts by tile, and hands back a
`Vec` the caller owns for the length of one question. **There is nothing to keep
in step and no `unblock` to forget**, which was the entry's whole argument
against putting mobiles in `Obstructions`.

Reach is the caller's, because only the caller knows what it is asking: `1` for
a step (the eight neighbours, and no more), `distance(from, to)` capped at
`CROWD_REACH = 32` for a plan. The cap costs a **re-plan and never a wrong
step** — the step is decided with its own crowd, read fresh.

### The three rules, and a fourth that was not in the entry

In `body_blocks` / `walks_through_bodies`, server-side, where the registry is:

| | |
|---|---|
| **The dead do not block** | A corpse is a `Drawn` item and never was a body. A **ghost keeps its `Body`** — a shroud is a body graphic — so before this a dead player walled a doorway the living could neither see nor pass |
| **The dead are stopped by nobody** | ServUO's `CanMoveOver`, the other half, which the entry did not name. A ghost has to be able to walk home and the living cannot see it to move aside |
| **A mobile steps off its own tile** | Nothing more than the mover being absent from its own crowd |
| **Staff walk through bodies** | `Staff` — the flag a `.gm` puts down — and not the account's access level. A *hidden* game master is in nobody's way either (ServUO's `t.Hidden && t.IsStaff()`); a hidden player still blocks, because being walked into is how you find one |

### And the exemption reaches the client ✅

`stance_of` now sets `StatusFlags::IGNORE_MOBILES` (`0x10`) on a staff mobile's
`0x77`/`0x78`. The client keeps its own copy of the body-blocking rule and
applies it to what it *predicts*, so a shard that exempts a mobile without
sending the bit gets a step allowed at one end and refused at the other — a
rubber-band, not a permission. The bit was already in `StatusFlags`' table with
nothing setting it, under the note that a constant nobody sets is a constant
nobody has tested. This is the day it was wanted.

## What was decided

**The seam is a field on `Footing`, not a trait and not a second index.** The
module doc that said "three things, and no fourth" now says four and why. The
alternative that was actually live — projecting mobiles into the `Overlay` beside
`Obstructions` and `Boats` — is the one the backlog had already refused, and the
refusal holds for the reason it gave: a body moves three times a second and one
missed `unblock` is a permanent invisible wall.

**`Bodies` carries no identity, and the crate boundary is why.** Which bodies
block is decided where the registry is; `openshard-movement` never learns who
anybody is. This is the same line `Overlay` draws — it says a door is in the way
and only the shard says which door — and it is what let the fourth field arrive
without `openshard-movement` growing a dependency on `openshard-entities`.

**A body blocks a diagonal's flanks.** ServUO does the same (`Movement.cs:552`)
but only for uncontrolled creatures; this engine gives everybody the strict
reading, which is the decision already taken for the corner rule itself and
recorded in `navigation_spans.md`'s *out of scope, named*.

**The goal tile is dropped from a plan's crowd.** ServUO drops the same one
(`Movement.cs:411`). Without it no chase is plannable at all: a creature's goal
is overwhelmingly the quarry, the quarry is a body, and the one tile the route
is *for* becomes the one tile it may not end on.

**The overlap is fifteen, not sixteen.** ServUO measures a body against another
body a unit shorter than it measures it against the ceiling
(`(mob.Z + 15) > newZ`, where `PersonHeight` is 16). Kept as its own constant
beside `PLAYER_HEIGHT` rather than folded into it — two questions that happen to
be one apart. One unit is the difference between a mezzanine being walkable with
somebody standing under it and not.

**A player is hard-blocked, and that is a divergence taken on purpose.** In UO a
player at full stamina *shoves* — 10 stamina, a cliloc, a reveal, and the step
goes through; only an already-tired player is stopped (`Mobile.CheckShove`). The
whole reading is in [`findings.md`](../../findings.md), and the shove is filed
rather than built: it is a stamina charge and two clilocs, which is gameplay and
not movement.

## What is clean

`cargo test --workspace`: **3,511 passed, 0 failed, 36 ignored** (nine new).
`cargo clippy --workspace --all-targets`: the same five findings in the same
three files, none this session's — `uofiles/src/map.rs`, `render/tests/traced.rs`
×3, `client/app/src/link.rs`. `cargo fmt --all --check` silent.

**The controls were run by hand.** With the one line in `landing` removed, the
two movement tests fail and both chase tests fail. That last part is the reason
`a_chase_rounds_a_line_of_bystanders` asserts twice: reaching the quarry passes
on a shard that has forgotten about bodies *entirely* — it walks over five of
them — so the assertion that matters is the second, that it never stood where
anybody was standing.

## What is next

Nothing here blocks anything, and era S's live publish is untouched and still
the next plan node.

The three this session created or made real, in order:

| | what would close it |
|---|---|
| **The client plans through a crowd**, which is the same rubber-band from the other side. `steer.rs` builds every footing with `Bodies::nobody`, so a click-to-walk route threads a crowd and the shard refuses it step by step | The client already has the mobile list — `Clutter` inserts every one at `PLAYER_HEIGHT` for the *drawn* route. What it has no counterpart of is `crowd_near`. Both ends of the wire want the same value here, which is the argument `Footing::guide` already makes |
| **`Sectors::nearby` is linear in a bucket, and this landed the second per-step reader on it** — predicted in that entry, now real. A castle's ~4,000 lockdowns share one or two buckets, and the movement path pays them now as well as the sight path | Unchanged: split a bucket into mobiles and items and let the caller say which it means. Every `crowd_near` and almost every sight call wants mobiles |
| **The shove**, and it is the one worth building. A rested player pushes past a body for 10 stamina, a message and a reveal; only a tired one is stopped. This engine hard-blocks — and the stock client applies the *mirror* of the rule to what it predicts, so it draws the step we refuse, today, on every facet | Written out in full in [the shove entry](2026-08-24-mobiles-and-the-shove-rule.md) — the eight branches, the four clilocs, the seams (all of which exist). One thing has to be decided first and it is not the shove: **this engine has no facet rulesets**, and the rule's first branch is ServUO's `MapRules.FreeMovement`, which is the Trammel/Felucca split |

And one that is simply unexamined: **two bodies on a deck that moves under
them.** The crowd is read off the sector grid, which holds a mobile's own tile;
nothing here asked what a moving multi does to that.
