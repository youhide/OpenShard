//! **Getting a frame off the GPU, so that two frames can be compared.**
//!
//! [`crate::frame::assemble`] made one assembly out of seven; this is the other
//! half of the same argument (`docs/parity.md`): a frame that cannot be read
//! back is a frame nothing can be said about. The tools could always dump a
//! picture — each of them with its own hand-rolled readback, four copies of the
//! same padded-row arithmetic at the last count — and the client, the thing that
//! is actually broken, could dump nothing at all. Every artefact a person
//! reported was therefore *searched for* in a tool's picture rather than
//! compared against the client's, which is how a defect stayed a session long in
//! a tool that had been reproducing it the whole time.
//!
//! So both ends read a frame back through here, and a plane dumped by the client
//! and a plane dumped by a tool are the same bytes for the same frame or they
//! are a finding.
//!
//! # What a plane is
//!
//! The lit frame folds every disagreement into one colour. The G-buffer's own
//! values do not: position, normal, solid, place each answer a different
//! question, and [`View`] is which of them the blit paints instead of the
//! picture. So a *plane* here is one blit of one assembled frame under one
//! [`View`] — the same pass the client draws with, not a second reader of the
//! same textures — and [`planes`] is that repeated over a list of views without
//! rebuilding anything in between. `docs/parity.md` D5: a comparison names which
//! plane disagreed, and "the frame differs" is not an answer.

use crate::blit::{
    Blit,
    Frame,
    ViewportRect,
};
use crate::debug::View;
use crate::light::Lighting;

/// One assembled frame as one PNG per view.
///
/// # Contract
///
/// `frame.target` must be a view of `into`, and `into` must carry
/// [`wgpu::TextureUsages::COPY_SRC`] and cover `frame.rect`. The two travel
/// together because the blit draws through a *view* and a copy reads a
/// *texture*, and nothing in `wgpu` relates the one to the other — so the caller
/// makes them at one place, one line apart, and this is the whole of what it has
/// to keep true.
///
/// The frame is drawn once per view, into the same texture, with everything else
/// held: the same world image, the same G-buffer, the same instance buffers, the
/// same lighting. Nothing is collected again between two planes, which is what
/// makes them planes *of one frame* rather than of one frame each.
///
/// The frame's own [`Lighting`] is *copied* rather than borrowed mutably, and
/// the copy is what has its view set thirteen times. A dump is a second reader
/// of a frame that has already been drawn, and a second reader that reaches into
/// what the first one was handed is the shape `frame::assemble`'s own note warns
/// about — "nothing may touch the lighting between the assembly and the blit"
/// stops meaning anything the moment a diagnostic is allowed to.
pub fn planes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    blit: &mut Blit,
    into: &wgpu::Texture,
    frame: Frame<'_>,
    lighting: &Lighting,
    views: &[View],
) -> Vec<(View, Vec<u8>)> {
    plane_bytes(device, queue, blit, into, frame, lighting, views)
        .into_iter()
        .map(|(view, pixels)| {
            (
                view,
                crate::png::encode_rgba(frame.rect.width, frame.rect.height, &pixels),
            )
        })
        .collect()
}

/// [`planes`] without the PNG step: one blit per view, read back as raw RGBA8.
///
/// `docs/parity.md` P3 wants this half and not the picture — a gate counts
/// *differing pixels*, which means two runs of raw bytes to zip, not two files
/// to open. `planes` is this with an encode on the end, kept as the one place
/// the loop over views is written.
pub fn plane_bytes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    blit: &mut Blit,
    into: &wgpu::Texture,
    frame: Frame<'_>,
    lighting: &Lighting,
    views: &[View],
) -> Vec<(View, Vec<u8>)> {
    // **The world's format and not a surface's**, and it is checked here rather
    // than assumed: what comes back is written out as RGBA8 bytes, so a texture
    // in any other format is either a picture with its channels swapped or —
    // where a texel is not four bytes at all — a readback that cannot even be
    // asked for. A window's surface is whatever the compositor offered
    // (`Bgra8Unorm`, `Rgba16Float`), so the client's dump goes through a target
    // and a pipeline of its own; the tool's already did. Two dumps are
    // comparable byte for byte or they are not a comparison.
    assert_eq!(
        into.format(),
        crate::blit::WORLD_FORMAT,
        "a dump is read back as RGBA8, so it is drawn into the world's own format",
    );
    let mut shown = lighting.clone();
    views
        .iter()
        .map(|&view| {
            shown.view = view;
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame dump"),
            });
            blit.render(device, queue, &mut encoder, frame, &shown);
            queue.submit([encoder.finish()]);
            (view, read_rect(device, queue, into, frame.rect))
        })
        .collect()
}

/// A rectangle of a texture, as tight rows of its own texels.
///
/// **The padding is the whole reason this is not three lines at each call site.**
/// A texture-to-buffer copy wants every row a multiple of
/// [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`], and a viewport's width is whatever a
/// person left the window at — so a readback that ignores it either panics or,
/// worse, quietly reads a picture sheared by a few pixels a row. Every tool in
/// this crate had its own copy of this arithmetic, and the ones that "did not
/// need it" were the ones whose width happened to be aligned.
///
/// **The texel's size comes from the format** rather than from the four bytes
/// every caller here happens to want. A window's surface is whatever the
/// compositor offered — `Rgba16Float` is eight — and a row measured as
/// `width * 4` against it is not a shorter row, it is a copy `wgpu` refuses
/// outright ("bytes per row is less than the number of bytes in a complete
/// row"). What comes back is therefore *bytes of `texture.format()`*, and
/// turning them into a picture is the caller's business.
///
/// `texture` must carry [`wgpu::TextureUsages::COPY_SRC`], contain `rect`, and
/// be an uncompressed colour format. The wait is unconditional: what comes back
/// is this submission's own pixels, not whatever the last frame left.
pub fn read_rect(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    rect: ViewportRect,
) -> Vec<u8> {
    let texel = texture
        .format()
        .block_copy_size(None)
        .expect("an uncompressed colour format, whose block is one texel");
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let row = rect.width * texel;
    let padded = row + (align - row % align) % align;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label:              Some("frame readback"),
        size:               u64::from(padded) * u64::from(rect.height),
        usage:              wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frame readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: rect.x,
                y: rect.y,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset:         0,
                bytes_per_row:  Some(padded),
                rows_per_image: Some(rect.height),
            },
        },
        wgpu::Extent3d {
            width:                 rect.width,
            height:                rect.height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("mapping a buffer this crate has just written");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting on our own submission");
    let mapped = slice.get_mapped_range().expect("the map completed above");
    let mut pixels = Vec::with_capacity((row * rect.height) as usize);
    for line in 0..rect.height {
        let start = (line * padded) as usize;
        pixels.extend_from_slice(&mapped[start..start + row as usize]);
    }
    drop(mapped);
    buffer.unmap();
    pixels
}
