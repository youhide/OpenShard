//! The hand: what a press on an item becomes, wherever the press landed.
//!
//! An item lying on the ground is pressed exactly the way an icon in a bag is
//! or a worn item is — one type for the press
//! ([`ItemPress`]), one rule for what it turns into
//! ([`ItemPress::dragged`]), and one slot for what ends up on the cursor
//! ([`Hand`]). None of that is about a *window*: a press held by a bag's pane
//! or a doll's pane is this module's [`ItemPress`] all the same, and the one
//! holder that is not a pane at all —
//! [`Windows::world_press`](crate::windows::Windows::world_press) — is that
//! same type again, because the ground has no pane to keep it in.
//!
//! This module is the states an item passes through between a press and a
//! settled transfer: [`ItemPress`] (not sent anywhere yet), [`Dragged`] (what
//! a press has become once the pointer has moved), [`DragOrigin`] (where a
//! held item was taken from), [`PendingDrop`] (where it has been put while
//! the shard decides), [`ItemDrag`] (what is on the cursor) and [`Hand`]
//! itself (held, or dropped and waiting). It is not the *manager* for any of
//! them: `docs/window_components.md`'s D2 and D7 still decide who is allowed
//! to write `Windows::hand`, `Windows::grip` and `Windows::world_press`,
//! and that stays [`crate::windows::Windows`] — this module only names what
//! those fields hold and the one rule for turning a press into a drag.

use openshard_client_render::gump::GumpPixel;
use openshard_protocol::containers::ContainedItem;
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::serial::Serial;
use openshard_protocol::world::Point;

/// A press on an item which becomes a drag only after the pointer actually
/// moves. Keeping it as an explicit state lets a normal click still
/// participate in the item's double-click "use" gesture.
///
/// **Held by whoever the press landed on**, which is decision 7's first half:
/// a press on an icon is `panes::container::ContainerPane`'s, a press on a worn
/// item is `panes::paperdoll::PaperdollPane`'s, and a press on the ground is
/// [`Windows::world_press`](crate::windows::Windows::world_press) because the
/// world has no pane. Nothing has been sent while one of these is alive —
/// that is what makes it private to its holder — and what it *becomes* is one
/// rule for all three: [`ItemPress::dragged`].
#[derive(Clone, Copy, Debug)]
pub struct ItemPress {
    pub item:   ContainedItem,
    /// The authoritative place the item is currently projected from.
    pub origin: DragOrigin,
    pub grab:   GumpPixel,
}

/// How far the pointer may wander before a press stops being a click.
///
/// Three pixels, so that the hand shaking on a double click does not lift the
/// item out from under the second one.
///
/// **Screen pixels — the ones the player's hand moves in**, and that is a
/// decision rather than an accident of where the number is read. The slop is a
/// *gesture* threshold and belongs with [`DOUBLE_CLICK`](crate::DOUBLE_CLICK):
/// both of them describe the hand and the mouse, not the picture underneath,
/// so neither may change when a window is drawn bigger. `SPLIT_OFFSET` is
/// magnified because it is half the picker's *own art*, and the cascade
/// constants are not because they are screen placement — this is the third
/// kind of constant in that argument, and it answers the way the second does.
/// See `docs/window_components.md`'s Backlog entry of this name.
const DRAG_SLOP: i32 = 3;

/// Whether the pointer has travelled far enough from where the button went
/// down for the press to stop being a click.
///
/// **Both positions absolute, and there is nowhere to put a scale.** This is
/// [`crate::windows::WindowGrip`]'s argument one device over: the pointer that
/// went down and the pointer now are read from the same field
/// ([`Input::pointer_gump`](crate::input::Input::pointer_gump)), so the
/// question "how far has the hand moved" is asked in one space and cannot be
/// asked in two. It used to be asked against
/// [`ItemPress`]'s own remembered position, which each of the three holders
/// stored in whichever pixels *it* counted in — a bag's pane and a doll's pane
/// in window-local ones, divided by [`WindowScale`](crate::desk::WindowScale),
/// the manager in surface ones — so an icon in a bag drawn at three times the
/// art needed three times the hand movement of one lying on the ground.
pub fn past_slop(down_at: GumpPixel, cursor: GumpPixel) -> bool {
    (cursor.x - down_at.x).abs() > DRAG_SLOP || (cursor.y - down_at.y).abs() > DRAG_SLOP
}

/// Where the pointer takes hold of an item that has no gump position of its
/// own.
///
/// A worn item and an item lying in the world are both drawn as something
/// other than their icon — a paperdoll layer, a ground sprite — so there is no
/// "this far into the picture" to remember. The icon's own centre goes under
/// the pointer instead, and that same offset is what a drop into a bag is
/// measured by, so the picture does not jump when it lands.
///
/// Zero for art this install does not ship, which draws the icon from its
/// corner: a missing graphic is not a reason to refuse the drag.
pub fn centre_of(graphic: openshard_protocol::wire::Graphic, art: &openshard_uofiles::art::Art) -> GumpPixel {
    art.static_art(graphic)
        .ok()
        .flatten()
        .map(|art| GumpPixel::new(i32::from(art.width()) / 2, i32::from(art.height()) / 2))
        .unwrap_or_default()
}

/// What a press becomes once the pointer has moved off it.
///
/// One rule with three holders — a bag's pane, a doll's pane, and the manager
/// for the world's own press — so the answer is a value they each act on rather
/// than three copies of the same `if`. Each turns it into what it can: a pane
/// into [`Effect`](crate::panes::Effect)s, the manager into its own writes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dragged {
    /// Not far enough yet. The press is still a click, and a second one within
    /// [`DOUBLE_CLICK`](crate::DOUBLE_CLICK) is still a use.
    Still,
    /// Shift, and a stack worth dividing: the client's own amount prompt goes
    /// up and the press waits for its answer. The number is the most that can
    /// be taken — the whole pile less the one that stays behind.
    Ask(u16),
    /// The lift itself: this is what goes onto the cursor.
    Lift(ItemDrag),
}

impl ItemPress {
    /// What this press has become, now that the pointer has moved.
    ///
    /// `past_slop` is the manager's answer and not this press's own — see
    /// [`past_slop`], and [`PaneCtx::past_slop`](crate::panes::PaneCtx::past_slop)
    /// for the field a pane reads it out of. A press holds no position at all,
    /// which is what leaves no space for its holder to measure the travel in
    /// the wrong one.
    ///
    /// Shift is asked *after* the slop, in that order and not the other way
    /// round: a Shift-click that never moved is still a click, and putting the
    /// prompt in front of the slop would open one on every press of a stack.
    pub fn dragged(self, past_slop: bool, shift: bool) -> Dragged {
        if !past_slop {
            return Dragged::Still;
        }
        if shift && self.item.amount.0 > 1 {
            return Dragged::Ask(self.item.amount.0 - 1);
        }
        Dragged::Lift(ItemDrag {
            item:   self.item,
            origin: self.origin,
            grab:   self.grab,
        })
    }

    /// The part of this press the player chose to take, or `None` for a stack
    /// that cannot be divided at all.
    ///
    /// Clamped into `1..=total - 1`: taking none of a pile is not a split and
    /// taking all of it is a lift, and the prompt's own bounds are the player's
    /// rather than a promise — the pile can have changed since it went up.
    pub fn split(self, amount: u16) -> Option<ItemDrag> {
        let total = self.item.amount.0;
        let amount = (total > 1).then(|| amount.clamp(1, total - 1))?;
        Some(ItemDrag {
            item:   ContainedItem {
                amount: openshard_protocol::items::ItemAmount(amount),
                ..self.item
            },
            origin: self.origin,
            grab:   self.grab,
        })
    }
}

/// The source removed by a drag transaction. Rendering is a projection of the
/// authoritative view with this source subtracted until the server confirms a
/// destination or cancels the transaction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DragOrigin {
    Ground,
    Container(Serial),
    Equipment {
        mobile: Serial,
        layer:  openshard_protocol::wire::Layer,
    },
}

/// Where a held item has been put, and the projection that draws it there
/// while the authoritative answer is in flight.
///
/// It is also *what a pane asks for* — see
/// [`Effect::Drop`](crate::panes::Effect::Drop) — because the three places an
/// item can be put down are three packets and nothing else: a window says
/// where, and [`PendingDrop::packet`] is the only translation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PendingDrop {
    Container {
        container: Serial,
        at:        GumpPoint,
    },
    Ground(Point),
    Equipment {
        mobile: Serial,
        layer:  openshard_protocol::wire::Layer,
    },
}

impl PendingDrop {
    /// The packet that puts `item` here.
    ///
    /// One place, so that a fourth kind of destination is a compile error here
    /// rather than a `match` somebody forgot in the router. Equipping is a
    /// `0x13` and the other two are `0x08` with different coordinates — the
    /// wire's own distinction, not this client's.
    pub fn packet(self, item: Serial) -> openshard_client_net::action::Outgoing {
        use openshard_client_net::action::Outgoing;
        match self {
            Self::Container { container, at } => Outgoing::DropInto { item, container, at },
            Self::Ground(at) => Outgoing::DropOnGround { item, at },
            Self::Equipment { mobile, layer } => {
                Outgoing::Equip {
                    item,
                    layer: openshard_protocol::wire::RawLayer(layer.0),
                    mobile,
                }
            }
        }
    }
}

/// The item the client has asked the shard to put on its cursor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemDrag {
    pub item:   ContainedItem,
    pub origin: DragOrigin,
    /// Offset from the item's top-left corner where the pointer grabbed it.
    pub grab:   GumpPixel,
}

/// What is on the cursor, and where it is on its way to.
///
/// **The client's mirror of the shard's own slot** — `Connection::held`, one
/// per connection, because a cursor holds one thing — which is the whole of
/// decision 7 in `docs/window_components.md`. The press that may *become* one
/// of these is not here: it belongs to whichever pane the press landed on (see
/// [`ItemPress`]), because nothing has been sent while a press is only a press,
/// and a lift is what puts the shard and this end into the same state.
///
/// Two states rather than three for that reason, and the pair is not a
/// gesture: [`Held`](Hand::Held) can outlive the window the item came out of,
/// survive a walk across the map, and be put down in a bag that was not open
/// when it was picked up.
#[derive(Clone, Copy, Debug)]
pub enum Hand {
    /// The shard has been asked for it and has not refused.
    Held(ItemDrag),
    /// It has been put somewhere and the answer is still in flight. The source
    /// stays subtracted and the destination is drawn until a packet settles it
    /// — see `App::apply_packet`.
    Dropped {
        drag:        ItemDrag,
        destination: PendingDrop,
    },
}

impl Hand {
    /// What is on the cursor. Not an `Option` any more, and that is the point:
    /// there used to be a third state in this enum that held nothing, so every
    /// reader had to ask twice — `owns_cursor` beside `drag` — and the answer
    /// to "is the hand full" was a method rather than the field being `Some`.
    pub const fn drag(self) -> ItemDrag {
        match self {
            Self::Held(drag) | Self::Dropped { drag, .. } => drag,
        }
    }

    /// Where it has been put, while the shard is still deciding.
    pub const fn pending_drop(self) -> Option<PendingDrop> {
        match self {
            Self::Dropped { destination, .. } => Some(destination),
            Self::Held(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::containers::GridSlot;
    use openshard_protocol::items::ItemAmount;
    use openshard_protocol::wire::{
        Graphic,
        Hue,
    };

    use super::*;

    fn press(amount: u16) -> ItemPress {
        ItemPress {
            item:   ContainedItem {
                serial:  Serial::new(0x4000_0001).expect("an item serial"),
                graphic: Graphic(0x0EED),
                amount:  ItemAmount(amount),
                at:      GumpPoint::new(0, 0),
                grid:    GridSlot(0),
                hue:     Hue::NONE,
            },
            origin: DragOrigin::Container(Serial::new(0x4000_0100).expect("a bag serial")),
            grab:   GumpPixel::new(4, 4),
        }
    }

    /// The slop is what keeps a double click from lifting the item out from
    /// under its own second half: a hand that shakes three pixels is still
    /// clicking.
    ///
    /// A square and not a circle, which the first case pins: three pixels in
    /// both directions at once is a diagonal further than three and is still
    /// inside the slop.
    #[test]
    fn the_pointer_has_to_leave_the_press_before_it_stops_being_a_click() {
        let down = GumpPixel::new(100, 100);
        assert!(!past_slop(down, GumpPixel::new(103, 97)));
        assert!(!past_slop(down, down));
        assert!(past_slop(down, GumpPixel::new(104, 100)));
        assert!(past_slop(down, GumpPixel::new(100, 96)));
    }

    /// A press remembers no position, so there is no scale to apply to one:
    /// the travel is measured by the manager, in the pixels the hand moves
    /// in, and handed to every holder as the same answer.
    ///
    /// This is the whole of `docs/window_components.md`'s "`DRAG_SLOP` is
    /// measured in whichever pixels its holder counts in" — an icon in a
    /// magnified bag and one lying on the ground now lift after the same
    /// movement of the mouse, because neither of them is what is being
    /// measured.
    #[test]
    fn a_press_becomes_a_lift_on_the_answer_it_is_handed() {
        assert_eq!(press(1).dragged(false, false), Dragged::Still);
        assert!(matches!(press(1).dragged(true, false), Dragged::Lift(_)));
    }

    /// Shift divides a pile and nothing else: a single item has nothing to
    /// divide, and a Shift-press that never moved is still a click.
    #[test]
    fn shift_asks_for_an_amount_only_when_there_is_a_pile_to_divide() {
        assert_eq!(
            press(20).dragged(true, true),
            Dragged::Ask(19),
            "the most that can be taken is the pile less the one left behind"
        );
        assert!(matches!(press(1).dragged(true, true), Dragged::Lift(_)));
        assert_eq!(
            press(20).dragged(false, true),
            Dragged::Still,
            "the slop is asked first, so a Shift-click is a click"
        );
    }

    /// Taking none of a pile is not a split and taking all of it is a lift, so
    /// the answer is clamped between them — and a single item cannot be
    /// divided at all.
    #[test]
    fn a_split_never_takes_none_or_the_whole_stack() {
        let amount = |press: &ItemPress, want| press.split(want).map(|drag| drag.item.amount.0);
        let pile = press(10);
        assert_eq!(amount(&pile, 0), Some(1));
        assert_eq!(amount(&pile, 4), Some(4));
        assert_eq!(amount(&pile, 10), Some(9));
        assert_eq!(amount(&press(1), 1), None);
    }

    /// A split keeps everything about the press but the number: the same
    /// serial, the same source and the same grip.
    #[test]
    fn a_split_carries_its_press_forward() {
        let pile = press(10);
        let drag = pile.split(3).expect("a pile of ten divides");
        assert_eq!(drag.item.serial, pile.item.serial);
        assert_eq!(drag.origin, pile.origin);
        assert_eq!(drag.grab, pile.grab);
    }

    /// Where an item is put down decides which packet says so, and there is
    /// one place that decides it.
    #[test]
    fn a_destination_names_its_own_packet() {
        use openshard_client_net::action::Outgoing;
        let item = Serial::new(0x4000_0001).expect("an item serial");
        let bag = Serial::new(0x4000_0100).expect("a bag serial");
        let me = Serial::new(0x0000_002A).expect("a mobile serial");

        assert!(matches!(
            PendingDrop::Container {
                container: bag,
                at: GumpPoint::new(7, 9),
            }
            .packet(item),
            Outgoing::DropInto { container, .. } if container == bag
        ));
        assert!(matches!(
            PendingDrop::Ground(Point::new(1, 2, 3)).packet(item),
            Outgoing::DropOnGround { .. }
        ));
        assert!(matches!(
            PendingDrop::Equipment {
                mobile: me,
                layer: openshard_protocol::wire::Layer(5),
            }
            .packet(item),
            Outgoing::Equip { mobile, layer, .. } if mobile == me && layer.0 == 5
        ));
    }
}
