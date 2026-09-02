//! Read models that cross the client wire/presentation seam.
//!
//! These types deliberately describe what the client retains, not a packet:
//! connection-only identity and independently refreshed values stay outside.

use openshard_protocol::mobile::{
    MobileStatus,
    Vitals,
};
use openshard_protocol::skill::{
    SkillEntry,
    SkillLock,
};

/// One skill's line, as the shard last stated it — every value is in tenths.
///
/// The id remains the key that files the line: keeping it both there and in
/// this value would admit two disagreeing identities for one skill.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Skill {
    /// What the skill is worth in play: trained, plus what the body's stats lend it.
    pub value: u16,
    /// What is trained, before any of that.
    pub base:  u16,
    /// Which way the shard is training it.
    pub lock:  SkillLock,
    /// This character's own ceiling for it.
    pub cap:   u16,
}

impl From<&SkillEntry> for Skill {
    fn from(entry: &SkillEntry) -> Self {
        Self {
            value: entry.value,
            base:  entry.base,
            lock:  entry.lock,
            cap:   entry.cap,
        }
    }
}

/// The non-positional half of a `0x11` status reply.
///
/// The packet's serial is the connection's own player serial and has already
/// decided where this belongs; carrying it again would make two identities for
/// one status. Hits similarly live beside the player position because `0xA1`
/// can refresh that value between status replies, and **mana** left for the same
/// reason the day `0xA2` arrived: a pool that two packets can state must have one
/// home, or the status window and the bar under the character disagree for as
/// long as it takes the next `0x11` to come. Everything below has no other packet
/// that can state it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Status {
    /// The character name the status window displays.
    pub name:          String,
    /// Whether the character is female.
    pub female:        bool,
    /// Strength.
    pub strength:      u16,
    /// Dexterity.
    pub dexterity:     u16,
    /// Intelligence.
    pub intelligence:  u16,
    /// Stamina, current and maximum.
    pub stamina:       Vitals,
    /// Gold held in the pack.
    pub gold:          u32,
    /// Physical resistance, or armour for the older packet shape.
    pub armor:         u16,
    /// Carried weight.
    pub weight:        u16,
    /// The weight the character can carry before becoming overloaded.
    pub max_weight:    u16,
    /// The combined stat cap.
    pub stat_cap:      u16,
    /// Pets currently following.
    pub followers:     u8,
    /// The greatest number of pets that may follow.
    pub followers_max: u8,
}

impl From<&MobileStatus> for Status {
    fn from(status: &MobileStatus) -> Self {
        Self {
            name:          status.name.clone(),
            female:        status.female,
            strength:      status.strength,
            dexterity:     status.dexterity,
            intelligence:  status.intelligence,
            stamina:       status.stamina,
            gold:          status.gold,
            armor:         status.armor,
            weight:        status.weight,
            max_weight:    status.max_weight,
            stat_cap:      status.stat_cap,
            followers:     status.followers,
            followers_max: status.followers_max,
        }
    }
}
