//! Reading a journal back, and grouping it the way a person asks about it.
//!
//! Nobody debugs a *line*. What a report is about is one click and everything
//! that followed it — the order, the four replans, and how it ended — so that is
//! the unit this hands back: an [`Episode`].

use std::path::Path;

use crate::record::{
    Abandonment,
    Arrival,
    Entry,
    Event,
    Order,
    Plan,
    Session,
};

/// Why a journal could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("the journal at {path} could not be opened: {source}")]
    Open { path: String, source: std::io::Error },
    /// One line did not parse. Named by line number, because the rest of the
    /// file is usually fine and the answer is normally "the session was killed
    /// mid-write".
    #[error("line {line} of {path} is not a record: {source}")]
    Line {
        path:   String,
        line:   usize,
        source: serde_json::Error,
    },
}

/// Every line of a journal, in order.
///
/// # Errors
///
/// [`ReadError`] when the file cannot be opened or one of its lines is not a
/// record. A trailing partial line — the client was killed mid-write — is
/// dropped rather than refused: it is the ordinary way a session ends, and
/// refusing the whole file for it would throw away the very click that killed
/// it.
pub fn read(path: &Path) -> Result<Vec<Entry>, ReadError> {
    let text = std::fs::read_to_string(path).map_err(|source| {
        ReadError::Open {
            path: path.display().to_string(),
            source,
        }
    })?;
    let complete = match text.ends_with('\n') {
        true => text.as_str(),
        // The last line has no newline behind it, so it is whatever got written
        // before the process went away.
        false => &text[..text.rfind('\n').map_or(0, |end| end + 1)],
    };
    complete
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str::<Entry>(line).map_err(|source| {
                ReadError::Line {
                    path: path.display().to_string(),
                    line: index + 1,
                    source,
                }
            })
        })
        .collect()
}

/// What a session said about itself, if it got as far as saying it.
#[must_use]
pub fn session(entries: &[Entry]) -> Option<&Session> {
    entries.iter().find_map(|entry| {
        match &entry.event {
            Event::Session(session) => Some(session),
            Event::Order(_) | Event::Plan(_) | Event::Arrived(_) | Event::Abandoned(_) | Event::Closed(_) => {
                None
            }
        }
    })
}

/// What the session line **in force for line `seq`** said.
///
/// Not always the first one. A client that starts without a coarse graph and
/// bakes one when the world arrives writes a second session line at that
/// moment, and a plan after it was planned under the later line — see
/// [`crate::write::Journal::note_coarse`]. A journal turned off and on again in
/// the F1 window writes one too.
///
/// Falls back to the first session line for a `seq` in front of all of them: a
/// journal can start mid-walk, and what a reader wants there is the file's own
/// account of itself rather than nothing.
#[must_use]
pub fn session_at(entries: &[Entry], seq: u64) -> Option<&Session> {
    entries
        .iter()
        .take_while(|entry| entry.seq <= seq)
        .filter_map(|entry| {
            match &entry.event {
                Event::Session(session) => Some(session),
                Event::Order(_)
                | Event::Plan(_)
                | Event::Arrived(_)
                | Event::Abandoned(_)
                | Event::Closed(_) => None,
            }
        })
        .last()
        .or_else(|| session(entries))
}

/// One destination, from the click that named it to whatever became of it.
///
/// **A plan without an order in front of it still makes an episode.** A journal
/// can start mid-walk — the variable was set for a session that was already
/// under way in someone's head, the first click happened before the window was
/// focused — and the plans are the interesting part either way.
#[derive(Clone, Debug)]
pub struct Episode {
    /// Position in the session, from one: what `--episode` names.
    pub number:   usize,
    /// The click, when the journal caught one.
    pub order:    Option<Order>,
    /// Every search this destination was answered by, in order. The first is
    /// the click's own; the rest are replans, from wherever the walk had got
    /// to.
    pub plans:    Vec<(u64, Plan)>,
    /// How it ended, if it did — a session that was still walking when the
    /// window closed ends in neither.
    pub ending:   Option<Ending>,
    /// Line numbers, for a reader that wants to go and look at the file.
    pub seq_from: u64,
    pub seq_to:   u64,
}

/// The two ways an order stops being an order.
#[derive(Clone, Debug)]
pub enum Ending {
    Arrived(Arrival),
    Abandoned(Abandonment),
}

/// Group a session into one episode per destination.
///
/// An order opens an episode and an arrival or an abandonment closes it; a
/// second order closes whatever was open, because that is what the client does
/// — a new destination replaces the old one, walked or not.
#[must_use]
pub fn episodes(entries: &[Entry]) -> Vec<Episode> {
    let mut episodes: Vec<Episode> = Vec::new();
    for entry in entries {
        match &entry.event {
            // Neither opens or closes an episode: one says what the file is
            // about and the other says the file stops here.
            Event::Session(_) | Event::Closed(_) => {}
            Event::Order(order) => {
                episodes.push(Episode {
                    number:   episodes.len() + 1,
                    order:    Some(order.clone()),
                    plans:    Vec::new(),
                    ending:   None,
                    seq_from: entry.seq,
                    seq_to:   entry.seq,
                });
            }
            Event::Plan(plan) => {
                // A plan for a destination nobody saw named — the journal
                // started mid-walk — opens an episode of its own rather than
                // being dropped.
                let open = episodes
                    .last()
                    .is_some_and(|episode| episode.ending.is_none() && names(episode, plan));
                if !open {
                    episodes.push(Episode {
                        number:   episodes.len() + 1,
                        order:    None,
                        plans:    Vec::new(),
                        ending:   None,
                        seq_from: entry.seq,
                        seq_to:   entry.seq,
                    });
                }
                let episode = episodes.last_mut().expect("one was just pushed if none was open");
                episode.plans.push((entry.seq, plan.clone()));
                episode.seq_to = entry.seq;
            }
            Event::Arrived(arrival) => {
                if let Some(episode) = episodes.last_mut() {
                    if episode.ending.is_none() {
                        episode.ending = Some(Ending::Arrived(arrival.clone()));
                        episode.seq_to = entry.seq;
                    }
                }
            }
            Event::Abandoned(abandonment) => {
                if let Some(episode) = episodes.last_mut() {
                    if episode.ending.is_none() {
                        episode.ending = Some(Ending::Abandoned(abandonment.clone()));
                        episode.seq_to = entry.seq;
                    }
                }
            }
        }
    }
    episodes
}

/// Whether an open episode is the one this plan belongs to: the destination it
/// was given for.
///
/// The *destination* and not the start — a replan starts from wherever the walk
/// has got to, which is precisely what changes between one plan and the next.
fn names(episode: &Episode, plan: &Plan) -> bool {
    match &episode.order {
        Some(order) => order.to == plan.to,
        // An episode the journal never saw the order for is held together by
        // its plans agreeing with each other.
        None => episode.plans.last().is_some_and(|(_, first)| first.to == plan.to),
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::world::Point;

    use super::*;
    use crate::record::{
        Exit,
        Place,
        Probe,
        Step,
    };
    use crate::write::Journal;

    fn probe(arrived: bool) -> Probe {
        Probe {
            arrived,
            exit: match arrived {
                true => Exit::Goal,
                false => Exit::Budget,
            },
            explored: 42,
            written: 84,
            long: None,
        }
    }

    fn plan(to: Point, open: Vec<Step>) -> Plan {
        Plan {
            from: Place::of(Point::new(100, 100, 0)),
            to: Place::of(to),
            resolved: Place::of(to),
            live: probe(!open.is_empty()),
            doors_open: None,
            open,
            barred: Vec::new(),
            open_points: Vec::new(),
            barred_points: Vec::new(),
            refusal: None,
            elapsed_us: 1_200,
        }
    }

    /// A whole session goes out through the writer and comes back through the
    /// reader unchanged — the round trip this crate is, end to end.
    #[test]
    fn a_session_written_is_a_session_read_back() {
        let dir = std::env::temp_dir().join("openshard-pathlog-read-test");
        std::fs::create_dir_all(&dir).expect("the test's own directory");
        let path = dir.join(crate::write::DEFAULT_PATH);
        let goal = Point::new(120, 100, 0);
        {
            // The session line is the journal's own first line — see
            // `Journal::record` — so this writes four events and reads five.
            let mut journal = Journal::at(
                path.clone(),
                Session {
                    facet:  0,
                    world:  None,
                    coarse: true,
                    budget: 700,
                    weight: "5/4".to_owned(),
                },
            );
            journal.record(Event::Order(Order {
                from: Place::of(Point::new(100, 100, 0)),
                to:   Place::of(goal),
            }));
            journal.record(Event::Plan(plan(goal, vec![Step::East, Step::East])));
            journal.record(Event::Plan(plan(goal, vec![Step::East])));
            journal.record(Event::Arrived(Arrival {
                at:   Place::of(goal),
                goal: Place::of(goal),
            }));
        }
        let entries = read(&path).expect("the journal was just written");
        assert_eq!(entries.len(), 5);
        let session = session(&entries).expect("the session line is first");
        assert_eq!((session.budget, session.coarse), (700, true));

        let episodes = episodes(&entries);
        assert_eq!(episodes.len(), 1, "one click is one episode");
        let episode = &episodes[0];
        assert_eq!(episode.plans.len(), 2, "the click and the replan after it");
        assert_eq!(episode.order.as_ref().expect("the click").to, Place::of(goal));
        assert!(
            matches!(episode.ending, Some(Ending::Arrived(_))),
            "it ended by arriving"
        );
        std::fs::remove_file(&path).expect("the test's own file");
    }

    /// A second destination closes the first, walked or not — which is what the
    /// client does when a player clicks somewhere else mid-walk.
    #[test]
    fn a_second_destination_closes_the_first() {
        let first = Point::new(120, 100, 0);
        let second = Point::new(90, 90, 0);
        let entries: Vec<Entry> = [
            Event::Order(Order {
                from: Place::of(Point::new(100, 100, 0)),
                to:   Place::of(first),
            }),
            Event::Plan(plan(first, vec![Step::East])),
            Event::Order(Order {
                from: Place::of(Point::new(105, 100, 0)),
                to:   Place::of(second),
            }),
            Event::Plan(plan(second, vec![Step::West])),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, event)| {
            Entry {
                seq: index as u64 + 1,
                at_ms: index as u64 * 100,
                event,
            }
        })
        .collect();
        let episodes = episodes(&entries);
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[0].plans.len(), 1);
        assert_eq!(episodes[1].plans.len(), 1);
        assert!(
            episodes.iter().all(|episode| episode.ending.is_none()),
            "neither destination was reached, and nothing pretends otherwise"
        );
    }

    /// A session that bakes its graph mid-file is read one way before that
    /// moment and the other way after it — the whole point of writing a second
    /// session line instead of editing the first.
    ///
    /// This is the reading finding 27 was about: the click the player
    /// complained about came *after* the login bake, and a replay that took the
    /// file's first line would have replayed it as a client with no corridor to
    /// ask.
    #[test]
    fn the_session_in_force_is_the_last_one_written_before_the_line() {
        let dir = std::env::temp_dir().join("openshard-pathlog-coarse-test");
        std::fs::create_dir_all(&dir).expect("the test's own directory");
        let path = dir.join(crate::write::DEFAULT_PATH);
        let early = Point::new(120, 100, 0);
        let late = Point::new(1_342, 1_893, 88);
        {
            let mut journal = Journal::at(
                path.clone(),
                Session {
                    facet:  0,
                    world:  None,
                    coarse: false,
                    budget: 700,
                    weight: "5/4".to_owned(),
                },
            );
            journal.record(Event::Plan(plan(early, vec![Step::East])));
            journal.note_coarse(true);
            journal.record(Event::Plan(plan(late, vec![Step::West])));
        }
        let entries = read(&path).expect("the journal was just written");
        let episodes = episodes(&entries);
        assert_eq!(episodes.len(), 2, "two destinations, one either side of the bake");
        assert!(
            !session(&entries).expect("the file says what it is").coarse,
            "the file's first line is still true of the lines under it"
        );
        assert!(
            !session_at(&entries, episodes[0].seq_from)
                .expect("the plan before the bake has a session")
                .coarse,
            "the plan before the bake had no graph to ask"
        );
        assert!(
            session_at(&entries, episodes[1].seq_from)
                .expect("and so does the one after it")
                .coarse,
            "the plan after the bake was planned with a graph, and the file says so"
        );
        std::fs::remove_file(&path).expect("the test's own file");
    }

    /// A journal whose last line was cut off mid-write still reads: that is how
    /// a killed client's file ends, and the lines before it are the report.
    #[test]
    fn a_journal_cut_off_mid_line_reads_up_to_the_cut() {
        let dir = std::env::temp_dir().join("openshard-pathlog-truncated-test");
        std::fs::create_dir_all(&dir).expect("the test's own directory");
        let path = dir.join(crate::write::DEFAULT_PATH);
        {
            let mut journal = Journal::at(
                path.clone(),
                Session {
                    facet:  0,
                    world:  None,
                    coarse: false,
                    budget: 700,
                    weight: "5/4".to_owned(),
                },
            );
            journal.record(Event::Order(Order {
                from: Place::of(Point::new(1, 1, 0)),
                to:   Place::of(Point::new(2, 2, 0)),
            }));
        }
        let mut text = std::fs::read_to_string(&path).expect("the journal was just written");
        text.push_str("{\"seq\":3,\"at_ms\":1,\"eve");
        std::fs::write(&path, &text).expect("the test's own file");
        let entries = read(&path).expect("a cut line is not a broken journal");
        assert_eq!(
            entries.len(),
            2,
            "the session line and the order survive the cut one"
        );
        std::fs::remove_file(&path).expect("the test's own file");
    }
}
