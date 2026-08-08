//! What the lighting pass costs, on a real frame at the widest zoom.
//!
//! `docs/lighting.md`'s step 6. The plan claims the world-coordinate pass is
//! *cheaper* per pixel than the screen-space circles it replaced, and the
//! argument for it is a branch: a fragment outside every radius leaves the loop
//! at once, where the old arrangement ran all sixty-four lights for every pixel
//! of the screen. That is a claim about a shader and it is worth a number.
//!
//! # What is measured, and against what
//!
//! The arrangement it replaced is gone from the tree, so there is nothing to run
//! it against — a second copy of a deleted shader would be a benchmark against
//! something nobody ships. What *can* be measured is the mechanism the claim
//! rests on, and it is measured by holding the frame still and changing one
//! thing:
//!
//! - `copy` — [`Lighting::NONE`]. The pass as a plain blit, and the floor
//!   every other number stands on.
//! - `dark` — the frame's real occlusion grid, its ambient, and **no flames**.
//!   The night this client draws indoors with nothing burning. Everything the
//!   lit cases pay except the light loop.
//! - `far` — the same, with the frame's sixty-four flames moved a thousand tiles
//!   away. Every fragment is outside every radius, so this is exactly the branch
//!   the claim is about: `far - dark` is what a screen full of misses costs.
//! - `night` — the flames where the map put them. The frame as played.
//! - `sun` — no flames, a midday sun: the directional walk, which runs on every
//!   ground pixel rather than only inside a pool. Off by default in the app
//!   until this said what it costs.
//!
//! Each case builds its own [`Blit`], so each pays its own uniform write and its
//! own grid upload — the same bytes in every case, so a difference between two
//! of them is the shader and not the transfer.
//!
//! # Why a batch and not a frame
//!
//! One pass over two million fragments is under the noise of a submission, so
//! each case is [`REPEATS`] passes in one encoder, submitted once and waited on.
//! What comes out is divided back down to one frame. The batches are run more
//! than once and the **fastest** is kept: a slow batch is a machine doing
//! something else, and the minimum is the only estimator here that a busy
//! desktop cannot inflate.
//!
//! # The companion that says the data is real
//!
//! A lighting benchmark on a frame with no lights in it is fast, green and
//! meaningless — this repository has produced that shape of result before. So
//! the numbers are printed only after the frame is shown to be one: the flames
//! collected are counted, the standing cells of the grid are counted, and the
//! `night` frame is read back and compared with `dark` pixel by pixel. A frame
//! whose lights changed nothing is a failure here, not a fast result.
//!
//! Two things gate it, both honest skips: `OPENSHARD_CLIENT`, because no client
//! files live in this repository, and an adapter, because not every machine has
//! one. Ignored besides, because it is a measurement and not an assertion about
//! a number nobody has agreed on:
//!
//! ```sh
//! OPENSHARD_CLIENT=… cargo test --release -p openshard-client-render \
//!     --test cost -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use openshard_client_render::animate::StaticAnimations;
use openshard_client_render::atlas::{LandAtlas, StaticAtlas, TexmapAtlas};
use openshard_client_render::blit::{Blit, ViewportRect};
use openshard_client_render::camera::{Camera, Zoom};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::geometry::Vec2;
use openshard_client_render::light::{self, Lighting};
use openshard_client_render::renderer::{self, GroundRenderer, SpriteRenderer, Target};
use openshard_client_render::solids::SolidsRenderer;
use openshard_client_render::{ground, statics};
use openshard_protocol::world::Point;
use openshard_uofiles::art::Art;
use openshard_uofiles::hues::Hues;
use openshard_uofiles::map::Map;
use openshard_uofiles::texmaps::TexMaps;
use openshard_uofiles::tiledata::TileData;

use openshard_client_render::hue::HueRamp;

/// The window this is measured for, in physical pixels: a 1080p display with
/// nothing docked over the world.
///
/// The fragment count *is* this rectangle — the blit draws one quad over the
/// viewport — so it is the number every per-pixel figure below is divided by,
/// and it is stated rather than read from a surface because a benchmark that
/// depended on the machine's monitor could not be compared with itself.
const VIEWPORT: (u32, u32) = (1920, 1080);

/// The middle of Britain, which is where every other picture in this workspace
/// is taken from.
const BRITAIN: Point = Point::new(1495, 1629, 0);

/// How many passes go into one batch. Enough that the batch is milliseconds
/// rather than the noise of a submission.
const REPEATS: u32 = 32;

/// How many batches each case is run for. The fastest is the reading — see the
/// module docs.
const BATCHES: u32 = 5;

/// How far away `far`'s flames are put, in tiles. Well outside the grid, so no
/// fragment is inside any radius and no ray is ever walked.
const FAR_AWAY: f32 = 1000.0;

/// The client's files, or `None` where the environment does not point at any.
fn client_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?))
}

/// A GPU to draw with, or `None` where there is none.
///
/// The same four lines as `frame.rs`'s: a test binary is its own crate and
/// cannot import another's helpers, and a shared module for one adapter request
/// would be a file to keep in step for no saving.
fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// The widest view the ladder offers — where the world image is largest, the
/// grid holds the most tiles and the most flames are in reach.
fn widest() -> Zoom {
    let mut zoom = Zoom::ONE;
    while !zoom.is_widest() {
        zoom = zoom.scale_down();
    }
    zoom
}

/// Where to point the camera, and how close: `BRITAIN` at [`widest`] unless
/// `OPENSHARD_FRAME_AT=x,y,z` names a place, in which case that place at
/// `Zoom::ONE` — close enough to tell one tile's edge from the next, which is
/// the point of naming a place at all. Replaces the hand edit
/// `docs/lighting.md`'s backlog used to describe: `BRITAIN`'s literal and
/// `widest()` → `Zoom::ONE`, run, then `git checkout -- tests/cost.rs`.
fn frame_point_and_zoom() -> (Point, Zoom) {
    match std::env::var("OPENSHARD_FRAME_AT") {
        Err(_) => (BRITAIN, widest()),
        Ok(spec) => {
            let coords: Vec<&str> = spec.split(',').collect();
            let [x, y, z] = coords[..] else {
                panic!("OPENSHARD_FRAME_AT wants `x,y,z`, got {spec:?}");
            };
            let point = Point::new(
                x.trim()
                    .parse()
                    .unwrap_or_else(|_| panic!("OPENSHARD_FRAME_AT x: {x:?}")),
                y.trim()
                    .parse()
                    .unwrap_or_else(|_| panic!("OPENSHARD_FRAME_AT y: {y:?}")),
                z.trim()
                    .parse()
                    .unwrap_or_else(|_| panic!("OPENSHARD_FRAME_AT z: {z:?}")),
            );
            (point, Zoom::ONE)
        }
    }
}

/// One case's reading.
struct Reading {
    what: &'static str,
    /// The fastest batch, divided back down to one pass.
    per_frame: Duration,
}

impl Reading {
    /// Nanoseconds a fragment, which is the number the claim is about.
    fn per_pixel(&self) -> f64 {
        self.per_frame.as_secs_f64() * 1e9 / f64::from(VIEWPORT.0 * VIEWPORT.1)
    }
}

/// Run one case: a fresh [`Blit`], a warm-up batch, then [`BATCHES`] measured
/// ones, keeping the fastest.
///
/// The warm-up is not politeness: the first pass through a pipeline pays for the
/// driver compiling it and for both grid textures being created at the frame's
/// size, and counted in it would make whichever case ran first the slow one.
#[allow(clippy::too_many_arguments)]
fn measure(
    what: &'static str,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world: &wgpu::TextureView,
    place: &wgpu::TextureView,
    face_instances: &wgpu::Buffer,
    mobile_instances: &wgpu::Buffer,
    mesh_instances: &wgpu::Buffer,
    ground_instances: &wgpu::Buffer,
    surface: &wgpu::TextureView,
    lighting: &Lighting,
    zoom: Zoom,
) -> Reading {
    let mut blit = Blit::new(device, openshard_client_render::blit::WORLD_FORMAT);
    let frame = openshard_client_render::blit::Frame {
        target: surface,
        world,
        place,
        face_instances,
        mobile_instances,
        mesh_instances,
        ground_instances,
        zoom,
        rect: ViewportRect {
            x: 0,
            y: 0,
            width: VIEWPORT.0,
            height: VIEWPORT.1,
        },
    };

    let mut batch = |passes: u32| {
        let start = Instant::now();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        for _ in 0..passes {
            blit.render(device, queue, &mut encoder, frame, lighting);
        }
        queue.submit([encoder.finish()]);
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("waiting on our own submission");
        start.elapsed()
    };

    batch(REPEATS);
    let mut fastest = Duration::MAX;
    for _ in 0..BATCHES {
        fastest = fastest.min(batch(REPEATS));
    }
    Reading {
        what,
        per_frame: fastest / REPEATS,
    }
}

/// Read a texture back as RGBA8 rows, for the companion that says the lights did
/// something.
fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let (width, height) = (texture.width(), texture.height());
    assert_eq!(width * 4 % 256, 0, "a row copy has to be 256-byte aligned");
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(width) * u64::from(height) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("mapping a buffer this test just wrote");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting on our own submission");
    let pixels = slice
        .get_mapped_range()
        .expect("the map completed above")
        .to_vec();
    readback.unmap();
    pixels
}

/// Draw the world image and its place attachment once, and light it every way.
///
/// The world passes run once and their output is reused by every case: what is
/// being timed is the blit, and re-drawing Britain between the readings would
/// put the ground pass into all five of them equally and the noise of an atlas
/// into whichever one grew it.
#[test]
#[ignore = "a measurement, not an assertion; needs a client and an adapter"]
fn what_the_lighting_pass_costs_at_the_widest_zoom() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        eprintln!("no client files or no adapter: nothing measured");
        return;
    };

    let map = Map::load_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");

    let (point, zoom) = frame_point_and_zoom();
    let mut camera = Camera::new(point, VIEWPORT.0, VIEWPORT.1);
    camera.zoom_about(0, 0, zoom);
    let (width, height) = camera.image_size();
    if point == BRITAIN {
        assert!(camera.minifies(), "the widest rung of the ladder minifies");
    }

    // The world, drawn once, exactly as the app draws it.
    let wanted = ground::visible_graphics(&map, &camera);
    let land = LandAtlas::build(&art, wanted.iter().copied()).expect("a screen of land fits");
    let texmaps = {
        let texmaps = TexMaps::open(&dir).expect("texidx.mul and texmaps.mul");
        TexmapAtlas::build(&texmaps, &tiledata, wanted).expect("a screen of textures fits")
    };
    let animations = StaticAnimations::default();
    let quads = ground::collect(&map, &camera, &land, &texmaps, &Cutaway::OPEN);
    let static_atlas = StaticAtlas::build(&art, statics::visible_graphics(&map, &camera, &animations))
        .expect("a screen of statics fits");
    let static_quads = statics::collect(
        &map,
        &camera,
        &tiledata,
        &animations,
        &static_atlas,
        &Cutaway::OPEN,
        &openshard_client_render::occlusion::Occlusion::EMPTY,
    )
    .quads;
    // `docs/gbuffer.md` step 2: the id width has to be sized against a real
    // frame's *face* count, not the object count decision 3 replaces. Quads are
    // objects — one per static, one per land cell — and every one of them is
    // exactly one face except a corner, which decision 3 splits into two. So the
    // face count this frame would produce is the quad count plus one more for
    // every corner-stance quad; treads (top+riser, decision 3) are not counted
    // here because nothing in the tree decomposes one yet (`gbuffer.md` step 4)
    // — see the doc for how that gap is carried forward instead of guessed at.
    let corner_statics = static_quads
        .iter()
        .filter(|quad| quad.place.stance as u8 >= openshard_client_render::place::STANCE_CORNER)
        .count();
    eprintln!(
        "{} ground quads, {} static quads ({corner_statics} of them corners, so {} faces \
         once decision 3 splits each in two) — `docs/gbuffer.md` step 2",
        quads.len(),
        static_quads.len(),
        static_quads.len() + corner_statics,
    );

    // Step 4 (corners): `split_corners` is what actually gives each corner
    // its second face, and its row count should match the measurement above
    // exactly, not just in the print statement.
    let static_instances = openshard_client_render::sprite::split_corners(static_quads);
    assert_eq!(
        static_instances.rows.len(),
        static_instances.drawn as usize + corner_statics,
        "one shadow row per corner and nothing else grew the buffer",
    );

    let format = openshard_client_render::blit::WORLD_FORMAT;
    let world = openshard_client_render::blit::world_texture(&device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let place = openshard_client_render::place::texture(&device, width, height);
    let place_view = place.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(&device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let hue_ramp = HueRamp::build(&Hues::load(dir.join("hues.mul")).expect("hues.mul"));

    let mut ground_pass = GroundRenderer::new(&device, &queue, format, &land, &texmaps);
    let mut statics_pass = SpriteRenderer::new(&device, &queue, format, static_atlas.pixels(), &hue_ramp);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target = Target {
        place: &place_view,
        view: &world_view,
        depth: &depth_view,
        width,
        height,
        projection: camera.projection(),
    };
    ground_pass.render(&device, &queue, &mut encoder, target, &quads);
    statics_pass.render(
        &device,
        &queue,
        &mut encoder,
        target,
        &static_instances.rows,
        Some(static_instances.drawn),
    );
    queue.submit([encoder.finish()]);

    // The surface the blit draws onto: the viewport, not the world image.
    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width: VIEWPORT.0,
            height: VIEWPORT.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let surface_view = surface.create_view(&wgpu::TextureViewDescriptor::default());
    // No mobile pass in this fixture: the dummy stands in for it.
    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    // No mesh-face pass either: this fixture measures the blit pass alone.
    let dummy_mesh_instances = openshard_client_render::blit::dummy_mesh_instances(&device);

    // The lighting, collected the way the app collects it — and timed, because
    // the walk of the map and the grid it builds are a per-frame cost of this
    // arrangement that the shader's cheapness does not pay for.
    let collect = |time: f32| {
        light::collect(
            &map,
            &[],
            &camera,
            &tiledata,
            &Cutaway::OPEN,
            // Flat, as the app is by default: the sky field is a second thing
            // changing every tile of the picture, and the frame this test dumps
            // is read as a picture of the *flames*. See `docs/lighting.md`,
            // decision 20. It costs the pass nothing either way — the grid is
            // built and uploaded whichever ambient reads it.
            light::NIGHT.flattened(),
            time,
            Some(&static_atlas),
            // Uncached on purpose: `cpu` below is the frame this pass costs
            // without a bake, and the bake is timed against it a few lines down.
            None,
        )
    };
    // Three readings and not one, because they have three different fixes: the
    // walk of the map is what a spatial index would answer, the grid is what
    // keeping the buffer between frames would answer, and the bytes are two
    // allocations of the same rectangle on the way to the queue. A single
    // "2.8ms" names none of them.
    let (mut cpu, mut cpu_grid, mut cpu_bytes) = (Duration::MAX, Duration::MAX, Duration::MAX);
    // The same grid again out of a cache that survives the loop — step 21.5. The
    // first batch is all misses and the rest are all hits, and taking the fastest
    // is what makes this the *steady* reading rather than an average of the two
    // states.
    let mut bake = openshard_client_render::occlusion::bake::Bake::new();
    let mut cpu_baked = Duration::MAX;
    for batch in 0..BATCHES {
        let start = Instant::now();
        let built = collect(batch as f32);
        cpu = cpu.min(start.elapsed());

        let start = Instant::now();
        let grid = openshard_client_render::occlusion::collect(
            &map,
            &[],
            light::lit_tiles(&camera),
            &tiledata,
            &Cutaway::OPEN,
            Some(&static_atlas),
        );
        cpu_grid = cpu_grid.min(start.elapsed());

        let start = Instant::now();
        let cached = openshard_client_render::occlusion::bake::collect(
            &mut bake,
            &map,
            &[],
            light::lit_tiles(&camera),
            &tiledata,
            &Cutaway::OPEN,
            Some(&static_atlas),
        );
        cpu_baked = cpu_baked.min(start.elapsed());
        // The oracle, on the real map rather than on a built town: the cache is
        // only worth anything if what comes out of it is the grid the walk
        // builds, and 25,702 statics over 187x187 tiles is where a per-block
        // read-out that dropped a rim tile or reordered a run would show.
        assert_eq!(
            cached, grid,
            "the baked grid is not the one the walk builds, batch {batch}"
        );

        // What the upload sends. Timed apart and `black_box`ed, because a length
        // nobody reads is an allocation a release build may delete.
        let start = Instant::now();
        std::hint::black_box(
            built.occlusion.bytes().len()
                + grid.field_bytes().len()
                + grid.id_bytes().len()
                + grid.solid_bytes().len(),
        );
        cpu_bytes = cpu_bytes.min(start.elapsed());
    }
    let night = collect(0.0);
    let cells = night.occlusion.boxes().count();
    let grid = night.occlusion.bounds();

    // The companion. A frame with no flames in it would make every reading below
    // a measurement of the ambient.
    assert!(
        !night.lights.is_empty(),
        "no light source in a whole widest-zoom frame of Britain: the cases below would all be `dark`"
    );
    assert!(cells > 0, "nothing stands in the grid, so no ray can be stopped");

    let mut dark = night.clone();
    dark.lights.clear();
    let mut far = night.clone();
    for light in &mut far.lights {
        far_away(&mut light.at);
    }
    let mut sun = dark.clone();
    sun.sun = Some(light::midday());

    let cases = [
        ("copy", Lighting::NONE),
        ("dark", dark.clone()),
        ("far", far),
        ("night", night.clone()),
        ("sun", sun),
    ];
    let readings: Vec<Reading> = cases
        .iter()
        .map(|(what, lighting)| {
            measure(
                what,
                &device,
                &queue,
                &world_view,
                &place_view,
                statics_pass.instances_buffer(),
                &dummy_instances,
                &dummy_mesh_instances,
                ground_pass.instances_buffer(),
                &surface_view,
                lighting,
                zoom,
            )
        })
        .collect();

    // And the other half of "the data is real": the flames have to change the
    // picture. Drawn after the timings so that nothing here is inside them.
    let picture = |lighting: &Lighting| {
        let mut blit = Blit::new(&device, format);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        blit.render(
            &device,
            &queue,
            &mut encoder,
            openshard_client_render::blit::Frame {
                target: &surface_view,
                world: &world_view,
                place: &place_view,
                face_instances: statics_pass.instances_buffer(),
                mobile_instances: &dummy_instances,
                mesh_instances: &dummy_mesh_instances,
                ground_instances: ground_pass.instances_buffer(),
                zoom,
                rect: ViewportRect {
                    x: 0,
                    y: 0,
                    width: VIEWPORT.0,
                    height: VIEWPORT.1,
                },
            },
            lighting,
        );
        queue.submit([encoder.finish()]);
        read_back(&device, &queue, &surface)
    };
    let lit = picture(&night);
    let unlit = picture(&dark);
    let changed = lit
        .chunks_exact(4)
        .zip(unlit.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    let total = (VIEWPORT.0 * VIEWPORT.1) as usize;
    assert!(
        changed > total / 1000,
        "the flames changed {changed} of {total} pixels: the `night` reading is a measurement of `dark`"
    );

    // What the solids view costs on this same frame — `docs/lighting.md` step
    // 23.0's open DoD item, and the reason that view is a pass rather than an
    // overlay drawn by the client's UI toolkit: a picture drawn by egui appears
    // in none of the tests that take the pictures, and a cost drawn by egui
    // cannot be timed beside the passes it runs with.
    //
    // Measured the way the cases above are — a warm batch, then the fastest of
    // several — and over the frame the blit has already lit, which is where it
    // draws in the app. The list is built once and outside the timing: what is
    // being asked is what the *pass* costs, and the walk that produces the list
    // is `light::collect`'s cost, already reported above.
    // Under the cut the app opens with, which is the reading a person compares
    // theirs against: `Cut::Nothing` draws a pier's every plank and is a
    // different picture with a different cost.
    let boxes = openshard_client_render::solid::standing(
        &night.occlusion,
        openshard_client_render::solid::Cut::BelowFeet(point.z),
    );
    let mut solids_pass = SolidsRenderer::new(&device, format);
    let solids_frame = openshard_client_render::solids::Frame {
        target: &surface_view,
        size: VIEWPORT,
        rect: ViewportRect {
            x: 0,
            y: 0,
            width: VIEWPORT.0,
            height: VIEWPORT.1,
        },
    };
    let mut drawn = 0;
    let mut solids_batch = |passes: u32| {
        let start = Instant::now();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        for _ in 0..passes {
            drawn = solids_pass.render(
                &device,
                &queue,
                &mut encoder,
                solids_frame,
                &camera,
                &boxes,
                openshard_client_render::solids::Style::default(),
            );
        }
        queue.submit([encoder.finish()]);
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("waiting on our own submission");
        start.elapsed()
    };
    solids_batch(REPEATS);
    let mut solids_fastest = Duration::MAX;
    for _ in 0..BATCHES {
        solids_fastest = solids_fastest.min(solids_batch(REPEATS));
    }
    let solids = Reading {
        what: "solids",
        per_frame: solids_fastest / REPEATS,
    };

    // A picture for a person, beside the numbers, because step 6 asks for both.
    //
    // `OPENSHARD_FRAME_VIEW` picks which of `debug::View`'s pictures to write,
    // by number, defaulting to the lit frame. The one worth knowing about is
    // `5`, `View::Light`: the lighting with the art thrown away, which is the
    // only way to see a seam or a step in a pool — over real art a wall's own
    // timbers and windows swamp the lighting's own gradient, and a brightness
    // profile taken across a drawn wall measures the picture rather than the
    // pass. It is what a change to how a wall's pixels are placed has to be
    // judged on.
    if let Some(path) = std::env::var_os("OPENSHARD_FRAME_DUMP") {
        let wanted = std::env::var("OPENSHARD_FRAME_VIEW")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .and_then(|v| openshard_client_render::debug::View::ALL.get(v).copied());
        let mut shown = night.clone();
        if let Some(view) = wanted {
            shown.view = view;
        }
        // Redrawn here rather than reusing `lit`, because what follows draws
        // *over* the surface and the surface currently holds whichever picture
        // was taken last.
        let mut frame = picture(&shown);
        // And the geometry over it, when asked: `OPENSHARD_FRAME_SOLIDS=1`. This
        // is the picture step 23.0 is judged on, and the whole reason it can be
        // taken here rather than by pointing a camera at a window.
        if std::env::var_os("OPENSHARD_FRAME_SOLIDS").is_some() {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            solids_pass.render(
                &device,
                &queue,
                &mut encoder,
                solids_frame,
                &camera,
                &boxes,
                openshard_client_render::solids::Style::default(),
            );
            queue.submit([encoder.finish()]);
            frame = read_back(&device, &queue, &surface);
        }
        std::fs::write(
            &path,
            openshard_client_render::png::encode_rgba(VIEWPORT.0, VIEWPORT.1, &frame),
        )
        .expect("writing the frame");
        eprintln!("wrote {} ({:?})", PathBuf::from(&path).display(), wanted);
    }

    eprintln!(
        "\n{point:?} at {zoom}, {}x{} on screen, world image {width}x{height}",
        VIEWPORT.0, VIEWPORT.1
    );
    eprintln!(
        "{} flames, {cells} standing cells in a {}x{} grid holding {} solids under {} \
         references, {} of them lit this frame",
        night.lights.len(),
        grid.width(),
        grid.height(),
        night.occlusion.solid_count(),
        night.occlusion.reference_count(),
        changed,
    );
    // The two counts side by side, because step 23.1's ownership is exactly the
    // difference between them: a solid is what the world holds and a reference is
    // a cell naming one. They are equal until something spans a cell — decision
    // 38.2's spill is the first thing that will — and the *equality* is the fact
    // worth printing, not a redundancy in the line above.
    assert!(
        night.occlusion.reference_count() >= night.occlusion.solid_count(),
        "fewer references than solids: a solid nothing points at is a wall no ray can find",
    );

    // Decision 30.6: how many solids a cell may reference is a **distribution
    // measured over a city** and not a number anybody picks, and whatever is
    // dropped is said out loud rather than silently capped. A total cannot tell
    // ten thousand tiles of one solid from ten thousand of one and a tile of
    // forty, and it is the second that names a bound.
    let histogram = night.occlusion.histogram();
    let standing: usize = histogram[1..].iter().sum();
    eprintln!("solids a standing cell references, of {standing} standing cells:");
    for (count, tiles) in histogram.iter().enumerate().skip(1) {
        if *tiles > 0 {
            eprintln!(
                "  {count:>3}  {tiles:>7}  {:>5.1}%",
                *tiles as f64 * 100.0 / standing as f64
            );
        }
    }
    eprintln!(
        "  and {} solids dropped because their tile was full\n",
        night.occlusion.dropped(),
    );

    eprintln!(
        "CPU a frame: light::collect {:.2}ms — of which the grid {:.2}ms, and {:.2}ms more \
         to lay the three planes out as bytes for the queue\n",
        cpu.as_secs_f64() * 1e3,
        cpu_grid.as_secs_f64() * 1e3,
        cpu_bytes.as_secs_f64() * 1e3,
    );
    // The companion the plan asked for, and it is the whole reading: a cache that
    // is never hit costs what the walk costs and looks identical in a total. The
    // camera does not move in this test, so this is the ceiling — every block
    // wanted is a block already held — and `what_the_grid_costs_to_build` is
    // where a panning eye is measured.
    let (hits, misses) = bake.served();
    eprintln!(
        "the same grid out of a bake: {:.2}ms against {:.2}ms, {} blocks held, \
         {hits} served and {misses} built over {BATCHES} frames of a still camera\n",
        cpu_baked.as_secs_f64() * 1e3,
        cpu_grid.as_secs_f64() * 1e3,
        bake.len(),
    );

    let floor = readings
        .iter()
        .find(|reading| reading.what == "dark")
        .expect("the dark case was measured");
    eprintln!(
        "{:>6}  {:>9}  {:>10}  {:>12}",
        "case", "ms/frame", "ns/pixel", "over dark"
    );
    for reading in &readings {
        let over = reading.per_frame.as_secs_f64() - floor.per_frame.as_secs_f64();
        eprintln!(
            "{:>6}  {:>9.3}  {:>10.3}  {:>+11.3}%",
            reading.what,
            reading.per_frame.as_secs_f64() * 1e3,
            reading.per_pixel(),
            100.0 * over / floor.per_frame.as_secs_f64(),
        );
    }
    // Beside the table rather than in it: the cases above are one pass with one
    // thing changed, and this is a different pass over the same frame — putting
    // it in the same column as `dark` would invite a percentage that compares
    // two unrelated things.
    eprintln!(
        "\nsolids over the same frame: {:.3}ms, {drawn} of {} boxes drawn — \
         `docs/lighting.md` step 23.0",
        solids.per_frame.as_secs_f64() * 1e3,
        boxes.len(),
    );
}

/// Move a flame out of every fragment's reach, keeping everything else about it.
fn far_away(at: &mut Vec2) {
    at.x += FAR_AWAY;
    at.y += FAR_AWAY;
}
