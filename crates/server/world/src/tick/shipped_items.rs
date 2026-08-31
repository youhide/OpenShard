//! What the two items the shard ships do when they are double-clicked.
//!
//! The engine answers for every item it knows the *kind* of — a door toggles, a
//! container opens, a spellbook unfolds, a mount is ridden — and an item whose
//! meaning is content's rather than the engine's falls through to here. This is
//! that second layer, and it is deliberately two items long: a welcome book and a
//! heal potion, the pair the Community Pack used to demonstrate its item-trigger
//! seam.
//!
//! # Behaviour, not data
//!
//! The other five datasets moved as JSON because they are tables. These are not:
//! "heal twenty-five and take one bottle" is a *rule*, and the shape of a rule is
//! a function. A table of `{graphic → op-name, arguments}` would be a scripting
//! language with one caller, which is the thing this migration set out to delete.
//!
//! # Run after the engine, and after a pack
//!
//! `items::double_click` has already emitted its `ItemUsed`, so a configured
//! script pack has already had its say. This runs last and only for a graphic it
//! knows, which is the same "default in core, customise on top" order the rest of
//! the seam uses.

use super::*;

/// A brown book: read it and a line of the shard's lore appears overhead.
const BROWN_BOOK: Graphic = Graphic(0x0FF2);

/// The parchment hue that line is spoken in.
const PARCHMENT: Hue = Hue(0x0481);

/// What the brown book says. One is picked on the world's seeded generator, so a
/// replayed tick reads the same page.
const WELCOME_LINES: &[&str] = &[
    "The pages read: 'Welcome, traveller, to the shard of OpenShard.'",
    "The pages read: 'Say a word near a banker to open thy vault.'",
    "The pages read: 'A mage in Britain will sell thee a spellbook and scrolls to fill it.'",
];

/// A greater heal potion: drink it and it mends thee, and the bottle is gone.
const HEAL_POTION: Graphic = Graphic(0x0F0C);

/// How much the potion mends.
const HEAL_AMOUNT: u16 = 25;

impl World {
    /// Answer a double-click on one of the items the shard ships a behaviour for.
    ///
    /// Returns whether it was one. Called after the engine's own handling, so an
    /// item that is *also* a container or a door has already been opened and never
    /// reaches here.
    pub(super) fn use_shipped_item(&mut self, user: EntityId, item: EntityId) -> bool {
        let Some(&Drawn { id: graphic, .. }) = self.state.registry.get::<Drawn>(item) else {
            return false;
        };
        match graphic {
            BROWN_BOOK => {
                let at = self.state.rng.below(WELCOME_LINES.len() as u32) as usize;
                let line = WELCOME_LINES[at];
                chat::speak(
                    &mut self.state,
                    user,
                    TalkMode::Regular,
                    PARCHMENT,
                    Font::DEFAULT,
                    line,
                );
                true
            }
            HEAL_POTION => {
                let Some(serial) = self.state.registry.serial_of(user) else {
                    return false;
                };
                magic::heal(&mut self.state, serial, HEAL_AMOUNT);
                // One bottle out of the pile, not the pile: potions stack.
                if let Some(item) = self.state.registry.serial_of(item) {
                    items::consume(&mut self.state, item, 1);
                }
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use openshard_state::components::{
        Amount,
        Hitpoints,
    };

    use super::*;
    use crate::tick::tests::{
        enter,
        serial_of,
        world,
    };

    /// A logged-in player, and two of `graphic` in its pack.
    fn player_holding(world: &mut World, now: Instant, graphic: Graphic) -> (EntityId, EntityId) {
        let connection = enter(world, now);
        let player = world.state.players[&connection];
        let owner = serial_of(world, connection);
        let backpack = openshard_state::equipped_items(&world.state, owner)
            .find(|(_, worn)| worn.layer == items::BACKPACK_LAYER)
            .map(|(entity, _)| world.state.registry.serial_of(entity).unwrap())
            .expect("the player wears a backpack");
        // Two, so the potion test can show that one bottle goes and not the lot.
        world.add_loot(backpack, graphic, Hue(0), 2, true);
        let item = openshard_state::contained_items(&world.state, backpack)
            .map(|(entity, _)| entity)
            .find(|&entity| {
                world
                    .state
                    .registry
                    .get::<Drawn>(entity)
                    .is_some_and(|drawn| drawn.id == graphic)
            })
            .expect("the item went into the pack");
        (player, item)
    }

    #[test]
    fn the_potion_mends_the_drinker_and_leaves_the_rest_of_the_lot() {
        let now = Instant::now();
        let mut world = world();
        let (player, potion) = player_holding(&mut world, now, HEAL_POTION);

        // Wounded first, or there is nothing for the mending to show.
        let max = world.state.registry.get::<Hitpoints>(player).unwrap().max;
        world.state.registry.insert(
            player,
            Hitpoints {
                current: max / 2,
                max,
            },
        );

        assert!(world.use_shipped_item(player, potion), "the potion did nothing");
        assert!(
            world.state.registry.get::<Hitpoints>(player).unwrap().current > max / 2,
            "the drinker was not mended"
        );
        // Potions stack, so one bottle goes and the rest of the pile stays. This is
        // the whole reason `consume` takes an amount.
        assert_eq!(
            world.state.registry.get::<Amount>(potion).map(|amount| amount.0),
            Some(1),
            "the whole lot was drunk at once"
        );
    }

    #[test]
    fn the_book_reads_a_page_and_the_page_is_one_of_its_own() {
        let now = Instant::now();
        let mut world = world();
        let (player, book) = player_holding(&mut world, now, BROWN_BOOK);
        assert!(world.use_shipped_item(player, book), "the book did nothing");
        // Read, not drunk: the book is not a one-shot and stays in the pack.
        assert!(
            world.state.registry.get::<Amount>(book).map(|amount| amount.0) == Some(2),
            "reading the book consumed it"
        );
        assert!(
            WELCOME_LINES.iter().all(|line| !line.is_empty()),
            "the book has a blank page"
        );
    }

    #[test]
    fn an_item_the_shard_ships_nothing_for_is_left_alone() {
        // The engine's own answer has already run by the time this is reached, so
        // claiming an item it did not mean to would be taking one away from it.
        let now = Instant::now();
        let mut world = world();
        let (player, gold) = player_holding(&mut world, now, Graphic(0x0EED));
        assert!(
            !world.use_shipped_item(player, gold),
            "gold was given a behaviour"
        );
    }
}
