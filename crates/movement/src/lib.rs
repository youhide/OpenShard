//! Movement, pathfinding, line of sight, and fastwalk prevention.
//!
//! # Sans-io, like everything else on this path
//!
//! A [`Walker`] takes a `0x02` and returns a [`Walk`]. No sockets, no world, no
//! clock. The caller turns the outcome into `0x22` or `0x21`.
//!
//! ```
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
//! assert!(matches!(walker.request(step, &OpenWorld), Walk::Moved { .. }));
//! ```
//!
//! # What is here and what is not
//!
//! The walk *handshake* is done: the sequence rules, turning as a step, the
//! world edge. What is not is [`Terrain`] — knowing whether a tile can be stood
//! on needs the client's map files, the statics, the multis and every other
//! mobile. [`OpenWorld`] stands in: no floor, no walls, every step allowed.
//!
//! That split is deliberate. The handshake is protocol and can be finished and
//! pinned now; terrain is a project of its own, and the trait means it lands
//! without touching any of this.
//!
//! # Fastwalk
//!
//! There is none, and the `0x02` key field is ignored. It was a 1999 attempt at
//! stopping speed hacks, was broken almost immediately, and Sphere stopped
//! reading it. Real speed limiting is a server-side timer on how often a step
//! is allowed, which belongs with the tick and not here.

mod sequence;
mod walk;

pub use sequence::{OutOfSequence, WalkSequence};
pub use walk::{step_from, OpenWorld, Terrain, Walk, Walker};
