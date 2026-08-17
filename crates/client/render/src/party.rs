//! The party manifest — the reference client's `PartyGump`, laid out from the
//! art the client ships.
//!
//! One `0x0A28` background stretched to 450×480, ten name plates down it, and
//! the handful of buttons this client has packets for. Every coordinate is the
//! reference's own, for [`crate::status`]'s reason: they are what the art was
//! drawn against, and arithmetic over the atlas would agree with them only by
//! luck.
//!
//! # What is deliberately not on it
//!
//! The reference's manifest carries four controls this one does not, and each is
//! missing because **there is no packet behind it**, not because it was hard:
//!
//! - the per-member *Tell* buttons, which address one member — this client can
//!   only say a line to the whole party (`0xBF 0x06`, see
//!   `openshard_client_net::party`);
//! - *Send the party a message*, which in the reference only types a `/` into
//!   the chat box — this client's chat has its own party channel already;
//!   see [`crate::chart`]'s neighbour `crate::text` — the button would be a
//!   second door onto the same room.
//! - the loot-type toggle, which needs a party-loot request this client's
//!   [`Outgoing`](openshard_client_net::action::Outgoing) has no arm for.
//!
//! Leaving a dead button on a plate is worse than leaving the plate plain: a
//! player who presses it learns nothing, and the next reader has to find out
//! whether it is unfinished or broken. What is here is what works.

use openshard_protocol::serial::Serial;
use openshard_protocol::speech::Font;
use openshard_protocol::wire::{Graphic, Hue};

use crate::gump::{self, GumpArt, GumpAtlas, GumpPixel, Picture, PictureIndex};
use crate::text::GumpLabel;

/// The background, as a `resizepic`: nine pieces from `0x0A28` to `0x0A30`.
const BACKGROUND: Graphic = Graphic(0x0A28);

/// How big the reference stretches it to.
pub const WIDTH: i32 = 450;
pub const HEIGHT: i32 = 480;

/// The plate one member's name is written on: 272×26.
const NAME_PLATE: Graphic = Graphic(0x0475);

/// Turn a member out — up, over and pressed, in the reference's own order of
/// arguments (`0x0FB1`, `0x0FB3`, `0x0FB2`), which is why the pressed face is
/// the *last* of the three and not the middle one.
const KICK: (Graphic, Graphic) = (Graphic(0x0FB1), Graphic(0x0FB2));

/// Ask the shard to raise a cursor for a new member.
const ADD: (Graphic, Graphic) = (Graphic(0x0FA8), Graphic(0x0FA9));

/// Leave, or disband if this client leads.
const LEAVE: (Graphic, Graphic) = (Graphic(0x0FAE), Graphic(0x0FAF));

/// The plate's own dismissal, which sends nothing: this window is a view of the
/// roster and closing it leaves the party alone.
const CLOSE: (Graphic, Graphic) = (Graphic(0x00F3), Graphic(0x00F2));

/// How many rows the plate has. Ten, as the reference draws them, whether or not
/// there is anybody to put on them — the empty plates are what make the window
/// the size the art is.
pub const ROWS: usize = 10;

/// The first row's top, and the step between rows.
const ROW_TOP: i32 = 48;
const ROW_STEP: i32 = 25;

/// Where things sit on a row: the kick button, the plate, and the name on it.
const KICK_AT: i32 = 80;
const PLATE_AT: i32 = 130;
const NAME_AT: i32 = 140;
const NAME_WIDTH: i32 = 250;

/// And the three buttons under the rows, with their captions.
const LEAVE_AT: GumpPixel = GumpPixel::new(70, 360);
const ADD_AT: GumpPixel = GumpPixel::new(70, 385);
const CLOSE_AT: GumpPixel = GumpPixel::new(130, 430);
const CAPTION_AT: i32 = 110;

/// The two headings.
const TITLE_AT: GumpPixel = GumpPixel::new(153, 20);
const KICK_HEADING_AT: GumpPixel = GumpPixel::new(80, 30);

/// The faces the reference writes this window in: `font: 1` for the small
/// heading over the kick column, `font: 2` for everything else.
const HEADING_FONT: Font = Font(1);
const FONT: Font = Font(2);
const HUE: Hue = Hue(0x0386);

/// What a press on this window means.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    /// Turn this member out — by *row*, because the row is what was pressed and
    /// what is on it is the caller's to look up. A serial baked in here would be
    /// a copy of the roster taken when the window was drawn, and the roster
    /// arrives whole on every change.
    Kick(usize),
    /// Ask the shard for a cursor to add somebody.
    Add,
    /// Leave, or disband.
    Leave,
    /// Put the window away, and nothing else.
    Close,
}

/// One line this window writes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Line {
    pub at: GumpPixel,
    pub text: String,
    pub font: Font,
    /// The box it is cropped to, or `None` for a heading that stands on its own.
    pub clip: Option<(i32, i32)>,
}

impl Line {
    #[must_use]
    pub fn label(&self) -> GumpLabel<'_> {
        GumpLabel {
            at: self.at,
            text: &self.text,
            font: self.font,
            hue: HUE,
            clip: self.clip,
        }
    }
}

/// The manifest laid out for one frame.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Window {
    /// The background's nine pieces, the plates and the buttons, in painter's
    /// order.
    pub pictures: Vec<Picture>,
    /// The headings and the names.
    pub lines: Vec<Line>,
    /// Which pictures answer the mouse — [`crate::confirm::Window::hits`]'s
    /// shape, and the same reason: what is drawn and what is clicked are one
    /// list.
    hits: Vec<(PictureIndex, Hit)>,
}

impl Window {
    /// Which control owns `cursor`, if any.
    ///
    /// [`crate::gump::pick_hit`]'s test: against each button's whole rectangle,
    /// because the reference's `Button` owns its bounds and testing the ink
    /// turns a bevelled margin into a dead zone.
    #[must_use]
    pub fn hit(&self, cursor: GumpPixel, atlas: &GumpAtlas) -> Option<Hit> {
        crate::gump::pick_hit(
            &self.pictures,
            atlas,
            cursor,
            self.hits.iter().map(|(index, hit)| (*index, *hit)),
        )
    }
}

/// Every graphic this window can draw, whichever button is down.
///
/// The background's nine pieces are named through
/// [`gump::resize`](crate::gump::resize)'s own remap rather than as a range: the
/// middle piece is `+4` and the client's art is not in reading order, which is
/// the one thing about a `resizepic` that cannot be guessed.
pub fn art_of() -> impl Iterator<Item = GumpArt> {
    let background = (0..9).map(|piece| GumpArt::Gump(Graphic(BACKGROUND.0.wrapping_add(piece))));
    let furniture = [
        NAME_PLATE, KICK.0, KICK.1, ADD.0, ADD.1, LEAVE.0, LEAVE.1, CLOSE.0, CLOSE.1,
    ]
    .into_iter()
    .map(GumpArt::Gump);
    background.chain(furniture)
}

/// Lay the manifest out at `at`.
///
/// `members` is the roster, leader first — the view's own order, which is the
/// only thing that says who leads. `leading` is whether *this* client is that
/// leader, which decides whether the two controls only a leader has are drawn at
/// all: a member who pressed Kick would be sending a packet the shard refuses,
/// and a button that cannot work is worse than no button.
///
/// `atlas` is read for one thing: how big the background's nine pieces turned
/// out to be, which is what decides where its edges go. See
/// [`gump::resize`](crate::gump::resize).
#[must_use]
pub fn window(
    members: &[Serial],
    leading: bool,
    held: Option<Hit>,
    at: GumpPixel,
    atlas: &GumpAtlas,
) -> Window {
    let face = |hit: Hit, (up, down): (Graphic, Graphic)| {
        if held == Some(hit) { down } else { up }
    };
    let mut pictures = gump::resize(atlas, BACKGROUND, at, WIDTH, HEIGHT);
    let mut hits: Vec<(PictureIndex, Hit)> = Vec::new();
    let mut lines = vec![
        Line {
            at: at.offset(TITLE_AT),
            text: "Party Manifest".to_owned(),
            font: FONT,
            clip: None,
        },
        Line {
            at: at.offset(KICK_HEADING_AT),
            text: "Kick".to_owned(),
            font: HEADING_FONT,
            clip: None,
        },
    ];
    for row in 0..ROWS {
        let y = ROW_TOP + row as i32 * ROW_STEP;
        // The plate first, so a kick button pushed after it is drawn over it —
        // painter's order is z-order in this pass.
        pictures.push(Picture::plain(
            GumpArt::Gump(NAME_PLATE),
            at.offset(GumpPixel::new(PLATE_AT, y)),
        ));
        let Some(member) = members.get(row) else {
            // An empty row keeps its plate and gets nothing else: there is
            // nobody on it to turn out, and the reference's own button there
            // answers for nobody.
            continue;
        };
        if leading {
            pictures.push(Picture::plain(
                GumpArt::Gump(face(Hit::Kick(row), KICK)),
                at.offset(GumpPixel::new(KICK_AT, y + 2)),
            ));
            hits.push((PictureIndex::new(pictures.len() - 1), Hit::Kick(row)));
        }
        lines.push(Line {
            at: at.offset(GumpPixel::new(NAME_AT, y + 1)),
            // By serial, because that is all the roster carries: a `0xBF 0x06`
            // sub-command names its members by serial and never by name, and
            // this client may have neither clicked nor hovered any of them. The
            // leader is the first row and the wire never says so outright — see
            // `view::Party`.
            text: match row {
                0 => format!("{:#010X} (leader)", member.raw()),
                _ => format!("{:#010X}", member.raw()),
            },
            font: FONT,
            clip: Some((NAME_WIDTH, ROW_STEP)),
        });
    }
    let mut control = |hit: Hit, faces: (Graphic, Graphic), corner: GumpPixel, caption: &str| {
        pictures.push(Picture::plain(GumpArt::Gump(face(hit, faces)), at.offset(corner)));
        hits.push((PictureIndex::new(pictures.len() - 1), hit));
        if !caption.is_empty() {
            lines.push(Line {
                at: at.offset(GumpPixel::new(CAPTION_AT, corner.y)),
                text: caption.to_owned(),
                font: FONT,
                clip: None,
            });
        }
    };
    // Leaving is `0x02` naming yourself, and a leader who does it disbands the
    // party — one packet, two words for it, and the caption is the only place
    // this client can say which one the player is about to get.
    control(
        Hit::Leave,
        LEAVE,
        LEAVE_AT,
        if leading {
            "Disband the party"
        } else {
            "Leave the party"
        },
    );
    if leading {
        control(Hit::Add, ADD, ADD_AT, "Add new member");
    }
    // No caption: the reference's own close button carries its word in the art.
    control(Hit::Close, CLOSE, CLOSE_AT, "");
    Window {
        pictures,
        lines,
        hits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use openshard_uofiles::color::Color16;
    use openshard_uofiles::image::Image;

    fn member(serial: u32) -> Serial {
        Serial::new(serial).unwrap()
    }

    /// A solid picture of a given size: every texel opaque, so that a hit test
    /// against one answers for its whole box.
    fn block(width: u16, height: u16) -> Image {
        Image::new(
            width,
            height,
            vec![Color16(0x7FFF); usize::from(width) * usize::from(height)],
        )
    }

    /// An atlas holding every picture this window draws, each a solid block of
    /// the size the real art is — which is what the hit test measures against.
    fn shipping() -> GumpAtlas {
        let sizes = |art: GumpArt| match art {
            GumpArt::Gump(Graphic(0x0A28 | 0x0A2A | 0x0A2E | 0x0A30)) => (44, 44),
            GumpArt::Gump(Graphic(0x0A29 | 0x0A2F)) => (427, 44),
            GumpArt::Gump(Graphic(0x0A2B | 0x0A2D)) => (44, 316),
            GumpArt::Gump(Graphic(0x0A2C)) => (427, 316),
            GumpArt::Gump(Graphic(0x0475)) => (272, 26),
            GumpArt::Gump(Graphic(0x00F2 | 0x00F3)) => (63, 23),
            _ => (30, 22),
        };
        GumpAtlas::pack(art_of().map(|art| {
            let (width, height) = sizes(art);
            (art, block(width, height))
        }))
        .expect("a handful of blocks fit one atlas")
    }

    /// A leader sees the two controls only a leader has, and a member does not
    /// — the whole of what `leading` decides.
    #[test]
    fn only_a_leader_is_offered_kick_and_add() {
        let roster = [member(1), member(2)];
        let atlas = shipping();
        let leader = window(&roster, true, None, GumpPixel::new(0, 0), &atlas);
        let hits: Vec<Hit> = leader.hits.iter().map(|(_, hit)| *hit).collect();
        assert_eq!(
            hits,
            vec![Hit::Kick(0), Hit::Kick(1), Hit::Leave, Hit::Add, Hit::Close]
        );

        let member_view = window(&roster, false, None, GumpPixel::new(0, 0), &atlas);
        let hits: Vec<Hit> = member_view.hits.iter().map(|(_, hit)| *hit).collect();
        assert_eq!(hits, vec![Hit::Leave, Hit::Close]);
    }

    /// Ten plates whatever the roster is, and a name on each occupied row.
    #[test]
    fn the_plate_has_ten_rows_however_many_are_filled() {
        let atlas = shipping();
        let window = window(&[member(1)], false, None, GumpPixel::new(0, 0), &atlas);
        let plates = window
            .pictures
            .iter()
            .filter(|picture| picture.graphic == GumpArt::Gump(NAME_PLATE))
            .count();
        assert_eq!(plates, ROWS);
        let names = window
            .lines
            .iter()
            .filter(|line| line.text.starts_with("0x"))
            .count();
        assert_eq!(names, 1, "one member, one name");
    }

    /// Each control answers for its own box, and the rows do not answer at all
    /// — a name plate is not a button, which is what leaves a press on one free
    /// to pick the window up.
    #[test]
    fn the_controls_answer_for_their_own_boxes() {
        let atlas = shipping();
        let window = window(
            &[member(1), member(2)],
            true,
            None,
            GumpPixel::new(20, 10),
            &atlas,
        );
        // Inside the second row's kick button: (80, 48 + 25 + 2) plus the
        // window's own corner.
        assert_eq!(
            window.hit(GumpPixel::new(20 + 90, 10 + 80), &atlas),
            Some(Hit::Kick(1))
        );
        assert_eq!(
            window.hit(GumpPixel::new(20 + 80, 10 + 370), &atlas),
            Some(Hit::Leave)
        );
        assert_eq!(
            window.hit(GumpPixel::new(20 + 80, 10 + 395), &atlas),
            Some(Hit::Add)
        );
        assert_eq!(
            window.hit(GumpPixel::new(20 + 140, 10 + 440), &atlas),
            Some(Hit::Close)
        );
        assert_eq!(
            window.hit(GumpPixel::new(20 + 200, 10 + 50), &atlas),
            None,
            "a name plate is not a control"
        );
    }

    /// A held control is drawn in its pressed face, and only that one is.
    #[test]
    fn holding_a_control_swaps_its_face_alone() {
        let atlas = shipping();
        let window = window(&[member(1)], true, Some(Hit::Add), GumpPixel::new(0, 0), &atlas);
        let drawn: Vec<Graphic> = window
            .pictures
            .iter()
            .filter_map(|picture| match picture.graphic {
                GumpArt::Gump(graphic) if graphic == ADD.1 || graphic == LEAVE.0 => Some(graphic),
                _ => None,
            })
            .collect();
        assert_eq!(drawn, vec![LEAVE.0, ADD.1]);
    }
}
