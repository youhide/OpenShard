//! The staff menu behind `.admin`: a gump only a game master may open, and the
//! handler for the buttons it comes back with.
//!
//! The menu is engine-owned — it is an operator tool, not gameplay. Its buttons
//! carry a *verb*: "populate" registers the spawn regions (see
//! [`crate::spawner`]) the tick then keeps populated, "decorate" lays the
//! static/door/container art, "regions" gives a facet its named areas, and the
//! three "clear" verbs undo them. One click each lays or clears the world.
//!
//! **Who answers a verb is not this module's business, and more than one may.**
//! The verb is a string on an
//! [`AdminMenuAction`](crate::events::AdminMenuAction); everyone listening turns
//! it into commands, and the world applies all of them. `server::content` answers
//! all three lay verbs from data in the tree. A verb nobody answers lays nothing
//! and says nothing.

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_npc as npc;
use openshard_protocol::gump::admin::{
    CREATURE_CREATE,
    CREATURE_KIND_FIELD,
    ITEM_AMOUNT_FIELD,
    ITEM_CREATE,
    ITEM_CREATE_KIND,
    ITEM_GRAPHIC_FIELD,
    ITEM_HUE_FIELD,
    ITEM_KIND_FIELD,
    ITEM_MATERIAL_FIELD,
    ITEM_STACKABLE,
};
use openshard_protocol::gump::{
    ButtonId,
    CloseGump,
    GumpAnswer,
    GumpButton,
    GumpDisplay,
    GumpId,
    GumpKey,
    GumpLayout,
    GumpPoint,
    GumpResponse,
};
use openshard_protocol::item_kind::{
    ItemKindId,
    MaterialId,
};
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::target::{
    TargetCursor,
    TargetKind,
};
use openshard_protocol::wire::{
    CursorId,
    Graphic,
    Hue,
};
use openshard_protocol::world::{
    Aggression,
    DamageType,
    PhysicalResistance,
    Point,
    Sight,
};
use openshard_state::WorldState;
use openshard_state::components::Client;

/// The id the admin gump answers under. High byte `0xAD` for "admin", so a stray
/// `0xB1` for some other dialog never lands in the admin handler by accident.
pub const ADMIN_GUMP: GumpId = openshard_protocol::gump::id::ADMIN;

/// The form reached through the item row of [`ADMIN_GUMP`].
pub const ADMIN_ITEM_GUMP: GumpId = openshard_protocol::gump::id::ADMIN_ITEM;

/// The request-only form used by F1 to select an animal for target placement.
pub const ADMIN_CREATURE_GUMP: GumpId = openshard_protocol::gump::id::ADMIN_CREATURE;

/// The reply which opens the item form from the main staff menu.
const OPEN_ITEM_CREATOR: ButtonId = ButtonId(40);

/// One row of the menu: a reply button and the verb its id means.
///
/// The table below is the *only* place a button id is written. The layout draws
/// from it and [`button_action`] looks a reply up in it, so the drawn button and
/// the handled one cannot drift apart — which they could while the layout was a
/// string with the ids spelled into it by hand.
struct Row {
    /// What the `0xB1` comes back with. Never [`ButtonId::CLOSE_BOX`] — this menu
    /// offers no button of its own under the client's close box.
    id:     ButtonId,
    /// The button's top edge. The label sits two pixels lower, which is what
    /// makes the two look level.
    y:      i32,
    /// Gump art: unpressed, then pressed.
    art:    (u32, u32),
    /// The label's hue. One for the verbs that lay a facet's worth of world
    /// down, another for the ones that take it away again.
    hue:    u32,
    label:  &'static str,
    action: RowAction,
}

/// What a button in the main menu means.  World-building rows still publish a
/// verb for content packs; the item row is an engine-owned form instead.
#[derive(Clone, Copy)]
enum RowAction {
    Verb(&'static str),
    OpenItemCreator,
}

/// The menu, top to bottom: three verbs that lay the world down, then three that
/// clear them.
const ROWS: [Row; 7] = [
    Row {
        id:     ButtonId(13),
        y:      54,
        art:    (4005, 4007),
        hue:    1153,
        label:  "Populate Felucca",
        action: RowAction::Verb("populate:felucca"),
    },
    Row {
        id:     ButtonId(22),
        y:      88,
        art:    (4005, 4007),
        hue:    1153,
        label:  "Decorate Felucca",
        action: RowAction::Verb("decorate:felucca"),
    },
    Row {
        id:     ButtonId(31),
        y:      122,
        art:    (4005, 4007),
        hue:    1153,
        label:  "Regions: Felucca",
        action: RowAction::Verb("regions:felucca"),
    },
    Row {
        id:     ButtonId(12),
        y:      164,
        art:    (4017, 4019),
        hue:    33,
        label:  "Clear spawns",
        action: RowAction::Verb("clear"),
    },
    Row {
        id:     ButtonId(21),
        y:      198,
        art:    (4017, 4019),
        hue:    33,
        label:  "Clear deco",
        action: RowAction::Verb("clear:deco"),
    },
    Row {
        id:     ButtonId(30),
        y:      232,
        art:    (4017, 4019),
        hue:    33,
        label:  "Clear regions",
        action: RowAction::Verb("clear:regions"),
    },
    Row {
        id:     OPEN_ITEM_CREATOR,
        y:      266,
        art:    (4005, 4007),
        hue:    89,
        label:  "Create item in backpack",
        action: RowAction::OpenItemCreator,
    },
];

/// Draw the menu.
///
/// One flat page: the verbs that lay the whole facet, and the ones that clear
/// them. Nothing to switch between, so there are no tabs to fall out of sync.
fn menu() -> GumpLayout {
    let mut layout = GumpLayout::new();
    layout.background(0, 0, 300, 304, 5054);
    layout.label(105, 14, 2100, "Admin");
    for row in &ROWS {
        layout.button(30, row.y, row.art.0, row.art.1, GumpButton::Reply, 0, row.id);
        layout.label(66, row.y + 2, row.hue, row.label);
    }
    layout
}

/// The item form deliberately asks for graphics rather than maintaining a
/// second, inevitably incomplete, catalogue of every item in the installed
/// client files.  It accepts the same decimal or `0x` hexadecimal identifiers
/// as staff commands do.
fn item_creator() -> GumpLayout {
    let mut layout = GumpLayout::new();
    layout.background(0, 0, 360, 232, 5054);
    layout.label(120, 14, 2100, "Create item");
    layout.label(24, 54, 1153, "Graphic (hex or decimal)");
    layout.text_entry(190, 50, 135, 20, 1153, u32::from(ITEM_GRAPHIC_FIELD), "0x");
    layout.label(24, 88, 1153, "Hue");
    layout.text_entry(190, 84, 135, 20, 1153, u32::from(ITEM_HUE_FIELD), "0");
    layout.label(24, 122, 1153, "Amount");
    layout.text_entry(190, 118, 135, 20, 1153, u32::from(ITEM_AMOUNT_FIELD), "1");
    layout.check(24, 153, 210, 211, true, ITEM_STACKABLE);
    layout.label(54, 155, 1153, "Stack identical items");
    layout.button(116, 188, 4005, 4007, GumpButton::Reply, 0, ITEM_CREATE);
    layout.label(152, 190, 89, "Create in backpack");
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
    // Close then draw, the pattern every other gump follows (see
    // `healer.rs`'s `open_healer_gump`): a client told to draw twice for the
    // same id draws two windows, and a right-click only ever takes the
    // topmost off screen.
    state.send_packet(
        connection,
        &ServerPacket::CloseGump(CloseGump {
            gump_id: ADMIN_GUMP,
            button:  ButtonId::CLOSE_BOX,
        }),
    );
    let packet = ServerPacket::GumpDisplay(GumpDisplay {
        serial,
        gump_id: ADMIN_GUMP,
        at: GumpPoint::new(100, 100),
        layout: layout.to_owned(),
        lines: lines.to_vec(),
    });
    state.send_packet(connection, &packet);
}

/// Open the administrator's generic item form.
pub fn open_item_creator(state: &mut WorldState, actor: EntityId) {
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(actor) else {
        return;
    };
    let gump = item_creator();
    let (layout, lines) = gump.finish();
    let serial = state
        .registry
        .serial_of(actor)
        .map_or(GumpKey::STANDALONE, GumpKey::on);
    state.send_packet(
        connection,
        &ServerPacket::CloseGump(CloseGump {
            gump_id: ADMIN_ITEM_GUMP,
            button:  ButtonId::CLOSE_BOX,
        }),
    );
    state.send_packet(
        connection,
        &ServerPacket::GumpDisplay(GumpDisplay {
            serial,
            gump_id: ADMIN_ITEM_GUMP,
            at: GumpPoint::new(100, 100),
            layout: layout.to_owned(),
            lines: lines.to_vec(),
        }),
    );
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
/// The verb is not validated here, and cannot be: the world publishes the string
/// and does not know who is listening. An unknown one is read by
/// `server::content`, matches nothing there, and lays nothing.
pub fn seed(state: &mut WorldState, action: &str) {
    state.bus.send(crate::events::AdminMenuAction {
        serial: None,
        action: action.to_owned(),
    });
}

/// Interpret a `0xB1` for the admin gump: the acting mobile and the *verb* its
/// button asked for, or `None` if it is not our gump, the close box, or a forgery.
/// The verb is a plain string a listener switches on; `server::content` is what
/// answers it, from `data/*.json` in the domain crates.
///
/// Re-checks the authority here, not only on the `.admin` that opened the gump:
/// the gump id is not a secret, so a non-staff client could send this packet. This
/// only reads, so the gate is safe here.
/// A validated button the admin UI can produce.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonAction {
    /// Send an existing content-pack verb.
    Verb(&'static str),
    /// Show the generic item form.
    OpenItemCreator,
    /// Submit the generic item form.
    CreateItem,
    /// Submit a registered gameplay identity from the F1 catalogue.
    CreateItemKind,
    /// Begin target placement for a server-owned animal preset.
    PlaceCreature,
}

/// A small, safe catalogue for quickly setting up gameplay scenes. The body,
/// stats and behaviour stay on the server; the client may only select one id.
#[derive(Clone, Copy)]
struct CreaturePreset {
    id:     u16,
    body:   Graphic,
    hits:   u16,
    damage: u16,
    fame:   i32,
    karma:  i32,
}

const CREATURES: [CreaturePreset; 10] = [
    CreaturePreset {
        id:     1,
        body:   Graphic(200),
        hits:   37,
        damage: 4,
        fame:   300,
        karma:  300,
    },
    CreaturePreset {
        id:     2,
        body:   Graphic(217),
        hits:   20,
        damage: 6,
        fame:   0,
        karma:  300,
    },
    CreaturePreset {
        id:     3,
        body:   Graphic(201),
        hits:   6,
        damage: 5,
        fame:   0,
        karma:  150,
    },
    CreaturePreset {
        id:     4,
        body:   Graphic(216),
        hits:   18,
        damage: 3,
        fame:   300,
        karma:  0,
    },
    CreaturePreset {
        id:     5,
        body:   Graphic(207),
        hits:   12,
        damage: 2,
        fame:   300,
        karma:  0,
    },
    CreaturePreset {
        id:     6,
        body:   Graphic(208),
        hits:   3,
        damage: 5,
        fame:   150,
        karma:  0,
    },
    CreaturePreset {
        id:     7,
        body:   Graphic(205),
        hits:   5,
        damage: 2,
        fame:   150,
        karma:  0,
    },
    CreaturePreset {
        id:     8,
        body:   Graphic(220),
        hits:   21,
        damage: 4,
        fame:   300,
        karma:  0,
    },
    CreaturePreset {
        id:     9,
        body:   Graphic(25),
        hits:   41,
        damage: 5,
        fame:   450,
        karma:  0,
    },
    CreaturePreset {
        id:     10,
        body:   Graphic(167),
        hits:   53,
        damage: 9,
        fame:   450,
        karma:  0,
    },
];

/// The data one of the two item forms makes into an item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemRequest {
    /// Explicitly unregistered client art for decoration and asset debugging.
    LegacyArt {
        graphic:   Graphic,
        hue:       Hue,
        amount:    u16,
        stackable: bool,
    },
    /// Durable gameplay identity from the shared definition catalogue.
    Kind {
        kind:      ItemKindId,
        material:  Option<MaterialId>,
        amount:    u16,
        stackable: bool,
    },
}

pub fn button_action(
    state: &WorldState,
    connection: ConnectionId,
    response: &GumpResponse,
) -> Option<(EntityId, ButtonAction)> {
    let gump = response
        .gump_id
        .validate(&[ADMIN_GUMP, ADMIN_ITEM_GUMP, ADMIN_CREATURE_GUMP])?;
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
    let action = if gump == ADMIN_GUMP {
        match ROWS.iter().find(|row| row.id == button)?.action {
            RowAction::Verb(verb) => ButtonAction::Verb(verb),
            RowAction::OpenItemCreator => ButtonAction::OpenItemCreator,
        }
    } else if gump == ADMIN_ITEM_GUMP && button == ITEM_CREATE {
        ButtonAction::CreateItem
    } else if gump == ADMIN_ITEM_GUMP && button == ITEM_CREATE_KIND {
        ButtonAction::CreateItemKind
    } else if gump == ADMIN_CREATURE_GUMP && button == CREATURE_CREATE {
        ButtonAction::PlaceCreature
    } else {
        return None;
    };
    Some((actor, action))
}

/// Parse the chosen catalogue entry. Keeping this separate from
/// [`button_action`] makes both the F1 form and a possible classic gump use the
/// same strict allow-list.
pub fn creature_kind(response: &GumpResponse) -> Result<u16, &'static str> {
    let kind = field(response, CREATURE_KIND_FIELD)
        .and_then(|value| parse_u16(value.trim()))
        .filter(|kind| CREATURES.iter().any(|creature| creature.id == *kind))
        .ok_or("That animal is not in the administrator catalogue.")?;
    Ok(kind)
}

/// Raise a location cursor so the administrator can choose the animal's tile.
pub fn begin_creature_placement(state: &mut WorldState, actor: EntityId, kind: u16) {
    if !CREATURES.iter().any(|creature| creature.id == kind) {
        return;
    }
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(actor) else {
        return;
    };
    let serial = state.registry.serial_of(actor).map_or(0, |serial| serial.raw());
    state.raise_target(actor, openshard_state::TargetPurpose::AdminCreature { kind });
    state.send_packet(
        connection,
        &ServerPacket::TargetCursor(TargetCursor {
            cursor_id: CursorId(serial),
            kind:      TargetKind::Location,
        }),
    );
    state.system_message(actor, "Choose where to place the animal.");
}

/// Put the selected animal on a targeted tile. Authority is deliberately
/// checked here as well as on the form: a cursor may outlive staff access.
pub fn place_creature(state: &mut WorldState, actor: EntityId, kind: u16, position: Point) {
    if !state.staff_authority(actor) {
        return;
    }
    let Some(creature) = CREATURES.iter().find(|creature| creature.id == kind) else {
        return;
    };
    let facet = state.facet_of(actor);
    let spawned = npc::spawn(
        state,
        npc::SpawnSpec {
            body: creature.body,
            hue: Hue(0),
            hits: creature.hits,
            notoriety: Notoriety::Innocent,
            damage: creature.damage,
            resistance: PhysicalResistance::new(0),
            swing: 0,
            sight: Sight(10),
            aggression: Aggression::Passive,
            beat: 0,
            ranged: None,
            ranged_kind: DamageType::Physical,
            wander: true,
            position,
            facet,
            name: None,
            title: None,
            shoe: npc::ShoeType::None,
            fame: creature.fame,
            karma: creature.karma,
            night_home: None,
            banker: false,
            vendor: false,
            healer: false,
            equipment: Vec::new(),
            skills: Vec::new(),
        },
    );
    if spawned.is_some() {
        state.system_message(actor, "Animal placed.");
    }
}

/// Validate an item form submission.  Field ids are checked by looking each up
/// explicitly; a forged or duplicate unknown field carries no meaning.
pub fn item_request(response: &GumpResponse) -> Result<ItemRequest, &'static str> {
    let amount = field(response, ITEM_AMOUNT_FIELD)
        .and_then(|value| parse_u16(value.trim()))
        .filter(|amount| *amount > 0)
        .ok_or("Amount must be a whole number from 1 to 65535.")?;
    let stackable = response
        .switches
        .iter()
        .any(|switch| switch.0 == ITEM_STACKABLE.0);
    if response.button.interpret() == GumpAnswer::Pressed(ITEM_CREATE_KIND) {
        let kind = field(response, ITEM_KIND_FIELD)
            .and_then(|value| value.trim().parse::<u32>().ok())
            .and_then(ItemKindId::new)
            .ok_or("Item kind must name a registered positive definition id.")?;
        let material = field(response, ITEM_MATERIAL_FIELD)
            .and_then(|value| parse_u16(value.trim()))
            .ok_or("Material must be zero or a positive definition id.")?;
        let material = if material == 0 {
            None
        } else {
            MaterialId::new(material)
        };
        return Ok(ItemRequest::Kind {
            kind,
            material,
            amount,
            stackable,
        });
    }
    let graphic = field(response, ITEM_GRAPHIC_FIELD)
        .and_then(|value| parse_u16(value.trim()))
        .map(Graphic)
        .ok_or("Graphic must be a decimal or 0x hexadecimal number.")?;
    let hue = field(response, ITEM_HUE_FIELD)
        .and_then(|value| parse_u16(value.trim()))
        .map(Hue)
        .ok_or("Hue must be a decimal or 0x hexadecimal number.")?;
    Ok(ItemRequest::LegacyArt {
        graphic,
        hue,
        amount,
        stackable,
    })
}

fn field(response: &GumpResponse, wanted: u16) -> Option<&str> {
    response
        .text_entries
        .iter()
        .find(|(id, _)| *id == wanted)
        .map(|(_, text)| text.as_str())
}

fn parse_u16(text: &str) -> Option<u16> {
    text.strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .map_or_else(|| text.parse().ok(), |hex| u16::from_str_radix(hex, 16).ok())
}

#[cfg(test)]
mod tests {
    use openshard_protocol::gump::{
        ButtonId,
        GumpAnswer,
        RawButtonId,
    };

    use super::*;

    /// The layout as it was written by hand, before [`menu`] built it from
    /// [`ROWS`]. The client parses this string positionally and says nothing
    /// when it is wrong — it draws an empty window — so the bytes are the only
    /// thing there is to assert, and this is what they were when the menu was
    /// last seen working in a client.
    const HAND_WRITTEN: &str = "\
{ resizepic 0 0 5054 300 304 }\
{ text 105 14 2100 0 }\
{ button 30 54 4005 4007 1 0 13 }{ text 66 56 1153 1 }\
{ button 30 88 4005 4007 1 0 22 }{ text 66 90 1153 2 }\
{ button 30 122 4005 4007 1 0 31 }{ text 66 124 1153 3 }\
{ button 30 164 4017 4019 1 0 12 }{ text 66 166 33 4 }\
{ button 30 198 4017 4019 1 0 21 }{ text 66 200 33 5 }\
{ button 30 232 4017 4019 1 0 30 }{ text 66 234 33 6 }\
{ button 30 266 4005 4007 1 0 40 }{ text 66 268 89 7 }";

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
                "Create item in backpack",
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
                row.label
            );
            let clash = ROWS[..i].iter().find(|other| other.id == row.id);
            assert!(
                clash.is_none(),
                "{} shares id {:?} with {}",
                row.label,
                row.id,
                clash.map_or("", |other| other.label)
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

    #[test]
    fn item_form_accepts_hex_fields_and_the_stack_switch() {
        let response = openshard_protocol::gump::GumpResponse {
            serial:       openshard_protocol::gump::RawGumpKey(0),
            gump_id:      openshard_protocol::gump::RawGumpId(ADMIN_ITEM_GUMP.0),
            button:       openshard_protocol::gump::RawButtonId(ITEM_CREATE.0),
            switches:     vec![openshard_protocol::gump::RawSwitchId(ITEM_STACKABLE.0)],
            text_entries: vec![
                (ITEM_GRAPHIC_FIELD, "0x0eed".to_owned()),
                (ITEM_HUE_FIELD, "0x0481".to_owned()),
                (ITEM_AMOUNT_FIELD, "25".to_owned()),
            ],
        };

        assert_eq!(
            item_request(&response),
            Ok(ItemRequest::LegacyArt {
                graphic:   openshard_protocol::wire::Graphic(0x0eed),
                hue:       openshard_protocol::wire::Hue(0x0481),
                amount:    25,
                stackable: true,
            })
        );
    }

    #[test]
    fn item_form_accepts_a_semantic_identity_without_client_art() {
        let response = openshard_protocol::gump::GumpResponse {
            serial:       openshard_protocol::gump::RawGumpKey(0),
            gump_id:      openshard_protocol::gump::RawGumpId(ADMIN_ITEM_GUMP.0),
            button:       openshard_protocol::gump::RawButtonId(ITEM_CREATE_KIND.0),
            switches:     Vec::new(),
            text_entries: vec![
                (ITEM_KIND_FIELD, "9".to_owned()),
                (ITEM_MATERIAL_FIELD, "1".to_owned()),
                (ITEM_AMOUNT_FIELD, "1".to_owned()),
            ],
        };

        assert_eq!(
            item_request(&response),
            Ok(ItemRequest::Kind {
                kind:      ItemKindId(9),
                material:  Some(MaterialId(1)),
                amount:    1,
                stackable: false,
            })
        );
    }

    #[test]
    fn item_form_rejects_a_zero_amount() {
        let response = openshard_protocol::gump::GumpResponse {
            serial:       openshard_protocol::gump::RawGumpKey(0),
            gump_id:      openshard_protocol::gump::RawGumpId(ADMIN_ITEM_GUMP.0),
            button:       openshard_protocol::gump::RawButtonId(ITEM_CREATE.0),
            switches:     Vec::new(),
            text_entries: vec![
                (ITEM_GRAPHIC_FIELD, "0x0eed".to_owned()),
                (ITEM_HUE_FIELD, "0".to_owned()),
                (ITEM_AMOUNT_FIELD, "0".to_owned()),
            ],
        };

        assert_eq!(
            item_request(&response),
            Err("Amount must be a whole number from 1 to 65535.")
        );
    }
}
