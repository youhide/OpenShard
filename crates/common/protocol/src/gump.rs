//! Generic gumps: the server-driven dialog (`0xB0`) and the client's reply
//! (`0xB1`).
//!
//! A gump is UO's windowing primitive — a paperdoll, a shopkeeper, a book, and
//! the staff menus this shard needs are all gumps. The server sends a *layout*: a
//! little command language (`{ resizepic ... }`, `{ button ... }`, `{ page N }`)
//! plus a list of text strings the layout refers to by index. The client renders
//! it and, when the player clicks a button, sends back the button id in a `0xB1`.
//!
//! This is the uncompressed `0xB0` form. Clients from 5.0 also accept a zlib-packed
//! `0xDD`, and ClassicUO and the modern 2D client both still render `0xB0`, so the
//! compression is a later optimisation, not a requirement.

pub mod layout;

use std::fmt;
use std::fmt::Write as _;

use crate::codec::{PacketReader, PacketWriter};
use crate::error::DecodeError;
use crate::packet::{DecodePacket, EncodePacket, PacketLength};
use crate::serial::Serial;
use crate::version::ClientVersion;
use crate::wire::{ClilocId, Graphic, Hue};

/// Which dialog: the id the server gives a gump and the client echoes back in
/// its `0xB1`, so a reply can be routed to whoever drew the window.
///
/// Server-chosen in every direction — the client only ever repeats one — so a
/// `GumpId` in hand means *this shard named this dialog*. What arrives off the
/// wire is a [`RawGumpId`] until a handler says it is one of its own.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, serde::Deserialize, serde::Serialize)]
#[serde(transparent)]
pub struct GumpId(pub u32);

/// Every dialog this engine draws, in one table.
///
/// # Why they are not each declared where they are drawn
///
/// They were, and two of them collided: the guild window and the craft window
/// were both `0x0052_0001`, and the only thing that noticed was the craft tests
/// going red — because the router asks each handler in turn, and the first one
/// to claim a reply wins. Nothing on the wire or in the type system says an id
/// is taken. Nine constants across five crates is nine chances to pick one
/// already in use, and no place to look first.
///
/// So the ids live here, below every crate that draws a window, and
/// [`all_gump_ids_are_distinct`] is the check that could not exist while they
/// were scattered. A window still names its own — `pub const CRAFT_GUMP: GumpId
/// = id::CRAFT;` — so the constant is still where the code that draws it is.
///
/// [`all_gump_ids_are_distinct`]: self#tests
pub mod id {
    use super::GumpId;

    /// The staff menu behind `.admin`. High byte `0xAD` for "admin".
    pub const ADMIN: GumpId = GumpId(0x00AD_0001);
    /// The item creator opened from the staff menu.  It shares the admin
    /// namespace but is a distinct reply target, so a form submission cannot
    /// be mistaken for a world-management button.
    pub const ADMIN_ITEM: GumpId = GumpId(0x00AD_0002);
    /// The creature-placement request from the F1 administrator catalogue.
    /// It is deliberately separate from item creation: its answer raises a
    /// target cursor instead of putting an object into a backpack.
    pub const ADMIN_CREATURE: GumpId = GumpId(0x00AD_0003);
    /// The quest log, offer and turn-in — one window, many pages.
    pub const QUEST: GumpId = GumpId(0x0051_0001);
    /// "Give this quest up?"
    pub const QUEST_RESIGN: GumpId = GumpId(0x0051_0002);
    /// The craft window.
    pub const CRAFT: GumpId = GumpId(0x0052_0001);
    /// A runebook's entries.
    pub const RUNEBOOK: GumpId = GumpId(0x0053_0001);
    /// A moongate's destination list.
    pub const MOONGATE: GumpId = GumpId(0x0053_0002);
    /// A healer's "wouldst thou like to be resurrected?".
    pub const HEALER: GumpId = GumpId(0x0054_0001);
    /// The guild window: founding, the roster, wars and alliances.
    pub const GUILD: GumpId = GumpId(0x0055_0001);
    /// What Animal Lore draws about a creature.
    pub const ANIMAL_LORE: GumpId = GumpId(0x0A11);
    /// A house sign: who owns it, and who may come in.
    pub const HOUSE: GumpId = GumpId(0x0056_0001);

    /// The table, for the distinctness check and for anything that wants to ask
    /// whether an id belongs to the engine at all.
    pub const ALL: [GumpId; 12] = [
        ADMIN,
        ADMIN_ITEM,
        ADMIN_CREATURE,
        QUEST,
        QUEST_RESIGN,
        CRAFT,
        RUNEBOOK,
        MOONGATE,
        HEALER,
        GUILD,
        ANIMAL_LORE,
        HOUSE,
    ];
}

/// Wire contracts for engine-owned administrator requests.
///
/// The classic gump and the F1 developer panel share these ids, so their
/// meaning is not duplicated across client and server UI implementations.
pub mod admin {
    use super::{ButtonId, SwitchId};

    pub const ITEM_CREATE: ButtonId = ButtonId(1);
    pub const ITEM_GRAPHIC_FIELD: u16 = 1;
    pub const ITEM_HUE_FIELD: u16 = 2;
    pub const ITEM_AMOUNT_FIELD: u16 = 3;
    pub const ITEM_STACKABLE: SwitchId = SwitchId(1);

    /// Submit one of the server-owned animal presets for placement.
    pub const CREATURE_CREATE: ButtonId = ButtonId(1);
    /// The animal preset selected in the F1 catalogue. This is an id, not a
    /// body graphic: the shard chooses all of a creature's gameplay stats.
    pub const CREATURE_KIND_FIELD: u16 = 1;
}

/// A dialog id exactly as a client's `0xB1` echoed it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct RawGumpId(pub u32);

impl RawGumpId {
    /// The dialog this answers, when it is one of the `offered` — the ids the
    /// asking handler drew.
    ///
    /// `docs/protocol_newtypes.md` N5's "is this one I offered", and the reason
    /// the argument is a list: the quest system draws two windows and claims a
    /// reply for either. An `Option` rather than a typed error, on
    /// `RawSerial::validate`'s licence: every handler in the router asks in
    /// turn, and all but one of them legitimately answer "not mine" — a reply
    /// for no engine dialog at all belongs to the script pack and is forwarded,
    /// not refused.
    #[must_use]
    pub fn validate(self, offered: &[GumpId]) -> Option<GumpId> {
        offered.iter().copied().find(|id| id.0 == self.0)
    }
}

/// The id a layout gives one of its buttons — what comes back when it is
/// pressed. Server-chosen, like [`GumpId`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ButtonId(pub u32);

impl ButtonId {
    /// The id the client's own close box reports: `0`.
    ///
    /// A layout may deliberately hand this to a button of its own — the quest
    /// window's `X` does — and then the two are the same answer by
    /// construction, which is what ServUO's `Buttons.Close = 0` means too.
    ///
    /// [`UNUSED`](Self::UNUSED) is this same `0` saying something else, and the
    /// two are the only meanings the value carries. A *third* would be one too
    /// many: at that point the type wants to be an enum with a `Reply(u32)` arm
    /// rather than a newtype with named zeroes, because three names for one
    /// value is a discriminant nobody wrote down. See
    /// `docs/protocol_newtypes.md` N5.
    pub const CLOSE_BOX: Self = Self(0);

    /// What a [`GumpButton::Page`] button puts in the id field: nothing.
    ///
    /// A page button is handled inside the client — it flips a page and sends
    /// no `0xB1` at all — so its id is never read by anyone. The same `0` as
    /// [`CLOSE_BOX`](Self::CLOSE_BOX) and a different statement, which is why
    /// it has its own name: ServUO writes `0` here too, and reading that as
    /// "the close box" would be a coincidence mistaken for a meaning.
    pub const UNUSED: Self = Self(0);
}

/// A button id exactly as a client's `0xB1` sent it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct RawButtonId(pub u32);

impl RawButtonId {
    /// What the player did — total, and the point of the type.
    ///
    /// Every one of the 2³² values is either the close box or a button id, so
    /// nothing can be refused here; what the split removes is the bare `0`
    /// that three handlers were each comparing against by hand. The same shape
    /// as `DoubleClick::interpret` (N4): one field carrying a value *and* an
    /// answer, told apart once.
    ///
    /// Whether the id is one the *layout* actually drew is a different
    /// question, and a per-window one — the craft window's ids are computed
    /// (`kind + index * 7`) where the quest log's are a table plus a row
    /// offset — so it stays in each handler's `match`, which is where it has
    /// always been.
    #[inline]
    #[must_use]
    pub const fn interpret(self) -> GumpAnswer {
        match self.0 {
            0 => GumpAnswer::Closed,
            id => GumpAnswer::Pressed(ButtonId(id)),
        }
    }
}

/// What a `0xB1` says the player did with the window.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GumpAnswer {
    /// The close box: dismissed without choosing.
    Closed,
    /// A button was pressed, by the id the layout gave it.
    Pressed(ButtonId),
}

/// The id a layout gives a checkbox or a radio button. The client returns the
/// ids of the ones left *on* when a reply button is pressed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct SwitchId(pub u32);

/// A switch id exactly as a client's `0xB1` sent it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct RawSwitchId(pub u32);

impl RawSwitchId {
    /// The switch this names, when the group that was drawn had `offered` of
    /// them, numbered from zero.
    ///
    /// Every switch group this engine draws is numbered that way — a radio per
    /// destination, a yes and a no — so the count is the whole domain, and a
    /// group's *length* is the one thing a handler always still has when the
    /// reply arrives. Refused rather than clamped, for
    /// [`crate::context::RawContextMenuIndex::validate`]'s reason: picking a
    /// neighbouring row is worse than picking none.
    pub const fn validate(self, offered: usize) -> Result<SwitchId, InvalidSwitchId> {
        if (self.0 as usize) < offered {
            Ok(SwitchId(self.0))
        } else {
            Err(InvalidSwitchId { id: self.0, offered })
        }
    }
}

/// A `0xB1` naming a switch the gump it answers does not have.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InvalidSwitchId {
    /// The id the client sent.
    pub id: u32,
    /// How many switches the group actually had.
    pub offered: usize,
}

impl fmt::Display for InvalidSwitchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "switch {} was set in a group of {}", self.id, self.offered)
    }
}

impl std::error::Error for InvalidSwitchId {}

/// A point in the client's gump space: pixels, with the origin at the top left
/// of whatever the coordinate is measured against.
///
/// A pair and not two numbers, for `MapSize`'s and `Vitals`' reason (N1/N2):
/// both halves are read and written together, and half a position is not a
/// smaller one, it is a window in the wrong place. Signed because the layout
/// language allows it — the quest frame puts an element at `x = -16`, and
/// formatting that through an unsigned type sends `4294967280` and the client
/// drops the whole layout.
///
/// The same space is measured from two origins: [`GumpDisplay`] positions a
/// window on the screen, and `containers::ContainedItem` positions an icon
/// inside a container's art. The wire widths differ (four bytes and two), the
/// space does not.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct GumpPoint {
    /// Pixels from the left.
    pub x: i32,
    /// Pixels from the top.
    pub y: i32,
}

impl GumpPoint {
    /// A point, from its two halves.
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// The value a client keys an open gump on and echoes back in its `0xB1`.
///
/// Opaque the way [`CursorId`](crate::wire::CursorId) is: the server picks it,
/// the client repeats it, and nothing about the number itself means anything.
/// This engine keys a window on the mobile it was drawn for, so
/// [`GumpKey::on`] is how one is usually made — but that is a convention and
/// not the field's meaning. ServUO puts `Gump.Serial` here, a per-instance
/// counter that is never an object, and `AnimalLore` keys its window on its own
/// dialog id. So this is deliberately not a [`Serial`]: `0` is legal and means
/// a standalone dialog, which no serial can say.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct GumpKey(pub u32);

impl GumpKey {
    /// A dialog belonging to nothing in the world. ServUO passes `0` for these
    /// too; some clients will not answer a `0xB1` for one, so a window that
    /// wants a reply should be keyed on something.
    pub const STANDALONE: Self = Self(0);

    /// The key a window drawn for `object` takes — this engine's convention.
    #[must_use]
    pub const fn on(object: Serial) -> Self {
        Self(object.raw())
    }
}

/// A gump key exactly as a client's `0xB1` echoed it.
///
/// Class D in `docs/protocol_newtypes.md`'s table, and so it has no promotion:
/// nothing reads it. A reply is routed by its [`RawGumpId`] alone, and each
/// handler then matches against the context it remembers drawing — which is a
/// stronger check than the echo could be, since the client chose what to echo.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct RawGumpKey(pub u32);

/// The client's white, in the 15-bit colour a gump element takes.
pub const GUMP_WHITE: u32 = 0x7FFF;
/// ServUO's `BaseQuestGump.DarkGreen` — the colour a quest title is drawn in.
pub const GUMP_DARK_GREEN: u32 = 10_000;
/// ServUO's `BaseQuestGump.LightGreen` — body text and objective labels.
pub const GUMP_LIGHT_GREEN: u32 = 90_000;
/// The red a failed quest is drawn in (ServUO passes `0x3C00` literally).
pub const GUMP_RED: u32 = 0x3C00;

/// Expand one of the client's 15-bit colours to the 24-bit RGB an HTML
/// `<BASEFONT COLOR=#RRGGBB>` tag wants. ServUO's `BaseQuestGump.C16232`.
#[must_use]
pub const fn gump_color_rgb(c16: u32) -> u32 {
    let c16 = c16 & 0x7FFF;
    let r = ((c16 >> 10) & 0x1F) << 3;
    let g = ((c16 >> 5) & 0x1F) << 3;
    let b = (c16 & 0x1F) << 3;
    (r << 16) | (g << 8) | b
}

/// What a gump button does when it is clicked.
///
/// `Reply` closes the gump and sends a `0xB1`; `Page` is handled entirely by the
/// client, flipping to another page of the same gump without a round trip.
/// ServUO's `GumpButtonType`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GumpButton {
    /// Send the id back to the server.
    Reply,
    /// Flip to another page, client-side.
    Page,
}

impl GumpButton {
    const fn code(self) -> i64 {
        match self {
            Self::Page => 0,
            Self::Reply => 1,
        }
    }
}

/// A gump layout under construction: the command string, plus the text lines it
/// refers to by index.
///
/// The wire format is a little command language (`{ resizepic 0 0 5054 200 200 }`)
/// and a separate table of strings; an element that shows text stores it in the
/// table and names its index. Writing that by hand works for a two-button dialog
/// and does not survive a real one — the quest log is forty-odd elements, and one
/// mistyped keyword renders as an empty window with nothing to debug. So the
/// elements are methods, and each one is ported from the matching
/// `Server/Gumps/Gump*.cs` in ServUO: the keyword *and* the argument order, both
/// of which the client reads positionally.
///
/// Call [`finish`](Self::finish) and hand the two halves to
/// [`GumpDisplay`].
#[derive(Clone, Default, Debug)]
pub struct GumpLayout {
    layout: String,
    lines: Vec<String>,
}

// A gump element's arguments are the client's, in the client's order — grouping
// them into structs would hide exactly the thing that has to be checked against
// ServUO line by line. Long argument lists are the format, not a design choice.
#[allow(clippy::too_many_arguments)]
impl GumpLayout {
    /// An empty layout.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern a text line and return the index the layout refers to it by.
    ///
    /// Identical lines are *not* pooled: ServUO does not pool them either, and an
    /// index that silently aliases another element's text is a worse surprise than
    /// a few duplicated strings.
    pub fn text(&mut self, line: impl Into<String>) -> usize {
        self.lines.push(line.into());
        self.lines.len() - 1
    }

    /// The window may not be dragged.
    pub fn no_move(&mut self) {
        self.layout.push_str("{ nomove }");
    }

    /// The window has no close box — the player must answer with a button.
    pub fn no_close(&mut self) {
        self.layout.push_str("{ noclose }");
    }

    /// The window cannot be dismissed with right-click.
    pub fn no_dispose(&mut self) {
        self.layout.push_str("{ nodispose }");
    }

    /// The window cannot be resized.
    pub fn no_resize(&mut self) {
        self.layout.push_str("{ noresize }");
    }

    /// Start a page. Page `0` is drawn on every page; the rest are switched
    /// between by a [`GumpButton::Page`] button, client-side.
    pub fn page(&mut self, page: u32) {
        self.element("page", &[i64::from(page)]);
    }

    /// A gump-art image.
    pub fn image(&mut self, x: i32, y: i32, gump: u32) {
        self.element("gumppic", &[i64::from(x), i64::from(y), i64::from(gump)]);
    }

    /// A gump-art image in a hue. The hue rides as a `hue=` suffix, not a plain
    /// argument — the client parses the two forms differently.
    pub fn image_hued(&mut self, x: i32, y: i32, gump: u32, hue: u32) {
        self.element("gumppic", &[i64::from(x), i64::from(y), i64::from(gump)]);
        // Re-open the element the way ServUO's `GumpImage` does: the suffix is
        // appended inside the braces, after the id.
        let brace = self.layout.pop();
        debug_assert_eq!(brace, Some('}'));
        self.layout.pop(); // the space before it
        write!(&mut self.layout, " hue={hue} }}").expect("writing to a String cannot fail");
    }

    /// An image tiled to fill a rectangle.
    pub fn image_tiled(&mut self, x: i32, y: i32, width: i32, height: i32, gump: u32) {
        self.element(
            "gumppictiled",
            &[
                i64::from(x),
                i64::from(y),
                i64::from(width),
                i64::from(height),
                i64::from(gump),
            ],
        );
    }

    /// An OpenShard flat-colour rectangle in RGB555.  This is deliberately a
    /// primitive rather than a gump-art id: table cells and their one-pixel
    /// borders should not depend on whatever ornamental art an installed UO
    /// client happens to ship.
    pub fn rect(&mut self, x: i32, y: i32, width: i32, height: i32, color: u16) {
        self.element(
            "rect",
            &[
                i64::from(x),
                i64::from(y),
                i64::from(width),
                i64::from(height),
                i64::from(color),
            ],
        );
    }

    /// A darkened, semi-transparent rectangle.
    pub fn alpha_region(&mut self, x: i32, y: i32, width: i32, height: i32) {
        self.element(
            "checkertrans",
            &[i64::from(x), i64::from(y), i64::from(width), i64::from(height)],
        );
    }

    /// A stretched background frame. Note the order: the gump id comes *before*
    /// the size, unlike every other element.
    pub fn background(&mut self, x: i32, y: i32, width: i32, height: i32, gump: u32) {
        self.element(
            "resizepic",
            &[
                i64::from(x),
                i64::from(y),
                i64::from(gump),
                i64::from(width),
                i64::from(height),
            ],
        );
    }

    /// A button. `normal` and `pressed` are gump art; `id` is what comes back in
    /// the `0xB1` (for [`GumpButton::Reply`]) or the page to flip to (for
    /// [`GumpButton::Page`], where `param` carries the page instead).
    pub fn button(
        &mut self,
        x: i32,
        y: i32,
        normal: u32,
        pressed: u32,
        kind: GumpButton,
        param: u32,
        id: ButtonId,
    ) {
        self.element(
            "button",
            &[
                i64::from(x),
                i64::from(y),
                i64::from(normal),
                i64::from(pressed),
                kind.code(),
                i64::from(param),
                i64::from(id.0),
            ],
        );
    }

    /// A radio button. Only one per group may be on; the client returns the
    /// `switch_id` of whichever is set when a reply button is pressed.
    pub fn radio(&mut self, x: i32, y: i32, off: u32, on: u32, initial: bool, switch_id: SwitchId) {
        self.element(
            "radio",
            &[
                i64::from(x),
                i64::from(y),
                i64::from(off),
                i64::from(on),
                i64::from(initial),
                i64::from(switch_id.0),
            ],
        );
    }

    /// A checkbox. Like [`radio`](Self::radio), but independent of its neighbours.
    pub fn check(&mut self, x: i32, y: i32, off: u32, on: u32, initial: bool, switch_id: SwitchId) {
        self.element(
            "checkbox",
            &[
                i64::from(x),
                i64::from(y),
                i64::from(off),
                i64::from(on),
                i64::from(initial),
                i64::from(switch_id.0),
            ],
        );
    }

    /// A one-line label in a hue.
    pub fn label(&mut self, x: i32, y: i32, hue: u32, text: impl Into<String>) {
        let line = self.text(text);
        self.element("text", &[i64::from(x), i64::from(y), i64::from(hue), line as i64]);
    }

    /// A label clipped to a box rather than overflowing it.
    pub fn cropped_label(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        hue: u32,
        text: impl Into<String>,
    ) {
        let line = self.text(text);
        self.element(
            "croppedtext",
            &[
                i64::from(x),
                i64::from(y),
                i64::from(width),
                i64::from(height),
                i64::from(hue),
                line as i64,
            ],
        );
    }

    /// A block of HTML text.
    pub fn html(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        text: impl Into<String>,
        background: bool,
        scrollbar: bool,
    ) {
        let line = self.text(text);
        self.element(
            "htmlgump",
            &[
                i64::from(x),
                i64::from(y),
                i64::from(width),
                i64::from(height),
                line as i64,
                i64::from(background),
                i64::from(scrollbar),
            ],
        );
    }

    /// A block of HTML text in one of the client's 15-bit colours.
    ///
    /// There is no colour argument on `htmlgump`: ServUO wraps the text in a
    /// `<BASEFONT>` tag instead (`BaseQuestGump.Color`), which is why this takes
    /// the same colour constants the localized form does and converts.
    pub fn html_colored(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        text: &str,
        color: u32,
        background: bool,
        scrollbar: bool,
    ) {
        let rgb = gump_color_rgb(color);
        let wrapped = format!("<BASEFONT COLOR=#{rgb:06X}>{text}</BASEFONT>");
        self.html(x, y, width, height, wrapped, background, scrollbar);
    }

    /// A localized string — the client looks the number up in its own `cliloc`
    /// file, so no text travels.
    pub fn html_localized(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        cliloc: ClilocId,
        background: bool,
        scrollbar: bool,
    ) {
        self.element(
            "xmfhtmlgump",
            &[
                i64::from(x),
                i64::from(y),
                i64::from(width),
                i64::from(height),
                i64::from(cliloc.0),
                i64::from(background),
                i64::from(scrollbar),
            ],
        );
    }

    /// A localized string in a colour.
    pub fn html_localized_colored(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        cliloc: ClilocId,
        color: u32,
        background: bool,
        scrollbar: bool,
    ) {
        self.element(
            "xmfhtmlgumpcolor",
            &[
                i64::from(x),
                i64::from(y),
                i64::from(width),
                i64::from(height),
                i64::from(cliloc.0),
                i64::from(background),
                i64::from(scrollbar),
                i64::from(color),
            ],
        );
    }

    /// A localized string with `~1_VAL~`-style arguments filled in, tab-separated.
    ///
    /// Note the argument order: unlike every other localized form, the cliloc
    /// comes *after* the flags and colour. ServUO's `xmfhtmltok`.
    pub fn html_localized_args(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        cliloc: ClilocId,
        args: &str,
        color: u32,
        background: bool,
        scrollbar: bool,
    ) {
        self.element(
            "xmfhtmltok",
            &[
                i64::from(x),
                i64::from(y),
                i64::from(width),
                i64::from(height),
                i64::from(background),
                i64::from(scrollbar),
                i64::from(color),
                i64::from(cliloc.0),
            ],
        );
        let brace = self.layout.pop();
        debug_assert_eq!(brace, Some('}'));
        self.layout.pop();
        write!(&mut self.layout, " @{args}@ }}").expect("writing to a String cannot fail");
    }

    /// An item, drawn from the art the world uses rather than the gump art.
    pub fn item(&mut self, x: i32, y: i32, graphic: Graphic, hue: Hue) {
        if hue == Hue::NONE {
            self.element("tilepic", &[i64::from(x), i64::from(y), i64::from(graphic.0)]);
        } else {
            self.element(
                "tilepichue",
                &[i64::from(x), i64::from(y), i64::from(graphic.0), i64::from(hue.0)],
            );
        }
    }

    /// An OpenShard item cell: the decoded item art is proportionally fitted
    /// and centred inside this rectangle by our client. Classic clients do
    /// not know this extension and simply leave its cell empty.
    pub fn fitted_item(&mut self, x: i32, y: i32, width: i32, height: i32, graphic: Graphic, hue: Hue) {
        self.element(
            "tilepicfit",
            &[
                i64::from(x),
                i64::from(y),
                i64::from(width),
                i64::from(height),
                i64::from(graphic.0),
                i64::from(hue.0),
            ],
        );
    }

    /// A text field the player types into; its contents come back in the `0xB1`
    /// under `entry_id`.
    pub fn text_entry(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        hue: u32,
        entry_id: u32,
        initial: impl Into<String>,
    ) {
        let line = self.text(initial);
        self.element(
            "textentry",
            &[
                i64::from(x),
                i64::from(y),
                i64::from(width),
                i64::from(height),
                i64::from(hue),
                i64::from(entry_id),
                line as i64,
            ],
        );
    }

    /// The finished layout string and its text table.
    #[must_use]
    pub fn finish(&self) -> (&str, &[String]) {
        (&self.layout, &self.lines)
    }

    /// One `{ keyword arg arg ... }` element.
    fn element(&mut self, keyword: &str, args: &[i64]) {
        self.layout.push_str("{ ");
        self.layout.push_str(keyword);
        for arg in args {
            self.layout.push(' ');
            self.layout.push_str(&arg.to_string());
        }
        self.layout.push_str(" }");
    }
}

/// `0xBF` subcommand `0x04` — close an open gump on the client. Fixed 13 bytes.
///
/// The server sends this before re-sending a dialog that replaces itself (the
/// quest gump's own page chain does exactly that); without it the client stacks
/// the new window on the old one. `button` is what the closed gump reports as its
/// answer — ServUO passes `0` for a silent close. Ported from ServUO's
/// `CloseGump`.
///
/// Fixed despite living under `0xBF`, for the same reason [`crate::mobile::StatLocks`]
/// is: the body never varies, so the constant `u16(13)` is written here by hand,
/// since [`crate::packet::frame_body`] only back-patches a length for
/// [`PacketLength::Variable`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CloseGump {
    /// Which dialog to close.
    pub gump_id: GumpId,
    /// What the closed gump reports as its answer. ServUO passes
    /// [`ButtonId::CLOSE_BOX`], and so does every caller here.
    pub button: ButtonId,
}

impl CloseGump {
    /// Which `0xBF` this is.
    pub const SUBCOMMAND: u16 = 0x0004;
}

impl EncodePacket for CloseGump {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Fixed(13);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(13);
        out.u16(Self::SUBCOMMAND);
        out.u32(self.gump_id.0);
        out.u32(self.button.0);
    }
}

impl DecodePacket for CloseGump {
    const ID: u8 = 0xBF;

    /// # The reader is past the length either way
    ///
    /// `0xBF` is `Variable` in the framing table, so `decode_server` skips the
    /// two length bytes before this runs — even for the handful of `0xBF`
    /// payloads (this one, `StatLocks`) that are really fixed and write their own
    /// length. That is what makes every `0xBF` body start at its subcommand, and
    /// is why reading it here is uniform rather than a special case.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for a close-gump",
                value: u32::from(subcommand),
            });
        }
        Ok(Self {
            gump_id: GumpId(reader.u32()?),
            button: ButtonId(reader.u32()?),
        })
    }
}

/// `0xB0` — display a generic gump. Variable length.
///
/// `serial` is the context the gump belongs to (a mobile, an item, or `0` for a
/// standalone dialog); `gump_id` names *which* dialog, and the client echoes both
/// back in its `0xB1` so the server knows what was answered. `layout` is the gump
/// command string; `lines` are the strings it references by index (`{ text X Y
/// LINE }` picks `lines[LINE]`), sent as big-endian UTF-16.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GumpDisplay {
    /// What the client keys the open window on, and echoes back.
    pub serial: GumpKey,
    /// Which dialog — echoed back in the `0xB1`.
    pub gump_id: GumpId,
    /// Where the window opens on the screen.
    pub at: GumpPoint,
    /// The gump command string — see [`GumpLayout::finish`].
    pub layout: String,
    /// The strings the layout refers to by index.
    pub lines: Vec<String>,
}

impl EncodePacket for GumpDisplay {
    const ID: u8 = 0xB0;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(self.serial.0);
        out.u32(self.gump_id.0);
        out.u32(self.at.x as u32);
        out.u32(self.at.y as u32);

        // The layout is ASCII with a byte-count prefix; the client reads exactly
        // that many bytes, so no terminator is needed.
        let layout = self.layout.as_bytes();
        out.u16(u16::try_from(layout.len()).unwrap_or(u16::MAX));
        out.bytes(layout);

        // The text table: a count, then each line as its own UTF-16 run.
        out.u16(u16::try_from(self.lines.len()).unwrap_or(u16::MAX));
        for line in &self.lines {
            let units: Vec<u16> = line.encode_utf16().collect();
            out.u16(u16::try_from(units.len()).unwrap_or(u16::MAX));
            for unit in units {
                out.u16(unit); // big-endian, like every u16 on the wire
            }
        }
    }
}

impl DecodePacket for GumpDisplay {
    const ID: u8 = 0xB0;

    /// The other end of [`EncodePacket::encode_body`] above, for a client
    /// reading a window it has been sent.
    ///
    /// Nothing here refuses a layout: the string is handed on as it arrived and
    /// [`layout::parse`] is what makes sense of it, element by element, keeping
    /// the ones it does not know rather than dropping the window. A malformed
    /// element is a picture that does not draw; a malformed *length* is a
    /// desynchronised stream, and that is what the reader's bounds catch.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let serial = GumpKey(reader.u32()?);
        let gump_id = GumpId(reader.u32()?);
        // Written as unsigned by the encoder above, and signed in the layout's
        // own space — see `GumpPoint`. The cast is the same one, backwards.
        let x = reader.u32()? as i32;
        let y = reader.u32()? as i32;

        let layout_len = reader.u16()? as usize;
        let layout = String::from_utf8_lossy(reader.bytes(layout_len)?).into_owned();

        let count = reader.u16()? as usize;
        // Capped only in the *reservation*: the count is the sender's claim and
        // the reader is what proves each line is really there, so a huge count
        // over a short packet fails on the next read rather than on an
        // allocation.
        let mut lines = Vec::with_capacity(count.min(64));
        for _ in 0..count {
            let units_len = reader.u16()? as usize;
            let mut units = Vec::with_capacity(units_len.min(256));
            for _ in 0..units_len {
                units.push(reader.u16()?);
            }
            lines.push(String::from_utf16_lossy(&units));
        }

        Ok(Self {
            serial,
            gump_id,
            at: GumpPoint::new(x, y),
            layout,
            lines,
        })
    }
}

/// `0xB1` — the client's answer to a gump: which button was pressed, plus the
/// state of any switches (checkboxes, radios) and text fields.
///
/// A button of `0` is the close box — the player dismissed the gump without
/// choosing. Otherwise it is the id the layout gave the button that was clicked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GumpResponse {
    /// The key the gump was opened under, echoed back. Never read — see
    /// [`RawGumpKey`].
    pub serial: RawGumpKey,
    /// Which dialog answered — the `gump_id` the server sent, as echoed.
    pub gump_id: RawGumpId,
    /// The button pressed, or the close box. See [`RawButtonId::interpret`].
    pub button: RawButtonId,
    /// The ids of the switches (checkboxes, radio buttons) left *on*.
    pub switches: Vec<RawSwitchId>,
    /// Text fields, as `(field id, contents)`.
    ///
    /// Both halves stay untyped, and the reason is that a field id means nothing
    /// on its own: it is whatever number the *layout* gave it, so the check "is
    /// this a field I drew" belongs to whoever drew the window and cannot be made
    /// here. The guild window is the first that reads one — it names its two
    /// founding fields and gives each roster row its own index — and it resolves
    /// them against the context it remembers drawing, which is the same rule its
    /// buttons follow. The engine types what the engine can check —
    /// `docs/protocol_newtypes.md`, N5's amendments.
    pub text_entries: Vec<(u16, String)>,
}

impl DecodePacket for GumpResponse {
    const ID: u8 = 0xB1;

    /// `decode_packet` has already skipped the length field for a `Variable`
    /// id — see its doc comment — so this starts straight at the serial.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let serial = RawGumpKey(reader.u32()?);
        let gump_id = RawGumpId(reader.u32()?);
        let button = RawButtonId(reader.u32()?);

        let switch_count = reader.u32()? as usize;
        let mut switches = Vec::with_capacity(switch_count.min(64));
        for _ in 0..switch_count {
            switches.push(RawSwitchId(reader.u32()?));
        }

        let text_count = reader.u32()? as usize;
        let mut text_entries = Vec::with_capacity(text_count.min(64));
        for _ in 0..text_count {
            let id = reader.u16()?;
            let len = reader.u16()? as usize;
            let mut units = Vec::with_capacity(len);
            for _ in 0..len {
                units.push(reader.u16()?);
            }
            text_entries.push((id, String::from_utf16_lossy(&units)));
        }

        Ok(Self {
            serial,
            gump_id,
            button,
            switches,
            text_entries,
        })
    }
}

impl GumpResponse {
    /// Encode a whole `0xB1`. What `crates/client/net` sends when a button is
    /// pressed; this server only ever decodes one.
    ///
    /// Every field is a `Raw*` and that is the point: a reply *is* an echo. The
    /// client repeats the key and the dialog id it was sent, and the server's
    /// check is against the window it remembers drawing — so nothing is
    /// promoted on the way out, because nothing here is this end's to name.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        crate::packet::frame_body(
            <Self as DecodePacket>::ID,
            PacketLength::Variable,
            |out: &mut PacketWriter| {
                out.u32(self.serial.0);
                out.u32(self.gump_id.0);
                out.u32(self.button.0);

                out.u32(self.switches.len() as u32);
                for switch in &self.switches {
                    out.u32(switch.0);
                }

                out.u32(self.text_entries.len() as u32);
                for (id, text) in &self.text_entries {
                    out.u16(*id);
                    let units: Vec<u16> = text.encode_utf16().collect();
                    out.u16(u16::try_from(units.len()).unwrap_or(u16::MAX));
                    for unit in units {
                        out.u16(unit);
                    }
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    /// The check that could not exist while the ids were declared in five
    /// different crates.
    ///
    /// A duplicate is not a compile error and nothing on the wire notices one:
    /// the router asks each handler in turn and the first to claim a reply wins,
    /// so the *other* window silently stops answering. That is how the guild
    /// window took the craft window's `0x0052_0001` — caught by the craft tests
    /// going red, which is luck, not a check.
    #[test]
    fn all_gump_ids_are_distinct() {
        for (i, id) in super::id::ALL.iter().enumerate() {
            let clash = super::id::ALL[..i].iter().position(|other| other == id);
            assert!(clash.is_none(), "{id:?} appears twice in the table");
        }
    }

    use super::*;
    use crate::packet::{decode_packet, encode_packet};

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    #[test]
    fn a_built_element_matches_the_string_it_replaces() {
        // The exact bytes the admin gump has been hand-writing, from the builder.
        let mut layout = GumpLayout::new();
        layout.background(0, 0, 200, 200, 5054);
        layout.button(10, 10, 4005, 4007, GumpButton::Reply, 0, ButtonId(1));
        assert_eq!(
            layout.finish().0,
            "{ resizepic 0 0 5054 200 200 }{ button 10 10 4005 4007 1 0 1 }"
        );
    }

    #[test]
    fn a_negative_coordinate_stays_negative() {
        // The quest frame puts an element at x = -16. Formatting it through an
        // unsigned type would send 4294967280 and the client would drop the whole
        // layout, which looks like a gump that simply never opens.
        let mut layout = GumpLayout::new();
        layout.image(-16, 285, 0x28A2);
        assert_eq!(layout.finish().0, "{ gumppic -16 285 10402 }");
    }

    #[test]
    fn a_label_interns_its_text_and_names_the_index() {
        let mut layout = GumpLayout::new();
        layout.label(20, 18, 1153, "Quest Log");
        layout.label(20, 40, 1153, "A Plague of Rats");
        let (string, lines) = layout.finish();
        assert_eq!(string, "{ text 20 18 1153 0 }{ text 20 40 1153 1 }");
        assert_eq!(lines, ["Quest Log", "A Plague of Rats"]);
    }

    #[test]
    fn window_flags_come_before_the_elements() {
        let mut layout = GumpLayout::new();
        layout.no_close();
        layout.no_resize();
        layout.page(0);
        assert_eq!(layout.finish().0, "{ noclose }{ noresize }{ page 0 }");
    }

    #[test]
    fn a_hued_image_carries_its_hue_as_a_suffix() {
        // `hue=` is parsed differently from a positional argument; a plain fifth
        // number is silently ignored.
        let mut layout = GumpLayout::new();
        layout.image_hued(5, 6, 100, 33);
        assert_eq!(layout.finish().0, "{ gumppic 5 6 100 hue=33 }");
    }

    #[test]
    fn coloured_html_wraps_the_text_in_a_basefont_tag() {
        let mut layout = GumpLayout::new();
        layout.html_colored(
            98,
            156,
            312,
            180,
            "Slay five rats.",
            GUMP_LIGHT_GREEN,
            false,
            true,
        );
        let (string, lines) = layout.finish();
        assert_eq!(string, "{ htmlgump 98 156 312 180 0 0 1 }");
        assert_eq!(lines, ["<BASEFONT COLOR=#B8E080>Slay five rats.</BASEFONT>"]);
    }

    #[test]
    fn a_localized_line_with_arguments_ends_in_its_argument_run() {
        let mut layout = GumpLayout::new();
        layout.html_localized_args(
            98,
            140,
            312,
            16,
            ClilocId(1_074_782),
            "Britain",
            GUMP_WHITE,
            false,
            false,
        );
        assert_eq!(
            layout.finish().0,
            "{ xmfhtmltok 98 140 312 16 0 0 32767 1074782 @Britain@ }"
        );
    }

    #[test]
    fn a_close_gump_names_its_dialog_and_button() {
        let bytes = encode_packet(
            &CloseGump {
                gump_id: GumpId(0x0051_0001),
                button: ButtonId::CLOSE_BOX,
            },
            version(),
        );
        assert_eq!(bytes.len(), 13, "ServUO's EnsureCapacity(13)");
        assert_eq!(bytes[0], 0xBF);
        assert_eq!(u16::from_be_bytes([bytes[1], bytes[2]]), 13);
        assert_eq!(u16::from_be_bytes([bytes[3], bytes[4]]), 0x04);
        assert_eq!(
            u32::from_be_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]),
            0x0051_0001
        );
    }

    #[test]
    fn a_built_layout_round_trips_through_the_display_packet() {
        let mut layout = GumpLayout::new();
        layout.background(0, 0, 340, 220, 5054);
        layout.label(20, 18, 1153, "A Plague of Rats");
        let (string, lines) = layout.finish();
        let bytes = encode_packet(
            &GumpDisplay {
                serial: GumpKey::STANDALONE,
                gump_id: GumpId(0x0051_0001),
                at: GumpPoint::new(75, 25),
                layout: string.to_owned(),
                lines: lines.to_vec(),
            },
            version(),
        );
        let declared = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
        assert_eq!(declared, bytes.len());
    }

    #[test]
    fn a_radio_reply_carries_its_switches() {
        // The resign dialog is radios plus one OK button: without the switch ids
        // the answer is "OK" with no idea what was chosen.
        let mut layout = GumpLayout::new();
        layout.radio(100, 100, 0x25F8, 0x25FB, false, SwitchId(0));
        layout.radio(100, 120, 0x25F8, 0x25FB, true, SwitchId(1));
        assert_eq!(
            layout.finish().0,
            "{ radio 100 100 9720 9723 0 0 }{ radio 100 120 9720 9723 1 1 }"
        );

        let mut p = vec![0xB1u8, 0, 0];
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(&0x0051_0002u32.to_be_bytes());
        p.extend_from_slice(&1u32.to_be_bytes()); // the OK button
        p.extend_from_slice(&1u32.to_be_bytes()); // one switch on
        p.extend_from_slice(&1u32.to_be_bytes()); // ...its id
        p.extend_from_slice(&0u32.to_be_bytes()); // no text
        let len = u16::try_from(p.len()).unwrap();
        p[1..3].copy_from_slice(&len.to_be_bytes());

        let got: GumpResponse = decode_packet(&p, version()).unwrap();
        assert_eq!(got.button.interpret(), GumpAnswer::Pressed(ButtonId(1)));
        assert_eq!(got.switches, vec![RawSwitchId(1)], "which radio was set");
    }

    #[test]
    fn a_gump_declares_its_own_length_and_id() {
        let bytes = encode_packet(
            &GumpDisplay {
                serial: GumpKey::STANDALONE,
                gump_id: GumpId(0x00AD_0001),
                at: GumpPoint::new(50, 40),
                layout: "{ resizepic 0 0 5054 200 200 }{ button 10 10 4005 4007 1 0 1 }".to_owned(),
                lines: vec!["Spawn".to_owned()],
            },
            version(),
        );
        assert_eq!(bytes[0], 0xB0);
        let declared = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
        assert_eq!(declared, bytes.len(), "the length word must match the bytes");
        assert_eq!(
            u32::from_be_bytes([bytes[7], bytes[8], bytes[9], bytes[10]]),
            0x00AD_0001,
            "the gump id round-trips"
        );
    }

    #[test]
    fn a_line_rides_as_big_endian_utf16() {
        // "Ok" is two code units; each is a big-endian u16.
        let bytes = encode_packet(
            &GumpDisplay {
                serial: GumpKey::STANDALONE,
                gump_id: GumpId(1),
                at: GumpPoint::new(0, 0),
                layout: "{ page 0 }".to_owned(),
                lines: vec!["Ok".to_owned()],
            },
            version(),
        );
        // Find the text table: it is the tail, `count(2) len(2) 'O'(2) 'k'(2)`.
        let tail = &bytes[bytes.len() - 8..];
        assert_eq!(u16::from_be_bytes([tail[0], tail[1]]), 1, "one line");
        assert_eq!(u16::from_be_bytes([tail[2], tail[3]]), 2, "two chars");
        assert_eq!(u16::from_be_bytes([tail[4], tail[5]]), b'O' as u16);
        assert_eq!(u16::from_be_bytes([tail[6], tail[7]]), b'k' as u16);
    }

    /// The two halves this engine had never needed until it grew a client of its
    /// own: the window out and the answer back, each read by the end that did
    /// not write it. A field written in the wrong width desynchronises the rest
    /// of the packet rather than failing, so the assertion is on the whole
    /// value, not on a byte.
    #[test]
    fn a_window_the_server_drew_is_the_window_the_client_reads() {
        let sent = GumpDisplay {
            serial: GumpKey(0x1234),
            gump_id: GumpId(0x00AD_0001),
            at: GumpPoint::new(100, 100),
            // A negative coordinate on purpose: it rides as unsigned and has to
            // come back signed — see `GumpPoint`.
            layout: "{ resizepic -16 0 5054 300 270 }{ text 105 14 2100 0 }".to_owned(),
            lines: vec!["Admin".to_owned(), "Populate Felucca".to_owned()],
        };
        let bytes = encode_packet(&sent, version());
        let read: GumpDisplay = decode_server_packet(&bytes, version());
        assert_eq!(read, sent);
    }

    #[test]
    fn the_answer_a_client_encodes_is_the_answer_the_server_decodes() {
        let sent = GumpResponse {
            serial: RawGumpKey(0x1234),
            gump_id: RawGumpId(0x00AD_0001),
            button: RawButtonId(13),
            switches: vec![RawSwitchId(7), RawSwitchId(9)],
            text_entries: vec![(2, "Hi".to_owned())],
        };
        let bytes = sent.encode();
        assert_eq!(bytes[0], 0xB1);
        assert_eq!(
            u16::from_be_bytes([bytes[1], bytes[2]]) as usize,
            bytes.len(),
            "the length field counts the whole packet"
        );
        let read: GumpResponse = decode_packet(&bytes, version()).unwrap();
        assert_eq!(read, sent);
    }

    /// Decode a packet the *server* sent, the way a client does: the length
    /// table is the other one, so `decode_packet` — which asks the client table
    /// — would not know to skip `0xB0`'s length field.
    fn decode_server_packet(bytes: &[u8], version: ClientVersion) -> GumpDisplay {
        match crate::server_packet::ServerPacket::decode(bytes, version) {
            Ok(Some(crate::server_packet::ServerPacket::GumpDisplay(gump))) => gump,
            other => panic!("0xB0 did not decode as one: {other:?}"),
        }
    }

    #[test]
    fn a_response_reads_the_button_and_its_fields() {
        // 0xB1: len, serial, gumpId, button=3, 1 switch (id 7), 1 text (id 2, "Hi").
        let mut p = vec![0xB1u8, 0, 0];
        p.extend_from_slice(&0x1234u32.to_be_bytes());
        p.extend_from_slice(&0x00AD_0001u32.to_be_bytes());
        p.extend_from_slice(&3u32.to_be_bytes());
        p.extend_from_slice(&1u32.to_be_bytes()); // switch count
        p.extend_from_slice(&7u32.to_be_bytes()); // switch id
        p.extend_from_slice(&1u32.to_be_bytes()); // text count
        p.extend_from_slice(&2u16.to_be_bytes()); // text id
        p.extend_from_slice(&2u16.to_be_bytes()); // char count
        p.extend_from_slice(&(b'H' as u16).to_be_bytes());
        p.extend_from_slice(&(b'i' as u16).to_be_bytes());
        let len = u16::try_from(p.len()).unwrap();
        p[1..3].copy_from_slice(&len.to_be_bytes());

        let got: GumpResponse = decode_packet(&p, version()).unwrap();
        assert_eq!(got.serial, RawGumpKey(0x1234));
        assert_eq!(got.gump_id, RawGumpId(0x00AD_0001));
        assert_eq!(got.button, RawButtonId(3));
        assert_eq!(got.switches, vec![RawSwitchId(7)]);
        assert_eq!(got.text_entries, vec![(2, "Hi".to_owned())]);
    }

    #[test]
    fn a_closed_gump_is_button_zero() {
        let mut p = vec![0xB1u8, 0, 0];
        p.extend_from_slice(&0u32.to_be_bytes()); // serial
        p.extend_from_slice(&5u32.to_be_bytes()); // gumpId
        p.extend_from_slice(&0u32.to_be_bytes()); // button 0 = closed
        p.extend_from_slice(&0u32.to_be_bytes()); // no switches
        p.extend_from_slice(&0u32.to_be_bytes()); // no text
        let len = u16::try_from(p.len()).unwrap();
        p[1..3].copy_from_slice(&len.to_be_bytes());

        let got: GumpResponse = decode_packet(&p, version()).unwrap();
        assert_eq!(got.button.interpret(), GumpAnswer::Closed, "the close box");
        assert!(got.switches.is_empty());
    }

    #[test]
    fn every_button_a_client_can_send_interprets() {
        // Class B is total or it is not class B. `0` is the close box and
        // everything else is an id; there is no third answer, including at the
        // ends.
        assert_eq!(RawButtonId(0).interpret(), GumpAnswer::Closed);
        assert_eq!(RawButtonId(1).interpret(), GumpAnswer::Pressed(ButtonId(1)));
        assert_eq!(
            RawButtonId(u32::MAX).interpret(),
            GumpAnswer::Pressed(ButtonId(u32::MAX)),
            "a button id no layout drew is still a press; whether it was offered \
             is the handler's match, not this"
        );
    }

    #[test]
    fn a_reply_for_a_dialog_this_side_never_drew_names_none_of_them() {
        // N9's pair for the gump id. The forged reply decodes — a `0xB1` is a
        // well-formed packet whichever number it carries — and then names no
        // dialog the asking handler offered.
        let quest = GumpId(0x0051_0001);
        let resign = GumpId(0x0051_0002);
        assert_eq!(RawGumpId(0x0051_0002).validate(&[quest, resign]), Some(resign));
        assert_eq!(
            RawGumpId(0x00AD_0001).validate(&[quest, resign]),
            None,
            "the admin menu's id is not the quest system's to answer"
        );
        assert_eq!(RawGumpId(0).validate(&[quest]), None);
    }

    #[test]
    fn a_switch_no_group_offered_decodes_cleanly_and_is_refused() {
        // N9's pair for the switches. A `0xB1` claiming radio 9 of a
        // three-destination list must not select destination 9 — nor
        // destination 2, which is what clamping would do.
        let mut p = vec![0xB1u8, 0, 0];
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(&5u32.to_be_bytes());
        p.extend_from_slice(&1u32.to_be_bytes()); // a reply button
        p.extend_from_slice(&1u32.to_be_bytes()); // one switch on
        p.extend_from_slice(&9u32.to_be_bytes()); // ...id 9
        p.extend_from_slice(&0u32.to_be_bytes()); // no text
        let len = u16::try_from(p.len()).unwrap();
        p[1..3].copy_from_slice(&len.to_be_bytes());

        let got: GumpResponse = decode_packet(&p, version()).unwrap();
        assert_eq!(got.switches, vec![RawSwitchId(9)], "the id arrives intact");
        assert_eq!(
            got.switches[0].validate(3),
            Err(InvalidSwitchId { id: 9, offered: 3 })
        );
        assert_eq!(
            RawSwitchId(2).validate(3),
            Ok(SwitchId(2)),
            "the last row of three"
        );
    }
}
