//! What this client was told about fighting, and when — the recorder behind
//! *"there was a stall right here"*.
//!
//! # Why a recorder and not another assertion
//!
//! Everything the shard says about a fight crosses the wire as an **edge**: an
//! action begins, a stage changes, an outcome lands, a refusal starts or lifts.
//! A stall is the *absence* of an edge, and an absence cannot be looked at. The
//! shard-side oracle (`fight_timeline`, in the world crate) can walk a fight tick
//! by tick and say that no tick was unaccounted for — and it did, and the fight
//! it walked was sound — which leaves exactly the half a server test cannot
//! reach: what a *screen* was holding, on a real shard, at the moment a person
//! decided nothing was happening.
//!
//! So this keeps the last few thousand things this client heard, on this client's
//! own clock — the same clock the bar ages against — and lets the person watching
//! stamp a mark into it. The mark carries a snapshot of what was drawn at that
//! instant, because *"what was on screen when you said that"* is the question,
//! and it is the one thing that is gone by the time anybody reads the log.
//!
//! # What it is not
//!
//! Not a substitute for the shard saying why. Every refusal the commit pass makes
//! already crosses the wire with a reason (`docs/combat_actions.md`'s D11), and
//! this records those rather than inferring them. What it adds is *time*: the gap
//! between two edges, which is the only place a stall can live once every edge
//! has a name.

use std::collections::VecDeque;
use std::time::Duration;

use openshard_protocol::feedback::{
    ActionPhase, ActionStage, BalkState, CombatActionKind, CombatActionOutcome,
};
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::Graphic;

use crate::crowd::{ActionFill, ActionProgress};

/// How many entries are kept. Older ones fall off the front.
///
/// A fight at the shipped pace produces on the order of ten entries a second per
/// fighter, so this is minutes of one fight or a shorter stretch of a crowd —
/// long enough that a person who noticed a stall and then reached for the panel
/// still has it, which is the only requirement.
const CAPACITY: usize = 4_000;

/// One thing this client was told, or one thing the person watching said.
#[derive(Clone, PartialEq, Debug)]
pub struct Entry {
    /// This client's own clock when it arrived.
    ///
    /// Not a server tick, and there is no honest way to make it one: the wire
    /// carries no tick number, so what can be measured here is when the *client*
    /// learned something. That is also the right clock for the question — the bar
    /// is drawn against it too, so a gap in this log is a gap the player saw.
    pub at: Duration,
    /// Whose fighting this is about, where the packet names somebody.
    pub actor: Option<Serial>,
    /// What happened.
    pub event: Event,
}

/// The vocabulary. One variant per thing the wire can say about a fight, plus
/// the one thing a person can.
#[derive(Clone, PartialEq, Debug)]
pub enum Event {
    /// `CombatActionPhase` — an action began, or re-announced its interval.
    Committed {
        kind: CombatActionKind,
        phase: ActionPhase,
    },
    /// `CombatActionStage` — it moved into a new stretch.
    Staged { stage: ActionStage },
    /// `CombatActionEnded` — it is over, and this is how.
    Ended { outcome: CombatActionOutcome },
    /// `CombatActionBalked` — it cannot begin, or can again.
    Balked { balk: BalkState },
    /// `SwingTiming` — how long the animation that follows should be stretched
    /// over. Recorded separately from the phase because they are two packets and
    /// the whole business of this log is noticing when two things that should
    /// agree do not.
    Timed { millis: u32 },
    /// An animation packet, which is what the body is *drawn* doing. The other
    /// half of every complaint that begins "he just stands there".
    Animated { group: u16 },
    /// A moving effect — the arrow, on its way. `Serial` is the actor it came
    /// from where the packet named one.
    Flight { art: Graphic },
    /// The person watching said something was wrong here, and what was on screen
    /// when they did.
    Mark {
        /// Whatever the person typed, or empty.
        note: String,
        /// What this client was drawing over the marked body at that instant.
        /// `None` is itself the report: nothing at all.
        seen: Option<Seen>,
    },
}

/// What was on screen over one body, flattened out of
/// [`ActionProgress`](crate::crowd::ActionProgress).
///
/// Flattened rather than held, because the progress is a live projection that
/// ages: keeping one would be keeping a value that changes after the moment it
/// was supposed to describe.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Seen {
    /// The bar, how full it is, and whether the short loose follows a held
    /// draw: `None` for a body with no action running.
    pub bar: Option<(CombatActionKind, ActionStage, ActionFill, bool)>,
    /// The word standing beside it, if one was.
    pub outcome: Option<CombatActionOutcome>,
    /// What it was held up by, if anything.
    pub balked: Option<openshard_protocol::feedback::InterruptReason>,
}

impl Seen {
    /// What the HUD was showing, at the moment of asking.
    #[must_use]
    pub fn of(progress: ActionProgress) -> Self {
        Self {
            bar: progress.running.map(|running| {
                (
                    running.kind,
                    running.stage,
                    running.fill,
                    running.released_from_held_draw,
                )
            }),
            outcome: progress.ended,
            balked: progress.balked,
        }
    }
}

/// The last few thousand things this client heard about fighting.
///
/// Always recording, and that is the design rather than an oversight: a person
/// notices a stall *after* it has happened, and a recorder they have to arm
/// first is one that is never armed when it matters. It costs a bounded deque
/// and one push per combat packet.
#[derive(Clone, Debug)]
pub struct CombatLog {
    entries: VecDeque<Entry>,
    /// This client's clock, advanced by the app with the same delta the crowd
    /// gets — so a timestamp here and the fill of a bar are the same clock, and a
    /// gap in the log is a gap a person could have seen.
    now: Duration,
}

impl Default for CombatLog {
    /// An empty log at time zero.
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            now: Duration::ZERO,
        }
    }
}

impl CombatLog {
    /// Age by one frame. The same delta the crowd is given.
    pub fn advance(&mut self, dt: Duration) {
        self.now = self.now.saturating_add(dt);
    }

    /// This client's clock, for whoever needs to place something against the log.
    #[must_use]
    pub const fn now(&self) -> Duration {
        self.now
    }

    /// Write one thing down.
    pub fn record(&mut self, actor: Option<Serial>, event: Event) {
        if self.entries.len() == CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            at: self.now,
            actor,
            event,
        });
    }

    /// Stamp a mark: *"here"*, with what was drawn over `actor` at that instant.
    pub fn mark(&mut self, actor: Option<Serial>, note: String, seen: Option<ActionProgress>) {
        self.record(
            actor,
            Event::Mark {
                note,
                seen: seen.map(Seen::of),
            },
        );
    }

    /// How many are kept.
    ///
    /// The only reader of the entries themselves is [`to_text`](Self::to_text):
    /// the panel and the file are one rendering, deliberately, so a line a person
    /// read on screen and the line they hand to somebody else cannot say two
    /// different things about the same edge.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Throw it all away.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// The whole log as text, one entry a line, with the gap since the previous
    /// line in front of each.
    ///
    /// **The gap is the column the file exists for.** Every edge already carries
    /// its own reason; what a stall looks like is two edges with a hole between
    /// them, and a reader scanning for one wants it in a fixed place rather than
    /// subtracted by hand.
    #[must_use]
    pub fn to_text(&self, only: Option<Serial>) -> String {
        let mut out = String::new();
        let mut previous: Option<Duration> = None;
        for entry in self.entries.iter().filter(|entry| match only {
            // A mark is never filtered out: it is the reader's own bookmark, and
            // a bookmark that vanishes when the filter narrows is one nobody can
            // navigate by.
            Some(_) if matches!(entry.event, Event::Mark { .. }) => true,
            Some(serial) => entry.actor == Some(serial),
            None => true,
        }) {
            let gap = previous.map_or(Duration::ZERO, |at| entry.at.saturating_sub(at));
            previous = Some(entry.at);
            out.push_str(&format!(
                "{:>9.3}s  +{:>7.0}ms  {:>10}  {}\n",
                entry.at.as_secs_f32(),
                gap.as_secs_f32() * 1000.0,
                entry
                    .actor
                    .map_or_else(|| "-".to_owned(), |serial| format!("{:08X}", serial.raw())),
                describe(&entry.event),
            ));
        }
        out
    }
}

/// One event as a line of text, for the panel and for the file alike — they must
/// not be able to say two different things about the same entry.
#[must_use]
pub fn describe(event: &Event) -> String {
    match event {
        Event::Committed { kind, phase } => match phase {
            ActionPhase::Arming { ready_in } => {
                format!("commit  {kind:?} arming over {}ms", ready_in.millis())
            }
            ActionPhase::Releasing { impact_in } => {
                format!("commit  {kind:?} releasing over {}ms", impact_in.millis())
            }
            ActionPhase::Armed { endurance } => {
                format!("commit  {kind:?} armed for {}ms", endurance.millis())
            }
        },
        Event::Staged { stage } => format!("stage   {stage:?}"),
        Event::Ended { outcome } => format!("end     {outcome:?}"),
        Event::Balked { balk } => match balk {
            BalkState::Blocked(reason) => format!("balk    blocked: {reason:?}"),
            BalkState::Clear => "balk    lifted".to_owned(),
        },
        Event::Timed { millis } => format!("timing  animation stretched to {millis}ms"),
        Event::Animated { group } => format!("animate group {group}"),
        Event::Flight { art } => format!("flight  art 0x{:04X}", art.0),
        Event::Mark { note, seen } => {
            let what = match seen {
                None => "nothing at all was drawn over that body".to_owned(),
                Some(seen) => describe_seen(*seen),
            };
            match note.is_empty() {
                true => format!("MARK    {what}"),
                false => format!("MARK    {note} — {what}"),
            }
        }
    }
}

/// What was on screen, as a sentence.
#[must_use]
fn describe_seen(seen: Seen) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some((kind, stage, fill, released_from_held_draw)) = seen.bar {
        parts.push(match fill {
            ActionFill::Arming { filled } => {
                format!("{kind:?} drawing {:.0}% ({stage:?})", filled * 100.0)
            }
            ActionFill::Armed => format!("{kind:?} held ({stage:?})"),
            ActionFill::Releasing { filled } if released_from_held_draw => {
                format!("{kind:?} {:.0}% ({stage:?}; bow drawn)", filled * 100.0)
            }
            ActionFill::Releasing { filled } => format!("{kind:?} {:.0}% ({stage:?})", filled * 100.0),
        });
    }
    if let Some(outcome) = seen.outcome {
        parts.push(format!("last: {outcome:?}"));
    }
    if let Some(reason) = seen.balked {
        parts.push(format!("held up: {reason:?}"));
    }
    match parts.is_empty() {
        true => "nothing at all was drawn over that body".to_owned(),
        false => parts.join(", "),
    }
}

#[cfg(test)]
mod tests {
    use super::{CombatLog, Event};
    use openshard_protocol::feedback::{ActionStage, CombatActionOutcome, InterruptReason};
    use openshard_protocol::serial::Serial;
    use std::time::Duration;

    fn serial(raw: u32) -> Serial {
        Serial::new(raw).expect("a nonzero serial")
    }

    fn log_with_two_edges(gap: Duration) -> CombatLog {
        let mut log = CombatLog::default();
        log.record(
            Some(serial(7)),
            Event::Staged {
                stage: ActionStage::Load,
            },
        );
        log.advance(gap);
        log.record(
            Some(serial(7)),
            Event::Ended {
                outcome: CombatActionOutcome::Miss,
            },
        );
        log
    }

    /// The column the whole file exists for: how long nothing happened.
    #[test]
    fn the_text_puts_the_gap_between_two_edges_in_front_of_the_second() {
        let text = log_with_two_edges(Duration::from_millis(1_600)).to_text(None);
        let second = text.lines().nth(1).expect("two lines");
        assert!(
            second.contains("+   1600ms"),
            "the gap is not in the line: {second}"
        );
    }

    /// A ring, and it drops the *oldest*: a recorder that filled up and then
    /// stopped listening would be empty of exactly the moment somebody reached
    /// for it.
    #[test]
    fn the_log_is_bounded_and_keeps_the_newest() {
        let mut log = CombatLog::default();
        for stage in [ActionStage::Ready, ActionStage::Load, ActionStage::Release]
            .into_iter()
            .cycle()
            .take(super::CAPACITY + 10)
        {
            log.record(None, Event::Staged { stage });
        }
        assert_eq!(log.len(), super::CAPACITY);
    }

    /// A mark survives the filter. It is the reader's bookmark, and a bookmark
    /// that disappears when you narrow the view is one nobody can navigate by.
    #[test]
    fn narrowing_to_one_body_keeps_every_mark() {
        let mut log = CombatLog::default();
        log.record(
            Some(serial(1)),
            Event::Balked {
                balk: openshard_protocol::feedback::BalkState::Blocked(InterruptReason::OutOfReach),
            },
        );
        log.mark(Some(serial(2)), "here".to_owned(), None);
        let mine = log.to_text(Some(serial(1)));
        assert_eq!(
            mine.lines().count(),
            2,
            "the other body's balk went, the mark stayed"
        );
        assert!(mine.contains("MARK"));
        assert!(mine.contains("nothing at all was drawn"));
    }
}
