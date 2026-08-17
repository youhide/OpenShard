//! The amount picker a Shift-drag opens over a stack — the client's own
//! `SplitMenuGump`, laid out from the art the client ships rather than drawn as
//! a panel over it.
//!
//! It is one background, one slider knob, one button and one number, and every
//! coordinate below is the reference client's own: the picture is `0x085C`
//! (164×74), the knob is `0x0845` (15×15) sliding along a 105-pixel bar whose
//! trough is painted *into* the background, and the button is the three faces
//! `0x085D`, `0x085E` and `0x085F` at `(102, 37)`. Nothing here is measured from
//! the art at run time, for [`crate::status`]'s reason: the reference's numbers
//! are what the art was drawn against, and arithmetic over the atlas would agree
//! with them only by luck.
//!
//! This module knows nothing about which item is being divided, which press the
//! prompt is standing over, or what the answer will do — see
//! `crate::panes::split` on the app side, which owns all three.

use openshard_protocol::speech::Font;
use openshard_protocol::wire::{Graphic, Hue};

use crate::gump::{GumpArt, GumpPixel, Picture};
use crate::text::GumpLabel;

/// The window's background, which carries the slider's trough and the box the
/// number is written in.
const BACKGROUND: Graphic = Graphic(0x085C);

/// The three faces of the one button: at rest, under the pointer, and held.
const OK: Graphic = Graphic(0x085D);
const OK_OVER: Graphic = Graphic(0x085F);
const OK_PRESSED: Graphic = Graphic(0x085E);

/// The knob that slides along the trough.
const KNOB: Graphic = Graphic(0x0845);

/// How wide the knob is. The travel is the bar less this, which is what keeps
/// the knob inside the trough at the top of its range — the reference's own
/// `CalculateOffset`, which subtracts the same width before it scales.
const KNOB_WIDTH: i32 = 15;

/// And how tall, which is the height of the strip a press counts as the
/// slider's.
const KNOB_HEIGHT: i32 = 15;

/// Where the trough starts, and how long it is.
const SLIDER_AT: GumpPixel = GumpPixel::new(29, 16);
const BAR_WIDTH: i32 = 105;

/// Where the button sits, and how big its three faces are — all three the same
/// size, which is what lets one box answer for whichever is drawn.
const OK_AT: GumpPixel = GumpPixel::new(102, 37);
const OK_WIDTH: i32 = 46;
const OK_HEIGHT: i32 = 21;

/// Where the number is written, and the box it is cropped to.
const NUMBER_AT: GumpPixel = GumpPixel::new(29, 42);
const NUMBER_SIZE: (i32, i32) = (60, 20);

/// The face the number is written in, and its hue — the reference's
/// `StbTextBox(1, isunicode: false, hue: 0x0386)`.
const FONT: Font = Font(1);
const HUE: Hue = Hue(0x0386);

/// How far the knob may travel: the bar less the knob's own width.
const TRAVEL: i32 = BAR_WIDTH - KNOB_WIDTH;

/// What the pointer is on, when it is on one of this window's controls.
///
/// The background is not an arm: a press that lands on neither control is what
/// picks the window up, and saying so is the caller's business rather than a
/// third thing to be *on*.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    /// The trough or the knob, which are one control: pressing anywhere along
    /// the bar moves the knob there, exactly as the reference's `CalculateNew`
    /// does.
    Slider,
    /// The one button, which is the answer.
    Ok,
}

/// How the button is drawn, which is not a fact about the layout — it is where
/// the pointer is and whether it is down.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Face {
    /// Nothing is on it.
    #[default]
    Rest,
    /// The pointer is over it.
    Over,
    /// And the button is down.
    Pressed,
}

impl Face {
    /// The picture this face is drawn as.
    const fn graphic(self) -> Graphic {
        match self {
            Self::Rest => OK,
            Self::Over => OK_OVER,
            Self::Pressed => OK_PRESSED,
        }
    }
}

/// One line this window writes — the chosen number, and nothing else.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Line {
    pub at: GumpPixel,
    pub text: String,
}

impl Line {
    /// Draw it in the reference's own face and hue, cropped to its box so that
    /// a five-figure pile cannot write past the frame it is in.
    #[must_use]
    pub fn label(&self) -> GumpLabel<'_> {
        GumpLabel {
            at: self.at,
            text: &self.text,
            font: FONT,
            hue: HUE,
            clip: Some(NUMBER_SIZE),
        }
    }
}

/// The amount picker laid out for one frame.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Window {
    /// The background, the knob and the button, in painter's order.
    pub pictures: Vec<Picture>,
    /// The number, written over them.
    pub lines: Vec<Line>,
}

/// Every picture this window can draw, so the atlas can be grown for it before
/// it is laid out.
///
/// Five, and a frame only ever draws three of them: the button changes picture
/// under the pointer, and an atlas grown for the face that is drawn *now* would
/// pack the next one on the frame it is first needed — a button that flickers
/// blank the first time it is hovered.
pub const ART: [GumpArt; 5] = [
    GumpArt::Gump(BACKGROUND),
    GumpArt::Gump(KNOB),
    GumpArt::Gump(OK),
    GumpArt::Gump(OK_OVER),
    GumpArt::Gump(OK_PRESSED),
];

/// Lay the picker out at `at`, showing `amount` out of `most`.
///
/// `most` is the largest number that may be chosen — the pile less the one that
/// stays behind, which is the app side's rule and not this module's — and
/// `amount` is where the knob is. A `most` of one leaves the knob at the start
/// of its travel, which is also the end of it: there is exactly one number to
/// choose and the slider says so by not moving.
#[must_use]
pub fn window(amount: u16, most: u16, face: Face, at: GumpPixel) -> Window {
    Window {
        pictures: vec![
            Picture::plain(GumpArt::Gump(BACKGROUND), at),
            Picture::plain(
                GumpArt::Gump(KNOB),
                at.offset(GumpPixel::new(
                    SLIDER_AT.x + knob_offset(amount, most),
                    SLIDER_AT.y,
                )),
            ),
            Picture::plain(GumpArt::Gump(face.graphic()), at.offset(OK_AT)),
        ],
        lines: vec![Line {
            at: at.offset(NUMBER_AT),
            text: amount.to_string(),
        }],
    }
}

/// How far along its travel the knob stands for `amount`.
///
/// The reference's `CalculateOffset` in one expression, with its percentage
/// left out: a number scaled to a hundred and back is the same number, and the
/// round trip is what loses a pixel at the top of a long bar.
#[must_use]
pub fn knob_offset(amount: u16, most: u16) -> i32 {
    let span = i32::from(most).saturating_sub(1);
    if span <= 0 {
        return 0;
    }
    let value = i32::from(amount).saturating_sub(1).clamp(0, span);
    (TRAVEL * value / span).clamp(0, TRAVEL)
}

/// And the inverse: the number the knob stands for when it is dragged to `x`,
/// measured from the window's own left edge.
///
/// The reference measures from the *trough's* left edge and does not subtract
/// half the knob, so the knob's own left corner follows the pointer rather than
/// its middle. That is copied rather than corrected: it is what a player who
/// has used the reference client expects the bar to feel like.
#[must_use]
pub fn amount_at(x: i32, most: u16) -> u16 {
    let span = i32::from(most).saturating_sub(1);
    if span <= 0 {
        return 1;
    }
    let along = (x - SLIDER_AT.x).clamp(0, TRAVEL);
    let value = 1 + (span * along + TRAVEL / 2) / TRAVEL;
    value.clamp(1, i32::from(most)) as u16
}

/// Which of this window's two controls `cursor` is on, if either.
///
/// Box tests and not a texel pick, for both: the trough is painted into the
/// background — there is no picture of it to pick against — and the button is a
/// rectangle whose art fills its own box, so the two answers cannot differ by
/// anything a player could aim at.
#[must_use]
pub fn hit(cursor: GumpPixel) -> Option<Hit> {
    let inside = |at: GumpPixel, width: i32, height: i32| {
        (at.x..at.x + width).contains(&cursor.x) && (at.y..at.y + height).contains(&cursor.y)
    };
    if inside(OK_AT, OK_WIDTH, OK_HEIGHT) {
        return Some(Hit::Ok);
    }
    if inside(SLIDER_AT, BAR_WIDTH, KNOB_HEIGHT) {
        return Some(Hit::Slider);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The knob stands at both ends of its travel and nowhere outside it, and
    /// the two ends are the two ends of the *choice* — one, and the whole of
    /// what may be taken.
    #[test]
    fn the_knob_spans_its_travel_and_no_further() {
        assert_eq!(knob_offset(1, 100), 0);
        assert_eq!(knob_offset(100, 100), TRAVEL);
        assert_eq!(knob_offset(200, 100), TRAVEL, "a number past the pile is clamped");
        assert_eq!(knob_offset(1, 1), 0, "one choice, and the bar says so");
    }

    /// Dragging the knob and reading it back is the same number — **for a pile
    /// the bar has pixels for**, which is 91 of them and not one more.
    ///
    /// A pile bigger than the travel cannot round-trip and it is not a defect
    /// that it does not: there are more numbers than positions, so the bar
    /// names one number per pixel and the keyboard names the rest. That is the
    /// reference's arrangement too — its text box is what a player types an
    /// exact figure into — and it is why the box below is a control and not a
    /// readout.
    #[test]
    fn the_bar_reads_back_what_it_was_dragged_to() {
        for amount in [1_u16, 2, 25, 49, 50] {
            let x = SLIDER_AT.x + knob_offset(amount, 50);
            assert_eq!(
                amount_at(x, 50),
                amount,
                "amount {amount} did not survive the trip"
            );
        }
        // The ends still hold for a pile past the bar's resolution, which is
        // what "take one" and "take all but one" are.
        assert_eq!(amount_at(SLIDER_AT.x + knob_offset(1, 60_000), 60_000), 1);
        assert_eq!(
            amount_at(SLIDER_AT.x + knob_offset(60_000, 60_000), 60_000),
            60_000
        );
    }

    /// A press outside the bar is still the bar's, clamped to its ends — the
    /// reference's own behaviour, and what makes the far end of a long pile
    /// reachable without pixel-hunting.
    #[test]
    fn a_press_past_the_bar_is_clamped_to_it() {
        assert_eq!(amount_at(-40, 500), 1);
        assert_eq!(amount_at(1_000, 500), 500);
        assert_eq!(amount_at(0, 1), 1, "a pile of two divides one way");
    }

    /// The two controls answer for their own boxes and the background answers
    /// for nothing, which is what leaves a press on the frame free to pick the
    /// window up.
    #[test]
    fn only_the_two_controls_are_hit() {
        assert_eq!(hit(GumpPixel::new(110, 45)), Some(Hit::Ok));
        assert_eq!(hit(GumpPixel::new(60, 20)), Some(Hit::Slider));
        assert_eq!(hit(GumpPixel::new(5, 5)), None, "the frame");
        assert_eq!(hit(GumpPixel::new(60, 60)), None, "under both");
    }

    /// What is drawn: the frame, the knob where the number says, and whichever
    /// face the button is wearing.
    #[test]
    fn the_window_draws_its_three_pictures_and_its_number() {
        let window = window(50, 100, Face::Pressed, GumpPixel::new(200, 100));
        assert_eq!(
            window
                .pictures
                .iter()
                .map(|picture| (picture.graphic, picture.at))
                .collect::<Vec<_>>(),
            vec![
                (GumpArt::Gump(BACKGROUND), GumpPixel::new(200, 100)),
                (
                    GumpArt::Gump(KNOB),
                    GumpPixel::new(200 + 29 + knob_offset(50, 100), 100 + 16)
                ),
                (GumpArt::Gump(OK_PRESSED), GumpPixel::new(302, 137)),
            ]
        );
        assert_eq!(
            window.lines,
            vec![Line {
                at: GumpPixel::new(229, 142),
                text: "50".to_owned(),
            }]
        );
    }
}
