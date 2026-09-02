//! The three faces of a lock arrow, shared by every window that draws one.
//!
//! ClassicUO's `GetStatusButtonGraphic` and `GetStatLockGraphic` are the same
//! three graphics under two names — the skill sheet's arrow and the status
//! frame's are one control in the client files, and a second copy of the three
//! ids in a second module is exactly the kind of duplicate that drifts: a shard
//! serving a file set where one of them moved would fix the sheet and leave the
//! status frame drawing the old art.

use openshard_protocol::skill::SkillLock;
use openshard_protocol::wire::Graphic;

/// Trained up: the arrow points at the ceiling.
pub const UP: Graphic = Graphic(0x0984);
/// Trained down.
pub const DOWN: Graphic = Graphic(0x0986);
/// Held where it is — a padlock rather than an arrow.
pub const HELD: Graphic = Graphic(0x082C);

/// The picture for one arrow.
#[must_use]
pub const fn art(lock: SkillLock) -> Graphic {
    match lock {
        SkillLock::Up => UP,
        SkillLock::Down => DOWN,
        SkillLock::Locked => HELD,
    }
}
