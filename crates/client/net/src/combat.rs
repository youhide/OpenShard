//! What this client does about a fight: aim at somebody (`0x05`).
//!
//! [`doll::war_mode`](crate::doll::war_mode) is the other half and lives there
//! rather than here, because it is a *button on the paperdoll* and this is not:
//! the stance is asked for by the frame, and the aim is asked for by a click on
//! a body in the world. Two gestures, two modules, one fight.
//!
//! # Aiming is all this asks for
//!
//! No swing is sent, ever. The client says who it is fighting and the server
//! does the rest — reach, the timer, the skill roll, the damage, the death
//! (`crates/server/combat`'s `attack` aims and `swings` strikes). So there is no
//! "attack now" packet to get wrong and no client-side cooldown to keep in step
//! with the shard's; a client that struck on its own would show blows a server
//! refused.
//!
//! # What the server answers with
//!
//! A `0xAA` naming the mobile that is now the target, or naming nobody when the
//! aim was refused or given up. That answer is what a client draws a highlight
//! from — not this request. The paperdoll's rule (`docs/client/design_windows.md`, decision 8)
//! for the world: **nothing is done locally on the way out.**

use openshard_protocol::combat::AttackRequest;
use openshard_protocol::serial::Serial;

/// Attack the mobile with this serial: the `0x05` to write to the socket.
///
/// A [`Serial`] and not a `RawSerial`, for [`interact::use_object`]'s reason:
/// this end is naming a body it has been shown, out of the
/// [`WorldView`](crate::view::WorldView), rather than repeating a number back.
///
/// [`interact::use_object`]: crate::interact::use_object
#[must_use]
pub fn attack(mobile: Serial) -> Vec<u8> {
    AttackRequest { target: Some(mobile) }.encode()
}

/// Stop attacking: the same `0x05` naming nobody.
///
/// Not a second packet and not a flag — the protocol's own way of saying it, and
/// the reason [`AttackRequest::target`] is an `Option` on both sides of the
/// wire. Nothing in this client sends one yet; it is here because the encoder
/// that can only name somebody is the one that makes "stop" look like a missing
/// feature rather than a call nobody has made.
#[must_use]
pub fn stop_attacking() -> Vec<u8> {
    AttackRequest { target: None }.encode()
}

#[cfg(test)]
mod tests {
    use openshard_protocol::client_packet::ClientPacket;
    use openshard_protocol::version::ClientVersion;

    use super::*;

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    /// The test this module exists for: what this client writes is what the
    /// shard's own dispatch reads, and it reads it as an attack on the body that
    /// was clicked.
    #[test]
    fn a_click_on_a_body_reaches_the_server_as_an_attack_on_it() {
        let rat = Serial::new(0x0000_002A).unwrap();
        let ClientPacket::Attack(heard) =
            ClientPacket::decode(&attack(rat), version()).expect("0x05 decodes")
        else {
            panic!("0x05 decoded as some other packet");
        };
        assert_eq!(heard.target, Some(rat));
    }

    /// And giving the aim up is the same packet naming nobody — not a malformed
    /// one, which is what a shard would see if the sentinel were anything else.
    #[test]
    fn giving_up_the_aim_names_nobody() {
        let ClientPacket::Attack(heard) =
            ClientPacket::decode(&stop_attacking(), version()).expect("0x05 decodes")
        else {
            panic!("0x05 decoded as some other packet");
        };
        assert_eq!(heard.target, None);
    }
}
