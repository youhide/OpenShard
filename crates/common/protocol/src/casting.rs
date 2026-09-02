//! Casting from the client — a spellbook or a macro asking to cast a spell.
//!
//! What the *effect* of a spell is — its mana, its reagents, its damage — is not
//! here and not the engine's: a script owns it, Sphere-scriptpack style. This is
//! only the request off the wire, so the server can hand "this player wants to
//! cast spell N" to the script and let the script's spell data do the rest.
//!
//! The modern client (ClassicUO, 7.x) casts from a spellbook through the extended
//! `0xBF` packet, subcommand `0x1C` — the shape read from ServUO's
//! `PacketHandlers.CastSpell`. There is an older text-command form (`0x12`) too;
//! this handles the one a modern client actually sends.

use crate::codec::PacketReader;
use crate::error::DecodeError;

/// `0xBF` subcommand `0x1C` — a spellbook (or macro) asking to cast a spell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CastSpellRequest {
    /// Which spell, exactly as the wire named it. See [`RawSpellId::interpret`].
    pub spell: RawSpellId,
}

impl CastSpellRequest {
    /// The subcommand that means "cast a spell". See
    /// [`ExtendedRequest`](crate::extended::ExtendedRequest), the single place
    /// that reads a `0xBF` envelope and picks a subcommand's body decoder by it.
    pub const SUBCOMMAND: u16 = 0x1C;

    /// Read the body, `reader` already past the id, length and subcommand.
    pub(crate) fn decode_body(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        // ServUO's reading: a flag word, and when it is 1 the spellbook's serial
        // follows, then the one-based spell id. The spellbook is context the
        // engine does not need — only which spell.
        if reader.u16()? == 1 {
            let _spellbook = reader.u32()?;
        }
        let spell = RawSpellId(reader.u16()?);
        Ok(Self { spell })
    }
}

/// A spell id after the wire's one-based numbering is normalized.
///
/// The value is deliberately not limited to the 64 spells the core Magery table
/// currently knows. The protocol names a spell; the table that receives it owns
/// the separate question whether it has an entry. Keeping that distinction is
/// what lets a future spellbook family share the wire type without teaching this
/// dependency-free crate about gameplay data.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SpellId(pub u16);

/// A spell id exactly as a `0xBF 0x1C` carried it: one-based on the wire, so
/// `1` is the first spell and `0` is not a spell at all — no legitimate
/// client ever sends it.
///
/// `decode_body` used to fold this with `saturating_sub(1)` *while decoding*,
/// which made the wire's `0` (garbage) and its `1` (a real spell — Magery's
/// first) indistinguishable once stored: both became zero-based `0`. That is
/// the same shape as `StatLockRequest`'s and `0xAD`'s findings — "wherever a
/// decoder normalises, the raw byte is being destroyed" — so the fold moved
/// out of `decode_body` and into [`interpret`](Self::interpret), which keeps
/// the two apart. See `docs/protocol/evidence/2026-08-31-the-newtype-sweep.md`,
/// "Amendments forced by N7".
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct RawSpellId(pub u16);

impl RawSpellId {
    /// The zero-based [`SpellId`] this names, or `None` for the wire's `0`.
    ///
    /// Total: every `u16` has an answer, so this may run right at the network
    /// seam rather than waiting for a tick system to have the domain in hand
    /// (`docs/protocol/evidence/2026-08-31-the-newtype-sweep.md`'s N4
    /// containers amendment 2 licence for a
    /// packet-level `interpret`). Whether the *number* names a spell in the
    /// table is a different, fallible question — `openshard_magic::info`'s,
    /// at whatever seam already asks it.
    #[inline]
    #[must_use]
    pub const fn interpret(self) -> Option<SpellId> {
        match self.0 {
            0 => None,
            n => Some(SpellId(n - 1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extended::ExtendedRequest;

    /// Build a `0xBF.0x1C` cast packet the way the client does.
    fn cast_packet(spellbook: Option<u32>, one_based_spell: u16) -> Vec<u8> {
        let mut body = vec![0xBFu8, 0, 0]; // id + length patched below
        body.extend_from_slice(&0x1Cu16.to_be_bytes()); // subcommand
        match spellbook {
            Some(serial) => {
                body.extend_from_slice(&1u16.to_be_bytes());
                body.extend_from_slice(&serial.to_be_bytes());
            }
            None => body.extend_from_slice(&0u16.to_be_bytes()),
        }
        body.extend_from_slice(&one_based_spell.to_be_bytes());
        let len = u16::try_from(body.len()).unwrap();
        body[1..3].copy_from_slice(&len.to_be_bytes());
        body
    }

    #[test]
    fn a_spellbook_cast_names_its_spell_one_based_on_the_wire() {
        // The client sends the sixth spell as 6; interpret() is what turns
        // that into 5.
        let packet = cast_packet(Some(0x4000_0001), 6);
        let request = ExtendedRequest::decode(&packet).unwrap();
        assert_eq!(
            request,
            ExtendedRequest::Cast(CastSpellRequest { spell: RawSpellId(6) })
        );
        assert_eq!(
            match request {
                ExtendedRequest::Cast(cast) => cast.spell.interpret(),
                _ => None,
            },
            Some(SpellId(5))
        );
    }

    #[test]
    fn a_macro_cast_carries_no_spellbook() {
        let packet = cast_packet(None, 1);
        let request = ExtendedRequest::decode(&packet).unwrap();
        assert_eq!(
            request,
            ExtendedRequest::Cast(CastSpellRequest { spell: RawSpellId(1) }),
            "the first spell, one-based on the wire"
        );
    }

    #[test]
    fn a_hostile_zero_decodes_cleanly_but_interprets_to_no_spell() {
        // N9's pair, adapted for a total interpretation rather than a
        // Result: the wire's 0 is never a legitimate one-based spell id, and
        // it must not decode to the same thing as the real first spell (the
        // wire's 1) once interpreted — the bug `decode_body`'s old
        // `saturating_sub(1)` had.
        let packet = cast_packet(None, 0);
        let request = ExtendedRequest::decode(&packet).unwrap();
        assert_eq!(
            request,
            ExtendedRequest::Cast(CastSpellRequest { spell: RawSpellId(0) })
        );
        assert_eq!(RawSpellId(0).interpret(), None);
        assert_eq!(
            RawSpellId(1).interpret(),
            Some(SpellId(0)),
            "distinct from the wire's 0"
        );
    }
}
