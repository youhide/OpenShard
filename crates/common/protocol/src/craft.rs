//! The tool-free craft catalogue request and its OpenShard-only data stream.
//!
//! A craft tool is still what *makes* an item, but it is a poor affordance for
//! learning the game: a player without tongs cannot even see what tongs would
//! let them make. This private `0xBF` request opens the read-only catalogue.

use crate::access::OPENSHARD_SUBCOMMANDS;
use crate::codec::{PacketReader, PacketWriter};
use crate::error::DecodeError;
use crate::gump::GumpId;
use crate::packet::{DecodePacket, EncodePacket, PacketLength, frame_body};
use crate::wire::{ClilocId, Graphic, Hue};

/// `0xBF.0xE015` — open the craft catalogue without selecting a tool.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpenCraftCatalogue;

impl OpenCraftCatalogue {
    /// The first private subcommand after the turn request.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 21;

    /// Read the empty body. Extra bytes are refused so a future extension must
    /// name its versioned shape instead of silently changing this request.
    pub(crate) fn decode_body(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        if reader.remaining() != 0 {
            return Err(DecodeError::UnknownValue {
                field: "craft catalogue body byte count",
                value: u32::try_from(reader.remaining()).unwrap_or(u32::MAX),
            });
        }
        Ok(Self)
    }

    /// Encode the complete extended request.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        frame_body(0xBF, PacketLength::Variable, |out| out.u16(Self::SUBCOMMAND))
    }
}

/// One material cell in a catalogue row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CraftCatalogueComponent {
    pub graphic: Graphic,
    pub hue: Hue,
    /// Localized material name for a human-readable tooltip.
    pub name: ClilocId,
    pub amount: u16,
}

/// Combat family used by an item which can be wielded. The catalogue keeps
/// this compact presentation data beside its recipe rather than requiring the
/// client to depend on server combat definitions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CraftWeaponKind {
    Slashing,
    Piercing,
    Bashing,
    Axe,
    Polearm,
    Staff,
    Ranged,
}

impl CraftWeaponKind {
    const fn encode(self) -> u8 {
        match self {
            Self::Slashing => 0,
            Self::Piercing => 1,
            Self::Bashing => 2,
            Self::Axe => 3,
            Self::Polearm => 4,
            Self::Staff => 5,
            Self::Ranged => 6,
        }
    }

    fn decode(value: u8) -> Result<Self, DecodeError> {
        match value {
            0 => Ok(Self::Slashing),
            1 => Ok(Self::Piercing),
            2 => Ok(Self::Bashing),
            3 => Ok(Self::Axe),
            4 => Ok(Self::Polearm),
            5 => Ok(Self::Staff),
            6 => Ok(Self::Ranged),
            value => Err(DecodeError::UnknownValue {
                field: "craft weapon kind",
                value: u32::from(value),
            }),
        }
    }
}

/// The concise combat facts a player needs while comparing crafted weapons.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CraftWeaponProperties {
    pub combat_skill: ClilocId,
    pub kind: CraftWeaponKind,
    pub damage_min: u16,
    pub damage_max: u16,
    /// Milliseconds would be needless precision; the authoritative ML number
    /// is centiseconds and remains that unit on the wire.
    pub speed_centis: u16,
    /// A ranged weapon's distance in tiles. `None` means melee.
    pub range: Option<u8>,
}

/// One locally-scrollable catalogue row.  It contains data, not coordinates:
/// the OpenShard client owns the table geometry, fitting and scroll position.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CraftCatalogueRow {
    /// The normal craft-gump reply id for opening this recipe's details.
    pub button: u32,
    pub result: Graphic,
    pub result_hue: Hue,
    pub name: ClilocId,
    pub skill: ClilocId,
    /// The lowest effective skill value allowed to attempt the primary skill
    /// check, in tenths of a percent.
    pub skill_min: u16,
    pub ready: bool,
    pub weapon: Option<CraftWeaponProperties>,
    pub components: Vec<CraftCatalogueComponent>,
}

/// `0xBF.0xE016` — the complete compact data model for a craft catalogue.
///
/// This deliberately travels outside `0xB0`: a gump layout is capped at a
/// `u16` byte count, while a full catalogue expressed as ordinary gump rows
/// would overflow it before the client could scroll locally.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CraftCatalogue {
    /// The gump shell this data belongs to.
    pub gump_id: GumpId,
    pub rows: Vec<CraftCatalogueRow>,
}

impl CraftCatalogue {
    pub const ID: u8 = 0xBF;
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 22;
}

impl EncodePacket for CraftCatalogue {
    const ID: u8 = Self::ID;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: crate::version::ClientVersion) {
        out.u16(Self::SUBCOMMAND);
        out.u32(self.gump_id.0);
        out.u16(u16::try_from(self.rows.len()).expect("a craft catalogue fits a u16 row count"));
        for row in &self.rows {
            out.u32(row.button);
            out.u16(row.result.0);
            out.u16(row.result_hue.0);
            out.u32(row.name.0);
            out.u32(row.skill.0);
            out.u16(row.skill_min);
            out.u8(u8::from(row.ready));
            match row.weapon {
                Some(weapon) => {
                    out.u8(1);
                    out.u32(weapon.combat_skill.0);
                    out.u8(weapon.kind.encode());
                    out.u16(weapon.damage_min);
                    out.u16(weapon.damage_max);
                    out.u16(weapon.speed_centis);
                    out.u8(weapon.range.unwrap_or(0));
                }
                None => out.u8(0),
            }
            out.u8(u8::try_from(row.components.len()).expect("a craft row fits a u8 component count"));
            for component in &row.components {
                out.u16(component.graphic.0);
                out.u16(component.hue.0);
                out.u32(component.name.0);
                out.u16(component.amount);
            }
        }
    }
}

impl DecodePacket for CraftCatalogue {
    const ID: u8 = Self::ID;

    fn decode_body(
        reader: &mut PacketReader<'_>,
        _version: crate::version::ClientVersion,
    ) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for a craft catalogue",
                value: u32::from(subcommand),
            });
        }
        let gump_id = GumpId(reader.u32()?);
        let count = usize::from(reader.u16()?);
        let mut rows = Vec::with_capacity(count);
        for _ in 0..count {
            let button = reader.u32()?;
            let result = Graphic(reader.u16()?);
            let result_hue = Hue(reader.u16()?);
            let name = ClilocId(reader.u32()?);
            let skill = ClilocId(reader.u32()?);
            let skill_min = reader.u16()?;
            let ready = reader.u8()? != 0;
            let weapon = match reader.u8()? {
                0 => None,
                1 => Some(CraftWeaponProperties {
                    combat_skill: ClilocId(reader.u32()?),
                    kind: CraftWeaponKind::decode(reader.u8()?)?,
                    damage_min: reader.u16()?,
                    damage_max: reader.u16()?,
                    speed_centis: reader.u16()?,
                    range: match reader.u8()? {
                        0 => None,
                        range => Some(range),
                    },
                }),
                value => {
                    return Err(DecodeError::UnknownValue {
                        field: "craft weapon presence",
                        value: u32::from(value),
                    });
                }
            };
            let components = (0..reader.u8()?)
                .map(|_| {
                    Ok(CraftCatalogueComponent {
                        graphic: Graphic(reader.u16()?),
                        hue: Hue(reader.u16()?),
                        name: ClilocId(reader.u32()?),
                        amount: reader.u16()?,
                    })
                })
                .collect::<Result<Vec<_>, DecodeError>>()?;
            rows.push(CraftCatalogueRow {
                button,
                result,
                result_hue,
                name,
                skill,
                skill_min,
                ready,
                weapon,
                components,
            });
        }
        Ok(Self { gump_id, rows })
    }
}

#[cfg(test)]
mod tests {
    use crate::gump::GumpId;
    use crate::packet::encode_packet;
    use crate::server_packet::ServerPacket;
    use crate::version::ClientVersion;
    use crate::wire::{ClilocId, Graphic, Hue};

    use crate::extended::ExtendedRequest;

    use super::*;

    #[test]
    fn the_catalogue_request_round_trips_through_the_extended_envelope() {
        assert_eq!(
            ExtendedRequest::decode(&OpenCraftCatalogue.encode()).unwrap(),
            ExtendedRequest::CraftCatalogue(OpenCraftCatalogue)
        );
    }

    #[test]
    fn catalogue_rows_round_trip_in_their_own_extended_packet() {
        let sent = CraftCatalogue {
            gump_id: GumpId(0x00AD_0001),
            rows: vec![CraftCatalogueRow {
                button: 8,
                result: Graphic(0x13EB),
                result_hue: Hue::NONE,
                name: ClilocId(1_022_036),
                skill: ClilocId(1_044_067),
                skill_min: 300,
                ready: true,
                weapon: Some(CraftWeaponProperties {
                    combat_skill: ClilocId(1_044_100),
                    kind: CraftWeaponKind::Slashing,
                    damage_min: 11,
                    damage_max: 14,
                    speed_centis: 350,
                    range: None,
                }),
                components: vec![CraftCatalogueComponent {
                    graphic: Graphic(0x1BF2),
                    hue: Hue::NONE,
                    name: ClilocId(1_045_000),
                    amount: 3,
                }],
            }],
        };
        let bytes = encode_packet(&sent, ClientVersion::TOL);
        assert!(matches!(
            ServerPacket::decode(&bytes, ClientVersion::TOL),
            Ok(Some(ServerPacket::CraftCatalogue(found))) if found == sent
        ));
    }
}
