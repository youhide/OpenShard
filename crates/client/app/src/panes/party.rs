//! The party manifest as a component: who is in the party, and the three things
//! this client can do about it.
//!
//! The eighth window kind, and the second one this session took off egui — see
//! [`super::confirm`] for the first. Both were `egui::Window`s drawn over the
//! gump layer off the same [`Party`](openshard_client_net::view::Party) this
//! reads, which meant two window systems, two fonts, and a click that egui
//! claimed before any of this client's own windows were offered it.
//!
//! # It holds no roster
//!
//! Nothing here remembers who is in the party. The roster arrives whole on every
//! change — a `0xBF 0x06` sub-command carries the *whole list*, never a
//! difference — so this pane reads [`PaneFrame::view`] every frame and a
//! [`Hit`] names a **row**, not a serial. The lookup from row to member happens
//! at the release, against the roster as it stands then; a serial captured when
//! the window was drawn would be this client kicking whoever used to be on that
//! line.

use openshard_client_net::action::Outgoing;
use openshard_client_net::view::WorldView;
use openshard_client_render::gump::{
    GumpArt,
    GumpPixel,
};
use openshard_client_render::party as party_art;
use openshard_client_render::party::Hit;
use openshard_protocol::serial::Serial;

use crate::panes::{
    Button,
    Effect,
    Input,
    PaneCtx,
    PaneFrame,
    Response,
};
use crate::windows::Drawn;

/// The open manifest: which of its controls is down, and nothing else.
#[derive(Debug, Default)]
pub struct PartyPane {
    /// The control the mouse went down on — [`super::confirm::ConfirmPane`]'s
    /// field, for the same reason: the layout draws the pressed face by
    /// comparing this against the hit it computes, so what looks pressed and
    /// what the release will act on are one value.
    held: Option<Hit>,
}

/// Whether this client's own body leads the party it is in.
///
/// The roster is leader-first and the wire says so nowhere else — see
/// [`Party::leader`](openshard_client_net::view::Party::leader) — so this is the
/// one place the question is asked, and both readers (the layout and the
/// release) ask it here rather than each comparing the first row themselves.
fn leading(view: &WorldView) -> bool {
    view.party.leader() == Some(view.player.serial)
}

/// Whether there is a party at all, which is the whole of whether this window
/// exists.
///
/// The manager's question, the way
/// [`Question::stands`](super::confirm::Question::stands) is: the window opens
/// when a roster arrives and goes when the last member leaves, and nobody has to
/// press anything for either.
pub(crate) fn in_a_party(view: &WorldView) -> bool {
    !view.party.is_empty()
}

impl PartyPane {
    /// A left press somewhere in the window: a control goes down, or the frame
    /// is picked up.
    fn press(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let raised = Response::changed().with(Effect::Raise);
        let Some(Drawn::Party(laid_out)) = ctx.drawn else {
            return raised;
        };
        match laid_out.hit(ctx.frame.cursor, ctx.frame.files.gump_atlas) {
            Some(hit) => {
                self.held = Some(hit);
                raised
            }
            // A name plate, a heading, the background: nothing that answers, so
            // the press moves the window instead. Already local to this window —
            // see `PaneFrame::cursor`'s doc.
            None => raised.with(Effect::Grab),
        }
    }

    /// The release that finishes a press on one of the controls.
    ///
    /// The pointer has to still be on the control it went down on, the same rule
    /// every other window's buttons follow.
    fn release(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let Some(held) = self.held.take() else {
            return Response::ignored();
        };
        let Some(Drawn::Party(laid_out)) = ctx.drawn else {
            return Response::changed();
        };
        if laid_out.hit(ctx.frame.cursor, ctx.frame.files.gump_atlas) != Some(held) {
            return Response::changed();
        }
        let view = ctx.frame.view;
        match held {
            // The roster as it stands *now*, not as it stood when the window was
            // drawn — see the module docs. A row the roster has since lost is a
            // press that answers nothing rather than one that turns out the
            // wrong person.
            Hit::Kick(row) => {
                match view.party.members.get(row).copied() {
                    Some(member) => Response::changed().with(Effect::Net(Outgoing::PartyRemove(member))),
                    None => Response::changed(),
                }
            }
            // Leaving is `0x02` naming *yourself* — the same packet a leader
            // kicks with, which is the wire's own shape rather than this
            // client's shortcut. See `openshard_client_net::party::remove`.
            Hit::Leave => Response::changed().with(Effect::Net(Outgoing::PartyRemove(me(view)))),
            Hit::Add => Response::changed().with(Effect::Net(Outgoing::PartyAdd)),
            // The one control that sends nothing: this window is a view of the
            // roster, and putting it away leaves the party alone.
            Hit::Close => Response::changed().with(Effect::Close),
        }
    }
}

/// This client's own body.
fn me(view: &WorldView) -> Serial {
    view.player.serial
}

impl PartyPane {
    /// The background's nine pieces, the name plate, and both faces of every
    /// control — see [`party_art::art_of`].
    pub(super) fn art(&self, _frame: &PaneFrame<'_>) -> Vec<GumpArt> {
        party_art::art_of().collect()
    }

    /// The roster, if there is one.
    pub(super) fn layout(&self, frame: &PaneFrame<'_>) -> Option<Drawn> {
        if !in_a_party(frame.view) {
            return None;
        }
        Some(Drawn::Party(party_art::window(
            &frame.view.party.members,
            leading(frame.view),
            self.held,
            // Window-local — see `PaneFrame::cursor`'s doc.
            GumpPixel::new(0, 0),
            frame.files.gump_atlas,
        )))
    }

    pub(super) fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response {
        match input {
            Input::Press(Button::Left) if ctx.under_pointer => self.press(ctx),
            Input::Release(Button::Left) => self.release(ctx),
            // No wheel and no keys: ten rows fit on the plate, and nothing on it
            // is typed into.
            _ => Response::ignored(),
        }
    }
}

#[cfg(test)]
mod tests {
    use openshard_client_render::gump::GumpArt;
    use openshard_protocol::wire::Graphic;

    use super::*;
    use crate::panes::fixture;

    fn serial(raw: u32) -> Serial {
        Serial::new(raw).unwrap()
    }

    /// An install shipping every picture the manifest draws, each a solid block
    /// of the size the real art is — what the hit test is measured against.
    fn plate() -> fixture::Install {
        fixture::Install::shipping(party_art::art_of().map(|art| {
            let size = match art {
                GumpArt::Gump(Graphic(0x0A28 | 0x0A2A | 0x0A2E | 0x0A30)) => (44, 44),
                GumpArt::Gump(Graphic(0x0A29 | 0x0A2F)) => (427, 44),
                GumpArt::Gump(Graphic(0x0A2B | 0x0A2D)) => (44, 316),
                GumpArt::Gump(Graphic(0x0A2C)) => (427, 316),
                GumpArt::Gump(Graphic(0x0475)) => (272, 26),
                GumpArt::Gump(Graphic(0x00F2 | 0x00F3)) => (63, 23),
                _ => (30, 22),
            };
            (art, size)
        }))
    }

    /// A party of two, this client leading.
    fn led_by_me() -> WorldView {
        let mut view = fixture::world(serial(1));
        view.party.members = vec![serial(1), serial(2)];
        view
    }

    /// No party, no window — the layout says so, and `reconcile_own_windows`
    /// reads the same predicate to take the window away.
    #[test]
    fn a_client_in_no_party_has_no_manifest() {
        let view = fixture::world(serial(1));
        assert!(!in_a_party(&view));
        let install = plate();
        assert!(
            PartyPane::default()
                .layout(&install.ctx(&view, None, GumpPixel::new(0, 0), true).frame)
                .is_none()
        );
    }

    /// Kicking names the member on that row **as the roster stands at the
    /// release** — the module docs' rule, and the one thing a row-keyed hit
    /// buys.
    #[test]
    fn kicking_names_the_member_on_that_row_now() {
        let mut view = led_by_me();
        let install = plate();
        let mut pane = PartyPane::default();
        let drawn = pane
            .layout(&install.ctx(&view, None, GumpPixel::new(0, 0), true).frame)
            .expect("a party has a manifest");
        let second_row = GumpPixel::new(90, 80);

        let _down = pane.handle(
            Input::Press(Button::Left),
            &install.ctx(&view, Some(&drawn), second_row, true),
        );
        assert_eq!(pane.held, Some(Hit::Kick(1)));

        // The roster changes under the press: somebody else is on row 1 now.
        view.party.members = vec![serial(1), serial(3)];
        let release = pane.handle(
            Input::Release(Button::Left),
            &install.ctx(&view, Some(&drawn), second_row, true),
        );
        assert!(matches!(
            release.out.as_slice(),
            [Effect::Net(Outgoing::PartyRemove(member))] if *member == serial(3)
        ));
    }

    /// Leaving names this client's own body, which is what the wire's one
    /// removal packet means when it is addressed to yourself.
    #[test]
    fn leaving_names_this_client() {
        let view = led_by_me();
        let install = plate();
        let mut pane = PartyPane::default();
        let drawn = pane
            .layout(&install.ctx(&view, None, GumpPixel::new(0, 0), true).frame)
            .expect("a party has a manifest");
        let leave = GumpPixel::new(80, 370);

        let _down = pane.handle(
            Input::Press(Button::Left),
            &install.ctx(&view, Some(&drawn), leave, true),
        );
        let release = pane.handle(
            Input::Release(Button::Left),
            &install.ctx(&view, Some(&drawn), leave, true),
        );
        assert!(matches!(
            release.out.as_slice(),
            [Effect::Net(Outgoing::PartyRemove(member))] if *member == serial(1)
        ));
    }

    /// A member is not offered the leader's two controls, so a press where the
    /// Add button would be picks the window up instead.
    #[test]
    fn a_member_has_no_add_button_to_press() {
        let mut view = fixture::world(serial(2));
        view.party.members = vec![serial(1), serial(2)];
        assert!(!leading(&view));
        let install = plate();
        let mut pane = PartyPane::default();
        let drawn = pane
            .layout(&install.ctx(&view, None, GumpPixel::new(0, 0), true).frame)
            .expect("a party has a manifest");

        let press = pane.handle(
            Input::Press(Button::Left),
            &install.ctx(&view, Some(&drawn), GumpPixel::new(80, 395), true),
        );
        assert!(matches!(press.out.as_slice(), [Effect::Raise, Effect::Grab]));
        assert_eq!(pane.held, None);
    }
}
