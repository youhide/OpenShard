//! Writing the journal, from inside a client that is drawing frames.
//!
//! # A value with an owner, and not a sink the process reaches for
//!
//! The journal is held by the thing that runs searches — the client's
//! `Steering` — and handed to the plan that writes to it. It used to be a
//! `OnceLock` keyed off an environment variable, which is the shape a
//! diagnostic takes when nobody wants to thread it anywhere; what that cost is
//! that the switch was invisible in the client, unreachable from the F1 window
//! where every other diagnostic lives, and had to be *predicted* before the
//! session that needed it.
//!
//! So it is on by default now and turned off in the F1 window, and this type is
//! what that switch moves: [`Journal::set_aside`] keeps the file and stops
//! writing, [`Journal::take_up`] starts again with a fresh `session` line so a
//! reader can see where the gap was.
//!
//! # Lazily opened, rotated once, and bounded
//!
//! Nothing touches the disk until there is a line worth writing: a client
//! started to look at a gump plans no route, creates no file, and does not push
//! the previous session out of the way. The first line is what rotates
//! `path-journal.jsonl` to `path-journal.prev.jsonl` — so the session somebody
//! has just quit survives exactly one restart, which is the shape of *play,
//! quit, ask about it*.
//!
//! And it stops at [`SIZE_CAP`]. A journal that is always on outlives the
//! session it was interesting for, and the failure mode of an unbounded one is
//! a full disk on somebody's long night. The line that says it stopped is
//! written *into the file*, so a reader can tell a cap from a crash.
//!
//! # It is a diagnostic, and it fails like one
//!
//! A file that cannot be opened or written — no permission, no space — stops
//! the journal and says so once on stderr and in [`Journal::tally`], which the
//! F1 window draws. It does not take the client down: nothing about the game
//! depends on this file existing.

use std::fs::File;
use std::io::{
    BufWriter,
    Write,
};
use std::path::{
    Path,
    PathBuf,
};
use std::time::Instant;

use crate::record::{
    Closure,
    Entry,
    Event,
    Session,
};

/// The file a client writes unless somebody turns it off, relative to wherever
/// the client was started — beside `client_ui.ron`, which is the other file
/// that is one person's own.
pub const DEFAULT_PATH: &str = "path-journal.jsonl";

/// Where the previous session's journal is moved when a new one starts writing.
pub const PREVIOUS_PATH: &str = "path-journal.prev.jsonl";

/// How much one session may write before the journal closes itself.
///
/// Roughly a hundred thousand plans — two orders of magnitude past any session
/// anybody has debugged, and small enough that a client left running overnight
/// cannot fill a disk with it.
pub const SIZE_CAP: u64 = 64 * 1024 * 1024;

/// Why a journal stopped writing before the session ended.
///
/// Not "off": a journal somebody switched off in the F1 window is not stopped,
/// it is waiting — see [`Journal::take_up`]. These two are the ones it cannot
/// come back from on its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// [`SIZE_CAP`] was reached.
    SizeCap,
    /// The file could not be opened or written. The text is the operating
    /// system's, kept for the F1 line that shows it.
    Trouble(String),
}

/// What a journal has done this session, for the window that draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tally {
    /// Whether lines are being written at all.
    pub writing: bool,
    /// Destinations named.
    pub orders:  u32,
    /// Searches that answered them — always the larger of the two, because a
    /// route is replanned as it is walked.
    pub plans:   u32,
    /// Bytes on disk, against [`SIZE_CAP`].
    pub bytes:   u64,
    /// Why it is not writing any more, when it is not.
    pub stopped: Option<Stopped>,
}

/// The journal of one session.
#[derive(Debug)]
pub struct Journal {
    path:       PathBuf,
    /// What the first line says. Kept rather than written up front, because the
    /// first line is what creates the file — see the module docs — and because
    /// a journal turned off and on again owes the reader a fresh one.
    session:    Session,
    /// Whether the `session` line is still owed.
    header_due: bool,
    opened:     Instant,
    /// `None` until the first line. Not "no journal": a journal with no file
    /// yet is one nobody has planned a route in.
    file:       Option<BufWriter<File>>,
    /// Whether the previous session's file has already been moved aside. Once
    /// per process, however often the switch is flipped.
    rotated:    bool,
    writing:    bool,
    seq:        u64,
    bytes:      u64,
    orders:     u32,
    plans:      u32,
    stopped:    Option<Stopped>,
}

impl Journal {
    /// A journal that will write to `path`, starting now.
    ///
    /// Nothing is opened here. `session` is the line every later one is read
    /// against — the facet, whether a coarse graph was loaded, the budget and
    /// the weight.
    #[must_use]
    pub fn at(path: PathBuf, session: Session) -> Self {
        Self {
            path,
            session,
            header_due: true,
            opened: Instant::now(),
            file: None,
            rotated: false,
            writing: true,
            seq: 0,
            bytes: 0,
            orders: 0,
            plans: 0,
            stopped: None,
        }
    }

    /// The file this writes to, for the line a client prints at startup and for
    /// the window that names it.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What it has done this session.
    #[must_use]
    pub fn tally(&self) -> Tally {
        Tally {
            writing: self.writing && self.stopped.is_none(),
            orders:  self.orders,
            plans:   self.plans,
            bytes:   self.bytes,
            stopped: self.stopped.clone(),
        }
    }

    /// Stop writing, keeping what is on disk.
    ///
    /// The lines already written are the report; discarding them because
    /// somebody unticked a box would throw away the very thing they are about
    /// to attach to it.
    pub fn set_aside(&mut self) {
        self.writing = false;
    }

    /// Write again from now on, with a fresh `session` line so that a reader
    /// can see where the gap was.
    ///
    /// Does nothing for a journal that [`Stopped`] — the cap is a cap, and a
    /// file that could not be written will not be written by asking twice.
    pub fn take_up(&mut self) {
        if self.stopped.is_some() {
            return;
        }
        self.writing = true;
        self.header_due = true;
    }

    /// The coarse graph the session line names has arrived — or gone.
    ///
    /// A world off the wire is baked after the client is already running, so a
    /// session that started without a graph acquires one; a facet replaced
    /// mid-play loses the one it had. Either way a replay that read the old
    /// line would call every long route planned after the change something the
    /// client never did.
    ///
    /// **Which is why a change after the header has gone out is a fresh session
    /// line rather than an edit.** The lines already on disk are true: those
    /// routes really were planned without a graph. What the file owes a reader
    /// is where that stopped being so, and that is a line — the same shape
    /// [`Journal::take_up`] writes for a gap. A reader takes the session in
    /// force for a line rather than the first one; see `read::session_at`.
    ///
    /// A journal that has not written its header yet simply tells the truth in
    /// the line it still owes, and touches no disk: a client that plans no
    /// route creates no file, bake or no bake.
    pub fn note_coarse(&mut self, coarse: bool) {
        if self.session.coarse == coarse {
            return;
        }
        self.session.coarse = coarse;
        // Set aside or stopped writes nothing at all, and the header still due
        // carries the new value when it goes.
        if self.writing && self.stopped.is_none() && !self.header_due {
            let session = Event::Session(self.session.clone());
            self.write_line(session);
        }
    }

    /// Write one event, and flush it.
    ///
    /// **Flushed per line, on purpose.** The session this records ends when
    /// somebody closes a window or kills a hung client, and a buffered tail
    /// lost at exactly that moment is the tail that holds whatever they were
    /// trying to show. One `write` per click is not a cost worth optimising
    /// against that.
    pub fn record(&mut self, event: Event) {
        if !self.writing || self.stopped.is_some() {
            return;
        }
        match &event {
            Event::Order(_) => self.orders += 1,
            Event::Plan(_) => self.plans += 1,
            Event::Session(_) | Event::Arrived(_) | Event::Abandoned(_) | Event::Closed(_) => {}
        }
        if self.header_due {
            self.header_due = false;
            let session = Event::Session(self.session.clone());
            self.write_line(session);
        }
        self.write_line(event);
        if self.bytes >= SIZE_CAP && self.stopped.is_none() {
            self.write_line(Event::Closed(Closure {
                bytes:  self.bytes,
                cap:    SIZE_CAP,
                orders: self.orders,
                plans:  self.plans,
            }));
            eprintln!(
                "path journal: {} reached the {SIZE_CAP}-byte cap and stopped; \
                 turn it off and on again in F1 to start a new one",
                self.path.display()
            );
            self.stopped = Some(Stopped::SizeCap);
            self.file = None;
        }
    }

    /// One line onto the disk, opening — and rotating — the file if this is the
    /// first of them.
    fn write_line(&mut self, event: Event) {
        let at_ms = self.opened.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        self.seq += 1;
        let entry = Entry {
            seq: self.seq,
            at_ms,
            event,
        };
        let line = serde_json::to_string(&entry).expect("a record is made of numbers and words");
        if self.file.is_none() {
            match self.open() {
                Ok(file) => self.file = Some(file),
                Err(error) => {
                    self.stop_with(&error);
                    return;
                }
            }
        }
        let file = self.file.as_mut().expect("it was just opened");
        match writeln!(file, "{line}").and_then(|()| file.flush()) {
            // The newline is a byte of the file too, and the cap is about the
            // file rather than about the records in it.
            Ok(()) => self.bytes += line.len() as u64 + 1,
            Err(error) => {
                let error = error.to_string();
                self.stop_with(&error);
            }
        }
    }

    /// Create the file, moving the previous session's out of the way first.
    ///
    /// # Errors
    ///
    /// The operating system's, as text: this is a diagnostic, and what a reader
    /// of the F1 line needs is the sentence rather than a type to match on.
    fn open(&mut self) -> Result<BufWriter<File>, String> {
        if !self.rotated {
            self.rotated = true;
            if self.path.exists() {
                let previous = self.path.with_file_name(PREVIOUS_PATH);
                // A failed rotation is not a failed journal: what it costs is
                // the session before this one, and this one is the one somebody
                // is about to look at.
                if let Err(error) = std::fs::rename(&self.path, &previous) {
                    eprintln!(
                        "path journal: {} could not be kept as {}: {error}",
                        self.path.display(),
                        previous.display()
                    );
                }
            }
        }
        File::create(&self.path)
            .map(BufWriter::new)
            .map_err(|error| error.to_string())
    }

    /// Give up on this file, saying so once.
    fn stop_with(&mut self, error: &str) {
        eprintln!(
            "path journal: {} could not be written: {error}",
            self.path.display()
        );
        self.stopped = Some(Stopped::Trouble(error.to_owned()));
        self.file = None;
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::world::Point;

    use super::*;
    use crate::record::{
        Order,
        Place,
    };

    fn session() -> Session {
        Session {
            facet:  0,
            world:  None,
            coarse: true,
            budget: 700,
            weight: "5/4".to_owned(),
        }
    }

    fn order(x: u16) -> Event {
        Event::Order(Order {
            from: Place::of(Point::new(x, 0, 0)),
            to:   Place::of(Point::new(x, 1, 0)),
        })
    }

    /// A directory of this test's own, so two tests never rotate each other's
    /// files: the journal's whole business is a fixed file name.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("openshard-pathlog-{name}"));
        std::fs::create_dir_all(&dir).expect("the test's own directory");
        dir
    }

    /// A journal nobody wrote to touches no disk at all — the whole of the
    /// lazy open, and what keeps a client that planned no route from pushing
    /// somebody's evidence out of `.prev`.
    #[test]
    fn a_journal_with_nothing_in_it_creates_no_file() {
        let dir = scratch("lazy");
        let path = dir.join(DEFAULT_PATH);
        let journal = Journal::at(path.clone(), session());
        assert!(
            !path.exists(),
            "nothing has been planned, so there is nothing to write"
        );
        assert_eq!(journal.tally().plans, 0);
    }

    /// The first line writes the session line in front of itself, and lines are
    /// numbered from one in the order they were written.
    #[test]
    fn the_first_line_carries_the_session_in_front_of_it() {
        let dir = scratch("header");
        let path = dir.join(DEFAULT_PATH);
        let mut journal = Journal::at(path.clone(), session());
        journal.record(order(1));
        journal.record(order(2));
        let text = std::fs::read_to_string(&path).expect("the journal was just written");
        let entries: Vec<Entry> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("every line is one record"))
            .collect();
        assert_eq!(entries.len(), 3, "the session line and the two orders");
        assert!(matches!(entries[0].event, Event::Session(_)));
        assert_eq!(
            entries.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(journal.tally().orders, 2);
        std::fs::remove_file(&path).expect("the test's own file");
    }

    /// A second session moves the first out of the way rather than over it, so
    /// the afternoon somebody has just quit survives one restart.
    #[test]
    fn a_new_session_keeps_the_one_before_it() {
        let dir = scratch("rotate");
        let path = dir.join(DEFAULT_PATH);
        let previous = dir.join(PREVIOUS_PATH);
        let mut first = Journal::at(path.clone(), session());
        first.record(order(1));
        let mut second = Journal::at(path.clone(), session());
        second.record(order(2));
        let kept = std::fs::read_to_string(&previous).expect("the first session was kept");
        let now = std::fs::read_to_string(&path).expect("the second session is writing");
        assert!(kept.contains("\"x\":1"), "the previous file is the first session");
        assert!(now.contains("\"x\":2"), "and the current one is the second");
        assert!(!now.contains("\"x\":1"), "the two are not in one file");
        std::fs::remove_file(&path).expect("the test's own file");
        std::fs::remove_file(&previous).expect("the test's own file");
    }

    /// Turning it off keeps the file and writes nothing; turning it back on
    /// writes a fresh session line, which is what tells a reader where the gap
    /// was.
    #[test]
    fn a_journal_set_aside_keeps_what_it_wrote_and_says_where_the_gap_is() {
        let dir = scratch("switch");
        let path = dir.join(DEFAULT_PATH);
        let mut journal = Journal::at(path.clone(), session());
        journal.record(order(1));
        journal.set_aside();
        journal.record(order(2));
        assert!(!journal.tally().writing);
        journal.take_up();
        journal.record(order(3));
        let text = std::fs::read_to_string(&path).expect("the journal was written");
        let events: Vec<Entry> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("every line is one record"))
            .collect();
        assert_eq!(
            events
                .iter()
                .filter(|entry| matches!(entry.event, Event::Session(_)))
                .count(),
            2,
            "one session line per stretch of writing"
        );
        assert!(
            !text.contains("\"x\":2"),
            "nothing is written while it is set aside"
        );
        assert!(text.contains("\"x\":3"), "and it writes again when taken up");
        std::fs::remove_file(&path).expect("the test's own file");
    }

    /// A session line with no graph in it, for the two tests about a bake that
    /// happens after the client is already running.
    fn without_a_graph() -> Session {
        Session {
            facet:  0,
            world:  None,
            coarse: false,
            budget: 700,
            weight: "5/4".to_owned(),
        }
    }

    /// The world arrives, the bake finishes, and the *first* click of the
    /// session is still ahead: the only session line the file ever gets says
    /// there was a graph, because there was one for every line under it.
    ///
    /// And nothing is on disk until that click, bake or no bake — a client
    /// somebody opened to look at a gump still pushes nobody's evidence out of
    /// `.prev`.
    #[test]
    fn a_graph_that_arrives_before_the_first_line_is_in_the_line_itself() {
        let dir = scratch("coarse-before");
        let path = dir.join(DEFAULT_PATH);
        let mut journal = Journal::at(path.clone(), without_a_graph());
        journal.note_coarse(true);
        assert!(
            !path.exists(),
            "no route has been planned, so the bake wrote nothing"
        );
        journal.record(order(1));
        let text = std::fs::read_to_string(&path).expect("the journal was just written");
        let entries: Vec<Entry> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("every line is one record"))
            .collect();
        assert_eq!(entries.len(), 2, "the session line and the order");
        assert!(
            matches!(&entries[0].event, Event::Session(session) if session.coarse),
            "the header went out after the bake and says so"
        );
        std::fs::remove_file(&path).expect("the test's own file");
    }

    /// The ordinary shape of a login: a click, *then* the graph. The header
    /// already on disk is true of the lines under it and stays as it is; what
    /// the change costs is a second session line, and everything after it is
    /// read under that one.
    #[test]
    fn a_graph_that_arrives_after_the_header_writes_a_fresh_session_line() {
        let dir = scratch("coarse-after");
        let path = dir.join(DEFAULT_PATH);
        let mut journal = Journal::at(path.clone(), without_a_graph());
        journal.record(order(1));
        journal.note_coarse(true);
        // The same news twice is one line: a second `Update::Navigation` for a
        // graph the journal already knows about says nothing new.
        journal.note_coarse(true);
        journal.record(order(2));
        let text = std::fs::read_to_string(&path).expect("the journal was just written");
        let entries: Vec<Entry> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("every line is one record"))
            .collect();
        let sessions: Vec<bool> = entries
            .iter()
            .filter_map(|entry| {
                match &entry.event {
                    Event::Session(session) => Some(session.coarse),
                    _ => None,
                }
            })
            .collect();
        assert_eq!(
            sessions,
            vec![false, true],
            "the header as it was, and one line where the graph arrived"
        );
        assert_eq!(entries.len(), 4, "two session lines and the two orders");
        assert!(
            matches!(&entries[2].event, Event::Session(_)),
            "the new line stands between the order before the bake and the one after it"
        );
        std::fs::remove_file(&path).expect("the test's own file");
    }
}
