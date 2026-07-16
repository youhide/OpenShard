//! Typed, deterministic, double-buffered event bus with independent reader cursors.
//!
//! Systems in OpenShard do not call each other. Combat does not call the guild
//! system to update war scores; it emits `NpcKilled` and moves on. Whoever cares
//! reads it. That is what keeps crates decoupled, plugins possible, and logging,
//! metrics and replay free rather than bolted on.
//!
//! ```
//! use openshard_events::EventBus;
//!
//! struct SkillGain { skill: u16, amount: u16 }
//!
//! let mut bus = EventBus::new();
//!
//! // Each reader keeps its own cursor, taken once at startup.
//! let mut metrics = bus.cursor::<SkillGain>();
//! let mut journal = bus.cursor::<SkillGain>();
//!
//! bus.send(SkillGain { skill: 1, amount: 5 });
//!
//! // Reading does not consume: both readers see the same event.
//! assert_eq!(bus.read(&mut metrics).count(), 1);
//! assert_eq!(bus.read(&mut journal).count(), 1);
//!
//! // The game loop ticks the bus once, after every system has run.
//! bus.update();
//! ```
//!
//! # Why not callbacks
//!
//! A subscription model would mean the bus owns handlers, handlers own state,
//! and emitting an event runs arbitrary code at an unpredictable point in the
//! tick. Reentrancy, ordering bugs, and a simulation that stops being
//! deterministic. Here, sending an event only pushes to a `Vec`; reading happens
//! where the reader chooses. The tick order is whatever the game loop says it is,
//! and a replay of the same events produces the same world.
//!
//! # Ordering guarantee
//!
//! Events live for two ticks, not one. A system that runs *before* the emitter
//! within a tick still sees the event on the next tick rather than missing it
//! forever, so system order stops being load-bearing. See [`Events`].
//!
//! # Layering
//!
//! This crate is machinery only — it defines no game events. `PlayerLogin` lives
//! with login, `NpcKilled` with combat, `HouseCreated` with housing. Each event
//! type belongs to the crate that owns the rule that emits it, which is what
//! stops this crate from becoming a dependency hub that every other crate has to
//! agree on.

mod bus;
mod queue;

pub use bus::EventBus;
pub use queue::{Cursor, Event, Events};
