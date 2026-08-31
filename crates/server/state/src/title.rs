//! What a character is *called*, and how the standing behind it is earned —
//! ServUO's `Scripts/Misc/Titles.cs`.
//!
//! The table and the title it yields are data, which is why they sit here with
//! `creature_name` and the body table: the single-click label and the `0xD6` tooltip
//! both read a title.
//!
//! The **awarding** is here too, and it did not start out that way — it lived in
//! `combat`, whose own note said a crate of its own "would depend on combat for its
//! only input". A kill stopped being the only input the moment a skill could cost
//! you karma (Poisoning takes twenty for coating a blade, and it is not the last),
//! and `skills` cannot depend on `combat` because `combat` already depends on it. So
//! standing lives where every crate can reach it, beside the table it feeds.
//!
//! # The curve is the interesting part
//!
//! Awarding is not addition. ServUO subtracts `current / 100` from every offset, in
//! *both* directions, so the same kill is worth less to a famous character than to
//! an unknown one and a fall from grace slows as it deepens. That is what stops fame
//! being a counter of monsters killed. It is ported exactly, including the detail
//! that the reduction applies to a loss as well as a gain — read quickly it looks
//! like a bug, and "fixing" it would make infamy accelerate.

use openshard_entities::EntityId;

use crate::WorldState;
use crate::components::{
    Body,
    Fame,
    Karma,
};

/// One band of the title table: a fame ceiling and the karma bands inside it.
struct FameBand {
    /// The highest fame this band covers.
    fame:  i32,
    /// `(karma ceiling, title)`, in ascending order of karma. `{}` is the name and
    /// `{lord}` the Lord/Lady the top fame band earns.
    karma: &'static [(i32, &'static str)],
}

/// ServUO's `m_FameEntries`, verbatim. Five fame bands of eleven karma bands each.
const TITLES: &[FameBand] = &[
    FameBand {
        fame:  1249,
        karma: &[
            (-10000, "The Outcast {}"),
            (-5000, "The Despicable {}"),
            (-2500, "The Scoundrel {}"),
            (-1250, "The Unsavory {}"),
            (-625, "The Rude {}"),
            (624, "{}"),
            (1249, "The Fair {}"),
            (2499, "The Kind {}"),
            (4999, "The Good {}"),
            (9999, "The Honest {}"),
            (10000, "The Trustworthy {}"),
        ],
    },
    FameBand {
        fame:  2499,
        karma: &[
            (-10000, "The Wretched {}"),
            (-5000, "The Dastardly {}"),
            (-2500, "The Malicious {}"),
            (-1250, "The Dishonorable {}"),
            (-625, "The Disreputable {}"),
            (624, "The Notable {}"),
            (1249, "The Upstanding {}"),
            (2499, "The Respectable {}"),
            (4999, "The Honorable {}"),
            (9999, "The Commendable {}"),
            (10000, "The Estimable {}"),
        ],
    },
    FameBand {
        fame:  4999,
        karma: &[
            (-10000, "The Nefarious {}"),
            (-5000, "The Wicked {}"),
            (-2500, "The Vile {}"),
            (-1250, "The Ignoble {}"),
            (-625, "The Notorious {}"),
            (624, "The Prominent {}"),
            (1249, "The Reputable {}"),
            (2499, "The Proper {}"),
            (4999, "The Admirable {}"),
            (9999, "The Famed {}"),
            (10000, "The Great {}"),
        ],
    },
    FameBand {
        fame:  9999,
        karma: &[
            (-10000, "The Dread {}"),
            (-5000, "The Evil {}"),
            (-2500, "The Villainous {}"),
            (-1250, "The Sinister {}"),
            (-625, "The Infamous {}"),
            (624, "The Renowned {}"),
            (1249, "The Distinguished {}"),
            (2499, "The Eminent {}"),
            (4999, "The Noble {}"),
            (9999, "The Illustrious {}"),
            (10000, "The Glorious {}"),
        ],
    },
    FameBand {
        fame:  10000,
        karma: &[
            (-10000, "The Dread {lord} {}"),
            (-5000, "The Evil {lord} {}"),
            (-2500, "The Dark {lord} {}"),
            (-1250, "The Sinister {lord} {}"),
            (-625, "The Dishonored {lord} {}"),
            (624, "{lord} {}"),
            (1249, "The Distinguished {lord} {}"),
            (2499, "The Eminent {lord} {}"),
            (4999, "The Noble {lord} {}"),
            (9999, "The Illustrious {lord} {}"),
            (10000, "The Glorious {lord} {}"),
        ],
    },
];

/// The name a mobile is known by, title and all — ServUO's `Titles.ComputeFameTitle`.
///
/// The band is the first whose fame ceiling the mobile reaches (or the last), then the
/// first karma ceiling inside it. `female` picks Lady over Lord, which only the top
/// fame band uses.
#[must_use]
pub fn compute_title(name: &str, fame: i32, karma: i32, female: bool) -> String {
    let band = TITLES
        .iter()
        .find(|band| fame <= band.fame)
        .unwrap_or(&TITLES[TITLES.len() - 1]);
    let pattern = band
        .karma
        .iter()
        .find(|&&(ceiling, _)| karma <= ceiling)
        .map_or(band.karma[band.karma.len() - 1].1, |&(_, title)| title);
    pattern
        .replace("{lord}", if female { "Lady" } else { "Lord" })
        .replace("{}", name)
}

/// A mobile's earned name, or its plain one when it has earned nothing.
///
/// ServUO shows a fame title to the mobile itself always and to onlookers only once its
/// fame reaches 5000 (`ShowFameTitle`); below that a stranger reads the bare name. This
/// is the onlooker's view, which is the one every label in the engine draws.
#[must_use]
pub fn titled_name(state: &WorldState, mobile: EntityId, name: &str) -> String {
    let fame = state.registry.get::<Fame>(mobile).map_or(0, |f| f.0);
    if fame < 5000 {
        return name.to_owned();
    }
    let karma = state.registry.get::<Karma>(mobile).map_or(0, |k| k.0);
    let female = state
        .registry
        .get::<Body>(mobile)
        .is_some_and(|body| body.id.0 == 0x0191 || body.id.0 == 0x0193);
    compute_title(name, fame, karma, female)
}

/// ServUO's `Titles.MinFame`/`MaxFame`.
pub const MIN_FAME: i32 = 0;
/// The most fame a character may hold.
pub const MAX_FAME: i32 = 32_000;
/// ServUO's `Titles.MinKarma`/`MaxKarma`.
pub const MIN_KARMA: i32 = -32_000;
/// The most karma a character may hold.
pub const MAX_KARMA: i32 = 32_000;

/// How much of `offset` actually lands, given what is already held.
///
/// ServUO's `AwardFame`/`AwardKarma` share this shape: the offset is reduced by
/// `current / 100` **whichever way it points**, then clamped into range. So a famous
/// character gains little and loses little, and an unknown one swings freely.
fn diminish(current: i32, offset: i32, min: i32, max: i32) -> i32 {
    if offset > 0 {
        if current >= max {
            return 0;
        }
        // Note the sign: ServUO subtracts in both branches. A gain shrinks toward zero.
        (offset - current / 100).max(0).min(max - current)
    } else if offset < 0 {
        if current <= min {
            return 0;
        }
        // And a loss also has `current / 100` subtracted, which for positive `current`
        // makes the loss *bigger* and for negative `current` smaller — so infamy slows
        // as it deepens. Clamped at zero the other way.
        (offset - current / 100).min(0).max(min - current)
    } else {
        0
    }
}

/// Award (or take) fame. Returns what actually landed, for the message.
pub fn award_fame(state: &mut WorldState, mobile: EntityId, offset: i32) -> i32 {
    let current = state.registry.get::<Fame>(mobile).map_or(0, |f| f.0);
    let landed = diminish(current, offset, MIN_FAME, MAX_FAME);
    if landed != 0 {
        state.registry.insert(mobile, Fame(current + landed));
    }
    landed
}

/// Award (or take) karma. Returns what actually landed.
pub fn award_karma(state: &mut WorldState, mobile: EntityId, offset: i32) -> i32 {
    let current = state.registry.get::<Karma>(mobile).map_or(0, |k| k.0);
    let landed = diminish(current, offset, MIN_KARMA, MAX_KARMA);
    if landed != 0 {
        state.registry.insert(mobile, Karma(current + landed));
    }
    landed
}

/// What ServUO tells a player about a change in standing — `1019051..1019066`, as
/// plain text. `None` when nothing landed, so nothing is said.
#[must_use]
pub fn award_message(landed: i32, karma: bool) -> Option<&'static str> {
    let (kind, band) = (if karma { "karma" } else { "fame" }, landed);
    Some(match (kind, band) {
        (_, 0) => return None,
        ("fame", n) if n > 40 => "You have gained a lot of fame.",
        ("fame", n) if n > 20 => "You have gained a good amount of fame.",
        ("fame", n) if n > 10 => "You have gained some fame.",
        ("fame", n) if n > 0 => "You have gained a little fame.",
        ("fame", n) if n < -40 => "You have lost a lot of fame.",
        ("fame", n) if n < -20 => "You have lost a good amount of fame.",
        ("fame", n) if n < -10 => "You have lost some fame.",
        ("fame", _) => "You have lost a little fame.",
        (_, n) if n > 40 => "You have gained a lot of karma.",
        (_, n) if n > 20 => "You have gained a good amount of karma.",
        (_, n) if n > 10 => "You have gained some karma.",
        (_, n) if n > 0 => "You have gained a little karma.",
        (_, n) if n < -40 => "You have lost a lot of karma.",
        (_, n) if n < -20 => "You have lost a good amount of karma.",
        (_, n) if n < -10 => "You have lost some karma.",
        (_, _) => "You have lost a little karma.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_titles_are_servuos() {
        assert_eq!(compute_title("Rowena", 0, 0, false), "Rowena");
        assert_eq!(compute_title("Rowena", 1000, 5000, false), "The Honest Rowena");
        assert_eq!(
            compute_title("Rowena", 1000, -6000, false),
            "The Despicable Rowena"
        );
        assert_eq!(compute_title("Rowena", 9000, 20000, false), "The Glorious Rowena");
    }

    #[test]
    fn only_the_top_band_earns_a_lordship() {
        // `{1}` in ServUO's table, and it appears in exactly one fame band.
        assert_eq!(compute_title("Rowena", 20000, 0, true), "Lady Rowena");
        assert_eq!(compute_title("Rowena", 20000, 0, false), "Lord Rowena");
        assert_eq!(
            compute_title("Rowena", 20000, 20000, true),
            "The Glorious Lady Rowena"
        );
        for karma in [-20000, 0, 20000] {
            assert!(
                !compute_title("Rowena", 9999, karma, true).contains("Lady"),
                "karma {karma}"
            );
        }
    }

    #[test]
    fn a_placeholder_never_survives_into_a_title() {
        for fame in [0, 1249, 2500, 5000, 10000, 32000] {
            for karma in [-32000, -1000, 0, 1000, 32000] {
                let title = compute_title("Rowena", fame, karma, false);
                assert!(!title.contains('{'), "{fame}/{karma}: {title}");
                assert!(title.contains("Rowena"), "{fame}/{karma}: {title}");
            }
        }
    }

    #[test]
    fn the_same_kill_is_worth_less_to_a_famous_character() {
        // The whole point of the curve: without it fame is a counter of monsters killed.
        assert_eq!(diminish(0, 100, MIN_FAME, MAX_FAME), 100);
        assert_eq!(diminish(5000, 100, MIN_FAME, MAX_FAME), 50);
        assert_eq!(diminish(10000, 100, MIN_FAME, MAX_FAME), 0);
    }

    #[test]
    fn a_fall_from_grace_slows_as_it_deepens() {
        // ServUO subtracts `current / 100` from a *loss* too. Read quickly that looks
        // like a bug — it makes a loss bigger while karma is positive — and the reason
        // it is not is the other half: once karma is negative it makes each further
        // loss smaller, so infamy decelerates. Fixing the "bug" would make it run away.
        assert_eq!(diminish(0, -100, MIN_KARMA, MAX_KARMA), -100);
        assert_eq!(diminish(5000, -100, MIN_KARMA, MAX_KARMA), -150);
        assert_eq!(diminish(-5000, -100, MIN_KARMA, MAX_KARMA), -50);
        assert_eq!(diminish(-10000, -100, MIN_KARMA, MAX_KARMA), 0);
    }

    #[test]
    fn nothing_lands_past_the_bounds() {
        assert_eq!(diminish(MAX_FAME, 500, MIN_FAME, MAX_FAME), 0);
        assert_eq!(diminish(MIN_FAME, -500, MIN_FAME, MAX_FAME), 0);
        assert_eq!(diminish(MIN_KARMA, -500, MIN_KARMA, MAX_KARMA), 0);
        // And a partial award never overshoots the ceiling.
        assert_eq!(diminish(MAX_FAME - 10, 5000, MIN_FAME, MAX_FAME), 10);
    }

    #[test]
    fn a_message_is_only_sent_when_something_landed() {
        assert_eq!(award_message(0, false), None);
        assert_eq!(award_message(50, false), Some("You have gained a lot of fame."));
        assert_eq!(award_message(-5, true), Some("You have lost a little karma."));
    }
}
