//! Combat: damage, death, war mode, the swing timer, and criminal flagging.
//!
//! A gameplay system in its own crate, operating on the shared [`WorldState`].
//! Damage passes through one door — [`damage`] applies the target's resistance
//! for the kind of damage, whether the blow came from a sword, a spell, or a
//! script — and emits [`MobileDamaged`], then [`MobileDied`] at zero. What death
//! *does* (loot, notoriety, a corpse) is a reader's to decide off that event;
//! combat says what happened and moves on.
//!
//! [`swings`] is the interactive half, run each tick against the tick counter so
//! it reads no clock: a combatant in war mode with a target in reach strikes on
//! its timer. AI drives the same machinery — a brain that hands a creature a
//! `Combat` is fought by `swings` exactly as a player is.

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_map::overlay::Doors;
use openshard_protocol::combat::{AttackTarget, WarMode};
use openshard_protocol::feedback::{EffectKind, GraphicalEffect};
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::serial::Serial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::wire::{Graphic, SoundId};
use openshard_protocol::world::{Point, PoisonLevel};
use openshard_state::components::{
    BehaviourBuffs, Body, Client, Combat, CriminalUntil, DamageType, Equipped, Frozen, Ghost, Guard, Hidden,
    Hitpoints, MeleeDamage, MurderDecay, Murders, PoisonCharges, Poisoned, Position, RangedAttack,
    Resistance, Skills, Stamina, Stats, SwingSpeed, WrestlingAmbushCooldown, WrestlingCombo,
    WrestlingInterceptCooldown, WrestlingOpener, WrestlingStride, body_is_female, body_opens_doors,
    creature_base_sound,
};
use openshard_state::sectors::in_range;
use openshard_state::weapon::{LAYER_ONE_HANDED, LAYER_TWO_HANDED};
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
/// At the 50ms tick a tenth of a second is two ticks, so the result is doubled.
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
    tenths * 2
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
    if let Some(combat) = state.registry.get_mut::<Combat>(player) {
        combat.warmode = war;
    }
    state.send_packet(connection, &ServerPacket::WarMode(WarMode { war }));
}

/// Set a player's attack target. The blow itself is not struck here — this only
/// aims; [`swings`] turns "in war mode, in reach, timer up" into damage.
pub fn attack(state: &mut WorldState, connection: ConnectionId, target: Option<Serial>) {
    let Some(&player) = state.players.get(&connection) else {
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
        .and_then(|combat| combat.target);
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
    if let Some(combat) = state.registry.get_mut::<Combat>(player) {
        combat.target = Some(serial);
        combat.next_swing = next;
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
            .is_some_and(|combat| combat.warmode && combat.target.is_none())
            && state
                .registry
                .get::<Hitpoints>(victim)
                .is_some_and(|hits| hits.current > 0);
        if !ready_to_retaliate {
            continue;
        }
        let next_swing = state.ticks + swing_speed(state, victim);
        if let Some(combat) = state.registry.get_mut::<Combat>(victim) {
            combat.target = Some(attacker);
            combat.next_swing = next_swing;
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

/// Strike, for every mobile whose swing is due.
///
/// The interactive half of combat, run each tick against the tick counter so it
/// reads no clock. A swing lands when the attacker is in war mode, has a target
/// within [`MELEE_RANGE`] on the same facet, and its timer is up; out of reach it
/// simply waits, its timer unspent, so the blow falls the instant the gap closes.
/// Loose every ranged attack whose timer is up: a warlike combatant with a
/// [`RangedAttack`], a target inside its reach but beyond arm's length, and a
/// clear line to it fires — through [`damage`], the one door all damage passes,
/// so resistance and murder attribution already apply. Sharing the swing timer
/// with melee means a creature closes to bite or stands to shoot, never both
/// in one beat.
pub fn volleys(state: &mut WorldState) {
    let now = state.ticks;
    let ready: Vec<(EntityId, Serial, u8, DamageType)> = state
        .registry
        .query::<Combat>()
        .filter_map(|(attacker, combat)| {
            if !combat.warmode || now < combat.next_swing {
                return None;
            }
            let ranged = state.registry.get::<RangedAttack>(attacker)?;
            combat
                .target
                .map(|target| (attacker, target, ranged.range.get(), ranged.kind))
        })
        .collect();
    for (attacker, target_serial, range, kind) in ready {
        let Some(target) = state.registry.entity_of(target_serial) else {
            clear_target(state, attacker);
            continue;
        };
        if !attackable(state, target) {
            clear_target(state, attacker);
            continue;
        };
        let (Some(&Position(from)), Some(&Position(to))) = (
            state.registry.get::<Position>(attacker),
            state.registry.get::<Position>(target),
        ) else {
            continue;
        };
        let facet = state.facet_of(attacker);
        if state.facet_of(target) != facet
            || in_range(from, to, MELEE_RANGE)
            || !in_range(from, to, u32::from(range))
        {
            continue; // melee's beat, or out of reach — the brain closes in
        }
        if !openshard_movement::sight_clear(&state.footing(facet, Doors::AsTheyStand), from, to) {
            continue; // no shooting through walls
        }
        let by = state.registry.serial_of(attacker);
        let pace = swing_speed(state, attacker);
        if let Some(combat) = state.registry.get_mut::<Combat>(attacker) {
            combat.next_swing = now + pace;
        }
        // The bolt's flight, then the thwack — emitted before the blow lands, so
        // the mark is still drawn for the arrow to fly at. A moving effect from
        // shooter to target, then the hit sound, both to everyone who can see it.
        let arrow = GraphicalEffect {
            kind: EffectKind::Moving,
            from: by,
            to: Some(target_serial),
            art: ARROW_GRAPHIC,
            from_point: Point::new(from.x, from.y, from.z),
            to_point: Point::new(to.x, to.y, to.z),
            speed: RANGED_EFFECT_SPEED,
            duration: 1,
            fixed_direction: false,
            explode: false,
        };
        state.animate(attacker, Action::Attack);
        state.broadcast_packet(attacker, &ServerPacket::Effect(arrow));
        let sound = attack_sound(state, attacker, RANGED_HIT_SOUND);
        state.play_sound(attacker, sound);
        // The bolt still flew and twanged; on a miss it simply finds no mark. The
        // hit roll trains the shooter's Archery the same as a melee swing trains
        // its weapon. Damage precedence matches melee via `scaled_blow`.
        if check_hit(state, attacker, target, 0) {
            let blow = scaled_blow(state, attacker, target);
            if let Some(hit) = state.registry.serial_of(target) {
                damage(state, hit, blow.amount, kind, by);
                if blow.critical {
                    state.system_message(attacker, "Critical hit!");
                }
            }
        }
    }
}

pub fn swings(state: &mut WorldState) {
    let now = state.ticks;
    // Collected first: `damage` mutates the registry, so the query cannot be held
    // across it.
    let ready: Vec<(EntityId, Serial)> = state
        .registry
        .query::<Combat>()
        .filter_map(|(attacker, combat)| {
            (combat.warmode && now >= combat.next_swing)
                .then(|| combat.target.map(|target| (attacker, target)))
                .flatten()
        })
        .collect();

    for (attacker, target_serial) in ready {
        let Some(target) = state.registry.entity_of(target_serial) else {
            // The target is gone — a creature killed, a player logged out.
            clear_target(state, attacker);
            continue;
        };
        // A target may have died since combat selected it.  In particular, a
        // player remains a mobile after death as a ghost, so just resolving its
        // serial is not enough: without this guard monsters keep animating
        // attacks at the ghost until their next AI beat clears the target.
        if !attackable(state, target) {
            clear_target(state, attacker);
            continue;
        }
        let (Some(&Position(attacker_pos)), Some(&Position(target_pos))) = (
            state.registry.get::<Position>(attacker),
            state.registry.get::<Position>(target),
        ) else {
            continue;
        };
        let facet = state.facet_of(attacker);
        if state.facet_of(target) != facet
            || !in_range(attacker_pos, target_pos, MELEE_RANGE)
            // Adjacent tiles can still be separated by a closed door or wall.
            // Melee follows the same live-terrain sight rule as a volley and an
            // interaction: range alone must not allow a blow through an obstacle.
            || !openshard_movement::sight_clear(
                &state.footing(facet, Doors::AsTheyStand),
                attacker_pos,
                target_pos,
            )
        {
            continue;
        }
        // Use WorldState's common turn path instead of only broadcasting a
        // `0x77`: the owner of this mobile ignores that packet, and needs the
        // accompanying `0x20` to show a combat turn after turning manually.
        state.face_point(attacker, target_pos);
        // The attacker's serial rides along so a lethal blow can be blamed —
        // `damage` is the one place murder is tallied, melee or spell alike.
        let by = state.registry.serial_of(attacker);
        // A mobile a bard has calmed does not swing — ServUO's `BardPacified`,
        // checked where the blow would land rather than folded into the target.
        if state
            .registry
            .has::<openshard_state::components::Pacified>(attacker)
        {
            continue;
        }
        // The opener is captured before revealing the attacker and spent on this
        // attempt even on a miss.  Cover is a way into a fight, never a permanent
        // accuracy aura.
        let ambush = take_wrestling_opener(state, attacker, target_serial);
        // Swinging at somebody is the loudest thing you can do — ServUO calls
        // `RevealingAction` in the combat timer, before the blow is even rolled.
        state.break_cover(attacker);
        // The swing animates whether it lands or not — a miss still gestures.
        state.animate(attacker, Action::Attack);
        // Roll to hit (and train the weapon skill by trying). A miss whistles past
        // and does no damage; the timer resets either way.
        if !check_hit(state, attacker, target, if ambush { 25 } else { 0 }) {
            state.registry.remove::<WrestlingCombo>(attacker);
            state.play_sound(attacker, SoundId(miss_sound(state, attacker)));
            set_next_swing(state, attacker, now + swing_speed(state, attacker));
            continue;
        }
        let mut blow = scaled_blow(state, attacker, target);
        if is_wrestling(state, attacker) {
            if wrestling_combo_lands(state, attacker, target_serial) {
                blow.amount =
                    (u32::from(blow.amount) * (100 + u32::from(WRESTLING_COMBO_DAMAGE_PERCENT)) / 100) as u16;
                restore_wrestling_stamina(state, attacker);
                state.system_message(attacker, "Combo strike!");
            }
        } else {
            // A weapon hit interrupts a bare-handed sequence even if the fighter
            // puts it away before the combo window expires.
            state.registry.remove::<WrestlingCombo>(attacker);
        }
        // The blow lands with the attacker's own thwack — a creature's growl, a
        // human's fist — from the attacker, who is still here even when the blow
        // just killed the target.
        let sound = attack_sound(state, attacker, MELEE_HIT_SOUND);
        damage(state, target_serial, blow.amount, DamageType::Physical, by);
        if blow.critical {
            state.system_message(attacker, "Critical hit!");
        }
        state.play_sound(attacker, sound);
        // A coated blade spends a dose into whatever it just cut.
        deliver_weapon_poison(state, attacker, target_serial, now);
        set_next_swing(state, attacker, now + swing_speed(state, attacker));
        // The blow may have killed it; a dead target is no target. Dead means gone
        // *or* standing at zero hits — a creature killed this tick is not swept off
        // the map until the tick's `reap`, so the entity still resolves for a beat.
        if target_is_dead(state, target_serial) {
            clear_target(state, attacker);
        }
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
const MURDER_DECAY_TICKS: u64 = 8 * 3600 * 20;

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
        combat.next_swing = tick;
    }
}

/// Stop a combatant attacking whatever it was.
pub fn clear_target(state: &mut WorldState, attacker: EntityId) {
    if let Some(combat) = state.registry.get_mut::<Combat>(attacker) {
        combat.target = None;
    }
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
    let Some(weapon) = state
        .registry
        .query::<Equipped>()
        .find(|(_, worn)| {
            worn.mobile == serial && (worn.layer == LAYER_ONE_HANDED || worn.layer == LAYER_TWO_HANDED)
        })
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
        let min = openshard_state::weapon::by_era(weapon.old_min, weapon.aos_min, era);
        let max = openshard_state::weapon::by_era(weapon.old_max, weapon.aos_max, era);
        let span = u32::from(max.saturating_sub(min)) + 1;
        return min + state.rng.below(span) as u16;
    }
    SWING_DAMAGE
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
fn check_hit(
    state: &mut WorldState,
    attacker: EntityId,
    defender: EntityId,
    accuracy_bonus_percent: u16,
) -> bool {
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
    let chance = (base_chance * (100 + u32::from(accuracy_bonus_percent)) / 100).min(1000);
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
