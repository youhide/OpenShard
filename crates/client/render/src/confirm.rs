//! The client's own yes/no window: one question, two buttons, and no wire.
//!
//! This is the reference client's `QuestionGump` — the small `0x0816` plate with
//! a Cancel and an Okay button on it — and it is the one window shape this
//! client had nowhere to put. Everything else it draws is either the shard's
//! (`0xB0`, whose layout says where every button goes) or a catalogue of
//! something in the view; a question this end asks on its own behalf is neither,
//! and until this module it was an `egui::Window` floating over the gump layer
//! with its own font, its own frame and its own idea of where a button is.
//!
//! # What this module does not know
//!
//! *What is being asked*, and *what either answer means*. It takes a finished
//! string and hands back pictures, hit rectangles and lines — the same contract
//! [`crate::status`] and [`crate::vendor`] have, and the reason the party
//! invitation's wording and its two packets live in `panes::confirm` instead.
//! A second question reuses every pixel here and adds nothing to it.
//!
//! # The positions are the reference's, taken as written
//!
//! `QuestionGump`'s own numbers: the message at `(33, 30)` in a 165-pixel
//! column, Cancel at `(37, 75)` and Okay at `(100, 75)`. They are not derived
//! from the plate's size — the two buttons are different widths (56 and 46) and
//! the gap between them is not the difference — so computing them "properly"
//! from the 178×108 background moves both off the recesses the art draws for
//! them.

use openshard_protocol::speech::Font;
use openshard_protocol::wire::{Graphic, Hue};

use crate::atlas::FontAtlas;
use crate::gump::{GumpArt, GumpAtlas, GumpPixel, Picture, PictureIndex};
use crate::text::{self, GumpLabel};

/// The plate every question is written on: 178×108 in the client this app
/// reads.
const BACKGROUND: Graphic = Graphic(0x0816);

/// The left button — Cancel in the reference, "no" here — as up and pressed
/// faces. `0x0819` is the third face the reference uses for hover, which
/// nothing in this client draws for any button.
const NO: (Graphic, Graphic) = (Graphic(0x0817), Graphic(0x0818));

/// The right button: Okay in the reference, "yes" here.
const YES: (Graphic, Graphic) = (Graphic(0x081A), Graphic(0x081B));

/// Where the question is written, and how wide a line of it may be before it
/// wraps.
const TEXT_AT: GumpPixel = GumpPixel::new(33, 30);
const TEXT_WIDTH: i32 = 165;

const NO_AT: GumpPixel = GumpPixel::new(37, 75);
const YES_AT: GumpPixel = GumpPixel::new(100, 75);

/// The face the reference writes a question in, and its hue.
pub const FONT: Font = Font(1);
const HUE: Hue = Hue(0x0386);

/// The gap between two wrapped lines, when the font atlas cannot be asked.
const FALLBACK_LINE_STEP: i32 = 14;

/// What a press on this window means.
///
/// Two arms and no third: a question with a "maybe" on it is a different window.
/// Dismissing one — the right button over it, or Escape — is the manager's
/// gesture and not a hit, the same as for every other window kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    /// The right-hand button: Okay in the reference's own art.
    Yes,
    /// The left-hand one: Cancel.
    No,
}

/// One wrapped line of the question, already placed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Line {
    pub at: GumpPixel,
    pub text: String,
}

impl Line {
    /// Draw it in the plate's own face and hue — which is the whole of what
    /// this window lets a caller choose about its text: nothing.
    #[must_use]
    pub fn label(&self) -> GumpLabel<'_> {
        GumpLabel {
            at: self.at,
            text: &self.text,
            font: FONT,
            hue: HUE,
            clip: None,
        }
    }
}

/// A yes/no window laid out for one frame.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Window {
    /// The plate and the two buttons, in painter's order.
    pub pictures: Vec<Picture>,
    /// The question written over them.
    pub lines: Vec<Line>,
    /// Which of the pictures answer the mouse.
    ///
    /// [`crate::gump::Window::hits`]'s shape, with a `Vec` instead of a map
    /// because there are two of them: an index into `pictures` and what a press
    /// on it means, so that what is drawn and what is clicked are one list.
    hits: Vec<(PictureIndex, Hit)>,
}

impl Window {
    /// Which button owns `cursor`, if either does.
    ///
    /// Against the button's whole rectangle rather than its opaque texels, for
    /// [`crate::gump::Window::hit`]'s reason: the reference's `Button` owns its
    /// bounds, and testing the ink turns a bevelled margin into a dead zone.
    #[must_use]
    pub fn hit(&self, cursor: GumpPixel, atlas: &GumpAtlas) -> Option<Hit> {
        self.hits.iter().rev().find_map(|(index, hit)| {
            let picture = self.pictures.get(index.position())?;
            let sprite = atlas.sprite(picture.graphic)?;
            (cursor.x >= picture.at.x
                && cursor.y >= picture.at.y
                && cursor.x < picture.at.x + i32::from(sprite.width)
                && cursor.y < picture.at.y + i32::from(sprite.height))
            .then_some(*hit)
        })
    }
}

/// Every graphic this window can draw, whichever button is down.
///
/// Both faces of both buttons and not just the ones on screen: a face packed on
/// the frame a button goes down is a button that is drawn blank for that frame.
pub fn art_of() -> impl Iterator<Item = GumpArt> {
    [BACKGROUND, NO.0, NO.1, YES.0, YES.1]
        .into_iter()
        .map(GumpArt::Gump)
}

/// Lay the question out at `at`, with `held` drawn pressed.
///
/// `held` is the mouse and nothing else — see [`crate::gump::window`]'s own
/// `held` — so what looks pressed and what the release will act on are one
/// value.
///
/// `fonts` is read only to measure the message: the plate is a fixed size and
/// the question has to wrap inside it, so a line's width is the one thing here
/// that cannot be a constant.
#[must_use]
pub fn window(message: &str, held: Option<Hit>, at: GumpPixel, fonts: &FontAtlas) -> Window {
    let face = |hit: Hit, (up, down): (Graphic, Graphic)| {
        if held == Some(hit) { down } else { up }
    };
    let pictures = vec![
        Picture::plain(GumpArt::Gump(BACKGROUND), at),
        Picture::plain(GumpArt::Gump(face(Hit::No, NO)), at.offset(NO_AT)),
        Picture::plain(GumpArt::Gump(face(Hit::Yes, YES)), at.offset(YES_AT)),
    ];
    let step = line_step(fonts);
    Window {
        lines: wrap(message, fonts)
            .into_iter()
            .enumerate()
            .map(|(row, text)| Line {
                at: at.offset(GumpPixel::new(TEXT_AT.x, TEXT_AT.y + row as i32 * step)),
                text,
            })
            .collect(),
        // The indices the two buttons were pushed at, above. Written out rather
        // than searched for, because `pictures` is built right here and a lookup
        // would be this list asking itself a question it already knows.
        hits: vec![(PictureIndex::new(1), Hit::No), (PictureIndex::new(2), Hit::Yes)],
        pictures,
    }
}

/// How far apart two wrapped lines sit.
///
/// The tallest glyph plus two, which is what the tooltip stack measures its own
/// rows by — a client with no `fonts.mul` at all gets a plausible constant
/// instead of every line on top of the last.
fn line_step(fonts: &FontAtlas) -> i32 {
    fonts
        .glyph(FONT, b'M')
        .map_or(FALLBACK_LINE_STEP, |sprite| i32::from(sprite.height) + 2)
}

/// Break `message` into lines that fit the plate's column.
///
/// By whole words, and a word wider than the column on its own gets a line to
/// itself and overflows it — which is the honest failure: a name is one word,
/// and cutting it in half to fit would be this client editing what it was told.
fn wrap(message: &str, fonts: &FontAtlas) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in message.split_whitespace() {
        match lines.last_mut() {
            Some(line) if text::gump_width(&format!("{line} {word}"), FONT, fonts) <= TEXT_WIDTH => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A font atlas with no glyphs in it, which is what every test here wants:
    /// the wrap is exercised for the property that survives any advance table —
    /// every word, in order — and not for where one particular install's `M`
    /// happens to break the line.
    fn fonts() -> FontAtlas {
        FontAtlas::pack([]).expect("no glyphs fit any atlas")
    }

    /// The plate and both buttons are drawn, and the two that answer the mouse
    /// are the two that answer it — the hit table indexes the pictures beside
    /// it and not a second walk.
    #[test]
    fn the_plate_carries_two_buttons_at_the_reference_positions() {
        let window = window("Join?", None, GumpPixel::new(300, 200), &fonts());
        assert_eq!(
            window
                .pictures
                .iter()
                .map(|picture| (picture.graphic, picture.at))
                .collect::<Vec<_>>(),
            vec![
                (GumpArt::Gump(BACKGROUND), GumpPixel::new(300, 200)),
                (GumpArt::Gump(NO.0), GumpPixel::new(337, 275)),
                (GumpArt::Gump(YES.0), GumpPixel::new(400, 275)),
            ]
        );
        assert_eq!(
            window.hits,
            vec![(PictureIndex::new(1), Hit::No), (PictureIndex::new(2), Hit::Yes)]
        );
    }

    /// A held button is drawn in its pressed face, and only that one is.
    #[test]
    fn holding_a_button_swaps_its_face_alone() {
        let window = window("Join?", Some(Hit::Yes), GumpPixel::new(0, 0), &fonts());
        assert_eq!(window.pictures[1].graphic, GumpArt::Gump(NO.0));
        assert_eq!(window.pictures[2].graphic, GumpArt::Gump(YES.1));
    }

    /// Every word survives the wrap, in order — the property a column of text
    /// has to keep however wide the glyphs turn out to be.
    #[test]
    fn wrapping_keeps_every_word_and_their_order() {
        let message = "0x0000002A has invited you to a party.";
        let lines = wrap(message, &fonts());
        assert_eq!(
            lines.join(" ").split_whitespace().collect::<Vec<_>>(),
            message.split_whitespace().collect::<Vec<_>>()
        );
    }

    /// An empty question draws no line at all rather than one empty one: there
    /// is nothing to say, and a blank row would still take the plate's height.
    #[test]
    fn an_empty_question_writes_nothing() {
        let window = window("   ", None, GumpPixel::new(0, 0), &fonts());
        assert!(window.lines.is_empty());
    }
}
