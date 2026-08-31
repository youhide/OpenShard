//! What the world does to a combat action while it runs — D4 and D5 of
//! `docs/combat_actions.md`.
//!
//! A rule is a **pair**: a condition the server already knows, and an effect on
//! the action running at the moment it becomes true. A boolean *"movement breaks
//! this"* on the weapon cannot express shooting on the move at a penalty, which
//! is a thing this shard wants, so the pair is data — the table is an operator
//! setting, and no code decides what running does to a bow.
//!
//! Two properties are load-bearing and neither is obvious from the types:
//!
//! - **A condition is pushed from where it is already known, never polled.** The
//!   step seam has `running` and the `Riding` lookup in hand; `damage` is the one
//!   door every wound passes. Reading a mobile's `Heading` in a sustain pass
//!   instead would be wrong and subtly so — the run bit persists in the facing
//!   after the step, so a fighter who ran once would sway forever.
//! - **A condition applies at most once to any one action.** A draw lasting ten
//!   seconds takes twenty steps, and a sway charged per step would put an
//!   archer's chance at zero for walking across a room — while a `Slow` charged
//!   per step would push the impact out faster than it approaches, and the shot
//!   would never be taken at all. So a rule is a fact *about the action* — "it
//!   ran", "it was struck", which is how [`ActorCondition::Struck`] is worded to
//!   begin with — and [`ConditionSet`] on the action is what remembers. The
//!   per-tick spender in the model is `Drain`, which is levied by the sustain
//!   pass against a held condition rather than pushed at an event, and is Ф5.

use openshard_config::{
    ActionEffectConfig,
    ActionRulesConfig,
    ConditionRulesConfig,
};
use openshard_protocol::feedback::InterruptReason;

use crate::components::ActionKind;

/// Something true of the fighter that a rule may key on.
///
/// A closed list for the same reason `Watch` is one: each is a fact the server
/// already computes at a seam it already runs, and a condition nobody can point
/// at a seam for is one nobody can cost.
///
/// `Winded` — below the stamina threshold that already makes a step cost extra —
/// belongs here and is Ф5's, which is the phase that gives combat a stamina
/// spender to read it from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ActorCondition {
    /// This tick's step carried `Facing::running`.
    Running,
    /// A step that was not a run.
    Walking,
    /// The fighter is on a mount (`Riding`). Pushed at the step, with the pace,
    /// because that is the seam that has the mount in hand — a rider standing
    /// still is not doing anything a rule has an opinion about.
    Mounted,
    /// Took damage since the action began.
    Struck,
    /// The line of sight to the committed target is gone.
    Blinded,
}

impl ActorCondition {
    /// The name an interruption by this condition ends under.
    ///
    /// Three of them share [`InterruptReason::Moved`]: all three are facts of a
    /// step, and what the watcher is being told is that the fighter moved, not
    /// which of the three ways it was moving.
    #[must_use]
    pub const fn interrupt_reason(self) -> InterruptReason {
        match self {
            Self::Running | Self::Walking | Self::Mounted => InterruptReason::Moved,
            Self::Struck => InterruptReason::Struck,
            Self::Blinded => InterruptReason::NoLineOfSight,
        }
    }

    /// Its place in a [`ConditionSet`].
    const fn bit(self) -> u8 {
        match self {
            Self::Running => 1,
            Self::Walking => 1 << 1,
            Self::Mounted => 1 << 2,
            Self::Struck => 1 << 3,
            Self::Blinded => 1 << 4,
        }
    }
}

/// Which conditions have already been applied to one action.
///
/// Small enough to live on the component, which is where it has to live: the
/// answer is per action and dies with it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ConditionSet(u8);

impl ConditionSet {
    /// Nothing has happened to this action yet.
    pub const EMPTY: Self = Self(0);

    /// Whether this condition has already been charged against the action.
    #[must_use]
    pub const fn contains(self, condition: ActorCondition) -> bool {
        self.0 & condition.bit() != 0
    }

    /// The same set with this condition charged.
    #[must_use]
    pub const fn with(self, condition: ActorCondition) -> Self {
        Self(self.0 | condition.bit())
    }
}

/// What a condition does to the action it catches.
///
/// `Drain { stamina }` — fatigue per tick while a condition holds, owed at the
/// impact — is the fourth of these and is Ф5's, together with the `owed_stamina`
/// on the action that would receive it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ActionEffect {
    /// The action ends, interrupted, naming the condition that spoiled it.
    Break,
    /// The impact is pushed out by this percentage of the time still to run. A
    /// hundred doubles what is left; nothing is pushed once there is nothing
    /// left to push.
    Slow { percent: u16 },
    /// Taken off the hit roll when the action resolves, as a signed percentage
    /// of the base chance — the same scale an ambush's bonus is on. A *negative*
    /// penalty steadies, which is how "an archer steadies on horseback" is
    /// written.
    Sway { penalty: i16 },
}

impl ActionEffect {
    /// The effect as the operator wrote it. `config` validates the numbers — a
    /// slow past [`MAX_SLOW_PERCENT`](openshard_config::MAX_SLOW_PERCENT) never
    /// reaches a running shard — so this is a rename and nothing more.
    #[must_use]
    pub const fn from_config(effect: ActionEffectConfig) -> Self {
        match effect {
            ActionEffectConfig::Break => Self::Break,
            ActionEffectConfig::Slow { percent } => Self::Slow { percent },
            ActionEffectConfig::Sway { penalty } => Self::Sway { penalty },
        }
    }
}

/// One kind of action's rules: an effect per condition, or none for a condition
/// that kind does not care about.
///
/// [`None`] here is a rule saying "this does nothing", which is a real answer —
/// walking is *free* for an archer on this shard, not unspecified.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ConditionEffects {
    /// What a run does.
    pub running: Option<ActionEffect>,
    /// What a walk does.
    pub walking: Option<ActionEffect>,
    /// What being mounted does, charged at the step.
    pub mounted: Option<ActionEffect>,
    /// What a wound taken mid-action does.
    pub struck:  Option<ActionEffect>,
    /// What losing the line to the target does.
    pub blinded: Option<ActionEffect>,
}

impl ConditionEffects {
    /// No rule at all — every condition passes through.
    #[must_use]
    pub const fn free() -> Self {
        Self {
            running: None,
            walking: None,
            mounted: None,
            struck:  None,
            blinded: None,
        }
    }

    /// One row as the operator wrote it.
    ///
    /// A condition the row leaves out is no rule — deliberately not the shipped
    /// default, so what an operator reads in the file is the whole of what the
    /// shard runs for that kind.
    #[must_use]
    pub fn from_config(row: &ConditionRulesConfig) -> Self {
        Self {
            running: row.running.map(ActionEffect::from_config),
            walking: row.walking.map(ActionEffect::from_config),
            mounted: row.mounted.map(ActionEffect::from_config),
            struck:  row.struck.map(ActionEffect::from_config),
            blinded: row.blinded.map(ActionEffect::from_config),
        }
    }

    /// The effect this kind of action suffers from that condition.
    #[must_use]
    pub const fn of(self, condition: ActorCondition) -> Option<ActionEffect> {
        match condition {
            ActorCondition::Running => self.running,
            ActorCondition::Walking => self.walking,
            ActorCondition::Mounted => self.mounted,
            ActorCondition::Struck => self.struck,
            ActorCondition::Blinded => self.blinded,
        }
    }
}

/// The whole table, keyed by what the action is — a blow, a shot, a breath.
///
/// Keyed by kind and not by weapon on purpose: what a run does to *a shot* is
/// one line an operator can read, where a column on every ranged row is fifty
/// places for two of them to disagree. A weapon that wants its own answer is
/// what Ф6's reach column is for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ActionRules {
    /// A blow's rules.
    pub swing:  ConditionEffects,
    /// A shot's rules.
    pub shot:   ConditionEffects,
    /// An innate ranged attack's rules.
    pub breath: ConditionEffects,
}

impl ActionRules {
    /// How much of the base hit chance a shot loosed at a run gives up.
    ///
    /// The same scale, and near enough the same size, as the bonus an ambush
    /// from cover already carries: running with a bow is worth about what
    /// stalking with a knife is.
    pub const RUNNING_SHOT_SWAY: i16 = 25;

    /// What this build ships with, and the three sentences it is meant to read
    /// as: **walking is free, running sways a shot, and a mount is neutral.**
    ///
    /// The fourth is not a choice so much as today's behaviour written down: a
    /// line that is cut ends the action, which is what the sustain pass did with
    /// a bare `NoLineOfSight` before there was a table to route it through. A
    /// shard that wants a fighter to keep swinging into the dark now says so in
    /// its config instead of asking for a patch.
    /// Every field of every row is written out — no update syntax over
    /// [`ConditionEffects::free`] — because a condition added to the table later
    /// must be *decided* for each kind here rather than quietly inherit "no
    /// rule" from a base value nobody re-read.
    #[must_use]
    pub const fn shipped() -> Self {
        Self {
            swing:  ConditionEffects {
                running: None,
                walking: None,
                mounted: None,
                struck:  None,
                blinded: Some(ActionEffect::Break),
            },
            shot:   ConditionEffects {
                running: Some(ActionEffect::Sway {
                    penalty: Self::RUNNING_SHOT_SWAY,
                }),
                walking: None,
                mounted: None,
                struck:  None,
                blinded: Some(ActionEffect::Break),
            },
            breath: ConditionEffects {
                running: None,
                walking: None,
                mounted: None,
                struck:  None,
                blinded: Some(ActionEffect::Break),
            },
        }
    }

    /// The table as the operator wrote it.
    #[must_use]
    pub fn from_config(rules: &ActionRulesConfig) -> Self {
        Self {
            swing:  ConditionEffects::from_config(&rules.swing),
            shot:   ConditionEffects::from_config(&rules.shot),
            breath: ConditionEffects::from_config(&rules.breath),
        }
    }

    /// The row for one kind of action.
    #[must_use]
    pub const fn row(self, kind: ActionKind) -> ConditionEffects {
        match kind {
            ActionKind::Swing { .. } => self.swing,
            ActionKind::Shot { .. } => self.shot,
            ActionKind::Breath { .. } => self.breath,
        }
    }

    /// What happens to an action of this kind when that condition catches it.
    #[must_use]
    pub const fn effect(self, kind: ActionKind, condition: ActorCondition) -> Option<ActionEffect> {
        self.row(kind).of(condition)
    }
}

#[cfg(test)]
mod tests {
    use openshard_config::ActionRulesConfig;

    use super::{
        ActionRules,
        ActorCondition,
    };

    /// The shipped table is written twice — once in the operator's vocabulary
    /// and once in the systems' — and the two halves are read by different
    /// people at different times. A shard that ships one default and documents
    /// the other is a bug nobody can see from either file alone.
    #[test]
    fn the_shipped_table_and_the_shipped_config_are_the_same_table() {
        assert_eq!(
            ActionRules::from_config(&ActionRulesConfig::shipped()),
            ActionRules::shipped(),
        );
    }

    /// The three sentences `shipped` is meant to read as, asserted rather than
    /// described: walking is free, running sways a shot, a mount is neutral.
    #[test]
    fn walking_is_free_running_sways_a_shot_and_a_mount_is_neutral() {
        use openshard_protocol::world::RangedRange;

        use crate::components::ActionKind;

        let rules = ActionRules::shipped();
        let shot = ActionKind::Shot {
            reach:  RangedRange::new(10).unwrap(),
            nocked: openshard_protocol::wire::Graphic(0x0F3F),
            art:    openshard_protocol::wire::Graphic(0x0F42),
        };
        assert_eq!(rules.effect(shot, ActorCondition::Walking), None);
        assert_eq!(rules.effect(shot, ActorCondition::Mounted), None);
        assert_eq!(
            rules.effect(shot, ActorCondition::Running),
            Some(super::ActionEffect::Sway {
                penalty: ActionRules::RUNNING_SHOT_SWAY,
            }),
        );
    }
}
