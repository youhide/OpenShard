//! Whole-tick benchmark for the level-of-detail (LOD) AI gate.
//!
//! The scripting benchmark measures a script call in isolation; this one times
//! the real thing — `World::tick` over a world full of wandering creatures, with
//! and without LOD. The load is deliberately lopsided: a knot of players in one
//! corner and thousands of creatures spread across a wide square, so most
//! creatures have no player near. That is exactly the shape LOD is for — an idle
//! frontier no one is watching.
//!
//! Run it release, or the numbers are meaningless:
//!
//! ```sh
//! cargo run -p openshard-world --example lod_bench --release
//! ```
//!
//! It prints, for each creature count and each of LOD off / LOD on: nanoseconds
//! per tick, milliseconds per tick, and what fraction of the 50ms tick budget
//! that is — then the speedup LOD bought. "Awake" is how many creatures sit
//! within `lod_radius` of the player cluster, the set LOD keeps thinking at full
//! rate; the rest doze.

use std::time::{Duration, Instant};

use openshard_gateway::ConnectionId;
use openshard_protocol::{AccessLevel, ClientVersion, Point};
use openshard_world::{Brain, Command, Gameplay, World, TICK_INTERVAL};

/// Britain, the same spot the tests use — a real, walkable patch of the map is
/// not needed here (dev mode allows every step), only a plausible coordinate.
const START: (u16, u16) = (1363, 1600);

/// The 50ms / 20Hz tick budget, to report each measurement as a fraction of.
const TICK_BUDGET: Duration = Duration::from_millis(50);

/// Rules with LOD off — every creature simulates at full rate.
fn lod_off() -> Gameplay {
    Gameplay {
        lod: false,
        ..Default::default()
    }
}

/// Rules with LOD on at the shipped defaults (radius 32, idle factor 8).
fn lod_on() -> Gameplay {
    Gameplay {
        lod: true,
        lod_radius: 32,
        lod_idle_factor: 8,
        ..Default::default()
    }
}

/// Build a world of `creatures` wandering monsters spread over a square with
/// `players` clustered near `START`, under `gameplay`. Warms a few ticks so the
/// per-creature timers settle, then returns the world and how many creatures
/// start within `lod_radius` of the cluster (the awake set).
fn populate(gameplay: Gameplay, creatures: u32, players: u32) -> (World, u32) {
    let radius = gameplay.lod_radius;
    let mut world = World::new(START).with_gameplay(gameplay);

    // A tight knot of players in one corner.
    for i in 0..players {
        world.queue(Command::Enter {
            connection: ConnectionId::from_raw(u64::from(i + 1)),
            version: ClientVersion::TOL,
            account: "bench".to_owned(),
            name: format!("Player{i}"),
            serial: None,
            position: Some(Point::new(START.0 + (i % 4) as u16, START.1, 0)),
            facet: 0,
            appearance: None,
            sheet: None,
            access: AccessLevel::Player,
        });
    }

    // Creatures on a grid four tiles apart, filling a square out from the corner.
    let side = (f64::from(creatures)).sqrt().ceil() as u32;
    let spacing = 4u16;
    let mut placed = 0u32;
    let mut awake = 0u32;
    'grid: for gy in 0..side {
        for gx in 0..side {
            if placed >= creatures {
                break 'grid;
            }
            let x = START.0 + (gx as u16).saturating_mul(spacing);
            let y = START.1 + (gy as u16).saturating_mul(spacing);
            // Chebyshev distance from the cluster; within the radius it is awake.
            if u32::from(x - START.0).max(u32::from(y - START.1)) <= radius {
                awake += 1;
            }
            world.queue(Command::SpawnMobile {
                body: 0x00D1, // a wandering creature that does not work door handles
                hue: 0,
                hits: 50,
                notoriety: 5,
                damage: 5,
                resistance: 0,
                swing: 0,
                sight: 10,
                aggression: 2,
                beat: 0,
                ranged: 0,
                ranged_kind: 0,
                wander: true,
                position: Point::new(x, y, 0),
                facet: 0,
                name: None,
                banker: false,
                vendor: false,
                equipment: Vec::new(),
                skills: Vec::new(),
            });
            placed += 1;
        }
    }

    // Apply the queued spawns and let the timers settle.
    let mut clock = Instant::now();
    for _ in 0..5 {
        clock += TICK_INTERVAL;
        world.tick(clock);
    }
    (world, awake)
}

/// Time `rounds` ticks and return the mean seconds per tick.
fn time_ticks(world: &mut World, rounds: u32) -> f64 {
    let mut clock = Instant::now();
    let start = Instant::now();
    for _ in 0..rounds {
        clock += TICK_INTERVAL;
        world.tick(clock);
    }
    start.elapsed().as_secs_f64() / f64::from(rounds)
}

fn report(label: &str, per_tick: f64) {
    let ns = per_tick * 1e9;
    let ms = per_tick * 1e3;
    let budget = per_tick / TICK_BUDGET.as_secs_f64() * 100.0;
    println!("    {label:<8}  {ns:>10.0} ns/tick  {ms:>8.3} ms/tick  {budget:>6.1}% of 50ms");
}

fn main() {
    const PLAYERS: u32 = 5;
    const ROUNDS: u32 = 60;

    println!("LOD whole-tick benchmark — {PLAYERS} players clustered, creatures spread\n");

    for &count in &[2_000u32, 10_000u32] {
        let (mut off, awake) = populate(lod_off(), count, PLAYERS);
        let (mut on, _) = populate(lod_on(), count, PLAYERS);

        // Confirm the load actually built.
        let brains = off.registry().query::<Brain>().count();

        println!(
            "  {count} creatures ({brains} brains, {awake} within LOD radius of the cluster):"
        );
        let off_per = time_ticks(&mut off, ROUNDS);
        let on_per = time_ticks(&mut on, ROUNDS);
        report("LOD off", off_per);
        report("LOD on", on_per);
        println!("    speedup   {:>10.2}x\n", off_per / on_per);
    }
}
