//! A paperdoll against the files it is drawn out of.
//!
//! What the unit tests beside `paperdoll.rs` cannot say: they pin the *order*,
//! which is a table and needs nothing, and stop at the point where a picture has
//! to exist. Everything here is about the third index space — that an `AnimID`
//! plus 50000 is a picture the client actually ships, that the female half of
//! the file is sparse and the fallback is what makes it work, and that the list
//! a window is laid out as begins with the body and ends with the backpack.
//!
//! Ignored and gated on `OPENSHARD_CLIENT`: no client files live in this
//! repository, ever.

use openshard_client_render::gump::{
    GumpArt,
    GumpPixel,
    PictureIndex,
};
use openshard_client_render::mobiles::EquipmentLayer;
use openshard_client_render::paperdoll::{
    self,
    FEMALE_GUMP_OFFSET,
    MALE_GUMP_OFFSET,
    Wearer,
    Whose,
};
use openshard_protocol::wire::{
    Graphic,
    Hue,
    Layer,
};
use openshard_tiles::{
    AnimId,
    TileData,
};
use openshard_uofiles::equipconv::EquipConv;
use openshard_uofiles::gumpart::Gumps;

/// The three files a paperdoll needs, or `None` where no client is installed.
fn client() -> Option<(Gumps, EquipConv, TileData)> {
    let dir = std::env::var_os("OPENSHARD_CLIENT").map(std::path::PathBuf::from)?;
    let gumps = Gumps::open(&dir).expect("gumpartLegacyMUL.uop");
    // The table is optional in a client install and its absence is not a
    // failure — an empty one resolves nothing, which is the ordinary case for
    // most rows anyway.
    let equip_conv = EquipConv::load(dir.join("Equipconv.def")).unwrap_or_default();
    let tiledata = openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");
    Some((gumps, equip_conv, tiledata))
}

/// A male human, and a female one — the two bodies every claim below is made
/// about.
const MALE: u16 = 0x0190;
const FEMALE: u16 = 0x0191;

fn worn(layer: Layer, graphic: AnimId) -> EquipmentLayer {
    EquipmentLayer {
        graphic,
        hue: Hue::NONE,
        layer,
    }
}

/// The `AnimID` of a shipped item, which is what a worn layer carries: the wire
/// graphic is `tiledata`'s key and never reaches a paperdoll.
fn anim_id(tiledata: &TileData, graphic: u16) -> AnimId {
    tiledata.static_tile(graphic).anim_id
}

/// The doll itself: everything in a laid-out window that is neither the frame
/// nor a piece of the frame's own furniture.
///
/// Told apart by [`paperdoll::Doll::hits`] rather than by counting, which is
/// what keeps these tests about the doll when a button is added or moved: a
/// button and a scroll answer the mouse and a garment does not.
fn stack(doll: &paperdoll::Doll) -> Vec<openshard_client_render::gump::Picture> {
    doll.pictures
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 0 && !doll.hits.contains_key(&PictureIndex::new(*index)))
        .map(|(_, picture)| *picture)
        .collect()
}

/// The window is the frame first, then the body, then what is worn, then the
/// backpack — and every one of them is a gump the client ships.
///
/// Two claims worth the test. The frame is the *window*, so it is at the
/// window's own origin and everything inside it is one `BODY_AT` further in —
/// one offset for the whole stack, not one per garment. And the backpack is
/// drawn *outside* the ordering, last, because nothing is ever drawn over it.
#[test]
#[ignore]
fn a_dressed_body_draws_its_gump_first_and_its_backpack_last() {
    let Some((gumps, equip_conv, tiledata)) = client() else {
        return;
    };
    // A plain shirt, a pair of pants and the backpack every character wears.
    let equipment = [
        worn(Layer::SHIRT, anim_id(&tiledata, 0x1517)),
        worn(Layer::PANTS, anim_id(&tiledata, 0x152E)),
        worn(Layer::BACKPACK, anim_id(&tiledata, 0x0E75)),
    ];
    let wearer = Wearer {
        body:      Graphic(MALE),
        hue:       Hue::NONE,
        equipment: &equipment,
    };
    let at = GumpPixel::new(100, 50);
    let whose = Whose::Own { war: false };
    let doll = paperdoll::window(Some(&wearer), whose, None, None, None, &equip_conv, &gumps, at);
    let stack = stack(&doll);

    assert_eq!(
        stack.len(),
        3,
        "a body and two garments — the backpack is a hit now, not a plain layer"
    );
    assert_eq!(
        doll.pictures[0].graphic,
        GumpArt::Gump(paperdoll::frame(whose)),
        "the frame is the first picture, and it is our own doll's"
    );
    assert_eq!(doll.pictures[0].at, at, "the frame is the window");
    assert_eq!(
        stack[0].graphic,
        GumpArt::Gump(Graphic(0x000C)),
        "the male body is drawn on it"
    );
    for picture in &stack {
        assert_eq!(
            picture.at,
            GumpPixel::new(at.x + paperdoll::BODY_AT.x, at.y + paperdoll::BODY_AT.y),
            "every layer sits at the one origin inside the frame"
        );
    }
    for picture in &doll.pictures {
        let GumpArt::Gump(graphic) = picture.graphic else {
            panic!("a paperdoll draws gump art and nothing else");
        };
        assert!(
            gumps.has(graphic).expect("the container reads"),
            "the client ships gump 0x{:04X}",
            graphic.0
        );
    }
    let backpack = paperdoll::gump_of(
        Graphic(MALE),
        anim_id(&tiledata, 0x0E75),
        false,
        &equip_conv,
        &gumps,
    );
    let last = PictureIndex::new(doll.pictures.len() - 1);
    assert_eq!(
        doll.pictures[last.position()].graphic,
        GumpArt::Gump(backpack),
        "the backpack is drawn last, outside the order"
    );
    assert_eq!(
        doll.hits.get(&last),
        Some(&paperdoll::DollButton::Backpack),
        "and it answers a double click, the one worn item that does"
    );
}

/// Every picture the frame's own furniture is drawn from is one the client
/// ships, and the two frames carry different sets of it.
///
/// The claim the coordinates cannot make: a button at the right `y` drawn from
/// a graphic `gumpart` has never heard of is a gap in the column, and
/// [`openshard_client_render::gump::collect`] skips a missing picture without
/// saying so. Both frames are asked, because the stranger's carries three of
/// the eleven and a mistake there would hide behind our own doll's full column.
#[test]
#[ignore]
fn every_button_on_a_frame_is_a_picture_the_client_ships() {
    let Some((gumps, equip_conv, _)) = client() else {
        return;
    };
    for whose in [
        Whose::Own { war: false },
        Whose::Own { war: true },
        Whose::Another,
    ] {
        let doll = paperdoll::window(
            None,
            whose,
            None,
            None,
            None,
            &equip_conv,
            &gumps,
            GumpPixel::new(0, 0),
        );
        assert!(!doll.hits.is_empty(), "a frame has furniture on it: {whose:?}");
        for index in doll.hits.keys() {
            let GumpArt::Gump(graphic) = doll.pictures[index.position()].graphic else {
                panic!("a paperdoll draws gump art and nothing else");
            };
            assert!(
                gumps.has(graphic).expect("the container reads"),
                "the client ships gump 0x{:04X} ({:?} on {whose:?})",
                graphic.0,
                doll.hits[index],
            );
        }
        // And every one of them is pressable: a button drawn in a face the file
        // has no pressed twin for would light up as a hole under the finger.
        let pressed = paperdoll::window(
            None,
            whose,
            doll.hits.values().next().copied(),
            None,
            None,
            &equip_conv,
            &gumps,
            GumpPixel::new(0, 0),
        );
        for index in pressed.hits.keys() {
            let GumpArt::Gump(graphic) = pressed.pictures[index.position()].graphic else {
                panic!("a paperdoll draws gump art and nothing else");
            };
            assert!(
                gumps.has(graphic).expect("the container reads"),
                "the client ships the pressed gump 0x{:04X}",
                graphic.0
            );
        }
    }
}

/// Our own doll's column is the long one, and a stranger's is not: the frames
/// differ by exactly the buttons there is no room for.
#[test]
#[ignore]
fn a_stranger_gets_the_status_button_and_none_of_the_rest() {
    let Some((gumps, equip_conv, _)) = client() else {
        return;
    };
    let buttons = |whose| {
        let doll = paperdoll::window(
            None,
            whose,
            None,
            None,
            None,
            &equip_conv,
            &gumps,
            GumpPixel::new(0, 0),
        );
        doll.hits.values().copied().collect::<Vec<_>>()
    };
    let stranger = buttons(Whose::Another);
    assert!(stranger.contains(&paperdoll::DollButton::Status));
    assert!(!stranger.contains(&paperdoll::DollButton::LogOut));
    assert!(
        !stranger.contains(&paperdoll::DollButton::Party),
        "the party manifest is our own doll's"
    );

    let own = buttons(Whose::Own { war: false });
    for wanted in [
        paperdoll::DollButton::Help,
        paperdoll::DollButton::Options,
        paperdoll::DollButton::LogOut,
        paperdoll::DollButton::Quests,
        paperdoll::DollButton::Skills,
        paperdoll::DollButton::Guild,
        paperdoll::DollButton::WarMode,
        paperdoll::DollButton::Status,
        paperdoll::DollButton::Profile,
        paperdoll::DollButton::Party,
        paperdoll::DollButton::Virtue,
    ] {
        assert!(own.contains(&wanted), "our own doll carries {wanted:?}");
    }
}

/// A layer with no `AnimID` draws nothing — a ring, an earring, anything the
/// paperdoll has no picture of. `docs/client/design_windows.md`'s "done when", in one
/// assertion.
#[test]
#[ignore]
fn a_layer_with_no_anim_id_draws_nothing() {
    let Some((gumps, equip_conv, _)) = client() else {
        return;
    };
    let equipment = [worn(Layer::RING, AnimId(0))];
    let wearer = Wearer {
        body:      Graphic(MALE),
        hue:       Hue::NONE,
        equipment: &equipment,
    };
    let doll = paperdoll::window(
        Some(&wearer),
        Whose::Another,
        None,
        None,
        None,
        &equip_conv,
        &gumps,
        GumpPixel::new(0, 0),
    );
    assert_eq!(stack(&doll).len(), 1, "the body, and nothing for the ring");
}

/// A paperdoll of a mobile this client has never been told the body of is a
/// frame and nothing else — not an empty window.
///
/// The claim is about what a window *is*: the frame is what the pointer finds
/// and what the right button closes, so a doll waiting on its `0x77` has to
/// keep it. Drawing nothing at all would leave a window in the list that
/// nothing on screen corresponds to.
#[test]
#[ignore]
fn a_paperdoll_of_an_unknown_body_is_still_a_frame() {
    let Some((gumps, equip_conv, _)) = client() else {
        return;
    };
    let doll = paperdoll::window(
        None,
        Whose::Another,
        None,
        None,
        None,
        &equip_conv,
        &gumps,
        GumpPixel::new(0, 0),
    );
    assert!(stack(&doll).is_empty(), "no body, and so no layers");
    assert_eq!(
        doll.pictures[0].graphic,
        GumpArt::Gump(paperdoll::frame(Whose::Another))
    );
}

/// The two frames are two different pictures, and both are shipped: the one
/// with room for the buttons a player gets over their own doll, and the plain
/// one a stranger's is drawn in.
#[test]
#[ignore]
fn both_frames_are_pictures_the_client_ships_and_they_differ() {
    let Some((gumps, _, _)) = client() else {
        return;
    };
    let own = paperdoll::frame(Whose::Own { war: false });
    let another = paperdoll::frame(Whose::Another);
    assert_ne!(own, another);
    assert!(gumps.has(own).expect("the container reads"));
    assert!(gumps.has(another).expect("the container reads"));
}

/// The two offsets, and the fallback between them.
///
/// The claim: a garment's female picture is `AnimID + 60000` **where the client
/// ships one**, and the male picture where it does not — which is not an edge
/// case but the ordinary state of the file. The test walks the shipped range
/// and asserts the rule held for every garment in it, and counts both outcomes
/// so that a run where the fallback never fired cannot pass silently pretending
/// it was exercised.
#[test]
#[ignore]
fn a_female_gump_falls_back_to_the_male_one_where_the_file_has_none() {
    let Some((gumps, equip_conv, tiledata)) = client() else {
        return;
    };
    let (mut own, mut fell_back, mut checked) = (0usize, 0usize, 0usize);
    for graphic in 0..u16::MAX {
        let anim = anim_id(&tiledata, graphic);
        if anim == AnimId(0) {
            continue;
        }
        // A pair the table has an opinion about is a different claim — the
        // override happens *before* the offset, so the picture is no longer
        // "this anim plus 60000" and this test would be asserting the table's
        // contents rather than the fallback. `Equipconv.def` really does carry
        // such rows for a female body (anim 0x02A8 among them, which is how
        // this line came to exist).
        if equip_conv.resolve(Graphic(FEMALE), anim).is_some() {
            continue;
        }
        let male = Graphic(anim.0.wrapping_add(MALE_GUMP_OFFSET));
        if !gumps.has(male).expect("the container reads") {
            continue;
        }
        checked += 1;
        let female = Graphic(anim.0.wrapping_add(FEMALE_GUMP_OFFSET));
        let drawn = paperdoll::gump_of(Graphic(FEMALE), anim, true, &equip_conv, &gumps);
        match gumps.has(female).expect("the container reads") {
            true => {
                own += 1;
                assert_eq!(
                    drawn, female,
                    "anim 0x{:04X} has a female picture of its own",
                    anim.0
                );
            }
            false => {
                fell_back += 1;
                assert_eq!(
                    drawn, male,
                    "anim 0x{:04X} has none, and takes the male one",
                    anim.0
                );
            }
        }
    }
    assert!(checked > 100, "the client ships equipment gumps: {checked}");
    assert!(own > 0, "some garments have a female picture: {own}");
    assert!(
        fell_back > 0,
        "and some do not — a run that never fell back has not tested the fallback: {fell_back}"
    );
}
