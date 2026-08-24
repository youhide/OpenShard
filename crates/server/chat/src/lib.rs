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

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_protocol::serial::Serial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::speech::{DEFAULT_LANGUAGE_TAG, Font, TalkMode, UnicodeMessage};
use openshard_protocol::wire::Hue;
use openshard_state::components::{Body, Client, Name, Position};
use openshard_state::{Gameplay, WorldState};

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

/// How far speech in `mode` carries, in tiles. A whisper is heard only right up
/// close, a yell two screens off, everything else across the screen — the
/// operator's three `distance_*` ranges, chosen by the mode the client sent.
///
/// # Two modes have no range at all
///
/// [`Guild`](TalkMode::Guild) and [`Alliance`](TalkMode::Alliance) pick their
/// listeners by membership, not by distance, and are routed before anything here
/// is asked — see `World::say`. They are named anyway, and answered **zero**,
/// because the `_` arm they used to fall into would have given a guild line the
/// ordinary talk range: if that routing ever fails to happen, the failure should
/// be a line nobody hears rather than a private one shouted down the street.
#[must_use]
pub const fn speech_range(mode: TalkMode, gameplay: &Gameplay) -> u32 {
    match mode {
        TalkMode::Whisper => gameplay.distance_whisper,
        TalkMode::Yell => gameplay.distance_yell,
        TalkMode::Guild | TalkMode::Alliance => 0,
        _ => gameplay.distance_talk,
    }
}

/// A player says something. The connection names the speaker.
pub fn say(
    state: &mut WorldState,
    connection: ConnectionId,
    mode: TalkMode,
    hue: Hue,
    font: Font,
    text: &str,
) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    speak(state, player, mode, hue, font, text);
}

/// Put words over a mobile's head, for everyone in earshot, and say on the bus
/// that it spoke. The shared body of [`say`] and a script's speak command.
pub fn speak(state: &mut WorldState, entity: EntityId, mode: TalkMode, hue: Hue, font: Font, text: &str) {
    let Some(serial) = state.registry.serial_of(entity) else {
        return;
    };
    let Some(&Position(pos)) = state.registry.get::<Position>(entity) else {
        return;
    };
    let facet = state.facet_of(entity);
    // Speaking gives you away, and breaks concentration with it — ServUO's
    // `OnSaid` calls `RevealingAction`, whose last line is `DisruptiveAction`. A
    // trance you can talk through is not a trance, and nor is a hiding place.
    state.break_cover(entity);
    let graphic = state.registry.get::<Body>(entity).map(|b| b.id);
    // Owned before the packet, so the immutable borrow of the name is done by the
    // time the mutable outbox is touched.
    let name = state
        .registry
        .get::<Name>(entity)
        .map_or(String::new(), |n| n.0.clone());
    // Everything goes out as Unicode `0xAE`, always. The old `0x1C` renders in
    // the client's antique ASCII font, so choosing the packet by content — ASCII
    // on `0x1C`, accented on `0xAE` — made the same speaker flip fonts mid-breath
    // the moment a word carried an accent. One font, the modern one, for every
    // line; `0xAE` also carries any script `0x1C` cannot. The hue is left as the
    // caller (the client, for a player) chose it.
    let packet = ServerPacket::UnicodeMessage(UnicodeMessage {
        serial: Some(serial),
        graphic,
        mode,
        hue,
        font,
        language: DEFAULT_LANGUAGE_TAG.to_owned(),
        name,
        text: text.to_owned(),
    });

    let range = speech_range(mode, &state.gameplay);
    let sectors = state.facet_state(facet).sectors();
    // The living do not hear the dead — unless they have reached for them. A ghost
    // is drawn only to other ghosts and to staff (`can_see_mobile`, ServUO's
    // `CanSee`), and without a gate here it would be invisible but audible, which
    // reads as a bug in the client and is one here. The gate is the *hearing* one
    // (`can_hear_mobile`), which is the same question plus Spirit Speak: a living
    // mobile that has contacted the netherworld catches the voice without the
    // speaker becoming visible.
    let listeners: Vec<EntityId> = sectors
        .mobiles_near(pos, range)
        .filter(|&(listener, _)| state.can_hear_mobile(listener, entity))
        .map(|(listener, _)| listener)
        .collect();
    for listener in listeners {
        if let Some(&Client { connection, .. }) = state.registry.get::<Client>(listener) {
            state.send_packet(connection, &packet);
        }
    }
    state.bus.send(MobileSpoke {
        entity,
        serial,
        text: text.to_owned(),
    });
}
