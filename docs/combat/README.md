# Combat, skills and magic: where they stand

The canon of the `combat` domain — `crates/server/combat`, `crates/server/skills`
and `crates/server/magic`, plus the tables they are written against in
`server/state` (`skill`, `weapon`, `armor`, `summon`, `instrument`, `tame`,
`harvest`, `effect`, `action_rules`), the sight walk in `common/movement`, and
the four passes in `world::tick` that drive them (`spells`, `fields`, `gates`,
`dispel`). This is everything a fighter *does*: what makes a click an attack, how
long a blow takes, what stops it, what a skill roll is worth, what a spell costs
and what it leaves behind.

What a corpse is once it is on the ground belongs to [`items/`](../items/README.md);
what a creature *decides* belongs to `npc`; what the client draws belongs to
[`client/`](../client/README.md) and [`render/`](../render/README.md).

**One entry point.** This page answers "what can a fighter do today" and says
which document holds the reasoning for each line. Where this page and a design
document disagree, the design document is right and this page is stale.

## The one-line answer

**An action is an object with a deadline, a watch and a reason to end, and every
one of the three ends the same way: on the wire.** A blow, a shot and a breath
are one schedule and three impacts; a skill roll and a spell roll are the same
two calls with different numbers; and all damage — sword, arrow, fireball, poison
pulse, burning field, script — passes through one door that applies the target's
resistance for its type.

```text
  commit ──▶ sustain ──▶ resolve          three passes, once each per tick
     │          │           │             (built order: sustain, resolve, commit)
     │          │           └─ combat::damage ── the one damage door
     │          └─ the condition/effect table, an operator's
     └─ every precondition, once, and a Balked with a reason if it refuses

  skill_value ──▶ roll_skill_band ──▶ gain_chance      one curve, every system
  begin_cast  ──▶ pay_and_roll     ──▶ apply_spell_effect
```

## What the area is, by part

| Part | State | What is left | Held by |
|---|---|---|---|
| War mode, the aim, health bars, death, the ghost, the corpse as a body, looting | ✅ shipping | a per-stranger bar *window* — row 15 | [`design_fight_loop.md`](design_fight_loop.md) |
| The action object: three passes, four wire packets, an end with a name | ✅ shipping | — | [`design_actions.md`](design_actions.md) |
| One schedule for a blow, a shot and a breath | ✅ shipping | reach is a column for ranged rows only — the plan's Ф6 | the same, D7 |
| The condition/effect table in operator settings, charged once per action | ✅ shipping | `Winded` and `Drain` are not built — the plan's Ф5 | the same, D4 |
| The stage walk and the preparation bar, off shares the operator owns | ✅ shipping | the armed bar spends none of `expires_at` — row 12 | the same, D12 |
| A refusal as a standing state with a name, on the edge in both directions | ✅ shipping | reasons do not compose — row 13 | the same, D11 |
| Arming: `TargetInSight`, over a bow, from the live shard | 🚧 one watch of three | `TargetInReach`, `Contact`, the untargeted doorway | [`plans/combat/actions/PLAN.md`](../../plans/combat/actions/PLAN.md) |
| Fatigue: the step cost, the overload branch, the winded threshold, three regens | ✅ shipping | an action costs no stamina at all — the plan's Ф5 | `combat/src/vitals.rs` |
| The weapon and armour tables, the to-hit roll, damage scaling, absorption | ✅ shipping | a blow is always physical — row 9 | [`evidence/2026-08-24-the-combat-phase.md`](evidence/2026-08-24-the-combat-phase.md) |
| Ranged: reach off the weapon row, real ammunition, the `0x70` flight | ✅ shipping | a breath is drawn as an arrow — row 14 | [`evidence/2026-08-27-the-ranged-shot.md`](evidence/2026-08-27-the-ranged-shot.md) |
| Sight: one traced walk, `sight_clear` a reading of it, an overlay and `.sight` | ✅ shipping | the rule's own limits, unchanged by design | [`design_sight.md`](design_sight.md) |
| The skill check, the gain curve, the total cap, the locks, stat gain | ✅ shipping | — | [`design_skills.md`](design_skills.md) |
| The usable skills, and four that are subsystems (stealth, bard, taming, harvest) | ✅ shipping | Inscribe, Tracking, Camping, stabling, loyalty, Herding — row 16 | [`evidence/2026-08-24-the-skills-phase.md`](evidence/2026-08-24-the-skills-phase.md) |
| Magery: one gate, 64 rows, the cast sequence in two styles | ✅ shipping | 14 rows are `Unimplemented` — row 7 | [`design_magic.md`](design_magic.md) |
| Effects that outlive the cast: poison, both buff ledgers, paralysis, fields, summons | ✅ shipping | — | the same, and [`evidence/2026-08-24-the-magic-phase.md`](evidence/2026-08-24-the-magic-phase.md) |
| Resisting Spells, the three dispels, the travel family and the nine moongates | ✅ shipping | an `Item`-targeted spell's aimed point — row 4 | the same |
| Anything but a player casting a spell | ⬜ not built | the decision is `ai`'s, the cast is `magic`'s — row 6 | [`plans/combat/spells/PLAN.md`](../../plans/combat/spells/PLAN.md) |

## What is enforced, and by what

The order this domain keeps arriving at is the one the `server` invariants sweep
wrote down: **a type beats a build-time check, and a build-time check beats a
test.** Combat has less of the first two than `items` does and more oracles,
because most of what goes wrong here is a *sequence over time* rather than a bad
row.

- **Every fighter is accounted for, every tick, and the shard says so itself.**
  `commit_actions` ends by walking every fighter and asserting that one alive and
  in war holds either a `CombatAction` or a `Balked`. An assertion and not a
  repair: a pass that invented a reason for the odd one out would hide exactly
  the defect it exists to name. `#[cfg(debug_assertions)]`, so the playground
  runs it on every tick and a release shard pays nothing.
- **A whole fight is run, tick by tick, against a model of the screen.**
  `fight_timeline` (`world/src/tick/tests.rs`) walks six hundred ticks and writes
  each one down twice — what the shard had the fighter doing, and what a
  watcher's screen would hold, rebuilt from that tick's packets alone. The second
  question is the one that matters and no assertion about server state can reach
  it. It must read a tick's packets **in arrival order**: one tick carries the
  end of one action and the commit of the next.
- **The tick's unit is welded to its interval.** A `const` assertion in
  `world/src/tick/defaults.rs` ties `TICK_INTERVAL` to `TICKS_PER_SECOND`, so the
  next person to move the tick gets a compile error instead of a shard quietly
  running every timer at half speed — which is what happened once, in thirteen
  places at once, and the swing timer was the one a player felt.
- **Damage has one door and randomness has one owner.** `combat::damage` is the
  only place a resistance is applied, a murder is attributed, a reflect is
  bounced or a `Frozen` is lifted. Every roll — to-hit, skill gain, dispel,
  resist, summon lifetime, loot — draws on the world's own seeded `Rng`, advanced
  only by the tick, and a test asserts that two identical runs reach the same
  skill roll for roll.
- **An invalid spell circle cannot be attached to a spell.** `SpellCircle::new`
  is the only constructor and it refuses anything outside 1..=8; mana, delay and
  difficulty are all derived from it, so there is no row that can disagree with
  itself. `SpellEffect` and `Watch` are closed enums for the same reason: a watch
  that cannot be named is a watch nobody can cost.
- **The operator's tables are parsed, not merged.** A row an operator writes in
  `[gameplay.action_rules.<kind>]` is the *whole* row — a condition left out is
  no rule, not the shipped default quietly restored — because a table that reads
  one way in the file and runs another is the `..Default::default()` hazard in a
  different hat. `config`'s own tests parse the shipped text.
- **`print_a_bow_fight`** is `#[ignore]`d and run by name: it prints the cadence
  in ticks. It proves nothing and is not meant to — it is what you run before
  arguing about frames.

One crate-wide invariant sits above all of it, stated as a dependency rather than
as prose: **`magic` → `combat` → `skills` → `items` → `state`, and never
backwards.** That single edge decides where half the rules in this domain live —
why a Poisoning fumble is decided in `skills` and applied by the tick through
`combat`, why fame and karma live in `state::title`, why a trap's trigger is in
`world::tick::traps`, and why the one door out of a `CombatAction` is on
`WorldState` rather than in `combat`.

## What is open, ranked

**1. 🚩 The stamina pool never reaches the client, and a client that believes it
has none refuses to run.** `0xA2` landed for mana with one `set_mana` door
beside it; there is no `0xA3` and no `set_stamina`, and `Stamina` is mutated in
place by every step and every regen tick with nothing sent. The pool reaches a
client only inside a `0x11`, which `refresh_statuses` sends on a diff of
*inventory-derived* numbers. The stakes are higher than a stale bar — the client
will not run, and shows no error for it. The fix is exactly the shape mana took:
one door, one packet, and this client's pool moving out of `Status` into
`Player`.

**2. 🚩 `contained_items` is the tick-killer that has not been fixed yet.** It
sits ten lines from `equipped_items`, which cost 80% of a tick on a restored
Felucca and made every duration the shard announced about five times shorter than
the one it delivered. Both walk every located item in the world to answer a
question about one container. Nothing in the hot path calls `contained_items`
today — which is the only reason it is a line here and not the same emergency —
and the argument that fixed the other one applies to it word for word.

**3. 🚩 A blow's reach is a constant and a shot's is a column.** `MELEE_REACH` is
the last hard-coded reach in the crate, one tile for every weapon that is not a
bow. The polearm at two tiles falls exactly on that seam, and so does the
`RangedRange` → `TileReach` rename the model has been describing for two phases.
It is the plan's Ф6 and it is cheap now that one `ActionKind` holds both.

**4. 🚩 An `Item`-targeted spell has no trustworthy aimed point.**
`handle_target` passes the client's `location` through, and for an object the
client fills it with the *item's* coordinates — which for something in a pack are
the slot inside the container, not a place in the world. Nothing reads it today
(the travel family all voice themselves at the caster), but `spell_feedback`
still falls back to `target_location` when the mark has no `Position`, so the
first `Item` spell that wants a picture on the thing it aims at will draw it in
the sea. The fix is to resolve an object target to *the holder's* position rather
than trusting the wire.

**5. Two copies of one rule, in two crates that may not name each other.**
`fight_timeline`'s screen model and the client's `crowd::ActionRecord` write the
same holds, the same timeout and the same arrival-order handling twice, and
nothing makes them agree: a change to `OUTCOME_HOLD` would break the oracle
silently — it would still pass, about a client nobody ships. The honest fix is a
shared crate for the record itself, which is a `common/` question rather than a
combat one.

**6. Nothing but a player ever casts.** No mana on a creature, no choice of spell
in `fight_phase`, no cast in the beat — so a lich, a mage-brigand and a healing
dragon are all impossible and the whole of magic is one-directional. The cast
path is reusable (`resolve_cast` and `apply_spell_effect` are not client seams);
what is missing is the decision and an aim that does not come from a cursor. See
the spells plan.

**7. Fourteen spell rows cast and do nothing, and eleven of them need no new
subsystem.** Create Food, Mana Drain, Mana Vampire, Arch Protection, Mass Curse,
Invisibility, Reveal, Magic Lock, Unlock, Magic Trap and Untrap are
`Unimplemented` only because nobody has written the arm — every piece each one
needs is in the tree. The genuinely blocked three are Telekinesis, Incognito and
Polymorph. A scroll is likewise a textbook and not a spell: it teaches its spell
to a book and casts nothing.

**8. Combat state is an edge, and a screen that arrives late is told once and
then never.** `WorldState::show` was taught to send a running action, its stage
and a standing refusal to a body coming into view — but that covers a *body*
arriving on a screen, not a *screen* arriving at a body, which is what a
`0x22`-driven rebuild or a reconnect is. Whether the client drops its crowd
records there has not been checked, and if it does, the same silence comes back
by the one route the fix does not cover.

**9. A blow is always physical, whatever it is delivered with.**
`ActionKind::damage_type` answers `Physical` for a `Swing` and a `Shot` alike;
only a `Breath` carries a kind. A fire sword and a lightning bow are therefore
inexpressible, and the resistance system that would read them is already built.
Pre-existing, and it survived two rewrites of where the blow is resolved.

**10. `next_swing` is the impact, so there is no recovery and a fight has no
rhythm between blows.** As built the next gesture opens on the tick the last one
lands and covers the whole interval. Making recovery a real, separate span is a
change to how a fight *feels* and wants a number in operator settings before
anyone writes it — the plan puts it with Ф5, where the interval already gains a
second meaning.

**11. Two vocabularies collapse where a reader now wants them apart.**
`InterruptReason::Abandoned` covers disengaged, retargeted, died and logged out;
`Moved` covers walked, ran and rode. Both were left whole because nothing read
them, and the preparation bar now does: *"disengaged"* and *"died"* are one word
on screen. Splitting either costs one byte on the wire and no compatibility.
Beside it, `NoTarget` is a balk that can never be an interrupt and the type does
not say so — nothing stops a caller handing it to `end_combat_action`, where it
would be a sentence with no meaning.

**12. The armed bar drops the one number an armed action has.** `expires_at`
crosses the wire as the phase's own interval and the picture spends none of it: a
bow about to give out is drawn exactly like one just armed. Held rather than
filling is the right shape; *nearly out* is a real thing to say and nothing says
it.

**13. Small truths about the refusal, each written where it lives.** A target
both out of reach *and* behind a wall reports whichever `obstruction` tests first
(reach), which is the impact's own long-standing order and only visible now that
somebody reads the answer. `Balked` is no longer rare — a town square of guards
in war mode is one apiece — so anything that assumed it was should be re-read. A
concealed action still broadcasts its *end*, which is the over-broad audience the
commit had, from the other side. And nothing plays the ambusher's own stroke, so
their bar fills over a body standing still.

**14. Nothing consumes `Breath`'s `art`, and it is always the arrow graphic.** A
`RangedAttack` carries a reach and a damage kind but no picture, so a dragon's
breath crosses the gap drawn as an arrow. The field is on the action so the
impact stays self-describing; filling it needs a column on `RangedAttack`, which
is a content question.

**15. The pictures the loop does not draw.** A per-stranger health bar *window*
(the reference opens one by dragging a name-plate, a gesture this client has
neither half of); damage numbers over heads (ServUO's `DamagePacket` `0x0B` does
not exist in `openshard_protocol` at either end); the buff-icon button on either
status frame, which wants a buff window and a `0xDF`; a corpse's own name, which
arrives only in answer to a single click this client never sends; and exact
per-weapon and per-body `0x6E` actions — the classic-packet action is a coarse
humanoid/creature split where ServUO computes it from the body tables. The modern
`0xE2` path is already exact, so this refines the minority client.

**16. The skills that are named and not built.** Inscribe (wants a writable book
to copy), Tracking (two gumps and the `0x9A` quest-arrow packet), Camping (wants
a reason to light a fire — logging out safely in the wild — more than it wants
the fire), and taming's three: stabling (which wants a pet saved with no
position, the logged-out-character shape), loyalty (pointless without feeding)
and Herding. Beside them, the bard's flat thirty-second effects want per-target
duration scaling, and Discordance wants its AoS/SE resistance-mod form.

**17. Numbers nobody has watched, and tests nobody has written.**
`gameplay.action_speed.shot = 64` was chosen to make one weapon at one dexterity
come to 1.6 seconds and nobody has fought at either end of the range it scales.
`RUNNING_GRACE` is three seconds and both halves of the sentence justifying it
are guesses. `Mounted` is charged at the step, so a rider who stands still is
neutral by omission rather than by decision. Nothing cuts a line of sight
mid-swing in a test — the fixture wants a wall between two fighters and the tick
tests have none outside housing scenes. Nothing tests that a bar lands over the
right head, for either bar. And no test drives a stock, non-OpenShard client past
the four new `0xBF` subcommands: the contract that unknown ones are skipped is
believed rather than exercised.

**18. The recorder cannot be lined up with the shard, and does not prove it sees
everything.** The wire carries no tick number, so the client can say when it
*learned* something and never when the shard *decided* it — a shard running
behind its rate and a packet delivered late are the same picture. `record_combat`
is a `match` with a `_ => {}` arm, which is exactly the shape that goes quietly
out of date. And the mark's snapshot is of your own body only, while the
interesting report is usually about somebody else.

**19. Corrections to records that would otherwise send someone to redo done
work.** The skills phase record asks for casting to be routed through
`break_cover` and says a trance survives a spell — `begin_cast` has called
`break_cover` since the reveal slice, and the magic record says so; the two
records disagree and the code is with the second. The same record and the magic
one call the unbuilt archetype `SpellEffect::Scripted`, which has been
`SpellEffect::Unimplemented` since the script-pack seam was retired: a reader
greps the name and finds nothing. Neither record is edited — a record is a
record — and this line is the correction.

**20. Smaller, and each written where it lives.** `Crowd` ages bodies and
corpses on two copies of one rate, which is not yet enough callers to be worth a
type. The preparation bar has no toggle, where every other overlay in this client
has one. Its outcome labels are hard-coded English, on the day this client grows
a string table. A server packet cannot be round-tripped inside a `protocol` unit
test, because `decode_packet` asks the client length table and `decode_server`
is private. And the debug assert walks every fighter every tick — bounded by
fighters rather than by mobiles, so affordable, but it is the first check in this
crate whose cost scales with the fight rather than with the defect.

## The documents

**Design** — the model as built, no status in them:

- [`design_fight_loop.md`](design_fight_loop.md) — war mode, the aim, the two
  health pictures, the animation as a one-shot, death in the tonemap, and the
  corpse as a body out of `anim.mul`. Eleven decisions, and the packet table the
  loop is made of.
- [`design_actions.md`](design_actions.md) — an action as an object on three
  axes: what the impact does, what releases it, what the world does to it in
  between. The four verbs, twelve decisions, and the four `0xBF` subcommands that
  carry a beginning, an end, a refusal and a stage.
- [`design_sight.md`](design_sight.md) — the ray a shot is allowed by: what stops
  it and at what height, why the boolean is a reading of the trace rather than a
  second walk, and the seven things the rule as built cannot do.
- [`design_skills.md`](design_skills.md) — three questions in one order, the
  total cap that makes a character a build, the four doors into a skill, and the
  four skills that turned out to be subsystems.
- [`design_magic.md`](design_magic.md) — one gate, sixty-four rows, the order of
  the refusals, the closed archetype list, and the six shapes of effect that
  outlive the cast.

**Evidence** — measurements and closed records; none of them is a status:

- [`evidence/2026-08-11-the-fight-loop-phases.md`](evidence/2026-08-11-the-fight-loop-phases.md)
  — the six phases that closed the client's half of the fight, and where two
  decisions came out differently once the code was in front of them.
- [`evidence/2026-08-24-the-combat-phase.md`](evidence/2026-08-24-the-combat-phase.md)
  — the roadmap's record of the fight itself: the swing formula per era, the
  weapon and armour tables, notoriety and murder decay, corpses, ghosts, and the
  four status-bar numbers that were constants.
- [`evidence/2026-08-24-the-skills-phase.md`](evidence/2026-08-24-the-skills-phase.md)
  — fifty-eight skills as data, the check and gain ported whole, and every skill
  as it landed, including the four subsystems.
- [`evidence/2026-08-24-the-magic-phase.md`](evidence/2026-08-24-the-magic-phase.md)
  — Magery family by family, from mana and the effect seam to the summons, travel
  and the three dispels.
- [`evidence/2026-08-27-the-action-phases.md`](evidence/2026-08-27-the-action-phases.md)
  — the deadline the object replaced, seven phases and five half-phases of what
  playing it found, and the whole backlog those found. The tick-pace measurement
  that turned out to be a world scan is in here.
- [`evidence/2026-08-27-the-ranged-shot.md`](evidence/2026-08-27-the-ranged-shot.md)
  — three unrelated gaps that made archery a skin on wrestling, and the one
  sprite in this renderer that is not tile-snapped.
- [`evidence/2026-08-27-the-sight-overlay.md`](evidence/2026-08-27-the-sight-overlay.md)
  — five phases of making a boolean legible, ending with the half of a refusal
  the first four never drew.

**Plans** — what is not built lives outside `docs/`:

- [`plans/combat/actions/PLAN.md`](../../plans/combat/actions/PLAN.md) — fatigue,
  reach as data, and the two watches that are not built.
- [`plans/combat/spells/PLAN.md`](../../plans/combat/spells/PLAN.md) — the eleven
  arms, the scroll, the creature that casts, and every named deferral the spell
  families left behind.
