//! The typed line and its rendering, together: [`Chat`] is what has not been
//! sent yet, and [`draw_chat_and_speech`] is the speech line and the journal
//! above it, over the finished picture and under egui's.

use openshard_client_render::atlas::TextSize;
use openshard_client_render::geometry::Rect;
use openshard_client_render::gump::{self as gump_art, GumpPixel, Scissor};
use openshard_client_render::sprite::SpriteQuad;
use openshard_client_render::text::{self, GumpLabel};
use openshard_commands::{PREFIX, StaffCommand};
use openshard_protocol::access::AccessLevel;
use openshard_protocol::speech::Font;
use openshard_protocol::wire::Hue;

use crate::window::Screen;
use crate::{
    CHAT_LINE_HEIGHT, CHAT_LINES, CHAT_MARGIN, desk, profile, resources, scaled_gump_quads, shell, world,
};

/// Which channel the typed line goes to when Enter is pressed.
///
/// # Why a channel and not a prefix
///
/// A guild line is not a command with a `/` in front of it — it is ordinary
/// speech with a different **mode byte**, which is a property of the line rather
/// than of its first character (`docs/roadmap.md` §6, guild chat). A reference
/// client puts a dropdown above the entry field for exactly that reason, and
/// this is that dropdown: [`channel_button`] draws it at the left end of the
/// input line, a click turns it, and it is drawn whether or not the line is
/// open — so a player can always see which channel they are about to speak on
/// rather than discovering it after pressing Enter.
///
/// The alternative — reserving `/` or `\` at the front of the line — was
/// rejected because it makes a character unsayable and hides the state it sets.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Channel {
    /// Out loud, heard by whoever is nearby. The default and the way back.
    #[default]
    Say,
    /// To everyone in your guild, wherever they are.
    Guild,
    /// To every guild yours is allied with.
    Alliance,
    /// To everyone in your party.
    Party,
}

impl Channel {
    /// The channels a key cycles through, in order.
    const ALL: [Self; 4] = [Self::Say, Self::Guild, Self::Alliance, Self::Party];

    /// The next one round, wrapping back to [`Say`](Self::Say).
    #[must_use]
    pub(crate) fn next(self) -> Self {
        let at = Self::ALL.iter().position(|channel| *channel == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// What the prompt calls it.
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Say => "say",
            Self::Guild => "guild",
            Self::Alliance => "alliance",
            Self::Party => "party",
        }
    }
}

/// How many completions are drawn above the speech line at once.
///
/// A lone `.` matches all twenty-five commands and the journal is six lines: a
/// popup that drew every match would push the conversation off the top of the
/// window to show a list nobody reads past the first screen of. What does not
/// fit is counted on the last row instead — see [`Offer::rows`].
///
/// [`CHAT_LINES`] and not a number of its own: the popup and the journal are
/// drawn in the same column at the same line height, so the two together are
/// what has to fit above the input line, and at `desk::ChatScale`'s largest that
/// is already a third of a small window.
///
/// A *taste* and not a limit: what the window can actually hold is
/// [`room_above`], and the popup is drawn to whichever of the two is smaller.
const COMPLETION_ROWS: usize = CHAT_LINES;

/// How dark the chat's own plates are: the channel button, and the bar behind
/// the highlighted completion.
///
/// A quarter of the way up `hues.mul`'s thirty-two rungs, and neither end of
/// them on purpose. Black would read as a hole cut in the world rather than as a
/// piece of interface — the gump pass does no blending, so a plate covers what
/// is under it outright (see [`gump_art::plate`]) — and anything bright enough
/// to read as a plate at a glance is bright enough to swallow the grey the rows
/// are drawn in. This is the rung at which the shard's own system grey still
/// stands off it.
///
/// One shade for both, because they are one thing: the furniture the chat draws
/// under its own text. Two constants of the same value would be two things to
/// change when this is next argued about.
const PLATE_SHADE: f32 = 0.25;

/// A plate filling `box_`, in whichever space the caller draws in.
///
/// `to_real` is `1.0` for the `fonts.mul` path, whose quads are in gump pixels
/// and are multiplied by the shader, and the surface's own scale for the
/// TrueType path, whose quads are already real (see [`text::collect_gump_ttf`]).
/// The box itself is always in gump pixels, which is the space every layout
/// answer in this module is in.
fn plate_of(box_: Scissor, to_real: f32) -> SpriteQuad {
    gump_art::plate(
        Rect {
            x: (box_.at.x as f32 * to_real).round(),
            y: (box_.at.y as f32 * to_real).round(),
            width: box_.width as f32 * to_real,
            height: box_.height as f32 * to_real,
        },
        Hue::NONE,
        gump_art::Shade::new(PLATE_SHADE),
    )
}

/// How many rows of chat fit between the input line and the top of the window.
///
/// The column is laid out **upward** from the input line — the popup first and
/// the journal above it — and until this function existed nothing asked how tall
/// the window was: six journal lines and up to six popup rows were drawn at
/// `desk::ChatScale`'s line height wherever that landed, which at scale 4 on a
/// small window is off the top of the screen. What does not fit is not drawn,
/// which is the only answer that does not lie: a line above the surface is a
/// line the player cannot read, and drawing it costs the same as drawing one
/// they can.
///
/// Both arguments are in gump pixels, the space
/// [`draw_chat_and_speech`] lays the column out in. `canvas_height` is the
/// surface's own height there, and `line_height` is one row of it — which is
/// [`CHAT_LINE_HEIGHT`] scaled by `desk::ChatScale` for `fonts.mul`, and
/// unscaled for a TrueType face (see the call site).
///
/// The arithmetic is the layout's own, read back: the input line's top sits at
/// `canvas_height - CHAT_MARGIN - line_height`, row `k` above it at
/// `line_height * k` higher, and the topmost row drawn must still start at or
/// below [`CHAT_MARGIN`]. Zero is a real answer — a window shorter than its own
/// margins plus one line has room for the input line and nothing else.
pub(crate) fn room_above(canvas_height: i32, line_height: i32) -> usize {
    // A line height of zero would be a division by it; it is a constant times a
    // clamped scale and cannot be, but the layout must not depend on that being
    // true two refactors from now.
    if line_height <= 0 {
        return 0;
    }
    let input_top = canvas_height - CHAT_MARGIN - line_height;
    usize::try_from((input_top - CHAT_MARGIN) / line_height).unwrap_or(0)
}

/// One row of the chat column, in gump pixels.
///
/// The two faces are spaced by two different things, because they are sized by
/// two different things: `fonts.mul` has a fixed height per face and an
/// integer `desk::ChatScale` on top of it, and a TrueType face has a real
/// size — [`desk::FontSizes::speech`] — which is already in the gump pixels
/// this answers in. So the TrueType row is the size the player asked for, and
/// the rows move apart when they turn it up rather than staying put and
/// overlapping. See `docs/text_sizes.md`.
///
/// A function and not a line inside the draw, for [`channel_width`]'s reason:
/// the pointer has to land on the same row the frame drew.
pub(crate) fn line_height(truetype: bool, chat_style: desk::Chat, fonts: desk::FontSizes) -> i32 {
    match truetype {
        true => fonts.speech.pixels().round() as i32,
        false => CHAT_LINE_HEIGHT * chat_style.scale.glyph_scale_factor() as i32,
    }
}

/// The surface's height in gump pixels rather than real ones — `Frame::scale`'s
/// doc is what the pass multiplies back out, and this is that arithmetic done
/// once for where the bottom of the window is.
pub(crate) fn canvas_height(surface_height: u32, scale: f32) -> i32 {
    (surface_height as f32 / scale) as i32
}

/// How wide the widest channel's name is drawn, in gump pixels.
///
/// **The one measurement the channel button's box is built from**, and it is a
/// function rather than a number for `docs/parity.md`'s reason: the frame that
/// draws the button and the click that lands on it are two places, and a box
/// they each worked out for themselves would agree by coincidence. They call
/// this instead.
///
/// The *widest* of the four and not the one showing, so the button does not
/// change size under the pointer when the channel is cycled — a control that
/// moves as it is pressed is a control that is hard to press twice.
///
/// `ttf` is the atlas when a TrueType face is set, which measures in **real**
/// pixels (see [`text::gump_width_ttf`]) and is divided back here — this
/// answers in gump pixels whichever face is in use, because that is the space
/// the layout and the pointer both speak. `magnify` is `desk::ChatScale`'s
/// factor, which multiplies `fonts.mul`'s own pixels and does nothing to a
/// TrueType face.
pub(crate) fn channel_width(
    font_atlas: &openshard_client_render::atlas::FontAtlas,
    ttf: Option<(&openshard_client_render::atlas::TtfAtlas, TextSize)>,
    magnify: i32,
    scale: f32,
) -> i32 {
    let widest = Channel::ALL
        .iter()
        .map(|channel| match ttf {
            Some((atlas, size)) => text::gump_width_ttf(channel.label(), atlas, size),
            None => text::gump_width(channel.label(), Font::DEFAULT, font_atlas) * magnify,
        })
        .max()
        .unwrap_or_default();
    match ttf {
        // Real pixels back to gump ones, once and here, rather than at each of
        // the two call sites — where one of them would eventually round it the
        // other way.
        Some(_) => (widest as f32 / scale).round() as i32,
        None => widest,
    }
}

/// The channel button's box, in gump pixels: at the left end of the input line,
/// exactly as tall as it.
///
/// A **button** and not a chord. `Shift+Tab` still turns the channel and is
/// documented on [`Chat::channel`], but a modifier chord was what this client
/// had instead of a control it had not drawn — the reference client puts a
/// dropdown above its entry field, and a player who has never read a key list
/// cannot find a chord at all.
///
/// The air around the label is [`CHAT_MARGIN`] rather than a padding of its own:
/// the same gap the chat column keeps from the edge of the window, kept from the
/// edge of the plate.
///
/// `channel_width` is [`channel_width`]'s answer — the two are separate so that
/// the pointer's side can measure once and this can stay pure arithmetic.
pub(crate) fn channel_button(canvas_height: i32, line_height: i32, channel_width: i32) -> Scissor {
    Scissor {
        at: GumpPixel::new(CHAT_MARGIN, canvas_height - CHAT_MARGIN - line_height),
        width: channel_width + CHAT_MARGIN * 2,
        height: line_height,
    }
}

/// Where the typed line starts: past the button, with the same air after it.
fn line_starts_at(button: Scissor) -> GumpPixel {
    GumpPixel::new(button.at.x + button.width + CHAT_MARGIN, button.at.y)
}

/// What the speech line is offering to complete, right now.
///
/// Derived from [`Chat::typed`] on every edit rather than stored alongside it
/// (see [`Chat::refresh`]): an offer that is recomputed cannot disagree with the
/// line it is offering against, and "the popup is showing yesterday's matches"
/// is the whole class of bug a completer has.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) enum Offer {
    /// Nothing to offer: the line is not a command, no command matches what has
    /// been typed, or the popup has been put away with Escape.
    #[default]
    Nothing,
    /// The command word is still being typed, and these begin with it.
    ///
    /// `at` is which one is highlighted — always a valid index into `of`, which
    /// is why this variant can only be built by [`Chat::refresh`].
    Candidates { of: Vec<StaffCommand>, at: usize },
    /// The word is a whole command and its arguments are being typed. There is
    /// nothing left to complete, so what is offered is what to type next.
    Arguments(StaffCommand),
}

/// One line of the popup, ready to draw.
pub(crate) struct Row {
    /// What it says.
    pub(crate) text: String,
    /// Whether it is the one Tab would take.
    ///
    /// Drawn on a plate — [`gump_art::plate`] at [`PLATE_SHADE`], as wide as
    /// the widest row of the popup — rather than in a second ink, which is what
    /// it was while the gump pass had no primitive that painted an area. The `>`
    /// marker [`Offer::rows`] puts at the front of the row stays: a plate says
    /// which row is highlighted to somebody looking at the popup, and the marker
    /// still says it in the text itself, which is what a test can read.
    pub(crate) highlighted: bool,
}

impl Offer {
    /// The command Tab would take, if the popup is offering one.
    #[must_use]
    pub(crate) fn highlighted(&self) -> Option<StaffCommand> {
        match self {
            Self::Candidates { of, at } => of.get(*at).copied(),
            Self::Nothing | Self::Arguments(_) => None,
        }
    }

    /// The popup, top row first — that is, furthest from the input line.
    ///
    /// Empty when there is nothing to offer, which is what "no popup" is: there
    /// is no separate visibility flag to disagree with the contents.
    ///
    /// `limit` is how many rows the caller has room for — never more than
    /// [`COMPLETION_ROWS`] and often fewer, because the window has a top
    /// ([`room_above`]). It is a hard cap on the length of what comes back,
    /// counting the "… n more" row: a popup that answered with more rows than it
    /// was given room for would be the caller's problem to crop, and cropping it
    /// there is what would drop the highlighted row rather than the furthest one.
    #[must_use]
    pub(crate) fn rows(&self, limit: usize) -> Vec<Row> {
        if limit == 0 {
            return Vec::new();
        }
        match self {
            Self::Nothing => Vec::new(),
            // A whole command with its arguments spelled out: the same sentence
            // the shard answers a mistyped command with, before it is mistyped.
            Self::Arguments(command) => vec![Row {
                text: describe(*command),
                highlighted: false,
            }],
            // `refresh` never builds this variant empty — an offer of nothing is
            // `Nothing` — but the slice below would panic rather than draw an
            // empty popup if it ever did, and that is not a trade worth taking.
            Self::Candidates { of, .. } if of.is_empty() => Vec::new(),
            Self::Candidates { of, at } => {
                // A list longer than the box spends one of its own rows saying
                // how many it did not draw, so the commands get one fewer than
                // `limit` exactly when there is something left over to count.
                let shown = match limit < of.len() {
                    true => limit - 1,
                    false => of.len(),
                };
                // One row of room and a list that does not fit in it: the count
                // is all there is space for, and it is the more useful of the
                // two — a single command drawn out of twenty-five reads as *the*
                // match rather than as one of them.
                if shown == 0 {
                    return vec![Row {
                        text: format!("  ... {} more", of.len()),
                        highlighted: false,
                    }];
                }
                // The window scrolls with the highlight rather than sitting at
                // the top: a player arrowing down past the eighth match must not
                // be moving a highlight they cannot see.
                let start = at.saturating_sub(shown - 1).min(of.len() - 1);
                let end = (start + shown).min(of.len());
                let mut rows: Vec<Row> = of[start..end]
                    .iter()
                    .enumerate()
                    .map(|(row, command)| Row {
                        text: format!(
                            "{} {}",
                            match start + row == *at {
                                true => '>',
                                false => ' ',
                            },
                            describe(*command)
                        ),
                        highlighted: start + row == *at,
                    })
                    .collect();
                // What did not fit, counted rather than dropped silently: a list
                // that stops at eight with no sign of it is a list that says the
                // ninth command does not exist.
                let hidden = of.len() - (end - start);
                if hidden > 0 {
                    rows.insert(
                        0,
                        Row {
                            text: format!("  ... {hidden} more"),
                            highlighted: false,
                        },
                    );
                }
                rows
            }
        }
    }
}

/// One command on one line: what to type, and what it does.
fn describe(command: StaffCommand) -> String {
    match command.arguments().is_empty() {
        true => format!("{PREFIX}{}  -  {}", command.name(), command.summary()),
        false => format!(
            "{PREFIX}{} {}  -  {}",
            command.name(),
            command.arguments(),
            command.summary()
        ),
    }
}

/// The speech line: what has not been said yet, and whether the keyboard is
/// listening for it.
///
/// Lives on `App` rather than the HUD now — see `shell::Shell`'s old `typed`
/// field — because typing into it has to win the keyboard *before* a letter
/// is read as a hotkey or a walk key, which is a decision `App::window_event`
/// makes and the HUD no longer does.
#[derive(Default, Debug)]
pub(crate) struct Chat {
    /// What has been typed and not yet sent, in bytes: `fonts.mul` is drawn
    /// per byte (see `text::collect`), and every cursor and edit position here
    /// is a byte offset into this string for exactly that reason — a `char`
    /// index would have to be translated back at every glyph anyway.
    pub(crate) typed: String,
    /// Where the caret sits: a byte offset into `typed`, always on a `char`
    /// boundary.
    pub(crate) cursor: usize,
    /// Whether a keystroke that is not a hotkey reaches this line rather than
    /// the character. Opened by Enter, the reference client's own gesture —
    /// there is no mouse hit test for it, so nothing else about picking has
    /// to change for this to work.
    pub(crate) focused: bool,
    /// Where the next line goes. **Kept** across sends — see [`Channel`], and
    /// note that a channel which reset itself after every line would make a
    /// conversation on one channel four keystrokes a sentence.
    ///
    /// Turned two ways, which are the same state and not two: the **button** at
    /// the left of the input line ([`channel_button`], and what a hand on the
    /// mouse reaches for), and **Shift+Tab** while the line has the keyboard —
    /// which stays because a hand already typing should not have to leave the
    /// keyboard to answer in the same channel it was answering in. It was plain
    /// Tab until the completer took that key: a channel is chosen once a
    /// conversation and a command completed once a word, so the cheaper gesture
    /// went to the commoner act.
    pub(crate) channel: Channel,
    /// What the completer is offering against [`typed`](Self::typed) right now.
    ///
    /// Never written from outside: every path that changes the line ends in
    /// [`refresh`](Self::refresh), so the offer cannot be stale. See [`Offer`].
    pub(crate) offer: Offer,
    /// Whether Escape has put the popup away for the line as it stands.
    ///
    /// Cleared by the next edit, because the player who types another letter is
    /// asking a new question and the old refusal was about the old one.
    dismissed: bool,
}

impl Chat {
    /// Insert typed text at the caret and move the caret past it.
    ///
    /// `authority` is what the shard holds this character at — see
    /// [`Chat::refresh`] for why every path that changes the line carries it
    /// rather than the line remembering it.
    pub(crate) fn insert(&mut self, text: &str, authority: AccessLevel) {
        self.typed.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.edited(authority);
    }

    /// Delete the `char` before the caret, if any.
    pub(crate) fn backspace(&mut self, authority: AccessLevel) {
        let Some(before) = self.typed[..self.cursor].chars().next_back() else {
            return;
        };
        let start = self.cursor - before.len_utf8();
        self.typed.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.edited(authority);
    }

    /// Delete the word before the caret, together with whitespace immediately
    /// before it — the conventional `Ctrl+Backspace` edit. A word is a run of
    /// non-whitespace Unicode characters, so Cyrillic is handled exactly like
    /// Latin and the cursor stays on a UTF-8 boundary.
    pub(crate) fn backspace_word(&mut self, authority: AccessLevel) {
        let before = &self.typed[..self.cursor];
        let whitespace_end = before.trim_end_matches(char::is_whitespace).len();
        let start = before[..whitespace_end]
            .trim_end_matches(|character: char| !character.is_whitespace())
            .len();
        if start == self.cursor {
            return;
        }
        self.typed.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.edited(authority);
    }

    /// Delete the `char` after the caret, if any.
    pub(crate) fn delete(&mut self, authority: AccessLevel) {
        let Some(after) = self.typed[self.cursor..].chars().next() else {
            return;
        };
        let end = self.cursor + after.len_utf8();
        self.typed.replace_range(self.cursor..end, "");
        self.edited(authority);
    }

    /// The line changed under the player's own hand: a dismissed popup is asked
    /// again, and the offer is recomputed.
    fn edited(&mut self, authority: AccessLevel) {
        self.dismissed = false;
        self.refresh(authority);
    }

    /// Recompute [`offer`](Self::offer) from the line.
    ///
    /// The one place the popup's contents are decided, and it reads nothing but
    /// `typed`, `dismissed` and the authority it is handed.
    ///
    /// **The authority is a parameter and not a field.** It is the shard's
    /// answer, it lives on the view
    /// (`openshard_client_net::view::WorldView::authority`), and a copy kept
    /// here would be a second place for it to be wrong — the offer is recomputed
    /// on every keystroke, so reading it at every keystroke costs nothing and
    /// cannot go stale. Every path that changes the line therefore carries it.
    ///
    /// Note what it does *not* read: the caret. An
    /// offer that followed the caret would flicker as it was walked back through
    /// a finished line, and Tab replaces the command word wherever the caret
    /// stands — which is the only word a `.` line has that can be completed.
    fn refresh(&mut self, authority: AccessLevel) {
        // What was highlighted before, so that a narrowing list keeps the
        // player's choice instead of snapping back to the alphabetically first.
        let chosen = self.offer.highlighted();
        self.offer = Offer::Nothing;
        if self.dismissed {
            return;
        }
        let Some(body) = self.typed.strip_prefix(PREFIX) else {
            return;
        };
        // Nothing at all for somebody who may command nothing — the *usage
        // hint* below included, which `StaffCommand::parse` would otherwise hand
        // out for a finished word: a player who may not run `.go` must not be
        // shown how to spell it either. `matching` makes the same test for the
        // list; both read `StaffCommand::AUTHORITY`, which is also the constant
        // the shard's own gate compares against.
        if !authority.allows(StaffCommand::AUTHORITY) {
            return;
        }
        match body.split_once(char::is_whitespace) {
            // Past the word: the command is settled, and what is left to offer
            // is its arguments. A first word that is not a command offers
            // nothing — the shard will say so on Enter, and a popup that
            // repeated the refusal while the rest of the line is typed is noise.
            Some((word, _)) => {
                if let Some(command) = StaffCommand::parse(word) {
                    self.offer = Offer::Arguments(command);
                }
            }
            // Still in the word.
            None => {
                let of = StaffCommand::matching(body, authority);
                if of.is_empty() {
                    return;
                }
                let at = chosen
                    .and_then(|command| of.iter().position(|other| *other == command))
                    .unwrap_or(0);
                self.offer = Offer::Candidates { of, at };
            }
        }
    }

    /// Take the highlighted completion — Tab. Answers whether the line changed.
    ///
    /// The whole command word is replaced, and a space is left after it: the
    /// next thing a player types is an argument, and a completer that made them
    /// press space themselves has completed half a word.
    pub(crate) fn complete(&mut self, authority: AccessLevel) -> bool {
        let Some(command) = self.offer.highlighted() else {
            return false;
        };
        let word = self.typed.find(char::is_whitespace).unwrap_or(self.typed.len());
        let name = format!("{PREFIX}{}", command.name());
        self.typed.replace_range(..word, &name);
        // Exactly one space after it, whether or not the line already had one —
        // a completion in front of existing arguments must not double the gap.
        if !self.typed[name.len()..].starts_with(char::is_whitespace) {
            self.typed.insert(name.len(), ' ');
        }
        self.cursor = name.len() + 1;
        self.edited(authority);
        true
    }

    /// Move the highlight down the popup, wrapping. Answers whether it moved.
    pub(crate) fn highlight_next(&mut self) -> bool {
        self.highlight(1)
    }

    /// Move the highlight up the popup, wrapping. Answers whether it moved.
    pub(crate) fn highlight_previous(&mut self) -> bool {
        self.highlight(-1)
    }

    /// One step round the popup. Wraps, because a list a player can fall off the
    /// end of makes them look for the end.
    fn highlight(&mut self, by: isize) -> bool {
        let Offer::Candidates { of, at } = &mut self.offer else {
            return false;
        };
        let count = of.len() as isize;
        *at = ((*at as isize + by).rem_euclid(count)) as usize;
        true
    }

    /// Escape: put the popup away, or — with no popup — the line itself.
    ///
    /// Answers whether the line is still open. Two meanings on one key and in
    /// that order, which is every editor's rule: the innermost thing showing is
    /// the thing dismissed.
    pub(crate) fn cancel(&mut self, authority: AccessLevel) -> bool {
        if self.offer != Offer::Nothing {
            self.dismissed = true;
            self.refresh(authority);
            return true;
        }
        self.typed.clear();
        self.cursor = 0;
        self.focused = false;
        self.dismissed = false;
        false
    }

    /// Move the caret one `char` left, if it is not already at the start.
    pub(crate) fn left(&mut self) {
        if let Some(before) = self.typed[..self.cursor].chars().next_back() {
            self.cursor -= before.len_utf8();
        }
    }

    /// Move the caret one `char` right, if it is not already at the end.
    pub(crate) fn right(&mut self) {
        if let Some(after) = self.typed[self.cursor..].chars().next() {
            self.cursor += after.len_utf8();
        }
    }

    /// Take the typed line and close it back to empty, or `None` for a stray
    /// Enter on nothing worth sending — the same rule `shell::speech_line` had:
    /// an empty message is not silence worth sending, it is the server drawing
    /// nothing over the player's head.
    pub(crate) fn take(&mut self) -> Option<String> {
        let line = std::mem::take(&mut self.typed);
        self.cursor = 0;
        // The popup belongs to the line that has just left. Both halves of it:
        // an offer against an empty line is nothing, and a refusal of an offer
        // that no longer exists would silence the next command's popup.
        self.offer = Offer::Nothing;
        self.dismissed = false;
        // Submitting a line gives the keyboard back to the game.  Leaving this
        // true made every following hotkey look like more chat input: after
        // asking a vendor to buy, `P` was typed into the empty line instead of
        // reopening the character paperdoll.
        self.focused = false;
        (!line.trim().is_empty()).then_some(line)
    }
}

impl crate::app::App {
    /// What the shard holds this character's authority at, or
    /// [`AccessLevel::Player`] before there is a view to ask.
    ///
    /// The one reader is the speech line's completer. It is asked at every
    /// keystroke rather than copied onto [`Chat`] — see [`Chat::refresh`] — and
    /// the answer is always the view's, never this end's opinion: nothing here
    /// gates anything, the shard refuses what it refuses.
    pub(crate) fn authority(&self) -> AccessLevel {
        self.world
            .authoritative
            .view
            .as_ref()
            .map_or(AccessLevel::default(), |view| view.authority)
    }

    /// The channel button, pressed — answers whether the click was its.
    ///
    /// **Asked before the window layer.** The chat is drawn over every window
    /// this client has (one pass, and it is the last one — see the call in
    /// `presentation.rs`), so a click that lands on both belongs to whatever is
    /// on top, and that is this. A button drawn over a container and picked
    /// under it would be the pointer disagreeing with the picture, which is
    /// `docs/parity.md`'s defect in the one place a player can feel it.
    ///
    /// The box comes out of [`channel_button`] and [`channel_width`], the same
    /// two the frame draws it with, so the two cannot disagree about where it
    /// is. The measurement needs the atlas the glyphs were packed into, which is
    /// why this reads the window and not only the chat.
    pub(crate) fn press_channel_button(&mut self) -> bool {
        let Some(window) = self.window.as_ref() else {
            return false;
        };
        let scale = self.gump_scale();
        let chat_style = self.chat_style();
        // `ttf_atlas` is `Some` exactly when a face is set (`create_window`), so
        // this reads the face rather than the atlas and cannot pick the
        // TrueType arithmetic for a `fonts.mul` frame.
        let truetype = self.resources.ttf_font.is_some();
        let fonts = self.desk.fonts;
        // The size the frame drew this line at, density and all — the pointer
        // has to measure the button against the same glyphs that are on the
        // screen, and those were rasterized at the real size.
        let ttf = window
            .ttf_atlas
            .as_ref()
            .filter(|_| truetype)
            .map(|atlas| (atlas, fonts.speech.scaled(scale)));
        let button = channel_button(
            canvas_height(window.config.height, scale),
            line_height(truetype, chat_style, fonts),
            channel_width(
                &self.resources.font_atlas,
                ttf,
                chat_style.scale.glyph_scale_factor() as i32,
                scale,
            ),
        );
        if !button.contains(self.input.pointer_gump) {
            return false;
        }
        self.chat.channel = self.chat.channel.next();
        true
    }
}

/// The speech line and the journal above it, over the finished picture and
/// under egui's — the same corner `shell::speech_line`'s `egui::Panel::bottom`
/// used to claim before this moved to the client's own rendering. Always
/// drawn, unlike `crate::presentation::draw_gump_windows`: the font atlas
/// needs no shard-sent gump art to exist, so there is nothing here to be
/// `None` until.
///
/// The plainest of this frame's free functions: every parameter is `&`, and
/// the one exception — `text_quads` — is appended to rather than replaced,
/// so the caller keeps owning the one instance buffer `GumpRenderer` has
/// room for (see the comment at this call's site in `App::draw_from`).
/// Nothing here is written back to `self` at all.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_chat_and_speech(
    resources: &resources::Resources,
    world: &world::WorldState,
    chat: &Chat,
    shell: Option<&shell::Shell>,
    window: &mut Screen,
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    chat_style: desk::Chat,
    fonts: desk::FontSizes,
    screen_speech: &[text::ScreenLabel<'_>],
    screen_counts: &[text::ScreenLabel<'_>],
    text_quads: &mut Vec<SpriteQuad>,
    ttf_quads: &mut Vec<SpriteQuad>,
) {
    let scale = shell.map(|shell| shell.pixels_per_point()).unwrap_or(1.0);
    // The two roles this function draws through a TrueType face, each at its
    // own real size — `docs/text_sizes.md`'s D3 — with the display's density
    // folded into the size rather than into the finished quad (D4).
    let speech_size = fonts.speech.scaled(scale);
    let count_size = fonts.stack_count.scaled(scale);
    // The surface's size in gump pixels rather than real ones —
    // `Frame::scale`'s doc is what the one below multiplies out, and
    // this is that arithmetic done once for where the corner is
    // rather than for every quad in it.
    let canvas = GumpPixel::new(
        (window.config.width as f32 / scale) as i32,
        canvas_height(window.config.height, scale),
    );
    let font = Font::DEFAULT;
    let line_height = line_height(resources.ttf_font.is_some(), chat_style, fonts);
    let input_at = GumpPixel::new(CHAT_MARGIN, canvas.y - CHAT_MARGIN - line_height);

    // Owned before it is borrowed into `GumpLabel`s: the journal's own
    // strings are formatted here (name and text joined the way
    // `shell::Hud::said` used to) and the prompt is built from
    // the caller's own chat, so both need somewhere to live for the length of
    // `collect_gump`'s borrow.
    let mut rows: Vec<(GumpPixel, Hue, Font, String)> = Vec::new();
    // How much column there is between the input line and the top of the
    // window, which is what both blocks below are cut to — see [`room_above`].
    // The popup is served first because it is the one a keystroke is moving:
    // the journal is a record and can wait a line, an offer the player is
    // arrowing through cannot.
    let room = room_above(canvas.y, line_height);
    // The completion popup, directly above the input line and below the
    // journal: what is being typed and what it could become read as one block,
    // and the conversation moves up out of the way rather than being drawn over.
    //
    // Bottom-up, so `Offer::rows`' first row — its own top — ends up furthest
    // from the line. The count is carried into the journal's own offset below,
    // which is the whole of "the popup pushes the conversation up".
    let popup = match chat.focused {
        true => chat.offer.rows(room.min(COMPLETION_ROWS)),
        false => Vec::new(),
    };
    let mut above = 0;
    // Which of `rows` the highlighted one is, so the plate can be laid under it
    // below — where the two font paths part company over what a pixel is.
    let mut highlighted: Option<usize> = None;
    for row in popup.iter().rev() {
        above += 1;
        let at = GumpPixel::new(CHAT_MARGIN, input_at.y - line_height * above);
        // Every row in the shard's grey, including the highlighted one: the
        // highlight is the plate behind it now (see [`PLATE_SHADE`]), and a
        // row that changed ink *as well* would be saying the same thing twice —
        // which is what it did while there was no primitive to say it with.
        if row.highlighted {
            highlighted = Some(rows.len());
        }
        rows.push((at, Hue::SYSTEM, font, row.text.clone()));
    }
    // The popup's own rows, which is what the plate is measured across: a bar as
    // wide as the widest offer reads as one list, where a bar cut to the
    // highlighted row's own text would jog in and out as the highlight moves.
    let popup_rows = popup.len();
    if let Some(view) = world.authoritative.view.as_ref() {
        // What the popup left of the column, and never more than the journal's
        // own six: the newest lines, since the walk is `rev`.
        //
        // `saturating_sub` states an invariant rather than guarding a case:
        // `Offer::rows` was given `room` as a hard cap, so it cannot have
        // answered with more. Underflowing here would wrap to a number the
        // `.min` below would happily clamp to six — a silent return of the very
        // defect this reads the window's height to avoid.
        let journal_rows = room.saturating_sub(popup.len()).min(CHAT_LINES);
        for (row, line) in view.journal.iter().rev().take(journal_rows).enumerate() {
            let at = GumpPixel::new(CHAT_MARGIN, input_at.y - line_height * (above + row as i32 + 1));
            let text = match line.name.is_empty() {
                true => line.text.clone(),
                false => format!("{}: {}", line.name, line.text),
            };
            rows.push((at, line.hue, line.font, text));
        }
    }
    // What stands on the input line beside the button: the line as typed, or —
    // with the line shut — the key that opens it.
    //
    // A hint and not an empty line, because there is no mouse click to discover
    // it by (see `App::window_event`'s `Hotkey::Speak` arm). The channel is
    // *not* named here any more: it is the button's own label, drawn whether or
    // not the line is open, which is what a player who left it on `guild` reads
    // without having to open anything.
    let prompt = match chat.focused {
        true => chat.typed.clone(),
        false => "[Enter] to speak".to_owned(),
    };
    let labels: Vec<GumpLabel<'_>> = rows
        .iter()
        .map(|(at, hue, font, text)| GumpLabel {
            at: *at,
            text,
            font: *font,
            hue: *hue,
            clip: None,
        })
        .collect();
    // The channel, on a plate, at the left end of the line — see
    // [`channel_button`] for why this is a control and not a chord.
    let channel_label = chat.channel.label();
    // The caret, a lone glyph rather than a new quad primitive: the
    // gump pass draws through an atlas of packed sprites and has
    // nothing that paints a solid rectangle, and `fonts.mul` already
    // has a `|` to stand in for one — as does every TrueType face,
    // `.notdef` or otherwise (`openshard_uofiles::ttf_font::TtfFont::glyph`'s
    // "never fails" doc). Blinks off wall-clock time rather than a
    // stored `Instant`, so nothing on `Chat` has to track when focus
    // began.
    let caret_text = "|";
    let blink_on = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| (elapsed.as_millis() / 500) % 2 == 0)
        .unwrap_or(true);
    // `fonts.mul` has no Cyrillic past `0xFF` — see `run`'s
    // `--ttf-font` doc — and this is the box a player actually reads
    // what they typed back from, so unlike the dialog captions
    // `text_quads` carries below, this switches to `App::ttf_font`
    // and `Screen::ttf_gump_pass` whenever one is set rather than
    // drawing a line nobody can read the second half of.
    if let Some(font) = &resources.ttf_font {
        let atlas = window
            .ttf_atlas
            .as_mut()
            .expect("create_window builds ttf_atlas whenever ttf_font is set");
        // Every channel's name and not only the one showing: the button is as
        // wide as the widest of the four (see [`channel_width`]), so all four
        // have to be measurable — and a glyph the atlas has not packed measures
        // zero, which would size the button off one frame's worth of letters.
        let wanted = labels
            .iter()
            .flat_map(|label| label.text.chars())
            .chain(prompt.chars())
            .chain(Channel::ALL.iter().flat_map(|channel| channel.label().chars()))
            .chain(std::iter::once('|'))
            .chain(screen_speech.iter().flat_map(|label| label.text.chars()));
        if let Err(error) = atlas.add_or_reset(font, speech_size, wanted) {
            // Same corner as the speech line's own `atlas.add` above.
            eprintln!("packing ttf glyphs: {error}");
        }
        // The counts written over piles, grown at *their* size — the same
        // characters at another size are other glyphs, which is the whole of
        // `TtfAtlas`'s `(char, size)` key. Skipped entirely when nothing on
        // screen is counted, which is most frames.
        if !screen_counts.is_empty() {
            let digits = screen_counts.iter().flat_map(|label| label.text.chars());
            if let Err(error) = atlas.add_or_reset(font, count_size, digits) {
                eprintln!("packing ttf glyphs: {error}");
            }
        }
        // `labels`' own positions are gump pixels, `rows`/`input_at`'s
        // space — real pixels only once here, not per glyph inside
        // `collect_gump_ttf`: see that function's doc for why the
        // earlier per-glyph version read soft and its baseline
        // sawtoothed.
        let to_real = |p: GumpPixel| {
            GumpPixel::new(
                (p.x as f32 * scale).round() as i32,
                (p.y as f32 * scale).round() as i32,
            )
        };
        let mut real_labels: Vec<GumpLabel<'_>> = labels
            .iter()
            .map(|label| GumpLabel {
                at: to_real(label.at),
                ..*label
            })
            .collect();
        // The button's box, in gump pixels like every other layout answer here,
        // and converted once — the pointer reads the same box out of the same
        // two functions, which is the whole of why they are functions.
        let button = channel_button(
            canvas.y,
            line_height,
            channel_width(&resources.font_atlas, Some((&*atlas, speech_size)), 1, scale),
        );
        let line_at = to_real(line_starts_at(button));
        real_labels.push(GumpLabel {
            at: to_real(GumpPixel::new(button.at.x + CHAT_MARGIN, button.at.y)),
            text: channel_label,
            font: Font::DEFAULT,
            hue: Hue(chat_style.hue),
            clip: None,
        });
        real_labels.push(GumpLabel {
            at: line_at,
            text: &prompt,
            font: Font::DEFAULT,
            hue: Hue(chat_style.hue),
            clip: None,
        });
        if chat.focused && blink_on {
            let caret_x = text::gump_width_ttf(&chat.typed[..chat.cursor], atlas, speech_size);
            real_labels.push(GumpLabel {
                at: GumpPixel::new(line_at.x + caret_x, line_at.y),
                text: caret_text,
                font: Font::DEFAULT,
                hue: Hue(chat_style.hue),
                clip: None,
            });
        }
        // The plate first, so the row's own glyphs land on it: this pass has no
        // depth and painter's order is the only order there is. In real pixels
        // like everything else in this branch, which is why the row's height is
        // multiplied here and not in the branch below.
        let mut hud_quads = std::mem::take(ttf_quads);
        hud_quads.push(plate_of(button, scale));
        if let Some(index) = highlighted {
            let widest = rows[..popup_rows]
                .iter()
                .map(|(_, _, _, text)| text::gump_width_ttf(text, atlas, speech_size))
                .max()
                .unwrap_or_default();
            let at = to_real(rows[index].0);
            hud_quads.push(gump_art::plate(
                Rect {
                    x: at.x as f32,
                    y: at.y as f32,
                    width: widest as f32,
                    height: (line_height as f32 * scale).round(),
                },
                Hue::NONE,
                gump_art::Shade::new(PLATE_SHADE),
            ));
        }
        hud_quads.extend(text::collect_gump_ttf(&real_labels, atlas, speech_size));
        // Overhead speech's own quads, folded into this same list
        // rather than a render call of their own — `GumpRenderer::render`'s
        // doc is explicit that a second call the same frame does not
        // add a second draw, it *replaces* the first: the instances
        // live in one buffer written through `queue.write_buffer`,
        // which lands before either call's encoded draw actually
        // runs, so a first, separate `screen_speech` call earlier in
        // the frame was silently overwritten by this one and never
        // drew anything. One call, everything it should draw.
        hud_quads.extend(text::collect_screen_ttf(screen_speech, atlas, speech_size));
        // And the counts, in their own size — the second role this one call
        // draws. One call rather than two passes, for the reason the comment
        // above gives: `GumpRenderer::render` replaces its instances rather
        // than adding to them, so everything that is to be drawn through it
        // this frame has to be in one list.
        hud_quads.extend(text::collect_screen_ttf(screen_counts, atlas, count_size));
        // Picks up this call's own `add` above and, the first time
        // through this frame, the speech line's — see
        // `Screen::upload_ttf_dirty`'s doc.
        window.upload_ttf_dirty();
        let timed = profile::begin(window.gpu.as_ref(), "ttf gump text", encoder);
        window
            .ttf_gump_pass
            .as_mut()
            .expect("create_window builds ttf_gump_pass whenever ttf_atlas is")
            .render(
                &window.device,
                &window.queue,
                encoder,
                gump_art::Frame {
                    target: view,
                    width: window.config.width,
                    height: window.config.height,
                    // Not `scale`: `hud_quads` are already in real
                    // pixels, so the shader's own multiply — the one
                    // `text_quads` below still needs, being in gump
                    // pixels — would double it.
                    scale: 1.0,
                },
                &hud_quads,
            );
        profile::end(window.gpu.as_ref(), encoder, timed);
    } else {
        let magnify = chat_style.scale.glyph_scale_factor() as i32;
        let mut labels = labels;
        // The same two functions the TrueType branch above calls and the pointer
        // calls — measured through `fonts.mul` here, which is the whole of the
        // difference between the two branches.
        let button = channel_button(
            canvas.y,
            line_height,
            channel_width(&resources.font_atlas, None, magnify, scale),
        );
        let line_at = line_starts_at(button);
        labels.push(GumpLabel {
            at: GumpPixel::new(button.at.x + CHAT_MARGIN, button.at.y),
            text: channel_label,
            font,
            hue: Hue(chat_style.hue),
            clip: None,
        });
        labels.push(GumpLabel {
            at: line_at,
            text: &prompt,
            font,
            hue: Hue(chat_style.hue),
            clip: None,
        });
        if chat.focused && blink_on {
            let caret_x = text::gump_width(&chat.typed[..chat.cursor], font, &resources.font_atlas);
            labels.push(GumpLabel {
                // **Magnified**, like the glyphs it is counting past.
                // `gump_width` measures `fonts.mul`'s own pixels and
                // `scaled_gump_quads` draws them at `magnify` times that, each
                // label from its own anchor — so an anchor placed at the
                // unmagnified width put the caret at a fraction of the way along
                // the line it was measuring, which at `ChatScale`'s default of
                // two was halfway back through what had been typed.
                at: GumpPixel::new(line_at.x + caret_x * magnify, line_at.y),
                text: caret_text,
                font,
                hue: Hue(chat_style.hue),
                clip: None,
            });
        }
        // The button's plate, before every label that stands on it — including
        // the journal's, which cannot reach it, and the button's own, which is
        // the point.
        text_quads.push(plate_of(button, 1.0));
        // The plate under the highlighted row, before the text that stands on
        // it — the TrueType branch's own paragraph, in gump pixels: `magnify`
        // is what `fonts.mul`'s measured widths are drawn at, and `line_height`
        // already carries it.
        if let Some(index) = highlighted {
            let widest = rows[..popup_rows]
                .iter()
                .map(|(_, _, row_font, text)| text::gump_width(text, *row_font, &resources.font_atlas))
                .max()
                .unwrap_or_default();
            let at = rows[index].0;
            text_quads.push(gump_art::plate(
                Rect {
                    x: at.x as f32,
                    y: at.y as f32,
                    width: (widest * magnify) as f32,
                    height: line_height as f32,
                },
                Hue::NONE,
                gump_art::Shade::new(PLATE_SHADE),
            ));
        }
        text_quads.extend(scaled_gump_quads(
            &labels,
            &resources.font_atlas,
            chat_style.scale.glyph_scale_factor(),
        ));
    }
    // The one call, with the windows' lines already in front of the
    // chat's: painter's order inside a single pass, and the only order
    // there is — see `text_quads` for what a second call would cost.
    // Draws only the windows' captions when `App::ttf_font` is set:
    // the chat's own quads went through `ttf_gump_pass` above instead.
    let timed = profile::begin(window.gpu.as_ref(), "gump text", encoder);
    window.gump_text_pass.render(
        &window.device,
        &window.queue,
        encoder,
        gump_art::Frame {
            target: view,
            width: window.config.width,
            height: window.config.height,
            scale,
        },
        text_quads,
    );
    profile::end(window.gpu.as_ref(), encoder, timed);
}

#[cfg(test)]
mod tests {
    use openshard_client_render::gump::GumpPixel;
    use openshard_commands::StaffCommand;
    use openshard_protocol::access::AccessLevel;

    use super::{Channel, Chat, Offer};

    /// The authority these tests type under: a game master, because the
    /// completer has nothing at all to offer anybody else — that rule is
    /// `StaffCommand::matching`'s and is tested there, and the one test below
    /// that cares about it names its own level.
    const STAFF: AccessLevel = AccessLevel::GameMaster;

    /// A line as a player would have typed it: the caret at the end, the offer
    /// recomputed, which is the state every method below is entered from.
    fn typing(line: &str) -> Chat {
        typed_by(line, STAFF)
    }

    /// The same, for somebody the shard holds at `authority`.
    fn typed_by(line: &str, authority: AccessLevel) -> Chat {
        let mut chat = Chat {
            focused: true,
            ..Chat::default()
        };
        chat.insert(line, authority);
        chat
    }

    #[test]
    fn submitting_speech_returns_the_keyboard_to_the_game() {
        let mut chat = Chat {
            typed: "buy".to_owned(),
            cursor: 3,
            focused: true,
            channel: Channel::Say,
            ..Chat::default()
        };

        assert_eq!(chat.take().as_deref(), Some("buy"));
        assert!(chat.typed.is_empty());
        assert_eq!(chat.cursor, 0);
        assert!(!chat.focused, "hotkeys must work after a spoken line");
    }

    #[test]
    fn ctrl_backspace_removes_the_preceding_unicode_word_and_its_trailing_space() {
        let mut chat = typing("привет, мир   ");
        chat.backspace_word(STAFF);
        assert_eq!(chat.typed, "привет, ");
        assert_eq!(chat.cursor, "привет, ".len());

        chat.backspace_word(STAFF);
        assert!(chat.typed.is_empty());
        assert_eq!(chat.cursor, 0);
    }

    /// The channel survives a line, and a send does not put it back to `say`.
    /// A channel that reset itself would make a conversation on one of them four
    /// keystrokes a sentence, which is the whole reason it is state rather than
    /// a prefix typed each time.
    #[test]
    fn the_channel_outlives_the_line_it_was_chosen_for() {
        let mut chat = Chat {
            typed: "regroup".to_owned(),
            cursor: 7,
            focused: true,
            channel: Channel::Guild,
            ..Chat::default()
        };
        assert_eq!(chat.take().as_deref(), Some("regroup"));
        assert_eq!(chat.channel, Channel::Guild);
    }

    /// Ordinary speech is not a command and offers nothing: a popup that opened
    /// on "hello" would cover the conversation for every line anybody says.
    #[test]
    fn speech_offers_nothing_and_a_dot_offers_everything() {
        assert_eq!(typing("hello there").offer, Offer::Nothing);
        let Offer::Candidates { of, at } = typing(".").offer else {
            panic!("a lone prefix offers the whole vocabulary");
        };
        assert_eq!(of.len(), StaffCommand::ALL.len());
        assert_eq!(at, 0);
    }

    #[test]
    fn a_prefix_narrows_the_offer_and_a_word_that_matches_nothing_closes_it() {
        let Offer::Candidates { of, .. } = typing(".hd").offer else {
            panic!("three house commands begin with hd");
        };
        assert_eq!(
            of,
            vec![
                StaffCommand::HDemolish,
                StaffCommand::HDesign,
                StaffCommand::HDrop
            ]
        );
        assert_eq!(typing(".zzz").offer, Offer::Nothing);
    }

    /// The whole gesture: type a prefix, arrow to the one you meant, Tab.
    #[test]
    fn tab_takes_the_highlighted_command_and_leaves_a_space_for_its_arguments() {
        let mut chat = typing(".hd");
        assert!(chat.highlight_next(), "hdemolish -> hdesign");
        assert!(chat.complete(STAFF));
        assert_eq!(chat.typed, ".hdesign ");
        assert_eq!(
            chat.cursor,
            chat.typed.len(),
            "the caret is where the argument goes"
        );
        assert_eq!(
            chat.offer,
            Offer::Arguments(StaffCommand::HDesign),
            "and the popup turns into the usage hint"
        );
    }

    /// Completing what is already complete is not a no-op worth guarding: it is
    /// how a player who typed the whole word gets the space and the hint.
    #[test]
    fn tab_on_a_finished_word_still_completes_it() {
        let mut chat = typing(".save");
        assert!(chat.complete(STAFF));
        assert_eq!(chat.typed, ".save ");
    }

    #[test]
    fn tab_does_nothing_when_nothing_is_offered() {
        let mut chat = typing("hello");
        assert!(!chat.complete(STAFF));
        assert_eq!(chat.typed, "hello");
    }

    /// The highlight is kept as the list narrows around it — a completer that
    /// snapped back to the first match would undo the arrow the player just
    /// pressed with the letter they typed next.
    #[test]
    fn narrowing_the_list_keeps_what_was_chosen() {
        let mut chat = typing(".h");
        chat.highlight_next();
        chat.highlight_next();
        let chosen = chat.offer.highlighted().expect("a highlight");
        chat.insert(&chosen.name()[1..2], STAFF);
        assert_eq!(chat.offer.highlighted(), Some(chosen));
    }

    #[test]
    fn the_highlight_wraps_in_both_directions() {
        let mut chat = typing(".hd");
        assert_eq!(chat.offer.highlighted(), Some(StaffCommand::HDemolish));
        chat.highlight_previous();
        assert_eq!(
            chat.offer.highlighted(),
            Some(StaffCommand::HDrop),
            "up from the first is the last"
        );
        chat.highlight_next();
        assert_eq!(chat.offer.highlighted(), Some(StaffCommand::HDemolish));
    }

    /// Escape twice: the popup, then the line. The first must not close the line
    /// — a player dismissing a suggestion has not given up on the sentence.
    #[test]
    fn escape_puts_the_popup_away_before_the_line() {
        let mut chat = typing(".hd");
        assert!(chat.cancel(STAFF), "the line stays open");
        assert_eq!(chat.offer, Offer::Nothing);
        assert_eq!(chat.typed, ".hd", "and keeps what was typed");
        assert!(!chat.cancel(STAFF), "the second closes the line");
        assert!(!chat.focused);
        assert!(chat.typed.is_empty());
    }

    /// A dismissed popup comes back on the next letter: the refusal was about
    /// the word as it stood, and the next keystroke asks a different question.
    #[test]
    fn typing_after_a_dismissal_asks_again() {
        let mut chat = typing(".h");
        chat.cancel(STAFF);
        assert_eq!(chat.offer, Offer::Nothing);
        chat.insert("d", STAFF);
        assert!(matches!(chat.offer, Offer::Candidates { .. }));
    }

    /// The popup does not survive the line it was offered against.
    #[test]
    fn sending_a_line_clears_the_offer() {
        let mut chat = typing(".save");
        assert!(matches!(chat.offer, Offer::Candidates { .. }));
        assert_eq!(chat.take().as_deref(), Some(".save"));
        assert_eq!(chat.offer, Offer::Nothing);
    }

    /// Past the word, the popup stops being a list and becomes the usage line —
    /// the same sentence the shard answers a mistyped command with.
    #[test]
    fn arguments_are_offered_once_the_word_is_settled() {
        assert_eq!(typing(".go ").offer, Offer::Arguments(StaffCommand::Go));
        assert_eq!(typing(".go 1425 1690").offer, Offer::Arguments(StaffCommand::Go));
        assert_eq!(
            typing(".nosuch arg").offer,
            Offer::Nothing,
            "a first word that is no command offers nothing rather than repeating a refusal"
        );
    }

    /// The list is longer than the popup, and what does not fit is counted
    /// rather than dropped — with the highlight always among the rows drawn.
    #[test]
    fn a_long_list_is_windowed_around_the_highlight_and_says_how_many_are_hidden() {
        let mut chat = typing(".");
        let rows = chat.offer.rows(super::COMPLETION_ROWS);
        assert_eq!(
            rows.len(),
            super::COMPLETION_ROWS,
            "the count is one of the rows it was given, not a row on top of them"
        );
        assert!(rows[0].text.contains("more"));
        assert!(rows.iter().any(|row| row.highlighted));
        for _ in 0..StaffCommand::ALL.len() - 1 {
            chat.highlight_next();
            assert!(
                chat.offer
                    .rows(super::COMPLETION_ROWS)
                    .iter()
                    .any(|row| row.highlighted),
                "the highlight is never scrolled out of the popup"
            );
        }
    }

    /// The whole of the fit: a popup asked for `n` rows draws `n`, whatever it
    /// has to say. What a window with no room asks for is nothing at all, and a
    /// popup that answered anyway would be drawing above the top of the screen.
    #[test]
    fn the_popup_never_answers_with_more_rows_than_it_was_given() {
        let long = typing(".");
        let short = typing(".hd");
        let hint = typing(".go ");
        for limit in 0..=super::COMPLETION_ROWS {
            for offer in [&long.offer, &short.offer, &hint.offer] {
                assert!(
                    offer.rows(limit).len() <= limit,
                    "an offer drew {} rows into {limit}",
                    offer.rows(limit).len()
                );
            }
        }
        assert!(long.offer.rows(0).is_empty());
    }

    /// One row of room and twenty-five matches: the count is what that row says.
    /// A lone command drawn out of twenty-five would read as *the* match rather
    /// than as one of them, which is the wrong of the two things to say.
    #[test]
    fn a_single_row_of_room_counts_rather_than_picks() {
        let rows = typing(".").offer.rows(1);
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0]
                .text
                .contains(&format!("{} more", StaffCommand::ALL.len()))
        );
        assert!(!rows[0].highlighted);
    }

    /// The defect this arithmetic was written for: at `ChatScale` 4 on a small
    /// window the chat block ran off the top of the screen. Whatever the window
    /// and whatever the scale, the topmost row drawn starts at or below the
    /// margin — which is the same statement as "every row is on the surface".
    #[test]
    fn no_row_is_ever_laid_out_above_the_top_of_the_window() {
        for canvas_height in [0, 1, 32, 47, 48, 120, 480, 1080] {
            for scale in 1..=4 {
                let line_height = crate::CHAT_LINE_HEIGHT * scale;
                let room = super::room_above(canvas_height, line_height) as i32;
                let input_top = canvas_height - crate::CHAT_MARGIN - line_height;
                let topmost = input_top - line_height * room;
                assert!(
                    room == 0 || topmost >= crate::CHAT_MARGIN,
                    "canvas {canvas_height} at line height {line_height}: \
                     {room} rows put the top one at {topmost}"
                );
                // And it is the *most* that fits: one more would not.
                assert!(
                    topmost - line_height < crate::CHAT_MARGIN,
                    "canvas {canvas_height} at line height {line_height}: \
                     {room} rows leave room for another"
                );
            }
        }
    }

    /// A window too short for a single row above the line still draws the line
    /// itself: the input is the one row that is never dropped, because a client
    /// that hid what is being typed would be a client with no way to speak.
    #[test]
    fn a_window_with_no_room_above_the_line_asks_for_no_rows() {
        assert_eq!(
            super::room_above(crate::CHAT_LINE_HEIGHT * 2, crate::CHAT_LINE_HEIGHT),
            0
        );
        assert_eq!(super::room_above(0, crate::CHAT_LINE_HEIGHT), 0);
        assert_eq!(
            super::room_above(4096, 0),
            0,
            "a line height of zero is no rows, not a division by it"
        );
    }

    /// The button's box and the line's start, which are one arithmetic: what is
    /// typed must never land on the control that would be clicked to change what
    /// it is typed into.
    #[test]
    fn the_channel_button_holds_the_left_of_the_input_line() {
        let line = crate::CHAT_LINE_HEIGHT * 2;
        let button = super::channel_button(480, line, 40);

        assert_eq!(
            button.at.y,
            480 - crate::CHAT_MARGIN - line,
            "the input line's own row, which is the row a player is looking at"
        );
        assert_eq!(button.height, line, "and exactly as tall as it");
        assert!(
            button.contains(GumpPixel::new(button.at.x, button.at.y)),
            "its own corner"
        );
        assert!(
            !button.contains(GumpPixel::new(button.at.x + button.width, button.at.y)),
            "and not the column one past its right edge — the box is half open"
        );
        assert!(
            !button.contains(GumpPixel::new(button.at.x, button.at.y - 1)),
            "nor the row above it, which is the popup's"
        );

        let line_at = super::line_starts_at(button);
        assert!(
            line_at.x >= button.at.x + button.width,
            "what is typed starts past the button, not on it"
        );
        assert_eq!(line_at.y, button.at.y, "on the same row");
    }

    /// The button is as wide as the *widest* channel, so that pressing it does
    /// not move it: a control that changed size under the pointer would be one
    /// a player cannot press twice in a row.
    #[test]
    fn the_button_is_one_size_whatever_channel_it_is_showing() {
        let widest = Channel::ALL
            .iter()
            .map(|channel| channel.label().len())
            .max()
            .expect("four channels");
        assert_eq!(
            Channel::Alliance.label().len(),
            widest,
            "alliance is the long one; if that changes, the button's width is measured over all four anyway"
        );
    }

    /// The line still says whatever was typed — a player may say ".hello" out
    /// loud, and the shard treats it as speech — but nothing is offered to
    /// complete it with. The rule is the vocabulary's; what this pins is that
    /// the speech line asks it at all, and asks it with the shard's own answer.
    #[test]
    fn an_ordinary_player_is_offered_no_commands() {
        let player = typed_by(".hd", AccessLevel::Player);
        assert_eq!(player.offer, Offer::Nothing);
        assert_eq!(player.typed, ".hd", "and the line is untouched");
        assert!(
            matches!(typing(".hd").offer, Offer::Candidates { .. }),
            "the same keystrokes from staff do offer"
        );
    }

    /// Past the command word the popup is a usage hint, and that is an offer
    /// too: a player who may not run `.go` must not be shown how to.
    #[test]
    fn an_ordinary_player_is_not_shown_a_usage_hint_either() {
        assert_eq!(typed_by(".go ", AccessLevel::Player).offer, Offer::Nothing);
        assert_eq!(typing(".go ").offer, Offer::Arguments(StaffCommand::Go));
    }

    #[test]
    fn the_channels_cycle_and_come_back_round() {
        let mut channel = Channel::default();
        assert_eq!(channel, Channel::Say);
        let mut seen = vec![channel];
        for _ in 0..Channel::ALL.len() - 1 {
            channel = channel.next();
            seen.push(channel);
        }
        assert_eq!(seen, Channel::ALL.to_vec());
        assert_eq!(channel.next(), Channel::Say, "and wraps back to the default");
    }
}
