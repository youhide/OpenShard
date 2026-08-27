//! Combat: damage, death, war mode, the swing timer, and criminal flagging.
//!
//! A gameplay system in its own crate, operating on the shared [`WorldState`].
//! Damage passes through one door — [`damage`] applies the target's resistance
//! for the kind of damage, whether the blow came from a sword, a spell, or a
//! script — and emits [`MobileDamaged`], then [`MobileDied`] at zero. What death
//! *does* (loot, notoriety, a corpse) is a reader's to decide off that event;
//! combat says what happened and moves on.
//!
//! The interactive half is a small state machine over one component,
//! [`CombatAction`], run each tick against the tick counter so it reads no
//! clock. Three passes, in this order and once each:
//! [`commit_actions`] starts what a ready fighter promises, [`sustain_actions`]
//! applies the world to what is running and ends what the world has spoiled, and
//! [`resolve_actions`] lands what has reached its impact. A due swing held out
//! of reach becomes a fresh complete swing when the target enters reach. AI
//! drives the same machinery — a brain that hands a creature a `Combat` is
//! fought by these three exactly as a player is.
//!
//! **A blow, a shot and a breath are one schedule and three impacts.** They are
//! committed, sustained and resolved by the same three passes, and differ only
//! in what arrives: a projectile crossing the gap, a round leaving the pack.
//! That is why an archer is a body drawing a bow for the whole interval rather
//! than a statue that spits an arrow at the end of one.
//!
//! What the machine buys over the deadline it replaced is that an action can
//! *end*: every one of them finishes as a hit, a miss or a named interruption,
//! and the end crosses the wire, so a telegraph that was cancelled stops being
//! drawn instead of running out its promised duration over an empty tile. See
//! `docs/combat_actions.md`.

use std::collections::HashSet;

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_map::overlay::Doors;
use openshard_protocol::combat::{AttackTarget, WarMode};
use openshard_protocol::feedback::{
    ActionStage, BalkState, CombatActionOutcome, EffectKind, GraphicalEffect, InterruptReason,
};
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::serial::Serial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::wire::{Graphic, SoundId};
use openshard_protocol::world::{Point, PoisonLevel, RangedRange};
use openshard_state::action_rules::{ActionEffect, ActorCondition, ConditionSet};
use openshard_state::components::{
    ActionKind, Balked, BehaviourBuffs, Body, Client, Combat, CombatAction, CriminalUntil, DamageType,
    Frozen, Ghost, Guard, Hidden, Hitpoints, ItemAffix, ItemAffixes, MeleeDamage, MurderDecay, Murders,
    Phase, PoisonCharges, Poisoned, Position, RangedAttack, Resistance, Skills, Stamina, Stats, SwingSpeed,
    WrestlingAmbushCooldown, WrestlingCombo, WrestlingInterceptCooldown, WrestlingOpener, WrestlingStride,
    body_is_female, body_opens_doors, creature_base_sound,
};
use openshard_state::sectors::in_range;
use openshard_state::weapon::{ARROW, LAYER_ONE_HANDED, LAYER_TWO_HANDED, WeaponKind};
use openshard_state::{Action, Skill, TICKS_PER_SECOND, WorldState, WorldTick};

pub mod armor;
mod vitals;
pub mod weapons;

pub use vitals::{
    HITS_REGEN_TICKS, MOUNTED_STEPS_PER_STAMINA, OVERLOAD_ALLOWANCE, STAMINA_REGEN_TICKS, STEPS_PER_STAMINA,
    regen_hits, regen_stamina, spend_step_stamina,
};

/// How near, in tiles (Chebyshev), a mobile must be to land a melee blow: the
/// next tile over, diagonals included.
pub const MELEE_RANGE: u32 = 1;
/// The same tile count as [`MELEE_RANGE`], as the reach newtype every action
/// commits to.
///
/// A constant rather than a weapon row, and that is the seam a polearm at two
/// tiles falls on: reach is a number in the ranged half of the weapon table and
/// a constant in the melee half, and making it one column is a phase of its own.
pub const MELEE_REACH: RangedRange = match RangedRange::new(MELEE_RANGE as u8) {
    Some(reach) => reach,
    None => panic!("melee reach is one tile, which is not zero"),
};
/// What an ambush from cover adds to the hit roll, in percent — captured at the
/// commit and spent once when the action resolves.
const AMBUSH_ACCURACY_PERCENT: i16 = 25;
/// The swing base of bare hands — Sphere's wrestling value. A wielded weapon
/// supplies its own base from [`weapons`]; a mobile holding nothing (or holding
/// something not in the weapon table) falls back to this, modulated by dexterity.
pub const WRESTLING_SPEED: u64 = 50;
/// The dexterity a mobile with no [`Stats`] swings at.
const DEFAULT_DEXTERITY: u16 = 100;
/// Damage a swing deals. A flat number until the damage formula — resistances,
/// weapon, strength — is written, and that is a script-first slice of its own.
pub const SWING_DAMAGE: u16 = 5;
/// A hidden wrestler has this long to reach the target they selected before the
/// opening disappears.  It keeps an ambush immediate without making it a buff
/// one can carry across the map.
const WRESTLING_OPENER_TICKS: u64 = 2 * TICKS_PER_SECOND;
/// A victim can be ambushed again only after this recovery.
const WRESTLING_AMBUSH_COOLDOWN_TICKS: u64 = 12 * TICKS_PER_SECOND;
/// Three quick steps are enough to turn a normal first swing into an intercept.
const WRESTLING_INTERCEPT_STEPS: u8 = 3;
/// Footwork goes stale quickly; walking in circles before a fight is not a
/// charge.
const WRESTLING_STRIDE_TICKS: u64 = TICKS_PER_SECOND + TICKS_PER_SECOND / 2;
/// Intercept is a first-contact privilege, not a permanent attack-speed bonus.
const WRESTLING_INTERCEPT_COOLDOWN_TICKS: u64 = 8 * TICKS_PER_SECOND;
/// A combo must remain continuous.
const WRESTLING_COMBO_TICKS: u64 = 6 * TICKS_PER_SECOND;
/// The third consecutive unarmed hit gets this much extra damage.
const WRESTLING_COMBO_DAMAGE_PERCENT: u16 = 35;
/// The human unarmed thwack — ServUO's `Fists.HitSound`, the fallback for a body
/// with no creature sound of its own (a player, a townsperson). A creature makes
/// its own attack sound instead; see [`attack_sound`].
pub const MELEE_HIT_SOUND: SoundId = SoundId(0x0137);
/// The whistle of a blow that finds only air — a swing that missed. Coarse (one
/// swish for every weapon, not ServUO's per-weapon `DefMissSound`), but a miss is
/// no longer silent, so the client reads the whiff.
pub const MELEE_MISS_SOUND: u16 = 0x0238;
/// The twang of a bow — ServUO's `BaseRanged.DefHitSound`, the fallback for a
/// humanoid archer; a creature that shoots uses its own sound.
pub const RANGED_HIT_SOUND: SoundId = SoundId(0x0234);

/// A creature's `BaseSoundID` from its body, or `None` for a human or an unlisted
/// body — the key both [`attack_sound`] and [`death_sound`] read.
fn body_base_sound(state: &WorldState, entity: EntityId) -> Option<SoundId> {
    creature_base_sound(state.registry.get::<Body>(entity)?.id)
}

/// The sound `attacker` makes landing a blow: a creature's own attack sound
/// (ServUO's `BaseSoundID + 2`), or the human fists thwack. So an orc growls its
/// attack instead of punching like a man, which was the point of the sound rule.
fn attack_sound(state: &WorldState, attacker: EntityId, humanoid_fallback: SoundId) -> SoundId {
    // The offset is arithmetic on the base, so the base is opened for it and the
    // result named again — ServUO's `BaseSoundID + 2`.
    body_base_sound(state, attacker)
        .map(|base| SoundId(base.0.wrapping_add(2)))
        .unwrap_or(humanoid_fallback)
}

/// The growl a creature makes noticing prey — ServUO's `GetAngerSound`
/// (`BaseSoundID + 0`). `None` for a human, which does not growl. The `ai` plays
/// it on the aggro transition, so a monster announces itself when it sees you.
pub fn anger_sound(state: &WorldState, entity: EntityId) -> Option<SoundId> {
    body_base_sound(state, entity)
}

/// The sound `victim` makes dying: a creature's death sound (`BaseSoundID + 4`), a
/// humanoid's gendered death cry (ServUO's `Random(0x423, 5)` male / `Random(0x314,
/// 4)` female, drawn from the tick's seeded rng so a death replays), or `None` for
/// the passive fauna ServUO leaves silent.
fn death_sound(state: &mut WorldState, victim: EntityId) -> Option<SoundId> {
    let body = state.registry.get::<Body>(victim)?.id;
    if let Some(base) = creature_base_sound(body) {
        return Some(SoundId(base.0.wrapping_add(4)));
    }
    if body_opens_doors(body) {
        return Some(SoundId(if body_is_female(body) {
            0x0314 + state.rng.below(4) as u16
        } else {
            0x0423 + state.rng.below(5) as u16
        }));
    }
    None
}
/// The arrow that flies from a bow — ServUO's `Bow.EffectID`. A moving graphical
/// effect draws it crossing the gap to the mark.
const ARROW_GRAPHIC: Graphic = Graphic(0x0F42);
/// How fast the arrow crosses, ServUO's `MovingEffect` speed for a bow shot.
const RANGED_EFFECT_SPEED: u8 = 18;

/// A mobile took damage.
///
/// Emitted whenever hit points fall — the hook combat gives everything that
/// cares without combat having to know who does: a health bar redraw, an
/// aggression tracker, a script that heals its pet. This is the crate boundary
/// the architecture is built on — combat says what happened and moves on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MobileDamaged {
    /// The mobile.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// How much it lost.
    pub amount: u16,
    /// What it has left.
    pub remaining: u16,
    /// Who dealt it, when the blow had an author — what retaliation reads.
    pub by: Option<Serial>,
}

/// A mobile died — its hit points reached zero.
///
/// The event the whole "systems emit, they do not call" rule is named for:
/// combat emits this, and loot, notoriety, guild war scores and quests read it,
/// none of them wired into combat. What death *does* — a corpse, a ghost, a
/// resurrection — is not decided here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MobileDied {
    /// The mobile.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Its body — so a pack can tell *what* died (a rat, an orc) for a kill quest
    /// without a second lookup. `0` if it somehow has none.
    pub body: Graphic,
    /// Who dealt the killing blow, if known — carried so a pack can attribute a
    /// kill (a quest's "slay N", a bounty). `None` for a death with no attacker: a
    /// field or a reflected blow, a script's unattributed damage.
    pub killer: Option<Serial>,
}

/// Ticks between swings for a mobile of dexterity `dex` wielding a weapon of base
/// speed `base`, under combat `era` with scale factor `scale` — Sphere's
/// `Calc_CombatAttackSpeed` (`CResourceCalc.cpp`).
///
/// Both implemented eras start from `(dex + 100) * base` and divide the scale by
/// it, so higher dexterity or a faster weapon means fewer ticks; they differ in
/// the floor and the halving AoS added:
///
/// - **Era 1 (pre-AoS):** the swing takes `(scale * 10) / ((dex + 100) * base)`
///   tenths of a second, floored at one tenth.
/// - **Era 2 (AoS):** the same, halved, floored at 1.2s (twelve tenths).
///
/// The reference's answer is in tenths of a second and the shard counts ticks, so
/// the result is scaled by [`TICKS_PER_SECOND`] over ten. Written that way and not
/// as the number it currently comes to: this used to be a bare `* 2`, correct at
/// the 50ms tick it was written under, and it silently halved every swing on the
/// shard the day the tick became 25ms.
/// Eras 0, 3 and 4 need weapon weight or ML-format speeds the shard has no data
/// for yet, so config validation accepts only 1 and 2; an
/// unknown era here falls back to era 1.
///
/// The eras are Sphere's `m_iCombatSpeedEra` (`CResourceCalc.cpp`): `0` custom,
/// `1` pre-AoS, `2` AoS, `3` SE, `4` ML. Each takes a different `base` — pre-AoS
/// eras the `old_speed`, AoS/SE the `aos_speed`, ML the `ml_speed` in hundredths of
/// a second — which [`openshard_state::weapon::swing_base`] picks. `scale` is the operator's
/// `speed_scale_factor` (15000 pre-AoS, 40000 AoS, 80000 SE; ML ignores it).
#[must_use]
pub const fn swing_ticks(dex: u16, base: u64, era: u8, scale: u64) -> u64 {
    let base = if base == 0 { 1 } else { base };
    let dex = dex as u64;
    let denom = (dex + 100) * base;
    let tenths = match era {
        // AoS: half the pre-AoS interval, floored at 1.25s (12 tenths).
        2 => {
            let t = ((scale * 10) / denom) / 2;
            if t < 12 { 12 } else { t }
        }
        // SE: `scale/((dex+100)·speed) - 2` in 0.25s ticks, floored at 5, then
        // converted to tenths (`·10/4`). `scale` is 80000.
        3 => {
            let ticks = (scale / denom).saturating_sub(2);
            let ticks = if ticks < 5 { 5 } else { ticks };
            (ticks * 10) / 4
        }
        // ML: `speed·4 - dex/30` in 0.25s ticks, floored at 5, then tenths. `base`
        // is `ml_speed` in hundredths of a second (so `·4/100`), and ML ignores
        // `scale` entirely.
        4 => {
            let ticks = ((base * 4) / 100).saturating_sub(dex / 30);
            let ticks = if ticks < 5 { 5 } else { ticks };
            (ticks * 10) / 4
        }
        // Sphere custom (0): pre-AoS with a 0.5s (5-tenths) floor.
        0 => {
            let t = (scale * 10) / denom;
            if t < 5 { 5 } else { t }
        }
        // Pre-AoS (1) and the fallback.
        _ => {
            let t = (scale * 10) / denom;
            if t == 0 { 1 } else { t }
        }
    };
    tenths * TICKS_PER_SECOND / 10
}

/// Deal damage to a mobile, of a kind its resistance to that kind reduces.
///
/// `attacker` is who dealt it, if anyone — the melee swinger, or the caster a
/// script names on a spell's damage. It is the whole of murder attribution: a
/// lethal blow that leaves a blue mobile dead tallies against the attacker, so a
/// fireball counts the same as a sword. Unattributed damage (a script's raw
/// `Command::Damage` with no `by`, an environmental hazard) kills without blame.
pub fn damage(
    state: &mut WorldState,
    serial: Serial,
    amount: u16,
    kind: DamageType,
    attacker: Option<Serial>,
) {
    let Some(entity) = state.registry.entity_of(serial) else {
        return;
    };
    let Some(&Hitpoints { current, max }) = state.registry.get::<Hitpoints>(entity) else {
        return;
    };
    // Already dead — a player lying at zero, not yet a ghost. A further blow does
    // nothing, and in particular does not announce a second death.
    if current == 0 {
        return;
    }
    // The victim's standing has to be read before it dies — killing a blue is
    // what a murder is.
    let victim_was_blue = matches!(
        state.notoriety_of(entity),
        Notoriety::Innocent | Notoriety::Friend
    );
    // Armour takes its cut, of this kind of damage. One place now, so a fireball
    // and a sword swing both go through the same door.
    let resist = state
        .registry
        .get::<Resistance>(entity)
        .map_or(0, |r| r.against(kind));
    let amount = (u32::from(amount) * u32::from(100 - resist) / 100) as u16;
    let remaining = current.saturating_sub(amount);
    state.registry.insert(
        entity,
        Hitpoints {
            current: remaining,
            max,
        },
    );
    state.bus.send(MobileDamaged {
        entity,
        serial,
        amount,
        remaining,
        by: attacker,
    });
    state.broadcast_health(entity);
    // A blow wakes a paralyzed mobile — ServUO clears `Paralyzed` inline in
    // `Mobile.Damage`. Any real (post-resist) damage lifts it at once, and breaks
    // concentration with it: nobody holds a trance through a sword.
    if amount > 0 {
        state.registry.remove::<Frozen>(entity);
        // And it gives away anyone hiding: you cannot be struck and stay unseen.
        state.break_cover(entity);
        // D5's second seam. This is the one door every wound passes — a blow, an
        // arrow, a spell, a reflected hit — so a rule about being struck is
        // pushed from here and never gone looking for by a pass. What it does is
        // the shard's table's business: the shipped one lets a fighter swing
        // through a wound, and `struck = "break"` is how an operator says
        // otherwise.
        apply_condition(state, entity, ActorCondition::Struck);
    }
    // Reactive Armor bounces a share of a melee physical blow back at the
    // attacker. The reflected hit is unattributed (attacker `None`), which both
    // breaks the recursion — a reflected blow never reflects again — and keeps a
    // reflect kill blameless.
    if kind == DamageType::Physical && amount > 0 {
        if let Some(attacker_serial) = attacker {
            if let Some(pct) = state
                .registry
                .get::<BehaviourBuffs>(entity)
                .and_then(|b| {
                    b.active
                        .iter()
                        .find(|x| x.kind == openshard_state::BehaviourBuffKind::REACTIVE_ARMOR)
                })
                .map(|x| x.amount)
            {
                let reflected = (u32::from(amount) * pct.max(0) as u32 / 100) as u16;
                if reflected > 0 {
                    damage(state, attacker_serial, reflected, DamageType::Physical, None);
                }
            }
        }
    }
    if remaining == 0 {
        if victim_was_blue {
            if let Some(killer) = attacker.and_then(|s| state.registry.entity_of(s)) {
                record_murder(state, killer);
            }
        }
        die(state, entity, serial, attacker);
    }
}

/// A mobile's hit points reached zero.
///
/// Emits [`MobileDied`] for whoever cares — loot, notoriety, a script — and then,
/// for a creature, takes it off the world. A *player* who dies stays put for now:
/// ghosts, corpses and resurrection are a later slice, and despawning someone
/// still connected is worse than leaving them standing.
pub fn die(state: &mut WorldState, entity: EntityId, serial: Serial, killer: Option<Serial>) {
    // The death throe and cry, while the mobile is still on screen to play them:
    // a wolf's yelp, a human's death gasp.
    state.animate(entity, Action::Die);
    if let Some(sound) = death_sound(state, entity) {
        state.play_sound(entity, sound);
    }
    // Announce it and stop. What becomes of the body — a corpse for a creature, a
    // ghost for a player — is the world's job off this event (the tick's `reap`);
    // combat reports the death, it does not dispose of the body. A player is left
    // standing at zero hits for now (ghosts are a later slice); a creature the
    // world turns into a corpse and takes off the map.
    let body = state.registry.get::<Body>(entity).map_or(Graphic(0), |b| b.id);
    state.bus.send(MobileDied {
        entity,
        serial,
        body,
        killer,
    });
}

/// Set a player's war stance and tell it the settled one.
pub fn war_mode(state: &mut WorldState, connection: ConnectionId, war: bool) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    // The packet is an intent. Missing session state is an invariant violation,
    // not evidence that the player must be at peace; reject it without
    // manufacturing state. A ghost likewise settles at peace.
    let requested = war && attackable(state, player);
    let Some((previous_war, previous_target)) = state
        .registry
        .get::<Combat>(player)
        .map(|combat| (combat.warmode(), combat.target()))
    else {
        debug_assert!(false, "a connected player must carry combat session state");
        return;
    };
    let combat = state
        .registry
        .get_mut::<Combat>(player)
        .expect("the combat row read immediately above cannot disappear");
    if requested {
        let transitioned = combat.enter_war();
        debug_assert!(
            transitioned,
            "a player session cannot carry a creature-only combat state"
        );
    } else {
        combat.leave_combat();
    }
    let war = state
        .registry
        .get::<Combat>(player)
        .is_some_and(|combat| combat.warmode());
    let target = state
        .registry
        .get::<Combat>(player)
        .and_then(|combat| combat.target());
    let changed = previous_war != war;
    state.send_packet(connection, &ServerPacket::WarMode(WarMode { war }));
    if previous_target != target {
        state.send_packet(connection, &ServerPacket::AttackTarget(AttackTarget { target }));
    }
    if changed {
        state.broadcast_move(player);
    }
}

/// Set a player's attack target. The blow itself is not struck here — this only
/// aims; [`commit_actions`] turns "in war mode, in reach, recovery up" into a
/// blow or a shot that is on its way.
pub fn attack(state: &mut WorldState, connection: ConnectionId, target: Option<Serial>) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    // The player-session invariant is the same one `war_mode` enforces. A dead
    // player cannot aim; disengaging also corrects any stale stance/marker held
    // by its client.
    if !attackable(state, player) {
        state.disengage(player);
        return;
    }
    let Some(_) = state.registry.get::<Combat>(player) else {
        debug_assert!(false, "a connected player must carry combat session state");
        return;
    };
    // A target that cannot be attacked — a serial of zero, an item, the attacker
    // itself, a corpse/ghost, or an invulnerable mobile — clears the aim and
    // un-highlights the client's bar.  A ghost retains its mobile serial so the
    // client can walk it to a healer, but it is not a combat target.
    let valid = target
        .and_then(|serial| state.registry.entity_of(serial).map(|entity| (serial, entity)))
        .filter(|&(_, entity)| {
            entity != player
                && attackable(state, entity)
                && state.notoriety_of(entity) != Notoriety::Invulnerable
        });
    let Some((serial, target_entity)) = valid else {
        clear_target(state, player);
        state.send_packet(
            connection,
            &ServerPacket::AttackTarget(AttackTarget { target: None }),
        );
        return;
    };
    // Wrestling owns the first contact rather than the whole exchange.  An
    // ambush from cover wins it outright; recent footwork halves the usual wait.
    // Both are target-bound and cooldown-gated before the mutable Combat borrow.
    let now = state.ticks;
    let unarmed = is_wrestling(state, player);
    let previous_target = state
        .registry
        .get::<Combat>(player)
        .and_then(|combat| combat.target());
    let ambush = unarmed
        && state.registry.has::<Hidden>(player)
        && state
            .registry
            .get::<WrestlingAmbushCooldown>(player)
            .is_none_or(|cooldown| now >= cooldown.until);
    let intercept = unarmed
        && !ambush
        && previous_target != Some(serial)
        && state
            .registry
            .get::<WrestlingStride>(player)
            .is_some_and(|stride| stride.steps >= WRESTLING_INTERCEPT_STEPS && now <= stride.expires_at)
        && state
            .registry
            .get::<WrestlingInterceptCooldown>(player)
            .is_none_or(|cooldown| now >= cooldown.until);
    let pace = swing_speed(state, player);
    let next = if ambush {
        now
    } else if intercept {
        now + (pace / 2).max(1)
    } else {
        now + pace
    };
    let aimed = state
        .registry
        .get_mut::<Combat>(player)
        .is_some_and(|combat| combat.aim(serial, next));
    if !aimed {
        state.send_packet(
            connection,
            &ServerPacket::AttackTarget(AttackTarget { target: None }),
        );
        return;
    }
    // A new aim abandons whatever was being swung at the old one: an action is
    // committed to a target, and does not follow a change of mind. Face a
    // reachable target immediately; `commit_actions` starts the gesture this
    // tick and tells the client how long it lasts before impact.
    state.end_combat_action(
        player,
        CombatActionOutcome::Interrupted(InterruptReason::Abandoned),
    );
    if melee_reachable(state, player, target_entity) {
        state.face_toward(player, target_entity);
    }
    if ambush {
        state.registry.insert(
            player,
            WrestlingOpener {
                target: serial,
                expires_at: now + WRESTLING_OPENER_TICKS,
            },
        );
        state.registry.insert(
            player,
            WrestlingAmbushCooldown {
                until: now + WRESTLING_AMBUSH_COOLDOWN_TICKS,
            },
        );
    }
    if intercept {
        state.registry.remove::<WrestlingStride>(player);
        state.registry.insert(
            player,
            WrestlingInterceptCooldown {
                until: now + WRESTLING_INTERCEPT_COOLDOWN_TICKS,
            },
        );
    }
    // Raising a hand against someone blue or green is a crime — it turns the
    // attacker grey. (Flagged on the attack, not the landed blow: close enough,
    // and it is the intent a town guard would act on.)
    if matches!(
        state.notoriety_of(target_entity),
        Notoriety::Innocent | Notoriety::Friend
    ) {
        flag_criminal(state, player);
    }
    state.send_packet(
        connection,
        &ServerPacket::AttackTarget(AttackTarget { target: Some(serial) }),
    );
}

/// Put a war-mode player under attack onto the attacker, if they have not
/// selected a target already.
///
/// This is retaliation rather than a new aggressive action: it deliberately
/// bypasses [`attack`], which would flag the defending player criminal for
/// naming an innocent attacker.  Creatures make the corresponding choice in
/// `ai::retaliate`; players have no `Brain`, so their combat state belongs here.
pub fn retaliate_players(state: &mut WorldState, blows: &[MobileDamaged]) {
    for blow in blows {
        let Some(attacker) = blow.by else {
            continue;
        };
        let Some(attacker_entity) = state.registry.entity_of(attacker) else {
            continue;
        };
        if attacker == blow.serial {
            continue;
        }
        let victim = blow.entity;
        let Some(connection) = state.connection_of(victim) else {
            continue;
        };
        let ready_to_retaliate = state
            .registry
            .get::<Combat>(victim)
            .is_some_and(|combat| combat.warmode() && combat.target().is_none())
            && state
                .registry
                .get::<Hitpoints>(victim)
                .is_some_and(|hits| hits.current > 0);
        if !ready_to_retaliate {
            continue;
        }
        let next_swing = state.ticks + swing_speed(state, victim);
        if let Some(combat) = state.registry.get_mut::<Combat>(victim) {
            combat.aim(attacker, next_swing);
        }
        // A defensive target selection is visible immediately.  Waiting for the
        // first swing leaves a war-mode player staring at their previous target
        // for a full swing delay despite already aiming back at the attacker.
        state.face_toward(victim, attacker_entity);
        state.send_packet(
            connection,
            &ServerPacket::AttackTarget(AttackTarget {
                target: Some(attacker),
            }),
        );
    }
}

/// The action a ranged attacker would open, or `None` for a fighter who has no
/// way to reach anything but arm's length.
///
/// Either a creature's innate [`RangedAttack`], or a mobile that merely wields a
/// `WeaponKind::Ranged` weapon — read fresh at every commit the same way
/// [`equipped_weapon`](weapons::equipped_weapon) itself is, so no component is
/// ever mirrored onto a player for holding a bow. The innate attack is asked
/// first: a creature that breathes fire does not stop breathing it for picking
/// up a crossbow.
fn ranged_action(state: &WorldState, attacker: EntityId) -> Option<ActionKind> {
    if let Some(ranged) = state.registry.get::<RangedAttack>(attacker) {
        return Some(ActionKind::Breath {
            reach: ranged.range,
            damage: ranged.kind,
            art: ARROW_GRAPHIC,
        });
    }
    let weapon = weapons::equipped_weapon(state, attacker)?;
    if weapon.kind != WeaponKind::Ranged {
        return None;
    }
    // All three shipped ranged rows carry a reach, a round and a flight graphic
    // together (`weapon::ranged` sets them in one place). A row with a ranged
    // kind and none of them is a hole in the table rather than a weapon that
    // shoots for free, and its bearer falls back on clubbing with it.
    Some(ActionKind::Shot {
        reach: weapon.range?,
        nocked: weapon.ammo?,
        art: weapon.effect_art.unwrap_or(ARROW_GRAPHIC),
    })
}

/// How far `attacker` can presently strike — the weapon's own reach, or arm's
/// length for everyone who has none.
///
/// The same reading [`commit_actions`] takes at the commit, minus the action it
/// builds around it. It exists because half of an [`obstruction`] refusal is a
/// number nobody outside this crate could say: `.sight` reports a clear look
/// that is still not a shot, and "clear, and fourteen tiles from a bow that
/// reaches ten" is the whole of that answer.
#[must_use]
pub fn reach_of(state: &WorldState, attacker: EntityId) -> RangedRange {
    ranged_action(state, attacker).map_or(MELEE_REACH, ActionKind::reach)
}

/// Whether `attacker`'s pack holds a round of the kind its shot wants.
///
/// A read and not a draw: the round leaves the pack at the loose, so what the
/// nock needs is only the answer to "is there one", and an interrupted draw has
/// nothing to hand back. A mobile with no pack at all — every creature — carries
/// no rounds, which is why a beast that shoots does it with a [`RangedAttack`]
/// and not with a bow.
fn carries_round(state: &WorldState, attacker: EntityId, round: Graphic) -> bool {
    let Some(serial) = state.registry.serial_of(attacker) else {
        return false;
    };
    openshard_items::backpack_of(state, serial)
        .is_some_and(|pack| openshard_items::count_in_container(state, pack, round) > 0)
}

/// What a shot that could not be fired for want of ammunition tells the shooter —
/// ServUO's `BaseRanged` refusal, split by which the weapon wants.
fn out_of_ammo_message(ammo: Graphic) -> &'static str {
    if ammo == ARROW {
        "You do not have enough arrows."
    } else {
        "You do not have enough bolts."
    }
}

/// The flight of a shot, from where it was loosed to the mark it was aimed at.
///
/// Emitted before the blow lands, so the mark is still drawn for the arrow to
/// fly at: a target killed by the same impact is swept off the map a moment
/// later, and an effect addressed to it then would cross the wire naming
/// nobody.
fn projectile(from: Point, to: Point, by: Option<Serial>, at: Serial, art: Graphic) -> GraphicalEffect {
    GraphicalEffect {
        kind: EffectKind::Moving,
        from: by,
        to: Some(at),
        art,
        from_point: from,
        to_point: to,
        speed: RANGED_EFFECT_SPEED,
        duration: 1,
        fixed_direction: false,
        explode: false,
    }
}

/// Land every action whose impact has arrived — the third of the four verbs.
///
/// What is *not* here is the point. Reach, sight, pacification and the target's
/// life were asked by [`sustain_actions`] a moment ago, in this same tick, and
/// what fails there ends the action with a name instead of vanishing. Two things
/// can still have changed since: another blow in this very pass may have killed
/// the target, and only that is re-asked.
pub fn resolve_actions(state: &mut WorldState) {
    let now = state.ticks;
    // Collected first: `damage` mutates the registry, so the query cannot be held
    // across it.
    let due: Vec<EntityId> = state
        .registry
        .query::<CombatAction>()
        .filter(|(_, action)| action.impact().is_some_and(|impact| now >= impact))
        .map(|(attacker, _)| attacker)
        .collect();

    for attacker in due {
        // Re-read rather than carry a copy from the collection above: a blow
        // struck earlier in *this* pass reaches its victim through `damage`,
        // which pushes `Struck` at whatever the victim was doing — and the
        // shard's table may have ended it or pushed its impact away. Resolving
        // the snapshot would land a blow the rules had already taken back.
        let Some(&action) = state.registry.get::<CombatAction>(attacker) else {
            continue;
        };
        if !action.impact().is_some_and(|impact| now >= impact) {
            continue;
        }
        let target_serial = action.target;
        // A blow struck earlier in this same pass may have killed it. A player
        // also remains a mobile after death as a ghost, so resolving the serial
        // is not enough on its own.
        let Some(target) = state
            .registry
            .entity_of(target_serial)
            .filter(|&target| attackable(state, target))
        else {
            state.end_combat_action(
                attacker,
                CombatActionOutcome::Interrupted(InterruptReason::TargetGone),
            );
            clear_target(state, attacker);
            continue;
        };
        let Some(&Position(target_pos)) = state.registry.get::<Position>(target) else {
            state.end_combat_action(
                attacker,
                CombatActionOutcome::Interrupted(InterruptReason::TargetGone),
            );
            continue;
        };
        // The attacker's serial rides along so a lethal blow can be blamed —
        // `damage` is the one place murder is tallied, melee or spell alike.
        let by = state.registry.serial_of(attacker);
        // Swinging at somebody is the loudest thing you can do — ServUO calls
        // `RevealingAction` in the combat timer, before the blow is even rolled.
        state.break_cover(attacker);
        // A telegraphed action played its stroke at the commit and stretched it
        // to exactly this moment. An untelegraphed one — a concealed fighter's
        // opener, which had no wind-up to give away — is drawn here instead, and
        // turns here too: the owner ignores `0x77` and needs the accompanying
        // `0x20` player update, which is what `face_point` sends.
        if !action.telegraphed {
            state.face_point(attacker, target_pos);
            state.animate(attacker, Action::Attack);
        }
        // The round leaves the pack here, at the loose, and this is the one
        // thing the commit could not settle: an archer may have dropped, traded
        // or drunk away their quiver while the bow was bending. It costs the
        // shot and says so by name rather than by silence.
        if let Some(round) = action.kind.round() {
            let drawn =
                by.is_some_and(|shooter| openshard_items::take_from_backpack(state, shooter, round, 1) > 0);
            if !drawn {
                state.system_message(attacker, out_of_ammo_message(round));
                set_next_swing(state, attacker, now + swing_speed(state, attacker));
                state.end_combat_action(
                    attacker,
                    CombatActionOutcome::Interrupted(InterruptReason::NoAmmo),
                );
                continue;
            }
        }
        // A shot announces itself whichever way the roll goes: the bolt flew and
        // twanged before anyone could know whether it would land. A blow is the
        // other way about — its thwack *is* the sound of landing, and a whiff
        // has a whistle of its own below.
        let flight = action.kind.flight();
        if let Some(art) = flight {
            let Some(&Position(from)) = state.registry.get::<Position>(attacker) else {
                continue;
            };
            let arrow = projectile(from, target_pos, by, target_serial, art);
            state.broadcast_packet(attacker, &ServerPacket::Effect(arrow));
            let twang = attack_sound(state, attacker, RANGED_HIT_SOUND);
            state.play_sound(attacker, twang);
        }
        // Roll to hit (and train the weapon skill by trying), spending whatever
        // the action accumulated on its way here. A miss whistles past and does
        // no damage; the timer resets either way.
        if !check_hit(state, attacker, target, action.accuracy) {
            if flight.is_none() {
                state.registry.remove::<WrestlingCombo>(attacker);
                state.play_sound(attacker, SoundId(miss_sound(state, attacker)));
            }
            set_next_swing(state, attacker, now + swing_speed(state, attacker));
            state.end_combat_action(attacker, CombatActionOutcome::Miss);
            continue;
        }
        let mut blow = scaled_blow(state, attacker, target);
        // Wrestling is bare hands finding a body, so an arrow and a breath are
        // outside it entirely: neither continues a chain nor breaks one.
        if flight.is_none() {
            if is_wrestling(state, attacker) {
                if wrestling_combo_lands(state, attacker, target_serial) {
                    blow.amount = (u32::from(blow.amount) * (100 + u32::from(WRESTLING_COMBO_DAMAGE_PERCENT))
                        / 100) as u16;
                    restore_wrestling_stamina(state, attacker);
                    state.system_message(attacker, "Combo strike!");
                }
            } else {
                // A weapon hit interrupts a bare-handed sequence even if the
                // fighter puts it away before the combo window expires.
                state.registry.remove::<WrestlingCombo>(attacker);
            }
        }
        // The blow lands with the attacker's own thwack — a creature's growl, a
        // human's fist. Read before the damage, because Reactive Armor can kill
        // the attacker with its own blow and a corpse has no growl left; a shot
        // has no thwack at all, having already twanged on the way out.
        let thwack = flight
            .is_none()
            .then(|| attack_sound(state, attacker, MELEE_HIT_SOUND));
        damage(state, target_serial, blow.amount, action.kind.damage_type(), by);
        if blow.critical {
            state.system_message(attacker, "Critical hit!");
        }
        if let Some(sound) = thwack {
            state.play_sound(attacker, sound);
        }
        // A coated blade spends a dose into whatever it just cut, and a weapon
        // whose affixes carry poison rolls them — a bow among them, which is
        // ServUO's rule too: `BaseRanged.OnHit` is `BaseWeapon.OnHit` with a
        // flight in front of it. The Poisoning skill itself still refuses to
        // smear a bow (it coats blades and points only), so a *coating* remains
        // melee's alone by the skill's rule rather than by a branch here.
        deliver_weapon_poison(state, attacker, target_serial, now);
        deliver_affix_poison(state, attacker, target_serial, now);
        set_next_swing(state, attacker, now + swing_speed(state, attacker));
        state.end_combat_action(attacker, CombatActionOutcome::Hit);
        // The blow may have killed it; a dead target is no target. Dead means gone
        // *or* standing at zero hits — a creature killed this tick is not swept off
        // the map until the tick's `reap`, so the entity still resolves for a beat.
        if target_is_dead(state, target_serial) {
            clear_target(state, attacker);
        }
    }
}

/// Start an action for every fighter who is ready for one — the first verb.
///
/// Every precondition is tested *here*, and what the fighter promises is frozen
/// into the component: the target, the reach it is committed to, and — for a
/// shot — which round it will spend and what that round flies as. The server
/// owns the timer, including operator combat settings, dexterity, scripted
/// [`SwingSpeed`], range and live sight, and sending the ordinary action at the
/// start of that authoritative interval keeps the client synchronized without
/// asking it to duplicate rules it cannot fully know. A due swing that was held
/// out of reach opens a full interval when the fighters meet, rather than
/// landing before a frame can be shown.
///
/// Only [`Phase::Releasing`] is reachable: arming is the last phase of
/// `docs/combat_actions.md` and nothing commits into a watch yet.
pub fn commit_actions(state: &mut WorldState) {
    let now = state.ticks;
    let pending: Vec<(EntityId, Serial, WorldTick)> = state
        .registry
        .query::<Combat>()
        .filter_map(|(attacker, combat)| {
            // One action at a time. A fighter already swinging is sustained, not
            // committed again — that is what makes the wind-up a process rather
            // than a marker to be re-stamped every tick.
            (combat.warmode() && attackable(state, attacker) && !state.registry.has::<CombatAction>(attacker))
                .then(|| Some((attacker, combat.target()?, combat.next_swing()?)))?
        })
        .collect();

    // Whoever this pass refused, and why. Everything else that holds a `Balked`
    // is cleared at the end — a fighter who committed, who stopped fighting, or
    // whose obstacle is gone. **Every path out of the loop below records one or
    // the other**: a `continue` that leaves this set untouched is the silent
    // refusal this pass was built to stop having.
    let mut balked: HashSet<EntityId> = HashSet::new();

    for (attacker, target_serial, due) in pending {
        // A mobile a bard has calmed does not start one — ServUO's `BardPacified`.
        if state
            .registry
            .has::<openshard_state::components::Pacified>(attacker)
        {
            balk(state, &mut balked, attacker, InterruptReason::Pacified);
            continue;
        }
        let Some(target) = state
            .registry
            .entity_of(target_serial)
            .filter(|&target| attackable(state, target))
        else {
            // The target is gone — a creature killed, a player logged out, or a
            // player still standing as a ghost, which keeps its mobile serial and
            // is not a combat target. Dropping a stale aim is the swing beat's
            // job, so it survives exactly as long as the timer that would have
            // struck it: without the guard, monsters keep aiming at a ghost until
            // their next AI beat notices.
            if now >= due {
                clear_target(state, attacker);
            }
            balk(state, &mut balked, attacker, InterruptReason::TargetGone);
            continue;
        };
        // Which of the three this fighter is about to make. Whoever can shoot
        // shoots, at arm's length as readily as across a field: ServUO puts no
        // floor under a bow's range either, and the old `volleys` refusal to
        // fire inside [`MELEE_RANGE`] only existed because the melee pass would
        // otherwise strike in the same beat. There is one pass now.
        //
        // The melee half commits to a constant rather than a weapon row until
        // reach becomes data, and the polearm falls exactly on that seam; the
        // ranged half already reads its reach off the weapon, which is what
        // makes the seam visible here.
        let kind = ranged_action(state, attacker).unwrap_or(ActionKind::Swing { reach: MELEE_REACH });
        // The commonest refusal on the shard, and the one that used to be a bare
        // `continue`: a target round a corner or two tiles past a bow's reach.
        // An archer stands in this state for as long as the quarry stays there,
        // so it is the one a player is most likely to be looking at.
        if let Some(reason) = obstruction(state, attacker, target, kind.reach()) {
            balk(state, &mut balked, attacker, reason);
            continue;
        }
        // The quiver is asked here, at the nock, and not ten seconds later at
        // the loose: an archer with nothing to shoot is told before drawing
        // rather than after standing through a whole interval. The round is not
        // taken yet — it leaves the pack when the arrow does, so a draw that is
        // spoiled cannot rob its archer, and nothing has to be handed back
        // through the ends an action can have. The refusal still costs the
        // interval, or an empty quiver would repeat itself every tick.
        if let Some(round) = kind.round() {
            if !carries_round(state, attacker, round) {
                state.system_message(attacker, out_of_ammo_message(round));
                set_next_swing(state, attacker, now + swing_speed(state, attacker));
                // The message goes to the archer alone and says it once; the
                // standing state is what a watcher — the archer's own screen
                // included — reads for as long as the quiver is empty.
                balk(state, &mut balked, attacker, InterruptReason::NoAmmo);
                continue;
            }
        }
        // A concealed fighter is not telegraphed: drawing a wind-up would break
        // cover before the blow, which is the whole of an ambush.
        let telegraphed = !state.registry.has::<Hidden>(attacker);
        let impact = if telegraphed {
            if due > now {
                due
            } else {
                now.saturating_add(swing_speed(state, attacker).max(1))
            }
        } else {
            // No gesture to stretch, so nothing to stretch it over: the blow
            // lands when it was already due.
            due.max(now)
        };
        // The opener is captured at the commit — what the fighter promised is
        // frozen here — and spent by the hit roll at the impact even on a miss.
        // Cover is a way into a fight, never a permanent accuracy aura.
        let accuracy = if take_wrestling_opener(state, attacker, target_serial) {
            AMBUSH_ACCURACY_PERCENT
        } else {
            0
        };
        set_next_swing(state, attacker, impact);
        let action = CombatAction {
            target: target_serial,
            kind,
            phase: Phase::Releasing { impact },
            started_at: now,
            accuracy,
            applied: ConditionSet::EMPTY,
            telegraphed,
            stage: ActionStage::FIRST,
        };
        state.registry.insert(attacker, action);
        if telegraphed {
            // A blow is delivered by the body, so the body turns to deliver it.
            // A shot is delivered down a line, and turning the shooter at every
            // nock would take a kiting archer's step away from it: a step in a
            // direction the mobile is not facing turns instead of moving, so a
            // brain that beats no faster than the shard re-aims it spends every
            // beat spinning and never opens the gap. See `spawn_archer`, which
            // carries the same finding from the other end.
            if kind.flight().is_none() {
                state.face_toward(attacker, target);
            }
            state.break_cover(attacker);
            state.animate_timed(attacker, Action::Attack, impact.saturating_sub(now));
            state.announce_action(attacker, action);
        }
    }

    // Everyone still standing in a refusal this pass did not renew is free of
    // it: the wall was opened, the quarry stepped back into reach, the fighter
    // committed, or the fight is over. Walked over the component rather than
    // over every combatant, so the cost is the number of fighters actually held
    // up and not the number of fighters.
    let stale: Vec<EntityId> = state
        .registry
        .query::<Balked>()
        .map(|(entity, _)| entity)
        .filter(|entity| !balked.contains(entity))
        .collect();
    for entity in stale {
        state.set_balked(entity, BalkState::Clear);
    }
}

/// Record that `attacker` could not begin an action, and say so on the edge.
///
/// The set is what the end of the commit pass reads to tell a refusal that is
/// still in force from one that has been lifted; [`WorldState::set_balked`] is
/// what decides whether anything crosses the wire, so calling this every tick
/// costs a component lookup and a hash insert and sends nothing.
fn balk(state: &mut WorldState, balked: &mut HashSet<EntityId>, attacker: EntityId, reason: InterruptReason) {
    balked.insert(attacker);
    state.set_balked(attacker, BalkState::Blocked(reason));
}

/// Apply the world to every running action — the second verb, and the one the
/// old model had no room for at all.
///
/// Both phases are sustained the same way: an armed fighter is interruptible and
/// can be spoiled, which is what stops "wait for the perfect moment" from being
/// a free option. What ends an action here ends it *with a reason*, and the
/// reason crosses the wire — the silent `continue` at the impact is what made a
/// player watch a full swing and get neither a blow nor a word.
///
/// The condition rules of `docs/combat_actions.md`'s D4 — a run that sways a
/// shot, a wound that spoils it — are a later phase. What is applied today is
/// only what the world does: the fighter's own life, a bard's calm, and the
/// three ways a committed target stops being strikeable.
pub fn sustain_actions(state: &mut WorldState) {
    let now = state.ticks;
    let running: Vec<(EntityId, CombatAction)> = state
        .registry
        .query::<CombatAction>()
        .map(|(attacker, action)| (attacker, *action))
        .collect();

    for (attacker, action) in running {
        // The fighter itself first: a dead or ghosted attacker has no action,
        // whatever its target is doing.
        if !attackable(state, attacker) {
            state.end_combat_action(
                attacker,
                CombatActionOutcome::Interrupted(InterruptReason::Abandoned),
            );
            continue;
        }
        if state
            .registry
            .has::<openshard_state::components::Pacified>(attacker)
        {
            state.end_combat_action(
                attacker,
                CombatActionOutcome::Interrupted(InterruptReason::Pacified),
            );
            continue;
        }
        let Some(target) = state
            .registry
            .entity_of(action.target)
            .filter(|&target| attackable(state, target))
        else {
            state.end_combat_action(
                attacker,
                CombatActionOutcome::Interrupted(InterruptReason::TargetGone),
            );
            continue;
        };
        // Against the *committed* reach, not the weapon's reach now: a fighter
        // is held to what it promised, and a weapon swapped mid-swing does not
        // lengthen the blow already in flight.
        //
        // A cut line is the one refusal here that is a *rule* rather than a
        // verdict: `Blinded` is a row in the table, and the shipped row breaks
        // the action, which is what this did with a bare reason before there was
        // a table to route it through. A shard that lets a fighter keep swinging
        // into the dark now says so in its config. The other two are not
        // negotiable — a target on another facet or outside the committed reach
        // is not somewhere a rule can put it back.
        match obstruction(state, attacker, target, action.reach()) {
            Some(InterruptReason::NoLineOfSight) => {
                apply_condition(state, attacker, ActorCondition::Blinded);
                if !state.registry.has::<CombatAction>(attacker) {
                    continue;
                }
            }
            Some(reason) => {
                state.end_combat_action(attacker, CombatActionOutcome::Interrupted(reason));
                continue;
            }
            None => {}
        }
        // An arm that was never released gives out. Nothing arms yet, so nothing
        // reaches this today; it is the endurance that stops a couched lance
        // from becoming a permanent property of a rider.
        if let Phase::Armed { expires_at, .. } = action.phase {
            if now >= expires_at {
                state.end_combat_action(attacker, CombatActionOutcome::Expired);
                continue;
            }
        }
        advance_stage(state, attacker, now);
    }
}

/// Move a running action into the stretch its interval says it is in, and tell
/// every watcher when that changes.
///
/// The component is re-read rather than taken from the sustain loop's copy: a
/// condition rule applied earlier in the same pass may have pushed the impact
/// out (`Slow`) or ended the action outright, and a stage computed from the
/// stale interval would be announced against a schedule nobody is on any more.
///
/// **A stage never goes backwards.** A `Slow` lowers the fraction of the
/// interval that has passed, and a fighter who has drawn a bow has not un-drawn
/// it; the shot simply hangs at full draw for longer, which is what a rule that
/// slows an archer means. Only a forward move is recorded and only a forward
/// move is announced.
///
/// An untelegraphed action says nothing, for the reason it has no wind-up
/// either: a concealed fighter narrating their own draw would give away the
/// ambush the concealment is for.
fn advance_stage(state: &mut WorldState, attacker: EntityId, now: WorldTick) {
    let Some(&action) = state.registry.get::<CombatAction>(attacker) else {
        return;
    };
    let Some(impact) = action.impact() else {
        // An armed action is not running through an interval — it is waiting on
        // the world, and its stage is whatever it was armed in. Ф7's release is
        // what starts a clock for this to walk.
        return;
    };
    let span = impact.saturating_sub(action.started_at);
    let elapsed = now.saturating_sub(action.started_at);
    // A zero-length interval is already at its impact: an action that takes no
    // time is entirely its release. Not a case so much as a division guard.
    let percent = match span {
        0 => 100,
        span => u16::try_from(elapsed.saturating_mul(100) / span).unwrap_or(100),
    };
    let stage = state.gameplay.action_stages.stage_at(action.kind, percent);
    if stage <= action.stage {
        return;
    }
    let mut moved = action;
    moved.stage = stage;
    state.registry.insert(attacker, moved);
    if action.telegraphed {
        state.announce_stage(attacker, stage);
    }
}

/// Push a condition at whatever `mobile` is doing — D5's one door.
///
/// Called from the seam that already knows the fact, never from a pass that goes
/// looking for it: the step has `running` and the mount in hand, and [`damage`]
/// is the door every wound passes. A mobile with no action is the ordinary case
/// and costs one component lookup.
///
/// **A condition is charged at most once per action.** A ten-second draw takes
/// twenty steps, and a sway charged per step would put an archer's chance at
/// zero for crossing a room, while a `Slow` charged per step would push the
/// impact away faster than the wait brings it closer and the shot would never be
/// taken at all. So the rule is a fact about the action — *it ran*, *it was
/// struck* — and [`ConditionSet`] on the action remembers. The per-tick spender
/// in the model is `Drain`, levied against a held condition by the sustain pass,
/// and it is Ф5's.
pub fn apply_condition(state: &mut WorldState, mobile: EntityId, condition: ActorCondition) {
    let Some(&action) = state.registry.get::<CombatAction>(mobile) else {
        return;
    };
    if action.applied.contains(condition) {
        return;
    }
    let Some(effect) = state.gameplay.action_rules.effect(action.kind, condition) else {
        // No rule for this pair, which is a real answer and not a gap: walking
        // is *free* for an archer on the shipped shard. Nothing is charged, so
        // nothing is remembered either.
        return;
    };
    let now = state.ticks;
    let mut action = action;
    action.applied = action.applied.with(condition);
    match effect {
        ActionEffect::Break => {
            state.end_combat_action(
                mobile,
                CombatActionOutcome::Interrupted(condition.interrupt_reason()),
            );
        }
        ActionEffect::Sway { penalty } => {
            // Spent once by the hit roll at the impact, beside whatever the
            // commit put there. A penalty deep enough to take the chance below
            // zero takes it to zero — `check_hit` clamps.
            action.accuracy = action.accuracy.saturating_sub(penalty);
            state.registry.insert(mobile, action);
        }
        ActionEffect::Slow { percent } => {
            // An armed action has no clock to push yet — its release is what
            // starts one, and Ф7 is where a watch can fire. Nothing to do but
            // remember that the condition has been charged.
            let Some(impact) = action.impact() else {
                state.registry.insert(mobile, action);
                return;
            };
            let pushed = impact.saturating_add(impact.saturating_sub(now) * u64::from(percent) / 100);
            action.phase = Phase::Releasing { impact: pushed };
            state.registry.insert(mobile, action);
            // The deadline goes with the impact it was pinned to at the commit,
            // and so does the picture: a watcher was given an interval to
            // stretch a stroke over, and an impact that moved without saying so
            // is exactly the desync this model was built to stop.
            set_next_swing(state, mobile, pushed);
            if action.telegraphed && pushed != impact {
                state.animate_timed(mobile, Action::Attack, pushed.saturating_sub(now));
                state.announce_action(mobile, action);
            }
        }
    }
}

/// The movement half of D5: a step, with what kind of step it was.
///
/// Both step seams call it — the client's own walk and a decreed one — because a
/// fighter that was moved is a fighter that moved. The pace is pushed first and
/// the mount second: the step is the event, and being mounted is a thing that
/// was true of it.
pub fn stepped(state: &mut WorldState, mobile: EntityId, running: bool, mounted: bool) {
    if !state.registry.has::<CombatAction>(mobile) {
        return;
    }
    apply_condition(
        state,
        mobile,
        if running {
            ActorCondition::Running
        } else {
            ActorCondition::Walking
        },
    );
    if mounted {
        apply_condition(state, mobile, ActorCondition::Mounted);
    }
}

/// Whether a target counts as dead: its entity already gone (reaped), or still
/// present but at zero hits (killed this tick, not yet reaped). Either way a
/// combatant stops swinging at it.
fn target_is_dead(state: &WorldState, serial: Serial) -> bool {
    match state.registry.entity_of(serial) {
        None => true,
        Some(entity) => !attackable(state, entity),
    }
}

/// Whether `entity` is a living mobile that combat may select and strike.
///
/// A dead player stays in the registry as a [`Ghost`] so it can walk and be
/// resurrected.  Its serial and zero hit points alone therefore cannot be used
/// as a proxy for a valid combat target.
fn attackable(state: &WorldState, entity: EntityId) -> bool {
    !state.registry.has::<Ghost>(entity)
        && state
            .registry
            .get::<Hitpoints>(entity)
            .is_some_and(|hp| hp.current > 0)
}

/// Sphere's murder count threshold: the fifth innocent killed makes you red.
const MURDER_THRESHOLD: u16 = 5;
/// How long one murder count takes to fade — Sphere's short-term default, eight
/// hours at the tick rate. A reformed killer washes blue eventually, not never.
const MURDER_DECAY_TICKS: u64 = 8 * 3600 * TICKS_PER_SECOND;

/// Tally a killed innocent against `killer`, turn it red once the tally reaches
/// the threshold, and start the slow fade if it is not already running.
///
/// A town guard is exempt: executing the guilty is the whole of its purpose, and
/// a guard that went red after five sentences would be hunted by the next one.
/// ServUO says the same thing by clearing the guard's own `Criminal` and `Kills`
/// on every beat of its attack timer.
fn record_murder(state: &mut WorldState, killer: EntityId) {
    if state.registry.has::<Guard>(killer) {
        return;
    }
    let count = state.registry.get::<Murders>(killer).map_or(0, |m| m.0) + 1;
    state.registry.insert(killer, Murders(count));
    if !state.registry.has::<MurderDecay>(killer) {
        state.registry.insert(
            killer,
            MurderDecay {
                at_tick: state.ticks + MURDER_DECAY_TICKS,
            },
        );
    }
    if count >= MURDER_THRESHOLD && state.notoriety_of(killer) != Notoriety::Murderer {
        state.registry.insert(killer, Notoriety::Murderer);
        state.broadcast_move(killer);
    }
}

/// Age murder counts off, one per fire. Runs each tick against the tick counter,
/// like decay and criminal expiry: a mobile whose [`MurderDecay`] is due loses a
/// murder, reschedules if any remain, and — if the loss drops it below the
/// threshold — washes back from red to blue (unless a grey flag still covers it,
/// which [`expire_criminality`] will resolve).
pub fn decay_murders(state: &mut WorldState) {
    let now = state.ticks;
    let due: Vec<EntityId> = state
        .registry
        .query::<MurderDecay>()
        .filter(|(_, decay)| decay.at_tick <= now)
        .map(|(entity, _)| entity)
        .collect();
    for entity in due {
        let was_murderer = is_murderer(state, entity);
        let count = state.registry.get::<Murders>(entity).map_or(0, |m| m.0);
        let count = count.saturating_sub(1);
        if count == 0 {
            state.registry.remove::<Murders>(entity);
            state.registry.remove::<MurderDecay>(entity);
        } else {
            state.registry.insert(entity, Murders(count));
            state.registry.insert(
                entity,
                MurderDecay {
                    at_tick: now + MURDER_DECAY_TICKS,
                },
            );
        }
        // Dropped below the line: no longer a murderer. Only repaint if a grey
        // flag is not currently the colour shown — that one lifts on its own timer.
        if was_murderer && !is_murderer(state, entity) && state.notoriety_of(entity) == Notoriety::Murderer {
            state.registry.insert(entity, Notoriety::Innocent);
            state.broadcast_move(entity);
        }
    }
}

/// Whether a mobile's murder tally has passed the threshold — a murderer whether
/// or not a grey flag is currently painted over the red.
fn is_murderer(state: &WorldState, entity: EntityId) -> bool {
    state
        .registry
        .get::<Murders>(entity)
        .is_some_and(|m| m.0 >= MURDER_THRESHOLD)
}

/// Push a combatant's next swing out to `tick`.
pub fn set_next_swing(state: &mut WorldState, attacker: EntityId, tick: WorldTick) {
    if let Some(combat) = state.registry.get_mut::<Combat>(attacker) {
        combat.schedule_swing(tick);
    }
}

/// Stop a combatant attacking whatever it was.
///
/// Whatever it was swinging goes with the target it was swinging at: an action
/// is bound to the opponent it committed to and does not follow a change of aim.
pub fn clear_target(state: &mut WorldState, attacker: EntityId) {
    state.end_combat_action(
        attacker,
        CombatActionOutcome::Interrupted(InterruptReason::TargetGone),
    );
    if let Some(combat) = state.registry.get_mut::<Combat>(attacker) {
        combat.clear_target();
    }
}

/// Why `attacker` cannot presently strike `target` within `reach` — `None` when
/// it can.
///
/// One test, read at the commit and again every tick the action runs. It used to
/// be two copies of the same three lines at two different moments, and the
/// second copy is what turned a spoiled swing into a bare `continue`: adjacent
/// tiles can still be separated by a closed door or a wall, so melee follows the
/// same live-terrain sight rule as a volley and an interaction.
fn obstruction(
    state: &WorldState,
    attacker: EntityId,
    target: EntityId,
    reach: RangedRange,
) -> Option<InterruptReason> {
    let (Some(&Position(from)), Some(&Position(to))) = (
        state.registry.get::<Position>(attacker),
        state.registry.get::<Position>(target),
    ) else {
        return Some(InterruptReason::TargetGone);
    };
    let facet = state.facet_of(attacker);
    if state.facet_of(target) != facet {
        return Some(InterruptReason::TargetGone);
    }
    if !in_range(from, to, u32::from(reach.get())) {
        return Some(InterruptReason::OutOfReach);
    }
    if !openshard_movement::sight_clear(&state.footing(facet, Doors::AsTheyStand), from, to) {
        return Some(InterruptReason::NoLineOfSight);
    }
    None
}

/// Whether a melee gesture can actually belong to this target.
///
/// A distant target is an aim, not a reason to slash empty air;
/// [`commit_actions`] opens its animation window when the attacker reaches it.
fn melee_reachable(state: &WorldState, attacker: EntityId, target: EntityId) -> bool {
    obstruction(state, attacker, target, MELEE_REACH).is_none()
}

/// Turn a mobile grey for `gameplay.criminal_ticks`, or push the timer out if it is
/// already grey. Only an innocent flags; a red murderer stays red.
///
/// The colour change is broadcast with `broadcast_move` — a `0x77` carries
/// notoriety, so everyone watching sees the attacker turn grey without anyone
/// having to move.
pub fn flag_criminal(state: &mut WorldState, mobile: EntityId) {
    let noto = state.notoriety_of(mobile);
    if noto != Notoriety::Innocent && noto != Notoriety::Criminal {
        return;
    }
    let already_grey = noto == Notoriety::Criminal;
    state.registry.insert(mobile, Notoriety::Criminal);
    state.registry.insert(
        mobile,
        CriminalUntil {
            tick: state.ticks + state.gameplay.criminal_ticks,
        },
    );
    // Only the turn to grey needs redrawing; refreshing the timer changes no
    // colour.
    if !already_grey {
        state.broadcast_move(mobile);
    }
}

/// Restore anyone whose criminal flag has run out to their base standing, and
/// redraw it for everyone watching. Runs each tick against the tick counter.
///
/// Base standing, not always innocent: a murderer wears grey while its criminal
/// flag lasts, but the red underneath does not lapse, so a lapsing flag uncovers
/// it rather than washing it blue.
/// Ticks between poison pulses — about two seconds, ServUO's pulse cadence.
pub const POISON_INTERVAL: u64 = 40;
/// How many pulses a fresh poison runs before it wears off.
pub const POISON_PULSES: u8 = 8;

/// The damage one pulse of a poison of `level` deals, before poison resistance.
#[must_use]
pub const fn poison_damage(level: PoisonLevel) -> u16 {
    level.get() as u16 + 1
}

/// Poison a mobile at `level` (0 lesser .. 4 lethal), starting its pulses at
/// `now`. A stronger poison overrides a weaker one; a weaker never downgrades a
/// Deliver a poisoned weapon's dose into the mobile it just hit, and spend a charge.
///
/// ServUO's `BaseWeapon.OnHit`: a blade the Poisoning skill has coated carries
/// `18 - level*2` doses and gives one up per landed blow, poisoning what it cuts.
/// The last dose takes the coating with it — the component is removed rather than
/// left at zero, so "is this blade poisoned" stays one question with one answer.
///
/// Nothing here decides *whether* the blow landed: it is called from the one place
/// that knows, after the damage has gone through the one damage door.
fn deliver_weapon_poison(state: &mut WorldState, attacker: EntityId, target: Serial, now: WorldTick) {
    let Some(serial) = state.registry.serial_of(attacker) else {
        return;
    };
    // The item on a weapon layer, whatever it is — the poison is on the *item*, so
    // this does not go through the weapon table.
    let Some(weapon) = openshard_state::equipped_items(state, serial)
        .find(|(_, worn)| worn.layer == LAYER_ONE_HANDED || worn.layer == LAYER_TWO_HANDED)
        .map(|(entity, _)| entity)
    else {
        return;
    };
    let Some(&PoisonCharges { level, charges }) = state.registry.get::<PoisonCharges>(weapon) else {
        return;
    };
    apply_poison(state, target, level, now);
    match charges.saturating_sub(1) {
        0 => {
            state.registry.remove::<PoisonCharges>(weapon);
        }
        left => {
            state
                .registry
                .insert(weapon, PoisonCharges { level, charges: left });
        }
    }
}

/// Roll and deliver an equipped weapon's `HitPoison` affixes after a landed blow.
///
/// Unlike a temporary Poisoning-skill coating, this is a permanent property of
/// the weapon and never spends a charge. The affix list is copied before the
/// world's generator is advanced, so the registry is not borrowed across the
/// mutable roll or poison application.
fn deliver_affix_poison(state: &mut WorldState, attacker: EntityId, target: Serial, now: WorldTick) {
    let Some(weapon) = weapons::equipped_weapon_item(state, attacker) else {
        return;
    };
    let effects: Vec<(u8, u16)> = state
        .registry
        .get::<ItemAffixes>(weapon)
        .map(|affixes| {
            affixes
                .0
                .iter()
                .filter_map(|affix| match *affix {
                    ItemAffix::HitPoison {
                        level,
                        chance_per_mille,
                    } => Some((level, chance_per_mille)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    for (level, chance) in effects {
        let lands = chance >= 1_000 || (chance > 0 && state.rng.below(1_000) < u32::from(chance));
        if lands {
            apply_poison(state, target, PoisonLevel::new(level), now);
        }
    }
}

/// stronger one already working — ServUO's rule.
pub fn apply_poison(state: &mut WorldState, serial: Serial, level: PoisonLevel, now: WorldTick) {
    let Some(entity) = state.registry.entity_of(serial) else {
        return;
    };
    if state
        .registry
        .get::<Poisoned>(entity)
        .is_some_and(|existing| existing.level > level)
    {
        return;
    }
    state.registry.insert(
        entity,
        Poisoned {
            level,
            next_pulse: now + POISON_INTERVAL,
            pulses_left: POISON_PULSES,
        },
    );
}

/// Cure a mobile's poison — returns whether it had any to cure.
pub fn cure_poison(state: &mut WorldState, serial: Serial) -> bool {
    let Some(entity) = state.registry.entity_of(serial) else {
        return false;
    };
    state.registry.remove::<Poisoned>(entity).is_some()
}

/// Each tick, land a pulse on every poisoned mobile whose pulse is due, and
/// clear a poison that has run its course. The damage passes through [`damage`],
/// so poison resistance cuts it and a lethal dose kills like any other — and a
/// poisoned caster's pulse disturbs its own spell, through the same
/// `MobileDamaged`.
pub fn poison_tick(state: &mut WorldState) {
    let now = state.ticks;
    let due: Vec<(EntityId, PoisonLevel, u8)> = state
        .registry
        .query::<Poisoned>()
        .filter(|(_, poison)| now >= poison.next_pulse)
        .map(|(entity, poison)| (entity, poison.level, poison.pulses_left))
        .collect();
    for (entity, level, pulses_left) in due {
        let Some(serial) = state.registry.serial_of(entity) else {
            continue;
        };
        damage(state, serial, poison_damage(level), DamageType::Poison, None);
        // The blow may have killed and despawned a creature; only touch the
        // poison if the mobile is still here.
        if state.registry.get::<Hitpoints>(entity).is_none() {
            continue;
        }
        if pulses_left <= 1 {
            state.registry.remove::<Poisoned>(entity);
        } else {
            state.registry.insert(
                entity,
                Poisoned {
                    level,
                    next_pulse: now + POISON_INTERVAL,
                    pulses_left: pulses_left - 1,
                },
            );
        }
    }
}

pub fn expire_criminality(state: &mut WorldState) {
    let now = state.ticks;
    let expired: Vec<EntityId> = state
        .registry
        .query::<CriminalUntil>()
        .filter(|(_, flag)| flag.tick <= now)
        .map(|(entity, _)| entity)
        .collect();
    for entity in expired {
        state.registry.remove::<CriminalUntil>(entity);
        let base = if is_murderer(state, entity) {
            Notoriety::Murderer
        } else {
            Notoriety::Innocent
        };
        state.registry.insert(entity, base);
        state.broadcast_move(entity);
    }
}

/// How many ticks `mobile` waits between swings.
///
/// An explicit [`SwingSpeed`] wins — a script pinning an exact cadence, a special
/// creature. Otherwise the pace is derived from the mobile's dexterity through
/// [`swing_ticks`], wrestling speed for now (no weapon properties yet). A mobile
/// with neither swings at the default-dexterity wrestling pace.
#[must_use]
pub fn swing_speed(state: &WorldState, mobile: EntityId) -> u64 {
    if let Some(s) = state.registry.get::<SwingSpeed>(mobile) {
        return s.ticks;
    }
    let dex = state
        .registry
        .get::<Stats>(mobile)
        .map_or(DEFAULT_DEXTERITY, |s| s.dexterity);
    // A wielded weapon lends its speed base (which value depends on the era); bare
    // hands (or an off-table item) keep wrestling. Read fresh here — no cache to
    // invalidate when the weapon swaps.
    let era = state.gameplay.combat_era;
    let base = weapons::equipped_weapon(state, mobile).map_or(WRESTLING_SPEED, |weapon| {
        u64::from(openshard_state::weapon::swing_base(&weapon, era))
    });
    swing_ticks(
        dex,
        base,
        state.gameplay.combat_era.value(),
        state.gameplay.speed_scale_factor,
    )
}

/// Record one successful step for an unarmed fighter's next first contact.
///
/// Called by both player and server-directed movement after the position has
/// changed. A turn, failed step, teleport, or mounted movement intentionally
/// does not count: this is footwork, not a generic movement-speed bonus.
pub fn record_wrestling_step(state: &mut WorldState, mobile: EntityId) {
    if !is_wrestling(state, mobile) {
        state.registry.remove::<WrestlingStride>(mobile);
        return;
    }
    let now = state.ticks;
    let steps = state
        .registry
        .get::<WrestlingStride>(mobile)
        .filter(|stride| now <= stride.expires_at)
        .map_or(1, |stride| stride.steps.saturating_add(1));
    state.registry.insert(
        mobile,
        WrestlingStride {
            steps,
            expires_at: now + WRESTLING_STRIDE_TICKS,
        },
    );
}

/// Bare hands are the wrestling setup. This deliberately follows the combat
/// weapon resolver, so an item the shard does not recognise as a weapon keeps
/// its existing wrestling behaviour instead of becoming a loophole.
fn is_wrestling(state: &WorldState, mobile: EntityId) -> bool {
    weapons::equipped_weapon(state, mobile).is_none()
}

/// Spend a concealed wrestler's opener if this is the target it was armed for.
fn take_wrestling_opener(state: &mut WorldState, attacker: EntityId, target: Serial) -> bool {
    let now = state.ticks;
    let Some(opener) = state.registry.remove::<WrestlingOpener>(attacker) else {
        return false;
    };
    is_wrestling(state, attacker) && opener.target == target && now <= opener.expires_at
}

/// Continue an unarmed three-hit chain and report whether this was its payoff.
fn wrestling_combo_lands(state: &mut WorldState, attacker: EntityId, target: Serial) -> bool {
    let now = state.ticks;
    let hits = state
        .registry
        .get::<WrestlingCombo>(attacker)
        .filter(|combo| combo.target == target && now <= combo.expires_at)
        .map_or(1, |combo| combo.hits.saturating_add(1));
    if hits >= 3 {
        state.registry.remove::<WrestlingCombo>(attacker);
        true
    } else {
        state.registry.insert(
            attacker,
            WrestlingCombo {
                target,
                hits,
                expires_at: now + WRESTLING_COMBO_TICKS,
            },
        );
        false
    }
}

/// The combo's small stamina refund, capped at the mobile's own pool.
fn restore_wrestling_stamina(state: &mut WorldState, attacker: EntityId) {
    let Some(&Stamina { current, max }) = state.registry.get::<Stamina>(attacker) else {
        return;
    };
    state.registry.insert(
        attacker,
        Stamina {
            current: current.saturating_add(5).min(max),
            max,
        },
    );
}

/// The base damage a blow from `attacker` carries, before armour. Precedence:
/// an explicit [`MeleeDamage`] (a creature's natural blow, a script's pin) wins;
/// else a wielded weapon rolls its era's min..=max; else the bare-hands default.
/// The roll uses the world's seeded [`rng`](WorldState::rng), never a wall clock,
/// so a fight replays. The target's resistance is applied later, in [`damage`].
#[must_use]
pub fn melee_blow(state: &mut WorldState, attacker: EntityId) -> u16 {
    if let Some(damage) = state.registry.get::<MeleeDamage>(attacker) {
        return damage.amount;
    }
    if let Some(weapon) = weapons::equipped_weapon(state, attacker) {
        let era = state.gameplay.combat_era;
        let mut min = openshard_state::weapon::by_era(weapon.old_min, weapon.aos_min, era);
        let mut max = openshard_state::weapon::by_era(weapon.old_max, weapon.aos_max, era);
        if let Some(item) = weapons::equipped_weapon_item(state, attacker) {
            if let Some(affixes) = state.registry.get::<ItemAffixes>(item) {
                for affix in &affixes.0 {
                    if let ItemAffix::DamageBonus { minimum, maximum } = *affix {
                        min = offset_damage(min, minimum);
                        max = offset_damage(max, maximum);
                    }
                }
            }
        }
        if min > max {
            std::mem::swap(&mut min, &mut max);
        }
        let span = u32::from(max.saturating_sub(min)) + 1;
        return min + state.rng.below(span) as u16;
    }
    SWING_DAMAGE
}

/// Apply one signed item-property offset without widening ordinary weapon math.
fn offset_damage(value: u16, offset: i16) -> u16 {
    if offset >= 0 {
        value.saturating_add(offset as u16)
    } else {
        value.saturating_sub(offset.unsigned_abs())
    }
}

/// A mobile's value in a skill, in tenths (0 for untrained or no sheet).
fn skill_value(state: &WorldState, mobile: EntityId, skill: Skill) -> u16 {
    state
        .registry
        .get::<Skills>(mobile)
        .map_or(0, |skills| skills.get(skill))
}

/// Whether `attacker`'s swing at `defender` lands — and, as it rolls, trains the
/// attacker's weapon skill (ServUO's hit roll *is* a `CheckSkill`). Pre-AoS
/// `CheckHit`: `chance = (atk + 50) / ((def + 50) · 2)`, `atk`/`def` the two
/// mobiles' weapon-skill standings, the defender's own weapon skill (Wrestling
/// unarmed) its guard.
///
/// Gated on the attacker carrying a `Skills` sheet: a creature or an untrained
/// mobile has none and keeps the pre-feature certainty — its natural blow always
/// lands and trains nothing. The moment a mobile has skills (a trained player, a
/// creature the pack equips with them) its swings roll and gain.
///
/// `accuracy` is what the action accumulated on its way to the impact, as a
/// signed percentage of the base chance: an ambush from cover adds to it, and a
/// condition rule that sways a shot will subtract. A penalty that would take the
/// chance below zero takes it to zero rather than wrapping.
fn check_hit(state: &mut WorldState, attacker: EntityId, defender: EntityId, accuracy: i16) -> bool {
    if !state.registry.has::<Skills>(attacker) {
        return true;
    }
    let attack_skill = weapons::combat_skill_id(state, attacker);
    let attack = skill_value(state, attacker, attack_skill);
    let defend_skill = weapons::combat_skill_id(state, defender);
    let defend = skill_value(state, defender, defend_skill);
    // Values are tenths, so ServUO's `(v/10 + 50)` is `(v + 500)/10`; the tenths
    // cancel, leaving `chance = (atk + 500) / (2·(def + 500))`, per-mille below and
    // clamped to certainty (pre-AoS lets a wide skill gap always land).
    let base_chance = 1000 * (u32::from(attack) + 500) / (2 * (u32::from(defend) + 500));
    let scale = (100 + i32::from(accuracy)).max(0) as u32;
    let chance = (base_chance * scale / 100).min(1000);
    openshard_skills::roll_skill_chance(state, attacker, attack_skill, chance)
}

/// The sound a whiffed swing makes: the wielded weapon's own miss sound (ServUO's
/// `DefMissSound`), or the generic swish for bare hands / an off-table item.
fn miss_sound(state: &WorldState, attacker: EntityId) -> u16 {
    weapons::equipped_weapon(state, attacker)
        .map(|weapon| weapon.miss_sound)
        .filter(|&sound| sound != 0)
        .unwrap_or(MELEE_MISS_SOUND)
}

/// ServUO's AoS `GetBonus`: `value·scalar` per point, plus `offset` once the skill
/// reaches `threshold`, as a fraction (the `/100`).
fn get_bonus(value: f64, scalar: f64, threshold: f64, offset: f64) -> f64 {
    let mut bonus = value * scalar;
    if value >= threshold {
        bonus += offset;
    }
    bonus / 100.0
}

/// The blow after the attacker's skills scale it — Tactics, Strength and Anatomy,
/// ServUO's `ScaleDamage`. Gated on a `Skills` sheet, the same boundary as
/// [`check_hit`]: a creature or untrained mobile deals its raw weapon/natural blow
/// as before; a trained fighter scales it. Era 1 uses the pre-AoS coefficients
/// (Tactics its own ±50% about parity, then Str and Anatomy summed), era 2 the AoS
/// bonuses. At least 1, so a heavily-nerfed blow still stings.
struct Blow {
    amount: u16,
    critical: bool,
}

/// The fully mitigated weapon blow, plus whether the shard-specific critical
/// rule amplified it.  The caller owns feedback because only a player client
/// can receive a journal message.
fn scaled_blow(state: &mut WorldState, attacker: EntityId, defender: EntityId) -> Blow {
    let base = f64::from(melee_blow(state, attacker));
    let era = state.gameplay.combat_era;
    // Skill scaling — a trained fighter only; a creature/untrained mobile deals raw.
    let scaled = if state.registry.has::<Skills>(attacker) {
        let tactics = f64::from(skill_value(state, attacker, weapons::TACTICS_SKILL)) / 10.0;
        let anatomy = f64::from(skill_value(state, attacker, weapons::ANATOMY_SKILL)) / 10.0;
        let strength = f64::from(state.registry.get::<Stats>(attacker).map_or(0, |s| s.strength));
        // Lumberjacking lends an axe a bonus, nothing else.
        let is_axe = weapons::equipped_weapon(state, attacker).is_some_and(|weapon| weapon.is_axe);
        let lumber = if is_axe {
            f64::from(skill_value(state, attacker, weapons::LUMBERJACKING_SKILL)) / 10.0
        } else {
            0.0
        };
        if era.value() >= 2 {
            // The AoS family (AoS, SE, ML) shares the AoS damage-bonus formula.
            let bonus = get_bonus(strength, 0.30, 100.0, 5.0)
                + get_bonus(anatomy, 0.50, 100.0, 5.0)
                + get_bonus(tactics, 0.625, 100.0, 6.25)
                + get_bonus(lumber, 0.20, 100.0, 10.0);
            base + base * bonus
        } else {
            // Tactics is its own multiplier about the 50-point parity, then Strength
            // (1%/5), Anatomy (1%/5, +10% at GM) and axe Lumberjacking (1%/5, capped
            // 20%) sum into a second.
            let mut damage = base + base * ((tactics - 50.0) / 100.0);
            let mut modifiers = (strength / 5.0) / 100.0 + (anatomy / 5.0) / 100.0;
            if anatomy >= 100.0 {
                modifiers += 0.1;
            }
            modifiers += ((lumber / 5.0) / 100.0).min(0.2);
            damage += damage * modifiers;
            damage
        }
    } else {
        base
    };
    // Criticals are a shard-specific rule, deliberately applied before every
    // defence below: armour, resistances and the pre-AoS PvP split still matter.
    // The roll is made only after a hit has landed, and consumes the world's
    // seeded RNG, so combat remains replayable.
    let critical = state.rng.below(1000) < u32::from(state.gameplay.critical_chance);
    let scaled = if critical {
        scaled * f64::from(state.gameplay.critical_damage_percent) / 100.0
    } else {
        scaled
    };
    // ServUO's pre-AoS `ComputeDamage`: outside AoS, full damage lands only when a
    // player strikes a non-player — every other pairing (a monster's blow, PvP) is
    // halved. "Player" is a mobile with a client. Applies to every blow, skilled or
    // not, so it sits past the skill gate.
    let is_player = |entity| state.registry.has::<Client>(entity);
    let final_damage = if era.value() < 2 && (is_player(defender) || !is_player(attacker)) {
        scaled / 2.0
    } else {
        scaled
    };
    // Slayer is a property of the weapon instance, checked against the target's
    // body before armour gets to absorb the blow. It never lives on the mobile,
    // so removing the weapon cannot leave a stale combat bonus behind.
    let slayer_bonus = slayer_bonus_percent(state, attacker, defender);
    let final_damage = final_damage * (100.0 + f64::from(slayer_bonus)) / 100.0;
    // Truncate like ServUO's `(int)`, floored at 1.
    let blow = final_damage.max(1.0) as u16;
    // Then the defender's worn armour takes its bite — ServUO's
    // `BaseWeapon.AbsorbDamage`, which is a *weapon* rule, not a `Mobile.Damage`
    // one: a sword is stopped by a breastplate where a fireball is not. Pre-AoS
    // only; from AoS armour speaks through resistances instead, which `damage`
    // already applies. A blow that armour swallows whole still lands for 1
    // (`if (!Core.AOS && damage < 1) damage = 1`).
    if era.value() < 2 {
        Blow {
            amount: armor::absorb_physical(state, defender, blow).max(1),
            critical,
        }
    } else {
        Blow {
            amount: blow,
            critical,
        }
    }
}

/// The sum of an equipped weapon's slayer bonuses matching `defender`'s body.
fn slayer_bonus_percent(state: &WorldState, attacker: EntityId, defender: EntityId) -> u16 {
    let Some(body) = state.registry.get::<Body>(defender).map(|body| body.id.0) else {
        return 0;
    };
    let Some(weapon) = weapons::equipped_weapon_item(state, attacker) else {
        return 0;
    };
    state
        .registry
        .get::<ItemAffixes>(weapon)
        .map(|affixes| {
            affixes
                .0
                .iter()
                .filter_map(|affix| match *affix {
                    ItemAffix::Slayer {
                        body: target,
                        bonus_percent,
                    } if target == body => Some(u16::from(bonus_percent)),
                    _ => None,
                })
                .sum()
        })
        .unwrap_or(0)
}
