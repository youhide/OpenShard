//! The simulation: game loop, spatial index, and composition of every gameplay system.
//!
//! # What is here
//!
//! The map, and only the map. Reading the client's `map*.mul`, `statics*.mul`
//! and `tiledata.mul`, and answering the one question movement asks: can a
//! mobile stand here.
//!
//! The tick, the spatial index and the systems are not written yet. This crate
//! is named for what it will hold; today it holds the part that unblocks
//! walking.
//!
//! # The client's files are the source of truth
//!
//! The server does not send map tiles — the client already has them, and has had
//! them since it was installed. What the server needs the map for is *deciding*:
//! how high the ground is, what blocks, what floats. If the two disagree the
//! client draws a wall the server lets you walk through, and the player watches
//! themselves rubber-band.
//!
//! So these parsers are not "reading a file format", they are agreeing with a
//! binary from 1997 about the shape of Britannia. Two things in them are not
//! stated anywhere in the files and will silently produce a plausible, wrong
//! world if guessed:
//!
//! - **Block order is column-major** — `bx * (height/8) + by`. See [`map`].
//! - **`tiledata.mul` has two layouts** and no version field. See [`tiledata`].
//!
//! Both are settled by arithmetic and pinned by tests against real files.

pub mod map;
pub mod terrain;
pub mod tiledata;
pub mod uop;

pub use map::{LandCell, Map, MapError, StaticItem, BLOCK_SIZE};
pub use terrain::{MapTerrain, MAX_STEP_DOWN, MAX_STEP_UP, PLAYER_HEIGHT};
pub use tiledata::{LandTile, StaticTile, TileData, TileDataError, TileDataFormat, TileFlags};
pub use uop::UopError;
