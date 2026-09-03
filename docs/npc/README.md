# People and creatures: where they stand

The canon of the `npc` domain — `crates/server/npc`, `ai`, `chat`, `party`,
`guilds` and `quests`, plus the parts of `server/state` they own (`Brain`,
`Aggression`, `Route`, `Npc`, `Title`, `Pet`, `Summoned`, `Guard`, `Guilds`,
`Alliance`, `Party`, `QuestDef`), the staff command layer in `world::gm`, and the
six passes in `world::tick` that drive them (`spawners`, `wake`, `speech`,
`regions`, `party`, `guilds`). This is everything in the world that is **not the
player and not scenery**: a thing that decides, that says something, that
remembers you, or that belongs to a group.

What a creature *fights* with belongs to [`combat/`](../combat/README.md); the
map it walks and the regions it stands in belong to
[`world/`](../world/README.md); what it carries and what it drops belong to
[`items/`](../items/README.md).

**One entry point.** This page answers "what is alive on this shard today" and
says which document holds the reasoning for each line. Where this page and a
design document disagree, the design document is right and this page is stale.

## The one-line answer

**Everything alive here decides on a beat, and only ever returns a direction.**
The brain, the townsperson and the pet all share one seam — read the world,
decide, hand back at most one step — and the world does the stepping. That is
what lets ten thousand creatures and seven hundred townsfolk run in the same
tick, because a decision that returns instead of acting is a decision that can be
*skipped* when nobody is watching.

The second half is that nothing here is told anything. Progress is found, a
region crossing is found, a cancel is found: every reader diffs the world once a
tick rather than being called beside each mutation, because a call beside every
mover is a call somewhere it is forgotten.

```text
  a mobile ── Brain { sight, wander } ── Aggression ──▶ ai::think_one ──▶ Direction
     │                                                       the tick steps it
     ├─ Npc { home, wander } ── Title ──▶ speech.json    the townsperson's beat
     ├─ Pet { master }  (+ Summoned { until })           somebody's creature
     └─ Guard { until }                                  a sentence with a timer

  a player ─┬─ Party — keyed by the leader's serial ─┐
            ├─ Guild ── Alliance                     ├─▶ a line to a set of people
            └─ QuestLog ◀── QuestGiver on an NPC     ┘   (never over a head)
```

## What the area is, by part

| Part | State | What is left | Held by |
|---|---|---|---|
| A creature that notices, chases, fights, flees and drifts, on a seeded beat | ✅ shipping | — | [`design_brain.md`](design_brain.md) |
| Aggression postures — passive, defensive, aggressive — and the brave-hits floor | ✅ shipping | — | the same |
| Line of sight as the acquisition gate, one ray shared with the ranged shot | ✅ shipping | — | the same, and [`combat/design_sight.md`](../combat/design_sight.md) |
| Two searches: the bounded exact one, then the baked coarse graph, refined live | ✅ shipping | a chase still plans onto the quarry's own tile — row 4 | [`design_brain.md`](design_brain.md) § two searches |
| A kept route, keyed by whether the goal moves, and a remembered refusal | ✅ shipping | — | the same, § a route is kept |
| Ranged creatures that volley and hold a gap | ✅ shipping | the kiting livelock — row 2 | the same, § postures |
| Level of detail, and the sector-crossing wake that has to come with it | ✅ shipping | — | the same, § level of detail |
| Spawn regions: jittered, tallied in one sweep, dormant where nobody is | ✅ shipping | — | [`evidence/2026-08-24-the-ai-phase.md`](evidence/2026-08-24-the-ai-phase.md) |
| A townsperson with a body, a name, a trade, a post and a beat | ✅ shipping | three cooldowns are running at half their stated span — row 3 | [`design_townsfolk.md`](design_townsfolk.md) |
| Keyword answering, greetings and barks, per trade, from `speech.json` | ✅ shipping | — | the same, § what it answers |
| The banker and the bank box | ✅ shipping | — | the same, § the banker |
| The shopkeeper: buy, sell, restock, and access refused at all four doors | ✅ shipping | — | the same, § the shopkeeper |
| A townsfolk routine off the world clock, with derived night homes | ✅ shipping, off by default | — | the same, § the routine |
| Guards, as a sentence rather than a fight | ✅ shipping | — | the same, § the guard |
| Pets: tamed, controlled, ordered, counted against followers | ✅ shipping | stabling, feeding, loyalty, Herding — row 5 | [`plans/npc/pets/PLAN.md`](../../plans/npc/pets/PLAN.md) |
| Summons, as a pet with a deadline | ✅ shipping | — | [`design_townsfolk.md`](design_townsfolk.md) § pets and summons |
| Speech both ways, in four packets, with the encoder chosen by content | ✅ shipping | — | [`design_speech.md`](design_speech.md) |
| Whisper, talk and yell as operator settings; the dead unheard | ✅ shipping | — | the same |
| The `.`-prefixed staff layer, gated in the world and not in the commands | ✅ shipping | — | the same, § the staff layer |
| Parties: invite, accept, leave, kick, chat, the loot flag | ✅ shipping | the loot flag has no consumer — row 7 | [`design_groups.md`](design_groups.md) § a party |
| Guilds: five ranks, war, named alliances, guild and alliance chat | ✅ shipping | the guildstone is not an item — row 10 | the same, § a guild |
| Notoriety answered per viewer, murderer and criminal before guild | ✅ shipping | — | the same, § notoriety |
| Quests: four objectives, a log, the ported window, turn-in, escorts | ✅ shipping | reward choice, chains, two objective kinds, the converter — row 6 | [`design_quests.md`](design_quests.md) |
| A creature that casts a spell: mana and a repertoire as spawn data, a cast branch above the melee one, the player's own cast sequence below it | 🟡 C1 built | it throws the strongest thing it can afford and nothing chooses by *category* — heal, curse, escape are C2; the cadence clauses beyond rooting are C3 — row 1 | [`plans/npc/creature_casting/PLAN.md`](../../plans/npc/creature_casting/PLAN.md), [`evidence/2026-09-03-a-creature-that-casts.md`](evidence/2026-09-03-a-creature-that-casts.md) |
| A channel window (`0xB3`/`0xB5`) | ⬜ not built | row 8 | — |
| The Town Crier | ⬜ not built | row 9 | — |

## What is enforced, and by what

- **A brain never moves anything.** `think_one` returns a direction and the tick
  calls `step`; there is no creature movement path and no creature attack path —
  a creature swings through `combat::swings` with the `Combat` a player's attack
  builds. `a_frozen_creature_does_not_step` and
  `an_aggressive_creature_chases_a_player` sit on the two halves of that seam.
- **Acquisition is a ray and not a radius.**
  `a_creature_does_not_notice_prey_through_a_shut_door` is the whole statement: a
  shut door is opaque, so closing one behind you does something.
- **Three route constants are `pub` because a test has to name them.**
  `PATH_BUDGET`, `REPATH_TICKS` and `REFUSAL_TICKS` are public not as knobs but
  because `a_creature_routes_past_its_exact_budget_over_the_coarse_graph`,
  `a_planned_route_is_walked_rather_than_planned_again`,
  `a_route_to_a_place_is_walked_past_the_window_that_would_have_lapsed`,
  `a_refused_long_route_is_remembered_until_it_lapses` and
  `a_body_that_did_not_move_plans_its_route_again` each have to wait exactly one
  of those spans — and a copy of the number in a test is a second place to change
  it.
- **A crowd does not beat in lockstep, and the jitter has one home.**
  `next_beat` is the only place a beat is armed, because it had been forgotten at
  four of them (the restore path, the beat itself, the LOD doze and the creature
  brain). `a_crowd_of_townsfolk_does_not_beat_in_lockstep` and
  `restored_townsfolk_do_not_all_beat_on_the_same_tick` are the two ways a crowd
  gets welded together.
- **LOD is six tests and each is a different failure.**
  `lod_off_a_far_creature_still_ambles` (the flag is opt-in),
  `lod_a_far_creature_dozes` (it works), `lod_a_near_creature_thinks_at_full_rate`
  (the radius), `lod_an_engaged_creature_keeps_simulating` (a fight must not
  freeze), `lod_walking_into_a_sleeping_town_wakes_it` (the missing half), and
  `lod_a_spawner_with_no_player_near_stays_dormant_then_wakes`.
- **A keyword is a whole word, and a bare one needs the vendor named.**
  `a_banker_keyword_is_a_word_and_not_a_substring`,
  `a_trade_answers_its_own_keyword_and_only_within_earshot`, and
  `a_shop_keyword_needs_the_vendor_named_and_an_empty_sell_answers_overhead`.
  The middle one is the whole reason the earshot bound is in the port.
- **A town is dressed by dice that replay.**
  `the_same_seed_dresses_the_same_townsperson`, `no_two_items_land_on_one_layer`,
  `a_skin_hue_is_a_partial_hue`, `a_woman_wears_no_beard_and_a_man_may`,
  `barefoot_is_possible_and_shoes_are_the_default`, and — for the other half of a
  label — `the_two_lists_do_not_name_the_same_person` and
  `the_lists_are_wide_enough_for_a_whole_facet`.
- **A spawn stands on the floor**, not at whatever z the data named:
  `a_spawn_stands_on_the_floor_not_under_it`. And an unnamed creature reads:
  `an_unnamed_creature_takes_its_body_default_name`.
- **A guild line is never said out loud.** `speech_range` answers zero for the
  guild and alliance modes and `World::say` branches before it measures
  anything, so a routing failure is silence; `a_guild_line_is_not_said_out_loud`
  is the test that a private line stays private.
- **A red cannot hide inside a tabard.**
  `a_guildmate_is_green_and_a_guild_at_war_is_orange` and
  `a_murderer_stays_red_inside_a_guild_tabard` pin the resolution order, which is
  the one thing about relative notoriety that a rewrite could silently invert.
- **A guard is a rule about a place, not about a mobile.**
  `calling_the_guards_kills_a_criminal_in_a_guarded_town`,
  `the_guards_do_not_touch_the_innocent`,
  `the_guards_are_not_called_outside_a_guarded_region`,
  `staff_are_never_guard_candidates`,
  `a_murderer_walking_into_town_is_hunted_without_a_call`, and
  `a_guard_earns_no_murder_count_and_leaves_when_it_is_done`.
- **Quest progress is found and attributed.**
  `obtain_progress_is_found_by_the_diffing_pass_not_announced`,
  `a_slain_body_advances_only_the_killers_objective`, and
  `an_unattributed_death_advances_nothing`.
- **A giver survives a restart, and a restore is not a spawn.**
  `a_quest_giver_is_still_a_giver_after_a_restart`,
  `restoring_a_mobile_announces_it_as_restored_not_as_spawned`,
  `a_restore_announces_the_post_an_npc_belongs_to_not_where_it_wandered`, and
  `a_quest_log_survives_a_restart_with_its_progress_and_cooldowns`. The second is
  the shape of the defect that made every quest on the shard work exactly once.
- **A window's reply is judged against what the server drew.**
  `a_reply_to_a_gump_that_was_never_opened_does_nothing`, and the page a button
  was clicked on comes from the server's memory rather than the packet.
- **A turn-in is all or nothing**:
  `a_player_one_item_short_loses_nothing_and_is_paid_nothing` and
  `a_rewarded_quest_without_a_backpack_stays_active_and_charges_nothing`.
- **A shelf remembers what full meant.**
  `a_bought_out_shelf_refills_when_its_hour_is_up` and
  `a_vendor_and_its_priced_stock_survive_a_restart`; the hours are
  `a_shop_that_keeps_hours_is_shut_after_them` against
  `a_shard_with_no_schedule_never_closes`, which is the flag being genuinely off.
- **The shipped region data reaches the world**, not just a fixture:
  `every_region_the_tree_ships_reaches_the_world`, the same argument the housing
  domain's one facet-sized test makes.
- **94 `#[test]` functions inside the six crates** — 47 in `guilds`, 21 in
  `party`, 18 in `npc`, 5 in `quests`, 2 in `chat`, 1 in `ai` — plus the ones
  that need a world: 28 in `world/src/tick/quest_tests.rs`, 16 in
  `region_tests.rs`, and the creature, townsfolk, vendor, LOD and route groups in
  `world/src/tick/tests.rs`. The lopsided counts are honest rather than a gap:
  `guilds` is nearly all rules and can be tested as rules, and `ai` is nearly all
  *world*, so its tests live where a world does.

## What is open, ranked

**1. A creature casts, but it does not choose.** The magic domain stopped being
one-directional on 2026-09-03: a creature carries a `Mana` pool and a
`Repertoire` from its spawn data, `fight_phase` throws before it closes, and the
cast goes through the sequence a player's does — see
[`evidence/2026-09-03-a-creature-that-casts.md`](evidence/2026-09-03-a-creature-that-casts.md).
What is left is the half the plan calls the decision. It throws the **strongest
spell it can afford that is aimed at a mobile**, which means nothing heals, buffs,
curses or teleports away: a healing dragon is still impossible, and a lich at one
hit point fights on rather than escaping. Those are categories, and the category
choice is C2. C3 is the rest of the cadence — a caster is rooted already, but LOD
may still doze one mid-cast and the determinism claim has no test.
[`plans/npc/creature_casting/PLAN.md`](../../plans/npc/creature_casting/PLAN.md).

**2. 🚩 A kiting archer can livelock, and that is how a tick-rate change was
caught.** A turn costs a whole beat (the motion path's turn-as-step) and combat
re-faces a fighter at its target before each swing, so where the swing is quicker
than the beat the creature spends every beat turning round and never opens the
gap. The fixture that found it now states 500ms rather than a tick count, which
hides it again; **the rule that a turn and a step compete for one beat is the
real thing to look at**, and it is shared with `world`'s motion path rather than
being this domain's alone.

**3. 🚩 Three of this domain's own constants are still bare tick counts, and each
is running at half its documented span.** `GREET_COOLDOWN`,
`GREET_COOLDOWN_JITTER` and `BARK_COOLDOWN`
([`npc/src/live.rs:91`](../../crates/server/npc/src/live.rs#L91)) are written as
`seconds * 20`, and `TICKS_PER_SECOND` is 40 — so a townsperson greets every 7.5
seconds where its own doc comment says fifteen, and barks every 30 where the
comment says a street of shopkeepers shouting every sixty would be worse than
silence. `BEAT_TICKS`, `IDLE_TICKS`, `RESTOCK_TICKS` and `SUMMON_SWING_TICKS` in
the same crates were converted; these three were missed. The fix is the
conversion the others took, and it is the concrete instance of the backlog's
"every remaining bare tick count is a latent one of these".

**4. A chase plans onto the quarry's own tile and stops one short by the reach
check.** Adjacent-tile pathing is the remaining refinement from the A\* work: the
route should be *to a tile beside* the goal, which is also the shape a delivery,
an escort and a pet following an owner all want. The crowd rule already drops the
goal tile, so half the argument is made.

**5. 🚩 A pet cannot be put away, fed, or lost.** Taming resolves and a pet
follows, obeys and counts against a follower slot, but there is no stable, no
food, no loyalty and no Herding — and loyalty without feeding is a number that
only ever goes one way, which is why the two are one phase.
[`plans/npc/pets/PLAN.md`](../../plans/npc/pets/PLAN.md).

**6. The quest model's other half is structural rather than per-quest**: reward
*choice*, chains, `ApprenticeObjective`, the question-and-answer objective, the
staff force-complete button, and the converter pass over the reference's own
quest subclasses — which is possible for the first time now that the model
matches theirs. [`plans/npc/quests/PLAN.md`](../../plans/npc/quests/PLAN.md).

**7. The party loot flag has no consumer.** `WorldState::party_may_loot` answers
and nothing asks, because corpses on this shard are open to anybody: there is no
criminal-act rule on looting one for a party to be exempt from. The missing half
belongs with the criminal system rather than here, which is why this is a row and
not a plan.

**8. Chat channels (`0xB3`/`0xB5`) are untouched, and are not the party.** Party,
guild and alliance chat are lines addressed to a *group you are in*; a channel is
a window you join. The router they were built on is the right substrate for it,
but the window, the channel list and the two packets do not exist.

**9. The Town Crier is the reference's real source of street noise and is not
built.** Barks cover a shopkeeper talking about its own stock; a crier wants a
news queue and a staff gump, which is a feature rather than a line of dialogue.

**10. The guildstone is not a placeable item.** Everything a guildstone does is
reachable from the paperdoll's Guild button, so this is presentation rather than
capability — but a guild with no object in the world is one a passer-by cannot
discover.

**11. A name on the guild roster resolves only while its owner is logged in.** A
serial resolves to an entity and an offline character has none, so the fallback
is the serial. **The house sign has the identical gap and the identical fix** — a
name read off the character store — and neither has it; whichever domain does it
should do it once.

**12. The Felucca converter drops creatures whose body is not a literal**, and
the loudest loss is not a creature at all. `resolveBody` reads a literal body, a
`RandomList`, a `SetBody` or the first element of a mount table, so a healer
whose body is set indirectly, the odd-cased mounts, and — worst — the **camp
meta-spawners** fall through: a camp has no body of its own, so *its* creatures
are lost with it. Where a body does resolve, a `RandomList` keeps only the first
and the hit-point and damage ranges are averaged.

**13. Converter notoriety is a karma-sign heuristic**, not the reference's full
alignment and fame computation — negative karma reads enemy-orange, everything
else grey. Fame and karma themselves are built, so this is the converter's gap
alone and not the model's.

**14. Town NPC types with no vendor class and no shop are skipped by the
converter**, which is where the escortables and the Bard-Mastery knights land
today. They are exactly the mobiles a quest giver would be bound to, so this row
and row 6's converter pass are the same work approached from two ends.

**15. Three tables share the `body` key and are three files.** `body_types.json`
answers what *type* a body is, `creature_names.json` what it is *called*, and
`creature_sounds.json` what it *sounds* like — and `creature_base_sound`'s own
doc says "grow it alongside `creature_name`", which is an invariant stated in
prose because nothing enforces it. They were separated on purpose: the three
disagree about which bodies share a row (three wolves are three names and one
howl) and the sound rows carry notes the others have no column for. One file with
three optional columns would end the drift, at the cost of a format that can say
"these four bodies share a sound but not a name".

**16. What the regions phase deferred is not this domain's to close.** The record
lives here because the guard does; the rest of its deferred list belongs
elsewhere and is named so nothing is silently dropped — `0x65` weather and a
calendar that turns the season are `protocol` and `world`, per-region light for
creatures (only players are told) is `world`, and `RegionFlags::safe` is already
ranked in [`housing/README.md`](../housing/README.md).

## The documents

**Design** — the model as built, no status in them:

- [`design_brain.md`](design_brain.md) — what a creature decides on its beat: the
  four phases, the two searches, the kept route and the remembered refusal, the
  postures, and the level of detail with the wake that has to come with it.
- [`design_townsfolk.md`](design_townsfolk.md) — what makes a town people rather
  than props: the base, the beat that translates instead of pirouetting, the
  dress roll, the name, the shop and its four doors, the routine and its derived
  night homes, and the guard that is a sentence.
- [`design_speech.md`](design_speech.md) — a line and who hears it: four packets
  with the encoder chosen by content, range as a mode, the hearing gate, the
  three lines that go to a set of people, and the staff layer.
- [`design_groups.md`](design_groups.md) — a party keyed by its leader, a guild
  whose five ranks are not nested, and an alliance that is a named object rather
  than a fact about a pair.
- [`design_quests.md`](design_quests.md) — the model against the content, four
  objectives each found rather than announced, and a window whose button numbers
  are copied on purpose.

**Evidence** — records and measurements; none of them is a status:

- [`evidence/2026-08-24-the-ai-phase.md`](evidence/2026-08-24-the-ai-phase.md) —
  the brain, the behaviours, the LOD benchmark and the whole-facet populate it
  was built for.
- [`evidence/2026-08-24-the-chat-and-administration-phase.md`](evidence/2026-08-24-the-chat-and-administration-phase.md)
  — the widest of the gameplay phases: speech, the staff layer, the `.admin`
  gump, the townsfolk, and a set of rows that belong to other domains.
- [`evidence/2026-08-24-the-regions-and-guards-phase.md`](evidence/2026-08-24-the-regions-and-guards-phase.md)
  — a place had to exist before anything could be true *there*.
- [`evidence/2026-08-24-the-guilds-phase.md`](evidence/2026-08-24-the-guilds-phase.md)
  — the ranks, the handshake, and the alliance that used to be pairwise.
- [`evidence/2026-08-24-the-parties-and-quests-phase.md`](evidence/2026-08-24-the-parties-and-quests-phase.md)
  — the router party was built to be, and the three things a client found wrong
  with a pack-first quest system.

**Plans** — what is not built lives outside `docs/`:

- [`plans/npc/creature_casting/PLAN.md`](../../plans/npc/creature_casting/PLAN.md)
  — mana, a choice, and a cadence that does not fight the tick.
- [`plans/npc/pets/PLAN.md`](../../plans/npc/pets/PLAN.md) — stabling, feeding
  and loyalty, and Herding.
- [`plans/npc/quests/PLAN.md`](../../plans/npc/quests/PLAN.md) — reward choice,
  chains, the two missing objectives, and the converter pass.
