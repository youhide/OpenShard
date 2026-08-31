//! The live map editor's commit conversation.
//!
//! Both packets are OpenShard extensions under `0xBF`.  A request contains
//! only the proposed facet, parent revision and editor operations.  In
//! particular it contains no author or authority: those are facts the shard
//! already established for the connection at login, and accepting either from
//! this body would turn attribution and permission into client claims.

use crate::access::OPENSHARD_SUBCOMMANDS;
use crate::chunks::WorldRevision;
use crate::codec::{
    PacketReader,
    PacketWriter,
};
use crate::error::DecodeError;
use crate::packet::{
    DecodePacket,
    EncodePacket,
    PacketLength,
};
use crate::version::ClientVersion;
use crate::wire::{
    Graphic,
    Hue,
};
use crate::world::Facet;

/// The most operations one commit request may carry.
///
/// This is deliberately below what the envelope could theoretically fit.  It
/// bounds tick work and allocation independently of packet framing; a larger
/// brush is split into several revision-checked commits by the editor.
pub const MAX_EDIT_OPS: u16 = 1_024;

/// The largest request body, measured after the `0xBF` envelope's subcommand.
///
/// The general framer already caps every packet at 18,000 bytes.  This tighter
/// cap belongs to the expensive operation family itself and remains in force if
/// the general ceiling changes.
pub const MAX_EDIT_BODY_BYTES: usize = 16 * 1_024;

/// A tile coordinate in a map-edit request.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct EditTile {
    /// East/west tile coordinate.
    pub x: EditX,
    /// North/south tile coordinate.
    pub y: EditY,
}

/// An unvalidated x coordinate.  The addressed facet supplies its bound.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct EditX(pub u16);

/// An unvalidated y coordinate.  The addressed facet supplies its bound.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct EditY(pub u16);

/// A land-tile id.  Its constructor keeps values outside tiledata's land table
/// out of a well-typed request.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EditLandTile(u16);

impl EditLandTile {
    /// The number of land entries in every UO tiledata table.
    pub const COUNT: u16 = 0x4000;

    /// Interpret a wire value as a land-tile id.
    #[must_use]
    pub const fn from_wire(value: u16) -> Option<Self> {
        if value < Self::COUNT {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The integer written on the wire and used to index tiledata.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// A signed terrain/static height as one wire byte.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EditZ(pub i8);

/// Which static on a tile to remove after the preceding operations in this
/// request have been applied.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EditStaticId(pub u16);

/// One canonical operation proposed by an editor.
///
/// The reversible `was` values used by `openshard_map::patch::PatchOp` do not
/// cross the trust boundary.  The shard reads them from the parent snapshot
/// while compiling this request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapEditOp {
    /// Replace one land cell.
    SetLand {
        /// Where.
        at:   EditTile,
        /// New land tile.
        tile: EditLandTile,
        /// New height.
        z:    EditZ,
    },
    /// Add one static after the statics already standing on its tile.
    AddStatic {
        /// Where.
        at:      EditTile,
        /// Static art id.
        graphic: Graphic,
        /// Base height.
        z:       EditZ,
        /// Tint, or zero.
        hue:     Hue,
    },
    /// Remove one static by its ordinal at this point in the request.
    RemoveStatic {
        /// Where.
        at:    EditTile,
        /// Which static on that tile.
        which: EditStaticId,
    },
}

impl MapEditOp {
    const SET_LAND: u8 = 0;
    const ADD_STATIC: u8 = 1;
    const REMOVE_STATIC: u8 = 2;

    fn decode(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        let kind = reader.u8()?;
        let at = EditTile {
            x: EditX(reader.u16()?),
            y: EditY(reader.u16()?),
        };
        Ok(match kind {
            Self::SET_LAND => {
                let raw = reader.u16()?;
                let Some(tile) = EditLandTile::from_wire(raw) else {
                    return Err(DecodeError::UnknownValue {
                        field: "map-edit land tile",
                        value: u32::from(raw),
                    });
                };
                Self::SetLand {
                    at,
                    tile,
                    z: EditZ(reader.u8()? as i8),
                }
            }
            Self::ADD_STATIC => {
                Self::AddStatic {
                    at,
                    graphic: Graphic(reader.u16()?),
                    z: EditZ(reader.u8()? as i8),
                    hue: Hue(reader.u16()?),
                }
            }
            Self::REMOVE_STATIC => {
                Self::RemoveStatic {
                    at,
                    which: EditStaticId(reader.u16()?),
                }
            }
            other => {
                return Err(DecodeError::UnknownValue {
                    field: "map-edit operation",
                    value: u32::from(other),
                });
            }
        })
    }

    fn encode(self, out: &mut PacketWriter) {
        let (kind, at) = match self {
            Self::SetLand { at, .. } => (Self::SET_LAND, at),
            Self::AddStatic { at, .. } => (Self::ADD_STATIC, at),
            Self::RemoveStatic { at, .. } => (Self::REMOVE_STATIC, at),
        };
        out.u8(kind);
        out.u16(at.x.0);
        out.u16(at.y.0);
        match self {
            Self::SetLand { tile, z, .. } => {
                out.u16(tile.get());
                out.u8(z.0 as u8);
            }
            Self::AddStatic { graphic, z, hue, .. } => {
                out.u16(graphic.0);
                out.u8(z.0 as u8);
                out.u16(hue.0);
            }
            Self::RemoveStatic { which, .. } => out.u16(which.0),
        }
    }
}

/// `0xBF` subcommand `0xE009` — commit an editor draft against one published
/// revision.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MapEditRequest {
    /// Which facet to edit, still an input until the shard looks it up.
    pub facet:  Facet,
    /// The exact published revision the editor drew and built ordinals against.
    pub parent: WorldRevision,
    /// Canonical operations, in application order.
    pub ops:    Vec<MapEditOp>,
}

impl MapEditRequest {
    /// Which `0xBF` this is.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 9;

    /// Encode a request for `Command::Send` on the client side.
    ///
    /// # Panics
    ///
    /// If `ops` exceeds [`MAX_EDIT_OPS`] or the editor-specific byte cap.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        assert!(
            self.ops.len() <= usize::from(MAX_EDIT_OPS),
            "too many map-edit operations"
        );
        let encoded = crate::packet::frame_body(0xBF, PacketLength::Variable, |out| {
            out.u16(Self::SUBCOMMAND);
            out.u8(self.facet.0);
            out.u64(self.parent.0);
            out.u16(u16::try_from(self.ops.len()).expect("the operation cap fits u16"));
            for op in &self.ops {
                op.encode(out);
            }
        });
        assert!(
            encoded.len() - 5 <= MAX_EDIT_BODY_BYTES,
            "map-edit body exceeds its byte cap"
        );
        encoded
    }

    pub(crate) fn decode_body(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        if reader.remaining() > MAX_EDIT_BODY_BYTES {
            return Err(DecodeError::UnknownValue {
                field: "map-edit body byte count",
                value: u32::try_from(reader.remaining()).unwrap_or(u32::MAX),
            });
        }
        let facet = Facet(reader.u8()?);
        let parent = WorldRevision(reader.u64()?);
        let count = reader.u16()?;
        if count > MAX_EDIT_OPS {
            return Err(DecodeError::UnknownValue {
                field: "map-edit operation count",
                value: u32::from(count),
            });
        }
        let mut ops = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            ops.push(MapEditOp::decode(reader)?);
        }
        if !reader.is_empty() {
            return Err(DecodeError::UnknownValue {
                field: "trailing map-edit byte count",
                value: u32::try_from(reader.remaining()).unwrap_or(u32::MAX),
            });
        }
        Ok(Self { facet, parent, ops })
    }
}

/// Why a syntactically valid editor commit was refused by the shard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapEditRefusal {
    /// The authenticated account is below GameMaster.
    NotAuthorized,
    /// This shard did not load the requested facet.
    UnknownFacet,
    /// The loaded facet has no ground.
    NoGround,
    /// The editor submitted no operations.
    EmptyDraft,
    /// The editor's parent is no longer current.
    Conflict,
    /// At least one operation names a tile outside the facet.
    OffMap,
    /// A removal ordinal does not exist at that parent and point in the batch.
    NoSuchStatic,
    /// The facet is read from client files and has no patch log of its own.
    NotOurWorld,
    /// The patch applied, but durable logging failed and the world was restored.
    Storage,
}

impl MapEditRefusal {
    const fn wire(self) -> u8 {
        match self {
            Self::NotAuthorized => 0,
            Self::UnknownFacet => 1,
            Self::NoGround => 2,
            Self::EmptyDraft => 3,
            Self::Conflict => 4,
            Self::OffMap => 5,
            Self::NoSuchStatic => 6,
            Self::NotOurWorld => 7,
            Self::Storage => 8,
        }
    }

    const fn from_wire(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::NotAuthorized,
            1 => Self::UnknownFacet,
            2 => Self::NoGround,
            3 => Self::EmptyDraft,
            4 => Self::Conflict,
            5 => Self::OffMap,
            6 => Self::NoSuchStatic,
            7 => Self::NotOurWorld,
            8 => Self::Storage,
            _ => return None,
        })
    }
}

/// The result of a commit request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapEditOutcome {
    /// The log has the patch and the world is now at this revision.
    Accepted,
    /// Nothing changed; `revision` is the shard's current revision when one is
    /// available.
    Refused(MapEditRefusal),
}

/// `0xBF` subcommand `0xE00A` — the one answer to a [`MapEditRequest`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MapEditReply {
    /// Which requested facet this answers.
    pub facet:    Facet,
    /// The new revision on acceptance, or the current revision on refusal.
    /// Zero means the facet/revision did not exist or was not disclosed.
    pub revision: WorldRevision,
    /// Accepted or a typed refusal.
    pub outcome:  MapEditOutcome,
}

impl MapEditReply {
    /// Which `0xBF` this is.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 10;
    /// id, length, subcommand, facet, outcome, revision, refusal/padding.
    pub const LENGTH_BYTES: u16 = 16;

    /// Encode the whole packet.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        crate::packet::encode_packet(&self, ClientVersion::new(4, 0, 0, 0))
    }
}

impl EncodePacket for MapEditReply {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Fixed(Self::LENGTH_BYTES);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(Self::LENGTH_BYTES);
        out.u16(Self::SUBCOMMAND);
        out.u8(self.facet.0);
        match self.outcome {
            MapEditOutcome::Accepted => {
                out.u8(0);
                out.u64(self.revision.0);
                out.u8(0);
            }
            MapEditOutcome::Refused(reason) => {
                out.u8(1);
                out.u64(self.revision.0);
                out.u8(reason.wire());
            }
        }
    }
}

impl DecodePacket for MapEditReply {
    const ID: u8 = 0xBF;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for a map-edit reply",
                value: u32::from(subcommand),
            });
        }
        let facet = Facet(reader.u8()?);
        let status = reader.u8()?;
        let revision = WorldRevision(reader.u64()?);
        let reason = reader.u8()?;
        let outcome = match status {
            0 if reason == 0 => MapEditOutcome::Accepted,
            1 => {
                let Some(reason) = MapEditRefusal::from_wire(reason) else {
                    return Err(DecodeError::UnknownValue {
                        field: "map-edit refusal",
                        value: u32::from(reason),
                    });
                };
                MapEditOutcome::Refused(reason)
            }
            other => {
                return Err(DecodeError::UnknownValue {
                    field: "map-edit outcome",
                    value: u32::from(other),
                });
            }
        };
        Ok(Self {
            facet,
            revision,
            outcome,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extended::ExtendedRequest;
    use crate::server_packet::ServerPacket;

    fn tile(value: u16) -> EditLandTile {
        EditLandTile::from_wire(value).expect("a fixture tile")
    }

    #[test]
    fn a_request_round_trips_through_the_extended_dispatch() {
        let sent = MapEditRequest {
            facet:  Facet(2),
            parent: WorldRevision(41),
            ops:    vec![
                MapEditOp::SetLand {
                    at:   EditTile {
                        x: EditX(12),
                        y: EditY(34),
                    },
                    tile: tile(0x123),
                    z:    EditZ(-7),
                },
                MapEditOp::AddStatic {
                    at:      EditTile {
                        x: EditX(12),
                        y: EditY(34),
                    },
                    graphic: Graphic(0x0edd),
                    z:       EditZ(5),
                    hue:     Hue(9),
                },
                MapEditOp::RemoveStatic {
                    at:    EditTile {
                        x: EditX(8),
                        y: EditY(9),
                    },
                    which: EditStaticId(3),
                },
            ],
        };
        assert_eq!(
            ExtendedRequest::decode(&sent.encode()).expect("our request decodes"),
            ExtendedRequest::MapEdit(sent)
        );
    }

    #[test]
    fn hostile_counts_unknown_ops_and_bad_land_are_refused_without_allocation_or_panic() {
        let envelope = |body: &[u8]| {
            crate::packet::frame_body(0xBF, PacketLength::Variable, |out| {
                out.u16(MapEditRequest::SUBCOMMAND);
                out.bytes(body);
            })
        };
        let mut over = vec![0, 0, 0, 0, 0, 0, 0, 0, 1]; // facet + parent
        over.extend_from_slice(&(MAX_EDIT_OPS + 1).to_be_bytes());
        assert!(ExtendedRequest::decode(&envelope(&over)).is_err());

        let mut unknown = vec![0, 0, 0, 0, 0, 0, 0, 0, 1];
        unknown.extend_from_slice(&1u16.to_be_bytes());
        unknown.extend_from_slice(&[99, 0, 0, 0, 0]);
        assert!(ExtendedRequest::decode(&envelope(&unknown)).is_err());

        let mut bad_land = vec![0, 0, 0, 0, 0, 0, 0, 0, 1];
        bad_land.extend_from_slice(&1u16.to_be_bytes());
        bad_land.extend_from_slice(&[MapEditOp::SET_LAND, 0, 0, 0, 0, 0x40, 0, 0]);
        assert!(ExtendedRequest::decode(&envelope(&bad_land)).is_err());
    }

    #[test]
    fn trailing_bytes_are_not_silently_ignored() {
        let mut bytes = MapEditRequest {
            facet:  Facet(0),
            parent: WorldRevision(1),
            ops:    Vec::new(),
        }
        .encode();
        bytes.push(0xaa);
        let length = u16::try_from(bytes.len()).unwrap().to_be_bytes();
        bytes[1..3].copy_from_slice(&length);
        assert!(ExtendedRequest::decode(&bytes).is_err());
    }

    #[test]
    fn accepted_and_refused_replies_round_trip_through_server_dispatch() {
        for sent in [
            MapEditReply {
                facet:    Facet(0),
                revision: WorldRevision(8),
                outcome:  MapEditOutcome::Accepted,
            },
            MapEditReply {
                facet:    Facet(1),
                revision: WorldRevision(7),
                outcome:  MapEditOutcome::Refused(MapEditRefusal::Conflict),
            },
        ] {
            let packet = ServerPacket::MapEditReply(sent);
            assert_eq!(
                ServerPacket::decode(&packet.encode(ClientVersion::TOL), ClientVersion::TOL)
                    .expect("our packet decodes"),
                Some(packet)
            );
        }
    }
}
