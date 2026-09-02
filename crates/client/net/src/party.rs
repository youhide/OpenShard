//! What this client asks its party to do (`0xBF` subcommand `0x06`).
//!
//! Five of the seven the protocol defines. The two left out are deliberate:
//! `0x03` (say this to one member) has no UI to name a recipient from yet, and
//! `0x06` (may the party loot my corpse) has no consumer on the shard — see
//! `docs/roadmap/06-gameplay/parties-and-quests.md`. Both are one function each
//! the day something wants
//! them.
//!
//! # The serial on an accept and a decline is not read
//!
//! `0x08` and `0x09` carry the leader the *client* thinks invited it, and the
//! shard ignores it: the invitation it is holding is the record, and trusting
//! the packet would let a client accept an invitation it never had by naming
//! anybody who happens to be inviting. So this end writes the serial the shard
//! told it, and nothing depends on the shard reading it back.

use openshard_protocol::codec::PacketWriter;
use openshard_protocol::party::SUBCOMMAND;
use openshard_protocol::serial::Serial;

/// Start a `0xBF 0x06` and hand back the writer for the rest of the body.
///
/// The length is patched by [`finish`] rather than counted here — a body whose
/// length is written before its contents is one that goes wrong the first time
/// somebody adds a field.
fn open(kind: u8) -> PacketWriter {
    let mut writer = PacketWriter::with_capacity(16);
    writer.u8(0xBF);
    writer.u16(0); // length, patched in `finish`
    writer.u16(SUBCOMMAND);
    writer.u8(kind);
    writer
}

/// Patch the envelope's length and hand back the bytes.
fn finish(writer: PacketWriter) -> Vec<u8> {
    let mut bytes = writer.into_bytes();
    let length = u16::try_from(bytes.len()).expect("a party request outgrew its u16 length");
    bytes[1..3].copy_from_slice(&length.to_be_bytes());
    bytes
}

/// `0x01` — ask the shard to raise a cursor for whoever is to be added.
///
/// Carries nothing: who it lands on comes back as an ordinary target reply, so
/// this end never names anybody.
#[must_use]
pub fn add() -> Vec<u8> {
    finish(open(0x01))
}

/// `0x02` — turn `member` out, or leave by naming yourself.
///
/// One packet and two meanings, which is the wire's own shape rather than this
/// module's: the shard decides which it is by whether the sender leads.
#[must_use]
pub fn remove(member: Serial) -> Vec<u8> {
    let mut writer = open(0x02);
    writer.u32(member.raw());
    finish(writer)
}

/// `0x04` — say this to the whole party.
///
/// Big-endian UTF-16, like `0xAE` speech and unlike a property list's arguments
/// — which is what this is, a line somebody typed.
#[must_use]
pub fn say(text: &str) -> Vec<u8> {
    let mut writer = open(0x04);
    writer.null_terminated_string_utf16(text);
    finish(writer)
}

/// `0x08` — accept the invitation this client is holding.
#[must_use]
pub fn accept() -> Vec<u8> {
    let mut writer = open(0x08);
    // The leader the shard named, echoed. Zero rather than a serial when this
    // client is holding no invitation, which the shard reads as "no leader" and
    // refuses — the same answer it would give to a fabricated one.
    writer.u32(0);
    finish(writer)
}

/// `0x09` — decline it.
#[must_use]
pub fn decline() -> Vec<u8> {
    let mut writer = open(0x09);
    writer.u32(0);
    finish(writer)
}

#[cfg(test)]
mod tests {
    use openshard_protocol::client_packet::ClientPacket;
    use openshard_protocol::extended::ExtendedRequest;
    use openshard_protocol::party::PartyRequest;
    use openshard_protocol::serial::RawSerial;
    use openshard_protocol::version::ClientVersion;

    use super::*;

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    fn request(bytes: &[u8]) -> PartyRequest {
        let ClientPacket::Extended(request) = ClientPacket::decode(bytes, version()).expect("a 0xBF decodes")
        else {
            panic!("0xBF decoded as some other packet");
        };
        match request {
            ExtendedRequest::Party(request) => request,
            other => panic!("decoded as {other:?}"),
        }
    }

    /// The test this module exists for: what this client writes is what the
    /// shard's own dispatch reads, and it reads it as the request that was
    /// meant. The five share a subcommand and differ by one byte, so a wrong
    /// one is not a malformed packet — it is a well-formed packet that does
    /// something else.
    #[test]
    fn each_request_reaches_the_server_as_itself() {
        assert_eq!(request(&add()), PartyRequest::Add);
        assert_eq!(
            request(&remove(Serial::new(0x2A).unwrap())),
            PartyRequest::Remove(RawSerial(0x2A))
        );
        assert_eq!(
            request(&say("regroup")),
            PartyRequest::PublicMessage("regroup".to_owned())
        );
        assert_eq!(request(&accept()), PartyRequest::Accept(RawSerial(0)));
        assert_eq!(request(&decline()), PartyRequest::Decline(RawSerial(0)));
    }

    #[test]
    fn the_envelope_length_is_the_real_one() {
        // Patched after the body, not counted before it. A length written up
        // front is what goes wrong the first time a field is added, and the
        // framer would then cut the next packet in the wrong place.
        for bytes in [add(), remove(Serial::new(0x2A).unwrap()), say("hello")] {
            assert_eq!(bytes[0], 0xBF);
            assert_eq!(u16::from_be_bytes([bytes[1], bytes[2]]) as usize, bytes.len());
        }
    }
}
