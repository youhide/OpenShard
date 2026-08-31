//! **Does a pass that draws a static name the occluder the grid holds it as?**
//!
//! The claim nobody was stating, and the one field a picture cannot show. Every
//! world pass writes a place attachment beside its pixels, and one word of it is
//! `SpriteQuad::owner` — which occluder of its own tile the thing drawn there is,
//! joined out of this frame's `Occlusion` by `Occlusion::owner_at`. A pass that
//! forgets the join writes `OwnerId::NONE`, which is also what a *glyph* writes,
//! honestly, because a letter over a speaker's head is a point of no occluder. So
//! a forgotten number and an honest one are the same word, and nothing in the tree
//! told them apart.
//!
//! `plan::elevation` had the forgotten one for three phases. The two wall pictures
//! drawn through it passed the whole time — `light::same_run` covered for the
//! exemption the missing owner made unreachable, so not one pixel moved — and it
//! surfaced only when that rule was neutralised for a different reason.
//! `docs/lighting_rebuild.md`'s backlog asks for this file.
//!
//! # What is gated, and what each half is worth
//!
//! - **A world pass** — `items::collect`, the route a scene's statics take. Its
//!   own call to `owner_at` makes "the row equals `owner_at`" circular, so what is
//!   asserted instead is the round trip: the number the row carries, looked up in
//!   the grid's own list for that tile, must name a solid whose owner key is the
//!   very static that was drawn. That catches a join done against the wrong key,
//!   which equality against the same expression cannot.
//! - **The instruments** — `plan::elevation` and `plan::draw`, which build a
//!   G-buffer by hand and so can say anything at all. This is where the defect was
//!   and where the injection below turns red.
//!
//! # Which writers this claim is about, and which it is not
//!
//! `place::Kind` **is** enough to say who must ask, among the passes that write a
//! G-buffer: `Kind::Static` is exactly the set. `Kind::Land` never can be an
//! occluder — `occlusion::Builder` is only ever handed statics and ground items —
//! and `Kind::Mobile` and `Kind::Nothing` are honestly no occluder, which is what
//! `mobiles.rs`, `text.rs` and `gump.rs` stamp.
//!
//! The two passes that write `Kind::Static` beside a bare `NONE` —
//! `statics::selected` and `items::outlined` — are outside the claim rather than
//! exceptions to it: their quads go to the silhouette pipeline, whose vertex
//! layout **declares no `place` attribute at all** (`renderer.rs`, the `mask`
//! pipeline: *"a silhouette has no hue and no place, so those two attributes are
//! simply not declared"*). A row that never reaches an attachment cannot be wrong
//! about one.

use openshard_client_render::animate::StaticAnimations;
use openshard_client_render::debug::View;
use openshard_client_render::facing::Face;
use openshard_client_render::occlusion::{
    self,
    Occlusion,
    OwnerId,
};
use openshard_client_render::place::Kind;
use openshard_client_render::{
    items,
    plan,
    scene,
};

/// The flicker's instant, as everywhere else: a fixture that differed by which
/// tenth of a second it was asked in is not a fixture.
const STILL: f32 = 0.0;

/// Pixels a tile in the elevation. Small: nothing here reads a pixel, and the row
/// count is one per tile whatever the scale is.
const SCALE: u32 = 8;

/// A GPU to draw with, or `None` where there is none — the same bargain the rest
/// of the GPU suite makes.
fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: openshard_client_render::gbuffer::required_limits(),
        ..Default::default()
    }))
    .ok()
}

/// **Which statics the grid holds on `(x, y)` under the number `owner`.**
///
/// The round trip a row's claim has to survive, and it shares nothing with the
/// join that produced the row: `owner_at` goes from a static's key to a number by
/// scanning the cell, and this goes from the number back to the keys. A row built
/// against the wrong key comes back naming a different static, where a comparison
/// against `owner_at`'s own expression would agree with it.
///
/// A list because an owner numbers a *static* and a static can be several solids
/// — a flight's treads, a wall's two panels — so what the caller checks is that
/// every one of them is the same static.
fn named_by(occlusion: &Occlusion, x: i32, y: i32, owner: OwnerId) -> Vec<occlusion::Owner> {
    occlusion
        .ids_at(x, y)
        .iter()
        .zip(occlusion.owners_at(x, y))
        .filter(|(_, held)| **held == owner)
        .map(|(id, _)| occlusion.solid(*id).owner)
        .collect()
}

/// How many rows a run over one scene checked, and how many of those stood on a
/// tile holding more than one occluder.
///
/// The second number is what makes the round trip mean anything. On a tile that
/// holds one solid, *every* non-`NONE` owner resolves to that solid, so the
/// assertion "the number names the static that was drawn" cannot fail there —
/// it degenerates into "the number is in range". A tile carrying two statics is
/// where naming the wrong one is a different answer from naming the right one,
/// and a run that saw none of those has not exercised the claim.
#[derive(Debug, Default)]
struct Census {
    /// Rows examined.
    rows:      usize,
    /// Of those, rows whose tile holds more than one solid.
    ambiguous: usize,
}

/// **Every row a world pass built for one scene names the static the grid holds
/// it as** — the round trip, over `items::collect`'s output.
///
/// `items::collect` is the route a built scene's statics take: a scene has no map
/// statics, since `WorldMap::from_blocks` builds none, and it is the same `quad_of`
/// and the same join `statics::collect` uses for the map's own.
fn world_pass_census(scene: &scene::Scene, atlas: &openshard_client_render::atlas::StaticAtlas) -> Census {
    let lighting = scene.lighting(STILL);
    let geometry = items::collect(
        &scene.items,
        &scene.camera,
        &scene.tiledata,
        &StaticAnimations::default(),
        atlas,
        &scene.cutaway,
        None,
        &lighting.occlusion,
        None,
    );

    let mut census = Census::default();
    for quad in &geometry.quads {
        // Which graphic this row is of. The row does not carry one — an attachment
        // is about *where* a pixel is, not what art it came from — so the scene's
        // own list is what turns a quad back into the thing it draws. One item a
        // place, which the assertion here pins rather than assumes.
        let place = quad.place;
        let of: Vec<_> = scene
            .items
            .iter()
            .filter(|item| (item.at.x, item.at.y, item.at.z) == (place.x, place.y, place.z))
            .collect();
        assert_eq!(
            of.len(),
            1,
            "({}, {}, {}) holds {} of this scene's items, so a row there names none of them \
             in particular",
            place.x,
            place.y,
            place.z,
            of.len(),
        );
        let key = occlusion::Owner::new(place.z, of[0].graphic);
        // The row's own word, back into the type. A `u32` on the wire because an
        // instance buffer's unit is a word; a byte here because an owner is one,
        // and a value that did not fit is the two sides disagreeing about the
        // field rather than a number to truncate.
        let owner = OwnerId::from_raw(u8::try_from(quad.owner).expect("an owner is a byte"));
        assert_ne!(
            owner,
            OwnerId::NONE,
            "{key:?} at ({}, {}) is in the grid and its row is a point of nothing",
            place.x,
            place.y,
        );
        let (x, y) = (i32::from(place.x), i32::from(place.y));
        let held = named_by(&lighting.occlusion, x, y, owner);
        assert!(
            !held.is_empty(),
            "the row at ({x}, {y}) names occluder {owner:?}, which that tile does not hold",
        );
        for solid in &held {
            assert_eq!(
                *solid, key,
                "the row at ({x}, {y}) names {solid:?}, and the static drawn there is {key:?}",
            );
        }
        census.rows += 1;
        if lighting.occlusion.ids_at(x, y).len() > 1 {
            census.ambiguous += 1;
        }
    }
    census
}

/// **A world pass's row names the static the grid holds it as.**
///
/// Two scenes, and the second is what makes the claim reachable:
///
/// - **A run of wall** — four tiles of one graphic at one height. Every row must
///   name a solid whose owner key is `(z 0, WALL)` on its own tile, and none may
///   be `NONE`: the grid accepted this wall, and a fragment of an accepted static
///   is a point of it. But its tiles hold one solid each, so the round trip cannot
///   distinguish "named the right static" from "named a number in range".
/// - **A two-storey house** — its ring tiles carry a wall at `z 0` **and** a wall
///   at `z WALL_HEIGHT`, two owners on one tile. A row that carried its neighbour's
///   number, or the same graphic at the wrong height, resolves to a different key
///   and fails here. That scene carries no pictures of its own, so it is handed
///   [`scene::south_faced_wall`] — the wall run's own atlas — rather than a second
///   copy of one.
#[test]
fn a_world_pass_row_names_the_static_the_grid_holds_it_as() {
    let run = scene::wall_run_lit_from_along_it();
    let atlas = run
        .art
        .as_ref()
        .expect("this scene is drawn from its own silhouettes");
    let along = world_pass_census(&run, atlas);
    assert_eq!(
        along.rows,
        usize::from(scene::RUN),
        "the run is {} tiles and {} rows were checked",
        scene::RUN,
        along.rows,
    );

    let house = scene::storey_over_a_torch();
    let storeys = world_pass_census(&house, &scene::south_faced_wall());
    // A census that examined nothing passes, and a census that examined only
    // unambiguous tiles passes for the wrong reason. Both are stated as numbers.
    assert!(
        storeys.ambiguous > 0,
        "no row of the two-storey house stood on a tile holding two occluders, so the round \
         trip was never asked to tell two statics apart: {storeys:?}",
    );
}

/// **An elevation names the panel it is a picture of.**
///
/// The instrument the defect was in, and the assertion that goes red without the
/// fix. `plan::elevation` draws a run of wall unrolled; every pixel of it is on a
/// panel the grid holds, so every row it builds must name that panel and not
/// `NONE`.
///
/// Three claims, of which the middle one is the whole gate:
///
/// 1. every row is a face row — an elevation draws no ground;
/// 2. **none of them is a point of nothing**, which is exactly what
///    `owner_of = |_| OwnerId::NONE` produced for three phases;
/// 3. and the number each carries names, in the grid, the wall the picture is of —
///    the same round trip the world pass is held to above, so a `Wall::of` naming
///    some other static fails here rather than drawing a picture of it.
#[test]
fn an_elevation_names_the_panel_it_is_a_picture_of() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let scene = scene::wall_run_lit_from_along_it();
    let lighting = scene.lighting(STILL);
    let of = occlusion::Owner::new(0, scene::WALL);
    let wall = plan::Wall {
        from: (scene::CENTRE.x, scene::CENTRE.y),
        face: Face::South,
        tiles: u32::from(scene::RUN),
        top: i32::from(scene::WALL_HEIGHT),
        of,
    };
    let drawn = plan::elevation(&device, &queue, &lighting, View::Flames, wall, SCALE);

    assert_eq!(
        drawn.named.len(),
        usize::from(scene::RUN),
        "an elevation of {} tiles built {} rows",
        scene::RUN,
        drawn.named.len(),
    );
    for row in &drawn.named {
        assert_eq!(row.kind, Kind::Static, "an elevation drew ground: {row:?}");
        assert_ne!(
            row.owner,
            OwnerId::NONE,
            "{row:?} is a point of nothing — exempt from nothing and shadowed by its own panel",
        );
        let held = named_by(
            &lighting.occlusion,
            i32::from(row.tile.0),
            i32::from(row.tile.1),
            row.owner,
        );
        assert!(
            !held.is_empty(),
            "{row:?} names an occluder its own tile does not hold",
        );
        for solid in &held {
            assert_eq!(*solid, of, "{row:?} is a picture of {of:?} and names {solid:?}");
        }
    }
}

/// **A plan view is all ground**, which is what makes it the one instrument with
/// nothing to name.
///
/// The honest half of the claim. A `ground::GroundQuad` carries **no owner field
/// at all**, and that is the right shape rather than a gap: `occlusion::Builder`
/// is only ever handed statics and ground items — see `occlusion::place` — so no
/// land tile is ever a solid, and a field that could only ever hold `NONE` is a
/// field a later writer could get wrong for free.
///
/// So what is asserted is the *premise* and not the `NONE`: every row this
/// instrument builds is a land row. Asserting the owner would be a tautology —
/// `drawn` writes `NONE` for a land row because a `GroundQuad` has nowhere else to
/// put one, so no injection could turn it red, and a gate that cannot go red is
/// not a gate. This one can: a `Kind::Static` fragment in `draw`'s closure builds
/// a face row and fails here, which is exactly the day the question stops being
/// vacuous.
///
/// The scene has a wall in it, so the grid is not empty and the picture covers
/// tiles that hold occluders — the count below says so rather than assuming it.
#[test]
fn a_plan_view_is_all_ground_and_names_no_occluder() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let scene = scene::torch_before_a_wall();
    let lighting = scene.lighting(STILL);
    let (cx, cy) = (scene::CENTRE.x, scene::CENTRE.y);
    let bounds = openshard_client_render::camera::TileBounds {
        min_x: i32::from(cx) - 6,
        max_x: i32::from(cx) + 6,
        min_y: i32::from(cy) - 6,
        max_y: i32::from(cy) + 6,
    };
    let drawn = plan::draw(&device, &queue, &lighting, View::Light, bounds, SCALE);

    assert!(!drawn.named.is_empty(), "the plan view built no rows at all");
    for row in &drawn.named {
        assert_eq!(row.kind, Kind::Land, "a plan view drew a static: {row:?}");
    }

    // And the grid it was drawn over holds occluders on tiles this picture covers,
    // or every line above is a statement about an empty world.
    let holding = drawn
        .named
        .iter()
        .filter(|row| {
            !lighting
                .occlusion
                .ids_at(i32::from(row.tile.0), i32::from(row.tile.1))
                .is_empty()
        })
        .count();
    assert!(
        holding > 0,
        "no tile of this plan view holds an occluder, so `NONE` everywhere means nothing",
    );
}
