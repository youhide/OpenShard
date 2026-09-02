//! The window *manager*: which of this client's own windows exist, which one
//! the pointer is on, which one a gesture takes down — kept apart from
//! `ui_command.rs`'s walk and targeting even though both answer to the same
//! click, because a press on a window and a press on the ground are different
//! subsystems that happen to share an input device.
//!
//! [`App::sync_own_windows`] is the once-a-frame fold from the
//! [`WorldView`](openshard_client_net::view::WorldView) the shard has sent;
//! [`App::window_under_pointer`] answers every located input against what that
//! fold last laid out — see [`windows::Windows::drawn_windows`] for why the
//! picture a click is tested against is the *last frame's*.
//!
//! # What is not here any more, and the three things that are
//!
//! This file used to answer every window's input. `docs/client/design_panes.md`
//! moved all six kinds into [`crate::panes`], one step at a time, and what is
//! left falls into three groups:
//!
//! - **The manager's own**: which windows exist, the z-order, the raise, the
//!   press that picks a window up when no pane wanted it, and the two closes.
//! - **The world's, which no pane can answer for**: the press on an item lying
//!   on the ground ([`App::press_world_item`]), what it becomes
//!   ([`App::drag_world_item`]), and the drop of a held item onto a tile
//!   ([`App::drop_hand_on_ground`]). The ground is not a window.
//! - **The machines that run across frames**: the stack pass, which sends one
//!   merge per authoritative snapshot and cannot live in a pane that only ever
//!   sees one input.

use std::time::Instant;

use openshard_client_net::view::WorldView;
use openshard_client_render::gump as gump_art;
use openshard_protocol::containers::ContainedItem;
use openshard_protocol::gump::{
    GumpId,
    GumpPoint,
};
use openshard_protocol::serial::Serial;
use openshard_protocol::speech::TalkMode;
use openshard_protocol::wire::{
    Graphic,
    MultiId,
};
use openshard_protocol::world::Point;

use crate::app::App;
use crate::windows::{
    self,
    Drawn,
    WindowSubject,
};
use crate::{
    chat,
    hand,
    link,
};

mod sync;

const MAX_STACK: u16 = 60_000;
const GOLD_GRAPHIC: Graphic = Graphic(0x0EED);
const ARROW_GRAPHIC: Graphic = Graphic(0x0F3F);
const BOLT_GRAPHIC: Graphic = Graphic(0x1BFB);
/// The permanent plaque created beside a house.  It is an ordinary world item
/// on the wire, but it is house infrastructure rather than something a player
/// can put in a pack.
const HOUSE_SIGN_GRAPHIC: Graphic = Graphic(0x0BD2);
/// ClassicUO's `Constants.DRAG_ITEMS_DISTANCE`.
const DRAG_ITEMS_DISTANCE: u16 = 3;

/// The tile a visible drop target ultimately belongs to. A nested pouch has no
/// tile of its own, so walk its open-container parent chain until reaching the
/// ground item or the body wearing its outermost pack.
pub(crate) fn drop_target_position(view: &WorldView, destination: hand::PendingDrop) -> Option<Point> {
    fn target_position(view: &WorldView, serial: Serial, remaining: u8) -> Option<Point> {
        if remaining == 0 {
            return None;
        }
        if serial == view.player.serial || view.player.equipment.iter().any(|item| item.serial == serial) {
            return Some(view.player.position);
        }
        if let Some(mobile) = view.mobiles.get(&serial) {
            return Some(mobile.position);
        }
        if let Some(item) = view.items.get(&serial) {
            return Some(item.position);
        }
        if let Some(mobile) = view
            .mobiles
            .values()
            .find(|mobile| mobile.equipment.iter().any(|item| item.serial == serial))
        {
            return Some(mobile.position);
        }
        let (&parent, _) = view
            .contents
            .iter()
            .find(|(_, contents)| contents.iter().any(|item| item.serial == serial))?;
        target_position(view, parent, remaining - 1)
    }

    match destination {
        hand::PendingDrop::Ground(at) => Some(at),
        hand::PendingDrop::Container { container, .. } => target_position(view, container, 16),
        hand::PendingDrop::Item { target } => target_position(view, target, 16),
        hand::PendingDrop::Equipment { mobile, .. } => target_position(view, mobile, 1),
    }
}

pub(crate) fn in_drag_range(from: Point, to: Point) -> bool {
    from.x.abs_diff(to.x) <= DRAG_ITEMS_DISTANCE && from.y.abs_diff(to.y) <= DRAG_ITEMS_DISTANCE
}

/// Whether this world graphic can start the client's item-drag gesture.
///
/// A multi is one item on the wire but many pieces on screen.  Starting a lift
/// for it subtracts that one item from the local projection, making a whole
/// house or ship flash out of existence until the shard rejects the drag.
/// House signs are likewise derived, fixed world objects.  The shard makes the
/// authoritative component-based decision too; this guard prevents the
/// misleading local cursor and flicker for the two representations the client
/// can identify from the wire alone.
fn movable_world_graphic(graphic: Graphic) -> bool {
    graphic.0 & MultiId::FLAG == 0 && graphic != HOUSE_SIGN_GRAPHIC
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct StackStep {
    source: Serial,
    target: Serial,
    amount: u16,
}

/// Pick one safe merge for this pass. Whole donor piles are preferred when
/// they fit; a partial donor is used only to finish a target at 60,000. That
/// keeps serial churn low, while recomputing after every server refresh makes
/// split remainders available to the following pass.
fn next_stack_step(items: &[ContainedItem]) -> Option<StackStep> {
    for (target_index, target) in items.iter().enumerate() {
        let room = MAX_STACK.saturating_sub(target.amount.0);
        if room == 0 {
            continue;
        }
        let matches = |item: &&ContainedItem| {
            item.graphic == target.graphic
                && item.hue == target.hue
                // The container wire format does not carry `Stackable`.  A
                // single coin, arrow or bolt has amount one, so recognise the
                // three intrinsic stack arts as well as already-visible piles.
                && (intrinsically_stackable(target.graphic) || target.amount.0 > 1 || item.amount.0 > 1)
        };
        let donors: Vec<_> = items[target_index + 1..].iter().filter(matches).collect();
        let Some(source) = donors
            .iter()
            .rev()
            .find(|item| item.amount.0 <= room)
            .copied()
            .or_else(|| donors.last().copied())
        else {
            continue;
        };
        return Some(StackStep {
            source: source.serial,
            target: target.serial,
            amount: room.min(source.amount.0),
        });
    }
    None
}

const fn intrinsically_stackable(graphic: Graphic) -> bool {
    matches!(graphic, GOLD_GRAPHIC | ARROW_GRAPHIC | BOLT_GRAPHIC)
}

impl App {
    // No `stack_all_button_under_pointer`, no `take_all_button_under_pointer`
    // and no `take_all_from_container`: both plates are a bag's own furniture
    // and both are `panes::container::ContainerPane`'s now — the hit test, the
    // caption drawn under the window, and the sweep the one of them performs.
    //
    // The walk they shared is what the plan's Backlog entry was about: each
    // asked `window_under_pointer()` again *inside* its own loop over every
    // window, so the answer depended on a second top-down walk taken per
    // iteration. What that predicate was reaching for is the router's own rule
    // — a press stops at the window it landed on — so a pane hit-tests itself
    // and the walk is gone rather than restated.

    /// Begin compacting the like piles in one container.
    ///
    /// Asked for by [`Effect::StackAll`](crate::panes::Effect::StackAll), and
    /// the manager's rather than the pane's because it is a machine that runs
    /// **across frames**: one merge is sent, the container is asked for again,
    /// and the next merge is planned against the answer. A pane only ever sees
    /// one input.
    pub(crate) fn start_stack_pass(&mut self, container: Serial) {
        if self.windows.hand.is_some() {
            return;
        }
        self.windows.stack_pass = Some(crate::windows::StackPass {
            container,
            awaiting: None,
        });
        self.advance_stack_pass();
    }

    /// Send one merge from the latest authoritative container snapshot.
    /// A refresh follows every drag because a successful merge consumes the
    /// source serial without echoing a Remove packet to the client that lifted
    /// it. The next pass therefore never plans against a stale split remainder.
    pub(crate) fn advance_stack_pass(&mut self) {
        const RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
        let Some(pass) = self.windows.stack_pass.as_ref() else {
            return;
        };
        if self.windows.hand.is_some() {
            self.windows.stack_pass = None;
            return;
        }
        let container_serial = pass.container;
        let Some(items) = self
            .world
            .authoritative
            .view
            .as_ref()
            .and_then(|view| view.contents.get(&container_serial))
        else {
            self.windows.stack_pass = None;
            return;
        };
        if let Some((source, sent_at)) = &pass.awaiting {
            if items.iter().any(|item| item.serial == *source) {
                if sent_at.elapsed() >= RESPONSE_TIMEOUT {
                    self.windows.stack_pass = None;
                }
                return;
            }
        }
        let Some(step) = next_stack_step(items) else {
            self.windows.stack_pass = None;
            return;
        };
        let Some(link) = self.world.shard.link() else {
            self.windows.stack_pass = None;
            return;
        };
        link.pick_up_item(step.source, openshard_protocol::items::ItemAmount(step.amount));
        link.drop_onto_item(step.source, step.target);
        // Reopening requests a complete 0x3C list and repairs the intentionally
        // suppressed Remove-to-lifter packet before the following pass.
        link.use_object(container_serial);
        if let Some(pass) = self.windows.stack_pass.as_mut() {
            pass.awaiting = Some((step.source, Instant::now()));
        }
    }

    /// Remember a world item's press so a following pointer move can lift it.
    /// A plain click remains available to the normal selection/double-click
    /// use path in the event loop.
    pub(crate) fn press_world_item(&mut self) -> bool {
        let Some(hovered) = self.picking.hover.item else {
            return false;
        };
        let serial = hovered.serial;
        let Some(item) = self
            .world
            .authoritative
            .view
            .as_ref()
            .and_then(|view| view.items.get(&serial))
        else {
            return false;
        };
        let openshard_protocol::items::WorldItemPayload::Stack(amount) = item.payload else {
            return false;
        };
        if !movable_world_graphic(item.graphic) {
            return false;
        }
        self.windows.world_press = Some(hand::ItemPress {
            item:   ContainedItem {
                serial,
                graphic: item.graphic,
                amount,
                // A ground item has no gump position. It becomes relevant only
                // after a drop, which supplies a real one.
                at: GumpPoint::new(0, 0),
                grid: Default::default(),
                hue: item.hue,
            },
            origin: hand::DragOrigin::Ground,
            // The completed frame preserved the actual art texel under the
            // pointer, already in the cursor preview's gump pixels.
            grab:   hovered.grab,
        });
        true
    }

    /// Which container icon the pointer is resting on, asked of the window it
    /// is on.
    ///
    /// **The manager asking a pane, the way `close_window` asks a dialog for
    /// its dismissal.** The tint and the label are `ContainerPane`'s own now —
    /// it remembers what it drew — and this exists because one reader is not a
    /// window at all: the tooltip pick order, which puts an icon in an open bag
    /// in front of anything in the world behind it. Top-down and first answer
    /// wins, so it is the same window the pointer is on.
    pub(crate) fn hovered_container_item(&self) -> Option<Serial> {
        self.windows.own_windows.iter().rev().find_map(|window| {
            match &window.pane {
                crate::panes::AnyPane::Container(pane) => pane.hovered(),
                // Every other kind's hover is about something that is not an
                // item the shard can be asked about.
                _ => None,
            }
        })
    }

    // No `container_item_under_pointer`, no `hover_container_item` and no
    // `paperdoll_item_under_pointer`: each was a second walk over a picture
    // some pane had already laid out, and each is that pane's own hit test now.
    // The container's was the worst of the three — it picked an index out of
    // the pictures and then counted that far into a list *rebuilt* from the
    // view, with the lifted icon filtered out again by hand, in the order the
    // layout had filtered it. `container::Window` carries what it drew.

    /// Turn a genuine pointer move into a lift, for the one press no window
    /// holds: an item lying in the world.
    ///
    /// The rule itself is [`ItemPress::dragged`](hand::ItemPress::dragged), which a bag's pane and a
    /// doll's pane ask of their own presses — one policy, three holders. What
    /// is different here is only what happens to the answer, because there is
    /// no pane to hand an effect to.
    pub(crate) fn drag_world_item(&mut self) -> bool {
        let Some(press) = self.windows.world_press else {
            return false;
        };
        // Suspended under the amount prompt: the number the player is choosing
        // is about this press, and lifting it now would divide nothing.
        if self.windows.prompt.is_some() {
            return false;
        }
        match press.dragged(self.input.past_slop(), self.input.shift_held) {
            hand::Dragged::Still => false,
            hand::Dragged::Ask(most) => {
                self.open_split_prompt(
                    windows::Asking::World,
                    crate::panes::SplitPrompt {
                        item: press.item.serial,
                        most,
                    },
                );
                true
            }
            hand::Dragged::Lift(drag) => {
                self.windows.world_press = None;
                self.lift(drag);
                self.windows.grip.release();
                true
            }
        }
    }

    /// Put `drag` on the cursor: the `0x07` and the hand it fills.
    ///
    /// The manager's half of [`Effect::Lift`](crate::panes::Effect::Lift),
    /// shared with it rather than restated — a pane's lift and the world's are
    /// the same act, and there is one place that performs it.
    /// Nothing happens without a shard to ask: the hand is a *mirror* of the
    /// other end's slot, and one filled with no packet behind it is an item
    /// this client has taken out of a bag nobody else knows about.
    pub(crate) fn lift(&mut self, drag: hand::ItemDrag) {
        let Some(link) = self.world.shard.link() else {
            return;
        };
        link.pick_up_item(drag.item.serial, drag.item.amount);
        self.windows.hand = Some(hand::Hand::Held(drag));
        self.reproject_item_drag();
    }

    /// Put the amount picker up over a press, and remember whose press it is.
    ///
    /// The one door, for both pressers: a bag's pane asks for it as
    /// [`Effect::Prompt`](crate::panes::Effect::Prompt) and the manager asks for
    /// it directly for an item lying on the ground, and everything about *the
    /// window* is the same either way. What differs is one value — `asker`, the
    /// record the answer is later routed by — which is exactly the thing
    /// [`windows::Asking`] exists to name.
    ///
    /// It needs no shell. The picker used to be an `egui::Window`, so a client
    /// running without a HUD could not divide a stack at all — the effect was
    /// dropped on the floor when `App::shell` was `None`. It is a gump window
    /// now, drawn by the same pass as every other, and the gesture works in any
    /// build that can draw a container.
    pub(crate) fn open_split_prompt(&mut self, asker: windows::Asking, prompt: crate::panes::SplitPrompt) {
        // Read before the list is borrowed: the scale comes off the shell, so
        // asking for it holds the whole of `self`.
        let scale = self.window_scale();
        windows::open_split_window(
            &mut self.windows.own_windows,
            prompt,
            self.input.pointer_gump,
            scale,
        );
        self.windows.prompt = Some(asker);
        // The keys go to the picker from the moment it opens — the reference's
        // own `SetKeyboardFocus`, and what lets an exact figure be typed into a
        // pile the bar has no pixels for. The window it takes them *from* is
        // whatever had them, which is the manager's to say (decision 2).
        self.windows.keyboard = Some(WindowSubject::Split {
            item: prompt.item,
            most: prompt.most,
        });
        // A window cannot be being dragged while a question about a press is
        // standing over it.
        self.windows.grip.release();
    }

    /// The picker has been answered: take it down and hand the answer to
    /// whichever press it went up over.
    ///
    /// **Closing first, delivering second.** The presser may answer by lifting,
    /// dropping and asking for the container again ([`ContainerPane`]'s three
    /// effects), and none of that should be happening underneath a window that
    /// is still on the screen. The record of who was asked
    /// ([`windows::Windows::prompt`]) is cleared *after* the walk, because the
    /// walk is what reads it to find the addressee.
    pub(crate) fn answer_prompt(&mut self, answer: crate::panes::Answer) {
        self.windows
            .own_windows
            .retain(|window| !matches!(window.subject, WindowSubject::Split { .. }));
        if matches!(self.windows.keyboard, Some(WindowSubject::Split { .. })) {
            self.windows.keyboard = None;
        }
        let _answered = self.deliver(crate::panes::Input::Answered(answer));
        self.windows.prompt = None;
    }

    /// The amount prompt has been answered, and the press it suspended is the
    /// world's.
    ///
    /// A ground stack stays on the cursor once it is divided — there is no
    /// window to put the remainder back into, and the shard's own `0x1A` will
    /// show what is left lying there.
    pub(crate) fn split_world_press(&mut self, answer: crate::panes::Answer) {
        let Some(press) = self.windows.world_press.take() else {
            return;
        };
        let crate::panes::Answer::Split(amount) = answer else {
            return;
        };
        let Some(drag) = press.split(amount) else {
            return;
        };
        self.lift(drag);
    }

    /// Put a held item down where no window claimed it: on the ground under
    /// the pointer.
    ///
    /// Reached only after every pane has declined the release — a bag answers
    /// a drop into itself and a doll an equip — which is why this arm is the
    /// world's and not a `match` over window kinds. Releasing over a shop or a
    /// skill sheet drops on the ground behind it, exactly as it did: neither
    /// window is a place to put anything.
    pub(crate) fn drop_hand_on_ground(&mut self) -> bool {
        let Some(hand) = self.windows.hand else {
            return false;
        };
        // A drop already in flight: the release is still the hand's — it must
        // not walk the body — and there is nothing more to send.
        let hand::Hand::Held(drag) = hand else {
            return true;
        };
        // Outside a gump the protocol's x/y/z are world coordinates, not gump
        // pixels. `pick_tile` already answers against the frame the player
        // released over.
        let Some(tile) = self.pick_tile(*self.control.camera()) else {
            self.audio.play_ui_sound(
                openshard_protocol::wire::SoundId(0x0051),
                self.world.motion.planning_state().position,
            );
            return true;
        };
        let at = Point::new(tile.at.x, tile.at.y, tile.stand_z.0);
        if !in_drag_range(self.world.motion.planning_state().position, at) {
            self.audio.play_ui_sound(
                openshard_protocol::wire::SoundId(0x0051),
                self.world.motion.planning_state().position,
            );
            return true;
        }
        let Some(link) = self.world.shard.link() else {
            return true;
        };
        link.drop_on_ground(drag.item.serial, at);
        self.windows.hand = Some(hand::Hand::Dropped {
            drag,
            destination: hand::PendingDrop::Ground(at),
        });
        self.reproject_item_drag();
        true
    }

    /// Finish a press on a world item that never became a drag.
    ///
    /// The double-click decision was made on the press — see `event_loop`'s own
    /// pairing — so there is nothing left to do but forget it.
    pub(crate) fn release_world_press(&mut self) -> bool {
        // Suspended under the prompt: the release that opened it must not put
        // the press down before the number arrives. Named rather than
        // `is_some`, because a *window's* prompt is that window's business and
        // its pane has already answered this release.
        if self.windows.prompt == Some(windows::Asking::World) {
            return true;
        }
        self.windows.world_press.take().is_some()
    }

    /// Which window the cursor is over, topmost first, or `None`.
    ///
    /// Against **every picture the window drew**, and each against its own
    /// opaque texels rather than a bounding box: a bag's art has transparent
    /// corners, a paperdoll's frame has a large transparent middle, and a click
    /// in either belongs to whatever is behind it — which is usually the world.
    /// A hat that the doll wears past the edge of its frame is the window's, and
    /// a hole in the frame's own corner is not: both fall out of asking the
    /// list, and neither did when this asked the background alone.
    ///
    /// The list is the last frame's — see [`windows::Windows::drawn_windows`] for why it is
    /// remembered rather than laid out again here — and the z-order is
    /// [`windows::Windows::own_windows`]'s, which is current: raising a window on the press
    /// must not wait for a frame.
    pub(crate) fn window_under_pointer(&self) -> Option<WindowSubject> {
        let cursor = self.input.pointer_gump;
        self.windows.own_windows.iter().rev().find_map(|window| {
            let drawn = self.drawn(window.subject)?;
            // Every pane laid this window out window-local and at its art's
            // own size (see `panes::PaneFrame::cursor`'s doc), so the pointer
            // has to be converted into *this* window's own pixels before it is
            // tested against what that window drew — the other half of the one
            // arithmetic `render_passes.rs`'s draw pass does with
            // `gump::place`.
            let local = window.local_cursor(cursor, self.window_scale());
            if let Drawn::Vendor(vendor) = drawn {
                return vendor.contains(local).then_some(window.subject);
            }
            // A dialog's fields are the one part of a window that is a box
            // rather than a picture — see `gump::Field` — and a click in one is
            // still a click on the window. It sits over the background, which is
            // a picture, so this only matters for a field the layout hung
            // outside its own frame; asking is cheaper than being wrong there.
            if let Drawn::Dialog(laid_out) = drawn {
                if gump_art::field(&laid_out.art.fields, local).is_some() {
                    return Some(window.subject);
                }
            }
            if let Drawn::Minimap(minimap) = drawn {
                return minimap.contains(local).then_some(window.subject);
            }
            if let Drawn::WorldMap(map) = drawn {
                return map.contains(local).then_some(window.subject);
            }
            gump_art::pick(drawn.pictures(), local, &self.resources.gump_atlas).map(|_| window.subject)
        })
    }

    /// What the last frame drew for one window, or `None` for a window that has
    /// not been drawn yet — every window on the frame its packet arrived.
    pub(crate) fn drawn(&self, subject: WindowSubject) -> Option<&Drawn> {
        self.windows
            .drawn_windows
            .iter()
            .find(|(drawn, _)| *drawn == subject)
            .map(|(_, drawn)| drawn)
    }

    /// The dialog a subject names, out of the view, or `None` if the shard has
    /// taken it away since.
    pub(crate) fn open_gump(&self, gump_id: GumpId) -> Option<&openshard_client_net::view::OpenGump> {
        self.world
            .authoritative
            .view
            .as_ref()?
            .gumps
            .iter()
            .find(|gump| gump.gump_id == gump_id)
    }

    /// Raise a window to the top of the pile, so that the one just clicked is
    /// the one drawn over the others.
    pub(crate) fn raise_window(&mut self, subject: WindowSubject) {
        if let Some(index) = self
            .windows
            .own_windows
            .iter()
            .position(|window| window.subject == subject)
        {
            let window = self.windows.own_windows.remove(index);
            self.windows.own_windows.push(window);
        }
    }

    /// A left press over one of this client's windows that no pane answered:
    /// raise it and take hold of it.
    ///
    /// Answers whether the press belonged to a window, so the caller can leave
    /// the world's own click alone when it did — a press that raised a bag must
    /// not also select the tile behind it.
    ///
    /// **All that is left of a function that used to branch on five window
    /// kinds.** Every kind answers its own press in its own pane now, and each
    /// of them emits [`Effect::Grab`](crate::panes::Effect::Grab) for the press
    /// that landed on nothing in particular — so what reaches here is a press
    /// on a kind that has *no* input of its own to speak of: a status frame,
    /// which is a unit struct, or a window that has never been drawn and has no
    /// layout to hit-test.
    ///
    /// It is in the router's third rung (`App::fallback_gestures`) rather than
    /// in `manager_gestures` because it has to run **behind** the panes: a
    /// shop's Confirm button and a sheet's thumb are asked first, and only a
    /// press none of them wanted picks the window up.
    ///
    /// The press while the hand is full is **not** asked about here: it is the
    /// manager's first question, ahead of every pane and of this — see
    /// `App::manager_gestures` and decision 7 in `docs/client/design_panes.md`.
    ///
    /// `owner` is `App::window_under_pointer`'s answer, asked once by
    /// `App::deliver` and handed down rather than asked again here.
    pub(crate) fn press_on_own_window(&mut self, owner: Option<WindowSubject>) -> bool {
        // A press that missed every window gives the keyboard back, and that is
        // the manager's own gesture now — see `App::manager_gestures`, which
        // runs it ahead of every pane and of this.
        let Some(subject) = owner else {
            return false;
        };
        self.raise_window(subject);
        self.grab_window(subject);
        true
    }

    /// Take hold of one window: the press half of a drag, whoever asked for it.
    ///
    /// The one door, and it has two callers — this rung's
    /// [`App::press_on_own_window`] above, and
    /// [`Effect::Grab`](crate::panes::Effect::Grab) for the panes, which is
    /// every window kind that answers its own press. They used to each work
    /// out the grab for themselves, in two different pixel spaces, which is
    /// the defect [`WindowGrip`] documents; now neither of them computes
    /// anything at all — a grab is "the pointer went down on this window", and
    /// where it went down is read here, once, from the same field the move
    /// will read.
    ///
    /// A subject with no window on the list is not held: nothing to carry, and
    /// a hold on a window that does not exist would swallow the next real
    /// press. Unreachable today — both callers name a window they have just
    /// found — and written down so the contract does not rest on that.
    pub(crate) fn grab_window(&mut self, subject: WindowSubject) {
        let Some(window) = self
            .windows
            .own_windows
            .iter()
            .find(|window| window.subject == subject)
        else {
            return;
        };
        self.windows
            .grip
            .press(subject, self.input.pointer_gump, window.at);
    }

    /// Carry the held window: it stands where it stood when the button went
    /// down, moved by exactly as far as the pointer has travelled since. The
    /// follow half of [`WindowGrip`], and the only writer of
    /// [`OwnWindow::at`](crate::windows::OwnWindow::at) while a window is
    /// held.
    ///
    /// Answers whether anything moved, which is the caller's redraw: a pointer
    /// that has come back to where it went down puts the window back where it
    /// started, and says nothing changed because nothing did.
    pub(crate) fn drag_own_window(&mut self) -> bool {
        let Some((subject, at)) = self.windows.grip.follow(self.input.pointer_gump) else {
            return false;
        };
        let Some(window) = self
            .windows
            .own_windows
            .iter_mut()
            .find(|window| window.subject == subject)
        else {
            // The window went away under the press — closed by a packet, or
            // dropped by the reconcile because the view stopped naming what it
            // was over. Let the grip go rather than holding a subject nothing
            // answers to: the release that would have cleared it is about to
            // arrive over some *other* window, and a stale hold is a second
            // window that starts moving the moment the list gets that subject
            // back.
            self.windows.grip.release();
            return false;
        };
        let moved = window.at != at;
        window.at = at;
        moved
    }

    /// Close the window under the cursor, if there is one.
    ///
    /// The right button, which is what the reference client closes a gump with,
    /// and it is *not* a conflict with the right-hold that steers: a press over
    /// a window never reaches the world, the same way a press over a panel does
    /// not. Answers whether the press was the window's — see
    /// [`App::close_window`].
    ///
    /// `owner` is `App::window_under_pointer`'s answer, asked once by
    /// `App::deliver` and handed down rather than asked again here.
    pub(crate) fn close_window_under_pointer(&mut self, owner: Option<WindowSubject>) -> bool {
        let Some(subject) = owner else {
            return false;
        };
        self.close_window(subject)
    }

    /// The topmost of this client's own windows, closed from the keyboard.
    ///
    /// [`windows::Windows::own_windows`] is in painter's order, so its last entry is the one
    /// drawn over the others — which is what a player means by "this window"
    /// when they have not pointed at anything.
    ///
    /// **Why the keyboard needs a route of its own.** A gump window is drawn by
    /// this client's own pass and egui is painted *over* it, so a floating panel
    /// standing on one covers it and takes the mouse with it:
    /// `Shell::on_window_event` claims the click before any of `window_event`'s
    /// arms are reached, and the right button never gets as far as
    /// [`App::close_window_under_pointer`]. The skill window cascades to
    /// `CONTAINER_ORIGIN`, which is inside where the dev window opens — so for
    /// as long as Escape quit the client, it was a window with no way out.
    pub(crate) fn close_top_window(&mut self) -> bool {
        let Some(subject) = self.windows.own_windows.last().map(|window| window.subject) else {
            return false;
        };
        self.close_window(subject)
    }

    /// Take one window down, whichever gesture asked for it — the right button
    /// over it, or Escape on the topmost.
    ///
    /// Answers whether the window *took* the request rather than whether it
    /// closed: a `{ noclose }` dialog stays up and still answers true, because
    /// the press that asked was the window's and must not reach the world
    /// behind it.
    ///
    /// Nothing goes out on the wire, for either kind. There is no
    /// close-container packet and no close-paperdoll packet — the shard keeps
    /// its own list of who has what open — which is why this end predicts the
    /// close locally (see [`windows::Windows::locally_closed`]) rather than waiting for a
    /// packet that never comes.
    /// A dialog is the one kind that *does* send something: the shard is
    /// waiting for a `0xB1` and gets button zero, which is what the reference
    /// client's close box answers with. A `{ noclose }` layout has no such
    /// answer to give — `dismiss` refuses it — and the window stays up, which is
    /// what that flag is for.
    pub(crate) fn close_window(&mut self, subject: WindowSubject) -> bool {
        if let WindowSubject::Dialog(gump_id) = subject {
            let Some(gump) = self.open_gump(gump_id).cloned() else {
                return false;
            };
            // Asked of the window's own pane, because what a dialog answers with
            // is made of what the player set on it. The manager still decides
            // *that* it closes — both gestures that close one are its own, and
            // Escape closes the topmost window without ever pointing at it — so
            // this is the one door and the pane is the one that can fill in the
            // packet.
            let Some(window) = self
                .windows
                .own_windows
                .iter()
                .find(|window| window.subject == subject)
            else {
                // No window with this subject is open at all — there is nothing
                // here for this press to have taken, so it did not. Unreachable
                // today: both callers (`close_window_under_pointer`,
                // `close_top_window`) only ever pass a subject that came from a
                // real window. Written down anyway, so the function's contract
                // does not depend on its callers never trying otherwise.
                return false;
            };
            let dismissal = match &window.pane {
                crate::panes::AnyPane::Dialog(pane) => pane.dismiss(&gump),
                // A dialog window always holds a dialog pane — `AnyPane::of`
                // is a `match` on the subject — so this is not a case, it is
                // the compiler being told the same thing twice.
                _ => None,
            };
            let Some(reply) = dismissal else {
                // The dialog is open, but nothing dismiss-worthy has been
                // answered on it yet. The press is still the window's — it
                // must not steer the body — so this says the window took it.
                return true;
            };
            self.answer_gump(reply);
            self.windows
                .own_windows
                .retain(|window| window.subject != subject);
            self.windows.grip.release();
            return true;
        }
        // Closing the picker *is* dismissing the prompt, which is the same
        // shape a dialog's close has one arm up: the window is a question, so
        // taking it down has to answer it. Ahead of the view check below and
        // not below it, because the press it is standing over is this client's
        // own state — a prompt left up over a world that has gone away is a
        // window nothing can take down.
        if let WindowSubject::Split { .. } = subject {
            self.answer_prompt(crate::panes::Answer::Cancelled);
            self.windows.grip.release();
            return true;
        }
        if self.world.authoritative.view.is_none() {
            return false;
        }
        match subject {
            WindowSubject::Container(serial) => {
                // The overlay is what says this is closed — see D2 in
                // `docs/client/evidence/2026-08-15-one-owner-for-a-window.md`. The line under it writes the
                // same fact into this thread's own view, which is redundant
                // for a container and is not for a vendor (the arm below sets
                // only the overlay); both are kept until that plan's backlog
                // settles which one stays. The link thread is told neither:
                // its `WorldView` is cloned across exactly once, at world
                // entry, so there is nothing there to go stale.
                self.windows.locally_closed.insert(subject);
                self.apply_close_window(link::CloseTarget::Container(serial));
            }
            // Nothing to forget by hand: what was chosen and how far down the
            // list the player had got are fields of the pane, and the `retain`
            // at the end of this drops the window and them with it. There used
            // to be two `remove` calls here, and a kind that was added without
            // them would have leaked its state for the life of the client.
            WindowSubject::Vendor(_) => {
                self.windows.locally_closed.insert(subject);
            }
            WindowSubject::Paperdoll(serial) => {
                self.windows.locally_closed.insert(subject);
                self.apply_close_window(link::CloseTarget::Paperdoll(serial));
            }
            // The shard has no close packet for a spellbook.  Forget this
            // thread's `0xBF 0x1B` snapshot; using the book again supplies a
            // fresh one and reopens it through ordinary reconciliation.
            WindowSubject::Spellbook(serial) => {
                self.windows.locally_closed.insert(subject);
                self.apply_close_window(link::CloseTarget::Spellbook(serial));
            }
            // Nothing in the view to tell and so nothing to overlay: the
            // skills and the status numbers stay where they are, the way a
            // paperdoll's equipment does. What closing takes away is whatever
            // the window's pane held — a tree for one kind, nothing at all for
            // the other — and the `retain` at the end of this is what takes it.
            // That is deliberate and not a loss: the reference's windows do not
            // remember either, and a window with no memory is the backlog entry
            // every kind here shares.
            //
            // The status arm used to be `self.windows.status = false`, which is
            // the same close said twice: once here and once in a field. Step 3
            // deleted the field.
            WindowSubject::Skills
            | WindowSubject::Status
            | WindowSubject::Minimap
            | WindowSubject::WorldMap => {}
            // Both are answered above, each by the one act that is its close:
            // a dialog sends button zero, and the picker dismisses the press it
            // was standing over.
            WindowSubject::Split { .. } => unreachable!("dismissed above"),
            // A question is the view's, so closing one *is* overlaid — the
            // container's shape rather than the skill sheet's. Nothing goes out
            // on the wire: dismissing a plate is not an answer, and the shard
            // hears from this client only when a button was pressed. The overlay
            // is dropped by `reconcile_own_windows` on the frame the view agrees
            // the question is settled, which is what stops the plate from coming
            // straight back up.
            WindowSubject::Confirm(_) | WindowSubject::Party => {
                self.windows.locally_closed.insert(subject);
            }
            WindowSubject::Dialog(_) => unreachable!("answered above"),
        }
        self.windows
            .own_windows
            .retain(|window| window.subject != subject);
        self.windows.grip.release();
        true
    }

    /// Say a line out loud, if there is a shard to hear it.
    ///
    /// Nothing is echoed locally. A shard sends every speaker their own words
    /// back — that is what makes `0xAE` exist — so a client that also drew them
    /// itself would show everything twice, and a line that never reached the
    /// server would look exactly like one that did.
    ///
    /// Offline the line goes nowhere and says so in the log rather than
    /// silently: the map viewer has nobody to talk to, and a chat box that
    /// swallowed what was typed would read as a broken connection.
    pub(crate) fn say(&mut self, line: String) {
        let Some(link) = self.world.shard.link() else {
            tracing::info!(%line, "nothing said: no shard is connected");
            return;
        };
        // Which channel decides which *packet*, not only which mode byte: a
        // guild or alliance line is `0xAD` speech with a different mode, and a
        // party line is not speech at all — it is `0xBF 0x06`, and putting it
        // through the speech path would say it out loud to the street.
        match self.chat.channel {
            chat::Channel::Say => link.say(line, TalkMode::Regular),
            chat::Channel::Guild => link.say(line, TalkMode::Guild),
            chat::Channel::Alliance => link.say(line, TalkMode::Alliance),
            chat::Channel::Party => link.say_to_party(line),
        }
    }

    /// Answer an open dialog and take it off the screen.
    ///
    /// The close is this end's, and it is why the overlay is set here rather
    /// than waiting for a packet: the server sends one `0xB0` and waits for
    /// one `0xB1`, and nothing ever arrives to say the window is gone. See
    /// [`windows::Windows::locally_closed`].
    pub(crate) fn answer_gump(&mut self, reply: link::GumpReply) {
        let gump_id = openshard_protocol::gump::GumpId(reply.gump_id.0);
        if let Some(link) = self.world.shard.link() {
            link.answer_gump(reply);
            // The reply leaves on the wire and says nothing about the window
            // being done, so this end has to close it itself — here in this
            // thread's view, and in the overlay below.
            self.apply_close_window(link::CloseTarget::Gump(gump_id));
        }
        if self.world.authoritative.view.is_some() {
            self.windows.locally_closed.insert(WindowSubject::Dialog(gump_id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn houses_and_their_signs_never_start_an_item_drag() {
        assert!(!movable_world_graphic(Graphic(MultiId::FLAG | 0x0064)));
        assert!(!movable_world_graphic(HOUSE_SIGN_GRAPHIC));
        assert!(movable_world_graphic(Graphic(0x0EED)));
    }
}

#[cfg(test)]
mod stack_tests {
    use openshard_protocol::containers::GridSlot;
    use openshard_protocol::items::ItemAmount;
    use openshard_protocol::wire::Hue;

    use super::*;

    fn pile(serial: u32, graphic: Graphic, amount: u16) -> ContainedItem {
        ContainedItem {
            serial: Serial::new(serial).expect("item serial"),
            graphic,
            amount: ItemAmount(amount),
            at: GumpPoint::new(0, 0),
            grid: GridSlot(0),
            hue: Hue::NONE,
        }
    }

    #[test]
    fn stacking_prefers_a_whole_pile_that_fits() {
        let items = [
            pile(0x4000_0001, GOLD_GRAPHIC, 50_000),
            pile(0x4000_0002, GOLD_GRAPHIC, 20_000),
            pile(0x4000_0003, GOLD_GRAPHIC, 5_000),
        ];
        assert_eq!(
            next_stack_step(&items),
            Some(StackStep {
                source: items[2].serial,
                target: items[0].serial,
                amount: 5_000,
            })
        );
    }

    #[test]
    fn stacking_plans_the_two_gold_piles_in_the_backpack() {
        let items = [
            pile(0x4000_0001, GOLD_GRAPHIC, 1_838),
            pile(0x4000_0002, GOLD_GRAPHIC, 60),
        ];
        assert_eq!(
            next_stack_step(&items),
            Some(StackStep {
                source: items[1].serial,
                target: items[0].serial,
                amount: 60,
            })
        );
    }

    #[test]
    fn stacking_splits_only_enough_to_fill_sixty_thousand() {
        let items = [
            pile(0x4000_0001, GOLD_GRAPHIC, 55_000),
            pile(0x4000_0002, GOLD_GRAPHIC, 20_000),
        ];
        assert_eq!(next_stack_step(&items).expect("one pass").amount, 5_000);
    }

    #[test]
    fn full_piles_are_skipped_on_the_way_to_a_remainder() {
        let items = [
            pile(0x4000_0001, GOLD_GRAPHIC, MAX_STACK),
            pile(0x4000_0002, GOLD_GRAPHIC, 15_000),
            pile(0x4000_0003, GOLD_GRAPHIC, 10_000),
        ];
        let step = next_stack_step(&items).expect("the remainder piles merge");
        assert_eq!(step.target, items[1].serial);
        assert_eq!(step.amount, 10_000);
    }

    #[test]
    fn stacking_recognises_single_arrows_and_bolts() {
        for graphic in [ARROW_GRAPHIC, BOLT_GRAPHIC] {
            let items = [pile(0x4000_0001, graphic, 1), pile(0x4000_0002, graphic, 1)];
            assert_eq!(
                next_stack_step(&items),
                Some(StackStep {
                    source: items[1].serial,
                    target: items[0].serial,
                    amount: 1,
                }),
                "0x{:04X} is stackable as a singleton",
                graphic.0
            );
        }
    }

    // The split's own bounds moved with the rule: `ItemPress::split` is what
    // divides a pile now, and it is exercised beside it in `hand.rs`.
}

#[cfg(test)]
mod range_tests {
    use super::*;

    #[test]
    fn classic_drag_range_is_three_tiles_in_chebyshev_distance() {
        let here = Point::new(100, 100, 0);
        assert!(in_drag_range(here, Point::new(103, 103, 0)));
        assert!(!in_drag_range(here, Point::new(104, 100, 0)));
    }
}
