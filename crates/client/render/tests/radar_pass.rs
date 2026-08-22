//! The radar pass, against a real device.
//!
//! `radar_chunk.wgsl` and `radar_marker.wgsl` are validated when their
//! pipelines are created and at no other point: they are `include_str!`d, so a
//! syntax error or a binding that does not match the layout compiles clean and
//! fails at run time, on the first frame a player opens the window on. Every
//! test here builds its pass, which is what moves that failure to CI.
//!
//! Skipped where there is no GPU, like every other test in this crate that
//! needs one.

use openshard_client_render::gump::Frame;
use openshard_client_render::radar::{
    BASE_CHUNK_TILES, PLAYER_MARKER, RadarCache, RadarChunk, RadarChunkCoord, RadarExtent, RadarRegion,
    RadarTile, UNKNOWN,
};
use openshard_client_render::radar_pass::{Placement, RadarChunkRenderer, RadarMarker, RadarOverlayRenderer};
use openshard_protocol::world::Facet;
use openshard_uofiles::color::Color16;

/// What the surface is, and what the pass is built against.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn region(facet: Facet, origin: (u32, u32), extent: (u16, u16)) -> RadarRegion {
    RadarRegion::new(
        facet,
        RadarTile::from(origin),
        RadarExtent::new(extent.0, extent.1).expect("a non-empty GPU test region"),
    )
}

/// A GPU to draw with, or `None` where there is none. `frame.rs`'s, without the
/// G-buffer check: these passes draw quads onto an ordinary colour target and
/// ask for nothing above the defaults.
fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// **Two chunks in one region are two different pictures.** The regression this
/// exists for: the placement, the source rectangle and the page of each chunk
/// used to be written into one uniform buffer between recorded draws.
/// `Queue::write_buffer` is ordered against the submission rather than against
/// the commands inside it, so every draw in the frame read the *last* chunk's
/// values and the minimap showed one chunk's slice where all of them belonged.
/// Both halves being their own colour is what says each instance carries its
/// own rectangle.
#[test]
fn adjacent_chunks_each_draw_their_own_pixels() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU: skipping");
        return;
    };
    let cache = RadarCache::default();
    let facet = Facet(0);
    let solid = |x: u32, colour: Color16| {
        RadarChunk::new(
            cache.key(facet, 0, RadarChunkCoord::new(x, 0)),
            vec![colour; usize::from(BASE_CHUNK_TILES) * usize::from(BASE_CHUNK_TILES)],
        )
        .expect("a complete chunk")
    };
    // Red west, green east: two colours no reduction or filter could produce
    // from each other, so a half showing the wrong one is unambiguous.
    let west = solid(0, Color16(0x7C00));
    let east = solid(1, Color16(0x03E0));

    // Two whole chunks wide and one tall, drawn at one screen pixel a tile.
    let (width, height) = (u32::from(BASE_CHUNK_TILES) * 2, u32::from(BASE_CHUNK_TILES));
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let (target, view) = cleared_target(&device, &mut encoder, width, height);

    let mut chunks = RadarChunkRenderer::new(&device, FORMAT, 16 * 1024 * 1024);
    chunks.render_region(
        &device,
        &queue,
        &mut encoder,
        Frame {
            target: &view,
            width,
            height,
            scale: 1.0,
        },
        region(facet, (0, 0), (BASE_CHUNK_TILES * 2, BASE_CHUNK_TILES)),
        Placement {
            origin: (0.0, 0.0),
            extent: (width as f32, height as f32),
            circle: false,
            rotation: 0.0,
        },
        Placement {
            origin: (0.0, 0.0),
            extent: (width as f32, height as f32),
            circle: false,
            rotation: 0.0,
        },
        [&west, &east],
    );

    let pixels = read_back(&device, &queue, encoder, &target, width, height);
    let at = |x: u32, y: u32| {
        let i = ((y * width + x) * 4) as usize;
        (pixels[i], pixels[i + 1], pixels[i + 2])
    };
    let last = u32::from(BASE_CHUNK_TILES) - 1;
    assert_eq!(at(0, 0), (255, 0, 0), "the west chunk starts at the west edge");
    assert_eq!(at(last, last), (255, 0, 0), "and reaches its own far corner");
    assert_eq!(
        at(last + 1, 0),
        (0, 255, 0),
        "the east chunk starts one column past it, with its own colour and its \
         own page — not the west chunk's rectangle drawn twice",
    );
    assert_eq!(
        at(width - 1, height - 1),
        (0, 255, 0),
        "and reaches the east edge"
    );
}

/// **The body's marker is drawn over the terrain, not into it.** The overlay's
/// own pipeline is validated by being built, and the cross's shape and place are
/// asserted against the one chunk under it: a marker that landed a tile out, or
/// that the terrain pass drew over, is what this catches.
#[test]
fn a_marker_lands_on_its_tile_over_the_terrain() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU: skipping");
        return;
    };
    let cache = RadarCache::default();
    let facet = Facet(0);
    let ground = RadarChunk::new(
        cache.key(facet, 0, RadarChunkCoord::new(0, 0)),
        vec![Color16(0x7C00); usize::from(BASE_CHUNK_TILES) * usize::from(BASE_CHUNK_TILES)],
    )
    .expect("a complete chunk");

    // One chunk, one screen pixel a tile, so a tile coordinate and a pixel
    // coordinate are the same number and the cross can be written down.
    let side = u32::from(BASE_CHUNK_TILES);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let (target, view) = cleared_target(&device, &mut encoder, side, side);
    let frame = Frame {
        target: &view,
        width: side,
        height: side,
        scale: 1.0,
    };
    let region = region(facet, (0, 0), (BASE_CHUNK_TILES, BASE_CHUNK_TILES));
    let at = Placement {
        origin: (0.0, 0.0),
        extent: (side as f32, side as f32),
        circle: false,
        rotation: 0.0,
    };

    let mut chunks = RadarChunkRenderer::new(&device, FORMAT, 16 * 1024 * 1024);
    chunks.render_region(&device, &queue, &mut encoder, frame, region, at, at, [&ground]);
    let mut overlay = RadarOverlayRenderer::new(&device, FORMAT);
    overlay.render_markers(
        &device,
        &queue,
        &mut encoder,
        frame,
        region,
        at,
        at,
        &[RadarMarker {
            tile: RadarTile::new(10, 20),
            color: PLAYER_MARKER,
        }],
    );

    let pixels = read_back(&device, &queue, encoder, &target, side, side);
    let colour_at = |x: u32, y: u32| {
        let i = ((y * side + x) * 4) as usize;
        (pixels[i], pixels[i + 1], pixels[i + 2])
    };
    assert_eq!(colour_at(10, 20), (255, 255, 255), "the tile the body stands on");
    assert_eq!(colour_at(9, 20), (255, 255, 255), "and the arm west of it");
    assert_eq!(colour_at(10, 21), (255, 255, 255), "and the arm south of it");
    assert_eq!(
        colour_at(8, 20),
        (255, 0, 0),
        "two tiles out is terrain again — the cross is five tiles, not a blob",
    );
    assert_eq!(
        colour_at(9, 19),
        (255, 0, 0),
        "and the diagonal is not an arm, which is what says the marker was not \
         drawn as a square",
    );
}

/// **A window with no ready terrain is filled, not left see-through.** The
/// backdrop is what makes an unbuilt minimap read as unmapped ground rather than
/// as a hole with the world behind it, and the circular mask excludes the
/// square corners beneath the minimap's rim.
#[test]
fn an_unmapped_window_is_filled_rather_than_left_transparent() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU: skipping");
        return;
    };
    let (width, height) = (64u32, 64u32);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let (target, view) = cleared_target(&device, &mut encoder, width, height);

    let mut overlay = RadarOverlayRenderer::new(&device, FORMAT);
    overlay.render_backdrop(
        &device,
        &queue,
        &mut encoder,
        Frame {
            target: &view,
            width,
            height,
            scale: 1.0,
        },
        Placement {
            origin: (16.0, 16.0),
            extent: (32.0, 32.0),
            circle: true,
            rotation: 0.0,
        },
        UNKNOWN,
    );

    let pixels = read_back(&device, &queue, encoder, &target, width, height);
    let colour_at = |x: u32, y: u32| {
        let i = ((y * width + x) * 4) as usize;
        (pixels[i], pixels[i + 1], pixels[i + 2])
    };
    // `UNKNOWN` is deliberately not `Color16(0)`: near-black, but not the black
    // an untouched target is, so "nothing was drawn here" and "this ground is
    // not mapped" are two different pictures.
    let unmapped = {
        let rgb = UNKNOWN.rgb8();
        (rgb.red, rgb.green, rgb.blue)
    };
    assert_ne!(unmapped, (0, 0, 0), "unmapped is not absent");
    assert_eq!(colour_at(32, 16), unmapped, "the circle's north edge");
    assert_eq!(colour_at(32, 32), unmapped, "and its centre");
    assert_eq!(colour_at(16, 16), (0, 0, 0), "the square corner is masked");
    assert_eq!(colour_at(47, 47), (0, 0, 0), "and so is the opposite corner");
    assert_eq!(colour_at(15, 16), (0, 0, 0), "one column short of it is not");
    assert_eq!(colour_at(48, 47), (0, 0, 0), "and one column past it is not");
}

/// **A coarse stand-in fills the ground its children have not been built for,
/// and the built one paints over it.** The fallback the cache selects is only
/// worth selecting if the pass can draw it at its own LOD — which is the whole
/// of what this asserts, on the two products at once.
#[test]
fn a_coarse_ancestor_draws_under_the_one_chunk_that_is_ready() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU: skipping");
        return;
    };
    let cache = RadarCache::default();
    let facet = Facet(0);
    let pixels = |colour| vec![colour; usize::from(BASE_CHUNK_TILES) * usize::from(BASE_CHUNK_TILES)];
    // The level-one product over all four base chunks at the origin, and the
    // one base chunk of the four that has been built.
    let coarse = RadarChunk::new(
        cache.key(facet, 1, RadarChunkCoord::new(0, 0)),
        pixels(Color16(0x03E0)),
    )
    .expect("a complete chunk");
    let fine = RadarChunk::new(
        cache.key(facet, 0, RadarChunkCoord::new(0, 0)),
        pixels(Color16(0x7C00)),
    )
    .expect("a complete chunk");

    let side = u32::from(BASE_CHUNK_TILES) * 2;
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let (target, view) = cleared_target(&device, &mut encoder, side, side);
    let mut chunks = RadarChunkRenderer::new(&device, FORMAT, 16 * 1024 * 1024);
    chunks.render_region(
        &device,
        &queue,
        &mut encoder,
        Frame {
            target: &view,
            width: side,
            height: side,
            scale: 1.0,
        },
        region(facet, (0, 0), (BASE_CHUNK_TILES * 2, BASE_CHUNK_TILES * 2)),
        Placement {
            origin: (0.0, 0.0),
            extent: (side as f32, side as f32),
            circle: false,
            rotation: 0.0,
        },
        Placement {
            origin: (0.0, 0.0),
            extent: (side as f32, side as f32),
            circle: false,
            rotation: 0.0,
        },
        [&fine, &coarse],
    );

    let pixels = read_back(&device, &queue, encoder, &target, side, side);
    let colour_at = |x: u32, y: u32| {
        let i = ((y * side + x) * 4) as usize;
        (pixels[i], pixels[i + 1], pixels[i + 2])
    };
    let half = u32::from(BASE_CHUNK_TILES);
    assert_eq!(
        colour_at(0, 0),
        (255, 0, 0),
        "the built chunk owns its own quarter"
    );
    assert_eq!(colour_at(half - 1, half - 1), (255, 0, 0), "all of it");
    assert_eq!(
        colour_at(half, 0),
        (0, 255, 0),
        "and the stand-in covers the three quarters that are not built yet",
    );
    assert_eq!(colour_at(0, half), (0, 255, 0));
    assert_eq!(colour_at(side - 1, side - 1), (0, 255, 0));
}

/// A cleared colour target to draw a radar into, and the view of it.
///
/// Every pass here loads rather than clears, for the gump pass's reason, so
/// something has to have painted the target before one runs.
fn cleared_target(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("radar test target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    encoder
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        })
        .forget_lifetime();
    (target, view)
}

/// Submit `encoder` and read the target back as RGBA8 rows.
fn read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mut encoder: wgpu::CommandEncoder,
    target: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<u8> {
    // `copy_texture_to_buffer` wants 256-byte rows; 64 pixels is exactly one, so
    // the fixture's width is chosen to make the padding arithmetic a no-op and
    // the assertion above readable.
    let stride = width * 4;
    assert!(stride.is_multiple_of(256), "the fixture avoids row padding");

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("radar readback"),
        size: u64::from(stride * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: target,
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
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the readback maps");
    let bytes = slice
        .get_mapped_range()
        .expect("the mapped range is there once the poll returned")
        .to_vec();
    readback.unmap();
    bytes
}
