//! What this client says: speech out (`0xAD`) and a dialog answered (`0xB1`).
//!
//! Two packets and no state, which is why they share a file: both are this end
//! *telling* the server something, neither has a handshake, and everything they
//! decide is a field the protocol leaves to the client — a hue, a font, a
//! language. That choice is the whole content of this module, and it is written
//! down here rather than at each call site so a shard sees one client and not
//! three.
//!
//! # Speech is how a player reaches the shard's commands
//!
//! Staff commands are `.`-prefixed *speech* — Sphere's convention, kept, and
//! what `crates/server/world/src/gm.rs` parses. There is no command packet: a
//! game master types `.admin` into the same box everything else is said in, and
//! the server answers with a gump. So a client with no way to speak has no way
//! to reach any of it, and nothing here treats a leading `.` specially — the
//! gate is the server's, and a client that filtered locally would be deciding
//! what a shard's commands are.

use openshard_protocol::gump::{GumpResponse, RawButtonId, RawGumpId, RawGumpKey, RawSwitchId};
use openshard_protocol::speech::{Font, RawFont, RawTalkMode, TalkMode};
use openshard_protocol::wire::RawHue;

/// The hue a modern client sends for ordinary speech.
///
/// ClassicUO's default, and what the server echoes back to everyone in earshot
/// if it has no rule of its own — a shard that recolours speech does it on the
/// way out, not by refusing what came in.
const SPEECH_HUE: RawHue = RawHue(0x0384);

/// Say something out loud: the `0xAD` to write to the socket.
///
/// Always [`TalkMode::Regular`]. Emote, whisper and yell are the same packet
/// with another mode byte, and they will arrive when there is a UI that can ask
/// for one — a mode nothing can select would be an untested branch.
#[must_use]
pub fn say(text: &str) -> Vec<u8> {
    openshard_protocol::speech::UnicodeTalkRequest {
        mode: RawTalkMode(TalkMode::Regular.to_wire()),
        hue: SPEECH_HUE,
        font: RawFont(Font::DEFAULT.0),
        text: text.to_owned(),
    }
    .encode()
}

/// Answer an open dialog: the `0xB1` to write to the socket.
///
/// Every argument is echoed from the [`OpenGump`](crate::view::OpenGump) being
/// answered, which is why they are all `Raw*`: the server named them, this end
/// only repeats them, and the check that a reply belongs to a window is the
/// server's — see [`GumpResponse`]. `button` of [`RawButtonId`]`(0)` is the
/// close box, and it is a legitimate answer: a dismissed window is an answer the
/// shard is waiting for.
#[must_use]
pub fn answer_gump(
    key: RawGumpKey,
    gump_id: RawGumpId,
    button: RawButtonId,
    switches: Vec<RawSwitchId>,
    text_entries: Vec<(u16, String)>,
) -> Vec<u8> {
    GumpResponse {
        serial: key,
        gump_id,
        button,
        switches,
        text_entries,
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

    /// The test the whole module exists for: what this client writes is what a
    /// shard's own decoder reads, through the same dispatch a real connection
    /// uses. A hue or a font in the wrong width would not fail here — it would
    /// shift the text — so the assertion is on the words.
    #[test]
    fn a_command_typed_here_is_the_command_the_server_hears() {
        let bytes = say(".admin");
        let ClientPacket::UnicodeTalk(heard) = ClientPacket::decode(&bytes, version()).expect("0xAD decodes")
        else {
            panic!("0xAD decoded as some other packet");
        };
        assert_eq!(heard.text, ".admin");
        assert_eq!(heard.mode.interpret(), TalkMode::Regular);
    }

    #[test]
    fn a_pressed_button_reaches_the_server_as_itself() {
        let bytes = answer_gump(
            RawGumpKey(0x2A),
            RawGumpId(0x00AD_0001),
            RawButtonId(13),
            vec![RawSwitchId(1)],
            Vec::new(),
        );
        let ClientPacket::GumpResponse(heard) =
            ClientPacket::decode(&bytes, version()).expect("0xB1 decodes")
        else {
            panic!("0xB1 decoded as some other packet");
        };
        assert_eq!(heard.gump_id, RawGumpId(0x00AD_0001));
        assert_eq!(heard.button, RawButtonId(13));
        assert_eq!(heard.switches, vec![RawSwitchId(1)]);
    }
}
