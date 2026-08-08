//! The staff menu behind `.admin`: a gump only a game master may open, and the
//! handler for the buttons it comes back with.
//!
//! The menu is engine-owned — it is an operator tool, not gameplay. Its buttons
//! carry a *verb* the community pack acts on: "populate" registers the spawn
//! regions (see [`crate::spawner`]) the tick then keeps populated, "decorate"
//! lays the static/door/container art, and the two "clear" verbs undo them. The
//! engine holds no spawn or decoration data of its own — a whole facet's worth
//! comes from the pack, registered through a script op under the verb a button
//! sends. One click each lays or clears the world.

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_protocol::gump::{
    ButtonId, GumpAnswer, GumpButton, GumpDisplay, GumpId, GumpKey, GumpLayout, GumpPoint, GumpResponse,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_state::WorldState;
use openshard_state::components::Client;

/// The id the admin gump answers under. High byte `0xAD` for "admin", so a stray
/// `0xB1` for some other dialog never lands in the admin handler by accident.
pub const ADMIN_GUMP: GumpId = GumpId(0x00AD_0001);

/// One row of the menu: a reply button and the verb its id means.
///
/// The table below is the *only* place a button id is written. The layout draws
/// from it and [`button_action`] looks a reply up in it, so the drawn button and
/// the handled one cannot drift apart — which they could while the layout was a
/// string with the ids spelled into it by hand.
struct Row {
    /// What the `0xB1` comes back with. Never [`ButtonId::CLOSE_BOX`] — this menu
    /// offers no button of its own under the client's close box.
    id: ButtonId,
    /// The button's top edge. The label sits two pixels lower, which is what
    /// makes the two look level.
    y: i32,
    /// Gump art: unpressed, then pressed.
    art: (u32, u32),
    /// The label's hue. One for the verbs that lay a facet's worth of world
    /// down, another for the ones that take it away again.
    hue: u32,
    label: &'static str,
    /// What the community pack switches on. The engine holds no spawn or
    /// decoration data, so this string is the whole of what a click means.
    verb: &'static str,
}

/// The menu, top to bottom: three verbs that lay the world down, then three that
/// clear them.
const ROWS: [Row; 6] = [
    Row {
        id: ButtonId(13),
        y: 54,
        art: (4005, 4007),
        hue: 1153,
        label: "Populate Felucca",
        verb: "populate:felucca",
    },
    Row {
        id: ButtonId(22),
        y: 88,
        art: (4005, 4007),
        hue: 1153,
        label: "Decorate Felucca",
        verb: "decorate:felucca",
    },
    Row {
        id: ButtonId(31),
        y: 122,
        art: (4005, 4007),
        hue: 1153,
        label: "Regions: Felucca",
        verb: "regions:felucca",
    },
    Row {
        id: ButtonId(12),
        y: 164,
        art: (4017, 4019),
        hue: 33,
        label: "Clear spawns",
        verb: "clear",
    },
    Row {
        id: ButtonId(21),
        y: 198,
        art: (4017, 4019),
        hue: 33,
        label: "Clear deco",
        verb: "clear:deco",
    },
    Row {
        id: ButtonId(30),
        y: 232,
        art: (4017, 4019),
        hue: 33,
        label: "Clear regions",
        verb: "clear:regions",
    },
];

/// Draw the menu.
///
/// One flat page: the verbs that lay the whole facet, and the ones that clear
/// them. Nothing to switch between, so there are no tabs to fall out of sync.
fn menu() -> GumpLayout {
    let mut layout = GumpLayout::new();
    layout.background(0, 0, 300, 270, 5054);
    layout.label(105, 14, 2100, "Admin");
    for row in &ROWS {
        layout.button(30, row.y, row.art.0, row.art.1, GumpButton::Reply, 0, row.id);
        layout.label(66, row.y + 2, row.hue, row.label);
    }
    layout
}

/// Open the admin menu for `actor`. The caller has already checked the authority
/// (the `.admin` command is game-master-gated), so this only draws.
pub fn open_menu(state: &mut WorldState, actor: EntityId) {
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(actor) else {
        return;
    };
    let gump = menu();
    let (layout, lines) = gump.finish();
    // The key is the game master's own serial — a non-zero value the client
    // keys the open gump on and echoes back. `GumpKey::STANDALONE` can leave
    // some clients with no gump to answer for, so no `0xB1` ever comes; a
    // player always has a serial, so this falls back only in principle.
    let serial = state
        .registry
        .serial_of(actor)
        .map_or(GumpKey::STANDALONE, GumpKey::on);
    let packet = ServerPacket::GumpDisplay(GumpDisplay {
        serial,
        gump_id: ADMIN_GUMP,
        at: GumpPoint::new(100, 100),
        layout: layout.to_owned(),
        lines: lines.to_vec(),
    });
    state.send_packet(connection, &packet);
}

/// Send a verb the way a button would, with nobody having pressed it.
///
/// This is what `--seed` is: an operator naming on the command line the same
/// verbs a game master would click, so a shard can lay its world down without a
/// client attached at all. Deliberately *not* gated on the world being empty —
/// the flag is the whole of the intent, and a seeded shard that then decides for
/// itself whether to obey would be worse than one that does what it was told.
/// Laying the same verb twice duplicates what it lays; that is the operator's to
/// know, and [`ROWS`]' clear verbs are how it is undone.
///
/// The verb is not validated here. The engine holds no spawn data, so it cannot
/// know which verbs a pack answers to — an unknown one reaches `onEvent` and
/// falls through it, exactly as a pack that dropped a set would.
pub fn seed(state: &mut WorldState, action: &str) {
    state.bus.send(crate::events::AdminMenuAction {
        serial: None,
        action: action.to_owned(),
    });
}

/// Interpret a `0xB1` for the admin gump: the acting mobile and the *verb* its
/// button asked for, or `None` if it is not our gump, the close box, or a forgery.
/// The verb is a plain string the script pack switches on — the engine holds no
/// spawn data of its own, so a shard's spawns are edited in the pack, not here.
///
/// Re-checks the authority here, not only on the `.admin` that opened the gump:
/// the gump id is not a secret, so a non-staff client could send this packet. This
/// only reads, so the gate is safe here.
pub fn button_action(
    state: &WorldState,
    connection: ConnectionId,
    response: &GumpResponse,
) -> Option<(EntityId, &'static str)> {
    response.gump_id.validate(&[ADMIN_GUMP])?;
    let &actor = state.players.get(&connection)?;
    // Re-checked on the button, not only on open — and on the account's
    // authority, the same gate the `.` commands use, so a game master testing
    // with their staff mode off can still press it.
    if !state.staff_authority(actor) {
        return None;
    }

    let GumpAnswer::Pressed(button) = response.button.interpret() else {
        return None; // the close box
    };
    // `None` for a button this layout never drew.
    let row = ROWS.iter().find(|row| row.id == button)?;
    Some((actor, row.verb))
}

#[cfg(test)]
mod tests {
    use super::{ROWS, menu};
    use openshard_protocol::gump::{ButtonId, GumpAnswer, RawButtonId};

    /// The layout as it was written by hand, before [`menu`] built it from
    /// [`ROWS`]. The client parses this string positionally and says nothing
    /// when it is wrong — it draws an empty window — so the bytes are the only
    /// thing there is to assert, and this is what they were when the menu was
    /// last seen working in a client.
    const HAND_WRITTEN: &str = "\
{ resizepic 0 0 5054 300 270 }\
{ text 105 14 2100 0 }\
{ button 30 54 4005 4007 1 0 13 }{ text 66 56 1153 1 }\
{ button 30 88 4005 4007 1 0 22 }{ text 66 90 1153 2 }\
{ button 30 122 4005 4007 1 0 31 }{ text 66 124 1153 3 }\
{ button 30 164 4017 4019 1 0 12 }{ text 66 166 33 4 }\
{ button 30 198 4017 4019 1 0 21 }{ text 66 200 33 5 }\
{ button 30 232 4017 4019 1 0 30 }{ text 66 234 33 6 }";

    #[test]
    fn builder_draws_the_menu_the_hand_written_string_drew() {
        let gump = menu();
        let (layout, lines) = gump.finish();
        assert_eq!(layout, HAND_WRITTEN);
        // The text table is referred to by index, so its *order* is part of the
        // layout: element `{ text .. 1 }` means whatever line 1 happens to be.
        assert_eq!(
            lines,
            [
                "Admin",
                "Populate Felucca",
                "Decorate Felucca",
                "Regions: Felucca",
                "Clear spawns",
                "Clear deco",
                "Clear regions",
            ]
        );
    }

    /// The one thing the table cannot enforce by construction. Two rows sharing
    /// an id would draw two buttons and answer as one — the earlier row's verb
    /// for both — and nothing would report it.
    #[test]
    fn button_ids_are_distinct_and_none_is_the_close_box() {
        for (i, row) in ROWS.iter().enumerate() {
            assert_ne!(
                row.id,
                ButtonId::CLOSE_BOX,
                "{} answers under the client's close box",
                row.verb
            );
            let clash = ROWS[..i].iter().find(|other| other.id == row.id);
            assert!(
                clash.is_none(),
                "{} shares id {:?} with {}",
                row.verb,
                row.id,
                clash.map_or("", |other| other.verb)
            );
        }
    }

    /// A `0xB1` naming a button this menu never drew is no verb at all — the
    /// gump id is not a secret, so the id in a reply is as much an input as the
    /// rest of the packet.
    #[test]
    fn a_button_the_menu_never_drew_names_no_verb() {
        let GumpAnswer::Pressed(button) = RawButtonId(999).interpret() else {
            panic!("999 is not the close box");
        };
        assert!(ROWS.iter().all(|row| row.id != button));
    }
}
