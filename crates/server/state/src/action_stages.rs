//! How a running action's interval divides into the stretches a watcher is told
//! about — the second half of the answer to *"what is that fighter doing right
//! now?"*.
//!
//! A phase says whether an action is waiting or landing and how long it has; a
//! *stage* says which part of the landing it is in. The two are separate on the
//! wire for the same reason they are separate here: the phase carries the
//! interval a picture is measured against, and a stage changes inside that
//! interval without moving it.
//!
//! Three properties are load-bearing:
//!
//! - **The boundaries are the shard's, never the client's.** A client that
//!   guessed *"past 60% is drawing"* from a percentage would be inventing a fact
//!   the shard never stated, and every shard that retuned the shares would be
//!   drawn wrong by every client. So the server holds the table, computes the
//!   stage, and sends the transition.
//! - **An action never goes back a stage.** A `Slow` rule pushes the impact out
//!   mid-action, which lowers the fraction of the interval that has passed; the
//!   fighter has not un-drawn the bow, so the stage is held rather than rewound.
//!   [`ActionStage`] is ordered for exactly this comparison.
//! - **[`ActionStage::Aim`] is not in this table at all.** Aiming is *holding*,
//!   and a released action holds nothing: its impact is coming whether or not
//!   anybody waits for it. Given a share of the interval it read on screen as a
//!   stretch in which the bow was already bent and the arrow had not left — a
//!   delay with no cause, and it was reported as one. `Aim` is the stage a
//!   [`Phase::Armed`](crate::components::Phase::Armed) action sits in, which is
//!   the only thing on this shard that waits on purpose.

use openshard_config::{ActionStagesConfig, StageSharesConfig};
use openshard_protocol::feedback::ActionStage;

use crate::components::ActionKind;

/// Where one kind of action's stages begin, as shares of its whole interval.
///
/// Two shares and not three: the release is the remainder, so no table can
/// describe an action that finishes getting ready and then has nothing left to
/// land in. `config` validates that the two do not pass a hundred, so the
/// arithmetic here has no failure case.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StageShares {
    /// The share spent bringing the weapon up.
    pub ready: u8,
    /// The share spent on the effort — the bow bending, the arm cocking.
    pub load: u8,
}

impl StageShares {
    /// One row as the operator wrote it.
    #[must_use]
    pub const fn from_config(shares: &StageSharesConfig) -> Self {
        Self {
            ready: shares.ready,
            load: shares.load,
        }
    }

    /// Which stretch an action is in, given how much of its interval has gone.
    ///
    /// `elapsed_percent` is clamped by the caller's own arithmetic rather than
    /// here: a hundred and over is the release, which is what an action past its
    /// own impact is in anyway.
    ///
    /// A share of zero is a stage this kind skips, and skipping is the right
    /// behaviour rather than a degenerate one: a shard whose blow is all strike
    /// and no wind-up writes a zero there and never sends that transition.
    ///
    /// [`ActionStage::Aim`] is never returned. See the module header: holding is
    /// an armed action's, and this walks the interval of a released one.
    #[must_use]
    pub const fn stage_at(self, elapsed_percent: u16) -> ActionStage {
        let ready = self.ready as u16;
        let load = ready + self.load as u16;
        if elapsed_percent < ready {
            ActionStage::Ready
        } else if elapsed_percent < load {
            ActionStage::Load
        } else {
            ActionStage::Release
        }
    }
}

/// The whole table, keyed by what the action is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ActionStages {
    /// How a blow divides.
    pub swing: StageShares,
    /// How a shot divides.
    pub shot: StageShares,
    /// How an innate ranged attack divides.
    pub breath: StageShares,
}

impl ActionStages {
    /// What this build ships with, and the three sentences it is meant to read
    /// as: **a blow is mostly its wind-up, a shot is mostly its draw, and a
    /// breath is mostly the filling of the lungs.**
    ///
    /// Every field of every row is written out — no update syntax over a base —
    /// for [`ActionRules::shipped`](crate::action_rules::ActionRules::shipped)'s
    /// reason: a stage added to the table later must be decided per kind here
    /// rather than quietly inherit a zero nobody re-read.
    #[must_use]
    pub const fn shipped() -> Self {
        Self {
            swing: StageShares { ready: 15, load: 65 },
            shot: StageShares { ready: 10, load: 70 },
            breath: StageShares { ready: 20, load: 60 },
        }
    }

    /// The table as the operator wrote it.
    #[must_use]
    pub fn from_config(stages: &ActionStagesConfig) -> Self {
        Self {
            swing: StageShares::from_config(&stages.swing),
            shot: StageShares::from_config(&stages.shot),
            breath: StageShares::from_config(&stages.breath),
        }
    }

    /// The row for one kind of action.
    #[must_use]
    pub const fn row(self, kind: ActionKind) -> StageShares {
        match kind {
            ActionKind::Swing { .. } => self.swing,
            ActionKind::Shot { .. } => self.shot,
            ActionKind::Breath { .. } => self.breath,
        }
    }

    /// Which stretch an action of this kind is in, that far through its
    /// interval.
    #[must_use]
    pub const fn stage_at(self, kind: ActionKind, elapsed_percent: u16) -> ActionStage {
        self.row(kind).stage_at(elapsed_percent)
    }
}

#[cfg(test)]
mod tests {
    use super::{ActionStages, StageShares};
    use openshard_config::ActionStagesConfig;
    use openshard_protocol::feedback::ActionStage;

    /// The shipped table is written twice — once in the operator's vocabulary
    /// and once in the systems' — and a shard that ships one and documents the
    /// other is a bug nobody can see from either file alone.
    #[test]
    fn the_shipped_table_and_the_shipped_config_are_the_same_table() {
        assert_eq!(
            ActionStages::from_config(&ActionStagesConfig::shipped()),
            ActionStages::shipped(),
        );
    }

    /// The boundaries are half-open upwards: a share's last percent belongs to
    /// it and the next percent starts the following stage. Asserted at every
    /// edge, because an off-by-one here is a stage that is announced one tick
    /// early for every action on the shard.
    #[test]
    fn each_share_owns_its_own_stretch_and_the_release_takes_the_rest() {
        let shares = StageShares { ready: 10, load: 70 };
        assert_eq!(shares.stage_at(0), ActionStage::Ready);
        assert_eq!(shares.stage_at(9), ActionStage::Ready);
        assert_eq!(shares.stage_at(10), ActionStage::Load);
        assert_eq!(shares.stage_at(79), ActionStage::Load);
        assert_eq!(shares.stage_at(80), ActionStage::Release);
        assert_eq!(shares.stage_at(100), ActionStage::Release);
        assert_eq!(
            shares.stage_at(500),
            ActionStage::Release,
            "an action past its own impact is still landing, not rewound"
        );
    }

    /// A zero share is a stage this kind does not have, and the walk skips it
    /// rather than reporting a stretch of no length.
    #[test]
    fn a_zero_share_is_a_stage_the_kind_skips() {
        let all_release = StageShares { ready: 0, load: 0 };
        assert_eq!(all_release.stage_at(0), ActionStage::Release);

        let no_lift = StageShares { ready: 0, load: 80 };
        assert_eq!(no_lift.stage_at(0), ActionStage::Load);
        assert_eq!(no_lift.stage_at(79), ActionStage::Load);
        assert_eq!(no_lift.stage_at(80), ActionStage::Release);
    }

    /// The stretch this table does *not* have, asserted so that nobody puts it
    /// back by widening the walk. Aiming is holding, holding is an armed
    /// action's, and `combat::advance_stage` is the one place it is entered —
    /// a share of a running interval spent "aimed" is a delay with no cause, and
    /// that is what it read as when there was one.
    #[test]
    fn no_share_of_a_running_interval_is_ever_the_aim() {
        let shares = StageShares { ready: 10, load: 70 };
        for percent in 0..=200 {
            assert_ne!(
                shares.stage_at(percent),
                ActionStage::Aim,
                "{percent}% of a released action's interval reported an aim"
            );
        }
    }
}
