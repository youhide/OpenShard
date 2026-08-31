//! Tests spanning more than one subsystem file — the bearing arithmetic in
//! `ui_command.rs`, the frame dump in `presentation.rs`, and the own-window
//! reconciliation in `own_windows.rs`. Not split further: most of these
//! cases exercise the same fixtures (`bare_view`, `doll`) across more than
//! one of those files, and a test module per subsystem would either
//! duplicate the fixtures or reach across files for them — worse than one
//! file that says up front it is about more than one thing.

use openshard_client_render::{
    camera,
    mobiles,
};
use openshard_movement::{
    Heading,
    Lean,
};
use openshard_protocol::direction::Facing;

use super::*;
use crate::presentation::write_frame_dump;
use crate::ui_command::{
    ask_between,
    on_screen,
};
use crate::windows::{
    WindowSubject,
    reconcile_own_windows,
};

/// Which way the cursor points, for the tests that are about the bearing
/// and not about the ring it fell in.
fn heading_to(body: camera::WorldPixel, cursor: camera::WorldPixel) -> Option<Heading> {
    ask_between(body, cursor).map(steer::Ask::heading)
}

/// A dump is a directory holding one picture per plane, named for the plane,
/// and the inputs the frame was assembled from.
///
/// The naming is the contract: `docs/parity.md`'s gate compares a client's
/// plane against a tool's, and it finds them by name. A dump that wrote
/// `0.png` … `12.png` would be a dump whose reader has to keep
/// [`View::ALL`]'s order by hand — the shape the plan complains about, since
/// that order has already shifted twice.
#[test]
fn a_dump_is_a_picture_per_plane_named_for_it_and_the_inputs_beside_them() {
    let into = std::env::temp_dir().join("openshard-frame-dump-test");
    // Whatever a previous run left: this asserts on the directory's whole
    // contents, so a stale file is a false failure — or worse, a false pass
    // for a picture this run never wrote.
    let _ = std::fs::remove_dir_all(&into);

    let planes: Vec<(View, Vec<u8>)> = View::ALL.iter().map(|&view| (view, vec![1, 2, 3])).collect();
    write_frame_dump(&into, &planes, "view = lit\n").expect("a directory under the temp dir");

    for view in View::ALL {
        let picture = into.join(format!("{}.png", view.name()));
        assert!(
            picture.is_file(),
            "no {} plane at {}",
            view.name(),
            picture.display()
        );
    }
    assert_eq!(
        std::fs::read_to_string(into.join("inputs.txt")).expect("the summary"),
        "view = lit\n",
        "the inputs are written beside the pictures, verbatim",
    );
    let written = std::fs::read_dir(&into).expect("the dump directory").count();
    assert_eq!(
        written,
        View::ALL.len() + 1,
        "a dump is every plane and the summary, and nothing else",
    );
}

/// A paperdoll window, for the pairing tests below.
fn doll(serial: u32) -> WindowSubject {
    WindowSubject::Paperdoll(Serial::new(serial).expect("a serial"))
}

/// A bare view with our own body entered and nothing else — enough to
/// carry a paperdoll or container entry for the tests below.
fn bare_view() -> openshard_client_net::view::WorldView {
    openshard_client_net::view::WorldView::entered(openshard_protocol::world::PlayerStart {
        serial:   Serial::new(0x0000_002A).expect("a serial"),
        body:     Graphic(0x0190),
        position: Point::new(1475, 1770, 20),
        facing:   Facing::walking(Direction::South),
        map:      openshard_protocol::world::MapSize::BRITANNIA,
    })
}

/// The bug this overlay exists to close: a paperdoll this end closed
/// stays closed even when the next snapshot — cloned from the link
/// thread's own `WorldView` before it has heard about the close — still
/// lists it open. `docs/client_window_state.md`'s S3.
#[test]
fn a_closed_paperdoll_does_not_reopen_on_an_unrelated_world_change() {
    let subject = doll(0x2A);
    let WindowSubject::Paperdoll(serial) = subject else {
        unreachable!()
    };

    let mut view = bare_view();
    view.paperdolls.insert(
        serial,
        openshard_client_net::view::Paperdoll {
            title:    "Someone".to_string(),
            can_lift: false,
        },
    );
    let mut own_windows = Vec::new();
    let mut locally_closed = HashSet::new();

    // The window opens, same as any other frame's sync.
    reconcile_own_windows(&view, &mut own_windows, &mut locally_closed);
    assert!(
        own_windows.iter().any(|window| window.subject == subject),
        "the paperdoll opened"
    );

    // The close: this end's overlay is set, but nothing about `view` —
    // the stand-in for the link thread's copy — has changed yet, exactly
    // as it has not the instant `App::close_window` sends the command.
    own_windows.retain(|window| window.subject != subject);
    locally_closed.insert(subject);

    // An unrelated world change arrives — a mobile's own step would be
    // enough — and is folded into a snapshot that is still, itself,
    // built from the link thread's pre-close copy: `view` here is
    // unchanged, standing in for exactly that clone.
    reconcile_own_windows(&view, &mut own_windows, &mut locally_closed);
    assert!(
        !own_windows.iter().any(|window| window.subject == subject),
        "the closed paperdoll must not reopen just because an unrelated \
         change folded in before the close reached the link thread's view"
    );
    assert!(
        locally_closed.contains(&subject),
        "the overlay survives until the view itself agrees the paperdoll is gone"
    );

    // And now the view itself agrees the paperdoll is gone — the
    // reconciliation this overlay is for. In the running client that is
    // `App::apply_close_window` writing the same fact into this thread's copy,
    // or the shard taking the mobile away with a `0x1D`; the link thread is
    // never involved, and the `Command::CloseWindow` this comment used to name
    // has not existed since S2 in `docs/client_window_state.md`.
    view.paperdolls.remove(&serial);
    reconcile_own_windows(&view, &mut own_windows, &mut locally_closed);
    assert!(
        !locally_closed.contains(&subject),
        "the overlay clears once the view it was ahead of agrees"
    );
    assert!(
        !own_windows.iter().any(|window| window.subject == subject),
        "and the window stays closed, not reopened by the overlay clearing"
    );
}

/// A window's pane is its own kind's, and it is gone the moment the window is.
///
/// The invariant `OwnWindow::pane` exists for, and the half of
/// `docs/window_components.md`'s "no window has private state" that S0 closes: a
/// shop's scroll position used to be an entry in a map on `Windows` that
/// `App::close_window` had to remember to `remove` by hand, so nothing tied it
/// to the window's lifetime and a reopened shop could inherit the last one's
/// scroll. Here the state travels in the record `retain` drops.
///
/// Exercised through `reconcile_own_windows` rather than through `App`, for the
/// reason that function was pulled out to begin with: an `App` needs real client
/// asset files to construct at all.
#[test]
fn a_window_carries_a_pane_of_its_own_kind_and_loses_it_with_the_window() {
    let subject = doll(0x2A);
    let WindowSubject::Paperdoll(serial) = subject else {
        unreachable!()
    };
    let mut view = bare_view();
    view.paperdolls.insert(
        serial,
        openshard_client_net::view::Paperdoll {
            title:    "Someone".to_string(),
            can_lift: false,
        },
    );
    let mut own_windows = Vec::new();
    let mut locally_closed = HashSet::new();

    // Opened with the skills window beside it, so that two kinds are in the
    // list at once and each has to have got its *own* pane rather than the
    // first kind's. The skill sheet is put there by hand because nothing in
    // the view asks for one — being in this list is the whole of "it is open",
    // which is what step 2 of the plan made true.
    crate::windows::open_local_window(&mut own_windows, WindowSubject::Skills);
    reconcile_own_windows(&view, &mut own_windows, &mut locally_closed);
    let paned: Vec<(WindowSubject, bool)> = own_windows
        .iter()
        .map(|window| {
            (
                window.subject,
                matches!(
                    (window.subject, &window.pane),
                    (WindowSubject::Paperdoll(_), crate::panes::AnyPane::Paperdoll(_))
                        | (WindowSubject::Skills, crate::panes::AnyPane::Skills(_))
                ),
            )
        })
        .collect();
    assert_eq!(paned.len(), 2, "the paperdoll and the skill window: {paned:?}");
    assert!(
        paned.iter().all(|(_, matched)| *matched),
        "every window got the pane its kind names: {paned:?}"
    );

    // The shard takes the mobile away. Nothing removes the pane by name — the
    // `retain` that drops the window is what drops it, which is the whole point
    // of the pane living in the record.
    view.paperdolls.remove(&serial);
    reconcile_own_windows(&view, &mut own_windows, &mut locally_closed);
    assert!(
        own_windows
            .iter()
            .all(|window| window.subject == WindowSubject::Skills),
        "the paperdoll's window went, and its pane with it"
    );
}

/// A `0x11` opens nothing, the button opens one window, and the `retain` that
/// closes it is the whole of closing it.
///
/// Step 3 of `docs/window_components.md` deleted the `bool` this used to be
/// asked through: `reconcile_own_windows` took a `status_open` and answered
/// with a window, which is a window's openness kept in two places that could
/// disagree. What is left is the same three facts, said about the list itself.
#[test]
fn a_status_window_is_opened_by_local_intent_not_by_the_status_reply() {
    let view = bare_view();
    let mut own_windows = Vec::new();
    let mut locally_closed = HashSet::new();

    // Entry data. `bare_view` is a world this client is in, and a shard sends
    // the status numbers at that moment — a reconcile over it must still put
    // nothing on the screen.
    reconcile_own_windows(&view, &mut own_windows, &mut locally_closed);
    assert!(own_windows.is_empty(), "entry data alone opens no local window");

    // The button. In the running client this is `doll_clicked`, which asks for a
    // fresh `0x11` and calls exactly this.
    crate::windows::open_local_window(&mut own_windows, WindowSubject::Status);
    assert_eq!(
        own_windows
            .iter()
            .map(|window| window.subject)
            .collect::<Vec<_>>(),
        vec![WindowSubject::Status],
        "the button opens exactly one window"
    );

    // And a second press leaves the one it finds alone, position and all —
    // `open_local_window`'s contract, and the reason it is a door rather than a
    // push.
    crate::windows::open_local_window(&mut own_windows, WindowSubject::Status);
    reconcile_own_windows(&view, &mut own_windows, &mut locally_closed);
    assert_eq!(
        own_windows.len(),
        1,
        "pressing Status again opens no second frame"
    );
    assert!(
        matches!(own_windows[0].pane, crate::panes::AnyPane::Status(_)),
        "and it kept its own pane"
    );

    // Closing is `App::close_window`'s `retain`, which nothing here has to
    // mirror: there is no second copy of the fact for the reconcile to read.
    own_windows.retain(|window| window.subject != WindowSubject::Status);
    reconcile_own_windows(&view, &mut own_windows, &mut locally_closed);
    assert!(own_windows.is_empty(), "closing is local too, and it is the list");
}

/// The two kinds the view cannot answer for, named by a predicate rather than
/// by a list.
///
/// What the disconnect arm of `net_command.rs` retains by, and the reason it is
/// a predicate: a third local kind would otherwise have to be remembered in two
/// places, and the failure mode of forgetting is a window that survives the
/// world it was opened in.
#[test]
fn a_window_is_local_exactly_when_nothing_in_the_view_holds_it_open() {
    let serial = Serial::new(0x0000_002A).expect("a serial");
    assert!(WindowSubject::Skills.is_local());
    assert!(WindowSubject::Status.is_local());
    assert!(WindowSubject::Minimap.is_local());
    assert!(WindowSubject::WorldMap.is_local());
    assert!(
        WindowSubject::Split {
            item: serial,
            most: 5,
        }
        .is_local(),
        "the amount picker is this client's own too — nothing in the view holds it up"
    );
    assert!(!WindowSubject::Container(serial).is_local());
    assert!(!WindowSubject::Vendor(serial).is_local());
    assert!(!WindowSubject::Paperdoll(serial).is_local());
    assert!(!WindowSubject::Dialog(openshard_protocol::gump::GumpId(3)).is_local());
}

#[test]
fn a_minimap_is_a_local_idempotent_window() {
    let view = bare_view();
    let mut own_windows = Vec::new();
    let mut locally_closed = HashSet::new();

    reconcile_own_windows(&view, &mut own_windows, &mut locally_closed);
    assert!(own_windows.is_empty());
    crate::windows::open_local_window(&mut own_windows, WindowSubject::Minimap);
    crate::windows::open_local_window(&mut own_windows, WindowSubject::Minimap);
    assert_eq!(own_windows.len(), 1);
    assert!(matches!(own_windows[0].pane, crate::panes::AnyPane::Minimap(_)));

    reconcile_own_windows(&view, &mut own_windows, &mut locally_closed);
    assert_eq!(own_windows.len(), 1, "the view does not own minimap openness");
    own_windows.retain(|window| window.subject != WindowSubject::Minimap);
    reconcile_own_windows(&view, &mut own_windows, &mut locally_closed);
    assert!(own_windows.is_empty(), "closing it is local too");
}

#[test]
fn a_world_map_is_a_local_idempotent_window() {
    let view = bare_view();
    let mut own_windows = Vec::new();
    let mut locally_closed = HashSet::new();

    crate::windows::open_local_window(&mut own_windows, WindowSubject::WorldMap);
    crate::windows::open_local_window(&mut own_windows, WindowSubject::WorldMap);
    assert_eq!(own_windows.len(), 1);
    assert!(matches!(own_windows[0].pane, crate::panes::AnyPane::WorldMap(_)));

    reconcile_own_windows(&view, &mut own_windows, &mut locally_closed);
    assert_eq!(own_windows.len(), 1, "the view does not own map openness");
}

/// A vendor's buy window is an `OpenContainer` on the vendor serial, whereas
/// the character sheet is an `OpenPaperdoll` on the player serial.  They are
/// independent overlay subjects: opening the first must not make the second
/// disappear or leave it behind the reconciliation layer.
#[test]
fn a_trade_gump_and_own_paperdoll_stay_open_together() {
    let mut view = bare_view();
    let vendor = Serial::new(0x0000_0042).expect("a vendor serial");
    let player = view.player.serial;
    view.containers.insert(vendor, Graphic(0x0030));
    view.paperdolls.insert(
        player,
        openshard_client_net::view::Paperdoll {
            title:    "Someone".to_owned(),
            can_lift: true,
        },
    );
    let mut own_windows = Vec::new();
    let mut locally_closed = HashSet::new();

    reconcile_own_windows(&view, &mut own_windows, &mut locally_closed);

    assert_eq!(
        own_windows
            .iter()
            .map(|window| window.subject)
            .collect::<Vec<_>>(),
        vec![WindowSubject::Container(vendor), WindowSubject::Paperdoll(player)],
        "the later paperdoll is a separate, topmost window over the trade gump"
    );
}

// The scroll-pairing rule lives in `panes::paperdoll` since step 5 of
// `docs/window_components.md`, and so do its tests — including the half the
// old rule had to compare and the new shape answers by construction: two
// clicks on two different dolls cannot pair, because each pane keeps its own
// `last_scroll`.

/// The screen bearings of the eight directions, as the isometric actually
/// draws them — and they are not the grid's. On screen the diamond is
/// turned an eighth: north-east points due right, south-east due down.
/// Anything reading a cursor has to answer in *these* terms, which is the
/// whole reason the heading is measured here rather than on the grid.
#[test]
fn the_screen_bearings_are_the_grid_turned_an_eighth() {
    assert_eq!(on_screen(Direction::NorthEast), (44, 0), "due right");
    assert_eq!(on_screen(Direction::SouthEast), (0, 44), "due down");
    assert_eq!(on_screen(Direction::SouthWest), (-44, 0), "due left");
    assert_eq!(on_screen(Direction::NorthWest), (0, -44), "due up");
    assert_eq!(on_screen(Direction::East), (22, 22), "down and right");
    assert_eq!(on_screen(Direction::North), (22, -22));
    assert_eq!(on_screen(Direction::South), (-22, 22));
    assert_eq!(on_screen(Direction::West), (-22, -22));
}

/// A cursor held away from the body in each of those eight screen bearings
/// asks for that direction — including the one that catches a heading
/// measured on the grid by mistake: straight down the screen is
/// *south-east*, and a grid reading would call it south.
#[test]
fn a_cursor_on_a_screen_bearing_asks_for_that_direction() {
    let body = camera::WorldPixel { x: 0, y: 0 };
    for direction in Direction::ALL {
        let (sx, sy) = on_screen(direction);
        let cursor = camera::WorldPixel { x: sx * 7, y: sy * 7 };
        let heading = heading_to(body, cursor).expect("the cursor is not on the body");
        assert_eq!(heading.direction, direction, "screen bearing {sx},{sy}");
        assert_eq!(
            heading.lean,
            Lean::Centred,
            "squarely on the bearing leans neither way"
        );
    }
}

/// The atlas is grown for the group that will be *drawn*, not for the one
/// the last packet named.
///
/// The two used to be different lists. `App::wanted_in` asked
/// `needed_animations` about `self.world.presentation.player`/`self.world.presentation.others`, built at the last
/// `see`, while `mobiles::collect` drew the group `Crowd::group_for` gives —
/// and `Crowd::advance` moves a body from walking to standing with no packet
/// in between. So a body that stopped was drawn from a standing frame the
/// atlas had never been asked to pack, `mobiles::place` found nothing, and
/// the sprite disappeared — and stayed gone, because a body standing still
/// sends nothing that would rebuild the list.
#[test]
fn the_group_packed_is_the_group_the_crowd_is_playing() {
    const PLAYER: u16 = 400;
    let mut crowd = Crowd::default();
    let facing = Facing::walking(Direction::East);
    crowd.see(
        None,
        Point::new(10, 10, 0),
        Graphic(PLAYER),
        facing,
        Hue::NONE,
        false,
        false,
    );
    // The snapshot the app would store in `self.world.presentation.player`: walking, because a
    // step had just landed when the packet was folded.
    let stepped = crowd.see(
        None,
        Point::new(11, 10, 0),
        Graphic(PLAYER),
        facing,
        Hue::NONE,
        false,
        false,
    );
    let walking = stepped.group;

    // Long enough that the walk gives up on its own timer. No packet.
    crowd.advance(openshard_movement::WALK_HOLD * 2);
    let standing = crowd.group_for(None).expect("the crowd is tracking this body");
    assert_ne!(walking, standing, "the scene is only a scene if the group moved");

    // Through the list `App::wanted_in` grows the atlases from, and not
    // through `advance_groups` directly: what is being protected is that the
    // packing path goes through the refresh at all.
    let drawn = App::everyone_drawn(&crowd, None, &stepped, &[], &[]);
    let mobiles: Vec<Mobile> = drawn.into_iter().map(|(_, mobile)| mobile).collect();
    let wanted = mobiles::needed_animations(&mobiles, &EquipConv::default());
    let (direction, _) = openshard_uofiles::anim::facing(mobiles[0].facing);
    assert!(
        wanted.contains(&openshard_client_render::atlas::AnimationKey::new(
            Graphic(PLAYER),
            standing,
            direction,
        )),
        "the standing group has to be packed to be drawn: {wanted:?}"
    );
}

/// A frame changes only a body's clocks and drawn position. Its equipment is
/// authoritative presentation data until the next server update, so rebuilding
/// the frame list must retain that immutable allocation rather than copying it.
#[test]
fn a_frame_snapshot_reuses_a_mobiles_equipment() {
    let mut crowd = Crowd::default();
    let mut player = crowd.see(
        None,
        Point::new(10, 10, 0),
        Graphic(400),
        Facing::walking(Direction::SouthEast),
        Hue::NONE,
        false,
        false,
    );
    player.equipment = vec![openshard_client_render::mobiles::EquipmentLayer {
        graphic: openshard_tiles::AnimId(7005),
        hue:     Hue::NONE,
        layer:   openshard_protocol::wire::Layer::TUNIC,
    }]
    .into();

    let snapshot = App::everyone_drawn(&crowd, None, &player, &[], &[]);
    assert!(std::rc::Rc::ptr_eq(&player.equipment, &snapshot[0].1.equipment));
}

/// And off the bearing, the lean says which side — which is the thing the
/// eight sectors throw away and the only thing that can settle a corner
/// with two open ways round it. Straight down the screen is south-east;
/// nudged to the right of that, the ask is still south-east but is leaning
/// toward east, which is where east is drawn.
#[test]
fn a_cursor_off_the_bearing_leans_toward_the_side_it_is_on() {
    let body = camera::WorldPixel { x: 0, y: 0 };
    let down_and_right = heading_to(body, camera::WorldPixel { x: 6, y: 300 }).unwrap();
    assert_eq!(down_and_right.direction, Direction::SouthEast);
    assert_eq!(down_and_right.lean, Lean::Counter);

    let down_and_left = heading_to(body, camera::WorldPixel { x: -6, y: 300 }).unwrap();
    assert_eq!(down_and_left.direction, Direction::SouthEast);
    assert_eq!(down_and_left.lean, Lean::Clockwise);
}

/// The cursor on the body names no direction at all, rather than the
/// nearest one: an ask nobody made.
#[test]
fn a_cursor_on_the_body_asks_for_nothing() {
    let body = camera::WorldPixel { x: 17, y: -3 };
    assert_eq!(ask_between(body, body), None);
}

/// And neither does one merely *near* it, all the way round: the dead zone
/// is a disc, so the same distance means the same thing on the diagonal as
/// on the cardinal. This is the bug it exists for — a button held with the
/// mouse sitting still over the character used to walk it off in whichever
/// of the eight directions the last pixel of hand tremor happened to name.
#[test]
fn a_cursor_inside_the_dead_zone_asks_for_nothing() {
    let body = camera::WorldPixel { x: 17, y: -3 };
    for degrees in 0..360 {
        let radians = f64::from(degrees).to_radians();
        let (unit_x, unit_y) = (radians.cos(), radians.sin());
        // Just inside and just outside, in the same bearing: the pair is
        // what pins the radius rather than merely the existence of a zone.
        let at = |distance: f64| {
            camera::WorldPixel {
                x: body.x + (unit_x * distance).round() as i32,
                y: body.y + (unit_y * distance).round() as i32,
            }
        };
        assert_eq!(
            ask_between(body, at(DEAD_ZONE - 2.0)),
            None,
            "{degrees}° inside the dead zone"
        );
        assert!(
            ask_between(body, at(DEAD_ZONE + 2.0)).is_some(),
            "{degrees}° outside the dead zone"
        );
    }
}

/// The ring between the two radii: the cursor names a direction, and what
/// it asks for is a facing rather than a walk. The classic client's, and
/// the only way a mouse can turn a character on the spot — every other ask
/// it makes also sets the body walking.
///
/// Swept all the way round, because the ring is a ring: the same distance
/// has to mean the same thing on the diagonal as on the cardinal, and a
/// zone written as two axis comparisons would be a square.
#[test]
fn a_cursor_inside_the_turn_ring_asks_for_a_facing_and_no_ground() {
    let body = camera::WorldPixel { x: -8, y: 42 };
    let mut checked = 0;
    for degrees in 0..360 {
        let radians = f64::from(degrees).to_radians();
        let (unit_x, unit_y) = (radians.cos(), radians.sin());
        let at = |distance: f64| {
            camera::WorldPixel {
                x: body.x + (unit_x * distance).round() as i32,
                y: body.y + (unit_y * distance).round() as i32,
            }
        };
        // Inside the ring and outside it, on one bearing: the pair is what
        // pins the radius rather than merely the existence of a zone.
        let inside = ask_between(body, at(TURN_ZONE - 2.0)).expect("outside the dead zone");
        assert!(
            matches!(inside, steer::Ask::Turn(_)),
            "{degrees}° inside the turn ring asked to walk: {inside:?}"
        );
        let outside = ask_between(body, at(TURN_ZONE + 2.0)).expect("outside the dead zone");
        assert!(
            matches!(outside, steer::Ask::Walk(_)),
            "{degrees}° outside the turn ring asked only to turn: {outside:?}"
        );
        checked += 1;
    }
    assert_eq!(checked, 360, "every bearing is a case, and every one was checked");

    // And the ring decides what is asked for, never which way: on the eight
    // screen bearings, where rounding a two-pixel offset onto whole pixels
    // cannot tip the answer into a neighbouring sector, both sides of it
    // name the same direction.
    for direction in Direction::ALL {
        let (sx, sy) = on_screen(direction);
        let unit = f64::from(sx).hypot(f64::from(sy));
        let at = |distance: f64| {
            camera::WorldPixel {
                x: body.x + (f64::from(sx) * distance / unit).round() as i32,
                y: body.y + (f64::from(sy) * distance / unit).round() as i32,
            }
        };
        let inside = ask_between(body, at(TURN_ZONE - 2.0)).expect("outside the dead zone");
        let outside = ask_between(body, at(TURN_ZONE + 2.0)).expect("outside the dead zone");
        assert_eq!(inside.heading().direction, direction);
        assert_eq!(outside.heading().direction, direction);
    }
}

/// Where a step stops overshooting — and that [`TURN_ZONE`] reaches it,
/// which is what makes the walk ring start where walking becomes the right
/// answer rather than at a number somebody liked.
///
/// From `22 / cos 22.5°` out, the step this answers with ends *nearer* the
/// cursor than the body started, so the ask cannot reverse. Nearer than
/// that it can, and the dead zone deliberately does not cover the gap: what
/// it exists for is the jitter at a couple of pixels, and a radius half a
/// tile wide would be a hole in the picture the player can feel. This test
/// is here so the number stays a decision — it is derived from the
/// projection, so a tile drawn 2:1 one day moves it, and the constant above
/// has to be re-argued rather than silently left behind.
///
/// Swept over every bearing, because the worst case is not on a direction's
/// own bearing but at the corner of its sector, 22.5° off, where the step
/// spends most of its length going sideways.
#[test]
fn a_step_stops_overshooting_further_out_than_the_dead_zone() {
    // The longest step the projection draws, halved and opened out by the
    // widest the cursor can sit off the bearing that wins its sector.
    let overshoot_free = Direction::ALL
        .into_iter()
        .map(|direction| {
            let (step_x, step_y) = on_screen(direction);
            f64::from(step_x).hypot(f64::from(step_y))
        })
        .fold(0.0_f64, f64::max)
        / (2.0 * 22.5_f64.to_radians().cos());
    assert!(
        DEAD_ZONE < overshoot_free,
        "the dead zone is the smaller of the two on purpose: {DEAD_ZONE} against {overshoot_free}"
    );
    // The band between them is the turn ring, and it has to cover the whole
    // of the overshoot: a cursor anywhere a step would land past is
    // answered with a facing and no ground.
    assert!(
        TURN_ZONE >= overshoot_free,
        "the turn ring stops short of the overshoot: {TURN_ZONE} against {overshoot_free}"
    );

    let body = camera::WorldPixel { x: 0, y: 0 };
    // Counted, because a sweep is only worth what it got to.
    let mut checked = 0;
    for tenths in 0..3600 {
        let radians = (f64::from(tenths) / 10.0).to_radians();
        // Just outside, where a step has the least room to be an
        // improvement — plus the ¾ of a pixel that rounding a bearing onto
        // the whole-pixel grid can move it inward, so every bearing is a
        // case this actually gets to claim something about rather than a
        // skip.
        let distance = TURN_ZONE + 0.75;
        let (cursor_x, cursor_y) = (radians.cos() * distance, radians.sin() * distance);
        let cursor = camera::WorldPixel {
            x: cursor_x.round() as i32,
            y: cursor_y.round() as i32,
        };
        let heading = heading_to(body, cursor).expect("well outside the dead zone");
        let (step_x, step_y) = on_screen(heading.direction);
        let after = f64::from(cursor.x - step_x).hypot(f64::from(cursor.y - step_y));
        let before = f64::from(cursor.x).hypot(f64::from(cursor.y));
        assert!(before > overshoot_free, "the rounding margin holds at {before}");
        assert!(
            after < before,
            "at {}° the step {:?} ends {after} away, having started {before} away",
            f64::from(tenths) / 10.0,
            heading.direction,
        );
        checked += 1;
    }
    assert_eq!(
        checked, 3600,
        "every bearing is a case, and every one was checked"
    );
}

/// The pointer's half of a window's placement: `gump::place`'s inverse, which
/// is what decides whether a click lands on the picture the player is pointing
/// at once a window is drawn bigger than its art.
///
/// The picture's half is pinned in the render crate
/// (`gump::a_magnified_window_picks_what_it_draws`); this is the arithmetic
/// that has to match it, and the case it is here for is the *negative* one. A
/// cursor left of or above a window has a negative local coordinate, and
/// truncating division rounds those toward zero — which would put the column
/// one pixel outside a window's left edge on column `0`, inside whatever
/// picture starts there, and hand a pane a click that landed outside it.
#[test]
fn a_windows_own_cursor_is_its_placement_and_its_scale_undone() {
    use openshard_client_render::gump::GumpPixel;

    use crate::desk::WindowScale;
    use crate::windows::OwnWindow;

    let subject = WindowSubject::Skills;
    let window = OwnWindow {
        subject,
        at: GumpPixel::new(300, 200),
        pane: crate::panes::AnyPane::of(subject),
    };

    // At the art's own size the placement is a subtraction and nothing else —
    // what this client did before the scale existed, and what a saved file
    // without one still asks for.
    assert_eq!(
        window.local_cursor(GumpPixel::new(318, 218), WindowScale::new(1.0)),
        GumpPixel::new(18, 18)
    );

    // Doubled, the same picture is under a cursor twice as far into the
    // window: `(336, 236)` is local `(18, 18)` drawn at two pixels each.
    assert_eq!(
        window.local_cursor(GumpPixel::new(336, 236), WindowScale::new(2.0)),
        GumpPixel::new(18, 18)
    );
    // And the pixel between two drawn ones belongs to the earlier of them,
    // which is the same floor `place` drew it with.
    assert_eq!(
        window.local_cursor(GumpPixel::new(337, 237), WindowScale::new(2.0)),
        GumpPixel::new(18, 18)
    );

    // Outside, up and to the left: negative, and not rounded up into the
    // window. One pixel out at twice the scale is still one pixel out.
    assert_eq!(
        window.local_cursor(GumpPixel::new(299, 199), WindowScale::new(2.0)),
        GumpPixel::new(-1, -1)
    );
    assert_eq!(
        window.local_cursor(GumpPixel::new(298, 198), WindowScale::new(2.0)),
        GumpPixel::new(-1, -1)
    );
    assert_eq!(
        window.local_cursor(GumpPixel::new(297, 197), WindowScale::new(2.0)),
        GumpPixel::new(-2, -2)
    );

    // A fraction, where the two directions have to agree about a boundary that
    // is not on a pixel: at 1.5 the art pixel `12` is drawn from screen 18.0 up
    // to 19.5, so both 18 and 19 into the window are that pixel and 20 is the
    // next one. Floor is what makes that true, and the same floor is why the
    // negative side above does not round toward zero.
    let three_halves = WindowScale::new(1.5);
    assert_eq!(
        window.local_cursor(GumpPixel::new(318, 218), three_halves),
        GumpPixel::new(12, 12)
    );
    assert_eq!(
        window.local_cursor(GumpPixel::new(319, 219), three_halves),
        GumpPixel::new(12, 12)
    );
    assert_eq!(
        window.local_cursor(GumpPixel::new(320, 220), three_halves),
        GumpPixel::new(13, 13)
    );
    assert_eq!(
        window.local_cursor(GumpPixel::new(299, 199), three_halves),
        GumpPixel::new(-1, -1)
    );
}
