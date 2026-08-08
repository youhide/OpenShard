//! The gump pass on a real device: art lands where the layout put it, at the
//! size the scale asks for.
//!
//! Separate from `frame.rs` because it shares nothing with it: no camera, no
//! depth buffer, no place attachment and no world image — which is the whole
//! argument in `openshard_client_render::gump` for the pass existing, stated
//! once more as a test harness that needs none of those.
//!
//! Synthetic art throughout, so this runs without a client installed: what is
//! under test is placement and scaling, and a solid block proves those where a
//! real gump would only make a failure harder to read.

use openshard_client_render::gump::{self, Frame, GumpArt, GumpAtlas, GumpPixel, GumpRenderer, Picture};
use openshard_client_render::hue::HueRamp;
use openshard_protocol::wire::Graphic;
use openshard_uofiles::color::Color16;
use openshard_uofiles::hues::Hues;
use openshard_uofiles::image::Image;

/// A device, or `None` where the test runner has no GPU — the same skip
/// `frame.rs` takes, and for the same reason: a headless CI box is not a
/// failure.
fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// A solid opaque block, the one picture every test here draws.
fn block(width: u16, height: u16) -> Image {
    Image::new(
        width,
        height,
        vec![Color16(0x7FFF); usize::from(width) * usize::from(height)],
    )
}

/// A rendered frame, as RGBA8 rows.
struct Rendered {
    width: u32,
    pixels: Vec<u8>,
}

impl Rendered {
    fn drawn(&self, x: u32, y: u32) -> bool {
        let at = ((y * self.width + x) * 4) as usize;
        self.pixels[at + 3] == u8::MAX
    }

    fn drawn_count(&self) -> usize {
        self.pixels
            .chunks_exact(4)
            .filter(|pixel| pixel[3] == u8::MAX)
            .count()
    }
}

/// Clear a target, draw `pictures` over it, and read the result back.
///
/// The clear is the harness's own and not the pass's: the gump pass loads what
/// is already on the surface, because on a real frame that is the world.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &GumpAtlas,
    pictures: &[Picture],
    width: u32,
    height: u32,
    scale: f32,
) -> Rendered {
    assert_eq!(width * 4 % 256, 0, "a row copy has to be 256-byte aligned");
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gump frame"),
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
        label: Some("readback"),
        size: u64::from(width) * u64::from(height) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    // One row of hues, all black: nothing here asks for a tint, and the ramp is
    // bound whether or not it is read.
    let ramp = HueRamp::build(&Hues::parse(&[0u8; 708]).expect("one empty group"));
    let mut pass = GumpRenderer::new(device, queue, format, atlas.pixels(), &ramp);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        })
        .forget_lifetime();

    let quads = gump::collect(pictures, atlas);
    pass.render(
        device,
        queue,
        &mut encoder,
        Frame {
            target: &view,
            width,
            height,
            scale,
        },
        &quads,
    );
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
    Rendered { width, pixels }
}

fn atlas_of(pictures: impl IntoIterator<Item = (Graphic, Image)>) -> GumpAtlas {
    // Through the same shelf packer the real one uses, with the pictures handed
    // in directly instead of decoded out of a client's container. Gump art, not
    // item art: every picture in here stands for something out of
    // `gumpartLegacyMUL.uop`.
    GumpAtlas::pack(
        pictures
            .into_iter()
            .map(|(graphic, image)| (GumpArt::Gump(graphic), image)),
    )
    .expect("small blocks fit an atlas 2048 on a side")
}

/// One picture, at one scale: the rectangle it covers is its own, at the
/// coordinate the layout named, and nothing outside it is touched.
#[test]
fn a_picture_covers_exactly_its_own_rectangle() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let atlas = atlas_of([(Graphic(1), block(8, 4))]);
    let frame = render(
        &device,
        &queue,
        &atlas,
        &[Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(10, 6))],
        64,
        64,
        1.0,
    );

    assert_eq!(frame.drawn_count(), 8 * 4, "the art's own area and no more");
    assert!(frame.drawn(10, 6), "its top-left corner");
    assert!(frame.drawn(17, 9), "its bottom-right corner");
    assert!(!frame.drawn(9, 6), "one pixel left of it");
    assert!(!frame.drawn(18, 9), "one pixel right of it");
    assert!(!frame.drawn(10, 10), "one pixel below it");
}

/// The scale multiplies coordinates and art together — which is the whole
/// reason the interface left egui, where a bitmap could not follow a point.
#[test]
fn the_scale_moves_a_picture_as_far_as_it_grows_it() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let atlas = atlas_of([(Graphic(1), block(8, 4))]);
    let frame = render(
        &device,
        &queue,
        &atlas,
        &[Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(10, 6))],
        64,
        64,
        2.0,
    );

    assert_eq!(frame.drawn_count(), 8 * 4 * 4, "twice as wide and twice as tall");
    assert!(frame.drawn(20, 12), "the corner, moved by the same scale");
    assert!(frame.drawn(35, 19), "the far corner");
    assert!(!frame.drawn(19, 12), "and nothing left of it");
    assert!(!frame.drawn(36, 19));
}

/// A tiled strip fills its box exactly, whether or not the box is a whole
/// number of repetitions. The clipped last one is what a window of an arbitrary
/// width is made of.
#[test]
fn a_tiled_strip_fills_its_box_to_the_pixel() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let atlas = atlas_of([(Graphic(1), block(6, 3))]);
    let frame = render(
        &device,
        &queue,
        &atlas,
        &[Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(4, 4)).tiled(20, 7)],
        64,
        64,
        1.0,
    );

    assert_eq!(frame.drawn_count(), 20 * 7, "the box, not a multiple of the art");
    assert!(frame.drawn(23, 10), "its far corner");
    assert!(!frame.drawn(24, 10), "and not one pixel past it");
}

/// Later covers earlier: with no depth buffer, the caller's order is the
/// interface's order, which is what lets a background be listed before the
/// buttons standing on it.
#[test]
fn a_later_picture_covers_an_earlier_one() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let white = block(8, 8);
    let mut grey = block(8, 8);
    grey = Image::new(8, 8, vec![Color16(0x3DEF); grey.pixels().len()]);
    let atlas = atlas_of([(Graphic(1), white), (Graphic(2), grey)]);
    let frame = render(
        &device,
        &queue,
        &atlas,
        &[
            Picture::plain(GumpArt::Gump(Graphic(1)), GumpPixel::new(4, 4)),
            Picture::plain(GumpArt::Gump(Graphic(2)), GumpPixel::new(4, 4)),
        ],
        64,
        64,
        1.0,
    );

    let at = ((4 * frame.width + 4) * 4) as usize;
    assert!(
        frame.pixels[at] < 200,
        "the second picture's grey, not the first's white"
    );
}
