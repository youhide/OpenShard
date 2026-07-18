//! Ultima Online wire protocol: client versioning, feature gates, packet encode/decode.
//!
//! OpenShard aims to be compatible with the UO protocol, not with SphereServer.
//! That distinction is the whole design: the protocol is a fixed external
//! contract two decades of clients already implement, while Sphere's internals
//! are one team's answer to it and we are free to give a different one.
//!
//! # Multi-era from the start
//!
//! There is no single "the protocol". A 2.0 client and a 7.0.95 client speak
//! meaningfully different dialects, and a shard picks which ones it accepts.
//! Rather than hard-coding one era and retrofitting the rest later — which means
//! auditing every packet encoder twice — versioning is the first thing this
//! crate models.
//!
//! ```
//! use openshard_protocol::{ClientVersion, Era, Feature};
//!
//! // The client sends its version in the 0xBD seed packet.
//! let client: ClientVersion = "4.0.3.0".parse().unwrap();
//!
//! assert_eq!(client.era(), Era::Aos);
//! assert!(client.supports(Feature::Tooltips));
//! assert!(!client.supports(Feature::TooltipHash));
//! ```
//!
//! # The rule
//!
//! Gameplay and encoder code asks [`ClientVersion::supports`]. It never compares
//! version numbers, and it never branches on [`Era`].
//!
//! Features did not land in era-sized batches — tooltips at 4.0.0a, stat locks
//! at 4.0.1a, tooltip hashes at 4.0.5a, all within "AoS" — so an era check is
//! wrong for most of the clients it covers, and wrong in the worst way: the
//! client drops the packet in silence rather than complaining. Keeping every
//! boundary in [`Feature::since`] means one table to fix when a boundary turns
//! out to be off by a patch.
//!
//! [`Era`] is for coarse decisions only: which map set to load, whether housing
//! is customisable.
//!
//! # Where the numbers come from
//!
//! The version boundaries are ported from SphereServer's `MINCLIVER_*` table.
//! That table is observed protocol behaviour — two decades of finding out which
//! client breaks on what — and it is the one part of Sphere worth carrying
//! across. The architecture around it is not.
//!
//! # Status
//!
//! Versioning, feature gates, the client packet length table, framing and the
//! byte codec are written. Individual packet types are not; see
//! `docs/roadmap.md`.

mod access;
mod casting;
mod codec;
mod combat;
mod containers;
mod direction;
mod feature;
pub mod huffman;
mod items;
mod login;
mod mobile;
mod packet;
mod seed;
mod speech;
mod version;
mod world;

pub use access::{AccessLevel, UnknownAccessLevel};
pub use casting::CastSpellRequest;
pub use codec::{CodecError, CodecResult, PacketReader, PacketWriter};
pub use combat::{encode_attack, encode_health, encode_war_mode, AttackRequest, WarModeRequest};
pub use containers::{
    encode_add_to_container, encode_container_contents, encode_open_container, ContainedItem,
    DoubleClick,
};
pub use direction::{Direction, Facing, RUNNING_BIT};
pub use feature::{Feature, FeatureSet};
pub use items::{
    encode_drag_cancel, encode_equip, DragCancelReason, DropItem, EquipItemRequest, PickUpItem,
    WorldItem, DROP_TO_GROUND,
};
pub use login::{
    encode_character_list, encode_login_denied, encode_relay, encode_shard_list, AccountLogin,
    CharacterEntry, ClientVersionReport, DenyReason, GameServerLogin, LoginDecodeError,
    SelectShard, ShardEntry, StartLocation, WrongPacket, ACCOUNT_NAME_LENGTH,
    CHARACTER_NAME_LENGTH, MAX_SHARDS, MIN_CHARACTER_SLOTS, PASSWORD_LENGTH, SHARD_NAME_LENGTH,
};
pub use mobile::{
    encode_open_paperdoll, encode_remove, Equipment, MobileIncoming, MobileMove, MobileStatus,
    Notoriety, StatusFlags, PAPERDOLL_CAN_LIFT, PAPERDOLL_WARMODE,
};
pub use packet::{
    client_packet_length, frame_client_packet, Frame, FrameError, PacketLength, MAX_PACKET_SIZE,
    SEED_LENGTH_NEW, SEED_LENGTH_OLD,
};
pub use seed::{Seed, SeedReader, SEED_COMMAND};
pub use speech::{
    encode_message, encode_unicode_message, TalkRequest, UnicodeTalkRequest, DEFAULT_LANGUAGE_TAG,
    NO_GRAPHIC, SYSTEM_SERIAL,
};
pub use version::{ClientVersion, Era, ParseVersionError};
pub use world::{
    encode_light_level, encode_login_complete, encode_map_change, encode_walk_ack,
    encode_walk_reject, CharacterPlay, CreateCharacter, PlayerStart, PlayerUpdate, Point, Race,
    SkillChoice, WalkRequest, DEFAULT_MAP_HEIGHT, DEFAULT_MAP_WIDTH,
};
