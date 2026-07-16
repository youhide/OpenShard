//! Movement, pathfinding, line of sight, and fastwalk prevention.
//!
//! # Sans-io, like everything else on this path
//!
//! A [`Walker`] takes a `0x02` and returns a [`Walk`]. No sockets, no world, no
//! clock. The caller turns the outcome into `0x22` or `0x21`.
//!
//! ```
//! use std::time::Instant;
//! use openshard_movement::{OpenWorld, Walk, Walker};
//! use openshard_protocol::{Direction, Facing, Point, WalkRequest};
//!
//! let mut walker = Walker::new(Point::new(100, 100, 0), Facing::walking(Direction::North));
//!
//! let step = WalkRequest {
//!     facing: Facing::walking(Direction::North),
//!     sequence: 0,
//!     fastwalk_key: 0,
//! };
//! assert!(matches!(walker.request(step, &OpenWorld, Instant::now()), Walk::Moved { .. }));
//! ```
//!
//! # What is here and what is not
//!
//! The walk *handshake*: the sequence rules, turning as a step, the world edge.
//! And [`WalkPace`], which decides how often a step is allowed.
//!
//! [`Terrain`] — whether a tile can be stood on — is a trait, because the answer
//! needs the client's map files, the statics, the multis and every other mobile,
//! and none of that belongs here. `openshard-world` implements it;
//! [`OpenWorld`] is what a shard with no client files runs.
//!
//! # Fastwalk
//!
//! The `0x02` fastwalk key is ignored. It was a 1999 attempt at stopping speed
//! hacks, was broken almost immediately, and Sphere stopped reading it. The
//! defence that works is server-side: see [`WalkPace`].

mod pace;
mod sequence;
mod walk;

pub use pace::{Pace, WalkPace, RUN_INTERVAL, WALK_BUFFER, WALK_INTERVAL};
pub use sequence::{OutOfSequence, WalkSequence};
pub use walk::{step_from, OpenWorld, Terrain, Walk, Walker};
