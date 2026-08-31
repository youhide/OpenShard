//! How long each kind of combat action takes, as a share of what the weapon
//! table and the era's swing formula come to on their own.
//!
//! The third table keyed by *kind* rather than by weapon, beside
//! [`action_rules`](crate::action_rules) and
//! [`action_stages`](crate::action_stages), and it is one for their reason:
//! *"archery on this shard is quicker than the reference makes it"* is a
//! sentence an operator writes once, where the same statement spread across
//! fifty weapon rows is fifty places for two of them to disagree.
//!
//! Two properties are load-bearing:
//!
//! - **It scales the derivation, it does not replace it.** Dexterity and the
//!   weapon still decide the interval; this says what fraction of that answer
//!   this shard runs at. A setting that named milliseconds outright would have
//!   made every bow, every archer and every point of dexterity the same speed,
//!   which is a different game rather than a faster one.
//! - **An explicit [`SwingSpeed`](crate::components::SwingSpeed) is not
//!   scaled.** That component is a script pinning an exact cadence on one
//!   creature — it is already the last word about that mobile, and a percentage
//!   applied on top would mean a script could no longer say what it says.

use openshard_config::ActionSpeedsConfig;

use crate::components::ActionKind;

/// The whole table, keyed by what the action is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ActionSpeeds {
    /// A blow's pace, as a percentage of the derived interval.
    pub swing:  u16,
    /// A shot's.
    pub shot:   u16,
    /// An innate ranged attack's.
    pub breath: u16,
}

impl ActionSpeeds {
    /// What this build ships with: blows and breaths at the formula's own pace,
    /// and a bow at roughly two thirds of it.
    ///
    /// The shot is the one row that is not `100`, and the reason is a measured
    /// one rather than a preference. The pre-AoS column gives a bow at default
    /// dexterity two and a half seconds between arrows, and no integer in that
    /// column lands on a rounder number; `64` is what turns 2.5s into 1.6s.
    /// Every other bow and every other dexterity scales with it, because this
    /// multiplies the derivation instead of overriding it.
    #[must_use]
    pub const fn shipped() -> Self {
        Self {
            swing:  100,
            shot:   64,
            breath: 100,
        }
    }

    /// The table as the operator wrote it.
    #[must_use]
    pub const fn from_config(speeds: &ActionSpeedsConfig) -> Self {
        Self {
            swing:  speeds.swing,
            shot:   speeds.shot,
            breath: speeds.breath,
        }
    }

    /// The row for one kind of action.
    #[must_use]
    pub const fn row(self, kind: ActionKind) -> u16 {
        match kind {
            ActionKind::Swing { .. } => self.swing,
            ActionKind::Shot { .. } => self.shot,
            ActionKind::Breath { .. } => self.breath,
        }
    }

    /// `ticks` at this kind's pace.
    ///
    /// Floored at one tick, not at zero: an action that lands on the tick it was
    /// committed on has no interval for a bar to measure, no frame for an
    /// animation and no moment in which anything could spoil it. Config refuses
    /// a percentage of zero outright; this is the floor for the case a very fast
    /// weapon and a low percentage produce together.
    #[must_use]
    pub const fn scale(self, kind: ActionKind, ticks: u64) -> u64 {
        let scaled = ticks * self.row(kind) as u64 / 100;
        if scaled == 0 { 1 } else { scaled }
    }
}

#[cfg(test)]
mod tests {
    use openshard_config::ActionSpeedsConfig;
    use openshard_protocol::wire::Graphic;
    use openshard_protocol::world::RangedRange;

    use super::ActionSpeeds;
    use crate::components::ActionKind;

    const REACH: RangedRange = match RangedRange::new(10) {
        Some(reach) => reach,
        None => panic!("ten tiles is a reach"),
    };

    fn shot() -> ActionKind {
        ActionKind::Shot {
            reach:  REACH,
            nocked: Graphic(0x0F3F),
            art:    Graphic(0x0F42),
        }
    }

    /// The shipped table is written twice — once in the operator's vocabulary
    /// and once in the systems' — and a shard that ships one and documents the
    /// other is a bug nobody can see from either file alone.
    #[test]
    fn the_shipped_table_and_the_shipped_config_are_the_same_table() {
        assert_eq!(
            ActionSpeeds::from_config(&ActionSpeedsConfig::shipped()),
            ActionSpeeds::shipped(),
        );
    }

    /// The number the shipped row was chosen for, asserted rather than left in a
    /// comment: a bow at default dexterity is a shot every 1.6 seconds, which at
    /// 40 ticks a second is 64 of them.
    #[test]
    fn the_shipped_shot_turns_a_default_bow_into_a_one_and_a_half_second_shot() {
        assert_eq!(ActionSpeeds::shipped().scale(shot(), 100), 64);
    }

    /// A blow is untouched, which is what `100` has to mean or the table is a
    /// tax on every weapon that did not ask for one.
    #[test]
    fn a_hundred_percent_is_the_derivation_itself() {
        let swing = ActionKind::Swing { reach: REACH };
        assert_eq!(ActionSpeeds::shipped().scale(swing, 57), 57);
    }

    /// Never zero. An action that lands on its own commit tick is not a fast
    /// action, it is one nothing can draw, measure or spoil.
    #[test]
    fn a_scaled_interval_never_reaches_zero() {
        let speeds = ActionSpeeds {
            swing:  100,
            shot:   1,
            breath: 100,
        };
        assert_eq!(speeds.scale(shot(), 1), 1);
        assert_eq!(speeds.scale(shot(), 50), 1);
    }
}
