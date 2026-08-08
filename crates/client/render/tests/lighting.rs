//! What the light does in a room, tile by tile.
//!
//! These are the questions a screenshot cannot answer: is that tile dark because
//! no flame reaches it, or because a wall stopped one that would? The scenes are
//! built rather than loaded (`render/src/scene.rs`), the numbers come from
//! `light::sample` — the shader's own arithmetic in Rust, held to it by
//! `frame.rs`'s parity test — and a failure prints the room.
//!
//! No GPU and no client files: everything here runs everywhere, which is the
//! point of a scene that is a `Map` with three items on it.

use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::debug;
use openshard_client_render::facing::Face;
use openshard_client_render::geometry::Vec2;
use openshard_client_render::light::{self, Lighting, Spot};
use openshard_client_render::occlusion;
use openshard_client_render::scene::{self, CENTRE, DOORWAY, Scene};

/// The flicker's instant. Always zero: a flame's brightness swings by a tenth,
/// and an assertion about a leak must not depend on which tenth of a second it
/// was asked in.
const STILL: f32 = 0.0;

/// The ambient alone at one tile, as one number — what that tile comes out at
/// with no flame reaching it.
///
/// Per tile and no longer per frame, which is `docs/lighting_world.md`'s
/// decision 1 arriving in the tests: a tile under a roof, a wall tile whose own
/// column is shaded, and the tile beside it that the blur touched all get
/// different shares of the sky. A single constant would now be the right answer
/// only in the open, and every "this tile is the ambient exactly" assertion
/// below would be measuring the field instead of the leak it is about.
fn ambient(lighting: &Lighting, tile: (u16, u16)) -> f32 {
    let sky = lighting.occlusion.sky_at(i32::from(tile.0), i32::from(tile.1));
    let lit = lighting.ambient.at(sky);
    lit.iter().sum::<f32>() / lit.len() as f32
}

/// How much brighter one light quantity is than another, **as a displayed
/// value** — the domain every absolute margin in this file was authored in.
///
/// `docs/lighting_rebuild.md` phase 1 moved light into linear radiance, and in
/// doing so it silently changed what every `+ 0.2` below meant. They were chosen
/// against a picture — "distinguishably brighter than the ambient" — back when a
/// brightness was a fraction of a *displayed* value, and a fifth of a displayed
/// value is not a fifth of a radiance. Night's ambient is `0.019` of radiance and
/// `0.145` of a displayed value, so a margin left in the wrong domain is a margin
/// roughly eight times its author's intent: `a_hole_in_a_floor_lets_the_light_
/// through` went red on `0.037` against its `0.04` while the light coming up
/// through the hole was three times the ambient.
///
/// This is [`light::GROUND_AMBIENT`]'s treatment applied to the tests: the
/// authored number stays exactly as authored, and the conversion happens where it
/// is read. A margin is *not* converted where the claim is a ratio — twice the
/// light is a statement about radiance and means nothing about stored bytes — so
/// those stay in the linear domain, and say so.
fn brighter_by(lit: f32, than: f32) -> f32 {
    openshard_client_render::tonemap::linear_to_srgb(lit)
        - openshard_client_render::tonemap::linear_to_srgb(than)
}

/// How bright the middle of a tile is, at a height.
fn at(lighting: &Lighting, tile: (u16, u16), z: f32) -> f32 {
    light::sample(spot(tile, z), lighting).brightness()
}

/// The middle of a tile, at a height.
///
/// A point of **no occluder** — see [`on_the_static`] for the probe that is a
/// point of one. That is the honest default here: most of these probes are the
/// air or the floor of a room, and a point in the open is a point of nothing.
fn spot(tile: (u16, u16), z: f32) -> Spot {
    Spot::at(
        Vec2::new(f32::from(tile.0) + 0.5, f32::from(tile.1) + 0.5),
        z,
        (i32::from(tile.0), i32::from(tile.1)),
    )
}

/// How bright a point **of the static standing on `tile`** is, at a height —
/// what a pixel of that static's own picture would come out at.
///
/// The distinction [`at`] cannot make and `docs/lighting_height.md` phase 3 is
/// about. A wall's face lies *on* the panel it is the face of, so the wall must
/// not shadow it — and until that phase the rule that said so was "a point is
/// never shadowed by its own *tile*", which a point of the air standing in the
/// same tile got for free. Now it is the *thing*: a fragment carries which
/// occluder of its cell it was drawn from, and only that one is exempt.
///
/// `graphic` and `z_at` are the static's own, the pair
/// [`occlusion::Occlusion::owner_at`] keys on — the same two the scene placed it
/// with, not the height the probe is taken at.
fn on_the_static(
    lighting: &Lighting,
    tile: (u16, u16),
    z: f32,
    graphic: openshard_protocol::wire::Graphic,
    z_at: i8,
) -> f32 {
    let owner = lighting
        .occlusion
        .owner_at(i32::from(tile.0), i32::from(tile.1), z_at, graphic);
    assert_ne!(
        owner,
        occlusion::OwnerId::NONE,
        "no {graphic:?} at {tile:?} in this scene's grid — the probe would be \
         asking about a point of nothing, which is a different question",
    );
    light::sample(spot(tile, z).owned_by(owner), lighting).brightness()
}

/// The room, drawn, for the message a failing assertion carries.
fn picture(scene: &Scene, lighting: &Lighting) -> String {
    format!(
        "\n{}:\n{}",
        scene.name,
        debug::diagram(lighting, debug::around(CENTRE, 6), 0.0)
    )
}

/// A torch in a shut room lights the room and nothing outside it.
///
/// The claim the whole pass was built for. Both halves matter and they fail
/// separately: an inside that is not lit means the flame was never collected,
/// an outside that is means the ring of wall has a hole in it — and until there
/// was a diagram, those two looked the same from a screenshot.
#[test]
fn a_shut_room_keeps_its_light_inside() {
    let scene = scene::room();
    let lighting = scene.lighting(STILL);
    let picture = picture(&scene, &lighting);

    let lit_tile = (CENTRE.0 + 1, CENTRE.1);
    let inside = at(&lighting, lit_tile, 0.0);
    assert!(
        brighter_by(inside, ambient(&lighting, lit_tile)) > 0.2,
        "the room is not lit: {inside}{picture}"
    );

    // Every tile outside the ring, on all four sides, is the ambient exactly —
    // not merely dimmer. "Stops" is a different claim from "falls off", and a
    // radius that happened to be short would pass the weaker one.
    for tile in [
        (CENTRE.0 + scene::ROOM_HALF + 1, CENTRE.1),
        (CENTRE.0 - scene::ROOM_HALF - 1, CENTRE.1),
        (CENTRE.0, CENTRE.1 + scene::ROOM_HALF + 1),
        (CENTRE.0, CENTRE.1 - scene::ROOM_HALF - 1),
    ] {
        let outside = at(&lighting, tile, 0.0);
        assert!(
            (outside - ambient(&lighting, tile)).abs() < 1e-6,
            "light leaks out at {tile:?}: {outside} against the ambient's {}{picture}",
            ambient(&lighting, tile),
        );
    }
}

/// The edge of a shadow lands where the geometry puts it, not on a tile
/// boundary.
///
/// **The claim that outlived its measurement.** While a cell was all-or-nothing
/// the only two answers a ray could come back with were `1.0` and `0.0`,
/// whatever the fraction of a tile a fragment was at, so every shadow in the
/// frame had a tile's straight side and stepped between two neighbouring
/// samples. This was written as "a sweep across the spill passes through the
/// values in between", and until decision 24 that is what it read: a doorpost was
/// a whole-tile occluder, what it stopped was scaled by the length of the
/// crossing, and a ray clipping its corner kept most of its light.
///
/// That softening was the leak. It is the same arithmetic that let a ray through
/// the corner of a house and into the room behind it — see
/// [`a_lamp_outside_a_house_corner_does_not_light_the_room_behind_it`] — and what
/// closed the leak took the sideways gradient with it, for the reason decision 18
/// gives: a cell-local softening is measured from the *cell's* boundary and not
/// from the surface's silhouette, so it is wrong in both directions wherever a
/// wall carries on into the next tile.
///
/// What is left is the claim underneath, and it is the one that was worth having:
/// **the fan out of a doorway is wider than the doorway**, by the fraction of a
/// tile similar triangles say and not by a whole tile or by none. A staircase on
/// tile boundaries puts the two edges exactly on the doorway tile's own sides;
/// the geometry puts them a little outside, and how far outside is decided by
/// where the flame stands. The surviving penumbra is vertical, and
/// [`a_ray_grazing_the_top_of_a_wall_is_dimmed_rather_than_switched`] is where it
/// is measured.
///
/// Across the spill and not along it, at a hundredth of a tile: the sweep is over
/// what a wall did to the ray, so it reads `Reach::through` — the shadow term
/// alone — rather than the brightness, which falls off with distance and would
/// show an edge even if every ray were binary.
#[test]
fn the_edge_of_a_shadow_lands_where_the_geometry_puts_it() {
    let scene = scene::room_with_open_door();
    let lighting = scene.lighting(STILL);
    let picture = picture(&scene, &lighting);

    // A line across the fan of light on the ground a tile and a half south of the
    // doorway, from well inside the shadow of one doorpost to well inside the
    // other's.
    let y = f32::from(DOORWAY.1) + 1.5;
    let sweep: Vec<(f32, f32)> = (-100..=100)
        .map(|step| f32::from(DOORWAY.0) + 0.5 + step as f32 / 100.0)
        .map(|x| {
            let spot = Spot::at(Vec2::new(x, y), 0.0, (x.floor() as i32, y.floor() as i32));
            let through = light::sample(spot, &lighting)
                .reaches
                .iter()
                .find(|reach| reach.within)
                .map_or(0.0, |reach| reach.through);
            (x, through)
        })
        .collect();
    let lit: Vec<f32> = sweep
        .iter()
        .filter(|(_, through)| *through > 0.5)
        .map(|(x, _)| *x)
        .collect();
    let (west, east) = (
        *lit.first().expect("some of the sweep is lit"),
        *lit.last().expect("some of the sweep is lit"),
    );

    // The doorway's own tile spans `x` from `DOORWAY.0` to one past it. The fan a
    // tile and a half beyond it is wider than that at both ends — which is what
    // says the edge is not on a tile boundary — and by a fraction of a tile rather
    // than by a whole one, which is what says it is not on the *next* boundary
    // either. The bounds are wide because the number they hold is a consequence of
    // where the torch stands and what a flame's lift is; the reading is 0.08 of a
    // tile at each end and either end failing is the same defect.
    let doorway = f32::from(DOORWAY.0);
    assert!(
        west < doorway && doorway - west < 0.5,
        "the spill's west edge is at {west}, not a fraction of a tile past {doorway}\n\
         {sweep:?}{picture}",
    );
    assert!(
        east > doorway + 1.0 && east - (doorway + 1.0) < 0.5,
        "the spill's east edge is at {east}, not a fraction of a tile past {}\n\
         {sweep:?}{picture}",
        doorway + 1.0,
    );
}

/// A ray grazing the top of a wall is dimmed rather than switched.
///
/// **The penumbra that survives**, and the whole of it: a flame is a body rather
/// than a point, so the edge of what a wall casts is soft over a band of the
/// similar-triangles width decision 14 derived — `spread * t / (1 - t)`, in `z`
/// units. Decision 18 kept it vertical and dropped it sideways, and decision 24
/// dropped the last of the sideways one when it stopped scaling a whole-tile
/// occluder by the length of the crossing. Nothing measured this until then; what
/// did was a sideways sweep, and it was measuring the term that went.
///
/// Up the wall and not across it, at a quarter of a `z` unit: the spot climbs
/// from below the wall's top to well above it, so the ray to the torch on the far
/// side crosses the wall's plane at a height that walks up through the top edge
/// of the span. Three claims, and they fail separately — the low end is dark, the
/// high end is clear, and it is monotone in between rather than a step with noise
/// either side of it.
#[test]
fn a_ray_grazing_the_top_of_a_wall_is_dimmed_rather_than_switched() {
    let scene = scene::torch_before_a_wall();
    let lighting = scene.lighting(STILL);
    let picture = picture(&scene, &lighting);

    // On the far side of the wall from the torch, a tile and a half out, climbing
    // past the top of it.
    let at = Vec2::new(f32::from(CENTRE.0) + 0.5, f32::from(CENTRE.1) + 1.5);
    let tile = (at.x.floor() as i32, at.y.floor() as i32);
    let sweep: Vec<(f32, f32)> = (0..=120)
        .map(|step| step as f32 / 4.0)
        .map(|z| {
            let through = light::sample(Spot::at(at, z, tile), &lighting)
                .reaches
                .iter()
                .find(|reach| reach.within)
                .map_or(0.0, |reach| reach.through);
            (z, through)
        })
        .collect();

    let low = sweep.first().expect("the sweep has a bottom").1;
    let high = sweep.last().expect("the sweep has a top").1;
    assert!(low < 1e-6, "the wall passes light at its base: {low}{picture}");
    assert!(high > 0.99, "the wall shadows the sky over it: {high}{picture}");

    let partial = sweep
        .iter()
        .filter(|(_, through)| *through > 0.02 && *through < 0.98)
        .count();
    assert!(
        partial >= 4,
        "the wall's top edge switches rather than dims: {partial} samples in between\n\
         {sweep:?}{picture}",
    );
    // And it climbs. A band that went dark again above the wall would give the
    // same count and would be a different, worse answer.
    for pair in sweep.windows(2) {
        let [(z, below), (_, above)] = pair else {
            continue;
        };
        assert!(
            *above >= below - 1e-6,
            "the shadow deepens on the way up the wall, at z {z}: {below} then {above}\n\
             {sweep:?}{picture}",
        );
    }
}

/// And the report says *which* wall stopped it.
///
/// The half of observability that a picture cannot carry: a tile is dark, and
/// this is the cell that made it dark. If the shadow ever comes from a tile
/// nobody built, that is the assertion that says so.
#[test]
fn the_report_names_the_cell_that_stopped_the_ray() {
    let scene = scene::room();
    let lighting = scene.lighting(STILL);
    let east = (CENTRE.0 + scene::ROOM_HALF + 1, CENTRE.1);
    let sample = light::sample(spot(east, 0.0), &lighting);

    let reach = sample
        .reaches
        .iter()
        .find(|reach| reach.within)
        .unwrap_or_else(|| panic!("the torch does not even reach the tile:\n{sample}"));
    assert_eq!(
        reach.stopped_by.map(|stopper| stopper.cell),
        Some((i32::from(CENTRE.0 + scene::ROOM_HALF), i32::from(CENTRE.1))),
        "stopped somewhere other than the east wall:\n{sample}",
    );
}

/// A shut door is a wall; an open one is a hole, and the light goes through it.
///
/// The two scenes differ in exactly one graphic — nothing in the lighting knows
/// what a door is, and this is what says the mechanism is the tiledata flag and
/// not a special case. See `docs/lighting.md`, decision 11.
#[test]
fn opening_a_door_spills_light_onto_the_ground_outside() {
    let shut = scene::room_with_shut_door();
    let open = scene::room_with_open_door();
    let (shut_light, open_light) = (shut.lighting(STILL), open.lighting(STILL));

    // Straight out of the doorway, one tile past the wall. Asserted through the
    // *reason* rather than through a brightness threshold: at four tiles of a
    // torch's six the pool has fallen to a twentieth, so any number chosen here
    // would be a number about the falloff, and what this test is about is
    // whether the ray got through at all.
    let outside = (DOORWAY.0, DOORWAY.1 + 1);
    let shut_reach = light::sample(spot(outside, 0.0), &shut_light).reaches[0];
    let open_reach = light::sample(spot(outside, 0.0), &open_light).reaches[0];
    assert_eq!(
        shut_reach.stopped_by.map(|stopper| stopper.cell),
        Some((i32::from(DOORWAY.0), i32::from(DOORWAY.1))),
        "a shut door is not a wall{}",
        picture(&shut, &shut_light),
    );
    assert_eq!(
        (open_reach.stopped_by, open_reach.through),
        (None, 1.0),
        "the open door still stops light{}",
        picture(&open, &open_light),
    );
    let (closed, opened) = (at(&shut_light, outside, 0.0), at(&open_light, outside, 0.0));
    assert!(
        (closed - ambient(&shut_light, outside)).abs() < 1e-6 && opened > closed,
        "the ground outside the doorway: {opened} open against {closed} shut{}",
        picture(&open, &open_light),
    );

    // And it is a fan through the opening rather than a glow around the house:
    // the tile diagonally out from the doorway is behind the wall beside it, and
    // stays dark. Without this the test would pass for a door that stopped
    // occluding the whole ring.
    let beside = (DOORWAY.0 + 2, DOORWAY.1 + 1);
    let spill = at(&open_light, beside, 0.0);
    assert!(
        (spill - ambient(&open_light, beside)).abs() < 1e-6,
        "the open door lit the ground behind the wall beside it: {spill}{}",
        picture(&open, &open_light),
    );
}

/// A pane of glass dims light; a wall stops it.
///
/// `WINDOW` sits beside `NO_SHOOT` in the reference's line of sight
/// (`Map.LineOfSight`, `Server/Map.cs:3040`) — and that is a fact about arrows.
/// A window that stopped light makes a lit room read as a bunker and hides the
/// one thing a candle is for at night, so the pane keeps four fifths of what
/// crosses it. The three cases are asserted together because what matters is
/// that they are *ordered*: through the wall, through the glass, and through the
/// doorway are three different numbers and always in that order.
#[test]
fn a_pane_of_glass_dims_light_where_a_wall_stops_it() {
    let outside = (DOORWAY.0, DOORWAY.1 + 1);
    let walled = scene::room();
    let glazed = scene::room_with_window();
    let opened = scene::room_with_open_door();
    let (walled_light, glazed_light, open_light) = (
        walled.lighting(STILL),
        glazed.lighting(STILL),
        opened.lighting(STILL),
    );

    // What the *flame* added, and not what the tile came out at: the three scenes
    // differ by one graphic, and a graphic that shades its column differently
    // gives the same tile a different share of the sky in each of them. Comparing
    // the totals would be comparing the ambient as much as the light through the
    // opening, which is the question this test is not asking.
    let added = |lighting: &Lighting| at(lighting, outside, 0.0) - ambient(lighting, outside);
    let (wall, glass, doorway) = (added(&walled_light), added(&glazed_light), added(&open_light));
    assert!(
        wall.abs() < 1e-6,
        "the wall no longer stops light: {wall} arrives through it{}",
        picture(&walled, &walled_light),
    );
    assert!(
        wall < glass && glass < doorway,
        "the glass is not between the wall and the open door: \
         {wall} walled, {glass} glazed, {doorway} open{}",
        picture(&glazed, &glazed_light),
    );

    // And by about the fraction `occlusion::PANE` states, rather than by
    // whatever a threshold would tolerate: the light through the pane is what
    // the open doorway passes, less a fifth.
    let want = doorway * 0.8;
    assert!(
        (glass - want).abs() < 1e-3,
        "the pane passes {glass}, not the {want} its opacity says{}",
        picture(&glazed, &glazed_light),
    );
}

/// The wall of a lit room is lit on the inside, at every height up its face.
///
/// The bug the world-coordinate pass was written against: in screen space the
/// wall's own sprite stands over the ground it shadows, so the face turned
/// towards the flame was the darkest thing in the picture. Here the wall's
/// pixels carry the wall's tile — and, since `docs/lighting_height.md` phase 3,
/// which occluder of that tile they are pixels *of*, which is what says the wall
/// does not shadow its own face. Probed through [`on_the_static`] for exactly
/// that reason: a point of the air in the same tile is a different question and
/// gets a different answer.
#[test]
fn the_face_of_a_wall_is_lit_from_inside_the_room() {
    let scene = scene::room();
    let lighting = scene.lighting(STILL);
    let wall = (CENTRE.0 + scene::ROOM_HALF, CENTRE.1);
    // The foot of the wall, halfway up it, and the top: a sprite's pixels differ
    // in `z` and nothing else, and all three are inside the pool.
    for z in [
        0.0,
        f32::from(scene::WALL_HEIGHT) / 2.0,
        f32::from(scene::WALL_HEIGHT),
    ] {
        let lit = on_the_static(&lighting, wall, z, scene::WALL, 0);
        assert!(
            brighter_by(lit, ambient(&lighting, wall)) > 0.1,
            "the wall is dark at z {z}: {lit}{}",
            picture(&scene, &lighting),
        );
    }
}

/// A sconce lights the street it hangs over and not the room behind it.
///
/// **The test that was the other way round for as long as this pass existed.** A
/// light's own tile is exempt from occluding it — decision 3, and right for a
/// torch standing in a doorway — so a lamp bolted to a house lit the room inside
/// exactly as brightly as the street outside, and the old version of this
/// asserted the two were *equal* and said it would fail the day a facing arrived.
///
/// What answered it is not an exemption but a place: the wall's own art names
/// which side of its tile it stands on, so the flame belongs outside that plane
/// rather than at its tile's centre — `light::mounted_at`, decision 26. The wall
/// then stops being the flame's own cell and becomes an ordinary occluder, and
/// the room goes dark for the same reason any room behind a wall does.
///
/// Both sides are asserted, and the second is what keeps the fix honest: a sconce
/// that lit nothing at all would pass "the room is dark" perfectly.
#[test]
fn a_sconce_lights_the_street_and_not_the_room_behind_it() {
    let scene = scene::sconce_on_wall();
    let lighting = scene.lighting(STILL);
    let picture = picture(&scene, &lighting);
    // The wall stands on its tiles' south edge, so the street it faces is south
    // and the room is north.
    let (street, room) = ((CENTRE.0, CENTRE.1 + 1), (CENTRE.0, CENTRE.1 - 1));
    let lit = at(&lighting, street, 0.0);
    let dark = at(&lighting, room, 0.0);
    assert!(
        brighter_by(lit, ambient(&lighting, street)) > 0.2,
        "the sconce lights nothing at all: {lit}{picture}",
    );
    assert!(
        (dark - ambient(&lighting, room)).abs() < 1e-6,
        "the sconce lights through its own wall: {dark} against an ambient of {}{picture}",
        ambient(&lighting, room),
    );
}

/// A light carried in a hand lights what the character is facing far more
/// brightly than what is behind them.
///
/// The whole claim of a beam, and the reason it is not simply a torch on the
/// player's tile: an omnidirectional pool centred on a body lights the wall
/// behind it exactly as brightly as the one it is walking towards, which reads as
/// the character glowing rather than as the character carrying something.
///
/// Stated as a ratio and not as "behind is the ambient exactly", because a hand
/// is not a shutter: `light::BEAM_SPILL` of the flame goes every way, so the
/// character and the ground at their feet are lit. What the cone has to buy is
/// the *difference*, and that is what is measured.
#[test]
fn a_carried_light_lights_the_way_it_is_pointed() {
    let scene = scene::lantern_in_a_room();
    let lighting = scene.lighting(STILL);
    let picture = picture(&scene, &lighting);

    // East is the facing, so `+x` is ahead. Two tiles out on either side: the
    // same distance from the flame, so the falloff is the same for both and the
    // only difference between them is the direction.
    let (ahead, behind) = ((CENTRE.0 + 2, CENTRE.1), (CENTRE.0 - 2, CENTRE.1));
    // Two domains, on purpose. The floor below is an absolute margin, authored
    // against a picture, and so it is read as a displayed value — `brighter_by`.
    // The ratio is a claim about *quantities* of light: "three times as much
    // reaches ahead as behind" is true or false of radiance and says nothing
    // about stored bytes, so it stays linear.
    let lit = at(&lighting, ahead, 0.0) - ambient(&lighting, ahead);
    let dark = at(&lighting, behind, 0.0) - ambient(&lighting, behind);
    assert!(
        brighter_by(at(&lighting, ahead, 0.0), ambient(&lighting, ahead)) > 0.2,
        "the beam lights nothing ahead of it: {lit} over the ambient{picture}",
    );
    assert!(
        dark > 0.0,
        "nothing spills out of the beam, so the character is in a black hole: \
         {dark}{picture}",
    );
    assert!(
        lit > dark * 3.0,
        "the beam is not pointed anywhere: {lit} ahead against {dark} behind{picture}",
    );

    // And the same for the two walls the room has on those sides, up their whole
    // height: a beam that lit the floor and left every wall in the frame at the
    // ambient would look like a decal on the ground. The east face is what says
    // the light has hit something.
    let (front_wall, back_wall) = (
        (CENTRE.0 + scene::ROOM_HALF, CENTRE.1),
        (CENTRE.0 - scene::ROOM_HALF, CENTRE.1),
    );
    for z in [0.0, f32::from(scene::WALL_HEIGHT) / 2.0] {
        // Points **of** those walls, not points of the air inside their tiles —
        // see `on_the_static`, and `docs/lighting_height.md` phase 3.
        // The same pair of domains as above: a displayed margin for the floor, a
        // linear ratio for the comparison.
        let front = on_the_static(&lighting, front_wall, z, scene::WALL, 0);
        let face = front - ambient(&lighting, front_wall);
        let back = on_the_static(&lighting, back_wall, z, scene::WALL, 0) - ambient(&lighting, back_wall);
        assert!(
            brighter_by(front, ambient(&lighting, front_wall)) > 0.1,
            "the wall the beam points at is dark at z {z}: {face}{picture}",
        );
        assert!(
            face > back * 3.0,
            "both walls are lit the same at z {z}: {face} against {back}{picture}",
        );
    }
}

/// The beam's own edge is a gradient, and it is sixty degrees wide.
///
/// Two claims one sweep answers. A cone with a hard rim reads as a stencil laid
/// over the scene — the same complaint the tile-edged shadows drew — so the
/// values between the two ends have to exist; and the *width* is the number
/// somebody will change by accident, so it is measured rather than trusted: at
/// four tiles out, a sixty-degree beam's rim is `4 * tan(30°)` ≈ 2.3 tiles off
/// the axis, and a spot three tiles across is outside it by any softening.
#[test]
fn the_edge_of_a_beam_is_a_gradient_of_the_stated_width() {
    let scene = scene::lantern_in_a_room();
    let lighting = scene.lighting(STILL);
    let picture = picture(&scene, &lighting);
    let carried = lighting.lights[0];
    let beam = carried.beam.expect("the scene's character carries a beam");

    // Straight along the axis and then off it, in tenths of a tile, four tiles
    // out — the distance is fixed so that only the angle changes.
    let mut values = Vec::new();
    for step in 0..=50 {
        let across = step as f32 / 10.0;
        values.push(beam.lights([4.0, across, 0.0]));
    }
    let spill = light::BEAM_SPILL;
    let partial = values.iter().filter(|v| **v > spill + 0.01 && **v < 0.99).count();
    assert!(
        partial >= 3,
        "the beam's rim is a step and not a gradient: {values:?}{picture}",
    );
    assert!(
        values[0] > 0.99,
        "the beam's own axis is not fully lit: {}{picture}",
        values[0],
    );
    // `4 * tan(30°)` is 2.31 tiles: inside two, outside three, whatever the
    // softening does in between.
    assert!(
        values[20] > spill,
        "the beam is narrower than the sixty degrees it says: {}{picture}",
        values[20],
    );
    assert_eq!(
        values[30], spill,
        "the beam is wider than the sixty degrees it says{picture}",
    );
}

/// A torch in a cellar does not light the street above it, with nothing in
/// between.
///
/// Distance alone, in three dimensions: `z` divided into tiles at eleven units
/// each. There is no floor in this scene on purpose — a test that also had one
/// would pass even if the height were being ignored entirely.
#[test]
fn a_cellar_does_not_light_the_street_above_it() {
    let scene = scene::cellar_under_street();
    let lighting = scene.lighting(STILL);
    let street = at(&lighting, CENTRE, 0.0);
    assert!(
        (street - ambient(&lighting, CENTRE)).abs() < 1e-6,
        "the cellar lights the street: {street}{}",
        picture(&scene, &lighting),
    );
    // And the flame is real: it lights its own floor. Without this the test
    // above would pass for a scene where the torch was never collected.
    let cellar = at(&lighting, CENTRE, f32::from(scene::CELLAR_DEPTH));
    assert!(
        brighter_by(cellar, ambient(&lighting, CENTRE)) > 0.2,
        "the cellar itself is dark: {cellar}"
    );
}

/// A torch on the ground floor does not light the storey above it.
///
/// **A floor is a plane, and a plane has no thickness to be travelled through.**
/// Every other occluder in these scenes is a slab — a wall twenty `z` deep, a
/// roof five — and what one of those stops is scaled by how far the ray ran
/// inside its span, which is right for a solid and is exactly zero for a floor:
/// a real one is `height 0` in `tiledata.mul`, in 4,534 of the 4,647 lids over
/// the block of Britain `artscan`'s `column` example reads. So the whole upper
/// storey of a house came out lit from under its own floorboards, brightest on
/// the upper wall, which takes the ray head on.
///
/// Read four tiles across the room from the flame — see [`scene::STOREY_TORCH`]:
/// nearer than that and the ray crosses the floor inside the lit end's own cell,
/// which every walk exempts and must, since a pixel standing on a floor is not
/// shadowed by the floor it stands on.
///
/// The third assertion is the one that keeps this honest: the ground floor is
/// lit. Without it the scene would pass with the torch never collected at all.
#[test]
fn a_torch_does_not_light_the_storey_above_it() {
    let scene = scene::storey_over_a_torch();
    let lighting = scene.lighting(STILL);
    let picture = picture(&scene, &lighting);

    // **Every tile of the row, the torch's own included.** Three separate leaks
    // lived on this row and each was invisible from the others: the crossing rule
    // itself, the two ends' exemptions — a sconce and the storey's wall are one
    // tile with the floor in between, and both ends used to exempt it — and the
    // walk's shortcut for a ray with no ground run at all, which returned "open"
    // for the one geometry a floor is most obviously in the way of. A single spot
    // four tiles off would have passed with any two of the three fixed.
    //
    // The last of the row is the upper storey's *wall*, which is what a player
    // actually sees lit, and the flat pixels beside it are the storey's floor.
    for x in CENTRE.0 - scene::ROOM_HALF..=CENTRE.0 + scene::ROOM_HALF {
        let tile = (x, CENTRE.1);
        let upstairs = at(&lighting, tile, scene::STOREY_Z);
        assert!(
            (upstairs - ambient(&lighting, tile)).abs() < 1e-6,
            "the torch lights {tile:?} above its own ceiling: {upstairs} against \
             the ambient's {}{picture}",
            ambient(&lighting, tile),
        );
    }

    // The same square of the map, below the floor instead of above it: lit. Half
    // a room away from the flame the pool is thinner than the tile-away margin
    // the other scenes assert — which is the point of reading it here rather than
    // beside the torch, where a floor that stopped nothing would still pass.
    let downstairs = at(&lighting, scene::STOREY_SPOT, 0.0);
    assert!(
        brighter_by(downstairs, ambient(&lighting, scene::STOREY_SPOT)) > 0.05,
        "the ground floor itself is dark, so the scene proves nothing: \
         {downstairs}{picture}"
    );
}

/// And light comes up through a **hole** in that floor.
///
/// The other half of decision 32, and the half that says the rule reads the grid
/// rather than covering the world: a lid that stopped everything everywhere would
/// pass [`a_torch_does_not_light_the_storey_above_it`] and be wrong in a way no
/// shut room could show. One plank is taken out of the same house —
/// [`scene::hole_in_a_floor`] differs from it by one item — and what appears
/// above is a patch, not a glow: the cell the ray crosses the plane in is the one
/// that decides, so the tiles either side of the gap stay at the ambient exactly.
///
/// It lands **beyond** the hole rather than over it, which is geometry and not a
/// tolerance: from a spot directly above a gap the flame is three tiles off to
/// the side, so that line of sight passes under the planks of the tile between
/// them. The patch is where the ray from the flame comes up, and that is a tile
/// further on.
#[test]
fn a_hole_in_a_floor_lets_the_light_through() {
    let scene = scene::hole_in_a_floor();
    let lighting = scene.lighting(STILL);
    let picture = picture(&scene, &lighting);

    let through = at(&lighting, scene::STOREY_SPOT, scene::STOREY_Z);
    assert!(
        brighter_by(through, ambient(&lighting, scene::STOREY_SPOT)) > 0.04,
        "nothing comes up through the hole: {through} against the ambient's \
         {}{picture}",
        ambient(&lighting, scene::STOREY_SPOT),
    );

    // Either side of the gap, at the same height: the planks are still there and
    // the storey above them is not lit. Without this the assertion above would
    // pass for a rule that had stopped reading the grid at all.
    for tile in [scene::FLOOR_HOLE, (CENTRE.0, CENTRE.1)] {
        let beside = at(&lighting, tile, scene::STOREY_Z);
        assert!(
            (beside - ambient(&lighting, tile)).abs() < 1e-6,
            "the floor leaks at {tile:?}, a tile away from the hole: {beside} \
             against the ambient's {}{picture}",
            ambient(&lighting, tile),
        );
    }
}

/// A room lights its own wall, and not the storey standing on it, and not the
/// line where the two meet.
///
/// [`scene::storey_over_a_lit_room`] is the client's own house, tile for tile:
/// two walls on one tile with the art naming their edge, the storey's floor laid
/// over the room beside them and stopping at the wall, and the torch in the room
/// *under* that floor. Reported from a frame as a bright stroke along the
/// floorboards — four screen pixels of wall at the floor line, lit from the
/// storey below.
///
/// **Read at the face's own fraction.** `statics.wgsl` puts a face pixel at
/// [`scene::INSIDE`], eight thousandths of a tile short of the plane it is the
/// face of, and that is the only horizontal position a frame ever draws one at.
/// The middle of the tile is swept beside it for the claim that is about the
/// *room* — its own wall is lit — and deliberately not for anything at or above
/// the floor line, which is where the two positions part. What closes the line is
/// `light::stand_clear` walking a face pixel from a hair in **front** of its
/// plane, so that the cell the ray starts in is the one the floor is in; from the
/// middle of a tile that hair is half a tile short, the ray crosses the floor's
/// plane inside the wall's own column — which has no plank over it, because a
/// house's floor stops at its wall — and the light comes back. A model whose
/// answer depends on where in its cell a pixel stands is worth saying out loud;
/// what makes it sound is that the drawn position is the only one that exists.
///
/// The other half is [`light::ON_TOP`], and neither half closes it alone: a
/// pixel whose `z` is exactly the floor's lies *in* the plane, and the crossing
/// test is strict, so the ray runs along the plane rather than through it. Strict
/// it must stay — inclusive, it would lay half a floor's shadow across every room
/// lit from inside it — so the point is moved onto the boards instead of the test
/// being loosened. A pixel is drawn on top of a floor, and so is the candle
/// standing on it.
#[test]
fn a_room_lights_its_own_wall_and_not_the_storey_over_it() {
    let scene = scene::storey_over_a_lit_room();
    let lighting = scene.lighting(STILL);
    let picture = picture(&scene, &lighting);
    let tile = scene::STOREY_WALL;
    let ambient = ambient(&lighting, tile);
    let face = |across: f32, z: f32| {
        let at = Vec2::new(f32::from(tile.0) + across, f32::from(tile.1) + 0.5);
        light::sample(
            Spot::face(at, z, (i32::from(tile.0), i32::from(tile.1)), Face::East),
            &lighting,
        )
        .brightness()
    };

    // The ground floor's own wall, lit by the ground floor's own sconce, at both
    // fractions: this half is not sensitive to where in its cell the point is,
    // and it is what fails if the floor starts stopping everything rather than
    // what crosses it.
    for across in [scene::INSIDE, 0.5] {
        for z in [10.0, f32::from(scene::WALL_HEIGHT) - 1.0] {
            let inside = face(across, z);
            assert!(
                brighter_by(inside, ambient) > 0.2,
                "the room does not light its own wall at {across}, z {z}: \
                 {inside}{picture}",
            );
        }
    }

    // And everything from the floor line up, at the fraction a frame draws a face
    // pixel at. The line itself first, then three heights of the storey's wall:
    // one spot could pass on a ray that happened to miss the room.
    for z in [
        f32::from(scene::WALL_HEIGHT),
        f32::from(scene::WALL_HEIGHT) + 1.0,
        25.0,
        35.0,
    ] {
        let storey = face(scene::INSIDE, z);
        assert!(
            (storey - ambient).abs() < 1e-6,
            "the sconce lights the storey at z {z}: {storey} against the \
             ambient's {ambient}{picture}",
        );
    }
}

/// A ray does **not** slip between two walls that touch only at their corners.
///
/// This test used to pin the opposite, and the leak it pinned was real: the walk
/// steps one cell at a time, so a ray running the diagonal left the first cell
/// and entered the second across the corner — over a crossing of no length, which
/// the old length-scaled occluder rounded to nothing. Whichever of the two cells
/// the comparison happened to pick was the only one asked, and the other was
/// asked over nothing.
///
/// What closed it is the corner case in `light::walk_cells` and `blit.wgsl`'s
/// `walk`: where the two boundaries land together the walk asks *both* cells that
/// share the corner, at the height the ray is at when it passes through it, and
/// then steps diagonally past them. Which is the supercover walk the backlog
/// asked for, at two extra samples on the rays that hit a corner exactly rather
/// than at twice the samples everywhere.
#[test]
fn a_ray_does_not_slip_between_two_walls_that_touch_at_a_corner() {
    let scene = scene::diagonal_gap();
    let lighting = scene.lighting(STILL);
    let behind = at(&lighting, CENTRE, 0.0);
    assert!(
        (behind - ambient(&lighting, CENTRE)).abs() < 1e-6,
        "light slips through the corner where two walls touch: {behind} against \
         the ambient's {}{}",
        ambient(&lighting, CENTRE),
        picture(&scene, &lighting),
    );

    // And the flame is real: a tile the walls do not stand between is lit. Without
    // this the assertion above would hold for a scene whose torch was never
    // collected at all.
    let open = (CENTRE.0 + 2, CENTRE.1 + 1);
    let lit = at(&lighting, open, 0.0);
    assert!(
        brighter_by(lit, ambient(&lighting, open)) > 0.1,
        "the torch lights nothing at all: {lit}{}",
        picture(&scene, &lighting),
    );
}

/// A ray near the same corner but **off** the exact diagonal still does not
/// slip through.
///
/// The test above samples dead centre, where the two boundaries land on the
/// same instant to the bit. A quarter tile off that line the ordinary,
/// non-corner step already catches it — it walks into one of the two wall
/// cells directly rather than skipping past both — so this scene does not by
/// itself tell `light::corner_tie`'s derived width apart from the old, bare
/// `1e-4` it replaced (checked by hand: both pass it). It is kept anyway as
/// the regression a person reads next to the exact-diagonal case, and
/// `light::corner_tie_converts_back_into_exactly_one_panel_thickness_of_world_distance`
/// is where the width itself is actually pinned.
#[test]
fn a_ray_near_a_corner_and_off_the_exact_diagonal_still_does_not_slip_through() {
    let scene = scene::diagonal_gap();
    let lighting = scene.lighting(STILL);
    let off_diagonal_at = Vec2::new(f32::from(CENTRE.0) + 0.25, f32::from(CENTRE.1) + 0.5);
    let off_diagonal = Spot::at(
        off_diagonal_at,
        0.0,
        (off_diagonal_at.x.floor() as i32, off_diagonal_at.y.floor() as i32),
    );
    let leaked = light::sample(off_diagonal, &lighting)
        .reaches
        .iter()
        .find(|reach| reach.within)
        .map_or(0.0, |reach| reach.through);
    assert!(
        leaked < 1e-6,
        "light slips through the corner off the exact diagonal: {leaked}{}",
        picture(&scene, &lighting),
    );
}

/// A lamp outside the corner of a house does not light the room behind it.
///
/// **Britain at `(1441, 1692)`, built** — see [`scene::house_corner`], which
/// carries the path a leaking ray takes. Reported from the client as a bright
/// seam at 45° out of a house corner, and the mechanism is decision 18's spoke
/// arriving where a run of wall has to turn: the last tile of the run is entered
/// through its north side and left eastwards, so its own panel is never crossed;
/// the corner tile is faceless and therefore a *body*, and a body was the one
/// branch still scaled by the length of the crossing, which for a sliver is
/// nothing.
///
/// The spot is on the diagonal from the flame, a third of a tile north of the
/// wall's line, which is where the sliver is longest and the leak was 85%. Two
/// tiles back from the corner and not one: a spot beside the corner would be lit
/// by the exemption of the flame's own tile rather than by the defect.
#[test]
fn a_lamp_outside_a_house_corner_does_not_light_the_room_behind_it() {
    let scene = scene::house_corner();
    let lighting = scene.lighting(STILL);
    let picture = picture(&scene, &lighting);

    // Inside the house, on the diagonal running back from the flame through the
    // corner: `(1439.5, 1691.17)` of Britain, in this scene's coordinates.
    let inside_at = Vec2::new(f32::from(CENTRE.0) - 1.5, f32::from(CENTRE.1) - 0.83);
    let inside = Spot::at(
        inside_at,
        0.0,
        (inside_at.x.floor() as i32, inside_at.y.floor() as i32),
    );
    let leaked = light::sample(inside, &lighting)
        .reaches
        .iter()
        .find(|reach| reach.within)
        .map_or(0.0, |reach| reach.through);
    assert!(
        leaked < 1e-6,
        "light slips through the house corner into the room: {leaked} of the flame\
         {picture}",
    );

    // And the flame is real and reaches: the street on the wall's own side is lit
    // the whole way along the run. Without this the assertion above would hold for
    // a scene whose torch was never collected — and it is the assertion that would
    // catch a fix that closed the leak by walling the corner off in every
    // direction, which is the failure the conservative direction invites.
    let street_at = Vec2::new(f32::from(CENTRE.0) - 1.5, f32::from(CENTRE.1) + 1.5);
    let street = Spot::at(
        street_at,
        0.0,
        (street_at.x.floor() as i32, street_at.y.floor() as i32),
    );
    let outside = light::sample(street, &lighting)
        .reaches
        .iter()
        .find(|reach| reach.within)
        .map_or(0.0, |reach| reach.through);
    assert!(
        outside > 0.99,
        "the lamp does not light the street it stands in: {outside}{picture}",
    );
}

/// A corner's two faces are lit as two surfaces, not as one tile.
///
/// The other half of the same report, and the half decision 24 could not reach:
/// a corner whose art named no edge was `Stance::Upright`, which has no outward
/// normal at all — so `blit.wgsl`'s facing test was skipped for it and both of
/// the faces the picture draws came out equally bright, including the one the
/// corner itself stands between the flame and. Decision 22 fixed exactly this for
/// a wall and could not reach a corner, because there was nothing in the
/// attachment to fix it with.
///
/// The lamp stands due south of the corner, where Britain's own lamp post stands
/// — so the corner's **south** face is turned towards it and its **east** face is
/// turned away, half a tile behind the plane the lamp is in front of. Both
/// samples are on the corner's own tile, at the same height, differing in nothing
/// but which surface they are a point of, which is the whole claim: one tile, two
/// answers.
///
/// Reported from the client with the coordinates: lamp at `(1441, 1693)`, corner
/// at `(1441, 1692)`, and the face leaning towards `(1442, 1692)` — the east one
/// — lit when it should not be. What lit it was the facing test's exemption for a
/// flame in the wall's own line, which a lamp post in the street is in for the
/// length of the column. Decision 26.
///
/// What says the renderer agrees is two other tests. `frame.rs`'s
/// `a_corner_s_pixel_carries_the_face_of_the_half_it_is_drawn_on` is what puts
/// those faces into the attachment per half, and the GPU parity fixture is what
/// says the shader reads them the way `light::sample` does here.
#[test]
fn the_two_faces_of_a_corner_are_lit_from_the_side_each_looks_at() {
    use openshard_client_render::facing::Face;

    let scene = scene::house_corner_named_by_its_art();
    let lighting = scene.lighting(STILL);
    let picture = picture(&scene, &lighting);
    let (cx, cy) = (f32::from(CENTRE.0), f32::from(CENTRE.1));

    // The south face lies on the `y1` line of the corner's tile, halfway along
    // it; the east face on the `x1` line. Both a step of the attachment's own
    // fraction inside the tile, which is where `statics.wgsl` writes them — see
    // `INSIDE`, and decision 16 for what a clean 1 would do to the walk.
    let inside = 1.0 - 1.0 / 127.0;
    let corner = (CENTRE.0 as i32, CENTRE.1 as i32);
    let reach = |at: Vec2, face: Face| {
        let spot = Spot::face(at, f32::from(scene::WALL_HEIGHT) / 2.0, corner, face);
        light::sample(spot, &lighting)
            .reaches
            .iter()
            .find(|reach| reach.within)
            .map_or(0.0, |reach| reach.through * reach.cone)
    };
    // `cone` is where a surface's facing lands — the same number a beam does,
    // because both are "how much of this flame is turned this way".
    let towards = reach(Vec2::new(cx + 0.5, cy + inside), Face::South);
    let away = reach(Vec2::new(cx + inside, cy + 0.5), Face::East);

    assert!(
        towards > 0.5,
        "the corner's south face does not see the lamp standing south of it: {towards}{picture}",
    );
    assert!(
        away < 1e-6,
        "the corner's east face is lit by a flame behind its plane: {away}{picture}",
    );
}

/// A wall throws a shadow across the ground away from the sun, and the ground
/// on the sun's side stays lit.
///
/// The first half of decision 12. Both sides are asserted because only one of
/// them fails on its own: a walk that stepped the wrong way would darken the
/// wrong tile and a test that only looked at the dark one would be green for it.
#[test]
fn a_wall_throws_its_shadow_away_from_the_sun() {
    let scene = scene::wall_in_the_sun();
    let lighting = scene.lighting(STILL);
    let sun = scene.sun.expect("the scene has a sun");

    // The sun is towards +x, so the shadow lies at lower `x`.
    let shaded = (CENTRE.0 - 1, CENTRE.1);
    let sunlit = (CENTRE.0 + 1, CENTRE.1);
    let dark = light::sample(spot(shaded, 0.0), &lighting);
    let bright = light::sample(spot(sunlit, 0.0), &lighting);

    assert_eq!(
        dark.sun
            .expect("a sunlit frame")
            .stopped_by
            .map(|stopper| stopper.cell),
        Some((i32::from(CENTRE.0), i32::from(CENTRE.1))),
        "the tile away from the sun is not shadowed by the wall:\n{dark}{}",
        picture(&scene, &lighting),
    );
    assert_eq!(
        bright.sun.expect("a sunlit frame").through,
        1.0,
        "the tile towards the sun is shadowed:\n{bright}{}",
        picture(&scene, &lighting),
    );
    assert!(
        bright.brightness() > dark.brightness(),
        "the shadow is not darker than the ground beside it{}",
        picture(&scene, &lighting),
    );

    // And the shadow is as long as the wall is tall, at 45°: twenty units of
    // height is under two tiles, so the second tile out is clear and the third
    // certainly is. The number is the *geometry*, which is what a slope buys —
    // a shadow that ignored the wall's height would run to the edge of the grid.
    assert_eq!(sun.rise_per_tile(), 1.0, "the scene's sun is no longer at 45°");
    let past = light::sample(spot((CENTRE.0 - 3, CENTRE.1), 0.0), &lighting);
    assert_eq!(
        past.sun.expect("a sunlit frame").through,
        1.0,
        "the shadow runs further than the wall is tall:\n{past}{}",
        picture(&scene, &lighting),
    );
}

/// The sun comes through a window and not through the wall beside it.
///
/// The picture decision 12 exists for: a room whose floor is in shadow, with a
/// brighter band behind the pane. Asserted as an ordering rather than as a
/// level, because how bright the band is depends on `occlusion::PANE` and on the
/// sun's intensity, and neither of those is what this is about.
#[test]
fn the_sun_reaches_the_floor_through_a_window() {
    let scene = scene::sunlit_room_with_window();
    let lighting = scene.lighting(STILL);

    // The floor tile just inside the pane, and one three tiles further in. The
    // sun travels towards +x, so a ray leaving the first goes out through the
    // window and a ray leaving the second meets the roof.
    let behind_pane = (scene::WINDOW_TILE.0 - 1, scene::WINDOW_TILE.1);
    let behind_wall = (scene::WINDOW_TILE.0 - 3, scene::WINDOW_TILE.1);
    let lit = light::sample(spot(behind_pane, 0.0), &lighting);
    let dark = light::sample(spot(behind_wall, 0.0), &lighting);
    let sun_of = |sample: &light::Sample| sample.sun.expect("a sunlit frame");

    assert!(
        sun_of(&lit).through > sun_of(&dark).through,
        "the pane passes no more sun than the wall does:\n{lit}\n{dark}{}",
        picture(&scene, &lighting),
    );
    assert!(
        sun_of(&lit).through > 0.0 && sun_of(&dark).through == 0.0,
        "the window is not a window, or the wall is not a wall:\n{lit}\n{dark}{}",
        picture(&scene, &lighting),
    );

    // And the patch is as wide as the opening, which the ordering above cannot
    // see. With the sun's ray sampled one point per tile, the *whole* column one
    // tile in from the wall read `1.0` — brighter than the window's own patch and
    // the same the length of the wall, because what it was reading was a ray that
    // had stepped over the top of the wall rather than through the pane. Read off
    // the sun view, that is a stripe down the room with no window at the end of
    // it: reported from the client as the light from the windows looking
    // inverted, and the reason a floor is swept here rather than two tiles named.
    let mut lit_rows = Vec::new();
    let mut swept = 0;
    for x in CENTRE.0 - scene::ROOM_HALF + 1..CENTRE.0 + scene::ROOM_HALF {
        for y in CENTRE.1 - scene::ROOM_HALF + 1..CENTRE.1 + scene::ROOM_HALF {
            if sun_of(&light::sample(spot((x, y), 0.0), &lighting)).through > 0.0 {
                lit_rows.push((x, y));
            }
            swept += 1;
        }
    }
    let interior = (scene::ROOM_HALF * 2 - 1) as usize;
    assert_eq!(swept, interior * interior, "the sweep did not cover the floor");
    assert!(
        lit_rows.iter().all(|(_, y)| *y == scene::WINDOW_TILE.1),
        "the sun reaches floor the window is not opposite: {lit_rows:?}{}",
        picture(&scene, &lighting),
    );
    assert!(
        !lit_rows.is_empty(),
        "no floor at all is lit, so the assertion above is about nothing{}",
        picture(&scene, &lighting),
    );
}

/// A shut house lets no sun onto its floor. Not one tile of it.
///
/// The regression the sun's walk was rewritten for, and it was found in the sun
/// view rather than by reasoning: every interior tile read `0` **except** the
/// column one tile in from the sunward wall, which read a full `255` — a stripe
/// of noon down the inside of a sealed building. The sun's ray sampled one point
/// per tile, so at 45° it crossed the wall's plane at `z = 16`, inside a span of
/// `0..=20`, and was next looked at one tile later at `z = 22`. It stepped over
/// the top of a wall it had gone through.
///
/// Which is why this sweeps the whole floor rather than asserting on the middle:
/// the middle was always dark, and a test that asked it would have been green for
/// as long as the bug existed. It counts what it swept for the same reason — a
/// sweep over an empty range asserts nothing and looks identical in the output.
#[test]
fn a_shut_house_lets_no_sun_onto_any_tile_of_its_floor() {
    let house = scene::roofed_room();
    let lighting = house.lighting(STILL);

    let mut swept = 0;
    for x in CENTRE.0 - scene::ROOM_HALF + 1..CENTRE.0 + scene::ROOM_HALF {
        for y in CENTRE.1 - scene::ROOM_HALF + 1..CENTRE.1 + scene::ROOM_HALF {
            let sample = light::sample(spot((x, y), 0.0), &lighting);
            let sun = sample.sun.expect("a sunlit frame");
            assert_eq!(
                sun.through,
                0.0,
                "({x}, {y}) is inside a shut house and the sun reaches it:\n{sample}{}",
                picture(&house, &lighting),
            );
            swept += 1;
        }
    }
    let interior = (scene::ROOM_HALF * 2 - 1) as usize;
    assert_eq!(swept, interior * interior, "the sweep did not cover the floor");
}

/// A frame with no sun pays nothing and says so.
///
/// `None` and `0.0` are different answers — "there is no sky" against "the sky
/// is dark here" — and the report has to keep them apart, because the first is
/// where a person looking for a missing sunbeam should stop looking.
#[test]
fn a_frame_without_a_sun_reports_none_rather_than_nothing() {
    let scene = scene::room();
    let lighting = scene.lighting(STILL);
    assert!(light::sample(spot(CENTRE, 0.0), &lighting).sun.is_none());
}

/// How much of the sky a tile of a scene can see.
fn sky(scene: &Scene, tile: (u16, u16)) -> u8 {
    scene
        .lighting(STILL)
        .occlusion
        .sky_at(i32::from(tile.0), i32::from(tile.1))
}

/// A tile well outside the house, for the "and the street is open" half of
/// everything below.
const STREET: (u16, u16) = (CENTRE.0, CENTRE.1 + scene::ROOM_HALF + 3);

/// A room under a roof does not get the sky's light, and the street outside it
/// does.
///
/// `docs/lighting_world.md`, decision 1, and it is the largest visible change
/// this plan makes: today a room is lit exactly as brightly as the road, because
/// the ambient is one colour for the whole frame. Nothing about a flame is in
/// this test — the field is what a *place* has before anything burns in it.
#[test]
fn a_roof_takes_the_sky_from_the_room_under_it() {
    let house = scene::roofed_room();
    let street = sky(&house, STREET);
    let room = sky(&house, CENTRE);
    assert_eq!(street, occlusion::SKY_OPEN, "the street is not open sky");
    assert_eq!(room, 0, "the middle of a roofed room still sees the sky");
}

/// And the room is *darker* for it, before anything burns in it.
///
/// The other half of decision 1, and the half a field alone does not give: the
/// sky byte is only a number until an ambient is split in two and scaled by it.
/// What this asserts is the whole visible change of step 2 — a room under a roof
/// is deep, the street outside is not, and neither is black.
///
/// Held as an ordering and a floor rather than as levels: how dark the room is
/// is `light::GROUND_AMBIENT`, which is a number tuned against a picture, and a
/// test that pinned it would fail every time somebody looked at the picture.
#[test]
fn a_roof_makes_the_room_under_it_darker_than_the_street() {
    let house = scene::roofed_room();
    let lighting = house.lighting(STILL);

    let room = ambient(&lighting, CENTRE);
    let street = ambient(&lighting, STREET);
    assert!(
        brighter_by(street, room) > 0.1,
        "the room is lit as brightly as the road outside it: {room} against {street}",
    );
    // And not a hole: an unlit black rectangle is not atmosphere, it is a bug
    // report — which is the whole of what the ground term is for.
    // The scene is at noon, so it is lit by `light::SKYLIGHT` — and a tile with
    // no sky at all gets that ambient's ground term and nothing else.
    let floor: f32 = light::GROUND_AMBIENT.iter().sum::<f32>() / 3.0;
    assert!(
        (room - floor).abs() < 1e-6,
        "the roofed room is not the ground ambient exactly: {room} against {floor}",
    );

    // And the same through the whole formula rather than through its first term:
    // what a person actually sees is the ambient plus whatever reaches the tile,
    // and nothing here would show up in a frame if the sun happened to make the
    // difference up.
    let (room, street) = (at(&lighting, CENTRE, 0.0), at(&lighting, STREET, 0.0));
    assert!(
        brighter_by(street, room) > 0.1,
        "lit, the room is as bright as the road: {room} against {street}{}",
        picture(&house, &lighting),
    );
}

/// The threshold of an open door is brighter than the room and darker than the
/// street.
///
/// Decision 2: the blur is what makes a doorway a gradient. Without it the field
/// steps from 1 to 0 at the wall line, which is the artefact the whole track
/// exists to remove — and the two scenes differ by exactly one graphic, so a
/// difference between them is the door and nothing else.
#[test]
fn a_doorway_is_a_threshold_and_not_a_step() {
    let open = scene::roofed_room_with_open_door();
    let shut = scene::roofed_room();

    let threshold = sky(&open, DOORWAY);
    assert!(
        threshold > 0 && threshold < occlusion::SKY_OPEN,
        "the doorway of {} reads {threshold}, which is the room or the street",
        open.name,
    );
    assert!(
        threshold > sky(&shut, DOORWAY),
        "an open door is worth no more sky than a shut one",
    );
    assert!(
        threshold < sky(&open, STREET),
        "the doorway is as bright as the road outside it",
    );
}

/// A glazed wall is worth some of the sky, and a solid one is worth none.
///
/// The crude half of decision 14, and the whole of what stands in for
/// `docs/lighting.md`'s step 16 until it lands: a pane passes its share in the
/// column, and the blur is what carries it inwards. What is asserted is the
/// ordering and not the level — how much a pane passes is `occlusion::PANE`,
/// which is a guess about glass and not a number from any file.
#[test]
fn a_window_is_worth_more_sky_than_the_wall_it_replaces() {
    let glazed = scene::roofed_room_with_window();
    let solid = scene::roofed_room();
    let inside = (scene::WINDOW_TILE.0 - 1, scene::WINDOW_TILE.1);

    assert!(
        sky(&glazed, scene::WINDOW_TILE) > sky(&solid, scene::WINDOW_TILE),
        "the pane itself is as dark as a wall",
    );
    assert!(
        sky(&glazed, inside) > sky(&solid, inside),
        "the room behind the window is no lighter than a cellar",
    );
    assert_eq!(sky(&solid, inside), 0, "and the windowless room is a cellar");
}

/// Every scene draws a diagram with something in it.
///
/// A weak assertion on purpose: what it guards is the failure mode of a
/// diagnostic, which is being silently empty. A diagram of nothing but spaces
/// would still make every message above *look* informative, and that is worse
/// than no diagram at all.
#[test]
fn every_scene_prints_a_diagram_that_is_not_blank() {
    for scene in scene::all() {
        let lighting = scene.lighting(STILL);
        let drawn = debug::diagram(&lighting, debug::around(CENTRE, 6), 0.0);
        // A flame, an occluder, or lit ground: every scene has at least one of
        // the three, and a sunlit one has no flame at all.
        assert!(
            drawn.contains('*') || drawn.contains('#'),
            "nothing stands in the diagram of {}:\n{drawn}",
            scene.name,
        );
        assert!(
            drawn.lines().count() > 12,
            "the diagram of {} is too small to read:\n{drawn}",
            scene.name,
        );
    }
}

/// Light travels *along* a wall and not through it.
///
/// The one an occluder that was a whole tile could not get right, and the reason
/// `docs/lighting.md`'s decision 3 was revised once step 15 could measure which
/// edge a wall stands on. A lamp mounted on a house used to be shadowed by the
/// next tile of its own wall, so the street it hung over came out with a band of
/// darkness that nothing visible was casting — which is how this was found.
///
/// Asserted on `Reach::through` rather than on brightness, because that is the
/// number the change is about: how much of the flame the walk let past. A
/// brightness would fold in the falloff and the ambient and would need a
/// tolerance argued about instead of a fact.
///
/// Three rays, and the third is what makes the other two mean anything:
///
/// - *Along* the wall — a point of the wall's own **face**, three tiles west of
///   the lamp, so the ray runs down the run and enters each tile through one side
///   and leaves through the other without ever crossing a panel it is not part
///   of. All of it arrives.
///
///   The face and not the ground of the wall's tiles, which is what this sampled
///   before decision 28: a floor pixel on a wall tile is a different surface, it
///   is inside the room, and its own tile's panel is now allowed to shadow it.
/// - *Across* it — a spot south of the row, so the ray goes through a face.
///   Most of it does not arrive; not all, because it clips the tile obliquely
///   and decision 14's penumbra is doing its job.
/// - *The same scene with no art at all*, where nothing names an edge and every
///   occluder is the whole tile it was before. The along-ray is stopped. That is
///   the old behaviour, and it is what says this test would fail on the code it
///   was written against rather than passing for some other reason.
#[test]
fn light_runs_along_a_wall_and_stops_across_it() {
    let scene = scene::wall_with_a_torch_beside_it();
    let (cx, cy) = CENTRE;
    let through = |scene: &Scene, spot: Spot| {
        let lighting = scene.lighting(STILL);
        let sample = light::sample(spot, &lighting);
        let reach = sample.reaches[0];
        assert!(reach.within, "{spot:?} is outside the torch's radius: {sample}");
        reach.through
    };
    // A point of the wall's south face, halfway along the tile and halfway up it.
    // The fraction is held one step inside its own tile, which is where
    // `statics.wgsl` writes it — decision 16.
    let inside = 1.0 - 1.0 / 127.0;
    let face = |x: u16| {
        Spot::face(
            Vec2::new(f32::from(x) + 0.5, f32::from(cy) + inside),
            f32::from(scene::WALL_HEIGHT) / 2.0,
            (i32::from(x), i32::from(cy)),
            openshard_client_render::facing::Face::South,
        )
    };

    // Along: three tiles west of the torch, on the wall's own face, with four
    // tiles of the same wall in between.
    let along = through(&scene, face(cx));
    assert!(
        along > 0.99,
        "{}: the wall shadows the light running along it — {along:.3} of it arrives",
        scene.name,
    );

    // Across: on the far side of the wall from the flame, which is the *north*
    // side. The wall stands on its tiles' south edge and a sconce bolted to it
    // burns outside that plane (`light::mounted_at`), so what is across the wall
    // from the lamp is the room and not the street it hangs over. Two tiles back
    // and one tile west, so the ray crosses a panel that is not the flame's own
    // cell's — a flame's own tile never shadows it.
    let across = through(&scene, spot((cx - 1, cy - 2), 0.0));
    assert!(
        across < 0.5,
        "{}: the wall let light through its own face — {across:.3} of it arrives",
        scene.name,
    );

    // And the same scene with the art taken away. Nothing then says which edge
    // the wall is on, every occluder is the whole tile, and the along-ray dies —
    // which is the defect this whole change is about, reproduced on demand.
    let blind = Scene {
        art: None,
        ..scene::wall_with_a_torch_beside_it()
    };
    let along_blind = through(&blind, face(cx));
    assert!(
        along_blind < 0.01,
        "with no art an occluder is the whole tile and the along-ray must die — {along_blind:.3} \
         of it arrived, so this test is not measuring the edge at all",
    );
}

/// A ray through the gap between two walls on one tile goes through it.
///
/// **Step 21.2, and the one place in `docs/lighting.md`'s decision 30 where the
/// picture had to move.** `occlusion::Builder::add` used to union everything
/// standing on a tile into one span, so a wall from `z 0` to `z 10` and another
/// from `z 30` to `z 40` came out as one wall from 0 to 40 and closed thirty `z`
/// of open air between them — a band of shadow with nothing in the picture
/// casting it, which is the failure this whole pass exists to keep out of a
/// frame.
///
/// The grid is built by hand rather than out of a scene, and deliberately: a
/// scene is a map, and what this is about is two statics on one tile, which is
/// the thing a `Map` makes fiddly to say and a `Builder` makes one line. What it
/// costs is that the flame is placed here rather than collected, and the
/// assertion is written to survive that — it compares the gap against the wall
/// beside it rather than against a constant.
#[test]
fn a_ray_through_the_gap_between_two_walls_on_one_tile_passes() {
    use openshard_client_render::camera::TileBounds;
    use openshard_client_render::light::{Ambient, Light};
    use openshard_uofiles::tiledata::{StaticTile, TileFlags};

    const WALL: (u16, u16) = (105, 105);
    /// The air between the two walls, and the height a ray is asked about.
    const GAP: f32 = 20.0;
    /// Inside the lower wall, where a ray must still die.
    const SOLID: f32 = 5.0;

    let bounds = TileBounds {
        min_x: 100,
        max_x: 110,
        min_y: 100,
        max_y: 110,
    };
    let wall = StaticTile {
        flags: TileFlags::new(TileFlags::NO_SHOOT),
        height: 10,
        ..StaticTile::default()
    };
    let mut grid = occlusion::Builder::new(bounds);
    // Two walls on one tile with thirty `z` of air between them. No facing: with
    // no art an occluder is the whole tile, which is the body rule and the one
    // the union used to hand a span that covered both.
    grid.add(
        WALL.0,
        WALL.1,
        0,
        openshard_protocol::wire::Graphic(0),
        &wall,
        occlusion::Shape::UNREAD,
    );
    grid.add(
        WALL.0,
        WALL.1,
        30,
        openshard_protocol::wire::Graphic(0),
        &wall,
        occlusion::Shape::UNREAD,
    );
    let lighting = Lighting {
        ambient: Ambient {
            sky: [0.0, 0.0, 0.0],
            ground: [0.0, 0.0, 0.0],
        },
        lights: vec![Light {
            // Due west of the wall, so a ray to it crosses the tile squarely and
            // the only thing deciding it is the height.
            at: Vec2::new(f32::from(WALL.0) - 1.5, f32::from(WALL.1) + 0.5),
            z: GAP,
            radius: 8.0,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam: None,
        }],
        occlusion: grid.finish(&Cutaway::OPEN),
        sun: None,
        view: openshard_client_render::debug::View::Lit,
    };

    // Due east of the wall, level with the flame: the ray runs straight through
    // the air between the two walls.
    let east = (WALL.0 + 2, WALL.1);
    let through = |z: f32| light::sample(spot(east, z), &lighting).reaches[0].through;

    assert!(
        through(GAP) > 0.9,
        "the gap between the two walls is shut — {:.3} of the ray survived it, \
         which is the union closing air the map has nothing standing in",
        through(GAP),
    );
    // And the walls themselves are unchanged, which is what says the gap opened
    // rather than the tile emptying: a ray at the height of the lower wall dies
    // on it exactly as it always did.
    let solid = light::sample(
        Spot::at(
            Vec2::new(f32::from(east.0) + 0.5, f32::from(east.1) + 0.5),
            SOLID,
            (i32::from(east.0), i32::from(east.1)),
        ),
        &Lighting {
            lights: vec![Light {
                z: SOLID,
                ..lighting.lights[0]
            }],
            occlusion: lighting.occlusion.clone(),
            ..lighting
        },
    );
    assert!(
        solid.reaches[0].through < 0.01,
        "the wall itself stopped being a wall — {:.3} of the ray went through the \
         solid part of it",
        solid.reaches[0].through,
    );
}

/// One grid, one panel, one hole: the fixture the two tests below aim rays at.
///
/// Built by hand rather than out of a scene, for the reason the gap test above
/// is: what these are about is one surface with a stated hole in it, which a
/// `Builder` says in a line and a `Map` makes fiddly. The panel stands on the
/// **south** side of `HOLED_WALL`, so it lies in the plane `y = HOLED_WALL.1 + 1`
/// and what runs along it is `x` — which is the coordinate a hole's `near` and
/// `far` are measured in. See `facing::Hole`, which is what the art measures and
/// what a `Shape` carries; the wall here stands at `z = 0`, so the rectangle the
/// grid ends up with is the same four numbers placed.
fn wall_with_hole(hole: openshard_client_render::facing::Hole) -> occlusion::Occlusion {
    use openshard_client_render::camera::TileBounds;
    use openshard_client_render::facing::{Face, Facing};
    use openshard_uofiles::tiledata::{StaticTile, TileFlags};

    let mut grid = occlusion::Builder::new(TileBounds {
        min_x: 95,
        max_x: 115,
        min_y: 95,
        max_y: 115,
    });
    grid.add(
        HOLED_WALL.0,
        HOLED_WALL.1,
        0,
        openshard_protocol::wire::Graphic(0),
        &StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT),
            height: 20,
            ..StaticTile::default()
        },
        occlusion::Shape {
            facing: Some(Facing::One(Face::South)),
            hole: Some(hole),
            prism: None,
            blocks: openshard_client_render::facing::Blocks::EMPTY,
        },
    );
    grid.finish(&Cutaway::OPEN)
}

/// The tile the panel above stands on.
const HOLED_WALL: (u16, u16) = (105, 105);

/// How much of a ray from `from` to a flame at `to` survives that grid.
///
/// The flame is put **far** behind the wall and the lit point close in front of
/// it on purpose: the penumbra a crossing gets is `FLAME_SPREAD * t / (1 - t)`
/// with `t` measured from the lit end, so a crossing near the lit end is the
/// sharpest edge this walk draws. A hole half a tile wide judged through a
/// penumbra two thirds of a tile wide would be a test of the softening rather
/// than of the hole.
fn ray(grid: &occlusion::Occlusion, from: (f32, f32, f32), to: (f32, f32, f32)) -> f32 {
    use openshard_client_render::light::{Ambient, Light};

    let lighting = Lighting {
        ambient: Ambient {
            sky: [0.0, 0.0, 0.0],
            ground: [0.0, 0.0, 0.0],
        },
        lights: vec![Light {
            at: Vec2::new(to.0, to.1),
            z: to.2,
            radius: 12.0,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam: None,
        }],
        occlusion: grid.clone(),
        sun: None,
        view: debug::View::Lit,
    };
    let tile = (from.0.floor() as i32, from.1.floor() as i32);
    light::sample(Spot::at(Vec2::new(from.0, from.1), from.2, tile), &lighting).reaches[0].through
}

/// A ray that goes through the hole in a wall passes; one that goes through the
/// wall beside it does not.
///
/// **Step 21.3**, and the axis it is about is the one that is new: *where along
/// the surface's own run* the ray crosses. Everything before this asked a
/// crossing only how high it was, so a window could dim a whole tile and nothing
/// finer — which is a dimmer tile and not a beam.
///
/// The two rays differ in nothing but that. Same wall, same graphic, same
/// opacity, same height, same flame: one crosses the panel at the middle of its
/// run and the other near its end, and the hole is stated to cover the middle
/// half. A test that moved the wall instead would be measuring two walls.
#[test]
fn a_ray_through_a_hole_in_a_wall_passes_and_one_beside_it_does_not() {
    // The middle half of the run, open from the wall's own base to higher than
    // it stands, so that the only thing deciding either ray is `v`. The height
    // half is the test below.
    let grid = wall_with_hole(openshard_client_render::facing::Hole {
        near: 64,
        far: 191,
        bottom: 0,
        top: 255,
    });
    // Five tiles north of the wall: far enough that the crossing is a twentieth
    // of the way along the ray, which is the sharpest penumbra the walk draws.
    //
    // Level, at five `z`: **well inside the hole rather than on its sill**. A
    // hole is measured above the base of the wall it is cut into, so its floor
    // is a real edge with a real penumbra across it, and a ray along `z = 0`
    // would be asking about the softening rather than about `v`.
    let flame = (105.5, 100.5, 5.0);

    let middle = ray(&grid, (105.5, 106.2, 5.0), flame);
    let beside = ray(&grid, (105.95, 106.2, 5.0), flame);

    assert!(
        middle > 0.95,
        "the hole is not a hole — only {middle:.3} of a ray aimed straight through \
         its middle survived",
    );
    assert!(
        beside < 0.05,
        "the wall beside the hole stopped being a wall — {beside:.3} of the ray went \
         through it, so the hole is the whole tile and not a rectangle in it",
    );
}

/// And the same hole in the other direction: a ray over the top of it is stopped
/// by the wall above.
///
/// The `z` half of the rectangle, and it needs its own test because the scene
/// that measures the fan on the ground cannot ask it — a floor pixel and a flame
/// are both near `z = 0`, so every ray in that picture crosses at one height. A
/// hole whose `z` span was ignored would pass both of these and look right in
/// every picture this file has.
#[test]
fn a_ray_over_a_hole_in_a_wall_is_stopped_by_the_wall_above_it() {
    // Open across the whole run, so that `v` cannot be what decides either ray,
    // and the bottom half of a twenty-tall wall.
    let grid = wall_with_hole(openshard_client_render::facing::Hole {
        near: 0,
        far: 255,
        bottom: 0,
        top: 10,
    });

    // Level rays, so the height a ray crosses at is the height it is asked at.
    let through_it = ray(&grid, (105.5, 106.2, 5.0), (105.5, 100.5, 5.0));
    let over_it = ray(&grid, (105.5, 106.2, 15.0), (105.5, 100.5, 15.0));

    assert!(
        through_it > 0.95,
        "the hole is shut at the height it is open at — {through_it:.3} survived",
    );
    assert!(
        over_it < 0.05,
        "the wall above the hole stopped being a wall — {over_it:.3} of a ray five \
         `z` over the top of the hole went through it",
    );
}

/// A hole in one tile of a run of wall throws a fan of light onto the ground
/// behind it, and the tiles either side of it stay dark.
///
/// The picture step 21.3 exists to produce, on a scene rather than on a
/// hand-built grid: `scene::wall_with_a_hole_in_it` is `torch_before_a_wall`
/// with the middle tile's graphic swapped for one that carries a hole, so the
/// wall either side is the same graphic at the same height with the same
/// opacity. A fan that appeared without the hole would be some other defect and
/// a fan that failed to appear cannot be blamed on the wall.
#[test]
fn a_hole_in_a_wall_throws_a_fan_of_light_onto_the_ground_behind_it() {
    let scene = scene::wall_with_a_hole_in_it();
    let lighting = scene.lighting(STILL);
    let (cx, cy) = CENTRE;

    // Behind the holed tile, and behind the solid wall two tiles along it. Same
    // distance from the flame in `y`, so what differs is the hole and nothing.
    // Against each tile's *own* ambient rather than against a constant: the sky
    // field gives a tile beside a wall a different share of the night than one
    // in the open, so a bare difference of brightnesses would be measuring that
    // as much as the hole.
    let behind_hole = brighter_by(at(&lighting, (cx, cy + 1), 0.0), ambient(&lighting, (cx, cy + 1)));
    let behind_wall = at(&lighting, (cx + 2, cy + 1), 0.0);
    let dark = ambient(&lighting, (cx + 2, cy + 1));

    assert!(
        behind_hole > 0.1,
        "no fan came through the hole: {behind_hole:.3} of flame over the ambient \
         behind it, against {:.3} behind the wall two tiles along{}",
        brighter_by(behind_wall, dark),
        picture(&scene, &lighting),
    );
    // Linear, and deliberately not `brighter_by`: this slack is for numerical
    // dribble in the quantity itself, not a perceptual threshold somebody chose
    // against a picture, so it belongs in the domain the quantity is computed in.
    assert!(
        (behind_wall - dark).abs() < 0.01,
        "the wall beside the hole leaks: {behind_wall:.3} against an ambient of \
         {dark:.3}, so the hole was cut in the whole run{}",
        picture(&scene, &lighting),
    );
    // And it is a *fan*: it is still there two tiles further out, where a hole
    // that only lit the tile against the wall would have closed.
    // And it is a **fan**: wider further from the wall than against it. Measured
    // as the width at half the sweep's own peak rather than at a fixed number,
    // because a hole this size is seen through a penumbra of about its own width
    // — the flame is a body, and `FLAME_SPREAD * t / (1 - t)` is two thirds of a
    // tile by the time the ray has crossed — so there is no plateau to find an
    // edge of. Half the peak is where the shape is, and the shape is the claim.
    let width = |y: f32| {
        let sweep: Vec<(f32, f32)> = (-200..=200)
            .map(|step| f32::from(cx) + 0.5 + step as f32 / 100.0)
            .map(|x| {
                let tile = (x.floor() as i32, y.floor() as i32);
                let through = light::sample(Spot::at(Vec2::new(x, y), 0.0, tile), &lighting)
                    .reaches
                    .iter()
                    .find(|reach| reach.within)
                    .map_or(0.0, |reach| reach.through);
                (x, through)
            })
            .collect();
        let peak = sweep.iter().map(|(_, t)| *t).fold(0.0, f32::max);
        let lit: Vec<f32> = sweep
            .iter()
            .filter(|(_, through)| *through > peak * 0.5)
            .map(|(x, _)| *x)
            .collect();
        match (lit.first(), lit.last()) {
            (Some(west), Some(east)) => east - west,
            _ => 0.0,
        }
    };
    // A tile and a half behind the wall, and three and a half — the second is as
    // far out as a six-tile pool reaches at all, which is what bounds the pair.
    let (near, far) = (width(f32::from(cy) + 1.5), width(f32::from(cy) + 3.5));
    assert!(
        far > near + 0.2,
        "the fan does not widen: {near:.2} tiles across half a tile behind \
         the wall and {far:.2} two and a half behind it{}",
        picture(&scene, &lighting),
    );
}

/// A point on its own tile's far edge reads that tile, not the next one —
/// the boundary `light::sample`'s `walk_cells` used to get wrong.
///
/// `docs/lighting_raymarch.md` step 3. The real defect this pins:
/// `mesh_face.wgsl`'s `fract()` bug (`docs/lighting.md`, "Fixed: the
/// shadow-raymarch anomaly") was a *GPU* fragment reading `world.x = 1498.0`
/// — a stair tread's own outer corner, exactly on the tile's far edge — as
/// belonging to the tile beyond the stair rather than the tread's own.
/// `walk_cells`'s CPU twin had the same `floor()` and step 2 fixed it by
/// carrying `Spot::tile` instead of re-deriving it. Written *against* that
/// fixed `Spot` on purpose: a bare `Spot` could not even state "the point at
/// the tile's far edge is still this tile's", so a test against the old API
/// would only be re-encoding the bug, not catching it.
///
/// Same fixture as `light::tests::a_treads_top_is_not_shadowed_by_its_own_riser`
/// — a climbable `Prism` three treads tall — read at the top tread's own
/// outer corner instead of its middle, which is the one point the centre
/// reading never touches.
#[test]
fn a_point_on_its_own_tiles_far_edge_reads_that_tile_not_the_next_one() {
    use openshard_client_render::facing::Prism;
    use openshard_client_render::occlusion::{Builder, Shape};
    use openshard_protocol::wire::Graphic;
    use openshard_uofiles::tiledata::{StaticTile, TileFlags};

    let stair = StaticTile {
        flags: TileFlags::new(TileFlags::NO_SHOOT | TileFlags::CLIMBABLE),
        height: 20,
        ..StaticTile::default()
    };
    let prism = Prism::new(Face::West, &[1, 3, 5]).expect("three treads");
    let mut grid = Builder::new(openshard_client_render::camera::TileBounds {
        min_x: 95,
        max_x: 105,
        min_y: 95,
        max_y: 105,
    });
    grid.add(100, 100, 0, Graphic(0x0736), &stair, Shape::solid(prism));
    let grid = grid.finish(&Cutaway::OPEN);

    // The same fixture and the same reading as
    // `light::tests::a_treads_top_is_not_shadowed_by_its_own_riser`: the
    // tallest tread's own top, lit from the east, unshadowed by the riser it
    // caps. That test only ever reads the tile's *middle* in `y`; this one
    // extends the same claim to the tile's own far `y` edge — a whole
    // number — which is exactly the coordinate shape the real defect had.
    let top = grid
        .solids_at(100, 100)
        .filter(|solid| solid.edges == 0)
        .max_by_key(|solid| solid.top())
        .expect("the climb built three tops");
    let mid_x = ((top.space.min.x + top.space.max.x) / 2.0) as f32;
    let edge_y = top.space.max.y as f32;
    let z = top.top() as f32;
    let tile = (100, 100);

    // East, level with the top tread — the same light the proven fixture
    // uses, foot of the flight, where a person actually stands a torch.
    let light = light::Light {
        at: Vec2::new(102.5, 100.5),
        z,
        radius: 6.0,
        color: [1.0, 1.0, 1.0],
        intensity: 1.0,
        beam: None,
    };
    let lighting = Lighting {
        ambient: light::NIGHT,
        lights: vec![light],
        occlusion: grid,
        sun: None,
        view: debug::View::default(),
    };

    // The tile's own middle in `y`, and its far edge: both are points of the
    // same tread, both fed the same `tile` a real caller would carry, and
    // neither should be shadowed by the riser it caps
    // (`Surface::shadowed_by_own_tile`'s exemption). A flip between them is
    // `walk_cells` disagreeing with itself about which tile the ray left
    // from — the old `first = from.floor()` read the far edge as the tile
    // beyond this one, where the exemption never applies.
    let middle = light::sample(Spot::flat(Vec2::new(mid_x, 100.5), z, tile), &lighting).reaches[0].through;
    let on_edge = light::sample(Spot::flat(Vec2::new(mid_x, edge_y), z, tile), &lighting).reaches[0].through;

    assert!(
        (middle - on_edge).abs() < 0.05,
        "through flips across the tile's own far edge: {middle:.3} at the tile's middle, \
         {on_edge:.3} exactly on its far edge",
    );
    assert!(
        on_edge > 0.9,
        "the tread's own outer corner is shadowed by the riser it caps: through {on_edge:.3}",
    );
}

/// How densely [`brute_force_blocked`] samples a ray, in tiles of ground
/// distance.
///
/// **Not `occlusion::PANEL_THICKNESS`'s own depth, on purpose — tighter than
/// that by twenty.** The step used to be `0.02`, sized to the thinnest a
/// solid's box gets, and it was too coarse: `docs/lighting_raymarch.md`
/// session 10 found this fuzz test (and its `walk_cells_exact` counterpart,
/// added the same session) fail on a random seed whose spot and light sat
/// exactly far enough apart that the ray only clipped the wall tile's own
/// far corner for about three thousandths of a tile of real depth —
/// `ray_vs_solid`-confirmed a genuine hit, `walk_cells` and
/// `walk_cells_exact` both correctly called it blocked, and this oracle's
/// `0.02`-tile step stepped clean over the sliver and called it open. A
/// thin *panel* and a thin *corner graze* are different questions —
/// `PANEL_THICKNESS` bounds the first, nothing before session 10 measured
/// the second — so the step is tighter than either fixture in this file has
/// yet defeated, not merely tight enough for the one that was measured.
const BRUTE_STEP: f32 = 0.001;

/// Whether the straight segment from `from` to `to` passes through any solid
/// standing between the two tiles the walk itself exempts.
///
/// Deliberately dumb, the way `docs/lighting_raymarch.md` step 4 asks: fixed
/// steps along the line, a point-in-box test against
/// [`occlusion::Occlusion::solids_at`]'s own boxes, no `floor()`/`fract()`
/// reconstruction of a cell and no DDA. It shares no arithmetic with
/// `light::walk_cells` or `blit.wgsl`'s `walk`, so a bug the two of them share
/// — the shape this whole doc is about — cannot hide from it the way a second
/// DDA rewrite could.
///
/// Scoped to a binary answer, blocked or not: a boundary misread flips exactly
/// that, and `walk_cells`'s own soft gradient across a grazing edge is a
/// different property with its own tests already (`a_wall_stops_the_light...`
/// et al.). Scoped to aperture-free occluders too — every fixture built below
/// — and asserts that premise rather than silently ignoring a hole this
/// oracle does not model.
fn brute_force_blocked(
    from: [f32; 3],
    to: [f32; 3],
    own_tile: (i32, i32),
    target_tile: (i32, i32),
    skip_last: bool,
    occlusion: &occlusion::Occlusion,
) -> bool {
    let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let ground = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
    let steps = ((ground / BRUTE_STEP).ceil() as u32).max(1);
    // Interior points only: the two ends stand where they are drawn, on the
    // geometry they are a point of, and asking whether an endpoint is "inside"
    // its own surface is not a question this oracle exists to answer.
    for step in 1..steps {
        let t = step as f32 / steps as f32;
        let point = [
            from[0] + delta[0] * t,
            from[1] + delta[1] * t,
            from[2] + delta[2] * t,
        ];
        let tile = (point[0].floor() as i32, point[1].floor() as i32);
        if tile == own_tile || (skip_last && tile == target_tile) {
            continue;
        }
        for solid in occlusion.solids_at(tile.0, tile.1) {
            assert!(
                solid.aperture.is_none(),
                "brute_force_blocked does not model apertures; at ({}, {})",
                tile.0,
                tile.1,
            );
            let (min, max) = (solid.space.min, solid.space.max);
            let inside = f64::from(point[0]) >= min.x
                && f64::from(point[0]) <= max.x
                && f64::from(point[1]) >= min.y
                && f64::from(point[1]) <= max.y
                && f64::from(point[2]) >= min.z
                && f64::from(point[2]) <= max.z;
            if inside {
                return true;
            }
        }
    }
    false
}

/// A brute-force point sampler, swept over a grid of light positions and
/// angles, agrees with `light::sample`'s `walk_cells` — the net
/// `docs/lighting_raymarch.md` step 4 asks for, independent of both DDA
/// implementations rather than a second one.
///
/// **A single whole-tile wall, not the climbable stair** step 2/3 pin their
/// own regression against. Tried first against the stair and abandoned: the
/// stair packs three treads and their risers onto *one* tile, and which of
/// them a spot's own tile exempts is exactly `Surface::shadowed_by_own_tile`'s
/// selective, surface-aware rule — a blanket "this whole tile is exempt",
/// which is all a brute-force point sampler can cheaply state, disagreed with
/// the real walk on genuine self-occlusion (a lower tread's ray ducking
/// through a higher tread's own body while still leaving its tile) that has
/// nothing to do with the boundary bug this oracle exists to catch. A plain
/// wall's occlusion is one solid on one tile with nothing else there, so the
/// same blanket exemption is exactly right and the only thing left to
/// disagree about is the boundary derivation itself.
///
/// The spot stands exactly on the open tile's own far edge — `x` a whole
/// number, the coordinate shape `mesh_face.wgsl`'s `fract()` bug and
/// `walk_cells`'s old `first = from.floor()` both got wrong — at a sweep of
/// `y` and `z` along that edge, with the wall one tile further east. A light
/// beyond the wall is blocked by it; a light that never reaches past the wall
/// tile at all is open; a grid of positions and heights exercises both.
#[test]
fn a_brute_force_oracle_agrees_with_the_walk_over_a_grid_of_lights() {
    use openshard_client_render::occlusion::{Builder, Shape};
    use openshard_protocol::wire::Graphic;
    use openshard_uofiles::tiledata::{StaticTile, TileFlags};

    let wall = StaticTile {
        flags: TileFlags::new(TileFlags::NO_SHOOT),
        height: 20,
        ..StaticTile::default()
    };
    let mut grid = Builder::new(openshard_client_render::camera::TileBounds {
        min_x: 90,
        max_x: 110,
        min_y: 90,
        max_y: 110,
    });
    // A whole-tile occluder, no face named: `a_wall_stops_the_light_behind_it`
    // (`tests/frame.rs`) is the same shape, for the same reason — a named edge
    // would let the ray past on the sides it does not cross, which is a
    // different question from the one this oracle asks.
    grid.add(101, 100, 0, Graphic(0x0100), &wall, Shape::UNREAD);
    let occlusion = grid.finish(&Cutaway::OPEN);

    // Points on tile (100, 100)'s own far edge — `x` exactly `101.0` — at a
    // spread of `y` across the tile's width and `z` up its height. Every one
    // of them still belongs to (100, 100), one hair short of the wall's own
    // face, which is what `Spot::tile` says and what `walk_cells`'s old
    // `from.floor()` could read as the wall's tile instead.
    //
    // `y` stays inside the tile's own middle rather than sweeping to its
    // corners: a ray aimed at a large `dy` from a corner spot grazes the
    // wall box's own corner for a hair's width of its path, and a DDA and a
    // continuous point sampler are not obliged to agree about a ray that
    // only ever touches a corner — `corner_tie`'s own test is where that
    // case is already pinned. This oracle is about which *tile* a boundary
    // point belongs to, not about that.
    let mut spots = Vec::new();
    for y in [100.2_f32, 100.5, 100.8] {
        for z in [1.0_f32, 5.0, 10.0, 18.0] {
            spots.push((Vec2::new(101.0, y), z));
        }
    }

    // A grid of light positions and heights: some beyond the wall (blocked),
    // some short of it or offset far enough in `y` to clear it (open) — both
    // outcomes have to show up, or the oracle is only ever agreeing about one
    // of them. `dy` stays modest for the same corner-grazing reason `spots`
    // does; `z` past the wall's own height of 20 is the "open" case for
    // height rather than for `y`.
    //
    // `dx` skips the wall's own `x` span, `101..102`: a flame standing there
    // is a flame *on* the wall's own tile, and `walk_cells`'s exemption for
    // that case is `on_surface` — it reaches only as high as the flame sits
    // *on* the panel, which a point floating above a body at `z 25` is not.
    // Modelling that correctly is a real, separate claim with its own tests
    // (`on_surface`, `flame_end`); this oracle is scoped to the boundary
    // question and keeps every light off the tile the wall is asking about.
    let mut lights = Vec::new();
    for dx in [0.5_f32, 2.5, 4.0] {
        for dy in [-0.6_f32, -0.3, 0.0, 0.3, 2.0] {
            for z in [0.5_f32, 8.0, 17.0, 25.0] {
                lights.push((Vec2::new(100.0 + dx, 100.5 + dy), z));
            }
        }
    }

    let mut compared = 0;
    let mut blocked_count = 0;
    let mut disagreed = Vec::new();
    for &(light_at, light_z) in &lights {
        let light = light::Light {
            at: light_at,
            z: light_z,
            radius: 6.0,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam: None,
        };
        let lighting = Lighting {
            ambient: light::NIGHT,
            lights: vec![light],
            occlusion: occlusion.clone(),
            sun: None,
            view: debug::View::default(),
        };
        let target_tile = (light_at.x.floor() as i32, light_at.y.floor() as i32);
        for &(at, z) in &spots {
            let spot = Spot::flat(at, z, (100, 100));
            let sample = light::sample(spot, &lighting);
            let Some(reach) = sample.reaches.iter().find(|reach| reach.within) else {
                continue;
            };
            let walked_blocked = reach.through <= 0.004;
            let brute_blocked = brute_force_blocked(
                [at.x, at.y, z],
                [light_at.x, light_at.y, light_z],
                (100, 100),
                target_tile,
                true,
                &occlusion,
            );
            compared += 1;
            blocked_count += usize::from(walked_blocked);
            if walked_blocked != brute_blocked {
                disagreed.push(format!(
                    "spot ({:.2}, {:.2}, {z:.1}), light ({:.2}, {:.2}, {light_z:.1}): \
                     walk_cells says {}, the brute-force oracle says {}",
                    at.x,
                    at.y,
                    light_at.x,
                    light_at.y,
                    if walked_blocked { "blocked" } else { "open" },
                    if brute_blocked { "blocked" } else { "open" },
                ));
            }
        }
    }
    assert!(
        compared > 100,
        "the grid compared only {compared} spot/light pairs",
    );
    // Both outcomes have to appear, or the grid's geometry never actually put
    // the wall in the way and this test would be green for any oracle at all.
    assert!(
        blocked_count > 0 && blocked_count < compared,
        "{blocked_count} of {compared} pairs were blocked; the grid should mix both",
    );
    assert!(
        disagreed.is_empty(),
        "{} of {compared} disagreed:\n{}",
        disagreed.len(),
        disagreed.join("\n"),
    );
}

/// The same claim as [`a_brute_force_oracle_agrees_with_the_walk_over_a_grid_of_lights`]
/// — `light::sample`'s walk agrees with [`brute_force_blocked`] — but fuzzed
/// rather than swept over one fixed grid, and shrunk to a minimal
/// counter-example when it disagrees.
///
/// **Why this exists rather than one more hand-built fixture.** The grid test
/// deliberately keeps every ray a comfortable distance from a real corner —
/// its own comment says so, because a DDA and a continuous sampler are not
/// obliged to agree about a ray that only grazes one. `corner_tie`'s "A new
/// `walk_cells` miss" bug lived exactly in that excluded region: a flame
/// sitting near a row's own grid line, reached by a shallow ray from the
/// other side. `flame_y` below is biased into that region on purpose — most
/// runs land the flame within three tenths of a whole `y`, which is the
/// shape that broke `corner_tie` before it carried a per-axis clamp — while
/// the rest of the ranges stay free to roam, so the oracle keeps covering
/// whatever the fixed grid does not think to ask either.
#[test]
fn a_fuzzed_flame_near_a_row_edge_agrees_with_the_brute_force_oracle() {
    use openshard_client_render::occlusion::{Builder, Shape};
    use openshard_protocol::wire::Graphic;
    use openshard_uofiles::tiledata::{StaticTile, TileFlags};
    use proptest::prelude::*;

    proptest!(ProptestConfig::with_cases(512), |(
        spot_dx in 0.05_f32..8.0,
        // Kept inside the wall's own row, `(100, 100)`-`(101, 101)`: a spot in
        // a different row can pass close enough to the wall's *corner*, on
        // its way there, that the walk and a continuous sampler are not
        // obliged to agree — the same carve-out
        // `a_brute_force_oracle_agrees_with_the_walk_over_a_grid_of_lights`
        // takes, found the hard way when an earlier, wider `spot_dy` range
        // turned up exactly that corner-grazing disagreement instead of a
        // real one.
        spot_frac in 0.05_f32..0.95,
        spot_z in 1.0_f32..19.0,
        flame_dx in 0.05_f32..8.0,
        flame_z in 1.0_f32..19.0,
        // Which of the wall row's own two edges to bias near, and how far
        // off it — within three tenths of a whole `y`, the shape that broke
        // `corner_tie` before it carried a per-axis clamp.
        row in prop_oneof![Just(100.0_f32), Just(101.0_f32)],
        frac in -0.3_f32..0.3,
    )| {
        let flame_y = row + frac;

        let wall = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT),
            height: 20,
            ..StaticTile::default()
        };
        let mut grid = Builder::new(openshard_client_render::camera::TileBounds {
            min_x: 90,
            max_x: 110,
            min_y: 90,
            max_y: 110,
        });
        grid.add(100, 100, 0, Graphic(0x0100), &wall, Shape::UNREAD);
        let occlusion = grid.finish(&Cutaway::OPEN);

        let spot_at = Vec2::new(101.0 + spot_dx, 100.0 + spot_frac);
        let spot_tile = (spot_at.x.floor() as i32, spot_at.y.floor() as i32);

        let light_at = Vec2::new(99.0 - flame_dx, flame_y);
        let target_tile = (light_at.x.floor() as i32, light_at.y.floor() as i32);
        prop_assume!(target_tile != (100, 100));

        let light = light::Light {
            at: light_at,
            z: flame_z,
            radius: 30.0,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam: None,
        };
        let lighting = Lighting {
            ambient: light::NIGHT,
            lights: vec![light],
            occlusion: occlusion.clone(),
            sun: None,
            view: debug::View::default(),
        };

        let spot = Spot::flat(spot_at, spot_z, spot_tile);
        let sample = light::sample(spot, &lighting);
        let Some(reach) = sample.reaches.iter().find(|reach| reach.within) else {
            return Ok(());
        };
        let walked_blocked = reach.through <= 0.004;
        let brute_blocked = brute_force_blocked(
            [spot_at.x, spot_at.y, spot_z],
            [light_at.x, light_at.y, flame_z],
            spot_tile,
            target_tile,
            true,
            &occlusion,
        );
        prop_assert_eq!(
            walked_blocked,
            brute_blocked,
            "spot ({:.4}, {:.4}, {:.2}) tile {:?}, light ({:.4}, {:.4}, {:.2}) \
             tile {:?}: walk_cells says {}, the brute-force oracle says {}",
            spot_at.x, spot_at.y, spot_z, spot_tile,
            light_at.x, light_at.y, flame_z, target_tile,
            if walked_blocked { "blocked" } else { "open" },
            if brute_blocked { "blocked" } else { "open" },
        );
    });
}

/// The same claim as
/// [`a_brute_force_oracle_agrees_with_the_walk_over_a_grid_of_lights`], but
/// through [`light::sample_exact`] — `docs/lighting_raymarch.md`'s point 3,
/// the ray-vs-`Solid` walk against an oracle that shares no arithmetic with
/// either DDA.
///
/// `light.rs`'s own `mod tests` already checked `walk_cells_exact` against
/// `walk_cells` directly on this exact scene (a single wall, off
/// `corner_tie`'s path); what that could not exercise is the public seam
/// itself — `sample_exact` threading a spot and a light through `Sample`,
/// `Reach`, `stand_clear`, everything between "where is the flame" and "does
/// the ray get there" that only ever runs through `sample`. Same grid, same
/// fixture, same reason each of `spots`/`lights` is shaped the way it is —
/// see that test's own comments — swapped to the exact walk and to a fresh,
/// independent oracle rather than the walk this doc is trying to replace.
#[test]
fn a_brute_force_oracle_agrees_with_the_exact_walk_over_a_grid_of_lights() {
    use openshard_client_render::occlusion::{Builder, Shape};
    use openshard_protocol::wire::Graphic;
    use openshard_uofiles::tiledata::{StaticTile, TileFlags};

    let wall = StaticTile {
        flags: TileFlags::new(TileFlags::NO_SHOOT),
        height: 20,
        ..StaticTile::default()
    };
    let mut grid = Builder::new(openshard_client_render::camera::TileBounds {
        min_x: 90,
        max_x: 110,
        min_y: 90,
        max_y: 110,
    });
    grid.add(101, 100, 0, Graphic(0x0100), &wall, Shape::UNREAD);
    let occlusion = grid.finish(&Cutaway::OPEN);

    let mut spots = Vec::new();
    for y in [100.2_f32, 100.5, 100.8] {
        for z in [1.0_f32, 5.0, 10.0, 18.0] {
            spots.push((Vec2::new(101.0, y), z));
        }
    }

    let mut lights = Vec::new();
    for dx in [0.5_f32, 2.5, 4.0] {
        for dy in [-0.6_f32, -0.3, 0.0, 0.3, 2.0] {
            for z in [0.5_f32, 8.0, 17.0, 25.0] {
                lights.push((Vec2::new(100.0 + dx, 100.5 + dy), z));
            }
        }
    }

    let mut compared = 0;
    let mut blocked_count = 0;
    let mut disagreed = Vec::new();
    for &(light_at, light_z) in &lights {
        let light = light::Light {
            at: light_at,
            z: light_z,
            radius: 6.0,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam: None,
        };
        let lighting = Lighting {
            ambient: light::NIGHT,
            lights: vec![light],
            occlusion: occlusion.clone(),
            sun: None,
            view: debug::View::default(),
        };
        let target_tile = (light_at.x.floor() as i32, light_at.y.floor() as i32);
        for &(at, z) in &spots {
            let spot = Spot::flat(at, z, (100, 100));
            let sample = light::sample_exact(spot, &lighting);
            let Some(reach) = sample.reaches.iter().find(|reach| reach.within) else {
                continue;
            };
            let walked_blocked = reach.through <= 0.004;
            let brute_blocked = brute_force_blocked(
                [at.x, at.y, z],
                [light_at.x, light_at.y, light_z],
                (100, 100),
                target_tile,
                true,
                &occlusion,
            );
            compared += 1;
            blocked_count += usize::from(walked_blocked);
            if walked_blocked != brute_blocked {
                disagreed.push(format!(
                    "spot ({:.2}, {:.2}, {z:.1}), light ({:.2}, {:.2}, {light_z:.1}): \
                     walk_cells_exact says {}, the brute-force oracle says {}",
                    at.x,
                    at.y,
                    light_at.x,
                    light_at.y,
                    if walked_blocked { "blocked" } else { "open" },
                    if brute_blocked { "blocked" } else { "open" },
                ));
            }
        }
    }
    assert!(
        compared > 100,
        "the grid compared only {compared} spot/light pairs",
    );
    assert!(
        blocked_count > 0 && blocked_count < compared,
        "{blocked_count} of {compared} pairs were blocked; the grid should mix both",
    );
    assert!(
        disagreed.is_empty(),
        "{} of {compared} disagreed:\n{}",
        disagreed.len(),
        disagreed.join("\n"),
    );
}

/// The same claim as [`a_fuzzed_flame_near_a_row_edge_agrees_with_the_brute_force_oracle`],
/// through [`light::sample_exact`] instead of [`light::sample`] —
/// [`a_brute_force_oracle_agrees_with_the_exact_walk_over_a_grid_of_lights`]'s
/// own reason for existing, fuzzed the same way its own counterpart is:
/// biased near the wall row's own edges, the shape that broke `corner_tie`
/// before it carried a per-axis clamp. `candidate_tiles` probes both
/// single-axis neighbours at every corner unconditionally, so this domain —
/// deliberately excluded from the grid test above for being too close to a
/// real corner — is exactly where the exact walk and `walk_cells` are least
/// obliged to agree with each other, which is what makes it worth checking
/// each of them against a third, independent oracle instead.
#[test]
fn a_fuzzed_flame_near_a_row_edge_agrees_with_the_brute_force_oracle_through_the_exact_walk() {
    use openshard_client_render::occlusion::{Builder, Shape};
    use openshard_protocol::wire::Graphic;
    use openshard_uofiles::tiledata::{StaticTile, TileFlags};
    use proptest::prelude::*;

    proptest!(ProptestConfig::with_cases(512), |(
        spot_dx in 0.05_f32..8.0,
        spot_frac in 0.05_f32..0.95,
        spot_z in 1.0_f32..19.0,
        flame_dx in 0.05_f32..8.0,
        flame_z in 1.0_f32..19.0,
        row in prop_oneof![Just(100.0_f32), Just(101.0_f32)],
        frac in -0.3_f32..0.3,
    )| {
        let flame_y = row + frac;

        let wall = StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT),
            height: 20,
            ..StaticTile::default()
        };
        let mut grid = Builder::new(openshard_client_render::camera::TileBounds {
            min_x: 90,
            max_x: 110,
            min_y: 90,
            max_y: 110,
        });
        grid.add(100, 100, 0, Graphic(0x0100), &wall, Shape::UNREAD);
        let occlusion = grid.finish(&Cutaway::OPEN);

        let spot_at = Vec2::new(101.0 + spot_dx, 100.0 + spot_frac);
        let spot_tile = (spot_at.x.floor() as i32, spot_at.y.floor() as i32);

        let light_at = Vec2::new(99.0 - flame_dx, flame_y);
        let target_tile = (light_at.x.floor() as i32, light_at.y.floor() as i32);
        prop_assume!(target_tile != (100, 100));

        let light = light::Light {
            at: light_at,
            z: flame_z,
            radius: 30.0,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam: None,
        };
        let lighting = Lighting {
            ambient: light::NIGHT,
            lights: vec![light],
            occlusion: occlusion.clone(),
            sun: None,
            view: debug::View::default(),
        };

        let spot = Spot::flat(spot_at, spot_z, spot_tile);
        let sample = light::sample_exact(spot, &lighting);
        let Some(reach) = sample.reaches.iter().find(|reach| reach.within) else {
            return Ok(());
        };
        let walked_blocked = reach.through <= 0.004;
        let brute_blocked = brute_force_blocked(
            [spot_at.x, spot_at.y, spot_z],
            [light_at.x, light_at.y, flame_z],
            spot_tile,
            target_tile,
            true,
            &occlusion,
        );
        prop_assert_eq!(
            walked_blocked,
            brute_blocked,
            "spot ({:.4}, {:.4}, {:.2}) tile {:?}, light ({:.4}, {:.4}, {:.2}) \
             tile {:?}: walk_cells_exact says {}, the brute-force oracle says {}",
            spot_at.x, spot_at.y, spot_z, spot_tile,
            light_at.x, light_at.y, flame_z, target_tile,
            if walked_blocked { "blocked" } else { "open" },
            if brute_blocked { "blocked" } else { "open" },
        );
    });
}
