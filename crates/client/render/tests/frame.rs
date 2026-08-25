//! Frames, rendered on a real GPU and read back pixel by pixel.
//!
//! A renderer's usual problem is that it has no oracle: the output is looked at,
//! and "looks right" survives an off-by-one in the projection, a swapped colour
//! channel and a sprite sampled one texel over. Rendering to a texture instead
//! of a window removes that excuse — the frame is bytes, and bytes can be
//! compared with the art the frame was built from.
//!
//! Two things gate these tests, and both are honest skips rather than failures:
//! `OPENSHARD_CLIENT`, because no client files live in this repository, and the
//! presence of an adapter, because CI machines do not always have one.

use std::collections::BTreeSet;
use std::path::PathBuf;

use openshard_client_render::animate::StaticAnimations;
use openshard_client_render::atlas::{
    AnimAtlas, AnimationKey, FrameKey, LandAtlas, StaticAtlas, StaticAtlasPage, StaticAtlasPages, TexmapAtlas,
};
use openshard_client_render::blit::{Blit, ViewportRect};
use openshard_client_render::camera::ViewPoint;
use openshard_client_render::camera::{Camera, Projection, RealPixel, WorldPoint, Zoom};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::debug::View;
use openshard_client_render::geometry::{Rect, Vec2};
use openshard_client_render::ground::{self, GroundQuad};
use openshard_client_render::hue::HueRamp;
use openshard_client_render::impostor::{Fringe, Range, Volume};
use openshard_client_render::light::{Light, Lighting, Surface, WorldVec};

/// The reach the lighting tests give their flame, in tiles.
///
/// Chosen here rather than read from `light::TORCH`: these tests are about the
/// shader's falloff and its shadow walk, not about which flame a graphic gets.
/// Three and not a torch's six: at 44 pixels a tile, a 256-pixel frame holds
/// five tiles of one row, and a pool that reached six of them would have no
/// "outside" to compare against inside the picture.
const TORCH_TILES: f32 = 3.0;
use openshard_client_render::camera::TileBounds;
use openshard_client_render::composite::{
    CaptureSource, CompositeCache, CompositeKey, CompositeProducerJob, CompositeQuad, CompositeRenderer,
    CompositeTier, CompositeWorkQueue, ImmutableRevision, MapBlockBounds,
};
use openshard_client_render::gbuffer;
use openshard_client_render::mobiles::{self, Mobile};
use openshard_client_render::occlusion::{self, Builder, Occlusion, OwnerId, Shape, SolidId};
use openshard_client_render::outline::{self, Outline, Ring};
use openshard_client_render::place::{Kind, Place};
use openshard_client_render::renderer::{self, GroundRenderer, SpriteRenderer, Target};
use openshard_client_render::sprite::{SpriteQuad, split_corners};
use openshard_client_render::statics;
use openshard_map::grid::BlockCoord;
use openshard_protocol::direction::Direction;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_tiles::{StaticTile, TileFlags};
use openshard_uofiles::anim::{Anim, AnimFrame, AnimationDirection, AnimationFrameIndex, AnimationGroup};
use openshard_uofiles::art::{Art, LAND_TILE_SIZE, land_row};
use openshard_uofiles::color::{Color16, Rgb8};
use openshard_uofiles::equipconv::EquipConv;
use openshard_uofiles::hues::Hues;
use openshard_uofiles::image::Image;
use openshard_uofiles::texmaps::TexMaps;

/// The client's files, or `None` when the environment does not point at any.
fn client_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?))
}

/// The texture atlas for a set of land graphics, read from a real install.
///
/// Two files rather than one: the textures themselves, and the `tiledata` that
/// says which of them a land graphic uses.
fn texmap_atlas(dir: &std::path::Path, wanted: impl IntoIterator<Item = Graphic>) -> TexmapAtlas {
    let texmaps = TexMaps::open(dir).expect("texidx.mul and texmaps.mul");
    let tiledata = openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");
    TexmapAtlas::build(&texmaps, &tiledata, wanted).expect("a screen of textures fits")
}

/// A GPU to draw with, or `None` where there is none.
fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    // The real client requires this exact G-buffer target. Some CI adapters
    // expose a device but only through a downlevel path that cannot render to
    // `Rgba32Float`; treating that as a usable GPU lets a test panic halfway
    // through setup instead of honestly skipping what this machine cannot draw.
    if !adapter
        .get_texture_format_features(gbuffer::POSITION_FORMAT)
        .allowed_usages
        .contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
    {
        return None;
    }
    // The defaults, plus the one thing this crate asks for above them —
    // `gbuffer::required_limits`, whose own doc has the arithmetic. Asking for
    // exactly what the app asks for is the point: a test running under wider
    // limits than the client gets would be a test that cannot find the pipeline
    // the client will fail to create.
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: openshard_client_render::gbuffer::required_limits(),
        ..Default::default()
    }))
    .ok()
}

/// A rendered frame, as RGBA8 rows.
struct Frame {
    width: u32,
    pixels: Vec<u8>,
}

/// The finished picture of the small cutaway harness and the opaque G-buffer
/// it was composed over. The latter is read directly to prove translucent
/// architecture did not replace the main identity that picking and masks use.
struct CutawayFrame {
    picture: Frame,
    main_ids: Vec<u8>,
}

impl CutawayFrame {
    fn main_id(&self, x: u32, y: u32) -> u32 {
        let at = ((y * self.picture.width + x) * 4) as usize;
        u32::from_le_bytes(self.main_ids[at..at + 4].try_into().expect("one R32Uint texel"))
    }
}

impl Frame {
    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[at],
            self.pixels[at + 1],
            self.pixels[at + 2],
            self.pixels[at + 3],
        ]
    }

    /// How many pixels the ground pass wrote. Anything drawn is opaque and the
    /// clear is fully transparent, so this counts exactly, with no threshold.
    fn drawn(&self) -> usize {
        self.pixels.chunks_exact(4).filter(|p| p[3] == u8::MAX).count()
    }
}

/// Draw ground into a `width` x `height` texture and read the result back.
///
/// The common case, and the one every test written before statics existed used.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    quads: &[GroundQuad],
    width: u32,
    height: u32,
) -> Frame {
    let empty = StaticAtlas::pack([]).expect("nothing always fits");
    let no_mobiles = AnimAtlas::pack([]).expect("nothing always fits");
    render_both(
        device,
        queue,
        atlas,
        texmaps,
        quads,
        &empty,
        &[],
        (no_mobiles.pixels(), &[]),
        width,
        height,
        Projection::one_to_one(width, height),
    )
}

/// Draw ground through a camera's own projection, rather than 1:1.
///
/// The magnified path, which [`render`] cannot reach: it is the same ground pass
/// and the same quads, and what differs is the two numbers the vertex shader
/// ends on.
#[allow(clippy::too_many_arguments)]
fn render_projected(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    quads: &[GroundQuad],
    width: u32,
    height: u32,
    camera: Camera,
) -> Frame {
    let empty = StaticAtlas::pack([]).expect("nothing always fits");
    let no_mobiles = AnimAtlas::pack([]).expect("nothing always fits");
    render_both(
        device,
        queue,
        atlas,
        texmaps,
        quads,
        &empty,
        &[],
        (no_mobiles.pixels(), &[]),
        width,
        height,
        camera.projection(),
    )
}

/// Draw both passes into a `width` x `height` texture and read the result back.
///
/// `width * 4` must be a multiple of 256: that is the row alignment a buffer
/// copy demands, and padding it here would only hide the constraint from the
/// callers, which choose their own sizes.
#[allow(clippy::too_many_arguments)]
fn render_both(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    quads: &[GroundQuad],
    static_atlas: &StaticAtlas,
    static_quads: &[SpriteQuad],
    mobiles: (&[u8], &[SpriteQuad]),
    width: u32,
    height: u32,
    projection: Projection,
) -> Frame {
    render_both_with_cutaway(
        device,
        queue,
        atlas,
        texmaps,
        quads,
        static_atlas,
        static_quads,
        mobiles,
        &[],
        width,
        height,
        projection,
    )
    .picture
}

/// [`render_both`] with the independently deferred cutaway layer enabled.
#[allow(clippy::too_many_arguments)]
fn render_both_with_cutaway(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    quads: &[GroundQuad],
    static_atlas: &StaticAtlas,
    static_quads: &[SpriteQuad],
    mobiles: (&[u8], &[SpriteQuad]),
    cutaway_quads: &[SpriteQuad],
    width: u32,
    height: u32,
    projection: Projection,
) -> CutawayFrame {
    assert_eq!(width * 4 % 256, 0, "a row copy has to be 256-byte aligned");

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cutaway frame"),
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
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let world = openshard_client_render::blit::world_texture(device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let cutaway_world = openshard_client_render::blit::world_texture(device, width, height);
    let cutaway_world_view = cutaway_world.create_view(&wgpu::TextureViewDescriptor::default());

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(width) * u64::from(height) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let ids_readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cutaway main ids readback"),
        size: u64::from(width) * u64::from(height) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    // The depth buffer both passes share. Created here rather than inside the
    // renderer because a test that could not hand the two passes the same one
    // would not be testing the thing that makes them agree.
    let depth = renderer::depth_texture(device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(device, width, height);
    let gbuffer_views = gbuffer.views();
    let cutaway_gbuffer = openshard_client_render::gbuffer::Gbuffer::new(device, width, height);
    let cutaway_gbuffer_views = cutaway_gbuffer.views();

    // None of these frames ask for a hue — every quad built below carries
    // `hue: 0` — so an empty ramp is a real texture the shader can bind rather
    // than a special case: it is never indexed because nothing here sets the
    // bit that would make the shader look.
    let hue_ramp = HueRamp::build(&Hues::parse(&[0u8; 708]).expect("one empty group"));

    let mut renderer = GroundRenderer::new(device, queue, format, atlas, texmaps);
    let mut statics = SpriteRenderer::new(device, queue, format, static_atlas.pixels(), &hue_ramp);
    // The mobiles are the same pass again with another atlas bound, which is
    // the whole of the difference between a static and a creature on the GPU.
    let mut people = SpriteRenderer::new(device, queue, format, mobiles.0, &hue_ramp);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target_view = Target {
        gbuffer: &gbuffer_views,
        view: &world_view,
        depth: &depth_view,
        width,
        height,
        projection,
    };
    renderer.render(device, queue, &mut encoder, target_view, quads);
    // No boxes, on purpose: what this harness reads back is the *picture*, and
    // the impostor decides a fragment's position and normal rather than its
    // colour. A quad with no volume is the billboard reading — see
    // `statics.wesl` — and nothing here looks at what it wrote.
    // `render_places` below is the harness that does, and it takes them.
    statics.render(device, queue, &mut encoder, target_view, static_quads, &[], None);
    people.render(device, queue, &mut encoder, target_view, mobiles.1, &[], None);
    if !cutaway_quads.is_empty() {
        let cutaway_target = Target {
            gbuffer: &cutaway_gbuffer_views,
            view: &cutaway_world_view,
            depth: &depth_view,
            width,
            height,
            projection,
        };
        statics.render_cutaway(
            device,
            queue,
            &mut encoder,
            cutaway_target,
            cutaway_quads,
            &[],
            cutaway_quads.len() as u32,
        );
    }
    let mut blit = Blit::new(device, format);
    // The two dummy buffers must outlive the bindings recorded in this encoder.
    // They describe categories absent from this small harness, while statics and
    // ground use the renderers' real instance data above.
    let mesh_instances = openshard_client_render::blit::dummy_mesh_instances(device);
    let frame = |world, gbuffer, face_instances| openshard_client_render::blit::Frame {
        target: &view,
        world,
        gbuffer,
        face_instances,
        item_instances: face_instances,
        mobile_instances: people.instances_buffer(),
        mesh_instances: &mesh_instances,
        ground_instances: renderer.instances_buffer(),
        zoom: Zoom::ONE,
        rect: ViewportRect {
            x: 0,
            y: 0,
            width,
            height,
        },
    };
    blit.render(
        device,
        queue,
        &mut encoder,
        frame(&world_view, &gbuffer_views, statics.instances_buffer()),
        &Lighting::NONE,
    );
    if !cutaway_quads.is_empty() {
        blit.render_cutaway(
            device,
            queue,
            &mut encoder,
            frame(
                &cutaway_world_view,
                &cutaway_gbuffer_views,
                statics.cutaway_instances_buffer(),
            ),
            &Lighting::NONE,
            openshard_client_render::cutaway::TRANSLUCENT_ALPHA,
        );
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
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
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: gbuffer.ids(),
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &ids_readback,
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
    let ids_slice = ids_readback.slice(..);
    ids_slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("mapping the main ids this test just wrote");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting on our own submission");
    let pixels = slice
        .get_mapped_range()
        .expect("the map completed above")
        .to_vec();
    readback.unmap();
    let main_ids = ids_slice
        .get_mapped_range()
        .expect("the ids map completed above")
        .to_vec();
    ids_readback.unmap();

    CutawayFrame {
        picture: Frame { width, pixels },
        main_ids,
    }
}

/// One sprite, drawn alone, compared to the art texel for texel.
///
/// This is the test that ties the three layers together: the atlas packed the
/// sprite somewhere, the instance carried texture coordinates, and the shader
/// sampled them. Any of the three being off by a texel moves the diamond, and
/// nothing else in the suite would notice — a whole frame of ground still looks
/// like ground when every tile samples its neighbour.
#[test]
fn a_lone_sprite_matches_the_art_it_came_from() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let art = Art::open(&dir).expect("artLegacyMUL.uop");

    // The first land graphic the client actually ships. Which one it is does not
    // matter; that it is real art with a real shape does.
    let (graphic, image) = (0..0x4000u16)
        .map(Graphic)
        .find_map(|g| art.land(g).expect("reading land art").map(|image| (g, image)))
        .expect("a modern client ships thousands of land tiles");

    let atlas = LandAtlas::build(&art, [graphic]).expect("one graphic fits");
    let region = atlas.region(graphic).expect("just packed");

    // Level, and centred so its bounding square starts at the viewport's origin:
    // viewport coordinates are then the sprite's own. A tile whose four corners
    // share a height is drawn as the art's square, which is what makes this
    // comparison texel for texel possible at all — see `ground.wgsl`.
    let quads = [GroundQuad {
        x: f32::from(LAND_TILE_SIZE) / 2.0,
        y: f32::from(LAND_TILE_SIZE) / 2.0,
        corners: [0.0; 4],
        region,
        texmap: None,
        // Anything inside clip space: this frame holds one quad, so there is
        // nothing for the depth test to decide.
        depth: 0.5,
        place: Place::land(0, 0),
    }];
    let side = u32::from(LAND_TILE_SIZE);
    let empty = TexmapAtlas::pack([]).expect("nothing always fits");
    let frame = render(&device, &queue, &atlas, &empty, &quads, 64, 64);

    let mut compared = 0;
    for y in 0..side {
        let row = land_row(y as u16);
        for x in 0..side {
            let got = frame.pixel(x, y);
            if !row.contains(&(x as u16)) {
                assert_eq!(got[3], 0, "({x}, {y}) is outside the diamond but was drawn");
                continue;
            }
            // Inside the diamond every pixel is drawn, black ones included:
            // ground has no transparency, and a tile that loses its zero pixels
            // is a tile with pinholes in it.
            let Rgb8 {
                red: r,
                green: g,
                blue: b,
            } = image.pixel(x as u16, y as u16).expect("inside the sprite").rgb8();
            assert_eq!(
                got,
                [r, g, b, u8::MAX],
                "({x}, {y}) does not match the art: the sprite is sampled from the wrong place",
            );
            compared += 1;
        }
    }

    // The diamond is 1,012 of the square's 1,936 pixels. Without this the loop
    // above would pass on a sprite that decoded to nothing at all.
    assert_eq!(compared, 1012, "the diamond should be 1,012 drawn pixels");

    // And nothing outside the sprite's own square was touched.
    for y in 0..64 {
        for x in 0..64 {
            if x < side && y < side {
                continue;
            }
            assert_eq!(frame.pixel(x, y)[3], 0, "({x}, {y}) is outside the quad");
        }
    }
}

/// Level ground tiles the viewport exactly, with no pixel left over.
///
/// This is the assertion the projection lives or dies by: the diamonds only
/// meet if a step is exactly 22 pixels on each axis and the sprite is exactly
/// 44 across. Any other numbers leave a lattice of gaps, and a lattice of gaps
/// against a black background is close to invisible on a screenshot.
///
/// It is deliberately *level* ground. Flat diamonds are only the whole truth
/// where the four corners of a tile share a height, and the sea is the largest
/// place that is true — see the sibling test for what happens on a hillside.
#[test]
fn level_ground_covers_every_pixel() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");

    // Open sea off the north-west corner: 80 tiles square at a single height.
    let camera = Camera::new(Point::new(200, 200, -5), 768, 512);

    // The premise, checked rather than assumed. If this patch of Felucca ever
    // stopped being level the coverage assertion below would start measuring
    // the terrain instead of the projection, and would still be green.
    for y in 160..240u16 {
        for x in 160..240u16 {
            assert_eq!(
                map.land(x, y).map(|cell| cell.z),
                Some(-5),
                "({x}, {y}) is not at sea level; this test needs level ground",
            );
        }
    }

    let wanted = ground::visible_graphics(&map, &camera);
    let atlas = LandAtlas::build(&art, wanted.iter().copied()).expect("fits");
    let texmaps = texmap_atlas(&dir, wanted);
    let quads = ground::collect(&map, &camera, &atlas, &texmaps, &Cutaway::OPEN);
    assert!(!quads.is_empty(), "the sea is made of land tiles too");

    let frame = render(
        &device,
        &queue,
        &atlas,
        &texmaps,
        &quads,
        camera.width,
        camera.height,
    );
    let total = (camera.width * camera.height) as usize;
    assert_eq!(
        frame.drawn(),
        total,
        "level ground left holes: the diamonds do not meet",
    );

    // One flat colour would satisfy everything above, and is also what a broken
    // atlas produces.
    let first = frame.pixel(0, 0);
    assert!(
        (0..camera.height).any(|y| (0..camera.width).any(|x| frame.pixel(x, y) != first)),
        "the whole frame is one colour",
    );
}

/// A screen of Britain: hilly ground covers the viewport as completely as the
/// sea does.
///
/// This is the assertion stretched ground exists for. Flat 44x44 diamonds drawn
/// at different heights pull apart along a slope and leave a lattice of seams —
/// which is what this test used to pin, at 97.7% of the viewport. A tile
/// stretched over its four corner heights cannot do that: neighbours are built
/// from *the same* corners, so the mesh is watertight by construction rather
/// than by the projection's arithmetic coming out even.
///
/// Real terrain is also the only place the two shapes meet, so this covers the
/// join between them: a flat tile beside a sloped one, and no gap at the seam.
#[test]
fn hilly_ground_covers_every_pixel() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");

    // Britain, near the bank: the ground here runs from z = -15 to z = 25.
    let camera = Camera::new(Point::new(1495, 1629, 0), 768, 512);
    let wanted = ground::visible_graphics(&map, &camera);
    let atlas = LandAtlas::build(&art, wanted.iter().copied()).expect("fits");
    let texmaps = texmap_atlas(&dir, wanted);
    let quads = ground::collect(&map, &camera, &atlas, &texmaps, &Cutaway::OPEN);

    // Every cell the camera can see became a quad: the client ships art for all
    // of them, so a missing quad would mean the atlas or the lookup lost one.
    let bounds = camera.visible_tiles();
    let cells = (bounds.min_y.max(0)..=bounds.max_y)
        .flat_map(|y| (bounds.min_x.max(0)..=bounds.max_x).map(move |x| (x, y)))
        .filter(|&(x, y)| map.land(x as u16, y as u16).is_some())
        .count();
    assert_eq!(quads.len(), cells, "a visible tile was dropped");

    // The premise: this camera has to be looking at a hillside, or the test is
    // the level-ground one again under another name and would stay green
    // through the loss of everything it is here to protect.
    let sloped = quads
        .iter()
        .filter(|quad| quad.corners.iter().any(|z| *z != quad.corners[0]))
        .count();
    assert!(sloped > 100, "only {sloped} of {} quads slope", quads.len());

    // And most of those slopes are textured rather than falling back to the
    // stretched art, or the texture path is being exercised by nothing.
    let textured = quads
        .iter()
        .filter(|quad| quad.corners.iter().any(|z| *z != quad.corners[0]) && quad.texmap.is_some())
        .count();
    assert!(
        textured * 2 > sloped,
        "only {textured} of {sloped} sloped tiles have a texture map",
    );

    let frame = render(
        &device,
        &queue,
        &atlas,
        &texmaps,
        &quads,
        camera.width,
        camera.height,
    );
    let total = (camera.width * camera.height) as usize;
    assert_eq!(
        frame.drawn(),
        total,
        "hilly ground left holes: the corner heights do not meet",
    );
}

/// A sloped tile is drawn from its texture map, and a level one from its art.
///
/// The one assertion that says the branch in `ground.wgsl` is on the *heights*
/// and reads the *right* atlas. Both pictures are made here rather than read
/// from a client, and they are told apart by colour alone: green art, red
/// texture. Nothing subtler is needed and nothing subtler would survive a
/// reader's understanding writing both sides of the comparison — which is
/// exactly the trap `uofiles` fell into.
#[test]
fn a_sloped_tile_is_drawn_from_its_texture_and_a_level_one_from_its_art() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    let green = Color16(0b0_00000_11111_00000);
    let red = Color16(0b0_11111_00000_00000);

    let side = usize::from(LAND_TILE_SIZE);
    let art = Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![green; side * side]);
    let texture = Image::new(64, 64, vec![red; 64 * 64]);
    let atlas = LandAtlas::pack([(GRAPHIC, art)]).expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([(GRAPHIC, texture)]).expect("one texture fits");
    let region = atlas.region(GRAPHIC).expect("packed");
    let texmap = texmaps.region(GRAPHIC).expect("packed");

    // Two tiles in one frame, far enough apart not to touch: one level, one
    // over four different corner heights. Same graphic, same regions — only the
    // heights differ, which is the whole claim.
    let quads = [
        GroundQuad {
            x: 64.0,
            y: 128.0,
            corners: [0.0; 4],
            region,
            texmap: Some(texmap),
            depth: 0.5,
            place: Place::land(1, 1),
        },
        GroundQuad {
            // Its own corner raised and its neighbours level: a hillock, and
            // the one direction that makes the quad *bigger* than the diamond
            // rather than shearing it into something smaller.
            x: 192.0,
            y: 128.0,
            corners: [4.0, 0.0, 0.0, 0.0],
            region,
            texmap: Some(texmap),
            depth: 0.5,
            place: Place::land(2, 2),
        },
    ];
    let frame = render(&device, &queue, &atlas, &texmaps, &quads, 256, 256);

    let (mut art_pixels, mut textured_pixels) = (0, 0);
    for y in 0..256 {
        for x in 0..256 {
            let pixel = frame.pixel(x, y);
            if pixel[3] == 0 {
                continue;
            }
            // The left half is the level tile and the right half the slope, so
            // a colour on the wrong side is a tile drawn from the wrong atlas.
            let expected = if x < 128 { green } else { red };
            let Rgb8 {
                red: r,
                green: g,
                blue: b,
            } = expected.rgb8();
            let [r, g, b] = openshard_client_render::tonemap::shade_u8([r, g, b], [1.0; 3]);
            assert_eq!(
                pixel,
                [r, g, b, u8::MAX],
                "({x}, {y}) was drawn from the wrong atlas",
            );
            if x < 128 {
                art_pixels += 1;
            } else {
                textured_pixels += 1;
            }
        }
    }

    // A level tile is the art's diamond, exactly as the lone-sprite test pins
    // it. Anything else here and the comparison above was made against an empty
    // frame.
    assert_eq!(art_pixels, 1012, "the level tile is not the art's diamond");
    assert!(
        textured_pixels > 1012,
        "the sloped tile covered only {textured_pixels} pixels; it should be a stretched diamond",
    );
}

/// Read an RGBA8 texture back into a [`Frame`].
///
/// `width * 4` must be a multiple of 256, the row alignment a buffer copy
/// demands. Split out of [`render_both`] because the blit test reads two
/// textures — the world image and the surface it was blitted onto — and
/// comparing them is the whole assertion.
fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Frame {
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
    Frame { width, pixels }
}

/// Read a texture whose row may not be a WebGPU copy-alignment multiple.
///
/// The producer's fixed 864-pixel source is deliberately not 256-byte aligned
/// as RGBA8.  Test readback pads rows for the transfer, then removes that
/// padding before callers inspect pixels in the normal `width * height` order.
fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    bytes_per_texel: u32,
) -> Vec<u8> {
    let (width, height) = (texture.width(), texture.height());
    let row = width * bytes_per_texel;
    let stride = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("producer coverage readback"),
        size: u64::from(stride) * u64::from(height),
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
                bytes_per_row: Some(stride),
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
        result.expect("mapping a producer attachment we just copied");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting for the producer attachment readback");
    let padded = slice
        .get_mapped_range()
        .expect("the producer attachment map completed");
    let mut compact = Vec::with_capacity((row * height) as usize);
    for source_row in padded.chunks_exact(stride as usize) {
        compact.extend_from_slice(&source_row[..row as usize]);
    }
    drop(padded);
    readback.unmap();
    compact
}

fn producer_owner_tiles(
    block: BlockCoord,
    width: u32,
    kind: Kind,
    ids: &[u8],
    positions: &[u8],
    color: &[u8],
) -> BTreeSet<(u16, u16)> {
    let mut owned = BTreeSet::new();
    for at in 0..(width * width) as usize {
        let id = u32::from_le_bytes(ids[at * 4..at * 4 + 4].try_into().expect("one ID texel"));
        if openshard_client_render::gbuffer::ids_kind(id) != Some(kind) {
            continue;
        }
        assert_ne!(
            color[at * 4 + 3],
            0,
            "{kind:?} ID at texel {at} has transparent producer colour"
        );
        let x = f32::from_le_bytes(positions[at * 16..at * 16 + 4].try_into().expect("position x"));
        let y = f32::from_le_bytes(
            positions[at * 16 + 4..at * 16 + 8]
                .try_into()
                .expect("position y"),
        );
        assert!(
            x.is_finite() && y.is_finite(),
            "{kind:?} ID at texel {at} has no finite world position"
        );
        let tile = (x.floor() as u16, y.floor() as u16);
        if BlockCoord::containing(tile.0, tile.1) == block {
            owned.insert(tile);
        }
    }
    owned
}

/// A real map producer must retain every owned land and map-static tile after
/// it is captured and restored at LOD1. This is the coverage gate synthetic
/// attachment tests cannot provide: fixed camera, real map art, ownership
/// filtering, downsampled cache planes and deferred restore form one chain.
#[test]
fn real_map_block_producer_keeps_every_owned_map_tile_after_restore() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let tiledata = openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");
    // A dense central-Britain block: a quiet sea block proves only ground.
    let block = BlockCoord { x: 186, y: 203 };
    let key = CompositeKey {
        block,
        tier: CompositeTier::Lod1,
        revision: ImmutableRevision::default(),
    };
    let job = CompositeProducerJob::new(key);
    let size = job.source_size();
    let wanted = ground::visible_graphics(&map, &job.camera());
    let land = LandAtlas::build(&art, wanted.iter().copied()).expect("producer land atlas fits");
    let texmaps = texmap_atlas(&dir, wanted);
    let ground = ground::collect(&map, &job.camera(), &land, &texmaps, &Cutaway::OPEN);
    assert!(!ground.is_empty(), "the producer camera collected no map land");
    let animations = StaticAnimations::default();
    let wanted_statics = statics::visible_graphics(&map, &job.camera(), &animations);
    let static_atlas = StaticAtlas::build(&art, wanted_statics).expect("producer static atlas fits");
    let static_geometry = statics::collect(
        &map,
        &job.camera(),
        &tiledata,
        &animations,
        &static_atlas,
        &Cutaway::OPEN,
        &openshard_client_render::occlusion::Occlusion::EMPTY,
        None,
        None,
    );
    let static_rows = split_corners(static_geometry.quads);

    let source_world = openshard_client_render::blit::world_texture(&device, size.width, size.height);
    let source_world_view = source_world.create_view(&wgpu::TextureViewDescriptor::default());
    let source_depth = renderer::depth_texture(&device, size.width, size.height);
    let source_depth_view = source_depth.create_view(&wgpu::TextureViewDescriptor::default());
    let source_gbuffer = openshard_client_render::gbuffer::Gbuffer::new(&device, size.width, size.height);
    let source_views = source_gbuffer.views();
    let mut ground_pass = GroundRenderer::new(
        &device,
        &queue,
        openshard_client_render::blit::WORLD_FORMAT,
        &land,
        &texmaps,
    );
    let hues = HueRamp::build(&Hues::parse(&[0u8; 708]).expect("one empty hue group"));
    let mut statics_pass = SpriteRenderer::new(
        &device,
        &queue,
        openshard_client_render::blit::WORLD_FORMAT,
        static_atlas.pixels(),
        &hues,
    );
    let mut composite = CompositeRenderer::new(&device);
    let mut cache = CompositeCache::default();
    let mut work = CompositeWorkQueue::new(1, 1).expect("one bounded producer job");
    let bounds = MapBlockBounds {
        min_x: block.x,
        max_x: block.x,
        min_y: block.y,
        max_y: block.y,
    };
    work.refresh(
        bounds,
        bounds,
        openshard_client_render::lod::BlockLod::Lod1,
        key.revision,
        |_| false,
    );
    assert_eq!(
        work.take_for_frame().len(),
        1,
        "the oracle must dispatch its producer key"
    );

    let restored_world = openshard_client_render::blit::world_texture(&device, size.width, size.height);
    let restored_world_view = restored_world.create_view(&wgpu::TextureViewDescriptor::default());
    let restored_depth = renderer::depth_texture(&device, size.width, size.height);
    let restored_depth_view = restored_depth.create_view(&wgpu::TextureViewDescriptor::default());
    let restored_gbuffer = openshard_client_render::gbuffer::Gbuffer::new(&device, size.width, size.height);
    let restored_views = restored_gbuffer.views();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let source_target = Target::whole(
        &source_world_view,
        &source_depth_view,
        &source_views,
        size.width,
        size.height,
    );
    ground_pass.render(&device, &queue, &mut encoder, source_target, &ground);
    statics_pass.render(
        &device,
        &queue,
        &mut encoder,
        source_target,
        &static_rows.rows,
        &static_geometry.boxes,
        Some(static_rows.drawn),
    );
    let source = CaptureSource {
        color: &source_world,
        ids: source_gbuffer.ids(),
        position: source_gbuffer.position(),
        normal: source_gbuffer.normal(),
        depth: &source_depth_view,
        depth_base: 0,
        rect: ViewportRect {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height,
        },
    };
    work.finish_capture(
        &device,
        &queue,
        &mut encoder,
        &mut composite,
        &mut cache,
        key,
        source,
        job.ground(),
    )
    .expect("the dispatched producer capture completes");
    let texture = cache.get(key).expect("the capture inserted its exact key");
    {
        // `render_deferred` deliberately loads its target in the client: it
        // interleaves cached and detailed blocks. This isolated oracle has no
        // detailed neighbours, so make its untouched pixels deterministically
        // empty before asking the cache image to restore into them.
        let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("producer coverage restore clear"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: &restored_world_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &restored_views.ids,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(openshard_client_render::gbuffer::IDS_CLEAR),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &restored_views.position,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(openshard_client_render::gbuffer::POSITION_CLEAR),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &restored_views.normal,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(openshard_client_render::gbuffer::NORMAL_CLEAR),
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &restored_depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
    }
    composite.render_deferred(
        &device,
        &queue,
        &mut encoder,
        Target::whole(
            &restored_world_view,
            &restored_depth_view,
            &restored_views,
            size.width,
            size.height,
        ),
        0.0,
        &[CompositeQuad {
            texture,
            rect: job.source_rect(),
        }],
    );
    queue.submit([encoder.finish()]);

    // `origin` answers in `u32` because a block coordinate need not be on any
    // facet; this one is the block the producer was pointed at.
    let (first_x, first_y) = block.origin();
    let first_x = u16::try_from(first_x).expect("the producer block is on the facet");
    let first_y = u16::try_from(first_y).expect("the producer block is on the facet");
    let expected: BTreeSet<_> = (first_y..first_y + 8)
        .flat_map(|y| (first_x..first_x + 8).map(move |x| (x, y)))
        .filter(|&(x, y)| map.land(x, y).is_some())
        .collect();
    assert_eq!(
        expected.len(),
        64,
        "the selected producer block must be a complete map block"
    );
    let source_owned = producer_owner_tiles(
        block,
        size.width,
        Kind::Land,
        &read_texture(&device, &queue, source_gbuffer.ids(), 4),
        &read_texture(&device, &queue, source_gbuffer.position(), 16),
        &read_texture(&device, &queue, &source_world, 4),
    );
    let restored_land = producer_owner_tiles(
        block,
        size.width,
        Kind::Land,
        &read_texture(&device, &queue, restored_gbuffer.ids(), 4),
        &read_texture(&device, &queue, restored_gbuffer.position(), 16),
        &read_texture(&device, &queue, &restored_world, 4),
    );
    let source_statics = producer_owner_tiles(
        block,
        size.width,
        Kind::Static,
        &read_texture(&device, &queue, source_gbuffer.ids(), 4),
        &read_texture(&device, &queue, source_gbuffer.position(), 16),
        &read_texture(&device, &queue, &source_world, 4),
    );
    let restored_statics = producer_owner_tiles(
        block,
        size.width,
        Kind::Static,
        &read_texture(&device, &queue, restored_gbuffer.ids(), 4),
        &read_texture(&device, &queue, restored_gbuffer.position(), 16),
        &read_texture(&device, &queue, &restored_world, 4),
    );
    assert!(
        source_statics.len() >= 8,
        "the dense producer block exercised only {source_statics:?}"
    );
    // A tall/opaque map static can cover every land fragment of its own tile.
    // The immutable map representation still covers that tile, but with a
    // static ID rather than a land ID. Assert complete *visible map* coverage
    // and then compare each plane's identity across capture/restore.
    let source_visible = source_owned
        .union(&source_statics)
        .copied()
        .collect::<BTreeSet<_>>();
    let restored_visible = restored_land
        .union(&restored_statics)
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        source_visible, expected,
        "the producer source omitted an owned visible map tile"
    );
    assert_eq!(
        restored_visible, expected,
        "the cached composite omitted an owned visible map tile"
    );
    assert_eq!(
        restored_land, source_owned,
        "the cached composite changed owned land identity"
    );
    assert_eq!(
        restored_statics, source_statics,
        "the cached composite omitted an owned map-static tile"
    );
}

/// At zoom 1 the blit moves no pixel: every texel of the surface is the world
/// image's own texel, put through the colour pipeline and nothing else.
///
/// The property every pixel-exact assertion in this file depends on now that the
/// world is drawn offscreen and stretched onto the surface: if the blit is not
/// the identity at 1:1, then every other test here is measuring an image the
/// screen never shows. A half-texel of sampling error, a flipped vertical axis
/// or a filter left on all read as "slightly soft" on a screenshot and are exact
/// here.
///
/// It used to assert a byte-for-byte copy, and could while lighting was a
/// multiplication of stored bytes by `1.0`. `docs/lighting_rebuild.md`'s phase 1
/// decodes the art out of sRGB, multiplies in linear light and curves the
/// result, and a curve that left `1.0` alone would not be a curve. So the
/// prediction goes through `tonemap::shade_u8` — which is a **stronger**
/// assertion than the copy was: it catches a blit that shifts by a texel and a
/// colour pipeline that has drifted from its own CPU twin, and only the first of
/// those was ever covered here.
///
/// No client files: the scene is two coloured diamonds made in memory, which is
/// enough to have edges — a flat field of one colour would survive any of those
/// mistakes.
#[test]
fn the_blit_at_zoom_one_is_the_world_image_texel_for_texel() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    let side = usize::from(LAND_TILE_SIZE);
    let art = Image::new(
        LAND_TILE_SIZE,
        LAND_TILE_SIZE,
        (0..side * side)
            // A gradient rather than a wash: a filter left on averages
            // neighbours, and neighbours that differ are what makes that
            // visible.
            .map(|at| Color16(((at % 31) as u16) << 10 | ((at / 31 % 31) as u16) << 5 | 1))
            .collect(),
    );
    let atlas = LandAtlas::pack([(GRAPHIC, art)]).expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let region = atlas.region(GRAPHIC).expect("packed");
    let quads: Vec<GroundQuad> = [(40.0, 40.0), (150.0, 96.0)]
        .into_iter()
        .map(|(x, y)| GroundQuad {
            x,
            y,
            corners: [0.0; 4],
            region,
            texmap: None,
            depth: 0.5,
            place: Place::land(1, 1),
        })
        .collect();

    let (width, height) = (256, 256);
    let world = openshard_client_render::blit::world_texture(&device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(&device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(&device, width, height);
    let gbuffer_views = gbuffer.views();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut ground_pass = GroundRenderer::new(&device, &queue, format, &atlas, &texmaps);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    ground_pass.render(
        &device,
        &queue,
        &mut encoder,
        Target::whole(&world_view, &depth_view, &gbuffer_views, width, height),
        &quads,
    );
    queue.submit([encoder.finish()]);

    // The surface stands in for a window: same size, so a zoom of 1 asks the
    // blit for exactly the identity.
    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
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
    });
    let surface_view = surface.create_view(&wgpu::TextureViewDescriptor::default());
    let mut blit = Blit::new(&device, format);
    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    let dummy_mesh_instances = openshard_client_render::blit::dummy_mesh_instances(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    blit.render(
        &device,
        &queue,
        &mut encoder,
        // Qualified: this file has a `Frame` of its own, which is a read-back
        // picture rather than a blit's arguments.
        openshard_client_render::blit::Frame {
            target: &surface_view,
            world: &world_view,
            gbuffer: &gbuffer_views,
            // Ground only: nothing here ever indexes either static/mobile
            // buffer, but the ground quads drawn above are real, so their id
            // has to resolve through the real buffer and not a dummy.
            face_instances: &dummy_instances,
            item_instances: &dummy_instances,
            mobile_instances: &dummy_instances,
            mesh_instances: &dummy_mesh_instances,
            ground_instances: ground_pass.instances_buffer(),
            zoom: Zoom::ONE,
            rect: ViewportRect {
                x: 0,
                y: 0,
                width,
                height,
            },
        },
        // The identity: this test is about the blit being a copy, and lighting
        // is a multiplication by one for it.
        &Lighting::NONE,
    );
    queue.submit([encoder.finish()]);

    let drawn = read_back(&device, &queue, &world);
    let blitted = read_back(&device, &queue, &surface);

    // The scene has to be worth comparing. Two diamonds of a gradient cover a
    // couple of thousand pixels of a 65,536-pixel frame, and an empty frame
    // would compare equal to another empty frame.
    assert!(
        drawn.drawn() > 2000,
        "the world image holds only {} drawn pixels",
        drawn.drawn(),
    );
    for y in 0..height {
        for x in 0..width {
            let world = drawn.pixel(x, y);
            let got = blitted.pixel(x, y);
            // `Lighting::NONE` is a white ambient, so the light is `1.0` on
            // every channel and what is left is the pipeline itself.
            let rgb = openshard_client_render::tonemap::shade_u8([world[0], world[1], world[2]], [1.0; 3]);
            let want = [rgb[0], rgb[1], rgb[2], world[3]];
            // One step, for the reason the parity sweep allows one: the GPU and
            // the CPU evaluate the same `pow` on different hardware.
            for channel in 0..4 {
                let apart = i32::from(want[channel]) - i32::from(got[channel]);
                assert!(
                    apart.abs() <= 1,
                    "({x}, {y}) channel {channel}: the blit drew {got:?}, the pipeline says \
                     {want:?} for a world texel of {world:?}",
                );
            }
        }
    }
}

/// A light brightens the pixels under it and nothing else, and the ambient
/// darkens everything the light does not reach.
///
/// The only oracle the lighting shader has: everything else about it is CPU
/// arithmetic with tests of its own in `light.rs`, and the part that is neither
/// — the falloff, the multiply, the loop bound by a count in a uniform — exists
/// only as WGSL and can only be read back off a GPU.
///
/// The scene is a flat grey world image, deliberately: this test is about what
/// the *lighting* did to a pixel, and a gradient underneath would make every
/// comparison between two pixels a statement about the art as well.
#[test]
fn a_light_brightens_its_own_pool_and_the_ambient_darkens_the_rest() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let (width, height) = (256, 256);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let world = openshard_client_render::blit::world_texture(&device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(&device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(&device, width, height);
    let gbuffer_views = gbuffer.views();

    // A flat grey field: one land graphic whose art is a single value, drawn
    // over the whole frame. Mid-grey and not white, so that "brighter" is
    // expressible in both directions.
    const GRAPHIC: Graphic = Graphic(1);
    let side = usize::from(LAND_TILE_SIZE);
    let grey = Color16(15 << 10 | 15 << 5 | 15);
    let art = Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![grey; side * side]);
    let atlas = LandAtlas::pack([(GRAPHIC, art)]).expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let region = atlas.region(GRAPHIC).expect("packed");
    // The field, as a lattice of tiles: each quad carries a tile of its own in
    // the place channel, because the lighting is computed in tiles and a frame
    // whose every pixel named one tile would be lit as a single flat thing.
    // `spots` remembers where each of them landed, which is how this test asks
    // "how bright is tile (i, j)" of a picture.
    let mut quads = Vec::new();
    let mut spots = Vec::new();
    for (row, y) in (0..height as i32 + 44).step_by(22).enumerate() {
        for (column, x) in (-44..width as i32 + 44).step_by(44).enumerate() {
            let at = ((x + (y / 22 % 2) * 22) as f32, y as f32);
            let tile = (100 + column as u16, 100 + row as u16);
            quads.push(GroundQuad {
                x: at.0,
                y: at.1,
                corners: [0.0; 4],
                region,
                texmap: None,
                depth: 0.5,
                place: Place::land(tile.0, tile.1),
            });
            spots.push((tile, at));
        }
    }
    let mut ground_pass = GroundRenderer::new(&device, &queue, format, &atlas, &texmaps);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    ground_pass.render(
        &device,
        &queue,
        &mut encoder,
        Target::whole(&world_view, &depth_view, &gbuffer_views, width, height),
        &quads,
    );
    queue.submit([encoder.finish()]);

    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
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
    });
    let surface_view = surface.create_view(&wgpu::TextureViewDescriptor::default());
    let mut blit = Blit::new(&device, format);

    // Three tiles of one row, all of them well inside the picture: the one the
    // flame stands on, one half way out of its reach, and one outside it
    // altogether. Near the left edge, because the row has to be long enough to
    // hold all three at 44 pixels a tile.
    let inside = |at: &(f32, f32)| (10.0..230.0).contains(&at.0) && (30.0..200.0).contains(&at.1);
    let (burning, burning_at) = *spots
        .iter()
        .find(|(_, at)| inside(at) && at.0 < 40.0 && at.1 > 100.0)
        .expect("the lattice covers the left of the frame");
    let find = |dx: u16| {
        *spots
            .iter()
            .find(|((x, y), at)| *x == burning.0 + dx && *y == burning.1 && inside(at))
            .unwrap_or_else(|| panic!("no tile {dx} east of the flame is on screen"))
    };
    let (_, half_way_at) = find(1);
    let (_, outside_at) = find(TORCH_TILES as u16 + 1);

    // One flame, on its own tile, reaching six tiles of ground.
    //
    // At `FLAME_LIFT` and not at the tile's own `z`, which is where `light::gather`
    // puts every flame it builds: a fire's flame is above the thing that is
    // burning, and a source lying exactly *in* the ground's plane has a cosine of
    // zero against it — so a scene that puts one there is asking about a
    // degenerate case rather than about a torch.
    let lighting = Lighting {
        ambient: openshard_client_render::light::NIGHT,
        lights: vec![Light {
            at: Vec2::new(f32::from(burning.0), f32::from(burning.1)),
            z: openshard_client_render::light::FLAME_LIFT,
            radius: TORCH_TILES,
            color: [1.0, 0.7, 0.35],
            intensity: 1.0,
            // A fire on the ground, lighting every direction: what a beam does
            // is `a_carried_beam_lights_the_way_it_is_pointed`'s claim.
            beam: None,
        }],
        // Nothing stands in the way: what a wall does is
        // `a_wall_stops_the_light_behind_it`'s claim, not this one's.
        occlusion: Occlusion::EMPTY,
        sun: None,
        view: View::Lit,
        flame_radius: openshard_client_render::light::FLAME_RADIUS,
        shadow_rays: openshard_client_render::light::ShadowRays::DEFAULT,
        dead: false,
    };
    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    let dummy_mesh_instances = openshard_client_render::blit::dummy_mesh_instances(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    blit.render(
        &device,
        &queue,
        &mut encoder,
        openshard_client_render::blit::Frame {
            target: &surface_view,
            world: &world_view,
            gbuffer: &gbuffer_views,
            // Ground only, and the ground quads drawn above are real, so
            // their id has to resolve through the real buffer.
            face_instances: &dummy_instances,
            item_instances: &dummy_instances,
            mobile_instances: &dummy_instances,
            mesh_instances: &dummy_mesh_instances,
            ground_instances: ground_pass.instances_buffer(),
            zoom: Zoom::ONE,
            rect: ViewportRect {
                x: 0,
                y: 0,
                width,
                height,
            },
        },
        &lighting,
    );
    queue.submit([encoder.finish()]);

    let drawn = read_back(&device, &queue, &world);
    let lit = read_back(&device, &queue, &surface);
    // The scene has to have something in it, or every comparison below is
    // between two black pixels and holds for any shader at all.
    assert!(drawn.drawn() > 60_000, "the world image is mostly empty");

    // **In linear light, not in stored bytes.** Every comparison below is a
    // ratio — "twice as bright", "brighter than" — and a ratio of sRGB bytes is
    // a statement about a transfer function rather than about the light. The
    // frame is `encode(tonemap(radiance))`, so decoding gives back the tonemapped
    // radiance: monotone in the light, which is all a ratio here needs.
    let luma = |pixel: [u8; 4]| {
        let channel = |value: u8| openshard_client_render::tonemap::srgb_to_linear(f32::from(value) / 255.0);
        // Scaled to the same `0..765` the byte sum used, so the numbers in these
        // messages stay the size a reader of this test is used to.
        ((channel(pixel[0]) + channel(pixel[1]) + channel(pixel[2])) * 255.0) as u32
    };
    // A tile's own middle: `GroundQuad::x`/`y` is the diamond's centre in
    // viewport pixels, which is the one pixel of a tile that is certainly the
    // tile's own and not its neighbour's.
    let sample = |at: (f32, f32)| (at.0 as u32, at.1 as u32);
    let (cx, cy) = sample(burning_at);
    let centre = luma(lit.pixel(cx, cy));
    let (hx, hy) = sample(half_way_at);
    let edge_of_pool = luma(lit.pixel(hx, hy));
    let (fx, fy) = sample(outside_at);
    let far = luma(lit.pixel(fx, fy));
    let unlit = luma(drawn.pixel(fx, fy));

    assert!(
        far < unlit,
        "the ambient did not darken the frame: {far} against {unlit}"
    );
    assert!(
        centre > far * 2,
        "the pool is not brighter than the dark around it: {centre} against {far}",
    );
    assert!(
        edge_of_pool > far && edge_of_pool < centre,
        "the falloff is not monotonic: centre {centre}, edge {edge_of_pool}, outside {far}",
    );
    // The pool is warm, not white: a light whose colour was dropped would pass
    // every brightness assertion above.
    let middle = lit.pixel(cx, cy);
    assert!(
        middle[0] > middle[2],
        "the light's colour was ignored: {middle:?}",
    );
    // And nothing outside the radius is touched by the light at all — the
    // ambient alone accounts for it, which is what makes the pool a shape.
    // The grid is empty, so every tile sees the whole sky and the ambient here is
    // the night's two terms summed — see `light::Ambient`.
    let night = openshard_client_render::light::NIGHT.at(openshard_client_render::occlusion::SKY_OPEN);
    let outside = lit.pixel(fx, fy);
    for (channel, (got, (drawn, ambient))) in outside
        .iter()
        .zip(drawn.pixel(fx, fy).iter().zip(night))
        .take(3)
        .enumerate()
    {
        // Through the pipeline rather than by multiplying the stored byte —
        // `docs/lighting_rebuild.md` phase 1, and the same correction the wall
        // test below carries.
        let expected = (openshard_client_render::tonemap::shade(f32::from(*drawn) / 255.0, ambient) * 255.0)
            .round() as i32;
        assert!(
            (i32::from(*got) - expected).abs() <= 1,
            "outside the pool, channel {channel} is {got} against the ambient's {expected}",
        );
    }
}

/// A wall stops the light behind it.
///
/// The claim `docs/lighting.md` exists for: a torch inside a house must not
/// light the street. `docs/lighting.md`'s own second half of that
/// claim — a wall's own face stays the brightest thing near the flame — is
/// deliberately not this fixture's to make: it stands the occluder in as a
/// *ground* quad rather than a real wall sprite ("what occludes is the grid,
/// and a picture of a wall would only make the frame prettier", below), and
/// a ground pixel is `Stance::Flat`, not the `Stance::Face*` a real wall's
/// own visible surface carries. Those are exempted from a same-tile body's
/// own shadow by a different mechanism (`own_run`'s run-of-wall check) that
/// this fixture never exercises — `two_faces_sharing_an_edge_agree_with_
/// light_sample`/`the_shader_and_light_sample_agree_about_which_side_a_wall_
/// is_on` cover that half instead, on a real face. A `Flat` fragment gets no
/// such exemption at all, by design (`light::exemption`'s own doc: a body a
/// second, taller body stands on needs the precision), so ground standing at
/// a whole-tile body's own base is genuinely inside that body looking out —
/// `docs/lighting_raymarch.md`'s ground-stance entry found this fixture had
/// been passing for the wrong reason, propped up by a land pixel that never
/// named a stance at all and so read as `Upright`, not `Flat`, to the very
/// check this fixture meant to exercise.
#[test]
fn a_wall_stops_the_light_behind_it() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let (width, height) = (256, 256);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let world = openshard_client_render::blit::world_texture(&device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(&device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(&device, width, height);
    let gbuffer_views = gbuffer.views();

    // Four tiles of one row, drawn as four flat squares side by side: the flame
    // stands on the first, the wall is on the second, and the two after it are
    // what the wall's shadow falls on. Drawn as ground rather than as a wall
    // sprite deliberately — what occludes is the *grid*, and a picture of a wall
    // would only make the frame prettier.
    const GRAPHIC: Graphic = Graphic(1);
    let side = usize::from(LAND_TILE_SIZE);
    let grey = Color16(15 << 10 | 15 << 5 | 15);
    let art = Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![grey; side * side]);
    let atlas = LandAtlas::pack([(GRAPHIC, art)]).expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let region = atlas.region(GRAPHIC).expect("packed");

    const ROW: u16 = 100;
    const FIRST: u16 = 100;
    let centre_of = |tile: u16| (40.0 + f32::from(tile - FIRST) * 44.0, 128.0);
    let quads: Vec<GroundQuad> = (0..4u16)
        .map(|step| {
            let at = centre_of(FIRST + step);
            GroundQuad {
                x: at.0,
                y: at.1,
                corners: [0.0; 4],
                region,
                texmap: None,
                depth: 0.5,
                place: Place::land(FIRST + step, ROW),
            }
        })
        .collect();

    let mut ground_pass = GroundRenderer::new(&device, &queue, format, &atlas, &texmaps);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    ground_pass.render(
        &device,
        &queue,
        &mut encoder,
        Target::whole(&world_view, &depth_view, &gbuffer_views, width, height),
        &quads,
    );
    queue.submit([encoder.finish()]);

    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
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
    });
    let surface_view = surface.create_view(&wgpu::TextureViewDescriptor::default());
    let mut blit = Blit::new(&device, format);

    let flame = Light {
        at: Vec2::new(f32::from(FIRST), f32::from(ROW)),
        // Where `light::gather` puts a flame on a tile — see the pool test's own
        // note. In the ground's own plane the cosine against it is zero and the
        // whole row would be dark with or without a wall in it.
        z: openshard_client_render::light::FLAME_LIFT,
        // Six tiles, so that the far side of the wall is inside the pool and
        // dark only because the wall is there — a radius that fell short would
        // pass this test for the wrong reason.
        //
        // It was four until phase 3, and four stopped being enough for a reason
        // that is not the radius: a flame half a tile up throws a cosine of a
        // sixth onto ground three tiles away, and `(1 - d)²` of that landed under
        // one step of the frame's eight bits. The pool still *reached* the far
        // tile; it no longer said anything there that a byte could hold, so the
        // walled and open frames read alike and the test would have passed by
        // measuring nothing. Widening the reach is what puts a measurable
        // quantity back at the tile being asked about.
        radius: 6.0,
        color: [1.0, 1.0, 1.0],
        intensity: 1.0,
        beam: None,
    };
    let bounds = TileBounds {
        min_x: 90,
        max_x: 110,
        min_y: 90,
        max_y: 110,
    };

    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    let dummy_mesh_instances = openshard_client_render::blit::dummy_mesh_instances(&device);
    let read = |blit: &mut Blit, occlusion: Occlusion| -> Frame {
        let lighting = Lighting {
            ambient: openshard_client_render::light::NIGHT,
            lights: vec![flame],
            occlusion,
            sun: None,
            view: View::Lit,
            flame_radius: openshard_client_render::light::FLAME_RADIUS,
            shadow_rays: openshard_client_render::light::ShadowRays::DEFAULT,
            dead: false,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        blit.render(
            &device,
            &queue,
            &mut encoder,
            openshard_client_render::blit::Frame {
                target: &surface_view,
                world: &world_view,
                gbuffer: &gbuffer_views,
                // Ground only, and the ground quads drawn above are real, so
                // their id has to resolve through the real buffer.
                face_instances: &dummy_instances,
                item_instances: &dummy_instances,
                mobile_instances: &dummy_instances,
                mesh_instances: &dummy_mesh_instances,
                ground_instances: ground_pass.instances_buffer(),
                zoom: Zoom::ONE,
                rect: ViewportRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
            },
            &lighting,
        );
        queue.submit([encoder.finish()]);
        read_back(&device, &queue, &surface)
    };

    let luma = |pixel: [u8; 4]| u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2]);
    let at = |frame: &Frame, tile: u16| {
        let (x, y) = centre_of(tile);
        luma(frame.pixel(x as u32, y as u32))
    };

    // With nothing in the way, every tile of the row is lit.
    let open = read(&mut blit, Builder::new(bounds).finish(&Cutaway::OPEN));
    let (open_wall, open_behind, open_far) = (at(&open, 101), at(&open, 102), at(&open, 103));

    // And with a wall on the second tile, the two behind it are not — while the
    // wall's own tile is exactly as bright as it was.
    let mut occlusion = Builder::new(bounds);
    occlusion.add(
        101,
        ROW,
        0,
        // A plain wall graphic: `occlusion::opacity` asks `doors` first, and a
        // door would be exempt for a reason this test is not about.
        Graphic(0x0100),
        &StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT),
            height: 20,
            ..StaticTile::default()
        },
        // No face: the whole-tile occluder this test has always been about. A
        // named edge would let the ray past on the sides it does not cross,
        // which is a different test — see `occlusion`'s own.
        Shape::UNREAD,
    );
    let walled = read(&mut blit, occlusion.finish(&Cutaway::OPEN));
    let (wall, behind, far) = (at(&walled, 101), at(&walled, 102), at(&walled, 103));

    // The ground *at* the wall's own base is genuinely inside its body looking
    // out at the flame — see this test's own doc comment — and the query
    // point sits at the tile's own *centre*, deep enough inside the body's
    // whole-tile footprint that the ray crosses well past `RAY_CUTOFF`
    // before it ever reaches open air: as dark as `behind`, not a hair off it.
    assert_eq!(
        wall, behind,
        "the wall's own tile did not read as fully inside its own body: {wall}, open {open_wall}, behind {behind}",
    );
    assert!(
        wall < open_wall,
        "the wall's own tile was not darkened by its own body: {wall}, open {open_wall}",
    );
    assert!(
        behind < open_behind && far < open_far,
        "the wall did not stop the light: {behind} of {open_behind}, {far} of {open_far}",
    );
    // Not merely dimmer: as dark as the ambient alone, which is what "stops"
    // means and what a falloff that happened to be steep would not reproduce.
    let world_pixel = read_back(&device, &queue, &world).pixel(centre_of(102).0 as u32, 128);
    // Nothing was ever shaded into this grid — the wall went in through
    // `Occlusion::add`, which is about rays and not about the sky — so every tile
    // still sees the whole of it and the ambient is the night's two terms summed.
    let night = openshard_client_render::light::NIGHT.at(openshard_client_render::occlusion::SKY_OPEN);
    // Through the colour pipeline, not by multiplying the stored bytes: an
    // unlit byte times an ambient is not the byte an ambient produces, and
    // `docs/lighting_rebuild.md` phase 1 is that sentence.
    let ambient_pixel =
        openshard_client_render::tonemap::shade_u8([world_pixel[0], world_pixel[1], world_pixel[2]], night);
    let expected: u32 = ambient_pixel.iter().copied().map(u32::from).sum();
    assert!(
        behind <= expected + 3,
        "the shadow is a dimming rather than a shadow: {behind} against the ambient's {expected}",
    );
}

/// The world passes draw into the world texture on a surface that is not
/// `Rgba8Unorm`.
///
/// The surface's format and the world texture's are two different values, and
/// the frame here is the arrangement that makes them differ: an HDR display
/// offers `Rgba16Float` first among its non-sRGB formats, and a world pipeline
/// built from the surface's format instead of `blit::WORLD_FORMAT` fails
/// validation at `set_pipeline` — the whole client dies on the first frame with
/// nothing drawn. Nothing is read back: the assertion is that the submission
/// validates at all, and a mismatch panics inside `wgpu` before it returns.
#[test]
fn the_world_passes_are_built_for_the_world_texture_not_the_surface() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let (width, height) = (64, 64);
    let world = openshard_client_render::blit::world_texture(&device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(&device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(&device, width, height);
    let gbuffer_views = gbuffer.views();

    // A sprite made here rather than read from a client, and one real quad: a
    // pass handed nothing returns before it binds its pipeline, which is the
    // one step this test is about.
    const GRAPHIC: Graphic = Graphic(1);
    let art = Image::new(8, 8, vec![Color16(0b0_00000_11111_00000); 64]);
    let atlas = StaticAtlas::pack([(GRAPHIC, art)]).expect("one sprite fits");
    let sprite = atlas.sprite(GRAPHIC).expect("packed");
    let quads = [SpriteQuad {
        rect: Rect {
            x: 4.0,
            y: 4.0,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
        volumes: openshard_client_render::impostor::Range::default(),
    }];
    let hue_ramp = HueRamp::build(&Hues::parse(&[0u8; 708]).expect("one empty group"));
    let mut sprites = SpriteRenderer::new(
        &device,
        &queue,
        openshard_client_render::blit::WORLD_FORMAT,
        atlas.pixels(),
        &hue_ramp,
    );
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    sprites.render(
        &device,
        &queue,
        &mut encoder,
        Target::whole(&world_view, &depth_view, &gbuffer_views, width, height),
        &quads,
        &[],
        None,
    );

    // The stand-in for the HDR surface, in the format the blit and the HUD —
    // and only they — are built for.
    let surface_format = wgpu::TextureFormat::Rgba16Float;
    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: surface_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let surface_view = surface.create_view(&wgpu::TextureViewDescriptor::default());
    let mut blit = Blit::new(&device, surface_format);
    // No mobile pass in this test: the dummy stands in for it. No ground pass
    // either — this test is about the sprite pass's own target format — so
    // the dummy stands in for that too.
    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    let dummy_mesh_instances = openshard_client_render::blit::dummy_mesh_instances(&device);
    let dummy_ground_instances = openshard_client_render::blit::dummy_ground_instances(&device);
    blit.render(
        &device,
        &queue,
        &mut encoder,
        // Qualified: this file has a `Frame` of its own, which is a read-back
        // picture rather than a blit's arguments.
        openshard_client_render::blit::Frame {
            target: &surface_view,
            world: &world_view,
            gbuffer: &gbuffer_views,
            face_instances: sprites.instances_buffer(),
            item_instances: sprites.instances_buffer(),
            mobile_instances: &dummy_instances,
            mesh_instances: &dummy_mesh_instances,
            ground_instances: &dummy_ground_instances,
            zoom: Zoom::ONE,
            rect: ViewportRect {
                x: 0,
                y: 0,
                width,
                height,
            },
        },
        // The identity: this test is about the blit being a copy, and lighting
        // is a multiplication by one for it.
        &Lighting::NONE,
    );
    queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting on our own submission");
}

/// A static sprite is drawn at its own size, in its own place, and its
/// transparent pixels are not drawn at all.
///
/// The statics counterpart of the lone-sprite test, and it needs no client: the
/// picture is made here, so the frame can be compared against it exactly. What
/// it pins is the whole chain — the shelf packer put the sprite somewhere, the
/// instance carried a rectangle and a region, and the shader sampled one texel
/// per pixel. A sprite drawn at the wrong scale still looks like a sprite.
#[test]
fn a_static_sprite_is_drawn_texel_for_texel_with_its_shape_intact() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    let (width, height) = (17u16, 23u16);

    // A picture with a hole in it: the middle column is absent, which is what
    // a sprite's shape is made of and what the pass has to discard rather than
    // draw black.
    let mut pixels = vec![Color16(0b0_00000_11111_00000); usize::from(width) * usize::from(height)];
    for row in 0..usize::from(height) {
        pixels[row * usize::from(width) + 8] = Color16::TRANSPARENT;
    }
    let art = Image::new(width, height, pixels.clone());
    let atlas = StaticAtlas::pack([(GRAPHIC, art)]).expect("one sprite fits");
    let sprite = atlas.sprite(GRAPHIC).expect("packed");

    let quads = [SpriteQuad {
        rect: Rect {
            x: 10.0,
            y: 20.0,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
        volumes: openshard_client_render::impostor::Range::default(),
    }];
    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let none = AnimAtlas::pack([]).expect("nothing always fits");
    let frame = render_both(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &atlas,
        &quads,
        (none.pixels(), &[]),
        128,
        128,
        Projection::one_to_one(128, 128),
    );

    let Rgb8 {
        red: green_r,
        green: green_g,
        blue: green_b,
    } = Color16(0b0_00000_11111_00000).rgb8();
    let mut drawn = 0;
    for y in 0..128u32 {
        for x in 0..128u32 {
            let got = frame.pixel(x, y);
            let inside =
                (10..10 + u32::from(width)).contains(&x) && (20..20 + u32::from(height)).contains(&y);
            let transparent = inside && x - 10 == 8;
            if !inside || transparent {
                assert_eq!(got[3], 0, "({x}, {y}) should not have been drawn");
                continue;
            }
            assert_eq!(
                got,
                [green_r, green_g, green_b, u8::MAX],
                "({x}, {y}) is not the sprite's own pixel",
            );
            drawn += 1;
        }
    }
    // Every pixel of the rectangle except the absent column, and nothing else:
    // a sprite drawn at the wrong scale fails this even when every pixel it did
    // draw was the right colour.
    assert_eq!(drawn, usize::from(width - 1) * usize::from(height));
}

/// Cutaway art is composed over the world, rather than replacing it. The
/// destination is an ordinary opaque static here; a mobile has the same depth
/// and colour contract, while this keeps the assertion independent of anim art.
#[test]
fn a_cutaway_sprite_is_alpha_blended_over_the_picture_behind_it() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const BEHIND: Graphic = Graphic(1);
    const CUTAWAY: Graphic = Graphic(2);
    let green = Color16(0b0_00000_11111_00000);
    let red = Color16(0b0_11111_00000_00000);
    let atlas = StaticAtlas::pack([
        (BEHIND, Image::new(1, 1, vec![green])),
        (CUTAWAY, Image::new(1, 1, vec![red])),
    ])
    .expect("two one-pixel sprites fit");
    let quad = |graphic: Graphic, depth: f32| {
        let sprite = atlas.sprite(graphic).expect("packed");
        SpriteQuad {
            rect: Rect {
                x: 20.0,
                y: 20.0,
                width: 1.0,
                height: 1.0,
            },
            region: sprite.region,
            depth,
            hue: 0,
            place: Place::of_static(Point::new(100, 100, 0)),
            twin: 0,
            owner: 0,
            volumes: Range::default(),
        }
    };
    let land = LandAtlas::pack([]).expect("empty land atlas");
    let texmaps = TexmapAtlas::pack([]).expect("empty texmap atlas");
    let mobiles = AnimAtlas::pack([]).expect("empty animation atlas");
    let frame = render_both_with_cutaway(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &atlas,
        &[quad(BEHIND, 0.6)],
        (mobiles.pixels(), &[]),
        &[quad(CUTAWAY, 0.5)],
        64,
        64,
        Projection::one_to_one(64, 64),
    );

    let pixel = frame.picture.pixel(20, 20);
    // Both layers have already passed through their ordinary deferred-lighting
    // curve; the cutaway blit then premultiplies red at this alpha. Source-over
    // therefore combines the displayed values, not the source art's raw bytes.
    let alpha = openshard_client_render::cutaway::TRANSLUCENT_ALPHA;
    let [red, _, _] = openshard_client_render::tonemap::shade_u8([255, 0, 0], [1.0; 3]);
    let [_, green, _] = openshard_client_render::tonemap::shade_u8([0, 255, 0], [1.0; 3]);
    let red = (f32::from(red) * alpha).round() as i16;
    let green = (f32::from(green) * (1.0 - alpha)).round() as i16;
    assert!(
        (i16::from(pixel[0]) - red).abs() <= 1,
        "red was not blended: {pixel:?}"
    );
    assert!(
        (i16::from(pixel[1]) - green).abs() <= 1,
        "green was not retained: {pixel:?}"
    );
    assert_eq!(pixel[2], 0, "unexpected blue in the blended pixel: {pixel:?}");
    assert_eq!(pixel[3], 255, "blending changed the frame coverage: {pixel:?}");
    assert_eq!(
        openshard_client_render::gbuffer::ids_kind(frame.main_id(20, 20)),
        Some(Kind::Static),
        "the cutaway replaced the main G-buffer identity instead of only blending over it"
    );
}

/// One `hues.mul` group, `Hue(1)`'s ramp set to `colors` and the other seven
/// entries left at zero — the same construction [`crate`]'s own unit tests
/// cannot reuse across crates, so this test file builds its own bytes from the
/// documented layout rather than from a private helper.
fn one_hue_group(colors: [Color16; 32]) -> Hues {
    const ENTRY_BYTES: usize = 32 * 2 + 2 + 2 + 20;
    let mut bytes = vec![0u8; 4 + 8 * ENTRY_BYTES];
    for (index, color) in colors.iter().enumerate() {
        let at = 4 + index * 2;
        bytes[at..at + 2].copy_from_slice(&color.0.to_le_bytes());
    }
    Hues::parse(&bytes).expect("one whole group")
}

/// The art is not tinted, it is replaced: a full hue looks a pixel up by its
/// own red channel and draws whatever `hues.mul` says, discarding the pixel's
/// original colour entirely — even a pixel that was never grey.
///
/// Both texels below carry the same 5-bit red value and different green and
/// blue, so a shader that multiplied by a tint would leave them visibly
/// different; one that replaces them by index draws them identically.
#[test]
fn a_full_hue_replaces_the_pixel_by_its_red_channel_regardless_of_its_own_colour() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    const INDEX: u8 = 10;

    // Genuinely grey — all three channels equal `INDEX` — against a texel
    // whose red channel is the same `INDEX` but whose green and blue are not:
    // "partial" is decided by the *pixel*, not by the index alone, so the two
    // have to share an index and differ in colour for the test to mean anything.
    let index = u16::from(INDEX);
    let grey = Color16((index << 10) | (index << 5) | index);
    let coloured = Color16((index << 10) | 0b0_00000_00000_11111);
    assert_ne!(
        grey, coloured,
        "the two texels have to differ for this to test anything"
    );

    let art = Image::new(2, 1, vec![grey, coloured]);
    let atlas = StaticAtlas::pack([(GRAPHIC, art)]).expect("one sprite fits");
    let sprite = atlas.sprite(GRAPHIC).expect("packed");

    let mut ramp_colors = [Color16::TRANSPARENT; 32];
    ramp_colors[usize::from(INDEX)] = Color16(0b0_00000_00000_11111); // pure blue
    let hues = one_hue_group(ramp_colors);
    let hue_ramp = HueRamp::build(&hues);

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");

    let quad = |hue: u32| SpriteQuad {
        rect: Rect {
            x: 0.0,
            y: 0.0,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.5,
        hue,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
        volumes: openshard_client_render::impostor::Range::default(),
    };

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let render_with_ramp = |hue: u32| -> Frame {
        let quads = [quad(hue)];
        render_hued(
            &device, &queue, &land, &texmaps, &atlas, &quads, &hue_ramp, format,
        )
    };

    let Rgb8 {
        red: blue_r,
        green: blue_g,
        blue: blue_b,
    } = Color16(0b0_00000_00000_11111).rgb8();
    let Rgb8 {
        red: grey_r,
        green: grey_g,
        blue: grey_b,
    } = grey.rgb8();
    let Rgb8 {
        red: coloured_r,
        green: coloured_g,
        blue: coloured_b,
    } = coloured.rgb8();

    // Hue 1, no partial flag: both texels come back as the ramp's own colour,
    // not as anything blended with what was there.
    let full = render_with_ramp(1);
    assert_eq!(
        full.pixel(0, 0),
        [blue_r, blue_g, blue_b, u8::MAX],
        "the grey texel"
    );
    assert_eq!(
        full.pixel(1, 0),
        [blue_r, blue_g, blue_b, u8::MAX],
        "the coloured texel too — a full hue does not ask what a pixel looked like",
    );

    // The same hue, partial: only the grey texel is grey enough to tint: the
    // coloured one is left exactly as the art drew it.
    let partial = render_with_ramp(1 | 0x8000);
    assert_eq!(
        partial.pixel(0, 0),
        [blue_r, blue_g, blue_b, u8::MAX],
        "partial still tints a genuinely grey pixel",
    );
    assert_eq!(
        partial.pixel(1, 0),
        [coloured_r, coloured_g, coloured_b, u8::MAX],
        "partial leaves an already-coloured pixel alone",
    );

    // And hue 0 is "no hue": the ramp exists but nothing here samples it.
    let none_hue = render_with_ramp(0);
    assert_eq!(none_hue.pixel(0, 0), [grey_r, grey_g, grey_b, u8::MAX]);
    assert_eq!(
        none_hue.pixel(1, 0),
        [coloured_r, coloured_g, coloured_b, u8::MAX]
    );
}

/// Draw one static pass with a real hue ramp bound, and read the result back.
///
/// [`render_both`] always binds an empty ramp — nothing it draws asks for a
/// hue — so a test that needs a real one calls this instead rather than
/// growing every caller of `render_both` an argument none of them use.
#[allow(clippy::too_many_arguments)]
fn render_hued(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    land: &LandAtlas,
    texmaps: &TexmapAtlas,
    static_atlas: &StaticAtlas,
    quads: &[SpriteQuad],
    hue_ramp: &HueRamp,
    format: wgpu::TextureFormat,
) -> Frame {
    let (width, height) = (64u32, 64u32);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hued frame"),
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
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hued readback"),
        size: u64::from(width) * u64::from(height) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let depth = renderer::depth_texture(device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(device, width, height);
    let gbuffer_views = gbuffer.views();

    let mut ground = GroundRenderer::new(device, queue, format, land, texmaps);
    let mut statics = SpriteRenderer::new(device, queue, format, static_atlas.pixels(), hue_ramp);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target_view = Target::whole(&view, &depth_view, &gbuffer_views, width, height);
    ground.render(device, queue, &mut encoder, target_view, &[]);
    statics.render(device, queue, &mut encoder, target_view, quads, &[], None);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
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

    Frame { width, pixels }
}

/// Every pixel says which tile it came from, and a wall's pixels say the
/// wall's tile rather than the ground's.
///
/// The attachment `docs/lighting.md` turns on, and the claim that makes it worth
/// having: a wall's picture stands 44 pixels above the tile it is on, so the
/// ground behind it and the wall itself are neighbouring pixels of one image
/// that belong to different tiles at different heights. Everything the lighting
/// pass does rests on being able to tell those two apart, and nothing else in
/// this suite would notice if the channel held the ground's tile everywhere.
#[test]
fn every_pixel_names_the_tile_it_came_from() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    let green = Color16(0b0_00000_11111_00000);
    let red = Color16(0b0_11111_00000_00000);

    let side = usize::from(LAND_TILE_SIZE);
    let land = LandAtlas::pack([(
        GRAPHIC,
        Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![green; side * side]),
    )])
    .expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([(GRAPHIC, Image::new(20, 20, vec![red; 20 * 20]))]).expect("fits");
    let region = land.region(GRAPHIC).expect("packed");
    let sprite = statics.sprite(GRAPHIC).expect("packed");

    // The ground tile fills the middle of the image; the wall stands on the
    // *next* tile at a height of its own and is drawn over part of it, which is
    // exactly the pair of pixels this is about.
    let ground = [GroundQuad {
        x: 64.0,
        y: 64.0,
        corners: [0.0; 4],
        region,
        texmap: None,
        depth: 0.6,
        place: Place::land(300, 400),
    }];
    let wall = [SpriteQuad {
        rect: Rect {
            x: 60.0,
            y: 60.0,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.4,
        hue: 0,
        place: Place::of_static(Point::new(301, 400, 15)),
        twin: 0,
        owner: 0,
        volumes: openshard_client_render::impostor::Range::default(),
    }];

    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &ground,
        &statics,
        &wall,
        &[],
        &[],
        &[],
        128,
    );

    // A pixel of the wall: an id naming the one static this frame drew — its
    // own tile is `docs/gbuffer.md` step 3's `instances[id]` row now, not a
    // number this attachment carries directly — and the static's kind in the
    // low bits of the fourth channel.
    let wall_pixel = places.at(64, 64);
    assert_eq!(
        gbuffer::ids_id(wall_pixel),
        0,
        "a wall's pixel did not name the only static drawn this frame, by id",
    );
    assert_eq!(
        gbuffer::ids_kind(wall_pixel),
        Some(Kind::Static),
        "and another kind",
    );
    // Its height is the pixel's own, not the sprite's base: four pixels up the
    // picture is one unit of `z`, which is what gives a wall a gradient down its
    // face instead of one flat brightness.
    //
    // And two pixels up is **half** a unit, which the position plane says
    // outright. The attachment this replaced could only say it after
    // `docs/lighting_height.md` phase 1 put a fraction of sixteenths under the
    // whole units; before that, this same pair of pixels differed by a whole
    // unit or by none at all, which is the staircase that phase is about — and
    // sixteenths are gone now too, along with everything else that quantised a
    // height on the way to a reader.
    let higher = places.position_at(64, 62);
    let wall_point = places.position_at(64, 64);
    assert_eq!(
        higher[2] - wall_point[2],
        0.5,
        "two pixels up the wall is not half a unit of height: {higher:?} against {wall_point:?}",
    );
    // A pixel of the ground beside it: an id naming the one ground quad this
    // frame drew — its own tile is `docs/gbuffer.md` step 7's
    // `ground_instances[id]` row, not a number a fragment carries directly, the
    // same move step 3 made for the wall above — at the height the corners gave
    // it, and the land kind.
    let ground_pixel = places.at(64, 84);
    assert_eq!(
        ground[gbuffer::ids_id(ground_pixel) as usize].place,
        Place::land(300, 400),
        "the ground beside the wall named something else",
    );
    // `ground.wgsl` stamps its own stance, the same way `statics.wgsl` always
    // has — see `docs/lighting_raymarch.md`'s ground-stance entry for why a land
    // pixel that never named one read as `Stance::Upright` to `blit.wgsl`'s own
    // exemption logic instead.
    assert_eq!(
        gbuffer::ids_stance(ground_pixel),
        openshard_client_render::place::Stance::Flat as u32,
        "and another stance",
    );
    assert_eq!(places.position_at(64, 84)[2], 0.0, "and another height",);
    assert_eq!(
        gbuffer::ids_kind(ground_pixel),
        Some(Kind::Land),
        "and another kind",
    );
    // And the ground's place in its tile moves with the pixel, which is what
    // the lighting reads to make a pool a gradient rather than a set of flat
    // tiles. Two pixels apart on the screen are two different places in a tile.
    assert_ne!(
        places.position_at(70, 84)[0],
        places.position_at(64, 84)[0],
        "a pixel six across is at the same place in its tile",
    );
    // And a corner nothing was drawn on stays the clear value, whose kind is
    // `Nothing` — a background the lighting must leave alone.
    assert_eq!(places.at(2, 2), 0, "an untouched pixel claimed a tile");
}

/// A floor's pixels are spread across its tile; a wall's run up it.
///
/// The two stances of `place::Stance`, and the reason there are two: a floor
/// static is a picture *of* the tile's diamond, so its fraction has to move in
/// both directions and its height must not move at all, while a wall is a
/// billboard whose picture is height and whose only horizontal information is
/// how far across the column a pixel is. Written because a floor lit as one flat
/// value per tile — which is what a constant fraction gives — is the blockiness
/// this stance exists to remove, and no other test here would see it.
#[test]
fn a_floor_spreads_across_its_tile_and_a_wall_stands_up_it() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    const SIDE: u16 = 44;
    const ORIGIN: f32 = 40.0;
    let red = Color16(0b0_11111_00000_00000);

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([(
        GRAPHIC,
        Image::new(SIDE, SIDE, vec![red; usize::from(SIDE) * usize::from(SIDE)]),
    )])
    .expect("fits");
    let sprite = statics.sprite(GRAPHIC).expect("packed");
    // `volumes` is which of the frame's boxes this picture's own pixels are met
    // against — the whole of the impostor's association, and the difference
    // between the two halves of this test: the floor has a lid, and the wall
    // below is deliberately given nothing.
    let quad = |place, volumes| SpriteQuad {
        rect: Rect {
            x: ORIGIN,
            y: ORIGIN,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.4,
        hue: 0,
        place,
        twin: 0,
        owner: 0,
        volumes,
    };
    let at = Point::new(301, 400, 15);
    // Where in its tile a pixel is, and how high — both off the position plane
    // and both exact. They used to be a seven-bit fraction and a height in
    // sixteenths, read out of the two `u16`s an attachment carried them in, so
    // "the middle of the tile" was `64` of `127` rather than `0.5`.
    let sub = |point: [f32; 4]| (point[0] - f32::from(at.x), point[1] - f32::from(at.y));
    let height = |point: [f32; 4]| point[2];
    // A floor's own box: the lid the occlusion grid stands for it, flat at the
    // static's own height and the whole tile across. Stated through
    // `Occlusion::box_of` rather than as six numbers, because that function is
    // the one place a kind becomes geometry and a fixture inventing its own slab
    // would assert about a shape the grid never holds.
    let lid = [Volume::of(
        &occlusion::Solid::box_of(
            i32::from(at.x),
            i32::from(at.y),
            i32::from(at.z),
            i32::from(at.z),
            occlusion::Edges::NONE,
        ),
        occlusion::Edges::NONE,
        None,
    )];
    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &[quad(Place::of_floor(at), Range { offset: 0, count: 1 })],
        &lid,
        &[],
        &[],
        128,
    );
    // The middle of the sprite is the middle of the tile, and the four
    // directions off it are the four world directions — a step right is further
    // along `x` and less along `y`, a step down is further along both.
    let middle = places.position_at(62, 62);
    let (mid_x, mid_y) = sub(middle);
    let (right_x, right_y) = sub(places.position_at(72, 62));
    let (below_x, below_y) = sub(places.position_at(62, 72));
    assert!(
        right_x > mid_x && right_y < mid_y,
        "right of the middle: {right_x} {right_y}"
    );
    assert!(
        below_x > mid_x && below_y > mid_y,
        "below the middle: {below_x} {below_y}"
    );
    // And a floor is at one height everywhere: what runs down its picture is the
    // tile, which the fraction has already spent.
    assert_eq!(
        [height(middle), height(places.position_at(62, 72))],
        [f32::from(at.z); 2],
        "a floor's pixels stand at different heights",
    );
    // And it says it is a floor, in the id word's stance bits. That a fragment
    // carries a stance at all is what lets the lighting tell a mesh face's row
    // from a sprite's and keeps a run of wall from shadowing itself — see
    // `crate::place::Stance` — and a floor's own value is what says the bits are
    // being written rather than left at zero.
    assert_eq!(
        gbuffer::ids_stance(places.at(62, 62)),
        openshard_client_render::place::Stance::Flat as u32,
        "a floor's pixel does not carry its stance",
    );

    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &[quad(Place::of_static(at), Range::default())],
        &[],
        &[],
        &[],
        128,
    );
    // A wall claims the middle of its tile at every pixel, and deliberately: what
    // its picture runs along is the world axis the wall is built on, which in
    // this projection is a screen diagonal, and nothing in the tiledata says
    // which of the two axes it is. Spreading its pixels along the horizontal —
    // `x - y`, the one direction no wall runs — is the shape this asserts is not
    // being written.
    let (mid_x, mid_y) = sub(places.position_at(62, 62));
    assert_eq!(
        (mid_x, mid_y),
        (0.5, 0.5),
        "a wall's fraction is not its tile's middle"
    );
    assert_eq!(
        sub(places.position_at(72, 62)),
        (mid_x, mid_y),
        "it moved across the picture"
    );
    assert_eq!(
        sub(places.position_at(62, 72)),
        (mid_x, mid_y),
        "it moved down the picture"
    );
    // And its height is the picture's, which is the half of this the older test
    // covers — asserted here too so that the two stances are one comparison.
    assert!(
        height(places.position_at(62, 62)) > height(places.position_at(62, 72)),
        "a wall is not taller further up its picture",
    );

    // **A mobile, drawn with no volume exactly as the wall above, spreads.**
    //
    // `docs/lighting_rebuild.md` phase 7: a mobile has no volume by
    // construction rather than for want of a measurement, so it is a billboard
    // — a vertical plane through its tile's centre, turned towards the camera —
    // and a fragment of it is where its own view ray meets that plane. The
    // stanza above and this one are therefore *deliberately* different answers
    // to the same missing box, and the pair is the only thing that says the
    // pass tells the two apart. `impostor::billboard_at` carries the
    // derivation.
    //
    // What this is a gate on is not the arithmetic — `impostor.rs` states that
    // as its own two properties — but that a mobile's pixels stop sharing one
    // point. Sharing one was visible: a figure lit flat across, with horizontal
    // bands over it wherever `blit.wesl`'s `dither` gave one screen row one
    // turn of the sample spiral and the next row another.
    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        // Through the statics pass, which is the pass a mobile is really drawn
        // by: what makes it a mobile is the kind in its own `Place`, and that is
        // the word the branch under test reads.
        &[quad(Place::of_mobile(at), Range::default())],
        &[],
        &[],
        &[],
        128,
    );
    let (mid_x, mid_y) = sub(places.position_at(62, 62));
    let (right_x, right_y) = sub(places.position_at(72, 62));
    assert!(
        right_x > mid_x && right_y < mid_y,
        "a mobile's pixels do not move along the plane it is drawn on: \
         ({mid_x}, {mid_y}) at the middle against ({right_x}, {right_y}) ten pixels right",
    );
    // On the plane and not merely apart: the two coordinates move by the same
    // amount in opposite directions, which is what `x - y` means and is the one
    // direction a billboard's own plane runs along.
    assert!(
        ((right_x - mid_x) + (right_y - mid_y)).abs() < 1e-4,
        "({right_x}, {right_y}) is off the plane through ({mid_x}, {mid_y})",
    );
    // And straight down the picture is straight down the plane: the height moves
    // and the ground position does not.
    assert_eq!(
        sub(places.position_at(62, 72)),
        (mid_x, mid_y),
        "a mobile's pixel moved across the world going down its own picture",
    );
    assert!(
        height(places.position_at(62, 62)) > height(places.position_at(62, 72)),
        "a mobile is not taller further up its picture",
    );
}

/// A ground pixel decodes to `Stance::Flat`, read the same direct way the two
/// sibling tests above read a floor static's and a corner's.
///
/// `every_pixel_names_the_tile_it_came_from` already pins a land pixel's third
/// channel to the literal `384` — which is `128 | (STANCE_FLAT << 8)`, but
/// nothing at that call site says so, so a reader (or a future stance value
/// shifting the packing) cannot tell height and stance apart in it without doing
/// the arithmetic by hand. `docs/lighting_raymarch.md`'s backlog calls this out
/// by name: the fixture where session 23's bug — `ground.wgsl` never stamping a
/// stance at all — shipped unnoticed, because no test decoded the stance bits on
/// their own and compared them against the enum. This is that test, using the
/// same `place::STANCE_SHIFT` decode as `a_floor_spreads_across_its_tile_and_a_
/// wall_stands_up_it`.
#[test]
fn a_ground_pixel_carries_its_own_stance() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    let green = Color16(0b0_00000_11111_00000);

    let side = usize::from(LAND_TILE_SIZE);
    let land = LandAtlas::pack([(
        GRAPHIC,
        Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![green; side * side]),
    )])
    .expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([]).expect("nothing always fits");
    let region = land.region(GRAPHIC).expect("packed");

    let ground = [GroundQuad {
        x: 64.0,
        y: 64.0,
        corners: [0.0; 4],
        region,
        texmap: None,
        depth: 0.6,
        place: Place::land(300, 400),
    }];
    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &ground,
        &statics,
        &[],
        &[],
        &[],
        &[],
        128,
    );

    let stance = |x: u32, y: u32| {
        let word = places.at(x, y);
        assert_eq!(
            gbuffer::ids_kind(word),
            Some(Kind::Land),
            "nothing was drawn at ({x}, {y})",
        );
        gbuffer::ids_stance(word)
    };
    assert_eq!(
        stance(64, 64),
        openshard_client_render::place::Stance::Flat as u32,
        "a ground pixel does not carry its stance",
    );
}

/// Two wall tiles in a row are one surface, not two sprites.
///
/// The seam is the whole reason a wall's face is measured out of its art. Before
/// it, every pixel of a wall tile claimed the tile's middle: a row of walls came
/// out as flat 44-pixel bands with a step at each boundary, which is what a torch
/// against a wall actually looked like. With it, the fraction runs from 0 to 1
/// along the edge and the next tile picks up where this one stopped, so the world
/// coordinate a pixel names is *continuous across the join*.
///
/// Stated as a difference across the boundary and not as an absolute, because
/// that is the property that fails: a face read but mapped backwards, or a run
/// that saturates halfway, both put a wall in the right tiles and still break the
/// join. The pair of assertions below is what separates them — one that the step
/// across the seam is small, one that a whole tile is actually traversed, and a
/// mapping that returns a constant fails the second while passing the first.
#[test]
fn two_wall_tiles_in_a_row_name_one_continuous_surface() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    const HEIGHT: u16 = 60;
    // The south face runs along `+x`, so its neighbour is the tile at `x + 1` —
    // which in this projection is 22 pixels right and 22 down.
    const FACE: openshard_client_render::facing::Face = openshard_client_render::facing::Face::South;
    const ORIGIN: f32 = 20.0;

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([(GRAPHIC, openshard_client_render::facing::silhouette(FACE, HEIGHT))])
        .expect("fits");
    let sprite = statics.sprite(GRAPHIC).expect("packed");
    // The atlas is what measures the face, and if it did not this test would go
    // on to assert about two `Upright` sprites and pass for the wrong reason —
    // their fractions are equal, so the step across the seam would be zero.
    assert_eq!(
        sprite.facing,
        Some(openshard_client_render::facing::Facing::One(FACE)),
        "the atlas did not read the fixture",
    );

    let tile = |x: u16| Point::new(x, 400, 0);
    let quad = |at: Point, run: u32, dx: f32, dy: f32| SpriteQuad {
        rect: Rect {
            x: ORIGIN + dx,
            y: ORIGIN + dy,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.4,
        hue: 0,
        place: Place {
            stance: openshard_client_render::place::Stance::FaceSouth,
            ..Place::of_static(at)
        },
        twin: 0,
        owner: 0,
        // The `n`th tile's panel is the `n`th box, which is what a quad's own
        // range says and what makes each fragment meet its *own* wall.
        volumes: Range {
            offset: run,
            count: 1,
        },
    };
    // The two panels the grid stands for a run of wall: a slab on the south edge
    // of each tile, `PANEL_THICKNESS` deep into the tile it stands on, as tall as
    // the art — four screen pixels to a `z` unit, `camera::Z_STEP`. Through
    // `Solid::box_of`, which is the one place a kind becomes geometry.
    let panel = |x: u16| {
        Volume::of(
            &occlusion::Solid::box_of(
                i32::from(x),
                400,
                0,
                i32::from(HEIGHT) / openshard_client_render::camera::Z_STEP,
                openshard_client_render::occlusion::Edges::SOUTH,
            ),
            openshard_client_render::occlusion::Edges::SOUTH,
            None,
        )
    };
    let boxes = [panel(300), panel(301)];
    let quads = [quad(tile(300), 0, 0.0, 0.0), quad(tile(301), 1, 22.0, 22.0)];
    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &quads,
        &boxes,
        &[],
        &[],
        256,
    );

    // Where in the world a pixel says it is, in tiles. This is exactly what
    // `blit.wgsl` reads to measure a distance, which is why the assertions below
    // are about it rather than about the bits.
    let world_x = |x: u32, y: u32| {
        assert_eq!(
            gbuffer::ids_kind(places.at(x, y)),
            Some(Kind::Static),
            "nothing was drawn at ({x}, {y})",
        );
        places.position_at(x, y)[0]
    };

    // A row that crosses the join. The first sprite's face occupies its left
    // half — columns 0..=21 of a 44-wide picture — and the second sprite starts
    // 22 pixels right of it, so the two are edge to edge with no gap and no
    // overlap. A row a third of the way up the wall is drawn in every one of
    // those columns.
    let row = ORIGIN as u32 + u32::from(HEIGHT);
    let left = ORIGIN as u32;
    let last_of_the_first = world_x(left + 21, row);
    let first_of_the_second = world_x(left + 22, row + 1);
    assert!(
        (first_of_the_second - last_of_the_first).abs() < 0.15,
        "the seam steps: {last_of_the_first} then {first_of_the_second}",
    );
    // And a whole tile is crossed getting there, which is what says the fraction
    // is a mapping and not a constant somewhere near the middle.
    let first_of_the_first = world_x(left, row - 21);
    assert!(
        (last_of_the_first - first_of_the_first) > 0.85,
        "one tile of wall spans {} of a tile",
        last_of_the_first - first_of_the_first,
    );
    // The fixed coordinate is the edge, **exactly** — because a south panel's
    // camera-facing plane is `y + 1` and the impostor answers with the point
    // where the view ray leaves that box. A fraction that drifted off it would
    // put the lit surface inside the tile rather than on its boundary, which the
    // two assertions above cannot see because both only ever look at `x`.
    //
    // *Exactly*, and this is the claim that was re-taken at
    // `docs/lighting_rebuild.md` phase 6c. It used to be *one step short* of the
    // edge — `120/127`, produced by `statics.wgsl`'s `INSIDE` clamp — and the
    // reason was that `blit.wgsl` found a fragment's cell with
    // `floor(position)`, so a clean whole number named the tile beyond the wall
    // and the wall stopped being exempt from shadowing itself. Neither half of
    // that survives: the walk takes the cell from the tile the *instance*
    // carries (a whole number a pass knew exactly), and what a fragment is
    // exempt from is decided by primitive identity since phase 4. So the honest
    // number is the plane the geometry states, and this asserts it to the float
    // rather than to a step of a byte.
    for (x, y) in [(left, row - 21), (left + 21, row), (left + 22, row + 1)] {
        let point = places.position_at(x, y);
        // Which tile this pixel's own sprite stands on — off the row its id
        // names, which is `docs/gbuffer.md` step 3's whole point and the only
        // way to turn a position back into a fraction of a *named* tile.
        let stands = quads[gbuffer::ids_id(places.at(x, y)) as usize].place;
        let sub_y = point[1] - f32::from(stands.y);
        assert!(
            (sub_y - 1.0).abs() < 1e-5,
            "the south face is not on its own edge at ({x}, {y}): {sub_y}"
        );
    }
}

/// A corner's two faces are two halves of one picture, and a pixel belongs to
/// the one it is drawn on.
///
/// The whole of decision 25 on the GPU side: `statics.wgsl` resolves a corner per
/// fragment, so what reaches the attachment is a single face with a single
/// outward normal and `blit.wgsl` never learns that corners exist. Before it, a
/// corner was `Upright` — every pixel of it claiming the middle of its tile, a
/// flat 44-pixel band between two continuous runs of wall, lit identically on the
/// side turned towards the flame and the side turned away.
///
/// Asserted as the *two halves disagreeing*, which is the property that fails:
/// a corner resolved to one face for the whole tile draws a wall along an axis
/// half of it does not run on, and every pixel would still carry a face and a
/// fraction that moves. Only comparing the halves says which face is where.
#[test]
fn a_corner_s_pixel_carries_the_face_of_the_half_it_is_drawn_on() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    use openshard_client_render::facing::{Face, Facing};
    use openshard_client_render::place::Stance;

    const GRAPHIC: Graphic = Graphic(1);
    const HEIGHT: u16 = 60;
    const ORIGIN: f32 = 20.0;
    // The pair a camera can see, which is what every corner the client ships is:
    // the east face on the right half of the picture, the south face on the left.
    const RIGHT: Face = Face::East;
    const LEFT: Face = Face::South;

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([(
        GRAPHIC,
        openshard_client_render::facing::corner_silhouette(RIGHT, LEFT, HEIGHT),
    )])
    .expect("fits");
    let sprite = statics.sprite(GRAPHIC).expect("packed");
    // The atlas is what measures it, and without this the test would go on to
    // assert about an `Upright` sprite whose halves agree — and pass by drawing
    // exactly the artefact it is here to catch.
    assert_eq!(
        sprite.facing,
        Some(Facing::Corner {
            right: RIGHT,
            left: LEFT
        }),
        "the atlas did not read the fixture as a corner",
    );

    let at = Point::new(300, 400, 0);
    // A corner is two panels, and the grid pushes them one at a time — so the
    // fragment is met against both and the ray decides which it is a pixel of.
    // Through `Solid::box_of` for the same reason the wall-run test above does.
    let corner = [RIGHT, LEFT].map(|face| {
        Volume::of(
            &occlusion::Solid::box_of(
                i32::from(at.x),
                i32::from(at.y),
                0,
                i32::from(HEIGHT) / openshard_client_render::camera::Z_STEP,
                openshard_client_render::occlusion::edges_of(Some(Facing::One(face))),
            ),
            openshard_client_render::occlusion::edges_of(Some(Facing::One(face))),
            None,
        )
    });
    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &[SpriteQuad {
            rect: Rect {
                x: ORIGIN,
                y: ORIGIN,
                width: f32::from(sprite.width),
                height: f32::from(sprite.height),
            },
            region: sprite.region,
            depth: 0.4,
            hue: 0,
            place: Place {
                stance: Stance::of(&openshard_tiles::StaticTile::default(), sprite.facing),
                ..Place::of_static(at)
            },
            twin: 0,
            owner: 0,
            volumes: Range { offset: 0, count: 2 },
        }],
        &corner,
        &[],
        &[],
        256,
    );

    // A row a third of the way up the picture, one pixel either side of the
    // column the sprite is centred on: the two halves of the same corner, as
    // close together as the picture allows.
    let row = ORIGIN as u32 + u32::from(HEIGHT);
    let middle = ORIGIN as u32 + 22;
    let stance = |x: u32, y: u32| {
        let word = places.at(x, y);
        assert_eq!(
            gbuffer::ids_kind(word),
            Some(Kind::Static),
            "nothing was drawn at ({x}, {y})",
        );
        gbuffer::ids_stance(word)
    };
    assert_eq!(
        stance(middle + 4, row),
        Stance::FaceEast as u32,
        "the right half of the corner is not its east face",
    );
    assert_eq!(
        stance(middle - 4, row),
        Stance::FaceSouth as u32,
        "the left half of the corner is not its south face",
    );

    // And where in its tile each half sits is its own face's: an east face lies
    // on `x + 1` and a south face on `y + 1`, so the two halves are two
    // different surfaces of one tile and not one surface read twice. Compared as
    // the *fixed* coordinate of each, because that is what a face is — the run
    // along the edge moves in both.
    let sub = |x: u32, y: u32| {
        let point = places.position_at(x, y);
        (point[0] - f32::from(at.x), point[1] - f32::from(at.y))
    };
    let (right_x, _) = sub(middle + 4, row);
    let (_, left_y) = sub(middle - 4, row);
    // On the edge exactly, for the reason the wall-run test above states at
    // length: the plane is the box's, and `INSIDE`'s step short of it is gone
    // with the clamp that produced it.
    assert!(
        (right_x - 1.0).abs() < 1e-5,
        "the east half is off its own edge: {right_x}"
    );
    assert!(
        (left_y - 1.0).abs() < 1e-5,
        "the south half is off its own edge: {left_y}"
    );

    // And the two halves are two different rows, not one instance's id read
    // twice — `docs/gbuffer.md` step 4. This frame drew exactly one corner,
    // so `split_corners` gives it id `0` and a shadow row at id `1`: the
    // right half (its own instance) keeps `0`, the left half (the diagonal
    // test's other side) takes the shadow's `1`.
    let id = |x: u32, y: u32| gbuffer::ids_id(places.at(x, y));
    assert_eq!(id(middle + 4, row), 0, "the right half is not the drawn instance");
    assert_eq!(id(middle - 4, row), 1, "the left half is not its shadow row");
}

/// `docs/lighting_rebuild.md` phase 7: a mobile has no volume, so before this
/// its normal was the zero vector — "lit from every side", the flatness a
/// person reported beside a torch. The plane it is drawn on has exactly one
/// normal, and this is the gate that the shader writes *that* one rather than
/// the zero vector or some other guess: the word the GPU wrote, checked
/// against `impostor::billboard_normal` packed on this side, exactly, the
/// same shape `two_mesh_faces_carry_their_own_two_normals` and
/// `a_sprite_pixel_meets_the_same_box_on_both_sides` already hold their own
/// producers to.
#[test]
fn a_billboards_normal_is_the_plane_it_is_drawn_on() {
    let Some((device, queue)) = gpu() else {
        return;
    };

    const GRAPHIC: Graphic = Graphic(1);
    const SIZE: u16 = 20;
    const ORIGIN: f32 = 20.0;

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([(
        GRAPHIC,
        Image::new(
            SIZE,
            SIZE,
            vec![Color16(0b0_00000_11111_00000); usize::from(SIZE) * usize::from(SIZE)],
        ),
    )])
    .expect("fits");
    let sprite = statics.sprite(GRAPHIC).expect("packed");

    let at = Point::new(300, 400, 0);
    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &[SpriteQuad {
            rect: Rect {
                x: ORIGIN,
                y: ORIGIN,
                width: f32::from(sprite.width),
                height: f32::from(sprite.height),
            },
            region: sprite.region,
            depth: 0.4,
            hue: 0,
            place: Place::of_mobile(at),
            twin: 0,
            owner: 0,
            volumes: Range::default(),
        }],
        &[],
        &[],
        &[],
        256,
    );

    let x = ORIGIN as u32 + u32::from(SIZE) / 2;
    let y = ORIGIN as u32 + u32::from(SIZE) / 2;
    assert_eq!(
        gbuffer::ids_kind(places.at(x, y)),
        Some(Kind::Mobile),
        "nothing was drawn at ({x}, {y})",
    );
    // Not the exact word, unlike the corner and mesh-face gates beside this
    // one: those compare *cardinal* faces, and `NORMAL_AXIS_SPAN`'s evenness
    // is what buys a cardinal a bit-for-bit round trip. `(1, 1, 0)` is a
    // diagonal on the octahedral map's own equator — the fold this crate's
    // own sweep already measured — so `normalize`'s GPU and CPU paths land a
    // quantisation step apart on `z` alone (`8.6e-5` here, both sides reading
    // `0.0` in every digit a person would type). The bound is
    // `a_direction_survives_the_normal_packing`'s own: `0.01°` is what a
    // channel can show.
    let angle = |a: [f32; 3], b: [f32; 3]| {
        let chord: f64 = (0..3)
            .map(|i| {
                let d = f64::from(a[i]) - f64::from(b[i]);
                d * d
            })
            .sum::<f64>()
            .sqrt();
        2.0 * (chord / 2.0).clamp(-1.0, 1.0).asin().to_degrees()
    };
    let gpu = gbuffer::unpack_normal(places.normal_at(x, y));
    let cpu = gbuffer::unpack_normal(gbuffer::pack_normal(
        openshard_client_render::impostor::billboard_normal(),
    ));
    let off = angle(gpu, cpu);
    assert!(
        off < 0.01,
        "a billboard's normal is {off}° off the plane it is drawn on: {gpu:?} vs {cpu:?}",
    );
}

/// **The mob anchor defect, closed.** [`mobiles::cell_centre`] places a
/// walking body's screen rect at its real, fractional drawn position, but
/// [`Place::of_mobile`] carries only the whole tile it is arriving at — and
/// before `crate::mobiles::billboard_offset` existed, `impostor.wesl`'s
/// `billboard_at` read light from *that* tile, up to a whole tile short of
/// where the sprite actually was. This walks a body through five points of
/// one step and checks, at each, that the position plane's own `(x, y)`
/// agrees with the fraction `camera::unproject_ground` reads out of
/// [`Mobile::drawn`] directly — the GPU's own arithmetic against this crate's,
/// the same shape [`a_billboards_normal_is_the_plane_it_is_drawn_on`] already
/// holds the normal to.
#[test]
fn a_walking_billboard_is_lit_where_it_is_drawn_not_where_it_is_going() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const BODY: u16 = 400;
    // Odd and small: `center_x` lands exactly on `SIZE / 2`, which is what
    // makes the sprite's screen anchor (`middle_x`) exactly `cell_centre.x` —
    // see `mobiles::place`'s own doc on the anchor. A one-pixel image keeps
    // `across` inside a single fragment's width so the readback names one
    // texel unambiguously.
    const SIZE: u16 = 8;

    let frame = AnimFrame {
        center_x: (SIZE / 2) as i16,
        center_y: 0,
        image: Image::new(
            SIZE,
            SIZE,
            vec![Color16(0b0_00000_11111_00000); usize::from(SIZE) * usize::from(SIZE)],
        ),
    };
    let atlas = AnimAtlas::pack([(
        FrameKey::new(
            AnimationKey::new(Graphic(BODY), AnimationGroup(4), AnimationDirection(0)),
            AnimationFrameIndex(0),
        ),
        frame,
    )])
    .expect("one frame fits");

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([]).expect("nothing always fits");

    let from = Point::new(300, 400, 0);
    let to = Point::new(301, 400, 0);
    let camera = Camera::new(from, 256, 256);

    for left in [1.0, 0.75, 0.5, 0.25, 0.0] {
        let drawn = openshard_client_render::follow::Gaze::on(to)
            .back_towards(openshard_client_render::follow::Gaze::on(from), left);
        let mobile = Mobile {
            at: to,
            body: Graphic(BODY),
            group: AnimationGroup(4),
            // `SouthEast` is the one facing stored unmirrored at direction
            // `0` (`anim::facing`), which keeps the anchor at `center_x`
            // rather than `width - center_x` — this test's own arithmetic
            // for `middle_x` assumes the unmirrored case.
            facing: Direction::SouthEast,
            frame: AnimationFrameIndex(0),
            from: Some(from),
            corpse: false,
            hue: openshard_protocol::wire::Hue::NONE,
            drawn,
            equipment: Vec::new().into(),
        };
        let quads = mobiles::collect(
            &[mobile],
            &camera,
            &atlas,
            &Cutaway::OPEN,
            &EquipConv::default(),
            None,
        );
        assert_eq!(quads.len(), 1, "the frame is packed, so it draws");
        let quad = &quads[0];

        let places = render_places_with_mobile(
            &device,
            &queue,
            &land,
            &texmaps,
            &[],
            &statics,
            &[],
            atlas.pixels(),
            std::slice::from_ref(quad),
            256,
        );

        // The pixel this crate's own `middle_x` — `quad.rect.x +
        // quad.rect.width / 2`, exact for this frame's centred `center_x` —
        // lands nearest, so `across` at the point sampled is small and known
        // rather than assumed zero.
        let middle_x = quad.rect.x + quad.rect.width / 2.0;
        let x = middle_x.round() as u32;
        let y = (quad.rect.y + quad.rect.height / 2.0).round() as u32;
        assert_eq!(
            gbuffer::ids_kind(places.at(x, y)),
            Some(Kind::Mobile),
            "nothing was drawn at ({x}, {y}), left = {left}",
        );

        let across = (x as f32 + 0.5) - middle_x;
        let (tx, ty) = openshard_client_render::camera::unproject_ground(drawn.x, drawn.y);
        let tile_width = openshard_client_render::camera::TILE_WIDTH as f32;
        let expected_x = tx as f32 + 0.5 + across / tile_width;
        let expected_y = ty as f32 + 0.5 - across / tile_width;

        let point = places.position_at(x, y);
        assert!(
            (point[0] - expected_x).abs() < 1e-4,
            "left = {left}: x is {} but the drawn position names {expected_x}",
            point[0],
        );
        assert!(
            (point[1] - expected_y).abs() < 1e-4,
            "left = {left}: y is {} but the drawn position names {expected_y}",
            point[1],
        );
    }
}

/// The impostor is written twice — `impostor.wesl` and [`impostor`] — and this
/// is the only thing that compares them.
///
/// `docs/lighting_rebuild.md` phase 6c, and the same argument
/// `normal_format.wesl`'s own gate makes one plane down: two spellings of one
/// arithmetic have no compiler between them, and every reader downstream sees
/// only the answer, so a disagreement about *which box* or *which face* would
/// show as a picture somebody eventually calls wrong. What is asserted is
/// therefore not "the position looks reasonable" but **the word the GPU wrote
/// equals what this side answers for the same ray and the same boxes** — the
/// normal as an integer, exactly, and the point to a ten-thousandth of a tile,
/// which is a hundredth of a screen pixel.
///
/// Swept over the sprite's whole rectangle rather than at three chosen points,
/// because what a second spelling gets wrong is a *case*: a tie between two exit
/// faces, the fold at a corner, the box in front. A sweep meets all of them and
/// counts what it met, so the test says how much of the arithmetic it reached
/// instead of leaving that to a reader.
///
/// **And it is where the phase's second number is taken.** `Meeting::outside` is
/// how far outside its own volume a fragment fell — the sprite overhanging the
/// boxes, which is the thing `WIDTH_OVERLAP` used to hide — and this fixture's
/// art is a plain rectangle that deliberately overhangs on every side, so the
/// count is large and the *bound* is the claim: no fragment is answered with a
/// point more than a tile away from the shape it belongs to.
#[test]
fn a_sprite_pixel_meets_the_same_box_on_both_sides() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    use openshard_client_render::impostor;

    const GRAPHIC: Graphic = Graphic(1);
    const WIDE: u16 = 44;
    const TALL: u16 = 105;
    const ORIGIN: f32 = 10.0;
    const SIZE: u32 = 128;

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let red = Color16(0b0_11111_00000_00000);
    // Opaque everywhere, so that every texel of the quad is a fragment: the
    // sweep wants the whole rectangle, including the pixels that miss.
    let statics = StaticAtlas::pack([(
        GRAPHIC,
        Image::new(WIDE, TALL, vec![red; usize::from(WIDE) * usize::from(TALL)]),
    )])
    .expect("fits");
    let sprite = statics.sprite(GRAPHIC).expect("packed");

    let at = Point::new(300, 400, 0);
    // Two boxes on one tile, one in front of the other and each a strip of it —
    // a flight of two treads, which is the shape with a *selection* in it: the
    // near tread hides the lower half of the far one, so the sweep crosses both
    // a lid and the front of a rise, and `nearest`'s "the box in front wins" is
    // load-bearing rather than decorative. Written out rather than taken from
    // `Solid::box_of` because what is under test here is the arithmetic and not
    // the grid's own shapes — the two tests above are where those are stated.
    //
    // **Three distinct names**, which is the other half of what the sweep
    // compares since `solid_format.wesl`: a fragment carries the name of the box
    // it was met against, so ids that were all `NOBODY` (or all equal) would let
    // a pass that always answered with the *first* box pass this test — which is
    // exactly the defect that shipped when phase 6d took the mesh pass off real
    // statics and left `blit.wesl` narrowing an owner by a stance. Arbitrary and
    // non-consecutive on purpose: nothing here may pass by counting.
    //
    // **And the masks are the ones the grid would hand these shapes**, because
    // the mask is what decides whether a fragment takes the met face as a facing
    // at all — `impostor::Volume::edges`. A tread is `Edges::ANY`, which
    // `boxes_of` says outright ("one box a tread, in climb order, and it is a
    // **body**"); the lid below names no side. So this sweep carries both sides
    // of that rule over every texel of the quad, and the *face* itself is still
    // compared everywhere through the stance, which is the met face whatever the
    // art named.
    let boxes = [
        Volume {
            lo: WorldVec::new(300.0, 400.5, 0.0),
            hi: WorldVec::new(301.0, 401.0, 3.0),
            solid: Some(occlusion::SolidId::new(7)),
            edges: occlusion::Edges::ANY,
        },
        Volume {
            lo: WorldVec::new(300.0, 400.0, 0.0),
            hi: WorldVec::new(301.0, 400.5, 6.0),
            solid: Some(occlusion::SolidId::new(11)),
            edges: occlusion::Edges::ANY,
        },
        // And a lid, flat: `lo.z == hi.z`, which is what the grid stands for a
        // floor and the one shape whose `z` slab is a point — so the sweep runs
        // the degenerate path, where a division answers one `t` twice, on both
        // sides. Not the *tie* between two exits, which is a line across this
        // box and not an area: a sweep of whole pixels reaches it only by luck,
        // and `impostor::tests::a_lid_with_no_thickness_is_met_on_its_own_plane`
        // is where that case is constructed rather than hoped for.
        Volume {
            lo: WorldVec::new(300.0, 400.0, 9.0),
            hi: WorldVec::new(301.0, 401.0, 9.0),
            solid: Some(occlusion::SolidId::new(13)),
            edges: occlusion::Edges::NONE,
        },
    ];
    let quads = [SpriteQuad {
        rect: Rect {
            x: ORIGIN,
            y: ORIGIN,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.4,
        hue: 0,
        place: Place::of_static(at),
        twin: 0,
        owner: 0,
        volumes: Range {
            offset: 0,
            count: boxes.len() as u32,
        },
    }];
    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &quads,
        &boxes,
        &[],
        &[],
        SIZE,
    );

    // Where the fragment stage's own two numbers come from, restated here and
    // nowhere else: `across` is the offset from the column the sprite is centred
    // on and `down` the offset from the row the tile's centre projects to, both
    // measured at the **fragment's centre**, which is half a texel past its
    // corner. A half-pixel error here would move every comparison below by a
    // fortieth of a tile and none of them would pass.
    let middle_x = ORIGIN + f32::from(sprite.width) * 0.5;
    let bottom_y = ORIGIN + f32::from(sprite.height);
    let half_tile_height = (openshard_client_render::camera::TILE_WIDTH / 2) as f32;

    let (mut compared, mut outside, mut worst) = (0u32, 0u32, 0.0f32);
    let mut faces = [0u32; 3];
    // **The sprite's own rectangle, not "wherever a static was drawn"** — which
    // is the difference the discard makes and the whole of what this sweep now
    // states. A texel whose ray misses every box is no longer drawn at all
    // (`statics.wesl`, and the amendment in `docs/lighting_rebuild.md`'s "One
    // silhouette"), so filtering on `Kind::Static` would quietly skip exactly the
    // pixels the rule is about and the test would agree with any rule at all.
    // Walked as the quad's own texels instead, and each one is asked the
    // question the shader was asked.
    for y in ORIGIN as u32..ORIGIN as u32 + u32::from(TALL) {
        for x in ORIGIN as u32..ORIGIN as u32 + u32::from(WIDE) {
            let across = x as f32 + 0.5 - middle_x;
            let down = y as f32 + 0.5 - (bottom_y - half_tile_height);
            let start = impostor::ray_from((i32::from(at.x), i32::from(at.y)), f32::from(at.z), across, down);
            let (which, met) = impostor::nearest(
                start,
                boxes
                    .iter()
                    .enumerate()
                    .map(|(n, volume)| (n, volume.lo, volume.hi)),
            )
            .expect("two boxes");

            // **A miss is answered by the box it came nearest, exactly as a hit
            // is** — so every assertion below is asked of every texel, and
            // `outside` is a *count* rather than a branch.
            //
            // Three states in three commits, and this is the third. The clamp
            // was replaced by `discard` because it handed a fragment whichever
            // face exits first, which along a silhouette is a side one: a
            // lattice of wall-shaded dots on floors and roofs. `discard` was
            // replaced by "a measurement that is missing" — the tile's centre and
            // the zero normal — because it threw away 11.09% of every panel's art
            // and 32.44% of every whole-tile one, a display case losing its whole
            // top. And that state is *lit from every side*, so it draws brighter
            // than the measured surface beside it: on a floor it was a dashed
            // glowing line along every tile seam.
            //
            // The clamp is back because its own defect is now cured at the root
            // rather than avoided — `impostor::shows_a_side` refuses a face
            // thinner than the grid that reads it, so a flat box answers with its
            // lid all the way to its rim and there is no side face to hand out.
            // What is left of "no measurement" is the honest case: a static the
            // grid holds no boxes for at all, which this fixture is not.
            if !met.hit() {
                outside += 1;
                worst = worst.max(met.outside);
            }
            assert_eq!(
                gbuffer::ids_kind(places.at(x, y)),
                Some(Kind::Static),
                "({x}, {y}) was not drawn",
            );

            // **The face the ray met, compared on every fragment** — through
            // the stance, which is what the pass writes it to. It is the met
            // face whether or not the art named that side, so this is the
            // CPU-against-GPU claim about `meets`'s own arithmetic and it is
            // asked of every texel of the quad.
            assert_eq!(
                gbuffer::ids_stance(places.at(x, y)),
                openshard_client_render::place::Stance::of_normal(met.normal.array())
                    .expect("a met face is axis-aligned") as u32,
                "({x}, {y}) met a different face on the GPU: {met:?}",
            );
            // **And the facing it claims from that face, which is the rule.** A
            // box the art named a side of writes the face; a **body** writes
            // none, because the box it met is the tile's own walls rather than a
            // plane anybody drew. `impostor::Volume::edges` carries the
            // argument, and injecting the fault — writing the face for a body
            // too — turns this red on every fragment of the two treads.
            let facing = match boxes[which].edges == occlusion::Edges::ANY {
                true => [0.0; 3],
                false => met.normal.array(),
            };
            assert_eq!(
                places.normal_at(x, y),
                gbuffer::pack_normal(facing),
                "({x}, {y}) claims a facing its box does not give it: {met:?}",
            );
            let point = places.position_at(x, y);
            let met_at = met.at.array();
            for (axis, name) in [(0, 'x'), (1, 'y'), (2, 'z')] {
                assert!(
                    (point[axis] - met_at[axis]).abs() < 1e-4,
                    "({x}, {y}) landed elsewhere on {name}: {point:?} against {:?}",
                    met.at,
                );
            }
            // And whatever it answered is a point *of the box it named* — the
            // property every reader downstream leans on, and the one the
            // nearest-point fallback exists to keep. Stated as containment
            // rather than as a bound on `outside`, because a bound is a number
            // somebody would have to pick and this is the claim itself.
            let volume = &boxes[which];
            let (volume_lo, volume_hi) = (volume.lo.array(), volume.hi.array());
            for axis in 0..3 {
                assert!(
                    met_at[axis] >= volume_lo[axis] - 1e-5 && met_at[axis] <= volume_hi[axis] + 1e-5,
                    "({x}, {y}) was answered off its own box on axis {axis}: {:?}",
                    met.at,
                );
            }
            // **And it carries that box's own name**, which is what the shadow
            // walk compares and the whole of `solid_format.wesl`. A `SolidId` is
            // three bytes and an `f32` holds every integer to twenty-four, so
            // this is an equality and not a tolerance — the same standard the
            // normal above is held to.
            assert_eq!(
                point[3],
                gbuffer::pack_solid(occlusion::SolidId::word(volume.solid)),
                "({x}, {y}) is a point of box {which} and says it is a point of {}",
                gbuffer::unpack_solid(point[3]),
            );
            compared += 1;
            faces[met
                .normal
                .array()
                .iter()
                .position(|n| *n == 1.0)
                .expect("one axis")] += 1;
        }
    }

    // What the sweep actually reached, printed rather than left to a reader: a
    // pass that agreed about one face on four pixels would satisfy every
    // assertion above.
    eprintln!(
        "{compared} fragments compared — {} on an east face, {} on a south face, {} on a lid; \
         {outside} of them fell outside their own volume, the worst by {worst} of a tile",
        faces[0], faces[1], faces[2],
    );
    // **Every texel of the quad was compared**, and that is the assertion the
    // clamp's return makes possible: a static the grid holds boxes for has no
    // unanswered pixels at all now, so this is the whole rectangle rather than
    // a partition of it. A rule that drew nothing fails here; so does one that
    // left a fragment unmeasured and hoped nobody swept it.
    assert_eq!(
        compared,
        u32::from(WIDE) * u32::from(TALL),
        "every texel of an opaque sprite is a point of one of its boxes",
    );
    assert!(
        faces.iter().all(|seen| *seen > 100),
        "the sweep should meet all three of a box's camera-facing sides: {faces:?}",
    );
    // And that the miss case was reached at all — the positive control for
    // "answered by the nearest box" being a rule this sweep actually exercised,
    // which it says nothing about on a fixture where every texel hits. **How
    // far** a real static's art overhangs its own fitted prism is the number
    // phase 6's own "done when" asks for, and it is not this: this fixture's
    // picture is a plain rectangle nobody fitted to anything, so its overhang is
    // a property of the fixture — and here it is nearly half the sprite.
    assert!(outside > 0, "a rectangle over two strips should overhang them");
}

/// **The fringe switch draws three different frames**, which is the one thing a
/// knob for looking at frames has to do and the one thing nothing else here
/// would catch.
///
/// `impostor::Fringe` is a debug state in the sense `debug::View` is: two of its
/// three answers were measured and refused as defaults
/// (`docs/lighting_state.md`'s fringe entry), and they are kept reachable
/// because refusing them by argument is what this backlog item had already done
/// twice. A switch wired to nothing would leave every one of those pictures
/// identical to the shipped one and read as "I looked and saw no difference" —
/// so this asserts the difference exists, in the direction each state claims:
///
/// - `Discard` draws **strictly fewer** pixels, and every pixel it does draw is
///   one `Clamp` drew — it removes the fringe rather than moving it.
/// - `Volume` draws **exactly** the same pixels and gives some of them a
///   different facing — it moves the normal and nothing else.
///
/// The fixture is the sibling sweep's: a plain rectangle over three boxes it
/// overhangs, so there are misses to argue about at all.
#[test]
fn the_fringe_switch_draws_three_different_frames() {
    let Some((device, queue)) = gpu() else {
        return;
    };

    const GRAPHIC: Graphic = Graphic(1);
    const WIDE: u16 = 44;
    const TALL: u16 = 105;
    const ORIGIN: f32 = 10.0;
    const SIZE: u32 = 128;

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let red = Color16(0b0_11111_00000_00000);
    let statics = StaticAtlas::pack([(
        GRAPHIC,
        Image::new(WIDE, TALL, vec![red; usize::from(WIDE) * usize::from(TALL)]),
    )])
    .expect("fits");
    let sprite = statics.sprite(GRAPHIC).expect("packed");
    let at = Point::new(300, 400, 0);
    // A panel thin in `y` and a body over it: two boxes whose *presented* faces
    // differ from each other and from the lid a clamp mostly names, so `Volume`
    // has something to change. See `impostor::presented_face`.
    //
    // The panel names the side it stands on and the body names all four, which
    // is `edges_of`'s way of saying none — so only the panel's own fragments can
    // show a *facing* changing at all, and that is what the `volume` row below
    // counts. See `impostor::Volume::edges`.
    let boxes = [
        Volume {
            lo: WorldVec::new(300.0, 400.8, 0.0),
            hi: WorldVec::new(301.0, 401.0, 12.0),
            solid: Some(occlusion::SolidId::new(7)),
            edges: occlusion::Edges::SOUTH,
        },
        Volume {
            lo: WorldVec::new(300.0, 400.0, 0.0),
            hi: WorldVec::new(301.0, 401.0, 3.0),
            solid: Some(occlusion::SolidId::new(11)),
            edges: occlusion::Edges::ANY,
        },
    ];
    let quads = [SpriteQuad {
        rect: Rect {
            x: ORIGIN,
            y: ORIGIN,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.4,
        hue: 0,
        place: Place::of_static(at),
        twin: 0,
        owner: 0,
        volumes: Range {
            offset: 0,
            count: boxes.len() as u32,
        },
    }];

    let frame = |fringe| {
        render_places_with_fringe(
            &device,
            &queue,
            &land,
            &texmaps,
            &[],
            &statics,
            &quads,
            &boxes,
            &[],
            &[],
            SIZE,
            fringe,
        )
    };
    let clamped = frame(Fringe::Clamp);
    let dropped = frame(Fringe::Discard);
    let volume = frame(Fringe::Volume);

    let (mut drawn, mut gone, mut appeared, mut turned, mut lost) = (0u32, 0u32, 0u32, 0u32, 0u32);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let is_static = |places: &Places| {
                gbuffer::ids_kind(places.at(x, y)) == Some(openshard_client_render::place::Kind::Static)
            };
            let (here, without, presented) = (is_static(&clamped), is_static(&dropped), is_static(&volume));
            drawn += u32::from(here);
            gone += u32::from(here && !without);
            appeared += u32::from(!here && without);
            lost += u32::from(here != presented);
            if here && presented && clamped.normal_at(x, y) != volume.normal_at(x, y) {
                turned += 1;
            }
        }
    }

    eprintln!(
        "{drawn} static fragments under the clamp; discard removes {gone} and adds {appeared}; \
         the volume's face turns {turned} of them",
    );
    assert!(drawn > 0, "the fixture drew nothing at all");
    assert!(
        gone > 0,
        "OPENSHARD_FRINGE=discard drew the same picture as the clamp — the switch reaches nothing",
    );
    assert_eq!(
        appeared, 0,
        "discarding a miss cannot make a fragment appear: {appeared} did",
    );
    assert_eq!(
        lost, 0,
        "the volume's face changes a normal and never whether a pixel is drawn: {lost} moved",
    );
    assert!(
        turned > 0,
        "OPENSHARD_FRINGE=volume gave every fringe fragment the clamp's own face — \
         either the switch reaches nothing or `presented_face` agrees everywhere here",
    );
}

/// A real static's fragment is a point of the primitive it names: its position
/// on that primitive's own boundary, its normal one of that primitive's
/// camera-facing faces, and its stance that face's own — `docs/lighting_
/// rebuild.md` phase 6i, item 3.
///
/// Position, normal, solid and stance are not four independent measurements
/// once a static has a shape behind it: three of them are properties of one
/// box, and nothing before this compared them against each other — each
/// producer was checked against its own arithmetic
/// (`a_sprite_pixel_meets_the_same_box_on_both_sides` against
/// `impostor::nearest`, `a_direction_survives_the_normal_packing` against
/// `pack_normal`) and never against a sibling plane. 6f, 6g and 6h were each a
/// fragment whose plane was not its primitive's own number, and every one of
/// them was found by a person looking at a lit frame rather than by a test —
/// this is the sweep that would have caught each, over the one shape none of
/// this crate's other GPU fixtures drives through the sprite path at all: a
/// merged run of wall (`docs/occluders.md`'s D6, 6h's own bill), a fitted
/// climbable (6f's own shape), a corner (6g's — `occlusion::boxes_of`'s own
/// doc is why a stair's base reads as one), a lone wall panel and a floor.
#[test]
fn a_sprite_fragment_is_a_point_of_the_primitive_it_names() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    use openshard_client_render::facing::{Face, Facing, Prism};
    use openshard_client_render::place::Stance;

    const SIZE: u32 = 640;
    const GRAPHIC: Graphic = Graphic(1);
    const WIDE: u16 = 70;
    const TALL: u16 = 160;

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let red = Color16(0b0_11111_00000_00000);
    // Opaque everywhere, for the same reason the sibling sweep's is: every
    // texel of every rectangle below has to be a fragment.
    let statics = StaticAtlas::pack([(
        GRAPHIC,
        Image::new(WIDE, TALL, vec![red; usize::from(WIDE) * usize::from(TALL)]),
    )])
    .expect("fits");
    let sprite = statics.sprite(GRAPHIC).expect("packed");

    let bounds = TileBounds {
        min_x: 190,
        max_x: 250,
        min_y: 190,
        max_y: 210,
    };
    let mut builder = Builder::new(bounds);

    let wall_tile = StaticTile {
        flags: TileFlags::new(TileFlags::NO_SHOOT),
        height: 4,
        ..StaticTile::default()
    };
    // The merged run: three tiles of one graphic, one owner, so
    // `occlusion::merge` folds them into a single primitive.
    const RUN_GRAPHIC: Graphic = Graphic(10);
    for x in 200..203u16 {
        builder.add(
            x,
            200,
            0,
            RUN_GRAPHIC,
            &wall_tile,
            Shape::faced(Facing::One(Face::South)),
        );
    }
    // A fitted climbable, three treads under one owner.
    const STAIR_GRAPHIC: Graphic = Graphic(11);
    let stair_tile = StaticTile {
        flags: TileFlags::new(TileFlags::CLIMBABLE | TileFlags::BLOCK | TileFlags::NO_SHOOT),
        height: 5,
        ..StaticTile::default()
    };
    let prism = Prism::new(Face::North, &[1, 3, 5]).expect("three treads is a legal profile");
    builder.add(210, 200, 0, STAIR_GRAPHIC, &stair_tile, Shape::solid(prism));
    // A corner: two panels of one picture.
    const CORNER_GRAPHIC: Graphic = Graphic(12);
    builder.add(
        220,
        200,
        0,
        CORNER_GRAPHIC,
        &wall_tile,
        Shape::faced(Facing::Corner {
            right: Face::East,
            left: Face::South,
        }),
    );
    // A lone wall panel, unmerged: its own graphic, no neighbour to join.
    const LONE_WALL_GRAPHIC: Graphic = Graphic(13);
    builder.add(
        230,
        200,
        0,
        LONE_WALL_GRAPHIC,
        &wall_tile,
        Shape::faced(Facing::One(Face::East)),
    );
    // A floor: a lid, flat at its own height.
    const FLOOR_GRAPHIC: Graphic = Graphic(14);
    let floor_tile = StaticTile {
        flags: TileFlags::new(TileFlags::FLOOR | TileFlags::NO_SHOOT),
        height: 0,
        ..StaticTile::default()
    };
    builder.add(240, 200, 0, FLOOR_GRAPHIC, &floor_tile, Shape::UNREAD);

    let grid = builder.finish(&Cutaway::OPEN);

    // Every static's own boxes, named through the grid exactly as
    // `statics::push_volumes` does — `boxes_of` for the shape, `id_of` for the
    // grid's own name of it. Restated rather than called: `push_volumes` is
    // `pub(crate)`, and this file is outside the crate that defines it.
    fn push_boxes(
        boxes: &mut Vec<Volume>,
        grid: &Occlusion,
        x: u16,
        y: u16,
        graphic: Graphic,
        tile: &StaticTile,
        shape: &Shape,
    ) -> Range {
        let offset = boxes.len() as u32;
        let owner = occlusion::Owner::new(0, graphic);
        occlusion::boxes_of(
            i32::from(x),
            i32::from(y),
            0,
            tile,
            shape,
            |part, edges, space| {
                let named = grid.id_of(i32::from(x), i32::from(y), owner, part);
                let space = match named {
                    Some(id) => grid.solid(id).space,
                    None => space,
                };
                boxes.push(Volume::of(&space, edges, named));
            },
        );
        Range {
            offset,
            count: boxes.len() as u32 - offset,
        }
    }

    let mut boxes: Vec<Volume> = Vec::new();
    let mut quads = Vec::new();
    let mut origin_x = 10.0f32;
    let mut place =
        |x: u16, graphic: Graphic, tile: &StaticTile, shape: &Shape, quads: &mut Vec<SpriteQuad>| {
            let range = push_boxes(&mut boxes, &grid, x, 200, graphic, tile, shape);
            quads.push(SpriteQuad {
                rect: Rect {
                    x: origin_x,
                    y: 20.0,
                    width: f32::from(sprite.width),
                    height: f32::from(sprite.height),
                },
                region: sprite.region,
                depth: 0.4,
                hue: 0,
                place: Place::of_static(Point::new(x, 200, 0)),
                twin: 0,
                owner: 0,
                volumes: range,
            });
            origin_x += f32::from(WIDE) + 20.0;
        };
    for x in 200..203u16 {
        place(
            x,
            RUN_GRAPHIC,
            &wall_tile,
            &Shape::faced(Facing::One(Face::South)),
            &mut quads,
        );
    }
    place(210, STAIR_GRAPHIC, &stair_tile, &Shape::solid(prism), &mut quads);
    place(
        220,
        CORNER_GRAPHIC,
        &wall_tile,
        &Shape::faced(Facing::Corner {
            right: Face::East,
            left: Face::South,
        }),
        &mut quads,
    );
    place(
        230,
        LONE_WALL_GRAPHIC,
        &wall_tile,
        &Shape::faced(Facing::One(Face::East)),
        &mut quads,
    );
    place(240, FLOOR_GRAPHIC, &floor_tile, &Shape::UNREAD, &mut quads);

    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &quads,
        &boxes,
        &[],
        &[],
        SIZE,
    );

    // Which face's own value `stance_of` names — `statics.wesl`'s own function,
    // restated because nothing on this side may call a shader. Only three
    // cases occur, exactly as that function's own doc says: `meets` only ever
    // names a camera-facing face.
    let stance_of = |normal: [f32; 3]| -> Stance {
        if normal[2] == 1.0 {
            Stance::Flat
        } else if normal[0] == 1.0 {
            Stance::FaceEast
        } else {
            Stance::FaceSouth
        }
    };
    // And the way back: which axis a stance's face lies on. **The stance and not
    // the normal is what this sweep reads the face out of**, because the pass
    // writes the met face there for every fragment while a *facing* is only
    // written where the art named the side — see `impostor::Volume::edges`. A
    // body's fragments carry no facing at all, and reading the axis out of a
    // zero vector would have made this sweep a test about which shapes the
    // fixture happens to contain.
    let axis_of = |stance: u32| -> usize {
        match stance {
            s if s == Stance::Flat as u32 => 2,
            s if s == Stance::FaceEast as u32 => 0,
            s if s == Stance::FaceSouth as u32 => 1,
            other => panic!("a met face is one of three stances, not {other}"),
        }
    };

    const EPS: f32 = 1e-3;
    let mut compared = 0u32;
    // How many of them were a body's — no facing written, the face known from
    // the stance alone.
    let mut bodies = 0u32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let word = places.at(x, y);
            if gbuffer::ids_kind(word) != Some(Kind::Static) {
                continue;
            }
            let point = places.position_at(x, y);
            let mine = gbuffer::unpack_solid(point[3]);
            if mine == SolidId::NOBODY {
                continue;
            }
            let solid = grid.solid(SolidId::new(mine));
            let lo = [
                solid.space.min.x as f32,
                solid.space.min.y as f32,
                solid.space.min.z as f32,
            ];
            let hi = [
                solid.space.max.x as f32,
                solid.space.max.y as f32,
                solid.space.max.z as f32,
            ];
            let at = [point[0], point[1], point[2]];

            // The position lies on the boundary of the primitive it names —
            // 6f's own line: a fragment naming the wrong tread is a fragment
            // whose position is nowhere on that tread's box.
            let on_boundary =
                (0..3).any(|axis| (at[axis] - lo[axis]).abs() < EPS || (at[axis] - hi[axis]).abs() < EPS);
            assert!(
                on_boundary,
                "({x}, {y}) is not on the boundary of the primitive it names: {at:?} against \
                 {lo:?}..{hi:?}",
            );

            // The face it met is a camera-facing face of that same primitive —
            // 6h's own line: a face buried inside a merged box is interior to
            // it, not on its boundary in the met face's own axis.
            let stance = gbuffer::ids_stance(word);
            let axis = axis_of(stance);
            // **An equality and not a tolerance**, which is the half of phase 6i's
            // second item that survived reading it: `traced.rs`'s
            // `a_face_fragments_own_plane_is_the_primitives_own_number` makes this
            // claim bit for bit over *mesh* fragments, and the item asked for the
            // same sweep over sprite ones. It could not be had by inverting that
            // test's filter — the scene it runs on draws no sprite at all — and it
            // could not be had by measurement either: `impostor::meets` reached the
            // plane through a divide and a multiply by `Z_PER_TILE`, eleven, so the
            // `z` round trip was exact for the numbers this fixture happens to use
            // (measured: 0 of 78,400 off) and for no stated reason. It is stated
            // now — `meets` takes the exit axis's coordinate from the bound that
            // chose it — so this is an equality on both sides of the wire.
            assert_eq!(
                at[axis], hi[axis],
                "({x}, {y})'s normal names axis {axis} but the fragment is not on that \
                 primitive's high face there: {at:?} against {hi:?}",
            );

            // And the *facing*, where the box gave one, is that same face —
            // 6g's own line. A **body**'s box names no side, so its fragments
            // carry no facing at all and there is nothing here to agree with;
            // both populations are counted below, so neither branch can empty
            // out unnoticed.
            let normal = gbuffer::unpack_normal(places.normal_at(x, y));
            match normal == [0.0; 3] {
                true => bodies += 1,
                false => assert_eq!(
                    stance,
                    stance_of(normal) as u32,
                    "({x}, {y})'s stance does not agree with its own normal: {normal:?}",
                ),
            }
            compared += 1;
        }
    }
    // What the sweep actually reached: a pass that drew nothing, or named no
    // solid for anything, would satisfy every assertion above by never
    // running one.
    // The floor moved from ten thousand to five, and **the reason is the
    // discard**: a fragment whose ray meets no box is no longer drawn
    // (`statics.wesl`, and the amendment in `docs/lighting_rebuild.md`'s "One
    // silhouette"), so the fringe this sweep used to count is not in the frame
    // any more. It reached 7,382 the first time it ran under the new rule. A
    // floor is here at all so that a pass drawing nothing cannot satisfy every
    // assertion above by never running one — and lowering it is only honest
    // because what left is a stated change rather than an unexplained loss.
    assert!(
        compared > 5_000,
        "only {compared} fragments named a primitive: the sweep reached too little to say anything",
    );
    // **And both populations are in it.** The facing claim above is asked of one
    // and skipped for the other, so a scene that had drifted to all bodies or to
    // no bodies would pass it by never reaching it — which is the shape of a
    // gate that has stopped gating. This scene stands a wall run, a floor and a
    // flight of steps, so it holds both by construction.
    assert!(
        bodies > 0 && bodies < compared,
        "{bodies} of {compared} fragments were a body's: the sweep no longer reaches both a box \
         the art named a side of and one it did not",
    );
}

/// A mesh-face fragment's own `place.z` carries the routing sentinel,
/// `Stance::MeshFace`, decoded the same direct way as the two sibling tests
/// above — not the real face this fragment stands on, which
/// `MeshFaceRow::stance` carries instead, read back through `blit.wgsl`'s
/// `mesh_instances` storage buffer, a different consumer than this
/// attachment.
///
/// `docs/lighting_raymarch.md`'s backlog names this the second of two
/// producers with no direct pixel-decode coverage, and the one that needed
/// real plumbing: unlike ground and statics, `render_places` never drove the
/// mesh-face pass before this — it does now, wired in right after the statics
/// pass, the same order `crates/client/app/src/lib.rs`'s real frame runs it
/// in.
#[test]
fn a_mesh_face_pixel_carries_the_mesh_face_sentinel() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    use openshard_client_render::mesh_face::{MeshFaceRow, MeshFaceVertex};
    use openshard_client_render::place::Stance;

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([]).expect("nothing always fits");

    // One flat quad, two triangles, standing over a tile at height 15 — the
    // exact position within the tile is not what this test is about, only
    // that a fragment lands and carries the sentinel.
    let tile = [300.0, 400.0];
    let world = [tile[0] + 0.5, tile[1] + 0.5, 15.0];
    let corner = |x: f32, y: f32| MeshFaceVertex {
        screen: ViewPoint::new(x, y),
        world,
        depth: 0.4,
        id: 0,
        tile,
        normal: Stance::FaceEast.normal(),
        // Not this test's subject — see `a_mesh_face_pixel_carries_its_exact_world_position`.
        colour: [1.0, 1.0, 1.0],
    };
    let vertices = [
        corner(54.0, 54.0),
        corner(74.0, 54.0),
        corner(74.0, 74.0),
        corner(54.0, 54.0),
        corner(74.0, 74.0),
        corner(54.0, 74.0),
    ];
    let rows = [MeshFaceRow {
        tile: (300, 400),
        stance: Stance::FaceEast,
        solid: openshard_client_render::occlusion::SolidId::NOBODY,
    }];

    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &[],
        &[],
        &vertices,
        &rows,
        128,
    );
    let word = places.at(64, 64);
    assert_eq!(
        gbuffer::ids_kind(word),
        Some(Kind::Static),
        "nothing was drawn at (64, 64)",
    );
    assert_eq!(
        gbuffer::ids_stance(word),
        Stance::MeshFace as u32,
        "a mesh-face pixel does not carry the mesh-face sentinel",
    );
}

/// A mesh face's fragment says where it is, to the float.
///
/// `docs/lighting_rebuild.md` phase 2's own "done when", and the mesh pass is
/// the producer to ask it of: its vertices carry their true world positions
/// (`MeshFaceVertex::world`) and the rasteriser interpolates them, so the
/// number the pass has is the number the geometry has — there is no projection
/// to invert and nothing to reconstruct. What this pins is that the number
/// *survives*: the attachment this replaced held `z` to a sixteenth of a unit
/// and the tile fraction to a hundred-and-twenty-seventh, and every constant on
/// the height track exists because the lighting read those instead of this.
///
/// It used to assert the packed height beside the position, so that the two
/// were compared rather than merely both present. There is nothing left to
/// compare: the id plane carries no height at all, which is the phase's own
/// point arriving. What is left is the exactness — and the fixture below is
/// still chosen so that a pipeline which quantised anywhere would fail it.
///
/// The quad is flat and one point over one tile, so every fragment of it has
/// the same position and any pixel inside the shape answers — no clamp is in
/// play, since the point is the tile's middle and not its edge.
#[test]
fn a_mesh_face_pixel_carries_its_exact_world_position() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    use openshard_client_render::mesh_face::{MeshFaceRow, MeshFaceVertex};
    use openshard_client_render::place::Stance;

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([]).expect("nothing always fits");

    // A height no sixteenth and a fraction no hundred-and-twenty-seventh can
    // hold: `15.1` is not a multiple of either. That is the point — a test at
    // `15.0` and `0.5` would pass through the retired packing untouched and
    // prove nothing, so this one fails if anything on the path quantises.
    let tile = [300.0, 400.0];
    let world = [tile[0] + 0.3, tile[1] + 0.7, 15.1];
    let corner = |x: f32, y: f32| MeshFaceVertex {
        screen: ViewPoint::new(x, y),
        world,
        depth: 0.4,
        id: 0,
        tile,
        normal: Stance::Flat.normal(),
        // Not this test's subject — see `a_mesh_face_pixel_carries_the_mesh_face_sentinel`.
        colour: [1.0, 1.0, 1.0],
    };
    let vertices = [
        corner(54.0, 54.0),
        corner(74.0, 54.0),
        corner(74.0, 74.0),
        corner(54.0, 54.0),
        corner(74.0, 74.0),
        corner(54.0, 74.0),
    ];
    let rows = [MeshFaceRow {
        tile: (300, 400),
        stance: Stance::Flat,
        solid: openshard_client_render::occlusion::SolidId::NOBODY,
    }];

    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &[],
        &[],
        &vertices,
        &rows,
        128,
    );
    assert_eq!(
        gbuffer::ids_kind(places.at(64, 64)),
        Some(Kind::Static),
        "nothing was drawn at (64, 64)",
    );
    assert_eq!(
        places.position_at(64, 64),
        // The fourth channel is `SOLID_NONE` and that is this pass's own answer
        // rather than an absence: a mesh face names its solid in its *row*, so
        // `blit.wesl`'s `STANCE_MESH_FACE` branch reads it there and the channel
        // every other producer states it in says "ask the row". See
        // `mesh_face.wesl` and `solid_format.wesl`.
        [world[0], world[1], world[2], gbuffer::SOLID_NONE],
        "a mesh face's fragment does not carry the position the pass computed",
    );
}

/// A mesh face's pixel carries **its own face's** normal, and the two faces of
/// one flight carry two different ones.
///
/// The other half of `docs/lighting_rebuild.md` phase 2's "done when", and the
/// mesh pass is the producer to ask it of for the same reason the position half
/// asks it here: its normals are measured geometry — `crate::mesh::Face::normal`,
/// one per face — rather than a stance a reader turns back into a direction.
///
/// Two faces in one draw, a tread's top and its riser, because one face alone
/// cannot fail this: a plane that wrote a constant would pass it. And the place
/// attachment is asserted beside them, saying `MeshFace` for **both** — that
/// sentinel is a routing tag and not a facing, so the attachment genuinely
/// cannot tell these two surfaces apart and the plane genuinely can. That is
/// the whole of what this phase moved.
///
/// **It is also where the two halves of the octahedral packing are compared.**
/// The plane holds one `u32` now (`gbuffer::NORMAL_FORMAT`), and what is
/// asserted is that word against `gbuffer::pack_normal` of the vector this test
/// handed the pass — an integer the GPU computed against an integer this side
/// computed, with no tolerance between them. `normal_format.wesl` and
/// `gbuffer::pack_normal` are two spellings of one mapping that no compiler
/// compares, and this is the only thing that does.
///
/// The third face is there for that and for nothing else: a **slope**, whose
/// components are none of `-1`, `0` or `1`. The two cardinal faces above go
/// through the packing's exact cases and would pass under a fold spelled
/// differently on the two sides; a direction off the axes would not.
#[test]
fn two_mesh_faces_carry_their_own_two_normals() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    use openshard_client_render::mesh_face::{MeshFaceRow, MeshFaceVertex};
    use openshard_client_render::place::Stance;

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([]).expect("nothing always fits");

    let tile = [300.0, 400.0];
    // One square of screen per face, far enough apart that neither's pixels are
    // the other's. The projection this pass is handed maps a view-space
    // coordinate to the pixel of the same number — see `Target::whole` — so
    // these are the pixels the two faces land on.
    let quad = |from: f32, id: u32, normal: [f32; 3]| {
        let corner = |x: f32, y: f32| MeshFaceVertex {
            screen: ViewPoint::new(x, y),
            world: [tile[0] + 0.5, tile[1] + 0.5, 15.0],
            depth: 0.4,
            id,
            tile,
            normal,
            // Not this test's subject — see `two_mesh_faces_carry_their_own_two_normals`.
            colour: [1.0, 1.0, 1.0],
        };
        let to = from + 20.0;
        [
            corner(from, from),
            corner(to, from),
            corner(to, to),
            corner(from, from),
            corner(to, to),
            corner(from, to),
        ]
    };
    // A tread's top looks up; its riser looks east. `Prism::mesh` builds both
    // from exactly these two shapes, and `Stance::normal` is the same table.
    let top = Stance::Flat.normal();
    let riser = Stance::FaceEast.normal();
    // And a slope, off every axis, which is what `ground.wesl`'s own bilinear
    // patch writes on a hillside and what no cardinal case can stand in for.
    // Normalised here rather than authored as a unit vector so that the number
    // fed to the pass is the number `pack_normal` is asked about.
    let slope = {
        let raw = [0.37f32, -0.62, 0.69];
        let length = (raw[0] * raw[0] + raw[1] * raw[1] + raw[2] * raw[2]).sqrt();
        [raw[0] / length, raw[1] / length, raw[2] / length]
    };
    let vertices: Vec<MeshFaceVertex> = quad(24.0, 0, slope)
        .into_iter()
        .chain(quad(54.0, 1, top))
        .chain(quad(84.0, 2, riser))
        .collect();
    let row = |stance| MeshFaceRow {
        tile: (300, 400),
        stance,
        solid: openshard_client_render::occlusion::SolidId::NOBODY,
    };
    // The slope's row carries `Flat`: a stance is four faces and a lid and
    // cannot name a hillside, which is exactly why the normal is a plane of its
    // own and not four bits of the id word.
    let rows = [row(Stance::Flat), row(Stance::Flat), row(Stance::FaceEast)];

    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &[],
        &[],
        &vertices,
        &rows,
        128,
    );
    let faces = [
        (34, 34, slope, "a slope"),
        (64, 64, top, "a tread's top"),
        (94, 94, riser, "a riser"),
    ];
    for (x, y, normal, what) in faces {
        assert_eq!(
            gbuffer::ids_kind(places.at(x, y)),
            Some(Kind::Static),
            "nothing was drawn on {what}",
        );
        // The word, not the direction: see `Places::normal_at`. What a failure
        // *means* is spelled out beside it, since two words differing in their
        // low bits and two words naming opposite directions read alike as
        // integers.
        let word = places.normal_at(x, y);
        assert_eq!(
            word,
            gbuffer::pack_normal(normal),
            "{what} carries {:?} where the pass was handed {normal:?}",
            gbuffer::unpack_normal(word),
        );
        // And the field the lighting used to read this from cannot separate
        // them: every one of these fragments' stance bits holds the routing
        // sentinel, which names no direction at all.
        assert_eq!(
            gbuffer::ids_stance(places.at(x, y)),
            Stance::MeshFace as u32,
            "the id word stopped carrying the sentinel on {what}",
        );
    }
    // A pixel nothing drew is the cleared word, which is how `NORMAL_DRAWN`
    // earns its bit: zero is "no fragment here" and a fragment with no facing
    // is that bit alone, and the two must not read alike.
    assert_eq!(places.normal_at(4, 4), 0, "the clear is not the undrawn word");
}

/// Draw ground, statics and any mesh faces standing on them, and read back the
/// *place* attachment and the position plane beside it. `size * 8` must be a
/// multiple of 256, as every readback here.
#[allow(clippy::too_many_arguments)]
fn render_places(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    quads: &[GroundQuad],
    static_atlas: &StaticAtlas,
    static_quads: &[SpriteQuad],
    static_boxes: &[openshard_client_render::impostor::Volume],
    mesh_vertices: &[openshard_client_render::mesh_face::MeshFaceVertex],
    mesh_rows: &[openshard_client_render::mesh_face::MeshFaceRow],
    size: u32,
) -> Places {
    render_places_with_fringe(
        device,
        queue,
        atlas,
        texmaps,
        quads,
        static_atlas,
        static_quads,
        static_boxes,
        mesh_vertices,
        mesh_rows,
        size,
        Fringe::Clamp,
    )
}

/// [`render_places`], with the fringe switch stated rather than defaulted —
/// `impostor::Fringe`, the three answers a fragment that met no box has had.
///
/// A parameter here and a field on the pass in the renderer, for the same
/// reason: every other caller in this file is asking about a *frame*, and the
/// switch is a state of the picture that a frame test has no opinion about.
#[allow(clippy::too_many_arguments)]
fn render_places_with_fringe(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    quads: &[GroundQuad],
    static_atlas: &StaticAtlas,
    static_quads: &[SpriteQuad],
    static_boxes: &[openshard_client_render::impostor::Volume],
    mesh_vertices: &[openshard_client_render::mesh_face::MeshFaceVertex],
    mesh_rows: &[openshard_client_render::mesh_face::MeshFaceRow],
    size: u32,
    fringe: Fringe,
) -> Places {
    // The narrowest plane decides it: the id plane is four bytes a texel where
    // the `place` attachment it replaced was eight, so a size that used to be
    // exactly on the boundary is now half of one.
    assert_eq!(size * 4 % 256, 0, "a row copy has to be 256-byte aligned");
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let world = openshard_client_render::blit::world_texture(device, size, size);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(device, size, size);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(device, size, size);
    let gbuffer_views = gbuffer.views();
    let hue_ramp = HueRamp::build(&Hues::parse(&[0u8; 708]).expect("one empty group"));

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ids"),
        size: u64::from(size) * u64::from(size) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let position_readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("positions"),
        size: u64::from(size) * u64::from(size) * 16,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let normal_readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("normals"),
        size: u64::from(size) * u64::from(size) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut ground_pass = GroundRenderer::new(device, queue, format, atlas, texmaps);
    let mut sprite_pass = SpriteRenderer::new(device, queue, format, static_atlas.pixels(), &hue_ramp);
    sprite_pass.set_fringe(fringe);
    let mut mesh_pass = renderer::MeshFaceRenderer::new(device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target = Target::whole(&world_view, &depth_view, &gbuffer_views, size, size);
    ground_pass.render(device, queue, &mut encoder, target, quads);
    // A corner static's two faces get their own id here too, the same as the
    // real pass — see `sprite::split_corners`'s own doc and
    // `docs/gbuffer.md` step 4.
    let instances = openshard_client_render::sprite::split_corners(static_quads.to_vec());
    sprite_pass.render(
        device,
        queue,
        &mut encoder,
        target,
        &instances.rows,
        static_boxes,
        Some(instances.drawn),
    );
    // Right after statics, into the same static's own pixels — the real
    // renderer's own order (`docs/gbuffer.md` step 4c), so depth and place
    // only ever tie or improve on what the billboard sprite just wrote.
    mesh_pass.render(device, queue, &mut encoder, target, mesh_vertices, mesh_rows);
    // **And the silhouette pass, with nothing to ring** — which is what the
    // client does on every frame that highlights nothing, and which is the whole
    // reason this line exists. It draws into a mask of its own, writes no depth
    // and touches no plane read back below, so it cannot change what any test
    // here asserts *through the picture*. What it does touch is the uniform
    // block it shares with the pass above, and a queue write is applied at the
    // submission rather than where it sits in the encoder: a value this pass got
    // wrong would reach the statics draw that was recorded before it. That is
    // exactly what happened to `Fringe` — the switch worked in every tool and in
    // this file, and did nothing in the client, because no tool and no test drew
    // a ring. Skipping it here would be a frame nobody actually renders.
    let mask = outline::mask_texture(device, size, size);
    let mask_view = mask.create_view(&wgpu::TextureViewDescriptor::default());
    sprite_pass.render_mask(device, queue, &mut encoder, target, &mask_view, &[]);
    let mut copy = |texture: &wgpu::Texture, buffer: &wgpu::Buffer, stride: u32| {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * stride),
                    rows_per_image: Some(size),
                },
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
    };
    copy(gbuffer.ids(), &readback, 4);
    copy(gbuffer.position(), &position_readback, 16);
    copy(gbuffer.normal(), &normal_readback, 4);
    queue.submit([encoder.finish()]);

    let read = |buffer: &wgpu::Buffer| {
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |result| {
            result.expect("mapping a buffer this test just wrote");
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("waiting on our own submission");
        let bytes = slice
            .get_mapped_range()
            .expect("the map completed above")
            .to_vec();
        buffer.unmap();
        bytes
    };
    Places {
        width: size,
        bytes: read(&readback),
        positions: read(&position_readback),
        normals: read(&normal_readback),
    }
}

/// [`render_places`], with a second sprite pass for a mobile's own atlas —
/// the shape [`render_both`] already uses for colour, here for the G-buffer
/// instead. `mobile_quads` draws after `static_quads`, the real renderer's
/// own order.
#[allow(clippy::too_many_arguments)]
fn render_places_with_mobile(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    quads: &[GroundQuad],
    static_atlas: &StaticAtlas,
    static_quads: &[SpriteQuad],
    mobile_atlas: &[u8],
    mobile_quads: &[SpriteQuad],
    size: u32,
) -> Places {
    assert_eq!(size * 4 % 256, 0, "a row copy has to be 256-byte aligned");
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let world = openshard_client_render::blit::world_texture(device, size, size);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(device, size, size);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(device, size, size);
    let gbuffer_views = gbuffer.views();
    let hue_ramp = HueRamp::build(&Hues::parse(&[0u8; 708]).expect("one empty group"));

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ids"),
        size: u64::from(size) * u64::from(size) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let position_readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("positions"),
        size: u64::from(size) * u64::from(size) * 16,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let normal_readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("normals"),
        size: u64::from(size) * u64::from(size) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut ground_pass = GroundRenderer::new(device, queue, format, atlas, texmaps);
    let mut sprite_pass = SpriteRenderer::new(device, queue, format, static_atlas.pixels(), &hue_ramp);
    let mut mobile_pass = SpriteRenderer::new(device, queue, format, mobile_atlas, &hue_ramp);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target = Target::whole(&world_view, &depth_view, &gbuffer_views, size, size);
    ground_pass.render(device, queue, &mut encoder, target, quads);
    let instances = openshard_client_render::sprite::split_corners(static_quads.to_vec());
    sprite_pass.render(
        device,
        queue,
        &mut encoder,
        target,
        &instances.rows,
        &[],
        Some(instances.drawn),
    );
    mobile_pass.render(device, queue, &mut encoder, target, mobile_quads, &[], None);
    let mut copy = |texture: &wgpu::Texture, buffer: &wgpu::Buffer, stride: u32| {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * stride),
                    rows_per_image: Some(size),
                },
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
    };
    copy(gbuffer.ids(), &readback, 4);
    copy(gbuffer.position(), &position_readback, 16);
    copy(gbuffer.normal(), &normal_readback, 4);
    queue.submit([encoder.finish()]);

    let read = |buffer: &wgpu::Buffer| {
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |result| {
            result.expect("mapping a buffer this test just wrote");
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("waiting on our own submission");
        let bytes = slice
            .get_mapped_range()
            .expect("the map completed above")
            .to_vec();
        buffer.unmap();
        bytes
    };
    Places {
        width: size,
        bytes: read(&readback),
        positions: read(&position_readback),
        normals: read(&normal_readback),
    }
}

/// The G-buffer read back: three planes over one frame's pixels.
struct Places {
    width: u32,
    /// The id plane — one `u32` a texel, `crate::gbuffer::pack_ids`'s layout.
    bytes: Vec<u8>,
    /// The position plane over the same pixels — read back with the id one
    /// and never apart from it, because the two are one frame's answer and a
    /// fixture that copied them from different draws would compare a fragment
    /// against a different fragment.
    positions: Vec<u8>,
    /// And the normal plane over the same pixels, on the same terms.
    normals: Vec<u8>,
}

impl Places {
    /// The id word at one pixel — kind, stance and row, and
    /// `crate::gbuffer`'s three readers are how to take it apart.
    fn at(&self, x: u32, y: u32) -> u32 {
        let start = ((y * self.width + x) * 4) as usize;
        u32::from_le_bytes([
            self.bytes[start],
            self.bytes[start + 1],
            self.bytes[start + 2],
            self.bytes[start + 3],
        ])
    }

    /// `(x, y, z, 1)` at one pixel — [`openshard_client_render::gbuffer`]'s
    /// position plane, the number itself rather than the fields above.
    fn position_at(&self, x: u32, y: u32) -> [f32; 4] {
        Self::float_at(&self.positions, self.width, x, y)
    }

    /// The octahedral word at one pixel —
    /// [`openshard_client_render::gbuffer`]'s normal plane, the word itself
    /// rather than the direction in it.
    ///
    /// The **word**, deliberately, because that is what makes this a comparison
    /// between two implementations rather than a tolerance: a caller asserts it
    /// against `gbuffer::pack_normal` of the vector it handed the pass, so the
    /// integer the GPU computed is checked against the integer this side
    /// computes, exactly. `unpack_normal` is there for a caller that wants to
    /// *say* what a failure means.
    fn normal_at(&self, x: u32, y: u32) -> u32 {
        let start = ((y * self.width + x) * 4) as usize;
        u32::from_le_bytes([
            self.normals[start],
            self.normals[start + 1],
            self.normals[start + 2],
            self.normals[start + 3],
        ])
    }

    /// One texel of the position plane, the crate's one remaining
    /// `Rgba32Float` attachment.
    fn float_at(plane: &[u8], width: u32, x: u32, y: u32) -> [f32; 4] {
        let start = ((y * width + x) * 16) as usize;
        let mut out = [0f32; 4];
        for (channel, slot) in out.iter_mut().enumerate() {
            let at = start + channel * 4;
            *slot = f32::from_le_bytes([plane[at], plane[at + 1], plane[at + 2], plane[at + 3]]);
        }
        out
    }
}

/// A hill in front hides a wall behind it.
///
/// The assertion the depth buffer exists for, and the one no pass order can
/// satisfy: all the ground is drawn before any static, so without a shared
/// depth every static would be in front of every tile. Both quads are built
/// here rather than read from a map, because what is being checked is the
/// *ordering*, and a real hillside would decide the geometry as well.
#[test]
fn ground_in_front_hides_a_static_behind_it() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    let green = Color16(0b0_00000_11111_00000);
    let red = Color16(0b0_11111_00000_00000);

    let side = usize::from(LAND_TILE_SIZE);
    let land = LandAtlas::pack([(
        GRAPHIC,
        Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![green; side * side]),
    )])
    .expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([(GRAPHIC, Image::new(60, 60, vec![red; 60 * 60]))]).expect("fits");
    let region = land.region(GRAPHIC).expect("packed");
    let sprite = statics.sprite(GRAPHIC).expect("packed");

    // Same pixels on screen, and the ground is nearer. A wall standing behind
    // a hill: the sprite's rectangle covers the tile's diamond entirely, so
    // every pixel of the diamond is a pixel both quads want.
    let ground = [GroundQuad {
        x: 64.0,
        y: 64.0,
        corners: [0.0; 4],
        region,
        texmap: None,
        depth: 0.4,
        place: Place::land(1, 1),
    }];
    let wall = [SpriteQuad {
        rect: Rect {
            x: 64.0 - 30.0,
            y: 64.0 - 30.0,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.6,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
        volumes: openshard_client_render::impostor::Range::default(),
    }];
    let none = AnimAtlas::pack([]).expect("nothing always fits");
    let frame = render_both(
        &device,
        &queue,
        &land,
        &texmaps,
        &ground,
        &statics,
        &wall,
        (none.pixels(), &[]),
        128,
        128,
        Projection::one_to_one(128, 128),
    );

    let Rgb8 {
        red: green_r,
        green: green_g,
        blue: green_b,
    } = green.rgb8();
    let [green_r, green_g, green_b] =
        openshard_client_render::tonemap::shade_u8([green_r, green_g, green_b], [1.0; 3]);
    let mut ground_pixels = 0;
    for y in 0..128u32 {
        for x in 0..128u32 {
            if frame.pixel(x, y) == [green_r, green_g, green_b, u8::MAX] {
                ground_pixels += 1;
            }
        }
    }
    // The diamond, whole: not one of its pixels was overwritten by the static
    // that came after it.
    assert_eq!(ground_pixels, 1012, "the wall drew over the hill in front of it");

    // And the reverse ordering does the opposite, or the assertion above is
    // satisfied by a statics pass that draws nothing at all.
    let front = [SpriteQuad {
        depth: 0.2,
        ..wall[0]
    }];
    let frame = render_both(
        &device,
        &queue,
        &land,
        &texmaps,
        &ground,
        &statics,
        &front,
        (none.pixels(), &[]),
        128,
        128,
        Projection::one_to_one(128, 128),
    );
    let covered = (0..128u32)
        .flat_map(|y| (0..128u32).map(move |x| (x, y)))
        .filter(|&(x, y)| frame.pixel(x, y) == [green_r, green_g, green_b, u8::MAX])
        .count();
    assert_eq!(covered, 0, "a static in front left ground showing through");
}

/// Two things at one depth are decided by which is drawn later, and that is the
/// client's own tie-break rather than an accident of the pass order.
///
/// `Chunk.AddGameObject` inserts by `PriorityZ` and, on a tie, puts the land
/// tile *first* in the per-tile list — so the flagstone lying at exactly the
/// height of the ground under it is drawn second, and covers it. Here that is
/// `LessEqual` in `renderer::depth_state` plus the order the passes already
/// run in: the ground pass, then the statics, then the mobiles.
///
/// It needs a frame because the depth *state* is what is being asserted. Every
/// number this crate computes can be right and this still be backwards, and
/// under `Less` it was: the depths agreed with the client and the first writer
/// kept the pixel, so the ground won every tie it should have lost.
#[test]
fn at_one_depth_the_later_pass_wins() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    let green = Color16(0b0_00000_11111_00000);
    let red = Color16(0b0_11111_00000_00000);

    let side = usize::from(LAND_TILE_SIZE);
    let land = LandAtlas::pack([(
        GRAPHIC,
        Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![green; side * side]),
    )])
    .expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([(GRAPHIC, Image::new(60, 60, vec![red; 60 * 60]))]).expect("fits");
    let region = land.region(GRAPHIC).expect("packed");
    let sprite = statics.sprite(GRAPHIC).expect("packed");

    // The same depth, to the bit: not "very close", which the test would pass
    // under either comparison.
    const TIED: f32 = 0.5;
    let ground = [GroundQuad {
        x: 64.0,
        y: 64.0,
        corners: [0.0; 4],
        region,
        texmap: None,
        depth: TIED,
        place: Place::land(1, 1),
    }];
    let flagstone = [SpriteQuad {
        rect: Rect {
            x: 64.0 - 30.0,
            y: 64.0 - 30.0,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: TIED,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
        volumes: openshard_client_render::impostor::Range::default(),
    }];
    let none = AnimAtlas::pack([]).expect("nothing always fits");
    let frame = render_both(
        &device,
        &queue,
        &land,
        &texmaps,
        &ground,
        &statics,
        &flagstone,
        (none.pixels(), &[]),
        128,
        128,
        Projection::one_to_one(128, 128),
    );

    let Rgb8 {
        red: green_r,
        green: green_g,
        blue: green_b,
    } = green.rgb8();
    let showing = (0..128u32)
        .flat_map(|y| (0..128u32).map(move |x| (x, y)))
        .filter(|&(x, y)| frame.pixel(x, y) == [green_r, green_g, green_b, u8::MAX])
        .count();
    assert_eq!(showing, 0, "the ground kept a pixel from the static tied with it");

    // And the static really covered those pixels rather than the frame being
    // empty: the sprite's whole rectangle is its own colour.
    let Rgb8 {
        red: red_r,
        green: red_g,
        blue: red_b,
    } = red.rgb8();
    let covered = (0..128u32)
        .flat_map(|y| (0..128u32).map(move |x| (x, y)))
        .filter(|&(x, y)| frame.pixel(x, y) == [red_r, red_g, red_b, u8::MAX])
        .count();
    assert_eq!(covered, 60 * 60, "the static did not draw its whole rectangle");
}

/// A mobile is drawn from its own atlas, in front of the ground it stands on,
/// and a mirrored facing is the same picture backwards.
///
/// The mirror is what needs a frame rather than an assertion on numbers: the
/// region arithmetic is checked in `sprite`, but whether a *negative* region
/// width actually samples backwards is the GPU's answer and not ours. A shader
/// that clamped it instead would leave every west-facing creature looking east,
/// which is a bug a screenshot of one direction cannot show.
#[test]
fn a_mobile_is_drawn_over_the_ground_and_mirrors_with_its_facing() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const BODY: u16 = 400;
    let green = Color16(0b0_00000_11111_00000);
    let red = Color16(0b0_11111_00000_00000);

    // A two-pixel-wide frame: red on the left, green on the right. Mirrored,
    // the two swap, and nothing else about the quad changes.
    let frame = AnimFrame {
        center_x: 1,
        center_y: 0,
        image: Image::new(2, 1, vec![red, green]),
    };
    let atlas = AnimAtlas::pack([(
        FrameKey::new(
            AnimationKey::new(Graphic(BODY), AnimationGroup(4), AnimationDirection(1)),
            AnimationFrameIndex(0),
        ),
        frame,
    )])
    .expect("one frame fits");

    // Ground under it, at the same tile: the mobile has to win, and the ground
    // is what makes that a claim rather than a drawing on an empty frame.
    let side = usize::from(LAND_TILE_SIZE);
    let blue = Color16(0b0_00000_00000_11111);
    let land = LandAtlas::pack([(
        Graphic(1),
        Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![blue; side * side]),
    )])
    .expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([]).expect("nothing always fits");
    let centre = Point::new(100, 100, 0);
    let camera = Camera::new(centre, 256, 256);

    // The ground quad is built here rather than collected: `WorldMap` cannot be
    // constructed in memory — see the backlog in docs/client.md — and what this
    // test needs is one tile under the mobile's feet at the depth `depth` would
    // have given it.
    let at = camera.to_screen(centre);
    let ground = [GroundQuad {
        x: at.x as f32,
        y: at.y as f32,
        corners: [0.0; 4],
        region: land.region(Graphic(1)).expect("packed"),
        texmap: None,
        depth: openshard_client_render::depth::Order {
            tile: 200,
            priority_z: openshard_client_render::depth::land_priority_z([0; 4]),
        }
        .to_depth(openshard_client_render::depth::base_for(100, 100)),
        place: Place::land(100, 100),
    }];

    let colours = |facing| {
        let quads = mobiles::collect(
            &[Mobile {
                at: centre,
                body: Graphic(BODY),
                group: AnimationGroup(4),
                facing,
                frame: AnimationFrameIndex(0),
                from: None,
                corpse: false,
                hue: openshard_protocol::wire::Hue::NONE,
                drawn: openshard_client_render::follow::Gaze::on(centre),
                equipment: Vec::new().into(),
            }],
            &camera,
            &atlas,
            &Cutaway::OPEN,
            &EquipConv::default(),
            None,
        );
        assert_eq!(quads.len(), 1, "the frame is packed, so it draws");
        let frame = render_both(
            &device,
            &queue,
            &land,
            &texmaps,
            &ground,
            &statics,
            &[],
            (atlas.pixels(), &quads),
            256,
            256,
            Projection::one_to_one(256, 256),
        );
        // The two pixels the sprite covers, left and right.
        let x = quads[0].rect.x as u32;
        let y = quads[0].rect.y as u32;
        (frame.pixel(x, y), frame.pixel(x + 1, y))
    };

    let Rgb8 {
        red: red_r,
        green: red_g,
        blue: red_b,
    } = red.rgb8();
    let [red_r, red_g, red_b] = openshard_client_render::tonemap::shade_u8([red_r, red_g, red_b], [1.0; 3]);
    let Rgb8 {
        red: green_r,
        green: green_g,
        blue: green_b,
    } = green.rgb8();
    let [green_r, green_g, green_b] =
        openshard_client_render::tonemap::shade_u8([green_r, green_g, green_b], [1.0; 3]);
    // South is stored direction 1 unflipped, East is the same picture mirrored.
    assert_eq!(
        colours(Direction::South),
        (
            [red_r, red_g, red_b, u8::MAX],
            [green_r, green_g, green_b, u8::MAX]
        ),
        "the mobile is not drawn over the ground, or not from its own atlas",
    );
    assert_eq!(
        colours(Direction::East),
        (
            [green_r, green_g, green_b, u8::MAX],
            [red_r, red_g, red_b, u8::MAX]
        ),
        "a mirrored facing drew the picture the same way round",
    );
}

/// The same camera twice is the same bytes.
///
/// Determinism is not a nicety here: it is what makes every other assertion in
/// this file reproducible, and the ordering it depends on — the sort in
/// `ground::collect`, the `BTreeSet` in the atlas — is easy to lose to a
/// `HashMap` in a later change that looks harmless.
#[test]
fn the_same_camera_renders_the_same_frame() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let camera = Camera::new(Point::new(1495, 1629, 0), 768, 512);

    let mut frames = Vec::new();
    for _ in 0..2 {
        let wanted = ground::visible_graphics(&map, &camera);
        let atlas = LandAtlas::build(&art, wanted.iter().copied()).expect("fits");
        let texmaps = texmap_atlas(&dir, wanted);
        let quads = ground::collect(&map, &camera, &atlas, &texmaps, &Cutaway::OPEN);
        frames.push(
            render(
                &device,
                &queue,
                &atlas,
                &texmaps,
                &quads,
                camera.width,
                camera.height,
            )
            .pixels,
        );
    }
    assert_eq!(frames[0], frames[1]);

    // And a different camera is a different frame — otherwise the assertion
    // above is satisfied by a renderer that draws nothing at all.
    let moved = Camera::new(Point::new(1497, 1629, 0), 768, 512);
    let wanted = ground::visible_graphics(&map, &moved);
    let atlas = LandAtlas::build(&art, wanted.iter().copied()).expect("fits");
    let texmaps = texmap_atlas(&dir, wanted);
    let quads = ground::collect(&map, &moved, &atlas, &texmaps, &Cutaway::OPEN);
    let other = render(
        &device,
        &queue,
        &atlas,
        &texmaps,
        &quads,
        moved.width,
        moved.height,
    );
    assert_ne!(frames[0], other.pixels, "moving the camera changed nothing");
}

/// The gate `docs/camera.md` D11 asks for, on the GPU: magnified, moving the
/// eye by `1/zoom` of a virtual pixel moves the picture by exactly one real one.
///
/// Everything else about D11 is arithmetic that can be asserted without a
/// device. This is the claim that cannot: that the shader's last two lines,
/// the rasteriser and `nearest` sampling together produce a frame that is the
/// other frame *translated*, rather than one resampled by a fraction of a texel.
/// The second is what a magnification usually costs, it looks like a slight
/// change in the art, and no arithmetic in `camera.rs` would notice it.
///
/// Two cameras a third of a virtual pixel apart at `3x`, which is one real pixel
/// and is the finest step the display has. The quads are built once and shared
/// deliberately: `to_view` measures from the eye *rounded*, and both eyes round
/// to the same virtual pixel, so the only difference between the two frames is
/// `Projection::origin` — which is exactly the claim.
///
/// A third and not a half, because a half is the one fraction that could be
/// right for the wrong reason: it is on the lattice of `2x` as well, so a
/// rounding that quietly went to the nearest *even* real pixel would pass it.
#[test]
fn a_third_of_a_virtual_pixel_moves_a_magnified_frame_one_real_pixel() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");

    let mut camera = Camera::new(Point::new(1495, 1629, 0), 512, 256);
    let mut zoom = Zoom::ONE;
    for _ in 0..2 {
        zoom = zoom.scale_up();
    }
    camera.zoom_about(RealPixel::new(256, 128), zoom);
    assert_eq!(camera.zoom().to_string(), "3x", "the rung this test is about");
    assert!(
        !camera.minifies(),
        "and the world is drawn at the display's own size"
    );

    let wanted = ground::visible_graphics(&map, &camera);
    let land = LandAtlas::build(&art, wanted.iter().copied()).expect("fits");
    let texmaps = texmap_atlas(&dir, wanted);
    let quads = ground::collect(&map, &camera, &land, &texmaps, &Cutaway::OPEN);

    // Along `x` only: a diagonal move would pass on a frame shifted the right
    // distance the wrong way round, and the two axes are separate lines of
    // shader.
    let mut shifted = camera;
    let at = camera.eye_at();
    shifted.look_at(WorldPoint {
        x: at.x + camera.quantum(),
        y: at.y,
    });
    assert_eq!(shifted.eye(), camera.eye(), "the same whole virtual pixel");
    assert_ne!(shifted.projection(), camera.projection(), "and a different frame");

    let (width, height) = (camera.width, camera.height);
    let before = render_projected(&device, &queue, &land, &texmaps, &quads, width, height, camera);
    let after = render_projected(&device, &queue, &land, &texmaps, &quads, width, height, shifted);
    assert_ne!(before.pixels, after.pixels, "a real pixel moved nothing at all");

    // The eye moved right, so the world moved left: what is at `x` in the second
    // frame was at `x + 1` in the first. Compared over the interior, because the
    // column the shift walks in from has no counterpart to be compared with.
    let mut checked = 0usize;
    let mut moved = 0usize;
    let mut resampled = 0usize;
    for y in 0..height {
        for x in 0..width - 1 {
            checked += 1;
            if after.pixel(x, y) == before.pixel(x + 1, y) {
                moved += 1;
            } else {
                resampled += 1;
            }
        }
    }
    // Counted and asserted, because "every pixel matched" and "no pixel was
    // looked at" are the same green — this repository has produced the second
    // one before.
    assert_eq!(checked, (width as usize - 1) * height as usize);

    // What is *not* an exact translation, and why it cannot be. A sloped tile is
    // textured by stretching a square texmap over a diamond, so its `uv` is
    // interpolated across a quad that is not axis-aligned and a fragment centre
    // a third of a texel along lands on the other side of a texel boundary here
    // and there. There is no placement of the quantiser that fixes that: it is
    // what stretching a texture means. Everything drawn from the *art* — flat
    // ground, statics, sprites — is texel-aligned and translates exactly, which
    // is what the sprite half of this gate below asserts with no allowance at
    // all.
    //
    // One in a thousand is a ceiling and not a measurement (it is one in seven
    // thousand over Britain), and the mutation is what says a ceiling is enough:
    // an origin that rounds its fraction away draws the *same* frame twice, so
    // the number this is separating a correct camera from is not 1 in 7,000 but
    // 130,815 in 130,816.
    assert!(
        resampled * 1000 < checked,
        "{resampled} of {checked} pixels are not the frame before it, translated",
    );
    assert!(
        moved > checked / 2,
        "a frame that agreed nowhere is not a translation"
    );

    // And the guard against all of that holding vacuously. Comparing the two
    // frames *without* the translation is not a good enough test on its own —
    // ground is large flat regions of colour, so more than half the pixels have
    // the same value one pixel over regardless — so what is asserted is that the
    // translation explains strictly more of the frame than standing still does.
    // Under the mutation the two are equal, which is what makes this the
    // discriminating comparison rather than a restatement of the one above.
    let still = (0..height)
        .flat_map(|y| (0..width - 1).map(move |x| (x, y)))
        .filter(|&(x, y)| after.pixel(x, y) == before.pixel(x, y))
        .count();
    assert!(
        moved > still,
        "translating explains {moved} pixels and standing still explains {still}",
    );
}

/// And the half of that gate with no allowance in it: a *sprite* at `3x`,
/// shifted a third of a virtual pixel, is the same picture one real pixel over.
///
/// Everything drawn from the art rather than from a texmap is texel-aligned —
/// the quad is the sprite's own rectangle, so a fragment centre lands on a
/// texel centre at every magnification — and a translation of the quad by a
/// whole real pixel is therefore a translation of the picture, exactly, with no
/// resampling anywhere. This is the claim the character's own smoothness rests
/// on, so it is asserted without a tolerance.
#[test]
fn a_magnified_sprite_translates_texel_for_texel() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let art = Art::open(&dir).expect("artLegacyMUL.uop");

    let mut camera = Camera::new(Point::new(200, 200, 0), 512, 256);
    let mut zoom = Zoom::ONE;
    for _ in 0..2 {
        zoom = zoom.scale_up();
    }
    camera.zoom_about(RealPixel::new(256, 128), zoom);
    assert_eq!(camera.zoom().to_string(), "3x");

    // A static's sprite, drawn through the same pass a mobile uses: what is
    // being asserted is the pass and the transform, and a static is the one this
    // suite can build without an animation file.
    let graphic = Graphic(0x0CE3);
    let atlas = StaticAtlas::build(&art, [graphic]).expect("one sprite fits");
    let sprite = atlas.sprite(graphic).expect("just packed");
    let quads = vec![SpriteQuad {
        rect: Rect {
            x: (camera.render_width() as i32 / 2) as f32,
            y: (camera.render_height() as i32 / 2) as f32,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
        volumes: openshard_client_render::impostor::Range::default(),
    }];

    let mut shifted = camera;
    let at = camera.eye_at();
    shifted.look_at(WorldPoint {
        x: at.x + camera.quantum(),
        y: at.y,
    });

    let land = LandAtlas::build(&art, []).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let none = AnimAtlas::pack([]).expect("nothing always fits");
    let (width, height) = (camera.width, camera.height);
    let frame = |camera: Camera| {
        render_both(
            &device,
            &queue,
            &land,
            &texmaps,
            &[],
            &atlas,
            &quads,
            (none.pixels(), &[]),
            width,
            height,
            camera.projection(),
        )
    };
    let before = frame(camera);
    let after = frame(shifted);

    let mut drawn = 0usize;
    for y in 0..height {
        for x in 0..width - 1 {
            assert_eq!(
                after.pixel(x, y),
                before.pixel(x + 1, y),
                "({x}, {y}) is not the frame before it, translated",
            );
            if after.pixel(x, y)[3] != 0 {
                drawn += 1;
            }
        }
    }
    // The sprite has to actually be on screen, or the assertion above compared
    // a cleared frame with a cleared frame and passed for it.
    assert!(
        drawn > 1_000,
        "only {drawn} pixels of sprite: a blank frame agrees"
    );
}

/// A screen of Britain with its statics on it: the buildings cover a real part
/// of the frame, and the ground still covers all of it.
///
/// Two claims in one, and they are the two ways this layer fails as a whole.
/// Statics covering nothing means the sprites, the atlas or the placement
/// dropped everything and the frame is the old ground-only one; ground no
/// longer covering the viewport means the depth buffer or the second pass took
/// pixels away from it, which is a hole in the world rather than a wall.
#[test]
fn britains_statics_cover_part_of_a_frame_that_is_still_whole() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let tiledata = openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");
    let camera = Camera::new(Point::new(1495, 1629, 0), 768, 512);

    let wanted = ground::visible_graphics(&map, &camera);
    let land = LandAtlas::build(&art, wanted.iter().copied()).expect("fits");
    let texmaps = texmap_atlas(&dir, wanted);
    let quads = ground::collect(&map, &camera, &land, &texmaps, &Cutaway::OPEN);

    let wanted_statics = statics::visible_graphics(&map, &camera, &StaticAnimations::default());
    let static_atlas = StaticAtlas::build(&art, wanted_statics).expect("a screen of statics fits");
    let static_quads = statics::collect(
        &map,
        &camera,
        &tiledata,
        &StaticAnimations::default(),
        &static_atlas,
        &Cutaway::OPEN,
        &openshard_client_render::occlusion::Occlusion::EMPTY,
        None,
        None,
    )
    .quads;
    assert!(
        static_quads.len() > 500,
        "only {} statics in the middle of Britain",
        static_quads.len(),
    );

    let ground_only = render(
        &device,
        &queue,
        &land,
        &texmaps,
        &quads,
        camera.width,
        camera.height,
    );
    let none = AnimAtlas::pack([]).expect("nothing always fits");
    let frame = render_both(
        &device,
        &queue,
        &land,
        &texmaps,
        &quads,
        &static_atlas,
        &static_quads,
        (none.pixels(), &[]),
        camera.width,
        camera.height,
        camera.projection(),
    );

    // Still whole: every pixel drawn, exactly as with ground alone.
    let total = (camera.width * camera.height) as usize;
    assert_eq!(frame.drawn(), total, "the statics pass left holes in the world");

    // And a real part of it changed. A tenth is a floor rather than a
    // measurement — the point is that it is not a handful of pixels, which is
    // what a placement off by a tile or an atlas that packed nothing produces.
    let changed = (0..camera.height)
        .flat_map(|y| (0..camera.width).map(move |x| (x, y)))
        .filter(|&(x, y)| frame.pixel(x, y) != ground_only.pixel(x, y))
        .count();
    assert!(
        changed > total / 10,
        "the statics changed only {changed} of {total} pixels",
    );
}

/// Write a frame of Britain out as a picture, for a person to look at.
///
/// Ignored: it is not an assertion, it is the eye. Every other test here counts
/// pixels, and counting is what catches a sprite sampled one texel over — it is
/// not what catches ground that is the right shape and the wrong terrain. Run it
/// with a client and look:
///
/// ```sh
/// OPENSHARD_CLIENT=… cargo test -p openshard-client-render --test frame -- \
///     --ignored dump_a_frame
/// ```
///
/// PNG, through `crate::png` — the crate's own encoder, so nothing has to be
/// added to the workspace to write one.
#[test]
#[ignore = "writes a picture for a person, and asserts nothing"]
fn dump_a_frame_of_britain() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let centre = Point::new(1495, 1629, 0);
    let camera = Camera::new(centre, 768, 512);

    let tiledata = openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");
    let wanted = ground::visible_graphics(&map, &camera);
    let atlas = LandAtlas::build(&art, wanted.iter().copied()).expect("fits");
    let texmaps = texmap_atlas(&dir, wanted);
    let quads = ground::collect(&map, &camera, &atlas, &texmaps, &Cutaway::OPEN);

    let static_atlas = StaticAtlas::build(
        &art,
        statics::visible_graphics(&map, &camera, &StaticAnimations::default()),
    )
    .expect("statics fit");
    let static_quads = statics::collect(
        &map,
        &camera,
        &tiledata,
        &StaticAnimations::default(),
        &static_atlas,
        &Cutaway::OPEN,
        &openshard_client_render::occlusion::Occlusion::EMPTY,
        None,
        None,
    )
    .quads;

    // A character standing where the camera looks, facing each way in turn, so
    // the picture shows both the placement and the mirrored facings.
    let mut anim = Anim::open(&dir).expect("anim.idx and anim.mul");
    let people: Vec<Mobile> = Direction::ALL
        .iter()
        .enumerate()
        .map(|(index, facing)| {
            let (x, y) = (centre.x - 3 + index as u16 % 4, centre.y - 3 + index as u16 / 4);
            // On the ground rather than at the camera's height: a mobile
            // standing below the terrain is *correctly* hidden by it, which is
            // what the first run of this dump showed.
            //
            // The tile's average and not its stored corner, which is where a
            // body actually stands (`WorldMap::average_land_z`): the corner is the
            // diamond's northern vertex, and on a slope standing at it is
            // standing under the floor — the ground sorts at that same average,
            // less two, so it is drawn over the body rather than beside it.
            let ground = Point::new(x, y, map.average_land_z(x, y).expect("inside the facet"));
            Mobile {
                at: ground,
                body: Graphic(400),
                group: AnimationGroup(4),
                facing: *facing,
                frame: AnimationFrameIndex(0),
                // Standing, so there is no second tile to sort between.
                from: None,
                corpse: false,
                hue: openshard_protocol::wire::Hue::NONE,
                // Standing where the server put them: nothing here is walking.
                drawn: openshard_client_render::follow::Gaze::on(ground),
                equipment: Vec::new().into(),
            }
        })
        .collect();
    let equip_conv = EquipConv::default();
    let mobile_atlas =
        AnimAtlas::build(&mut anim, mobiles::needed_animations(&people, &equip_conv)).expect("a body fits");
    let mobile_quads = mobiles::collect(&people, &camera, &mobile_atlas, &Cutaway::OPEN, &equip_conv, None);

    let frame = render_both(
        &device,
        &queue,
        &atlas,
        &texmaps,
        &quads,
        &static_atlas,
        &static_quads,
        (mobile_atlas.pixels(), &mobile_quads),
        camera.width,
        camera.height,
        camera.projection(),
    );

    let path = std::env::var_os("OPENSHARD_FRAME_DUMP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("britain.png"));
    std::fs::write(
        &path,
        openshard_client_render::png::encode_rgba(camera.width, camera.height, &frame.pixels),
    )
    .expect("writing the frame");
    eprintln!("wrote {}", path.display());
}

/// A sprite packed into an atlas *after* its renderer was built is drawn, and
/// drawn from its own pixels.
///
/// The load-bearing test for growing an atlas instead of rebuilding it. The
/// whole saving is that a growth uploads a band of rows rather than a 16MB
/// texture, and the band is the one thing in that arrangement with arithmetic in
/// it: `write_rows` cuts a slice out of the atlas and names a `y` to start it at,
/// and the two have to agree. If they do not, the sprite is drawn from whatever
/// the texture held there — which on a fresh atlas is transparent, so the
/// failure is a graphic that silently does not appear rather than one that
/// appears wrong.
///
/// The first sprite is 2,040 wide on purpose: it fills the shelf, so the second
/// starts a new row and the band has a non-zero origin. A band starting at zero
/// passes with the offset arithmetic missing entirely.
///
/// No client files: the pictures are this test's own.
#[test]
fn a_sprite_added_after_the_pass_was_built_is_drawn_from_the_rows_uploaded() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const SHELF_FILLER: Graphic = Graphic(1);
    const LATE: Graphic = Graphic(2);
    let (width, height) = (24u16, 18u16);
    let color = Color16(0b0_11111_00000_00000);

    let mut atlas = StaticAtlas::pack([(
        SHELF_FILLER,
        Image::new(2040, 40, vec![Color16(0b0_00000_11111_00000); 2040 * 40]),
    )])
    .expect("one wide sprite fits");

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let hue_ramp = HueRamp::build(&Hues::parse(&[0u8; 708]).expect("one empty group"));
    // Built from the atlas as it stands, which is the point: the pass below is
    // never rebuilt, exactly as `client/app` no longer rebuilds it.
    let mut statics = SpriteRenderer::new(&device, &queue, format, atlas.pixels(), &hue_ramp);

    atlas
        .pack_more([(
            LATE,
            Image::new(
                width,
                height,
                vec![color; usize::from(width) * usize::from(height)],
            ),
        )])
        .expect("a second sprite fits");
    let rows = atlas.take_dirty().expect("the growth wrote something");
    assert!(
        rows.clone().into_range().start > 0,
        "the second sprite should have started a new shelf"
    );
    statics.upload_rows(&queue, atlas.pixels(), rows);

    let sprite = atlas.sprite(LATE).expect("packed");
    let quads = [SpriteQuad {
        rect: Rect {
            x: 10.0,
            y: 12.0,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
        volumes: openshard_client_render::impostor::Range::default(),
    }];

    let (frame_width, frame_height) = (128u32, 128u32);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frame"),
        size: wgpu::Extent3d {
            width: frame_width,
            height: frame_height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(&device, frame_width, frame_height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(&device, frame_width, frame_height);
    let gbuffer_views = gbuffer.views();
    // The ground pass clears; the sprite pass loads what it left. Given nothing
    // to draw, it is the clear on its own.
    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let mut ground = GroundRenderer::new(&device, &queue, format, &land, &texmaps);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target_view = Target::whole(&view, &depth_view, &gbuffer_views, frame_width, frame_height);
    ground.render(&device, &queue, &mut encoder, target_view, &[]);
    statics.render(&device, &queue, &mut encoder, target_view, &quads, &[], None);
    queue.submit([encoder.finish()]);
    let frame = read_back(&device, &queue, &target);

    let Rgb8 {
        red: r,
        green: g,
        blue: b,
    } = color.rgb8();
    let mut drawn = 0;
    for y in 0..frame_height {
        for x in 0..frame_width {
            let inside =
                (10..10 + u32::from(width)).contains(&x) && (12..12 + u32::from(height)).contains(&y);
            let got = frame.pixel(x, y);
            if !inside {
                assert_eq!(got[3], 0, "({x}, {y}) is outside the sprite and was drawn");
                continue;
            }
            assert_eq!(
                got,
                [r, g, b, u8::MAX],
                "({x}, {y}) is not the late sprite's pixel"
            );
            drawn += 1;
        }
    }
    assert_eq!(drawn, usize::from(width) * usize::from(height));
}

/// A static page is a source texture, not a different kind of sprite: page one
/// has to paint its own pixels and stamp the ordinary static row into the
/// G-buffer.  Keeping both assertions together catches the tempting partial
/// implementation where page-batched colour draws but its G-buffer run is
/// accidentally left on page zero.
#[test]
fn a_second_static_atlas_page_draws_its_picture_and_gbuffer_row() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const FIRST: Graphic = Graphic(1);
    const SECOND: Graphic = Graphic(2);
    let first = Color16(0b0_11111_00000_00000);
    let second = Color16(0b0_00000_11111_00000);
    // Two full-width shelves that cannot share a page. This is deliberately a
    // geometry boundary rather than an item-count shortcut, because shelf
    // packing is the real condition that allocates a texture page.
    let image = |color| Image::new(2048, 1025, vec![color; 2048 * 1025]);
    let atlas = StaticAtlasPages::pack_with_limit([(FIRST, image(first)), (SECOND, image(second))], 2)
        .expect("two static pages fit the policy");
    assert_eq!(
        atlas.page_count(),
        2,
        "the fixture has to exercise more than one page"
    );
    let sprite = atlas.sprite(SECOND).expect("the second picture was packed");
    assert_eq!(
        sprite.page,
        StaticAtlasPage(1),
        "the test sprite must not be on the legacy page"
    );

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let hue_ramp = HueRamp::build(&Hues::parse(&[0u8; 708]).expect("one empty group"));
    let mut statics = SpriteRenderer::new_static_pages(&device, &queue, format, &atlas, &hue_ramp);
    let quads = [SpriteQuad {
        rect: Rect {
            x: 10.0,
            y: 12.0,
            width: 24.0,
            height: 18.0,
        },
        region: sprite.sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::of_static(Point::new(7, 9, 0)),
        twin: 0,
        owner: 0,
        volumes: Range::default(),
    }
    .with_static_atlas_page(sprite.page)];

    let (width, height) = (128u32, 128u32);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("paged static frame"),
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
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(&device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(&device, width, height);
    let gbuffer_views = gbuffer.views();
    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let mut ground = GroundRenderer::new(&device, &queue, format, &land, &texmaps);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target_view = Target::whole(&view, &depth_view, &gbuffer_views, width, height);
    ground.render(&device, &queue, &mut encoder, target_view, &[]);
    statics.render(&device, &queue, &mut encoder, target_view, &quads, &[], None);
    queue.submit([encoder.finish()]);

    let frame = read_back(&device, &queue, &target);
    let ids = read_back(&device, &queue, gbuffer.ids());
    let Rgb8 {
        red: r,
        green: g,
        blue: b,
    } = second.rgb8();
    for y in 12..30 {
        for x in 10..34 {
            assert_eq!(
                frame.pixel(x, y),
                [r, g, b, u8::MAX],
                "({x}, {y}) did not sample page one"
            );
            let id = u32::from_le_bytes(ids.pixel(x, y));
            assert_eq!(
                gbuffer::ids_kind(id),
                Some(Kind::Static),
                "({x}, {y}) lacks the static G-buffer kind"
            );
            assert_eq!(gbuffer::ids_id(id), 0, "({x}, {y}) names the wrong static row");
        }
    }
    assert_eq!(
        gbuffer::ids_kind(u32::from_le_bytes(ids.pixel(0, 0))),
        Some(Kind::Nothing)
    );
}

/// Draw `quads` into a world image, ring the ones in `outlined`, blit the lot
/// onto a surface and read the surface back.
///
/// The whole outline pipeline in one helper, in the order the client runs it:
/// the picture, then the silhouette mask against the picture's own depth, then
/// the blit, then the ring over it. A test that skipped a step would be
/// asserting about a pipeline nothing draws.
/// `width` and `height` are the *world image*'s, as the client's are; the
/// surface comes out at `zoom` of them, which is where a minified ring is
/// point-sampled and can lose half of itself. See `Ring::for_zoom`.
#[allow(clippy::too_many_arguments)]
fn render_outlined(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &StaticAtlas,
    quads: &[SpriteQuad],
    outlined: &[SpriteQuad],
    width: u32,
    height: u32,
    zoom: Zoom,
    ring: Ring,
) -> Frame {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let world = openshard_client_render::blit::world_texture(device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(device, width, height);
    let gbuffer_views = gbuffer.views();
    let mask = outline::mask_texture(device, width, height);
    let mask_view = mask.create_view(&wgpu::TextureViewDescriptor::default());
    let hue_ramp = HueRamp::build(&Hues::parse(&[0u8; 708]).expect("one empty group"));

    let target = Target::whole(&world_view, &depth_view, &gbuffer_views, width, height);
    let empty_land = LandAtlas::pack([]).expect("nothing always fits");
    let empty_texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    // The ground pass with nothing in it, purely to clear the world image: it is
    // the pass that owns the clear, and a world texture nobody cleared holds
    // whatever the driver left there.
    let mut ground_pass = GroundRenderer::new(device, queue, format, &empty_land, &empty_texmaps);
    let mut sprites = SpriteRenderer::new(device, queue, format, atlas.pixels(), &hue_ramp);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    ground_pass.render(device, queue, &mut encoder, target, &[]);
    sprites.render(device, queue, &mut encoder, target, quads, &[], None);
    // One quad, one ring — the item case, and what the tests below assert
    // about two sprites that touch.
    let rings: Vec<&[SpriteQuad]> = outlined.iter().map(std::slice::from_ref).collect();
    sprites.render_mask(device, queue, &mut encoder, target, &mask_view, &rings);

    let (surface_width, surface_height) = (
        width * zoom.numerator() / zoom.denominator(),
        height * zoom.numerator() / zoom.denominator(),
    );
    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width: surface_width,
            height: surface_height,
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
    let rect = ViewportRect {
        x: 0,
        y: 0,
        width: surface_width,
        height: surface_height,
    };
    // No mobile pass in this fixture: the dummy stands in for it.
    let dummy_instances = openshard_client_render::blit::dummy_instances(device);
    let dummy_mesh_instances = openshard_client_render::blit::dummy_mesh_instances(device);
    Blit::new(device, format).render(
        device,
        queue,
        &mut encoder,
        openshard_client_render::blit::Frame {
            target: &surface_view,
            world: &world_view,
            gbuffer: &gbuffer_views,
            face_instances: sprites.instances_buffer(),
            item_instances: sprites.instances_buffer(),
            mobile_instances: &dummy_instances,
            mesh_instances: &dummy_mesh_instances,
            // Empty, like the ground pass drawn above — a real buffer of no
            // rows rather than a dummy, the same reason that pass was given
            // `&[]` instead of being skipped.
            ground_instances: ground_pass.instances_buffer(),
            zoom,
            rect,
        },
        &Lighting::NONE,
    );
    Outline::new(device, format).render(
        device,
        queue,
        &mut encoder,
        openshard_client_render::outline::Frame {
            target: &surface_view,
            mask: &mask_view,
            mask_size: (width, height),
            rect,
        },
        ring,
    );
    queue.submit([encoder.finish()]);

    read_back(device, queue, &surface)
}

/// A solid square of one colour, packed alone.
fn square(graphic: Graphic, side: u16, color: Color16) -> StaticAtlas {
    StaticAtlas::pack([(
        graphic,
        Image::new(side, side, vec![color; usize::from(side) * usize::from(side)]),
    )])
    .expect("one sprite fits")
}

/// The ring is exactly the pixels next to the silhouette and outside it — and
/// the sprite itself is left alone.
///
/// Both halves are the assertion. A dilation that drew the *whole* grown shape
/// instead of the grown-minus-original ring passes any test that only looks at
/// the border, and it covers the art it was supposed to be pointing at.
#[test]
fn a_ring_is_drawn_around_a_silhouette_and_not_over_it() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    const SIDE: u16 = 16;
    let green = Color16(0b0_00000_11111_00000);
    let atlas = square(GRAPHIC, SIDE, green);
    let sprite = atlas.sprite(GRAPHIC).expect("packed");
    let (x, y) = (40.0, 50.0);
    let quads = [SpriteQuad {
        rect: Rect {
            x,
            y,
            width: f32::from(SIDE),
            height: f32::from(SIDE),
        },
        region: sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
        volumes: openshard_client_render::impostor::Range::default(),
    }];

    let (width, height) = (128, 128);
    let frame = render_outlined(
        &device,
        &queue,
        &atlas,
        &quads,
        &quads,
        width,
        height,
        Zoom::ONE,
        Ring::DEFAULT,
    );

    let Rgb8 {
        red: green_r,
        green: green_g,
        blue: green_b,
    } = green.rgb8();
    let white = [u8::MAX; 4];
    let (left, top) = (x as u32, y as u32);
    let (right, bottom) = (left + u32::from(SIDE), top + u32::from(SIDE));
    let mut ringed = 0;
    for py in 0..height {
        for px in 0..width {
            let inside = (left..right).contains(&px) && (top..bottom).contains(&py);
            // One pixel out on every side, corners included: an eight-tap
            // neighbourhood rings the diagonal too, and a four-tap one does not
            // — which is the difference between a closed ring and one with four
            // holes in it.
            let bordering = (left - 1..right + 1).contains(&px) && (top - 1..bottom + 1).contains(&py);
            let got = frame.pixel(px, py);
            if inside {
                assert_eq!(
                    got,
                    [green_r, green_g, green_b, u8::MAX],
                    "({px}, {py}) is inside the sprite and the ring painted over it",
                );
            } else if bordering {
                assert_eq!(got, white, "({px}, {py}) borders the sprite and was not ringed");
                ringed += 1;
            } else {
                assert_eq!(got[3], 0, "({px}, {py}) is nowhere near the sprite");
            }
        }
    }
    // The frame of a 16x16 square grown by one: 18² - 16².
    assert_eq!(ringed, 18 * 18 - 16 * 16);
}

/// Two outlined sprites that touch keep one ring each.
///
/// This is the whole reason the mask holds an *id* rather than a coverage bit.
/// With coverage the shared edge is interior to the union — every neighbour of
/// it is "drawn" — so no ring is grown there and the pair comes out outlined as
/// a single blob. The seam below is the pixel column where that shows.
#[test]
fn two_touching_silhouettes_are_ringed_separately() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    const SIDE: u16 = 16;
    let green = Color16(0b0_00000_11111_00000);
    let atlas = square(GRAPHIC, SIDE, green);
    let sprite = atlas.sprite(GRAPHIC).expect("packed");
    let (x, y) = (40.0f32, 50.0f32);
    // Edge to edge, sharing no pixel: the left one ends where the right begins.
    let quads: Vec<SpriteQuad> = [x, x + f32::from(SIDE)]
        .into_iter()
        .map(|at| SpriteQuad {
            rect: Rect {
                x: at,
                y,
                width: f32::from(SIDE),
                height: f32::from(SIDE),
            },
            region: sprite.region,
            depth: 0.5,
            hue: 0,
            place: Place::NOWHERE,
            twin: 0,
            owner: 0,
            volumes: openshard_client_render::impostor::Range::default(),
        })
        .collect();

    let (width, height) = (128, 128);
    let frame = render_outlined(
        &device,
        &queue,
        &atlas,
        &quads,
        &quads,
        width,
        height,
        Zoom::ONE,
        Ring::DEFAULT,
    );

    let white = [u8::MAX; 4];
    let seam = x as u32 + u32::from(SIDE);
    let middle = y as u32 + u32::from(SIDE) / 2;
    assert_eq!(
        frame.pixel(seam - 1, middle),
        white,
        "the left sprite's own edge against the right one was not ringed — \
         the mask is behaving like coverage rather than an identity",
    );
    assert_eq!(
        frame.pixel(seam, middle),
        white,
        "and neither was the right sprite's edge against the left one",
    );
    // The outer edges are still there: a rule that only ever fired between two
    // ids would ring the seam and nothing else.
    assert_eq!(frame.pixel(x as u32 - 1, middle), white, "the pair's left edge");
    assert_eq!(
        frame.pixel(x as u32 + 2 * u32::from(SIDE), middle),
        white,
        "the pair's right edge",
    );
}

/// The glow reaches past the ring, fades with distance, and leaves the art it
/// is pointing at alone.
///
/// All three halves are the assertion, and the third is the one a blur gets
/// wrong: an additive wash that covered the silhouette too would brighten
/// exactly the pixels the player is being asked to look at, and it would still
/// look like a glow in a screenshot.
#[test]
fn a_glow_reaches_past_the_ring_and_fades_with_distance() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    const SIDE: u16 = 16;
    let green = Color16(0b0_00000_11111_00000);
    let atlas = square(GRAPHIC, SIDE, green);
    let sprite = atlas.sprite(GRAPHIC).expect("packed");
    let (x, y) = (40.0, 50.0);
    let quads = [SpriteQuad {
        rect: Rect {
            x,
            y,
            width: f32::from(SIDE),
            height: f32::from(SIDE),
        },
        region: sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
        volumes: openshard_client_render::impostor::Range::default(),
    }];

    let (width, height) = (128, 128);
    let frame = render_outlined(
        &device,
        &queue,
        &atlas,
        &quads,
        &quads,
        width,
        height,
        Zoom::ONE,
        Ring::SOFT,
    );

    let Rgb8 {
        red: green_r,
        green: green_g,
        blue: green_b,
    } = green.rgb8();
    let right = x as u32 + u32::from(SIDE);
    let middle = y as u32 + u32::from(SIDE) / 2;
    // Just past the ring, and further out. Read on the red channel: the glow is
    // white and the background is the ground pass's cleared black, so every
    // channel carries the same number and one of them is the measurement.
    let near = frame.pixel(right + 1, middle)[0];
    let far = frame.pixel(right + 4, middle)[0];
    assert!(near > 0, "nothing is lit one pixel past the ring");
    assert!(
        near > far,
        "the glow does not fade: {near} at one pixel out against {far} at four",
    );
    assert!(far > 0, "the glow stops dead at four pixels rather than fading");
    // Well past `Glow::DEFAULT`'s reach of six.
    assert_eq!(
        frame.pixel(right + 24, middle)[0],
        0,
        "the glow reaches 24 pixels, which is most of a static",
    );
    // And the sprite is exactly its own colour still.
    assert_eq!(
        frame.pixel(x as u32 + 2, middle),
        [green_r, green_g, green_b, u8::MAX],
        "the glow was added over the art it is pointing at",
    );
}

/// A minified ring keeps all four of its sides.
///
/// The mask is the world image and the composite reads it at the *surface*'s
/// resolution, so below 1:1 it is point-sampled: at `1/2` only every other mask
/// texel is ever looked at, and a one-texel ring loses whichever of its sides
/// falls on the parity nothing samples. `Ring::for_zoom` is the fix and the
/// second half of this test is why it is not imagined — the same frame with a
/// fixed one-texel ring comes back with edges missing.
#[test]
fn a_minified_ring_keeps_every_side() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    const SIDE: u16 = 16;
    let green = Color16(0b0_00000_11111_00000);
    let atlas = square(GRAPHIC, SIDE, green);
    let sprite = atlas.sprite(GRAPHIC).expect("packed");
    let (x, y) = (40.0, 50.0);
    let quads = [SpriteQuad {
        rect: Rect {
            x,
            y,
            width: f32::from(SIDE),
            height: f32::from(SIDE),
        },
        region: sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
        volumes: openshard_client_render::impostor::Range::default(),
    }];

    let (width, height) = (128, 128);
    // Half, the widest rung of the ladder and the worst case for this.
    let zoom = Zoom::ONE.scale_down().scale_down().scale_down();
    assert_eq!(
        (zoom.numerator(), zoom.denominator()),
        (1, 2),
        "the rung this is about"
    );

    // Where each side of the ring lands on a half-sized surface: one screen
    // pixel outside the sprite's own screen rectangle, on the middle of each
    // side.
    let (left, top) = (x as u32 / 2, y as u32 / 2);
    let side = u32::from(SIDE) / 2;
    let (middle_x, middle_y) = (left + side / 2, top + side / 2);
    let edges = [
        ("left", left - 1, middle_y),
        ("right", left + side, middle_y),
        ("top", middle_x, top - 1),
        ("bottom", middle_x, top + side),
    ];

    let lit = |ring: Ring| {
        let frame = render_outlined(&device, &queue, &atlas, &quads, &quads, width, height, zoom, ring);
        edges
            .into_iter()
            .filter(|(_, px, py)| frame.pixel(*px, *py) == [u8::MAX; 4])
            .map(|(name, _, _)| name)
            .collect::<Vec<_>>()
    };

    assert_eq!(
        lit(Ring::DEFAULT.for_zoom(zoom)).len(),
        edges.len(),
        "a ring widened for the zoom lost a side",
    );
    // The companion: without it the ring is not merely thinner, it is gone on
    // two sides — so the assertion above is measuring something.
    let naive = lit(Ring::DEFAULT);
    assert!(
        naive.len() < edges.len(),
        "a one-texel ring survived being point-sampled at half, so this test \
         proves nothing: {naive:?}",
    );
}

/// Write the soft highlight out as a picture, for a person to look at.
///
/// Ignored, for the reason [`dump_a_frame_of_britain`] is: the tests above count
/// the glow's pixels and counting cannot say whether it *reads* as a highlight.
/// A grey slab stands in for the world behind it, because a glow is additive and
/// what matters is how far it lifts a picture that is already there.
///
/// ```sh
/// cargo test -p openshard-client-render --test frame -- --ignored dump_a_glow
/// ```
#[test]
#[ignore = "writes a picture for a person, and asserts nothing"]
fn dump_a_glowing_sprite() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const BACKDROP: Graphic = Graphic(1);
    const ITEM: Graphic = Graphic(2);
    let grey = Color16(0b0_01100_01100_01100);
    let green = Color16(0b0_00110_11000_00110);
    let atlas = StaticAtlas::pack([
        (BACKDROP, Image::new(96, 96, vec![grey; 96 * 96])),
        (ITEM, Image::new(20, 20, vec![green; 20 * 20])),
    ])
    .expect("two sprites fit");

    let quad = |graphic: Graphic, x: f32, y: f32, side: f32, depth: f32| SpriteQuad {
        rect: Rect {
            x,
            y,
            width: side,
            height: side,
        },
        region: atlas.sprite(graphic).expect("packed").region,
        depth,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
        volumes: openshard_client_render::impostor::Range::default(),
    };
    let backdrop = quad(BACKDROP, 16.0, 16.0, 96.0, 0.9);
    let item = quad(ITEM, 54.0, 54.0, 20.0, 0.5);

    let frame = render_outlined(
        &device,
        &queue,
        &atlas,
        &[backdrop, item],
        &[item],
        128,
        128,
        Zoom::ONE,
        Ring::SOFT,
    );

    let path = std::env::var_os("OPENSHARD_FRAME_DUMP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("glow.png"));
    std::fs::write(
        &path,
        openshard_client_render::png::encode_rgba(128, 128, &frame.pixels),
    )
    .expect("writing the frame");
    eprintln!("wrote {}", path.display());
}

/// How many pixels of the parity frame make one tile.
///
/// Eight, so that a 64-pixel frame is eight tiles across — the room in
/// `scene::room` plus a ring of street around it — and so that the sub-tile
/// fraction takes eight distinct values down each tile rather than being a
/// constant the shader could ignore without anybody noticing.
const PARITY_TILE: u32 = 8;

/// Where the parity frame says a pixel is: a tile, and where in it.
///
/// The frame is laid out as a plain grid of tiles rather than as a projection.
/// Nothing here is testing the projection — what is being compared is one
/// formula against another, and a synthetic layout means the *expected* value
/// comes from a tile this function names rather than from a second copy of the
/// camera's arithmetic.
fn parity_place(px: u32, py: u32) -> (u16, u16, f32, f32) {
    let (cx, cy) = (
        openshard_client_render::scene::CENTRE.x,
        openshard_client_render::scene::CENTRE.y,
    );
    let tile_x = cx - 4 + (px / PARITY_TILE) as u16;
    let tile_y = cy - 4 + (py / PARITY_TILE) as u16;
    // Sixteen hundred-and-twenty-sevenths of a tile a pixel. The grain is what
    // the retired attachment's seven-bit fraction could hold exactly, kept
    // rather than rounded off to eighths so that these fixtures' numbers did not
    // all move on the day the position became a float — a parity margin re-taken
    // for a reason that is not the one under test is a margin nobody can read.
    let step = |along: u32| (along % PARITY_TILE) as f32 * 16.0 / 127.0;
    (tile_x, tile_y, step(px), step(py))
}

/// What every pixel of a parity fixture *is*: a surface, at a height, of an
/// occluder.
///
/// The three travel together because they are one statement about the fragment
/// and the two sides have to make the same one — the attachment says it to the
/// shader and the [`Spot`](openshard_client_render::light::Spot) says it to
/// `light::sample`, and a fixture that set them apart could tell the two
/// different stories without failing to compile.
#[derive(Clone, Copy)]
struct Fixture {
    surface: Surface,
    z: i8,
    /// Which **solid** of the grid every pixel is a point of.
    ///
    /// [`None`] for every fixture that predates sub-tile lids, and that is the
    /// honest default: those scenes are flat ground and walls, where a pixel is
    /// a point of nothing and identity decides nothing. A fixture whose scene
    /// has a *flight* in it must say otherwise — a tread's top is excused from
    /// its own lid by identity alone, and without one the fragment is shadowed
    /// by the very step it stands on and every other question about it is
    /// unreachable.
    ///
    /// **A `SolidId` and not an `OwnerId`, which is what the shader compares** —
    /// `docs/lighting_rebuild.md` phase 4. It was the coarser one until the
    /// solid came to ride in the position plane (`solid_format.wesl`): the
    /// shader took an owner off the instance row and narrowed it per fragment by
    /// the stance, which is exact for a wall and ambiguous by construction for a
    /// flight — three lids, one owner, one flat stance — so which tread a flat
    /// pixel came out a point of was the grid's own reference order. This states
    /// it instead.
    ///
    /// **One solid for the whole frame**, which is the fixture's own limit and
    /// not the format's: every pixel here is written from one `Fixture`, so a
    /// scene whose pixels stand on *different* solids says so by choosing which
    /// one it is asking about. `the_shader_does_not_stop_a_vertical_ray_with_a_
    /// lid_it_is_not_under` is the one test in that shape, and its two pixels
    /// are picked around exactly this: the lit one really is on the tread named
    /// here, and the control one is not and is blocked by its own.
    solid: Option<SolidId>,
    /// How far past the sub-tile fraction every fragment's *position* is written,
    /// while its **tile stays what [`parity_place`] says**.
    ///
    /// A fixture's fragments are otherwise inside their own tile by construction
    /// — the fraction runs to `112/127` and never reaches an edge — so the one
    /// state a real frame is full of since `docs/lighting_rebuild.md` phase 6c
    /// could not be stated here at all: a position on, or a rounding past, the
    /// boundary its own instance's tile ends at. That is what the impostor writes
    /// for every south and east face, and a share of them land a hair over.
    ///
    /// Zero for every fixture that is not about it. It exists to make a
    /// fragment's position and its carried tile disagree, which
    /// `docs/occluders.md`'s S4 turned from a rule's own subject into a fact
    /// nothing downstream may read: the walk seeds itself from the position and
    /// the tile is not passed to it at all.
    drift: (f32, f32),
}

impl Fixture {
    /// Flat ground at `z = 0`, a point of nothing: what every parity scene was
    /// before there was anything else to be.
    fn ground() -> Self {
        Self {
            surface: Surface::Upright,
            z: 0,
            solid: None,
            drift: (0.0, 0.0),
        }
    }
}

/// One frame of the parity fixture: a white world, a place attachment this test
/// wrote, and `scene::room`'s lighting drawn in `view`.
///
/// White because the blit multiplies the art by the lighting: with every channel
/// at one, what comes out *is* the multiplier, clamped — so a mismatch is a
/// mismatch in the lighting and not in a sprite.
fn parity_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    lighting: &Lighting,
    width: u32,
    height: u32,
    fixture: Fixture,
) -> Frame {
    let Fixture {
        surface,
        z,
        solid,
        drift,
    } = fixture;
    let world = openshard_client_render::blit::world_texture(device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(device, width, height);
    let gbuffer_views = gbuffer.views();

    // Neither `Kind::Static` nor `Kind::Land` pixels carry their own `x`/`y`
    // in the attachment any more — `docs/gbuffer.md` step 3 moved a static's
    // to a row this fixture has to build itself, the same way `statics.wgsl`
    // would have, and step 7 did the same for the ground. One row per
    // distinct tile the sweep below actually uses, keyed by first sight: the
    // sweep visits the same handful of tiles many times over (`PARITY_TILE`
    // pixels apiece), and a row per *pixel* would be the id repeating the
    // very repetition decision 2 exists to remove.
    let mut face_ids: std::collections::HashMap<(u16, u16), u32> = std::collections::HashMap::new();
    let mut face_rows: Vec<u8> = Vec::new();
    let mut id_of = |x: u16, y: u16| -> u32 {
        *face_ids.entry((x, y)).or_insert_with(|| {
            let id = (face_rows.len() as u64 / openshard_client_render::sprite::SpriteQuad::STRIDE) as u32;
            SpriteQuad {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                },
                region: openshard_client_render::atlas::Region {
                    u: 0.0,
                    v: 0.0,
                    du: 0.0,
                    dv: 0.0,
                },
                depth: 0.0,
                hue: 0,
                place: openshard_client_render::place::Place::land(x, y),
                // This sweep never asks for a corner `Stance`, so there is
                // never a second half to point at — see
                // `crate::sprite::split_corners` for the real pass's row.
                twin: 0,
                // **Never read by the shader any more**, and the row carries it
                // only because `SpriteQuad` has the field: what a fragment is a
                // point of rides in the position plane now, per fragment, and
                // `blit.wesl`'s owner-and-stance narrowing went with the scan
                // that used it. `OwnerId::NONE`'s own word, which is what a row
                // that means nothing by it should say.
                owner: u32::from(OwnerId::NONE.raw()),
                volumes: openshard_client_render::impostor::Range::default(),
            }
            .write(&mut face_rows);
            id
        })
    };

    // `Kind::Land` pixels need the same treatment since `docs/gbuffer.md`
    // step 7 — one row per distinct tile, keyed by first sight, the ground
    // half of `id_of` above.
    let mut ground_ids: std::collections::HashMap<(u16, u16), u32> = std::collections::HashMap::new();
    let mut ground_rows: Vec<u8> = Vec::new();
    let mut ground_id_of = |x: u16, y: u16| -> u32 {
        *ground_ids.entry((x, y)).or_insert_with(|| {
            let id = (ground_rows.len() as u64 / openshard_client_render::ground::GroundQuad::STRIDE) as u32;
            openshard_client_render::ground::GroundQuad {
                x: 0.0,
                y: 0.0,
                corners: [0.0; 4],
                region: openshard_client_render::atlas::Region {
                    u: 0.0,
                    v: 0.0,
                    du: 0.0,
                    dv: 0.0,
                },
                texmap: None,
                depth: 0.0,
                place: openshard_client_render::place::Place::land(x, y),
            }
            .write(&mut ground_rows);
            id
        })
    };

    let mut ids: Vec<u32> = Vec::with_capacity((width * height) as usize);
    let mut positions: Vec<f32> = Vec::with_capacity((width * height * 4) as usize);
    let mut normals: Vec<u32> = Vec::with_capacity((width * height) as usize);
    for py in 0..height {
        for px in 0..width {
            let (x, y, sub_x, sub_y) = parity_place(px, py);
            // Land at `z = 0` — the ground of the room — unless the fixture is
            // about a wall's face, in which case every pixel is a static
            // standing on that face.
            //
            // The stance is what the facing test reads, and a fixture without one
            // would leave the whole of that test uncompared: `light::sample` would
            // agree with the shader about a formula neither of them ran.
            let (kind, stance) = match surface {
                // Land with no stance: what every fixture that predates surfaces
                // is, and a billboard's answer — nothing is known about which way
                // it looks, so every flame that reaches it lights it.
                Surface::Upright => (
                    openshard_client_render::place::Kind::Land,
                    openshard_client_render::place::Stance::Upright,
                ),
                // A floor, a rug, the top of a wall: it looks up, and that is the
                // fixture decision 27 needed. Without one the shader could return
                // any normal at all for a flat pixel and every parity test here
                // would still pass.
                Surface::Flat => (
                    openshard_client_render::place::Kind::Static,
                    openshard_client_render::place::Stance::Flat,
                ),
                Surface::Face(face) => (
                    openshard_client_render::place::Kind::Static,
                    openshard_client_render::place::Stance::face(face),
                ),
            };
            // Both planes off one statement about the fragment — see
            // `openshard_client_render::gbuffer::Fragment`, which is the format
            // itself and not this fixture's reading of it.
            let fragment = openshard_client_render::gbuffer::Fragment {
                tile: (x, y),
                // The tile is the row's and the fraction is the fixture's — the
                // asymmetry `drift` exists to state, and the one a real static's
                // own impostor produces.
                sub: (sub_x + drift.0, sub_y + drift.1),
                z: f32::from(z),
                kind,
                stance,
                // The fixture's own, so the shader is told what `Spot` is told —
                // one statement about the fragment, written into the plane the
                // real passes write it into.
                solid,
            };
            // A static's or a mobile's *tile* comes from the row — that is what
            // the id plane names, and the position plane keeps saying where the
            // fragment itself is.
            let id = match kind {
                openshard_client_render::place::Kind::Static => id_of(x, y),
                _ => ground_id_of(x, y),
            };
            ids.push(fragment.ids(id));
            positions.extend_from_slice(&fragment.position());
            normals.push(fragment.normal());
        }
    }
    let upload = |texture: &wgpu::Texture, bytes: &[u8], stride: u32| {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * stride),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    };
    let bytes: Vec<u8> = ids.iter().flat_map(|word| word.to_le_bytes()).collect();
    upload(gbuffer.ids(), &bytes, 4);
    let bytes: Vec<u8> = positions.iter().flat_map(|value| value.to_le_bytes()).collect();
    upload(gbuffer.position(), &bytes, 16);
    let bytes: Vec<u8> = normals.iter().flat_map(|word| word.to_le_bytes()).collect();
    upload(gbuffer.normal(), &bytes, 4);

    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: openshard_client_render::blit::WORLD_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let surface_view = surface.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    // The white world, as a clear rather than as an upload: a render pass that
    // stores its clear is the one way to fill a texture that is a render target
    // and not a copy destination.
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("white world"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &world_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        multiview_mask: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    let mut blit = Blit::new(device, openshard_client_render::blit::WORLD_FORMAT);
    let dummy_instances = openshard_client_render::blit::dummy_instances(device);
    let dummy_mesh_instances = openshard_client_render::blit::dummy_mesh_instances(device);
    let dummy_ground_instances = openshard_client_render::blit::dummy_ground_instances(device);
    // Only built above when the fixture used `Kind::Static` at all — a fixture
    // built entirely from `Surface::Flat`/`Surface::Face` has nothing for the
    // dummy to stand in for.
    let face_instances = if face_rows.is_empty() {
        None
    } else {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("parity face instances"),
            size: face_rows.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, &face_rows);
        Some(buffer)
    };
    // The mirror of `face_instances`: built only when the fixture used
    // `Surface::Upright`'s `Kind::Land` — every fixture but the ones that
    // never leave a wall's face.
    let ground_instances = if ground_rows.is_empty() {
        None
    } else {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("parity ground instances"),
            size: ground_rows.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, &ground_rows);
        Some(buffer)
    };
    blit.render(
        device,
        queue,
        &mut encoder,
        openshard_client_render::blit::Frame {
            target: &surface_view,
            world: &world_view,
            gbuffer: &gbuffer_views,
            face_instances: face_instances.as_ref().unwrap_or(&dummy_instances),
            item_instances: &dummy_instances,
            // No mobile pixels in this fixture: the dummy stands in for it.
            mobile_instances: &dummy_instances,
            mesh_instances: &dummy_mesh_instances,
            ground_instances: ground_instances.as_ref().unwrap_or(&dummy_ground_instances),
            zoom: Zoom::ONE,
            rect: ViewportRect {
                x: 0,
                y: 0,
                width,
                height,
            },
        },
        lighting,
    );
    queue.submit([encoder.finish()]);
    read_back(device, queue, &surface)
}

/// **The shader reads a primitive's own corners, at a fraction of a tile no
/// byte could name** — `docs/occluders.md`'s S1 gate, the shader's third of it.
///
/// `light::a_primitive_at_no_fraction_a_byte_could_name_reads_the_same_three_ways`
/// is the same fixture put to the two CPU walks and to a brute-force oracle over
/// every primitive; this is the same box, on the GPU, and the coordinates are
/// stated here the same way for the same reason — half a step off the `1/255`
/// grid a footprint used to be quantised on, which is the point that grid is
/// maximally wrong about.
///
/// **The sun and not a flame, and that is what makes the claim resolvable at
/// all.** A flame is a sphere and a fragment casts eight rays at it, so the
/// bundle spreads by `FLAME_RADIUS * t` at the crossing — a twentieth of a tile
/// for any ordinary geometry, forty times the half-step this fixture is aiming
/// inside. The sun is one exact ray in one direction, so a hair either side of a
/// face is a hair either side of the answer. Due east and level: `y` and `z` stay
/// what the fragment's own are for the whole run, so which side of the box's
/// north face the ray passes is the only thing deciding it.
///
/// Two frames, differing in [`Fixture::drift`] alone and by a thousandth of a
/// tile: the fragment a half-thousandth **inside** the box's north face is
/// shadowed and the one a half-thousandth **outside** it is not. Put the byte
/// quantisation back on the wire and the face moves nearly two thousandths
/// further in — past both — so both frames come out sunlit and the pair stops
/// being a pair.
#[test]
fn the_shader_reads_a_primitive_at_no_fraction_a_byte_could_name() {
    let Some((device, queue)) = gpu() else {
        return;
    };

    // The same construction as the CPU gate's, and the same numbers: half a step
    // off the byte grid across, half a step off the sixteen-bit grid up.
    let across = |units: f64| (units + 0.5) / 255.0;
    let up = |steps: f64| (steps + 0.5) / 256.0 - 128.0;
    let (cx, cy) = (
        openshard_client_render::scene::CENTRE.x,
        openshard_client_render::scene::CENTRE.y,
    );
    // Thin across the sun's own run, which keeps the ray inside the box for a
    // fiftieth of a tile: nothing about this test depends on that, and it is
    // what makes the fixture readable as "a slab standing in the way" rather
    // than as a tile that happens to be narrowed.
    let (min_x, max_x) = (f64::from(cx) + across(30.0), f64::from(cx) + across(35.0));
    let (min_y, max_y) = (f64::from(cy) + across(74.0), f64::from(cy) + across(200.0));
    let (min_z, max_z) = (up(33_049.0), up(35_000.0));

    let mut builder = Builder::new(TileBounds {
        min_x: i32::from(cx) - 10,
        max_x: i32::from(cx) + 10,
        min_y: i32::from(cy) - 10,
        max_y: i32::from(cy) + 10,
    });
    builder.add_raw(
        cx,
        cy,
        openshard_client_render::solid::Solid {
            min: openshard_client_render::camera::WorldSpot {
                x: min_x,
                y: min_y,
                z: min_z,
            },
            max: openshard_client_render::camera::WorldSpot {
                x: max_x,
                y: max_y,
                z: max_z,
            },
        },
        openshard_client_render::occlusion::Owner::new(0, Graphic(1)),
    );
    let occlusion = builder.finish(&Cutaway::OPEN);

    // The pixel: the tile west of the box's, so the fragment's own tile is not
    // the one the sun's walk exempts. `px % PARITY_TILE == 0` puts its sub-tile
    // fraction at zero, so the drift below *is* its position within the tile.
    const AT: (u32, u32) = (24, 32);
    let (tile_x, tile_y, sub_x, sub_y) = parity_place(AT.0, AT.1);
    assert_eq!(
        (tile_x + 1, tile_y, sub_x, sub_y),
        (cx, cy, 0.0, 0.0),
        "the fragment has to sit on the tile west of the box, at its own corner",
    );

    // A twentieth of the old wire's own half step across a tile: inside the
    // blind spot a byte leaves, and nowhere near an `f32`'s own out here at a
    // hundred tiles.
    const OFF: f64 = 0.0005;
    // Due east and level: the ray keeps the fragment's `y` and `z` for its whole
    // run, so the box's north face is the only thing that can decide it.
    let sun = openshard_client_render::light::Sun::towards(1.0, 0.0, 0.0, [1.0, 1.0, 1.0], 3.0);

    let read = |offset: f64| {
        let lighting = Lighting {
            ambient: openshard_client_render::light::NIGHT,
            lights: Vec::new(),
            occlusion: occlusion.clone(),
            sun: Some(sun),
            view: View::default(),
            flame_radius: openshard_client_render::light::FLAME_RADIUS,
            shadow_rays: openshard_client_render::light::ShadowRays::DEFAULT,
            dead: false,
        };
        let fixture = Fixture {
            surface: Surface::Upright,
            // Inside the box's own span, which runs from just over 1 to just
            // under 9: a ray level with the fragment crosses it in `z`.
            z: 4,
            solid: None,
            drift: (0.5, (min_y - f64::from(cy) + offset) as f32),
        };
        let frame = parity_frame(&device, &queue, &lighting, 64, 64, fixture);
        i32::from(frame.pixel(AT.0, AT.1)[0])
    };

    let inside = read(OFF);
    let outside = read(-OFF);
    assert!(
        outside > inside + 40,
        "the fragment a half-thousandth of a tile north of the box's own face reads \
         {outside} in the sun, and the one a half-thousandth inside it reads {inside}: \
         the shader is not reading the face where the record puts it",
    );
}

/// **A vertical ray on the GPU is stopped only by lids it is actually under.**
///
/// `light::a_vertical_ray_is_not_stopped_by_lids_it_is_not_over` is the same
/// claim about the two CPU walks; this is the shader's own copy of the shortcut,
/// and it needed a fixture of its own because **nothing here could see it**.
/// Deleting the gate from `blit.wesl` alone left all forty-seven frame tests
/// green: no parity scene has a sub-tile lid in it, so the branch that reads one
/// was never run. The gap was in the harness, not in the sweep's size.
///
/// The frame is read **directly**, and that is the whole of it now. This used to
/// **The ray count on the wire is the count the shader casts.**
///
/// `light::ShadowRays` is a number a person turns in the Light tab, and it
/// travels as one word of the blit's own header — `blit.rs`'s `lighting_bytes`,
/// read back by `blit.wesl`'s `shadow_rays`. Everything between the slider and
/// the loop is untyped: a word written at the wrong offset, or a shader that
/// went on reading a constant, is a knob that moves nothing and says nothing.
///
/// So the claim is the *difference*: one ray and eight, over one scene at one
/// instant, must not draw the same frame. They cannot — one ray is a hard
/// shadow with no penumbra at all, eight is a gradient — and a shader ignoring
/// the header draws the same picture twice. The second half is that eight and
/// eight *do* draw the same frame, which is what says the first half found a ray
/// count rather than a frame that wobbles on its own.
///
/// `scene::room` is the fixture for the reason every parity test here uses it: a
/// torch inside a walled ring, so the frame has a wall's own shadow edge across
/// it rather than an open pool where every ray of a flame arrives whatever the
/// count.
#[test]
fn the_shader_casts_as_many_rays_as_the_frame_asks_for() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let (width, height) = (64, 64);
    let mut lighting = openshard_client_render::scene::room().lighting(0.0);
    // A flame a tile across, against the art's own eighth of one: the penumbra a
    // shadow's edge has *is* the disagreement between the rays, so a body too
    // small to disagree over makes this test about nothing — measured, at the
    // real `FLAME_RADIUS` the two frames are seventeen pixels apart, which is a
    // shadow edge one fixture-pixel wide and no bar worth setting. It is the same
    // knob the Light tab's "flame size" is, turned to where the thing under test
    // is visible.
    lighting.flame_radius = 1.0;

    let draw = |rays: u32| {
        let lighting = Lighting {
            shadow_rays: openshard_client_render::light::ShadowRays::new(rays),
            ..lighting.clone()
        };
        parity_frame(&device, &queue, &lighting, width, height, Fixture::ground())
    };
    let one = draw(1);
    let eight = draw(8);
    let again = draw(8);

    let differing = |a: &Frame, b: &Frame| {
        (0..height)
            .flat_map(|py| (0..width).map(move |px| (px, py)))
            .filter(|&(px, py)| a.pixel(px, py) != b.pixel(px, py))
            .count()
    };
    assert_eq!(
        differing(&eight, &again),
        0,
        "one frame drawn twice is not the same frame, so the comparison below \
         cannot mean anything",
    );
    // A hundred of four thousand is far below what a whole penumbra is and far
    // above the nothing a dead knob draws — the bar is "this is not noise".
    // Measured: 2,601 pixels move between one ray and eight.
    let moved = differing(&one, &eight);
    assert!(
        moved > 100,
        "one ray and eight drew the same picture but for {moved} pixels: the \
         count is not reaching the shader",
    );
}

/// stand beside a parity sweep of the shader against `light::sample`, and the
/// sweep was the circular half: both sides were fixed together, so it reported
/// agreement whether or not either was right. What is left is the claim neither
/// walk gets a vote on — the pixel over the bottom tread is lit and the pixel
/// over the middle one is not, which is a statement about this scene's geometry.
///
/// Why those two pixels. Every point of the tile is over exactly one tread — the
/// three strips tile it — so "a lid the ray is not under" is only reachable from
/// a fragment standing on a tread of its own, excused from that one lid by
/// identity and asking about the other two. Hence [`Fixture::owner`]: without it
/// the fragment is shadowed by the step it stands on and the question never
/// arises. The middle-tread pixel is the control, blocked by its own tread's lid
/// two units under it, and it is what says the scene occludes at all.
#[test]
fn the_shader_does_not_stop_a_vertical_ray_with_a_lid_it_is_not_under() {
    let Some((device, queue)) = gpu() else {
        return;
    };

    // The three-tread flight of `light`'s own copy of this test: north, so the
    // treads divide the tile up `y` — tread 0 over `100.667..101` capped at
    // `z 1`, tread 1 over `100.333..100.667` at `z 3`, tread 2 over
    // `100..100.333` at `z 5`.
    let stair = openshard_tiles::StaticTile {
        flags: openshard_tiles::TileFlags::new(
            openshard_tiles::TileFlags::NO_SHOOT | openshard_tiles::TileFlags::CLIMBABLE,
        ),
        height: 20,
        ..openshard_tiles::StaticTile::default()
    };
    let prism =
        openshard_client_render::facing::Prism::new(openshard_client_render::facing::Face::North, &[1, 3, 5])
            .expect("three treads");
    let (cx, cy) = (
        openshard_client_render::scene::CENTRE.x,
        openshard_client_render::scene::CENTRE.y,
    );
    let mut builder = Builder::new(TileBounds {
        min_x: i32::from(cx) - 10,
        max_x: i32::from(cx) + 10,
        min_y: i32::from(cy) - 10,
        max_y: i32::from(cy) + 10,
    });
    let graphic = Graphic(0x0736);
    builder.add(cx, cy, 0, graphic, &stair, Shape::solid(prism));
    let occlusion = builder.finish(&Cutaway::OPEN);
    // **The bottom tread's own solid**, named rather than arrived at: `LIT` sits
    // in that tread's strip, so this is what a fragment there really is a point
    // of. It used to be `owner_at` and the shader's own narrowing, which for a
    // flight comes down to the grid's reference order — three lids, one owner —
    // and the test passed because that order happens to put tread 0 first. The
    // solid rides in the position plane now (`solid_format.wesl`), so the
    // fixture says which one and the reference order decides nothing.
    let solid = occlusion.id_of(
        i32::from(cx),
        i32::from(cy),
        occlusion::Owner::new(0, graphic),
        occlusion::Part::nth(0),
    );
    assert!(
        solid.is_some(),
        "the flight has to have a bottom tread or the fragment is shadowed by the step it stands on",
    );

    // The pixel the ray is vertical at, and its control. Both are on the centre
    // tile; `LIT` sits in the *bottom* tread's strip and `BLOCKED` in the middle
    // tread's. The flame's position comes out of `parity_place` rather than being
    // written down again — "directly above" has to be exact, and a second copy of
    // the fixture's own arithmetic is the one way to get it a float off.
    //
    // **Above and not below**, which is not arbitrary: a `Stance::Flat` fragment
    // looks up, so a flame under it is behind its own plane and `light::faces`
    // takes the whole term to nothing — both pixels come out at the ambient and
    // the fixture measures the facing rule instead of the shortcut. Hence the
    // bottom tread: it is the one whose two neighbouring lids are *over* it.
    const LIT: (u32, u32) = (35, 38);
    const BLOCKED: (u32, u32) = (35, 35);
    let (x, y, sub_x, sub_y) = parity_place(LIT.0, LIT.1);
    assert_eq!(
        (x, y),
        (cx, cy),
        "the lit pixel has to be on the flight's own tile"
    );
    let over = Vec2::new(f32::from(x) + sub_x, f32::from(y) + sub_y);

    let lighting = Lighting {
        ambient: openshard_client_render::light::NIGHT,
        lights: vec![Light {
            at: over,
            // Well above the flight, so the ray runs up past every tread.
            z: 15.0,
            radius: 40.0,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam: None,
        }],
        occlusion,
        sun: None,
        view: View::default(),
        // **A point flame, because a sphere does not send a vertical ray.**
        // `light::flame_points` lays its samples on the disc the sphere presents,
        // at `sqrt((i + 0.5) / n)` of the radius, so none of them is the centre —
        // a flame directly overhead is eight rays each leaning a
        // `FLAME_RADIUS` out of the vertical. This test was written before phase 5
        // gave the flame a body and went on passing afterwards without ever
        // entering the branch it is named for. The control below is what says it
        // does now.
        flame_radius: 0.0,
        shadow_rays: openshard_client_render::light::ShadowRays::DEFAULT,
        dead: false,
    };
    let straight = openshard_client_render::light::flame_points(
        openshard_client_render::light::Spot::flat(over, 1.0, (i32::from(cx), i32::from(cy))),
        [over.x, over.y, 15.0],
        lighting.flame_radius,
        lighting.shadow_rays,
    )
    .iter()
    .all(|point| point[0] == over.x && point[1] == over.y);
    assert!(
        straight,
        "the fixture is not sending a vertical ray, so it cannot be about one",
    );
    let fixture = Fixture {
        surface: Surface::Flat,
        drift: (0.0, 0.0),
        // The bottom tread's own height: what makes `LIT` a point *of* that
        // tread rather than of the air over it.
        z: 1,
        solid,
    };

    let (width, height) = (64, 64);
    let frame = parity_frame(&device, &queue, &lighting, width, height, fixture);
    let lit = frame.pixel(LIT.0, LIT.1);
    let blocked = frame.pixel(BLOCKED.0, BLOCKED.1);
    assert!(
        lit[0] > blocked[0] + 40,
        "the flame is directly over {LIT:?} and the lids between them are strips \
         of the tile it is not under: {lit:?} there against {blocked:?} over the \
         middle tread, which its own lid does stop",
    );
}

/// **A vertical ray on the GPU is stopped by the wall it stands inside** —
/// `docs/occluders.md`'s S4, the shader's third of the vertical shortcut.
///
/// That branch skipped every panel outright, on the argument that a panel is a
/// plane and a vertical ray lying in a wall's own plane is a graze it had no rule
/// for. A panel is not a plane in the grid: it is a `PANEL_THICKNESS`-deep slab,
/// and a fragment standing inside one is behind the whole height of a wall.
/// `light.rs`'s grave note has the argument and
/// `lighting.rs`'s `a_vertical_ray_meets_what_stands_over_it_whatever_shape_it_is`
/// is the same claim about the two CPU walks; this is the shader's own copy,
/// which nothing else here reaches.
///
/// The pixel is chosen for its **fraction**: `parity_place` steps a tile in
/// sixteen hundred-and-twenty-sevenths, so the last pixel of a tile sits at
/// `112/127` of it — inside a south panel's own `0.8..1.0` slab, where the pixel
/// before it at `96/127` is not. The control is the same pixel with the wall
/// taken out of the grid and nothing else changed, so what is being read is the
/// wall and not the geometry of the fixture.
///
/// `flame_radius` is `0.0` for the reason the test above states: a sphere sends
/// no vertical ray, and at `FLAME_RADIUS` this fixture would measure the ordinary
/// walk.
#[test]
fn the_shader_stops_a_vertical_ray_with_the_panel_it_stands_inside() {
    let Some((device, queue)) = gpu() else {
        return;
    };

    let (cx, cy) = (
        openshard_client_render::scene::CENTRE.x,
        openshard_client_render::scene::CENTRE.y,
    );
    let bounds = TileBounds {
        min_x: i32::from(cx) - 10,
        max_x: i32::from(cx) + 10,
        min_y: i32::from(cy) - 10,
        max_y: i32::from(cy) + 10,
    };
    // The last pixel down the centre tile: `112/127` of the way into it, which is
    // inside a south panel and is the only fraction this frame draws that is.
    const INSIDE: (u32, u32) = (35, 39);
    let (x, y, sub_x, sub_y) = parity_place(INSIDE.0, INSIDE.1);
    assert_eq!((x, y), (cx, cy), "the pixel has to be on the wall's own tile");
    assert!(
        f64::from(sub_y) >= 1.0 - openshard_client_render::occlusion::PANEL_THICKNESS,
        "the pixel is not inside the panel's own slab, so this test is about nothing",
    );
    let over = Vec2::new(f32::from(x) + sub_x, f32::from(y) + sub_y);

    let frame = |grid: openshard_client_render::occlusion::Occlusion| {
        let lighting = Lighting {
            ambient: openshard_client_render::light::NIGHT,
            lights: vec![Light {
                at: over,
                z: 20.0,
                radius: 40.0,
                color: [1.0, 1.0, 1.0],
                intensity: 1.0,
                beam: None,
            }],
            occlusion: grid,
            sun: None,
            view: View::default(),
            flame_radius: 0.0,
            shadow_rays: openshard_client_render::light::ShadowRays::DEFAULT,
            dead: false,
        };
        let fixture = Fixture {
            // A point of nothing, looking nowhere: the wall is a different
            // static, so identity and D2 have nothing to say and what is left is
            // the shape alone.
            surface: Surface::Upright,
            z: 0,
            solid: None,
            drift: (0.0, 0.0),
        };
        let frame = parity_frame(&device, &queue, &lighting, 64, 64, fixture);
        i32::from(frame.pixel(INSIDE.0, INSIDE.1)[0])
    };

    let mut wall = Builder::new(bounds);
    wall.add(
        cx,
        cy,
        0,
        Graphic(0),
        &openshard_tiles::StaticTile {
            flags: openshard_tiles::TileFlags::new(openshard_tiles::TileFlags::NO_SHOOT),
            height: 20,
            ..openshard_tiles::StaticTile::default()
        },
        Shape {
            facing: Some(openshard_client_render::facing::Facing::One(
                openshard_client_render::facing::Face::South,
            )),
            hole: None,
            prism: None,
            blocks: openshard_client_render::facing::Blocks::EMPTY,
            footprint: None,
        },
    );

    let behind = frame(wall.finish(&Cutaway::OPEN));
    let open = frame(Builder::new(bounds).finish(&Cutaway::OPEN));
    assert!(
        open > behind + 40,
        "a fragment inside the wall's own slab, lit from twenty `z` straight \
         overhead, reads {behind} against {open} with the wall taken away: the \
         shader is letting a vertical ray through a panel",
    );
}

/// **A fragment a hair past its own tile's edge is shadowed by the wall it has
/// drifted into.**
///
/// **What it is a claim about has changed, and `docs/occluders.md`'s S4 is
/// why.** It used to compare `blit.wesl`'s `starting_cell` against
/// `light::starting_cell` — one rule written twice with no compiler between the
/// two — and both are deleted. What it states now is one step further back:
/// **the shader answers from where a fragment stands, not from the tile its id
/// plane names.** Those two are the same number for almost every pixel, and this
/// fixture is the only place in the crate where they are made to differ.
///
/// The scene is one body on the centre tile and a flame four tiles west of it,
/// with the fixture's whole frame drifted a fifth of a tile east. That puts the
/// rightmost pixel of the tile *west* of the wall at `x = centre + 0.08` — inside
/// the wall's own cell, and inside its box — while the id plane goes on saying
/// the tile it was drawn on. Its neighbour one pixel left stays west of the wall
/// and stands in the open, which is what makes the pair a claim rather than a
/// number: the one inside the wall must be dark and the one beside it must not.
///
/// ⚠ **It is a fixture rather than a gate, and that is measured.** Seed the
/// shader's walk with a cell the fragment is *not* in and this stays green,
/// because the ray runs west and reaches the wall's own cell within a hair
/// either way — the injection reddens `the_shader_stops_a_vertical_ray_with_the_
/// panel_it_stands_inside`, both path-tracer gates and `pictures.rs`'s
/// `a_wall_in_front_of_a_torch_darkens_the_ground_behind_it_and_not_beside_it`
/// instead. Its CPU twin was deleted for exactly this and its grave note in
/// `light.rs` says so; this one is kept because nothing else in the crate builds
/// a fragment whose position and carried tile disagree, and the day something
/// starts reading that tile again this is the scene that shows it.
///
/// `Surface::Upright` on purpose: the ray here is horizontal, so a `Flat`
/// fragment would answer with a cosine of zero and both pixels would be dark for
/// a reason that is not the walk.
#[test]
fn a_fragment_a_hair_inside_a_wall_is_shadowed_by_the_cell_it_drifted_into() {
    let Some((device, queue)) = gpu() else {
        return;
    };

    let wall = openshard_tiles::StaticTile {
        flags: openshard_tiles::TileFlags::new(openshard_tiles::TileFlags::NO_SHOOT),
        height: 20,
        ..openshard_tiles::StaticTile::default()
    };
    let (cx, cy) = (
        openshard_client_render::scene::CENTRE.x,
        openshard_client_render::scene::CENTRE.y,
    );
    let mut builder = Builder::new(TileBounds {
        min_x: i32::from(cx) - 10,
        max_x: i32::from(cx) + 10,
        min_y: i32::from(cy) - 10,
        max_y: i32::from(cy) + 10,
    });
    builder.add(cx, cy, 0, Graphic(0x0100), &wall, Shape::UNREAD);
    let occlusion = builder.finish(&Cutaway::OPEN);

    // A fifth of a tile, which is far more than the rounding a real frame drifts
    // by — the claim is about which side of a boundary a point is on, and a
    // fixture that states it at `f32`'s own scale would be testing the arithmetic
    // of the fixture rather than the rule.
    const DRIFT: f32 = 0.2;
    // `px % PARITY_TILE == 7` is the last pixel of its tile, fraction `112/127`;
    // with the drift its position is `0.08` into the next tile. `px - 1` is
    // `96/127 + 0.2`, still short of the boundary.
    const INSIDE: (u32, u32) = (31, 35);
    const BESIDE: (u32, u32) = (30, 35);
    let (tile_x, tile_y, sub_x, sub_y) = parity_place(INSIDE.0, INSIDE.1);
    assert_eq!(
        (tile_x + 1, tile_y),
        (cx, cy),
        "the drifted pixel has to sit on the tile just west of the wall",
    );
    let at = (f32::from(tile_x) + sub_x + DRIFT, f32::from(tile_y) + sub_y);
    assert!(
        at.0 > f32::from(cx) && at.0 < f32::from(cx) + 1.0,
        "the drifted position {at:?} has to land inside the wall's own tile",
    );

    let lighting = Lighting {
        ambient: openshard_client_render::light::NIGHT,
        lights: vec![Light {
            // Due west of both pixels, so the ray runs back across the tile the
            // drifted fragment claims and the wall it stands in is the only thing
            // between them.
            at: Vec2::new(f32::from(cx) - 3.5, at.1),
            z: 10.0,
            radius: 12.0,
            color: [1.0, 1.0, 1.0],
            intensity: 3.0,
            beam: None,
        }],
        occlusion,
        sun: None,
        view: View::default(),
        flame_radius: openshard_client_render::light::FLAME_RADIUS,
        shadow_rays: openshard_client_render::light::ShadowRays::DEFAULT,
        dead: false,
    };
    let fixture = Fixture {
        surface: Surface::Upright,
        // Inside the body's own `0..20`, and level with the flame.
        z: 10,
        solid: None,
        drift: (DRIFT, 0.0),
    };

    let (width, height) = (64, 64);
    let frame = parity_frame(&device, &queue, &lighting, width, height, fixture);
    let inside = frame.pixel(INSIDE.0, INSIDE.1);
    let beside = frame.pixel(BESIDE.0, BESIDE.1);
    assert!(
        i32::from(beside[0]) > i32::from(inside[0]) + 40,
        "the fragment at {at:?} stands inside the wall on ({cx}, {cy}) and reads \
         {inside:?}, against {beside:?} one pixel west of it in the open",
    );
}

/// Whether `Reach::through` counts as blocked — `light.rs`'s own
/// `RAY_CUTOFF`, restated here rather than imported: it is not `pub`, and
/// every oracle in `docs/lighting_raymarch.md` (`tests/lighting.rs`'s
/// brute-force grid and fuzz) already carries its own copy of the same
/// number rather than reach into the crate for it.
const EXACT_WALK_BLOCKED: f32 = 0.004;

/// Whether `point` stands anywhere in `tile`'s own column, boundaries included —
/// the exemption's predicate, closed on both sides.
///
/// See [`ground_truth_blocked`] for why the tie a boundary point makes is
/// resolved towards the exemption instead of by `floor()`.
fn in_column(point: [f32; 3], tile: (i32, i32)) -> bool {
    point[0] >= tile.0 as f32
        && point[0] <= tile.0 as f32 + 1.0
        && point[1] >= tile.1 as f32
        && point[1] <= tile.1 as f32 + 1.0
}

/// Whether the straight segment from `from` to `to` passes through any solid
/// standing between the two tiles the walk itself exempts — the same idea as
/// `tests/lighting.rs`'s `brute_force_blocked`, restated here rather than
/// shared across two independent test crates: dumb, fixed-step marching and a
/// point-in-box test, sharing no arithmetic with either walk.
///
/// **And against every solid in the frame, not the ones a cell lists** — the same
/// repair `brute_force_blocked` took on 2026-08-09 and for the same measured
/// reason, since this oracle had the identical `solids_at(floor(x), floor(y))` in
/// it. A point on a box's own `max` face floors into the neighbouring cell, which
/// does not list that box, so the march could stand inside a wall and report open
/// ground. It arbitrates between the two walks here, which makes an oracle that
/// can be wrong about a corner worse than useless: it convicts whichever walk was
/// right. See `docs/occluders.md` § *The oracle*.
///
/// **Twenty thousand steps over the whole segment, not `brute_force_blocked`'s
/// fixed `0.02`-tile step.** The first version of this oracle used that same
/// constant and returned a false "open" for a real, `ray_vs_solid`-confirmed
/// crossing: a ray clipping a panel box at a shallow, corner-grazing angle can
/// spend far less than [`crate::occlusion::PANEL_THICKNESS`]'s own depth
/// inside it — a hundredth of a tile, in the case that found this — and
/// `brute_force_blocked`'s own step was sized to the panel's *thickness*, not
/// to how thin a graze through it can be. A point sampler cannot rule out an
/// arbitrarily thin sliver at any finite resolution; twenty thousand steps
/// is a practical stand-in that this file's own scenes have not defeated, not
/// a claim of exactness the way `ray_vs_solid`'s analytic slab test is one.
///
/// Returns `None` where the march finds itself **inside** a solid that has an
/// aperture — the one case it cannot judge, since the point may be standing in
/// the hole. `brute_force_blocked` instead hard-`assert!`s a scene never has one,
/// which it can afford: every fixture in that file is hand-built to keep the
/// premise true. This one runs over `scene::wall_with_a_hole_in_it`, whose
/// entire point is an aperture, so a disagreement whose path crosses the hole
/// is left unexplained rather than a reason to panic the whole sweep.
fn ground_truth_blocked(
    from: [f32; 3],
    to: [f32; 3],
    own_tile: (i32, i32),
    target_tile: (i32, i32),
    skip_last: bool,
    occlusion: &Occlusion,
) -> Option<bool> {
    const STEPS: u32 = 20_000;
    let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    for step in 1..STEPS {
        let t = step as f32 / STEPS as f32;
        let point = [
            from[0] + delta[0] * t,
            from[1] + delta[1] * t,
            from[2] + delta[2] * t,
        ];
        if in_column(point, own_tile) || (skip_last && in_column(point, target_tile)) {
            continue;
        }
        for solid in occlusion.solids() {
            let (min, max) = (solid.space.min, solid.space.max);
            let inside = f64::from(point[0]) >= min.x
                && f64::from(point[0]) <= max.x
                && f64::from(point[1]) >= min.y
                && f64::from(point[1]) <= max.y
                && f64::from(point[2]) >= min.z
                && f64::from(point[2]) <= max.z;
            if inside {
                // A hole is the one thing this march cannot judge, and it is only
                // unable to judge a solid it is actually *inside*: bailing on
                // every apertured solid in the frame would make
                // `scene::wall_with_a_hole_in_it` unanswerable everywhere,
                // including the rays that never come near the window.
                return match solid.aperture {
                    Some(_) => None,
                    None => Some(true),
                };
            }
        }
    }
    Some(false)
}

/// Whether the march that backs [`ground_truth_blocked`] ever finds itself a
/// real, non-degenerate distance inside `blamed`'s own box, rather than only
/// brushing its corner.
///
/// `docs/lighting_raymarch.md`'s own established stance on a ray that only
/// ever touches a solid's corner — never a length of its inside — is that
/// this is not a bug in either walk to resolve, it is the accepted overlap
/// two panels physically share at a shared corner
/// (`crate::occlusion::PANEL_THICKNESS`). [`walk_the_record`]'s
/// `candidate_tiles` unconditionally probes both diagonal neighbours at
/// every step — a deliberate feature, so a genuine corner-cutting occluder
/// is never missed — and at an *exact* corner tie a ray can graze a
/// diagonal neighbour's own corner point without its straight line ever
/// entering that tile's interior at all. [`walk_the_wire`] has no
/// such probe and simply never visits that tile, which is why the two can
/// disagree exactly here without either being wrong: [`exact_walk_
/// disagreements`] arbitrates by [`ground_truth_blocked`], which is blind to
/// *which* tile it found itself inside, so a genuine crossing elsewhere on
/// the segment can back a blamed tile it was never actually about. This
/// walks the identical march restricted to `blamed` alone and requires more
/// than a couple of isolated samples — `floor`'s own discontinuity at an
/// exact tile boundary can put one or two stray samples on the wrong side of
/// a corner it never really entered, which is the same hazard this doc's own
/// `floor`-vs-`round` entry already named for a different oracle.
fn blamed_tile_has_a_real_crossing(
    from: [f32; 3],
    to: [f32; 3],
    blamed: (i32, i32),
    occlusion: &Occlusion,
) -> bool {
    const STEPS: u32 = 20_000;
    const MIN_SAMPLES: u32 = 4;
    let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut samples = 0;
    for step in 1..STEPS {
        let t = step as f32 / STEPS as f32;
        let point = [
            from[0] + delta[0] * t,
            from[1] + delta[1] * t,
            from[2] + delta[2] * t,
        ];
        let tile = (point[0].floor() as i32, point[1].floor() as i32);
        if tile != blamed {
            continue;
        }
        for solid in occlusion.solids_at(tile.0, tile.1) {
            let (min, max) = (solid.space.min, solid.space.max);
            let inside = f64::from(point[0]) >= min.x
                && f64::from(point[0]) <= max.x
                && f64::from(point[1]) >= min.y
                && f64::from(point[1]) <= max.y
                && f64::from(point[2]) >= min.z
                && f64::from(point[2]) <= max.z;
            if inside {
                samples += 1;
                if samples >= MIN_SAMPLES {
                    return true;
                }
            }
        }
    }
    false
}

/// What a sweep of [`light::sample_exact`] against [`light::sample`] found,
/// over one of decision 9's own real-geometry scenes.
struct ExactWalkReport {
    /// A classification flip [`ground_truth_blocked`] backs `walk_cells`
    /// for — a genuine, new defect in `walk_the_record` this doc has not
    /// already catalogued, and the only thing these tests fail on.
    bugs: Vec<String>,
    /// A classification flip [`ground_truth_blocked`] backs
    /// `walk_the_record` for: an already-real `walk_cells` gap, the same
    /// family `docs/lighting_raymarch.md`'s session 9 found four of.
    explained: usize,
    /// A classification flip this oracle cannot rule on — the marched path
    /// crossed a solid with an aperture, which it does not model.
    unexplained: usize,
    /// A classification flip backed by a tile `walk_the_record` blamed that
    /// the straight segment only ever grazes the corner of — the accepted
    /// corner-grazing ambiguity, not a defect. See `blamed_tile_has_a_real_
    /// crossing`'s own doc comment.
    grazed: usize,
}

/// [`light::sample_exact`] against [`light::sample`], over one of decision
/// 9's own real-geometry scenes — `docs/lighting_raymarch.md`'s point 3, the
/// half of it that needs no GPU because neither side being compared is the
/// shader. The scene is the fixture, the same reason decision 9's own suite
/// reuses these rather than a fixture built for this test alone: a room's
/// torch, a house corner, a hole in a wall, a window's sun each put a ray
/// through geometry nobody hand-picked to flatter either walk.
///
/// **Not full agreement, on purpose.** The first version of this sweep
/// asserted zero classification flips and failed on more than a hundred
/// pixels of the plainest scene here, `scene::room` — a spot standing on one
/// tile of a straight wall run blamed a *neighbour* tile of the same run for
/// blocking a ray that, marched by hand, never enters that neighbour's box
/// at all. `docs/lighting_raymarch.md`'s session 9 already catalogued four
/// `walk_cells` gaps this exact track's fuzzing found; this is a real fifth,
/// found by real geometry rather than a fuzzer, in the opposite direction
/// from the other four (over-occlusion, not under). See this test file's own
/// handoff entry for the coordinates and the confirming hand-march.
///
/// So a flip is not itself a failure: [`ground_truth_blocked`] — independent
/// of both walks, sharing no arithmetic with either — is asked to arbitrate.
/// Backing `walk_the_record` explains the flip as one of `walk_cells`'s own
/// gaps; backing `walk_cells` instead is a real bug in the exact walk, which
/// is what fails these tests.
fn exact_walk_disagreements(lighting: &Lighting, surface: Surface, z: i8) -> ExactWalkReport {
    let mut report = ExactWalkReport {
        bugs: Vec::new(),
        explained: 0,
        grazed: 0,
        unexplained: 0,
    };
    for py in 0..64 {
        for px in 0..64 {
            let (x, y, sub_x, sub_y) = parity_place(px, py);
            let own_tile = (i32::from(x), i32::from(y));
            let spot = openshard_client_render::light::Spot {
                surface,
                ..openshard_client_render::light::Spot::at(
                    Vec2::new(f32::from(x) + sub_x, f32::from(y) + sub_y),
                    f32::from(z),
                    own_tile,
                )
            };
            let from = [spot.at.x, spot.at.y, spot.z];
            let sample = openshard_client_render::light::sample(spot, lighting);
            let exact = openshard_client_render::light::sample_exact(spot, lighting);
            for (index, (a, b)) in sample.reaches.iter().zip(exact.reaches.iter()).enumerate() {
                if !a.within {
                    continue;
                }
                let a_blocked = a.through <= EXACT_WALK_BLOCKED;
                let b_blocked = b.through <= EXACT_WALK_BLOCKED;
                if a_blocked == b_blocked {
                    continue;
                }
                let light = &lighting.lights[index];
                let to = [light.at.x, light.at.y, light.z];
                let target_tile = (light.at.x.floor() as i32, light.at.y.floor() as i32);
                match ground_truth_blocked(from, to, own_tile, target_tile, true, &lighting.occlusion) {
                    None => report.unexplained += 1,
                    Some(truth) if truth == b_blocked => report.explained += 1,
                    // `walk_the_record` blamed a tile the straight segment
                    // never actually enters — only its own corner, which
                    // `candidate_tiles`' unconditional diagonal probe reaches
                    // and a bare point-in-box march (blind to which tile it
                    // is even asking about) cannot itself rule out. See
                    // `blamed_tile_has_a_real_crossing`'s own doc comment:
                    // this is `docs/lighting_raymarch.md`'s own accepted
                    // corner-grazing ambiguity, the same shape `walk_cells`'
                    // `corner_tie` used to be generous about on purpose, not
                    // a defect in `walk_the_record` to chase.
                    Some(_)
                        if b_blocked
                            && !blamed_tile_has_a_real_crossing(
                                from,
                                to,
                                b.stopped_by.expect("blocked implies a blamed tile").cell,
                                &lighting.occlusion,
                            ) =>
                    {
                        report.grazed += 1;
                    }
                    Some(_) => report.bugs.push(format!(
                        "({px}, {py}) light {index}: walk_cells {} ({:.4}), walk_the_record {} \
                         ({:.4}), ground truth says {}",
                        if a_blocked { "blocked" } else { "open" },
                        a.through,
                        if b_blocked { "blocked" } else { "open" },
                        b.through,
                        if a_blocked { "blocked" } else { "open" },
                    )),
                }
            }
            // The sun has no `target_tile` of its own to skip — `walk_sun`/
            // `walk_sun_exact` both pass `skip_last: false` — and no scene
            // here disagreed about it even once this oracle existed to check,
            // so it is compared but not (yet) arbitrated by ground truth.
            if let (Some(a), Some(b)) = (sample.sun, exact.sun) {
                let a_blocked = a.through <= EXACT_WALK_BLOCKED;
                let b_blocked = b.through <= EXACT_WALK_BLOCKED;
                if a_blocked != b_blocked {
                    report.bugs.push(format!(
                        "({px}, {py}) sun: walk_cells {} ({:.4}), walk_the_record {} ({:.4}), \
                         not yet arbitrated by ground truth",
                        if a_blocked { "blocked" } else { "open" },
                        a.through,
                        if b_blocked { "blocked" } else { "open" },
                        b.through,
                    ));
                }
            }
        }
    }
    report
}

/// Asserts [`exact_walk_disagreements`] found no unexplained bug, and prints
/// the explained/unexplained counts either way — visible with `--nocapture`,
/// and in a failure's own message, since a reader deciding whether a fifth
/// `walk_cells` gap needs its own backlog entry wants those numbers regardless
/// of which count actually fired.
fn assert_no_exact_walk_bugs(report: &ExactWalkReport) {
    println!(
        "{} explained (known walk_cells gaps), {} unexplained (aperture on the path), \
         {} grazed (corner-only touch)",
        report.explained, report.unexplained, report.grazed,
    );
    assert!(
        report.bugs.is_empty(),
        "{} of 4096 pixels found a real walk_the_record bug:\n{}",
        report.bugs.len(),
        report.bugs.join("\n"),
    );
}

#[test]
fn the_exact_walk_agrees_with_light_sample_over_a_room() {
    let scene = openshard_client_render::scene::room();
    let report = exact_walk_disagreements(&scene.lighting(0.0), Surface::Upright, 0);
    assert_no_exact_walk_bugs(&report);
}

/// And the same for a scene whose occluders are panels on named edges —
/// see `the_shader_and_light_sample_agree_about_which_side_a_wall_is_on`'s
/// own comment for why this shape gets its own fixture.
#[test]
fn the_exact_walk_agrees_with_light_sample_about_which_side_a_wall_is_on() {
    let scene = openshard_client_render::scene::wall_with_a_torch_beside_it();
    let report = exact_walk_disagreements(&scene.lighting(0.0), Surface::Upright, 0);
    assert_no_exact_walk_bugs(&report);
}

/// And the same at a house corner — a run of panels meeting a faceless whole
/// tile, exactly the shape `corner_tie` exists for and the one
/// `docs/lighting_raymarch.md`'s session 9 traced two of `walk_cells`'s own
/// gaps from.
#[test]
fn the_exact_walk_agrees_with_light_sample_at_the_corner_of_a_house() {
    let scene = openshard_client_render::scene::house_corner();
    let report = exact_walk_disagreements(&scene.lighting(0.0), Surface::Upright, 0);
    assert_no_exact_walk_bugs(&report);
}

/// And the same for a scene with a hole in one of its panels — where
/// [`ground_truth_blocked`]'s own aperture scope is expected to leave some
/// flips unexplained rather than arbitrated, see this test's own handoff
/// entry for the count.
#[test]
fn the_exact_walk_agrees_with_light_sample_about_a_hole_in_a_wall() {
    let scene = openshard_client_render::scene::wall_with_a_hole_in_it();
    let report = exact_walk_disagreements(&scene.lighting(0.0), Surface::Upright, 0);
    assert_no_exact_walk_bugs(&report);
}

/// And the same for a scene with a sun in it — [`walk_sun_exact`] against
/// [`walk_sun`], not just the flame end of the seam.
#[test]
fn the_exact_walk_agrees_with_light_sample_about_the_sun() {
    let scene = openshard_client_render::scene::sunlit_room_with_window();
    let report = exact_walk_disagreements(&scene.lighting(0.0), Surface::Upright, 0);
    assert_no_exact_walk_bugs(&report);
}

/// And the same over a surface that looks up, at both a lit floor and a
/// lid over the flame — decision 27's own reason for a second fixture,
/// restated for this seam.
#[test]
fn the_exact_walk_agrees_with_light_sample_about_a_surface_that_looks_up() {
    let lighting = openshard_client_render::scene::room().lighting(0.0);
    for z in [0, 20] {
        let report = exact_walk_disagreements(&lighting, Surface::Flat, z);
        assert_no_exact_walk_bugs(&report);
    }
}

/// And the same for a carried beam — the one term the other scenes above
/// cannot exercise, since every static flame lights all ways.
#[test]
fn the_exact_walk_agrees_with_light_sample_about_a_carried_beam() {
    let scene = openshard_client_render::scene::lantern_in_a_room();
    let report = exact_walk_disagreements(&scene.lighting(0.0), Surface::Upright, 0);
    assert_no_exact_walk_bugs(&report);
}

/// What one pixel of `View::Shadow` says, as the fact the view draws rather
/// than as a colour.
///
/// Three states and not one number, because two of the three are painted a
/// *colour* on purpose — `blit.wesl`'s own `debug_color` says why: "no flame
/// reaches here at all" and "a flame reaches and every ray of it was stopped"
/// are different facts, and a view that drew both as black would answer two
/// questions with one shade. A comparison between the two backends has to
/// separate them for the same reason: a fragment nothing reaches agreeing with a
/// fragment behind a wall would be the sweep agreeing about nothing.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Shadow {
    /// No flame's own pool holds this fragment — nothing was walked.
    Unreached,
    /// A flame reaches it and the walk let none of it through.
    Blocked,
    /// The share of the nearest flame's own body this fragment can see:
    /// `Arrival::visible`, with no cosine, no falloff and no beam in it.
    Through(f32),
}

/// [`Shadow`] read off a pixel of the rendered shadow view.
///
/// The three colours `blit.wesl` paints, inverted. A grey is the value itself,
/// and the two flat colours are recognised by their *shape* — a channel that is
/// zero where a grey's would not be — rather than by their exact byte, so this
/// does not restate the constants and cannot drift into agreeing with them.
fn shadow_drawn(pixel: [u8; 4]) -> Shadow {
    match pixel {
        // `(0, 0, 0.35)`: the blue that says no flame reaches.
        [0, 0, blue, _] if blue > 0 => Shadow::Unreached,
        // `(0.2, 0, 0)`: the dark red that says one does and was stopped.
        [red, 0, 0, _] if red > 0 => Shadow::Blocked,
        [red, green, blue, _] => {
            assert_eq!(
                (red, red),
                (green, blue),
                "the shadow view paints visibility as a grey, and {pixel:?} is not one",
            );
            Shadow::Through(f32::from(red) / 255.0)
        }
    }
}

/// And [`Shadow`] as `light::sample` answers it: the same three states off the
/// [`Reach`](openshard_client_render::light::Reach)es of one spot.
///
/// **The nearest flame by its own share of its own reach**, which is the one
/// rule of the shadow view this side has to restate — `blit.wesl` keeps `d =
/// |offset| / reach` while it lights the fragment and this recomputes it from
/// the numbers a `Reach` carries. Everything else compared here is a number both
/// sides already have a name for.
///
/// A flame outside its own pool by the centre — `d >= 1` — is the shader's blue
/// as much as no flame at all is: the near-side cull that let it into the loop is
/// deliberately wider than the pool (`docs/lighting_rebuild.md` phase 5b), so
/// "within the cull" and "within the pool" are two different questions and the
/// view answers the second.
fn shadow_wanted(sample: &openshard_client_render::light::Sample, lighting: &Lighting) -> (Shadow, f32) {
    let mut nearest: Option<(f32, f32)> = None;
    for reach in &sample.reaches {
        if !reach.within {
            continue;
        }
        let share = reach.distance / lighting.lights[reach.light.position()].radius.max(0.001);
        if nearest.is_none_or(|(seen, _)| share < seen) {
            nearest = Some((share, reach.through));
        }
    }
    match nearest {
        None => (Shadow::Unreached, f32::INFINITY),
        Some((share, _)) if share >= 1.0 => (Shadow::Unreached, share),
        Some((share, through)) if through <= EXACT_WALK_BLOCKED => (Shadow::Blocked, share),
        Some((share, through)) => (Shadow::Through(through), share),
    }
}

/// What a sweep of the **shader** against `light::sample` found over one scene.
///
/// The counts are printed whatever happens, the same discipline
/// [`assert_no_exact_walk_bugs`] keeps: a sweep that says only "0 failures" has
/// not said how much it looked at.
struct ShaderSweep {
    /// Pixels where the two backends said different things and neither boundary
    /// below excuses it.
    bugs: Vec<String>,
    /// How many pixels were compared at all.
    compared: usize,
    /// Pixels sitting on the rim of a pool — `d` within [`RIM`] of `1.0`, where
    /// "inside the pool" is decided by an `f32` comparison the two backends
    /// compute separately. A flip there is the rim being drawn a hair wider on
    /// one side, not a primitive going missing.
    rim: usize,
    /// And pixels sitting on the cutoff, where `Blocked` and `Through` are the
    /// same answer to within a rounding of the same product.
    cutoff: usize,
    /// How many pixels `light::sample` says are behind something — every ray of
    /// the nearest flame stopped.
    ///
    /// **The census that keeps this sweep from being vacuous**, and it is
    /// asserted rather than printed: a fixture where nothing is in shadow
    /// compares four thousand pixels of "no flame reaches" and agrees about the
    /// walk on none of them. `docs/occluders.md`'s own "a gate can be vacuous
    /// three times over" is what this is here for.
    blocked: usize,
    /// And how many see the nearest flame at all — the other half of the same
    /// census: a fixture where *everything* is behind a wall compares one answer
    /// as surely as one where nothing is.
    lit: usize,
    /// Of those, how many see part of a flame and not all of it: a penumbra
    /// pixel, where visibility is a count of eighths rather than nought or one.
    /// Printed and not asserted — a flame is a twentieth of a tile across and a
    /// fixture pixel an eighth of one, so a scene whose shadow edges fall
    /// between pixels legitimately has none.
    partly: usize,
}

/// How near `d = 1` a fragment has to be for a `Unreached`/`Through` flip to be
/// the pool's own rim rather than a disagreement about geometry.
///
/// A pool is three tiles across and a fixture pixel is an eighth of a tile, so
/// this is a ten-thousandth of the *pixel* — far tighter than anything a lost
/// primitive could hide behind, and wider than the last bits of an `f32`
/// division at these magnitudes.
const RIM: f32 = 1.0e-4;

/// How far apart two visibilities may be and still be one answer: a byte of the
/// view they are drawn into, plus one for the rounding either side of it.
///
/// **Not a tolerance for a lost primitive to hide in.** Eight rays make
/// visibility a multiple of an eighth wherever occlusion is opaque, so a
/// primitive the broad phase drops moves this by `0.125` at the very least —
/// thirty times what this allows.
const VIEW_BYTE: f32 = 2.0 / 255.0;

/// The **shader** against `light::sample`, pixel by pixel, over the shadow view.
///
/// `docs/occluders.md`'s S5, and the reason it exists is written there: the
/// surviving CPU comparisons in this file are the exact walk against the
/// streaming one, and since the tree landed those two *share* their broad phase —
/// so a broad phase that loses a primitive is invisible to them. This compares
/// the two things that do not share one: `blit.wesl`'s traversal and
/// `light::candidates`.
///
/// **The shadow view and not the lit frame**, which is what makes the subject the
/// walk rather than the whole shading model. What it draws is `Arrival::visible`
/// — visibility alone, linear, un-tone-mapped, in a byte — so a ray the tree
/// stopped handing over is a full eighth of the number, where the lit frame would
/// put it through a cosine, a falloff and a curve that saturates on white art.
fn shader_sweep(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    lighting: &Lighting,
    fixture: Fixture,
) -> ShaderSweep {
    let (width, height) = (64, 64);
    let mut shown = lighting.clone();
    shown.view = View::Shadow;
    let frame = parity_frame(device, queue, &shown, width, height, fixture);
    let mut sweep = ShaderSweep {
        bugs: Vec::new(),
        compared: 0,
        rim: 0,
        cutoff: 0,
        blocked: 0,
        lit: 0,
        partly: 0,
    };
    for py in 0..height {
        for px in 0..width {
            let (x, y, sub_x, sub_y) = parity_place(px, py);
            let spot = openshard_client_render::light::Spot {
                surface: fixture.surface,
                ..openshard_client_render::light::Spot::at(
                    Vec2::new(f32::from(x) + sub_x, f32::from(y) + sub_y),
                    f32::from(fixture.z),
                    (i32::from(x), i32::from(y)),
                )
            };
            let sample = openshard_client_render::light::sample(spot, lighting);
            let (wanted, share) = shadow_wanted(&sample, lighting);
            let drawn = shadow_drawn(frame.pixel(px, py));
            sweep.compared += 1;
            match wanted {
                Shadow::Blocked => sweep.blocked += 1,
                Shadow::Through(through) => {
                    sweep.lit += 1;
                    if through < 1.0 {
                        sweep.partly += 1;
                    }
                }
                Shadow::Unreached => {}
            }
            match (wanted, drawn) {
                (Shadow::Through(a), Shadow::Through(b)) if (a - b).abs() <= VIEW_BYTE => continue,
                (a, b) if a == b => continue,
                // The pool's own rim, where which side of `d = 1` a fragment
                // falls on is an `f32` division each backend does for itself.
                (Shadow::Unreached, _) | (_, Shadow::Unreached) if (share - 1.0).abs() <= RIM => {
                    sweep.rim += 1;
                }
                // And the cutoff, where `Blocked` and a very dim `Through` are
                // the same product read either side of one comparison.
                (Shadow::Blocked, Shadow::Through(through)) | (Shadow::Through(through), Shadow::Blocked)
                    if (through - EXACT_WALK_BLOCKED).abs() <= VIEW_BYTE =>
                {
                    sweep.cutoff += 1;
                }
                (wanted, drawn) => sweep.bugs.push(format!(
                    "({px}, {py}) at ({:.4}, {:.4}, z {}): light::sample says {wanted:?}, the \
                     shader drew {drawn:?}",
                    spot.at.x, spot.at.y, spot.z,
                )),
            }
        }
    }
    sweep
}

/// Asserts a [`shader_sweep`] found nothing, and prints what it looked at either
/// way — see [`assert_no_exact_walk_bugs`], whose discipline this keeps.
fn assert_the_shader_agrees(sweep: &ShaderSweep, about: &str) {
    println!(
        "{about}: {} pixels compared, {} of them in shadow, {} seeing a flame ({} partly), \
         {} on a pool's rim, {} on the cutoff",
        sweep.compared, sweep.blocked, sweep.lit, sweep.partly, sweep.rim, sweep.cutoff,
    );
    // The census first, because a sweep that agreed about nothing agrees.
    assert!(
        sweep.blocked > 0 && sweep.lit > 0,
        "the fixture for {about} put {} pixels behind something and left {} seeing a flame: \
         with either at zero this sweep compares which pixels are out of reach and \
         nothing about the walk",
        sweep.blocked,
        sweep.lit,
    );
    assert!(
        sweep.bugs.is_empty(),
        "{} of {} pixels: the shader and light::sample disagree about {about}\n{}",
        sweep.bugs.len(),
        sweep.compared,
        sweep.bugs.join("\n"),
    );
}

#[test]
fn the_shader_and_light_sample_agree_about_what_a_room_stops() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let scene = openshard_client_render::scene::room();
    let sweep = shader_sweep(&device, &queue, &scene.lighting(0.0), Fixture::ground());
    assert_the_shader_agrees(&sweep, "a room");
}

/// And over panels standing on named edges — the shape whose rule is
/// `pierced`'s, not a body's opacity.
#[test]
fn the_shader_and_light_sample_agree_about_which_side_a_wall_is_on() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let scene = openshard_client_render::scene::wall_with_a_torch_beside_it();
    let sweep = shader_sweep(&device, &queue, &scene.lighting(0.0), Fixture::ground());
    assert_the_shader_agrees(&sweep, "which side a wall is on");
}

/// And at a house corner: a run of panels meeting a faceless whole tile, which
/// is where two primitives of one tile overlap and the retired per-cell `max`
/// used to group them.
#[test]
fn the_shader_and_light_sample_agree_at_the_corner_of_a_house() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let scene = openshard_client_render::scene::house_corner();
    let sweep = shader_sweep(&device, &queue, &scene.lighting(0.0), Fixture::ground());
    assert_the_shader_agrees(&sweep, "a house corner");
}

/// And through a hole in one — the aperture, which is the one per-primitive rule
/// with a fetch of its own behind it.
#[test]
fn the_shader_and_light_sample_agree_about_a_hole_in_a_wall() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let scene = openshard_client_render::scene::wall_with_a_hole_in_it();
    let sweep = shader_sweep(&device, &queue, &scene.lighting(0.0), Fixture::ground());
    assert_the_shader_agrees(&sweep, "a hole in a wall");
}

/// And for a carried beam, whose cone is the one term no static flame here has.
#[test]
fn the_shader_and_light_sample_agree_about_a_carried_beam() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let scene = openshard_client_render::scene::lantern_in_a_room();
    let sweep = shader_sweep(&device, &queue, &scene.lighting(0.0), Fixture::ground());
    assert_the_shader_agrees(&sweep, "a carried beam");
}

/// And over a surface that looks up, at the floor and at a height above the
/// flame — where the lid rule decides and the fragment's own normal turns half
/// the flame's sphere off.
#[test]
fn the_shader_and_light_sample_agree_about_a_surface_that_looks_up() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let lighting = openshard_client_render::scene::room().lighting(0.0);
    for z in [0, 20] {
        let fixture = Fixture {
            surface: Surface::Flat,
            z,
            ..Fixture::ground()
        };
        let sweep = shader_sweep(&device, &queue, &lighting, fixture);
        assert_the_shader_agrees(&sweep, &format!("a surface looking up at z {z}"));
    }
}

/// **The shader meets what stands at the corner two leaves meet at** —
/// `docs/occluders.md`'s backlog entry on the corner-grazing candidate, on the
/// side the grid's own probe was deleted from.
///
/// The grid carried an *unconditional diagonal probe* in `blit.wesl`'s `walk`, so
/// a ray grazing a corner without entering either cell's interior still met what
/// stood on it. S5 measured what that probe was worth — replacing its result with
/// `0.0` left the **whole crate green** — and then deleted it with the grid, which
/// made the deletion unmeasurable rather than measured. `lighting.rs`'s
/// `a_segment_through_the_corner_two_leaves_meet_at_finds_what_stands_there` is
/// the same claim about the two CPU walks; this is the shader's own third of it,
/// and nothing else here reaches it.
///
/// **One pixel, and everything about it is exact.** Eight whole-tile bodies down
/// the diagonal; the median split cuts the run in half and the two leaf boxes
/// meet at exactly one point, the shared corner of the two bodies either side of
/// the split. The fragment is a tile's own top-left corner and the flame stands
/// on the anti-diagonal through that point, so the whole segment lies in the
/// plane `x + y = cx + cy` — the one plane that touches those two boxes along a
/// single vertical edge and misses the other six outright. So the *only* thing
/// between this fragment and its flame is a graze of exactly zero length at the
/// corner, and the control is the same frame with those two bodies taken out.
///
/// `flame_radius` is `0.0` for the reason the vertical-ray fixtures give: a
/// sphere of samples puts every ray off the plane this fixture is built on, and
/// what it would then measure is the ordinary walk.
#[test]
fn the_shader_meets_what_stands_at_the_corner_two_leaves_meet_at() {
    let Some((device, queue)) = gpu() else {
        return;
    };

    let (cx, cy) = (
        openshard_client_render::scene::CENTRE.x,
        openshard_client_render::scene::CENTRE.y,
    );
    let bounds = TileBounds {
        min_x: i32::from(cx) - 10,
        max_x: i32::from(cx) + 10,
        min_y: i32::from(cy) - 10,
        max_y: i32::from(cy) + 10,
    };
    // Which of the run's eight bodies sit either side of the split, and therefore
    // which two share the corner at `(cx, cy)`.
    const WEST: u16 = 3;
    const EAST: u16 = 4;

    let run = |without: &[u16]| {
        let mut grid = Builder::new(bounds);
        for step in 0..8_u16 {
            let tile = (cx - 4 + step, cy - 4 + step);
            if without.contains(&step) {
                continue;
            }
            grid.add(
                tile.0,
                tile.1,
                0,
                Graphic(0x0100),
                &openshard_tiles::StaticTile {
                    flags: openshard_tiles::TileFlags::new(openshard_tiles::TileFlags::NO_SHOOT),
                    // Under the run's own eight tiles of extent, so the tree
                    // splits the diagonal in `x` rather than in `z`, where every
                    // one of these boxes sits at the same height.
                    height: 5,
                    ..openshard_tiles::StaticTile::default()
                },
                Shape::UNREAD,
            );
        }
        grid.finish(&Cutaway::OPEN)
    };

    // **The fixture's own subject, asserted.** Two primitives meeting at a corner
    // is a question for the narrow phase; two leaf *boxes* meeting there is what
    // makes it a question for the broad one, and it is the split that puts them
    // there rather than anything this test writes down.
    let whole = run(&[]);
    let (corner_x, corner_y) = (f64::from(cx), f64::from(cy));
    let bvh = whole.bvh();
    let ends = bvh
        .nodes()
        .iter()
        .any(|node| node.leaf.is_some() && node.space.max.x == corner_x && node.space.max.y == corner_y);
    let starts = bvh
        .nodes()
        .iter()
        .any(|node| node.leaf.is_some() && node.space.min.x == corner_x && node.space.min.y == corner_y);
    assert!(
        ends && starts,
        "no two leaf boxes meet at ({corner_x}, {corner_y}): one ending there {ends}, one \
         starting there {starts} — the run is under a single leaf and this fixture is about \
         the narrow phase only",
    );

    // The tile's own top-left corner, which is the only fraction `parity_place`
    // draws that lands exactly on the plane — see the doc.
    const ON_THE_PLANE: (u32, u32) = (40, 24);
    let (x, y, sub_x, sub_y) = parity_place(ON_THE_PLANE.0, ON_THE_PLANE.1);
    assert_eq!(
        (x, y, sub_x, sub_y),
        (cx + 1, cy - 1, 0.0, 0.0),
        "the pixel has to be the corner of the tile one step off the anti-diagonal, or its \
         ray does not run through the corner at all",
    );

    let frame = |grid: Occlusion| {
        let lighting = Lighting {
            ambient: openshard_client_render::light::NIGHT,
            lights: vec![Light {
                // On the anti-diagonal through the corner, and low enough that the
                // segment crosses the corner column three quarters of a `z` up —
                // inside the run's own height, where a flame overhead would clear
                // it.
                at: Vec2::new(f32::from(cx) - 3.0, f32::from(cy) + 3.0),
                z: 3.0,
                radius: 20.0,
                color: [1.0, 1.0, 1.0],
                intensity: 1.0,
                beam: None,
            }],
            occlusion: grid,
            sun: None,
            view: View::Shadow,
            flame_radius: 0.0,
            shadow_rays: openshard_client_render::light::ShadowRays::DEFAULT,
            dead: false,
        };
        let frame = parity_frame(&device, &queue, &lighting, 64, 64, Fixture::ground());
        shadow_drawn(frame.pixel(ON_THE_PLANE.0, ON_THE_PLANE.1))
    };

    let grazed = frame(whole);
    assert_eq!(
        grazed,
        Shadow::Blocked,
        "the shader lets a ray through the corner where two bodies meet: it drew {grazed:?} \
         where the segment touches both of them at exactly one point",
    );

    // **The control, and it has to take both bodies away**: the segment touches
    // each of them at the same instant, so either one alone still stops it. That
    // is worth having as its own reading rather than as an argument — it is what
    // says the two grazes are one point and not two chances at a thicker
    // crossing.
    for gone in [&[WEST][..], &[EAST][..]] {
        let still = frame(run(gone));
        assert_eq!(
            still,
            Shadow::Blocked,
            "with body {gone:?} of the run taken out the other still meets the ray at the \
             corner, and the shader drew {still:?}",
        );
    }
    let open = frame(run(&[WEST, EAST]));
    let Shadow::Through(visible) = open else {
        panic!(
            "with both bodies at the corner taken away nothing is left between this fragment \
             and its flame, and the shader drew {open:?} — so what the reading above measures \
             is not the corner",
        );
    };
    assert!(
        visible >= 0.99,
        "the control fragment sees only {visible:.3} of its flame with the corner cleared, so \
         something else in the run is on the segment and the graze is not what was measured",
    );
}

/// The light view has no plateau in the middle of a pool.
///
/// The failure this protects against is the one it was written for: the view
/// clamped, a torch's multiplier is past `1.0` for the whole middle of its pool,
/// and so the core of every flame — the part of the shape no other view shows,
/// because the lit frame multiplies it by dark art — was drawn as one flat white
/// disc. A clamp is invisible in a screenshot of a *dim* scene, which is why it
/// survived; a monotone curve is the property, and the way to state it is that
/// walking towards the flame never stops getting brighter.
#[test]
fn the_light_view_keeps_a_pools_shape_where_it_is_brightest() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let (width, height) = (64, 64);
    let scene = openshard_client_render::scene::room();
    let mut lighting = scene.lighting(0.0);
    lighting.view = View::Light;
    let frame = parity_frame(&device, &queue, &lighting, width, height, Fixture::ground());

    // The torch is at the room's centre, which the fixture puts at the middle of
    // the frame: a row through it runs from the pool's rim to its brightest
    // point. Every step inwards must be brighter than the last — under a clamp
    // the last two tiles' worth of it were one number.
    let middle = height / 2;
    for px in 1..width / 2 {
        let before = i32::from(frame.pixel(px - 1, middle)[0]);
        let after = i32::from(frame.pixel(px, middle)[0]);
        assert!(
            after >= before,
            "at ({px}, {middle}) the row towards the flame darkens: {before} then {after}",
        );
    }
    // And it is a ramp rather than a couple of terraces: a curve that clipped
    // would still pass the test above, which is the trap the first version fell
    // into. So **every** step of the row that is actually inside the room rises,
    // which is a stronger claim than the "more than a quarter of the whole row"
    // this used to make — and it has to be stated over that stretch rather than
    // over the whole row, because the row starts outside the house.
    //
    // The fixture's leftmost two tiles are the ground outside the ring and the
    // ring's own wall tile, and both are flat dark. That the *wall* stretch is
    // flat is `docs/lighting_height.md` phase 3: this fixture's pixels are
    // `Surface::Upright` points of no occluder at all, so they are exempt from
    // nothing, and a point standing inside the room's own wall body is behind it.
    // The old count happened to include those eight pixels because the height
    // guess exempted them from the wall they stand in — which is the guess this
    // phase removed, and a bar that counted them was measuring the guess.
    let inside = (4 - u32::from(openshard_client_render::scene::ROOM_HALF) + 1) * PARITY_TILE;
    let mut steps = 0;
    for px in inside..width / 2 {
        let before = i32::from(frame.pixel(px - 1, middle)[0]);
        let after = i32::from(frame.pixel(px, middle)[0]);
        assert!(
            after > before,
            "at ({px}, {middle}) the row inside the room does not rise: {before} then {after}",
        );
        steps += 1;
    }
    assert_eq!(
        steps,
        (width / 2 - inside) as usize,
        "the sweep inside the room compared nothing like the stretch it claims to",
    );
    // Nothing saturates, anywhere. A tone map that reached white would be a
    // clamp again for whatever is brighter than the thing that reached it.
    for py in 0..height {
        for px in 0..width {
            let pixel = frame.pixel(px, py);
            assert!(
                pixel[..3].iter().all(|channel| *channel < 255),
                "at ({px}, {py}) the light view saturates: {pixel:?}",
            );
        }
    }
}

/// Write every debug view of a scene out as a picture, for a person to look at.
///
/// Ignored, and asserts nothing: the views are read as shapes, and a shape is
/// the one thing a count cannot check. Run it and look:
///
/// ```sh
/// OPENSHARD_VIEW_DUMP=/tmp/views cargo test -p openshard-client-render --test frame -- \
///     --ignored dump_the_lighting_views
/// ```
#[test]
#[ignore = "writes pictures for a person, and asserts nothing"]
fn dump_the_lighting_views() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let (width, height) = (64, 64);
    let dir = std::env::var_os("OPENSHARD_VIEW_DUMP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("views"));
    std::fs::create_dir_all(&dir).expect("the dump directory");
    let scenes = [
        ("room", openshard_client_render::scene::room()),
        (
            "window",
            openshard_client_render::scene::sunlit_room_with_window(),
        ),
        ("roofed", openshard_client_render::scene::roofed_room()),
    ];
    for (name, scene) in scenes {
        for view in View::ALL {
            let mut lighting = scene.lighting(0.0);
            lighting.view = view;
            let frame = parity_frame(&device, &queue, &lighting, width, height, Fixture::ground());
            let path = dir.join(format!("{name}-{}.png", view.name()));
            std::fs::write(
                &path,
                openshard_client_render::png::encode_rgba(width, height, &frame.pixels),
            )
            .expect("writing the frame");
            eprintln!("wrote {}", path.display());
        }
    }
}

/// A debug view reaches the shader, and draws what it says it draws.
///
/// Two things at once, and both are contracts no compiler checks: that
/// `View`'s numbers are the ones `blit.wgsl` switches on, and that the uniform
/// block's third header `vec4` lands where the shader looks for it — a field
/// written at the wrong offset would not fail validation, it would silently be
/// the light count or a corner of the occlusion grid.
#[test]
fn a_debug_view_reaches_the_shader() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let (width, height) = (64, 64);
    let scene = openshard_client_render::scene::room();
    let mut lighting = scene.lighting(0.0);
    lighting.view = View::Kind;
    let frame = parity_frame(&device, &queue, &lighting, width, height, Fixture::ground());

    // Every pixel of the fixture is land, and the kind view paints land one
    // colour whatever the lighting did — so a frame that still shows a pool of
    // firelight means the view never arrived.
    let land = [
        (0.20 * 255.0) as i32,
        (0.65 * 255.0) as i32,
        (0.30 * 255.0) as i32,
    ];
    for (px, py) in [(0, 0), (31, 31), (63, 63), (8, 40)] {
        let drawn = frame.pixel(px, py);
        for (channel, want) in land.iter().enumerate() {
            assert!(
                (want - i32::from(drawn[channel])).abs() <= 1,
                "at ({px}, {py}), channel {channel}: {} is not the land colour {want}",
                drawn[channel],
            );
        }
    }
}
