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

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_protocol::casting::SpellId;
use openshard_protocol::containers::{
    BOOK_GUMP, ContainedItem, ContainerContents, GridSlot, encode_add_to_container, encode_open_container,
};
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::items::{DragCancel, DragCancelReason, DropDestination, EquipUpdate};
use openshard_protocol::mobile::{OpenPaperdoll, PaperdollFlags, Remove};
use openshard_protocol::serial::{RawSerial, Serial, SerialKind};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::spellbook::SpellbookContent;
use openshard_protocol::target::{TargetCursor, TargetKind};
use openshard_protocol::trade::{encode_trade_close, encode_trade_open, encode_trade_update};
use openshard_protocol::wire::{ClilocId, CursorId, Graphic, Hue, Layer, RawLayer, SoundId};
use openshard_protocol::world::{Facet, Point};
use openshard_state::components::{
    Amount, Body, Client, Combat, Contained, Container, Corpse, Decays, Decoration, Door, Drawn, Equipped,
    Ghost, KeyValue, Lock, Name, PoisonCharges, Position, RUNEBOOK_ENTRIES, RUNEBOOK_GRAPHIC, Ridden, Riding,
    RuneMark, Runebook, RunebookEntry, SPELLBOOK_GRAPHIC, Seated, Spellbook, Stackable, Weapon,
    mount_item_for, scroll_spell,
};
use openshard_state::sectors::in_range;
use openshard_state::{HeldItem, Occupant, Origin, Outbound, TICKS_PER_SECOND, TradeWindow, WorldState};
use tracing::{debug, warn};

mod backpack;
mod capacity;
mod consume;
mod containers;
mod decay;
mod defaults;
mod doors;
mod drag;
mod equip;
mod mounts;
mod seating;
mod spawn;
mod stack;
mod trade;
mod trigger;
mod weight;

pub use backpack::*;
pub use capacity::*;
pub use consume::*;
pub use containers::*;
pub use decay::*;
pub use defaults::{apply_core_defaults, restore_uses, uses_left};
pub use doors::*;
pub use drag::*;
pub use equip::*;
pub use mounts::*;
pub use seating::*;
pub use spawn::*;
pub use stack::*;
pub use trade::*;
pub use trigger::*;
pub use weight::*;
