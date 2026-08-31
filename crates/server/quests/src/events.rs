use openshard_protocol::serial::Serial;
use openshard_state::QuestKey;

/// Which objective a quest definition's `objectives` names — the same index a
/// `QuestState::progress` slot is at. Crosses the event bus into scripting
/// (Community Pack content reads it), so a bare `usize` here would be a plain
/// integer at the one boundary where a pack author has nothing else to check
/// it against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ObjectiveIndex(pub usize);

/// A count of credit earned toward a quest objective.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ObjectiveCount(u16);

impl ObjectiveCount {
    pub const fn new(raw: u16) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u16 {
        self.0
    }
}

/// The current and required amounts for one objective.
///
/// These values always travel together: a progress update without its goal is
/// not useful to either the client or a script, and two positional counts made
/// it easy to reverse them while relaying an update through the quest systems.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ObjectiveProgress {
    /// How much credit the player has now.
    pub current: ObjectiveCount,
    /// How much credit completes this objective.
    pub goal:    ObjectiveCount,
}

impl ObjectiveProgress {
    pub const fn new(current: u16, goal: u16) -> Self {
        Self {
            current: ObjectiveCount::new(current),
            goal:    ObjectiveCount::new(goal),
        }
    }

    pub const fn is_complete(self) -> bool {
        self.current.raw() >= self.goal.raw()
    }
}

/// A player accepted a quest.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QuestAccepted {
    /// Who took it.
    pub player: Serial,
    /// Which quest, by its key.
    pub key:    QuestKey,
}

/// A player turned an offered quest down. Nothing was started.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QuestRefused {
    /// Who refused.
    pub player: Serial,
    /// Which quest, by its key.
    pub key:    QuestKey,
}

/// A player gave up on a quest they had taken.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QuestResigned {
    /// Who resigned.
    pub player: Serial,
    /// Which quest, by its key.
    pub key:    QuestKey,
}

/// An objective moved — a kill counted, an item found, a leg of a journey walked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QuestObjectiveUpdated {
    /// Whose quest.
    pub player:    Serial,
    /// Which quest, by its key.
    pub key:       QuestKey,
    /// Which objective, by its index in the definition.
    pub objective: ObjectiveIndex,
    /// Its current and required amounts.
    pub progress:  ObjectiveProgress,
}

/// A timed quest ran out of time.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QuestFailed {
    /// Whose quest.
    pub player: Serial,
    /// Which quest, by its key.
    pub key:    QuestKey,
}

/// A quest was turned in and paid.
///
/// The pack's hook for anything the core's flat reward list cannot express — a
/// title, a skill, a follow-up quest, a line of dialogue. The core has already
/// paid the declared rewards by the time this is read; a script *adds*, exactly
/// as it does off `CorpseCreated`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QuestCompleted {
    /// Who finished it.
    pub player: Serial,
    /// Which quest, by its key.
    pub key:    QuestKey,
    /// Who it was turned in to, if the giver is still around.
    pub giver:  Option<Serial>,
}

#[cfg(test)]
mod tests {
    use super::{
        ObjectiveCount,
        ObjectiveProgress,
    };

    #[test]
    fn completion_includes_progress_past_the_goal() {
        assert!(!ObjectiveProgress::new(4, 5).is_complete());
        assert!(ObjectiveProgress::new(5, 5).is_complete());
        assert!(ObjectiveProgress::new(6, 5).is_complete());
    }

    #[test]
    fn count_preserves_the_domain_value() {
        assert_eq!(ObjectiveCount::new(42).raw(), 42);
    }
}
