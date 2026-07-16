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

mod codec;
mod feature;
mod packet;
mod version;

pub use codec::{CodecError, CodecResult, PacketReader, PacketWriter};
pub use feature::{Feature, FeatureSet};
pub use packet::{
    client_packet_length, frame_client_packet, Frame, FrameError, PacketLength, MAX_PACKET_SIZE,
    SEED_LENGTH_NEW, SEED_LENGTH_OLD,
};
pub use version::{ClientVersion, Era, ParseVersionError};
