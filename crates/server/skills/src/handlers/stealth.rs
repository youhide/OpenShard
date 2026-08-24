//! Hiding, Stealth, Detecting Hidden and Stealing — the skills that turn on being
//! seen or not.
//!
//! ServUO's `Hiding`, `Stealth`, `DetectHidden` and `Stealing`. What makes them a
//! subsystem rather than four skills is that the state they share —
//! [`Hidden`](openshard_state::components::Hidden) and
//! [`Stealthing`](openshard_state::components::Stealthing) — is read by one gate
//! (`WorldState::can_see_mobile`) and broken by one call
//! (`WorldState::break_cover`), both in `state`, so that attacking, speaking,
//! casting and lifting can each give a hider away without knowing what hiding is.
//!
//! Pre-AoS throughout: the armour rating a hider is judged on is the plain worn
//! rating, not AoS's per-material stealth table, and the thresholds are the ones
//! ServUO uses outside AoS.

use openshard_entities::EntityId;
use openshard_protocol::wire::ClilocId;
use openshard_state::components::{Combat, Contained, Hidden, Position, Stealthing};
use openshard_state::{Skill, WorldState, in_range};

use crate::check::{roll_skill_band, roll_skill_chance};

/// "You can't seem to hide right now." — somebody is fighting you.
const CANNOT_HIDE_NOW: ClilocId = ClilocId(501_237);
/// "You have hidden yourself well."
const HIDDEN_WELL: ClilocId = ClilocId(501_240);
/// "You can't seem to hide here."
const CANNOT_HIDE_HERE: ClilocId = ClilocId(501_241);
/// "You must hide first."
const HIDE_FIRST: ClilocId = ClilocId(502_725);
/// "You are not hidden well enough. Become better at hiding."
const NOT_HIDDEN_ENOUGH: ClilocId = ClilocId(502_726);
/// "You could not hope to move quietly wearing this much armor."
const TOO_MUCH_ARMOUR: ClilocId = ClilocId(502_727);
/// "You begin to move quietly."
const MOVING_QUIETLY: ClilocId = ClilocId(502_730);
/// "You fail in your attempt to move unnoticed."
const FAILED_TO_STEALTH: ClilocId = ClilocId(502_731);
/// "You disturb the stillness and reveal what was hidden." — ServUO's reveal line.
const YOU_REVEAL: ClilocId = ClilocId(500_814);
/// "You are unable to detect anything unusual."
const DETECT_NOTHING: ClilocId = ClilocId(500_817);

/// The Hiding a mobile needs before Stealth will work at all, in tenths — ServUO's
/// `HidingRequirement` outside SE.
const STEALTH_NEEDS_HIDING: u16 = 800;
/// The armour rating past which nobody moves quietly, pre-AoS.
const STEALTH_ARMOUR_CAP: u16 = 26;
/// How many points of skill buy one quiet step, in tenths — `value / 10.0`
/// pre-AoS, which over a value in tenths is a divide by a hundred.
const SKILL_PER_STEALTH_STEP: u16 = 100;

/// Hiding: drop out of sight where you stand.
///
/// ServUO's `Hiding.OnUse`. The interesting gate is the first: **you cannot hide
/// from somebody who is fighting you** and can see you, whatever your skill — which
/// is what stops hiding being a combat escape, and it is checked both ways (your
/// own combatant, and anyone whose target is you).
pub(super) fn hiding(state: &mut WorldState, actor: EntityId) {
    let skill = Skill::Hiding;
    // `min((100 - value)/2 + 8, 18)` — the better the hider, the shorter the
    // distance at which a fight still gives them away.
    let value = crate::skill_value(state, actor, skill).min(1000) / 10;
    let range = u32::from((100 - value) / 2 + 8).min(18);
    if somebody_is_fighting(state, actor, range) {
        state.break_cover(actor);
        state.localized_message(actor, CANNOT_HIDE_NOW, "");
        return;
    }
    if roll_skill_band(state, actor, skill, crate::SkillBand::new(0, 1000)) {
        // Hiding drops war mode: ServUO's `Hidden = true` setter clears it, and a
        // hider still visibly squared up would be a contradiction on every screen.
        state.registry.remove::<Combat>(actor);
        state.conceal(actor);
        state.localized_message(actor, HIDDEN_WELL, "");
    } else {
        state.break_cover(actor);
        state.localized_message(actor, CANNOT_HIDE_HERE, "");
    }
}

/// Stealth: move without being seen, for a few steps.
///
/// The budget is `value / 10` steps pre-AoS, spent by the movement paths through
/// `WorldState::step_while_hidden`. Everything here is a gate on being allowed to
/// start: you must be hidden, hidden *well* (80.0 Hiding), and not in armour.
pub(super) fn stealth(state: &mut WorldState, actor: EntityId) {
    let skill = Skill::Stealth;
    if !state.registry.has::<Hidden>(actor) {
        state.localized_message(actor, HIDE_FIRST, "");
        return;
    }
    let hiding = state
        .registry
        .get::<openshard_state::Skills>(actor)
        .map_or(0, |s| s.get(Skill::Hiding));
    if hiding < STEALTH_NEEDS_HIDING {
        state.break_cover(actor);
        state.localized_message(actor, NOT_HIDDEN_ENOUGH, "");
        return;
    }
    // Pre-AoS this is the plain worn rating, which is why plate ends the attempt
    // and leather does not. (AoS swaps in a per-material table; that is the
    // deferred half, like the AoS variants of the behaviour buffs.)
    let armour = openshard_state::armor::worn_armor_rating(state, actor);
    if armour >= STEALTH_ARMOUR_CAP {
        state.break_cover(actor);
        state.localized_message(actor, TOO_MUCH_ARMOUR, "");
        return;
    }
    // `-20 + ar*2 .. 80 + ar*2`: armour makes the roll harder at both ends.
    let shift = i32::from(armour) * 20;
    if roll_skill_band(
        state,
        actor,
        skill,
        crate::SkillBand::new(-200 + shift, 800 + shift),
    ) {
        let steps = (crate::skill_value(state, actor, skill) / SKILL_PER_STEALTH_STEP).max(1);
        state.registry.insert(actor, Stealthing { steps_left: steps });
        state.localized_message(actor, MOVING_QUIETLY, "");
    } else {
        state.break_cover(actor);
        state.localized_message(actor, FAILED_TO_STEALTH, "");
    }
}

/// Detecting Hidden: look around for what is not there.
///
/// ServUO's `DetectHidden`, with its own reach (`1 + value/10` tiles) and its own
/// contest: your Detect Hidden against each hider's Hiding, not a flat roll. A
/// successful *search* that finds nobody says so, which is a different line from
/// failing to search at all — the client has both, and they are the difference
/// between "there is nobody here" and "you learned nothing".
pub(super) fn detect_hidden(state: &mut WorldState, actor: EntityId) {
    let skill = Skill::DetectHidden;
    let value = crate::skill_value(state, actor, skill);
    if !roll_skill_band(state, actor, skill, crate::SkillBand::new(0, 1000)) {
        state.localized_message(actor, DETECT_NOTHING, "");
        return;
    }
    let range = 1 + u32::from(value.min(1000) / 100);
    let Some(&Position(at)) = state.registry.get::<Position>(actor) else {
        return;
    };
    let facet = state.facet_of(actor);
    let hidden: Vec<EntityId> = state
        .facet_state(facet)
        .sectors()
        .mobiles_near(at, range)
        .map(|(entity, _)| entity)
        .filter(|&entity| entity != actor && state.registry.has::<Hidden>(entity))
        .collect();
    let mut found = false;
    for target in hidden {
        // ServUO's contest: `srcSkill / 1.5` against the hider's Hiding. A
        // grandmaster searcher does not automatically strip a grandmaster hider.
        let theirs = crate::skill_value(state, target, Skill::Hiding);
        let chance = i32::from(value) * 1000 / 1500 - i32::from(theirs);
        if chance > i32::try_from(state.rng.below(1000)).unwrap_or(0) {
            state.break_cover(target);
            found = true;
        }
    }
    if found {
        state.localized_message(actor, YOU_REVEAL, "");
    } else {
        state.localized_message(actor, DETECT_NOTHING, "");
    }
}

/// "You catch yourself red-handed." — stealing from yourself.
const RED_HANDED: ClilocId = ClilocId(502_704);
/// "You cannot steal that."
const CANNOT_STEAL: ClilocId = ClilocId(502_710);
/// "That is too heavy to steal."
const TOO_HEAVY: ClilocId = ClilocId(502_711);
/// "You reach into their pack and take the item."
const STOLE_IT: ClilocId = ClilocId(502_724);
/// "You fail to steal the item." — and everyone finds out.
const FAILED_TO_STEAL: ClilocId = ClilocId(502_723);
/// "You notice %s trying to steal from you!" — said to the victim.
const CAUGHT_YOU: ClilocId = ClilocId(1_010_585);
/// The most a thief can lift, in stones — ServUO's `10 + value/10`, over a value
/// in tenths.
const STEAL_WEIGHT_PER_SKILL: u16 = 100;
/// The base weight a novice can manage.
const STEAL_BASE_WEIGHT: u16 = 10;
/// How far a thief can reach — ServUO's `Target(1, …)`.
pub(super) const STEAL_RANGE: u32 = 1;

/// Stealing: take something out of somebody else's pack.
///
/// ServUO's `Stealing`, in its pre-AoS shape. Two things about it are the whole
/// skill and both are here: the weight limit (`10 + value/10` stones, so a
/// grandmaster still cannot lift a suit of plate off somebody) and what happens
/// when you fail — the victim is *told*, by name, and you are a criminal. A thief
/// who could try freely would simply try until it worked.
pub(super) fn stealing(state: &mut WorldState, actor: EntityId, item: EntityId) -> Option<Stolen> {
    let skill = Skill::Stealing;
    // Only something in somebody else's pack can be stolen: the ground is a lift
    // and your own pack is not theft.
    let Some(&Contained { container, .. }) = state.registry.get::<Contained>(item) else {
        state.localized_message(actor, CANNOT_STEAL, "");
        return None;
    };
    let Some(victim) = openshard_items::owner_of_container(state, container) else {
        state.localized_message(actor, CANNOT_STEAL, "");
        return None;
    };
    if victim == actor {
        state.localized_message(actor, RED_HANDED, "");
        return None;
    }
    let weight = openshard_items::weight_of(state, item);
    let allowed = STEAL_BASE_WEIGHT + crate::skill_value(state, actor, skill) / STEAL_WEIGHT_PER_SKILL;
    if weight > allowed {
        state.localized_message(actor, TOO_HEAVY, "");
        return None;
    }
    // The chance is ServUO's: the item's weight sets the difficulty, so a purse of
    // gold is easy and a breastplate is not worth trying.
    let chance = 1000 - i32::from(weight) * 1000 / i32::from(allowed.max(1));
    let took = roll_skill_chance(state, actor, skill, chance.clamp(0, 1000) as u32);
    // Caught or not, the reach was made and it gives you away.
    state.break_cover(actor);
    if took {
        state.localized_message(actor, STOLE_IT, "");
    } else {
        state.localized_message(actor, FAILED_TO_STEAL, "");
        let name = state
            .registry
            .get::<openshard_state::components::Name>(actor)
            .map_or_else(String::new, |n| n.0.clone());
        state.localized_message(victim, CAUGHT_YOU, &name);
    }
    Some(Stolen {
        thief: actor,
        victim,
        item,
        took,
    })
}

/// A theft was attempted. Returned rather than carried out, because moving an item
/// into a backpack is `items`' door and turning a thief criminal is `combat`'s —
/// the tick does both, the decide-then-apply split again.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stolen {
    /// Who reached.
    pub thief: EntityId,
    /// Whose pack it was.
    pub victim: EntityId,
    /// What they reached for.
    pub item: EntityId,
    /// Whether they got it.
    pub took: bool,
}

/// Whether anybody within `range` is in a fight with `actor` — ServUO's two-way
/// check, which is what stops hiding being a combat escape.
fn somebody_is_fighting(state: &WorldState, actor: EntityId, range: u32) -> bool {
    let Some(&Position(at)) = state.registry.get::<Position>(actor) else {
        return false;
    };
    let Some(serial) = state.registry.serial_of(actor) else {
        return false;
    };
    // Your own combatant, if they are near enough to watch you do it.
    if let Some(combat) = state.registry.get::<Combat>(actor) {
        if let Some(target) = combat.target.and_then(|t| state.registry.entity_of(t)) {
            if state
                .registry
                .get::<Position>(target)
                .is_some_and(|Position(spot)| in_range(at, *spot, range))
            {
                return true;
            }
        }
    }
    // And anyone whose target is you.
    let facet = state.facet_of(actor);
    state
        .facet_state(facet)
        .sectors()
        .mobiles_near(at, range)
        .any(|(entity, _)| {
            entity != actor
                && state
                    .registry
                    .get::<Combat>(entity)
                    .is_some_and(|combat| combat.target == Some(serial))
        })
}

/// "You cannot peek into the container." — a staff pack, or someone dead.
const CANNOT_PEEK: ClilocId = ClilocId(500_209);
/// What snooping costs in karma, every time, whether it works or not.
const SNOOP_KARMA: i32 = -4;

/// Snooping: open a container in somebody else's pack without their leave.
///
/// ServUO's `Container_Snoop`, called where a container is *opened* rather than
/// from the skill window — Snooping is one of the skills with no button, because
/// the action that uses it is an ordinary double-click. Returns whether the gump
/// may open.
///
/// Two things happen whatever the roll: the victim is told, by name, if the snoop
/// was clumsy, and the snooper loses karma. That is what stops it being free to
/// try.
pub fn snooping(state: &mut WorldState, actor: EntityId, container: EntityId) -> bool {
    let Some(serial) = state.registry.serial_of(container) else {
        return true;
    };
    let Some(owner) = openshard_items::owner_of_container(state, serial) else {
        return true; // a chest on the ground belongs to nobody: no snooping to do
    };
    if owner == actor || state.is_staff(actor) {
        return true;
    }
    // ServUO refuses outright on a staff mobile's pack, and quietly on a dead one.
    if state.is_staff(owner) {
        state.localized_message(actor, CANNOT_PEEK, "");
        return false;
    }
    if state.registry.has::<openshard_state::components::Ghost>(owner) {
        return false;
    }
    let skill = Skill::Snooping;
    // The *noticing* is a separate roll from the success, and it comes first:
    // ServUO compares the raw skill against a d100, so a clumsy snoop is spotted
    // even when the peek itself then works.
    if i32::from(crate::skill_value(state, actor, skill) / 10)
        < i32::try_from(state.rng.below(100)).unwrap_or(0)
    {
        let name = state
            .registry
            .get::<openshard_state::components::Name>(actor)
            .map_or_else(String::new, |n| n.0.clone());
        state.system_message(owner, &format!("You notice {name} peeking into your belongings!"));
    }
    openshard_state::title::award_karma(state, actor, SNOOP_KARMA);
    roll_skill_band(state, actor, skill, crate::SkillBand::new(0, 1000))
}
