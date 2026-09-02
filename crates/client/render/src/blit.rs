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

/// The uniform block's size: seven header `vec4`s — the sky ambient with the
/// light count, the ground ambient, the occlusion grid's rectangle, which view
/// to draw, the sun's direction and colour, and the compositing opacity — then
/// three per light: where it burns, what colour, and which way it points.
const LIGHTING_BYTES: u64 = (7 + 3 * MAX_LIGHTS as u64) * 16;

/// Where the world image goes on the surface, in physical pixels.
///
/// Not always the whole window: a docked panel shrinks it, which is the same
/// path a resize already takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ViewportRect {
    /// Pixels from the surface's left edge.
    pub x:      u32,
    /// Pixels from its top edge.
    pub y:      u32,
    /// Width in physical pixels.
    pub width:  u32,
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
    pub target:           &'a wgpu::TextureView,
    /// The world image it comes from.
    pub world:            &'a wgpu::TextureView,
    /// What the world passes wrote about each of that image's pixels beside the
    /// picture — see [`crate::gbuffer`]. What the lighting is computed against,
    /// and the reason this pass can tell a wall's lit face from the shadow
    /// behind it.
    pub gbuffer:          &'a crate::gbuffer::Views,
    /// The statics pass's own instance buffer, bound a second time as storage
    /// so a static's fragment can read `instances[id]` back instead of
    /// carrying its `(x, y, z)` on every pixel of its own picture — see
    /// `docs/archive/render/gbuffer.md` decision 2. [`dummy_instances`] when a caller has
    /// none this frame.
    pub face_instances:   &'a wgpu::Buffer,
    /// Server-item rows.  Their ids carry `IDS_DYNAMIC_ITEM`, so immutable map
    /// rows and cached composites never accidentally address this buffer.
    pub item_instances:   &'a wgpu::Buffer,
    /// The same, for the mobiles pass — a separate buffer because mobiles are
    /// a separate `SpriteRenderer` with its own instance list, not a second
    /// user of the statics one.
    pub mobile_instances: &'a wgpu::Buffer,
    /// `docs/archive/render/gbuffer.md` step 4c's mesh-face pass's own row buffer, bound a
    /// second time as storage. A fragment's `Stance::MeshFace` sentinel
    /// (`place.z`'s stance bits) tells this pass to read `mesh_instances[id]`
    /// instead of `face_instances[id]` — a mesh face has no picture, so its
    /// row is not `SpriteQuad`-shaped and lives in its own buffer.
    /// [`dummy_mesh_instances`] when a caller has none this frame.
    pub mesh_instances:   &'a wgpu::Buffer,
    /// The ground pass's own instance buffer, bound a second time as storage —
    /// `docs/archive/render/gbuffer.md` step 7, the ground half of what step 3 did for a
    /// static's tile. A `Kind::Land` pixel's `place.x`/`place.y` is an id into
    /// this, not a tile, the same move and the same reason.
    /// [`dummy_ground_instances`] when a caller has none this frame — every
    /// real frame does, since the ground pass always runs.
    pub ground_instances: &'a wgpu::Buffer,
    /// Which way the scaling goes, and so which sampler is right.
    pub zoom:             Zoom,
    /// The rectangle of `target` the world gets.
    pub rect:             ViewportRect,
}

/// Draws one texture over a rectangle of another.
#[derive(Debug)]
pub struct Blit {
    pipeline:         wgpu::RenderPipeline,
    /// The same deferred shader, but source-over composited instead of clearing
    /// the surface. It is used only for the private cutaway layer.
    cutaway_pipeline: wgpu::RenderPipeline,
    layout:           wgpu::BindGroupLayout,
    /// For magnifying: a texel has to stay a square.
    nearest:          wgpu::Sampler,
    /// For minifying: nearest would sample one texel in four and the ground
    /// would shimmer as the camera walks.
    linear:           wgpu::Sampler,
    /// The frame's lights, rewritten every frame — see [`crate::light`].
    lighting:         wgpu::Buffer,
    /// The cutaway's independent copy of the same uniform block. Both blits
    /// are recorded before one submission; sharing a buffer would make the
    /// second opacity overwrite the first draw's uniform data.
    cutaway_lighting: wgpu::Buffer,
    /// What stands in their way, as one texel a tile — see
    /// [`crate::occlusion`]. Recreated when the frame's grid changes size, which
    /// is a zoom step or a resize and not an ordinary frame; rewritten every
    /// frame, because the camera moves and the grid is relative to it.
    occluders:        wgpu::Texture,
    /// What each of those tiles *is*, over the same rectangle and in the same
    /// order — the sky field today, an aperture and a body's opacity when the
    /// steps that write them land. See [`crate::occlusion::Occlusion::field_bytes`].
    ///
    /// A second texture and not four more channels of the first: the occluder
    /// cell is what a ray walks through cell after cell in a loop, and this is
    /// read once per fragment. `docs/archive/render/lighting_world.md` decides it once, there.
    field:            wgpu::Texture,
    /// What the cells above name — one texel a reference, folded into rows
    /// [`crate::occlusion::LIST_ROW`] wide. See
    /// [`Occlusion::id_bytes`](crate::occlusion::Occlusion::id_bytes).
    ///
    /// The level step 23.1 put between a cell and a solid, and the whole of what
    /// it costs the shader is this one extra `textureLoad`. What it buys is that
    /// a solid is a shape the world holds rather than a tile's property, so the
    /// same box can be referenced by every cell it stands over.
    ids:              wgpu::Texture,
    /// The primitives those references name — one struct a solid, indexed
    /// outright. See
    /// [`Occlusion::primitive_bytes`](crate::occlusion::Occlusion::primitive_bytes).
    ///
    /// **A storage buffer, and that is `docs/render/design_occluders.md`'s D8 replacing
    /// decision 30.5.** Three textures stood here — the solids, their
    /// footprints and their `z` spans, three encodings of one box indexed by one
    /// number — because the ceiling was WebGL2, which has neither compute nor
    /// storage buffers, so a list the shader could index had to be a texture
    /// read with `textureLoad`. Phase 6a settled the ceiling as WebGPU.
    ///
    /// Grown when the frame holds more primitives than it has room for, on its
    /// own terms and not the camera's: the two planes above are the camera's
    /// rectangle, and this is a list whose length is what the camera happens to
    /// be looking at.
    primitives:       wgpu::Buffer,
    /// The hole in each of those primitives, one struct a solid and in the
    /// [`Occlusion::primitive_bytes`](crate::occlusion::Occlusion::primitive_bytes)
    /// order — see
    /// [`Occlusion::aperture_bytes`](crate::occlusion::Occlusion::aperture_bytes).
    ///
    /// **A storage buffer since `docs/render/design_occluders.md`'s S6**, and the last thing
    /// indexed by a `SolidId` to stop being an `Rgba8Uint` plane. Still a list
    /// beside the primitives rather than four more fields of one:
    /// `Occlusion::aperture_bytes` argues why, and the argument is about how
    /// often a hole is read rather than about what a texture can hold.
    ///
    /// Grown with [`Blit::primitives`] and by its count, since the two are
    /// indexed by one number. **Written only when something in the frame has a
    /// hole**: the primitive's own `HOLED` bit is what makes the shader read this
    /// at all, so a frame with no window in it neither lays these bytes out nor
    /// sends them, which is every frame of a real map until step 16 lands.
    apertures:        wgpu::Texture,
    /// The broad phase, as the shader traverses it: the tree's nodes, depth
    /// first, the root first. See
    /// [`Occlusion::node_bytes`](crate::occlusion::Occlusion::node_bytes) and
    /// `docs/render/design_occluders.md`'s S5.
    ///
    /// Grown and never shrunk, like [`Blit::primitives`] and for a stronger
    /// reason than that one has: a traversal ends at the **root's own escape**,
    /// which is this frame's node count, so capacity left over from a larger
    /// frame is not merely unreferenced but unreachable.
    nodes:            wgpu::Buffer,
    /// And the permutation its leaves index into — one `SolidId` a word. See
    /// [`Occlusion::order_bytes`](crate::occlusion::Occlusion::order_bytes).
    ///
    /// A second buffer and not a field of the node: a leaf is a *run* of this,
    /// which is two numbers in the node and however many primitives here, and
    /// folding them together would be the same list held twice.
    order:            wgpu::Buffer,
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
            label:   Some("blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding:    0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Texture {
                        sample_type:    wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled:   false,
                    },
                    count:      None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding:    1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count:      None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding:    2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Buffer {
                        ty:                 wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count:      None,
                },
                // The id plane and the occlusion grid, both integer
                // textures and therefore both unfilterable: there is no sampler
                // for either, and the shader reads exact texels.
                wgpu::BindGroupLayoutEntry {
                    binding:    3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Texture {
                        sample_type:    wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled:   false,
                    },
                    count:      None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding:    4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Texture {
                        sample_type:    wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled:   false,
                    },
                    count:      None,
                },
                // And the field plane over the same rectangle — what a tile *is*
                // rather than what a ray passes through. Unfilterable for the
                // same reason: a sky byte averaged with its neighbour's would be
                // a second blur over the one `Occlusion::blur_sky` already did,
                // at the resolution of the screen instead of of the map.
                wgpu::BindGroupLayoutEntry {
                    binding:    5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Texture {
                        sample_type:    wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled:   false,
                    },
                    count:      None,
                },
                // And the primitives the grid indexes into — the list of
                // decision 30, and one of the three that are not pictures of the
                // camera's rectangle. A storage buffer since
                // `docs/render/design_occluders.md`'s D8; read-only, the same as 9 through 12
                // below.
                wgpu::BindGroupLayoutEntry {
                    binding:    6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Buffer {
                        ty:                 wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count:      None,
                },
                // And the hole in each of those solids, indexed by the same
                // number. This is an exact, unfilterable texture because the
                // device's fragment storage-buffer limit is eight; keeping this
                // four-float record as a texel saves one storage binding without
                // quantising the aperture.
                wgpu::BindGroupLayoutEntry {
                    binding:    7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Texture {
                        sample_type:    wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled:   false,
                    },
                    count:      None,
                },
                // And the references between the two: what a cell counts through
                // is a run of these, and each one names a solid. Step 23.1.
                wgpu::BindGroupLayoutEntry {
                    binding:    8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Texture {
                        sample_type:    wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled:   false,
                    },
                    count:      None,
                },
                // The map statics, server items and mobiles passes' own instance data, each
                // bound a second time as storage — decision 2's `instances[id]`.
                // Read-only: this pass never writes a fragment's own instance
                // back, only looks one up. `docs/archive/render/gbuffer.md` step 3.
                wgpu::BindGroupLayoutEntry {
                    binding:    9,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Buffer {
                        ty:                 wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count:      None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding:    17,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Buffer {
                        ty:                 wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count:      None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding:    10,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Buffer {
                        ty:                 wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count:      None,
                },
                // The mesh-face pass's own row buffer — `docs/archive/render/gbuffer.md`
                // step 4c. Read-only, the same reason 9 and 10 are.
                wgpu::BindGroupLayoutEntry {
                    binding:    11,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Buffer {
                        ty:                 wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count:      None,
                },
                // The ground pass's own instance data, bound a second time as
                // storage — `docs/archive/render/gbuffer.md` step 7, `Kind::Land`'s share of
                // decision 2. Read-only, the same reason 9, 10 and 11 are.
                wgpu::BindGroupLayoutEntry {
                    binding:    12,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Buffer {
                        ty:                 wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count:      None,
                },
                // Where each pixel's fragment is — the G-buffer's second plane,
                // `crate::gbuffer::POSITION_FORMAT`. Unfilterable, and not
                // because `Rgba32Float` happens to be: a filtered position is
                // a point on neither of the two surfaces it was averaged from.
                wgpu::BindGroupLayoutEntry {
                    binding:    13,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Texture {
                        sample_type:    wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled:   false,
                    },
                    count:      None,
                },
                // And which way that pixel's surface looks — the third plane,
                // `crate::gbuffer::NORMAL_FORMAT`, one octahedral word. An
                // integer texture, which cannot be filtered at all, and that is
                // the answer this plane wanted anyway: the average of two unit
                // vectors is not a unit vector, and the average of a wall's
                // normal and the ground's behind it points into the seam
                // between them.
                wgpu::BindGroupLayoutEntry {
                    binding:    14,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Texture {
                        sample_type:    wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled:   false,
                    },
                    count:      None,
                },
                // And the broad phase: the tree's nodes and the permutation its
                // leaves index into, `docs/render/design_occluders.md`'s S5. Read-only storage,
                // the same as every list above — a traversal indexes them and
                // writes nothing.
                wgpu::BindGroupLayoutEntry {
                    binding:    15,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Buffer {
                        ty:                 wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count:      None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding:    16,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty:         wgpu::BindingType::Buffer {
                        ty:                 wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count:      None,
                },
            ],
        });

        // Zeroed at creation, which is *not* the identity — a zero ambient is
        // black — so the first frame writes it before drawing. Every frame
        // does; this only has to be a buffer of the right size to bind.
        let lighting = device.create_buffer(&wgpu::BufferDescriptor {
            label:              Some("lighting"),
            size:               LIGHTING_BYTES,
            usage:              wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cutaway_lighting = device.create_buffer(&wgpu::BufferDescriptor {
            label:              Some("cutaway lighting"),
            size:               LIGHTING_BYTES,
            usage:              wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label:  Some("blit"),
            // `src/shaders/blit.wesl`, compiled to plain WGSL by `build.rs`.
            source: wgpu::ShaderSource::Wgsl(include_str!(concat!(env!("OUT_DIR"), "/blit.wgsl")).into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label:              Some("blit"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size:     0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label:          Some("blit"),
            layout:         Some(&pipeline_layout),
            vertex:         wgpu::VertexState {
                module:              &shader,
                entry_point:         Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                // The corners come from the vertex index. WebGL2 has
                // `gl_VertexID`, so this costs nothing there either.
                buffers:             &[],
            },
            fragment:       Some(wgpu::FragmentState {
                module:              &shader,
                entry_point:         Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets:             &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive:      wgpu::PrimitiveState {
                topology:           wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face:         wgpu::FrontFace::Ccw,
                cull_mode:          None,
                unclipped_depth:    false,
                polygon_mode:       wgpu::PolygonMode::Fill,
                conservative:       false,
            },
            // No depth at all: the world's depth buffer ordered the world, and
            // this draws the result of that as a picture.
            depth_stencil:  None,
            multisample:    wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache:          None,
        });

        let cutaway_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label:          Some("cutaway blit"),
            layout:         Some(&pipeline_layout),
            vertex:         wgpu::VertexState {
                module:              &shader,
                entry_point:         Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers:             &[],
            },
            fragment:       Some(wgpu::FragmentState {
                module:              &shader,
                entry_point:         Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets:             &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive:      wgpu::PrimitiveState {
                topology:           wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face:         wgpu::FrontFace::Ccw,
                cull_mode:          None,
                unclipped_depth:    false,
                polygon_mode:       wgpu::PolygonMode::Fill,
                conservative:       false,
            },
            depth_stencil:  None,
            multisample:    wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache:          None,
        });

        Self {
            pipeline,
            cutaway_pipeline,
            layout,
            nearest: sampler("blit nearest", wgpu::FilterMode::Nearest),
            linear: sampler("blit linear", wgpu::FilterMode::Linear),
            lighting,
            cutaway_lighting,
            // One texel, which is a grid of one open tile: a daylit frame binds
            // it and never reads it, and the first lit frame replaces it. A
            // texture of no size is not a thing wgpu will make.
            occluders: grid_texture(device, "occluders", 1, 1),
            field: grid_texture(device, "field", 1, 1),
            // One row, which is a list of no solids: the grid above says every
            // tile stands nothing, so nothing indexes into it.
            ids: grid_texture(device, "solid ids", crate::occlusion::LIST_ROW, 1),
            // And room for one primitive nothing points at, for the same
            // reason: a buffer of no size is not a thing wgpu will bind. The
            // hole beside it is the same one primitive's, unread — nothing
            // carries the `HOLED` bit that would make the shader look.
            primitives: primitive_buffer(device, 1),
            apertures: aperture_texture(device, 1),
            // One node and one word of permutation: the empty tree, whose root
            // escapes to zero, so a traversal over it ends before its first
            // node. See `Occlusion::node_bytes`.
            nodes: tree_buffer(device, "bvh nodes", crate::occlusion::NODE_BYTES),
            order: tree_buffer(device, "bvh order", 4),
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
        self.render_layer(device, queue, encoder, frame, lighting, 1.0, false);
    }

    /// Deferred-light and source-over a private cutaway layer onto a world the
    /// ordinary [`Self::render`] has already put on the surface.
    ///
    /// `opacity` is supplied by the cutaway policy, once, at the composition
    /// seam. The shader premultiplies its lit output with it, and this pipeline
    /// uses premultiplied source-over blending; there is no alpha literal in a
    /// second sprite shader to drift from the product setting.
    pub fn render_cutaway(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame<'_>,
        lighting: &Lighting,
        opacity: f32,
    ) {
        self.render_layer(device, queue, encoder, frame, lighting, opacity, true);
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the deferred layer needs its encoder, frame, lighting and composition policy"
    )]
    fn render_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame<'_>,
        lighting: &Lighting,
        opacity: f32,
        cutaway: bool,
    ) {
        let Frame {
            target,
            world,
            gbuffer,
            face_instances,
            item_instances,
            mobile_instances,
            mesh_instances,
            ground_instances,
            zoom,
            rect,
        } = frame;
        self.upload_grid(device, queue, lighting);
        self.upload_tree(device, queue, lighting);
        let lighting_buffer = match cutaway {
            true => &self.cutaway_lighting,
            false => &self.lighting,
        };
        queue.write_buffer(lighting_buffer, 0, &lighting_bytes(lighting, opacity));
        // A bind group per call rather than per `Blit`: the world texture is
        // recreated on every resize and every zoom step, and a cached group
        // would be a handle to a texture that is no longer being drawn into.
        let magnifying = zoom.numerator() >= zoom.denominator();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:   Some(if cutaway { "cutaway blit" } else { "blit" }),
            layout:  &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding:  0,
                    resource: wgpu::BindingResource::TextureView(world),
                },
                wgpu::BindGroupEntry {
                    binding:  1,
                    resource: wgpu::BindingResource::Sampler(if magnifying {
                        &self.nearest
                    } else {
                        &self.linear
                    }),
                },
                wgpu::BindGroupEntry {
                    binding:  2,
                    resource: lighting_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding:  3,
                    resource: wgpu::BindingResource::TextureView(&gbuffer.ids),
                },
                wgpu::BindGroupEntry {
                    binding:  4,
                    resource: wgpu::BindingResource::TextureView(
                        &self
                            .occluders
                            .create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding:  5,
                    resource: wgpu::BindingResource::TextureView(
                        &self.field.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding:  6,
                    resource: self.primitives.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding:  7,
                    resource: wgpu::BindingResource::TextureView(
                        &self
                            .apertures
                            .create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding:  8,
                    resource: wgpu::BindingResource::TextureView(
                        &self.ids.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding:  9,
                    resource: face_instances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding:  10,
                    resource: mobile_instances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding:  11,
                    resource: mesh_instances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding:  12,
                    resource: ground_instances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding:  13,
                    resource: wgpu::BindingResource::TextureView(&gbuffer.position),
                },
                wgpu::BindGroupEntry {
                    binding:  14,
                    resource: wgpu::BindingResource::TextureView(&gbuffer.normal),
                },
                wgpu::BindGroupEntry {
                    binding:  15,
                    resource: self.nodes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding:  16,
                    resource: self.order.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding:  17,
                    resource: item_instances.as_entire_binding(),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view:           target,
                depth_slice:    None,
                resolve_target: None,
                ops:            wgpu::Operations {
                    // The opaque picture owns the clear. The transparent layer
                    // loads it and source-overs only its non-empty texels.
                    load:  match cutaway {
                        true => wgpu::LoadOp::Load,
                        false => wgpu::LoadOp::Clear(crate::renderer::CLEAR),
                    },
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

        pass.set_pipeline(match cutaway {
            true => &self.cutaway_pipeline,
            false => &self.pipeline,
        });
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
fn lighting_bytes(lighting: &Lighting, opacity: f32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(LIGHTING_BYTES as usize);
    let count = lighting.lights.len().min(MAX_LIGHTS);

    // `sky`, with the light count in the fourth channel: what a tile with an
    // open column above it gets, before the field says how much of that column
    // is open. See `crate::light::Ambient`.
    for channel in lighting.ambient.sky {
        bytes.extend_from_slice(&channel.to_le_bytes());
    }
    bytes.extend_from_slice(&(count as f32).to_le_bytes());

    // `ground`: the floor every tile gets, roof or no roof. The fourth channel
    // held a stated zero — a `vec3` is padded to four either way and this file's
    // own rule is that a channel is claimed when a reader exists — and one does
    // now: `Lighting::flame_radius`, how big a flame's own sphere is. It rides
    // here rather than in a plane of its own because it is one number a frame,
    // read once per ray bundle, and the header already had the room.
    for channel in lighting.ambient.ground {
        bytes.extend_from_slice(&channel.to_le_bytes());
    }
    bytes.extend_from_slice(&lighting.flame_radius.to_le_bytes());

    // `grid`: where the occlusion texture's corner is on the map, and how big it
    // is. Signed integers, which is what the shader declares that field as —
    // a rectangle may start at a negative tile for a camera near the map's
    // corner, and the walk has to be able to say so.
    let bounds = lighting.occlusion.bounds();
    for value in [bounds.min_x, bounds.min_y, bounds.width(), bounds.height()] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    // `view`: which of the pass's own values to draw instead of the frame, and
    // beside it how many rays a fragment casts at each flame, and beside that
    // whether this is a ghost's frame, and finally which short lighting path is
    // valid: zero for the full path, one for an identity, two for ambient only.
    // Both short paths retain the ID test that leaves the clear background
    // alone, then skip the instance lookup the full path needs. The array that
    // follows still starts on this field's sixteen-byte boundary.
    bytes.extend_from_slice(&(lighting.view as u32).to_le_bytes());
    bytes.extend_from_slice(&lighting.shadow_rays.raw().to_le_bytes());
    bytes.extend_from_slice(&(lighting.dead as u32).to_le_bytes());
    let fast_path = match (lighting.is_identity(), lighting.is_ambient_only()) {
        (true, _) => 1u32,
        (_, true) => 2u32,
        _ => 0u32,
    };
    bytes.extend_from_slice(&fast_path.to_le_bytes());

    // The sun: its direction, then the height above which nothing in this
    // frame's grid can stop it. That height is where a sunbeam's segment *ends* —
    // the sun has no position, so this is what gives its ray a far end to walk to
    // — and over an open street it ends a tile or two out.
    //
    // A frame with no sun writes a direction of zero and an intensity of zero,
    // and the shader tests the intensity: one branch, and a night frame does not
    // pay for a sky it does not have.
    let sun = lighting.sun.unwrap_or(crate::light::Sun {
        toward:    crate::light::TileVec::default(),
        color:     [0.0; 3],
        intensity: 0.0,
    });
    // `axes` and not any arithmetic: this is the wire, and the shader reads the
    // three numbers in tile space exactly as they stand. See
    // [`crate::light::TileVec`] — the newtype is unwrapped at the serialisation
    // boundary and nowhere else.
    for value in sun.toward.axes() {
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

    // The final operation this blit performs. Opaque world rendering writes
    // one; the cutaway variant writes its policy opacity and uses a blending
    // pipeline. The other three words are deliberately reserved as a coherent
    // fourth header vector rather than smuggling a float into an integer view.
    bytes.extend_from_slice(&opacity.to_le_bytes());
    bytes.extend_from_slice(&[0; 12]);

    for light in &lighting.lights[..count] {
        // A fire in the open lights every direction, and says so with an axis of
        // nothing and a rim below every cosine there is: the shader's one test is
        // `cos_half > -1`, so an omnidirectional flame costs a comparison and
        // never a dot product. See `crate::light::Beam`.
        let beam = light.beam.unwrap_or(crate::light::Beam {
            toward:   crate::light::TileVec::default(),
            cos_half: -1.0,
        });
        let toward = beam.toward.axes();
        for value in [
            light.at.x,
            light.at.y,
            light.z,
            light.radius,
            light.color[0],
            light.color[1],
            light.color[2],
            light.intensity,
            toward[0],
            toward[1],
            toward[2],
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
        label:           Some(label),
        size:            wgpu::Extent3d {
            width:                 width.max(1),
            height:                height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count:    1,
        dimension:       wgpu::TextureDimension::D2,
        format:          wgpu::TextureFormat::Rgba8Uint,
        usage:           wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats:    &[],
    })
}

/// Room for `count` primitives — [`Blit::primitives`], and the one place the
/// buffer's size is [`crate::occlusion::PRIMITIVE_BYTES`] times a count.
///
/// At least one, because a buffer of no size is not a thing wgpu will bind and
/// a frame with no occluder in it still binds this. Nothing points at that one
/// primitive: what says a tile stands nothing is its own count in the index.
fn primitive_buffer(device: &wgpu::Device, count: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label:              Some("primitives"),
        size:               (count.max(1) * crate::occlusion::PRIMITIVE_BYTES) as u64,
        usage:              wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Room for the holes in `count` primitives — [`Blit::apertures`].
///
/// The same `count` as [`primitive_buffer`] and never its own, because the two
/// are indexed by one [`SolidId`](crate::occlusion::SolidId): an aperture at an
/// index this buffer does not reach would be a hole read off the end of the
/// list. Sized whether or not the frame has a hole in it, since the bind group
/// needs a resource either way and only the *write* is conditional.
fn aperture_texture(device: &wgpu::Device, count: usize) -> wgpu::Texture {
    let (width, height) = aperture_extent(count, device.limits().max_texture_dimension_2d);
    device.create_texture(&wgpu::TextureDescriptor {
        label:           Some("apertures"),
        size:            wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count:    1,
        dimension:       wgpu::TextureDimension::D2,
        format:          wgpu::TextureFormat::Rgba32Float,
        usage:           wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats:    &[],
    })
}

/// The two-dimensional extent which holds `count` aperture records.
///
/// A [`SolidId`](crate::occlusion::SolidId) is still the record's linear index;
/// the shader folds it into this texture's rows.  Keeping a row no wider than
/// the device's limit matters on maps whose visible occluder list is wider than
/// an older GPU's maximum 2D texture dimension.
fn aperture_extent(count: usize, max_dimension: u32) -> (u32, u32) {
    let width = (count.max(1) as u64).min(u64::from(max_dimension)) as u32;
    let height = (count.max(1) as u64).div_ceil(u64::from(width)) as u32;
    assert!(
        height <= max_dimension,
        "{} aperture records exceed this device's {} by {} texture capacity",
        count,
        max_dimension,
        max_dimension
    );
    (width, height)
}

/// Room for `bytes` of the broad phase — [`Blit::nodes`] and [`Blit::order`].
///
/// One function for both because they are grown by one rule: whatever the frame
/// laid out, never smaller than the buffer already is, and never zero — a buffer
/// of no size is not a thing wgpu will bind, and the empty tree is one node of
/// zeros rather than no node at all.
fn tree_buffer(device: &wgpu::Device, label: &str, bytes: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label:              Some(label),
        size:               bytes.max(4) as u64,
        usage:              wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
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
        label:              Some("blit dummy instances"),
        size:               crate::sprite::SpriteQuad::STRIDE,
        usage:              wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    })
}

/// The same, for [`Frame::mesh_instances`] — a caller with no mesh faces to
/// draw this frame still needs a valid resource in binding 11.
pub fn dummy_mesh_instances(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label:              Some("blit dummy mesh instances"),
        size:               crate::mesh_face::MeshFaceRow::STRIDE,
        usage:              wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    })
}

/// The same, for [`Frame::ground_instances`] — a fixture with no real ground
/// pass still needs a valid resource in binding 12. Real frames never need
/// this: the ground pass always runs and its own instance buffer is always
/// the argument, empty or not.
pub fn dummy_ground_instances(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label:              Some("blit dummy ground instances"),
        size:               crate::ground::GroundQuad::STRIDE,
        usage:              wgpu::BufferUsages::STORAGE,
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
                    offset:         0,
                    bytes_per_row:  Some(width * 4),
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

        // And the lists the grid indexes into. Their lengths are the frame's own
        // counts and not the camera's rectangle, so they are grown on their own
        // terms: a camera that has not moved keeps the same rows, and walking
        // into a city grows them. `id_bytes` pads to a whole row, which is what
        // makes the upload's `bytes_per_row` exact.
        let row = crate::occlusion::LIST_ROW;
        let primitives = lighting.occlusion.primitive_bytes();
        // The two are grown together and by one count, since one `SolidId`
        // indexes both: a hole at an index the aperture buffer does not reach
        // would be read off the end of it.
        if (self.primitives.size() as usize) < primitives.len() {
            self.primitives = primitive_buffer(device, lighting.occlusion.solid_count());
            self.apertures = aperture_texture(device, lighting.occlusion.solid_count());
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
                    offset:         0,
                    bytes_per_row:  Some(row * 4),
                    rows_per_image: Some((bytes.len() / (row as usize * 4)) as u32),
                },
                wgpu::Extent3d {
                    width:                 row,
                    height:                (bytes.len() / (row as usize * 4)) as u32,
                    depth_or_array_layers: 1,
                },
            );
        };
        list(&self.ids, &references);
        // And the primitives, every frame: the whole of a solid's geometry is
        // in these bytes now, so there is nothing here that can be written on
        // one frame and read on another.
        queue.write_buffer(&self.primitives, 0, &primitives);
        // And the holes, only where there are any. What makes skipping this safe
        // rather than a stale read is the `HOLED` bit: it is written into the
        // primitive above, on this frame, and the shader reads a hole only
        // where it is set — so a frame with no window in it leaves whatever these
        // bytes held and nothing looks at them.
        if lighting.occlusion.any_aperture() {
            let bytes = lighting.occlusion.aperture_bytes();
            let record_bytes = crate::occlusion::APERTURE_BYTES;
            let row_bytes = self.apertures.width() as usize * record_bytes;
            // Upload by row: the final row is usually short, and writing it as
            // a full row would require inventing padding that the CPU-side
            // aperture list deliberately does not contain.
            for (row, bytes) in bytes.chunks(row_bytes).enumerate() {
                let width = (bytes.len() / record_bytes) as u32;
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture:   &self.apertures,
                        mip_level: 0,
                        origin:    wgpu::Origin3d {
                            x: 0,
                            y: row as u32,
                            z: 0,
                        },
                        aspect:    wgpu::TextureAspect::All,
                    },
                    bytes,
                    wgpu::TexelCopyBufferLayout {
                        offset:         0,
                        bytes_per_row:  Some(width * record_bytes as u32),
                        rows_per_image: Some(1),
                    },
                    wgpu::Extent3d {
                        width,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
    }

    /// The broad phase on the GPU: the tree's nodes and the permutation its
    /// leaves index into, `docs/render/design_occluders.md`'s S5.
    ///
    /// Every frame, and **before** [`Blit::upload_grid`]'s own early return
    /// rather than after it: a frame with no grid at all still binds this, and a
    /// tree left over from the last frame would be a traversal of geometry the
    /// camera has walked away from. The empty frame's own tree is one node whose
    /// escape is zero, which is a traversal that ends before its first node —
    /// see `Occlusion::node_bytes`.
    fn upload_tree(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, lighting: &Lighting) {
        let nodes = lighting.occlusion.node_bytes();
        if (self.nodes.size() as usize) < nodes.len() {
            self.nodes = tree_buffer(device, "bvh nodes", nodes.len());
        }
        queue.write_buffer(&self.nodes, 0, &nodes);
        let order = lighting.occlusion.order_bytes();
        if (self.order.size() as usize) < order.len() {
            self.order = tree_buffer(device, "bvh order", order.len());
        }
        queue.write_buffer(&self.order, 0, &order);
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
        label:           Some("world"),
        size:            wgpu::Extent3d {
            width:                 width.max(1),
            height:                height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count:    1,
        dimension:       wgpu::TextureDimension::D2,
        // `Rgba8Unorm`, like every other texture here: the world passes write
        // the art's own bytes and this carries them to the surface unconverted.
        format:          WORLD_FORMAT,
        usage:           wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            // So a test can read the world image back and compare it with what
            // the blit produced, which is the only way to know the blit is a
            // copy at 1:1 rather than merely plausible.
            | wgpu::TextureUsages::COPY_SRC,
        view_formats:    &[],
    })
}

#[cfg(test)]
mod aperture_texture_tests {
    use super::aperture_extent;

    #[test]
    fn folds_a_list_past_the_device_row_limit_into_the_next_row() {
        assert_eq!(aperture_extent(10_018, 8_192), (8_192, 2));
    }

    #[test]
    fn keeps_a_small_list_on_one_row() {
        assert_eq!(aperture_extent(7, 8_192), (7, 1));
    }
}
