//! The world image, scaled onto the viewport.
//!
//! This is where the zoom is, and it is the only place. The three world passes
//! draw at 1:1 into an offscreen texture of [`Camera::render_width`] by
//! [`Camera::render_height`]; this stretches that texture over the rectangle the
//! UI left free. Every quad, every atlas region and every pixel-exact assertion
//! in the world passes therefore keeps meaning what it meant, and what is new is
//! one fullscreen quad and one sampler.
//!
//! Scaling the *geometry* instead would resample five-bit art through a filter
//! at every fractional step, and would put a scale factor inside three passes
//! that currently have none. It is also what ClassicUO does in substance, and it
//! is the only arrangement where an interface drawn at 1:1 stays crisp over a
//! magnified world.
//!
//! [`Camera::render_width`]: crate::camera::Camera::render_width
//! [`Camera::render_height`]: crate::camera::Camera::render_height

use crate::camera::Zoom;
use crate::light::Lighting;

/// How many lights the uniform block holds. `blit.wgsl`'s `MAX_LIGHTS`, and the
/// two are one number: the array's length is fixed at shader compile time, so a
/// buffer written to a different one is rejected by wgpu rather than drawn
/// wrongly.
const MAX_LIGHTS: usize = Lighting::MAX;

/// The uniform block's size: six header `vec4`s — the sky ambient with the light
/// count, the ground ambient, the occlusion grid's rectangle, which view to
/// draw, and the sun's direction and colour — then three per light: where it
/// burns, what colour, and which way it points.
const LIGHTING_BYTES: u64 = (6 + 3 * MAX_LIGHTS as u64) * 16;

/// Where the world image goes on the surface, in physical pixels.
///
/// Not always the whole window: a docked panel shrinks it, which is the same
/// path a resize already takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ViewportRect {
    /// Pixels from the surface's left edge.
    pub x: u32,
    /// Pixels from its top edge.
    pub y: u32,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
}

/// What one blit draws, and where.
///
/// The four values that describe *this frame's* picture, grouped for the reason
/// [`Target`](crate::renderer::Target) is: they always travel together, and a
/// `render` that took them one by one alongside a device, a queue, an encoder
/// and the lighting would be a call whose arguments are told apart by position
/// alone.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    /// Where the picture goes — the surface.
    pub target: &'a wgpu::TextureView,
    /// The world image it comes from.
    pub world: &'a wgpu::TextureView,
    /// Which tile each of that image's pixels came from — see [`crate::place`].
    /// What the lighting is computed against, and the reason this pass can tell
    /// a wall's lit face from the shadow behind it.
    pub place: &'a wgpu::TextureView,
    /// The statics pass's own instance buffer, bound a second time as storage
    /// so a static's fragment can read `instances[id]` back instead of
    /// carrying its `(x, y, z)` on every pixel of its own picture — see
    /// `docs/gbuffer.md` decision 2. [`dummy_instances`] when a caller has
    /// none this frame.
    pub face_instances: &'a wgpu::Buffer,
    /// The same, for the mobiles pass — a separate buffer because mobiles are
    /// a separate `SpriteRenderer` with its own instance list, not a second
    /// user of the statics one.
    pub mobile_instances: &'a wgpu::Buffer,
    /// `docs/gbuffer.md` step 4c's mesh-face pass's own row buffer, bound a
    /// second time as storage. A fragment's `Stance::MeshFace` sentinel
    /// (`place.z`'s stance bits) tells this pass to read `mesh_instances[id]`
    /// instead of `face_instances[id]` — a mesh face has no picture, so its
    /// row is not `SpriteQuad`-shaped and lives in its own buffer.
    /// [`dummy_mesh_instances`] when a caller has none this frame.
    pub mesh_instances: &'a wgpu::Buffer,
    /// The ground pass's own instance buffer, bound a second time as storage —
    /// `docs/gbuffer.md` step 7, the ground half of what step 3 did for a
    /// static's tile. A `Kind::Land` pixel's `place.x`/`place.y` is an id into
    /// this, not a tile, the same move and the same reason.
    /// [`dummy_ground_instances`] when a caller has none this frame — every
    /// real frame does, since the ground pass always runs.
    pub ground_instances: &'a wgpu::Buffer,
    /// Which way the scaling goes, and so which sampler is right.
    pub zoom: Zoom,
    /// The rectangle of `target` the world gets.
    pub rect: ViewportRect,
}

/// Draws one texture over a rectangle of another.
#[derive(Debug)]
pub struct Blit {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// For magnifying: a texel has to stay a square.
    nearest: wgpu::Sampler,
    /// For minifying: nearest would sample one texel in four and the ground
    /// would shimmer as the camera walks.
    linear: wgpu::Sampler,
    /// The frame's lights, rewritten every frame — see [`crate::light`].
    lighting: wgpu::Buffer,
    /// What stands in their way, as one texel a tile — see
    /// [`crate::occlusion`]. Recreated when the frame's grid changes size, which
    /// is a zoom step or a resize and not an ordinary frame; rewritten every
    /// frame, because the camera moves and the grid is relative to it.
    occluders: wgpu::Texture,
    /// What each of those tiles *is*, over the same rectangle and in the same
    /// order — the sky field today, an aperture and a body's opacity when the
    /// steps that write them land. See [`crate::occlusion::Occlusion::field_bytes`].
    ///
    /// A second texture and not four more channels of the first: the occluder
    /// cell is what a ray walks through cell after cell in a loop, and this is
    /// read once per fragment. `docs/lighting_world.md` decides it once, there.
    field: wgpu::Texture,
    /// What the cells above name — one texel a reference, folded into rows
    /// [`crate::occlusion::LIST_ROW`] wide. See
    /// [`Occlusion::id_bytes`](crate::occlusion::Occlusion::id_bytes).
    ///
    /// The level step 23.1 put between a cell and a solid, and the whole of what
    /// it costs the shader is this one extra `textureLoad`. What it buys is that
    /// a solid is a shape the world holds rather than a tile's property, so the
    /// same box can be referenced by every cell it stands over.
    ids: wgpu::Texture,
    /// The solids those references name — one texel a solid, folded the same way.
    /// See [`Occlusion::solid_bytes`](crate::occlusion::Occlusion::solid_bytes).
    ///
    /// Not a storage buffer, and that is decision 30.5 rather than a preference:
    /// the ceiling here is WebGL2, which has neither compute nor storage
    /// buffers, so a list the shader can index has to be a texture read with
    /// `textureLoad`.
    ///
    /// Its *width* is fixed and its height grows with the frame, which is the
    /// opposite of the two planes above: they are the camera's rectangle and
    /// this is a list whose length is what the camera happens to be looking at.
    solids: wgpu::Texture,
    /// The hole in each of those solids, one texel a solid and in the same
    /// order — see
    /// [`Occlusion::aperture_bytes`](crate::occlusion::Occlusion::aperture_bytes).
    ///
    /// Grown with [`Blit::solids`] and never on its own, because the two are
    /// indexed by one number. **Written only when something in the frame has a
    /// hole**: the solid's own `HOLED` bit is what makes the shader read this
    /// at all, so a frame with no window in it neither lays these bytes out nor
    /// sends them, which is every frame of a real map until step 16 lands.
    apertures: wgpu::Texture,
    /// One solid's own horizontal footprint, one texel a solid and in the same
    /// order — see
    /// [`Occlusion::footprint_bytes`](crate::occlusion::Occlusion::footprint_bytes).
    ///
    /// Grown with [`Blit::solids`] and never on its own, the same reason
    /// [`Blit::apertures`] is — but written every frame regardless, unlike
    /// apertures: every solid has a footprint, where almost none has a hole.
    footprints: wgpu::Texture,
    /// And what its two whole-unit `z` channels rounded away, one texel a solid
    /// and in the same order — see
    /// [`Occlusion::solid_z_bytes`](crate::occlusion::Occlusion::solid_z_bytes).
    ///
    /// Grown and written on exactly [`Blit::footprints`]'s terms, and for the
    /// same reason: it is the other half of one solid's own box, and the two
    /// halves must never be indexed by a number only one of them holds.
    solid_z: wgpu::Texture,
}

impl Blit {
    /// Build the pipeline for a target of `format`.
    ///
    /// `format` should be a non-sRGB one, as everywhere else here: this pass
    /// copies the world image through untouched, and an sRGB target would gamma
    /// it on the way out — see the crate docs.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let sampler = |label, filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            })
        };

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // The place attachment and the occlusion grid, both integer
                // textures and therefore both unfilterable: there is no sampler
                // for either, and the shader reads exact texels.
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // And the field plane over the same rectangle — what a tile *is*
                // rather than what a ray passes through. Unfilterable for the
                // same reason: a sky byte averaged with its neighbour's would be
                // a second blur over the one `Occlusion::blur_sky` already did,
                // at the resolution of the screen instead of of the map.
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // And the solids the grid indexes into — the list of decision
                // 30, and one of the three that are not pictures of the camera's
                // rectangle.
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // And the hole in each of those solids, indexed by the same
                // number. A plane beside the list rather than four more channels
                // of it: `Occlusion::aperture_bytes` argues why, and the short
                // form is that the list is what the walk reads in a loop and a
                // hole is what almost nothing has.
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // And the references between the two: what a cell counts through
                // is a run of these, and each one names a solid. Step 23.1.
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // The statics and mobiles passes' own instance data, each
                // bound a second time as storage — decision 2's `instances[id]`.
                // Read-only: this pass never writes a fragment's own instance
                // back, only looks one up. `docs/gbuffer.md` step 3.
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // The mesh-face pass's own row buffer — `docs/gbuffer.md`
                // step 4c. Read-only, the same reason 9 and 10 are.
                wgpu::BindGroupLayoutEntry {
                    binding: 11,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // The ground pass's own instance data, bound a second time as
                // storage — `docs/gbuffer.md` step 7, `Kind::Land`'s share of
                // decision 2. Read-only, the same reason 9, 10 and 11 are.
                wgpu::BindGroupLayoutEntry {
                    binding: 12,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // A solid's own horizontal footprint, indexed by the same
                // number as 6 and 7 above — `docs/lighting_raymarch.md`'s
                // "second bigger idea": the reader `box_of` used to have no
                // channel to read, arrived.
                wgpu::BindGroupLayoutEntry {
                    binding: 13,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // And the vertical half of the same box: what 6's two whole-unit
                // `z` channels rounded away. `docs/lighting_height.md` phase 2.
                wgpu::BindGroupLayoutEntry {
                    binding: 14,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        // Zeroed at creation, which is *not* the identity — a zero ambient is
        // black — so the first frame writes it before drawing. Every frame
        // does; this only has to be a buffer of the right size to bind.
        let lighting = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lighting"),
            size: LIGHTING_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit"),
            // `src/shaders/blit.wesl`, compiled to plain WGSL by `build.rs`.
            source: wgpu::ShaderSource::Wgsl(include_str!(concat!(env!("OUT_DIR"), "/blit.wgsl")).into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                // The corners come from the vertex index. WebGL2 has
                // `gl_VertexID`, so this costs nothing there either.
                buffers: &[],
            },
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
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            // No depth at all: the world's depth buffer ordered the world, and
            // this draws the result of that as a picture.
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            layout,
            nearest: sampler("blit nearest", wgpu::FilterMode::Nearest),
            linear: sampler("blit linear", wgpu::FilterMode::Linear),
            lighting,
            // One texel, which is a grid of one open tile: a daylit frame binds
            // it and never reads it, and the first lit frame replaces it. A
            // texture of no size is not a thing wgpu will make.
            occluders: grid_texture(device, "occluders", 1, 1),
            field: grid_texture(device, "field", 1, 1),
            // One row, which is a list of no solids: the grid above says every
            // tile stands nothing, so nothing indexes into it.
            ids: grid_texture(device, "solid ids", crate::occlusion::LIST_ROW, 1),
            solids: grid_texture(device, "solids", crate::occlusion::LIST_ROW, 1),
            apertures: grid_texture(device, "apertures", crate::occlusion::LIST_ROW, 1),
            footprints: grid_texture(device, "footprints", crate::occlusion::LIST_ROW, 1),
            solid_z: grid_texture(device, "solid z", crate::occlusion::LIST_ROW, 1),
        }
    }

    /// Draw `world` over `rect` of `target`, clearing whatever is outside it.
    ///
    /// The filter follows the direction of the zoom: nearest magnifying, linear
    /// minifying. Two rules rather than one, because pixel art wants its texels
    /// square when they are grown and wants them averaged when four of them have
    /// to become one.
    ///
    /// `lighting` is what the frame's flames do to the image on the way past —
    /// see [`crate::light`]. [`Lighting::NONE`] leaves it a copy, which is what
    /// a daylit frame and every frame test pass.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame<'_>,
        lighting: &Lighting,
    ) {
        let Frame {
            target,
            world,
            place,
            face_instances,
            mobile_instances,
            mesh_instances,
            ground_instances,
            zoom,
            rect,
        } = frame;
        queue.write_buffer(&self.lighting, 0, &lighting_bytes(lighting));
        self.upload_grid(device, queue, lighting);
        // A bind group per call rather than per `Blit`: the world texture is
        // recreated on every resize and every zoom step, and a cached group
        // would be a handle to a texture that is no longer being drawn into.
        let magnifying = zoom.numerator() >= zoom.denominator();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(world),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(if magnifying {
                        &self.nearest
                    } else {
                        &self.linear
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.lighting.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(place),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(
                        &self
                            .occluders
                            .create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(
                        &self.field.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(
                        &self.solids.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(
                        &self
                            .apertures
                            .create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(
                        &self.ids.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: face_instances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: mobile_instances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: mesh_instances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: ground_instances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 13,
                    resource: wgpu::BindingResource::TextureView(
                        &self
                            .footprints
                            .create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 14,
                    resource: wgpu::BindingResource::TextureView(
                        &self.solid_z.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Clears the whole surface, not just the rect: whatever the
                    // UI does not cover and the world does not fill is this
                    // frame's, and leaving the last one there would smear.
                    load: wgpu::LoadOp::Clear(crate::renderer::CLEAR),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        if rect.width == 0 || rect.height == 0 {
            // A minimised window, or a UI that has taken the whole surface. The
            // clear above still happened, which is the frame.
            return;
        }

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        // The viewport is what puts the quad in the rect: the shader emits clip
        // space corners and this is the rectangle clip space maps onto.
        pass.set_viewport(
            rect.x as f32,
            rect.y as f32,
            rect.width as f32,
            rect.height as f32,
            0.0,
            1.0,
        );
        pass.draw(0..4, 0..1);
    }
}

/// The uniform block `blit.wgsl` reads, laid out by hand.
///
/// Written as bytes rather than through a `#[repr(C)]` struct for the reason
/// every other pass here does it: the layout is a contract with text the Rust
/// compiler never sees, and a field order stated once in the shader and once in
/// a struct is two statements that can disagree. This way the writing order is
/// the shader's declaration order, in one place.
///
/// Lights past [`Lighting::MAX`] are dropped rather than wrapping the array —
/// [`crate::light::collect`] already keeps only the nearest that many, so this
/// is the second half of one rule and not a policy of its own.
fn lighting_bytes(lighting: &Lighting) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(LIGHTING_BYTES as usize);
    let count = lighting.lights.len().min(MAX_LIGHTS);

    // `sky`, with the light count in the fourth channel: what a tile with an
    // open column above it gets, before the field says how much of that column
    // is open. See `crate::light::Ambient`.
    for channel in lighting.ambient.sky {
        bytes.extend_from_slice(&channel.to_le_bytes());
    }
    bytes.extend_from_slice(&(count as f32).to_le_bytes());

    // `ground`: the floor every tile gets, roof or no roof. A fourth channel of
    // nothing, because a `vec3` in a uniform block is padded to four either way
    // and a stated zero is a zero somebody wrote.
    for channel in lighting.ambient.ground {
        bytes.extend_from_slice(&channel.to_le_bytes());
    }
    bytes.extend_from_slice(&0.0_f32.to_le_bytes());

    // `grid`: where the occlusion texture's corner is on the map, and how big it
    // is. Signed integers, which is what the shader declares that field as —
    // a rectangle may start at a negative tile for a camera near the map's
    // corner, and the walk has to be able to say so.
    let bounds = lighting.occlusion.bounds();
    for value in [bounds.min_x, bounds.min_y, bounds.width(), bounds.height()] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    // `view`: which of the pass's own values to draw instead of the frame. Three
    // words of padding after it, because a uniform block's members are aligned
    // to sixteen bytes and the array that follows has to start on one.
    bytes.extend_from_slice(&(lighting.view as u32).to_le_bytes());
    bytes.extend_from_slice(&[0; 12]);

    // The sun: its direction, then the height above which nothing in this
    // frame's grid can stop it. That height is where a sunbeam's segment *ends* —
    // the sun has no position, so this is what gives its ray a far end to walk to
    // — and over an open street it ends a tile or two out.
    //
    // A frame with no sun writes a direction of zero and an intensity of zero,
    // and the shader tests the intensity: one branch, and a night frame does not
    // pay for a sky it does not have.
    let sun = lighting.sun.unwrap_or(crate::light::Sun {
        toward: [0.0; 3],
        color: [0.0; 3],
        intensity: 0.0,
    });
    for value in sun.toward {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    // `f32` and not an integer: the shader compares it against a ray's height,
    // which is fractional, and a grid with nothing in it says "everything is
    // above the tallest thing" by being below every ray.
    let ceiling = lighting.occlusion.tallest().unwrap_or(i32::MIN / 2) as f32;
    bytes.extend_from_slice(&ceiling.to_le_bytes());
    for channel in sun.color {
        bytes.extend_from_slice(&channel.to_le_bytes());
    }
    bytes.extend_from_slice(&sun.intensity.to_le_bytes());

    for light in &lighting.lights[..count] {
        // A fire in the open lights every direction, and says so with an axis of
        // nothing and a rim below every cosine there is: the shader's one test is
        // `cos_half > -1`, so an omnidirectional flame costs a comparison and
        // never a dot product. See `crate::light::Beam`.
        let beam = light.beam.unwrap_or(crate::light::Beam {
            toward: [0.0; 3],
            cos_half: -1.0,
        });
        for value in [
            light.at.x,
            light.at.y,
            light.z,
            light.radius,
            light.color[0],
            light.color[1],
            light.color[2],
            light.intensity,
            beam.toward[0],
            beam.toward[1],
            beam.toward[2],
            beam.cos_half,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    // The tail of the array is never read — the shader stops at the count — but
    // the buffer is bound whole, and a short write leaves whatever the last
    // frame put there. Zeroed, so a partial upload cannot be mistaken for one.
    bytes.resize(LIGHTING_BYTES as usize, 0);
    bytes
}

/// One plane of the frame's grid as a texture: one texel a tile. See
/// [`crate::occlusion`] for what the four channels of each hold.
///
/// `Rgba8Uint` and not four floats, because every one of them is a byte in the
/// grid already, and because an integer texture cannot be filtered — a wall
/// averaged with the open ground beside it would be a half-wall standing on
/// neither tile.
///
/// Both planes go through here: they are the same rectangle, the same format and
/// the same order, and the only thing that differs is what the bytes mean.
fn grid_texture(device: &wgpu::Device, label: &str, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Uint,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// One zeroed row, standing in for [`Frame::face_instances`] or
/// [`Frame::mobile_instances`] when a caller has no real one to bind —
/// ground-only fixtures, and [`crate::plan`]'s synthetic picture, which by its
/// own doc only ever writes `Kind::Land`. A bind group needs a valid resource
/// in every slot regardless of which branch the shader takes.
///
/// A free function and not a field of [`Blit`]: [`Blit::render`] takes
/// `&mut self`, and a `Frame` borrowing a buffer `self` owns cannot be built
/// for a call that also borrows `self` mutably. The caller owns this instead,
/// the same way it owns a real instance buffer when it has one.
pub fn dummy_instances(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("blit dummy instances"),
        size: crate::sprite::SpriteQuad::STRIDE,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    })
}

/// The same, for [`Frame::mesh_instances`] — a caller with no mesh faces to
/// draw this frame still needs a valid resource in binding 11.
pub fn dummy_mesh_instances(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("blit dummy mesh instances"),
        size: crate::mesh_face::MeshFaceRow::STRIDE,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    })
}

/// The same, for [`Frame::ground_instances`] — a fixture with no real ground
/// pass still needs a valid resource in binding 12. Real frames never need
/// this: the ground pass always runs and its own instance buffer is always
/// the argument, empty or not.
pub fn dummy_ground_instances(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("blit dummy ground instances"),
        size: crate::ground::GroundQuad::STRIDE,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    })
}

impl Blit {
    /// Put this frame's grid on the GPU — both planes — growing the textures if
    /// its rectangle has changed size.
    ///
    /// The *size* changes on a zoom step or a resize and on nothing else: the
    /// grid is the visible tiles grown by a fixed margin, so a camera walking
    /// keeps its dimensions and only its contents move. Recreating a texture per
    /// frame would be a hundred kilobytes of allocation on every one of them.
    ///
    /// The two planes are uploaded together and never apart: they are one
    /// rectangle indexed by one `lighting.grid`, and a frame that wrote the
    /// occluders of this camera over the field of the last one would light every
    /// tile from a place the picture is not of.
    fn upload_grid(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, lighting: &Lighting) {
        let bounds = lighting.occlusion.bounds();
        let (width, height) = (bounds.width().max(1) as u32, bounds.height().max(1) as u32);
        if self.occluders.width() != width || self.occluders.height() != height {
            self.occluders = grid_texture(device, "occluders", width, height);
            self.field = grid_texture(device, "field", width, height);
        }
        let occluders = lighting.occlusion.bytes();
        if occluders.is_empty() {
            // A daylit frame, or one with no grid at all: the textures keep
            // whatever they held and the shader never reads them — there are no
            // lights to walk a ray for, and a grid of no tiles is open sky
            // everywhere by `Occlusion::sky_at`'s own rule, which is what the
            // shader answers for a texel outside the rectangle.
            return;
        }
        let write = |texture: &wgpu::Texture, bytes: &[u8]| {
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
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        };
        write(&self.occluders, &occluders);
        write(&self.field, &lighting.occlusion.field_bytes());

        // And the lists the grid indexes into. Their heights are the frame's own
        // counts and not the camera's rectangle, so they are grown on their own
        // terms: a camera that has not moved keeps the same rows, and walking into
        // a city grows them. Both `id_bytes` and `solid_bytes` pad to a whole row,
        // which is what makes the upload's `bytes_per_row` exact.
        let row = crate::occlusion::LIST_ROW;
        let bytes = lighting.occlusion.solid_bytes();
        let rows = (bytes.len() / (row as usize * 4)) as u32;
        if self.solids.height() != rows {
            // The four planes are indexed by one number, so they are grown
            // together and never apart — a hole, footprint or height-fraction
            // texel at an index the solid texture holds and one of these does
            // not would read as no hole, or the whole tile, or half a unit low,
            // which is the one direction this cannot be allowed to be wrong in
            // silently.
            self.solids = grid_texture(device, "solids", row, rows);
            self.apertures = grid_texture(device, "apertures", row, rows);
            self.footprints = grid_texture(device, "footprints", row, rows);
            self.solid_z = grid_texture(device, "solid z", row, rows);
        }
        // The references are their own height: equal to the solids' until
        // something is shared, and *not* assumed equal, because the day the two
        // differ is the day a shared solid arrives and a list grown to the wrong
        // one would drop the last cell's references off the end.
        let references = lighting.occlusion.id_bytes();
        let id_rows = (references.len() / (row as usize * 4)) as u32;
        if self.ids.height() != id_rows {
            self.ids = grid_texture(device, "solid ids", row, id_rows);
        }
        let list = |texture: &wgpu::Texture, bytes: &[u8]| {
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
                    bytes_per_row: Some(row * 4),
                    rows_per_image: Some((bytes.len() / (row as usize * 4)) as u32),
                },
                wgpu::Extent3d {
                    width: row,
                    height: (bytes.len() / (row as usize * 4)) as u32,
                    depth_or_array_layers: 1,
                },
            );
        };
        list(&self.ids, &references);
        list(&self.solids, &bytes);
        // Every frame, unlike the holes below: every solid has a footprint —
        // an ordinary lid or body is the whole tile, `(0, 255, 0, 255)` — so
        // there is no bit to gate this read on the way `HOLED` gates that one.
        list(&self.footprints, &lighting.occlusion.footprint_bytes());
        // And the other half of the same box, on the same terms: a solid's
        // height has a fraction, and "none" is a byte rather than an absence.
        list(&self.solid_z, &lighting.occlusion.solid_z_bytes());
        // And the holes, only where there are any. What makes skipping this safe
        // rather than a stale read is the `HOLED` bit: it is written into the
        // surface plane above, on this frame, and the shader reads a hole only
        // where it is set — so a frame with no window in it leaves whatever these
        // texels held and nothing looks at them.
        if lighting.occlusion.any_aperture() {
            list(&self.apertures, &lighting.occlusion.aperture_bytes());
        }
    }
}

/// The format of the texture the world is drawn into.
///
/// Every pipeline that draws into that texture — ground, statics, mobiles —
/// must be built with *this* format and never with the surface's. The two are
/// not the same value: a surface may offer `Rgba16Float` first among its
/// non-sRGB formats (an HDR display does), and a pipeline built for it fails
/// validation at `set_pipeline` against a pass whose attachment is this
/// texture. Only the blit and the HUD, which draw to the surface itself, take
/// the surface's format.
pub const WORLD_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Create the texture the world is drawn into, at a camera's render size.
///
/// Here rather than in the caller because the format and the usage are this
/// crate's decision: a texture created without `TEXTURE_BINDING` fails at
/// bind-group time with an error that names neither the blit nor the pass that
/// filled it.
pub fn world_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("world"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // `Rgba8Unorm`, like every other texture here: the world passes write
        // the art's own bytes and this carries them to the surface unconverted.
        format: WORLD_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            // So a test can read the world image back and compare it with what
            // the blit produced, which is the only way to know the blit is a
            // copy at 1:1 rather than merely plausible.
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
