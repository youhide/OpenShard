//! One line of the journal, and everything that can be on it.
//!
//! # These types are the wire, not the domain
//!
//! Every enum here has a twin in `openshard-movement` or in the client's
//! `steer.rs` — [`Exit`] is `SearchExit`, [`Refusal`] is `steer::Refusal`, and
//! so on. That is deliberate and is the reason this crate depends on neither:
//! a journal is a **file format**, and a file written last week has to keep
//! parsing after the search grows a new way of stopping.
//!
//! The copies do not drift, because the writer converts with an exhaustive
//! `match` and no wildcard arm: a new variant on the domain side is a compile
//! error at the one place that translates it, which is exactly where somebody
//! has to decide what the file should call it.

use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;
use serde::{
    Deserialize,
    Serialize,
};

/// One line of the file: what happened, when, and in what order.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    /// Position in the session, from one. Nothing is derived from it — it is
    /// there so that a person and a replay can name the same line.
    pub seq:   u64,
    /// Milliseconds since the journal was opened.
    ///
    /// Not a wall clock: what a reader wants is the gap between a click and the
    /// replan that followed it, and a monotonic offset says that without
    /// putting a timestamp of the operator's afternoon in a file they may want
    /// to attach to a report.
    pub at_ms: u64,
    #[serde(flatten)]
    pub event: Event,
}

/// The five things a session has to say.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The facet this session is walking on. First line of the file.
    Session(Session),
    /// A destination was named — a click, or a drag that moved it somewhere new.
    Order(Order),
    /// One search answered an order. Several per order: a route is replanned
    /// whenever what is left of the last one runs out, and every one of those
    /// is a line of its own.
    Plan(Plan),
    /// The body reached the place the order named. The ordinary end.
    Arrived(Arrival),
    /// The order gave up: the body did not move for as many steps as the
    /// client's patience allows.
    Abandoned(Abandonment),
    /// The journal stopped itself. Last line of the file, and the difference
    /// between a session that was cut short by policy and one that was cut
    /// short by a crash.
    Closed(Closure),
}

/// What the client is walking on, said once so that every line after it can be
/// short.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    /// The facet, as the wire numbers it.
    pub facet:  u8,
    /// The base set this facet's ground came out of, by file name — absent for
    /// a facet read from the install's own `map*` files.
    ///
    /// The *name* and not the path: a journal is a thing to attach to a report,
    /// and one operator's directory layout is nobody else's business.
    pub world:  Option<String>,
    /// Whether a baked coarse graph was loaded. A client without one refuses
    /// every destination past `COARSE_MIN_DISTANCE`, and a replay that did not
    /// know would call that refusal a bug.
    pub coarse: bool,
    /// The node budget every search in this session was given.
    pub budget: usize,
    /// The heuristic weight, as the ratio it is: `"5/4"` for a body's own
    /// route.
    pub weight: String,
}

/// A destination was named.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Order {
    /// Where the body stood when it was given.
    pub from: Place,
    /// The place the player pointed at — the height the click carried, not a
    /// surface. What that resolves to is [`Plan::resolved`].
    pub to:   Place,
}

/// One search, and what it answered.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    /// Where the body stood for *this* search, which is not where the order was
    /// given: a replan starts from wherever the walk has got to.
    pub from:          Place,
    /// The destination as the order named it.
    pub to:            Place,
    /// The standing place that destination resolves to — a table's top rather
    /// than the height the cursor hit. The search compares against this, and a
    /// route that "ends on the goal" ends here.
    pub resolved:      Place,
    /// The world as it stands: shut doors shut, crates where they are, and
    /// everybody who is standing about in the way.
    pub live:          Probe,
    /// The same ground with every shut door standing open, asked only when the
    /// first found no way through. Absent when it was not asked.
    pub doors_open:    Option<Probe>,
    /// The part of the route the world as it stands allows — what the body
    /// actually walks.
    pub open:          Vec<Step>,
    /// What is left of the way past the first thing standing in it. Empty
    /// unless something is.
    pub barred:        Vec<Step>,
    /// Where those steps land, in order.
    ///
    /// **The one thing a replay cannot recompute.** Everything else here is a
    /// question, and a replay can ask it again; these are where the body was
    /// going to put its feet *on the ground as it was*, which is the evidence
    /// that a house, a door or a crowd was standing somewhere at the time.
    pub open_points:   Vec<Place>,
    pub barred_points: Vec<Place>,
    /// Why the route does not end on the destination, when it does not.
    pub refusal:       Option<Refusal>,
    /// What the whole plan cost, microseconds — both searches, the cut at the
    /// door, and the walk that produced the points.
    pub elapsed_us:    u64,
}

/// The body reached the place the order named.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Arrival {
    pub at:   Place,
    /// The destination as the order named it, height and all.
    pub goal: Place,
}

/// The order gave up.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Abandonment {
    /// Where the body was standing, and stayed standing.
    pub at:      Place,
    pub goal:    Place,
    /// How many steps in a row left it there — the client's whole patience.
    pub stalled: u8,
}

/// The journal stopped writing at its own size cap.
///
/// Written **into the file**, because the alternative is a reader guessing: a
/// journal that ends mid-session looks exactly like a client that was killed,
/// and the two want different next questions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Closure {
    /// What had been written when it stopped.
    pub bytes:  u64,
    /// The cap it reached.
    pub cap:    u64,
    /// What is in the file: destinations named, and searches that answered
    /// them.
    pub orders: u32,
    pub plans:  u32,
}

/// What one search did, reported the way the search itself reports it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Probe {
    /// Whether the route reached the goal, rather than merely getting nearer.
    pub arrived:  bool,
    /// How the bounded search stopped.
    pub exit:     Exit,
    /// Standing places it finalised — what the budget counts.
    pub explored: usize,
    /// Standing places it wrote down: the finalised ones and the frontier over
    /// them.
    pub written:  usize,
    /// How the long-route query ended, when the bounded search did not answer
    /// and the coarse graph was asked. Absent when it was not.
    pub long:     Option<LongEnd>,
}

/// Why a bounded search stopped — `openshard_movement::SearchExit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exit {
    /// The goal was reached.
    Goal,
    /// Everything reachable was settled and the goal was not among it. No
    /// budget would have changed the answer.
    Exhausted,
    /// The node budget ran out first. A bigger one might have arrived.
    Budget,
}

/// How a long-route query ended — `openshard_movement::LongExit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LongEnd {
    /// A corridor was found and walked.
    Route,
    /// Both endpoints are on the graph and no chain of portals joins them —
    /// two islands, which is a real "there is no way".
    NoCorridor,
    /// An endpoint the graph has no region for.
    OffGraph,
    /// An endpoint that joined no portal.
    NoJoin,
    /// Every portal the corridor offered was tried.
    PortalsExhausted,
    /// The query's own effort ran out.
    Spent,
}

/// Why a route does not end where it was asked to — the client's `Refusal`,
/// which is what a player is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// There is no way there for a body that walks.
    Nowhere,
    /// A way may exist; this search did not get to it from here.
    TooFar,
    /// The only way through is a shut door.
    Barred,
    /// Too far for a bounded search, with no coarse graph to divide it up.
    NoGraph,
}

/// A place to stand: a tile **and** a height, because a column can hold two
/// floors and an order to one of them is not an order to the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub x: u16,
    pub y: u16,
    pub z: i8,
}

impl Place {
    /// The point, written down.
    #[must_use]
    pub const fn of(point: Point) -> Self {
        Self {
            x: point.x,
            y: point.y,
            z: point.z,
        }
    }

    /// The point back, for a replay that is about to ask the same question.
    #[must_use]
    pub const fn point(self) -> Point {
        Point::new(self.x, self.y, self.z)
    }
}

/// One step of a route.
///
/// Spelled the way a map is read — `"N"`, `"NE"` — because the first reader of
/// a journal is a person looking for the step where a route turned into
/// something silly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    #[serde(rename = "N")]
    North,
    #[serde(rename = "NE")]
    NorthEast,
    #[serde(rename = "E")]
    East,
    #[serde(rename = "SE")]
    SouthEast,
    #[serde(rename = "S")]
    South,
    #[serde(rename = "SW")]
    SouthWest,
    #[serde(rename = "W")]
    West,
    #[serde(rename = "NW")]
    NorthWest,
}

impl Step {
    /// The direction, written down.
    #[must_use]
    pub const fn of(direction: Direction) -> Self {
        match direction {
            Direction::North => Self::North,
            Direction::NorthEast => Self::NorthEast,
            Direction::East => Self::East,
            Direction::SouthEast => Self::SouthEast,
            Direction::South => Self::South,
            Direction::SouthWest => Self::SouthWest,
            Direction::West => Self::West,
            Direction::NorthWest => Self::NorthWest,
        }
    }

    /// The direction back, for a replay that walks the recorded route over
    /// ground of its own.
    #[must_use]
    pub const fn direction(self) -> Direction {
        match self {
            Self::North => Direction::North,
            Self::NorthEast => Direction::NorthEast,
            Self::East => Direction::East,
            Self::SouthEast => Direction::SouthEast,
            Self::South => Direction::South,
            Self::SouthWest => Direction::SouthWest,
            Self::West => Direction::West,
            Self::NorthWest => Direction::NorthWest,
        }
    }
}

/// A route as one line of text — `"N NE E E"` — for a report a person reads.
///
/// Not the serialised form: the file keeps the list, so that a reader can index
/// into it. This is what a replay prints.
#[must_use]
pub fn route_text(steps: &[Step]) -> String {
    if steps.is_empty() {
        return "(none)".to_owned();
    }
    let mut text = String::with_capacity(steps.len() * 3);
    for (index, step) in steps.iter().enumerate() {
        if index > 0 {
            text.push(' ');
        }
        text.push_str(match step {
            Step::North => "N",
            Step::NorthEast => "NE",
            Step::East => "E",
            Step::SouthEast => "SE",
            Step::South => "S",
            Step::SouthWest => "SW",
            Step::West => "W",
            Step::NorthWest => "NW",
        });
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every direction survives the round trip through the file's own spelling.
    /// A step written one way and read back another is a replay walking a
    /// different route than the one that was recorded, which is the one failure
    /// this crate must not have.
    #[test]
    fn every_direction_comes_back_the_way_it_went_in() {
        for direction in Direction::ALL {
            let step = Step::of(direction);
            let json = serde_json::to_string(&step).expect("a step is one word");
            let read: Step = serde_json::from_str(&json).expect("the word it just wrote");
            assert_eq!(read.direction(), direction, "{direction:?} came back as {read:?}");
        }
    }

    /// And a place: the height is signed and the coordinates are not, which is
    /// the one shape of bug a hand-written format gets wrong.
    #[test]
    fn a_place_survives_the_round_trip_with_its_sign() {
        for point in [
            Point::new(0, 0, 0),
            Point::new(1340, 1676, 52),
            Point::new(u16::MAX, u16::MAX, i8::MIN),
            Point::new(1, 2, i8::MAX),
        ] {
            let json = serde_json::to_string(&Place::of(point)).expect("three numbers");
            let read: Place = serde_json::from_str(&json).expect("the three it just wrote");
            assert_eq!(read.point(), point);
        }
    }

    /// The route a person reads is the route the file holds, in travel order.
    #[test]
    fn a_route_reads_as_the_steps_in_order() {
        assert_eq!(route_text(&[]), "(none)");
        assert_eq!(route_text(&[Step::North, Step::SouthWest, Step::East]), "N SW E");
    }
}
