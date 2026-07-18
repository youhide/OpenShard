//! What the world says happened.
//!
//! # These live here, not in `openshard-events`
//!
//! That crate is machinery: `Events<E>`, `Cursor<E>`, a bus. It defines no game
//! events and must not, or every crate ends up depending on a file every other
//! crate edits.
//!
//! So a domain event lives with the rule that emits it. These are the world's,
//! because the world's tick is what decides a player moved. `NpcKilled` will
//! belong to combat; `HouseCreated` to housing.

use openshard_entities::{EntityId, Serial};
use openshard_protocol::{Facing, Point};

/// A character entered the world.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlayerEntered {
    /// The entity.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Where it appeared.
    pub position: Point,
}

// `ItemSpawned` moved to `openshard-items` with the item system that emits it.
// `world` re-exports it.

/// A client asked to cast a spell — from its spellbook or a macro.
///
/// The request off the wire, no more: what the spell *costs* and *does* — mana,
/// reagents, damage — is a script's, read off this event, the same script-first
/// decoupling `MobileSpoke` and `SkillUsed` have. The world hears "this mobile
/// wants spell N" and says so; a script turns that into an actual cast.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpellRequested {
    /// The would-be caster.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Which spell, zero-based.
    pub spell: u16,
}

/// A creature or NPC appeared in the world.
///
/// The mobile counterpart of [`PlayerEntered`], for the mobiles no client drives
/// — a spawned creature. A script reads it to learn a fresh mobile's serial, the
/// name it needs to take control of it (`op_control`) or act on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MobileSpawned {
    /// The entity.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Where it appeared.
    pub position: Point,
}

/// A mobile took a step.
///
/// Emitted for the step, not for the turn: a turn changes no tile, and a
/// listener that cares about *where* things are should not have to filter out
/// events where nothing went anywhere.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MobileMoved {
    /// The entity.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Where it was.
    pub from: Point,
    /// Where it is.
    pub to: Point,
    /// Which way it now faces.
    pub facing: Facing,
}

/// A mobile turned on the spot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MobileTurned {
    /// The entity.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Which way it now faces.
    pub facing: Facing,
}

/// A step was refused.
///
/// Worth an event rather than a log line: this is what a speedhack looks like
/// from the outside, and metrics and a GM tool both want to count it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StepRefused {
    /// The entity.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Why.
    pub reason: RefusedReason,
}

/// Why a step was refused.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum RefusedReason {
    /// The client's walk sequence was out of step.
    OutOfSequence,
    /// Something is in the way, or the ground is not there.
    Blocked,
    /// The client is moving faster than a body can move.
    TooFast,
}

/// A character left the world.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlayerLeft {
    /// The entity, now despawned.
    pub entity: EntityId,
    /// Its wire identity, now released.
    pub serial: Serial,
}

// `MobileDamaged` and `MobileDied` moved to `openshard-combat` with the combat
// system that emits them. `world` re-exports both.

// `SkillUsed` moved to `openshard-skills` with the skill system that emits it.
// `world` re-exports it.

// `SpellCast` moved to `openshard-magic` with the casting system that emits it.
// `world` re-exports it.

// `MobileSpoke` moved to `openshard-chat` with the speech system that emits it —
// "domain events live with the crate that owns the rule". `world` re-exports it.
