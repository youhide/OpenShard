//! Writing the journal, from inside a client that is drawing frames.
//!
//! # Why the writer is a process-wide value
//!
//! The route planner is a free function — `steer::plan` takes a reading of the
//! ground and two points and owns nothing — and the walk that calls it is
//! reached from four inputs. Threading a journal handle down to it would put a
//! debug parameter in the signature of every one of those, and in the tests
//! that call them, for a file that exists only when an operator asks for one.
//!
//! So the journal is what the slow-query diagnostics beside it already are: a
//! sink the process opens once, from the environment, and writes to from
//! wherever the interesting thing happened. [`journal`] is the whole of that
//! decision, and it is `None` — nothing written, nothing to lock, one atomic
//! load per plan — unless `OPENSHARD_PATH_JOURNAL` named a file.
//!
//! # It is a diagnostic, and it fails like one
//!
//! A path that cannot be *opened* is an operator's typo and is worth a panic:
//! they asked for a journal, and a session that quietly does not write one
//! wastes the play session it was turned on for. A write that fails afterwards
//! — a full disk, mid-session — is not worth taking the client down over, so it
//! says so on stderr and the game carries on.

use std::fs::File;
use std::io::{
    BufWriter,
    Write,
};
use std::path::{
    Path,
    PathBuf,
};
use std::sync::{
    Mutex,
    OnceLock,
};
use std::time::Instant;

use crate::record::{
    Entry,
    Event,
};

/// The environment variable that names the file, and turns the whole thing on.
pub const JOURNAL_VAR: &str = "OPENSHARD_PATH_JOURNAL";

/// An open journal.
///
/// **The lock is over the file and the counter together**, because the two are
/// one fact: a line's `seq` is its position in the file, and a counter incremented
/// outside the lock would hand out numbers in an order the lines are not written
/// in. Contention is not a concern — one line per click and per replan, from a
/// client that plans on one thread — and correctness under a second writer is,
/// since a shard thread in the same process may one day want the same file.
#[derive(Debug)]
pub struct Journal {
    path:   PathBuf,
    opened: Instant,
    out:    Mutex<Writing>,
}

/// The two things a line needs, under one lock.
#[derive(Debug)]
struct Writing {
    file: BufWriter<File>,
    seq:  u64,
}

impl Journal {
    /// Open `path` for writing, truncating whatever was there.
    ///
    /// Truncating, not appending: a journal is about **this** session, and a
    /// file that accumulated three afternoons of clicks would make the replay's
    /// "the last order" mean the last one of whichever afternoon ended last.
    ///
    /// # Panics
    ///
    /// If the file cannot be created. See the module docs.
    #[must_use]
    pub fn create(path: &Path) -> Self {
        let file = File::create(path).unwrap_or_else(|error| {
            panic!(
                "{JOURNAL_VAR} names {} and it cannot be written: {error}",
                path.display()
            )
        });
        Self {
            path:   path.to_path_buf(),
            opened: Instant::now(),
            out:    Mutex::new(Writing {
                file: BufWriter::new(file),
                seq:  0,
            }),
        }
    }

    /// The file this is writing to, for the line a client prints at startup so
    /// that a person knows where to look.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write one event, and flush it.
    ///
    /// **Flushed per line, on purpose.** The session this is recording ends when
    /// somebody closes a window or kills a hung client, and a buffered tail lost
    /// at exactly that moment is the tail that holds whatever they were trying
    /// to show. One `write` syscall per click is not a cost worth optimising
    /// against that.
    pub fn record(&self, event: Event) {
        let at_ms = self.opened.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        let mut writing = self
            .out
            .lock()
            .expect("the journal's lock is held only for the length of one line");
        writing.seq += 1;
        let entry = Entry {
            seq: writing.seq,
            at_ms,
            event,
        };
        let line = serde_json::to_string(&entry).expect("a record is made of numbers and words");
        if let Err(error) = writeln!(writing.file, "{line}").and_then(|()| writing.file.flush()) {
            eprintln!(
                "path-journal: {} could not be written: {error}",
                self.path.display()
            );
        }
    }
}

/// The journal this process is writing, or `None` when nobody asked for one.
///
/// Read from [`JOURNAL_VAR`] once. A session that starts without the variable
/// stays without it — turning the journal on means starting the client again,
/// which is what an operator does anyway to reproduce the click they are after.
#[must_use]
pub fn journal() -> Option<&'static Journal> {
    static JOURNAL: OnceLock<Option<Journal>> = OnceLock::new();
    JOURNAL
        .get_or_init(|| std::env::var_os(JOURNAL_VAR).map(|path| Journal::create(Path::new(&path))))
        .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{
        Order,
        Place,
    };

    /// Lines come out numbered from one, in the order they were written, one
    /// JSON object per line. A replay indexes into the file by that number.
    #[test]
    fn lines_are_numbered_in_the_order_they_were_written() {
        let path = std::env::temp_dir().join("openshard-pathlog-write-test.jsonl");
        let journal = Journal::create(&path);
        for x in 0..3u16 {
            journal.record(Event::Order(Order {
                from: Place { x, y: 0, z: 0 },
                to:   Place { x, y: 1, z: 0 },
            }));
        }
        let text = std::fs::read_to_string(&path).expect("the journal was just written");
        let seqs: Vec<u64> = text
            .lines()
            .map(|line| {
                serde_json::from_str::<Entry>(line)
                    .expect("every line is one record")
                    .seq
            })
            .collect();
        assert_eq!(seqs, vec![1, 2, 3]);
        std::fs::remove_file(&path).expect("the test's own file");
    }
}
