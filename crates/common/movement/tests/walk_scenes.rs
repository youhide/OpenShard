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

use openshard_movement::scene::{SIDE, Scene};
use openshard_movement::{MAX_STEP_UP, MapTerrain, PLAYER_HEIGHT};
use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;
use openshard_tiles::TileFlags;

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
fn stands(terrain: &MapTerrain<'_>, scene: &Scene, x: u16, y: u16) -> Vec<i32> {
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

// ------------------------------------------------------------ the live layer

/// A villa's staircase, walked from the ground to its first floor.
///
/// **R3's own acceptance.** A house is not in the map's files at all — the
/// shard placed it this morning — so every tile it stands on is answered by the
/// map with the ground underneath, and the storey above is the live layer's to
/// offer. Until [`can_step`](openshard_movement::can_step) read that layer's
/// surfaces, a placed staircase was a picture and its first floor was
/// unreachable however correctly the overlay described them.
///
/// The geometry is multi `0x0064`'s, measured off the shipped file: stone
/// stairs (`PLATFORM | CLIMBABLE`, five tall) at `dz = 2`, wooden boards
/// (`PLATFORM`, height zero) at `dz = 7`, plaster walls (`BLOCK`, nineteen
/// tall) at the same `dz` as the boards.
///
/// The covers come out of [`Cover::of_static`] over a tiledata this test
/// declares, and never by hand: what is under test includes *which* cover that
/// reading lays, so a fixture that stated the answer would agree with itself.
/// The three layers compose in one order, over ground whose spans were baked.
///
/// `docs/map/navigation_spans.md`'s N3 asks for exactly this, and it is a
/// composition test rather than a step test: since N3 the map's half of a
/// landing is read off a [`SpanIndex`](openshard_movement::spans::SpanIndex)
/// instead of derived from the column's statics, and the live world still has
/// to be able to add a floor the bake does not know about, take away a tile the
/// bake says is fine, and hang a door that one reading walks through and the
/// other does not.
///
/// **The bake is deliberately not a fixture here.** A [`Scene`] carries one and
/// keeps it in step with its own map, so this walks the same two-layer
/// arrangement the shard does — with the third layer, which the bake is
/// constructed never to see, laid over the top.
#[test]
fn the_live_world_adds_takes_away_and_hangs_a_door_over_baked_spans() {
    use openshard_map::grid::Tile;
    use openshard_map::overlay::{Cover, Doors, Overlay};
    use openshard_movement::{Footing, can_step};

    // Flat ground at zero with nothing on it: every column here is the *bare*
    // tier of the bake — no span is stored for it at all — which is the tier
    // 92% of a facet is and the one an overlay has to be able to overrule.
    let scene = Scene::flat(0);
    let from = Point::new(1, 1, 0);

    // Bare ground first, so each claim below is a change from a known answer.
    let nothing = Overlay::default();
    let bare = Footing::new(Some(scene.terrain()), &nothing, Doors::AsTheyStand);
    for tile in [(2, 1), (3, 1), (4, 1)] {
        assert_eq!(
            can_step(&bare, from, Point::new(tile.0, tile.1, 0)),
            Some(Point::new(tile.0, tile.1, 0)),
            "the ground at {tile:?} is ground before anything is laid on it"
        );
    }

    let mut live = Overlay::default();
    // A deck two above the ground: in reach of a body standing on it (a step
    // reaches `start_top + 2`), and higher than the ground it covers, which is
    // what makes it the surface rather than a no-op.
    live.set(Tile::new(2, 1), vec![Cover::standing(2, 0)]);
    // A crate in the body's own span on the next tile.
    live.set(Tile::new(3, 1), vec![Cover::blocking(0, 20)]);
    // And a shut door on the third.
    live.set(Tile::new(4, 1), vec![Cover::door(0, 20)]);

    let walked = Footing::new(Some(scene.terrain()), &live, Doors::AsTheyStand);
    assert_eq!(
        can_step(&walked, from, Point::new(2, 1, 0)),
        Some(Point::new(2, 1, 2)),
        "a deck the live world laid over bare ground is what a body stands on"
    );
    assert_eq!(
        can_step(&walked, from, Point::new(3, 1, 0)),
        None,
        "a blocker in the body's own span refuses a step the ground allows"
    );
    assert_eq!(
        can_step(&walked, from, Point::new(4, 1, 0)),
        None,
        "a shut door is in the way of a body walking into it"
    );

    // The same ground, the same three covers, read by somebody who will open
    // what is shut: only the door changes its answer.
    let planned = walked.reading(Doors::AllOpen);
    assert_eq!(
        can_step(&planned, from, Point::new(4, 1, 0)),
        Some(Point::new(4, 1, 0)),
        "a route planned by a body that opens doors goes through the doorway"
    );
    assert_eq!(
        can_step(&planned, from, Point::new(3, 1, 0)),
        None,
        "a crate is not a door, and no reading of the doors moves it"
    );
    assert_eq!(
        can_step(&planned, from, Point::new(2, 1, 0)),
        Some(Point::new(2, 1, 2)),
        "and the deck is still the deck"
    );
}

/// A two-storey villa on flat ground: a stair up from the south, boards for a
/// first floor over three tiles of it, and a plaster wall standing on those
/// boards.
///
/// The scene and the overlay come back apart because they are held apart: the
/// map is what the client shipped and the house is what the shard laid over it.
/// Built once here because two tests need the same house — one for the step
/// rule that climbs it, one for the search that plans the climb.
fn a_villa() -> (Scene, openshard_map::overlay::Overlay) {
    use openshard_map::grid::Tile;
    use openshard_map::overlay::{Cover, Overlay};

    const STAIR: u16 = 0x0751;
    const BOARDS: u16 = 0x04AC;
    const PLASTER: u16 = 0x0203;

    let mut scene = Scene::flat(0);
    scene.art(STAIR, TileFlags::PLATFORM | TileFlags::CLIMBABLE, 5);
    scene.art(BOARDS, TileFlags::PLATFORM, 0);
    scene.art(PLASTER, TileFlags::BLOCK, 19);

    // The house, as the shard's index would hold it: the component list, laid
    // through the one reading of the art both ends of the wire use.
    let house: [(u16, u16, i8, u16); 5] = [
        (5, 5, 2, STAIR),
        (5, 4, 7, BOARDS),
        (4, 4, 7, BOARDS),
        (6, 4, 7, BOARDS),
        (4, 3, 7, PLASTER),
    ];
    let mut live = Overlay::default();
    for (x, y, z, graphic) in house {
        let tile = Tile::new(x, y);
        let mut covers = live.at(tile).to_vec();
        covers.extend(Cover::of_static(scene.tiles().static_tile(graphic)).based_at(z));
        live.set(tile, covers);
    }
    (scene, live)
}

#[test]
fn a_villa_stair_carries_a_body_to_its_first_floor() {
    use openshard_map::overlay::Doors;
    use openshard_movement::{Footing, can_step};

    let (scene, live) = a_villa();
    let footing = Footing::new(Some(scene.terrain()), &live, Doors::AsTheyStand);

    // Up: the ground, then the tread (half way up a five-tall stair based at
    // two), then the boards.
    let ground = Point::new(5, 6, 0);
    let tread = can_step(&footing, ground, Point::new(5, 5, 0)).expect("the stair is out of reach");
    assert_eq!(tread, Point::new(5, 5, 4), "you stand half way up the tread");
    let landing = can_step(&footing, tread, Point::new(5, 4, 0)).expect("the first floor is out of reach");
    assert_eq!(landing, Point::new(5, 4, 7), "the boards are where the storey is");

    // Along the first floor, and not through the wall standing on it.
    assert_eq!(
        can_step(&footing, landing, Point::new(4, 4, 0)),
        Some(Point::new(4, 4, 7)),
        "a body on the first floor fell back to the ground crossing it"
    );
    assert_eq!(
        can_step(&footing, Point::new(4, 4, 7), Point::new(4, 3, 0)),
        None,
        "the upper-storey wall is not in the way of a body on the storey"
    );

    // Down again, the same way.
    assert_eq!(
        can_step(&footing, landing, Point::new(5, 5, 0)),
        Some(Point::new(5, 5, 4))
    );
    assert_eq!(can_step(&footing, tread, ground), Some(ground));

    // And the ground floor is still the ground floor: a body walking under the
    // boards is not lifted onto them, and the wall that stands on them is not
    // in its way either.
    assert_eq!(
        can_step(&footing, Point::new(4, 5, 0), Point::new(4, 4, 0)),
        Some(Point::new(4, 4, 0)),
        "the storey above pulled a body up through its own floor"
    );
    // What is *not* asserted here: that the ground under (4, 3) stays open. A
    // plaster wall based at seven spans up to twenty-six, and a body sixteen
    // tall standing at zero is squarely inside it — correctly, and the reason a
    // real house's upper walls stand over its ground-floor walls rather than
    // over open room. The z-span leaving a lower storey open is
    // `openshard-housing`'s own test, at the height where the two spans miss.

    // The first floor is out of reach from the ground: there is a staircase for
    // that, and a body does not step seven units into the air.
    assert_eq!(
        can_step(&footing, Point::new(6, 5, 0), Point::new(6, 4, 0)),
        Some(Point::new(6, 4, 0)),
        "the boards were climbed from the ground beside them"
    );
}

/// The `0x22` walk acknowledgement carries no position, so the client has to
/// predict the same live surfaces the shard just used.  Reading the bare map
/// here leaves a body at ground level: neither this stair nor this first floor
/// exists in the facet files, because the villa was placed at runtime.
#[test]
fn a_client_predicts_the_height_of_a_placed_villas_stair_and_floor() {
    use openshard_map::grid::Tile;
    use openshard_map::overlay::Doors;
    use openshard_movement::{Footing, predict_step};

    let (scene, live) = a_villa();
    let footing = Footing::new(Some(scene.terrain()), &live, Doors::AsTheyStand);
    let ground = Point::new(5, 6, 0);

    let tread_z = predict_step(&footing, ground, Tile::new(5, 5));
    assert_eq!(tread_z, 4, "the live stair was ignored in the predicted z");

    let tread = Point::new(5, 5, tread_z as i8);
    let floor_z = predict_step(&footing, tread, Tile::new(5, 4));
    assert_eq!(floor_z, 7, "the live first floor was ignored in the predicted z");

    assert_eq!(
        scene.terrain().predict_step(ground, 5, 5),
        0,
        "the fixture must prove that the static map alone cannot predict the placed house",
    );
}

/// And the search plans that climb, for a body standing under the floor it is
/// sent to.
///
/// `(4, 4)` carries two places to stand — the ground, and the boards seven
/// above it — and the route between them leaves the column and comes back:
/// out to the stair, up onto the first floor, and back west over the same
/// tile. **Before `navigation_spans.md`'s N3b this was answered with success
/// and an empty route**, because the search compared the goal's *tile* to the
/// start's and found them equal; the body then stood where it was, believing
/// it was upstairs. A tile-keyed `closed` could not have found the real route
/// either — the column is finalised by the first pop, so the return is
/// forbidden by the search's own bookkeeping rather than by the house.
#[test]
fn a_route_climbs_from_a_villas_ground_floor_to_its_first_floor() {
    use openshard_map::overlay::Doors;
    use openshard_movement::{Footing, Weight, find_path, step_allowed};

    let (scene, live) = a_villa();
    let footing = Footing::new(Some(scene.terrain()), &live, Doors::AsTheyStand);
    let under = Point::new(4, 4, 0);
    let upstairs = Point::new(4, 4, 7);

    let route = find_path(&footing, under, upstairs, 200, Weight::EXACT).expect("the villa has a staircase");
    assert_eq!(
        route,
        vec![Direction::SouthEast, Direction::NorthWest],
        "the way up is onto the stair and back over one's own column",
    );
    // Walked by the shipped step rule: a route the search invented that the
    // step rule refuses is worse than no route at all.
    let mut at = under;
    for &dir in &route {
        at = step_allowed(&footing, at, dir).expect("the search planned a step nobody may take");
    }
    assert_eq!(at, upstairs, "the loop comes home one storey up");

    // Both directions, because the step rule is not symmetric and the closed
    // set is what used to decide this: coming down, the boards are climbed
    // again from the tread, so the way back to the ground is round the house.
    let down = find_path(&footing, upstairs, under, 200, Weight::EXACT).expect("there is a way down");
    let mut at = upstairs;
    for &dir in &down {
        at = step_allowed(&footing, at, dir).expect("the search planned a step nobody may take");
    }
    assert_eq!(at, under, "the way down does not end on the floor it started on");
}
