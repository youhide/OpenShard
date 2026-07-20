//! The skill window: the `0x3A` packets in both directions, and the `0x12`
//! text-command a client sends to *use* a skill.
//!
//! What a skill *does* — mine the ore, pick the lock, hide — is not here and not
//! the engine's, the same decoupling casting has: this is only the wire. The
//! server sends the client its skills so the window fills (`encode_skills_full`),
//! updates one line when a skill changes (`encode_skill_update`), reads the arrow
//! the player clicks (`SkillLockRequest`), and reads "use skill N"
//! (`UseSkillRequest`). The byte layout is ServUO's `SkillUpdate`/`SkillChange`
//! and its `ChangeSkillLock`/`TextCommand` handlers.

use crate::codec::PacketWriter;
use crate::feature::Feature;
use crate::login::{expect_id, LoginDecodeError};
use crate::version::ClientVersion;

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
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SkillEntry {
    /// The skill id, zero-based (Alchemy is 0), as the client numbers them.
    pub id: u8,
    /// The value in play — base plus any item/buff modifier, capped. No modifiers
    /// exist yet, so it equals `base` for now.
    pub value: u16,
    /// The trained value, before modifiers.
    pub base: u16,
    /// How the window trains it.
    pub lock: SkillLock,
    /// The individual skill cap.
    pub cap: u16,
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

/// Patch the two-byte length placeholder a variable packet leaves at `[1..3]`.
fn framed(writer: PacketWriter) -> Vec<u8> {
    let mut bytes = writer.into_bytes();
    let length = u16::try_from(bytes.len()).expect("a skill packet outgrew its u16 length");
    bytes[1..3].copy_from_slice(&length.to_be_bytes());
    bytes
}

/// The full skill list (`0x3A`) — every skill, to fill the window on login.
///
/// `caps` (`Feature::SkillCaps`, since 4.0.0a) adds the per-skill cap field and
/// switches the type byte to `0x02`; an older client gets the shorter `0x00`
/// form. The ids ride one-based here, terminated by a zero id — the classic
/// quirk that lets skill 0 (Alchemy) coexist with the terminator.
#[must_use]
pub fn encode_skills_full(entries: &[SkillEntry], caps: bool) -> Vec<u8> {
    let per = if caps { 9 } else { 7 };
    let mut writer = PacketWriter::with_capacity(5 + entries.len() * per + 2);
    writer.u8(0x3A);
    writer.u16(0); // length, patched by `framed`
    writer.u8(if caps { 0x02 } else { 0x00 }); // absolute, capped or not
    for entry in entries {
        writer.u16(u16::from(entry.id) + 1); // one-based; a zero id terminates
        writer.u16(entry.value);
        writer.u16(entry.base);
        writer.u8(entry.lock.to_bits());
        if caps {
            writer.u16(entry.cap);
        }
    }
    writer.u16(0); // terminator
    framed(writer)
}

/// A single skill's update (`0x3A`), sent when one changes so an open window
/// follows a gain. The id rides zero-based here, and there is no terminator.
#[must_use]
pub fn encode_skill_update(entry: &SkillEntry, caps: bool) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(13);
    writer.u8(0x3A);
    writer.u16(0);
    writer.u8(if caps { 0xDF } else { 0xFF }); // delta, capped or not
    writer.u16(u16::from(entry.id));
    writer.u16(entry.value);
    writer.u16(entry.base);
    writer.u8(entry.lock.to_bits());
    if caps {
        writer.u16(entry.cap);
    }
    framed(writer)
}

/// `0x3A` from the client — the player clicked a skill's up/down/lock arrow.
/// ServUO's `ChangeSkillLock`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SkillLockRequest {
    /// Which skill, zero-based.
    pub skill: u8,
    /// The new lock state.
    pub lock: SkillLock,
}

impl SkillLockRequest {
    /// The packet id.
    pub const ID: u8 = 0x3A;

    /// Decode the incoming skill-lock request.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        let _length = reader.u16()?;
        // The wire carries the id as a word; every skill id fits a byte.
        let skill = reader.u16()? as u8;
        let lock = SkillLock::from_bits(reader.u8()?);
        Ok(Self { skill, lock })
    }
}

/// `0x12` — a client text command. The engine cares about one type, `0x24`
/// ("use skill"), whose payload is the skill index as an ASCII string. ServUO's
/// `TextCommand` case `0x24` → `Skills.UseSkill`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct UseSkillRequest {
    /// Which skill, zero-based.
    pub skill: u8,
}

impl UseSkillRequest {
    /// The packet id — the text-command envelope.
    pub const ID: u8 = 0x12;
    /// The command type that means "use skill".
    const TYPE_USE_SKILL: u8 = 0x24;

    /// Decode a `0x12`, returning the skill request if that is what it is. Any
    /// other command type reads as `None` rather than an error, so the dispatcher
    /// can pass on the ones it does not handle (an emote, a `go`, an open-book).
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        let _length = reader.u16()?;
        let kind = reader.u8()?;
        if kind != Self::TYPE_USE_SKILL {
            return Ok(None);
        }
        // "N" or "N 0" — the index, maybe with a trailing field the engine
        // ignores. A payload that is not a number is not a use we can act on.
        let command = reader.null_terminated_string()?;
        match command.split(' ').next().unwrap_or("").trim().parse::<u8>() {
            Ok(skill) => Ok(Some(Self { skill })),
            Err(_) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aos() -> ClientVersion {
        ClientVersion::new(4, 0, 0, 0)
    }

    fn pre_aos() -> ClientVersion {
        ClientVersion::new(3, 0, 0, 0)
    }

    #[test]
    fn skill_locks_round_trip_through_the_wire_byte() {
        for lock in [SkillLock::Up, SkillLock::Down, SkillLock::Locked] {
            assert_eq!(SkillLock::from_bits(lock.to_bits()), lock);
        }
        assert_eq!(
            SkillLock::from_bits(99),
            SkillLock::Up,
            "unknown reads as Up"
        );
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
                id: 0, // Alchemy — sent as 1, so the 0 terminator is unambiguous
                value: 755,
                base: 700,
                lock: SkillLock::Locked,
                cap: 1000,
            },
            SkillEntry {
                id: 45, // Mining
                value: 500,
                base: 500,
                lock: SkillLock::Up,
                cap: 1000,
            },
        ];
        let packet = encode_skills_full(&entries, true);
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
            id: 0,
            value: 100,
            base: 100,
            lock: SkillLock::Up,
            cap: 1000,
        }];
        let packet = encode_skills_full(&entries, false);
        assert_eq!(packet[3], 0x00, "the uncapped absolute type");
        assert_eq!(
            packet.len(),
            4 + 7 + 2,
            "type + one 7-byte entry + terminator"
        );
    }

    #[test]
    fn a_single_update_is_zero_based_and_unterminated() {
        let entry = SkillEntry {
            id: 25, // Magery
            value: 501,
            base: 501,
            lock: SkillLock::Up,
            cap: 1000,
        };
        let packet = encode_skill_update(&entry, true);
        assert_eq!(packet[0], 0x3A);
        assert_eq!(packet[3], 0xDF, "the capped delta type");
        assert_eq!(
            u16::from_be_bytes([packet[4], packet[5]]),
            25,
            "zero-based here"
        );
        assert_eq!(packet.len(), 13);
    }

    #[test]
    fn a_lock_request_reads_its_skill_and_lock() {
        // 0x3A, length, skill(u16)=45, lock=1 (down).
        let packet = [0x3A, 0x00, 0x06, 0x00, 0x2D, 0x01];
        let request = SkillLockRequest::decode(&packet).unwrap();
        assert_eq!(request.skill, 45);
        assert_eq!(request.lock, SkillLock::Down);
    }

    #[test]
    fn a_use_skill_command_reads_its_index() {
        // 0x12, length, type 0x24, "45\0".
        let mut packet = vec![0x12u8, 0x00, 0x00, 0x24];
        packet.extend_from_slice(b"45\0");
        let len = packet.len() as u16;
        packet[1..3].copy_from_slice(&len.to_be_bytes());
        let request = UseSkillRequest::decode(&packet).unwrap().unwrap();
        assert_eq!(request.skill, 45, "Mining, zero-based");
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
}
