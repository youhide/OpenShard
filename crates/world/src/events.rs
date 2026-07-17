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

/// An item appeared in the world.
///
/// Emitted when the server puts a thing on the ground — the item counterpart of
/// [`PlayerEntered`]. What a script or persistence does with it is their affair;
/// the world's part is only to say it happened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemSpawned {
    /// The entity.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Where it lies.
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

/// A mobile took damage.
///
/// Emitted whenever hit points fall — the hook combat gives everything that
/// cares without combat having to know who does: a health bar redraw, an
/// aggression tracker, a script that heals its pet. This is the crate boundary
/// the architecture is built on — combat says what happened and moves on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MobileDamaged {
    /// The mobile.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// How much it lost.
    pub amount: u16,
    /// What it has left.
    pub remaining: u16,
}

/// A mobile used a skill: the check resolved, and any gain is already applied.
///
/// What the *use* accomplishes is not decided here — whether the ore comes out
/// of the rock, whether the lockpick turns — only whether the roll passed and
/// where the skill stands now. A script reads this and grants the reward, the
/// same decoupling combat's `MobileDied` has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SkillUsed {
    /// The mobile.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Which skill, by id.
    pub skill: u8,
    /// Whether the check succeeded.
    pub success: bool,
    /// The skill's value now, in tenths, after any gain.
    pub value: u16,
}

/// A spell was cast: the mana was paid and the skill rolled. What the spell
/// *does* is a script's to decide — this only says who cast what at whom, and
/// whether it took. A fireball's damage, a heal's mending, a summon's creature
/// all hang off this event, none of them known to the casting machinery.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpellCast {
    /// The caster.
    pub caster: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Which spell, by id.
    pub spell: u16,
    /// The target's serial, or zero for a spell that needs none.
    pub target: u32,
    /// Whether the cast succeeded (mana paid and the skill check passed).
    pub success: bool,
}

/// A mobile said something.
///
/// The hook chat hangs everything off: a GM command, an NPC that answers its
/// name, a keyword that starts a quest. Combat's decoupling once more — the
/// speaker only says it happened; whoever cares reads the words. Carries an owned
/// `String`, so unlike most events it is not `Copy`; the bus never needed it to
/// be.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MobileSpoke {
    /// The speaker.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// What was said.
    pub text: String,
}

/// A mobile died — its hit points reached zero.
///
/// The event the whole "systems emit, they do not call" rule is named for:
/// combat emits this, and loot, notoriety, guild war scores and quests read it,
/// none of them wired into combat. What death *does* — a corpse, a ghost, a
/// resurrection — is not decided here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MobileDied {
    /// The mobile.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
}
