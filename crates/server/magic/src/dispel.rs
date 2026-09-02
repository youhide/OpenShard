//! Dispel: whether a summoned creature goes when a mage tells it to.
//!
//! The three spells that ask — Dispel, Mass Dispel and Dispel Field — all reduce
//! to one question the summon slice already answered: *is this thing summoned*.
//! [`Summoned`](openshard_state::components::Summoned) is the marker, its `kind`
//! names the row of [`openshard_state::summon`], and that row carries the two
//! numbers this file turns into a chance.
//!
//! ServUO's `DispelSpell` and `MassDispelSpell` share one line of arithmetic:
//!
//! ```text
//! chance = 0.5 + (Magery - GetDispelDifficulty()) / (DispelFocus * 2)
//! ```
//!
//! which is read most easily backwards: at exactly the creature's difficulty the
//! roll is even, and the focus says how many skill points either side of that it
//! takes to make it certain or hopeless. A blade spirit's `0.0 / 20.0` means
//! anyone with any Magery at all sends it away; a daemon's `125.0 / 45.0` sits
//! above a grandmaster's whole skill, so even a master loses that roll more often
//! than not.
//!
//! Everything is in tenths — of a skill point going in, of a per-cent coming out —
//! for [`crate::resist`]'s reason: the reference works in halves and quarters, and
//! rounding those to whole per-cent would quietly move the curve.

use openshard_entities::EntityId;
use openshard_state::WorldState;
use openshard_state::components::SummonKind;
use openshard_state::summon::summoned;

use crate::resist::CERTAIN;

/// How far Mass Dispel reaches from the aimed spot, in tiles — ServUO's
/// `GetMobilesInRange(p, 8)`.
///
/// Four times the radius of an area *damage* spell ([`crate::AREA_RADIUS`]) and
/// deliberately not the same constant: this is a seventh-circle spell whose whole
/// point is to clear a field of somebody else's summons, and it hurts nothing that
/// is not one.
pub const MASS_DISPEL_RANGE: u32 = 8;

/// An even bet, in tenths of a per-cent — where the curve sits when the caster's
/// Magery is exactly the creature's difficulty.
const EVEN: i64 = CERTAIN as i64 / 2;

/// The chance `caster` dispels a summon of `kind`, in tenths of a per-cent (so
/// `1000` is certain).
///
/// Both of the creature's numbers come from its own row, and the caster brings
/// nothing but Magery: no reagent, no circle and no Resisting Spells enter it —
/// ServUO's Dispel does not call `CheckResisted`, because a summon is not shrugging
/// the spell off, it is being unmade.
#[must_use]
pub fn dispel_chance(state: &WorldState, caster: EntityId, kind: SummonKind) -> u32 {
    let data = summoned(kind);
    let magery = i64::from(openshard_skills::skill_value(state, caster, crate::MAGERY_SKILL));
    let difficulty = i64::from(data.difficulty);
    // The table forbids a zero focus and a test holds it to that; the floor here is
    // belt and braces around a division, not a second opinion about the data.
    let focus = i64::from(data.focus).max(1);
    let chance = EVEN + i64::from(CERTAIN) * (magery - difficulty) / (2 * focus);
    chance.clamp(0, i64::from(CERTAIN)) as u32
}

/// Roll [`dispel_chance`] on the tick's own seeded generator, so a dispelled
/// daemon replays as one.
///
/// Nothing is trained by it: Magery is trained by the *cast*, which has already
/// rolled its band by the time this is asked, and the creature has no skill of its
/// own in this — the difficulty is the class's, not something it learnt.
#[must_use]
pub fn check_dispelled(state: &mut WorldState, caster: EntityId, kind: SummonKind) -> bool {
    let chance = dispel_chance(state, caster, kind);
    if chance == 0 {
        return false;
    }
    state.rng.below(CERTAIN) < chance
}
