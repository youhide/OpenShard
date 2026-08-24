# Mobiles and the shove rule

[World index](README.md) · [Roadmap](../README.md) · [Backlog](../backlog/README.md)

## Closed: a mobile is not an obstacle — it was two entries

**A mobile is an obstacle, on both sides of the step.** The method is the one
this entry chose — ask the sector grid, not a second copy in `Obstructions` —
and it is now where every caller reads it rather than at two call sites.

Read the shape before the history: `Footing` has a **fourth field**,
[`Bodies`](../../../crates/common/movement/src/footing.rs), and `walk::landing` asks
it last, at the height the body would arrive at, the way ServUO's `Check` does
(`Movement.cs:344`). Because `landing` is what each of `steps_out_of`'s eight
answers is, the diagonal's flanks obey it too, and so does everything above it:
`can_step`, `step_allowed`, `find_path`, the coarse route's refinement.

`Bodies` holds feet and no identity — sorted by tile, borrowed, **built at the
question and thrown away**, exactly as a `MapTerrain` is. `WorldState::crowd_near`
builds one out of the sector grid; there is nothing to keep in step and no
`unblock` to forget. A step asks for a reach of 1, a plan for the distance to its
goal capped at `CROWD_REACH`, and the bound costs a re-plan and never a wrong
step, because the step reads its own crowd.

The three rules are in `body_blocks` and `walks_through_bodies`, where the
registry is:

- **The dead do not block, and the dead are stopped by nobody** — ServUO's
  `CanMoveOver`, both halves. A corpse is a `Drawn` item and never was a body; a
  *ghost* keeps its `Body` (a shroud is a body graphic) and used to wall a
  doorway the living could neither see nor pass.
- **A mobile may always step off its own tile**, which comes to nothing more
  than the mover being absent from its own crowd.
- **Staff walk through bodies as through walls** — `Staff`, the flag a `.gm`
  puts down, not the account's access level. A *hidden* game master is in nobody
  else's way either, which is ServUO's `t.Hidden && t.IsStaff()`; a hidden player
  still blocks.

And the exemption **reaches the client**: `stance_of` sets
`StatusFlags::IGNORE_MOBILES` (`0x10`) on a staff mobile's `0x77`/`0x78`. The
client keeps its own copy of the rule and applies it to what it predicts, so
without the bit a game master's step is allowed at one end and refused at the
other. That bit was in `StatusFlags`' table with nothing setting it, under the
note that a constant nobody sets is a constant nobody has tested; this is the
day it was wanted.

### What this entry got wrong, and what is left

**Half of it was already built.** `WorldState::mobile_occupies` had been refusing
a step onto an occupied tile since 2026-08-14, in `tick/motion.rs`'s two step
paths. So "a player walks through a standing NPC" was false when it was written.
What was true — and worse than the entry said — is that the *step* knew about
bodies and the *plan* did not: a creature whose quarry stood behind a bystander
walked into the bystander, was refused, re-decided the same direction next beat,
and never went round. `a_chase_rounds_a_line_of_bystanders` is that bug, and its
second assertion is that the creature went round rather than through — reaching
the quarry on its own would also pass on a shard that had forgotten bodies
entirely.

~~**The client plans through bodies.**~~ **Closed.** `clutter::crowd` is the
client's `crowd_near`: the mobiles in its view, filtered, sorted by tile, built
at the question and thrown away, and handed to every footing a *step* is decided
against through `Footing::among` — the held arrow, the click-to-walk plan, and
the route the HUD draws, which is the same plan. The guide reading keeps
`Bodies::nobody`, because a bystander must not rewrite a corridor's topology.

What the client had was not a crowd but a *disguise*: `clutter::fill` laid every
mobile into the overlay as furniture a body's height tall, under a comment
admitting it was "not a category the shared type names". Two things were wrong
with it, and the second is why the disguise could never have been made right:

- **Sixteen against fifteen.** A cover blocks `[z, z + height)` and a mobile was
  given `PLAYER_HEIGHT`; the shard measures body against body with
  `MOBILE_OVERLAP`, which is one less on purpose. At exactly the boundary this
  end refused a step the shard allows.
- **A cover cannot name an exemption.** The overlay has no idea who is asking, so
  staff and the dead were held to a rule the shard exempts them from.

And the bit that carries the exemption **did not reach the one client that needs
it**. `stance_of` fills the `0x77`/`0x78`, which is how a client learns about
*somebody else* — but a client only ever predicts its *own* step, and all three
senders of the `0x20` wrote `StatusFlags::NONE` into it. A game master learned
that every other staff member walks through bodies and never that they do. The
`0x78` a player is sent about itself does carry the byte, which is what let the
gap survive: it is true until the first step or relocation sends a `0x20` over
it. All three now read `stance_of`, which is also how a *ghost* is told — death's
own `0x20` — and that one is not a corner case: a ghost's walk home passes
through the living, who cannot see it to move aside.

Left open, and none of it blocks anything:

- ~~**A player does not shove, and in UO a player shoves.** This engine
  hard-blocks, which is not parity and is not invisible: the stock client has
  the mirror of the rule and draws the step we refuse.~~ **Closed** — a rested
  player shoves past for ten stamina now, and the facet ruleset the rule opens
  with exists. See the backlog entry below.
- **A boat's deck and a moving multi.** The crowd is read off the sector grid,
  which holds a mobile's own tile. Nothing here asks what happens to two bodies
  on a deck that moves under them.
- ~~**`Sectors::nearby` is still linear in a bucket**, and this entry is the second
  per-step reader that was predicted below. It is now real.~~ **Closed** — a
  bucket is two lists now and `crowd_near` reads the mobile one. See the backlog
  entry below.

### Found while closing it

- **`ai::step_toward` has no production caller.** Every body that walks goes
  through `step_body_toward`, which is the sibling with somewhere to write a
  refusal down; the only thing left calling the plain one is
  `tick/tests.rs`'s `walk_toward`. It is `pub`, it carries the doc comment that
  explains the two-search fall-back for both of them, and it gained a `mover`
  argument this session for a crowd only its test will ever have. Either its
  test uses `step_body_toward` and it goes, or its doc moves to the sibling and
  it stays as the deliberate pure-function reading — but "public, documented,
  and called once from a test" is not a state anybody chose.
- **The crowd is now built on every walk request, and used on some of them.**
  `mobile_occupies` was asked only when the walk had already succeeded; the
  crowd has to exist before `Walker::request` is called, so a *turn* and a
  pace-refused step pay a sector sweep for nothing. It is one `nearby` call and
  turns are a small share of requests, so it was left alone deliberately — but
  it is the same sweep the entry above says is linear in a decorated town, and
  if that ever needs shrinking, `intend` is the shared function that says
  whether a request steps at all.
- **Two client diagnostics quietly stopped counting bodies, and they are right
  to.** `picking_query.rs`'s level marker and `terrain_overlay` both ask
  `can_fit` over the cluttered footing, so while mobiles were covers a bystander
  made a tile read "blocked" in a debug wash and on the height diagram. `can_fit`
  has said all along that "a body is not what this places", so removing them puts
  the two back in step with their own contract — a diagnostic about the *ground*
  no longer flickers as people walk about. Written down because it is a visible
  change nobody asked for, not because it wants undoing.
- ~~**The client's crowd is built per ask, not per view.**~~ **Closed.** It is a
  projection now, written by `clutter::project` — one call that replaces the
  furniture in the facet's live layer *and* the bodies in `WorldState::bodies`,
  because a view change that refreshes one and not the other is a step decided
  against two different moments. The four call sites read the field. The clocks
  differ at the two ends of the wire and the reason is in the arguments, which
  is now written on `Bodies`: `crowd_near` takes a mover, a centre and a reach,
  so its answer is per asker and cannot be projected; the client's is a function
  of the view alone.
- ~~**`is_ghost` is the client's whole answer to "is that one dead".**~~
  **Closed: it is two answers now, and they have to agree.** The crowd leaves
  out a body whose *graphic* is a ghost's **and** whose flag byte carries
  `IGNORE_MOBILES`. Each half alone is wrong in its own direction — the graphic
  alone calls a living spectral NPC dead and walks into it a hold at a time; the
  bit alone is `walks_through_bodies`, staff *or* dead, and walks into a visible
  game master standing in a doorway. Together they are exact to within a game
  master who has taken a ghost's graphic while alive.

  **This first went in as a deletion, on a proof that was one gameplay rule from
  false.** The proof was that a client holding a ghost is itself dead or staff,
  both exempt — true of `can_see_mobile` *here*, and not of UO: ServUO's
  `CanSee` ends `... || IsStaff() || m.Warmode` (`Server/Mobile.cs:9229`), so a
  ghost in war mode is visible to the living, which is exactly how the living
  find one to resurrect. Rules the shard has not implemented yet are not a thing
  to build a client's step rule on top of. Filed below.
- ~~**The `0x20`'s flag byte is now sent and still half-ignored.**~~ **Closed.**
  The player's `flags: StatusFlags` is now `walks_through_bodies: bool`: the one
  bit this end answers from, folded at the door, and no second home for the
  stance sitting in a field where the next reader would find it first. `0x72`
  keeps `Player::war` — the same split the reference client makes
  (`PlayerMobile.InWarMode` is its own field there, while `Mobile.InWarMode`
  reads the byte). A `0x72` moves the stance with no `0x20` behind it, so the
  byte's war bit is not merely unread: it is wrong for as long as the body
  stands still.

### Found while closing those three

- 🚩 **A ghost in war mode is visible to the living, and this shard has never
  said so.** ServUO's `CanSee(Mobile m)` (`Server/Mobile.cs:9229`) ends
  `((m.Alive || (Core.SE && Skills.SpiritSpeak.Value >= 100.0)) || !Alive ||
  IsStaff() || m.Warmode)`. Two ways in, and the second is the one that matters
  for play: **the manifest** — a ghost draws its stance and the living can see
  it, which is how somebody who died in the woods is found, and it is the
  precondition for a stranger resurrecting them at all. `can_see_mobile` has
  neither clause and its doc quoted `CanSee` without them, which is fixed.

  What it costs is not a predicate: a war toggle on a ghost becomes a `reveal`
  for every living watcher in range and its reverse becomes a `hide`, so the
  `seen` set has to move when `warmode` moves — the same shape as
  `break_cover`/`hide`. Spirit Speak's clause is cheaper (it is a property of
  the *watcher*, so it only changes what a `show` decides) and it is SE-era, so
  it can come second. The client's crowd is already written not to depend on
  their absence — see the entry above.
- ~~**A ghost walks through a shut door, and neither end knows.**~~ **Closed,
  and the exception this entry promised thought about turned out not to exist.**
  ServUO's `MovementImpl.Check` sets `ignoreDoors = (m_AlwaysIgnoreDoors ||
  m == null || !m.Alive || m.Body.BodyID == 0x3DB || m.IsDeadBondedPet)`
  (`Scripts/Services/Pathing/Movement.cs:173`) and `IsOk` then steps past
  anything carrying `TileFlag.Door`. It was one argument at each end and both
  now carry it: `WorldState::walking_doors` on the shard, off the same
  `is_alive` that `walks_through_bodies` reads — one definition of dead, not two
  to drift apart — and `world::walking_doors(dead, auto_open_doors)` in the
  client, which the HUD's route reads too, so the green line does not stop at a
  leaf the body is about to drift through.

  **A house's door is not an exception.** `BaseHouseDoor.CheckAccess` guards
  `Use` (`Scripts/Items/Functional/HouseDoors.cs:194`) — the hand on the latch,
  which is `items::doors::may_pass` at this end — and movement never asks whose
  door it is once `ignoreDoors` is set. What a ghost drifting into a stranger's
  house can do there is nothing: it cannot lift, cannot open, and nobody living
  hears it.

  What came with it, because it is the same mechanic seen from the other side: a
  ghost cannot *work* a latch either. ServUO gates every double-click on
  `CheckAlive` before the item is asked (`Server/Mobile.cs:4402`), so
  `toggle_door` answers "I am dead and cannot do that" and the client's
  auto-door stops sending the use. The dead pass through the door they cannot
  open — and a ghost that could swing one would be opening shopfronts in front
  of living people who cannot see who did it.
- **A stranger's `IGNORE_MOBILES` is not "this one is out of the way".** The bit
  is `walks_through_bodies` — staff *or* dead — while the crowd wants
  `body_blocks`, which a living, visible game master satisfies (`body_blocks`
  lets out only a *hidden* one). Filtering the crowd on the bit alone walks this
  end into a game master standing in a doorway. It is half of the conjunction
  above and it is not a rule on its own; the two questions are separate
  functions on `WorldState` for exactly this reason, and there is a test at the
  client's end named after the mistake.
- **Nothing tests that `App::entered` calls `clutter::project`.** The seam has
  its own test (both halves replaced from one view) and the four readers have
  theirs, but the wiring between them is untested because an `App` needs a
  window and a GPU to exist. The gap is one line wide and it is the line that
  would make every step decision a packet stale.
- **A lost shard puts the world out of the view and leaves every projection
  standing.** `WorldView::shard_lost` clears the mobiles, the items and the
  containers — and `entered` is not called afterwards, so the live overlay, the
  presentation's draw lists and now `WorldState::bodies` all keep the dead
  shard's world. Nothing walks over it (`App::walk` refuses once the shard is
  lost) and the last frame is deliberately left on screen, but that is the
  opposite of what `shard_lost`'s own doc argues for: it exists so that a
  picture which goes on looking right cannot outlive the connection, and the
  picture is precisely what does. One call to the projection in the `Lost` arm
  would settle it, and it is a change to what a disconnect *looks like*, so it
  wants its own decision rather than a quiet fix.
- **`clutter.rs` cited `WorldState::visible_to`, which has never existed.** The
  function is `can_see_mobile`. Fixed in passing; noted because the citation was
  load-bearing — it is the proof that a ghost cannot reach the client's crowd,
  and a proof resting on a function nobody can find is a proof nobody can check.

### Found while letting the dead through the doors

- **Nothing gates the dead out of using things in general, and every new
  double-click has to remember on its own.** ServUO answers it once and early:
  `Mobile.Use` reaches `CheckAlive` before the item is ever asked
  (`Server/Mobile.cs:4402`), so *everything* refuses a ghost by default and an
  exception has to be written. Here it is a scatter of `has::<Ghost>` —
  `items::trade`, `items::seating`, `skills::button`, three skill handlers, and
  now `items::doors` — the same rule written once per place that remembered it,
  defaulting the wrong way in every place that did not. The one that forgets
  gives a ghost hands, and it will be the one written after this sentence. The
  choke point exists: `tick.rs`'s `Command::DoubleClick { request:
  UseRequest::Use(..) }` arm, where ServUO asks. What it lacks is the question —
  and one named exception, because ServUO gates mobiles too (`Mobile.Use(Mobile)`
  ends at the same `CheckAlive`) and reaches a ghost by *movement* instead:
  `BaseHealer.OnMovement` offers the resurrection when the dead walk up. This
  shard has that path **and** a double-click on the healer, and the second one
  is what a blanket gate would take away.
- **The walkability wash still paints a shut door blocked for a ghost.**
  `picking_query::terrain_overlay` asks `can_fit`, which reads
  `Doors::AsTheyStand` whatever the footing it is handed says — deliberately,
  because it answers "does a thing *fit* here" and a door that could be opened is
  still hanging in the gap. That contract is right for placement and wrong for a
  diagnostic about where *this* body may stand, which is what the wash is drawn
  for. The route line and the wash now disagree for one player: the dead one.
  Small, visible, and the fix is a body-aware `can_stand` rather than another
  argument on `can_fit`.

## ✅ The shove — a rested player pushes past a body rather than stopping at it

**A good mechanic, and the reason to write it down is not only that it is
good.** A body in the way is not a wall in UO: a player at full stamina walks
*through* the crowd, spends ten stamina and is told they shoved somebody. It is
a small rule that does a lot of work — it keeps a doorway from being griefable by
standing in it, it makes a busy bank an obstacle course rather than a wall, and
it prices moving through people in the one currency a fight already cares about.
Stamina is the throttle: shove your way across a market and you arrive with
nothing left to run with.

**And this engine currently contradicts the client about it.** ClassicUO applies
the mirror of the same rule to what it *predicts* — see
[`findings.md`](../../findings.md) for the line and its reading — so the stock client
walks a rested player's body into a crowd and the shard snaps it back. That is
today's behaviour on every facet, not a hypothetical.

### The rule, as the two references state it

`Mobile.CheckShove` (`Server/Mobile.cs:3517`), called on the **mover** through
`OnMoveOver` from `Mobile.Move` (`:3216` and `:3243`, at the same
`(other.Z + 15) > z` overlap the rest of the step uses):

| when | what happens |
|---|---|
| The facet has `MapRules.FreeMovement` (everything but Felucca — `Server/Map.cs:129`) | The rule does not run at all. Everyone walks through everyone |
| The mover has `IgnoreMobiles` | Same: no rule, no cost, no message |
| Either party is dead, or is a dead bonded pet | Allowed, silently and free — this is `CanMoveOver` again, and it is already built |
| The one being walked over is hidden **and** staff | Allowed, silently and free — also already built |
| Already shoved once this step (`m_Pushing`) | Allowed, silently and free. The flag is cleared once per `Mobile.Move` (`:3180`), so walking over two overlapping bodies costs one shove, not two |
| The mover is staff | A message, and nothing else — no stamina, no reveal |
| The mover is at **exactly** full stamina | **10 stamina, a reveal of the mover, a message — and the step goes through** |
| Anything else — a mover one point below full | **Refused.** This is the only branch that stops anybody |

The four messages are clilocs, and which one depends on whether the mover is
staff and whether the body being shoved is hidden:

| | visible | hidden |
|---|---|---|
| staff | `1019040` | `1019041` |
| player | `1019042` | `1019043` |

`1019042` reads *"Being perfectly rested, you shove them out of the way"* and
`1019043` *"Being perfectly rested, you shove something invisible out of the
way"* — read off the client's own table, so the mechanic's in-game name is
*shove* and the hidden form deliberately does not say who. The staff pair's text
has not been read. Note what the wording settles: the line goes to the **mover**,
and the reveal is the mover's too — shoving a hidden player does not reveal
*them*.

The staff pair *has* been read now, off the same table:
`1019040` is "You shove them out of the way." and `1019041` its invisible form.
All four are in `protocol/src/localized.rs`'s catalogue.

### What was ours to decide, and the ruleset it grew

**This engine had no facet rulesets.** `Facet` was an id and nothing else, so the
rule's first row had nowhere to come from. The choice was between shoving
everywhere — cheap, and wrong wherever the client's hardcoded
`_world.Map.Index == 0` disagrees — and growing the ruleset. **The ruleset was
grown**, as this entry predicted it would have to be:
[`FacetRules`](../../../crates/server/state/src/facet_rules.rs), one field, read
through `FacetState::rules()`.

Three decisions inside it are worth having written down.

**The default is derived from the facet number, which is the one place this
engine reads meaning into one.** That looks like exactly the guess a config is
supposed to replace, and it is not: the *client* decides this question for
itself, hardcoded, and is never told. A default that disagreed with the client
would be a stutter on every step near a body. So `FacetRules::classic` is a
statement about what the other end already believes, and `world.free_movement`
is the operator's way to overrule it — while knowingly buying the stutter.

**Only the flag with a reader.** `MapRules` has four; `Internal` names a map this
engine does not have, and `BeneficialRestrictions`/`HarmfulRestrictions` are the
same question asked of a *spell*. They are named in the module doc and not built,
because a flag nothing asks about is a flag nothing keeps honest — and the second
one to grow a reader is what decides whether the config's table becomes a
per-facet rules table.

**The ruleset is read one layer above where ServUO reads it.** `CheckShove` asks
about `FreeMovement` first; `crowd_near` asks instead, so a facet with free
movement has no crowd at all. That is the same answer for a step and a *different*
one for a route: on a Trammel-ruleset facet a path across a market no longer
detours round the shoppers, which is what "people are not obstacles here" means
when said once instead of twice.

### How it is built

- **The refusal names itself.** `Walk::Refused` carried no reason, and `motion.rs`
  guessed one — which is why `RefusedReason::TooFast` was a variant nothing ever
  sent, and a speedhack and a wall were one number in the metrics.
  `movement::Refusal` has the four `Walker::request` actually distinguishes, in
  the order it asks them.
- **A refusal by a body is told from a refusal by ground by asking again.** The
  same step, the same doors, with `Bodies::nobody`: `None` is the ground, and
  ground does not move for ten stamina. Only then is the identity fetched —
  `WorldState::body_standing_at`, which is `crowd_near`'s identity half and is
  paid for only on the steps a body has already refused.
- **The rule itself is `WorldState::shove`**, beside `crowd_near`,
  `walks_through_bodies` and `body_blocks` — the family that already answers "who
  is in whose way". Three of ServUO's eight branches are absent because they are
  decided before anything reaches it, and its doc says which is where.
- **One shove per step**, which is `m_Pushing`: a paid shove re-asks the step with
  the whole crowd gone, so two overlapping bodies cost one shove, not two.
- **Both step paths ask it** — the client's `0x02` and the server's decree — so
  the rule is a property of the engine rather than of the packet. On the decree
  path it almost never fires, because a shove is paid in stamina and a creature
  carries no pool; that is deliberate and its own paragraph in `shove`'s doc.

**Done**: a rested player walks through a standing body and arrives ten stamina
poorer, a tired one is stopped, a wall stops both, a facet with free movement
charges nobody anything, and the decreed step obeys the same rule. Five tests in
`tick/tests.rs`, and the wall one is the control that keeps the shove from being
"retry the step without its crowd".

### Found while closing it

- 🚩 **The unnamed full-suite flake has a name, and it is wider than a test.
  ✅ Fixed.** The previous entry recorded "one full-suite run reported a single
  failure with no name captured". It is
  `a_creature_routes_past_its_exact_budget_over_the_coarse_graph`, and three
  `movement::navigation` tests join it — all four green in isolation, all four
  red under a loaded full-suite run:

  ```
  left:  Point { x: 37, y: 31, z: 0 }
  right: Point { x: 2, y: 48, z: 0 }
  ```

  The cause was `MAX_SEARCH_TIME: Duration::from_millis(50)`: the path search was
  bounded by **wall-clock**, so a loaded machine gave up sooner and the creature
  turned somewhere else. The flake was the small half. The large half is that the
  timer sat inside the tick, and `docs/architecture.md` says the tick is
  deterministic and a world replays roll for roll — a search that answers
  differently by how busy the box is breaks that, silently, in production and not
  only in a test.

  **Both constants are gone.** An exact search is bounded by its node budget and
  by nothing else — 400 or 600 nodes is 0.1–0.25 ms, so the 50 ms it was also
  measured against was never reachable and only cost the clock read. A *long*
  query is many searches, and what bounds the sum of them is now
  `LONG_PATH_EFFORT`, one counted wallet the floods and the refinement passes
  draw from. It is set from measurement rather than converted: 87 long queries
  over two origins on facet 0 spend a median of ~1,900 node expansions and a
  worst of 4,377, and the ceiling is 100,000. `SearchExit::Deadline` and
  `LongExit::Deadline` are `Spent`, which is a fact about the ground rather than
  about the machine.

  **It paid for itself twice**: `clock_gettime` was the only syscall in the hot
  loop and 6.5% of a profile of the search.
- **`occupy_chair` never reserved the seat**, though its doc said it did — "this
  tiny server-side marker reserves that occupied seat". Nothing checked whether
  another mobile was already `Seated` on that chair. It was unreachable rather
  than harmless: the only route onto an occupied chair's tile was through the
  occupant's own body, and a body was a wall. The shove made it a route and the
  next thing through seated them both. Fixed here, with the test that found it —
  and it is the shape to expect more of, since every rule written against "a
  body is a wall" now has a way past.
- **The crowd blocks flanks and ServUO's shove does not.** `steps_out_of` asks
  `Bodies::blocks` about a diagonal's two flanks as well as its landing, which is
  this engine's own choice and predates the shove. ServUO checks `OnMoveOver`
  only for the tile being *entered*. So a diagonal squeezed between two bodies is
  refused by a body and has nobody in its landing to shove: `shove_target`
  answers `None` and the step stays refused. Conservative, and possibly right —
  but it is a divergence nobody has weighed, and the place to weigh it is the
  flank rule rather than the shove.
- **A shove does not disturb the shoved.** ServUO's `CheckShove` writes nothing
  to `shoved` at all — no reveal, no message, no interruption — and this matches
  it. Worth recording because it reads like an omission: being walked through
  while meditating, hidden or casting costs the person standing there nothing.
