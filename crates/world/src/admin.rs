//! The staff menu behind `.admin`: a gump only a game master may open, and the
//! handler for the buttons it comes back with.
//!
//! The menu is engine-owned — it is an operator tool, not gameplay. Its buttons
//! register spawn regions (see [`crate::spawner`]) the tick then keeps populated.
//! The spawn *sets* here are scaffolding: a small, curated Britain and cemetery to
//! exercise the machinery end to end. The real, point-faithful sets belong in the
//! community pack, registered through a script op — this is the shape they take.

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_protocol::{encode_gump_display, AccessLevel, GumpResponse};
use openshard_state::components::{Access, Client};
use openshard_state::WorldState;

/// The id the admin gump answers under. High byte `0xAD` for "admin", so a stray
/// `0xB1` for some other dialog never lands in the admin handler by accident.
pub const ADMIN_GUMP: u32 = 0x00AD_0001;

/// Button ids the layout gives its reply buttons. `0` is the client's close box.
const BTN_POPULATE_BRITAIN: u32 = 10;
const BTN_POPULATE_CEMETERY: u32 = 11;
const BTN_CLEAR: u32 = 12;
const BTN_DECORATE_BRITAIN: u32 = 20;
const BTN_CLEAR_DECO: u32 = 21;

/// Open the admin menu for `actor`. The caller has already checked the authority
/// (the `.admin` command is game-master-gated), so this only draws.
pub fn open_menu(state: &mut WorldState, actor: EntityId) {
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(actor) else {
        return;
    };
    // A tabbed window: page 0 is always drawn (the title and the tab buttons that
    // switch pages), and each further page is a tab's contents.
    let layout = "\
{ resizepic 0 0 5054 320 260 }\
{ page 0 }\
{ text 120 12 2100 0 }\
{ button 18 44 4005 4007 0 1 0 }{ text 52 46 1153 1 }\
{ button 130 44 4005 4007 0 2 0 }{ text 164 46 1153 5 }\
{ page 1 }\
{ button 30 92 4005 4007 1 0 10 }{ text 66 94 1153 2 }\
{ button 30 124 4005 4007 1 0 11 }{ text 66 126 1153 3 }\
{ button 30 168 4017 4019 1 0 12 }{ text 66 170 33 4 }\
{ page 2 }\
{ button 30 92 4005 4007 1 0 20 }{ text 66 94 1153 6 }\
{ button 30 136 4017 4019 1 0 21 }{ text 66 138 33 7 }";
    let lines = [
        "Admin".to_owned(),
        "Spawn".to_owned(),
        "Populate Britain".to_owned(),
        "Populate cemetery".to_owned(),
        "Clear spawns".to_owned(),
        "Deco".to_owned(),
        "Decorate Britain".to_owned(),
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
        BTN_POPULATE_BRITAIN => "populate:britain",
        BTN_POPULATE_CEMETERY => "populate:cemetery",
        BTN_CLEAR => "clear",
        BTN_DECORATE_BRITAIN => "decorate:britain",
        BTN_CLEAR_DECO => "clear:deco",
        _ => return None, // the close box, or a button we do not know
    };
    Some((actor, verb))
}
