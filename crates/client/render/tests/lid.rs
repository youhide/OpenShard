//! **S5 — the frame gate.** `docs/footprints.md`: the bookcase pair
//! `docs/parity.md`'s "shard's own furniture" names — `0x0A97` at
//! `(1505, 1656)` and `0x0A98` at `(1506, 1656)`, both `z = 27` — asserting the
//! lid shrinks to the measured slab, with the whole tile as the injection that
//! must go red.
//!
//! Two tests, because the claim has two halves that need two different
//! witnesses:
//!
//! - [`a_bookcases_measured_slab_is_narrower_than_the_whole_tile_and_a_lost_footprint_stands_it_back_up`]
//!   is the mutation. [`occlusion::boxes_of`] is the one function
//!   `frame::assemble` reads a lid's box from, so it is asked directly, once
//!   with the [`Shape`] the real art measures and once with `footprint: None` —
//!   the fallback [`occlusion::shape_of`]'s own doc names as "every occluder …
//!   before [`crate::facing`] existed". A gate that only checked the first half
//!   would pass an `occlusion::boxes_of` that ignored `footprint` entirely.
//! - [`a_real_frames_view_normal_draws_the_bookcases_lid_inside_that_slab`] is
//!   the picture: a real frame, `View::NormalGeometry` read back, and the count
//!   of pixels it drew over the bookcases' own tiles held under the whole
//!   tile's own fill — the claim the mutation above proves is *possible*, shown
//!   to be what a drawn frame actually does.
//!
//! No mutation of the second kind: `StaticAtlas`'s `footprints` table has no
//! public way to be cleared once a real picture has measured one (only
//! [`StaticAtlas::state_hole`]/`state_prism` exist, for a scene that never had
//! art to measure), so the whole-tile picture the first test's `footprint: None`
//! stands for is never drawn here. The geometry test is what is witnessed by
//! mutation; the picture test is its positive control.
//!
//! Gated on `OPENSHARD_CLIENT`, like every test here that needs the client's
//! own files, and a no-op without it.

use std::path::PathBuf;

use openshard_client_render::atlas::{LandAtlas, StaticAtlas, TexmapAtlas};
use openshard_client_render::blit::{self, Blit, ViewportRect};
use openshard_client_render::camera::Camera;
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::debug::View;
use openshard_client_render::frame::{self, Impostor};
use openshard_client_render::geometry::Vec2;
use openshard_client_render::hue::HueRamp;
use openshard_client_render::items::{self, GroundItem};
use openshard_client_render::light::{self, Tuning};
use openshard_client_render::occlusion::{self, Shape};
use openshard_client_render::renderer::{self, GroundRenderer, MeshFaceRenderer, SpriteRenderer, Target};
use openshard_client_render::statics::StaticGeometry;
use openshard_client_render::{dump, ground};
use openshard_protocol::direction::Direction;
use openshard_protocol::items::ItemAmount;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Point;
use openshard_uofiles::animdata::AnimData;
use openshard_uofiles::art::Art;
use openshard_uofiles::hues::Hues;
use openshard_uofiles::map::{LandCell, Map};
use openshard_uofiles::texmaps::TexMaps;
use openshard_uofiles::tiledata::TileData;

/// The pair `docs/parity.md`'s "shard's own furniture" section names by their
/// real placement — `0x0A97`/`0x0A98` at `(1505, 1656)`/`(1506, 1656)`, both
/// `z = 27`.
const BOOKCASES: [(u16, u16, u16); 2] = [(0x0A97, 1505, 1656), (0x0A98, 1506, 1656)];

const VIEWPORT: (u32, u32) = (300, 300);

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

/// **The mutation.** `occlusion::boxes_of` is offered the same static twice,
/// once with the [`Shape`] its own art measures and once with the footprint
/// taken back out — [`occlusion::shape_of`]'s own "every occluder … before
/// `facing` existed" fallback, reproduced by hand because nothing exposes a
/// way to un-measure a real picture's atlas entry.
#[test]
#[ignore]
fn a_bookcases_measured_slab_is_narrower_than_the_whole_tile_and_a_lost_footprint_stands_it_back_up() {
    let Some(dir) = client() else { return };
    let art = Art::open(&dir).expect("the client's art");
    let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");

    for (id, x, y) in BOOKCASES {
        let image = art
            .static_art(Graphic(id))
            .expect("read")
            .unwrap_or_else(|| panic!("{id:#06X}: no art in this install"));
        let shape = Shape::of(&image);
        assert!(
            shape.footprint.is_some(),
            "{id:#06X} is not the case this gate is about — `Shape::of` gave it no footprint, so \
             `docs/footprints.md`'s S5 fixture is naming the wrong graphic",
        );
        let tile = tiledata.static_tile(id);

        let mut slab = Vec::new();
        occlusion::boxes_of(x.into(), y.into(), 27, tile, &shape, |_part, _edges, solid| {
            slab.push(solid);
        });
        assert_eq!(
            slab.len(),
            1,
            "{id:#06X}: a footprint case is one lid, not {}",
            slab.len(),
        );
        let slab = slab[0];

        let whole_tile_shape = Shape {
            footprint: None,
            ..shape
        };
        let mut whole = Vec::new();
        occlusion::boxes_of(
            x.into(),
            y.into(),
            27,
            tile,
            &whole_tile_shape,
            |_part, _edges, solid| whole.push(solid),
        );
        let whole = whole[0];

        let slab_area = (slab.max.x - slab.min.x) * (slab.max.y - slab.min.y);
        let whole_area = (whole.max.x - whole.min.x) * (whole.max.y - whole.min.y);
        assert!(
            (whole_area - 1.0).abs() < 1e-9,
            "{id:#06X}: a lost footprint is not the whole tile ({whole_area})",
        );
        assert!(
            slab_area < whole_area - 0.01,
            "{id:#06X}: the measured slab ({slab_area:.3}) is not narrower than the whole tile \
             ({whole_area:.3}) — losing the footprint should have widened the lid back out, and it \
             did not, so this gate could never turn red",
        );
        eprintln!("{id:#06X}: slab {slab_area:.3} of the tile, whole {whole_area:.3}");
    }
}

/// Everything one drawn frame leaves behind — `tests/dump.rs`'s own `Drawn`,
/// trimmed to the one view this file reads.
struct Drawn {
    world: wgpu::Texture,
    gbuffer: openshard_client_render::gbuffer::Gbuffer,
    lighting: light::Lighting,
    ground: GroundRenderer,
    statics: SpriteRenderer,
    mesh: MeshFaceRenderer,
}

/// One frame, drawn from a synthetic tile of land and the two `items` given —
/// the tool's own route (`items::collect`), the way `tests/parity.rs`'s gate
/// draws the shard's furniture rather than the map's.
#[allow(clippy::too_many_arguments)]
fn draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    dir: &std::path::Path,
    art: &Art,
    tiledata: &TileData,
    animations: &openshard_client_render::animate::StaticAnimations,
    at: Point,
    items: &[GroundItem],
) -> Drawn {
    let map = Map::from_blocks(2, 2, |_, _| LandCell {
        tile: openshard_uofiles::map::LandTile(0),
        z: 27,
    });
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
    let static_atlas =
        StaticAtlas::build(art, items::needed_graphics(items, animations)).expect("both bookcases fit");

    let tuning = Tuning::DEFAULT;
    let mut fades = openshard_client_render::cutaway::Fades::default();
    let inputs = frame::Inputs {
        map: &map,
        items,
        camera: &camera,
        tiledata,
        animations,
        cutaway: &cutaway,
        land: &land,
        texmaps: &texmaps,
        statics: &static_atlas,
        sky: Some(light::NIGHT.flattened()),
        sun: None,
        carried: Some((at, Vec2::default(), Direction::South)),
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

/// **The picture.** A real frame over the bookcase pair, `View::NormalGeometry`
/// read back, and the drawn pixels over each bookcase's own tile held under
/// what a whole tile of the same box would fill — a diamond of `n` pixels'
/// bounding rectangle is filled to almost exactly half, by construction
/// ([`openshard_client_render::solid::Solid::faces`]'s own top face is that
/// diamond), so a slab narrower than the tile draws fewer pixels than that half
/// even though its own fill ratio is the same shape.
#[test]
#[ignore]
fn a_real_frames_view_normal_draws_the_bookcases_lid_inside_that_slab() {
    let (Some(dir), Some((device, queue))) = (client(), gpu()) else {
        return;
    };
    let art = Art::open(&dir).expect("the client's art");
    let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");
    let animdata = AnimData::load(&dir).expect("animdata.mul");
    let animations = openshard_client_render::animate::StaticAnimations::build(&animdata, &tiledata);

    let at = Point::new(BOOKCASES[0].1, BOOKCASES[0].2, 27);
    let items: Vec<GroundItem> = BOOKCASES
        .iter()
        .map(|&(id, x, y)| GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(x, y, 27),
            graphic: Graphic(id),
            hue: Hue::NONE,
        })
        .collect();

    let drawn = draw(&device, &queue, &dir, &art, &tiledata, &animations, at, &items);
    let camera = Camera::new(at, VIEWPORT.0, VIEWPORT.1);

    let format = blit::WORLD_FORMAT;
    let into = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("lid gate dump"),
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
    let mut blit = Blit::new(&device, format);
    let dummy_mobiles = blit::dummy_instances(&device);
    let rect = ViewportRect {
        x: 0,
        y: 0,
        width: VIEWPORT.0,
        height: VIEWPORT.1,
    };
    let mut lighting = drawn.lighting.clone();
    lighting.view = View::NormalGeometry;
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
            rect,
        },
        &lighting,
    );
    queue.submit([encoder.finish()]);
    let geometry = dump::read_rect(&device, &queue, &into, rect);

    let tile = tiledata.static_tile(BOOKCASES[0].0);
    let whole_tile_shape = Shape {
        footprint: None,
        ..Shape::UNREAD
    };
    let mut whole_box = Vec::new();
    occlusion::boxes_of(
        BOOKCASES[0].1.into(),
        BOOKCASES[0].2.into(),
        27,
        tile,
        &whole_tile_shape,
        |_part, _edges, solid| whole_box.push(solid),
    );

    let mut drawn_over_both_tiles = 0usize;
    let mut bounding_area = 0usize;
    for (id, x, y) in BOOKCASES {
        let tile = tiledata.static_tile(id);
        let mut whole = Vec::new();
        occlusion::boxes_of(
            x.into(),
            y.into(),
            27,
            tile,
            &whole_tile_shape,
            |_p, _e, solid| whole.push(solid),
        );
        let whole = whole[0];
        let corners = whole.faces(&camera)[0].1; // the top face: the tile's own diamond
        let (min_x, max_x) = corners
            .iter()
            .map(|p| p.x)
            .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        let (min_y, max_y) = corners
            .iter()
            .map(|p| p.y)
            .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        let (min_x, max_x) = (min_x.max(0.0) as u32, (max_x.min(VIEWPORT.0 as f32)) as u32);
        let (min_y, max_y) = (min_y.max(0.0) as u32, (max_y.min(VIEWPORT.1 as f32)) as u32);
        bounding_area += ((max_x.saturating_sub(min_x)) * (max_y.saturating_sub(min_y))) as usize;

        for py in min_y..max_y {
            for px in min_x..max_x {
                let at = ((py * VIEWPORT.0 + px) * 4) as usize;
                if geometry[at..at + 3] != [0, 0, 0] {
                    drawn_over_both_tiles += 1;
                }
            }
        }
    }

    assert!(
        bounding_area > 0,
        "the bookcases' own tiles project to no pixels — the camera at {at:?} is not over them",
    );
    // A whole tile's own diamond fills its bounding rectangle to very nearly
    // half — see `Solid::faces`'s own doc for why the top face is exactly the
    // tile diamond. A slab narrower on both axes draws fewer pixels than that
    // half over the *same* bounding rectangles, which is what is asserted:
    // not a fitted number, the geometric floor a whole-tile picture would sit
    // at if the footprint measured above were never read.
    let half_of_whole_tiles = bounding_area / 2;
    assert!(
        drawn_over_both_tiles < half_of_whole_tiles,
        "{drawn_over_both_tiles} of {bounding_area} pixels over the bookcases' own tiles are drawn \
         geometry — at or above half the whole tile's own fill, so the frame is not drawing a lid \
         narrower than the tile it stands on",
    );
    eprintln!(
        "{drawn_over_both_tiles} pixels drawn over {bounding_area} of whole-tile bounding rect \
         (half: {half_of_whole_tiles})",
    );
}
