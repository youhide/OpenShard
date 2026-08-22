//! What the spellbook sends when one of its entries is chosen.
//!
//! The server ultimately owns whether the book still holds the spell and
//! whether it can be cast.  This module only writes the modern spellbook form
//! of `0xBF 0x1C`, so a spell row and a macro share the same server path.

use openshard_protocol::casting::SpellId;
use openshard_protocol::serial::Serial;

/// Ask to cast `spell` from `spellbook`.
///
/// Spell ids are zero-based in the program and one-based on the wire.  The UI
/// can only construct the 64 Magery entries, so adding one is exact here.
#[must_use]
pub fn cast(spellbook: Serial, spell: SpellId) -> Vec<u8> {
    let mut packet = Vec::with_capacity(13);
    packet.push(0xBF);
    packet.extend_from_slice(&13u16.to_be_bytes());
    packet.extend_from_slice(&0x1Cu16.to_be_bytes());
    packet.extend_from_slice(&1u16.to_be_bytes());
    packet.extend_from_slice(&spellbook.raw().to_be_bytes());
    packet.extend_from_slice(&(spell.0 + 1).to_be_bytes());
    packet
}

#[cfg(test)]
mod tests {
    use openshard_protocol::client_packet::ClientPacket;
    use openshard_protocol::extended::ExtendedRequest;
    use openshard_protocol::version::ClientVersion;

    use super::*;

    #[test]
    fn a_spellbook_row_sends_the_one_based_spell_id() {
        let book = Serial::new(0x4000_0001).expect("an item serial");
        let packet = cast(book, SpellId(5));
        assert_eq!(
            ClientPacket::decode(&packet, ClientVersion::new(7, 0, 45, 65)).expect("cast packet"),
            ClientPacket::Extended(ExtendedRequest::Cast(
                openshard_protocol::casting::CastSpellRequest {
                    spell: openshard_protocol::casting::RawSpellId(6),
                }
            ))
        );
    }
}
