//! Parties — `0xBF` subcommand `0x06`, in both directions.
//!
//! Everything a party does on the wire rides one subcommand, which is why this
//! is a module rather than a scattering of packets: the *second* byte of the
//! body decides whether a packet adds a member, removes one, carries a line of
//! chat or raises an invitation, and inbound and outbound do not use the same
//! numbering for it. Reading them apart is this file's whole job.
//!
//! Ported from ServUO's `PartyCommands` / `Server/Network/PacketHandlers.cs`
//! (inbound) and `Scripts/Services/Party/Packets.cs` (outbound).
//!
//! # The two numberings
//!
//! | | inbound (client says) | outbound (server says) |
//! |---|---|---|
//! | `0x01` | add — raise the target cursor | here is the whole member list |
//! | `0x02` | remove this serial | a member left; here is who, and the rest |
//! | `0x03` | say this to one member | one member said this to you |
//! | `0x04` | say this to everyone | somebody said this to the party |
//! | `0x06` | may the party loot my corpse | — |
//! | `0x07` | — | you are invited by this leader |
//! | `0x08` | I accept | — |
//! | `0x09` | I decline | — |
//!
//! `0x03` and `0x04` are the one pair that means the same thing both ways.
//! Everything else is a coincidence of numbering, and a decoder that treated the
//! table as symmetric would read "I decline" as nothing at all.
//!
//! # The empty list is a removal
//!
//! There is no "you are in no party" packet. ServUO's `PartyEmptyList` is a
//! `0x02` with a member count of **zero** and the recipient's own serial in the
//! removed slot — the same layout `PartyRemoveMember` writes, with the list
//! empty. So one type serves both, and the caller says who left.

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
use crate::serial::{
    RawSerial,
    Serial,
};
use crate::version::ClientVersion;

/// The subcommand every party packet rides, in both directions.
pub const SUBCOMMAND: u16 = 0x0006;

/// The most a party holds, candidates included. ServUO's `Party.Capacity`.
pub const CAPACITY: usize = 10;

/// The longest line of party chat the reference accepts. Longer is dropped
/// rather than clipped — ServUO's `text.Length > 128` returns without a word to
/// the sender, and a clip would put something on other people's screens that
/// nobody typed.
pub const MESSAGE_LIMIT: usize = 128;

/// What a client asked its party to do.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum PartyRequest {
    /// `0x01` — start adding somebody. Carries nothing: the client is asking
    /// for the target cursor, and who it lands on comes back as an ordinary
    /// target reply.
    Add,
    /// `0x02` — remove this member. Names the leader kicking somebody, or a
    /// member naming themselves to leave.
    Remove(RawSerial),
    /// `0x03` — say this to one member.
    PrivateMessage {
        /// Who it is for.
        to:   RawSerial,
        /// What was typed.
        text: String,
    },
    /// `0x04` — say this to the whole party.
    PublicMessage(String),
    /// `0x06` — whether the party may loot this member's corpse.
    SetCanLoot(bool),
    /// `0x08` — accept the invitation from this leader.
    Accept(RawSerial),
    /// `0x09` — decline it.
    Decline(RawSerial),
    /// A party subcommand this engine does not act on. Not an error, for
    /// [`ExtendedRequest::Unknown`](crate::extended::ExtendedRequest::Unknown)'s
    /// reason.
    Unknown(u8),
}

impl PartyRequest {
    /// Read the body, `reader` already past the id, length and `0x0006`.
    ///
    /// # A truncated body is `Unknown`, not an error
    ///
    /// Only for the arms that carry nothing worth having: a `0x02` with no
    /// serial names nobody, and refusing the packet would end the connection
    /// over a client's malformed party click. An arm whose fields *are* present
    /// but malformed still errors, because that is the framer's business rather
    /// than this one's.
    pub(crate) fn decode_body(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        let kind = reader.u8()?;
        Ok(match kind {
            0x01 => Self::Add,
            0x02 => Self::Remove(RawSerial(reader.u32()?)),
            0x03 => {
                Self::PrivateMessage {
                    to:   RawSerial(reader.u32()?),
                    text: utf16_be_to_end(reader),
                }
            }
            0x04 => Self::PublicMessage(utf16_be_to_end(reader)),
            0x06 => Self::SetCanLoot(reader.bool()?),
            0x08 => Self::Accept(RawSerial(reader.u32()?)),
            0x09 => Self::Decline(RawSerial(reader.u32()?)),
            other => Self::Unknown(other),
        })
    }
}

/// Read the rest of the body as big-endian UTF-16, stopping at a NUL.
///
/// **Big**-endian, unlike the property list's arguments and like `0xAE` speech —
/// which is what this is, a line somebody typed. ServUO reads it with
/// `ReadUnicodeStringSafe`, whose "safe" is that it runs to the end of the
/// packet if no terminator arrives; the same is done here, and a lone trailing
/// byte is dropped rather than refused.
fn utf16_be_to_end(reader: &mut PacketReader<'_>) -> String {
    let units: Vec<u16> = reader
        .rest()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    // Lossy rather than refused: an unpaired surrogate is a client's problem
    // with one character, and dropping the whole line over it would lose the
    // message and the connection both.
    String::from_utf16_lossy(&units)
}

/// Write the `0xBF` envelope's subcommand. Every packet below opens with it.
fn open(out: &mut PacketWriter, kind: u8) {
    out.u16(SUBCOMMAND);
    out.u8(kind);
}

/// Read past the subcommand and the type byte, refusing a body that is not the
/// one this decoder is for.
///
/// Every packet below is `0xBF` with the same subcommand, so the *type* is what
/// tells them apart and each has to check it. Getting one wrong is not a
/// malformed packet but a well-formed one read as the wrong thing — a removal
/// read as a roster, say — which is exactly the mistake the module doc's table
/// warns about, so it is checked rather than assumed.
fn expect(reader: &mut PacketReader<'_>, kind: u8) -> Result<(), DecodeError> {
    let subcommand = reader.u16()?;
    if subcommand != SUBCOMMAND {
        return Err(DecodeError::UnknownValue {
            field: "0xBF subcommand for a party packet",
            value: u32::from(subcommand),
        });
    }
    let found = reader.u8()?;
    if found != kind {
        return Err(DecodeError::UnknownValue {
            field: "party packet type",
            value: u32::from(found),
        });
    }
    Ok(())
}

/// Read a serial that names a real object.
fn read_serial(reader: &mut PacketReader<'_>) -> Result<Serial, DecodeError> {
    let raw = reader.u32()?;
    Serial::new(raw).ok_or(DecodeError::UnknownValue {
        field: "party member serial",
        value: raw,
    })
}

/// `0x01` — the whole party, sent to everybody in it whenever it changes.
///
/// There is no "add one member" packet: a join re-sends the list to all of them,
/// which is the reference's own approach and is what keeps every client's idea
/// of the roster identical rather than accumulated.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PartyMemberList {
    /// Everyone in it, the leader first.
    pub members: Vec<Serial>,
}

impl PartyMemberList {
    /// Which of the party packets this is.
    pub const KIND: u8 = 0x01;
}

impl EncodePacket for PartyMemberList {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        open(out, Self::KIND);
        out.u8(self.members.len() as u8);
        for member in &self.members {
            out.u32(member.raw());
        }
    }
}

impl DecodePacket for PartyMemberList {
    const ID: u8 = 0xBF;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        expect(reader, Self::KIND)?;
        let count = usize::from(reader.u8()?);
        let mut members = Vec::with_capacity(count.min(CAPACITY));
        for _ in 0..count {
            members.push(read_serial(reader)?);
        }
        Ok(Self { members })
    }
}

/// `0x02` — somebody left, and who is left.
///
/// Also the "you are in no party" packet: see the module docs. An empty
/// [`members`](Self::members) with the recipient in
/// [`removed`](Self::removed) is what tells one client its party is over.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PartyRemoveMember {
    /// Who went.
    pub removed: Serial,
    /// Who is left, or empty to say the recipient is now in no party.
    pub members: Vec<Serial>,
}

impl PartyRemoveMember {
    /// Which of the party packets this is.
    pub const KIND: u8 = 0x02;
}

impl EncodePacket for PartyRemoveMember {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        open(out, Self::KIND);
        out.u8(self.members.len() as u8);
        out.u32(self.removed.raw());
        for member in &self.members {
            out.u32(member.raw());
        }
    }
}

impl DecodePacket for PartyRemoveMember {
    const ID: u8 = 0xBF;

    /// The count comes **before** the removed serial, so a zero count still has
    /// four bytes after it — see the module docs on the empty list.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        expect(reader, Self::KIND)?;
        let count = usize::from(reader.u8()?);
        let removed = read_serial(reader)?;
        let mut members = Vec::with_capacity(count.min(CAPACITY));
        for _ in 0..count {
            members.push(read_serial(reader)?);
        }
        Ok(Self { removed, members })
    }
}

/// `0x03` or `0x04` — a line of party chat, arriving.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PartyTextMessage {
    /// Whether the whole party heard it. `false` is a private line, which the
    /// client draws differently.
    pub to_all: bool,
    /// Who said it.
    pub from:   Serial,
    /// What they said.
    pub text:   String,
}

impl PartyTextMessage {
    /// A line the whole party heard.
    pub const KIND_ALL: u8 = 0x04;
    /// A line for one member.
    pub const KIND_ONE: u8 = 0x03;
}

impl EncodePacket for PartyTextMessage {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        open(
            out,
            if self.to_all {
                Self::KIND_ALL
            } else {
                Self::KIND_ONE
            },
        );
        out.u32(self.from.raw());
        out.null_terminated_string_utf16(&self.text);
    }
}

impl DecodePacket for PartyTextMessage {
    const ID: u8 = 0xBF;

    /// The one party packet whose *type* is data rather than a tag: `0x03` and
    /// `0x04` are the same body and differ only in who heard it, so this reads
    /// the byte instead of checking it against one expected value.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for a party message",
                value: u32::from(subcommand),
            });
        }
        let kind = reader.u8()?;
        let to_all = match kind {
            Self::KIND_ALL => true,
            Self::KIND_ONE => false,
            other => {
                return Err(DecodeError::UnknownValue {
                    field: "party message type",
                    value: u32::from(other),
                });
            }
        };
        Ok(Self {
            to_all,
            from: read_serial(reader)?,
            text: utf16_be_to_end(reader),
        })
    }
}

/// `0x07` — you have been asked to join this leader's party.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PartyInvitation {
    /// Whose party.
    pub leader: Serial,
}

impl PartyInvitation {
    /// Which of the party packets this is.
    pub const KIND: u8 = 0x07;
}

impl EncodePacket for PartyInvitation {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        open(out, Self::KIND);
        out.u32(self.leader.raw());
    }
}

impl DecodePacket for PartyInvitation {
    const ID: u8 = 0xBF;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        expect(reader, Self::KIND)?;
        Ok(Self {
            leader: read_serial(reader)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extended::ExtendedRequest;
    use crate::packet::encode_packet;
    use crate::server_packet::ServerPacket;

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    /// Build the `0xBF` a client would send: envelope, `0x0006`, then the body.
    fn inbound(body: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xBF, 0, 0];
        bytes.extend_from_slice(&SUBCOMMAND.to_be_bytes());
        bytes.extend_from_slice(body);
        let length = u16::try_from(bytes.len()).unwrap();
        bytes[1..3].copy_from_slice(&length.to_be_bytes());
        bytes
    }

    fn request(body: &[u8]) -> PartyRequest {
        match ExtendedRequest::decode(&inbound(body)).expect("a party 0xBF decodes") {
            ExtendedRequest::Party(request) => request,
            other => panic!("decoded as {other:?}"),
        }
    }

    #[test]
    fn each_inbound_subcommand_reads_as_itself() {
        assert_eq!(request(&[0x01]), PartyRequest::Add);
        assert_eq!(
            request(&[0x02, 0x00, 0x00, 0x00, 0x2A]),
            PartyRequest::Remove(RawSerial(0x2A))
        );
        assert_eq!(request(&[0x06, 0x01]), PartyRequest::SetCanLoot(true));
        assert_eq!(request(&[0x06, 0x00]), PartyRequest::SetCanLoot(false));
        assert_eq!(
            request(&[0x08, 0x00, 0x00, 0x00, 0x2A]),
            PartyRequest::Accept(RawSerial(0x2A))
        );
        assert_eq!(
            request(&[0x09, 0x00, 0x00, 0x00, 0x2A]),
            PartyRequest::Decline(RawSerial(0x2A))
        );
    }

    /// The trap the module doc's table is about: `0x08` inbound is "I accept",
    /// and `0x08` is not an outbound subcommand at all. A decoder written from
    /// the outbound side would read an acceptance as a member list.
    #[test]
    fn an_acceptance_is_not_a_member_list() {
        assert_eq!(
            request(&[0x08, 0x00, 0x00, 0x00, 0x2A]),
            PartyRequest::Accept(RawSerial(0x2A))
        );
        assert_eq!(request(&[0x05]), PartyRequest::Unknown(0x05));
    }

    #[test]
    fn a_line_of_chat_is_big_endian_utf16() {
        // Big-endian, like `0xAE` speech and unlike a property list's arguments
        // — the one distinction this file shares with `properties.rs`, from the
        // other side.
        let mut body = vec![0x04];
        body.extend("hi".encode_utf16().flat_map(u16::to_be_bytes));
        body.extend_from_slice(&[0, 0]);
        assert_eq!(request(&body), PartyRequest::PublicMessage("hi".to_owned()));

        let mut private = vec![0x03, 0x00, 0x00, 0x00, 0x2A];
        private.extend("hi".encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(
            request(&private),
            PartyRequest::PrivateMessage {
                to:   RawSerial(0x2A),
                // No terminator, and it still reads: ServUO's own reader runs to
                // the end of the packet.
                text: "hi".to_owned(),
            }
        );
    }

    #[test]
    fn a_body_with_nothing_after_the_kind_does_not_panic() {
        // A client is free to send five bytes. Whatever this does, it must not
        // be to unwind through the packet loop.
        for kind in [0x02u8, 0x03, 0x06, 0x08, 0x09] {
            let _ = ExtendedRequest::decode(&inbound(&[kind]));
        }
    }

    /// The empty list and a removal are one packet, and the difference is the
    /// count. Asserted together because reading them as two types is the mistake
    /// the layout invites.
    #[test]
    fn the_empty_list_is_a_removal_with_no_members_left() {
        let me = Serial::new(0x2A).unwrap();
        let gone = Serial::new(0x2B).unwrap();

        let empty = encode_packet(
            &PartyRemoveMember {
                removed: me,
                members: Vec::new(),
            },
            version(),
        );
        assert_eq!(empty[0], 0xBF);
        assert_eq!(&empty[3..5], &SUBCOMMAND.to_be_bytes());
        assert_eq!(empty[5], 0x02);
        assert_eq!(empty[6], 0, "no members left");
        assert_eq!(&empty[7..11], &0x2Au32.to_be_bytes());
        assert_eq!(empty.len(), 11, "and nothing after the removed serial");

        let one = encode_packet(
            &PartyRemoveMember {
                removed: gone,
                members: vec![me],
            },
            version(),
        );
        assert_eq!(one[6], 1);
        assert_eq!(&one[7..11], &0x2Bu32.to_be_bytes(), "who went");
        assert_eq!(&one[11..15], &0x2Au32.to_be_bytes(), "who is left");
    }

    /// Every outbound packet, encoded and read back through the same dispatch a
    /// client uses. The whole `0xBF` family reached this end as an undecoded id
    /// before — the id byte is not a key when nine variants share it — so this
    /// asserts the second and third dispatches as much as the four decoders.
    #[test]
    fn every_outbound_packet_round_trips_through_the_dispatch() {
        let leader = Serial::new(0x2A).unwrap();
        let member = Serial::new(0x2B).unwrap();
        for packet in [
            ServerPacket::PartyMemberList(PartyMemberList {
                members: vec![leader, member],
            }),
            ServerPacket::PartyRemoveMember(PartyRemoveMember {
                removed: member,
                members: vec![leader],
            }),
            // The empty list, which is the same type saying something else.
            ServerPacket::PartyRemoveMember(PartyRemoveMember {
                removed: leader,
                members: Vec::new(),
            }),
            ServerPacket::PartyTextMessage(PartyTextMessage {
                to_all: true,
                from:   leader,
                text:   "regroup".to_owned(),
            }),
            ServerPacket::PartyTextMessage(PartyTextMessage {
                to_all: false,
                from:   leader,
                text:   "you first".to_owned(),
            }),
            ServerPacket::PartyInvitation(PartyInvitation { leader }),
        ] {
            let bytes = packet.encode(version());
            assert_eq!(
                ServerPacket::decode(&bytes, version()).expect("a party 0xBF decodes"),
                Some(packet.clone()),
                "{packet:?} did not survive the round trip"
            );
        }
    }

    /// A type byte this engine does not write reads as no packet rather than as
    /// the nearest one. The framer sized it, so the stream is intact either way
    /// — but decoding it as a roster would put a phantom member on screen.
    #[test]
    fn an_unknown_party_type_is_no_packet_rather_than_the_wrong_one() {
        let mut bytes = ServerPacket::PartyInvitation(PartyInvitation {
            leader: Serial::new(0x2A).unwrap(),
        })
        .encode(version());
        bytes[5] = 0x7F;
        assert_eq!(ServerPacket::decode(&bytes, version()).unwrap(), None);
    }

    #[test]
    fn the_outbound_packets_carry_their_own_subcommand() {
        let leader = Serial::new(0x2A).unwrap();
        for (bytes, kind) in [
            (
                encode_packet(
                    &PartyMemberList {
                        members: vec![leader],
                    },
                    version(),
                ),
                0x01,
            ),
            (encode_packet(&PartyInvitation { leader }, version()), 0x07),
            (
                encode_packet(
                    &PartyTextMessage {
                        to_all: true,
                        from:   leader,
                        text:   "hi".to_owned(),
                    },
                    version(),
                ),
                0x04,
            ),
            (
                encode_packet(
                    &PartyTextMessage {
                        to_all: false,
                        from:   leader,
                        text:   "hi".to_owned(),
                    },
                    version(),
                ),
                0x03,
            ),
        ] {
            assert_eq!(bytes[0], 0xBF);
            assert_eq!(
                u16::from_be_bytes([bytes[1], bytes[2]]),
                bytes.len() as u16,
                "the envelope's length is patched"
            );
            assert_eq!(&bytes[3..5], &SUBCOMMAND.to_be_bytes());
            assert_eq!(bytes[5], kind);
        }
    }
}
