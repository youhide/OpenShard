//! The world's runtime state: the data every system reads and writes.
//!
//! # Why this crate exists
//!
//! A gameplay system — combat, chat, skills — is a function over the world's
//! state: it reads components, rolls the world's generator, asks who is near a
//! point, and writes the result back. For those functions to live in their own
//! crates (`combat`, `chat`, …) rather than piling into one file, the state they
//! operate on has to sit *below* them in the dependency graph, in a crate they
//! can all depend on without depending on each other or on the tick that
//! sequences them.
//!
//! That is this crate. It owns the vocabulary of world state and nothing about
//! *when* it changes:
//!
//! - [`components`] — what a thing in the world is made of. Position, hit points,
//!   a combat stance, a skill map; a thing's identity is which of these it
//!   carries.
//! - [`Sectors`] — the spatial index that answers "what is near this point",
//!   Chebyshev distance, the square region a UO client draws.
//! - [`Rng`] — the seeded generator behind every roll. Deterministic on purpose:
//!   advanced only by the tick, never the OS, so a world replays roll for roll.
//!
//! The tick that drives all this, and the systems that act on it, live above.

pub mod components;
pub mod rng;
pub mod runtime;
pub mod sectors;

pub use components::{
    Account, Amount, Body, Brain, Client, Combat, Contained, Container, CriminalUntil, DamageType,
    Decays, Equipped, Facet, Graphic, Heading, Hitpoints, Mana, MeleeDamage, Movement, Murders,
    Name, Position, Resistance, Scripted, Skills, Stackable, Stats, SwingSpeed,
};
pub use rng::Rng;
pub use runtime::{FacetState, Gameplay, HeldItem, Origin, Outbound, WorldState, TICKS_PER_SECOND};
pub use sectors::{distance, in_range, Sectors, SECTOR_SIZE, VIEW_RANGE};
