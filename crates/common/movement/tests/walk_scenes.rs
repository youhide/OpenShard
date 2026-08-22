//! The step rule, walked over scenes this file builds — no client files needed.
//!
//! Two halves.
//!
//! The **regressions** are the three bugs a person found by walking Britain's
//! castle, each reduced to the few tiles that carry it. The real-map tests in
//! `terrain.rs` say those shapes exist in Britannia; these say what the rule does
//! with them, and they run on a machine that has never seen a client install —
//! which is every machine CI has.
//!
//! The **properties** are the same three claims made over scenes nobody wrote:
//! random ground, random stairs, random walls, every body position, every
//! direction. Each property is checked against the scene's own declaration —
//! this file placed those statics and knows their heights — and never against
//! [`MapTerrain`](openshard_movement::MapTerrain), which is the thing under test.
//! An oracle that asked the code what the answer should be would agree with it
//! always, including when it is wrong.

use openshard_map::map::WorldMap;
use openshard_movement::scene::{SIDE, Scene};
use openshard_movement::{MAX_STEP_UP, MapTerrain, PLAYER_HEIGHT, Terrain};
use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;
use openshard_uofiles::tiledata::TileData;

// ---------------------------------------------------------------- regressions

/// A staircase is climbed, and the client predicts the height the shard lands.
///
/// The stair is based level with the ground, so it is stepped onto at its base —
/// within a step — and stood on half way up. Both ends of the wire have to say
/// five: the shard because that is where the body is, the client because a `0x22`
/// carries no position and nothing else will ever correct it. `predict_z`, which
/// takes the surface *nearest* the height a body already has, says zero — the
/// floor the stair tile also carries — and that is a body drawn walking through
/// the staircase.
#[test]
fn a_stair_is_climbed_and_the_client_predicts_the_climb() {
    let mut scene = Scene::flat(0);
    scene.stair(1, 4, 0, 10);
    let terrain = scene.terrain();
    let from = Point::new(0, 4, 0);

    let landed = terrain
        .can_step(from, Point::new(1, 4, 0))
        .expect("a stair based at the ground is a step up, not a wall");
    assert_eq!(landed.z, 5, "you stand half way up a ten-high stair");
    assert_eq!(
        terrain.predict_step(from, 1, 4),
        i32::from(landed.z),
        "the client drew the body somewhere the shard did not put it",
    );
    assert_eq!(
        terrain.predict_z(1, 4, 0),
        0,
        "and the nearest-height guess is exactly the wrong answer this is about",
    );
}

/// A staircase is not entered from the side.
///
/// The stair here is based five above the ground, which puts it out of reach: a
/// step reaches two. What is left on the tile is the ground under the stairs, and
/// that is not somewhere to stand — the stair's own body is in it. Letting a
/// surface be waved through because "you can stand on it" put the body inside the
/// staircase at floor level.
#[test]
fn the_ground_under_a_staircase_is_not_a_way_in() {
    let mut scene = Scene::flat(0);
    scene.stair(1, 4, 5, 10);
    let terrain = scene.terrain();

    assert_eq!(
        terrain.can_step(Point::new(0, 4, 0), Point::new(1, 4, 0)),
        None,
        "walked into the tile under the stairs",
    );
    // The stair is reachable the way a stair is: from a height that reaches its
    // base. Standing at 3, a step reaches 5.
    let mut approach = Scene::flat(0);
    approach.stair(1, 4, 5, 10).floor(0, 4, 0, 3);
    assert_eq!(
        approach
            .terrain()
            .can_step(Point::new(0, 4, 3), Point::new(1, 4, 3))
            .map(|p| p.z),
        Some(10),
        "and from a step's reach of its base it is climbed",
    );
}

/// A wall at the height a body walks in at is a wall, however deep the pit
/// behind it.
///
/// Two tiles of the same wall, differing only in how far down the floor behind
/// them is. Measure the body from where it *lands* and the deeper one has its
/// head under the wall and reads as open — an eighteen-unit fall through a wall
/// that is at eye level on the way in. Measure it from where it walks in from and
/// both are what they look like.
///
/// The last assertion is what stops this test from passing vacuously: it hands
/// the oracle the landing the buggy code produced and requires it to be called
/// out. An oracle that cannot fail proves nothing about the code that passes it.
#[test]
fn a_wall_at_walking_height_is_a_wall_however_deep_the_pit() {
    // The terrace and the steps below it are *statics* on flat ground, the way
    // the castle's are. Sinking the land instead would not build a pit: a land
    // tile's corners are its neighbours' heights, so a single low tile among
    // high ones averages back up to nearly the height around it, and the body
    // would stand most of the way out of the hole it is supposed to fall into.
    let mut scene = Scene::flat(0);
    for x in 0..SIDE {
        scene.floor(x, 4, 40, 0);
    }
    for (x, step) in [(1u16, 27i8), (2, 22)] {
        scene.floor(x, 3, step, 0).wall(x, 3, 40, 9);
    }
    let terrain = scene.terrain();

    for x in 1..=2u16 {
        assert_eq!(
            terrain.surface_at(x, 4, 40),
            Some(40),
            "({x},4) is not the terrace this test walks off",
        );
        assert_eq!(
            terrain.can_step(Point::new(x, 4, 40), Point::new(x, 3, 40)),
            None,
            "({x},3) let a body walk into a wall standing at its own height",
        );
    }
    // The oracle can fail: the deep tile's floor is exactly what the old rule
    // landed a body on, and it is inside the wall.
    assert!(
        !clear_of_solids(&scene, 2, 3, 22, 40),
        "the oracle waved through the landing the bug produced",
    );
}

// ----------------------------------------------------------------- properties

/// Whether a body standing at `landing` on `(x, y)`, having walked in from
/// height `from_z`, has anything solid in it — read from the scene's own
/// statics, not from the rule under test.
///
/// The body occupies `landing` up to the higher of `landing` and `from_z` plus a
/// person's height: it has to *get* there, and it walks in at the height it
/// left. A surface tops out where a body standing on it stands, so the one
/// underfoot is never in the way; a wall's top is its art.
fn clear_of_solids(scene: &Scene, x: u16, y: u16, landing: i32, from_z: i32) -> bool {
    let head = landing.max(from_z) + PLAYER_HEIGHT;
    !scene.map().statics_at(x, y).any(|item| {
        let tile = scene.tiles().static_tile(item.tile.0);
        let platform = tile.flags.is_platform();
        if !platform && !tile.flags.is_blocking() {
            return false;
        }
        let base = i32::from(item.z);
        let height = i32::from(tile.height);
        let top = match (platform, tile.flags.is_climbable()) {
            (true, true) => base + height / 2,
            (true, false) => base + height,
            (false, _) => base + height.max(1),
        };
        top > landing && base < head
    })
}

/// How high a step from `(x, y, z)` reaches, from the scene's own declaration.
///
/// A step reaches two units above the *top* of what is underfoot, which is not
/// where a body's feet are: on a stair you stand half way up and step off the
/// whole of it. Deliberately generous — the top of anything the body is at or
/// above, not only the one surface the rule picked — because this bounds the
/// rule rather than restating it, and an oracle that reproduced the pick would
/// be the code again.
fn reach_from(scene: &Scene, x: u16, y: u16, z: i32) -> i32 {
    let mut top = z;
    // The land: you stand at the average of its corners and step off its highest.
    if let Some(corners) = scene.map().land_corners(x, y) {
        let land_stand = i32::from(openshard_map::map::average_corner_z(corners));
        if land_stand <= z {
            top = top.max(corners.iter().copied().map(i32::from).max().unwrap_or(z));
        }
    }
    for (base, height, stand, _) in surfaces(scene, x, y) {
        if stand <= z {
            top = top.max(base + height);
        }
    }
    top + MAX_STEP_UP
}

/// Every platform on a tile, as `(base, height, stand, step_onto)` — where a
/// body stands on it, and the edge a step has to reach to get on. They differ
/// only for a stair: you step onto its base and stand half way up.
fn surfaces(scene: &Scene, x: u16, y: u16) -> Vec<(i32, i32, i32, i32)> {
    scene
        .map()
        .statics_at(x, y)
        .filter_map(|item| {
            let tile = scene.tiles().static_tile(item.tile.0);
            if !tile.flags.is_platform() {
                return None;
            }
            let (base, height) = (i32::from(item.z), i32::from(tile.height));
            Some(match tile.flags.is_climbable() {
                true => (base, height, base + height / 2, base),
                false => (base, height, base + height, base + height),
            })
        })
        .collect()
}

/// Whether `landing` is a surface that is really on `(x, y)` and was really
/// within `reach` — the land's own average, or a platform's standing height.
///
/// This is the claim a step's *height* has to satisfy, and the one the reach
/// bound cannot make on its own: a stair legitimately lifts a body past
/// `reach`, because what has to be in reach is the base it is stepped onto and
/// not the height it is stood at.
fn justified(scene: &Scene, x: u16, y: u16, landing: i32, reach: i32) -> bool {
    if let Some(corners) = scene.map().land_corners(x, y) {
        let lowest = corners.iter().copied().map(i32::from).min().unwrap_or(landing);
        let average = i32::from(openshard_map::map::average_corner_z(corners));
        if average == landing && lowest <= reach {
            return true;
        }
    }
    surfaces(scene, x, y)
        .into_iter()
        .any(|(_, _, stand, step_onto)| stand == landing && step_onto <= reach)
}

/// One deterministic pseudo-random scene, and every body position on it.
///
/// SplitMix64: six lines, no dependency, and the same sequence on every machine
/// and every run — a scene that cannot be reproduced is a bug report nobody can
/// act on.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n`.
    fn upto(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A scene of low relief with stairs and walls scattered over it.
///
/// The shapes are the ones the bugs lived in and the numbers are small on
/// purpose: heights within a couple of body-heights of each other, so that walls
/// land *in* bodies rather than harmlessly above them, and stairs are reachable
/// often enough for a climb to be exercised rather than always refused.
fn random_scene(seed: u64) -> Scene {
    let mut rng = Rng(seed);
    let mut scene = Scene::flat(0);
    for y in 0..SIDE {
        for x in 0..SIDE {
            // Ground: mostly flat, sometimes a step or a pit.
            let ground = match rng.upto(4) {
                0 => rng.upto(21) as i8 - 10,
                _ => 0,
            };
            scene.ground(x, y, ground);
            for _ in 0..rng.upto(3) {
                let base = ground.saturating_add(rng.upto(25) as i8 - 5);
                let height = [0u8, 1, 3, 5, 10, 20][rng.upto(6) as usize];
                match rng.upto(3) {
                    0 => scene.stair(x, y, base, height),
                    1 => scene.floor(x, y, base, height),
                    _ => scene.wall(x, y, base, height),
                };
            }
        }
    }
    scene
}

/// Every height a body could be standing at on `(x, y)` — the land, and the top
/// of every platform — filtered to the ones the rule agrees are standing
/// positions. The filter is the rule's own answer on purpose: a body that is
/// somewhere it could not be is not a step this test has anything to say about.
fn stands(terrain: &MapTerrain<&WorldMap, &TileData>, scene: &Scene, x: u16, y: u16) -> Vec<i32> {
    let mut candidates = vec![];
    if let Some(corners) = scene.map().land_corners(x, y) {
        candidates.push(i32::from(openshard_map::map::average_corner_z(corners)));
    }
    for item in scene.map().statics_at(x, y) {
        let tile = scene.tiles().static_tile(item.tile.0);
        if tile.flags.is_platform() {
            let (base, height) = (i32::from(item.z), i32::from(tile.height));
            candidates.push(match tile.flags.is_climbable() {
                true => base + height / 2,
                false => base + height,
            });
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    candidates
        .into_iter()
        .filter(|&z| terrain.surface_at(x, y, z) == Some(z))
        .collect()
}

/// The three claims, over two hundred scenes nobody wrote.
///
/// 1. The client's predicted height is the shard's landing. This is the one that
///    broke first, and it is invisible on the wire: `0x22` carries no position.
/// 2. A landing never has anything solid in the body — checked against the
///    statics this file placed, at the height the body walks in at.
/// 3. A step never climbs higher than a step: two units above the top of the
///    surface underfoot, which for a stair is its full art.
///
/// The counters at the end are not decoration. Every one of them has a way of
/// silently going to zero — a generator that puts every static out of reach, a
/// scene where nothing is standable, an edge case that refuses every step — and
/// a sweep that checked nothing passes just as quietly as one that checked
/// everything.
#[test]
fn a_random_scene_is_walked_the_way_the_rules_say() {
    let (mut allowed, mut refused, mut climbs, mut drops, mut over_stairs, mut past_walls) =
        (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);

    for seed in 0..200u64 {
        let scene = random_scene(seed);
        let terrain = scene.terrain();
        for y in 0..SIDE {
            for x in 0..SIDE {
                for stand in stands(&terrain, &scene, x, y) {
                    let Ok(z) = i8::try_from(stand) else { continue };
                    let from = Point::new(x, y, z);
                    for direction in Direction::ALL {
                        let Some(to) = openshard_movement::step_from(from, direction) else {
                            continue;
                        };
                        if !scene.map().contains(to.x, to.y) {
                            continue;
                        }
                        let Some(landed) = terrain.can_step(from, to) else {
                            refused += 1;
                            continue;
                        };
                        allowed += 1;
                        let landing = i32::from(landed.z);

                        assert_eq!(
                            terrain.predict_step(from, to.x, to.y),
                            landing,
                            "seed {seed}: the client predicted a different height than the \
                             shard landed {from:?} -{direction:?}-> ({},{})",
                            to.x,
                            to.y,
                        );
                        assert!(
                            clear_of_solids(&scene, to.x, to.y, landing, stand),
                            "seed {seed}: {from:?} -{direction:?}-> ({},{}) landed at \
                             z={landing} with something solid in the body",
                            to.x,
                            to.y,
                        );
                        let reach = reach_from(&scene, x, y, stand);
                        assert!(
                            justified(&scene, to.x, to.y, landing, reach),
                            "seed {seed}: {from:?} -{direction:?}-> ({},{}) landed at \
                             z={landing}, which is no surface on that tile within the \
                             {reach} a step reaches",
                            to.x,
                            to.y,
                        );

                        if landing > stand {
                            climbs += 1;
                        }
                        if landing < stand - 5 {
                            drops += 1;
                        }
                        let tiles_here: Vec<_> = scene
                            .map()
                            .statics_at(to.x, to.y)
                            .map(|item| scene.tiles().static_tile(item.tile.0))
                            .collect();
                        if tiles_here.iter().any(|t| t.flags.is_climbable()) {
                            over_stairs += 1;
                        }
                        if tiles_here
                            .iter()
                            .any(|t| !t.flags.is_platform() && t.flags.is_blocking())
                        {
                            past_walls += 1;
                        }
                    }
                }
            }
        }
    }

    assert!(allowed > 5_000, "only {allowed} steps were allowed");
    assert!(refused > 5_000, "only {refused} steps were refused");
    assert!(climbs > 500, "only {climbs} steps went up");
    assert!(drops > 200, "only {drops} steps went down more than five");
    assert!(
        over_stairs > 500,
        "only {over_stairs} steps landed on a tile with a stair"
    );
    assert!(
        past_walls > 500,
        "only {past_walls} steps landed on a tile with a wall"
    );
}
