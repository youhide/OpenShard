//! Crafting: the tool, the workshop, the materials and what comes out.
//!
//! A child module of its own, like `harvest_tests.rs`, and for the same reason —
//! these run the whole path a player does, and every link in it fails silently.
//! A forge scan that reads only entities refuses a craft at half the shops in
//! Britannia and looks like a broken recipe; a hue-blind material take pays a
//! valorite order in iron and nobody notices until somebody compares two blades.
//!
//! The chance curve and the button encoding are unit-tested in `crafting` where
//! they are pure. What is here is everything that needs a world: a forge on the
//! ground, ingots in a pack, a tick counter, and a save.

use super::tests::{START, enter, packets_for, world};
use super::*;
use openshard_crafting::SystemId;
use openshard_movement::scene::Scene;
use openshard_protocol::containers::GridSlot;
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::serial::RawSerial;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_state::components::{Contained, CraftedBy, Crafting, Quality, Tool};
use openshard_state::harvest::ORE_GRAPHIC;
use openshard_state::{CraftGumpContext, CraftGumpPage, Skill};

/// A smith's tongs, one of the graphics that opens the blacksmithy window.
const TONGS: Graphic = Graphic(0x0FBB);
/// A sewing kit — a trade that needs no workshop, which is the contrast.
const SEWING_KIT: Graphic = Graphic(0x0F9D);
/// An anvil's static id, ServUO's `4015`.
const ANVIL: Graphic = Graphic(4015);
/// A forge's, `4017`.
const FORGE: Graphic = Graphic(4017);
/// An iron ingot.
const INGOT: Graphic = openshard_crafting::INGOT_GRAPHIC;
/// Valorite's hue — the top of the metal axis.
const VALORITE: Hue = Hue(0x08AB);

/// Stand the player in a shop with these statics under foot.
///
/// A forge and an anvil are *static* tiles in most of Britannia's shops, which is
/// the half of the scan that is easy to leave out — so the workshop tests use
/// statics and the "no workshop" one uses bare ground.
///
/// **A real map.** The statics come out of a [`Scene`]'s
/// [`WorldMap`](openshard_map::map::WorldMap) and the scan reads them through the
/// shard's own `statics_at`. They stand on the player's own tile rather than on
/// every tile of the facet, which is what the double did and what no shop looks
/// like; the scan reaches two tiles, so one tile is the whole of what it needs.
/// They are declared with no tiledata row on purpose — drawn, in the way of
/// nothing — because what `find_facilities` matches is the graphic id.
fn shop(world: &mut World, statics: &[(u16, i8)]) {
    let mut scene = Scene::flat_holding(START.0 + 4, START.1 + 4, 0);
    for &(graphic, z) in statics {
        scene.put(START.0, START.1, z, graphic);
    }
    let (map, tiles) = scene.into_shard(Facet(0));
    world.state.facet_state_mut(Facet(0)).set_map(Some(map), &tiles);
    world.state.set_tiles(tiles);
}

/// Put an item in the player's pack, through the door a vendor's shelf uses.
fn give(world: &mut World, connection: ConnectionId, graphic: Graphic, hue: Hue, amount: u16) -> EntityId {
    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).unwrap();
    let backpack = items::backpack_of(&world.state, owner).expect("a backpack");
    let (item, _) = world.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    world.state.registry.insert(item, Drawn { id: graphic, hue });
    openshard_state::establish_item_location(
        &mut world.state,
        item,
        openshard_state::ItemLocation::contained(Contained {
            container: backpack,
            position: GumpPoint::new(20, 20),
            grid: GridSlot(0),
        }),
    )
    .unwrap();
    if amount > 1 {
        world.state.registry.insert(item, Amount(amount));
        world
            .state
            .registry
            .insert(item, openshard_state::components::Stackable);
    }
    items::apply_core_defaults(&mut world.state, item, graphic);
    item
}

/// Give the player a skill outright, so a roll is a sure thing.
fn train(world: &mut World, connection: ConnectionId, skill: Skill, value: u16) {
    let entity = world.state.players[&connection];
    let serial = world.state.registry.serial_of(entity).unwrap();
    world.queue(Command::SetSkill {
        serial,
        skill: skill.id(),
        value,
    });
}

/// Every cliloc number the connection was sent this tick, in order.
fn clilocs(world: &mut World, connection: ConnectionId) -> Vec<u32> {
    packets_for(world, connection)
        .into_iter()
        .filter(|p| p[0] == 0xC1)
        .map(|p| u32::from_be_bytes([p[14], p[15], p[16], p[17]]))
        .collect()
}

/// How many of a graphic at a hue are in the player's backpack.
fn carried(world: &World, connection: ConnectionId, graphic: Graphic, hue: Hue) -> u32 {
    let player = world.state.players[&connection];
    let Some(owner) = world.state.registry.serial_of(player) else {
        return 0;
    };
    openshard_items::carried_amount_of_hue(&world.state, owner, graphic, Some(hue))
}

/// The index of the first blacksmithy recipe made of one ingot line, and the
/// group it is in.
///
/// Found rather than hard-coded: the tables are generated, and a recipe's index
/// moves whenever ServUO's own list does. What the tests want is "something cheap
/// a novice can make", not a particular dagger.
fn cheapest_smithing() -> (u16, u16) {
    let def = openshard_crafting::system(SystemId::new(0)).expect("blacksmithy");
    let (index, recipe) = def
        .recipes
        .iter()
        .enumerate()
        .filter(|(_, recipe)| recipe.resources.len() == 1 && recipe.skills.len() == 1)
        .min_by_key(|(_, recipe)| (recipe.skills[0].min, recipe.resources[0].amount))
        .expect("a one-material recipe");
    (u16::try_from(index).unwrap(), recipe.group)
}

/// Open the craft window on a tool and press the row that makes `recipe`.
fn craft(
    world: &mut World,
    connection: ConnectionId,
    tool: EntityId,
    recipe: u16,
    sub_res: u8,
    now: Instant,
) {
    let player = world.state.players[&connection];
    let def = openshard_crafting::system(SystemId::new(0)).expect("blacksmithy");
    let group = def.recipes[usize::from(recipe)].group;
    openshard_crafting::open(
        &mut world.state,
        player,
        CraftGumpContext {
            system: 0,
            tool,
            group,
            sub_res,
            page: CraftGumpPage::Items,
            notice: None,
        },
    );
    // The row's index is its place *within the group*, which is what the window
    // draws and what the reply carries.
    let row = def
        .recipes
        .iter()
        .enumerate()
        .filter(|(_, r)| r.group == group)
        .position(|(at, _)| at == usize::from(recipe))
        .expect("the recipe is in its own group");
    let serial = world.state.registry.serial_of(player).unwrap();
    world.queue(Command::GumpResponse {
        connection,
        response: openshard_protocol::gump::GumpResponse {
            serial: openshard_protocol::gump::RawGumpKey(serial.raw()),
            gump_id: openshard_protocol::gump::RawGumpId(openshard_crafting::CRAFT_GUMP.0),
            // ServUO's `1 + kind + index * 7`, kind 1 being "make".
            button: openshard_protocol::gump::RawButtonId(1 + 1 + u32::try_from(row).unwrap() * 7),
            switches: Vec::new(),
            text_entries: Vec::new(),
        },
    });
    world.tick(now);
}

/// Tick until the craft in flight resolves, or give up.
fn finish(world: &mut World, connection: ConnectionId, mut now: Instant) -> Instant {
    let player = world.state.players[&connection];
    for _ in 0..400 {
        if !world.state.registry.has::<Crafting>(player) {
            return now;
        }
        now += TICK_INTERVAL;
        world.tick(now);
    }
    panic!("the craft never finished");
}

#[test]
fn a_smith_at_a_forge_turns_ingots_into_a_blade() {
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    shop(&mut world, &[(FORGE.0, 0), (ANVIL.0, 0)]);
    let tongs = give(&mut world, connection, TONGS, Hue(0), 1);
    let (recipe, _) = cheapest_smithing();
    let def = openshard_crafting::system(SystemId::new(0)).unwrap();
    let wanted = def.recipes[usize::from(recipe)].resources[0].amount;
    let made = def.recipes[usize::from(recipe)].graphic;
    give(&mut world, connection, INGOT, Hue(0), wanted * 2);
    train(&mut world, connection, Skill::Blacksmith, 1000);
    now += TICK_INTERVAL;
    world.tick(now);
    let uses_before = world.state.registry.get::<Tool>(tongs).unwrap().uses_left;

    craft(&mut world, connection, tongs, recipe, 0, now);
    finish(&mut world, connection, now);

    assert_eq!(carried(&world, connection, made, Hue(0)), 1, "the blade");
    assert_eq!(
        carried(&world, connection, INGOT, Hue(0)),
        u32::from(wanted),
        "half the ingots are gone"
    );
    assert_eq!(
        world.state.registry.get::<Tool>(tongs).unwrap().uses_left,
        uses_before - 1,
        "a craft costs a use whether it worked or not"
    );
}

#[test]
fn the_same_smith_in_the_street_is_told_to_find_a_forge() {
    // The workshop gate, and the half that matters: a forge is a *static* in most
    // of Britannia's shops, so a scan that reads only entities would pass this
    // test on bare ground and fail in every real smithy.
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    shop(&mut world, &[]);
    let tongs = give(&mut world, connection, TONGS, Hue(0), 1);
    let (recipe, _) = cheapest_smithing();
    let def = openshard_crafting::system(SystemId::new(0)).unwrap();
    let wanted = def.recipes[usize::from(recipe)].resources[0].amount;
    let made = def.recipes[usize::from(recipe)].graphic;
    give(&mut world, connection, INGOT, Hue(0), wanted * 4);
    train(&mut world, connection, Skill::Blacksmith, 1000);
    now += TICK_INTERVAL;
    world.tick(now);

    let _ = clilocs(&mut world, connection);
    craft(&mut world, connection, tongs, recipe, 0, now);

    assert!(
        !world
            .state
            .registry
            .has::<Crafting>(world.state.players[&connection]),
        "nothing was begun"
    );
    assert_eq!(carried(&world, connection, made, Hue(0)), 0);
    assert!(
        clilocs(&mut world, connection).contains(&1_044_267),
        "you must be near an anvil and a forge"
    );
}

#[test]
fn an_anvil_alone_is_not_a_smithy() {
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    shop(&mut world, &[(ANVIL.0, 0)]);
    let tongs = give(&mut world, connection, TONGS, Hue(0), 1);
    let (recipe, _) = cheapest_smithing();
    let def = openshard_crafting::system(SystemId::new(0)).unwrap();
    give(
        &mut world,
        connection,
        INGOT,
        openshard_protocol::wire::Hue(0),
        def.recipes[usize::from(recipe)].resources[0].amount * 2,
    );
    train(&mut world, connection, Skill::Blacksmith, 1000);
    now += TICK_INTERVAL;
    world.tick(now);

    craft(&mut world, connection, tongs, recipe, 0, now);
    assert!(
        !world
            .state
            .registry
            .has::<Crafting>(world.state.players[&connection])
    );
}

#[test]
fn a_valorite_order_cannot_be_paid_in_iron() {
    // Hue *is* identity for a crafting material — nine metals share one graphic —
    // so a hue-blind take would quietly make a valorite blade out of iron. This is
    // the test that says the take asks.
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    shop(&mut world, &[(FORGE.0, 0), (ANVIL.0, 0)]);
    let tongs = give(&mut world, connection, TONGS, Hue(0), 1);
    let (recipe, _) = cheapest_smithing();
    let def = openshard_crafting::system(SystemId::new(0)).unwrap();
    let wanted = def.recipes[usize::from(recipe)].resources[0].amount;
    let made = def.recipes[usize::from(recipe)].graphic;
    give(&mut world, connection, INGOT, Hue(0), wanted * 10);
    train(&mut world, connection, Skill::Blacksmith, 1000);
    now += TICK_INTERVAL;
    world.tick(now);

    // Valorite is the ninth entry of the axis, and the player has none of it.
    let valorite = u8::try_from(def.sub_res.unwrap().entries.len() - 1).unwrap();
    craft(&mut world, connection, tongs, recipe, valorite, now);
    assert!(
        !world
            .state
            .registry
            .has::<Crafting>(world.state.players[&connection])
    );
    assert_eq!(carried(&world, connection, made, Hue(0)), 0);
    assert_eq!(
        carried(&world, connection, INGOT, Hue(0)),
        u32::from(wanted) * 10,
        "and the iron was not touched"
    );

    // With valorite in the pack it goes ahead, and the blade comes out the colour
    // of the metal it was made from.
    give(&mut world, connection, INGOT, VALORITE, wanted * 2);
    craft(&mut world, connection, tongs, recipe, valorite, now);
    finish(&mut world, connection, now);
    assert_eq!(carried(&world, connection, made, VALORITE), 1);
    assert_eq!(
        carried(&world, connection, INGOT, Hue(0)),
        u32::from(wanted) * 10,
        "the iron is still untouched"
    );
}

#[test]
fn a_novice_is_refused_rather_than_charged_for_a_failure() {
    // The distinction the chance module is about: below the band a craft is a
    // *refusal*, which costs nothing, and not a failed roll, which costs the
    // materials. Confusing the two eats the ingots of every player who clicked a
    // recipe they were not yet good enough for.
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    shop(&mut world, &[(FORGE.0, 0), (ANVIL.0, 0)]);
    let tongs = give(&mut world, connection, TONGS, Hue(0), 1);
    let def = openshard_crafting::system(SystemId::new(0)).unwrap();
    // The hardest one-material recipe in the table, which nobody at zero can try.
    let (recipe, _) = def
        .recipes
        .iter()
        .enumerate()
        .filter(|(_, r)| r.resources.len() == 1)
        .max_by_key(|(_, r)| r.skills[0].min)
        .map(|(at, r)| (u16::try_from(at).unwrap(), r))
        .expect("a hard recipe");
    let wanted = def.recipes[usize::from(recipe)].resources[0].amount;
    give(&mut world, connection, INGOT, Hue(0), wanted * 4);
    now += TICK_INTERVAL;
    world.tick(now);
    let _ = clilocs(&mut world, connection);

    craft(&mut world, connection, tongs, recipe, 0, now);

    assert_eq!(
        carried(&world, connection, INGOT, Hue(0)),
        u32::from(wanted) * 4,
        "a refusal costs nothing"
    );
    assert!(
        clilocs(&mut world, connection).contains(&1_044_153),
        "you don't have the required skills"
    );
}

#[test]
fn a_tailor_needs_no_workshop_at_all() {
    // The other half of the workshop rule: only blacksmithy demands one, and a
    // system-wide `Needs` that leaked onto the others would strand every tailor
    // on the shard in the middle of a field.
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    shop(&mut world, &[]);
    let kit = give(&mut world, connection, SEWING_KIT, Hue(0), 1);
    let player = world.state.players[&connection];
    train(&mut world, connection, Skill::Tailoring, 1000);
    now += TICK_INTERVAL;
    world.tick(now);

    let tailoring = openshard_crafting::SYSTEMS
        .iter()
        .position(|def| def.skill == Skill::Tailoring)
        .unwrap();
    let def = &openshard_crafting::SYSTEMS[tailoring];
    let (index, recipe) = def
        .recipes
        .iter()
        .enumerate()
        .filter(|(_, r)| r.resources.len() == 1 && r.skills.len() == 1)
        .min_by_key(|(_, r)| (r.skills[0].min, r.resources[0].amount))
        .expect("a simple garment");
    let cloth = recipe.resources[0];
    give(&mut world, connection, cloth.graphic, cloth.hue, cloth.amount * 2);

    assert!(
        openshard_crafting::begin(
            &mut world.state,
            player,
            kit,
            SystemId::from_index(tailoring).unwrap(),
            u16::try_from(index).unwrap(),
            0,
        ),
        "a tailor in a field is still a tailor"
    );
}

#[test]
fn ore_becomes_ingots_at_a_forge_and_nowhere_else() {
    // The step without which the whole of blacksmithy is unreachable from mining:
    // a miner is paid in ore and every recipe eats ingots.
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    shop(&mut world, &[]);
    let ore = give(&mut world, connection, ORE_GRAPHIC, Hue(0), 10);
    train(&mut world, connection, Skill::Mining, 1000);
    now += TICK_INTERVAL;
    world.tick(now);

    let serial = world.state.registry.serial_of(ore).unwrap();
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(serial.raw())),
    });
    now += TICK_INTERVAL;
    world.tick(now);
    assert_eq!(
        carried(&world, connection, INGOT, Hue(0)),
        0,
        "no forge, no ingots"
    );
    assert_eq!(carried(&world, connection, ORE_GRAPHIC, Hue(0)), 10);

    shop(&mut world, &[(FORGE.0, 0)]);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(serial.raw())),
    });
    now += TICK_INTERVAL;
    world.tick(now);
    assert_eq!(
        carried(&world, connection, INGOT, Hue(0)),
        20,
        "two ingots to the unit, ServUO's large-pile rate"
    );
    assert_eq!(carried(&world, connection, ORE_GRAPHIC, Hue(0)), 0);
}

#[test]
fn the_metal_a_pile_of_ore_is_survives_the_forge() {
    // A smelt that dropped the hue would turn every vein on the shard into iron,
    // and the mining bands that make valorite worth finding with it.
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    shop(&mut world, &[(FORGE.0, 0)]);
    let ore = give(&mut world, connection, ORE_GRAPHIC, VALORITE, 4);
    train(&mut world, connection, Skill::Mining, 1000);
    now += TICK_INTERVAL;
    world.tick(now);

    let serial = world.state.registry.serial_of(ore).unwrap();
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(serial.raw())),
    });
    now += TICK_INTERVAL;
    world.tick(now);

    assert_eq!(carried(&world, connection, INGOT, VALORITE), 8);
    assert_eq!(carried(&world, connection, INGOT, Hue(0)), 0);
}

#[test]
fn a_gump_reply_for_a_window_the_server_never_opened_makes_nothing() {
    // The context is the only place the tool, the category and the metal live, so
    // a reply with no remembered window has nothing to act on — the rule
    // `quests::reply` set, tested here because it is what stops an invented packet
    // crafting for free.
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    shop(&mut world, &[(FORGE.0, 0), (ANVIL.0, 0)]);
    give(&mut world, connection, TONGS, Hue(0), 1);
    let (recipe, _) = cheapest_smithing();
    let def = openshard_crafting::system(SystemId::new(0)).unwrap();
    let made = def.recipes[usize::from(recipe)].graphic;
    give(
        &mut world,
        connection,
        INGOT,
        openshard_protocol::wire::Hue(0),
        def.recipes[usize::from(recipe)].resources[0].amount * 4,
    );
    train(&mut world, connection, Skill::Blacksmith, 1000);
    now += TICK_INTERVAL;
    world.tick(now);

    let player = world.state.players[&connection];
    let serial = world.state.registry.serial_of(player).unwrap();
    world.queue(Command::GumpResponse {
        connection,
        response: openshard_protocol::gump::GumpResponse {
            serial: openshard_protocol::gump::RawGumpKey(serial.raw()),
            gump_id: openshard_protocol::gump::RawGumpId(openshard_crafting::CRAFT_GUMP.0),
            button: openshard_protocol::gump::RawButtonId(2), // "make", row 0
            switches: Vec::new(),
            text_entries: Vec::new(),
        },
    });
    now += TICK_INTERVAL;
    world.tick(now);

    assert!(!world.state.registry.has::<Crafting>(player));
    assert_eq!(carried(&world, connection, made, Hue(0)), 0);
}

#[test]
fn an_exceptional_piece_is_still_exceptional_after_a_restart() {
    // Schema v21. Without it every masterpiece on the shard quietly becomes
    // ordinary at the next boot — the `Murders` bug, over property somebody spent
    // an hour earning.
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).unwrap();
    let sword = give(
        &mut world,
        connection,
        openshard_protocol::wire::Graphic(0x0F5E),
        openshard_protocol::wire::Hue(0),
        1,
    );
    world.state.registry.insert(sword, Quality { exceptional: true });
    world.state.registry.insert(sword, CraftedBy("Rowena".into()));
    now += TICK_INTERVAL;
    world.tick(now);

    let record = World::item_record(
        &world.state.registry,
        sword,
        Some(owner),
        openshard_persistence::ItemLocation::Ground {
            facet: 0,
            x: START.0,
            y: START.1,
            z: 0,
        },
    )
    .expect("a swept item");
    assert_eq!(record.crafted, Some((true, Some("Rowena".into()))));

    // And an ordinary sword carries nothing at all, which is what keeps the two
    // columns empty for all but the handful of items a player made.
    let plain = give(
        &mut world,
        connection,
        openshard_protocol::wire::Graphic(0x0F5E),
        openshard_protocol::wire::Hue(0),
        1,
    );
    let plain = World::item_record(
        &world.state.registry,
        plain,
        Some(owner),
        openshard_persistence::ItemLocation::Ground {
            facet: 0,
            x: START.0,
            y: START.1,
            z: 0,
        },
    )
    .expect("a swept item");
    assert_eq!(plain.crafted, None);
}

#[test]
fn craftsmanship_is_read_where_the_armour_rating_is_worked_out() {
    // A read-site derivation, like a weapon's swing speed: nothing is folded into
    // the wearer, so a fine breastplate coming off leaves nothing to undo.
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let plate = give(
        &mut world,
        connection,
        openshard_protocol::wire::Graphic(0x1415),
        openshard_protocol::wire::Hue(0),
        1,
    ); // a plate chest
    let plain = openshard_state::armor::piece_rating(&world.state, plate);
    assert!(plain > 0, "the core table knows a breastplate");

    world.state.registry.insert(plate, Quality { exceptional: true });
    assert_eq!(
        openshard_state::armor::piece_rating(&world.state, plate),
        plain + 8,
        "ServUO's -8 + 8 * quality, with Exceptional being 2"
    );

    // And the metal is worth something too, which is the whole point of offering
    // a smith nine of them.
    let valorite = give(
        &mut world,
        connection,
        openshard_protocol::wire::Graphic(0x1415),
        VALORITE,
        1,
    );
    assert_eq!(
        openshard_state::armor::piece_rating(&world.state, valorite),
        plain + 16,
    );
    now += TICK_INTERVAL;
    world.tick(now);
}

#[test]
fn double_clicking_the_tongs_is_what_opens_the_window() {
    // The whole way in. There is no craft packet: the client sends an ordinary
    // use, and everything after that is a gump — so a tool that answers a
    // double-click with nothing is a trade nobody can reach, which is what all
    // five of these were before this slice.
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    shop(&mut world, &[(FORGE.0, 0), (ANVIL.0, 0)]);
    let tongs = give(&mut world, connection, TONGS, Hue(0), 1);
    now += TICK_INTERVAL;
    world.tick(now);
    let _ = packets_for(&mut world, connection);

    let serial = world.state.registry.serial_of(tongs).unwrap();
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(serial.raw())),
    });
    now += TICK_INTERVAL;
    world.tick(now);

    let player = world.state.players[&connection];
    assert!(
        world
            .state
            .row_of(player)
            .is_some_and(|row| row.craft_gump.is_some()),
        "the server remembers drawing it"
    );
    assert!(
        packets_for(&mut world, connection)
            .iter()
            .any(|packet| packet[0] == 0xB0),
        "and the client was sent one"
    );

    // And it is forgotten when the client goes. It used to be keyed by the
    // player's entity in a map of its own, which `disconnect` did not sweep — so
    // logging out with the window open left a context behind for an entity that
    // no longer existed, for as long as the process ran. Its own doc said
    // "Cleared on logout", which was true of the map it was written beside.
    world.queue(Command::Disconnect { connection });
    now += TICK_INTERVAL;
    world.tick(now);

    assert_eq!(
        world
            .state
            .connections
            .values()
            .filter(|row| row.craft_gump.is_some())
            .count(),
        0,
        "nothing anywhere still remembers drawing it"
    );
}

#[test]
fn a_tool_off_the_shelf_has_uses_in_it() {
    // `apply_core_defaults` is the one place graphic-implied state is attached,
    // and a craft tool with no `Tool` component is a prop that never wears out —
    // the bug the instruments and pickaxes each had before their own slice.
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    for graphic in [TONGS, SEWING_KIT] {
        let tool = give(&mut world, connection, graphic, Hue(0), 1);
        let uses = world
            .state
            .registry
            .get::<Tool>(tool)
            .expect("a craft tool wears out")
            .uses_left;
        assert!((25..=75).contains(&uses), "ServUO's RandomMinMax(25, 75)");
    }
    now += TICK_INTERVAL;
    world.tick(now);
}
