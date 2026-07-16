//! Entity Component System core plus UO serial identity.
//!
//! Everything in an OpenShard world is an entity: players, NPCs, items, houses,
//! boats, projectiles. None of them are subclasses of each other. What a thing
//! *is* falls out of which components it carries.
//!
//! ```
//! use openshard_entities::{Registry, SerialKind};
//!
//! struct Position { x: i32, y: i32 }
//! struct Health(u32);
//!
//! let mut world = Registry::new();
//!
//! // A mobile gets a serial, because the client has to address it.
//! let (goblin, serial) = world.spawn_with_serial(SerialKind::Mobile).unwrap();
//! world.insert(goblin, Position { x: 100, y: 200 });
//! world.insert(goblin, Health(40));
//!
//! // Incoming packets name entities by serial.
//! assert_eq!(world.entity_of(serial), Some(goblin));
//! assert_eq!(world.get::<Health>(goblin).unwrap().0, 40);
//! ```
//!
//! # Why this shape
//!
//! **Generational handles.** [`EntityId`] survives its entity's death: every
//! lookup validates the generation, so a stale handle reads `None` instead of
//! silently addressing whatever recycled the slot. Long-lived references to
//! entities are unavoidable in a UO server — a corpse remembers its killer, a
//! pet remembers its owner — so this has to be safe by construction rather than
//! by discipline.
//!
//! **Sparse-set columns.** Components come and go constantly here (an item
//! picked up loses its world position; an NPC gains a combat target), so
//! per-component columns with O(1) add/remove beat archetype tables, which pay
//! for that churn by moving whole rows between archetypes.
//!
//! **No global state.** A [`Registry`] is a plain value. Nothing is a `static`,
//! nothing is a singleton, and a test can spin up as many worlds as it likes.
//!
//! # Layering
//!
//! This crate knows nothing about gameplay. It defines no `Position`, no
//! `Health`, no `Combat` — those live in the crates that own those rules. What
//! it provides is identity ([`EntityId`], [`Serial`]) and storage
//! ([`Registry`]).

mod component;
mod entity;
mod registry;
mod serial;

pub use component::{Component, Entities, Iter, IterMut, SparseSet};
pub use entity::EntityId;
pub use registry::{BindSerialError, Query, QueryMut, Registry, SpawnError};
pub use serial::{
    Serial, SerialAllocator, SerialKind, SerialPoolExhausted, ITEM_MAX, ITEM_MIN, MOBILE_MAX,
    MOBILE_MIN,
};
