//! `0xD7` — the AoS "encoded command", a family of client requests keyed by a
//! subcommand word.
//!
//! The paperdoll's Quest and Guild buttons are here, and nowhere else: they are
//! not gump replies (the paperdoll is drawn entirely client-side and has no
//! server-sent layout to answer), so a shard that does not read `0xD7` has a
//! paperdoll with two dead buttons and no way to tell. The layout is
//! `[0xD7][length u16][serial u32][subcommand u16][payload]`, from ServUO's
//! `PacketHandlers.EncodedCommand` and Sphere's `Event_ExtCmd` equivalent — the
//! two agree.

use crate::codec::{
    PacketReader,
    PacketWriter,
};
use crate::error::{
    DecodeError,
    expect_id,
};
use crate::packet::{
    PacketLength,
    frame_body,
};
use crate::wire::Graphic;

/// `0xD7` — a client request named by its subcommand.
///
/// The header is all a subcommand without a payload has, which was every one of
/// them until the design editor's verbs arrived. [`DesignEdit`] is the payload
/// of the three that carry one, and it is a separate type on purpose: this
/// struct knows the shape of the payloads *this engine reads*, and nothing about
/// the rest of the family.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EncodedCommand {
    /// The entity the command is about — the player's own serial, for the
    /// paperdoll buttons. Class D: never read. Nothing here routes by it —
    /// every subcommand this engine acts on already knows whose paperdoll
    /// sent it from the connection, the same shape as [`RawGumpKey`]'s echo.
    ///
    /// [`RawGumpKey`]: crate::gump::RawGumpKey
    pub serial:     RawEncodedSerial,
    /// Which command, exactly as sent. See [`RawEncodedSubcommand::interpret`].
    pub subcommand: RawEncodedSubcommand,
    /// The change to a house design this `0xD7` asks for, for the three
    /// subcommands that ask for one.
    ///
    /// `Some` exactly when [`subcommand`](Self::subcommand) is one of those
    /// three — [`decode`](Self::decode) refuses the packet rather than
    /// returning a design subcommand with nothing behind it — so the pair
    /// cannot disagree. Absent for every other `0xD7`, which is `Option`'s own
    /// meaning here and not a placeholder: a Quest button carries no edit.
    pub edit:       Option<DesignEdit>,
}

impl EncodedCommand {
    /// The packet id.
    pub const ID: u8 = 0xD7;

    /// Decode a `0xD7`: the header always, and the payload for the subcommands
    /// that carry one this engine reads.
    ///
    /// Every field is read through the bounds-checked reader, so a truncated
    /// packet is an error rather than a panic — the length on the wire is the
    /// client's word, not this end's.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        // The packet carries its own u16 length at offset 1; the framer already
        // sized the slice, so it is read past rather than trusted.
        reader.u16()?;
        let serial = RawEncodedSerial(reader.u32()?);
        let subcommand = RawEncodedSubcommand(reader.u16()?);
        let edit = DesignEdit::decode(&mut reader, subcommand.interpret())?;
        Ok(Self {
            serial,
            subcommand,
            edit,
        })
    }

    /// The `0xD7` this client sends for one subcommand, with whatever that
    /// subcommand writes after the word.
    ///
    /// Private, and reached through the named requests below, because the tail
    /// is *not* a constant: `Send_QuestMenuRequest` writes a zero,
    /// `Send_GuildMenuRequest` a `0x0A`, and a design verb writes a whole
    /// payload. It is the subcommand's own business, short enough in two cases
    /// to look like padding, and a single encoder taking whatever the caller
    /// happened to pass would let one button send the other's.
    fn encode(serial: RawEncodedSerial, subcommand: u16, payload: impl FnOnce(&mut PacketWriter)) -> Vec<u8> {
        frame_body(Self::ID, PacketLength::Variable, |out: &mut PacketWriter| {
            out.u32(serial.0);
            out.u16(subcommand);
            payload(out);
        })
    }
}

/// The paperdoll's Quest button: open the quest log —
/// `GameActions.RequestQuestMenu`, whose packet is `0xD7` subcommand `0x32`
/// followed by a zero byte.
///
/// `serial` is the asking player's own, which is what the reference writes and
/// what this engine ignores on the way in (see [`EncodedCommand::serial`]).
#[must_use]
pub fn quest_log_request(serial: RawEncodedSerial) -> Vec<u8> {
    EncodedCommand::encode(serial, EncodedSubcommand::QUEST_GUMP_REQUEST, |out| out.u8(0x00))
}

/// The paperdoll's Guild button — `0xD7` subcommand `0x28`, and the `0x0A`
/// `Send_GuildMenuRequest` writes after it.
///
/// The shard opens its own guild window on this — founding, the roster, wars and
/// alliances. The button was written before `guilds` existed, on the argument
/// that a packet which never leaves is a defect the day the system lands and one
/// nobody would look for; the system landed, and it was.
#[must_use]
pub fn guild_menu_request(serial: RawEncodedSerial) -> Vec<u8> {
    EncodedCommand::encode(serial, EncodedSubcommand::GUILD_GUMP_REQUEST, |out| out.u8(0x0A))
}

/// The design editor's "lay this piece here" — `0xD7` subcommand `0x06`, the
/// graphic and the tile it goes on.
///
/// No height on the wire: which storey the piece lands on is the session's own
/// [`DesignEdit::Floor`] state, and the shard derives the `z` from it. That
/// asymmetry with [`design_erase_request`] is the reference's own and it is the
/// reason the two are not one function.
#[must_use]
pub fn design_build_request(serial: RawEncodedSerial, graphic: Graphic, dx: i32, dy: i32) -> Vec<u8> {
    EncodedCommand::encode(serial, EncodedSubcommand::DESIGN_BUILD, |out| {
        write_tagged_i32(out, i32::from(graphic.0));
        write_tagged_i32(out, dx);
        write_tagged_i32(out, dy);
        out.u8(PAYLOAD_END);
    })
}

/// The design editor's "take that piece away" — `0xD7` subcommand `0x05`, the
/// graphic and the tile it stands on, height included.
#[must_use]
pub fn design_erase_request(
    serial: RawEncodedSerial,
    graphic: Graphic,
    dx: i32,
    dy: i32,
    dz: i32,
) -> Vec<u8> {
    EncodedCommand::encode(serial, EncodedSubcommand::DESIGN_ERASE, |out| {
        write_tagged_i32(out, i32::from(graphic.0));
        write_tagged_i32(out, dx);
        write_tagged_i32(out, dy);
        write_tagged_i32(out, dz);
        out.u8(PAYLOAD_END);
    })
}

/// The design editor's storey picker — `0xD7` subcommand `0x12`.
///
/// ClassicUO's `Send_CustomHouseGoToFloor` writes this one as a `u32` zero and
/// then the floor byte, which is the same five bytes a tagged value is: the
/// zero is the type byte plus the top three bytes of the number. Written here as
/// the tagged value it is, so one encoder covers all four design verbs.
#[must_use]
pub fn design_floor_request(serial: RawEncodedSerial, storey: RawStorey) -> Vec<u8> {
    EncodedCommand::encode(serial, EncodedSubcommand::DESIGN_FLOOR, |out| {
        write_tagged_i32(out, storey.0);
        out.u8(PAYLOAD_END);
    })
}

/// The design editor's "make this the house" — `0xD7` subcommand `0x04`, and
/// nothing but the terminator behind it.
///
/// A bracket rather than an edit, which is why it carries no [`DesignEdit`]: what
/// is committed is the working design the shard has been keeping all along, and
/// naming it on the wire would let a client commit something the shard never
/// agreed to.
///
/// The reference answers this one with a confirmation window, because it charges
/// gold per component and the player is owed the number before they pay it. This
/// shard has no price on a house at all, so there is nothing to confirm and the
/// commit is the commit.
#[must_use]
pub fn design_commit_request(serial: RawEncodedSerial) -> Vec<u8> {
    EncodedCommand::encode(serial, EncodedSubcommand::DESIGN_COMMIT, |out| {
        out.u8(PAYLOAD_END)
    })
}

/// The design editor's "start again from the house as it stands" — `0xD7`
/// subcommand `0x1A`, and the terminator.
///
/// The opposite of [`design_commit_request`] and not of any one edit: it throws
/// the whole working copy away rather than undoing the last thing done to it.
#[must_use]
pub fn design_revert_request(serial: RawEncodedSerial) -> Vec<u8> {
    EncodedCommand::encode(serial, EncodedSubcommand::DESIGN_REVERT, |out| {
        out.u8(PAYLOAD_END)
    })
}

/// What a `0xD7` asks be done to the house design its sender has open.
///
/// The three verbs of `plans/housing/customisation/PLAN.md`'s step 2, and every
/// one of them is about the *working* copy: nothing here names a revision, and
/// nothing here can be seen by a client that is not the editor. The offsets are
/// the wire's own `i32` rather than the `i16` a
/// [`Component`](openshard_uofiles::multi::Component) carries, deliberately: an
/// offset outside a house is a refusal in the game rules, and a decode failure
/// here would close the connection over a misplaced click.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DesignEdit {
    /// `0x06` — lay `graphic` at this offset, on whichever storey the editor is
    /// on.
    Build {
        /// The piece.
        graphic: Graphic,
        /// East of the house's origin.
        dx:      i32,
        /// South of it.
        dy:      i32,
    },
    /// `0x05` — take away the piece standing at this offset and height.
    Erase {
        /// Which piece, because several stand on one tile.
        graphic: Graphic,
        /// East of the house's origin.
        dx:      i32,
        /// South of it.
        dy:      i32,
        /// And how far up. On the wire here and not on [`Build`](Self::Build)'s:
        /// erasing names a tile that exists, building names one that does not.
        dz:      i32,
    },
    /// `0x12` — edit this storey from now on.
    Floor {
        /// Which one, exactly as sent. See [`RawStorey`].
        storey: RawStorey,
    },
}

impl DesignEdit {
    /// Read the payload of a `0xD7` whose subcommand is one of the three, or
    /// answer `None` for every other subcommand — which is where the reader is
    /// left sitting on bytes nothing reads, exactly as before.
    ///
    /// The trailing `0x0A` both references write after the last value is not
    /// read: it terminates the payload for a reader that is counting, and this
    /// one knows how many values each verb has.
    fn decode(
        reader: &mut PacketReader<'_>,
        subcommand: EncodedSubcommand,
    ) -> Result<Option<Self>, DecodeError> {
        let edit = match subcommand {
            EncodedSubcommand::DesignBuild => {
                Self::Build {
                    graphic: read_tagged_graphic(reader)?,
                    dx:      read_tagged_i32(reader)?,
                    dy:      read_tagged_i32(reader)?,
                }
            }
            EncodedSubcommand::DesignErase => {
                Self::Erase {
                    graphic: read_tagged_graphic(reader)?,
                    dx:      read_tagged_i32(reader)?,
                    dy:      read_tagged_i32(reader)?,
                    dz:      read_tagged_i32(reader)?,
                }
            }
            EncodedSubcommand::DesignFloor => {
                Self::Floor {
                    storey: RawStorey(read_tagged_i32(reader)?),
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(edit))
    }
}

/// Which storey an editor asked to move to, exactly as sent.
///
/// Not yet checked against the house's own ceiling, which is three storeys or
/// four depending on how wide the foundation is — a fact about *that house*,
/// which this crate does not hold. `openshard-housing` does the checking, and
/// clamps rather than refuses, which is the reference's own answer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RawStorey(pub i32);

/// The type byte an encoded payload's four-byte integer is introduced by.
///
/// ServUO's `EncodedReader.ReadInt32` reads a byte and answers zero unless it is
/// this one; ClassicUO writes it before each of `Send_CustomHouseAddItem`'s
/// three values. Answering zero for a wrong tag is what this decoder does *not*
/// do — a payload that is not the shape it claims is malformed, and inventing a
/// zero out of it would lay a piece at the origin.
const INT_TAG: u8 = 0x00;

/// The byte both references write after a design verb's last value. Written on
/// the way out and read past on the way in; nothing carries a meaning.
const PAYLOAD_END: u8 = 0x0A;

/// One tagged four-byte integer of a `0xD7` payload.
fn read_tagged_i32(reader: &mut PacketReader<'_>) -> Result<i32, DecodeError> {
    let tag = reader.u8()?;
    if tag != INT_TAG {
        return Err(DecodeError::UnknownValue {
            field: "0xD7 payload value type",
            value: u32::from(tag),
        });
    }
    Ok(reader.i32()?)
}

/// The same, promoted to the art id it names.
///
/// ServUO masks the value down to a legal item id instead; masking turns a
/// nonsense number into a *real piece* somewhere else in the table, which is a
/// worse answer than refusing the packet.
fn read_tagged_graphic(reader: &mut PacketReader<'_>) -> Result<Graphic, DecodeError> {
    let raw = read_tagged_i32(reader)?;
    let id = u16::try_from(raw).map_err(|_| {
        DecodeError::UnknownValue {
            field: "0xD7 design piece",
            value: raw.cast_unsigned(),
        }
    })?;
    Ok(Graphic(id))
}

/// Write one, the way both references do.
fn write_tagged_i32(out: &mut PacketWriter, value: i32) {
    out.u8(INT_TAG);
    out.i32(value);
}

/// The entity a `0xD7` claims to be about, exactly as sent. No promotion: see
/// [`EncodedCommand::serial`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct RawEncodedSerial(pub u32);

/// A `0xD7` subcommand word exactly as sent, not yet checked against the ones
/// this engine names.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct RawEncodedSubcommand(pub u16);

impl RawEncodedSubcommand {
    /// Total: every subcommand this engine has never seen is `Other`, exactly
    /// [`crate::speech::TalkMode`]'s shape — a byte with a name beats an enum
    /// with a guessed leftover arm.
    #[inline]
    #[must_use]
    pub const fn interpret(self) -> EncodedSubcommand {
        match self.0 {
            EncodedSubcommand::SET_ABILITY => EncodedSubcommand::SetAbility,
            EncodedSubcommand::DESIGN_COMMIT => EncodedSubcommand::DesignCommit,
            EncodedSubcommand::DESIGN_REVERT => EncodedSubcommand::DesignRevert,
            EncodedSubcommand::DESIGN_ERASE => EncodedSubcommand::DesignErase,
            EncodedSubcommand::DESIGN_BUILD => EncodedSubcommand::DesignBuild,
            EncodedSubcommand::END_CUSTOMISATION => EncodedSubcommand::EndCustomisation,
            EncodedSubcommand::DESIGN_FLOOR => EncodedSubcommand::DesignFloor,
            EncodedSubcommand::GUILD_GUMP_REQUEST => EncodedSubcommand::GuildGumpRequest,
            EncodedSubcommand::QUEST_GUMP_REQUEST => EncodedSubcommand::QuestGumpRequest,
            other => EncodedSubcommand::Other(other),
        }
    }
}

/// A `0xD7` subcommand this engine has a name for, or the raw word if it does
/// not.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EncodedSubcommand {
    /// Set a weapon's special ability (AoS). Not acted on: combat has no
    /// abilities yet. Named so the byte layout is not re-derived when it does.
    SetAbility,
    /// The working design is the house from now on — the swap C7 exists for.
    ///
    /// Carries no [`DesignEdit`]: what commits is the design the shard has been
    /// keeping, and a client naming its own would be a client asserting a shape
    /// nothing checked.
    DesignCommit,
    /// Throw the working design away and start again from the committed one.
    /// [`DesignCommit`](Self::DesignCommit)'s opposite, and not any edit's.
    DesignRevert,
    /// A piece was taken off the design being edited. Carries a
    /// [`DesignEdit::Erase`].
    DesignErase,
    /// A piece was laid on the design being edited. Carries a
    /// [`DesignEdit::Build`].
    DesignBuild,
    /// The house-design window was closed — end the customisation session.
    ///
    /// The one design subcommand that is a bracket rather than an edit, and the
    /// first of the family this engine spoke. Its opposite number is not here:
    /// customisation *begins* from the house's own window, server-side, exactly
    /// as the reference's `BeginCustomize` does.
    EndCustomisation,
    /// The editor moved to another storey. Carries a [`DesignEdit::Floor`].
    DesignFloor,
    /// The paperdoll's Guild button — the shard answers it with the guild window.
    GuildGumpRequest,
    /// The paperdoll's Quest button — open the quest log.
    QuestGumpRequest,
    /// A subcommand this engine does not name.
    Other(u16),
}

impl EncodedSubcommand {
    const SET_ABILITY: u16 = 0x19;
    /// ServUO's `Designer_Commit` (`HouseFoundation.cs:811`), ClassicUO's
    /// `Send_CustomHouseCommit`.
    const DESIGN_COMMIT: u16 = 0x04;
    /// ServUO's `Designer_Revert` (`HouseFoundation.cs:825`), ClassicUO's
    /// `Send_CustomHouseRevert`. One word away from [`SET_ABILITY`], which is
    /// `0x19` — the reason both have a test naming the other.
    ///
    /// [`SET_ABILITY`]: Self::SET_ABILITY
    const DESIGN_REVERT: u16 = 0x1A;
    /// ServUO's `Designer_Delete`, registered at `HouseFoundation.cs:812`, and
    /// ClassicUO's `Send_CustomHouseDeleteItem`. Every hex here is read out of
    /// the references rather than guessed, per `style.md`'s "ports name their
    /// source" — and the two agree, which is what makes them worth citing.
    const DESIGN_ERASE: u16 = 0x05;
    /// ServUO's `Designer_Build` (`HouseFoundation.cs:813`), ClassicUO's
    /// `Send_CustomHouseAddItem`.
    const DESIGN_BUILD: u16 = 0x06;
    /// ServUO's `Designer_Close`, registered at `HouseFoundation.cs:815`.
    const END_CUSTOMISATION: u16 = 0x0C;
    /// ServUO's `Designer_Level` (`HouseFoundation.cs:820`), ClassicUO's
    /// `Send_CustomHouseGoToFloor`. "Level" there and *storey* here, because
    /// this engine spends `level` on skills.
    const DESIGN_FLOOR: u16 = 0x12;
    const GUILD_GUMP_REQUEST: u16 = 0x28;
    const QUEST_GUMP_REQUEST: u16 = 0x32;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(serial: u32, subcommand: u16) -> Vec<u8> {
        let mut bytes = vec![0xD7u8, 0, 0];
        bytes.extend_from_slice(&serial.to_be_bytes());
        bytes.extend_from_slice(&subcommand.to_be_bytes());
        let length = u16::try_from(bytes.len()).unwrap();
        bytes[1..3].copy_from_slice(&length.to_be_bytes());
        bytes
    }

    #[test]
    fn an_encoded_command_reads_its_serial_and_subcommand() {
        let got =
            EncodedCommand::decode(&packet(0x0000_1234, EncodedSubcommand::QUEST_GUMP_REQUEST)).unwrap();
        assert_eq!(got.serial, RawEncodedSerial(0x0000_1234));
        assert_eq!(got.subcommand.interpret(), EncodedSubcommand::QuestGumpRequest);
    }

    #[test]
    fn a_truncated_encoded_command_is_refused_not_panicked() {
        let full = packet(1, EncodedSubcommand::GUILD_GUMP_REQUEST);
        for cut in 1..full.len() {
            assert!(
                EncodedCommand::decode(&full[..cut]).is_err(),
                "a {cut}-byte packet must not decode"
            );
        }
    }

    #[test]
    fn another_packet_id_is_not_an_encoded_command() {
        let mut bytes = packet(1, EncodedSubcommand::SET_ABILITY);
        bytes[0] = 0xD6;
        assert!(EncodedCommand::decode(&bytes).is_err());
    }

    /// The two paperdoll buttons, written by this crate and read back by it:
    /// the length field the framer patched, the subcommand each button means,
    /// and the trailing byte that differs between them.
    #[test]
    fn the_two_paperdoll_requests_decode_as_themselves() {
        let quest = quest_log_request(RawEncodedSerial(0x0000_002A));
        assert_eq!(quest.len(), 10, "id, length, serial, subcommand, payload");
        assert_eq!(
            &quest[1..3],
            &10u16.to_be_bytes(),
            "the framer patched the length"
        );
        assert_eq!(quest[9], 0x00, "the quest request's own trailing byte");
        let decoded = EncodedCommand::decode(&quest).unwrap();
        assert_eq!(decoded.serial, RawEncodedSerial(0x0000_002A));
        assert_eq!(
            decoded.subcommand.interpret(),
            EncodedSubcommand::QuestGumpRequest
        );

        let guild = guild_menu_request(RawEncodedSerial(0x0000_002A));
        assert_eq!(guild[9], 0x0A, "and the guild request's is not the same byte");
        assert_eq!(
            EncodedCommand::decode(&guild).unwrap().subcommand.interpret(),
            EncodedSubcommand::GuildGumpRequest
        );
    }

    /// The bracket subcommand, and the neighbour it must not be confused with:
    /// `0x0C` closes the editor, `0x0D` lays a stair and is still nothing here.
    #[test]
    fn the_close_subcommand_is_the_one_the_reference_registers() {
        assert_eq!(
            EncodedCommand::decode(&packet(1, 0x0C))
                .unwrap()
                .subcommand
                .interpret(),
            EncodedSubcommand::EndCustomisation
        );
        assert_eq!(
            RawEncodedSubcommand(0x0D).interpret(),
            EncodedSubcommand::Other(0x0D),
            "the stair subcommand is not the close one"
        );
    }

    /// The two brackets that decide what the working design was for, written by
    /// this crate and read back by it. Ten bytes each and no payload — the whole
    /// of what `Send_CustomHouseCommit` and `Send_CustomHouseRevert` write.
    #[test]
    fn the_commit_and_revert_subcommands_decode_as_themselves() {
        let who = RawEncodedSerial(0x0000_002A);
        for (packet, expected) in [
            (design_commit_request(who), EncodedSubcommand::DesignCommit),
            (design_revert_request(who), EncodedSubcommand::DesignRevert),
        ] {
            assert_eq!(packet.len(), 10, "id, length, serial, subcommand, terminator");
            assert_eq!(packet[9], 0x0A, "the terminator both clients write");
            let decoded = EncodedCommand::decode(&packet).unwrap();
            assert_eq!(decoded.subcommand.interpret(), expected);
            assert_eq!(decoded.edit, None, "a bracket carries no edit");
        }
    }

    /// Revert is `0x1A` and the weapon ability is `0x19`. One word apart, and
    /// the only thing keeping them apart is that both are named.
    #[test]
    fn revert_is_not_the_weapon_ability_beside_it() {
        assert_eq!(
            RawEncodedSubcommand(0x19).interpret(),
            EncodedSubcommand::SetAbility
        );
        assert_eq!(
            RawEncodedSubcommand(0x1A).interpret(),
            EncodedSubcommand::DesignRevert
        );
        // And the neighbours of commit, which this engine does not name: `0x02`
        // and `0x03` are backup and restore, and they are the plan's step 5.
        assert_eq!(
            RawEncodedSubcommand(0x03).interpret(),
            EncodedSubcommand::Other(0x03)
        );
    }

    #[test]
    fn an_unnamed_subcommand_interprets_total_to_other() {
        // N1 amendment 1's shape: every one of the 65536 words this engine has
        // never named still interprets, to the raw word rather than a panic
        // or a guessed name.
        assert_eq!(
            RawEncodedSubcommand(0x7F).interpret(),
            EncodedSubcommand::Other(0x7F)
        );
    }

    /// The three editing verbs, written by this crate and read back by it. What
    /// is asserted beyond the round trip is the *bytes*: a tag before each
    /// value, four bytes behind it, and the terminator both references write —
    /// because the client on the other end is not this one.
    #[test]
    fn the_three_design_verbs_decode_as_themselves() {
        let who = RawEncodedSerial(0x0000_002A);

        let build = design_build_request(who, Graphic(0x0007), -3, 4);
        assert_eq!(
            &build[9..],
            &[
                0x00, 0x00, 0x00, 0x00, 0x07, // the piece
                0x00, 0xFF, 0xFF, 0xFF, 0xFD, // dx, signed
                0x00, 0x00, 0x00, 0x00, 0x04, // dy
                0x0A, // and the terminator
            ],
            "the payload ClassicUO's Send_CustomHouseAddItem writes"
        );
        assert_eq!(
            EncodedCommand::decode(&build).unwrap().edit,
            Some(DesignEdit::Build {
                graphic: Graphic(0x0007),
                dx:      -3,
                dy:      4,
            })
        );

        let erase = design_erase_request(who, Graphic(0x0006), 1, 2, 7);
        assert_eq!(
            EncodedCommand::decode(&erase).unwrap().edit,
            Some(DesignEdit::Erase {
                graphic: Graphic(0x0006),
                dx:      1,
                dy:      2,
                dz:      7,
            })
        );

        let floor = design_floor_request(who, RawStorey(2));
        assert_eq!(
            EncodedCommand::decode(&floor).unwrap().edit,
            Some(DesignEdit::Floor { storey: RawStorey(2) })
        );
    }

    /// ClassicUO writes the storey as a `u32` zero and then the floor byte,
    /// which is byte-for-byte the tagged value this crate writes. Asserted
    /// against the reference's own bytes rather than against our encoder, since
    /// agreeing with ourselves proves nothing.
    #[test]
    fn the_storey_request_is_the_five_bytes_the_client_sends() {
        let mut bytes = vec![0xD7u8, 0, 0];
        bytes.extend_from_slice(&0x0000_002Au32.to_be_bytes());
        bytes.extend_from_slice(&0x12u16.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes()); // Send_CustomHouseGoToFloor's zero
        bytes.push(3); // and the floor byte behind it
        bytes.push(0x0A);
        let length = u16::try_from(bytes.len()).unwrap();
        bytes[1..3].copy_from_slice(&length.to_be_bytes());

        assert_eq!(
            EncodedCommand::decode(&bytes).unwrap().edit,
            Some(DesignEdit::Floor { storey: RawStorey(3) })
        );
        assert_eq!(
            bytes,
            design_floor_request(RawEncodedSerial(0x0000_002A), RawStorey(3)),
            "and this crate writes the same packet the client does"
        );
    }

    /// A design subcommand with nothing behind it is refused rather than
    /// answered with a zeroed edit — the reference's `EncodedReader` returns
    /// zero for a wrong type byte, which would lay a piece at the origin.
    #[test]
    fn a_design_verb_with_a_malformed_payload_is_refused() {
        assert!(
            EncodedCommand::decode(&packet(1, 0x06)).is_err(),
            "a build with no payload at all"
        );

        let mut wrong_tag = design_build_request(RawEncodedSerial(1), Graphic(7), 0, 0);
        wrong_tag[9] = 0x03; // ReadPoint3D's tag, not ReadInt32's
        assert!(
            EncodedCommand::decode(&wrong_tag).is_err(),
            "a value introduced as some other type"
        );

        let mut no_such_piece = design_build_request(RawEncodedSerial(1), Graphic(7), 0, 0);
        no_such_piece[10..14].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        assert!(
            EncodedCommand::decode(&no_such_piece).is_err(),
            "an art id past the end of every client's table"
        );
    }

    /// Every truncation of every design verb, for the reason the paperdoll's own
    /// truncation test exists: the payload is read through the bounds-checked
    /// reader, so a short packet is an error and never a panic.
    #[test]
    fn a_truncated_design_verb_is_refused_not_panicked() {
        let who = RawEncodedSerial(1);
        for full in [
            design_build_request(who, Graphic(7), 1, 1),
            design_erase_request(who, Graphic(7), 1, 1, 7),
            design_floor_request(who, RawStorey(1)),
        ] {
            // Down to the subcommand word: everything shorter than the whole
            // payload has to be refused, and the last byte is the terminator
            // nothing reads, so the cut that keeps it all is the whole packet.
            for cut in 1..full.len() - 1 {
                assert!(
                    EncodedCommand::decode(&full[..cut]).is_err(),
                    "a {cut}-byte design verb must not decode"
                );
            }
        }
    }

    /// A `0xD7` that is not one of the three carries no edit, and the pair
    /// cannot disagree: the payload is decoded from the subcommand.
    #[test]
    fn a_subcommand_with_no_edit_carries_none() {
        for subcommand in [
            EncodedSubcommand::QUEST_GUMP_REQUEST,
            EncodedSubcommand::END_CUSTOMISATION,
            0x0D,
        ] {
            assert_eq!(
                EncodedCommand::decode(&packet(1, subcommand)).unwrap().edit,
                None,
                "0x{subcommand:02X} is not an edit"
            );
        }
    }
}
