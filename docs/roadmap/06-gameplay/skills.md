# Skills

[Gameplay index](README.md) · [Roadmap](../README.md) · [Backlog](../backlog/README.md)

- [x] `skills` — the table, the check, the gain
  - [x] **The fifty-eight skills are data now** (`state::skill`, ported whole from
    ServUO's `Server/Skills.cs`): each skill's client id, its name and title, the
    stats it leans on and the weight it lends each of them, its gain factor, and
    whether it can be used from the window at all. Fixed point, not floats —
    scales in hundredths, gains in thousandths, factors per-mille — because the
    tick replays. **This turned up a real bug:** five of the eight skill ids
    combat used were wrong (Fencing on Cooking's, Macing on Discordance's,
    Tactics on Poisoning's, Wrestling on Tailoring's, Swords on Mace Fighting's).
    They are the client's own `skills.mul` indices and they ride the `0x3A` both
    ways, so a swordsman's gains showed on the Mace Fighting bar. Nothing noticed:
    a roll trains whatever id it is handed.
  - [x] **The check and the gain are ServUO's.** Sphere's `Calc_GetSCurve` against
    a single difficulty is gone, and so is the flat linear gain that stood in for a
    curve. In its place: `CheckSkill` over a difficulty **band** — under it you
    cannot, at it you learn nothing — and `GetGainChance`, which averages the
    headroom under the skill's own cap and under the **total** one. That total cap
    (700.0) is the point: it is what makes a character a build rather than a list,
    and the engine had no notion of it. With it come the rules that hang off it —
    a `Locked` skill holds, a `Down` skill gives ground so another can rise past
    the cap, and a creature is exempt as ServUO exempts it.
  - [x] **Stat gain**, in both of ServUO's mechanics: before ML each stat rolls its
    own weight from the skill's row (`StrGain / 33.3`), from ML one flat chance
    picks the skill's primary stat three times in four. Per-stat and total caps
    bind, a stat at the total cap takes its point from one set to fall, and a
    per-stat cooldown (a tick count, so it replays) stops a flurry of uses pouring
    into one stat. Three `StatLocks` of their own, on the wire in both directions.
  - [x] **A skill is worth more than it is trained.** `skill_value` is ServUO's
    `Skill.NonRacialValue`: the base plus what the mobile's stats lend it, fading
    as the base rises and capped at the row's own ceiling. A **read-site
    derivation**, so a Strength spell raises a smith's effective skill with no
    bookkeeping and nothing to undo. Gone from AoS on, as
    `AOS.DisableStatInfluences` makes it. The `0x3A`'s `value` and `base` are two
    different numbers at last — they had carried the same one since the beginning.
  - [x] **A seeded generator in the world.** A roll is randomness inside a tick,
    and the tick must replay. So `Rng` (xorshift64\*) is a plain field the world
    owns, seeded once from a fixed default and advanced only by the tick — two
    identical runs reach the same skill, roll for roll (there is a test that
    asserts exactly this).
  - [x] **stats** (str/dex/int). A mobile carries `Stats { strength, dexterity,
    intelligence }`; `enter` gives a character the classic 100/100/100 and derives
    its `Hitpoints.max` from strength, `Mana.max` from intelligence and
    `Stamina.max` from dexterity. `skills::apply_stats` is the one door they change
    through, so the three pools can never drift from them.
  - [x] **The skills window on the client** (`0x3A`, both ways from ServUO's
    `SkillUpdate`/`SkillChange`), with per-skill caps and the lock arrows, and the
    status bar's three stat arrows beside it (`0xBF 0x1A` in, `0xBF 0x19` type 2
    out — relayed, unlike a skill arrow, because nothing else sends the stat bits
    and a client that never gets them draws all three pointing up).
  - [x] **The window's buttons work** (`0x12` type `0x24`). It was decoded, tested
    and routed nowhere, so pressing a skill did nothing at all — no message, no
    error, nothing in a log. Now it runs ServUO's `Skills.UseSkill`: a ghost is
    silent, a use inside another's cooldown is refused out loud (cliloc 500118),
    and the thirty-five skills that cannot be used this way get the client's own
    line for it (**cliloc 500014**), which is the right core default and not a gap.
    The twenty-three that can emit a `SkillRequested` for the pack *and* run the
    core's own handler — the "default in core, customise in the pack" split spells
    and loot have.
  - [x] **The cursor seam**, and the first two skills through it. An object cursor
    (`0x6C` type 0) goes up, the world remembers which skill asked
    (`TargetPurpose::Skill`), and the answer reaches the skill a packet later, its
    reach re-checked server-side. **Anatomy** and **Evaluating Intelligence** are
    done and set the shape: a margin of error narrowing with skill, a roll that
    both decides and trains, and an answer chosen by arithmetic on a base cliloc
    (`1038045 + strength*11 + dexterity`), drawn over the thing looked at and sent
    to one connection. Adds `encode_localized_message` (`0xC1`) — whose arguments
    are UTF-16 **little-endian**, the opposite of the `0xAE` a few lines above it
    in the same file.
  - [x] **The gear tables are data, in `state`.** Arms Lore needs a weapon's kind
    and damage and an armour piece's rating — the same rows `combat` reads to swing
    and to absorb — so the tables moved down to `state::weapon` and `state::armor`,
    the `state::title`/`combat::titles` split already in the tree: data below,
    rules in the crate that owns them. `equipped_weapon`, `swing_ticks` and
    `absorb_physical` did not move. The weapon table grew ServUO's `WeaponType`
    (Slashing/Piercing/Bashing/Axe/Polearm/Staff/Ranged), which is *not* derivable
    from the skill column — a war axe is an axe that bashes, a dagger a knife that
    pierces, and Arms Lore reads five different cliloc blocks off exactly that.
  - [x] **And the tiledata layer byte is read at last.** Whether a weapon takes
    both hands is in `tiledata.mul` — the *quality* field, which ServUO reads
    straight into `Layer` (`BaseWeapon`: `Layer = (Layer)ItemData.Quality`) — and
    this reader dropped it. It is `StaticTile::layer` and `Terrain::item_layer`
    now, pinned against a real file. Six weapon classes override it in code and
    only those six carry a `WeaponData::hands`, because measured against a real
    `tiledata.mul` the file is simply **wrong** about them: it files the bow, the
    crossbow, the heavy crossbow, the battle axe and the war hammer as one-handed.
    That is why the fact is read from the client *and* overridable, rather than
    either alone.
  - **The other twenty-one usable skills.** In rough order of what they cost:
    - [x] **Arms Lore, Item Identification and Forensic Evaluation.** The same
      shape as Anatomy and Eval Int, over three different subjects, so the handlers
      split by what they read: `handlers/lore.rs` (a living body),
      `handlers/appraise.rs` (an object), `handlers/forensics.rs` (a crime). The
      cursor's **prompt and reach are per skill** now (a table, not one shared
      range): Arms Lore reaches 2 tiles, Item ID 8, Forensics 10, each with
      ServUO's own prompt cliloc, which the two skills that were already done had
      been sending none of.
      **Forensics needed the world to keep notes**, and that is the interesting
      part: a `Corpse` component (owner, killer, forensicist, looters) is written
      where a corpse is *laid* and a looter is recorded where an item is *lifted*,
      so the skill only reads what somebody else's rule already recorded — and it
      **persists** (schema v17), because a body lies for seven minutes and a shard
      restarts inside that window. The killer is kept as a **name**, not a serial:
      ServUO holds a live `Mobile` and reads `.Name` at examination time, which
      cannot answer once the killer has logged out, and a corpse outliving its
      killer's session is the ordinary case. Arms Lore's durability lines are
      deliberately absent (an item here has no hit points) and Item ID prices only
      what the pack priced — a guessed value would read as authoritative.
    - **Taste Identification** — lands with Poisoning below, because what it
      tastes *for* is the poison that slice adds.
    - [x] **Animal Lore**, once pets existed — which is exactly why it waited. Its
      three gates *are* the skill (under 100.0 only a tamed creature, under 110.0
      that or a tameable one, above it anything), and every one of them asks a
      question only the pet slice can answer. The window is ServUO's
      `AnimalLoreGump` in its ML frame through the typed `GumpLayout` builder, in
      **two pages rather than five**: this engine has the attributes and the combat
      ratings, and the three pages it drops are numbers nothing in the world sets
      yet — a column of dashes is worse than a page that is not there.
    - [x] **Meditation and Spirit Speak** — the two skills a mobile turns on itself,
      so pressing the button *is* the whole use and no cursor goes up.
      **Meditation** is one `Meditating` marker and no timer: what ends a trance is
      somebody doing something, and that is now a real seam — `WorldState::disrupt`
      (ServUO's `DisruptiveAction`) called from the step, the blow, the word and the
      lift, which is the same call list the stealth slice will reveal on. Its gates
      are ServUO's in order (busy 501845, body under a tenth 501849, at peace
      501846, hands not free 502626 — a spellbook allowed, a shield not).
      **And the trance had to be worth something**, so mana regen stopped being a
      flat sixty ticks for everybody and became ServUO's pre-AoS curve:
      `medPoints = (Int + Meditation)/2` from seven seconds a point down to three
      quarters of one, plus an **armour offset in seconds** — which is what makes a
      mage in plate regenerate like a warrior and the free-hands rule mean anything.
      The offset needed one more column of ServUO's armour data
      (`MedAllowance`: leather `All`, studded `Half`, metal `None`), and the per-mobile
      rate is **stateless** — a mobile gets its point when the tick counter divides
      its *own* rate, so nothing is stored and nothing is saved.
      **Spirit Speak** is the pre-AoS form: `HearsGhosts { until }` for
      `base/50*90` seconds (floor fifteen), and the gate it feeds is a *second*
      predicate — `can_hear_mobile`, not a relaxed `can_see_mobile`, because a ghost
      must stay invisible to the listener or contacting the netherworld would make
      the dead walk visibly among the living. It does not persist, being seconds long,
      like a cast in flight.
    - [x] **Poisoning and Taste Identification**, the two ends of one fact. A
      `PoisonCharges { level, charges }` on an *item* is both a bottled dose and a
      coating on a blade — ServUO tells them apart by what the item is, and so does
      this. Poisoning is the engine's only **two-cursor** skill (the potion, then the
      blade), which added `TargetPurpose::SkillSecond`; the potion is spent either
      way and leaves the empty bottle; a coated blade holds `18 - level*2` doses and
      `combat` spends one into whatever it cuts, through the one `apply_poison` door.
      A fumble under grandmaster can poison the poisoner — decided in `skills`,
      *emitted*, and applied by the tick through combat, because applying poison is
      combat's door and `skills` sits below it. Taste ID reads the same component; so
      does Arms Lore, which is ServUO's behaviour (a weapon master does not have to
      lick a sword). The four potions **share a graphic** (`0x0F0A`), so which poison
      a bottle holds cannot come from a core table: it is on the item, put there by
      the pack (`op_set_poison`) or a staff `.poison <level>`, and **persisted**
      (schema v18) for exactly the reason a spellbook's mask is.
      **Awarding fame and karma moved out of `combat` into `state::title`** with
      this: Poisoning costs twenty karma, and `skills` cannot depend on `combat`
      because `combat` already depends on `skills`. The file's own note had said a
      crate of its own "would depend on combat for its only input" — a kill stopped
      being the only input, so standing now lives beside the table it feeds.
    - [x] **Begging and Remove Trap.** Begging is ServUO's, with one deliberate
      change: its beggar takes a tenth of what is actually in the target's pack,
      because its NPCs carry pack gold — ours carry none and a corpse's gold is
      already invented at death, so a townsperson gives from a notional purse and a
      *vendor* refuses (its till is a stock crate, not a purse). The karma cost is
      exact: up to forty, down to a floor of −3000, which is what stops the loss
      running away and a career beggar being free. It also added the two small
      substrate pieces it needed — `WorldState::face_toward` (two people talking
      face each other, ServUO's `GetDirectionTo`, which moved `direction_toward`
      down into `movement` beside its inverse `step_from`) and an `Action::Bow`.
      **Remove Trap** brought traps with it: a `Trap { kind, power, level }` on a
      container, ServUO's four kinds and their damage, sprung when the chest is
      opened by anyone but staff (a sprung trap hurts, it does not bar the lid) and
      taken off by the skill. The trigger lives in `tick/traps.rs` rather than in
      `items`, because the damage has to go through `combat::damage` and `items`
      cannot depend on `combat` without closing the `skills → items → combat →
      skills` loop. Neither reference traps anything in Britannia's own data, so —
      exactly like the `Lock` slice before it — it ships with a staff `.trap` and a
      path to pack data rather than as a rule nothing can reach. It **persists**
      (schema v19): a restart that quietly disarms every chest on the shard is the
      same silent loss as one that forgets a lock.
    - **Inscribe** — the last of the six, and the one that wants a writable book
      to copy.
    - [x] **Stealth is a subsystem, not a skill** — and it landed as one. `Hidden`
      and `Stealthing { steps_left }` live in `state`, read by the *one* gate
      `WorldState::can_see_mobile` (where `Ghost` already lives) and broken by the
      *one* call `WorldState::break_cover` (ServUO's `RevealingAction`, whose last
      line is `DisruptiveAction` — so it disrupts a trance too, and the two are one
      call here as they are there). That is what lets attacking, speaking and
      lifting each give a hider away without a single one of them knowing what
      hiding is: `combat::swings`, `combat::damage`, `chat::speak` and
      `items::pick_up` call `break_cover`, and the two movement paths call
      `step_while_hidden`, which spends a stealth step or gives you away.
      **Hiding** is ServUO's, including the gate that matters: you cannot hide from
      somebody who is *fighting* you within `(100-skill)/2 + 8` tiles, checked both
      ways, which is what stops hiding being a combat escape. **Stealth** wants
      80.0 Hiding and armour under 26 (the plain worn rating pre-AoS — which moved
      `worn_armor_rating` down to `state::armor` beside its data, three readers
      now), and buys `value/10` steps. **Detect Hidden** is a contest
      (`detect/1.5` against each hider's Hiding), not a flat roll, over
      `1 + value/10` tiles. **Stealing** is weight-gated (`10 + value/10` stones)
      and tells the victim *by name* when it fails; the theft itself is returned as
      an intent, because moving an item is `items`' door and flagging a criminal is
      `combat`'s. **Snooping** has no button at all — the action that uses it is an
      ordinary double-click on a container in somebody else's pack, so it is called
      from the tick where the click is dispatched, costs karma every time, and a
      clumsy peek is noticed by name.
      Deferred: **Tracking** (two gumps and the `0x9A` quest-arrow packet) and the
      AoS per-material stealth-armour table.
    - [x] **Bard is a subsystem too**, and it landed as one. `state::instrument` is
      the core table (six classic instruments, each with the pair of sounds its
      ServUO class passes to `base(graphic, well, badly)`), an `Instrument
      { uses_left }` on the item is spent by every attempt, and the three skills
      share a **bard range** (`8 + value/15`), a **Musicianship check before the
      skill's own roll** — which is what makes Musicianship worth training on its
      own — and one `base_difficulty` computed from the target's pools and skills
      rather than a fixed band. A bard with no instrument in the pack gets no cursor
      at all.
      The two lasting effects are components with a tick expiry and **neither is
      folded into anything**. `Pacified` is read where a blow would land
      (`combat::swings`) and where the AI decides (`ai::think_one`), so a calmed
      creature neither swings nor hunts. `Discorded` is read in **`skill_value`** —
      the one question every other system already asks about how good somebody is —
      so a discorded creature hits worse, resists worse and casts worse without
      combat, magic or the AI knowing what a lute is. Provocation reuses the
      `Combat` component the AI already drives, so there is no second fight loop.
      **Musicianship** is the one bard skill with no target: it comes through the
      double-click seam (`tick/skills_wire.rs`'s `use_item_skill`), run *after* the
      `ItemUsed` the pack sees — default in core, customise in the pack, in that
      order. Deferred: the per-target duration scaling (a flat thirty seconds here),
      and the AoS/SE resistance-mod form of Discordance.
    - [x] **Taming, and the pets it wanted.** A `Pet { owner, slots, order,
      order_target }` on the creature and a `Tamable { min_skill, slots }` for the
      kind, with a core table keyed by body (`state::tame`) that a spawn may
      override — and **every rideable body is tamable**, derived from the mount
      table rather than listed twice, because a horse you cannot tame is a horse
      nobody can have (the `mount_body_for` lesson, applied before it could bite
      again).
      **Animal Taming** keeps every gate in ServUO's order — not tamable, already
      tame, too many followers, no chance — and the anger roll, which is what makes
      taming a bear a decision rather than a formality; its timer is dropped the way
      Poisoning's is. The taming itself is an intent: `npc::tame` makes the pet,
      because `npc` owns what a creature *is*, and it gives a brainless prop animal
      a brain, without which a pet would never beat and so never follow.
      **A pet does not decide anything**: `ai::pet_beat` carries out its last order
      and returns a direction, so a pet moves through the same `step` a wild
      creature and a townsperson use, and an attack order simply points the `Combat`
      the AI already drives. What it keeps of the creature it was is its own
      brain, doors included — a tamed orc opens the shop door in its way and a
      llama stops at it, which is ServUO's `BaseAI.CanOpenDoors` and the same
      read a wild brain gets. **Orders come through speech** (`npc::pets`) — "all
      kill", "<name> stay" — matched on the words, because the `0xAD` keyword block
      is skipped by the parser; ServUO's keyword ids are recorded beside the table
      for the day it is decoded. **Follower slots** are a read-site derivation
      (`skills::followers_of`, pets plus the mount), so the bar and the taming
      refusal can never disagree, and the pet **persists** on the mobile's JSON
      record — a restart that quietly released every pet on the shard would be the
      `Murders` lesson again, over property somebody spent an hour earning.
      Deferred: **stabling** (which wants a pet saved with no position, the
      logged-out-character shape), **loyalty** (which is pointless without feeding),
      and **Herding**.
  - [x] **Item-triggered skills** — Healing, Veterinary and Lockpicking, through
    the double-click seam rather than the window, because the action that uses them
    *is* a double-click on the bandage or the pick. They come in through
    `tick/skills_wire.rs`'s `use_item_skill`, run after the `ItemUsed` the pack
    sees, and each raises its own cursor by reusing `TargetPurpose::SkillSecond` —
    the item is the first answer, the patient or the lock the second.
    **A bandage is the one skill whose duration is the mechanic**, so unlike
    Poisoning (whose two-second beat is flavour and resolves at once) it really does
    keep a `Bandaging { patient, done_at }` and finish on the tick counter: ServUO's
    pre-AoS timing off dexterity (about ten seconds on yourself, three on somebody
    else, five more for a resurrection), the bandage spent when the work *begins*,
    and the three outcomes — mend, cure, resurrect — with their own thresholds and
    chances. Each is returned as an intent and applied by the tick through the crate
    that owns the door. **Lockpicking** gave `Lock` the two levels ServUO has
    (`required_skill`/`max_skill`): without them every lock is either free or
    impossible, and a failed pick snaps. Deferred: **Camping**, which wants a reason
    to light a fire (logging out safely in the wild) more than it wants the fire.
  - [x] **And the shops already sell what the new skills need.** The converter reads
    ServUO's own `SB*.cs`, so the Community Pack's vendors were already stocking
    bandages (37 of them), lockpicks (19), instruments (15) and poison potions (26)
    — they were simply inert. An item's core state now lands where the item is
    *made* (`items::apply_core_defaults`, called from the shelf, the spawn and the
    staff `.add`), because a graphic alone cannot say how many tunes are left in a
    lute or which of the four poisons is in a bottle. The poison is read off the
    **label**, which the converter carries through from ServUO: "a greater poison
    potion" is level two, and an unlabelled bottle is the middling one.
  - [x] **Mining, Lumberjacking and Fishing — the harvest system.** ServUO's
    `Scripts/Services/Harvest/`, and the pillar Crafting was waiting on: nothing
    in the engine could produce a raw material. The four definitions (ore, sand,
    lumber, fishing) are core data in `state::harvest` with their real numbers —
    ore a bank of 8×8 holding 10–34, respawning in 10–20 minutes at reach 2, nine
    veins from iron at 49.6% down to valorite at 1.4%, each richer vein
    disappointing into iron one swing in two hundred; lumber a bank of 4×3 holding
    20–45 over 20–30 minutes, ten logs a swing and twenty in Felucca; sand six
    beats to a swing; fishing a single eight-second cast at reach 4. Skills in
    tenths, chances in hundredths of a percent, every duration a tick count.

    **A bank belongs to the ground, not to an entity**, so `Banks` sits on
    `FacetState` beside the sector grid and the obstruction index, keyed by kind
    and block. It is **deliberately not persisted**, as ServUO does not persist
    it — a restart repays every vein, which is written beside the struct so it is
    not filed as a bug later. What *is* saved is the vein's *position*: where
    ServUO seeds a `Random` with `(x*17)+(y*11)+(map*3)`, this hashes the same
    three inputs, because a bank that is not saved must still find the same ore
    under the same block after a reboot or a valorite vein wanders.

    **The load-bearing half is reading the tile.** A `0x6C` location reply carries
    a graphic only when a *static* was clicked; a click on bare land arrives with
    a graphic of **zero** and the land tile id is never on the wire, so the server
    looks it up — a new `Terrain::land_tile`, beside `statics_at`. And a claimed
    static is verified against the map at that exact id *and* z before it is
    believed (ServUO's `PacketHandlers.cs` cancels the target otherwise): without
    that a client names a tree at its feet and mines the middle of Britain. A
    static is matched as `(id & 0x3FFF) | 0x4000` and land raw, which is why the
    mountain *ground* and the mountain *wall* both reach the ore definition.

    The rest is the shape the bandage slice set: a double-click on the tool
    (`use_item_skill`, so an axe is a lumberjack's tool and a weapon at once —
    derived from `state::weapon`'s `is_axe`, not listed twice), a **location**
    cursor under `TargetPurpose::Harvest`, a `Harvesting` component beaten down on
    the tick counter with its ServUO gesture and sound each time, and on the last
    beat `CheckHarvestSkill` — the flat `req_skill` *and* `roll_skill_band`, the
    same call combat's to-hit makes, so a miner trains from the attempt. Every
    gate is re-checked on every beat, because all of them change under a swing
    that takes seconds; walking away mid-swing gets a **different** line from
    clicking too far off, which is ServUO's distinction and the whole feedback.
    A tool spends a use per swing and breaks, which needed schema **v20**: one
    nullable `uses` column serving both the new `Tool` and the existing
    `Instrument` — the latter a bug this fixes, since a half-played lute came back
    full at every reboot. The seven woods are gated on `[gameplay] expansion`
    (ML by default), which threaded `expansion` into `Gameplay` as an ordinal so
    the `0xB9` mask and the content tables read one setting.

    The vendors already stocked the tools — 46 pickaxes, 40 hatchets, 21 fishing
    poles — and were inert, exactly as the bandages and lutes were before their
    slice. Deferred: ML **bonus resources** (gems, bark fragments, pearls), whose
    items do not exist yet; **granite** and the special deep-water catches;
    `BaseOre`'s pile-size art swap, without which rolling ServUO's four ore
    graphics would leave four piles that refuse to merge; and High Seas' lava
    tiles. The **pack-capacity** refusal this list also carried has landed — see
    `items::capacity` under the staff-command entry in §7.
  - Sphere's per-skill `AdvRate` tables and its "learn only from a challenge"
    `GainRadius` — **dropped, not deferred**: ServUO's band *is* the
    learn-from-a-challenge rule, and its `gain_factor` column is the per-skill
    rate. Kept here only so nobody re-adds it from the old plan.
