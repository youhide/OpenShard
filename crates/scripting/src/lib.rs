//! The scripting runtime: TypeScript/JavaScript in-process, behind a narrow seam.
//!
//! # What this crate is
//!
//! The spike for roadmap §5 — the largest open technical risk in the project.
//! The question it answers is not "can we run JavaScript" but "can we run it
//! *inside a tick*": at 20Hz a tick has 50ms, and if a script hook has to fire
//! for a thousand mobiles every one of those, the per-call cost has to be small
//! enough that `entities × cost` leaves room for everything else a tick does.
//! [`DenoEngine`] is the answer and `examples/benchmark.rs` is the measurement.
//!
//! # The seam
//!
//! A script is not a new kind of thing wired through the engine. It is one more
//! consumer of the same two channels every system already uses:
//!
//! - **Domain events in.** [`ScriptEngine::deliver`] hands the script a
//!   [`Event`] — `PlayerEntered`, `MobileMoved`, `StepRefused`, `PlayerLeft` —
//!   exactly as the client and persistence receive the same facts. The engine
//!   also keeps a small read model from these, so a hook can read where a mobile
//!   *is* without a round-trip into the world.
//! - **Commands out.** A script never writes the world. It enqueues a
//!   [`Command`], drained with [`ScriptEngine::take_commands`], and the tick
//!   applies it in order — the same rule the network layer lives by. Reads are
//!   direct; writes go through the queue.
//!
//! [`ScriptEngine`] is deliberately small. Nothing in its signatures is
//! V8-shaped: no isolate, no `deno_core` type, no `v8::Local`. That is the graded
//! constraint of the spike — the runtime behind the trait has to be replaceable,
//! so `deno_core` lives *entirely* inside [`DenoEngine`] and never leaks past it.

mod engine;

pub use engine::DenoEngine;

/// A wire serial: the identity every packet about a mobile already carries, and
/// so the identity a script names an entity by.
///
/// A plain `u32` on purpose. The scripting layer has no opinion about how the
/// world stores entities — it speaks the same identity the protocol does, and
/// the glue that owns both maps one to the other.
pub type Serial = u32;

/// Something the world says happened, handed to a script.
///
/// These mirror the world's domain events rather than re-inventing them: a
/// script is another reader of the same bus. The engine both forwards each to
/// the script's handler and updates its own read model from it, which is what
/// lets a hook read a mobile's position without asking the world.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(tag = "type")]
pub enum Event {
    /// A character entered the world.
    PlayerEntered {
        /// Its wire identity.
        serial: Serial,
        /// Where it appeared.
        x: u16,
        /// Where it appeared.
        y: u16,
        /// Where it appeared.
        z: i8,
    },
    /// A mobile took a step.
    MobileMoved {
        /// Its wire identity.
        serial: Serial,
        /// Where it is now.
        x: u16,
        /// Where it is now.
        y: u16,
        /// Where it is now.
        z: i8,
        /// Which way it now faces.
        facing: u8,
    },
    /// A step was refused — what a speedhack looks like from outside.
    StepRefused {
        /// Its wire identity.
        serial: Serial,
        /// Why, as the world's `RefusedReason` discriminant.
        reason: u8,
    },
    /// A character left the world.
    PlayerLeft {
        /// Its wire identity, now released.
        serial: Serial,
    },
    /// A mobile died — combat's headline event, for loot, notoriety and quests.
    MobileDied {
        /// Its wire identity.
        serial: Serial,
    },
}

/// What a script asks the world to do.
///
/// A script never touches the world directly; it enqueues one of these and the
/// tick applies it, in order, on the tick's thread. The vocabulary is small on
/// purpose — the spike proves the path, gameplay (§6) fills it in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    /// Move a mobile one step in a direction (0–7, N clockwise), the world to
    /// validate it exactly as it validates a client's step.
    Move {
        /// Whom to move.
        serial: Serial,
        /// Which way (0–7).
        direction: u8,
    },
    /// Put an item on the ground for the world to draw to everyone in range.
    SpawnItem {
        /// The tiledata graphic id.
        graphic: u16,
        /// Its hue, or 0 for none.
        hue: u16,
        /// How many, for a stackable item; 0 or 1 is a single.
        amount: u16,
        /// Whether it merges with an identical pile when dropped onto one.
        stackable: bool,
        /// Where it lies.
        x: u16,
        /// Where it lies.
        y: u16,
        /// Where it lies.
        z: i8,
        /// Which facet.
        facet: u8,
    },
    /// Put a container on the ground — an item others can be put inside.
    SpawnContainer {
        /// The tiledata graphic id.
        graphic: u16,
        /// The gump the client opens when it is double-clicked.
        gump: u16,
        /// Its hue, or 0 for none.
        hue: u16,
        /// Where it lies.
        x: u16,
        /// Where it lies.
        y: u16,
        /// Where it lies.
        z: i8,
        /// Which facet.
        facet: u8,
    },
    /// Put a mobile in the world — a creature to fight or an NPC to stand there.
    SpawnMobile {
        /// The body graphic.
        body: u16,
        /// Its hue.
        hue: u16,
        /// Its starting and maximum hit points.
        hits: u16,
        /// Its standing (health-bar colour) as a wire byte: 1 innocent, 5 enemy,
        /// 7 invulnerable.
        notoriety: u8,
        /// How hard it hits in melee, before the target's armour.
        damage: u16,
        /// Its physical resistance, 0–100.
        resistance: u8,
        /// Where it stands.
        x: u16,
        /// Where it stands.
        y: u16,
        /// Where it stands.
        z: i8,
        /// Which facet.
        facet: u8,
    },
    /// Deal damage to a mobile.
    Damage {
        /// Whom.
        serial: Serial,
        /// How much.
        amount: u16,
    },
}

/// Why a script call failed.
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    /// The script did not compile or threw while evaluating.
    #[error("script evaluation failed: {0}")]
    Evaluate(String),
    /// A hook threw when called.
    #[error("script hook `{hook}` threw: {message}")]
    Hook {
        /// Which hook.
        hook: &'static str,
        /// The exception message.
        message: String,
    },
}

/// A scripting runtime, narrow enough to be swapped.
///
/// The whole surface: evaluate a script (again, to hot-reload), deliver a domain
/// event, run the per-tick hook for one entity, and take the commands the script
/// wants applied. A backend that is not V8 could implement exactly this.
pub trait ScriptEngine {
    /// Evaluate `source`, replacing whatever was loaded before.
    ///
    /// This *is* hot reload: calling it again with new source rebinds the hooks
    /// in the live runtime, no restart. Setup code in the script runs now; the
    /// exported `onTick` / `onEvent` functions are captured for later calls.
    fn load(&mut self, source: &str) -> Result<(), ScriptError>;

    /// Forward a domain event to the script and fold it into the read model.
    ///
    /// Calls the script's `onEvent` if it exports one. Either way the engine's
    /// own view of where mobiles are is updated, so a later [`tick`](Self::tick)
    /// can read it.
    fn deliver(&mut self, event: &Event) -> Result<(), ScriptError>;

    /// Run the per-tick hook for one entity, if the script exports `onTick`.
    ///
    /// Synchronous by contract: this is called from inside a tick, and a tick
    /// never awaits. A hook that reads state does so through a direct op; a hook
    /// that changes the world enqueues a [`Command`].
    fn tick(&mut self, entity: Serial) -> Result<(), ScriptError>;

    /// Take the commands enqueued since the last drain.
    fn take_commands(&mut self) -> Vec<Command>;
}
