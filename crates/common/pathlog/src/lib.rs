//! What a running client did when somebody clicked somewhere, written down as
//! it happened.
//!
//! # The question this exists to answer
//!
//! *"I clicked over there and my body walked at a wall"* is a report nobody can
//! act on: the click is gone, the route is gone, and the six replans that
//! followed it are gone with them. The pathfinder itself is a pure function over
//! the map — `openshard_movement::find_path` rolls no dice — so the one thing
//! standing between a report and a test is that nobody wrote down **which
//! question was asked**. This is where it gets written down.
//!
//! A session with `OPENSHARD_PATH_JOURNAL` set drops one JSON object per line
//! into that file, as it plays. Afterwards `openshard-movement`'s `path_replay`
//! example reads the file back, re-asks a chosen search over the real facet, and
//! prints the two answers side by side.
//!
//! # What a record holds, and what it deliberately does not
//!
//! It holds the **question and the answer**: where the body stood, where the
//! player pointed, what budget was spent, which of the two searches answered,
//! how it stopped, and every step of the route that came back — plus the points
//! those steps landed on, which is the one thing a replay over different ground
//! cannot reconstruct.
//!
//! It does **not** hold the world. No map, no live layer, no crowd — a journal
//! that carried a slice of the overlay would be a journal nobody can read and a
//! fixture nobody can edit. A replay opens the same facet the client had; when
//! the disagreement that is left is a door, a crate or a house that was standing
//! there at the time, the test that pins it *builds that house*, in the scene it
//! is a test of. A scene written out in a test is a thing a person can reason
//! about; a captured octet-soup of covers is not.
//!
//! # The shape of a session
//!
//! ```text
//! session   the facet, and whether a coarse graph was loaded — once, up front
//! order     a destination was named: a Ctrl-click, or a drag that moved it
//! plan      one search answered it — and there are several per order, because
//!           a route is replanned whenever the last one runs out
//! arrived   the body reached the place the order named
//! abandoned the order gave up: four steps that did not move the body
//! ```
//!
//! [`read::episodes`] is that grouping made into values: one episode per order,
//! with every replan under it.

pub mod read;
pub mod record;
pub mod write;
