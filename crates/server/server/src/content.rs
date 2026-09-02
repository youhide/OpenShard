//! The shard's own gameplay content, laid down at boot.
//!
//! Content — quests, regions, spawns, decoration — is data in the domain crates,
//! compiled by their `build.rs` (see `docs/architecture.md` § "A big table is
//! data"). Somebody still has to hand it to the world, and that is a wiring job
//! between a crate that holds the data and a crate that owns the tick, which is
//! what the server is for. So this module is the counterpart of
//! [`scripting`](crate::scripting): the same `Command`s, from the tree instead of
//! from a script — which is gone, and this is what took its place.
//!
//! # Why the commands are returned rather than applied
//!
//! [`boot`] hands back a `Vec<Command>` and queues nothing itself. That is what
//! makes the equivalence test below possible at all — the migration off the
//! script pack is only finished when both sources produce the *same* commands,
//! and comparing two worlds after the fact would compare everything that ever
//! happened to them instead. One list against another is the check that means
//! something.
//!
//! # Two ways in, and which dataset takes which
//!
//! [`boot`] is for content that is simply *true* of the shard: quests and
//! townsfolk speech are registered unconditionally, before the first tick.
//!
//! [`verb`] is for content an operator lays and clears by hand — the staff
//! menu's buttons, and the `--seed` argument that sends the same strings without
//! a client attached. `world::admin` owns the buttons; this owns what each one
//! means, now that the answer is in the tree rather than in a pack's `onEvent`.
//!
//! Both return commands and queue nothing, for the reason above.
//!
//! # Every dataset is here now
//!
//! Quests, townsfolk speech, the named regions, the spawn regions, the placed
//! townsfolk with their stock and their escorts, and the decoration. What is left
//! in the Community Pack is *logic* — loot tables, two item behaviours — and not
//! content, so this module is finished until something new is written.

use openshard_state::{
    dialogue,
    quest,
    region,
};
use openshard_world::Command;

/// Every command the shard's own content lays down, before the first tick.
///
/// Called after the world is restored, so it can never overwrite a save it has
/// not seen; and before the first tick, so a player entering on tick one finds a
/// world that is already furnished.
///
/// # Registered wholesale
///
/// Both destinations replace everything before them —
/// [`QuestDefs::set`](openshard_state::quest::QuestDefs::set) and
/// [`Dialogue::set_tables`](openshard_state::Dialogue::set_tables) — so this is
/// the shard's whole answer for each, not an addition to one.
#[must_use]
pub fn boot() -> Vec<Command> {
    vec![
        Command::RegisterQuests {
            quests: quest::shipped(),
        },
        Command::RegisterNpcSpeech {
            trades: dialogue::shipped(),
        },
    ]
}

/// What an admin verb lays down — the staff menu's buttons, and `--seed`.
///
/// Empty for a verb the tree has no data for, which is not an error: an unknown
/// string is what a pack that dropped a set produces, and the engine has never
/// treated it as a failure.
///
/// # Why the verb is in the data
///
/// The set carries its own verb ([`RegionSet::verb`](openshard_state::region::RegionSet)),
/// so this is a lookup rather than a `match`. A `match` here would be a second
/// list to keep level with `world::admin`'s `ROWS`, and the failure when they
/// drifted would be a button that silently does nothing.
///
/// # One verb, several datasets
///
/// `populate:felucca` lays the spawn regions *and* the standing townsfolk, which
/// is what it has always meant. A configured pack answering the same verb is
/// fine: the world applies what each lays, and each of them de-duplicates.
///
/// # Laying twice
///
/// Every verb here is idempotent, which is what makes `--seed` safe on a shard
/// that already has a world. `Regions::set` replaces the facet's whole list;
/// `register_spawner` de-duplicates by `SpawnArea`; `decorate` skips a row
/// already standing; and a placed townsperson is skipped when one of its trade
/// already stands on the tile. None of that was true before this migration, and
/// `main.rs` used to warn that seeding twice laid everything twice.
#[must_use]
pub fn verb(action: &str) -> Vec<Command> {
    let regions = region::shipped()
        .into_iter()
        .filter(|set| set.verb == action)
        .map(|set| {
            Command::RegisterRegions {
                facet:   set.facet,
                regions: set.regions,
            }
        });
    let spawners = openshard_world::spawner::shipped()
        .into_iter()
        .filter(|set| set.verb == action)
        .flat_map(|set| set.spawners)
        .map(|spawner| Command::RegisterSpawner { spawner });
    // The farmland, on the same verb: a facet's cotton is part of its population,
    // and the fields are laid before the people for the reason the regions are —
    // the wilderness first, then who lives in it.
    let fields = openshard_world::crops::shipped()
        .into_iter()
        .filter(|set| set.verb == action)
        .flat_map(|set| set.fields)
        .map(|field| Command::RegisterCropField { field });
    // The people, after the regions that keep the wilderness full — the pack's
    // order, and the order the verb has always meant: a populate both maintains a
    // facet and puts its named townsfolk on their doorsteps.
    let people = openshard_world::townsfolk::shipped()
        .into_iter()
        .filter(|set| set.verb == action)
        .flat_map(|set| set.townsfolk);
    // Decoration, then the door generation that reads it. The order is the pack's
    // and it is load-bearing: a generated door goes in the gap between two static
    // frames, and some of those frames are laid by the batch above.
    let decor = openshard_world::decoration::shipped()
        .into_iter()
        .filter(|set| set.verb == action)
        .flat_map(|set| {
            let batch = Command::Decorate {
                facet:      set.facet,
                statics:    set.statics.to_vec(),
                doors:      set.doors.to_vec(),
                containers: set.containers.to_vec(),
            };
            let scans = set.door_regions.iter().map(move |&(x, y, width, height)| {
                Command::GenerateDoors {
                    facet: set.facet,
                    x,
                    y,
                    width,
                    height,
                }
            });
            std::iter::once(batch).chain(scans)
        });
    // The three destructive menu rows name actions rather than datasets, but
    // they belong here for the same reason the lay rows do: this is the shard's
    // one translation from an admin verb into world commands.  Leaving them
    // out made the buttons reach `AdminMenuAction` and then silently stop.
    let clear = match action {
        "clear" => vec![Command::ClearSpawners],
        "clear:deco" => vec![Command::ClearDecorations],
        "clear:regions" => {
            vec![Command::ClearRegions {
                facet: openshard_protocol::world::Facet(0),
            }]
        }
        _ => Vec::new(),
    };
    regions
        .chain(spawners)
        .chain(fields)
        .chain(people)
        .chain(decor)
        .chain(clear)
        .collect()
}

#[cfg(test)]
mod tests {
    use openshard_state::WorldTick;

    use super::*;

    /// What CI gets, since the equivalence test below skips without the pack:
    /// the shard's content reaches the world as one registration carrying every
    /// quest the tree ships.
    ///
    /// Where it stops is deliberate. `WorldState` is not reachable from this
    /// crate — the server drives the world through commands and reads it through
    /// events, and widening that for a test would cost more than the test is
    /// worth. What `RegisterQuests` then *does* is `world`'s own to prove, and it
    /// does (`tick/quest_tests.rs`); what is unproven anywhere else, and proven
    /// here, is that `boot` emits it and emits all of it.
    #[test]
    fn boot_hands_the_world_every_dataset_the_tree_ships() {
        let quests = quest::shipped();
        let trades = dialogue::shipped();
        assert!(!quests.is_empty(), "the shard ships no quests at all");
        assert!(!trades.is_empty(), "the shard ships no trade speech at all");

        assert_eq!(
            boot(),
            vec![
                Command::RegisterQuests { quests },
                Command::RegisterNpcSpeech { trades },
            ],
            "the tree's content is not reaching the world intact"
        );
    }

    #[test]
    fn the_staff_menus_region_button_lays_the_regions_the_tree_ships() {
        // The verb string is the whole of the contract between `world::admin`'s
        // button and this module, and neither side can check the other at compile
        // time. If they drift, the button silently lays nothing.
        let commands = verb("regions:felucca");
        assert_eq!(commands.len(), 1, "the region button lays nothing");
        let Command::RegisterRegions { facet, regions } = &commands[0] else {
            panic!("the region verb laid something other than regions: {commands:?}");
        };
        assert_eq!(*facet, openshard_protocol::world::Facet(0));
        assert!(regions.len() > 100, "only {} regions on Felucca", regions.len());
        assert!(
            regions.iter().any(|region| region.flags.guarded),
            "no region on Felucca is guarded, so no guard will ever answer"
        );
    }

    #[test]
    fn the_staff_menus_populate_button_lays_every_spawn_region_the_tree_ships() {
        let commands = verb("populate:felucca");
        let spawners = only_spawners(commands);
        assert!(
            spawners.len() > 1000,
            "only {} spawn regions on Felucca",
            spawners.len()
        );
        // Every one comes out with the placeholder id and no timer: both belong to
        // the live spawner, and `register_spawner` sets them. A number written into
        // the data would be a second source for either.
        assert!(
            spawners.iter().all(|s| {
                s.id == openshard_state::SpawnerId::PLACEHOLDER && s.next_spawn == WorldTick::ZERO
            }),
            "a shipped spawn region arrived with an id or a timer already set"
        );
        assert!(
            spawners
                .iter()
                .all(|s| !s.creatures.is_empty() && s.max_count > 0),
            "a shipped spawn region can never put anything down"
        );
    }

    #[test]
    fn the_staff_menus_decorate_button_lays_the_art_then_scans_for_doors() {
        // The order is the whole of it. A generated door goes in the gap between
        // two static frames, and some of those frames are in the batch above it —
        // so a scan that ran first would find a doorway that is not there yet.
        let commands = verb("decorate:felucca");
        let Some(Command::Decorate {
            statics,
            doors,
            containers,
            ..
        }) = commands.first()
        else {
            panic!(
                "the decorate verb does not lay decoration first: {:?}",
                commands.first()
            );
        };
        assert!(statics.len() > 10_000, "only {} statics", statics.len());
        assert!(!doors.is_empty() && !containers.is_empty());
        assert!(
            commands[1..]
                .iter()
                .all(|c| matches!(c, Command::GenerateDoors { .. })),
            "something other than a door scan followed the decoration"
        );
        assert!(commands.len() > 1, "no region is scanned for implied doors");
    }

    #[test]
    fn the_staff_menus_clear_buttons_remove_their_matching_content() {
        assert!(matches!(verb("clear").as_slice(), [Command::ClearSpawners]));
        assert!(matches!(
            verb("clear:deco").as_slice(),
            [Command::ClearDecorations]
        ));
        assert!(matches!(
            verb("clear:regions").as_slice(),
            [Command::ClearRegions {
                facet: openshard_protocol::world::Facet(0),
            }]
        ));
    }

    #[test]
    fn a_verb_the_tree_has_no_content_for_lays_nothing() {
        // Not an error and not a panic: an unknown verb is what a pack that
        // dropped a set would produce, and the engine has never treated it as a
        // failure.
        assert!(verb("").is_empty());
        assert!(verb("regions:trammel").is_empty());
        assert!(verb("populate:trammel").is_empty());
    }

    /// The spawn regions out of a command stream.
    fn only_spawners(commands: Vec<Command>) -> Vec<openshard_world::spawner::Spawner> {
        commands
            .into_iter()
            .filter_map(|command| {
                match command {
                    Command::RegisterSpawner { spawner } => Some(spawner),
                    _ => None,
                }
            })
            .collect()
    }
}
