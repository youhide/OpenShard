//! What the shard lays on a facet that is not terrain, and not alive.
//!
//! The statics a town needs beyond the map's own art — signs, furniture, the
//! rugs and crates that make a room a room — plus the doors that open, the
//! containers that hold something, and the boxes [`doorgen`](crate::doorgen)
//! scans for the shop doors a building's frames only imply.
//!
//! It is `data/deco.json`, compiled by `build.rs`, and it is the largest dataset
//! the tree ships: 18,832 statics, 5,598 containers, 638 doors and 14 scan boxes
//! for one facet.
//!
//! # Static, where the other datasets are owned
//!
//! Quests, speech, regions and spawns are each *replaced wholesale* in something
//! that owns them, so each `shipped` builds fresh values. Decoration is read once
//! and copied into a [`Command::Decorate`](crate::Command), so it stays `const`
//! and the copy happens at the one call site. Twenty-five thousand rows is the
//! wrong size to allocate twice.
//!
//! # Laying it twice
//!
//! Decoration is the dataset with no natural idempotency. A region set replaces;
//! a spawner de-duplicates by its box. Decoration is *additive* and persisted, so
//! a second press of the staff button used to lay a second copy of Britain on top
//! of the first. `tick::decor` answers that now — against the world rather than
//! against this file, because the file legitimately repeats itself.

use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::{Facet, Point};

use crate::{DecorContainer, DecorDoor};

/// One facet's decoration, and the admin verb that lays it.
///
/// The shape [`SpawnSet`](crate::spawner::SpawnSet) and
/// [`RegionSet`](openshard_state::region::RegionSet) established, with one
/// difference: the four payloads are borrowed rather than owned. See the module
/// header.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecorSet {
    /// What the staff menu's button sends: `decorate:felucca`.
    pub verb: &'static str,
    /// Which facet all of it belongs to.
    pub facet: Facet,
    /// The plain statics.
    pub statics: &'static [(Graphic, Hue, Point)],
    /// The doors that open.
    pub doors: &'static [DecorDoor],
    /// The containers that hold something.
    pub containers: &'static [DecorContainer],
    /// The boxes `doorgen` scans, as `(x, y, width, height)`.
    pub door_regions: &'static [(u16, u16, u16, u16)],
}

include!(concat!(env!("OUT_DIR"), "/deco.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_decoration_is_whole() {
        let sets = shipped();
        assert_eq!(sets.len(), 1, "one facet is decorated");
        let set = sets[0];
        // 18,832 flattened rows, with 140 multi-tile addon roots replaced by
        // their 391 component tiles.  This is the broad guard beside the oven
        // witness below: a future import cannot quietly collapse all addons
        // back to their root rows.
        assert_eq!(set.statics.len(), 19_083, "{} statics", set.statics.len());
        assert!(!set.doors.is_empty() && !set.containers.is_empty());
        assert!(
            !set.door_regions.is_empty(),
            "no box for doorgen to scan, so no shop door is ever generated"
        );
    }

    #[test]
    fn every_door_opens_into_the_graphic_beside_it() {
        // Derived rather than stored, so this is really a test of the derivation:
        // a door family is a run of leaves, each followed by its opened twin.
        for door in shipped()[0].doors {
            assert_eq!(
                door.open.0,
                door.closed.0 + 1,
                "the door at {} does not open into the graphic beside it",
                door.position
            );
        }
    }

    #[test]
    fn east_stone_ovens_keep_both_addon_components() {
        // `StoneOvenEastAddon` is not a single static: ServUO places its root
        // `0x092C` plus `0x092B` immediately south.  The decoration converter
        // originally retained the graphic on the .cfg type line but not this
        // second component, leaving every east-facing oven as a one-tile prop.
        // `deco_addons.json` now carries that component layout; keep this
        // concrete original report as the regression witness.
        let statics = shipped()[0].statics;
        let roots: Vec<_> = statics
            .iter()
            .copied()
            .filter(|&(graphic, _, _)| graphic == Graphic(0x092C))
            .collect();
        assert_eq!(roots.len(), 19, "unexpected east stone-oven population");
        for (_, _, at) in roots {
            assert!(
                statics.contains(&(Graphic(0x092B), Hue(0), Point::new(at.x, at.y + 1, at.z))),
                "the east stone oven rooted at {at} lost its south component"
            );
        }
    }

    #[test]
    fn a_door_graphic_hangs_only_one_way() {
        // The invariant the `door_hinges` table stands on, asserted again on the
        // far side of the build so that it is a fact about what the world receives
        // and not only about what the file says. `build.rs` cannot express it once
        // it has expanded the table, and this is where it would break: someone
        // giving one graphic two entries by hand-editing the generated source, or
        // a future dataset merged in from another facet.
        use std::collections::HashMap;
        let mut by_graphic: HashMap<u16, (i16, i16)> = HashMap::new();
        for door in shipped()[0].doors {
            let hinge = (door.offset_x, door.offset_y);
            if let Some(first) = by_graphic.insert(door.closed.0, hinge) {
                assert_eq!(
                    first, hinge,
                    "door graphic {:#06x} hangs two different ways",
                    door.closed.0
                );
            }
        }
        assert!(by_graphic.len() > 50, "only {} door graphics", by_graphic.len());
    }
}
