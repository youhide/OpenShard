//! What is drawn, and only that.
//!
//! [`App::draw`] and [`App::advance`] are the two halves of a frame — the
//! clock and the picture — and [`App::draw_from`] is the picture itself:
//! atlases grown, geometry assembled, every pass encoded. [`App::frame_facts`]
//! is the one place a pick happens, because the highlight and the tile marker
//! have to agree with what was actually drawn. [`assemble_geometry`] and the
//! free functions beside it are kept free rather than folded into
//! `draw_from` on purpose — see that method's own doc for the borrow-checker
//! reason.
//!
//! **A pure reader of command state.** Nothing here writes a walk target, a
//! gump's contents or anything a packet fills in — that is `net_command.rs`'s
//! and `ui_command.rs`'s and `own_windows.rs`'s job, upstream of a frame. What
//! this file *does* still mutate is purely presentational: animation clocks,
//! the atlases, the frame counters — state about how a picture is drawn, not
//! about what is true.

use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use openshard_client_render::atlas::{AnimAtlas, AnimationKey, StaticAtlasPage};
use openshard_client_render::blit::{self, Blit, ViewportRect};
use openshard_client_render::camera::{Camera, ViewPixel};
use openshard_client_render::composite::{
    CompositeKey, CompositeProducerJob, CompositeQuarantineReason, CompositeTexture, CompositeTier,
    ImmutableRevision, MapBlockBounds,
};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::debug::View;
use openshard_client_render::gbuffer::Gbuffer;
use openshard_client_render::gump::GumpPixel;
use openshard_client_render::items::{self};
use openshard_client_render::lod::BlockLod;
use openshard_client_render::mobiles::{self, Mobile};
use openshard_client_render::outline::{self};
use openshard_client_render::radar::{self, RadarBuildScratch, build_chunk_reusing, build_ready_ancestors};
use openshard_client_render::radar_pass::Placement;
use openshard_client_render::renderer::{self, Target};
use openshard_client_render::sprite::SpriteQuad;
use openshard_client_render::text::{self, Label};
use openshard_client_render::{ground, light, statics};
use openshard_protocol::speech::Font;
use openshard_protocol::wire::Hue;
use openshard_protocol::world::Point;
use openshard_uofiles::grid::BlockCoord;
use openshard_uofiles::map::Map;

use crate::app::App;
use crate::chat::draw_chat_and_speech;
use crate::crowd::{Crowd, Who};
use crate::diagnostics::Pick;
use crate::frame_geometry::{FrameFacts, assemble_geometry};
use crate::graphics::HighlightTarget;
use crate::picking::SelectedIdentity;
use crate::profile;
use crate::render_passes::{WorldPassAudit, draw_gump_windows, encode_world_passes};
use crate::window::{Screen, prepare_composite_job, ready_atlases};
use crate::world::{
    DAMAGE_NUMBER_HOLD, DAMAGE_NUMBER_RISE, PlayerMotion, SPEECH_LINE_HEIGHT, advance_presentation_to,
};

mod composite_producer;

/// Text which belongs to a thing in the world, rather than to a client window
/// or the HUD.
///
/// `fonts.mul` glyphs draw into the world texture, so their quads travel with
/// [`encode_world_passes`]. A TrueType glyph must instead be drawn after the
/// world has been blitted to the surface: unlike a pixel-art sprite, it must
/// not be scaled by camera zoom. The two routes differ technically, but they
/// occupy the same compositor layer. Keeping that fact in this enum makes it
/// impossible for a TrueType world label to quietly become HUD text again.
enum WorldText<'a> {
    Bitmap(Vec<SpriteQuad>),
    TrueType {
        labels: Vec<text::ScreenLabel<'a>>,
        counts: Vec<text::ScreenLabel<'a>>,
    },
}

impl WorldText<'_> {
    /// The part that is rasterized into the camera's world texture.
    fn bitmap_quads(&self) -> &[SpriteQuad] {
        match self {
            Self::Bitmap(quads) => quads,
            Self::TrueType { .. } => &[],
        }
    }
}

/// Draw the TrueType half of [`WorldText`] after the world reaches the surface
/// and before a client window can cover it.
///
/// This is deliberately separate from `draw_chat_and_speech`: a name, speech
/// bubble, damage number or pile count is anchored to the world even when the
/// font needs a surface-space pass. `GumpRenderer::render_layer` gives this
/// layer an independent instance buffer, so later HUD text cannot replace it.
fn draw_world_text(
    resources: &crate::resources::Resources,
    window: &mut Screen,
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    fonts: crate::desk::FontSizes,
    density: f32,
    text: &WorldText<'_>,
) {
    let WorldText::TrueType { labels, counts } = text else {
        return;
    };
    let font = resources
        .ttf_font
        .as_ref()
        .expect("TrueType world text requires the configured TrueType face");
    let speech_size = fonts.speech.scaled(density);
    let count_size = fonts.stack_count.scaled(density);
    let quads = {
        let atlas = window
            .ttf_atlas
            .as_mut()
            .expect("create_window builds ttf_atlas whenever ttf_font is set");
        if let Err(error) = atlas.add_or_reset(
            font,
            speech_size,
            labels.iter().flat_map(|label| label.text.chars()),
        ) {
            eprintln!("packing world TTF glyphs: {error}");
        }
        if !counts.is_empty() {
            if let Err(error) = atlas.add_or_reset(
                font,
                count_size,
                counts.iter().flat_map(|label| label.text.chars()),
            ) {
                eprintln!("packing world TTF glyphs: {error}");
            }
        }
        let mut quads = text::collect_screen_ttf(labels, atlas, speech_size);
        quads.extend(text::collect_screen_ttf(counts, atlas, count_size));
        quads
    };
    window.upload_ttf_dirty();
    let timed = profile::begin(window.gpu.as_ref(), "world text", encoder);
    window
        .ttf_gump_pass
        .as_mut()
        .expect("create_window builds ttf_gump_pass whenever ttf_atlas is")
        .render_layer(
            &window.device,
            &window.queue,
            encoder,
            openshard_client_render::gump::Frame {
                target: view,
                width: window.config.width,
                height: window.config.height,
                // `ScreenLabel` coordinates and glyphs are already real pixels.
                scale: 1.0,
            },
            &quads,
        );
    profile::end(window.gpu.as_ref(), encoder, timed);
}

/// Read one texture into packed rows for an opt-in producer/cache audit.
fn audit_texture_bytes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    bytes_per_texel: u32,
    label: &'static str,
) -> Option<Vec<u8>> {
    let row = texture.width() * bytes_per_texel;
    let stride = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: u64::from(stride) * u64::from(texture.height()),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
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
                rows_per_image: Some(texture.height()),
            },
        },
        wgpu::Extent3d {
            width: texture.width(),
            height: texture.height(),
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (sent, received) = mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
        let _ = sent.send(result);
    });
    if device.poll(wgpu::PollType::wait_indefinitely()).is_err()
        || received.recv().ok().and_then(Result::ok).is_none()
    {
        return None;
    }
    let mapped = readback.slice(..).get_mapped_range().ok()?;
    let packed = mapped
        .chunks_exact(stride as usize)
        .flat_map(|source_row| source_row[..row as usize].iter().copied())
        .collect();
    drop(mapped);
    readback.unmap();
    Some(packed)
}

/// Read the completed cache entry itself, after the producer command buffer
/// has run and before any camera frame can restore it.  This is deliberately
/// opt-in: it waits for a GPU map, which is appropriate for the injected field
/// scenario but never for ordinary play.
fn audit_captured_composite_ids(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    map: &Map,
    captured: &CompositeTexture,
    source_color: &wgpu::Texture,
    source_ids: &wgpu::Texture,
) {
    if std::env::var_os("OPENSHARD_COMPOSITE_AUDIT").is_none() {
        return;
    }
    let Some((ids, _, _, _)) = captured.deferred_textures_for_audit() else {
        return;
    };
    let row = ids.width() * 4;
    let stride = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let bytes = u64::from(stride) * u64::from(ids.height());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("map composite IDs audit readback"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("map composite IDs audit"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: ids,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(ids.height()),
            },
        },
        wgpu::Extent3d {
            width: ids.width(),
            height: ids.height(),
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (sent, received) = mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
        let _ = sent.send(result);
    });
    if device.poll(wgpu::PollType::wait_indefinitely()).is_err()
        || received.recv().ok().and_then(Result::ok).is_none()
    {
        tracing::warn!(key = ?captured.key(), "could not read captured map-composite IDs for audit");
        return;
    }
    let mapped = readback
        .slice(..)
        .get_mapped_range()
        .expect("completed map-composite audit has mapped bytes");
    let (mut nothing, mut land, mut statics, mut mobile, mut invalid) = (0_u64, 0_u64, 0_u64, 0_u64, 0_u64);
    for source_row in mapped.chunks_exact(stride as usize) {
        for word in source_row[..row as usize].chunks_exact(4) {
            match openshard_client_render::gbuffer::ids_kind(u32::from_le_bytes(
                word.try_into().expect("four ID bytes"),
            )) {
                Some(openshard_client_render::place::Kind::Nothing) => nothing += 1,
                Some(openshard_client_render::place::Kind::Land) => land += 1,
                Some(openshard_client_render::place::Kind::Static) => statics += 1,
                Some(openshard_client_render::place::Kind::Mobile) => mobile += 1,
                None => invalid += 1,
            }
        }
    }
    let job = CompositeProducerJob::for_flat_ground(captured.key(), captured.ground());
    let divisor = captured.key().tier.source_pixels_per_texel();
    let mut missing_owner_centres = Vec::new();
    let (first_x, first_y) = openshard_client_render::composite::tile_origin(captured.key().block);
    for y in first_y..first_y + openshard_uofiles::map::BLOCK_SIZE as u16 {
        for x in first_x..first_x + openshard_uofiles::map::BLOCK_SIZE as u16 {
            let Some(land) = map.land(x, y) else {
                continue;
            };
            let corners = ground::corner_heights(map, x, y, land.z);
            // Slopes deliberately remain in the live LOD0 layer. Their absent
            // producer texel is the expected result, not a cache coverage
            // hole; the full-scene oracle checks their current-frame result.
            if !corners.iter().all(|height| *height == corners[0]) {
                continue;
            }
            let at = job
                .camera()
                .to_screen(openshard_protocol::world::Point::new(x, y, land.z));
            let sample_x = (at.x.max(0) as u32 / divisor).min(ids.width() - 1);
            let sample_y = (at.y.max(0) as u32 / divisor).min(ids.height() - 1);
            let offset = sample_y as usize * stride as usize + sample_x as usize * 4;
            let id = u32::from_le_bytes(
                mapped[offset..offset + 4]
                    .try_into()
                    .expect("one cached ID texel"),
            );
            if openshard_client_render::gbuffer::ids_kind(id)
                == Some(openshard_client_render::place::Kind::Nothing)
                && missing_owner_centres.len() < 12
            {
                missing_owner_centres.push((x, y));
            }
        }
    }
    drop(mapped);
    readback.unmap();
    tracing::info!(
        key = ?captured.key(),
        width = ids.width(),
        height = ids.height(),
        nothing,
        land,
        statics,
        mobile,
        invalid,
        missing_owner_centres = ?missing_owner_centres,
        "captured map-composite IDs before restore"
    );
    if captured.key().tier == openshard_client_render::composite::CompositeTier::Lod1 {
        let Some(source_color) =
            audit_texture_bytes(device, queue, source_color, 4, "composite source colour audit")
        else {
            tracing::warn!(key = ?captured.key(), "could not read LOD1 producer colour for audit");
            return;
        };
        let Some(captured_color) = audit_texture_bytes(
            device,
            queue,
            captured.texture(),
            4,
            "composite cached colour audit",
        ) else {
            tracing::warn!(key = ?captured.key(), "could not read LOD1 cached colour for audit");
            return;
        };
        let Some(source_ids) =
            audit_texture_bytes(device, queue, source_ids, 4, "composite source IDs audit")
        else {
            tracing::warn!(key = ?captured.key(), "could not read LOD1 producer IDs for audit");
            return;
        };
        let Some(captured_ids) =
            audit_texture_bytes(device, queue, ids, 4, "composite cached IDs equality audit")
        else {
            tracing::warn!(key = ?captured.key(), "could not read LOD1 cached IDs for equality audit");
            return;
        };
        let color_difference = source_color
            .iter()
            .zip(&captured_color)
            .position(|(source, captured)| source != captured);
        let ids_difference = source_ids
            .iter()
            .zip(&captured_ids)
            .position(|(source, captured)| source != captured);
        if source_color.len() == captured_color.len()
            && source_ids.len() == captured_ids.len()
            && color_difference.is_none()
            && ids_difference.is_none()
        {
            tracing::info!(key = ?captured.key(), "lossless LOD1 cache bytes match producer source");
        } else {
            tracing::error!(
                key = ?captured.key(),
                source_color_bytes = source_color.len(),
                captured_color_bytes = captured_color.len(),
                source_ids_bytes = source_ids.len(),
                captured_ids_bytes = captured_ids.len(),
                ?color_difference,
                ?ids_difference,
                "lossless LOD1 cache bytes differ from producer source"
            );
        }
    }
}

/// Compare the resident static-atlas texture against the bytes that the CPU
/// atlas says belong there. The injected max-zoom soak calls this sparingly,
/// after all dirty-row uploads for that frame have been queued.
fn audit_static_atlas_pages(window: &crate::window::Screen) {
    fn digest(bytes: &[u8]) -> u64 {
        let mut hash = DefaultHasher::new();
        bytes.hash(&mut hash);
        hash.finish()
    }

    for index in 0..window.atlases.statics.page_count() {
        let page = StaticAtlasPage(index as u8);
        let cpu = window
            .atlases
            .statics
            .page(page)
            .expect("static atlas page_count owns every page");
        let Some(texture) = window.statics.atlas_page_texture_for_audit(page) else {
            tracing::error!(page = index, "static atlas CPU page has no GPU texture");
            continue;
        };
        let row = texture.width() * 4;
        let stride = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = window.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("static atlas soak readback"),
            size: u64::from(stride) * u64::from(texture.height()),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = window
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("static atlas soak audit"),
            });
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
                    rows_per_image: Some(texture.height()),
                },
            },
            wgpu::Extent3d {
                width: texture.width(),
                height: texture.height(),
                depth_or_array_layers: 1,
            },
        );
        window.queue.submit([encoder.finish()]);
        let (sent, received) = mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = sent.send(result);
        });
        if window.device.poll(wgpu::PollType::wait_indefinitely()).is_err()
            || received.recv().ok().and_then(Result::ok).is_none()
        {
            tracing::error!(page = index, "could not read static atlas GPU texture");
            continue;
        }
        let mapped = readback
            .slice(..)
            .get_mapped_range()
            .expect("completed static atlas audit has mapped bytes");
        let gpu = mapped
            .chunks_exact(stride as usize)
            .flat_map(|source_row| source_row[..row as usize].iter().copied())
            .collect::<Vec<_>>();
        drop(mapped);
        readback.unmap();
        let cpu_hash = digest(cpu.pixels());
        let gpu_hash = digest(&gpu);
        if cpu_hash == gpu_hash && cpu.pixels() == gpu {
            tracing::info!(
                page = index,
                revision = window.atlases.statics.revision(),
                bytes = gpu.len(),
                hash = cpu_hash,
                "static atlas CPU and GPU state agree"
            );
        } else {
            tracing::error!(
                page = index,
                revision = window.atlases.statics.revision(),
                cpu_hash,
                gpu_hash,
                "static atlas GPU state differs from CPU source"
            );
        }
    }

    let (land, texmaps) = window.renderer.atlas_textures_for_audit();
    audit_atlas_texture(window, "land", land, window.atlases.land.pixels());
    audit_atlas_texture(window, "texmaps", texmaps, window.atlases.texmaps.pixels());
}

/// Compare one ordinary RGBA atlas texture with its CPU packing bytes.
fn audit_atlas_texture(
    window: &crate::window::Screen,
    label: &'static str,
    texture: &wgpu::Texture,
    cpu: &[u8],
) {
    let row = texture.width() * 4;
    let stride = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = window.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("atlas soak readback"),
        size: u64::from(stride) * u64::from(texture.height()),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = window
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("atlas soak audit"),
        });
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
                rows_per_image: Some(texture.height()),
            },
        },
        wgpu::Extent3d {
            width: texture.width(),
            height: texture.height(),
            depth_or_array_layers: 1,
        },
    );
    window.queue.submit([encoder.finish()]);
    let (sent, received) = mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
        let _ = sent.send(result);
    });
    if window.device.poll(wgpu::PollType::wait_indefinitely()).is_err()
        || received.recv().ok().and_then(Result::ok).is_none()
    {
        tracing::error!(atlas = label, "could not read atlas GPU texture");
        return;
    }
    let mapped = readback
        .slice(..)
        .get_mapped_range()
        .expect("completed atlas audit has mapped bytes");
    let gpu = mapped
        .chunks_exact(stride as usize)
        .flat_map(|source_row| source_row[..row as usize].iter().copied())
        .collect::<Vec<_>>();
    drop(mapped);
    readback.unmap();
    let mut cpu_hasher = DefaultHasher::new();
    cpu.hash(&mut cpu_hasher);
    let mut gpu_hasher = DefaultHasher::new();
    gpu.hash(&mut gpu_hasher);
    let (cpu_hash, gpu_hash) = (cpu_hasher.finish(), gpu_hasher.finish());
    if cpu_hash == gpu_hash && cpu == gpu {
        tracing::info!(
            atlas = label,
            bytes = gpu.len(),
            hash = cpu_hash,
            "atlas CPU and GPU state agree"
        );
    } else {
        tracing::error!(
            atlas = label,
            cpu_hash,
            gpu_hash,
            "atlas GPU state differs from CPU source"
        );
    }
}

/// Compare the bytes the scene renderer will fetch for this frame against the
/// current CPU serialization. This is the direct oracle for a suspected
/// circular/staging overwrite of sprite placement rather than atlas pixels.
fn audit_scene_instance_buffers(window: &crate::window::Screen) {
    for (label, (source, expected)) in [
        ("map statics", window.statics.instance_state_for_audit()),
        ("items", window.items_pass.instance_state_for_audit()),
        ("mobiles", window.mobile_pass.instance_state_for_audit()),
    ] {
        if expected.is_empty() {
            continue;
        }
        let readback = window.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene instance soak readback"),
            size: expected.len() as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = window
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scene instance soak audit"),
            });
        encoder.copy_buffer_to_buffer(source, 0, &readback, 0, expected.len() as u64);
        window.queue.submit([encoder.finish()]);
        let (sent, received) = mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = sent.send(result);
        });
        if window.device.poll(wgpu::PollType::wait_indefinitely()).is_err()
            || received.recv().ok().and_then(Result::ok).is_none()
        {
            tracing::error!(scene = label, "could not read scene instance buffer");
            continue;
        }
        let actual = readback
            .slice(..)
            .get_mapped_range()
            .expect("completed scene audit has mapped bytes")
            .to_vec();
        readback.unmap();
        if actual == expected {
            tracing::info!(
                scene = label,
                bytes = expected.len(),
                "scene instance CPU and GPU state agree"
            );
        } else {
            let first_difference = actual
                .iter()
                .zip(expected)
                .position(|(actual, expected)| actual != expected);
            tracing::error!(
                scene = label,
                bytes = expected.len(),
                ?first_difference,
                "scene instance GPU state differs from current CPU rows"
            );
        }
    }
}

/// Inspect the actual frame G-buffer at every visible ground-tile centre.
///
/// This catches the failure a picture can only suggest: a map block was marked
/// ready (therefore its LOD0 rows were omitted), but its restored deferred
/// rectangle wrote `Kind::Nothing` at a tile it owns.  The check is opt-in
/// because mapping a full screen attachment intentionally fences the device.
fn audit_visible_ground_centres(window: &crate::window::Screen, map: &Map, camera: Camera) {
    let ids = window.gbuffer.ids();
    let row = ids.width() * 4;
    let stride = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let position = window.gbuffer.position();
    let position_row = position.width() * 16;
    let position_stride =
        position_row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = window.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("LOD screen G-buffer audit readback"),
        size: u64::from(stride) * u64::from(ids.height()),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let position_readback = window.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("LOD screen G-buffer position audit readback"),
        size: u64::from(position_stride) * u64::from(position.height()),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = window
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("LOD screen G-buffer audit"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: ids,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(ids.height()),
            },
        },
        wgpu::Extent3d {
            width: ids.width(),
            height: ids.height(),
            depth_or_array_layers: 1,
        },
    );
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: position,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &position_readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(position_stride),
                rows_per_image: Some(position.height()),
            },
        },
        wgpu::Extent3d {
            width: position.width(),
            height: position.height(),
            depth_or_array_layers: 1,
        },
    );
    window.queue.submit([encoder.finish()]);
    let (sent, received) = mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
        let _ = sent.send(result);
    });
    let (position_sent, position_received) = mpsc::channel();
    position_readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = position_sent.send(result);
        });
    if window.device.poll(wgpu::PollType::wait_indefinitely()).is_err()
        || received.recv().ok().and_then(Result::ok).is_none()
        || position_received.recv().ok().and_then(Result::ok).is_none()
    {
        tracing::error!("could not read frame G-buffer for LOD bounds audit");
        return;
    }
    let samples: Vec<_> = camera
        .visible_tiles()
        .clamp_to(map.width(), map.height())
        .into_iter()
        .flat_map(|(xs, ys)| {
            ys.flat_map(move |y| {
                xs.clone().filter_map(move |x| {
                    let land = map.land(x, y)?;
                    let at = camera.to_screen(openshard_protocol::world::Point::new(x, y, land.z));
                    (at.x >= 0 && at.y >= 0 && at.x < ids.width() as i32 && at.y < ids.height() as i32)
                        .then_some((x, y, at.x as u32, at.y as u32))
                })
            })
        })
        .collect();
    let mapped = readback
        .slice(..)
        .get_mapped_range()
        .expect("completed LOD screen audit has mapped bytes");
    let mapped_position = position_readback
        .slice(..)
        .get_mapped_range()
        .expect("completed LOD screen position audit has mapped bytes");
    let mut composite = 0_u64;
    let mut missing = Vec::new();
    let mut misplaced_land = Vec::new();
    for (x, y, screen_x, screen_y) in &samples {
        let offset = *screen_y as usize * stride as usize + *screen_x as usize * 4;
        let id = u32::from_le_bytes(mapped[offset..offset + 4].try_into().expect("one ID texel"));
        if id & openshard_client_render::gbuffer::IDS_COMPOSITE_MAP != 0 {
            composite += 1;
        }
        if openshard_client_render::gbuffer::ids_kind(id)
            == Some(openshard_client_render::place::Kind::Nothing)
            && missing.len() < 12
        {
            missing.push((*x, *y));
        }
        if id & openshard_client_render::gbuffer::IDS_COMPOSITE_MAP != 0
            && openshard_client_render::gbuffer::ids_kind(id)
                == Some(openshard_client_render::place::Kind::Land)
        {
            let position_offset = *screen_y as usize * position_stride as usize + *screen_x as usize * 16;
            let actual_x = f32::from_le_bytes(
                mapped_position[position_offset..position_offset + 4]
                    .try_into()
                    .expect("cached land x position"),
            )
            .floor() as i32;
            let actual_y = f32::from_le_bytes(
                mapped_position[position_offset + 4..position_offset + 8]
                    .try_into()
                    .expect("cached land y position"),
            )
            .floor() as i32;
            if (actual_x != i32::from(*x) || actual_y != i32::from(*y)) && misplaced_land.len() < 12 {
                misplaced_land.push(((*x, *y), (actual_x, actual_y)));
            }
        }
    }
    drop(mapped);
    drop(mapped_position);
    readback.unmap();
    position_readback.unmap();
    // A ground diamond's visual centre can belong to either neighbouring
    // triangle, so this optional position sample is diagnostic context rather
    // than a coverage failure. `missing` alone is the LOD readiness invariant.
    if missing.is_empty() {
        tracing::info!(
            samples = samples.len(),
            composite,
            misplaced_land = ?misplaced_land,
            "LOD screen G-buffer covers every visible tile centre"
        );
    } else {
        tracing::error!(
            samples = samples.len(),
            composite,
            missing = ?missing,
            misplaced_land = ?misplaced_land,
            "LOD screen G-buffer has uncovered visible tile centres"
        );
    }
}

/// Render the immutable map portion once more, entirely at LOD0, and compare
/// it with the cached pixels already present in the real frame.  This is the
/// field oracle for a valid-but-wrong cached pixel: coverage checks cannot see
/// a sprite or depth winner borrowed from another block.
fn audit_lod_map_equivalence(
    window: &mut crate::window::Screen,
    camera: Camera,
    geometry: &crate::frame_geometry::FrameGeometry,
    pass: WorldPassAudit,
) -> String {
    let width = window.gbuffer.ids().width();
    let height = window.gbuffer.ids().height();
    let expected_world = blit::world_texture(&window.device, width, height);
    let expected_world_view = expected_world.create_view(&wgpu::TextureViewDescriptor::default());
    let expected_depth = openshard_client_render::renderer::depth_texture(&window.device, width, height);
    let expected_depth_view = expected_depth.create_view(&wgpu::TextureViewDescriptor::default());
    let expected_gbuffer = Gbuffer::new(&window.device, width, height);
    let expected_views = expected_gbuffer.views();
    let target = Target {
        view: &expected_world_view,
        depth: &expected_depth_view,
        gbuffer: &expected_views,
        width,
        height,
        projection: camera.projection(),
    };
    let mut encoder = window
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("LOD map equivalence oracle"),
        });
    // This is deliberately the complete immutable geometry, not
    // `detail_*`: the reference says what the same camera would draw with no
    // ready composite at all.
    // This pass is recorded after the real frame has been submitted.  It must
    // not reuse that frame's mutable instance buffer: a diagnostic must only
    // observe the scene, never enqueue a later upload into the stream the
    // already-submitted frame is reading. `composite_ground` has the same
    // shared atlas textures but an independent uniform/instance stream.
    window.composite_ground.render(
        &window.device,
        &window.queue,
        &mut encoder,
        target,
        &geometry.quads,
    );
    window.statics.render(
        &window.device,
        &window.queue,
        &mut encoder,
        target,
        &geometry.map_static_instances.rows,
        &geometry.mesh.boxes,
        Some(geometry.map_static_instances.drawn),
    );
    window.mesh_pass.render(
        &window.device,
        &window.queue,
        &mut encoder,
        target,
        &geometry.mesh.mesh_vertices,
        &geometry.mesh.mesh_rows,
    );
    window.queue.submit([encoder.finish()]);

    // Raw G-buffer equality proves capture/restore. Run the actual deferred
    // lighting route as well: cached IDs intentionally take a different
    // branch in `blit.wesl`, so this catches a valid raw pixel which becomes a
    // wrong visible pixel only after lighting and selection resolve it.
    let lit_actual = blit::world_texture(&window.device, width, height);
    let lit_actual_view = lit_actual.create_view(&wgpu::TextureViewDescriptor::default());
    let lit_expected = blit::world_texture(&window.device, width, height);
    let lit_expected_view = lit_expected.create_view(&wgpu::TextureViewDescriptor::default());
    let actual_views = window.gbuffer.views();
    let mut lit = Blit::new(&window.device, blit::WORLD_FORMAT);
    let raw_rect = ViewportRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    let actual_world_view = window.world.create_view(&wgpu::TextureViewDescriptor::default());
    let frame = |target, world, gbuffer| blit::Frame {
        target,
        world,
        gbuffer,
        face_instances: window.statics.instances_buffer(),
        item_instances: window.items_pass.instances_buffer(),
        mobile_instances: window.mobile_pass.instances_buffer(),
        mesh_instances: window.mesh_pass.rows_buffer(),
        ground_instances: window.renderer.instances_buffer(),
        zoom: camera.zoom(),
        rect: raw_rect,
    };
    let mut encoder = window
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("LOD map equivalence lighting oracle"),
        });
    lit.render(
        &window.device,
        &window.queue,
        &mut encoder,
        frame(&lit_actual_view, &actual_world_view, &actual_views),
        &geometry.lighting,
    );
    lit.render(
        &window.device,
        &window.queue,
        &mut encoder,
        frame(&lit_expected_view, &expected_world_view, &expected_views),
        &geometry.lighting,
    );
    window.queue.submit([encoder.finish()]);

    let Some(actual_color) = audit_texture_bytes(
        &window.device,
        &window.queue,
        &window.world,
        4,
        "LOD actual world equivalence readback",
    ) else {
        tracing::warn!("could not read actual world for LOD equivalence oracle");
        return "status=unavailable\nreason=actual-world-readback\n".to_owned();
    };
    let Some(expected_color) = audit_texture_bytes(
        &window.device,
        &window.queue,
        &expected_world,
        4,
        "LOD expected world equivalence readback",
    ) else {
        tracing::warn!("could not read expected world for LOD equivalence oracle");
        return "status=unavailable\nreason=expected-world-readback\n".to_owned();
    };
    let Some(actual_ids) = audit_texture_bytes(
        &window.device,
        &window.queue,
        window.gbuffer.ids(),
        4,
        "LOD actual IDs equivalence readback",
    ) else {
        tracing::warn!("could not read actual IDs for LOD equivalence oracle");
        return "status=unavailable\nreason=actual-ids-readback\n".to_owned();
    };
    let Some(expected_ids) = audit_texture_bytes(
        &window.device,
        &window.queue,
        expected_gbuffer.ids(),
        4,
        "LOD expected IDs equivalence readback",
    ) else {
        tracing::warn!("could not read expected IDs for LOD equivalence oracle");
        return "status=unavailable\nreason=expected-ids-readback\n".to_owned();
    };
    let Some(actual_position) = audit_texture_bytes(
        &window.device,
        &window.queue,
        window.gbuffer.position(),
        16,
        "LOD actual position equivalence readback",
    ) else {
        tracing::warn!("could not read actual position for LOD equivalence oracle");
        return "status=unavailable\nreason=actual-position-readback\n".to_owned();
    };
    let Some(expected_position) = audit_texture_bytes(
        &window.device,
        &window.queue,
        expected_gbuffer.position(),
        16,
        "LOD expected position equivalence readback",
    ) else {
        tracing::warn!("could not read expected position for LOD equivalence oracle");
        return "status=unavailable\nreason=expected-position-readback\n".to_owned();
    };
    let Some(lit_actual) = audit_texture_bytes(
        &window.device,
        &window.queue,
        &lit_actual,
        4,
        "LOD actual lit equivalence readback",
    ) else {
        tracing::warn!("could not read actual lit world for LOD equivalence oracle");
        return "status=unavailable\nreason=actual-lit-readback\n".to_owned();
    };
    let Some(lit_expected) = audit_texture_bytes(
        &window.device,
        &window.queue,
        &lit_expected,
        4,
        "LOD expected lit equivalence readback",
    ) else {
        tracing::warn!("could not read expected lit world for LOD equivalence oracle");
        return "status=unavailable\nreason=expected-lit-readback\n".to_owned();
    };

    let (
        mut compared,
        mut color_mismatches,
        mut lit_mismatches,
        mut identity_mismatches,
        mut position_mismatches,
    ) = (0_u64, 0_u64, 0_u64, 0_u64, 0_u64);
    let mut color_examples = Vec::new();
    let mut lit_examples = Vec::new();
    let mut position_examples = Vec::new();
    let mut rejected_blocks = BTreeSet::new();
    for pixel in 0..(width * height) as usize {
        let byte = pixel * 4;
        let actual_id = u32::from_le_bytes(actual_ids[byte..byte + 4].try_into().expect("actual ID"));
        let expected_id = u32::from_le_bytes(expected_ids[byte..byte + 4].try_into().expect("expected ID"));
        let expected_map = matches!(
            openshard_client_render::gbuffer::ids_kind(expected_id),
            Some(openshard_client_render::place::Kind::Land | openshard_client_render::place::Kind::Static)
        );
        // The direct reference deliberately contains immutable map geometry
        // only. A live server item or mobile may legitimately cover that map
        // pixel in the real frame, but an empty actual pixel cannot: it is the
        // exact black ground hole a composite seam would otherwise evade by
        // simply dropping its `IDS_COMPOSITE_MAP` bit.
        let actual_is_dynamic = openshard_client_render::gbuffer::ids_id(actual_id)
            & openshard_client_render::gbuffer::IDS_DYNAMIC_ITEM
            != 0
            || openshard_client_render::gbuffer::ids_kind(actual_id)
                == Some(openshard_client_render::place::Kind::Mobile);
        if !expected_map || actual_is_dynamic {
            continue;
        }
        let expected_source_tile = match openshard_client_render::gbuffer::ids_kind(expected_id) {
            Some(openshard_client_render::place::Kind::Land) => geometry
                .quads
                .get(openshard_client_render::gbuffer::ids_id(expected_id) as usize)
                .map(|quad| (quad.place.x, quad.place.y)),
            _ => None,
        };
        let expected_source_is_flat = match openshard_client_render::gbuffer::ids_kind(expected_id) {
            Some(openshard_client_render::place::Kind::Land) => geometry
                .quads
                .get(openshard_client_render::gbuffer::ids_id(expected_id) as usize)
                .map(|quad| quad.is_flat()),
            _ => None,
        };
        let expected_source_block = expected_source_tile.map(|(x, y)| BlockCoord::containing(x, y));
        // Do not make a person wait for a second report to get their scene
        // back. A direct-map land pixel replaced by `Nothing` is a definite
        // cache coverage failure, not a benign ordering difference. Quarantine
        // just that source block, so the next frame returns to the known-good
        // LOD0 ground path while unaffected blocks keep their cache benefit.
        if openshard_client_render::gbuffer::ids_kind(actual_id)
            == Some(openshard_client_render::place::Kind::Nothing)
        {
            if let Some(block) = expected_source_block {
                rejected_blocks.insert(block);
            }
        }
        compared += 1;
        let same_identity = openshard_client_render::gbuffer::ids_kind(actual_id)
            == openshard_client_render::gbuffer::ids_kind(expected_id)
            && openshard_client_render::gbuffer::ids_stance(actual_id)
                == openshard_client_render::gbuffer::ids_stance(expected_id);
        let same_color = actual_color[byte..byte + 4] == expected_color[byte..byte + 4];
        let same_lit = lit_actual[byte..byte + 4] == lit_expected[byte..byte + 4];
        let position_byte = pixel * 16;
        let read_point = |bytes: &[u8]| {
            [0, 4, 8, 12].map(|offset| {
                f32::from_le_bytes(
                    bytes[position_byte + offset..position_byte + offset + 4]
                        .try_into()
                        .expect("position component"),
                )
            })
        };
        let actual_point = read_point(&actual_position);
        let expected_point = read_point(&expected_position);
        // The independent producer and camera frame use equivalent triangle
        // interpolation in a different draw arrangement. A few ULPs are not
        // a changed world point; report only a material coordinate change.
        let same_position = actual_point
            .iter()
            .zip(expected_point)
            .all(|(actual, expected)| (actual - expected).abs() <= 1e-4);
        identity_mismatches += u64::from(!same_identity);
        color_mismatches += u64::from(!same_color);
        lit_mismatches += u64::from(!same_lit);
        position_mismatches += u64::from(!same_position);
        if !same_color && color_examples.len() < 12 {
            color_examples.push((
                (pixel as u32 % width, pixel as u32 / width),
                [
                    actual_color[byte],
                    actual_color[byte + 1],
                    actual_color[byte + 2],
                    actual_color[byte + 3],
                ],
                [
                    expected_color[byte],
                    expected_color[byte + 1],
                    expected_color[byte + 2],
                    expected_color[byte + 3],
                ],
                actual_id,
                expected_id,
                actual_point,
                expected_point,
                expected_source_tile,
                expected_source_is_flat,
                expected_source_block,
            ));
        }
        if !same_lit && lit_examples.len() < 12 {
            lit_examples.push((
                (pixel as u32 % width, pixel as u32 / width),
                [
                    lit_actual[byte],
                    lit_actual[byte + 1],
                    lit_actual[byte + 2],
                    lit_actual[byte + 3],
                ],
                [
                    lit_expected[byte],
                    lit_expected[byte + 1],
                    lit_expected[byte + 2],
                    lit_expected[byte + 3],
                ],
                actual_id,
                expected_id,
            ));
        }
        if !same_position && position_examples.len() < 12 {
            position_examples.push((
                (pixel as u32 % width, pixel as u32 / width),
                actual_point,
                expected_point,
                actual_id,
                expected_id,
            ));
        }
    }
    let rejected: Vec<_> = rejected_blocks.iter().copied().collect();
    for block in &rejected {
        // This is the exact cache identity that the world pass was allowed to
        // restore. Keeping its ground proof turns a field report into a
        // reproducible producer owner, rather than merely a screen position.
        let key = CompositeKey {
            block: *block,
            tier: CompositeTier::from_lod(pass.requested_lod).unwrap_or(CompositeTier::Lod1),
            revision: pass.composite_revision,
        };
        let ground = window.composites.get(key).map(|texture| texture.ground());
        window.composites.reject_block(
            key,
            ground,
            CompositeQuarantineReason::OracleMissingGroundCoverage,
        );
    }
    if !rejected.is_empty() {
        tracing::error!(
            ?rejected,
            "LOD oracle quarantined cache blocks with missing ground coverage"
        );
    }
    let quarantine_count = window.composites.quarantined_len();
    let latest_quarantine = window.composites.latest_quarantine();
    let lod_state = lod_diagnostic_state(pass, quarantine_count);
    if color_mismatches == 0 && lit_mismatches == 0 && identity_mismatches == 0 && position_mismatches == 0 {
        tracing::info!(compared, "LOD map equivalence oracle matches full LOD0 scene");
        format!(
            "status=match\nlod_state={lod_state}\nsize={width}x{height}\nworld_pass_requested_lod={:?}\nworld_pass_composite_revision={:?}\nworld_pass_ready_blocks={}\nworld_pass_live_ground_quads={}\nworld_pass_full_ground_quads={}\ncompared={compared}\ncolor_mismatches=0\nlit_mismatches=0\nidentity_mismatches=0\nposition_mismatches=0\nquarantine_count={quarantine_count}\nlatest_quarantine={latest_quarantine:#?}\nrejected_blocks={rejected:#?}\n",
            pass.requested_lod,
            pass.composite_revision,
            pass.ready_blocks,
            pass.live_ground_quads,
            pass.full_ground_quads,
        )
    } else {
        tracing::error!(
            compared,
            color_mismatches,
            lit_mismatches,
            identity_mismatches,
            position_mismatches,
            color_examples = ?color_examples,
            lit_examples = ?lit_examples,
            position_examples = ?position_examples,
            "rendered immutable map differs from full LOD0 scene"
        );
        format!(
            "status=mismatch\nlod_state={lod_state}\nsize={width}x{height}\nworld_pass_requested_lod={:?}\nworld_pass_composite_revision={:?}\nworld_pass_ready_blocks={}\nworld_pass_live_ground_quads={}\nworld_pass_full_ground_quads={}\ncompared={compared}\ncolor_mismatches={color_mismatches}\nlit_mismatches={lit_mismatches}\nidentity_mismatches={identity_mismatches}\nposition_mismatches={position_mismatches}\nquarantine_count={quarantine_count}\nlatest_quarantine={latest_quarantine:#?}\nrejected_blocks={rejected:#?}\ncolor_examples={color_examples:#?}\nlit_examples={lit_examples:#?}\nposition_examples={position_examples:#?}\n",
            pass.requested_lod,
            pass.composite_revision,
            pass.ready_blocks,
            pass.live_ground_quads,
            pass.full_ground_quads,
        )
    }
}

/// The effective LOD condition of the exact world pass captured by F12.
///
/// It deliberately names safe direct rendering separately from a compositor
/// mismatch: a cache miss or quarantine is expected to retain LOD0 ground,
/// whereas `status=mismatch` above is evidence that the produced scene differs.
fn lod_diagnostic_state(pass: WorldPassAudit, quarantined: usize) -> &'static str {
    match pass.requested_lod {
        BlockLod::Lod0 => "lod-disabled",
        _ if quarantined > 0 && pass.ready_blocks == 0 => "quarantine-safe-lod0",
        _ if pass.ready_blocks == 0 => "lod-not-ready-safe-lod0",
        _ if pass.live_ground_quads < pass.full_ground_quads => "cached-with-lod0-fallback",
        _ => "cached",
    }
}

/// LOD2 stays held back while the repaired LOD1 restore path is checked against
/// live server/NPC traffic. LOD1 keeps every producer texel lossless.
const fn visible_composite_lod(selected: BlockLod) -> BlockLod {
    match selected {
        BlockLod::Lod2 => BlockLod::Lod1,
        lod => lod,
    }
}

/// The immutable boundary between advancing the client and presenting one
/// frame. It contains the one camera and the read-only facts every pass,
/// overlay and next-frame click must agree on.
struct PreparedFrame {
    started: Instant,
    camera: Camera,
    facts: FrameFacts,
}

impl App {
    /// Everyone to draw, each beside the serial their clock is keyed by.
    ///
    /// Our own body first, and `None` for it while no shard has named us.
    ///
    /// The group is refreshed from the crowd here and not in
    /// [`App::advance_to_clocks`] alone, because this list is what *packs* the
    /// atlas as well as what draws from it — see [`App::wanted_in`]. `self.world.presentation.player`
    /// and `self.world.presentation.others` hold the group as of the last packet, and
    /// [`Crowd::advance`] changes it without one: a body that walked into view
    /// and then stopped is drawn standing while the packet-time list still says
    /// walking. Pack one group and draw another and [`mobiles::place`] finds no
    /// frame, so the body simply vanishes — and stays vanished for as long as it
    /// stands still, there being no further packet to correct the list with.
    pub(crate) fn drawn_mobiles(&self) -> Vec<(Who, Mobile)> {
        Self::everyone_drawn(
            &self.world.presentation.crowd,
            self.world.me(),
            &self.world.presentation.player,
            &self.world.presentation.others,
            &self.world.presentation.corpses,
        )
    }

    /// [`App::drawn_mobiles`] over the four fields it reads, so a test can build
    /// the list the atlases are grown from without a window, a device or a
    /// shard. The snapshot clones each mobile's cheap immutable equipment
    /// handle; its time-varying fields are then advanced below.
    pub(crate) fn everyone_drawn(
        crowd: &Crowd,
        me: Who,
        player: &Mobile,
        others: &[(Who, Mobile)],
        corpses: &[(Who, Mobile)],
    ) -> Vec<(Who, Mobile)> {
        let mut mobiles = Vec::with_capacity(others.len() + corpses.len() + 1);
        mobiles.push((me, player.clone()));
        mobiles.extend_from_slice(others);
        mobiles.extend_from_slice(corpses);
        Self::advance_groups(crowd, &mut mobiles);
        mobiles
    }

    /// Refresh each body's animation group from the crowd's clock.
    ///
    /// Split out of [`App::advance_to_clocks`] because the group is the one part
    /// of a mobile that has to be right *before* the atlases are grown, and the
    /// growth happens with no atlas to ask for a frame count. Both paths go
    /// through here so there is one statement of "which group is playing".
    pub(crate) fn advance_groups(crowd: &Crowd, drawn: &mut [(Who, Mobile)]) {
        for (who, mobile) in drawn.iter_mut() {
            // `Crowd::advance` drops a walking body to standing on its own
            // timer, with nothing that looks like a packet to refresh
            // `mobile.group` from — a group read once and left stale plays the
            // walking sprite for ever, timed by a clock that has moved on to
            // the standing group's.
            if let Some(group) = crowd.group_for(*who) {
                mobile.group = group;
            }
        }
    }

    /// Fill in the time-varying presentation state. `Crowd` provides animation
    /// groups and frames for everyone; the local player's pose and sorting
    /// source come exclusively from [`PlayerMotion`].
    ///
    /// An associated function taking the two fields it reads rather than a
    /// method, because both callers hold a borrow of one of `App`'s fields
    /// while they ask: the frame holds `self.window` mutably, and the pick
    /// holds it shared. A `&self` method would borrow all of `App` and neither
    /// could call it.
    ///
    /// `atlas` is asked for the frame *count*: a group's length is the
    /// animation's, and taking it from anywhere else makes "frame 7 of a
    /// 6-frame walk" expressible. Under the body the atlas packed — for a ghost
    /// the living body it borrows its pictures from — or a ghost counts zero
    /// frames, lands on frame 0 for ever and slides along standing still.
    pub(crate) fn advance_to_clocks(
        crowd: &Crowd,
        atlas: &AnimAtlas,
        me: Who,
        motion: &PlayerMotion,
        drawn: &mut [(Who, Mobile)],
    ) {
        // The group is read back first and not only the frame and the glide —
        // the frame count below is asked *under* it. Idempotent when the caller
        // is [`App::drawn_mobiles`], which is every caller today; here so this
        // function is right on its own terms rather than on its callers'.
        Self::advance_groups(crowd, drawn);
        for (who, mobile) in drawn.iter_mut() {
            let (direction, _) = openshard_uofiles::anim::facing(mobile.facing);
            let frame_count = atlas.frame_count(AnimationKey::new(
                openshard_uofiles::anim::animation_body(mobile.body),
                mobile.group,
                direction,
            ));
            mobile.frame = openshard_uofiles::anim::AnimationFrameIndex(crowd.frame_for(*who, frame_count));
            if *who == me {
                // This is the boundary that used to reintroduce the bug: the
                // frame builder overwrote the local `GameMotion` pose from
                // `Crowd`, so a stuck crowd made a moving HUD look detached
                // from a stationary body.
                Self::project_local_motion(motion, mobile);
            } else {
                if let Some(at) = crowd.drawn_for(*who) {
                    mobile.drawn = at;
                }
                // A remote mobile has no local movement core. Its crowd entry
                // remains the presentation source for sort order.
                mobile.from = crowd.stepping_from(*who);
            }
        }
        if let Some((_, player)) = drawn.iter().find(|(who, _)| *who == me) {
            debug_assert_eq!(player.drawn, motion.drawn());
            debug_assert_eq!(player.from, motion.transition_from());
        }
    }

    /// Apply the only two movement-owned fields of the local render mobile.
    /// Kept separate so this boundary is testable without a window or atlas.
    fn project_local_motion(motion: &PlayerMotion, mobile: &mut Mobile) {
        mobile.drawn = motion.drawn();
        mobile.from = motion.transition_from();
    }

    /// Everyone as they are drawn *this instant*, clocks and all — the list
    /// [`mobiles::pick`] and [`mobiles::collect`] both index into.
    ///
    /// Built twice a frame, once for the pick and once for the picture, rather
    /// than threaded between them: the two happen either side of the atlas
    /// growth and of a mutable borrow of the window, and the work is a handful
    /// of map lookups over whoever is on screen. What matters is that the
    /// *order* is [`App::drawn_mobiles`]'s both times, so an index answered by
    /// the pick still names the same creature to the passes below.
    pub(crate) fn drawn_now(&self, atlas: &AnimAtlas) -> Vec<(Who, Mobile)> {
        let mut drawn = self.drawn_mobiles();
        Self::advance_to_clocks(
            &self.world.presentation.crowd,
            atlas,
            self.world.me(),
            &self.world.motion,
            &mut drawn,
        );
        drawn
    }

    pub(crate) fn draw(&mut self) {
        // The assets and GPU must exist for the login conversation, but a shard
        // has not yet named a world while that conversation is under way. Keep
        // the surface untouched until its first complete view: the startup
        // placeholder at `START` is for offline inspection, not a temporary
        // online character position.
        if !self.world.render_ready {
            return;
        }
        // Movement is advanced before the HUD is assembled below. Clear the
        // frame-local plan here so both consumers share at most one search.
        self.steer.begin_frame();
        let started = Instant::now();
        // The frame boundary the flamegraph is cut on, put at the same place
        // `started` is sampled so that a frame in `puffin_viewer` and a frame in
        // the `frames` panel are the same span of time. Free when nobody is
        // recording — see [`profile`].
        profile::frame();
        puffin::profile_scope!("draw");
        // What the shard has opened, and what it has taken away: the view is
        // filled by `client/net`, which knows nothing about screens, so a
        // window appearing is this end noticing.
        self.sync_own_windows();
        // # The frame is three steps, and this is the first of them
        //
        // Everything that writes runs in `Self::advance`, before anything
        // reads — see that method's own doc for why the clock and the eye
        // move there and not here. After it returns, nothing in the frame
        // moves the world or the camera again; the snapshot below is what
        // every reader from here on is handed.
        self.advance(started);
        let camera = *self.control.camera();
        self.draw_from(started, camera);
    }

    /// **Step one of three**: everything that writes. What the shell asked
    /// for last frame, then every clock, then the eye.
    ///
    /// The animation clock moves here, at the top of the frame that is about
    /// to show its answer — not when the timer that asked for this frame
    /// fired.
    ///
    /// A glide is a position read off a clock, so the moment that clock is
    /// read has to be the moment the picture is built or the walk judders:
    /// the timer fires, the loop then lays out the UI, grows an atlas and
    /// waits on the swapchain, and however long that took is error in the
    /// body's position — error that varies frame to frame, which is exactly
    /// what an eye reads as a stutter. It also puts the sampling back in step
    /// with the display: `WaitUntil` is a floor, the timer's 16ms beats
    /// against a 60Hz refresh, and a frame drawn from the previous tick's
    /// clock lands on the wrong side of that beat every second or so.
    ///
    /// Whatever really passed — see `App::last_advance`. A stall longer than
    /// a frame, the window minimised or the machine asleep, moves the clock
    /// the whole way rather than queuing a burst of catch-up frames for time
    /// nobody watched: a body that was walking through it has long since
    /// arrived.
    ///
    /// The defect this staging is written against: the HUD used to be built
    /// at the top of the frame and the eye moved a few lines further down, so
    /// the overlay egui laid out — the tile highlight, the hover, the walk
    /// goal — was drawn against the *previous* frame's camera while the world
    /// pass below drew from this one's. The gap between them is one frame of
    /// camera motion, which is not a constant: it is whatever the display
    /// gave this frame, so the markers shivered against the ground they were
    /// meant to be lying on, and every missed interval made them jump.
    /// Reordering two calls would have fixed today's version of it and left
    /// the shape that produced it, which is a second reader picking the
    /// camera up at a different moment. So the frame is staged instead.
    pub(crate) fn advance(&mut self, started: Instant) {
        let elapsed = started.saturating_duration_since(self.last_advance);
        let asked = std::mem::take(&mut self.pending);
        self.apply(asked);
        // The viewport the last frame's layout left free — `Shell` holds it
        // between frames for exactly this. It has to be settled before the eye
        // is, because it is what decides how much world a camera can see.
        if let Some(shell) = self.shell.as_ref() {
            let viewport = shell.viewport();
            self.control.resize(viewport.width, viewport.height);
        }
        advance_presentation_to(
            &mut self.world.presentation,
            &mut self.world.motion,
            &mut self.last_advance,
            started,
        );
        // A track that has reached its end and is meant to loop starts again
        // here. The mixer runs on its own thread but owns no clock of its own,
        // and this is the frame's step for everything that moves without being
        // asked to.
        self.audio.advance();
        self.project_player_motion();
        // Whatever scenario is being walked delivers its knots for the span that
        // just passed, before the eye is asked where the body is: a step that
        // arrived this frame is one the camera has to answer this frame.
        let prediction_before_replay = self.world.motion.planning_state();
        self.advance_replay(elapsed);
        self.advance_lod_sweep(elapsed);
        if self.world.motion.planning_state() != prediction_before_replay {
            if let Some(trace) = self.movement_trace.as_mut() {
                trace.record(
                    "frame_replay_changed_prediction",
                    &self.world,
                    self.control.camera(),
                );
            }
        }
        // A viewport that grew may have taken the world texture past what the
        // device allows, which no zoom step asked for.
        self.fit_zoom_to_device();
        // And the eye goes where the body is *this frame*: a step arrives once
        // and is then walked across for the next 400ms, so every frame in
        // between has a different answer.
        self.follow_player(elapsed);
        self.report_stationary_soak_server_updates();
        if let Some(sweep) = self
            .lod_sweep
            .as_mut()
            .filter(|sweep| sweep.stationary_soak && sweep.stationary_zoomed)
        {
            let camera = *self.control.camera();
            if let Some(previous) = sweep.stationary_camera.replace(camera) {
                if previous != camera {
                    tracing::error!(
                        previous = ?previous,
                        current = ?camera,
                        "stationary LOD soak camera changed after its injected zoom"
                    );
                }
            } else {
                tracing::info!(?camera, "stationary LOD soak camera locked after injected zoom");
            }
        }
        if let Some(trace) = self.movement_trace.as_mut() {
            trace.record("frame", &self.world, self.control.camera());
        }
    }

    /// **Step two**: one snapshot, and it is a value.
    ///
    /// Every question this frame's picture and HUD are built from, asked
    /// once against one camera and one cutaway and answered as a plain
    /// value — purely a function of `&self`, so a caller cannot mistake this
    /// for a place the frame's state changes. It has none: `on_static`,
    /// `on_mobile` and `on_item` still have to land in `self.picking` for the
    /// click to read next frame, but that write happens in the three lines at
    /// `draw_from`'s call site instead, which is the "mutations applied
    /// separately" half of the shape `Self::advance` set up for the first
    /// step.
    pub(crate) fn frame_facts(&self, camera: Camera) -> FrameFacts {
        // Read before the window is borrowed below, for the same reason the
        // pacing at the foot of the frame is a fact about the whole app
        // rather than about it.
        let watched = self.watched();
        // The same, for the two the item highlight needs — both are questions
        // about the whole of `self` and are asked once, here.
        let owns_pointer = self.world_owns_pointer();
        let cursor = self.control.cursor();

        // What this frame does not draw, read once from the tile the player is
        // standing on. Once, and from the *player's* tile rather than the
        // camera's: a free camera looking at a rooftop three streets away has
        // not walked indoors, and the client's rule is about where the body is.
        // See `openshard_client_render::cutaway`.
        //
        // `self.world.presentation.cutaway_at`, not `self.world.presentation.player.at`: the latter is this end's
        // own unconfirmed prediction, which for one frame can be a tile a
        // held direction was refused on — see the field's own doc.
        //
        // Here, in the snapshot, and not beside the passes that draw from it:
        // the item pick below needs it, and the pick has to be answered before
        // the HUD is built — see the next paragraph.
        let cutaway = self.cutaway();
        let interior = self.interior_frame();
        // The ground tile under the cursor, and its ring — asked here beside
        // the picks below rather than a second time when the HUD is built:
        // this used to be `App::hud`'s own call to `Self::pick_tile`, a second
        // "what is the cursor over" answered from a *different* camera in
        // spirit even when it happened to be the same value in practice. One
        // frame's worth of picks belongs in one place — this function — same
        // as `on_mobile`/`on_item`/`on_static` below.
        let hover = owns_pointer
            .then(|| self.pick_tile(camera))
            .flatten()
            .filter(|tile| {
                interior.as_ref().is_none_or(|frame| {
                    let z = self
                        .resources
                        .map
                        .map()
                        .land(tile.at.x, tile.at.y)
                        .map_or(0, |land| land.z);
                    frame.shows_at(Point::new(tile.at.x, tile.at.y, z))
                })
            });
        let neighbours = hover.as_ref().map_or_else(Vec::new, |tile| self.tile_ring(tile));
        // What the cursor is over, asked here rather than remembered from the
        // last click: the picture moves under a still mouse — the body walks,
        // the camera follows, a door swings — so where the cursor is pointing is
        // a question about *this* frame's picture and has to be asked against
        // this frame's camera. The same `items::pick` a double-click asks, so
        // what is lit is what would be used.
        //
        // Asked once and answered to three readers: the hue the picture is drawn
        // in, the silhouette the ring is grown from, and whether the HUD marks
        // the tile under the cursor at all. Two picks would be two chances to
        // disagree about what the cursor is on, and the visible form of that
        // disagreement is a barrel ringed with the ground under it diamonded.
        //
        // Against the atlas as it stands *before* this frame grows it, which is
        // the one thing given up by asking this early. An item that came on
        // screen this very frame has no sprite packed yet and so no rectangle to
        // be pointed at, and is pickable a frame later; the alternative was a
        // tile marker that decides whether to draw itself from the previous
        // frame's answer, which flickers along every item's edge.
        // **The picks are the frame's *facts*, and the mode decides only what is
        // drawn from them.** They used to be skipped under
        // `HighlightTarget::Tiles`, which folded two questions into one field:
        // "what is the cursor on" and "what may light up". A click reads the
        // first — see the `MouseInput` arm — so with the two folded together a
        // player who had pinned the highlight to tiles could not select a wall at
        // all, and the reason was invisible. The mode is applied to `lit_*`
        // below instead, where it is about lighting and nothing else.
        //
        // Creatures are asked first and they win: a mobile stands *on* the
        // clutter of its tile — it is sorted above whatever is lying there, and
        // it is what a player pointing at a shopkeeper standing on a rug means.
        // Then the server's items, then the map's own furniture. One chain, and
        // every later question is asked only where the earlier ones found
        // nothing — so "what is under the cursor" has exactly one answer and the
        // ring, the wash, the tile marker and the click cannot disagree about it.
        // Kept whole, and not just picked from: the click reads a mobile back by
        // [`Who`] rather than by this index, which is only ever good for this
        // one frame's own `Vec` — see `FrameFacts::on_mobile`.
        let drawn_mobiles = self
            .window
            .as_ref()
            .map(|window| self.drawn_now(&window.atlases.mobiles));
        let on_mobile = match (owns_pointer, self.window.as_ref(), &drawn_mobiles) {
            (true, Some(window), Some(drawn)) => mobiles::pick_iter_with_interior(
                drawn.iter().map(|(_, mobile)| mobile),
                &camera,
                &window.atlases.mobiles,
                &cutaway,
                &self.resources.equip_conv,
                cursor,
                interior.as_ref(),
            ),
            _ => None,
        };
        let on_item = match owns_pointer && on_mobile.is_none() {
            true => self.window.as_ref().and_then(|window| {
                items::pick_with_interior(
                    &self.world.presentation.items,
                    &camera,
                    &self.resources.tiledata,
                    &self.world.presentation.tile_animations,
                    &window.atlases.statics,
                    &cutaway,
                    cursor,
                    interior.as_ref(),
                )
            }),
            false => None,
        };
        // And the map's own furniture last, which is the one a wall is: it has no
        // serial and cannot be used, so it loses to anything that can. Asked
        // every frame rather than at the click, because it is what the *tile
        // marker* has to know — a wall under the cursor takes the highlight, and
        // the diamond drawn on the ground behind it was the client answering the
        // same question twice with two different tiles.
        //
        // This is the one pick that walks the map: `statics::pick` covers the
        // cells `statics::collect` is about to draw. It is a second walk of them
        // per frame with the pointer over the world, and the placement it does
        // per static is the collector's own — see the Frames tab if it ever
        // shows.
        let on_static = match owns_pointer && on_mobile.is_none() && on_item.is_none() {
            true => self.window.as_ref().and_then(|window| {
                statics::pick_with_interior(
                    self.resources.map.map(),
                    &camera,
                    &self.resources.tiledata,
                    &self.world.presentation.tile_animations,
                    &window.atlases.statics,
                    &cutaway,
                    cursor,
                    interior.as_ref(),
                )
            }),
            false => None,
        };
        // What the mode allows to light up. `Tiles` lights neither, which is the
        // whole of that setting; the facts above are unchanged by it.
        let lit_mobile = on_mobile.filter(|_| self.graphics.highlight != HighlightTarget::Tiles);
        let lit_item = on_item.filter(|_| self.graphics.highlight != HighlightTarget::Tiles);

        // The server-confirmed combat target owns the persistent mobile ring.
        // It takes precedence over a local click selection: selection may move
        // to a tile or an item while combat continues, but the target marker
        // must stay on the body the shard says we are fighting.
        let targeted_mobile = self
            .world
            .authoritative
            .view
            .as_ref()
            .and_then(|view| view.player.attacking)
            .filter(|_| self.graphics.drawing.mobiles)
            .and_then(|who| {
                drawn_mobiles.as_ref().and_then(|drawn| {
                    drawn
                        .iter()
                        .position(|(candidate, _)| *candidate == Some(who))
                        .map(openshard_client_render::mobiles::MobileIndex::new)
                })
            });
        // What a click is *holding*, turned from identity back into this
        // frame's index — the reverse of `on_mobile`/`on_item` just above.
        // This is the held ring's own pick, asked once here rather than at
        // every reader, for the reason `lit_item`'s doc gives for asking
        // `on_item` once: two lookups are two chances to disagree about which
        // creature a `Who` still names.
        //
        // Valid only while the crowd is actually drawn: `drawn` below is
        // emptied whole when `self.graphics.drawing.mobiles` is off, and an index into
        // `drawn_mobiles` would then point at a `Vec` the held ring never
        // sees.
        let selected_mobile = self
            .picking
            .selected
            .and_then(SelectedIdentity::as_mobile)
            .filter(|_| self.graphics.drawing.mobiles)
            .and_then(|who| {
                drawn_mobiles.as_ref().and_then(|drawn| {
                    drawn
                        .iter()
                        .position(|(candidate, _)| *candidate == who)
                        .map(openshard_client_render::mobiles::MobileIndex::new)
                })
            });
        let held_mobile = targeted_mobile.or(selected_mobile);
        let selected_item = self
            .picking
            .selected
            .and_then(SelectedIdentity::as_item)
            .and_then(|serial| {
                self.world
                    .presentation
                    .item_serials
                    .iter()
                    .position(|candidate| *candidate == serial)
                    .map(openshard_client_render::items::ItemIndex::new)
            });
        FrameFacts {
            watched,
            cutaway,
            interior,
            pick: Pick {
                tile: hover,
                neighbours,
                static_: on_static,
                mobile: lit_mobile,
                item: lit_item,
            },
            drawn_mobiles,
            on_mobile,
            on_item,
            held_mobile,
            selected_item,
        }
    }

    /// Freeze the values presentation may read this frame. Writers publish
    /// their small, deliberate aftermath through [`Self::publish_frame_picks`]
    /// before any pass is encoded; no pass reaches back into live input or a
    /// newly moved camera.
    fn prepare_frame(&self, started: Instant, camera: Camera) -> PreparedFrame {
        PreparedFrame {
            started,
            camera,
            facts: self.frame_facts(camera),
        }
    }

    /// Publish the identities the next input event must read from a prepared
    /// frame. The facts remain otherwise immutable: this is the only bridge
    /// from the current picture to next-frame click handling.
    fn publish_frame_picks(&mut self, facts: &FrameFacts) {
        self.picking.hover.static_ = facts.pick.static_;
        self.picking.hover.mobile = facts.on_mobile.and_then(|index| {
            facts
                .drawn_mobiles
                .as_ref()
                .and_then(|drawn| drawn.get(index.position()))
                .map(|(who, _)| *who)
        });
        self.picking.hover.item = facts
            .on_item
            .map(|index| self.world.presentation.item_serials[index.position()]);
    }

    /// **Steps two and three**: the frame `Self::advance` staged for. Takes
    /// the camera as a parameter rather than reading `self.control` again —
    /// a `&Camera` handed to five collectors is five reads of a field that
    /// something between them might have moved, which is the defect
    /// `Self::advance`'s doc is written against, expressed as a borrow. A
    /// `Camera` is `Copy`, so the one read in `draw` costs nothing and cannot
    /// be stale in one place and fresh in another.
    pub(crate) fn draw_from(&mut self, started: Instant, camera: Camera) {
        // # Step two: one snapshot, and it is a value
        //
        // `Self::frame_facts` asks every question this frame's picture and HUD
        // are built from, purely against `&self` — and answers three of them,
        // `on_static`/`on_mobile`/`on_item`, into `self.picking` right here,
        // which is the "mutations applied separately" half of the shape
        // `Self::advance` set up for the first: a function that only *asks*
        // stays a function that only asks, and the one write this frame still
        // owes `self.picking` is named where it happens rather than folded into
        // the asking.
        let facts_started = Instant::now();
        let prepared = self.prepare_frame(started, camera);
        let facts_cost = facts_started.elapsed();
        self.publish_frame_picks(&prepared.facts);
        let PreparedFrame {
            started,
            camera,
            facts,
        } = prepared;
        let FrameFacts {
            watched,
            cutaway,
            interior,
            pick,
            drawn_mobiles,
            on_mobile: _,
            on_item: _,
            held_mobile,
            selected_item,
        } = facts;

        // # Step three: present. Nothing below this line writes the world.
        //
        // The UI first, because it is what the surface is composited from
        // bottom-up and because its layout is what next frame's viewport comes
        // from. Its request is *held* rather than applied — see [`App::pending`].
        //
        // Timed, and separately from the world below: the two halves of a frame
        // are built by two things that grow for different reasons, and a single
        // build time cannot say which of them ate the frame. See [`frames`].
        //
        // The `Instant`s from here down are instrumentation and not a clock the
        // picture depends on: they measure what this frame cost, and no position
        // in it is a function of them. The one sampling of time that the frame is
        // built from is `started`, at the top.
        let ui_started = Instant::now();
        let (hud, hud_timings) = self.hud(camera, &pick, &cutaway, drawn_mobiles.as_deref());
        let ui_hud_cost = ui_started.elapsed();
        let painting = self.window.as_ref().map(|screen| Arc::clone(&screen.window));
        let ui_layout_started = Instant::now();
        let ui = match (self.shell.as_mut(), painting.as_ref()) {
            (Some(shell), Some(window)) => {
                let (request, output) = shell.run(window, &hud, camera, &self.world);
                let viewport = shell.viewport();
                Some((request, output, viewport))
            }
            _ => None,
        };
        let ui_layout_cost = ui_layout_started.elapsed();
        let mut ui_cost = ui_hud_cost + ui_layout_cost;
        if let Some((request, _, _)) = &ui {
            self.pending = request.clone();
        }

        // The Light tab's own numbers, which live in the shell — read here,
        // once for the whole frame: the flames, the ambient and the sun below
        // are all turned by them, and so is `want` just below, since the
        // atlases have to be grown for the same bound `light::collect` reads
        // them over.
        let tuning = self.tuning();
        // The producer needs the same static-impostor mode as the camera
        // frame. Lighting itself is applied later from the restored G-buffer,
        // but a lit frame's map statics still need their real box intersection
        // instead of the daylight billboard fallback.
        // The Chat tab's own numbers, the same reason and the same place:
        // gathered before the window is borrowed below, since `App::chat_style`
        // also reads the whole of `self`.
        let chat_style = self.chat_style();
        // The player's own sizes, read before the window is borrowed for the
        // reason the line above is: they come from the live HUD when it exists,
        // and the window is part of `self`.
        let fonts = self.font_sizes();
        // What the camera has walked onto since the atlases were last grown.
        // Gathered before the window is borrowed, and not inside the borrow: it
        // reads the whole of `self`, and the window is part of it.
        let want = light::lit_tiles(&camera, &tuning);
        let wanted = self.wanted_since(camera, &tuning, self.graphics.covered);
        // Only schedule immutable map-block work here.  `refresh` merely
        // reprioritises bounded requests; it does not build or upload pixels,
        // so a newly exposed far-zoom block continues through the detailed
        // representation until an idle producer has completed its composite.
        // The completed image enters `Screen::composites` through this queue;
        // Work 4 owns drawing that ready texture in the depth-aware world pass.
        let map_width = self.resources.map.map().width();
        let map_height = self.resources.map.map().height();
        let map_tiles = openshard_client_render::camera::TileBounds {
            min_x: 0,
            max_x: map_width.saturating_sub(1) as i32,
            min_y: 0,
            max_y: map_height.saturating_sub(1) as i32,
        };
        let composite_visible = MapBlockBounds::from_tiles(camera.visible_tiles(), map_width, map_height);
        // Producer coverage is proven through the real-map capture/restore
        // oracle in `tests/frame.rs`. Roll out the first cache tier only: a
        // far enough camera may request LOD1, while the selector continues to
        // retain its LOD2 hysteresis state for that tier's later validation.
        let selected_composite_lod = self.composite_lod.update_camera(&camera);
        // A cached composite is final immutable map pixels. Until it carries an
        // interior-frame fingerprint it cannot stand in for a room that has
        // deliberately become transparent, so the active building picture
        // keeps this frame on its detailed LOD0 path.
        let composite_lod = interior
            .is_some()
            .then_some(openshard_client_render::lod::BlockLod::Lod0)
            .unwrap_or_else(|| visible_composite_lod(selected_composite_lod));
        // A composite stores final map pixels and deferred facts, not atlas
        // UVs. Static-atlas pages are append-only, so packing art for a newly
        // entered block cannot alter a completed block composite. In
        // particular, do not key this cache to the atlas's growth revision:
        // at far zoom each scroll would otherwise discard the whole visible
        // LOD working set merely because one new sprite was packed.
        let composite_revision = ImmutableRevision(self.graphics.fringe as u64);
        if let (Some(visible), Some(map)) = (
            composite_visible,
            MapBlockBounds::from_tiles(map_tiles, map_width, map_height),
        ) {
            let composites = self.window.as_ref().map(|window| &window.composites);
            self.composite_work
                .refresh(visible, map, composite_lod, composite_revision, |key| {
                    composites.is_some_and(|cache| cache.is_rejected(key.block) || cache.get(key).is_some())
                });
        }
        let mut drawn = self.drawn_mobiles();
        // Likewise: the cut the solids view is drawn under reads the player, and
        // the pass that uses it runs inside the window's borrow.
        let solid_cut = self.solid_cut();
        // And likewise the tooltip, for the same reason plus one of its own: it
        // both reads the view and may put a `0xD6` on the wire, which is exactly
        // the mixture the drawing half is kept free of.
        let hover = self.hover_tooltip();
        // And the house under a placement cursor, up here for the same reason
        // the three above are: it reads the view and the pointer, and the pass
        // that draws it runs inside the window's borrow. It is the one thing in
        // the draw list rebuilt per *frame* rather than per packet, because it
        // follows the pointer.
        self.refresh_multi_preview(camera);

        // How big this client's own windows draw, read before the surface is
        // borrowed below: it is the shell's live copy and not the app's loaded
        // one (see `App::window_scale`), so asking for it holds the whole of
        // `self` — which the `&mut self.window` on the next line would refuse.
        let window_scale = self.window_scale();
        let gump_scale = self.gump_scale();
        let radar_facet_extent = radar::RadarExtent::new(
            u16::try_from(self.resources.map.map().width()).expect("a UO map width fits u16"),
            u16::try_from(self.resources.map.map().height()).expect("a UO map height fits u16"),
        )
        .expect("a map has an extent");
        let player_tile = self.world.authoritative.view.as_ref().map(|view| {
            radar::RadarTile::new(
                u32::from(view.player.position.x),
                u32::from(view.player.position.y),
            )
        });
        // Where every open radar window draws, and the **one** place a
        // `RadarView` is built. What the requester below asks for and what
        // `draw_gump_windows` draws are the same value handed across rather
        // than two constructions that agree by arithmetic coincidence — the
        // requested region *is* the drawn region, which is the property
        // `docs/parity.md` exists to protect and the one this whole file used
        // to leave to luck.
        //
        // A frame behind, like every other reader of `drawn_windows` — see its
        // own doc for why that list is the previous frame's layout, and why
        // this client already picks with it. The window's *position* is not:
        // `own_windows` holds the live one, the same one the art pass places
        // the frame with a moment later, so a window being dragged keeps its
        // terrain under its own rim.
        let mut radar_views: Vec<(crate::windows::WindowSubject, radar::RadarView, radar::RadarLod)> =
            Vec::new();
        if let Some(player_tile) = player_tile {
            for (subject, drawn) in &self.windows.drawn_windows {
                let at = self
                    .windows
                    .own_windows
                    .iter()
                    .find(|open| open.subject == *subject)
                    .map(|open| open.at)
                    .unwrap_or_default();
                let view = match drawn {
                    crate::windows::Drawn::Minimap(bounds) => {
                        let (content_at, content_extent) = bounds.content();
                        let placement = Placement {
                            origin: (
                                at.x as f32 + content_at.x as f32 * window_scale.factor(),
                                at.y as f32 + content_at.y as f32 * window_scale.factor(),
                            ),
                            extent: (
                                content_extent.0 as f32 * window_scale.factor(),
                                content_extent.1 as f32 * window_scale.factor(),
                            ),
                            circle: true,
                            rotation: std::f32::consts::FRAC_PI_4,
                        };
                        radar::RadarView::new(
                            openshard_protocol::world::Facet(crate::FACET),
                            player_tile,
                            radar_facet_extent,
                            1.0 / bounds.zoom(),
                            placement,
                            gump_scale,
                        )
                        .with_tangent_margin_fraction(
                            content_extent,
                            bounds.zoom(),
                            crate::panes::minimap::tangent_margin_fraction(),
                        )
                    }
                    crate::windows::Drawn::WorldMap(bounds) => {
                        let (content_at, content_extent) = bounds.content();
                        radar::RadarView::new(
                            openshard_protocol::world::Facet(crate::FACET),
                            bounds.centre,
                            radar_facet_extent,
                            bounds.tiles_per_pixel / (window_scale.factor() * gump_scale),
                            Placement {
                                origin: (
                                    at.x as f32 + content_at.x as f32 * window_scale.factor(),
                                    at.y as f32 + content_at.y as f32 * window_scale.factor(),
                                ),
                                extent: (
                                    content_extent.0 as f32 * window_scale.factor(),
                                    content_extent.1 as f32 * window_scale.factor(),
                                ),
                                circle: false,
                                rotation: 0.0,
                            },
                            gump_scale,
                        )
                    }
                    _ => continue,
                };
                let lod = match subject {
                    crate::windows::WindowSubject::Minimap => self.minimap_radar_lod.update(view),
                    crate::windows::WindowSubject::WorldMap => self.world_map_radar_lod.update(view),
                    _ => continue,
                };
                radar_views.push((*subject, view, lod));
            }
        }
        let Some(window) = self.window.as_mut() else {
            return;
        };
        let atlases_started = Instant::now();
        let (repacked, atlas_work) = ready_atlases(
            &mut self.resources,
            &mut self.graphics,
            &self.world,
            &mut self.repacks,
            window,
            want,
            &wanted,
            &drawn,
        );
        // Full GPU readback fences the device.  It is useful for the explicit
        // field audit, but must not change the timing of the ordinary injected
        // slow-pan scenario whose purpose is to expose asynchronous churn.
        let atlas_audit_due = std::env::var_os("OPENSHARD_ATLAS_AUDIT").is_some()
            && self.lod_sweep.as_mut().is_some_and(|sweep| {
                if (!sweep.atlas_soak && !sweep.stationary_soak) || sweep.elapsed < sweep.next_atlas_audit {
                    return false;
                }
                sweep.next_atlas_audit = sweep.elapsed + Duration::from_secs(2);
                true
            });
        // This is intentionally a run against the connection the user opened,
        // rather than a synthetic scene.  The zoom-soak state leaves every
        // packet, NPC animation and server mutation enabled; the oracle takes
        // a deliberately sparse (two-second) GPU snapshot so it can run for
        // minutes without turning ordinary animation into a readback benchmark.
        let live_oracle_sample = self.lod_sweep.as_mut().and_then(|sweep| {
            if !sweep.live_oracle || !sweep.stationary_zoomed || sweep.elapsed < sweep.next_live_oracle {
                return None;
            }
            sweep.next_live_oracle = sweep.elapsed + Duration::from_secs(2);
            let sample = sweep.live_oracle_samples;
            sweep.live_oracle_samples += 1;
            Some(sample)
        });
        // A person captured this exact frame because it already looked wrong.
        // Give that one-shot dump the same independent atlas/screen/full-LOD0
        // checks as the slow field scenario, without making ordinary play pay
        // for a GPU readback or requiring an environment flag beforehand.
        let manual_frame_dump = self.graphics.frame_dump.clone();
        let manual_frame_diagnostic = manual_frame_dump.is_some();
        if window.composite_output_format != blit::WORLD_FORMAT {
            window.composites.clear();
            self.composite_work.clear();
            window.composite_output_format = blit::WORLD_FORMAT;
        }
        // Prepare at most one immutable block's art in the same stable order
        // the eventual producer will dispatch.  This appends to atlas pages
        // and uploads only their dirty rows; a full/page-limited atlas does
        // not take the ordinary frame's rebuild route for a background job.
        // The job remains pending until an independent offscreen map draw can
        // consume the prepared inputs, so this does not re-enable the former
        // camera-frame capture path.
        if cutaway == Cutaway::OPEN {
            for work in self.composite_work.preparation_candidates() {
                if let Some(ground) = prepare_composite_job(&mut self.resources, window, work.key) {
                    self.composite_work.mark_prepared(work.key, ground);
                }
            }
            let producer_jobs = self.composite_work.take_marked_prepared_for_frame();
            for work in producer_jobs {
                composite_producer::produce(&self.resources, window, &mut self.composite_work, work);
            }
        }
        // Radar terrain for every open view. `radar_queue` bounds the pure-CPU
        // map/colour-table work in base-chunk units; publishing needs no GPU
        // step, and `Screen::radar_chunks` uploads a product only when a
        // content pass first draws it.
        if let Some(colors) = self.resources.radar_colors.as_ref() {
            let mut protected = radar::request_views(
                radar_views.iter().map(|(_, view, lod)| (*view, *lod)),
                &self.radar_cache,
                &mut self.radar_queue,
            );
            let facet = openshard_protocol::world::Facet(crate::FACET);
            let world_map_open = radar_views
                .iter()
                .any(|(subject, _, _)| *subject == crate::windows::WindowSubject::WorldMap);
            if world_map_open && self.radar_cache.begin_sweep(facet) {
                let whole_facet =
                    radar::RadarRegion::new(facet, radar::RadarTile::new(0, 0), radar_facet_extent);
                for lod in (radar::SWEEP_LOD.value()..=radar::max_lod(radar_facet_extent).value()).rev() {
                    let lod = radar::RadarLod::new(lod);
                    for coord in radar::region_chunks(whole_facet, lod) {
                        let key = self.radar_cache.key(facet, lod, coord);
                        if self.radar_cache.get(key).is_none() {
                            self.radar_queue.request_sweep(key);
                        }
                    }
                }
            }
            self.radar_queue.reconcile(&self.radar_cache);
            let producer_centre = player_tile
                .map(radar::world_tile_to_base_chunk)
                .map(|(chunk, _)| chunk)
                .unwrap_or_else(|| radar::RadarChunkCoord::new(0, 0));
            let mut scratch = RadarBuildScratch::default();
            for key in self.radar_queue.take_for_producer_near(producer_centre) {
                let built = build_chunk_reusing(self.resources.map.map(), colors, key, &mut scratch);
                let Some(chunk) = built else {
                    // The slot goes back rather than being lost — see
                    // `RadarWorkQueue::abandon`.
                    self.radar_queue.abandon(key);
                    continue;
                };
                if self.radar_queue.finish(&mut self.radar_cache, chunk) {
                    build_ready_ancestors(&mut self.radar_cache, key, radar::max_lod(radar_facet_extent));
                }
            }
            let selected_for_draw: Vec<_> = protected
                .iter()
                .filter_map(|key| self.radar_cache.select_ready(*key))
                .map(|ready| ready.chunk().key())
                .collect();
            protected.extend(selected_for_draw);
            self.radar_cache.evict_to_budget(protected);
        }
        // Three time-varying halves of a mobile, filled in per frame rather
        // than per packet: the crowd is the only thing that knows what a
        // clock — and a group — has done since the `0x77` landed, and
        // `self.world.presentation.player`/`self.world.presentation.others` were built when it did. Against the atlas
        // as it stands *after* this frame's growth, which is the one the
        // picture below is drawn from.
        Self::advance_to_clocks(
            &self.world.presentation.crowd,
            &window.atlases.mobiles,
            self.world.me(),
            &self.world.motion,
            &mut drawn,
        );
        // Whoever the crowd is still holding a line for, hung above whichever
        // of `drawn`'s mobiles their serial belongs to. Read out here, before
        // `who` is dropped below: a label with no mobile to anchor to has
        // nothing to draw either way, so the two share the same "still on
        // screen" question `mobiles::head_anchor` answers.
        let mut overhead: Vec<(ViewPixel, String, Font, Hue)> = drawn
            .iter()
            .filter_map(|(who, mobile)| {
                let lines: Vec<_> = self.world.presentation.crowd.speaking(*who).collect();
                let anchor = mobiles::head_anchor(mobile, &camera, &window.atlases.mobiles)?;
                // Newest nearest the head, older ones pushed up — which is the
                // way a line arriving reads as *arriving* rather than as the
                // stack shifting under it. `speaking` yields oldest first, so
                // the walk is reversed.
                Some(
                    lines
                        .into_iter()
                        .rev()
                        .enumerate()
                        .map(move |(above, (text, font, hue))| {
                            let mut at = anchor;
                            at.y -= (above as i32) * SPEECH_LINE_HEIGHT;
                            (at, text.to_string(), font, hue)
                        }),
                )
            })
            .flatten()
            .collect();
        // How many are in each pile on the ground, over the pile. The list is
        // `presentation.items` and not the chained one `frame_geometry` draws:
        // the house under a placement cursor is components rather than piles,
        // and every one of them carries `ItemAmount::ONE` by construction — a
        // walk over it would find nothing to say and cost a placement each.
        //
        // Which piles get a number, and what the number reads, is
        // `items::stack_label`'s single rule — see it, and `items::labels` for
        // the anchor. Both live in the render crate beside the placement they
        // are measured against, so a count cannot drift off the picture it
        // belongs to.
        //
        // Its own list rather than another `overhead` entry, because a count
        // is its own *role*: it is drawn in `FontSizes::stack_count` and
        // speech is drawn in `FontSizes::speech`, and one list can only be
        // handed to one size — see `docs/text_sizes.md`'s D1a.
        let counts: Vec<(ViewPixel, String, Font, Hue)> = items::labels(
            &self.world.presentation.items,
            &camera,
            &self.resources.tiledata,
            &self.world.presentation.tile_animations,
            &window.atlases.statics,
            &cutaway,
        )
        .into_iter()
        .map(|(anchor, text)| (anchor, text, items::STACK_COUNT_FONT, Hue::STACK_COUNT))
        .collect();
        // A combat number follows the same mobile anchor as speech, but its
        // y-coordinate is aged every frame so it rises smoothly rather than
        // moving only when the network sends another packet.
        for number in &self.world.presentation.damage_numbers {
            if let Some((_, mobile)) = drawn.iter().find(|(who, _)| *who == Some(number.serial)) {
                if let Some(mut anchor) = mobiles::head_anchor(mobile, &camera, &window.atlases.mobiles) {
                    let progress = number.elapsed.as_secs_f32() / DAMAGE_NUMBER_HOLD.as_secs_f32();
                    anchor.y -= (DAMAGE_NUMBER_RISE as f32 * progress) as i32;
                    overhead.push((anchor, number.amount.to_string(), Font::DEFAULT, number.hue));
                }
            }
        }
        // **The crowd, or none of it** — `frame::Draw::mobiles`, which this
        // function honours because `frame::assemble` does not collect mobiles at
        // all. Emptied here and not at each of the three uses below, so that the
        // picture, the ring and the outline cannot disagree about who is in the
        // frame: a body left out of the world image and still ringed would be a
        // halo round nothing.
        //
        // The speech above is deliberately *not* filtered by it. A label is not a
        // thing standing in the street — `Kind::Nothing`, see `crate::place::Kind`
        // — and turning the crowd off to look at a wall is not a request to go
        // deaf.
        let drawn: Vec<Mobile> = match self.graphics.drawing.mobiles {
            true => drawn.into_iter().map(|(_, mobile)| mobile).collect(),
            false => Vec::new(),
        };
        let atlases_cost = atlases_started.elapsed();

        // The vsync wait, and the reason it is timed on its own: under
        // `PresentMode::Fifo` this call blocks until the display has taken the
        // frame before it, which on an idle client is most of the interval.
        // Counted as build time it would report a client that is asleep as one
        // at full load, and the panel exists to tell those two apart.
        let acquire_started = Instant::now();
        let frame = match window.surface.get_current_texture() {
            // Suboptimal still draws: the surface wants reconfiguring, and the
            // next resize event will do it.
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                frame
            }
            // The swapchain no longer matches the window. Rebuild it and let the
            // next redraw use it; drawing into a stale one is a crash on some
            // backends and a stretched frame on others.
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                window.surface.configure(&window.device, &window.config);
                return;
            }
            // Nothing was acquired and nothing is wrong: the window is hidden,
            // or the compositor took too long. Skipping the frame is the answer.
            other => {
                if !matches!(
                    other,
                    wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded
                ) {
                    eprintln!("acquiring a frame: {other:?}");
                }
                return;
            }
        };
        let wait = acquire_started.elapsed();
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Where the world goes on the surface: the rect the panels left free, so
        // a docked panel shrinks the world rather than covering it.
        let viewport = ui.as_ref().map_or(
            ViewportRect {
                x: 0,
                y: 0,
                width: window.config.width,
                height: window.config.height,
            },
            |(_, _, viewport)| *viewport,
        );

        // The image the world is drawn into. Its size is the camera's, so a
        // resize and a zoom step are the same event here — and recreating it is
        // the only thing either of them costs.
        //
        // Magnified it is the *viewport's* size and the magnification rides in
        // the vertex transform, so the world is drawn at the display's own
        // resolution and the blit below is a copy; minified it is the world's
        // own larger extent and the blit shrinks it. `docs/camera.md` D11 is the
        // argument, and the short of it is that an image of virtual resolution
        // cannot express an offset of one real pixel — which is the whole of
        // what made a magnified scroll coarser than the screen it was on.
        let targets_started = Instant::now();
        let (render_width, render_height) = camera.image_size();
        if window.world.width() != render_width || window.world.height() != render_height {
            window.world = blit::world_texture(&window.device, render_width, render_height);
            window.cutaway_world = blit::world_texture(&window.device, render_width, render_height);
            // Tested pixel for pixel against that image, so it is exactly its
            // size or it is nothing.
            window.depth = renderer::depth_texture(&window.device, render_width, render_height);
            // And the mask with it: it is the colour attachment of a pass whose
            // depth attachment is that buffer, and wgpu requires the two to be
            // one size.
            window.outline_mask = outline::mask_texture(&window.device, render_width, render_height);
            window.select_mask = outline::mask_texture(&window.device, render_width, render_height);
            window.held_mask = outline::mask_texture(&window.device, render_width, render_height);
            // And the G-buffer, whose planes are attachments of those same
            // passes and are read texel for texel against that image.
            window.gbuffer = Gbuffer::new(&window.device, render_width, render_height);
            window.cutaway_gbuffer = Gbuffer::new(&window.device, render_width, render_height);
        }
        let world_view = window.world.create_view(&wgpu::TextureViewDescriptor::default());
        let cutaway_world_view = window
            .cutaway_world
            .create_view(&wgpu::TextureViewDescriptor::default());
        let targets_cost = targets_started.elapsed();

        // **The frame's occluders are built before its pictures are collected**,
        // and that ordering is `docs/lighting_height.md` phase 3's one real cost.
        // A static's drawn row now carries the number this grid gave it
        // (`occlusion::Occlusion::owner_at`), so that a fragment of it can say
        // which occluder it is a point of instead of having that guessed from its
        // height; collecting the pictures first would stamp numbers off the grid
        // of the frame before. Nothing else about either step changed — the
        // statics used to go first for no reason anyone recorded.
        //
        // The lights come from the same camera, cutaway and item list the passes
        // below draw from, so a torch that was not drawn casts nothing and a
        // torch that was is lighting the pixels it is standing in rather than the
        // pixels it stood in last frame.
        //
        // `assemble_geometry` is a free function for the same reason
        // `ready_atlases` is one: it is handed `&mut self.graphics` only for
        // the one field it writes (`occlusion_bake`, through
        // `frame::Inputs::bake`), and every other field it reads is a plain
        // `&`, so the signature alone says this is not a place `self.world`
        // or `self.resources` change.
        let geometry_started = Instant::now();
        let geometry = assemble_geometry(
            &self.resources,
            &mut self.graphics,
            &mut self.world,
            &self.picking,
            window,
            camera,
            &cutaway,
            interior.as_ref(),
            &tuning,
            pick.item,
            pick.mobile,
            selected_item,
            held_mobile,
            &drawn,
        );
        let geometry_cost = geometry_started.elapsed();
        let assembly_costs = geometry.assembly_costs;
        let geometry_costs = geometry.geometry_costs;
        // `geometry` is kept whole rather than destructured here: it travels
        // to `encode_world_passes` and, on the F12 path below, to the dump —
        // both read it as the one value `assemble_geometry` built, not as a
        // dozen loose slices that happen to have arrived together.
        // `fonts.mul` or the operator-supplied TrueType face, never a mix
        // within one frame — see `run`'s doc for why `ttf_font` is an
        // all-or-nothing switch. `fonts.mul` still draws into the world
        // image, at the world's own camera-scaled zoom — a bitmap font's
        // blocky nearest-sampled magnification is the look every other
        // sprite already has. A TrueType face reaches the surface after the
        // blit, but remains in the same *world* compositor layer: see
        // `WorldText` and `draw_world_text`, which keep it below every
        // client window rather than folding it into the HUD.
        let encode_started = Instant::now();
        let world_text = match self.resources.ttf_font.is_some() {
            true => {
                // Nothing is packed here. Growing the atlas needs the size
                // each line is drawn at, and world text is packed by the
                // world-text layer that draws it below.
                //
                // `to_viewport` and not the projection directly: it is the
                // one place that already undoes both a magnifying zoom's
                // vertex-shader scale *and* a minifying one's blit-shrink
                // with the same number — see its own doc. `viewport`'s own
                // corner is added because `to_viewport` answers in pixels
                // of the rect the world goes into, not the surface.
                let project = |anchor: &ViewPixel| {
                    let real = camera.to_viewport(*anchor);
                    GumpPixel::new(
                        viewport.x as i32 + real.x.round() as i32,
                        viewport.y as i32 + real.y.round() as i32,
                    )
                };
                // A named function rather than a closure: a closure that
                // both takes a borrow and returns something borrowed from
                // it cannot state that the two are the same lifetime, and
                // this one is handed two different lists.
                fn screen_of<'a>(
                    list: &'a [(ViewPixel, String, Font, Hue)],
                    project: impl Fn(&ViewPixel) -> GumpPixel,
                ) -> Vec<text::ScreenLabel<'a>> {
                    list.iter()
                        .map(|(anchor, line, _font, hue)| text::ScreenLabel {
                            anchor: project(anchor),
                            text: line.as_str(),
                            hue: *hue,
                        })
                        .collect()
                }
                WorldText::TrueType {
                    labels: screen_of(&overhead, project),
                    counts: screen_of(&counts, project),
                }
            }
            false => {
                let labels: Vec<Label<'_>> = overhead
                    .iter()
                    .chain(counts.iter())
                    .map(|(anchor, line, font, hue)| Label {
                        anchor: *anchor,
                        text: line.as_str(),
                        font: *font,
                        hue: *hue,
                        // Nearer than anything the world draws, rather than
                        // an `Order` of its own: speech reads as an overlay
                        // above whoever said it in every reference client,
                        // and there is no real case here of a wall in front
                        // of the speaker hiding it that a viewer would want
                        // honoured. Worth revisiting with a
                        // `depth::text_priority_z` alongside the mobile's
                        // own if that ever stops being true.
                        depth: 0.0,
                    })
                    .collect();
                WorldText::Bitmap(text::collect(&labels, &self.resources.font_atlas))
            }
        };
        let depth_view = window.depth.create_view(&wgpu::TextureViewDescriptor::default());
        let gbuffer_views = window.gbuffer.views();
        let cutaway_gbuffer_views = window.cutaway_gbuffer.views();
        let target = Target {
            view: &world_view,
            depth: &depth_view,
            gbuffer: &gbuffer_views,
            width: render_width,
            height: render_height,
            projection: camera.projection(),
        };
        let mut encoder = window
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        // `encode_world_passes` is a free function for the same reason
        // `assemble_geometry` is one: every world pass it records is drawn
        // from the values handed to it, and the one thing on `self` it
        // writes — `graphics.solids_held`/`graphics.solids_drawn` — is
        // written through the `&mut GraphicsSettings` its signature already
        // names, not through a `&mut self` that would let it touch anything
        // else too.
        let world_pass_audit = encode_world_passes(
            &mut self.graphics,
            &self.picking,
            window,
            &mut encoder,
            target,
            &view,
            &world_view,
            &gbuffer_views,
            &cutaway_world_view,
            &cutaway_gbuffer_views,
            viewport,
            camera,
            solid_cut,
            &geometry,
            world_text.bitmap_quads(),
            render_width,
            render_height,
            composite_lod,
            composite_revision,
            composite_visible,
        );
        // The composition contract is explicit: world-attached overlays sit
        // over the finished map but below every client window. In particular a
        // mobile's health bar must disappear beneath a vendor gump rather than
        // leak through its art.
        if let Some(shell) = self.shell.as_mut() {
            shell.paint_world_overlays(
                &window.device,
                &window.queue,
                &mut encoder,
                &view,
                [window.config.width, window.config.height],
            );
        }
        // The surface-space half of the world layer. This must remain before
        // `draw_gump_windows`: a character's name or speech belongs to the
        // character in the map, never to whichever paperdoll happens to be
        // open over that point.
        draw_world_text(
            &self.resources,
            window,
            &mut encoder,
            &view,
            fonts,
            self.shell
                .as_ref()
                .map(|shell| shell.pixels_per_point())
                .unwrap_or(1.0),
            &world_text,
        );
        // The shard's dialogs, in the client's own art, over the finished
        // picture and under egui's.
        //
        // Under egui and not over it, deliberately: the widgets that *answer* a
        // gump are still egui's, laid out at the same coordinates in the same
        // units — one gump pixel is one egui point, and the scale below is the
        // window's own scale factor, which is what makes those two spaces the
        // same one. So the art draws the window and egui's transparent widgets
        // sit exactly on it. See `client/app/src/gump.rs`.
        //
        // The atlas grows here rather than when the packet arrived: a page
        // button flips pages inside the client, so what a window needs is every
        // page's art and not the showing one's — `gump::art_of` is that list,
        // and it is asked for on the frame the window is drawn on because that
        // is the frame that knows the window is open at all.
        // `draw_gump_windows` is a free function for the same reason as its
        // neighbours above: `resources.gump_atlas` and `windows.drawn_windows`
        // are the two things on `self` it really writes, and both are named
        // in its signature rather than reached through `&mut self`.
        let mut window_text_quads: Vec<SpriteQuad> = Vec::new();
        let mut window_ttf_quads: Vec<SpriteQuad> = Vec::new();
        draw_gump_windows(
            &mut self.resources,
            &self.world,
            &mut self.windows,
            &self.radar_cache,
            &radar_views,
            self.input.pointer_gump,
            &hover,
            window_scale,
            fonts,
            self.shell.as_ref(),
            window,
            &mut encoder,
            &view,
            &mut window_text_quads,
            &mut window_ttf_quads,
        );
        // Gump-space text belongs either to the client windows above or to
        // the HUD below. World-attached text was already drawn in its own
        // layer, so no later addition can put it above a paperdoll.
        //
        // **One list because there is one pass.** `GumpRenderer` holds a single
        // instance buffer, and `queue.write_buffer` lands before the encoder is
        // submitted — so two `render` calls in a frame do not draw twice, they
        // draw the *second* call's instances twice and lose the first's. That
        // is what happened to every window's text for as long as there was a
        // line in the journal to overwrite it with: a paperdoll's name plate
        // was written, cut, submitted, and then quietly replaced by the chat.
        // A second walk over `drawn_windows` used to stand here, matching the
        // same `(WindowSubject, Drawn)` pairs `draw_gump_windows` matches, to
        // turn each window's text into labels. **It has iterated
        // `std::iter::empty` since window text moved next to its own art** —
        // which it had to, because one text pass for every window drew a lower
        // catalogue's lines over a later paperdoll — so every arm in it was
        // unreachable, and the vendor arm's comment said as much about a walk
        // that was not running at all.
        //
        // It is deleted rather than kept in step. `docs/window_components.md`
        // filed it as `docs/parity.md`'s defect class — one frame assembled in
        // two places, so agreement is a coincidence — and the honest version of
        // that entry is that the second place was already dead. There is one
        // walk now, in `render_passes.rs`, and step 4 was the step that would
        // otherwise have had to update a dialog's arm in both.
        // `draw_chat_and_speech` is a free function like its neighbours
        // above, though a plainer one: nothing it is handed is written back
        // to `self` at all, only appended to the caller's window/HUD lists.
        draw_chat_and_speech(
            &self.resources,
            &self.world,
            &self.chat,
            self.shell.as_ref(),
            window,
            &mut encoder,
            &view,
            chat_style,
            fonts,
            &mut window_text_quads,
            &mut window_ttf_quads,
        );
        let encode_cost = encode_started.elapsed();
        // The UI over it, with no depth attachment: the world's depth buffer
        // ordered the world, and this is drawn on the result.
        if let (Some(shell), Some((_, output, _))) = (self.shell.as_mut(), ui) {
            let painting = Instant::now();
            let timed = profile::begin(window.gpu.as_ref(), "egui", &mut encoder);
            shell.paint(
                &window.device,
                &window.queue,
                &mut encoder,
                &view,
                output,
                [window.config.width, window.config.height],
            );
            profile::end(window.gpu.as_ref(), &mut encoder, timed);
            ui_cost += painting.elapsed();
        }
        let ui_paint_cost = ui_cost.saturating_sub(ui_hud_cost).saturating_sub(ui_layout_cost);
        // Every query closed above, copied out of its set and into the buffer
        // the next frame will map — recorded into this encoder, so it has to
        // happen before the submit and after the last `profile::end`.
        if let Some(gpu) = window.gpu.as_mut() {
            gpu.resolve(&mut encoder);
        }
        window.queue.submit([encoder.finish()]);
        if atlas_audit_due || manual_frame_diagnostic || live_oracle_sample.is_some() {
            if manual_frame_diagnostic {
                tracing::info!("running one-shot LOD diagnostics for manual GPU frame dump");
            }
            audit_static_atlas_pages(window);
            audit_scene_instance_buffers(window);
            if manual_frame_diagnostic || std::env::var_os("OPENSHARD_LOD_SCREEN_AUDIT").is_some() {
                audit_visible_ground_centres(window, self.resources.map.map(), camera);
            }
            let oracle_report = if manual_frame_diagnostic
                || live_oracle_sample.is_some()
                || std::env::var_os("OPENSHARD_LOD_FRAME_ORACLE").is_some()
            {
                Some(audit_lod_map_equivalence(
                    window,
                    camera,
                    &geometry,
                    world_pass_audit,
                ))
            } else {
                None
            };
            // Logs from a running graphical client often have no durable sink.
            // The clicked frame is the evidence, so keep the oracle verdict in
            // its directory next to the exact planes it compared.
            if let (Some(into), Some(report)) = (manual_frame_dump.as_deref(), oracle_report.as_deref()) {
                if let Err(error) = std::fs::create_dir_all(into)
                    .and_then(|()| std::fs::write(into.join("lod-oracle.txt"), report))
                {
                    tracing::warn!(into = %into.display(), %error, "writing LOD oracle report");
                }
            }
            if let (Some(sample), Some(report)) = (live_oracle_sample, oracle_report.as_deref()) {
                let into = frame_dump_root().join("live-oracle");
                let result = std::fs::create_dir_all(&into)
                    .and_then(|()| std::fs::write(into.join(format!("sample-{sample:05}.txt")), report));
                if let Err(error) = result {
                    tracing::warn!(into = %into.display(), %error, "writing live LOD oracle sample");
                } else if report.starts_with("status=mismatch") {
                    tracing::error!(sample, into = %into.display(), "live LOD oracle caught a server-driven frame mismatch");
                } else {
                    tracing::info!(sample, "live LOD oracle matches this server-driven frame");
                }
            }
        }
        // And the frame closed, which is what makes those buffers eligible to be
        // mapped. What comes back is an older frame's timings — see [`profile`]
        // for why that is the right trade and not a defect.
        if let Some(gpu) = window.gpu.as_mut() {
            gpu.end_frame(&window.device, &window.queue);
        }
        // **This frame, written out** — F12, and `docs/parity.md`'s first
        // backlog item. After the submit above and not beside the blit, because
        // what is read back has to be pixels the device has actually been given
        // the commands for; the world image, the G-buffer and the instance
        // buffers all still hold this frame's own, since nothing writes them
        // again until the next one.
        //
        // Not the surface: what is presented has the HUD, the panels and the
        // solids overlay on top of it, and a tool's frame has none of those.
        // What a comparison wants is the world as the blit left it, so the blit
        // is run again into a texture of its own — the same pass, the same
        // lighting, the same rect — once per plane. `docs/parity.md` D5.
        if let Some(into) = self.graphics.frame_dump.take() {
            let dump = window.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("frame dump"),
                size: wgpu::Extent3d {
                    width: window.config.width,
                    height: window.config.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // **The world's format and not the surface's**, which is what
                // the first press of F12 found: a surface is whatever the
                // compositor offered — here `Rgba16Float`, eight bytes a texel
                // — and reading it back as RGBA8 is a copy `wgpu` refuses. Even
                // where it is four (`Bgra8Unorm`) it is the wrong four: the
                // picture would come out with its red and blue swapped, and
                // nothing would say so. `isolated_scene` has always drawn into
                // this format, and a dump exists to be compared with that one.
                format: blit::WORLD_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let dump_view = dump.create_view(&wgpu::TextureViewDescriptor::default());
            // A second pipeline for that format, built here and dropped with the
            // dump: `Screen::blit` is bound to the surface's format and cannot
            // draw into this target. The same shader and the same uniforms — the
            // format is the whole of the difference — and it is built per press
            // rather than kept, because every frame that is not being dumped
            // would otherwise carry a pipeline nobody draws with.
            let mut dump_blit = Blit::new(&window.device, blit::WORLD_FORMAT);
            let planes = openshard_client_render::dump::planes(
                &window.device,
                &window.queue,
                &mut dump_blit,
                &dump,
                blit::Frame {
                    // A view of `dump`, made one line above it: `dump::planes`'s
                    // own contract, and the whole of what this call site has to
                    // keep true.
                    target: &dump_view,
                    world: &world_view,
                    gbuffer: &gbuffer_views,
                    face_instances: window.statics.instances_buffer(),
                    item_instances: window.items_pass.instances_buffer(),
                    mobile_instances: window.mobile_pass.instances_buffer(),
                    mesh_instances: window.mesh_pass.rows_buffer(),
                    ground_instances: window.renderer.instances_buffer(),
                    zoom: camera.zoom(),
                    rect: viewport,
                },
                &geometry.lighting,
                // Every one of them. A dump taken because something looked wrong
                // is taken once, and a plane left out is a plane somebody has to
                // reproduce the moment they want it — by which time the frame is
                // gone.
                &View::ALL,
            );
            match write_frame_dump(
                &into,
                &planes,
                geometry
                    .asked_for
                    .as_deref()
                    .expect("the summary is taken whenever a dump is armed, above"),
            ) {
                Ok(()) => tracing::info!(
                    into = %into.display(),
                    planes = planes.len(),
                    "frame dumped",
                ),
                // A dump that could not be written is a diagnostic that failed,
                // not a frame that failed: the client goes on drawing.
                Err(error) => tracing::warn!(into = %into.display(), %error, "dumping the frame"),
            }
        }
        // Presentation moved onto the queue in wgpu 30; the texture is consumed.
        window.queue.present(frame);
        // And the next frame is asked for here rather than through the timer,
        // unconditionally while somebody is watching. This is the pacer: the
        // surface presents in FIFO, so `get_current_texture` above blocks the
        // next frame until the display has taken this one, and asking again
        // straight away runs the loop at the display's own rate instead of at a
        // 16ms timer that beats against it.
        //
        // Every frame and not only the gliding ones, which is the change: a
        // client that only redrew when something moved dropped to 12.5 frames a
        // second the moment the player stood still, and however correct the
        // reason was, what it looked like was a stall. The timer stays for the
        // window nobody is looking at — see [`App::pacing`].
        if watched {
            window.window.request_redraw();
        }
        let took = started.elapsed();
        // The interval between two *drawn* frames, and where this one's time
        // went: the pacing and the price, which are the two things a drop in
        // frame rate can be — and the price split between the panels and the
        // world, which are the two things the price can be. See [`frames`].
        //
        // The scene is what is left after the UI and the wait rather than a
        // fourth clock, so the three always add up to the frame exactly: a
        // fourth `Instant` would leave a remainder nobody could account for.
        let scene = took.saturating_sub(ui_cost).saturating_sub(wait);
        // The device's own number, which is *not* about this frame: it is
        // whichever frame the timestamps have come back for, two or three ago.
        // Recorded against this one anyway, because what it answers — "is the
        // wait above slack or a stall" — is a question about a standing cost and
        // not about one frame's spike. See [`profile`].
        let gpu = self
            .window
            .as_ref()
            .and_then(|window| window.gpu.as_ref())
            .map(profile::Gpu::total);
        self.frames.record(
            started.saturating_duration_since(self.last_frame),
            ui_cost,
            scene,
            wait,
            gpu,
            repacked,
        );
        if let Some(frame) = self.frames.frames().last().copied() {
            let gpu_passes = self
                .window
                .as_ref()
                .and_then(|window| window.gpu.as_ref())
                .map_or(&[][..], crate::profile::Gpu::passes);
            crate::jank::record(
                frame,
                crate::jank::CpuPasses {
                    ui_hud: ui_hud_cost,
                    ui_terrain: hud_timings.terrain,
                    ui_route: hud_timings.route,
                    ui_occluders: hud_timings.occluders,
                    ui_picking: hud_timings.picking,
                    ui_perf: hud_timings.perf,
                    ui_layout: ui_layout_cost,
                    ui_paint: ui_paint_cost,
                    facts: facts_cost,
                    atlases: atlases_cost,
                    targets: targets_cost,
                    geometry: geometry_cost,
                    lighting: assembly_costs.lighting,
                    ground: assembly_costs.ground,
                    statics: assembly_costs.statics,
                    static_walk: assembly_costs.static_walk,
                    static_sort: assembly_costs.static_sort,
                    items: assembly_costs.items,
                    static_cache_copy: geometry_costs.static_cache_copy,
                    split_corners: geometry_costs.split,
                    overlays: geometry_costs.overlays,
                    ground_quads: geometry_costs.ground_quads,
                    static_rows: geometry_costs.static_rows,
                    item_rows: geometry_costs.item_rows,
                    encode: encode_cost,
                    encode_ground: world_pass_audit.cpu_ground,
                    encode_composites: world_pass_audit.cpu_composites,
                    encode_ground_detail: world_pass_audit.cpu_ground_detail,
                    ground_detail_cpu_uniforms: world_pass_audit.ground_detail_cpu_uniforms,
                    ground_detail_cpu_serialize: world_pass_audit.ground_detail_cpu_serialize,
                    ground_detail_cpu_upload: world_pass_audit.ground_detail_cpu_upload,
                    ground_detail_cpu_pass: world_pass_audit.ground_detail_cpu_pass,
                    encode_statics: world_pass_audit.cpu_statics,
                    encode_items: world_pass_audit.cpu_items,
                    composite_blocks: world_pass_audit.ready_blocks,
                    composite_bindings_created: world_pass_audit.composite_bindings_created,
                    composite_bindings_reused: world_pass_audit.composite_bindings_reused,
                    composite_cpu_upload: world_pass_audit.composite_cpu_upload,
                    composite_cpu_bindings: world_pass_audit.composite_cpu_bindings,
                    composite_cpu_pass: world_pass_audit.composite_cpu_pass,
                    static_animated: assembly_costs.static_animated,
                },
                atlas_work,
                gpu_passes,
            );
        }
        self.last_frame = started;
    }
}

/// Where a frame dump goes: `OPENSHARD_FRAME_DUMP_DIR`, or a directory of our
/// own under the system temp.
///
/// Never the source tree — one dump is thirteen uncompressed pictures and none
/// of them belongs in a diff. The same rule, and the same shape, as
/// [`dst::dump_dir`](crate::dst)'s.
///
/// **Not `OPENSHARD_FRAME_DUMP`**, which the render crate's own tools already
/// read as the *file* their one picture is written to
/// (`examples/isolated_scene.rs`, `tests/cost.rs`). One name meaning a file to
/// one caller and a directory to another is precisely the quiet difference
/// `docs/parity.md` exists to stop, so the client's knob is its own name — and a
/// directory, because what the client has to dump is every plane at once plus
/// the inputs they came from.
pub(crate) fn frame_dump_root() -> std::path::PathBuf {
    std::env::var_os("OPENSHARD_FRAME_DUMP_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("openshard-frame"))
}

/// One dump: a directory holding a picture per plane, named for the plane, and
/// the inputs the frame was assembled from.
///
/// The summary is written last on purpose — a directory that has `inputs.txt` in
/// it has every picture beside it, so a reader never compares a half-written
/// dump against a whole one.
///
/// **`inputs.txt` is written verbatim and gets no line of its own from here**,
/// which is worth stating because one line of it reads oddly beside the
/// directory: `view` is what the *window* was showing when the key was pressed,
/// while each picture beside it is named for the plane it actually is. Adding a
/// note to explain that would be a line the tool's own summary does not have,
/// and the two are written to be diffed — an extra line here is a difference in
/// every comparison, forever, to save one sentence of documentation.
pub(crate) fn write_frame_dump(
    into: &std::path::Path,
    planes: &[(View, Vec<u8>)],
    asked_for: &str,
) -> std::io::Result<()> {
    std::fs::create_dir_all(into)?;
    for (view, png) in planes {
        std::fs::write(into.join(format!("{}.png", view.name())), png)?;
    }
    std::fs::write(into.join("inputs.txt"), asked_for)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_client_render::follow::Gaze;
    use openshard_protocol::direction::{Direction, Facing};
    use openshard_protocol::wire::{Graphic, Hue};
    use openshard_protocol::world::Point;
    use openshard_uofiles::anim::BodyKind;

    #[test]
    fn local_render_projection_uses_game_motion_not_a_presentation_clock() {
        let start = Point::new(100, 100, 0);
        let end = Point::new(101, 100, 0);
        let east = Facing::walking(Direction::East);
        let mut motion = PlayerMotion::new(start, east);
        let mut player = Mobile {
            at: end,
            body: Graphic(0x0190),
            group: BodyKind::of(Graphic(0x0190)).standing(),
            facing: Direction::East,
            frame: openshard_uofiles::anim::AnimationFrameIndex(0),
            from: None,
            hue: Hue::NONE,
            // Deliberately an impossible stale presentation pose: the test
            // proves the frame projection replaces it from GameMotion alone.
            drawn: Gaze::on(start),
            equipment: Vec::new().into(),
        };

        motion.accept_trusted_step(end, east);
        motion.advance(openshard_movement::WALK_HOLD / 2);
        App::project_local_motion(&motion, &mut player);

        assert_eq!(player.drawn, motion.drawn());
        assert_ne!(player.drawn, Gaze::on(start));
        assert_eq!(player.from, Some(start));
    }

    #[test]
    fn visible_composite_gate_allows_lod_one_but_holds_lod_two() {
        assert_eq!(visible_composite_lod(BlockLod::Lod0), BlockLod::Lod0);
        assert_eq!(visible_composite_lod(BlockLod::Lod1), BlockLod::Lod1);
        assert_eq!(visible_composite_lod(BlockLod::Lod2), BlockLod::Lod1);
    }

    #[test]
    fn dump_state_names_safe_lod0_fallbacks_separately_from_disabled_lod() {
        let pass = |requested_lod, ready_blocks, live_ground_quads| WorldPassAudit {
            requested_lod,
            composite_revision: ImmutableRevision(4),
            ready_blocks,
            live_ground_quads,
            full_ground_quads: 12,
            cpu_ground: Duration::ZERO,
            cpu_composites: Duration::ZERO,
            cpu_ground_detail: Duration::ZERO,
            ground_detail_cpu_uniforms: Duration::ZERO,
            ground_detail_cpu_serialize: Duration::ZERO,
            ground_detail_cpu_upload: Duration::ZERO,
            ground_detail_cpu_pass: Duration::ZERO,
            cpu_statics: Duration::ZERO,
            cpu_items: Duration::ZERO,
            composite_bindings_created: 0,
            composite_bindings_reused: 0,
            composite_cpu_upload: Duration::ZERO,
            composite_cpu_bindings: Duration::ZERO,
            composite_cpu_pass: Duration::ZERO,
        };
        assert_eq!(
            lod_diagnostic_state(pass(BlockLod::Lod0, 0, 12), 0),
            "lod-disabled"
        );
        assert_eq!(
            lod_diagnostic_state(pass(BlockLod::Lod1, 0, 12), 0),
            "lod-not-ready-safe-lod0"
        );
        assert_eq!(
            lod_diagnostic_state(pass(BlockLod::Lod1, 0, 12), 1),
            "quarantine-safe-lod0"
        );
        assert_eq!(
            lod_diagnostic_state(pass(BlockLod::Lod1, 1, 4), 0),
            "cached-with-lod0-fallback"
        );
    }
}
