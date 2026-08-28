//! Whole-tick benchmark for a *populated town*, which is the load that actually
//! exists.
//!
//! `lod_bench` measures the AI gate: thousands of bare creatures spread thin, most
//! of them far from anyone. That is the right shape for the question it asks and
//! the wrong shape for almost every other cost in the tick. Its creatures are
//! spawned with `equipment: Vec::new()` and there is no decoration anywhere, so it
//! never touches two of the three things a real Felucca spends its tick on:
//!
//! - **Worn items.** A dressed townsperson carries five or six `Equipped` rows,
//!   and 726 of them is a column thousands long that some lookups filter whole.
//! - **A crowded sector grid.** Decoration and ground items share the index with
//!   mobiles, so `nearby` in a decorated town returns hundreds of entries to find
//!   a handful of neighbours.
//! - **Mobiles that move.** A stationary world never pays interest management.
//!
//! So this one builds the opposite shape: a dense block of dressed townsfolk with
//! thousands of decoration statics among them and players standing in the middle
//! of it, which is a market square at noon.
//!
//! Run it release, or the numbers are meaningless:
//!
//! ```sh
//! cargo run -p openshard-world --example town_bench --release
//! ```

use std::time::{Duration, Instant};

use openshard_gateway::ConnectionId;
use openshard_protocol::identity::{AccountName, CharacterName};
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::{Aggression, DamageType, Facet, Point, Sight};
use openshard_protocol::{access::AccessLevel, version::ClientVersion};
use openshard_world::{Character, Command, Entering, FreshCharacter, Gameplay, TICK_INTERVAL, World};

/// Britain, the same spot the tests use.
const START: openshard_map::grid::Tile = openshard_map::grid::Tile::new(1363, 1600);

/// The world's current tick budget, to report each measurement as a fraction of.
const TICK_BUDGET: Duration = TICK_INTERVAL;

/// A town of `folk` dressed townsfolk over a block, `decor` statics scattered
/// through it, and `players` standing in the middle.
///
/// Two tiles apart, which is roughly how ServUO's own town spawns sit — a market
/// square, not a parade ground. Everyone gets a `Title`, because that is what
/// makes `npc::dress` run and hang real worn items on them.
fn populate(gameplay: Gameplay, folk: u32, decor: u32, players: u32) -> World {
    let mut world = World::new(START).with_gameplay(gameplay);
    let side = (f64::from(folk)).sqrt().ceil() as u16;

    for i in 0..players {
        world.queue(Command::Enter(Entering {
            connection: ConnectionId::from_raw(u64::from(i + 1)),
            version: ClientVersion::TOL,
            account: AccountName("bench".to_owned()),
            name: CharacterName(format!("Player{i}")),
            access: AccessLevel::Player,
            character: Character::Fresh(FreshCharacter {
                facet: Facet(0),
                start: Some(Point::new(START.x + side + (i % 4) as u16, START.y + side, 0)),
                appearance: None,
                sheet: None,
            }),
        }));
    }

    let trades = [
        "the blacksmith",
        "the provisioner",
        "the tailor",
        "the baker",
        "the healer",
    ];
    let mut placed = 0u32;
    'grid: for gy in 0..side {
        for gx in 0..side {
            if placed >= folk {
                break 'grid;
            }
            world.queue(Command::SpawnMobile {
                body: Graphic(0x0190),
                hue: Hue(0),
                hits: 100,
                notoriety: Notoriety::from_bits(7),
                damage: 0,
                resistance: openshard_protocol::world::PhysicalResistance::new(0),
                swing: 0,
                sight: Sight(0),
                aggression: Aggression::from_bits(2),
                beat: 0,
                ranged: None,
                ranged_kind: DamageType::Physical,
                wander: false,
                position: Point::new(START.x + gx * 2, START.y + gy * 2, 0),
                facet: Facet(0),
                name: None,
                title: Some(trades[placed as usize % trades.len()].to_owned()),
                shoe: 1,
                fame: 0,
                karma: 0,
                night_home: None,
                banker: false,
                vendor: placed.is_multiple_of(3),
                healer: false,
                equipment: Vec::new(),
                skills: Vec::new(),
                stock: Vec::new(),
                escort_to: None,
                quests: Vec::new(),
            });
            placed += 1;
        }
    }

    // Decoration among them, on the odd tiles the townsfolk are not standing on,
    // so it lands in the same sectors and the same views.
    let mut statics = Vec::with_capacity(decor as usize);
    for i in 0..decor {
        let gx = (i % u32::from(side * 2)) as u16;
        let gy = (i / u32::from(side * 2)) as u16;
        statics.push((Graphic(0x0B4F), Hue(0), Point::new(START.x + gx, START.y + gy, 0)));
    }
    world.queue(Command::Decorate {
        facet: Facet(0),
        statics,
        doors: Vec::new(),
        containers: Vec::new(),
    });

    let mut clock = Instant::now();
    for _ in 0..5 {
        clock += TICK_INTERVAL;
        world.tick(clock);
    }
    world
}

/// Time `rounds` ticks and return the mean seconds per tick.
///
/// With `walking`, every player takes a step each tick, pacing back and forth
/// across the crowd. That is the case worth measuring rather than a still one: a
/// player who does not move is drawn once and never redrawn, so a standing
/// benchmark never pays interest management — no `refresh_around`, no first-sight
/// draw of a neighbour, none of the per-draw work of assembling what that
/// neighbour is wearing. Walking through a market square pays all of it, and it
/// is what a player actually does.
fn time_ticks(world: &mut World, rounds: u32, walking: bool) -> f64 {
    let movers: Vec<Serial> = if walking {
        world.player_serials()
    } else {
        Vec::new()
    };
    let mut clock = Instant::now();
    let start = Instant::now();
    for round in 0..rounds {
        // South for a while, then north, so the walk stays inside the town.
        let direction = if (round / 16) % 2 == 0 { 4 } else { 0 };
        for &serial in &movers {
            world.queue(Command::Step { serial, direction });
        }
        clock += TICK_INTERVAL;
        world.tick(clock);
    }
    start.elapsed().as_secs_f64() / f64::from(rounds)
}

fn report(label: &str, per_tick: f64) {
    let ns = per_tick * 1e9;
    let ms = per_tick * 1e3;
    let budget = per_tick / TICK_BUDGET.as_secs_f64() * 100.0;
    println!(
        "    {label:<10}{ns:>11.0} ns/tick  {ms:>8.3} ms/tick  {budget:>6.1}% of {}ms",
        TICK_BUDGET.as_millis()
    );
}

fn main() {
    const ROUNDS: u32 = 200;

    println!("Populated-town whole-tick benchmark\n");

    for &(folk, decor, players) in &[(200u32, 2_000u32, 5u32), (726, 8_000, 5), (726, 8_000, 25)] {
        let mut world = populate(Gameplay::default(), folk, decor, players);
        let worn = world
            .registry()
            .query::<openshard_state::components::Equipped>()
            .count();
        println!("  {folk} townsfolk ({worn} worn items), {decor} decoration, {players} players");
        report("standing", time_ticks(&mut world, ROUNDS, false));
        report("walking", time_ticks(&mut world, ROUNDS, true));
        println!();
    }
}
