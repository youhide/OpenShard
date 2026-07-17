//! The arithmetic of a skill check: the chance a use succeeds, and the chance it
//! teaches something.
//!
//! Pure functions, no randomness — the roll happens in the tick, against the
//! world's [`Rng`](openshard_state::rng::Rng). Keeping the curves here, separate and
//! total, is what lets them be tested against exact numbers rather than a
//! sampled distribution.

/// The most a skill can be, in tenths: 100.0.
pub const SKILL_CAP: u16 = 1000;

/// The bell curve's 25%-point, in tenths. Ten skill points either side of the
/// difficulty moves the chance a quarter of the way — Sphere's `SKILL_VARIANCE`.
const SKILL_VARIANCE: i32 = 100;

/// The gain chance at zero skill, in per-mille. A starting number, not Sphere's
/// per-skill `AdvRate` tables — those are data-driven config, a later refinement.
const GAIN_AT_ZERO: u32 = 300;

/// The chance, in per-mille (0–1000), that a use of `skill` against `difficulty`
/// succeeds.
///
/// `skill` is in tenths (0–1000); `difficulty` is 0–100, the level at which an
/// equal skill has an even chance. Ported from Sphere's `Skill_CheckSuccess`:
/// an S-curve of the gap between the two, 50% when they match, ~75% ten points
/// ahead, ~25% ten behind.
pub fn success_chance(skill: u16, difficulty: u16) -> u32 {
    let gap = i32::from(skill) - i32::from(difficulty) * 10;
    s_curve(gap, SKILL_VARIANCE) as u32
}

/// The chance, in per-mille, that a use of `skill` gains a tenth of a point.
///
/// Falls from [`GAIN_AT_ZERO`] at nothing to zero at the cap — the higher you
/// are, the less each use teaches. Independent of difficulty for now; Sphere's
/// "you learn only from a challenge" (its `GainRadius`) is a refinement that
/// wants the difficulty passed through, and is noted for when crafting needs it.
pub fn gain_chance(skill: u16) -> u32 {
    let skill = skill.min(SKILL_CAP);
    GAIN_AT_ZERO * u32::from(SKILL_CAP - skill) / u32::from(SKILL_CAP)
}

/// Sphere's `Calc_GetSCurve`: the bell curve, mirrored so a positive gap (skill
/// above difficulty) reads as a *high* chance rather than a narrow one.
fn s_curve(gap: i32, variance: i32) -> i32 {
    let chance = bell_curve(gap, variance);
    if gap > 0 {
        1000 - chance
    } else {
        chance
    }
}

/// Sphere's `Calc_GetBellCurve`: 500 at the centre, halving every `variance`
/// step out, so a use far from your level is near-certain one way or the other.
fn bell_curve(gap: i32, variance: i32) -> i32 {
    if variance <= 0 {
        return 500;
    }
    let mut diff = gap.abs();
    let mut chance = 500;
    while diff > variance && chance != 0 {
        chance /= 2;
        diff -= variance;
    }
    chance - (chance / 2 * diff / variance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_even_match_is_a_coin_toss() {
        // Skill 50.0 against difficulty 50: the gap is zero, the chance 50%.
        assert_eq!(success_chance(500, 50), 500);
    }

    #[test]
    fn ten_points_ahead_is_three_in_four() {
        // +10.0 skill over the difficulty is one variance step: 75%.
        assert_eq!(success_chance(600, 50), 750);
        // And ten behind is one in four.
        assert_eq!(success_chance(400, 50), 250);
    }

    #[test]
    fn far_ahead_is_all_but_certain() {
        // A grandmaster at a trivial task: the chance saturates at 1000, an
        // always-succeeds the roll can never beat.
        assert_eq!(success_chance(1000, 0), 1000);
    }

    #[test]
    fn far_behind_is_all_but_hopeless() {
        assert_eq!(success_chance(0, 100), 0);
    }

    #[test]
    fn gain_falls_as_skill_rises() {
        assert_eq!(gain_chance(0), GAIN_AT_ZERO);
        assert_eq!(gain_chance(SKILL_CAP), 0);
        assert!(gain_chance(500) < gain_chance(100));
    }
}
