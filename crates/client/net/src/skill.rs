//! What the skill window's own gestures send: the lock arrow and the "use
//! skill" button.

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
}
