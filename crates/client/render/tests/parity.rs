//! **P3 — the gate.** `docs/parity.md`: one real place, assembled twice — once
//! the client's way (statics off the map, through [`crate::statics::collect`])
//! and once the tool's way (statics pulled into [`GroundItem`]s and drawn
//! through [`crate::items::collect`]) — and the G-buffer compared plane by
//! plane. D1 already made the two routes share one [`frame::assemble`]; what
//! this gates is the thing D1 did not give them, named in the backlog as still
//! open: a shared *route through* the assembly, not just a shared assembly.
//!
//! # Why the tool's map is real-anchored here and not near the origin
//!
//! `isolated_scene`'s ordinary anchor translates a real place onto
//! [`SYN_ANCHOR_NEAR_THE_ORIGIN`](https://docs.rs) — cheap, and irrelevant to a
//! byte comparison: a G-buffer's position and place planes carry *absolute*
//! world coordinates, so two frames anchored at different numbers would differ
//! everywhere before a single input was allowed to. D4's own `_ANCHOR_REAL`
//! knob is the one that removes the translation rather than the one that adds
//! it, so this gate builds its synthetic map the same way that knob does:
//! [`WorldMap::from_blocks`] filled from the real map's own land, one for one, wide
//! enough to hold every place this test looks at. Measured at 32ms for a block
//! covering the whole of the area these three places sit in — D4's own backlog
//! item, closed for the size this gate needs.
//!
//! # What is shared and what is not
//!
//! D6: an input that is allowed to differ between the two routes is exactly
//! one — which of the four collectors puts the statics in the frame. Camera,
//! cutaway, tuning, the carried flame, the sky, the clock: one value, handed to
//! both [`frame::Inputs`] literals. The cutaway is read off the *real* map even
//! for the tool's frame — [`Cutaway::at`] walks a stack of tiles the synthetic
//! map, which carries no statics at all, cannot answer about, and this gate is
//! not testing whether a synthetic map can stand in for cutaway; `isolated_scene`
//! never asks it to either, and takes the field as an argument instead.
//!
//! Gated on `OPENSHARD_CLIENT`, like every test here that needs the client's
//! own files, and a no-op without it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use openshard_client_render::animate::StaticAnimations;
use openshard_client_render::atlas::{LandAtlas, StaticAtlas, TexmapAtlas};
use openshard_client_render::blit::{self, Blit, ViewportRect};
use openshard_client_render::camera::Camera;
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::debug::View;
use openshard_client_render::frame::{self, Impostor};
use openshard_client_render::geometry::Vec2;
use openshard_client_render::hue::HueRamp;
use openshard_client_render::items::{self, GroundItem};
use openshard_client_render::light::{self, Lighting, Tuning};
use openshard_client_render::renderer::{self, GroundRenderer, MeshFaceRenderer, SpriteRenderer, Target};
use openshard_client_render::statics::StaticGeometry;
use openshard_client_render::{dump, ground, statics};
use openshard_map::grid::BlockExtent;
use openshard_map::map::{LandCell, WorldMap};
use openshard_protocol::direction::Direction;
use openshard_protocol::items::ItemAmount;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_tiles::TileData;
use openshard_uofiles::animdata::AnimData;
use openshard_uofiles::art::Art;
use openshard_uofiles::hues::Hues;
use openshard_uofiles::texmaps::TexMaps;

/// Three real places with a house on them, named in `docs/parity.md` itself:
/// the corner `tests/dump.rs` already dumps, the stair corner phase 6 was
/// built against, and the cabinet the shard reader's own entry is about.
/// Chosen for a lit pixel too (backlog: "a gate laid on a frame with no flame
/// that reaches anything is ... blind about the light") — every place here
/// gets a carried flame at its own coordinates.
///
/// **The first one's `z` was `0`, and that is what made five planes gate
/// nothing.** A place is a *stance* and not a column: the land at
/// `(1501, 1659)` is at `z = 20` and the floor standing on it at `27`, so a
/// camera aimed at `0` is a player twenty units under the ground. What follows
/// is not "a slightly different view" — [`Cutaway::at`] takes its storey from
/// the same number, so the whole building above was cut away, **78% of the
/// frame came back as the cleared background**, and [`light::collect`] found
/// **one** flame (the carried one) where the same place at `27` finds eight.
/// No flame reached a single drawn pixel there, which left `light`, `flames`,
/// `shadow` and `reach` *constant* — and a constant plane reports zero
/// differing pixels whatever the geometry does. See [`plane_colours`], which
/// is the gate against ever taking such a zero for agreement again.
const PLACES: [Point; 3] = [
    Point::new(1501, 1659, 27),
    Point::new(1497, 1626, 10),
    Point::new(1504, 1655, 27),
];

const VIEWPORT: (u32, u32) = (900, 700);

/// The same viewport one pixel wider and one taller — `docs/parity.md` P5's G2.
///
/// Every picture this repository has ever compared has been drawn on an even
/// extent, by unanimous accident and never by a decision, and the client's own
/// window is whatever the compositor hands it. An odd extent is the input whose
/// *unanimity* hid a defect a person could see from across the room, so it is
/// varied here: once as a second case of the route gate below, and once on its
/// own, where the two extents are compared against each other.
const ODD_VIEWPORT: (u32, u32) = (901, 701);

fn client_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?))
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: openshard_client_render::gbuffer::required_limits(),
        ..Default::default()
    }))
    .ok()
}

/// A synthetic map wide and tall enough to hold every real coordinate any of
/// [`PLACES`]'s cameras can see, filled from the real map's own land — see this
/// module's own doc for why the anchor is real rather than translated.
///
/// Sized off [`light::lit_tiles`] and not [`Camera::visible_tiles`]: the
/// occlusion grid [`light::collect`] builds is the wider of the two (grown by
/// the widest flame's own reach), and a synthetic map that stopped at the
/// narrower bound would starve [`pull_map_statics`] at its own edge before this
/// map's own edge was the reason.
fn synthetic_map_covering(real: &WorldMap, places: &[Point], tuning: &Tuning) -> WorldMap {
    let mut furthest = (0i32, 0i32);
    for &at in places {
        for viewport in [VIEWPORT, ODD_VIEWPORT] {
            let bounds = light::lit_tiles(&Camera::new(at, viewport.0, viewport.1), tuning);
            furthest.0 = furthest.0.max(bounds.max_x);
            furthest.1 = furthest.1.max(bounds.max_y);
        }
    }
    // A block of margin past the furthest bound, the same slack
    // `isolated_scene`'s own `blocks` closure leaves.
    let blocks_wide = (furthest.0.max(0) as u32) / 8 + 4;
    let blocks_down = (furthest.1.max(0) as u32) / 8 + 4;
    WorldMap::from_blocks(
        BlockExtent {
            wide: blocks_wide,
            down: blocks_down,
        },
        |x, y| {
            real.land(x, y).unwrap_or(LandCell {
                tile: openshard_map::map::LandTile(0),
                z: 0,
            })
        },
    )
}

/// The real map's own statics over the cells [`light::collect`] builds its
/// occlusion grid from, as the [`GroundItem`]s the tool's route draws them
/// through.
///
/// **[`light::lit_tiles`] and not [`Camera::visible_tiles`].** The drawn
/// sprites only need the narrower bound, but the *grid* [`occlusion::collect`]
/// builds is grown by the widest flame's own reach — pulling only the visible
/// cells starves the tool's grid of exactly the occluders standing just off
/// the edge of what is drawn, which is invisible everywhere except at a box
/// whose shape a *neighbour* outside the narrower bound would have changed.
/// Found by this gate itself: the first run of it, over [`Camera::visible_tiles`]
/// alone, put ~1.2% of several G-buffer planes in disagreement at every one of
/// [`PLACES`], concentrated exactly there.
fn pull_map_statics(real: &WorldMap, camera: &Camera, tuning: &Tuning) -> Vec<GroundItem> {
    let bounds = light::lit_tiles(camera, tuning);
    let Some((xs, ys)) = bounds.clamp_to(real.width(), real.height()) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for y in ys {
        for x in xs.clone() {
            for s in real.statics_at(x, y) {
                items.push(GroundItem {
                    amount: ItemAmount::ONE,
                    at: Point::new(x, y, s.z),
                    graphic: s.tile,
                    hue: s.hue,
                });
            }
        }
    }
    items
}

/// Everything one drawn frame leaves behind — [`tests/dump.rs`]'s own `Drawn`,
/// one copy per `docs/parity.md`'s own backlog item about the GPU test
/// scaffolding every file here keeps separately.
struct Drawn {
    world: wgpu::Texture,
    gbuffer: openshard_client_render::gbuffer::Gbuffer,
    lighting: Lighting,
    ground: GroundRenderer,
    statics: SpriteRenderer,
    mesh: MeshFaceRenderer,
    /// What this frame was asked for, as [`frame::Inputs::summary`] states it.
    ///
    /// Kept because a picture on its own cannot be reproduced and because P5's
    /// G3 is about this string: two frames that differ by the window's width
    /// have to *diff* here, or the summary is not naming an input that decides
    /// the picture.
    summary: String,
}

/// Assemble one frame — `map`'s own statics if `items` is empty, `items` if
/// `map` carries none — draw its three world passes, and stop before the blit.
/// The one function both routes go through, so the only thing a caller can
/// differ on is which of `map`/`items` it hands over.
#[allow(clippy::too_many_arguments)]
fn assemble_and_draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    art: &Art,
    dir: &Path,
    tiledata: &TileData,
    animations: &StaticAnimations,
    hue_ramp: &HueRamp,
    tuning: &Tuning,
    map: &WorldMap,
    items: &[GroundItem],
    camera: &Camera,
    cutaway: &Cutaway,
    at: Point,
) -> Drawn {
    let land_wanted = ground::visible_graphics(map, camera);
    let land = LandAtlas::build(art, land_wanted.iter().copied()).expect("a screen of land fits");
    let texmaps = TexmapAtlas::build(
        &TexMaps::open(dir).expect("texidx.mul and texmaps.mul"),
        tiledata,
        land_wanted,
    )
    .expect("a screen of textures fits");
    // `light::lit_tiles`, not `camera.visible_tiles` (which
    // `statics::visible_graphics` uses): the occlusion grid this frame's own
    // lighting builds reads this same atlas for an occluder's facing
    // (`occlusion::shape_of`), and the grid is grown by the widest flame's own
    // reach. A graphic standing only in that margin — off screen, still
    // occluding — needs the atlas to hold it or its shape falls back to the
    // whole tile. Both routes read the wide bound here so that fallback is not
    // itself a difference between them; whether the *live* client's own atlas
    // (grown from `App::wanted_now`, the narrow bound) ever meets a graphic
    // that exists only in its own margin is a separate question this gate does
    // not ask. Found by the gate itself: the first version of it, over the
    // narrow bound on both sides, put ~1.2% of several planes in disagreement
    // at every one of `PLACES`, and forty-six percent of `solid` at the stair
    // corner, where the tool's item list (which spans the wide bound
    // regardless) had already outgrown its own narrower atlas.
    let mut needed: BTreeSet<Graphic> = BTreeSet::new();
    statics::graphics_in(map, light::lit_tiles(camera, tuning), animations, &mut needed);
    needed.extend(items::needed_graphics(items, animations));
    let static_atlas = StaticAtlas::build(art, needed).expect("the scene's own statics fit");

    let mut fades = openshard_client_render::cutaway::Fades::default();
    let inputs = frame::Inputs {
        map,
        items,
        camera,
        tiledata,
        animations,
        cutaway,
        interior: None,
        land: &land,
        texmaps: &texmaps,
        statics: &static_atlas,
        // Night and flat, the client's F10-on picture — a lit frame is the one
        // whose planes disagree with each other.
        sky: Some(light::NIGHT.flattened()),
        sun: None,
        // A flame at the place itself: the backlog item this gate closes says a
        // comparison with nothing lighting it is blind to the light entirely.
        carried: Some((at, Vec2::default(), Direction::South)),
        tuning,
        flame_time: 0.0,
        bake: None,
        highlight: None,
        impostor: Impostor::Met,
        draw: frame::Draw::EVERYTHING,
        view: View::Lit,
        dead: false,
        player_rect: None,
        player_mask: None,
        fades: &mut fades,
    };

    let summary = inputs.summary();

    let frame::Frame {
        lighting,
        ground: ground_quads,
        statics:
            StaticGeometry {
                quads: static_quads,
                cutaway_quads: _,
                cutaway_boxes: _,
                mesh_vertices,
                mesh_rows,
                boxes,
            },
    } = frame::assemble(inputs);

    let (width, height) = camera.image_size();
    let format = blit::WORLD_FORMAT;
    let world = blit::world_texture(device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(device, width, height);
    let gbuffer_views = gbuffer.views();
    let depth = renderer::depth_texture(device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

    let mut ground_pass = GroundRenderer::new(device, queue, format, &land, &texmaps);
    let mut statics_pass = SpriteRenderer::new(device, queue, format, static_atlas.pixels(), hue_ramp);
    let mut mesh_pass = MeshFaceRenderer::new(device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target = Target {
        gbuffer: &gbuffer_views,
        view: &world_view,
        depth: &depth_view,
        width,
        height,
        projection: camera.projection(),
    };
    ground_pass.render(device, queue, &mut encoder, target, &ground_quads);
    statics_pass.render(device, queue, &mut encoder, target, &static_quads, &boxes, None);
    mesh_pass.render(device, queue, &mut encoder, target, &mesh_vertices, &mesh_rows);
    queue.submit([encoder.finish()]);

    Drawn {
        world,
        gbuffer,
        lighting,
        ground: ground_pass,
        statics: statics_pass,
        mesh: mesh_pass,
        summary,
    }
}

/// One assembled [`Drawn`] frame, every [`View::ALL`] plane of it, raw RGBA8.
fn dump_all_planes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    blit: &mut Blit,
    into: &wgpu::Texture,
    drawn: &Drawn,
    dummy_mobiles: &wgpu::Buffer,
    viewport: (u32, u32),
) -> Vec<(View, Vec<u8>)> {
    let into_view = into.create_view(&wgpu::TextureViewDescriptor::default());
    let world_view = drawn.world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer_views = drawn.gbuffer.views();
    let rect = ViewportRect {
        x: 0,
        y: 0,
        width: viewport.0,
        height: viewport.1,
    };
    dump::plane_bytes(
        device,
        queue,
        blit,
        into,
        blit::Frame {
            target: &into_view,
            world: &world_view,
            gbuffer: &gbuffer_views,
            face_instances: drawn.statics.instances_buffer(),
            item_instances: drawn.statics.instances_buffer(),
            mobile_instances: dummy_mobiles,
            mesh_instances: drawn.mesh.rows_buffer(),
            ground_instances: drawn.ground.instances_buffer(),
            zoom: openshard_client_render::camera::Zoom::ONE,
            rect,
        },
        &drawn.lighting,
        &View::ALL,
    )
}

/// **How many distinct colours a plane holds where something was drawn** — the
/// one number that says whether a count of zero differing pixels is evidence.
///
/// `docs/parity.md`'s own backlog: five planes came back `0 of 630,000` from a
/// positive control that reddened nine others, and the entry recorded two
/// readings of it — either they are blind to a difference that large, or they
/// are not drawn from the frame under test at all. Measuring says a third
/// thing, and it is the one nothing in the file could have said: at the place
/// the control ran, those planes held **one colour**. A plane with one colour
/// agrees with every other plane of one colour, and no mutation of the
/// geometry can make it disagree.
///
/// The background is excluded by its alpha rather than by its colour:
/// `blit.wesl` returns the world image's own alpha, and a fragment nothing was
/// drawn at carries the cleared `0`. Counting it would let a frame that drew
/// *nothing at all* read as two colours and pass.
fn plane_colours(pixels: &[u8]) -> usize {
    let mut seen = BTreeSet::new();
    for pixel in pixels.chunks_exact(4) {
        if pixel[3] != 0 {
            seen.insert([pixel[0], pixel[1], pixel[2]]);
        }
    }
    seen.len()
}

/// The planes this gate's own inputs hold constant, which therefore gate
/// nothing and are excluded from [`plane_colours`]'s assertion by name.
///
/// One entry, and it is the sun: every [`frame::Inputs`] here sets
/// `sun: None`, so `blit.wesl`'s sun branch answers its one "there is no sun"
/// blue at every pixel. **Listed and not tolerated** — D6 says an input that
/// differs is set the same or the case is not gated, and the same sentence
/// read from the other end says a plane the inputs flatten is not gated
/// either. What varying it would cost is `docs/parity.md`'s backlog.
const CONSTANT_BY_CONSTRUCTION: [View; 1] = [View::Sun];

/// How many pixels of two same-sized RGBA8 buffers disagree.
fn differing_pixels(a: &[u8], b: &[u8]) -> usize {
    assert_eq!(
        a.len(),
        b.len(),
        "two dumps of the same rect came back different sizes"
    );
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(p, q)| p != q)
        .count()
}

/// **The gate.** One real place, assembled the client's way and the tool's way,
/// every plane compared. `tool_items` is the one thing D6 allows to differ
/// between the two calls — the positive control below passes the wrong one on
/// purpose.
#[allow(clippy::too_many_arguments)]
fn gate_at(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    art: &Art,
    dir: &Path,
    tiledata: &TileData,
    animations: &StaticAnimations,
    hue_ramp: &HueRamp,
    tuning: &Tuning,
    real_map: &WorldMap,
    synthetic_map: &WorldMap,
    at: Point,
    tool_items: &[GroundItem],
    viewport: (u32, u32),
) -> Vec<(View, usize)> {
    let camera = Camera::new(at, viewport.0, viewport.1);
    // Read off the real map even for the tool's own frame — see this module's
    // own doc for why the synthetic map, carrying no statics, cannot answer it.
    let cutaway = Cutaway::at(real_map, tiledata, at, true);

    let client = assemble_and_draw(
        device,
        queue,
        art,
        dir,
        tiledata,
        animations,
        hue_ramp,
        tuning,
        real_map,
        &[],
        &camera,
        &cutaway,
        at,
    );
    let tool = assemble_and_draw(
        device,
        queue,
        art,
        dir,
        tiledata,
        animations,
        hue_ramp,
        tuning,
        synthetic_map,
        tool_items,
        &camera,
        &cutaway,
        at,
    );

    let into = openshard_client_render::blit::world_texture(device, viewport.0, viewport.1);
    let mut blit = Blit::new(device, blit::WORLD_FORMAT);
    let dummy_mobiles = blit::dummy_instances(device);

    let client_planes = dump_all_planes(device, queue, &mut blit, &into, &client, &dummy_mobiles, viewport);
    let tool_planes = dump_all_planes(device, queue, &mut blit, &into, &tool, &dummy_mobiles, viewport);

    client_planes
        .into_iter()
        .zip(tool_planes)
        .map(|((view, a), (_, b))| {
            // **Before the comparison, not after it.** A plane that holds one
            // colour reports zero differing pixels against anything, so every
            // count below is a claim about the two routes only where this
            // holds. It is asserted on the *client's* frame — the reference
            // side — and at every place and both parities the callers ask for,
            // which is what makes it a statement about the frames this gate
            // actually drew rather than about the one somebody measured once.
            if !CONSTANT_BY_CONSTRUCTION.contains(&view) {
                assert!(
                    plane_colours(&a) > 1,
                    "at {at:?}, {}x{}: the {} plane is one colour over everything this frame drew, \
                     so its count of differing pixels is not evidence of anything — see PLACES's \
                     own doc for how a z of 0 emptied four of these",
                    viewport.0,
                    viewport.1,
                    view.name(),
                );
            }
            (view, differing_pixels(&a, &b))
        })
        .collect()
}

/// The client's own files, loaded once: `docs/parity.md`'s P3 is a comparison,
/// not a load, and every place gated here reads the same tables.
struct Client {
    dir: PathBuf,
    art: Art,
    tiledata: TileData,
    animations: StaticAnimations,
    hue_ramp: HueRamp,
    tuning: Tuning,
    real_map: WorldMap,
    synthetic_map: WorldMap,
}

fn load(dir: PathBuf) -> Client {
    let real_map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let tiledata = openshard_uofiles::tiledata::load(dir.join("tiledata.mul"))
        .expect("tiledata.mul")
        .tiles;
    let animdata = AnimData::load(&dir).expect("animdata.mul");
    let animations = StaticAnimations::build(&animdata, &tiledata);
    let hue_ramp = HueRamp::build(&Hues::load(dir.join("hues.mul")).expect("hues.mul"));
    let tuning = Tuning::DEFAULT;
    let synthetic_map = synthetic_map_covering(&real_map, &PLACES, &tuning);
    Client {
        dir,
        art,
        tiledata,
        animations,
        hue_ramp,
        tuning,
        real_map,
        synthetic_map,
    }
}

/// **Done when it is green at three places with a house on them** —
/// `docs/parity.md` P3's own words. `map: &WorldMap` reached through
/// `statics::collect` and `items: &[GroundItem]` reached through
/// `items::collect` are D1's two routes into one assembly; this is the gate
/// that they draw the same thing, not only that they are called the same way.
#[test]
fn the_map_route_and_the_item_route_agree_pixel_for_pixel_at_three_real_places() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let c = load(dir);

    for at in PLACES {
        // **Both parities of the viewport at every place** — P5's G2. It doubles
        // this gate's cost and buys the half G1 cannot reach: G1 is arithmetic on
        // `Camera` and says nothing about what the *shader* does with the numbers
        // it is handed, and a commensurability nobody has thought of yet would
        // show up here rather than there.
        for viewport in [VIEWPORT, ODD_VIEWPORT] {
            let tool_items =
                pull_map_statics(&c.real_map, &Camera::new(at, viewport.0, viewport.1), &c.tuning);
            let diffs = gate_at(
                &device,
                &queue,
                &c.art,
                &c.dir,
                &c.tiledata,
                &c.animations,
                &c.hue_ramp,
                &c.tuning,
                &c.real_map,
                &c.synthetic_map,
                at,
                &tool_items,
                viewport,
            );
            for (view, count) in &diffs {
                assert_eq!(
                    *count,
                    0,
                    "at {at:?}, {}x{}: the {} plane differs in {count} of {} pixels between the map \
                     route and the item route — D6 says an input that differs is set the same or the \
                     case is not gated",
                    viewport.0,
                    viewport.1,
                    view.name(),
                    viewport.0 * viewport.1,
                );
            }
            eprintln!(
                "{at:?} at {}x{}: {} items, every plane byte-identical",
                viewport.0,
                viewport.1,
                tool_items.len(),
            );
        }
    }
}

/// **A frame drawn at an odd extent is the even one with a column added, not the
/// even one shifted half a pixel** — `docs/parity.md` P5's G2, and the gate the
/// window-parity repair itself is held by.
///
/// Everything upstream of the shader's last line is *identical* between a
/// `900x700` camera and a `901x701` one, and by integer division rather than by
/// coincidence: `render_width() / 2` is 450 for both, so the eye, the projection's
/// origin, [`Camera::visible_tiles`] and therefore every collected static, every
/// atlas and the whole occlusion grid come out the same. Both premises are
/// asserted below rather than described, because the comparison is only about
/// the one line if they hold.
///
/// What is left is `floor(viewport.size * 0.5)`. Floored, both frames centre the
/// world on real pixel 450 and the odd one is the even one with a column and a
/// row of extra world on its far edges. Unfloored, the odd frame centres on
/// 450.5 — half a real pixel across the whole picture, which is what puts a
/// primary sample on a box's own vertical corner and draws the green line down
/// every `+X` wall.
///
/// **Removing the `floor` from any one of `ground.wesl`, `statics.wesl` or
/// `mesh_face.wesl` turns this red, and it names which plane.** That is P5's own
/// "done when", and it is what the three shaders had no gate for: every other
/// picture test in this repository draws at an even extent, where `floor` of a
/// whole number is that number and the mutation is invisible.
#[test]
fn a_frame_at_an_odd_extent_is_the_even_one_with_a_column_added() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let c = load(dir);
    let at = PLACES[0];

    let even_camera = Camera::new(at, VIEWPORT.0, VIEWPORT.1);
    let odd_camera = Camera::new(at, ODD_VIEWPORT.0, ODD_VIEWPORT.1);
    // The premises. Nothing but the target's own size may differ, or a
    // difference below would be about which statics were collected.
    assert_eq!(
        even_camera.visible_tiles(),
        odd_camera.visible_tiles(),
        "one pixel of extent moved the tiles this frame is built from, so the comparison below \
         would be about geometry rather than about centring",
    );
    assert_eq!(
        even_camera.projection().origin,
        odd_camera.projection().origin,
        "one pixel of extent moved the projection's origin",
    );
    assert_eq!(even_camera.projection().scale, odd_camera.projection().scale);

    let cutaway = Cutaway::at(&c.real_map, &c.tiledata, at, true);
    let draw = |camera: &Camera| {
        assemble_and_draw(
            &device,
            &queue,
            &c.art,
            &c.dir,
            &c.tiledata,
            &c.animations,
            &c.hue_ramp,
            &c.tuning,
            &c.real_map,
            &[],
            camera,
            &cutaway,
            at,
        )
    };
    let even = draw(&even_camera);
    let odd = draw(&odd_camera);

    // **G3, in the one place it can be checked.** Two frames whose only
    // difference is the window's width by one pixel used to diff in no line of
    // `Inputs::summary` at all — which is precisely the failure the summary was
    // built to end, on the input that decided the picture.
    assert_ne!(
        even.summary, odd.summary,
        "the summary says nothing about an extent's parity, so two dumps a pixel apart read as \
         one frame",
    );
    assert!(
        odd.summary.contains("(odd by odd)") && even.summary.contains("(even by even)"),
        "the summary names the parity as a field a person scans for:\n{}\n{}",
        even.summary,
        odd.summary,
    );

    let mut blit = Blit::new(&device, blit::WORLD_FORMAT);
    let dummy_mobiles = blit::dummy_instances(&device);
    let even_into = blit::world_texture(&device, VIEWPORT.0, VIEWPORT.1);
    let odd_into = blit::world_texture(&device, ODD_VIEWPORT.0, ODD_VIEWPORT.1);
    let even_planes = dump_all_planes(
        &device,
        &queue,
        &mut blit,
        &even_into,
        &even,
        &dummy_mobiles,
        VIEWPORT,
    );
    let odd_planes = dump_all_planes(
        &device,
        &queue,
        &mut blit,
        &odd_into,
        &odd,
        &dummy_mobiles,
        ODD_VIEWPORT,
    );

    // The odd picture's own overlap with the even one: rows of the even frame's
    // width, taken off the corner the two share.
    let overlapping = |pixels: &[u8]| {
        let mut out = Vec::with_capacity((VIEWPORT.0 * VIEWPORT.1 * 4) as usize);
        for row in 0..VIEWPORT.1 {
            let from = (row * ODD_VIEWPORT.0 * 4) as usize;
            out.extend_from_slice(&pixels[from..from + (VIEWPORT.0 * 4) as usize]);
        }
        out
    };

    for ((view, even_bytes), (_, odd_bytes)) in even_planes.into_iter().zip(odd_planes) {
        let differing = differing_pixels(&even_bytes, &overlapping(&odd_bytes));
        assert_eq!(
            differing,
            0,
            "the {} plane differs in {differing} of {} shared pixels between a {}x{} frame and a \
             {}x{} one — the world is centred differently at the two parities, which is \
             docs/parity.md's window-parity entry",
            view.name(),
            VIEWPORT.0 * VIEWPORT.1,
            VIEWPORT.0,
            VIEWPORT.1,
            ODD_VIEWPORT.0,
            ODD_VIEWPORT.1,
        );
    }
    eprintln!("{at:?}: every plane of {VIEWPORT:?} is {ODD_VIEWPORT:?}'s own corner, byte for byte");
}

/// **The positive control.** D6 is not optional and neither is this: a gate
/// that cannot be made to fail is not gating. Drop the map's own statics from
/// the tool's route on purpose — the house standing at [`PLACES`]`[0]` goes
/// missing from one side and not the other — and confirm the comparison above
/// says so instead of staying quiet.
#[test]
fn the_gate_is_red_when_the_tool_forgets_the_maps_statics() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let c = load(dir);
    let at = PLACES[0];

    let diffs = gate_at(
        &device,
        &queue,
        &c.art,
        &c.dir,
        &c.tiledata,
        &c.animations,
        &c.hue_ramp,
        &c.tuning,
        &c.real_map,
        &c.synthetic_map,
        at,
        // The mutation: the tool's own statics, dropped rather than pulled.
        &[],
        VIEWPORT,
    );
    let total: usize = diffs.iter().map(|(_, count)| count).sum();
    assert!(
        total > 0,
        "dropping the map's own statics from the tool's route produced no difference at all: \
         the gate cannot see this frame's geometry",
    );
    for (view, count) in &diffs {
        eprintln!(
            "{}: {count} of {} pixels differ",
            view.name(),
            VIEWPORT.0 * VIEWPORT.1
        );
    }
}
