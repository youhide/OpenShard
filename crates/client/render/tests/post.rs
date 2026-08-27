//! **A post's shadow, before and after its box was narrowed.**
//! `docs/footprints.md`'s backlog item "a post's shadow narrowed and that is
//! probably right" — forty-two placements in that plan's own class are pieces
//! the grid holds a primitive for, so each of them casts a shadow the plan has
//! already changed, and what was missing was anyone having *looked* at one.
//!
//! The subject is Britain's `0x0009` "wooden post" at `(1465, 1683, 0)`, which
//! `examples/tile_probe` says stands alone on cobblestones with nothing else on
//! its tile — so the picture is one post, one flame, and one shadow, with no
//! second occluder in it. Its own art measures `x (6,8) y (6,8)`: the far
//! quarter of the tile, which is what a post *is*.
//!
//! Two frames, differing in exactly one thing —
//! [`StaticAtlas::forget_footprints`], which stands every picture in the scene
//! back on the whole tile, the box that shipped before this plan's S3. So the
//! comparison is a mutation rather than two builds measured a session apart,
//! and a `boxes_of` that read `Shape::footprint` no longer would make the two
//! counts equal and this gate red.
//!
//! What is asserted is the *size* of the shadow and not its prettiness: the
//! narrowed box shadows well under half of what the whole tile shadowed. The
//! judgement that the narrow one is also the *right* one is in the plan, made
//! by looking at the pair of pictures this test draws — the drawn post is a
//! stick a few pixels wide and the whole tile's shadow was a wedge some four
//! times wider than it, cast by a volume nothing in the frame draws.
//!
//! Gated on `OPENSHARD_CLIENT` and on a GPU, like every test here that needs
//! either, and a no-op without them.

use std::path::PathBuf;

use openshard_client_render::atlas::{LandAtlas, StaticAtlas, TexmapAtlas};
use openshard_client_render::blit::{self, Blit, ViewportRect};
use openshard_client_render::camera::Camera;
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::debug::View;
use openshard_client_render::frame::{self, Impostor};
use openshard_client_render::hue::HueRamp;
use openshard_client_render::items::{self, GroundItem};
use openshard_client_render::light::{self, Tuning};
use openshard_client_render::renderer::{self, GroundRenderer, MeshFaceRenderer, SpriteRenderer, Target};
use openshard_client_render::statics::StaticGeometry;
use openshard_client_render::{dump, ground};
use openshard_map::grid::BlockExtent;
use openshard_map::map::{LandCell, WorldMap};
use openshard_protocol::items::ItemAmount;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Point;
use openshard_tiles::TileData;
use openshard_uofiles::animdata::AnimData;
use openshard_uofiles::art::Art;
use openshard_uofiles::hues::Hues;
use openshard_uofiles::texmaps::TexMaps;

/// The post's graphic, and a flame's.
///
/// The *placement* is synthetic — the middle of the small flat map below rather
/// than Britain's own `(1465, 1683)` — because nothing about this question is
/// about where the post stands: the shadow is thrown by the box the art
/// measured, and the art is the same art wherever it is put. What the real
/// coordinates buy is the subject, and they are in this file's own doc.
const POST: u16 = 0x0009;
const FLAME: u16 = 0x0B24;

/// A flat map big enough that the camera stands well inside it, so there is
/// ground everywhere for a shadow to land on.
const BLOCKS: u32 = 4;
/// Where the post stands, and the flame two tiles away on the `+x` side — far
/// enough that the shadow it throws is long, close enough that all of it is
/// inside the torch's own reach.
const AT: (u16, u16) = (16, 16);
const FLAME_AT: (u16, u16) = (18, 16);
/// Grass, so the ground is a real surface with a real texture under the shadow.
const LAND: u16 = 0x0003;

const VIEWPORT: (u32, u32) = (512, 512);

fn client() -> Option<PathBuf> {
    std::env::var_os("OPENSHARD_CLIENT").map(PathBuf::from)
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

/// Everything one drawn frame leaves behind — `tests/lid.rs`'s own `Drawn`.
struct Drawn {
    world: wgpu::Texture,
    gbuffer: openshard_client_render::gbuffer::Gbuffer,
    lighting: light::Lighting,
    ground: GroundRenderer,
    statics: SpriteRenderer,
    mesh: MeshFaceRenderer,
}

/// One frame over flat synthetic land, holding the post and the flame — and
/// built from an atlas this caller was handed first, which is the whole point:
/// the second frame's atlas has had its footprints taken back out.
#[allow(clippy::too_many_arguments)]
fn draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    dir: &std::path::Path,
    art: &Art,
    tiledata: &TileData,
    animations: &openshard_client_render::animate::StaticAnimations,
    static_atlas: &StaticAtlas,
    at: Point,
    items: &[GroundItem],
) -> Drawn {
    let map = WorldMap::from_blocks(
        BlockExtent {
            wide: BLOCKS,
            down: BLOCKS,
        },
        |_, _| LandCell {
            tile: openshard_tiles::LandTileId(LAND),
            z: 0,
        },
    );
    let camera = Camera::new(at, VIEWPORT.0, VIEWPORT.1);
    let cutaway = Cutaway::at(&map, tiledata, at, true);

    let land_wanted = ground::visible_graphics(&map, &camera);
    let land = LandAtlas::build(art, land_wanted.iter().copied()).expect("a screen of land fits");
    let texmaps = TexmapAtlas::build(
        &TexMaps::open(dir).expect("texidx.mul and texmaps.mul"),
        tiledata,
        land_wanted,
    )
    .expect("a screen of textures fits");

    let tuning = Tuning::DEFAULT;
    let mut fades = openshard_client_render::cutaway::Fades::default();
    let inputs = frame::Inputs {
        map: &map,
        items,
        drawn_items: items,
        camera: &camera,
        tiledata,
        animations,
        cutaway: &cutaway,
        interior: None,
        land: &land,
        texmaps: &texmaps,
        statics: openshard_client_render::atlas::StaticArt::Single(static_atlas),
        sky: Some(light::NIGHT.flattened()),
        sun: None,
        carried: None,
        tuning: &tuning,
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
        &HueRamp::build(&Hues::load(dir.join("hues.mul")).expect("hues.mul")),
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
    }
}

/// How many pixels of a `View::Shadow` readback are in shadow — **blocked**,
/// which is not the same as dark.
///
/// That view's own three answers (`blit.wesl`, `VIEW_SHADOW`) are what this has
/// to tell apart, and the middle one is the trap the shader's own comment names:
///
/// - **blue** `(0, 0, 89)` — no flame reaches here at all. Most of a 512-pixel
///   frame around one torch at night, and *not* a shadow: counting it made the
///   two frames below differ by 4% when their shadows differ by four times.
/// - **dark red** `(51, 0, 0)` — a flame reaches and is fully blocked.
/// - **grey** — partly blocked, the penumbra, `nearest_through` straight out.
///
/// So: something in the red channel (which rules out both the blue and the
/// cleared background a fragment with no surface keeps), dark, and not bluer
/// than it is red. Half-lit counts as shadow, which is deliberately loose —
/// this gate is about a fourfold difference, and where exactly a penumbra ends
/// is a different question with a different instrument.
fn shadowed(pixels: &[u8]) -> usize {
    pixels
        .chunks_exact(4)
        .filter(|px| px[0] > 0 && px[1] < 128 && px[0] >= px[2])
        .count()
}

/// Read one drawn frame's `View::Shadow` back.
fn shadow_plane(device: &wgpu::Device, queue: &wgpu::Queue, drawn: &Drawn) -> Vec<u8> {
    let format = blit::WORLD_FORMAT;
    let into = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("post shadow dump"),
        size: wgpu::Extent3d {
            width: VIEWPORT.0,
            height: VIEWPORT.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let into_view = into.create_view(&wgpu::TextureViewDescriptor::default());
    let world_view = drawn.world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer_views = drawn.gbuffer.views();
    let mut blit = Blit::new(device, format);
    let dummy_mobiles = blit::dummy_instances(device);
    let rect = ViewportRect {
        x: 0,
        y: 0,
        width: VIEWPORT.0,
        height: VIEWPORT.1,
    };
    let mut lighting = drawn.lighting.clone();
    lighting.view = View::Shadow;
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    blit.render(
        device,
        queue,
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
            rect,
        },
        &lighting,
    );
    queue.submit([encoder.finish()]);
    dump::read_rect(device, queue, &into, rect)
}

/// **The gate.** Two frames of one place, one flame, and one difference: the
/// second atlas has forgotten what the post's own base edge measured. The
/// shadow the post throws in the first is a fraction of the one it throws in
/// the second, and a `boxes_of` that stopped reading the footprint would make
/// them the same picture.
#[test]
#[ignore]
fn a_posts_shadow_is_a_quarter_tiles_and_a_lost_footprint_makes_it_a_whole_tiles() {
    let (Some(dir), Some((device, queue))) = (client(), gpu()) else {
        return;
    };
    let art = Art::open(&dir).expect("the client's art");
    let tiledata = openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");
    let animdata = AnimData::load(&dir).expect("animdata.mul");
    let animations = openshard_client_render::animate::StaticAnimations::build(&animdata, &tiledata);

    let at = Point::new(AT.0, AT.1, 0);
    let items: Vec<GroundItem> = [(POST, AT), (FLAME, FLAME_AT)]
        .iter()
        .map(|&(id, (x, y))| GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(x, y, 0),
            graphic: Graphic(id),
            hue: Hue::NONE,
        })
        .collect();

    let measured = StaticAtlas::build(&art, items::needed_graphics(&items, &animations))
        .expect("the post and the flame fit");
    assert!(
        measured.footprint(Graphic(POST)).is_some(),
        "{:#06X} is not the case this gate is about — its art measured no footprint, so \
         `docs/footprints.md`'s post item is naming the wrong graphic",
        POST,
    );
    let mut whole_tile = StaticAtlas::build(&art, items::needed_graphics(&items, &animations))
        .expect("the post and the flame fit");
    whole_tile.forget_footprints();

    let with = draw(
        &device,
        &queue,
        &dir,
        &art,
        &tiledata,
        &animations,
        &measured,
        at,
        &items,
    );
    let without = draw(
        &device,
        &queue,
        &dir,
        &art,
        &tiledata,
        &animations,
        &whole_tile,
        at,
        &items,
    );

    let narrow = shadowed(&shadow_plane(&device, &queue, &with));
    let wide = shadowed(&shadow_plane(&device, &queue, &without));

    assert!(
        wide > 0,
        "nothing in this frame is in shadow at all — the flame is not reaching the \
         post, so neither number below means anything",
    );
    assert!(
        narrow * 2 < wide,
        "the post shadows {narrow} pixels with its own measured box and {wide} with the whole \
         tile — not the fraction a quarter-tile post should throw, so the frame is not reading \
         the footprint its art measured",
    );
    eprintln!("post shadow: {narrow} px measured, {wide} px whole-tile");
}
