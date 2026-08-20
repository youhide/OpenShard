//! The radar's own pages, and its own draws.
//!
//! # Why this is not a `GumpArt` variant
//!
//! [`GumpArt`](crate::gump::GumpArt) is a closed enum whose two arms both **name
//! a picture in a client file**, and [`GumpAtlas`](crate::gump::GumpAtlas) is a
//! shelf packer for art that never changes once it is packed. A radar is a
//! bitmap this client *generates*: [`RadarChunkRenderer`] keeps revisioned
//! immutable products in bounded texture-array pages, and
//! [`RadarOverlayRenderer`] draws the solid rectangles under and over them.
//!
//! The deciding property is mutability rather than identity. Shelf-packing
//! something that is rewritten per step either fragments the atlas or forces the
//! whole-atlas rebuild `docs/client.md` names as the tightest resource in the
//! client — and a mutable entry in a structure whose entries are immutable by
//! construction is the same shape of mistake `docs/boats.md` refused when it
//! kept a moving hull out of `Obstructions`.
//!
//! That is also *cheaper* than the alternative: the gump atlas is 2048 square,
//! and reserving a corner of it for a radar would carry sixteen megabytes to
//! draw a fraction of it.
//!
//! # What it does not have
//!
//! **No hue.** `hues.mul` tints art, and there is no ramp for a colour that was
//! never in a client file — `radarcol.mul`'s entries *are* the colours.
//!
//! **No per-draw uniforms.** Everything that differs between the draws of one
//! window travels in an instance buffer, because
//! [`wgpu::Queue::write_buffer`] is ordered against the submission rather than
//! against the commands inside it: a uniform rewritten between two recorded
//! draws reaches both of them with the last values written.
//!
//! **No blending, and nothing to discard.** A radar tile with no colour is
//! [`radar::UNKNOWN`](crate::radar::UNKNOWN) rather than transparent, so the
//! terrain is opaque everywhere by construction, and a marker over it replaces
//! rather than tints what it stands on.

use openshard_uofiles::color::Color16;
use std::collections::BTreeMap;

use crate::gump::Frame;
use crate::radar::{BASE_CHUNK_TILES, MARKER_ARMS, RadarChunk, RadarChunkKey, RadarRegion};
use crate::renderer::QUAD;

/// Where the radar is drawn and how big it is, in gump pixels.
///
/// The same coordinate space every window in this client is placed in, so a
/// radar beside a skill sheet is placed the way the skill sheet is. Not the
/// terrain's own size: a region of 256 tiles shown in a 128-pixel window is
/// drawn at one physical pixel a tile. The caller expands the region for a
/// HiDPI surface instead of enlarging texels, so the sampler remains nearest.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placement {
    /// The top-left corner.
    pub origin: (f32, f32),
    pub extent: (f32, f32),
    /// Clip all terrain and overlays to this circle.  The minimap uses this
    /// with its classic round frame; rectangular consumers can leave it off.
    pub circle: bool,
    /// Clockwise rotation in screen coordinates, in radians.
    pub rotation: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radar::{RadarChunkCoord, RadarRevision};
    use openshard_protocol::world::Facet;

    fn chunk(x: u32, y: u32) -> RadarChunk {
        RadarChunk::new(
            RadarChunkKey::new(Facet(0), 0, RadarChunkCoord::new(x, y), RadarRevision(0)),
            vec![Color16(0x03e0); usize::from(BASE_CHUNK_TILES).pow(2)],
        )
        .expect("a complete chunk")
    }

    #[test]
    fn selecting_a_region_splits_at_chunk_edges_without_a_uv_gap() {
        let west = chunk(0, 0);
        let east = chunk(1, 0);
        let region = RadarRegion {
            facet: Facet(0),
            lod: 0,
            origin: (32, 0),
            extent: (64, 16),
        };
        let draws = select_region_chunks(
            region,
            Placement {
                origin: (10.0, 20.0),
                extent: (128.0, 32.0),
                circle: false,
                rotation: 0.0,
            },
            [&west, &east],
        );

        assert_eq!(draws.len(), 2);
        assert_eq!(draws[0].placement.extent.0, 64.0);
        assert_eq!(
            draws[0].placement.origin.0 + draws[0].placement.extent.0,
            draws[1].placement.origin.0
        );
        assert_eq!(draws[0].uv.origin.0, 0.5);
        assert_eq!(draws[0].uv.extent.0, 0.5);
        assert_eq!(draws[1].uv.origin.0, 0.0);
        assert_eq!(draws[1].uv.extent.0, 0.5);
    }

    #[test]
    fn selecting_a_region_does_not_mix_facets() {
        let matching = chunk(0, 0);
        let other_facet = RadarChunk::new(
            RadarChunkKey::new(Facet(1), 0, RadarChunkCoord::new(0, 0), RadarRevision(0)),
            vec![Color16(0x03e0); usize::from(BASE_CHUNK_TILES).pow(2)],
        )
        .unwrap();
        let draws = select_region_chunks(
            RadarRegion {
                facet: Facet(0),
                lod: 0,
                origin: (0, 0),
                extent: (16, 16),
            },
            Placement {
                origin: (0.0, 0.0),
                extent: (16.0, 16.0),
                circle: false,
                rotation: 0.0,
            },
            [&matching, &other_facet],
        );
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].chunk.key(), matching.key());
    }

    #[test]
    fn a_coarse_stand_in_is_placed_by_its_own_lod_and_painted_under_the_fine_one() {
        // The level-one product covering the four base chunks at the origin: the
        // same pixel count over twice the ground in each direction.
        let parent = RadarChunk::new(
            RadarChunkKey::new(Facet(0), 1, RadarChunkCoord::new(0, 0), RadarRevision(0)),
            vec![Color16(0x03e0); usize::from(BASE_CHUNK_TILES).pow(2)],
        )
        .unwrap();
        let north_west = chunk(0, 0);
        let side = BASE_CHUNK_TILES * 2;
        let draws = select_region_chunks(
            RadarRegion {
                facet: Facet(0),
                lod: 0,
                origin: (0, 0),
                extent: (side, side),
            },
            Placement {
                origin: (0.0, 0.0),
                extent: (f32::from(side), f32::from(side)),
                circle: false,
                rotation: 0.0,
            },
            // Handed in fine-first on purpose: the order drawn is this
            // function's answer, not its caller's.
            [&north_west, &parent],
        );

        assert_eq!(draws.len(), 2);
        assert_eq!(draws[0].chunk.key().lod(), 1, "the stand-in is painted first");
        assert_eq!(
            draws[0].placement.extent,
            (f32::from(side), f32::from(side)),
            "and it covers all four children's ground, not one chunk's",
        );
        assert_eq!(draws[0].uv.extent, (1.0, 1.0), "all of its own pixels");
        assert_eq!(draws[1].chunk.key().lod(), 0, "the exact product paints over it");
        assert_eq!(
            draws[1].placement.extent,
            (f32::from(BASE_CHUNK_TILES), f32::from(BASE_CHUNK_TILES)),
        );
    }

    #[test]
    fn one_product_is_one_draw_however_many_requests_fell_back_to_it() {
        let parent = RadarChunk::new(
            RadarChunkKey::new(Facet(0), 1, RadarChunkCoord::new(0, 0), RadarRevision(0)),
            vec![Color16(0x03e0); usize::from(BASE_CHUNK_TILES).pow(2)],
        )
        .unwrap();
        let side = BASE_CHUNK_TILES * 2;
        let draws = select_region_chunks(
            RadarRegion {
                facet: Facet(0),
                lod: 0,
                origin: (0, 0),
                extent: (side, side),
            },
            Placement {
                origin: (0.0, 0.0),
                extent: (f32::from(side), f32::from(side)),
                circle: false,
                rotation: 0.0,
            },
            // What four base-chunk requests all falling back to one ancestor
            // hand in — the same product, four times.
            [&parent, &parent, &parent, &parent],
        );
        assert_eq!(draws.len(), 1);
    }

    /// A region of `side` tiles drawn at `magnify` window pixels a tile.
    fn window(side: u16, magnify: f32) -> (RadarRegion, Placement) {
        (
            RadarRegion {
                facet: Facet(0),
                lod: 0,
                origin: (100, 100),
                extent: (side, side),
            },
            Placement {
                origin: (10.0, 20.0),
                extent: (f32::from(side) * magnify, f32::from(side) * magnify),
                circle: false,
                rotation: 0.0,
            },
        )
    }

    #[test]
    fn a_marker_is_a_cross_of_tile_sized_quads_where_the_body_stands() {
        let (region, at) = window(16, 2.0);
        let marker = RadarMarker {
            tile: (104, 108),
            color: Color16(0x7FFF),
        };
        let quads = select_marker_quads(region, at, [&marker]);

        assert_eq!(quads.len(), MARKER_ARMS.len(), "every arm is on the map");
        // The centre arm comes first, and it is the tile itself: four tiles east
        // and eight south of the region's corner, at two window pixels a tile.
        assert_eq!(quads[0].0.origin, (10.0 + 8.0, 20.0 + 16.0));
        assert_eq!(
            quads[0].0.extent,
            (2.0, 2.0),
            "a marker pixel is a tile, so it keeps its size as the window is magnified"
        );
        assert!(quads.iter().all(|(_, color)| *color == marker.color));
    }

    #[test]
    fn a_markers_arms_are_dropped_at_the_regions_edge_rather_than_clamped_onto_it() {
        let (region, at) = window(16, 1.0);
        // The region's own north-west tile: the west and north arms are outside
        // the window, and drawing them at the edge would put the cross's centre
        // one tile from where the body actually is.
        let corner = RadarMarker {
            tile: (100, 100),
            color: Color16(0x7FFF),
        };
        let quads = select_marker_quads(region, at, [&corner]);
        assert_eq!(quads.len(), 3);
        assert!(quads.iter().all(|(placement, _)| {
            placement.origin.0 >= at.origin.0 && placement.origin.1 >= at.origin.1
        }));

        let outside = RadarMarker {
            tile: (200, 200),
            color: Color16(0x7FFF),
        };
        assert!(
            select_marker_quads(region, at, [&outside]).is_empty(),
            "a body off the shown rectangle has no marker at all, not one at its edge"
        );
    }
}

/// A rectangle in a resident chunk texture, expressed as normalised UVs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ChunkUv {
    pub origin: (f32, f32),
    pub extent: (f32, f32),
}

/// One complete chunk's part of a [`RadarRegion`].  The placement is already
/// clipped to the requested world rectangle, so adjacent chunks share their
/// edge exactly instead of relying on filtered texels to hide a seam.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RadarChunkDraw<'a> {
    pub chunk: &'a RadarChunk,
    pub placement: Placement,
    pub uv: ChunkUv,
}

/// Select the chunks and exact source UVs covering a region.
///
/// `ready` may contain a larger cache working set, and products from another
/// facet are ignored. Each chunk is placed by **its own** LOD rather than the
/// region's: that is what lets a coarse ancestor stand in for a child the cache
/// has not built yet — see
/// [`RadarCache::select_ready`](crate::radar::RadarCache::select_ready). The
/// result is ordered coarsest first, so a ready fine product paints over the
/// stand-in that covers it, and one key contributes one draw however many
/// requests fell back to it.
///
/// Missing products are still omitted. A rectangle no product covers is the
/// backdrop's to fill, not this function's to fake.
#[must_use]
pub fn select_region_chunks<'a>(
    region: RadarRegion,
    at: Placement,
    ready: impl IntoIterator<Item = &'a RadarChunk>,
) -> Vec<RadarChunkDraw<'a>> {
    if region.extent.0 == 0 || region.extent.1 == 0 || at.extent.0 <= 0.0 || at.extent.1 <= 0.0 {
        return Vec::new();
    }
    let left = u64::from(region.origin.0);
    let top = u64::from(region.origin.1);
    let right = left.saturating_add(u64::from(region.extent.0));
    let bottom = top.saturating_add(u64::from(region.extent.1));
    let mut selected: Vec<_> = ready
        .into_iter()
        .filter_map(|chunk| {
            let key = chunk.key();
            if key.facet() != region.facet {
                return None;
            }
            // How much world one of this product's texels is, and therefore how
            // much of it the whole product is. A level-one chunk is the same
            // number of pixels over twice the ground in each direction.
            let texel_world = 1_u64.checked_shl(u32::from(key.lod()))?;
            let chunk_world = u64::from(BASE_CHUNK_TILES).saturating_mul(texel_world);
            let chunk_left = u64::from(key.chunk().x).saturating_mul(chunk_world);
            let chunk_top = u64::from(key.chunk().y).saturating_mul(chunk_world);
            let chunk_right = chunk_left.saturating_add(chunk_world);
            let chunk_bottom = chunk_top.saturating_add(chunk_world);
            let x0 = left.max(chunk_left);
            let y0 = top.max(chunk_top);
            let x1 = right.min(chunk_right);
            let y1 = bottom.min(chunk_bottom);
            (x0 < x1 && y0 < y1).then(|| {
                let scale_x = at.extent.0 / f32::from(region.extent.0);
                let scale_y = at.extent.1 / f32::from(region.extent.1);
                RadarChunkDraw {
                    chunk,
                    placement: Placement {
                        origin: (
                            at.origin.0 + (x0 - left) as f32 * scale_x,
                            at.origin.1 + (y0 - top) as f32 * scale_y,
                        ),
                        extent: ((x1 - x0) as f32 * scale_x, (y1 - y0) as f32 * scale_y),
                        circle: at.circle,
                        rotation: at.rotation,
                    },
                    uv: ChunkUv {
                        origin: (
                            (x0 - chunk_left) as f32 / chunk_world as f32,
                            (y0 - chunk_top) as f32 / chunk_world as f32,
                        ),
                        extent: (
                            (x1 - x0) as f32 / chunk_world as f32,
                            (y1 - y0) as f32 / chunk_world as f32,
                        ),
                    },
                }
            })
        })
        .collect();
    // Coarsest first, then newest, then in reading order: a stand-in is painted
    // before whatever covers it more exactly, and the sort is total so a frame's
    // draw list does not depend on the order the cache happened to answer in.
    selected.sort_by_key(|draw| {
        let key = draw.chunk.key();
        (
            std::cmp::Reverse(key.lod()),
            key.revision(),
            key.chunk().y,
            key.chunk().x,
        )
    });
    // One product, one draw. Four requests falling back to the same ancestor
    // are four answers naming one rectangle, and drawing it four times would be
    // three redundant passes over the same window pixels.
    selected.dedup_by_key(|draw| draw.chunk.key());
    selected
}

/// Fixed-size, recreatable GPU pages for immutable radar chunks.
///
/// A page is uploaded only when a new CPU key first becomes resident.  Normal
/// walking only selects different pages and rewrites this pass's tiny uniforms.
/// The array is deliberately bounded; eviction only discards a GPU copy, never
/// the `RadarChunk` held by the content cache.
#[derive(Debug)]
pub struct RadarChunkRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    quad: wgpu::Buffer,
    texture: wgpu::Texture,
    pages: BTreeMap<RadarChunkKey, ResidentPage>,
    capacity: u32,
    clock: u64,
}

#[derive(Clone, Copy, Debug)]
struct ResidentPage {
    layer: u32,
    last_used: u64,
}

/// Surface size, and the scale it is drawn at: the whole of what every chunk in
/// one region draw shares.
const CHUNK_UNIFORM_BYTES: u64 = 64;
/// Placement origin and extent, source UV origin and extent, page layer.
const CHUNK_INSTANCE_STRIDE: u64 = 36;
const CHUNK_RGBA_BYTES: u64 = (BASE_CHUNK_TILES as u64) * (BASE_CHUNK_TILES as u64) * 4;

impl RadarChunkRenderer {
    /// Create a texture-array page cache with at most `byte_budget` bytes.
    /// At least one page is retained even for a tiny non-zero budget.
    #[must_use]
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, byte_budget: u64) -> Self {
        let limit = u64::from(device.limits().max_texture_array_layers);
        let capacity = (byte_budget / CHUNK_RGBA_BYTES)
            .clamp(1, limit)
            .try_into()
            .unwrap_or(u32::MAX);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("radar chunk pages"),
            size: wgpu::Extent3d {
                width: u32::from(BASE_CHUNK_TILES),
                height: u32::from(BASE_CHUNK_TILES),
                depth_or_array_layers: capacity,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("radar chunk nearest sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("radar chunk draw"),
            size: CHUNK_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("radar chunk"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("radar chunk"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("radar chunk"),
            source: wgpu::ShaderSource::Wgsl(include_str!("radar_chunk.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("radar chunk"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("radar chunk"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        }],
                    }),
                    // One instance a chunk rectangle — the same shape as the
                    // gump pass's, and for the same reason: what differs between
                    // the draws of one layer belongs in a buffer written once,
                    // not in a uniform rewritten between them.
                    Some(wgpu::VertexBufferLayout {
                        array_stride: CHUNK_INSTANCE_STRIDE,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 0,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 8,
                                shader_location: 2,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 16,
                                shader_location: 3,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 24,
                                shader_location: 4,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Uint32,
                                offset: 32,
                                shader_location: 5,
                            },
                        ],
                    }),
                ],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let quad = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("radar chunk quad"),
            size: std::mem::size_of_val(&QUAD) as u64,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: true,
        });
        let bytes: Vec<u8> = QUAD.iter().flat_map(|value| value.to_le_bytes()).collect();
        quad.get_mapped_range_mut(..)
            .expect("fresh quad is mapped")
            .copy_from_slice(&bytes);
        quad.unmap();
        Self {
            pipeline,
            bind_group,
            uniforms,
            quad,
            texture,
            pages: BTreeMap::new(),
            capacity,
            clock: 0,
        }
    }

    #[must_use]
    pub const fn byte_capacity(&self) -> u64 {
        (self.capacity as u64) * CHUNK_RGBA_BYTES
    }

    #[must_use]
    pub fn resident_len(&self) -> usize {
        self.pages.len()
    }

    fn resident_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        chunk: &RadarChunk,
    ) -> u32 {
        self.clock = self.clock.wrapping_add(1);
        let key = chunk.key();
        if let Some(page) = self.pages.get_mut(&key) {
            page.last_used = self.clock;
            return page.layer;
        }
        let layer = if self.pages.len() < self.capacity as usize {
            self.pages.len() as u32
        } else {
            let (&evict, page) = self
                .pages
                .iter()
                .min_by_key(|(_, page)| page.last_used)
                .expect("non-empty when full");
            let layer = page.layer;
            self.pages.remove(&evict);
            layer
        };
        let mut bytes = Vec::with_capacity(CHUNK_RGBA_BYTES as usize);
        for colour in chunk.pixels() {
            let rgb = colour.rgb8();
            bytes.extend_from_slice(&[rgb.red, rgb.green, rgb.blue, 255]);
        }
        // Encode this copy before the draw that uses it.  A queue write would
        // run before the whole command buffer, and a one-page cache could then
        // overwrite the first chunk before its already-recorded draw executes.
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("radar chunk upload"),
            size: CHUNK_RGBA_BYTES,
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&staging, 0, &bytes);
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(u32::from(BASE_CHUNK_TILES) * 4),
                    rows_per_image: Some(u32::from(BASE_CHUNK_TILES)),
                },
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: u32::from(BASE_CHUNK_TILES),
                height: u32::from(BASE_CHUNK_TILES),
                depth_or_array_layers: 1,
            },
        );
        self.pages.insert(
            key,
            ResidentPage {
                layer,
                last_used: self.clock,
            },
        );
        layer
    }

    /// Draw complete ready chunks, clipped to the minimap window.  Call this
    /// before recording player/waypoint geometry in the same window layer.
    ///
    /// Every selected chunk is one instance of a single draw.  That is not an
    /// optimisation: a chunk's placement, source rectangle and page cannot live
    /// in the uniform block, because [`wgpu::Queue::write_buffer`] is ordered
    /// against the *submission* rather than against the commands inside it, so
    /// a uniform rewritten between two recorded draws gives both of them the
    /// last values written.  The same rule is why the page upload below is an
    /// encoder copy from a staging buffer.
    #[allow(clippy::too_many_arguments)]
    pub fn render_region<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame<'_>,
        region: RadarRegion,
        at: Placement,
        ready: impl IntoIterator<Item = &'a RadarChunk>,
    ) {
        let mut draws = select_region_chunks(region, at, ready);
        if draws.is_empty() {
            return;
        }
        // A region wider than the whole page cache would evict a page this very
        // call had already handed to an instance, and that instance would then
        // sample whatever replaced it.  Dropping the surplus leaves those chunks
        // undrawn instead of drawn wrong; it is unreachable at any sane budget —
        // a 160-tile window touches at most sixteen chunks — so it is a bound
        // rather than a policy.
        if draws.len() > self.capacity as usize {
            eprintln!(
                "radar page cache holds {} of {} chunks this region needs: the rest go undrawn",
                self.capacity,
                draws.len()
            );
            draws.truncate(self.capacity as usize);
        }
        let Some(scissor) = window_scissor(frame, at) else {
            return;
        };
        let mut uniform_bytes = Vec::with_capacity(CHUNK_UNIFORM_BYTES as usize);
        let center = (
            (at.origin.0 + at.extent.0 / 2.0) * frame.scale,
            (at.origin.1 + at.extent.1 / 2.0) * frame.scale,
        );
        let radius = at.extent.0.min(at.extent.1) * frame.scale / 2.0;
        for value in [
            frame.width as f32,
            frame.height as f32,
            frame.scale,
            0.0,
            center.0,
            center.1,
            radius,
            if at.circle { 1.0 } else { 0.0 },
            at.origin.0,
            at.origin.1,
            at.extent.0,
            at.extent.1,
            at.rotation,
        ] {
            uniform_bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.uniforms, 0, &uniform_bytes);
        // Residency first, for all of them: every page copy has to be recorded
        // before the pass that samples it begins, and a pass cannot be open
        // while the encoder is asked for a copy.
        let mut instance_bytes = Vec::with_capacity(draws.len() * CHUNK_INSTANCE_STRIDE as usize);
        for draw in &draws {
            let layer = self.resident_layer(device, queue, encoder, draw.chunk);
            for value in [
                draw.placement.origin.0,
                draw.placement.origin.1,
                draw.placement.extent.0,
                draw.placement.extent.1,
                draw.uv.origin.0,
                draw.uv.origin.1,
                draw.uv.extent.0,
                draw.uv.extent.1,
            ] {
                instance_bytes.extend_from_slice(&value.to_le_bytes());
            }
            instance_bytes.extend_from_slice(&layer.to_le_bytes());
        }
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("radar chunk instances"),
            size: draws.len() as u64 * CHUNK_INSTANCE_STRIDE,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&instances, 0, &instance_bytes);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("radar chunk"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.set_vertex_buffer(1, instances.slice(..));
        pass.set_scissor_rect(scissor.0, scissor.1, scissor.2, scissor.3);
        pass.draw(0..4, 0..draws.len() as u32);
    }
}

/// The window's own clip rectangle in surface pixels: `(x, y, width, height)`,
/// or `None` where the placement has no pixels on this frame at all.
///
/// One rule for the terrain and for the overlay over it. Two windows'-worth of
/// this arithmetic would be two chances for a marker to survive a clip its own
/// terrain did not, which is a player dot outside the window it belongs to.
fn window_scissor(frame: Frame<'_>, at: Placement) -> Option<(u32, u32, u32, u32)> {
    let x = (at.origin.0 * frame.scale).max(0.0).floor() as u32;
    let y = (at.origin.1 * frame.scale).max(0.0).floor() as u32;
    let right = ((at.origin.0 + at.extent.0) * frame.scale)
        .clamp(0.0, frame.width as f32)
        .ceil() as u32;
    let bottom = ((at.origin.1 + at.extent.1) * frame.scale)
        .clamp(0.0, frame.height as f32)
        .ceil() as u32;
    (x < right && y < bottom).then_some((x, y, right - x, bottom - y))
}

/// One overlay dot over ready terrain: where a body stands, in world tiles.
///
/// Deliberately *not* part of a [`RadarChunk`]. A marker moves every step and a
/// terrain product does not, so stamping one into a cached raster would make a
/// step invalidate terrain — the one thing `docs/minimap_lod_plan.md` exists to
/// prevent. It is drawn after the chunks, from per-frame data that costs an
/// instance rather than an upload.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RadarMarker {
    /// The world tile it stands on — the same coordinate space a
    /// [`RadarRegion`]'s origin is in, not a pixel inside the window.
    pub tile: (u32, u32),
    pub color: Color16,
}

/// Bytes per overlay quad: placement origin and extent, then a colour.
const MARKER_INSTANCE_STRIDE: u64 = 32;

/// The screen rectangles a region's markers occupy, one per arm of each cross.
///
/// A marker outside the region contributes nothing, and an arm that falls off
/// the region's edge is dropped rather than clamped onto it — the same
/// clipping [`radar::mark`](crate::radar::mark) does in a bitmap, so the cross
/// beside the window's edge looks the same in both pictures.
#[must_use]
pub fn select_marker_quads<'a>(
    region: RadarRegion,
    at: Placement,
    markers: impl IntoIterator<Item = &'a RadarMarker>,
) -> Vec<(Placement, Color16)> {
    if region.extent.0 == 0 || region.extent.1 == 0 || at.extent.0 <= 0.0 || at.extent.1 <= 0.0 {
        return Vec::new();
    }
    // One tile's worth of window, which is also one marker pixel's size: the
    // cross is drawn in tiles, so it keeps its shape at any window scale
    // instead of shrinking to a dot as the region grows.
    let scale_x = at.extent.0 / f32::from(region.extent.0);
    let scale_y = at.extent.1 / f32::from(region.extent.1);
    let mut quads = Vec::new();
    for marker in markers {
        for (dx, dy) in MARKER_ARMS {
            let (Some(x), Some(y)) = (
                marker.tile.0.checked_add_signed(dx),
                marker.tile.1.checked_add_signed(dy),
            ) else {
                continue;
            };
            let (Some(column), Some(row)) = (x.checked_sub(region.origin.0), y.checked_sub(region.origin.1))
            else {
                continue;
            };
            if column >= u32::from(region.extent.0) || row >= u32::from(region.extent.1) {
                continue;
            }
            quads.push((
                Placement {
                    origin: (
                        at.origin.0 + column as f32 * scale_x,
                        at.origin.1 + row as f32 * scale_y,
                    ),
                    extent: (scale_x, scale_y),
                    circle: at.circle,
                    rotation: at.rotation,
                },
                marker.color,
            ));
        }
    }
    quads
}

/// The pass that draws a radar's solid rectangles: the backdrop under the
/// terrain, and the markers over it.
///
/// It owns a pipeline and nothing else. There is no residency here and there is
/// nothing to evict — the whole of a frame's overlay is a handful of rectangles
/// written into a fresh instance buffer, which is why a walking player uploads
/// no texture at all.
#[derive(Debug)]
pub struct RadarOverlayRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    quad: wgpu::Buffer,
}

impl RadarOverlayRenderer {
    /// Build the overlay pass against the **surface's** format, for
    /// [`RadarChunkRenderer::new`]'s reason: a minimap is drawn on the finished
    /// picture rather than into the world texture.
    #[must_use]
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("radar overlay frame"),
            size: CHUNK_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("radar overlay"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("radar overlay"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("radar overlay"),
            source: wgpu::ShaderSource::Wgsl(include_str!("radar_marker.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("radar overlay"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("radar overlay"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        }],
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: MARKER_INSTANCE_STRIDE,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 0,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 8,
                                shader_location: 2,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 16,
                                shader_location: 3,
                            },
                        ],
                    }),
                ],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                // No blending: a marker replaces the terrain under it, exactly
                // as the bitmap stamp does. A translucent player dot would take
                // the colour of the ground it stands on, which is the one thing
                // it must not do.
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let quad = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("radar overlay quad"),
            size: std::mem::size_of_val(&QUAD) as u64,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: true,
        });
        let bytes: Vec<u8> = QUAD.iter().flat_map(|value| value.to_le_bytes()).collect();
        quad.get_mapped_range_mut(..)
            .expect("fresh quad is mapped")
            .copy_from_slice(&bytes);
        quad.unmap();
        Self {
            pipeline,
            bind_group,
            uniforms,
            quad,
        }
    }

    /// Draw `markers` over the terrain already recorded for this window.
    ///
    /// `region` and `at` must be the pair [`RadarChunkRenderer::render_region`]
    /// was given, and the call must come after it: this is the same window,
    /// clipped by the same rectangle, painted on top.
    #[allow(clippy::too_many_arguments)]
    pub fn render_markers<'a>(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame<'_>,
        region: RadarRegion,
        at: Placement,
        markers: impl IntoIterator<Item = &'a RadarMarker>,
    ) {
        self.draw_quads(
            device,
            queue,
            encoder,
            frame,
            at,
            &select_marker_quads(region, at, markers),
        );
    }

    /// Fill the whole window with one colour, **before** its terrain.
    ///
    /// A minimap with no ready chunk under part of it would otherwise show the
    /// world through that part — a hole, which `docs/minimap_lod_plan.md`'s
    /// contract rules out as firmly as it rules out stale pixels. Painting
    /// [`radar::UNKNOWN`](crate::radar::UNKNOWN) under everything says the same
    /// thing there that it says inside a chunk: this ground is not mapped yet.
    /// It is a floor rather than a fallback — a ready coarser ancestor is still
    /// the better picture wherever one exists.
    pub fn render_backdrop(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame<'_>,
        at: Placement,
        color: Color16,
    ) {
        self.draw_quads(device, queue, encoder, frame, at, &[(at, color)]);
    }

    /// Record one instanced draw of solid rectangles, clipped to `at`.
    ///
    /// Both of this pass's callers come through here, so a marker and the
    /// backdrop under it cannot end up clipped by two different rectangles.
    fn draw_quads(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame<'_>,
        at: Placement,
        quads: &[(Placement, Color16)],
    ) {
        if quads.is_empty() {
            return;
        }
        let Some(scissor) = window_scissor(frame, at) else {
            return;
        };
        let mut uniform_bytes = Vec::with_capacity(CHUNK_UNIFORM_BYTES as usize);
        let center = (
            (at.origin.0 + at.extent.0 / 2.0) * frame.scale,
            (at.origin.1 + at.extent.1 / 2.0) * frame.scale,
        );
        let radius = at.extent.0.min(at.extent.1) * frame.scale / 2.0;
        for value in [
            frame.width as f32,
            frame.height as f32,
            frame.scale,
            0.0,
            center.0,
            center.1,
            radius,
            if at.circle { 1.0 } else { 0.0 },
            at.origin.0,
            at.origin.1,
            at.extent.0,
            at.extent.1,
            at.rotation,
        ] {
            uniform_bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.uniforms, 0, &uniform_bytes);
        let mut instance_bytes = Vec::with_capacity(quads.len() * MARKER_INSTANCE_STRIDE as usize);
        for (placement, color) in quads {
            let rgb = color.rgb8();
            for value in [
                placement.origin.0,
                placement.origin.1,
                placement.extent.0,
                placement.extent.1,
                f32::from(rgb.red) / 255.0,
                f32::from(rgb.green) / 255.0,
                f32::from(rgb.blue) / 255.0,
                1.0,
            ] {
                instance_bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("radar overlay instances"),
            size: quads.len() as u64 * MARKER_INSTANCE_STRIDE,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&instances, 0, &instance_bytes);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("radar overlay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.set_vertex_buffer(1, instances.slice(..));
        pass.set_scissor_rect(scissor.0, scissor.1, scissor.2, scissor.3);
        pass.draw(0..4, 0..quads.len() as u32);
    }
}
