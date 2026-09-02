//! Resisting Spells: what a target does about a spell that has already landed.
//!
//! The skill was in the table, on the trainers' lists and in every saved
//! character sheet, and nothing anywhere read it: a grandmaster warder took a
//! flamestrike exactly as hard as a mage in a robe. This is the read site.
//!
//! ServUO's `Spell.CheckResisted`, pre-AoS shape. Two things happen in one call,
//! and both are the reference's:
//!
//! 1. **The roll.** A resist is not a shield — it does not stop the spell, it
//!    takes a quarter off what the spell does (the caller applies that; see
//!    [`RESIST_NUMERATOR`]). The chance is the *better* of two readings of the
//!    skill, halved: a flat `resist / 5`, which is what a low-circle spell meets,
//!    and a contested one that weighs the caster's Magery and the circle against
//!    the same skill, which is what makes an eighth-circle spell land on people a
//!    first-circle one would not.
//! 2. **The training.** Being cast at is how the skill is learned, and only while
//!    the spell is hard enough to be worth learning from — ServUO stops offering a
//!    gain above `(1 + circle) * 10 + (1 + circle / 6) * 25`, so a grandmaster
//!    cannot train on first-circle spam.
//!
//! Everything is in tenths of a skill point (`1000` is a grandmaster) and tenths
//! of a per-cent, because the reference works in halves and fifths of a percent
//! and rounding those to whole per-cent would quietly move the curve.

use openshard_entities::EntityId;
use openshard_state::{
    Skill,
    WorldState,
};

use crate::spells::SpellCircle;

/// The skill a target rolls to shrug a spell off, and trains by being cast at.
pub const RESIST_SKILL: Skill = Skill::MagicResist;

/// What survives a resist, over [`RESIST_DENOMINATOR`] — ServUO's `damage *= 0.75`
/// on the resisted branch of every pre-AoS attack spell, and the same three
/// quarters a resisted duration keeps.
pub const RESIST_NUMERATOR: u32 = 3;
/// The denominator of [`RESIST_NUMERATOR`].
pub const RESIST_DENOMINATOR: u32 = 4;

/// "You feel yourself resisting magical energy." — the line ServUO's attack spells
/// send on the resisted branch, and the only sign a player has that the skill did
/// anything.
pub const RESISTED_MESSAGE: openshard_protocol::wire::ClilocId = openshard_protocol::wire::ClilocId(501_783);

/// A whole certainty, in tenths of a per-cent — the scale [`resist_chance`] answers
/// on and the range the roll is drawn from.
///
/// Shared with [`crate::dispel`], which answers on the same scale and draws from the
/// same range: two spell rolls that read "out of a thousand" differently would be
/// two different meanings for one number.
pub(crate) const CERTAIN: u32 = 1000;

/// The chance `target` resists a spell of `circle` cast by `caster`, in tenths of a
/// per-cent (so `1000` is certain).
///
/// `max(resist / 5, resist - ((magery - 20) / 5 + (1 + circle) * 5)) / 2`, with both
/// skills in points — evaluated here in tenths throughout so the fifths and the
/// halving stay exact. The second reading goes negative against a strong caster,
/// which is why the maximum of the two is taken rather than the sum: the flat
/// fifth is the floor nobody drops below.
#[must_use]
pub fn resist_chance(state: &WorldState, caster: EntityId, target: EntityId, circle: SpellCircle) -> u32 {
    let resist = i64::from(openshard_skills::skill_value(state, target, RESIST_SKILL));
    let magery = i64::from(openshard_skills::skill_value(state, caster, crate::MAGERY_SKILL));
    // Zero-based, as ServUO's `SpellCircle` enum is: the first circle contributes
    // one step, not two.
    let zero_based = i64::from(circle.get() - SpellCircle::MIN);
    // Both readings in tenths of a per-cent, before the halving.
    let flat = resist / 5;
    let contested = resist - ((magery - 200) / 5 + (1 + zero_based) * 50);
    let chance = flat.max(contested) / 2;
    chance.clamp(0, i64::from(CERTAIN)) as u32
}

/// Roll [`resist_chance`], and let the target learn from having been cast at.
///
/// The gain is offered before the roll and regardless of how it goes — ServUO
/// trains on the attempt, not on the success — but only while the spell is above
/// the target's skill, `(1 + circle) * 10 + (1 + circle / 6) * 25` points. Rolled
/// against the tick's own seeded generator, so a resisted flamestrike replays as
/// one.
#[must_use]
pub fn check_resisted(
    state: &mut WorldState,
    caster: EntityId,
    target: EntityId,
    circle: SpellCircle,
) -> bool {
    let chance = resist_chance(state, caster, target, circle);
    let zero_based = u16::from(circle.get() - SpellCircle::MIN);
    // In tenths, as every skill value here is.
    let ceiling = ((1 + zero_based) * 10 + (1 + zero_based / 6) * 25) * 10;
    if openshard_skills::skill_value(state, target, RESIST_SKILL) < ceiling {
        // The band is the whole scale: what decides the gain is the skill's own
        // distance from its cap, the same call meditation makes.
        let _ = openshard_skills::roll_skill_band(
            state,
            target,
            RESIST_SKILL,
            openshard_skills::SkillBand::new(0, 1000),
        );
    }
    if chance == 0 {
        return false;
    }
    state.rng.below(CERTAIN) < chance
}

/// What a resisted spell is left with — three quarters, rounded down, of whatever
/// the spell would have done. One helper for damage and duration alike (hence the
/// widest of the two types), so the two cannot drift apart.
#[must_use]
pub fn resisted(value: u64) -> u64 {
    value * u64::from(RESIST_NUMERATOR) / u64::from(RESIST_DENOMINATOR)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one thing here that needs no world. The chance itself is pinned in
    /// `world`'s suite, against a live mobile with a real skill sheet, so that the
    /// test reads the formula rather than restating it.
    #[test]
    fn a_resisted_spell_keeps_three_quarters() {
        assert_eq!(resisted(28), 21);
        assert_eq!(resisted(1), 0, "a one-point spell resisted is nothing");
    }
}
