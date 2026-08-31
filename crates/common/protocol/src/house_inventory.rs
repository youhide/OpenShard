//! Bounded, read-only house inventory search for the OpenShard client.

use crate::access::OPENSHARD_SUBCOMMANDS;
use crate::codec::{
    PacketReader,
    PacketWriter,
};
use crate::error::DecodeError;
use crate::item_kind::{
    ItemKindId,
    MaterialId,
};
use crate::packet::{
    DecodePacket,
    EncodePacket,
    PacketLength,
    frame_body,
};
use crate::serial::Serial;
use crate::wire::{
    Graphic,
    Hue,
};

/// Maximum exact identities one search request may name.
pub const MAX_HOUSE_INVENTORY_SELECTORS: usize = 32;
/// Maximum root rows one response page may carry.
pub const MAX_HOUSE_INVENTORY_PAGE: usize = 50;

/// Search identity: semantic where known, exact legacy presentation otherwise.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum HouseItemIdentity {
    Semantic {
        kind:     ItemKindId,
        material: Option<MaterialId>,
    },
    Legacy {
        graphic: Graphic,
        hue:     Hue,
    },
}

/// One entry in the client-owned text/category search catalogue.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HouseCatalogueEntry {
    pub identity: HouseItemIdentity,
    pub name:     &'static str,
    pub tags:     &'static [&'static str],
    pub graphic:  Graphic,
    pub hue:      Hue,
}

include!(concat!(env!("OUT_DIR"), "/house_item_catalogue.rs"));

/// Stable continuation after one returned `(identity, root)` row.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct HouseInventoryCursor {
    pub identity: HouseItemIdentity,
    pub root:     Serial,
}

/// One permission-filtered root result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HouseInventoryRow {
    pub identity:        HouseItemIdentity,
    pub aggregate_total: u64,
    pub root:            Serial,
    pub root_total:      u64,
    pub first_pile:      Serial,
    pub pile_count:      u32,
}

/// A client request. Text never crosses the wire; it resolves to exact static
/// catalogue identities before this packet is built.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HouseInventoryRequest {
    Search {
        expected_epoch: Option<u64>,
        selectors:      Vec<HouseItemIdentity>,
        after:          Option<HouseInventoryCursor>,
        limit:          u8,
    },
    Resolve {
        epoch:    u64,
        identity: HouseItemIdentity,
        root:     Serial,
        item:     Serial,
    },
}

impl HouseInventoryRequest {
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 24;

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        frame_body(0xBF, PacketLength::Variable, |out| {
            out.u16(Self::SUBCOMMAND);
            match self {
                Self::Search {
                    expected_epoch,
                    selectors,
                    after,
                    limit,
                } => {
                    out.u8(0);
                    write_optional_u64(out, *expected_epoch);
                    out.u8(u8::try_from(selectors.len()).expect("house selectors fit the protocol bound"));
                    for &identity in selectors {
                        write_identity(out, identity);
                    }
                    out.u8(u8::from(after.is_some()));
                    if let Some(cursor) = after {
                        write_identity(out, cursor.identity);
                        out.u32(cursor.root.raw());
                    }
                    out.u8(*limit);
                }
                Self::Resolve {
                    epoch,
                    identity,
                    root,
                    item,
                } => {
                    out.u8(1);
                    out.u64(*epoch);
                    write_identity(out, *identity);
                    out.u32(root.raw());
                    out.u32(item.raw());
                }
            }
        })
    }

    pub(crate) fn decode_body(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        match reader.u8()? {
            0 => {
                let expected_epoch = read_optional_u64(reader)?;
                let count = usize::from(reader.u8()?);
                if count == 0 || count > MAX_HOUSE_INVENTORY_SELECTORS {
                    return Err(DecodeError::UnknownValue {
                        field: "house inventory selector count",
                        value: count as u32,
                    });
                }
                let selectors = (0..count)
                    .map(|_| read_identity(reader))
                    .collect::<Result<Vec<_>, _>>()?;
                let after = match reader.u8()? {
                    0 => None,
                    1 => {
                        Some(HouseInventoryCursor {
                            identity: read_identity(reader)?,
                            root:     read_serial(reader, "house inventory cursor root")?,
                        })
                    }
                    value => {
                        return Err(DecodeError::UnknownValue {
                            field: "house inventory cursor presence",
                            value: u32::from(value),
                        });
                    }
                };
                let limit = reader.u8()?;
                if limit == 0 || usize::from(limit) > MAX_HOUSE_INVENTORY_PAGE || reader.remaining() != 0 {
                    return Err(DecodeError::UnknownValue {
                        field: "house inventory page size or trailing bytes",
                        value: u32::from(limit),
                    });
                }
                Ok(Self::Search {
                    expected_epoch,
                    selectors,
                    after,
                    limit,
                })
            }
            1 => {
                let request = Self::Resolve {
                    epoch:    reader.u64()?,
                    identity: read_identity(reader)?,
                    root:     read_serial(reader, "house inventory result root")?,
                    item:     read_serial(reader, "house inventory result item")?,
                };
                if reader.remaining() != 0 {
                    return Err(DecodeError::UnknownValue {
                        field: "house inventory resolve trailing bytes",
                        value: u32::try_from(reader.remaining()).unwrap_or(u32::MAX),
                    });
                }
                Ok(request)
            }
            value => {
                Err(DecodeError::UnknownValue {
                    field: "house inventory request kind",
                    value: u32::from(value),
                })
            }
        }
    }
}

/// Why a search or result-open request was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HouseInventoryRefusal {
    NotInHouse,
    Banned,
    InvalidRequest,
    Unavailable,
    Stale,
    NotFound,
}

/// The bounded server answer retained by the client-owned search window.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HouseInventoryReply {
    Page {
        epoch: u64,
        rows:  Vec<HouseInventoryRow>,
        next:  Option<HouseInventoryCursor>,
    },
    Resolved {
        epoch: u64,
        root:  Serial,
        item:  Serial,
    },
    Refused {
        reason:        HouseInventoryRefusal,
        current_epoch: u64,
    },
}

impl HouseInventoryReply {
    pub const ID: u8 = 0xBF;
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 25;
}

impl EncodePacket for HouseInventoryReply {
    const ID: u8 = Self::ID;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: crate::version::ClientVersion) {
        out.u16(Self::SUBCOMMAND);
        match self {
            Self::Page { epoch, rows, next } => {
                out.u8(0);
                out.u64(*epoch);
                out.u8(u8::try_from(rows.len()).expect("house page fits its protocol bound"));
                for row in rows {
                    write_identity(out, row.identity);
                    out.u64(row.aggregate_total);
                    out.u32(row.root.raw());
                    out.u64(row.root_total);
                    out.u32(row.first_pile.raw());
                    out.u32(row.pile_count);
                }
                out.u8(u8::from(next.is_some()));
                if let Some(cursor) = next {
                    write_identity(out, cursor.identity);
                    out.u32(cursor.root.raw());
                }
            }
            Self::Resolved { epoch, root, item } => {
                out.u8(1);
                out.u64(*epoch);
                out.u32(root.raw());
                out.u32(item.raw());
            }
            Self::Refused {
                reason,
                current_epoch,
            } => {
                out.u8(2);
                out.u8(refusal_code(*reason));
                out.u64(*current_epoch);
            }
        }
    }
}

impl DecodePacket for HouseInventoryReply {
    const ID: u8 = Self::ID;

    fn decode_body(
        reader: &mut PacketReader<'_>,
        _version: crate::version::ClientVersion,
    ) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for house inventory",
                value: u32::from(subcommand),
            });
        }
        let reply = match reader.u8()? {
            0 => {
                let epoch = reader.u64()?;
                let count = usize::from(reader.u8()?);
                if count > MAX_HOUSE_INVENTORY_PAGE {
                    return Err(DecodeError::UnknownValue {
                        field: "house inventory result count",
                        value: count as u32,
                    });
                }
                let rows = (0..count)
                    .map(|_| {
                        Ok(HouseInventoryRow {
                            identity:        read_identity(reader)?,
                            aggregate_total: reader.u64()?,
                            root:            read_serial(reader, "house inventory row root")?,
                            root_total:      reader.u64()?,
                            first_pile:      read_serial(reader, "house inventory first pile")?,
                            pile_count:      reader.u32()?,
                        })
                    })
                    .collect::<Result<Vec<_>, DecodeError>>()?;
                let next = match reader.u8()? {
                    0 => None,
                    1 => {
                        Some(HouseInventoryCursor {
                            identity: read_identity(reader)?,
                            root:     read_serial(reader, "house inventory next root")?,
                        })
                    }
                    value => {
                        return Err(DecodeError::UnknownValue {
                            field: "house inventory next presence",
                            value: u32::from(value),
                        });
                    }
                };
                Self::Page { epoch, rows, next }
            }
            1 => {
                Self::Resolved {
                    epoch: reader.u64()?,
                    root:  read_serial(reader, "resolved house inventory root")?,
                    item:  read_serial(reader, "resolved house inventory item")?,
                }
            }
            2 => {
                Self::Refused {
                    reason:        read_refusal(reader.u8()?)?,
                    current_epoch: reader.u64()?,
                }
            }
            value => {
                return Err(DecodeError::UnknownValue {
                    field: "house inventory reply kind",
                    value: u32::from(value),
                });
            }
        };
        if reader.remaining() != 0 {
            return Err(DecodeError::UnknownValue {
                field: "house inventory reply trailing bytes",
                value: u32::try_from(reader.remaining()).unwrap_or(u32::MAX),
            });
        }
        Ok(reply)
    }
}

fn write_identity(out: &mut PacketWriter, identity: HouseItemIdentity) {
    match identity {
        HouseItemIdentity::Semantic { kind, material } => {
            out.u8(0);
            out.u32(kind.0);
            out.u8(u8::from(material.is_some()));
            if let Some(material) = material {
                out.u16(material.0);
            }
        }
        HouseItemIdentity::Legacy { graphic, hue } => {
            out.u8(1);
            out.u16(graphic.0);
            out.u16(hue.0);
        }
    }
}

fn read_identity(reader: &mut PacketReader<'_>) -> Result<HouseItemIdentity, DecodeError> {
    match reader.u8()? {
        0 => {
            let raw_kind = reader.u32()?;
            let kind = ItemKindId::new(raw_kind).ok_or(DecodeError::UnknownValue {
                field: "house inventory item kind",
                value: raw_kind,
            })?;
            let material = match reader.u8()? {
                0 => None,
                1 => {
                    let raw = reader.u16()?;
                    Some(MaterialId::new(raw).ok_or(DecodeError::UnknownValue {
                        field: "house inventory material",
                        value: u32::from(raw),
                    })?)
                }
                value => {
                    return Err(DecodeError::UnknownValue {
                        field: "house inventory material presence",
                        value: u32::from(value),
                    });
                }
            };
            Ok(HouseItemIdentity::Semantic { kind, material })
        }
        1 => {
            Ok(HouseItemIdentity::Legacy {
                graphic: Graphic(reader.u16()?),
                hue:     Hue(reader.u16()?),
            })
        }
        value => {
            Err(DecodeError::UnknownValue {
                field: "house inventory identity kind",
                value: u32::from(value),
            })
        }
    }
}

fn write_optional_u64(out: &mut PacketWriter, value: Option<u64>) {
    out.u8(u8::from(value.is_some()));
    if let Some(value) = value {
        out.u64(value);
    }
}

fn read_optional_u64(reader: &mut PacketReader<'_>) -> Result<Option<u64>, DecodeError> {
    match reader.u8()? {
        0 => Ok(None),
        1 => Ok(Some(reader.u64()?)),
        value => {
            Err(DecodeError::UnknownValue {
                field: "house inventory epoch presence",
                value: u32::from(value),
            })
        }
    }
}

fn read_serial(reader: &mut PacketReader<'_>, field: &'static str) -> Result<Serial, DecodeError> {
    let raw = reader.u32()?;
    Serial::new(raw).ok_or(DecodeError::UnknownValue { field, value: raw })
}

const fn refusal_code(reason: HouseInventoryRefusal) -> u8 {
    match reason {
        HouseInventoryRefusal::NotInHouse => 0,
        HouseInventoryRefusal::Banned => 1,
        HouseInventoryRefusal::InvalidRequest => 2,
        HouseInventoryRefusal::Unavailable => 3,
        HouseInventoryRefusal::Stale => 4,
        HouseInventoryRefusal::NotFound => 5,
    }
}

fn read_refusal(value: u8) -> Result<HouseInventoryRefusal, DecodeError> {
    match value {
        0 => Ok(HouseInventoryRefusal::NotInHouse),
        1 => Ok(HouseInventoryRefusal::Banned),
        2 => Ok(HouseInventoryRefusal::InvalidRequest),
        3 => Ok(HouseInventoryRefusal::Unavailable),
        4 => Ok(HouseInventoryRefusal::Stale),
        5 => Ok(HouseInventoryRefusal::NotFound),
        value => {
            Err(DecodeError::UnknownValue {
                field: "house inventory refusal",
                value: u32::from(value),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extended::ExtendedRequest;
    use crate::packet::encode_packet;
    use crate::server_packet::ServerPacket;
    use crate::version::ClientVersion;

    fn serial(raw: u32) -> Serial {
        Serial::new(raw).expect("test object serial")
    }

    #[test]
    fn bounded_search_round_trips_through_the_extended_request() {
        let request = HouseInventoryRequest::Search {
            expected_epoch: Some(17),
            selectors:      vec![HouseItemIdentity::Semantic {
                kind:     ItemKindId(43),
                material: Some(MaterialId(9)),
            }],
            after:          Some(HouseInventoryCursor {
                identity: HouseItemIdentity::Legacy {
                    graphic: Graphic(0x0EED),
                    hue:     Hue::NONE,
                },
                root:     serial(0x4000_0010),
            }),
            limit:          50,
        };
        assert_eq!(
            ExtendedRequest::decode(&request.encode()).unwrap(),
            ExtendedRequest::HouseInventory(request)
        );
    }

    #[test]
    fn a_house_inventory_page_round_trips_as_one_bounded_server_packet() {
        let reply = HouseInventoryReply::Page {
            epoch: 9,
            rows:  vec![HouseInventoryRow {
                identity:        HouseItemIdentity::Semantic {
                    kind:     ItemKindId(1),
                    material: Some(MaterialId(1)),
                },
                aggregate_total: 400,
                root:            serial(0x4000_0010),
                root_total:      250,
                first_pile:      serial(0x4000_0011),
                pile_count:      3,
            }],
            next:  None,
        };
        let bytes = encode_packet(&reply, ClientVersion::TOL);
        assert!(matches!(
            ServerPacket::decode(&bytes, ClientVersion::TOL),
            Ok(Some(ServerPacket::HouseInventory(found))) if found == reply
        ));
    }

    #[test]
    fn static_house_catalogue_covers_names_categories_and_materials() {
        assert!(
            HOUSE_ITEM_CATALOGUE
                .iter()
                .any(|entry| { entry.name == "valorite ringmail gloves" && entry.tags.contains(&"armor") })
        );
        assert!(
            HOUSE_ITEM_CATALOGUE
                .iter()
                .any(|entry| { entry.name == "smith's tongs" && entry.tags.contains(&"tool") })
        );
    }
}
