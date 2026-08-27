//! Which creature a mount item stands for, and which item a rideable creature is
//! drawn as.
//!
//! One table, read from both ends of the wire, because a ride is the one thing
//! the two ends have to name differently and mean the same by. The shard equips
//! the rider with an *item* on [`Layer::MOUNT`](crate::wire::Layer::MOUNT) — the
//! creature itself leaves the world while it is ridden — and the client, handed
//! that item, has to draw the *creature* underneath its rider. Neither half is
//! derivable from the other, and both are here.
//!
//! # Why the client cannot read this out of `tiledata.mul`
//!
//! A worn item's picture is ordinarily its tiledata `AnimID`, and a mount item
//! looks like it should be no different. It is: on a stock 7.0 install the mount
//! block's entries are leftovers — `0x3E9F`, the ordinary horse, is named "ship"
//! and carries `AnimID` 820, and `anim.idx` holds no animation for body 820 at
//! all (its lookup word is `0xFFFFFFFF`). A client that trusts the file draws
//! nothing under the rider, which is exactly how this was found.
//!
//! ClassicUO carries the same table for the same reason
//! (`Game/Data/Mounts.cs`), and the rows here are ServUO's `BaseMount`
//! subclasses — the `base(name, bodyID, itemID, …)` each one passes, plus the
//! alternating body/item arrays a class that rolls between several looks keeps
//! (`Horse` is one of four).

use crate::wire::Graphic;

/// The mount-item graphic each rideable body is drawn as, sorted by body id.
///
/// Both directions of the mapping are derived from this one table: two
/// hand-kept halves is how a saved ride comes back as the wrong animal.
const MOUNTS: &[(u16, u16)] = &[
    (0x0074, 0x3EA7),
    (0x0075, 0x3EA8),
    (0x007A, 0x3EB4),
    (0x0084, 0x3EAD),
    (0x0090, 0x3EB3),
    (0x00A9, 0x3E95),
    (0x00BB, 0x3EBA),
    (0x00BC, 0x3EB8),
    (0x00BE, 0x3E9E),
    (0x00C8, 0x3E9F),
    (0x00CC, 0x3EA2),
    (0x00D2, 0x3EA3),
    (0x00DA, 0x3EA4),
    (0x00DB, 0x3EA5),
    (0x00DC, 0x3EA6),
    (0x00E2, 0x3EA0),
    (0x00E4, 0x3EA1),
    (0x00F3, 0x3E94),
    (0x0114, 0x3E90),
    (0x0115, 0x3E91),
    (0x0317, 0x3EBC),
    (0x0319, 0x3EBB),
    (0x031A, 0x3EBD),
    (0x031F, 0x3EBE),
    (0x057F, 0x3ECB),
    (0x0580, 0x3ECD),
    (0x0582, 0x3ECC),
    (0x05A0, 0x3ECF),
    (0x05A1, 0x3ED0),
    (0x05E6, 0x3ED1),
];

/// The item graphic that draws a body as a mount on a rider, for the bodies that
/// can be ridden at all. `None` is "not rideable", which is what the shard's
/// double-click checks first.
///
/// A binary search over the sorted table, so it is cheap enough for the tick
/// paths that ask it about every creature in range.
#[must_use]
pub fn mount_item_for(body: Graphic) -> Option<Graphic> {
    MOUNTS
        .binary_search_by_key(&body.0, |&(id, _)| id)
        .ok()
        .map(|index| Graphic(MOUNTS[index].1))
}

/// The creature body a mount-item graphic stands for — the inverse of
/// [`mount_item_for`]. `None` is "not a mount item".
///
/// Two callers, both of which would otherwise guess. Persistence saves the worn
/// mount item and not the ridden creature (which lives only while ridden), so
/// restoring a saved ride rebuilds the creature from the item it was drawn as.
/// And the client draws that same creature under its rider every frame — see
/// this module's own note on why the file cannot tell it.
///
/// A linear scan: the table is sorted by body, not by item, and thirty rows once
/// per equip is not worth a second sorted copy.
#[must_use]
pub fn mount_body_for(item: Graphic) -> Option<Graphic> {
    MOUNTS
        .iter()
        .find(|&&(_, graphic)| graphic == item.0)
        .map(|&(body, _)| Graphic(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_by_body_and_lists_no_body_twice() {
        // `mount_item_for` binary-searches it, and a row appended out of order
        // makes that search miss a mount that is right there in the table.
        for pair in MOUNTS.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "body {:#06x} is out of order or ridden twice",
                pair[1].0
            );
        }
    }

    #[test]
    fn a_horse_is_drawn_as_its_saddle_and_the_saddle_back_as_the_horse() {
        // The five ordinary rides, both ways round. `0x3E9F` is the one the
        // reference client's own table omits — it falls back to the tiledata
        // `AnimID` for it, which on a stock install names an animation that does
        // not exist.
        for (body, item) in [
            (0x00C8, 0x3E9F),
            (0x00CC, 0x3EA2),
            (0x00E2, 0x3EA0),
            (0x00E4, 0x3EA1),
            (0x00DC, 0x3EA6),
        ] {
            assert_eq!(
                mount_item_for(Graphic(body)),
                Some(Graphic(item)),
                "body {body:#06x}"
            );
            assert_eq!(
                mount_body_for(Graphic(item)),
                Some(Graphic(body)),
                "item {item:#06x}"
            );
        }
    }

    #[test]
    fn a_person_is_not_a_mount_and_a_shirt_is_not_a_saddle() {
        assert_eq!(mount_item_for(Graphic(0x0190)), None);
        assert_eq!(mount_body_for(Graphic(0x1517)), None);
    }

    #[test]
    fn no_two_mounts_share_one_item_graphic() {
        // `mount_body_for` is the inverse of one table, and an inverse only
        // exists if the mapping is one to one — otherwise a saved ride comes
        // back as whichever animal the search happened to reach first.
        let mut items: Vec<u16> = MOUNTS.iter().map(|&(_, item)| item).collect();
        items.sort_unstable();
        let before = items.len();
        items.dedup();
        assert_eq!(before, items.len(), "a mount item graphic is used twice");
    }

    #[test]
    fn every_row_round_trips() {
        // The property the two halves exist for: neither direction may know a
        // pair the other does not.
        for &(body, item) in MOUNTS {
            assert_eq!(mount_body_for(Graphic(item)), Some(Graphic(body)));
            assert_eq!(mount_item_for(Graphic(body)), Some(Graphic(item)));
        }
    }
}
