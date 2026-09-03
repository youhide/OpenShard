# The combat phase

*The roadmap's own record of how the fight was built — swing timers, damage,
resistances, notoriety, corpses, ghosts and the numbers behind each. A record,
not a status: what is built and what is open today is
[`README.md`](../README.md), and the model is
[`design_fight_loop.md`](../design_fight_loop.md) and
[`design_actions.md`](../design_actions.md).*

- [x] `combat` — swing timers, damage, resistances, notoriety
  - [x] **Hit points, damage and death.** Mobiles carry `Hitpoints`; scripts
    spawn creatures (`op_spawn_mobile` → `Command::SpawnMobile`, an entity with a
    body and no client, drawn through the same interest machinery as a player)
    and damage them (`op_damage` → `Command::Damage`). A blow lowers hits and
    redraws the `0xA1` bar — the mobile itself sees the real numbers, everyone
    else a percentage, so a stranger's exact health never crosses the wire. At
    zero it emits `MobileDied`, which the server delivers to scripts, so loot,
    notoriety and quests hang off death without combat knowing they exist — the
    "systems emit, they do not call" rule made concrete. A creature is removed on
    death; a player stays (ghosts and corpses are a later slice).
  - [x] **The interactive layer.** A player toggles war mode (`0x72`, echoed
    back settled) and picks a target (`0x05` → `0xAA`); a `Combat` component
    holds the stance, the target and the next-swing tick. `swings()` runs each
    tick: a combatant in war mode with a target within `MELEE_RANGE` on the same
    facet strikes when its timer is up, out of reach it waits with its timer
    unspent, and a killed target ends the attack. The timer is a tick count, like
    decay — no clock in the tick. A `SwingSpeed` component sets the cadence per
    mobile as an explicit override, but with no override the pace is now *derived*
    from the wielder's dexterity through Sphere's pre-AoS formula
    (`CResourceCalc.cpp`, era 1): swing tenths = `(15000 · 10) / ((dex + 100) ·
    base)`, wrestling base 50, so a `dex 100` mobile swings every 1.5s and a
    nimbler one sooner. The weapon `base` is now the wielded weapon's, not always
    wrestling — see **Weapon properties** below.
  - [x] **Resistances and the damage formula.** A swing's damage is no longer
    flat: `melee_blow` takes the attacker's `MeleeDamage` and cuts it by the
    target's `Resistance { physical }`. Both are components a script sets when it
    spawns a mobile (`op_spawn_mobile` grew `damage` and `resistance`), so a
    hard-hitting ogre or an armoured knight is a data change, not a code one — the
    script-first part. Melee is physical; the other damage types arrived with
    `magic`.
  - [x] **Notoriety and criminal flagging.** Mobiles carry a `Notoriety` (the
    enum already in the protocol), drawn as the health-bar colour in every
    `0x78`/`0x77` — the world stopped hardcoding "innocent". A script sets it at
    spawn; an invulnerable (yellow) mobile cannot be attacked. Raising a hand
    against someone blue or green turns the attacker grey — a `CriminalUntil`
    flag, its expiry a tick count like decay, broadcast to every watcher with a
    `0x77`. **And murderer flagging is real** — the red a repeat killer earns. A
    `Murders` count tallies innocents killed (attributed in `swings`, where the
    killer and the blue victim are both known); the fifth turns the killer red for
    good. Unlike the lapsing grey flag it is persistent, so `expire_criminality`
    now restores a mobile's *base* standing — murderer if the tally stands, else
    innocent — rather than always washing it blue. Attribution is *not*
    melee-only: `damage` takes an `attacker`, and a script's `op_damage`/spell
    carries a `by` serial, so a fireball that kills a blue is a murder the same as
    a sword; unattributed damage kills without blame. And old kills fade — a
    `MurderDecay` clock ages one count off at a time, washing a reformed killer
    back to blue once it drops below the threshold. (Sphere's separate short- and
    long-term counts are a finer model this stands in for.)
  - The **typed damage** this once deferred landed with `magic` below: `damage`
    takes a `DamageType` (physical, fire, cold, poison, energy) and cuts it by the
    target's `Resistance` for that type, in the one place all damage passes — melee,
    spell, poison pulse and script alike.
  - [x] **Weapon properties — swing speed and damage from the wielded weapon.**
    The weapon a mobile holds now drives its swing pace and its damage roll, so a
    katana strikes faster than a maul and a longsword hits harder than a dagger.
    **Not from tiledata**, despite the old heading: weapon speed/damage genuinely
    are *not* in `tiledata.mul` (the reader drops the layer/quality byte, and the
    numbers were never there) — in classic UO they are per-weapon-*class* constants.
    So they live in a **core table keyed by graphic** (`combat::weapons`, ~40
    classic weapons ported from ServUO's `BaseWeapon` subclasses, both the pre-AoS
    `Old*` and the AoS `Aos*` sets, `by_era` picking between them), the same
    "data keyed by graphic, default in core" shape as `creature_name`. The seam was
    already right and stayed put: `swing_speed`/`melee_blow` are **read-site
    derivations**, recomputed fresh each swing, so they consult the item on the
    wielder's weapon layer (`equipped_weapon`) with no mirror stamped on the mobile
    — a weapon coming *off* reverts to wrestling with nothing to undo, none of the
    per-mutation bookkeeping the persistence rule warns against. Precedence is
    **explicit override → weapon → default**: a creature's `MeleeDamage`/`SwingSpeed`
    (its natural blow, a script's pin) still wins, a player (who no longer carries a
    fixed `MeleeDamage`) derives from the weapon, and bare hands stay wrestling and
    `SWING_DAMAGE`. The damage roll uses the world's seeded `rng`, so a fight
    replays.
  - [x] **Hit chance, skill gain, damage scaling, and a pack override.** The follow-ups
    the weapon table set up, all ServUO-faithful:
    - **To-hit.** A swing now rolls to land — pre-AoS `CheckHit`, `chance = (atk + 50)
      / ((def + 50)·2)`, the two mobiles' weapon-skill standings (the defender's own
      weapon skill, Wrestling unarmed, its guard). A miss whistles past (`MELEE_MISS_SOUND`,
      the swing still animates) and does no damage; the timer resets either way. The
      roll **is** a `CheckSkill`, so the same call trains the weapon skill — a new
      `skills::roll_skill_chance` (ServUO's `CheckSkill(skill, chance)`) shares the
      gain half with `roll_skill_band`, and a player's Swords/Archery/… creeps up with use,
      surfacing in the `0x3A` window with no extra wiring.
    - **Damage scaling.** A landed blow scales by the attacker's Tactics, Strength and
      Anatomy — ServUO's `ScaleDamageOld` (era 1: Tactics its own ±50% about parity,
      then Str 1%/5 and Anatomy 1%/5 +10% at GM) and `ScaleDamageAOS` (era 2 the
      `GetBonus` coefficients).
    - **Gated on a `Skills` sheet.** Applying these to a mobile with no skills (an
      untrained player) would make it miss half the time and deal half damage. So both
      the to-hit roll and the scaling engage only when the *attacker* carries a
      `Skills` component; without one it keeps the pre-feature certainty — its natural
      blow always lands, unscaled. A clean, forward-compatible boundary: the moment a
      creature is given skills it starts rolling.
    - **Archery** rolls the wielded bow's damage band now (through `scaled_blow`, the
      same path as melee), not the flat default.
    - **Pack per-item override** — a `Weapon { speed, min, max }` component (set by
      `op_set_weapon` → `Command::SetWeapon` → `items::set_weapon`) on a weapon item
      replaces the core table's stats for a magic sword, `equipped_weapon` reading it
      first while keeping the graphic's skill; era-independent.
  - [x] **The rest of the combat deferrals landed too.**
    - **Pre-AoS PvE damage-halving.** ServUO's `ComputeDamage`: outside AoS, full
      damage lands only when a player strikes a non-player — every other pairing (a
      monster's blow, PvP) is halved. In `scaled_blow`, past the skill gate, keyed on
      a `Client` component, era `< 2` only.
    - **Per-weapon miss sounds and the Axe/Lumberjacking bonus.** The weapon table
      grew `miss_sound` (ServUO's `DefMissSound`, resolved through the base classes)
      and an `is_axe` flag; a whiff plays the wielded weapon's own swish, and an axe
      in a lumberjack's hands hits harder (era 1 capped 20%, era 2 the AoS `GetBonus`).
    - **Creatures carry combat skills.** The spawn path — `op_spawn_mobile` /
      `Command::SpawnMobile` / the spawner's `CreatureTemplate` — grew a `skills` list
      that `npc::spawn` turns into a `Skills` sheet, and it **persists** (a `skills`
      field on `MobileRecord` and the spawner's `CreatureData`, both JSON records, so
      no schema bump). A monster given Wrestling/Tactics rolls to hit and scales
      damage exactly as a player does. The remaining half is data: the converter
      scraping ServUO's `SetSkill` per creature, a Community-Pack follow-up — the
      engine is ready for it.
  - [x] **Combat eras 3–5** — Sphere's `m_iCombatSpeedEra` `0` (custom), `3` (SE) and
    `4` (ML) join the implemented `1`/`2`, ported from `CResourceCalc.cpp` into
    `swing_ticks`: SE `scale/((dex+100)·aos_speed) - 2`, ML `ml_speed·4 - dex/30` (a
    third speed column on the weapon table, from ServUO's `MlSpeed`), era 0 pre-AoS
    with a 0.5s floor. `by_era` and the damage scaling follow the family split (0/1
    pre-AoS, 2/3/4 AoS), and `config` accepts `0..=4`. Set `speed_scale_factor` to
    match (15000 pre-AoS, 40000 AoS, 80000 SE; ML ignores it).
  - [x] **Creature corpses and loot.** A slain creature no longer vanishes: the
    tick's `reap` (reading `MobileDied` — combat emits the death, the world
    disposes of the body) lays a corpse where it fell — item `0x2006` with
    `CorpseBody = body` (the protocol special case that draws the right corpse), a
    container on gump `0x0009` holding the creature's worn gear and a core gold
    drop scaled from its toughness. It decays after seven minutes and takes its
    loot down with it (`items::decay` now cascades into a container's contents, so
    nothing is orphaned). `combat::die` stopped despawning — it announces, `reap`
    disposes. The corpse persists as a ground container; a restored one gets a
    fresh decay timer (the tick is not saved).
  - [x] **A corpse lies the way it fell.** The corpse's picture is a pair — which
    body, and facing where — so `CorpseBody` carries both and `0x1A` sends the
    facing in its direction/light byte (announced by the top bit of `x`, written
    between `y` and `z`; see `docs/findings.md`). Before this the client drew every
    corpse southeast: the death *animation* was right, because it is the mobile's
    own, and the body then turned as it settled and again on every later fold of
    the world. The facing is the heading the mobile died with, and it is saved
    beside the corpse's story rather than in a column of its own — the item row's
    `amount` already carries the body — so a corpse restored from a save written
    before this comes back lying north.
  - [x] **The shard says which corpse a body became (`0xAF`).** The premise of
    the entry this replaces — that the wire has no field pairing a corpse with the
    mobile it was — was wrong: `0xAF` is exactly that packet, thirteen bytes of
    killed serial, corpse serial and a run flag, and it is what ClassicUO's
    `CorpseManager` is built on. `WorldState::announce_death` sends it to everyone
    watching except the dying player's own client (ServUO excludes it too — that
    client has `0x2C` and a ghost, not a corpse to pair), and `Crowd::died` lifts
    the falling body out of the crowd and holds it under the corpse's serial for
    `Crowd::corpse` to finish. The tile-and-body search is gone, and with it the
    case where two of the same creature dying together swapped falls. Holding the
    fall by serial also means the removal and the corpse no longer have to arrive
    in one batch for the hand-off to work.
  - [x] **`0x1A`'s light and flags bytes are read instead of refused.** Both used
    to make the decoder reject the packet, which lost the whole item to save a
    hint — and the flags byte is not rare: ServUO sets `0x20` on everything a
    player may pick up, so the rule refused most of a real shard's ground.
    `WorldItem` now carries `light: Option<LightId>` and `flags: ItemFlags`; this
    shard sends neither (an item's light comes from its graphic's tiledata, and
    what may be lifted is decided when the player tries), so they exist to keep a
    foreign shard's item readable and to be there the day `light.mul` is.
  - [x] **Ghosts and resurrection.** A player who dies no longer stands at zero
    hits: `reap` now lays a **player corpse** holding their worn armour (the
    backpack and bank box stay on them — worn containers, not loot) and puts them
    into a **ghost state**. The ghost wears the grey ghost body (`0x0192` male /
    `0x0193` female, ServUO's `Race.GhostBody`) and a death shroud (`0x204E`), and
    the client is told it is dead (`0x2C`), which greys the world and gives the
    gliding ghost walk. **The living cannot see the dead:** the interest gate
    `WorldState::can_see_mobile` draws a ghost only to another ghost or to staff —
    ServUO's `CanSee(Mobile)` clause — so a living watcher is told to forget it
    (`0x1D`) the moment it dies. The AI already ignores it (a 0-hit mobile is
    neither acquired nor pursued). **Resurrection** lifts the `Ghost` marker,
    restores the living body it remembered, strips the shroud, and hands back a
    tenth of the hit points; the corpse stays where it fell to be walked back to
    and looted. Two paths reach it: the **Resurrection spell** (moved out of
    `Scripted` into a core `SpellEffect::Resurrect`, the effect the table was
    "waiting on") and a staff `.res`. **It persists (schema v9):** a `dead` flag
    rides the character row while the `body`/`hue` stay the *living* ones, so a
    ghost that logs out logs back a ghost — the grey body re-derived, the corpse
    already a saved ground item, no duplicate laid.
  - [x] **Pack loot tables.** The corpse gold stays a flat core baseline (so a
    bare shard still loots); the real per-creature loot is the pack's, off a
    `CorpseCreated` event `reap` emits when a creature's corpse is laid — carrying
    the corpse serial and the body, the pack's loot-table key. A script fills the
    corpse by serial through a new `op_add_loot` (→ `Command::AddLoot` →
    `items::give` for a stackable, `items::place_one` for a discrete piece),
    guarded so a stray serial adds nothing. The "default in core, customise in the
    pack" split, same as spell and skill effects. The Community Pack ships
    `loot.js`: a `Pack.loot[body]` table of `{ graphic, amount, stackable, chance }`
    drops (an orc, a spectre), rolled in `index.js` — pack loot may use
    `Math.random`, since determinism is the core's seeded rng and a script is an
    external input to it.
  - [x] **Stamina as a real pool.** The status bar sent `stamina = dexterity`
    outright, a placeholder that only existed so the client would run at all; now a
    real `Stamina { current, max }` component (`state`, the sibling of `Mana` and
    `Hitpoints`) carries the pool, `max` = dexterity — the UO identity where the
    bar *is* the stat — full at enter, and `send_status` reads it. A dexterity
    change re-caps it the way strength re-caps hit points, both from
    `skills::set_stats` and from an Agility/Clumsy buff (`magic::shift_stats`, whose
    "dexterity's stamina pool has no component yet" comment this retires).
    `combat::regen_stamina` trickles it back from the tick like `magic::regen_mana`,
    a touch faster (`STAMINA_REGEN_TICKS`). The first real consumer — the
    overweight drain — landed with the status-bar slice below; the war-mode
    push-through cost is still a follow-up.
  - [x] **Hit points regenerate.** Mana and stamina trickled back and hits never
    did, so a wounded character could only ever be mended by someone else — a gap
    nothing in the roadmap had recorded. `combat::regen_hits` is the third of the
    trio, the same tick-counter shape: a point every eleven seconds (ServUO's
    pre-AoS `Mobile.DefaultHitsRate`), and none at all for the dead or the
    poisoned, which is literally ServUO's `CanRegenHits` (`Alive && !Poisoned`).
  - [x] **Armour, worn and felt.** Armour rating is not in `tiledata.mul` either —
    the same finding the weapon table recorded — so it is a core table keyed by
    graphic (`combat::armor`, the classic leather/studded/ring/chain/plate/bone
    suits, helms and shields from ServUO's `BaseArmor` subclasses), with an
    `Armor { rating }` component the pack can lay over one item. Where a piece
    sits comes from the layer it is *worn* on, not from the table, because the
    wearer already carries that fact — ServUO derives its own `BodyPosition` the
    same way. `worn_armor_rating` is the read-site derivation (each piece scaled by
    how much of a body it covers, ServUO's `ArmorScalars`), shown on the status bar
    and spent in combat: pre-AoS a swing gives up a share of it through
    `absorb_physical`, ServUO's `BaseWeapon.AbsorbDamage` — a rolled hit location,
    that piece and any shield eating their own bite, then a cut of the wearer's
    total. Rolled on the seeded `rng`, so a fight still replays; a mobile wearing
    nothing rates zero, which is why no existing combat test moved. One tidy-up is
    recorded in the code: ServUO's two ladders disagree by a swap (its
    piece-selection gives the arms the 14%-wide band while its scalar array gives
    the helm 0.14), and this port uses the scalars array for both.
  - [x] **The status bar stops lying.** Four of its numbers were constants — gold
    `0`, armour `0`, weight a flat body weight, followers `0` — read by a player
    every session with no way to check them. They are read-site derivations now,
    the shape `equipped_weapon` established: gold and weight from `items::carried`
    walking the worn tree (a pouch inside the pack counts), armour from the table
    above, followers from whether a mount is under the rider. Gold weighs ServUO's
    `0.02` stones a coin, not the tile's weight, or a bank run would pin a
    character to the floor. Two edges are ServUO's `Mobile.UpdateTotals` exactly,
    and both were wrong in the first cut: **the bank box is not carried** (it is
    `IsVirtualItem`, so neither its weight nor its gold reaches its owner — which
    is why the banker has to *tell* you your balance), and **a held item is** (it
    adds `m_Holding` explicitly, so lifting the anvil onto the cursor is not how
    you carry it home). The re-send is a **diffing pass** (`tick/status.rs`,
    twice a second over the online players) rather than a call beside every item
    mutation — the pattern that decays, the same argument the persistence rule
    makes. And the female flag now follows the body rather than calling every
    character male.
  - [x] **A staff account can put the staff down (`.gm`).** Sphere's split, which
    the fatigue rule above made necessary: its `PLEVEL` says who may command and
    its `PRIV_GM` flag says who is currently exempt from the game's rules, and
    `.GM` toggles the second without touching the first
    (`Source-X/src/game/clients/CClient.cpp:836`). Here that is `Access` (the
    account's authority, re-derived each login) and a new `Staff` marker (the
    mode, given at login to a game master and taken off by `.gm`).
    `WorldState::is_staff` reads the marker and is the one choke point every
    exemption already went through, so the whole behaviour change is two rules:
    with the mode off a game master **tires under its load** and **cannot see or
    hear the dead**. The command gate stays on `staff_authority` — the `.`-prefix
    split and the admin gump's re-check both — or `.gm off` would lock a game
    master out of `.gm on`. The screen is rebuilt on the spot (`refresh_around`,
    the call death and resurrection make), so ghosts appear or are forgotten as
    the mode flips. Without this there is no way to *test* a player-facing rule
    from the only kind of account that can set one up.
  - [x] **The bank is a wallet, on two `[gameplay]` flags.** Taking the bank box
    out of what a character carries (see above) left banked gold buying nothing.
    ServUO does the other half: `BaseVendor` tries the pack, then the bank, and
    says which paid. So `vendor_bank_payment` (default **on**, UO and ServUO) lets
    a purchase fall back to the bank whole — never split across the two, the
    reference's rule and the honest one — and `bank_gold_in_status` (default
    **off**, ServUO's truth) decides whether the status bar's gold adds the box.
    Weight is on neither switch: banked goods are never carried, or banking a pile
    would make you overweight. One rule counts the coins for all three readers
    (`items::banked_gold` — the banker's "balance", the bar, the vendor), and it
    walks the box's whole tree, so a purse *inside* the bank counts where the old
    one-level scan missed it. `take_from_container` widened to `u32` with it: a
    bank purchase runs past what one `u16` stack can hold, and the old cap refused
    it with "thou canst not afford that", which was a lie.
  - [x] **Carrying too much costs something.** ServUO's
    `WeightOverloading.EventSink_Movement`, ported into `combat::spend_step_stamina`
    and consulted by the player walk: over the cap (plus a four-stone allowance) a
    step costs `5 + over/25` stamina — a third of that mounted, double at a run —
    a pool under a tenth full costs an extra point, and every sixteenth step on
    foot (forty-eighth mounted) costs one anyway. A pool at zero refuses the step
    with the reason, through the same `0x21` reject path paralysis uses. Staff never
    tire. This corrects the earlier claim that unencumbered running is free: ServUO
    charges the baseline, and against the regen it is very nearly a wash, which is
    why classic running feels endless without being free.
  - [x] **Combat sounds, per creature, and the projectile.** A landed blow plays
    the *attacker's own* sound — a creature's ServUO `BaseSoundID + 2`, a human's
    fists thwack — so an orc growls its attack instead of punching like a man; a
    death plays the victim's cry (`BaseSoundID + 4`, or a gendered human gasp), and
    a creature growls (`+ 0`) the moment the `ai` aggros it. The sounds derive from
    the body id via `creature_base_sound(body)` (keyed like `creature_name`,
    ServUO's per-creature `BaseSoundID`), so the pack needs no sound data — it
    falls out of the bodies it already spawns. A ranged volley also flies a real
    arrow: a `0x70` moving graphical effect from shooter to mark. The `protocol`
    feedback encoders (`0x54` sound, `0x70`/`0xC0` effect, `0x6E`/`0xE2` animation)
    and `WorldState::broadcast_from`/`play_sound`/`animate` are the seam; every
    source broadcasts to the players in view range.
  - [x] **Swing, death and cast animation.** `WorldState::animate(mobile, Action)`
    animates a swing, a death throe or a cast gesture for everyone in view range.
    The modern client (7.0.0.0+, gated on `Feature::NewMobileAnimation`) gets the
    `0xE2` new-animation packet, where the server names a body-agnostic
    `AnimationType` (Attack/Die/Spell) and the *client* picks the frames — so no
    body table is needed there, the path the test clients take. An older client
    gets the `0x6E` classic packet, whose action id is body-specific, chosen off a
    coarse humanoid-vs-creature split (`body_opens_doors`). Wired into the melee
    and ranged swing, death (`combat::die`), and the cast. ServUO's ids: Wrestle
    31, human die 21, human cast 16; monster attack 4, die 2, cast 12.
  - **Exact per-weapon and per-body `0x6E` actions** — the classic-packet
    action is a coarse humanoid/creature split, not the per-weapon (slash vs bash
    vs pierce), mounted, or per-monster action ServUO computes from the body
    tables. The modern `0xE2` path is exact; this only refines the old 2D client,
    the minority path, and wants the body-animation tables.
