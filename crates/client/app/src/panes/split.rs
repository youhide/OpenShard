//! The amount picker as a component: the seventh window kind, and the first one
//! this client opens *for itself* rather than because the shard or the player
//! asked for a window.
//!
//! It replaces an `egui::Window` with a `DragValue` and two buttons in
//! `shell.rs` — a panel drawn over the gump layer, in a second window system,
//! with a second opinion about where everything is. What is here instead is
//! ClassicUO's `SplitMenuGump`, made of the pictures the client ships:
//! `crate::panes::split` owns the number and the gestures, and
//! [`openshard_client_render::split`] owns where every pixel of it goes.
//!
//! # Who the number belongs to
//!
//! Not to this window. A prompt suspends exactly one
//! [`ItemPress`](crate::hand::ItemPress) — in a bag's pane, or the manager's own
//! for an item lying on the ground — and the answer has to reach *that* presser,
//! which is what [`Windows::prompt`](crate::windows::Windows::prompt) is the
//! record of. So this pane's whole output is one
//! [`Effect::Answered`](crate::panes::Effect::Answered): it says what was
//! chosen and never touches the hand, the item or the wire.
//!
//! That is also why the window closes on the way out rather than on the way in:
//! answering *is* closing, the same act a dialog's
//! [`Effect::Answer`](crate::panes::Effect::Answer) is, and taking it apart
//! would let a second answer leave a window that is already gone.
//!
//! # One number, not a number and a string
//!
//! The reference keeps a slider and a text box side by side and spends forty
//! lines keeping them in step. There is one field here — [`SplitPane::amount`] —
//! and both controls are ways of writing to it: the bar scales it, and a digit
//! typed shifts it up a decimal place. A box that held its own string would be
//! the same state in two shapes, and the shape the player is not looking at is
//! the one that ends up in the packet.

use openshard_client_render::gump::{GumpArt, GumpPixel};
use openshard_client_render::split as split_art;

use crate::panes::{Answer, Button, Effect, Input, Key, Pane, PaneCtx, PaneFrame, Response};
use crate::windows::Drawn;

/// One open amount picker: what may be taken, what is chosen, and which of its
/// two controls the mouse is on.
#[derive(Debug)]
pub struct SplitPane {
    /// The most that may be chosen — the pile less the one that stays behind,
    /// which is [`ItemPress::dragged`](crate::hand::ItemPress::dragged)'s rule
    /// and not this window's.
    ///
    /// Handed over at construction and never changed: the pile it was measured
    /// from can move while the prompt stands, and re-reading it would slide the
    /// bar under the player's finger. What guards against a stale bound is not
    /// this field but [`ItemPress::split`](crate::hand::ItemPress::split), which
    /// clamps the answer against the pile as it is when the answer arrives.
    most: u16,
    /// The number the bar and the box are both showing.
    amount: u16,
    /// The bar is being dragged: every move writes [`Self::amount`] until the
    /// button comes up.
    sliding: bool,
    /// The pointer went down on the button and has not come up.
    held: bool,
    /// The pointer is over the button, which is a third face and not a second —
    /// see [`split_art::Face`].
    over: bool,
}

impl SplitPane {
    /// A picker over a pile that can spare `most`.
    ///
    /// It opens at the top of its range, which is the reference's own default:
    /// a player who Shift-drags a pile and presses the button without touching
    /// the bar takes as much of it as the gesture allows.
    pub fn new(most: u16) -> Self {
        Self {
            most: most.max(1),
            amount: most.max(1),
            sliding: false,
            held: false,
            over: false,
        }
    }

    /// The face the button is wearing, which is where the pointer is rather
    /// than anything the layout decides.
    const fn face(&self) -> split_art::Face {
        match (self.held, self.over) {
            (true, _) => split_art::Face::Pressed,
            (false, true) => split_art::Face::Over,
            (false, false) => split_art::Face::Rest,
        }
    }

    /// Move the bar to the pointer.
    fn slide(&mut self, cursor: GumpPixel) -> Response {
        let amount = split_art::amount_at(cursor.x, self.most);
        if self.amount == amount {
            return Response::consumed();
        }
        self.amount = amount;
        Response::changed()
    }

    /// Write one number into the box, the way a box takes one: a digit shifts
    /// what is there up a place, and a rubbing-out shifts it back down.
    ///
    /// Clamped into `1..=most` at every step and not only at the answer, so the
    /// bar never shows a number the button could not send. A digit that would
    /// take it past the pile leaves it *at* the pile, which is the reference's
    /// end state by a longer road.
    fn typed(&mut self, key: Key) -> Response {
        let amount = match key {
            Key::Typed(character) => {
                let Some(digit) = character.to_digit(10) else {
                    // Not a number, so not this box's — the reference's
                    // `NumbersOnly`. It is still *taken*: a letter aimed at the
                    // prompt must not walk the body behind it.
                    return Response::consumed();
                };
                u32::from(self.amount)
                    .saturating_mul(10)
                    .saturating_add(digit)
                    .min(u32::from(self.most)) as u16
            }
            Key::Backspace => (self.amount / 10).max(1),
            // Answered by the caller: neither is a change to the number.
            Key::Done | Key::Cancel => return Response::consumed(),
        };
        let amount = amount.clamp(1, self.most);
        if self.amount == amount {
            return Response::consumed();
        }
        self.amount = amount;
        Response::changed()
    }

    /// A left press on one of the two controls, or on the frame.
    fn press(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let raised = Response::changed().with(Effect::Raise);
        match split_art::hit(ctx.frame.cursor) {
            Some(split_art::Hit::Ok) => {
                self.held = true;
                raised
            }
            Some(split_art::Hit::Slider) => {
                self.sliding = true;
                let _moved = self.slide(ctx.frame.cursor);
                raised
            }
            // Anywhere else on the picture picks the window up, which is how a
            // gump with no title bar is moved. Already window-local — see
            // `PaneFrame::cursor` — so the grab offset needs nothing subtracted.
            None => raised.with(Effect::Grab(ctx.frame.cursor)),
        }
    }

    /// The release that finishes whichever gesture was going.
    ///
    /// The button has to still be under the pointer, the same rule every other
    /// window kind's buttons follow: a press dragged off its button is a press
    /// taken back. The bar has no such rule — the reference's own `OnMouseUp`
    /// takes the pointer wherever it is, clamped to the ends — because dragging
    /// a slider off its trough is how the ends of a long one are reached at all.
    fn release(&mut self, ctx: &PaneCtx<'_>) -> Response {
        if self.sliding {
            self.sliding = false;
            let mut answer = self.slide(ctx.frame.cursor);
            answer.taken = true;
            answer.redraw = true;
            return answer;
        }
        if !self.held {
            return Response::ignored();
        }
        self.held = false;
        if split_art::hit(ctx.frame.cursor) != Some(split_art::Hit::Ok) {
            return Response::changed();
        }
        Response::changed().with(Effect::Answered(Answer::Split(self.amount)))
    }

    /// The pointer moved: the bar follows it, and the button lights or stops
    /// lighting.
    ///
    /// Neither is `taken` — a move is not an exclusive event — and the tint is
    /// gated on [`PaneCtx::under_pointer`] so that a button cannot light up
    /// through a window drawn over it, the rule `ContainerPane`'s plate follows.
    fn moved(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let over = ctx.under_pointer && split_art::hit(ctx.frame.cursor) == Some(split_art::Hit::Ok);
        let mut answer = if self.over == over {
            Response::ignored()
        } else {
            self.over = over;
            Response::stale()
        };
        if self.sliding {
            answer.redraw |= self.slide(ctx.frame.cursor).redraw;
        }
        answer
    }
}

impl Pane for SplitPane {
    /// All five pictures, named rather than left to the sweep over what was
    /// laid out — see [`split_art::ART`], which is where the reason is: the
    /// button's other two faces are drawn on the frame the pointer arrives, and
    /// an atlas grown from the last frame's layout would not hold them yet.
    fn art(&self, _frame: &PaneFrame<'_>) -> Vec<GumpArt> {
        split_art::ART.to_vec()
    }

    /// The frame, the knob where the number says, the button's current face,
    /// and the number itself.
    ///
    /// Never `None`, and it is the only kind of which that is true: every other
    /// window is drawn out of something in the view that can go away underneath
    /// it, and everything this one draws it is holding.
    fn layout(&self, _frame: &PaneFrame<'_>) -> Option<Drawn> {
        Some(Drawn::Split(split_art::window(
            self.amount,
            self.most,
            self.face(),
            // Window-local — see `PaneFrame::cursor`'s doc.
            GumpPixel::new(0, 0),
        )))
    }

    /// Both controls, and the keyboard this window holds from the moment it
    /// opens.
    ///
    /// **The wheel is this window's**, which no other kind of ours claims for a
    /// control this small: a notch over the picker steps the number by one, the
    /// reference's `HSliderBar::OnMouseWheel`, and a notch that fell through to
    /// the camera would zoom the map out from under a prompt the player is
    /// aiming at.
    fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response {
        match input {
            Input::Press(Button::Left) => {
                if !ctx.under_pointer {
                    return Response::ignored();
                }
                self.press(ctx)
            }
            Input::Release(Button::Left) => self.release(ctx),
            Input::Move => self.moved(ctx),
            Input::Wheel(notches) => {
                if !ctx.under_pointer {
                    return Response::ignored();
                }
                let amount = if notches > 0.0 {
                    self.amount.saturating_add(1)
                } else {
                    self.amount.saturating_sub(1)
                }
                .clamp(1, self.most);
                if self.amount == amount {
                    return Response::consumed();
                }
                self.amount = amount;
                Response::changed()
            }
            // Enter answers and Escape dismisses, which is why the two are
            // separate keys at all — see [`Key::Cancel`]. A dialog's field
            // cannot tell them apart and does not need to; a modal is exactly
            // the window where "done" and "never mind" are opposite answers.
            Input::Key(Key::Done) => Response::changed().with(Effect::Answered(Answer::Split(self.amount))),
            Input::Key(Key::Cancel) => Response::changed().with(Effect::Answered(Answer::Cancelled)),
            Input::Key(key) => self.typed(key),
            // The right button is the manager's close, and closing this window
            // *is* dismissing the prompt — see `App::close_window`, which is
            // the one door both gestures go through.
            //
            // An answer is never this window's: it is addressed to whoever is
            // holding the press this prompt went up over, and that is never the
            // prompt itself.
            Input::Press(Button::Right) | Input::Release(Button::Right) | Input::Answered(_) => {
                Response::ignored()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The picker opens on the whole of what the gesture allows, which is what
    /// pressing the button without touching anything means.
    #[test]
    fn it_opens_at_the_top_of_its_range() {
        let pane = SplitPane::new(40);
        assert_eq!(pane.amount, 40);
        assert_eq!(pane.most, 40);
    }

    /// Typing is one field's arithmetic and not a second string: a digit shifts
    /// the number up a place, backspace shifts it down, and neither can leave
    /// the range the bar is drawn against.
    #[test]
    fn the_box_types_into_the_same_number_the_bar_moves() {
        let mut pane = SplitPane::new(500);
        pane.amount = 1;
        let _ = pane.typed(Key::Typed('2'));
        assert_eq!(pane.amount, 12);
        let _ = pane.typed(Key::Typed('3'));
        assert_eq!(pane.amount, 123);
        let _ = pane.typed(Key::Backspace);
        assert_eq!(pane.amount, 12);
        let _ = pane.typed(Key::Typed('9'));
        assert_eq!(pane.amount, 129);
        let _ = pane.typed(Key::Typed('9'));
        assert_eq!(pane.amount, 500, "a digit past the pile stops at the pile");
        let _ = pane.typed(Key::Typed('x'));
        assert_eq!(pane.amount, 500, "and a letter is not a digit");
    }

    /// Rubbing out the last figure never empties the box: one is the smallest
    /// split there is, so the box's floor is the same as the bar's.
    #[test]
    fn the_box_never_empties() {
        let mut pane = SplitPane::new(50);
        pane.amount = 7;
        let _ = pane.typed(Key::Backspace);
        assert_eq!(pane.amount, 1);
        let _ = pane.typed(Key::Backspace);
        assert_eq!(pane.amount, 1);
    }

    /// A letter aimed at the prompt is *taken* even though it changes nothing:
    /// a key that fell through would walk the body behind the window.
    #[test]
    fn a_letter_is_swallowed_rather_than_walked() {
        let mut pane = SplitPane::new(50);
        assert!(pane.typed(Key::Typed('w')).taken);
        assert!(!pane.typed(Key::Typed('w')).redraw);
    }

    /// The bar writes the same field the box does, clamped to the same ends.
    #[test]
    fn the_bar_writes_the_number() {
        let mut pane = SplitPane::new(100);
        let _ = pane.slide(GumpPixel::new(29, 20));
        assert_eq!(pane.amount, 1, "the near end is one");
        let _ = pane.slide(GumpPixel::new(1_000, 20));
        assert_eq!(pane.amount, 100, "and the far end is the whole of it");
    }

    /// The button wears three faces and the pointer decides which.
    #[test]
    fn the_button_shows_where_the_pointer_is() {
        let mut pane = SplitPane::new(10);
        assert_eq!(pane.face(), split_art::Face::Rest);
        pane.over = true;
        assert_eq!(pane.face(), split_art::Face::Over);
        pane.held = true;
        assert_eq!(pane.face(), split_art::Face::Pressed);
    }

    /// The gestures, through [`Pane::handle`] and against an install that ships
    /// the five pictures — the shape step 8 of `docs/window_components.md` made
    /// possible, and the only way to assert what the *manager* is asked for.
    mod gestures {
        use openshard_client_render::gump::GumpArt;
        use openshard_protocol::serial::Serial;
        use openshard_protocol::wire::Graphic;

        use super::*;
        use crate::panes::fixture::Install;

        /// The five blocks the picker draws, at the reference's own sizes.
        fn install() -> Install {
            Install::shipping([
                (GumpArt::Gump(Graphic(0x085C)), (164, 74)),
                (GumpArt::Gump(Graphic(0x0845)), (15, 15)),
                (GumpArt::Gump(Graphic(0x085D)), (46, 21)),
                (GumpArt::Gump(Graphic(0x085E)), (46, 21)),
                (GumpArt::Gump(Graphic(0x085F)), (46, 21)),
            ])
        }

        /// Somewhere on the button, and somewhere on the bar.
        const ON_BUTTON: GumpPixel = GumpPixel::new(110, 45);
        const ON_BAR_MIDDLE: GumpPixel = GumpPixel::new(29 + 45, 20);

        fn world() -> openshard_client_net::view::WorldView {
            crate::panes::fixture::world(Serial::new(0x0000_0001).expect("a serial"))
        }

        /// Pressing the button and letting go on it is the answer, and the
        /// window says so with one effect rather than with a packet: it does
        /// not know whose press it is standing over.
        #[test]
        fn the_button_answers_on_the_way_up() {
            let install = install();
            let view = world();
            let mut pane = SplitPane::new(40);

            let pressed = pane.handle(
                Input::Press(Button::Left),
                &install.ctx(&view, None, ON_BUTTON, true),
            );
            assert!(pressed.taken, "the press is the window's");
            assert!(
                matches!(pressed.out.as_slice(), [Effect::Raise]),
                "and nothing is answered yet — the button is only down"
            );
            assert_eq!(pane.face(), split_art::Face::Pressed);

            let released = pane.handle(
                Input::Release(Button::Left),
                &install.ctx(&view, None, ON_BUTTON, true),
            );
            assert!(matches!(
                released.out.as_slice(),
                [Effect::Answered(Answer::Split(40))]
            ));
        }

        /// A press dragged off the button is a press taken back — every other
        /// window kind's rule, and this one's.
        #[test]
        fn a_press_dragged_off_the_button_answers_nothing() {
            let install = install();
            let view = world();
            let mut pane = SplitPane::new(40);
            let _ = pane.handle(
                Input::Press(Button::Left),
                &install.ctx(&view, None, ON_BUTTON, true),
            );
            let released = pane.handle(
                Input::Release(Button::Left),
                &install.ctx(&view, None, GumpPixel::new(5, 5), true),
            );
            assert!(
                released.out.is_empty(),
                "the button came back up and nothing else"
            );
            assert_eq!(pane.face(), split_art::Face::Rest);
        }

        /// The bar is written on the way *down* and follows the pointer while
        /// it is held — including off the end of its own trough, which is how
        /// the top of a long pile is reached without pixel-hunting.
        #[test]
        fn the_bar_follows_the_pointer_while_it_is_held() {
            let install = install();
            let view = world();
            let mut pane = SplitPane::new(91);

            let pressed = pane.handle(
                Input::Press(Button::Left),
                &install.ctx(&view, None, ON_BAR_MIDDLE, true),
            );
            assert!(pressed.taken);
            assert_eq!(pane.amount, 46, "half a bar of ninety-one");

            let _ = pane.handle(
                Input::Move,
                &install.ctx(&view, None, GumpPixel::new(1_000, 20), true),
            );
            assert_eq!(pane.amount, 91);
            let _ = pane.handle(
                Input::Release(Button::Left),
                &install.ctx(&view, None, GumpPixel::new(1_000, 20), true),
            );
            assert_eq!(pane.amount, 91);

            // Let go, and the pointer no longer writes anything.
            let _ = pane.handle(
                Input::Move,
                &install.ctx(&view, None, GumpPixel::new(29, 20), true),
            );
            assert_eq!(pane.amount, 91, "the bar is not being dragged any more");
        }

        /// Enter answers with the number and Escape dismisses the press behind
        /// it — the two keys a `{ textentry }` cannot tell apart, and the whole
        /// reason [`Key::Cancel`] exists.
        #[test]
        fn enter_answers_and_escape_dismisses() {
            let install = install();
            let view = world();
            let mut pane = SplitPane::new(12);

            let done = pane.handle(Input::Key(Key::Done), &install.ctx(&view, None, ON_BUTTON, true));
            assert!(matches!(
                done.out.as_slice(),
                [Effect::Answered(Answer::Split(12))]
            ));

            let cancelled = pane.handle(
                Input::Key(Key::Cancel),
                &install.ctx(&view, None, ON_BUTTON, true),
            );
            assert!(matches!(
                cancelled.out.as_slice(),
                [Effect::Answered(Answer::Cancelled)]
            ));
        }

        /// A notch steps the number by one and **is taken**: a wheel over the
        /// picker must not reach the camera behind it. At the end of the range
        /// it is still taken and asks for no frame — decision 4's two questions,
        /// answered separately.
        #[test]
        fn a_notch_steps_the_number_and_never_reaches_the_camera() {
            let install = install();
            let view = world();
            let mut pane = SplitPane::new(3);
            pane.amount = 2;

            let up = pane.handle(Input::Wheel(1.0), &install.ctx(&view, None, ON_BUTTON, true));
            assert!(up.taken && up.redraw);
            assert_eq!(pane.amount, 3);

            let past = pane.handle(Input::Wheel(1.0), &install.ctx(&view, None, ON_BUTTON, true));
            assert!(past.taken, "still the window's");
            assert!(!past.redraw, "and nothing moved");

            let down = pane.handle(Input::Wheel(-1.0), &install.ctx(&view, None, ON_BUTTON, true));
            assert!(down.taken);
            assert_eq!(pane.amount, 2);

            let elsewhere = pane.handle(Input::Wheel(1.0), &install.ctx(&view, None, ON_BUTTON, false));
            assert!(!elsewhere.taken, "a notch over another window is not this one's");
        }

        /// **Every face is asked for on every frame**, including the two the
        /// button is not wearing — see [`split_art::ART`]. A pane that named
        /// only what it draws would pack the hover face on the frame the
        /// pointer arrives, which is one frame after it is needed.
        #[test]
        fn every_face_is_asked_for_before_any_of_them_is_drawn() {
            let install = install();
            let view = world();
            let pane = SplitPane::new(5);
            let ctx = install.ctx(&view, None, ON_BUTTON, true);
            assert_eq!(pane.art(&ctx.frame), split_art::ART.to_vec());
            let Some(Drawn::Split(drawn)) = pane.layout(&ctx.frame) else {
                panic!("the picker always has a layout");
            };
            assert!(
                drawn
                    .pictures
                    .iter()
                    .all(|picture| pane.art(&ctx.frame).contains(&picture.graphic)),
                "and nothing is drawn that was not asked for"
            );
        }
    }
}
