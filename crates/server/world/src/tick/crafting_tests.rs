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
use openshard_protocol::item_kind::{ItemKindId, MaterialId};
use openshard_protocol::serial::RawSerial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::version::ClientVersion;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_state::components::{Contained, CraftedBy, Crafting, ItemKind, Material, Quality, Tool};
use openshard_state::harvest::ORE_GRAPHIC;
use openshard_state::{CraftGumpContext, CraftGumpPage, Skill};

/// A smith's tongs, one of the graphics that opens the blacksmithy window.
const TONGS: Graphic = Graphic(0x0FBB);
/// A sewing kit — a trade that needs no workshop, which is the contrast.
const SEWING_KIT: Graphic = Graphic(0x0F9D);
/// Fletcher's tools, sold by every bowyer.
const FLETCHER_TOOLS: Graphic = Graphic(0x1022);
/// Tinker's tools, which make the registered metal tool family.
const TINKER_TOOLS: Graphic = Graphic(0x1EB8);
/// A regular wooden board.
const BOARD: Graphic = Graphic(0x1BD7);
/// One wooden shaft.
const SHAFT: Graphic = Graphic(0x1BD4);
/// A leather hide, used by tailoring's material axis.
const LEATHER: Graphic = Graphic(0x1081);
/// A bird's feather.
const FEATHER: Graphic = Graphic(0x1BD1);
/// The ammunition a bow consumes.
const ARROW: Graphic = Graphic(0x0F3F);
/// An anvil's static id, ServUO's `4015`.
const ANVIL: Graphic = Graphic(4015);
/// A forge's, `4017`.
const FORGE: Graphic = Graphic(4017);
/// An iron ingot.
const INGOT: Graphic = openshard_crafting::INGOT_GRAPHIC;
/// Valorite's hue — the top of the metal axis.
const VALORITE: Hue = Hue(0x08AB);
/// Oak's material hue, the second wood on the fletching axis.
const OAK: Hue = Hue(0x07DA);

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
    let mut scene = Scene::flat_holding(START.x + 4, START.y + 4, 0);
    for &(graphic, z) in statics {
        scene.put(START.x, START.y, z, graphic);
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
    craft_in_system(world, connection, tool, SystemId::new(0), recipe, sub_res, now);
}

/// Open one trade's craft window and press one recipe row.
fn craft_in_system(
    world: &mut World,
    connection: ConnectionId,
    tool: EntityId,
    system: SystemId,
    recipe: u16,
    sub_res: u8,
    now: Instant,
) {
    let player = world.state.players[&connection];
    let def = openshard_crafting::system(system).expect("craft system");
    let group = def.recipes[usize::from(recipe)].group;
    openshard_crafting::open(
        &mut world.state,
        player,
        CraftGumpContext {
            system: system.index() as u8,
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
fn output_serial_exhaustion_currently_spends_successful_craft_ingredients() {
    // This characterises the non-atomic seam A5 must remove. The successful
    // roll consumes the ingots before output placement asks the exhausted item
    // allocator for a serial. A5 changes the target expectation to unchanged
    // ingredients and an unchanged logical-state snapshot.
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    shop(&mut world, &[(FORGE.0, 0), (ANVIL.0, 0)]);
    let tongs = give(&mut world, connection, TONGS, Hue(0), 1);
    let (recipe, _) = cheapest_smithing();
    let def = openshard_crafting::system(SystemId::new(0)).expect("blacksmithy");
    let wanted = def.recipes[usize::from(recipe)].resources[0].amount;
    let made = def.recipes[usize::from(recipe)].graphic;
    give(&mut world, connection, INGOT, Hue(0), wanted);
    train(&mut world, connection, Skill::Blacksmith, 1000);
    now += TICK_INTERVAL;
    world.tick(now);
    let mut crafted = world.state.bus.cursor_at_end::<openshard_crafting::ItemCrafted>();
    world
        .state
        .registry
        .reserve_serial(
            Serial::new(openshard_protocol::serial::ITEM_MAX).expect("the final item serial"),
        );

    craft(&mut world, connection, tongs, recipe, 0, now);
    finish(&mut world, connection, now);

    assert_eq!(
        carried(&world, connection, INGOT, Hue(0)),
        0,
        "the current commit order spends the ingredients before placement fails"
    );
    assert_eq!(
        carried(&world, connection, made, Hue(0)),
        0,
        "the exhausted allocator cannot place the result"
    );
    assert_eq!(
        world.state.bus.read(&mut crafted).count(),
        0,
        "an unplaced result is not reported as crafted"
    );
    assert!(openshard_state::audit_item_graph(&world.state).is_empty());
}

#[test]
fn tinkering_turns_a_typed_ingot_into_a_typed_metal_tool() {
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let tool = give(&mut world, connection, TINKER_TOOLS, Hue::NONE, 1);
    let tinkering = openshard_crafting::system(SystemId::new(3)).expect("tinkering");
    let recipe = u16::try_from(
        tinkering
            .recipes
            .iter()
            .position(|recipe| recipe.kind == Some(ItemKindId(19)))
            .expect("typed smith hammer recipe"),
    )
    .unwrap();
    let wanted = tinkering.recipes[usize::from(recipe)].resources[0].amount;
    give(&mut world, connection, INGOT, VALORITE, wanted);
    train(&mut world, connection, Skill::Tinkering, 1000);
    now += TICK_INTERVAL;
    world.tick(now);

    craft_in_system(
        &mut world,
        connection,
        tool,
        SystemId::new(3),
        recipe,
        8, // valorite in the metal axis
        now,
    );
    finish(&mut world, connection, now);

    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).unwrap();
    let pack = items::backpack_of(&world.state, owner).unwrap();
    assert!(world.state.registry.query::<Contained>().any(|(item, held)| {
        held.container == pack
            && world.state.registry.get::<ItemKind>(item) == Some(&ItemKind(ItemKindId(19)))
            && world.state.registry.get::<Material>(item) == Some(&Material(MaterialId(9)))
            && world.state.registry.has::<Tool>(item)
    }));
}

#[test]
fn smithing_turns_valorite_ingots_into_typed_ringmail_with_its_material_bonus() {
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    shop(&mut world, &[(FORGE.0, 0), (ANVIL.0, 0)]);
    let tongs = give(&mut world, connection, TONGS, Hue::NONE, 1);
    let smithing = openshard_crafting::system(SystemId::new(0)).expect("blacksmithy");
    let recipe = u16::try_from(
        smithing
            .recipes
            .iter()
            .position(|recipe| recipe.kind == Some(ItemKindId(43)))
            .expect("typed ringmail gloves recipe"),
    )
    .unwrap();
    let wanted = smithing.recipes[usize::from(recipe)].resources[0].amount;
    give(&mut world, connection, INGOT, VALORITE, wanted);
    train(&mut world, connection, Skill::Blacksmith, 1000);
    now += TICK_INTERVAL;
    world.tick(now);

    craft(&mut world, connection, tongs, recipe, 8, now);
    finish(&mut world, connection, now);

    let ringmail = world
        .state
        .registry
        .query::<ItemKind>()
        .find_map(|(item, kind)| (kind.0 == ItemKindId(43)).then_some(item))
        .expect("typed ringmail gloves");
    assert_eq!(
        world.state.registry.get::<Material>(ringmail),
        Some(&Material(MaterialId(9)))
    );
    assert_eq!(openshard_state::armor::piece_rating(&world.state, ringmail), 38);
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
    let def = openshard_crafting::system(SystemId::new(0)).unwrap();
    let recipe = u16::try_from(
        def.recipes
            .iter()
            .position(|recipe| recipe.kind == Some(ItemKindId(4)))
            .expect("the typed longsword recipe"),
    )
    .unwrap();
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

    // Same client art and valorite hue is still not a crafting material when a
    // semantic component says it is another kind. This is the invariant the
    // ItemKind migration is for; a hue-only consumer would accept this pile.
    let impostor = give(&mut world, connection, INGOT, VALORITE, wanted * 2);
    world.state.registry.insert(impostor, ItemKind(ItemKindId(999)));
    world.state.registry.insert(impostor, Material(MaterialId(9)));
    craft(&mut world, connection, tongs, recipe, valorite, now);
    assert_eq!(carried(&world, connection, made, VALORITE), 0);

    // With valorite in the pack it goes ahead, and the blade comes out the colour
    // of the metal it was made from.
    give(&mut world, connection, INGOT, VALORITE, wanted * 2);
    craft(&mut world, connection, tongs, recipe, valorite, now);
    finish(&mut world, connection, now);
    assert_eq!(carried(&world, connection, made, VALORITE), 1);
    let blade = world
        .state
        .registry
        .query::<Drawn>()
        .find_map(|(item, drawn)| {
            (*drawn
                == Drawn {
                    id: Graphic(0x0F61),
                    hue: VALORITE,
                })
            .then_some(item)
        })
        .expect("the typed longsword was placed");
    assert_eq!(
        world.state.registry.get::<ItemKind>(blade),
        Some(&ItemKind(ItemKindId(4))),
        "the recipe defines its result independently of its classic art"
    );
    assert_eq!(
        world.state.registry.get::<Material>(blade),
        Some(&Material(MaterialId(9))),
        "the output inherits the selected ingot material, not its hue"
    );
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
fn tailoring_turns_spined_leather_into_typed_armour_with_its_material_bonus() {
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let kit = give(&mut world, connection, SEWING_KIT, Hue::NONE, 1);
    let tailoring_index = openshard_crafting::SYSTEMS
        .iter()
        .position(|def| def.skill == Skill::Tailoring)
        .expect("tailoring system");
    let tailoring = &openshard_crafting::SYSTEMS[tailoring_index];
    let recipe = u16::try_from(
        tailoring
            .recipes
            .iter()
            .position(|recipe| recipe.kind == Some(ItemKindId(99)))
            .expect("typed leather chest recipe"),
    )
    .unwrap();
    let wanted = tailoring.recipes[usize::from(recipe)].resources[0].amount;
    give(&mut world, connection, LEATHER, Hue(0x08AC), wanted);
    train(&mut world, connection, Skill::Tailoring, 1000);
    now += TICK_INTERVAL;
    world.tick(now);

    craft_in_system(
        &mut world,
        connection,
        kit,
        SystemId::from_index(tailoring_index).unwrap(),
        recipe,
        1, // spined leather
        now,
    );
    finish(&mut world, connection, now);

    let chest = world
        .registry()
        .query::<ItemKind>()
        .find_map(|(item, kind)| (kind.0 == ItemKindId(99)).then_some(item))
        .expect("typed leather chest");
    assert_eq!(
        world.registry().get::<Material>(chest),
        Some(&Material(MaterialId(41)))
    );
    assert_eq!(openshard_state::armor::piece_rating(&world.state, chest), 23);
}

#[test]
fn a_fletcher_turns_boards_and_feathers_into_every_affordable_arrow() {
    // Both steps are batch crafts: all boards become shafts first, then the
    // smaller of the shaft and feather piles decides the ammunition result.
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    shop(&mut world, &[]);
    let tool = give(&mut world, connection, FLETCHER_TOOLS, Hue(0), 1);
    give(&mut world, connection, BOARD, OAK, 12);
    give(&mut world, connection, FEATHER, Hue(0), 8);
    train(&mut world, connection, Skill::Fletching, 1000);
    now += TICK_INTERVAL;
    world.tick(now);

    let system = openshard_crafting::SYSTEMS
        .iter()
        .position(|def| def.skill == Skill::Fletching)
        .expect("fletching is a craft system");
    let def = &openshard_crafting::SYSTEMS[system];
    let shaft_recipe = def
        .recipes
        .iter()
        .position(|recipe| recipe.graphic == SHAFT)
        .expect("the fletching table makes shafts");
    let arrow_recipe = def
        .recipes
        .iter()
        .position(|recipe| recipe.graphic == ARROW)
        .expect("the fletching table makes arrows");
    let uses_before = world.state.registry.get::<Tool>(tool).unwrap().uses_left;
    let player = world.state.players[&connection];

    assert!(
        openshard_crafting::begin(
            &mut world.state,
            player,
            tool,
            SystemId::from_index(system).unwrap(),
            u16::try_from(shaft_recipe).unwrap(),
            1,
        ),
        "shaft-making starts without a workshop"
    );
    now = finish(&mut world, connection, now);
    assert_eq!(carried(&world, connection, SHAFT, Hue(0)), 12);

    assert!(
        openshard_crafting::begin(
            &mut world.state,
            player,
            tool,
            SystemId::from_index(system).unwrap(),
            u16::try_from(arrow_recipe).unwrap(),
            1,
        ),
        "arrow-making starts without a workshop"
    );
    finish(&mut world, connection, now);

    assert_eq!(carried(&world, connection, ARROW, Hue(0)), 8);
    assert_eq!(carried(&world, connection, BOARD, OAK), 0);
    assert_eq!(carried(&world, connection, SHAFT, Hue(0)), 4);
    assert_eq!(carried(&world, connection, FEATHER, Hue(0)), 0);
    assert!(world.registry().query::<Contained>().any(|(item, held)| {
        held.container
            == items::backpack_of(
                &world.state,
                world
                    .registry()
                    .serial_of(world.state.players[&connection])
                    .unwrap(),
            )
            .unwrap()
            && world.registry().get::<ItemKind>(item) == Some(&ItemKind(ItemKindId(41)))
    }));
    assert_eq!(
        world.state.registry.get::<Tool>(tool).unwrap().uses_left,
        uses_before - 2,
        "each batch costs one tool use"
    );

    // A bow is a material-bearing weapon, unlike its material-less arrows.
    // Oak boards must therefore produce an oak bow identity and not merely a
    // bow graphic tinted oak.
    give(&mut world, connection, BOARD, OAK, 7);
    let bow_recipe = def
        .recipes
        .iter()
        .position(|recipe| recipe.kind == Some(ItemKindId(91)))
        .expect("the fletching table makes typed bows");
    assert!(openshard_crafting::begin(
        &mut world.state,
        player,
        tool,
        SystemId::from_index(system).unwrap(),
        u16::try_from(bow_recipe).unwrap(),
        1,
    ));
    finish(&mut world, connection, now);
    assert!(world.registry().query::<Contained>().any(|(item, held)| {
        held.container
            == items::backpack_of(
                &world.state,
                world
                    .registry()
                    .serial_of(world.state.players[&connection])
                    .unwrap(),
            )
            .unwrap()
            && world.registry().get::<ItemKind>(item) == Some(&ItemKind(ItemKindId(91)))
            && world.registry().get::<Material>(item) == Some(&Material(MaterialId(21)))
    }));
}

#[test]
fn fletching_requires_the_semantic_board_kind() {
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let impostor = give(&mut world, connection, BOARD, OAK, 1);
    world.state.registry.insert(impostor, ItemKind(ItemKindId(999)));
    world.state.registry.insert(impostor, Material(MaterialId(21)));
    train(&mut world, connection, Skill::Fletching, 1000);
    now += TICK_INTERVAL;
    world.tick(now);

    let system = openshard_crafting::SYSTEMS
        .iter()
        .position(|def| def.skill == Skill::Fletching)
        .expect("fletching is a craft system");
    let shaft_recipe = openshard_crafting::SYSTEMS[system]
        .recipes
        .iter()
        .position(|recipe| recipe.graphic == SHAFT)
        .expect("the fletching table makes shafts");
    let player = world.state.players[&connection];
    let def = &openshard_crafting::SYSTEMS[system];
    assert!(
        matches!(
            openshard_crafting::consume::check(&world.state, player, def, &def.recipes[shaft_recipe], 1),
            Err(openshard_crafting::Refusal::NotEnough(_))
        ),
        "same art and wood hue with a different kind cannot pay for a board"
    );

    // Old saves have no components yet, but their exact audited projection is
    // accepted once; normal construction will install ItemKind(36) instead.
    give(&mut world, connection, BOARD, OAK, 1);
    assert!(
        openshard_crafting::consume::check(&world.state, player, def, &def.recipes[shaft_recipe], 1).is_ok()
    );
}

#[test]
fn one_crafted_arrow_joins_the_stack_already_in_the_pack() {
    // `use_all_res` is what identifies a stacking recipe even when only one set
    // of inputs is present. Without it the one-arrow case takes the discrete-item
    // path and leaves an unstackable duplicate beside the existing pile.
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let tool = give(&mut world, connection, FLETCHER_TOOLS, Hue(0), 1);
    let existing_arrows = give(&mut world, connection, ARROW, Hue(0), 5);
    world
        .state
        .registry
        .insert(existing_arrows, ItemKind(ItemKindId(41)));
    give(&mut world, connection, SHAFT, Hue(0), 1);
    give(&mut world, connection, FEATHER, Hue(0), 1);
    // Enough to guarantee an arrow, but not enough to work the selected oak.
    // The wood axis must not gate a recipe made only from shafts and feathers.
    train(&mut world, connection, Skill::Fletching, 400);
    now += TICK_INTERVAL;
    world.tick(now);

    let system = openshard_crafting::SYSTEMS
        .iter()
        .position(|def| def.skill == Skill::Fletching)
        .unwrap();
    let recipe = openshard_crafting::SYSTEMS[system]
        .recipes
        .iter()
        .position(|recipe| recipe.graphic == ARROW)
        .unwrap();
    let player = world.state.players[&connection];
    assert!(openshard_crafting::begin(
        &mut world.state,
        player,
        tool,
        SystemId::from_index(system).unwrap(),
        u16::try_from(recipe).unwrap(),
        1,
    ));
    finish(&mut world, connection, now);

    let owner = world.state.registry.serial_of(player).unwrap();
    let pack = items::backpack_of(&world.state, owner).unwrap();
    let arrow_piles = openshard_state::contained_items(&world.state, pack)
        .filter(|(item, _)| {
            world
                .state
                .registry
                .get::<Drawn>(*item)
                .is_some_and(|drawn| drawn.id == ARROW)
        })
        .count();
    assert_eq!(carried(&world, connection, ARROW, Hue(0)), 6);
    assert_eq!(arrow_piles, 1, "the crafted arrow merged onto the pile");
}

#[test]
fn double_clicking_fletchers_tools_opens_the_fletching_window() {
    let mut now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let tool = give(&mut world, connection, FLETCHER_TOOLS, Hue(0), 1);
    now += TICK_INTERVAL;
    world.tick(now);
    let _ = packets_for(&mut world, connection);

    let serial = world.state.registry.serial_of(tool).unwrap();
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(serial.raw())),
    });
    now += TICK_INTERVAL;
    world.tick(now);

    let player = world.state.players[&connection];
    let context = world
        .state
        .row_of(player)
        .and_then(|row| row.craft_gump)
        .expect("the server remembers the fletching window");
    let def = openshard_crafting::system(SystemId::new(context.system)).unwrap();
    assert_eq!(def.skill, Skill::Fletching);
    assert!(
        packets_for(&mut world, connection)
            .iter()
            .any(|packet| packet[0] == 0xB0),
        "the client receives the craft gump"
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
    let ingot = world
        .state
        .registry
        .query::<ItemKind>()
        .find(|(_, kind)| **kind == ItemKind(ItemKindId(1)))
        .map(|(entity, _)| entity)
        .expect("smelting creates a semantic ingot");
    assert_eq!(
        world.state.registry.get::<Material>(ingot),
        Some(&Material(MaterialId(9)))
    );
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
            x: START.x,
            y: START.y,
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
            x: START.x,
            y: START.y,
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

    let typed_valorite = items::spawn_item_kind(
        &mut world.state,
        ItemKindId(5),
        Some(MaterialId(9)),
        1,
        false,
        Point::new(START.x, START.y, 0),
        Facet(0),
    )
    .expect("typed plate chest");
    assert_eq!(
        openshard_state::armor::piece_rating(&world.state, typed_valorite),
        plain + 16,
        "a migrated armour reader uses the kind definition and the material component"
    );
    now += TICK_INTERVAL;
    world.tick(now);
}

#[test]
fn a_semantic_item_record_restores_without_reinterpreting_its_art() {
    let mut source = world();
    let item = items::spawn_item_kind(
        &mut source.state,
        ItemKindId(4),       // longsword
        Some(MaterialId(9)), // valorite
        1,
        false,
        Point::new(START.x, START.y, 0),
        Facet(0),
    )
    .expect("typed item spawn");
    let serial = source.state.registry.serial_of(item).expect("item serial");
    let record = World::item_record(
        &source.state.registry,
        item,
        None,
        openshard_persistence::ItemLocation::Ground {
            facet: 0,
            x: START.x,
            y: START.y,
            z: 0,
        },
    )
    .expect("typed item saves");
    assert_eq!(record.kind, Some(4));
    assert_eq!(record.material, Some(9));

    let mut restored = world();
    let characters = restored.restore_characters(Vec::new());
    restored.restore_items(vec![record], &characters);
    let item = restored.state.registry.entity_of(serial).expect("restored item");
    assert_eq!(
        restored.state.registry.get::<ItemKind>(item),
        Some(&ItemKind(ItemKindId(4)))
    );
    assert_eq!(
        restored.state.registry.get::<Material>(item),
        Some(&Material(MaterialId(9)))
    );
    assert_eq!(
        restored.state.registry.get::<Drawn>(item),
        Some(&Drawn {
            id: Graphic(0x0F61),
            hue: VALORITE,
        })
    );
}

#[test]
fn a_pre_item_kind_record_migrates_through_its_audited_presentation() {
    let mut source = world();
    let item = items::spawn_item_kind(
        &mut source.state,
        ItemKindId(2),       // ore
        Some(MaterialId(9)), // valorite
        4,
        true,
        Point::new(START.x, START.y, 0),
        Facet(0),
    )
    .expect("typed ore spawn");
    let serial = source.state.registry.serial_of(item).expect("item serial");
    let mut record = World::item_record(
        &source.state.registry,
        item,
        None,
        openshard_persistence::ItemLocation::Ground {
            facet: 0,
            x: START.x,
            y: START.y,
            z: 0,
        },
    )
    .expect("ore saves");
    // This is precisely what a v34-or-earlier record contains: the visible
    // client projection but no semantic columns.
    record.kind = None;
    record.material = None;

    let mut restored = world();
    let characters = restored.restore_characters(Vec::new());
    restored.restore_items(vec![record], &characters);
    let ore = restored.state.registry.entity_of(serial).expect("restored ore");
    assert_eq!(
        restored.state.registry.get::<ItemKind>(ore),
        Some(&ItemKind(ItemKindId(2)))
    );
    assert_eq!(
        restored.state.registry.get::<Material>(ore),
        Some(&Material(MaterialId(9)))
    );
    assert_eq!(
        restored.state.registry.get::<Drawn>(ore),
        Some(&Drawn {
            id: ORE_GRAPHIC,
            hue: VALORITE,
        }),
        "migration retains the old record's visible projection"
    );
}

#[test]
fn a_corrupt_semantic_record_is_not_retyped_from_its_drawing() {
    let mut source = world();
    let item = items::spawn_item_kind(
        &mut source.state,
        ItemKindId(4),       // longsword
        Some(MaterialId(9)), // valorite
        1,
        false,
        Point::new(START.x, START.y, 0),
        Facet(0),
    )
    .expect("typed sword spawn");
    let serial = source.state.registry.serial_of(item).expect("item serial");
    let mut record = World::item_record(
        &source.state.registry,
        item,
        None,
        openshard_persistence::ItemLocation::Ground {
            facet: 0,
            x: START.x,
            y: START.y,
            z: 0,
        },
    )
    .expect("sword saves");
    // A bad external edit says this is a longsword, but its retained client art
    // is a plate chest. Restore must preserve the evidence for diagnosis rather
    // than guess either identity from it.
    record.graphic = 0x1415;
    record.hue = VALORITE.0;

    let mut restored = world();
    let characters = restored.restore_characters(Vec::new());
    restored.restore_items(vec![record], &characters);
    let item = restored.state.registry.entity_of(serial).expect("restored item");
    assert!(
        !restored.state.registry.has::<ItemKind>(item),
        "a corrupt semantic save must not silently become another registered kind"
    );
    assert_eq!(
        restored.state.registry.get::<Drawn>(item),
        Some(&Drawn {
            id: Graphic(0x1415),
            hue: VALORITE,
        })
    );
}

#[test]
fn double_clicking_the_tongs_is_what_opens_the_window() {
    // The whole way in. There is no craft packet: the client sends an ordinary
    // use, and everything after that is a gump — so a tool that answers a
    // double-click with nothing is a trade nobody can reach, which is what all
    // these were before this slice.
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
    let packets = packets_for(&mut world, connection);
    assert!(
        packets.iter().any(|packet| packet[0] == 0xB0),
        "and the client was sent one"
    );
    let workbench =
        packets
            .iter()
            .find_map(|packet| match ServerPacket::decode(packet, ClientVersion::TOL) {
                Ok(Some(ServerPacket::CraftWorkbench(workbench))) => Some(workbench),
                _ => None,
            });
    assert!(
        workbench.is_some(),
        "the normal craft gump has the typed egui workbench payload"
    );
    assert_eq!(
        workbench
            .as_ref()
            .and_then(|workbench| workbench.selected_material.as_ref())
            .map(|material| (material.item_kind, material.material)),
        Some((Some(ItemKindId(1)), Some(MaterialId(1)))),
        "the default iron row carries its semantic resource and material ids beside its render art"
    );
    assert!(
        matches!(
            workbench.as_ref().map(|workbench| &workbench.page),
            Some(openshard_protocol::craft::CraftWorkbenchPage::Items { recipes })
                if recipes.iter().any(|recipe| {
                    recipe.result.item_kind == Some(ItemKindId(5))
                        && recipe
                            .components
                            .iter()
                            .any(|component| component.item_kind == Some(ItemKindId(1)))
                })
        ),
        "a migrated recipe reports its declared input/output kinds, not identities inferred from art"
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
    for graphic in [TONGS, SEWING_KIT, FLETCHER_TOOLS] {
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

#[test]
fn the_private_catalogue_opens_without_a_tool_and_selects_a_recipe() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];

    world.queue(Command::OpenCraftCatalogue { connection });
    world.tick(now + TICK_INTERVAL);
    let catalogue = packets_for(&mut world, connection)
        .into_iter()
        .find_map(|packet| match ServerPacket::decode(&packet, ClientVersion::TOL) {
            Ok(Some(ServerPacket::CraftCatalogue(catalogue))) => Some(catalogue),
            _ => None,
        });
    assert!(catalogue.is_some_and(|catalogue| {
        catalogue.rows.iter().any(|row| {
            row.result_item_kind == Some(ItemKindId(5))
                && row.components.iter().any(|component| {
                    component.item_kind == Some(ItemKindId(1)) && component.material == Some(MaterialId(1))
                })
        })
    }));
    let context = world
        .state
        .row_of(player)
        .and_then(|row| row.craft_gump)
        .expect("the catalogue is an open craft gump");
    assert_eq!(context.page, CraftGumpPage::Catalogue);
    assert_eq!(context.tool, player, "browse mode has no item tool");

    let serial = world.state.registry.serial_of(player).unwrap();
    world.queue(Command::GumpResponse {
        connection,
        response: openshard_protocol::gump::GumpResponse {
            serial: openshard_protocol::gump::RawGumpKey(serial.raw()),
            gump_id: openshard_protocol::gump::RawGumpId(openshard_crafting::CRAFT_GUMP.0),
            // The first flattened catalogue row is a details button: `1 +
            // kind(2) + index(0) * 7`. It names the first smithing recipe,
            // without making a craft system a UI parent of the item.
            button: openshard_protocol::gump::RawButtonId(3),
            switches: Vec::new(),
            text_entries: Vec::new(),
        },
    });
    world.tick(now + TICK_INTERVAL * 2);
    let context = world
        .state
        .row_of(player)
        .and_then(|row| row.craft_gump)
        .expect("a selected recipe leaves the catalogue open");
    assert_eq!(context.system, 0, "the recipe's internal craft system");
    assert_eq!(context.page, CraftGumpPage::Details(0));
    assert_eq!(context.tool, player, "it remains browse-only");

    // The catalogue omits make buttons, but a client can always manufacture a
    // gump reply. The sentinel is therefore also checked at the craft gate: a
    // forged make press cannot turn browsing into free tool-less crafting.
    world.queue(Command::GumpResponse {
        connection,
        response: openshard_protocol::gump::GumpResponse {
            serial: openshard_protocol::gump::RawGumpKey(serial.raw()),
            gump_id: openshard_protocol::gump::RawGumpId(openshard_crafting::CRAFT_GUMP.0),
            button: openshard_protocol::gump::RawButtonId(1), // detail-page MAKE
            switches: Vec::new(),
            text_entries: Vec::new(),
        },
    });
    world.tick(now + TICK_INTERVAL * 3);
    assert!(
        !world.state.registry.has::<Crafting>(player),
        "a browse-only context cannot start a craft"
    );
}
