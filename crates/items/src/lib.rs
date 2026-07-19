//! Items: spawning, the drag protocol, stacking, decay, containers, and gear.
//!
//! A gameplay system in its own crate, operating on the shared [`WorldState`].
//! An item is an entity in exactly one of three places — on the ground
//! ([`Position`]), inside a container ([`Contained`]), or worn ([`Equipped`]) —
//! and these functions move it between them: spawn it, lift it onto a cursor,
//! drop it, stack or split it, decay it, put it in a container, wear it. Reach
//! and layer checks are server-authoritative; the client's word is never taken.
//!
//! The drawing goes through [`WorldState`]'s interest machinery (`reveal`,
//! `show`, `forget`); this crate owns the *rules* of where a thing is.

use openshard_entities::{EntityId, Serial, SerialKind};
use openshard_gateway::ConnectionId;
use openshard_protocol::{
    encode_add_to_container, encode_container_contents, encode_drag_cancel, encode_equip,
    encode_open_container, encode_open_paperdoll, encode_remove, ContainedItem, DragCancelReason,
    Point, DROP_TO_GROUND, PAPERDOLL_CAN_LIFT, PAPERDOLL_WARMODE,
};
use openshard_state::components::{
    mount_item_for, Amount, Body, Client, Combat, Contained, Container, Decays, Decoration, Door,
    Equipped, Facet, Graphic, Name, Position, Ridden, Riding, Stackable,
};
use openshard_state::sectors::in_range;
use openshard_state::{HeldItem, Origin, Outbound, WorldState, TICKS_PER_SECOND};
use tracing::{debug, warn};

mod containers;
mod decay;
mod doors;
mod drag;
mod equip;
mod mounts;
mod spawn;
mod stack;

pub use containers::*;
pub use decay::*;
pub use doors::*;
pub use drag::*;
pub use equip::*;
pub use mounts::*;
pub use spawn::*;
pub use stack::*;
