//! The named people a town is made of, placed once and standing where they were
//! put.
//!
//! Bankers, shopkeepers, guildmasters and the travellers waiting for an escort —
//! everything a [`spawner`](crate::spawner) does *not* maintain. A spawn region
//! keeps a patch of wilderness full; these are individuals with a trade, a
//! doorway, and a shelf.
//!
//! It is `data/townsfolk.json`, compiled by `build.rs`.
//!
//! # One row is one person, whole
//!
//! This is the file that retired a join. The same content used to be three
//! tables in the script pack — a placement, a shelf, an escort destination —
//! keyed to each other by the **tile the NPC stands on**, because nothing outside
//! the world could name a mobile until the world had answered with its serial. A
//! script placed an NPC, waited for `MobileSpawned`, looked the tile up in two
//! maps, and sent two more commands.
//!
//! In-tree content is handed to the world as one [`Command::SpawnMobile`], which
//! carries the stock and the escort destination with it, and the world applies
//! all three the moment the mobile exists. There is no rendezvous, no tile key,
//! and nothing to get out of step. What made the tile a *usable* key in the first
//! place — that no two townsfolk share one — is checked in `build.rs` and is now
//! only a sanity check rather than a load-bearing property.

use crate::Command;

/// One facet's placed townsfolk, and the admin verb that places them.
///
/// The shape [`SpawnSet`](crate::spawner::SpawnSet) and
/// [`DecorSet`](crate::decoration::DecorSet) established. The payload is
/// [`Command`]s rather than a description of them, because a placement is
/// *exactly* a spawn and inventing a second spelling of twenty-six fields to
/// convert back would be a copy that can drift.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TownsfolkSet {
    /// What the staff menu's button sends: `populate:felucca`. The same verb the
    /// spawn regions answer to — a populate lays both, as it always has.
    pub verb:      String,
    /// One `SpawnMobile` per person.
    pub townsfolk: Vec<Command>,
}

include!(concat!(env!("OUT_DIR"), "/townsfolk.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_townsperson_is_a_spawn_with_a_trade() {
        let sets = shipped();
        assert_eq!(sets.len(), 1);
        let people = &sets[0].townsfolk;
        assert!(people.len() > 500, "only {} townsfolk", people.len());
        for command in people {
            let Command::SpawnMobile { title, sight, .. } = command else {
                panic!("a townsperson is not a spawn: {command:?}");
            };
            assert!(title.is_some(), "a placed townsperson with no trade");
            // The trade is the key its dress, its generated name and its speech
            // all hang off, and a sighted townsperson would hunt customers.
            assert_eq!(sight.0, 0, "a townsperson that notices a foe");
        }
    }

    #[test]
    fn the_shelves_and_the_escorts_ride_on_the_placement() {
        // The whole point of the file: no second command, no tile key. If these
        // ever come back empty, the rendezvous has been reintroduced.
        let people = &shipped()[0].townsfolk;
        let stocked = people
            .iter()
            .filter(|c| matches!(c, Command::SpawnMobile { stock, .. } if !stock.is_empty()))
            .count();
        let escorts = people
            .iter()
            .filter(|c| matches!(c, Command::SpawnMobile { escort_to, .. } if escort_to.is_some()))
            .count();
        assert!(stocked > 100, "only {stocked} shopkeepers carry stock");
        assert!(escorts > 10, "only {escorts} travellers want an escort");

        // A shelf with no shop is stock nobody can reach; `build.rs` rejects it,
        // and this is the same statement about what the world receives.
        assert!(
            people.iter().all(|c| {
                matches!(
                    c,
                    Command::SpawnMobile { stock, vendor, .. } if stock.is_empty() || *vendor
                )
            }),
            "someone carries stock without being a vendor"
        );
    }

    #[test]
    fn the_shipped_healers_keep_their_resurrection_service() {
        // The town data, rather than a spelling rule in the generator, owns
        // which people are healers. Losing this bit leaves every town healer
        // looking right while unable to raise a ghost.
        let healers = shipped()[0]
            .townsfolk
            .iter()
            .filter(|command| matches!(command, Command::SpawnMobile { healer: true, .. }))
            .count();
        assert_eq!(healers, 56, "the healer placements lost their service");
    }
}
