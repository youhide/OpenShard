//! The live cursor answer and a persistent diagnostic selection: [`Picking`].
//!
//! [`Hover`] names only the object under the cursor in the last completed
//! frame. [`SelectedIdentity`] is a left click's independent, durable answer
//! for the selection panel. Neither is a fallback for the other.

use openshard_client_render::statics::PickedStatic;
use openshard_protocol::serial::Serial;

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

/// What the cursor was on in the last completed frame.
///
/// A click happens between frames, so it must read this already-drawn picture
/// rather than ask a camera which may have moved since. Each answer is an
/// identity rather than a collector index and therefore survives the next
/// frame's rebuilt draw lists.
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
    /// A shard item under the cursor.
    pub item: Option<Serial>,
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
    use super::*;

    #[test]
    fn a_new_hover_does_not_replace_the_diagnostic_selection() {
        let harp = Serial::new(0x4000_0001).expect("valid item serial");
        let drum = Serial::new(0x4000_0002).expect("valid item serial");
        let mut picking = Picking {
            hover: Hover {
                item: Some(harp),
                ..Hover::default()
            },
            selected: Some(SelectedIdentity::Item(harp)),
        };

        picking.hover.item = Some(drum);

        assert_eq!(picking.hover.item, Some(drum), "the cursor moved to the drum");
        assert_eq!(
            picking.selected,
            Some(SelectedIdentity::Item(harp)),
            "the inspector still holds the clicked harp"
        );
    }
}
