//! Begging, and Remove Trap: the two skills used *on* something that is not gear.
//!
//! ServUO's `Begging` and `RemoveTrap`. They share a file because they share a
//! shape — a cursor, a set of refusals with the client's own lines, and one roll —
//! and neither is large enough to be alone.

use openshard_entities::EntityId;
use openshard_protocol::localized::begging::{
    FEEL_SORRY, FROM_A_THING, FROM_PLAYER, NOT_ENOUGH_MONEY, NOT_TRUSTWORTHY, TOO_FAR_HER, TOO_FAR_HIM,
    UNWILLING,
};
use openshard_protocol::wire::ClilocId;
use openshard_state::components::{Body, BodyType, Client, Fame, Karma, Lock, Riding, Trap, body_type};
use openshard_state::{Skill, WorldState};

use crate::check::roll_skill_band;

/// How close you must be to beg — ServUO's `InRange(targ, 2)`.
const BEG_RANGE: u32 = 2;
/// The karma floor begging pushes toward, and the most it takes at once.
const BEG_KARMA_FLOOR: i32 = -3000;
/// The most karma one successful beg costs.
const BEG_KARMA_MAX: i32 = 40;
/// The smallest handout a beggar can talk somebody out of.
const BEG_MIN_GOLD: u32 = 10;
/// The largest, at maximum fame.
const BEG_MAX_GOLD: u32 = 14;
/// How much fame buys one more coin — ServUO's `10 + Fame / 2500`.
const FAME_PER_COIN: i32 = 2500;
/// The bad-karma refusal chance, in per-mille: `0.5 - karma/8570`.
const BAD_KARMA_BASE: i32 = 500;
/// The divisor under that chance, scaled to per-mille.
const BAD_KARMA_DIVISOR: i32 = 857;

/// "You do not know enough about locks. Become better at picking locks."
const NOT_ENOUGH_LOCKS: ClilocId = ClilocId(502_366);
/// "You are not perceptive enough. Become better at detect hidden."
const NOT_PERCEPTIVE: ClilocId = ClilocId(502_367);
/// "That is locked."
const THAT_IS_LOCKED: ClilocId = ClilocId(501_283);
/// "That doesn't appear to be trapped."
const NOT_TRAPPED: ClilocId = ClilocId(502_373);
/// "You successfully render the trap harmless."
const TRAP_REMOVED: ClilocId = ClilocId(502_377);
/// "You fail to disarm the trap... but you don't set it off."
const TRAP_NOT_REMOVED: ClilocId = ClilocId(502_372);
/// "You feel that such an action would be inappropriate." — disarming a person.
const INAPPROPRIATE: ClilocId = ClilocId(502_816);
/// The Lockpicking and Detect Hidden a would-be trap remover needs, in tenths.
const TRAP_PREREQUISITE: u16 = 500;
/// How much harder than its power a trap is to take off — ServUO's band.
const TRAP_BAND_WIDTH: i32 = 100;

/// Whether a mobile may be begged from at all, and the line if not.
fn beg_refusal(state: &WorldState, target: EntityId) -> Option<ClilocId> {
    if state.registry.has::<Client>(target) {
        return Some(FROM_PLAYER);
    }
    let Some(body) = state.registry.get::<Body>(target) else {
        return Some(FROM_A_THING);
    };
    if body_type(body.id) != BodyType::Human {
        return Some(FROM_A_THING);
    }
    None
}

/// Begging: ask a townsperson for coin, and be a little worse for it.
///
/// ServUO's `Begging`, resolved at once rather than after its two-second timer —
/// the bow still plays, and the button is held ten seconds either way, so the beat
/// is flavour rather than mechanism.
///
/// **Where the coin comes from is ours.** ServUO takes a tenth of what is actually
/// in the target's pack, and its NPCs carry pack gold because that is where its
/// corpse loot comes from; here an NPC carries none and a corpse's baseline gold is
/// invented at death (`corpse_gold`). So a townsperson begged from gives from a
/// notional purse, the same invention the corpse already makes, and the "I have not
/// enough money" line is kept for the case that matters: begging from something
/// that is not a person.
pub(super) fn begging(state: &mut WorldState, actor: EntityId, target: EntityId) {
    if let Some(line) = beg_refusal(state, target) {
        state.localized_message(actor, line, "");
        return;
    }
    if !super::within(state, actor, target, BEG_RANGE) {
        let female = state
            .registry
            .get::<Body>(target)
            .is_some_and(|body| openshard_state::components::body_is_female(body.id));
        state.localized_message(actor, if female { TOO_FAR_HER } else { TOO_FAR_HIM }, "");
        return;
    }
    // Nobody hands coin up to somebody on a horse.
    if state.registry.has::<Riding>(actor) {
        state.localized_message(actor, UNWILLING, "");
        return;
    }

    // The bow, whatever comes of it.
    state.face_toward(actor, target);
    state.face_toward(target, actor);
    state.animate(actor, openshard_state::Action::Bow);

    let karma = state.registry.get::<Karma>(actor).map_or(0, |k| k.0);
    // `0.5 - karma/8570` in per-mille: a beggar of ill repute is turned away, and
    // one of good standing hardly ever is. Rolled on the world's generator.
    let bad_chance = BAD_KARMA_BASE - karma / BAD_KARMA_DIVISOR;
    if karma < 0 && bad_chance > i32::from(crate::roll_u16(&mut state.rng, crate::PER_MILLE)) {
        state.private_overhead_cliloc(actor, target, NOT_TRUSTWORTHY, "");
        return;
    }
    let skill = Skill::Begging;
    if !roll_skill_band(state, actor, skill, crate::SkillBand::new(0, 1000)) {
        state.localized_message(actor, UNWILLING, "");
        return;
    }
    // `10 + Fame/2500`, clamped to 10..=14 — a famous beggar does better.
    let fame = state.registry.get::<Fame>(actor).map_or(0, |f| f.0);
    let amount = u32::try_from(fame / FAME_PER_COIN)
        .unwrap_or(0)
        .saturating_add(BEG_MIN_GOLD)
        .clamp(BEG_MIN_GOLD, BEG_MAX_GOLD);
    // A vendor's till is its stock crate, not a purse — ServUO's beggar takes a
    // tenth of what is in the target's pack, and a shopkeeper is the one townsperson
    // here whose goods are visibly *not* coin.
    if state.registry.has::<openshard_state::components::Vendor>(target) {
        state.private_overhead_cliloc(actor, target, NOT_ENOUGH_MONEY, "");
        return;
    }
    state.private_overhead_cliloc(actor, target, FEEL_SORRY, "");
    state.bus.send(Begged {
        entity: actor,
        gold: amount,
    });
    // And it costs you, up to forty, down to a floor of −3000: the classic reason
    // a career beggar is nobody's idea of a hero.
    if karma > BEG_KARMA_FLOOR {
        let loss = (karma - BEG_KARMA_FLOOR).min(BEG_KARMA_MAX);
        openshard_state::title::award_karma(state, actor, -loss);
    }
}

/// A beggar talked somebody out of some coin.
///
/// Emitted rather than paid, because putting an item in a backpack is the `items`
/// crate's door and this one only decides — the tick pays it, the same
/// decide-then-apply split the poison fumble uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Begged {
    /// Who begged.
    pub entity: EntityId,
    /// How much they were given.
    pub gold: u32,
}

/// Whether a mobile knows enough to attempt a trap at all — the two prerequisites
/// ServUO checks before it even raises the cursor.
pub(super) fn may_remove_traps(state: &mut WorldState, actor: EntityId) -> bool {
    if crate::skill_value(state, actor, Skill::Lockpicking) < TRAP_PREREQUISITE {
        state.localized_message(actor, NOT_ENOUGH_LOCKS, "");
        return false;
    }
    if crate::skill_value(state, actor, Skill::DetectHidden) < TRAP_PREREQUISITE {
        state.localized_message(actor, NOT_PERCEPTIVE, "");
        return false;
    }
    true
}

/// Remove Trap: take the trap off a chest without setting it off.
pub(super) fn remove_trap(state: &mut WorldState, actor: EntityId, target: EntityId) {
    if state.registry.has::<Body>(target) {
        state.localized_message(actor, INAPPROPRIATE, "");
        return;
    }
    // A locked chest cannot be worked on — you would be picking the lock, not the
    // trap. ServUO refuses in that order and so does this.
    if state.registry.has::<Lock>(target) {
        state.localized_message(actor, THAT_IS_LOCKED, "");
        return;
    }
    let Some(&Trap { power, .. }) = state.registry.get::<Trap>(target) else {
        state.localized_message(actor, NOT_TRAPPED, "");
        return;
    };
    let skill = Skill::RemoveTrap;
    let band = i32::from(power);
    if roll_skill_band(
        state,
        actor,
        skill,
        crate::SkillBand::new(band, band + TRAP_BAND_WIDTH),
    ) {
        state.registry.remove::<Trap>(target);
        state.localized_message(actor, TRAP_REMOVED, "");
    } else {
        // A failure is not a spring: ServUO is explicit that you do not set it off.
        state.localized_message(actor, TRAP_NOT_REMOVED, "");
    }
}
