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

use openshard_entities::{EntityId, Serial};
use openshard_gateway::ConnectionId;
use openshard_movement::Terrain;
use openshard_protocol::{
    encode_attack, encode_graphical_effect, encode_war_mode, EffectKind, EffectPoint, Notoriety,
};
use openshard_state::components::{
    body_is_female, body_opens_doors, creature_base_sound, effect, BehaviourBuffs, Body, Client,
    Combat, CriminalUntil, DamageType, Frozen, Ghost, Hitpoints, MeleeDamage, MurderDecay, Murders,
    Poisoned, Position, RangedAttack, Resistance, Skills, Stamina, Stats, Steps, SwingSpeed,
};
use openshard_state::sectors::in_range;
use openshard_state::{Action, WorldState};

pub mod armor;
pub mod weapons;

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
/// The human unarmed thwack — ServUO's `Fists.HitSound`, the fallback for a body
/// with no creature sound of its own (a player, a townsperson). A creature makes
/// its own attack sound instead; see [`attack_sound`].
pub const MELEE_HIT_SOUND: u16 = 0x0137;
/// The whistle of a blow that finds only air — a swing that missed. Coarse (one
/// swish for every weapon, not ServUO's per-weapon `DefMissSound`), but a miss is
/// no longer silent, so the client reads the whiff.
pub const MELEE_MISS_SOUND: u16 = 0x0238;
/// The twang of a bow — ServUO's `BaseRanged.DefHitSound`, the fallback for a
/// humanoid archer; a creature that shoots uses its own sound.
pub const RANGED_HIT_SOUND: u16 = 0x0234;

/// How many stones over its carry cap a mobile may be before it starts to tire —
/// ServUO's `WeightOverloading.OverloadAllowance`. Four stones of slack, so a
/// character who picks up one thing too many is warned by fatigue rather than
/// pinned to the spot.
pub const OVERLOAD_ALLOWANCE: u16 = 4;
/// Steps on foot between the baseline stamina point a walk costs — ServUO's
/// non-SA `StepsTaken % 16`. Against `STAMINA_REGEN_TICKS` this very nearly
/// balances at a run, which is why classic running feels endless without being
/// free.
pub const STEPS_PER_STAMINA: u32 = 16;
/// The same mounted: a horse does the walking, so three times as far per point.
pub const MOUNTED_STEPS_PER_STAMINA: u32 = 48;
/// Below this percentage of its stamina pool, every step costs an extra point —
/// ServUO's "under 10%" clause, the wall a fleeing player hits.
const WINDED_PERCENT: u16 = 10;

/// Spend what one step costs in stamina, and say whether the mobile may take it.
///
/// ServUO's `WeightOverloading.EventSink_Movement`, in its order: being over the
/// cap costs `5 + over/25` (a third of that mounted, double at a run), a nearly
/// empty pool costs an extra point, and every sixteenth step on foot costs one
/// anyway. `Some(message)` refuses the step and says why — the one thing that
/// stops a mule: a mobile at zero stamina cannot move at all.
///
/// `over_weight` is how many stones past its cap (allowance included) the mobile
/// is carrying; the caller weighs the pack, since what an item weighs is the
/// item crate's business, not combat's. A mobile with no [`Stamina`] pool — an
/// NPC, a bare test mobile — never tires.
pub fn spend_step_stamina(
    state: &mut WorldState,
    mobile: EntityId,
    running: bool,
    mounted: bool,
    over_weight: u16,
) -> Option<&'static str> {
    let &Stamina { current, max } = state.registry.get::<Stamina>(mobile)?;
    let mut left = current;
    let spend = |cost: u16, left: &mut u16| *left = left.saturating_sub(cost);

    if over_weight > 0 {
        let mut loss = 5 + over_weight / 25;
        if mounted {
            loss /= 3;
        }
        if running {
            loss *= 2;
        }
        spend(loss, &mut left);
        if left == 0 {
            store_stamina(state, mobile, left, max);
            return Some("You are too fatigued to move, because you are carrying too much weight!");
        }
    }
    if max > 0 && (u32::from(left) * 100 / u32::from(max)) < u32::from(WINDED_PERCENT) {
        spend(1, &mut left);
    }
    if left == 0 {
        store_stamina(state, mobile, left, max);
        return Some(if mounted {
            "Your mount is too fatigued to move."
        } else {
            "You are too fatigued to move."
        });
    }
    // The baseline: walking costs something, eventually. Counted in steps rather
    // than ticks so a slow walker and a sprinter pay the same per tile.
    let every = if mounted {
        MOUNTED_STEPS_PER_STAMINA
    } else {
        STEPS_PER_STAMINA
    };
    let steps = state.registry.get::<Steps>(mobile).map_or(0, |s| s.0) + 1;
    state.registry.insert(mobile, Steps(steps));
    if steps.is_multiple_of(every) {
        spend(1, &mut left);
    }
    store_stamina(state, mobile, left, max);
    None
}

/// Write a stamina pool back, if it moved.
fn store_stamina(state: &mut WorldState, mobile: EntityId, current: u16, max: u16) {
    state.registry.insert(mobile, Stamina { current, max });
}

/// How many ticks between the hit-point trickle — a point back every 11 seconds,
/// ServUO's pre-AoS `Mobile.DefaultHitsRate`. Slow on purpose: it is what makes a
/// bandage, a heal spell and an inn worth anything, and it is the number both
/// references shipped for the classic era. A tick count, like decay, so a
/// wounded mobile heals identically in a replay.
pub const HITS_REGEN_TICKS: u64 = 220;

/// Heal everyone below their maximum, one point each regen tick.
///
/// The dead do not mend and the poisoned only get worse — ServUO's `CanRegenHits`
/// is literally `Alive && !Poisoned`, which here is "no [`Ghost`] marker and no
/// [`Poisoned`] component". A mobile at zero hits is not yet a ghost (`reap`
/// disposes of it a beat later) and must not be healed back out of its own death,
/// so it is skipped too.
pub fn regen_hits(state: &mut WorldState) {
    if !state.ticks.is_multiple_of(HITS_REGEN_TICKS) {
        return;
    }
    let wounded: Vec<EntityId> = state
        .registry
        .query::<Hitpoints>()
        .filter(|(_, hits)| hits.current > 0 && hits.current < hits.max)
        .map(|(entity, _)| entity)
        .filter(|&entity| {
            !state.registry.has::<Poisoned>(entity) && !state.registry.has::<Ghost>(entity)
        })
        .collect();
    for entity in wounded {
        if let Some(&Hitpoints { current, max }) = state.registry.get::<Hitpoints>(entity) {
            state.registry.insert(
                entity,
                Hitpoints {
                    current: (current + 1).min(max),
                    max,
                },
            );
            state.broadcast_health(entity);
        }
    }
}

/// How many ticks between the stamina trickle — a point back roughly every 1.5s,
/// faster than mana's (a mobile winded from a fight recovers its footing sooner
/// than its spells). A tick count, like decay, so a fight replays.
pub const STAMINA_REGEN_TICKS: u64 = 30;

/// Trickle stamina back for everyone below their maximum, one point each regen
/// tick — the mirror of `magic::regen_mana`, run from the tick. Nothing spends
/// stamina on foot in the classic era, so today this only tops a pool off after a
/// stat change lowered and re-raised the cap; it is the seam the combat and
/// overweight drains regenerate against once they land.
pub fn regen_stamina(state: &mut WorldState) {
    if !state.ticks.is_multiple_of(STAMINA_REGEN_TICKS) {
        return;
    }
    let winded: Vec<EntityId> = state
        .registry
        .query::<Stamina>()
        .filter(|(_, stamina)| stamina.current < stamina.max)
        .map(|(entity, _)| entity)
        .collect();
    for entity in winded {
        if let Some(&Stamina { current, max }) = state.registry.get::<Stamina>(entity) {
            state.registry.insert(
                entity,
                Stamina {
                    current: (current + 1).min(max),
                    max,
                },
            );
        }
    }
}

/// A creature's `BaseSoundID` from its body, or `None` for a human or an unlisted
/// body — the key both [`attack_sound`] and [`death_sound`] read.
fn body_base_sound(state: &WorldState, entity: EntityId) -> Option<u16> {
    creature_base_sound(state.registry.get::<Body>(entity)?.id)
}

/// The sound `attacker` makes landing a blow: a creature's own attack sound
/// (ServUO's `BaseSoundID + 2`), or the human fists thwack. So an orc growls its
/// attack instead of punching like a man, which was the point of the sound rule.
fn attack_sound(state: &WorldState, attacker: EntityId, humanoid_fallback: u16) -> u16 {
    body_base_sound(state, attacker)
        .map(|base| base.wrapping_add(2))
        .unwrap_or(humanoid_fallback)
}

/// The growl a creature makes noticing prey — ServUO's `GetAngerSound`
/// (`BaseSoundID + 0`). `None` for a human, which does not growl. The `ai` plays
/// it on the aggro transition, so a monster announces itself when it sees you.
pub fn anger_sound(state: &WorldState, entity: EntityId) -> Option<u16> {
    body_base_sound(state, entity)
}

/// The sound `victim` makes dying: a creature's death sound (`BaseSoundID + 4`), a
/// humanoid's gendered death cry (ServUO's `Random(0x423, 5)` male / `Random(0x314,
/// 4)` female, drawn from the tick's seeded rng so a death replays), or `None` for
/// the passive fauna ServUO leaves silent.
fn death_sound(state: &mut WorldState, victim: EntityId) -> Option<u16> {
    let body = state.registry.get::<Body>(victim)?.id;
    if let Some(base) = creature_base_sound(body) {
        return Some(base.wrapping_add(4));
    }
    if body_opens_doors(body) {
        return Some(if body_is_female(body) {
            0x0314 + state.rng.below(4) as u16
        } else {
            0x0423 + state.rng.below(5) as u16
        });
    }
    None
}
/// The arrow that flies from a bow — ServUO's `Bow.EffectID`. A moving graphical
/// effect draws it crossing the gap to the mark.
const ARROW_GRAPHIC: u16 = 0x0F42;
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
    pub body: u16,
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
/// a second — which [`weapons::swing_base`] picks. `scale` is the operator's
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
            if t < 12 {
                12
            } else {
                t
            }
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
            if t < 5 {
                5
            } else {
                t
            }
        }
        // Pre-AoS (1) and the fallback.
        _ => {
            let t = (scale * 10) / denom;
            if t == 0 {
                1
            } else {
                t
            }
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
/// `op_damage` with no `by`, an environmental hazard) kills without blame.
pub fn damage(
    state: &mut WorldState,
    serial: u32,
    amount: u16,
    kind: DamageType,
    attacker: Option<Serial>,
) {
    let Some(serial) = Serial::new(serial) else {
        return;
    };
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
    // `Mobile.Damage`. Any real (post-resist) damage lifts it at once.
    if amount > 0 {
        state.registry.remove::<Frozen>(entity);
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
                .and_then(|b| b.active.iter().find(|x| x.kind == effect::REACTIVE_ARMOR))
                .map(|x| x.amount)
            {
                let reflected = (u32::from(amount) * pct.max(0) as u32 / 100) as u16;
                if reflected > 0 {
                    damage(
                        state,
                        attacker_serial.raw(),
                        reflected,
                        DamageType::Physical,
                        None,
                    );
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
    let body = state.registry.get::<Body>(entity).map_or(0, |b| b.id);
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
    state.send(connection, encode_war_mode(war));
}

/// Set a player's attack target. The blow itself is not struck here — this only
/// aims; [`swings`] turns "in war mode, in reach, timer up" into damage.
pub fn attack(state: &mut WorldState, connection: ConnectionId, target: u32) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    // A target that cannot be attacked — a serial of zero, an item, the attacker
    // itself, or an invulnerable mobile — clears the aim and un-highlights the
    // client's bar.
    let valid = Serial::new(target)
        .and_then(|serial| {
            state
                .registry
                .entity_of(serial)
                .map(|entity| (serial, entity))
        })
        .filter(|&(_, entity)| {
            entity != player
                && state.registry.has::<Hitpoints>(entity)
                && state.notoriety_of(entity) != Notoriety::Invulnerable
        });
    let Some((serial, target_entity)) = valid else {
        clear_target(state, player);
        state.send(connection, encode_attack(0));
        return;
    };
    let next = state.ticks + swing_speed(state, player);
    if let Some(combat) = state.registry.get_mut::<Combat>(player) {
        combat.target = Some(serial);
        combat.next_swing = next;
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
    state.send(connection, encode_attack(target));
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
    let ready: Vec<(EntityId, Serial, u8, u8)> = state
        .registry
        .query::<Combat>()
        .filter_map(|(attacker, combat)| {
            if !combat.warmode || now < combat.next_swing {
                return None;
            }
            let ranged = state.registry.get::<RangedAttack>(attacker)?;
            combat
                .target
                .map(|target| (attacker, target, ranged.range, ranged.kind))
        })
        .collect();
    for (attacker, target_serial, range, kind) in ready {
        let Some(target) = state.registry.entity_of(target_serial) else {
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
        if !state
            .facet_state(facet)
            .live_terrain()
            .sight_clear(from, to)
        {
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
        let arrow = encode_graphical_effect(
            EffectKind::Moving,
            by.map_or(0, |s| s.raw()),
            target_serial.raw(),
            ARROW_GRAPHIC,
            EffectPoint {
                x: from.x,
                y: from.y,
                z: from.z,
            },
            EffectPoint {
                x: to.x,
                y: to.y,
                z: to.z,
            },
            RANGED_EFFECT_SPEED,
            1,
            false,
            false,
        );
        state.animate(attacker, Action::Attack);
        state.broadcast_from(attacker, arrow);
        let sound = attack_sound(state, attacker, RANGED_HIT_SOUND);
        state.play_sound(attacker, sound);
        // The bolt still flew and twanged; on a miss it simply finds no mark. The
        // hit roll trains the shooter's Archery the same as a melee swing trains
        // its weapon. Damage precedence matches melee via `scaled_blow`.
        if check_hit(state, attacker, target) {
            let amount = scaled_blow(state, attacker, target);
            if let Some(raw) = state.registry.serial_of(target).map(Serial::raw) {
                damage(state, raw, amount, DamageType::from_u8(kind), by);
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
        let (Some(&Position(attacker_pos)), Some(&Position(target_pos))) = (
            state.registry.get::<Position>(attacker),
            state.registry.get::<Position>(target),
        ) else {
            continue;
        };
        if state.facet_of(attacker) != state.facet_of(target)
            || !in_range(attacker_pos, target_pos, MELEE_RANGE)
        {
            continue;
        }
        // The attacker's serial rides along so a lethal blow can be blamed —
        // `damage` is the one place murder is tallied, melee or spell alike.
        let by = state.registry.serial_of(attacker);
        // The swing animates whether it lands or not — a miss still gestures.
        state.animate(attacker, Action::Attack);
        // Roll to hit (and train the weapon skill by trying). A miss whistles past
        // and does no damage; the timer resets either way.
        if !check_hit(state, attacker, target) {
            state.play_sound(attacker, miss_sound(state, attacker));
            set_next_swing(state, attacker, now + swing_speed(state, attacker));
            continue;
        }
        let blow = scaled_blow(state, attacker, target);
        // The blow lands with the attacker's own thwack — a creature's growl, a
        // human's fist — from the attacker, who is still here even when the blow
        // just killed the target.
        let sound = attack_sound(state, attacker, MELEE_HIT_SOUND);
        damage(state, target_serial.raw(), blow, DamageType::Physical, by);
        state.play_sound(attacker, sound);
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
        Some(entity) => state
            .registry
            .get::<Hitpoints>(entity)
            .is_some_and(|hp| hp.current == 0),
    }
}

/// Sphere's murder count threshold: the fifth innocent killed makes you red.
const MURDER_THRESHOLD: u16 = 5;
/// How long one murder count takes to fade — Sphere's short-term default, eight
/// hours at the tick rate. A reformed killer washes blue eventually, not never.
const MURDER_DECAY_TICKS: u64 = 8 * 3600 * 20;

/// Tally a killed innocent against `killer`, turn it red once the tally reaches
/// the threshold, and start the slow fade if it is not already running.
fn record_murder(state: &mut WorldState, killer: EntityId) {
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
        if was_murderer
            && !is_murderer(state, entity)
            && state.notoriety_of(entity) == Notoriety::Murderer
        {
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
pub fn set_next_swing(state: &mut WorldState, attacker: EntityId, tick: u64) {
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
pub const fn poison_damage(level: u8) -> u16 {
    level as u16 + 1
}

/// Poison a mobile at `level` (0 lesser .. 4 lethal), starting its pulses at
/// `now`. A stronger poison overrides a weaker one; a weaker never downgrades a
/// stronger one already working — ServUO's rule.
pub fn apply_poison(state: &mut WorldState, serial: u32, level: u8, now: u64) {
    let Some(entity) = Serial::new(serial).and_then(|s| state.registry.entity_of(s)) else {
        return;
    };
    let level = level.min(4);
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
pub fn cure_poison(state: &mut WorldState, serial: u32) -> bool {
    let Some(entity) = Serial::new(serial).and_then(|s| state.registry.entity_of(s)) else {
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
    let due: Vec<(EntityId, u8, u8)> = state
        .registry
        .query::<Poisoned>()
        .filter(|(_, poison)| now >= poison.next_pulse)
        .map(|(entity, poison)| (entity, poison.level, poison.pulses_left))
        .collect();
    for (entity, level, pulses_left) in due {
        let Some(serial) = state.registry.serial_of(entity) else {
            continue;
        };
        damage(
            state,
            serial.raw(),
            poison_damage(level),
            DamageType::Poison,
            None,
        );
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
        u64::from(weapons::swing_base(&weapon, era))
    });
    swing_ticks(
        dex,
        base,
        state.gameplay.combat_era,
        state.gameplay.speed_scale_factor,
    )
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
        let min = weapons::by_era(weapon.old_min, weapon.aos_min, era);
        let max = weapons::by_era(weapon.old_max, weapon.aos_max, era);
        let span = u32::from(max.saturating_sub(min)) + 1;
        return min + state.rng.below(span) as u16;
    }
    SWING_DAMAGE
}

/// A mobile's value in a skill, in tenths (0 for untrained or no sheet).
fn skill_value(state: &WorldState, mobile: EntityId, skill: u8) -> u16 {
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
fn check_hit(state: &mut WorldState, attacker: EntityId, defender: EntityId) -> bool {
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
    let chance = (1000 * (u32::from(attack) + 500) / (2 * (u32::from(defend) + 500))).min(1000);
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
fn scaled_blow(state: &mut WorldState, attacker: EntityId, defender: EntityId) -> u16 {
    let base = f64::from(melee_blow(state, attacker));
    let era = state.gameplay.combat_era;
    // Skill scaling — a trained fighter only; a creature/untrained mobile deals raw.
    let scaled = if state.registry.has::<Skills>(attacker) {
        let tactics = f64::from(skill_value(state, attacker, weapons::TACTICS_SKILL)) / 10.0;
        let anatomy = f64::from(skill_value(state, attacker, weapons::ANATOMY_SKILL)) / 10.0;
        let strength = f64::from(
            state
                .registry
                .get::<Stats>(attacker)
                .map_or(0, |s| s.strength),
        );
        // Lumberjacking lends an axe a bonus, nothing else.
        let is_axe = weapons::equipped_weapon(state, attacker).is_some_and(|weapon| weapon.is_axe);
        let lumber = if is_axe {
            f64::from(skill_value(state, attacker, weapons::LUMBERJACKING_SKILL)) / 10.0
        } else {
            0.0
        };
        if era >= 2 {
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
    // ServUO's pre-AoS `ComputeDamage`: outside AoS, full damage lands only when a
    // player strikes a non-player — every other pairing (a monster's blow, PvP) is
    // halved. "Player" is a mobile with a client. Applies to every blow, skilled or
    // not, so it sits past the skill gate.
    let is_player = |entity| state.registry.has::<Client>(entity);
    let final_damage = if era < 2 && (is_player(defender) || !is_player(attacker)) {
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
    if era < 2 {
        armor::absorb_physical(state, defender, blow).max(1)
    } else {
        blow
    }
}
