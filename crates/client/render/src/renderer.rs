//! The ground pass, on the GPU.
//!
//! Given a device somebody else created and a view somebody else owns, this
//! uploads an atlas once and then draws one instanced quad per visible tile.
//! It never asks for an adapter and never presents: a surface belongs to the
//! application, and a test has no surface at all.

use crate::atlas::{LandAtlas, StaticAtlas, TexmapAtlas};

use crate::camera::{Projection, TILE_HEIGHT, TILE_WIDTH, Z_STEP};
use crate::ground::GroundQuad;
use crate::hue::HueRamp;
use crate::mesh_face::{MeshFaceRow, MeshFaceVertex};
use crate::sprite::SpriteQuad;

/// What an untouched pixel is left as.
///
/// Fully transparent, and the fragment shader always writes `a = 1.0`. That
/// makes "was anything drawn here" a question about one byte, with no colour a
/// tile could coincidentally match — which is the difference between a frame
/// test that measures coverage and one that hopes.
pub const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

/// Bytes of the ground pass's uniform block: the target's size, the land
/// sprite's, the height step, the scale, and the origin — eight floats, which is
/// already a multiple of the sixteen a uniform block is rounded up to.
const UNIFORM_BYTES: u64 = 32;

/// The unit quad, as a triangle strip: (0,0) (1,0) (0,1) (1,1).
pub(crate) const QUAD: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];

/// How many quads the instance buffer starts out able to hold.
const INITIAL_QUADS: u64 = 4096;

/// The depth format every pass here uses.
///
/// `Depth24Plus` and not `Depth32Float`: 24 bits is what WebGL2 guarantees, and
/// the browser is a target. It resolves about 6e-8, where one step of the
/// ordering in [`crate::depth`] is 1e-6 — a margin of more than ten, which that
/// module asserts rather than assumes.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

/// Where a frame is being drawn: the views, and the size they actually are.
///
/// They travel together because the shader turns viewport pixels into clip
/// space and needs the size to do it. Passing them separately invites a resize
/// that updates one and not the other, which draws a correct frame at the wrong
/// scale — and looks like a projection bug.
#[derive(Clone, Copy, Debug)]
pub struct Target<'a> {
    /// What to draw into.
    pub view: &'a wgpu::TextureView,
    /// The depth buffer both passes share.
    ///
    /// Shared on purpose: it is the only thing that makes the ground pass and
    /// the statics pass agree about what covers what. A pass with its own
    /// buffer would be back to "everything drawn later wins", which puts every
    /// wall in front of every hill.
    pub depth: &'a wgpu::TextureView,
    /// Where every pass writes which tile its pixels came from.
    ///
    /// Shared for the same reason the depth buffer is, and filled by the same
    /// draws: it is one answer per pixel about one frame, and a pass that wrote
    /// its own would leave the lighting reading the ground's tile under a wall's
    /// picture. See [`crate::place`].
    pub place: &'a wgpu::TextureView,
    /// Its width in real pixels.
    pub width: u32,
    /// Its height in real pixels.
    pub height: u32,
    /// How the world's own virtual pixels land on those real ones — see
    /// [`crate::camera::Projection`].
    ///
    /// A projection and not a zoom, because this crate's passes are not told
    /// what a zoom is: they are told where the middle of what they are drawing
    /// goes and how big a virtual pixel is, and those two answers are the same
    /// shape whether the target is the surface or the offscreen image.
    pub projection: Projection,
}

impl<'a> Target<'a> {
    /// The whole of an image of this size, drawn 1:1 with the eye in the middle.
    ///
    /// What a frame test wants — it renders into a texture it owns and compares
    /// the result against the art, where a magnification would be a resampling
    /// step between the two — and what the minifying path hands the passes, for
    /// the same reason in a different order: there the scaling is the blit's.
    pub fn whole(
        view: &'a wgpu::TextureView,
        depth: &'a wgpu::TextureView,
        place: &'a wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            view,
            depth,
            place,
            width,
            height,
            projection: Projection::one_to_one(width, height),
        }
    }
}

/// Create the depth texture a [`Target`] needs, at a given size.
///
/// Here rather than in the caller because the format is this crate's decision
/// and a texture created with another one fails at pipeline-bind time with an
/// error that names neither side.
pub fn depth_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

/// The depth-stencil state every pass here shares.
///
/// Nearer is smaller — see [`crate::depth`] — so an untouched buffer at 1.0
/// means "nothing drawn yet", and writing is on so every quad that passes the
/// test also records itself for the next one.
///
/// `LessEqual` rather than `Less`, and that is the *tie-break*, not a rounding
/// allowance. Two things at one depth are two things the client puts on one
/// tile at one `PriorityZ`, and it draws them in the order it inserted them:
/// the land tile first, then the map's statics in file order, then whatever
/// arrived from the server. `Chunk.AddGameObject` is where that is decided, and
/// the whole of it here is `LessEqual` plus the order the passes and their
/// quads are already in — later drawn wins, so ground gives way to the
/// flagstone lying on it, and the flagstone gives way to the body standing on
/// the flagstone. Under `Less` every one of those ties resolved backwards, and
/// silently: the depths were right and the first writer kept the pixel.
/// The second colour target every world pass writes: which tile a pixel is.
///
/// Shared by the ground and the sprite pipelines because it is one attachment
/// and one format — see [`crate::place`]. No blending, like the picture beside
/// it: a place is an identity, and averaging two of them names a third tile that
/// nothing was drawn on.
pub(crate) const PLACE_TARGET: wgpu::ColorTargetState = wgpu::ColorTargetState {
    format: crate::place::FORMAT,
    blend: None,
    write_mask: wgpu::ColorWrites::ALL,
};

pub(crate) fn depth_state() -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(true),
        depth_compare: Some(wgpu::CompareFunction::LessEqual),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

/// Draws ground.
#[derive(Debug)]
pub struct GroundRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    quad: wgpu::Buffer,
    instances: wgpu::Buffer,
    /// Quads the instance buffer can hold before it has to be replaced.
    capacity: u64,
    /// The two atlas textures, kept rather than only viewed.
    ///
    /// A view is all a bind group needs, and holding the texture as well is what
    /// lets an atlas *grow* into the one already bound — see
    /// [`GroundRenderer::upload_changes`]. Dropping them here meant a new
    /// graphic at the edge of the view had to build a whole new renderer, which
    /// recreates the pipeline behind it for two sprites' worth of pixels.
    land_texture: wgpu::Texture,
    texmap_texture: wgpu::Texture,
}

impl GroundRenderer {
    /// Upload an atlas and build the pipeline for a target of `format`.
    ///
    /// `format` should be a non-sRGB one. The crate's colour rule is that a
    /// pixel in the art is the pixel in the frame, and an sRGB target silently
    /// breaks it — see the crate docs.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        atlas: &LandAtlas,
        texmaps: &TexmapAtlas,
    ) -> Self {
        // Two atlases of the same size, uploaded the same way: the flat art and
        // the textures a slope is stretched over. They stay apart because their
        // grids do — 44 pixels against 64 — and not because the shader could not
        // sample one texture.
        let land_texture = upload(
            device,
            queue,
            "land atlas",
            LandAtlas::side(),
            LandAtlas::side(),
            atlas.pixels(),
        );
        let texmap_texture = upload(
            device,
            queue,
            "texmap atlas",
            TexmapAtlas::side(),
            TexmapAtlas::side(),
            texmaps.pixels(),
        );
        let view = land_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let texmap_view = texmap_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Nearest, and clamped. UO art is pixel art on an exact grid: filtering
        // it would sample a neighbouring tile across the atlas seam, which shows
        // up as a one-pixel fringe along every diamond.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("land atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport"),
            size: UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ground"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
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
                        view_dimension: wgpu::TextureViewDimension::D2,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ground"),
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&texmap_view),
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ground"),
            // `src/shaders/ground.wesl`, compiled to plain WGSL by `build.rs`
            // — the pilot for `docs/lighting_raymarch.md`'s shared `place`
            // format module.
            source: wgpu::ShaderSource::Wgsl(include_str!(concat!(env!("OUT_DIR"), "/ground.wgsl")).into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ground"),
            bind_group_layouts: &[Some(&layout)],
            // No immediate data: everything per-frame travels in the uniform
            // block, which WebGL2 supports and push constants do not.
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ground"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[
                    // The unit quad.
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        }],
                    }),
                    // One instance per tile. The layout is asserted in
                    // `GroundQuad::write`'s test, which is the only thing that
                    // links this to the shader's `@location`s.
                    //
                    // Five attributes and not six: `docs/gbuffer.md` step 7
                    // replaced the tile — `GroundQuad::write`'s last two words —
                    // with `@builtin(instance_index)`, so `ground.wgsl`'s vertex
                    // stage no longer fetches them. The bytes are still in the
                    // buffer, at the same offset they always were: this pipeline
                    // simply stops declaring an attribute for them, and
                    // `blit.wgsl`/`select.wgsl` read them back through the same
                    // buffer bound a second time as storage — see
                    // `GroundRenderer::instances_buffer`.
                    Some(wgpu::VertexBufferLayout {
                        array_stride: GroundQuad::STRIDE,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 0,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 8,
                                shader_location: 2,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 24,
                                shader_location: 3,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 40,
                                shader_location: 4,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32,
                                offset: 56,
                                shader_location: 5,
                            },
                        ],
                    }),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format,
                        // No blending: the shader discards transparent texels,
                        // so every fragment that survives is opaque.
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    // And the same fragments say where in the world they are.
                    Some(PLACE_TARGET),
                ],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                // Quads are emitted in one winding and never seen from behind.
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(depth_state()),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let quad = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("unit quad"),
            size: std::mem::size_of_val(&QUAD) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut quad_bytes = Vec::with_capacity(QUAD.len() * 4);
        for value in QUAD {
            quad_bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&quad, 0, &quad_bytes);

        let instances = new_instance_buffer(device, INITIAL_QUADS);

        Self {
            pipeline,
            bind_group,
            uniforms,
            quad,
            instances,
            capacity: INITIAL_QUADS,
            land_texture,
            texmap_texture,
        }
    }

    /// Send whatever the two atlases have grown by since this was last asked.
    ///
    /// The pair travels together because a ground quad asks both the same
    /// question: a land graphic packed into one and not yet uploaded from the
    /// other is a slope textured with somebody else's terrain for a frame.
    ///
    /// Nothing to upload is the ordinary frame — a camera standing still
    /// introduces no graphics — and a growth is a band of rows rather than the
    /// 16MB texture.
    pub fn upload_changes(&self, queue: &wgpu::Queue, land: &mut LandAtlas, texmaps: &mut TexmapAtlas) {
        if let Some(rows) = land.take_dirty() {
            write_rows(queue, &self.land_texture, land.pixels(), rows);
        }
        if let Some(rows) = texmaps.take_dirty() {
            write_rows(queue, &self.texmap_texture, texmaps.pixels(), rows);
        }
    }

    /// This pass's own instance buffer, as `blit.wgsl`/`select.wgsl` need it:
    /// bound a second time, as storage, so `ground_instances[id]` can read a
    /// fragment's own tile back instead of carrying it on every pixel of its
    /// own picture. See `docs/gbuffer.md` decision 2 and step 7.
    pub fn instances_buffer(&self) -> &wgpu::Buffer {
        &self.instances
    }

    /// Draw `quads` into `target`, clearing it first.
    ///
    /// The quads carry viewport coordinates, which the shader turns into clip
    /// space using the target's size.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: Target<'_>,
        quads: &[GroundQuad],
    ) {
        let mut uniform_bytes = Vec::with_capacity(UNIFORM_BYTES as usize);
        let projection = target.projection;
        for value in [
            target.width as f32,
            target.height as f32,
            TILE_WIDTH as f32,
            TILE_HEIGHT as f32,
            Z_STEP as f32,
            projection.scale,
            projection.origin.x,
            projection.origin.y,
        ] {
            uniform_bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.uniforms, 0, &uniform_bytes);

        if quads.len() as u64 > self.capacity {
            // Grow in powers of two rather than to the exact size: the camera
            // moves every frame and the count wobbles with it.
            self.capacity = (quads.len() as u64).next_power_of_two();
            self.instances = new_instance_buffer(device, self.capacity);
        }
        let mut instance_bytes = Vec::with_capacity(quads.len() * GroundQuad::STRIDE as usize);
        for quad in quads {
            quad.write(&mut instance_bytes);
        }
        if !instance_bytes.is_empty() {
            queue.write_buffer(&self.instances, 0, &instance_bytes);
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ground"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                // Cleared with the picture and by the same pass, for the same
                // reason the depth buffer is: this is the frame's first pass,
                // and what it leaves is what the passes after it load.
                Some(wgpu::RenderPassColorAttachment {
                    view: target.place,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(crate::place::CLEAR),
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ],
            // Cleared to the far plane here, because this is the frame's first
            // pass. Whatever runs after it loads what this left behind, which
            // is how one ordering spans two passes.
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: target.depth,
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

        if quads.is_empty() {
            // The pass still runs: clearing is the frame when there is nothing
            // to draw, and skipping it would leave the previous one on screen.
            return;
        }

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.set_vertex_buffer(1, self.instances.slice(..));
        pass.draw(0..4, 0..quads.len() as u32);
    }
}

/// The side of every sprite atlas this pass can bind.
///
/// One number rather than a parameter: all three atlases here are the same
/// square, because the ceiling is WebGL2's guaranteed `MAX_TEXTURE_SIZE` and
/// not a choice any of them makes.
pub const SPRITE_ATLAS_SIDE: u32 = StaticAtlas::side();

/// Draws sprites — statics or mobiles — into whatever the ground pass left
/// behind.
///
/// Its own pipeline rather than a mode of [`GroundRenderer`]: the two share
/// nothing but the projection — different atlas, different quad shape,
/// different instance layout — and the one thing they do have to agree on, the
/// ordering, is a number the CPU computes for both.
#[derive(Debug)]
pub struct SpriteRenderer {
    pipeline: wgpu::RenderPipeline,
    /// The same sprites as shapes rather than pictures — see
    /// [`SpriteRenderer::render_mask`] and [`crate::outline`].
    ///
    /// Here rather than in a renderer of its own because a silhouette is drawn
    /// from *this* pass's atlas, sampler, uniform block and unit quad. A
    /// separate type would have to be handed all four, and the one thing it
    /// must not get wrong — that the silhouette lands exactly where the picture
    /// did — is guaranteed by sharing them.
    mask_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    quad: wgpu::Buffer,
    instances: wgpu::Buffer,
    /// Quads the instance buffer can hold before it has to be replaced.
    capacity: u64,
    /// The silhouette pass's own instances, kept apart from `instances`
    /// because the two lists are drawn in the same frame and one is a handful
    /// of quads while the other is the whole screen.
    mask_instances: wgpu::Buffer,
    /// One `u32` per instance of `mask_instances`: which *ring* that quad
    /// belongs to.
    ///
    /// A buffer and not the instance index, because a ring is not a sprite. A
    /// creature is a body and everything it is wearing — several quads that
    /// must come out under one id, or the ring pass finds a boundary between
    /// the tunic and the arm under it and draws an edge along every seam. See
    /// [`SpriteRenderer::render_mask`].
    mask_rings: wgpu::Buffer,
    mask_capacity: u64,
    /// The atlas texture, kept so that it can be grown into rather than
    /// replaced. See [`SpriteRenderer::upload_rows`].
    atlas_texture: wgpu::Texture,
}

impl SpriteRenderer {
    /// Upload an atlas and build the pipeline for a target of `format`.
    ///
    /// `pixels` is any square RGBA8 atlas of [`SPRITE_ATLAS_SIDE`] — the
    /// statics' or the mobiles' — because this pass does not care which: it
    /// draws rectangles of somebody else's picture, and where they go is the
    /// caller's arithmetic. Two atlases therefore mean two of these rather than
    /// one that switches, since a draw call binds one texture either way.
    ///
    /// `format` should be a non-sRGB one, for the reason in the crate docs.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        pixels: &[u8],
        hue_ramp: &HueRamp,
    ) -> Self {
        let atlas_texture = upload(
            device,
            queue,
            "sprite atlas",
            SPRITE_ATLAS_SIDE,
            SPRITE_ATLAS_SIDE,
            pixels,
        );
        let view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let ramp_texture = upload(
            device,
            queue,
            "hue ramp",
            HueRamp::width(),
            hue_ramp.height(),
            hue_ramp.pixels(),
        );
        let ramp_view = ramp_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Nearest and clamped, exactly as the ground: UO art is pixel art, and
        // filtering it samples whatever was packed next door. The ramp is looked
        // up the same way — `statics.wgsl` picks one exact texel, and filtering
        // between two hues would blend colours nothing in `hues.mul` asked for.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("static atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("statics viewport"),
            size: STATIC_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("statics"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
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
                        view_dimension: wgpu::TextureViewDimension::D2,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("statics"),
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&ramp_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("statics"),
            // `src/shaders/statics.wesl`, compiled to plain WGSL by `build.rs`.
            source: wgpu::ShaderSource::Wgsl(include_str!(concat!(env!("OUT_DIR"), "/statics.wgsl")).into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("statics"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("statics"),
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
                    // One instance per static. The layout is asserted in
                    // `SpriteQuad::write`'s test, which is the only thing
                    // linking this to the shader's `@location`s.
                    Some(wgpu::VertexBufferLayout {
                        array_stride: SpriteQuad::STRIDE,
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
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32,
                                offset: 32,
                                shader_location: 4,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Uint32,
                                offset: 36,
                                shader_location: 5,
                            },
                            // The tile and height, as
                            // `crate::place::Place::packed` wrote them.
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Uint32x2,
                                offset: 40,
                                shader_location: 6,
                            },
                            // A corner static's paired shadow row — see
                            // `crate::sprite::SpriteQuad::twin` and
                            // `docs/gbuffer.md` step 4. Only `statics.wgsl`
                            // declares a vertex input at this location; the
                            // mobile pass shares this same layout and simply
                            // never reads it.
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Uint32,
                                offset: 48,
                                shader_location: 7,
                            },
                        ],
                    }),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format,
                        // No blending: the shader discards transparent texels,
                        // so every fragment that survives is opaque.
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    // And the same fragments say where in the world they are.
                    Some(PLACE_TARGET),
                ],
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
            depth_stencil: Some(depth_state()),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // The same sprites as shapes. Built here, where the bind group layout
        // and the shared uniform block are still in scope, and from a cut-down
        // vertex layout: a silhouette has no hue and no place, so those two
        // attributes are simply not declared. A vertex buffer may carry more
        // than a pipeline reads.
        let mask_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("silhouette"),
            source: wgpu::ShaderSource::Wgsl(include_str!("silhouette.wgsl").into()),
        });
        let mask_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("silhouette"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &mask_shader,
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
                        array_stride: SpriteQuad::STRIDE,
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
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32,
                                offset: 32,
                                shader_location: 4,
                            },
                        ],
                    }),
                    // The ring each instance belongs to, in its own buffer:
                    // several quads of one creature share an id, so it cannot
                    // be the instance index and there is no room for it in
                    // `SpriteQuad` — which is the picture passes' layout and
                    // has no business carrying a highlight's identity.
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 4,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 0,
                            shader_location: 5,
                        }],
                    }),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &mask_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    // Never `format`: the mask is ids, not colour — see
                    // [`crate::outline::MASK_FORMAT`].
                    format: crate::outline::MASK_FORMAT,
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
            // Tested against the world's depth and *not* written: the ordering
            // was settled by the passes that drew the picture, and a silhouette
            // that wrote depth would settle it again, differently, for whatever
            // pass came after. See [`SpriteRenderer::render_mask`].
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let quad = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("unit quad"),
            size: std::mem::size_of_val(&QUAD) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut quad_bytes = Vec::with_capacity(QUAD.len() * 4);
        for value in QUAD {
            quad_bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&quad, 0, &quad_bytes);

        let instances = new_static_instance_buffer(device, INITIAL_QUADS);
        // One quad is the frame that has an item under the cursor and the
        // common case; the buffer grows the same way the big one does if a
        // caller ever outlines more.
        let mask_capacity = 8;
        let mask_instances = new_static_instance_buffer(device, mask_capacity);
        let mask_rings = new_ring_buffer(device, mask_capacity);

        Self {
            pipeline,
            mask_pipeline,
            bind_group,
            uniforms,
            quad,
            instances,
            capacity: INITIAL_QUADS,
            mask_instances,
            mask_rings,
            mask_capacity,
            atlas_texture,
        }
    }

    /// Send a band of an atlas that has grown since it was uploaded.
    ///
    /// `pixels` is the whole atlas and `rows` is what
    /// [`StaticAtlas::take_dirty`](crate::atlas::StaticAtlas::take_dirty) — or
    /// the animation atlas's — just handed back. Untyped for the same reason
    /// [`SpriteRenderer::new`] is: this pass draws rectangles of somebody else's
    /// picture and does not care which atlas they came from. The pairing is the
    /// caller's, and taking the band is what makes it hard to get wrong — a
    /// range only exists because something was written.
    pub fn upload_rows(&self, queue: &wgpu::Queue, pixels: &[u8], rows: std::ops::Range<u32>) {
        write_rows(queue, &self.atlas_texture, pixels, rows);
    }

    /// This pass's own instance buffer, as `blit.wgsl` needs it: bound a
    /// second time, as storage, so `instances[id]` can read a fragment's own
    /// `SpriteQuad` back instead of decoding it from the `place` attachment.
    /// See `docs/gbuffer.md` decision 2 and step 3.
    pub fn instances_buffer(&self) -> &wgpu::Buffer {
        &self.instances
    }

    /// Draw `quads` into `target`, keeping what is already there.
    ///
    /// Loads rather than clears, colour and depth alike: this runs after the
    /// ground pass and both of the ground's results are what it is drawing
    /// against. A pass that cleared here would erase the world and then test
    /// against a depth buffer describing it.
    ///
    /// `drawn` caps how many of `quads` the draw call actually asks for —
    /// `None` draws all of them, same as before this parameter existed. A
    /// corner static's shadow row (`crate::sprite::split_corners`,
    /// `docs/gbuffer.md` step 4) is uploaded here so `instances_buffer()`
    /// can address it by id, but has no picture of its own to rasterise: it
    /// rides at the tail of `quads`, past `drawn`, and the pass never draws
    /// it.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: Target<'_>,
        quads: &[SpriteQuad],
        drawn: Option<u32>,
    ) {
        let mut uniform_bytes = Vec::with_capacity(STATIC_UNIFORM_BYTES as usize);
        let projection = target.projection;
        for value in [
            target.width as f32,
            target.height as f32,
            projection.scale,
            0.0,
            projection.origin.x,
            projection.origin.y,
            0.0,
            0.0,
        ] {
            uniform_bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.uniforms, 0, &uniform_bytes);

        if quads.len() as u64 > self.capacity {
            self.capacity = (quads.len() as u64).next_power_of_two();
            self.instances = new_static_instance_buffer(device, self.capacity);
        }
        let mut instance_bytes = Vec::with_capacity(quads.len() * SpriteQuad::STRIDE as usize);
        for quad in quads {
            quad.write(&mut instance_bytes);
        }
        if instance_bytes.is_empty() {
            // Nothing to draw and nothing to clear: unlike the ground pass,
            // this one owns no pixels of its own, so an empty pass would only
            // cost a submission.
            return;
        }
        queue.write_buffer(&self.instances, 0, &instance_bytes);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("statics"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                }),
                // Loaded, not cleared: the ground pass wrote the tiles under
                // these sprites, and a wall keeps only the pixels it drew.
                Some(wgpu::RenderPassColorAttachment {
                    view: target.place,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: target.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.set_vertex_buffer(1, self.instances.slice(..));
        pass.draw(0..4, 0..drawn.unwrap_or(quads.len() as u32));
    }

    /// Draw `groups` into an outline mask as shapes, **one id per group**.
    ///
    /// `groups` is *only* what is to be outlined, not the frame — the id a
    /// silhouette is written in is its group's position in this list plus one,
    /// so the list is the numbering and nothing on [`SpriteQuad`] carries it.
    /// Past [`MAX_OUTLINED`](crate::outline::MAX_OUTLINED) the tail is dropped
    /// rather than wrapped: an id that wrapped to another object's would ring
    /// the two as one, silently.
    ///
    /// **A group and not a quad, because a ring is not a sprite.** An item is
    /// one quad and one ring, but a creature is a body and every layer it is
    /// wearing, and those have to come out under one id: the ring pass draws an
    /// edge wherever two *different* ids meet, so a tunic numbered apart from
    /// the arm inside it is ringed along every seam of the clothing. One quad
    /// per group is the item case, written `&[&[quad]]` and costing nothing.
    ///
    /// `target`'s `view` is ignored and `mask` is drawn into instead; everything
    /// else about it — the size, the projection, the depth buffer — is the
    /// frame's own, and has to be, or the silhouette lands somewhere the picture
    /// did not. The mask is cleared here, so this is the pass that owns it.
    ///
    /// Must run **after** the passes that drew the picture and **before**
    /// anything that writes depth over it (the text pass does): the depth test
    /// is what keeps a barrel behind a wall out of the mask, and it can only
    /// answer that once the wall is in the buffer.
    pub fn render_mask(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: Target<'_>,
        mask: &wgpu::TextureView,
        groups: &[&[SpriteQuad]],
    ) {
        // The same uniform block the picture pass writes, written again rather
        // than assumed: this is called with the frame's own target and the two
        // calls are only adjacent by convention.
        let mut uniform_bytes = Vec::with_capacity(STATIC_UNIFORM_BYTES as usize);
        let projection = target.projection;
        for value in [
            target.width as f32,
            target.height as f32,
            projection.scale,
            0.0,
            projection.origin.x,
            projection.origin.y,
            0.0,
            0.0,
        ] {
            uniform_bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.uniforms, 0, &uniform_bytes);

        let groups = &groups[..groups.len().min(crate::outline::MAX_OUTLINED)];
        let instances: usize = groups.iter().map(|group| group.len()).sum();
        if instances as u64 > self.mask_capacity {
            self.mask_capacity = (instances as u64).next_power_of_two();
            self.mask_instances = new_static_instance_buffer(device, self.mask_capacity);
            self.mask_rings = new_ring_buffer(device, self.mask_capacity);
        }
        let mut instance_bytes = Vec::with_capacity(instances * SpriteQuad::STRIDE as usize);
        let mut ring_bytes = Vec::with_capacity(instances * 4);
        for (index, group) in groups.iter().enumerate() {
            // Zero is "nothing here" in the mask, so the first group is 1 —
            // `silhouette.wgsl` reads this number and does not add to it.
            let ring = index as u32 + 1;
            for quad in *group {
                quad.write(&mut instance_bytes);
                ring_bytes.extend_from_slice(&ring.to_le_bytes());
            }
        }
        if !instance_bytes.is_empty() {
            queue.write_buffer(&self.mask_instances, 0, &instance_bytes);
            queue.write_buffer(&self.mask_rings, 0, &ring_bytes);
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("silhouette"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: mask,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Zero is "nothing here", so clearing is what makes last
                    // frame's ring go away when the cursor moves off.
                    load: wgpu::LoadOp::Clear(CLEAR),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: target.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        if instances == 0 {
            // The clear still happened, which is the frame with nothing lit.
            return;
        }

        pass.set_pipeline(&self.mask_pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.set_vertex_buffer(1, self.mask_instances.slice(..));
        pass.set_vertex_buffer(2, self.mask_rings.slice(..));
        pass.draw(0..4, 0..instances as u32);
    }
}

/// Draws `docs/gbuffer.md` step 4c's mesh-geometry pass: depth and place
/// only, for a [`crate::mesh::Mesh`]'s faces — never colour, because the
/// enclosing static's own billboard sprite already drew the picture, and
/// this pass exists only to give that same static's pixels a more honest
/// per-face normal than one blended stance could.
///
/// No atlas, no sampler, no texture at all — unlike [`SpriteRenderer`], it
/// has nothing to upload at construction and nothing an atlas repack would
/// ever invalidate, so it is built once, at window setup, and never rebuilt.
#[derive(Debug)]
pub struct MeshFaceRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    vertices: wgpu::Buffer,
    capacity: u64,
    rows: wgpu::Buffer,
    rows_capacity: u64,
}

impl MeshFaceRenderer {
    /// Build the pipeline. Takes no atlas and no colour format: this pass
    /// writes only [`PLACE_TARGET`] and the shared depth buffer.
    pub fn new(device: &wgpu::Device) -> Self {
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh face viewport"),
            size: STATIC_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh face"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh face"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh face"),
            // `src/shaders/mesh_face.wesl`, compiled to plain WGSL by `build.rs`.
            source: wgpu::ShaderSource::Wgsl(
                include_str!(concat!(env!("OUT_DIR"), "/mesh_face.wgsl")).into(),
            ),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh face"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh face"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                // One raw vertex at a time — not a unit quad plus per-instance
                // data: a face's true screen shape is an arbitrary projected
                // quadrilateral, not an axis-aligned rectangle a shader could
                // reconstruct from an origin and a size.
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: MeshFaceVertex::STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 8,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: 20,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 24,
                            shader_location: 3,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 28,
                            shader_location: 4,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(PLACE_TARGET)],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(depth_state()),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group,
            uniforms,
            vertices: new_mesh_vertex_buffer(device, INITIAL_MESH_FACES * 6),
            capacity: INITIAL_MESH_FACES * 6,
            rows: new_mesh_row_buffer(device, INITIAL_MESH_FACES),
            rows_capacity: INITIAL_MESH_FACES,
        }
    }

    /// This pass's own row buffer, as `blit.wgsl` needs it: bound a second
    /// time, as storage, so `mesh_instances[id]` can read a face's tile and
    /// real stance back once a fragment's `Stance::MeshFace` sentinel says to
    /// look here instead of `face_instances`.
    pub fn rows_buffer(&self) -> &wgpu::Buffer {
        &self.rows
    }

    /// Draw `vertices` (`crate::mesh::Face::fan`'s output, six per face) into
    /// `target`'s place and depth, keeping its colour untouched.
    ///
    /// Loads rather than clears: this runs after the statics pass, into the
    /// same static's own pixels, so its depth test can only tie or improve on
    /// what the billboard sprite already wrote there.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: Target<'_>,
        vertices: &[MeshFaceVertex],
        rows: &[MeshFaceRow],
    ) {
        let mut uniform_bytes = Vec::with_capacity(STATIC_UNIFORM_BYTES as usize);
        let projection = target.projection;
        for value in [
            target.width as f32,
            target.height as f32,
            projection.scale,
            0.0,
            projection.origin.x,
            projection.origin.y,
            0.0,
            0.0,
        ] {
            uniform_bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.uniforms, 0, &uniform_bytes);

        if rows.len() as u64 > self.rows_capacity {
            self.rows_capacity = (rows.len() as u64).next_power_of_two();
            self.rows = new_mesh_row_buffer(device, self.rows_capacity);
        }
        let mut row_bytes = Vec::with_capacity(rows.len() * MeshFaceRow::STRIDE as usize);
        for row in rows {
            row.write(&mut row_bytes);
        }
        if !row_bytes.is_empty() {
            queue.write_buffer(&self.rows, 0, &row_bytes);
        }

        if vertices.is_empty() {
            // Nothing to draw and nothing to clear: this pass owns no pixels
            // of its own, the same reason `SpriteRenderer::render` returns
            // early here.
            return;
        }
        if vertices.len() as u64 > self.capacity {
            self.capacity = (vertices.len() as u64).next_power_of_two();
            self.vertices = new_mesh_vertex_buffer(device, self.capacity);
        }
        let mut vertex_bytes = Vec::with_capacity(vertices.len() * MeshFaceVertex::STRIDE as usize);
        for vertex in vertices {
            vertex.write(&mut vertex_bytes);
        }
        queue.write_buffer(&self.vertices, 0, &vertex_bytes);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mesh face"),
            // No colour target at all: `target.view` already carries the
            // billboard sprite's own picture, and this pass has nothing to
            // add to it — only `target.place` and the shared depth buffer.
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.place,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: target.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..vertices.len() as u32, 0..1);
    }
}

/// How many mesh faces the row and vertex buffers start out able to hold —
/// unmeasured against a real frame, the same honestly-unmeasured state
/// `INITIAL_QUADS`'s own doc admits it is in: climbable statics are a small,
/// bounded class next to ordinary ones, and both buffers grow the same
/// power-of-two-on-demand way `SpriteRenderer`'s own instance buffer does.
const INITIAL_MESH_FACES: u64 = 64;

fn new_mesh_vertex_buffer(device: &wgpu::Device, vertices: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mesh face vertices"),
        size: vertices * MeshFaceVertex::STRIDE,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn new_mesh_row_buffer(device: &wgpu::Device, rows: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mesh face rows"),
        size: rows * MeshFaceRow::STRIDE,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Bytes of the sprite pass's uniform block: the target's size, the scale, the
/// origin, and the padding a uniform block's size is rounded up to.
const STATIC_UNIFORM_BYTES: u64 = 32;

/// The silhouette pass's per-instance ring ids, at the same capacity as the
/// quads they belong to — the two are written together and grow together.
fn new_ring_buffer(device: &wgpu::Device, quads: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("silhouette rings"),
        size: quads * 4,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

// `STORAGE` alongside `VERTEX`: this buffer is what `docs/gbuffer.md`'s
// decision 2 means by "the same memory bound a second way" — `blit.wgsl`
// indexes it by id instead of decoding a fact already spent on every fragment
// of the quad it belongs to. `new_instance_buffer` below gains the same flag,
// for the ground pass — step 7.
pub(crate) fn new_static_instance_buffer(device: &wgpu::Device, quads: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("static instances"),
        size: quads * SpriteQuad::STRIDE,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Create an RGBA8 texture, fill it, and hand back a view of it.
///
/// Not necessarily square: every sprite atlas here is, but [`HueRamp`] is
/// [`HueRamp::width`] wide by however many hues were read, and giving this its
/// own two dimensions is simpler than a wrapper that pretends it is square.
///
/// `Rgba8Unorm` and never `…Srgb`: the atlas holds the file's own bytes and a
/// gamma conversion here would mean the pixel that went in is not the pixel that
/// comes out — see the crate docs.
pub(crate) fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixels,
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
    texture
}

/// Write a band of rows into an atlas texture already on the device.
///
/// `rows` indexes the atlas, and `pixels` is the *whole* atlas — the band is cut
/// out of it here, because `write_texture` wants a slice that starts at the
/// first texel it will write and the caller should not have to do that
/// arithmetic twice.
///
/// An empty or out-of-range band writes nothing rather than failing: the atlas
/// that produced the range and the texture it is written into are the same size
/// by construction, so a range outside it is a bug in this crate and not
/// something a frame should die of.
pub(crate) fn write_rows(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    pixels: &[u8],
    rows: std::ops::Range<u32>,
) {
    let width = texture.width();
    let height = texture.height();
    let (top, bottom) = (rows.start.min(height), rows.end.min(height));
    if top >= bottom {
        return;
    }
    let stride = width as usize * 4;
    let from = top as usize * stride;
    let to = bottom as usize * stride;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y: top, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        &pixels[from..to],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(bottom - top),
        },
        wgpu::Extent3d {
            width,
            height: bottom - top,
            depth_or_array_layers: 1,
        },
    );
}

// `STORAGE` alongside `VERTEX`, the same reason `new_static_instance_buffer`
// carries it: `docs/gbuffer.md` step 7 gives `blit.wgsl`/`select.wgsl`
// `ground_instances[id]`, read off this same buffer bound a second time — see
// `GroundRenderer::instances_buffer`.
fn new_instance_buffer(device: &wgpu::Device, quads: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ground instances"),
        size: quads * GroundQuad::STRIDE,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
