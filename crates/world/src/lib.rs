//! The simulation: game loop, spatial index, and composition of every gameplay system.
//!
//! # What is here
//!
//! The tick, the components a character is made of, the sector grid that
//! answers "what is near this point", and the client's map files.
//!
//! [`World::tick`] is the deterministic half of the boundary the gateway's
//! channel draws: commands queue in from network tasks, are applied in a fixed
//! order at a fixed rate, and packets come out. Nothing inside it awaits, reads
//! a clock, or touches a socket.
//!
//! The gameplay systems are not written yet.
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

pub mod components;
pub mod events;
pub mod map;
mod rng;
pub mod sectors;
mod skills;
pub mod terrain;
pub mod tick;
pub mod tiledata;
pub mod uop;

pub use components::{
    Account, Amount, Body, Brain, Client, Combat, Contained, Container, CriminalUntil, DamageType,
    Decays, Equipped, Facet, Graphic, Heading, Hitpoints, Mana, MeleeDamage, Movement, Name,
    Position, Resistance, Skills, Stackable, Stats, SwingSpeed,
};
pub use events::{
    ItemSpawned, MobileDamaged, MobileDied, MobileMoved, MobileSpoke, MobileTurned, PlayerEntered,
    PlayerLeft, RefusedReason, SkillUsed, SpellCast, StepRefused,
};
pub use map::{LandCell, Map, MapError, StaticItem, BLOCK_SIZE};
pub use sectors::{distance, in_range, Sectors, SECTOR_SIZE, VIEW_RANGE};
pub use terrain::{MapTerrain, MAX_STEP_DOWN, MAX_STEP_UP, PLAYER_HEIGHT};
pub use tick::{Appearance, Command, Outbound, World, TICK_INTERVAL};
pub use tiledata::{LandTile, StaticTile, TileData, TileDataError, TileDataFormat, TileFlags};
pub use uop::UopError;
