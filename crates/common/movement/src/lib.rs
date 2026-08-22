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
//! use openshard_protocol::world::{Point, RawFastwalkKey, RawStepSequence, WalkRequest};
//! use openshard_protocol::direction::{Direction, Facing};
//!
//! let mut walker = Walker::new(Point::new(100, 100, 0), Facing::walking(Direction::North));
//!
//! let step = WalkRequest {
//!     facing: Facing::walking(Direction::North),
//!     sequence: RawStepSequence(0),
//!     fastwalk_key: RawFastwalkKey(0),
//! };
//! assert!(matches!(walker.request(step, &OpenWorld, Instant::now(), false), Walk::Moved { .. }));
//! ```
//!
//! # What is here and what is not
//!
//! The walk *handshake*: the sequence rules, turning as a step, the world edge.
//! And [`WalkPace`], which decides how often a step is allowed.
//!
//! [`Terrain`] — whether a tile can be stood on — is a trait, because the answer
//! needs the client's map files, the statics, the multis and every other mobile.
//! [`MapTerrain`] is the static half — the map and `tiledata.mul`, nothing else —
//! shared between the server tick and the client's own click-to-walk planner;
//! [`OpenWorld`] is what a shard with no client files runs. The dynamic half —
//! doors, placed items, other mobiles — is `openshard-state::obstruct`, which
//! stays server-side because it needs the entity registry.
//!
//! And two ways of getting somewhere, which answer different questions.
//! [`find_path`] needs a destination and searches for a route to it.
//! [`Detour`] needs neither: it takes a direction a body is already walking and
//! the four tiles around it that decide the next step, and answers with where
//! that body actually goes — the way past what is directly in the way, or
//! nothing when there is no way past. A heading has no destination to plan to,
//! which is why the second exists.
//!
//! # Fastwalk
//!
//! The `0x02` fastwalk key is ignored. It was a 1999 attempt at stopping speed
//! hacks, was broken almost immediately, and Sphere stopped reading it. The
//! defence that works is server-side: see [`WalkPace`].

pub mod bake;
mod cache;
mod detour;
pub mod door_frames;
mod navigation;
mod pace;
mod path;
pub mod scene;
mod sequence;
mod terrain;
mod walk;

pub use cache::{CachedTerrain, TransitionCacheStats};
pub use detour::{Around, Detour, Leeway, Step};
pub use navigation::{NavigationGraph, find_long_path};
pub use openshard_map::map::LandTile;
pub use pace::{
    Pace, RUN_HOLD, RUN_INTERVAL, WALK_BUFFER, WALK_HOLD, WALK_INTERVAL, WalkPace, step_hold, step_progress,
};
pub use path::{find_path, find_path_toward};
pub(crate) use path::{find_path_toward_until, find_path_until};
pub use sequence::{OutOfSequence, StepCounter, WalkSequence};
pub use terrain::{MAX_STEP_UP, MapTerrain, PLAYER_HEIGHT};
pub use walk::{
    Heading, Intent, Lean, OpenWorld, Terrain, Tile, Walk, Walker, direction_toward, heading_toward, intend,
    line_tiles, step_allowed, step_from,
};
