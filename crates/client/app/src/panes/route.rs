//! The one place an input is offered to the client's own windows, and the one
//! place a pane's answer is carried out.
//!
//! This is the plan's `Panes::deliver`. It replaces the `||` chains in
//! `event_loop.rs`, whose single `bool` answered "this event is mine" and "ask
//! for a redraw" at the same time — see [`Response`] for the defect that cost.
//!
//! Three steps, in this order, and the order is the whole design:
//!
//! 1. **The manager's own gestures.** Moving a window and letting go of it
//!    belong to nobody's pane (decision 2). They run whether a pane takes the
//!    event or not.
//! 2. **The panes, top down, first `taken` wins.** Painter's order is z-order,
//!    so the last window in the list is the one drawn over the others and the
//!    first one offered an event.
//! 3. **[`App::fallback_gestures`], the manager's own gestures that are a
//!    *fallback* rather than a precondition.** Reached only when no pane
//!    answered — so it is never a second opinion about the same click. No
//!    window kind is answered here at all (step 6 took the last): what is left
//!    is the press that picks a window up, and the world's own press and drop,
//!    which no window can answer for because the ground is not a window.
//!
//! [`App::window_under_pointer`] is asked once, here, and handed down as
//! `owner` rather than asked again by steps 1 and 3 — the walk is a texel pick
//! against every window's last frame, and three of those per press was step 7's
//! own Backlog entry.

use std::time::Instant;

use openshard_client_render::gump::GumpPixel;

use crate::app::App;
use crate::hand::Hand;
use crate::panes::{Button, Effect, Input, Modifiers, Pane, PaneCtx, PaneFiles, PaneFrame, Response};
use crate::windows::{Asking, WindowSubject};

impl App {
    /// Offer one input to the window layer, and carry out whatever it asked
    /// for.
    ///
    /// The answer's [`out`](Response::out) is always empty: the effects a pane
    /// asked for have already been performed by the time this returns, and what
    /// is left is the two questions the caller has to act on — whether the
    /// event was the window layer's ([`taken`](Response::taken)), and whether
    /// the frame has to be drawn again ([`redraw`](Response::redraw)).
    ///
    /// The caller acts on `taken` by *not* doing whatever it would have done
    /// with the event: no map zoom under a pointer that is over a shop, no tile
    /// selected behind a bag that was only being raised.
    pub(crate) fn deliver(&mut self, input: Input) -> Response {
        // Asked once for the whole of `deliver`: a texel pick against every
        // window's last frame, wanted by all three rungs below and, before this,
        // recomputed by each of them that needed it.
        let owner = self.window_under_pointer();
        let mut response = self.manager_gestures(input, owner);
        if !response.taken {
            response.absorb(self.offer_to_panes(input, owner));
        }
        if !response.taken {
            response.absorb(self.fallback_gestures(input, owner));
        }
        response
    }

    /// The gestures that are the manager's and no pane's: the window being
    /// dragged follows the pointer, letting the button go ends that, and a
    /// press while the hand is full does nothing at all.
    ///
    /// Only the last of the three is `taken`. A move is not an exclusive event
    /// (see [`Input::Move`]), and the release that let go of a frame is also the
    /// release that drops a held item into the bag under it — swallowing it here
    /// would strand the hand.
    fn manager_gestures(&mut self, input: Input, owner: Option<WindowSubject>) -> Response {
        match input {
            Input::Move => {
                if self.drag_own_window() {
                    Response::stale()
                } else {
                    Response::ignored()
                }
            }
            Input::Release(Button::Left) => {
                self.windows.dragging = None;
                Response::ignored()
            }
            // A press that landed on no window at all gives the keyboard back.
            // The manager's because the keyboard is (see
            // [`Effect::TakeKeyboard`]), and here rather than in a pane because
            // no pane is offered a press that is not on it — a window cannot
            // notice the player clicking somewhere else. Not `taken`: the world
            // still gets the click that put the field down.
            Input::Press(Button::Left) if self.windows.keyboard.is_some() && owner.is_none() => {
                self.windows.keyboard = None;
                Response::stale()
            }
            // **Decision 7's precondition, and the manager's because the hand
            // is.** Once a lift has gone to the shard the hand *is* the cursor:
            // a second press is choosing a destination for the item already on
            // it, and it must not reach a window, which would answer it by
            // starting a second source. Ahead of the panes rather than inside
            // each of them, because a pane that forgot to ask is a pane that
            // quietly overwrites the hand.
            //
            // It is the field being `Some` now rather than a method on it: the
            // state that had sent nothing left this enum for the panes at step
            // 6, so everything that is here owns the cursor by being here.
            Input::Press(Button::Left) if self.windows.hand.is_some() => {
                self.windows.dragging = None;
                Response::changed()
            }
            // The answer to the client's own amount prompt, when the press it
            // suspended is the *world's* — see `Asking::World`. Every other
            // presser is a window, and the walk below delivers it there.
            Input::Answered(answer) if self.windows.prompt == Some(Asking::World) => {
                self.windows.prompt = None;
                self.split_world_press(answer);
                Response::changed()
            }
            // A keystroke is never the manager's: it is addressed to a window by
            // name, and the manager's part of that is having said which window —
            // see `App::keyboard_window`. A modal's answer is the same, one
            // device over, and the arm above is the one presser that has no
            // window to be addressed by.
            Input::Press(_)
            | Input::Release(Button::Right)
            | Input::Wheel(_)
            | Input::Key(_)
            | Input::Answered(_) => Response::ignored(),
        }
    }

    /// Which of this client's windows the keys go to, or `None` for the world.
    ///
    /// The field is the answer, filtered by whether that window is still open:
    /// a dialog answered by its own button leaves the list before the next
    /// frame's `sync_own_windows` can tidy up after it, and a keystroke in
    /// between must not be swallowed by a window that is no longer there. Both
    /// callers ask this rather than the field — the router below, and
    /// `event_loop`'s question of whether a letter is a letter typed or a step
    /// walked — and `sync_own_windows` writes it back through the same
    /// predicate, so there is one rule and not three.
    pub(crate) fn keyboard_window(&self) -> Option<WindowSubject> {
        let subject = self.windows.keyboard?;
        self.windows
            .own_windows
            .iter()
            .any(|window| window.subject == subject)
            .then_some(subject)
    }

    /// Offer the input to every pane from the top down, and perform what the
    /// answers asked for.
    ///
    /// A pane that declines is walked past, and its `redraw` is still kept: a
    /// window can go stale without the event being its — a hover tint leaving
    /// one as the pointer crosses onto the window above it, say. The walk stops
    /// at the first pane that says `taken`, which is what "under this one" means
    /// in [`Response::taken`]'s own doc.
    ///
    /// # A located input stops at the window it is on
    ///
    /// The walk also stops *after* the window the pointer is over, for the two
    /// inputs that are somewhere: a press and a notch. Nothing below the window
    /// under the pointer may answer either — the rule `owner` states once here
    /// rather than a rule six panes would each have had to ask
    /// `window_under_pointer` to remember for themselves.
    ///
    /// A release and a move are not bounded. A release finishes a press
    /// wherever the pointer has got to since — a paperdoll's button has to come
    /// back up even if the finger slid off the window — and a move is offered to
    /// every window so that a tint left behind can be cleared.
    ///
    /// # A keystroke is offered to one window by name
    ///
    /// [`Input::Key`] is not located at all: it goes to the window that holds
    /// the keyboard and to nothing else, wherever that window is in the pile and
    /// wherever the pointer is. A player typing into a `{ textentry }` can raise
    /// a bag over it, and the letters must not follow the click. The walk below
    /// is still the walk — one place builds a context and one place performs
    /// what came back — with everything but the addressee skipped.
    fn offer_to_panes(&mut self, input: Input, owner: Option<WindowSubject>) -> Response {
        let located = matches!(input, Input::Press(_) | Input::Wheel(_));
        // Asked here for the same reason `owner` is: both are the manager's
        // answer about a window rather than a pane's about itself. `None` for a
        // keystroke with nobody to take it means the walk offers it to no
        // window at all.
        let keyboard = self.keyboard_window();
        // Which window is holding the press the amount prompt went up over, if
        // one is: the same shape as the keyboard, and the same reason — one
        // device on the screen, so the manager says whose it is.
        let prompt = match self.windows.prompt {
            Some(Asking::Window(subject)) => Some(subject),
            Some(Asking::World) | None => None,
        };
        // **The two inputs that are not located are addressed instead**, each
        // by the manager's own record of who owns the device: a keystroke by
        // `Windows::keyboard`, a modal's answer by `Windows::prompt`. Neither
        // follows the pointer, because a player may raise a bag over the window
        // that is typing or waiting.
        let addressed = match input {
            Input::Key(_) => match keyboard {
                Some(subject) => Some(subject),
                None => return Response::ignored(),
            },
            Input::Answered(_) => match prompt {
                Some(subject) => Some(subject),
                // The world's own press, or none at all: `manager_gestures`
                // above has already answered whichever it is.
                None => return Response::ignored(),
            },
            _ => None,
        };
        // No world, no windows: `sync_own_windows` has already emptied the list,
        // and there is no authoritative picture to build a context out of.
        //
        // Field borrows rather than method calls from here down, so that the
        // context can hold the view, the files and the last frame's layouts
        // while the pane list is borrowed mutably beside them. They are
        // disjoint fields of `App`, which is what makes a readonly context and
        // a `&mut` pane possible in one loop at all.
        let Some(view) = self.world.authoritative.view.as_ref() else {
            return Response::ignored();
        };
        let files = PaneFiles::of(&self.resources);
        let drawn_windows = &self.windows.drawn_windows;
        // The pointer, in absolute gump pixels — turned into each window's
        // own local cursor inside the loop below, the same arithmetic
        // `render_passes.rs` does when it builds a `PaneFrame` for `art` and
        // `layout`. See `PaneFrame::cursor`'s doc.
        let cursor = self.input.pointer_gump;
        let modifiers = Modifiers {
            shift: self.input.shift_held,
            ctrl: self.input.ctrl_held,
        };
        let now = Instant::now();
        let hand = self.windows.hand;

        let mut response = Response::ignored();
        // Collected rather than performed inside the loop: an effect needs
        // `&mut self`, and the loop above holds half of `self` borrowed.
        let mut asked: Vec<(WindowSubject, Effect)> = Vec::new();
        for open in self.windows.own_windows.iter_mut().rev() {
            if addressed.is_some_and(|subject| subject != open.subject) {
                continue;
            }
            let under_pointer = owner == Some(open.subject);
            let local_cursor = GumpPixel::new(cursor.x - open.at.x, cursor.y - open.at.y);
            let ctx = PaneCtx {
                frame: PaneFrame {
                    view,
                    files,
                    cursor: local_cursor,
                    hand,
                    has_keyboard: keyboard == Some(open.subject),
                    has_prompt: prompt == Some(open.subject),
                },
                drawn: drawn_windows
                    .iter()
                    .find(|(subject, _)| *subject == open.subject)
                    .map(|(_, drawn)| drawn),
                under_pointer,
                modifiers,
                now,
            };
            let answer = open.pane.handle(input, &ctx);
            let taken = answer.taken;
            let subject = open.subject;
            response.taken |= taken;
            response.redraw |= answer.redraw;
            asked.extend(answer.out.into_iter().map(|effect| (subject, effect)));
            if taken || (located && under_pointer) {
                break;
            }
        }
        for (subject, effect) in asked {
            self.perform(subject, effect);
        }
        response
    }

    /// Carry out one thing a pane asked for, on the window it asked from.
    ///
    /// The manager's half of decision 5: a pane names what it wants and this is
    /// the only code that does it, which is why no pane holds a
    /// [`Link`](crate::link::Link) or writes its own position.
    fn perform(&mut self, subject: WindowSubject, effect: Effect) {
        match effect {
            Effect::Raise => self.raise_window(subject),
            Effect::Close => {
                self.close_window(subject);
            }
            Effect::Grab(grab) => self.windows.dragging = Some((subject, grab)),
            // A closed channel is not an error here, the same as everywhere else
            // this client sends: the shard thread has already said why it went.
            Effect::Net(action) => {
                if let Some(link) = self.world.shard.link() {
                    link.act(action);
                }
            }
            // One arm for both local kinds, and idempotent — which is the point:
            // pressing Skills twice must not scroll the sheet back to the top.
            // See `open_local_window`, which leaves a window it finds alone.
            Effect::Open(local) => {
                crate::windows::open_local_window(&mut self.windows.own_windows, local.subject());
            }
            // The wire's half and this end's, which are one act — see
            // [`Effect::Answer`]. The `retain` and not `close_window`: that door
            // asks a dialog for its *dismissal* answer, and a window that has
            // just answered with the button the player pressed must not send
            // button zero after it.
            Effect::Answer(reply) => {
                self.answer_gump(reply);
                self.windows
                    .own_windows
                    .retain(|window| window.subject != subject);
            }
            Effect::TakeKeyboard => self.windows.keyboard = Some(subject),
            Effect::ReleaseKeyboard => self.windows.keyboard = None,
            // **The wire and the mirror, in one act** — see [`Effect::Lift`],
            // and the shard's half goes first: a hand filled with nothing on
            // the wire behind it is an item this end has taken out of a bag
            // nobody else knows about. The local projection follows
            // immediately rather than on the next packet, so the icon leaves
            // the bag on the frame the player dragged it out of.
            Effect::Lift(drag) => self.lift(drag),
            // The other half, and the one place that knows *what* is being put
            // down. A pane names only the destination, so a drop asked for
            // while the last one is still in flight is refused here — the pane
            // cannot tell a hand that is holding from one that is waiting, and
            // does not have to.
            Effect::Drop(destination) => {
                let Some(Hand::Held(drag)) = self.windows.hand else {
                    return;
                };
                if let Some(link) = self.world.shard.link() {
                    link.act(destination.packet(drag.item.serial));
                    self.windows.hand = Some(Hand::Dropped { drag, destination });
                    self.reproject_item_drag();
                }
            }
            // Up it goes, and the manager remembers whose press it is standing
            // over — which is what the answer is later routed by.
            Effect::Prompt(prompt) => self.open_split_prompt(Asking::Window(subject), prompt),
            // The other end of it, asked for by the *picker's* window rather
            // than by the presser: the number goes to whoever `Windows::prompt`
            // says is waiting, and the picker goes off the list. See
            // `App::answer_prompt`, which is the one door — the right button
            // over the picker and Escape on it arrive there through
            // `App::close_window` instead.
            Effect::Answered(answer) => self.answer_prompt(answer),
            Effect::StackAll => {
                if let WindowSubject::Container(container) = subject {
                    self.start_stack_pass(container);
                }
            }
        }
    }

    /// The third rung: the manager's own gestures that are a *fallback* rather
    /// than a precondition, reached only when no pane has taken the input.
    ///
    /// Nothing here is a window kind — step 6 took the last of those out — so
    /// this is not a `match` over what was clicked, the way it used to be. What
    /// is left is the manager's own leftovers, and they have to run **behind**
    /// the panes rather than beside `manager_gestures`:
    ///
    /// - the press that picks a window up, which every pane that declines a
    ///   press falls through to (a status frame is dragged this way);
    /// - the world's own item press, and the drop of a held item onto the
    ///   ground — the one destination that is not a window, so no pane can
    ///   answer for it.
    ///
    /// Both are named for what they are rather than for what they used to be
    /// beside — the plan's step 7, finished.
    fn fallback_gestures(&mut self, input: Input, owner: Option<WindowSubject>) -> Response {
        match input {
            Input::Press(Button::Left) => {
                if self.press_on_own_window(owner) {
                    Response::changed()
                } else {
                    Response::ignored()
                }
            }
            // Two questions on the way up, in this order: a held item that no
            // window claimed goes on the ground, and the world's own press,
            // which never became a drag, is dropped. Both used to be asked of
            // every window kind here; a bag answers its own drop in
            // `panes::container` now, and a doll its own equip.
            Input::Release(Button::Left) => {
                if self.drop_hand_on_ground() || self.release_world_press() {
                    Response::changed()
                } else {
                    Response::ignored()
                }
            }
            // The right button over a window closes it — the reference client's
            // own gesture. `cancel_target_cursor` is asked before this, in the
            // event loop: a targeting cursor is put out before a window is taken
            // down under it.
            Input::Press(Button::Right) => {
                if self.close_window_under_pointer(owner) {
                    Response::changed()
                } else {
                    Response::ignored()
                }
            }
            Input::Release(Button::Right) => Response::ignored(),
            // The world's press becoming a lift, and nothing else: the bag's
            // hover that used to stand beside it is `ContainerPane`'s, its
            // press is too, and the skill sheet's thumb and the doll's tint
            // went at steps 2 and 5. What is left is the one press no window
            // holds.
            Input::Move => {
                if self.drag_world_item() {
                    Response::stale()
                } else {
                    Response::ignored()
                }
            }
            // **Empty, and that is the plan's own milestone.** This arm was
            // `scroll_skills() || scroll_vendor() || zoom()` — the `||` chain
            // the whole of `docs/window_components.md` was written about, whose
            // one `bool` answered "the notch was taken" and "the list moved" at
            // the same time. Both windows own their wheel now, each answering
            // the two questions as two fields, and the zoom is the caller's
            // business, reached only when nothing above said the notch was its.
            // No kind that is left has a wheel at all.
            Input::Wheel(_) => Response::ignored(),
            // **Empty for the arm below's reason.** A modal's answer reaches
            // the presser the manager named, which is either a window's pane or
            // `manager_gestures`' own arm for the world's press.
            Input::Answered(_) => Response::ignored(),
            // **Empty from the day it was written.** A keystroke is only ever
            // offered to the window the manager says holds the keyboard, and the
            // one kind that can hold it is a pane already.
            Input::Key(_) => Response::ignored(),
        }
    }
}
