//! The typed line and its rendering, together: [`Chat`] is what has not been
//! sent yet, and [`draw_chat_and_speech`] is the speech line and the journal
//! above it, over the finished picture and under egui's.

use openshard_client_render::gump::{self as gump_art, GumpPixel};
use openshard_client_render::sprite::SpriteQuad;
use openshard_client_render::text::{self, GumpLabel};
use openshard_commands::{PREFIX, StaffCommand};
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
/// this is that dropdown: the prompt already draws the channel's name, so
/// cycling it with a key costs no new widget and no new hit test, and a player
/// can always see which channel they are about to speak on rather than
/// discovering it after pressing Enter.
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
const COMPLETION_ROWS: usize = CHAT_LINES;

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
    /// Whether it is the one Tab would take. Drawn in the player's own chat
    /// hue, where the rest are drawn in the shard's grey — the gump pass has no
    /// primitive that paints a solid rectangle (see the caret in
    /// [`draw_chat_and_speech`]), so a highlight is ink and a marker rather than
    /// a bar.
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
    #[must_use]
    pub(crate) fn rows(&self) -> Vec<Row> {
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
                // The window scrolls with the highlight rather than sitting at
                // the top: a player arrowing down past the eighth match must not
                // be moving a highlight they cannot see.
                let start = at.saturating_sub(COMPLETION_ROWS - 1).min(of.len() - 1);
                let end = (start + COMPLETION_ROWS).min(of.len());
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
    /// Where the next line goes. Cycled with **Shift+Tab** while the line has
    /// the keyboard, and **kept** across sends — see [`Channel`], and note that
    /// a channel which reset itself after every line would make a conversation
    /// on one channel four keystrokes a sentence.
    ///
    /// It was plain Tab until the completer took that key: a channel is chosen
    /// once a conversation and a command completed once a word, so the cheaper
    /// gesture goes to the commoner act. The intended end state is neither —
    /// a button on screen, which is what the reference client's dropdown is.
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
    pub(crate) fn insert(&mut self, text: &str) {
        self.typed.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.edited();
    }

    /// Delete the `char` before the caret, if any.
    pub(crate) fn backspace(&mut self) {
        let Some(before) = self.typed[..self.cursor].chars().next_back() else {
            return;
        };
        let start = self.cursor - before.len_utf8();
        self.typed.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.edited();
    }

    /// Delete the `char` after the caret, if any.
    pub(crate) fn delete(&mut self) {
        let Some(after) = self.typed[self.cursor..].chars().next() else {
            return;
        };
        let end = self.cursor + after.len_utf8();
        self.typed.replace_range(self.cursor..end, "");
        self.edited();
    }

    /// The line changed under the player's own hand: a dismissed popup is asked
    /// again, and the offer is recomputed.
    fn edited(&mut self) {
        self.dismissed = false;
        self.refresh();
    }

    /// Recompute [`offer`](Self::offer) from the line.
    ///
    /// The one place the popup's contents are decided, and it reads nothing but
    /// `typed` and `dismissed`. Note what it does *not* read: the caret. An
    /// offer that followed the caret would flicker as it was walked back through
    /// a finished line, and Tab replaces the command word wherever the caret
    /// stands — which is the only word a `.` line has that can be completed.
    fn refresh(&mut self) {
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
                let of = StaffCommand::matching(body);
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
    pub(crate) fn complete(&mut self) -> bool {
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
        self.edited();
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
    pub(crate) fn cancel(&mut self) -> bool {
        if self.offer != Offer::Nothing {
            self.dismissed = true;
            self.refresh();
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
    screen_speech: &[text::ScreenLabel<'_>],
    text_quads: &mut Vec<SpriteQuad>,
) {
    let scale = shell.map(|shell| shell.pixels_per_point()).unwrap_or(1.0);
    // The surface's size in gump pixels rather than real ones —
    // `Frame::scale`'s doc is what the one below multiplies out, and
    // this is that arithmetic done once for where the corner is
    // rather than for every quad in it.
    let canvas = GumpPixel::new(
        (window.config.width as f32 / scale) as i32,
        (window.config.height as f32 / scale) as i32,
    );
    let font = Font::DEFAULT;
    // The TrueType path draws at [`TTF_BASE_PIXEL_HEIGHT`] regardless
    // of [`desk::ChatScale`] — see [`scaled_gump_quads`]'s doc for
    // why an integer upscale is right for `fonts.mul` and wrong for an
    // antialiased face — so the line spacing only grows when the
    // glyphs it is spacing actually will.
    let line_height = match resources.ttf_font {
        Some(_) => CHAT_LINE_HEIGHT,
        None => CHAT_LINE_HEIGHT * chat_style.scale.glyph_scale_factor() as i32,
    };
    let input_at = GumpPixel::new(CHAT_MARGIN, canvas.y - CHAT_MARGIN - line_height);

    // Owned before it is borrowed into `GumpLabel`s: the journal's own
    // strings are formatted here (name and text joined the way
    // `shell::Hud::said` used to) and the prompt is built from
    // the caller's own chat, so both need somewhere to live for the length of
    // `collect_gump`'s borrow.
    let mut rows: Vec<(GumpPixel, Hue, Font, String)> = Vec::new();
    // The completion popup, directly above the input line and below the
    // journal: what is being typed and what it could become read as one block,
    // and the conversation moves up out of the way rather than being drawn over.
    //
    // Bottom-up, so `Offer::rows`' first row — its own top — ends up furthest
    // from the line. The count is carried into the journal's own offset below,
    // which is the whole of "the popup pushes the conversation up".
    let popup = match chat.focused {
        true => chat.offer.rows(),
        false => Vec::new(),
    };
    let mut above = 0;
    for row in popup.iter().rev() {
        above += 1;
        let at = GumpPixel::new(CHAT_MARGIN, input_at.y - line_height * above);
        // The highlighted row in the player's own chat ink and the rest in the
        // shard's grey — see `chat::Row::highlighted` for why the highlight is
        // ink rather than a bar behind it.
        let hue = match row.highlighted {
            true => Hue(chat_style.hue),
            false => Hue::SYSTEM,
        };
        rows.push((at, hue, font, row.text.clone()));
    }
    if let Some(view) = world.authoritative.view.as_ref() {
        for (row, line) in view.journal.iter().rev().take(CHAT_LINES).enumerate() {
            let at = GumpPixel::new(CHAT_MARGIN, input_at.y - line_height * (above + row as i32 + 1));
            let text = match line.name.is_empty() {
                true => line.text.clone(),
                false => format!("{}: {}", line.name, line.text),
            };
            rows.push((at, line.hue, line.font, text));
        }
    }
    let prompt = match chat.focused {
        // The channel's own name, which is the whole of its UI: there is no
        // widget, and a player has to be able to see what they are about to
        // speak on *before* they press Enter.
        true => format!("{}: {}", chat.channel.label(), chat.typed),
        // A hint and not an empty line: there is no mouse click to
        // discover this by any more (see `App::window_event`'s
        // `KeyCode::Enter` arm), so the one thing worth saying here is
        // the key that opens it. The channel is named even when shut, because
        // it survives a send and a player who left it on `guild` should not
        // have to open the line to find that out.
        false => match chat.channel {
            Channel::Say => "[Enter] say".to_owned(),
            other => format!("[Enter] {}", other.label()),
        },
    };
    let mut labels: Vec<GumpLabel<'_>> = rows
        .iter()
        .map(|(at, hue, font, text)| GumpLabel {
            at: *at,
            text,
            font: *font,
            hue: *hue,
            clip: None,
        })
        .collect();
    labels.push(GumpLabel {
        at: input_at,
        text: &prompt,
        font,
        hue: Hue(chat_style.hue),
        clip: None,
    });
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
        let wanted = labels
            .iter()
            .flat_map(|label| label.text.chars())
            .chain(std::iter::once('|'));
        if let Err(error) = atlas.add(font, wanted) {
            // Same corner as the speech line's own `atlas.add` above.
            eprintln!("packing ttf glyphs: {error}");
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
        // The channel's own prefix, not the constant `"say: "` this used to
        // measure: the caret would sit under the wrong letter the moment the
        // prompt said "alliance".
        let prefix = format!("{}: ", chat.channel.label());
        let prefix_width = text::gump_width_ttf(&prefix, atlas);
        if chat.focused && blink_on {
            let real_input_at = to_real(input_at);
            let caret_x = prefix_width + text::gump_width_ttf(&chat.typed[..chat.cursor], atlas);
            real_labels.push(GumpLabel {
                at: GumpPixel::new(real_input_at.x + caret_x, real_input_at.y),
                text: caret_text,
                font: Font::DEFAULT,
                hue: Hue(chat_style.hue),
                clip: None,
            });
        }
        let mut hud_quads = text::collect_gump_ttf(&real_labels, atlas);
        // Overhead speech's own quads, folded into this same list
        // rather than a render call of their own — `GumpRenderer::render`'s
        // doc is explicit that a second call the same frame does not
        // add a second draw, it *replaces* the first: the instances
        // live in one buffer written through `queue.write_buffer`,
        // which lands before either call's encoded draw actually
        // runs, so a first, separate `screen_speech` call earlier in
        // the frame was silently overwritten by this one and never
        // drew anything. One call, everything it should draw.
        hud_quads.extend(text::collect_screen_ttf(screen_speech, atlas));
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
        // See the TrueType path above: the prefix is the channel's.
        let prefix = format!("{}: ", chat.channel.label());
        let prefix_width = text::gump_width(&prefix, font, &resources.font_atlas);
        if chat.focused && blink_on {
            let caret_x =
                prefix_width + text::gump_width(&chat.typed[..chat.cursor], font, &resources.font_atlas);
            labels.push(GumpLabel {
                at: GumpPixel::new(input_at.x + caret_x, input_at.y),
                text: caret_text,
                font,
                hue: Hue(chat_style.hue),
                clip: None,
            });
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
    use openshard_commands::StaffCommand;

    use super::{Channel, Chat, Offer};

    /// A line as a player would have typed it: the caret at the end, the offer
    /// recomputed, which is the state every method below is entered from.
    fn typing(line: &str) -> Chat {
        let mut chat = Chat {
            focused: true,
            ..Chat::default()
        };
        chat.insert(line);
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
        assert!(chat.complete());
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
        assert!(chat.complete());
        assert_eq!(chat.typed, ".save ");
    }

    #[test]
    fn tab_does_nothing_when_nothing_is_offered() {
        let mut chat = typing("hello");
        assert!(!chat.complete());
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
        chat.insert(&chosen.name()[1..2]);
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
        assert!(chat.cancel(), "the line stays open");
        assert_eq!(chat.offer, Offer::Nothing);
        assert_eq!(chat.typed, ".hd", "and keeps what was typed");
        assert!(!chat.cancel(), "the second closes the line");
        assert!(!chat.focused);
        assert!(chat.typed.is_empty());
    }

    /// A dismissed popup comes back on the next letter: the refusal was about
    /// the word as it stood, and the next keystroke asks a different question.
    #[test]
    fn typing_after_a_dismissal_asks_again() {
        let mut chat = typing(".h");
        chat.cancel();
        assert_eq!(chat.offer, Offer::Nothing);
        chat.insert("d");
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
        let rows = chat.offer.rows();
        assert_eq!(rows.len(), super::COMPLETION_ROWS + 1, "eight rows and a count");
        assert!(rows[0].text.contains("more"));
        assert!(rows.iter().any(|row| row.highlighted));
        for _ in 0..StaffCommand::ALL.len() - 1 {
            chat.highlight_next();
            assert!(
                chat.offer.rows().iter().any(|row| row.highlighted),
                "the highlight is never scrolled out of the popup"
            );
        }
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
