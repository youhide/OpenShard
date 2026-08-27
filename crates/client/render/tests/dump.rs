//! **A real place, assembled and read back with no window in front of it.**
//!
//! `docs/parity.md`'s gate (P3) needs two frames of one place and had only one:
//! the tools could dump a picture and the client — the thing that is actually
//! broken — could not, so a defect visible in one and absent in the other said
//! nothing about either. The client's own half of that is F12
//! (`App::frame_dump`); this is the other shape the backlog named, a headless
//! run that assembles a frame the way the client assembles one and reads its
//! planes back.
//!
//! What it is *for* is the machinery underneath the gate, checked before the
//! gate is written on top of it:
//!
//! - one picture per [`View`], of the size that was asked for;
//! - the view actually reaching the shader, which is the positive control — a
//!   dump that returned thirteen copies of the lit frame would satisfy every
//!   count and answer no question;
//! - [`dump::read_rect`] surviving a width whose rows are not 256-byte aligned
//!   and an origin that is not the corner. Both are the client's ordinary case:
//!   a window is whatever size a person left it, and a docked panel moves the
//!   world's rect off the surface's corner.
//!
//! Gated on `OPENSHARD_CLIENT` like every test here that needs the client's own
//! files, and a no-op without it.

use std::path::PathBuf;

use openshard_client_render::animate::StaticAnimations;
use openshard_client_render::atlas::{LandAtlas, StaticAtlas, TexmapAtlas};
use openshard_client_render::blit::{self, Blit, ViewportRect};
use openshard_client_render::camera::{Camera, RealPixel, Zoom};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::debug::View;
use openshard_client_render::frame::{self, Impostor};
use openshard_client_render::geometry::Vec2;
use openshard_client_render::light::{self, Tuning};
use openshard_client_render::renderer::{self, GroundRenderer, MeshFaceRenderer, SpriteRenderer, Target};
use openshard_client_render::statics::StaticGeometry;
use openshard_client_render::{dump, ground, statics};
use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;
use openshard_uofiles::animdata::AnimData;
use openshard_uofiles::art::Art;
use openshard_uofiles::hues::Hues;
use openshard_uofiles::texmaps::TexMaps;

/// The house corner in Britain every lighting question in this repository has
/// been asked at — `docs/parity.md`'s own coordinate, so the frame this dumps is
/// the frame the plan talks about.
const AT: Point = Point::new(1501, 1659, 0);

/// The same quarter of Britain, stood on the wall run itself rather than beside
/// it — the eye tile a **magnified** frame has to use.
///
/// [`AT`] is a fine place to ask a question of a `1:1` frame and is not one to
/// ask it of a `4x` frame, and that is a property of the *place* rather than a
/// defect anywhere: the nine statics [`AT`] draws all stand in the top-left
/// corner of its 900x700 image, some four hundred pixels from the eye. Magnify
/// and the image shrinks around the eye — `225x175` world pixels at `4x` — so
/// the whole cluster leaves the frame and a correct cull collects nothing.
/// Measured rather than reasoned: at `AT`, `1x` collects 9 statics, `2x` and
/// `4x` collect none; from here the same three zooms collect 109, 54 and 30.
///
/// Found by [`the_magnified_frame_over_a_wall_run_still_collects_its_statics`],
/// which is what keeps the sentence above from becoming a comment nobody can
/// fail.
const ON_THE_WALLS: Point = Point::new(1486, 1664, 0);

/// Deliberately not 256-byte-row aligned (`900 * 4 = 3600`), and deliberately
/// not a round number of tiles: a readback that ignored the copy's padding would
/// return a sheared picture here, and one that panicked on the assertion the
/// tools used to carry would not get this far.
const VIEWPORT: (u32, u32) = (900, 700);

/// The client's files, or `None` when the environment does not point at any.
fn client_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?))
}

/// A GPU to draw with, or `None` where there is none. The client's own limits —
/// see `tests/frame.rs`'s copy for why they are asked for rather than defaulted.
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

/// Everything one drawn frame leaves behind: the world image, what the passes
/// said about each of its pixels, and the light that goes over it.
///
/// Held together because the blit needs all four and they are only valid
/// together — a G-buffer from one frame over another frame's world image is a
/// picture of nothing.
struct Drawn {
    world: wgpu::Texture,
    gbuffer: openshard_client_render::gbuffer::Gbuffer,
    lighting: light::Lighting,
    ground: GroundRenderer,
    statics: SpriteRenderer,
    mesh: MeshFaceRenderer,
    /// How many static pictures the assembly collected — what `draw` above let
    /// through, kept because a count is the only thing about the *drawing* that
    /// the four fields above have already turned into GPU buffers.
    statics_collected: usize,
    /// And how many quads of land, on the same terms.
    land_collected: usize,
}

/// Assemble a frame the way `App::draw` assembles one, draw its three world
/// passes, and stop before the blit.
///
/// The map's own statics and no server items, the player's own cutaway, night
/// with a flame in hand: the client's values, because a fixture that quietly
/// chose easier ones is the coincidence `docs/parity.md` is about.
///
/// `at`, `draw` and `zoom` are the inputs a caller here varies. `at` is the eye
/// tile — [`AT`] for every question asked at `1:1`, and [`ON_THE_WALLS`] for one
/// asked of a magnified frame, for the reason written there. `draw` is
/// everything, or the subset a person has ticked in the World tab
/// ([`frame::Draw`]); `zoom` is the magnification, which every question about
/// `docs/silhouettes.md` needs because the two edges it is about are the same
/// line at `1:1` and are not at `4x`.
fn draw_britain(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    dir: &std::path::Path,
    at: Point,
    draw: frame::Draw,
    zoom: openshard_client_render::camera::Zoom,
) -> Drawn {
    let map = openshard_uofiles::map::read_facet(dir, 0).expect("Felucca");
    let art = Art::open(dir).expect("artLegacyMUL.uop");
    let tiledata = openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");
    let animdata = AnimData::load(dir).expect("animdata.mul");
    let animations = StaticAnimations::build(&animdata, &tiledata);

    let mut camera = Camera::new(at, VIEWPORT.0, VIEWPORT.1);
    // About the middle, so the place the frame is *of* stays the place it is of
    // at every rung — `Camera::zoom_about` holds what is under the cursor fixed,
    // and the corner would slide the house out of a magnified frame.
    camera.zoom_about(RealPixel::new(VIEWPORT.0 as i32 / 2, VIEWPORT.1 as i32 / 2), zoom);
    let cutaway = Cutaway::at(&map, &tiledata, at, true);

    let land_wanted = ground::visible_graphics(&map, &camera);
    let land = LandAtlas::build(&art, land_wanted.iter().copied()).expect("a screen of land fits");
    let texmaps = TexmapAtlas::build(
        &TexMaps::open(dir).expect("texidx.mul and texmaps.mul"),
        &tiledata,
        land_wanted,
    )
    .expect("a screen of textures fits");
    let static_atlas = StaticAtlas::build(&art, statics::visible_graphics(&map, &camera, &animations))
        .expect("a screen of statics fits");

    let tuning = Tuning::DEFAULT;
    let mut fades = openshard_client_render::cutaway::Fades::default();
    let inputs = frame::Inputs {
        map: &map,
        items: &[],
        drawn_items: &[],
        camera: &camera,
        tiledata: &tiledata,
        animations: &animations,
        cutaway: &cutaway,
        interior: None,
        land: &land,
        texmaps: &texmaps,
        statics: openshard_client_render::atlas::StaticArt::Single(&static_atlas),
        // Night and flat, which is what the client draws with F10 on: a lit
        // frame is the one whose planes disagree with each other, and a daylight
        // frame's blit is a copy.
        sky: Some(light::NIGHT.flattened()),
        sun: None,
        carried: Some((at, Vec2::default(), Direction::South)),
        tuning: &tuning,
        flame_time: 0.0,
        bake: None,
        highlight: None,
        impostor: Impostor::Met,
        draw,
        // Set per plane by `dump::planes`; what it is here is what a caller that
        // never dumped would draw.
        view: View::Lit,
        dead: false,
        player_rect: None,
        player_mask: None,
        fades: &mut fades,
    };
    // The summary is the other half of a dump — see `Inputs::summary`. Asked for
    // here so that a change that breaks it breaks a test rather than a person's
    // afternoon, and printed so a failing run says which place it was at.
    let asked_for = inputs.summary();
    assert!(
        asked_for.lines().count() >= 18,
        "a summary shorter than `Inputs` has fields is a summary that has stopped naming all of them:\n{asked_for}",
    );
    println!("{asked_for}");

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
    let mut statics_pass = SpriteRenderer::new(
        device,
        queue,
        format,
        static_atlas.pixels(),
        &openshard_client_render::hue::HueRamp::build(&Hues::load(dir.join("hues.mul")).expect("hues.mul")),
    );
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
        statics_collected: static_quads.len(),
        land_collected: ground_quads.len(),
    }
}

/// A texture the blit can draw into and a copy can read out of, at the size the
/// surface would be.
fn dump_target(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::Texture {
    dump_target_sized(device, format, VIEWPORT.0, VIEWPORT.1)
}

/// [`dump_target`], for a surface bigger than [`VIEWPORT`] — what a window is
/// once a docked panel has left the world less than the whole of it.
fn dump_target_sized(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("dump target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// The width and height a PNG declares, off its own `IHDR` — read rather than
/// trusted, because a picture of the wrong size that opens is exactly what a
/// readback with the padding left in produces.
fn png_size(png: &[u8]) -> (u32, u32) {
    assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10], "not a PNG");
    assert_eq!(&png[12..16], b"IHDR", "the first chunk of a PNG is its header");
    let width = u32::from_be_bytes(png[16..20].try_into().expect("four bytes"));
    let height = u32::from_be_bytes(png[20..24].try_into().expect("four bytes"));
    (width, height)
}

/// **A row is measured in the texture's own texels, not in four bytes.**
///
/// The defect the first press of F12 found, and the reason this test needs
/// neither client files nor a drawn frame: the client dumped into a texture of
/// the *surface's* format, and this machine's compositor offers `Rgba16Float` —
/// eight bytes a texel. A row measured as `width * 4` against that is not a
/// shorter row, it is a copy `wgpu` refuses outright, and the client died on the
/// keypress.
///
/// Both halves matter and both are here: the format's own texel size, and the
/// alignment padding on top of it. `301 * 8 = 2408` is aligned to neither.
#[test]
fn a_readback_measures_a_row_in_the_textures_own_texels() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let rect = ViewportRect {
        x: 0,
        y: 0,
        width: 301,
        height: 97,
    };
    for (format, texel) in [
        (blit::WORLD_FORMAT, 4),
        (wgpu::TextureFormat::Bgra8Unorm, 4),
        (wgpu::TextureFormat::Rgba16Float, 8),
    ] {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("a texture of some format"),
            size: wgpu::Extent3d {
                width: rect.width,
                height: rect.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        assert_eq!(
            dump::read_rect(&device, &queue, &texture, rect).len(),
            (rect.width * rect.height * texel) as usize,
            "{format:?} is {texel} bytes a texel and the readback came back a different length",
        );
    }
}

#[test]
fn a_frame_dumps_one_picture_per_view_at_the_size_asked_for() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let drawn = draw_britain(&device, &queue, &dir, AT, frame::Draw::EVERYTHING, Zoom::ONE);
    let format = blit::WORLD_FORMAT;
    let into = dump_target(&device, format);
    let into_view = into.create_view(&wgpu::TextureViewDescriptor::default());
    let world_view = drawn.world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer_views = drawn.gbuffer.views();
    let mut blit = Blit::new(&device, format);
    let dummy_mobiles = blit::dummy_instances(&device);

    let rect = ViewportRect {
        x: 0,
        y: 0,
        width: VIEWPORT.0,
        height: VIEWPORT.1,
    };
    let planes = dump::planes(
        &device,
        &queue,
        &mut blit,
        &into,
        blit::Frame {
            target: &into_view,
            world: &world_view,
            gbuffer: &gbuffer_views,
            face_instances: drawn.statics.instances_buffer(),
            item_instances: drawn.statics.instances_buffer(),
            mobile_instances: &dummy_mobiles,
            mesh_instances: drawn.mesh.rows_buffer(),
            ground_instances: drawn.ground.instances_buffer(),
            zoom: openshard_client_render::camera::Zoom::ONE,
            rect,
        },
        &drawn.lighting,
        &View::ALL,
    );

    assert_eq!(
        planes.iter().map(|(view, _)| *view).collect::<Vec<_>>(),
        View::ALL.to_vec(),
        "a dump is every plane, in the order it was asked for",
    );
    for (view, png) in &planes {
        assert_eq!(
            png_size(png),
            VIEWPORT,
            "the {} plane came back a different size than the rect it was read from",
            view.name(),
        );
    }

    // **The positive control.** Fifteen pictures of the right size are what a
    // dump that ignored the view would also produce, and it would answer
    // nothing: these three planes are three different questions about one frame
    // — what it looks like, which place each pixel belongs to, and which way
    // each pixel faces — and on a real street they cannot agree.
    let plane = |want: View| {
        &planes
            .iter()
            .find(|(view, _)| *view == want)
            .expect("every view was asked for")
            .1
    };
    for (left, right) in [
        (View::Lit, View::Place),
        (View::Lit, View::Normal),
        (View::Place, View::Normal),
    ] {
        // `assert!` and not `assert_ne!`: the operands are megabytes of PNG, and
        // a failure that prints them is a failure nobody can read.
        assert!(
            plane(left) != plane(right),
            "the {} and {} planes came back identical: the view is not reaching the shader",
            left.name(),
            right.name(),
        );
    }
}

#[test]
fn a_readback_off_the_corner_is_the_same_pixels_shifted() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let drawn = draw_britain(&device, &queue, &dir, AT, frame::Draw::EVERYTHING, Zoom::ONE);
    let format = blit::WORLD_FORMAT;
    let into = dump_target(&device, format);
    let into_view = into.create_view(&wgpu::TextureViewDescriptor::default());
    let world_view = drawn.world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer_views = drawn.gbuffer.views();
    let mut blit = Blit::new(&device, format);
    let dummy_mobiles = blit::dummy_instances(&device);

    let whole = ViewportRect {
        x: 0,
        y: 0,
        width: VIEWPORT.0,
        height: VIEWPORT.1,
    };
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    blit.render(
        &device,
        &queue,
        &mut encoder,
        blit::Frame {
            target: &into_view,
            world: &world_view,
            gbuffer: &gbuffer_views,
            face_instances: drawn.statics.instances_buffer(),
            item_instances: drawn.statics.instances_buffer(),
            mobile_instances: &dummy_mobiles,
            mesh_instances: drawn.mesh.rows_buffer(),
            ground_instances: drawn.ground.instances_buffer(),
            zoom: openshard_client_render::camera::Zoom::ONE,
            rect: whole,
        },
        &drawn.lighting,
    );
    queue.submit([encoder.finish()]);

    // A rect off the corner, of a width that is not aligned either: what a
    // docked panel does to the client's own viewport, and the case the tools'
    // hand-rolled readbacks never had to handle because they always read from
    // `(0, 0)`.
    let corner = ViewportRect {
        x: 37,
        y: 11,
        width: 301,
        height: 97,
    };
    let all = dump::read_rect(&device, &queue, &into, whole);
    let part = dump::read_rect(&device, &queue, &into, corner);
    assert_eq!(
        part.len(),
        (corner.width * corner.height * 4) as usize,
        "a readback is tight rows of the rect asked for, padding stripped",
    );
    for row in 0..corner.height {
        let from = (((row + corner.y) * whole.width + corner.x) * 4) as usize;
        let took = (row * corner.width * 4) as usize;
        assert!(
            part[took..took + (corner.width * 4) as usize] == all[from..from + (corner.width * 4) as usize],
            "row {row} of the offset readback is not row {} of the whole picture: the copy's \
             origin is not being honoured",
            row + corner.y,
        );
    }
}

/// **A docked panel's offset survives the blit and the readback together, not
/// just the readback's own arithmetic.**
///
/// The test above proves [`dump::read_rect`] honours an origin against one
/// texture read twice. It never calls [`Blit::render`] with a non-zero `rect.x`
/// or `rect.y` at all, so it cannot say whether the blit *places* the world at
/// the surface's own corner — `Shell::viewport()`'s documented contract,
/// `docs/pixels.md`'s backlog item about `ViewportRect`. This closes that gap
/// end to end: the same drawn frame, blit once into a target exactly its own
/// size at `(0, 0)`, and once into a bigger "window" texture at the corner a
/// docked panel would leave it — then read back and compared byte for byte.
/// Nothing here re-sizes the [`Camera`]; only the surface around the rect
/// grows, which is the property `Camera::image_size` not carrying an origin
/// (`docs/pixels.md`) relies on.
#[test]
fn a_docked_panels_offset_places_the_same_picture_it_shows_at_the_corner() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let drawn = draw_britain(&device, &queue, &dir, AT, frame::Draw::EVERYTHING, Zoom::ONE);
    let format = blit::WORLD_FORMAT;
    let world_view = drawn.world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer_views = drawn.gbuffer.views();
    let mut blit = Blit::new(&device, format);
    let dummy_mobiles = blit::dummy_instances(&device);

    let frame = |rect: ViewportRect| blit::Frame {
        target: &world_view, // overwritten per call below
        world: &world_view,
        gbuffer: &gbuffer_views,
        face_instances: drawn.statics.instances_buffer(),
        item_instances: drawn.statics.instances_buffer(),
        mobile_instances: &dummy_mobiles,
        mesh_instances: drawn.mesh.rows_buffer(),
        ground_instances: drawn.ground.instances_buffer(),
        zoom: openshard_client_render::camera::Zoom::ONE,
        rect,
    };

    let direct = dump_target(&device, format);
    let direct_view = direct.create_view(&wgpu::TextureViewDescriptor::default());
    let whole = ViewportRect {
        x: 0,
        y: 0,
        width: VIEWPORT.0,
        height: VIEWPORT.1,
    };
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    blit.render(
        &device,
        &queue,
        &mut encoder,
        blit::Frame {
            target: &direct_view,
            ..frame(whole)
        },
        &drawn.lighting,
    );
    queue.submit([encoder.finish()]);
    let direct_pixels = dump::read_rect(&device, &queue, &direct, whole);

    // A window forty-some pixels bigger on each side than the world it shows —
    // exactly what `Shell::viewport()` leaves once a docked panel has taken a
    // slice off the top and the left.
    let window = dump_target_sized(&device, format, VIEWPORT.0 + 50, VIEWPORT.1 + 50);
    let window_view = window.create_view(&wgpu::TextureViewDescriptor::default());
    let corner = ViewportRect {
        x: 37,
        y: 11,
        width: VIEWPORT.0,
        height: VIEWPORT.1,
    };
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    blit.render(
        &device,
        &queue,
        &mut encoder,
        blit::Frame {
            target: &window_view,
            ..frame(corner)
        },
        &drawn.lighting,
    );
    queue.submit([encoder.finish()]);
    let corner_pixels = dump::read_rect(&device, &queue, &window, corner);

    assert_eq!(
        direct_pixels, corner_pixels,
        "the same frame blit at the window's corner is not the same picture blit at (0, 0): \
         the docked-panel offset is not surviving the blit and the readback together",
    );
}

/// **Each normal layer holds its own category and nothing else.**
///
/// The two layers were added to answer whether a fringe along a silhouette
/// belongs to a sprite drawn over geometry, and that question only has an answer
/// if a picture of one layer really is only that layer. The first report against
/// them was a person looking at a client dump and finding a speaker's letters
/// standing in the *geometry* picture — `KIND_NOTHING` pixels, which every other
/// diagnostic passes through on purpose so the world's silhouette stays findable.
/// A layer that promises to be one category cannot afford that landmark.
///
/// So this reads the kind plane beside the two, and holds them to the partition
/// they claim: nothing is in neither, land is measured, a mobile is a picture,
/// and a static is in exactly one of the two — never both, never neither.
/// Black is the mark of "not in this layer" and it is unambiguous: a normal
/// reaches it only at `(-1, -1, -1)`, which is not a unit vector.
#[test]
fn each_normal_layer_holds_its_own_category_and_nothing_else() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let drawn = draw_britain(&device, &queue, &dir, AT, frame::Draw::EVERYTHING, Zoom::ONE);
    let format = blit::WORLD_FORMAT;
    let into = dump_target(&device, format);
    let into_view = into.create_view(&wgpu::TextureViewDescriptor::default());
    let world_view = drawn.world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer_views = drawn.gbuffer.views();
    let mut blit = Blit::new(&device, format);
    let dummy_mobiles = blit::dummy_instances(&device);
    let rect = ViewportRect {
        x: 0,
        y: 0,
        width: VIEWPORT.0,
        height: VIEWPORT.1,
    };

    // One blit per view into the same texture, read back raw — the picture is
    // not what is being asked about here, the bytes are. The world image is an
    // argument because the control below draws over a copy of it.
    let mut plane_of = |view: View, world: &wgpu::TextureView| {
        let mut lighting = drawn.lighting.clone();
        lighting.view = view;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("layer partition"),
        });
        blit.render(
            &device,
            &queue,
            &mut encoder,
            blit::Frame {
                target: &into_view,
                world,
                gbuffer: &gbuffer_views,
                face_instances: drawn.statics.instances_buffer(),
                item_instances: drawn.statics.instances_buffer(),
                mobile_instances: &dummy_mobiles,
                mesh_instances: drawn.mesh.rows_buffer(),
                ground_instances: drawn.ground.instances_buffer(),
                zoom: openshard_client_render::camera::Zoom::ONE,
                rect,
            },
            &lighting,
        );
        queue.submit([encoder.finish()]);
        dump::read_rect(&device, &queue, &into, rect)
    };
    let kinds = plane_of(View::Kind, &world_view);

    // **The positive control, and the defect that was actually reported.** A
    // pixel nothing drew keeps whatever the world image holds there, which is how
    // a speaker's letters ended up standing in the geometry layer — the text pass
    // writes the image and leaves the id plane at `Kind::Nothing`. This frame has
    // no text in it, so without a control the whole `NOTHING` branch below is
    // vacuous: the background is black either way and the assertion passes over a
    // shader that has stopped painting it. Measured, not assumed — the rule was
    // removed from `blit.wesl` and every assertion here stayed green.
    //
    // So one background pixel is painted white in a *copy* of the world image,
    // which is the text pass's own shape with none of its machinery: same image,
    // same G-buffer, one pixel that the world did not draw and cannot be black.
    let background = (0..(rect.width * rect.height) as usize)
        .find(|pixel| kinds[pixel * 4..pixel * 4 + 3] == [0, 0, 0])
        .expect("a frame of a street has background around its edges");
    let (width, height) = (drawn.world.width(), drawn.world.height());
    let painted = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("the world image with a message written over it"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("copy the world image"),
    });
    encoder.copy_texture_to_texture(
        drawn.world.as_image_copy(),
        painted.as_image_copy(),
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &painted,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: background as u32 % rect.width,
                y: background as u32 / rect.width,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        &[255, 255, 255, 255],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let painted_view = painted.create_view(&wgpu::TextureViewDescriptor::default());

    let geometry = plane_of(View::NormalGeometry, &painted_view);
    let sprites = plane_of(View::NormalSprites, &painted_view);

    // `debug_color`'s own four colours for `VIEW_KIND`, written as the floats the
    // shader states rather than as bytes: what an `Rgba8Unorm` attachment does to
    // a value that lands exactly halfway is the driver's business — `0.30 * 255`
    // is `76.5` and this one answers `76` — so the comparison allows the last bit
    // and nothing more. A drifted colour is several bits away, not one.
    const NOTHING: [f32; 3] = [0.0, 0.0, 0.0];
    const LAND: [f32; 3] = [0.20, 0.65, 0.30];
    const STATIC: [f32; 3] = [0.25, 0.45, 1.00];
    const MOBILE: [f32; 3] = [1.00, 0.40, 0.15];
    let is = |pixel: &[u8], want: [f32; 3]| {
        (0..3).all(|channel| (f32::from(pixel[channel]) - want[channel] * 255.0).abs() <= 1.0)
    };
    // Which of the four a kind pixel is, as an index into `counted` below. An
    // unknown colour is a failure and not a fifth bucket: it would mean the kind
    // view draws something this test does not know the rule for, and every
    // assertion below would then be silently skipping those pixels.
    let named = |pixel: &[u8]| match pixel {
        _ if is(pixel, NOTHING) => Some(0),
        _ if is(pixel, LAND) => Some(1),
        _ if is(pixel, STATIC) => Some(2),
        _ if is(pixel, MOBILE) => Some(3),
        _ => None,
    };

    let mut counted = [0u32; 4];
    for pixel in 0..(rect.width * rect.height) as usize {
        let at = pixel * 4;
        let in_geometry = !is(&geometry[at..at + 3], NOTHING);
        let in_sprites = !is(&sprites[at..at + 3], NOTHING);
        let (x, y) = (pixel as u32 % rect.width, pixel as u32 / rect.width);
        match named(&kinds[at..at + 3]) {
            Some(0) => {
                counted[0] += 1;
                assert!(
                    !in_geometry && !in_sprites,
                    "({x}, {y}) is nothing drawn and stands in a layer: \
                     geometry {in_geometry}, sprites {in_sprites}",
                );
            }
            Some(1) => {
                counted[1] += 1;
                assert!(
                    in_geometry && !in_sprites,
                    "({x}, {y}) is land, whose normal is its own patch's, and it is not \
                     in the geometry layer alone: geometry {in_geometry}, sprites {in_sprites}",
                );
            }
            Some(2) => {
                counted[2] += 1;
                assert!(
                    in_geometry != in_sprites,
                    "({x}, {y}) is a static and stands in {} layers, not one",
                    u32::from(in_geometry) + u32::from(in_sprites),
                );
            }
            Some(3) => {
                counted[3] += 1;
                assert!(
                    !in_geometry && in_sprites,
                    "({x}, {y}) is a mobile, which is a billboard and never a measured \
                     surface: geometry {in_geometry}, sprites {in_sprites}",
                );
            }
            _ => panic!(
                "({x}, {y}) is a kind colour nothing draws: {:?}",
                &kinds[at..at + 3],
            ),
        }
    }

    // **And the frame was worth asking.** A viewport of nothing but background
    // passes every assertion above, and a street that drew no static would leave
    // the one case with two possible answers untested.
    assert!(
        counted[0] > 0,
        "no background pixel, so the painted control was never looked at",
    );
    assert!(counted[1] > 0, "no land pixel in a frame drawn on a street");
    assert!(counted[2] > 0, "no static pixel at Britain's house corner");
}

/// The five shades the two silhouette layers are allowed to be, as the floats
/// `blit.wesl` states them — `debug_color`'s own branch, and the vocabulary this
/// pair of views promises to draw and nothing else.
const SILHOUETTE_SHADES: [(&str, [f32; 3]); 5] = [
    ("nothing", [0.0, 0.0, 0.0]),
    ("inside", [0.05, 0.05, 0.06]),
    ("the other layer", [0.16, 0.16, 0.20]),
    ("this layer, art", [1.00, 0.35, 0.10]),
    ("this layer, box", [0.25, 0.55, 1.00]),
];

/// White, which is *both* layers and is the same colour in both views.
const BOTH: [f32; 3] = [1.0, 1.0, 1.0];

/// Which of [`SILHOUETTE_SHADES`] (or [`BOTH`]) a pixel is, by name.
///
/// A colour no branch spells is a failure and never a sixth bucket: it would
/// mean the view draws something this test does not know the rule for, and every
/// count below would then be quietly skipping those pixels.
fn shade_of(pixel: &[u8]) -> &'static str {
    let is = |want: [f32; 3]| (0..3).all(|c| (f32::from(pixel[c]) - want[c] * 255.0).abs() <= 1.0);
    if is(BOTH) {
        return "both";
    }
    for (name, shade) in SILHOUETTE_SHADES {
        if is(shade) {
            return name;
        }
    }
    panic!("a silhouette layer drew a colour no branch of it spells: {pixel:?}");
}

/// **The two edges a frame draws, and they are two different lines.**
///
/// `docs/silhouettes.md` phase Z1's own "done when". The plan set out to
/// attribute a silhouette between two bounds and the pair of views answers
/// something sharper: since a box miss stopped being discarded, the picture's
/// outline is the art's alone and the box's line is a seam *inside* it. This
/// holds the pair to the partition it claims.
///
/// Five things, and none of them is satisfied by an empty frame:
///
/// - every pixel of either view is one of the six colours the branch spells;
/// - the two views agree, pixel for pixel, about which layer a fragment is in —
///   they are one record read twice, so a disagreement is the bits not surviving
///   the trip through the id plane;
/// - a **mobile** is never in the box layer. It is a billboard by construction,
///   with no volume for a ray to run out of;
/// - a fragment in the box layer is always **measured geometry** — it met a box,
///   so [`View::NormalGeometry`] holds it. This is the control that a rule made
///   to answer wrongly fails: a `box_edge` that had quietly been reading the
///   art's alpha would light up over the unmeasured remainder of a sprite,
///   which is precisely the half [`View::NormalSprites`] holds;
/// - land is in neither, and background is black.
///
/// The counts are printed rather than asserted against a number. They are the
/// measurement Z2 asks for, and a literal here would be a snapshot of one
/// afternoon's map data pretending to be an invariant.
///
/// **At `1:1`, which is where the two rules can be told apart from each other
/// but not the two widths.** The two edges are two *rules* at every
/// magnification and two different *widths* only above it, so the width half of
/// Z1 wants a picture at `4x` — over [`ON_THE_WALLS`] and not over [`AT`], for
/// the reason written on that constant. A magnified frame here collects no
/// static, and that was read as a defect in the assembly until it was measured:
/// it is the cull answering correctly about a place whose statics all stand four
/// hundred pixels from the eye.
#[test]
fn the_two_silhouette_layers_are_two_lines_and_a_frame_agrees_about_both() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let drawn = draw_britain(&device, &queue, &dir, AT, frame::Draw::EVERYTHING, Zoom::ONE);
    let format = blit::WORLD_FORMAT;
    let into = dump_target(&device, format);
    let into_view = into.create_view(&wgpu::TextureViewDescriptor::default());
    let world_view = drawn.world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer_views = drawn.gbuffer.views();
    let mut blit = Blit::new(&device, format);
    let dummy_mobiles = blit::dummy_instances(&device);
    let rect = ViewportRect {
        x: 0,
        y: 0,
        width: VIEWPORT.0,
        height: VIEWPORT.1,
    };

    let mut plane_of = |view: View| {
        let mut lighting = drawn.lighting.clone();
        lighting.view = view;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("silhouette layer"),
        });
        blit.render(
            &device,
            &queue,
            &mut encoder,
            blit::Frame {
                target: &into_view,
                world: &world_view,
                gbuffer: &gbuffer_views,
                face_instances: drawn.statics.instances_buffer(),
                item_instances: drawn.statics.instances_buffer(),
                mobile_instances: &dummy_mobiles,
                mesh_instances: drawn.mesh.rows_buffer(),
                ground_instances: drawn.ground.instances_buffer(),
                zoom: Zoom::ONE,
                rect,
            },
            &lighting,
        );
        queue.submit([encoder.finish()]);
        dump::read_rect(&device, &queue, &into, rect)
    };
    let art = plane_of(View::SilhouetteArt);
    let boxes = plane_of(View::SilhouetteBox);
    let kinds = plane_of(View::Kind);
    let geometry = plane_of(View::NormalGeometry);

    // The kind view's own four colours — `debug_color`'s `VIEW_KIND` branch,
    // stated the same way `each_normal_layer_holds_its_own_category_and_nothing_else`
    // states them.
    const NOTHING: [f32; 3] = [0.0, 0.0, 0.0];
    const LAND: [f32; 3] = [0.20, 0.65, 0.30];
    const STATIC: [f32; 3] = [0.25, 0.45, 1.00];
    const MOBILE: [f32; 3] = [1.00, 0.40, 0.15];
    let is = |pixel: &[u8], want: [f32; 3]| {
        (0..3).all(|channel| (f32::from(pixel[channel]) - want[channel] * 255.0).abs() <= 1.0)
    };

    // Counted per layer rather than per pixel: how many fragments the art's own
    // texel ended, how many the boxes ran out under, and how many are both.
    let (mut only_art, mut only_box, mut both, mut statics_seen, mut mobiles_seen) =
        (0u32, 0u32, 0u32, 0u32, 0u32);
    for pixel in 0..(rect.width * rect.height) as usize {
        let at = pixel * 4;
        let (x, y) = (pixel as u32 % rect.width, pixel as u32 / rect.width);
        let in_art = shade_of(&art[at..at + 3]);
        let in_box = shade_of(&boxes[at..at + 3]);

        // **One record read twice.** Each view draws its own layer bright and the
        // other dim, so the two pictures are a transposition of each other — and
        // a pair that disagreed would mean the bits are not surviving the id
        // plane, which no single picture could show.
        let agreed = matches!(
            (in_art, in_box),
            ("both", "both")
                | ("inside", "inside")
                | ("nothing", "nothing")
                | ("this layer, art", "the other layer")
                | ("the other layer", "this layer, box")
        );
        assert!(
            agreed,
            "({x}, {y}) is '{in_art}' in the art layer and '{in_box}' in the box layer: \
             the two views are not reading one record",
        );

        match in_art {
            "this layer, art" => only_art += 1,
            "the other layer" => only_box += 1,
            "both" => both += 1,
            _ => {}
        }

        // **The rule made to answer wrongly, and the plane that shows it.** A
        // fragment in the box layer met a box, so its normal is a measured face
        // and `NormalGeometry` holds it — black there is the mark of "not in this
        // layer", and a normal cannot reach black (`(-1, -1, -1)` is not a unit
        // vector). A `box_edge` reading the art's alpha instead would mark the
        // unmeasured remainder of a sprite too, and every one of those pixels is
        // black here.
        if in_box == "this layer, box" || in_box == "both" {
            assert!(
                geometry[at..at + 3] != [0, 0, 0],
                "({x}, {y}) is in the box layer and carries no measured normal: the box edge is \
                 being marked somewhere no box was met",
            );
        }

        if is(&kinds[at..at + 3], NOTHING) {
            assert_eq!(
                in_art, "nothing",
                "({x}, {y}) is background and stands in a layer"
            );
        } else if is(&kinds[at..at + 3], LAND) {
            assert_eq!(
                in_art, "inside",
                "({x}, {y}) is land, which has no silhouette of its own, and it is on an edge",
            );
        } else if is(&kinds[at..at + 3], STATIC) {
            statics_seen += 1;
        } else if is(&kinds[at..at + 3], MOBILE) {
            mobiles_seen += 1;
            // **The positive control.** A billboard has no volume for a ray to
            // run out of, so a mobile can carry the art bit and never the box
            // one. A box bit here would mean the mark is coming from something
            // other than the boxes this instance stands as.
            assert!(
                in_box != "this layer, box" && in_box != "both",
                "({x}, {y}) is a mobile and stands in the box layer, which has no box to run out",
            );
        }
    }

    println!(
        "silhouettes at 1:1, Britain {AT:?}: art only {only_art}, box only {only_box}, \
         both {both}, of {statics_seen} static pixels and {mobiles_seen} mobile ones",
    );
    // **And the frame was worth asking.** Every assertion above passes over a
    // viewport of background, and the two counts are the plan's own measurement:
    // a zero in either is a layer that was never looked at.
    assert!(statics_seen > 0, "no static pixel at Britain's house corner");
    assert!(
        only_art > 0,
        "no fragment in the art layer, so its half of the pair said nothing"
    );
    assert!(
        only_box > 0,
        "no fragment in the box layer, so its half of the pair said nothing"
    );
}

/// **Magnifying does not lose the statics — it loses the ones that were never
/// near the eye.**
///
/// The measurement behind [`ON_THE_WALLS`], and the reason `docs/silhouettes.md`
/// carried a blocker it did not have: a `4x` frame over [`AT`] collects land and
/// not one static, which reads exactly like a cull that has gone wrong at
/// magnification. It is not. The nine statics that place draws stand in the
/// top-left corner of its `900x700` image; `4x` shrinks the drawn image to
/// `225x175` **world** pixels around the same eye, and a corner four hundred
/// pixels out is outside it. The cull is right and the scene is the wrong scene.
///
/// Two claims, and the pair is what makes this a control rather than an
/// anecdote — either alone is satisfied by a cull that keeps everything or by
/// one that keeps nothing:
///
/// - over a place whose statics stand **at** the eye, every rung of the ladder
///   collects some, `4x` included;
/// - over [`AT`], `4x` collects none while `1:1` collects some — the asymmetry
///   this test exists to attribute to the place.
///
/// Counts are printed and not asserted against literals, for
/// [`the_two_silhouette_layers_are_two_lines_and_a_frame_agrees_about_both`]'s
/// own reason: they are one afternoon's map data.
#[test]
fn the_magnified_frame_over_a_wall_run_still_collects_its_statics() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let collected = |at, zoom| {
        let drawn = draw_britain(&device, &queue, &dir, at, frame::Draw::EVERYTHING, zoom);
        (drawn.statics_collected, drawn.land_collected)
    };

    for zoom in [
        Zoom::ONE,
        Zoom::ONE.scale_up(),
        Zoom::ONE.scale_up().scale_up().scale_up(),
    ] {
        let (statics, land) = collected(ON_THE_WALLS, zoom);
        println!("{zoom} over {ON_THE_WALLS:?}: {statics} statics, {land} land");
        assert!(
            statics > 0,
            "{zoom} over a wall run collected no static: the cull loses geometry at magnification",
        );
        // The land is the positive control on the frame itself: a zoom that
        // assembled nothing at all would satisfy nothing here, and would look
        // the same from the statics' side.
        assert!(
            land > 0,
            "{zoom} assembled no land either, so this frame is empty"
        );
    }

    // And the other half — the place, not the zoom.
    let (near, _) = collected(AT, Zoom::ONE);
    let (magnified, land) = collected(AT, Zoom::ONE.scale_up().scale_up().scale_up());
    println!("4x over {AT:?}: {magnified} statics, {land} land, against {near} at 1:1");
    assert!(near > 0, "even 1:1 draws no static at Britain's house corner");
    assert!(
        land > 0,
        "the magnified frame at Britain's house corner is empty of land too, \
         so its lack of statics says nothing about the place",
    );
    // `<` and not `== 0`: what is being attributed is that magnifying *here*
    // loses statics while magnifying over a wall run does not, and a literal
    // zero would be this afternoon's map data written down as a law.
    assert!(
        magnified < near,
        "{AT:?} kept all {near} of its statics at 4x, so the pair above is not an asymmetry \
         and this test no longer says what it is for",
    );
}

/// **Ticking a producer off narrows the drawing and leaves the lighting whole.**
///
/// `frame::Draw` exists because a G-buffer holds one answer per pixel: the way to
/// look at a wall something is standing in front of is to draw a frame that thing
/// is not in. What makes that a *diagnostic* rather than a second world is the
/// half this pins — the grid, the flames and the ambient are collected from
/// everything whatever is ticked, so the wall in the narrowed frame is lit
/// exactly as the wall in the full one.
///
/// The opposite implementation is the one a person would reach for first and it
/// is a trap: reaching the same picture by handing `assemble` fewer statics takes
/// them out of the occlusion grid too, and a room with its walls "not drawn"
/// would quietly light up. Both halves are asserted, because the picture alone
/// cannot tell the two apart — the statics are missing either way.
#[test]
fn ticking_a_producer_off_narrows_the_drawing_and_not_the_light() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let whole = draw_britain(&device, &queue, &dir, AT, frame::Draw::EVERYTHING, Zoom::ONE);
    let land_only = draw_britain(
        &device,
        &queue,
        &dir,
        AT,
        frame::Draw {
            statics: false,
            ..frame::Draw::EVERYTHING
        },
        Zoom::ONE,
    );

    assert!(
        whole.statics_collected > 0,
        "no static at Britain's house corner, so nothing was narrowed",
    );
    assert_eq!(
        land_only.statics_collected, 0,
        "the statics were ticked off and the frame collected them anyway",
    );
    assert_eq!(
        land_only.land_collected, whole.land_collected,
        "ticking the statics off moved the land",
    );

    // **And the light is the whole world's.** Every box of the grid and every
    // flame, in a frame that drew none of the statics they belong to.
    assert_eq!(
        land_only.lighting.occlusion.boxes().count(),
        whole.lighting.occlusion.boxes().count(),
        "the statics left the occlusion grid with the drawing",
    );
    assert_eq!(
        land_only.lighting.lights.len(),
        whole.lighting.lights.len(),
        "the statics took their flames with them",
    );
    assert!(
        whole.lighting.occlusion.boxes().count() > 0,
        "an empty grid agrees with an empty grid, so the two counts above said nothing",
    );
}
