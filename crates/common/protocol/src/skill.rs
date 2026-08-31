//! The skill window: the `0x3A` packets in both directions, and the `0x12`
//! text-command a client sends to *use* a skill.
//!
//! What a skill *does* — mine the ore, pick the lock, hide — is not here and not
//! the engine's, the same decoupling casting has: this is only the wire. The
//! server sends the client its skills so the window fills ([`SkillsFull`]),
//! updates one line when a skill changes ([`SkillUpdate`]), reads the arrow
//! the player clicks ([`SkillLockRequest`]), and reads "use skill N"
//! ([`UseSkillRequest`]). The byte layout is ServUO's `SkillUpdate`/`SkillChange`
//! and its `ChangeSkillLock`/`TextCommand` handlers.

use crate::codec::{
    PacketReader,
    PacketWriter,
};
use crate::error::{
    DecodeError,
    expect_id,
};
use crate::feature::Feature;
use crate::packet::{
    DecodePacket,
    EncodePacket,
    PacketLength,
    frame_body,
};
use crate::version::ClientVersion;
use crate::wire::RawSkillId;

/// How the skill window is set to train a skill — ServUO's `SkillLock`. The wire
/// byte the `0x3A` packets carry.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum SkillLock {
    /// Train up on use — the default.
    #[default]
    Up,
    /// Train down (atrophy toward the floor) to make room under the cap.
    Down,
    /// Held fixed: neither gains nor falls.
    Locked,
}

impl SkillLock {
    /// The wire byte.
    #[must_use]
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Up => 0,
            Self::Down => 1,
            Self::Locked => 2,
        }
    }

    /// From the wire byte; anything unknown reads as `Up`, the default.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Down,
            2 => Self::Locked,
            _ => Self::Up,
        }
    }
}

/// One skill's line in a `0x3A` packet, every value in tenths (so 75.5 is 755).
///
/// Every field here is a bare integer by decision, and both halves of N10's
/// allowlist are represented: `id` is server-chosen (class A) but its domain
/// type, `openshard_state::Skill`, lives in a server crate above `protocol`
/// and cannot be held here — the same reason `version.rs`'s `ClientVersion`
/// components and `feedback.rs`'s animation numbers stay plain. `value`,
/// `base` and `cap` are quantities — trained, computed and clamped in
/// `openshard_skills` and `[gameplay]` config, far above `protocol` — exactly
/// `mobile::Vitals`'s argument. See `docs/protocol_newtypes.md`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SkillEntry {
    /// The skill id, zero-based (Alchemy is 0), as the client numbers them.
    pub id:    u8,
    /// The value in play — base plus any item/buff modifier, capped. No modifiers
    /// exist yet, so it equals `base` for now.
    pub value: u16,
    /// The trained value, before modifiers.
    pub base:  u16,
    /// How the window trains it.
    pub lock:  SkillLock,
    /// The individual skill cap.
    pub cap:   u16,
}

/// How many skills a client of this version knows — the length of the full list,
/// so the window fills completely without overrunning an older client's fixed
/// skill array. The table grew with the expansions.
#[must_use]
pub fn skill_count(version: ClientVersion) -> usize {
    if version.supports(Feature::SaPackets) {
        58 // + Mysticism, Imbuing, Throwing
    } else if version.supports(Feature::MlPackets) {
        55 // + Spellweaving
    } else if version.supports(Feature::SePackets) {
        54 // + Bushido, Ninjitsu
    } else if version.supports(Feature::AosPackets) {
        52 // + Necromancy, Chivalry, Focus
    } else {
        49 // Alchemy .. RemoveTrap
    }
}

/// The full skill list (`0x3A`) — every skill, to fill the window on login.
///
/// [`Feature::SkillCaps`] (since 4.0.0a) adds the per-skill cap field and
/// switches the type byte to `0x02`; an older client gets the shorter `0x00`
/// form. The ids ride one-based here, terminated by a zero id — the classic
/// quirk that lets skill 0 (Alchemy) coexist with the terminator.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SkillsFull {
    /// Every skill the client's window shows.
    pub entries: Vec<SkillEntry>,
}

impl EncodePacket for SkillsFull {
    const ID: u8 = 0x3A;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, version: ClientVersion) {
        let caps = version.supports(Feature::SkillCaps);
        out.u8(if caps { 0x02 } else { 0x00 }); // absolute, capped or not
        for entry in &self.entries {
            out.u16(u16::from(entry.id) + 1); // one-based; a zero id terminates
            out.u16(entry.value);
            out.u16(entry.base);
            out.u8(entry.lock.to_bits());
            if caps {
                out.u16(entry.cap);
            }
        }
        out.u16(0); // terminator
    }
}

/// Which of the two `0x3A` packets a body is, and whether its rows carry a cap.
///
/// The one place in this protocol where the id is not the whole answer: both
/// packets are `0x3A`, and the byte after the length field says which — so a
/// client dispatching on the id alone can route the packet but not decode it.
/// That byte also says whether the rows carry the cap field, and **it is the
/// byte that is believed, not the version**: the version says what a shard
/// *should* send, and this says what it did. A decoder that asked
/// [`Feature::SkillCaps`] instead would read every field of every row two bytes
/// out of place the first time the two disagreed.
///
/// The reference's own reading — `PacketHandlers.UpdateSkills`, whose two lines
/// `haveCap = type != 0 && type <= 0x03 || type == 0xDF` and `isSingleUpdate =
/// type == 0xFF || type == 0xDF` are this whole table.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SkillsForm {
    /// Every skill, ids one-based and the list zero-terminated — [`SkillsFull`].
    WholeList {
        /// Whether each row ends with the skill's cap.
        caps: bool,
    },
    /// One skill, id zero-based, no terminator — [`SkillUpdate`].
    OneLine {
        /// Whether the row ends with the skill's cap.
        caps: bool,
    },
    /// `0xFE` — the *names* of the skills, which a shard may send to replace the
    /// client's own `skills.mul` table. Recognised so that it can be refused by
    /// name rather than read as a list of values; this engine never sends one.
    NameTable,
}

impl SkillsForm {
    /// What a type byte means, or `None` for one that means nothing.
    #[must_use]
    pub const fn of(type_byte: u8) -> Option<Self> {
        match type_byte {
            0x00 => Some(Self::WholeList { caps: false }),
            // 0x01 and 0x03 are the capped whole list *and* an instruction to
            // open the window, which this client does not take: its window opens
            // when the player asks for it. See `docs/client.md`'s backlog.
            0x01..=0x03 => Some(Self::WholeList { caps: true }),
            0xDF => Some(Self::OneLine { caps: true }),
            0xFE => Some(Self::NameTable),
            0xFF => Some(Self::OneLine { caps: false }),
            _ => None,
        }
    }
}

/// A `0x3A` off the wire, whichever of the two it turned out to be.
///
/// A decoder and not a packet: nothing encodes this. It exists because
/// [`DecodePacket`] routes on the id and the id is shared, so the split happens
/// one byte later — and it happens *once*, here, rather than in each of the two
/// decoders plus the dispatcher that picks between them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SkillsPacket {
    /// The list that fills a window.
    WholeList(SkillsFull),
    /// The line that follows a gain.
    OneLine(SkillUpdate),
}

/// Read one row: id, value, base, lock, and the cap if this form carries one.
///
/// `one_based` is the whole-list form's quirk — see [`SkillsFull`] — and is why
/// a zero id can terminate the list without colliding with Alchemy.
fn decode_entry(
    reader: &mut PacketReader<'_>,
    caps: bool,
    one_based: bool,
) -> Result<SkillEntry, DecodeError> {
    let raw = reader.u16()?;
    let id = if one_based {
        raw - 1 // the caller has already refused a zero
    } else {
        raw
    };
    let id = u8::try_from(id).map_err(|_| {
        DecodeError::UnknownValue {
            field: "skill id",
            value: u32::from(id),
        }
    })?;
    let value = reader.u16()?;
    let base = reader.u16()?;
    let lock = SkillLock::from_bits(reader.u8()?);
    // A capless row is refused rather than given a cap: 1000 is the reference's
    // own stand-in, and a window drawing an invented ceiling as if the shard had
    // sent one is worse than a client that says it cannot read this form. The
    // form only reaches a client that declared itself older than 4.0.0a, and the
    // day one does, this is where `cap` becomes an `Option`.
    if !caps {
        return Err(DecodeError::Unsupported {
            packet: <SkillsFull as EncodePacket>::ID,
            form:   "a capless skill row, from before 4.0.0a",
        });
    }
    let cap = reader.u16()?;
    Ok(SkillEntry {
        id,
        value,
        base,
        lock,
        cap,
    })
}

impl DecodePacket for SkillsPacket {
    const ID: u8 = 0x3A;

    /// Decode whichever `0x3A` this is.
    ///
    /// The whole list ends at its zero id — the terminator the one-based
    /// numbering exists to make unambiguous — or at the end of the body, because
    /// a shard that sent the count in the length field and no terminator is
    /// still saying the same thing.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let byte = reader.u8()?;
        let form = SkillsForm::of(byte).ok_or(DecodeError::UnknownValue {
            field: "skill packet type",
            value: u32::from(byte),
        })?;
        match form {
            SkillsForm::WholeList { caps } => {
                let mut entries = Vec::new();
                while !reader.is_empty() {
                    // Peek at the id: a zero ends the list, and a list that ends
                    // exactly at the terminator has nothing after it to read.
                    let mut ahead = reader.clone();
                    if ahead.u16()? == 0 {
                        break;
                    }
                    entries.push(decode_entry(reader, caps, true)?);
                }
                Ok(Self::WholeList(SkillsFull { entries }))
            }
            SkillsForm::OneLine { caps } => {
                Ok(Self::OneLine(SkillUpdate {
                    entry: decode_entry(reader, caps, false)?,
                }))
            }
            SkillsForm::NameTable => {
                Err(DecodeError::Unsupported {
                    packet: Self::ID,
                    form:   "the shard's own skill-name table (0xFE)",
                })
            }
        }
    }
}

/// A single skill's update (`0x3A`), sent when one changes so an open window
/// follows a gain. The id rides zero-based here, and there is no terminator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SkillUpdate {
    /// The skill that changed.
    pub entry: SkillEntry,
}

impl EncodePacket for SkillUpdate {
    const ID: u8 = 0x3A;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, version: ClientVersion) {
        let caps = version.supports(Feature::SkillCaps);
        out.u8(if caps { 0xDF } else { 0xFF }); // delta, capped or not
        out.u16(u16::from(self.entry.id));
        out.u16(self.entry.value);
        out.u16(self.entry.base);
        out.u8(self.entry.lock.to_bits());
        if caps {
            out.u16(self.entry.cap);
        }
    }
}

/// `0x3A` from the client — the player clicked a skill's up/down/lock arrow.
/// ServUO's `ChangeSkillLock`.
///
/// `skill` is never checked against `openshard_state::Skill`'s table before
/// this stage — `Skills::set_lock` took whatever arrived, a `HashMap` insert
/// with no range on the path at all. It is unwrapped and validated at the
/// seam that owns the skill list, `World::set_skill_lock`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SkillLockRequest {
    /// Which skill, zero-based, exactly as sent.
    pub skill: RawSkillId,
    /// The new lock state.
    pub lock:  SkillLock,
}

impl DecodePacket for SkillLockRequest {
    const ID: u8 = 0x3A;

    /// Decode the incoming skill-lock request.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        // The wire carries the id as a word; every skill id fits a byte.
        let skill = RawSkillId(reader.u16()? as u8);
        let lock = SkillLock::from_bits(reader.u8()?);
        Ok(Self { skill, lock })
    }
}

impl SkillLockRequest {
    /// Encode a whole `0x3A` lock request. What `crates/client/net` sends when
    /// the player clicks a skill's lock arrow; this server never sends one,
    /// only ever decodes it — the same split as
    /// [`DoubleClick::encode`](crate::containers::DoubleClick::encode).
    ///
    /// **Unanswered by design.** ServUO's own client redraws the arrow the
    /// instant it is clicked and never waits for a reply, which is why
    /// `World::set_skill_lock` sends nothing back — see that function's own
    /// doc. A caller that held this off until an answer arrived would show the
    /// old face forever.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        // `Variable`, not `Fixed`: `packet::client_packet_length` files this
        // client-originated `0x3A` as variable-length, so the wire carries a
        // length field even though every lock request is the same six bytes —
        // `decode_packet`'s own dispatch skips exactly two bytes for it before
        // calling `decode_body`, which does not read one itself.
        frame_body(
            <Self as DecodePacket>::ID,
            PacketLength::Variable,
            |out: &mut PacketWriter| {
                out.u16(u16::from(self.skill.0));
                out.u8(self.lock.to_bits());
            },
        )
    }
}

/// `0x12` — a client text command. The engine cares about one type, `0x24`
/// ("use skill"), whose payload is the skill index as an ASCII string. ServUO's
/// `TextCommand` case `0x24` → `Skills.UseSkill`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct UseSkillRequest {
    /// Which skill, zero-based, exactly as sent — checked against the table by
    /// `openshard_skills::use_skill_button`, which already refused an id past
    /// it before this stage; the type just makes that visible.
    pub skill: RawSkillId,
}

impl UseSkillRequest {
    /// The packet id — the text-command envelope.
    pub const ID: u8 = 0x12;
    /// The command type that means "use skill".
    const TYPE_USE_SKILL: u8 = 0x24;

    /// Decode a `0x12`, returning the skill request if that is what it is. Any
    /// other command type reads as `None` rather than an error, so the dispatcher
    /// can pass on the ones it does not handle (an emote, a `go`, an open-book).
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, DecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        let _length = reader.u16()?;
        let kind = reader.u8()?;
        if kind != Self::TYPE_USE_SKILL {
            return Ok(None);
        }
        // "N" or "N 0" — the index, maybe with a trailing field the engine
        // ignores. A payload that is not a number is not a use we can act on.
        let command = reader.null_terminated_string()?;
        Ok(command
            .split(' ')
            .next()
            .unwrap_or("")
            .trim()
            .parse::<u8>()
            .ok()
            .map(|skill| {
                Self {
                    skill: RawSkillId(skill),
                }
            }))
    }

    /// Encode a whole `0x12` "use skill" text command. What `crates/client/net`
    /// sends when the player presses a skill's own use button; this server
    /// never sends one, only ever decodes it — the same split as
    /// [`SkillLockRequest::encode`].
    ///
    /// The index as an ASCII decimal string, null-terminated, and nothing
    /// after it — `decode`'s own `split(' ').next()` tolerates a trailing
    /// field the reference sometimes sends, but nothing here needs to write
    /// one.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        frame_body(Self::ID, PacketLength::Variable, |out: &mut PacketWriter| {
            out.u8(Self::TYPE_USE_SKILL);
            out.null_terminated_string(&self.skill.0.to_string());
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{
        decode_packet,
        encode_packet,
    };

    fn aos() -> ClientVersion {
        ClientVersion::new(4, 0, 0, 0)
    }

    fn pre_aos() -> ClientVersion {
        ClientVersion::new(3, 0, 0, 0)
    }

    /// Decode a `0x3A` the way a client does — through the dispatcher, and not
    /// through `SkillsPacket::decode_body` directly.
    ///
    /// The routing is half of what is under test: a `ServerPacket` the client
    /// has no arm for answers `Ok(None)`, is stepped over, and is never heard
    /// of again, which is how the `0x72` answering the war toggle stayed
    /// invisible for as long as the toggle existed. A test that called the
    /// decoder itself would pass with the arm missing.
    fn decode_server_packet(bytes: &[u8]) -> crate::server_packet::ServerPacket {
        match crate::server_packet::ServerPacket::decode(bytes, aos()) {
            Ok(Some(packet)) => packet,
            other => panic!("0x3A did not decode as a skill packet: {other:?}"),
        }
    }

    /// The same, for the packets that must be refused.
    fn decode_server_error(bytes: &[u8]) -> DecodeError {
        match crate::server_packet::ServerPacket::decode(bytes, aos()) {
            Err(crate::server_packet::ServerDecodeError::Skills(error)) => error,
            other => panic!("0x3A decoded, and should not have: {other:?}"),
        }
    }

    #[test]
    fn skill_locks_round_trip_through_the_wire_byte() {
        for lock in [SkillLock::Up, SkillLock::Down, SkillLock::Locked] {
            assert_eq!(SkillLock::from_bits(lock.to_bits()), lock);
        }
        assert_eq!(SkillLock::from_bits(99), SkillLock::Up, "unknown reads as Up");
    }

    #[test]
    fn the_skill_count_grows_with_the_expansions() {
        assert_eq!(skill_count(pre_aos()), 49);
        assert_eq!(skill_count(aos()), 52);
        assert_eq!(skill_count(ClientVersion::new(7, 0, 0, 0)), 58);
    }

    #[test]
    fn the_full_list_is_one_based_and_zero_terminated_with_caps() {
        let entries = [
            SkillEntry {
                id:    0, // Alchemy — sent as 1, so the 0 terminator is unambiguous
                value: 755,
                base:  700,
                lock:  SkillLock::Locked,
                cap:   1000,
            },
            SkillEntry {
                id:    45, // Mining
                value: 500,
                base:  500,
                lock:  SkillLock::Up,
                cap:   1000,
            },
        ];
        let packet = encode_packet(
            &SkillsFull {
                entries: entries.to_vec(),
            },
            aos(),
        );
        assert_eq!(packet[0], 0x3A);
        assert_eq!(
            u16::from_be_bytes([packet[1], packet[2]]) as usize,
            packet.len(),
            "the length field matches the packet"
        );
        assert_eq!(packet[3], 0x02, "the capped absolute type");
        // First entry, at offset 4: id+1, value, base, lock, cap.
        assert_eq!(
            u16::from_be_bytes([packet[4], packet[5]]),
            1,
            "Alchemy is sent as 1"
        );
        assert_eq!(u16::from_be_bytes([packet[6], packet[7]]), 755);
        assert_eq!(u16::from_be_bytes([packet[8], packet[9]]), 700);
        assert_eq!(packet[10], SkillLock::Locked.to_bits());
        assert_eq!(u16::from_be_bytes([packet[11], packet[12]]), 1000);
        // Second entry at 13: id 45 → 46.
        assert_eq!(u16::from_be_bytes([packet[13], packet[14]]), 46);
        // Terminator: the last two bytes are a zero id.
        let end = packet.len();
        assert_eq!(u16::from_be_bytes([packet[end - 2], packet[end - 1]]), 0);
        assert_eq!(end, 4 + 2 * 9 + 2, "type + two 9-byte entries + terminator");
    }

    #[test]
    fn the_full_list_drops_the_cap_field_on_an_old_client() {
        let entries = [SkillEntry {
            id:    0,
            value: 100,
            base:  100,
            lock:  SkillLock::Up,
            cap:   1000,
        }];
        let packet = encode_packet(
            &SkillsFull {
                entries: entries.to_vec(),
            },
            pre_aos(),
        );
        assert_eq!(packet[3], 0x00, "the uncapped absolute type");
        assert_eq!(packet.len(), 4 + 7 + 2, "type + one 7-byte entry + terminator");
    }

    #[test]
    fn a_single_update_is_zero_based_and_unterminated() {
        let entry = SkillEntry {
            id:    25, // Magery
            value: 501,
            base:  501,
            lock:  SkillLock::Up,
            cap:   1000,
        };
        let packet = encode_packet(&SkillUpdate { entry }, aos());
        assert_eq!(packet[0], 0x3A);
        assert_eq!(packet[3], 0xDF, "the capped delta type");
        assert_eq!(u16::from_be_bytes([packet[4], packet[5]]), 25, "zero-based here");
        assert_eq!(packet.len(), 13);
    }

    /// The full list, back off the wire the way a client reads it: the ids come
    /// back zero-based, every field lands where it was put, and the terminator
    /// is not a row.
    #[test]
    fn the_whole_list_comes_back_off_the_wire_as_it_was_sent() {
        let entries = vec![
            SkillEntry {
                id:    0, // Alchemy, the id the one-based numbering exists for
                value: 755,
                base:  700,
                lock:  SkillLock::Locked,
                cap:   1000,
            },
            SkillEntry {
                id:    45,
                value: 500,
                base:  480,
                lock:  SkillLock::Down,
                cap:   1200,
            },
            SkillEntry {
                id:    57, // the last skill a 7.0 client knows
                value: 0,
                base:  0,
                lock:  SkillLock::Up,
                cap:   1000,
            },
        ];
        let packet = encode_packet(
            &SkillsFull {
                entries: entries.clone(),
            },
            aos(),
        );
        let decoded = decode_server_packet(&packet);
        assert_eq!(
            decoded,
            crate::server_packet::ServerPacket::SkillsFull(SkillsFull { entries })
        );
    }

    /// The delta, whose id is *not* one-based — read as if it were, every single
    /// update would train the skill above the one that moved.
    #[test]
    fn a_single_line_comes_back_on_the_skill_it_was_sent_for() {
        let entry = SkillEntry {
            id:    25, // Magery
            value: 501,
            base:  501,
            lock:  SkillLock::Up,
            cap:   1000,
        };
        let packet = encode_packet(&SkillUpdate { entry }, aos());
        let decoded = decode_server_packet(&packet);
        assert_eq!(
            decoded,
            crate::server_packet::ServerPacket::SkillUpdate(SkillUpdate { entry })
        );
    }

    /// The type byte is the whole of the difference between the two, and it is
    /// what is believed about the cap field as well.
    #[test]
    fn the_type_byte_says_which_packet_and_whether_the_rows_carry_a_cap() {
        assert_eq!(SkillsForm::of(0x00), Some(SkillsForm::WholeList { caps: false }));
        assert_eq!(SkillsForm::of(0x02), Some(SkillsForm::WholeList { caps: true }));
        assert_eq!(SkillsForm::of(0x03), Some(SkillsForm::WholeList { caps: true }));
        assert_eq!(SkillsForm::of(0xDF), Some(SkillsForm::OneLine { caps: true }));
        assert_eq!(SkillsForm::of(0xFF), Some(SkillsForm::OneLine { caps: false }));
        assert_eq!(SkillsForm::of(0xFE), Some(SkillsForm::NameTable));
        assert_eq!(SkillsForm::of(0x7A), None, "nothing names this");
    }

    /// A form this crate knows of and does not read says so, rather than reading
    /// its bytes as the form it does know — which would decode, and would be
    /// wrong from the first row.
    #[test]
    fn a_shard_sent_name_table_is_refused_by_name() {
        // 0x3A, length, type 0xFE, a count and nothing else.
        let packet = [0x3A, 0x00, 0x06, 0xFE, 0x00, 0x01];
        let error = decode_server_error(&packet);
        assert!(
            matches!(error, DecodeError::Unsupported { packet: 0x3A, .. }),
            "{error:?}"
        );
    }

    /// The capless rows of a pre-AoS client are refused for the same reason, and
    /// not given the reference's stand-in cap of 1000: a window drawing a ceiling
    /// nobody sent cannot be told from one drawing a ceiling somebody did.
    #[test]
    fn a_capless_row_is_refused_rather_than_given_an_invented_cap() {
        let packet = encode_packet(
            &SkillsFull {
                entries: vec![SkillEntry {
                    id:    0,
                    value: 100,
                    base:  100,
                    lock:  SkillLock::Up,
                    cap:   1000,
                }],
            },
            pre_aos(),
        );
        let error = decode_server_error(&packet);
        assert!(matches!(error, DecodeError::Unsupported { .. }), "{error:?}");
    }

    #[test]
    fn a_lock_request_reads_its_skill_and_lock() {
        // 0x3A, length, skill(u16)=45, lock=1 (down).
        let packet = [0x3A, 0x00, 0x06, 0x00, 0x2D, 0x01];
        let request: SkillLockRequest = decode_packet(&packet, aos()).unwrap();
        assert_eq!(request.skill, RawSkillId(45));
        assert_eq!(request.lock, SkillLock::Down);
    }

    #[test]
    fn a_lock_request_past_the_skill_table_decodes_cleanly() {
        // N9's decode half: 255 names no skill, but nothing here refuses it —
        // that check is `openshard_state::Skill::from_id`, at the seam that
        // owns the skill list (`World::set_skill_lock`), not in this decoder.
        let packet = [0x3A, 0x00, 0x06, 0x00, 0xFF, 0x00];
        let request: SkillLockRequest = decode_packet(&packet, aos()).unwrap();
        assert_eq!(request.skill, RawSkillId(255));
    }

    #[test]
    fn a_use_skill_command_reads_its_index() {
        // 0x12, length, type 0x24, "45\0".
        let mut packet = vec![0x12u8, 0x00, 0x00, 0x24];
        packet.extend_from_slice(b"45\0");
        let len = packet.len() as u16;
        packet[1..3].copy_from_slice(&len.to_be_bytes());
        let request = UseSkillRequest::decode(&packet).unwrap().unwrap();
        assert_eq!(request.skill, RawSkillId(45), "Mining, zero-based");
    }

    #[test]
    fn another_text_command_is_not_a_skill_use() {
        // Type 0xC7 (animate) is not a skill use.
        let mut packet = vec![0x12u8, 0x00, 0x00, 0xC7];
        packet.extend_from_slice(b"bow\0");
        let len = packet.len() as u16;
        packet[1..3].copy_from_slice(&len.to_be_bytes());
        assert_eq!(UseSkillRequest::decode(&packet).unwrap(), None);
    }

    /// Both encoders, routed through the same dispatcher a real connection
    /// uses — `doll.rs`'s
    /// `every_button_reaches_the_server_as_the_packet_it_means` reason: a
    /// missing `ClientPacket` arm would leave `encode` looking correct while
    /// the server never heard it.
    #[test]
    fn a_lock_click_reaches_the_server_as_the_request_it_means() {
        let sent = SkillLockRequest {
            skill: RawSkillId(45),
            lock:  SkillLock::Locked,
        };
        let heard = crate::client_packet::ClientPacket::decode(&sent.encode(), aos()).expect("it decodes");
        assert!(matches!(
            heard,
            crate::client_packet::ClientPacket::SkillLock(request) if request == sent
        ));
    }

    #[test]
    fn a_use_skill_press_reaches_the_server_as_the_request_it_means() {
        let sent = UseSkillRequest {
            skill: RawSkillId(45),
        };
        let heard = crate::client_packet::ClientPacket::decode(&sent.encode(), aos()).expect("it decodes");
        assert!(matches!(
            heard,
            crate::client_packet::ClientPacket::UseSkill(request) if request == sent
        ));
    }
}
