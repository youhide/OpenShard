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
use openshard_protocol::{encode_attack, encode_war_mode, Notoriety};
use openshard_state::components::{
    Client, Combat, CriminalUntil, DamageType, Hitpoints, MeleeDamage, Position, Resistance, Stats,
    SwingSpeed,
};
use openshard_state::sectors::in_range;
use openshard_state::WorldState;

/// How near, in tiles (Chebyshev), a mobile must be to land a melee blow: the
/// next tile over, diagonals included.
pub const MELEE_RANGE: u32 = 1;
/// The base swing speed of bare hands — Sphere's wrestling value. A real weapon's
/// speed (from its tiledata) will replace it once equipment carries properties;
/// until then every mobile swings wrestling-fast, modulated by dexterity alone.
pub const WRESTLING_SPEED: u64 = 50;
/// The dexterity a mobile with no [`Stats`] swings at.
const DEFAULT_DEXTERITY: u16 = 100;
/// Damage a swing deals. A flat number until the damage formula — resistances,
/// weapon, strength — is written, and that is a script-first slice of its own.
pub const SWING_DAMAGE: u16 = 5;

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
/// for yet, so [config validation](openshard_config) accepts only 1 and 2; an
/// unknown era here falls back to era 1.
#[must_use]
pub const fn swing_ticks(dex: u16, base: u64, era: u8, scale: u64) -> u64 {
    let base = if base == 0 { 1 } else { base };
    let denom = (dex as u64 + 100) * base;
    let tenths = match era {
        2 => {
            let t = ((scale * 10) / denom) / 2;
            if t < 12 {
                12
            } else {
                t
            }
        }
        // Era 1 and the fallback.
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
pub fn damage(state: &mut WorldState, serial: u32, amount: u16, kind: DamageType) {
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
    });
    state.broadcast_health(entity);
    if remaining == 0 {
        die(state, entity, serial);
    }
}

/// A mobile's hit points reached zero.
///
/// Emits [`MobileDied`] for whoever cares — loot, notoriety, a script — and then,
/// for a creature, takes it off the world. A *player* who dies stays put for now:
/// ghosts, corpses and resurrection are a later slice, and despawning someone
/// still connected is worse than leaving them standing.
pub fn die(state: &mut WorldState, entity: EntityId, serial: Serial) {
    state.bus.send(MobileDied { entity, serial });
    if state.registry.has::<Client>(entity) {
        return;
    }
    let facet = state.facet_of(entity);
    for watcher in state.watchers_of(entity) {
        state.forget(watcher, entity, serial);
    }
    state.seen.remove(&entity);
    state.facet_state_mut(facet).sectors.remove(entity);
    state.registry.despawn(entity);
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
        let blow = melee_blow(state, attacker);
        damage(state, target_serial.raw(), blow, DamageType::Physical);
        set_next_swing(state, attacker, now + swing_speed(state, attacker));
        // The blow may have killed it; a dead target is no target.
        if state.registry.entity_of(target_serial).is_none() {
            clear_target(state, attacker);
        }
    }
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

/// Turn a mobile grey for [`CRIMINAL_TICKS`], or push the timer out if it is
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

/// Restore anyone whose criminal flag has run out to innocent, and redraw the
/// blue for everyone watching. Runs each tick against the tick counter.
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
        state.registry.insert(entity, Notoriety::Innocent);
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
    swing_ticks(
        dex,
        WRESTLING_SPEED,
        state.gameplay.combat_era,
        state.gameplay.speed_scale_factor,
    )
}

/// The base damage a blow from `attacker` carries, before armour — its
/// [`MeleeDamage`], or the default. The target's resistance is applied later, in
/// [`damage`], the one place all damage passes through.
#[must_use]
pub fn melee_blow(state: &WorldState, attacker: EntityId) -> u16 {
    state
        .registry
        .get::<MeleeDamage>(attacker)
        .map_or(SWING_DAMAGE, |d| d.amount)
}
