# The magic phase

*The roadmap's own record of Magery: mana, the cast sequence, the 64-spell table
and every family that came out of `Unimplemented` — poison, the buffs, the
fields, paralysis, resistance, the summons, travel and the dispels. A record, not
a status: what is built and what is open today is [`README.md`](../README.md),
and the model is [`design_magic.md`](../design_magic.md).*

- [x] `magic` — spells, reagents, casting
  - [x] **Mana, casting, and the effect seam.** A mobile carries `Mana` (spent by
    casting, trickling back on a tick-counter regen). `Command::CastSpell` is the
    gate every spell passes: it checks the mana, rolls the casting skill (through
    the same band roll a mined ore uses, so casting trains Magery), spends the
    mana, and emits `SpellCast { caster, spell, target, success }`. What the spell
    *does* — a fireball's damage, a heal, a summon — is not here: a script reads
    `SpellCast` and gives it its effect, `MobileDied`'s decoupling a third time.
    `Command::Heal` mends toward the maximum; `op_cast_spell`/`op_heal`/typed
    `op_damage` are the script's hands.
  - [x] **Live mana redraw (`0xA2`).** The blue line under the character read the
    last full `0x11`, and `refresh_statuses` only sends one when an
    *inventory-derived* number moves — so a mage could empty the pool in a fight
    and watch a full bar throughout. The pool now has **one door**,
    `WorldState::set_mana` (`broadcast_health`'s sibling): it writes the component
    and sends the owner a `0xA2` mana bar in the same breath, and the four sites
    that mutated `Mana` in place from three crates — the cast, the fizzle, the
    meditation trickle, an intelligence buff's cap shift — all go through it. Only
    a character *sheet* being assembled at login still writes directly: there is no
    client on the other end of it yet. `0xA2` carries the real pool and goes to the
    mobile itself alone, as ServUO's `MobileMana` does (the scaled party copy has no
    reader here yet). On this client the pool left `Status` for `Player::mana`, the
    way hit points live in `Player::hits`, because a value two packets can state
    must have one home.
  - [x] **A cast reveals, and breaks concentration.** `Spell.Cast` calls
    `RevealingAction` the moment the state turns to casting, and that call ends in a
    `DisruptiveAction` — neither happened here, so a hidden mage stayed hidden
    through a fireball and a meditating one cast out of the trance and kept
    regenerating at twice the rate. One `break_cover` in `begin_cast` does both (it
    already ends in `disrupt`), placed after the free refusals and after the
    one-cast-at-a-time gate: a spell the book does not hold was never begun, so it
    gives nobody away.
  - [x] **Typed damage and resistances** (the piece combat deferred). `damage`
    now takes a `DamageType` — physical, fire, cold, poison, energy — and cuts it
    by the target's `Resistance` *for that type*, in the one place all damage
    passes through, so a fireball and a sword swing share the door. Melee is
    physical; a spell picks its element.
  - [x] **reagents** — a spell consumes items from a pack. `items` grew the
    container search the deferral named — `count_in_container` and an
    all-or-nothing `take_from_container` — and `cast_spell` grew a second gate
    beside mana: a `Cast` now carries a `pack` and a `(graphic, count)` reagent
    list, and the spell fizzles spending *nothing* unless the pack holds every
    reagent, then consumes them. Reagents-as-data: the script names them per
    spell, the world enforces them. A pack open on a client redraws live too:
    `WorldState` remembers who has each container open (`double_click` records it,
    logout clears it), and a consumed reagent is pushed to those watchers — a
    `0x1D` for an item burned whole, a re-sent `0x25` for a dipped stack.
  - [x] **the client cast path** — a spellbook cast (`0xBF.0x1C`, read from
    ServUO's `PacketHandlers.CastSpell`) decodes to a `RequestCast`. It once
    became a `SpellRequested` event for the script to own the mana and reagents
    (Sphere-scriptpack style); the core spell table below took that over, running
    the whole cast itself, and `SpellRequested` is left dormant behind it. The
    older `0x12` text-command form is a fill-in; a modern client sends the `0xBF`.
  - [x] **The 64-spell core table and the full cast sequence.** All eight circles
    of Magery live in a core table (`magic::spells`, ported from ServUO's
    `SpellInfo` + the classic reagent lists): each spell's circle — which sets its
    mana, cast delay and difficulty — its reagents, what it targets, and its
    *default effect*. `RequestCast` → `World::begin_cast` runs the sequence in the
    core: mana and reagents from the pack, the Magery roll (the same band roll
    a mined ore uses), the target cursor, and the effect. The core runs the
    archetypes the engine can do — direct and area typed damage, heal, teleport —
    and tags the rest `SpellEffect::Scripted`: they still *cast* fully and emit
    `SpellCast` for the pack to give an effect, the "default in core, customise in
    the pack" split skills has. A pack overrides any spell the same way, off
    `SpellCast`.
  - [x] **Cast style, a `[gameplay]` flag** (the Sphere-vs-ServUO knob asked for).
    `cast_style = "servuo"` roots the caster over a cast delay held in a `Casting`
    component and only then raises the target — moving breaks it, and `spell_disturb`
    decides whether a blow mid-cast fizzles it; `cast_style = "sphere"` resolves
    the cast as it is made, walking. Both threaded from `openshard.toml` into
    `WorldState.gameplay`, never branched on `Era`.
  - [x] **Spell cost, three more `[gameplay]` flags** (Sphere's magic model,
    confirmed in `Source-X`). Sphere spends mana and reagents at *resolution* —
    once the cast has succeeded or fizzled, not up front — so `pay_and_roll` now
    checks availability, rolls, and only then spends: `reagents` (require and
    consume reagents at all, off for a mana-only shard), `mana_loss_on_fail`
    (Sphere's `ManaLossFail` — does a fizzle still burn the mana) and
    `reagent_loss_on_fail` (`ReagentLossFail`). A successful cast always spends;
    the UO/ServUO original is all three on. Orthogonal to `cast_style`, which is
    the rooting/precast axis (`MAGICF_PRECAST`/`FREEZEONCAST`).
  - [x] **Poison, in the core.** Poison, Cure and Arch Cure run a `Poisoned`
    component that `combat::poison_tick` pulses like decay — typed poison damage
    cut by poison resistance, in the one place all damage passes — with a dose
    scaled from the caster's Magery. This is the first spell effect that is *stateful
    over time* rather than instantaneous, the shape the timed buffs then reuse.
  - [x] **Timed stat buffs — the Bless/Curse family.** Strength, Agility, Cunning
    and Bless, and their opposites Weaken, Clumsy, Feeblemind and Curse, all moved
    from `Scripted` into the core. `magic::apply_stat_buff` folds a Magery-scaled
    offset into the target's `Stats` and the caps that hang off them (str→hits,
    int→mana), a `StatMods` ledger remembers exactly how to give it back, and
    `magic::expire_buffs` lifts it on the tick counter; a recast refreshes its kind
    rather than stacking a second. The stat change redraws the player's status bar
    (`0x11`), the one thing that re-sends str/dex/int. The effect kinds are
    canonical in `state::effect`.
  - [x] **Effects persist (schema v7).** A live effect is saved with its mobile —
    a `Poisoned` or a `StatMods` entry becomes an `EffectRecord { kind, amount,
    remaining }` on the character or mobile row — and restored on login and boot
    alike, so a relog cannot wash a debuff off. The ServUO/Sphere model reached the
    way this engine saves anything: a record, swept whole, not a stopped world.
    Poison restores as the whole component; a buff as its ledger *only* (its shift
    is already folded into the saved stats, so re-applying would double it).
    `World::effects_of`/`apply_effects` are the one seam; every future buff and
    debuff slots into the same `effects` list with no schema change.
  - [x] **The spellbook gates the cast (schema v8).** A `Spellbook` is a `u64`
    mask (bit _n_ = spell _n_) on the book item; `begin_cast` refuses a spell the
    caster's book does not hold — classic UO, cast only what you scribed. A mage
    sells an empty book (`0x0EFA`) and Magery scrolls (`0x1F2D + spell`); dragging
    a scroll onto the book learns the spell and consumes it (Sphere's scribe
    flow), and double-click opens it (`0x24` gump `0xFFFF` + the `0xBF 0x1B`
    content packet). The mask persists on a nullable `spellbook` item column (a
    `u64` bit-cast to the signed SQL integer so the full book's top bit survives),
    swept in `item_record` and restored in `place_ground_item`/`restore_inventory`
    — without it a relogged book silently refused to open. `.spellbook` is the
    staff shortcut; the Community Pack's Britain mage stocks reagents, book and all
    64 scrolls.
  - [x] **Cast sound and visual.** A resolved spell now plays a sound and throws a
    visual: a fire bolt for fire damage (0x36D4, sound 0x15E), an energy bolt for
    energy (0x379F/0x20A), a magic-arrow bolt for the rest (0x36E4/0x1E5), a
    sparkle on the mark for a heal, cure or buff (0x376A/0x374A/0x373A), an
    explosion at the aimed spot for an area blast — ServUO's per-spell sound and
    particle ids, resolved in `apply_spell_effect` where the core runs the effect.
    Coarse (keyed on the effect variant, not exact per-spell art) but the cast is
    no longer silent and invisible, which was the single most visible gap against a
    real client. A `Scripted` spell voices itself in the pack, off `SpellCast`.
  - [x] **Per-spell exact art, power words, and the cast gesture.** The table
    grew three columns and the guessing stopped.
    - **The art** was keyed on the coarse `SpellEffect`, so Fireball and
      Flamestrike were one picture and one sound between them, and eight stat
      spells shared a single sparkle. Each row now carries ServUO's own
      `PlaySound`/`FixedParticles` call for that spell: `SpellArt::Landing
      { sound, visual }`, or `Silent` for the three kinds of spell that have
      none — one whose art belongs to its *effect* rather than its cast (Recall's
      two ends, a gate's pair, and Mark, whose sound moved beside the rune it
      writes, so a refused mark is now quiet), one the reference itself leaves
      bare (Earthquake, whose noise is everybody it hurts), and one the engine
      does not run yet. `SpellVisual` names the placement rule that was already
      here — bolt, on-target, at-spot — plus `Lightning`, the strike that carries
      no art id because the graphic is the client's, and `Unseen`, for a field
      whose tiles are its own picture. Where ServUO branches on `Core.AOS` the
      classic side is taken, era 1 as everywhere else (Fireball's `0x44B`, Harm's
      `0x1F1`).
    - **The power words.** `MessageType.Spell` had been sitting in a test as the
      example of a byte with no rule; it is `TalkMode::Spell` now, carrying like
      ordinary speech because ServUO's `SayMantra` is a `PublicOverheadMessage`.
      `begin_cast` says it through `chat::speak` in the same breath as the
      reveal — before a tick of the cast delay is measured, because a warning
      that arrives together with the fireball is not one, and it is said whether
      the cast then takes or fizzles.
    - **The gesture** moved from resolution to the start, where the second of
      rooted casting it is supposed to fill actually is. ServUO's twenty
      `SpellInfo.Action` ids across 203..=269 read like twenty animations and are
      two: the client's own `Anim2.def` replaces 203..=245 with group `{16}` and
      260..=269 with `{17}`. So `Action::Cast` split into `CastDirected` and
      `CastArea`, and nine spells — the seven summons, Gate Travel and Mass
      Dispel — raise both arms. The rooted style holds it for the cast delay
      through `animate_timed`, the seam a swing and a pick stroke already use.
    - Deferred: art played once **per victim** for an area spell (ServUO strikes
      every mobile Chain Lightning catches and throws a fireball at each one
      Meteor Swarm does; one landing stands at the aimed spot instead), the
      hand particle at the cast's start (`LeftHandEffect`/`RightHandEffect`,
      which want the `0xC7` particle packet this engine does not send), and a
      mantra in the caster's own speech hue — the client's chosen hue passes
      through `chat::say` and is never stored, so there is nothing to read back.
  - **An `Item`-targeted spell has no trustworthy aimed point.** `handle_target`
    passes the client's `location` through, and for an object the client fills it
    with the *item's* coordinates — which for something in a pack are the slot
    inside the container, not a place in the world. Nothing reads it today
    (`SpellTarget::Item` is the travel family, and all three voice themselves at
    the caster), and the art slice kept it that way rather than sounding Mark at a
    rune's container slot. But `spell_feedback` still falls back to
    `target_location` when the mark has no `Position`
    (`world/src/tick/spells.rs`), so the first `Item` spell that wants a picture
    on the thing it aims at will draw it in the sea. The fix is to resolve an
    object target to *the holder's* position rather than trusting the wire.
  - [x] **The non-stat magical buffs — Protection, Reactive Armor, Night Sight,
    Magic Reflection.** The family that modifies a *behaviour*, not a number, moved
    from `Scripted` into the core. All four ride one `BehaviourBuffs` component (the
    sibling of `StatMods`: a ledger, at most one entry per kind, a recast refreshes;
    kinds `9..12` in `state::effect`), timed on the tick counter and swept into the
    same saved `effects` list with **no schema change** — a relog keeps them. Each
    is read where its behaviour is decided, pre-AoS (era 1) classic, Magery-scaled:
    **Reactive Armor** bounces a percent of a melee physical blow back at the
    attacker in `combat::damage` (the one damage door; the reflected hit is
    unattributed, which both breaks the recursion and keeps a reflect kill
    blameless); **Protection** rolls its chance to hold concentration in
    `advance_casts` where a blow would else break a cast; **Magic Reflection** bounces
    the next offensive spell back at its caster at the top of `apply_spell_effect`
    and is spent doing it; **Night Sight** sends the caster its personal light
    (`0x4F`, brightest) — a visual no-op until a day/night cycle exists, but sent and
    restored correctly for the moment one does. Each plays ServUO's per-spell sound
    and sparkle. Deferred: the AoS (era 2) resist-swap variants, and a day/night
    cycle for Night Sight to fight.
  - [x] **Persistent fields — Fire, Poison, Energy, Paralyze Field and Wall of
    Stone.** A spell lays a row of ground-tile entities (each a `Graphic`+`Position`
    drawn like a dropped item, carrying a `Field` component), perpendicular to the
    line of fire (ServUO's `eastToWest` from the caster→target vector), Magery-scaled
    in duration. A new `World::field_tick` (`tick/fields.rs`) pulses and expires them
    on the tick counter — the `combat::poison_tick` shape, so a field replays like
    decay, and it runs before `reap` so a field kill lays its corpse the same tick.
    **Fire** pulses fire damage (through the one `combat::damage` door, credited to
    the caster), **Poison** applies poison, and **Paralyze** freezes (see below) to
    whoever stands on a tile (`sectors.nearby(pos, 0)`); **Energy** and **Wall of
    Stone** register each tile in the per-facet obstruction index
    (`obstructions.block`, `door: false`) so they bar players and A\* alike, and free
    it on expiry — the door-toggle/decoration pattern. Fields are transient
    (10–54 s), so like a cast in flight they are **excluded from the save sweep**
    rather than restored as eternal statics. The cast voices only its ServUO sound
    and gesture; the tiles are the visual. Deferred: the 300 ms row stagger, and
    per-tile `stand_z` on slopes. (Dispel Field, deferred here for want of the
    dispel roll, landed with the family below.)
  - [x] **Paralyze, and the freeze mechanic.** A `Frozen { until }` component (tick
    count, like `CriminalUntil`) that the two movement paths consult and refuse
    while it holds — the player walk (`walk`, a `0x21` reject) and the creature/NPC/
    decree step (`step`), the smallest set of edits since there is no single shared
    step. The **Paralyze** spell (Mobile-targeted, moved out of `Scripted`) freezes
    the aimed mobile for `7 + Magery*0.2` s (`magic::apply_paralyze`, a no-op if
    already frozen — ServUO's `Paralyze()`), and the **Paralyze Field** applies the
    same to whoever a tile catches. Classic pre-AoS: paralysis is *move-only*
    (casting and swinging are left to the client, as ServUO's engine does), and **any
    blow lifts it** — `combat::damage` clears `Frozen` inline the moment real damage
    lands, so a reflected hit wakes too. It expires on the tick counter
    (`magic::expire_frozen`, thawing with a "you can move again" line) and **persists**
    on the same `effects` list (kind `13`), so a relog does not thaw it. The
    Resisting-Spells cut landed with the skill below; still deferred: barring a cast
    while paralyzed.
  - [x] **Resisting Spells, the skill nothing read.** `Skill::MagicResist` was in
    the table, on the trainers' lists and in every saved sheet while no code
    anywhere consulted it: a grandmaster warder took a flamestrike exactly as hard
    as a mage in a robe. `magic::resist` is the read site — ServUO's
    `Spell.CheckResisted` pre-AoS, in tenths of a per-cent so the reference's fifths
    and halves stay exact. The chance is the better of two readings of the skill,
    halved: a flat `resist / 5` floor, and a contested one weighing the caster's
    Magery and the circle, which is what makes an eighth-circle spell land where a
    first-circle one would not. **A resist is not a shield** — it takes a quarter
    off, and off *whatever* the spell was going to do: damage for the bolts (each
    victim of an area blast rolls its own), duration for Paralyze and for the debuff
    half of the Bless/Curse family. Being cast at is also how the skill trains, and
    only while the spell is above `(1 + circle) * 10 + (1 + circle / 6) * 25` points,
    so a grandmaster cannot train on first-circle spam. Two spans became spans rather
    than deadlines to make the cut possible (`stat_buff_terms`, `paralyze_ticks`); a
    Paralyze *Field* still freezes outright, as `ParalyzeFieldSpell` does. Deferred:
    the AoS resist-swap variants.
  - [x] **Summons with a lifetime.** The last eight `Unimplemented` rows of the
    Magery table that have an effect at all — Summon Creature, Blade Spirits,
    Energy Vortex, the four elementals and Summon Daemon — now call something up.
    - **A summon is a pet with a deadline**, and that is the whole reason the
      slice is small. ServUO's `BaseCreature.Summon` sets `ControlMaster` and
      `Summoned`, and everything a *controlled* creature does then follows: it is
      friendly, it heels, it answers "all kill", it counts against `Followers`.
      All four already existed here as `Pet`, so a summon **is** one, and the
      `Summoned` marker beside it carries only what a pet has not got — the tick
      it goes. Nothing that follows, obeys or counts had to learn a second kind
      of creature, and the follower number on the status bar needed no telling
      at all: it is derived from what stands in the world
      (`skills::followers_of`), so the bar's own half-second diff sees the slot
      taken and freed.
    - **What each one *is* is a table** (`state::summon`), the shape `tame` and
      `weapon` set: body, hit points, blow, physical resistance, trained skills,
      follower cost and lifetime rule, read straight off
      `Scripts/Mobiles/Summons`. Pre-AoS throughout, so a daemon costs five slots
      and fills a mage's whole following, and a blade spirit costs one.
    - **Skill buys time and nothing else.** `(2 * Magery.Fixed) / 5` seconds —
      four hundred at grandmaster — for the six that appear beside the caster; a
      flat `Random(80, 40)` for Blade Spirits and Energy Vortex, which ignore the
      caster entirely. A novice's elemental is exactly as strong and goes far
      sooner. The roll is on the tick's seeded generator, so a replay summons for
      the same span.
    - **The refusal costs nothing**, `begin_cast` beside Recall's: ServUO gives
      every summoning spell a `CheckCast` that turns it down when
      `Followers + ControlSlots > FollowersMax` (cliloc 1049645). It cannot wait
      for resolution — a mage charged eighth-circle mana to be told the daemon
      will not fit has paid for a "no". The number the gate reads and the slots
      the creature then takes are one column of one table, because a gate that
      asks for more room than it admits is a cap nobody can reason about.
    - **Where it stands** is the creature's own business, and it is two rules
      because the reference has two: the pair that take a target are laid on the
      aimed tile and refused if it is blocked, while the six that take none walk
      the eight neighbours of the caster from a seeded rotation and never land on
      the caster's own tile (`FindValidSpawnLocation(.., surroundingsOnly:
      true)`). Both read `movement::arrival_z` and not the bare map, so a summon
      can be called onto a deck or a house floor. **Summon Creature lost a target
      cursor it should never have had**: its row said `Location` while it did
      nothing, and ServUO's `SpellInfo` passes `allowTarg: false` — a cursor whose
      answer the spell ignores is a lie the moment the row runs.
    - **A summon leaves no corpse**, which is not cosmetic. Pre-AoS ServUO
      deletes the one it just made (`DeleteCorpseOnDeath`); here none is laid,
      because a corpse is filled by `fill_creature_loot`, whose gold baseline
      scales with the dead thing's hit points — a two-hundred-hit daemon conjured
      for fifty mana and killed on the spot would be a coin press.
    - **And it is not written down**, on the field tile's and the spell gate's
      own terms: restored, a five-minute daemon is a permanent one whose caster no
      longer exists, standing as somebody's pet against a cap nothing will ever
      free.
    - **It goes out in a puff** either way — expiry, death or (later) a dispel all
      leave through one `unsummon`. ServUO's `UnsummonTimer` is silent, and a
      creature blinking out of existence with no feedback reads as a client
      glitch, so the art is its own `BaseCreature.Dispel` (`0x3728`, sound
      `0x201`). That flash is now `npc::flash`, shared with the guard who
      materialises in the same picture with a different noise.
    - Deferred: **Blade Spirits and Energy Vortex are summoned controlled here
      and the reference summons them free** (`BaseCreature.Summon(.., controlled:
      false, ..)`), which is why on OSI they famously turn on the mage who called
      them. Reproducing that wants a hostility model the engine has not got — its
      `acquire_phase` only ever acquires *players*, so an "uncontrolled" spirit
      would hunt the caster and walk past an orc. **Summon Creature's beasts share
      one stat block**: the reference draws eighteen classes with their own
      numbers, this draws nine bodies the engine can name over one modest woodland
      animal, because there is no per-body stat table to draw from and inventing
      eighteen is a bestiary rather than a spell. (**Dispel**, listed here as
      waiting only on being written, was written next — see the family below.)
  - **The roadmap still calls the unbuilt archetype `SpellEffect::Scripted`.** It
    is `SpellEffect::Unimplemented` in the tree and has been since the script-pack
    seam was retired; the entries above that name the old tag (the core-table
    slice, the cast-art slice, the buff and field slices) point at a variant that
    does not exist. Prose about a past decision, so nothing is wrong with the
    code — but a reader greps for the name and finds nothing.
  - [x] **Travel — Recall, Mark, Gate Travel, and the moongates.** The last big
    Magery family out of `Scripted`, and the first reader of `no_recall`, which
    had been carried through persistence, the converter and the script bridge
    since regions landed with nothing to consume it.
    - **`SpellTarget::Item`** is the fourth target kind: all three spells aim at
      a rune or a runebook, so they raise the *object* cursor (`0x6C` type 0) and
      the client itself refuses bare ground. A recall rune is a graphic plus a
      `RuneMark { facet, destination }`, and a blank one is a rune *without* the
      component — there is no `marked` flag to disagree with a destination that
      would mean nothing when false. (Gate Travel's reagent row was wrong while
      we were in it: blood moss where ServUO and the classic list both have black
      pearl.)
    - **The permission model is one end and one kind.** ServUO's
      `SpellHelper.CheckTravel` is a `bool[7,24]` matrix over twenty-four corners
      of Britannia; almost none exist here and the rest are region flags, so it
      collapses onto `no_teleport` and `no_recall`. What survives is the shape:
      the kinds are *directional*, and `RecallFrom` is the only permissive row,
      so a dungeon nobody may recall **into** is still one you may recall **out**
      of. Folding both ends into one call and testing them against a single kind
      reads tidier and makes every such region a one-way trap; a test caught
      exactly that, and the doc comment says why the tidy version is wrong.
      Sphere's four separate antimagic bits (`RECALL_IN`/`RECALL_OUT`/`GATE`/
      `TELEPORT`) are what the single bool collapses.
    - **Recall's refusals cost nothing**, in `begin_cast` before a point of mana:
      criminal, mid-fight, overloaded, holding something. The carry cap moved
      into `items` beside the walk that sums what is under it — three rules read
      `40 + 3.5 * str` now, and two copies is a shard where a mule can walk but
      cannot recall.
    - **Mark wants the rune in your own pack** (cliloc 1062422); **Recall does
      not**, because ServUO's target does not — a rune held out by a friend is a
      classic way to be fetched. The asymmetry is deliberate on both sides.
    - **Gate Travel lays a pair with no link field**: each gate points at the
      other's *tile*, so the link is the destination and there are not two halves
      to keep honest. Spawned the `spawn_field_tile` way and never through
      `items::spawn_item`, which would stamp a second, contradictory clock and
      announce an `ItemSpawned` the pack reads as a drop. Excluded from the save
      sweep beside `Field`, as ServUO deletes its own on deserialise: restored, a
      half-minute portal is a permanent one whose caster no longer exists.
    - **Walking in is found, not announced** (`tick/gates.rs`), off this tick's
      `MobileMoved` — there are two movement paths and a call beside each is one
      to forget, and unlike a position scan it cannot miss somebody who steps on
      and off inside one batch of commands.
    - **The nine city moongates carry no component at all.** Their destination is
      derived from where they stand, so they are saved and restored as ordinary
      decoration with no schema and no restore hook. They are placed *without* an
      obstruction, which is the thing here that would have been silently wrong:
      `place_decoration` seals a tile whose tiledata calls the graphic
      impassable, and a blocked gate is not a worse gate but one whose walk-in
      trigger is dead code that reads as a broken step check. Their list window
      is the first engine code to read a gump's **switches**.
    - **The runebook** binds sixteen destinations (the rune is consumed), spends
      charges for free travel, and pays the ordinary price on its Recall and Gate
      buttons through the one `magic::pay_and_roll`. Recharging leaves the
      surplus on the cursor rather than eating it. ServUO's button ids verbatim,
      decoded highest-range-first (`BOOK_USE_CHARGE + 40` would else read as a
      Recall), with a row the book does not hold refused rather than clamped.
    - **The facet change underneath** is `WorldState::move_to`, the one door every
      relocation now goes through. Five caches remember where a mobile is and none
      is compiler-checked: the traveller's own screen, every watcher's, the old
      facet's sector grid, `InRegion` (which gained the facet its id belongs to —
      region 3 on two facets compared equal, so a crossing between them fired no
      event, no music and no guards) and the walk sequence, whose reset was a
      latent bug in plain teleports too. The client is told with `0xBF 0x08` and
      the new `0x76`, never `0x1B`; the size in it comes from new `FacetState`
      dimensions, which also fixed login handing every facet Britannia's
      hardcoded 7168×4096. `[gameplay] cross_facet_travel` (off) is the classic
      pre-AoS refusal on top — a rule, not a missing feature.
    - **Schema v22** carries the rune and the book; one bump and not two, because
      there are no migrations and two inside one slice means throwing a test
      database away twice.
    - Deferred: Sacred Journey (decoded and ignored), the moon-phase gates,
      red/young travel restrictions, ship-mark runes, an `op_place_moongate` for
      the pack, and a tooltip that refreshes mid-life — a marked rune is the
      first thing in the world whose *name* changes.
  - [x] **The three dispels — Dispel, Mass Dispel, Dispel Field.** The counterspell
    family, and the one kind of spell that only ever *removes*: it needed the summon
    slice to land first and needs nothing after it. All three were
    `Unimplemented` with their question already answered — `Summoned` is the
    marker, and it had been read by the save sweep and the death path for a slice
    already.
    - **What a summon costs to send away is a table**, two columns beside the ones
      that say what it is (`state::summon`): `difficulty`, the Magery at which the
      roll is even, and `focus`, how steep the curve is either side of it — ServUO's
      `DispelDifficulty`/`DispelFocus`, in tenths like every other skill number here.
      `magic::dispel_chance` is the read site, `0.5 + (Magery - difficulty) /
      (focus * 2)` in tenths of a per-cent so the reference's halves stay exact.
      The two ends are the whole design: a blade spirit's `0.0/20.0` means anyone
      with any Magery at all sends it away, while a daemon's `125.0/45.0` sits
      *above* a grandmaster's entire skill, so what is dearest to call up is dearest
      to be rid of. Nothing is trained by the roll — Magery was trained by the cast
      that carried it, and the creature's difficulty is its class's, not something
      it learnt.
    - **A dispelled summon leaves by the door it already had**, `npc::unsummon`, so
      an expiry, a death and a dispel are one exit with one picture, and the
      follower slot needs no telling it is free — the count is derived from what
      stands in the world. ServUO says the same thing twice (`BaseCreature.Dispel`
      and `DispelSpell` play the identical `0x3728`/`0x201`); here it is said once.
    - **The art is the outcome's, so all three rows are `Silent`.** A dispel has two
      endings and a table row carries one: the puff of a creature that goes, or
      `FixedEffect(0x3779)` on one that holds. Voicing it from the table would sound
      a spell aimed at a rock, which is the reason Mark is silent too.
    - **Dispel Field aims at an object, not at the ground** — the fourth
      `SpellTarget::Item` spell, because ServUO builds its cursor with
      `allowGround: false` and the thing it unmakes *is* an item. One tile per cast,
      as in the reference, since a field is a row of separate tiles here as it is
      there; the obstruction goes with the tile through the existing
      `remove_field`, which is the half that would have been silently wrong — a
      dispelled stone wall that still blocks reads as a broken step check.
    - **A gate is dispellable and a city moongate is not, and that needed no flag.**
      ServUO carries `[DispellableField]` on `Moongate` while `PublicMoongate` is a
      plain `Item`; here the spell's gate carries the `Moongate` component and a
      city gate carries nothing at all, its meaning derived from where it stands —
      so the distinction the reference spends an attribute on is already the shape
      of the data. **Only the aimed end closes**, as in the reference: the far half
      stands out its half-minute as a one-way door, the pair having no link field
      to follow by design.
    - **Two wrong reagents fell out of writing the rows.** Dispel and Mass Dispel
      both named spider's silk where ServUO and the classic list have sulfurous
      ash — Gate Travel's blood moss again, the error that is invisible from every
      direction but the table: the spell casts, costs and works, and only charges
      for something the player never needed to buy.
    - Deferred: the caster's line of sight ("Target can not be seen", cliloc
      500237), unchecked here as it is everywhere else in this engine; ServUO's
      `SummonMaster == from || CheckHSequence` gate, which lets you always dispel
      your *own* summon and asks a harmful-action question about anyone else's;
      and `IsAnimatedDead`, the other half of `IsDispellable`, which waits on a
      necromancy that does not exist here.
  - [x] **Resurrection** — landed with the ghost slice: `SpellEffect::Resurrect`
    raises the aimed ghost through the core `resurrect` path (a no-op on the
    living).
  - **Polymorph** — still waiting on a subsystem of its own: a body-swap that
    restores cleanly.
  - **The Poisoning skill for the deadlier doses** — the Magery-cast dose caps
    at greater; the higher poison levels (deadly, lethal) want the Poisoning skill
    to set them.
