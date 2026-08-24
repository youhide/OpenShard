//! Paperdoll frame artwork, controls, and title label layout.

use openshard_protocol::speech::Font;
use openshard_protocol::wire::{Graphic, Hue};

use crate::gump::{GumpArt, GumpPixel, Picture, PictureIndex};
use crate::text::GumpLabel;

use super::Doll;

/// Whether a paperdoll belongs to this client or another mobile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Whose {
    /// Our own doll, including its current war-mode toggle state.
    Own { war: bool },
    /// Another mobile's doll.
    Another,
}

/// The frame background selected for a paperdoll owner.
pub fn frame(whose: Whose) -> Graphic {
    match whose {
        Whose::Own { .. } => Graphic(0x07D0),
        Whose::Another => Graphic(0x07D1),
    }
}

/// A clickable control on the paperdoll frame.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum DollButton {
    Help,
    Options,
    LogOut,
    Quests,
    Skills,
    Guild,
    WarMode,
    Status,
    Profile,
    Party,
    Virtue,
    Backpack,
}

const BUTTON_X: i32 = 185;
const BUTTON_TOP: i32 = 44;
const BUTTON_STEP: i32 = 27;
const OWN_BUTTONS: [(DollButton, u16, u16); 6] = [
    (DollButton::Help, 0x07EF, 0x07F0),
    (DollButton::Options, 0x07D6, 0x07D7),
    (DollButton::LogOut, 0x07D9, 0x07DA),
    (DollButton::Quests, 0x57B5, 0x57B7),
    (DollButton::Skills, 0x07DF, 0x07E0),
    (DollButton::Guild, 0x57B2, 0x57B4),
];
const PEACE_TOGGLE: (u16, u16) = (0x07E5, 0x07E6);
const WAR_TOGGLE: (u16, u16) = (0x07E8, 0x07E9);
const STATUS_BUTTON: (u16, u16) = (0x07EB, 0x07EC);
const WAR_ROW: i32 = 6;
const STATUS_ROW: i32 = 7;
const SCROLL: Graphic = Graphic(0x07D2);
const SCROLL_AT: GumpPixel = GumpPixel::new(25, 196);
const SCROLL_STEP: i32 = 14;
const VIRTUE: Graphic = Graphic(0x0071);
const VIRTUE_AT: GumpPixel = GumpPixel::new(80, 4);

/// Title label location within the frame.
pub const NAME_AT: GumpPixel = GumpPixel::new(39, 262);
/// Title label hue.
pub const NAME_HUE: Hue = Hue(0x0386);
const NAME_WIDTH: i32 = 185;
const NAME_HEIGHT: i32 = 20;
/// Title label font.
pub const NAME_FONT: Font = Font(1);

/// Build the clipped name/title label for a paperdoll frame.
pub fn title(text: &str, at: GumpPixel) -> GumpLabel<'_> {
    GumpLabel {
        at: at.offset(NAME_AT),
        hue: NAME_HUE,
        clip: Some((NAME_WIDTH, NAME_HEIGHT)),
        text,
        font: NAME_FONT,
    }
}

/// Add the controls that sit between the background and the body art.
pub(super) fn furniture(doll: &mut Doll, whose: Whose, held: Option<DollButton>, at: GumpPixel) {
    let button = |doll: &mut Doll, which: DollButton, faces: (u16, u16), row: i32| {
        let face = if held == Some(which) { faces.1 } else { faces.0 };
        doll.pictures.push(Picture::plain(
            GumpArt::Gump(Graphic(face)),
            at.offset(GumpPixel::new(BUTTON_X, BUTTON_TOP + BUTTON_STEP * row)),
        ));
        doll.hits
            .insert(PictureIndex::new(doll.pictures.len() - 1), which);
    };

    if let Whose::Own { war } = whose {
        for (row, (which, up, down)) in OWN_BUTTONS.iter().enumerate() {
            button(doll, *which, (*up, *down), row as i32);
        }
        button(
            doll,
            DollButton::WarMode,
            if war { WAR_TOGGLE } else { PEACE_TOGGLE },
            WAR_ROW,
        );
    }
    button(doll, DollButton::Status, STATUS_BUTTON, STATUS_ROW);

    let scroll = |doll: &mut Doll, which: DollButton, x: i32| {
        doll.pictures.push(Picture::plain(
            GumpArt::Gump(SCROLL),
            at.offset(GumpPixel::new(x, SCROLL_AT.y)),
        ));
        doll.hits
            .insert(PictureIndex::new(doll.pictures.len() - 1), which);
    };
    scroll(doll, DollButton::Profile, SCROLL_AT.x);
    if matches!(whose, Whose::Own { .. }) {
        scroll(doll, DollButton::Party, SCROLL_AT.x + SCROLL_STEP);
    }
    doll.pictures
        .push(Picture::plain(GumpArt::Gump(VIRTUE), at.offset(VIRTUE_AT)));
    doll.hits
        .insert(PictureIndex::new(doll.pictures.len() - 1), DollButton::Virtue);
}
