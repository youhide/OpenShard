//! The status frame as a component: the third window kind to move in, and the
//! one with the least of its own to remember.
//!
//! Step 3 of `docs/client/design_panes.md`. There is still nothing here to keep
//! between frames — every number and every arrow is read out of the view as the
//! window is drawn — so what the pane is, is a layout, the fact that the window
//! is open, and one control: the stat arrows, which the frame gained when the
//! client learned to decode `0xBF 0x19`. Step 2 had already made the openness a
//! place in [`Windows::own_windows`](crate::windows::Windows::own_windows)
//! rather than a `bool` beside it.
//!
//! # What the frame is, on the wire
//!
//! A `0x11`, and a `0xBF 0x19` beside it. The first carries the name, the three
//! stats, the armour, the gold, the weight and — since the AoS tail stopped
//! being skipped — the resistances, luck, weapon damage and the suit bonuses the
//! modern frame has columns for. The second carries the three arrows, which
//! cannot be worked out from any of those numbers.
//!
//! Neither carries a **window**: the shard sends both at world entry, so a
//! client that opened a frame on the data would open one at every login. That is
//! why this kind's *existence* is local — see
//! [`LocalWindow`](crate::panes::LocalWindow) — and why the Status button both
//! asks for a fresh `0x11` and opens the window, in that order.
//!
//! The health line is not in that packet's copy of the numbers:
//! [`Player::hits`](openshard_client_net::view::Player::hits) is shared with
//! `0xA1`, which refreshes it between status replies, and the layout below reads
//! it from there so the bar over the player's head and the line on this frame
//! cannot disagree.
//!
//! # Which frame
//!
//! Two of them — [`status::Form`](openshard_client_render::status::Form) — and
//! the choice is a setting rather than anything this pane holds. It arrives as
//! [`PaneFrame::status_form`], read from `client_ui.ron` through the shell; see
//! [`desk::StatusFrame`](crate::desk::StatusFrame) for why the preference lives
//! there and not here.

use openshard_client_net::action::Outgoing;
use openshard_client_render::gump::{
    self as gump_art,
    GumpArt,
    GumpPixel,
};
use openshard_client_render::status;

use crate::panes::{
    Button,
    Effect,
    Input,
    PaneCtx,
    PaneFrame,
    Response,
};
use crate::windows::Drawn;

/// This character's status frame, open.
///
/// A unit struct, and that is the whole statement of the step: the two kinds
/// before it moved a scroll position, a set of shut headings and a held control
/// out of `Windows`, and this one has nothing of the sort to move. What it
/// gains by being a pane anyway is that it is laid out by the same call as its
/// neighbours, and that its openness is the same fact as theirs.
#[derive(Debug, Default)]
pub struct StatusPane;

impl StatusPane {
    /// Nothing of its own, for [`SkillsPane`](crate::panes::skills)'s reason:
    /// the frame art and the glyphs over it are offered to the atlas by the
    /// sweep over every laid-out window at the end of
    /// `render_passes::draw_gump_windows`, and naming them here would be a
    /// second answer to "what does this window draw", worked out before the
    /// layout that decides it.
    pub(super) fn art(&self, _frame: &PaneFrame<'_>) -> Vec<GumpArt> {
        Vec::new()
    }

    /// The frame, and the numbers written over it out of the view.
    ///
    /// `None` is **reachable here**, unlike the skill sheet's: the Status button
    /// opens the window and asks for a `0x11` in the same press, so there is a
    /// frame or two in which the window is open and this client has never been
    /// told a single one of the values. Drawing the empty frame then would be
    /// drawing a status window belonging to nobody; drawing nothing is the
    /// honest picture, and the window becomes visible with its numbers on the
    /// frame the reply lands.
    ///
    /// The arrows are **not** part of that gate. They arrive in their own
    /// `0xBF 0x19`, which may land before or after the status reply, so the
    /// layout takes whatever the view has and draws none while that is nothing
    /// — see `status::Numbers::locks`.
    pub(super) fn layout(&self, frame: &PaneFrame<'_>) -> Option<Drawn> {
        let player = &frame.view.player;
        Some(Drawn::Status(status::window(
            frame.status_form,
            status::Numbers {
                status: player.status.as_ref()?,
                hits:   player.hits?,
                mana:   player.mana?,
                locks:  player.stat_locks,
            },
            |text, font| openshard_client_render::text::gump_width(text, font, frame.files.font_atlas),
            // Window-local — see `PaneFrame::cursor`'s doc.
            GumpPixel::new(0, 0),
        )))
    }

    /// One control, and it is the same one on both frames: a stat's arrow.
    ///
    /// A press on one asks the shard to move it round `up → down → held → up`,
    /// the reference client's `(lock + 1) % 3`. Nothing is drawn differently
    /// here on the press itself, and that is deliberate: unlike the skill
    /// sheet's arrow — whose `0x3A` is never answered, so the sheet must turn
    /// its own picture over — this shard replies with a fresh `0xBF 0x19`
    /// ([`World::set_stat_lock`]), and drawing a predicted arrow beside it
    /// would be a second answer that is wrong whenever the shard refuses.
    ///
    /// Everything else a player aims at this window is the manager's: raising
    /// it, picking it up by any other pixel, and the right button that closes
    /// it. Decision 2 is what keeps those out of here.
    pub(super) fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response {
        let Input::Press(Button::Left) = input else {
            return Response::ignored();
        };
        if !ctx.under_pointer {
            return Response::ignored();
        }
        let Some(Drawn::Status(window)) = ctx.drawn else {
            return Response::ignored();
        };
        // Against what was *drawn*, never a fresh layout — `PaneCtx::drawn`'s
        // rule, and the pick is over the arrows' own opaque pixels rather than
        // the boxes around them.
        let Some(stat) = gump_art::pick(&window.pictures, ctx.frame.cursor, ctx.frame.files.gump_atlas)
            .and_then(|index| window.locks.get(&index).copied())
        else {
            return Response::ignored();
        };
        // The arrow's current face comes from the view, which is the one home
        // for it: the picture under the pointer says *which* stat and the shard
        // says where its arrow points.
        let Some(locks) = ctx.frame.view.player.stat_locks else {
            return Response::ignored();
        };
        Response::changed()
            .with(Effect::Raise)
            .with(Effect::Net(Outgoing::StatLock {
                stat,
                lock: status::next(status::arrow_of(locks, stat)),
            }))
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::serial::Serial;
    use openshard_protocol::skill::SkillLock;

    use super::*;
    use crate::panes::fixture;

    /// **S3's `None` gap, closed through `AnyPane::layout`.** A `0x1B` opens no
    /// window (see the module docs), so the Status button's own path opens one
    /// and asks for a fresh `0x11` in the same press — and the frame or two
    /// before that reply lands is a window with nothing to draw. Drawing the
    /// empty frame would draw a status window belonging to nobody; `None` is
    /// the honest answer, and `fixture::world` is exactly that gap — a `0x1B`
    /// and nothing after it, so `player.status` and `player.hits` are both
    /// `None` the way they are on the frame the window opens.
    #[test]
    fn a_frame_with_no_status_reply_yet_lays_out_nothing() {
        let files = fixture::Install::shipping([]);
        let view = fixture::world(Serial::new(0x0000_002A).unwrap());
        let pane = StatusPane;
        let laid_out = pane.layout(&files.ctx(&view, None, GumpPixel::new(0, 0), true).frame);
        assert!(
            laid_out.is_none(),
            "no `0x11` has arrived, so there is nothing to draw yet"
        );
    }

    /// **Decision 2's half that survived the arrows.** A press with nothing
    /// drawn behind it is nobody's: the manager's raise-and-grab takes it, and
    /// this pane never reaches into a layout that does not exist.
    #[test]
    fn a_press_with_no_layout_behind_it_is_still_ignored() {
        let files = fixture::Install::shipping([]);
        let view = fixture::world(Serial::new(0x0000_002A).unwrap());
        let mut pane = StatusPane;
        let ctx = files.ctx(&view, None, GumpPixel::new(0, 0), true);
        let answer = pane.handle(Input::Press(Button::Left), &ctx);
        assert!(!answer.taken);
        assert!(!answer.redraw);
        assert!(answer.out.is_empty());
    }

    /// A world with a status reply and a set of arrows already in it — the
    /// state the frame is drawn from once a shard has answered.
    fn answered(player: Serial) -> openshard_client_net::view::WorldView {
        use openshard_protocol::mobile::{
            AosStatus,
            DamageRange,
            MobileStatus,
            Resistances,
            StatLockBits,
            StatLocks,
            Vitals,
        };
        use openshard_protocol::server_packet::ServerPacket;

        let mut view = fixture::world(player);
        view.apply(&ServerPacket::MobileStatus(MobileStatus {
            serial:        player,
            name:          "Lord British".to_owned(),
            hits:          Vitals {
                current: 98,
                max:     100,
            },
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
            resistances:   Resistances::NONE,
            luck:          0,
            damage:        DamageRange::BARE,
            tithing:       0,
            aos:           AosStatus::NONE,
        }));
        view.apply(&ServerPacket::StatLocks(StatLocks {
            serial: player,
            locks:  StatLockBits {
                strength:     SkillLock::Up,
                dexterity:    SkillLock::Down,
                intelligence: SkillLock::Locked,
            },
        }));
        view
    }

    /// **The frame's one control.** A press on dexterity's arrow asks the shard
    /// to move that arrow on by one — down becomes held — and asks for nothing
    /// else: no local repaint of the arrow, because this shard answers with a
    /// fresh `0xBF 0x19` and a predicted face would be a second answer.
    #[test]
    fn a_press_on_an_arrow_asks_the_shard_to_move_that_one() {
        let player = Serial::new(0x0000_002A).unwrap();
        // The frame and the three arrow faces, each a block big enough to be
        // picked: the arrows sit 12 pixels apart, so 10×10 keeps them apart.
        let files = fixture::Install::shipping([
            (
                GumpArt::Gump(openshard_protocol::wire::Graphic(0x0802)),
                (282, 151),
            ),
            (GumpArt::Gump(openshard_client_render::lock::UP), (10, 10)),
            (GumpArt::Gump(openshard_client_render::lock::DOWN), (10, 10)),
            (GumpArt::Gump(openshard_client_render::lock::HELD), (10, 10)),
        ]);
        let view = answered(player);
        let mut pane = StatusPane;

        let laid_out = pane
            .layout(&files.ctx(&view, None, GumpPixel::new(0, 0), true).frame)
            .expect("a status reply has landed, so the frame draws");
        assert_eq!(
            laid_out.pictures().len(),
            4,
            "the frame and the three arrows the shard stated"
        );

        // Dexterity's arrow is the second, at (28, 74) — a press two pixels
        // into it.
        let ctx = files.ctx(&view, Some(&laid_out), GumpPixel::new(30, 76), true);
        let answer = pane.handle(Input::Press(Button::Left), &ctx);
        assert!(answer.taken, "the arrow takes the press away from the drag");
        let asked: Vec<_> = answer
            .out
            .iter()
            .filter_map(|effect| {
                match effect {
                    Effect::Net(out) => Some(out),
                    _ => None,
                }
            })
            .collect();
        assert_eq!(asked.len(), 1, "one packet, and nothing else is asked for");
        assert!(
            matches!(
                asked[0],
                Outgoing::StatLock {
                    stat: openshard_protocol::mobile::Stat::Dexterity,
                    lock: SkillLock::Locked,
                }
            ),
            "down moves on to held, for the stat that was pressed"
        );
    }

    /// A press on the frame itself is the manager's, arrows or no arrows: it
    /// picks the window up. Without this the whole plate would answer as if it
    /// were a control the moment the arrows appeared.
    #[test]
    fn a_press_beside_the_arrows_is_still_the_windows_own() {
        let player = Serial::new(0x0000_002A).unwrap();
        let files = fixture::Install::shipping([
            (
                GumpArt::Gump(openshard_protocol::wire::Graphic(0x0802)),
                (282, 151),
            ),
            (GumpArt::Gump(openshard_client_render::lock::UP), (10, 10)),
            (GumpArt::Gump(openshard_client_render::lock::DOWN), (10, 10)),
            (GumpArt::Gump(openshard_client_render::lock::HELD), (10, 10)),
        ]);
        let view = answered(player);
        let mut pane = StatusPane;
        let laid_out = pane
            .layout(&files.ctx(&view, None, GumpPixel::new(0, 0), true).frame)
            .expect("the frame draws");

        let ctx = files.ctx(&view, Some(&laid_out), GumpPixel::new(200, 120), true);
        let answer = pane.handle(Input::Press(Button::Left), &ctx);
        assert!(!answer.taken, "nothing here is a control");
        assert!(answer.out.is_empty());
    }
}
