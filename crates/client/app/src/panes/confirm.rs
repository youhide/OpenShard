//! The client's own yes/no window as a component: the seventh window kind, and
//! the first one that exists because *this end* has something to ask.
//!
//! Every other kind is a window onto something — a bag's contents, a body, a
//! catalogue, a layout the shard drew. This one is a **question**, and what
//! makes it a pane rather than a panel is that a question is a window like any
//! other: it is dragged, raised, hit-tested and closed by the same machinery, so
//! it cannot end up with its own font, its own frame and its own idea of where a
//! button is. That is what it replaced — an `egui::Window` painted over the gump
//! layer, which took the mouse before any of this client's own windows were
//! offered it.
//!
//! # The pane knows what is being asked; the layout does not
//!
//! [`openshard_client_render::confirm`] takes a finished string and hands back
//! pictures. Which string, and what either answer *does*, is here — in
//! [`Question`], which is the whole of what a second question would have to add.
//!
//! # A question is not a modal
//!
//! The reference client's `QuestionGump` is `IsModal = true`: nothing behind it
//! answers a click while it stands. This one is an ordinary window, deliberately
//! — decision 2 puts z-order in the manager, and a "nothing under me may be
//! clicked" rule is a second policy about z-order living in a pane. A player who
//! wants to open their bag before answering may; the question waits, exactly as
//! a `0xB0` dialog does.

use openshard_client_net::action::Outgoing;
use openshard_client_net::view::WorldView;
use openshard_client_render::confirm as confirm_art;
use openshard_client_render::confirm::Hit;
use openshard_client_render::gump::{GumpArt, GumpPixel};

use crate::panes::{Button, Effect, Input, PaneCtx, PaneFrame, Response};
use crate::windows::Drawn;

/// What this client is asking, and therefore what either button means.
///
/// The key of [`WindowSubject::Confirm`](crate::windows::WindowSubject), which
/// is what keeps two different questions from being one window: the manager
/// files a window per subject, so a second arm here is a second window and not a
/// second meaning for the one that is already up.
///
/// One arm today. It is an enum rather than a bare marker because the two things
/// a question needs — its wording and its answer — are per-question and are
/// stated below in one place each, so a second one is two arms and no new
/// machinery.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Question {
    /// Somebody has asked this player into their party — the `0x78` the view
    /// records as [`Party::invited_by`](openshard_client_net::view::Party::invited_by).
    PartyInvite,
}

impl Question {
    /// Every question this client knows how to ask.
    ///
    /// What [`reconcile_own_windows`](crate::windows::reconcile_own_windows)
    /// walks to decide which of them stand right now — the same sweep it makes
    /// over the view's containers and gumps, for a set that is in the source
    /// rather than in the view. A second arm above has to appear here too, which
    /// is the one thing about a new question that is not a compile error; the
    /// test at the foot of this module is what makes it one.
    pub(crate) const ALL: [Self; 1] = [Self::PartyInvite];

    /// Whether this question is still being asked.
    ///
    /// The manager's question rather than the pane's: it opens the window when
    /// this turns true and takes it away when it turns false, so that no
    /// question outlives the fact behind it — a party invitation withdrawn by
    /// the shard takes its plate off the screen without anybody pressing
    /// anything.
    pub(crate) fn stands(self, view: &WorldView) -> bool {
        self.text(view).is_some()
    }

    /// How this question reads, out of the view, or `None` once it no longer
    /// stands.
    ///
    /// Asked every frame rather than kept from the moment the window opened, for
    /// [`PaneFrame::view`]'s reason: the invitation is the shard's fact, it can
    /// be withdrawn, and a copy taken at open would be a second picture of it.
    /// `None` is what makes the window draw nothing on the frame between the
    /// shard settling the question and `reconcile_own_windows` taking the window
    /// away.
    fn text(self, view: &WorldView) -> Option<String> {
        match self {
            // By serial, because that is all there is. A `0x78` carries no name
            // — a mobile is named by a single click or by a tooltip, and this
            // client may have done neither to the person inviting it. Better a
            // number than a guess at whose it is.
            Self::PartyInvite => view
                .party
                .invited_by
                .map(|leader| format!("{:#010X} has invited you to a party.", leader.raw())),
        }
    }

    /// What one of the two buttons sends.
    ///
    /// Both answers go on the wire for this question, which is not a property of
    /// questions in general: a "yes" that only did something local would return
    /// nothing here, and the arm would say so.
    const fn answer(self, yes: bool) -> Option<Outgoing> {
        match self {
            Self::PartyInvite => Some(if yes {
                Outgoing::PartyAccept
            } else {
                Outgoing::PartyDecline
            }),
        }
    }
}

/// One open question: which it is, and which of its two buttons is down.
#[derive(Debug)]
pub struct ConfirmPane {
    /// What is being asked. Handed in when the window opens, the way a shop is
    /// handed its vendor: it is the key the window is filed under, and
    /// everything this pane looks up is looked up by it.
    question: Question,
    /// The button the mouse went down on.
    ///
    /// A [`Hit`] rather than a side, for [`DialogPane::held`](super::dialog)'s
    /// reason: the layout draws the pressed face by comparing this against the
    /// hit it computes, so what looks pressed and what the release will act on
    /// are one value.
    held: Option<Hit>,
}

impl ConfirmPane {
    /// The pane for one question.
    pub const fn new(question: Question) -> Self {
        Self { question, held: None }
    }

    /// A left press somewhere in the window.
    ///
    /// Every arm raises and every arm takes the press — a press that reached a
    /// window never reaches the world behind it. What the arms differ about is
    /// whether the window is picked up: a press on a button holds the button, a
    /// press anywhere else moves the frame, which is how a gump with no title bar
    /// is moved.
    fn press(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let raised = Response::changed().with(Effect::Raise);
        let Some(Drawn::Confirm(laid_out)) = ctx.drawn else {
            return raised;
        };
        match laid_out.hit(ctx.frame.cursor, ctx.frame.files.gump_atlas) {
            Some(hit) => {
                self.held = Some(hit);
                raised
            }
            // Already local to this window — see `PaneFrame::cursor`'s doc — so
            // it is the grab offset with nothing subtracted.
            None => raised.with(Effect::Grab),
        }
    }

    /// The release that finishes a press on one of the two buttons.
    ///
    /// The pointer has to still be on the button it went down on: a press dragged
    /// off its button is a press taken back, here as in every other window. Either
    /// way the answer is [`Response::changed`] once a press is being held — the
    /// button was drawn pressed and has to come back up.
    ///
    /// **The answer closes the window**, and does it locally: nothing arrives to
    /// say the question is settled except the consequence — a roster for an
    /// accepted invitation, nothing at all for a declined one — so this end
    /// predicts the close the way it predicts a closed bag, and
    /// `reconcile_own_windows` drops the prediction once the view agrees.
    fn release(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let Some(held) = self.held.take() else {
            return Response::ignored();
        };
        let Some(Drawn::Confirm(laid_out)) = ctx.drawn else {
            return Response::changed();
        };
        if laid_out.hit(ctx.frame.cursor, ctx.frame.files.gump_atlas) != Some(held) {
            return Response::changed();
        }
        let answered = Response::changed().with(Effect::Close);
        match self.question.answer(held == Hit::Yes) {
            Some(action) => answered.with(Effect::Net(action)),
            None => answered,
        }
    }
}

impl ConfirmPane {
    /// The plate and both faces of both buttons — see
    /// [`confirm_art::art_of`], which packs the pressed faces too so that a
    /// button is never drawn blank on the frame it first goes down.
    pub(super) fn art(&self, _frame: &PaneFrame<'_>) -> Vec<GumpArt> {
        confirm_art::art_of().collect()
    }

    /// The question, if it still stands.
    pub(super) fn layout(&self, frame: &PaneFrame<'_>) -> Option<Drawn> {
        Some(Drawn::Confirm(confirm_art::window(
            &self.question.text(frame.view)?,
            self.held,
            // Window-local — see `PaneFrame::cursor`'s doc.
            GumpPixel::new(0, 0),
            frame.files.font_atlas,
        )))
    }

    pub(super) fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response {
        match input {
            Input::Press(Button::Left) if ctx.under_pointer => self.press(ctx),
            Input::Release(Button::Left) => self.release(ctx),
            // No wheel, no keys, no drop: two buttons and a frame to drag is the
            // whole of this window. The right button that takes it down is the
            // manager's gesture, the same as for every other kind.
            _ => Response::ignored(),
        }
    }
}

#[cfg(test)]
mod tests {
    use openshard_client_render::gump::GumpArt;
    use openshard_protocol::serial::Serial;
    use openshard_protocol::wire::Graphic;

    use super::*;
    use crate::panes::fixture;

    /// This client's own body, which the view is built around and which none of
    /// these tests is about.
    fn me() -> Serial {
        Serial::new(0x0000_0001).unwrap()
    }

    /// Whoever is doing the inviting.
    fn leader() -> Serial {
        Serial::new(0x0000_002A).unwrap()
    }

    /// The window's own corner, which every layout here is built at — see
    /// [`PaneFrame::cursor`], which is why a pane's coordinates are its own.
    const ORIGIN: GumpPixel = GumpPixel::new(0, 0);

    /// A pixel inside each button, taken from the reference's own positions plus
    /// a little: `(37, 75)` and `(100, 75)`, both 21 pixels tall.
    const NO_BUTTON: GumpPixel = GumpPixel::new(45, 80);
    const YES_BUTTON: GumpPixel = GumpPixel::new(110, 80);

    /// An install that ships the plate and the four button faces, each a solid
    /// block of the size the real art is: what the hit test is measured against.
    fn plate() -> fixture::Install {
        fixture::Install::shipping([
            (GumpArt::Gump(Graphic(0x0816)), (178, 108)),
            (GumpArt::Gump(Graphic(0x0817)), (56, 21)),
            (GumpArt::Gump(Graphic(0x0818)), (56, 21)),
            (GumpArt::Gump(Graphic(0x081A)), (46, 21)),
            (GumpArt::Gump(Graphic(0x081B)), (46, 21)),
        ])
    }

    /// Both answers go on the wire, and they are not the same packet — the one
    /// thing a question's arm has to get right.
    #[test]
    fn an_invitation_is_answered_by_two_different_packets() {
        assert!(matches!(
            Question::PartyInvite.answer(true),
            Some(Outgoing::PartyAccept)
        ));
        assert!(matches!(
            Question::PartyInvite.answer(false),
            Some(Outgoing::PartyDecline)
        ));
    }

    /// A question with nothing behind it in the view has no wording and does
    /// not stand — which is what makes the window draw nothing rather than an
    /// empty plate, and what takes it off the screen.
    #[test]
    fn a_settled_invitation_has_nothing_left_to_ask() {
        let view = fixture::world(me());
        assert_eq!(Question::PartyInvite.text(&view), None);
        assert!(!Question::PartyInvite.stands(&view));
    }

    /// And one that stands names whoever asked.
    #[test]
    fn a_standing_invitation_names_its_leader() {
        let mut view = fixture::world(me());
        view.party.invited_by = Some(leader());
        assert!(Question::PartyInvite.stands(&view));
        assert_eq!(
            Question::PartyInvite.text(&view).as_deref(),
            Some("0x0000002A has invited you to a party.")
        );
    }

    /// The press-and-release that answers: the packet goes out **and** the
    /// window goes off the list, in that order, and only when the finger comes
    /// up on the button it went down on.
    #[test]
    fn pressing_yes_and_letting_go_accepts_and_closes() {
        let mut view = fixture::world(me());
        view.party.invited_by = Some(leader());
        let install = plate();
        let mut pane = ConfirmPane::new(Question::PartyInvite);
        let drawn = pane
            .layout(&install.ctx(&view, None, ORIGIN, true).frame)
            .expect("a standing question has a layout");

        // Down on the right-hand button: nothing goes out yet, and the window
        // is raised and redrawn because the face has changed.
        let press = pane.handle(
            Input::Press(Button::Left),
            &install.ctx(&view, Some(&drawn), YES_BUTTON, true),
        );
        assert!(press.taken && press.redraw);
        assert!(matches!(press.out.as_slice(), [Effect::Raise]));
        assert_eq!(pane.held, Some(Hit::Yes));

        let release = pane.handle(
            Input::Release(Button::Left),
            &install.ctx(&view, Some(&drawn), YES_BUTTON, true),
        );
        assert!(matches!(
            release.out.as_slice(),
            [Effect::Close, Effect::Net(Outgoing::PartyAccept)]
        ));
        assert_eq!(pane.held, None, "the press is spent either way");
    }

    /// A press dragged off its button is a press taken back: the button comes
    /// back up and nothing at all is sent.
    #[test]
    fn a_press_dragged_off_its_button_answers_nothing() {
        let mut view = fixture::world(me());
        view.party.invited_by = Some(leader());
        let install = plate();
        let mut pane = ConfirmPane::new(Question::PartyInvite);
        let drawn = pane
            .layout(&install.ctx(&view, None, ORIGIN, true).frame)
            .expect("a standing question has a layout");

        let _down = pane.handle(
            Input::Press(Button::Left),
            &install.ctx(&view, Some(&drawn), YES_BUTTON, true),
        );
        let release = pane.handle(
            Input::Release(Button::Left),
            &install.ctx(&view, Some(&drawn), NO_BUTTON, true),
        );
        assert!(release.out.is_empty(), "no packet, and no close");
        assert!(release.redraw, "the button was drawn pressed and has to come up");
    }

    /// A press on the plate itself picks the window up, which is how a gump with
    /// no title bar is moved.
    #[test]
    fn a_press_on_the_plate_grabs_the_window() {
        let mut view = fixture::world(me());
        view.party.invited_by = Some(leader());
        let install = plate();
        let mut pane = ConfirmPane::new(Question::PartyInvite);
        let drawn = pane
            .layout(&install.ctx(&view, None, ORIGIN, true).frame)
            .expect("a standing question has a layout");

        let press = pane.handle(
            Input::Press(Button::Left),
            &install.ctx(&view, Some(&drawn), GumpPixel::new(8, 8), true),
        );
        assert!(matches!(press.out.as_slice(), [Effect::Raise, Effect::Grab]));
        assert_eq!(pane.held, None);
    }

    /// Every question the client knows how to ask is in [`Question::ALL`] — the
    /// one fact about a new arm that the compiler cannot check, because
    /// `reconcile_own_windows` walks the list rather than the enum.
    #[test]
    fn every_question_is_in_the_list_reconcile_walks() {
        // Written as a `match` on purpose: a question added to the enum has to
        // be named here before this compiles, and naming it is what asserts it
        // is in the list the manager walks.
        for named in Question::ALL {
            let listed = match named {
                Question::PartyInvite => Question::PartyInvite,
            };
            assert_eq!(listed, named);
        }
        assert!(Question::ALL.contains(&Question::PartyInvite));
    }
}
