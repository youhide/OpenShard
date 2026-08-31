//! The craft window.
//!
//! A port of ServUO's `CraftGump` and `CraftGumpItem` through the typed
//! [`GumpLayout`] builder `protocol` already has — the path `MondainQuestGump`
//! took, and for the same reason: a fifty-element layout is not writable as a
//! hand-built string, and a mistyped keyword renders as an empty window with
//! nothing at all to debug.
//!
//! **ServUO's button encoding is kept verbatim**: `id = 1 + kind + index * 7`,
//! seven kinds and an unbounded index. It looks arbitrary and it is worth
//! copying rather than improving, because the decode on the other side has to
//! agree exactly and a scheme of one's own is a second thing to get wrong for no
//! gain.
//!
//! **The reply is matched against what the server remembers drawing.** The
//! selected category, the chosen material and the tool all live in the
//! [`CraftGumpContext`] the world holds per player — never in the packet — so a
//! client that invents a category index selects nothing, and one that answers a
//! window this side never opened does nothing at all.

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_protocol::craft::{
    CraftCatalogue,
    CraftText,
    CraftWorkbench,
    CraftWorkbenchComponent,
    CraftWorkbenchGroup,
    CraftWorkbenchMaterial,
    CraftWorkbenchPage,
    CraftWorkbenchRecipe,
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
    RawGumpId,
};
use openshard_protocol::item_kind::ItemSelector;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::wire::{
    ClilocId,
    Graphic,
    Hue,
};
use openshard_state::components::{
    Client,
    Drawn,
    Position,
    Tool,
};
use openshard_state::{
    CraftGumpContext,
    CraftGumpPage,
    WorldState,
    kind_from_drawn,
};

use crate::chance::chance;
use crate::defs::system;
use crate::recipe::Recipe;
use crate::system::{
    CraftSystemDef,
    SystemId,
    Text,
};
use crate::{
    craft,
    environment,
};

/// The window's own id. Distinct from the quest log's, so the two claims of a
/// `0xB1` cannot be confused.
pub const CRAFT_GUMP: GumpId = openshard_protocol::gump::id::CRAFT;

/// Where the window sits, ServUO's `base(40, 40)`.
const WINDOW_X: i32 = 40;
/// The other half of it.
const WINDOW_Y: i32 = 40;

/// The label colour every line of both windows is drawn in — ServUO's
/// `LabelColor`.
const LABEL: u32 = 0xFFFF;
/// The hue a bare (non-cliloc) label takes.
const LABEL_HUE: u32 = 0x480;
/// A warm accent for the active category.  It gives the left-hand navigation a
/// stable visual cursor even when a category spans several pages on the right.
const SELECTED_LABEL: u32 = 0x35;

/// Rows to a page, in both lists.  Ten keeps every trade a short click away,
/// while the wider new list gives each row room for its item art and name.
const PER_PAGE: usize = 10;

/// Flat RGB555 colours for client-drawn UI primitives. They deliberately do
/// not reference UO gump art: a table cell should stay a table cell under every
/// art pack.
const RECT_FILL: u16 = 0x1084;
const RECT_STROKE: u16 = 0x35AD;

/// The vertical coordinate of one row, refusing a list too large for the gump
/// coordinate space instead of drawing its tail over the first row.
fn row_y(first: i32, row: usize) -> i32 {
    i32::try_from(row)
        .ok()
        .and_then(|row| row.checked_mul(20))
        .and_then(|offset| first.checked_add(offset))
        .expect("a craft-gump row fits its i32 coordinate space")
}

/// The one-based gump page containing a list position.
fn page_of(position: usize) -> u32 {
    u32::try_from(position / PER_PAGE)
        .ok()
        .and_then(|page| page.checked_add(1))
        .expect("a craft-gump list fits its u32 page space")
}

/// Which operation an encoded craft-window button asks for.
///
/// Kept distinct from [`ButtonIndex`]: the wire formula stores both as `u32`,
/// but exchanging them still produces a valid-looking [`ButtonId`] for a
/// different operation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct ButtonKind(u32);

/// Which category, recipe, material, or miscellaneous command a button names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct ButtonIndex(u32);

impl ButtonIndex {
    /// Turn an in-memory list position into the number the wire formula uses.
    fn from_position(position: usize) -> Self {
        Self(u32::try_from(position).expect("a craft-gump list cannot contain 2^32 rows"))
    }

    /// Narrow a client-supplied index to a category stored in the gump context.
    fn as_group(self) -> Option<u16> {
        u16::try_from(self.0).ok()
    }

    /// Narrow a client-supplied index to a material stored in the gump context.
    fn as_material(self) -> Option<u8> {
        u8::try_from(self.0).ok()
    }
}

/// ServUO's seven button kinds. The three this slice does not serve are still
/// decoded, so an unhandled press is a no-op and never a mis-read of another
/// kind.
mod kind {
    use super::ButtonKind;

    /// A category on the left.
    pub const GROUP: ButtonKind = ButtonKind(0);
    /// Make the item on this row.
    pub const MAKE: ButtonKind = ButtonKind(1);
    /// Show the item's detail page.
    pub const DETAILS: ButtonKind = ButtonKind(2);
    /// A material off the axis.
    pub const RESOURCE: ButtonKind = ButtonKind(5);
    /// Everything else, told apart by its index.
    pub const MISC: ButtonKind = ButtonKind(6);
    /// How many kinds there are — the modulus.
    pub const COUNT: u32 = 7;
}

/// The `MISC` sub-buttons, by index.
mod misc {
    use super::ButtonIndex;

    /// Open the material list.
    pub const RESOURCES: ButtonIndex = ButtonIndex(0);
    /// Re-scan the tool and the facilities around the player.  The window stays
    /// open while a player walks, so this makes its workbench panel an explicit
    /// fresh reading rather than a stale promise from when it was opened.
    pub const REFRESH: ButtonIndex = ButtonIndex(1);
    /// Cancel a craft in flight.
    pub const CANCEL: ButtonIndex = ButtonIndex(11);
}

/// The detail page's buttons, which are plain small numbers rather than encoded.
mod detail {
    use openshard_protocol::gump::ButtonId;

    /// Back to the list. ServUO's `CraftGumpItem` gives this button the close
    /// box's own id, so dismissing the detail page and pressing Back are one
    /// answer and cannot be told apart — see the reply, which keeps that.
    pub const BACK: ButtonId = ButtonId::CLOSE_BOX;
    /// Make it.
    pub const MAKE: ButtonId = ButtonId(1);
}

/// Whether a gump id is this window's.
#[must_use]
pub fn owns(gump_id: RawGumpId) -> bool {
    gump_id.validate(&[CRAFT_GUMP]).is_some()
}

/// ServUO's `GetButtonID`.
const fn button_id(kind: ButtonKind, index: ButtonIndex) -> ButtonId {
    ButtonId(1 + kind.0 + index.0 * kind::COUNT)
}

/// And its inverse: which kind of button, and which row of it.
///
/// Total on a [`ButtonId`], where it used to have to answer `None` for `0` as
/// well: the close box is no longer a button id at all — `RawButtonId::
/// interpret` takes it apart one step earlier — so the `id == 0` guard this
/// function opened with is gone.
const fn decode_button(button: ButtonId) -> (ButtonKind, ButtonIndex) {
    let id = button.0 - 1;
    (ButtonKind(id % kind::COUNT), ButtonIndex(id / kind::COUNT))
}

/// Draw the craft window for a player, and remember what they are looking at.
///
/// `notice` is the line in the window's own message box — the "you failed to
/// create the item" a previous attempt left behind, which is how ServUO reports
/// a craft without a separate message.
pub fn open(state: &mut WorldState, player: EntityId, context: CraftGumpContext) {
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(player) else {
        return;
    };
    let Some(serial) = state.registry.serial_of(player) else {
        return;
    };
    let Some(def) = system(SystemId::new(context.system)) else {
        return;
    };
    let layout = match context.page {
        CraftGumpPage::Catalogue => catalogue(),
        CraftGumpPage::Details(recipe) => {
            match def.recipes.get(usize::from(recipe)) {
                Some(recipe) => details(state, player, def, recipe, &context),
                None => return,
            }
        }
        _ => main(state, player, def, &context),
    };
    let (string, lines) = layout.finish();
    // Close what is already open before drawing: a client told to draw twice
    // draws two windows, and ServUO closes both craft gumps in every branch.
    state.send_packet(
        connection,
        &ServerPacket::CloseGump(CloseGump {
            gump_id: CRAFT_GUMP,
            button:  ButtonId::CLOSE_BOX,
        }),
    );
    if context.page == CraftGumpPage::Catalogue {
        let catalogue = catalogue_data(state, player);
        state.send_packet(connection, &ServerPacket::CraftCatalogue(catalogue));
    } else {
        state.send_packet(
            connection,
            &ServerPacket::CraftWorkbench(workbench_data(state, player, def, &context)),
        );
    }
    state.send_packet(
        connection,
        &ServerPacket::GumpDisplay(GumpDisplay {
            serial:  GumpKey::on(serial),
            gump_id: CRAFT_GUMP,
            at:      GumpPoint::new(WINDOW_X, WINDOW_Y),
            layout:  string.to_owned(),
            lines:   lines.to_vec(),
        }),
    );
    if let Some(row) = state.row_of_mut(player) {
        row.craft_gump = Some(context);
    }
}

/// Open the complete recipe catalogue with no tool selected.
///
/// `tool` keeps its entity shape even in browse mode, so a reply is still tied
/// to a player-owned context rather than acquiring a second, weaker gump
/// context. A player is never a [`Tool`], which makes it an unambiguous
/// read-only sentinel; [`crate::craft::can_craft`] confirms that at its gate.
pub fn open_catalogue(state: &mut WorldState, player: EntityId) {
    open(
        state,
        player,
        CraftGumpContext {
            system:  0,
            tool:    player,
            group:   0,
            sub_res: 0,
            page:    CraftGumpPage::Catalogue,
            notice:  None,
        },
    );
}

/// Shut the window and forget it.
pub fn close(state: &mut WorldState, player: EntityId) {
    if let Some(row) = state.row_of_mut(player) {
        row.craft_gump = None;
    }
    if let Some(&Client { connection, .. }) = state.registry.get::<Client>(player) {
        state.send_packet(
            connection,
            &ServerPacket::CloseGump(CloseGump {
                gump_id: CRAFT_GUMP,
                button:  ButtonId::CLOSE_BOX,
            }),
        );
    }
}

/// Put a line in the window's notice box and redraw it.
fn reopen(state: &mut WorldState, player: EntityId, mut context: CraftGumpContext, notice: Option<ClilocId>) {
    context.notice = notice;
    context.page = CraftGumpPage::Items;
    open(state, player, context);
}

// ---------------------------------------------------------------------------
// The main window

/// The list window: categories left, selections right, notices and buttons below.
fn main(
    state: &WorldState,
    player: EntityId,
    def: &CraftSystemDef,
    context: &CraftGumpContext,
) -> GumpLayout {
    let mut layout = GumpLayout::new();
    layout.page(0);
    // A workbench rather than two anonymous columns: the wider frame leaves
    // room for every recipe's actual art, its name, and the material picker
    // without turning the selection list into a wall of text.
    layout.background(0, 0, 850, 526, 5054);
    layout.image_tiled(10, 10, 830, 24, 2624);
    layout.image_tiled(10, 39, 205, 330, 2624);
    layout.image_tiled(220, 39, 450, 330, 2624);
    layout.image_tiled(10, 374, 660, 72, 2624);
    layout.image_tiled(10, 451, 660, 65, 2624);
    layout.image_tiled(675, 39, 165, 477, 2624);
    layout.alpha_region(10, 10, 830, 506);

    title(&mut layout, def);
    tool_icon(&mut layout, state, context);
    workbench(&mut layout, state, player, def, context);
    layout.html_localized_colored(15, 43, 190, 22, ClilocId(1_044_010), LABEL, false, false); // CATEGORIES
    layout.html_localized_colored(230, 43, 300, 22, ClilocId(1_044_011), LABEL, false, false); // SELECTIONS
    layout.html_localized_colored(15, 456, 150, 22, ClilocId(1_044_012), LABEL, false, false); // NOTICES

    layout.button(490, 479, 4017, 4019, GumpButton::Reply, 0, ButtonId::CLOSE_BOX);
    layout.html_localized_colored(525, 482, 100, 18, ClilocId(1_011_441), LABEL, false, false); // EXIT

    layout.button(
        320,
        479,
        4017,
        4019,
        GumpButton::Reply,
        0,
        button_id(kind::MISC, misc::CANCEL),
    );
    layout.html_localized_colored(355, 482, 130, 18, ClilocId(1_112_698), LABEL, false, false); // CANCEL MAKE

    if let Some(notice) = context.notice {
        layout.html_localized_colored(170, 456, 140, 48, notice, LABEL, false, false);
    }

    // The material row: which metal or wood is selected, and how much of it the
    // player is carrying. A system without an axis draws neither.
    if let Some(axis) = def.sub_res {
        let entry = axis
            .entries
            .get(usize::from(context.sub_res))
            .or_else(|| axis.entries.first());
        layout.button(
            15,
            398,
            4005,
            4007,
            GumpButton::Reply,
            0,
            button_id(kind::MISC, misc::RESOURCES),
        );
        if let Some(entry) = entry {
            layout.html_localized_colored(50, 382, 250, 18, ClilocId(1_044_055), LABEL, false, false); // MATERIALS
            let held = carried_axis(
                state,
                player,
                axis.item_kind,
                entry.material,
                axis.graphic,
                entry.hue,
            );
            label(&mut layout, 50, 401, 250, entry.name, &held.to_string());
            layout.label(50, 421, LABEL_HUE, format!("{held} available"));
        }
    }

    // The categories are drawn on **page zero**, which is what puts them on every
    // page of a paginated selection list. ServUO calls `CreateGroupList` before
    // any `AddPage(1)`, and moving it inside the pagination makes the whole left
    // column vanish the moment a trade's category runs past ten rows — which most
    // of them do.
    groups(&mut layout, def, context.group);
    match context.page {
        CraftGumpPage::Resources => resources(&mut layout, state, player, def),
        _ => {
            items(
                &mut layout,
                def,
                context.group,
                state.registry.has::<Tool>(context.tool),
            )
        }
    }
    layout
}

/// The catalogue's first page: every recipe, independent of a carried tool.
///
/// A trade picker made the catalogue a second path to the old window and hid
/// the actual answer behind two more clicks.  This is deliberately flattened:
/// every shipped recipe gets a row (ten per server page), its own art and a
/// live "ready" reading. A server page is necessary here: a UO gump sends the
/// layouts for all of its client-side pages in a single `u16`-sized packet, and
/// all 492 recipes would exceed that packet before the player saw it.
///
/// A detail button carries the flattened index, which is resolved back to the
/// owning trade on reply.
fn catalogue() -> GumpLayout {
    let mut layout = GumpLayout::new();
    layout.page(0);
    layout.background(0, 0, 720, 410, 5054);
    layout.image_tiled(10, 10, 700, 24, 2624);
    flat_box(&mut layout, 10, 39, 700, 310, RECT_STROKE);
    layout.image_tiled(10, 354, 700, 46, 2624);
    layout.alpha_region(10, 10, 700, 390);
    layout.label(20, 14, LABEL_HUE, "CRAFT CATALOGUE");
    layout.label(
        20,
        42,
        LABEL_HUE,
        "ALL RECIPES · GREEN = SKILL, MATERIALS AND WORKSHOP READY",
    );
    layout.label(55, 57, LABEL_HUE, "COMPONENTS");
    layout.label(210, 57, LABEL_HUE, "RESULT");
    layout.label(270, 57, LABEL_HUE, "RECIPE");
    layout.button(575, 370, 4017, 4019, GumpButton::Reply, 0, ButtonId::CLOSE_BOX);
    layout.html_localized_colored(610, 373, 80, 18, ClilocId(1_011_441), LABEL, false, false); // EXIT
    layout
}

/// The complete catalogue's data, sent once in a compact OpenShard packet.
/// There are deliberately no coordinates here: the client's `ScrollTable`
/// virtualizes rows, clips them and keeps the scroll position locally.
fn catalogue_data(state: &mut WorldState, player: EntityId) -> CraftCatalogue {
    let facilities = environment::around(state, player);
    let backpack = state
        .registry
        .serial_of(player)
        .and_then(|serial| openshard_items::backpack_of(state, serial));
    let (backpack_revision, amounts) = backpack
        .and_then(|backpack| state.craft_stock_amounts(backpack).ok())
        .unwrap_or_else(|| (0, vec![0; openshard_protocol::craft::CRAFT_KEY_COUNT]));
    let request_id = state
        .registry
        .get::<Client>(player)
        .map(|client| client.connection)
        .and_then(|connection| state.connections.get_mut(&connection))
        .map(|connection| {
            connection.craft_catalogue_request = connection.craft_catalogue_request.wrapping_add(1).max(1);
            connection.craft_catalogue_request
        })
        .unwrap_or_default();
    CraftCatalogue {
        gump_id: CRAFT_GUMP,
        request_id,
        catalogue_revision: openshard_protocol::craft::CRAFT_CATALOGUE_REVISION,
        craft_projection_revision: 0,
        backpack_revision,
        has_pack: backpack.is_some(),
        facilities: facility_mask_found(facilities),
        skills: openshard_protocol::craft::CRAFT_SKILL_IDS
            .iter()
            .filter_map(|&id| {
                openshard_state::Skill::from_id(id)
                    .map(|skill| (id, openshard_skills::skill_value(state, player, skill)))
            })
            .collect(),
        amounts,
        rows: Vec::new(),
    }
}

/// The normal, tool-specific craft gump as data rather than a 0xB0 layout.
///
/// This intentionally mirrors the server context already used by the legacy
/// gump.  The client receives no authority to select a recipe: every action is
/// the exact existing reply id and `handle` still checks it against this
/// context before crafting anything.
fn workbench_data(
    state: &WorldState,
    player: EntityId,
    def: &CraftSystemDef,
    context: &CraftGumpContext,
) -> CraftWorkbench {
    let groups = def
        .groups
        .iter()
        .enumerate()
        .map(|(index, name)| {
            CraftWorkbenchGroup {
                button:   button_id(kind::GROUP, ButtonIndex::from_position(index)).0,
                name:     craft_text(*name),
                selected: u16::try_from(index).ok() == Some(context.group),
            }
        })
        .collect();
    let selected_material = def.sub_res.and_then(|axis| {
        axis.entries.get(usize::from(context.sub_res)).map(|entry| {
            CraftWorkbenchMaterial {
                button:    button_id(
                    kind::RESOURCE,
                    ButtonIndex::from_position(usize::from(context.sub_res)),
                )
                .0,
                item_kind: Some(axis.item_kind),
                material:  Some(entry.material),
                graphic:   axis.graphic,
                hue:       entry.hue,
                name:      craft_text(entry.name),
                carried:   carried_axis(
                    state,
                    player,
                    axis.item_kind,
                    entry.material,
                    axis.graphic,
                    entry.hue,
                ),
                selected:  true,
            }
        })
    });
    let tool = state.registry.get::<Tool>(context.tool);
    let tool_uses = tool.map(|tool| tool.uses_left);
    let tool_carried = tool.is_some() && !state.registry.has::<Position>(context.tool);
    let nearby = environment::around(state, player);
    let facilities = facility_mask(def.needs);
    let present_facilities = facility_mask_found(nearby);
    let page = match context.page {
        CraftGumpPage::Resources => {
            CraftWorkbenchPage::Resources {
                materials: def.sub_res.map_or_else(Vec::new, |axis| {
                    axis.entries
                        .iter()
                        .enumerate()
                        .map(|(index, entry)| {
                            CraftWorkbenchMaterial {
                                button:    button_id(kind::RESOURCE, ButtonIndex::from_position(index)).0,
                                item_kind: Some(axis.item_kind),
                                material:  Some(entry.material),
                                graphic:   axis.graphic,
                                hue:       entry.hue,
                                name:      craft_text(entry.name),
                                carried:   carried_axis(
                                    state,
                                    player,
                                    axis.item_kind,
                                    entry.material,
                                    axis.graphic,
                                    entry.hue,
                                ),
                                selected:  index == usize::from(context.sub_res),
                            }
                        })
                        .collect()
                }),
            }
        }
        CraftGumpPage::Details(recipe) => {
            let recipe = def
                .recipes
                .get(usize::from(recipe))
                .expect("craft context only opens detail pages for real recipes");
            let odds = chance(state, player, def, recipe);
            CraftWorkbenchPage::Details {
                recipe:                workbench_recipe(
                    state,
                    player,
                    def,
                    context,
                    recipe,
                    None,
                    state.registry.has::<Tool>(context.tool).then_some(detail::MAKE.0),
                ),
                success_per_mille:     u16::try_from(odds.success).unwrap_or(u16::MAX),
                exceptional_per_mille: recipe
                    .markable
                    .then(|| u16::try_from(odds.exceptional).unwrap_or(u16::MAX)),
            }
        }
        CraftGumpPage::Items | CraftGumpPage::Catalogue => {
            CraftWorkbenchPage::Items {
                recipes: def
                    .recipes
                    .iter()
                    .filter(|recipe| recipe.group == context.group)
                    .enumerate()
                    .map(|(index, recipe)| {
                        workbench_recipe(state, player, def, context, recipe, Some(index), None)
                    })
                    .collect(),
            }
        }
    };
    CraftWorkbench {
        gump_id: CRAFT_GUMP,
        title: craft_text(def.title),
        groups,
        selected_material,
        tool_uses,
        tool_carried,
        required_facilities: facilities,
        present_facilities,
        notice: context.notice.map(CraftText::Cliloc),
        materials_button: def.sub_res.map(|_| button_id(kind::MISC, misc::RESOURCES).0),
        refresh_button: button_id(kind::MISC, misc::REFRESH).0,
        cancel_button: button_id(kind::MISC, misc::CANCEL).0,
        page,
    }
}

fn craft_text(text: Text) -> CraftText {
    match text {
        Text::Cliloc(id) => CraftText::Cliloc(id),
        Text::Str(value) => CraftText::Literal(value.to_owned()),
    }
}

fn workbench_recipe(
    state: &WorldState,
    player: EntityId,
    def: &CraftSystemDef,
    context: &CraftGumpContext,
    recipe: &Recipe,
    group_index: Option<usize>,
    detail_make: Option<u32>,
) -> CraftWorkbenchRecipe {
    let selected = context.sub_res;
    let components = recipe
        .resources
        .iter()
        .map(|resource| {
            let hue = axis_hue(def, resource, selected);
            let axis_identity = resource
                .from_axis
                .then(|| {
                    def.sub_res.and_then(|axis| {
                        axis.entries
                            .get(usize::from(selected))
                            .or_else(|| axis.entries.first())
                            .map(|entry| (axis.item_kind, entry.material))
                    })
                })
                .flatten();
            CraftWorkbenchComponent {
                item_kind: selector_kind(resource.selector)
                    .or_else(|| axis_identity.map(|(kind, _)| kind))
                    .or_else(|| {
                        kind_from_drawn(Drawn {
                            id: resource.graphic,
                            hue,
                        })
                        .map(|(kind, _)| kind)
                    }),
                graphic: resource.graphic,
                hue,
                name: craft_text(axis_name(def, resource, selected).unwrap_or(resource.name)),
                amount: resource.amount,
                carried: Some(match axis_identity {
                    Some((kind, material)) => {
                        carried_axis(state, player, kind, material, resource.graphic, hue)
                    }
                    None => carried(state, player, resource.graphic, hue),
                }),
            }
        })
        .collect();
    let index = group_index.map(ButtonIndex::from_position);
    CraftWorkbenchRecipe {
        make_button: detail_make.or_else(|| {
            state
                .registry
                .has::<Tool>(context.tool)
                .then_some(index)
                .flatten()
                .map(|index| button_id(kind::MAKE, index).0)
        }),
        details_button: index.map(|index| button_id(kind::DETAILS, index).0),
        result: CraftWorkbenchComponent {
            item_kind: recipe.kind,
            graphic:   recipe.graphic,
            hue:       recipe.hue,
            name:      craft_text(recipe.name),
            amount:    recipe.amount,
            carried:   None,
        },
        skills: recipe
            .skills
            .iter()
            .map(|skill| {
                (
                    CraftText::Cliloc(skill_label(skill.skill)),
                    (skill.min - recipe.min_skill_offset).clamp(0, i32::from(u16::MAX)) as u16,
                )
            })
            .collect(),
        components,
        use_all_resources: recipe.use_all_res,
        markable: recipe.markable,
    }
}

/// The exact input kind a migrated recipe declares for presentation.
///
/// Tags deliberately remain `None`: several kinds can satisfy them, and only
/// the recipe evaluator knows which concrete instance will be consumed.
fn selector_kind(selector: Option<ItemSelector>) -> Option<openshard_protocol::item_kind::ItemKindId> {
    match selector? {
        ItemSelector::Exact(kind) | ItemSelector::KindWithMaterial { kind, .. } => Some(kind),
        ItemSelector::Tag(_) => None,
    }
}

fn facility_mask(needs: crate::system::Needs) -> u8 {
    (needs.forge as u8)
        | ((needs.anvil as u8) << 1)
        | ((needs.heat as u8) << 2)
        | ((needs.oven as u8) << 3)
        | ((needs.mill as u8) << 4)
        | ((needs.water as u8) << 5)
}

fn facility_mask_found(found: crate::environment::Facilities) -> u8 {
    (found.forge as u8)
        | ((found.anvil as u8) << 1)
        | ((found.heat as u8) << 2)
        | ((found.oven as u8) << 3)
        | ((found.mill as u8) << 4)
        | ((found.water as u8) << 5)
}

/// One flat egui-style frame: opaque fill with a one-pixel outline.
///
/// It is intentionally built from the reusable `{ rect }` primitive instead
/// of classic gump art.  Those old nine-slice pictures have ornamental corners
/// whose fixed size exceeds a compact item cell and was the source of the
/// earlier off-centre catalogue layout.
fn flat_box(layout: &mut GumpLayout, x: i32, y: i32, width: i32, height: i32, stroke: u16) {
    layout.rect(x, y, width, height, RECT_FILL);
    layout.rect(x, y, width, 1, stroke);
    layout.rect(x, y + height - 1, width, 1, stroke);
    layout.rect(x, y, 1, height, stroke);
    layout.rect(x + width - 1, y, 1, height, stroke);
}

/// Resolve the flattened catalogue index back to the system and per-system
/// recipe index that normal craft detail pages store in their context.
fn catalogue_recipe(index: ButtonIndex) -> Option<(SystemId, u16)> {
    let position = usize::try_from(index.0).ok()?;
    let &(system, recipe) = openshard_protocol::craft::CRAFT_RECIPE_LOCATIONS.get(position)?;
    Some((SystemId::from_index(usize::from(system))?, recipe))
}

/// The window's own heading.
fn title(layout: &mut GumpLayout, def: &CraftSystemDef) {
    title_at(layout, 15, 13, 740, def);
}

/// A craft-system title at a caller-selected point — shared by the workbench
/// header and catalogue cards, so both name a trade with the same localised
/// source.
fn title_at(layout: &mut GumpLayout, x: i32, y: i32, width: i32, def: &CraftSystemDef) {
    match def.title {
        Text::Cliloc(cliloc) => {
            layout.html_localized_colored(x, y, width, 20, cliloc, LABEL, false, false);
        }
        Text::Str(text) => layout.html_colored(x, y, width, 20, text, LABEL, false, false),
    }
}

/// The tool in the header is a small but useful piece of visual context: a
/// player who has several trade windows open can identify the bench at a
/// glance, without spending another label on the same information.
fn tool_icon(layout: &mut GumpLayout, state: &WorldState, context: &CraftGumpContext) {
    if state.registry.has::<Tool>(context.tool) {
        if let Some(tool) = state
            .registry
            .get::<openshard_state::components::Drawn>(context.tool)
        {
            layout.item(790, 8, tool.id, tool.hue);
        }
    }
}

/// The live workbench reading at the edge of the craft list.  It is intentionally
/// derived from the same tool and facility facts [`craft::begin`] checks, so a
/// green line means the player can act on it rather than merely decorating the
/// gump with a guess.
fn workbench(
    layout: &mut GumpLayout,
    state: &WorldState,
    player: EntityId,
    def: &CraftSystemDef,
    context: &CraftGumpContext,
) {
    const X: i32 = 685;
    layout.label(X, 46, LABEL_HUE, "WORKBENCH");
    layout.label(X, 72, LABEL_HUE, "TOOL");
    if let Some(tool) = state.registry.get::<Tool>(context.tool) {
        layout.label(X, 92, LABEL_HUE, format!("Uses: {}", tool.uses_left));
        let held = !state.registry.has::<Position>(context.tool);
        status_line(layout, X, 112, "Carried", held, true);
    } else {
        status_line(layout, X, 92, "Tool", false, true);
    }

    layout.label(X, 145, LABEL_HUE, "NEARBY");
    let found = environment::around(state, player);
    facility_line(layout, X, 165, "Forge", found.forge, def.needs.forge);
    facility_line(layout, X, 185, "Anvil", found.anvil, def.needs.anvil);
    facility_line(layout, X, 205, "Fire", found.heat, def.needs.heat);
    facility_line(layout, X, 225, "Oven", found.oven, def.needs.oven);
    facility_line(layout, X, 245, "Mill", found.mill, def.needs.mill);
    facility_line(layout, X, 265, "Water", found.water, def.needs.water);

    layout.label(X, 302, LABEL_HUE, "WORKSPACE");
    let usable_tool = state
        .registry
        .get::<Tool>(context.tool)
        .is_some_and(|tool| tool.uses_left > 0 && !state.registry.has::<Position>(context.tool));
    status_line(
        layout,
        X,
        322,
        "Ready to craft",
        usable_tool && found.satisfy(def.needs),
        true,
    );
    layout.label(X, 350, LABEL_HUE, "Refresh after moving");
    layout.button(
        X,
        475,
        4005,
        4007,
        GumpButton::Reply,
        0,
        button_id(kind::MISC, misc::REFRESH),
    );
    layout.label(X + 35, 478, LABEL_HUE, "REFRESH");
}

/// One facility readout.  Optional fixtures remain visible but subdued; a
/// required missing fixture is a red `MISSING`, the exact reason a craft would
/// be refused.
fn facility_line(layout: &mut GumpLayout, x: i32, y: i32, name: &str, found: bool, required: bool) {
    if !required && !found {
        layout.label(x, y, LABEL_HUE, format!("{name}: --"));
    } else {
        status_line(layout, x, y, name, found, required);
    }
}

/// A status label whose colour carries success, failure, or an optional detail.
fn status_line(layout: &mut GumpLayout, x: i32, y: i32, name: &str, available: bool, required: bool) {
    let (state, hue) = if available {
        ("READY", 0x59)
    } else if required {
        ("MISSING", 0x21)
    } else {
        ("--", LABEL_HUE)
    };
    layout.label(x, y, LABEL_HUE, name);
    layout.label(x + 80, y, hue, state);
}

/// One line that may be a cliloc with an argument or a bare string.
fn label(layout: &mut GumpLayout, x: i32, y: i32, width: i32, text: Text, argument: &str) {
    label_colored(layout, x, y, width, text, argument, LABEL);
}

/// One line in a caller-selected colour.  The active craft category uses this
/// instead of a second background gump, so the highlight survives every client
/// skin and leaves the row's click target unchanged.
fn label_colored(
    layout: &mut GumpLayout,
    x: i32,
    y: i32,
    width: i32,
    text: Text,
    argument: &str,
    color: u32,
) {
    match text {
        Text::Cliloc(cliloc) if argument.is_empty() => {
            layout.html_localized_colored(x, y, width, 18, cliloc, color, false, false);
        }
        Text::Cliloc(cliloc) => {
            layout.html_localized_args(x, y, width, 18, cliloc, argument, color, false, false);
        }
        Text::Str(line) => layout.label(x, y - 3, if color == LABEL { LABEL_HUE } else { color }, line),
    }
}

/// One compact table-cell label.  Unlike a bare `{ text }`, its string branch
/// has a real right edge: the client's fixed-size form font can never paint a
/// long recipe name over the detail button in the next column.
fn table_label(layout: &mut GumpLayout, x: i32, y: i32, width: i32, text: Text, argument: &str) {
    match text {
        Text::Cliloc(cliloc) if argument.is_empty() => {
            layout.html_localized_colored(x, y, width, 18, cliloc, LABEL, false, false);
        }
        Text::Cliloc(cliloc) => {
            layout.html_localized_args(x, y, width, 18, cliloc, argument, LABEL, false, false);
        }
        Text::Str(line) => layout.cropped_label(x, y - 3, width, 18, LABEL_HUE, line),
    }
}

/// The left-hand column of categories.
fn groups(layout: &mut GumpLayout, def: &CraftSystemDef, selected: u16) {
    for (i, group) in def.groups.iter().enumerate() {
        let y = row_y(75, i);
        let index = ButtonIndex::from_position(i);
        layout.button(
            15,
            y,
            4005,
            4007,
            GumpButton::Reply,
            0,
            button_id(kind::GROUP, index),
        );
        if u16::try_from(i).ok() == Some(selected) {
            label_colored(layout, 50, y + 3, 150, *group, "", SELECTED_LABEL);
        } else {
            label(layout, 50, y + 3, 150, *group, "");
        }
    }
}

/// The recipes of one category, ten to a page.
///
/// The index a button carries is the row's place **within the group**, which is
/// what ServUO's `CreateItemList` sends and what [`recipe_in_group`] turns back
/// into a recipe.
fn items(layout: &mut GumpLayout, def: &CraftSystemDef, group: u16, can_make: bool) {
    let rows: Vec<(usize, &Recipe)> = def
        .recipes
        .iter()
        .enumerate()
        .filter(|(_, recipe)| recipe.group == group)
        .collect();
    for (i, (_, recipe)) in rows.iter().enumerate() {
        let row = i % PER_PAGE;
        let page = page_of(i);
        if row == 0 {
            if i > 0 {
                layout.button(515, 340, 4005, 4007, GumpButton::Page, page, ButtonId::UNUSED);
                layout.html_localized_colored(550, 343, 100, 18, ClilocId(1_044_045), LABEL, false, false);
                // NEXT PAGE
            }
            layout.page(page);
            if i > 0 {
                layout.button(230, 340, 4014, 4015, GumpButton::Page, page - 1, ButtonId::UNUSED);
                layout.html_localized_colored(265, 343, 100, 18, ClilocId(1_044_044), LABEL, false, false);
                // PREV PAGE
            }
        }
        let y = row_y(70, row);
        let index = ButtonIndex::from_position(i);
        if can_make {
            layout.button(
                230,
                y,
                4005,
                4007,
                GumpButton::Reply,
                0,
                button_id(kind::MAKE, index),
            );
        } else {
            layout.label(230, y + 3, LABEL_HUE, "VIEW");
        }
        // A cell rather than a naked `tilepic`: a recipe's art may be a tiny
        // ingot or a wide piece of furniture, and neither should shove the
        // name column sideways or spill into the next row.
        layout.fitted_item(270, y + 1, 28, 18, recipe.graphic, recipe.hue);
        table_label(layout, 304, y + 3, 276, recipe.name, "");
        layout.button(
            625,
            y,
            4011,
            4012,
            GumpButton::Reply,
            0,
            button_id(kind::DETAILS, index),
        );
    }
}

/// The material list, in place of the recipe list.
fn resources(layout: &mut GumpLayout, state: &WorldState, player: EntityId, def: &CraftSystemDef) {
    let Some(axis) = def.sub_res else { return };
    for (i, entry) in axis.entries.iter().enumerate() {
        let row = i % PER_PAGE;
        let page = page_of(i);
        if row == 0 {
            if i > 0 {
                layout.button(635, 340, 4005, 4007, GumpButton::Page, page, ButtonId::UNUSED);
            }
            layout.page(page);
            if i > 0 {
                layout.button(605, 340, 4014, 4015, GumpButton::Page, page - 1, ButtonId::UNUSED);
            }
        }
        let y = row_y(70, row);
        let index = ButtonIndex::from_position(i);
        layout.button(
            230,
            y,
            4005,
            4007,
            GumpButton::Reply,
            0,
            button_id(kind::RESOURCE, index),
        );
        let held = carried_axis(
            state,
            player,
            axis.item_kind,
            entry.material,
            axis.graphic,
            entry.hue,
        );
        layout.fitted_item(270, y + 1, 28, 18, axis.graphic, entry.hue);
        table_label(layout, 304, y + 3, 226, entry.name, &held.to_string());
        layout.label(555, y + 3, LABEL_HUE, held.to_string());
    }
}

// ---------------------------------------------------------------------------
// The detail page

/// One recipe's page: what it makes, what it wants, and what the odds are.
///
/// The two percentages are the only place a player can read what the chance
/// curve is doing, which is why they are drawn from the same [`chance`] the roll
/// uses rather than from an approximation of it.
fn details(
    state: &WorldState,
    player: EntityId,
    def: &CraftSystemDef,
    recipe: &Recipe,
    context: &CraftGumpContext,
) -> GumpLayout {
    let mut layout = GumpLayout::new();
    layout.page(0);
    layout.background(0, 0, 680, 500, 5054);
    layout.image_tiled(10, 10, 660, 24, 2624);
    layout.image_tiled(10, 39, 220, 385, 2624);
    layout.image_tiled(235, 39, 435, 168, 2624);
    layout.image_tiled(235, 212, 435, 212, 2624);
    layout.image_tiled(10, 429, 660, 61, 2624);
    layout.alpha_region(10, 10, 660, 480);

    title(&mut layout, def);
    layout.html_localized_colored(245, 43, 150, 20, ClilocId(1_044_053), LABEL, false, false); // ITEM
    layout.html_localized_colored(245, 216, 150, 22, ClilocId(1_044_055), LABEL, false, false); // MATERIALS
    layout.html_localized_colored(245, 322, 150, 22, ClilocId(1_044_056), LABEL, false, false); // OTHER

    if state.registry.has::<Tool>(context.tool) {
        layout.button(490, 452, 4005, 4007, GumpButton::Reply, 0, detail::MAKE);
        layout.html_localized_colored(525, 455, 120, 18, ClilocId(1_044_151), LABEL, false, false); // MAKE NOW
    } else {
        layout.label(470, 455, LABEL_HUE, "VIEW MODE — TOOL REQUIRED");
    }
    layout.button(20, 452, 4014, 4016, GumpButton::Reply, 0, detail::BACK);
    layout.html_localized_colored(55, 455, 120, 18, ClilocId(1_044_150), LABEL, false, false); // BACK

    label(&mut layout, 405, 43, 240, recipe.name, "");
    layout.item(95, 145, recipe.graphic, recipe.hue);

    let mut other = 0;
    if recipe.use_all_res {
        layout.html_localized_colored(405, 322, 240, 18, ClilocId(1_048_176), LABEL, false, false); // makes as many as possible
        other += 1;
    }
    if recipe.markable {
        layout.html_localized_colored(
            405,
            322 + other * 20,
            240,
            18,
            ClilocId(1_044_059),
            LABEL,
            false,
            false,
        ); // may hold a maker's mark
    }

    // One row per required skill, at the value it starts to be possible.
    for (i, want) in recipe.skills.iter().enumerate() {
        let y = row_y(126, i);
        layout.html_localized_colored(245, y, 220, 18, skill_label(want.skill), LABEL, false, false);
        layout.label(585, y, LABEL_HUE, tenths(want.min.max(0)));
    }

    let odds = chance(state, player, def, recipe);
    layout.html_localized_colored(245, 76, 280, 18, ClilocId(1_044_057), LABEL, false, false); // Success Chance:
    layout.label(585, 76, LABEL_HUE, percent(odds.success));
    if recipe.markable {
        layout.html_localized_colored(245, 100, 280, 18, ClilocId(1_044_058), LABEL, false, false); // Exceptional Chance:
        layout.label(585, 100, LABEL_HUE, percent(odds.exceptional));
    }

    // Four material rows at most, which is ServUO's own limit and more than any
    // recipe in the shipped tables uses.
    for (i, res) in recipe.resources.iter().take(4).enumerate() {
        let y = row_y(248, i);
        let hue = axis_hue(def, res, 0);
        let name = axis_name(def, res, 0).unwrap_or(res.name);
        layout.item(275, y - 3, res.graphic, hue);
        label(&mut layout, 315, y, 220, name, "");
        layout.label(585, y, LABEL_HUE, res.amount.to_string());
    }
    layout
}

/// A skill's name, as the client's own list numbers them — ServUO's
/// `AosSkillBonuses.GetLabel`, whose three exceptions are skills no craft wants.
fn skill_label(skill: openshard_state::Skill) -> ClilocId {
    ClilocId(1_044_060 + u32::from(skill.id()))
}

/// `620` tenths as `"62.0"`.
fn tenths(value: i32) -> String {
    format!("{}.{}", value / 10, (value % 10).abs())
}

/// Per-mille as the percentage the client shows.
fn percent(value: u32) -> String {
    format!("{}.{}%", value / 10, value % 10)
}

/// The hue a material line is really taken at, once the axis has been applied.
fn axis_hue(def: &CraftSystemDef, res: &crate::recipe::CraftRes, sub_res: u8) -> Hue {
    if !res.from_axis {
        return res.hue;
    }
    def.sub_res
        .and_then(|axis| axis.entries.get(usize::from(sub_res)).map(|e| e.hue))
        .unwrap_or(res.hue)
}

/// And the name that goes with it.
fn axis_name(def: &CraftSystemDef, res: &crate::recipe::CraftRes, sub_res: u8) -> Option<Text> {
    if !res.from_axis {
        return None;
    }
    def.sub_res
        .and_then(|axis| axis.entries.get(usize::from(sub_res)).map(|e| e.name))
}

/// How many of a material a player is carrying.
fn carried(state: &WorldState, player: EntityId, graphic: Graphic, hue: Hue) -> u32 {
    state.registry.serial_of(player).map_or(0, |serial| {
        let drawn = Drawn { id: graphic, hue };
        match kind_from_drawn(drawn) {
            Some((kind, material)) => {
                openshard_items::carried_amount_of_identity_or_legacy(state, serial, kind, material, drawn)
            }
            None => openshard_items::carried_amount_of_hue(state, serial, graphic, Some(hue)),
        }
    })
}

/// Live count for a material-axis row whose identity is declared in recipe data.
fn carried_axis(
    state: &WorldState,
    player: EntityId,
    kind: openshard_protocol::item_kind::ItemKindId,
    material: openshard_protocol::item_kind::MaterialId,
    graphic: Graphic,
    hue: Hue,
) -> u32 {
    state.registry.serial_of(player).map_or(0, |serial| {
        openshard_items::carried_amount_of_identity_or_legacy(
            state,
            serial,
            kind,
            Some(material),
            Drawn { id: graphic, hue },
        )
    })
}

// ---------------------------------------------------------------------------
// The reply

/// Answer a `0xB1`. Returns whether this window claimed it.
///
/// The context is **taken** rather than borrowed, so a reply to a window the
/// server does not remember drawing does nothing at all — the shape
/// `quests::reply` set, and the reason a replayed or invented packet cannot
/// craft anything.
pub fn handle(state: &mut WorldState, connection: ConnectionId, response: &GumpResponse) -> bool {
    if !owns(response.gump_id) {
        return false;
    }
    let answer = response.button.interpret();
    let Some(&player) = state.players.get(&connection) else {
        return true;
    };
    let Some(context) = state.row_of_mut(player).and_then(|row| row.craft_gump.take()) else {
        return true;
    };
    let Some(def) = system(SystemId::new(context.system)) else {
        return true;
    };

    if context.page == CraftGumpPage::Catalogue {
        let GumpAnswer::Pressed(pressed) = answer else {
            return true;
        };
        let (kind, index) = decode_button(pressed);
        if kind == kind::DETAILS {
            let Some((system, recipe)) = catalogue_recipe(index) else {
                return true;
            };
            let mut next = context;
            next.system = system.raw();
            next.page = CraftGumpPage::Details(recipe);
            open(state, player, next);
        }
        return true;
    }

    if let CraftGumpPage::Details(recipe) = context.page {
        handle_details(state, player, context, recipe, answer);
        return true;
    }

    let GumpAnswer::Pressed(pressed) = answer else {
        return true; // EXIT, or the close box — the same id on this page
    };
    let (kind, index) = decode_button(pressed);
    handle_list_button(state, player, context, def, kind, index);
    true
}

/// Dispatch the detail page, whose small plain button ids are intentionally
/// different from the encoded ids used by every list page.
fn handle_details(
    state: &mut WorldState,
    player: EntityId,
    context: CraftGumpContext,
    recipe: u16,
    answer: GumpAnswer,
) {
    match answer {
        GumpAnswer::Pressed(detail::MAKE) => make(state, player, context, recipe),
        // `detail::BACK` *is* the close box (see the constant), so this arm is
        // both, exactly as ServUO's `OnResponse` reads it.
        GumpAnswer::Closed => {
            let mut back = context;
            // A no-tool context belongs to the flattened catalogue.  Its
            // `group` is the catalogue page number, so Back returns to the
            // same slice rather than pretending this recipe had one trade-only
            // parent in the UI.
            back.page = if state.registry.has::<Tool>(context.tool) {
                CraftGumpPage::Items
            } else {
                CraftGumpPage::Catalogue
            };
            open(state, player, back);
        }
        GumpAnswer::Pressed(_) => {}
    }
}

/// Apply one decoded list-page command to the context taken by [`handle`].
fn handle_list_button(
    state: &mut WorldState,
    player: EntityId,
    context: CraftGumpContext,
    def: &CraftSystemDef,
    kind: ButtonKind,
    index: ButtonIndex,
) {
    match kind {
        kind::GROUP => {
            // An invented large index selects nothing. Falling back to zero
            // here would silently turn it into the first real category.
            let Some(group) = index.as_group() else {
                return;
            };
            let mut next = context;
            next.group = group;
            next.page = CraftGumpPage::Items;
            next.notice = None;
            open(state, player, next);
        }
        kind::MAKE => {
            if let Some(recipe) = recipe_in_group(def, context.group, index) {
                make(state, player, context, recipe);
            }
        }
        kind::DETAILS => {
            if let Some(recipe) = recipe_in_group(def, context.group, index) {
                let mut next = context;
                next.page = CraftGumpPage::Details(recipe);
                open(state, player, next);
            }
        }
        kind::RESOURCE => {
            let mut next = context;
            // Refused rather than clamped: an index off the end is a client
            // inventing one, and quietly selecting iron instead would let it
            // craft with a material it never picked.
            if let Some(axis) = def.sub_res {
                if let Some(material) = index
                    .as_material()
                    .filter(|material| usize::from(*material) < axis.entries.len())
                {
                    next.sub_res = material;
                }
            }
            next.page = CraftGumpPage::Items;
            open(state, player, next);
        }
        kind::MISC => {
            match index {
                misc::RESOURCES => {
                    let mut next = context;
                    next.page = CraftGumpPage::Resources;
                    open(state, player, next);
                }
                misc::REFRESH => open(state, player, context),
                misc::CANCEL => {
                    state
                        .registry
                        .remove::<openshard_state::components::Crafting>(player);
                    let mut next = context;
                    next.page = CraftGumpPage::Items;
                    open(state, player, next);
                }
                _ => {}
            }
        }
        _ => {}
    }
}

/// The `index`-th recipe of a category, as the window numbered it.
fn recipe_in_group(def: &CraftSystemDef, group: u16, index: ButtonIndex) -> Option<u16> {
    def.recipes
        .iter()
        .enumerate()
        .filter(|(_, recipe)| recipe.group == group)
        .nth(usize::try_from(index.0).ok()?)
        .and_then(|(at, _)| u16::try_from(at).ok())
}

/// Start the craft the player asked for, and put the window back up.
///
/// ServUO redraws the gump on every branch of a craft, which is what makes a run
/// of items one click each rather than a re-open per attempt. The window comes
/// back showing whatever the attempt had to say.
fn make(state: &mut WorldState, player: EntityId, context: CraftGumpContext, recipe: u16) {
    let started = craft::begin(
        state,
        player,
        context.tool,
        SystemId::new(context.system),
        recipe,
        context.sub_res,
    );
    let notice = if started { None } else { context.notice };
    reopen(state, player, context, notice);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_button_encoding_round_trips() {
        // ServUO's `1 + type + index * 7`, and the decode on the other side has
        // to agree exactly — a scheme of one's own would be a second thing to get
        // wrong for no gain.
        for raw_kind in 0..kind::COUNT {
            for raw_index in 0..50 {
                let kind = ButtonKind(raw_kind);
                let index = ButtonIndex(raw_index);
                let id = button_id(kind, index);
                assert_eq!(decode_button(id), (kind, index));
            }
        }
    }

    #[test]
    fn the_exit_button_is_not_a_button_id_at_all() {
        // Button 0 is EXIT and the window's close box alike, and it must never
        // read as a category — index 0 of kind 0 is button *1*. The guard that
        // used to live in `decode_button` is now one step earlier and applies
        // to every window: `RawButtonId::interpret`.
        assert_eq!(
            openshard_protocol::gump::RawButtonId(0).interpret(),
            GumpAnswer::Closed
        );
        assert_eq!(
            decode_button(button_id(kind::GROUP, ButtonIndex(0))),
            (kind::GROUP, ButtonIndex(0))
        );
    }

    #[test]
    fn an_oversized_category_does_not_fall_back_to_the_first_one() {
        let forged = ButtonIndex(u32::from(u16::MAX) + 1);

        assert_eq!(forged.as_group(), None);
    }

    #[test]
    fn the_numbers_the_window_prints_are_the_numbers_underneath() {
        assert_eq!(tenths(620), "62.0");
        assert_eq!(tenths(0), "0.0");
        assert_eq!(tenths(995), "99.5");
        assert_eq!(percent(1000), "100.0%");
        assert_eq!(percent(0), "0.0%");
        assert_eq!(percent(455), "45.5%");
    }

    #[test]
    fn rows_and_pages_keep_their_positions() {
        assert_eq!(row_y(60, 0), 60);
        assert_eq!(row_y(60, 9), 240);
        assert_eq!(page_of(0), 1);
        assert_eq!(page_of(9), 1);
        assert_eq!(page_of(10), 2);
    }

    #[test]
    fn a_catalogue_gump_is_only_the_shell_for_the_client_table() {
        let shell = catalogue();
        let (layout, lines) = shell.finish();

        assert!(layout.contains("{ rect 10 39 700 310 4228 }"));
        assert!(lines.iter().any(|line| line == "COMPONENTS"));
        assert!(!layout.contains("tilepicfit"));
    }
}
