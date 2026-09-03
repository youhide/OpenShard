# Skills: one check, one gain curve, and four ways to start one

Fifty-eight skills, and the engine has **one** answer to "did it work" and
**one** answer to "did it teach anything". Combat's to-hit, a mined ore, a cast
and a picked lock are the same two calls with different numbers, which is why
there is one gain curve on this shard and not four.

The crates, in dependency order — `skills` depends on `items` and `state` and on
nothing above it, which is the invariant that decides where several rules live:

| Where | What lives there |
|---|---|
| `state::skill` | what a skill *is*: client id, name and title, the stats it leans on and their weights, its gain factor, whether the window can use it |
| `skills::check` | `skill_value`, `roll_skill_band`, `roll_skill_chance`, `gain_chance` |
| `skills::stats` | the stat gain, the caps and the per-stat cooldown |
| `skills::button` | the skill window's button: who may use what, and the default for the rest |
| `skills::handlers::*` | one module per family, and one dispatch for the cursor's answer |
| `world::tick::skills_wire` | the `0x3A` traffic that shows a sheet to a client |

**This document is the model.** What is built and what is open is
[`README.md`](README.md); the record of every skill as it landed is
[`evidence/2026-08-24-the-skills-phase.md`](evidence/2026-08-24-the-skills-phase.md).

## Three questions, in this order

Every use asks the same three, and `skills::check`'s module doc is the canonical
statement of them:

1. **What is the skill worth?** `skill_value` — the trained base plus what the
   mobile's stats lend it, ServUO's `Skill.NonRacialValue`. It is a **read-site
   derivation**: nothing is mirrored onto the mobile, so a Strength spell raises
   a smith's effective skill for as long as it lasts, with nothing to undo when
   it expires. The bonus fades as the base rises and is capped by the row's own
   `stat_total`. From AoS on it is gone entirely — ServUO calls
   `AOS.DisableStatInfluences()` at startup, and here an `if` on the era says the
   same thing without a mutable table. `Discorded` is folded in at this one site,
   which is how a bard's discord makes a creature hit worse, resist worse and
   cast worse without combat, magic or the AI knowing what a lute is.
2. **Did it work?** `roll_skill_band` turns a difficulty **band** into a chance
   and draws against it. A band, not a number: under its lower edge you cannot,
   above its upper edge you learn nothing. That *is* the learn-from-a-challenge
   rule, which is why Sphere's separate `AdvRate` and `GainRadius` tables were
   dropped rather than deferred.
3. **Did it teach anything?** `gain_chance` averages the headroom under this
   skill's own cap **and** under the total one. The total cap is the point of the
   model: it is what makes a character a build rather than a list, and the engine
   had no notion of it before. With it come the rules that hang off it — a
   `Locked` skill holds, a `Down` skill gives ground so another can rise past the
   cap, and a creature is exempt as ServUO exempts it.

**Fixed point, never floats.** Skill values are tenths (`755` is 75.5), chances
per-mille, factors per-mille, gains in thousandths — because the tick replays and
`Rng` draws integers. ServUO's doubles are carried as integers throughout, and
where the two could differ the comment says which way and why.

**The randomness is the world's.** `Rng` (xorshift64\*) is a plain field the
world owns, seeded once and advanced only by the tick, so two identical runs
reach the same skill roll for roll — there is a test that asserts exactly that.

## Stats are the same shape

A mobile carries `Stats { strength, dexterity, intelligence }`, and
`skills::apply_stats` is the one door they change through, so the three pools
that derive from them — `Hitpoints.max` from strength, `Mana.max` from
intelligence, `Stamina.max` from dexterity — can never drift.

Stat *gain* is ServUO's two mechanics, chosen by era: before ML each stat rolls
its own weight from the skill's row (`StrGain / 33.3`); from ML one flat chance
picks the skill's primary stat three times in four. Per-stat and total caps bind,
a stat at the total cap takes its point from one set to fall, and a per-stat
cooldown — a tick count, so it replays — stops a flurry of uses pouring into one
stat. Three `StatLocks` ride the wire in both directions beside the skill locks.

## Four doors into a skill, and why there are four

A skill is started by whatever the *player's gesture* actually is, and there are
four different gestures. This is not four dispatchers; it is one pair of
functions (`handlers::start` for a use, `handlers::on_target` for the answer)
reached four ways.

- **The window's button** (`0x12` type `0x24`). `skills::button` runs ServUO's
  `Skills.UseSkill`: a ghost is silent, a use inside another's cooldown is
  refused out loud, and **the default for a bare skill is nothing, and saying
  so** — thirty-five of the fifty-eight are not usable this way (Tactics is
  passive, Mining wants a pickaxe, Magery wants a spellbook) and the client has a
  line for exactly that case, cliloc 500014. It is the answer, not a gap.
- **A double-click on the tool** — Healing, Veterinary, Lockpicking, Mining,
  Lumberjacking, Fishing, Musicianship. The action that uses the skill *is* the
  double-click, so it comes through `world::tick::skills_wire`'s `use_item_skill`,
  **after** the `ItemUsed` the pack sees: default in core, customise in the pack,
  in that order.
- **An ordinary interaction that happens to be a skill** — Snooping has no button
  at all, because the action that uses it is a double-click on a container inside
  somebody else's pack.
- **A passive read** — Tactics, Anatomy in a damage roll, `MagicResist` when
  something is cast at you. Nothing starts these; a rule elsewhere asks
  `skill_value` and rolls.

**The cursor is one seam.** Pressing a button rarely *does* anything: it asks a
question. An object or location cursor goes up, the world remembers which skill
asked (`TargetPurpose::Skill`, `SkillSecond`, `Harvest`), and the answer arrives
a packet later with its reach re-checked server-side. The prompt cliloc and the
reach are **per skill**, in a table rather than one shared range: Arms Lore
reaches 2 tiles, Item ID 8, Forensics 10. Poisoning is the engine's only
two-cursor skill — the potion, then the blade — which is what `SkillSecond` is
for.

A skill missing from the handler table is one whose core behaviour is not built:
it still passes every gate and still announces `SkillRequested`. That is the
difference between "the core has no opinion" and "the client cannot do this", and
the two are decided a step apart.

## A skill that is really a subsystem

Four of them turned out to be subsystems rather than handlers, and each is
recognisable by the same signature: **one gate everybody reads, and one call that
breaks it**, so the systems that trip it need not know it exists.

- **Stealth.** `Hidden` and `Stealthing { steps_left }` live in `state`, read by
  the *one* gate `WorldState::can_see_mobile` (where `Ghost` already lives) and
  broken by the *one* call `WorldState::break_cover` — ServUO's
  `RevealingAction`, whose last line is `DisruptiveAction`, so the two are one
  call here as they are there. That is what lets attacking, speaking, lifting and
  casting each give a hider away and end a trance without any of them knowing
  what hiding is.
- **Bard.** `state::instrument` is the table, an `Instrument { uses_left }` on
  the item is spent by every attempt, and the three skills share a bard range, a
  **Musicianship check before the skill's own roll** — which is what makes
  Musicianship worth training alone — and one `base_difficulty` computed from the
  target's pools and skills rather than a fixed band. The two lasting effects are
  components with a tick expiry and neither is folded into anything: `Pacified`
  is read where a blow would land and where the AI decides; `Discorded` is read
  in `skill_value`.
- **Taming.** A `Pet { owner, slots, order, order_target }` on the creature and a
  `Tamable { min_skill, slots }` for the kind, over a core table keyed by body
  that a spawn may override — and **every rideable body is tamable**, derived
  from the mount table rather than listed twice. A pet decides nothing:
  `ai::pet_beat` carries out its last order and returns a direction, so a pet
  moves through the same `step` a wild creature uses, and an attack order points
  the `Combat` component the AI already drives. Follower slots are a read-site
  derivation (`skills::followers_of`), so the status bar and the taming refusal
  can never disagree — and neither can the summoning cap, which reads the same
  number.
- **Harvest.** The four definitions are core data in `state::harvest`; a bank
  belongs to the *ground* (`Banks` on `FacetState`, beside the sector grid and
  the obstruction index) rather than to an entity, and is **deliberately not
  persisted**, as ServUO does not persist it. What *is* stable is the vein's
  position: where ServUO seeds a `Random` with `(x*17)+(y*11)+(map*3)`, this
  hashes the same three inputs, because a bank that is not saved must still find
  the same ore under the same block after a reboot.

## Where a skill's *result* is applied, and why it is not here

`skills` sits below `combat`, `magic` and `npc`, so a skill that needs one of
them **returns an intent** and the tick applies it through the crate that owns
the door:

- a Poisoning fumble is *decided* in `skills`, emitted, and applied by the tick
  through `combat::apply_poison`, because applying poison is combat's door;
- Stealing returns the theft, because moving an item is `items`' door and
  flagging a criminal is `combat`'s;
- Taming's success calls `npc::tame`, because `npc` owns what a creature *is*;
- a Remove Trap trigger lives in `world::tick::traps` rather than in `items`,
  because the damage has to go through `combat::damage` and `items` cannot depend
  on `combat` without closing the `skills → items → combat → skills` loop;
- fame and karma live in `state::title` for the same reason: `skills` awards them
  and `combat` cannot be depended on.

Two things moved *down* into `state` under the same rule, because the data has
several readers while the rules have one owner: `state::weapon` and
`state::armor` (Arms Lore reads the rows combat swings and absorbs by), and
`worn_armor_rating` beside them, which Stealth, Meditation and the status bar all
ask.
