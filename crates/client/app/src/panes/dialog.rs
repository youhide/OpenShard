//! A `0xB0` dialog as a component: the fourth window kind to own its state, its
//! layout and its input.
//!
//! Step 4 of `docs/client/design_panes.md`, and the kind the plan called "mostly
//! a move": `crate::gump::Dialogs` already held the page, the switches, the
//! typed text and the held button, keyed by gump id, so what this step really
//! does is turn *one map keyed by window* into *one window's worth of state*.
//! That map is gone and so is the module it lived in — the key was the window,
//! and a window is now a place in
//! [`Windows::own_windows`](crate::windows::Windows::own_windows) with a pane
//! beside it.
//!
//! # Three things the wire does not say
//!
//! Inherited whole from the module this replaces, because they are facts about
//! the protocol rather than about where the state was kept:
//!
//! 1. **A window closes when it is answered.** No packet says so — the shard
//!    sends one `0xB0`, waits for one `0xB1`, and both ends assume the client
//!    took the window down. That is why answering is [`Effect::Answer`] and not
//!    an ordinary [`Effect::Net`] beside an [`Effect::Close`]: see the effect's
//!    own doc.
//! 2. **A page button never reaches the shard.** `{ button ... 0 N id }` flips
//!    to page `N` inside the client, and only a reply button answers. So
//!    [`DialogPane::page`] is state that lives here and nowhere else.
//! 3. **A button is pressed on the way down and answered on the way up.** The
//!    layout carries two pictures for every button and nothing that says when to
//!    draw the second; it is the mouse, and [`DialogPane::held`] is where the
//!    mouse is remembered between the two events — the same field
//!    [`SkillsPane::held`](crate::panes::skills) is, one kind over.
//!
//! # Nothing is seeded any more
//!
//! `Dialogs::sync` ran every frame and wrote a layout's `initial` flags into a
//! map *the first time* each id was seen, because writing them a second time
//! would have pulled a checkbox back out from under the player's finger. That
//! whole distinction is gone: absence in [`DialogPane::switches`] and
//! [`DialogPane::entries`] **means "whatever the layout says"**, so there is
//! nothing to seed and no first time to get right. What the player has touched
//! is what is in the maps, and a re-sent layout can no longer overwrite it.
//!
//! The one place that difference is visible is the answer. A reply used to
//! carry the entries the map happened to hold; it is built from the *layout's*
//! fields now ([`DialogPane::reply`]), so every field the shard asked for
//! travels whether or not the player typed in it — which is what the seeding
//! was quietly arranging, in a place where forgetting it would have dropped a
//! field from the packet rather than from the screen.

use std::collections::{
    BTreeSet,
    HashMap,
};

use openshard_client_net::action::GumpReply;
use openshard_client_net::view::{
    OpenGump,
    WorldView,
};
use openshard_client_render::gump::{
    self as gump_art,
    CAPTION_FONT,
    CaptionSource,
    GumpArt,
    GumpPixel,
    Hit,
};
use openshard_client_render::text;
use openshard_protocol::gump::layout::{
    Element,
    Flag,
    Switch,
};
use openshard_protocol::gump::{
    GumpId,
    RawButtonId,
    RawGumpId,
    RawGumpKey,
    RawSwitchId,
};
use openshard_protocol::localized;
use openshard_protocol::wire::ClilocId;
use openshard_uofiles::cliloc::ClilocNumber;

use crate::panes::{
    Button,
    Effect,
    Input,
    Key,
    Line,
    PaneCtx,
    PaneFrame,
    Response,
};
use crate::windows::Drawn;

/// One open dialog: the page it is showing, what the player has set on it, and
/// which of its controls the mouse and the keyboard are on.
///
/// Every field here is something no packet carries, which is the whole reason
/// this type exists — the pictures, the boxes and the hit table are worked out
/// from the layout in the view on every frame, and none of them is remembered.
#[derive(Debug)]
pub struct DialogPane {
    /// Which dialog this is a pane of.
    ///
    /// A shop's serial, one kind over: the subject carries a key and the pane is
    /// handed it when the window opens, because everything the pane looks up —
    /// the layout, the text table, the reply key — is in the view under this id.
    gump_id:  GumpId,
    /// The page being shown. Page `0` is drawn on every page, so this starts at
    /// zero and a layout with no pages at all never leaves it.
    page:     GumpPage,
    /// The switches the player has turned over, by their id.
    ///
    /// **Absence means the layout's own `initial`** — see the module docs. So a
    /// checkbox nobody has touched is not in here at all, and cannot be
    /// overwritten by a frame, a re-sent layout or a second reader.
    switches: HashMap<RawSwitchId, bool>,
    /// What has been typed into each field, by its id. Absent means the line the
    /// layout pointed the field at, for the same reason.
    entries:  HashMap<TextEntryId, String>,
    /// The button the mouse went down on.
    ///
    /// A [`Hit`] rather than a button id because that is what a press *is*:
    /// [`gump_art::window`] draws the pressed face by comparing this against the
    /// hit it computes, so what looks pressed and what the release will act on
    /// are one value.
    ///
    /// One per window now, where `Dialogs` kept one per client with the window
    /// it belonged to beside it. There is still one pointer — a press on this
    /// window is only ever offered to this pane, and it can only be followed by
    /// the release that ends it — and the window it belongs to is the window
    /// this pane is in.
    held:     Option<Hit>,
    /// The field taking keystrokes, **if this window has the keyboard at all**.
    ///
    /// Two questions, and the other one is the manager's: which *window* the
    /// keys go to is [`Windows::keyboard`](crate::windows::Windows::keyboard),
    /// because a keyboard is exclusive across windows the way z-order and the
    /// hand are (decisions 2 and 7), and a pane cannot know what another pane
    /// has taken. This says only which of *this* window's boxes the keys would
    /// land in, and it is read only when [`PaneFrame::has_keyboard`] says they
    /// are coming here — so a field left focused in a window the player has
    /// clicked away from draws no caret and takes no letter.
    focus:    Option<TextEntryId>,
}

/// A page inside one gump layout.
///
/// Page numbers are client-side state, distinct from gump ids and from the
/// layout's text-entry ids. The renderer's layout boundary stays raw because
/// that is where its parser declares the number.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct GumpPage(u32);

impl GumpPage {
    const fn new(page: u32) -> Self {
        Self(page)
    }

    const fn raw(self) -> u32 {
        self.0
    }
}

/// The identity of one editable field inside a gump layout.
///
/// It is not a page, a button, or a switch id. The wire/layout seam is where it
/// becomes the `u16` a `0xB1` text-entry list carries.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
struct TextEntryId(u16);

impl TextEntryId {
    const fn new(id: u16) -> Self {
        Self(id)
    }

    const fn raw(self) -> u16 {
        self.0
    }
}

/// A dialog, laid out for one frame: what is drawn and what answers the mouse,
/// and what is written over it.
///
/// The second half is new with step 4, and it is why this is an app-side type
/// rather than [`gump_art::Window`] alone. A caption in a layout is a *key* —
/// into the text table that arrived beside the packet, or into the client's own
/// `Cliloc.enu` — and resolving one needs both of those plus what the player has
/// typed, none of which `client/render` has ever heard of. That resolution used
/// to happen in the text pass, from a `Dialogs` the pass reached into; it
/// happens here, in the pane that owns the answer, and both passes now draw a
/// dialog's text exactly the way they draw a shop's or a status frame's: out of
/// what the layout produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Window {
    /// The pictures, the hit table and the boxes — everything the pointer is
    /// tested against.
    pub art:   gump_art::Window,
    /// The text over them, already resolved to strings.
    pub lines: Vec<Line>,
}

/// What the window flags in a layout ask for.
#[derive(Default, Clone, Copy)]
pub struct WindowFlags {
    /// `{ nomove }`: the player may not drag it.
    pub no_move:  bool,
    /// `{ noclose }`: the right button does not take it down, and it has to be
    /// answered by one of its own buttons.
    pub no_close: bool,
}

/// Read the flags out of a layout. `{ nodispose }` and `{ noresize }` have
/// nothing to honour here: no window this client draws is resizable, and there
/// is no separate dismissal to suppress.
pub fn flags(gump: &OpenGump) -> WindowFlags {
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

impl DialogPane {
    /// The pane for the dialog the shard filed under `gump_id`.
    pub fn new(gump_id: GumpId) -> Self {
        Self {
            gump_id,
            page: GumpPage::default(),
            switches: HashMap::new(),
            entries: HashMap::new(),
            held: None,
            focus: None,
        }
    }

    /// This pane's layout, out of the view, or `None` once the shard has taken
    /// the dialog away.
    ///
    /// Looked up every time rather than kept: the layout is the shard's and can
    /// be re-sent, and a copy taken when the window opened would be a second
    /// picture of the same dialog — see [`PaneFrame::view`].
    fn gump<'view>(&self, view: &'view WorldView) -> Option<&'view OpenGump> {
        view.gumps.iter().find(|gump| gump.gump_id == self.gump_id)
    }

    /// Whether one switch is on: what the player made it, or what the layout
    /// asked for if they have not touched it.
    fn set(&self, switch: &Switch) -> bool {
        self.switches.get(&switch.id).copied().unwrap_or(switch.initial)
    }

    /// What is in one field: what has been typed into it, or the line the
    /// layout pointed it at.
    ///
    /// Empty for a layout that named a row past the end of its own text table,
    /// which is a bug on the sending side and not a reason to leave the box out.
    fn typed<'a>(&'a self, field: TextEntryId, gump: &'a OpenGump, line: usize) -> &'a str {
        self.entries
            .get(&field)
            .map(String::as_str)
            .or_else(|| gump.line(line))
            .unwrap_or_default()
    }

    /// Turn a checkbox over.
    fn toggle(&mut self, gump: &OpenGump, switch: RawSwitchId) {
        // The element the hit named, read out of the layout that was hit-tested.
        // Absent only if the shard has re-sent the dialog between the frame that
        // drew the box and the press that landed on it, in which case there is
        // nothing to turn over.
        let Some(set) = gump.elements.iter().find_map(|element| {
            match element {
                Element::Check(check) if check.id == switch => Some(self.set(check)),
                _ => None,
            }
        }) else {
            return;
        };
        self.switches.insert(switch, !set);
    }

    /// Turn a radio on and the rest of its group off.
    ///
    /// The client is what enforces that, not the shard: the wire carries the ids
    /// that are left on and trusts that only one of a group is among them. Every
    /// other radio in the layout is the group, because the layout has no way to
    /// say otherwise.
    fn choose(&mut self, gump: &OpenGump, switch: RawSwitchId) {
        for element in &gump.elements {
            if let Element::Radio(other) = element {
                self.switches.insert(other.id, other.id == switch);
            }
        }
    }

    /// The field the keyboard is going into, ready to be written to.
    ///
    /// This is where a field stops being the layout's and becomes the player's:
    /// the first keystroke copies the line the layout named, and every one after
    /// it appends to that copy.
    fn entry<'a>(&'a mut self, gump: &OpenGump, field: TextEntryId) -> &'a mut String {
        let line = gump.elements.iter().find_map(|element| {
            match element {
                Element::TextEntry { entry_id, line, .. } if TextEntryId::new(*entry_id) == field => {
                    Some(*line)
                }
                _ => None,
            }
        });
        let start = line
            .and_then(|line| gump.line(line))
            .unwrap_or_default()
            .to_owned();
        self.entries.entry(field).or_insert(start)
    }

    /// What travels back: the button, and everything the player set on the way
    /// to pressing it.
    ///
    /// **Built from the layout and not from what this pane happens to hold.**
    /// Every switch the layout declares is asked whether it is on, and every
    /// field it declares is asked what is in it, so an untouched control travels
    /// exactly as the shard drew it. The map-driven version of this was correct
    /// only because a sweep every frame had already copied the layout into the
    /// maps.
    ///
    /// The switches and the fields are sorted before they are sent. Nothing on
    /// the wire requires it — the shard reads the ids as a set — but a packet
    /// whose bytes depend on the iteration order of a hash map is one that
    /// cannot be compared against a recording.
    fn reply(&self, gump: &OpenGump, button: RawButtonId) -> GumpReply {
        let mut switches: Vec<RawSwitchId> = gump
            .elements
            .iter()
            .filter_map(|element| {
                match element {
                    Element::Check(switch) | Element::Radio(switch) => self.set(switch).then_some(switch.id),
                    _ => None,
                }
            })
            .collect();
        switches.sort_unstable();
        // A layout that declares one id twice is a layout the shard should not
        // have sent; saying it twice on the way back would make it the client's
        // bug as well.
        switches.dedup();

        let mut text_entries: Vec<(u16, String)> = gump
            .elements
            .iter()
            .filter_map(|element| {
                match element {
                    Element::TextEntry { entry_id, line, .. } => {
                        let field = TextEntryId::new(*entry_id);
                        Some((field.raw(), self.typed(field, gump, *line).to_owned()))
                    }
                    _ => None,
                }
            })
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

    /// The answer a window closed by the right button — or by Escape — sends:
    /// button zero.
    ///
    /// The shard is waiting for it exactly as much as it is waiting for a real
    /// button, which is why closing a dialog is not the same as forgetting it.
    /// `None` for a `{ noclose }` layout, which has to be answered by one of its
    /// own buttons and whose window therefore stays up.
    ///
    /// Asked by the manager rather than reached by an input, because both
    /// gestures that close a window are the manager's (decision 2) and one of
    /// them — Escape on the topmost — never touches this window at all. See
    /// `App::close_window`, which is the one door both go through.
    pub fn dismiss(&self, gump: &OpenGump) -> Option<GumpReply> {
        if flags(gump).no_close {
            return None;
        }
        Some(self.reply(gump, RawButtonId(0)))
    }

    /// A left press somewhere in the window, against the layout it was drawn as.
    ///
    /// A switch answers on the way *down* and a button does not: that is the
    /// reference's own split. A checkbox is its own answer and there is nothing
    /// to wait for; a button has a pressed picture to show while the finger is
    /// on it, and what it means happens when the finger comes off.
    ///
    /// Every arm raises, and every arm takes the press — a press that reached a
    /// window never reaches the world behind it. What the arms differ about is
    /// the keyboard and the grab.
    fn press(&mut self, gump: &OpenGump, laid_out: &Window, ctx: &PaneCtx<'_>) -> Response {
        let raised = Response::changed().with(Effect::Raise);
        // A field is a box and not a picture, and it is tested first for that
        // reason: it lies over the background, which *is* a picture, and asking
        // the pictures first would answer "the background" for every click into
        // a field.
        if let Some(id) = gump_art::field(&laid_out.art.fields, ctx.frame.cursor) {
            self.focus = Some(TextEntryId::new(id));
            return raised.with(Effect::TakeKeyboard);
        }
        // Every other press in this window gives the keyboard back, which is
        // what the old global `Dialogs::focus = None` said: a field keeps the
        // keys only while the player is still in the box.
        self.focus = None;
        let raised = raised.with(Effect::ReleaseKeyboard);
        match laid_out.art.hit(ctx.frame.cursor, ctx.frame.files.gump_atlas) {
            Some(hit @ (Hit::Reply(_) | Hit::Page(_))) => {
                self.held = Some(hit);
                raised
            }
            Some(Hit::Check(id)) => {
                self.toggle(gump, id);
                raised
            }
            Some(Hit::Radio(id)) => {
                self.choose(gump, id);
                raised
            }
            // The background, a `{ gumppic }`, a label: nothing that answers, so
            // the press moves the window instead — which is how a gump is moved
            // when it has no title bar to move it by.
            //
            // `{ nomove }` is the one window in this client that is taken and
            // *not* grabbed: a shard that pins a dialog somewhere means it, and
            // the press is still the window's so that it cannot steer the body
            // behind it.
            None => {
                if flags(gump).no_move {
                    raised
                } else {
                    // Already local to this window — see `PaneFrame::cursor`'s
                    // doc — so it is the grab offset with nothing subtracted.
                    raised.with(Effect::Grab)
                }
            }
        }
    }

    /// The release that finishes a press on one of this window's buttons.
    ///
    /// The pointer has to still be on the button it went down on: a press
    /// dragged off its button is a press taken back, in this client as in every
    /// other. Either way the answer is [`Response::changed`] once a press is
    /// being held — the button was drawn pressed and has to come back up.
    fn release(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let Some(held) = self.held.take() else {
            return Response::ignored();
        };
        let (Some(gump), Some(Drawn::Dialog(laid_out))) = (self.gump(ctx.frame.view), ctx.drawn) else {
            return Response::changed();
        };
        if laid_out.art.hit(ctx.frame.cursor, ctx.frame.files.gump_atlas) != Some(held) {
            return Response::changed();
        }
        match held {
            // Inside the client and nowhere else — see the module docs.
            Hit::Page(page) => {
                self.page = GumpPage::new(page);
                Response::changed()
            }
            Hit::Reply(button) => Response::changed().with(Effect::Answer(self.reply(gump, button))),
            // A switch is never held — see `press`.
            Hit::Check(_) | Hit::Radio(_) => Response::changed(),
        }
    }

    /// One keystroke, offered to this pane because the manager says the keyboard
    /// is this window's.
    ///
    /// Which field it lands in is [`DialogPane::focus`], and a window that holds
    /// the keyboard with nothing focused takes nothing: that is a window the
    /// player clicked into and then clicked out of, and the letter belongs to
    /// whatever asks next.
    ///
    /// Handed the layout rather than the whole context, which is what lets it be
    /// exercised without a `Resources` — see the plan's step 8, which is blocked
    /// on exactly that for everything that hit-tests.
    fn key(&mut self, key: Key, gump: &OpenGump) -> Response {
        let Some(field) = self.focus else {
            return Response::ignored();
        };
        match key {
            // Enter and Escape both mean "done with this box", and Escape means
            // it *instead* of closing the window — which is the one place this
            // client's Escape does something other than take the topmost window
            // down.
            Key::Done | Key::Cancel => {
                self.focus = None;
                Response::changed().with(Effect::ReleaseKeyboard)
            }
            Key::Backspace => {
                self.entry(gump, field).pop();
                Response::changed()
            }
            // A newline in a `{ textentry }` is not a character the reference
            // lets in either, and neither is anything else the keyboard produces
            // that is not a letter.
            Key::Typed(character) if character.is_control() => Response::ignored(),
            Key::Typed(character) => {
                self.entry(gump, field).push(character);
                Response::changed()
            }
        }
    }

    /// Every line a laid-out dialog draws: its captions, and what is in its
    /// fields.
    ///
    /// The captions name rows of the text table that arrived beside the layout,
    /// or numbers in the client's own cliloc, and this is where either is
    /// resolved — `client/render` has never heard of an [`OpenGump`] and should
    /// not. A row a layout names and the table does not hold draws nothing,
    /// which is a shard bug and not a reason to drop the window.
    ///
    /// A focused field is written with a caret after it, and only while this
    /// window actually has the keyboard. That is the only thing this client
    /// draws that the layout did not ask for, and it is the one thing a player
    /// cannot otherwise see: which of two boxes the keys are going into.
    fn lines(&self, gump: &OpenGump, art: &gump_art::Window, frame: &PaneFrame<'_>) -> Vec<Line> {
        let mut lines: Vec<Line> = art
            .captions
            .iter()
            .filter_map(|caption| {
                // A wire line is missing when the layout named a row past the
                // table's end — a bug on the sending side. A cliloc is missing
                // both when this client has no `Cliloc.enu` at all and when the
                // number simply is not one of the ~40,000 it holds; either way
                // there is nothing to draw and the caption is dropped rather
                // than shown as a placeholder.
                let text = match caption.source {
                    CaptionSource::Line(line) => gump.line(line)?,
                    CaptionSource::Cliloc(number) => {
                        frame
                            .files
                            .cliloc
                            .and_then(|table| table.get(ClilocNumber::new(number)))
                            .or_else(|| localized::fallback(ClilocId(number)))?
                    }
                };
                Some(Line {
                    at:   caption.at,
                    font: CAPTION_FONT,
                    hue:  caption.hue,
                    clip: caption.clip,
                    text: text.to_owned(),
                })
            })
            .collect();
        for field in &art.fields {
            let id = TextEntryId::new(field.id);
            let typed = self.typed(id, gump, field.line);
            lines.push(Line {
                at:   field.at,
                font: CAPTION_FONT,
                hue:  field.hue,
                clip: Some(field.size),
                text: typed.to_owned(),
            });
            if frame.has_keyboard && self.focus == Some(id) {
                // A line of its own rather than a character appended to the
                // first: where it goes has to be *measured*, which is what
                // `text::gump_width` is for — the same walk `letters` advances
                // by, so the caret lands where the next character will.
                lines.push(Line {
                    at:   field.at.offset(GumpPixel::new(
                        text::gump_width(typed, CAPTION_FONT, frame.files.font_atlas),
                        0,
                    )),
                    font: CAPTION_FONT,
                    hue:  field.hue,
                    clip: None,
                    text: CARET.to_owned(),
                });
            }
        }
        lines
    }
}

impl DialogPane {
    /// Every page's art, and not the showing page's.
    ///
    /// Two reasons, both the shard's doing: a `{ resizepic }` cannot be placed
    /// until its nine pieces are packed — where its edges go is decided by how
    /// big its corners turned out to be — and a page button flips pages inside
    /// the client, so a window needs the art of a page nobody has looked at yet.
    ///
    /// This is the one kind whose art really is named here rather than left to
    /// the sweep at the end of `render_passes::draw_gump_windows`, which is why
    /// decision 6's phase exists at all: the layout cannot be worked out until
    /// the atlas has been asked. That packing used to be a loop over every gump
    /// in the *view*, one rung above the windows — so a dialog this client had
    /// closed and the view had not yet forgotten still had its art packed.
    pub(super) fn art(&self, frame: &PaneFrame<'_>) -> Vec<GumpArt> {
        self.gump(frame.view)
            .map(|gump| gump_art::art_of(&gump.elements).into_iter().collect())
            .unwrap_or_default()
    }

    /// The layout, and the text over it.
    ///
    /// `None` is **reachable**, the same as a shop's: the view can drop a gump
    /// between the packet that opened the window and the frame that draws it,
    /// and `reconcile_own_windows` takes the window itself away one frame later.
    pub(super) fn layout(&self, frame: &PaneFrame<'_>) -> Option<Drawn> {
        let gump = self.gump(frame.view)?;
        // Only the switches that are *on*, which is what `gump_art::window`
        // wants: a set it can ask about, rather than the map of everything this
        // pane knows.
        let on: BTreeSet<RawSwitchId> = gump
            .elements
            .iter()
            .filter_map(|element| {
                match element {
                    Element::Check(switch) | Element::Radio(switch) => self.set(switch).then_some(switch.id),
                    _ => None,
                }
            })
            .collect();
        let art = gump_art::window(
            &gump.elements,
            // Window-local — see `PaneFrame::cursor`'s doc.
            GumpPixel::new(0, 0),
            self.page.raw(),
            &on,
            self.held,
            frame.files.gump_atlas,
        );
        let lines = self.lines(gump, &art, frame);
        Some(Drawn::Dialog(Window { art, lines }))
    }

    /// A press on the window's furniture, the release that completes it, and
    /// the keys that go into a field.
    ///
    /// The press is *located*, so it begins by asking whether this window is the
    /// one the pointer is on — see [`PaneCtx::under_pointer`]. Neither of the
    /// other two is: a release finishes a press this pane started wherever the
    /// pointer has got to, and a keystroke is routed by *identity* — the manager
    /// offers it to the window that holds the keyboard and to no other.
    ///
    /// **No wheel.** A dialog has no scrolling control, so a notch over one goes
    /// past it to the camera, exactly as it does today. Which windows claim a
    /// wheel they have no use for is the plan's own Backlog entry, and it is not
    /// settled by moving this kind's input.
    pub(super) fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response {
        match input {
            Input::Press(Button::Left) => {
                if !ctx.under_pointer {
                    return Response::ignored();
                }
                // Both halves are last frame's: the window the pointer is over,
                // and the layout it was drawn as. Laying the dialog out again
                // here would ask the atlas and the view a second question whose
                // answer is free to differ from the picture the player is
                // pointing at — the rule `drawn_windows` exists for.
                let (Some(gump), Some(Drawn::Dialog(laid_out))) = (self.gump(ctx.frame.view), ctx.drawn)
                else {
                    return Response::ignored();
                };
                self.press(gump, laid_out, ctx)
            }
            Input::Release(Button::Left) => self.release(ctx),
            Input::Key(key) => {
                match self.gump(ctx.frame.view) {
                    Some(gump) => self.key(key, gump),
                    // The shard has taken the dialog away and the window has not
                    // been reconciled off the list yet: there is nothing to type
                    // into.
                    None => Response::ignored(),
                }
            }
            // The right button is the manager's close, which for this kind is
            // the one close that travels — see `DialogPane::dismiss`. A
            // modal's answer is the shard's dialog asking nothing of the
            // client's own: the two are different windows, and only the one
            // that asked is offered the number.
            Input::Move
            | Input::Wheel(_)
            | Input::Press(Button::Right)
            | Input::Release(Button::Right)
            | Input::Answered(_) => Response::ignored(),
        }
    }
}

/// What a focused field draws after what has been typed into it.
const CARET: &str = "|";

#[cfg(test)]
mod tests {
    use openshard_protocol::gump::layout::parse;
    use openshard_protocol::gump::{
        ButtonId,
        GumpButton,
        GumpKey,
        GumpLayout,
        GumpPoint,
        SwitchId,
    };

    use super::*;

    fn admin_menu() -> OpenGump {
        let mut layout = GumpLayout::new();
        layout.background(0, 0, 300, 270, 5054);
        layout.label(105, 14, 2100, "Admin");
        layout.button(30, 54, 4005, 4007, GumpButton::Reply, 0, ButtonId(13));
        layout.label(66, 56, 1153, "Populate Felucca");
        layout.check(30, 100, 210, 211, false, SwitchId(1));
        layout.radio(30, 130, 208, 209, false, SwitchId(2));
        layout.radio(30, 150, 208, 209, true, SwitchId(3));
        layout.text_entry(30, 180, 120, 20, 1153, 7, "Britain");
        let (string, lines) = layout.finish();
        OpenGump {
            key:      GumpKey(0x2A),
            gump_id:  GumpId(0x00AD_0001),
            at:       GumpPoint::new(100, 100),
            elements: parse(string),
            lines:    lines.to_vec(),
        }
    }

    fn pane(gump: &OpenGump) -> DialogPane {
        DialogPane::new(gump.gump_id)
    }

    /// A switch answers with what the layout asked for until the player touches
    /// it, and with what they made it afterwards — with **nothing written down**
    /// in between.
    ///
    /// `Dialogs` arranged the same answer by copying every layout's initial
    /// flags into a map on the first frame and being careful never to do it
    /// again. Absence means the layout here, so there is no first frame to get
    /// right and a re-sent dialog cannot reach in.
    #[test]
    fn an_untouched_switch_is_the_layouts_and_a_touched_one_is_the_players() {
        let gump = admin_menu();
        let mut pane = pane(&gump);
        assert!(pane.switches.is_empty(), "nothing is remembered until it is done");

        let check = Switch {
            x:       30,
            y:       100,
            off:     210,
            on:      211,
            initial: false,
            id:      RawSwitchId(1),
        };
        assert!(!pane.set(&check), "the layout said off");

        pane.toggle(&gump, RawSwitchId(1));
        assert!(pane.set(&check), "and the player turned it on");
        assert_eq!(pane.switches.len(), 1, "only what was touched is written down");
    }

    /// A radio turns its neighbours off and a checkbox does not. The whole
    /// difference between the two, and the client is what enforces it: the wire
    /// carries a set of ids and trusts that only one of a group is in it.
    #[test]
    fn a_radio_turns_its_group_off_and_a_checkbox_minds_its_own_business() {
        let gump = admin_menu();
        let mut pane = pane(&gump);

        pane.choose(&gump, RawSwitchId(2));
        assert_eq!(pane.switches.get(&RawSwitchId(2)), Some(&true));
        assert_eq!(
            pane.switches.get(&RawSwitchId(3)),
            Some(&false),
            "the other radio went off, initial or not"
        );

        pane.toggle(&gump, RawSwitchId(1));
        assert_eq!(pane.switches.get(&RawSwitchId(1)), Some(&true));
        assert_eq!(
            pane.switches.get(&RawSwitchId(2)),
            Some(&true),
            "a checkbox left the radios alone"
        );
    }

    /// What travels back. Four things worth pinning: only the switches that are
    /// *on* are sent, they are sent in a stable order, what was typed goes with
    /// them under the id the layout gave the field, and the key is echoed rather
    /// than invented.
    #[test]
    fn an_answer_carries_what_the_player_set_and_nothing_else() {
        let gump = admin_menu();
        let mut pane = pane(&gump);
        pane.toggle(&gump, RawSwitchId(1));
        pane.choose(&gump, RawSwitchId(3));
        pane.entry(&gump, TextEntryId::new(7)).push('!');

        let reply = pane.reply(&gump, RawButtonId(13));
        assert_eq!(reply.gump_id, RawGumpId(0x00AD_0001));
        assert_eq!(reply.key, RawGumpKey(0x2A), "the key is echoed, not invented");
        assert_eq!(reply.button, RawButtonId(13));
        assert_eq!(reply.switches, vec![RawSwitchId(1), RawSwitchId(3)]);
        assert_eq!(reply.text_entries, vec![(7, "Britain!".to_owned())]);
    }

    /// **The answer is the layout's list, not the pane's.** A dialog nobody has
    /// touched still answers with every field the shard declared, holding the
    /// line the shard put in it — which is what the frame-by-frame seeding used
    /// to arrange, in a place where forgetting it would have dropped a field
    /// from the packet rather than from the screen.
    #[test]
    fn an_untouched_dialog_still_answers_with_its_fields() {
        let gump = admin_menu();
        let pane = pane(&gump);
        let reply = pane.reply(&gump, RawButtonId(0));
        assert_eq!(
            reply.text_entries,
            vec![(7, "Britain".to_owned())],
            "the field travels with what the layout put in it"
        );
        assert_eq!(
            reply.switches,
            vec![RawSwitchId(3)],
            "and the radio the layout started on is the one that is on"
        );
    }

    /// A field is the layout's until the first key lands in it, and the
    /// player's from then on — including the rubbing out, which cannot start
    /// from an empty box.
    #[test]
    fn a_key_copies_the_layouts_line_before_it_changes_it() {
        let gump = admin_menu();
        let mut pane = pane(&gump);
        assert!(pane.entries.is_empty());

        pane.entry(&gump, TextEntryId::new(7)).pop();
        assert_eq!(
            pane.typed(TextEntryId::new(7), &gump, 7),
            "Britai",
            "backspace ate the layout's own last letter, not nothing"
        );
    }

    /// Keys go into the field the player clicked into, and a window holding the
    /// keyboard with nothing focused takes none of them.
    ///
    /// The second half is what makes [`DialogPane::focus`] and
    /// [`Windows::keyboard`](crate::windows::Windows::keyboard) two questions
    /// rather than one fact in two shapes: the manager can be pointing the keys
    /// at this window while this window has no box open, and the honest answer
    /// then is "not mine".
    #[test]
    fn a_keystroke_needs_a_field_to_go_into() {
        let gump = admin_menu();
        let mut pane = pane(&gump);
        assert!(!pane.key(Key::Typed('x'), &gump).taken, "nothing focused");
        assert!(!pane.key(Key::Backspace, &gump).taken);
        assert!(pane.entries.is_empty(), "and nothing was written down");

        pane.focus = Some(TextEntryId::new(7));
        assert!(pane.key(Key::Typed('x'), &gump).redraw);
        assert!(
            !pane.key(Key::Typed('\n'), &gump).redraw,
            "a control character is not a character a field takes, and asks for no frame"
        );
        assert!(pane.key(Key::Backspace, &gump).redraw);
        assert_eq!(pane.typed(TextEntryId::new(7), &gump, 7), "Britain");
    }

    /// Enter and Escape hand the keyboard back, and that is an *effect*: the
    /// pane can put its own field down but only the manager can say the keys
    /// are the world's again.
    #[test]
    fn being_done_with_a_field_gives_the_keyboard_back() {
        let gump = admin_menu();
        let mut pane = pane(&gump);
        pane.focus = Some(TextEntryId::new(7));

        let answer = pane.key(Key::Done, &gump);
        assert!(answer.redraw, "the caret went, so the frame is stale");
        assert!(
            matches!(answer.out.as_slice(), [Effect::ReleaseKeyboard]),
            "and the manager is asked to give them back"
        );
        assert_eq!(pane.focus, None);
        assert!(
            !pane.key(Key::Typed('x'), &gump).taken,
            "the next letter is nobody's here"
        );
    }

    /// `{ noclose }` means the window cannot be walked away from: the right
    /// button gets no answer out of it, and the shard keeps waiting for one of
    /// its own buttons.
    #[test]
    fn a_noclose_window_is_not_dismissed_by_the_right_button() {
        let mut layout = GumpLayout::new();
        layout.no_close();
        layout.background(0, 0, 100, 100, 5054);
        let (string, lines) = layout.finish();
        let gump = OpenGump {
            key:      GumpKey(1),
            gump_id:  GumpId(2),
            at:       GumpPoint::new(0, 0),
            elements: parse(string),
            lines:    lines.to_vec(),
        };

        assert!(pane(&gump).dismiss(&gump).is_none());
        assert!(
            pane(&admin_menu()).dismiss(&admin_menu()).is_some(),
            "and an ordinary one answers with button zero"
        );
        assert_eq!(
            pane(&admin_menu())
                .dismiss(&admin_menu())
                .map(|reply| reply.button),
            Some(RawButtonId(0))
        );
    }

    /// **D9/S4's split, on the one field that reads it.**
    ///
    /// *Which* window the keys reach at all is never this pane's question — see
    /// [`Input::Key`]'s own doc: the manager offers a keystroke only to the
    /// window [`Windows::keyboard`](crate::windows::Windows::keyboard) names,
    /// by identity and ahead of ever building a [`PaneCtx`], so
    /// `DialogPane::handle`'s own `Input::Key` arm is reached at all only when
    /// that has already been decided. What is left for `PaneFrame::has_keyboard`
    /// to answer is the second question the module docs draw the line at: which
    /// of *this* window's boxes the keys are landing in should still draw a
    /// caret, and which should not — a field left focused in a window the
    /// player has clicked away from must go dark rather than keep blinking.
    /// That is read in exactly one place, [`DialogPane::lines`], which is what
    /// this proves it through — [`DialogPane::layout`], not `handle`.
    #[test]
    fn the_caret_is_drawn_only_while_this_window_holds_the_keyboard() {
        use openshard_protocol::serial::Serial;

        use crate::panes::fixture;

        let gump = admin_menu();
        let mut pane = pane(&gump);
        // A field the player has already clicked into, same as
        // `a_keystroke_needs_a_field_to_go_into` arms it — the caret is drawn
        // over *this* box and no other.
        pane.focus = Some(TextEntryId::new(7));

        let mut view = fixture::world(Serial::new(0x0000_0001).unwrap());
        view.gumps.push(gump);
        // No art is needed to pin this: `Field`s and `Caption`s come straight
        // out of the layout's own coordinates, never out of the atlas — see
        // `gump_art::window`'s `Element::TextEntry` arm.
        let files = fixture::Install::shipping([]);

        let with_keyboard = PaneFrame {
            view:         &view,
            files:        files.files(),
            cursor:       GumpPixel::new(0, 0),
            hand:         None,
            has_keyboard: true,
            has_prompt:   false,
            // A dialog does not read it; the field is the status window's.
            status_form:  openshard_client_render::status::Form::Old,
        };
        let Some(Drawn::Dialog(drawn)) = pane.layout(&with_keyboard) else {
            panic!("a dialog whose gump is still in the view lays itself out");
        };
        assert!(
            drawn.lines.iter().any(|line| line.text == CARET),
            "this window holds the keyboard, so the field it is focused on draws a caret"
        );

        let without_keyboard = PaneFrame {
            has_keyboard: false,
            ..with_keyboard
        };
        let Some(Drawn::Dialog(drawn)) = pane.layout(&without_keyboard) else {
            panic!("the same dialog, laid out again");
        };
        assert!(
            !drawn.lines.iter().any(|line| line.text == CARET),
            "the manager says the keys are elsewhere now, and the caret goes with them"
        );
    }
}
