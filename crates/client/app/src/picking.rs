//! The live cursor answer and a persistent diagnostic selection: [`Picking`].
//!
//! [`Hover`] names only the object under the cursor in the last completed
//! frame. [`SelectedIdentity`] is a left click's independent, durable answer
//! for the selection panel. Neither is a fallback for the other.

use openshard_client_render::depth::Hit;
use openshard_client_render::items::ItemIndex;
use openshard_client_render::statics::PickedStatic;
use openshard_protocol::serial::Serial;
use openshard_protocol::world::Point;

use crate::crowd::Who;

/// What a left click named, kept as identity rather than as data — see
/// [`Picking::selected`] for why. [`crate::App::resolve_selection`] is the
/// only reader.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SelectedIdentity {
    /// Bare ground: nothing with its own identity was under the cursor.
    Tile { x: u16, y: u16 },
    /// The map's own furniture — never moves, so the pick itself is kept
    /// rather than just a reference to re-look-up.
    Static(PickedStatic),
    /// A creature, by [`Who`] — `None` for the player's own body.
    Mobile(Who),
    /// An item lying on the ground, by its serial.
    Item(Serial),
}

impl SelectedIdentity {
    /// The static half alone, for the two render passes that wash and mask
    /// it — [`openshard_client_render::select`] and `statics::selected`.
    /// `None` whenever a click landed on anything else, which is what
    /// switches both passes off.
    ///
    /// A free function on the value rather than a method on `App`: both call
    /// sites read `self.picking.selected` while `self.window` is already
    /// borrowed mutably, and a method taking `&self` would borrow the whole
    /// struct where a direct field read borrows only the one field.
    pub fn as_static(self) -> Option<PickedStatic> {
        match self {
            SelectedIdentity::Static(picked) => Some(picked),
            _ => None,
        }
    }

    /// The mobile half alone, for the held-selection ring — see
    /// `Screen::held_mask`.
    pub fn as_mobile(self) -> Option<Who> {
        match self {
            SelectedIdentity::Mobile(who) => Some(who),
            _ => None,
        }
    }

    /// The item half alone, for the held-selection ring.
    pub fn as_item(self) -> Option<Serial> {
        match self {
            SelectedIdentity::Item(serial) => Some(serial),
            _ => None,
        }
    }
}

/// A shard item under the cursor: which item, and where the picture that was
/// hit stands.
///
/// **The serial cannot name the place.** A house is one item and a hundred
/// pieces, and every piece carries the *house's* serial — see
/// `App::apply_items`, where the multi is expanded — so a serial looked up in
/// `presentation.items` finds whichever piece happens to be first in the list
/// rather than the storey the cursor was on. The place therefore travels with
/// the pick, taken from the very entry the hit test hit. See
/// [`crate::App::walk_destination`], which is what a click on an upper floor
/// turns into.
///
/// The serial half stays an identity, for [`Picking::selected`]'s reason: it is
/// what the shard is told, and it survives the next frame's rebuilt lists. The
/// place half is deliberately *not* an identity — it is a fact about the frame
/// that was drawn, exactly as [`PickedStatic::at`] is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct HoveredItem {
    /// What the shard knows it by — a house's serial, for any of its pieces.
    pub serial: Serial,
    /// Where the piece the cursor actually hit stands.
    pub at: Point,
}

/// What the cursor was on in the last completed frame.
///
/// A click happens between frames, so it must read this already-drawn picture
/// rather than ask a camera which may have moved since. Each answer is an
/// identity rather than a collector index and therefore survives the next
/// frame's rebuilt draw lists.
///
/// **At most one of the three is `Some`.** They are three separate hit tests
/// over three separate lists, and which of their answers the cursor is really
/// on is settled once, when the frame's facts are taken — the crowd by
/// precedence, the other two by [`in_front`]. Every reader here therefore reads
/// one answer and not a shortlist to re-rank.
#[derive(Default)]
pub struct Hover {
    /// Map furniture under the cursor, when no closer dynamic object won.
    ///
    /// A frame behind, and that is what makes it right rather than what it
    /// costs: a click arrives *between* frames, so the picture it is a click
    /// on is the one already drawn. Picking again at the click would ask a
    /// camera that has moved since — see the `MouseInput` arm, where this is
    /// read.
    ///
    /// It is also the tile marker's reason for going out: a wall under the
    /// cursor is what the click would take, so the diamond on the ground
    /// behind it must not be drawn as well. See
    /// [`diagnostics::Hud::hover_lit`](crate::diagnostics::Hud::hover_lit).
    pub static_: Option<PickedStatic>,
    /// A mobile under the cursor; `None` inside `Some` is the local player.
    pub mobile: Option<Who>,
    /// A shard item under the cursor, and where its picture stands.
    pub item: Option<HoveredItem>,
}

/// Which of the two world hit tests the frame drew in front, with the loser put
/// out.
///
/// **The crowd is not in here, and that is deliberate.** A creature is asked
/// for first and wins outright, because it stands *on* the clutter of its tile
/// and a player pointing at a shopkeeper standing on a rug means the
/// shopkeeper. Between the shard's items and the map's own furniture there is
/// no such argument to make, and the order the two lists happen to be asked in
/// is not one either: both go through `statics::place`, both write the
/// [`depth::Order`](openshard_client_render::depth::Order) they carry into one
/// depth buffer, and so the frame has *already* decided which of them the
/// player can see. This reads that decision back rather than inventing a
/// second one.
///
/// **`>=` gives a tie to the item**, because that is what the picture does with
/// it. The two lists are appended into one pass — the map's statics first, the
/// shard's items after (`render::frame::assemble`) — and the depth test is
/// `LessEqual`, so of two equal depths the *later* drawn keeps the pixel. See
/// `renderer::depth_state`, which settles every tie in this client that way,
/// and `items::pick`, which breaks its own list's ties with the same `>=` for
/// the same reason.
pub(crate) fn in_front(
    item: Option<Hit<ItemIndex>>,
    map_static: Option<Hit<PickedStatic>>,
) -> (Option<ItemIndex>, Option<PickedStatic>) {
    match (item, map_static) {
        (Some(item), Some(map_static)) => match item.order >= map_static.order {
            true => (Some(item.what), None),
            false => (None, Some(map_static.what)),
        },
        // One of them, or neither: nothing to compare, and the hit that exists
        // is the answer.
        (item, map_static) => (item.map(|hit| hit.what), map_static.map(|hit| hit.what)),
    }
}

/// The two independent pointer states — see the module docs.
#[derive(Default)]
pub struct Picking {
    /// The live, temporary cursor answer. Replaced after every drawn frame.
    pub hover: Hover,
    /// What a left click last landed on, kept by *identity* until the next
    /// click — a coordinate, a static's own graphic-and-place, or a
    /// creature's or item's serial. Never the data itself:
    /// [`crate::App::hud`] turns this into a
    /// [`shell::Selection`](crate::shell::Selection) fresh every frame, the
    /// same way [`crate::App::tile_info`] always re-reads the column rather
    /// than remembering one — so a selected mobile's row keeps up with it
    /// walking, and a selected item's row goes away the moment it is picked
    /// up, instead of the panel quietly lying about where either still is.
    pub selected: Option<SelectedIdentity>,
}

#[cfg(test)]
mod tests {
    use openshard_client_render::depth::Order;
    use openshard_protocol::wire::Graphic;

    use super::*;

    fn hovered(serial: Serial) -> HoveredItem {
        HoveredItem {
            serial,
            at: Point::new(1400, 1600, 0),
        }
    }

    #[test]
    fn a_new_hover_does_not_replace_the_diagnostic_selection() {
        let harp = Serial::new(0x4000_0001).expect("valid item serial");
        let drum = Serial::new(0x4000_0002).expect("valid item serial");
        let mut picking = Picking {
            hover: Hover {
                item: Some(hovered(harp)),
                ..Hover::default()
            },
            selected: Some(SelectedIdentity::Item(harp)),
        };

        picking.hover.item = Some(hovered(drum));

        assert_eq!(
            picking.hover.item.map(|item| item.serial),
            Some(drum),
            "the cursor moved to the drum"
        );
        assert_eq!(
            picking.selected,
            Some(SelectedIdentity::Item(harp)),
            "the inspector still holds the clicked harp"
        );
    }

    /// A hit on each list, at two orders: the nearer one is the answer and the
    /// further one is put out, whichever list it came from.
    #[test]
    fn the_nearer_of_the_two_hit_tests_is_the_one_the_cursor_is_on() {
        let floor = Hit {
            order: Order {
                tile: 3037,
                priority_z: 19,
            },
            what: ItemIndex::new(7),
        };
        let wall = PickedStatic {
            at: Point::new(1400, 1637, 0),
            graphic: Graphic(0x0006),
        };
        // One tile nearer the eye, which outranks any height on the tile behind
        // it — see `depth::Order`.
        let nearer_wall = Hit {
            order: Order {
                tile: 3038,
                priority_z: 1,
            },
            what: wall,
        };
        let further_wall = Hit {
            order: Order {
                tile: 3036,
                priority_z: 1,
            },
            what: wall,
        };

        assert_eq!(
            in_front(Some(floor), Some(nearer_wall)),
            (None, Some(wall)),
            "the wall drawn in front of the floor did not take the cursor"
        );
        assert_eq!(
            in_front(Some(floor), Some(further_wall)),
            (Some(ItemIndex::new(7)), None),
            "a wall the frame drew behind the floor took the cursor anyway"
        );
    }

    /// Two pictures at one depth: the item wins, because the item is the one the
    /// pass draws second and the depth test is `LessEqual`.
    #[test]
    fn an_item_takes_a_tie_with_a_map_static() {
        let order = Order {
            tile: 3037,
            priority_z: 4,
        };
        let (item, map_static) = in_front(
            Some(Hit {
                order,
                what: ItemIndex::new(0),
            }),
            Some(Hit {
                order,
                what: PickedStatic {
                    at: Point::new(1400, 1637, 4),
                    graphic: Graphic(0x0006),
                },
            }),
        );

        assert_eq!(item, Some(ItemIndex::new(0)), "the tie went to the map's static");
        assert_eq!(map_static, None, "both lists answered one cursor");
    }

    /// One hit and no other is not a comparison, and must not be turned into
    /// one: this is the ordinary case — a wall with nothing lying against it,
    /// or a house floor over ground the map has no static on.
    #[test]
    fn a_lone_hit_is_the_answer() {
        let item = Hit {
            order: Order {
                tile: 3037,
                priority_z: 19,
            },
            what: ItemIndex::new(2),
        };

        assert_eq!(in_front(Some(item), None), (Some(ItemIndex::new(2)), None));
        assert_eq!(in_front(None, None), (None, None));
    }
}
