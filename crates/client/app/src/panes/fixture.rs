//! An install-shaped hole: everything a pane needs to be *asked a question*,
//! built out of nothing at all.
//!
//! Step 8 of `docs/window_components.md` is one assertion — a catalogue at its
//! last row takes the notch and asks for no frame — made *through*
//! [`Pane::handle`](crate::panes::Pane::handle) rather than through the pane's
//! own private method. What blocked it for seven steps was a single field: a
//! [`PaneCtx`](crate::panes::PaneCtx) carried the whole of
//! [`Resources`](crate::resources::Resources), which holds the facet, the
//! navigation graph and the 195MB animation file and cannot be built without a
//! client install on disk.
//!
//! [`PaneFiles`] is what unblocked it, and this is the other half: the nine
//! borrows it wants, owned by one value a test can put on the stack. Nothing
//! here invents what an install says — the two atlases are packed from solid
//! blocks the caller names, the tables are empty rather than plausible, and a
//! test that needs a real name or a real sprite reads a real install the way
//! `tests/client_files.rs` does.

use openshard_client_net::view::WorldView;
use openshard_client_render::atlas::FontAtlas;
use openshard_client_render::gump::{GumpArt, GumpAtlas, GumpPixel};
use openshard_protocol::direction::{Direction, Facing};
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::{MapSize, PlayerStart, Point};
use openshard_tiles::TileData;
use openshard_uofiles::art::Art;
use openshard_uofiles::color::Color16;
use openshard_uofiles::equipconv::EquipConv;
use openshard_uofiles::image::Image;
use openshard_uofiles::skillgrp::SkillGroups;
use openshard_uofiles::skills::Skills;

use crate::panes::{Modifiers, PaneCtx, PaneFiles, PaneFrame};
use crate::windows::Drawn;

/// A solid rectangle of opaque pixels, the shape of one gump picture.
///
/// Every texel is opaque, which is what makes it usable as a hit-test target:
/// [`GumpAtlas::opaque_at`](openshard_client_render::gump::GumpAtlas) answers
/// for the *pixels*, so art that was transparent everywhere would be a picture
/// nothing can be picked against.
pub(crate) fn block(width: u16, height: u16) -> Image {
    Image::new(
        width,
        height,
        vec![Color16(0x7FFF); usize::from(width) * usize::from(height)],
    )
}

/// The client's files, as a window is allowed to see them, owned.
///
/// Held by value because [`PaneFiles`] is nine borrows and has to borrow from
/// somewhere — a test keeps one of these alive for as long as the
/// [`PaneCtx`] it lends to.
pub(crate) struct Install {
    gump_atlas: GumpAtlas,
    font_atlas: FontAtlas,
    art: Art,
    tiledata: TileData,
    equip_conv: EquipConv,
    skill_names: Skills,
    skill_groups: SkillGroups,
}

impl Install {
    /// An install that ships exactly the gump pictures named, each a solid
    /// block of the size given, and nothing else.
    ///
    /// The graphics are named by the caller rather than defaulted, because
    /// which art a window needs is the window's own business — a shop asks for
    /// its two parchment panels by number
    /// ([`vendor::art_of`](openshard_client_render::vendor::art_of)) and a test
    /// that packed something else would be laying the window out against art it
    /// does not draw.
    pub(crate) fn shipping(pictures: impl IntoIterator<Item = (GumpArt, (u16, u16))>) -> Self {
        let packed = pictures
            .into_iter()
            .map(|(art, (width, height))| (art, block(width, height)));
        Self {
            gump_atlas: GumpAtlas::pack(packed).expect("a handful of blocks fit in one atlas"),
            // No glyphs: a test that measures text is a test about text, and it
            // reads a real `fonts.mul`. Every width here is zero, honestly so —
            // this file ships nothing rather than guessing at a face.
            font_atlas: FontAtlas::pack([]).expect("an atlas of no glyphs"),
            art: Art::empty(),
            tiledata: TileData::empty(),
            equip_conv: EquipConv::default(),
            skill_names: Skills::default(),
            skill_groups: SkillGroups::default(),
        }
    }

    /// The nine borrows a pane reads.
    pub(crate) fn files(&self) -> PaneFiles<'_> {
        PaneFiles {
            gump_atlas: &self.gump_atlas,
            font_atlas: &self.font_atlas,
            art: &self.art,
            tiledata: &self.tiledata,
            // The two an install may legitimately not ship, and this one does
            // not. Both readers already treat `None` as "nothing to draw"
            // rather than as an error.
            gumps: None,
            cliloc: None,
            equip_conv: &self.equip_conv,
            skill_names: &self.skill_names,
            skill_groups: &self.skill_groups,
        }
    }

    /// The context one input is answered against.
    ///
    /// `under_pointer` is `true`, because that is the interesting case: a pane
    /// that declines everything because the manager said the pointer is
    /// elsewhere would pass an assertion about `taken` by never being asked.
    /// A test about the gate itself passes `false` and expects
    /// [`Response::ignored`](crate::panes::Response).
    ///
    /// `past_slop` is `false`, which is what a press that has just landed is
    /// answered against. A test that is about the *move* out of a press sets
    /// the field on the context it got back — the manager's answer is a
    /// `bool` a test states, exactly as `now` is an instant a test names.
    pub(crate) fn ctx<'a>(
        &'a self,
        view: &'a WorldView,
        drawn: Option<&'a Drawn>,
        cursor: GumpPixel,
        under_pointer: bool,
    ) -> PaneCtx<'a> {
        PaneCtx {
            frame: PaneFrame {
                view,
                files: self.files(),
                cursor,
                hand: None,
                has_keyboard: false,
                has_prompt: false,
            },
            drawn,
            under_pointer,
            modifiers: Modifiers::default(),
            // The clock a pane is *given*, which is decision 3's reason for the
            // field: a pair of clicks is a timing rule and a rule read off an
            // ambient clock cannot be exercised. A test that is about a pair
            // builds two of these and names both instants; this is the one a
            // test that has no pair in it takes.
            now: std::time::Instant::now(),
            past_slop: false,
        }
    }
}

/// A world with this client's character in it and nothing else — the `0x1B`
/// and no packet after it.
pub(crate) fn world(player: Serial) -> WorldView {
    WorldView::entered(PlayerStart {
        serial: player,
        body: Graphic(400),
        position: Point::new(1000, 1000, 0),
        facing: Facing::walking(Direction::South),
        map: MapSize::BRITANNIA,
    })
}
