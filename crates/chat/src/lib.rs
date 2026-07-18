//! Speech: what a mobile says, and who hears it.
//!
//! The first gameplay system to live in its own crate rather than in
//! `world::tick`. A system here is a plain function over the shared
//! [`WorldState`]: [`say`] and [`speak`] read the speaker's position, draw the
//! words over its head for everyone in earshot, and emit [`MobileSpoke`] for a
//! script to answer. The world's tick calls them; it does not reach inside.
//!
//! What makes this a *crate* and not just a module is the dependency direction:
//! `chat` depends only on the state below it, never on `world` above. The event
//! it owns, [`MobileSpoke`], lives here too — "domain events live with the crate
//! that owns the rule" — and `world` re-exports it for the reader (a script, the
//! journal) that does not know chat by name.

use openshard_entities::{EntityId, Serial};
use openshard_gateway::ConnectionId;
use openshard_protocol::{
    encode_message, encode_unicode_message, DEFAULT_LANGUAGE_TAG, NO_GRAPHIC,
};
use openshard_state::components::{Body, Client, Name, Position};
use openshard_state::sectors::in_range;
use openshard_state::{Gameplay, Outbound, WorldState};

/// A mobile said something.
///
/// The hook chat hangs everything off: a GM command, an NPC that answers its
/// name, a keyword that starts a quest. Combat's decoupling once more — the
/// speaker only says it happened; whoever cares reads the words. Carries an owned
/// `String`, so unlike most events it is not `Copy`; the bus never needed it to
/// be.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MobileSpoke {
    /// The speaker.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// What was said.
    pub text: String,
}

/// The talk mode of a whisper — heard only by those right beside the speaker.
/// Sphere's `TALKMODE_WHISPER`; the client sends it for `;`-prefixed speech.
pub const TALKMODE_WHISPER: u8 = 8;
/// The talk mode of a yell — carried two screens off. Sphere's `TALKMODE_YELL`,
/// the client's `!`-prefixed speech.
pub const TALKMODE_YELL: u8 = 9;
/// A middling font the client renders speech in when the speaker names none.
pub const DEFAULT_FONT: u16 = 3;

/// How far speech in `mode` carries, in tiles. A whisper is heard only right up
/// close, a yell two screens off, everything else across the screen — the
/// operator's three `distance_*` ranges, chosen by the mode byte the client
/// sends.
#[must_use]
pub const fn speech_range(mode: u8, gameplay: &Gameplay) -> u32 {
    match mode {
        TALKMODE_WHISPER => gameplay.distance_whisper,
        TALKMODE_YELL => gameplay.distance_yell,
        _ => gameplay.distance_talk,
    }
}

/// A player says something. The connection names the speaker.
pub fn say(
    state: &mut WorldState,
    connection: ConnectionId,
    mode: u8,
    hue: u16,
    font: u16,
    text: &str,
) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    speak(state, player, mode, hue, font, text);
}

/// Put words over a mobile's head, for everyone in earshot, and say on the bus
/// that it spoke. The shared body of [`say`] and a script's speak command.
pub fn speak(state: &mut WorldState, entity: EntityId, mode: u8, hue: u16, font: u16, text: &str) {
    let Some(serial) = state.registry.serial_of(entity) else {
        return;
    };
    let Some(&Position(pos)) = state.registry.get::<Position>(entity) else {
        return;
    };
    let facet = state.facet_of(entity);
    let graphic = state
        .registry
        .get::<Body>(entity)
        .map_or(NO_GRAPHIC, |b| b.id);
    // Owned before the packet, so the immutable borrow of the name is done by the
    // time the mutable outbox is touched.
    let name = state
        .registry
        .get::<Name>(entity)
        .map_or(String::new(), |n| n.0.clone());
    // Latin-1 speech rides the universally-understood `0x1C`; anything ASCII
    // cannot carry — an accent, a non-Latin script — has to go out as Unicode
    // `0xAE`, and a player who typed it necessarily spoke `0xAD` to begin with.
    let packet = if text.is_ascii() {
        encode_message(serial.raw(), graphic, mode, hue, font, &name, text)
    } else {
        encode_unicode_message(
            serial.raw(),
            graphic,
            mode,
            hue,
            font,
            DEFAULT_LANGUAGE_TAG,
            &name,
            text,
        )
    };

    let range = speech_range(mode, &state.gameplay);
    let sectors = &state.facet_state(facet).sectors;
    let listeners: Vec<EntityId> = sectors
        .nearby(pos, range)
        .filter(|(_, listener_pos)| in_range(pos, *listener_pos, range))
        .map(|(id, _)| id)
        .collect();
    for listener in listeners {
        if let Some(&Client { connection, .. }) = state.registry.get::<Client>(listener) {
            state.outbox.push(Outbound {
                connection,
                packet: packet.clone(),
            });
        }
    }
    state.bus.send(MobileSpoke {
        entity,
        serial,
        text: text.to_owned(),
    });
}
