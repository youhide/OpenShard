# 2026-08-24 — a body is not a wall

The shove, which every entry since the mobile obstacle has listed as *the one
worth building* and every one of them has deferred for the same reason: it opens
with a facet ruleset this engine did not have. It has one now, and the shove is
built on it — a rested player pushes past a body for ten stamina, a tired one is
stopped, and the stock client stops being contradicted about it.

## Where it stands

### The ruleset came first, because the rule's first branch is one ✅

`Mobile.CheckShove` opens with `(m_Map.Rules & MapRules.FreeMovement) == 0`, and
`Facet` here was an id and nothing else.
[`FacetRules`](../../../crates/server/state/src/facet_rules.rs) is one field —
`free_movement` — on `FacetState`, set at load and read through `rules()`.

**The default is derived from the facet number**, which is the only place this
engine reads meaning into one, and that is the decision worth defending. It looks
like the guess a setting is supposed to replace. It is not: the client decides
this question for *itself*, hardcoded as `_world.Map.Index == 0`, and is never
told. A default disagreeing with the client is a stutter on every step near a
body, so `FacetRules::classic` is a statement about what the other end already
believes. `world.free_movement` overrules it per facet, and an operator who does
is knowingly buying the stutter — the config says so.

**Only the flag with a reader.** `MapRules` has four. `Internal` names a map this
engine has no equivalent of, and `BeneficialRestrictions`/`HarmfulRestrictions`
are the same question asked of a spell rather than a step. Named in the module
doc, not built.

### 🚩 The ruleset is read one layer above where ServUO reads it

`CheckShove` asks about `FreeMovement` first. `crowd_near` asks instead — so a
facet with free movement has **no crowd at all**, for anybody.

That is the same answer for a step and a *different* answer for a route: on a
Trammel-ruleset facet a path across a market no longer detours round the
shoppers. Which is what "people are not obstacles here" means once it is said in
one place instead of two, and it is also what keeps this end and the client's
agreeing — the client's own crowd is empty on the same facets, decided the same
way.

### The refusal learned to name itself ✅

`Walk::Refused` carried no reason and `motion.rs` guessed one from outside, under
a comment admitting it: *"the pace and the terrain are the two left and this
cannot yet tell them apart"*. The guess had a cost nobody had noticed —
`RefusedReason::TooFast` was a variant **nothing ever sent**, so a speedhack and
a wall were one number in the metrics.

`movement::Refusal` names the four `Walker::request` actually distinguishes, in
the order it asks them: `OutOfSequence`, `TooFast`, `OffTheMap`, `Blocked`. The
shove needs exactly the last one; the honest metric came free.

### And a body is told from ground by asking the ground again

`Blocked` is the ground and the crowd together, because `movement` is handed a
`Footing` and cannot see who is in it. So `shove_target` re-asks the same step,
same doors, with `Bodies::nobody`: `None` is the ground saying no, and ground
does not move for ten stamina. Only past that is an identity fetched —
`WorldState::body_standing_at`, `crowd_near`'s identity half, paid for only on
the steps a body has already refused.

## What was decided

**Three of ServUO's eight branches are absent, and none is skipped.** `CheckShove`
opens with four conditions that all mean "allowed, silently and free", and every
one is already decided before anything reaches `WorldState::shove`: the facet
ruleset and `IgnoreMobiles` are `crowd_near`'s first line, *either party dead* is
`walks_through_bodies` and `body_blocks`, and *the shoved is hidden staff* is the
rest of `body_blocks`. The rule is called only where the answer is genuinely in
doubt, and the caller's evidence that it is in doubt is that a body refused a
step.

**One shove per step**, which is `m_Pushing`: a paid shove re-asks the step with
the whole crowd gone, so two overlapping bodies cost one shove rather than two.
The re-ask rewinds the walker to a saved copy first — `request` writes its
position, its sequence reset and its pace credit into a copy that has not reached
the registry, so the rewind is exact and the second ask is a first ask. Charging
the pace twice for one `0x02` would rubber-band a player *for shoving*.

**A mobile with no stamina pool cannot shove**, which is the reverse of
`spend_step_stamina`'s reading of the same absence — there a missing pool means
walking costs nothing, because the pool is what a step draws down. Here it is a
price, and "perfectly rested" is not a state something with no notion of rest can
be in. In practice the pool is a player's, so this is also the conservative
answer: a creature stopped by a body today goes on being stopped by one.

**Both step paths ask**, the client's `0x02` and the server's decree, so the rule
belongs to the engine rather than to the packet — even though on the decree path
it almost never fires, for the reason above.

**The mover is revealed and the shoved is not touched at all.** ServUO writes
nothing to the shoved: no reveal, no message, no interruption. Barging through a
crowd is loud; being barged into is not, and a hidden player found by being walked
into stays hidden. Staff shove for free and are not revealed either — a game
master walking through a crowd would otherwise undo their own `.hide` one
bystander at a time.

## What is clean

`cargo test --workspace`: **3,540 passed, 0 failed**, 36 ignored — five new tests
over the shove, two over the config table, one over the ruleset's default.
`cargo clippy --workspace --all-targets`: the same five findings in the same
three files, none this session's — `uofiles/src/map.rs`, `render/tests/traced.rs`
×3, `client/app/src/link.rs`. `cargo fmt --all --check` silent.

**The control is the wall**, and it is the one the rested/tired pair cannot be: a
rested player is exactly the case that shoves, so if the ground were re-asked
without its bodies, `a_rested_player_does_not_shove_past_a_wall` would walk
through a closed door for ten stamina. It asserts the stamina too — a refusal
charges nothing.

The four clilocs were read off the client's own `Cliloc.enu` through this repo's
own `uofiles::cliloc`, **including the staff pair the backlog recorded as
unread**: `1019040` "You shove them out of the way." and `1019041` its invisible
form.

## What is next

| | what would close it |
|---|---|
| 🚩 **The unnamed flake has a name, and it is wider than a test** | `MAX_SEARCH_TIME` is 50ms of **wall-clock** inside the path search, so a loaded box answers differently. Four tests, all green alone. The flake is the small half; the large half is that the timer sits inside a tick `architecture.md` calls deterministic and replayable. A counted budget instead of a measured one — `ai::PATH_BUDGET` is already that shape |
| **The crowd blocks flanks and ServUO's shove does not** | A diagonal squeezed between two bodies is refused by a body with nobody in its landing to shove, so it stays refused. Weigh it at the flank rule, not at the shove |
| **`move_to`'s signature still does not say "a mobile"** | Unchanged — [the grid gets a door](2026-08-24-the-grid-gets-a-door.md) |
| **Every rule written against "a body is a wall"** | `occupy_chair` was the first and is fixed here. There will be more: the shove turned a wall into a route, so anything that relied on the wall is now reachable |

And two this session made, filed in [`roadmap.md`](../../roadmap.md) under the
entry this closed:

- **`occupy_chair` never reserved the seat**, though its doc said it did.
  Nothing checked whether another mobile was already `Seated` on that chair —
  unreachable rather than harmless, because the only route onto an occupied
  chair's tile was through the occupant's own body. Fixed with the test that
  found it.
- **A shove does not disturb the shoved**, which matches ServUO exactly and reads
  like an omission: being walked through while meditating, hidden or casting
  costs the person standing there nothing.
