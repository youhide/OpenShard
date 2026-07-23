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
use openshard_protocol::{encode_gump_display, AccessLevel, GumpResponse};
use openshard_state::components::{Access, Client};
use openshard_state::WorldState;

/// The id the admin gump answers under. High byte `0xAD` for "admin", so a stray
/// `0xB1` for some other dialog never lands in the admin handler by accident.
pub const ADMIN_GUMP: u32 = 0x00AD_0001;

/// Button ids the layout gives its reply buttons. `0` is the client's close box.
const BTN_POPULATE_FELUCCA: u32 = 13;
const BTN_DECORATE_FELUCCA: u32 = 22;
const BTN_CLEAR: u32 = 12;
const BTN_CLEAR_DECO: u32 = 21;

/// Open the admin menu for `actor`. The caller has already checked the authority
/// (the `.admin` command is game-master-gated), so this only draws.
pub fn open_menu(state: &mut WorldState, actor: EntityId) {
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(actor) else {
        return;
    };
    // One flat page: two actions that lay the whole facet, and the two that clear
    // them. Nothing to switch between, so there are no tabs to fall out of sync.
    let layout = "\
{ resizepic 0 0 5054 300 210 }\
{ text 105 14 2100 0 }\
{ button 30 54 4005 4007 1 0 13 }{ text 66 56 1153 1 }\
{ button 30 88 4005 4007 1 0 22 }{ text 66 90 1153 2 }\
{ button 30 130 4017 4019 1 0 12 }{ text 66 132 33 3 }\
{ button 30 164 4017 4019 1 0 21 }{ text 66 166 33 4 }";
    let lines = [
        "Admin".to_owned(),
        "Populate Felucca".to_owned(),
        "Decorate Felucca".to_owned(),
        "Clear spawns".to_owned(),
        "Clear deco".to_owned(),
    ];
    // The context serial is the game master's own — a non-zero value the client
    // keys the open gump on and echoes back. A zero here can leave some clients
    // with no gump to answer for, so no `0xB1` ever comes.
    let serial = state.registry.serial_of(actor).map_or(0, |s| s.raw());
    let packet = encode_gump_display(serial, ADMIN_GUMP, 100, 100, layout, &lines);
    state.send(connection, packet);
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
    if response.gump_id != ADMIN_GUMP {
        return None;
    }
    let &actor = state.players.get(&connection)?;
    let is_staff = state
        .registry
        .get::<Access>(actor)
        .is_some_and(|access| access.0 >= AccessLevel::GameMaster);
    if !is_staff {
        return None;
    }

    let verb = match response.button {
        BTN_POPULATE_FELUCCA => "populate:felucca",
        BTN_DECORATE_FELUCCA => "decorate:felucca",
        BTN_CLEAR => "clear",
        BTN_CLEAR_DECO => "clear:deco",
        _ => return None, // the close box, or a button we do not know
    };
    Some((actor, verb))
}
