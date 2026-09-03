//! Town guards: the classic answer to a criminal flag.
//!
//! The engine has been able to turn someone grey since notoriety landed, and
//! nothing has ever come of it — there was no such thing as a *place* where that
//! mattered. Regions are that place, and this is what happens there.
//!
//! # A sentence, not a fight
//!
//! ServUO's `WarriorGuard` does not chase, wear the offender down and win: it
//! materialises on top of them, bolts them, and deals their entire hit point
//! total (`AttackTimer.OnTick`, the block its own author labelled
//! `<instakill>`). That is the classic behaviour and it is the right one — a
//! guard that can be fought is a guard that can be beaten, and then a town is
//! just a place with slightly more dangerous scenery.
//!
//! So a guard here is barely a creature. It is spawned when called, it executes,
//! and it wanders off and vanishes ([`Guard`] carries the tick it does). It
//! never earns a murder count for the killing, because killing the guilty is the
//! whole of its purpose — ServUO says the same thing by clearing the guard's own
//! `Criminal` and `Kills` on every beat of its timer.

use openshard_combat as combat;
use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::wire::{
    Graphic,
    Hue,
    Layer,
    SoundId,
};
use openshard_protocol::world::{
    Facet,
    Point,
    Sight,
};
use openshard_state::WorldState;
use openshard_state::components::{
    Aggression,
    Client,
    CriminalUntil,
    DamageType,
    Ghost,
    Guard,
    Hitpoints,
    Murders,
    Position,
    Spellbook,
    Staff,
};

use crate::dress::ShoeType;
use crate::names::personal_name;
use crate::notify;
use crate::spawn::{
    SpawnSpec,
    spawn,
};

/// How far a call for guards reaches for someone to answer it — ServUO's 14.
const CALL_RANGE: u32 = 14;
/// How far a guard looks for trouble it can already see, before deciding a call
/// found nobody. Kept below [`CALL_RANGE`] so the call is the wider net.
const GUARD_SIGHT: Sight = Sight(8);
/// How many murders make a mobile guard-worthy on sight — combat's own
/// threshold, so "red" means the same thing to the guards as to the health bar.
const MURDER_THRESHOLD: u16 = 5;
/// How long a guard sticks around with nothing to do before vanishing, in ticks
/// — forty seconds. ServUO's idle timer wanders it a while, then deletes it.
const IDLE_TICKS: u64 = 40 * openshard_state::TICKS_PER_SECOND;
/// The male and female guard bodies — plain humans, as ServUO's guard is.
const GUARD_BODIES: [Graphic; 2] = [Graphic(0x0190), Graphic(0x0191)];
/// Plate and a halberd, so it reads as a guard on sight: `(graphic, layer, hue)`.
const GUARD_KIT: [(Graphic, Layer, Hue); 4] = [
    (Graphic(0x1415), Layer(0x05), Hue(0)), // plate chest, on the torso layer
    (Graphic(0x1410), Layer(0x13), Hue(0)), // plate arms
    (Graphic(0x1411), Layer(0x04), Hue(0)), // plate legs
    (Graphic(0x143E), Layer(0x02), Hue(0)), // halberd, in hand
];
/// The teleport-in sparkle and its sound — ServUO's `WarriorGuard.TeleportTo`,
/// the same pair the Teleport spell uses.
const ARRIVAL_GRAPHIC: Graphic = Graphic(0x3728);
const ARRIVAL_SOUND: u16 = 0x01FE;
/// What a guard says as it takes its focus — ServUO's cliloc 500131.
const GUARD_LINE: &str = "Thou wilt regret thine actions, swine!";

/// Answer the "guards" keyword for a speaking player.
///
/// Only inside a guarded region: the words are the *call*, and a call carries no
/// authority in the wilds. ServUO matches the same speech keyword (`0x0007`);
/// matching the plain word is the shape the banker's "bank" already set.
pub fn guard_keywords(state: &mut WorldState, connection: ConnectionId, actor: EntityId, text: &str) {
    let lower = text.to_lowercase();
    if !lower
        .split(|c: char| !c.is_alphabetic())
        .any(|word| word == "guards" || word == "guard")
    {
        return;
    }
    let Some(&Position(at)) = state.registry.get::<Position>(actor) else {
        return;
    };
    let facet = state.facet_of(actor);
    if !guarded_here(state, facet, at) {
        return;
    }
    if !call_guards(state, at, facet) {
        notify(state, connection, "The guards find no one to punish here.");
    }
}

/// Call the guards on whoever near `at` deserves it. `true` if one was sent.
///
/// ServUO's `GuardedRegion.CallGuards`: the first candidate within
/// [`CALL_RANGE`] is taken, and only one guard is made — a call is not a militia
/// muster.
pub fn call_guards(state: &mut WorldState, at: Point, facet: Facet) -> bool {
    let Some(target) = nearest_candidate(state, at, facet) else {
        return false;
    };
    make_guard(state, target);
    true
}

/// Send a guard for one specific mobile, if it is a candidate and standing
/// somewhere guarded — the murderer-walks-into-town path, off the region
/// crossing.
pub fn hunt_with_guards(state: &mut WorldState, target: EntityId) -> bool {
    let Some(&Position(at)) = state.registry.get::<Position>(target) else {
        return false;
    };
    let facet = state.facet_of(target);
    if !guarded_here(state, facet, at) || !is_candidate(state, target) {
        return false;
    }
    make_guard(state, target);
    true
}

/// Whether a guarded region covers a point — and whether the shard has guards on
/// at all.
fn guarded_here(state: &WorldState, facet: Facet, at: Point) -> bool {
    state.gameplay.guards
        && state
            .region_at(facet, at)
            .is_some_and(|region| region.flags.guarded)
}

/// Whether a mobile is something the guards may act on — ServUO's
/// `IsGuardCandidate`, plus the standing this engine keeps.
///
/// Being *guilty* is the last clause, not the first: a dead man, a guard and a
/// game master are all exempt whatever they have done.
fn is_candidate(state: &WorldState, mobile: EntityId) -> bool {
    if state.registry.has::<Guard>(mobile)
        || state.registry.has::<Staff>(mobile)
        || state.registry.has::<Ghost>(mobile)
    {
        return false;
    }
    if state.notoriety_of(mobile) == Notoriety::Invulnerable {
        return false;
    }
    // A mobile at zero hits is already dead in every rule that matters.
    if state
        .registry
        .get::<Hitpoints>(mobile)
        .is_some_and(|hits| hits.current == 0)
    {
        return false;
    }
    let criminal = state.registry.has::<CriminalUntil>(mobile);
    let murderer = state
        .registry
        .get::<Murders>(mobile)
        .is_some_and(|count| count.0 >= MURDER_THRESHOLD);
    // A monster loose in a town is guard business too, exactly as in ServUO —
    // which is why a dragon cannot camp the bank.
    let monster = state
        .registry
        .get::<openshard_state::components::Brain>(mobile)
        .is_some_and(|brain| brain.aggression == Aggression::Aggressive)
        && !state.registry.has::<Client>(mobile);
    criminal || murderer || monster
}

/// The nearest guard-worthy mobile to a point, on the same facet.
fn nearest_candidate(state: &WorldState, at: Point, facet: Facet) -> Option<EntityId> {
    state
        .facets
        .get(&facet)?
        .sectors()
        .mobiles_near(at, CALL_RANGE)
        .filter(|&(entity, _)| is_candidate(state, entity))
        .min_by_key(|&(_, point)| openshard_state::sectors::distance(point, at))
        .map(|(entity, _)| entity)
}

/// Spawn a guard for `target` and let it do its work.
///
/// The spawn, the arrival and the sentence are one call because they are one
/// event: unlike a creature, a guard has no life before or after this.
fn make_guard(state: &mut WorldState, target: EntityId) {
    let Some(&Position(at)) = state.registry.get::<Position>(target) else {
        return;
    };
    let facet = state.facet_of(target);
    let body = GUARD_BODIES[state.rng.below(GUARD_BODIES.len() as u32) as usize];
    let name = guard_name(state);
    let Some(guard) = spawn(
        state,
        SpawnSpec {
            body,
            hue: Hue(0),
            // Not invulnerable, but not meant to be fought either: it is gone
            // before a fight could start.
            hits: 1000,
            notoriety: Notoriety::Innocent,
            damage: 0,
            resistance: openshard_protocol::world::PhysicalResistance::new(0),
            swing: 0,
            sight: GUARD_SIGHT,
            aggression: Aggression::Defensive,
            beat: 0,
            ranged: None,
            ranged_kind: DamageType::Physical,
            wander: false,
            position: at,
            facet,
            name: Some(name),
            // A guard is not a townsperson: it appears, speaks its one line, deals
            // its blow and is gone. No trade, so no generated dress (its body
            // already *is* the uniform), no beat, no keyword table.
            title: None,
            shoe: ShoeType::default(),
            // A guard earns nobody anything: it is gone before it can be fought, and a
            // shard where killing the town watch made you famous would be a different game.
            fame: 0,
            karma: 0,
            night_home: None,
            banker: false,
            vendor: false,
            healer: false,
            equipment: GUARD_KIT.to_vec(),
            skills: Vec::new(),
            // A guard kills by decree, not by spell — see `execute` below, which
            // is the whole of its violence.
            mana: 0,
            spells: Spellbook(0),
        },
    ) else {
        return;
    };
    let until = state.ticks + IDLE_TICKS;
    state.registry.insert(guard, Guard { until });
    flash(state, guard, at);
    execute(state, guard, target);
}

/// The flash a guard comes and goes in: the teleport sparkle and its sound, to
/// everyone who can see the spot. A mobile that simply blinks into existence —
/// or out of it — with no feedback reads as a client glitch, which is why
/// [`crate::flash`] is shared with the summon that goes out in one.
fn flash(state: &mut WorldState, guard: EntityId, at: Point) {
    crate::flash(state, guard, at, ARRIVAL_GRAPHIC, SoundId(ARRIVAL_SOUND));
}

/// The sentence: the guard speaks, and the target takes its whole hit point
/// total as physical damage.
///
/// The blow goes through `combat::damage` like every other — so the corpse, the
/// loot, the `MobileDied` and the criminal bookkeeping all happen the usual way,
/// and there is no second death path to keep in step with the first.
fn execute(state: &mut WorldState, guard: EntityId, target: EntityId) {
    crate::say(state, guard, GUARD_LINE);
    let Some(serial) = state.registry.serial_of(target) else {
        return;
    };
    let by = state.registry.serial_of(guard);
    let blow = state
        .registry
        .get::<Hitpoints>(target)
        .map_or(u16::MAX, |hits| hits.max);
    combat::damage(state, serial, blow, DamageType::Physical, by);
}

/// Retire the guards whose time is up. The tick counter, like every other timer
/// here, so a shard replays.
pub fn expire_guards(state: &mut WorldState) {
    let now = state.ticks;
    let done: Vec<EntityId> = state
        .registry
        .query::<Guard>()
        .filter(|(_, guard)| now >= guard.until)
        .map(|(entity, _)| entity)
        .collect();
    for guard in done {
        if let Some(&Position(at)) = state.registry.get::<Position>(guard) {
            flash(state, guard, at);
        }
        state.despawn_mobile(guard);
    }
}

/// A guard's name: a personal name and the title, from the world's seeded
/// generator so a replay names it the same.
fn guard_name(state: &mut WorldState) -> String {
    let name = personal_name(&mut state.rng, false);
    format!("{name} the guard")
}
