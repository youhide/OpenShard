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
use openshard_protocol::gump::{
    ButtonId, CloseGump, GumpAnswer, GumpButton, GumpDisplay, GumpId, GumpKey, GumpLayout, GumpPoint,
    GumpResponse, RawGumpId,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_state::components::Client;
use openshard_state::{CraftGumpContext, CraftGumpPage, WorldState};

use crate::chance::chance;
use crate::craft;
use crate::defs::system;
use crate::recipe::Recipe;
use crate::system::{CraftSystemDef, SystemId, Text};
use openshard_protocol::wire::{ClilocId, Graphic, Hue};

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

/// Rows to a page, in both lists.
const PER_PAGE: usize = 10;

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
        CraftGumpPage::Details(recipe) => match def.recipes.get(usize::from(recipe)) {
            Some(recipe) => details(state, player, def, recipe),
            None => return,
        },
        _ => main(state, player, def, &context),
    };
    let (string, lines) = layout.finish();
    // Close what is already open before drawing: a client told to draw twice
    // draws two windows, and ServUO closes both craft gumps in every branch.
    state.send_packet(
        connection,
        &ServerPacket::CloseGump(CloseGump {
            gump_id: CRAFT_GUMP,
            button: ButtonId::CLOSE_BOX,
        }),
    );
    state.send_packet(
        connection,
        &ServerPacket::GumpDisplay(GumpDisplay {
            serial: GumpKey::on(serial),
            gump_id: CRAFT_GUMP,
            at: GumpPoint::new(WINDOW_X, WINDOW_Y),
            layout: string.to_owned(),
            lines: lines.to_vec(),
        }),
    );
    if let Some(row) = state.row_of_mut(player) {
        row.craft_gump = Some(context);
    }
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
                button: ButtonId::CLOSE_BOX,
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
    // Every id and rectangle here is ServUO's, in ServUO's order. It is furniture
    // and nothing reads it, which is exactly why it is copied rather than
    // approximated — there is no way to tell a subtly wrong frame from a right
    // one except by looking at it in a client.
    layout.background(0, 0, 530, 497, 5054);
    layout.image_tiled(10, 10, 510, 22, 2624);
    layout.image_tiled(10, 292, 150, 45, 2624);
    layout.image_tiled(165, 292, 355, 45, 2624);
    layout.image_tiled(10, 342, 510, 145, 2624);
    layout.image_tiled(10, 37, 200, 250, 2624);
    layout.image_tiled(215, 37, 305, 250, 2624);
    layout.alpha_region(10, 10, 510, 477);

    title(&mut layout, def);
    layout.html_localized_colored(10, 37, 200, 22, ClilocId(1_044_010), LABEL, false, false); // CATEGORIES
    layout.html_localized_colored(215, 37, 305, 22, ClilocId(1_044_011), LABEL, false, false); // SELECTIONS
    layout.html_localized_colored(10, 302, 150, 25, ClilocId(1_044_012), LABEL, false, false); // NOTICES

    layout.button(15, 442, 4017, 4019, GumpButton::Reply, 0, ButtonId::CLOSE_BOX);
    layout.html_localized_colored(50, 445, 150, 18, ClilocId(1_011_441), LABEL, false, false); // EXIT

    layout.button(
        115,
        442,
        4017,
        4019,
        GumpButton::Reply,
        0,
        button_id(kind::MISC, misc::CANCEL),
    );
    layout.html_localized_colored(150, 445, 150, 18, ClilocId(1_112_698), LABEL, false, false); // CANCEL MAKE

    if let Some(notice) = context.notice {
        layout.html_localized_colored(170, 295, 350, 40, notice, LABEL, false, false);
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
            362,
            4005,
            4007,
            GumpButton::Reply,
            0,
            button_id(kind::MISC, misc::RESOURCES),
        );
        if let Some(entry) = entry {
            let held = carried(state, player, axis.graphic, entry.hue);
            label(&mut layout, 50, 365, 250, entry.name, &held.to_string());
        }
    }

    // The categories are drawn on **page zero**, which is what puts them on every
    // page of a paginated selection list. ServUO calls `CreateGroupList` before
    // any `AddPage(1)`, and moving it inside the pagination makes the whole left
    // column vanish the moment a trade's category runs past ten rows — which most
    // of them do.
    groups(&mut layout, def);
    match context.page {
        CraftGumpPage::Resources => resources(&mut layout, state, player, def),
        _ => items(&mut layout, def, context.group),
    }
    layout
}

/// The window's own heading.
fn title(layout: &mut GumpLayout, def: &CraftSystemDef) {
    match def.title {
        Text::Cliloc(cliloc) => {
            layout.html_localized_colored(10, 12, 510, 20, cliloc, LABEL, false, false);
        }
        Text::Str(text) => layout.html_colored(10, 12, 510, 20, text, LABEL, false, false),
    }
}

/// One line that may be a cliloc with an argument or a bare string.
fn label(layout: &mut GumpLayout, x: i32, y: i32, width: i32, text: Text, argument: &str) {
    match text {
        Text::Cliloc(cliloc) if argument.is_empty() => {
            layout.html_localized_colored(x, y, width, 18, cliloc, LABEL, false, false);
        }
        Text::Cliloc(cliloc) => {
            layout.html_localized_args(x, y, width, 18, cliloc, argument, LABEL, false, false);
        }
        Text::Str(line) => layout.label(x, y - 3, LABEL_HUE, line),
    }
}

/// The left-hand column of categories.
fn groups(layout: &mut GumpLayout, def: &CraftSystemDef) {
    for (i, group) in def.groups.iter().enumerate() {
        let y = 80 + i32::try_from(i).unwrap_or(0) * 20;
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
        label(layout, 50, y + 3, 150, *group, "");
    }
}

/// The recipes of one category, ten to a page.
///
/// The index a button carries is the row's place **within the group**, which is
/// what ServUO's `CreateItemList` sends and what [`recipe_in_group`] turns back
/// into a recipe.
fn items(layout: &mut GumpLayout, def: &CraftSystemDef, group: u16) {
    let rows: Vec<(usize, &Recipe)> = def
        .recipes
        .iter()
        .enumerate()
        .filter(|(_, recipe)| recipe.group == group)
        .collect();
    for (i, (_, recipe)) in rows.iter().enumerate() {
        let row = i % PER_PAGE;
        let page = u32::try_from(i / PER_PAGE).unwrap_or(0) + 1;
        if row == 0 {
            if i > 0 {
                layout.button(370, 260, 4005, 4007, GumpButton::Page, page, ButtonId::UNUSED);
                layout.html_localized_colored(405, 263, 100, 18, ClilocId(1_044_045), LABEL, false, false);
                // NEXT PAGE
            }
            layout.page(page);
            if i > 0 {
                layout.button(220, 260, 4014, 4015, GumpButton::Page, page - 1, ButtonId::UNUSED);
                layout.html_localized_colored(255, 263, 100, 18, ClilocId(1_044_044), LABEL, false, false);
                // PREV PAGE
            }
        }
        let y = 60 + i32::try_from(row).unwrap_or(0) * 20;
        let index = ButtonIndex::from_position(i);
        layout.button(
            220,
            y,
            4005,
            4007,
            GumpButton::Reply,
            0,
            button_id(kind::MAKE, index),
        );
        label(layout, 255, y + 3, 220, recipe.name, "");
        layout.button(
            480,
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
        let page = u32::try_from(i / PER_PAGE).unwrap_or(0) + 1;
        if row == 0 {
            if i > 0 {
                layout.button(485, 290, 4005, 4007, GumpButton::Page, page, ButtonId::UNUSED);
            }
            layout.page(page);
            if i > 0 {
                layout.button(455, 290, 4014, 4015, GumpButton::Page, page - 1, ButtonId::UNUSED);
            }
        }
        let y = 60 + i32::try_from(row).unwrap_or(0) * 20;
        let index = ButtonIndex::from_position(i);
        layout.button(
            220,
            y,
            4005,
            4007,
            GumpButton::Reply,
            0,
            button_id(kind::RESOURCE, index),
        );
        let held = carried(state, player, axis.graphic, entry.hue);
        label(layout, 255, y + 3, 220, entry.name, &held.to_string());
    }
}

// ---------------------------------------------------------------------------
// The detail page

/// One recipe's page: what it makes, what it wants, and what the odds are.
///
/// The two percentages are the only place a player can read what the chance
/// curve is doing, which is why they are drawn from the same [`chance`] the roll
/// uses rather than from an approximation of it.
fn details(state: &WorldState, player: EntityId, def: &CraftSystemDef, recipe: &Recipe) -> GumpLayout {
    let mut layout = GumpLayout::new();
    layout.page(0);
    layout.background(0, 0, 530, 417, 5054);
    layout.image_tiled(10, 10, 510, 22, 2624);
    layout.image_tiled(10, 37, 150, 148, 2624);
    layout.image_tiled(165, 37, 355, 90, 2624);
    layout.image_tiled(10, 190, 155, 22, 2624);
    layout.image_tiled(10, 240, 150, 57, 2624);
    layout.image_tiled(165, 132, 355, 80, 2624);
    layout.image_tiled(10, 325, 150, 57, 2624);
    layout.image_tiled(165, 217, 355, 80, 2624);
    layout.image_tiled(165, 302, 355, 80, 2624);
    layout.image_tiled(10, 387, 510, 22, 2624);
    layout.alpha_region(10, 10, 510, 399);

    title(&mut layout, def);
    layout.html_localized_colored(170, 40, 150, 20, ClilocId(1_044_053), LABEL, false, false); // ITEM
    layout.html_localized_colored(10, 217, 150, 22, ClilocId(1_044_055), LABEL, false, false); // MATERIALS
    layout.html_localized_colored(10, 302, 150, 22, ClilocId(1_044_056), LABEL, false, false); // OTHER

    layout.button(405, 387, 4005, 4007, GumpButton::Reply, 0, detail::MAKE);
    layout.html_localized_colored(445, 390, 150, 18, ClilocId(1_044_151), LABEL, false, false); // MAKE NOW
    layout.button(15, 387, 4014, 4016, GumpButton::Reply, 0, detail::BACK);
    layout.html_localized_colored(50, 390, 150, 18, ClilocId(1_044_150), LABEL, false, false); // BACK

    label(&mut layout, 330, 40, 180, recipe.name, "");
    layout.item(90, 110, recipe.graphic, recipe.hue);

    let mut other = 0;
    if recipe.use_all_res {
        layout.html_localized_colored(170, 302, 310, 18, ClilocId(1_048_176), LABEL, false, false); // makes as many as possible
        other += 1;
    }
    if recipe.markable {
        layout.html_localized_colored(
            170,
            302 + other * 20,
            310,
            18,
            ClilocId(1_044_059),
            LABEL,
            false,
            false,
        ); // may hold a maker's mark
    }

    // One row per required skill, at the value it starts to be possible.
    for (i, want) in recipe.skills.iter().enumerate() {
        let y = 132 + i32::try_from(i).unwrap_or(0) * 20;
        layout.html_localized_colored(170, y, 200, 18, skill_label(want.skill), LABEL, false, false);
        layout.label(430, y, LABEL_HUE, tenths(want.min.max(0)));
    }

    let odds = chance(state, player, def, recipe);
    layout.html_localized_colored(170, 80, 250, 18, ClilocId(1_044_057), LABEL, false, false); // Success Chance:
    layout.label(430, 80, LABEL_HUE, percent(odds.success));
    if recipe.markable {
        layout.html_localized_colored(170, 100, 250, 18, ClilocId(1_044_058), LABEL, false, false); // Exceptional Chance:
        layout.label(430, 100, LABEL_HUE, percent(odds.exceptional));
    }

    // Four material rows at most, which is ServUO's own limit and more than any
    // recipe in the five tables uses.
    for (i, res) in recipe.resources.iter().take(4).enumerate() {
        let y = 219 + i32::try_from(i).unwrap_or(0) * 20;
        let hue = axis_hue(def, res, 0);
        let name = axis_name(def, res, 0).unwrap_or(res.name);
        label(&mut layout, 170, y, 220, name, "");
        layout.label(430, y, LABEL_HUE, res.amount.to_string());
        let _ = hue;
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
        openshard_items::carried_amount_of_hue(state, serial, graphic, Some(hue))
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

    // The detail page's buttons are plain small numbers, not encoded ones, so it
    // is dispatched on its own before the list's decode.
    if let CraftGumpPage::Details(recipe) = context.page {
        match answer {
            GumpAnswer::Pressed(detail::MAKE) => {
                make(state, player, context, recipe);
            }
            // `detail::BACK` *is* the close box (see the constant), so this arm
            // is both, exactly as ServUO's `OnResponse` reads it.
            GumpAnswer::Closed => {
                let mut back = context;
                back.page = CraftGumpPage::Items;
                open(state, player, back);
            }
            GumpAnswer::Pressed(_) => {}
        }
        return true;
    }

    let GumpAnswer::Pressed(pressed) = answer else {
        return true; // EXIT, or the close box — the same id on this page
    };
    let (kind, index) = decode_button(pressed);
    match kind {
        kind::GROUP => {
            // An invented large index selects nothing. Falling back to zero
            // here would silently turn it into the first real category.
            let Some(group) = index.as_group() else {
                return true;
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
        kind::MISC => match index {
            misc::RESOURCES => {
                let mut next = context;
                next.page = CraftGumpPage::Resources;
                open(state, player, next);
            }
            misc::CANCEL => {
                state
                    .registry
                    .remove::<openshard_state::components::Crafting>(player);
                let mut next = context;
                next.page = CraftGumpPage::Items;
                open(state, player, next);
            }
            _ => {}
        },
        _ => {}
    }
    true
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
}
