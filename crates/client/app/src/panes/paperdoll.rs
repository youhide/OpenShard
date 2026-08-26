//! A body's paperdoll as a component: the fifth window kind to own its state,
//! its layout and its input.
//!
//! Step 5 of `docs/window_components.md`, and the first kind whose effects are
//! mostly [`Effect::Open`] and [`Effect::Net`]: nearly every control on the
//! frame asks for another window — the shard's, through a packet, or one of
//! the two local kinds, through [`Effect::Open`], which had been waiting under
//! `#[expect(dead_code)]` since step 2 for exactly this pane. The enum the
//! buttons come from says so in its own doc: [`paperdoll::DollButton`] "names
//! windows rather than actions".
//!
//! # The worn item, which arrived at step 6
//!
//! A press on a worn item of this client's own body starts an item transfer,
//! and step 5 **declined** that one press so that the fallback chain could
//! answer it: the machinery it needs — a press that may become a lift, and the
//! hand it fills — moved with the container at step 6. Both halves are here
//! now. A worn item is pressed, dragged off the doll and lifted like an icon
//! in a bag ([`PaperdollPane::pressed`]), and an item let go over *any* doll
//! is offered to that body's empty slot ([`Effect::Drop`]). The backpack is
//! the exception: its picture names the container itself, so a drop on it
//! goes into the pack without requiring its window to be open first.
//!
//! What is still not this pane's is the hand itself: which item is on the
//! cursor is [`Windows::hand`](crate::windows::Windows::hand), because there
//! is one cursor and the shard has one slot — decision 7.

use std::time::Instant;

use openshard_client_net::action::Outgoing;
use openshard_client_render::gump::{self as gump_art, GumpArt, GumpPixel};
use openshard_client_render::mobiles::EquipmentLayer;
use openshard_client_render::paperdoll;
use openshard_protocol::containers::{ContainedItem, GridSlot};
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::items::{ItemAmount, classic_weapon_layer, is_classic_weapon};
use openshard_protocol::mobile::Equipment;
use openshard_protocol::serial::Serial;
use openshard_protocol::speech::Font;
use openshard_protocol::wire::{Hue, Layer};

use crate::DOUBLE_CLICK;
use crate::crowd;
use crate::hand::{DragOrigin, Dragged, ItemPress, PendingDrop, centre_of};
use crate::panes::{Button, Effect, Input, Line, LocalWindow, PaneCtx, PaneFrame, Response};
use crate::windows::Drawn;

/// One open paperdoll window.
#[derive(Debug)]
pub struct PaperdollPane {
    /// Whose body this is a window over.
    ///
    /// A shop's serial, one kind over: the subject carries a key and the pane
    /// is handed it when the window opens, because whether this doll is the
    /// player's own — which decides the frame, the buttons and what a press on
    /// a worn item means — is a comparison against this.
    mobile: Serial,
    /// The button the mouse went down on.
    ///
    /// [`DialogPane::held`](crate::panes::dialog)'s counterpart, and the same
    /// three things it buys: the pressed picture is drawn while the finger is
    /// down, the release acts only if the pointer is still on the *same*
    /// button, and a press on a button does not also drag the frame under it.
    ///
    /// A [`paperdoll::DollButton`] and not a picture index: the doll is laid
    /// out afresh every frame — a hat coming off changes how many pictures are
    /// in front of the buttons — so an index taken at the press would name a
    /// different picture by the time the button comes up. The button itself is
    /// stable.
    held: Option<paperdoll::DollButton>,
    /// The last completed click on one of the three scrolls, and when.
    ///
    /// The scrolls answer a *double* click where the seven buttons answer a
    /// single one (`GumpPic.MouseDoubleClick` against `Button`'s
    /// `OnButtonClick`), and this is that pair. Separate from
    /// [`last_click`](crate::input::Input::last_click), which pairs clicks on
    /// the *world*: a click on a window never reaches that one. It used to be
    /// `Windows::last_scroll`, keyed by subject; the subject key is gone
    /// because the state is this window's own now, so two clicks on two dolls
    /// cannot pair **by construction** rather than by a comparison.
    last_scroll: Option<(Instant, paperdoll::DollButton)>,
    /// The last worn item clicked on this doll. A second click on the same
    /// item is a normal UO `Use`, just as it is for an icon in a container;
    /// it must be keyed by serial because a dagger and an axe may occupy the
    /// two hand layers at once.
    last_item_click: Option<(Instant, Serial)>,
    /// The worn layer under the pointer, tinted on the next frame.
    ///
    /// Remembered so that the *move* that changes it can say
    /// [`Response::stale`]: the tint is decided in the layout, and what draws
    /// a frame is the animation clock, not the move — a picture that depends
    /// on the pointer owes the ask itself. `Windows::hovered_equipment` kept
    /// this for every doll at once, keyed by serial; per-pane, the key is
    /// gone.
    hovered: Option<Layer>,
    /// The cursor, with the hand full, is over this doll — and the doll is
    /// this client's own body, so the held wearable is drawn on it
    /// translucently as a preview.
    ///
    /// Only the *whereabouts* are remembered; the item itself is read out of
    /// [`PaneFrame::hand`] at layout time, so a hand that has emptied takes
    /// the preview with it on the next frame whether or not the pointer moves
    /// again. `Windows::preview_equipment` kept a copy of the item instead,
    /// which could outlive the drag it described.
    hand_over: bool,
    /// The press on a worn item of this body, until it becomes a lift.
    ///
    /// [`ContainerPane::pressed`](crate::panes::container)'s counterpart, and
    /// the press step 5 declined for want of it. Nothing has been sent while
    /// it is alive; the item is still worn, still drawn on the doll, and a
    /// release without a move puts it back down without a packet.
    ///
    /// Only ever a press on *our own* body: a stranger's shirt is not this
    /// client's to lift, so a press on one moves the window instead, exactly
    /// as it did.
    pressed: Option<ItemPress>,
}

/// A paperdoll, laid out for one frame: the doll, and what is written over it.
///
/// [`dialog::Window`](crate::panes::dialog::Window)'s shape for the same
/// reason: the name on the plate comes out of the view and the hover label out
/// of `tiledata`, neither of which `client/render` has heard of, so the text
/// is resolved here — in the pane that owns the hover — and the pass draws it
/// out of what the layout produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Window {
    /// The frame, its furniture and the doll — everything the pointer is
    /// tested against.
    pub doll: paperdoll::Doll,
    /// The text over them: the name on the plate, and the worn item the
    /// pointer is resting on.
    pub lines: Vec<Line>,
}

/// Whether a click on `button` at `now` is the second of a pair.
///
/// A pair is two clicks on one scroll of one window. The window half is the
/// pane itself now — `last` never held another window's click — so what is
/// left to compare is the picture and the clock: the profile scroll and the
/// party manifest sit fourteen pixels apart on the same frame, and a rule that
/// only looked at the time would open a profile because the hand slipped
/// between two clicks.
fn pairs(
    last: Option<(Instant, paperdoll::DollButton)>,
    now: Instant,
    button: paperdoll::DollButton,
) -> bool {
    last.is_some_and(|(at, picture)| picture == button && now.duration_since(at) <= DOUBLE_CLICK)
}

/// What one completed click on a doll control asks for.
///
/// The whole of what was `App::doll_clicked`, as effects instead of link
/// calls — and a free function of what it actually reads, so the mapping can
/// be pinned without a view. Always [`Response::changed`]: the button was
/// drawn pressed and has to come back up, whether or not its arm asks for
/// anything.
fn button_effects(
    button: paperdoll::DollButton,
    mobile: Serial,
    own: bool,
    war: bool,
    backpack: Option<Serial>,
    paired: bool,
) -> Response {
    let answer = Response::changed();
    match button {
        paperdoll::DollButton::WarMode => answer.with(Effect::Net(Outgoing::WarMode(!war))),
        paperdoll::DollButton::LogOut => answer.with(Effect::Net(Outgoing::LogOut)),
        paperdoll::DollButton::Quests => answer.with(Effect::Net(Outgoing::QuestLog)),
        paperdoll::DollButton::Guild => answer.with(Effect::Net(Outgoing::GuildMenu)),
        // The reference's Options button owns a client-local window.  This
        // client has a facet map ready for exactly that role; unlike a shard
        // gump it must be opened here, because there is no packet to wait for.
        paperdoll::DollButton::Options if own => answer.with(Effect::Open(LocalWindow::WorldMap)),
        // Status packets describe this connection's player, so a stranger's
        // Status control must not open a misleading local window — and it
        // asks the shard nothing either, which is what the old handler did.
        paperdoll::DollButton::Status if own => answer
            .with(Effect::Net(Outgoing::Status(mobile)))
            .with(Effect::Open(LocalWindow::Status)),
        // Window visibility is local intent; the `0x3A` merely refreshes
        // data.
        paperdoll::DollButton::Skills if own => answer
            .with(Effect::Net(Outgoing::Skills(mobile)))
            .with(Effect::Open(LocalWindow::Skills)),
        paperdoll::DollButton::Virtue if paired => answer.with(Effect::Net(Outgoing::Virtue(mobile))),
        // The roster is already in the view; the scroll's job is to restore
        // its window when the player closed it, or bring the existing one up.
        paperdoll::DollButton::Party if paired => {
            answer.with(Effect::Reopen(crate::windows::WindowSubject::Party))
        }
        // Opening the backpack is a use of the worn bag, which needs its
        // serial — a body wearing none has nothing to open.
        paperdoll::DollButton::Backpack if paired => match backpack {
            Some(serial) => answer.with(Effect::Net(Outgoing::Use(serial))),
            None => answer,
        },
        // Help and the first click of every scroll pair: drawn,
        // pressed, released — and wired to nothing yet, exactly as they were
        // before this pane existed.
        _ => answer,
    }
}

impl PaperdollPane {
    /// The pane for the doll of `mobile`.
    pub const fn new(mobile: Serial) -> Self {
        Self {
            mobile,
            held: None,
            last_scroll: None,
            last_item_click: None,
            hovered: None,
            hand_over: false,
            pressed: None,
        }
    }

    /// Whether this doll is the player's own body.
    fn own(&self, frame: &PaneFrame<'_>) -> bool {
        frame.view.player.serial == self.mobile
    }

    /// The `0x88` permission is authoritative. It is normally set for our
    /// own body, but shards may set it for a pet too; ClassicUO permits both.
    /// Our own body remains liftable while an older shard has not sent its
    /// `0x88` yet — identity is the protocol's baseline permission.
    fn can_lift(&self, frame: &PaneFrame<'_>) -> bool {
        self.own(frame)
            || frame
                .view
                .paperdolls
                .get(&self.mobile)
                .is_some_and(|doll| doll.can_lift)
    }

    /// What the mobile is wearing, as the wire said it — the serials and wire
    /// graphics a click and a label need, as opposed to the `AnimID`s the
    /// doll is drawn from.
    fn equipment<'view>(&self, frame: &PaneFrame<'view>) -> &'view [Equipment] {
        match self.own(frame) {
            true => &frame.view.player.equipment,
            false => frame
                .view
                .mobiles
                .get(&self.mobile)
                .map_or(&[] as &[Equipment], |mobile| mobile.equipment.as_slice()),
        }
    }

    /// Match the shard's hand constraint for the classic weapon catalogue.
    /// A shield is not a weapon, so it remains valid beside a one-handed
    /// weapon; a weapon in the two-handed layer is not.
    fn hands_conflict(
        &self,
        frame: &PaneFrame<'_>,
        graphic: openshard_protocol::wire::Graphic,
        layer: Layer,
    ) -> bool {
        if !is_classic_weapon(graphic) {
            return false;
        }
        let equipment = self.equipment(frame);
        match layer {
            Layer::TWO_HANDED => equipment.iter().any(|item| item.layer == Layer::ONE_HANDED),
            Layer::ONE_HANDED => equipment
                .iter()
                .any(|item| item.layer == Layer::TWO_HANDED && is_classic_weapon(item.graphic)),
            _ => false,
        }
    }

    /// Whether this was the second click on the same scroll, and the
    /// bookkeeping either way: a completed pair arms nothing, a first click
    /// arms the next.
    fn scroll_paired(&mut self, button: paperdoll::DollButton, now: Instant) -> bool {
        let paired = pairs(self.last_scroll, now, button);
        self.last_scroll = (!paired).then_some((now, button));
        paired
    }

    /// Whether this is the second click on this exact worn item. The pane is
    /// one paperdoll, leaving its item serial as the only identity that must
    /// be compared: clicking the axe and then the dagger starts two gestures,
    /// never a use of whichever one was clicked first.
    fn item_paired(&mut self, item: Serial, now: Instant) -> bool {
        let paired = self
            .last_item_click
            .is_some_and(|(at, previous)| previous == item && now.duration_since(at) <= DOUBLE_CLICK);
        self.last_item_click = (!paired).then_some((now, item));
        paired
    }

    /// A left press somewhere in the window, against the layout it was drawn
    /// as.
    ///
    /// Every arm that answers raises — a press on a window is what puts it on
    /// top. A button takes the press away from the drag: the column runs down
    /// the middle of the frame, and pressing one used to pick the whole doll
    /// up. A worn item of *our own* body takes it away from the drag too, and
    /// keeps it: that press may become a lift on the next move.
    fn press(&mut self, window: &Window, ctx: &PaneCtx<'_>) -> Response {
        let raised = Response::changed().with(Effect::Raise);
        if let Some(index) = gump_art::pick(
            &window.doll.pictures,
            ctx.frame.cursor,
            ctx.frame.files.gump_atlas,
        ) {
            if let Some(button) = window.doll.hits.get(&index) {
                self.held = Some(*button);
                return raised;
            }
            // A worn item on our own body: the press that starts an item
            // transfer. A stranger's is not this client's to lift and falls
            // through to the grab below, as it always has.
            if let Some(layer) = window.doll.equipment_hits.get(&index).copied() {
                if self.can_lift(&ctx.frame) {
                    if let Some(item) = self
                        .equipment(&ctx.frame)
                        .iter()
                        .find(|item| item.layer == layer)
                        .copied()
                    {
                        // A worn tool is still an ordinary object. Its second
                        // click must reach the shard, otherwise axes cannot
                        // chop and daggers cannot cut while equipped.
                        if self.item_paired(item.serial, ctx.now) {
                            return raised.with(Effect::Net(Outgoing::Use(item.serial)));
                        }
                        self.pressed = Some(ItemPress {
                            item: ContainedItem {
                                serial: item.serial,
                                graphic: item.graphic,
                                // A worn item is one thing, whatever the wire
                                // says about the pile it came out of: nothing
                                // stacks on a body.
                                amount: ItemAmount(1),
                                // No gump position — it is on a body, not in a
                                // bag. One becomes relevant only after a drop,
                                // which supplies its own.
                                at: GumpPoint::new(0, 0),
                                grid: GridSlot(0),
                                hue: item.hue,
                            },
                            origin: DragOrigin::Equipment {
                                mobile: self.mobile,
                                layer,
                            },
                            // A doll's sprite has no gump-local grab point:
                            // what is drawn there is the *paperdoll* art, and
                            // what goes on the cursor is the item's own icon.
                            // Anchor its centre to the pointer, and use the
                            // same offset if it is released into a bag.
                            grab: centre_of(item.graphic, ctx.frame.files.art),
                        });
                        return raised;
                    }
                }
            }
        }
        // The frame, the body, a stranger's shirt: nothing that answers, so
        // the press moves the window. Where in the frame it landed is not said
        // — a window drag is a delta the manager measures against its own
        // pointer, and a pane's window-local cursor in that arithmetic is what
        // used to teleport this very window; see `Effect::Grab` and
        // `windows::WindowGrip`.
        raised.with(Effect::Grab)
    }

    /// The release that finishes whatever this window was in the middle of: a
    /// press on one of its buttons, a press on a worn item, or an item let go
    /// over the body.
    ///
    /// The pointer has to still be on the button it went down on: a press
    /// dragged off its button is a press taken back, in this client as in
    /// every other. Either way the answer is [`Response::changed`] once a
    /// press is being held — the button was drawn pressed and has to come
    /// back up.
    fn release(&mut self, ctx: &PaneCtx<'_>) -> Response {
        // A full hand let go over this doll is an equip, and it is asked for
        // over **any** body: whether a stranger may be dressed by this player
        // is the shard's answer, and a refusal comes back as a `0x27` that
        // puts the item where it came from. Keeping that judgement here used
        // to strand the transfer with nobody to bounce it.
        if let (Some(hand), true) = (ctx.frame.hand, ctx.under_pointer) {
            // The worn backpack's picture is both paperdoll furniture (a
            // double click opens it) and the closed container itself.  A drop
            // on those opaque pixels therefore names the bag, not the body's
            // equipment slot.  Without this arm the exact same gesture only
            // worked after opening the container gump, because every release
            // handled here fell through to `Equip`.
            let on_backpack = match ctx.drawn {
                Some(Drawn::Paperdoll(window)) => {
                    gump_art::pick(
                        &window.doll.pictures,
                        ctx.frame.cursor,
                        ctx.frame.files.gump_atlas,
                    )
                    .and_then(|index| window.doll.hits.get(&index).copied())
                        == Some(paperdoll::DollButton::Backpack)
                }
                _ => false,
            };
            if on_backpack {
                if let Some(backpack) = self
                    .equipment(&ctx.frame)
                    .iter()
                    .find(|item| item.layer == Layer::BACKPACK)
                    .map(|item| item.serial)
                {
                    // A closed bag has no meaningful gump-local cursor
                    // coordinate. The shard accepts the same neutral origin
                    // used by its other container-targeted gestures.
                    return Response::changed().with(Effect::Drop(PendingDrop::Container {
                        container: backpack,
                        at: GumpPoint::new(0, 0),
                    }));
                }
            }
            let layer = classic_weapon_layer(
                hand.drag().item.graphic,
                Layer(
                    ctx.frame
                        .files
                        .tiledata
                        .static_tile(hand.drag().item.graphic.0)
                        .layer,
                ),
            );
            if self.hands_conflict(&ctx.frame, hand.drag().item.graphic, layer) {
                return Response::consumed();
            }
            return Response::changed().with(Effect::Drop(PendingDrop::Equipment {
                mobile: self.mobile,
                layer,
            }));
        }
        // A worn item pressed and let go without a move: put it back down.
        // Nothing was ever sent, so there is nothing to take back and nothing
        // new to draw.
        if self.pressed.take().is_some() {
            return Response::consumed();
        }
        let Some(held) = self.held.take() else {
            return Response::ignored();
        };
        let Some(Drawn::Paperdoll(window)) = ctx.drawn else {
            return Response::changed();
        };
        let on = gump_art::pick(
            &window.doll.pictures,
            ctx.frame.cursor,
            ctx.frame.files.gump_atlas,
        )
        .and_then(|index| window.doll.hits.get(&index).copied());
        if on != Some(held) {
            return Response::changed();
        }
        self.clicked(held, ctx)
    }

    /// What one completed click means: gather what the effects are about —
    /// whose doll, what stance, which bag — and ask [`button_effects`].
    fn clicked(&mut self, button: paperdoll::DollButton, ctx: &PaneCtx<'_>) -> Response {
        let own = self.own(&ctx.frame);
        let backpack = self
            .equipment(&ctx.frame)
            .iter()
            .find(|item| item.layer == Layer::BACKPACK)
            .map(|item| item.serial);
        let paired = match button {
            paperdoll::DollButton::Profile
            | paperdoll::DollButton::Party
            | paperdoll::DollButton::Virtue
            | paperdoll::DollButton::Backpack => self.scroll_paired(button, ctx.now),
            _ => false,
        };
        button_effects(
            button,
            self.mobile,
            own,
            ctx.frame.view.player.war,
            backpack,
            paired,
        )
    }

    /// The move that pulls a worn item off the body, and the hover beside it.
    ///
    /// A worn item is never a stack, so the Shift half of the rule cannot
    /// fire here and the answer is a lift or nothing — but it is the *same*
    /// rule, asked of the same [`ItemPress`], because a press is a press
    /// wherever it landed.
    fn moved(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let mut answer = self.hover(ctx);
        let Some(press) = self.pressed else {
            return answer;
        };
        if let Dragged::Lift(drag) = press.dragged(ctx.past_slop, ctx.modifiers.shift) {
            self.pressed = None;
            answer.absorb(Response::stale().with(Effect::Lift(drag)));
        }
        answer
    }

    /// Follow the pointer with the two answers the layout will read: which
    /// worn layer is under it, and whether a full hand is over this doll.
    ///
    /// Not `taken` — a move is not an exclusive event — and
    /// [`Response::stale`] only when an answer changed: this is the Backlog's
    /// "a picture that depends on the pointer owes a frame", paid by
    /// remembering what the picture was.
    fn hover(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let hovered = match (ctx.under_pointer, ctx.drawn) {
            (true, Some(Drawn::Paperdoll(window))) => gump_art::pick(
                &window.doll.pictures,
                ctx.frame.cursor,
                ctx.frame.files.gump_atlas,
            )
            .and_then(|index| window.doll.equipment_hits.get(&index).copied()),
            _ => None,
        };
        let hand_over = ctx.under_pointer && self.own(&ctx.frame) && ctx.frame.hand.is_some();
        if self.hovered == hovered && self.hand_over == hand_over {
            return Response::ignored();
        }
        self.hovered = hovered;
        self.hand_over = hand_over;
        Response::stale()
    }

    /// The text over the doll: the name on the plate, and the worn item the
    /// pointer is resting on, named at the cursor.
    ///
    /// The name comes out of the view — the `0x88` carries it and nothing
    /// else does — and the hover label out of `tiledata`, which is why both
    /// are resolved here rather than in the render crate's layout.
    fn lines(&self, frame: &PaneFrame<'_>) -> Vec<Line> {
        let mut lines = Vec::new();
        // `0x88` is the paperdoll title's usual authority.  At world entry,
        // though, `0x11` already contains the character name; retain that as a
        // fallback so a client never shows an anonymous own doll while a
        // lagged/older shard has not supplied the optional title string.
        let title = frame
            .view
            .paperdolls
            .get(&self.mobile)
            .map(|doll| doll.title.as_str())
            .filter(|title| !title.is_empty())
            .or_else(|| {
                self.own(frame)
                    .then(|| {
                        frame
                            .view
                            .player
                            .status
                            .as_ref()
                            .map(|status| status.name.as_str())
                    })
                    .flatten()
            });
        if let Some(title) = title {
            // Window-local — see `PaneFrame::cursor`'s doc.
            let name = paperdoll::title(title, GumpPixel::new(0, 0));
            lines.push(Line {
                at: name.at,
                font: name.font,
                hue: name.hue,
                clip: name.clip,
                text: name.text.to_owned(),
            });
        }
        if let Some(layer) = self.hovered {
            if let Some(item) = self.equipment(frame).iter().find(|item| item.layer == layer) {
                lines.push(Line {
                    // The same step down-right the container hover uses, so
                    // the two hovers read as one behaviour.
                    at: frame.cursor.offset(GumpPixel::new(14, 18)),
                    font: Font(1),
                    hue: Hue::LABEL,
                    clip: None,
                    text: frame.files.tiledata.static_tile(item.graphic.0).name.clone(),
                });
            }
        }
        lines
    }
}

impl PaperdollPane {
    /// Nothing of its own, and it could not be otherwise: which picture a
    /// worn item is, is the answer to [`paperdoll::gump_of`], so asking for
    /// the list *is* laying the window out — [`paperdoll::art_of`]'s own doc.
    /// The sweep over every laid-out window at the end of
    /// `render_passes::draw_gump_windows` packs what the layout produced.
    pub(super) fn art(&self, _frame: &PaneFrame<'_>) -> Vec<GumpArt> {
        Vec::new()
    }

    /// The frame, the body, every worn layer over it, and the text over the
    /// lot.
    ///
    /// `None` only for a client with no gump art at all, in which case no
    /// window is drawn anyway. A mobile the view has never revealed still
    /// draws the *frame* — the window is open, and a window with no picture
    /// is one the pointer cannot find and the player cannot close; the doll
    /// appears on the frame the `0x77` arrives.
    pub(super) fn layout(&self, frame: &PaneFrame<'_>) -> Option<Drawn> {
        let files = frame.files.gumps.as_ref()?;
        let view = frame.view;
        let own = self.own(frame);
        let body = match own {
            true => Some((view.player.body, view.player.hue)),
            false => view
                .mobiles
                .get(&self.mobile)
                .map(|mobile| (mobile.body, mobile.hue)),
        };
        // The `0x88` carries no equipment — see `WorldView::paperdolls` — so
        // it is read off the body the window names, resolved once through
        // tiledata for the sprite and the doll alike (`crowd::worn`). An item
        // the hand has lifted off this body is subtracted until the shard
        // answers, exactly as its bag subtracts a lifted icon.
        let equipment: Vec<EquipmentLayer> = crowd::worn(self.equipment(frame), frame.files.tiledata)
            .into_iter()
            .filter(|item| {
                frame.hand.is_none_or(|hand| {
                    hand.drag().origin
                        != DragOrigin::Equipment {
                            mobile: self.mobile,
                            layer: item.layer,
                        }
                })
            })
            .collect();
        let wearer = body.map(|(body, hue)| paperdoll::Wearer {
            body,
            hue,
            equipment: &equipment,
        });
        // The stance, off the player and not off the `0x88` the window opened
        // on: a `0x72` moves it while that packet stands still, and the
        // toggle is the one picture on the frame that has to follow.
        let whose = match own {
            true => paperdoll::Whose::Own { war: view.player.war },
            false => paperdoll::Whose::Another,
        };
        // The held wearable, projected onto our own doll while the hand is
        // over it. The item is the hand's, read now rather than remembered at
        // the move (see `hand_over`), and only a layer the body has empty
        // takes a preview.
        let preview = match self.hand_over {
            true => frame.hand.and_then(|hand| {
                let drag = hand.drag();
                let tile = frame.files.tiledata.static_tile(drag.item.graphic.0);
                let layer = classic_weapon_layer(drag.item.graphic, Layer(tile.layer));
                (own && layer.0 > 0
                    && layer.0 <= 25
                    && !equipment.iter().any(|worn| worn.layer == layer)
                    && !self.hands_conflict(frame, drag.item.graphic, layer))
                .then_some(EquipmentLayer {
                    graphic: tile.anim_id,
                    hue: drag.item.hue,
                    layer,
                })
            }),
            false => None,
        };
        let doll = paperdoll::window(
            wearer.as_ref(),
            whose,
            self.held,
            self.hovered,
            preview,
            frame.files.equip_conv,
            files,
            // Window-local — see `PaneFrame::cursor`'s doc.
            GumpPixel::new(0, 0),
        );
        let lines = self.lines(frame);
        Some(Drawn::Paperdoll(Window { doll, lines }))
    }

    /// A press on the window's furniture, the release that completes it, and
    /// the move that keeps the hover honest.
    ///
    /// The press is *located*, so it begins by asking whether this window is
    /// the one the pointer is on — see [`PaneCtx::under_pointer`]. The
    /// release is not: it finishes a press this pane started, wherever the
    /// pointer has got to. A move is offered to every window, which is what
    /// lets a tint be cleared on the doll the pointer just left.
    ///
    /// **No wheel.** Nothing on a doll scrolls, so a notch over one goes past
    /// it to the camera, exactly as it did. The right button is the manager's
    /// close, and there is nothing here to type into.
    pub(super) fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response {
        match input {
            Input::Press(Button::Left) => {
                if !ctx.under_pointer {
                    return Response::ignored();
                }
                // Never drawn, so there is nothing to hit-test against and no
                // pixels the player can have meant. The window is still
                // raised and moved by the manager's own path.
                let Some(Drawn::Paperdoll(window)) = ctx.drawn else {
                    return Response::ignored();
                };
                self.press(window, ctx)
            }
            Input::Release(Button::Left) => self.release(ctx),
            Input::Move => self.moved(ctx),
            // A worn item is one thing, so there is nothing here to divide and
            // this window never asks for the amount prompt — and an answer
            // reaches only the window that did.
            Input::Wheel(_)
            | Input::Press(Button::Right)
            | Input::Release(Button::Right)
            | Input::Key(_)
            | Input::Answered(_) => Response::ignored(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial(raw: u32) -> Serial {
        Serial::new(raw).expect("a mobile serial")
    }

    /// The pairing rule, minus the half the shape now answers: two clicks on
    /// two different dolls cannot pair because each pane keeps its own
    /// `last_scroll` — there is no cross-window state left to compare.
    #[test]
    fn two_clicks_on_one_scroll_are_a_pair_and_two_scrolls_are_not() {
        let first = Instant::now();
        let soon = first + DOUBLE_CLICK / 2;
        let profile = paperdoll::DollButton::Profile;

        assert!(
            !pairs(None, first, profile),
            "the first click of all has nothing to pair with"
        );
        assert!(pairs(Some((first, profile)), soon, profile));
        assert!(
            !pairs(Some((first, profile)), soon, paperdoll::DollButton::Party),
            "the manifest sits fourteen pixels along, and is not the same scroll"
        );
        assert!(
            !pairs(
                Some((first, profile)),
                first + DOUBLE_CLICK + std::time::Duration::from_millis(1),
                profile
            ),
            "and a click a breath too late is a first click"
        );
    }

    /// A completed pair disarms the state and a first click arms it, so three
    /// quick clicks are a pair and a single, not a pair and a half.
    #[test]
    fn a_completed_pair_starts_over() {
        let mut pane = PaperdollPane::new(serial(0x2A));
        let first = Instant::now();
        let step = DOUBLE_CLICK / 4;
        let virtue = paperdoll::DollButton::Virtue;

        assert!(!pane.scroll_paired(virtue, first), "armed");
        assert!(pane.scroll_paired(virtue, first + step), "paired");
        assert!(
            !pane.scroll_paired(virtue, first + step * 2),
            "the third click starts a new pair rather than riding the old one"
        );
    }

    #[test]
    fn a_worn_item_uses_only_its_own_double_click_pair() {
        let mut pane = PaperdollPane::new(serial(0x2A));
        let axe = serial(0x4000_0001);
        let dagger = serial(0x4000_0002);
        let first = Instant::now();
        let step = DOUBLE_CLICK / 4;

        assert!(!pane.item_paired(axe, first));
        assert!(pane.item_paired(axe, first + step));
        assert!(!pane.item_paired(dagger, first + step * 2));
        assert!(pane.item_paired(dagger, first + step * 3));
    }

    /// The buttons, as the packets and windows they ask for. WarMode toggles
    /// the stance it was drawn in, and the two window buttons ask the shard
    /// for fresh numbers *and* open the local window, in that order.
    #[test]
    fn a_button_asks_for_the_window_it_is_named_after() {
        let me = serial(0x2A);

        let answer = button_effects(paperdoll::DollButton::WarMode, me, true, false, None, false);
        assert!(matches!(
            answer.out.as_slice(),
            [Effect::Net(Outgoing::WarMode(true))]
        ));
        let answer = button_effects(paperdoll::DollButton::WarMode, me, true, true, None, false);
        assert!(
            matches!(answer.out.as_slice(), [Effect::Net(Outgoing::WarMode(false))]),
            "the toggle asks for the stance it is not in"
        );

        let answer = button_effects(paperdoll::DollButton::Skills, me, true, false, None, false);
        assert!(matches!(
            answer.out.as_slice(),
            [
                Effect::Net(Outgoing::Skills(who)),
                Effect::Open(LocalWindow::Skills)
            ] if *who == me
        ));
        let answer = button_effects(paperdoll::DollButton::Options, me, true, false, None, false);
        assert!(matches!(
            answer.out.as_slice(),
            [Effect::Open(LocalWindow::WorldMap)]
        ));
        let answer = button_effects(paperdoll::DollButton::Status, me, true, false, None, false);
        assert!(matches!(
            answer.out.as_slice(),
            [
                Effect::Net(Outgoing::Status(who)),
                Effect::Open(LocalWindow::Status)
            ] if *who == me
        ));

        let answer = button_effects(paperdoll::DollButton::LogOut, me, true, false, None, false);
        assert!(matches!(answer.out.as_slice(), [Effect::Net(Outgoing::LogOut)]));
        let answer = button_effects(paperdoll::DollButton::Quests, me, true, false, None, false);
        assert!(matches!(answer.out.as_slice(), [Effect::Net(Outgoing::QuestLog)]));
        let answer = button_effects(paperdoll::DollButton::Guild, me, true, false, None, false);
        assert!(matches!(
            answer.out.as_slice(),
            [Effect::Net(Outgoing::GuildMenu)]
        ));
    }

    /// A stranger's Status control asks for nothing at all: the packets it
    /// would send describe this connection's player, so both the request and
    /// the local window would be about the wrong body.
    #[test]
    fn a_strangers_status_button_neither_asks_nor_opens() {
        let stranger = serial(0xFF);
        let answer = button_effects(paperdoll::DollButton::Status, stranger, false, false, None, false);
        assert!(answer.redraw, "the button still comes back up");
        assert!(answer.out.is_empty(), "and nothing travels");
        let answer = button_effects(paperdoll::DollButton::Skills, stranger, false, false, None, false);
        assert!(answer.out.is_empty());
    }

    /// The scrolls act only on the second click of a pair: the first click
    /// asks for nothing, the paired one asks for the virtue menu or opens the
    /// worn backpack — and a body wearing no backpack has nothing to open.
    #[test]
    fn a_scroll_acts_on_the_paired_click_and_not_the_first() {
        let me = serial(0x2A);
        let pack = serial(0x4000_0001);

        let answer = button_effects(paperdoll::DollButton::Virtue, me, true, false, None, false);
        assert!(answer.out.is_empty(), "the first click only arms the pair");
        let answer = button_effects(paperdoll::DollButton::Virtue, me, true, false, None, true);
        assert!(matches!(
            answer.out.as_slice(),
            [Effect::Net(Outgoing::Virtue(who))] if *who == me
        ));

        let answer = button_effects(paperdoll::DollButton::Backpack, me, true, false, Some(pack), true);
        assert!(matches!(
            answer.out.as_slice(),
            [Effect::Net(Outgoing::Use(bag))] if *bag == pack
        ));

        let answer = button_effects(paperdoll::DollButton::Party, me, true, false, None, true);
        assert!(matches!(
            answer.out.as_slice(),
            [Effect::Reopen(crate::windows::WindowSubject::Party)]
        ));
        let answer = button_effects(paperdoll::DollButton::Backpack, me, true, false, None, true);
        assert!(answer.out.is_empty(), "no worn backpack, nothing to use");
    }

    /// **Step 8's paperdoll quarter, through [`AnyPane::handle`](crate::panes::AnyPane::handle).**
    ///
    /// S5 declined this press so the legacy chain could answer it; S6 is what
    /// moved the transfer machinery in, and the module docs above say what
    /// changed: "a worn item is pressed, dragged off the doll and lifted like
    /// an icon in a bag." So the press this pane owns today is not `ignored`
    /// at all — it is [`ContainerPane::press`](crate::panes::container)'s
    /// shape, one holder over: taken, held as [`PaperdollPane::pressed`], and
    /// turned into [`Effect::Lift`] by the move that follows, past the same
    /// slop [`ItemPress::dragged`] pins for every holder.
    ///
    /// The doll is built by hand rather than through [`PaperdollPane::layout`] — a real
    /// doll picture needs `gumpartLegacyMUL.uop`, which
    /// [`fixture::Install`] deliberately does not ship (see its own doc) —
    /// but `press` never asks how a picture got into `ctx.drawn`, only what is
    /// in it, so a hand-built `Doll` naming one worn layer exercises the same
    /// code a real frame would.
    #[test]
    fn a_press_on_our_own_worn_item_becomes_a_lift_through_handle() {
        use std::collections::BTreeMap;

        use openshard_client_render::gump::{GumpArt, PictureIndex};
        use openshard_protocol::wire::Graphic;

        use crate::panes::fixture;

        const SHIRT: Graphic = Graphic(0x1F03);
        const WORN_LAYER: Layer = Layer(5);

        let me = serial(0x0000_002A);
        let files = fixture::Install::shipping([(GumpArt::Item(SHIRT), (20, 20))]);
        let mut view = fixture::world(me);
        view.player.equipment.push(Equipment {
            serial: serial(0x4000_0001),
            graphic: SHIRT,
            layer: WORN_LAYER,
            hue: Hue::NONE,
        });

        let mut pane = PaperdollPane::new(me);
        let at = GumpPixel::new(50, 50);
        let doll = paperdoll::Doll {
            pictures: vec![openshard_client_render::gump::Picture::plain(
                GumpArt::Item(SHIRT),
                at,
            )],
            hits: BTreeMap::new(),
            equipment_hits: BTreeMap::from([(PictureIndex::new(0), WORN_LAYER)]),
        };
        let drawn = Drawn::Paperdoll(Window {
            doll,
            lines: Vec::new(),
        });

        // Inside the shirt's own picture, 20×20 at (50, 50).
        let cursor = at.offset(GumpPixel::new(5, 5));
        let ctx = files.ctx(&view, Some(&drawn), cursor, true);
        let press = pane.handle(Input::Press(Button::Left), &ctx);
        assert!(press.taken, "a press on our own window is always ours");
        assert!(
            matches!(press.out.as_slice(), [Effect::Raise]),
            "nothing has been sent yet — the transfer starts as a press, not a packet"
        );
        assert!(pane.pressed.is_some());

        // Past the slop, so the held press has become a drag — the manager's
        // answer, handed in, not measured against a position this press kept
        // (`PaneCtx::past_slop`).
        let dragged = cursor.offset(GumpPixel::new(10, 0));
        let mut ctx = files.ctx(&view, Some(&drawn), dragged, true);
        ctx.past_slop = true;
        let answer = pane.handle(Input::Move, &ctx);
        assert!(
            matches!(answer.out.as_slice(), [Effect::Lift(_)]),
            "the move past the slop is what turns the held press into a lift"
        );
        assert!(
            pane.pressed.is_none(),
            "the press is spent once it becomes a lift"
        );

        // The next click on the same equipped item is its ordinary `Use`, not
        // another drag. This is the path an equipped axe or dagger uses.
        let mut second = files.ctx(&view, Some(&drawn), cursor, true);
        second.now = ctx.now + DOUBLE_CLICK / 4;
        let used = pane.handle(Input::Press(Button::Left), &second);
        assert!(matches!(
            used.out.as_slice(),
            [Effect::Raise, Effect::Net(Outgoing::Use(item))]
                if *item == serial(0x4000_0001)
        ));
    }

    /// A closed backpack is still a container target on the paperdoll. This
    /// pins the resurrection report directly: the axe is already on the hand,
    /// the pack has no container window, and releasing over its drawn pixels
    /// must emit a container drop rather than trying to equip the axe again.
    #[test]
    fn a_drop_on_the_paperdoll_backpack_goes_into_the_closed_pack() {
        use std::collections::BTreeMap;

        use openshard_client_render::gump::{GumpArt, Picture, PictureIndex};
        use openshard_protocol::containers::GridSlot;
        use openshard_protocol::items::ItemAmount;
        use openshard_protocol::wire::Graphic;

        use crate::hand::{Hand, ItemDrag};
        use crate::panes::fixture;

        const BACKPACK: Graphic = Graphic(0x0E75);
        const AXE: Graphic = Graphic(0x0F43);

        let me = serial(0x0000_002A);
        let pack = serial(0x4000_0100);
        let axe = serial(0x4000_0200);
        let files = fixture::Install::shipping([(GumpArt::Item(BACKPACK), (30, 30))]);
        let mut view = fixture::world(me);
        view.player.equipment.push(Equipment {
            serial: pack,
            graphic: BACKPACK,
            layer: Layer::BACKPACK,
            hue: Hue::NONE,
        });

        let at = GumpPixel::new(90, 80);
        let doll = paperdoll::Doll {
            pictures: vec![Picture::plain(GumpArt::Item(BACKPACK), at)],
            hits: BTreeMap::from([(PictureIndex::new(0), paperdoll::DollButton::Backpack)]),
            equipment_hits: BTreeMap::new(),
        };
        let drawn = Drawn::Paperdoll(Window {
            doll,
            lines: Vec::new(),
        });
        let mut ctx = files.ctx(&view, Some(&drawn), at.offset(GumpPixel::new(5, 5)), true);
        ctx.frame.hand = Some(Hand::Held(ItemDrag {
            item: ContainedItem {
                serial: axe,
                graphic: AXE,
                amount: ItemAmount(1),
                at: GumpPoint::new(0, 0),
                grid: GridSlot(0),
                hue: Hue::NONE,
            },
            origin: DragOrigin::Ground,
            grab: GumpPixel::new(5, 5),
        }));

        let mut pane = PaperdollPane::new(me);
        let answer = pane.handle(Input::Release(Button::Left), &ctx);
        assert!(matches!(
            answer.out.as_slice(),
            [Effect::Drop(PendingDrop::Container { container, at })]
                if *container == pack && *at == GumpPoint::new(0, 0)
        ));
    }

    /// A bow's tiledata layer is one-handed, but the shard's weapon class
    /// rule makes it two-handed. The outgoing request must already use that
    /// corrected layer or the shard rejects the equip.
    #[test]
    fn a_bow_drop_requests_the_two_handed_layer() {
        use openshard_protocol::wire::Graphic;

        use crate::hand::{Hand, ItemDrag};
        use crate::panes::fixture;

        const BOW: Graphic = Graphic(0x13B2);

        let me = serial(0x0000_002A);
        let files = fixture::Install::shipping([]);
        let view = fixture::world(me);
        let mut ctx = files.ctx(&view, None, GumpPixel::new(50, 50), true);
        ctx.frame.hand = Some(Hand::Held(ItemDrag {
            item: ContainedItem {
                serial: serial(0x4000_0001),
                graphic: BOW,
                amount: ItemAmount::ONE,
                at: GumpPoint::new(0, 0),
                grid: GridSlot(0),
                hue: Hue::NONE,
            },
            origin: DragOrigin::Ground,
            grab: GumpPixel::new(0, 0),
        }));

        let mut pane = PaperdollPane::new(me);
        let answer = pane.handle(Input::Release(Button::Left), &ctx);
        assert!(matches!(
            answer.out.as_slice(),
            [Effect::Drop(PendingDrop::Equipment { mobile, layer })]
                if *mobile == me && *layer == Layer::TWO_HANDED
        ));
    }
}
