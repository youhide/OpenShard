//! The server's dialogs, drawn with egui.
//!
//! A gump is UO's windowing primitive, and the shard sends one as a *layout* —
//! a list of elements at pixel coordinates, keyed to art in the client's files.
//! `.admin` opens one; so does a shopkeeper, a quest and a book.
//!
//! # What this draws, and what it deliberately does not
//!
//! It draws the layout's *structure* with egui's own widgets: a frame is a
//! window, a `{ button }` is a button, a `{ text }` is a label at the
//! coordinate the server asked for. It does **not** draw the gump art those
//! elements name — no `gumpart.mul` reader exists yet, and inventing one here
//! would decide how this client renders art before the renderer has an opinion.
//! An element whose whole content is a picture (`gumppic`, `tilepic`) is drawn
//! as a placeholder naming the graphic, which is honest and, for a menu of
//! labelled buttons like `.admin`'s, loses nothing: the words are in the text
//! table and the words are what the buttons mean.
//!
//! So this is the dev-HUD rendering of a gump, in the same spirit as the rest
//! of `shell.rs` — enough to use the shard's dialogs and enough to tell a
//! protocol bug from a drawing one. What the real interface is remains M4's
//! decision, see `docs/client.md`.
//!
//! # One gump pixel is one egui point, deliberately
//!
//! A layout's coordinates are the reference client's *pixels* — it draws gump
//! art one for one and has no display scaling at all, so on a 4K screen its
//! windows are postage stamps. egui lays out in logical *points*, which is a
//! different space on any display whose scale factor is not 1. This module
//! reads a layout coordinate as a point, and that is a decision rather than an
//! oversight:
//!
//! - What is drawn here is egui's widgets, and a widget's size — its text, its
//!   padding, the box a checkbox needs — is measured in points. Scaling the
//!   *coordinates* to physical pixels while the widgets kept their point sizes
//!   would pull the rows together underneath text that did not shrink with
//!   them, which is worse than a window that is merely large.
//! - The reference client is not the authority on this. It predates display
//!   scaling entirely; "what it does" here is "nothing", and copying that gives
//!   an unreadable window on the hardware people have.
//!
//! It stops being right the day gump *art* is drawn, because art is a bitmap of
//! a fixed pixel size and cannot be reinterpreted. At that point the window
//! wants one scale applied to the coordinates **and** to the font sizes
//! together, and the two places to change are [`point`] and [`size`] — every
//! number a layout carries passes through one of them, which is why they exist
//! as functions returning what they are given.
//!
//! # Two things the wire does not say
//!
//! 1. **A window closes when it is answered.** No packet says so — the server
//!    sends one `0xB0`, waits for one `0xB1`, and both ends assume the client
//!    took the window down. That is [`WorldView::gump_closed`], called by the
//!    caller once the reply is on its way.
//! 2. **A page button never reaches the server.** `{ button ... 0 N id }` flips
//!    to page `N` inside the client, and only a reply button answers. So the
//!    current page is state that lives here and nowhere else — which is why
//!    this module holds any state at all.
//!
//! [`WorldView::gump_closed`]: openshard_client_net::view::WorldView::gump_closed

use std::collections::HashMap;

use openshard_client_net::view::OpenGump;
use openshard_protocol::gump::layout::{Element, Flag, Switch};
use openshard_protocol::gump::{GumpButton, RawButtonId, RawGumpId, RawGumpKey, RawSwitchId};
use openshard_protocol::wire::Hue;
use openshard_uofiles::hues::Hues;

use crate::link::GumpReply;

/// Which of a hue's 32-step ramp a `{ text }` element's colour is read from.
///
/// A hue applied to *art* retints every pixel through the whole ramp — see
/// `client/render`'s `hue.rs` — because a sprite's own grey level is the
/// column to read. A gump label has no such per-pixel input: the glyphs egui
/// draws are not `fonts.mul`'s bitmaps, so there is nothing here to regrade
/// pixel by pixel, and one solid colour is wanted instead.
///
/// `8` is not a guess: it is [ClassicUO's
/// `HuesLoader.GetUnicodeFontColor`](https://github.com/ClassicUO/ClassicUO/blob/main/src/ClassicUO.Assets/HuesLoader.cs),
/// the reference client's own answer to "what solid colour does this hue mean
/// for a piece of text" —
/// `HuesRange[g].Entries[e].ColorTable[8]`, verbatim. That function exists
/// beside `GetColor`, which instead does `ColorTable[(c >> 10) & 0x1F]` to
/// regrade an *already-coloured* glyph pixel — the ASCII font's own bitmap
/// colour reused as the ramp index, the same trick statics are tinted with.
/// Unicode-font text (what a gump draws through) has no such per-pixel colour
/// to reuse, which is why it is answered by a fixed column instead.
const TEXT_RAMP_STEP: usize = 8;

/// The colour a `{ text }` or `{ croppedtext }` element's `hue` draws in, or
/// `None` for "the layout asked for none".
///
/// `Hue(0)` is the server's "no colour", not `hues.mul`'s first row — see
/// `Hues::get`'s docs — and that is a caller's cue to draw the surrounding
/// theme's own text colour, not a lookup that merely came back empty. A hue
/// past the end of the table (out of range, or corrupt data) answers the same
/// way, for the same reason `Hues::get` does: a bad hue index off the wire is
/// a thing to draw plainly, not a thing to crash the dialog over.
fn text_color(hues: &Hues, hue: u32) -> Option<egui::Color32> {
    let entry = hues.get(Hue(hue as u16))?;
    let (r, g, b) = entry.colors[TEXT_RAMP_STEP].rgb8();
    Some(egui::Color32::from_rgb(r, g, b))
}

/// How wide a label is allowed to run when the layout gives it no box.
///
/// `{ text }` carries no width — the client measures the string in the font it
/// draws it in, and this end draws in a different font — so a bound is picked
/// here. Generous, because the failure it prevents is a label wrapping into the
/// row below it, and the failure it causes is a label overlapping one to its
/// right, which is the less confusing of the two.
const LABEL_WIDTH: f32 = 220.0;

/// How tall a row of text is, for the same reason.
const LABEL_HEIGHT: f32 = 18.0;

/// The size a button is drawn at, in place of the art it names.
const BUTTON_SIZE: egui::Vec2 = egui::vec2(26.0, 20.0);

/// One coordinate out of a layout, in the space this module draws in.
///
/// The identity, and the module docs are why: a gump pixel is an egui point
/// here. It is a function rather than a cast at each call site so that the day
/// that stops being true has one place to change instead of thirty.
fn point(value: i32) -> f32 {
    value as f32
}

/// The same for a width and a height, which always travel together.
fn size(width: i32, height: i32) -> egui::Vec2 {
    egui::vec2(point(width), point(height))
}

/// What a window has that its layout does not: which page it is showing, and
/// what the player has set on it.
///
/// Keyed by dialog id rather than by index: the list of open windows is rebuilt
/// from the [`WorldView`](openshard_client_net::view::WorldView) every frame, so
/// a position in it is not an identity, and a redrawn dialog would otherwise
/// silently inherit the state of whatever now stands where it used to.
#[derive(Default)]
pub struct Windows {
    by_dialog: HashMap<u32, Sheet>,
    /// Real pixels per egui point, as of the last frame drawn.
    ///
    /// Recorded rather than asked for again, because the art pass has to be
    /// handed *the same number egui laid out with* — a gump pixel is an egui
    /// point here, so any disagreement between the two is a window whose art
    /// slides off its buttons. Zero before the first frame, which is what
    /// [`Windows::placement`] returning `None` protects.
    pixels_per_point: f32,
}

/// Where a window's art goes, and the state it is drawn in.
///
/// What [`Windows`] hands the art pass every frame — see
/// [`Windows::placement`]. All three fields are things no packet carries: the
/// position is egui's (the player may have dragged the window), the page is a
/// click this client answered itself, and the switches are what the player has
/// set since the layout arrived.
pub struct Placement {
    /// The window's content origin in egui points, which is also its origin in
    /// gump pixels — the two spaces are one here, and [`Windows::scale`] is
    /// what keeps them that way.
    pub at: (i32, i32),
    /// The page showing.
    pub page: u32,
    /// Which switches are on.
    pub on: std::collections::BTreeSet<RawSwitchId>,
}

/// One window's answers-in-progress.
#[derive(Default)]
struct Sheet {
    /// The page being shown. Page `0` is drawn on every page, so this starts at
    /// zero and a layout with no pages at all never leaves it.
    page: u32,
    /// Which switches are on, by their id. Seeded from the layout's own
    /// `initial` flags the first time an id is seen, and the player's after
    /// that — which is why absence and `false` are different here.
    switches: HashMap<u32, bool>,
    /// What has been typed into each field, by its id.
    entries: HashMap<u16, String>,
    /// Where this window's *content* ended up on the screen last frame, in egui
    /// points, or `None` before it has been drawn once.
    ///
    /// The seam between the two halves of a gump. The art is drawn by
    /// `client/render`'s pass and the widgets by egui, and what makes them land
    /// on each other is that the art is placed here — at the rectangle egui
    /// allocated — rather than at the coordinate the server asked for. Which
    /// means a dragged window drags its art with it, and neither half has to
    /// know how egui decorates a frame.
    origin: Option<(f32, f32)>,
}

impl Windows {
    /// Where a window's art goes and what state it is in, or `None` for a
    /// window egui has not laid out yet — which is every window on the frame
    /// its packet arrived, and exactly the frame there is nowhere to put it.
    ///
    /// The two pieces of state this module holds that no packet carries — see
    /// the module docs — handed out so that
    /// [`openshard_client_render::gump::window`] can lay the same window out
    /// through the client's own files. Both ends read one copy: a second page
    /// counter beside this one would drift the moment a page button was pressed
    /// and only one of them heard about it.
    ///
    /// A window this has never seen is on page 0 with nothing set, which is
    /// exactly what a layout that has just arrived means.
    pub fn placement(&self, gump_id: u32) -> Option<Placement> {
        let sheet = self.by_dialog.get(&gump_id)?;
        let (x, y) = sheet.origin?;
        Some(Placement {
            at: (x.round() as i32, y.round() as i32),
            page: sheet.page,
            on: sheet
                .switches
                .iter()
                .filter(|(_, set)| **set)
                .map(|(id, _)| RawSwitchId(*id))
                .collect(),
        })
    }

    /// Real pixels per point, for whoever draws this window's art at the same
    /// scale egui laid its widgets out at. See [`Windows::pixels_per_point`].
    pub fn scale(&self) -> f32 {
        self.pixels_per_point
    }

    /// Draw every open dialog, and hand back the one answer this frame produced.
    ///
    /// At most one: a reply closes its window, and a player cannot press two
    /// buttons in one frame. The caller sends it and calls
    /// [`WorldView::gump_closed`](openshard_client_net::view::WorldView::gump_closed)
    /// — see the module docs for why that is this end's job.
    pub fn show(&mut self, context: &egui::Context, open: &[OpenGump], hues: &Hues) -> Option<GumpReply> {
        self.pixels_per_point = context.pixels_per_point();
        // The state of a window the server has taken away is not worth keeping:
        // a dialog that comes back comes back as the server drew it.
        self.by_dialog
            .retain(|id, _| open.iter().any(|gump| gump.gump_id.0 == *id));

        let mut answer = None;
        for gump in open {
            if let Some(reply) = self.show_one(context, gump, hues) {
                answer = Some(reply);
            }
        }
        answer
    }

    /// One window.
    fn show_one(&mut self, context: &egui::Context, gump: &OpenGump, hues: &Hues) -> Option<GumpReply> {
        let sheet = self.by_dialog.entry(gump.gump_id.0).or_default();
        seed_switches(sheet, gump);

        let flags = window_flags(gump);
        // A window with no close box has to be answered by one of its buttons —
        // that is what `{ noclose }` means, and honouring it is the difference
        // between a dialog a shard can rely on and one a player can walk away
        // from.
        let mut still_open = true;
        let mut answer = None;

        // A transparent frame, and this is not cosmetic: the art pass draws
        // before egui does, so an opaque window background would be painted
        // *over* the pictures it is supposed to be behind and the window would
        // look exactly as it did before any of this existed. The shadow goes
        // for the same reason — it is drawn under the frame and over the art.
        // What is kept is the title bar and the close box, because until this
        // client owns the mouse over a gump they are the only way to move or
        // dismiss one.
        let mut frame = egui::Frame::window(&context.style_of(context.theme()));
        frame.fill = egui::Color32::TRANSPARENT;
        frame.shadow = egui::epaint::Shadow::NONE;

        let mut window = egui::Window::new(title(gump))
            .frame(frame)
            // The title is the layout's first label, which two dialogs may
            // share; the dialog id is what actually distinguishes them.
            .id(egui::Id::new(("gump", gump.gump_id.0)))
            // Where on the screen the server asked for it, in the same space as
            // everything inside it — see the module docs.
            .default_pos([point(gump.at.x), point(gump.at.y)])
            .movable(!flags.no_move)
            .resizable(false)
            .collapsible(false);
        if !flags.no_close {
            window = window.open(&mut still_open);
        }
        window.show(context, |ui| {
            answer = draw(ui, gump, sheet, hues);
        });

        // The close box: an answer of button zero, and the shard is waiting for
        // it exactly as much as it is waiting for a real button.
        if !still_open && answer.is_none() {
            answer = Some(RawButtonId(0));
        }
        answer.map(|button| reply(gump, sheet, button))
    }
}

/// What the window flags in a layout ask for.
#[derive(Default)]
struct WindowFlags {
    no_move: bool,
    no_close: bool,
}

/// Read the flags out of a layout. `{ nodispose }` and `{ noresize }` have
/// nothing to honour here: this window is never resizable, and there is no
/// right-click dismissal to suppress.
fn window_flags(gump: &OpenGump) -> WindowFlags {
    let mut flags = WindowFlags::default();
    for element in &gump.elements {
        match element {
            Element::Flag(Flag::NoMove) => flags.no_move = true,
            Element::Flag(Flag::NoClose) => flags.no_close = true,
            _ => {}
        }
    }
    flags
}

/// What to call the window: its first label, which is where every dialog this
/// engine draws puts its heading, or the dialog id when it has none.
fn title(gump: &OpenGump) -> String {
    gump.elements
        .iter()
        .find_map(|element| match element {
            Element::Label { line, .. } | Element::CroppedLabel { line, .. } => gump.line(*line),
            _ => None,
        })
        .map_or_else(|| format!("gump 0x{:08X}", gump.gump_id.0), str::to_owned)
}

/// Give every switch a starting value the first time the window is drawn.
///
/// Only the first time: after that the map holds what the *player* set, and
/// re-seeding would drag a checkbox back under their finger every frame.
fn seed_switches(sheet: &mut Sheet, gump: &OpenGump) {
    for element in &gump.elements {
        if let Element::Check(switch) | Element::Radio(switch) = element {
            sheet.switches.entry(switch.id.0).or_insert(switch.initial);
        }
    }
}

/// Lay the elements out at the coordinates the server gave them, and answer with
/// the button pressed, if one was.
fn draw(ui: &mut egui::Ui, gump: &OpenGump, sheet: &mut Sheet, hues: &Hues) -> Option<RawButtonId> {
    let content = content_size(gump);
    let (rect, _) = ui.allocate_exact_size(content, egui::Sense::hover());
    let origin = rect.min;
    // Where the art goes this frame — see `Sheet::origin`. Written every frame
    // rather than once, because egui's own window is draggable and the art has
    // to follow it.
    sheet.origin = Some((origin.x, origin.y));
    let at = |x: i32, y: i32, size: egui::Vec2| {
        egui::Rect::from_min_size(origin + egui::vec2(point(x), point(y)), size)
    };

    let mut pressed = None;
    // Which page the elements that follow belong to. Zero until the layout says
    // otherwise, and page zero is drawn whatever page the window is on.
    let mut page = 0;
    for element in &gump.elements {
        if let Element::Page(number) = element {
            page = *number;
            continue;
        }
        if page != 0 && page != sheet.page {
            continue;
        }
        match element {
            Element::Page(_) => unreachable!("handled above"),
            Element::Flag(_) => {}
            // The frame the window already is: egui has drawn it, and painting a
            // second rectangle over it would only hide the elements inside.
            Element::Background { .. } => {}
            Element::AlphaRegion { x, y, width, height } => {
                let region = at(*x, *y, size(*width, *height));
                ui.painter()
                    .rect_filled(region, 2.0, egui::Color32::from_black_alpha(96));
            }
            Element::Button {
                x,
                y,
                kind,
                page: target,
                id,
                ..
            } => {
                // Transparent and unlabelled: the button's picture is drawn
                // by the art pass underneath, and what is left for egui is the
                // click. A filled widget here would hide the art it is
                // standing on.
                let face = egui::Button::new("")
                    .fill(egui::Color32::TRANSPARENT)
                    .stroke(egui::Stroke::NONE);
                if ui.put(at(*x, *y, BUTTON_SIZE), face).clicked() {
                    match kind {
                        // Client-side, and the reason this module holds state:
                        // no packet is sent and the server never learns the
                        // player turned a page.
                        GumpButton::Page => sheet.page = *target,
                        GumpButton::Reply => pressed = Some(*id),
                    }
                }
            }
            Element::Check(switch) => draw_switch(ui, sheet, switch, at(switch.x, switch.y, BUTTON_SIZE)),
            Element::Radio(switch) => {
                let was = sheet
                    .switches
                    .get(&switch.id.0)
                    .copied()
                    .unwrap_or(switch.initial);
                draw_switch(ui, sheet, switch, at(switch.x, switch.y, BUTTON_SIZE));
                // A radio turns its neighbours off — that is the whole
                // difference from a checkbox, and the client is what enforces
                // it: the server is sent the ids that are left on and trusts
                // that only one of a group is.
                let now = sheet.switches.get(&switch.id.0).copied().unwrap_or(false);
                if now && !was {
                    let group: Vec<u32> = gump
                        .elements
                        .iter()
                        .filter_map(|other| match other {
                            Element::Radio(other) if other.id != switch.id => Some(other.id.0),
                            _ => None,
                        })
                        .collect();
                    for id in group {
                        sheet.switches.insert(id, false);
                    }
                }
            }
            Element::Label { x, y, line, hue, .. } => {
                let text = gump.line(*line).unwrap_or("<no such line>");
                // `None` — the layout carried no hue, or asked for `Hue(0)` —
                // leaves the `RichText` uncoloured, which is egui's own text
                // colour for the current theme rather than a colour this
                // module invented.
                let mut rich = egui::RichText::new(text);
                if let Some(color) = text_color(hues, *hue) {
                    rich = rich.color(color);
                }
                ui.put(
                    at(*x, *y, egui::vec2(LABEL_WIDTH, LABEL_HEIGHT)),
                    egui::Label::new(rich).halign(egui::Align::LEFT).extend(),
                );
            }
            Element::CroppedLabel {
                x,
                y,
                width,
                height,
                line,
                hue,
                ..
            } => {
                let text = gump.line(*line).unwrap_or("<no such line>");
                let mut rich = egui::RichText::new(text);
                if let Some(color) = text_color(hues, *hue) {
                    rich = rich.color(color);
                }
                ui.put(
                    at(*x, *y, size(*width, *height)),
                    egui::Label::new(rich).halign(egui::Align::LEFT).truncate(),
                );
            }
            Element::Html {
                x,
                y,
                width,
                height,
                line,
                ..
            } => {
                // The tags are left in. Stripping them would be a second, worse
                // HTML parser beside the client's own, and what this is for is
                // reading what the shard sent.
                let text = gump.line(*line).unwrap_or("<no such line>");
                ui.put(
                    at(*x, *y, size(*width, *height)),
                    egui::Label::new(text).halign(egui::Align::LEFT),
                );
            }
            Element::TextEntry {
                x,
                y,
                width,
                height,
                entry_id,
                line,
                ..
            } => {
                let field = sheet
                    .entries
                    .entry(*entry_id)
                    .or_insert_with(|| gump.line(*line).unwrap_or_default().to_owned());
                ui.put(
                    at(*x, *y, size(*width, *height)),
                    egui::TextEdit::singleline(field),
                );
            }
            // Gump art, drawn by the pass underneath this window and not
            // here — see `Sheet::origin`. Nothing is allocated for them: they
            // are already accounted for in `content_size`, which is what
            // reserves the room the art needs.
            Element::Image { .. } | Element::ImageTiled { .. } => {}
            Element::Item { x, y, graphic, .. } => {
                placeholder(ui, at(*x, *y, BUTTON_SIZE), &format!("[{graphic}]"));
            }
            Element::Localized {
                x,
                y,
                width,
                height,
                cliloc,
            } => placeholder(
                ui,
                at(*x, *y, size(*width, *height)),
                // The string lives in the client's own `cliloc` file, which
                // nothing here reads yet: the number is all there is to show.
                &format!("cliloc {cliloc}"),
            ),
            Element::Unknown { keyword } => {
                // Drawn nowhere in particular — an element that did not parse
                // has no coordinates to trust. Reported rather than dropped, so
                // a window that is missing a row says which one.
                ui.painter().text(
                    rect.left_bottom(),
                    egui::Align2::LEFT_BOTTOM,
                    format!("unhandled: {keyword}"),
                    egui::FontId::monospace(10.0),
                    ui.visuals().weak_text_color(),
                );
            }
        }
    }
    pressed
}

/// A checkbox, and the map entry it reads and writes.
fn draw_switch(ui: &mut egui::Ui, sheet: &mut Sheet, switch: &Switch, rect: egui::Rect) {
    let mut set = sheet
        .switches
        .get(&switch.id.0)
        .copied()
        .unwrap_or(switch.initial);
    if ui.put(rect, egui::Checkbox::without_text(&mut set)).changed() {
        sheet.switches.insert(switch.id.0, set);
    }
}

/// An element this client has no picture for, drawn as what it names.
fn placeholder(ui: &egui::Ui, rect: egui::Rect, label: &str) {
    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(9.0),
        ui.visuals().weak_text_color(),
    );
}

/// How big the window's content is: the furthest corner any element reaches.
///
/// Read from the elements rather than from the background frame alone, because
/// a layout is free to draw outside its own frame and several do — and a
/// content rect that is too small clips the elements that hang over it.
fn content_size(gump: &OpenGump) -> egui::Vec2 {
    let mut width = 0.0f32;
    let mut height = 0.0f32;
    // `w` and `h` are already in this module's space — some come out of a
    // layout through `size`, some are this module's own constants — so only the
    // position is converted here.
    let mut extend = |x: i32, y: i32, w: f32, h: f32| {
        width = width.max(point(x) + w);
        height = height.max(point(y) + h);
    };
    for element in &gump.elements {
        match element {
            Element::Background {
                x,
                y,
                width: w,
                height: h,
                ..
            }
            | Element::ImageTiled {
                x,
                y,
                width: w,
                height: h,
                ..
            }
            | Element::AlphaRegion {
                x,
                y,
                width: w,
                height: h,
            }
            | Element::CroppedLabel {
                x,
                y,
                width: w,
                height: h,
                ..
            }
            | Element::Html {
                x,
                y,
                width: w,
                height: h,
                ..
            }
            | Element::Localized {
                x,
                y,
                width: w,
                height: h,
                ..
            }
            | Element::TextEntry {
                x,
                y,
                width: w,
                height: h,
                ..
            } => {
                let wanted = size(*w, *h);
                extend(*x, *y, wanted.x, wanted.y);
            }
            Element::Label { x, y, .. } => extend(*x, *y, LABEL_WIDTH, LABEL_HEIGHT),
            Element::Button { x, y, .. } | Element::Image { x, y, .. } | Element::Item { x, y, .. } => {
                extend(*x, *y, BUTTON_SIZE.x, BUTTON_SIZE.y);
            }
            Element::Check(switch) | Element::Radio(switch) => {
                extend(switch.x, switch.y, BUTTON_SIZE.x, BUTTON_SIZE.y);
            }
            Element::Page(_) | Element::Flag(_) | Element::Unknown { .. } => {}
        }
    }
    // A layout with nothing positioned in it still needs somewhere to put the
    // "unhandled" notes, and an empty window is indistinguishable from a hung
    // one.
    egui::vec2(width.max(120.0), height.max(40.0))
}

/// The answer to send: the button, and everything the player set on the way to
/// pressing it.
///
/// The switches are sorted before they are sent. Nothing on the wire requires
/// it — the server reads the ids as a set — but a `HashMap`'s order is not
/// stable between runs, and a packet whose bytes depend on the iteration order
/// of a hash map is one that cannot be compared against a recording.
fn reply(gump: &OpenGump, sheet: &Sheet, button: RawButtonId) -> GumpReply {
    let mut switches: Vec<RawSwitchId> = sheet
        .switches
        .iter()
        .filter(|(_, &set)| set)
        .map(|(&id, _)| RawSwitchId(id))
        .collect();
    switches.sort_unstable();

    let mut text_entries: Vec<(u16, String)> = sheet
        .entries
        .iter()
        .map(|(&id, text)| (id, text.clone()))
        .collect();
    text_entries.sort_unstable_by_key(|(id, _)| *id);

    GumpReply {
        key: RawGumpKey(gump.key.0),
        gump_id: RawGumpId(gump.gump_id.0),
        button,
        switches,
        text_entries,
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::gump::layout::parse;
    use openshard_protocol::gump::{ButtonId, GumpId, GumpKey, GumpLayout, GumpPoint, SwitchId};
    use openshard_uofiles::color::Color16;
    use openshard_uofiles::hues::COLORS_PER_HUE;

    use super::*;

    fn admin_menu() -> OpenGump {
        let mut layout = GumpLayout::new();
        layout.background(0, 0, 300, 270, 5054);
        layout.label(105, 14, 2100, "Admin");
        layout.button(30, 54, 4005, 4007, GumpButton::Reply, 0, ButtonId(13));
        layout.label(66, 56, 1153, "Populate Felucca");
        layout.check(30, 100, 210, 211, false, SwitchId(1));
        let (string, lines) = layout.finish();
        OpenGump {
            key: GumpKey(0x2A),
            gump_id: GumpId(0x00AD_0001),
            at: GumpPoint::new(100, 100),
            elements: parse(string),
            lines: lines.to_vec(),
        }
    }

    /// The window takes its name from the heading the layout draws, which is
    /// what makes two open dialogs tellable apart on screen. The id is what
    /// keeps them apart in egui — see `show_one`.
    #[test]
    fn a_window_is_named_by_its_own_heading() {
        assert_eq!(title(&admin_menu()), "Admin");
    }

    /// The frame the server asked for is the size the window has to be: an
    /// element hanging outside a content rect is an element the player cannot
    /// click.
    #[test]
    fn the_content_is_as_big_as_the_frame_the_server_drew() {
        let size = content_size(&admin_menu());
        assert!(
            size.x >= 300.0 && size.y >= 270.0,
            "the 300x270 frame fits: {size:?}"
        );
    }

    /// What travels back. The two things worth pinning: only the switches that
    /// are *on* are sent, and they are sent in a stable order — a `HashMap`'s
    /// own order is not one.
    #[test]
    fn an_answer_carries_the_switches_that_are_set_and_nothing_else() {
        let gump = admin_menu();
        let mut sheet = Sheet::default();
        sheet.switches.insert(9, true);
        sheet.switches.insert(1, false);
        sheet.switches.insert(4, true);

        let reply = reply(&gump, &sheet, RawButtonId(13));
        assert_eq!(reply.gump_id, RawGumpId(0x00AD_0001));
        assert_eq!(reply.key, RawGumpKey(0x2A), "the key is echoed, not invented");
        assert_eq!(reply.button, RawButtonId(13));
        assert_eq!(reply.switches, vec![RawSwitchId(4), RawSwitchId(9)]);
    }

    /// A switch starts where the layout said it starts, and only the first time:
    /// re-seeding every frame would pull a checkbox back out from under the
    /// player's finger.
    #[test]
    fn a_switch_is_seeded_once_and_then_belongs_to_the_player() {
        let gump = admin_menu();
        let mut sheet = Sheet::default();
        seed_switches(&mut sheet, &gump);
        assert_eq!(sheet.switches.get(&1), Some(&false), "the layout's own initial");

        sheet.switches.insert(1, true);
        seed_switches(&mut sheet, &gump);
        assert_eq!(
            sheet.switches.get(&1),
            Some(&true),
            "and it stays where it was put"
        );
    }

    /// Bytes enough for one group of eight hues, hue 1's ramp set from
    /// `colors` and the other seven entries left at zero — the same shape
    /// `hues.rs`'s and `client/render`'s `hue.rs`'s own tests build, since
    /// there is no public constructor for `Hues` that skips the file format.
    fn one_hue_group(colors: [Color16; COLORS_PER_HUE]) -> Hues {
        const GROUP_HEADER: usize = 4;
        const ENTRY_BYTES: usize = COLORS_PER_HUE * 2 + 2 + 2 + 20;
        const HUES_PER_GROUP: usize = 8;
        let mut bytes = vec![0u8; GROUP_HEADER + HUES_PER_GROUP * ENTRY_BYTES];
        for (index, color) in colors.iter().enumerate() {
            let at = GROUP_HEADER + index * 2;
            bytes[at..at + 2].copy_from_slice(&color.0.to_le_bytes());
        }
        Hues::parse(&bytes).expect("one whole group")
    }

    /// The colour pinned to a controlled ramp rather than a real `hues.mul`:
    /// this is a claim about which *column* `text_color` reads, and the only
    /// way a wrong column reads as a wrong colour instead of a coincidentally
    /// plausible one is to put a different, known colour in every column.
    #[test]
    fn a_hued_label_takes_the_ninth_step_of_its_ramp_not_the_first() {
        let mut colors = [Color16::TRANSPARENT; COLORS_PER_HUE];
        colors[0] = Color16(0b0_00000_00000_11111); // pure blue: the wrong answer
        colors[TEXT_RAMP_STEP] = Color16(0b0_11111_00000_00000); // pure red: the right one
        let hues = one_hue_group(colors);

        assert_eq!(
            text_color(&hues, 1),
            Some(egui::Color32::from_rgb(255, 0, 0)),
            "hue 1's ninth step (GetUnicodeFontColor's ColorTable[8]), widened"
        );
    }

    /// `Hue(0)` is the server's "draw no colour at all", not a request for
    /// `hues.mul` row zero — see `Hues::get`'s own docs for the one-off this
    /// guards. Answered with a ramp where every step is a real colour, so a
    /// bug that treated `0` as an index would come back `Some`, not panic.
    #[test]
    fn hue_zero_is_no_colour_rather_than_a_row() {
        let hues = one_hue_group([Color16(0b0_11111_00000_00000); COLORS_PER_HUE]);
        assert_eq!(text_color(&hues, 0), None);
    }
}
