//! The player's classic status window.
//!
//! `0x11` says the numbers; this module says only how those numbers occupy the
//! client's own `0x0802` frame. It does not know a socket, a `WorldView`, or
//! which player supplied them — the caller joins those facts at the app/render
//! boundary, as it does for [`crate::skills`].

use openshard_client_model::Status;
use openshard_protocol::mobile::Vitals;
use openshard_protocol::speech::Font;
use openshard_protocol::wire::{
    Graphic,
    Hue,
};

use crate::gump::{
    GumpArt,
    GumpPixel,
    Picture,
};
use crate::text::GumpLabel;

/// One line the status window writes, already placed in gump pixels.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Line {
    pub at:   GumpPixel,
    pub text: String,
}

impl Line {
    /// Draw the status line in the reference frame's face and hue.
    #[must_use]
    pub fn label(&self) -> GumpLabel<'_> {
        GumpLabel {
            at:   self.at,
            text: &self.text,
            font: FONT,
            hue:  HUE,
            clip: None,
        }
    }
}

/// A status window laid out for one frame.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Window {
    /// The background art, behind every label.
    pub pictures: Vec<Picture>,
    /// The name and ten values written over that art.
    pub lines:    Vec<Line>,
}

/// The old status frame. It is 282×151 in the post-ML client files this app
/// reads; the layout below deliberately uses the reference's coordinates
/// instead of relying on that size for arithmetic.
const FRAME: Graphic = Graphic(0x0802);
const FONT: Font = Font(1);
const HUE: Hue = Hue(0x0386);

/// Lay out the old-style status window at `at`.
///
/// These are `StatusGumpOld`'s positions: the name at `(86, 42)`, the five
/// left-column values at `y = 62 + 12n`, and the five right-column values at
/// the same rows. The art itself carries the labels and no caller supplies a
/// second localized copy of them.
#[must_use]
pub fn window(status: &Status, hits: Vitals, at: GumpPixel) -> Window {
    let left = 86;
    let right = 171;
    let row = |x, y, text: String| {
        Line {
            at: at.offset(GumpPixel::new(x, y)),
            text,
        }
    };
    Window {
        pictures: vec![Picture::plain(GumpArt::Gump(FRAME), at)],
        lines:    vec![
            row(left, 42, status.name.clone()),
            row(left, 62, status.strength.to_string()),
            row(left, 74, status.dexterity.to_string()),
            row(left, 86, status.intelligence.to_string()),
            row(left, 98, if status.female { "Female" } else { "Male" }.to_owned()),
            row(left, 110, status.armor.to_string()),
            row(right, 62, vitals(hits)),
            row(right, 74, vitals(status.mana)),
            row(right, 86, vitals(status.stamina)),
            row(right, 98, status.gold.to_string()),
            row(right, 110, format!("{}/{}", status.weight, status.max_weight)),
        ],
    }
}

fn vitals(vitals: Vitals) -> String {
    format!("{}/{}", vitals.current, vitals.max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> Status {
        Status {
            name:          "Lord British".to_owned(),
            female:        false,
            strength:      100,
            dexterity:     50,
            intelligence:  75,
            stamina:       Vitals {
                current: 49,
                max:     50,
            },
            mana:          Vitals {
                current: 72,
                max:     75,
            },
            gold:          1_234,
            armor:         42,
            weight:        12,
            max_weight:    450,
            stat_cap:      225,
            followers:     0,
            followers_max: 5,
        }
    }

    #[test]
    fn the_status_frame_carries_its_own_numbers_at_the_reference_positions() {
        let window = window(
            &status(),
            Vitals {
                current: 98,
                max:     100,
            },
            GumpPixel::new(300, 200),
        );
        assert_eq!(
            window.pictures,
            vec![Picture::plain(GumpArt::Gump(FRAME), GumpPixel::new(300, 200))]
        );
        assert_eq!(
            window
                .lines
                .iter()
                .map(|line| (line.at, line.text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (GumpPixel::new(386, 242), "Lord British"),
                (GumpPixel::new(386, 262), "100"),
                (GumpPixel::new(386, 274), "50"),
                (GumpPixel::new(386, 286), "75"),
                (GumpPixel::new(386, 298), "Male"),
                (GumpPixel::new(386, 310), "42"),
                (GumpPixel::new(471, 262), "98/100"),
                (GumpPixel::new(471, 274), "72/75"),
                (GumpPixel::new(471, 286), "49/50"),
                (GumpPixel::new(471, 298), "1234"),
                (GumpPixel::new(471, 310), "12/450"),
            ]
        );
    }
}
