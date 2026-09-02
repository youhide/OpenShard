//! What the skill window's own gestures send: the lock arrow and the "use
//! skill" button — and the status window's arrow, which is the same gesture on
//! a different sheet and the reason [`set_stat_lock`] lives beside them rather
//! than in a module of its own.

use openshard_protocol::mobile::{
    RawStat,
    RawStatLock,
    Stat,
    StatLockRequest,
};
use openshard_protocol::skill::{
    SkillLock,
    SkillLockRequest,
    UseSkillRequest,
};
use openshard_protocol::wire::RawSkillId;

/// Ask to set a skill's lock: the `0x3A` to write to the socket.
///
/// Unanswered by design — see [`SkillLockRequest::encode`]'s doc. The window
/// draws the new face on the click itself rather than waiting for a shard
/// that is never going to reply.
#[must_use]
pub fn set_lock(skill: RawSkillId, lock: SkillLock) -> Vec<u8> {
    SkillLockRequest { skill, lock }.encode()
}

/// Ask to use a skill: the `0x12` "use skill" text command to write to the
/// socket.
#[must_use]
pub fn use_skill(skill: RawSkillId) -> Vec<u8> {
    UseSkillRequest { skill }.encode()
}

/// Ask to move one of the status window's three stat arrows: the `0xBF 0x1A`.
///
/// [`set_lock`]'s twin down to the silence — the shard answers neither, and the
/// window turns the arrow over on the click. The two typed values are widened
/// to their raw forms here, at the wire, because that is where a domain becomes
/// a byte: nothing above this line has any business holding a `RawStat`.
#[must_use]
pub fn set_stat_lock(stat: Stat, lock: SkillLock) -> Vec<u8> {
    StatLockRequest {
        stat: RawStat(stat.to_bits()),
        lock: RawStatLock(lock.to_bits()),
    }
    .encode()
}

#[cfg(test)]
mod tests {
    use openshard_protocol::client_packet::ClientPacket;
    use openshard_protocol::version::ClientVersion;

    use super::*;

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    /// `doll.rs`'s `every_button_reaches_the_server_as_the_packet_it_means`
    /// reason: what this crate writes has to be what the server's own
    /// dispatch decodes, through the same `ClientPacket` a real connection is
    /// routed by.
    #[test]
    fn both_gestures_reach_the_server_as_the_requests_they_mean() {
        let heard = |bytes: &[u8]| ClientPacket::decode(bytes, version()).expect("it decodes");

        assert!(matches!(
            heard(&set_lock(RawSkillId(45), SkillLock::Locked)),
            ClientPacket::SkillLock(request)
                if request.skill == RawSkillId(45) && request.lock == SkillLock::Locked
        ));
        assert!(matches!(
            heard(&use_skill(RawSkillId(45))),
            ClientPacket::UseSkill(request) if request.skill == RawSkillId(45)
        ));
    }

    /// The status window's arrow, through the same door: a `0xBF` envelope the
    /// server's own dispatch has to recognise as a stat lock, with the stat and
    /// the arrow the player actually asked for.
    #[test]
    fn a_moved_stat_arrow_reaches_the_server_as_an_extended_request() {
        use openshard_protocol::extended::ExtendedRequest;

        let bytes = set_stat_lock(Stat::Dexterity, SkillLock::Down);
        let ClientPacket::Extended(request) = ClientPacket::decode(&bytes, version()).expect("it decodes")
        else {
            panic!("a stat lock is a 0xBF");
        };
        let ExtendedRequest::StatLock(request) = request else {
            panic!("and its 0x1A subcommand");
        };
        assert_eq!(request.stat.validate(), Ok(Stat::Dexterity));
        assert_eq!(request.lock.interpret(), SkillLock::Down);
    }
}
